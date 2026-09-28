//! CSR (Compressed Sparse Row) topology store — two layers (REQ-105,
//! REQ-106, REQ-107 in `.specs/features/storage-primitives/spec.md`). See
//! `.specs/features/storage-primitives/design.md` for the architecture
//! diagram.

pub mod base;
pub mod delta;
pub mod facade;

pub use base::{CsrBase, CsrError};
pub use delta::CsrDelta;
pub use facade::Csr;
