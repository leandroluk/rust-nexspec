//! NexSpec — in-process context engine for AI coding agents.
//!
//! See `.specs/project/PROJECT.md` for the product overview and
//! `.specs/project/ROADMAP.md` for the phase-by-phase build plan.

pub mod code;
pub mod git;
pub mod graph;
pub mod hybrid;
pub mod search;
pub mod sync;
pub mod sync_orchestrator;
#[cfg(feature = "full")]
pub mod vector;

pub use code::{Language, extract as extract_code};
pub use git::GitSource;
pub use graph::{Csr, CsrParticipant, Edge, EdgeType, Node, NodeType};
pub use hybrid::{expand, seed_discovery};
pub use search::{TantivyParticipant, find_by_id, search_text};
pub use sync::{Coordinator, MutationSet, SyncParticipant};
pub use sync_orchestrator::SyncOrchestrator;
#[cfg(feature = "full")]
pub use vector::{Embedder, HnswParticipant, VectorError};
