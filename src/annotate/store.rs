//! What agents and people concluded about nodes, kept in a file (REQ-1801, REQ-1802, REQ-1803, REQ-1807 in
//! `.specs/features/semantic-annotations/spec.md`; decisions D1, D2, D8).
//!
//! `.specs/.memory/annotations.jsonl` is the source of truth: one JSON object per line, in a stable order, meant
//! for Git. Targets are stored by a *stable key* (`path`, `path::Name`, `REQ-1`, `table:public.tb_x`…), never by
//! an internal id, so the index can be deleted and rebuilt without losing a word.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::enrich::select::find_secret;
use crate::graph::node::{NodePayload, endpoint_node_id, file_node_id, package_node_id, table_node_id};
use crate::memory::note::{MEMORY_DIR, format_iso};
use crate::query::filter::relation_types;
use crate::report::snapshot::GraphSnapshot;
use crate::sync::mutation::StableId;

pub const ANNOTATIONS_FILE: &str = "annotations.jsonl";
/// Longest note (REQ-1807).
pub const MAX_NOTE_CHARS: usize = 500;
const MAX_LABEL_CHARS: usize = 120;

#[derive(Debug, thiserror::Error)]
pub enum AnnotateError {
    #[error("an annotation needs a --label, a --note, a --relation with --to, or an --outcome")]
    Empty,
    #[error("the note is {0} characters; the limit is {MAX_NOTE_CHARS} (annotations are for short conclusions)")]
    NoteTooLong(usize),
    #[error("the label is {0} characters; the limit is {MAX_LABEL_CHARS}")]
    LabelTooLong(usize),
    #[error("the {0} looks like a {1}: annotations live in the repository, so secrets are never saved")]
    Secret(&'static str, &'static str),
    #[error("--relation {0} needs --to <target>, and --to needs --relation")]
    RelationIncomplete(String),
    #[error("unknown relation `{0}` (imports, calls, references, extends, implements, depends_on, satisfies…)")]
    UnknownRelation(String),
    #[error("unknown outcome `{0}` (useful, dead_end or corrected)")]
    Outcome(String),
    #[error("author must be `agent` or `user`, not `{0}`")]
    Author(String),
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Annotation {
    /// Ten hex characters of a hash of what was said and about what: saying the same thing again is the same annotation.
    pub id: String,
    /// Stable key of the annotated target.
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// A relation to `to`: an `INFERRED` edge with context `annotation` (never overrides an extracted one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// `useful`, `dead_end` or `corrected`: feeds the ranking like the work memory does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// `agent` or `user`.
    pub author: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// ISO 8601 UTC.
    pub at: String,
    /// Hash of what the target was when the annotation was made; a different hash later makes it `stale`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hash: Option<String>,
    /// Always `INFERRED`: an annotation is a reading, not an extracted fact.
    pub confidence: String,
}

/// What an annotation says, before it gets an id and a time.
#[derive(Debug, Clone, Default)]
pub struct Draft {
    pub target: String,
    pub label: Option<String>,
    pub note: Option<String>,
    pub relation: Option<String>,
    pub to: Option<String>,
    pub outcome: Option<String>,
    pub author: String,
    pub model: Option<String>,
    pub source_hash: Option<String>,
}

fn clean(text: Option<String>) -> Option<String> {
    text.map(|t| t.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|t| !t.is_empty())
}

impl Annotation {
    pub fn new(draft: Draft, at: u64) -> Result<Self, AnnotateError> {
        let (label, note, relation, to, outcome) = (clean(draft.label), clean(draft.note), clean(draft.relation), clean(draft.to), clean(draft.outcome));
        if label.is_none() && note.is_none() && relation.is_none() && outcome.is_none() {
            return Err(AnnotateError::Empty);
        }
        if let Some(n) = &note
            && n.chars().count() > MAX_NOTE_CHARS
        {
            return Err(AnnotateError::NoteTooLong(n.chars().count()));
        }
        if let Some(l) = &label
            && l.chars().count() > MAX_LABEL_CHARS
        {
            return Err(AnnotateError::LabelTooLong(l.chars().count()));
        }
        for (what, text) in [("label", &label), ("note", &note)] {
            if let Some(kind) = text.as_deref().and_then(find_secret) {
                return Err(AnnotateError::Secret(what, kind));
            }
        }
        if relation.is_some() != to.is_some() {
            return Err(AnnotateError::RelationIncomplete(relation.unwrap_or_default()));
        }
        if let Some(r) = &relation
            && relation_types(r).is_none()
        {
            return Err(AnnotateError::UnknownRelation(r.clone()));
        }
        if let Some(o) = &outcome
            && !matches!(o.as_str(), "useful" | "dead_end" | "corrected")
        {
            return Err(AnnotateError::Outcome(o.clone()));
        }
        if !matches!(draft.author.as_str(), "agent" | "user") {
            return Err(AnnotateError::Author(draft.author));
        }
        let id = annotation_id(&draft.target, &label, &note, &relation, &to, &outcome);
        Ok(Self {
            id,
            target: draft.target,
            label,
            note,
            relation,
            to,
            outcome,
            author: draft.author,
            model: clean(draft.model),
            at: format_iso(at),
            source_hash: draft.source_hash,
            confidence: "INFERRED".to_string(),
        })
    }

