//! [`CsrParticipant`] — the CSR's [`crate::sync::SyncParticipant`]
//! implementation (REQ-106, REQ-107, REQ-109). The first real consumer of the
//! trait designed in Fase 0 besides `RedbParticipant` — see
//! `.specs/features/sync-coordinator/design.md` → Risks for why that contract
//! was considered provisional until this point.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::graph::csr::base::{CsrBase, CsrError};
use crate::graph::csr::delta::CsrDelta;
use crate::graph::csr::facade::Csr;
use crate::graph::edge::{Edge, EdgeType};
use crate::sync::mutation::{EdgeMutation, MutationSet};
use crate::sync::participant::{SyncError, SyncParticipant};

fn storage_err<E: std::fmt::Display>(e: E) -> SyncError {
    SyncError::Storage(e.to_string())
}
impl From<CsrError> for SyncError {
    fn from(e: CsrError) -> Self {
        storage_err(e)
    }
}

/// Default compaction threshold (REQ-107): fold the delta into a fresh base
/// once it reaches 5% of the base's current edge count.
const DEFAULT_COMPACTION_THRESHOLD_RATIO: f64 = 0.05;

pub struct CsrParticipant {
    // `Arc` so callers can keep a shared handle to query `Csr` (e.g. after
    // this participant has been moved into a `Coordinator`'s participant
    // list) — the whole point of Csr being lock-free is that reads don't
    // need to go through the coordinator at all.
    csr: Arc<Csr>,
    /// Path to the base `edges.bin` file — compaction writes to
    /// `<path>.staging` and renames it here atomically.
    base_path: PathBuf,
    staging: Mutex<Option<(u64, CsrDelta)>>,
    committed_version: AtomicU64,
    compaction_threshold_ratio: f64,
}

impl CsrParticipant {
    pub fn new(csr: Arc<Csr>, base_path: PathBuf) -> Self {
        Self {
            csr,
            base_path,
            staging: Mutex::new(None),
            committed_version: AtomicU64::new(0),
            compaction_threshold_ratio: DEFAULT_COMPACTION_THRESHOLD_RATIO,
        }
    }

    pub fn csr(&self) -> &Csr {
        &self.csr
    }

    /// A cloneable, shared handle to the same `Csr` this participant writes
    /// to — keep this before moving the participant into a `Coordinator` if
    /// you need to query it afterwards.
    pub fn csr_handle(&self) -> Arc<Csr> {
        Arc::clone(&self.csr)
    }

    /// Force compaction outside the automatic threshold (REQ-604 in
    /// `.specs/features/cli-mcp-server/spec.md`, `nexspec compact`) — a
    /// one-line wrapper since compaction isn't a mutation cycle (it doesn't
    /// touch the WAL or `sync_version`, so it needs no `Coordinator`).
    pub fn compact_now(&self) -> Result<(), SyncError> {
        self.compact()
    }

    fn staging_path(&self) -> PathBuf {
        let mut s = self.base_path.clone().into_os_string();
        s.push(".staging");
        PathBuf::from(s)
    }

    /// Fold the currently published base + delta into a fresh base file,
    /// rename it atomically into place, remap it, and reset the delta to
    /// empty (REQ-107).
    fn compact(&self) -> Result<(), SyncError> {
        let base = self.csr.current_base();
        let delta = self.csr.current_delta();

        let mut all = base.all_edges();
        all.retain(|e| !delta.is_removed(&e.id));
        all.extend(delta.added_edges().iter().cloned());

        let staging_path = self.staging_path();
        CsrBase::build(&all, &staging_path)?;
        std::fs::rename(&staging_path, &self.base_path).map_err(storage_err)?;
        let new_base = CsrBase::open(&self.base_path)?;
        self.csr.replace_base(new_base);
        Ok(())
    }
}

