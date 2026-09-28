//! Integration tests for `GitSource::diff_since` (T-203, REQ-203).

mod fixtures;

use std::path::PathBuf;

use fixtures::FixtureRepo;
use nexspec::git::GitSource;

#[test]
fn since_none_reports_everything_as_added() {
    let repo = FixtureRepo::init();
    repo.write_file("a.md", "# A");
    repo.write_file("dir/b.md", "# B");
    repo.commit("chore: init");

    let source = GitSource::open(repo.path()).unwrap();
    let diff = source.diff_since(None).unwrap();

    let mut added = diff.added.clone();
    added.sort();
    assert_eq!(
        added,
        vec![PathBuf::from("a.md"), PathBuf::from("dir/b.md")]
    );
    assert!(diff.modified.is_empty());
    assert!(diff.deleted.is_empty());
}

#[test]
fn since_a_commit_classifies_added_modified_deleted() {
    let repo = FixtureRepo::init();
    repo.write_file("a.md", "# A");
    repo.write_file("b.md", "# B");
    let first_oid = repo.commit("chore: first");
    let mut first = [0u8; 20];
    hex_decode(&first_oid, &mut first);

    repo.write_file("a.md", "# A, modified");
    repo.remove_file("b.md");
    repo.write_file("c.md", "# C");
    repo.commit("chore: second");

    let source = GitSource::open(repo.path()).unwrap();
    let diff = source.diff_since(Some(first)).unwrap();

    assert_eq!(diff.added, vec![PathBuf::from("c.md")]);
    assert_eq!(diff.modified, vec![PathBuf::from("a.md")]);
    assert_eq!(diff.deleted, vec![PathBuf::from("b.md")]);
}

fn hex_decode(hex: &str, out: &mut [u8; 20]) {
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
    }
}