    /// Short text for lists: the label, else the start of the note, else the relation or outcome.
    pub fn summary(&self) -> String {
        if let Some(label) = &self.label {
            return label.clone();
        }
        if let Some(note) = &self.note {
            return note.chars().take(60).collect();
        }
        match (&self.relation, &self.to, &self.outcome) {
            (Some(r), Some(to), _) => format!("{r} {to}"),
            (_, _, Some(o)) => o.clone(),
            _ => String::new(),
        }
    }
}

fn annotation_id(target: &str, label: &Option<String>, note: &Option<String>, relation: &Option<String>, to: &Option<String>, outcome: &Option<String>) -> String {
    let part = |o: &Option<String>| o.clone().unwrap_or_default();
    let material = [target.to_string(), part(label), part(note), part(relation), part(to), part(outcome)].join("\n");
    blake3::hash(material.as_bytes()).to_hex()[..10].to_string()
}

pub fn annotations_path(repo: &Path) -> PathBuf {
    repo.join(MEMORY_DIR).join(ANNOTATIONS_FILE)
}

/// Every readable annotation, in file order, and how many lines could not be read.
pub fn load(repo: &Path) -> (Vec<Annotation>, usize) {
    let Ok(text) = std::fs::read_to_string(annotations_path(repo)) else { return (Vec::new(), 0) };
    let mut annotations = Vec::new();
    let mut skipped = 0;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        match serde_json::from_str::<Annotation>(line) {
            Ok(a) => annotations.push(a),
            Err(_) => skipped += 1,
        }
    }
    (annotations, skipped)
}

/// Writes all annotations in the stable order (target, time, id), through a temporary file.
pub fn save_all(repo: &Path, annotations: &mut [Annotation]) -> Result<PathBuf, AnnotateError> {
    annotations.sort_by(|a, b| (&a.target, &a.at, &a.id).cmp(&(&b.target, &b.at, &b.id)));
    let path = annotations_path(repo);
    let io = |source| AnnotateError::Io { path: path.display().to_string(), source };
    std::fs::create_dir_all(path.parent().expect("has a parent")).map_err(io)?;
    let mut text = String::new();
    for annotation in annotations.iter() {
        text.push_str(&serde_json::to_string(annotation).expect("annotation serialises"));
        text.push('\n');
    }
    let tmp = path.with_extension("jsonl.tmp");
    std::fs::write(&tmp, text).map_err(io)?;
    std::fs::rename(&tmp, &path).map_err(io)?;
    Ok(path)
}

/// Adds the annotation, replacing an earlier one with the same id (that renews its time).
pub fn add(repo: &Path, annotation: Annotation) -> Result<PathBuf, AnnotateError> {
    let (mut all, _) = load(repo);
    all.retain(|a| a.id != annotation.id);
    all.push(annotation);
    save_all(repo, &mut all)
}

/// Removes the annotation whose id starts with `prefix`; errors are `None` (no match) and `Some(Err(ids))` (several).
pub fn remove(repo: &Path, prefix: &str) -> Result<Result<Annotation, Vec<String>>, AnnotateError> {
    let (mut all, _) = load(repo);
    let matching: Vec<usize> = all.iter().enumerate().filter(|(_, a)| !prefix.is_empty() && a.id.starts_with(prefix)).map(|(i, _)| i).collect();
    match matching.as_slice() {
        [one] => {
            let removed = all.remove(*one);
            save_all(repo, &mut all)?;
            Ok(Ok(removed))
        }
        other => Ok(Err(other.iter().map(|i| all[*i].id.clone()).collect())),
    }
}

// ---------------------------------------------------------------------------
// Stable keys
// ---------------------------------------------------------------------------

