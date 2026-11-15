//! The journal: an append-only file that holds the document's history.
//!
//! Every change to the document is written here before it counts as made.
//! After a crash the journal alone is enough to rebuild the text and the
//! undo tree: it starts from a snapshot of the text and each record carries
//! both sides of its changes.
//!
//! The file is a magic number followed by records. A record is its payload's
//! length, the payload's CRC-32, and the payload. A record that is cut short
//! or fails its checksum ends the journal: it and everything after it never
//! happened, which is exactly what a crash in the middle of a write means.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::time::{Instant, SystemTime};

use crate::change::Change;
use crate::disk::{self, Hash};
use crate::history::StateId;

const MAGIC: &[u8; 8] = b"ZWJOURN1";

/// Larger payloads are taken for corruption rather than allocated.
const MAX_PAYLOAD: usize = 1 << 30;

/// One entry of the journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Record {
    /// The text of state `state` in full.
    Snapshot { state: StateId, text: String },
    /// A new state `id`, reached from `parent` (the current state) by
    /// `changes`. It becomes the current state.
    Push {
        id: StateId,
        parent: StateId,
        changes: Vec<Change>,
    },
    /// Further changes folded into the current state `state`.
    Extend {
        state: StateId,
        changes: Vec<Change>,
    },
    /// The current state moved to `state`: an undo, a redo, or a jump.
    Goto { state: StateId },
    /// Redo from `state` leads to `child`.
    Prefer { state: StateId, child: StateId },
    /// The file on disk holds state `state`; `hash` is of the file's bytes.
    Saved { state: StateId, hash: Hash },
}

