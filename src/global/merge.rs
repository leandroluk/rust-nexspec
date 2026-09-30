//! Putting the exports of several repositories into one graph (REQ-1301, REQ-1303, REQ-1304 in
//! `.specs/features/multi-repo-graph/spec.md`; decisions D1-D5 in its design).
//!
//! Each export is *tagged*: ids become `blake3("repo:<tag>:<id>")` (the same `package.json` exists in every
//! repository), and the origin shows in the node's own name (path and package directory get `<tag>/`,
//! table schemas and requirement titles get `<tag>:`). The tagged graphs are united, communities are
//! renumbered, and links that cross repositories are added: a package depending on a package of another
//! repository, and a client call matching an endpoint served elsewhere. Both are inferred from names.

use std::collections::{BTreeMap, BTreeSet};

use crate::code::parser::edge_id;
use crate::domain::http::{is_specific, paths_match};
use crate::export::{ExportCommunity, ExportEdge, ExportGraph, ExportNode, ExportPayload, SCHEMA_VERSION};
use crate::graph::edge::{Confidence, EdgeContext, EdgeType, encode_meta};
use crate::query::filter::{parse_context, relation_types};
use crate::search::unhex;
use crate::sync::mutation::{EdgeMutation, MutationSet, NodeMutation};

/// A repository tag: letters, digits, `.`, `_`, `-`; it becomes a path prefix, so no separators.
pub fn valid_tag(tag: &str) -> bool {
    !tag.is_empty() && tag.len() <= 64 && tag.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// The id a node of repository `tag` has in the global graph.
pub fn global_id(tag: &str, id: &str) -> String {
    blake3::hash(format!("repo:{tag}:{id}").as_bytes()).to_hex().to_string()
}

fn prefixed(tag: &str, path: &str) -> String {
    if path.is_empty() { tag.to_string() } else { format!("{tag}/{path}") }
}

/// The export of one repository, with global ids and its origin written into names.
pub fn tag_export(graph: &ExportGraph, tag: &str) -> ExportGraph {
    let map = |id: &str| global_id(tag, id);
    let nodes = graph
        .nodes
        .iter()
        .map(|n| {
            let mut node = n.clone();
            node.id = map(&n.id);
            node.repo = Some(tag.to_string());
            match &mut node.payload {
                ExportPayload::File { path, .. } => {
                    *path = prefixed(tag, path);
                    node.label = path.clone();
                }
                ExportPayload::Symbol { name, .. } => {
                    if let Some(p) = &n.path {
                        node.label = format!("{name} ({})", prefixed(tag, p));
                    }
                }
                ExportPayload::Requirement { title, .. } | ExportPayload::Task { title, .. } | ExportPayload::Adr { title, .. } | ExportPayload::DocSection { title, .. } => {
                    *title = format!("{tag}:{title}");
                    node.label = title.clone();
                }
                ExportPayload::Table { schema, .. } => *schema = format!("{tag}:{schema}"),
                ExportPayload::Package { dir, .. } => *dir = prefixed(tag, dir),
                ExportPayload::Column { .. } | ExportPayload::Constraint { .. } | ExportPayload::Endpoint { .. } => {}
            }
            node.path = n.path.as_deref().map(|p| prefixed(tag, p));
            node
        })
        .collect();
    let edges = graph.edges.iter().map(|e| ExportEdge { from: map(&e.from), to: map(&e.to), ..e.clone() }).collect();
    let communities = graph
        .communities
        .iter()
        .map(|c| ExportCommunity { id: c.id, label: format!("{tag}: {}", c.label), size: c.size, files: c.files.iter().map(|f| prefixed(tag, f)).collect() })
        .collect();
    ExportGraph { schema_version: SCHEMA_VERSION, nodes, edges, communities }
}

/// One graph from many repositories. Order matters only for a repeated tag: **the later one replaces the
/// earlier** (exports carry no commit, so "most recent" is what the caller says it is).
pub fn merge(parts: Vec<(String, ExportGraph)>) -> ExportGraph {
    let mut by_tag: BTreeMap<String, ExportGraph> = BTreeMap::new();
    for (tag, graph) in parts {
        let tagged = tag_export(&graph, &tag);
        by_tag.insert(tag, tagged);
    }
    let mut nodes: BTreeMap<String, ExportNode> = BTreeMap::new();
    let mut edges: BTreeSet<(String, String, String, String, String)> = BTreeSet::new();
    let mut communities: Vec<ExportCommunity> = Vec::new();
    for graph in by_tag.into_values() {
        let offset = communities.iter().map(|c| c.id).max().unwrap_or(0);
        for mut node in graph.nodes {
            node.community = node.community.map(|c| c + offset);
            nodes.insert(node.id.clone(), node);
        }
        for e in graph.edges {
            edges.insert((e.from, e.to, e.relation, e.context, e.confidence));
        }
        communities.extend(graph.communities.into_iter().map(|c| ExportCommunity { id: c.id + offset, ..c }));
    }
    let mut merged = ExportGraph {
        schema_version: SCHEMA_VERSION,
        nodes: nodes.into_values().collect(),
        edges: edges.into_iter().map(|(from, to, relation, context, confidence)| ExportEdge { from, to, relation, confidence, context }).collect(),
        communities,
    };
    add_cross_links(&mut merged);
    merged.edges.sort_by(|a, b| (&a.from, &a.to, &a.relation, &a.context).cmp(&(&b.from, &b.to, &b.relation, &b.context)));
    merged.edges.dedup();
    merged
}

fn inferred(from: &str, to: &str, relation: &str) -> ExportEdge {
    ExportEdge { from: from.to_string(), to: to.to_string(), relation: relation.to_string(), confidence: "inferred".to_string(), context: "runtime".to_string() }
}

/// Package dependencies and HTTP calls that cross from one repository into another (REQ-1304).
fn add_cross_links(graph: &mut ExportGraph) {
    let mut packages: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new(); // name -> [(node id, repo)]
    let mut served: Vec<(&str, &str, &str, &str)> = Vec::new(); // (id, repo, method, path)
    for node in &graph.nodes {
        let repo = node.repo.as_deref().unwrap_or_default();
        match &node.payload {
            ExportPayload::Package { name, .. } => packages.entry(name.as_str()).or_default().push((node.id.as_str(), repo)),
            ExportPayload::Endpoint { method, path, external: false, .. } if is_specific(path) => served.push((node.id.as_str(), repo, method.as_str(), path.as_str())),
            _ => {}
        }
    }
    let mut added: Vec<ExportEdge> = Vec::new();
    for node in &graph.nodes {
        let repo = node.repo.as_deref().unwrap_or_default();
        match &node.payload {
            ExportPayload::Package { dependencies, .. } => {
                for dependency in dependencies {
                    for (target, target_repo) in packages.get(dependency.as_str()).into_iter().flatten() {
                        if *target_repo != repo {
                            added.push(inferred(&node.id, target, "depends_on"));
                        }
                    }
                }
            }
            ExportPayload::Endpoint { method, path, external: true, .. } if is_specific(path) => {
                for (target, target_repo, target_method, target_path) in &served {
                    if *target_repo != repo && target_method == method && paths_match(path, target_path) {
                        added.push(inferred(&node.id, target, "calls"));
                    }
                }
            }
            _ => {}
        }
    }
    graph.edges.extend(added);
}

/// The index mutations that put `graph` into an (empty) index. Nodes with an unreadable payload hash are skipped.
pub fn to_mutations(graph: &ExportGraph) -> MutationSet {
    let mut set = MutationSet::default();
    for node in &graph.nodes {
        let (Some(id), Some(payload)) = (unhex(&node.id), node.payload.to_payload()) else { continue };
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&payload).expect("NodePayload must always serialize").to_vec();
        set.nodes.push(NodeMutation::Upsert { id, payload: bytes });
    }
    for edge in &graph.edges {
        let (Some(from), Some(to)) = (unhex(&edge.from), unhex(&edge.to)) else { continue };
        let Some(edge_type) = relation_types(&edge.relation).and_then(|t| t.first().copied()) else { continue };
        let context = parse_context(&edge.context).unwrap_or(EdgeContext::Runtime);
        let confidence = if edge.confidence == "inferred" { Confidence::Inferred } else { Confidence::Extracted };
        set.edges.push(EdgeMutation::Upsert {
            id: edge_id(&format!("global-{}-{}", edge.relation, edge.context), &from, &to),
            from,
            to,
            edge_type: EdgeType::to_code(edge_type),
            payload: vec![encode_meta(confidence, context)],
        });
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, kind: &str, label: &str, path: Option<&str>, payload: ExportPayload) -> ExportNode {
        ExportNode { id: id.into(), kind: kind.into(), label: label.into(), path: path.map(str::to_string), community: None, repo: None, payload }
    }

    fn file(id: &str, path: &str) -> ExportNode {
        node(id, "file", path, Some(path), ExportPayload::File { path: path.into(), source_hash: "00".repeat(32) })
    }

    fn package(id: &str, name: &str, deps: &[&str]) -> ExportNode {
        node(id, "package", name, Some("pkgs/x"), ExportPayload::Package { name: name.into(), version: "1".into(), dir: "pkgs/x".into(), dependencies: deps.iter().map(|d| d.to_string()).collect() })
    }

    fn endpoint(id: &str, method: &str, path: &str, external: bool) -> ExportNode {
        node(id, "endpoint", &format!("{method} {path}"), None, ExportPayload::Endpoint { method: method.into(), path: path.into(), operation_id: String::new(), external })
    }

    fn edge(from: &str, to: &str, relation: &str) -> ExportEdge {
        ExportEdge { from: from.into(), to: to.into(), relation: relation.into(), confidence: "extracted".into(), context: "runtime".into() }
    }

    fn graph(nodes: Vec<ExportNode>, edges: Vec<ExportEdge>) -> ExportGraph {
        ExportGraph { schema_version: 1, nodes, edges, communities: vec![] }
    }

    #[test]
    fn tags_are_path_safe() {
        assert!(valid_tag("svc-a") && valid_tag("billing.v2") && valid_tag("A_b"));
        assert!(!valid_tag("") && !valid_tag("a/b") && !valid_tag("a b") && !valid_tag(&"x".repeat(65)));
    }

    #[test]
    fn the_same_file_in_two_repositories_does_not_collide_and_shows_where_it_came_from() {
        let a = graph(vec![file("11", "package.json"), file("22", "src/a.ts")], vec![edge("22", "11", "imports")]);
        let b = graph(vec![file("11", "package.json")], vec![]);
        let merged = merge(vec![("svc-a".into(), a), ("svc-b".into(), b)]);
        assert_eq!(merged.nodes.len(), 3, "package.json of each repository is its own node");
        let labels: Vec<&str> = merged.nodes.iter().map(|n| n.label.as_str()).collect();
        assert!(labels.contains(&"svc-a/package.json") && labels.contains(&"svc-b/package.json") && labels.contains(&"svc-a/src/a.ts"), "{labels:?}");
        assert!(merged.nodes.iter().all(|n| n.repo.is_some()));
        assert_eq!(merged.edges.len(), 1);
        assert_eq!(global_id("svc-a", "11"), merged.edges[0].to, "edges follow the renamed ids");
    }

    #[test]
    fn a_repeated_tag_is_replaced_by_the_later_one_and_the_result_is_deterministic() {
        let old = graph(vec![file("11", "old.ts")], vec![]);
        let new = graph(vec![file("11", "new.ts")], vec![]);
        let merged = merge(vec![("svc".into(), old.clone()), ("svc".into(), new.clone())]);
        assert_eq!(merged.nodes.len(), 1);
        assert_eq!(merged.nodes[0].label, "svc/new.ts");
        let other = graph(vec![file("33", "x.ts")], vec![]);
        assert_eq!(
            merge(vec![("a".into(), new.clone()), ("b".into(), other.clone())]).to_json(),
            merge(vec![("b".into(), other), ("a".into(), new)]).to_json(),
            "input order does not change the bytes (different tags)"
        );
    }

    #[test]
    fn names_carry_the_origin_for_requirements_tables_and_packages() {
        let g = graph(
            vec![
                node("1", "requirement", "REQ-1", None, ExportPayload::Requirement { title: "REQ-1".into(), source_hash: "00".repeat(32), body: "b".into() }),
                node("2", "table", "tb_x", None, ExportPayload::Table { schema: "public".into(), name: "tb_x".into(), is_view: false }),
                package("3", "web", &[]),
            ],
            vec![],
        );
        let tagged = tag_export(&g, "svc");
        let payloads: Vec<&ExportPayload> = tagged.nodes.iter().map(|n| &n.payload).collect();
        assert!(payloads.iter().any(|p| matches!(p, ExportPayload::Requirement { title, .. } if title == "svc:REQ-1")));
        assert!(payloads.iter().any(|p| matches!(p, ExportPayload::Table { schema, .. } if schema == "svc:public")));
        assert!(payloads.iter().any(|p| matches!(p, ExportPayload::Package { dir, .. } if dir == "svc/pkgs/x")));
    }

    #[test]
    fn a_package_depending_on_a_package_of_another_repository_is_linked_as_inferred() {
        let web = graph(vec![package("1", "web", &["@acme/shared", "react"])], vec![]);
        let shared = graph(vec![package("1", "@acme/shared", &[])], vec![]);
        let merged = merge(vec![("web".into(), web), ("shared".into(), shared)]);
        let link = merged.edges.iter().find(|e| e.relation == "depends_on").expect("cross-repository dependency");
        assert_eq!((link.from.as_str(), link.to.as_str(), link.confidence.as_str()), (global_id("web", "1").as_str(), global_id("shared", "1").as_str(), "inferred"));
        assert_eq!(merged.edges.iter().filter(|e| e.relation == "depends_on").count(), 1, "react is external: no node, no link");

        let same_repo = graph(vec![package("1", "a", &["b"]), package("2", "b", &[])], vec![]);
        assert!(merge(vec![("mono".into(), same_repo)]).edges.is_empty(), "inside one repository the manifest pass already linked them");
    }

    #[test]
    fn a_client_call_reaches_the_endpoint_another_repository_serves() {
        let client = graph(vec![endpoint("1", "GET", "/api/v1/contracts/{}", true), endpoint("2", "GET", "/health", true), endpoint("3", "POST", "/contracts", true)], vec![]);
        let server = graph(vec![endpoint("1", "GET", "/contracts/{}", false), endpoint("2", "GET", "/health", false), endpoint("3", "GET", "/contracts", false)], vec![]);
        let merged = merge(vec![("web".into(), client), ("contracts".into(), server)]);
        let calls: Vec<_> = merged.edges.iter().filter(|e| e.relation == "calls").collect();
        assert_eq!(calls.len(), 1, "{calls:?}: only the specific GET with a matching path; /health is generic and POST /contracts has no POST provider");
        assert_eq!((calls[0].from.as_str(), calls[0].to.as_str(), calls[0].confidence.as_str()), (global_id("web", "1").as_str(), global_id("contracts", "1").as_str(), "inferred"));
    }

    #[test]
    fn mutations_rebuild_nodes_and_typed_edges() {
        let g = merge(vec![("svc".into(), graph(vec![file("11", "a.ts"), file("22", "b.ts")], vec![edge("11", "22", "imports")]))]);
        let set = to_mutations(&g);
        assert_eq!((set.nodes.len(), set.edges.len()), (2, 1));
        let EdgeMutation::Upsert { edge_type, .. } = &set.edges[0] else { panic!("an upsert") };
        assert_eq!(EdgeType::from_code(*edge_type), Some(EdgeType::Imports));
    }

    #[test]
    fn communities_are_renumbered_without_clashing() {
        let mut a = graph(vec![file("1", "a/x.ts")], vec![]);
        a.nodes[0].community = Some(1);
        a.communities = vec![ExportCommunity { id: 1, label: "a/…".into(), size: 1, files: vec!["a/x.ts".into()] }];
        let mut b = a.clone();
        b.communities[0].label = "b/…".into();
        let merged = merge(vec![("one".into(), a), ("two".into(), b)]);
        let ids: Vec<usize> = merged.communities.iter().map(|c| c.id).collect();
        assert_eq!(ids, [1, 2]);
        assert!(merged.communities[1].label.starts_with("two: "));
        let communities: BTreeSet<Option<usize>> = merged.nodes.iter().map(|n| n.community).collect();
        assert_eq!(communities, BTreeSet::from([Some(1), Some(2)]));
    }
}
