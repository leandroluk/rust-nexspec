//! [`DirtyCache`] — detects working-tree changes since the last time *this
//! process* looked (REQ-204 in `.specs/features/git-integration/spec.md`).
//! Deliberately not persisted: it answers "what changed since I last
//! looked", which is inherently process-lifetime scoped — see
//! `.specs/features/git-integration/design.md` → Decision Log.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct DirtyCache {
    last_seen: HashMap<PathBuf, [u8; 32]>,
}

impl DirtyCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Re-hash every path in `tracked_paths` (relative to `repo_root`) and
    /// return the ones whose content changed since the last call — or, on
    /// the very first call for a given path, the path itself (nothing to
    /// compare against yet, so it counts as "changed" to avoid silently
    /// losing initial state). A path that can't be read (deleted, no
    /// permission) is skipped — this cache only tracks content changes, not
    /// existence.
    pub fn scan(&mut self, repo_root: &Path, tracked_paths: &[PathBuf]) -> Vec<PathBuf> {
        let mut changed = Vec::new();
        for rel_path in tracked_paths {
            let Ok(content) = std::fs::read(repo_root.join(rel_path)) else {
                continue;
            };
            let hash = *blake3::hash(&content).as_bytes();
            let is_changed = self.last_seen.get(rel_path) != Some(&hash);
            if is_changed {
                changed.push(rel_path.clone());
            }
            self.last_seen.insert(rel_path.clone(), hash);
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(dir: &Path, rel: &str, content: &str) {
        let full = dir.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(full, content).unwrap();
    }

    #[test]
    fn first_scan_reports_existing_files_as_changed() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "a.txt", "hello");
        write(dir.path(), "b.txt", "world");

        let mut cache = DirtyCache::new();
        let mut changed = cache.scan(
            dir.path(),
            &[PathBuf::from("a.txt"), PathBuf::from("b.txt")],
        );
        changed.sort();

        assert_eq!(changed, vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")]);
    }

    #[test]
    fn second_scan_with_no_changes_reports_empty() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "a.txt", "hello");
        let paths = vec![PathBuf::from("a.txt")];

        let mut cache = DirtyCache::new();
        cache.scan(dir.path(), &paths);
        let second = cache.scan(dir.path(), &paths);

        assert!(second.is_empty());
    }

    #[test]
    fn editing_a_file_between_scans_makes_it_reappear() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "a.txt", "hello");
        let paths = vec![PathBuf::from("a.txt")];

        let mut cache = DirtyCache::new();
        cache.scan(dir.path(), &paths);
        assert!(cache.scan(dir.path(), &paths).is_empty());

        write(dir.path(), "a.txt", "hello, edited");
        assert_eq!(cache.scan(dir.path(), &paths), vec![PathBuf::from("a.txt")]);
    }
}
