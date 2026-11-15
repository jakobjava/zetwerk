//! A document: text, its history, and their life on disk.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::change::{self, Buffer, Direction, EditError, Replace};
use crate::disk::{self, Hash, Lock};
use crate::history::{History, ROOT, StateId};
use crate::journal::{self, Journal, Record};

const BOM: &str = "\u{feff}";

/// A file being edited.
///
/// Every state the text is in while a `Document` exists has been written to
/// the document's journal first. Whatever happens to the process, opening
/// the file again brings back the text as it last was.
pub struct Document<B: Buffer> {
    /// The file, with symbolic links resolved.
    path: PathBuf,
    buffer: B,
    /// Whether the file starts with a byte order mark. It is not part of the
    /// text, and is written back on save.
    bom: bool,
    history: History,
    journal: Journal,
    _lock: Lock,
    /// The state whose text is in the file, if any state's is.
    saved: Option<StateId>,
    /// Hash of the file's bytes when last read or written; `None` if there
    /// was no file.
    disk_hash: Option<Hash>,
    revision: u64,
    /// When the unsaved changes that opening brought back from the
    /// journal were last made, until the text is saved or loaded.
    restored: Option<SystemTime>,
}

/// What opening a document found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opened {
    /// The text is the file's. Nothing was pending.
    Clean,
    /// The text has changes that were never saved, brought back from the
    /// journal. The file is untouched; the document is modified.
    Restored,
    /// The file was changed by something else since zetwerk last saw it, and
    /// there was nothing unsaved. The text is the file's, as one step on top
    /// of the history: undo brings back the text as zetwerk last had it.
    ChangedWhileClosed,
    /// The file was changed by something else *and* the journal holds
    /// changes that were never saved. The text is the file's, as one step
    /// on top of the history, so that undo brings back the unsaved
    /// version; that version is also handed over here, and a copy of the
    /// journal as it was is kept at `journal`. Neither version is lost and
    /// the user decides.
    Conflict { recovered: String, journal: PathBuf },
    /// A journal existed but could not be read. The text is the file's; the
    /// journal is kept at `journal`.
    JournalUnusable { journal: PathBuf },
}

/// Why a document could not be opened.
#[derive(Debug)]
pub enum OpenError {
    Io(PathBuf, io::Error),
    NotUtf8(PathBuf),
    /// Another zetwerk process has this file open.
    AlreadyOpen(PathBuf),
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(path, err) => write!(f, "{}: {err}", path.display()),
            Self::NotUtf8(path) => write!(f, "{} is not valid UTF-8", path.display()),
            Self::AlreadyOpen(path) => {
                write!(f, "{} is already open in another zetwerk", path.display())
            }
        }
    }
}

impl std::error::Error for OpenError {}

/// Why the document was not changed. The text is as it was before.
#[derive(Debug)]
pub enum DocumentError {
    Edit(EditError),
    /// The change could not be written to the journal. It was not made:
    /// a change that would not survive a crash is not accepted.
    Journal(io::Error),
}

impl fmt::Display for DocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Edit(err) => err.fmt(f),
            Self::Journal(err) => write!(f, "cannot write the journal: {err}"),
        }
    }
}

impl std::error::Error for DocumentError {}

impl From<EditError> for DocumentError {
    fn from(err: EditError) -> Self {
        Self::Edit(err)
    }
}

/// Why saving or reloading did not happen as asked.
#[derive(Debug)]
pub enum DiskError {
    /// The file is not what zetwerk last read or wrote: something else
    /// changed it. Nothing was written.
    ChangedOnDisk,
    /// The file does not exist.
    Missing,
    NotUtf8,
    Io(io::Error),
    /// The file was written, but the journal could not record that. The
    /// save itself stands.
    Journal(io::Error),
    Document(DocumentError),
}

impl fmt::Display for DiskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ChangedOnDisk => f.write_str("the file was changed by another program"),
            Self::Missing => f.write_str("the file no longer exists"),
            Self::NotUtf8 => f.write_str("the file is not valid UTF-8"),
            Self::Io(err) => err.fmt(f),
            Self::Journal(err) => write!(f, "saved, but cannot write the journal: {err}"),
            Self::Document(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for DiskError {}

/// How the file relates to what zetwerk last read or wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskStatus {
    Unchanged,
    /// Something else wrote the file.
    Changed,
    /// There was a file and now there is none.
    Missing,
}

