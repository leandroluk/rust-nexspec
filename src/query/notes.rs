//! Annotations as the queries show them (REQ-1806 in `.specs/features/semantic-annotations/spec.md`, decision D7):
//! `explain` lists them with author, date and state; `query` and `affected` add one short `notes:` line under a
//! node that has fresh ones. Everything comes from the `Annotation` nodes already in the graph.

use crate::graph::edge::EdgeType;
use crate::graph::node::NodePayload;
use crate::query::view::GraphView;
use crate::sync::mutation::StableId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeNote {
    /// `fresh` or `stale`.
    pub state: String,
    pub author: String,
    pub at: String,
    /// `label: note`, the note, the label or the outcome, whichever is there.
    pub text: String,
}

/// The annotations on `id`, newest first.
pub fn notes_for(view: &GraphView, id: &StableId) -> Vec<NodeNote> {
    let mut notes: Vec<NodeNote> = view
        .out_edges(id)
        .filter(|e| e.edge_type == EdgeType::AnnotatedBy)
        .filter_map(|e| match view.snapshot.nodes.get(&e.to)? {
            NodePayload::Annotation { label, note, author, at, state, outcome, .. } => {
                let text = match (label.is_empty(), note.is_empty()) {
                    (false, false) => format!("{label}: {note}"),
                    (false, true) => label.clone(),
                    (true, false) => note.clone(),
                    (true, true) => outcome.clone(),
                };
                Some(NodeNote { state: state.clone(), author: author.clone(), at: at.clone(), text })
            }
            _ => None,
        })
        .collect();
    notes.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| a.text.cmp(&b.text)));
    notes
}

/// `[state] author, date: text` for `explain`.
pub fn line(note: &NodeNote) -> String {
    let day = note.at.split('T').next().unwrap_or(&note.at);
    format!("[{}] {}, {day}: {}", note.state, note.author, note.text)
}

/// One `notes:` line for `query`/`affected`: the fresh annotations, at most two, cut short; `None` when there are none.
pub fn inline(view: &GraphView, id: &StableId) -> Option<String> {
    let fresh: Vec<String> = notes_for(view, id).into_iter().filter(|n| n.state == "fresh").take(2).map(|n| n.text).collect();
    if fresh.is_empty() {
        return None;
    }
    let joined = fresh.join(" | ");
    Some(if joined.chars().count() > 160 { format!("{}…", joined.chars().take(160).collect::<String>()) } else { joined })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::edge::Edge;
    use crate::report::snapshot::test_support::*;

    fn annotation(label: &str, note: &str, at: &str, state: &str) -> NodePayload {
        NodePayload::Annotation { target: "src/a.ts".into(), label: label.into(), note: note.into(), author: "agent".into(), at: at.into(), state: state.into(), outcome: String::new() }
    }

    fn view() -> GraphView {
        let e = |n, from, to| {
            let mut x: Edge = edge(n, from, to, EdgeType::AnnotatedBy);
            x.meta = crate::graph::edge::encode_meta(crate::graph::edge::Confidence::Inferred, crate::graph::edge::EdgeContext::Annotation);
            x
        };
        GraphView::new(snapshot(
            vec![(1, file("src/a.ts")), (2, annotation("Billing", "Charges residents.", "2026-09-30T10:00:00Z", "fresh")), (3, annotation("", "Old theory.", "2026-01-01T00:00:00Z", "stale")), (4, file("src/b.ts"))],
            vec![e(1, 1, 2), e(2, 1, 3)],
        ))
    }

    #[test]
    fn notes_come_newest_first_with_their_state() {
        let notes = notes_for(&view(), &id(1));
        assert_eq!(notes.len(), 2);
        assert_eq!(line(&notes[0]), "[fresh] agent, 2026-09-30: Billing: Charges residents.");
        assert_eq!(line(&notes[1]), "[stale] agent, 2026-01-01: Old theory.");
        assert!(notes_for(&view(), &id(4)).is_empty());
    }

    #[test]
    fn the_inline_line_has_only_fresh_notes_and_is_short() {
        assert_eq!(inline(&view(), &id(1)).as_deref(), Some("Billing: Charges residents."), "the stale one is left out");
        assert_eq!(inline(&view(), &id(4)), None);
    }
}