impl Record {
    /// The record as it is stored: length, checksum, payload.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut payload = Vec::new();
        match self {
            Self::Snapshot { state, text } => {
                payload.push(1);
                put_u64(&mut payload, *state);
                put_bytes(&mut payload, text.as_bytes());
            }
            Self::Push {
                id,
                parent,
                changes,
            } => {
                payload.push(2);
                put_u64(&mut payload, *id);
                put_u64(&mut payload, *parent);
                put_changes(&mut payload, changes);
            }
            Self::Extend { state, changes } => {
                payload.push(3);
                put_u64(&mut payload, *state);
                put_changes(&mut payload, changes);
            }
            Self::Goto { state } => {
                payload.push(4);
                put_u64(&mut payload, *state);
            }
            Self::Prefer { state, child } => {
                payload.push(5);
                put_u64(&mut payload, *state);
                put_u64(&mut payload, *child);
            }
            Self::Saved { state, hash } => {
                payload.push(6);
                put_u64(&mut payload, *state);
                payload.extend_from_slice(hash);
            }
        }
        let mut framed = Vec::with_capacity(payload.len() + 8);
        framed.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        framed.extend_from_slice(&crc32fast::hash(&payload).to_le_bytes());
        framed.extend_from_slice(&payload);
        framed
    }

    fn decode(payload: &[u8]) -> Option<Self> {
        let mut reader = Reader(payload);
        let record = match reader.u8()? {
            1 => Self::Snapshot {
                state: reader.u64()?,
                text: reader.string()?,
            },
            2 => Self::Push {
                id: reader.u64()?,
                parent: reader.u64()?,
                changes: reader.changes()?,
            },
            3 => Self::Extend {
                state: reader.u64()?,
                changes: reader.changes()?,
            },
            4 => Self::Goto {
                state: reader.u64()?,
            },
            5 => Self::Prefer {
                state: reader.u64()?,
                child: reader.u64()?,
            },
            6 => Self::Saved {
                state: reader.u64()?,
                hash: reader.take(32)?.try_into().ok()?,
            },
            _ => return None,
        };
        // Bytes left over mean the payload is not what its tag says.
        reader.0.is_empty().then_some(record)
    }
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    put_u64(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn put_changes(out: &mut Vec<u8>, changes: &[Change]) {
    put_u64(out, changes.len() as u64);
    for change in changes {
        put_u64(out, change.at as u64);
        put_bytes(out, change.old.as_bytes());
        put_bytes(out, change.new.as_bytes());
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let (taken, rest) = self.0.split_at_checked(count)?;
        self.0 = rest;
        Some(taken)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|bytes| bytes[0])
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn string(&mut self) -> Option<String> {
        let len = usize::try_from(self.u64()?).ok()?;
        String::from_utf8(self.take(len)?.to_vec()).ok()
    }

    fn changes(&mut self) -> Option<Vec<Change>> {
        let count = usize::try_from(self.u64()?).ok()?;
        // Each change takes at least 24 bytes; a count that cannot fit in
        // what is left is corruption, not a reason to allocate.
        if count > self.0.len() / 24 {
            return None;
        }
        (0..count)
            .map(|_| {
                Some(Change {
                    at: usize::try_from(self.u64()?).ok()?,
                    old: self.string()?,
                    new: self.string()?,
                })
            })
            .collect()
    }
}

/// The records of a journal file's contents, up to the first one that is
/// incomplete or damaged, and the number of bytes those records take.
/// `None` if `bytes` is not a journal at all.
#[must_use]
pub fn decode(bytes: &[u8]) -> Option<(Vec<Record>, usize)> {
    let mut rest = bytes.strip_prefix(MAGIC)?;
    let mut records = Vec::new();
    let mut valid = MAGIC.len();
    while let Some((header, body)) = rest.split_at_checked(8) {
        let len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let crc = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
        if len > MAX_PAYLOAD {
            break;
        }
        let Some((payload, after)) = body.split_at_checked(len) else {
            break;
        };
        if crc32fast::hash(payload) != crc {
            break;
        }
        let Some(record) = Record::decode(payload) else {
            break;
        };
        records.push(record);
        valid += 8 + len;
        rest = after;
    }
    Some((records, valid))
}

/// A journal file open for appending.
pub struct Journal {
    file: File,
    /// When the oldest record not yet forced to disk was written.
    unsynced_since: Option<Instant>,
}

impl Journal {
    /// Replaces the journal at `path` with one holding `records`, and opens
    /// it. The replacement is atomic: after a crash the file holds either
    /// the old journal or the new one.
    pub fn create(path: &Path, records: &[Record]) -> io::Result<Self> {
        let mut contents = MAGIC.to_vec();
        for record in records {
            contents.extend_from_slice(&record.encode());
        }
        disk::write_atomically(path, &contents)?;
        let file = OpenOptions::new().append(true).open(path)?;
        Ok(Self {
            file,
            unsynced_since: None,
        })
    }

    /// Writes `record` at the end of the journal.
    ///
    /// When this returns, the record is with the operating system and
    /// survives the process being killed. It survives a power failure once
    /// [`Journal::sync`] has returned.
    pub fn append(&mut self, record: &Record) -> io::Result<()> {
        // One write call per record: a record is either written or not,
        // as far as other readers of the file and a killed process go.
        self.file.write_all(&record.encode())?;
        self.unsynced_since.get_or_insert_with(Instant::now);
        Ok(())
    }

    /// Forces everything appended so far onto the disk.
    pub fn sync(&mut self) -> io::Result<()> {
        if self.unsynced_since.is_some() {
            self.file.sync_data()?;
            self.unsynced_since = None;
        }
        Ok(())
    }

    /// When the oldest record that a power failure would still lose was
    /// written, if there is one.
    #[must_use]
    pub fn unsynced_since(&self) -> Option<Instant> {
        self.unsynced_since
    }

    /// Makes the file say it was last written at `when`.
    pub fn set_modified(&self, when: SystemTime) -> io::Result<()> {
        self.file.set_modified(when)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples() -> Vec<Record> {
        let change = |at, old: &str, new: &str| Change {
            at,
            old: old.into(),
            new: new.into(),
        };
        vec![
            Record::Snapshot {
                state: 0,
                text: "grüße, wörld\n".into(),
            },
            Record::Push {
                id: 1,
                parent: 0,
                changes: vec![change(0, "", "a"), change(3, "üß", "")],
            },
            Record::Extend {
                state: 1,
                changes: vec![change(1, "", "b")],
            },
            Record::Goto { state: 0 },
            Record::Prefer { state: 0, child: 1 },
            Record::Saved {
                state: 1,
                hash: [7; 32],
            },
            Record::Push {
                id: 2,
                parent: 0,
                changes: vec![],
            },
        ]
    }

    fn encoded(records: &[Record]) -> Vec<u8> {
        let mut bytes = MAGIC.to_vec();
        for record in records {
            bytes.extend_from_slice(&record.encode());
        }
        bytes
    }

    #[test]
    fn records_survive_a_round_trip() {
        let records = samples();
        let bytes = encoded(&records);
        assert_eq!(decode(&bytes), Some((records, bytes.len())));
    }

    #[test]
    fn a_torn_tail_ends_the_journal_at_the_last_whole_record() {
        let records = samples();
        let bytes = encoded(&records);
        let whole = encoded(&records[..6]).len();
        // Cut anywhere inside the last record: the first six remain.
        for cut in whole..bytes.len() {
            assert_eq!(
                decode(&bytes[..cut]),
                Some((records[..6].to_vec(), whole)),
                "cut at {cut}"
            );
        }
    }

    #[test]
    fn a_damaged_record_ends_the_journal_before_it() {
        let records = samples();
        let mut bytes = encoded(&records);
        let before = encoded(&records[..2]).len();
        // Flip a bit in the third record's payload.
        bytes[before + 12] ^= 0x40;
        assert_eq!(decode(&bytes), Some((records[..2].to_vec(), before)));
    }

    #[test]
    fn other_files_are_not_journals() {
        assert_eq!(decode(b""), None);
        assert_eq!(decode(b"= A Typst document"), None);
        assert_eq!(decode(MAGIC), Some((vec![], 8)));
    }

    #[test]
    fn absurd_lengths_are_corruption_not_allocations() {
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(&[0; 4]);
        assert_eq!(decode(&bytes), Some((vec![], 8)));

        // A change count far beyond what the payload could hold.
        let mut payload = vec![3];
        put_u64(&mut payload, 1);
        put_u64(&mut payload, u64::MAX);
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&crc32fast::hash(&payload).to_le_bytes());
        bytes.extend_from_slice(&payload);
        assert_eq!(decode(&bytes), Some((vec![], 8)));
    }
}
