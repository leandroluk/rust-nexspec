//! Co-change graph (REQ-206 in `.specs/features/git-integration/spec.md`):
//! files that tend to appear together in the same commits, over a bounded
//! window (default: last 500 commits or 6 months, whichever is smaller).
//!
//! Implemented as a method on [`GitSource`] (not a free function taking
//! `&GitSource`, as `.specs/features/git-integration/tasks.md` sketched) so
//! that the "every `gix` call goes through `GitSource`" boundary from
//! REQ-201 holds without exposing `GitSource`'s internal `gix::Repository`.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use gix::object::tree::diff::Change;

use crate::git::source::{GitError, GitSource, op_err};
use crate::graph::edge::EdgeType;
use crate::graph::node::file_node_id;
use crate::sync::mutation::{EdgeMutation, StableId};

#[derive(Debug, Clone, Copy)]
pub struct CoChangeWindow {
    pub max_commits: usize,
    pub max_age: Duration,
}

impl Default for CoChangeWindow {
    fn default() -> Self {
        Self {
            max_commits: 500,
            max_age: Duration::from_secs(60 * 60 * 24 * 30 * 6), // ~6 months
        }
    }
}

fn stable_edge_id(from: &StableId, to: &StableId) -> StableId {
    let mut bytes = Vec::with_capacity(65);
    bytes.extend_from_slice(b"co-changes:");
    bytes.extend_from_slice(from);
    bytes.extend_from_slice(to);
    *blake3::hash(&bytes).as_bytes()
}

impl GitSource {
    /// `CoChanges` edges (both directions) between every pair of paths that
    /// changed together in the same commit, within `window`. Edges reference
    /// [`file_node_id`] for each path — deterministic from the path alone,
    /// so no lookup table is needed and any caller (including
    /// `sync_orchestrator`) derives the same id independently.
    pub fn co_change_edges(&self, window: &CoChangeWindow) -> Result<Vec<EdgeMutation>, GitError> {
        let head_id = self.repo.head_id().map_err(op_err)?;
        let cutoff_seconds = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
            - window.max_age.as_secs() as i64;

        let mut seen_pairs: HashSet<(StableId, StableId)> = HashSet::new();
        let mut edges = Vec::new();

        let walk = head_id.ancestors().all().map_err(op_err)?;
        for (count, info) in walk.enumerate() {
            if count >= window.max_commits {
                break;
            }
            let info = info.map_err(op_err)?;
            let commit = self.repo.find_commit(info.id).map_err(op_err)?;
            let commit_time = commit.time().map_err(op_err)?;
            if commit_time.seconds < cutoff_seconds {
                break; // walk is newest-first; once too old, everything after is too
            }

            let parent_id = info.parent_ids.first().copied();
            let changed_paths = self.changed_paths_for_commit(&commit, parent_id)?;

            let node_ids: Vec<StableId> = changed_paths
                .iter()
                .map(|p| file_node_id(&p.to_string_lossy()))
                .collect();
            if node_ids.len() < 2 {
                continue;
            }

            for i in 0..node_ids.len() {
                for j in 0..node_ids.len() {
                    if i == j {
                        continue;
                    }
                    let pair = (node_ids[i], node_ids[j]);
                    if seen_pairs.insert(pair) {
                        edges.push(EdgeMutation::Upsert {
                            id: stable_edge_id(&pair.0, &pair.1),
                            from: pair.0,
                            to: pair.1,
                            edge_type: EdgeType::CoChanges.to_code(),
                            payload: vec![],
                        });
                    }
                }
            }
        }

        Ok(edges)
    }

