//! Git integration (Fase 2). See `.specs/features/git-integration/design.md`.

pub mod cochange;
pub mod dirty_cache;
pub mod source;
pub mod spec_link;

pub use cochange::CoChangeWindow;
pub use dirty_cache::DirtyCache;
pub use source::{GitError, GitSource};
pub use spec_link::{CommitInfo, extract_commit_links};
