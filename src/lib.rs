//! NexSpec — in-process context engine for AI coding agents.
//!
//! See `.specs/project/PROJECT.md` for the product overview and
//! `.specs/project/ROADMAP.md` for the phase-by-phase build plan.

pub mod bench;
pub mod code;
pub mod engine;
pub mod git;
pub mod graph;
pub mod hybrid;
pub mod mcp;
pub mod query;
pub mod report;
pub mod search;
pub mod sync;
pub mod sync_orchestrator;
pub mod token;
pub mod workflow;
#[cfg(feature = "full")]
pub mod vector;

pub use code::{Language, extract as extract_code};
pub use engine::{BlameResult, DiffResult, Engine, EngineError, ImpactedSymbol, SearchHit, SearchResult, TraceHop, TraceOptions, TraceResult};
pub use git::GitSource;
pub use graph::{Csr, CsrParticipant, Edge, EdgeType, Node, NodePayload, NodeType};
pub use hybrid::{expand, seed_discovery};
pub use mcp::NexSpecMcp;
pub use search::{TantivyParticipant, find_by_id, search_text};
pub use sync::{Coordinator, MutationSet, SyncParticipant};
pub use sync_orchestrator::SyncOrchestrator;
pub use token::budget::{Budget, CharHeuristicTokenizer, TieredItem, Tier, TiktokenTokenizer, TokenError, Tokenizer};
pub use token::pruner::prune_symbol;
pub use token::serializer::serialize;
#[cfg(feature = "full")]
pub use vector::{Embedder, HnswParticipant, VectorError};
