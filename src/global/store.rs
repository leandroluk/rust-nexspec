//! The global graph on disk (REQ-1302 in `.specs/features/multi-repo-graph/spec.md`, decisions D6, D7).
//!
//! `<home>/global/repos/<tag>.json` keeps the export of each repository as it was added;
//! `<home>/global/index/` is an ordinary nexspec index rebuilt from all of them. Rebuilding on every change
//! keeps `add` (same tag replaces), `remove` and the links between repositories free of bookkeeping.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::engine::{Engine, EngineError};
use crate::export::{ExportFilter, ExportGraph};
use crate::global::merge::{merge, to_mutations, valid_tag};

#[derive(Debug, thiserror::Error)]
pub enum GlobalError {
    #[error("invalid tag `{0}`: use letters, digits, `.`, `_` and `-`")]
    Tag(String),
    #[error("cannot find a home directory: set NEXSPEC_HOME or HOME")]
    NoHome,
    #[error("{0} has no index yet: run `nexspec sync` there first")]
    NoIndex(String),
    #[error("{0}")]
    Export(String),
    #[error("{0}")]
    Engine(#[from] EngineError),
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> GlobalError + '_ {
    move |source| GlobalError::Io { path: path.display().to_string(), source }
}

/// `$NEXSPEC_HOME`, else `~/.nexspec`.
pub fn home() -> Result<PathBuf, GlobalError> {
    if let Some(dir) = std::env::var_os("NEXSPEC_HOME").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(|h| PathBuf::from(h).join(".nexspec")).ok_or(GlobalError::NoHome)
}

pub fn global_dir(home: &Path) -> PathBuf {
    home.join("global")
}

pub fn index_dir(home: &Path) -> PathBuf {
    global_dir(home).join("index")
}

fn repos_dir(home: &Path) -> PathBuf {
    global_dir(home).join("repos")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoEntry {
    pub tag: String,
    /// Where it came from: a repository directory or an export file, as given.
    pub source: String,
    pub nodes: usize,
    pub edges: usize,
}

fn meta_path(home: &Path, tag: &str) -> PathBuf {
    repos_dir(home).join(format!("{tag}.meta.json"))
}

fn export_path(home: &Path, tag: &str) -> PathBuf {
    repos_dir(home).join(format!("{tag}.json"))
}

/// Every repository in the global graph, by tag.
pub fn list(home: &Path) -> Vec<RepoEntry> {
    let mut entries: Vec<RepoEntry> = std::fs::read_dir(repos_dir(home))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".meta.json"))
        .filter_map(|e| serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok())
        .collect();
    entries.sort_by(|a, b| a.tag.cmp(&b.tag));
    entries
}

/// Exports the local index of the repository at `dir` (it must have been synced).
pub fn export_repository(dir: &Path) -> Result<ExportGraph, GlobalError> {
    let index = dir.join(".specs").join(".index");
    if !index.join("metadata.redb").is_file() {
        return Err(GlobalError::NoIndex(dir.display().to_string()));
    }
    let engine = Engine::open(&index, dir)?;
    engine.export_graph(&ExportFilter::default()).map_err(|e| GlobalError::Export(e.to_string()))
}

/// A sensible default tag: the directory or file name, without extension, made tag-safe.
pub fn default_tag(source: &Path) -> String {
    let canonical = source.canonicalize().unwrap_or_else(|_| source.to_path_buf());
    let stem = if canonical.is_dir() { canonical.file_name() } else { canonical.file_stem() };
    stem.map(|s| s.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' }).collect()).unwrap_or_default()
}

/// Stores the export under `tag` (replacing an earlier one) and rebuilds the global index.
pub fn add(home: &Path, tag: &str, graph: &ExportGraph, source: &str) -> Result<RepoEntry, GlobalError> {
    if !valid_tag(tag) {
        return Err(GlobalError::Tag(tag.to_string()));
    }
    let dir = repos_dir(home);
    std::fs::create_dir_all(&dir).map_err(io(&dir))?;
    let path = export_path(home, tag);
    std::fs::write(&path, graph.to_json()).map_err(io(&path))?;
    let entry = RepoEntry { tag: tag.to_string(), source: source.to_string(), nodes: graph.nodes.len(), edges: graph.edges.len() };
    let meta = meta_path(home, tag);
    std::fs::write(&meta, serde_json::to_string_pretty(&entry).expect("serialises")).map_err(io(&meta))?;
    rebuild(home)?;
    Ok(entry)
}

/// Drops `tag` and rebuilds; `false` when it was not there.
pub fn remove(home: &Path, tag: &str) -> Result<bool, GlobalError> {
    let existed = export_path(home, tag).is_file();
    for path in [export_path(home, tag), meta_path(home, tag)] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io(&path)(e)),
        }
    }
    if existed {
        rebuild(home)?;
    }
    Ok(existed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rebuilt {
    pub repos: usize,
    pub nodes: usize,
    pub edges: usize,
}

/// The merged graph of every stored repository.
pub fn merged(home: &Path) -> Result<ExportGraph, GlobalError> {
    let mut parts = Vec::new();
    for entry in list(home) {
        let path = export_path(home, &entry.tag);
        let text = std::fs::read_to_string(&path).map_err(io(&path))?;
        let graph = ExportGraph::from_json(&text).map_err(|e| GlobalError::Export(format!("{}: {e}", path.display())))?;
        parts.push((entry.tag, graph));
    }
    Ok(merge(parts))
}

/// Throws the global index away and builds it again from the stored exports.
pub fn rebuild(home: &Path) -> Result<Rebuilt, GlobalError> {
    let graph = merged(home)?;
    let index = index_dir(home);
    // Only ever our own directory: `<home>/global/index`.
    if index.exists() {
        std::fs::remove_dir_all(&index).map_err(io(&index))?;
    }
    let engine = Engine::open(&index, &global_dir(home))?;
    let set = to_mutations(&graph);
    engine.apply_mutations(set)?;
    Ok(Rebuilt { repos: list(home).len(), nodes: graph.nodes.len(), edges: graph.edges.len() })
}
