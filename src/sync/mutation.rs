//! Mutation types exchanged between the [`crate::sync::coordinator`] and every
//! [`crate::sync::participant::SyncParticipant`].
//!
//! The coordinator never interprets `payload` — it is opaque, participant-owned
//! bytes (e.g. a `NodePayload` for the `redb` participant, an edge record for the
//! future CSR participant). `id` is the entity's stable logical ID (a Blake3
//! content hash), decoupled from any store-specific physical/dense index.

/// Stable logical ID for a node, edge, or doc — a Blake3 hash, not a physical
/// offset. See `.specs/features/sync-coordinator/design.md`.
pub type StableId = [u8; 32];

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum NodeMutation {
    Upsert { id: StableId, payload: Vec<u8> },
    Remove { id: StableId },
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum EdgeMutation {
    Upsert {
        id: StableId,
        from: StableId,
        to: StableId,
        edge_type: u16,
        payload: Vec<u8>,
    },
    Remove {
        id: StableId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum DocMutation {
    Upsert { id: StableId, payload: Vec<u8> },
    Remove { id: StableId },
}

/// One atomic batch of mutations, corresponding to a single sync cycle
/// (one `target_version`). See REQ-001 in
/// `.specs/features/sync-coordinator/spec.md`.
#[derive(Debug, Clone, Default, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MutationSet {
    pub nodes: Vec<NodeMutation>,
    pub edges: Vec<EdgeMutation>,
    pub docs: Vec<DocMutation>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_preserves_data() {
        let set = MutationSet {
            nodes: vec![
                NodeMutation::Upsert {
                    id: [1u8; 32],
                    payload: b"node-payload".to_vec(),
                },
                NodeMutation::Remove { id: [2u8; 32] },
            ],
            edges: vec![EdgeMutation::Upsert {
                id: [3u8; 32],
                from: [1u8; 32],
                to: [2u8; 32],
                edge_type: 7,
                payload: b"edge-payload".to_vec(),
            }],
            docs: vec![DocMutation::Upsert {
                id: [4u8; 32],
                payload: b"doc-payload".to_vec(),
            }],
        };

        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&set).expect("serialize");
        let decoded: MutationSet =
            rkyv::from_bytes::<MutationSet, rkyv::rancor::Error>(&bytes).expect("deserialize");

        assert_eq!(set, decoded);
    }
}
