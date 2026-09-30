//! Markdown and JSON output for a [`Report`] (REQ-1005, REQ-1007).
//!
//! The Markdown sections and their names are a contract for the skill
//! (design.md D4): `Summary`, `God Nodes`, `Communities`, `Requirement
//! Coverage`, `Surprising Connections`, `Import Cycles`, `Suggested
//! Questions`. With a token budget, sections are kept in priority order and
//! the lowest-priority ones shrink first (their trailing items are replaced
//! by a "+N omitted" line) before anything is dropped whole.

use crate::report::Report;
use crate::token::budget::{CharHeuristicTokenizer, Tokenizer};

pub fn to_json(report: &Report) -> String {
    serde_json::to_string_pretty(report).expect("Report always serializes")
}

/// One Markdown section: a heading, an optional fixed preface and a list of
/// items that can be truncated.
struct Section {
    title: &'static str,
    preface: Vec<String>,
    items: Vec<String>,
    /// Table header lines repeated before `items` (kept when items shrink).
    table_header: Vec<String>,
    /// Lower = more important: shrinks last.
    priority: u8,
    omitted: usize,
}

impl Section {
    fn new(title: &'static str, priority: u8) -> Self {
        Section { title, preface: Vec::new(), items: Vec::new(), table_header: Vec::new(), priority, omitted: 0 }
    }

    fn render(&self) -> String {
        let mut out = format!("## {}\n\n", self.title);
        for line in &self.preface {
            out.push_str(line);
            out.push('\n');
        }
        if !self.preface.is_empty() {
            out.push('\n');
        }
        if !self.items.is_empty() {
            for line in &self.table_header {
                out.push_str(line);
                out.push('\n');
            }
            for item in &self.items {
                out.push_str(item);
                out.push('\n');
            }
        }
        if self.omitted > 0 {
            out.push_str(&format!("\n_+{} omitted to fit the token budget._\n", self.omitted));
        }
        out
    }
}

/// Longest list printed per section before "+N more" (the JSON keeps everything).
const MAX_COMMUNITIES_LISTED: usize = 15;
const MAX_COVERAGE_ITEMS: usize = 10;

/// Items of a list capped at `limit`, with a "+N more" line when cut.
fn capped(mut items: Vec<String>, limit: usize) -> Vec<String> {
    if items.len() > limit {
        let more = items.len() - limit;
        items.truncate(limit);
        items.push(format!("- … and {more} more (see `--format json`)"));
    }
    items
}

