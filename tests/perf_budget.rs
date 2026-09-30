//! Time budgets over the synthetic reference repository (T-907, REQ-902 in
//! `.specs/features/performance-guard/spec.md`). Ignored by default: timings
//! only mean something in release builds.
//!
//! ```text
//! cargo test --release --test perf_budget -- --ignored --nocapture
//! ```
//!
//! Knobs (all optional): `NEXSPEC_SYNTH_SCALE` (repo size, default 1.0) and
//! per-phase limits in seconds -- `NEXSPEC_BUDGET_COLD_S` (30),
//! `NEXSPEC_BUDGET_NOOP_S` (2), `NEXSPEC_BUDGET_ONE_FILE_S` (3),
//! `NEXSPEC_BUDGET_QUERY_S` (1).

mod fixtures;

use std::time::{Duration, Instant};

use fixtures::synthetic::{SyntheticParams, SyntheticRepo};
use nexspec::Engine;
use nexspec::engine::id_hex;

fn limit(var: &str, default_secs: f64) -> Duration {
    let secs = std::env::var(var)
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(default_secs);
    Duration::from_secs_f64(secs)
}

fn check(phase: &str, elapsed: Duration, var: &str, default_secs: f64) {
    let max = limit(var, default_secs);
    println!("perf_budget {phase}: {elapsed:.3?} (limit {max:.1?})");
    assert!(
        elapsed <= max,
        "budget exceeded in phase `{phase}`: took {elapsed:.3?}, limit {max:.1?} (override with {var})"
    );
}

fn best_of<T>(runs: usize, mut f: impl FnMut() -> T) -> (Duration, T) {
    let mut best: Option<(Duration, T)> = None;
    for _ in 0..runs {
        let started = Instant::now();
        let value = f();
        let elapsed = started.elapsed();
        if best.as_ref().is_none_or(|(d, _)| elapsed < *d) {
            best = Some((elapsed, value));
        }
    }
    best.expect("at least one run")
}

fn marker_id(marker: &str) -> nexspec::sync::mutation::StableId {
    *blake3::hash(format!("marker:{marker}").as_bytes()).as_bytes()
}

#[test]
#[ignore = "timing budget: run with --release -- --ignored"]
fn sync_and_query_stay_within_budget() {
    let params = SyntheticParams::from_env();
    let repo = SyntheticRepo::generate(&params);
    let index_dir = repo.path().join(".specs").join(".index");
    let wal_path = index_dir.join("sync.wal");

    // Cold: init + first sync.
    let started = Instant::now();
    let engine = Engine::open(&index_dir, repo.path()).expect("open engine");
    let report = engine.sync().expect("cold sync");
    check("cold init+sync", started.elapsed(), "NEXSPEC_BUDGET_COLD_S", 30.0);
    assert!(report.timings.nodes > 0);
    let wal_size = std::fs::metadata(&wal_path).expect("wal exists").len();
    assert!(wal_size < 10 * 1024 * 1024, "sync.wal must not accumulate finished cycles, is {wal_size} bytes");

    // No changes: fast, and no new WAL frame.
    let wal_before = std::fs::metadata(&wal_path).expect("wal exists").len();
    let (elapsed, noop) = best_of(3, || engine.sync().expect("no-op sync"));
    check("sync without changes", elapsed, "NEXSPEC_BUDGET_NOOP_S", 2.0);
    assert!(noop.target_version.is_none(), "no-op sync must not stage anything");
    assert_eq!(std::fs::metadata(&wal_path).unwrap().len(), wal_before, "no-op sync must not grow the WAL");

    // One file changed (committed).
    let first_ts = repo
        .repo
        .path()
        .join("src")
        .join("m00")
        .join("file0001.ts");
    let mut source = std::fs::read_to_string(&first_ts).expect("synthetic ts file exists");
    source.push_str("export const touched = 1;\n");
    repo.repo.write_file("src/m00/file0001.ts", &source);
    repo.repo.commit("chore: touch one file");
    let started = Instant::now();
    let changed = engine.sync().expect("sync after one change");
    check("sync after 1 changed file", started.elapsed(), "NEXSPEC_BUDGET_ONE_FILE_S", 3.0);
    assert_eq!(changed.files_modified, 1);

    // Queries.
    let (elapsed, result) = best_of(3, || engine.search("synthetic requirement number", None).expect("search"));
    check("search", elapsed, "NEXSPEC_BUDGET_QUERY_S", 1.0);
    assert!(!result.hits.is_empty(), "the synthetic specs must be searchable");

    let target = id_hex(&marker_id("REQ-0000"));
    let (elapsed, _) = best_of(3, || engine.trace(&target).expect("trace"));
    check("trace", elapsed, "NEXSPEC_BUDGET_QUERY_S", 1.0);
}
