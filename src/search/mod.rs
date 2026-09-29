//! Lexical search (Fase 3). See
//! `.specs/features/ast-lexical-search/design.md`. Sibling to `graph`/
//! `sync`/`git` — an index/storage concern parallel to the CSR, not a
//! graph-topology concept.

pub mod schema;
pub mod tantivy_participant;

pub use schema::TantivySchema;
pub use tantivy_participant::{SearchError, TantivyParticipant};
