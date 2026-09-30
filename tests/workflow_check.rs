//! `nexspec check-update` (T-1604, REQ-1603): one parseable line and stable exit codes.

mod fixtures;

use std::path::Path;
use std::process::{Command, Output};

use fixtures::FixtureRepo;

fn run(repo: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nexspec")).arg("--repo").arg(repo).args(args).output().unwrap()
}

fn first_line(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).lines().next().unwrap_or("").to_string()
}

#[test]
fn no_index_up_to_date_and_stale_have_their_own_line_and_exit_code() {
    let repo = FixtureRepo::init();
    repo.write_file(".gitignore", ".specs/.index/\n");
    repo.write_file("src/a.ts", "export const a = 1;\n");
    repo.commit("init");

    let none = run(repo.path(), &["check-update"]);
    assert_eq!((first_line(&none).as_str(), none.status.code()), ("no-index", Some(4)));
    assert!(!repo.path().join(".specs/.index").exists(), "check-update never creates the index");

    { let o = run(repo.path(), &["sync"]); assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr)); }
    let fresh = run(repo.path(), &["check-update"]);
    assert_eq!((first_line(&fresh).as_str(), fresh.status.code()), ("up-to-date", Some(0)));

    repo.write_file("src/b.ts", "export const b = 2;\n");
    let dirty = run(repo.path(), &["check-update"]);
    assert!(first_line(&dirty).starts_with("stale: "), "{}", first_line(&dirty));
    assert_eq!(dirty.status.code(), Some(3));

    { let o = run(repo.path(), &["sync"]); assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr)); }
    repo.commit("add b");
    let moved = run(repo.path(), &["check-update"]);
    assert!(first_line(&moved).starts_with("stale: HEAD is"), "{}", first_line(&moved));
    assert_eq!(moved.status.code(), Some(3));

    { let o = run(repo.path(), &["sync"]); assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr)); }
    assert_eq!(run(repo.path(), &["check-update"]).status.code(), Some(0));
}
