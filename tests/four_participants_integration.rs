//! End-to-end integration test for Fase 4 (T-408): a real `Coordinator`
//! with all four real `SyncParticipant`s — `RedbParticipant`,
//! `CsrParticipant`, `TantivyParticipant`, `HnswParticipant` — driven
//! directly (not through `SyncOrchestrator`, which doesn't generate
//! embeddings yet). Confirms the fourth participant coexists with the
//! other three without regressing them, using synthetic vectors (no
//! dependency on T-406's real model).
//!
//! Requires the `full` feature (REQ-404) — absent entirely from `lean`
//! builds, along with the `vector` module it exercises.
#![cfg(feature = "full")]

use std::sync::Arc;

use nexspec::graph::csr::{Csr, CsrBase, CsrParticipant};
use nexspec::graph::edge::EdgeType;
use nexspec::graph::node::NodePayload;
use nexspec::search::{TantivyParticipant, find_by_id, search_text};
use nexspec::sync::{Coordinator, EdgeMutation, MutationSet, NodeMutation, RedbParticipant, VersionPointer, Wal};
use nexspec::vector::{HnswParticipant, decode_vector, encode_vector};
use redb::Database;
use tempfile::{NamedTempFile, TempDir};

#[test]
fn four_participants_stay_consistent_after_one_sync_cycle() {
    let requirement_id = [1u8; 32];
    let symbol_id = [2u8; 32];

    let requirement_payload = rkyv::to_bytes::<rkyv::rancor::Error>(&NodePayload::Requirement {
        title: "REQ-1001".to_string(),
        source_hash: requirement_id,
        body: "vector engines must coexist with everything else".to_string(),
    })
    .unwrap()
    .to_vec();
    let symbol_payload = rkyv::to_bytes::<rkyv::rancor::Error>(&NodePayload::Symbol {
        name: "coexist".to_string(),
        source_hash: symbol_id,
        line_start: 1,
        line_end: 2,
    })
    .unwrap()
    .to_vec();

    // Vectors the HnswParticipant will index -- derived deterministically
    // from the same payload bytes so the test can assert an exact match
    // without needing a real embedding model (T-406).
    let requirement_vector = decode_vector(&requirement_payload);
    let symbol_vector = decode_vector(&symbol_payload);

    let mut set = MutationSet::default();
    set.nodes.push(NodeMutation::Upsert {
        id: requirement_id,
        payload: requirement_payload,
    });
    set.nodes.push(NodeMutation::Upsert {
        id: symbol_id,
        payload: symbol_payload,
    });
    // Vectors go through the docs channel.
    set.docs.push(nexspec::sync::DocMutation::Upsert { id: requirement_id, payload: encode_vector(&requirement_vector) });
    set.docs.push(nexspec::sync::DocMutation::Upsert { id: symbol_id, payload: encode_vector(&symbol_vector) });
    set.edges.push(EdgeMutation::Upsert {
        id: [9u8; 32],
        from: symbol_id,
        to: requirement_id,
        edge_type: EdgeType::Satisfies.to_code(),
        payload: vec![],
    });

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

    let hnsw_file = NamedTempFile::new().unwrap();
    let hnsw_participant = HnswParticipant::new(hnsw_file.path()).unwrap();

    let wal_file = NamedTempFile::new().unwrap();
    let wal = Wal::open(wal_file.path()).unwrap();
    let coordinator = Coordinator::new(
        wal,
        VersionPointer::new(&db),
        vec![
            Box::new(RedbParticipant::new(&db)),
            Box::new(csr_participant),
            Box::new(tantivy_participant),
            Box::new(hnsw_participant),
        ],
    );

    let target_version = coordinator.stage(set).unwrap();
    assert_eq!(target_version, 1);

    // 1. redb: both nodes committed.
    let redb = RedbParticipant::new(&db);
    assert!(redb.get_node(&requirement_id).unwrap().is_some());
    assert!(redb.get_node(&symbol_id).unwrap().is_some());

    // 2. CSR: the Satisfies edge is queryable.
    let satisfies = csr_handle.edges_from(&symbol_id, EdgeType::Satisfies);
    assert_eq!(satisfies.len(), 1);
    assert_eq!(satisfies[0].to, requirement_id);

    // 3. Tantivy: findable by id and by text.
    assert!(find_by_id(&tantivy_handle, &requirement_id).unwrap().is_some());
    let hits = search_text(&tantivy_handle, "coexist", 10).unwrap();
    assert!(!hits.is_empty());

    // 4. HNSW: exact-vector search returns each node as its own nearest
    // neighbor (a distance of ~0 for the identical vector).
    let reopened = HnswParticipant::new(hnsw_file.path()).unwrap();
    let req_hits = reopened.index().search(&requirement_vector, 1);
    assert_eq!(req_hits[0].0, requirement_id);
    let sym_hits = reopened.index().search(&symbol_vector, 1);
    assert_eq!(sym_hits[0].0, symbol_id);
}
