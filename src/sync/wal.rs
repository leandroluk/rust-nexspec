//! Write-Ahead Log (REQ-001, REQ-004 in
//! `.specs/features/sync-coordinator/spec.md`). Every mutation the coordinator
//! intends to apply is durably appended here — `fsync`ed — before any
//! [`crate::sync::participant::SyncParticipant`] is touched. See
//! `.specs/features/sync-coordinator/design.md` → "WAL — formato de frame".
//!
//! Frame layout (all integers little-endian):
//! `[u8 frame_type][u32 body_len][u64 target_version][body bytes][u32 crc32(body)]`
//!
//! `frame_type`: `0` = mutation frame (body = rkyv-serialized [`MutationSet`]),
//! `1` = commit-marker (body empty, marks `target_version` as fully committed).
//!
//! A frame that is truncated (not enough bytes left in the file for its
//! declared `body_len`) or whose `crc32` doesn't match is treated as an
//! incomplete write — silently discarded, never partially applied — since it
//! can only occur at the tail of the file after a crash mid-`append_frame`.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::sync::mutation::MutationSet;

const FRAME_TYPE_MUTATION: u8 = 0;
const FRAME_TYPE_COMMIT_MARKER: u8 = 1;
/// `[u8 frame_type][u32 body_len][u64 target_version]`.
const FRAME_HEADER_LEN: u64 = 13;

#[derive(Debug, thiserror::Error)]
pub enum WalError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("mutation set (de)serialization error: {0}")]
    Codec(String),
}

pub struct Wal {
    path: PathBuf,
}

impl Wal {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, WalError> {
        let path = path.into();
        // Ensure the file exists so reads on a fresh WAL see a clean empty file.
        OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self { path })
    }

    /// Durably append a mutation frame for `target_version`. Returns only
    /// after the write has been `fsync`ed.
    pub fn append_frame(
        &self,
        target_version: u64,
        mutations: &MutationSet,
    ) -> Result<(), WalError> {
        let body = rkyv::to_bytes::<rkyv::rancor::Error>(mutations)
            .map_err(|e| WalError::Codec(e.to_string()))?;
        self.append_raw_frame(FRAME_TYPE_MUTATION, target_version, &body)
    }

    /// Durably mark `target_version` as fully committed by every participant.
    pub fn mark_done(&self, target_version: u64) -> Result<(), WalError> {
        self.append_raw_frame(FRAME_TYPE_COMMIT_MARKER, target_version, &[])
    }

    fn append_raw_frame(
        &self,
        frame_type: u8,
        target_version: u64,
        body: &[u8],
    ) -> Result<(), WalError> {
        let mut file = OpenOptions::new().append(true).open(&self.path)?;
        let mut buf = Vec::with_capacity(1 + 4 + 8 + body.len() + 4);
        buf.push(frame_type);
        buf.extend_from_slice(&(body.len() as u32).to_le_bytes());
        buf.extend_from_slice(&target_version.to_le_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(&crc32fast::hash(body).to_le_bytes());
        file.write_all(&buf)?;
        file.sync_data()?;
        Ok(())
    }

    /// Empties the log when every mutation frame in it has a commit-marker
    /// (REQ-908 in `.specs/features/performance-guard/spec.md`): finished
    /// cycles are only history, and without this the file grew by the size of
    /// every sync forever (113 MB on a mid-sized repository). Returns whether
    /// it truncated. Call it only after the marker is durable.
    pub fn truncate_if_idle(&self) -> Result<bool, WalError> {
        if !self.is_idle()? {
            return Ok(false);
        }
        let file = OpenOptions::new().write(true).open(&self.path)?;
        file.set_len(0)?;
        file.sync_data()?;
        Ok(true)
    }

    /// Header-only scan (bodies are skipped, not decoded): `true` when no
    /// mutation frame lacks its commit-marker. A frame cut short at the tail
    /// is what [`Self::pending_frames`] discards too, so it does not count.
    fn is_idle(&self) -> Result<bool, WalError> {
        let file = File::open(&self.path)?;
        let len = file.metadata()?.len();
        let mut reader = BufReader::new(file);
        let mut open_versions: std::collections::BTreeSet<u64> = std::collections::BTreeSet::new();
        let mut position = 0u64;
        while len - position >= FRAME_HEADER_LEN {
            let mut header = [0u8; FRAME_HEADER_LEN as usize];
            reader.read_exact(&mut header)?;
            let frame_type = header[0];
            let body_len = u32::from_le_bytes(header[1..5].try_into().unwrap()) as u64;
            let target_version = u64::from_le_bytes(header[5..13].try_into().unwrap());
            let frame_end = position + FRAME_HEADER_LEN + body_len + 4;
            if frame_end > len {
                break; // truncated tail
            }
            match frame_type {
                FRAME_TYPE_MUTATION => {
                    open_versions.insert(target_version);
                }
                FRAME_TYPE_COMMIT_MARKER => {
                    open_versions.remove(&target_version);
                }
                _ => return Ok(false), // unknown content: never destroy it
            }
            reader.seek(SeekFrom::Start(frame_end))?;
            position = frame_end;
        }
        Ok(open_versions.is_empty())
    }

    /// Versions that have a mutation frame with no matching commit-marker,
    /// together with the mutations staged for them, in file order. In normal
    /// operation there is at most one (a single sync cycle in flight).
    pub fn pending_frames(&self) -> Result<Vec<(u64, MutationSet)>, WalError> {
        let mut pending: BTreeMap<u64, MutationSet> = BTreeMap::new();
        for frame in read_frames(&self.path)? {
            match frame {
                Frame::Mutation {
                    target_version,
                    mutations,
                } => {
                    pending.insert(target_version, mutations);
                }
                Frame::CommitMarker { target_version } => {
                    pending.remove(&target_version);
                }
            }
        }
        Ok(pending.into_iter().collect())
    }
}

