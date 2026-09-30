//! `report --diff <rev>` (REQ-1011): what changed in the graph between a
//! revision and the current index. The revision is indexed read-only into a
//! throwaway directory (design.md D6), so the repository is never touched.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Serialize;

use crate::engine::{Engine, EngineError, EngineOptions};
use crate::graph::node::NodePayload;
use crate::report::communities::Community;
use crate::report::snapshot::GraphSnapshot;
use crate::report::{Report, ReportOptions};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GodChange {
    pub label: String,
    pub before: Option<usize>,
    pub after: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct GodNodeChanges {
    pub entered: Vec<GodChange>,
    pub left: Vec<GodChange>,
    pub changed: Vec<GodChange>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Merge {
    /// The community that now holds them.
    pub into: String,
    pub from: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Split {
    pub from: String,
    pub into: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct CommunityChanges {
    pub merged: Vec<Merge>,
    pub split: Vec<Split>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct CycleChanges {
    pub new: Vec<Vec<String>>,
    pub resolved: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct RequirementChanges {
    pub newly_unimplemented: Vec<String>,
    pub newly_implemented: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ReportDiff {
    /// The revision compared against (as the user wrote it).
    pub base: String,
    pub total_nodes_delta: i64,
    pub total_edges_delta: i64,
    pub nodes_delta: BTreeMap<String, i64>,
    pub edges_delta: BTreeMap<String, i64>,
    pub files_added: Vec<String>,
    pub files_removed: Vec<String>,
    pub god_nodes: GodNodeChanges,
    pub communities: CommunityChanges,
    pub import_cycles: CycleChanges,
    pub requirements: RequirementChanges,
}

/// Paths of every file node in a snapshot.
pub fn file_paths(snapshot: &GraphSnapshot) -> BTreeSet<String> {
    snapshot
        .nodes
        .values()
        .filter_map(|p| match p {
            NodePayload::File { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect()
}

fn delta(old: &BTreeMap<String, usize>, new: &BTreeMap<String, usize>) -> BTreeMap<String, i64> {
    let keys: BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    keys.into_iter()
        .filter_map(|k| {
            let d = new.get(k).copied().unwrap_or(0) as i64 - old.get(k).copied().unwrap_or(0) as i64;
            (d != 0).then(|| (k.clone(), d))
        })
        .collect()
}

/// Compares the report (and file set) of `base` with the current ones.
pub fn compare(
    base_label: &str,
    old: &Report,
    old_files: &BTreeSet<String>,
    new: &Report,
    new_files: &BTreeSet<String>,
) -> ReportDiff {
    ReportDiff {
        base: base_label.to_string(),
        total_nodes_delta: new.summary.total_nodes as i64 - old.summary.total_nodes as i64,
        total_edges_delta: new.summary.total_edges as i64 - old.summary.total_edges as i64,
        nodes_delta: delta(&old.summary.nodes_by_type, &new.summary.nodes_by_type),
        edges_delta: delta(&old.summary.edges_by_type, &new.summary.edges_by_type),
        files_added: new_files.difference(old_files).cloned().collect(),
        files_removed: old_files.difference(new_files).cloned().collect(),
        god_nodes: god_changes(old, new),
        communities: community_changes(&old.communities.listed, &new.communities.listed),
        import_cycles: cycle_changes(old, new),
        requirements: requirement_changes(old, new),
    }
}

fn god_changes(old: &Report, new: &Report) -> GodNodeChanges {
    let degrees = |r: &Report| -> BTreeMap<String, usize> { r.god_nodes.iter().map(|g| (g.label.clone(), g.degree)).collect() };
    let (before, after) = (degrees(old), degrees(new));
    let mut changes = GodNodeChanges::default();
    for (label, &degree) in &after {
        match before.get(label) {
            None => changes.entered.push(GodChange { label: label.clone(), before: None, after: Some(degree) }),
            Some(&was) if was != degree => {
                changes.changed.push(GodChange { label: label.clone(), before: Some(was), after: Some(degree) });
            }
            Some(_) => {}
        }
    }
    for (label, &degree) in &before {
        if !after.contains_key(label) {
            changes.left.push(GodChange { label: label.clone(), before: Some(degree), after: None });
        }
    }
    changes
}

fn cycle_changes(old: &Report, new: &Report) -> CycleChanges {
    let set = |r: &Report| -> BTreeSet<Vec<String>> { r.import_cycles.iter().map(|c| c.files.clone()).collect() };
    let (before, after) = (set(old), set(new));
    CycleChanges {
        new: after.difference(&before).cloned().collect(),
        resolved: before.difference(&after).cloned().collect(),
    }
}

fn requirement_changes(old: &Report, new: &Report) -> RequirementChanges {
    let before: BTreeSet<&String> = old.requirement_coverage.unimplemented_requirements.iter().collect();
    let after: BTreeSet<&String> = new.requirement_coverage.unimplemented_requirements.iter().collect();
    let still_exist_before: BTreeSet<&String> = before.clone();
    RequirementChanges {
        newly_unimplemented: after.difference(&before).map(|s| (*s).clone()).collect(),
        newly_implemented: still_exist_before.difference(&after).map(|s| (*s).clone()).collect(),
    }
}

/// Communities are matched by membership, not by id (ids are just ranks).
/// A *merge*: one new community that holds at least half of each of two or
/// more old ones. A *split*: one old community at least 30% of which ended up
/// in each of two or more new ones.
fn community_changes(old: &[Community], new: &[Community]) -> CommunityChanges {
    let overlap = |a: &Community, b: &Community| -> usize {
        let set: BTreeSet<&String> = a.files.iter().collect();
        b.files.iter().filter(|f| set.contains(f)).count()
    };
    let name = |c: &Community| format!("{} ({} files)", c.label, c.size);
    let mut changes = CommunityChanges::default();
    for n in new {
        let absorbed: Vec<String> = old
            .iter()
            .filter(|o| !o.files.is_empty() && overlap(o, n) * 2 >= o.files.len())
            .map(name)
            .collect();
        if absorbed.len() >= 2 {
            changes.merged.push(Merge { into: name(n), from: absorbed });
        }
    }
    for o in old {
        let parts: Vec<String> = new
            .iter()
            .filter(|n| !o.files.is_empty() && overlap(o, n) * 10 >= o.files.len() * 3)
            .map(name)
            .collect();
        if parts.len() >= 2 {
            changes.split.push(Split { from: name(o), into: parts });
        }
    }
    changes
}

/// Indexes `revision` in a temporary directory (the repository is only read)
/// and reports on it.
pub fn report_at_revision(
    repo_root: &Path,
    revision: &str,
    options: ReportOptions,
) -> Result<(Report, BTreeSet<String>), EngineError> {
    let scratch = tempfile::TempDir::new()?;
    let engine = Engine::open_with(
        scratch.path(),
        repo_root,
        EngineOptions { vector_search: false, revision: Some(revision.to_string()), ..EngineOptions::default() },
    )?;
    engine.sync()?;
    let snapshot = engine.snapshot()?;
    let files = file_paths(&snapshot);
    let report = crate::report::build(&snapshot, engine.index_info()?, options);
    Ok((report, files))
}

/// Markdown for a [`ReportDiff`]; lists longer than `MAX_LISTED` are cut.
pub fn to_markdown(diff: &ReportDiff) -> String {
    const MAX_LISTED: usize = 15;
    let signed = |n: i64| if n > 0 { format!("+{n}") } else { n.to_string() };
    let mut out = format!("# NexSpec graph diff against `{}`\n\n", diff.base);

    out.push_str("## Size\n\n");
    let by = |m: &BTreeMap<String, i64>| {
        if m.is_empty() {
            "no change".to_string()
        } else {
            m.iter().map(|(k, v)| format!("{k} {}", signed(*v))).collect::<Vec<_>>().join(", ")
        }
    };
    out.push_str(&format!("- Nodes: {} ({})\n", signed(diff.total_nodes_delta), by(&diff.nodes_delta)));
    out.push_str(&format!("- Edges: {} ({})\n\n", signed(diff.total_edges_delta), by(&diff.edges_delta)));

    let list = |out: &mut String, title: &str, items: &[String]| {
        if items.is_empty() {
            return;
        }
        out.push_str(&format!("## {title} ({})\n\n", items.len()));
        for item in items.iter().take(MAX_LISTED) {
            out.push_str(&format!("- `{item}`\n"));
        }
        if items.len() > MAX_LISTED {
            out.push_str(&format!("- … and {} more\n", items.len() - MAX_LISTED));
        }
        out.push('\n');
    };
    list(&mut out, "Files added", &diff.files_added);
    list(&mut out, "Files removed", &diff.files_removed);

    let g = &diff.god_nodes;
    if !(g.entered.is_empty() && g.left.is_empty() && g.changed.is_empty()) {
        out.push_str("## God Nodes\n\n");
        for c in &g.entered {
            out.push_str(&format!("- entered the top: `{}` (degree {})\n", c.label, c.after.unwrap_or(0)));
        }
        for c in &g.left {
            out.push_str(&format!("- left the top: `{}` (was {})\n", c.label, c.before.unwrap_or(0)));
        }
        for c in &g.changed {
            out.push_str(&format!("- degree changed: `{}` {} -> {}\n", c.label, c.before.unwrap_or(0), c.after.unwrap_or(0)));
        }
        out.push('\n');
    }

    let cm = &diff.communities;
    if !(cm.merged.is_empty() && cm.split.is_empty()) {
        out.push_str("## Communities\n\n");
        for m in &cm.merged {
            out.push_str(&format!("- merged into `{}`: {}\n", m.into, m.from.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ")));
        }
        for s in &cm.split {
            out.push_str(&format!("- split from `{}`: {}\n", s.from, s.into.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ")));
        }
        out.push('\n');
    }

    let cy = &diff.import_cycles;
    if !(cy.new.is_empty() && cy.resolved.is_empty()) {
        out.push_str("## Import Cycles\n\n");
        for c in &cy.new {
            out.push_str(&format!("- new cycle: {}\n", c.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(" <-> ")));
        }
        for c in &cy.resolved {
            out.push_str(&format!("- resolved: {}\n", c.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(" <-> ")));
        }
        out.push('\n');
    }

    let rq = &diff.requirements;
    if !(rq.newly_unimplemented.is_empty() && rq.newly_implemented.is_empty()) {
        out.push_str("## Requirement Coverage\n\n");
        for r in &rq.newly_implemented {
            out.push_str(&format!("- now linked to code or a task: {r}\n"));
        }
        for r in &rq.newly_unimplemented {
            out.push_str(&format!("- no longer (or newly) unlinked: {r}\n"));
        }
        out.push('\n');
    }
    out.trim_end().to_string() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::edge::EdgeType;
    use crate::report::snapshot::test_support::*;
    use crate::report::{ReportOptions, build};

    fn report_of(snap: &GraphSnapshot) -> Report {
        build(snap, Default::default(), ReportOptions::default())
    }

    /// A triangle of files under `dir` (ids base..base+2) with a cycle among them.
    fn cluster(dir: &str, base: u8) -> (Vec<(u8, NodePayloadAlias)>, Vec<crate::graph::edge::Edge>) {
        let nodes: Vec<(u8, NodePayloadAlias)> = (0..3).map(|i| (base + i, file(&format!("{dir}/f{i}.ts")))).collect();
        let mut edges = Vec::new();
        for (i, (a, b)) in [(0u8, 1u8), (1, 2), (2, 0)].iter().enumerate() {
            edges.push(edge(base as u16 * 10 + i as u16, base + a, base + b, EdgeType::Imports));
        }
        (nodes, edges)
    }
    type NodePayloadAlias = crate::graph::node::NodePayload;

    #[test]
    fn size_files_and_requirement_changes_are_reported() {
        let (mut n_old, e_old) = cluster("app", 1);
        n_old.push((50, requirement("REQ-1")));
        n_old.push((51, requirement("REQ-2")));
        let old = snapshot(n_old.clone(), e_old.clone());

        let (mut n_new, mut e_new) = (n_old.clone(), e_old.clone());
        n_new.push((60, file("app/new.ts")));
        n_new.push((61, symbol("Impl")));
        e_new.push(edge(900, 61, 50, EdgeType::Satisfies)); // REQ-1 gets an implementation
        let new = snapshot(n_new, e_new);

        let diff = compare("HEAD~1", &report_of(&old), &file_paths(&old), &report_of(&new), &file_paths(&new));
        assert_eq!(diff.base, "HEAD~1");
        assert_eq!(diff.total_nodes_delta, 2);
        assert_eq!(diff.total_edges_delta, 1);
        assert_eq!(diff.nodes_delta["file"], 1);
        assert_eq!(diff.nodes_delta["symbol"], 1);
        assert_eq!(diff.edges_delta["Satisfies"], 1);
        assert_eq!(diff.files_added, vec!["app/new.ts"]);
        assert!(diff.files_removed.is_empty());
        assert_eq!(diff.requirements.newly_implemented, vec!["REQ-1"]);
        assert!(diff.requirements.newly_unimplemented.is_empty());
    }

    #[test]
    fn a_removed_import_resolves_the_cycle_and_a_new_one_is_flagged() {
        let (nodes, edges) = cluster("app", 1);
        let old = snapshot(nodes.clone(), edges.clone());
        // Break the cycle: drop the closing edge 3 -> 1.
        let broken: Vec<_> = edges.iter().filter(|e| !(e.from == id(3) && e.to == id(1))).cloned().collect();
        let new = snapshot(nodes, broken);
        let diff = compare("main", &report_of(&old), &file_paths(&old), &report_of(&new), &file_paths(&new));
        assert_eq!(diff.import_cycles.resolved.len(), 1);
        assert!(diff.import_cycles.new.is_empty());

        let back = compare("main", &report_of(&new), &file_paths(&new), &report_of(&old), &file_paths(&old));
        assert_eq!(back.import_cycles.new.len(), 1, "the reverse comparison sees it as new");
    }

    #[test]
    fn god_node_entries_exits_and_degree_changes() {
        // `hub.ts` is imported by 3 files in the old graph and 5 in the new one; `old.ts` disappears.
        let mk = |users: u8, with_old: bool| {
            let mut nodes = vec![(1, file("hub.ts"))];
            let mut edges = Vec::new();
            for i in 0..users {
                nodes.push((10 + i, file(&format!("user{i}.ts"))));
                edges.push(edge(i as u16, 10 + i, 1, EdgeType::Imports));
            }
            if with_old {
                nodes.push((40, file("old.ts")));
                for i in 0..2u8 {
                    nodes.push((41 + i, file(&format!("o{i}.ts"))));
                    edges.push(edge(100 + i as u16, 41 + i, 40, EdgeType::Imports));
                }
            }
            snapshot(nodes, edges)
        };
        let (old, new) = (mk(3, true), mk(5, false));
        let diff = compare("v1", &report_of(&old), &file_paths(&old), &report_of(&new), &file_paths(&new));
        assert!(diff.god_nodes.changed.iter().any(|c| c.label == "hub.ts" && c.before == Some(3) && c.after == Some(5)), "{:?}", diff.god_nodes);
        assert!(diff.god_nodes.left.iter().any(|c| c.label == "old.ts"), "{:?}", diff.god_nodes);
        assert!(diff.god_nodes.entered.iter().any(|c| c.label == "user3.ts"), "{:?}", diff.god_nodes);
    }

    fn community(label: &str, files: &[&str]) -> Community {
        Community {
            id: 1,
            label: label.into(),
            size: files.len(),
            cohesion: 0.5,
            fragile: false,
            top_files: vec![],
            files: files.iter().map(|f| f.to_string()).collect(),
        }
    }

    #[test]
    fn communities_that_merge_or_split_are_recognised_by_membership() {
        let a = community("a", &["a1", "a2", "a3"]);
        let b = community("b", &["b1", "b2", "b3"]);
        let ab = community("ab", &["a1", "a2", "a3", "b1", "b2", "b3"]);
        let merged = community_changes(&[a.clone(), b.clone()], std::slice::from_ref(&ab));
        assert_eq!(merged.merged.len(), 1);
        assert_eq!(merged.merged[0].from.len(), 2);
        assert!(merged.split.is_empty());

        let split = community_changes(std::slice::from_ref(&ab), &[a.clone(), b.clone()]);
        assert_eq!(split.split.len(), 1);
        assert_eq!(split.split[0].into.len(), 2);
        assert!(split.merged.is_empty());

        let same = community_changes(&[a.clone(), b.clone()], &[a, b]);
        assert_eq!(same, CommunityChanges::default());
    }

    #[test]
    fn markdown_lists_only_what_changed() {
        let (nodes, edges) = cluster("app", 1);
        let old = snapshot(nodes.clone(), edges.clone());
        let mut more = nodes;
        more.push((70, file("app/added.ts")));
        let new = snapshot(more, edges);
        let diff = compare("HEAD~2", &report_of(&old), &file_paths(&old), &report_of(&new), &file_paths(&new));
        let md = to_markdown(&diff);
        assert!(md.starts_with("# NexSpec graph diff against `HEAD~2`"), "{md}");
        assert!(md.contains("Nodes: +1 (file +1)") && md.contains("Edges: 0 (no change)"), "{md}");
        assert!(md.contains("## Files added (1)") && md.contains("`app/added.ts`"), "{md}");
        assert!(!md.contains("## Import Cycles") && !md.contains("## Communities"), "unchanged sections stay out: {md}");
    }
}
