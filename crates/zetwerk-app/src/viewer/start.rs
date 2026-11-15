//! Beginning: what a new document starts from, and the window before
//! there is a document.
//!
//! `:new` (or `:template`) lists the starters: an article, a report or
//! thesis, a letter, slides, notes, or nothing at all, and the user's
//! own, kept as `.typ` files in the `templates` directory beside the
//! settings. Choosing one fills an empty document with it, as one undo
//! step; a document with text in it is never filled. The places to fill
//! in are written between `‹` and `›`: so they show on the page as what
//! they are, and the document stays plain Typst. The cursor is put on
//! the first of them, what is typed there takes its place, and Tab leads
//! to the next.
//!
//! `zetwerk` without a file (or with a directory) opens a window with no
//! document in it, and the list of the documents opened last, with "new
//! document…" behind them; `:recent` puts up the same list at any time.
//! Choosing a document asks the window to open it in place of the one it
//! has (see `app::switch`). Which documents were opened, and when, is
//! kept in the state directory, newest first.

use std::fmt::Write as _;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::assist::Target;
use super::{Edited, Viewer};
use crate::editor::{self, Editor};
use crate::keys::{Key, KeyPress};

/// Where a place to fill in begins and ends.
const OPEN: char = '‹';
const CLOSE: char = '›';
/// Longer than this, in bytes, it is no place to fill in but text in
/// single guillemets.
const BLANK_LONGEST: usize = 240;

/// The file in the state directory that lists the documents opened last.
const RECENT_FILE: &str = "recent";
/// How many documents it lists.
const RECENT_KEPT: usize = 30;

/// What the status line says where the window has no document.
const NO_DOCUMENT: &str = "no document is open: :recent lists the last ones, \
                           :new <name> begins one, :q leaves";
/// What it says where the document is empty.
const EMPTY_DOCUMENT: &str = "a new document: :new for a starter (article, report, letter, \
                              slides, notes), i to write, g? for all there is to do";
/// Why a starter is not put in.
const NOT_EMPTY: &str = "there is text in this document: a starter goes only into an empty \
                         one (:new <name> makes one)";

/// Something to begin a document with.
pub struct Starter {
    /// What it is listed as and chosen by.
    pub name: &'static str,
    /// Other names it is chosen by.
    pub also: &'static [&'static str],
    /// What it holds, as listed beside it.
    pub what: &'static str,
    /// The text of the document.
    pub text: &'static str,
    /// Files it needs beside the document: their names and what is in
    /// them. Where one is there already, it is left as it is.
    pub beside: &'static [(&'static str, &'static str)],
}

/// The starters that come with zetwerk.
pub const STARTERS: &[Starter] = &[
    Starter {
        name: "blank",
        also: &["empty", "nothing"],
        what: "an empty page",
        text: "",
        beside: &[],
    },
    Starter {
        name: "article",
        also: &["paper"],
        what: "title, authors, abstract, numbered sections",
        text: include_str!("../../templates/article.typ"),
        beside: &[],
    },
    Starter {
        name: "report",
        also: &["thesis"],
        what: "title page, contents, chapters, bibliography",
        text: include_str!("../../templates/report.typ"),
        beside: &[("refs.bib", include_str!("../../templates/refs.bib"))],
    },
    Starter {
        name: "letter",
        also: &[],
        what: "sender, addressee, date, subject",
        text: include_str!("../../templates/letter.typ"),
        beside: &[],
    },
    Starter {
        name: "slides",
        also: &["presentation", "talk"],
        what: "a page for each slide, 16:9",
        text: include_str!("../../templates/slides.typ"),
        beside: &[],
    },
    Starter {
        name: "notes",
        also: &[],
        what: "topic, date, headings and points",
        text: include_str!("../../templates/notes.typ"),
        beside: &[],
    },
];

/// The starter called `name`, or by one of its other names.
fn starter(name: &str) -> Option<&'static Starter> {
    let name = name.to_lowercase();
    STARTERS
        .iter()
        .find(|starter| starter.name == name || starter.also.contains(&name.as_str()))
}

/// How the window came to show what it shows, for what it says first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opening {
    /// With no document: the list of the last ones is put up.
    Start,
    /// With a document that was asked for.
    Document,
    /// With a document just made: the starters are put up.
    New,
}

/// What the viewer keeps about beginnings.
#[derive(Default)]
pub(super) struct Start {
    /// Where the user's own starters are.
    templates: Option<PathBuf>,
    /// The document the user chose to go to, until the window takes it,
    /// and whether it was just made.
    wanted: Option<(PathBuf, bool)>,
    /// Whether Tab while typing leads to the next place to fill in: from
    /// when a starter was put in or Tab was pressed in normal mode, until
    /// typing ends.
    filling: bool,
    /// The place to fill in the cursor was put on: what is typed there
    /// takes its place.
    blank: Option<Range<usize>>,
    /// Whether the window has no document: no page is shown then.
    nothing: bool,
}