/// What is in a file: its text without the byte order mark, whether it had
/// one, and the hash of its bytes.
struct FileContents {
    text: String,
    bom: bool,
    hash: Hash,
}

fn read_file(path: &Path) -> Result<Option<FileContents>, DiskError> {
    let Some(bytes) = disk::read(path).map_err(DiskError::Io)? else {
        return Ok(None);
    };
    let hash = disk::hash(&bytes);
    let text = String::from_utf8(bytes).map_err(|_| DiskError::NotUtf8)?;
    Ok(Some(match text.strip_prefix(BOM) {
        Some(rest) => FileContents {
            text: rest.to_owned(),
            bom: true,
            hash,
        },
        None => FileContents {
            text,
            bom: false,
            hash,
        },
    }))
}

/// The bytes a file holding `text` consists of.
fn file_bytes(bom: bool, text: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len() + BOM.len());
    if bom {
        bytes.extend_from_slice(BOM.as_bytes());
    }
    bytes.extend_from_slice(text.as_bytes());
    bytes
}

/// A journal read back: the history it describes, the text of the state the
/// history is at, and the state last known to be in the file.
struct Replayed {
    history: History,
    text: String,
    saved: Option<(StateId, Hash)>,
    /// Whether every record was used. If not, a record contradicted the
    /// ones before it, which a crash alone cannot cause.
    complete: bool,
}

/// Rebuilds history and text from journal records. `None` if the records
/// never say what the text is.
fn replay(records: &[Record]) -> Option<Replayed> {
    let mut history = History::new();
    let mut text: Option<String> = None;
    let mut saved = None;
    let mut used = 0;
    for record in records {
        // A record that does not fit what came before ends the replay:
        // the state reached so far is consistent, the rest cannot be
        // trusted.
        if replay_record(&mut history, &mut text, &mut saved, record).is_none() {
            break;
        }
        used += 1;
    }
    Some(Replayed {
        history,
        text: text?,
        saved,
        complete: used == records.len(),
    })
}

/// Applies one record. On `None`, history and text are as before.
fn replay_record(
    history: &mut History,
    text: &mut Option<String>,
    saved: &mut Option<(StateId, Hash)>,
    record: &Record,
) -> Option<()> {
    match record {
        Record::Push {
            id,
            parent,
            changes,
        } => {
            if *id != history.len() as StateId || *parent != history.current() {
                return None;
            }
            if let Some(text) = text {
                change::apply(text, changes, Direction::Forward).ok()?;
            }
            history.push(changes.clone());
        }
        Record::Extend { state, changes } => {
            if *state != history.current() || !history.can_extend() {
                return None;
            }
            if let Some(text) = text {
                change::apply(text, changes, Direction::Forward).ok()?;
            }
            history.extend(changes.clone());
        }
        Record::Goto { state } => travel(history, text.as_mut(), *state)?,
        Record::Snapshot {
            state,
            text: snapshot,
        } => {
            let from = history.current();
            travel(history, text.as_mut(), *state)?;
            if text.as_ref().is_some_and(|known| known != snapshot) {
                // Going back the way we came cannot fail.
                travel(history, text.as_mut(), from);
                return None;
            }
            *text = Some(snapshot.clone());
        }
        Record::Prefer { state, child } => {
            history.set_redo_target(*state, *child).then_some(())?;
        }
        Record::Saved { state, hash } => {
            history.contains(*state).then_some(())?;
            *saved = Some((*state, *hash));
        }
    }
    Some(())
}

