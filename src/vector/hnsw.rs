//! [`HnswIndex`] + [`HnswParticipant`] — cosine-similarity nearest-neighbor
//! search (REQ-403 in `.specs/features/vector-engine/spec.md`). The fourth
//! real `SyncParticipant`, after `RedbParticipant`, `CsrParticipant`, and
//! `TantivyParticipant`.
//!
//! **Vector convention (Fase 4-specific, not a `NodePayload` variant):**
//! `instant-distance` builds its index once from a full point set rather
//! than supporting incremental insertion, so — unlike the CSR's real
//! base+delta split — this participant simply rebuilds the whole index on
//! every `commit()` from its complete committed point set. That's cheap at
//! the vector counts this crate deals with; revisit if profiling ever says
//! otherwise. Each staged [`crate::sync::mutation::NodeMutation::Upsert`]'s
//! `payload` is interpreted as a raw little-endian `f32` vector (via
//! [`encode_vector`]/[`decode_vector`]) — `NodePayload` has no embedding
//! variant yet, and nothing else gives universal meaning to node-payload
//! bytes at the coordinator level (`RedbParticipant` stores them opaquely;
//! `CsrParticipant` never looks at node payloads at all). Revisit once
//! real embedding generation (T-406) is wired into `SyncOrchestrator`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};

use instant_distance::{Builder, HnswMap, Search};

use crate::sync::mutation::{MutationSet, NodeMutation, StableId};
use crate::sync::participant::{SyncError, SyncParticipant};

#[derive(Debug, thiserror::Error)]
pub enum HnswError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("codec error: {0}")]
    Codec(String),
}

fn storage_err<E: std::fmt::Display>(e: E) -> SyncError {
    SyncError::Storage(e.to_string())
}
impl From<HnswError> for SyncError {
    fn from(e: HnswError) -> Self {
        storage_err(e)
    }
}

pub fn encode_vector(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Decode raw little-endian `f32` bytes into a vector. Any chunk that
/// doesn't decode to a finite value (arbitrary byte sequences — e.g. a
/// `rkyv`-serialized struct's bytes, in tests that don't have a real
/// embedding handy — routinely produce NaN/Infinity bit patterns) is
/// sanitized to `0.0`, since a single non-finite component would otherwise
/// poison every downstream cosine-distance comparison this crate makes.
pub fn decode_vector(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| {
            let f = f32::from_le_bytes(c.try_into().unwrap());
            if f.is_finite() { f } else { 0.0 }
        })
        .collect()
}

#[derive(Clone)]
struct EmbeddingPoint(Vec<f32>);

impl instant_distance::Point for EmbeddingPoint {
    fn distance(&self, other: &Self) -> f32 {
        // 1 - cosine_similarity: 0 for identical direction, up to 2 for
        // opposite. instant-distance treats a smaller value as "closer".
        //
        // Accumulates in f64: a real embedding model's output is small and
        // well-scaled, but this participant's "read arbitrary bytes as a
        // vector" convention (see module docs) can produce individual f32
        // components near f32::MAX from unrelated byte patterns — squaring
        // those in f32 overflows to infinity, and infinity/infinity is NaN,
        // which then poisons every comparison HNSW makes. f64 pushes that
        // threshold far out of reach for any value an f32 can represent.
        let dot: f64 = self.0.iter().zip(&other.0).map(|(a, b)| f64::from(*a) * f64::from(*b)).sum();
        let norm_a: f64 = self.0.iter().map(|a| f64::from(*a) * f64::from(*a)).sum::<f64>().sqrt();
        let norm_b: f64 = other.0.iter().map(|b| f64::from(*b) * f64::from(*b)).sum::<f64>().sqrt();
        if norm_a == 0.0 || norm_b == 0.0 {
            return 1.0;
        }
        let cosine = (dot / (norm_a * norm_b)).clamp(-1.0, 1.0);
        (1.0 - cosine) as f32
    }
}

/// The search structure — rebuilt wholesale on every commit from the
/// complete committed point set (see module docs).
pub struct HnswIndex {
    map: RwLock<Option<HnswMap<EmbeddingPoint, StableId>>>,
}

impl HnswIndex {
    pub fn new() -> Self {
        Self {
            map: RwLock::new(None),
        }
    }

    pub fn rebuild(&self, points: &HashMap<StableId, Vec<f32>>) {
        let new_map = if points.is_empty() {
            None
        } else {
            let (ids, vectors): (Vec<StableId>, Vec<EmbeddingPoint>) = points
                .iter()
                .map(|(id, v)| (*id, EmbeddingPoint(v.clone())))
                .unzip();
            Some(Builder::default().build(vectors, ids))
        };
        *self.map.write().unwrap() = new_map;
    }

