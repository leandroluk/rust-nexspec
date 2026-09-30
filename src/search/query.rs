//! Query helpers over a `TantivyParticipant` (REQ-307): exact-id fast path
//! (`TermQuery` on the untokenized `id` field — a term lookup, not a scan)
//! and free-text BM25 ranking over `text`.

use tantivy::collector::TopDocs;
use tantivy::query::{QueryParser, TermQuery};
use tantivy::schema::IndexRecordOption;
use tantivy::{TantivyDocument, Term};

use crate::search::schema::hex;
use crate::search::tantivy_participant::{SearchError, TantivyQueryable};
use crate::sync::mutation::StableId;

/// Exact lookup by stable id — a single `TermQuery` on the untokenized `id`
/// field, not a linear scan. Works with any [`TantivyQueryable`]
/// (`TantivyParticipant` or a `TantivyHandle` kept from before it was moved
/// into a `Coordinator`).
pub fn find_by_id(
    participant: &impl TantivyQueryable,
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

/// Weight of the `summary_*` fields relative to `text` (REQ-1907): `NEXSPEC_ENRICH_WEIGHT`,
/// default 0.5; 0 leaves the summaries out of the query.
pub fn summary_weight_from_env() -> f32 {
    std::env::var("NEXSPEC_ENRICH_WEIGHT")
        .ok()
        .and_then(|v| v.trim().parse::<f32>().ok())
        .filter(|w| w.is_finite() && *w >= 0.0)
        .unwrap_or(0.5)
}

/// Free-text BM25 search over `text` and the file summaries, ranked, top `limit` results.
pub fn search_text(
    participant: &impl TantivyQueryable,
    query_text: &str,
    limit: usize,
) -> Result<Vec<TantivyDocument>, SearchError> {
    search_text_weighted(participant, query_text, limit, summary_weight_from_env())
}

/// [`search_text`] with an explicit summary weight (`0.0` = `text` only, what `--no-enrich` asks for).
pub fn search_text_weighted(
    participant: &impl TantivyQueryable,
    query_text: &str,
    limit: usize,
    summary_weight: f32,
) -> Result<Vec<TantivyDocument>, SearchError> {
    let searcher = participant.reader().searcher();
    let schema = participant.schema();
    let mut fields = vec![schema.text_field];
    if summary_weight > 0.0 {
        fields.extend(schema.summary_fields.iter().map(|(_, f)| *f));
    }
    let mut query_parser = QueryParser::for_index(searcher.index(), fields);
    if summary_weight > 0.0 {
        for (_, field) in &schema.summary_fields {
            query_parser.set_field_boost(*field, summary_weight);
        }
    }
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
    use crate::search::tantivy_participant::TantivyParticipant;
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

    #[test]
    fn identifier_parts_are_searchable_but_whole_identifiers_still_match() {
        let (_dir, p) = participant_with([3u8; 32], "CsrDeltaCompactor");

        for query in ["csr delta", "compactor", "CsrDeltaCompactor", "csrdeltacompactor"] {
            assert_eq!(search_text(&p, query, 10).unwrap().len(), 1, "{query:?} must reach CsrDeltaCompactor");
        }
        assert!(search_text(&p, "unrelatedword", 10).unwrap().is_empty());
    }

    struct FixedSummaries(std::collections::BTreeMap<String, String>);

    impl crate::search::schema::SummarySource for FixedSummaries {
        fn summaries(&self, _path: &str) -> std::collections::BTreeMap<String, String> {
            self.0.clone()
        }
    }

    fn file_participant(id: StableId, path: &str, summaries: &[(&str, &str)]) -> (TempDir, TantivyParticipant) {
        let dir = TempDir::new().unwrap();
        let p = TantivyParticipant::new(dir.path()).unwrap();
        p.set_summary_source(std::sync::Arc::new(FixedSummaries(summaries.iter().map(|(l, s)| (l.to_string(), s.to_string())).collect())));
        let payload = NodePayload::File { path: path.to_string(), source_hash: id };
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&payload).unwrap().to_vec();
        let set = MutationSet { nodes: vec![NodeMutation::Upsert { id, payload: bytes }], edges: vec![], docs: vec![] };
        p.stage(1, &set).unwrap();
        p.commit(1).unwrap();
        (dir, p)
    }

    #[test]
    fn a_prose_question_reaches_a_file_through_its_summary_in_any_language() {
        let (_dir, p) = file_participant(
            [4u8; 32],
            "src/lease/create-lease.usecase.ts",
            &[("en", "Creates a tenant contract and validates its rental terms"), ("ru", "Создаёт договор аренды для арендатора")],
        );
        assert!(search_text_weighted(&p, "how is a tenant contract created", 10, 0.0).unwrap().is_empty(), "text alone cannot answer prose");
        assert_eq!(search_text_weighted(&p, "how is a tenant contract created", 10, 0.5).unwrap().len(), 1);
        assert_eq!(search_text_weighted(&p, "договоры аренды", 10, 0.5).unwrap().len(), 1, "Russian stemming: договоры -> договор");
    }

    #[test]
    fn re_applying_a_summary_replaces_the_document() {
        let (_dir, p) = file_participant([5u8; 32], "src/a.ts", &[("en", "billing invoices")]);
        let payload = NodePayload::File { path: "src/a.ts".to_string(), source_hash: [5u8; 32] };
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&payload).unwrap().to_vec();
        let set = MutationSet { nodes: vec![NodeMutation::Upsert { id: [5u8; 32], payload: bytes }], edges: vec![], docs: vec![] };
        p.stage(2, &set).unwrap();
        p.commit(2).unwrap();
        assert_eq!(search_text_weighted(&p, "invoices", 10, 0.5).unwrap().len(), 1, "one document, not two");
    }
}
