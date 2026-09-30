//! `query` (REQ-1101): grow a set of seed nodes (found by the hybrid search)
//! through the graph, breadth-first by default or depth-first, and print what
//! was reached with the relation that led there.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::graph::edge::{Confidence, EdgeContext, EdgeType};
use crate::query::filter::{EdgeFilter, context_name, relation_name};
use crate::query::path::neighbours;
use crate::query::view::GraphView;
use crate::sync::mutation::StableId;

#[derive(Debug, Clone)]
pub struct ExpandOptions {
    /// Depth-first instead of breadth-first.
    pub dfs: bool,
    pub max_depth: u8,
    /// Hard stop on listed nodes, before any token budget applies.
    pub max_nodes: usize,
    pub filter: EdgeFilter,
}

impl Default for ExpandOptions {
    fn default() -> Self {
        Self { dfs: false, max_depth: 3, max_nodes: 60, filter: EdgeFilter::default() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reached {
    pub id: StableId,
    pub depth: u8,
    pub parent: StableId,
    pub edge_type: EdgeType,
    /// `true` when the edge points `parent -> id`.
    pub forward: bool,
    pub meta: u8,
}

pub fn expand(view: &GraphView, seeds: &[StableId], options: &ExpandOptions) -> Vec<Reached> {
    let mut visited: HashSet<StableId> = seeds.iter().copied().collect();
    let mut reached: Vec<Reached> = Vec::new();
    // (node, depth)
    let mut queue: VecDeque<(StableId, u8)> = seeds.iter().map(|s| (*s, 0)).collect();
    while let Some((node, depth)) = if options.dfs { queue.pop_back() } else { queue.pop_front() } {
        if depth >= options.max_depth {
            continue;
        }
        let steps: Vec<_> = neighbours(view, &node, &options.filter)
            .into_iter()
            .filter(|(next, _, _)| !visited.contains(next))
            .collect();
        // For DFS the stack pops from the back, so push in reverse to explore the best first.
        let ordered: Vec<_> = if options.dfs { steps.into_iter().rev().collect() } else { steps };
        for (next, edge, forward) in ordered {
            if reached.len() >= options.max_nodes {
                return reached;
            }
            if !visited.insert(next) {
                continue;
            }
            reached.push(Reached { id: next, depth: depth + 1, parent: node, edge_type: edge.edge_type, forward, meta: edge.meta });
            queue.push_back((next, depth + 1));
        }
    }
    reached
}

fn flags(meta: u8) -> String {
    let (confidence, context) = crate::graph::edge::decode_meta(meta);
    let mut parts: Vec<&str> = Vec::new();
    if confidence == Confidence::Inferred {
        parts.push("inferred");
    }
    if context != EdgeContext::Runtime {
        parts.push(context_name(context));
    }
    if parts.is_empty() { String::new() } else { format!(" [{}]", parts.join(", ")) }
}

/// Markdown lines for a query result. `snippets` holds pruned source for seeds.
pub fn to_lines(
    view: &GraphView,
    question: &str,
    seeds: &[StableId],
    reached: &[Reached],
    snippets: &HashMap<StableId, String>,
) -> Vec<String> {
    let mut lines = vec![format!("# Query: {question}")];
    if seeds.is_empty() {
        lines.push("No starting points were found for this question.".to_string());
        return lines;
    }
    lines.push(String::new());
    lines.push("## Starting points".to_string());
    for seed in seeds {
        lines.push(format!("- `{}` ({})", view.snapshot.label(seed), view.snapshot.kind_name(seed)));
        if let Some(notes) = crate::query::notes::inline(view, seed) {
            lines.push(format!("  notes: {notes}"));
        }
        if let Some(snippet) = snippets.get(seed) {
            lines.push("  ```".to_string());
            lines.extend(snippet.lines().map(|l| format!("  {l}")));
            lines.push("  ```".to_string());
        }
    }
    if reached.is_empty() {
        lines.push(String::new());
        lines.push("Nothing else is connected to them (with these filters).".to_string());
        return lines;
    }
    lines.push(String::new());
    lines.push("## Connected".to_string());
    for r in reached {
        let relation = relation_name(r.edge_type);
        let arrow = if r.forward { format!("--{relation}-->") } else { format!("<--{relation}--") };
        lines.push(format!(
            "- d{} `{}` {arrow} `{}`{}",
            r.depth,
            view.snapshot.label(&r.parent),
            view.snapshot.label(&r.id),
            flags(r.meta)
        ));
        if let Some(notes) = crate::query::notes::inline(view, &r.id) {
            lines.push(format!("  notes: {notes}"));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::snapshot::test_support::*;

    /// seed 1 -> 2 -> 4, 1 -> 3 -> 5, and 6 -> 1 (incoming), 7 isolated.
    fn view() -> GraphView {
        GraphView::new(snapshot(
            (1..=7).map(|n| (n, file(&format!("f{n}.ts")))).collect(),
            vec![
                edge(1, 1, 2, EdgeType::Imports),
                edge(2, 2, 4, EdgeType::Imports),
                edge(3, 1, 3, EdgeType::Calls),
                edge(4, 3, 5, EdgeType::Calls),
                edge(5, 6, 1, EdgeType::Imports),
            ],
        ))
    }

    fn ids(reached: &[Reached]) -> Vec<StableId> {
        reached.iter().map(|r| r.id).collect()
    }

    #[test]
    fn breadth_first_lists_nearer_nodes_first_in_both_directions() {
        let reached = expand(&view(), &[id(1)], &ExpandOptions::default());
        assert_eq!(reached.len(), 5);
        let depths: Vec<u8> = reached.iter().map(|r| r.depth).collect();
        assert!(depths.windows(2).all(|w| w[0] <= w[1]), "BFS order is by depth: {depths:?}");
        assert!(reached.iter().any(|r| r.id == id(6) && !r.forward), "the importer is reached against the edge");
        assert!(!ids(&reached).contains(&id(7)), "isolated nodes are not reached");
    }

    #[test]
    fn depth_and_node_limits_stop_the_walk() {
        let shallow = expand(&view(), &[id(1)], &ExpandOptions { max_depth: 1, ..Default::default() });
        assert!(shallow.iter().all(|r| r.depth == 1));
        assert_eq!(shallow.len(), 3, "2, 3 and 6");
        let few = expand(&view(), &[id(1)], &ExpandOptions { max_nodes: 2, ..Default::default() });
        assert_eq!(few.len(), 2);
    }

    #[test]
    fn depth_first_follows_one_branch_to_its_end_before_the_next() {
        let reached = expand(&view(), &[id(1)], &ExpandOptions { dfs: true, ..Default::default() });
        let order = ids(&reached);
        let pos = |n: u8| order.iter().position(|i| *i == id(n)).unwrap();
        // Whichever branch comes first, its depth-2 node follows its depth-1 node immediately.
        let branch_a = pos(4) == pos(2) + 1;
        let branch_b = pos(5) == pos(3) + 1;
        assert!(branch_a || branch_b, "{order:?}");
        assert_eq!(reached.len(), 5);
    }

    #[test]
    fn filters_decide_what_is_followed() {
        let only_calls = EdgeFilter::from_strings(&["calls".to_string()], None, &[]).unwrap();
        let reached = expand(&view(), &[id(1)], &ExpandOptions { filter: only_calls, ..Default::default() });
        assert_eq!(ids(&reached), vec![id(3), id(5)]);
    }

    #[test]
    fn several_seeds_are_expanded_together_and_never_listed_as_reached() {
        let reached = expand(&view(), &[id(1), id(2)], &ExpandOptions::default());
        assert!(!ids(&reached).contains(&id(1)) && !ids(&reached).contains(&id(2)));
    }

    #[test]
    fn the_output_shows_seeds_snippets_and_the_relation_to_each_node() {
        let v = view();
        let reached = expand(&v, &[id(1)], &ExpandOptions { max_depth: 1, ..Default::default() });
        let mut snippets = HashMap::new();
        snippets.insert(id(1), "export function seed() {}".to_string());
        let text = to_lines(&v, "how does seed work", &[id(1)], &reached, &snippets).join("\n");
        assert!(text.starts_with("# Query: how does seed work"), "{text}");
        assert!(text.contains("## Starting points") && text.contains("- `f1.ts` (file)"), "{text}");
        assert!(text.contains("export function seed() {}"), "{text}");
        assert!(text.contains("- d1 `f1.ts` --imports--> `f2.ts`"), "{text}");
        assert!(text.contains("`f6.ts` --imports--> `f1.ts`") || text.contains("`f1.ts` <--imports-- `f6.ts`"), "{text}");
    }

    #[test]
    fn no_seeds_and_no_neighbours_say_so() {
        let v = view();
        assert!(to_lines(&v, "q", &[], &[], &HashMap::new()).join("\n").contains("No starting points"));
        let alone = to_lines(&v, "q", &[id(7)], &[], &HashMap::new()).join("\n");
        assert!(alone.contains("Nothing else is connected"), "{alone}");
    }
}
