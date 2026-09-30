//! Embedded MCP server (REQ-609 in `.specs/features/cli-mcp-server/spec.md`)
//! — an `rmcp` stdio tool router. Every `#[tool]` here is a thin call into
//! the matching [`Engine`] method plus a JSON-shaped response; none of them
//! re-implement logic the CLI (`src/bin/nexspec.rs`) doesn't already share
//! through `Engine`.

use std::sync::Arc;

use rmcp::ServerHandler;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::engine::{BlameResult, DiffResult, Engine, SearchResult, TraceResult, id_hex};
use crate::graph::node::NodePayload;
use crate::sync::mutation::StableId;
use crate::sync_orchestrator::SyncReport;

#[derive(Serialize)]
struct NodeDto {
    id: String,
    kind: &'static str,
    summary: String,
}

fn node_dto(id: &StableId, payload: &NodePayload) -> NodeDto {
    let (kind, summary) = match payload {
        NodePayload::Requirement { title, body, .. } => ("requirement", format!("{title}: {body}")),
        NodePayload::Task { title, body, .. } => ("task", format!("{title}: {body}")),
        NodePayload::Adr { title, body, .. } => ("adr", format!("{title}: {body}")),
        NodePayload::DocSection { title, .. } => ("doc_section", title.clone()),
        NodePayload::File { path, .. } => ("file", path.clone()),
        NodePayload::Symbol { name, .. } => ("symbol", name.clone()),
    };
    NodeDto { id: id_hex(id), kind, summary }
}

#[derive(Serialize)]
struct SearchHitDto {
    node: NodeDto,
    score: f32,
}

#[derive(Serialize)]
struct SearchResponse {
    hits: Vec<SearchHitDto>,
    markdown: Option<String>,
}

fn search_response(result: SearchResult) -> SearchResponse {
    SearchResponse {
        hits: result
            .hits
            .iter()
            .map(|hit| SearchHitDto { node: node_dto(&hit.id, &hit.payload), score: hit.score })
            .collect(),
        markdown: result.markdown,
    }
}

#[derive(Serialize)]
struct TraceHopDto {
    node: NodeDto,
    depth: u8,
    edge_type: String,
    incoming: bool,
    /// `extracted` or `inferred`.
    confidence: &'static str,
    /// `runtime`, `type-only`, `test` or `spec`.
    context: &'static str,
    /// File the node lives in, when known.
    path: Option<String>,
}

#[derive(Serialize)]
struct TraceResponse {
    hops: Vec<TraceHopDto>,
    /// Nodes left out because a hop exceeded the per-hop limit.
    omitted: usize,
}

fn trace_response(result: TraceResult) -> TraceResponse {
    TraceResponse {
        hops: result
            .hops
            .iter()
            .map(|hop| TraceHopDto {
                node: node_dto(&hop.id, &hop.payload),
                depth: hop.depth,
                edge_type: format!("{:?}", hop.edge_type),
                incoming: hop.incoming,
                confidence: match crate::graph::edge::decode_meta(hop.meta).0 {
                    crate::graph::edge::Confidence::Extracted => "extracted",
                    crate::graph::edge::Confidence::Inferred => "inferred",
                },
                context: match crate::graph::edge::decode_meta(hop.meta).1 {
                    crate::graph::edge::EdgeContext::Runtime => "runtime",
                    crate::graph::edge::EdgeContext::TypeOnly => "type-only",
                    crate::graph::edge::EdgeContext::Test => "test",
                    crate::graph::edge::EdgeContext::Spec => "spec",
                },
                path: hop.path.clone(),
            })
            .collect(),
        omitted: result.omitted,
    }
}

#[derive(Serialize)]
struct ImpactedSymbolDto {
    id: String,
    name: String,
    dependants: Vec<String>,
}

#[derive(Serialize)]
struct DiffResponse {
    changed_symbols: Vec<ImpactedSymbolDto>,
}

fn diff_response(result: DiffResult) -> DiffResponse {
    DiffResponse {
        changed_symbols: result
            .changed_symbols
            .into_iter()
            .map(|s| ImpactedSymbolDto {
                id: id_hex(&s.id),
                name: s.name,
                dependants: s.dependants.iter().map(id_hex).collect(),
            })
            .collect(),
    }
}

