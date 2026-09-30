//! The enrichment cache (REQ-1905 in `.specs/features/retrieval-enrichment/spec.md`).
//!
//! One JSON line per file x language in `.specs/.cache/enrichment.jsonl`,
//! ordered by (path, lang) so the file diffs cleanly. It lives outside
//! `.specs/.index/` on purpose: the index can be deleted and rebuilt without
//! paying for the summaries again.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::search::schema::SummarySource;

pub const CACHE_DIR: &str = ".specs/.cache";
pub const CACHE_FILE: &str = "enrichment.jsonl";

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub path: String,
    /// blake3 (hex) of the whole file when the summary was written.
    pub content_hash: String,
    pub lang: String,
    pub summary: String,
    pub model: String,
    pub prompt_version: u32,
    /// Unix seconds.
    pub at: u64,
    /// Always `INFERRED`: a summary is a model's reading, never an extracted fact.
    pub confidence: String,
}

impl Entry {
    pub fn new(path: &str, content_hash: &str, lang: &str, summary: &str, model: &str, prompt_version: u32, at: u64) -> Self {
        Self {
            path: path.to_string(),
            content_hash: content_hash.to_string(),
            lang: lang.to_string(),
            summary: summary.to_string(),
            model: model.to_string(),
            prompt_version,
            at,
            confidence: "INFERRED".to_string(),
        }
    }
}

pub fn content_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

pub fn cache_path(repo: &Path) -> PathBuf {
    repo.join(CACHE_DIR).join(CACHE_FILE)
}

#[derive(Debug, Default, Clone)]
pub struct EnrichmentCache {
    entries: BTreeMap<(String, String), Entry>,
    /// Lines that could not be read (kept out, reported by `--status`).
    pub skipped_lines: usize,
}

impl EnrichmentCache {
    pub fn load(repo: &Path) -> Result<Self, CacheError> {
        let path = cache_path(repo);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => return Err(CacheError::Io { path: path.display().to_string(), source }),
        };
        let mut cache = Self::default();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            match serde_json::from_str::<Entry>(line) {
                Ok(entry) => cache.upsert(entry),
                Err(_) => cache.skipped_lines += 1,
            }
        }
        Ok(cache)
    }

    /// Writes the whole cache through a temporary file and a rename, so an
    /// interruption leaves the previous complete file in place.
    pub fn save(&self, repo: &Path) -> Result<(), CacheError> {
        let path = cache_path(repo);
        let io = |source| CacheError::Io { path: path.display().to_string(), source };
        std::fs::create_dir_all(path.parent().expect("has a parent")).map_err(io)?;
        let mut text = String::new();
        for entry in self.entries.values() {
            text.push_str(&serde_json::to_string(entry).expect("entry serialises"));
            text.push('\n');
        }
        let tmp = path.with_extension("jsonl.tmp");
        std::fs::write(&tmp, text).map_err(io)?;
        std::fs::rename(&tmp, &path).map_err(io)
    }

    pub fn upsert(&mut self, entry: Entry) {
        self.entries.insert((entry.path.clone(), entry.lang.clone()), entry);
    }

    pub fn get(&self, path: &str, lang: &str) -> Option<&Entry> {
        self.entries.get(&(path.to_string(), lang.to_string()))
    }

    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn paths(&self) -> BTreeSet<&str> {
        self.entries.values().map(|e| e.path.as_str()).collect()
    }

    /// Drops the entries of files that no longer exist; returns how many.
    pub fn retain_paths(&mut self, existing: &BTreeSet<String>) -> usize {
        let before = self.entries.len();
        self.entries.retain(|(path, _), _| existing.contains(path));
        before - self.entries.len()
    }

    /// Whether `path` already has a summary in `lang` that is still valid for
    /// this content, model and prompt.
    pub fn is_fresh(&self, path: &str, lang: &str, hash: &str, model: &str, prompt_version: u32) -> bool {
        self.get(path, lang)
            .is_some_and(|e| e.content_hash == hash && e.model == model && e.prompt_version == prompt_version)
    }

    pub fn clear(repo: &Path) -> Result<bool, CacheError> {
        let path = cache_path(repo);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(CacheError::Io { path: path.display().to_string(), source }),
        }
    }
}

/// The part of the cache that is still true: summaries of files whose content has
/// not changed since (REQ-1906, a changed file is `stale` and leaves the ranking).
#[derive(Debug, Default, Clone)]
pub struct EnrichmentView {
    by_path: BTreeMap<String, BTreeMap<String, String>>,
}

/// Files with a cached summary whose content has changed since (or that no longer exist).
pub fn stale_files(repo: &Path, cache: &EnrichmentCache) -> usize {
    let mut stale = BTreeSet::new();
    for entry in cache.entries() {
        let current = std::fs::read(repo.join(&entry.path)).ok().map(|b| content_hash(&b));
        if current.as_deref() != Some(entry.content_hash.as_str()) {
            stale.insert(entry.path.as_str());
        }
    }
    stale.len()
}

