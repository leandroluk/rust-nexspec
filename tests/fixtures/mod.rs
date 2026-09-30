//! Shared test-only helper for creating throwaway Git repositories.
//! Shells out to the system `git` binary — this is explicitly allowed only
//! inside `tests/` fixture setup (never in `src/`), per
//! `.specs/features/git-integration/design.md` → Dependency Paths (REQ-208).
//!
//! `dead_code` is expected here: each `tests/*.rs` file compiles this module
//! as its own copy, and not every integration test uses every method — the
//! full API surface is intentional, not accidental cruft.
#![allow(dead_code)]

pub mod synthetic;

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

pub struct FixtureRepo {
    dir: TempDir,
}

impl FixtureRepo {
    /// A fresh, empty repo with deterministic local config (isolated from
    /// whatever the host machine's global git config does, so tests never
    /// depend on gpgsign/user.name being set globally).
    pub fn init() -> Self {
        let dir = TempDir::new().expect("create temp dir for fixture repo");
        run_git(dir.path(), &["init", "--quiet", "--initial-branch=main"]);
        run_git(dir.path(), &["config", "user.email", "fixture@nexspec.test"]);
        run_git(dir.path(), &["config", "user.name", "Fixture"]);
        run_git(dir.path(), &["config", "commit.gpgsign", "false"]);
        Self { dir }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn write_file(&self, rel_path: &str, content: &str) {
        let full = self.path().join(rel_path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).expect("create parent dirs");
        }
        std::fs::write(full, content).expect("write fixture file");
    }

    pub fn remove_file(&self, rel_path: &str) {
        std::fs::remove_file(self.path().join(rel_path)).expect("remove fixture file");
    }

    /// Stages everything and commits, returning the new commit's OID (hex).
    pub fn commit(&self, message: &str) -> String {
        run_git(self.path(), &["add", "-A"]);
        run_git(self.path(), &["commit", "--quiet", "-m", message]);
        head_oid(self.path())
    }

    /// Detach HEAD at the given commit OID (REQ-208 edge case).
    pub fn checkout_detached(&self, oid: &str) {
        run_git(self.path(), &["checkout", "--quiet", "--detach", oid]);
    }

    pub fn head_oid(&self) -> String {
        head_oid(self.path())
    }
}

fn head_oid(dir: &Path) -> String {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .expect("run git rev-parse HEAD");
    assert!(output.status.success(), "git rev-parse HEAD failed");
    String::from_utf8(output.stdout)
        .expect("git output is utf8")
        .trim()
        .to_string()
}

fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap_or_else(|e| panic!("failed to run git {args:?}: {e}"));
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}
