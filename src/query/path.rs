//! `path` (REQ-1102): the shortest chain of edges between two nodes. Edges
//! are walked in either direction (two nodes are related when they share a
//! dependency as much as when one uses the other); every hop reports its
//! real direction, relation and confidence.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::Serialize;

use crate::graph::edge::{Confidence, Edge};
use crate::query::filter::{EdgeFilter, context_name, relation_name};
use crate::query::view::GraphView;
use crate::sync::mutation::StableId;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Hop {
    pub from: String,
    pub to: String,
    pub relation: String,
    /// `true` when the edge really points `from -> to`; `false` when the walk
    /// crossed it backwards (the edge is `to -> from`).
    pub forward: bool,
    pub confidence: String,
    pub context: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PathResult {
    pub found: bool,
    pub hops: Vec<Hop>,
    /// When not found: how many nodes each side can reach (the two islands).
    pub reachable_from_start: usize,
    pub reachable_from_end: usize,
}

pub(crate) type Step<'a> = (StableId, &'a Edge, bool);

/// Neighbours of `id` in either direction through allowed edges, most
/// trustworthy first, then by id.
pub(crate) fn neighbours<'a>(view: &'a GraphView, id: &StableId, filter: &EdgeFilter) -> Vec<Step<'a>> {
    let mut steps: Vec<Step<'a>> = view
        .out_edges(id)
        .filter(|e| filter.allows(e) && view.contains(&e.to))
        .map(|e| (e.to, e, true))
        .chain(view.in_edges(id).filter(|e| filter.allows(e) && view.contains(&e.from)).map(|e| (e.from, e, false)))
        .collect();
    // Trusted links first, then by id: the same graph always yields the same path.
    steps.sort_by_key(|(id, edge, _)| (edge.meta & 1, (edge.meta >> 1) & 0b11, *id));
    steps
}

fn hop(view: &GraphView, from: &StableId, to: &StableId, edge: &Edge, forward: bool) -> Hop {
    let (confidence, context) = crate::graph::edge::decode_meta(edge.meta);
    Hop {
        from: view.snapshot.label(from),
        to: view.snapshot.label(to),
        relation: relation_name(edge.edge_type).to_string(),
        forward,
        confidence: match confidence {
            Confidence::Extracted => "extracted",
            Confidence::Inferred => "inferred",
        }
        .to_string(),
        context: context_name(context).to_string(),
    }
}

/// Nodes reachable from `start` (capped, it is only a size hint).
fn reachable(view: &GraphView, start: &StableId, filter: &EdgeFilter) -> usize {
    let mut seen: HashSet<StableId> = HashSet::from([*start]);
    let mut queue = VecDeque::from([*start]);
    while let Some(node) = queue.pop_front() {
        if seen.len() > 100_000 {
            break;
        }
        for (next, _, _) in neighbours(view, &node, filter) {
            if seen.insert(next) {
                queue.push_back(next);
            }
        }
    }
    seen.len() - 1
}

pub fn find_path(view: &GraphView, from: &StableId, to: &StableId, filter: &EdgeFilter, max_hops: usize) -> PathResult {
    if from == to {
        return PathResult { found: true, hops: Vec::new(), reachable_from_start: 0, reachable_from_end: 0 };
    }
    // node -> (previous node, the step that led here)
    let mut previous: HashMap<StableId, (StableId, Edge, bool)> = HashMap::new();
    let mut seen: HashSet<StableId> = HashSet::from([*from]);
    let mut frontier = vec![*from];
    let mut found = false;
    'search: for _ in 0..max_hops {
        let mut next_frontier = Vec::new();
        for node in &frontier {
            for (next, edge, forward) in neighbours(view, node, filter) {
                if seen.insert(next) {
                    previous.insert(next, (*node, edge.clone(), forward));
                    if next == *to {
                        found = true;
                        break 'search;
                    }
                    next_frontier.push(next);
                }
            }
        }
        if next_frontier.is_empty() {
            break;
        }
        frontier = next_frontier;
    }

    if !found {
        return PathResult {
            found: false,
            hops: Vec::new(),
            reachable_from_start: reachable(view, from, filter),
            reachable_from_end: reachable(view, to, filter),
        };
    }
    let mut hops = Vec::new();
    let mut cursor = *to;
    while cursor != *from {
        let (parent, edge, forward) = &previous[&cursor];
        hops.push(hop(view, parent, &cursor, edge, *forward));
        cursor = *parent;
    }
    hops.reverse();
    PathResult { found: true, hops, reachable_from_start: 0, reachable_from_end: 0 }
}

