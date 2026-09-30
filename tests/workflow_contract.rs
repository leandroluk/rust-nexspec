//! The exit-code contract of the workflow commands (T-1608, REQ-1606):
//! 0 ok, 1 error, 2 import cycle, 3 stale, 4 no index, 5 doctor failure.
//! The happy/stale/no-index paths are pinned in `workflow_check` and
//! `workflow_doctor`; this file pins the error side and keeps the table in
//! the README from drifting away from the code.

use std::process::Command;

use tempfile::TempDir;

fn nexspec(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_nexspec")).args(args).output().unwrap()
}

#[test]
fn an_execution_error_exits_1_with_a_message_on_stderr() {
    let repo = TempDir::new().unwrap();
    let out = nexspec(&["--repo", repo.path().to_str().unwrap(), "install", "--platform", "nope"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("error: unknown platform"));
    assert!(out.stdout.is_empty(), "errors never go to stdout");
}

#[test]
fn the_readme_documents_every_exit_code() {
    let readme = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md")).unwrap();
    for code in ["0", "1", "2", "3", "4", "5", "6", "7", "8"] {
        let has_row = readme.lines().any(|l| l.split('|').nth(1).is_some_and(|c| c.trim() == format!("`{code}`")));
        assert!(has_row, "README has no row for exit code {code}");
    }
}
