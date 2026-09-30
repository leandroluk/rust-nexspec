//! End-to-end integration test for Fase 3 (T-309): a real `Coordinator`
//! with all three real `SyncParticipant`s — `RedbParticipant`,
//! `CsrParticipant`, `TantivyParticipant` — driven by `SyncOrchestrator`
//! over a fixture with both Markdown and Rust code. Confirms consistency
//! across all three stores: the same logical entities are visible in
//! `redb`, the CSR, and the Tantivy index after one sync cycle.

mod fixtures;

use std::sync::Arc;

use fixtures::FixtureRepo;
use nexspec::git::GitSource;
use nexspec::graph::csr::{Csr, CsrBase, CsrParticipant};
use nexspec::graph::edge::EdgeType;
use nexspec::search::{TantivyParticipant, find_by_id, search_text};
use nexspec::sync::{Coordinator, RedbParticipant, VersionPointer, Wal};
use nexspec::sync_orchestrator::SyncOrchestrator;
use redb::Database;
use tempfile::{NamedTempFile, TempDir};

#[test]
fn markdown_and_code_stay_consistent_across_all_three_stores() {
    let repo = FixtureRepo::init();
    repo.write_file(
        "spec.md",
        "## Requirements\n- REQ-901: Greeters must be discoverable\n",
    );
    repo.write_file("greeter.rs", "// @spec REQ-901\nfn greet() {}\n");
    repo.commit("chore: add spec and greeter together");

    let db_file = NamedTempFile::new().unwrap();
    let db = Database::create(db_file.path()).unwrap();

    let csr_file = NamedTempFile::new().unwrap();
    CsrBase::build(&[], csr_file.path()).unwrap();
    let csr_base = CsrBase::open(csr_file.path()).unwrap();
    let csr_participant = CsrParticipant::new(Arc::new(Csr::new(csr_base)), csr_file.path().to_path_buf());
    let csr_handle = csr_participant.csr_handle();

    let tantivy_dir = TempDir::new().unwrap();
    let tantivy_participant = TantivyParticipant::new(tantivy_dir.path()).unwrap();
    let tantivy_handle = tantivy_participant.handle();

    let wal_file = NamedTempFile::new().unwrap();
    let wal = Wal::open(wal_file.path()).unwrap();
    let coordinator = Coordinator::new(
        wal,
        VersionPointer::new(&db),
        vec![
            Box::new(RedbParticipant::new(&db)),
            Box::new(csr_participant),
            Box::new(tantivy_participant),
        ],
    );

    let git = GitSource::open(repo.path()).unwrap();
    let mut orchestrator = SyncOrchestrator::new(git, coordinator, VersionPointer::new(&db));

    let report = orchestrator.run_once().unwrap();
    assert_eq!(report.files_added, 2);
    assert!(report.target_version.is_some());

    let req_id = *blake3::hash(b"marker:REQ-901").as_bytes();
    let symbol_id = *blake3::hash(b"greet@1").as_bytes();

    // 1. redb: both nodes committed.
    let redb = RedbParticipant::new(&db);
    assert!(redb.get_node(&req_id).unwrap().is_some(), "REQ-901 in redb");
    assert!(redb.get_node(&symbol_id).unwrap().is_some(), "greet symbol in redb");

    // 2. CSR: the Satisfies edge is queryable.
    let satisfies = csr_handle.edges_from(&symbol_id, EdgeType::Satisfies);
    assert_eq!(satisfies.len(), 1);
    assert_eq!(satisfies[0].to, req_id);

    // 3. Tantivy: both entities are findable by id and by text.
    assert!(find_by_id(&tantivy_handle, &req_id).unwrap().is_some());
    assert!(find_by_id(&tantivy_handle, &symbol_id).unwrap().is_some());
    let hits = search_text(&tantivy_handle, "discoverable", 10).unwrap();
    assert_eq!(hits.len(), 1, "REQ-901's body text is indexed and searchable");
}
