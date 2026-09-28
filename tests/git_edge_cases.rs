//! Integration tests for non-trivial repository states (T-208, REQ-208):
//! detached HEAD and a dirty working tree, driven through the real
//! `SyncOrchestrator` end-to-end — not just `GitSource` in isolation
//! (already covered by `tests/git_source.rs`).

mod fixtures;

use fixtures::FixtureRepo;
use nexspec::git::GitSource;
use nexspec::graph::csr::{Csr, CsrBase, CsrParticipant};
use nexspec::sync::{Coordinator, RedbParticipant, VersionPointer, Wal};
use nexspec::sync_orchestrator::SyncOrchestrator;
use redb::Database;
use std::sync::Arc;
use tempfile::NamedTempFile;

/// Returns the orchestrator plus the temp files its CSR/WAL are backed by --
/// callers must keep both temp files alive (not just the orchestrator) for
/// as long as they use it, since `CsrBase` mmaps its file directly.
fn build_orchestrator<'a>(
    repo_path: &std::path::Path,
    db: &'a Database,
) -> (SyncOrchestrator<'a>, NamedTempFile, NamedTempFile) {
    let csr_file = NamedTempFile::new().unwrap();
    CsrBase::build(&[], csr_file.path()).unwrap();
    let csr_base = CsrBase::open(csr_file.path()).unwrap();
    let csr_participant = CsrParticipant::new(Arc::new(Csr::new(csr_base)), csr_file.path().to_path_buf());

    let wal_file = NamedTempFile::new().unwrap();
    let wal = Wal::open(wal_file.path()).unwrap();

    let coordinator = Coordinator::new(
        wal,
        VersionPointer::new(db),
        vec![Box::new(RedbParticipant::new(db)), Box::new(csr_participant)],
    );
    let git = GitSource::open(repo_path).unwrap();
    let orchestrator = SyncOrchestrator::new(git, coordinator, VersionPointer::new(db));
    (orchestrator, csr_file, wal_file)
}

#[test]
fn run_once_succeeds_on_a_detached_head() {
    let repo = FixtureRepo::init();
    repo.write_file("spec1.md", "## Requirements\n- REQ-501: first\n");
    let first_oid = repo.commit("chore: first");
    repo.write_file("spec2.md", "## Requirements\n- REQ-502: second\n");
    repo.commit("chore: second");

    repo.checkout_detached(&first_oid);

    let db_file = NamedTempFile::new().unwrap();
    let db = Database::create(db_file.path()).unwrap();
    let (mut orchestrator, _csr_file, _wal_file) = build_orchestrator(repo.path(), &db);

    let report = orchestrator.run_once().unwrap();
    assert_eq!(report.files_added, 1, "only spec1.md is reachable from the detached HEAD");
    assert!(report.target_version.is_some());
}

#[test]
fn run_once_picks_up_dirty_working_tree_changes() {
    let repo = FixtureRepo::init();
    repo.write_file("spec1.md", "## Requirements\n- REQ-601: first\n");
    repo.commit("chore: first");

    let db_file = NamedTempFile::new().unwrap();
    let db = Database::create(db_file.path()).unwrap();
    let (mut orchestrator, _csr_file, _wal_file) = build_orchestrator(repo.path(), &db);

    // First run: the tree is clean right after a fresh commit, so
    // `GitSource::is_dirty()` short-circuits before `DirtyCache` even scans
    // -- no unnecessary Blake3 hashing when git already knows there's
    // nothing uncommitted.
    let first = orchestrator.run_once().unwrap();
    assert_eq!(first.files_dirty, 0);

    // No new commit, but edit the file without committing.
    repo.write_file("spec1.md", "## Requirements\n- REQ-601: first, edited\n");
    let second = orchestrator.run_once().unwrap();

    assert_eq!(second.files_added, 0, "no new commit");
    assert_eq!(second.files_modified, 0, "no new commit");
    assert_eq!(second.files_dirty, 1, "the uncommitted edit must be picked up");
    assert!(
        second.target_version.is_some(),
        "a dirty-only cycle must still stage something"
    );
}
