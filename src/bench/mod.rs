//! Retrieval benchmark (Fase 8, `.specs/features/retrieval-benchmark/`).

pub mod corpus;
pub mod locate;
pub mod metrics;

pub use corpus::{Corpus, CorpusError, Kind, Query};
pub use metrics::{Expectation, Location, Ranked};
