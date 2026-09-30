//! Co-change graph (REQ-206 in `.specs/features/git-integration/spec.md`):
//! files that tend to appear together in the same commits, over a bounded
//! window (default: last 500 commits or 6 months, whichever is smaller).
//!
//! Bounded on both axes (REQ-903 in `.specs/features/performance-guard/spec.md`):
//! a commit touching more than [`CoChangeWindow::max_files_per_commit`] files
//! (bulk renames, formatting passes, initial imports) is ignored as noise, and
//! each file keeps at most [`CoChangeWindow::max_pairs_per_file`] distinct
//! partners, favouring the most recent commits. Without the caps one
//! 800-file commit alone produced ~640k edges.
//!
//! Implemented as a method on [`GitSource`] (not a free function taking
//! `&GitSource`, as `.specs/features/git-integration/tasks.md` sketched) so
//! that the "every `gix` call goes through `GitSource`" boundary from
//! REQ-201 holds without exposing `GitSource`'s internal `gix::Repository`.

use std::collections::{HashMap, HashSet};
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
    /// Commits touching more files than this contribute no edges at all.
    pub max_files_per_commit: usize,
    /// Distinct co-change partners kept per file (newest commits win).
    pub max_pairs_per_file: usize,
}

pub const DEFAULT_MAX_FILES_PER_COMMIT: usize = 200;
pub const DEFAULT_MAX_PAIRS_PER_FILE: usize = 50;

impl Default for CoChangeWindow {
    fn default() -> Self {
        Self {
            max_commits: 500,
            max_age: Duration::from_secs(60 * 60 * 24 * 30 * 6), // ~6 months
            max_files_per_commit: DEFAULT_MAX_FILES_PER_COMMIT,
            max_pairs_per_file: DEFAULT_MAX_PAIRS_PER_FILE,
        }
    }
}

impl CoChangeWindow {
    /// Defaults, overridden by `COCHANGE_MAX_FILES` / `COCHANGE_MAX_PAIRS`
    /// when set to a positive integer.
    pub fn from_env() -> Self {
        let read = |name: &str, default: usize| {
            std::env::var(name)
                .ok()
                .and_then(|v| v.trim().parse::<usize>().ok())
                .filter(|n| *n > 0)
                .unwrap_or(default)
        };
        Self {
            max_files_per_commit: read("COCHANGE_MAX_FILES", DEFAULT_MAX_FILES_PER_COMMIT),
            max_pairs_per_file: read("COCHANGE_MAX_PAIRS", DEFAULT_MAX_PAIRS_PER_FILE),
            ..Self::default()
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
        let head_id = self.head_id_attached()?;
        let cutoff_seconds = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
            - window.max_age.as_secs() as i64;

        let mut seen_pairs: HashSet<(StableId, StableId)> = HashSet::new();
        let mut partners: HashMap<StableId, usize> = HashMap::new();
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
            if node_ids.len() < 2 || node_ids.len() > window.max_files_per_commit {
                continue;
            }

            for i in 0..node_ids.len() {
                for j in (i + 1)..node_ids.len() {
                    let (a, b) = (node_ids[i], node_ids[j]);
                    if seen_pairs.contains(&(a, b)) || seen_pairs.contains(&(b, a)) {
                        continue;
                    }
                    // Both directions are added together, so the cap is
                    // checked on both endpoints to keep the graph symmetric.
                    let a_full = partners.get(&a).copied().unwrap_or(0) >= window.max_pairs_per_file;
                    let b_full = partners.get(&b).copied().unwrap_or(0) >= window.max_pairs_per_file;
                    if a_full || b_full {
                        continue;
                    }
                    seen_pairs.insert((a, b));
                    *partners.entry(a).or_default() += 1;
                    *partners.entry(b).or_default() += 1;
                    for (from, to) in [(a, b), (b, a)] {
                        edges.push(EdgeMutation::Upsert {
                            id: stable_edge_id(&from, &to),
                            from,
                            to,
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
            ..CoChangeWindow::default()
        };
        let edges = source.co_change_edges(&window).unwrap();
        assert!(edges.is_empty(), "the co-changing commit is outside the 1-commit window");
    }

    fn write_many(dir: &Path, prefix: &str, count: usize, content: &str) {
        for i in 0..count {
            write(dir, &format!("{prefix}{i:03}.md"), content);
        }
    }

    fn edge_count(source: &GitSource, window: &CoChangeWindow) -> usize {
        source.co_change_edges(window).unwrap().len()
    }

    #[test]
    fn commit_over_the_file_cap_is_ignored_and_at_the_cap_is_kept() {
        let window = CoChangeWindow { max_pairs_per_file: usize::MAX, ..CoChangeWindow::default() };

        let at_cap = init_repo();
        write_many(at_cap.path(), "f", 200, "v1");
        commit(at_cap.path(), "chore: exactly at the cap");
        let source = GitSource::open(at_cap.path()).unwrap();
        assert_eq!(edge_count(&source, &window), 200 * 199, "every ordered pair of 200 files");

        let over = init_repo();
        write_many(over.path(), "f", 201, "v1");
        commit(over.path(), "chore: one file over the cap");
        let source = GitSource::open(over.path()).unwrap();
        assert_eq!(edge_count(&source, &window), 0, "201-file commit is treated as noise");
    }

    #[test]
    fn each_file_keeps_at_most_max_pairs_partners_favouring_recent_commits() {
        let dir = init_repo();
        // hub.md changes together with 6 different files, one per commit.
        for i in 0..6 {
            write(dir.path(), "hub.md", &format!("hub{i}"));
            write(dir.path(), &format!("p{i}.md"), "x");
            commit(dir.path(), &format!("chore: hub with p{i}"));
        }
        let source = GitSource::open(dir.path()).unwrap();
        let window = CoChangeWindow { max_pairs_per_file: 3, ..CoChangeWindow::default() };
        let edges = source.co_change_edges(&window).unwrap();

        let hub = file_node_id("hub.md");
        let partners_of_hub: Vec<StableId> = edges
            .iter()
            .filter_map(|e| match e {
                EdgeMutation::Upsert { from, to, .. } if *from == hub => Some(*to),
                _ => None,
            })
            .collect();
        assert_eq!(partners_of_hub.len(), 3);
        for recent in ["p5.md", "p4.md", "p3.md"] {
            assert!(partners_of_hub.contains(&file_node_id(recent)), "{recent} is recent, must be kept");
        }
        assert!(!partners_of_hub.contains(&file_node_id("p0.md")), "oldest partner is dropped");
    }

    #[test]
    fn env_overrides_are_respected_and_invalid_values_fall_back() {
        // Single test touching the environment: no other test reads these vars.
        unsafe {
            std::env::set_var("COCHANGE_MAX_FILES", "5");
            std::env::set_var("COCHANGE_MAX_PAIRS", "not-a-number");
        }
        let window = CoChangeWindow::from_env();
        unsafe {
            std::env::remove_var("COCHANGE_MAX_FILES");
            std::env::remove_var("COCHANGE_MAX_PAIRS");
        }
        assert_eq!(window.max_files_per_commit, 5);
        assert_eq!(window.max_pairs_per_file, DEFAULT_MAX_PAIRS_PER_FILE);
    }
}