/// Moves `history` to state `to`, taking `text` along if it is known. On
/// `None`, both are as before.
fn travel<B: Buffer>(history: &mut History, mut text: Option<&mut B>, to: StateId) -> Option<()> {
    let path = history.path(history.current(), to)?;
    // The changes of each step, in the direction the step takes them.
    let mut steps = Vec::with_capacity(path.len());
    let mut from = history.current();
    for &next in &path {
        let step = if history.parent(from) == Some(next) {
            (history.changes(from)?, Direction::Backward)
        } else {
            (history.changes(next)?, Direction::Forward)
        };
        steps.push(step);
        from = next;
    }
    if let Some(text) = text.as_mut() {
        for (done, (changes, direction)) in steps.iter().enumerate() {
            if change::apply(*text, changes, *direction).is_err() {
                // Each of these was just applied, so taking it back works.
                for (changes, direction) in steps[..done].iter().rev() {
                    let _ = change::apply(*text, changes, opposite(*direction));
                }
                return None;
            }
        }
    }
    for next in path {
        history.step_to(next);
    }
    Some(())
}

fn opposite(direction: Direction) -> Direction {
    match direction {
        Direction::Forward => Direction::Backward,
        Direction::Backward => Direction::Forward,
    }
}

/// The shortest journal that describes `history` at its current state.
fn compact(history: &History, text: &str, saved: Option<(StateId, Hash)>) -> Vec<Record> {
    let mut records = Vec::new();
    let mut at = ROOT;
    for id in 1..history.len() as StateId {
        let (Some(parent), Some(changes)) = (history.parent(id), history.changes(id)) else {
            continue;
        };
        if at != parent {
            records.push(Record::Goto { state: parent });
        }
        records.push(Record::Push {
            id,
            parent,
            changes: changes.to_vec(),
        });
        at = id;
    }
    if at != history.current() {
        records.push(Record::Goto {
            state: history.current(),
        });
    }
    // After all moves: moving up the tree changes where redo leads.
    for state in 0..history.len() as StateId {
        if let Some(child) = history.redo_target(state) {
            records.push(Record::Prefer { state, child });
        }
    }
    records.push(Record::Snapshot {
        state: history.current(),
        text: text.to_owned(),
    });
    if let Some((state, hash)) = saved {
        records.push(Record::Saved { state, hash });
    }
    records
}

/// Looks for a state of `history` whose text, as a file, has `hash`.
/// `text` is the text of the current state.
fn state_with_hash(history: &History, text: &str, bom: bool, hash: Hash) -> Option<StateId> {
    let mut history = history.clone();
    let mut text = text.to_owned();
    (0..history.len() as StateId).rev().find(|&state| {
        travel(&mut history, Some(&mut text), state).is_some()
            && disk::hash(&file_bytes(bom, &text)) == hash
    })
}

/// Where the journal of `path` is kept under `state_dir`.
fn journal_path(state_dir: &Path, path: &Path) -> PathBuf {
    let name = blake3::hash(path.as_os_str().as_encoded_bytes()).to_hex();
    state_dir
        .join("journals")
        .join(format!("{}.journal", &name[..32]))
}

/// A name beside `journal` that no file has yet, for keeping it as it is
/// because of `why`.
fn aside(journal: &Path, why: &str) -> PathBuf {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let mut kept = journal.with_extension(format!("{why}-{seconds}"));
    let mut attempt = 0;
    while kept.exists() {
        attempt += 1;
        kept = journal.with_extension(format!("{why}-{seconds}-{attempt}"));
    }
    kept
}

/// Moves a journal that will not be used out of the way without deleting
/// it. Returns where it went.
fn set_aside(journal: &Path, why: &str) -> io::Result<PathBuf> {
    let kept = aside(journal, why);
    std::fs::rename(journal, &kept)?;
    Ok(kept)
}

/// A history to carry on with: the history, the text of its current state,
/// and the state whose text is in the file.
type Adopted = (History, String, Option<StateId>);

