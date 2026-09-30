//! `nexspec report` end to end (T-1009, REQ-1006/1007/1010).

mod fixtures;

use std::path::Path;
use std::process::{Command, Output};

use fixtures::FixtureRepo;

fn repo(with_cycle: bool) -> FixtureRepo {
    let repo = FixtureRepo::init();
    repo.write_file(".specs/feat/spec.md", "## Requirements\n\n- REQ-1: built and traced\n- REQ-2: nobody implements this\n");
    repo.write_file("src/core.ts", "// @spec REQ-1\nexport function core() {}\n");
    repo.write_file("src/a.ts", "import { core } from './core';\nimport { b } from './b';\nexport function a() {\n  core();\n  b();\n}\n");
    repo.write_file(
        "src/b.ts",
        if with_cycle {
            "import { a } from './a';\nexport function b() {\n  a();\n}\n"
        } else {
            "export function b() {}\n"
        },
    );
    repo.commit("feat: sources");
    repo
}

fn run(repo: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nexspec"))
        .arg("--repo")
        .arg(repo)
        .arg("report")
        .args(args)
        .output()
        .expect("run nexspec report")
}

fn sync(repo: &Path) {
    let out = Command::new(env!("CARGO_BIN_EXE_nexspec")).arg("--repo").arg(repo).arg("sync").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn markdown_report_has_the_documented_sections_and_the_findings() {
    let repo = repo(true);
    sync(repo.path());
    let out = run(repo.path(), &[]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let md = String::from_utf8_lossy(&out.stdout);
    for heading in ["## Summary", "## God Nodes", "## Requirement Coverage", "## Communities", "## Surprising Connections", "## Import Cycles", "## Suggested Questions"] {
        assert!(md.contains(heading), "{heading} missing:\n{md}");
    }
    assert!(md.contains("not implemented: REQ-2"), "{md}");
    assert!(!md.contains("not implemented: REQ-1"), "REQ-1 is satisfied by core(): {md}");
    assert!(md.contains("src/a.ts") && md.contains("src/b.ts"), "the cycle is named: {md}");
    assert!(md.contains("TypeScript"), "{md}");
}

#[test]
fn json_format_and_top_limit() {
    let repo = repo(false);
    sync(repo.path());
    let out = run(repo.path(), &["--format", "json", "--top", "2"]);
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["god_nodes"].as_array().unwrap().len() <= 2);
    assert_eq!(v["requirement_coverage"]["requirements_total"], 2);
    assert!(v["import_cycles"].as_array().unwrap().is_empty());
}

#[test]
fn max_tokens_shortens_the_report() {
    let repo = repo(true);
    sync(repo.path());
    let full = String::from_utf8_lossy(&run(repo.path(), &[]).stdout).to_string();
    let small = String::from_utf8_lossy(&run(repo.path(), &["--max-tokens", "120"]).stdout).to_string();
    assert!(small.len() < full.len(), "{} vs {}", small.len(), full.len());
    assert!(small.contains("## Summary"));
}

#[test]
fn fail_on_cycle_exits_with_2_only_when_a_cycle_exists() {
    let cyclic = repo(true);
    sync(cyclic.path());
    let out = run(cyclic.path(), &["--fail-on-cycle"]);
    assert_eq!(out.status.code(), Some(2), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stderr).contains("import cycle"));

    let clean = repo(false);
    sync(clean.path());
    assert!(run(clean.path(), &["--fail-on-cycle"]).status.success());
}

#[test]
fn unknown_format_is_a_readable_error() {
    let repo = repo(false);
    let out = run(repo.path(), &["--format", "xml"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown format"));
}
