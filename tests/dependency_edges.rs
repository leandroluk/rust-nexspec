//! Cross-file dependency edges over a small TypeScript workspace (T-705,
//! REQ-701..704, REQ-710 in `.specs/features/dependency-edges/spec.md`):
//! path aliases, a barrel with re-exports, type-only imports, an import
//! cycle, an external package and an unresolved specifier.

mod fixtures;

use fixtures::ts_workspace::{Workspace, file, symbol};
use nexspec::graph::edge::{Confidence, EdgeContext, EdgeType};
use nexspec::sync::mutation::StableId;

fn synced() -> Workspace {
    let mut ws = Workspace::new(fixtures::ts_workspace::build());
    ws.sync();
    ws
}

#[test]
fn file_level_imports_follow_relative_alias_and_barrel_but_not_external_or_missing() {
    let ws = synced();
    let service = file("src/app/service.ts");
    let mut targets: Vec<StableId> = ws.edges(&service, EdgeType::Imports).iter().map(|e| e.to).collect();
    targets.sort();
    let mut expected = vec![file("src/cache/index.ts"), file("src/app/util.ts")];
    expected.sort();
    assert_eq!(targets, expected, "barrel + tsconfig alias only; `some-external-pkg` and `./does-not-exist` add nothing");

    let edge = ws.has_edge(&service, &file("src/app/util.ts"), EdgeType::Imports).unwrap();
    assert_eq!(edge.confidence(), Confidence::Extracted);
    assert_eq!(edge.context(), EdgeContext::Runtime);
}

#[test]
fn barrel_reexports_are_file_level_reexport_edges() {
    let ws = synced();
    let barrel = file("src/cache/index.ts");
    let mut targets: Vec<StableId> = ws.edges(&barrel, EdgeType::ReExports).iter().map(|e| e.to).collect();
    targets.sort();
    let mut expected = vec![file("src/cache/cache.port.ts"), file("src/cache/redis.adapter.ts")];
    expected.sort();
    assert_eq!(targets, expected);
}

#[test]
fn uses_link_to_the_symbol_that_declares_the_name_even_through_a_barrel() {
    let ws = synced();
    let service = symbol("src/app/service.ts", "Service");

    // Constructor parameter type, imported through `../cache` (a barrel).
    let port = ws.has_edge(&service, &symbol("src/cache/cache.port.ts", "CachePort"), EdgeType::References);
    let port = port.expect("Service -> CachePort through the barrel");
    assert_eq!(port.confidence(), Confidence::Extracted);
    assert_eq!(port.context(), EdgeContext::TypeOnly, "a type position is type-only");

    let created = ws.has_edge(&service, &symbol("src/cache/redis.adapter.ts", "RedisAdapter"), EdgeType::Instantiates);
    assert!(created.is_some(), "new RedisAdapter() -> the class, resolved through an `export ... from` barrel");

    let called = ws.has_edge(&service, &symbol("src/app/util.ts", "helper"), EdgeType::Calls);
    assert!(called.is_some(), "helper() through the `@app/*` alias");
}

#[test]
fn implements_becomes_an_extends_edge_and_type_only_imports_are_flagged() {
    let ws = synced();
    let adapter = symbol("src/cache/redis.adapter.ts", "RedisAdapter");
    let edge = ws
        .has_edge(&adapter, &symbol("src/cache/cache.port.ts", "CachePort"), EdgeType::Extends)
        .expect("RedisAdapter implements CachePort");
    assert_eq!(edge.context(), EdgeContext::TypeOnly, "`import type` is type-only");
    let file_edge = ws
        .has_edge(&file("src/cache/redis.adapter.ts"), &file("src/cache/cache.port.ts"), EdgeType::Imports)
        .unwrap();
    assert_eq!(file_edge.context(), EdgeContext::TypeOnly);
}

#[test]
fn import_cycles_are_representable_in_both_directions() {
    let ws = synced();
    assert!(ws.has_edge(&file("src/app/a.ts"), &file("src/app/b.ts"), EdgeType::Imports).is_some());
    assert!(ws.has_edge(&file("src/app/b.ts"), &file("src/app/a.ts"), EdgeType::Imports).is_some());
    assert!(ws.has_edge(&symbol("src/app/a.ts", "a"), &symbol("src/app/b.ts", "b"), EdgeType::Calls).is_some());
}

#[test]
fn edges_from_spec_files_carry_the_spec_context() {
    let ws = synced();
    let spec = file("src/app/service.spec.ts");
    let edge = ws.has_edge(&spec, &file("src/app/service.ts"), EdgeType::Imports).expect("spec imports the service");
    assert_eq!(edge.context(), EdgeContext::Spec);
    let created = ws
        .edges(&spec, EdgeType::Instantiates)
        .into_iter()
        .chain(ws.edges(&symbol("src/app/service.spec.ts", "describe"), EdgeType::Instantiates))
        .find(|e| e.to == symbol("src/app/service.ts", "Service"));
    // The `new Service(...)` sits inside callbacks, not inside a declared symbol.
    assert_eq!(created.map(|e| e.context()), Some(EdgeContext::Spec));
}

#[test]
fn unresolved_or_external_imports_create_no_edges_at_all() {
    let ws = synced();
    let service = file("src/app/service.ts");
    for edge_type in EdgeType::DEPENDENCY_TYPES {
        for edge in ws.edges(&service, edge_type) {
            let known = [file("src/cache/index.ts"), file("src/app/util.ts")];
            assert!(known.contains(&edge.to), "{edge_type:?} edge to an unexpected target");
        }
    }
}