/// Decides what the journal at `journal_path` means for a file whose bytes
/// hash to `file_hash` (`None`: there is no file). Returns the outcome and,
/// if the journal's history is to be continued, that history.
fn reconcile(
    journal_path: &Path,
    bom: bool,
    file_hash: Option<Hash>,
) -> io::Result<(Opened, Option<Adopted>)> {
    let Some(bytes) = disk::read(journal_path)?.filter(|bytes| !bytes.is_empty()) else {
        return Ok((Opened::Clean, None));
    };
    let Some(replayed) = journal::decode(&bytes).and_then(|(records, _)| replay(&records)) else {
        let journal = set_aside(journal_path, "unusable")?;
        return Ok((Opened::JournalUnusable { journal }, None));
    };
    if !replayed.complete {
        // Something beyond a crash went wrong with this journal. Use what
        // is consistent, keep the original for inspection.
        std::fs::copy(journal_path, journal_path.with_extension("damaged"))?;
    }
    let Replayed {
        history,
        text,
        saved,
        ..
    } = replayed;
    let current = history.current();
    let current_hash = disk::hash(&file_bytes(bom, &text));

    let Some(hash) = file_hash else {
        // No file. If the journal holds text, the file is gone but its
        // text is not.
        let opened = if text.is_empty() {
            Opened::Clean
        } else {
            Opened::Restored
        };
        return Ok((opened, Some((history, text, None))));
    };
    if hash == current_hash {
        return Ok((Opened::Clean, Some((history, text, Some(current)))));
    }
    // The file is not the journal's current text. If it is some other state
    // of this history, the difference is unsaved work; if it is none,
    // someone else wrote it.
    let known = saved
        .filter(|(_, saved_hash)| *saved_hash == hash)
        .map(|(state, _)| state)
        .or_else(|| state_with_hash(&history, &text, bom, hash));
    if let Some(state) = known {
        return Ok((Opened::Restored, Some((history, text, Some(state)))));
    }
    let unsaved = match saved {
        Some((state, _)) => state != current,
        None => !text.is_empty(),
    };
    // Either way the history goes on and the file's text is put on top of
    // it (see `Document::open`): what zetwerk had is one undo away, never
    // dropped.
    if unsaved {
        let journal = aside(journal_path, "conflict");
        std::fs::copy(journal_path, &journal)?;
        Ok((
            Opened::Conflict {
                recovered: text.clone(),
                journal,
            },
            Some((history, text, None)),
        ))
    } else {
        Ok((Opened::ChangedWhileClosed, Some((history, text, None))))
    }
}

/// Whether the journal under `state_dir` of the file at `path` holds
/// changes that were never saved and that the file does not hold: what
/// opening the file would bring back, or hand over if the file changed
/// since. Only reads: for files of a document that are not open, so that
/// unsaved work in them is not forgotten.
#[must_use]
pub fn has_unsaved_changes(path: &Path, state_dir: &Path) -> bool {
    let path = std::fs::canonicalize(path)
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_owned());
    let Ok(Some(bytes)) = disk::read(&journal_path(state_dir, &path)) else {
        return false;
    };
    let Some(replayed) = journal::decode(&bytes).and_then(|(records, _)| replay(&records)) else {
        return false;
    };
    let unsaved = match replayed.saved {
        Some((state, _)) => state != replayed.history.current(),
        None => !replayed.text.is_empty(),
    };
    let in_file = match read_file(&path) {
        Ok(Some(file)) => file.text == replayed.text,
        Ok(None) => replayed.text.is_empty(),
        Err(_) => false,
    };
    unsaved && !in_file
}

/// Removes temporary files that a session which died while saving left next
/// to the document. That directory is the user's, so only a file that
/// provably holds a text of this very document is removed; anything else is
/// not ours to judge.
fn remove_own_leftovers(
    path: &Path,
    file_hash: Option<Hash>,
    history: &History,
    text: &str,
    bom: bool,
) {
    for leftover in disk::leftovers(path) {
        let ours = disk::read(&leftover).ok().flatten().is_some_and(|bytes| {
            let hash = disk::hash(&bytes);
            Some(hash) == file_hash || state_with_hash(history, text, bom, hash).is_some()
        });
        if ours {
            let _ = std::fs::remove_file(leftover);
        }
    }
}

