//! `affected` (REQ-1104): who depends on a node, transitively, filtered by
//! relation, confidence and context, bounded per level, grouped by file.

use std::collections::{BTreeMap, HashSet};

use serde::Serialize;

use crate::graph::edge::{Confidence, EdgeType};
use crate::query::filter::{EdgeFilter, context_name, relation_name};
use crate::query::view::GraphView;
use crate::report::communities::Communities;
use crate::sync::mutation::StableId;

#[derive(Debug, Clone)]
pub struct AffectedOptions {
    pub depth: u8,
    /// Most new nodes kept per level; the rest are counted.
    pub max_per_hop: usize,
    pub filter: EdgeFilter,
}

impl Default for AffectedOptions {
    fn default() -> Self {
        Self { depth: 2, max_per_hop: 25, filter: EdgeFilter::default() }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AffectedNode {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub path: Option<String>,
    pub depth: u8,
    /// The edge that makes this node depend on its parent in the traversal.
    pub relation: String,
    pub confidence: String,
    pub context: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AffectedResult {
    pub target: String,
    pub nodes: Vec<AffectedNode>,
    /// Nodes found but not listed, in total and per level.
    pub omitted: usize,
    pub omitted_by_depth: Vec<(u8, usize)>,
}

pub fn affected(view: &GraphView, target: &StableId, options: &AffectedOptions) -> AffectedResult {
    let mut visited: HashSet<StableId> = HashSet::from([*target]);
    let mut frontier = vec![*target];
    let mut nodes = Vec::new();
    let mut omitted_by_depth: Vec<(u8, usize)> = Vec::new();

    for depth in 1..=options.depth {
        if frontier.is_empty() {
            break;
        }
        // (node, edge type, meta), first sighting wins.
        let mut candidates: Vec<(StableId, EdgeType, u8)> = Vec::new();
        let mut seen: HashSet<StableId> = HashSet::new();
        for node in &frontier {
            for edge in view.in_edges(node).filter(|e| options.filter.allows(e)) {
                if view.contains(&edge.from) && !visited.contains(&edge.from) && seen.insert(edge.from) {
                    candidates.push((edge.from, edge.edge_type, edge.meta));
                }
            }
        }
        candidates.sort_by(|a, b| {
            (a.2 & 1, (a.2 >> 1) & 0b11, view.snapshot.label(&a.0), a.0).cmp(&(b.2 & 1, (b.2 >> 1) & 0b11, view.snapshot.label(&b.0), b.0))
        });
        let mut next = Vec::new();
        let mut dropped = 0usize;
        for (index, (id, edge_type, meta)) in candidates.into_iter().enumerate() {
            if index >= options.max_per_hop {
                dropped += 1;
                continue;
            }
            visited.insert(id);
            next.push(id);
            let (confidence, context) = crate::graph::edge::decode_meta(meta);
            nodes.push(AffectedNode {
                id: crate::engine::id_hex(&id),
                label: view.snapshot.label(&id),
                kind: view.snapshot.kind_name(&id).to_string(),
                path: view.snapshot.path_of(&id).map(str::to_string),
                depth,
                relation: relation_name(edge_type).to_string(),
                confidence: match confidence {
                    Confidence::Extracted => "extracted",
                    Confidence::Inferred => "inferred",
                }
                .to_string(),
                context: context_name(context).to_string(),
            });
        }
        if dropped > 0 {
            omitted_by_depth.push((depth, dropped));
        }
        frontier = next;
    }
    let omitted = omitted_by_depth.iter().map(|(_, n)| n).sum();
    AffectedResult { target: crate::engine::id_hex(target), nodes, omitted, omitted_by_depth }
}

/// Markdown lines: nodes grouped by file (and the file's community when
/// known), files in path order.
pub fn to_lines(view: &GraphView, target: &StableId, result: &AffectedResult, communities: Option<&Communities>) -> Vec<String> {
    let community_of = |path: &str| -> Option<String> {
        communities?.listed.iter().find(|c| c.files.iter().any(|f| f == path)).map(|c| format!("{} (community {})", c.label, c.id))
    };
    let mut lines = vec![format!(
        "# Affected by `{}`: {} node{} within depth {}",
        view.snapshot.label(target),
        result.nodes.len(),
        if result.nodes.len() == 1 { "" } else { "s" },
        result.nodes.iter().map(|n| n.depth).max().unwrap_or(0).max(1)
    )];
    if result.nodes.is_empty() {
        lines.push("Nothing depends on it (with these filters).".to_string());
        return lines;
    }
    let mut by_file: BTreeMap<String, Vec<&AffectedNode>> = BTreeMap::new();
    for node in &result.nodes {
        by_file.entry(node.path.clone().unwrap_or_else(|| "(no file)".to_string())).or_default().push(node);
    }
    for (file, members) in by_file {
        let community = community_of(&file).map(|c| format!(" — {c}")).unwrap_or_default();
        lines.push(String::new());
        lines.push(format!("## {file}{community}"));
        for node in members {
            let mut flags: Vec<&str> = Vec::new();
            if node.confidence == "inferred" {
                flags.push("inferred");
            }
            if node.context != "runtime" {
                flags.push(node.context.as_str());
            }
            let flags = if flags.is_empty() { String::new() } else { format!(" [{}]", flags.join(", ")) };
            lines.push(format!("- depth {} `{}` ({}){flags}", node.depth, node.label, node.relation));
            if let Some(notes) = crate::search::unhex(&node.id).and_then(|id| crate::query::notes::inline(view, &id)) {
                lines.push(format!("  notes: {notes}"));
            }
        }
    }
    for (depth, count) in &result.omitted_by_depth {
        lines.push(String::new());
        lines.push(format!("+{count} omitted at depth {depth} (raise --depth/--limit or narrow with --relation)"));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::edge::{Edge, EdgeContext, encode_meta};
    use crate::report::snapshot::test_support::*;

    fn with_meta(mut e: Edge, confidence: Confidence, context: EdgeContext) -> Edge {
        e.meta = encode_meta(confidence, context);
        e
    }

    /// core <- a, b (files); a <- c; b <- c (diamond); core <- spec (test context).
    fn diamond() -> GraphView {
        GraphView::new(snapshot(
            vec![(1, file("core.ts")), (2, file("a.ts")), (3, file("b.ts")), (4, file("c.ts")), (5, file("core.spec.ts"))],
            vec![
                edge(1, 2, 1, EdgeType::Imports),
                edge(2, 3, 1, EdgeType::Calls),
                edge(3, 4, 2, EdgeType::Imports),
                edge(4, 4, 3, EdgeType::Imports),
                with_meta(edge(5, 5, 1, EdgeType::Imports), Confidence::Extracted, EdgeContext::Spec),
            ],
        ))
    }

    #[test]
    fn reverse_traversal_respects_depth_and_visits_each_node_once() {
        let view = diamond();
        let depth1 = affected(&view, &id(1), &AffectedOptions { depth: 1, ..Default::default() });
        let names: Vec<&str> = depth1.nodes.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(names, vec!["a.ts", "b.ts", "core.spec.ts"].into_iter().filter(|n| names.contains(n)).collect::<Vec<_>>());
        assert_eq!(depth1.nodes.len(), 3);
        assert!(depth1.nodes.iter().all(|n| n.depth == 1));

        let depth2 = affected(&view, &id(1), &AffectedOptions::default());
        let c: Vec<&AffectedNode> = depth2.nodes.iter().filter(|n| n.label == "c.ts").collect();
        assert_eq!(c.len(), 1, "c.ts is reachable two ways but listed once");
        assert_eq!(c[0].depth, 2);
        assert_eq!(depth2.omitted, 0);
    }

    #[test]
    fn relation_and_context_filters_narrow_the_result() {
        let view = diamond();
        let only_calls = EdgeFilter::from_strings(&["calls".to_string()], None, &[]).unwrap();
        let r = affected(&view, &id(1), &AffectedOptions { filter: only_calls, ..Default::default() });
        assert_eq!(r.nodes.iter().map(|n| n.label.as_str()).collect::<Vec<_>>(), vec!["b.ts"]);

        let runtime_only = EdgeFilter::from_strings(&[], None, &["runtime".to_string()]).unwrap();
        let r = affected(&view, &id(1), &AffectedOptions { filter: runtime_only, ..Default::default() });
        assert!(r.nodes.iter().all(|n| n.label != "core.spec.ts"), "spec context filtered out");
    }

    #[test]
    fn trusted_and_runtime_dependents_survive_the_per_hop_cap() {
        let mut nodes = vec![(1, file("target.ts")), (2, file("real.ts"))];
        let mut edges = vec![edge(0, 2, 1, EdgeType::Imports)];
        for i in 0..5u8 {
            nodes.push((10 + i, file(&format!("t{i}.spec.ts"))));
            edges.push(with_meta(edge(10 + i as u16, 10 + i, 1, EdgeType::Imports), Confidence::Inferred, EdgeContext::Spec));
        }
        let view = GraphView::new(snapshot(nodes, edges));
        let r = affected(&view, &id(1), &AffectedOptions { depth: 1, max_per_hop: 1, ..Default::default() });
        assert_eq!(r.nodes.len(), 1);
        assert_eq!(r.nodes[0].label, "real.ts");
        assert_eq!(r.omitted, 5);
        assert_eq!(r.omitted_by_depth, vec![(1, 5)]);
    }

    #[test]
    fn output_is_grouped_by_file_with_flags_and_omissions() {
        let view = diamond();
        let result = affected(&view, &id(1), &AffectedOptions::default());
        let lines = to_lines(&view, &id(1), &result, None).join("\n");
        assert!(lines.starts_with("# Affected by `core.ts`: 4 nodes"), "{lines}");
        assert!(lines.contains("## a.ts") && lines.contains("## core.spec.ts"), "{lines}");
        assert!(lines.contains("`core.spec.ts` (imports) [spec]"), "{lines}");
        assert!(lines.contains("depth 2 `c.ts`"), "{lines}");
    }

    #[test]
    fn a_node_nothing_depends_on_says_so() {
        let view = diamond();
        let result = affected(&view, &id(4), &AffectedOptions::default());
        assert!(result.nodes.is_empty());
        assert!(to_lines(&view, &id(4), &result, None).join("\n").contains("Nothing depends on it"));
    }
}
