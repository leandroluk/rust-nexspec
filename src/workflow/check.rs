//! `nexspec check-update` (REQ-1603 in `.specs/features/workflow-integration/spec.md`):
//! is the index in step with `HEAD` and the working tree? Never writes: it does
//! not create `.specs/.index`, and it opens an existing database only to read
//! two values.

use std::path::Path;

use redb::Database;

use crate::engine::INDEX_FORMAT;
use crate::git::GitSource;
use crate::sync::{SyncLock, VersionPointer};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Freshness {
    UpToDate,
    Stale(String),
    NoIndex,
}

impl Freshness {
    /// First stdout line of `check-update`, meant to be parsed by a skill.
    pub fn line(&self) -> String {
        match self {
            Self::UpToDate => "up-to-date".to_string(),
            Self::Stale(reason) => format!("stale: {reason}"),
            Self::NoIndex => "no-index".to_string(),
        }
    }

    /// Exit code contract: 0 in step, 3 stale, 4 no index.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::UpToDate => 0,
            Self::Stale(_) => 3,
            Self::NoIndex => 4,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    #[error("{0}")]
    Other(String),
}

fn other(e: impl std::fmt::Display) -> CheckError {
    CheckError::Other(e.to_string())
}

fn short(oid: &[u8; 20]) -> String {
    oid.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

pub fn check(repo: &Path, index_dir: &Path) -> Result<Freshness, CheckError> {
    let db_path = index_dir.join("metadata.redb");
    if !db_path.is_file() {
        return Ok(Freshness::NoIndex);
    }
    let (indexed, format) = {
        // The index lock keeps a running sync from holding the file while we read it.
        let _lock = SyncLock::acquire(index_dir, SyncLock::timeout_from_env()).map_err(other)?;
        let db = Database::open(&db_path).map_err(other)?;
        let version = VersionPointer::new(&db);
        (version.last_indexed_commit().map_err(other)?, version.index_format().map_err(other)?)
    };
    if format != Some(INDEX_FORMAT) {
        return Ok(Freshness::Stale(format!(
            "index format {} is not the one this build expects ({INDEX_FORMAT}); run `nexspec sync`",
            format.map_or("unknown".to_string(), |f| f.to_string())
        )));
    }
    let Some(indexed) = indexed else {
        return Ok(Freshness::NoIndex);
    };
    let git = GitSource::open(repo).map_err(other)?;
    let head = git.head_commit_oid().map_err(other)?;
    if head != indexed {
        return Ok(Freshness::Stale(format!("HEAD is {} but the index is at {}", short(&head), short(&indexed))));
    }
    let dirty = git.dirty_paths().map_err(other)?;
    let dirty: Vec<_> = dirty.iter().filter(|p| !p.starts_with(".specs/.index")).collect();
    if !dirty.is_empty() {
        return Ok(Freshness::Stale(format!("{} uncommitted change(s) in the working tree", dirty.len())));
    }
    Ok(Freshness::UpToDate)
}
