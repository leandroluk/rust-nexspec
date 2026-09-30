//! [`Coordinator`] — the single write path for every store (REQ-006 in
//! `.specs/features/sync-coordinator/spec.md`). Orchestrates: WAL append (durable
//! intent) → fan-out `stage()` to every participant → fan-out `commit()` →
//! bump the global `sync_version` → mark the WAL frame done. See
//! `.specs/features/sync-coordinator/design.md` for the full diagram and the
//! `resume()` recovery semantics (implemented in a later task).

use crate::sync::mutation::MutationSet;
use crate::sync::participant::{SyncError, SyncParticipant};
use crate::sync::version::{VersionError, VersionPointer};
use crate::sync::wal::{Wal, WalError};

impl From<WalError> for SyncError {
    fn from(e: WalError) -> Self {
        SyncError::Storage(e.to_string())
    }
}
impl From<VersionError> for SyncError {
    fn from(e: VersionError) -> Self {
        SyncError::Storage(e.to_string())
    }
}

pub struct Coordinator<'a> {
    wal: Wal,
    version: VersionPointer<'a>,
    participants: Vec<Box<dyn SyncParticipant + 'a>>,
}

impl<'a> Coordinator<'a> {
    pub fn new(
        wal: Wal,
        version: VersionPointer<'a>,
        participants: Vec<Box<dyn SyncParticipant + 'a>>,
    ) -> Self {
        Self {
            wal,
            version,
            participants,
        }
    }

    /// Run one full sync cycle for `mutations`: WAL-log the intent, stage it on
    /// every participant, and — only if all staged successfully — commit
    /// everywhere and make the new version visible. On any participant
    /// failure, every participant is aborted and the version pointer is left
    /// untouched; the WAL frame remains pending for `resume()` to reconcile.
    pub fn stage(&self, mutations: MutationSet) -> Result<u64, SyncError> {
        let target_version = self.version.current()? + 1;

        // REQ-001: durable intent before touching any participant.
        self.wal.append_frame(target_version, &mutations)?;

        if let Err(e) = self.stage_all(target_version, &mutations) {
            self.abort_all(target_version);
            return Err(e);
        }

        self.commit_all(target_version)?;
        self.version.bump(target_version)?;
        self.wal.mark_done(target_version)?;
        // Hygiene only: the cycle is already fully applied and durable, so a
        // failure here must not turn a successful sync into an error. The
        // next cycle simply tries again.
        let _ = self.wal.truncate_if_idle();
        Ok(target_version)
    }

    fn stage_all(&self, target_version: u64, mutations: &MutationSet) -> Result<(), SyncError> {
        for p in &self.participants {
            p.stage(target_version, mutations)?;
        }
        Ok(())
    }

    fn commit_all(&self, target_version: u64) -> Result<(), SyncError> {
        for p in &self.participants {
            p.commit(target_version)?;
        }
        Ok(())
    }

    fn abort_all(&self, target_version: u64) {
        for p in &self.participants {
            // Best-effort: a participant that never staged simply no-ops.
            let _ = p.abort(target_version);
        }
    }

