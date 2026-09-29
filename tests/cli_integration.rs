//! End-to-end integration test for Fase 6 (T-611): runs the actual
//! `nexspec` binary (via `CARGO_BIN_EXE_nexspec`, cargo's standard way to
//! locate a sibling binary target from an integration test) against a
//! fixture Git repo — init → sync → search/trace/blame/diff, confirming
//! non-empty, coherent output at each step.

use std::path::Path;
use std::process::Command;

fn init_repo(dir: &Path) {
    for args in [
        vec!["init", "--quiet", "--initial-branch=main"],
        vec!["config", "user.email", "fixture@nexspec.test"],
        vec!["config", "user.name", "Fixture"],
        vec!["config", "commit.gpgsign", "false"],
    ] {
        assert!(Command::new("git").args(&args).current_dir(dir).status().unwrap().success());
    }
    std::fs::create_dir_all(dir.join(".specs")).unwrap();
    std::fs::write(dir.join(".specs/req.md"), "## Requirements\n- REQ-1: cli integration test requirement\n").unwrap();
    std::fs::write(dir.join("lib.rs"), "fn hello() {}\n").unwrap();
    assert!(Command::new("git").args(["add", "-A"]).current_dir(dir).status().unwrap().success());
    assert!(
        Command::new("git")
            .args(["commit", "--quiet", "-m", "initial commit"])
            .current_dir(dir)
            .status()
            .unwrap()
            .success()
    );
}

fn nexspec(repo: &Path, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_nexspec"))
        .arg("--repo")
        .arg(repo)
        .args(args)
        .output()
        .expect("failed to run nexspec binary");
    assert!(
        output.status.success(),
        "nexspec {:?} failed: stdout={} stderr={}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

#[test]
fn init_sync_search_trace_blame_diff_all_work_end_to_end() {
    let repo_dir = tempfile::TempDir::new().unwrap();
    init_repo(repo_dir.path());

    let init_out = nexspec(repo_dir.path(), &["init"]);
    assert!(init_out.contains("initialized"));

    let sync_out = nexspec(repo_dir.path(), &["sync"]);
    assert!(sync_out.contains("target_version"));

    let search_out = nexspec(repo_dir.path(), &["search", "cli integration test requirement"]);
    assert!(!search_out.trim().is_empty(), "search should find the synced requirement");

    let trace_out = nexspec(repo_dir.path(), &["trace", "hello"]);
    assert!(trace_out.contains("DefinedIn"), "hello should trace to its file via DefinedIn");

    let blame_out = nexspec(repo_dir.path(), &["blame", "hello"]);
    assert!(blame_out.contains("fixture@nexspec.test"), "blame should attribute the line to the fixture author");

    // No dirty changes yet -- diff --staged prints nothing but must not error.
    let diff_out = nexspec(repo_dir.path(), &["diff", "--staged"]);
    assert!(diff_out.trim().is_empty());

    // Make a dirty change and confirm diff --staged now reports it.
    std::fs::write(repo_dir.path().join("lib.rs"), "fn hello() {}\nfn world() {}\n").unwrap();
    let diff_out_dirty = nexspec(repo_dir.path(), &["diff", "--staged"]);
    assert!(!diff_out_dirty.trim().is_empty(), "a dirty tree should report changed symbols");
}
