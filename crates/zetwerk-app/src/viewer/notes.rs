//! Reviewing together: notes on the text, questions for agents, and
//! proposals shown as what they would make of the page.
//!
//! A note is about a stretch of the text: what was selected, or the
//! paragraph or figure the cursor was in (`gn`, `:note`). A question is
//! a note addressed to the agents (`gq`, `:ask`): the program asks no AI
//! itself, it keeps the question until an agent reads it (`list_notes`)
//! and answers it (`answer_note`), with words and, where a change is the
//! answer, with a proposal that belongs to the note. Agents leave notes
//! for the user the same way (`add_note`).
//!
//! A note moves with the text as proposals do, and loses its place only
//! when all of its text is gone (it is kept, detached, until that text is
//! back or the note is deleted). It is shown faintly on the text, with a
//! tag in the right margin, and nowhere else: not in the text, so not in
//! an export or on a page an agent is shown.
//!
//! Notes are kept beside the journal, in the state directory, one file
//! for each file of the document; not in the `.typ` file. A note is the
//! reviewers' and not the document's: written into the source as a
//! comment it would travel with every copy of the file, be changed by
//! every edit near it as text, and an agent's note would change the text
//! the user is in charge of. Beside the journal it survives a restart as
//! the unsaved text does, and the file stays as the user wrote it.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use typst::layout::Rect;
use typst::syntax::{FileId, LinkedNode, Side, Source, SyntaxKind};
use zetwerk_typst::PageMap;

use super::agent::{Outcome, Proposal, line_column, offset_of};
use super::assist::{Target, cut};
use super::{
    Difference, LENS_EDGE, PANEL_MARGIN, PAPER, Quad, Scene, TextureId, Viewer, chrome, difference,
    solid, tint,
};
use crate::chrome::{Chrome, Image, InPlace};
use crate::editor::{self, Shown};
use crate::keys::{Key, KeyPress};

/// What a note on a page looks like: faintly laid over its text, and a
/// line under it.
const NOTE_FILL: [u8; 4] = [0xb1, 0x62, 0x86, 0x1c];
const NOTE_LINE: [u8; 4] = [0xb1, 0x62, 0x86, 0xa0];
/// The edge of the margin of the page the tags stand off from, in
/// points, and the size of their text.
const TAG_GAP: f64 = 4.0;
const TAG_SIZE: f64 = 5.5;
/// How many characters of a note its tag shows.
const TAG_WORDS: usize = 16;
/// The stripe of a tag: a note, a question for agents, and one that has
/// been answered.
const STRIPE_NOTE: &str = "#b16286";
const STRIPE_QUESTION: &str = "#458588";
const STRIPE_ANSWERED: &str = "#98971a";
/// The colours of the panels, as the rest of the interface has them.
const BACKGROUND: &str = "#282828";
const FOREGROUND: &str = "#ebdbb2";
const DIM: &str = "#928374";
const ACCENT: &str = "#fabd2f";
const FONT: &str = "DejaVu Sans Mono";
/// How wide the text of a panel is, in characters, and how many lines
/// of a note or an answer it shows.
const CARD_COLUMNS: usize = 64;
const CARD_LINES: usize = 6;
/// How much of the text a note is about an agent is given.
const MAX_QUOTE: usize = 2000;
/// The room a proposal set in place has at least, in points.
const LEAST_ROOM: f64 = 60.0;
/// What the store of notes begins with, and the version of its form.
const HEADER: &str = "zetwerk notes 1";
/// More image sizes than this are not remembered.
const SIZES_KEPT: usize = 2000;
/// References are to labels of the whole document, which a paragraph
/// set by itself does not have: they show as their names.
const SHOW_REFS: &str = "#show ref: it => text(fill: luma(110), str(it.target));";

/// Who wrote a note or an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Author {
    User,
    Agent,
}

impl Author {
    /// As the store and agents name it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Agent => "agent",
        }
    }
}

/// A note on a stretch of the text.
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent facts about a note, not states of one thing"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub id: u64,
    /// The file it is about.
    pub file: FileId,
    /// The bytes of the text it is about.
    pub range: Range<usize>,
    /// That text, as it was when it was last looked at: where to find
    /// the note again if the file changed while it was closed.
    pub quote: String,
    /// The note, or the question.
    pub text: String,
    pub by: Author,
    /// Whether it is a question for agents.
    pub to_agent: bool,
    /// What was said to it since, by whom.
    pub answers: Vec<(Author, String)>,
    pub resolved: bool,
    /// Whether all of its text is gone: it is shown nowhere then.
    pub detached: bool,
    /// Whether an agent said something in it that the user has not seen.
    pub unseen: bool,
}

/// Where an agent wants a note to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteAnchor<'a> {
    /// On this text, which is in the text exactly once.
    Text(&'a str),
    /// On the paragraph (or heading, item) of this line, counting from 1.
    Line(usize),
}

/// A note as an agent is given it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub id: u64,
    /// The file it is about, from the document's directory.
    pub file: String,
    /// Where its text begins, counting from 1.
    pub line: usize,
    pub column: usize,
    /// The text it is about; long text is cut.
    pub text: String,
    pub note: String,
    pub by: Author,
    pub to_agent: bool,
    pub answers: Vec<(Author, String)>,
    pub resolved: bool,
    pub detached: bool,
    /// The proposals that were made as answers to it and still wait.
    pub proposals: Vec<u64>,
}

/// Why an agent proposed a change, and the note it answers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct About {
    reason: Option<String>,
    note: Option<u64>,
}

/// What the notes keep between keys.
#[derive(Default)]
pub(super) struct Notes {
    /// The notes of all open files, each file's in the order of its text.
    all: Vec<Note>,
    /// The number the last note got.
    last: u64,
    /// The files whose stored notes have been read.
    loaded: Vec<FileId>,
    /// The text of the file being edited that the places of its notes
    /// are places in.
    seen: Option<(FileId, String)>,
    /// Whether notes changed in a way not yet written: their places.
    moved: bool,
    /// The place a note begun by a key is to be about, while its text is
    /// typed: the bytes, and the revision and the cursor then.
    pending: Option<(Range<usize>, u64, usize)>,
    /// The proposal the user flipped to see the page as it is.
    before: Option<u64>,
    /// Whether a proposal was shown as it would be in the frame being
    /// drawn.
    previewed: bool,
    /// What agents said of their proposals, by proposal.
    about: HashMap<u64, About>,
    /// The pixel sizes of tags the compositor may hold.
    sizes: HashMap<TextureId, (u32, u32)>,
    /// What tells the window that a proposal was typeset as it would be.
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
    /// The thread that typesets proposals as they would be.
    typesetter: Option<Typesetter>,
    /// The piece given to that thread and not back yet.
    typesetting: Option<TextureId>,
    /// The piece that came back last: `None` where it does not typeset.
    typeset: Option<(TextureId, Option<Image>)>,
}

/// A piece to typeset on the thread of the previews, and its name.
type Job = (TextureId, Box<dyn FnOnce() -> Option<Image> + Send>);

/// The thread that typesets a proposal as it would be, so that the
/// window never waits for Typst to show one: started when one is first
/// shown, and asleep on its channel while there is nothing to typeset. It
/// ends when the window does.
struct Typesetter {
    jobs: Sender<Job>,
    done: Receiver<(TextureId, Option<Image>)>,
}

impl Typesetter {
    /// Starts the thread; `wake` is called from it whenever a piece is
    /// done.
    fn spawn(wake: Option<Arc<dyn Fn() + Send + Sync>>) -> std::io::Result<Self> {
        let (jobs, waiting) = mpsc::channel::<Job>();
        let (finished, done) = mpsc::channel();
        std::thread::Builder::new()
            .name("zetwerk-preview".into())
            .spawn(move || {
                for (id, job) in waiting {
                    // A piece that cannot be typeset is shown otherwise.
                    let image = catch_unwind(AssertUnwindSafe(job)).ok().flatten();
                    if finished.send((id, image)).is_err() {
                        break;
                    }
                    if let Some(wake) = &wake {
                        wake();
                    }
                }
            })?;
        Ok(Self { jobs, done })
    }
}

/// Where byte `place` of a text is after `removed` bytes from `offset`
/// on were replaced by `inserted` bytes: as the start of a note (what is
/// put in at its start stays outside) or as its end (what is put in at
/// its end too).
fn moved_place(
    place: usize,
    (offset, removed, inserted): (usize, usize, usize),
    end: bool,
) -> usize {
    let after = offset + removed;
    if place < offset || (end && place == offset) {
        place
    } else if place >= after {
        place - removed + inserted
    } else if end {
        offset + inserted
    } else {
        offset
    }
}

