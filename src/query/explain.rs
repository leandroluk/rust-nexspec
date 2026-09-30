//! `explain` (REQ-1103): a compact description of one node. The pure part
//! works on the graph; what needs I/O (the pruned signature, the community,
//! recent authors) arrives through [`ExplainContext`] and each part is
//! omitted when it is not available.

use serde::Serialize;

use crate::graph::edge::{Confidence, Edge, EdgeType};
use crate::graph::node::NodePayload;
use crate::query::filter::{EdgeFilter, context_name, relation_name};
use crate::query::view::GraphView;
use crate::sync::mutation::StableId;

#[derive(Debug, Clone, Default)]
pub struct ExplainContext {
    pub signature: Option<String>,
    /// `(label, community id, cohesion)` of the file's community.
    pub community: Option<(String, usize, f64)>,
    pub authors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Neighbor {
    pub label: String,
    pub relation: String,
    pub confidence: String,
    pub context: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Explanation {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub path: Option<String>,
    /// 1-based inclusive line range (symbols).
    pub lines: Option<(u32, u32)>,
    /// First characters of a requirement/task/ADR body.
    pub text: Option<String>,
    pub signature: Option<String>,
    pub depends_on: Vec<Neighbor>,
    pub depends_on_total: usize,
    pub depended_on_by: Vec<Neighbor>,
    pub depended_on_by_total: usize,
    /// Requirements this node satisfies (symbols/tasks) or is satisfied by (requirements).
    pub requirements: Vec<String>,
    pub community: Option<String>,
    pub authors: Vec<String>,
}

fn neighbor(view: &GraphView, other: &StableId, edge: &Edge) -> Neighbor {
    let (confidence, context) = crate::graph::edge::decode_meta(edge.meta);
    Neighbor {
        label: view.snapshot.label(other),
        relation: relation_name(edge.edge_type).to_string(),
        confidence: match confidence {
            Confidence::Extracted => "extracted",
            Confidence::Inferred => "inferred",
        }
        .to_string(),
        context: context_name(context).to_string(),
    }
}

fn ranked<'a>(mut edges: Vec<(&'a StableId, &'a Edge)>, view: &GraphView) -> Vec<(&'a StableId, &'a Edge)> {
    edges.sort_by(|a, b| {
        (a.1.meta & 1, (a.1.meta >> 1) & 0b11, view.snapshot.label(a.0), a.0).cmp(&(b.1.meta & 1, (b.1.meta >> 1) & 0b11, view.snapshot.label(b.0), b.0))
    });
    edges
}

pub fn explain(view: &GraphView, id: &StableId, context: &ExplainContext, filter: &EdgeFilter, top: usize) -> Option<Explanation> {
    let payload = view.snapshot.nodes.get(id)?;
    let outgoing: Vec<(&StableId, &Edge)> = view
        .out_edges(id)
        .filter(|e| e.edge_type.is_dependency() && filter.allows(e) && view.contains(&e.to))
        .map(|e| (&e.to, e))
        .collect();
    let incoming: Vec<(&StableId, &Edge)> = view
        .in_edges(id)
        .filter(|e| e.edge_type.is_dependency() && filter.allows(e) && view.contains(&e.from))
        .map(|e| (&e.from, e))
        .collect();
    let (depends_on_total, depended_on_by_total) = (outgoing.len(), incoming.len());

    // Requirement links in both directions.
    let mut requirements: Vec<String> = Vec::new();
    match payload {
        NodePayload::Requirement { .. } => {
            for e in view.in_edges(id).filter(|e| e.edge_type == EdgeType::Satisfies) {
                if view.contains(&e.from) {
                    requirements.push(format!("satisfied by {}", view.snapshot.label(&e.from)));
                }
            }
        }
        _ => {
            for e in view.out_edges(id).filter(|e| e.edge_type == EdgeType::Satisfies) {
                if matches!(view.snapshot.nodes.get(&e.to), Some(NodePayload::Requirement { .. } | NodePayload::Adr { .. })) {
                    requirements.push(view.snapshot.label(&e.to));
                }
            }
        }
    }
    requirements.sort();
    requirements.dedup();

    let (lines, text) = match payload {
        NodePayload::Symbol { line_start, line_end, .. } => (Some((line_start + 1, line_end + 1)), None),
        NodePayload::Requirement { body, .. } | NodePayload::Task { body, .. } | NodePayload::Adr { body, .. } => {
            let clipped: String = body.chars().take(240).collect();
            (None, (!clipped.is_empty()).then_some(clipped))
        }
        _ => (None, None),
    };

    Some(Explanation {
        id: crate::engine::id_hex(id),
        label: view.snapshot.label(id),
        kind: view.snapshot.kind_name(id).to_string(),
        path: view.snapshot.path_of(id).map(str::to_string),
        lines,
        text,
        signature: context.signature.clone(),
        depends_on: ranked(outgoing, view).into_iter().take(top).map(|(o, e)| neighbor(view, o, e)).collect(),
        depends_on_total,
        depended_on_by: ranked(incoming, view).into_iter().take(top).map(|(o, e)| neighbor(view, o, e)).collect(),
        depended_on_by_total,
        requirements,
        community: context.community.as_ref().map(|(label, id, cohesion)| format!("{label} (community {id}, cohesion {cohesion:.2})")),
        authors: context.authors.clone(),
    })
}

fn flags(n: &Neighbor) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if n.confidence == "inferred" {
        parts.push("inferred");
    }
    if n.context != "runtime" {
        parts.push(n.context.as_str());
    }
    if parts.is_empty() { String::new() } else { format!(" [{}]", parts.join(", ")) }
}

