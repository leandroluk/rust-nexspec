//! Sync Coordinator & Transactional Integrity.
//!
//! See `.specs/features/sync-coordinator/design.md`.

pub mod coordinator;
pub mod mutation;
pub mod participant;
pub mod redb_participant;
pub mod version;
pub mod wal;

pub use coordinator::Coordinator;
pub use mutation::{DocMutation, EdgeMutation, MutationSet, NodeMutation, StableId};
pub use participant::{SyncError, SyncParticipant};
pub use redb_participant::RedbParticipant;
pub use version::{VersionError, VersionPointer};
pub use wal::{Wal, WalError};