/// The thing the cursor at `cursor` is in, for a note about it: the
/// figure, or else the paragraph (the line) without the blanks around
/// it. `None` on an empty line.
fn thing_at(source: &Source, cursor: usize) -> Option<Range<usize>> {
    let text = source.text();
    let cursor = cursor.min(text.len());
    let mut node = LinkedNode::new(source.root()).leaf_at(cursor, Side::After);
    while let Some(here) = node {
        let figure = here.kind() == SyntaxKind::FuncCall
            && here
                .children()
                .next()
                .is_some_and(|callee| callee.leaf_text() == "figure");
        if figure {
            let mut range = here.range();
            if text[..range.start].ends_with('#') {
                range.start -= 1;
            }
            return Some(range);
        }
        node = here.parent().cloned();
    }
    line_at(text, cursor)
}

/// The line byte `at` of `text` is in, without the blanks around it;
/// `None` if there is nothing else on it.
fn line_at(text: &str, at: usize) -> Option<Range<usize>> {
    let start = text[..at].rfind('\n').map_or(0, |found| found + 1);
    let end = text[at..].find('\n').map_or(text.len(), |found| at + found);
    let line = &text[start..end];
    let lead = line.len() - line.trim_start().len();
    let kept = line.trim().len();
    (kept > 0).then_some(start + lead..start + lead + kept)
}

/// How many bytes at the start of a line of markup only say what kind
/// of block it is: a heading's equals signs, an item's dash or plus,
/// with the blanks behind them.
fn marker_length(line: &str) -> usize {
    let blanks = line.len() - line.trim_start().len();
    let rest = &line[blanks..];
    let marks = if rest.starts_with('=') {
        rest.len() - rest.trim_start_matches('=').len()
    } else if rest.starts_with("- ") || rest.starts_with("+ ") {
        1
    } else {
        return 0;
    };
    let behind = &rest[marks..];
    if !behind.starts_with(' ') {
        return 0;
    }
    blanks + marks + behind.len() - behind.trim_start().len()
}

/// Markup for a change in running text: what goes struck through and
/// faint, what comes marked.
fn change_markup(old: &str, new: &str) -> String {
    let mut markup = String::new();
    if !old.is_empty() {
        let _ = write!(markup, "#text(fill: rgb(\"#cc241d\"))[#strike[{old}]];");
    }
    if !new.is_empty() {
        let _ = write!(
            markup,
            "#highlight(fill: rgb(\"#b8bb26\").transparentize(45%))[{new}];"
        );
    }
    markup
}

/// `text` as a Typst string literal: never read as markup.
fn literal(text: &str) -> String {
    let mut literal = String::with_capacity(text.len() + 2);
    literal.push('"');
    for c in text.chars() {
        match c {
            '\\' => literal.push_str("\\\\"),
            '"' => literal.push_str("\\\""),
            '\n' => literal.push_str("\\n"),
            '\r' => literal.push_str("\\r"),
            '\t' => literal.push_str("\\t"),
            c => literal.push(c),
        }
    }
    literal.push('"');
    literal
}

/// `text` in lines of at most `columns` characters, broken between
/// words, and no more than `most` of them.
fn wrap(text: &str, columns: usize, most: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let length = line.chars().count();
            if length > 0 && length + 1 + word.chars().count() > columns {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&cut(word, columns));
        }
        lines.push(line);
    }
    if lines.len() > most {
        lines.truncate(most);
        if let Some(last) = lines.last_mut() {
            last.push_str(" …");
        }
    }
    lines
}

/// Markup for the tag of a note in the margin: its first words on a
/// slip with a stripe of `stripe`'s colour.
fn tag_markup(words: &str, stripe: &str) -> String {
    format!(
        "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n\
         #set text(font: \"{FONT}\", size: {TAG_SIZE}pt, fill: rgb(\"#3c3836\"), hyphenate: false, \
         top-edge: \"ascender\", bottom-edge: \"descender\")\n\
         #box(fill: rgb(\"#fbf1c7\"), stroke: (left: 1.4pt + rgb(\"{stripe}\")), \
         inset: (x: 2.2pt, y: 1.2pt), radius: (right: 1.5pt), {})",
        literal(words)
    )
}

/// Markup for a panel of the reviewing: a `title`, `lines` of text and a
/// `hint` at the keys.
fn card_markup(title: &str, lines: &[String], hint: &str) -> String {
    let mut stacked = String::new();
    for line in lines {
        let _ = writeln!(stacked, "  {} + h(0pt),", literal(line));
    }
    format!(
        "#set page(width: auto, height: auto, margin: 8pt, fill: rgb(\"{BACKGROUND}\"))\n\
         #set text(font: \"{FONT}\", size: 9pt, fill: rgb(\"{FOREGROUND}\"), ligatures: false, hyphenate: false)\n\
         #stack(\n  spacing: 6pt,\n  text(fill: rgb(\"{ACCENT}\"), {}),\n{stacked}  text(fill: rgb(\"{DIM}\"), {}),\n)",
        literal(title),
        literal(hint)
    )
}

/// Where the notes of the file at `path` are kept under `state_dir`.
fn store_path(state_dir: &Path, path: &Path) -> PathBuf {
    // FNV-1a: a name that is the same in every run, for every path.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    state_dir.join("notes").join(format!("{hash:016x}.notes"))
}

/// A line of the store: a field's value, with what would break it into
/// lines written as escapes.
fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

fn unescape(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            plain.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => plain.push('\n'),
            Some('r') => plain.push('\r'),
            Some('t') => plain.push('\t'),
            Some(other) => plain.push(other),
            None => {}
        }
    }
    plain
}

/// The notes of the file at `path` as the store writes them.
fn stored(path: &Path, notes: &[&Note]) -> String {
    let mut out = format!("{HEADER}\npath\t{}\n", escape(&path.to_string_lossy()));
    let flag = |on: bool| if on { "yes" } else { "no" };
    for note in notes {
        let _ = writeln!(out, "note");
        let _ = writeln!(out, "range\t{}\t{}", note.range.start, note.range.end);
        let _ = writeln!(out, "quote\t{}", escape(&note.quote));
        let _ = writeln!(out, "text\t{}", escape(&note.text));
        let _ = writeln!(out, "by\t{}", note.by.name());
        let _ = writeln!(out, "question\t{}", flag(note.to_agent));
        let _ = writeln!(out, "resolved\t{}", flag(note.resolved));
        let _ = writeln!(out, "unseen\t{}", flag(note.unseen));
        for (by, text) in &note.answers {
            let _ = writeln!(out, "answer\t{}\t{}", by.name(), escape(text));
        }
        let _ = writeln!(out, "end");
    }
    out
}

/// The notes a store holds, for `file`, without their numbers yet. An
/// error if it is not a store of notes.
fn parse_store(text: &str, file: FileId) -> Result<Vec<Note>, String> {
    let mut lines = text.lines();
    if lines.next() != Some(HEADER) {
        return Err("it is not a store of notes this zetwerk can read".to_owned());
    }
    let author = |name: &str| match name {
        "agent" => Author::Agent,
        _ => Author::User,
    };
    let mut notes = Vec::new();
    let mut note: Option<Note> = None;
    for line in lines {
        let (key, value) = line.split_once('\t').unwrap_or((line, ""));
        if key == "note" {
            note = Some(Note {
                id: 0,
                file,
                range: 0..0,
                quote: String::new(),
                text: String::new(),
                by: Author::User,
                to_agent: false,
                answers: Vec::new(),
                resolved: false,
                detached: false,
                unseen: false,
            });
            continue;
        }
        let Some(current) = &mut note else {
            continue;
        };
        match key {
            "range" => {
                let (start, end) = value.split_once('\t').unwrap_or((value, value));
                let start = start.parse().unwrap_or(0);
                current.range = start..end.parse().unwrap_or(start).max(start);
            }
            "quote" => current.quote = unescape(value),
            "text" => current.text = unescape(value),
            "by" => current.by = author(value),
            "question" => current.to_agent = value == "yes",
            "resolved" => current.resolved = value == "yes",
            "unseen" => current.unseen = value == "yes",
            "answer" => {
                let (by, said) = value.split_once('\t').unwrap_or(("user", value));
                current.answers.push((author(by), unescape(said)));
            }
            "end" => notes.extend(note.take()),
            // What a later version writes and this one does not know.
            _ => {}
        }
    }
    Ok(notes)
}

/// Writes `contents` to the store at `store`, in a directory only the
/// user can enter.
fn write_store(store: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(directory) = store.parent() {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(directory)?;
    }
    zetwerk_core::write_atomically(store, contents.as_bytes())
}

/// Finds `note` in `text` again: where it was, or else where its text
/// is nearest to that. Detached if its text is nowhere.
fn reanchor(note: &mut Note, text: &str) {
    let fits = |range: &Range<usize>| {
        range.end <= text.len()
            && text.is_char_boundary(range.start)
            && text.is_char_boundary(range.end)
    };
    if !note.quote.is_empty() && fits(&note.range) && text[note.range.clone()] == note.quote {
        note.detached = false;
        return;
    }
    let near = text
        .match_indices(note.quote.as_str())
        .map(|(at, _)| at)
        .filter(|_| !note.quote.is_empty())
        .min_by_key(|at| at.abs_diff(note.range.start));
    if let Some(at) = near {
        note.range = at..at + note.quote.len();
        note.detached = false;
    } else {
        let mut at = note.range.start.min(text.len());
        while !text.is_char_boundary(at) {
            at -= 1;
        }
        note.range = at..at;
        note.detached = true;
    }
}

