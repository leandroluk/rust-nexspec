//! AST-aware blame (REQ-607 in `.specs/features/cli-mcp-server/spec.md`) —
//! git blame scoped to a symbol's line range, closing the gap Fase 2
//! deliberately left open (see `.specs/features/git-integration/spec.md`
//! → Out of Scope: it needed `Symbol.line_start`/`line_end`, which didn't
//! exist until Fase 3).

use std::ops::Range;
use std::path::Path;

use gix::blame::BlameRanges;
use gix::bstr::ByteSlice;

use crate::git::source::{GitError, GitSource, op_err};

#[derive(Debug, Clone)]
pub struct BlameHunk {
    pub commit_oid: [u8; 20],
    pub author_name: String,
    pub author_email: String,
    pub author_unix_seconds: i64,
    /// 0-indexed, exclusive-end range of lines in the blamed file this hunk
    /// covers.
    pub lines: Range<u32>,
}

/// Blames `path`'s `line_start..=line_end` (0-indexed, inclusive — matches
/// [`crate::graph::node::NodePayload::Symbol`]'s convention) at `HEAD`,
/// returning one [`BlameHunk`] per consecutive run of lines introduced by
/// the same commit. Always walks the file's full history (no `since`
/// bound) — a real blame algorithm, not a heuristic, per REQ-607.
pub fn blame_symbol(
    git: &GitSource,
    path: &Path,
    line_start: u32,
    line_end: u32,
) -> Result<Vec<BlameHunk>, GitError> {
    let head_oid = git.head_commit_oid()?;
    let suspect = gix::ObjectId::from_bytes_or_panic(&head_oid);

    let ranges = BlameRanges::from_one_based_inclusive_range((line_start + 1)..=(line_end + 1)).map_err(op_err)?;

    let path_str = path.to_string_lossy().replace('\\', "/");
    let file_path = path_str.as_bytes().as_bstr();

    let outcome = git
        .repo
        .blame_file(
            file_path,
            suspect,
            gix::repository::blame_file::Options {
                ranges,
                ..Default::default()
            },
        )
        .map_err(op_err)?;

    let mut hunks = Vec::with_capacity(outcome.entries.len());
    for entry in outcome.entries {
        let commit = git.repo.find_commit(entry.commit_id).map_err(op_err)?;
        let author = commit.author().map_err(op_err)?;
        let time = author.time().map_err(op_err)?;
        let mut commit_oid = [0u8; 20];
        commit_oid.copy_from_slice(entry.commit_id.as_bytes());
        hunks.push(BlameHunk {
            commit_oid,
            author_name: author.name.to_string(),
            author_email: author.email.to_string(),
            author_unix_seconds: time.seconds,
            lines: entry.start_in_blamed_file..(entry.start_in_blamed_file + entry.len.get()),
        });
    }
    Ok(hunks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    fn init_repo() -> TempDir {
        let dir = TempDir::new().unwrap();
        for args in [
            vec!["init", "--quiet", "--initial-branch=main"],
            vec!["config", "user.email", "fixture@nexspec.test"],
            vec!["config", "user.name", "Fixture"],
            vec!["config", "commit.gpgsign", "false"],
        ] {
            assert!(Command::new("git").args(&args).current_dir(dir.path()).status().unwrap().success());
        }
        dir
    }

    fn commit(dir: &Path, msg: &str) {
        assert!(Command::new("git").args(["add", "-A"]).current_dir(dir).status().unwrap().success());
        assert!(Command::new("git").args(["commit", "--quiet", "-m", msg]).current_dir(dir).status().unwrap().success());
    }

    #[test]
    fn blames_the_commit_that_introduced_a_later_line() {
        let dir = init_repo();
        std::fs::write(dir.path().join("file.rs"), "fn a() {}\n").unwrap();
        commit(dir.path(), "first commit");

        std::fs::write(dir.path().join("file.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        commit(dir.path(), "second commit");

        let git = GitSource::open(dir.path()).unwrap();
        let hunks = blame_symbol(&git, Path::new("file.rs"), 1, 1).unwrap();

        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].lines, 1..2);
        assert_eq!(hunks[0].author_email, "fixture@nexspec.test");
    }

    #[test]
    fn blames_the_first_commit_for_its_own_line() {
        let dir = init_repo();
        std::fs::write(dir.path().join("file.rs"), "fn a() {}\n").unwrap();
        commit(dir.path(), "first commit");

        std::fs::write(dir.path().join("file.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        commit(dir.path(), "second commit");

        let git = GitSource::open(dir.path()).unwrap();
        let hunks = blame_symbol(&git, Path::new("file.rs"), 0, 0).unwrap();

        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].lines, 0..1);
    }
}
