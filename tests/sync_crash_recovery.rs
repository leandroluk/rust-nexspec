//! Integration tests for crash recovery (REQ-004, REQ-005, REQ-007). Unlike
//! the unit tests in `src/sync/coordinator.rs` (which use `TestParticipant`
//! mocks), these exercise the real `Wal` + `RedbParticipant` + `redb` files on
//! disk, simulating a process restart at each of the four points in
//! `.specs/features/sync-coordinator/design.md`'s diagram: fresh `Wal`/
//! `Database`/`Coordinator` objects are constructed over the *same* files a
//! "crashed" run left behind, then `resume()` is called and the resulting
//! state is asserted to be fully consistent — never partial.

use redb::Database;
use nexspec::sync::{Coordinator, MutationSet, NodeMutation, RedbParticipant, SyncParticipant, VersionPointer, Wal};
use tempfile::NamedTempFile;

const NODE_ID: [u8; 32] = [9u8; 32];

fn sample_mutations() -> MutationSet {
    MutationSet {
        nodes: vec![NodeMutation::Upsert {
            id: NODE_ID,
            payload: b"payload".to_vec(),
        }],
        edges: vec![],
        docs: vec![],
    }
}

/// Crash point 1: before the WAL frame's `fsync` ever completed — from the
/// next run's point of view, the intent was never durably recorded, so there
/// is nothing to recover. `resume()` must be a safe no-op.
#[test]
fn crash_before_wal_fsync_leaves_nothing_to_recover() {
    let wal_file = NamedTempFile::new().unwrap();
    let db_file = NamedTempFile::new().unwrap();
    let db = Database::create(db_file.path()).unwrap();

    let wal = Wal::open(wal_file.path()).unwrap();
    let version = VersionPointer::new(&db);
    let coordinator = Coordinator::new(wal, version, vec![Box::new(RedbParticipant::new(&db))]);

    coordinator.resume().unwrap();

    assert_eq!(VersionPointer::new(&db).current().unwrap(), 0);
    assert!(RedbParticipant::new(&db).get_node(&NODE_ID).unwrap().is_none());
}

/// Crash point 2: WAL frame fsync'd, but the process died before any
/// participant's `stage()` was called. `resume()` must replay the mutation
/// set from the WAL and bring the participant fully up to date.
#[test]
fn crash_after_wal_before_fanout_recovers_via_resume() {
    let wal_file = NamedTempFile::new().unwrap();
    let db_file = NamedTempFile::new().unwrap();
    let db = Database::create(db_file.path()).unwrap();
    let mutations = sample_mutations();

    Wal::open(wal_file.path())
        .unwrap()
        .append_frame(1, &mutations)
        .unwrap();

    // "Restart": brand new objects over the same on-disk files.
    let wal = Wal::open(wal_file.path()).unwrap();
    let version = VersionPointer::new(&db);
    let coordinator = Coordinator::new(wal, version, vec![Box::new(RedbParticipant::new(&db))]);
    coordinator.resume().unwrap();

    assert_eq!(VersionPointer::new(&db).current().unwrap(), 1);
    assert_eq!(
        RedbParticipant::new(&db).get_node(&NODE_ID).unwrap().unwrap(),
        b"payload"
    );
    assert!(Wal::open(wal_file.path()).unwrap().pending_frames().unwrap().is_empty());
}

/// Crash point 3: mid fan-out — one participant (A) fully staged+committed
/// before the crash, another (B) never touched. Two separate `redb` files
/// stand in for two distinct stores (CSR/Tantivy don't exist until later
/// phases; two `RedbParticipant`s are a faithful enough stand-in for
/// "more than one store"). `resume()` must finish exactly the lagging one.
#[test]
fn crash_mid_fanout_finishes_only_the_lagging_participant() {
    let wal_file = NamedTempFile::new().unwrap();
    let db_a_file = NamedTempFile::new().unwrap();
    let db_b_file = NamedTempFile::new().unwrap();
    let db_a = Database::create(db_a_file.path()).unwrap();
    let db_b = Database::create(db_b_file.path()).unwrap();
    let mutations = sample_mutations();

    Wal::open(wal_file.path())
        .unwrap()
        .append_frame(1, &mutations)
        .unwrap();
    // Participant A finished before the crash; B never started.
    let participant_a = RedbParticipant::new(&db_a);
    participant_a.stage(1, &mutations).unwrap();
    participant_a.commit(1).unwrap();

    let wal = Wal::open(wal_file.path()).unwrap();
    let version = VersionPointer::new(&db_a); // global pointer colocated with A's store
    let coordinator = Coordinator::new(
        wal,
        version,
        vec![
            Box::new(RedbParticipant::new(&db_a)),
            Box::new(RedbParticipant::new(&db_b)),
        ],
    );
    coordinator.resume().unwrap();

    assert_eq!(
        RedbParticipant::new(&db_a).get_node(&NODE_ID).unwrap().unwrap(),
        b"payload"
    );
    assert_eq!(
        RedbParticipant::new(&db_b).get_node(&NODE_ID).unwrap().unwrap(),
        b"payload",
        "lagging participant must have been brought up to date"
    );
    assert_eq!(VersionPointer::new(&db_a).current().unwrap(), 1);
}

/// Crash point 4: every participant committed, but the process died before
/// bumping the global `sync_version`. `resume()` must only need to finalize
/// the pointer and mark the WAL frame done — no re-application of data.
#[test]
fn crash_after_fanout_before_version_bump_only_finalizes_pointer() {
    let wal_file = NamedTempFile::new().unwrap();
    let db_a_file = NamedTempFile::new().unwrap();
    let db_b_file = NamedTempFile::new().unwrap();
    let db_a = Database::create(db_a_file.path()).unwrap();
    let db_b = Database::create(db_b_file.path()).unwrap();
    let mutations = sample_mutations();

    Wal::open(wal_file.path())
        .unwrap()
        .append_frame(1, &mutations)
        .unwrap();
    let participant_a = RedbParticipant::new(&db_a);
    participant_a.stage(1, &mutations).unwrap();
    participant_a.commit(1).unwrap();
    let participant_b = RedbParticipant::new(&db_b);
    participant_b.stage(1, &mutations).unwrap();
    participant_b.commit(1).unwrap();
    // sync_version pointer was never bumped before the crash.
    assert_eq!(VersionPointer::new(&db_a).current().unwrap(), 0);

    let wal = Wal::open(wal_file.path()).unwrap();
    let version = VersionPointer::new(&db_a);
    let coordinator = Coordinator::new(
        wal,
        version,
        vec![
            Box::new(RedbParticipant::new(&db_a)),
            Box::new(RedbParticipant::new(&db_b)),
        ],
    );
    coordinator.resume().unwrap();

    assert_eq!(VersionPointer::new(&db_a).current().unwrap(), 1);
    assert!(Wal::open(wal_file.path()).unwrap().pending_frames().unwrap().is_empty());
}