impl<B: Buffer> Document<B> {
    /// Opens the file at `path` for editing, creating the text buffer with
    /// `buffer`. A file that does not exist yet opens as empty text.
    ///
    /// Journals live under `state_dir`. If the file's journal shows that an
    /// earlier session ended without saving, its changes are brought back;
    /// see [`Opened`] for the possible outcomes.
    pub fn open(
        path: &Path,
        state_dir: &Path,
        buffer: impl FnOnce(String) -> B,
    ) -> Result<(Self, Opened), OpenError> {
        let io = |at: &Path| {
            let at = at.to_owned();
            move |err| OpenError::Io(at, err)
        };
        // One file, one journal, whatever name the file was opened by.
        let path = match std::fs::canonicalize(path) {
            Ok(resolved) => resolved,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                std::path::absolute(path).map_err(io(path))?
            }
            Err(err) => return Err(OpenError::Io(path.to_owned(), err)),
        };
        let journal_path = journal_path(state_dir, &path);
        // When the last session last wrote it, before it is compacted.
        let last_written = std::fs::metadata(&journal_path)
            .and_then(|meta| meta.modified())
            .ok();
        let journal_dir = journal_path.parent().unwrap_or(state_dir);
        disk::create_private_dir(journal_dir).map_err(io(journal_dir))?;
        let lock_path = journal_path.with_extension("lock");
        let lock = Lock::acquire(&lock_path)
            .map_err(io(&lock_path))?
            .ok_or_else(|| OpenError::AlreadyOpen(path.clone()))?;
        // The journal directory is ours and this journal is locked: any
        // half-written replacement of it is from a session that died.
        for leftover in disk::leftovers(&journal_path) {
            let _ = std::fs::remove_file(leftover);
        }

        let file = read_file(&path).map_err(|err| match err {
            DiskError::NotUtf8 => OpenError::NotUtf8(path.clone()),
            DiskError::Io(err) => OpenError::Io(path.clone(), err),
            other => OpenError::Io(path.clone(), io::Error::other(other.to_string())),
        })?;
        let bom = file.as_ref().is_some_and(|file| file.bom);
        let file_hash = file.as_ref().map(|file| file.hash);

