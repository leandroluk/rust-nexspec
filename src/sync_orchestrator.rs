//! [`SyncOrchestrator`] — ties `GitSource`'s diff to `markdown::extract()`
//! and a single `Coordinator::stage()` call per cycle (REQ-205 in
//! `.specs/features/git-integration/spec.md`). Lives outside `sync::`/
//! `graph::` on purpose — it's the first component that depends on both,
//! a composition-root role those modules deliberately don't take on
//! themselves. See `.specs/features/git-integration/design.md`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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

/// Wall-clock time per phase of one sync cycle, plus what was staged
/// (REQ-908). `diff` includes the working-tree dirty scan; `markdown` and
/// `code` are the two extraction passes.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PhaseTimings {
    pub diff: Duration,
    pub markdown: Duration,
    pub code: Duration,
    pub co_change: Duration,
    pub stage: Duration,
    pub nodes: usize,
    pub edges: usize,
    pub co_change_edges: usize,
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
    pub timings: PhaseTimings,
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
        let mut timings = PhaseTimings::default();
        let started = Instant::now();
        let since = self.version.last_indexed_commit()?;
        let diff = self.git.diff_since(since)?;
        timings.diff = started.elapsed();

        let mut combined = MutationSet::default();
        let markdown_started = Instant::now();

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
            if !is_noise_path(path) {
                combined.nodes.push(file_node_mutation(path));
            }
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
        let mut scan_time = Duration::ZERO;
        let mut files_dirty = 0usize;
        let mut dirty_paths: Vec<PathBuf> = Vec::new();
        // `is_dirty()` ignores untracked files, so gate on the path list
        // itself: a brand-new (uncommitted) spec must still be indexed.
        if let Some(root) = self.git.work_dir() {
            let scan_started = Instant::now();
            let mut candidates = self.git.dirty_paths()?;
            // The engine's own artifacts are never source material, even in
            // a repo that forgot to git-ignore them.
            candidates.retain(|path| !is_engine_artifact(path));
            dirty_paths = self.dirty_cache.scan(root, &candidates);
            scan_time = scan_started.elapsed();
            timings.diff += scan_time;
            for path in &dirty_paths {
                if is_markdown(path)
                    && let Ok(text) = std::fs::read_to_string(root.join(path))
                {
                    let extracted = markdown::extract(&text);
                    combined.nodes.extend(extracted.nodes);
                    combined.edges.extend(extracted.edges);
                    combined.docs.extend(extracted.docs);
                }
                if !is_noise_path(path) {
                    combined.nodes.push(file_node_mutation(path));
                }
            }
            files_dirty = dirty_paths.len();
        }

        // Pass 2 (REQ-306): code files, now that Markdown-derived REQ/ADR
        // nodes from this same cycle are known.
        timings.markdown = markdown_started.elapsed().saturating_sub(scan_time);
        let code_started = Instant::now();
        let known_markers = known_markers_from(&combined.nodes);
        // Blob reads stay sequential (`gix::Repository` is not `Sync`); the
        // Tree-sitter parsing itself runs in parallel via `code::extract_all`.
        let mut code_files: Vec<(PathBuf, String, code::Language)> = Vec::new();
        for path in diff.added.iter().chain(diff.modified.iter()) {
            if let Some(language) = code::Language::from_extension(path)
                && let Some(bytes) = self.git.read_blob_at_head(path)?
            {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                code_files.push((path.clone(), text, language));
            }
        }
        for path in &dirty_paths {
            if let Some(language) = code::Language::from_extension(path)
                && let Some(root) = self.git.work_dir()
                && let Ok(text) = std::fs::read_to_string(root.join(path))
            {
                code_files.push((path.clone(), text, language));
            }
        }
        let extracted = code::extract_all(&code_files, &known_markers)?;
        combined.nodes.extend(extracted.nodes);
        combined.edges.extend(extracted.edges);
        combined.docs.extend(extracted.docs);
        timings.code = code_started.elapsed();

        // Co-change edges depend only on commit history: recompute them when
        // HEAD moved (or on the first sync), not on every no-op cycle -- the
        // full pair set is ~O(files^2) per large commit.
        let history_changed = since.is_none()
            || !(diff.added.is_empty() && diff.modified.is_empty() && diff.deleted.is_empty());
        let co_change_started = Instant::now();
        if history_changed {
            let co_change = self.git.co_change_edges(&CoChangeWindow::from_env())?;
            timings.co_change_edges = co_change.len();
            combined.edges.extend(co_change);
        }
        timings.co_change = co_change_started.elapsed();

        let has_changes =
            !combined.nodes.is_empty() || !combined.edges.is_empty() || !combined.docs.is_empty();
        timings.nodes = combined.nodes.len();
        timings.edges = combined.edges.len();
        let stage_started = Instant::now();
        let target_version = if has_changes {
            Some(self.coordinator.stage(combined)?)
        } else {
            None
        };
        timings.stage = stage_started.elapsed();

        self.version
            .set_last_indexed_commit(self.git.head_commit_oid()?)?;

        Ok(SyncReport {
            target_version,
            files_added: diff.added.len(),
            files_modified: diff.modified.len(),
            files_deleted: diff.deleted.len(),
            files_dirty,
            timings,
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

/// Directories the engine writes into the repository (`.specs/.index/`,
/// downloaded `.models/`): touching them must not make the tree look dirty.
fn is_engine_artifact(path: &Path) -> bool {
    path.starts_with(".specs/.index") || path.starts_with(".models")
}

/// Files that are not source material: images, fonts, archives, media and
/// machine-generated lockfiles. They get no `File` node, so they cannot show
/// up in search results (a slide PNG ranked in a benchmark run; lockfiles
/// match every dependency name).
fn is_noise_path(path: &Path) -> bool {
    const NOISE_EXTENSIONS: &[&str] = &[
        "png", "jpg", "jpeg", "gif", "webp", "ico", "bmp", "tiff", "svg", "pdf", "woff", "woff2", "ttf", "otf", "eot",
        "zip", "gz", "tgz", "bz2", "xz", "7z", "rar", "jar", "exe", "dll", "so", "dylib", "bin", "wasm", "onnx", "mp3",
        "mp4", "mov", "wav", "avi", "webm", "sqlite", "db", "lock",
    ];
    const NOISE_FILE_NAMES: &[&str] = &["package-lock.json", "pnpm-lock.yaml", "yarn.lock", "Cargo.lock", "composer.lock"];
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    if NOISE_FILE_NAMES.contains(&name) {
        return true;
    }
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| NOISE_EXTENSIONS.iter().any(|n| n.eq_ignore_ascii_case(ext)))
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
