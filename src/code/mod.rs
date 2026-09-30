//! Multi-language AST parsing (Fase 3). See
//! `.specs/features/ast-lexical-search/design.md`.

pub mod batch;
pub mod deps;
pub mod facts;
pub mod modules;
pub mod parser;
pub mod resolve;

pub use batch::{extract_all, extract_each};
pub use deps::{DependencyBuilder, ExtractedFile, FileSummary};
pub use parser::{CodeError, Language, SymbolSpan, extract, extract_with_facts};
pub use resolve::SpecifierResolver;