/// The places to fill in of `text`, in order: what stands between `‹`
/// and `›` on one line.
fn blanks(text: &str) -> impl Iterator<Item = Range<usize>> + '_ {
    text.match_indices(OPEN).filter_map(move |(start, _)| {
        let inside = start + OPEN.len_utf8();
        let rest = &text[inside..];
        let length = rest.find([CLOSE, OPEN, '\n'])?;
        let closed = rest[length..].starts_with(CLOSE);
        (closed && length > 0 && length <= BLANK_LONGEST)
            .then(|| start..inside + length + CLOSE.len_utf8())
    })
}

/// The first place to fill in of `text` that begins at or behind byte
/// `from`; else the first of all.
fn next_blank(text: &str, from: usize) -> Option<Range<usize>> {
    blanks(text)
        .find(|blank| blank.start >= from)
        .or_else(|| blanks(text).next())
}

/// Whether `range` of `text` is a place to fill in.
fn is_blank(text: &str, range: &Range<usize>) -> bool {
    blanks(text.get(range.clone()).unwrap_or("")).next() == Some(0..range.len())
}

/// Whether `a` and `b` name the same file.
fn same_file(a: &Path, b: &Path) -> bool {
    a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

/// How many files of a folder are looked at for the documents in it.
const FOLDER_SEEN: usize = 400;
/// Files longer than this are not read for what they include.
const READ_LONGEST: u64 = 1 << 20;

/// The Typst files in `folder` (not below it), the documents first and
/// then the files they include or import, each with the name of one
/// that includes it; by name within each.
fn documents_in(folder: &Path) -> Vec<(PathBuf, Option<String>)> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .take(FOLDER_SEEN)
        .map(|entry| entry.path().canonicalize().unwrap_or_else(|_| entry.path()))
        .filter(|path| path.extension().is_some_and(|ending| ending == "typ"))
        .filter(|path| path.is_file())
        .collect();
    files.sort();
    // Which file names which, as `include "…"` or `import "…"`.
    let mut part_of: Vec<Option<String>> = vec![None; files.len()];
    for including in &files {
        let small = std::fs::metadata(including).is_ok_and(|meta| meta.len() <= READ_LONGEST);
        let Some(text) = small
            .then(|| std::fs::read_to_string(including).ok())
            .flatten()
        else {
            continue;
        };
        let name = including.file_name().unwrap_or_default().to_string_lossy();
        for word in ["include \"", "import \""] {
            for (at, _) in text.match_indices(word) {
                let rest = &text[at + word.len()..];
                let Some(named) = rest.split('"').next().filter(|named| !named.contains('\n'))
                else {
                    continue;
                };
                let path = including.parent().unwrap_or(folder).join(named);
                for (file, part) in files.iter().zip(part_of.iter_mut()) {
                    if file != including && part.is_none() && same_file(file, &path) {
                        *part = Some(name.clone().into_owned());
                    }
                }
            }
        }
    }
    let mut found: Vec<(PathBuf, Option<String>)> = files.into_iter().zip(part_of).collect();
    found.sort_by_key(|(_, part)| part.is_some());
    found
}

/// How many characters of a folder are shown: the list cuts what is
/// longer at its end, where the folder's own name is.
const SHOWN: usize = 36;

/// `path` as it is shown: from the home directory, where it is in it;
/// where that is long, its end.
fn shown(path: &Path) -> String {
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty());
    let whole = match home.and_then(|home| path.strip_prefix(home).ok().map(Path::to_owned)) {
        Some(within) if within.as_os_str().is_empty() => "~".to_owned(),
        Some(within) => format!("~/{}", within.display()),
        None => path.display().to_string(),
    };
    let count = whole.chars().count();
    if count <= SHOWN {
        return whole;
    }
    let end: String = whole.chars().skip(count + 1 - SHOWN).collect();
    format!("…{end}")
}

/// How long ago `then` was, as it is said.
pub(super) fn ago(then: SystemTime, now: SystemTime) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    let seconds = now.duration_since(then).unwrap_or_default().as_secs();
    let (count, unit) = if seconds < MINUTE {
        return "just now".to_owned();
    } else if seconds < HOUR {
        (seconds / MINUTE, "minute")
    } else if seconds < DAY {
        (seconds / HOUR, "hour")
    } else if seconds < 2 * DAY {
        return "yesterday".to_owned();
    } else if seconds < 14 * DAY {
        (seconds / DAY, "day")
    } else if seconds < 60 * DAY {
        (seconds / (7 * DAY), "week")
    } else if seconds < 730 * DAY {
        (seconds / (30 * DAY), "month")
    } else {
        (seconds / (365 * DAY), "year")
    };
    match (count, unit) {
        (1, "hour") => "an hour ago".to_owned(),
        (1, _) => format!("a {unit} ago"),
        _ => format!("{count} {unit}s ago"),
    }
}

/// The documents listed in `state_dir` as opened last, newest first,
/// with when they were opened, in seconds since 1970; there or not.
fn read_recent(state_dir: &Path) -> Vec<(PathBuf, u64)> {
    let Ok(text) = std::fs::read_to_string(state_dir.join(RECENT_FILE)) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let (when, path) = line.split_once('\t')?;
            let path = PathBuf::from(path);
            path.is_absolute().then_some((path, when.parse().ok()?))
        })
        .take(RECENT_KEPT)
        .collect()
}

