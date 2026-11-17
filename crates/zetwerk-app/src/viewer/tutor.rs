//! `:tutor`: zetwerk taught on the page, in lessons the reader works
//! through in a document of their own.
//!
//! A lesson is a Typst document that comes with zetwerk. `:tutor` puts a
//! copy of it in a scratch place (the `tutor` directory of the state
//! directory, one directory for each lesson, with the files of
//! `lessons/shared/` beside it) and opens that copy in the window; it is
//! typeset like any document, and the reader does in it what it says.
//! Nothing of the user's documents is touched. `:tutor` goes on with the
//! first lesson not done yet, `:tutor 4` goes to the fourth (again, from
//! its beginning), `:tutor list` lists them all; `Ctrl-T` (or `:hint`)
//! says the keys of the step at hand. Which lessons are done is kept in
//! the state directory.
//!
//! # A lesson
//!
//! Each lesson is one file, `lessons/<language>/<name>.typ`: `en` for
//! English, which has them all; `de` and `nl` may have their own of any
//! of them, of the same name, and English stands in for those they do
//! not have. The lessons are taken in the order of their names, which
//! begin with a number: 01 to 12 are the ground (moving, typing, undo,
//! formats, equations, figures, references, a page that stops changing,
//! the managers, search, saving, agents), **13 to 79 are free for what
//! came later**, and 80 and 81 end the course (for Vim users, and for
//! those who never used Vim). Adding a lesson is writing its file: the
//! build finds it.
//!
//! The file is the document as the reader sees it, followed by a block
//! comment that begins with a line `/* tutor` and says what the steps
//! are; the copy is made without that block. One line for each thing:
//!
//! ```text
//! /* tutor
//! title: Typing and deleting
//!
//! step: take out the word that is there twice
//! keys: /word word<CR>wdw
//! hint: put the cursor on the second “word” (w goes there), then dw
//! done: lacks word word
//! done: has a word too many
//! */
//! ```
//!
//! - `title:` what the lesson is called in the list.
//! - `step:` begins a step: what the status line says it is about.
//! - `keys:` the keys that do it, as `keys::parse` reads them; ` <pause> `
//!   among them is a pause in typing, at which the page catches up. They
//!   are what the tests play, so that a lesson cannot say what no longer
//!   holds.
//! - `hint:` what `Ctrl-T` says (the keys, where nothing is said).
//! - `done:` a check that holds once the step is done; a step is done
//!   when all of its hold. Checks of the text read it without the
//!   instructions (the calls of `task`):
//!   `has <text>`, `lacks <text>`, `before <one> | <other>` (both there,
//!   one first), `count <n> | <text>`, `cursor <text>` (the cursor is in
//!   that text), `below <text>`, `above <text>` (the cursor is at or
//!   behind where that text begins, before it), `mode normal|insert|visual`, `view page|source|split`,
//!   `panel <title>` (the list of that title is up), `message <text>`
//!   (the status line or the panel of `K` says it), `saved`,
//!   `unchanged` (the text is as the lesson came), `changed` (it is not
//!   as it was when the step began), `back` (it is as it was when the
//!   step before began), `typeset`
//!   (Typst typesets the text as it is), `refused` (it does not), `why`
//!   (the panel of `:why` is up), `jumping` (the labels of `gw` are
//!   up), `exported <ending>` (the document was
//!   written out as that). `\n` in a text is a line break.
//!
//! The cursor begins on the title: the first heading.
//!
//! The steps go with the calls of `task` in the document, in order: the
//! instructions of each. Beside the instruction of a step that is done
//! stands a tick in the margin, beside that of the step at hand a mark;
//! the status line says which lesson and which step. A step is checked
//! after every key and every compile, and only the step at hand: once
//! done it stays done.

use std::path::{Path, PathBuf};

use typst::syntax::{LinkedNode, Source, SyntaxKind};

use super::assist::Target;
use super::{Quad, Scene, TextureId, Viewer};
use crate::editor;
use crate::keys::{Key, KeyPress};

include!(concat!(env!("OUT_DIR"), "/lessons.rs"));

