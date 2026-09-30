//! Report sections that are straight counting and ranking over a
//! [`GraphSnapshot`]: summary, God nodes, requirement coverage, import cycles
//! (REQ-1001, REQ-1002, REQ-1004, REQ-1010).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::Serialize;

use crate::graph::edge::{Edge, EdgeType};
use crate::graph::node::NodePayload;
use crate::report::snapshot::GraphSnapshot;
use crate::sync::mutation::StableId;

/// What the engine knows about the index itself.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct IndexInfo {
    pub last_indexed_commit: Option<String>,
    /// Unix seconds of the last indexed commit.
    pub last_indexed_at: Option<i64>,
    pub sync_version: u64,
    pub index_bytes: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Summary {
    pub total_nodes: usize,
    pub total_edges: usize,
    pub nodes_by_type: BTreeMap<String, usize>,
    pub edges_by_type: BTreeMap<String, usize>,
    pub files_by_language: BTreeMap<String, usize>,
    pub index: IndexInfo,
}

pub fn summary(snapshot: &GraphSnapshot, index: IndexInfo) -> Summary {
    let mut nodes_by_type: BTreeMap<String, usize> = BTreeMap::new();
    let mut files_by_language: BTreeMap<String, usize> = BTreeMap::new();
    for (id, payload) in &snapshot.nodes {
        *nodes_by_type.entry(snapshot.kind_name(id).to_string()).or_default() += 1;
        if let NodePayload::File { path, .. } = payload {
            *files_by_language.entry(language_of(path)).or_default() += 1;
        }
    }
    let mut edges_by_type: BTreeMap<String, usize> = BTreeMap::new();
    for edge in &snapshot.edges {
        *edges_by_type.entry(format!("{:?}", edge.edge_type)).or_default() += 1;
    }
    Summary {
        total_nodes: snapshot.nodes.len(),
        total_edges: snapshot.edges.len(),
        nodes_by_type,
        edges_by_type,
        files_by_language,
        index,
    }
}

fn language_of(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()) {
        Some(ext) => match ext.as_str() {
            "ts" | "tsx" | "mts" | "cts" => "TypeScript",
            "js" | "jsx" | "mjs" | "cjs" => "JavaScript",
            "rs" => "Rust",
            "py" => "Python",
            "go" => "Go",
            "md" | "mdx" => "Markdown",
            "json" | "yaml" | "yml" | "toml" => "Config",
            "xml" | "html" | "css" | "scss" => "Markup/Style",
            "sql" => "SQL",
            "sh" | "ps1" | "bat" => "Shell",
            _ => "Other",
        },
        None => "Other",
    }
    .to_string()
}

