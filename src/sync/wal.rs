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
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::sync::mutation::MutationSet;

const FRAME_TYPE_MUTATION: u8 = 0;
const FRAME_TYPE_COMMIT_MARKER: u8 = 1;

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
