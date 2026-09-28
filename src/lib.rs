//! SpecDB — in-process context engine for AI coding agents.
//!
//! See `.specs/project/PROJECT.md` for the product overview and
//! `.specs/project/ROADMAP.md` for the phase-by-phase build plan.

pub mod sync;

pub use sync::{Coordinator, MutationSet, SyncParticipant};
