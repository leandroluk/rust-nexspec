//! High-level entry points for the CLI and MCP (REQ-1101..1108): resolve the
//! target, run the query over the graph view, apply the token budget and
//! render Markdown or JSON. Everything here is shared by both front ends, so
//! they cannot drift apart.

use std::collections::HashMap;

use serde::Serialize;

use crate::engine::{Engine, EngineError, SearchHit};
use crate::graph::node::NodePayload;
use crate::query::affected::{self, AffectedOptions};
use crate::query::budget::fit_lines;
use crate::query::expand::{self, ExpandOptions};
use crate::query::explain;
use crate::query::filter::{EdgeFilter, relation_name};
use crate::query::path;
use crate::query::target::{self, Candidate, Resolved};
use crate::query::view::GraphView;
use crate::sync::mutation::StableId;

#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    #[error("{}", ambiguity_message(.0))]
    Ambiguous(Vec<Candidate>),
    #[error("{}", not_found_message(.0, .1))]
    NotFound(String, Vec<String>),
    #[error("--pick {0} is out of range: there are only {1} candidates")]
    PickOutOfRange(usize, usize),
    #[error("engine error: {0}")]
    Engine(#[from] EngineError),
    #[error("filter error: {0}")]
    Filter(#[from] crate::query::filter::FilterError),
    #[error("{0}")]
    Invalid(String),
}

fn ambiguity_message(candidates: &[Candidate]) -> String {
    let mut out = format!("the target matches {} nodes; say which one (`path/fragment:Name`, or --pick N):", candidates.len());
    for (i, c) in candidates.iter().enumerate() {
        out.push_str(&format!("\n  {}. {} [{}]", i + 1, c.label, c.kind));
    }
    out
}

fn not_found_message(target: &str, suggestions: &[String]) -> String {
    if suggestions.is_empty() {
        format!("no node matches {target:?}")
    } else {
        format!("no node matches {target:?}; did you mean: {}", suggestions.join(", "))
    }
}

/// Resolves `target`, optionally choosing among ambiguous candidates (1-based).
pub fn resolve(view: &GraphView, target: &str, pick: Option<usize>) -> Result<StableId, QueryError> {
    match target::resolve(&view.snapshot, target) {
        Resolved::One(id) => Ok(id),
        Resolved::Ambiguous(candidates) => match pick {
            Some(n) if n >= 1 && n <= candidates.len() => Ok(candidates[n - 1].id),
            Some(n) => Err(QueryError::PickOutOfRange(n, candidates.len())),
            None => Err(QueryError::Ambiguous(candidates)),
        },
        Resolved::NotFound(suggestions) => Err(QueryError::NotFound(target.to_string(), suggestions)),
    }
}

fn render(lines: Vec<String>, max_tokens: Option<u32>) -> String {
    fit_lines(lines, max_tokens).join("\n") + "\n"
}

fn json<T: Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).expect("query results always serialize") + "\n"
}

/// Options shared by every query command.
#[derive(Debug, Clone, Default)]
pub struct Common {
    pub max_tokens: Option<u32>,
    pub json: bool,
    pub pick: Option<usize>,
}

pub fn affected(engine: &Engine, target: &str, options: AffectedOptions, common: &Common) -> Result<String, QueryError> {
    let view = engine.query_view()?;
    let id = resolve(&view, target, common.pick)?;
    let result = affected::affected(&view, &id, &options);
    if common.json {
        return Ok(json(&result));
    }
    let communities = crate::report::communities::communities(&view.snapshot, 0, 0);
    Ok(render(affected::to_lines(&view, &id, &result, Some(&communities)), common.max_tokens))
}

pub fn find_path(engine: &Engine, from: &str, to: &str, filter: &EdgeFilter, common: &Common) -> Result<String, QueryError> {
    let view = engine.query_view()?;
    let a = resolve(&view, from, None)?;
    let b = resolve(&view, to, None)?;
    let result = path::find_path(&view, &a, &b, filter, 12);
    if common.json {
        return Ok(json(&result));
    }
    Ok(render(path::to_lines(&view, &a, &b, &result), common.max_tokens))
}

pub fn explain(engine: &Engine, target: &str, filter: &EdgeFilter, common: &Common) -> Result<String, QueryError> {
    let view = engine.query_view()?;
    let id = resolve(&view, target, common.pick)?;
    let context = engine.explain_context(&view, &id);
    let explanation = explain::explain(&view, &id, &context, filter, 10)
        .ok_or_else(|| QueryError::NotFound(target.to_string(), Vec::new()))?;
    if common.json {
        return Ok(json(&explanation));
    }
    Ok(render(explain::to_lines(&explanation), common.max_tokens))
}

#[derive(Serialize)]
struct QueryJson {
    question: String,
    seeds: Vec<String>,
    connected: Vec<ReachedJson>,
}

#[derive(Serialize)]
struct ReachedJson {
    depth: u8,
    from: String,
    to: String,
    relation: String,
    forward: bool,
}

pub fn query_graph(engine: &Engine, question: &str, options: ExpandOptions, common: &Common) -> Result<String, QueryError> {
    let view = engine.query_view()?;
    let hits: Vec<SearchHit> = engine.search(question, None)?.hits;
    // Seeds: the best-ranked hits that exist in the graph view, at most five.
    let mut seeds: Vec<StableId> = Vec::new();
    for hit in &hits {
        if view.contains(&hit.id) && !seeds.contains(&hit.id) {
            seeds.push(hit.id);
        }
        if seeds.len() >= 5 {
            break;
        }
    }
    let reached = expand::expand(&view, &seeds, &options);
    if common.json {
        return Ok(json(&QueryJson {
            question: question.to_string(),
            seeds: seeds.iter().map(|s| view.snapshot.label(s)).collect(),
            connected: reached
                .iter()
                .map(|r| ReachedJson {
                    depth: r.depth,
                    from: view.snapshot.label(&r.parent),
                    to: view.snapshot.label(&r.id),
                    relation: relation_name(r.edge_type).to_string(),
                    forward: r.forward,
                })
                .collect(),
        }));
    }
    let mut snippets: HashMap<StableId, String> = HashMap::new();
    for seed in &seeds {
        if let Some(signature) = engine.signature_of(seed) {
            snippets.insert(*seed, signature);
        } else if let Some(NodePayload::Requirement { body, .. } | NodePayload::Task { body, .. } | NodePayload::Adr { body, .. }) =
            view.snapshot.nodes.get(seed)
            && !body.is_empty()
        {
            snippets.insert(*seed, body.chars().take(200).collect());
        }
    }
    Ok(render(expand::to_lines(&view, question, &seeds, &reached, &snippets), common.max_tokens))
}

/// `ExplainContext` parts the engine computes (kept here so the engine file
/// does not grow query-specific helpers).
pub(crate) fn community_of(view: &GraphView, id: &StableId) -> Option<(String, usize, f64)> {
    let path = view.snapshot.path_of(id)?;
    let communities = crate::report::communities::communities(&view.snapshot, 0, 0);
    communities
        .listed
        .iter()
        .find(|c| c.files.iter().any(|f| f == path))
        .map(|c| (c.label.clone(), c.id, c.cohesion))
}
