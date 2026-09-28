//! NexSpec — in-process context engine for AI coding agents.
//!
//! See `.specs/project/PROJECT.md` for the product overview and
//! `.specs/project/ROADMAP.md` for the phase-by-phase build plan.

pub mod graph;
pub mod sync;

pub use graph::{Csr, CsrParticipant, Edge, EdgeType, Node, NodeType};
pub use sync::{Coordinator, MutationSet, SyncParticipant};