    fn changed_paths_for_commit(
        &self,
        commit: &gix::Commit<'_>,
        parent_id: Option<gix::ObjectId>,
    ) -> Result<Vec<PathBuf>, GitError> {
        let tree = commit.tree().map_err(op_err)?;

        let Some(parent_id) = parent_id else {
            // Root commit: every blob in its tree "changed" (it's all new).
            let mut paths = Vec::new();
            for entry in tree.traverse().breadthfirst.files().map_err(op_err)? {
                if entry.mode.is_blob() {
                    paths.push(PathBuf::from(entry.filepath.to_string()));
                }
            }
            return Ok(paths);
        };

        let parent_tree = self
            .repo
            .find_commit(parent_id)
            .map_err(op_err)?
            .tree()
            .map_err(op_err)?;

        let mut paths = Vec::new();
        parent_tree
            .changes()
            .map_err(op_err)?
            .options(|o| {
                o.track_path();
                o.track_rewrites(None);
            })
            .for_each_to_obtain_tree(&tree, |change| {
                let (location, entry_mode) = match change {
                    Change::Addition {
                        location,
                        entry_mode,
                        ..
                    }
                    | Change::Deletion {
                        location,
                        entry_mode,
                        ..
                    }
                    | Change::Modification {
                        location,
                        entry_mode,
                        ..
                    } => (location, entry_mode),
                    Change::Rewrite { .. } => unreachable!("track_rewrites(None) disables this variant"),
                };
                if entry_mode.is_blob() {
                    paths.push(PathBuf::from(location.to_string()));
                }
                Ok(std::ops::ControlFlow::Continue(()))
            })
            .map_err(op_err)?;

        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::source::GitSource;
    use std::path::Path;
    use std::process::Command;
    use tempfile::TempDir;

    fn init_repo() -> TempDir {
        let dir = TempDir::new().unwrap();
        for args in [
            vec!["init", "--quiet", "--initial-branch=main"],
            vec!["config", "user.email", "fixture@nexspec.test"],
            vec!["config", "user.name", "Fixture"],
            vec!["config", "commit.gpgsign", "false"],
        ] {
            assert!(Command::new("git").args(&args).current_dir(dir.path()).status().unwrap().success());
        }
        dir
    }

    fn write(dir: &Path, rel: &str, content: &str) {
        std::fs::write(dir.join(rel), content).unwrap();
    }

    fn commit(dir: &Path, msg: &str) {
        assert!(Command::new("git").args(["add", "-A"]).current_dir(dir).status().unwrap().success());
        assert!(Command::new("git").args(["commit", "--quiet", "-m", msg]).current_dir(dir).status().unwrap().success());
    }

    #[test]
    fn files_changed_together_produce_bidirectional_edges() {
        let dir = init_repo();
        write(dir.path(), "a.md", "a1");
        write(dir.path(), "b.md", "b1");
        commit(dir.path(), "chore: init"); // root commit, a+b together

        write(dir.path(), "a.md", "a2");
        write(dir.path(), "b.md", "b2");
        commit(dir.path(), "chore: touch a and b again"); // co-change again

        write(dir.path(), "c.md", "c1");
        commit(dir.path(), "chore: unrelated c"); // no co-change here

        let source = GitSource::open(dir.path()).unwrap();
        let edges = source.co_change_edges(&CoChangeWindow::default()).unwrap();

        let a = file_node_id("a.md");
        let b = file_node_id("b.md");
        let c = file_node_id("c.md");
        let has = |from: StableId, to: StableId| {
            edges.iter().any(|e| matches!(e, EdgeMutation::Upsert { from: f, to: t, .. } if *f == from && *t == to))
        };
        assert!(has(a, b), "a->b co-change edge expected");
        assert!(has(b, a), "b->a co-change edge expected");
        assert!(!has(a, c), "c never co-changed with a");
        assert!(!has(b, c), "c never co-changed with b");
    }

    #[test]
    fn narrow_commit_window_excludes_older_commits() {
        let dir = init_repo();
        write(dir.path(), "a.md", "a1");
        write(dir.path(), "b.md", "b1");
        commit(dir.path(), "chore: old co-change"); // commit 0 (oldest)
        write(dir.path(), "z.md", "z1");
        commit(dir.path(), "chore: unrelated newer commit"); // commit 1 (newest)

        let source = GitSource::open(dir.path()).unwrap();
        let window = CoChangeWindow {
            max_commits: 1, // only the newest commit is visited
            max_age: Duration::from_secs(u64::MAX / 2),
        };
        let edges = source.co_change_edges(&window).unwrap();
        assert!(edges.is_empty(), "the co-changing commit is outside the 1-commit window");
    }
}
