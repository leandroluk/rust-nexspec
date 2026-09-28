//! Graph storage & topology (Fase 1). See
//! `.specs/features/storage-primitives/design.md`.

pub mod csr;
pub mod edge;
pub mod markdown;
pub mod node;

pub use csr::{Csr, CsrParticipant};
pub use edge::{Edge, EdgeType};
pub use markdown::extract;
pub use node::{Node, NodePayload, NodeType};