/// The documents opened last that are still there, newest first, each
/// with when it was opened.
#[must_use]
pub fn recent(state_dir: &Path) -> Vec<(PathBuf, SystemTime)> {
    read_recent(state_dir)
        .into_iter()
        .filter(|(path, _)| path.is_file())
        .map(|(path, when)| (path, UNIX_EPOCH + Duration::from_secs(when)))
        .collect()
}

/// Notes in `state_dir` that the document at `path` was opened now:
/// first in the list, once, and the list no longer than it is kept.
///
/// # Errors
///
/// If the list cannot be written. A path that cannot be written as a
/// line of text is not noted, and that is no error.
pub fn remember(state_dir: &Path, path: &Path) -> std::io::Result<()> {
    let path = path.canonicalize().or_else(|_| std::path::absolute(path))?;
    let Some(name) = path.to_str().filter(|name| !name.contains('\n')) else {
        return Ok(());
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut text = format!("{now}\t{name}\n");
    let others = read_recent(state_dir)
        .into_iter()
        .filter(|(other, _)| *other != path)
        .take(RECENT_KEPT - 1);
    for (other, when) in others {
        let _ = writeln!(text, "{when}\t{}", other.display());
    }
    std::fs::create_dir_all(state_dir)?;
    // Another zetwerk reading it finds the old list or the new one, crash
    // or not.
    zetwerk_core::write_atomically(&state_dir.join(RECENT_FILE), text.as_bytes())
}

impl Viewer {
    /// What the window says or puts up first, now that it shows what it
    /// was opened with; `templates` is where the user's own starters are.
    pub fn welcome(&mut self, opening: Opening, templates: Option<PathBuf>) {
        self.start.templates = templates;
        match opening {
            Opening::Start => {
                "no document".clone_into(&mut self.file);
                self.start.nothing = true;
                self.open_start_list();
            }
            Opening::New => self.open_starters(),
            Opening::Document => {
                if self.text().is_some_and(|text| text.trim().is_empty()) {
                    self.notice(EMPTY_DOCUMENT);
                }
            }
        }
    }

    /// Whether the window has no document: the empty page of nothing is
    /// not shown behind the list of documents.
    pub(super) fn shows_no_document(&self) -> bool {
        self.start.nothing && self.editor.is_none()
    }

    /// The document the user chose to open in place of this one, and
    /// whether it was just made; once.
    pub fn take_wanted_document(&mut self) -> Option<(PathBuf, bool)> {
        self.start.wanted.take()
    }

    /// Says that the document at `path` could not be opened, and why.
    pub fn not_opened(&mut self, path: &Path, why: &str) {
        self.alert(format!("{} not opened: {why}", path.display()));
    }

    /// A key that is for beginnings: for the list, where there is no
    /// document; Tab, to the next place to fill in; and what is typed on
    /// one, which takes its place. Returns whether the key was taken.
    pub(super) fn start_key(&mut self, press: KeyPress) -> bool {
        if self.editor.is_none() {
            if self.picker.is_none() {
                return false;
            }
            self.assist_key(press);
            if self.picker.is_none() && self.start.wanted.is_none() && press.key == Key::Escape {
                self.notice(NO_DOCUMENT);
            }
            return true;
        }
        let blank = self.start.blank.take();
        let Some(editor) = &self.editor else {
            return false;
        };
        if self.picker.is_some() || self.completion.is_some() {
            return false;
        }
        let (mode, cursor, text) = (editor.mode(), editor.cursor(), editor.text());
        let typing = mode == editor::Mode::Insert;
        let blank = blank.filter(|blank| typing && cursor == blank.start && is_blank(text, blank));
        let plain = !press.ctrl && !press.alt;
        match press.key {
            Key::Tab if plain && mode == editor::Mode::Normal => {
                let next = text[cursor..].chars().next().map_or(0, char::len_utf8);
                if blanks(text).next().is_none() {
                    return false;
                }
                self.start.filling = true;
                self.go_to_blank(cursor + next)
            }
            Key::Tab if plain && typing && self.start.filling => {
                // A snippet's name becomes the snippet, as always; and the
                // rest of a word shown behind the cursor is what Tab
                // takes (see `Viewer::plain_key`).
                let snippet = crate::typst_text::snippet(editor.source(), cursor).is_some();
                if snippet || self.shows_word_rest() {
                    return false;
                }
                let from = blank.map_or(cursor, |blank| blank.end);
                if self.go_to_blank(from) {
                    return true;
                }
                self.start.filling = false;
                false
            }
            Key::Char(_) if plain => {
                if let Some(blank) = blank {
                    self.clear_blank(blank, Edited::Typing);
                }
                false
            }
            Key::Backspace | Key::Delete if blank.is_some() => {
                if let Some(blank) = blank {
                    self.clear_blank(blank, Edited::Command);
                }
                true
            }
            Key::Escape => {
                self.start.filling = false;
                false
            }
            _ => false,
        }
    }

    /// Takes the place to fill in `blank` out of the text.
    fn clear_blank(&mut self, blank: Range<usize>, edited: Edited) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        match editor.replace(&[(blank, "")]) {
            Ok(()) => self.text_changed(edited),
            Err(why) => self.alert(format!("not changed: {why}")),
        }
    }

    /// Puts the cursor on the first place to fill in at or behind byte
    /// `from` (or the first of all), typing, so that what is typed takes
    /// its place. Returns whether there is one.
    fn go_to_blank(&mut self, from: usize) -> bool {
        let Some(editor) = &self.editor else {
            return false;
        };
        let Some(blank) = next_blank(editor.text(), from) else {
            return false;
        };
        let what = editor.text()[blank.clone()].to_owned();
        if editor.mode() != editor::Mode::Normal {
            self.editor_key(KeyPress::plain(Key::Escape));
        }
        if let Some(editor) = &mut self.editor {
            editor.jump(blank.start);
        }
        self.editor_key(KeyPress::plain(Key::Char('i')));
        let at = self.editor.as_ref().map(Editor::cursor);
        if at == Some(blank.start) {
            self.start.blank = Some(blank);
            self.notice(format!(
                "what is typed takes the place of {what}; Tab: the next place to fill in"
            ));
        }
        self.follow_cursor();
        true
    }

    /// The commands about beginnings. Returns whether `command` was one.
    pub(super) fn start_command(&mut self, command: &str) -> bool {
        let (name, argument) = match command.split_once(' ') {
            Some((name, argument)) => (name, argument.trim()),
            None => (command, ""),
        };
        match (name, argument) {
            ("recent" | "start", "") => self.open_start_list(),
            ("recent", "new") => self.ask_for_name(),
            ("new" | "template" | "templates", "") => {
                let empty = self.text().is_some_and(|text| text.trim().is_empty());
                if empty {
                    self.open_starters();
                } else if name == "new" {
                    self.ask_for_name();
                } else if self.editor.is_some() {
                    self.alert(NOT_EMPTY);
                } else {
                    self.alert(NO_DOCUMENT);
                }
            }
            ("new", name) => self.new_document(name),
            ("template", which) => self.use_starter(which),
            ("open", path) => self.go_to_document(&self.folder().join(path)),
            // Without a document, the usual way to open a file opens one.
            ("e" | "edit", path) if self.editor.is_none() && !path.is_empty() => {
                self.go_to_document(&self.folder().join(path));
            }
            _ => return false,
        }
        true
    }

    /// Where new documents go: beside the one being edited, or where the
    /// window was opened.
    fn folder(&self) -> PathBuf {
        let beside = self.editor.as_ref().and_then(|editor| {
            let path = std::path::absolute(editor.path()).ok()?;
            path.parent().map(Path::to_owned)
        });
        beside.unwrap_or_else(|| self.project.root_dir().to_owned())
    }

    /// The documents of the folder (those that include others first),
    /// the documents opened last elsewhere, and "new document…" behind
    /// them.
    fn open_start_list(&mut self) {
        let now = SystemTime::now();
        let current = self.editor.as_ref().map(|editor| editor.path().to_owned());
        let listed = self.state_dir.as_deref().map(recent).unwrap_or_default();
        let not_shown = |path: &Path| !current.as_deref().is_some_and(|now| same_file(now, path));
        let mut items: Vec<(String, Target)> = Vec::new();
        let here = documents_in(&self.folder());
        for (path, part_of) in &here {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            let opened = listed.iter().find(|(other, _)| same_file(other, path));
            let what = match (part_of, opened) {
                (Some(main), _) => format!("part of {main}"),
                (None, Some((_, when))) => ago(*when, now),
                (None, None) => "here".to_owned(),
            };
            if not_shown(path) {
                let open = Target::Command(format!("open {}", path.display()));
                items.push((format!("{name}\t{what}"), open));
            }
        }
        let elsewhere = listed
            .into_iter()
            .filter(|(path, _)| not_shown(path))
            .filter(|(path, _)| !here.iter().any(|(other, _)| same_file(other, path)));
        items.extend(elsewhere.map(|(path, when)| {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            let within = path.parent().map(shown).unwrap_or_default();
            let label = format!("{name}   {within}\t{}", ago(when, now));
            (label, Target::Command(format!("open {}", path.display())))
        }));
        items.push((
            format!("new document…\tin {}", shown(&self.folder())),
            Target::Command("recent new".to_owned()),
        ));
        self.open_picker("open", items, 0);
    }

    /// Asks for the name of a new document; Escape leads back to the
    /// documents opened last.
    fn ask_for_name(&mut self) {
        let items = vec![(
            format!("type its name, then Enter\tin {}", shown(&self.folder())),
            Target::Nothing,
        )];
        let (back, free) = ("recent".to_owned(), Some("new ".to_owned()));
        self.open_picker_for("new document", items, 0, back, free);
    }

    /// The starters there are: those that come with zetwerk, then the
    /// user's own.
    fn open_starters(&mut self) {
        let mut items: Vec<(String, Target)> = STARTERS
            .iter()
            .map(|starter| {
                let label = format!("{}\t{}", starter.name, starter.what);
                (label, Target::Command(format!("template {}", starter.name)))
            })
            .collect();
        for path in self.own_starters() {
            let name = path.file_stem().unwrap_or_default().to_string_lossy();
            let label = format!("{name}\tyour own");
            items.push((
                label,
                Target::Command(format!("template {}", path.display())),
            ));
        }
        self.open_picker("begin with", items, 0);
    }

    /// The user's own starters: the Typst files in their directory.
    fn own_starters(&self) -> Vec<PathBuf> {
        let Some(directory) = &self.start.templates else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir(directory) else {
            return Vec::new();
        };
        let mut found: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ending| ending == "typ"))
            .filter(|path| path.is_file())
            .collect();
        found.sort();
        found
    }

    /// `:template <name>`: fills the empty document with the starter of
    /// that name, or with the user's own of that name or path.
    fn use_starter(&mut self, which: &str) {
        let own = match (&self.start.templates, which.contains('/')) {
            (_, true) => Some(PathBuf::from(which)),
            (Some(directory), false) => {
                let path = directory.join(which).with_extension("typ");
                path.is_file().then_some(path)
            }
            (None, false) => None,
        };
        let filled = match (starter(which), own) {
            (Some(starter), _) => self.fill(starter.text, starter.beside),
            (None, Some(path)) => match std::fs::read_to_string(&path) {
                Ok(text) => self.fill(&text, &[]),
                Err(err) => Err(format!("cannot read {}: {err}", path.display())),
            },
            (None, None) => Err(format!("there is no starter {which}: :new lists them")),
        };
        if let Err(why) = filled {
            self.alert(why);
        }
    }

    /// Fills the empty document with `text`, as one step that undo takes
    /// back, and makes the files of `beside` that are not there yet. The
    /// cursor goes to the first place to fill in.
    ///
    /// # Errors
    ///
    /// If no document is open, there is text in it, or a file cannot be
    /// made.
    fn fill(&mut self, text: &str, beside: &[(&str, &str)]) -> Result<(), String> {
        let folder = self.folder();
        let editor = self.editor.as_ref().ok_or(NO_DOCUMENT)?;
        if !editor.text().trim().is_empty() {
            return Err(NOT_EMPTY.to_owned());
        }
        if text.is_empty() {
            self.notice("a blank page: i to write, # for what can be put in");
            return Ok(());
        }
        let mut made = Vec::new();
        for (name, contents) in beside {
            let path = folder.join(name);
            let file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path);
            match file {
                Ok(mut file) => {
                    std::io::Write::write_all(&mut file, contents.as_bytes())
                        .map_err(|err| format!("cannot write {}: {err}", path.display()))?;
                    made.push(path);
                }
                // The user's own: it stays as it is.
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(err) => return Err(format!("cannot make {}: {err}", path.display())),
            }
        }
        let editor = self.editor.as_mut().ok_or(NO_DOCUMENT)?;
        let end = editor.text().len();
        editor
            .replace(&[(0..end, text)])
            .map_err(|why| format!("not changed: {why}"))?;
        self.text_changed(Edited::Command);
        for path in &made {
            self.file_changed(path);
        }
        self.start.filling = true;
        if !self.go_to_blank(0) {
            self.notice("begun: u takes it back");
        }
        Ok(())
    }

    /// `:new <name>`: makes a new, empty file of that name beside the
    /// document (with `.typ` where it has no ending) and has the window
    /// open it, with the starters up. A file that is there is not
    /// touched.
    fn new_document(&mut self, name: &str) {
        if !self.may_leave() {
            return;
        }
        let mut path = self.folder().join(name);
        if path.extension().is_none() {
            path.set_extension("typ");
        }
        let made = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path);
        match made {
            Ok(_) => self.start.wanted = Some((path, true)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => self.alert(format!(
                "there is a {} already: :open {name} opens it",
                path.display()
            )),
            Err(err) => self.alert(format!("cannot make {}: {err}", path.display())),
        }
    }

    /// Goes to the document at `path`: in this window, where it is open
    /// here, else in place of this one.
    fn go_to_document(&mut self, path: &Path) {
        let editors = || self.editor.iter().chain(&self.others);
        if editors().any(|editor| same_file(editor.path(), path)) {
            return self.open_path(path, false);
        }
        if !path.is_file() {
            self.alert(format!("there is no file {}", path.display()));
            return;
        }
        if self.may_leave() {
            self.start.wanted = Some((path.to_owned(), false));
        }
    }

    /// Whether the window may go to another document: whether all that
    /// is open is written. Says why not.
    fn may_leave(&mut self) -> bool {
        let unwritten = self.unwritten();
        if unwritten.is_empty() {
            return true;
        }
        self.alert(format!(
            "unsaved changes in {}: :wa writes them, then the other document can be opened",
            unwritten.join(", ")
        ));
        false
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use std::sync::Arc;

    use zetwerk_typst::{Environment, FontSet, Project};

    use super::*;
    use crate::viewer::tests::{compile, directory, editing, press};

    /// A fresh directory for a test of its own, named for it.
    fn scratch(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("zetwerk-start-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn every_starter_compiles_to_at_least_a_page() {
        let env = Environment::new(FontSet::Embedded);
        for starter in STARTERS {
            let directory = scratch(starter.name);
            for (name, contents) in starter.beside {
                std::fs::write(directory.join(name), contents).unwrap();
            }
            let project = Project::in_memory(
                Arc::clone(&env),
                directory,
                "main.typ",
                starter.text.to_owned(),
            )
            .unwrap();
            let world = project.snapshot();
            let compiled = zetwerk_typst::compile(&world);
            let describe = |diagnostic| zetwerk_typst::describe(&world, diagnostic);
            let errors: Vec<String> = compiled.errors.iter().map(describe).collect();
            let warnings: Vec<String> = compiled.warnings.iter().map(describe).collect();
            assert!(errors.is_empty(), "{}: {errors:?}", starter.name);
            assert!(warnings.is_empty(), "{}: {warnings:?}", starter.name);
            let pages = compiled
                .document
                .map_or(0, |document| document.pages().len());
            assert!(pages >= 1, "{} has no pages", starter.name);
            // Each but the blank one has places to fill in.
            assert_eq!(
                blanks(starter.text).next().is_some(),
                !starter.text.is_empty(),
                "{}",
                starter.name
            );
        }
        assert!(starter("thesis").is_some_and(|found| found.name == "report"));
        assert!(starter("Paper").is_some_and(|found| found.name == "article"));
        assert!(starter("novel").is_none());
    }

    #[test]
    fn places_to_fill_in_are_found_between_guillemets_on_a_line() {
        let text = "= ‹Title›\n‹a\nb› and ‹› and ‹x ‹y› z›";
        let found: Vec<&str> = blanks(text).map(|blank| &text[blank]).collect();
        assert_eq!(found, ["‹Title›", "‹y›"]);
        let title = 2..2 + "‹Title›".len();
        assert!(is_blank(text, &title));
        assert!(!is_blank(text, &(2..5)));
        assert_eq!(next_blank(text, 0), Some(title.clone()));
        // Behind the last, the first again.
        assert_eq!(next_blank(text, text.len()), Some(title));
        assert_eq!(next_blank("nothing", 0), None);
    }

    #[test]
    fn a_starter_fills_an_empty_document_as_one_step() {
        let (mut viewer, directory) = editing("\n");
        viewer.execute("template article");
        let text = viewer.text().unwrap().to_owned();
        assert_eq!(text, starter("article").unwrap().text);
        // Typing at the first place to fill in, the title, replaces it.
        let editor = viewer.editor.as_ref().unwrap();
        assert_eq!(editor.mode(), editor::Mode::Insert);
        assert_eq!(editor.cursor(), text.find("‹Title›").unwrap());
        compile(&mut viewer);
        press(&mut viewer, "On Averages");
        let text = viewer.text().unwrap().to_owned();
        assert!(text.contains("[On Averages]"), "{text}");
        assert!(!text.contains("‹Title›"));
        // Tab leads to the next place, the first author; Backspace takes
        // it out, typing fills it.
        press(&mut viewer, "<Tab>");
        let cursor = viewer.editor.as_ref().unwrap().cursor();
        assert_eq!(cursor, text.find("‹Author›").unwrap());
        press(&mut viewer, "<BS>A. Writer<Esc>");
        let text = viewer.text().unwrap().to_owned();
        assert!(text.contains("[A. Writer \\"), "{text}");
        // Undo takes it all back, step by step, to the empty document:
        // the author, the place for it, the title, the place for it, the
        // starter.
        press(&mut viewer, "uuuu");
        assert!(viewer.text().unwrap().contains("[‹Title›]"));
        press(&mut viewer, "u");
        assert_eq!(viewer.text(), Some("\n"));
        // A file the starter wants beside it is made, where it is not
        // there; one that is there is left alone.
        std::fs::write(directory.join("refs.bib"), "% mine\n").unwrap();
        viewer.execute("template thesis");
        assert!(
            viewer
                .text()
                .unwrap()
                .contains("#bibliography(\"refs.bib\")")
        );
        let bib = std::fs::read_to_string(directory.join("refs.bib")).unwrap();
        assert_eq!(bib, "% mine\n");
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_starter_does_not_go_into_a_document_with_text() {
        let (mut viewer, directory) = editing("Mine.\n");
        viewer.execute("template letter");
        assert_eq!(viewer.text(), Some("Mine.\n"));
        assert!(viewer.message.as_ref().is_some_and(|message| message.alert));
        // `:new` asks for the name of a new document instead.
        viewer.execute("new");
        assert!(viewer.picker.is_some());
        press(&mut viewer, "<Esc><Esc>");
        // `:template` alone says why there is nothing to choose.
        viewer.execute("template");
        assert!(viewer.picker.is_none());
        assert_eq!(viewer.text(), Some("Mine.\n"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_users_own_starters_are_listed_and_used() {
        let (mut viewer, directory) = editing("");
        let own = directory.join("templates");
        std::fs::create_dir_all(&own).unwrap();
        std::fs::write(own.join("memo.typ"), "= Memo\n\n‹What›\n").unwrap();
        std::fs::write(own.join("readme.txt"), "not one").unwrap();
        viewer.welcome(Opening::New, Some(own));
        let picker = viewer.picker.as_ref().unwrap();
        assert_eq!(
            picker.chosen(),
            Some(&Target::Command("template blank".into()))
        );
        // Narrowed to it by typing, and chosen.
        press(&mut viewer, "memo<CR>");
        assert_eq!(viewer.text(), Some("= Memo\n\n‹What›\n"));
        press(&mut viewer, "Why<Esc>");
        assert_eq!(viewer.text(), Some("= Memo\n\nWhy\n"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn tab_in_normal_mode_goes_to_the_places_left_to_fill_in() {
        let (mut viewer, directory) = editing("‹One› and ‹Two›\n");
        compile(&mut viewer);
        let cursor = |viewer: &Viewer| viewer.editor.as_ref().unwrap().cursor();
        let two = "‹One› and ".len();
        // From the first place, the cursor being on it, to the next.
        press(&mut viewer, "<Tab>");
        assert_eq!(cursor(&viewer), two);
        // Behind the last, round again to the first.
        press(&mut viewer, "<Tab>");
        assert_eq!(cursor(&viewer), 0);
        press(&mut viewer, "1<Esc>");
        assert_eq!(viewer.text(), Some("1 and ‹Two›\n"));
        press(&mut viewer, "<Tab>2<Esc>");
        assert_eq!(viewer.text(), Some("1 and 2\n"));
        press(&mut viewer, "<Tab>");
        assert_eq!(viewer.editor.as_ref().unwrap().mode(), editor::Mode::Normal);
        // Without places to fill in, Tab is what it always was.
        let (mut plain, other) = editing("Plain.\n");
        press(&mut plain, "<Tab>");
        assert_eq!(plain.editor.as_ref().unwrap().mode(), editor::Mode::Normal);
        let _ = std::fs::remove_dir_all(directory);
        let _ = std::fs::remove_dir_all(other);
    }

    #[test]
    fn documents_opened_are_remembered_newest_first_and_capped() {
        let state = scratch("recent");
        let documents = scratch("recent-documents");
        let paths: Vec<PathBuf> = (0..RECENT_KEPT + 3)
            .map(|index| {
                let path = documents.join(format!("{index}.typ"));
                std::fs::write(&path, "").unwrap();
                path
            })
            .collect();
        assert_eq!(recent(&state), []);
        for path in &paths {
            remember(&state, path).unwrap();
        }
        // Again: it comes first and is there once.
        remember(&state, &paths[5]).unwrap();
        let listed: Vec<PathBuf> = recent(&state).into_iter().map(|(path, _)| path).collect();
        assert_eq!(listed.len(), RECENT_KEPT);
        let canonical = |index: usize| paths[index].canonicalize().unwrap();
        assert_eq!(listed[0], canonical(5));
        assert_eq!(listed[1], canonical(RECENT_KEPT + 2));
        assert_eq!(
            listed.iter().filter(|path| **path == canonical(5)).count(),
            1
        );
        assert!(!listed.contains(&canonical(0)));
        // A document that is gone is not listed, and the rest still are.
        std::fs::remove_file(&paths[RECENT_KEPT + 2]).unwrap();
        let listed: Vec<PathBuf> = recent(&state).into_iter().map(|(path, _)| path).collect();
        assert_eq!(listed.len(), RECENT_KEPT - 1);
        assert_eq!(listed[0], canonical(5));
        // A list that is not one is no list.
        std::fs::write(state.join(RECENT_FILE), "garbage\n12\trelative.typ\n").unwrap();
        assert_eq!(recent(&state), []);
        remember(&state, &paths[1]).unwrap();
        assert_eq!(recent(&state).len(), 1);
        let _ = std::fs::remove_dir_all(state);
        let _ = std::fs::remove_dir_all(documents);
    }

    #[test]
    fn a_long_folder_is_shown_by_its_end() {
        let long = Path::new("/srv/projects/university/2026/thesis/chapters/methods");
        let said = shown(long);
        assert!(said.ends_with("chapters/methods"), "{said}");
        assert!(
            said.starts_with('…') && said.chars().count() == SHOWN,
            "{said}"
        );
        assert_eq!(shown(Path::new("/srv/paper")), "/srv/paper");
    }

    #[test]
    fn ages_are_said_as_people_say_them() {
        let now = UNIX_EPOCH + Duration::from_secs(10_000_000);
        let before = |seconds| now - Duration::from_secs(seconds);
        assert_eq!(ago(before(5), now), "just now");
        assert_eq!(ago(before(60), now), "a minute ago");
        assert_eq!(ago(before(150), now), "2 minutes ago");
        assert_eq!(ago(before(3600), now), "an hour ago");
        assert_eq!(ago(before(30 * 3600), now), "yesterday");
        assert_eq!(ago(before(3 * 86_400), now), "3 days ago");
        assert_eq!(ago(before(21 * 86_400), now), "3 weeks ago");
        assert_eq!(ago(before(90 * 86_400), now), "3 months ago");
        assert_eq!(ago(now + Duration::from_secs(9), now), "just now");
    }

    /// A viewer with no document, as `zetwerk` alone opens, in
    /// `directory`, with its state in `state`.
    fn starting(directory: &Path, state: &Path) -> Viewer {
        let env = Environment::new(FontSet::Embedded);
        let project = Project::in_memory(
            Arc::clone(&env),
            directory.to_owned(),
            "start.typ",
            String::new(),
        )
        .unwrap();
        let mut viewer = Viewer::new(env, project, (1000, 620), 1.0).unwrap();
        viewer.state_dir = Some(state.to_owned());
        viewer.welcome(Opening::Start, None);
        viewer
    }

    #[test]
    fn the_start_lists_recent_documents_and_makes_new_ones() {
        let directory = directory("");
        let state = directory.join("state");
        let earlier = directory.join("earlier.typ");
        std::fs::write(&earlier, "Earlier.\n").unwrap();
        remember(&state, &earlier).unwrap();
        remember(&state, &directory.join("gone.typ")).unwrap();

        let mut viewer = starting(&directory, &state);
        assert!(viewer.picker.is_some());
        // The one that is there, then "new document…".
        let open = format!("open {}", earlier.canonicalize().unwrap().display());
        assert_eq!(
            viewer.picker.as_ref().unwrap().chosen(),
            Some(&Target::Command(open))
        );
        press(&mut viewer, "<CR>");
        let wanted = viewer.take_wanted_document();
        assert_eq!(wanted, Some((earlier.canonicalize().unwrap(), false)));
        assert_eq!(viewer.take_wanted_document(), None);

        // Escape leaves the window empty and says what can be done.
        let mut viewer = starting(&directory, &state);
        press(&mut viewer, "<Esc>");
        assert!(viewer.picker.is_none());
        assert!(viewer.message.is_some());
        // `:recent` brings the list back; "new document…" asks for a name.
        press(&mut viewer, ":recent<CR>");
        press(&mut viewer, "new document<CR>");
        assert!(viewer.picker.is_some());
        press(&mut viewer, "draft<CR>");
        let made = directory.join("draft.typ");
        assert_eq!(viewer.take_wanted_document(), Some((made.clone(), true)));
        assert_eq!(std::fs::read_to_string(&made).unwrap(), "");
        // A file that is there is never made anew.
        press(&mut viewer, ":new earlier<CR>");
        assert_eq!(viewer.take_wanted_document(), None);
        assert_eq!(std::fs::read_to_string(&earlier).unwrap(), "Earlier.\n");
        // Nor is anything opened that is not there.
        press(&mut viewer, ":e missing.typ<CR>");
        assert_eq!(viewer.take_wanted_document(), None);
        assert!(!directory.join("missing.typ").exists());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_document_with_unsaved_changes_is_not_left() {
        let (mut viewer, directory) = editing("Text.\n");
        let other = directory.join("other.typ");
        std::fs::write(&other, "Other.\n").unwrap();
        press(&mut viewer, "x");
        viewer.execute("open other.typ");
        assert_eq!(viewer.take_wanted_document(), None);
        press(&mut viewer, ":w<CR>");
        viewer.execute("open other.typ");
        assert_eq!(viewer.take_wanted_document(), Some((other, false)));
        // The document being edited is not opened again.
        viewer.execute("open main.typ");
        assert_eq!(viewer.take_wanted_document(), None);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_start_shows_no_page_and_the_documents_of_its_folder() {
        let directory = directory("= Book\n#include \"part.typ\"\n");
        let folder = directory.canonicalize().unwrap();
        std::fs::write(directory.join("part.typ"), "Part.\n").unwrap();
        std::fs::write(directory.join("other.typ"), "Other.\n").unwrap();
        let state = directory.join("state");
        let mut viewer = starting(&directory, &state);
        let listed: Vec<String> = viewer
            .picker
            .as_ref()
            .unwrap()
            .listing()
            .1
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert_eq!(
            listed[..3],
            [
                "main.typ\there",
                "other.typ\there",
                "part.typ\tpart of main.typ"
            ],
            "{listed:?}"
        );
        assert!(listed[3].starts_with("new document…"), "{listed:?}");
        press(&mut viewer, "<CR>");
        let wanted = viewer.take_wanted_document();
        assert_eq!(wanted, Some((folder.join("main.typ"), false)));
        // Nothing is drawn of the page of no document.
        compile(&mut viewer);
        let scene = viewer.scene(&|_| false);
        assert!(scene.wanted.is_empty(), "{} tiles", scene.wanted.len());
        // An agent is told there is no document, and what to do.
        let refused = viewer.lines(None, None).unwrap_err();
        assert!(refused.contains("no document is open"), "{refused}");
        let _ = std::fs::remove_dir_all(directory);
    }
}
