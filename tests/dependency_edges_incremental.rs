//! Dependency edges follow the code as it changes (T-706, REQ-706 in
//! `.specs/features/dependency-edges/spec.md`): removing an import removes
//! its edge, renames and deletions leave no orphans, and an unchanged tree
//! stages nothing.

mod fixtures;

use fixtures::ts_workspace::{Workspace, file, symbol};
use nexspec::graph::edge::EdgeType;

fn synced() -> Workspace {
    let mut ws = Workspace::new(fixtures::ts_workspace::build());
    ws.sync();
    ws
}

#[test]
fn removing_an_import_removes_its_file_and_symbol_edges() {
    let mut ws = synced();
    let service = file("src/app/service.ts");
    assert!(ws.has_edge(&service, &file("src/app/util.ts"), EdgeType::Imports).is_some());
    assert!(ws.has_edge(&symbol("src/app/service.ts", "Service"), &symbol("src/app/util.ts", "helper"), EdgeType::Calls).is_some());

    ws.repo.write_file(
        "src/app/service.ts",
        "import { CachePort, RedisAdapter } from '../cache';\n\nexport class Service {\n  constructor(private cache: CachePort) {}\n  run() {\n    return new RedisAdapter();\n  }\n}\n",
    );
    ws.repo.commit("refactor: service no longer uses util");
    ws.sync();

    assert!(ws.has_edge(&service, &file("src/app/util.ts"), EdgeType::Imports).is_none(), "the import is gone");
    assert!(
        ws.has_edge(&symbol("src/app/service.ts", "Service"), &symbol("src/app/util.ts", "helper"), EdgeType::Calls).is_none(),
        "so is the call that depended on it"
    );
    assert!(
        ws.has_edge(&service, &file("src/cache/index.ts"), EdgeType::Imports).is_some(),
        "edges that still hold survive"
    );
    assert!(ws
        .has_edge(&symbol("src/app/service.ts", "Service"), &symbol("src/cache/redis.adapter.ts", "RedisAdapter"), EdgeType::Instantiates)
        .is_some());
}

#[test]
fn an_uncommitted_edit_is_reconciled_too() {
    let mut ws = synced();
    let service = file("src/app/service.ts");
    ws.repo.write_file("src/app/service.ts", "export class Service {}\n"); // dirty, no commit
    ws.sync();
    for edge_type in EdgeType::DEPENDENCY_TYPES {
        assert!(ws.edges(&service, edge_type).is_empty(), "{edge_type:?} edges from service.ts must be gone");
    }
    assert!(
        ws.edges(&symbol("src/app/service.ts", "Service"), EdgeType::References).is_empty(),
        "and so must the symbol-level ones"
    );
}

#[test]
fn deleting_a_file_removes_the_edges_that_pointed_at_it_and_left_from_it() {
    let mut ws = synced();
    let a = file("src/app/a.ts");
    let b = file("src/app/b.ts");
    assert!(ws.has_edge(&a, &b, EdgeType::Imports).is_some());
    assert!(ws.has_edge(&b, &a, EdgeType::Imports).is_some());

    ws.repo.remove_file("src/app/b.ts");
    ws.repo.commit("chore: delete b");
    ws.sync();

    assert!(ws.has_edge(&a, &b, EdgeType::Imports).is_none(), "no edge into the deleted file");
    assert!(ws.edges(&b, EdgeType::Imports).is_empty(), "no edge out of it");
    assert!(ws.edges(&symbol("src/app/b.ts", "b"), EdgeType::Calls).is_empty());
    assert!(
        ws.has_edge(&symbol("src/app/a.ts", "a"), &symbol("src/app/b.ts", "b"), EdgeType::Calls).is_none(),
        "a's call into the deleted symbol is gone"
    );
}

#[test]
fn renaming_a_file_moves_the_edges_when_the_importer_is_updated_with_it() {
    let mut ws = synced();
    ws.repo.remove_file("src/app/util.ts");
    ws.repo.write_file("src/app/helpers.ts", "export function helper() {}\n");
    ws.repo.write_file(
        "src/app/service.ts",
        "import { CachePort, RedisAdapter } from '../cache';\nimport { helper } from './helpers';\n\nexport class Service {\n  constructor(private cache: CachePort) {}\n  run() {\n    helper();\n    return new RedisAdapter();\n  }\n}\n",
    );
    ws.repo.commit("refactor: rename util to helpers");
    ws.sync();

    let service = file("src/app/service.ts");
    assert!(ws.has_edge(&service, &file("src/app/util.ts"), EdgeType::Imports).is_none(), "old target gone");
    assert!(ws.has_edge(&service, &file("src/app/helpers.ts"), EdgeType::Imports).is_some(), "new target linked");
    assert!(ws
        .has_edge(&symbol("src/app/service.ts", "Service"), &symbol("src/app/helpers.ts", "helper"), EdgeType::Calls)
        .is_some());
}

#[test]
fn a_second_sync_without_changes_stages_nothing() {
    let mut ws = synced();
    let report = ws.orchestrator.run_once().unwrap();
    assert!(report.target_version.is_none(), "nothing changed");
    assert_eq!(report.timings.edges, 0);
}

#[test]
fn unchanged_importers_keep_their_edges_when_a_dependency_is_edited() {
    let mut ws = synced();
    // Editing util.ts must not disturb service.ts's edges into it.
    ws.repo.write_file("src/app/util.ts", "export function helper() {\n  return 1;\n}\n");
    ws.repo.commit("feat: helper returns a value");
    ws.sync();
    assert!(ws.has_edge(&file("src/app/service.ts"), &file("src/app/util.ts"), EdgeType::Imports).is_some());
    assert!(ws
        .has_edge(&symbol("src/app/service.ts", "Service"), &symbol("src/app/util.ts", "helper"), EdgeType::Calls)
        .is_some());
}
