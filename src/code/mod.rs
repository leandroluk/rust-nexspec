//! Multi-language AST parsing (Fase 3). See
//! `.specs/features/ast-lexical-search/design.md`.

pub mod parser;

pub use parser::{CodeError, Language, extract};
