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

/// Deterministic node id for a `REQ-`/`TASK-`/`ADR-` marker, independent of
/// its body text or of which file it is written in. A task in `tasks.md`
/// and a `@spec` comment in code can therefore point at a requirement
/// defined in `spec.md` without any lookup table (cross-document links).
pub(crate) fn marker_node_id(marker: &str) -> StableId {
    stable_id(format!("marker:{marker}").as_bytes())
}

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
            NodeValue::Paragraph | NodeValue::List(_) | NodeValue::Item(_) | NodeValue::BlockQuote => {
                // Block children (a bullet's paragraph and its nested list)
                // must not run words together.
                out.push_str(&collect_text(child));
                out.push(' ');
            }
            _ => out.push_str(&collect_text(child)),
        }
    }
    out
}

/// Byte length of the id tail after a marker prefix: zero or more upper-case
/// feature segments (`TCK-`, `CTR2-`), then digits, then an optional single
/// lower-case letter (`021b`). `"TCK-001: x"` -> `Some(7)`; `"001"` -> `Some(3)`;
/// no digits -> `None`. Lets namespaced ids (`REQ-TCK-001`) resolve like `REQ-001`.
pub(crate) fn id_tail_len(after: &str) -> Option<usize> {
    let bytes = after.as_bytes();
    let mut i = 0usize;
    loop {
        if i < bytes.len() && bytes[i].is_ascii_uppercase() {
            let mut j = i + 1;
            while j < bytes.len() && (bytes[j].is_ascii_uppercase() || bytes[j].is_ascii_digit()) {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'-' {
                i = j + 1;
                continue;
            }
        }
        break;
    }
    let digits = bytes[i..].iter().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    i += digits;
    if i < bytes.len() && bytes[i].is_ascii_lowercase() {
        i += 1;
    }
    Some(i)
}

/// If `text` (trimmed) starts with `marker` followed by an id tail and a `:`,
/// returns `(marker+digits, rest-of-text-trimmed)`. E.g. `parse_marker("REQ-001: must do X", "REQ-")`
/// -> `Some(("REQ-001".into(), "must do X".into()))`.
fn parse_marker(text: &str, marker: &str) -> Option<(String, String)> {
    let text = text.trim();
    let rest = text.strip_prefix(marker)?;
    let tail_len = id_tail_len(rest)?;
    let mut after_tail = rest[tail_len..].trim_start();
    // `REQ-CTC-001 (Cadastro de Contrato): ...` -- a short label in
    // parentheses between the id and the colon is part of the title.
    let mut label = String::new();
    if let Some(inner) = after_tail.strip_prefix('(')
        && let Some(close) = inner.find(')')
    {
        label = inner[..close].trim().to_string();
        after_tail = inner[close + 1..].trim_start();
    }
    let after_colon = after_tail.strip_prefix(':')?.trim();
    let body = match (label.is_empty(), after_colon.is_empty()) {
        (true, _) => after_colon.to_string(),
        (false, true) => label,
        (false, false) => format!("{label}. {after_colon}"),
    };
    Some((format!("{marker}{}", &rest[..tail_len]), body))
}

/// Longest body kept per requirement/task/ADR: enough to be searchable,
/// small enough not to bloat the index with whole sections.
const MAX_BODY_CHARS: usize = 800;

fn append_body(body: &mut String, extra: &str) {
    let extra = extra.split_whitespace().collect::<Vec<_>>().join(" ");
    if extra.is_empty() || body.chars().count() >= MAX_BODY_CHARS {
        return;
    }
    if !body.is_empty() {
        body.push(' ');
    }
    body.push_str(&extra);
    if let Some((cut, _)) = body.char_indices().nth(MAX_BODY_CHARS) {
        body.truncate(cut);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MarkerKind {
    Requirement,
    Task,
    Adr,
}

/// A requirement/task/ADR whose body is still collecting the text that
/// follows its heading.
struct Pending {
    kind: MarkerKind,
    marker: String,
    id: StableId,
    body: String,
}

impl Pending {
    fn into_node(self) -> NodeMutation {
        let payload = match self.kind {
            MarkerKind::Requirement => NodePayload::Requirement { title: self.marker, source_hash: self.id, body: self.body },
            MarkerKind::Task => NodePayload::Task { title: self.marker, source_hash: self.id, body: self.body },
            MarkerKind::Adr => NodePayload::Adr { title: self.marker, source_hash: self.id, body: self.body },
        };
        NodeMutation::Upsert { id: self.id, payload: rkyv_bytes(&payload) }
    }
}

/// Every `marker`-prefixed id (`marker` + digits) found anywhere in `text`.
/// `pub(crate)` — also used by `git::spec_link` for commit-message linking
/// (REQ-207 in `.specs/features/git-integration/spec.md`), so a "REQ-001"
/// mention is recognized identically whether it's in a spec or a commit.
pub(crate) fn find_markers(text: &str, marker: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut offset = 0usize;
    while let Some(pos) = text[offset..].find(marker) {
        let start = offset + pos;
        let after = &text[start + marker.len()..];
        if let Some(tail_len) = id_tail_len(after) {
            found.push(format!("{marker}{}", &after[..tail_len]));
            offset = start + marker.len() + tail_len;
        } else {
            offset = start + marker.len();
        }
    }
    found
}

/// `Satisfies` edge from `from` to every `REQ-` marker mentioned in `text`.
/// The target id is derived from the marker alone, so the requirement may
/// live in another file (it simply dangles until that file is indexed).
fn push_satisfies(edges: &mut Vec<EdgeMutation>, from: StableId, text: &str) {
    for req_marker in find_markers(text, "REQ-") {
        let to = marker_node_id(&req_marker);
        let edge_id = stable_id(format!("satisfies:{req_marker}:{from:?}").as_bytes());
        edges.push(EdgeMutation::Upsert {
            id: edge_id,
            from,
            to,
            edge_type: EdgeType::Satisfies.to_code(),
            payload: vec![],
        });
    }
}

/// Parse `markdown` (the contents of one `.md` file) into a [`MutationSet`]:
/// one node per `REQ-\d+`/`TASK-\d+`/`ADR-\d+` found, plus a `Satisfies`
/// edge from a task/ADR to any requirement its body text references.
pub fn extract(markdown: &str) -> MutationSet {
    let arena = Arena::new();
    let root = parse_document(&arena, markdown, &Options::default());

    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    // Task/ADR section currently open: its paragraphs and bullets link to
    // the requirements they mention.
    let mut open: Option<StableId> = None;
    // Whatever heading-defined marker is collecting body text.
    let mut pending: Option<Pending> = None;

    for child in root.children() {
        let value = child.data.borrow().value.clone();
        match value {
            NodeValue::Heading(_) => {
                if let Some(done) = pending.take() {
                    nodes.push(done.into_node());
                }
                open = None;
                let text = collect_text(child);
                for (prefix, kind) in [
                    ("TASK-", MarkerKind::Task),
                    ("ADR-", MarkerKind::Adr),
                    ("REQ-", MarkerKind::Requirement),
                ] {
                    if let Some((marker, body)) = parse_marker(&text, prefix) {
                        let id = marker_node_id(&marker);
                        if kind != MarkerKind::Requirement {
                            open = Some(id);
                        }
                        pending = Some(Pending { kind, marker, id, body });
                        break;
                    }
                }
            }
            NodeValue::List(_) if open.is_some() => {
                // Bullet lists inside a TASK-/ADR- section (`- **REQ**: REQ-001`)
                // are body text: link them like a paragraph would.
                if let Some(open_id) = open {
                    for item in child.children() {
                        let text = collect_text(item);
                        push_satisfies(&mut edges, open_id, &text);
                        if let Some(p) = pending.as_mut() {
                            append_body(&mut p.body, &text);
                        }
                    }
                }
            }
            NodeValue::List(_) => {
                for item in child.children() {
                    let text = collect_text(item);
                    if let Some((marker, body)) = parse_marker(&text, "REQ-") {
                        if let Some(done) = pending.take() {
                            nodes.push(done.into_node());
                        }
                        nodes.push(
                            Pending { kind: MarkerKind::Requirement, id: marker_node_id(&marker), marker, body }.into_node(),
                        );
                    } else if let Some(p) = pending.as_mut() {
                        // Plain bullets under a `### REQ-x:` heading describe it.
                        append_body(&mut p.body, &text);
                    }
                }
            }
            NodeValue::Paragraph => {
                let text = collect_text(child);
                if let Some(open_id) = open {
                    push_satisfies(&mut edges, open_id, &text);
                }
                if let Some(p) = pending.as_mut() {
                    append_body(&mut p.body, &text);
                }
            }
            _ => {}
        }
    }
    if let Some(done) = pending.take() {
        nodes.push(done.into_node());
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

    #[test]
    fn namespaced_and_suffixed_ids_are_recognized() {
        assert_eq!(id_tail_len("001: x"), Some(3));
        assert_eq!(id_tail_len("TCK-001: x"), Some(7));
        assert_eq!(id_tail_len("021b)"), Some(4));
        assert_eq!(id_tail_len("TCK-: x"), None);
        assert_eq!(id_tail_len("abc"), None);

        assert_eq!(
            find_markers("see REQ-TCK-001, REQ-002 and REQ-PAG-021b.", "REQ-"),
            vec!["REQ-TCK-001", "REQ-002", "REQ-PAG-021b"]
        );

        let set = extract("## Requirements
- REQ-CTR-001: cadastrar contrato
");
        assert_eq!(set.nodes.len(), 1, "namespaced REQ list item becomes a node");
    }

    #[test]
    fn task_list_items_link_to_requirement_defined_in_another_file() {
        // `tasks.md` alone: REQ-CTR-001 is not defined here.
        let tasks = extract("### TASK-CTR-001: Do it\n\n- **REQ**: REQ-CTR-001, REQ-CTR-002\n- **What**: x\n");
        let task_id = marker_node_id("TASK-CTR-001");
        let targets: Vec<StableId> = tasks
            .edges
            .iter()
            .filter_map(|e| match e {
                EdgeMutation::Upsert { from, to, .. } if *from == task_id => Some(*to),
                _ => None,
            })
            .collect();
        assert_eq!(targets, vec![marker_node_id("REQ-CTR-001"), marker_node_id("REQ-CTR-002")]);

        // `spec.md` defines the requirement under the same deterministic id.
        let spec = extract("## Requirements\n- REQ-CTR-001: cadastrar\n");
        let defined: Vec<StableId> = spec
            .nodes
            .iter()
            .filter_map(|n| match n {
                NodeMutation::Upsert { id, .. } => Some(*id),
                _ => None,
            })
            .collect();
        assert_eq!(defined, vec![marker_node_id("REQ-CTR-001")]);
    }

    fn requirement_bodies(set: &MutationSet) -> Vec<(String, String)> {
        set.nodes
            .iter()
            .filter_map(|n| match n {
                NodeMutation::Upsert { payload, .. } => {
                    let mut aligned = rkyv::util::AlignedVec::<16>::new();
                    aligned.extend_from_slice(payload);
                    match rkyv::from_bytes::<NodePayload, rkyv::rancor::Error>(&aligned).unwrap() {
                        NodePayload::Requirement { title, body, .. } => Some((title, body)),
                        _ => None,
                    }
                }
                NodeMutation::Remove { .. } => None,
            })
            .collect()
    }

    #[test]
    fn bold_requirement_with_parenthesised_label_and_nested_bullets() {
        let set = extract(
            "### 1. Contrato\n\n\
             - **REQ-CTC-001 (Cadastro de Contrato)**:\n  \
               - `POST /tenant/contract` (persona sindico):\n    \
                 - Payload: `title` (max 200)\n\
             - **REQ-CTC-002 (Regra de Recorrencia)**:\n",
        );
        let reqs = requirement_bodies(&set);
        assert_eq!(reqs.len(), 2, "{reqs:?}");
        assert_eq!(reqs[0].0, "REQ-CTC-001");
        assert!(reqs[0].1.contains("Cadastro de Contrato"), "label is part of the body: {:?}", reqs[0].1);
        assert!(reqs[0].1.contains("POST /tenant/contract"), "nested bullets describe it: {:?}", reqs[0].1);
        assert!(reqs[0].1.contains("Payload: title"), "words are not glued together: {:?}", reqs[0].1);
        assert_eq!(reqs[1].0, "REQ-CTC-002");
        assert_eq!(reqs[1].1, "Regra de Recorrencia");
    }

    #[test]
    fn requirement_heading_collects_following_text_as_its_body() {
        let set = extract(
            "### REQ-WAI-001: Condominium Service Live Integration\n\n\
             The web app must call the live API.\n\n\
             - retries on 5xx\n- surfaces errors\n\n\
             ### Something else\n\nnot part of it\n",
        );
        let reqs = requirement_bodies(&set);
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].0, "REQ-WAI-001");
        for expected in ["Condominium Service Live Integration", "must call the live API", "retries on 5xx", "surfaces errors"] {
            assert!(reqs[0].1.contains(expected), "{expected:?} missing from {:?}", reqs[0].1);
        }
        assert!(!reqs[0].1.contains("not part of it"), "the next heading closes the requirement");
    }

    #[test]
    fn long_bodies_are_capped() {
        let long = "word ".repeat(1000);
        let set = extract(&format!("### REQ-9: big\n\n{long}\n"));
        let reqs = requirement_bodies(&set);
        assert!(reqs[0].1.chars().count() <= MAX_BODY_CHARS, "{}", reqs[0].1.chars().count());
    }

    #[test]
    fn plain_format_is_unchanged() {
        let set = extract("## Requirements\n- REQ-001: The system must do X\n");
        assert_eq!(requirement_bodies(&set), vec![("REQ-001".to_string(), "The system must do X".to_string())]);
    }
}
