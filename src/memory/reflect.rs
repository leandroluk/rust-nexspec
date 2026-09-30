//! From saved results to lessons (REQ-1502, REQ-1504 in `.specs/features/work-memory/spec.md`;
//! decisions D4-D6). Pure: the notes, what the graph still knows, and the clock go in; a classification per
//! node, the corrections and two renderings come out. No date ends up in the output, so `LESSONS.md` only
//! changes when a classification does.

use std::collections::BTreeMap;

use crate::memory::note::{Note, Outcome};

/// A signal older than two half-lives (weight < 0.25) no longer counts.
pub const LIVE_WEIGHT: f64 = 0.25;
pub const DEFAULT_HALF_LIFE_DAYS: f64 = 30.0;
pub const DEFAULT_MIN_USEFUL: usize = 2;

#[derive(Debug, Clone)]
pub struct ReflectOptions {
    /// Unix seconds "now" (a parameter so tests and `--now` are exact).
    pub now: u64,
    pub half_life_days: f64,
    /// Useful answers needed for a node to be *preferred*.
    pub min_useful: usize,
}

impl ReflectOptions {
    pub fn new(now: u64) -> Self {
        Self { now, half_life_days: DEFAULT_HALF_LIFE_DAYS, min_useful: DEFAULT_MIN_USEFUL }
    }