impl SyncParticipant for CsrParticipant {
    fn stage(&self, target_version: u64, mutations: &MutationSet) -> Result<(), SyncError> {
        if self.committed_version()? >= target_version {
            return Ok(()); // idempotent: already applied at or past this version
        }

        // Build the next delta on top of whatever is currently published —
        // accumulates across cycles until compact() folds it into the base.
        let mut next_delta: CsrDelta = (*self.csr.current_delta()).clone();
        for m in &mutations.edges {
            match m {
                EdgeMutation::Upsert {
                    id,
                    from,
                    to,
                    edge_type,
                    payload,
                } => {
                    let edge_type = EdgeType::from_code(*edge_type).ok_or_else(|| {
                        SyncError::Storage(format!("unknown edge_type code {edge_type}"))
                    })?;
                    next_delta.upsert(Edge {
                        id: *id,
                        from: *from,
                        to: *to,
                        edge_type,
                        // `payload[0]` carries confidence/context (see `encode_meta`).
                        meta: payload.first().copied().unwrap_or(0),
                    });
                }
                EdgeMutation::Remove { id } => {
                    next_delta.remove(*id);
                }
            }
        }

        *self.staging.lock().unwrap() = Some((target_version, next_delta));
        Ok(())
    }

    fn committed_version(&self) -> Result<u64, SyncError> {
        Ok(self.committed_version.load(Ordering::SeqCst))
    }

    fn commit(&self, target_version: u64) -> Result<(), SyncError> {
        if self.committed_version()? >= target_version {
            return Ok(()); // idempotent: already committed at or past this version
        }

        let staged = self.staging.lock().unwrap().take();
        let Some((staged_version, delta)) = staged else {
            return Err(SyncError::Storage(format!(
                "commit({target_version}) called with nothing staged"
            )));
        };
        if staged_version != target_version {
            return Err(SyncError::Storage(format!(
                "staged version {staged_version} does not match commit target {target_version}"
            )));
        }

        let delta_len = delta.len();
        self.csr.publish_delta(delta);
        self.committed_version.store(target_version, Ordering::SeqCst);

        let base_count = self.csr.current_base().edge_count().max(1);
        if delta_len as f64 / base_count as f64 >= self.compaction_threshold_ratio {
            self.compact()?;
        }
        Ok(())
    }

