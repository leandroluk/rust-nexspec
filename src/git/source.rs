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

use std::path::{Path, PathBuf};

use gix::object::tree::diff::Change;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("failed to open repository: {0}")]
    Open(String),
    #[error("git operation failed: {0}")]
    Op(String),
    #[error("unsupported object hash length {0} (expected 20-byte SHA-1)")]
    UnsupportedHash(usize),
}

pub(crate) fn op_err<E: std::fmt::Display>(e: E) -> GitError {
    GitError::Op(e.to_string())
}

/// Paths changed between two commits (or, when there's no prior index,
/// everything at `HEAD`) — REQ-203. Renames/copies are intentionally not
/// tracked as a distinct category (`track_rewrites(None)`): a rename shows
/// up as a `Deletion` + an `Addition`, which is exactly what
/// `markdown::extract()`'s remove/re-extract model expects (REQ-205).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TreeDiff {
    pub added: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
}

pub struct GitSource {
    pub(crate) repo: gix::Repository,
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
        self.repo.is_dirty().map_err(op_err)
    }

    /// Paths git itself reports as modified or untracked (non-ignored) in the
    /// working tree — honours `.gitignore`, autocrlf and stat caching, so an
    /// unchanged tracked file is never listed. Feed this (not the full
    /// tracked set) to the dirty-content scan.
    pub fn dirty_paths(&self) -> Result<Vec<PathBuf>, GitError> {
        let iter = self
            .repo
            .status(gix::progress::Discard)
            .map_err(op_err)?
            .untracked_files(gix::status::UntrackedFiles::Files)
            .into_index_worktree_iter(Vec::<gix::bstr::BString>::new())
            .map_err(op_err)?;
        let mut paths = Vec::new();
        for item in iter {
            let item = item.map_err(op_err)?;
            paths.push(PathBuf::from(item.rela_path().to_string()));
        }
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    /// The working tree root, if this repo isn't bare.
    pub fn work_dir(&self) -> Option<&Path> {
        self.repo.workdir()
    }

    /// Every blob path reachable from `HEAD` — used both by `diff_since`
    /// (when there's no prior index) and by callers wanting to scan the
    /// working tree for dirty files (REQ-204) against a known tracked set.
    pub fn tracked_paths_at_head(&self) -> Result<Vec<PathBuf>, GitError> {
        let head_commit = self.repo.head_commit().map_err(op_err)?;
        let tree = head_commit.tree().map_err(op_err)?;
        let mut paths = Vec::new();
        for entry in tree.traverse().breadthfirst.files().map_err(op_err)? {
            if entry.mode.is_blob() {
                paths.push(PathBuf::from(entry.filepath.to_string()));
            }
        }
        Ok(paths)
    }

    /// Files changed between `since` (exclusive) and `HEAD` (inclusive).
    /// `since: None` means "never indexed" — every blob reachable from
    /// `HEAD` is reported as `Added`.
    pub fn diff_since(&self, since: Option<[u8; 20]>) -> Result<TreeDiff, GitError> {
        let head_commit = self.repo.head_commit().map_err(op_err)?;
        let head_tree = head_commit.tree().map_err(op_err)?;

        let Some(since) = since else {
            return Ok(TreeDiff {
                added: self.tracked_paths_at_head()?,
                modified: Vec::new(),
                deleted: Vec::new(),
            });
        };

        let since_oid = gix::ObjectId::from_bytes_or_panic(&since);
        let since_commit = self.repo.find_commit(since_oid).map_err(op_err)?;
        let since_tree = since_commit.tree().map_err(op_err)?;

        let mut diff = TreeDiff::default();
        since_tree
            .changes()
            .map_err(op_err)?
            .options(|o| {
                o.track_path();
                o.track_rewrites(None);
            })
            .for_each_to_obtain_tree(&head_tree, |change| {
                match change {
                    Change::Addition {
                        location,
                        entry_mode,
                        ..
                    } if entry_mode.is_blob() => {
                        diff.added.push(PathBuf::from(location.to_string()));
                    }
                    Change::Deletion {
                        location,
                        entry_mode,
                        ..
                    } if entry_mode.is_blob() => {
                        diff.deleted.push(PathBuf::from(location.to_string()));
                    }
                    Change::Modification {
                        location,
                        entry_mode,
                        ..
                    } if entry_mode.is_blob() => {
                        diff.modified.push(PathBuf::from(location.to_string()));
                    }
                    _ => {} // directory entries and rewrites (disabled above) — nothing to record
                }
                Ok(std::ops::ControlFlow::Continue(()))
            })
            .map_err(op_err)?;

        Ok(diff)
    }

    /// Read a file's content straight from the `HEAD` tree (not the working
    /// tree filesystem) — works identically whether `HEAD` is on a branch or
    /// detached (REQ-208). `None` if the path doesn't exist at `HEAD`.
    pub fn read_blob_at_head(&self, path: &std::path::Path) -> Result<Option<Vec<u8>>, GitError> {
        let head_commit = self.repo.head_commit().map_err(op_err)?;
        let tree = head_commit.tree().map_err(op_err)?;
        let Some(entry) = tree.lookup_entry_by_path(path).map_err(op_err)? else {
            return Ok(None);
        };
        let object = entry.object().map_err(op_err)?;
        Ok(Some(object.detach().data))
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