/// The language every lesson is written in.
const ENGLISH: &str = "en";
/// Where in the state directory the copies of the lessons are made, and
/// the file there that names the lessons that are done.
const TUTOR_DIR: &str = "tutor";
const PROGRESS: &str = "progress";
/// The file that says the program was never started before: what the
/// first window says, once (see [`note_first_run`]).
const FIRST_RUN: &str = "first-run";
/// What begins the steps of a lesson, at the start of a line.
const STEPS: &str = "/* tutor";
/// The instructions in a lesson: calls of this.
const TASK: &str = "task";
/// Among the keys of a step: a pause in typing.
#[cfg(test)]
pub(super) const PAUSE: &str = " <pause> ";

/// What the first window ever says.
const FIRST: &str = "new to zetwerk? :tutor teaches it in twenty minutes, on the page \
                     (g? lists all there is to do)";

/// The colours of the marks beside the instructions: done, at hand.
const DONE_COLOUR: &str = "#79740e";
const NOW_COLOUR: &str = "#d65d0e";
/// How far left of an instruction its mark stands, and how large it is,
/// in points of the page.
const MARK_GAP: f64 = 3.5;
const MARK_SIZE: f64 = 10.0;

/// Notes, where `state_dir` is not there yet, that the program is
/// started for the first time: the first window says once that there is
/// a tutor. Not being able to note it costs nothing.
pub fn note_first_run(state_dir: &Path) {
    if state_dir.exists() {
        return;
    }
    let directory = state_dir.join(TUTOR_DIR);
    if zetwerk_core::create_private_dir(&directory).is_ok() {
        let _ = std::fs::write(directory.join(FIRST_RUN), b"");
    }
}

/// A check of whether a step is done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Check {
    Has(String),
    Lacks(String),
    Before(String, String),
    Count(usize, String),
    Cursor(String),
    Below(String),
    Above(String),
    Mode(editor::Mode),
    View(super::Panes),
    Panel(String),
    Message(String),
    Saved,
    Unchanged,
    Changed,
    Back,
    Typeset,
    Refused,
    Why,
    Jumping,
    Exported(String),
}

impl Check {
    /// The check `line` says (what follows `done:`).
    fn read(line: &str) -> Result<Self, String> {
        let (kind, argument) = line.split_once(' ').unwrap_or((line, ""));
        let argument = argument.trim().replace("\\n", "\n");
        let empty = argument.is_empty();
        let two = || {
            argument
                .split_once(" | ")
                .map(|(one, other)| (one.to_owned(), other.to_owned()))
                .ok_or_else(|| format!("{kind} wants two things, apart by \" | \": {line}"))
        };
        let check = match kind {
            "has" => Self::Has(argument),
            "lacks" => Self::Lacks(argument),
            "before" => {
                let (one, other) = two()?;
                Self::Before(one, other)
            }
            "count" => {
                let (count, text) = two()?;
                let count = count.parse().map_err(|_| format!("not a number: {line}"))?;
                Self::Count(count, text)
            }
            "cursor" => Self::Cursor(argument),
            "below" => Self::Below(argument),
            "above" => Self::Above(argument),
            "mode" => Self::Mode(match argument.as_str() {
                "normal" => editor::Mode::Normal,
                "insert" => editor::Mode::Insert,
                "visual" => editor::Mode::Visual,
                _ => return Err(format!("no such mode: {line}")),
            }),
            "view" => Self::View(match argument.as_str() {
                "page" => super::Panes::Page,
                "source" => super::Panes::Source,
                "split" => super::Panes::Split,
                _ => return Err(format!("no such view: {line}")),
            }),
            "panel" => Self::Panel(argument),
            "message" => Self::Message(argument),
            "saved" => Self::Saved,
            "unchanged" => Self::Unchanged,
            "changed" => Self::Changed,
            "back" => Self::Back,
            "typeset" => Self::Typeset,
            "refused" => Self::Refused,
            "why" => Self::Why,
            "jumping" => Self::Jumping,
            "exported" => Self::Exported(argument),
            _ => return Err(format!("no such check: {line}")),
        };
        let wants_text = matches!(
            check,
            Self::Has(_)
                | Self::Lacks(_)
                | Self::Cursor(_)
                | Self::Below(_)
                | Self::Above(_)
                | Self::Panel(_)
                | Self::Message(_)
                | Self::Exported(_)
        );
        if wants_text && empty {
            return Err(format!("{kind} of what? {line}"));
        }
        Ok(check)
    }
}