/// Where `quote` is in `text` within `reach` bytes of byte `around`,
/// the nearest to it.
fn near(text: &str, quote: &str, around: usize, reach: usize) -> Option<usize> {
    let mut from = around.saturating_sub(reach).min(text.len());
    let mut to = around.saturating_add(reach).min(text.len());
    while !text.is_char_boundary(from) {
        from -= 1;
    }
    while !text.is_char_boundary(to) {
        to += 1;
    }
    text[from..to]
        .match_indices(quote)
        .map(|(at, _)| from + at)
        .min_by_key(|at| at.abs_diff(around))
}

/// Whether the cursor at `cursor` is on `proposal`.
fn on_proposal(proposal: &Proposal, cursor: usize) -> bool {
    proposal.range.contains(&cursor) || proposal.range.start == cursor
}

/// Whether the cursor at `cursor` is on `note`.
fn on_note(note: &Note, cursor: usize) -> bool {
    !note.detached && (note.range.contains(&cursor) || note.range.start == cursor)
}

const NOT_EDITING: &str = super::agent::NOT_EDITING;

impl Viewer {
    /// Reads the stored notes of the file being edited, the first time
    /// they are wanted.
    pub(super) fn notes_ready(&mut self) {
        let file = self.file_id();
        if self.editor.is_none() || self.notes.loaded.contains(&file) {
            return;
        }
        self.notes.loaded.push(file);
        let (Some(state_dir), Some(editor)) = (&self.state_dir, &self.editor) else {
            return;
        };
        let store = store_path(state_dir, editor.path());
        let read = match std::fs::read_to_string(&store) {
            Ok(text) => parse_store(&text, file),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(err) => Err(err.to_string()),
        };
        match read {
            Ok(found) => {
                let text = editor.text();
                for mut note in found {
                    reanchor(&mut note, text);
                    self.notes.last += 1;
                    note.id = self.notes.last;
                    self.notes.all.push(note);
                }
                self.notes.all.sort_by_key(|note| note.range.start);
                self.remember_text();
            }
            Err(why) => {
                // Kept, not written over: it may be of a later zetwerk.
                let aside = store.with_extension("unreadable");
                let _ = std::fs::rename(&store, &aside);
                self.alert(format!(
                    "the notes of this file could not be read ({why}); they are kept in {}",
                    aside.display()
                ));
            }
        }
    }

    /// Notes the text the places of the notes of the file being edited
    /// are places in, where it is not noted yet.
    fn remember_text(&mut self) {
        let file = self.file_id();
        let Some(editor) = &self.editor else {
            return;
        };
        if self.notes.seen.as_ref().is_some_and(|(of, _)| *of == file) {
            return;
        }
        let text = editor.text();
        for note in self.notes.all.iter_mut().filter(|note| note.file == file) {
            reanchor(note, text);
        }
        self.notes.seen = Some((file, text.to_owned()));
    }

    /// Moves the notes of the file being edited along with a change of
    /// its text. Cheap where it has none.
    pub(super) fn sync_notes(&mut self) {
        let file = self.file_id();
        if !self.notes.all.iter().any(|note| note.file == file) {
            self.notes.seen = None;
            return;
        }
        if !self.notes.seen.as_ref().is_some_and(|(of, _)| *of == file) {
            return self.remember_text();
        }
        let (Some(editor), Some((_, before))) = (&self.editor, &mut self.notes.seen) else {
            return;
        };
        let now = editor.text();
        let Difference::Replacement {
            offset,
            removed,
            text,
        } = difference(before, now)
        else {
            return;
        };
        let change = (offset, removed, text.len());
        for note in self.notes.all.iter_mut().filter(|note| note.file == file) {
            let start = moved_place(note.range.start, change, false);
            let end = moved_place(note.range.end, change, true).max(start);
            note.range = start..end;
            if note.range.is_empty() && !note.quote.is_empty() {
                // All of its text is gone; an undo may bring it back
                // where it was.
                let reach = note.quote.len() + change.2;
                if let Some(at) = near(now, &note.quote, start, reach) {
                    note.range = at..at + note.quote.len();
                }
            }
            note.detached = note.range.is_empty();
        }
        before.clear();
        before.push_str(now);
        self.notes.moved = true;
    }

    /// Writes the notes where their places moved since they were written:
    /// when the journal is forced to disk. Those of every open file, as
    /// another may have been edited since.
    pub(super) fn keep_notes(&mut self) {
        if !std::mem::take(&mut self.notes.moved) {
            return;
        }
        let mut files: Vec<FileId> = Vec::new();
        for note in &self.notes.all {
            if !files.contains(&note.file) {
                files.push(note.file);
            }
        }
        for file in files {
            self.save_notes(file);
        }
    }

    /// Writes the notes of `file` to its store, with what they are about
    /// as it is now.
    fn save_notes(&mut self, file: FileId) {
        let Some(state_dir) = &self.state_dir else {
            return;
        };
        let Some(editor) = self
            .editor
            .iter()
            .chain(&self.others)
            .find(|e| e.file() == file)
        else {
            return;
        };
        let text = editor.text();
        for note in self.notes.all.iter_mut().filter(|note| note.file == file) {
            if let Some(quote) = text
                .get(note.range.clone())
                .filter(|quote| !quote.is_empty())
            {
                quote.clone_into(&mut note.quote);
            }
        }
        let store = store_path(state_dir, editor.path());
        let notes: Vec<&Note> = self
            .notes
            .all
            .iter()
            .filter(|note| note.file == file)
            .collect();
        if notes.is_empty() && !store.exists() {
            return;
        }
        if let Err(err) = write_store(&store, &stored(editor.path(), &notes)) {
            self.alert(format!("the notes could not be kept: {err}"));
        }
    }

    /// Adds a note on bytes `range` of the file being edited. Returns its
    /// number.
    fn add_note(&mut self, range: Range<usize>, text: &str, by: Author, to_agent: bool) -> u64 {
        self.notes_ready();
        let file = self.file_id();
        let quote = self
            .text()
            .and_then(|whole| whole.get(range.clone()))
            .unwrap_or_default()
            .to_owned();
        self.notes.last += 1;
        let id = self.notes.last;
        self.notes.all.push(Note {
            id,
            file,
            range,
            quote,
            text: text.trim().to_owned(),
            by,
            to_agent,
            answers: Vec::new(),
            resolved: false,
            detached: false,
            unseen: by == Author::Agent,
        });
        self.notes.all.sort_by_key(|note| note.range.start);
        self.remember_text();
        self.save_notes(file);
        id
    }

    /// What a note begun at the cursor is about: what is selected, or
    /// the thing the cursor is in.
    fn noted_here(&self) -> Option<Range<usize>> {
        let editor = self.editor.as_ref()?;
        if let Some(selected) = editor.selection().filter(|range| !range.is_empty()) {
            return Some(selected);
        }
        thing_at(editor.source(), editor.cursor())
    }

    /// `gn`, `gq`: what the note or the question is about is taken now,
    /// and the command line is opened for its text.
    fn begin_note(&mut self, ask: bool) {
        let Some(range) = self.noted_here() else {
            self.alert("there is nothing here to note: the line is empty");
            return;
        };
        let Some(editor) = &mut self.editor else {
            return;
        };
        let command = if ask { ":ask " } else { ":note " };
        for c in command.chars() {
            editor.key(KeyPress::plain(Key::Char(c)), &Shown::Pages(None));
        }
        self.notes.pending = Some((range, self.revision, editor.cursor()));
    }

    /// `:note text`, `:ask question`: a note on what `gn` or `gq` took,
    /// or on what is selected or under the cursor.
    fn note_here(&mut self, text: &str, ask: bool) {
        let pending = self.notes.pending.take();
        let Some(cursor) = self.editor.as_ref().map(editor::Editor::cursor) else {
            return;
        };
        let range = match pending {
            Some((range, revision, then)) if revision == self.revision && then == cursor => {
                Some(range)
            }
            _ => self.noted_here(),
        };
        let Some(range) = range else {
            self.alert("there is nothing here to note: the line is empty");
            return;
        };
        if let Some(editor) = &mut self.editor {
            // A selection is over with the note on it.
            editor.point(cursor);
        }
        self.add_note(range, text, Author::User, ask);
        self.notice(if ask {
            "asked: an agent sees the question (list_notes) and answers it here; :notes lists them"
        } else {
            "noted; :notes lists the notes, ]n goes to the next"
        });
    }

    /// The commands of reviewing. Returns whether `command` was one.
    pub(super) fn notes_command(&mut self, command: &str) -> bool {
        let (name, rest) = command
            .split_once(' ')
            .map_or((command, ""), |(name, rest)| (name, rest.trim()));
        match (name, rest) {
            ("note" | "ask" | "notes", _) if self.editor.is_none() => self.alert(NOT_EDITING),
            ("note" | "ask", "") => self.begin_note(name == "ask"),
            ("note" | "ask", text) => self.note_here(text, name == "ask"),
            ("notes", rest) => {
                self.notes_ready();
                self.notes_subcommand(rest);
            }
            ("proposal", "flip") => self.flip_proposal(),
            _ => return false,
        }
        true
    }

