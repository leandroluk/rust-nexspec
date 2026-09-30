//! The graph as queries see it: a [`GraphSnapshot`] plus adjacency lists in
//! both directions, so neighbours are a lookup instead of a scan.

use std::collections::HashMap;

use crate::graph::edge::Edge;
use crate::report::snapshot::GraphSnapshot;
use crate::sync::mutation::StableId;

#[derive(Debug, Clone, Default)]
pub struct GraphView {
    pub snapshot: GraphSnapshot,
    /// Node -> indices (into `snapshot.edges`) of edges leaving it.
    fwd: HashMap<StableId, Vec<usize>>,
    /// Node -> indices of edges arriving at it.
    rev: HashMap<StableId, Vec<usize>>,
}

impl GraphView {
    pub fn new(snapshot: GraphSnapshot) -> Self {
        let mut fwd: HashMap<StableId, Vec<usize>> = HashMap::new();
        let mut rev: HashMap<StableId, Vec<usize>> = HashMap::new();
        for (i, edge) in snapshot.edges.iter().enumerate() {
            fwd.entry(edge.from).or_default().push(i);
            rev.entry(edge.to).or_default().push(i);
        }
        Self { snapshot, fwd, rev }
    }

    /// Edges leaving `id`, in storage order.
    pub fn out_edges(&self, id: &StableId) -> impl Iterator<Item = &Edge> {
        self.fwd.get(id).into_iter().flatten().map(|&i| &self.snapshot.edges[i])
    }

    /// Edges arriving at `id`, in storage order.
    pub fn in_edges(&self, id: &StableId) -> impl Iterator<Item = &Edge> {
        self.rev.get(id).into_iter().flatten().map(|&i| &self.snapshot.edges[i])
    }

    pub fn contains(&self, id: &StableId) -> bool {
        self.snapshot.nodes.contains_key(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::edge::EdgeType;
    use crate::report::snapshot::test_support::*;

    #[test]
    fn adjacency_is_available_in_both_directions() {
        let view = GraphView::new(snapshot(
            vec![(1, file("a.ts")), (2, file("b.ts")), (3, file("c.ts"))],
            vec![edge(1, 1, 2, EdgeType::Imports), edge(2, 3, 2, EdgeType::Imports), edge(3, 2, 3, EdgeType::Imports)],
        ));
        let out_of_2: Vec<StableId> = view.out_edges(&id(2)).map(|e| e.to).collect();
        assert_eq!(out_of_2, vec![id(3)]);
        let into_2: Vec<StableId> = view.in_edges(&id(2)).map(|e| e.from).collect();
        assert_eq!(into_2, vec![id(1), id(3)]);
        assert_eq!(view.out_edges(&id(9)).count(), 0, "unknown nodes have no edges");
        assert!(view.contains(&id(1)) && !view.contains(&id(9)));
    }
}
