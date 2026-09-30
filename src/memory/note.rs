//! One remembered answer (REQ-1501, REQ-1505 in `.specs/features/work-memory/spec.md`; decisions D1, D2, D10).
//!
//! A note is a small Markdown file: a frontmatter with what `reflect` needs (stable id, time, outcome, the
//! nodes the answer cited) and a body a person can read and edit. Saving the same thing twice replaces the
//! note and renews its time; that is how a signal gets fresher.

use std::path::{Path, PathBuf};

use crate::enrich::select::find_secret;

pub const MEMORY_DIR: &str = ".specs/.memory";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Outcome {
    Useful,
    DeadEnd,
    Corrected,
}

impl Outcome {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "useful" => Some(Self::Useful),
            "dead_end" => Some(Self::DeadEnd),
            "corrected" => Some(Self::Corrected),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Useful => "useful",
            Self::DeadEnd => "dead_end",
            Self::Corrected => "corrected",
        }
    }

    /// `dead_end` and `corrected` both say "do not go there again".
    pub fn is_negative(self) -> bool {
        self != Self::Useful
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NoteError {
    #[error("a `corrected` result needs --correction: what was the right answer?")]
    CorrectionMissing,
    #[error("the {0} looks like a {1}: notes are kept in the repository, so secrets are never saved")]
    Secret(&'static str, &'static str),
    #[error("a note needs a question")]
    EmptyQuestion,
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRef {
    /// Stable node id, lowercase hex.
    pub id: String,
    /// What it was called when the note was written (`Name (path)`), for people.
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub id: String,
    /// Unix seconds.
    pub at: u64,
    /// `query`, `path`, `explain`, `affected`, `search`… free text, one word.
    pub kind: String,
    pub outcome: Outcome,
    pub nodes: Vec<NodeRef>,
    pub question: String,
    pub answer: String,
    pub correction: Option<String>,
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Note {
    pub fn new(question: &str, answer: &str, kind: &str, outcome: Outcome, mut nodes: Vec<NodeRef>, correction: Option<&str>, at: u64) -> Result<Self, NoteError> {
        let question = question.trim();
        if question.is_empty() {
            return Err(NoteError::EmptyQuestion);
        }
        let correction = correction.map(str::trim).filter(|c| !c.is_empty());
        if outcome == Outcome::Corrected && correction.is_none() {
            return Err(NoteError::CorrectionMissing);
        }
        for (what, text) in [("question", question), ("answer", answer), ("correction", correction.unwrap_or(""))] {
            if let Some(kind) = find_secret(text) {
                return Err(NoteError::Secret(what, kind));
            }
        }
        nodes.sort_by(|a, b| a.id.cmp(&b.id));
        nodes.dedup_by(|a, b| a.id == b.id);
        let id = note_id(question, outcome, &nodes);
        let kind = one_line(kind);
        Ok(Self {
            id,
            at,
            kind: if kind.is_empty() { "query".to_string() } else { kind },
            outcome,
            nodes: nodes.into_iter().map(|n| NodeRef { label: one_line(&n.label), id: n.id }).collect(),
            question: question.to_string(),
            answer: answer.trim().to_string(),
            correction: correction.map(str::to_string),
        })
    }

    pub fn to_markdown(&self) -> String {
        let mut out = String::from("---\n");
        out.push_str(&format!("id: {}\nat: {}\ntype: {}\noutcome: {}\nnodes:\n", self.id, format_iso(self.at), self.kind, self.outcome.as_str()));
        for node in &self.nodes {
            out.push_str(&format!("  - {} {}\n", node.id, node.label));
        }
        out.push_str("---\n\n## Question\n\n");
        out.push_str(&self.question);
        out.push_str("\n\n## Answer\n\n");
        out.push_str(&self.answer);
        out.push('\n');
        if let Some(correction) = &self.correction {
            out.push_str("\n## Correction\n\n");
            out.push_str(correction);
            out.push('\n');
        }
        out
    }

    pub fn from_markdown(text: &str) -> Option<Self> {
        let text = text.replace("\r\n", "\n");
        let rest = text.strip_prefix("---\n")?;
        let (front, body) = rest.split_once("\n---\n")?;
        let (mut id, mut at, mut kind, mut outcome) = (None, None, None, None);
        let mut nodes = Vec::new();
        for line in front.lines() {
            if let Some(item) = line.strip_prefix("  - ") {
                let (node_id, label) = item.split_once(' ').unwrap_or((item, ""));
                nodes.push(NodeRef { id: node_id.to_string(), label: label.to_string() });
            } else if let Some((key, value)) = line.split_once(':') {
                let value = value.trim();
                match key.trim() {
                    "id" => id = Some(value.to_string()),
                    "at" => at = parse_iso(value),
                    "type" => kind = Some(value.to_string()),
                    "outcome" => outcome = Outcome::parse(value),
                    _ => {}
                }
            }
        }
        let section = |name: &str| -> Option<String> {
            let marker = format!("## {name}\n\n");
            let start = body.find(&marker)? + marker.len();
            let end = body[start..].find("\n## ").map_or(body.len(), |e| start + e);
            Some(body[start..end].trim().to_string())
        };
        Some(Self { id: id?, at: at?, kind: kind?, outcome: outcome?, nodes, question: section("Question")?, answer: section("Answer").unwrap_or_default(), correction: section("Correction") })
    }
}

/// Ten hex characters of `blake3(question, outcome, sorted node ids)`: the same answer saved again is the same note.
pub fn note_id(question: &str, outcome: Outcome, nodes: &[NodeRef]) -> String {
    let mut material = format!("{}\n{}", one_line(question), outcome.as_str());
    for node in nodes {
        material.push('\n');
        material.push_str(&node.id);
    }
    blake3::hash(material.as_bytes()).to_hex()[..10].to_string()
}

pub fn notes_dir(repo: &Path) -> PathBuf {
    repo.join(MEMORY_DIR).join("notes")
}

/// Writes the note (replacing one with the same id) and returns where.
pub fn save(repo: &Path, note: &Note) -> Result<PathBuf, NoteError> {
    let dir = notes_dir(repo);
    let path = dir.join(format!("{}.md", note.id));
    let io = |source| NoteError::Io { path: path.display().to_string(), source };
    std::fs::create_dir_all(&dir).map_err(io)?;
    std::fs::write(&path, note.to_markdown()).map_err(io)?;
    Ok(path)
}

/// Every readable note, by id; files that are not notes are skipped and counted.
pub fn load_all(repo: &Path) -> (Vec<Note>, usize) {
    let mut notes = Vec::new();
    let mut skipped = 0;
    let Ok(entries) = std::fs::read_dir(notes_dir(repo)) else { return (notes, 0) };
    for entry in entries.flatten() {
        if entry.path().extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        match std::fs::read_to_string(entry.path()).ok().and_then(|t| Note::from_markdown(&t)) {
            Some(note) => notes.push(note),
            None => skipped += 1,
        }
    }
    notes.sort_by(|a, b| a.id.cmp(&b.id));
    (notes, skipped)
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

// --- ISO 8601 UTC without a date crate (civil <-> days: Howard Hinnant) ---

pub fn format_iso(seconds: u64) -> String {
    let (days, rest) = ((seconds / 86_400) as i64, seconds % 86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3600, rest % 3600 / 60, rest % 60)
}

pub fn parse_iso(text: &str) -> Option<u64> {
    let text = text.trim().strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (year, month, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.split(':').map(|p| p.parse::<i64>().ok());
    let (h, m, s) = (t.next()??, t.next()??, t.next()??);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || !(0..24).contains(&h) || !(0..60).contains(&m) || !(0..61).contains(&s) {
        return None;
    }
    let y = year - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + h * 3600 + m * 60 + s).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, label: &str) -> NodeRef {
        NodeRef { id: id.to_string(), label: label.to_string() }
    }

    #[test]
    fn iso_round_trips_across_leap_years_and_the_epoch() {
        for seconds in [0u64, 86_399, 951_782_400 /* 2000-02-29 */, 1_790_000_000, 4_102_444_800 /* 2100-01-01 */] {
            assert_eq!(parse_iso(&format_iso(seconds)), Some(seconds), "{seconds}");
        }
        assert_eq!(format_iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(parse_iso("2026-09-30T18:05:00Z"), Some(1_790_791_500));
        assert_eq!(parse_iso("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_iso("yesterday"), None);
    }

    #[test]
    fn a_note_survives_a_round_trip_through_markdown() {
        let note = Note::new("How does billing charge residents?", "Through ChargeUsecase.\nSecond line.", "query", Outcome::Corrected, vec![node("bb", "B (b.ts)"), node("aa", "A (a.ts)")], Some("It is ChargeService, not ChargeUsecase."), 1_790_791_500).unwrap();
        assert_eq!(note.nodes[0].id, "aa", "nodes are sorted by id");
        let again = Note::from_markdown(&note.to_markdown()).expect("readable");
        assert_eq!(again, note);
        assert!(note.to_markdown().contains("at: 2026-09-30T18:05:00Z"));
    }

    #[test]
    fn the_same_answer_saved_twice_is_the_same_note_and_another_outcome_is_not() {
        let a = Note::new("Q?", "x", "query", Outcome::Useful, vec![node("aa", "A")], None, 10).unwrap();
        let b = Note::new("  Q?  ", "a different answer text", "query", Outcome::Useful, vec![node("aa", "A")], None, 99).unwrap();
        assert_eq!(a.id, b.id, "the id depends on question, outcome and nodes only");
        let c = Note::new("Q?", "x", "query", Outcome::DeadEnd, vec![node("aa", "A")], None, 10).unwrap();
        assert_ne!(a.id, c.id);
        assert_eq!(a.id.len(), 10);
    }

    #[test]
    fn a_correction_is_required_and_secrets_are_refused_without_being_echoed() {
        assert!(matches!(Note::new("Q?", "x", "query", Outcome::Corrected, vec![], None, 1), Err(NoteError::CorrectionMissing)));
        let key = format!("{}{}", "gh", "p_abcdefghijklmnopqrstuvwxyz0123456789");
        let err = Note::new("Where is the token?", &format!("it is {key}"), "query", Outcome::Useful, vec![], None, 1).unwrap_err();
        assert!(matches!(err, NoteError::Secret("answer", "GitHub token")), "{err:?}");
        assert!(!err.to_string().contains(&key));
        assert!(matches!(Note::new("   ", "x", "query", Outcome::Useful, vec![], None, 1), Err(NoteError::EmptyQuestion)));
    }

    #[test]
    fn notes_are_saved_replaced_and_loaded_in_a_stable_order() {
        let dir = tempfile::TempDir::new().unwrap();
        let first = Note::new("Q1?", "x", "query", Outcome::Useful, vec![node("aa", "A")], None, 10).unwrap();
        let second = Note::new("Q2?", "y", "path", Outcome::DeadEnd, vec![node("bb", "B")], None, 20).unwrap();
        save(dir.path(), &first).unwrap();
        save(dir.path(), &second).unwrap();
        let newer = Note { at: 500, ..first.clone() };
        save(dir.path(), &newer).unwrap();
        std::fs::write(notes_dir(dir.path()).join("junk.md"), "not a note").unwrap();
        std::fs::write(notes_dir(dir.path()).join("readme.txt"), "ignored").unwrap();

        let (notes, skipped) = load_all(dir.path());
        assert_eq!((notes.len(), skipped), (2, 1));
        assert_eq!(notes.iter().find(|n| n.id == first.id).unwrap().at, 500, "saving again renewed the time");
        let ids: Vec<&str> = notes.iter().map(|n| n.id.as_str()).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
        assert_eq!(load_all(&dir.path().join("nowhere")).0.len(), 0);
    }
}
