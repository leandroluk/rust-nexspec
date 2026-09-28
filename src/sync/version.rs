//! [`VersionPointer`] — the atomic `sync_version` (REQ-002 in
//! `.specs/features/sync-coordinator/spec.md`). Readers must never trust data
//! past this version; it is the single value that makes a sync cycle "visible".

use redb::{Database, ReadableDatabase, TableDefinition};

const META_TABLE: TableDefinition<&str, u64> = TableDefinition::new("meta");
const SYNC_VERSION_KEY: &str = "sync_version";

// Separate table for non-u64 metadata (byte blobs). Same `redb` file, same
// transactional path as `META_TABLE` — just a different value type.
const META_BYTES_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("meta_bytes");
const LAST_INDEXED_COMMIT_KEY: &str = "last_indexed_commit";

#[derive(Debug, thiserror::Error)]
pub enum VersionError {
    #[error("redb error: {0}")]
    Redb(String),
}

impl From<redb::Error> for VersionError {
    fn from(e: redb::Error) -> Self {
        VersionError::Redb(e.to_string())
    }
}
impl From<redb::TransactionError> for VersionError {
    fn from(e: redb::TransactionError) -> Self {
        VersionError::Redb(e.to_string())
    }
}
impl From<redb::TableError> for VersionError {
    fn from(e: redb::TableError) -> Self {
        VersionError::Redb(e.to_string())
    }
}
impl From<redb::StorageError> for VersionError {
    fn from(e: redb::StorageError) -> Self {
        VersionError::Redb(e.to_string())
    }
}
impl From<redb::CommitError> for VersionError {
    fn from(e: redb::CommitError) -> Self {
        VersionError::Redb(e.to_string())
    }
}

/// Wraps the `meta` table in `metadata.redb` to expose the `sync_version`
/// pointer. Absence of the key means version 0 (nothing synced yet).
pub struct VersionPointer<'a> {
    db: &'a Database,
}

impl<'a> VersionPointer<'a> {
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    pub fn current(&self) -> Result<u64, VersionError> {
        let tx = self.db.begin_read()?;
        let table = match tx.open_table(META_TABLE) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(0),
            Err(e) => return Err(e.into()),
        };
        Ok(table.get(SYNC_VERSION_KEY)?.map(|v| v.value()).unwrap_or(0))
    }

    pub fn bump(&self, new_version: u64) -> Result<(), VersionError> {
        let tx = self.db.begin_write()?;
        {
            let mut table = tx.open_table(META_TABLE)?;
            table.insert(SYNC_VERSION_KEY, new_version)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The Git commit OID last fully indexed (REQ-202 in
    /// `.specs/features/git-integration/spec.md`) — `None` means "never
    /// indexed", not an error.
    pub fn last_indexed_commit(&self) -> Result<Option<[u8; 20]>, VersionError> {
        let tx = self.db.begin_read()?;
        let table = match tx.open_table(META_BYTES_TABLE) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let Some(guard) = table.get(LAST_INDEXED_COMMIT_KEY)? else {
            return Ok(None);
        };
        let bytes = guard.value();
        if bytes.len() != 20 {
            return Err(VersionError::Redb(format!(
                "corrupt last_indexed_commit: expected 20 bytes, got {}",
                bytes.len()
            )));
        }
        let mut oid = [0u8; 20];
        oid.copy_from_slice(bytes);
        Ok(Some(oid))
    }

    pub fn set_last_indexed_commit(&self, oid: [u8; 20]) -> Result<(), VersionError> {
        let tx = self.db.begin_write()?;
        {
            let mut table = tx.open_table(META_BYTES_TABLE)?;
            table.insert(LAST_INDEXED_COMMIT_KEY, oid.as_slice())?;
        }
        tx.commit()?;
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

    #[test]
    fn defaults_to_zero_when_absent() {
        let (_file, db) = temp_db();
        let vp = VersionPointer::new(&db);
        assert_eq!(vp.current().unwrap(), 0);
    }

    #[test]
    fn bump_then_current_reflects_new_value() {
        let (_file, db) = temp_db();
        let vp = VersionPointer::new(&db);
        vp.bump(42).unwrap();
        assert_eq!(vp.current().unwrap(), 42);
        vp.bump(43).unwrap();
        assert_eq!(vp.current().unwrap(), 43);
    }

    #[test]
    fn last_indexed_commit_defaults_to_none() {
        let (_file, db) = temp_db();
        let vp = VersionPointer::new(&db);
        assert_eq!(vp.last_indexed_commit().unwrap(), None);
    }

    #[test]
    fn set_then_get_last_indexed_commit_roundtrips() {
        let (_file, db) = temp_db();
        let vp = VersionPointer::new(&db);
        let oid = [7u8; 20];
        vp.set_last_indexed_commit(oid).unwrap();
        assert_eq!(vp.last_indexed_commit().unwrap(), Some(oid));
    }
}