/// A step of a lesson.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Step {
    /// What it is about, in a few words.
    pub(super) say: String,
    /// The keys that do it (see the module's documentation).
    pub(super) keys: String,
    /// What `Ctrl-T` says.
    pub(super) hint: String,
    /// What holds once it is done.
    pub(super) done: Vec<Check>,
}

/// A lesson, as its file has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Lesson {
    /// The language it is written in, and its name: its file's, without
    /// the ending.
    pub(super) language: &'static str,
    pub(super) name: &'static str,
    pub(super) title: String,
    /// The document, without the steps.
    pub(super) text: String,
    pub(super) steps: Vec<Step>,
}

impl Lesson {
    /// The lesson of `language` called `name` whose file holds `file`.
    pub(super) fn read(
        language: &'static str,
        name: &'static str,
        file: &str,
    ) -> Result<Self, String> {
        let at = file
            .match_indices(STEPS)
            .map(|(at, _)| at)
            .find(|at| *at == 0 || file[..*at].ends_with('\n'))
            .ok_or_else(|| format!("{name}: no steps (a line \"{STEPS}\")"))?;
        let text = file[..at].trim_end().to_owned() + "\n";
        let block = file[at + STEPS.len()..].trim_end();
        let block = block.strip_suffix("*/").unwrap_or(block);
        let mut title = None;
        let mut steps: Vec<Step> = Vec::new();
        for line in block.lines().map(str::trim).filter(|line| !line.is_empty()) {
            let (key, value) = line
                .split_once(':')
                .map(|(key, value)| (key.trim(), value.trim()))
                .ok_or_else(|| format!("{name}: not a line of a lesson: {line}"))?;
            let step = steps.last_mut();
            match (key, step) {
                ("title", _) => title = Some(value.to_owned()),
                ("step", _) => steps.push(Step {
                    say: value.to_owned(),
                    keys: String::new(),
                    hint: String::new(),
                    done: Vec::new(),
                }),
                ("keys", Some(step)) => value.clone_into(&mut step.keys),
                ("hint", Some(step)) => value.clone_into(&mut step.hint),
                ("done", Some(step)) => step
                    .done
                    .push(Check::read(value).map_err(|why| format!("{name}: {why}"))?),
                _ => return Err(format!("{name}: {key} where it does not belong: {line}")),
            }
        }
        for step in &mut steps {
            if step.done.is_empty() || step.keys.is_empty() {
                return Err(format!("{name}: “{}” lacks keys or checks", step.say));
            }
            if step.hint.is_empty() {
                step.hint.clone_from(&step.keys);
            }
        }
        let title = title.ok_or_else(|| format!("{name}: no title"))?;
        if steps.is_empty() {
            return Err(format!("{name}: no steps"));
        }
        Ok(Self {
            language,
            name,
            title,
            text,
            steps,
        })
    }

    /// The name of the directory its copy is made in.
    fn directory(&self) -> String {
        format!("{}-{}", self.language, self.name)
    }

    /// The name of the file of its copy: its name without the number.
    fn file_name(&self) -> String {
        let bare = self
            .name
            .trim_start_matches(|c: char| c.is_ascii_digit() || c == '-');
        format!("{}.typ", if bare.is_empty() { self.name } else { bare })
    }
}

/// The lessons of `language`, in order: those of English, each in
/// `language` where it has its own.
pub(super) fn lessons(language: &str) -> Vec<Lesson> {
    LESSON_FILES
        .iter()
        .filter(|(of, ..)| *of == ENGLISH)
        .filter_map(|(_, name, english)| {
            let own = LESSON_FILES
                .iter()
                .find(|(of, other, _)| *of == language && other == name);
            let (language, file) = own.map_or((ENGLISH, *english), |(of, _, file)| (*of, *file));
            Lesson::read(language, name, file).ok()
        })
        .collect()
}