    /// Nearest `k` neighbors to `query`, nearest first, as `(id, distance)`.
    /// Empty if the index has never had any points committed.
    pub fn search(&self, query: &[f32], k: usize) -> Vec<(StableId, f32)> {
        let guard = self.map.read().unwrap();
        let Some(map) = guard.as_ref() else {
            return Vec::new();
        };
        let query_point = EmbeddingPoint(query.to_vec());
        let mut search = Search::default();
        map.search(&query_point, &mut search)
            .take(k)
            .map(|item| (*item.value, item.distance))
            .collect()
    }
}

impl Default for HnswIndex {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
struct PersistedPoint {
    id: StableId,
    vector: Vec<f32>,
}

pub struct HnswParticipant {
    index: HnswIndex,
    path: PathBuf,
    committed_points: Mutex<HashMap<StableId, Vec<f32>>>,
    staged: Mutex<Option<(u64, Vec<(StableId, Vec<f32>)>)>>,
    committed_version: AtomicU64,
}

impl HnswParticipant {
    /// Loads any previously persisted points from `path` (if it exists) and
    /// rebuilds the search index from them.
    pub fn new(path: &Path) -> Result<Self, HnswError> {
        let committed_points = if path.exists() && std::fs::metadata(path)?.len() > 0 {
            let bytes = std::fs::read(path)?;
            let mut aligned = rkyv::util::AlignedVec::<16>::new();
            aligned.extend_from_slice(&bytes);
            let points = rkyv::from_bytes::<Vec<PersistedPoint>, rkyv::rancor::Error>(&aligned)
                .map_err(|e| HnswError::Codec(e.to_string()))?;
            points.into_iter().map(|p| (p.id, p.vector)).collect()
        } else {
            HashMap::new()
        };

        let index = HnswIndex::new();
        index.rebuild(&committed_points);

        Ok(Self {
            index,
            path: path.to_path_buf(),
            committed_points: Mutex::new(committed_points),
            staged: Mutex::new(None),
            committed_version: AtomicU64::new(0),
        })
    }

    pub fn index(&self) -> &HnswIndex {
        &self.index
    }

    fn persist(&self, points: &HashMap<StableId, Vec<f32>>) -> Result<(), HnswError> {
        let persisted: Vec<PersistedPoint> = points
            .iter()
            .map(|(id, vector)| PersistedPoint {
                id: *id,
                vector: vector.clone(),
            })
            .collect();
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&persisted)
            .map_err(|e| HnswError::Codec(e.to_string()))?;
        std::fs::write(&self.path, &bytes)?;
        Ok(())
    }
}

impl SyncParticipant for HnswParticipant {
    fn stage(&self, target_version: u64, mutations: &MutationSet) -> Result<(), SyncError> {
        if self.committed_version()? >= target_version {
            return Ok(());
        }
        let points: Vec<(StableId, Vec<f32>)> = mutations
            .nodes
            .iter()
            .filter_map(|m| match m {
                NodeMutation::Upsert { id, payload } => Some((*id, decode_vector(payload))),
                NodeMutation::Remove { .. } => None,
            })
            .collect();
        *self.staged.lock().unwrap() = Some((target_version, points));
        Ok(())
    }

    fn committed_version(&self) -> Result<u64, SyncError> {
        Ok(self.committed_version.load(Ordering::SeqCst))
    }

    fn commit(&self, target_version: u64) -> Result<(), SyncError> {
        if self.committed_version()? >= target_version {
            return Ok(());
        }
        let staged = self.staged.lock().unwrap().take();
        let Some((staged_version, points)) = staged else {
            return Err(SyncError::Storage(format!(
                "commit({target_version}) called with nothing staged"
            )));
        };
        if staged_version != target_version {
            return Err(SyncError::Storage(format!(
                "staged version {staged_version} does not match commit target {target_version}"
            )));
        }

        let mut committed = self.committed_points.lock().unwrap();
        for (id, vector) in points {
            committed.insert(id, vector);
        }
        self.persist(&committed)?;
        self.index.rebuild(&committed);
        self.committed_version.store(target_version, Ordering::SeqCst);
        Ok(())
    }

