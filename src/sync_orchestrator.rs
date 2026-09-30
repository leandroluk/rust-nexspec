//! [`SyncOrchestrator`] — ties `GitSource`'s diff to `markdown::extract()`
//! and a single `Coordinator::stage()` call per cycle (REQ-205 in
//! `.specs/features/git-integration/spec.md`). Lives outside `sync::`/
//! `graph::` on purpose — it's the first component that depends on both,
//! a composition-root role those modules deliberately don't take on
//! themselves. See `.specs/features/git-integration/design.md`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::code::{self, CodeError};
use crate::domain::{self, graph::DomainInput};
use crate::git::cochange::CoChangeWindow;
use crate::git::dirty_cache::DirtyCache;
use crate::git::source::{GitError, GitSource};
use crate::graph::csr::Csr;
use crate::graph::edge::EdgeType;
use crate::graph::markdown;
use crate::graph::node::{NodePayload, file_node_id};
use crate::sync::coordinator::Coordinator;
use crate::sync::mutation::{EdgeMutation, MutationSet, NodeMutation, StableId};
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
    /// The domain pass (Fase 14): schema, manifests, entity links. Zero when it did not run.
    pub domain: Duration,
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
    /// Read access to the committed graph, used to find what a file used to
    /// contribute so stale symbols and edges can be removed (REQ-706). Without
    /// it the orchestrator only ever adds.
    csr: Option<Arc<Csr>>,
    /// Ids of the domain nodes already indexed (Fase 14), asked for only when the domain pass runs.
    domain_state: Option<DomainState<'a>>,
    /// Run the domain pass even when nothing it reads changed in git (after `extract --postgres`).
    force_domain: bool,
}

type DomainState<'a> = Box<dyn Fn() -> HashSet<StableId> + 'a>;

impl<'a> SyncOrchestrator<'a> {
    pub fn new(git: GitSource, coordinator: Coordinator<'a>, version: VersionPointer<'a>) -> Self {
        Self {
            git,
            coordinator,
            version,
            dirty_cache: DirtyCache::new(),
            csr: None,
            domain_state: None,
            force_domain: false,
        }
    }

    /// Runs the domain pass on the next cycle whatever changed.
    pub fn force_domain_pass(mut self) -> Self {
        self.force_domain = true;
        self
    }

    /// Lets the domain pass remove tables, columns and packages that disappeared: `existing` returns
    /// the ids of the domain nodes currently in the index.
    pub fn with_domain_state(mut self, existing: impl Fn() -> HashSet<StableId> + 'a) -> Self {
        self.domain_state = Some(Box::new(existing));
        self
    }

