//! Integration test for `SyncOrchestrator::run_once` (T-207, REQ-205).

mod fixtures;

use fixtures::FixtureRepo;
use nexspec::git::GitSource;
use nexspec::graph::csr::{Csr, CsrBase, CsrParticipant};
use nexspec::sync::{Coordinator, RedbParticipant, VersionPointer};
use nexspec::sync_orchestrator::SyncOrchestrator;
use redb::Database;
use std::sync::Arc;
use tempfile::NamedTempFile;

#[test]
fn run_once_twice_only_processes_new_commits() {
    let repo = FixtureRepo::init();
    repo.write_file(
        "spec1.md",
        "## Requirements\n- REQ-401: first requirement\n",
    );
    repo.commit("chore: first spec");

    let db_file = NamedTempFile::new().unwrap();
    let db = Database::create(db_file.path()).unwrap();
    let csr_file = NamedTempFile::new().unwrap();
    CsrBase::build(&[], csr_file.path()).unwrap();
    let csr_base = CsrBase::open(csr_file.path()).unwrap();
    let csr_participant = CsrParticipant::new(Arc::new(Csr::new(csr_base)), csr_file.path().to_path_buf());
    let csr_handle = csr_participant.csr_handle();

    let wal_file = NamedTempFile::new().unwrap();
    let wal = nexspec::sync::Wal::open(wal_file.path()).unwrap();
    let coordinator = Coordinator::new(
        wal,
        VersionPointer::new(&db),
        vec![Box::new(RedbParticipant::new(&db)), Box::new(csr_participant)],
    );

    let git = GitSource::open(repo.path()).unwrap();
    let mut orchestrator = SyncOrchestrator::new(git, coordinator, VersionPointer::new(&db));

    let first_report = orchestrator.run_once().unwrap();
    assert_eq!(first_report.files_added, 1);
    assert!(first_report.target_version.is_some());

    let redb = RedbParticipant::new(&db);
    let req_id = *blake3::hash(b"marker:REQ-401").as_bytes();
    assert!(
        redb.get_node(&req_id).unwrap().is_some(),
        "REQ-401 must be committed after the first run"
    );

    // Second run with no new commits must be a no-op (empty diff -> no stage).
    let second_report = orchestrator.run_once().unwrap();
    assert_eq!(second_report.files_added, 0);
    assert_eq!(second_report.files_modified, 0);
    assert!(
        second_report.target_version.is_none(),
        "nothing changed since the last index -- no mutation set should be staged"
    );

    // A new commit is picked up on the third run, not reprocessing the first.
    repo.write_file("spec2.md", "## Requirements\n- REQ-402: second requirement\n");
    repo.commit("chore: second spec");

    let third_report = orchestrator.run_once().unwrap();
    assert_eq!(third_report.files_added, 1, "only the new file, not spec1.md again");

    let _ = csr_handle; // kept alive for potential future assertions
}

#[test]
fn run_once_reports_phase_timings_and_staged_counts() {
    let repo = FixtureRepo::init();
    repo.write_file("spec.md", "## Requirements
- REQ-410: timed requirement
");
    repo.write_file("a.ts", "export class Alpha {}
");
    repo.write_file("b.ts", "export class Beta {}
");
    repo.commit("chore: seed");

    let db_file = NamedTempFile::new().unwrap();
    let db = Database::create(db_file.path()).unwrap();
    let wal_file = NamedTempFile::new().unwrap();
    let wal = nexspec::sync::Wal::open(wal_file.path()).unwrap();
    let coordinator = Coordinator::new(
        wal,
        VersionPointer::new(&db),
        vec![Box::new(RedbParticipant::new(&db))],
    );
    let git = GitSource::open(repo.path()).unwrap();
    let mut orchestrator = SyncOrchestrator::new(git, coordinator, VersionPointer::new(&db));

    let first = orchestrator.run_once().unwrap();
    let t = &first.timings;
    assert!(t.nodes >= 3, "3 file nodes at least, got {}", t.nodes);
    assert!(t.co_change_edges > 0, "one commit touching 3 files yields co-change edges");
    assert!(t.edges >= t.co_change_edges);
    assert!(t.stage > std::time::Duration::ZERO);

    let second = orchestrator.run_once().unwrap();
    assert_eq!(second.timings.nodes, 0);
    assert_eq!(second.timings.edges, 0, "no-op sync stages nothing");
}

#[test]
fn engine_artifacts_never_count_as_dirty() {
    let repo = FixtureRepo::init(); // deliberately no .gitignore
    repo.write_file("spec.md", "## Requirements
- REQ-420: real spec
");
    repo.commit("chore: spec");
    repo.write_file(".specs/.index/edges.bin", "binary-ish");
    repo.write_file(".models/model.onnx", "weights");

    let db_file = NamedTempFile::new().unwrap();
    let db = Database::create(db_file.path()).unwrap();
    let wal_file = NamedTempFile::new().unwrap();
    let wal = nexspec::sync::Wal::open(wal_file.path()).unwrap();
    let coordinator = Coordinator::new(
        wal,
        VersionPointer::new(&db),
        vec![Box::new(RedbParticipant::new(&db))],
    );
    let git = GitSource::open(repo.path()).unwrap();
    let mut orchestrator = SyncOrchestrator::new(git, coordinator, VersionPointer::new(&db));

    let first = orchestrator.run_once().unwrap();
    assert_eq!(first.files_dirty, 0, "index/model files are not source");
    let second = orchestrator.run_once().unwrap();
    assert_eq!(second.files_dirty, 0);
    assert!(second.target_version.is_none(), "no-op sync must not stage a new frame");
}
