//! Symbol ids are stable across edits and symbols that disappear leave
//! nothing behind (T-702, REQ-706 in
//! `.specs/features/dependency-edges/spec.md`).

mod fixtures;

use fixtures::FixtureRepo;
use nexspec::Engine;
use nexspec::graph::node::{NodePayload, symbol_node_id};
use tempfile::TempDir;

fn open(repo: &FixtureRepo) -> (TempDir, Engine) {
    let index = TempDir::new().unwrap();
    let engine = Engine::open(index.path(), repo.path()).unwrap();
    engine.sync().unwrap();
    (index, engine)
}

fn has_symbol(engine: &Engine, path: &str, name: &str) -> bool {
    matches!(
        engine.node_payload(&symbol_node_id(path, name, 0)).unwrap(),
        Some(NodePayload::Symbol { .. })
    )
}

fn line_of(engine: &Engine, path: &str, name: &str) -> u32 {
    match engine.node_payload(&symbol_node_id(path, name, 0)).unwrap() {
        Some(NodePayload::Symbol { line_start, .. }) => line_start,
        other => panic!("{name} missing: {other:?}"),
    }
}

#[test]
fn symbol_id_survives_edits_that_move_it() {
    let repo = FixtureRepo::init();
    repo.write_file("src/a.ts", "export function alpha() {}\nexport function beta() {}\n");
    repo.commit("feat: a");
    let (_index, engine) = open(&repo);
    assert_eq!(line_of(&engine, "src/a.ts", "beta"), 1);

    repo.write_file("src/a.ts", "// header\n// more header\n\nexport function alpha() {}\nexport function beta() {}\n");
    repo.commit("chore: push everything down");
    engine.sync().unwrap();

    assert_eq!(line_of(&engine, "src/a.ts", "beta"), 4, "same id, new line range");
    assert!(has_symbol(&engine, "src/a.ts", "alpha"));
}

#[test]
fn a_symbol_removed_from_a_file_is_removed_from_the_index() {
    let repo = FixtureRepo::init();
    repo.write_file("src/a.ts", "export function alpha() {}\nexport function beta() {}\n");
    repo.commit("feat: a");
    let (_index, engine) = open(&repo);
    assert!(has_symbol(&engine, "src/a.ts", "beta"));
    assert!(!engine.search("beta", None).unwrap().hits.is_empty());

    repo.write_file("src/a.ts", "export function alpha() {}\n");
    repo.commit("refactor: drop beta");
    engine.sync().unwrap();

    assert!(!has_symbol(&engine, "src/a.ts", "beta"), "the node is gone");
    assert!(has_symbol(&engine, "src/a.ts", "alpha"));
    let hits = engine.search("beta", None).unwrap().hits;
    assert!(
        hits.iter().all(|h| !matches!(&h.payload, NodePayload::Symbol { name, .. } if name == "beta")),
        "search must not return the removed symbol"
    );
    let trace = engine.trace(&nexspec::engine::id_hex(&symbol_node_id("src/a.ts", "alpha", 0))).unwrap();
    assert!(
        trace.hops.iter().all(|h| !matches!(&h.payload, NodePayload::Symbol { name, .. } if name == "beta")),
        "no dangling edge to the removed symbol"
    );
}

#[test]
fn deleting_a_file_removes_its_symbols() {
    let repo = FixtureRepo::init();
    repo.write_file("src/a.ts", "export function alpha() {}\n");
    repo.write_file("src/b.ts", "export function gamma() {}\n");
    repo.commit("feat: a and b");
    let (_index, engine) = open(&repo);
    assert!(has_symbol(&engine, "src/a.ts", "alpha"));

    repo.remove_file("src/a.ts");
    repo.commit("chore: delete a");
    engine.sync().unwrap();

    assert!(!has_symbol(&engine, "src/a.ts", "alpha"));
    assert!(has_symbol(&engine, "src/b.ts", "gamma"), "other files are untouched");
}

#[test]
fn uncommitted_edits_replace_the_committed_symbols_too() {
    let repo = FixtureRepo::init();
    repo.write_file("src/a.ts", "export function alpha() {}\nexport function beta() {}\n");
    repo.commit("feat: a");
    let (_index, engine) = open(&repo);

    repo.write_file("src/a.ts", "export function alpha() {}\n"); // not committed
    engine.sync().unwrap();
    assert!(!has_symbol(&engine, "src/a.ts", "beta"));
    assert!(has_symbol(&engine, "src/a.ts", "alpha"));
}

#[test]
fn homonyms_in_one_file_get_distinct_stable_ordinals() {
    let repo = FixtureRepo::init();
    repo.write_file("src/a.ts", "export class A {\n  run() {}\n}\nexport class B {\n  run() {}\n}\n");
    repo.commit("feat: two run methods");
    let (_index, engine) = open(&repo);
    let first = engine.node_payload(&symbol_node_id("src/a.ts", "run", 0)).unwrap();
    let second = engine.node_payload(&symbol_node_id("src/a.ts", "run", 1)).unwrap();
    assert!(first.is_some() && second.is_some(), "both `run` methods are indexed under different ids");
}
