//! Temporal spec linking (REQ-207 in `.specs/features/git-integration/spec.md`):
//! connect a commit to the requirements/tasks/ADRs its message mentions
//! (`feat(auth): satisfy REQ-001`).

use crate::git::source::{GitError, GitSource, op_err};
use crate::graph::markdown::find_markers;

/// A commit relevant to spec linking — OID, message, author and timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub oid: [u8; 20],
    pub message: String,
    pub author: String,
    /// Seconds since Unix epoch.
    pub timestamp: i64,
}

/// Every `REQ-\d+`/`TASK-\d+`/`ADR-\d+` marker mentioned anywhere in a
/// commit message, e.g. `"feat(auth): satisfy REQ-001"` -> `["REQ-001"]`.
/// Reuses the exact same marker syntax `markdown::extract()` recognizes in
/// specs, so a mention means the same thing in both places.
pub fn extract_commit_links(message: &str) -> Vec<String> {
    let mut found = Vec::new();
    for marker in ["REQ-", "TASK-", "ADR-"] {
        found.extend(find_markers(message, marker));
    }
    found
}

impl GitSource {
    /// Commits reachable from `HEAD`, newest first, stopping *before*
    /// `since` (exclusive) if given — `None` walks the full history.
    pub fn commits_since(&self, since: Option<[u8; 20]>) -> Result<Vec<CommitInfo>, GitError> {
        let head_id = self.head_id_attached()?;
        let mut commits = Vec::new();

        let walk = head_id.ancestors().all().map_err(op_err)?;
        for info in walk {
            let info = info.map_err(op_err)?;
            let mut oid = [0u8; 20];
            oid.copy_from_slice(info.id.as_bytes());
            if since == Some(oid) {
                break;
            }

            let commit = self.repo.find_commit(info.id).map_err(op_err)?;
            let message = commit.message_raw_sloppy().to_string();
            let author = commit
                .author()
                .map_err(op_err)?
                .name
                .to_string();
            let timestamp = commit.time().map_err(op_err)?.seconds;

            commits.push(CommitInfo {
                oid,
                message,
                author,
                timestamp,
            });
        }

        Ok(commits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_commit_links_finds_all_marker_kinds() {
        assert_eq!(
            extract_commit_links("feat(auth): satisfy REQ-001"),
            vec!["REQ-001"]
        );
        // Grouped by marker kind (REQ, then TASK, then ADR), not by text
        // position -- extract_commit_links scans one marker kind at a time.
        assert_eq!(
            extract_commit_links("refs TASK-042 and ADR-007, plus REQ-001"),
            vec!["REQ-001", "TASK-042", "ADR-007"]
        );
    }

    #[test]
    fn extract_commit_links_returns_empty_for_plain_message() {
        assert!(extract_commit_links("chore: bump deps").is_empty());
    }
}