        let (opened, adopted) =
            reconcile(&journal_path, bom, file_hash).map_err(io(&journal_path))?;
        let (history, text, saved) = adopted.unwrap_or_else(|| {
            let text = file.map(|file| file.text).unwrap_or_default();
            (History::new(), text, file_hash.map(|_| ROOT))
        });
        remove_own_leftovers(&path, file_hash, &history, &text, bom);
        let saved_record = saved.zip(file_hash);
        let journal = Journal::create(&journal_path, &compact(&history, &text, saved_record))
            .map_err(io(&journal_path))?;
        let restored = last_written.filter(|_| opened == Opened::Restored);
        if let Some(when) = restored {
            // Compacting is no change: how old the unsaved changes are is
            // still known when the file is opened the next time.
            let _ = journal.set_modified(when);
        }
        let mut document = Self {
            path,
            buffer: buffer(text),
            bom,
            history,
            journal,
            _lock: lock,
            saved,
            disk_hash: file_hash,
            revision: 0,
            restored,
        };
        if matches!(opened, Opened::ChangedWhileClosed | Opened::Conflict { .. }) {
            // The file's text, as one undo step on top of the history.
            let path = document.path.clone();
            document
                .reload()
                .map_err(|err| OpenError::Io(path, io::Error::other(err.to_string())))?;
            document.revision = 0;
        }
        Ok((document, opened))
    }

    /// When the unsaved changes that opening brought back from the
    /// journal (see [`Opened::Restored`]) were last made, as far as the
    /// journal's file tells; `None` if none were brought back, or once
    /// the text has been saved or loaded from the file since.
    #[must_use]
    pub fn restored(&self) -> Option<SystemTime> {
        self.restored
    }

    /// The file, with symbolic links resolved.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn buffer(&self) -> &B {
        &self.buffer
    }

    #[must_use]
    pub fn text(&self) -> &str {
        self.buffer.text()
    }

    #[must_use]
    pub fn history(&self) -> &History {
        &self.history
    }

    /// The state the text is in.
    #[must_use]
    pub fn state(&self) -> StateId {
        self.history.current()
    }

    /// Counts every change of the text since the document was opened,
    /// undo and redo included.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Whether the text differs from what is in the file. With no file,
    /// only text that would be lost counts: an empty buffer is unmodified.
    #[must_use]
    pub fn is_modified(&self) -> bool {
        match self.saved {
            Some(saved) => saved != self.history.current(),
            None => !self.buffer.text().is_empty(),
        }
    }

    /// Applies `edits` one after the other; each range refers to the text
    /// as the edits before it left it. Either all are applied or none.
    ///
    /// The edits form one undo step, or, with `join`, become part of the
    /// previous step where that is possible (typing a word is many edits
    /// and one undo). Returns the state the text is in afterwards.
    pub fn edit(&mut self, edits: &[Replace], join: bool) -> Result<StateId, DocumentError> {
        if edits.is_empty() {
            return Ok(self.history.current());
        }
        let changes = change::perform(&mut self.buffer, edits)?;
        let current = self.history.current();
        // Never fold changes into the saved state: it would stop being it.
        let join = join && self.history.can_extend() && self.saved != Some(current);
        let record = if join {
            Record::Extend {
                state: current,
                changes: changes.clone(),
            }
        } else {
            let id = self.history.len() as StateId;
            Record::Push {
                id,
                parent: current,
                changes: changes.clone(),
            }
        };
        if let Err(err) = self.journal.append(&record) {
            // Just applied, so taking it back cannot fail.
            let _ = change::apply(&mut self.buffer, &changes, Direction::Backward);
            return Err(DocumentError::Journal(err));
        }
        if join {
            self.history.extend(changes);
        } else {
            self.history.push(changes);
        }
        self.revision += 1;
        Ok(self.history.current())
    }

    /// Takes back the last undo step. Returns the state reached, or `None`
    /// if there is nothing to undo.
    pub fn undo(&mut self) -> Result<Option<StateId>, DocumentError> {
        match self.history.undo_step().map(|step| step.to) {
            Some(to) => self.goto(to).map(|()| Some(to)),
            None => Ok(None),
        }
    }

    /// Makes the last undone step again. Returns the state reached, or
    /// `None` if there is nothing to redo.
    pub fn redo(&mut self) -> Result<Option<StateId>, DocumentError> {
        match self.history.redo_step().map(|step| step.to) {
            Some(to) => self.goto(to).map(|()| Some(to)),
            None => Ok(None),
        }
    }

    /// Moves the text to any state of its history.
    pub fn goto(&mut self, state: StateId) -> Result<(), DocumentError> {
        let from = self.history.current();
        if state == from {
            return Ok(());
        }
        let mismatch = DocumentError::Edit(EditError::Mismatch { at: 0 });
        let Some(()) = travel(&mut self.history, Some(&mut self.buffer), state) else {
            return Err(mismatch);
        };
        if let Err(err) = self.journal.append(&Record::Goto { state }) {
            // Going back the way we came cannot fail.
            let _ = travel(&mut self.history, Some(&mut self.buffer), from);
            return Err(DocumentError::Journal(err));
        }
        self.revision += 1;
        Ok(())
    }

    /// The text of any state of the history, without going there: the
    /// document, its history and its journal are as they were. `None` if
    /// there is no such state.
    ///
    /// This is for looking at what the text was. Looking must never cost
    /// anything, so it works on a copy of the text and writes nothing.
    #[must_use]
    pub fn text_at(&self, state: StateId) -> Option<String> {
        let steps = self.history.steps(self.history.current(), state)?;
        let mut text = self.buffer.text().to_owned();
        for step in steps {
            change::apply(&mut text, step.changes, step.direction).ok()?;
        }
        Some(text)
    }

    /// Makes `state` the one redo leads to from the state before it, so
    /// that a branch of the history which redo has left can be walked
    /// again. The text does not change. Returns `false`, and does
    /// nothing, if there is no such state or it is the root.
    ///
    /// Like every change of the history, this is in the journal before
    /// it counts: where redo leads survives a crash.
    pub fn prefer(&mut self, state: StateId) -> Result<bool, DocumentError> {
        let Some(parent) = self.history.parent(state) else {
            return Ok(false);
        };
        if self.history.redo_target(parent) == Some(state) {
            return Ok(true);
        }
        let record = Record::Prefer {
            state: parent,
            child: state,
        };
        self.journal
            .append(&record)
            .map_err(DocumentError::Journal)?;
        Ok(self.history.set_redo_target(parent, state))
    }

    /// Writes the text to the file.
    ///
    /// If something else changed the file since zetwerk last read or wrote
    /// it, nothing is written and [`DiskError::ChangedOnDisk`] is returned,
    /// unless `overwrite` is set. The write is atomic: the file is the old
    /// one or the new one at every moment, never part of each.
    pub fn save(&mut self, overwrite: bool) -> Result<(), DiskError> {
        if !overwrite && self.disk_status()? == DiskStatus::Changed {
            return Err(DiskError::ChangedOnDisk);
        }
        let bytes = file_bytes(self.bom, self.buffer.text());
        disk::write_atomically(&self.path, &bytes).map_err(DiskError::Io)?;
        let hash = disk::hash(&bytes);
        let state = self.history.current();
        self.disk_hash = Some(hash);
        self.saved = Some(state);
        self.restored = None;
        self.journal
            .append(&Record::Saved { state, hash })
            .and_then(|()| self.journal.sync())
            .map_err(DiskError::Journal)
    }

    /// Compares the file with what zetwerk last read or wrote.
    pub fn disk_status(&self) -> Result<DiskStatus, DiskError> {
        let on_disk = disk::read(&self.path).map_err(DiskError::Io)?;
        Ok(match (on_disk, self.disk_hash) {
            (None, None) => DiskStatus::Unchanged,
            (None, Some(_)) => DiskStatus::Missing,
            (Some(bytes), known) if Some(disk::hash(&bytes)) == known => DiskStatus::Unchanged,
            (Some(_), _) => DiskStatus::Changed,
        })
    }

    /// Replaces the text with the file's, as one undo step, so that what was
    /// in the buffer can be brought back with undo. The document is then
    /// unmodified.
    pub fn reload(&mut self) -> Result<(), DiskError> {
        let file = read_file(&self.path)?.ok_or(DiskError::Missing)?;
        if let Some(replace) = difference(self.buffer.text(), &file.text) {
            self.edit(&[replace], false).map_err(DiskError::Document)?;
        }
        let state = self.history.current();
        self.bom = file.bom;
        self.disk_hash = Some(file.hash);
        self.saved = Some(state);
        self.restored = None;
        self.journal
            .append(&Record::Saved {
                state,
                hash: file.hash,
            })
            .map_err(DiskError::Journal)
    }

    /// Forces the journal onto the disk. Until then, the newest changes
    /// survive a crash of zetwerk but not a power failure.
    pub fn sync(&mut self) -> io::Result<()> {
        self.journal.sync()
    }

    /// When the oldest change that a power failure would still lose was
    /// made, if there is one. The caller decides how long that may be.
    #[must_use]
    pub fn unsynced_since(&self) -> Option<Instant> {
        self.journal.unsynced_since()
    }
}

