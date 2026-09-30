//! `nexspec export` end to end (T-1205, REQ-1201..1205).

mod fixtures;

use std::path::Path;
use std::process::{Command, Output};

use fixtures::FixtureRepo;

fn run(repo: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nexspec")).arg("--repo").arg(repo).args(args).output().unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn repo() -> FixtureRepo {
    let repo = FixtureRepo::init();
    repo.write_file(".gitignore", ".specs/.index/\n");
    for (dir, names) in [("a", ["one", "two", "three"]), ("b", ["four", "five", "six"])] {
        for (i, name) in names.iter().enumerate() {
            let next = names[(i + 1) % 3];
            repo.write_file(&format!("src/{dir}/{name}.ts"), &format!("import {{ {next}Fn }} from './{next}';\nexport function {name}Fn() {{ return {next}Fn; }}\n"));
        }
    }
    repo.write_file("src/b/four.ts", "import { oneFn } from '../a/one';\nimport { fiveFn } from './five';\nexport function fourFn() { return [oneFn, fiveFn]; }\n");
    repo.write_file(".specs/feat/spec.md", "## Requirements\n\n- REQ-1: something\n");
    repo.commit("init");
    assert!(run(repo.path(), &["sync"]).status.success());
    repo
}

#[test]
fn json_is_complete_deterministic_and_filterable() {
    let repo = repo();
    let first = out(&run(repo.path(), &["export"]));
    assert_eq!(first, out(&run(repo.path(), &["export"])), "same graph, same bytes");
    let value: serde_json::Value = serde_json::from_str(&first).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert!(value["nodes"].as_array().unwrap().iter().any(|n| n["kind"] == "file" && n["path"] == "src/a/one.ts"));
    assert!(value["edges"].as_array().unwrap().iter().any(|e| e["type"] == "imports" && e["confidence"] == "extracted"));
    assert!(!first.contains("\"timestamp\"") && !first.contains("tool_version"));

    let files_only = out(&run(repo.path(), &["export", "--kind", "file", "--path", "src/a/**"]));
    let value: serde_json::Value = serde_json::from_str(&files_only).unwrap();
    let paths: Vec<&str> = value["nodes"].as_array().unwrap().iter().map(|n| n["path"].as_str().unwrap()).collect();
    assert_eq!(paths.len(), 3, "{paths:?}");
    assert!(paths.iter().all(|p| p.starts_with("src/a/")));

    let bad = run(repo.path(), &["export", "--kind", "banana"]);
    assert_eq!(bad.status.code(), Some(1));
}

#[test]
fn html_tree_and_wiki_are_written_and_check_notices_when_they_go_stale() {
    let repo = repo();
    let dir = tempfile::TempDir::new().unwrap();
    let html = dir.path().join("graph.html");
    let tree = dir.path().join("tree.html");
    let wiki = dir.path().join("wiki");

    for (format, target) in [("html", &html), ("tree", &tree), ("wiki", &wiki)] {
        let done = run(repo.path(), &["export", "--format", format, "--out", target.to_str().unwrap()]);
        assert!(done.status.success(), "{format}: {}", String::from_utf8_lossy(&done.stderr));
    }
    let page = std::fs::read_to_string(&html).unwrap();
    assert!(page.contains("graph-data") && page.contains("src/a/one.ts"));
    assert!(std::fs::read_to_string(&tree).unwrap().contains("role=\"tree\""));
    assert!(wiki.join("index.md").is_file());
    assert!(std::fs::read_dir(wiki.join("communities")).unwrap().count() >= 1, "at least one community article");

    for (format, target) in [("html", &html), ("tree", &tree), ("wiki", &wiki)] {
        let check = run(repo.path(), &["export", "--format", format, "--out", target.to_str().unwrap(), "--check"]);
        assert_eq!(check.status.code(), Some(0), "{format} is up to date: {}", out(&check));
        assert!(out(&check).contains("export up to date"));
    }

    // The graph changes: every export is now stale, and a wiki file nobody generates any more counts too.
    repo.write_file("src/a/extra.ts", "import { oneFn } from './one';\nexport const extra = oneFn;\n");
    repo.commit("more");
    assert!(run(repo.path(), &["sync"]).status.success());
    std::fs::write(wiki.join("communities").join("99-gone.md"), "old").unwrap();
    for (format, target) in [("html", &html), ("wiki", &wiki)] {
        let check = run(repo.path(), &["export", "--format", format, "--out", target.to_str().unwrap(), "--check"]);
        assert_eq!(check.status.code(), Some(7), "{format}: {}", out(&check));
        assert!(out(&check).starts_with("export stale:"), "{}", out(&check));
    }
    let rewritten = run(repo.path(), &["export", "--format", "wiki", "--out", wiki.to_str().unwrap()]);
    assert!(rewritten.status.success());
    assert!(!wiki.join("communities").join("99-gone.md").exists(), "a stale article is removed");
}

#[test]
fn wiki_needs_a_directory_and_unknown_formats_are_refused() {
    let repo = repo();
    let wiki = run(repo.path(), &["export", "--format", "wiki"]);
    assert_eq!(wiki.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&wiki.stderr).contains("--out"));
    assert_eq!(run(repo.path(), &["export", "--format", "graphml"]).status.code(), Some(1));
}
