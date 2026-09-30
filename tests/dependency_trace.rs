//! `trace` over dependency edges: dependents and dependencies, the per-hop
//! limit with its "omitted" count, and import cycles (T-707, REQ-707 and
//! REQ-712 in `.specs/features/dependency-edges/spec.md`).

mod fixtures;

use fixtures::FixtureRepo;
use nexspec::engine::{TraceOptions, id_hex};
use nexspec::graph::node::{NodePayload, symbol_node_id};
use nexspec::Engine;
use tempfile::TempDir;

fn engine_over(repo: &FixtureRepo) -> (TempDir, Engine) {
    let index = TempDir::new().unwrap();
    let engine = Engine::open(index.path(), repo.path()).unwrap();
    engine.sync().unwrap();
    (index, engine)
}

fn names_of(result: &nexspec::TraceResult, incoming: bool) -> Vec<String> {
    result
        .hops
        .iter()
        .filter(|h| h.incoming == incoming)
        .filter_map(|h| match &h.payload {
            NodePayload::Symbol { name, .. } => Some(name.clone()),
            NodePayload::File { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn trace_of_a_type_lists_the_symbols_that_use_it_with_their_files() {
    let repo = fixtures::ts_workspace::build();
    let (_index, engine) = engine_over(&repo);
    let target = id_hex(&symbol_node_id("src/cache/cache.port.ts", "CachePort", 0));
    let result = engine.trace(&target).unwrap();

    let dependents = names_of(&result, true);
    assert!(dependents.iter().any(|n| n == "Service"), "{dependents:?}");
    assert!(dependents.iter().any(|n| n == "RedisAdapter"), "{dependents:?}");

    let service = result
        .hops
        .iter()
        .find(|h| h.incoming && matches!(&h.payload, NodePayload::Symbol { name, .. } if name == "Service"))
        .unwrap();
    assert_eq!(service.path.as_deref(), Some("src/app/service.ts"), "each symbol hop names its file");
    assert!(service.depth == 1);
}

#[test]
fn trace_of_a_file_level_node_walks_both_directions() {
    let repo = fixtures::ts_workspace::build();
    let (_index, engine) = engine_over(&repo);
    let target = id_hex(&nexspec::graph::node::file_node_id("src/app/util.ts"));
    let result = engine.trace(&target).unwrap();
    let dependents = names_of(&result, true);
    assert!(
        dependents.iter().any(|n| n == "src/app/service.ts") || dependents.iter().any(|n| n == "Service"),
        "service.ts imports util.ts: {dependents:?}"
    );
}

#[test]
fn hops_beyond_the_limit_are_counted_not_listed() {
    let repo = FixtureRepo::init();
    repo.write_file("src/shared.ts", "export function shared() {}\n");
    for i in 0..40 {
        repo.write_file(
            &format!("src/user{i:02}.ts"),
            &format!("import {{ shared }} from './shared';\nexport function use{i:02}() {{\n  shared();\n}}\n"),
        );
    }
    repo.commit("feat: a widely used function");
    let (_index, engine) = engine_over(&repo);
    let target = id_hex(&symbol_node_id("src/shared.ts", "shared", 0));

    let limited = engine
        .trace_with(&target, TraceOptions { max_depth: 1, max_per_hop: 25 })
        .unwrap();
    assert_eq!(limited.hops.len(), 25, "capped at the per-hop limit");
    assert_eq!(limited.omitted, 16, "40 dependents + the defining file = 41 candidates; 25 fit, 16 do not");
    assert!(
        limited.hops.iter().all(|h| h.incoming),
        "dependents are listed before the DefinedIn bookkeeping edge, which is what got cut"
    );

    let unlimited = engine
        .trace_with(&target, TraceOptions { max_depth: 1, max_per_hop: 1000 })
        .unwrap();
    assert_eq!(unlimited.hops.iter().filter(|h| h.incoming).count(), 40);
    assert_eq!(unlimited.omitted, 0);
}

#[test]
fn trusted_dependents_survive_the_cap_before_inferred_or_test_ones() {
    let repo = FixtureRepo::init();
    repo.write_file("src/shared.ts", "export function shared() {}\n");
    repo.write_file("src/real.ts", "import { shared } from './shared';\nexport function real() {\n  shared();\n}\n");
    for i in 0..5 {
        repo.write_file(
            &format!("src/t{i}.spec.ts"),
            &format!("import {{ shared }} from './shared';\nexport function spec{i}() {{\n  shared();\n}}\n"),
        );
    }
    repo.commit("feat: one runtime user, five spec users");
    let (_index, engine) = engine_over(&repo);
    let target = id_hex(&symbol_node_id("src/shared.ts", "shared", 0));
    let result = engine.trace_with(&target, TraceOptions { max_depth: 1, max_per_hop: 1 }).unwrap();
    assert_eq!(names_of(&result, true), vec!["real".to_string()], "the runtime caller outranks the specs");
    assert_eq!(result.omitted, 6, "5 specs + the defining file");
}

#[test]
fn import_cycles_are_reported_as_groups_of_paths() {
    let repo = fixtures::ts_workspace::build();
    let (_index, engine) = engine_over(&repo);
    let cycles = engine.import_cycles().unwrap();
    assert_eq!(cycles, vec![vec!["src/app/a.ts".to_string(), "src/app/b.ts".to_string()]]);
}

#[test]
fn diff_staged_reports_dependents_through_the_new_edges() {
    let repo = fixtures::ts_workspace::build();
    let (_index, engine) = engine_over(&repo);
    // An uncommitted edit to the helper that Service calls.
    repo.write_file("src/app/util.ts", "export function helper() {
  return 1;
}
");

    let diff = engine.diff_staged().unwrap();
    let helper = diff
        .changed_symbols
        .iter()
        .find(|s| s.name == "helper")
        .expect("the edited symbol is reported");
    assert!(
        helper.dependants.contains(&symbol_node_id("src/app/service.ts", "Service", 0)),
        "Service calls helper, so it is in the blast radius: {:?}",
        helper.dependants
    );
}

fn hit_names(result: &nexspec::SearchResult) -> Vec<String> {
    result
        .hits
        .iter()
        .filter_map(|h| match &h.payload {
            NodePayload::Symbol { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_question_about_users_gets_the_dependents_and_a_plain_lookup_does_not() {
    let repo = fixtures::ts_workspace::build();
    let (_index, engine) = engine_over(&repo);

    let asked = hit_names(&engine.search("what uses CachePort", None).unwrap());
    assert!(asked.iter().any(|n| n == "Service"), "dependents are added for 'what uses': {asked:?}");
    assert!(asked.iter().any(|n| n == "RedisAdapter"), "{asked:?}");

    let plain = hit_names(&engine.search("CachePort", None).unwrap());
    assert!(plain.iter().any(|n| n == "CachePort"));
    assert!(!plain.iter().any(|n| n == "Service"), "a locate question does not pay for dependents: {plain:?}");
}

#[test]
fn dependent_intent_detection_covers_english_and_portuguese() {
    for yes in ["what uses X", "callers of handle", "who calls seed_discovery", "impact of changing Y", "quem usa o CachePort", "depende de Z"] {
        assert!(nexspec::engine::asks_for_dependents(yes), "{yes}");
    }
    for no in ["outbox decorator repository dispatch", "how does the wal recover", "CachePort"] {
        assert!(!nexspec::engine::asks_for_dependents(no), "{no}");
    }
}