/// The stable key of a node, or `None` for kinds that are not annotated (columns, constraints, annotations…).
pub fn key_of(snapshot: &GraphSnapshot, id: &StableId) -> Option<String> {
    Some(match snapshot.nodes.get(id)? {
        NodePayload::File { path, .. } => path.clone(),
        NodePayload::Symbol { name, .. } => format!("{}::{name}", snapshot.path_of(id)?),
        NodePayload::Requirement { title, .. } | NodePayload::Task { title, .. } | NodePayload::Adr { title, .. } => title.clone(),
        NodePayload::Table { schema, name, .. } => format!("table:{schema}.{name}"),
        NodePayload::Package { name, .. } => format!("package:{name}"),
        NodePayload::Endpoint { method, path, external, .. } => format!("endpoint:{}{method} {path}", if *external { "call:" } else { "" }),
        NodePayload::DocSection { .. } | NodePayload::Column { .. } | NodePayload::Constraint { .. } | NodePayload::Annotation { .. } => return None,
    })
}

/// The node a key points at today, if there is one (the lowest line for a repeated symbol name).
pub fn resolve_key(snapshot: &GraphSnapshot, key: &str) -> Option<StableId> {
    let exists = |id: StableId| snapshot.nodes.contains_key(&id).then_some(id);
    if let Some(rest) = key.strip_prefix("table:") {
        let (schema, name) = rest.split_once('.').unwrap_or(("public", rest));
        return exists(table_node_id(schema, name));
    }
    if let Some(name) = key.strip_prefix("package:") {
        return exists(package_node_id(name));
    }
    if let Some(rest) = key.strip_prefix("endpoint:") {
        let (external, rest) = rest.strip_prefix("call:").map_or((false, rest), |r| (true, r));
        let (method, path) = rest.split_once(' ')?;
        return exists(endpoint_node_id(method, path, external));
    }
    if let Some((path, name)) = key.rsplit_once("::") {
        return snapshot
            .nodes
            .iter()
            .filter(|(id, p)| matches!(p, NodePayload::Symbol { name: n, .. } if n == name) && snapshot.path_of(id) == Some(path))
            .min_by_key(|(_, p)| if let NodePayload::Symbol { line_start, .. } = p { *line_start } else { u32::MAX })
            .map(|(id, _)| *id);
    }
    if let Some(id) = exists(file_node_id(key)) {
        return Some(id);
    }
    // Only when the direct id misses (a dangling file key, or a graph whose file ids are not the usual ones).
    if let Some((id, _)) = snapshot.nodes.iter().find(|(_, p)| matches!(p, NodePayload::File { path, .. } if path == key)) {
        return Some(*id);
    }
    snapshot
        .nodes
        .iter()
        .find(|(_, p)| matches!(p, NodePayload::Requirement { title, .. } | NodePayload::Task { title, .. } | NodePayload::Adr { title, .. } if title == key))
        .map(|(id, _)| *id)
}

