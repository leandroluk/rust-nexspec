//! Doubling the input must not much more than double the time (T-908, REQ-904
//! in `.specs/features/performance-guard/spec.md`). Each case takes the best
//! of several runs at N and 2N and asserts `T(2N) / T(N)` stays under
//! `MAX_RATIO`. A quadratic regression shows up as ~4, so the bound has
//! plenty of room for allocator/cache noise while still catching it.
//!
//! `CsrDelta::edges_from` is deliberately not here: it is O(delta) per call
//! and the delta is bounded by the compaction threshold (5% of the base), so
//! it stays small in practice (see design.md, T-908 decision).

mod fixtures;

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use fixtures::synthetic::{SyntheticParams, SyntheticRepo};
use nexspec::code::{Language, extract as extract_code};
use nexspec::git::GitSource;
use nexspec::git::cochange::CoChangeWindow;
use nexspec::graph::csr::CsrDelta;
use nexspec::graph::edge::{Edge, EdgeType};
use nexspec::graph::extract as extract_markdown;
use nexspec::sync::mutation::StableId;

const MAX_RATIO: f64 = 2.5;
const RUNS: usize = 5;

fn best_of(mut f: impl FnMut()) -> Duration {
    (0..RUNS)
        .map(|_| {
            let started = Instant::now();
            f();
            started.elapsed()
        })
        .min()
        .expect("RUNS > 0")
}

fn assert_linear(name: &str, n: usize, mut run: impl FnMut(usize)) {
    run(n / 4); // warm caches and the allocator
    let small = best_of(|| run(n));
    let large = best_of(|| run(2 * n));
    let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-9);
    println!("complexity {name}: T({n})={small:.2?} T({})={large:.2?} ratio={ratio:.2}", 2 * n);
    assert!(ratio < MAX_RATIO, "{name} is not linear: doubling {n} took {ratio:.2}x as long");
}

fn id(i: usize) -> StableId {
    let mut bytes = [0u8; 32];
    bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
    bytes
}

fn edge(i: usize) -> Edge {
    Edge { id: id(i), from: id(i % 997), to: id(i / 997), edge_type: EdgeType::CoChanges }
}

#[test]
fn complexity_csr_delta_upsert_is_linear() {
    assert_linear("CsrDelta::upsert", 100_000, |n| {
        let mut delta = CsrDelta::default();
        for i in 0..n {
            delta.upsert(edge(i));
        }
        std::hint::black_box(delta.len());
    });
}

#[test]
fn complexity_csr_delta_remove_is_linear() {
    assert_linear("CsrDelta::remove", 100_000, |n| {
        let mut delta = CsrDelta::default();
        for i in 0..n {
            delta.upsert(edge(i));
        }
        for i in 0..n {
            delta.remove(id(i));
        }
        std::hint::black_box(delta.len());
    });
}

#[test]
fn complexity_markdown_extract_is_linear() {
    assert_linear("markdown::extract", 4_000, |n| {
        let mut text = String::from("## Requirements\n\n");
        for i in 0..n {
            text.push_str(&format!("- REQ-{i:05}: requirement number {i} with some body text\n"));
        }
        text.push_str("\n## Tasks\n\n");
        for i in 0..n / 4 {
            text.push_str(&format!("### TASK-{i:05}: task {i}\n\n- **REQ**: REQ-{i:05}\n- **What**: work\n\n"));
        }
        std::hint::black_box(extract_markdown(&text));
    });
}

#[test]
fn complexity_code_extract_is_linear_with_calls() {
    // Every function calls its predecessor, so call resolution is exercised
    // on top of symbol extraction (this used to scan all symbols per call).
    assert_linear("code::extract", 12_000, |n| {
        let mut source = String::new();
        for i in 0..n {
            let call = if i > 0 { format!("f{}();", i - 1) } else { String::new() };
            source.push_str(&format!("export function f{i}() {{ {call} }}\n"));
        }
        let set = extract_code(&source, Language::TypeScript, Path::new("big.ts"), &HashMap::new()).unwrap();
        std::hint::black_box(set.edges.len());
    });
}

#[test]
fn complexity_co_change_edges_is_linear_in_commits() {
    let build = |commits: usize| {
        let params = SyntheticParams {
            files: 400,
            markdown_files: 10,
            commits,
            big_commit_files: 100,
            seed: 7,
        };
        SyntheticRepo::generate(&params)
    };
    let small_repo = build(100);
    let large_repo = build(200);
    let window = CoChangeWindow { max_commits: usize::MAX, ..CoChangeWindow::default() };
    let time = |repo: &SyntheticRepo| {
        best_of(|| {
            let git = GitSource::open(repo.path()).unwrap();
            std::hint::black_box(git.co_change_edges(&window).unwrap());
        })
    };
    let (small, large) = (time(&small_repo), time(&large_repo));
    let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-9);
    println!("complexity co_change_edges: 100 commits={small:.2?} 200 commits={large:.2?} ratio={ratio:.2}");
    assert!(ratio < MAX_RATIO, "co_change_edges is not linear in commits: {ratio:.2}x");
}
