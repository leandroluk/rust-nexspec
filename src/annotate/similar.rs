//! `SimilarTo` edges from embeddings (REQ-1808 in `.specs/features/semantic-annotations/spec.md`; decision D9).
//!
//! Opt-in (`sync --similar`) and only in a `full` build with the model present: real vectors are computed first
//! (`sync --embed`), then each node gets edges to its nearest neighbours above a similarity threshold. The
//! edges are `INFERRED` with context `embedding`; the score is kept in the high bits of `Edge.meta`.

use std::collections::BTreeMap;

use crate::code::parser::edge_id;
use crate::graph::edge::{Confidence, EdgeContext, EdgeType, encode_meta_with_score};
use crate::graph::node::NodePayload;
use crate::report::snapshot::GraphSnapshot;
use crate::sync::mutation::{EdgeMutation, MutationSet, StableId};

pub const DEFAULT_K: usize = 3;
pub const DEFAULT_THRESHOLD: f32 = 0.75;

/// One word per camel/snake/path part, lower case: `alphaInvoiceHandler` → `alpha invoice handler`.
pub fn words(text: &str) -> String {
    let mut out = String::new();
    let mut previous_lower = false;
    for c in text.chars() {
        if c.is_alphanumeric() {
            if c.is_uppercase() && previous_lower {
                out.push(' ');
            }
            out.extend(c.to_lowercase());
            previous_lower = c.is_lowercase() || c.is_ascii_digit();
        } else {
            if !out.ends_with(' ') && !out.is_empty() {
                out.push(' ');
            }
            previous_lower = false;
        }
    }
    out.trim().to_string()
}

/// What gets embedded for a node, or `None` for the kinds that are not worth a vector (files, columns, …).
pub fn text_for(snapshot: &GraphSnapshot, id: &StableId) -> Option<String> {
    Some(match snapshot.nodes.get(id)? {
        NodePayload::Requirement { title, body, .. } | NodePayload::Task { title, body, .. } | NodePayload::Adr { title, body, .. } => {
            format!("{title}. {}", body.chars().take(600).collect::<String>())
        }
        NodePayload::DocSection { title, .. } => title.clone(),
        NodePayload::Symbol { name, .. } => match snapshot.path_of(id) {
            Some(path) => format!("{} in {}", words(name), words(path)),
            None => words(name),
        },
        NodePayload::Table { name, is_view, .. } => format!("{} {}", if *is_view { "view" } else { "table" }, words(name)),
        NodePayload::Annotation { label, note, state, .. } if state == "fresh" => format!("{label}. {note}"),
        _ => return None,
    })
}

#[cfg(feature = "full")]
/// Up to `k` neighbours per point with similarity (`1 - distance`) at least `threshold`, strongest first.
pub fn neighbours(index: &crate::vector::HnswIndex, points: &[(StableId, Vec<f32>)], k: usize, threshold: f32) -> Vec<(StableId, StableId, f32)> {
    let mut found = Vec::new();
    for (id, vector) in points {
        let mut near: Vec<(StableId, f32)> = index
            .search(vector, k + 1)
            .into_iter()
            .filter(|(other, _)| other != id)
            .map(|(other, distance)| (other, 1.0 - distance))
            .filter(|(_, similarity)| *similarity >= threshold)
            .collect();
        near.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
        near.truncate(k);
        found.extend(near.into_iter().map(|(other, similarity)| (*id, other, similarity)));
    }
    found.sort_by_key(|(from, to, _)| (*from, *to));
    found
}