fn sections(report: &Report) -> Vec<Section> {
    let mut all = Vec::new();

    let mut summary = Section::new("Summary", 0);
    let s = &report.summary;
    if s.total_nodes == 0 {
        summary.preface.push("**The index is empty**: run `nexspec sync` first, then ask for the report again.".to_string());
    }
    let by = |m: &std::collections::BTreeMap<String, usize>| m.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ");
    summary.preface.push(format!("- Nodes: {} ({})", s.total_nodes, by(&s.nodes_by_type)));
    summary.preface.push(format!("- Edges: {} ({})", s.total_edges, by(&s.edges_by_type)));
    if !s.files_by_language.is_empty() {
        summary.preface.push(format!("- Files by language: {}", by(&s.files_by_language)));
    }
    let commit = s.index.last_indexed_commit.as_deref().map(|c| &c[..c.len().min(12)]).unwrap_or("none");
    summary.preface.push(format!(
        "- Index: commit {commit}{}, sync version {}, {}",
        s.index.last_indexed_at.map(|t| format!(" ({})", format_date(t))).unwrap_or_default(),
        s.index.sync_version,
        format_bytes(s.index.index_bytes)
    ));
    all.push(summary);

    let mut god = Section::new("God Nodes", 1);
    if report.barrel_files_excluded > 0 {
        god.preface.push(format!(
            "{} barrel file(s) (`index.ts` re-exports) are left out: their degree only reflects import paths.",
            report.barrel_files_excluded
        ));
    }
    if report.god_nodes.is_empty() {
        god.preface.push("No node has structural links yet.".to_string());
    } else {
        god.table_header = vec![
            "| # | Node | Kind | Degree | Dependents | Dependencies | Requirement |".to_string(),
            "|---:|---|---|---:|---:|---:|---|".to_string(),
        ];
        god.items = report
            .god_nodes
            .iter()
            .enumerate()
            .map(|(i, g)| {
                format!(
                    "| {} | `{}` | {} | {} | {} | {} | {} |",
                    i + 1,
                    g.label,
                    g.kind,
                    g.degree,
                    g.dependents,
                    g.dependencies,
                    if g.has_requirement { "yes" } else { "no" }
                )
            })
            .collect();
    }
    all.push(god);

    let mut coverage = Section::new("Requirement Coverage", 2);
    let c = &report.requirement_coverage;
    coverage.preface.push(format!(
        "{} requirements; {} not linked to any code or task; {} tasks without requirement; {} code references to missing requirements.",
        c.requirements_total,
        c.unimplemented_requirements.len(),
        c.tasks_without_requirement.len(),
        c.orphan_references.len()
    ));
    coverage.items.extend(capped(
        c.unimplemented_requirements.iter().map(|r| format!("- not implemented: {r}")).collect(),
        MAX_COVERAGE_ITEMS,
    ));
    coverage.items.extend(capped(
        c.tasks_without_requirement.iter().map(|t| format!("- task without requirement: {t}")).collect(),
        MAX_COVERAGE_ITEMS,
    ));
    coverage.items.extend(capped(
        c.orphan_references.iter().map(|o| format!("- reference to a missing requirement: {o}")).collect(),
        MAX_COVERAGE_ITEMS,
    ));
    all.push(coverage);

    let mut communities = Section::new("Communities", 3);
    let cm = &report.communities;
    communities.preface.push(format!(
        "{} communities of 3+ files; {} smaller groups ({} files); {} files with no structural link.",
        cm.listed.len(),
        cm.small_groups,
        cm.small_group_files,
        cm.isolated_files
    ));
    if !cm.listed.is_empty() {
        communities.table_header = vec![
            "| # | Area | Files | Cohesion | Main files |".to_string(),
            "|---:|---|---:|---:|---|".to_string(),
        ];
        let total_listed = cm.listed.len();
        communities.items = cm
            .listed
            .iter()
            .take(MAX_COMMUNITIES_LISTED)
            .map(|c| {
                format!(
                    "| {} | `{}` | {} | {:.2}{} | {} |",
                    c.id,
                    c.label,
                    c.size,
                    c.cohesion,
                    if c.fragile { " (fragile)" } else { "" },
                    c.top_files.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ")
                )
            })
            .collect();
        if total_listed > MAX_COMMUNITIES_LISTED {
            communities.preface.push(format!(
                "Showing the {MAX_COMMUNITIES_LISTED} largest of {total_listed}; the rest are in `--format json`."
            ));
        }
    }
    all.push(communities);

    let mut surprising = Section::new("Surprising Connections", 4);
    if cm.surprising.is_empty() {
        surprising.preface.push("None: no dependency crosses communities in an unexpected way.".to_string());
    }
    surprising.items = cm.surprising.iter().enumerate().map(|(i, s)| format!("{}. {}", i + 1, s.explanation)).collect();
    all.push(surprising);

    let mut cycles = Section::new("Import Cycles", 5);
    if report.import_cycles.is_empty() {
        cycles.preface.push("No import cycles.".to_string());
    } else {
        cycles.preface.push("Suggested removals are an approximation (a greedy feedback edge set), not a proven minimum.".to_string());
    }
    cycles.items = report
        .import_cycles
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let cut = c
                .suggested_removals
                .iter()
                .map(|(a, b)| format!("`{a}` -> `{b}`"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}. {} files: {} (cut: {cut})", i + 1, c.files.len(), c.files.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", "))
        })
        .collect();
    all.push(cycles);

    let mut questions = Section::new("Suggested Questions", 6);
    if report.suggested_questions.is_empty() {
        questions.preface.push("Nothing stands out.".to_string());
    }
    questions.items = report.suggested_questions.iter().enumerate().map(|(i, q)| format!("{}. {q}", i + 1)).collect();
    all.push(questions);

    all
}

