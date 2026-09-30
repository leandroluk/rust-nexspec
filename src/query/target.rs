//! One resolver for every query target (REQ-1108): a name, `path:Symbol`, a
//! requirement marker or a hex id. Ambiguity is never settled silently: the
//! candidates come back and the caller shows them.

use crate::graph::node::NodePayload;
use crate::report::snapshot::GraphSnapshot;
use crate::search::unhex;
use crate::sync::mutation::StableId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub id: StableId,
    /// `Name (path)` for symbols, the path for files, the marker for requirements.
    pub label: String,
    pub kind: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    One(StableId),
    /// Several nodes match at the same level, ordered by label.
    Ambiguous(Vec<Candidate>),
    /// Nothing matches; these labels are the nearest by name.
    NotFound(Vec<String>),
}

fn candidate(snapshot: &GraphSnapshot, id: &StableId) -> Candidate {
    Candidate { id: *id, label: snapshot.label(id), kind: snapshot.kind_name(id) }
}

fn verdict(snapshot: &GraphSnapshot, mut ids: Vec<StableId>) -> Option<Resolved> {
    match ids.len() {
        0 => None,
        1 => Some(Resolved::One(ids.remove(0))),
        _ => {
            let mut candidates: Vec<Candidate> = ids.iter().map(|id| candidate(snapshot, id)).collect();
            candidates.sort_by(|a, b| a.label.cmp(&b.label).then(a.id.cmp(&b.id)));
            Some(Resolved::Ambiguous(candidates))
        }
    }
}

fn path_matches(path: &str, wanted: &str) -> bool {
    let wanted = wanted.trim_start_matches("./").replace('\\', "/");
    path == wanted || path.ends_with(&format!("/{wanted}"))
}

fn is_marker(target: &str) -> bool {
    ["REQ-", "TASK-", "ADR-"].iter().any(|p| target.starts_with(p) && target.len() > p.len())
}

