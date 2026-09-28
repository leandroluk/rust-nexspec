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
