//! [`Csr`] — the lock-free public facade over the two-layer CSR (REQ-106 in
//! `.specs/features/storage-primitives/spec.md`). Readers never block on a
//! `sync` publishing a new delta: `edges_from()` takes an atomic snapshot of
//! the current delta (`ArcSwap::load`), then merges it with the immutable
//! base — the merge always sees one whole, self-consistent delta, never a
//! delta half-way through being mutated.

use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::graph::csr::base::CsrBase;
use crate::graph::csr::delta::CsrDelta;
use crate::graph::edge::{Edge, EdgeType};
use crate::sync::mutation::StableId;

pub struct Csr {
    // Both layers are COW/lock-free: `base` also swaps atomically so that
    // compaction (T-107) — which rebuilds and remaps it — never blocks a
    // reader either, not just delta publication.
    base: ArcSwap<CsrBase>,
    delta: ArcSwap<CsrDelta>,
}

impl Csr {
    pub fn new(base: CsrBase) -> Self {
        Self {
            base: ArcSwap::from_pointee(base),
            delta: ArcSwap::from_pointee(CsrDelta::default()),
        }
    }

    /// Merged view of base + the currently published delta.
    pub fn edges_from(&self, from: &StableId, edge_type: EdgeType) -> Vec<Edge> {
        let base = self.base.load();
        let delta = self.delta.load();
        delta.merge_into(&base, from, edge_type)
    }

    /// Atomically swap in `new_delta` as the current one. Readers already
    /// holding a snapshot from `edges_from` keep seeing the old delta until
    /// they call `edges_from` again — never a torn/partial view.
    pub fn publish_delta(&self, new_delta: CsrDelta) {
        self.delta.store(Arc::new(new_delta));
    }

    /// The delta currently published — used by `CsrParticipant` to build the
    /// next version on top of it (COW: never mutates this snapshot).
    pub fn current_delta(&self) -> Arc<CsrDelta> {
        self.delta.load_full()
    }

    pub fn current_base(&self) -> Arc<CsrBase> {
        self.base.load_full()
    }

    /// Every edge in the merged base+delta view, regardless of `from`
    /// (REQ-608 in `.specs/features/cli-mcp-server/spec.md` — finding who
    /// depends on a changed symbol needs a reverse scan, since the CSR only
    /// indexes edges by `from`). Same merge logic as
    /// [`CsrParticipant`](crate::graph::csr::CsrParticipant)'s internal
    /// `compact()`, exposed here as a read-only query rather than
    /// duplicated a third time.
    pub fn all_edges(&self) -> Vec<Edge> {
        let base = self.base.load();
        let delta = self.delta.load();
        let mut all = base.all_edges();
        all.retain(|e| !delta.is_removed(&e.id));
        all.extend(delta.added_edges().iter().cloned());
        all
    }

    /// Publish a freshly compacted base and reset the delta to empty —
    /// everything the old delta held is now folded into `new_base`.
    pub fn replace_base(&self, new_base: CsrBase) {
        self.base.store(Arc::new(new_base));
        self.delta.store(Arc::new(CsrDelta::default()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::edge::Edge;
    use std::thread;
    use tempfile::NamedTempFile;

    fn empty_csr() -> (NamedTempFile, Csr) {
        let file = NamedTempFile::new().unwrap();
        CsrBase::build(&[], file.path()).unwrap();
        let base = CsrBase::open(file.path()).unwrap();
        (file, Csr::new(base))
    }

    #[test]
    fn publish_delta_is_visible_to_subsequent_reads() {
        let (_file, csr) = empty_csr();
        assert!(csr.edges_from(&[1u8; 32], EdgeType::DependsOn).is_empty());

        let mut delta = CsrDelta::default();
        delta.upsert(Edge {
            id: [9u8; 32],
            from: [1u8; 32],
            to: [2u8; 32],
            edge_type: EdgeType::DependsOn,
        });
        csr.publish_delta(delta);

        let edges = csr.edges_from(&[1u8; 32], EdgeType::DependsOn);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, [2u8; 32]);
    }

    #[test]
    fn concurrent_reads_and_publishes_never_panic_or_deadlock() {
        let (_file, csr) = empty_csr();

        thread::scope(|scope| {
            scope.spawn(|| {
                for i in 0u8..200 {
                    let mut delta = CsrDelta::default();
                    delta.upsert(Edge {
                        id: [i; 32],
                        from: [1u8; 32],
                        to: [i; 32],
                        edge_type: EdgeType::DependsOn,
                    });
                    csr.publish_delta(delta);
                }
            });
            scope.spawn(|| {
                for _ in 0u32..500 {
                    let _ = csr.edges_from(&[1u8; 32], EdgeType::DependsOn);
                }
            });
        });

        // No assertion beyond "didn't panic/deadlock" — the invariant is
        // structural (ArcSwap::load always returns a whole Arc<CsrDelta>).
        assert!(!csr.edges_from(&[1u8; 32], EdgeType::DependsOn).is_empty());
    }
}