    /// Lets the orchestrator reconcile changed files against the graph.
    pub fn with_csr(mut self, csr: Arc<Csr>) -> Self {
        self.csr = Some(csr);
        self
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
        // A path can be both committed and dirty: the working-tree version
        // (pushed last) is the current one.
        let mut latest: HashMap<PathBuf, usize> = HashMap::new();
        for (i, (path, _, _)) in code_files.iter().enumerate() {
            latest.insert(path.clone(), i);
        }
        let code_files: Vec<(PathBuf, String, code::Language)> = code_files
            .into_iter()
            .enumerate()
            .filter(|(i, (path, _, _))| latest[path] == *i)
            .map(|(_, f)| f)
            .collect();
        let mut per_file = code::extract_each(&code_files, &known_markers)?;
        // Cross-file dependency edges (Fase 7): resolve each file's imports
        // against the tracked files and link uses to declared symbols.
        if per_file.iter().any(|(_, f)| !f.facts.imports.is_empty()) {
            let mut tracked: Vec<String> = self
                .git
                .tracked_paths_at_head()?
                .iter()
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .collect();
            tracked.extend(code_files.iter().map(|(p, _, _)| p.to_string_lossy().replace('\\', "/")));
            tracked.extend(dirty_paths.iter().map(|p| p.to_string_lossy().replace('\\', "/")));
            let git = &self.git;
            let resolver = code::SpecifierResolver::new(tracked, |p| read_repo_text(git, p));
            let mut builder = code::DependencyBuilder::new(
                &resolver,
                Box::new(|p: &str| {
                    let language = code::Language::from_extension(Path::new(p))?;
                    read_repo_text(git, p).map(|text| (text, language))
                }),
            );
            for (path, file) in &per_file {
                builder.seed(&path.to_string_lossy(), file);
            }
            for (path, file) in per_file.iter_mut() {
                let edges = builder.edges_for(&path.to_string_lossy(), file);
                file.set.edges.extend(edges);
            }
        }
        let deleted_code: Vec<PathBuf> = diff
            .deleted
            .iter()
            .filter(|p| code::Language::from_extension(p).is_some())
            .cloned()
            .collect();
        if let Some(csr) = &self.csr {
            reconcile_code_files(csr, &per_file, &deleted_code, &mut combined);
        }
        for (_, file) in per_file {
            combined.nodes.extend(file.set.nodes);
            combined.edges.extend(file.set.edges);
            combined.docs.extend(file.set.docs);
        }
        timings.code = code_started.elapsed();

        // Pass 3 (Fase 14): database schema, manifests and entity links. The schema is cumulative, so the
        // pass reads every candidate and rebuilds the whole domain subgraph, but only when something it
        // depends on changed.
        let domain_started = Instant::now();
        let touched: Vec<String> = diff
            .added
            .iter()
            .chain(diff.modified.iter())
            .chain(diff.deleted.iter())
            .chain(dirty_paths.iter())
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();
        let entity_marker_changed = code_files.iter().any(|(_, text, _)| domain::orm::has_marker(text) || domain::http::has_call_marker(text))
            || diff.deleted.iter().any(|p| is_entity_candidate(&p.to_string_lossy()));
        if since.is_none() || self.force_domain || entity_marker_changed || touched.iter().any(|p| is_domain_path(p)) {
            let input = self.domain_input(&dirty_paths)?;
            let graph = domain::graph::build(&input);
            let (existing_nodes, existing_edges) = match (&self.domain_state, &self.csr) {
                (Some(existing), Some(csr)) => (
                    existing(),
                    csr.all_edges()
                        .iter()
                        // Annotations and similarity edges hang off domain nodes too, but they are not the domain pass's to remove.
                        .filter(|e| !matches!(e.edge_type, EdgeType::AnnotatedBy | EdgeType::SimilarTo) && e.context() != crate::graph::edge::EdgeContext::Annotation)
                        .map(|e| (e.id, e.from, e.to, e.edge_type == EdgeType::DependsOn))
                        .collect::<Vec<_>>(),
                ),
                _ => (HashSet::new(), Vec::new()),
            };
            let set = domain::graph::reconcile(&graph, &existing_nodes, &existing_edges);
            combined.nodes.extend(set.nodes);
            combined.edges.extend(set.edges);
        }
        timings.domain = domain_started.elapsed();

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

fn is_entity_candidate(path: &str) -> bool {
    [".ts", ".tsx", ".js", ".jsx", ".mts", ".cts", ".mjs", ".cjs", ".py", ".prisma"].iter().any(|e| path.ends_with(e))
}

/// Paths whose change can alter the domain subgraph (besides an entity marker in a code file).
fn is_domain_path(path: &str) -> bool {
    domain::liquibase::is_candidate(path) || domain::manifest::is_manifest(path) || path.ends_with(".prisma") || domain::http::is_openapi_file(path)
}

impl SyncOrchestrator<'_> {
    fn domain_input(&self, dirty_paths: &[PathBuf]) -> Result<DomainInput, SyncOrchestratorError> {
        collect_domain_input(&self.git, dirty_paths)
    }
}

/// Reads every file the domain pass needs: schema candidates, manifests and entity-looking code,
/// plus what the last `extract --postgres` saved.
pub fn collect_domain_input(git: &GitSource, dirty_paths: &[PathBuf]) -> Result<DomainInput, SyncOrchestratorError> {
    {
        let mut tracked: Vec<String> = git.tracked_paths_at_head()?.iter().map(|p| p.to_string_lossy().replace('\\', "/")).collect();
        tracked.extend(dirty_paths.iter().map(|p| p.to_string_lossy().replace('\\', "/")));
        tracked.sort();
        tracked.dedup();
        let mut input = DomainInput::default();
        for path in &tracked {
            if is_engine_artifact(Path::new(path)) {
                continue;
            }
            if domain::http::is_openapi_file(path) {
                if let Some(text) = read_repo_text(git, path) {
                    input.openapi_files.push((path.clone(), text));
                }
            } else if domain::liquibase::is_candidate(path) {
                if let Some(text) = read_repo_text(git, path) {
                    input.schema_files.push((path.clone(), text));
                }
            } else if domain::manifest::is_manifest(path) {
                if let Some(text) = read_repo_text(git, path) {
                    input.manifests.push((path.clone(), text));
                }
            } else if is_entity_candidate(path)
                && let Some(text) = read_repo_text(git, path)
            {
                if domain::http::has_call_marker(&text) && !path.ends_with(".prisma") {
                    let calls = domain::http::scan_client_calls(&text);
                    if !calls.is_empty() {
                        input.client_calls.push((path.clone(), calls));
                    }
                }
                if path.ends_with(".prisma") || domain::orm::has_marker(&text) {
                    input.entity_files.push((path.clone(), text));
                }
            }
        }
        input.tracked = tracked.into_iter().collect();
        input.live = git.work_dir().and_then(domain::live::load);
        Ok(input)
    }
}

/// Edge types a code file's extraction owns: when the file changes, the ones
/// it no longer produces must go (REQ-706).
fn is_code_owned_edge(edge_type: EdgeType) -> bool {
    matches!(edge_type, EdgeType::DefinedIn | EdgeType::Satisfies) || edge_type.is_dependency()
}

/// Removes what changed or deleted code files used to contribute and no longer
/// do: their old symbols, and the edges leaving those symbols and the file.
/// A deleted file also loses the dependency edges that pointed *at* it.
/// (Importers that did not change are not re-resolved: see design.md D5.)
fn reconcile_code_files(
    csr: &Csr,
    per_file: &[(PathBuf, code::ExtractedFile)],
    deleted: &[PathBuf],
    out: &mut MutationSet,
) {
    if per_file.is_empty() && deleted.is_empty() {
        return;
    }
    let edges = csr.all_edges();
    let mut symbols_of_file: HashMap<StableId, Vec<StableId>> = HashMap::new();
    let mut outgoing: HashMap<StableId, Vec<usize>> = HashMap::new();
    let mut incoming: HashMap<StableId, Vec<usize>> = HashMap::new();
    for (i, edge) in edges.iter().enumerate() {
        if edge.edge_type == EdgeType::DefinedIn {
            symbols_of_file.entry(edge.to).or_default().push(edge.from);
        }
        if is_code_owned_edge(edge.edge_type) {
            outgoing.entry(edge.from).or_default().push(i);
            if edge.edge_type.is_dependency() {
                incoming.entry(edge.to).or_default().push(i);
            }
        }
    }

    let mut removed_edges: HashSet<StableId> = HashSet::new();
    let mut remove_edge = |id: StableId, out: &mut MutationSet| {
        if removed_edges.insert(id) {
            out.edges.push(EdgeMutation::Remove { id });
        }
    };

    for (path, file) in per_file {
        let set = &file.set;
        let file_id = file_node_id(&path.to_string_lossy());
        let new_symbols: HashSet<StableId> = set
            .nodes
            .iter()
            .filter_map(|n| match n {
                NodeMutation::Upsert { id, .. } => Some(*id),
                NodeMutation::Remove { .. } => None,
            })
            .collect();
        let new_edges: HashSet<StableId> = set
            .edges
            .iter()
            .filter_map(|e| match e {
                EdgeMutation::Upsert { id, .. } => Some(*id),
                EdgeMutation::Remove { .. } => None,
            })
            .collect();
        let old_symbols = symbols_of_file.get(&file_id).cloned().unwrap_or_default();
        for source in old_symbols.iter().chain(std::iter::once(&file_id)) {
            for &i in outgoing.get(source).into_iter().flatten() {
                if !new_edges.contains(&edges[i].id) {
                    remove_edge(edges[i].id, out);
                }
            }
        }
        for symbol in old_symbols {
            if !new_symbols.contains(&symbol) {
                out.nodes.push(NodeMutation::Remove { id: symbol });
            }
        }
    }

    for path in deleted {
        let file_id = file_node_id(&path.to_string_lossy());
        let old_symbols = symbols_of_file.get(&file_id).cloned().unwrap_or_default();
        let gone: Vec<StableId> = old_symbols.iter().copied().chain(std::iter::once(file_id)).collect();
        for id in &gone {
            for &i in outgoing.get(id).into_iter().flatten() {
                remove_edge(edges[i].id, out);
            }
            for &i in incoming.get(id).into_iter().flatten() {
                remove_edge(edges[i].id, out);
            }
        }
        for symbol in old_symbols {
            out.nodes.push(NodeMutation::Remove { id: symbol });
        }
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

/// Text of a repository file: the working-tree copy when present (it is the
/// current one, committed or not), else the blob at `HEAD`.
fn read_repo_text(git: &GitSource, path: &str) -> Option<String> {
    if let Some(root) = git.work_dir()
        && let Ok(text) = std::fs::read_to_string(root.join(path))
    {
        return Some(text);
    }
    let bytes = git.read_blob_at_head(Path::new(path)).ok().flatten()?;
    String::from_utf8(bytes).ok()
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