    /// `:notes …`: the list, going to a note, and what can be done to
    /// one.
    fn notes_subcommand(&mut self, rest: &str) {
        let (verb, more) = rest
            .split_once(' ')
            .map_or((rest, ""), |(verb, more)| (verb, more.trim()));
        let (number, words) = more
            .split_once(' ')
            .map_or((more, ""), |(number, words)| (number, words.trim()));
        let at_cursor = || {
            let cursor = self.editor.as_ref().map(editor::Editor::cursor)?;
            let file = self.file_id();
            let mut here = self.notes.all.iter().filter(|note| note.file == file);
            here.rfind(|note| on_note(note, cursor)).map(|note| note.id)
        };
        let id = number.parse::<u64>().ok().or_else(|| match verb {
            "resolve" | "delete" => at_cursor(),
            _ => None,
        });
        match (verb, id) {
            ("", _) => self.list_notes(),
            ("next" | "previous", _) => self.goto_note(verb == "next"),
            ("go", Some(id)) => self.go_to_note(id),
            ("answer", Some(id)) if !words.is_empty() => self.answer_note(id, words),
            ("resolve", Some(id)) => self.resolve_note(id, Author::User),
            ("delete", Some(id)) => self.delete_note(id),
            ("proposal", Some(id)) => self.go_to_answer(id),
            (number, _) if number.parse::<u64>().is_ok() => {
                self.note_menu(number.parse().unwrap_or_default());
            }
            ("answer", _) => self.alert("answer which note, and what? :notes answer 3 the answer"),
            _ => self.alert("which note? the cursor is on none: :notes lists them"),
        }
    }

    /// The position of note `id` among all notes, or a message saying
    /// there is none.
    fn note_index(&self, id: u64) -> Result<usize, String> {
        self.notes
            .all
            .iter()
            .position(|note| note.id == id)
            .ok_or_else(|| format!("there is no note {id}; list_notes gives them"))
    }

    /// The name of `file`, from the document's directory.
    fn file_name_of(&self, file: FileId) -> String {
        let editor = self
            .editor
            .iter()
            .chain(&self.others)
            .find(|e| e.file() == file);
        editor.map_or_else(
            || file.vpath().get_without_slash().to_owned(),
            |editor| {
                let path = editor.path();
                let shown = path.strip_prefix(self.project.root_dir()).unwrap_or(path);
                shown.to_string_lossy().into_owned()
            },
        )
    }

    /// The text of `file`, if it is open.
    fn text_of(&self, file: FileId) -> Option<&str> {
        let mut editors = self.editor.iter().chain(&self.others);
        editors.find(|e| e.file() == file).map(editor::Editor::text)
    }

    /// `:notes`: all notes, those of this file first, to go to one and
    /// see what can be done to it.
    fn list_notes(&mut self) {
        let file = self.file_id();
        let mut order: Vec<&Note> = self.notes.all.iter().collect();
        order.sort_by_key(|note| (note.resolved, note.file != file));
        let items: Vec<(String, Target)> = order
            .iter()
            .map(|note| {
                let line = self
                    .text_of(note.file)
                    .map_or(0, |text| line_column(text, note.range.start).0);
                let mut about = format!("line {line}");
                if note.file != file {
                    let _ = write!(about, " of {}", self.file_name_of(note.file));
                }
                let kind = match (note.resolved, note.answers.is_empty(), note.to_agent) {
                    (true, ..) => ", resolved",
                    (false, false, _) if note.unseen => ", answered (new)",
                    (false, false, _) => ", answered",
                    (false, true, true) => ", question for agents",
                    (false, true, false) if note.by == Author::Agent => ", from an agent",
                    _ => "",
                };
                let first = note.text.lines().next().unwrap_or_default();
                let label = format!("{}\t{about}{kind}", cut(first, 60));
                (label, Target::Command(format!("notes {}", note.id)))
            })
            .collect();
        self.open_picker("notes", items, 0);
    }

    /// `:notes 3`: note 3 in full, and what can be done to it.
    fn note_menu(&mut self, id: u64) {
        let Ok(at) = self.note_index(id) else {
            return self.alert(format!("there is no note {id}"));
        };
        let proposals = self.answering(id);
        let note = &mut self.notes.all[at];
        note.unseen = false;
        let mut items: Vec<(String, Target)> = Vec::new();
        let said = |by: Author| match by {
            Author::User => "you",
            Author::Agent => "agent",
        };
        for line in wrap(&note.text, 70, CARD_LINES) {
            items.push((format!("{}: {line}", said(note.by)), Target::Nothing));
        }
        for (by, text) in &note.answers {
            for line in wrap(text, 70, CARD_LINES) {
                items.push((format!("{}: {line}", said(*by)), Target::Nothing));
            }
        }
        let chosen = items.len();
        items.push((
            "go to it".to_owned(),
            Target::Command(format!("notes go {id}")),
        ));
        if !proposals.is_empty() {
            let go = Target::Command(format!("notes proposal {id}"));
            items.push(("the change an agent proposes with it".to_owned(), go));
        }
        items.push((
            "answer …".to_owned(),
            Target::Ask(format!("notes answer {id} ")),
        ));
        let resolve = if note.resolved {
            "open again"
        } else {
            "resolve"
        };
        items.push((
            resolve.to_owned(),
            Target::Command(format!("notes resolve {id}")),
        ));
        items.push((
            "delete".to_owned(),
            Target::Command(format!("notes delete {id}")),
        ));
        self.notes.moved = true;
        self.open_picker_for("note", items, chosen, "notes".to_owned(), None);
    }

    /// The proposals that were made as answers to note `id` and wait.
    fn answering(&self, id: u64) -> Vec<u64> {
        let waiting = self.proposals.iter().chain(&self.proposals_elsewhere);
        waiting
            .filter(|proposal| {
                self.notes
                    .about
                    .get(&proposal.id)
                    .and_then(|about| about.note)
                    == Some(id)
            })
            .map(|proposal| proposal.id)
            .collect()
    }

    /// Puts the cursor on note `id`, in its file.
    fn go_to_note(&mut self, id: u64) {
        let Ok(at) = self.note_index(id) else {
            return self.alert(format!("there is no note {id}"));
        };
        let note = &mut self.notes.all[at];
        note.unseen = false;
        let (file, start) = (note.file, note.range.start);
        self.notes.moved = true;
        self.open_file(file, Some(start));
    }

    /// Puts the cursor on the change proposed as an answer to note `id`.
    fn go_to_answer(&mut self, id: u64) {
        let Some(proposal) = self.answering(id).first().copied() else {
            return self.alert("no change waits that answers this note");
        };
        let start = self
            .proposals
            .iter()
            .find(|waiting| waiting.id == proposal)
            .map(|waiting| waiting.range.start);
        match (start, &mut self.editor) {
            (Some(start), Some(editor)) => {
                editor.jump(start);
                self.follow_cursor();
            }
            _ => self.alert("the change is proposed for another file: go to the note there"),
        }
    }

    /// `]n`, `[n`: to the next or the previous note of this file that is
    /// not resolved.
    fn goto_note(&mut self, forward: bool) {
        let file = self.file_id();
        let Some(editor) = &mut self.editor else {
            return;
        };
        let cursor = editor.cursor();
        let starts: Vec<usize> = self
            .notes
            .all
            .iter()
            .filter(|note| note.file == file && !note.resolved && !note.detached)
            .map(|note| note.range.start)
            .collect();
        let target = if forward {
            starts
                .iter()
                .find(|start| **start > cursor)
                .or(starts.first())
        } else {
            starts
                .iter()
                .rfind(|start| **start < cursor)
                .or(starts.last())
        };
        match target {
            Some(start) => {
                editor.jump(*start);
                self.follow_cursor();
            }
            None => self.notice("no notes here: gn leaves one, gq asks an agent"),
        }
    }

    /// The user's answer to note `id`.
    fn answer_note(&mut self, id: u64, text: &str) {
        let Ok(at) = self.note_index(id) else {
            return self.alert(format!("there is no note {id}"));
        };
        let note = &mut self.notes.all[at];
        note.answers.push((Author::User, text.to_owned()));
        note.unseen = false;
        let file = note.file;
        self.save_notes(file);
        self.notice("answered; an agent sees it with list_notes");
    }

    /// Marks note `id` resolved, or open again if it was; for an agent,
    /// resolved only.
    fn resolve_note(&mut self, id: u64, by: Author) {
        let Ok(at) = self.note_index(id) else {
            return self.alert(format!("there is no note {id}"));
        };
        let note = &mut self.notes.all[at];
        note.resolved = by == Author::Agent || !note.resolved;
        note.unseen = by == Author::Agent;
        let (file, resolved) = (note.file, note.resolved);
        self.save_notes(file);
        self.notice(match (by, resolved) {
            (Author::Agent, _) => "an agent resolved a note; :notes has it",
            (Author::User, true) => "resolved; :notes still has it",
            (Author::User, false) => "open again",
        });
    }

