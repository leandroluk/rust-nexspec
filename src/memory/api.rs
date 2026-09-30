//! The two operations of the work memory over a repository: `save-result` and `reflect`
//! (REQ-1501, REQ-1502, REQ-1504 in `.specs/features/work-memory/spec.md`). The CLI and the MCP tools both call these.

use std::path::{Path, PathBuf};

use crate::engine::Engine;
use crate::memory::note::{self, NodeRef, Note, NoteError, Outcome, notes_dir};
use crate::memory::overlay::Overlay;
use crate::memory::reflect::{Reflection, ReflectOptions, reflect, summary_lines, to_markdown};
use crate::query::api::{QueryError, resolve};
use crate::search::{hex, unhex};

pub const LESSONS_FILE: &str = "LESSONS.md";

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("{0}")]
    Note(#[from] NoteError),
    #[error("{0}")]
    Query(#[from] QueryError),
    #[error("{0}")]
    Engine(#[from] crate::engine::EngineError),
    #[error("unknown outcome `{0}` (useful, dead_end or corrected)")]
    Outcome(String),
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, Default)]
pub struct SaveArgs {
    pub question: String,
    pub answer: String,
    /// `query`, `path`, `explain`, `affected`, `search`…
    pub kind: String,
    /// Node targets, as accepted by `query`/`explain`: names, `path:Name`, markers, ids.
    pub nodes: Vec<String>,
    pub outcome: String,
    pub correction: Option<String>,
}

/// Validates, resolves the cited nodes and writes the note; returns it and where it went.
pub fn save_result(engine: &Engine, repo: &Path, args: &SaveArgs) -> Result<(Note, PathBuf), MemoryError> {
    let outcome = Outcome::parse(&args.outcome).ok_or_else(|| MemoryError::Outcome(args.outcome.clone()))?;
    let view = engine.query_view()?;
    let mut nodes = Vec::new();
    for target in &args.nodes {
        let id = resolve(&view, target, None)?;
        nodes.push(NodeRef { id: hex(&id), label: view.snapshot.label(&id) });
    }
    let note = Note::new(&args.question, &args.answer, &args.kind, outcome, nodes, args.correction.as_deref(), note::now())?;
    let path = note::save(repo, &note)?;
    Ok((note, path))
}

/// The lessons the saved notes add up to, against the graph as it is now.
pub fn reflect_repo(engine: &Engine, repo: &Path, options: &ReflectOptions) -> Result<(Reflection, usize), MemoryError> {
    let (notes, skipped) = note::load_all(repo);
    let snapshot = engine.snapshot()?;
    let label_of = |id: &str| {
        let id = unhex(id)?;
        snapshot.nodes.contains_key(&id).then(|| snapshot.label(&id))
    };
    Ok((reflect(&notes, &label_of, options), skipped))
}

/// Writes `LESSONS.md` (shared) and the ranking cache (local); returns the path of the former.
pub fn write_lessons(repo: &Path, reflection: &Reflection) -> Result<PathBuf, MemoryError> {
    let dir = repo.join(note::MEMORY_DIR);
    let path = dir.join(LESSONS_FILE);
    let io = |p: &Path| {
        let shown = p.display().to_string();
        move |source| MemoryError::Io { path: shown, source }
    };
    std::fs::create_dir_all(&dir).map_err(io(&dir))?;
    std::fs::write(&path, to_markdown(reflection)).map_err(io(&path))?;
    let overlay = Overlay::from_reflection(reflection);
    if overlay.is_empty() {
        // Nothing to nudge: an old cache must not keep nudging.
        let _ = std::fs::remove_file(crate::memory::overlay::cache_path(repo));
    } else {
        overlay.save(repo).map_err(io(&crate::memory::overlay::cache_path(repo)))?;
    }
    Ok(path)
}

/// The short text a skill loads at the start of a session, cut to `max_tokens`.
pub fn session_summary(reflection: &Reflection, max_tokens: Option<u32>) -> String {
    let lines = crate::query::budget::fit_lines(summary_lines(reflection), max_tokens);
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// One line for people: what `reflect` found.
pub fn headline(reflection: &Reflection, notes_skipped: usize) -> String {
    use crate::memory::reflect::Class;
    let count = |wanted: fn(Class) -> bool| reflection.lessons.iter().filter(|l| wanted(l.class)).count();
    format!(
        "reflect: {} lesson(s) from {} note(s): {} preferred, {} tentative, {} contested, {} dead end(s); {} correction(s); {} note(s) faded, {} node(s) gone{}",
        reflection.lessons.len(),
        reflection.notes_live,
        count(|c| c == Class::Preferred),
        count(|c| c == Class::Tentative),
        count(|c| matches!(c, Class::Contested { .. })),
        count(|c| c == Class::DeadEnd),
        reflection.corrections.len(),
        reflection.notes_faded,
        reflection.nodes_dropped,
        if notes_skipped > 0 { format!("; {notes_skipped} file(s) in notes/ are not notes") } else { String::new() }
    )
}

pub fn notes_path(repo: &Path) -> PathBuf {
    notes_dir(repo)
}
