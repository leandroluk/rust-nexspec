//! Smoke test for the `tests/fixtures` helper itself (T-201) — not a test of
//! `nexspec` code.

mod fixtures;

use fixtures::FixtureRepo;

#[test]
fn fixture_repo_creates_a_valid_commit() {
    let repo = FixtureRepo::init();
    repo.write_file("README.md", "# hello\n");
    let oid = repo.commit("chore: init fixture");

    assert_eq!(oid.len(), 40, "expected a 40-char hex SHA-1 OID, got {oid:?}");
    assert!(oid.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(repo.head_oid(), oid);
}
