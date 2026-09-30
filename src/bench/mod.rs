//! Retrieval benchmark (Fase 8, `.specs/features/retrieval-benchmark/`).

pub mod baselines;
pub mod corpus;
pub mod locate;
pub mod metrics;
pub mod report;
pub mod runner;

pub use corpus::{Corpus, CorpusError, Kind, Query};
pub use metrics::{Expectation, Location, Ranked};