/// Edge types that count towards a node's degree: real structure, not
/// "defined in" bookkeeping and not co-change noise (Q2 in the spec).
fn counts_for_degree(edge_type: EdgeType) -> bool {
    matches!(edge_type, EdgeType::Satisfies | EdgeType::Implements) || edge_type.is_dependency()
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GodNode {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub path: Option<String>,
    pub degree: usize,
    /// Direct dependents (edges pointing at it).
    pub dependents: usize,
    pub dependencies: usize,
    /// Whether it is linked to a requirement (a symbol via `Satisfies`, a
    /// file if any of its symbols is).
    pub has_requirement: bool,
}

/// Files that mostly re-export (`index.ts` barrels): every import of the
/// package funnels through them, so their degree says nothing about design.
pub fn barrel_files(snapshot: &GraphSnapshot) -> HashSet<StableId> {
    let mut total: HashMap<StableId, usize> = HashMap::new();
    let mut reexports: HashMap<StableId, usize> = HashMap::new();
    for edge in snapshot.edges.iter().filter(|e| e.edge_type.is_dependency()) {
        if !matches!(snapshot.nodes.get(&edge.from), Some(NodePayload::File { .. })) {
            continue;
        }
        *total.entry(edge.from).or_default() += 1;
        if edge.edge_type == EdgeType::ReExports {
            *reexports.entry(edge.from).or_default() += 1;
        }
    }
    reexports
        .into_iter()
        .filter(|(id, count)| *count >= 2 && *count * 2 >= total.get(id).copied().unwrap_or(0))
        .map(|(id, _)| id)
        .collect()
}

pub fn god_nodes(snapshot: &GraphSnapshot, top: usize) -> Vec<GodNode> {
    let barrels = barrel_files(snapshot);
    let mut incoming: HashMap<StableId, usize> = HashMap::new();
    let mut outgoing: HashMap<StableId, usize> = HashMap::new();
    for edge in snapshot.edges.iter().filter(|e| counts_for_degree(e.edge_type)) {
        if snapshot.nodes.contains_key(&edge.to) {
            *incoming.entry(edge.to).or_default() += 1;
        }
        if snapshot.nodes.contains_key(&edge.from) {
            *outgoing.entry(edge.from).or_default() += 1;
        }
    }
    let with_requirement = nodes_with_requirement(snapshot);
    let mut ranked: Vec<(usize, StableId)> = snapshot
        .nodes
        .keys()
        // Requirements and tasks are documentation, not code structure.
        .filter(|id| matches!(snapshot.nodes.get(*id), Some(NodePayload::File { .. } | NodePayload::Symbol { .. })))
        .filter(|id| !barrels.contains(*id))
        .map(|id| (incoming.get(id).copied().unwrap_or(0) + outgoing.get(id).copied().unwrap_or(0), *id))
        .filter(|(degree, _)| *degree > 0)
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| snapshot.label(&a.1).cmp(&snapshot.label(&b.1))));
    ranked
        .into_iter()
        .take(top)
        .map(|(degree, id)| GodNode {
            id: crate::engine::id_hex(&id),
            label: snapshot.label(&id),
            kind: snapshot.kind_name(&id).to_string(),
            path: snapshot.path_of(&id).map(str::to_string),
            degree,
            dependents: incoming.get(&id).copied().unwrap_or(0),
            dependencies: outgoing.get(&id).copied().unwrap_or(0),
            has_requirement: with_requirement.contains(&id),
        })
        .collect()
}

