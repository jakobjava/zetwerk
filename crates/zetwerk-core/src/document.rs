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
    /// Whether every line of the file ends in CR LF. The text then has
    /// line feeds alone, as everything that edits it expects, and the
    /// carriage returns are written back on save (see [`file_bytes`]).
    crlf: bool,
    history: History,
    journal: Journal,
    /// Where the journal is.
    journal_path: PathBuf,
    /// Held, not read: the file is locked while it lives.
    lock: Lock,
    /// The state whose text is in the file, if any state's is.
    saved: Option<StateId>,
    /// Every state that was ever in the file, with the hash of its bytes
    /// then; the most recent last. Finding the state a file holds is a
    /// lookup in these, and thinning the history leaves them.
    saves: Vec<(StateId, Hash)>,
    /// Hash of the file's bytes when last read or written; `None` if there
    /// was no file.
    disk_hash: Option<Hash>,
    revision: u64,
    /// When the unsaved changes that opening brought back from the
    /// journal were last made, until the text is saved or loaded.
    restored: Option<SystemTime>,
    /// What opening found that is to be said; see [`Document::remarks`].
    remarks: Vec<String>,
    /// Whether the last save wrote over the file in place.
    in_place: bool,
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
/// one, whether its lines end in CR LF (the text then has line feeds alone),
/// and the hash of its bytes.
struct FileContents {
    text: String,
    bom: bool,
    crlf: bool,
    hash: Hash,
}

/// Whether every line of `text` ends in CR LF, and there is no carriage
/// return anywhere else: such a text is stored with line feeds alone and
/// comes back byte for byte. A text with endings of both kinds is taken
/// as it is.
fn ends_lines_in_crlf(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut any = false;
    for (at, byte) in bytes.iter().enumerate() {
        match byte {
            b'\n' if at == 0 || bytes[at - 1] != b'\r' => return false,
            b'\n' => any = true,
            b'\r' if bytes.get(at + 1) != Some(&b'\n') => return false,
            _ => {}
        }
    }
    any
}

fn read_file(path: &Path) -> Result<Option<FileContents>, DiskError> {
    match disk::read(path).map_err(DiskError::Io)? {
        Some(bytes) => contents(bytes).map(Some),
        None => Ok(None),
    }
}

/// What a file of `bytes` holds.
fn contents(bytes: Vec<u8>) -> Result<FileContents, DiskError> {
    let hash = disk::hash(&bytes);
    let text = String::from_utf8(bytes).map_err(|_| DiskError::NotUtf8)?;
    let (text, bom) = match text.strip_prefix(BOM) {
        Some(rest) => (rest.to_owned(), true),
        None => (text, false),
    };
    let crlf = ends_lines_in_crlf(&text);
    let text = if crlf {
        str::replace(&text, "\r\n", "\n")
    } else {
        text
    };
    Ok(FileContents {
        text,
        bom,
        crlf,
        hash,
    })
}

/// The bytes a file holding `text` consists of. With `crlf`, a carriage
/// return goes before every line feed that has none: a text kept from
/// before line ends were stored as line feeds alone has them already.
fn file_bytes(bom: bool, crlf: bool, text: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len() + BOM.len());
    if bom {
        bytes.extend_from_slice(BOM.as_bytes());
    }
    if crlf {
        let mut last = 0;
        for byte in text.bytes() {
            if byte == b'\n' && last != b'\r' {
                bytes.push(b'\r');
            }
            bytes.push(byte);
            last = byte;
        }
    } else {
        bytes.extend_from_slice(text.as_bytes());
    }
    bytes
}

/// A journal read back: the history it describes, the text of the state the
/// history is at, and the state last known to be in the file.
struct Replayed {
    history: History,
    text: String,
    saved: Option<(StateId, Hash)>,
    /// Every state that was ever in the file, as far as the journal tells,
    /// with the hash of the file's bytes then; the most recent last.
    saves: Vec<(StateId, Hash)>,
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
    let mut held = Vec::new();
    let mut used = 0;
    for record in records {
        // A record that does not fit what came before ends the replay:
        // the state reached so far is consistent, the rest cannot be
        // trusted.
        if replay_record(&mut history, &mut text, &mut saved, record).is_none() {
            break;
        }
        if let Record::Saved { state, hash } = record {
            note_save(&mut held, (*state, *hash));
        }
        used += 1;
    }
    Some(Replayed {
        history,
        text: text?,
        saved,
        saves: held,
        complete: used == records.len(),
    })
}

