//! `Engine::hit_locations` and the ranking built on it (T-802).

mod fixtures;

use fixtures::FixtureRepo;
use nexspec::Engine;
use nexspec::bench::locate::ranked_for;
use nexspec::bench::metrics::{Expectation, evaluate};
use tempfile::TempDir;

fn engine_over_fixture() -> (FixtureRepo, TempDir, Engine) {
    let repo = FixtureRepo::init();
    repo.write_file(".specs/feat/spec.md", "## Requirements\n\n- REQ-570: invoices must be idempotent\n");
    repo.write_file(
        "src/invoice/handler.ts",
        "// @spec REQ-570\nexport function handleInvoice() {\n  return helper();\n}\nfunction helper() { return 1; }\n",
    );
    repo.write_file("src/other/unrelated.ts", "export function somethingElse() {}\n");
    repo.commit("feat: invoice handler");
    let index = TempDir::new().unwrap();
    let engine = Engine::open(index.path(), repo.path()).unwrap();
    engine.sync().unwrap();
    (repo, index, engine)
}

#[test]
fn symbol_hit_resolves_to_its_file_and_name() {
    let (_repo, _index, engine) = engine_over_fixture();
    let result = engine.search("handleInvoice", None).unwrap();
    let ranked = ranked_for(&engine, &result).unwrap();
    assert!(ranked.symbols.iter().any(|s| s == "handleInvoice"), "{ranked:?}");
    assert!(ranked.files.iter().any(|f| f == "src/invoice/handler.ts"), "{ranked:?}");
}

#[test]
fn requirement_hit_reports_its_marker_and_the_code_that_satisfies_it() {
    let (_repo, _index, engine) = engine_over_fixture();
    let result = engine.search("invoices must be idempotent", None).unwrap();
    let ranked = ranked_for(&engine, &result).unwrap();
    assert!(ranked.markers.iter().any(|m| m == "REQ-570"), "{ranked:?}");
    assert!(
        ranked.files.iter().any(|f| f == "src/invoice/handler.ts"),
        "the file with @spec REQ-570 is an implementer: {ranked:?}"
    );
    assert!(ranked.symbols.iter().any(|s| s == "handleInvoice"), "{ranked:?}");
}

#[test]
fn evaluation_over_real_hits_scores_a_located_file() {
    let (_repo, _index, engine) = engine_over_fixture();
    let result = engine.search("handleInvoice", None).unwrap();
    let ranked = ranked_for(&engine, &result).unwrap();
    let metrics = evaluate(
        &["invoice/handler.ts".to_string(), "REQ-570".to_string(), "missingSymbol".to_string()],
        &ranked,
        &[5],
    );
    assert!(metrics.ranks[0].is_some(), "suffix path matches: {ranked:?}");
    assert!(metrics.ranks[2].is_none());
    assert_eq!(Expectation::parse("missingSymbol").rank(&ranked), None);
    assert!(metrics.recall[0].1 >= 1.0 / 3.0);
}
