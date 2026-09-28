//! [`GitSource`] — the narrow boundary around `gix` (REQ-201 in
//! `.specs/features/git-integration/spec.md`). Every `gix` call in this
//! crate goes through this module, so an API break in Gitoxide (which moves
//! faster than `redb`/`rkyv`) is a one-file fix — see
//! `.specs/features/git-integration/design.md` → Risks.
//!
//! Object ids are represented as `[u8; 20]` (SHA-1) — the default and, as of
//! this writing, only widely-used Git hash algorithm. A SHA-256 repository
//! would fail to open cleanly here; not handled (documented limitation, not
//! a silent truncation).

use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("failed to open repository: {0}")]
    Open(String),
    #[error("git operation failed: {0}")]
    Op(String),
    #[error("unsupported object hash length {0} (expected 20-byte SHA-1)")]
    UnsupportedHash(usize),
}

pub struct GitSource {
    repo: gix::Repository,
}

impl GitSource {
    pub fn open(path: &Path) -> Result<Self, GitError> {
        let repo = gix::open(path).map_err(|e| GitError::Open(e.to_string()))?;
        Ok(Self { repo })
    }

    /// The OID `HEAD` currently resolves to — works the same whether `HEAD`
    /// is on a branch or detached (REQ-208).
    pub fn head_commit_oid(&self) -> Result<[u8; 20], GitError> {
        let id = self.repo.head_id().map_err(|e| GitError::Op(e.to_string()))?;
        oid_from_bytes(id.as_bytes())
    }

    /// Whether the working tree has uncommitted changes relative to `HEAD`.
    /// Untracked files do not count (matches `gix`'s own definition).
    pub fn is_dirty(&self) -> Result<bool, GitError> {
        self.repo.is_dirty().map_err(|e| GitError::Op(e.to_string()))
    }
}

fn oid_from_bytes(bytes: &[u8]) -> Result<[u8; 20], GitError> {
    if bytes.len() != 20 {
        return Err(GitError::UnsupportedHash(bytes.len()));
    }
    let mut oid = [0u8; 20];
    oid.copy_from_slice(bytes);
    Ok(oid)
}