/// Notes that the file held `state` with the bytes of `hash`, as the most
/// recent of `saves`.
fn note_save(saves: &mut Vec<(StateId, Hash)>, save: (StateId, Hash)) {
    if saves.last() != Some(&save) {
        saves.retain(|known| *known != save);
        saves.push(save);
    }
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
            // A journal is rewritten from a snapshot on: the states saved
            // before it are those that were once in the file, and which
            // one is in it now is said after it (see `compact`).
            *saved = None;
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

/// Rebuilds what can be rebuilt from `records`, the part of a damaged
/// journal from a snapshot on (see [`journal::salvage`]): the history
/// before the snapshot is lost, so the snapshot's text is the root. The
/// records that follow are used as long as they fit; those about states
/// that are lost and that change no text (where redo leads, what was
/// saved) are passed over. `None` if `records` do not begin with a
/// snapshot.
///
/// With `file`, the text of the file, that is the root instead, as the
/// one state known to have been saved, and the snapshot's text a step
/// from it: what was recovered is unsaved, and undo leads to the file.
fn replay_salvaged(records: &[Record], file: Option<&FileContents>) -> Option<Replayed> {
    let (Record::Snapshot { state, text }, rest) = records.split_first()? else {
        return None;
    };
    let mut history = History::new();
    let mut seen = Vec::new();
    let mut snapshot = ROOT;
    let text = match file {
        Some(file) => {
            seen.push((ROOT, file.hash));
            let mut base = file.text.clone();
            if let Some(replace) = difference(&base, text) {
                let changes = change::perform(&mut base, &[replace]).ok()?;
                snapshot = history.push(changes);
            }
            base
        }
        None => text.clone(),
    };
    let mut text = text;
    let mut ids = std::collections::HashMap::from([(*state, snapshot)]);
    for record in rest {
        let current = history.current();
        let fits = match record {
            Record::Push {
                id,
                parent,
                changes,
            } => {
                let fits = ids.get(parent) == Some(&current)
                    && change::apply(&mut text, changes, Direction::Forward).is_ok();
                if fits {
                    ids.insert(*id, history.push(changes.clone()));
                }
                fits
            }
            Record::Extend { state, changes } => {
                let fits = ids.get(state) == Some(&current)
                    && change::apply(&mut text, changes, Direction::Forward).is_ok();
                if fits && !history.extend(changes.clone()) {
                    // The snapshot's own state is the root here, which
                    // takes no changes: they make a state of their own,
                    // which stands for it from now on.
                    ids.insert(*state, history.push(changes.clone()));
                }
                fits
            }
            Record::Goto { state } => ids
                .get(state)
                .is_some_and(|&to| travel(&mut history, Some(&mut text), to).is_some()),
            Record::Snapshot {
                state,
                text: snapshot,
            } => ids.get(state) == Some(&current) && *snapshot == text,
            Record::Prefer { state, child } => {
                if let (Some(&state), Some(&child)) = (ids.get(state), ids.get(child)) {
                    history.set_redo_target(state, child);
                }
                true
            }
            Record::Saved { state, hash } => {
                if let Some(&state) = ids.get(state) {
                    note_save(&mut seen, (state, *hash));
                }
                true
            }
        };
        if !fits {
            break;
        }
    }
    Some(Replayed {
        history,
        text,
        saved: seen.last().copied(),
        saves: seen,
        complete: false,
    })
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

/// The shortest journal that describes `history` at its current state:
/// `held` are the states that were once in the file, `saved` the one
/// that is in it now.
fn compact(
    history: &History,
    text: &str,
    held: &[(StateId, Hash)],
    saved: Option<(StateId, Hash)>,
) -> Vec<Record> {
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
    // Before the snapshot, which ends what they say about the file now.
    for &(state, hash) in held {
        if saved != Some((state, hash)) && history.contains(state) {
            records.push(Record::Saved { state, hash });
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

/// Looks for a state of `history` whose text is the one in `file`. `text`
/// is the text of the current state.
///
/// How long each state's text is follows from the changes alone, so only
/// the states as long as the file's text are gone to and compared: the
/// cost is that of reading the history, not of hashing every state's text.
fn state_holding(history: &History, text: &str, file: &FileContents) -> Option<StateId> {
    let growth = |state: StateId| {
        history.changes(state).map_or(0, |changes| {
            changes
                .iter()
                .map(|change| change.new.len() as i128 - change.old.len() as i128)
                .sum::<i128>()
        })
    };
    let mut root = text.len() as i128;
    let mut at = history.current();
    while let Some(parent) = history.parent(at) {
        root -= growth(at);
        at = parent;
    }
    let mut lengths = Vec::with_capacity(history.len());
    lengths.push(root);
    for state in 1..history.len() as StateId {
        let parent = history.parent(state)?;
        let parent = lengths[usize::try_from(parent).ok()?];
        lengths.push(parent + growth(state));
    }
    let wanted = file.text.len() as i128;
    let candidates: Vec<StateId> = (0..history.len() as StateId)
        .rev()
        .filter(|&state| usize::try_from(state).is_ok_and(|at| lengths[at] == wanted))
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let mut history = history.clone();
    let mut text = text.to_owned();
    candidates
        .into_iter()
        .find(|&state| travel(&mut history, Some(&mut text), state).is_some() && text == file.text)
}

/// Where the journal of `path` is kept under `state_dir`.
fn journal_path(state_dir: &Path, path: &Path) -> PathBuf {
    let name = blake3::hash(path.as_os_str().as_encoded_bytes()).to_hex();
    state_dir
        .join("journals")
        .join(format!("{}.journal", &name[..32]))
}

/// Where the journal of `path` is kept, beside the journal `journal`.
fn journal_beside(journal: &Path, path: &Path) -> PathBuf {
    let journals = journal.parent().unwrap_or(journal);
    journal_path(journals.parent().unwrap_or(journals), path)
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

/// Notes beside `journal` which file it is the journal of, so that the
/// journal can be found again when the file has moved. Only a path that
/// is text can be noted.
fn note_file(journal: &Path, path: &Path) {
    let note = journal.with_extension("file");
    let Some(name) = path.to_str() else {
        return;
    };
    if disk::read(&note).ok().flatten().as_deref() != Some(name.as_bytes()) {
        let _ = disk::write_atomically(&note, name.as_bytes());
    }
}

/// Which file `journal` is the journal of, if that was noted.
fn noted_file(journal: &Path) -> Option<PathBuf> {
    let bytes = disk::read(&journal.with_extension("file")).ok()??;
    String::from_utf8(bytes).ok().map(PathBuf::from)
}

/// Takes over the history of a file that was moved or renamed and is now
/// the file at `path`, holding `file`: called when `path` has no journal
/// of its own, which is to be at `journal`.
///
/// A journal is taken over if the text it last knew (the file's when it
/// was last written or read, or the text as it was left) is the file's,
/// and the file it was the journal of is not there or no longer holds a
/// text it knew: if it still does, that file is still its own. A journal
/// that a window has open, or that was taken over before, is not taken.
/// If more than one would do, none is: which is the right one is not for
/// zetwerk to guess. The journal taken over is copied, and the original
/// is kept where it was, marked as taken over.
///
/// Returns what to say about it, and when the journal was last written.
fn take_over(
    journal: &Path,
    path: &Path,
    file: &FileContents,
) -> io::Result<Option<(String, Option<SystemTime>)>> {
    let Some(directory) = journal.parent() else {
        return Ok(None);
    };
    let mut found = Vec::new();
    for entry in std::fs::read_dir(directory)?.flatten() {
        let candidate = entry.path();
        if candidate == journal
            || candidate
                .extension()
                .is_none_or(|ending| ending != "journal")
            || candidate.with_extension("moved").exists()
        {
            continue;
        }
        let was = noted_file(&candidate);
        let there = was
            .as_deref()
            .and_then(|was| disk::read(was).ok().flatten())
            .map(|bytes| disk::hash(&bytes));
        // A copy of a file that is still there, as it was.
        if there == Some(file.hash) {
            continue;
        }
        let Ok(Some(bytes)) = disk::read(&candidate) else {
            continue;
        };
        let Some(replayed) = journal::decode(&bytes).and_then(|(records, _)| replay(&records))
        else {
            continue;
        };
        let last_known = [
            replayed.saved.map(|(_, hash)| hash),
            Some(disk::hash(&file_bytes(file.bom, file.crlf, &replayed.text))),
        ];
        if !last_known.contains(&Some(file.hash))
            || there.is_some_and(|there| last_known.contains(&Some(there)))
        {
            continue;
        }
        let in_use = !matches!(
            Lock::acquire(&candidate.with_extension("lock")),
            Ok(Some(_))
        );
        if !in_use {
            found.push((candidate, was, bytes));
        }
    }
    match found.as_slice() {
        [] => Ok(None),
        [(candidate, was, bytes)] => {
            let when = std::fs::metadata(candidate)
                .and_then(|meta| meta.modified())
                .ok();
            disk::write_atomically(journal, bytes)?;
            if let Some(name) = path.to_str() {
                let _ = disk::write_atomically(&candidate.with_extension("moved"), name.as_bytes());
            }
            let from = was.as_ref().map_or_else(
                || "a file with this text that is no longer known".to_owned(),
                |was| was.display().to_string(),
            );
            Ok(Some((format!("history taken over from {from}"), when)))
        }
        several => Ok(Some((
            format!(
                "the histories of {} earlier files fit this one: none was taken over",
                several.len()
            ),
            None,
        ))),
    }
}

/// A history to carry on with: the history, the text of its current state,
/// the state whose text is in the file, and the states that were.
struct Found {
    history: History,
    text: String,
    saved: Option<StateId>,
    saves: Vec<(StateId, Hash)>,
}

/// What a journal means for the file: the outcome, the history to carry
/// on with if there is one, and what else is to be said.
struct Reconciled {
    opened: Opened,
    found: Option<Found>,
    remark: Option<String>,
}

/// Decides what the journal at `journal_path` means for the file `file`
/// (`None`: there is no file).
fn reconcile(journal_path: &Path, file: Option<&FileContents>) -> io::Result<Reconciled> {
    let Some(bytes) = disk::read(journal_path)?.filter(|bytes| !bytes.is_empty()) else {
        return Ok(Reconciled {
            opened: Opened::Clean,
            found: None,
            remark: None,
        });
    };
    let decoded = journal::decode(&bytes);
    let valid = decoded.as_ref().map_or(0, |(_, valid)| *valid);
    // Damage, not a write cut short, ends what can be read: there may be
    // more beyond it. A file that does not begin as a journal is damage
    // too, or no journal at all.
    let damaged = decoded.is_none() || !journal::is_torn_tail(&bytes[valid..]);
    let mut replayed = decoded.and_then(|(records, _)| replay(&records));
    let mut history_lost = false;
    if replayed.is_none() && damaged {
        // The text is never said before the damage. The last snapshot
        // beyond it says it, and what was done after it can follow.
        replayed = replay_salvaged(&journal::salvage(&bytes, valid), file);
        history_lost = replayed.is_some();
    }
    let Some(replayed) = replayed else {
        let journal = set_aside(journal_path, "unusable")?;
        return Ok(Reconciled {
            opened: Opened::JournalUnusable { journal },
            found: None,
            remark: None,
        });
    };
    let remark = if damaged || !replayed.complete {
        // Use what is consistent, keep the original as it is.
        let kept = aside(journal_path, "damaged");
        std::fs::copy(journal_path, &kept)?;
        let lost = if history_lost {
            "the history before the file was last opened is lost; the text as it was \
             last written there is back"
        } else {
            "what was done after a damaged part of it is not back"
        };
        Some(format!(
            "the journal was damaged: {lost}; it is kept as it was at {}",
            kept.display()
        ))
    } else {
        None
    };
    let current = replayed.history.current();
    let (opened, saved_now) = match file {
        // No file. If the journal holds text, the file is gone but its
        // text is not.
        None if replayed.text.is_empty() => (Opened::Clean, None),
        None => (Opened::Restored, None),
        Some(file) if file.hash == disk::hash(&file_bytes(file.bom, file.crlf, &replayed.text)) => {
            (Opened::Clean, Some(current))
        }
        Some(file) => {
            // The file is not the journal's current text. If it is some
            // other state of this history, the difference is unsaved work;
            // if it is none, someone else wrote it.
            let known = replayed
                .saved
                .into_iter()
                .chain(replayed.saves.iter().rev().copied())
                .find(|(_, hash)| *hash == file.hash)
                .map(|(state, _)| state)
                .or_else(|| state_holding(&replayed.history, &replayed.text, file));
            if let Some(state) = known {
                (Opened::Restored, Some(state))
            } else {
                let unsaved = match replayed.saved {
                    Some((state, _)) => state != current,
                    None => !replayed.text.is_empty(),
                };
                // Either way the history goes on and the file's text is
                // put on top of it (see `Document::open`): what zetwerk
                // had is one undo away, never dropped.
                if unsaved {
                    let journal = aside(journal_path, "conflict");
                    std::fs::copy(journal_path, &journal)?;
                    let opened = Opened::Conflict {
                        recovered: replayed.text.clone(),
                        journal,
                    };
                    (opened, None)
                } else {
                    (Opened::ChangedWhileClosed, None)
                }
            }
        }
    };
    Ok(Reconciled {
        opened,
        found: Some(Found {
            history: replayed.history,
            text: replayed.text,
            saved: saved_now,
            saves: replayed.saves,
        }),
        remark,
    })
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
        Ok(Some(file)) => file.hash == disk::hash(&file_bytes(file.bom, file.crlf, &replayed.text)),
        Ok(None) => replayed.text.is_empty(),
        Err(_) => false,
    };
    unsaved && !in_file
}

/// Asks the window that has the file at `path` open to say so, and to ask
/// for the user's attention: another start of zetwerk wanted the file
/// too. The window notices the file this leaves (see [`knock_path`]).
pub fn knock(path: &Path, state_dir: &Path) -> io::Result<()> {
    let knock = knock_path(path, state_dir);
    if let Some(directory) = knock.parent() {
        disk::create_private_dir(directory)?;
    }
    std::fs::write(knock, b"")
}

/// The file another start of zetwerk leaves to say it wanted the file at
/// `path` (see [`knock`]): one per file, in a directory of its own under
/// `state_dir`, which a window can watch.
#[must_use]
pub fn knock_path(path: &Path, state_dir: &Path) -> PathBuf {
    let path = std::fs::canonicalize(path)
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_owned());
    let journal = journal_path(state_dir, &path);
    let name = journal.file_stem().unwrap_or_default();
    state_dir.join("knocks").join(name)
}

/// Removes temporary files that a session which died while saving left next
/// to the document. That directory is the user's, so only a file that
/// provably holds a text of this very document is removed; anything else is
/// not ours to judge.
fn remove_own_leftovers(path: &Path, file_hash: Option<Hash>, found: &Found) {
    for leftover in disk::leftovers(path) {
        let ours = disk::read(&leftover).ok().flatten().is_some_and(|bytes| {
            let hash = disk::hash(&bytes);
            Some(hash) == file_hash
                || found.saves.iter().any(|(_, saved)| *saved == hash)
                || contents(bytes).is_ok_and(|leftover| {
                    state_holding(&found.history, &found.text, &leftover).is_some()
                })
        });
        if ours {
            let _ = std::fs::remove_file(leftover);
        }
    }
}

/// Above this many states, the oldest steps of a history are folded into
/// coarser ones when the file is opened; see [`Document::thin`].
const THIN_BEYOND: usize = 10_000;
/// How many of the newest states thinning leaves as they are.
const THIN_KEEPS: usize = 5_000;

/// `history` thinned (see [`History::thinned`]): the newest `recent`
/// states, and every state that was ever in the file, stay.
fn thin_history(
    history: &History,
    recent: usize,
    saves: &[(StateId, Hash)],
) -> (History, Vec<Option<StateId>>) {
    let first_recent = history.len().saturating_sub(recent) as StateId;
    let held: std::collections::HashSet<StateId> = saves.iter().map(|(state, _)| *state).collect();
    history.thinned(|state| state >= first_recent || held.contains(&state))
}

/// `saves` with the states' ids as `map` (from [`History::thinned`]) has
/// them.
fn renumber(saves: &[(StateId, Hash)], map: &[Option<StateId>]) -> Vec<(StateId, Hash)> {
    saves
        .iter()
        .filter_map(|&(state, hash)| {
            let state = map.get(usize::try_from(state).ok()?).copied()??;
            Some((state, hash))
        })
        .collect()
}

/// `path` made absolute, with symbolic links resolved in its directory.
fn resolved(path: &Path) -> io::Result<PathBuf> {
    let path = std::path::absolute(path)?;
    match (path.parent(), path.file_name()) {
        (Some(directory), Some(name)) => Ok(std::fs::canonicalize(directory)?.join(name)),
        _ => Ok(path),
    }
}

/// The error for a file at `path` that is not to be written over.
fn exists(path: &Path) -> DiskError {
    DiskError::Io(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("{} exists", path.display()),
    ))
}

/// Takes the lock of the file at `path`, whose journal is `journal`.
fn lock_for(journal: &Path, path: &Path) -> Result<Lock, DiskError> {
    Lock::acquire(&journal.with_extension("lock"))
        .map_err(DiskError::Io)?
        .ok_or_else(|| {
            DiskError::Io(io::Error::other(format!(
                "{} is open in another zetwerk",
                path.display()
            )))
        })
}

/// Starts the journal of the file at `path` at `journal` with `records`. A
/// journal that is there already is of a file that was at `path` before,
/// and is kept aside.
fn new_journal(journal: &Path, path: &Path, records: &[Record]) -> Result<Journal, DiskError> {
    if journal.exists() {
        set_aside(journal, "replaced").map_err(DiskError::Journal)?;
    }
    let created = Journal::create(journal, records).map_err(DiskError::Journal)?;
    note_file(journal, path);
    Ok(created)
}

impl<B: Buffer> Document<B> {
    /// Opens the file at `path` for editing, creating the text buffer with
    /// `buffer`. A file that does not exist yet opens as empty text.
    ///
    /// Journals live under `state_dir`. If the file's journal shows that an
    /// earlier session ended without saving, its changes are brought back;
    /// see [`Opened`] for the possible outcomes. A file without a journal
    /// may take over that of the file it was before it was moved (see
    /// [`Document::remarks`]).
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
        let mut last_written = std::fs::metadata(&journal_path)
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
        let crlf = file.as_ref().is_some_and(|file| file.crlf);
        let file_hash = file.as_ref().map(|file| file.hash);

        let mut remarks = Vec::new();
        let own = disk::read(&journal_path)
            .map_err(io(&journal_path))?
            .is_some_and(|bytes| !bytes.is_empty());
        if let Some(file) = file.as_ref().filter(|file| !own && !file.text.is_empty()) {
            // Finding none is no reason not to open the file.
            if let Ok(Some((remark, when))) = take_over(&journal_path, &path, file) {
                remarks.push(remark);
                last_written = when;
            }
        }
        note_file(&journal_path, &path);

        let reconciled = reconcile(&journal_path, file.as_ref()).map_err(io(&journal_path))?;
        let opened = reconciled.opened;
        remarks.extend(reconciled.remark);
        let mut found = reconciled.found.unwrap_or_else(|| Found {
            history: History::new(),
            text: file.map(|file| file.text).unwrap_or_default(),
            saved: file_hash.map(|_| ROOT),
            saves: Vec::new(),
        });
        if found.history.len() > THIN_BEYOND {
            let (history, map) = thin_history(&found.history, THIN_KEEPS, &found.saves);
            found.saves = renumber(&found.saves, &map);
            found.saved = found
                .saved
                .and_then(|state| map.get(usize::try_from(state).ok()?).copied()?);
            found.history = history;
        }
        remove_own_leftovers(&path, file_hash, &found);
        let saved_record = found.saved.zip(file_hash);
        if let Some(save) = saved_record {
            note_save(&mut found.saves, save);
        }
        let journal = Journal::create(
            &journal_path,
            &compact(&found.history, &found.text, &found.saves, saved_record),
        )
        .map_err(io(&journal_path))?;
        let restored = last_written.filter(|_| opened == Opened::Restored);
        if let Some(when) = restored {
            // Compacting is no change: how old the unsaved changes are is
            // still known when the file is opened the next time.
            let _ = journal.set_modified(when);
        }
        let mut document = Self {
            path,
            buffer: buffer(found.text),
            bom,
            crlf,
            history: found.history,
            journal,
            journal_path,
            lock,
            saved: found.saved,
            saves: found.saves,
            disk_hash: file_hash,
            revision: 0,
            restored,
            remarks,
            in_place: false,
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

    /// What opening found that [`Opened`] does not say, for the user to
    /// hear: that the history of the file before it was moved was taken
    /// over, that the journal was damaged and what of it is lost.
    #[must_use]
    pub fn remarks(&self) -> &[String] {
        &self.remarks
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
        // Never fold changes into a state the file has held: it would stop
        // being it. Not only the one saved last: a file that is found to
        // hold the bytes of an earlier save is taken to be at that state
        // (see `reconcile`), which must then still have that text.
        let join = join
            && self.history.can_extend()
            && self.saved != Some(current)
            && self.saves.iter().all(|(state, _)| *state != current);
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
    ///
    /// Only where that cannot be done because the directory takes no new
    /// file, while the file itself may be written, the file is written
    /// over in place: then, for a moment, it is neither; the journal is
    /// on the disk before, so the text is not lost even then. See
    /// [`Document::wrote_in_place`].
    pub fn save(&mut self, overwrite: bool) -> Result<(), DiskError> {
        if !overwrite && self.disk_status()? == DiskStatus::Changed {
            return Err(DiskError::ChangedOnDisk);
        }
        let bytes = file_bytes(self.bom, self.crlf, self.buffer.text());
        self.in_place = match disk::write_atomically(&self.path, &bytes) {
            Ok(()) => false,
            Err(err) if err.kind() == io::ErrorKind::PermissionDenied && self.path.is_file() => {
                self.journal.sync().map_err(DiskError::Journal)?;
                match disk::write_in_place(&self.path, &bytes) {
                    Ok(()) => true,
                    // Not even opened for writing: the file is as it was.
                    Err(again) if again.kind() == io::ErrorKind::PermissionDenied => {
                        return Err(DiskError::Io(err));
                    }
                    Err(again) => {
                        return Err(DiskError::Io(io::Error::other(format!(
                            "writing over the file failed ({again}): it may be cut short; \
                             the text is safe in the journal, :w! writes it again"
                        ))));
                    }
                }
            }
            Err(err) => return Err(DiskError::Io(err)),
        };
        let hash = disk::hash(&bytes);
        let state = self.history.current();
        self.disk_hash = Some(hash);
        self.saved = Some(state);
        note_save(&mut self.saves, (state, hash));
        self.restored = None;
        self.journal
            .append(&Record::Saved { state, hash })
            .and_then(|()| self.journal.sync())
            .map_err(DiskError::Journal)
    }

    /// Whether the last [`Document::save`] wrote over the file in place,
    /// because its directory takes no new file.
    #[must_use]
    pub fn wrote_in_place(&self) -> bool {
        self.in_place
    }

    /// Writes the text to the file at `to` as well, and goes on with the
    /// file it is. A file that is there is written over only with
    /// `overwrite`.
    pub fn write_copy(&self, to: &Path, overwrite: bool) -> Result<(), DiskError> {
        let to = std::path::absolute(to).map_err(DiskError::Io)?;
        if !overwrite && to.symlink_metadata().is_ok() {
            return Err(exists(&to));
        }
        let bytes = file_bytes(self.bom, self.crlf, self.buffer.text());
        disk::write_atomically(&to, &bytes).map_err(DiskError::Io)
    }

    /// Writes the text to the file at `to` and goes on as that file, with
    /// the history: Vim's `:saveas`. A file that is there is written over
    /// only with `overwrite`. The file the text was in stays as it is, and
    /// so does its journal, at the text that file holds: what was not
    /// written to it is in the new file, and opening it again brings
    /// nothing back.
    pub fn save_as(&mut self, to: &Path, overwrite: bool) -> Result<(), DiskError> {
        let to = resolved(to).map_err(DiskError::Io)?;
        if to == self.path {
            return self.save(overwrite);
        }
        if !overwrite && to.symlink_metadata().is_ok() {
            return Err(exists(&to));
        }
        let journal_path = journal_beside(&self.journal_path, &to);
        let lock = lock_for(&journal_path, &to)?;
        let bytes = file_bytes(self.bom, self.crlf, self.buffer.text());
        disk::write_atomically(&to, &bytes).map_err(DiskError::Io)?;
        let hash = disk::hash(&bytes);
        let state = self.history.current();
        let mut saves = self.saves.clone();
        note_save(&mut saves, (state, hash));
        let journal = new_journal(
            &journal_path,
            &to,
            &compact(
                &self.history,
                self.buffer.text(),
                &saves,
                Some((state, hash)),
            ),
        )?;
        if let Some(saved) = self.saved.filter(|saved| *saved != state) {
            // Never mind if it fails: the old file then brings back, as
            // unsaved, what is in the new one.
            let _ = self
                .journal
                .append(&Record::Goto { state: saved })
                .and_then(|()| self.journal.sync());
        }
        self.path = to;
        self.journal = journal;
        self.journal_path = journal_path;
        self.lock = lock;
        self.saved = Some(state);
        self.saves = saves;
        self.disk_hash = Some(hash);
        self.restored = None;
        self.in_place = false;
        Ok(())
    }

    /// Moves the file to `to`, and its journal along: the text, what is not
    /// written of it and the history are those of the file at `to` from now
    /// on. Nothing that is at `to` is ever written over. A file that was
    /// never written moves only its journal.
    ///
    /// The file is at both names for a moment, never at none; the journal
    /// at the new name is complete before the old one goes. If the old
    /// name cannot be removed then, the file stays there as a copy, and
    /// why is returned.
    pub fn rename(&mut self, to: &Path) -> Result<Option<io::Error>, DiskError> {
        let to = resolved(to).map_err(DiskError::Io)?;
        if to == self.path {
            return Ok(None);
        }
        if to.symlink_metadata().is_ok() {
            return Err(exists(&to));
        }
        let journal_path = journal_beside(&self.journal_path, &to);
        let lock = lock_for(&journal_path, &to)?;
        self.journal.sync().map_err(DiskError::Journal)?;
        let there = self.path.is_file();
        if there {
            disk::link_or_copy(&self.path, &to).map_err(|err| {
                if err.kind() == io::ErrorKind::AlreadyExists {
                    exists(&to)
                } else {
                    DiskError::Io(err)
                }
            })?;
        }
        let saved = self.saved.zip(self.disk_hash);
        let records = compact(&self.history, self.buffer.text(), &self.saves, saved);
        let journal = match new_journal(&journal_path, &to, &records) {
            Ok(journal) => journal,
            Err(err) => {
                if there {
                    let _ = std::fs::remove_file(&to);
                }
                return Err(err);
            }
        };
        let left = if there {
            std::fs::remove_file(&self.path).err()
        } else {
            None
        };
        let _ = std::fs::remove_file(&self.journal_path);
        let _ = std::fs::remove_file(self.journal_path.with_extension("file"));
        self.path = to;
        self.journal = journal;
        self.journal_path = journal_path;
        self.lock = lock;
        Ok(left)
    }

    /// Puts the buffer `buffer` makes of the one there in its place, if it
    /// holds the same text: for a buffer that knows the file's name, when
    /// the file has another. Returns whether it did.
    pub fn rebuffer(&mut self, buffer: impl FnOnce(&B) -> B) -> bool {
        let new = buffer(&self.buffer);
        let same = new.text() == self.buffer.text();
        if same {
            self.buffer = new;
        }
        same
    }

    /// Folds the old steps of the history into coarser ones: the newest
    /// `recent` states stay as they are, and so do every state that was
    /// ever in the file, the current one, the states where branches part
    /// and the ends of branches; every text that stays reachable is the
    /// same. Opening a file does this once its history has grown long.
    ///
    /// The journal is written anew first, at once (see [`Journal::create`]):
    /// a crash leaves it as it was or thinned, never in between. Returns,
    /// by old id, the new id of each state that stayed.
    pub fn thin(&mut self, recent: usize) -> Result<Vec<Option<StateId>>, DocumentError> {
        let (history, map) = thin_history(&self.history, recent, &self.saves);
        let renumbered = renumber(&self.saves, &map);
        let saved = self
            .saved
            .and_then(|state| map.get(usize::try_from(state).ok()?).copied()?);
        let records = compact(
            &history,
            self.buffer.text(),
            &renumbered,
            saved.zip(self.disk_hash),
        );
        self.journal =
            Journal::create(&self.journal_path, &records).map_err(DocumentError::Journal)?;
        self.history = history;
        self.saves = renumbered;
        self.saved = saved;
        Ok(map)
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
        self.crlf = file.crlf;
        self.disk_hash = Some(file.hash);
        self.saved = Some(state);
        note_save(&mut self.saves, (state, file.hash));
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