enum Frame {
    Mutation {
        target_version: u64,
        mutations: MutationSet,
    },
    CommitMarker {
        target_version: u64,
    },
}

fn read_frames(path: &Path) -> Result<Vec<Frame>, WalError> {
    let mut file = File::open(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;

    let mut frames = Vec::new();
    let mut cursor = 0usize;
    loop {
        // Header: 1 (type) + 4 (len) + 8 (version) = 13 bytes.
        if bytes.len() - cursor < 13 {
            break; // clean EOF or truncated header — nothing more to read
        }
        let frame_type = bytes[cursor];
        let body_len =
            u32::from_le_bytes(bytes[cursor + 1..cursor + 5].try_into().unwrap()) as usize;
        let target_version = u64::from_le_bytes(bytes[cursor + 5..cursor + 13].try_into().unwrap());
        let body_start = cursor + 13;
        let body_end = body_start + body_len;
        let crc_end = body_end + 4;
        if bytes.len() < crc_end {
            break; // truncated mid-write — discard this incomplete tail frame
        }
        let body = &bytes[body_start..body_end];
        let stored_crc = u32::from_le_bytes(bytes[body_end..crc_end].try_into().unwrap());
        if crc32fast::hash(body) != stored_crc {
            break; // corrupted/incomplete frame — treat as if never fully written
        }

        match frame_type {
            FRAME_TYPE_MUTATION => {
                // `body` is a slice into a plain `Vec<u8>` read from disk, not
                // aligned for rkyv's archived types — copy into an AlignedVec
                // before decoding.
                let mut aligned = rkyv::util::AlignedVec::<16>::new();
                aligned.extend_from_slice(body);
                let mutations = rkyv::from_bytes::<MutationSet, rkyv::rancor::Error>(&aligned)
                    .map_err(|e| WalError::Codec(e.to_string()))?;
                frames.push(Frame::Mutation {
                    target_version,
                    mutations,
                });
            }
            FRAME_TYPE_COMMIT_MARKER => {
                frames.push(Frame::CommitMarker { target_version });
            }
            _ => break, // unknown frame type — treat rest of file as unreadable tail
        }

        cursor = crc_end;
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use tempfile::NamedTempFile;

    fn temp_wal() -> (NamedTempFile, Wal) {
        let file = NamedTempFile::new().unwrap();
        let wal = Wal::open(file.path()).unwrap();
        (file, wal)
    }

    fn set_with_node(byte: u8) -> MutationSet {
        let mut set = MutationSet::default();
        set.nodes.push(crate::sync::mutation::NodeMutation::Remove { id: [byte; 32] });
        set
    }

    #[test]
    fn truncate_if_idle_empties_a_fully_committed_log() {
        let (file, wal) = temp_wal();
        wal.append_frame(1, &set_with_node(1)).unwrap();
        wal.mark_done(1).unwrap();
        assert!(std::fs::metadata(file.path()).unwrap().len() > 0);

        assert!(wal.truncate_if_idle().unwrap());
        assert_eq!(std::fs::metadata(file.path()).unwrap().len(), 0);
        assert!(wal.pending_frames().unwrap().is_empty());

        // The log is still usable afterwards.
        wal.append_frame(2, &set_with_node(2)).unwrap();
        assert_eq!(wal.pending_frames().unwrap().len(), 1);
    }

    #[test]
    fn truncate_if_idle_keeps_a_pending_frame() {
        let (file, wal) = temp_wal();
        wal.append_frame(1, &set_with_node(1)).unwrap();
        wal.mark_done(1).unwrap();
        wal.append_frame(2, &set_with_node(2)).unwrap(); // no marker: still in flight
        let before = std::fs::metadata(file.path()).unwrap().len();

        assert!(!wal.truncate_if_idle().unwrap());
        assert_eq!(std::fs::metadata(file.path()).unwrap().len(), before);
        assert_eq!(wal.pending_frames().unwrap().len(), 1);
    }

    #[test]
    fn truncate_if_idle_on_an_empty_or_torn_log_is_safe() {
        let (file, wal) = temp_wal();
        assert!(wal.truncate_if_idle().unwrap(), "empty log counts as idle");

        wal.append_frame(1, &set_with_node(1)).unwrap();
        wal.mark_done(1).unwrap();
        // A torn frame at the tail (crash mid-append) is discarded by
        // pending_frames, so it must not block truncation either.
        let mut raw = OpenOptions::new().append(true).open(file.path()).unwrap();
        raw.write_all(&[FRAME_TYPE_MUTATION, 200, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3]).unwrap();
        assert!(wal.truncate_if_idle().unwrap());
        assert_eq!(std::fs::metadata(file.path()).unwrap().len(), 0);
    }

    #[test]
    fn append_then_read_normal_frame() {
        let (_file, wal) = temp_wal();
        let set = MutationSet {
            nodes: vec![],
            edges: vec![],
            docs: vec![],
        };
        wal.append_frame(1, &set).unwrap();

        let pending = wal.pending_frames().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, 1);
        assert_eq!(pending[0].1, set);
    }

    #[test]
    fn truncated_trailing_frame_is_ignored() {
        let (file, wal) = temp_wal();
        wal.append_frame(1, &MutationSet::default()).unwrap();

        // Simulate a crash mid-write: truncate the last few bytes of the file.
        let len = std::fs::metadata(file.path()).unwrap().len();
        let f = OpenOptions::new().write(true).open(file.path()).unwrap();
        f.set_len(len - 3).unwrap();
        drop(f);

        let pending = wal.pending_frames().unwrap();
        assert!(pending.is_empty(), "truncated frame must not be observed as pending");
    }

    #[test]
    fn mark_done_removes_frame_from_pending() {
        let (_file, wal) = temp_wal();
        wal.append_frame(1, &MutationSet::default()).unwrap();
        assert_eq!(wal.pending_frames().unwrap().len(), 1);

        wal.mark_done(1).unwrap();
        assert!(wal.pending_frames().unwrap().is_empty());
    }
}