    fn abort(&self, target_version: u64) -> Result<(), SyncError> {
        let mut staged = self.staged.lock().unwrap();
        if matches!(&*staged, Some((v, _)) if *v == target_version) {
            *staged = None;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn upsert(id: StableId, vector: Vec<f32>) -> MutationSet {
        MutationSet {
            nodes: vec![NodeMutation::Upsert {
                id,
                payload: encode_vector(&vector),
            }],
            edges: vec![],
            docs: vec![],
        }
    }

    #[test]
    fn stage_then_commit_makes_vector_searchable() {
        let file = NamedTempFile::new().unwrap();
        let p = HnswParticipant::new(file.path()).unwrap();
        let set = upsert([1u8; 32], vec![1.0, 0.0, 0.0]);

        p.stage(1, &set).unwrap();
        assert!(p.index().search(&[1.0, 0.0, 0.0], 5).is_empty(), "not visible before commit");

        p.commit(1).unwrap();
        let hits = p.index().search(&[1.0, 0.0, 0.0], 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, [1u8; 32]);
        assert_eq!(p.committed_version().unwrap(), 1);
    }

    #[test]
    fn stage_then_abort_discards_vector() {
        let file = NamedTempFile::new().unwrap();
        let p = HnswParticipant::new(file.path()).unwrap();
        let set = upsert([2u8; 32], vec![0.0, 1.0, 0.0]);

        p.stage(5, &set).unwrap();
        p.abort(5).unwrap();

        assert!(p.index().search(&[0.0, 1.0, 0.0], 5).is_empty());
        assert_eq!(p.committed_version().unwrap(), 0);
    }

    #[test]
    fn stage_is_idempotent_and_commit_does_not_duplicate() {
        let file = NamedTempFile::new().unwrap();
        let p = HnswParticipant::new(file.path()).unwrap();
        let set = upsert([3u8; 32], vec![0.0, 0.0, 1.0]);

        p.stage(1, &set).unwrap();
        p.stage(1, &set).unwrap();
        p.commit(1).unwrap();
        p.commit(1).unwrap();

        assert_eq!(p.index().search(&[0.0, 0.0, 1.0], 5).len(), 1);
        assert_eq!(p.committed_version().unwrap(), 1);
    }

    /// Regression test for the overflow bug found while writing T-408's
    /// integration test: components decoded from arbitrary bytes can be
    /// near `f32::MAX`; squaring them in `f32` during distance computation
    /// overflowed to infinity, and infinity/infinity produced NaN, which
    /// made search return the wrong nearest neighbor entirely.
    #[test]
    fn distance_stays_finite_for_extreme_magnitude_vectors() {
        let file = NamedTempFile::new().unwrap();
        let p = HnswParticipant::new(file.path()).unwrap();
        let a_bytes: Vec<u8> = (0u8..80).collect();
        let b_bytes: Vec<u8> = (80u8..160).collect();
        let a_vec = decode_vector(&a_bytes);
        let b_vec = decode_vector(&b_bytes);

        let mut set = MutationSet::default();
        set.nodes.push(NodeMutation::Upsert { id: [1u8; 32], payload: a_bytes });
        set.nodes.push(NodeMutation::Upsert { id: [2u8; 32], payload: b_bytes });
        p.stage(1, &set).unwrap();
        p.commit(1).unwrap();

        let a_hits = p.index().search(&a_vec, 2);
        let b_hits = p.index().search(&b_vec, 2);
        assert_eq!(a_hits[0].0, [1u8; 32], "exact self-match must win despite extreme magnitude");
        assert_eq!(b_hits[0].0, [2u8; 32], "exact self-match must win despite extreme magnitude");
        assert!(a_hits.iter().all(|(_, d)| d.is_finite()));
        assert!(b_hits.iter().all(|(_, d)| d.is_finite()));
    }

    #[test]
    fn search_finds_the_correct_nearest_neighbor_among_synthetic_vectors() {
        let file = NamedTempFile::new().unwrap();
        let p = HnswParticipant::new(file.path()).unwrap();
        let mut set = MutationSet::default();
        set.nodes.push(NodeMutation::Upsert {
            id: [10u8; 32],
            payload: encode_vector(&[1.0, 0.0]),
        });
        set.nodes.push(NodeMutation::Upsert {
            id: [20u8; 32],
            payload: encode_vector(&[0.0, 1.0]),
        });
        set.nodes.push(NodeMutation::Upsert {
            id: [30u8; 32],
            payload: encode_vector(&[-1.0, 0.0]),
        });
        p.stage(1, &set).unwrap();
        p.commit(1).unwrap();

        let hits = p.index().search(&[0.9, 0.1], 1);
        assert_eq!(hits[0].0, [10u8; 32], "closest to [1,0] direction");
    }
}
