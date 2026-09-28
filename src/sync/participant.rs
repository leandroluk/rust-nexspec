//! [`SyncParticipant`] — the contract every physical store (redb, and later the
//! CSR, Tantivy, and HNSW stores) implements to take part in a sync cycle
//! orchestrated by [`crate::sync::coordinator::Coordinator`].
//!
//! **Not stable yet.** This trait is designed before any store besides `redb`
//! exists (see `.specs/features/sync-coordinator/design.md` → Risks). Treat its
//! shape as provisional until the Fase 1 (CSR) implementation validates it in
//! practice.

use crate::sync::mutation::MutationSet;

/// A store that participates in the two-phase staging protocol driven by the
/// [`crate::sync::coordinator::Coordinator`].
///
/// Implementations MUST be idempotent (REQ-005 in
/// `.specs/features/sync-coordinator/spec.md`): calling `stage` and `commit`
/// more than once for the same `target_version` must not duplicate effects —
/// e.g. on resume after a crash, the coordinator may replay the same
/// `MutationSet` against a participant that already applied it.
pub trait SyncParticipant {
    /// Stage `mutations` for `target_version` without making them visible yet.
    /// Safe to call again with the same `target_version` (no-op if already
    /// staged or already committed at or past that version).
    fn stage(&self, target_version: u64, mutations: &MutationSet) -> Result<(), SyncError>;

    /// The highest version this participant has fully committed.
    fn committed_version(&self) -> Result<u64, SyncError>;

    /// Promote the data staged for `target_version` to be the committed state.
    /// Safe to call again with the same `target_version` if already committed.
    fn commit(&self, target_version: u64) -> Result<(), SyncError>;

    /// Discard any data staged for `target_version` without promoting it.
    fn abort(&self, target_version: u64) -> Result<(), SyncError>;
}

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("participant storage error: {0}")]
    Storage(String),
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    /// In-memory [`SyncParticipant`] used by tests in later tasks (T-006/T-007/
    /// T-008) to exercise the coordinator without a real store.
    #[derive(Default)]
    pub struct TestParticipant {
        staged: Mutex<Option<(u64, MutationSet)>>,
        committed: AtomicU64,
        pub fail_stage: bool,
    }

    impl TestParticipant {
        pub fn failing() -> Self {
            Self {
                fail_stage: true,
                ..Default::default()
            }
        }
    }

    impl SyncParticipant for TestParticipant {
        fn stage(&self, target_version: u64, mutations: &MutationSet) -> Result<(), SyncError> {
            if self.fail_stage {
                return Err(SyncError::Storage("simulated stage failure".into()));
            }
            *self.staged.lock().unwrap() = Some((target_version, mutations.clone()));
            Ok(())
        }

        fn committed_version(&self) -> Result<u64, SyncError> {
            Ok(self.committed.load(Ordering::SeqCst))
        }

        fn commit(&self, target_version: u64) -> Result<(), SyncError> {
            self.committed.store(target_version, Ordering::SeqCst);
            *self.staged.lock().unwrap() = None;
            Ok(())
        }

        fn abort(&self, _target_version: u64) -> Result<(), SyncError> {
            *self.staged.lock().unwrap() = None;
            Ok(())
        }
    }

    // Lets tests share one `TestParticipant` instance between the coordinator
    // (which owns `Box<dyn SyncParticipant>`) and direct assertions/mutation
    // in the test body — interior mutability makes this safe.
    impl<T: SyncParticipant> SyncParticipant for std::sync::Arc<T> {
        fn stage(&self, target_version: u64, mutations: &MutationSet) -> Result<(), SyncError> {
            (**self).stage(target_version, mutations)
        }
        fn committed_version(&self) -> Result<u64, SyncError> {
            (**self).committed_version()
        }
        fn commit(&self, target_version: u64) -> Result<(), SyncError> {
            (**self).commit(target_version)
        }
        fn abort(&self, target_version: u64) -> Result<(), SyncError> {
            (**self).abort(target_version)
        }
    }

    #[test]
    fn test_participant_commits_and_reports_version() {
        let p = TestParticipant::default();
        assert_eq!(p.committed_version().unwrap(), 0);
        p.stage(1, &MutationSet::default()).unwrap();
        p.commit(1).unwrap();
        assert_eq!(p.committed_version().unwrap(), 1);
    }
}
