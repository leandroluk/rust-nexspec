//! [`CsrDelta`] — the append-only, in-memory overlay of edge additions/
//! removals since the last compaction (REQ-105). Immutable *snapshots* of it
//! are what get published lock-free via `ArcSwap` in [`crate::graph::csr::Csr`]
//! (T-106) — this type itself is a plain builder; the lock-free/COW property
//! lives in how the coordinator publishes new instances, not in this type's
//! own mutability.

use crate::graph::csr::base::CsrBase;
use crate::graph::edge::{Edge, EdgeType};
use crate::sync::mutation::StableId;

#[derive(Debug, Clone, Default)]
pub struct CsrDelta {
    added: Vec<Edge>,
    removed: Vec<StableId>,
}

impl CsrDelta {
    /// Add or replace an edge. Replacing means the same `id` upserted again
    /// (idempotent from the coordinator's point of view — REQ-005).
    pub fn upsert(&mut self, edge: Edge) {
        self.added.retain(|e| e.id != edge.id);
        self.removed.retain(|id| *id != edge.id);
        self.added.push(edge);
    }

    pub fn remove(&mut self, id: StableId) {
        self.added.retain(|e| e.id != id);
        if !self.removed.contains(&id) {
            self.removed.push(id);
        }
    }

    /// Edges added in this delta matching `from`/`edge_type` — does not
    /// consult the base layer.
    pub fn edges_from(&self, from: &StableId, edge_type: EdgeType) -> impl Iterator<Item = &Edge> {
        self.added
            .iter()
            .filter(move |e| &e.from == from && e.edge_type == edge_type)
    }

    /// The number of edges added in this delta — what
    /// [`crate::graph::csr::participant::CsrParticipant`] compares against
    /// the compaction threshold (REQ-107).
    pub fn len(&self) -> usize {
        self.added.len()
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
    }

    /// All edges currently staged for the next compaction, in an
    /// unspecified order — used by `CsrParticipant::compact()` to fold this
    /// delta into a fresh `CsrBase`.
    pub fn added_edges(&self) -> &[Edge] {
        &self.added
    }

    /// `base` filtered by this delta's removals, plus this delta's additions
    /// — the merged view a reader actually sees.
    pub fn merge_into(&self, base: &CsrBase, from: &StableId, edge_type: EdgeType) -> Vec<Edge> {
        let mut result: Vec<Edge> = base
            .edges_from(from, edge_type)
            .into_iter()
            .filter(|e| !self.removed.contains(&e.id))
            .collect();
        result.extend(self.edges_from(from, edge_type).cloned());
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn edge(id: StableId, from: StableId, to: StableId, edge_type: EdgeType) -> Edge {
        Edge {
            id,
            from,
            to,
            edge_type,
        }
    }

    fn base_with(edges: &[Edge]) -> (NamedTempFile, CsrBase) {
        let file = NamedTempFile::new().unwrap();
        CsrBase::build(edges, file.path()).unwrap();
        let base = CsrBase::open(file.path()).unwrap();
        (file, base)
    }

    #[test]
    fn empty_delta_matches_base_only() {
        let base_edges = vec![edge([1u8; 32], [10u8; 32], [11u8; 32], EdgeType::DependsOn)];
        let (_file, base) = base_with(&base_edges);
        let delta = CsrDelta::default();

        let merged = delta.merge_into(&base, &[10u8; 32], EdgeType::DependsOn);
        assert_eq!(merged, base_edges);
    }

    #[test]
    fn delta_addition_appears_in_merge() {
        let (_file, base) = base_with(&[]);
        let mut delta = CsrDelta::default();
        let added = edge([2u8; 32], [10u8; 32], [12u8; 32], EdgeType::Satisfies);
        delta.upsert(added.clone());

        let merged = delta.merge_into(&base, &[10u8; 32], EdgeType::Satisfies);
        assert_eq!(merged, vec![added]);
    }

    #[test]
    fn delta_removal_hides_base_edge() {
        let removed_edge = edge([3u8; 32], [10u8; 32], [13u8; 32], EdgeType::Implements);
        let (_file, base) = base_with(&[removed_edge.clone()]);
        let mut delta = CsrDelta::default();
        delta.remove(removed_edge.id);

        let merged = delta.merge_into(&base, &[10u8; 32], EdgeType::Implements);
        assert!(merged.is_empty());
    }
}