/// The single replacement that turns `old` into `new`: everything between
/// their common start and common end. `None` if they are equal.
fn difference(old: &str, new: &str) -> Option<Replace> {
    if old == new {
        return None;
    }
    let mut prefix = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let (old_rest, new_rest) = (&old[prefix..], &new[prefix..]);
    let mut suffix = old_rest
        .bytes()
        .rev()
        .zip(new_rest.bytes().rev())
        .take_while(|(a, b)| a == b)
        .count();
    while !old_rest.is_char_boundary(old_rest.len() - suffix) {
        suffix -= 1;
    }
    let range = prefix..old.len() - suffix;
    Some(Replace::new(range, &new[prefix..new.len() - suffix]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn difference_is_minimal_and_respects_characters() {
        assert_eq!(difference("same", "same"), None);
        assert_eq!(
            difference("hello world", "hello brave world"),
            Some(Replace::insert(6, "brave "))
        );
        assert_eq!(difference("abc", ""), Some(Replace::delete(0..3)));
        assert_eq!(difference("", "abc"), Some(Replace::insert(0, "abc")));
        // `é` and `è` share their first byte; the replacement must not
        // split them.
        let replace = difference("café", "cafè").unwrap_or_else(|| panic!("texts differ"));
        assert_eq!(replace, Replace::new(3..5, "è"));
        // Repetition must not make prefix and suffix overlap.
        let mut text = String::from("aaa");
        let replace = difference(&text, "aa").unwrap_or_else(|| panic!("texts differ"));
        text.replace_range(replace.range, &replace.text);
        assert_eq!(text, "aa");
    }
}
