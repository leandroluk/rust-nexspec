//! `nexspec report --diff <rev>` and the read-only revision view behind it
//! (T-1010, REQ-1011).

mod fixtures;

use std::path::Path;
use std::process::{Command, Output};

use fixtures::FixtureRepo;
use nexspec::GitSource;

/// Two commits: the first has `a.ts`/`b.ts` without a cycle; the second adds a
/// file and closes an import cycle.
fn two_commit_repo() -> (FixtureRepo, String) {
    let repo = FixtureRepo::init();
    repo.write_file("src/a.ts", "import { b } from './b';\nexport function a() {\n  b();\n}\n");
    repo.write_file("src/b.ts", "export function b() {}\n");
    repo.write_file(".specs/s.md", "## Requirements\n\n- REQ-1: something\n");
    let first = repo.commit("feat: a and b");
    repo.write_file("src/b.ts", "import { a } from './a';\nexport function b() {\n  a();\n}\n");
    repo.write_file("src/c.ts", "export const c = 1;\n");
    repo.commit("feat: cycle and c");
    (repo, first)
}

fn report(repo: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nexspec"))
        .arg("--repo")
        .arg(repo)
        .arg("report")
        .args(args)
        .output()
        .expect("run nexspec report")
}

#[test]
fn revision_view_reads_the_old_commit_and_ignores_the_working_tree() {
    let (repo, first) = two_commit_repo();
    repo.write_file("src/dirty.ts", "export const d = 1;\n"); // untracked noise

    let old = GitSource::at_revision(repo.path(), "HEAD~1").unwrap();
    let oid_hex: String = old.head_commit_oid().unwrap().iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(oid_hex, first, "HEAD~1 resolves to the first commit");

    let tracked: Vec<String> = old.tracked_paths_at_head().unwrap().iter().map(|p| p.to_string_lossy().replace('\\', "/")).collect();
    assert!(tracked.contains(&"src/a.ts".to_string()) && !tracked.contains(&"src/c.ts".to_string()), "{tracked:?}");

    let b = old.read_blob_at_head(Path::new("src/b.ts")).unwrap().unwrap();
    assert_eq!(String::from_utf8(b).unwrap(), "export function b() {}\n", "old content, not the current file");

    assert!(old.work_dir().is_none(), "no working tree in a revision view");
    assert!(old.dirty_paths().unwrap().is_empty());
    assert!(!old.is_dirty().unwrap());

    let current = GitSource::open(repo.path()).unwrap();
    assert!(current.work_dir().is_some(), "the normal view is unchanged");
    assert!(GitSource::at_revision(repo.path(), "no-such-rev").is_err());
}

#[test]
fn diff_reports_new_files_a_new_cycle_and_leaves_the_repo_untouched() {
    let (repo, _) = two_commit_repo();
    let out = report(repo.path(), &["--diff", "HEAD~1", "--format", "json"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON diff");
    assert_eq!(v["base"], "HEAD~1");
    assert!(v["files_added"].as_array().unwrap().iter().any(|f| f == "src/c.ts"), "{v}");
    assert_eq!(v["import_cycles"]["new"].as_array().unwrap().len(), 1, "{v}");
    assert!(v["import_cycles"]["resolved"].as_array().unwrap().is_empty());
    assert!(v["total_nodes_delta"].as_i64().unwrap() > 0);

    // The comparison indexes the old revision elsewhere: the repo's own
    // working tree shows only the index `report` itself created.
    let status = Command::new("git").args(["status", "--porcelain"]).current_dir(repo.path()).output().unwrap();
    let dirty = String::from_utf8_lossy(&status.stdout).to_string();
    assert!(dirty.lines().all(|l| l.contains(".specs/.index")), "unexpected changes: {dirty}");
}

#[test]
fn markdown_diff_and_fail_on_cycle() {
    let (repo, _) = two_commit_repo();
    let out = report(repo.path(), &["--diff", "HEAD~1"]);
    let md = String::from_utf8_lossy(&out.stdout);
    assert!(md.starts_with("# NexSpec graph diff against `HEAD~1`"), "{md}");
    assert!(md.contains("## Files added") && md.contains("`src/c.ts`"), "{md}");
    assert!(md.contains("new cycle"), "{md}");

    let failing = report(repo.path(), &["--diff", "HEAD~1", "--fail-on-cycle"]);
    assert_eq!(failing.status.code(), Some(2), "a cycle introduced since the base fails the gate");

    // Comparing HEAD with itself introduces nothing.
    let same = report(repo.path(), &["--diff", "HEAD", "--fail-on-cycle"]);
    assert!(same.status.success(), "{}", String::from_utf8_lossy(&same.stderr));
}

#[test]
fn unknown_revision_is_a_readable_error() {
    let (repo, _) = two_commit_repo();
    let out = report(repo.path(), &["--diff", "definitely-not-a-rev"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot resolve revision"), "{}", String::from_utf8_lossy(&out.stderr));
}
