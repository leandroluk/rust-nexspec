//! Local vector engine (Fase 4). See
//! `.specs/features/vector-engine/design.md`. Excluded entirely from
//! `lean` builds (REQ-404) — this whole module requires the `full` feature.

pub mod hnsw;

pub use hnsw::{HnswError, HnswIndex, HnswParticipant, decode_vector, encode_vector};
