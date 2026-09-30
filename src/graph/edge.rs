//! Typed edge entities (REQ-101, REQ-102 in
//! `.specs/features/storage-primitives/spec.md`).

use crate::sync::mutation::StableId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum EdgeType {
    Satisfies,
    DependsOn,
    Implements,
    DefinedIn,
    /// Files that tend to change together in the same commits (REQ-206 in
    /// `.specs/features/git-integration/spec.md`). Represented as a regular
    /// directed `Edge` — a co-change relationship is recorded as two edges,
    /// `A->B` and `B->A`, rather than introducing an undirected edge concept
    /// into the CSR (see `.specs/features/git-integration/design.md`).
    CoChanges,
    /// File -> file: `import`/`require`/`import()` that resolved to a tracked file.
    Imports,
    /// File -> file: `export ... from` / `export * from`.
    ReExports,
    /// Symbol -> symbol (or file): a call to a declared or imported name.
    Calls,
    /// Symbol -> symbol: `new X()`.
    Instantiates,
    /// Symbol -> symbol: `extends` / `implements`.
    Extends,
    /// Symbol -> symbol: any other use (types, decorators, values).
    References,
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
            EdgeType::CoChanges => 4,
            EdgeType::Imports => 5,
            EdgeType::ReExports => 6,
            EdgeType::Calls => 7,
            EdgeType::Instantiates => 8,
            EdgeType::Extends => 9,
            EdgeType::References => 10,
        }
    }

    /// Edge types meaning "this depends on that" (Fase 7, REQ-709): the
    /// aggregate that `trace`, `search` and `diff --staged` follow. `DependsOn`
    /// itself is kept for indexes and tests that still use the generic type.
    pub fn is_dependency(self) -> bool {
        matches!(
            self,
            EdgeType::DependsOn
                | EdgeType::Imports
                | EdgeType::ReExports
                | EdgeType::Calls
                | EdgeType::Instantiates
                | EdgeType::Extends
                | EdgeType::References
        )
    }

    /// Every dependency-flavoured type, for callers that iterate by type.
    pub const DEPENDENCY_TYPES: [EdgeType; 7] = [
        EdgeType::DependsOn,
        EdgeType::Imports,
        EdgeType::ReExports,
        EdgeType::Calls,
        EdgeType::Instantiates,
        EdgeType::Extends,
        EdgeType::References,
    ];

    pub fn from_code(code: u16) -> Option<Self> {
        match code {
            0 => Some(EdgeType::Satisfies),
            1 => Some(EdgeType::DependsOn),
            2 => Some(EdgeType::Implements),
            3 => Some(EdgeType::DefinedIn),
            4 => Some(EdgeType::CoChanges),
            5 => Some(EdgeType::Imports),
            6 => Some(EdgeType::ReExports),
            7 => Some(EdgeType::Calls),
            8 => Some(EdgeType::Instantiates),
            9 => Some(EdgeType::Extends),
            10 => Some(EdgeType::References),
            _ => None,
        }
    }
}

/// How sure the extractor is about an edge (REQ-710).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    /// Resolved without ambiguity (import target exists and declares the name).
    Extracted,
    /// Reached by name or by fallback (e.g. file-level when the symbol is unknown).
    Inferred,
}

/// Where an edge comes from (REQ-710).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeContext {
    Runtime,
    /// `import type`, or a use only in type position.
    TypeOnly,
    /// Source file is a test.
    Test,
    /// Source file is a spec/e2e file.
    Spec,
}

/// Packs [`Confidence`] and [`EdgeContext`] into the one byte stored in
/// [`Edge::meta`] and sent as `EdgeMutation::payload[0]`: bit 0 = confidence
/// (`1` inferred), bits 1-2 = context.
pub fn encode_meta(confidence: Confidence, context: EdgeContext) -> u8 {
    let c = match confidence {
        Confidence::Extracted => 0,
        Confidence::Inferred => 1,
    };
    let x = match context {
        EdgeContext::Runtime => 0,
        EdgeContext::TypeOnly => 1,
        EdgeContext::Test => 2,
        EdgeContext::Spec => 3,
    };
    c | (x << 1)
}

pub fn decode_meta(meta: u8) -> (Confidence, EdgeContext) {
    let confidence = if meta & 1 == 0 { Confidence::Extracted } else { Confidence::Inferred };
    let context = match (meta >> 1) & 0b11 {
        0 => EdgeContext::Runtime,
        1 => EdgeContext::TypeOnly,
        2 => EdgeContext::Test,
        _ => EdgeContext::Spec,
    };
    (confidence, context)
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Edge {
    pub id: StableId,
    pub from: StableId,
    pub to: StableId,
    pub edge_type: EdgeType,
    /// Packed confidence + context, see [`encode_meta`]. `0` = extracted, runtime.
    pub meta: u8,
}

impl Edge {
    pub fn confidence(&self) -> Confidence {
        decode_meta(self.meta).0
    }

    pub fn context(&self) -> EdgeContext {
        decode_meta(self.meta).1
    }
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
            meta: 0,
        };

        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&edge).expect("serialize");
        let decoded: Edge =
            rkyv::from_bytes::<Edge, rkyv::rancor::Error>(&bytes).expect("deserialize");

        assert_eq!(edge, decoded);
    }

    #[test]
    fn dependency_types_roundtrip_through_their_codes() {
        for ty in EdgeType::DEPENDENCY_TYPES {
            assert_eq!(EdgeType::from_code(ty.to_code()), Some(ty));
            assert!(ty.is_dependency());
        }
        assert_eq!(EdgeType::Imports.to_code(), 5);
        assert_eq!(EdgeType::References.to_code(), 10);
        assert_eq!(EdgeType::from_code(11), None);
        for ty in [EdgeType::Satisfies, EdgeType::DefinedIn, EdgeType::Implements, EdgeType::CoChanges] {
            assert!(!ty.is_dependency(), "{ty:?} is not a dependency");
        }
    }

    #[test]
    fn meta_packs_confidence_and_context_losslessly() {
        for confidence in [Confidence::Extracted, Confidence::Inferred] {
            for context in [EdgeContext::Runtime, EdgeContext::TypeOnly, EdgeContext::Test, EdgeContext::Spec] {
                assert_eq!(decode_meta(encode_meta(confidence, context)), (confidence, context));
            }
        }
        assert_eq!(encode_meta(Confidence::Extracted, EdgeContext::Runtime), 0, "default meta means extracted runtime");
        let edge = Edge {
            id: [1; 32],
            from: [2; 32],
            to: [3; 32],
            edge_type: EdgeType::Imports,
            meta: encode_meta(Confidence::Inferred, EdgeContext::TypeOnly),
        };
        assert_eq!(edge.confidence(), Confidence::Inferred);
        assert_eq!(edge.context(), EdgeContext::TypeOnly);
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&edge).unwrap();
        let back: Edge = rkyv::from_bytes::<Edge, rkyv::rancor::Error>(&bytes).unwrap();
        assert_eq!(back, edge);
    }
}
