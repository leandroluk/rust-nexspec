//! Integration test for `GitSource::commits_since` (T-206, REQ-207).

mod fixtures;

use fixtures::FixtureRepo;
use nexspec::git::{extract_commit_links, GitSource};

#[test]
fn commits_since_stops_before_the_given_oid_and_links_resolve() {
    let repo = FixtureRepo::init();
    repo.write_file("a.md", "# A");
    let first_oid = repo.commit("chore: add REQ-301 requirement");
    let mut first = [0u8; 20];
    for (i, b) in first.iter_mut().enumerate() {
        *b = u8::from_str_radix(&first_oid[i * 2..i * 2 + 2], 16).unwrap();
    }

    repo.write_file("b.md", "# B");
    repo.commit("feat(x): satisfy REQ-301");
    repo.write_file("c.md", "# C");
    repo.commit("chore: unrelated");

    let source = GitSource::open(repo.path()).unwrap();

    let all = source.commits_since(None).unwrap();
    assert_eq!(all.len(), 3);

    let since_first = source.commits_since(Some(first)).unwrap();
    assert_eq!(since_first.len(), 2, "excludes the first commit itself");
    assert_eq!(since_first[0].message.trim(), "chore: unrelated");
    assert_eq!(since_first[1].message.trim(), "feat(x): satisfy REQ-301");

    let links: Vec<_> = since_first
        .iter()
        .flat_map(|c| extract_commit_links(&c.message))
        .collect();
    assert_eq!(links, vec!["REQ-301"]);
}
