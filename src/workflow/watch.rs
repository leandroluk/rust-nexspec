//! Watching a working tree and syncing after a burst of changes (REQ-1601 in
//! `.specs/features/workflow-integration/spec.md`).
//!
//! The loop knows nothing about the index: it turns file-system events into
//! "something relevant changed", waits for a quiet `debounce`, and calls the
//! `sync` callback. `nexspec watch` passes a callback that opens the engine
//! just for the cycle; `nexspec mcp --watch` passes the server's own engine.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::{EventKind, RecursiveMode, Watcher};

#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    #[error("cannot watch {path}: {source}")]
    Notify {
        path: String,
        #[source]
        source: notify::Error,
    },
}

/// Directories nobody wants a sync for, whatever `.gitignore` says. The index
/// lives in one of them: syncing writes there, so watching it would loop.
const ALWAYS_IGNORED: &[&str] = &[".git", ".specs/.index", ".models", "target", "node_modules"];

/// Decides which paths are worth a sync.
pub struct PathFilter {
    root: PathBuf,
    gitignore: Gitignore,
}

impl PathFilter {
    pub fn new(root: &Path) -> Self {
        let mut builder = GitignoreBuilder::new(root);
        // A missing file is fine: a repository without a .gitignore is valid.
        let _ = builder.add(root.join(".gitignore"));
        let _ = builder.add(root.join(".git").join("info").join("exclude"));
        let gitignore = builder.build().unwrap_or_else(|_| Gitignore::empty());
        Self { root: root.to_path_buf(), gitignore }
    }

    /// Whether a change at `path` should trigger a sync.
    pub fn is_relevant(&self, path: &Path) -> bool {
        let Ok(relative) = path.strip_prefix(&self.root) else { return false };
        let normalized = relative.to_string_lossy().replace('\\', "/");
        if normalized.is_empty() {
            return false;
        }
        if ALWAYS_IGNORED.iter().any(|dir| normalized == *dir || normalized.starts_with(&format!("{dir}/"))) {
            return false;
        }
        // Nested `node_modules`/`target` directories too (monorepos).
        if normalized.split('/').any(|part| part == "node_modules" || part == ".git") {
            return false;
        }
        let is_dir = path.is_dir();
        !self.gitignore.matched_path_or_any_parents(relative, is_dir).is_ignore()
    }
}

#[derive(Debug, Clone)]
pub struct WatchOptions {
    /// Quiet time after the last relevant event before syncing.
    pub debounce: Duration,
    /// How often the loop looks at the stop flag.
    pub poll: Duration,
}

impl Default for WatchOptions {
    fn default() -> Self {
        Self { debounce: Duration::from_millis(500), poll: Duration::from_millis(100) }
    }
}

