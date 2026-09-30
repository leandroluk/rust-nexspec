//! The operations behind `annotate`: add, list, show, remove, lint (REQ-1801, REQ-1805, REQ-1807 in
//! `.specs/features/semantic-annotations/spec.md`). The CLI and the MCP tool `annotate_node` both call these.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::annotate::materialize::{State, evaluate};
use crate::annotate::store::{self, Annotation, Draft, MAX_NOTE_CHARS, key_of, target_hash};
use crate::engine::{Engine, EngineError};
use crate::enrich::select::find_secret;
use crate::memory::note::now;
use crate::query::api::{QueryError, resolve};

#[derive(Debug, thiserror::Error)]
pub enum AnnotateApiError {
    #[error("{0}")]
    Annotate(#[from] store::AnnotateError),
    #[error("{0}")]
    Query(#[from] QueryError),
    #[error("{0}")]
    Engine(#[from] EngineError),
    #[error("no community matches `{0}` (use `community:<number>` from `nexspec report`, or its label)")]
    NoCommunity(String),
    #[error("`{0}` is a {1}: columns, constraints, doc sections and annotations cannot be annotated; annotate the table, file or symbol instead")]
    NotAnnotatable(String, &'static str),
    #[error("no annotation with an id starting `{0}`")]
    NoSuchAnnotation(String),
    #[error("`{0}` matches several annotations ({1}); give more characters")]
    Ambiguous(String, String),
}

#[derive(Debug, Clone, Default)]
pub struct Request {
    pub target: String,
    pub label: Option<String>,
    pub note: Option<String>,
    pub relation: Option<String>,
    pub to: Option<String>,
    pub outcome: Option<String>,
    /// `user` or `agent`.
    pub author: String,
    pub model: Option<String>,
    pub pick: Option<usize>,
}

/// The stable key of whatever `spec` names, and the hash that will tell if it changed.
fn key_and_hash(engine: &Engine, repo: &Path, spec: &str, pick: Option<usize>) -> Result<(String, Option<String>), AnnotateApiError> {
    if spec.starts_with("community:") {
        let key = engine.community_key(spec)?.ok_or_else(|| AnnotateApiError::NoCommunity(spec.to_string()))?;
        return Ok((key, None));
    }
    let view = engine.query_view()?;
    let id = resolve(&view, spec, pick)?;
    let key = key_of(&view.snapshot, &id).ok_or_else(|| AnnotateApiError::NotAnnotatable(spec.to_string(), view.snapshot.kind_name(&id)))?;
    Ok((key, target_hash(repo, &view.snapshot, &id)))
}

/// Validates, resolves the target (and `--to`), saves, and brings the graph up to date.
pub fn annotate(engine: &Engine, repo: &Path, request: &Request) -> Result<(Annotation, PathBuf), AnnotateApiError> {
    let (target, source_hash) = key_and_hash(engine, repo, &request.target, request.pick)?;
    let to = match &request.to {
        Some(spec) => Some(key_and_hash(engine, repo, spec, None)?.0),
        None => None,
    };
    let annotation = Annotation::new(
        Draft {
            target,
            label: request.label.clone(),
            note: request.note.clone(),
            relation: request.relation.clone(),
            to,
            outcome: request.outcome.clone(),
            author: request.author.clone(),
            model: request.model.clone(),
            source_hash,
        },
        now(),
    )?;
    let path = store::add(repo, annotation.clone())?;
    engine.materialize_annotations()?;
    Ok((annotation, path))
}

fn state_name(state: State) -> &'static str {
    state.as_str()
}

/// One line per annotation: `id state author date target — summary`.
pub fn list(engine: &Engine, repo: &Path, target: Option<&str>, state: Option<&str>, max_tokens: Option<u32>) -> Result<String, AnnotateApiError> {
    let (annotations, _) = store::load(repo);
    let evaluated = evaluate(repo, &engine.snapshot()?, &annotations);
    let mut lines = vec![format!("# Annotations ({})", evaluated.len())];
    for e in &evaluated {
        if target.is_some_and(|t| !e.annotation.target.contains(t)) || state.is_some_and(|s| s != state_name(e.state)) {
            continue;
        }
        let a = &e.annotation;
        lines.push(format!("{} {} {} {} `{}` — {}", a.id, state_name(e.state), a.author, a.at.split('T').next().unwrap_or(&a.at), a.target, a.summary()));
    }
    if lines.len() == 1 {
        lines.push("(none)".to_string());
    }
    let mut text = crate::query::budget::fit_lines(lines, max_tokens).join("\n");
    text.push('\n');
    Ok(text)
}

fn find(repo: &Path, prefix: &str) -> Result<Annotation, AnnotateApiError> {
    let (all, _) = store::load(repo);
    let matching: Vec<&Annotation> = all.iter().filter(|a| !prefix.is_empty() && a.id.starts_with(prefix)).collect();
    match matching.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(AnnotateApiError::NoSuchAnnotation(prefix.to_string())),
        many => Err(AnnotateApiError::Ambiguous(prefix.to_string(), many.iter().map(|a| a.id.clone()).collect::<Vec<_>>().join(", "))),
    }
}

/// Everything about one annotation, including its state today.
pub fn show(engine: &Engine, repo: &Path, prefix: &str) -> Result<String, AnnotateApiError> {
    let annotation = find(repo, prefix)?;
    let state = evaluate(repo, &engine.snapshot()?, std::slice::from_ref(&annotation))[0].state;
    let mut out = format!("# Annotation {}\n- target: `{}`\n- state: {}\n- author: {}{}\n- at: {}\n- confidence: {}\n", annotation.id, annotation.target, state_name(state), annotation.author, annotation.model.as_ref().map(|m| format!(" ({m})")).unwrap_or_default(), annotation.at, annotation.confidence);
    for (name, value) in [("label", &annotation.label), ("note", &annotation.note), ("outcome", &annotation.outcome)] {
        if let Some(value) = value {
            out.push_str(&format!("- {name}: {value}\n"));
        }
    }
    if let (Some(relation), Some(to)) = (&annotation.relation, &annotation.to) {
        out.push_str(&format!("- relation: {relation} `{to}`\n"));
    }
    Ok(out)
}

/// Removes by id prefix and brings the graph up to date.
pub fn remove(engine: &Engine, repo: &Path, prefix: &str) -> Result<Annotation, AnnotateApiError> {
    let annotation = find(repo, prefix)?;
    store::remove(repo, &annotation.id)?.map_err(|_| AnnotateApiError::NoSuchAnnotation(prefix.to_string()))?;
    engine.materialize_annotations()?;
    Ok(annotation)
}

/// What needs attention: stale and dangling annotations, duplicates, notes over the limit, secrets, relations
/// pointing nowhere, and lines that are not annotations at all.
pub fn lint(engine: &Engine, repo: &Path) -> Result<Vec<String>, AnnotateApiError> {
    let (annotations, unreadable) = store::load(repo);
    let evaluated = evaluate(repo, &engine.snapshot()?, &annotations);
    let mut findings = Vec::new();
    if unreadable > 0 {
        findings.push(format!("{unreadable} line(s) of annotations.jsonl are not annotations"));
    }
    let mut same: BTreeMap<(String, String), Vec<&str>> = BTreeMap::new();
    for e in &evaluated {
        let a = &e.annotation;
        match e.state {
            State::Stale => findings.push(format!("stale: {} on `{}` — the target changed since ({})", a.id, a.target, a.summary())),
            State::Dangling => findings.push(format!("dangling: {} on `{}` — the target no longer exists ({})", a.id, a.target, a.summary())),
            State::Fresh => {}
        }
        if a.note.as_deref().is_some_and(|n| n.chars().count() > MAX_NOTE_CHARS) {
            findings.push(format!("too long: {} on `{}` — the note is over {MAX_NOTE_CHARS} characters", a.id, a.target));
        }
        if let Some(kind) = [a.label.as_deref(), a.note.as_deref()].into_iter().flatten().find_map(find_secret) {
            findings.push(format!("secret: {} on `{}` — looks like a {kind}; remove it", a.id, a.target));
        }
        if e.state != State::Dangling && a.relation.is_some() && e.to.is_none() && !a.to.as_deref().is_some_and(|t| t.starts_with("community:")) {
            findings.push(format!("relation target missing: {} on `{}` — `{}` no longer exists", a.id, a.target, a.to.as_deref().unwrap_or_default()));
        }
        if let Some(note) = &a.note {
            same.entry((a.target.clone(), note.to_lowercase())).or_default().push(a.id.as_str());
        }
    }
    for ((target, _), ids) in same {
        if ids.len() > 1 {
            findings.push(format!("duplicate: {} say the same thing about `{target}`", ids.join(", ")));
        }
    }
    Ok(findings)
}
