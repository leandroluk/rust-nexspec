//! Typed node entities (REQ-101, REQ-102 in
//! `.specs/features/storage-primitives/spec.md`). `Node::id` is a
//! [`crate::sync::mutation::StableId`] (Blake3 hash) — never a physical/dense
//! index; the dense `u32` index (REQ-103) is assigned only by
//! [`crate::graph::csr::CsrBase`] at build/compaction time.

use crate::sync::mutation::StableId;

/// Deterministic id for a [`NodeType::File`] node, keyed only by its
/// repo-relative path — used by `git::cochange` (REQ-206) and
/// `sync_orchestrator` (REQ-205) so both agree on the same id for the same
/// file without needing a shared lookup table.
pub fn file_node_id(path: &str) -> StableId {
    *blake3::hash(format!("file:{path}").as_bytes()).as_bytes()
}

/// Deterministic id for the `ordinal`-th symbol called `name` in `path`
/// (`.specs/features/dependency-edges/design.md` D1). Unlike the old
/// `name@line` scheme it does not change when lines above the symbol move,
/// and it can be computed for another file without reading that file: the
/// importing side only needs the resolved path and the imported name.
pub fn symbol_node_id(path: &str, name: &str, ordinal: usize) -> StableId {
    *blake3::hash(format!("symbol:{path}:{name}:{ordinal}").as_bytes()).as_bytes()
}

/// Deterministic ids of the domain nodes (Fase 14): the same object gets the same id
/// on every sync, whatever file it was found in.
pub fn table_node_id(schema: &str, name: &str) -> StableId {
    *blake3::hash(format!("table:{schema}.{name}").as_bytes()).as_bytes()
}

pub fn column_node_id(schema: &str, table: &str, name: &str) -> StableId {
    *blake3::hash(format!("column:{schema}.{table}.{name}").as_bytes()).as_bytes()
}

pub fn constraint_node_id(schema: &str, table: &str, name: &str) -> StableId {
    *blake3::hash(format!("constraint:{schema}.{table}.{name}").as_bytes()).as_bytes()
}

pub fn package_node_id(name: &str) -> StableId {
    *blake3::hash(format!("package:{name}").as_bytes()).as_bytes()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum NodeType {
    Requirement,
    Task,
    Adr,
    DocSection,
    Symbol,
    File,
    Table,
    Column,
    Constraint,
    Package,
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
        /// 0-indexed, inclusive line range in the source file (REQ-302 in
        /// `.specs/features/ast-lexical-search/spec.md`).
        line_start: u32,
        line_end: u32,
    },
    File {
        path: String,
        source_hash: [u8; 32],
    },
    /// A database table or view (Fase 14).
    Table {
        schema: String,
        name: String,
        is_view: bool,
    },
    Column {
        /// Name of the table it belongs to.
        table: String,
        name: String,
        sql_type: String,
        nullable: bool,
    },
    /// Primary key, unique, foreign key, check or index, with the name the database gives it.
    Constraint {
        table: String,
        name: String,
        /// `primary_key`, `unique`, `foreign_key`, `check`, `index` or `unique_index`.
        kind: String,
    },
    /// A workspace package (`package.json`, `Cargo.toml`).
    Package {
        name: String,
        version: String,
        /// Repository-relative directory.
        dir: String,
    },
}

impl NodePayload {
    /// `kind` and display name of the domain nodes; `None` for the rest.
    pub fn domain_label(&self) -> Option<(&'static str, String)> {
        match self {
            NodePayload::Table { name, is_view, .. } => Some((if *is_view { "view" } else { "table" }, name.clone())),
            NodePayload::Column { table, name, .. } => Some(("column", format!("{table}.{name}"))),
            NodePayload::Constraint { table, name, .. } => Some(("constraint", format!("{name} ({table})"))),
            NodePayload::Package { name, .. } => Some(("package", name.clone())),
            _ => None,
        }
    }

    /// Whether the node belongs to the domain pass (added and removed by it as a whole).
    pub fn is_domain(&self) -> bool {
        self.domain_label().is_some()
    }

    pub fn node_type(&self) -> NodeType {
        match self {
            NodePayload::Requirement { .. } => NodeType::Requirement,
            NodePayload::Task { .. } => NodeType::Task,
            NodePayload::Adr { .. } => NodeType::Adr,
            NodePayload::DocSection { .. } => NodeType::DocSection,
            NodePayload::Symbol { .. } => NodeType::Symbol,
            NodePayload::File { .. } => NodeType::File,
            NodePayload::Table { .. } => NodeType::Table,
            NodePayload::Column { .. } => NodeType::Column,
            NodePayload::Constraint { .. } => NodeType::Constraint,
            NodePayload::Package { .. } => NodeType::Package,
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
                    line_start: 1,
                    line_end: 3,
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
