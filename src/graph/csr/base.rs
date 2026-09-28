//! [`CsrBase`] — the immutable, memory-mapped base layer of the CSR topology
//! store (REQ-105 in `.specs/features/storage-primitives/spec.md`). Rebuilt
//! wholesale only on compaction (see `CsrParticipant`, T-107); never mutated
//! in place between compactions.
//!
//! On-disk layout: an `rkyv`-archived [`CsrBaseData`] — a flat `edges` array
//! plus a small `index` of `(from, edge_type) -> (start, len)` ranges into it,
//! sorted for grouping. `open()` eagerly hashes the (small) index into a
//! `HashMap` for O(1) average-case lookup, while `edges` stays mmap-backed and
//! is only deserialized for the slice a query actually touches — the bulk of
//! the file never needs a full read+parse pass.

use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;

use memmap2::Mmap;
use rkyv::rancor::Error as RkyvError;

use crate::graph::edge::{Edge, EdgeType};
use crate::sync::mutation::StableId;

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
struct IndexEntry {
    from: StableId,
    edge_type: EdgeType,
    start: u32,
    len: u32,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
struct CsrBaseData {
    /// Grouped by `(from, edge_type)`; each group's edges are contiguous in
    /// `edges` at `[start, start + len)`.
    index: Vec<IndexEntry>,
    edges: Vec<Edge>,
}

#[derive(Debug, thiserror::Error)]
pub enum CsrError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("codec error: {0}")]
    Codec(String),
}

pub struct CsrBase {
    // Kept alive for the lifetime of `archived`'s borrow; never read directly.
    _mmap: Mmap,
    archived: &'static ArchivedCsrBaseData,
    index: HashMap<(StableId, EdgeType), (u32, u32)>,
}

impl CsrBase {
    /// Write a fresh base file from `edges`. Overwrites `path` if it exists —
    /// callers wanting crash-safe compaction write to a `.staging` path and
    /// rename atomically themselves (that's the coordinator's job, not this
    /// type's).
    pub fn build(edges: &[Edge], path: &Path) -> Result<(), CsrError> {
        let mut sorted: Vec<Edge> = edges.to_vec();
        sorted.sort_by(|a, b| (a.from, a.edge_type as u8).cmp(&(b.from, b.edge_type as u8)));

        let mut index = Vec::new();
        let mut i = 0usize;
        while i < sorted.len() {
            let from = sorted[i].from;
            let edge_type = sorted[i].edge_type;
            let start = i as u32;
            let mut j = i;
            while j < sorted.len() && sorted[j].from == from && sorted[j].edge_type == edge_type {
                j += 1;
            }
            index.push(IndexEntry {
                from,
                edge_type,
                start,
                len: (j - i) as u32,
            });
            i = j;
        }

        let data = CsrBaseData {
            index,
            edges: sorted,
        };
        let bytes =
            rkyv::to_bytes::<RkyvError>(&data).map_err(|e| CsrError::Codec(e.to_string()))?;
        let mut file = File::create(path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        Ok(())
    }

    /// Memory-map `path` and eagerly hash its (small) index for O(1) lookup.
    pub fn open(path: &Path) -> Result<Self, CsrError> {
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };

        // SAFETY: `bytes` borrows from `mmap`, which we keep alive as long as
        // `Self` exists; we never expose `archived` past `Self`'s lifetime.
        let bytes: &'static [u8] =
            unsafe { std::slice::from_raw_parts(mmap.as_ptr(), mmap.len()) };
        let archived = rkyv::access::<ArchivedCsrBaseData, RkyvError>(bytes)
            .map_err(|e| CsrError::Codec(e.to_string()))?;
        let archived: &'static ArchivedCsrBaseData = unsafe { std::mem::transmute(archived) };

        let mut index = HashMap::with_capacity(archived.index.len());
        for entry in archived.index.iter() {
            let edge_type = rkyv::deserialize::<EdgeType, RkyvError>(&entry.edge_type)
                .map_err(|e| CsrError::Codec(e.to_string()))?;
            index.insert(
                (entry.from, edge_type),
                (entry.start.to_native(), entry.len.to_native()),
            );
        }

        Ok(Self {
            _mmap: mmap,
            archived,
            index,
        })
    }

    /// Total number of edges in this base file — used by `CsrParticipant` to
    /// evaluate the compaction threshold (REQ-107) without deserializing
    /// anything.
    pub fn edge_count(&self) -> usize {
        self.archived.edges.len()
    }

    /// Every edge in this base file, deserialized. Used only by
    /// `CsrParticipant::compact()` to fold this base with the current delta
    /// into a fresh one — never on a hot read path.
    pub fn all_edges(&self) -> Vec<Edge> {
        self.archived
            .edges
            .iter()
            .map(|e| {
                rkyv::deserialize::<Edge, RkyvError>(e).expect("archived edge failed to validate")
            })
            .collect()
    }

    /// O(1) average-case: hashmap lookup for the range, then deserialize only
    /// the matching slice of `edges` (never the whole file).
    pub fn edges_from(&self, from: &StableId, edge_type: EdgeType) -> Vec<Edge> {
        let Some(&(start, len)) = self.index.get(&(*from, edge_type)) else {
            return Vec::new();
        };
        self.archived.edges[start as usize..(start + len) as usize]
            .iter()
            .map(|e| {
                rkyv::deserialize::<Edge, RkyvError>(e).expect("archived edge failed to validate")
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn edge(from: StableId, to: StableId, edge_type: EdgeType) -> Edge {
        Edge {
            id: [0u8; 32],
            from,
            to,
            edge_type,
        }
    }

    #[test]
    fn build_then_open_returns_edges_grouped_by_from_and_type() {
        let file = NamedTempFile::new().unwrap();
        let edges = vec![
            edge([1u8; 32], [2u8; 32], EdgeType::DependsOn),
            edge([1u8; 32], [3u8; 32], EdgeType::DependsOn),
            edge([1u8; 32], [4u8; 32], EdgeType::Satisfies),
            edge([5u8; 32], [6u8; 32], EdgeType::Implements),
        ];
        CsrBase::build(&edges, file.path()).unwrap();

        let base = CsrBase::open(file.path()).unwrap();

        let mut depends = base.edges_from(&[1u8; 32], EdgeType::DependsOn);
        depends.sort_by_key(|e| e.to);
        assert_eq!(depends.len(), 2);
        assert_eq!(depends[0].to, [2u8; 32]);
        assert_eq!(depends[1].to, [3u8; 32]);

        let satisfies = base.edges_from(&[1u8; 32], EdgeType::Satisfies);
        assert_eq!(satisfies.len(), 1);
        assert_eq!(satisfies[0].to, [4u8; 32]);

        let implements = base.edges_from(&[5u8; 32], EdgeType::Implements);
        assert_eq!(implements.len(), 1);
    }

    #[test]
    fn unknown_node_returns_empty_slice_not_error() {
        let file = NamedTempFile::new().unwrap();
        CsrBase::build(&[], file.path()).unwrap();

        let base = CsrBase::open(file.path()).unwrap();
        assert!(base.edges_from(&[99u8; 32], EdgeType::DependsOn).is_empty());
    }
}
