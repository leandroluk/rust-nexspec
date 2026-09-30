//! One test per real bug found while dogfooding on a large repository
//! (T-905, REQ-905 in `.specs/features/performance-guard/spec.md`). Each name
//! is `regression_<bug>` so `cargo test regression_` lists the inventory.

mod fixtures;

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use fixtures::FixtureRepo;
use nexspec::code::{Language, extract as extract_code};
use nexspec::engine::id_hex;
use nexspec::git::GitSource;
use nexspec::graph::csr::CsrDelta;
use nexspec::graph::edge::{Edge, EdgeType};
use nexspec::graph::extract as extract_markdown;
use nexspec::graph::node::NodePayload;
use nexspec::sync::mutation::{MutationSet, NodeMutation, StableId};
use nexspec::Engine;
use tempfile::TempDir;

fn marker_id(marker: &str) -> StableId {
    *blake3::hash(format!("marker:{marker}").as_bytes()).as_bytes()
}

fn decode(payload: &[u8]) -> NodePayload {
    let mut aligned = rkyv::util::AlignedVec::<16>::new();
    aligned.extend_from_slice(payload);
    rkyv::from_bytes::<NodePayload, rkyv::rancor::Error>(&aligned).expect("valid NodePayload")
}

fn symbol_names(set: &MutationSet) -> Vec<String> {
    set.nodes
        .iter()
        .filter_map(|m| match m {
            NodeMutation::Upsert { payload, .. } => match decode(payload) {
                NodePayload::Symbol { name, .. } => Some(name),
                _ => None,
            },
            NodeMutation::Remove { .. } => None,
        })
        .collect()
}

fn git_config(dir: &Path, key: &str, value: &str) {
    let status = Command::new("git")
        .args(["config", key, value])
        .current_dir(dir)
        .status()
        .expect("run git config");
    assert!(status.success());
}

fn open_engine(repo: &FixtureRepo, index: &TempDir) -> Engine {
    Engine::open(index.path(), repo.path()).expect("open engine")
}

// (a) `CsrDelta::upsert` was O(n) per call (linear `retain`), making a bulk
// sync of ~1.1M co-change edges effectively never finish.
#[test]
fn regression_csr_upsert_bulk_is_linear() {
    let n: u32 = 200_000;
    let id = |i: u32| -> StableId {
        let mut bytes = [0u8; 32];
        bytes[..4].copy_from_slice(&i.to_le_bytes());
        bytes
    };
    let started = Instant::now();
    let mut delta = CsrDelta::default();
    for i in 0..n {
        delta.upsert(Edge {
            id: id(i),
            from: id(i % 1000),
            to: id(i / 1000),
            edge_type: EdgeType::CoChanges,
            meta: 0,
        });
    }
    // Replacing existing ids must stay O(1) too.
    for i in 0..n {
        delta.upsert(Edge { id: id(i), from: id(0), to: id(1), edge_type: EdgeType::CoChanges, meta: 0 });
    }
    assert_eq!(delta.len(), n as usize);
    let elapsed = started.elapsed();
    assert!(elapsed.as_secs_f64() < 3.0, "400k upserts took {elapsed:?}; quadratic behaviour is back");
}

// (b) A sync with nothing new re-staged every edge (co-change recomputed and
// dirty-scan noise), appending a huge frame to the WAL each time.
#[test]
fn regression_noop_sync_stages_nothing() {
    let repo = FixtureRepo::init();
    repo.write_file("spec.md", "## Requirements\n- REQ-530: steady state\n");
    repo.write_file("a.ts", "export function alpha() {}\n");
    repo.commit("chore: seed");
    let index = TempDir::new().unwrap();
    let engine = open_engine(&repo, &index);

    let first = engine.sync().unwrap();
    assert!(first.target_version.is_some());
    let wal = index.path().join("sync.wal");
    let wal_after_first = std::fs::metadata(&wal).unwrap().len();

    let second = engine.sync().unwrap();
    assert!(second.target_version.is_none(), "nothing changed, nothing to stage");
    assert_eq!(second.timings.edges, 0);
    assert_eq!(std::fs::metadata(&wal).unwrap().len(), wal_after_first, "no new WAL frame");
}