/// The `SimilarTo` edges the index should have, against the ones it has (`existing`: id → meta): new or rescored
/// ones are written, the rest that are no longer found are removed.
pub fn reconcile(found: &[(StableId, StableId, f32)], existing: &BTreeMap<StableId, u8>) -> MutationSet {
    let mut set = MutationSet::default();
    let mut wanted: BTreeMap<StableId, (StableId, StableId, u8)> = BTreeMap::new();
    for (from, to, similarity) in found {
        let meta = encode_meta_with_score(Confidence::Inferred, EdgeContext::Embedding, *similarity);
        wanted.insert(edge_id("similar-to", from, to), (*from, *to, meta));
    }
    for (id, (from, to, meta)) in &wanted {
        if existing.get(id) != Some(meta) {
            set.edges.push(EdgeMutation::Upsert { id: *id, from: *from, to: *to, edge_type: EdgeType::SimilarTo.to_code(), payload: vec![*meta] });
        }
    }
    for id in existing.keys().filter(|id| !wanted.contains_key(*id)) {
        set.edges.push(EdgeMutation::Remove { id: *id });
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::snapshot::test_support::*;
    #[cfg(feature = "full")]
    use crate::vector::HnswIndex;
    #[cfg(feature = "full")]
    use std::collections::HashMap;

    #[test]
    fn words_splits_camel_snake_and_paths() {
        assert_eq!(words("alphaInvoiceHandler"), "alpha invoice handler");
        assert_eq!(words("tb_contract_reminder"), "tb contract reminder");
        assert_eq!(words("src/invoice/CurrentHandler.ts"), "src invoice current handler ts");
        assert_eq!(words("HTTPServer"), "httpserver");
    }

    #[test]
    fn text_is_made_for_documents_and_symbols_and_not_for_files() {
        let snap = snapshot(
            vec![(1, file("src/a.ts")), (2, symbol("chargeResident")), (3, requirement("REQ-1")), (4, task("T-1"))],
            vec![edge(1, 2, 1, crate::graph::edge::EdgeType::DefinedIn)],
        );
        assert_eq!(text_for(&snap, &id(2)).as_deref(), Some("charge resident in src a ts"));
        assert!(text_for(&snap, &id(3)).unwrap().starts_with("REQ-1. "));
        assert_eq!(text_for(&snap, &id(1)), None, "a file path says little; its symbols carry the words");
    }

    #[cfg(feature = "full")]
    fn index_of(vectors: &[(u8, [f32; 2])]) -> (HnswIndex, Vec<(StableId, Vec<f32>)>) {
        let points: Vec<(StableId, Vec<f32>)> = vectors.iter().map(|(n, v)| ([*n; 32], v.to_vec())).collect();
        let index = HnswIndex::new();
        index.rebuild(&points.iter().cloned().collect::<HashMap<_, _>>());
        (index, points)
    }

    #[cfg(feature = "full")]
    #[test]
    fn each_point_gets_its_close_neighbours_above_the_threshold_and_never_itself() {
        let (index, points) = index_of(&[(1, [1.0, 0.0]), (2, [0.95, 0.05]), (3, [0.0, 1.0]), (4, [0.1, 0.99])]);
        let found = neighbours(&index, &points, 3, 0.75);
        let pairs: Vec<(u8, u8)> = found.iter().map(|(a, b, _)| (a[0], b[0])).collect();
        assert_eq!(pairs, [(1, 2), (2, 1), (3, 4), (4, 3)], "two tight pairs, nothing across");
        assert!(found.iter().all(|(a, b, s)| a != b && *s >= 0.75 && *s <= 1.0001));
        assert!(neighbours(&index, &points, 3, 0.9999).is_empty(), "a high bar leaves nothing");
    }

    #[cfg(feature = "full")]
    #[test]
    fn k_limits_the_neighbours_per_point() {
        let (index, points) = index_of(&[(1, [1.0, 0.0]), (2, [0.99, 0.01]), (3, [0.98, 0.02]), (4, [0.97, 0.03])]);
        let found = neighbours(&index, &points, 2, 0.5);
        for n in 1u8..=4 {
            assert_eq!(found.iter().filter(|(a, _, _)| a[0] == n).count(), 2, "point {n}");
        }
    }

    #[test]
    fn reconcile_writes_what_is_new_or_rescored_and_removes_what_is_gone() {
        let found = [([1u8; 32], [2u8; 32], 0.9f32), ([2u8; 32], [1u8; 32], 0.9)];
        let from_nothing = reconcile(&found, &BTreeMap::new());
        assert_eq!(from_nothing.edges.len(), 2);
        let existing: BTreeMap<StableId, u8> = from_nothing
            .edges
            .iter()
            .filter_map(|e| if let EdgeMutation::Upsert { id, payload, .. } = e { Some((*id, payload[0])) } else { None })
            .collect();
        assert!(reconcile(&found, &existing).edges.is_empty(), "nothing changed, nothing written");
        let rescored = reconcile(&[([1u8; 32], [2u8; 32], 0.8f32), ([2u8; 32], [1u8; 32], 0.9)], &existing);
        assert_eq!(rescored.edges.len(), 1, "only the edge whose score moved");
        let gone = reconcile(&[], &existing);
        assert!(gone.edges.iter().all(|e| matches!(e, EdgeMutation::Remove { .. })) && gone.edges.len() == 2);
    }
}