/// The hash that says "the target is still what the annotation was about" (decision D3): the file that holds the
/// target, or a requirement's title and body. `None` for kinds that never go stale.
pub fn target_hash(repo: &Path, snapshot: &GraphSnapshot, id: &StableId) -> Option<String> {
    match snapshot.nodes.get(id)? {
        NodePayload::Requirement { title, body, .. } | NodePayload::Task { title, body, .. } | NodePayload::Adr { title, body, .. } => {
            Some(blake3::hash(format!("{title}\n{body}").as_bytes()).to_hex().to_string())
        }
        NodePayload::File { .. } | NodePayload::Symbol { .. } => {
            let path = snapshot.path_of(id)?;
            std::fs::read(repo.join(path)).ok().map(|bytes| blake3::hash(&bytes).to_hex().to_string())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::snapshot::test_support::*;

    fn draft(target: &str) -> Draft {
        Draft { target: target.to_string(), author: "agent".to_string(), ..Draft::default() }
    }

    #[test]
    fn an_annotation_needs_something_to_say_and_keeps_it_short() {
        assert!(matches!(Annotation::new(draft("a.ts"), 0), Err(AnnotateError::Empty)));
        let long = Draft { note: Some("x".repeat(501)), ..draft("a.ts") };
        assert!(matches!(Annotation::new(long, 0), Err(AnnotateError::NoteTooLong(501))));
        let ok = Draft { note: Some("x".repeat(500)), ..draft("a.ts") };
        assert!(Annotation::new(ok, 0).is_ok(), "exactly at the limit is fine");
        let relation_only = Draft { relation: Some("calls".into()), ..draft("a.ts") };
        assert!(matches!(Annotation::new(relation_only, 0), Err(AnnotateError::RelationIncomplete(_))));
        let bad_relation = Draft { relation: Some("loves".into()), to: Some("b.ts".into()), ..draft("a.ts") };
        assert!(matches!(Annotation::new(bad_relation, 0), Err(AnnotateError::UnknownRelation(_))));
        assert!(matches!(Annotation::new(Draft { outcome: Some("great".into()), ..draft("a.ts") }, 0), Err(AnnotateError::Outcome(_))));
        assert!(matches!(Annotation::new(Draft { note: Some("n".into()), author: "robot".into(), ..draft("a.ts") }, 0), Err(AnnotateError::Author(_))));
    }

    #[test]
    fn secrets_are_refused_by_kind_without_being_echoed() {
        let token = format!("{}{}", "gh", "p_abcdefghijklmnopqrstuvwxyz0123456789");
        let err = Annotation::new(Draft { note: Some(format!("deploy with {token}")), ..draft("a.ts") }, 0).unwrap_err();
        assert!(matches!(err, AnnotateError::Secret("note", "GitHub token")), "{err:?}");
        assert!(!err.to_string().contains(&token));
    }

    #[test]
    fn saying_the_same_thing_twice_is_one_annotation_with_a_fresh_time() {
        let dir = tempfile::TempDir::new().unwrap();
        let a = Annotation::new(Draft { note: Some("This module bills residents.".into()), ..draft("src/a.ts") }, 100).unwrap();
        let again = Annotation::new(Draft { note: Some("  This module   bills residents. ".into()), ..draft("src/a.ts") }, 200).unwrap();
        assert_eq!(a.id, again.id, "whitespace does not make a new annotation");
        add(dir.path(), a).unwrap();
        add(dir.path(), again.clone()).unwrap();
        let (all, skipped) = load(dir.path());
        assert_eq!((all.len(), skipped), (1, 0));
        assert_eq!(all[0].at, again.at);
        assert_eq!(all[0].confidence, "INFERRED");
    }

    #[test]
    fn the_file_is_ordered_by_target_then_time_and_unreadable_lines_are_counted() {
        let dir = tempfile::TempDir::new().unwrap();
        for (target, note, at) in [("b.ts", "second file", 50), ("a.ts", "later note", 300), ("a.ts", "early note", 100)] {
            add(dir.path(), Annotation::new(Draft { note: Some(note.into()), ..draft(target) }, at).unwrap()).unwrap();
        }
        let (all, _) = load(dir.path());
        let order: Vec<(&str, &str)> = all.iter().map(|a| (a.target.as_str(), a.note.as_deref().unwrap())).collect();
        assert_eq!(order, [("a.ts", "early note"), ("a.ts", "later note"), ("b.ts", "second file")]);
        let path = annotations_path(dir.path());
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("not json\n");
        std::fs::write(&path, text).unwrap();
        assert_eq!(load(dir.path()).1, 1);
    }

    #[test]
    fn remove_takes_a_unique_id_prefix_and_refuses_an_ambiguous_or_unknown_one() {
        let dir = tempfile::TempDir::new().unwrap();
        let a = Annotation::new(Draft { note: Some("one".into()), ..draft("a.ts") }, 1).unwrap();
        let id = a.id.clone();
        add(dir.path(), a).unwrap();
        assert!(remove(dir.path(), "zzzz").unwrap().is_err());
        assert!(remove(dir.path(), "").unwrap().is_err(), "an empty prefix matches nothing");
        assert_eq!(remove(dir.path(), &id[..4]).unwrap().unwrap().id, id);
        assert!(load(dir.path()).0.is_empty());
    }

    #[test]
    fn keys_are_stable_and_point_back_at_the_node() {
        let snap = snapshot(
            vec![(1, file("src/a.ts")), (2, symbol("Alpha")), (3, requirement("REQ-7"))],
            vec![edge(1, 2, 1, crate::graph::edge::EdgeType::DefinedIn)],
        );
        assert_eq!(key_of(&snap, &id(1)).as_deref(), Some("src/a.ts"));
        assert_eq!(key_of(&snap, &id(2)).as_deref(), Some("src/a.ts::Alpha"));
        assert_eq!(key_of(&snap, &id(3)).as_deref(), Some("REQ-7"));
        assert_eq!(resolve_key(&snap, "src/a.ts::Alpha"), Some(id(2)));
        assert_eq!(resolve_key(&snap, "REQ-7"), Some(id(3)));
        assert_eq!(resolve_key(&snap, "src/gone.ts"), None);
        assert_eq!(resolve_key(&snap, "src/a.ts::Missing"), None);
    }

    #[test]
    fn a_requirement_hash_follows_its_words_and_a_symbol_hash_follows_its_file() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/a.ts"), "one").unwrap();
        let snap = snapshot(vec![(1, file("src/a.ts")), (2, symbol("Alpha"))], vec![edge(1, 2, 1, crate::graph::edge::EdgeType::DefinedIn)]);
        let before = target_hash(dir.path(), &snap, &id(2)).unwrap();
        assert_eq!(before, target_hash(dir.path(), &snap, &id(1)).unwrap(), "the symbol is as stale as its file");
        std::fs::write(dir.path().join("src/a.ts"), "two").unwrap();
        assert_ne!(before, target_hash(dir.path(), &snap, &id(2)).unwrap());
    }
}
