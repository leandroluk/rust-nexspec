//! The Git merge driver for versioned exports (`*.graph.json`; REQ-1306 in
//! `.specs/features/multi-repo-graph/spec.md`, decision D9).
//!
//! Git calls `nexspec merge-driver %O %A %B`: the common ancestor, our version and theirs. The result is
//! the **union** of ours and theirs, written over ours. Unions cannot express a deletion, so a node or edge
//! removed on one side comes back if the other side still has it; when both sides have a node with the
//! same id but different content, ours wins, which keeps the result the same whichever branch merges.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::export::{ExportGraph, SCHEMA_VERSION};

/// Nodes and edges of both graphs. Communities are ours (their numbering belongs to one export); nodes that
/// only theirs has are left without a community.
pub fn union(ours: &ExportGraph, theirs: &ExportGraph) -> ExportGraph {
    let mut nodes = BTreeMap::new();
    for node in &theirs.nodes {
        let mut node = node.clone();
        node.community = None;
        nodes.insert(node.id.clone(), node);
    }
    for node in &ours.nodes {
        nodes.insert(node.id.clone(), node.clone());
    }
    let mut edges = BTreeSet::new();
    for e in ours.edges.iter().chain(theirs.edges.iter()) {
        edges.insert((e.from.clone(), e.to.clone(), e.relation.clone(), e.context.clone(), e.confidence.clone()));
    }
    ExportGraph {
        schema_version: SCHEMA_VERSION,
        nodes: nodes.into_values().collect(),
        edges: edges
            .into_iter()
            .map(|(from, to, relation, context, confidence)| crate::export::ExportEdge { from, to, relation, confidence, context })
            .collect(),
        communities: ours.communities.clone(),
    }
}

fn read(path: &Path) -> Result<ExportGraph, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    // A side that does not have the file yet (added on one branch) is empty.
    if text.trim().is_empty() {
        return Ok(ExportGraph { schema_version: SCHEMA_VERSION, nodes: vec![], edges: vec![], communities: vec![] });
    }
    ExportGraph::from_json(&text).map_err(|e| format!("{} is not a graph export: {e}", path.display()))
}

/// `nexspec merge-driver <base> <ours> <theirs>`: writes the union over `ours`.
pub fn run(_base: &Path, ours: &Path, theirs: &Path) -> Result<(), String> {
    let merged = union(&read(ours)?, &read(theirs)?);
    std::fs::write(ours, merged.to_json()).map_err(|e| format!("cannot write {}: {e}", ours.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{ExportEdge, ExportNode, ExportPayload};

    fn node(id: &str, label: &str, community: Option<usize>) -> ExportNode {
        ExportNode { id: id.into(), kind: "file".into(), label: label.into(), path: Some(label.into()), community, repo: None, payload: ExportPayload::File { path: label.into(), source_hash: "00".repeat(32) } }
    }

    fn edge(from: &str, to: &str) -> ExportEdge {
        ExportEdge { from: from.into(), to: to.into(), relation: "imports".into(), confidence: "extracted".into(), context: "runtime".into() }
    }

    fn graph(nodes: Vec<ExportNode>, edges: Vec<ExportEdge>) -> ExportGraph {
        ExportGraph { schema_version: 1, nodes, edges, communities: vec![] }
    }

    #[test]
    fn the_union_has_everything_from_both_sides_and_ours_wins_a_conflict() {
        let ours = graph(vec![node("a", "ours-a.ts", Some(1)), node("b", "b.ts", None)], vec![edge("a", "b")]);
        let theirs = graph(vec![node("a", "theirs-a.ts", Some(7)), node("c", "c.ts", Some(7))], vec![edge("a", "b"), edge("c", "a")]);
        let merged = union(&ours, &theirs);
        let labels: Vec<&str> = merged.nodes.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(labels, ["ours-a.ts", "b.ts", "c.ts"]);
        assert_eq!(merged.edges.len(), 2, "the shared edge once");
        assert_eq!(merged.nodes[0].community, Some(1), "ours keeps its numbering");
        assert_eq!(merged.nodes[2].community, None, "a node only theirs has has no community of ours");
    }

    #[test]
    fn the_union_is_symmetric_in_what_it_contains() {
        let a = graph(vec![node("a", "a.ts", None)], vec![edge("a", "b")]);
        let b = graph(vec![node("b", "b.ts", None)], vec![edge("b", "a")]);
        let (ab, ba) = (union(&a, &b), union(&b, &a));
        assert_eq!(ab.nodes.len(), ba.nodes.len());
        assert_eq!(ab.edges, ba.edges);
    }

    #[test]
    fn the_driver_writes_over_ours_and_fails_on_garbage() {
        let dir = tempfile::TempDir::new().unwrap();
        let (base, ours, theirs) = (dir.path().join("base"), dir.path().join("ours"), dir.path().join("theirs"));
        std::fs::write(&base, "").unwrap();
        std::fs::write(&ours, graph(vec![node("a", "a.ts", None)], vec![]).to_json()).unwrap();
        std::fs::write(&theirs, graph(vec![node("b", "b.ts", None)], vec![]).to_json()).unwrap();
        run(&base, &ours, &theirs).unwrap();
        assert_eq!(ExportGraph::from_json(&std::fs::read_to_string(&ours).unwrap()).unwrap().nodes.len(), 2);

        std::fs::write(&theirs, "<<<<<<< not json").unwrap();
        assert!(run(&base, &ours, &theirs).unwrap_err().contains("not a graph export"));
        std::fs::write(&theirs, "").unwrap();
        run(&base, &ours, &theirs).expect("a side without the file counts as empty");
    }
}
