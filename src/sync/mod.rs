//! Sync Coordinator & Transactional Integrity.
//!
//! See `.specs/features/sync-coordinator/design.md`.

pub mod mutation;
pub mod participant;
pub mod version;

pub use mutation::{DocMutation, EdgeMutation, MutationSet, NodeMutation, StableId};
pub use participant::{SyncError, SyncParticipant};
pub use version::{VersionError, VersionPointer};
