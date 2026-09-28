//! Typed node entities (REQ-101, REQ-102 in
//! `.specs/features/storage-primitives/spec.md`). `Node::id` is a
//! [`crate::sync::mutation::StableId`] (Blake3 hash) — never a physical/dense
//! index; the dense `u32` index (REQ-103) is assigned only by
//! [`crate::graph::csr::CsrBase`] at build/compaction time.

use crate::sync::mutation::StableId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum NodeType {
    Requirement,
    Task,
    Adr,
    DocSection,
    Symbol,
    File,
}

/// One variant per [`NodeType`], serialized with `rkyv` for consistency with
/// the rest of the storage stack (WAL, CSR).
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum NodePayload {
    Requirement {
        title: String,
        source_hash: [u8; 32],
        body: String,
    },
    Task {
        title: String,
        source_hash: [u8; 32],
        body: String,
    },
    Adr {
        title: String,
        source_hash: [u8; 32],
        body: String,
    },
    DocSection {
        title: String,
        source_hash: [u8; 32],
    },
    Symbol {
        name: String,
        source_hash: [u8; 32],
    },
    File {
        path: String,
        source_hash: [u8; 32],
    },
}

impl NodePayload {
    pub fn node_type(&self) -> NodeType {
        match self {
            NodePayload::Requirement { .. } => NodeType::Requirement,
            NodePayload::Task { .. } => NodeType::Task,
            NodePayload::Adr { .. } => NodeType::Adr,
            NodePayload::DocSection { .. } => NodeType::DocSection,
            NodePayload::Symbol { .. } => NodeType::Symbol,
            NodePayload::File { .. } => NodeType::File,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Node {
    pub id: StableId,
    pub node_type: NodeType,
    pub payload: NodePayload,
}

impl Node {
    pub fn new(id: StableId, payload: NodePayload) -> Self {
        Self {
            id,
            node_type: payload.node_type(),
            payload,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_nodes() -> Vec<Node> {
        vec![
            Node::new(
                [1u8; 32],
                NodePayload::Requirement {
                    title: "REQ-001".into(),
                    source_hash: [2u8; 32],
                    body: "must do X".into(),
                },
            ),
            Node::new(
                [3u8; 32],
                NodePayload::Task {
                    title: "T-001".into(),
                    source_hash: [4u8; 32],
                    body: "implement X".into(),
                },
            ),
            Node::new(
                [5u8; 32],
                NodePayload::Adr {
                    title: "ADR-001".into(),
                    source_hash: [6u8; 32],
                    body: "we chose X".into(),
                },
            ),
            Node::new(
                [7u8; 32],
                NodePayload::DocSection {
                    title: "Overview".into(),
                    source_hash: [8u8; 32],
                },
            ),
            Node::new(
                [9u8; 32],
                NodePayload::Symbol {
                    name: "fn foo".into(),
                    source_hash: [10u8; 32],
                },
            ),
            Node::new(
                [11u8; 32],
                NodePayload::File {
                    path: "src/lib.rs".into(),
                    source_hash: [12u8; 32],
                },
            ),
        ]
    }

    #[test]
    fn node_type_matches_payload_variant() {
        for node in sample_nodes() {
            assert_eq!(node.node_type, node.payload.node_type());
        }
    }

    #[test]
    fn roundtrip_preserves_every_variant() {
        for node in sample_nodes() {
            let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&node).expect("serialize");
            let decoded: Node =
                rkyv::from_bytes::<Node, rkyv::rancor::Error>(&bytes).expect("deserialize");
            assert_eq!(node, decoded);
        }
    }
}
