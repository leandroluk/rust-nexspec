//! Cost baselines without an index (REQ-803, REQ-808): what answering a
//! question costs, in tokens, when an agent has to fall back to `grep`,
//! reading the files it should have found, or reading everything.
//!
//! All three read the files tracked at `HEAD` (like `git grep`), so they are
//! deterministic and match what the index was built from. The `grep` baseline
//! is done in-process and emits `path:line:text` exactly like `grep -rn`, so
//! it does not depend on a `grep` binary being on the `PATH` (design.md D4).

use std::path::Path;

use crate::bench::metrics::Expectation;
use crate::git::{GitError, GitSource};
use crate::token::budget::Tokenizer;

#[derive(Debug, thiserror::Error)]
pub enum BaselineError {
    #[error("git error: {0}")]
    Git(#[from] GitError),
    #[error("expected file {expected:?} matches no tracked file in the target repository")]
    ExpectedFileMissing { expected: String },
}

struct TextFile {
    path: String,
    content: String,
}

/// Text files tracked at `HEAD`, loaded once per benchmark run.
pub struct RepoSnapshot {
    files: Vec<TextFile>,
}

impl RepoSnapshot {
    pub fn load(git: &GitSource) -> Result<Self, BaselineError> {
        let mut paths = git.tracked_paths_at_head()?;
        paths.sort();
        let mut files = Vec::with_capacity(paths.len());
        for path in paths {
            let Some(bytes) = git.read_blob_at_head(&path)? else { continue };
            // Binary files are not something an agent would grep or read.
            if bytes.contains(&0) {
                continue;
            }
            let Ok(content) = String::from_utf8(bytes) else { continue };
            files.push(TextFile { path: normalize(&path), content });
        }
        Ok(Self { files })
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Tokens of `grep -rn <term>` over the tracked text files:
    /// one `path:line:text` row per matching line (case-sensitive substring,
    /// like a plain `grep`).
    pub fn grep_tokens(&self, term: &str, tokenizer: &dyn Tokenizer) -> u32 {
        tokenizer.estimate(&self.grep_output(term))
    }

    /// The text `grep -rn <term>` would print.
    pub fn grep_output(&self, term: &str) -> String {
        let mut out = String::new();
        if term.is_empty() {
            return out;
        }
        for file in &self.files {
            for (index, line) in file.content.lines().enumerate() {
                if line.contains(term) {
                    out.push_str(&format!("{}:{}:{}\n", file.path, index + 1, line));
                }
            }
        }
        out
    }

    /// Tokens of reading, in full, the files named by the path expectations.
    /// `None` when `expect` names no files (only symbols/markers), because
    /// then there is nothing to "read instead". A path that matches no
    /// tracked file is an error: a silent zero would flatter the index.
    pub fn read_tokens(&self, expect: &[String], tokenizer: &dyn Tokenizer) -> Result<Option<u32>, BaselineError> {
        let mut total = 0u32;
        let mut any = false;
        for entry in expect {
            let Expectation::Path(expected) = Expectation::parse(entry) else { continue };
            any = true;
            let expected = expected.trim_start_matches("./");
            let matched: Vec<&TextFile> = self
                .files
                .iter()
                .filter(|f| f.path == expected || f.path.ends_with(&format!("/{expected}")))
                .collect();
            if matched.is_empty() {
                return Err(BaselineError::ExpectedFileMissing { expected: expected.to_string() });
            }
            total += matched.iter().map(|f| tokenizer.estimate(&f.content)).sum::<u32>();
        }
        Ok(any.then_some(total))
    }

    /// Tokens of reading every tracked text file (REQ-808: the same
    /// "whole corpus" baseline `graphify benchmark` reports).
    pub fn corpus_tokens(&self, tokenizer: &dyn Tokenizer) -> u32 {
        self.files.iter().map(|f| tokenizer.estimate(&f.content)).sum()
    }
}

fn normalize(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