    /// Deletes note `id`, answers and all.
    fn delete_note(&mut self, id: u64) {
        let Ok(at) = self.note_index(id) else {
            return self.alert(format!("there is no note {id}"));
        };
        let file = self.notes.all.remove(at).file;
        self.save_notes(file);
        self.notice("the note is deleted");
    }

    /// `zp`: the proposal under the cursor shown as the page is, or as it
    /// would be.
    fn flip_proposal(&mut self) {
        let Some(cursor) = self.editor.as_ref().map(editor::Editor::cursor) else {
            return;
        };
        let Some(id) = self
            .proposals
            .iter()
            .find(|proposal| on_proposal(proposal, cursor))
            .map(|proposal| proposal.id)
        else {
            return self.alert("the cursor is on no proposal; ]p goes to the next one");
        };
        self.notes.before = if self.notes.before == Some(id) {
            None
        } else {
            Some(id)
        };
        self.notice(if self.notes.before.is_some() {
            "as it is; zp shows it as it would be"
        } else {
            "as it would be; zp shows it as it is"
        });
    }

    /// What the status line says of notes agents wrote that the user has
    /// not seen.
    pub(super) fn notes_status(&self) -> String {
        let unseen = self
            .notes
            .all
            .iter()
            .filter(|note| note.unseen && !note.resolved);
        let (answers, notes) = unseen.fold((0, 0), |(answers, notes), note| {
            if note.answers.is_empty() {
                (answers, notes + 1)
            } else {
                (answers + 1, notes)
            }
        });
        let plural = |count: usize, one: &str, many: &str| match count {
            0 => None,
            1 => Some(format!("1 {one}")),
            count => Some(format!("{count} {many}")),
        };
        let told: Vec<String> = [
            plural(answers, "answer", "answers"),
            plural(notes, "note", "notes"),
        ]
        .into_iter()
        .flatten()
        .collect();
        if told.is_empty() {
            String::new()
        } else {
            format!("{} :notes  ", told.join(", "))
        }
    }
}

impl Viewer {
    /// The notes, those of all open files, each file's in the order of
    /// its text.
    #[must_use]
    pub fn notes(&self) -> &[Note] {
        &self.notes.all
    }

    /// The notes for an agent: all of them, or those not resolved.
    pub fn notes_listed(&mut self, include_resolved: bool) -> Vec<Listed> {
        self.notes_ready();
        let current = self.file_id();
        let mut listed: Vec<Listed> = self
            .notes
            .all
            .iter()
            .filter(|note| include_resolved || !note.resolved)
            .map(|note| {
                let text = self.text_of(note.file).unwrap_or_default();
                let (line, column) = line_column(text, note.range.start);
                let about = text.get(note.range.clone()).unwrap_or_default();
                Listed {
                    id: note.id,
                    file: self.file_name_of(note.file),
                    line,
                    column,
                    text: cut(about, MAX_QUOTE),
                    note: note.text.clone(),
                    by: note.by,
                    to_agent: note.to_agent,
                    answers: note.answers.clone(),
                    resolved: note.resolved,
                    detached: note.detached,
                    proposals: self.answering(note.id),
                }
            })
            .collect();
        let name = self.file_name_of(current);
        listed.sort_by_key(|note| note.file != name);
        listed
    }

    /// Leaves an agent's note where `anchor` says, on the file being
    /// edited, if the text is still at `expected_revision`. Returns its
    /// number and the line it is on.
    pub fn agent_add_note(
        &mut self,
        text: &str,
        anchor: NoteAnchor,
        expected_revision: Option<u64>,
    ) -> Result<(u64, usize), String> {
        self.expect(expected_revision)?;
        if text.trim().is_empty() {
            return Err("the note is empty".to_owned());
        }
        let whole = self.text().ok_or(NOT_EDITING)?;
        let range = match anchor {
            NoteAnchor::Text("") => return Err("anchor_text is empty".to_owned()),
            NoteAnchor::Text(wanted) => {
                let found: Vec<usize> = whole.match_indices(wanted).map(|(at, _)| at).collect();
                match found.as_slice() {
                    [] => {
                        return Err(
                            "anchor_text is not in the text; read the source again".to_owned()
                        );
                    }
                    [at] => *at..*at + wanted.len(),
                    _ => {
                        return Err(format!(
                            "anchor_text is in the text {} times; give more of the text around it",
                            found.len()
                        ));
                    }
                }
            }
            NoteAnchor::Line(line) => {
                let at = offset_of(whole, line, 1)
                    .ok_or_else(|| format!("the text has no line {line}"))?;
                line_at(whole, at).ok_or_else(|| format!("line {line} is empty"))?
            }
        };
        let line = line_column(whole, range.start).0;
        let id = self.add_note(range, text, Author::Agent, false);
        self.notice(format!(
            "an agent left a note on line {line}; :notes has it"
        ));
        Ok((id, line))
    }

    /// An agent's answer to note `id`, if the text is still at
    /// `expected_revision`; with `change` (text to replace, once, and what
    /// to put there), also a proposal of that change that belongs to the
    /// note: never the change itself. Returns the proposal's number.
    pub fn agent_answer_note(
        &mut self,
        id: u64,
        text: &str,
        change: Option<(&str, &str)>,
        expected_revision: Option<u64>,
    ) -> Result<Option<u64>, String> {
        self.expect(expected_revision)?;
        self.notes_ready();
        let at = self.note_index(id)?;
        if text.trim().is_empty() {
            return Err("the answer is empty".to_owned());
        }
        let file = self.notes.all[at].file;
        let proposal = match change {
            Some(_) if file != self.file_id() => {
                return Err(format!(
                    "the note is about {}, which is not the file being edited: answer it in words",
                    self.file_name_of(file)
                ));
            }
            Some((old, new)) => {
                let first = self.next_proposal;
                match self.agent_replace(old, new, false, None, true)? {
                    Outcome::Proposed { id: proposal, .. } => {
                        self.explain(first, Some(text), Some(id));
                        Some(proposal)
                    }
                    Outcome::Changed { .. } => None,
                }
            }
            None => None,
        };
        let note = &mut self.notes.all[at];
        note.answers.push((Author::Agent, text.trim().to_owned()));
        note.unseen = true;
        let line = self.text_of(file).map_or(0, |whole| {
            line_column(whole, self.notes.all[at].range.start).0
        });
        self.save_notes(file);
        let first: String = text
            .split_whitespace()
            .take(8)
            .collect::<Vec<_>>()
            .join(" ");
        self.notice(format!(
            "an agent answered the note on line {line}: “{}” — :notes",
            cut(&first, 60)
        ));
        Ok(proposal)
    }

    /// An agent marks note `id` resolved.
    pub fn agent_resolve_note(&mut self, id: u64) -> Result<(), String> {
        self.notes_ready();
        self.note_index(id)?;
        self.resolve_note(id, Author::Agent);
        Ok(())
    }

    /// How many questions for agents wait for an answer (none was
    /// given, or the user said more since), and how many notes of the
    /// user's nobody has answered: told to an agent with the state of
    /// the editor, so that it need not ask for the notes to know.
    pub fn notes_waiting(&mut self) -> (usize, usize) {
        self.notes_ready();
        let open = self
            .notes
            .all
            .iter()
            .filter(|note| !note.resolved && note.by == Author::User);
        let (mut questions, mut unanswered) = (0, 0);
        for note in open {
            let last = note.answers.last().map(|(by, _)| *by);
            if note.to_agent && last != Some(Author::Agent) {
                questions += 1;
            } else if !note.to_agent && last.is_none() {
                unanswered += 1;
            }
        }
        (questions, unanswered)
    }

    /// The number the next proposal gets: those made from then on are a
    /// change's that was asked for then.
    #[must_use]
    pub fn next_proposal_id(&self) -> u64 {
        self.next_proposal
    }

    /// Notes why the agent proposed the changes numbered `first` and
    /// later.
    pub fn explain_proposals(&mut self, first: u64, reason: &str) {
        self.explain(first, Some(reason), None);
    }

    fn explain(&mut self, first: u64, reason: Option<&str>, note: Option<u64>) {
        let waiting: Vec<u64> = self
            .proposals
            .iter()
            .chain(&self.proposals_elsewhere)
            .map(|proposal| proposal.id)
            .collect();
        self.notes.about.retain(|id, _| waiting.contains(id));
        for id in waiting.into_iter().filter(|id| *id >= first) {
            let about = About {
                reason: reason.map(str::to_owned),
                note,
            };
            self.notes.about.insert(id, about);
        }
    }

    /// Why the agent proposed change `id`, and the note it answers.
    #[must_use]
    pub fn proposal_about(&self, id: u64) -> (Option<&str>, Option<u64>) {
        self.notes
            .about
            .get(&id)
            .map_or((None, None), |about| (about.reason.as_deref(), about.note))
    }
}

