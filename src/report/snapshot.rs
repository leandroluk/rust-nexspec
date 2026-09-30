//! A read-only picture of the graph that every report section is computed
//! from (`.specs/features/report-command/design.md`). Building it is the only
//! step that touches storage; the analyses are pure functions over it.

use std::collections::{BTreeMap, HashMap};

use crate::graph::edge::{Edge, EdgeType};
use crate::graph::node::NodePayload;
use crate::sync::mutation::StableId;

#[derive(Debug, Clone, Default)]
pub struct GraphSnapshot {
    pub nodes: BTreeMap<StableId, NodePayload>,
    pub edges: Vec<Edge>,
    /// Symbol -> the file that defines it (from `DefinedIn`).
    pub file_of: HashMap<StableId, StableId>,
}

impl GraphSnapshot {
    pub fn new(nodes: BTreeMap<StableId, NodePayload>, edges: Vec<Edge>) -> Self {
        let file_of = edges
            .iter()
            .filter(|e| e.edge_type == EdgeType::DefinedIn)
            .map(|e| (e.from, e.to))
            .collect();
        Self { nodes, edges, file_of }
    }

    /// The file a node belongs to: itself for a file, the defining file for a symbol.
    pub fn file_id_of(&self, id: &StableId) -> Option<StableId> {
        match self.nodes.get(id)? {
            NodePayload::File { .. } => Some(*id),
            NodePayload::Symbol { .. } => self.file_of.get(id).copied(),
            _ => None,
        }
    }

    /// Repository-relative path of a file node, or of a symbol's file.
    pub fn path_of(&self, id: &StableId) -> Option<&str> {
        let file = self.file_id_of(id)?;
        match self.nodes.get(&file)? {
            NodePayload::File { path, .. } => Some(path.as_str()),
            _ => None,
        }
    }

    /// Human-readable name: a file's path, a symbol as `name (path)`, a
    /// requirement/task/ADR by its marker.
    pub fn label(&self, id: &StableId) -> String {
        match self.nodes.get(id) {
            Some(NodePayload::File { path, .. }) => path.clone(),
            Some(NodePayload::Symbol { name, .. }) => match self.path_of(id) {
                Some(path) => format!("{name} ({path})"),
                None => name.clone(),
            },
            Some(NodePayload::Requirement { title, .. } | NodePayload::Task { title, .. } | NodePayload::Adr { title, .. }) => {
                title.clone()
            }
            Some(NodePayload::DocSection { title, .. }) => title.clone(),
            None => "(unknown node)".to_string(),
        }
    }

    pub fn kind_name(&self, id: &StableId) -> &'static str {
        match self.nodes.get(id) {
            Some(NodePayload::File { .. }) => "file",
            Some(NodePayload::Symbol { .. }) => "symbol",
            Some(NodePayload::Requirement { .. }) => "requirement",
            Some(NodePayload::Task { .. }) => "task",
            Some(NodePayload::Adr { .. }) => "adr",
            Some(NodePayload::DocSection { .. }) => "doc_section",
            None => "unknown",
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::graph::edge::Edge;

    pub fn id(n: u8) -> StableId {
        [n; 32]
    }

    pub fn file(path: &str) -> NodePayload {
        NodePayload::File { path: path.to_string(), source_hash: [0; 32] }
    }

    pub fn symbol(name: &str) -> NodePayload {
        NodePayload::Symbol { name: name.to_string(), source_hash: [0; 32], line_start: 0, line_end: 1 }
    }

    pub fn requirement(marker: &str) -> NodePayload {
        NodePayload::Requirement { title: marker.to_string(), source_hash: [0; 32], body: String::new() }
    }

    pub fn task(marker: &str) -> NodePayload {
        NodePayload::Task { title: marker.to_string(), source_hash: [0; 32], body: String::new() }
    }

    pub fn edge(n: u16, from: u8, to: u8, edge_type: EdgeType) -> Edge {
        let mut edge_id = [0u8; 32];
        edge_id[..2].copy_from_slice(&n.to_le_bytes());
        edge_id[31] = 0xEE;
        Edge { id: edge_id, from: id(from), to: id(to), edge_type, meta: 0 }
    }

    /// Files 1..=n named `f{i}.ts` under `dir{i/dirs}` are added with `file`.
    pub fn snapshot(nodes: Vec<(u8, NodePayload)>, edges: Vec<Edge>) -> GraphSnapshot {
        GraphSnapshot::new(nodes.into_iter().map(|(n, p)| (id(n), p)).collect(), edges)
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    #[test]
    fn symbols_resolve_to_their_files_and_labels_name_them() {
        let snap = snapshot(
            vec![(1, file("src/a.ts")), (2, symbol("Alpha")), (3, requirement("REQ-1"))],
            vec![edge(1, 2, 1, EdgeType::DefinedIn)],
        );
        assert_eq!(snap.file_id_of(&id(2)), Some(id(1)));
        assert_eq!(snap.file_id_of(&id(1)), Some(id(1)));
        assert_eq!(snap.file_id_of(&id(3)), None);
        assert_eq!(snap.path_of(&id(2)), Some("src/a.ts"));
        assert_eq!(snap.label(&id(2)), "Alpha (src/a.ts)");
        assert_eq!(snap.label(&id(1)), "src/a.ts");
        assert_eq!(snap.label(&id(3)), "REQ-1");
        assert_eq!(snap.label(&id(9)), "(unknown node)");
        assert_eq!(snap.kind_name(&id(2)), "symbol");
    }
}
