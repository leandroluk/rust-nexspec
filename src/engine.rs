//! [`Engine`] — the composition root (Fase 6, REQ-601 in
//! `.specs/features/cli-mcp-server/spec.md`). Opens or creates
//! `.specs/.index/` for a repository and exposes every operation the CLI
//! (`src/bin/nexspec.rs`) and the MCP server (`src/mcp.rs`) need, so neither
//! duplicates business logic — both are thin callers of `Engine` methods.
//!
//! Every method that *writes* (`sync`, `resume`, `compact`) builds its
//! [`crate::sync::SyncParticipant`]s fresh inside the call; none of Fases
//! 0-4's participant types were designed to be kept alive across a whole
//! CLI process's lifetime (see design.md's Decision Log for why a
//! long-lived `Coordinator` field would be a self-referential struct).
//! Every read-only method uses `self.csr` directly or opens a short-lived
//! read handle.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use redb::Database;

use crate::code::{self, CodeError, Language};
use crate::git::cochange::CoChangeWindow;
use crate::git::{BlameHunk, GitError, GitSource, blame_symbol};
use crate::graph::csr::{Csr, CsrBase, CsrError, CsrParticipant};
use crate::graph::edge::EdgeType;
use crate::graph::node::{NodePayload, file_node_id};
use crate::hybrid::{expand, seed_discovery};
use crate::search::{SearchError, TantivyParticipant, TantivyQueryable, hex, search_text, unhex};
use crate::sync::coordinator::Coordinator;
use crate::sync::mutation::{NodeMutation, StableId};
use crate::sync::participant::{SyncError, SyncParticipant};
use crate::sync::redb_participant::RedbParticipant;
use crate::sync::lock::{LOCK_FILE_NAME, LockError, SyncLock};
use crate::sync::version::{VersionError, VersionPointer};
use crate::sync::wal::{Wal, WalError};
use crate::sync_orchestrator::{SyncOrchestrator, SyncOrchestratorError, SyncReport};
use crate::token::budget::{Budget, CharHeuristicTokenizer, Tier, TieredItem};
use crate::token::pruner::prune_symbol;
use crate::token::serializer::serialize;
use tantivy::schema::Value;
#[cfg(feature = "full")]
use crate::vector::{Embedder, HnswError, HnswParticipant, VectorError};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("redb error: {0}")]
    RedbCreate(#[from] redb::DatabaseError),
    #[error("csr error: {0}")]
    Csr(#[from] CsrError),
    #[error("sync error: {0}")]
    Sync(#[from] SyncError),
    #[error("version error: {0}")]
    Version(#[from] VersionError),
    #[error("wal error: {0}")]
    Wal(#[from] WalError),
    #[error("{0}")]
    Lock(#[from] LockError),
    #[error("git error: {0}")]
    Git(#[from] GitError),
    #[error("sync orchestrator error: {0}")]
    Orchestrator(#[from] SyncOrchestratorError),
    #[error("search error: {0}")]
    Search(#[from] SearchError),
    #[error("code extraction error: {0}")]
    Code(#[from] CodeError),
    #[cfg(feature = "full")]
    #[error("vector error: {0}")]
    Vector(#[from] VectorError),
    #[cfg(feature = "full")]
    #[error("hnsw error: {0}")]
    Hnsw(#[from] HnswError),
    #[error("codec error: {0}")]
    Codec(String),
    #[error("no node found for target {0:?}")]
    TargetNotFound(String),
}

fn decode_node_payload(bytes: &[u8]) -> Result<NodePayload, EngineError> {
    let mut aligned = rkyv::util::AlignedVec::<16>::new();
    aligned.extend_from_slice(bytes);
    rkyv::from_bytes::<NodePayload, rkyv::rancor::Error>(&aligned)
        .map_err(|e| EngineError::Codec(e.to_string()))
}

fn language_label(language: Language) -> &'static str {
    match language {
        Language::Rust => "rust",
        Language::Python => "python",
        Language::Go => "go",
        Language::JavaScript => "javascript",
        Language::TypeScript => "typescript",
    }
}

pub struct SearchHit {
    pub id: StableId,
    pub payload: NodePayload,
    pub score: f32,
}

#[derive(Default)]
pub struct SearchResult {
    pub hits: Vec<SearchHit>,
    /// Populated only when `max_tokens` was passed to [`Engine::search`]
    /// (REQ-605) — the pruned, budget-fitted, dense Markdown payload from
    /// the Fase 5 pipeline.
    pub markdown: Option<String>,
}

pub struct TraceHop {
    pub id: StableId,
    pub payload: NodePayload,
    pub depth: u8,
    pub edge_type: EdgeType,
    /// `true` when this hop was reached against the edge direction (the
    /// node points *at* the one being traced, e.g. the code or task that
    /// `Satisfies` a requirement).
    pub incoming: bool,
}

#[derive(Default)]
pub struct TraceResult {
    pub hops: Vec<TraceHop>,
}

pub struct ImpactedSymbol {
    pub id: StableId,
    pub name: String,
    pub dependants: Vec<StableId>,
}

#[derive(Default)]
pub struct DiffResult {
    pub changed_symbols: Vec<ImpactedSymbol>,
}

pub struct BlameResult {
    pub hunks: Vec<BlameHunk>,
    pub co_changed_files: Vec<String>,
}

/// Version of everything the index derives from the repository. Bump it
/// whenever extraction or edge semantics change so stale indexes rebuild
/// themselves instead of serving wrong answers. 2 = capped co-change edges.
pub const INDEX_FORMAT: u64 = 2;

pub struct Engine {
    db: Database,
    csr: Arc<Csr>,
    index_dir: PathBuf,
    repo_root: PathBuf,
    #[cfg(feature = "full")]
    embedder: Embedder,
    /// HNSW graph reused across searches of one `Engine` (building it is
    /// O(points)); keyed by the `sync_version` it was loaded at.
    #[cfg(feature = "full")]
    hnsw_cache: std::sync::Mutex<Option<(u64, Arc<HnswParticipant>)>>,
    /// Declared last so it is released after everything above is dropped
    /// (the database file must be closed before another process may open it).
    _lock: Option<SyncLock>,
}

impl Engine {
    /// Opens `.specs/.index/` under `index_dir`, creating whatever's
    /// missing (REQ-602: idempotent — an existing structure is opened, not
    /// reset).
    pub fn open(index_dir: &Path, repo_root: &Path) -> Result<Self, EngineError> {
        std::fs::create_dir_all(index_dir)?;

        // REQ-907: wait for another process using this index instead of
        // failing with redb's "Database already open".
        let lock = SyncLock::acquire(index_dir, SyncLock::timeout_from_env())?;
        let db = Self::open_current_format_db(index_dir)?;

        let csr_path = index_dir.join("edges.bin");
        if !csr_path.exists() {
            CsrBase::build(&[], &csr_path)?;
        }
        let csr = Arc::new(Csr::new(CsrBase::open(&csr_path)?));

        // Ensures the Tantivy index directory/files exist; the participant
        // itself is discarded immediately (see module docs -- participants
        // are always rebuilt per call, never kept as an `Engine` field).
        drop(TantivyParticipant::new(&index_dir.join("tantivy"))?);

        #[cfg(feature = "full")]
        let embedder = Embedder::new(
            repo_root.join(".models").join("model_quantized.onnx"),
            repo_root.join(".models").join("tokenizer.json"),
        );

        Ok(Self {
            db,
            csr,
            index_dir: index_dir.to_path_buf(),
            repo_root: repo_root.to_path_buf(),
            #[cfg(feature = "full")]
            embedder,
            #[cfg(feature = "full")]
            hnsw_cache: std::sync::Mutex::new(None),
            _lock: Some(lock),
        })
    }

    /// Opens `metadata.redb`, first discarding the derived index when
    /// it was written by an incompatible build (`INDEX_FORMAT`). The index is
    /// always regenerable from Git + specs, so the next `sync` rebuilds it.
    /// Only the contents of `index_dir` are ever removed.
    fn open_current_format_db(index_dir: &Path) -> Result<Database, EngineError> {
        let db_path = index_dir.join("metadata.redb");
        let db = Database::create(&db_path)?;
        let version = VersionPointer::new(&db);
        let has_data = version.current()? > 0 || version.last_indexed_commit()?.is_some();
        match version.index_format()? {
            Some(INDEX_FORMAT) => Ok(db),
            None if !has_data => {
                version.set_index_format(INDEX_FORMAT)?;
                Ok(db)
            }
            found => {
                eprintln!(
                    "nexspec: index format {} is incompatible with this build (expects {INDEX_FORMAT}); rebuilding {}",
                    found.map_or_else(|| "unknown".to_string(), |n| n.to_string()),
                    index_dir.display()
                );
                drop(db);
                // Everything but the lock file we are holding.
                for entry in std::fs::read_dir(index_dir)? {
                    let path = entry?.path();
                    if path.file_name().is_some_and(|n| n == LOCK_FILE_NAME) {
                        continue;
                    }
                    if path.is_dir() {
                        std::fs::remove_dir_all(&path)?;
                    } else {
                        std::fs::remove_file(&path)?;
                    }
                }
                let db = Database::create(&db_path)?;
                VersionPointer::new(&db).set_index_format(INDEX_FORMAT)?;
                Ok(db)
            }
        }
    }

    fn wal_path(&self) -> PathBuf {
        self.index_dir.join("sync.wal")
    }
    fn csr_path(&self) -> PathBuf {
        self.index_dir.join("edges.bin")
    }
    fn tantivy_dir(&self) -> PathBuf {
        self.index_dir.join("tantivy")
    }
    #[cfg(feature = "full")]
    fn hnsw_path(&self) -> PathBuf {
        self.index_dir.join("vectors.bin")
    }

    fn participants(&self) -> Result<Vec<Box<dyn SyncParticipant + '_>>, EngineError> {
        let redb_participant = RedbParticipant::new(&self.db);
        let csr_participant = CsrParticipant::new(Arc::clone(&self.csr), self.csr_path());
        let tantivy_participant = TantivyParticipant::new(&self.tantivy_dir())?;
        #[cfg_attr(not(feature = "full"), allow(unused_mut))]
        let mut participants: Vec<Box<dyn SyncParticipant + '_>> = vec![
            Box::new(redb_participant),
            Box::new(csr_participant),
            Box::new(tantivy_participant),
        ];
        #[cfg(feature = "full")]
        {
            let hnsw_participant = HnswParticipant::new(&self.hnsw_path())?;
            participants.push(Box::new(hnsw_participant));
        }
        Ok(participants)
    }

    /// REQ-603: one incremental sync cycle via [`SyncOrchestrator`].
    pub fn sync(&self) -> Result<SyncReport, EngineError> {
        let git = GitSource::open(&self.repo_root)?;
        let wal = Wal::open(self.wal_path())?;
        let coordinator = Coordinator::new(wal, VersionPointer::new(&self.db), self.participants()?);
        let mut orchestrator = SyncOrchestrator::new(git, coordinator, VersionPointer::new(&self.db));
        Ok(orchestrator.run_once()?)
    }

    /// REQ-603: deterministic crash recovery, replaying any WAL frame no
    /// participant fully applied yet.
    pub fn resume(&self) -> Result<(), EngineError> {
        let wal = Wal::open(self.wal_path())?;
        let coordinator = Coordinator::new(wal, VersionPointer::new(&self.db), self.participants()?);
        coordinator.resume()?;
        Ok(())
    }

    /// REQ-604: force CSR delta compaction outside the automatic threshold.
    pub fn compact(&self) -> Result<(), EngineError> {
        let participant = CsrParticipant::new(Arc::clone(&self.csr), self.csr_path());
        participant.compact_now()?;
        Ok(())
    }

    /// The HNSW participant for the current `sync_version`, loaded once and
    /// reused until a sync moves the version.
    #[cfg(feature = "full")]
    fn cached_hnsw(&self) -> Result<Arc<HnswParticipant>, EngineError> {
        let version = VersionPointer::new(&self.db).current()?;
        let mut cache = self.hnsw_cache.lock().unwrap();
        if let Some((cached_version, hnsw)) = cache.as_ref()
            && *cached_version == version
        {
            return Ok(Arc::clone(hnsw));
        }
        let hnsw = Arc::new(HnswParticipant::new(&self.hnsw_path())?);
        *cache = Some((version, Arc::clone(&hnsw)));
        Ok(hnsw)
    }

    /// REQ-605: hybrid search (BM25 always, HNSW when `full` and a model is
    /// available) + RRF fusion + 1-hop expansion. `max_tokens` opts into the
    /// Fase 5 pruning/budgeting/serialization pipeline.
    pub fn search(&self, query: &str, max_tokens: Option<u32>) -> Result<SearchResult, EngineError> {
        let tantivy = TantivyParticipant::new(&self.tantivy_dir())?;
        let bm25_ranked: Vec<StableId> = search_text(&tantivy, query, 20)?
            .iter()
            .filter_map(|doc| doc.get_first(tantivy.schema().id_field)?.as_str().and_then(unhex))
            .collect();

        #[cfg(feature = "full")]
        let hnsw_ranked: Vec<StableId> = match self.embedder.embed(query) {
            Ok(vector) => {
                let hnsw = self.cached_hnsw()?;
                hnsw.index().search(&vector, 20).into_iter().map(|(id, _)| id).collect()
            }
            Err(VectorError::ModelNotAvailable(_)) => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        #[cfg(not(feature = "full"))]
        let hnsw_ranked: Vec<StableId> = Vec::new();

        let fused = seed_discovery(&bm25_ranked, &hnsw_ranked);
        let seed_ids: Vec<StableId> = fused.iter().map(|(id, _)| *id).collect();
        let expanded = expand(
            &seed_ids,
            &self.csr,
            &[EdgeType::DependsOn, EdgeType::Satisfies, EdgeType::DefinedIn, EdgeType::Implements],
            1,
        );

        let redb = RedbParticipant::new(&self.db);
        let mut hits = Vec::new();
        for id in &expanded {
            let Some(bytes) = redb.get_node(id)? else { continue };
            let payload = decode_node_payload(&bytes)?;
            let score = fused.iter().find(|(fid, _)| fid == id).map_or(0.0, |(_, s)| *s);
            hits.push((*id, payload, score));
        }

        let markdown = match max_tokens {
            Some(budget_tokens) => {
                let git = GitSource::open(&self.repo_root)?;
                let items: Vec<TieredItem> = hits
                    .iter()
                    .enumerate()
                    .map(|(i, (id, payload, _))| {
                        let tier = if i == 0 {
                            Tier::Target
                        } else if seed_ids.contains(id) {
                            Tier::Seed
                        } else {
                            Tier::Dependency
                        };
                        let (text, lang) = self.render_payload(id, payload, &git);
                        let mut item = TieredItem::new(tier, text);
                        if let Some(lang) = lang {
                            item = item.with_lang(lang);
                        }
                        item
                    })
                    .collect();
                let fitted = Budget::with_default_margin(budget_tokens).fit(items, &CharHeuristicTokenizer);
                Some(serialize(&fitted))
            }
            None => None,
        };

        Ok(SearchResult {
            hits: hits.into_iter().map(|(id, payload, score)| SearchHit { id, payload, score }).collect(),
            markdown,
        })
    }

    /// Renders a node's payload as text for the Fase 5 serializer,
    /// AST-pruning `Symbol` bodies via their source file (found through the
    /// `DefinedIn` edge to a `File` node) when that file is resolvable;
    /// falls back to the bare symbol name otherwise (deleted file, unknown
    /// language, ...).
    fn render_payload(&self, id: &StableId, payload: &NodePayload, git: &GitSource) -> (String, Option<&'static str>) {
        match payload {
            NodePayload::Requirement { title, body, .. }
            | NodePayload::Task { title, body, .. }
            | NodePayload::Adr { title, body, .. } => (format!("{title}\n{body}"), Some("markdown")),
            NodePayload::DocSection { title, .. } => (title.clone(), Some("markdown")),
            NodePayload::File { path, .. } => (path.clone(), None),
            NodePayload::Symbol { name, line_start, line_end, .. } => {
                if let Some(pruned) = self.prune_symbol_source(id, *line_start, *line_end, git) {
                    return pruned;
                }
                (name.clone(), None)
            }
        }
    }

    fn prune_symbol_source(
        &self,
        symbol_id: &StableId,
        line_start: u32,
        line_end: u32,
        git: &GitSource,
    ) -> Option<(String, Option<&'static str>)> {
        let file_edge = self.csr.edges_from(symbol_id, EdgeType::DefinedIn).into_iter().next()?;
        let redb = RedbParticipant::new(&self.db);
        let file_bytes = redb.get_node(&file_edge.to).ok()??;
        let NodePayload::File { path, .. } = decode_node_payload(&file_bytes).ok()? else {
            return None;
        };
        let language = Language::from_extension(Path::new(&path))?;
        let blob = git.read_blob_at_head(Path::new(&path)).ok()??;
        let text = String::from_utf8_lossy(&blob);
        let pruned = prune_symbol(&text, language, line_start, line_end);
        Some((pruned, Some(language_label(language))))
    }

    /// REQ-606: deterministic topological trace from `target` (a 64-hex-char
    /// `StableId`, or free text resolved via a BM25 top-hit — exact-marker
    /// resolution is a possible refinement, not needed for this fase's
    /// scope) across `{Satisfies, DependsOn, DefinedIn, Implements}`.
    pub fn trace(&self, target: &str) -> Result<TraceResult, EngineError> {
        let root_id = self.resolve_target(target)?;

        let redb = RedbParticipant::new(&self.db);
        let mut hops = Vec::new();
        let mut visited: std::collections::HashSet<StableId> = [root_id].into_iter().collect();
        let mut frontier = vec![root_id];
        let edge_types = [EdgeType::Satisfies, EdgeType::DependsOn, EdgeType::DefinedIn, EdgeType::Implements];
        // Reverse index (target -> sources) for the semantic edge types only:
        // "what satisfies / depends on / implements this". `DefinedIn` is not
        // reversed (a file would fan out to every symbol it contains).
        let mut incoming: std::collections::HashMap<StableId, Vec<(StableId, EdgeType)>> =
            std::collections::HashMap::new();
        for edge in self.csr.all_edges() {
            if matches!(edge.edge_type, EdgeType::Satisfies | EdgeType::DependsOn | EdgeType::Implements) {
                incoming.entry(edge.to).or_default().push((edge.from, edge.edge_type));
            }
        }

        for depth in 1..=8u8 {
            if frontier.is_empty() {
                break;
            }
            let mut next_frontier = Vec::new();
            for node in &frontier {
                for edge_type in edge_types {
                    for edge in self.csr.edges_from(node, edge_type) {
                        if visited.insert(edge.to) {
                            if let Some(bytes) = redb.get_node(&edge.to)? {
                                let payload = decode_node_payload(&bytes)?;
                                hops.push(TraceHop { id: edge.to, payload, depth, edge_type, incoming: false });
                            }
                            next_frontier.push(edge.to);
                        }
                    }
                }
                for (from, edge_type) in incoming.get(node).into_iter().flatten() {
                    if visited.insert(*from) {
                        if let Some(bytes) = redb.get_node(from)? {
                            let payload = decode_node_payload(&bytes)?;
                            hops.push(TraceHop { id: *from, payload, depth, edge_type: *edge_type, incoming: true });
                        }
                        next_frontier.push(*from);
                    }
                }
            }
            frontier = next_frontier;
        }

        Ok(TraceResult { hops })
    }

    /// Resolves a CLI/MCP-supplied target string to a `StableId`: tries
    /// hex-decoding it directly first (the id itself), then falls back to
    /// the top BM25 hit for it as free text.
    fn resolve_target(&self, target: &str) -> Result<StableId, EngineError> {
        if let Some(id) = unhex(target) {
            return Ok(id);
        }
        let tantivy = TantivyParticipant::new(&self.tantivy_dir())?;
        let hit = search_text(&tantivy, target, 1)?
            .into_iter()
            .next()
            .and_then(|doc| doc.get_first(tantivy.schema().id_field)?.as_str().and_then(unhex));
        hit.ok_or_else(|| EngineError::TargetNotFound(target.to_string()))
    }

    /// REQ-607: AST-aware blame for `symbol_name` — resolves it, finds the
    /// file it's defined in (via `DefinedIn`), runs `git::blame_symbol`
    /// scoped to its line range, and adds files that co-change with that
    /// file (REQ-206's window, or unrestricted when `full_history`).
    pub fn blame(&self, symbol_name: &str, full_history: bool) -> Result<BlameResult, EngineError> {
        let symbol_id = self.resolve_target(symbol_name)?;
        let Some(NodePayload::Symbol { line_start, line_end, .. }) = self.node_payload(&symbol_id)? else {
            return Err(EngineError::TargetNotFound(symbol_name.to_string()));
        };
        let file_edge = self
            .csr
            .edges_from(&symbol_id, EdgeType::DefinedIn)
            .into_iter()
            .next()
            .ok_or_else(|| EngineError::TargetNotFound(symbol_name.to_string()))?;
        let Some(NodePayload::File { path, .. }) = self.node_payload(&file_edge.to)? else {
            return Err(EngineError::TargetNotFound(symbol_name.to_string()));
        };

        let git = GitSource::open(&self.repo_root)?;
        let hunks = blame_symbol(&git, Path::new(&path), line_start, line_end)?;

        let window = if full_history {
            CoChangeWindow {
                max_commits: usize::MAX,
                max_age: std::time::Duration::MAX,
                ..CoChangeWindow::from_env()
            }
        } else {
            CoChangeWindow::from_env()
        };
        let file_id = file_node_id(&path);
        let mut co_changed_files = Vec::new();
        for edge in git.co_change_edges(&window)? {
            if let crate::sync::mutation::EdgeMutation::Upsert { from, to, .. } = edge
                && from == file_id
                && let Some(NodePayload::File { path: other_path, .. }) = self.node_payload(&to)?
            {
                co_changed_files.push(other_path);
            }
        }

        Ok(BlameResult { hunks, co_changed_files })
    }

    /// REQ-608: structural impact analysis over the dirty working tree —
    /// changed symbols plus whoever `DependsOn` them (1 hop), no Tantivy/
    /// HNSW touched (lean by design).
    pub fn diff_staged(&self) -> Result<DiffResult, EngineError> {
        let git = GitSource::open(&self.repo_root)?;
        let mut changed_symbols: Vec<(StableId, String)> = Vec::new();

        if let Some(root) = git.work_dir() {
            for path in git.dirty_paths()? {
                let Some(language) = Language::from_extension(&path) else { continue };
                let head_bytes = git.read_blob_at_head(&path)?.unwrap_or_default();
                let Ok(working_bytes) = std::fs::read(root.join(&path)) else { continue };
                if head_bytes == working_bytes {
                    continue;
                }
                let Ok(text) = String::from_utf8(working_bytes) else { continue };
                let extracted = code::extract(&text, language, &path, &HashMap::new())?;
                for m in extracted.nodes {
                    if let NodeMutation::Upsert { id, payload } = m
                        && let Ok(NodePayload::Symbol { name, .. }) = decode_node_payload(&payload)
                    {
                        changed_symbols.push((id, name));
                    }
                }
            }
        }

        let all_edges = self.csr.all_edges();
        let changed_symbols = changed_symbols
            .into_iter()
            .map(|(id, name)| {
                let dependants: Vec<StableId> = all_edges
                    .iter()
                    .filter(|e| e.to == id && e.edge_type == EdgeType::DependsOn)
                    .map(|e| e.from)
                    .collect();
                ImpactedSymbol { id, name, dependants }
            })
            .collect();

        Ok(DiffResult { changed_symbols })
    }

    pub fn index_dir(&self) -> &Path {
        &self.index_dir
    }
    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }
    /// Read-only access to the shared CSR, for callers (CLI/MCP) that need
    /// to render a node id back to a human-readable hex string, etc.
    pub fn node_payload(&self, id: &StableId) -> Result<Option<NodePayload>, EngineError> {
        let redb = RedbParticipant::new(&self.db);
        match redb.get_node(id)? {
            Some(bytes) => Ok(Some(decode_node_payload(&bytes)?)),
            None => Ok(None),
        }
    }
}

/// Lowercase hex encoding of a `StableId`, for display in CLI/MCP output.
pub fn id_hex(id: &StableId) -> String {
    hex(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::mutation::{EdgeMutation, MutationSet};
    use std::process::Command;
    use tempfile::TempDir;

    /// Same fixture pattern as `git::cochange::tests::init_repo` — shells
    /// out to the real `git` CLI (test-only; production code never does).
    fn init_git_repo_with_markdown(dir: &Path) {
        for args in [
            vec!["init", "--quiet", "--initial-branch=main"],
            vec!["config", "user.email", "fixture@nexspec.test"],
            vec!["config", "user.name", "Fixture"],
            vec!["config", "commit.gpgsign", "false"],
        ] {
            assert!(Command::new("git").args(&args).current_dir(dir).status().unwrap().success());
        }
        let specs_dir = dir.join(".specs");
        std::fs::create_dir_all(&specs_dir).unwrap();
        std::fs::write(specs_dir.join("req.md"), "## Requirements\n- REQ-9001: engine test requirement\n").unwrap();
        assert!(Command::new("git").args(["add", "-A"]).current_dir(dir).status().unwrap().success());
        assert!(
            Command::new("git")
                .args(["commit", "--quiet", "-m", "initial commit"])
                .current_dir(dir)
                .status()
                .unwrap()
                .success()
        );
    }

    #[test]
    fn open_over_an_existing_index_dir_does_not_reset_it() {
        let repo_dir = TempDir::new().unwrap();
        init_git_repo_with_markdown(repo_dir.path());
        let index_dir = TempDir::new().unwrap();

        let engine = Engine::open(index_dir.path(), repo_dir.path()).unwrap();
        engine.sync().unwrap();
        let node_count_before = engine.search("engine test requirement", None).unwrap().hits.len();
        assert!(node_count_before > 0, "expected the synced requirement to be searchable");
        drop(engine); // release the redb file lock before reopening

        // Reopening must not wipe the index back to empty.
        let reopened = Engine::open(index_dir.path(), repo_dir.path()).unwrap();
        let node_count_after = reopened.search("engine test requirement", None).unwrap().hits.len();
        assert_eq!(node_count_after, node_count_before, "reopening must not lose previously synced data");
    }

    fn stored_index_format(index_dir: &Path) -> Option<u64> {
        let db = Database::create(index_dir.join("metadata.redb")).unwrap();
        VersionPointer::new(&db).index_format().unwrap()
    }

    #[test]
    fn fresh_index_records_the_current_format() {
        let repo_dir = TempDir::new().unwrap();
        init_git_repo_with_markdown(repo_dir.path());
        let index_dir = TempDir::new().unwrap();
        drop(Engine::open(index_dir.path(), repo_dir.path()).unwrap());
        assert_eq!(stored_index_format(index_dir.path()), Some(INDEX_FORMAT));
    }

    #[test]
    fn incompatible_index_is_discarded_and_rebuilt_by_the_next_sync() {
        let repo_dir = TempDir::new().unwrap();
        init_git_repo_with_markdown(repo_dir.path());
        let workspace = TempDir::new().unwrap();
        let index_dir = workspace.path().join("index");
        let sibling = workspace.path().join("graph.json");
        std::fs::write(&sibling, "keep me").unwrap();

        let engine = Engine::open(&index_dir, repo_dir.path()).unwrap();
        engine.sync().unwrap();
        drop(engine);

        // Simulate an index written by an older build: older format, stale file.
        {
            let db = Database::create(index_dir.join("metadata.redb")).unwrap();
            VersionPointer::new(&db).set_index_format(INDEX_FORMAT - 1).unwrap();
        }
        std::fs::write(index_dir.join("stale.bin"), "old").unwrap();

        let engine = Engine::open(&index_dir, repo_dir.path()).unwrap();
        assert!(!index_dir.join("stale.bin").exists(), "old index contents are discarded");
        assert_eq!(std::fs::read_to_string(&sibling).unwrap(), "keep me", "nothing outside index_dir is touched");
        assert_eq!(engine.search("engine test requirement", None).unwrap().hits.len(), 0);

        engine.sync().unwrap();
        assert!(
            !engine.search("engine test requirement", None).unwrap().hits.is_empty(),
            "the next sync rebuilds the index from Git"
        );
        drop(engine);
        assert_eq!(stored_index_format(&index_dir), Some(INDEX_FORMAT));
    }

    #[test]
    fn index_without_a_format_but_with_data_is_rebuilt() {
        let repo_dir = TempDir::new().unwrap();
        init_git_repo_with_markdown(repo_dir.path());
        let index_dir = TempDir::new().unwrap();
        let engine = Engine::open(index_dir.path(), repo_dir.path()).unwrap();
        engine.sync().unwrap();
        drop(engine);
        // Pre-format-tracking index: data present, no format key.
        {
            let db = Database::create(index_dir.path().join("metadata.redb")).unwrap();
            let tx = db.begin_write().unwrap();
            {
                let mut t = tx
                    .open_table(redb::TableDefinition::<&str, u64>::new("meta"))
                    .unwrap();
                t.remove("index_format").unwrap();
            }
            tx.commit().unwrap();
        }
        let engine = Engine::open(index_dir.path(), repo_dir.path()).unwrap();
        assert_eq!(engine.search("engine test requirement", None).unwrap().hits.len(), 0);
    }

    #[test]
    fn sync_then_search_finds_the_requirement() {
        let repo_dir = TempDir::new().unwrap();
        init_git_repo_with_markdown(repo_dir.path());
        let index_dir = TempDir::new().unwrap();

        let engine = Engine::open(index_dir.path(), repo_dir.path()).unwrap();
        engine.sync().unwrap();

        let result = engine.search("engine test requirement", None).unwrap();
        assert!(!result.hits.is_empty(), "expected the synced requirement to be findable");
    }

    #[test]
    fn compact_resets_delta_after_forcing_it() {
        let repo_dir = TempDir::new().unwrap();
        init_git_repo_with_markdown(repo_dir.path());
        let index_dir = TempDir::new().unwrap();
        let engine = Engine::open(index_dir.path(), repo_dir.path()).unwrap();
        engine.sync().unwrap();

        engine.compact().unwrap();
        assert_eq!(engine.csr.current_delta().len(), 0);
    }

    #[test]
    fn trace_follows_satisfies_edge_from_requirement_to_symbol() {
        let db_file = tempfile::NamedTempFile::new().unwrap();
        let db = redb::Database::create(db_file.path()).unwrap();
        let csr_file = tempfile::NamedTempFile::new().unwrap();
        CsrBase::build(&[], csr_file.path()).unwrap();
        let csr = Arc::new(Csr::new(CsrBase::open(csr_file.path()).unwrap()));
        let tantivy_dir = TempDir::new().unwrap();
        drop(TantivyParticipant::new(tantivy_dir.path()).unwrap());

        let req_id = [1u8; 32];
        let symbol_id = [2u8; 32];
        let req_payload = NodePayload::Requirement {
            title: "REQ-1".into(),
            source_hash: req_id,
            body: "body".into(),
        };
        let symbol_payload = NodePayload::Symbol {
            name: "handler".into(),
            source_hash: symbol_id,
            line_start: 0,
            line_end: 1,
        };
        let mut set = MutationSet::default();
        set.nodes.push(NodeMutation::Upsert {
            id: req_id,
            payload: rkyv::to_bytes::<rkyv::rancor::Error>(&req_payload).unwrap().to_vec(),
        });
        set.nodes.push(NodeMutation::Upsert {
            id: symbol_id,
            payload: rkyv::to_bytes::<rkyv::rancor::Error>(&symbol_payload).unwrap().to_vec(),
        });
        set.edges.push(EdgeMutation::Upsert {
            id: [9u8; 32],
            from: symbol_id,
            to: req_id,
            edge_type: EdgeType::Satisfies.to_code(),
            payload: vec![],
        });

        {
            let wal_file = tempfile::NamedTempFile::new().unwrap();
            let wal = Wal::open(wal_file.path()).unwrap();
            let version = VersionPointer::new(&db);
            let redb_participant = RedbParticipant::new(&db);
            let csr_participant = CsrParticipant::new(Arc::clone(&csr), csr_file.path().to_path_buf());
            let tantivy_participant = TantivyParticipant::new(tantivy_dir.path()).unwrap();
            let coordinator = Coordinator::new(
                wal,
                version,
                vec![Box::new(redb_participant), Box::new(csr_participant), Box::new(tantivy_participant)],
            );
            coordinator.stage(set).unwrap();
        }

        let repo_dir = TempDir::new().unwrap();
        assert!(
            Command::new("git")
                .args(["init", "--quiet", "--initial-branch=main"])
                .current_dir(repo_dir.path())
                .status()
                .unwrap()
                .success()
        );
        let engine = Engine {
            db,
            csr,
            index_dir: PathBuf::new(),
            repo_root: repo_dir.path().to_path_buf(),
            #[cfg(feature = "full")]
            embedder: Embedder::new("nonexistent.onnx", "nonexistent.json"),
            #[cfg(feature = "full")]
            hnsw_cache: std::sync::Mutex::new(None),
            _lock: None,
        };
        let result = engine.trace(&id_hex(&symbol_id)).unwrap();
        assert_eq!(result.hops.len(), 1);
        assert_eq!(result.hops[0].id, req_id);
        assert_eq!(result.hops[0].edge_type, EdgeType::Satisfies);
        assert!(!result.hops[0].incoming);

        // Tracing the requirement finds what satisfies it (against the edge).
        let reverse = engine.trace(&id_hex(&req_id)).unwrap();
        assert_eq!(reverse.hops.len(), 1);
        assert_eq!(reverse.hops[0].id, symbol_id);
        assert!(reverse.hops[0].incoming);
    }

    #[test]
    fn diff_staged_is_empty_for_a_clean_tree() {
        let repo_dir = TempDir::new().unwrap();
        init_git_repo_with_markdown(repo_dir.path());
        let index_dir = TempDir::new().unwrap();
        let engine = Engine::open(index_dir.path(), repo_dir.path()).unwrap();
        engine.sync().unwrap();

        let result = engine.diff_staged().unwrap();
        assert!(result.changed_symbols.is_empty());
    }

    #[test]
    fn blame_finds_the_commit_that_introduced_a_symbol() {
        let repo_dir = TempDir::new().unwrap();
        for args in [
            vec!["init", "--quiet", "--initial-branch=main"],
            vec!["config", "user.email", "fixture@nexspec.test"],
            vec!["config", "user.name", "Fixture"],
            vec!["config", "commit.gpgsign", "false"],
        ] {
            assert!(Command::new("git").args(&args).current_dir(repo_dir.path()).status().unwrap().success());
        }
        std::fs::write(repo_dir.path().join("lib.rs"), "fn hello() {}\n").unwrap();
        assert!(Command::new("git").args(["add", "-A"]).current_dir(repo_dir.path()).status().unwrap().success());
        assert!(
            Command::new("git")
                .args(["commit", "--quiet", "-m", "add hello"])
                .current_dir(repo_dir.path())
                .status()
                .unwrap()
                .success()
        );

        let index_dir = TempDir::new().unwrap();
        let engine = Engine::open(index_dir.path(), repo_dir.path()).unwrap();
        engine.sync().unwrap();

        let result = engine.blame("hello", false).unwrap();
        assert_eq!(result.hunks.len(), 1);
        assert_eq!(result.hunks[0].author_email, "fixture@nexspec.test");
    }
}