/// What a proposal would make of the paragraph it is in: the markup that
/// sets it so, the area of the page it stands in (left, top, right and
/// bottom, in points), and the room it has.
struct Preview {
    body: String,
    area: (f64, f64, f64, f64),
    family: String,
    size: f64,
    weight: u16,
    italic: bool,
    room: f64,
}

impl Viewer {
    /// Draws on page `index` (whose top-left corner is at `origin`) what
    /// reviewing adds to it: the notes, and the proposal under the cursor
    /// as it would be.
    pub(super) fn review_page(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        index: usize,
        origin: (f64, f64),
    ) {
        if self.editor.is_none() {
            return;
        }
        self.notes_ready();
        self.note_marks(scene, has, index, origin);
        self.proposal_preview(scene, has, index, origin);
    }

    /// The notes on page `index`: a faint mark on their text and a tag in
    /// the right margin with their first words.
    fn note_marks(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        index: usize,
        origin: (f64, f64),
    ) {
        let file = self.file_id();
        let shown: Vec<(Range<usize>, String)> = self
            .notes
            .all
            .iter()
            .filter(|note| note.file == file && !note.resolved && !note.detached)
            .map(|note| {
                let stripe = match (note.answers.is_empty(), note.to_agent) {
                    (false, _) => STRIPE_ANSWERED,
                    (true, true) => STRIPE_QUESTION,
                    (true, false) => STRIPE_NOTE,
                };
                let first = note.text.lines().next().unwrap_or_default();
                let words = if note.to_agent {
                    format!("? {}", cut(first, TAG_WORDS))
                } else {
                    cut(first, TAG_WORDS)
                };
                (note.range.clone(), tag_markup(&words, stripe))
            })
            .collect();
        if shown.is_empty() {
            return;
        }
        // While the page is worked out from what is typed, the notes
        // stay where the page as compiled has them, but on the line typed
        // into: its words have moved. The tags stay, not to come and go
        // with every key.
        let Some(change) = self.places_of_text() else {
            return;
        };
        let typed = self
            .predicted
            .as_ref()
            .filter(|predicted| predicted.index == index);
        let typed = typed.map(|predicted| predicted.typed);
        let (Some(document), Some(world)) = (&self.document, &self.compiled_world) else {
            return;
        };
        let Some(page) = document.pages().get(index) else {
            return;
        };
        let zoom = self.view.zoom;
        let width = page.frame.width().to_pt();
        let map = self
            .maps
            .entry(index)
            .or_insert_with(|| PageMap::build(world, &page.frame));
        let place = |rect: Rect| {
            (
                origin.0 + rect.min.x.to_pt() * zoom,
                origin.1 + rect.min.y.to_pt() * zoom,
                (rect.max.x - rect.min.x).to_pt() * zoom,
                (rect.max.y - rect.min.y).to_pt() * zoom,
            )
        };
        let line = (1.2 * self.scale).round().max(1.0);
        let mut tags = Vec::new();
        for (range, markup) in shown {
            let range = change.back_or_edge(range.start)..change.back_or_edge(range.end);
            let rects = map.selection(file, &range);
            for rect in &rects {
                if typed.is_some_and(|typed| rect.min.y < typed.max.y && rect.max.y > typed.min.y) {
                    continue;
                }
                let (x, y, w, h) = place(*rect);
                tint(scene, has, NOTE_FILL, (x, y, w, h));
                tint(scene, has, NOTE_LINE, (x, y + h - line, w, line));
            }
            if let Some(first) = rects.first() {
                tags.push((first.min.y.to_pt(), markup));
            }
        }
        tags.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut below = f64::NEG_INFINITY;
        for (top, markup) in tags {
            let Some((id, (w, h))) = self.tag_image(scene, has, &markup, zoom) else {
                continue;
            };
            let (w, h) = (f64::from(w), f64::from(h));
            let x = origin.0 + (width - TAG_GAP) * zoom - w;
            let y = (origin.1 + top * zoom).max(below);
            below = y + h + 1.0;
            scene.quads.push(Quad {
                texture: id,
                x: x.round() as f32,
                y: y.round() as f32,
                width: w as f32,
                height: h as f32,
            });
        }
    }

    /// The image of a tag, made only where the compositor does not hold
    /// it: tags are many, and the same from frame to frame.
    fn tag_image(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        markup: &str,
        zoom: f64,
    ) -> Option<(TextureId, (u32, u32))> {
        let id = Chrome::id(markup, zoom);
        if has(id)
            && let Some(size) = self.notes.sizes.get(&id)
        {
            return Some((id, *size));
        }
        let image = self.chrome.render(markup, zoom)?;
        let size = (image.pixmap.width(), image.pixmap.height());
        if self.notes.sizes.len() >= SIZES_KEPT {
            self.notes.sizes.clear();
        }
        self.notes.sizes.insert(image.id, size);
        let id = image.id;
        if !has(id) && !scene.images.iter().any(|known| known.id == id) {
            scene.images.push(image);
        }
        Some((id, size))
    }

    /// What the proposal under the cursor would make of its paragraph on
    /// page `index`, if it begins there and can be shown so.
    fn preview_of(&mut self, index: usize) -> Option<Preview> {
        let editor = self.editor.as_ref()?;
        if editor.mode() != editor::Mode::Normal {
            return None;
        }
        let cursor = editor.cursor();
        let proposal = self
            .proposals
            .iter()
            .find(|proposal| on_proposal(proposal, cursor))?;
        if proposal.stale || self.notes.before == Some(proposal.id) {
            return None;
        }
        let change = self.places()?;
        let text = editor.text();
        let range = proposal.range.clone();
        let start = text[..range.start].rfind('\n').map_or(0, |at| at + 1);
        let end = text[range.end..]
            .find('\n')
            .map_or(text.len(), |at| range.end + at);
        let (from, to) = (change.back(start)?, change.back(end)?);
        let (document, world) = (self.document.as_ref()?, self.compiled_world.as_ref()?);
        let page = document.pages().get(index)?;
        let file = editor.file();
        let map = self
            .maps
            .entry(index)
            .or_insert_with(|| PageMap::build(world, &page.frame));
        // On the page it begins on.
        let at = change.back_or_edge(range.start);
        let begins = map.caret(file, at).or_else(|| map.caret_before(file, at))?;
        let rects = map.selection(file, &(from..to));
        let first = rects.first()?;
        let mut area = (
            first.min.x.to_pt(),
            first.min.y.to_pt(),
            first.max.x.to_pt(),
            first.max.y.to_pt(),
        );
        let mut lines = 1;
        for rect in &rects[1..] {
            if rect.min.y.to_pt() > area.3 - 0.5 {
                lines += 1;
            }
            area = (
                area.0.min(rect.min.x.to_pt()),
                area.1.min(rect.min.y.to_pt()),
                area.2.max(rect.max.x.to_pt()),
                area.3.max(rect.max.y.to_pt()),
            );
        }
        let run_of = map.caret(file, from).map_or(begins.run, |caret| caret.run);
        let run = map.runs().get(run_of)?;
        let lead = marker_length(&text[start..range.start]);
        let body = format!(
            "{SHOW_REFS}{}{}{}",
            &text[start + lead..range.start],
            change_markup(&text[range.clone()], &proposal.new),
            &text[range.end..end],
        );
        let width = page.frame.width().to_pt();
        let measure = area.2 - area.0;
        let room = if lines > 1 {
            measure
        } else {
            measure.max(width - 2.0 * area.0)
        };
        Some(Preview {
            body,
            area,
            family: run.family.clone(),
            size: run.size,
            weight: run.weight,
            italic: run.italic,
            room: room.max(LEAST_ROOM),
        })
    }

    /// Draws the proposal under the cursor as what it would make of the
    /// page, over the paragraph it is in: what goes struck through, what
    /// comes marked. Nothing where it cannot be typeset by itself: the
    /// panel of the proposal says what it is then.
    fn proposal_preview(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        index: usize,
        origin: (f64, f64),
    ) {
        let Some(preview) = self.preview_of(index) else {
            return;
        };
        let zoom = self.view.zoom;
        let style = InPlace {
            family: Some(preview.family.as_str()),
            size: preview.size,
            weight: preview.weight.max(100),
            italic: preview.italic,
            width: preview.room,
        };
        let markup = chrome::in_place_markup(&preview.body, &style, false);
        let Some(image) = self.typeset_apart(&markup, zoom) else {
            return;
        };
        let (left, top, right, bottom) = preview.area;
        let (x, y) = (origin.0 + left * zoom, origin.1 + top * zoom);
        let size = (
            f64::from(image.pixmap.width()),
            f64::from(image.pixmap.height()),
        );
        let cover = (
            x - 2.0,
            y - 2.0,
            ((right - left) * zoom).max(size.0) + 4.0,
            ((bottom - top) * zoom).max(size.1) + 4.0,
        );
        solid(scene, has, PAPER, cover);
        let bar = (2.0 * self.scale).round().max(1.0);
        tint(
            scene,
            has,
            super::PAGE_PROPOSAL,
            (x - 3.0 * bar, y, bar, size.1),
        );
        scene.quads.push(Quad {
            texture: image.id,
            x: x.round() as f32,
            y: y.round() as f32,
            width: size.0 as f32,
            height: size.1 as f32,
        });
        if !has(image.id) {
            scene.images.push(image);
        }
        self.notes.previewed = true;
    }