impl EnrichmentView {
    /// Reads each enriched file once to compare its hash, so a stale summary never reaches the index.
    pub fn build(repo: &Path, cache: &EnrichmentCache) -> Self {
        let mut view = Self::default();
        let mut hashes: BTreeMap<&str, Option<String>> = BTreeMap::new();
        for entry in cache.entries() {
            let current = hashes
                .entry(entry.path.as_str())
                .or_insert_with(|| std::fs::read(repo.join(&entry.path)).ok().map(|b| content_hash(&b)));
            if current.as_deref() == Some(entry.content_hash.as_str()) {
                view.by_path.entry(entry.path.clone()).or_default().insert(entry.lang.clone(), entry.summary.clone());
            }
        }
        view
    }

    pub fn is_empty(&self) -> bool {
        self.by_path.is_empty()
    }

    pub fn file_count(&self) -> usize {
        self.by_path.len()
    }
}

impl SummarySource for EnrichmentView {
    fn summaries(&self, path: &str) -> BTreeMap<String, String> {
        self.by_path.get(path).cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn entry(path: &str, lang: &str, hash: &str) -> Entry {
        Entry::new(path, hash, lang, &format!("summary of {path} in {lang}"), "m", 1, 10)
    }

    #[test]
    fn save_then_load_round_trips_in_a_stable_order() {
        let dir = TempDir::new().unwrap();
        let mut cache = EnrichmentCache::default();
        cache.upsert(entry("b.ts", "ru", "h2"));
        cache.upsert(entry("a.ts", "en", "h1"));
        cache.upsert(entry("a.ts", "ru", "h1"));
        cache.save(dir.path()).unwrap();

        let text = std::fs::read_to_string(cache_path(dir.path())).unwrap();
        let order: Vec<_> = text.lines().map(|l| serde_json::from_str::<Entry>(l).unwrap()).map(|e| (e.path, e.lang)).collect();
        assert_eq!(order, [("a.ts".into(), "en".into()), ("a.ts".into(), "ru".into()), ("b.ts".into(), "ru".into())]);
        let loaded = EnrichmentCache::load(dir.path()).unwrap();
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded.get("a.ts", "ru"), cache.get("a.ts", "ru"));
        assert_eq!(loaded.get("a.ts", "en").unwrap().confidence, "INFERRED");
    }

    #[test]
    fn a_missing_file_is_an_empty_cache_and_a_damaged_line_is_skipped_not_fatal() {
        let dir = TempDir::new().unwrap();
        assert!(EnrichmentCache::load(dir.path()).unwrap().is_empty());
        let mut cache = EnrichmentCache::default();
        cache.upsert(entry("a.ts", "en", "h"));
        cache.save(dir.path()).unwrap();
        let path = cache_path(dir.path());
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("{not json\n");
        std::fs::write(&path, text).unwrap();
        let loaded = EnrichmentCache::load(dir.path()).unwrap();
        assert_eq!((loaded.len(), loaded.skipped_lines), (1, 1));
    }

    #[test]
    fn freshness_depends_on_content_model_and_prompt() {
        let mut cache = EnrichmentCache::default();
        cache.upsert(entry("a.ts", "en", "h1"));
        assert!(cache.is_fresh("a.ts", "en", "h1", "m", 1));
        assert!(!cache.is_fresh("a.ts", "en", "h2", "m", 1), "content changed");
        assert!(!cache.is_fresh("a.ts", "en", "h1", "other", 1), "another model");
        assert!(!cache.is_fresh("a.ts", "en", "h1", "m", 2), "another prompt");
        assert!(!cache.is_fresh("a.ts", "pt", "h1", "m", 1), "another language is still pending");
    }

    #[test]
    fn removed_files_are_dropped() {
        let mut cache = EnrichmentCache::default();
        cache.upsert(entry("a.ts", "en", "h"));
        cache.upsert(entry("gone.ts", "en", "h"));
        let existing: BTreeSet<String> = ["a.ts".to_string()].into();
        assert_eq!(cache.retain_paths(&existing), 1);
        assert!(cache.get("gone.ts", "en").is_none());
    }

    #[test]
    fn the_view_leaves_out_files_whose_content_changed() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.ts"), "one").unwrap();
        std::fs::write(dir.path().join("b.ts"), "two").unwrap();
        let mut cache = EnrichmentCache::default();
        cache.upsert(entry("a.ts", "en", &content_hash(b"one")));
        cache.upsert(entry("b.ts", "en", &content_hash(b"two")));
        cache.upsert(entry("missing.ts", "en", "x"));
        std::fs::write(dir.path().join("b.ts"), "two, edited").unwrap();

        let view = EnrichmentView::build(dir.path(), &cache);
        assert_eq!(view.summaries("a.ts").get("en").map(String::as_str), Some("summary of a.ts in en"));
        assert!(view.summaries("b.ts").is_empty(), "stale: leaves the ranking");
        assert!(view.summaries("missing.ts").is_empty());
        assert_eq!(view.file_count(), 1);
    }
}
