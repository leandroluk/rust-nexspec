//! [`RedbParticipant`] — the only real [`SyncParticipant`] implementation in
//! Fase 0. Future phases (CSR/Tantivy/HNSW) implement the same trait; this one
//! also doubles as the generic key-value projection of committed nodes/edges/
//! docs until Phase 1 introduces the CSR topology store.

use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

use crate::sync::mutation::{DocMutation, EdgeMutation, MutationSet, NodeMutation, StableId};
use crate::sync::participant::{SyncError, SyncParticipant};

const NODES: TableDefinition<&[u8], &[u8]> = TableDefinition::new("redb_nodes");
const EDGES: TableDefinition<&[u8], &[u8]> = TableDefinition::new("redb_edges");
const DOCS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("redb_docs");
const STAGING_BLOB: TableDefinition<&str, &[u8]> = TableDefinition::new("redb_staging_blob");
const PARTICIPANT_META: TableDefinition<&str, u64> = TableDefinition::new("redb_participant_meta");

const STAGING_BLOB_KEY: &str = "mutations";
const STAGED_VERSION_KEY: &str = "staged_version";
const COMMITTED_VERSION_KEY: &str = "committed_version";

fn storage_err<E: std::fmt::Display>(e: E) -> SyncError {
    SyncError::Storage(e.to_string())
}

/// Encodes an edge's `from`/`to`/`edge_type` alongside its opaque payload.
fn encode_edge(from: &StableId, to: &StableId, edge_type: u16, payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(32 + 32 + 2 + payload.len());
    buf.extend_from_slice(from);
    buf.extend_from_slice(to);
    buf.extend_from_slice(&edge_type.to_le_bytes());
    buf.extend_from_slice(payload);
    buf
}

pub struct RedbParticipant<'a> {
    db: &'a Database,
}

impl<'a> RedbParticipant<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    pub fn get_node(&self, id: &StableId) -> Result<Option<Vec<u8>>, SyncError> {
        self.get_from(NODES, id)
    }

    /// Every committed node, in id order. Used by the report to picture the
    /// whole graph; not for hot paths.
    pub fn all_nodes(&self) -> Result<Vec<(StableId, Vec<u8>)>, SyncError> {
        let tx = self.db.begin_read().map_err(storage_err)?;
        let table = match tx.open_table(NODES) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(e) => return Err(storage_err(e)),
        };
        let mut out = Vec::new();
        for entry in table.iter().map_err(storage_err)? {
            let (key, value) = entry.map_err(storage_err)?;
            let Ok(id) = <StableId>::try_from(key.value()) else { continue };
            out.push((id, value.value().to_vec()));
        }
        Ok(out)
    }

    pub fn get_edge(&self, id: &StableId) -> Result<Option<Vec<u8>>, SyncError> {
        self.get_from(EDGES, id)
    }

    pub fn get_doc(&self, id: &StableId) -> Result<Option<Vec<u8>>, SyncError> {
        self.get_from(DOCS, id)
    }

    fn get_from(
        &self,
        table_def: TableDefinition<&[u8], &[u8]>,
        id: &StableId,
    ) -> Result<Option<Vec<u8>>, SyncError> {
        let tx = self.db.begin_read().map_err(storage_err)?;
        let table = match tx.open_table(table_def) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(storage_err(e)),
        };
        Ok(table
            .get(id.as_slice())
            .map_err(storage_err)?
            .map(|v| v.value().to_vec()))
    }
}

impl<'a> SyncParticipant for RedbParticipant<'a> {
    fn stage(&self, target_version: u64, mutations: &MutationSet) -> Result<(), SyncError> {
        if self.committed_version()? >= target_version {
            return Ok(()); // idempotent: already applied at or past this version
        }
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(mutations).map_err(storage_err)?;

        let tx = self.db.begin_write().map_err(storage_err)?;
        {
            let mut blob = tx.open_table(STAGING_BLOB).map_err(storage_err)?;
            blob.insert(STAGING_BLOB_KEY, bytes.as_slice())
                .map_err(storage_err)?;
            let mut meta = tx.open_table(PARTICIPANT_META).map_err(storage_err)?;
            meta.insert(STAGED_VERSION_KEY, target_version)
                .map_err(storage_err)?;
        }
        tx.commit().map_err(storage_err)?;
        Ok(())
    }

