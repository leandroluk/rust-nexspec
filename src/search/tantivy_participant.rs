//! [`TantivyParticipant`] — the lexical index's [`crate::sync::SyncParticipant`]
//! implementation (REQ-307, REQ-308). The third real consumer of the trait
//! designed in Fase 0, after `RedbParticipant` and `CsrParticipant`.
//!
//! Owns one long-lived `tantivy::IndexWriter` (creating one per cycle would
//! be wasteful — writer setup involves merge-thread bookkeeping). `stage()`/
//! `commit()`/`abort()` map directly onto Tantivy's own buffer/commit/
//! rollback, per `.specs/features/ast-lexical-search/design.md` → Decision
//! Log. A `stage()` repeated for the same `target_version` before `commit()`
//! is a no-op (does not re-`add_document`, which would otherwise duplicate
//! entries — Tantivy has no upsert-by-id semantics on its own).

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};

use tantivy::directory::MmapDirectory;
use tantivy::{Index, IndexReader, IndexWriter, TantivyDocument};

use crate::search::schema::{SummarySource, TantivySchema, register_tokenizers};
use crate::sync::mutation::{MutationSet, NodeMutation};
use crate::sync::participant::{SyncError, SyncParticipant};

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("tantivy error: {0}")]
    Tantivy(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

fn storage_err<E: std::fmt::Display>(e: E) -> SyncError {
    SyncError::Storage(e.to_string())
}
impl From<SearchError> for SyncError {
    fn from(e: SearchError) -> Self {
        storage_err(e)
    }
}
impl From<tantivy::TantivyError> for SearchError {
    fn from(e: tantivy::TantivyError) -> Self {
        SearchError::Tantivy(e.to_string())
    }
}
impl From<tantivy::directory::error::OpenDirectoryError> for SearchError {
    fn from(e: tantivy::directory::error::OpenDirectoryError) -> Self {
        SearchError::Tantivy(e.to_string())
    }
}
impl From<tantivy::query::QueryParserError> for SearchError {
    fn from(e: tantivy::query::QueryParserError) -> Self {
        SearchError::Tantivy(e.to_string())
    }
}

/// Anything that can be queried: an exact-id lookup or a BM25 text search
/// only needs the reader (for a `Searcher` snapshot) and the schema (for
/// field handles) — implemented by [`TantivyParticipant`] itself and by
/// [`TantivyHandle`], a lightweight clone kept before the participant is
/// moved into a [`crate::sync::coordinator::Coordinator`]'s participant
/// list (mirrors `CsrParticipant::csr_handle()`'s reasoning).
pub trait TantivyQueryable {
    fn reader(&self) -> &IndexReader;
    fn schema(&self) -> &TantivySchema;
}

/// A cheap, cloneable handle for querying after the owning
/// [`TantivyParticipant`] has been moved into a `Coordinator`.
/// `IndexReader` wraps an `Arc` internally, so cloning is cheap and every
/// handle observes the same committed state.
#[derive(Clone)]
pub struct TantivyHandle {
    reader: IndexReader,
    schema: TantivySchema,
}

impl TantivyQueryable for TantivyHandle {
    fn reader(&self) -> &IndexReader {
        &self.reader
    }
    fn schema(&self) -> &TantivySchema {
        &self.schema
    }
}

pub struct TantivyParticipant {
    schema: TantivySchema,
    writer: Mutex<IndexWriter<TantivyDocument>>,
    reader: IndexReader,
    summaries: Mutex<Option<Arc<dyn SummarySource>>>,
    staged_version: Mutex<Option<u64>>,
    committed_version: AtomicU64,
}

/// Opens the index writer, waiting briefly when its lock is still held.
/// Every `Engine` call builds its own participant and drops it when done; on a
/// loaded machine the previous writer's lock can take a moment to disappear
/// (seen as a rare `LockBusy` on CI). The cross-process `SyncLock` already
/// rules out a real second owner, so a short bounded wait is safe.
fn open_writer(index: &Index) -> Result<IndexWriter<TantivyDocument>, SearchError> {
    let mut waited = 0u32;
    loop {
        match index.writer::<TantivyDocument>(50_000_000) {
            Ok(writer) => return Ok(writer),
            Err(tantivy::TantivyError::LockFailure(_, _)) if waited < 80 => {
                waited += 1;
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

impl TantivyParticipant {
    pub fn new(index_path: &Path) -> Result<Self, SearchError> {
        std::fs::create_dir_all(index_path)?;
        let schema = TantivySchema::new();
        let dir = MmapDirectory::open(index_path)?;
        let index = Index::open_or_create(dir, schema.schema.clone())?;
        register_tokenizers(&index);
        let writer = open_writer(&index)?;
        let reader = index.reader()?;
        Ok(Self {
            schema,
            writer: Mutex::new(writer),
            reader,
            summaries: Mutex::new(None),
            staged_version: Mutex::new(None),
            committed_version: AtomicU64::new(0),
        })
    }

    /// Where file summaries come from when a file document is (re)built.
    pub fn set_summary_source(&self, source: Arc<dyn SummarySource>) {
        *self.summaries.lock().unwrap() = Some(source);
    }

    /// A cloneable handle to keep for querying — call this before moving
    /// `self` into a `Coordinator`'s participant list.
    pub fn handle(&self) -> TantivyHandle {
        TantivyHandle {
            reader: self.reader.clone(),
            schema: self.schema.clone(),
        }
    }
}

impl TantivyQueryable for TantivyParticipant {
    fn reader(&self) -> &IndexReader {
        &self.reader
    }
    fn schema(&self) -> &TantivySchema {
        &self.schema
    }
}

impl SyncParticipant for TantivyParticipant {
    fn stage(&self, target_version: u64, mutations: &MutationSet) -> Result<(), SyncError> {
        if self.committed_version()? >= target_version {
            return Ok(()); // idempotent: already applied at or past this version
        }
        let mut staged = self.staged_version.lock().unwrap();
        if *staged == Some(target_version) {
            return Ok(()); // already staged this exact cycle -- skip to avoid duplicate docs
        }

        let writer = self.writer.lock().unwrap();
        for m in &mutations.nodes {
            if let NodeMutation::Upsert { id, payload } = m {
                let mut aligned = rkyv::util::AlignedVec::<16>::new();
                aligned.extend_from_slice(payload);
                let decoded =
                    rkyv::from_bytes::<crate::graph::node::NodePayload, rkyv::rancor::Error>(
                        &aligned,
                    )
                    .map_err(|e| SyncError::Storage(e.to_string()))?;
                let summaries = match (&decoded, self.summaries.lock().unwrap().as_ref()) {
                    (crate::graph::node::NodePayload::File { path, .. }, Some(source)) => source.summaries(path),
                    _ => Default::default(),
                };
                let doc = self.schema.document_with_summaries(id, &decoded, &summaries);
                // An upsert replaces: without this, re-applying a summary would leave two documents.
                writer.delete_term(tantivy::Term::from_field_text(self.schema.id_field, &crate::search::schema::hex(id)));
                writer.add_document(doc).map_err(|e| storage_err(SearchError::from(e)))?;
            }
            // NodeMutation::Remove is not yet reflected in the index (no
            // delete_term call here) -- out of scope for T-307's Done
            // criteria (stage/commit/abort + findable by id/text); documented
            // limitation, same spirit as sync_orchestrator's stale-node note.
        }
        *staged = Some(target_version);
        Ok(())
    }

    fn committed_version(&self) -> Result<u64, SyncError> {
        Ok(self.committed_version.load(Ordering::SeqCst))
    }

    fn commit(&self, target_version: u64) -> Result<(), SyncError> {
        if self.committed_version()? >= target_version {
            return Ok(()); // idempotent: already committed at or past this version
        }
        let mut staged = self.staged_version.lock().unwrap();
        if *staged != Some(target_version) {
            return Err(SyncError::Storage(format!(
                "commit({target_version}) called with nothing staged"
            )));
        }
        let mut writer = self.writer.lock().unwrap();
        writer.commit().map_err(|e| storage_err(SearchError::from(e)))?;
        self.reader.reload().map_err(|e| storage_err(SearchError::from(e)))?;
        self.committed_version.store(target_version, Ordering::SeqCst);
        *staged = None;
        Ok(())
    }

    fn abort(&self, target_version: u64) -> Result<(), SyncError> {
        let mut staged = self.staged_version.lock().unwrap();
        if *staged == Some(target_version) {
            let mut writer = self.writer.lock().unwrap();
            writer.rollback().map_err(|e| storage_err(SearchError::from(e)))?;
            *staged = None;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::node::NodePayload;
    use crate::sync::mutation::{MutationSet, NodeMutation};
    use tantivy::collector::TopDocs;
    use tantivy::query::TermQuery;
    use tantivy::schema::IndexRecordOption;
    use tantivy::Term;
    use tempfile::TempDir;

    fn upsert(id: [u8; 32], name: &str) -> MutationSet {
        let payload = NodePayload::Symbol {
            name: name.to_string(),
            source_hash: id,
            line_start: 1,
            line_end: 2,
        };
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&payload)
            .unwrap()
            .to_vec();
        MutationSet {
            nodes: vec![NodeMutation::Upsert { id, payload: bytes }],
            edges: vec![],
            docs: vec![],
        }
    }

    fn find_by_id_count(p: &TantivyParticipant, id: [u8; 32]) -> usize {
        let searcher = p.reader().searcher();
        let term = Term::from_field_text(p.schema().id_field, &crate::search::schema::hex(&id));
        let query = TermQuery::new(term, IndexRecordOption::Basic);
        searcher
            .search(&query, &TopDocs::with_limit(10).order_by_score())
            .unwrap()
            .len()
    }

    #[test]
    fn stage_then_commit_makes_document_findable() {
        let dir = TempDir::new().unwrap();
        let p = TantivyParticipant::new(dir.path()).unwrap();
        let set = upsert([1u8; 32], "hello_world");

        p.stage(1, &set).unwrap();
        assert_eq!(find_by_id_count(&p, [1u8; 32]), 0, "not visible before commit");

        p.commit(1).unwrap();
        assert_eq!(find_by_id_count(&p, [1u8; 32]), 1);
        assert_eq!(p.committed_version().unwrap(), 1);
    }

    #[test]
    fn stage_then_abort_discards_document() {
        let dir = TempDir::new().unwrap();
        let p = TantivyParticipant::new(dir.path()).unwrap();
        let set = upsert([2u8; 32], "discarded");

        p.stage(5, &set).unwrap();
        p.abort(5).unwrap();

        p.commit(5).unwrap_err(); // nothing staged anymore, per-contract error
        assert_eq!(find_by_id_count(&p, [2u8; 32]), 0);
        assert_eq!(p.committed_version().unwrap(), 0);
    }

    #[test]
    fn stage_is_idempotent_and_commit_does_not_duplicate() {
        let dir = TempDir::new().unwrap();
        let p = TantivyParticipant::new(dir.path()).unwrap();
        let set = upsert([3u8; 32], "once");

        p.stage(1, &set).unwrap();
        p.stage(1, &set).unwrap(); // called twice -- must not duplicate
        p.commit(1).unwrap();
        p.commit(1).unwrap(); // replay after "crash" -- idempotent no-op

        assert_eq!(find_by_id_count(&p, [3u8; 32]), 1);
        assert_eq!(p.committed_version().unwrap(), 1);
    }

    #[test]
    fn committed_document_is_findable_by_bm25_text_search() {
        let dir = TempDir::new().unwrap();
        let p = TantivyParticipant::new(dir.path()).unwrap();
        let set = upsert([4u8; 32], "parse_markdown_file");
        p.stage(1, &set).unwrap();
        p.commit(1).unwrap();

        let searcher = p.reader().searcher();
        let query_parser = tantivy::query::QueryParser::for_index(searcher.index(), vec![p.schema().text_field]);
        let query = query_parser.parse_query("markdown").unwrap();
        let results = searcher
            .search(&query, &TopDocs::with_limit(10).order_by_score())
            .unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn a_new_participant_waits_for_the_previous_writer_to_let_go() {
        let dir = TempDir::new().unwrap();
        let first = TantivyParticipant::new(dir.path()).unwrap();
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            drop(first);
        });
        let started = std::time::Instant::now();
        let second = TantivyParticipant::new(dir.path()).expect("waits instead of failing with LockBusy");
        assert!(started.elapsed() >= std::time::Duration::from_millis(150), "it really had to wait");
        drop(second);
        releaser.join().unwrap();
    }
}
