//! Lexical search (Fase 3). See
//! `.specs/features/ast-lexical-search/design.md`. Sibling to `graph`/
//! `sync`/`git` — an index/storage concern parallel to the CSR, not a
//! graph-topology concept.

pub mod ident;
pub mod query;
pub mod schema;
pub mod tantivy_participant;

pub use query::{find_by_id, search_text};
pub use schema::{TantivySchema, hex, unhex};
pub use tantivy_participant::{SearchError, TantivyHandle, TantivyParticipant, TantivyQueryable};
