//! Integration test for code-file routing in `SyncOrchestrator::run_once`
//! (T-306, REQ-306): a single commit with both a `.md` spec and a `.rs`
//! file must produce nodes of both kinds, and the code symbol's
//! `@spec REQ-XXX` annotation must resolve against the Markdown-extracted
//! requirement from the *same* cycle (REQ-304).

mod fixtures;

use std::sync::Arc;

use fixtures::FixtureRepo;
use nexspec::graph::csr::{Csr, CsrBase, CsrParticipant};
use nexspec::graph::edge::EdgeType;
use nexspec::sync::{Coordinator, RedbParticipant, VersionPointer, Wal};
use nexspec::sync_orchestrator::SyncOrchestrator;
use redb::Database;
use tempfile::NamedTempFile;

#[test]
fn markdown_and_code_nodes_both_appear_and_satisfies_resolves() {
    let repo = FixtureRepo::init();
    repo.write_file(
        "spec.md",
        "## Requirements\n- REQ-801: Widgets must be greetable\n",
    );
    repo.write_file(
        "widget.rs",
        "// @spec REQ-801\nfn greet_widget() {}\n",
    );
    repo.commit("chore: add spec and code together");

    let db_file = NamedTempFile::new().unwrap();
    let db = Database::create(db_file.path()).unwrap();
    let csr_file = NamedTempFile::new().unwrap();
    CsrBase::build(&[], csr_file.path()).unwrap();
    let csr_base = CsrBase::open(csr_file.path()).unwrap();
    let csr_participant = CsrParticipant::new(Arc::new(Csr::new(csr_base)), csr_file.path().to_path_buf());
    let csr_handle = csr_participant.csr_handle();

    let wal_file = NamedTempFile::new().unwrap();
    let wal = Wal::open(wal_file.path()).unwrap();
    let coordinator = Coordinator::new(
        wal,
        VersionPointer::new(&db),
        vec![Box::new(RedbParticipant::new(&db)), Box::new(csr_participant)],
    );

    let git = nexspec::git::GitSource::open(repo.path()).unwrap();
    let mut orchestrator = SyncOrchestrator::new(git, coordinator, VersionPointer::new(&db));

    let report = orchestrator.run_once().unwrap();
    assert_eq!(report.files_added, 2, "spec.md + widget.rs");
    assert!(report.target_version.is_some());

    // Find the requirement's and the symbol's stable ids by scanning redb's
    // committed nodes -- simplest way to assert on real ids without
    // duplicating extract()'s internal hashing here.
    let req_id = *blake3::hash(b"marker:REQ-801").as_bytes();
    let redb = RedbParticipant::new(&db);
    assert!(redb.get_node(&req_id).unwrap().is_some(), "REQ-801 node committed");

    // The Symbol node's id depends only on name+line, computable directly.
    let symbol_id = *blake3::hash(b"greet_widget@1").as_bytes();
    assert!(
        redb.get_node(&symbol_id).unwrap().is_some(),
        "greet_widget symbol node committed"
    );

    let satisfies = csr_handle.edges_from(&symbol_id, EdgeType::Satisfies);
    assert_eq!(satisfies.len(), 1);
    assert_eq!(satisfies[0].to, req_id);
}
