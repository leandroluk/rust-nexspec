//! `nexspec doctor` (T-1606, REQ-1605): actionable lines, exit 5 only on failure.

mod fixtures;

use std::path::Path;
use std::process::{Command, Output};

use fixtures::FixtureRepo;
use tempfile::TempDir;

fn run(repo: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nexspec"))
        .arg("--repo")
        .arg(repo)
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .output()
        .unwrap()
}

fn text(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

#[test]
fn a_repository_without_an_index_fails_with_the_fixing_command_and_a_healthy_one_passes() {
    let home = TempDir::new().unwrap();
    let repo = FixtureRepo::init();
    repo.write_file("src/a.ts", "export const a = 1;\n");
    repo.commit("init");

    let broken = run(repo.path(), home.path(), &["doctor"]);
    assert_eq!(broken.status.code(), Some(5), "{}", text(&broken));
    let out = text(&broken);
    assert!(out.contains("[FAIL] index") && out.contains("fix: nexspec sync"), "{out}");
    assert!(out.contains("[warn] hooks") && out.contains("fix: nexspec hook install"), "{out}");
    assert!(out.contains("[warn] mcp") && out.contains("nexspec install --platform"), "{out}");

    repo.write_file(".gitignore", ".specs/.index/\n");
    repo.commit("ignore index");
    assert!(run(repo.path(), home.path(), &["sync"]).status.success());
    assert!(run(repo.path(), home.path(), &["hook", "install"]).status.success());
    assert!(run(repo.path(), home.path(), &["install", "--platform", "claude"]).status.success());

    let healthy = run(repo.path(), home.path(), &["doctor"]);
    let out = text(&healthy);
    assert_eq!(healthy.status.code(), Some(0), "warnings alone do not fail: {out}");
    assert!(out.contains("[ok  ] index") && out.contains("[ok  ] hooks") && out.contains("configured for: claude"), "{out}");
}
