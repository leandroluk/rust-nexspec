//! Multi-language AST parsing (Fase 3). See
//! `.specs/features/ast-lexical-search/design.md`.

pub mod batch;
pub mod parser;

pub use batch::extract_all;
pub use parser::{CodeError, Language, extract};