    /// `0.5 ^ (age / half-life)`.
    pub fn weight(&self, at: u64) -> f64 {
        let age_days = self.now.saturating_sub(at) as f64 / 86_400.0;
        0.5f64.powf(age_days / self.half_life_days.max(0.001))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Preferred,
    Tentative,
    /// Useful and negative signals together; the most recent one decides which way it leans.
    Contested { leaning_useful: bool },
    DeadEnd,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lesson {
    pub id: String,
    pub label: String,
    pub class: Class,
    pub useful: usize,
    pub negative: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Reflection {
    pub lessons: Vec<Lesson>,
    /// `(question, correction)` of the live `corrected` notes.
    pub corrections: Vec<(String, String)>,
    pub notes_live: usize,
    /// Notes whose signal has faded away.
    pub notes_faded: usize,
    /// Distinct cited nodes the graph no longer has.
    pub nodes_dropped: usize,
}

fn classify(useful: usize, negative: usize, latest_is_useful: bool, min_useful: usize) -> Option<Class> {
    match (useful, negative) {
        (0, 0) => None,
        (u, 0) if u >= min_useful.max(1) => Some(Class::Preferred),
        (_, 0) => Some(Class::Tentative),
        (0, _) => Some(Class::DeadEnd),
        _ => Some(Class::Contested { leaning_useful: latest_is_useful }),
    }
}

/// `current_label(id)` is the node's label in the graph now, or `None` if it no longer exists.
pub fn reflect(notes: &[Note], current_label: &dyn Fn(&str) -> Option<String>, options: &ReflectOptions) -> Reflection {
    let mut result = Reflection::default();
    // node id -> (label, useful count, negative count, time and sign of the latest signal)
    let mut nodes: BTreeMap<String, (String, usize, usize, (u64, bool))> = BTreeMap::new();
    let mut dropped: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for note in notes {
        if options.weight(note.at) < LIVE_WEIGHT {
            result.notes_faded += 1;
            continue;
        }
        result.notes_live += 1;
        if note.outcome == Outcome::Corrected
            && let Some(correction) = &note.correction
        {
            result.corrections.push((note.question.clone(), correction.clone()));
        }
        for node in &note.nodes {
            let Some(label) = current_label(&node.id) else {
                dropped.insert(node.id.as_str());
                continue;
            };
            let entry = nodes.entry(node.id.clone()).or_insert((label, 0, 0, (0, true)));
            if note.outcome.is_negative() {
                entry.2 += 1;
            } else {
                entry.1 += 1;
            }
            // Later wins; on the same second a negative signal wins (the careful reading).
            let signal = (note.at, !note.outcome.is_negative());
            if signal.0 > entry.3.0 || (signal.0 == entry.3.0 && !signal.1) {
                entry.3 = signal;
            }
        }
    }
    result.nodes_dropped = dropped.len();
    result.lessons = nodes
        .into_iter()
        .filter_map(|(id, (label, useful, negative, (_, latest_useful)))| {
            classify(useful, negative, latest_useful, options.min_useful).map(|class| Lesson { id, label, class, useful, negative })
        })
        .collect();
    result.lessons.sort_by(|a, b| a.label.cmp(&b.label).then(a.id.cmp(&b.id)));
    result.corrections.sort();
    result.corrections.dedup();
    result
}

fn times(n: usize) -> String {
    format!("×{n}")
}

/// `LESSONS.md`: fixed section order, stable item order, no dates.
pub fn to_markdown(reflection: &Reflection) -> String {
    let mut out = String::from("# Lessons\n\nLearned from saved results (`nexspec save-result`); regenerate with `nexspec reflect`.\n");
    let section = |out: &mut String, title: &str, items: Vec<String>| {
        if !items.is_empty() {
            out.push_str(&format!("\n## {title}\n\n"));
            for item in items {
                out.push_str(&format!("- {item}\n"));
            }
        }
    };
    let pick = |wanted: &dyn Fn(Class) -> bool| -> Vec<&Lesson> { reflection.lessons.iter().filter(|l| wanted(l.class)).collect() };
    section(&mut out, "Preferred", pick(&|c| c == Class::Preferred).iter().map(|l| format!("`{}` — useful {}", l.label, times(l.useful))).collect());
    section(&mut out, "Tentative", pick(&|c| c == Class::Tentative).iter().map(|l| format!("`{}` — useful {}", l.label, times(l.useful))).collect());
    section(
        &mut out,
        "Contested",
        pick(&|c| matches!(c, Class::Contested { .. }))
            .iter()
            .map(|l| {
                let leaning = matches!(l.class, Class::Contested { leaning_useful: true });
                format!("`{}` — useful {}, dead end or corrected {}; the latest says {}", l.label, times(l.useful), times(l.negative), if leaning { "useful" } else { "avoid" })
            })
            .collect(),
    );
    section(&mut out, "Dead ends", pick(&|c| c == Class::DeadEnd).iter().map(|l| format!("`{}` — dead end or corrected {}", l.label, times(l.negative))).collect());
    section(&mut out, "Corrections", reflection.corrections.iter().map(|(q, c)| format!("**{}** → {}", one_line(q), one_line(c))).collect());
    if reflection.lessons.is_empty() && reflection.corrections.is_empty() {
        out.push_str("\nNo lessons yet.\n");
    }
    out
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The short version a skill loads at the start of a session: what is most worth knowing first
/// (corrections, then what to avoid, then what to prefer), one line each, cut by the token budget.
pub fn summary_lines(reflection: &Reflection) -> Vec<String> {
    let mut lines = vec!["# Work memory (nexspec)".to_string()];
    for (question, correction) in &reflection.corrections {
        lines.push(format!("- correction: {} → {}", one_line(question), one_line(correction)));
    }
    for lesson in reflection.lessons.iter().filter(|l| matches!(l.class, Class::DeadEnd | Class::Contested { leaning_useful: false })) {
        lines.push(format!("- avoid: {}", lesson.label));
    }
    for lesson in reflection.lessons.iter().filter(|l| matches!(l.class, Class::Preferred | Class::Contested { leaning_useful: true })) {
        lines.push(format!("- prefer: {}", lesson.label));
    }
    if lines.len() == 1 {
        lines.push("- no lessons yet".to_string());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::note::NodeRef;

    const DAY: u64 = 86_400;
    const NOW: u64 = 1_790_000_000;

    fn note(question: &str, outcome: Outcome, nodes: &[&str], days_ago: u64, correction: Option<&str>) -> Note {
        Note::new(question, "answer", "query", outcome, nodes.iter().map(|id| NodeRef { id: id.to_string(), label: format!("old {id}") }).collect(), correction, NOW - days_ago * DAY).unwrap()
    }

    fn label(id: &str) -> Option<String> {
        (id != "gone").then(|| format!("Node {id}"))
    }

    fn reflect_now(notes: &[Note]) -> Reflection {
        reflect(notes, &label, &ReflectOptions::new(NOW))
    }

    fn class_of(r: &Reflection, id: &str) -> Option<Class> {
        r.lessons.iter().find(|l| l.id == id).map(|l| l.class)
    }

    #[test]
    fn corroboration_makes_a_node_preferred_and_one_signal_only_tentative() {
        let r = reflect_now(&[note("q1", Outcome::Useful, &["a", "b"], 1, None), note("q2", Outcome::Useful, &["a"], 2, None)]);
        assert_eq!(class_of(&r, "a"), Some(Class::Preferred));
        assert_eq!(class_of(&r, "b"), Some(Class::Tentative));
        assert_eq!(r.lessons.iter().find(|l| l.id == "a").unwrap().label, "Node a", "the label is the graph's today");
    }

    #[test]
    fn negative_signals_make_dead_ends_and_mixed_ones_are_decided_by_recency() {
        let r = reflect_now(&[
            note("q1", Outcome::DeadEnd, &["x"], 3, None),
            note("q2", Outcome::Corrected, &["y"], 4, Some("use z")),
            note("q3", Outcome::Useful, &["m"], 10, None),
            note("q4", Outcome::DeadEnd, &["m"], 1, None),
            note("q5", Outcome::DeadEnd, &["n"], 10, None),
            note("q6", Outcome::Useful, &["n"], 1, None),
        ]);
        assert_eq!(class_of(&r, "x"), Some(Class::DeadEnd));
        assert_eq!(class_of(&r, "y"), Some(Class::DeadEnd), "a correction says the cited node was wrong");
        assert_eq!(class_of(&r, "m"), Some(Class::Contested { leaning_useful: false }), "the dead end is newer");
        assert_eq!(class_of(&r, "n"), Some(Class::Contested { leaning_useful: true }), "the useful answer is newer");
        assert_eq!(r.corrections, [("q2".to_string(), "use z".to_string())]);
    }

    #[test]
    fn old_signals_fade_and_vanished_nodes_are_dropped() {
        let r = reflect_now(&[note("old", Outcome::Useful, &["a"], 90, None), note("fresh", Outcome::Useful, &["b"], 1, None), note("ghost", Outcome::Useful, &["gone", "b"], 1, None)]);
        assert_eq!(class_of(&r, "a"), None, "three half-lives ago no longer counts");
        assert_eq!((r.notes_live, r.notes_faded, r.nodes_dropped), (2, 1, 1));
        assert_eq!(class_of(&r, "b"), Some(Class::Preferred), "two live useful answers");
        assert!(class_of(&r, "gone").is_none());
    }

    #[test]
    fn the_half_life_and_the_threshold_are_what_the_options_say() {
        let options = ReflectOptions { half_life_days: 10.0, ..ReflectOptions::new(NOW) };
        let r = reflect(&[note("q", Outcome::Useful, &["a"], 25, None)], &label, &options);
        assert!(r.lessons.is_empty(), "25 days is 2.5 half-lives of 10");
        let strict = ReflectOptions { min_useful: 3, ..ReflectOptions::new(NOW) };
        let r = reflect(&[note("q1", Outcome::Useful, &["a"], 1, None), note("q2", Outcome::Useful, &["a"], 1, None)], &label, &strict);
        assert_eq!(class_of(&r, "a"), Some(Class::Tentative));
        assert!((ReflectOptions::new(NOW).weight(NOW - 30 * DAY) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn lessons_md_is_ordered_dateless_and_stable() {
        let notes = [note("q1", Outcome::Useful, &["b"], 1, None), note("q2", Outcome::Useful, &["b"], 2, None), note("q3", Outcome::DeadEnd, &["a"], 1, None), note("q4", Outcome::Corrected, &["c"], 1, Some("it is D"))];
        let md = to_markdown(&reflect_now(&notes));
        let order: Vec<usize> = ["## Preferred", "## Dead ends", "## Corrections"].iter().map(|h| md.find(h).unwrap()).collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{md}");
        assert!(md.contains("- `Node b` — useful ×2") && md.contains("- `Node a` — dead end or corrected ×1") && md.contains("- **q4** → it is D"), "{md}");
        assert!(!md.contains("2026") && !md.contains("T18"), "no dates");
        assert_eq!(md, to_markdown(&reflect_now(&notes)), "deterministic");
        assert!(to_markdown(&Reflection::default()).contains("No lessons yet."));
    }

    #[test]
    fn the_session_summary_puts_corrections_first_and_fits_a_budget() {
        let notes = [note("q1", Outcome::Useful, &["p"], 1, None), note("q2", Outcome::Useful, &["p"], 2, None), note("q3", Outcome::DeadEnd, &["d"], 1, None), note("q4", Outcome::Corrected, &["c"], 1, Some("it is D"))];
        let lines = summary_lines(&reflect_now(&notes));
        assert_eq!(lines[0], "# Work memory (nexspec)");
        assert!(lines[1].starts_with("- correction: q4 → it is D"), "{lines:?}");
        assert!(lines.iter().position(|l| l.starts_with("- avoid")) < lines.iter().position(|l| l.starts_with("- prefer")));
        let tiny = crate::query::budget::fit_lines(lines.clone(), Some(20));
        assert!(tiny.len() < lines.len() + 1 && tiny[0] == lines[0], "the title always stays");
        assert_eq!(summary_lines(&Reflection::default()), ["# Work memory (nexspec)", "- no lessons yet"]);
    }
}