    /// Deterministic crash recovery (REQ-004, REQ-005). Call once at startup,
    /// before serving any read. For every WAL frame still pending (no
    /// commit-marker): re-stage and commit on every participant that hasn't
    /// reached `target_version` yet (idempotent — a participant already there
    /// is skipped), then bump `sync_version` if it hasn't caught up, and mark
    /// the frame done. See `.specs/features/sync-coordinator/design.md` →
    /// "Resume — semântica determinística" for the full decision tree this
    /// implements.
    pub fn resume(&self) -> Result<(), SyncError> {
        for (target_version, mutations) in self.wal.pending_frames()? {
            for p in &self.participants {
                if p.committed_version()? < target_version {
                    p.stage(target_version, &mutations)?;
                    p.commit(target_version)?;
                }
            }
            if self.version.current()? < target_version {
                self.version.bump(target_version)?;
            }
            self.wal.mark_done(target_version)?;
        }
        let _ = self.wal.truncate_if_idle(); // hygiene, see `stage`
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::participant::test_support::TestParticipant;
    use redb::Database;
    use std::sync::Arc;
    use tempfile::NamedTempFile;

    fn setup() -> (NamedTempFile, Database, NamedTempFile) {
        let db_file = NamedTempFile::new().unwrap();
        let db = Database::create(db_file.path()).unwrap();
        let wal_file = NamedTempFile::new().unwrap();
        (db_file, db, wal_file)
    }

    #[test]
    fn happy_path_bumps_version_and_marks_wal_done() {
        let (_db_file, db, wal_file) = setup();
        let wal = Wal::open(wal_file.path()).unwrap();
        let version = VersionPointer::new(&db);

        let p1 = Box::new(TestParticipant::default());
        let p2 = Box::new(TestParticipant::default());
        let coordinator = Coordinator::new(wal, version, vec![p1, p2]);

        let result = coordinator.stage(MutationSet::default());
        assert_eq!(result.unwrap(), 1);

        let version_check = VersionPointer::new(&db);
        assert_eq!(version_check.current().unwrap(), 1);

        let wal_check = Wal::open(wal_file.path()).unwrap();
        assert!(
            wal_check.pending_frames().unwrap().is_empty(),
            "wal frame must be marked done after a successful cycle"
        );
    }

    #[test]
    fn completed_cycles_do_not_accumulate_in_the_wal() {
        let (_db_file, db, wal_file) = setup();
        let wal = Wal::open(wal_file.path()).unwrap();
        let coordinator = Coordinator::new(
            wal,
            VersionPointer::new(&db),
            vec![Box::new(TestParticipant::default())],
        );
        for _ in 0..3 {
            coordinator.stage(MutationSet::default()).unwrap();
            assert_eq!(std::fs::metadata(wal_file.path()).unwrap().len(), 0, "idle WAL is truncated");
        }
        assert_eq!(VersionPointer::new(&db).current().unwrap(), 3);
    }

    #[test]
    fn failed_cycle_keeps_its_frame_and_resume_then_empties_the_wal() {
        let (_db_file, db, wal_file) = setup();
        let wal = Wal::open(wal_file.path()).unwrap();
        let failing = Coordinator::new(
            wal,
            VersionPointer::new(&db),
            vec![Box::new(TestParticipant::failing())],
        );
        assert!(failing.stage(MutationSet::default()).is_err());
        assert!(std::fs::metadata(wal_file.path()).unwrap().len() > 0, "in-flight frame must survive");

        let healthy = Coordinator::new(
            Wal::open(wal_file.path()).unwrap(),
            VersionPointer::new(&db),
            vec![Box::new(TestParticipant::default())],
        );
        healthy.resume().unwrap();
        assert_eq!(VersionPointer::new(&db).current().unwrap(), 1);
        assert_eq!(std::fs::metadata(wal_file.path()).unwrap().len(), 0, "resume leaves an idle WAL empty");
    }

    #[test]
    fn failing_participant_aborts_all_and_leaves_version_untouched() {
        let (_db_file, db, wal_file) = setup();
        let wal = Wal::open(wal_file.path()).unwrap();
        let version = VersionPointer::new(&db);

        let ok_participant = Box::new(TestParticipant::default());
        let failing_participant = Box::new(TestParticipant::failing());
        let coordinator = Coordinator::new(wal, version, vec![ok_participant, failing_participant]);

        let result = coordinator.stage(MutationSet::default());
        assert!(result.is_err());

        let version_check = VersionPointer::new(&db);
        assert_eq!(version_check.current().unwrap(), 0, "version must not bump on failure");

        let wal_check = Wal::open(wal_file.path()).unwrap();
        assert_eq!(
            wal_check.pending_frames().unwrap().len(),
            1,
            "wal frame stays pending for resume() to reconcile"
        );
    }

    #[test]
    fn resume_replays_when_no_participant_applied_the_pending_frame() {
        let (_db_file, db, wal_file) = setup();
        let wal = Wal::open(wal_file.path()).unwrap();
        // Simulate a crash right after the WAL append but before any fan-out.
        wal.append_frame(1, &MutationSet::default()).unwrap();

        let p1 = Arc::new(TestParticipant::default());
        let p2 = Arc::new(TestParticipant::default());
        let coordinator = Coordinator::new(
            Wal::open(wal_file.path()).unwrap(),
            VersionPointer::new(&db),
            vec![Box::new(p1.clone()), Box::new(p2.clone())],
        );

        coordinator.resume().unwrap();

        assert_eq!(p1.committed_version().unwrap(), 1);
        assert_eq!(p2.committed_version().unwrap(), 1);
        assert_eq!(VersionPointer::new(&db).current().unwrap(), 1);
        assert!(Wal::open(wal_file.path()).unwrap().pending_frames().unwrap().is_empty());
    }

    #[test]
    fn resume_finishes_partially_applied_frame() {
        let (_db_file, db, wal_file) = setup();
        let wal = Wal::open(wal_file.path()).unwrap();
        wal.append_frame(1, &MutationSet::default()).unwrap();

        let p1 = Arc::new(TestParticipant::default());
        let p2 = Arc::new(TestParticipant::default());
        // Simulate a crash mid-fan-out: p1 already staged+committed, p2 didn't.
        p1.stage(1, &MutationSet::default()).unwrap();
        p1.commit(1).unwrap();

        let coordinator = Coordinator::new(
            Wal::open(wal_file.path()).unwrap(),
            VersionPointer::new(&db),
            vec![Box::new(p1.clone()), Box::new(p2.clone())],
        );

        coordinator.resume().unwrap();

        assert_eq!(p1.committed_version().unwrap(), 1);
        assert_eq!(p2.committed_version().unwrap(), 1);
        assert_eq!(VersionPointer::new(&db).current().unwrap(), 1);
    }

    #[test]
    fn resume_only_bumps_version_when_all_participants_already_committed() {
        let (_db_file, db, wal_file) = setup();
        let wal = Wal::open(wal_file.path()).unwrap();
        wal.append_frame(1, &MutationSet::default()).unwrap();

        let p1 = Arc::new(TestParticipant::default());
        let p2 = Arc::new(TestParticipant::default());
        // Simulate a crash right after fan-out commit but before the version bump.
        p1.stage(1, &MutationSet::default()).unwrap();
        p1.commit(1).unwrap();
        p2.stage(1, &MutationSet::default()).unwrap();
        p2.commit(1).unwrap();

        let coordinator = Coordinator::new(
            Wal::open(wal_file.path()).unwrap(),
            VersionPointer::new(&db),
            vec![Box::new(p1.clone()), Box::new(p2.clone())],
        );

        coordinator.resume().unwrap();

        assert_eq!(VersionPointer::new(&db).current().unwrap(), 1);
        assert!(Wal::open(wal_file.path()).unwrap().pending_frames().unwrap().is_empty());
    }
}
