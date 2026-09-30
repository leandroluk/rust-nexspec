//! Portable views of the graph (Fase 12, `.specs/features/graph-export/`).
//!
//! Everything starts from [`ExportGraph`]: nodes with their full payload, typed
//! edges with confidence and context, and the communities of Fase 10, all in a
//! stable order and without a timestamp, so the same graph always produces the
//! same bytes. JSON is the interchange format (and what `import` reads back);
//! HTML, tree and wiki are renderings of the same model.

pub mod html;
pub mod tree;
pub mod wiki;

use std::collections::{BTreeMap, HashMap};

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::graph::edge::{Confidence, decode_meta};
use crate::graph::node::NodePayload;
use crate::query::filter::{context_name, relation_name};
use crate::report::communities::Communities;
use crate::report::snapshot::GraphSnapshot;
use crate::search::{hex, unhex};
use crate::sync::mutation::StableId;

/// Bumped when the JSON layout changes in a way a reader has to know about.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("invalid path pattern `{0}`: {1}")]
    Glob(String, String),
    #[error("unknown node kind `{0}` (file, symbol, requirement, task, adr, doc_section, table, view, column, constraint, package, endpoint, annotation)")]
    Kind(String),
    #[error("{0}")]
    Json(String),
    #[error("schema_version {0} is newer than this build understands ({SCHEMA_VERSION})")]
    Version(u32),
}

const KINDS: [&str; 13] = ["file", "symbol", "requirement", "task", "adr", "doc_section", "table", "view", "column", "constraint", "package", "endpoint", "annotation"];

/// A node payload with every field, hashes as hex; the mirror of [`NodePayload`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExportPayload {
    Requirement { title: String, source_hash: String, body: String },
    Task { title: String, source_hash: String, body: String },
    Adr { title: String, source_hash: String, body: String },
    DocSection { title: String, source_hash: String },
    Symbol { name: String, source_hash: String, line_start: u32, line_end: u32 },
    File { path: String, source_hash: String },
    Table { schema: String, name: String, is_view: bool },
    Column { table: String, name: String, sql_type: String, nullable: bool },
    Constraint { table: String, name: String, kind: String },
    Package { name: String, version: String, dir: String, #[serde(default)] dependencies: Vec<String> },
    Endpoint { method: String, path: String, operation_id: String, external: bool },
    Annotation { target: String, label: String, note: String, author: String, at: String, state: String, outcome: String },
}

impl From<&NodePayload> for ExportPayload {
    fn from(payload: &NodePayload) -> Self {
        match payload {
            NodePayload::Requirement { title, source_hash, body } => Self::Requirement { title: title.clone(), source_hash: hex(source_hash), body: body.clone() },
            NodePayload::Task { title, source_hash, body } => Self::Task { title: title.clone(), source_hash: hex(source_hash), body: body.clone() },
            NodePayload::Adr { title, source_hash, body } => Self::Adr { title: title.clone(), source_hash: hex(source_hash), body: body.clone() },
            NodePayload::DocSection { title, source_hash } => Self::DocSection { title: title.clone(), source_hash: hex(source_hash) },
            NodePayload::Symbol { name, source_hash, line_start, line_end } => {
                Self::Symbol { name: name.clone(), source_hash: hex(source_hash), line_start: *line_start, line_end: *line_end }
            }
            NodePayload::File { path, source_hash } => Self::File { path: path.clone(), source_hash: hex(source_hash) },
            NodePayload::Table { schema, name, is_view } => Self::Table { schema: schema.clone(), name: name.clone(), is_view: *is_view },
            NodePayload::Column { table, name, sql_type, nullable } => Self::Column { table: table.clone(), name: name.clone(), sql_type: sql_type.clone(), nullable: *nullable },
            NodePayload::Constraint { table, name, kind } => Self::Constraint { table: table.clone(), name: name.clone(), kind: kind.clone() },
            NodePayload::Package { name, version, dir, dependencies } => Self::Package { name: name.clone(), version: version.clone(), dir: dir.clone(), dependencies: dependencies.clone() },
            NodePayload::Annotation { target, label, note, author, at, state, outcome } => Self::Annotation {
                target: target.clone(),
                label: label.clone(),
                note: note.clone(),
                author: author.clone(),
                at: at.clone(),
                state: state.clone(),
                outcome: outcome.clone(),
            },
            NodePayload::Endpoint { method, path, operation_id, external } => Self::Endpoint { method: method.clone(), path: path.clone(), operation_id: operation_id.clone(), external: *external },
        }
    }
}

