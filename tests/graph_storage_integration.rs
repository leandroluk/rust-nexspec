//! End-to-end integration test for Fase 1 (REQ-109 in
//! `.specs/features/storage-primitives/spec.md`): `markdown::extract()` over
//! a real fixture, its `MutationSet` staged through the Fase 0
//! `Coordinator` with `RedbParticipant` and `CsrParticipant` as the two
//! participants — no mocks anywhere in this test.

use std::sync::Arc;

use nexspec::graph::csr::{Csr, CsrBase, CsrParticipant};
use nexspec::graph::edge::EdgeType;
use nexspec::graph::markdown::extract;
use nexspec::graph::node::NodePayload;
use nexspec::sync::{
    Coordinator, NodeMutation, RedbParticipant, SyncParticipant, VersionPointer, Wal,
};
use redb::Database;
use tempfile::NamedTempFile;

const FIXTURE: &str = r#"
# Storage Primitives

## Requirements
- REQ-201: Nodes must have a stable id

## Tasks
### TASK-201: Implement Node struct
Satisfies REQ-201.
"#;

#[test]
fn extracted_specs_become_queryable_nodes_and_edges_via_coordinator() {
    let set = extract(FIXTURE);
    assert_eq!(set.nodes.len(), 2, "REQ-201 + TASK-201");
    assert_eq!(set.edges.len(), 1, "TASK-201 satisfies REQ-201");

    let node_ids: Vec<_> = set
        .nodes
        .iter()
        .map(|m| match m {
            NodeMutation::Upsert { id, .. } => *id,
            NodeMutation::Remove { .. } => panic!("extract() only upserts"),
        })
        .collect();

    let db_file = NamedTempFile::new().unwrap();
    let db = Database::create(db_file.path()).unwrap();
    let wal_file = NamedTempFile::new().unwrap();
    let wal = Wal::open(wal_file.path()).unwrap();
    let version = VersionPointer::new(&db);

    let csr_file = NamedTempFile::new().unwrap();
    CsrBase::build(&[], csr_file.path()).unwrap();
    let csr_base = CsrBase::open(csr_file.path()).unwrap();
    let csr_participant = CsrParticipant::new(Arc::new(Csr::new(csr_base)), csr_file.path().to_path_buf());
    let csr_handle = csr_participant.csr_handle(); // keep a handle before moving the participant

    let coordinator = Coordinator::new(
        wal,
        version,
        vec![
            Box::new(RedbParticipant::new(&db)),
            Box::new(csr_participant),
        ],
    );

    let target_version = coordinator.stage(set).unwrap();
    assert_eq!(target_version, 1);

    // Nodes ended up in RedbParticipant, queryable by stable id, and decode
    // back into the same NodePayload variant extract() produced.
    let redb = RedbParticipant::new(&db);
    let mut requirement_id = None;
    let mut task_id = None;
    for id in &node_ids {
        let bytes = redb.get_node(id).unwrap().expect("node must be committed");
        let mut aligned = rkyv::util::AlignedVec::<16>::new();
        aligned.extend_from_slice(&bytes);
        let payload = rkyv::from_bytes::<NodePayload, rkyv::rancor::Error>(&aligned).unwrap();
        match payload {
            NodePayload::Requirement { title, .. } => {
                assert_eq!(title, "REQ-201");
                requirement_id = Some(*id);
            }
            NodePayload::Task { title, .. } => {
                assert_eq!(title, "TASK-201");
                task_id = Some(*id);
            }
            other => panic!("unexpected payload variant: {other:?}"),
        }
    }
    let requirement_id = requirement_id.expect("REQ-201 node committed");
    let task_id = task_id.expect("TASK-201 node committed");

    // The Satisfies edge ended up in the CSR, queryable from the task.
    let edges = csr_handle.edges_from(&task_id, EdgeType::Satisfies);
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].to, requirement_id);
}