/// The language of the user, as the two letters of its code: what the
/// settings say spelling is checked in, else what the system says.
fn language_of(settings: Option<&str>) -> String {
    let system = std::env::var("LC_MESSAGES")
        .ok()
        .or_else(|| std::env::var("LANG").ok());
    let code = settings.map(str::to_owned).or(system).unwrap_or_default();
    code.chars()
        .take_while(char::is_ascii_alphabetic)
        .collect::<String>()
        .to_ascii_lowercase()
}

/// The bytes of `source` that are instructions: the calls of [`TASK`],
/// with the `#` before them.
fn instructions(source: &Source) -> Vec<std::ops::Range<usize>> {
    fn walk(node: &LinkedNode, found: &mut Vec<std::ops::Range<usize>>) {
        if node.kind() == SyntaxKind::FuncCall
            && node
                .children()
                .next()
                .is_some_and(|callee| callee.leaf_text() == TASK)
        {
            let start = node.offset().saturating_sub(1);
            found.push(start..node.offset() + node.len());
            return;
        }
        for child in node.children() {
            walk(&child, found);
        }
    }
    let mut found = Vec::new();
    walk(&LinkedNode::new(source.root()), &mut found);
    found
}

/// `text` without what `left_out` covers (byte ranges, in order).
fn without(text: &str, left_out: &[std::ops::Range<usize>]) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut from = 0;
    for range in left_out {
        kept.push_str(text.get(from..range.start).unwrap_or(""));
        from = range.end.max(from);
    }
    kept.push_str(text.get(from..).unwrap_or(""));
    kept
}