// (c) The dirty check ignored untracked files, so a brand-new spec that was
// never `git add`ed was invisible to the index.
#[test]
fn regression_untracked_only_file_is_indexed() {
    let repo = FixtureRepo::init();
    repo.write_file("README.txt", "unrelated\n");
    repo.commit("chore: readme");
    repo.write_file(".specs/new.md", "## Requirements\n- REQ-540: brand new untracked requirement\n");
    let index = TempDir::new().unwrap();
    let engine = open_engine(&repo, &index);

    let report = engine.sync().unwrap();
    assert!(report.files_dirty >= 1, "the untracked spec must be picked up");
    let hits = engine.search("brand new untracked requirement", None).unwrap().hits;
    assert!(!hits.is_empty(), "untracked requirement must be searchable");
}

// (d) With `core.autocrlf` on, every CRLF working-tree file looked modified
// (hash of the raw bytes != blob), so `dirty` listed the whole repository.
#[test]
fn regression_dirty_respects_git_status_with_autocrlf() {
    let repo = FixtureRepo::init();
    git_config(repo.path(), "core.autocrlf", "true");
    for name in ["a.md", "b.md", "c.md"] {
        repo.write_file(name, "line one\r\nline two\r\n");
    }
    repo.commit("chore: crlf files");
    let git = GitSource::open(repo.path()).unwrap();
    assert!(
        git.dirty_paths().unwrap().is_empty(),
        "untouched CRLF files are clean according to git"
    );

    repo.write_file("b.md", "line one\r\nline two changed\r\n");
    let dirty = git.dirty_paths().unwrap();
    assert_eq!(dirty, vec![Path::new("b.md").to_path_buf()], "only the edited file is dirty");
}

// (e) `export abstract class`, `enum` and `type` aliases produced no symbols.
#[test]
fn regression_abstract_class_enum_type_alias_are_symbols() {
    let source = "export abstract class Base { abstract run(): void }\n\
                  export enum Color { Red, Green }\n\
                  export type Id = string;\n\
                  export interface Shape { area(): number }\n";
    let set = extract_code(source, Language::TypeScript, Path::new("shapes.ts"), &HashMap::new()).unwrap();
    let names = symbol_names(&set);
    for expected in ["Base", "Color", "Id", "Shape"] {
        assert!(names.iter().any(|n| n == expected), "{expected} missing from {names:?}");
    }
}

// (f) Ids with a prefix (`REQ-CTR-001`) or a suffix (`REQ-021b`) were not
// recognised, so specs using them had no nodes at all.
#[test]
fn regression_prefixed_and_suffixed_requirement_ids() {
    let set = extract_markdown("## Requirements\n\n- REQ-CTR-001: prefixed id\n- REQ-021b: suffixed id\n- REQ-022: plain id\n");
    let ids: Vec<StableId> = set
        .nodes
        .iter()
        .filter_map(|m| match m {
            NodeMutation::Upsert { id, .. } => Some(*id),
            NodeMutation::Remove { .. } => None,
        })
        .collect();
    for marker in ["REQ-CTR-001", "REQ-021b", "REQ-022"] {
        assert!(ids.contains(&marker_id(marker)), "{marker} was not extracted");
    }
}

// (g) A TASK in `tasks.md` did not link to a REQ defined in another file's
// `spec.md`, and `trace REQ-…` did not follow edges against their direction,
// so it never returned the implementers.
#[test]
fn regression_task_links_to_requirement_across_files_and_trace_finds_implementers() {
    let repo = FixtureRepo::init();
    repo.write_file(".specs/features/f/spec.md", "## Requirements\n\n- REQ-550: cross file requirement\n");
    repo.write_file(
        ".specs/features/f/tasks.md",
        "### TASK-550: Build it\n\n- **REQ**: REQ-550\n- **What**: implement the handler\n",
    );
    repo.write_file(
        "src/handler.ts",
        "// @spec REQ-550\nexport function handleRequest() {\n  return 1;\n}\n",
    );
    repo.commit("feat: handler for REQ-550");
    let index = TempDir::new().unwrap();
    let engine = open_engine(&repo, &index);
    engine.sync().unwrap();

    let trace = engine.trace(&id_hex(&marker_id("REQ-550"))).unwrap();
    let has_task = trace
        .hops
        .iter()
        .any(|h| h.incoming && matches!(&h.payload, NodePayload::Task { title, .. } if title == "TASK-550"));
    let has_symbol = trace
        .hops
        .iter()
        .any(|h| h.incoming && matches!(&h.payload, NodePayload::Symbol { name, .. } if name == "handleRequest"));
    assert!(has_task, "TASK-550 must link to REQ-550 across files: {} hops", trace.hops.len());
    assert!(has_symbol, "the @spec-annotated symbol must be reported as an implementer");
}
