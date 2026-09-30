//! The Git merge driver for `*.graph.json` (T-1306, REQ-1306): registered by `hook install`, used by a real `git merge`.

mod fixtures;

use std::path::Path;
use std::process::{Command, Output};

use fixtures::FixtureRepo;

fn nexspec(repo: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nexspec")).arg("--repo").arg(repo).args(args).output().unwrap()
}

fn git(repo: &Path, args: &[&str]) -> Output {
    Command::new("git").current_dir(repo).args(args).output().unwrap()
}

fn graph(nodes: &[(&str, &str)]) -> String {
    let nodes: Vec<String> = nodes
        .iter()
        .map(|(id, path)| {
            format!(r#"{{"id":"{id}","kind":"file","label":"{path}","path":"{path}","payload":{{"type":"file","path":"{path}","source_hash":"{}"}}}}"#, "00".repeat(32))
        })
        .collect();
    format!(r#"{{"schema_version":1,"nodes":[{}],"edges":[],"communities":[]}}"#, nodes.join(","))
}

#[test]
fn hook_install_registers_the_driver_and_uninstall_takes_it_away() {
    let repo = FixtureRepo::init();
    repo.write_file(".gitattributes", "*.png binary\n");
    let installed = nexspec(repo.path(), &["hook", "install"]);
    assert!(installed.status.success(), "{}", String::from_utf8_lossy(&installed.stderr));
    let text = String::from_utf8_lossy(&installed.stdout).to_string();
    assert!(text.contains("gitattributes: installed") && text.contains("merge-driver: installed"), "{text}");

    let attributes = std::fs::read_to_string(repo.path().join(".gitattributes")).unwrap();
    assert!(attributes.contains("*.png binary") && attributes.contains("*.graph.json merge=nexspec"), "{attributes}");
    let config = std::fs::read_to_string(repo.path().join(".git/config")).unwrap();
    assert!(config.contains("[merge \"nexspec\"]") && config.contains("driver = nexspec merge-driver %O %A %B"), "{config}");

    let again = String::from_utf8_lossy(&nexspec(repo.path(), &["hook", "install"]).stdout).to_string();
    assert!(again.contains("gitattributes: already installed") && again.contains("merge-driver: already installed"), "{again}");
    assert_eq!(std::fs::read_to_string(repo.path().join(".git/config")).unwrap().matches("[merge \"nexspec\"]").count(), 1);

    let status = String::from_utf8_lossy(&nexspec(repo.path(), &["hook", "status"]).stdout).to_string();
    assert!(status.contains("merge-driver: installed"), "{status}");

    assert!(nexspec(repo.path(), &["hook", "uninstall"]).status.success());
    assert_eq!(std::fs::read_to_string(repo.path().join(".gitattributes")).unwrap(), "*.png binary\n", "the user's attributes are back as they were");
    assert!(!std::fs::read_to_string(repo.path().join(".git/config")).unwrap().contains("[merge"), "the driver section is gone");
}

#[test]
fn a_real_git_merge_of_two_branches_keeps_both_sides_nodes() {
    let repo = FixtureRepo::init();
    // What `hook install` writes, by hand: installing the post-commit hook here would make every commit of
    // this test start a background `nexspec sync`, and on Windows its lock file trips `git add`.
    repo.write_file(".gitattributes", "*.graph.json merge=nexspec\n");
    // Git runs the driver through the shell; point it at the binary under test.
    let exe = env!("CARGO_BIN_EXE_nexspec").replace('\\', "/");
    assert!(git(repo.path(), &["config", "merge.nexspec.driver", &format!("\"{exe}\" merge-driver %O %A %B")]).status.success());

    repo.write_file("g.graph.json", &graph(&[("aa", "base.ts")]));
    repo.commit("base");
    assert!(git(repo.path(), &["checkout", "--quiet", "-b", "feature"]).status.success());
    repo.write_file("g.graph.json", &graph(&[("aa", "base.ts"), ("bb", "feature.ts")]));
    repo.commit("feature adds a node");
    assert!(git(repo.path(), &["checkout", "--quiet", "main"]).status.success());
    repo.write_file("g.graph.json", &graph(&[("aa", "base.ts"), ("cc", "main.ts")]));
    repo.commit("main adds another");

    let merge = git(repo.path(), &["merge", "--no-edit", "feature"]);
    assert!(merge.status.success(), "{}{}", String::from_utf8_lossy(&merge.stdout), String::from_utf8_lossy(&merge.stderr));
    let merged: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(repo.path().join("g.graph.json")).unwrap()).unwrap();
    let ids: Vec<&str> = merged["nodes"].as_array().unwrap().iter().map(|n| n["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["aa", "bb", "cc"], "the union, in a stable order");
}

#[test]
fn the_driver_command_fails_clearly_on_a_conflicted_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let (base, ours, theirs) = (dir.path().join("o"), dir.path().join("a"), dir.path().join("b"));
    std::fs::write(&base, "").unwrap();
    std::fs::write(&ours, graph(&[("aa", "a.ts")])).unwrap();
    std::fs::write(&theirs, "<<<<<<< HEAD\n{}\n=======\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_nexspec")).arg("merge-driver").arg(&base).arg(&ours).arg(&theirs).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "Git treats a non-zero exit as a conflict it must leave for the user");
    assert!(String::from_utf8_lossy(&out.stderr).contains("not a graph export"), "{}", String::from_utf8_lossy(&out.stderr));
}