/// Markdown for `report`, fitted to `max_tokens` (90% safety margin, like
/// `search --max-tokens`) when given.
pub fn to_markdown(report: &Report, max_tokens: Option<u32>) -> String {
    let mut sections = sections(report);
    if let Some(budget) = max_tokens {
        fit(&mut sections, (budget as f64 * 0.9) as u32);
    }
    let mut out = String::from("# NexSpec graph report\n\n");
    for section in &sections {
        out.push_str(&section.render());
        out.push('\n');
    }
    out.trim_end().to_string() + "\n"
}

fn estimate(sections: &[Section]) -> u32 {
    let tokenizer = CharHeuristicTokenizer;
    let text: String = sections.iter().map(Section::render).collect::<Vec<_>>().join("\n");
    tokenizer.estimate(&text) + tokenizer.estimate("# NexSpec graph report")
}

/// Shrinks the least important sections until the report fits: first halve
/// their items (down to one), then drop whole sections from the bottom. The
/// summary is never dropped.
fn fit(sections: &mut Vec<Section>, budget: u32) {
    while estimate(sections) > budget {
        // Least important section that still has more than one item.
        let candidate = sections
            .iter()
            .enumerate()
            .filter(|(_, s)| s.items.len() > 1)
            .max_by_key(|(_, s)| s.priority)
            .map(|(i, _)| i);
        if let Some(i) = candidate {
            let keep = (sections[i].items.len() / 2).max(1);
            let dropped = sections[i].items.len() - keep;
            sections[i].items.truncate(keep);
            sections[i].omitted += dropped;
            continue;
        }
        // Everything is down to a single item: drop the least important section.
        let Some(i) = sections.iter().enumerate().filter(|(_, s)| s.priority > 0).max_by_key(|(_, s)| s.priority).map(|(i, _)| i) else {
            break;
        };
        sections.remove(i);
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

/// `YYYY-MM-DD` (UTC) from unix seconds, without a date dependency.
fn format_date(unix_seconds: i64) -> String {
    let days = unix_seconds.div_euclid(86_400);
    // Civil-from-days (Howard Hinnant), valid for the whole proleptic Gregorian calendar.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::analysis::{Coverage, CycleReport, GodNode, IndexInfo, Summary};
    use crate::report::communities::{Communities, Community, Surprise};
    use std::collections::BTreeMap;

    fn sample(god_count: usize) -> Report {
        Report {
            summary: Summary {
                total_nodes: 120,
                total_edges: 300,
                nodes_by_type: BTreeMap::from([("file".to_string(), 40), ("symbol".to_string(), 80)]),
                edges_by_type: BTreeMap::from([("Imports".to_string(), 100)]),
                files_by_language: BTreeMap::from([("TypeScript".to_string(), 40)]),
                index: IndexInfo {
                    last_indexed_commit: Some("abcdef1234567890".into()),
                    last_indexed_at: Some(1_700_000_000),
                    sync_version: 4,
                    index_bytes: 3_500_000,
                },
            },
            barrel_files_excluded: 2,
            god_nodes: (0..god_count)
                .map(|i| GodNode {
                    id: format!("{i:064x}"),
                    label: format!("Node{i} (src/n{i}.ts)"),
                    kind: "symbol".into(),
                    path: Some(format!("src/n{i}.ts")),
                    degree: 100 - i,
                    dependents: 60 - i,
                    dependencies: 40,
                    has_requirement: i % 2 == 0,
                })
                .collect(),
            communities: Communities {
                listed: vec![Community { id: 1, label: "src/app".into(), size: 9, cohesion: 0.2, fragile: true, top_files: vec!["src/app/a.ts".into()], files: vec![] }],
                small_groups: 2,
                small_group_files: 4,
                isolated_files: 7,
                surprising: vec![Surprise {
                    from: "ui/a.ts".into(),
                    to: "db/b.ts".into(),
                    from_community: 1,
                    to_community: 2,
                    score: 1.0,
                    explanation: "ui/a.ts depends on db/b.ts".into(),
                }],
            },
            requirement_coverage: Coverage {
                requirements_total: 5,
                unimplemented_requirements: vec!["REQ-2".into(), "REQ-3".into()],
                tasks_without_requirement: vec!["TASK-9".into()],
                orphan_references: vec!["Stray (x.ts)".into()],
            },
            import_cycles: vec![CycleReport { files: vec!["a.ts".into(), "b.ts".into()], suggested_removals: vec![("b.ts".into(), "a.ts".into())] }],
            suggested_questions: vec!["Is it intended that ui depends on db?".into()],
        }
    }

    #[test]
    fn markdown_has_the_stable_sections_in_order() {
        let md = to_markdown(&sample(3), None);
        let order = [
            "# NexSpec graph report",
            "## Summary",
            "## God Nodes",
            "## Requirement Coverage",
            "## Communities",
            "## Surprising Connections",
            "## Import Cycles",
            "## Suggested Questions",
        ];
        let mut last = 0;
        for heading in order {
            let at = md[last..].find(heading).unwrap_or_else(|| panic!("{heading} missing or out of order in:\n{md}"));
            last += at;
        }
        assert!(md.contains("commit abcdef123456 (2023-11-14)"), "{md}");
        assert!(md.contains("3.3 MB"), "{md}");
        assert!(md.contains("| 1 | `Node0 (src/n0.ts)` | symbol | 100 | 60 | 40 | yes |"), "{md}");
        assert!(md.contains("0.20 (fragile)"), "{md}");
        assert!(md.contains("not implemented: REQ-2"));
        assert!(md.contains("2 barrel file(s)"), "{md}");
        assert!(md.contains("cut: `b.ts` -> `a.ts`"));
        assert!(md.contains("approximation"));
    }

    #[test]
    fn json_keeps_the_same_keys_as_the_sections() {
        let v: serde_json::Value = serde_json::from_str(&to_json(&sample(2))).unwrap();
        for key in ["summary", "god_nodes", "communities", "requirement_coverage", "import_cycles", "suggested_questions"] {
            assert!(v.get(key).is_some(), "{key} missing");
        }
        assert_eq!(v["god_nodes"].as_array().unwrap().len(), 2);
        assert!(v["communities"]["surprising"].is_array());
    }

    #[test]
    fn a_token_budget_shrinks_the_least_important_sections_first_and_says_so() {
        let full = to_markdown(&sample(40), None);
        let full_tokens = CharHeuristicTokenizer.estimate(&full);
        let budget = full_tokens / 3;
        let small = to_markdown(&sample(40), Some(budget));
        let small_tokens = CharHeuristicTokenizer.estimate(&small);
        assert!(small_tokens <= budget, "{small_tokens} > {budget}");
        assert!(small.contains("## Summary") && small.contains("## God Nodes"), "{small}");
        assert!(small.contains("omitted to fit the token budget"), "{small}");
        assert!(small.contains("`Node0 (src/n0.ts)`"), "the most important row survives");
    }

    #[test]
    fn a_tiny_budget_still_keeps_the_summary() {
        let tiny = to_markdown(&sample(40), Some(40));
        assert!(tiny.contains("## Summary"), "{tiny}");
        assert!(!tiny.contains("## Suggested Questions"), "{tiny}");
    }

    #[test]
    fn an_empty_index_says_to_sync_first() {
        let mut r = sample(0);
        r.summary.total_nodes = 0;
        assert!(to_markdown(&r, None).contains("run `nexspec sync` first"));
        assert!(!to_markdown(&sample(1), None).contains("index is empty"));
    }

    #[test]
    fn dates_are_formatted_in_utc() {
        assert_eq!(format_date(0), "1970-01-01");
        assert_eq!(format_date(1_700_000_000), "2023-11-14");
        assert_eq!(format_date(951_782_400), "2000-02-29");
    }

    #[test]
    fn empty_sections_say_so_instead_of_printing_bare_headings() {
        let mut r = sample(0);
        r.communities = Communities::default();
        r.import_cycles.clear();
        r.suggested_questions.clear();
        r.requirement_coverage = Coverage::default();
        let md = to_markdown(&r, None);
        assert!(md.contains("No node has structural links yet."));
        assert!(md.contains("No import cycles."));
        assert!(md.contains("Nothing stands out."));
        assert!(md.contains("None: no dependency crosses communities"));
    }
}
