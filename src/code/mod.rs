//! Multi-language AST parsing (Fase 3). See
//! `.specs/features/ast-lexical-search/design.md`.

pub mod batch;
pub mod facts;
pub mod parser;
pub mod resolve;

pub use batch::{extract_all, extract_each};
pub use parser::{CodeError, Language, extract};
