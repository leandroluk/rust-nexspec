//! Query helpers over [`TantivyParticipant`] (REQ-307): exact-id fast path
//! (`TermQuery` on the untokenized `id` field — a term lookup, not a scan)
//! and free-text BM25 ranking over `text`.

use tantivy::collector::TopDocs;
use tantivy::query::{QueryParser, TermQuery};
use tantivy::schema::IndexRecordOption;
use tantivy::{TantivyDocument, Term};

use crate::search::schema::hex;
use crate::search::tantivy_participant::{SearchError, TantivyParticipant};
use crate::sync::mutation::StableId;

/// Exact lookup by stable id — a single `TermQuery` on the untokenized `id`
/// field, not a linear scan.
pub fn find_by_id(
    participant: &TantivyParticipant,
    id: &StableId,
) -> Result<Option<TantivyDocument>, SearchError> {
    let searcher = participant.reader().searcher();
    let term = Term::from_field_text(participant.schema().id_field, &hex(id));
    let query = TermQuery::new(term, IndexRecordOption::Basic);
    let top = searcher.search(&query, &TopDocs::with_limit(1).order_by_score())?;
    match top.first() {
        Some((_score, addr)) => Ok(Some(searcher.doc(*addr)?)),
        None => Ok(None),
    }
}

/// Free-text BM25 search over the `text` field, ranked, top `limit` results.
pub fn search_text(
    participant: &TantivyParticipant,
    query_text: &str,
    limit: usize,
) -> Result<Vec<TantivyDocument>, SearchError> {
    let searcher = participant.reader().searcher();
    let query_parser = QueryParser::for_index(searcher.index(), vec![participant.schema().text_field]);
    let query = query_parser.parse_query(query_text)?;
    let top = searcher.search(&query, &TopDocs::with_limit(limit).order_by_score())?;
    top.into_iter()
        .map(|(_score, addr)| searcher.doc(addr).map_err(SearchError::from))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::node::NodePayload;
    use crate::sync::mutation::{MutationSet, NodeMutation};
    use crate::sync::participant::SyncParticipant;
    use tantivy::schema::Value;
    use tempfile::TempDir;

    fn participant_with(id: StableId, name: &str) -> (TempDir, TantivyParticipant) {
        let dir = TempDir::new().unwrap();
        let p = TantivyParticipant::new(dir.path()).unwrap();
        let payload = NodePayload::Symbol {
            name: name.to_string(),
            source_hash: id,
            line_start: 1,
            line_end: 2,
        };
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&payload)
            .unwrap()
            .to_vec();
        let set = MutationSet {
            nodes: vec![NodeMutation::Upsert { id, payload: bytes }],
            edges: vec![],
            docs: vec![],
        };
        p.stage(1, &set).unwrap();
        p.commit(1).unwrap();
        (dir, p)
    }

    #[test]
    fn find_by_id_returns_the_exact_document() {
        let id = [7u8; 32];
        let (_dir, p) = participant_with(id, "stable_id_handler");

        let doc = find_by_id(&p, &id).unwrap().expect("document must be found");
        let stored_id = doc
            .get_first(p.schema().id_field)
            .and_then(|v| v.as_str())
            .unwrap();
        assert_eq!(stored_id, hex(&id));
    }

    #[test]
    fn find_by_id_returns_none_for_unknown_id() {
        let (_dir, p) = participant_with([1u8; 32], "something");
        assert!(find_by_id(&p, &[99u8; 32]).unwrap().is_none());
    }

    #[test]
    fn search_text_finds_matching_term_without_false_positive() {
        let (_dir, p) = participant_with([2u8; 32], "stable id handler");

        let hits = search_text(&p, "stable", 10).unwrap();
        assert_eq!(hits.len(), 1);

        let no_hits = search_text(&p, "completely_unrelated_zzz", 10).unwrap();
        assert!(no_hits.is_empty());
    }
}