    /// The image of `markup`, typeset on the thread of the previews:
    /// `None` until it is back, and where it does not typeset. One piece
    /// at a time; the window is woken when it is done, and asks for the
    /// piece it wants then.
    fn typeset_apart(&mut self, markup: &str, zoom: f64) -> Option<Image> {
        let id = Chrome::id(markup, zoom);
        let notes = &mut self.notes;
        if let Some(typesetter) = &notes.typesetter {
            while let Ok(back) = typesetter.done.try_recv() {
                if notes.typesetting == Some(back.0) {
                    notes.typesetting = None;
                }
                notes.typeset = Some(back);
            }
        }
        if let Some((done, image)) = &notes.typeset
            && *done == id
        {
            return image.clone();
        }
        if notes.typesetting.is_some() {
            return None;
        }
        if notes.typesetter.is_none() {
            // Without the thread, the proposal's panel says what it is.
            notes.typesetter = Typesetter::spawn(notes.wake.clone()).ok();
        }
        let job = Box::new(self.chrome.render_apart(markup, zoom));
        let notes = &mut self.notes;
        let sent = notes
            .typesetter
            .as_ref()
            .is_some_and(|typesetter| typesetter.jobs.send((id, job)).is_ok());
        if sent {
            notes.typesetting = Some(id);
        } else {
            notes.typesetter = None;
        }
        None
    }

    /// Has `wake` called when a proposal was typeset as it would be: it
    /// should make the window draw.
    pub fn wake_for_review(&mut self, wake: impl Fn() + Send + Sync + 'static) {
        self.notes.wake = Some(Arc::new(wake));
    }

    /// Waits until the proposal given to the thread of the previews is
    /// back, for a test to draw it.
    #[cfg(test)]
    fn preview_settled(&mut self) {
        let Some(typesetter) = &self.notes.typesetter else {
            return;
        };
        if self.notes.typesetting.is_some()
            && let Ok(back) = typesetter
                .done
                .recv_timeout(std::time::Duration::from_secs(60))
        {
            self.notes.typesetting = None;
            self.notes.typeset = Some(back);
        }
    }

    /// Whether the proposal under the cursor is shown by reviewing, as
    /// it would be or flipped to as it is, and needs no other panel.
    pub(super) fn proposal_explained(&self) -> bool {
        let cursor = self.editor.as_ref().map(editor::Editor::cursor);
        let flipped = self.proposals.iter().any(|proposal| {
            Some(proposal.id) == self.notes.before
                && cursor.is_some_and(|cursor| on_proposal(proposal, cursor))
        });
        self.notes.previewed || flipped
    }

    /// The panels of reviewing, in the bottom-left corner: what the
    /// proposal under the cursor is and why it was made, and the note
    /// under the cursor with what was said to it.
    pub(super) fn review_panels(&mut self, scene: &mut Scene, has: &dyn Fn(TextureId) -> bool) {
        let explained = self.proposal_explained();
        self.notes.previewed = false;
        let Some(cursor) = self.editor.as_ref().map(editor::Editor::cursor) else {
            return;
        };
        let on = self
            .proposals
            .iter()
            .position(|proposal| on_proposal(proposal, cursor));
        if self.notes.before.is_some() && on.map(|at| self.proposals[at].id) != self.notes.before {
            self.notes.before = None;
        }
        let margin = PANEL_MARGIN * self.scale;
        let mut bottom = f64::from(self.window.1) - f64::from(self.status_height()) - margin;
        if let (Some(at), true) = (on, explained) {
            let markup = self.proposal_card(at);
            bottom = self.corner_panel(scene, has, &markup, bottom);
        }
        let file = self.file_id();
        let note = self
            .notes
            .all
            .iter_mut()
            .rfind(|note| note.file == file && on_note(note, cursor));
        if let Some(note) = note {
            if note.unseen {
                note.unseen = false;
                self.notes.moved = true;
            }
            let markup = note_card(note);
            self.corner_panel(scene, has, &markup, bottom);
        }
    }

    /// The panel of proposal `at`: which it is, why, and the keys.
    fn proposal_card(&self, at: usize) -> String {
        let proposal = &self.proposals[at];
        let title = format!("proposal {} of {}", at + 1, self.proposals.len());
        let (reason, note) = self.proposal_about(proposal.id);
        let mut lines = Vec::new();
        if let Some(reason) = reason {
            for (index, line) in wrap(reason, CARD_COLUMNS, CARD_LINES)
                .into_iter()
                .enumerate()
            {
                lines.push(if index == 0 {
                    format!("why: {line}")
                } else {
                    format!("     {line}")
                });
            }
        }
        let asked = note.and_then(|id| self.notes.all.iter().find(|note| note.id == id));
        if let Some(asked) = asked {
            let first = asked.text.lines().next().unwrap_or_default();
            lines.push(format!("answers: {}", cut(first, CARD_COLUMNS - 9)));
        }
        let hint = if self.notes.before == Some(proposal.id) {
            "as it is · zp as it would be · ga accept · gr reject · ]p next"
        } else {
            "as it would be · zp as it is · ga accept · gr reject · ]p next"
        };
        card_markup(&title, &lines, hint)
    }

    /// Draws `markup` as a panel in the bottom-left corner, its bottom at
    /// `bottom`. Returns where the next panel's bottom goes.
    fn corner_panel(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        markup: &str,
        bottom: f64,
    ) -> f64 {
        let scale = self.scale;
        let Some(image) = self.chrome.render(markup, scale) else {
            return bottom;
        };
        let (width, height) = (
            f64::from(image.pixmap.width()),
            f64::from(image.pixmap.height()),
        );
        let margin = PANEL_MARGIN * scale;
        let (x, y) = (margin.round(), (bottom - height).round());
        let line = scale.round().max(1.0);
        solid(
            scene,
            has,
            LENS_EDGE,
            (x - line, y - line, width + 2.0 * line, height + 2.0 * line),
        );
        scene.quads.push(Quad {
            texture: image.id,
            x: x as f32,
            y: y as f32,
            width: width as f32,
            height: height as f32,
        });
        if !has(image.id) {
            scene.images.push(image);
        }
        y - margin / 2.0
    }
}