    fn abort(&self, target_version: u64) -> Result<(), SyncError> {
        let mut staging = self.staging.lock().unwrap();
        if matches!(&*staging, Some((v, _)) if *v == target_version) {
            *staging = None;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn edge_upsert(id: [u8; 32], from: [u8; 32], to: [u8; 32], edge_type: EdgeType) -> MutationSet {
        MutationSet {
            nodes: vec![],
            edges: vec![EdgeMutation::Upsert {
                id,
                from,
                to,
                edge_type: edge_type.to_code(),
                payload: vec![],
            }],
            docs: vec![],
        }
    }

    fn participant_over_empty_base() -> (NamedTempFile, CsrParticipant) {
        let file = NamedTempFile::new().unwrap();
        CsrBase::build(&[], file.path()).unwrap();
        let base = CsrBase::open(file.path()).unwrap();
        let participant = CsrParticipant::new(std::sync::Arc::new(Csr::new(base)), file.path().to_path_buf());
        (file, participant)
    }

    #[test]
    fn stage_then_commit_makes_edge_visible() {
        let (_file, p) = participant_over_empty_base();
        let set = edge_upsert([1u8; 32], [10u8; 32], [11u8; 32], EdgeType::DependsOn);

        p.stage(1, &set).unwrap();
        assert!(p.csr().edges_from(&[10u8; 32], EdgeType::DependsOn).is_empty());

        p.commit(1).unwrap();
        let edges = p.csr().edges_from(&[10u8; 32], EdgeType::DependsOn);
        assert_eq!(edges.len(), 1);
        assert_eq!(p.committed_version().unwrap(), 1);
    }

    #[test]
    fn stage_then_abort_discards_edge() {
        let (_file, p) = participant_over_empty_base();
        let set = edge_upsert([2u8; 32], [10u8; 32], [12u8; 32], EdgeType::Satisfies);

        p.stage(5, &set).unwrap();
        p.abort(5).unwrap();

        assert!(p.csr().edges_from(&[10u8; 32], EdgeType::Satisfies).is_empty());
        assert_eq!(p.committed_version().unwrap(), 0);
    }

    #[test]
    fn stage_is_idempotent_and_commit_does_not_duplicate() {
        let (_file, p) = participant_over_empty_base();
        let set = edge_upsert([3u8; 32], [10u8; 32], [13u8; 32], EdgeType::Implements);

        p.stage(1, &set).unwrap();
        p.stage(1, &set).unwrap(); // called twice — must not error or duplicate
        p.commit(1).unwrap();
        p.commit(1).unwrap(); // replay after "crash" — idempotent no-op

        assert_eq!(p.csr().edges_from(&[10u8; 32], EdgeType::Implements).len(), 1);
        assert_eq!(p.committed_version().unwrap(), 1);
    }

    #[test]
    fn delta_growth_past_threshold_triggers_automatic_compaction() {
        // Pre-build a base with 100 edges so the 5% threshold means "5 edges".
        let file = NamedTempFile::new().unwrap();
        let base_edges: Vec<Edge> = (0u8..100)
            .map(|i| Edge {
                id: [i; 32],
                from: [200u8; 32],
                to: [i; 32],
                edge_type: EdgeType::DefinedIn,
                meta: 0,
            })
            .collect();
        CsrBase::build(&base_edges, file.path()).unwrap();
        let base = CsrBase::open(file.path()).unwrap();
        let p = CsrParticipant::new(std::sync::Arc::new(Csr::new(base)), file.path().to_path_buf());
        assert_eq!(p.csr().current_base().edge_count(), 100);

        // Below threshold (1 edge / 100 = 1%): no compaction yet.
        let set = edge_upsert([101u8; 32], [10u8; 32], [20u8; 32], EdgeType::DependsOn);
        p.stage(1, &set).unwrap();
        p.commit(1).unwrap();
        assert_eq!(p.csr().current_base().edge_count(), 100, "still below threshold");
        assert_eq!(p.csr().current_delta().len(), 1);

        // Add 4 more (delta reaches 5 / 100 = 5% => threshold hit on this commit).
        let mut more = MutationSet::default();
        for i in 102u8..106 {
            more.edges.push(EdgeMutation::Upsert {
                id: [i; 32],
                from: [10u8; 32],
                to: [i; 32],
                edge_type: EdgeType::DependsOn.to_code(),
                payload: vec![],
            });
        }
        p.stage(2, &more).unwrap();
        p.commit(2).unwrap();

        assert_eq!(
            p.csr().current_delta().len(),
            0,
            "compaction must reset the delta to empty"
        );
        assert_eq!(
            p.csr().current_base().edge_count(),
            105,
            "compaction must fold the delta's 5 edges into the base"
        );
        // Data must still be queryable after compaction (base+delta merged).
        assert_eq!(p.csr().edges_from(&[10u8; 32], EdgeType::DependsOn).len(), 5);
    }

    #[test]
    fn edge_meta_travels_from_the_payload_and_survives_compaction() {
        use crate::graph::edge::{Confidence, EdgeContext, encode_meta};
        let (_file, p) = participant_over_empty_base();
        let meta = encode_meta(Confidence::Inferred, EdgeContext::Test);
        let set = MutationSet {
            nodes: vec![],
            edges: vec![EdgeMutation::Upsert {
                id: [7u8; 32],
                from: [10u8; 32],
                to: [14u8; 32],
                edge_type: EdgeType::Imports.to_code(),
                payload: vec![meta],
            }],
            docs: vec![],
        };
        p.stage(1, &set).unwrap();
        p.commit(1).unwrap();

        let in_delta = p.csr().edges_from(&[10u8; 32], EdgeType::Imports);
        assert_eq!(in_delta[0].meta, meta);
        assert_eq!(in_delta[0].confidence(), Confidence::Inferred);
        assert_eq!(in_delta[0].context(), EdgeContext::Test);

        p.compact_now().unwrap();
        let in_base = p.csr().edges_from(&[10u8; 32], EdgeType::Imports);
        assert_eq!(in_base.len(), 1);
        assert_eq!(in_base[0].meta, meta, "meta is stored in the base file too");
    }
}
