//! [`SyncOrchestrator`] — ties `GitSource`'s diff to `markdown::extract()`
//! and a single `Coordinator::stage()` call per cycle (REQ-205 in
//! `.specs/features/git-integration/spec.md`). Lives outside `sync::`/
//! `graph::` on purpose — it's the first component that depends on both,
//! a composition-root role those modules deliberately don't take on
//! themselves. See `.specs/features/git-integration/design.md`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::code::{self, CodeError};
use crate::git::cochange::CoChangeWindow;
use crate::git::dirty_cache::DirtyCache;
use crate::git::source::{GitError, GitSource};
use crate::graph::markdown;
use crate::graph::node::{NodePayload, file_node_id};
use crate::sync::coordinator::Coordinator;
use crate::sync::mutation::{MutationSet, NodeMutation, StableId};
use crate::sync::participant::SyncError;
use crate::sync::version::{VersionError, VersionPointer};

#[derive(Debug, thiserror::Error)]
pub enum SyncOrchestratorError {
    #[error("git error: {0}")]
    Git(#[from] GitError),
    #[error("sync error: {0}")]
    Sync(#[from] SyncError),
    #[error("version pointer error: {0}")]
    Version(#[from] VersionError),
    #[error("code extraction error: {0}")]
    Code(#[from] CodeError),
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncReport {
    /// `None` when the diff produced no mutations (nothing to stage).
    pub target_version: Option<u64>,
    pub files_added: usize,
    pub files_modified: usize,
    pub files_deleted: usize,
    /// Working-tree files with uncommitted changes processed this cycle
    /// (REQ-204) — independent of `files_added`/`files_modified`, which
    /// only reflect committed history.
    pub files_dirty: usize,
}

pub struct SyncOrchestrator<'a> {
    git: GitSource,
    coordinator: Coordinator<'a>,
    version: VersionPointer<'a>,
    dirty_cache: DirtyCache,
}

impl<'a> SyncOrchestrator<'a> {
    pub fn new(git: GitSource, coordinator: Coordinator<'a>, version: VersionPointer<'a>) -> Self {
        Self {
            git,
            coordinator,
            version,
            dirty_cache: DirtyCache::new(),
        }
    }

    /// Run one full incremental sync cycle: diff since `last_indexed_commit`,
    /// extract every changed `.md` file (pass 1) then every changed code
    /// file (pass 2 — REQ-306; runs after pass 1 so `@spec`/`@adr`
    /// annotations can resolve against the Markdown-derived requirement/ADR
    /// nodes from the same cycle, REQ-304), add co-change edges, stage
    /// everything in one `Coordinator::stage()` call, then advance
    /// `last_indexed_commit` — only after staging succeeds.
    pub fn run_once(&mut self) -> Result<SyncReport, SyncOrchestratorError> {
        let since = self.version.last_indexed_commit()?;
        let diff = self.git.diff_since(since)?;

        let mut combined = MutationSet::default();

        // Pass 1: Markdown (committed diff) + a File node for every touched path.
        for path in diff.added.iter().chain(diff.modified.iter()) {
            if is_markdown(path)
                && let Some(bytes) = self.git.read_blob_at_head(path)?
            {
                let text = String::from_utf8_lossy(&bytes);
                let extracted = markdown::extract(&text);
                combined.nodes.extend(extracted.nodes);
                combined.edges.extend(extracted.edges);
                combined.docs.extend(extracted.docs);
            }
            combined.nodes.push(file_node_mutation(path));
        }
        for path in &diff.deleted {
            combined
                .nodes
                .push(NodeMutation::Remove {
                    id: file_node_id(&path.to_string_lossy()),
                });
            // Entities extracted from this file's past content (REQ/TASK/ADR
            // nodes) are not individually removed here -- doing so requires
            // tracking which entities originated from which file, which is
            // out of scope for this task. Documented limitation, not a
            // silent correctness gap: those nodes simply go stale until a
            // future pass reconciles them.
        }

        // REQ-204: uncommitted working-tree changes also enter the sync,
        // independent of the committed-history diff above (a repo can have
        // no new commits but a dirty tree, or vice versa). Markdown pass.
        let mut files_dirty = 0usize;
        let mut dirty_paths: Vec<PathBuf> = Vec::new();
        if self.git.is_dirty()?
            && let Some(root) = self.git.work_dir()
        {
            let tracked = self.git.tracked_paths_at_head()?;
            dirty_paths = self.dirty_cache.scan(root, &tracked);
            for path in &dirty_paths {
                if is_markdown(path)
                    && let Ok(text) = std::fs::read_to_string(root.join(path))
                {
                    let extracted = markdown::extract(&text);
                    combined.nodes.extend(extracted.nodes);
                    combined.edges.extend(extracted.edges);
                    combined.docs.extend(extracted.docs);
                }
                combined.nodes.push(file_node_mutation(path));
            }
            files_dirty = dirty_paths.len();
        }

        // Pass 2 (REQ-306): code files, now that Markdown-derived REQ/ADR
        // nodes from this same cycle are known.
        let known_markers = known_markers_from(&combined.nodes);
        for path in diff.added.iter().chain(diff.modified.iter()) {
            if let Some(language) = code::Language::from_extension(path)
                && let Some(bytes) = self.git.read_blob_at_head(path)?
            {
                let text = String::from_utf8_lossy(&bytes);
                let extracted = code::extract(&text, language, path, &known_markers)?;
                combined.nodes.extend(extracted.nodes);
                combined.edges.extend(extracted.edges);
                combined.docs.extend(extracted.docs);
            }
        }
        for path in &dirty_paths {
            if let Some(language) = code::Language::from_extension(path)
                && let Some(root) = self.git.work_dir()
                && let Ok(text) = std::fs::read_to_string(root.join(path))
            {
                let extracted = code::extract(&text, language, path, &known_markers)?;
                combined.nodes.extend(extracted.nodes);
                combined.edges.extend(extracted.edges);
                combined.docs.extend(extracted.docs);
            }
        }

        combined
            .edges
            .extend(self.git.co_change_edges(&CoChangeWindow::default())?);

        let has_changes =
            !combined.nodes.is_empty() || !combined.edges.is_empty() || !combined.docs.is_empty();
        let target_version = if has_changes {
            Some(self.coordinator.stage(combined)?)
        } else {
            None
        };

        self.version
            .set_last_indexed_commit(self.git.head_commit_oid()?)?;

        Ok(SyncReport {
            target_version,
            files_added: diff.added.len(),
            files_modified: diff.modified.len(),
            files_deleted: diff.deleted.len(),
            files_dirty,
        })
    }
}

/// Marker (`"REQ-001"`, `"ADR-005"`, ...) -> stable id, scanned out of
/// already-extracted `Requirement`/`Adr` nodes — REQ-304's resolution table
/// for `code::extract`'s `@spec`/`@adr` annotations. A requirement/ADR not
/// present in *this* sync cycle's Markdown pass is simply unresolved (same
/// forward-reference limitation as `markdown::extract` itself).
fn known_markers_from(nodes: &[NodeMutation]) -> HashMap<String, StableId> {
    let mut map = HashMap::new();
    for m in nodes {
        let NodeMutation::Upsert { id, payload } = m else {
            continue;
        };
        let mut aligned = rkyv::util::AlignedVec::<16>::new();
        aligned.extend_from_slice(payload);
        let Ok(decoded) = rkyv::from_bytes::<NodePayload, rkyv::rancor::Error>(&aligned) else {
            continue;
        };
        match decoded {
            NodePayload::Requirement { title, .. } | NodePayload::Adr { title, .. } => {
                map.insert(title, *id);
            }
            _ => {}
        }
    }
    map
}

fn is_markdown(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("md")
}

fn file_node_mutation(path: &Path) -> NodeMutation {
    let path_str = path.to_string_lossy().to_string();
    let id = file_node_id(&path_str);
    let payload = NodePayload::File {
        path: path_str,
        source_hash: id,
    };
    let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&payload)
        .expect("NodePayload must always serialize")
        .to_vec();
    NodeMutation::Upsert { id, payload: bytes }
}
