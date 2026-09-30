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
        domain => domain.domain_label().unwrap_or(("unknown", String::new())),
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

/// Edge filters shared by the graph-query tools (same names as the CLI flags).
#[derive(Deserialize, JsonSchema, Default)]
pub struct QueryFilterArgs {
    /// Follow only these relations: imports, reexports, calls, instantiates, extends,
    /// references, satisfies, implements, defined_in, depends_on, cochanges, dependencies.
    #[serde(default)]
    pub relation: Vec<String>,
    /// Keep only these edge contexts: runtime, type-only, test, spec.
    #[serde(default)]
    pub context: Vec<String>,
    /// `extracted` drops inferred edges; `inferred` (default) keeps both.
    pub min_confidence: Option<String>,
}

impl QueryFilterArgs {
    fn build(&self) -> Result<crate::query::EdgeFilter, String> {
        crate::query::EdgeFilter::from_strings(&self.relation, self.min_confidence.as_deref(), &self.context)
            .map_err(|e| e.to_string())
    }
}

fn query_common(max_tokens: Option<u32>, format: &Option<String>, pick: Option<usize>) -> Result<crate::query::api::Common, String> {
    match format.as_deref() {
        None | Some("md") => Ok(crate::query::api::Common { max_tokens, json: false, pick, repo: None }),
        Some("json") => Ok(crate::query::api::Common { max_tokens, json: true, pick, repo: None }),
        Some(other) => Err(format!("unknown format {other:?} (expected md or json)")),
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct QueryGraphArgs {
    /// The question, in natural language.
    pub question: String,
    /// Depth-first instead of breadth-first.
    #[serde(default)]
    pub dfs: bool,
    /// How far to expand from the starting points (default 3).
    pub depth: Option<u8>,
    /// Token budget for the answer (default 2000).
    pub max_tokens: Option<u32>,
    #[serde(flatten)]
    pub filter: QueryFilterArgs,
    /// `md` (default) or `json`.
    pub format: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct FindPathArgs {
    /// Start node: a name, `path:Symbol`, `REQ-…` or a hex id.
    pub from: String,
    pub to: String,
    pub max_tokens: Option<u32>,
    #[serde(flatten)]
    pub filter: QueryFilterArgs,
    pub format: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ExplainNodeArgs {
    pub target: String,
    /// Choose among ambiguous matches (1-based).
    pub pick: Option<usize>,
    pub max_tokens: Option<u32>,
    #[serde(flatten)]
    pub filter: QueryFilterArgs,
    pub format: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct FindAffectedArgs {
    pub target: String,
    /// Levels to walk (default 2).
    pub depth: Option<u8>,
    /// Most nodes per level (default 25).
    pub limit: Option<usize>,
    pub pick: Option<usize>,
    pub max_tokens: Option<u32>,
    #[serde(flatten)]
    pub filter: QueryFilterArgs,
    pub format: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GraphReportArgs {
    /// `md` (default) or `json`.
    #[serde(default)]
    pub format: Option<String>,
    /// Fit the Markdown report into this many tokens.
    pub max_tokens: Option<u32>,
    /// God nodes to list (default 10).
    pub top: Option<usize>,
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

    #[tool(description = "Structural report of the graph: God nodes, communities with cohesion, requirement coverage, surprising connections, import cycles and suggested questions")]
    async fn graph_report(&self, Parameters(args): Parameters<GraphReportArgs>) -> Result<String, String> {
        let mut options = crate::report::ReportOptions::default();
        if let Some(top) = args.top {
            options.top = top.max(1);
        }
        let report = self.engine.report(options).map_err(|e| e.to_string())?;
        match args.format.as_deref() {
            Some("json") => Ok(crate::report::render::to_json(&report)),
            None | Some("md") => Ok(crate::report::render::to_markdown(&report, args.max_tokens)),
            Some(other) => Err(format!("unknown format {other:?} (expected md or json)")),
        }
    }

    #[tool(description = "Answer a question from the graph: hybrid-search starting points expanded through their relations, within a token budget")]
    async fn query_graph(&self, Parameters(args): Parameters<QueryGraphArgs>) -> Result<String, String> {
        let common = query_common(Some(args.max_tokens.unwrap_or(2000)), &args.format, None)?;
        let options = crate::query::expand::ExpandOptions {
            dfs: args.dfs,
            max_depth: args.depth.unwrap_or(3),
            filter: args.filter.build()?,
            ..Default::default()
        };
        crate::query::api::query_graph(&self.engine, &args.question, options, &common).map_err(|e| e.to_string())
    }

    #[tool(description = "Shortest chain of relations between two nodes (symbol, file or requirement); 'no path' is a valid answer")]
    async fn find_path(&self, Parameters(args): Parameters<FindPathArgs>) -> Result<String, String> {
        let common = query_common(args.max_tokens, &args.format, None)?;
        crate::query::api::find_path(&self.engine, &args.from, &args.to, &args.filter.build()?, &common).map_err(|e| e.to_string())
    }

    #[tool(description = "Describe one node: location, pruned signature, dependencies, dependents, requirements, community and recent authors")]
    async fn explain_node(&self, Parameters(args): Parameters<ExplainNodeArgs>) -> Result<String, String> {
        let common = query_common(args.max_tokens, &args.format, args.pick)?;
        crate::query::api::explain(&self.engine, &args.target, &args.filter.build()?, &common).map_err(|e| e.to_string())
    }

    #[tool(description = "Who depends on a node, transitively, filtered by relation/confidence/context and grouped by file")]
    async fn find_affected(&self, Parameters(args): Parameters<FindAffectedArgs>) -> Result<String, String> {
        let common = query_common(args.max_tokens, &args.format, args.pick)?;
        let options = crate::query::affected::AffectedOptions {
            depth: args.depth.unwrap_or(2),
            max_per_hop: args.limit.unwrap_or(25).max(1),
            filter: args.filter.build()?,
        };
        crate::query::api::affected(&self.engine, &args.target, options, &common).map_err(|e| e.to_string())
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
    async fn query_tools_answer_and_report_readable_errors() {
        let (_repo, _index, engine) = fixture_engine();
        let mcp = NexSpecMcp::new(Arc::new(engine));

        let explained = mcp
            .explain_node(Parameters(ExplainNodeArgs {
                target: "REQ-1".into(),
                pick: None,
                max_tokens: None,
                filter: QueryFilterArgs::default(),
                format: None,
            }))
            .await
            .unwrap();
        assert!(explained.starts_with("# REQ-1 (requirement)"), "{explained}");

        let affected = mcp
            .find_affected(Parameters(FindAffectedArgs {
                target: "REQ-1".into(),
                depth: None,
                limit: None,
                pick: None,
                max_tokens: None,
                filter: QueryFilterArgs::default(),
                format: Some("json".into()),
            }))
            .await
            .unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(&affected).unwrap()["nodes"].is_array());

        let missing = mcp
            .find_path(Parameters(FindPathArgs {
                from: "REQ-1".into(),
                to: "NoSuchNode".into(),
                max_tokens: None,
                filter: QueryFilterArgs::default(),
                format: None,
            }))
            .await
            .unwrap_err();
        assert!(missing.contains("no node matches"), "{missing}");

        let bad_relation = mcp
            .query_graph(Parameters(QueryGraphArgs {
                question: "anything".into(),
                dfs: false,
                depth: None,
                max_tokens: None,
                filter: QueryFilterArgs { relation: vec!["nope".into()], ..Default::default() },
                format: None,
            }))
            .await
            .unwrap_err();
        assert!(bad_relation.contains("unknown relation"), "{bad_relation}");
    }

    #[tokio::test]
    async fn graph_report_returns_markdown_json_and_rejects_unknown_formats() {
        let (_repo, _index, engine) = fixture_engine();
        let mcp = NexSpecMcp::new(Arc::new(engine));

        let md = mcp
            .graph_report(Parameters(GraphReportArgs { format: None, max_tokens: None, top: None }))
            .await
            .unwrap();
        assert!(md.contains("## Summary") && md.contains("## Requirement Coverage"), "{md}");
        assert!(md.contains("REQ-1"), "the unimplemented requirement is listed: {md}");

        let json = mcp
            .graph_report(Parameters(GraphReportArgs { format: Some("json".into()), max_tokens: None, top: Some(3) }))
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value["requirement_coverage"]["unimplemented_requirements"].as_array().unwrap().iter().any(|r| r == "REQ-1"));

        let err = mcp
            .graph_report(Parameters(GraphReportArgs { format: Some("xml".into()), max_tokens: None, top: None }))
            .await
            .unwrap_err();
        assert!(err.contains("unknown format"), "{err}");
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