pub fn resolve(snapshot: &GraphSnapshot, target: &str) -> Resolved {
    let target = target.trim();

    // 1. A full node id.
    if let Some(id) = unhex(target)
        && snapshot.nodes.contains_key(&id)
    {
        return Resolved::One(id);
    }

    // 2. A requirement / task / ADR marker.
    if is_marker(target) {
        let ids: Vec<StableId> = snapshot
            .nodes
            .iter()
            .filter(|(_, p)| matches!(p, NodePayload::Requirement { title, .. } | NodePayload::Task { title, .. } | NodePayload::Adr { title, .. } if title == target))
            .map(|(id, _)| *id)
            .collect();
        if let Some(found) = verdict(snapshot, ids) {
            return found;
        }
    }

    // 3. `path/fragment:Symbol`.
    if let Some((path_part, name)) = target.rsplit_once(':')
        && !path_part.is_empty()
        && !name.is_empty()
    {
        let ids: Vec<StableId> = snapshot
            .nodes
            .iter()
            .filter(|(id, p)| {
                matches!(p, NodePayload::Symbol { name: n, .. } if n == name)
                    && snapshot.path_of(id).is_some_and(|path| path_matches(path, path_part))
            })
            .map(|(id, _)| *id)
            .collect();
        if let Some(found) = verdict(snapshot, ids) {
            return found;
        }
    }

    // 4. A file path (or its tail).
    if target.contains('/') || target.contains('\\') || target.rsplit_once('.').is_some_and(|(_, ext)| !ext.is_empty() && ext.len() <= 5) {
        let ids: Vec<StableId> = snapshot
            .nodes
            .iter()
            .filter(|(_, p)| matches!(p, NodePayload::File { path, .. } if path_matches(path, target)))
            .map(|(id, _)| *id)
            .collect();
        if let Some(found) = verdict(snapshot, ids) {
            return found;
        }
    }

    // 5. An exact symbol name, then the same ignoring case.
    for exact in [true, false] {
        let ids: Vec<StableId> = snapshot
            .nodes
            .iter()
            .filter(|(_, p)| {
                matches!(p, NodePayload::Symbol { name, .. } if if exact { name == target } else { name.eq_ignore_ascii_case(target) })
            })
            .map(|(id, _)| *id)
            .collect();
        if let Some(found) = verdict(snapshot, ids) {
            return found;
        }
    }

    // 6. Nothing: offer the nearest names.
    let lowered = target.to_lowercase();
    let mut suggestions: Vec<String> = snapshot
        .nodes
        .iter()
        .filter(|(_, p)| !matches!(p, NodePayload::DocSection { .. }))
        .map(|(id, _)| snapshot.label(id))
        .filter(|label| !lowered.is_empty() && label.to_lowercase().contains(&lowered))
        .collect();
    suggestions.sort_by(|a, b| a.len().cmp(&b.len()).then(a.cmp(b)));
    suggestions.dedup();
    suggestions.truncate(5);
    Resolved::NotFound(suggestions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::edge::EdgeType;
    use crate::report::snapshot::test_support::*;

    fn world() -> GraphSnapshot {
        snapshot(
            vec![
                (1, file("pkgs/cache/src/cache.port.ts")),
                (2, file("pkgs/sql/src/sql.port.ts")),
                (3, symbol("Port")),
                (4, symbol("Port")),
                (5, symbol("CachePort")),
                (6, requirement("REQ-CTC-001")),
                (7, file("apps/api/src/app.module.ts")),
                (8, symbol("AppModule")),
            ],
            vec![
                edge(1, 3, 1, EdgeType::DefinedIn),
                edge(2, 4, 2, EdgeType::DefinedIn),
                edge(3, 5, 1, EdgeType::DefinedIn),
                edge(4, 8, 7, EdgeType::DefinedIn),
            ],
        )
    }

    #[test]
    fn hex_ids_markers_and_unique_names_resolve_directly() {
        let snap = world();
        assert_eq!(resolve(&snap, &crate::engine::id_hex(&id(5))), Resolved::One(id(5)));
        assert_eq!(resolve(&snap, "REQ-CTC-001"), Resolved::One(id(6)));
        assert_eq!(resolve(&snap, "CachePort"), Resolved::One(id(5)));
        assert_eq!(resolve(&snap, "cacheport"), Resolved::One(id(5)), "case-insensitive fallback");
        assert_eq!(resolve(&snap, "app.module.ts"), Resolved::One(id(7)), "a file by its tail");
        assert_eq!(resolve(&snap, "apps/api/src/app.module.ts"), Resolved::One(id(7)));
    }

    #[test]
    fn a_name_shared_by_two_symbols_is_ambiguous_and_path_qualified_resolves_it() {
        let snap = world();
        match resolve(&snap, "Port") {
            Resolved::Ambiguous(candidates) => {
                let labels: Vec<&str> = candidates.iter().map(|c| c.label.as_str()).collect();
                assert_eq!(labels, vec!["Port (pkgs/cache/src/cache.port.ts)", "Port (pkgs/sql/src/sql.port.ts)"]);
                assert!(candidates.iter().all(|c| c.kind == "symbol"));
            }
            other => panic!("expected ambiguity, got {other:?}"),
        }
        assert_eq!(resolve(&snap, "cache/src/cache.port.ts:Port"), Resolved::One(id(3)));
        assert_eq!(resolve(&snap, "sql.port.ts:Port"), Resolved::One(id(4)));
    }

    #[test]
    fn an_unknown_target_returns_nearby_names() {
        let snap = world();
        match resolve(&snap, "port") {
            // "port" is not an exact name anywhere, but is contained in several.
            Resolved::NotFound(names) => {
                assert!(names.iter().any(|n| n.contains("CachePort")), "{names:?}");
                assert!(names.len() <= 5);
            }
            Resolved::Ambiguous(c) => {
                // Case-insensitive match of `Port` symbols is also acceptable and explicit.
                assert_eq!(c.len(), 2);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(resolve(&snap, "NoSuchThing"), Resolved::NotFound(vec![]));
        assert_eq!(resolve(&snap, ""), Resolved::NotFound(vec![]));
    }

    #[test]
    fn a_path_fragment_that_matches_two_files_is_ambiguous() {
        let snap = snapshot(vec![(1, file("a/index.ts")), (2, file("b/index.ts"))], vec![]);
        assert!(matches!(resolve(&snap, "index.ts"), Resolved::Ambiguous(c) if c.len() == 2));
        assert_eq!(resolve(&snap, "a/index.ts"), Resolved::One(id(1)));
    }
}