#[derive(Serialize)]
struct BlameHunkDto {
    commit: String,
    author_name: String,
    author_email: String,
    unix_seconds: i64,
    line_start: u32,
    line_end_exclusive: u32,
}

#[derive(Serialize)]
struct BlameResponse {
    hunks: Vec<BlameHunkDto>,
    co_changed_files: Vec<String>,
}

fn blame_response(result: BlameResult) -> BlameResponse {
    BlameResponse {
        hunks: result
            .hunks
            .iter()
            .map(|h| BlameHunkDto {
                commit: h.commit_oid.iter().map(|b| format!("{b:02x}")).collect(),
                author_name: h.author_name.clone(),
                author_email: h.author_email.clone(),
                unix_seconds: h.author_unix_seconds,
                line_start: h.lines.start,
                line_end_exclusive: h.lines.end,
            })
            .collect(),
        co_changed_files: result.co_changed_files,
    }
}

#[derive(Serialize)]
struct SyncResponse {
    target_version: Option<u64>,
    files_added: usize,
    files_modified: usize,
    files_deleted: usize,
    files_dirty: usize,
}

fn sync_response(report: SyncReport) -> SyncResponse {
    SyncResponse {
        target_version: report.target_version,
        files_added: report.files_added,
        files_modified: report.files_modified,
        files_deleted: report.files_deleted,
        files_dirty: report.files_dirty,
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct QueryContextArgs {
    /// Free-text hybrid search query.
    pub query: String,
    /// When set, prunes/budgets/serializes the result into dense Markdown
    /// fitting this many tokens (with a 90% safety margin).
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SemanticSearchArgs {
    /// Free-text query for embedding-based similarity search (falls back
    /// to lexical BM25 automatically when no embedding model is loaded).
    pub query: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct TraceRequirementArgs {
    /// A 64-hex-char stable id, or free text resolved via search (a
    /// requirement/ADR marker, a symbol name, ...).
    pub target: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct FindImpactedCodeArgs {}

#[derive(Deserialize, JsonSchema)]
pub struct GetSymbolHistoryArgs {
    /// The symbol's name (or its stable id, hex-encoded).
    pub symbol: String,
    /// Use the unbounded co-change window instead of the default (last 500
    /// commits or 6 months, whichever is smaller).
    #[serde(default)]
    pub full_history: bool,
}

#[derive(Deserialize, JsonSchema)]
pub struct SyncWorkspaceArgs {
    /// Replay any WAL frame no store fully applied yet before syncing.
    #[serde(default)]
    pub resume: bool,
}

/// `rmcp` tool router wrapping an [`Engine`]. `Arc` (rather than the CLI's
/// single-owner `Engine`) because `rmcp`'s `ServerHandler` methods run
/// behind a `Service` that may be invoked from multiple async tasks over
/// the same stdio connection.
#[derive(Clone)]
pub struct NexSpecMcp {
    engine: Arc<Engine>,
}

impl NexSpecMcp {
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }
}

#[tool_router]
impl NexSpecMcp {
    #[tool(description = "Hybrid search (BM25 + vector when available) over specs/code/docs, with optional token-budgeted dense Markdown output")]
    async fn query_context(&self, Parameters(args): Parameters<QueryContextArgs>) -> Result<String, String> {
        let result = self.engine.search(&args.query, args.max_tokens).map_err(|e| e.to_string())?;
        serde_json::to_string(&search_response(result)).map_err(|e| e.to_string())
    }

    #[tool(description = "Vector-similarity search (degrades to hybrid BM25 automatically if no embedding model is available)")]
    async fn semantic_search(&self, Parameters(args): Parameters<SemanticSearchArgs>) -> Result<String, String> {
        let result = self.engine.search(&args.query, None).map_err(|e| e.to_string())?;
        serde_json::to_string(&search_response(result)).map_err(|e| e.to_string())
    }

    #[tool(description = "Deterministic topological trace of a requirement/ADR/symbol's dependencies (Satisfies/DependsOn/DefinedIn/Implements)")]
    async fn trace_requirement(&self, Parameters(args): Parameters<TraceRequirementArgs>) -> Result<String, String> {
        let result = self.engine.trace(&args.target).map_err(|e| e.to_string())?;
        serde_json::to_string(&trace_response(result)).map_err(|e| e.to_string())
    }

    #[tool(description = "Structural impact analysis over the dirty working tree: changed symbols and their direct dependants")]
    async fn find_impacted_code(&self, Parameters(_args): Parameters<FindImpactedCodeArgs>) -> Result<String, String> {
        let result = self.engine.diff_staged().map_err(|e| e.to_string())?;
        serde_json::to_string(&diff_response(result)).map_err(|e| e.to_string())
    }

    #[tool(description = "AST-aware git blame for a symbol, scoped to its line range, plus files that co-change with it")]
    async fn get_symbol_history(&self, Parameters(args): Parameters<GetSymbolHistoryArgs>) -> Result<String, String> {
        let result = self.engine.blame(&args.symbol, args.full_history).map_err(|e| e.to_string())?;
        serde_json::to_string(&blame_response(result)).map_err(|e| e.to_string())
    }

    #[tool(description = "Run one incremental sync cycle against the repository's Git history")]
    async fn sync_workspace(&self, Parameters(args): Parameters<SyncWorkspaceArgs>) -> Result<String, String> {
        if args.resume {
            self.engine.resume().map_err(|e| e.to_string())?;
        }
        let report = self.engine.sync().map_err(|e| e.to_string())?;
        serde_json::to_string(&sync_response(report)).map_err(|e| e.to_string())
    }
}

#[tool_handler]
impl ServerHandler for NexSpecMcp {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    fn fixture_engine() -> (TempDir, TempDir, Engine) {
        let repo_dir = TempDir::new().unwrap();
        for args in [
            vec!["init", "--quiet", "--initial-branch=main"],
            vec!["config", "user.email", "fixture@nexspec.test"],
            vec!["config", "user.name", "Fixture"],
            vec!["config", "commit.gpgsign", "false"],
        ] {
            assert!(Command::new("git").args(&args).current_dir(repo_dir.path()).status().unwrap().success());
        }
        std::fs::create_dir_all(repo_dir.path().join(".specs")).unwrap();
        std::fs::write(
            repo_dir.path().join(".specs/req.md"),
            "## Requirements\n- REQ-1: mcp test requirement\n",
        )
        .unwrap();
        assert!(Command::new("git").args(["add", "-A"]).current_dir(repo_dir.path()).status().unwrap().success());
        assert!(
            Command::new("git")
                .args(["commit", "--quiet", "-m", "init"])
                .current_dir(repo_dir.path())
                .status()
                .unwrap()
                .success()
        );

        let index_dir = TempDir::new().unwrap();
        let engine = Engine::open(index_dir.path(), repo_dir.path()).unwrap();
        engine.sync().unwrap();
        (repo_dir, index_dir, engine)
    }

    #[tokio::test]
    async fn query_context_returns_valid_json_with_a_hit() {
        let (_repo, _index, engine) = fixture_engine();
        let mcp = NexSpecMcp::new(Arc::new(engine));

        let result = mcp
            .query_context(Parameters(QueryContextArgs { query: "mcp test requirement".into(), max_tokens: None }))
            .await
            .unwrap();

        let value: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(!value["hits"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn sync_workspace_returns_a_report() {
        let (_repo, _index, engine) = fixture_engine();
        let mcp = NexSpecMcp::new(Arc::new(engine));

        let result = mcp.sync_workspace(Parameters(SyncWorkspaceArgs { resume: false })).await.unwrap();
        let value: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(value["files_added"].is_number());
    }

    #[tokio::test]
    async fn find_impacted_code_returns_empty_for_a_clean_tree() {
        let (_repo, _index, engine) = fixture_engine();
        let mcp = NexSpecMcp::new(Arc::new(engine));

        let result = mcp.find_impacted_code(Parameters(FindImpactedCodeArgs {})).await.unwrap();
        let value: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(value["changed_symbols"].as_array().unwrap().len(), 0);
    }
}