/// Runs until `stop` is set. `sync` is called once per burst of relevant
/// events; an error from it is reported through `on_error` and the loop keeps
/// going (a failed sync must not end the watcher). A cycle in progress always
/// finishes before the loop returns.
pub fn run(
    root: &Path,
    options: &WatchOptions,
    stop: &AtomicBool,
    mut sync: impl FnMut() -> Result<(), String>,
    mut on_error: impl FnMut(&str),
) -> Result<(), WatchError> {
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let _ = tx.send(event);
    })
    .map_err(|source| WatchError::Notify { path: root.display().to_string(), source })?;
    watcher
        .watch(root, RecursiveMode::Recursive)
        .map_err(|source| WatchError::Notify { path: root.display().to_string(), source })?;

    let filter = PathFilter::new(root);
    let mut last_relevant: Option<Instant> = None;
    while !stop.load(Ordering::SeqCst) {
        match rx.recv_timeout(options.poll) {
            Ok(Ok(event)) => {
                if matches!(event.kind, EventKind::Access(_)) {
                    continue;
                }
                if event.paths.iter().any(|p| filter.is_relevant(p)) {
                    last_relevant = Some(Instant::now());
                }
            }
            Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if let Some(at) = last_relevant
            && at.elapsed() >= options.debounce
        {
            last_relevant = None;
            if let Err(message) = sync() {
                on_error(&message);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use tempfile::TempDir;

    fn filter_for(files: &[(&str, &str)]) -> (TempDir, PathFilter) {
        let dir = TempDir::new().unwrap();
        for (path, content) in files {
            let full = dir.path().join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, content).unwrap();
        }
        let filter = PathFilter::new(dir.path());
        (dir, filter)
    }

    #[test]
    fn engine_artifacts_git_internals_and_gitignored_paths_are_not_relevant() {
        let (dir, filter) = filter_for(&[(".gitignore", "dist/\n*.log\n"), ("src/a.ts", "x")]);
        let r = |p: &str| filter.is_relevant(&dir.path().join(p));
        assert!(r("src/a.ts"));
        assert!(r("docs/new.md"));
        assert!(!r(".specs/.index/metadata.redb"));
        assert!(!r(".specs/.index/tantivy/meta.json"));
        assert!(!r(".git/index"));
        assert!(!r(".models/model.onnx"));
        assert!(!r("apps/web/node_modules/pkg/index.js"));
        assert!(!r("target/debug/x"));
        assert!(!r("dist/bundle.js"), "gitignored directory");
        assert!(!r("debug.log"), "gitignored pattern");
        assert!(r(".specs/project/STATE.md"), "specs are what we index");
        assert!(!filter.is_relevant(Path::new("/somewhere/else/a.ts")), "outside the root");
    }

    #[test]
    fn a_burst_of_changes_causes_one_sync_and_ignored_changes_cause_none() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "ignored/\n").unwrap();
        // Existing directories, as in a real repository (creating one is itself an event).
        std::fs::create_dir_all(dir.path().join("ignored")).unwrap();
        std::fs::create_dir_all(dir.path().join(".specs/.index")).unwrap();
        let root = dir.path().to_path_buf();
        let stop = Arc::new(AtomicBool::new(false));
        let syncs = Arc::new(AtomicUsize::new(0));

        let handle = {
            let (root, stop, syncs) = (root.clone(), Arc::clone(&stop), Arc::clone(&syncs));
            std::thread::spawn(move || {
                let options = WatchOptions { debounce: Duration::from_millis(250), poll: Duration::from_millis(25) };
                run(&root, &options, &stop, || { syncs.fetch_add(1, Ordering::SeqCst); Ok(()) }, |_| {}).unwrap();
            })
        };
        std::thread::sleep(Duration::from_millis(300)); // let the watcher attach

        for i in 0..5 {
            std::fs::write(root.join(format!("f{i}.txt")), "x").unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while syncs.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(syncs.load(Ordering::SeqCst), 1, "five writes in a burst are one sync");

        std::fs::write(root.join("ignored/x.txt"), "x").unwrap();
        std::fs::write(root.join(".specs/.index/metadata.redb"), "x").unwrap();
        std::thread::sleep(Duration::from_millis(800));
        assert_eq!(syncs.load(Ordering::SeqCst), 1, "gitignored paths and the index never trigger a sync");

        stop.store(true, Ordering::SeqCst);
        handle.join().expect("the loop ends once stopped");
    }

    #[test]
    fn a_failing_sync_is_reported_and_the_loop_keeps_running() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let stop = Arc::new(AtomicBool::new(false));
        let errors = Arc::new(AtomicUsize::new(0));
        let handle = {
            let (root, stop, errors) = (root.clone(), Arc::clone(&stop), Arc::clone(&errors));
            std::thread::spawn(move || {
                let options = WatchOptions { debounce: Duration::from_millis(150), poll: Duration::from_millis(25) };
                run(&root, &options, &stop, || Err("boom".to_string()), |m| {
                    assert_eq!(m, "boom");
                    errors.fetch_add(1, Ordering::SeqCst);
                })
                .unwrap();
            })
        };
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(root.join("a.txt"), "1").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while errors.load(Ordering::SeqCst) < 1 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(root.join("b.txt"), "2").unwrap();
        while errors.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(errors.load(Ordering::SeqCst) >= 2, "it kept watching after the first failure");
        stop.store(true, Ordering::SeqCst);
        handle.join().unwrap();
    }
}
