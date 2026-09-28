//! `.specs/**/*.md` extraction via `comrak` (REQ-108 in
//! `.specs/features/storage-primitives/spec.md`). Recognizes `REQ-\d+` list
//! items and `TASK-\d+`/`ADR-\d+` headings, and links a task/ADR to any
//! requirement its body text mentions (`Satisfies`).
//!
//! This is a pure function — it does not know about `sync::Coordinator` or
//! when to run (REQ-109's boundary, per
//! `.specs/features/storage-primitives/design.md`). A forward reference (a
//! requirement mentioned before it's been parsed) is not linked — full
//! cross-document resolution is future work, not required for Fase 1.

use std::collections::HashMap;

use comrak::nodes::{AstNode, NodeValue};
use comrak::{Arena, Options, parse_document};

use crate::graph::edge::EdgeType;
use crate::graph::node::NodePayload;
use crate::sync::mutation::{DocMutation, EdgeMutation, MutationSet, NodeMutation, StableId};

fn stable_id(bytes: &[u8]) -> StableId {
    *blake3::hash(bytes).as_bytes()
}

fn collect_text<'a>(node: &'a AstNode<'a>) -> String {
    let mut out = String::new();
    for child in node.children() {
        match &child.data.borrow().value {
            NodeValue::Text(t) => out.push_str(t),
            NodeValue::Code(c) => out.push_str(&c.literal),
            NodeValue::SoftBreak | NodeValue::LineBreak => out.push(' '),
            _ => out.push_str(&collect_text(child)),
        }
    }
    out
}

/// If `text` (trimmed) starts with `marker` followed by digits and a `:`,
/// returns `(marker+digits, rest-of-text-trimmed)`. E.g. `parse_marker("REQ-001: must do X", "REQ-")`
/// -> `Some(("REQ-001".into(), "must do X".into()))`.
fn parse_marker(text: &str, marker: &str) -> Option<(String, String)> {
    let text = text.trim();
    let rest = text.strip_prefix(marker)?;
    let digit_len = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    if digit_len == 0 {
        return None;
    }
    let after_digits = &rest[digit_len..];
    let after_colon = after_digits.strip_prefix(':')?;
    Some((
        format!("{marker}{}", &rest[..digit_len]),
        after_colon.trim().to_string(),
    ))
}

/// Every `marker`-prefixed id (`marker` + digits) found anywhere in `text`.
fn find_markers(text: &str, marker: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut offset = 0usize;
    while let Some(pos) = text[offset..].find(marker) {
        let start = offset + pos;
        let after = &text[start + marker.len()..];
        let digit_len = after.chars().take_while(|c| c.is_ascii_digit()).count();
        if digit_len > 0 {
            found.push(format!("{marker}{}", &after[..digit_len]));
            offset = start + marker.len() + digit_len;
        } else {
            offset = start + marker.len();
        }
    }
    found
}

/// Parse `markdown` (the contents of one `.md` file) into a [`MutationSet`]:
/// one node per `REQ-\d+`/`TASK-\d+`/`ADR-\d+` found, plus a `Satisfies`
/// edge from a task/ADR to any requirement its body text references.
pub fn extract(markdown: &str) -> MutationSet {
    let arena = Arena::new();
    let root = parse_document(&arena, markdown, &Options::default());

    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut known_ids: HashMap<String, StableId> = HashMap::new();
    let mut open: Option<StableId> = None;

    for child in root.children() {
        let value = child.data.borrow().value.clone();
        match value {
            NodeValue::Heading(_) => {
                let text = collect_text(child);
                if let Some((marker_id, body)) = parse_marker(&text, "TASK-") {
                    let id = stable_id(format!("{marker_id}:{body}").as_bytes());
                    known_ids.insert(marker_id.clone(), id);
                    nodes.push(NodeMutation::Upsert {
                        id,
                        payload: rkyv_bytes(&NodePayload::Task {
                            title: marker_id,
                            source_hash: id,
                            body,
                        }),
                    });
                    open = Some(id);
                } else if let Some((marker_id, body)) = parse_marker(&text, "ADR-") {
                    let id = stable_id(format!("{marker_id}:{body}").as_bytes());
                    known_ids.insert(marker_id.clone(), id);
                    nodes.push(NodeMutation::Upsert {
                        id,
                        payload: rkyv_bytes(&NodePayload::Adr {
                            title: marker_id,
                            source_hash: id,
                            body,
                        }),
                    });
                    open = Some(id);
                } else {
                    open = None;
                }
            }
            NodeValue::List(_) => {
                for item in child.children() {
                    let text = collect_text(item);
                    if let Some((marker_id, body)) = parse_marker(&text, "REQ-") {
                        let id = stable_id(format!("{marker_id}:{body}").as_bytes());
                        known_ids.insert(marker_id.clone(), id);
                        nodes.push(NodeMutation::Upsert {
                            id,
                            payload: rkyv_bytes(&NodePayload::Requirement {
                                title: marker_id,
                                source_hash: id,
                                body,
                            }),
                        });
                    }
                }
                open = None;
            }
            NodeValue::Paragraph => {
                if let Some(open_id) = open {
                    let text = collect_text(child);
                    for req_marker in find_markers(&text, "REQ-") {
                        if let Some(&req_id) = known_ids.get(&req_marker) {
                            let edge_id =
                                stable_id(format!("satisfies:{req_marker}:{open_id:?}").as_bytes());
                            edges.push(EdgeMutation::Upsert {
                                id: edge_id,
                                from: open_id,
                                to: req_id,
                                edge_type: EdgeType::Satisfies.to_code(),
                                payload: vec![],
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }

    MutationSet {
        nodes,
        edges,
        docs: Vec::<DocMutation>::new(),
    }
}

/// `NodePayload` isn't itself the `MutationSet` payload type (that's opaque
/// bytes, per the coordinator's boundary) — serialize it with `rkyv` here,
/// the same encoding `RedbParticipant` expects to store and, eventually,
/// decode back into a typed `Node`.
fn rkyv_bytes(payload: &NodePayload) -> Vec<u8> {
    rkyv::to_bytes::<rkyv::rancor::Error>(payload)
        .expect("NodePayload must always serialize")
        .to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
# Sample Spec

## Requirements
- REQ-001: The system must do X

## Tasks
### TASK-001: Implement X
Satisfies REQ-001.

## Decisions
### ADR-001: Use Y
We chose Y for this project.
"#;

    #[test]
    fn extracts_three_nodes_and_links_task_to_requirement() {
        let set = extract(FIXTURE);

        assert_eq!(set.nodes.len(), 3, "REQ-001, TASK-001, ADR-001");
        assert_eq!(set.edges.len(), 1, "TASK-001 satisfies REQ-001");

        let requirement_id = match &set.nodes[0] {
            NodeMutation::Upsert { id, .. } => *id,
            NodeMutation::Remove { .. } => panic!("expected upsert"),
        };
        match &set.edges[0] {
            EdgeMutation::Upsert { to, edge_type, .. } => {
                assert_eq!(*to, requirement_id);
                assert_eq!(*edge_type, EdgeType::Satisfies.to_code());
            }
            EdgeMutation::Remove { .. } => panic!("expected upsert"),
        }
    }

    #[test]
    fn requirement_with_no_referencing_task_produces_no_edges() {
        let set = extract("## Requirements\n- REQ-042: standalone requirement\n");
        assert_eq!(set.nodes.len(), 1);
        assert!(set.edges.is_empty());
    }
}
