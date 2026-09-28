//! Integration tests for `GitSource` (T-202, REQ-201/REQ-208), driven
//! against real repositories created by the `tests/fixtures` helper.

mod fixtures;

use fixtures::FixtureRepo;
use nexspec::git::GitSource;

fn hex(oid: [u8; 20]) -> String {
    oid.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn head_commit_oid_matches_fixture_head_on_a_branch() {
    let repo = FixtureRepo::init();
    repo.write_file("a.txt", "hello");
    let expected_oid = repo.commit("chore: add a.txt");

    let source = GitSource::open(repo.path()).unwrap();
    assert_eq!(hex(source.head_commit_oid().unwrap()), expected_oid);
}

#[test]
fn head_commit_oid_works_on_detached_head() {
    let repo = FixtureRepo::init();
    repo.write_file("a.txt", "hello");
    let first_oid = repo.commit("chore: first");
    repo.write_file("b.txt", "world");
    repo.commit("chore: second");

    repo.checkout_detached(&first_oid);

    let source = GitSource::open(repo.path()).unwrap();
    assert_eq!(
        hex(source.head_commit_oid().unwrap()),
        first_oid,
        "detached HEAD must resolve to the checked-out commit, not fail"
    );
}

#[test]
fn is_dirty_reflects_uncommitted_changes() {
    let repo = FixtureRepo::init();
    repo.write_file("a.txt", "hello");
    repo.commit("chore: add a.txt");

    let source = GitSource::open(repo.path()).unwrap();
    assert!(!source.is_dirty().unwrap(), "clean right after commit");

    repo.write_file("a.txt", "hello, modified");
    let source = GitSource::open(repo.path()).unwrap();
    assert!(source.is_dirty().unwrap(), "tracked file modified without commit");
}
