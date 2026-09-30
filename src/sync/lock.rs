//! [`SyncLock`] — cross-process lock on an index directory (REQ-907 in
//! `.specs/features/performance-guard/spec.md`). `redb` allows one process per
//! database, so a second `nexspec` invocation used to die with the opaque
//! "Database already open". The lock is taken *before* the database is
//! opened and waits (bounded) for the holder to finish instead.
//!
//! It is an OS advisory lock on `sync.lock` inside the index directory, so it
//! is released by the kernel if the holder crashes; the file only carries the
//! holder's PID for the error message.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const LOCK_FILE_NAME: &str = "sync.lock";
pub const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("io error on lock file: {0}")]
    Io(#[from] std::io::Error),
    #[error(
        "the index at {index_dir} is in use by another nexspec process{holder}; gave up after {waited:?} \
         (raise NEXSPEC_LOCK_TIMEOUT_S, or stop the other process -- a long-running `nexspec mcp` holds the index)"
    )]
    Timeout {
        index_dir: PathBuf,
        waited: Duration,
        holder: String,
    },
}

/// Held for as long as the value lives; dropping it releases the lock.
#[derive(Debug)]
pub struct SyncLock {
    _file: File,
}

impl SyncLock {
    /// Waits up to `timeout` for exclusive ownership of `index_dir`.
    pub fn acquire(index_dir: &Path, timeout: Duration) -> Result<Self, LockError> {
        Self::acquire_named(index_dir, LOCK_FILE_NAME, timeout)
    }

    /// Same lock, on another file of the directory (`watch.lock`: one watcher
    /// per repository, independent of the index lock).
    pub fn acquire_named(dir: &Path, file_name: &str, timeout: Duration) -> Result<Self, LockError> {
        let index_dir = dir;
        let path = dir.join(file_name);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        let started = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => break,
                Err(TryLockError::WouldBlock) => {
                    let waited = started.elapsed();
                    if waited >= timeout {
                        return Err(LockError::Timeout {
                            index_dir: index_dir.to_path_buf(),
                            waited,
                            holder: read_holder(&path),
                        });
                    }
                    std::thread::sleep(POLL_INTERVAL.min(timeout - waited));
                }
                Err(TryLockError::Error(e)) => return Err(e.into()),
            }
        }
        file.set_len(0)?;
        file.rewind()?;
        write!(file, "{}", std::process::id())?;
        file.flush()?;
        Ok(Self { _file: file })
    }

    /// `NEXSPEC_LOCK_TIMEOUT_S` (seconds, may be fractional), else 30 s.
    pub fn timeout_from_env() -> Duration {
        std::env::var("NEXSPEC_LOCK_TIMEOUT_S")
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|s| s.is_finite() && *s >= 0.0)
            .map(Duration::from_secs_f64)
            .unwrap_or(DEFAULT_LOCK_TIMEOUT)
    }
}

fn read_holder(path: &Path) -> String {
    let mut text = String::new();
    // A fresh handle: the content is only advisory and may be empty or stale.
    match File::open(path).and_then(|mut f| f.read_to_string(&mut text)) {
        Ok(_) if text.trim().parse::<u32>().is_ok() => format!(" (PID {})", text.trim()),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn second_acquire_times_out_while_the_first_is_held() {
        let dir = TempDir::new().unwrap();
        let _held = SyncLock::acquire(dir.path(), Duration::from_secs(1)).unwrap();
        let started = Instant::now();
        let err = SyncLock::acquire(dir.path(), Duration::from_millis(200)).unwrap_err();
        assert!(matches!(err, LockError::Timeout { .. }), "got {err:?}");
        assert!(started.elapsed() >= Duration::from_millis(200), "must actually wait");
        let message = err.to_string();
        assert!(message.contains("in use by another nexspec process"), "{message}");
        assert!(message.contains("NEXSPEC_LOCK_TIMEOUT_S"), "{message}");
    }

    #[test]
    fn released_lock_can_be_taken_again() {
        let dir = TempDir::new().unwrap();
        drop(SyncLock::acquire(dir.path(), Duration::from_secs(1)).unwrap());
        SyncLock::acquire(dir.path(), Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn waiter_gets_the_lock_once_the_holder_finishes() {
        let dir = TempDir::new().unwrap();
        let held = SyncLock::acquire(dir.path(), Duration::from_secs(1)).unwrap();
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(held);
        });
        let started = Instant::now();
        SyncLock::acquire(dir.path(), Duration::from_secs(10)).expect("acquired after the holder released");
        assert!(started.elapsed() >= Duration::from_millis(250));
        releaser.join().unwrap();
    }

    #[test]
    fn timeout_env_parses_fractions_and_falls_back() {
        // Single test touching the environment: no other test reads this var.
        unsafe { std::env::set_var("NEXSPEC_LOCK_TIMEOUT_S", "1.5") };
        assert_eq!(SyncLock::timeout_from_env(), Duration::from_millis(1500));
        unsafe { std::env::set_var("NEXSPEC_LOCK_TIMEOUT_S", "nope") };
        assert_eq!(SyncLock::timeout_from_env(), DEFAULT_LOCK_TIMEOUT);
        unsafe { std::env::remove_var("NEXSPEC_LOCK_TIMEOUT_S") };
    }

    #[test]
    fn named_locks_are_independent_of_the_index_lock() {
        let dir = TempDir::new().unwrap();
        let _index = SyncLock::acquire(dir.path(), Duration::from_secs(1)).unwrap();
        let _watch = SyncLock::acquire_named(dir.path(), "watch.lock", Duration::from_secs(1)).expect("different file, different lock");
        let second = SyncLock::acquire_named(dir.path(), "watch.lock", Duration::from_millis(100));
        assert!(matches!(second, Err(LockError::Timeout { .. })), "the same named lock is exclusive");
    }
}