pub fn to_lines(view: &GraphView, from: &StableId, to: &StableId, result: &PathResult) -> Vec<String> {
    let (a, b) = (view.snapshot.label(from), view.snapshot.label(to));
    if !result.found {
        return vec![
            format!("# No path between `{a}` and `{b}`"),
            format!(
                "They are in different parts of the graph (with these filters): `{a}` reaches {} node(s), `{b}` reaches {}.",
                result.reachable_from_start, result.reachable_from_end
            ),
        ];
    }
    if result.hops.is_empty() {
        return vec![format!("# `{a}` and `{b}` are the same node")];
    }
    let mut lines = vec![format!("# Path from `{a}` to `{b}` ({} hop{})", result.hops.len(), if result.hops.len() == 1 { "" } else { "s" })];
    for (i, h) in result.hops.iter().enumerate() {
        let arrow = if h.forward { format!("--{}-->", h.relation) } else { format!("<--{}--", h.relation) };
        let mut flags: Vec<&str> = Vec::new();
        if h.confidence == "inferred" {
            flags.push("inferred");
        }
        if h.context != "runtime" {
            flags.push(h.context.as_str());
        }
        let flags = if flags.is_empty() { String::new() } else { format!(" [{}]", flags.join(", ")) };
        lines.push(format!("{}. `{}` {arrow} `{}`{flags}", i + 1, h.from, h.to));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::edge::{EdgeContext, EdgeType, encode_meta};
    use crate::report::snapshot::test_support::*;

    /// a -> b -> c, a -> d <- c (so c reaches d backwards), e isolated.
    fn graph() -> GraphView {
        GraphView::new(snapshot(
            vec![(1, file("a.ts")), (2, file("b.ts")), (3, file("c.ts")), (4, file("d.ts")), (5, file("e.ts"))],
            vec![
                edge(1, 1, 2, EdgeType::Imports),
                edge(2, 2, 3, EdgeType::Calls),
                edge(3, 1, 4, EdgeType::Imports),
                edge(4, 3, 4, EdgeType::References),
            ],
        ))
    }

    #[test]
    fn shortest_path_with_direction_and_relation_per_hop() {
        let view = graph();
        let r = find_path(&view, &id(1), &id(3), &EdgeFilter::default(), 12);
        assert!(r.found);
        assert_eq!(r.hops.len(), 2, "a -> b -> c, or a -> d <- c: both are two hops");
        assert_eq!(r.hops[0].from, "a.ts");
        assert_eq!(r.hops.last().unwrap().to, "c.ts");
        assert!(r.hops.iter().all(|h| !h.relation.is_empty()));
    }

    #[test]
    fn a_path_can_walk_an_edge_backwards_and_says_so() {
        let view = GraphView::new(snapshot(
            vec![(1, file("user1.ts")), (2, file("shared.ts")), (3, file("user2.ts"))],
            vec![edge(1, 1, 2, EdgeType::Imports), edge(2, 3, 2, EdgeType::Imports)],
        ));
        let r = find_path(&view, &id(1), &id(3), &EdgeFilter::default(), 12);
        assert!(r.found);
        assert_eq!(r.hops.len(), 2);
        assert!(r.hops[0].forward, "user1 -> shared");
        assert!(!r.hops[1].forward, "shared <- user2 crossed backwards");
        let text = to_lines(&view, &id(1), &id(3), &r).join("\n");
        assert!(text.contains("`user1.ts` --imports--> `shared.ts`") && text.contains("`shared.ts` <--imports-- `user2.ts`"), "{text}");
    }

    #[test]
    fn no_path_is_a_result_not_an_error() {
        let view = graph();
        let r = find_path(&view, &id(1), &id(5), &EdgeFilter::default(), 12);
        assert!(!r.found && r.hops.is_empty());
        assert_eq!(r.reachable_from_start, 3);
        assert_eq!(r.reachable_from_end, 0);
        let text = to_lines(&view, &id(1), &id(5), &r).join("\n");
        assert!(text.contains("No path between `a.ts` and `e.ts`") && text.contains("reaches 3 node(s)"), "{text}");
    }

    #[test]
    fn the_hop_limit_and_identical_endpoints_are_handled() {
        let view = graph();
        assert!(!find_path(&view, &id(1), &id(3), &EdgeFilter::default(), 1).found, "needs two hops");
        let same = find_path(&view, &id(2), &id(2), &EdgeFilter::default(), 12);
        assert!(same.found && same.hops.is_empty());
    }

    #[test]
    fn filters_change_which_paths_exist_and_confidence_shows_in_the_hop() {
        let mut inferred = edge(1, 1, 2, EdgeType::Imports);
        inferred.meta = encode_meta(Confidence::Inferred, EdgeContext::TypeOnly);
        let view = GraphView::new(snapshot(vec![(1, file("a.ts")), (2, file("b.ts"))], vec![inferred]));
        let strict = EdgeFilter::from_strings(&[], Some("extracted"), &[]).unwrap();
        assert!(!find_path(&view, &id(1), &id(2), &strict, 5).found);
        let r = find_path(&view, &id(1), &id(2), &EdgeFilter::default(), 5);
        assert_eq!((r.hops[0].confidence.as_str(), r.hops[0].context.as_str()), ("inferred", "type-only"));
        assert!(to_lines(&view, &id(1), &id(2), &r).join("\n").contains("[inferred, type-only]"));
    }

    #[test]
    fn the_same_graph_always_gives_the_same_path() {
        let view = graph();
        let a = find_path(&view, &id(1), &id(3), &EdgeFilter::default(), 12);
        let b = find_path(&view, &id(1), &id(3), &EdgeFilter::default(), 12);
        assert_eq!(a, b);
    }
}