/// The lessons done, as the state directory has them.
fn done_lessons(state_dir: &Path) -> Vec<String> {
    std::fs::read_to_string(state_dir.join(TUTOR_DIR).join(PROGRESS))
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The lesson in the window, and how far the reader is in it.
pub(super) struct Active {
    lesson: Lesson,
    /// Its place in the list, from 1, and how many there are.
    number: usize,
    of: usize,
    /// How many of its steps are done.
    done: usize,
    /// The text as it was when each step began, up to the one at hand.
    began: Vec<String>,
}

/// What the tutor keeps.
#[derive(Default)]
pub(super) struct Tutor {
    active: Option<Active>,
    /// Where the instructions are on the pages, as the last compile
    /// typeset them: the page, and the left and the baseline of the
    /// first line of each, in points.
    marks: Vec<Option<(usize, f64, f64)>>,
}

impl Viewer {
    /// The language the lessons are taken in.
    fn lesson_language(&self) -> String {
        language_of(self.settings_language())
    }

    /// The lessons, in the user's language.
    fn all_lessons(&self) -> Vec<Lesson> {
        lessons(&self.lesson_language())
    }

    /// The commands of the tutor: `:tutor`, `:tutor <n>`, `:tutor list`
    /// and `:hint`. Returns whether `command` was one.
    pub(super) fn tutor_command(&mut self, command: &str) -> bool {
        let (name, wanted) = match command.split_once(' ') {
            Some((name, wanted)) => (name, wanted.trim()),
            None => (command, ""),
        };
        match (name, wanted) {
            ("tutor", "") => {
                let done = self
                    .state_dir
                    .as_deref()
                    .map(done_lessons)
                    .unwrap_or_default();
                let lessons = self.all_lessons();
                match lessons
                    .iter()
                    .position(|lesson| !done.iter().any(|name| name == lesson.name))
                {
                    Some(next) => self.open_lesson(next + 1),
                    None => self.list_lessons(),
                }
            }
            ("tutor", "list" | "?") => self.list_lessons(),
            ("tutor", number) => match number.parse::<usize>() {
                Ok(number) if number >= 1 => self.open_lesson(number),
                _ => self.alert(format!(
                    "no lesson {number}: :tutor 1 to :tutor {}, :tutor list lists them",
                    self.all_lessons().len()
                )),
            },
            ("hint", "") => self.tell_hint(),
            _ => return false,
        }
        true
    }

    /// `:tutor list`: the lessons, each with whether it is done.
    fn list_lessons(&mut self) {
        let done = self
            .state_dir
            .as_deref()
            .map(done_lessons)
            .unwrap_or_default();
        let lessons = self.all_lessons();
        let next = lessons
            .iter()
            .position(|lesson| !done.iter().any(|name| name == lesson.name));
        let items = lessons
            .iter()
            .enumerate()
            .map(|(index, lesson)| {
                let state = if done.iter().any(|name| name == lesson.name) {
                    "done"
                } else if Some(index) == next {
                    "next"
                } else {
                    ""
                };
                let label = format!("{:>2}  {}\t{state}", index + 1, lesson.title);
                (label, Target::Command(format!("tutor {}", index + 1)))
            })
            .collect();
        self.open_picker("lessons", items, next.unwrap_or(0));
    }

    /// Makes a fresh copy of lesson `number` (from 1) and opens it in the
    /// window.
    fn open_lesson(&mut self, number: usize) {
        let lessons = self.all_lessons();
        let Some(lesson) = lessons.get(number.wrapping_sub(1)).cloned() else {
            self.alert(format!(
                "no lesson {number}: :tutor 1 to :tutor {}",
                lessons.len()
            ));
            return;
        };
        let Some(state_dir) = self.state_dir.clone() else {
            self.alert("the lessons need a state directory, and there is none");
            return;
        };
        let directory = state_dir.join(TUTOR_DIR).join(lesson.directory());
        let path = directory.join(lesson.file_name());
        // The same lesson again, in this window: begun anew in place.
        if self
            .lesson_here()
            .is_some_and(|(here, _)| here.name == lesson.name)
        {
            self.begin_again(&lesson, number, lessons.len());
            return;
        }
        // A lesson the reader leaves needs no word about its changes.
        if self.lesson_here().is_some() {
            let _ = self.editor.as_mut().map(|editor| editor.save(false));
        }
        match zetwerk_core::forget(&path, &state_dir) {
            Ok(true) => {}
            Ok(false) => {
                self.alert("that lesson is open in another window: go on there");
                return;
            }
            Err(err) => {
                self.alert(format!("cannot begin the lesson: {err}"));
                return;
            }
        }
        if let Err(err) = make_copy(&directory, &path, &lesson.text) {
            self.alert(format!("cannot begin the lesson: {err}"));
            return;
        }
        self.execute(&format!("open {}", path.display()));
    }

    /// Begins the lesson in the window anew: its text as it came, as one
    /// step that undo takes back, and none of its steps done.
    fn begin_again(&mut self, lesson: &Lesson, number: usize, of: usize) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let end = editor.text().len();
        if let Err(why) = editor.replace(&[(0..end, &lesson.text)]) {
            self.alert(format!("not begun again: {why}"));
            return;
        }
        editor.jump(0);
        self.text_changed(super::Edited::Command);
        self.follow_cursor();
        self.begin_lesson(lesson.clone(), number, of);
    }

    /// The lesson the window shows a copy of, if it does, and its
    /// number.
    fn lesson_here(&self) -> Option<(Lesson, usize)> {
        let path = self.editor.as_ref()?.path();
        let tutor = self.state_dir.as_ref()?.join(TUTOR_DIR);
        let directory = path.parent()?;
        let same = |a: &Path, b: &Path| {
            a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
        };
        if !same(directory.parent()?, &tutor) {
            return None;
        }
        let name = directory.file_name()?.to_str()?;
        let lessons = self.all_lessons();
        let number = lessons.iter().position(|lesson| {
            lesson.directory() == name
                && path.file_name().and_then(|file| file.to_str()) == Some(&lesson.file_name())
        })?;
        Some((lessons.get(number)?.clone(), number + 1))
    }

    /// What the window says first, for the tutor: once ever that there
    /// is one; and where it shows a lesson, that it does.
    pub(super) fn tutor_welcome(&mut self) {
        if let Some(state_dir) = &self.state_dir {
            let marker = state_dir.join(TUTOR_DIR).join(FIRST_RUN);
            if marker.exists() {
                let _ = std::fs::remove_file(marker);
                self.notice(FIRST);
            }
        }
        if let Some((lesson, number)) = self.lesson_here() {
            let of = self.all_lessons().len();
            self.begin_lesson(lesson, number, of);
        }
    }

    /// Makes `lesson` the one at hand, with none of its steps done, and
    /// puts the cursor on its title.
    fn begin_lesson(&mut self, lesson: Lesson, number: usize, of: usize) {
        if let Some(editor) = &mut self.editor {
            let title = editor
                .text()
                .match_indices("\n= ")
                .next()
                .map(|(at, _)| at + 3);
            if let Some(title) = title {
                editor.jump(title);
            }
        }
        let first = lesson.steps.first().map(|step| step.say.clone());
        self.notice(format!(
            "lesson {number} of {of}: {} — do what the page says; Ctrl-T when stuck{}",
            lesson.title,
            first.map_or_else(String::new, |first| format!("  (first: {first})"))
        ));
        let began = self.text().map(str::to_owned).into_iter().collect();
        self.tutor.active = Some(Active {
            lesson,
            number,
            of,
            done: 0,
            began,
        });
        self.tutor.marks.clear();
        self.tutor_compiled();
    }

    /// `Ctrl-T`, `:hint`: the keys of the step at hand.
    fn tell_hint(&mut self) {
        let Some(active) = &self.tutor.active else {
            self.notice("no lesson is open: :tutor begins one");
            return;
        };
        match active.lesson.steps.get(active.done) {
            Some(step) => {
                let told = format!("{}: {}", step.say, step.hint);
                self.notice(told);
            }
            None => self.notice("this lesson is done: :tutor goes on with the next"),
        }
    }

    /// A key for the tutor: `Ctrl-T` while a lesson is open. Returns
    /// whether it was taken.
    pub(super) fn tutor_key(&mut self, press: KeyPress) -> bool {
        if self.tutor.active.is_none() || !(press.ctrl && press.key == Key::Char('t')) {
            return false;
        }
        self.tell_hint();
        true
    }

    /// After every key: whether the step at hand is done now.
    pub(super) fn tutor_keyed(&mut self) {
        let Some(active) = &self.tutor.active else {
            return;
        };
        let steps = active.lesson.steps.clone();
        let Some(step) = steps.get(active.done) else {
            return;
        };
        if !step.done.iter().all(|check| self.holds(check)) {
            return;
        }
        let text = self.text().map(str::to_owned).unwrap_or_default();
        let Some(active) = &mut self.tutor.active else {
            return;
        };
        active.done += 1;
        active.began.push(text);
        let done = active.done;
        let (number, of, name) = (active.number, active.of, active.lesson.name);
        let told = match steps.get(done) {
            Some(next) => format!("✓ done.  Next: {}", next.say),
            None if number < of => {
                let next = self
                    .all_lessons()
                    .get(number)
                    .map(|lesson| lesson.title.clone())
                    .unwrap_or_default();
                format!(
                    "✓ lesson {number} done.  :tutor goes on with lesson {}: {next}",
                    number + 1
                )
            }
            None => "✓ the last lesson is done: that is all of it. g? lists all there is to do"
                .to_owned(),
        };
        if done == steps.len() {
            self.lesson_done(name);
        }
        self.notice(told);
    }

    /// After a compile: whether the step at hand is done now, and where
    /// the instructions are on the new pages.
    pub(super) fn tutor_compiled(&mut self) {
        if self.tutor.active.is_none() {
            return;
        }
        self.tutor.marks.clear();
        if let (Some(document), Some(world), Some(editor)) =
            (&self.document, &self.compiled_world, &self.editor)
            && let Ok(source) = world.written(editor.file())
        {
            self.tutor.marks = instructions(&source)
                .into_iter()
                .map(|range| {
                    // The first letter of what it says: behind `#task[`.
                    let at = range.start + TASK.len() + 2;
                    zetwerk_typst::locate(document.pages(), &source, at, 0)
                        .map(|found| (found.page, found.x, found.y))
                })
                .collect();
        }
        self.tutor_keyed();
    }

    /// Notes in the state directory that lesson `name` is done.
    fn lesson_done(&self, name: &str) {
        let Some(state_dir) = &self.state_dir else {
            return;
        };
        let mut done = done_lessons(state_dir);
        if done.iter().any(|other| other == name) {
            return;
        }
        done.push(name.to_owned());
        let directory = state_dir.join(TUTOR_DIR);
        if zetwerk_core::create_private_dir(&directory).is_ok() {
            let text = done.join("\n") + "\n";
            let _ = zetwerk_core::write_atomically(&directory.join(PROGRESS), text.as_bytes());
        }
    }

    /// Whether `check` holds now.
    fn holds(&self, check: &Check) -> bool {
        let Some(editor) = &self.editor else {
            return false;
        };
        let practice = || without(editor.text(), &instructions(editor.source()));
        match check {
            Check::Has(text) => practice().contains(text.as_str()),
            Check::Lacks(text) => !practice().contains(text.as_str()),
            Check::Before(one, other) => {
                let practice = practice();
                matches!((practice.find(one.as_str()), practice.find(other.as_str())),
                    (Some(one), Some(other)) if one < other)
            }
            Check::Count(count, text) => practice().matches(text.as_str()).count() == *count,
            Check::Cursor(text) => {
                let cursor = editor.cursor();
                editor
                    .text()
                    .match_indices(text.as_str())
                    .any(|(at, found)| (at..at + found.len()).contains(&cursor))
            }
            Check::Below(text) => editor
                .text()
                .find(text.as_str())
                .is_some_and(|at| editor.cursor() >= at),
            Check::Above(text) => editor
                .text()
                .find(text.as_str())
                .is_some_and(|at| editor.cursor() < at),
            Check::Mode(mode) => match mode {
                editor::Mode::Visual => matches!(
                    editor.mode(),
                    editor::Mode::Visual | editor::Mode::VisualLine
                ),
                mode => editor.mode() == *mode,
            },
            Check::View(panes) => self.panes == *panes,
            Check::Panel(title) => self
                .picker
                .as_ref()
                .is_some_and(|picker| picker.title() == title),
            Check::Message(text) => {
                let said = self.message.as_ref().map(|message| message.text.as_str());
                [said, self.tooltip.as_deref()]
                    .into_iter()
                    .flatten()
                    .any(|said| said.contains(text.as_str()))
            }
            Check::Saved => !editor.is_modified(),
            Check::Changed => self.tutor.active.as_ref().is_some_and(|active| {
                active
                    .began
                    .last()
                    .is_some_and(|then| then != editor.text())
            }),
            Check::Back => self.tutor.active.as_ref().is_some_and(|active| {
                active
                    .began
                    .iter()
                    .rev()
                    .nth(1)
                    .is_some_and(|then| then == editor.text())
            }),
            Check::Unchanged => self
                .tutor
                .active
                .as_ref()
                .is_some_and(|active| active.lesson.text == editor.text()),
            Check::Typeset => {
                self.compiled_revision == Some(self.revision) && self.refusal().is_none()
            }
            Check::Refused => {
                self.compiled_revision == Some(self.revision) && self.refusal().is_some()
            }
            Check::Why => self.refused_up(),
            Check::Jumping => self.is_jumping(),
            Check::Exported(ending) => editor.path().with_extension(ending).is_file(),
        }
    }

    /// What the status line says of the lesson, at its right.
    pub(super) fn tutor_status(&self) -> String {
        let Some(active) = &self.tutor.active else {
            return String::new();
        };
        let steps = active.lesson.steps.len();
        if active.done >= steps {
            format!("lesson {}/{} done  ", active.number, active.of)
        } else {
            format!(
                "lesson {}/{} · step {}/{steps}  ",
                active.number,
                active.of,
                active.done + 1
            )
        }
    }

    /// Draws beside the instructions on page `index` (whose top-left
    /// corner is at `origin`) a tick for each step done and a mark for
    /// the step at hand.
    pub(super) fn tutor_marks(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        index: usize,
        origin: (f64, f64),
    ) {
        let Some(active) = &self.tutor.active else {
            return;
        };
        let done = active.done;
        let marks: Vec<(usize, f64, f64)> = self
            .tutor
            .marks
            .iter()
            .enumerate()
            .take(done + 1)
            .filter_map(|(step, mark)| {
                let (page, x, y) = (*mark)?;
                (page == index).then_some((step, x, y))
            })
            .collect();
        let zoom = self.view.zoom;
        for (step, x, y) in marks {
            let (sign, colour) = if step < done {
                ("✓", DONE_COLOUR)
            } else {
                ("▸", NOW_COLOUR)
            };
            let markup = format!(
                "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n\
                 #set text(size: {MARK_SIZE}pt, fill: rgb(\"{colour}\"))\n{sign}"
            );
            let Some((id, (width, height), image)) =
                self.chrome.render_unless_held(&markup, zoom, has)
            else {
                continue;
            };
            let (width, height) = (f64::from(width), f64::from(height));
            scene.quads.push(Quad {
                texture: id,
                x: (origin.0 + (x - MARK_GAP) * zoom - width).round() as f32,
                y: (origin.1 + (y - MARK_SIZE * 0.35) * zoom - height / 2.0).round() as f32,
                width: width as f32,
                height: height as f32,
            });
            if let Some(image) = image.filter(|_| !has(id)) {
                scene.images.push(image);
            }
        }
    }

    /// How many steps of the lesson at hand are done, if one is.
    #[cfg(test)]
    pub(super) fn steps_done(&self) -> Option<usize> {
        self.tutor.active.as_ref().map(|active| active.done)
    }
}

