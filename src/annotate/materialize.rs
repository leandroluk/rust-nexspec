//! From the annotations file to the graph (REQ-1802, REQ-1803, REQ-1804 in
//! `.specs/features/semantic-annotations/spec.md`; decisions D3-D5).
//!
//! Pure: the annotations, the graph as it is and the files on disk go in; the state of each annotation and the
//! difference between the annotation nodes and edges that *should* be in the index and the ones that *are* come out.
//! `Engine::materialize_annotations` applies the difference.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::annotate::store::{Annotation, resolve_key, target_hash};
use crate::code::parser::edge_id;
use crate::graph::edge::{Confidence, EdgeContext, EdgeType, encode_meta};
use crate::graph::node::{NodePayload, annotation_node_id};
use crate::query::filter::relation_types;
use crate::report::snapshot::GraphSnapshot;
use crate::sync::mutation::{EdgeMutation, MutationSet, NodeMutation, StableId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Fresh,
    /// The target changed since the annotation was made.
    Stale,
    /// The target no longer exists.
    Dangling,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Stale => "stale",
            Self::Dangling => "dangling",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Evaluated {
    pub annotation: Annotation,
    pub state: State,
    pub target: Option<StableId>,
    /// The node `--to` points at, when the annotation has a relation and that node exists.
    pub to: Option<StableId>,
}

/// Communities are not nodes (their label lives only in the file), so their annotations are never stale or dangling here.
pub fn is_community_key(key: &str) -> bool {
    key.starts_with("community:")
}

pub fn evaluate(repo: &Path, snapshot: &GraphSnapshot, annotations: &[Annotation]) -> Vec<Evaluated> {
    annotations
        .iter()
        .map(|annotation| {
            if is_community_key(&annotation.target) {
                return Evaluated { annotation: annotation.clone(), state: State::Fresh, target: None, to: None };
            }
            let Some(target) = resolve_key(snapshot, &annotation.target) else {
                return Evaluated { annotation: annotation.clone(), state: State::Dangling, target: None, to: None };
            };
            let stale = match (&annotation.source_hash, target_hash(repo, snapshot, &target)) {
                (Some(then), Some(now)) => *then != now,
                _ => false,
            };
            let to = annotation.to.as_deref().and_then(|key| resolve_key(snapshot, key));
            Evaluated { annotation: annotation.clone(), state: if stale { State::Stale } else { State::Fresh }, target: Some(target), to }
        })
        .collect()
}

/// What the index should hold for these annotations.
#[derive(Debug, Default, Clone)]
pub struct Desired {
    pub nodes: BTreeMap<StableId, NodePayload>,
    pub edges: BTreeMap<StableId, (StableId, StableId, EdgeType, u8)>,
}

pub fn desired(evaluated: &[Evaluated]) -> Desired {
    let mut out = Desired::default();
    for e in evaluated {
        let (Some(target), true) = (e.target, e.state != State::Dangling) else { continue };
        let a = &e.annotation;
        let node_id = annotation_node_id(&a.id);
        out.nodes.insert(
            node_id,
            NodePayload::Annotation {
                target: a.target.clone(),
                label: a.label.clone().unwrap_or_default(),
                note: a.note.clone().unwrap_or_default(),
                author: a.author.clone(),
                at: a.at.clone(),
                state: e.state.as_str().to_string(),
                outcome: a.outcome.clone().unwrap_or_default(),
            },
        );
        let meta = encode_meta(Confidence::Inferred, EdgeContext::Annotation);
        out.edges.insert(edge_id("annotation-by", &target, &node_id), (target, node_id, EdgeType::AnnotatedBy, meta));
        // A relation is only drawn while the annotation is fresh: a stale one leaves the graph's structure.
        if e.state == State::Fresh
            && let (Some(relation), Some(to)) = (&a.relation, e.to)
            && let Some(edge_type) = relation_types(relation).and_then(|t| t.first().copied())
        {
            out.edges.insert(edge_id(&format!("annotation-rel-{relation}"), &target, &to), (target, to, edge_type, meta));
        }
    }
    out
}

/// The mutations that turn what the index has (`existing_nodes`, `existing_edges`) into `desired`; empty when they agree.
pub fn reconcile(desired: &Desired, existing_nodes: &BTreeMap<StableId, NodePayload>, existing_edges: &BTreeSet<StableId>) -> MutationSet {
    let mut set = MutationSet::default();
    for (id, payload) in &desired.nodes {
        if existing_nodes.get(id) != Some(payload) {
            let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(payload).expect("NodePayload must always serialize").to_vec();
            set.nodes.push(NodeMutation::Upsert { id: *id, payload: bytes });
        }
    }
    for id in existing_nodes.keys().filter(|id| !desired.nodes.contains_key(*id)) {
        set.nodes.push(NodeMutation::Remove { id: *id });
    }
    for (id, (from, to, edge_type, meta)) in &desired.edges {
        if !existing_edges.contains(id) {
            set.edges.push(EdgeMutation::Upsert { id: *id, from: *from, to: *to, edge_type: edge_type.to_code(), payload: vec![*meta] });
        }
    }
    for id in existing_edges.iter().filter(|id| !desired.edges.contains_key(*id)) {
        set.edges.push(EdgeMutation::Remove { id: *id });
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotate::store::Draft;
    use crate::report::snapshot::test_support::*;

    fn annotation(target: &str, note: &str, source_hash: Option<&str>) -> Annotation {
        Annotation::new(Draft { target: target.into(), note: Some(note.into()), author: "agent".into(), source_hash: source_hash.map(str::to_string), ..Draft::default() }, 1).unwrap()
    }

    fn repo_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        for (path, content) in files {
            std::fs::create_dir_all(dir.path().join(path).parent().unwrap()).unwrap();
            std::fs::write(dir.path().join(path), content).unwrap();
        }
        dir
    }

    fn graph() -> GraphSnapshot {
        snapshot(vec![(1, file("src/a.ts")), (2, symbol("Alpha")), (3, file("src/b.ts"))], vec![edge(1, 2, 1, EdgeType::DefinedIn)])
    }

    fn hash_of(text: &str) -> String {
        blake3::hash(text.as_bytes()).to_hex().to_string()
    }

    #[test]
    fn a_target_that_changed_is_stale_and_one_that_vanished_is_dangling() {
        let repo = repo_with(&[("src/a.ts", "now"), ("src/b.ts", "b")]);
        let evaluated = evaluate(
            repo.path(),
            &graph(),
            &[
                annotation("src/a.ts::Alpha", "fresh note", Some(&hash_of("now"))),
                annotation("src/a.ts", "stale note", Some(&hash_of("before"))),
                annotation("src/gone.ts", "dangling note", None),
                annotation("src/b.ts", "no hash, never stale", None),
                annotation("community:src/…", "community label", None),
            ],
        );
        let states: Vec<State> = evaluated.iter().map(|e| e.state).collect();
        assert_eq!(states, [State::Fresh, State::Stale, State::Dangling, State::Fresh, State::Fresh]);
        assert!(evaluated[2].target.is_none() && evaluated[4].target.is_none(), "no node for a missing target or a community");
    }

    #[test]
    fn stale_annotations_stay_in_the_graph_marked_dangling_ones_do_not_and_relations_need_freshness() {
        let repo = repo_with(&[("src/a.ts", "now"), ("src/b.ts", "b")]);
        let mut related = Draft { target: "src/a.ts".into(), relation: Some("references".into()), to: Some("src/b.ts".into()), author: "agent".into(), source_hash: Some(hash_of("now")), ..Draft::default() };
        let fresh_relation = Annotation::new(related.clone(), 1).unwrap();
        related.source_hash = Some(hash_of("before"));
        related.to = Some("src/b.ts".into());
        related.relation = Some("calls".into());
        let stale_relation = Annotation::new(related, 2).unwrap();
        let evaluated = evaluate(repo.path(), &graph(), &[fresh_relation, stale_relation, annotation("src/gone.ts", "gone", None), annotation("src/b.ts", "kept", None)]);
        let d = desired(&evaluated);
        assert_eq!(d.nodes.len(), 3, "fresh relation, stale relation and the plain note; the dangling one is not in the graph");
        assert!(d.nodes.values().any(|p| matches!(p, NodePayload::Annotation { state, .. } if state == "stale")));
        let relation_edges: Vec<EdgeType> = d.edges.values().map(|e| e.2).filter(|t| *t != EdgeType::AnnotatedBy).collect();
        assert_eq!(relation_edges, [EdgeType::References], "only the fresh annotation draws its relation");
        assert!(d.edges.values().all(|e| crate::graph::edge::decode_meta(e.3) == (Confidence::Inferred, EdgeContext::Annotation)), "never extracted, always marked as an annotation");
    }

    #[test]
    fn reconcile_is_empty_when_the_index_already_matches_and_otherwise_says_exactly_what_differs() {
        let repo = repo_with(&[("src/a.ts", "now")]);
        let evaluated = evaluate(repo.path(), &graph(), &[annotation("src/a.ts", "one", None)]);
        let want = desired(&evaluated);
        let existing_nodes: BTreeMap<_, _> = want.nodes.clone();
        let existing_edges: BTreeSet<_> = want.edges.keys().copied().collect();
        let same = reconcile(&want, &existing_nodes, &existing_edges);
        assert!(same.nodes.is_empty() && same.edges.is_empty(), "nothing to do");

        let from_nothing = reconcile(&want, &BTreeMap::new(), &BTreeSet::new());
        assert_eq!((from_nothing.nodes.len(), from_nothing.edges.len()), (1, 1));

        let removed = reconcile(&Desired::default(), &existing_nodes, &existing_edges);
        assert!(removed.nodes.iter().all(|n| matches!(n, NodeMutation::Remove { .. })) && removed.edges.iter().all(|e| matches!(e, EdgeMutation::Remove { .. })));
        assert_eq!((removed.nodes.len(), removed.edges.len()), (1, 1));

        let mut changed = existing_nodes.clone();
        if let Some(NodePayload::Annotation { state, .. }) = changed.values_mut().next() {
            *state = "stale".into();
        }
        let updated = reconcile(&want, &changed, &existing_edges);
        assert_eq!((updated.nodes.len(), updated.edges.len()), (1, 0), "only the changed node is written again");
    }
}
