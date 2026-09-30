//! Micro-benchmarks for the operations whose cost must stay linear (T-908,
//! REQ-904 in `.specs/features/performance-guard/spec.md`). Each group runs
//! at N and 2N so `cargo bench` output shows the growth directly; the
//! pass/fail ratio check lives in `tests/complexity.rs`.
//!
//! `cargo bench --bench complexity`

#[path = "../tests/fixtures/mod.rs"]
mod fixtures;

use std::collections::HashMap;
use std::path::Path;

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use fixtures::synthetic::{SyntheticParams, SyntheticRepo};
use nexspec::code::{Language, extract as extract_code};
use nexspec::git::GitSource;
use nexspec::git::cochange::CoChangeWindow;
use nexspec::graph::csr::CsrDelta;
use nexspec::graph::edge::{Edge, EdgeType};
use nexspec::graph::extract as extract_markdown;
use nexspec::sync::mutation::StableId;

fn id(i: usize) -> StableId {
    let mut bytes = [0u8; 32];
    bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
    bytes
}

fn edge(i: usize) -> Edge {
    Edge { id: id(i), from: id(i % 997), to: id(i / 997), edge_type: EdgeType::CoChanges }
}

fn filled_delta(n: usize) -> CsrDelta {
    let mut delta = CsrDelta::default();
    for i in 0..n {
        delta.upsert(edge(i));
    }
    delta
}

fn csr_delta(c: &mut Criterion) {
    let mut group = c.benchmark_group("csr_delta");
    for n in [50_000usize, 100_000] {
        group.bench_with_input(BenchmarkId::new("upsert", n), &n, |b, &n| b.iter(|| filled_delta(n)));
        group.bench_with_input(BenchmarkId::new("remove", n), &n, |b, &n| {
            b.iter_batched(
                || filled_delta(n),
                |mut delta| {
                    for i in 0..n {
                        delta.remove(id(i));
                    }
                    delta
                },
                BatchSize::LargeInput,
            )
        });
        // O(delta) per call by design (bounded by the compaction threshold).
        group.bench_with_input(BenchmarkId::new("edges_from", n), &n, |b, &n| {
            let delta = filled_delta(n);
            let from = id(3);
            b.iter(|| delta.edges_from(&from, EdgeType::CoChanges).count())
        });
    }
    group.finish();
}

fn markdown(c: &mut Criterion) {
    let mut group = c.benchmark_group("markdown_extract");
    for n in [2_000usize, 4_000] {
        let mut text = String::from("## Requirements\n\n");
        for i in 0..n {
            text.push_str(&format!("- REQ-{i:05}: requirement number {i} with some body text\n"));
        }
        group.bench_with_input(BenchmarkId::from_parameter(n), &text, |b, text| b.iter(|| extract_markdown(text)));
    }
    group.finish();
}

fn symbols(c: &mut Criterion) {
    let mut group = c.benchmark_group("code_extract");
    for n in [1_500usize, 3_000] {
        let mut source = String::new();
        for i in 0..n {
            let call = if i > 0 { format!("f{}();", i - 1) } else { String::new() };
            source.push_str(&format!("export function f{i}() {{ {call} }}\n"));
        }
        group.bench_with_input(BenchmarkId::from_parameter(n), &source, |b, source| {
            b.iter(|| extract_code(source, Language::TypeScript, Path::new("big.ts"), &HashMap::new()).unwrap())
        });
    }
    group.finish();
}

fn co_change(c: &mut Criterion) {
    let mut group = c.benchmark_group("co_change_edges");
    group.sample_size(10);
    for commits in [100usize, 200] {
        let params = SyntheticParams { files: 400, markdown_files: 10, commits, big_commit_files: 100, seed: 7 };
        let repo = SyntheticRepo::generate(&params);
        let window = CoChangeWindow { max_commits: usize::MAX, ..CoChangeWindow::default() };
        group.bench_function(BenchmarkId::from_parameter(commits), |b| {
            b.iter(|| GitSource::open(repo.path()).unwrap().co_change_edges(&window).unwrap())
        });
    }
    group.finish();
}

criterion_group!(benches, csr_delta, markdown, symbols, co_change);
criterion_main!(benches);
