//! The durable document.
//!
//! Everything that stands between the user's text and losing it: edits that
//! verify what they change, an undo tree that forgets nothing, a journal
//! that makes every edit survive a crash, recovery, and saving that cannot
//! leave a half-written file. No windows, no Typst: this crate can be tested
//! to exhaustion on plain strings.

mod change;
mod disk;
mod document;
mod history;
mod journal;

pub use change::{Buffer, Change, Direction, EditError, Replace};
pub use disk::Hash;
pub use document::{DiskError, DiskStatus, Document, DocumentError, OpenError, Opened};
pub use history::{History, ROOT, StateId, Step};