pub fn to_lines(e: &Explanation) -> Vec<String> {
    let mut lines = vec![format!("# {} ({})", e.label, e.kind)];
    match (&e.path, e.lines) {
        (Some(path), Some((a, b))) => lines.push(format!("- Where: `{path}` lines {a}-{b}")),
        (Some(path), None) => lines.push(format!("- Where: `{path}`")),
        _ => {}
    }
    if let Some(text) = &e.text {
        lines.push(format!("- Text: {text}"));
    }
    if let Some(community) = &e.community {
        lines.push(format!("- Community: {community}"));
    }
    if !e.authors.is_empty() {
        lines.push(format!("- Recent authors: {}", e.authors.join(", ")));
    }
    if !e.requirements.is_empty() {
        lines.push(String::new());
        lines.push("## Requirements".to_string());
        lines.extend(e.requirements.iter().map(|r| format!("- {r}")));
    }
    if let Some(signature) = &e.signature {
        lines.push(String::new());
        lines.push("## Signature".to_string());
        lines.push("```".to_string());
        lines.extend(signature.lines().map(str::to_string));
        lines.push("```".to_string());
    }
    let section = |lines: &mut Vec<String>, title: &str, items: &[Neighbor], total: usize, arrow: &str| {
        if total == 0 {
            return;
        }
        lines.push(String::new());
        lines.push(format!("## {title} ({total})"));
        for n in items {
            lines.push(format!("- {arrow} {} `{}`{}", n.relation, n.label, flags(n)));
        }
        if total > items.len() {
            lines.push(format!("- … +{} more", total - items.len()));
        }
    };
    section(&mut lines, "Depends on", &e.depends_on, e.depends_on_total, "->");
    section(&mut lines, "Depended on by", &e.depended_on_by, e.depended_on_by_total, "<-");
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::edge::{EdgeContext, encode_meta};
    use crate::report::snapshot::test_support::*;

    fn view() -> GraphView {
        let mut spec_edge = edge(5, 5, 2, EdgeType::Calls);
        spec_edge.meta = encode_meta(Confidence::Inferred, EdgeContext::Spec);
        GraphView::new(snapshot(
            vec![
                (1, file("src/core.ts")),
                (2, symbol("handle")),
                (3, requirement("REQ-7")),
                (4, symbol("helper")),
                (5, symbol("handleSpec")),
                (6, symbol("caller")),
            ],
            vec![
                edge(1, 2, 1, EdgeType::DefinedIn),
                edge(2, 2, 3, EdgeType::Satisfies),
                edge(3, 2, 4, EdgeType::Calls),
                edge(4, 6, 2, EdgeType::Calls),
                spec_edge,
            ],
        ))
    }

    #[test]
    fn a_symbol_is_described_with_location_links_and_requirements() {
        let v = view();
        let ctx = ExplainContext {
            signature: Some("function handle() {\n  // …\n}".into()),
            community: Some(("src".into(), 3, 0.62)),
            authors: vec!["Ada".into(), "Grace".into()],
        };
        let e = explain(&v, &id(2), &ctx, &EdgeFilter::default(), 10).unwrap();
        assert_eq!((e.kind.as_str(), e.lines), ("symbol", Some((1, 2))));
        assert_eq!(e.requirements, vec!["REQ-7"]);
        assert_eq!(e.depends_on.len(), 1);
        assert_eq!(e.depended_on_by_total, 2);
        assert_eq!(e.depended_on_by[0].label, "caller", "the trusted runtime caller is listed before the inferred spec one");

        let text = to_lines(&e).join("\n");
        assert!(text.starts_with("# handle"), "{text}");
        assert!(text.contains("## Requirements") && text.contains("- REQ-7"), "{text}");
        assert!(text.contains("## Signature") && text.contains("function handle()"), "{text}");
        assert!(text.contains("## Depends on (1)") && text.contains("-> calls `helper"), "{text}");
        assert!(text.contains("## Depended on by (2)") && text.contains("<- calls `handleSpec") && text.contains("[inferred, spec]"), "{text}");
        assert!(text.contains("Community: src (community 3, cohesion 0.62)") && text.contains("Recent authors: Ada, Grace"), "{text}");
    }

    #[test]
    fn a_requirement_lists_what_satisfies_it() {
        let v = view();
        let e = explain(&v, &id(3), &ExplainContext::default(), &EdgeFilter::default(), 10).unwrap();
        assert_eq!(e.kind, "requirement");
        assert_eq!(e.requirements, vec!["satisfied by handle (src/core.ts)"]);
    }

    #[test]
    fn the_list_is_limited_and_reports_the_total() {
        let v = view();
        let e = explain(&v, &id(2), &ExplainContext::default(), &EdgeFilter::default(), 1).unwrap();
        assert_eq!(e.depended_on_by.len(), 1);
        assert_eq!(e.depended_on_by_total, 2);
        assert!(to_lines(&e).join("\n").contains("… +1 more"));
    }

    #[test]
    fn missing_context_parts_are_simply_left_out_and_unknown_nodes_give_none() {
        let v = view();
        let e = explain(&v, &id(1), &ExplainContext::default(), &EdgeFilter::default(), 5).unwrap();
        let text = to_lines(&e).join("\n");
        assert!(!text.contains("## Signature") && !text.contains("Community:") && !text.contains("Recent authors"), "{text}");
        assert!(explain(&v, &id(99), &ExplainContext::default(), &EdgeFilter::default(), 5).is_none());
    }
}