impl ExportPayload {
    /// The payload back as the index stores it; `None` if a hash is not 32 bytes of hex.
    pub fn to_payload(&self) -> Option<NodePayload> {
        let hash = |h: &str| unhex(h);
        Some(match self {
            Self::Requirement { title, source_hash, body } => NodePayload::Requirement { title: title.clone(), source_hash: hash(source_hash)?, body: body.clone() },
            Self::Task { title, source_hash, body } => NodePayload::Task { title: title.clone(), source_hash: hash(source_hash)?, body: body.clone() },
            Self::Adr { title, source_hash, body } => NodePayload::Adr { title: title.clone(), source_hash: hash(source_hash)?, body: body.clone() },
            Self::DocSection { title, source_hash } => NodePayload::DocSection { title: title.clone(), source_hash: hash(source_hash)? },
            Self::Symbol { name, source_hash, line_start, line_end } => {
                NodePayload::Symbol { name: name.clone(), source_hash: hash(source_hash)?, line_start: *line_start, line_end: *line_end }
            }
            Self::File { path, source_hash } => NodePayload::File { path: path.clone(), source_hash: hash(source_hash)? },
            Self::Table { schema, name, is_view } => NodePayload::Table { schema: schema.clone(), name: name.clone(), is_view: *is_view },
            Self::Column { table, name, sql_type, nullable } => NodePayload::Column { table: table.clone(), name: name.clone(), sql_type: sql_type.clone(), nullable: *nullable },
            Self::Constraint { table, name, kind } => NodePayload::Constraint { table: table.clone(), name: name.clone(), kind: kind.clone() },
            Self::Package { name, version, dir, dependencies } => NodePayload::Package { name: name.clone(), version: version.clone(), dir: dir.clone(), dependencies: dependencies.clone() },
            Self::Annotation { target, label, note, author, at, state, outcome } => NodePayload::Annotation {
                target: target.clone(),
                label: label.clone(),
                note: note.clone(),
                author: author.clone(),
                at: at.clone(),
                state: state.clone(),
                outcome: outcome.clone(),
            },
            Self::Endpoint { method, path, operation_id, external } => NodePayload::Endpoint { method: method.clone(), path: path.clone(), operation_id: operation_id.clone(), external: *external },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportNode {
    /// Stable id, lowercase hex.
    pub id: String,
    pub kind: String,
    pub label: String,
    /// Repository-relative path of the file (or the symbol's file, or the package directory).
    pub path: Option<String>,
    pub community: Option<usize>,
    /// Repository tag; only in merged or global exports (Fase 13).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    pub payload: ExportPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportEdge {
    pub from: String,
    pub to: String,
    #[serde(rename = "type")]
    pub relation: String,
    /// `extracted` or `inferred`.
    pub confidence: String,
    pub context: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportCommunity {
    pub id: usize,
    pub label: String,
    pub size: usize,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportGraph {
    pub schema_version: u32,
    pub nodes: Vec<ExportNode>,
    pub edges: Vec<ExportEdge>,
    pub communities: Vec<ExportCommunity>,
}

/// Which part of the graph to export (REQ-1201).
#[derive(Debug, Clone, Default)]
pub struct ExportFilter {
    /// Globs on repository-relative paths; empty = no path restriction.
    pub paths: Vec<String>,
    /// Node kinds (`file`, `symbol`, `table`, …); empty = every kind.
    pub kinds: Vec<String>,
}

struct CompiledFilter {
    paths: Option<GlobSet>,
    kinds: Vec<String>,
}

impl ExportFilter {
    fn compile(&self) -> Result<CompiledFilter, ExportError> {
        for kind in &self.kinds {
            if !KINDS.contains(&kind.as_str()) {
                return Err(ExportError::Kind(kind.clone()));
            }
        }
        let paths = if self.paths.is_empty() {
            None
        } else {
            let mut builder = GlobSetBuilder::new();
            for pattern in &self.paths {
                builder.add(Glob::new(pattern).map_err(|e| ExportError::Glob(pattern.clone(), e.to_string()))?);
            }
            Some(builder.build().map_err(|e| ExportError::Glob(self.paths.join(","), e.to_string()))?)
        };
        Ok(CompiledFilter { paths, kinds: self.kinds.clone() })
    }
}

impl CompiledFilter {
    fn allows(&self, kind: &str, path: Option<&str>) -> bool {
        (self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)) && self.paths.as_ref().is_none_or(|set| path.is_some_and(|p| set.is_match(p)))
    }
}

fn path_of(snapshot: &GraphSnapshot, id: &StableId) -> Option<String> {
    match snapshot.nodes.get(id)? {
        NodePayload::Package { dir, .. } => Some(dir.clone()),
        _ => snapshot.path_of(id).map(str::to_string),
    }
}

impl ExportGraph {
    pub fn from_snapshot(snapshot: &GraphSnapshot, communities: &Communities, filter: &ExportFilter) -> Result<Self, ExportError> {
        let filter = filter.compile()?;
        let community_of_file: HashMap<&str, usize> = communities.listed.iter().flat_map(|c| c.files.iter().map(move |f| (f.as_str(), c.id))).collect();

        let mut nodes: BTreeMap<String, ExportNode> = BTreeMap::new();
        for (id, payload) in &snapshot.nodes {
            let kind = snapshot.kind_name(id);
            let path = path_of(snapshot, id);
            if !filter.allows(kind, path.as_deref()) {
                continue;
            }
            let community = path.as_deref().and_then(|p| community_of_file.get(p)).copied();
            nodes.insert(hex(id), ExportNode { id: hex(id), kind: kind.to_string(), label: snapshot.label(id), path, community, repo: None, payload: payload.into() });
        }

        let mut edges: Vec<ExportEdge> = snapshot
            .edges
            .iter()
            .filter(|e| nodes.contains_key(&hex(&e.from)) && nodes.contains_key(&hex(&e.to)))
            .map(|e| {
                let (confidence, context) = decode_meta(e.meta);
                ExportEdge {
                    from: hex(&e.from),
                    to: hex(&e.to),
                    relation: relation_name(e.edge_type).to_string(),
                    confidence: if confidence == Confidence::Extracted { "extracted" } else { "inferred" }.to_string(),
                    context: context_name(context).to_string(),
                }
            })
            .collect();
        edges.sort_by(|a, b| (&a.from, &a.to, &a.relation, &a.context).cmp(&(&b.from, &b.to, &b.relation, &b.context)));
        edges.dedup();

        let kept_files: std::collections::HashSet<&str> = nodes.values().filter(|n| n.kind == "file").filter_map(|n| n.path.as_deref()).collect();
        let exported_communities: Vec<ExportCommunity> = communities
            .listed
            .iter()
            .map(|c| ExportCommunity { id: c.id, label: c.label.clone(), size: c.size, files: c.files.iter().filter(|f| kept_files.contains(f.as_str())).cloned().collect() })
            .filter(|c| !c.files.is_empty())
            .collect();

        Ok(Self { schema_version: SCHEMA_VERSION, nodes: nodes.into_values().collect(), edges, communities: exported_communities })
    }

    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).expect("the export model serialises");
        text.push('\n');
        text
    }

    pub fn from_json(text: &str) -> Result<Self, ExportError> {
        let graph: Self = serde_json::from_str(text).map_err(|e| ExportError::Json(e.to_string()))?;
        if graph.schema_version > SCHEMA_VERSION {
            return Err(ExportError::Version(graph.schema_version));
        }
        Ok(graph)
    }

    /// Edges touching each node, in and out, by node id.
    pub fn degrees(&self) -> HashMap<&str, usize> {
        let mut degrees: HashMap<&str, usize> = HashMap::new();
        for edge in &self.edges {
            *degrees.entry(edge.from.as_str()).or_default() += 1;
            *degrees.entry(edge.to.as_str()).or_default() += 1;
        }
        degrees
    }

    /// The `max` most connected nodes (ties by id) and the edges among them, plus how many nodes were left out.
    pub fn truncated(&self, max: usize) -> (ExportGraph, usize) {
        if self.nodes.len() <= max {
            return (self.clone(), 0);
        }
        let degrees = self.degrees();
        let mut ranked: Vec<&ExportNode> = self.nodes.iter().collect();
        ranked.sort_by(|a, b| degrees.get(b.id.as_str()).cmp(&degrees.get(a.id.as_str())).then_with(|| a.id.cmp(&b.id)));
        let keep: std::collections::HashSet<&str> = ranked.iter().take(max).map(|n| n.id.as_str()).collect();
        let nodes: Vec<ExportNode> = self.nodes.iter().filter(|n| keep.contains(n.id.as_str())).cloned().collect();
        let edges: Vec<ExportEdge> = self.edges.iter().filter(|e| keep.contains(e.from.as_str()) && keep.contains(e.to.as_str())).cloned().collect();
        let omitted = self.nodes.len() - nodes.len();
        (ExportGraph { schema_version: self.schema_version, nodes, edges, communities: self.communities.clone() }, omitted)
    }
}

/// Edges the pages leave out: "changes together" links are so many (tens of thousands) that they bury the
/// structure and the page; they stay in the JSON.
pub(crate) const PAGE_SKIPPED_RELATION: &str = "cochanges";

/// Edges as `[from index, to index, relation index, 1 if inferred]` over `nodes`, plus the relation names:
/// far smaller than objects with 64-character ids, which matters for a file the browser has to parse.
pub(crate) fn compact_edges(nodes: &[ExportNode], edges: &[ExportEdge]) -> (Vec<[usize; 4]>, Vec<String>) {
    let index: HashMap<&str, usize> = nodes.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
    let mut relations: Vec<String> = edges.iter().map(|e| e.relation.clone()).filter(|r| r != PAGE_SKIPPED_RELATION).collect();
    relations.sort();
    relations.dedup();
    let compact = edges
        .iter()
        .filter(|e| e.relation != PAGE_SKIPPED_RELATION)
        .filter_map(|e| {
            let relation = relations.iter().position(|r| *r == e.relation)?;
            Some([*index.get(e.from.as_str())?, *index.get(e.to.as_str())?, relation, usize::from(e.confidence == "inferred")])
        })
        .collect();
    (compact, relations)
}

/// JSON safe to embed inside `<script type="application/json">`: `</script>` and `<!--` cannot end or
/// confuse the element.
pub(crate) fn embed_json(json: &str) -> String {
    json.replace("</", "<\\/").replace("<!--", "<\\u0021--")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::edge::EdgeType;
    use crate::report::communities::communities;
    use crate::report::snapshot::test_support::*;

    fn sample() -> GraphSnapshot {
        snapshot(
            vec![(1, file("src/a/one.ts")), (2, file("src/a/two.ts")), (3, file("src/b/three.ts")), (4, symbol("Alpha")), (5, requirement("REQ-1"))],
            vec![edge(1, 1, 2, EdgeType::Imports), edge(2, 2, 3, EdgeType::Imports), edge(3, 4, 1, EdgeType::DefinedIn), edge(4, 4, 5, EdgeType::Satisfies)],
        )
    }

    fn export(filter: &ExportFilter) -> ExportGraph {
        let snap = sample();
        ExportGraph::from_snapshot(&snap, &communities(&snap, 5, 0), filter).unwrap()
    }

    #[test]
    fn everything_is_exported_in_a_stable_order() {
        let graph = export(&ExportFilter::default());
        assert_eq!((graph.nodes.len(), graph.edges.len(), graph.schema_version), (5, 4, 1));
        let ids: Vec<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted, "nodes by id");
        assert_eq!(graph.to_json(), export(&ExportFilter::default()).to_json(), "same graph, same bytes");
        let imports = graph.edges.iter().find(|e| e.relation == "imports").unwrap();
        assert_eq!((imports.confidence.as_str(), imports.context.as_str()), ("extracted", "runtime"));
    }

    #[test]
    fn path_and_kind_filters_keep_only_edges_between_kept_nodes() {
        let graph = export(&ExportFilter { paths: vec!["src/a/**".into()], kinds: vec!["file".into()] });
        let labels: Vec<&str> = graph.nodes.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(graph.nodes.len(), 2, "{labels:?}");
        assert_eq!(graph.edges.len(), 1, "only one.ts -> two.ts survives");

        let only_kind = export(&ExportFilter { paths: vec![], kinds: vec!["requirement".into()] });
        assert_eq!((only_kind.nodes.len(), only_kind.edges.len()), (1, 0));
        assert!(matches!(ExportFilter { paths: vec![], kinds: vec!["banana".into()] }.compile(), Err(ExportError::Kind(_))));
        assert!(matches!(ExportFilter { paths: vec!["[".into()], kinds: vec![] }.compile(), Err(ExportError::Glob(..))));
    }

    #[test]
    fn json_round_trips_and_rebuilds_the_same_payloads() {
        let snap = sample();
        let graph = export(&ExportFilter::default());
        let back = ExportGraph::from_json(&graph.to_json()).unwrap();
        assert_eq!(back, graph);
        for node in &back.nodes {
            let payload = node.payload.to_payload().expect("hashes are valid");
            let original = snap.nodes.get(&unhex(&node.id).unwrap()).unwrap();
            assert_eq!(&payload, original, "{}", node.label);
        }
        assert!(matches!(ExportGraph::from_json(r#"{"schema_version":99,"nodes":[],"edges":[],"communities":[]}"#), Err(ExportError::Version(99))));
        assert!(matches!(ExportGraph::from_json("nope"), Err(ExportError::Json(_))));
    }

    #[test]
    fn truncation_keeps_the_most_connected_nodes_and_says_how_many_went() {
        let graph = export(&ExportFilter::default());
        let (small, omitted) = graph.truncated(3);
        assert_eq!((small.nodes.len(), omitted), (3, 2));
        assert!(small.edges.iter().all(|e| small.nodes.iter().any(|n| n.id == e.from) && small.nodes.iter().any(|n| n.id == e.to)));
        assert_eq!(graph.truncated(100).1, 0);
    }

    #[test]
    fn embedded_json_cannot_close_the_script_element() {
        let json = r#"{"label":"</script><script>alert(1)</script>","c":"<!-- x"}"#;
        let safe = embed_json(json);
        assert!(!safe.contains("</") && !safe.contains("<!--"));
        let parsed: serde_json::Value = serde_json::from_str(&safe).unwrap();
        assert_eq!(parsed["label"], "</script><script>alert(1)</script>", "the JSON value is unchanged");
    }
}