/// The panel of a note: what it says and what was said to it.
fn note_card(note: &Note) -> String {
    let title = match (note.to_agent, note.by) {
        (true, _) => "question for agents",
        (false, Author::Agent) => "note from an agent",
        (false, Author::User) => "note",
    };
    let mut lines = wrap(&note.text, CARD_COLUMNS, CARD_LINES);
    for (by, text) in &note.answers {
        let who = match by {
            Author::User => "you",
            Author::Agent => "agent",
        };
        for (index, line) in wrap(text, CARD_COLUMNS - 7, CARD_LINES)
            .into_iter()
            .enumerate()
        {
            lines.push(if index == 0 {
                format!("{who}: {line}")
            } else {
                format!("       {line}")
            });
        }
    }
    let hint = if note.resolved {
        "resolved · :notes"
    } else {
        ":notes answer, resolve, delete · ]n next"
    };
    card_markup(title, &lines, hint)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use super::super::tests::{compile, editing, open, press, tints};
    use super::*;

    fn note(range: Range<usize>, quote: &str) -> Note {
        Note {
            id: 1,
            file: typst::syntax::RootedPath::new(
                typst::syntax::VirtualRoot::Project,
                typst::syntax::VirtualPath::new("main.typ").unwrap(),
            )
            .intern(),
            range,
            quote: quote.to_owned(),
            text: "line one\nline\ttwo \\ three".to_owned(),
            by: Author::Agent,
            to_agent: true,
            answers: vec![(Author::User, "yes\nno".to_owned())],
            resolved: true,
            detached: false,
            unseen: true,
        }
    }

    #[test]
    fn the_store_gives_back_what_it_was_given() {
        let kept = note(4..9, "quick");
        let file = kept.file;
        let text = stored(Path::new("/doc/main.typ"), &[&kept]);
        let back = parse_store(&text, file).unwrap();
        assert_eq!(back, vec![Note { id: 0, ..kept }]);
        assert!(parse_store("something else", file).is_err());
        // The name of a store is the same in every run.
        assert_eq!(
            store_path(Path::new("/state"), Path::new("/doc/main.typ")),
            store_path(Path::new("/state"), Path::new("/doc/main.typ"))
        );
    }

    #[test]
    fn a_note_keeps_to_its_text_as_the_text_around_it_changes() {
        let edit = |place, change, end| moved_place(place, change, end);
        // Typing before it, in it, right behind it.
        assert_eq!(
            (edit(4, (0, 0, 3), false), edit(9, (0, 0, 3), true)),
            (7, 12)
        );
        assert_eq!(
            (edit(4, (6, 0, 2), false), edit(9, (6, 0, 2), true)),
            (4, 11)
        );
        assert_eq!(
            (edit(4, (9, 0, 2), false), edit(9, (9, 0, 2), true)),
            (4, 9)
        );
        assert_eq!(
            (edit(4, (4, 0, 2), false), edit(9, (4, 0, 2), true)),
            (6, 11)
        );
        // Its first word replaced: the new one is part of it.
        assert_eq!(
            (edit(4, (2, 4, 1), false), edit(9, (2, 4, 1), true)),
            (2, 6)
        );
        // Found again where its text went, or detached.
        let mut moved = note(0..5, "quick");
        reanchor(&mut moved, "The quick fox, so quick.");
        assert_eq!((moved.range.clone(), moved.detached), (4..9, false));
        reanchor(&mut moved, "The slow fox.");
        assert!(moved.detached && moved.range.is_empty());
        assert_eq!(marker_length("== Results"), 3);
        assert_eq!(marker_length("- an item"), 2);
        assert_eq!(marker_length("Plain."), 0);
    }

    #[test]
    fn a_note_on_what_is_selected_moves_with_the_text() {
        let text = "The quick brown fox.\n\nThe lazy dog.\n";
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "wvegn");
        assert_eq!(
            viewer.editor.as_ref().unwrap().command_line(),
            Some("note ")
        );
        press(&mut viewer, "check this<CR>");
        assert_eq!(viewer.text(), Some(text), "a note is not in the text");
        let [note] = viewer.notes() else {
            panic!("one note: {:?}", viewer.notes())
        };
        assert_eq!(
            (&text[note.range.clone()], note.text.as_str()),
            ("quick", "check this")
        );
        assert!(!note.to_agent && note.by == Author::User);
        assert_ne!(viewer.editor.as_ref().unwrap().mode(), editor::Mode::Visual);
        compile(&mut viewer);
        assert!(
            !tints(&mut viewer, NOTE_LINE).is_empty(),
            "marked on the page"
        );

        // Typed before: it moves along; its word deleted, it is detached;
        // undone, it is back.
        press(&mut viewer, "0iVery <Esc>");
        let now = viewer.text().unwrap().to_owned();
        assert_eq!(&now[viewer.notes()[0].range.clone()], "quick");
        press(&mut viewer, "0wwdw");
        assert!(viewer.notes()[0].detached, "{:?}", viewer.notes());
        compile(&mut viewer);
        assert_eq!(tints(&mut viewer, NOTE_LINE).len(), 0);
        press(&mut viewer, "u");
        let now = viewer.text().unwrap().to_owned();
        assert_eq!(&now[viewer.notes()[0].range.clone()], "quick");
        assert!(!viewer.notes()[0].detached);

        // To it and away; resolved, it is shown no more.
        press(&mut viewer, "G]n");
        assert_eq!(
            viewer.editor.as_ref().unwrap().cursor(),
            now.find("quick").unwrap()
        );
        press(&mut viewer, ":notes resolve<CR>");
        assert!(viewer.notes()[0].resolved);
        compile(&mut viewer);
        assert_eq!(tints(&mut viewer, NOTE_LINE).len(), 0);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn notes_survive_a_restart() {
        let text = "= Results\n\nThe quick brown fox.\n";
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "G:note remember the fox<CR>");
        let range = viewer.notes()[0].range.clone();
        assert_eq!(
            &text[range.clone()],
            "The quick brown fox.",
            "the paragraph"
        );
        viewer.sync();
        drop(viewer);
        assert!(directory.join("state").join("notes").is_dir());
        let (mut viewer, _) = open(&directory);
        let listed = viewer.notes_listed(false);
        assert_eq!(listed.len(), 1);
        assert_eq!(
            (
                listed[0].line,
                listed[0].note.as_str(),
                listed[0].text.as_str()
            ),
            (3, "remember the fox", "The quick brown fox.")
        );
        // The file changed while it was closed: the note finds its text.
        drop(viewer);
        std::fs::write(directory.join("main.typ"), format!("Before.\n\n{text}")).unwrap();
        let (mut viewer, _) = open(&directory);
        assert_eq!(viewer.notes_listed(false)[0].line, 5);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_question_for_agents_is_answered_with_a_proposal() {
        let text = "The quick brown fox.\n\nThe lazy dog.\n";
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "Ggq");
        assert_eq!(viewer.editor.as_ref().unwrap().command_line(), Some("ask "));
        press(&mut viewer, "Is this sourced?<CR>");
        let listed = viewer.notes_listed(false);
        assert_eq!(listed.len(), 1);
        let id = listed[0].id;
        assert!(listed[0].to_agent);
        assert_eq!(
            (listed[0].line, listed[0].text.as_str()),
            (3, "The lazy dog.")
        );

        assert!(viewer.agent_answer_note(id + 9, "no", None, None).is_err());
        // Made for another text than there is: refused, and nothing said.
        let revision = viewer.state().revision;
        let late = Some(("lazy", "sleepy"));
        assert!(
            viewer
                .agent_answer_note(id, "late", late, Some(revision + 1))
                .is_err()
        );
        assert_eq!(viewer.notes()[0].answers, Vec::new());
        assert_eq!(viewer.proposals().len(), 0);
        let proposal = viewer
            .agent_answer_note(
                id,
                "Not yet; this says it plainly.",
                Some(("lazy", "sleepy")),
                Some(revision),
            )
            .unwrap()
            .unwrap();
        assert_eq!(viewer.text(), Some(text), "proposed, not made");
        assert_eq!(viewer.proposal_about(proposal).1, Some(id));
        assert_eq!(viewer.notes_listed(false)[0].proposals, vec![proposal]);
        assert!(viewer.notes_status().contains("1 answer"));
        // Seen, it is news no more.
        press(&mut viewer, &format!(":notes {id}<CR>"));
        assert!(viewer.is_picking());
        press(&mut viewer, "<Esc><Esc>");
        assert_eq!(viewer.notes_status(), "");

        // An agent's own note, and one it resolves.
        let (other, line) = viewer
            .agent_add_note("Cite this.", NoteAnchor::Text("quick"), Some(revision))
            .unwrap();
        assert_eq!(line, 1);
        let stale = NoteAnchor::Text("quick");
        assert!(
            viewer
                .agent_add_note("x", stale, Some(revision + 1))
                .is_err()
        );
        assert!(
            viewer
                .agent_add_note("x", NoteAnchor::Text("The"), None)
                .is_err()
        );
        assert!(
            viewer
                .agent_add_note("x", NoteAnchor::Line(2), None)
                .is_err()
        );
        assert!(
            viewer
                .agent_add_note("x", NoteAnchor::Line(9), None)
                .is_err()
        );
        viewer.agent_resolve_note(other).unwrap();
        assert_eq!(viewer.notes_listed(false).len(), 1);
        assert_eq!(viewer.notes_listed(true).len(), 2);
        press(&mut viewer, &format!(":notes delete {other}<CR>"));
        assert_eq!(viewer.notes().len(), 1);
        let _ = std::fs::remove_dir_all(directory);
    }

    /// How many times the paper colour is laid over the pages in the
    /// frame drawn now.
    fn papers_now(viewer: &mut Viewer) -> usize {
        let id = TextureId(typst::utils::hash128(&("solid", PAPER)));
        let scene = viewer.scene(&|_| false);
        scene.quads.iter().filter(|quad| quad.texture == id).count()
    }

    /// The same, once what is typeset off the window's thread is back.
    fn papers(viewer: &mut Viewer) -> usize {
        papers_now(viewer);
        viewer.preview_settled();
        papers_now(viewer)
    }

    #[test]
    fn a_proposal_is_shown_as_it_would_be_and_as_it_is() {
        let text = "#set heading(numbering: \"1.\")\n= Animals <animals>\n\n\
                    The quick brown fox jumps over the lazy dog.\n\nSee @animals too.\n";
        let (mut viewer, directory) = editing(text);
        viewer
            .agent_replace("lazy", "sleepy", false, None, true)
            .unwrap();
        let first = viewer.next_proposal_id();
        viewer
            .agent_replace("too", "as well", false, None, true)
            .unwrap();
        viewer.explain_proposals(first, "plainer");
        compile(&mut viewer);
        let pages = papers(&mut viewer);
        press(&mut viewer, "]p");
        // The window does not wait for it to be typeset.
        assert_eq!(papers_now(&mut viewer), pages, "not there at once");
        assert_eq!(
            papers(&mut viewer),
            pages + 1,
            "the paragraph as it would be"
        );
        press(&mut viewer, "zp");
        assert_eq!(papers(&mut viewer), pages, "as it is");
        press(&mut viewer, "zp");
        assert_eq!(papers(&mut viewer), pages + 1);
        // A reference, to what the paragraph alone does not have, does
        // not keep it from being shown.
        press(&mut viewer, "]p");
        assert_eq!(
            viewer.proposal_about(viewer.proposals()[1].id).0,
            Some("plainer")
        );
        assert_eq!(papers(&mut viewer), pages + 1);
        press(&mut viewer, "ga");
        assert_eq!(papers(&mut viewer), pages);
        let _ = std::fs::remove_dir_all(directory);
    }
}
