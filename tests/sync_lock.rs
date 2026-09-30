//! Cross-process behaviour of the index lock (T-909, REQ-907): a second
//! `nexspec` waits for the first instead of failing with "Database already
//! open", and gives a readable error when the wait times out.

mod fixtures;

use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use fixtures::FixtureRepo;
use nexspec::Engine;

fn repo_with_spec() -> FixtureRepo {
    let repo = FixtureRepo::init();
    repo.write_file(".specs/spec.md", "## Requirements\n- REQ-560: lock test requirement\n");
    repo.write_file("src/a.ts", "export function a() {}\n");
    repo.commit("chore: seed");
    repo
}

fn spawn_sync(repo: &Path, timeout_s: &str) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_nexspec"))
        .arg("--repo")
        .arg(repo)
        .arg("sync")
        .env("NEXSPEC_LOCK_TIMEOUT_S", timeout_s)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn nexspec sync")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

fn describe(out: &Output) -> String {
    format!("status={:?} stdout={} stderr={}", out.status, text(&out.stdout), text(&out.stderr))
}

#[test]
fn sync_gives_a_readable_error_when_the_index_stays_busy() {
    let repo = repo_with_spec();
    let index_dir = repo.path().join(".specs").join(".index");
    let _holder = Engine::open(&index_dir, repo.path()).expect("hold the index");

    let out = spawn_sync(repo.path(), "1").wait_with_output().unwrap();
    assert!(!out.status.success(), "must fail while the index is held: {}", describe(&out));
    let stderr = text(&out.stderr);
    assert!(stderr.contains("in use by another nexspec process"), "{}", describe(&out));
    assert!(stderr.contains("NEXSPEC_LOCK_TIMEOUT_S"), "{}", describe(&out));
    assert!(!stderr.contains("already open"), "the opaque redb error must not leak: {}", describe(&out));
}

#[test]
fn sync_waits_for_the_holder_and_then_succeeds() {
    let repo = repo_with_spec();
    let index_dir = repo.path().join(".specs").join(".index");
    let holder = Engine::open(&index_dir, repo.path()).expect("hold the index");

    let started = Instant::now();
    let child = spawn_sync(repo.path(), "30");
    std::thread::sleep(Duration::from_millis(700));
    drop(holder);

    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    assert!(text(&out.stdout).contains("sync:"), "{}", describe(&out));
    assert!(started.elapsed() >= Duration::from_millis(700), "must have waited for the holder");
}

#[test]
fn two_simultaneous_syncs_both_finish() {
    let repo = repo_with_spec();
    let first = spawn_sync(repo.path(), "30");
    let second = spawn_sync(repo.path(), "30");
    let (a, b) = (first.wait_with_output().unwrap(), second.wait_with_output().unwrap());
    assert!(a.status.success(), "{}", describe(&a));
    assert!(b.status.success(), "{}", describe(&b));
    for out in [&a, &b] {
        assert!(!text(&out.stderr).contains("already open"), "{}", describe(out));
    }
}
