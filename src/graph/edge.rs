//! Typed edge entities (REQ-101, REQ-102 in
//! `.specs/features/storage-primitives/spec.md`).

use crate::sync::mutation::StableId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum EdgeType {
    Satisfies,
    DependsOn,
    Implements,
    DefinedIn,
}

impl EdgeType {
    /// The `u16` code used by `sync::mutation::EdgeMutation`, which stays
    /// generic/opaque at the coordinator level (REQ-109's boundary: the
    /// coordinator never interprets domain edge types).
    pub fn to_code(self) -> u16 {
        match self {
            EdgeType::Satisfies => 0,
            EdgeType::DependsOn => 1,
            EdgeType::Implements => 2,
            EdgeType::DefinedIn => 3,
        }
    }

    pub fn from_code(code: u16) -> Option<Self> {
        match code {
            0 => Some(EdgeType::Satisfies),
            1 => Some(EdgeType::DependsOn),
            2 => Some(EdgeType::Implements),
            3 => Some(EdgeType::DefinedIn),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Edge {
    pub id: StableId,
    pub from: StableId,
    pub to: StableId,
    pub edge_type: EdgeType,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_preserves_data() {
        let edge = Edge {
            id: [1u8; 32],
            from: [2u8; 32],
            to: [3u8; 32],
            edge_type: EdgeType::DependsOn,
        };

        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&edge).expect("serialize");
        let decoded: Edge =
            rkyv::from_bytes::<Edge, rkyv::rancor::Error>(&bytes).expect("deserialize");

        assert_eq!(edge, decoded);
    }
}