/// Symbols with a `Satisfies` edge to an existing requirement, and the files
/// containing such symbols.
fn nodes_with_requirement(snapshot: &GraphSnapshot) -> HashSet<StableId> {
    let mut out = HashSet::new();
    for edge in snapshot.edges.iter().filter(|e| e.edge_type == EdgeType::Satisfies) {
        if matches!(snapshot.nodes.get(&edge.to), Some(NodePayload::Requirement { .. }))
            && matches!(snapshot.nodes.get(&edge.from), Some(NodePayload::Symbol { .. }))
        {
            out.insert(edge.from);
            if let Some(file) = snapshot.file_of.get(&edge.from) {
                out.insert(*file);
            }
        }
    }
    out
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Coverage {
    pub requirements_total: usize,
    /// Requirements nothing (no symbol, no task) claims to satisfy.
    pub unimplemented_requirements: Vec<String>,
    /// Tasks that do not link to any existing requirement.
    pub tasks_without_requirement: Vec<String>,
    /// Sources whose `Satisfies` target does not exist (`@spec REQ-x` with no such REQ).
    pub orphan_references: Vec<String>,
}

pub fn coverage(snapshot: &GraphSnapshot) -> Coverage {
    let mut satisfied: HashSet<StableId> = HashSet::new();
    let mut tasks_with_requirement: HashSet<StableId> = HashSet::new();
    let mut orphans: BTreeSet<String> = BTreeSet::new();
    for edge in snapshot.edges.iter().filter(|e| e.edge_type == EdgeType::Satisfies) {
        match snapshot.nodes.get(&edge.to) {
            Some(NodePayload::Requirement { .. }) => {
                let source = snapshot.nodes.get(&edge.from);
                if matches!(source, Some(NodePayload::Symbol { .. } | NodePayload::Task { .. })) {
                    satisfied.insert(edge.to);
                    if matches!(source, Some(NodePayload::Task { .. })) {
                        tasks_with_requirement.insert(edge.from);
                    }
                }
            }
            Some(_) => {}
            None => {
                if snapshot.nodes.contains_key(&edge.from) {
                    orphans.insert(snapshot.label(&edge.from));
                }
            }
        }
    }
    let mut requirements: Vec<String> = Vec::new();
    let mut unimplemented: Vec<String> = Vec::new();
    let mut tasks: Vec<String> = Vec::new();
    for (id, payload) in &snapshot.nodes {
        match payload {
            NodePayload::Requirement { title, .. } => {
                requirements.push(title.clone());
                if !satisfied.contains(id) {
                    unimplemented.push(title.clone());
                }
            }
            NodePayload::Task { title, .. } if !tasks_with_requirement.contains(id) => tasks.push(title.clone()),
            _ => {}
        }
    }
    unimplemented.sort();
    tasks.sort();
    Coverage {
        requirements_total: requirements.len(),
        unimplemented_requirements: unimplemented,
        tasks_without_requirement: tasks,
        orphan_references: orphans.into_iter().collect(),
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CycleReport {
    /// Files of the cycle, sorted.
    pub files: Vec<String>,
    /// Import edges whose removal breaks the cycle (an approximation: a
    /// greedy feedback edge set, not guaranteed minimal).
    pub suggested_removals: Vec<(String, String)>,
}

pub fn import_cycle_reports(snapshot: &GraphSnapshot) -> Vec<CycleReport> {
    let groups = crate::graph::cycles::import_cycles(&snapshot.edges);
    groups
        .into_iter()
        .map(|group| {
            let mut files: Vec<(String, StableId)> = group.iter().map(|id| (snapshot.label(id), *id)).collect();
            files.sort();
            let members: HashSet<StableId> = group.iter().copied().collect();
            let removals = feedback_edges(&files, &snapshot.edges, &members);
            CycleReport {
                files: files.iter().map(|(label, _)| label.clone()).collect(),
                suggested_removals: removals
                    .into_iter()
                    .map(|(from, to)| (snapshot.label(&from), snapshot.label(&to)))
                    .collect(),
            }
        })
        .collect()
}

/// Back edges of a depth-first search over one strongly connected component,
/// visiting files in label order: removing them leaves the component acyclic.
fn feedback_edges(files: &[(String, StableId)], edges: &[Edge], members: &HashSet<StableId>) -> Vec<(StableId, StableId)> {
    let label_of: HashMap<StableId, &str> = files.iter().map(|(label, id)| (*id, label.as_str())).collect();
    let mut adjacency: HashMap<StableId, Vec<StableId>> = HashMap::new();
    for edge in edges.iter().filter(|e| e.edge_type == EdgeType::Imports && e.from != e.to) {
        if members.contains(&edge.from) && members.contains(&edge.to) {
            adjacency.entry(edge.from).or_default().push(edge.to);
        }
    }
    for targets in adjacency.values_mut() {
        targets.sort_by_key(|t| label_of.get(t).copied().unwrap_or(""));
        targets.dedup();
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Unvisited,
        OnPath,
        Done,
    }
    let mut marks: HashMap<StableId, Mark> = members.iter().map(|id| (*id, Mark::Unvisited)).collect();
    let mut back_edges = Vec::new();
    for (_, start) in files {
        if marks[start] != Mark::Unvisited {
            continue;
        }
        // (node, next child index)
        let mut stack: Vec<(StableId, usize)> = vec![(*start, 0)];
        marks.insert(*start, Mark::OnPath);
        while let Some((node, next)) = stack.last().copied() {
            let children = adjacency.get(&node).map(Vec::as_slice).unwrap_or(&[]);
            if next < children.len() {
                stack.last_mut().expect("non-empty").1 += 1;
                let child = children[next];
                match marks[&child] {
                    Mark::Unvisited => {
                        marks.insert(child, Mark::OnPath);
                        stack.push((child, 0));
                    }
                    Mark::OnPath => back_edges.push((node, child)),
                    Mark::Done => {}
                }
            } else {
                marks.insert(node, Mark::Done);
                stack.pop();
            }
        }
    }
    back_edges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::snapshot::test_support::*;

    #[test]
    fn summary_counts_nodes_edges_and_languages() {
        let snap = snapshot(
            vec![(1, file("src/a.ts")), (2, file("src/b.rs")), (3, file("README.md")), (4, symbol("A")), (5, requirement("REQ-1"))],
            vec![edge(1, 4, 1, EdgeType::DefinedIn), edge(2, 1, 2, EdgeType::Imports), edge(3, 4, 5, EdgeType::Satisfies)],
        );
        let s = summary(&snap, IndexInfo::default());
        assert_eq!((s.total_nodes, s.total_edges), (5, 3));
        assert_eq!(s.nodes_by_type["file"], 3);
        assert_eq!(s.nodes_by_type["symbol"], 1);
        assert_eq!(s.nodes_by_type["requirement"], 1);
        assert_eq!(s.edges_by_type["Imports"], 1);
        assert_eq!(s.edges_by_type["DefinedIn"], 1);
        assert_eq!(s.files_by_language["TypeScript"], 1);
        assert_eq!(s.files_by_language["Rust"], 1);
        assert_eq!(s.files_by_language["Markdown"], 1);
    }

    #[test]
    fn god_nodes_rank_by_structural_degree_and_ignore_co_change_and_defined_in() {
        // File 1 is imported by 2, 3, 4; file 5 only by 2. CoChanges/DefinedIn must not count.
        let snap = snapshot(
            vec![(1, file("hub.ts")), (2, file("a.ts")), (3, file("b.ts")), (4, file("c.ts")), (5, file("leaf.ts")), (6, symbol("Hub"))],
            vec![
                edge(1, 2, 1, EdgeType::Imports),
                edge(2, 3, 1, EdgeType::Imports),
                edge(3, 4, 1, EdgeType::Imports),
                edge(4, 2, 5, EdgeType::Imports),
                edge(5, 5, 1, EdgeType::CoChanges),
                edge(6, 5, 1, EdgeType::CoChanges),
                edge(7, 6, 1, EdgeType::DefinedIn),
            ],
        );
        let gods = god_nodes(&snap, 2);
        assert_eq!(gods.len(), 2);
        assert_eq!(gods[0].label, "hub.ts");
        assert_eq!((gods[0].degree, gods[0].dependents, gods[0].dependencies), (3, 3, 0));
        assert_eq!(gods[1].label, "a.ts", "a.ts has 1 out + 1 out: degree 2, ahead of leaf.ts's 1");
        assert_eq!(gods[1].degree, 2);
        assert!(god_nodes(&snap, 1).len() == 1);
    }

    #[test]
    fn barrel_files_are_not_god_nodes() {
        // index.ts re-exports two modules and is imported by five files.
        let mut nodes = vec![(1, file("pkg/index.ts")), (2, file("pkg/a.ts")), (3, file("pkg/b.ts")), (9, file("real.ts"))];
        let mut edges = vec![edge(1, 1, 2, EdgeType::ReExports), edge(2, 1, 3, EdgeType::ReExports)];
        for (i, n) in (10u8..15).enumerate() {
            nodes.push((n, file(&format!("user{n}.ts"))));
            edges.push(edge(100 + i as u16, n, 1, EdgeType::Imports));
        }
        for (i, n) in (20u8..23).enumerate() {
            nodes.push((n, file(&format!("caller{n}.ts"))));
            edges.push(edge(200 + i as u16, n, 9, EdgeType::Imports));
        }
        let snap = snapshot(nodes, edges);
        assert!(barrel_files(&snap).contains(&id(1)));
        let gods = god_nodes(&snap, 3);
        assert_eq!(gods[0].label, "real.ts", "the barrel (degree 7) is skipped: {gods:?}");
        assert!(gods.iter().all(|g| g.label != "pkg/index.ts"));
    }

    #[test]
    fn god_nodes_report_requirement_links_for_symbols_and_their_files() {
        let snap = snapshot(
            vec![(1, file("a.ts")), (2, symbol("A")), (3, requirement("REQ-1")), (4, file("b.ts")), (5, symbol("B"))],
            vec![
                edge(1, 2, 1, EdgeType::DefinedIn),
                edge(2, 2, 3, EdgeType::Satisfies),
                edge(3, 5, 4, EdgeType::DefinedIn),
                edge(4, 5, 2, EdgeType::Calls),
                edge(5, 4, 1, EdgeType::Imports),
            ],
        );
        let gods = god_nodes(&snap, 10);
        let by_label = |l: &str| gods.iter().find(|g| g.label == l).unwrap_or_else(|| panic!("{l}"));
        assert!(by_label("A (a.ts)").has_requirement);
        assert!(by_label("a.ts").has_requirement, "a file containing a satisfying symbol counts");
        assert!(!by_label("B (b.ts)").has_requirement);
        assert!(!by_label("b.ts").has_requirement);
    }

    #[test]
    fn coverage_finds_unimplemented_requirements_loose_tasks_and_orphans() {
        let snap = snapshot(
            vec![
                (1, requirement("REQ-1")), // implemented by a symbol
                (2, requirement("REQ-2")), // claimed by a task only
                (3, requirement("REQ-3")), // nobody
                (4, symbol("Impl")),
                (5, task("TASK-1")), // -> REQ-2
                (6, task("TASK-2")), // -> nothing
                (7, symbol("Stray")), // @spec to a missing REQ
            ],
            vec![
                edge(1, 4, 1, EdgeType::Satisfies),
                edge(2, 5, 2, EdgeType::Satisfies),
                edge(3, 7, 99, EdgeType::Satisfies),
            ],
        );
        let c = coverage(&snap);
        assert_eq!(c.requirements_total, 3);
        assert_eq!(c.unimplemented_requirements, vec!["REQ-3"]);
        assert_eq!(c.tasks_without_requirement, vec!["TASK-2"]);
        assert_eq!(c.orphan_references, vec!["Stray"]);
    }

    #[test]
    fn a_requirement_satisfied_by_a_task_counts_as_claimed() {
        let snap = snapshot(vec![(1, requirement("REQ-1")), (2, task("TASK-1"))], vec![edge(1, 2, 1, EdgeType::Satisfies)]);
        assert!(coverage(&snap).unimplemented_requirements.is_empty());
    }

    #[test]
    fn cycle_reports_list_files_and_a_small_set_of_edges_that_break_them() {
        // a -> b -> c -> a, plus b -> a (two back edges depending on order).
        let snap = snapshot(
            vec![(1, file("a.ts")), (2, file("b.ts")), (3, file("c.ts")), (4, file("free.ts"))],
            vec![
                edge(1, 1, 2, EdgeType::Imports),
                edge(2, 2, 3, EdgeType::Imports),
                edge(3, 3, 1, EdgeType::Imports),
                edge(4, 3, 4, EdgeType::Imports),
            ],
        );
        let cycles = import_cycle_reports(&snap);
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].files, vec!["a.ts", "b.ts", "c.ts"]);
        assert_eq!(cycles[0].suggested_removals, vec![("c.ts".to_string(), "a.ts".to_string())]);
    }

    #[test]
    fn removing_the_suggested_edges_really_breaks_every_cycle() {
        let snap = snapshot(
            vec![(1, file("a.ts")), (2, file("b.ts")), (3, file("c.ts")), (4, file("d.ts"))],
            vec![
                edge(1, 1, 2, EdgeType::Imports),
                edge(2, 2, 1, EdgeType::Imports),
                edge(3, 2, 3, EdgeType::Imports),
                edge(4, 3, 4, EdgeType::Imports),
                edge(5, 4, 2, EdgeType::Imports),
            ],
        );
        let reports = import_cycle_reports(&snap);
        assert!(!reports.is_empty());
        let label_to_id: HashMap<String, StableId> = snap.nodes.keys().map(|id| (snap.label(id), *id)).collect();
        let removed: HashSet<(StableId, StableId)> = reports
            .iter()
            .flat_map(|r| r.suggested_removals.iter())
            .map(|(f, t)| (label_to_id[f], label_to_id[t]))
            .collect();
        let remaining: Vec<Edge> = snap.edges.iter().filter(|e| !removed.contains(&(e.from, e.to))).cloned().collect();
        assert!(crate::graph::cycles::import_cycles(&remaining).is_empty(), "no cycle survives: removed {removed:?}");
    }
}