/// Makes `directory` anew with the lesson's text in `path`, and the files
/// every lesson has beside it.
fn make_copy(directory: &Path, path: &PathBuf, text: &str) -> std::io::Result<()> {
    if directory.exists() {
        std::fs::remove_dir_all(directory)?;
    }
    zetwerk_core::create_private_dir(directory)?;
    for (name, contents) in SHARED_FILES {
        std::fs::write(directory.join(name), contents)?;
    }
    std::fs::write(path, text)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use super::*;

    #[test]
    fn a_lesson_is_read_from_its_file() {
        let file = "= Typing\n\n#task[Take it out.]\n\nA word word.\n\n/* tutor\n\
                    title: Typing\n\nstep: take out the word\nkeys: /word word<CR>wdw\n\
                    done: lacks word word\ndone: before A | word.\n*/\n";
        let lesson = Lesson::read("en", "02-typing", file).unwrap();
        assert_eq!(lesson.title, "Typing");
        assert_eq!(
            lesson.text,
            "= Typing\n\n#task[Take it out.]\n\nA word word.\n"
        );
        assert_eq!(lesson.steps.len(), 1);
        let step = &lesson.steps[0];
        assert_eq!(step.hint, step.keys, "the keys, where no hint is given");
        assert_eq!(
            step.done,
            [
                Check::Lacks("word word".into()),
                Check::Before("A".into(), "word.".into())
            ]
        );
        assert_eq!(lesson.file_name(), "typing.typ");
        assert_eq!(lesson.directory(), "en-02-typing");
        // What is not right is said.
        assert!(Lesson::read("en", "x", "Text.\n").is_err());
        let wrong = file.replace("done: lacks", "done: misses");
        assert!(Lesson::read("en", "x", &wrong).is_err());
    }

    #[test]
    fn the_instructions_are_not_what_the_checks_read() {
        let text = "Before #task[Type *this*.] after #task[More] end.";
        let source = Source::detached(text);
        let found = instructions(&source);
        assert_eq!(found.len(), 2);
        assert_eq!(without(text, &found), "Before  after  end.");
    }

    #[test]
    fn every_lesson_has_its_english_one_and_is_read() {
        let english = lessons(ENGLISH);
        let files = LESSON_FILES
            .iter()
            .filter(|(of, ..)| *of == ENGLISH)
            .count();
        assert_eq!(english.len(), files, "a lesson is not read");
        for (language, name, file) in LESSON_FILES {
            Lesson::read(language, name, file).unwrap_or_else(|why| panic!("{why}"));
            assert!(
                LESSON_FILES
                    .iter()
                    .any(|(of, other, _)| *of == ENGLISH && other == name),
                "{language}/{name} has no English lesson of that name"
            );
        }
        // A language without lessons of its own is taught in English.
        assert_eq!(lessons("fr"), english);
    }

    #[test]
    fn the_language_is_the_settings_and_else_the_systems() {
        assert_eq!(language_of(Some("de-DE")), "de");
        assert_eq!(language_of(Some("nl")), "nl");
    }
}