    fn committed_version(&self) -> Result<u64, SyncError> {
        let tx = self.db.begin_read().map_err(storage_err)?;
        let table = match tx.open_table(PARTICIPANT_META) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(0),
            Err(e) => return Err(storage_err(e)),
        };
        Ok(table
            .get(COMMITTED_VERSION_KEY)
            .map_err(storage_err)?
            .map(|v| v.value())
            .unwrap_or(0))
    }

    fn commit(&self, target_version: u64) -> Result<(), SyncError> {
        if self.committed_version()? >= target_version {
            return Ok(()); // idempotent: already committed at or past this version
        }

        let tx = self.db.begin_write().map_err(storage_err)?;
        {
            let staged_bytes = {
                let blob = tx.open_table(STAGING_BLOB).map_err(storage_err)?;
                blob.get(STAGING_BLOB_KEY)
                    .map_err(storage_err)?
                    .map(|v| v.value().to_vec())
            };
            let Some(staged_bytes) = staged_bytes else {
                return Err(SyncError::Storage(format!(
                    "commit({target_version}) called with nothing staged"
                )));
            };
            let mut aligned = rkyv::util::AlignedVec::<16>::new();
            aligned.extend_from_slice(&staged_bytes);
            let mutations = rkyv::from_bytes::<MutationSet, rkyv::rancor::Error>(&aligned)
                .map_err(storage_err)?;

            {
                let mut nodes = tx.open_table(NODES).map_err(storage_err)?;
                for m in &mutations.nodes {
                    match m {
                        NodeMutation::Upsert { id, payload } => {
                            nodes
                                .insert(id.as_slice(), payload.as_slice())
                                .map_err(storage_err)?;
                        }
                        NodeMutation::Remove { id } => {
                            nodes.remove(id.as_slice()).map_err(storage_err)?;
                        }
                    }
                }
            }
            {
                let mut edges = tx.open_table(EDGES).map_err(storage_err)?;
                for m in &mutations.edges {
                    match m {
                        EdgeMutation::Upsert {
                            id,
                            from,
                            to,
                            edge_type,
                            payload,
                        } => {
                            let value = encode_edge(from, to, *edge_type, payload);
                            edges
                                .insert(id.as_slice(), value.as_slice())
                                .map_err(storage_err)?;
                        }
                        EdgeMutation::Remove { id } => {
                            edges.remove(id.as_slice()).map_err(storage_err)?;
                        }
                    }
                }
            }
            {
                let mut docs = tx.open_table(DOCS).map_err(storage_err)?;
                for m in &mutations.docs {
                    match m {
                        DocMutation::Upsert { id, payload } => {
                            docs.insert(id.as_slice(), payload.as_slice())
                                .map_err(storage_err)?;
                        }
                        DocMutation::Remove { id } => {
                            docs.remove(id.as_slice()).map_err(storage_err)?;
                        }
                    }
                }
            }

            let mut blob = tx.open_table(STAGING_BLOB).map_err(storage_err)?;
            blob.remove(STAGING_BLOB_KEY).map_err(storage_err)?;
            let mut meta = tx.open_table(PARTICIPANT_META).map_err(storage_err)?;
            meta.remove(STAGED_VERSION_KEY).map_err(storage_err)?;
            meta.insert(COMMITTED_VERSION_KEY, target_version)
                .map_err(storage_err)?;
        }
        tx.commit().map_err(storage_err)?;
        Ok(())
    }

    fn abort(&self, target_version: u64) -> Result<(), SyncError> {
        let tx = self.db.begin_write().map_err(storage_err)?;
        {
            let staged_version = {
                let meta = tx.open_table(PARTICIPANT_META).map_err(storage_err)?;
                meta.get(STAGED_VERSION_KEY)
                    .map_err(storage_err)?
                    .map(|v| v.value())
            };
            if staged_version == Some(target_version) {
                let mut blob = tx.open_table(STAGING_BLOB).map_err(storage_err)?;
                blob.remove(STAGING_BLOB_KEY).map_err(storage_err)?;
                let mut meta = tx.open_table(PARTICIPANT_META).map_err(storage_err)?;
                meta.remove(STAGED_VERSION_KEY).map_err(storage_err)?;
            }
        }
        tx.commit().map_err(storage_err)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use redb::Database;
    use tempfile::NamedTempFile;

    fn temp_db() -> (NamedTempFile, Database) {
        let file = NamedTempFile::new().unwrap();
        let db = Database::create(file.path()).unwrap();
        (file, db)
    }

    fn node_upsert(id: StableId, payload: &str) -> MutationSet {
        MutationSet {
            nodes: vec![NodeMutation::Upsert {
                id,
                payload: payload.as_bytes().to_vec(),
            }],
            edges: vec![],
            docs: vec![],
        }
    }

    #[test]
    fn stage_then_commit_makes_data_visible() {
        let (_file, db) = temp_db();
        let p = RedbParticipant::new(&db);
        let set = node_upsert([1u8; 32], "hello");

        p.stage(1, &set).unwrap();
        assert!(p.get_node(&[1u8; 32]).unwrap().is_none(), "not visible before commit");

        p.commit(1).unwrap();
        assert_eq!(p.get_node(&[1u8; 32]).unwrap().unwrap(), b"hello");
        assert_eq!(p.committed_version().unwrap(), 1);
    }

    #[test]
    fn stage_then_abort_discards_data() {
        let (_file, db) = temp_db();
        let p = RedbParticipant::new(&db);
        let set = node_upsert([2u8; 32], "discarded");

        p.stage(5, &set).unwrap();
        p.abort(5).unwrap();

        assert!(p.get_node(&[2u8; 32]).unwrap().is_none());
        assert_eq!(p.committed_version().unwrap(), 0);
    }

    #[test]
    fn stage_is_idempotent_and_commit_does_not_duplicate() {
        let (_file, db) = temp_db();
        let p = RedbParticipant::new(&db);
        let set = node_upsert([3u8; 32], "once");

        p.stage(1, &set).unwrap();
        p.stage(1, &set).unwrap(); // called twice — must not error or duplicate
        p.commit(1).unwrap();
        p.commit(1).unwrap(); // replay after "crash" — idempotent no-op

        assert_eq!(p.get_node(&[3u8; 32]).unwrap().unwrap(), b"once");
        assert_eq!(p.committed_version().unwrap(), 1);
    }
}
