//! Retrieval enrichment (Fase 19): LLM file summaries that make prose questions findable.
//! See `.specs/features/retrieval-enrichment/`.

pub mod cache;
pub mod select;
pub mod provider;
pub mod cost;
pub mod run;
