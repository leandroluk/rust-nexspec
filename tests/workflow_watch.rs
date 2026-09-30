//! `nexspec watch` end to end (T-1602, REQ-1601): a change in the working tree
//! reaches the index without anyone calling `sync`, and only one watcher runs
//! per repository.

mod fixtures;

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use fixtures::FixtureRepo;

fn nexspec(repo: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nexspec"));
    command.arg("--repo").arg(repo);
    command
}

struct Watcher(Child);

impl Drop for Watcher {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_watch(repo: &Path) -> Watcher {
    let child = nexspec(repo)
        .args(["watch", "--debounce", "200"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn watch");
    Watcher(child)
}

fn finds(repo: &Path, identifier: &str, path: &str) -> bool {
    let out = nexspec(repo).args(["query", identifier]).output().unwrap();
    out.status.success() && String::from_utf8_lossy(&out.stdout).contains(path)
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn a_change_in_the_working_tree_reaches_the_index_and_a_second_watcher_is_refused() {
    let repo = FixtureRepo::init();
    repo.write_file("src/first.ts", "export function firstWatchedThing() { return 1; }\n");
    repo.commit("init");

    let watcher = start_watch(repo.path());
    // The initial sync indexes what is already there.
    wait_until("the initial sync", || finds(repo.path(), "firstWatchedThing", "src/first.ts"));

    repo.write_file("src/second.ts", "export function secondWatchedThing() { return 2; }\n");
    wait_until("the new file to be indexed", || finds(repo.path(), "secondWatchedThing", "src/second.ts"));

    let second = nexspec(repo.path()).args(["watch", "--no-initial-sync"]).output().unwrap();
    assert!(!second.status.success(), "a second watcher must not start");
    let message = String::from_utf8_lossy(&second.stderr);
    assert!(message.contains("already running"), "{message}");

    drop(watcher);
}
