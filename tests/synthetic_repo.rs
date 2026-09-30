//! Tests for the synthetic repository generator (T-902, REQ-901).

mod fixtures;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use fixtures::synthetic::{SyntheticParams, SyntheticRepo};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").args(args).current_dir(dir).output().expect("run git");
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8(out.stdout).expect("utf8 git output")
}

/// `(path, blake3 of content)` for every tracked file, sorted.
fn tree_fingerprint(dir: &Path) -> Vec<(String, String)> {
    let mut entries: Vec<(String, String)> = git(dir, &["ls-files"])
        .lines()
        .map(|path| {
            let bytes = std::fs::read(dir.join(path)).expect("read tracked file");
            (path.to_string(), blake3::hash(&bytes).to_hex().to_string())
        })
        .collect();
    entries.sort();
    entries
}

/// Files changed per commit, oldest first.
fn commit_sizes(dir: &Path) -> Vec<usize> {
    let log = git(dir, &["log", "--root", "--name-only", "--format=@@%H", "--reverse"]);
    let mut sizes = Vec::new();
    for line in log.lines() {
        if line.starts_with("@@") {
            sizes.push(0);
        } else if !line.trim().is_empty() {
            *sizes.last_mut().expect("commit header first") += 1;
        }
    }
    sizes
}

fn small() -> SyntheticParams {
    SyntheticParams::default().scaled(0.1)
}

#[test]
fn same_seed_produces_same_tree_and_topology() {
    let a = SyntheticRepo::generate(&small());
    let b = SyntheticRepo::generate(&small());
    assert_eq!(tree_fingerprint(a.path()), tree_fingerprint(b.path()));
    assert_eq!(commit_sizes(a.path()), commit_sizes(b.path()));
}

#[test]
fn different_seed_changes_content_layout() {
    let a = SyntheticRepo::generate(&small());
    let other = SyntheticParams { seed: 42, ..small() };
    let b = SyntheticRepo::generate(&other);
    assert_ne!(commit_sizes(a.path()), commit_sizes(b.path()));
}

#[test]
fn commit_count_files_and_markdown_match_params() {
    let params = small();
    let repo = SyntheticRepo::generate(&params);
    let count: usize = git(repo.path(), &["rev-list", "--count", "HEAD"]).trim().parse().unwrap();
    assert_eq!(count, params.commits);

    let files = tree_fingerprint(repo.path());
    assert_eq!(files.len(), params.files);
    let md = files.iter().filter(|(p, _)| p.ends_with(".md")).count();
    assert_eq!(md, params.markdown_files);

    let status = git(repo.path(), &["status", "--porcelain"]);
    assert!(status.trim().is_empty(), "working tree must match HEAD, got: {status}");
}

#[test]
fn exactly_one_big_commit() {
    let params = SyntheticParams::default();
    let repo = SyntheticRepo::generate(&params);
    let sizes = commit_sizes(repo.path());
    let big: Vec<_> = sizes.iter().filter(|n| **n >= params.big_commit_files).collect();
    assert_eq!(big.len(), 1, "sizes: {sizes:?}");
    assert_eq!(sizes[0], params.big_commit_files, "the big commit is the first one");
    let max_rest = sizes[1..].iter().max().copied().unwrap();
    assert!(max_rest < 30, "regular commits stay small, max was {max_rest}");
}

#[test]
fn default_shape_generates_in_under_fifteen_seconds() {
    let started = Instant::now();
    let repo = SyntheticRepo::generate(&SyntheticParams::default());
    let elapsed = started.elapsed();
    assert!(elapsed.as_secs() < 15, "took {elapsed:?}");
    let by_ext: BTreeMap<_, usize> = tree_fingerprint(repo.path()).iter().fold(BTreeMap::new(), |mut m, (p, _)| {
        *m.entry(p.rsplit('.').next().unwrap().to_string()).or_default() += 1;
        m
    });
    assert_eq!(by_ext.get("md"), Some(&100));
    assert_eq!(by_ext.get("ts"), Some(&1200));
}

/// Manual helper for T-903 measurements: generates the shape selected by
/// `NEXSPEC_SYNTH_SCALE` and leaves it on disk, printing the path.
/// `cargo test --test synthetic_repo keep_synthetic_repo -- --ignored --nocapture`
#[test]
#[ignore]
fn keep_synthetic_repo() {
    let repo = SyntheticRepo::generate(&SyntheticParams::from_env());
    println!("SYNTHETIC_REPO={}", repo.path().display());
    std::mem::forget(repo); // leak the TempDir handle so the directory survives
}
