//! Help with writing: what could be typed here (a list to pick from
//! while typing), what the thing under the cursor is (`K`), where it is
//! defined (`gd`), and a list of the document's headings or labels to go
//! to (`go`).
//!
//! What Typst knows comes from its own tooling (`zetwerk_typst::complete`
//! and its neighbours); this is the keys and the lists.

use zetwerk_typst::Candidate;

use crate::typst_text::{Insert, MATH_INSERTS, TEXT_INSERTS};

use super::{Scene, TextureId, Viewer, chrome};
use crate::editor;
use crate::keys::{Key, KeyPress};

/// How many rows of a list are shown at once.
const LIST_ROWS: usize = 9;
/// How many lines of an explanation are shown, and how long a line.
const TOOLTIP_LINES: usize = 14;
const TOOLTIP_COLUMNS: usize = 84;

/// The list of what could be typed at the cursor.
pub(super) struct Completion {
    pub(super) candidates: Vec<Entry>,
    chosen: usize,
}

/// One of the things that could be typed.
pub(super) struct Entry {
    pub(super) candidate: Candidate,
    /// The byte from which on it replaces what was typed.
    from: usize,
    /// What it is, if it is one of the things of the insert menu and not
    /// a name Typst knows.
    insert: Option<Insert>,
}

/// A list of places to go, narrowed by typing.
pub(super) struct Picker {
    title: &'static str,
    /// What each is listed as and where it is.
    items: Vec<(String, Target)>,
    query: String,
    /// The items the query leaves, best first.
    shown: Vec<usize>,
    chosen: usize,
}

/// Where an entry of a list leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Target {
    /// A byte of the file being edited.
    Offset(usize),
    /// A file of the project.
    File(std::path::PathBuf),
    /// Text to put in at the cursor, in place of the last `typed`
    /// characters before it; the cursor goes `within` bytes into it.
    Put {
        text: String,
        within: usize,
        typed: usize,
    },
}

/// How deep below the document's folder files are looked for, and how
/// many are looked at.
const FILES_DEEP: usize = 4;
const FILES_SEEN: usize = 4000;

/// The calls whose first argument is a file, and what such files end in.
const FILE_CALLS: &[(&str, &[&str])] = &[
    (
        "image(\"",
        &["png", "jpg", "jpeg", "gif", "svg", "webp", "pdf"],
    ),
    ("bibliography(\"", &["bib", "yml", "yaml"]),
    ("include \"", &["typ"]),
];

/// The files in and below `folder` whose names end in one of `endings`,
/// as paths from `folder`.
fn files_below(folder: &std::path::Path, endings: &[&str]) -> Vec<String> {
    let mut found = Vec::new();
    let mut seen = 0;
    let mut folders = vec![(folder.to_path_buf(), 0)];
    while let Some((here, depth)) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&here) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > FILES_SEEN {
                return found;
            }
            let path = entry.path();
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            if hidden {
                continue;
            }
            if path.is_dir() {
                if depth < FILES_DEEP && entry.file_name() != "target" {
                    folders.push((path, depth + 1));
                }
                continue;
            }
            let ending = path.extension().and_then(|ending| ending.to_str());
            let wanted = ending.is_some_and(|ending| {
                endings
                    .iter()
                    .any(|wanted| wanted.eq_ignore_ascii_case(ending))
            });
            if let (true, Ok(within)) = (wanted, path.strip_prefix(folder)) {
                found.push(within.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    found.sort();
    found
}

/// How well `query` is found in `text`, all its characters in their
/// order: more for characters that follow each other and that begin
/// words. `None` if it is not found.
fn score(text: &str, query: &str) -> Option<i32> {
    let mut wanted = query.chars().flat_map(char::to_lowercase).peekable();
    let (mut score, mut run, mut before) = (0, 0, None::<char>);
    for c in text.chars() {
        let Some(next) = wanted.peek() else {
            break;
        };
        if c.to_lowercase().eq(std::iter::once(*next)) {
            wanted.next();
            run += 1;
            let begins = before.is_none_or(|before| !before.is_alphanumeric());
            score += run * 2 + if begins { 6 } else { 0 };
        } else {
            run = 0;
            score -= 1;
        }
        before = Some(c);
    }
    wanted.peek().is_none().then_some(score)
}

/// A line of `text` cut to `columns` characters.
fn cut(text: &str, columns: usize) -> String {
    let mut shown: String = text.chars().take(columns).collect();
    if shown.len() < text.len() {
        shown.push('…');
    }
    shown
}

/// The things of the insert menu that go with `typed`, each with how
/// well: behind a hash in running text, by their names in an equation.
/// `begun` is the byte the typed word begins at.
fn menu_entries(in_math: bool, hash: bool, typed: &str, begun: usize) -> Vec<(i32, Entry)> {
    let (inserts, from) = match (in_math, hash) {
        (false, true) => (TEXT_INSERTS, begun.saturating_sub(1)),
        (true, false) => (MATH_INSERTS, begun),
        _ => return Vec::new(),
    };
    let mut entries = Vec::new();
    for insert in inserts {
        // What begins as typed (in any of its words) before what Typst
        // knows by that beginning; what only has the letters in it,
        // behind that.
        let lower = typed.to_lowercase();
        let begins = insert.name.split(' ').any(|word| word.starts_with(&lower));
        let named = if insert.short == typed && !typed.is_empty() {
            Some(3000)
        } else if begins {
            Some(2000)
        } else if in_math {
            None
        } else {
            score(insert.name, typed).map(|score| 500 + score.clamp(0, 400))
        };
        let Some(score) = named else {
            continue;
        };
        let (text, cursor) = crate::typst_text::insert_text(insert, false, false);
        let candidate = Candidate {
            label: insert.name.to_owned(),
            kind: if insert.short.is_empty() {
                "insert"
            } else {
                insert.short
            },
            detail: Some(insert.body.replace('|', "").replace('\n', " ")),
            text,
            cursor,
        };
        let entry = Entry {
            candidate,
            from,
            insert: Some(*insert),
        };
        entries.push((score, entry));
    }
    entries
}

impl Viewer {
    /// A key that may be for a list on screen. Returns whether it was.
    pub(super) fn assist_key(&mut self, press: KeyPress) -> bool {
        self.tooltip = None;
        if self.picker.is_some() {
            self.picker_key(press);
            return true;
        }
        let typing = self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.mode() == editor::Mode::Insert);
        if !typing {
            self.completion = None;
            return false;
        }
        let next = matches!(
            (press.key, press.ctrl),
            (Key::Char('n'), true) | (Key::Down, _)
        );
        let previous = matches!(
            (press.key, press.ctrl),
            (Key::Char('p'), true) | (Key::Up, _)
        );
        let take = matches!(
            (press.key, press.ctrl),
            (Key::Tab, _) | (Key::Char('y'), true)
        );
        // Enter too, for a thing of the insert menu: nobody means a new
        // paragraph behind half a name.
        let chosen_insert = self.completion.as_ref().is_some_and(|completion| {
            let chosen = completion.candidates.get(completion.chosen);
            chosen.is_some_and(|entry| entry.insert.is_some())
        });
        let take = take || (press.key == Key::Enter && chosen_insert);
        let asked = matches!((press.key, press.ctrl), (Key::Char('n' | ' '), true));
        let Some(completion) = &mut self.completion else {
            if asked {
                self.complete(true);
                if self.completion.is_none() {
                    self.notice("nothing to complete here");
                }
            }
            return asked;
        };
        let count = completion.candidates.len();
        if next {
            completion.chosen = (completion.chosen + 1) % count;
        } else if previous {
            completion.chosen = (completion.chosen + count - 1) % count;
        } else if take && self.typed_is_chosen() {
            // Nothing to take: the key is for the text.
            self.completion = None;
            return false;
        } else if take {
            self.take_completion();
        } else if matches!((press.key, press.ctrl), (Key::Char('e'), true)) {
            self.completion = None;
        } else {
            // Any other key is typed, and the list follows the typing.
            return false;
        }
        true
    }

    /// Brings the list of completions up to date with the text, after a
    /// key was typed: Typst is asked where a list is likely to be wanted
    /// (behind `#` and `@`), or wherever the user asked (`explicit`).
    pub(super) fn complete(&mut self, explicit: bool) {
        let was_open = self.completion.take().is_some();
        let Some(editor) = &self.editor else {
            return;
        };
        if editor.mode() != editor::Mode::Insert {
            return;
        }
        let (text, cursor) = (editor.text(), editor.cursor());
        // The word being typed, and what stands before it.
        let begun = text[..cursor]
            .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_' || c == '-')
            .len();
        let trigger = text[..begun].chars().next_back();
        let typed = &text[begun..cursor];
        let source = editor.source().clone();
        let in_math = crate::typst_text::is_in_equation(&source, begun.saturating_sub(1));
        let hash = trigger == Some('#');
        // In an equation a list comes with the second letter: there,
        // nearly every word is a name of something.
        let naming = in_math && typed.chars().count() >= 2 && !hash;
        if !(explicit || was_open || naming || matches!(trigger, Some('#' | '@'))) {
            return;
        }
        // A list that was up goes when the word it was for is over.
        if typed.is_empty() && !explicit && !matches!(trigger, Some('#' | '@')) {
            return;
        }

        let mut entries = menu_entries(in_math, hash, typed, begun);

        // And what Typst itself knows could stand here.
        self.project.set_source(source.clone());
        let world = self.project.snapshot();
        let document = self.document.as_deref();
        let asked = explicit || naming;
        if let Some((from, mut candidates)) =
            zetwerk_typst::complete(&world, document, &source, cursor, asked)
            && let Some(typed) = source.text().get(from..cursor)
        {
            candidates.dedup_by(|a, b| a.label == b.label);
            for candidate in candidates {
                let label = candidate.label.trim_start_matches(['#', '@', '<']);
                // What begins as typed, letter for letter, before what
                // only does so but for its capitals.
                let begins = label
                    .to_lowercase()
                    .starts_with(&typed.to_lowercase())
                    .then_some(1000 + i32::from(label.starts_with(typed)));
                // In an equation, only what begins with what was typed.
                let found = begins.or_else(|| score(label, typed).filter(|_| !naming));
                if let Some(score) = found {
                    let entry = Entry {
                        candidate,
                        from,
                        insert: None,
                    };
                    entries.push((score, entry));
                }
            }
        }
        // The best first. Among equals: the things of the menu in their
        // order, Typst's names the shortest first (of two that begin as
        // typed, the shorter is the nearer).
        entries.sort_by_key(|(score, entry)| {
            let length = match entry.insert {
                Some(_) => 0,
                None => entry.candidate.label.len(),
            };
            (std::cmp::Reverse(*score), length)
        });
        if entries.is_empty() {
            return;
        }
        self.completion = Some(Completion {
            candidates: entries.into_iter().map(|(_, entry)| entry).collect(),
            chosen: 0,
        });
    }

    /// Whether what is typed is the chosen completion already, whole.
    fn typed_is_chosen(&self) -> bool {
        let (Some(completion), Some(editor)) = (&self.completion, &self.editor) else {
            return false;
        };
        let Some(entry) = completion.candidates.get(completion.chosen) else {
            return false;
        };
        let typed = editor.text().get(entry.from..editor.cursor());
        entry.insert.is_none() && typed == Some(entry.candidate.text.as_str())
    }

    /// Puts the chosen completion into the text, as part of what is
    /// being typed.
    fn take_completion(&mut self) {
        let Some(completion) = self.completion.take() else {
            return;
        };
        let Some(entry) = completion.candidates.get(completion.chosen) else {
            return;
        };
        if let Some(insert) = entry.insert
            && self.insert_with_file(&insert, entry.from)
        {
            return;
        }
        let Some(editor) = &mut self.editor else {
            return;
        };
        let (text, cursor) = (editor.text(), editor.cursor());
        let typed = text
            .get(entry.from..cursor)
            .map_or(0, |typed| typed.chars().count());
        let (with, within) = match &entry.insert {
            // What stands by itself gets a paragraph of its own, where
            // it is put into the middle of one.
            Some(insert) => {
                let (before, behind) = crate::typst_text::around(text, entry.from, cursor);
                crate::typst_text::insert_text(insert, before, behind)
            }
            None => (entry.candidate.text.clone(), entry.candidate.cursor),
        };
        let press = editor.completed(typed, with, within);
        self.editor_key(press);
    }

    /// `K`: what Typst says the thing under the cursor is.
    pub(super) fn describe(&mut self) {
        let Some(editor) = &self.editor else {
            return;
        };
        let (source, cursor) = (editor.source().clone(), editor.cursor());
        self.project.set_source(source.clone());
        let world = self.project.snapshot();
        let document = self.document.as_deref();
        match zetwerk_typst::describe_at(&world, document, &source, cursor) {
            Some(text) => self.tooltip = Some(text),
            None => self.notice("Typst has nothing to say about this"),
        }
    }

    /// `gd`: to where the name under the cursor is defined.
    pub(super) fn goto_definition(&mut self) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let (source, cursor) = (editor.source().clone(), editor.cursor());
        self.project.set_source(source.clone());
        let world = self.project.snapshot();
        let document = self.document.as_deref();
        match zetwerk_typst::definition_of(&world, document, &source, cursor) {
            Some((file, range)) if file == editor.file() => {
                editor.jump(range.start);
                self.follow_cursor();
            }
            // In another file of the document: that file is opened.
            Some((file, range)) if self.project.path_of(file).is_some() => {
                self.open_file(file, Some(range.start));
            }
            Some((file, _)) => {
                let path = file.vpath().get_without_slash().to_owned();
                self.notice(format!("defined in {path}, a file of a package"));
            }
            None => self.notice("not defined in a file: built in, or no name"),
        }
    }

    /// Opens the list of the document's headings (`go`), or of its
    /// labels.
    pub(super) fn pick(&mut self, labels: bool) {
        let Ok(outline) = self.outline() else {
            return;
        };
        let Some(text) = self.text() else {
            return;
        };
        let start_of = |line: usize| {
            text.split_inclusive('\n')
                .take(line.saturating_sub(1))
                .map(str::len)
                .sum()
        };
        let items: Vec<(String, Target)> = if labels {
            let listed = |(name, line): &(String, usize)| {
                (format!("<{name}>"), Target::Offset(start_of(*line)))
            };
            outline.labels.iter().map(listed).collect()
        } else {
            let listed = |heading: &super::agent::Heading| {
                let indent = "  ".repeat(heading.level.saturating_sub(1));
                let place = Target::Offset(start_of(heading.line));
                (format!("{indent}{}", heading.text), place)
            };
            outline.headings.iter().map(listed).collect()
        };
        // The one the cursor is in or behind comes up chosen.
        let cursor = self
            .editor
            .as_ref()
            .map_or(0, crate::editor::Editor::cursor);
        let behind =
            |(_, target): &(String, Target)| matches!(target, Target::Offset(at) if *at <= cursor);
        let chosen = items.iter().rposition(behind).unwrap_or(0);
        let title = if labels { "labels" } else { "headings" };
        self.open_picker(title, items, chosen);
    }

    /// Where a file is to be named (the cursor is in the empty name of
    /// an image, a bibliography, a file to include), puts up the files
    /// there are to choose from: nobody knows their paths by heart.
    pub(super) fn offer_files(&mut self) {
        let Some(editor) = &self.editor else {
            return;
        };
        let (text, cursor) = (editor.text(), editor.cursor());
        let (Some(before), Some(behind)) = (text.get(..cursor), text.get(cursor..)) else {
            return;
        };
        let wanted = FILE_CALLS
            .iter()
            .find(|(call, _)| before.ends_with(call) && behind.starts_with('"'));
        let Some((_, endings)) = wanted else {
            return;
        };
        let put = |file: String| Target::Put {
            within: file.len(),
            text: file,
            typed: 0,
        };
        let items: Vec<(String, Target)> = self
            .files_beside(endings)
            .into_iter()
            .map(|file| (file.clone(), put(file)))
            .collect();
        if items.is_empty() {
            self.notice("no such files beside the document yet: type its path");
            return;
        }
        self.open_picker("files", items, 0);
    }

    /// The files in and below the folder of the file being edited whose
    /// names end in one of `endings`, as paths from there: as a call in
    /// that file names them.
    fn files_beside(&self, endings: &[&str]) -> Vec<String> {
        let Some(editor) = &self.editor else {
            return Vec::new();
        };
        let path = editor.path();
        let folder = match path.parent() {
            Some(folder) if !folder.as_os_str().is_empty() => folder,
            _ => std::path::Path::new("."),
        };
        let own = path.file_name().map(|name| name.to_string_lossy());
        let mut files = files_below(folder, endings);
        files.retain(|file| Some(file.as_str()) != own.as_deref());
        files
    }

    /// Where the thing of the insert menu at `entry` needs a file (a
    /// figure its image), puts up the files there are: the one chosen
    /// goes into the text with it, whole, and nothing before that.
    /// Returns whether it did.
    fn insert_with_file(&mut self, insert: &Insert, from: usize) -> bool {
        let Some(editor) = &self.editor else {
            return false;
        };
        let wanted = FILE_CALLS
            .iter()
            .find(|(call, _)| insert.body.contains(&format!("{call}|\"")));
        let Some((_, endings)) = wanted else {
            return false;
        };
        let (text, cursor) = (editor.text(), editor.cursor());
        let typed = text
            .get(from..cursor)
            .map_or(0, |typed| typed.chars().count());
        let (before, behind) = crate::typst_text::around(text, from, cursor);
        let (with, at) = crate::typst_text::insert_text(insert, before, behind);
        let rest = insert.body.split('|').nth(1).map_or(0, str::len);
        let items: Vec<(String, Target)> = self
            .files_beside(endings)
            .into_iter()
            .map(|file| {
                let mut text = with.clone();
                text.insert_str(at, &file);
                // On to what is to be written next: the caption.
                let named = at + file.len();
                let within = text[named..named + rest]
                    .find("[]")
                    .map_or(named + rest, |caption| named + caption + 1);
                let put = Target::Put {
                    text,
                    within,
                    typed,
                };
                (file, put)
            })
            .collect();
        if items.is_empty() {
            return false;
        }
        self.open_picker("files", items, 0);
        true
    }

    /// Puts up a list of places to choose from.
    pub(super) fn open_picker(
        &mut self,
        title: &'static str,
        items: Vec<(String, Target)>,
        chosen: usize,
    ) {
        if items.is_empty() {
            self.notice(format!("the document has no {title}"));
            return;
        }
        self.picker = Some(Picker {
            title,
            shown: (0..items.len()).collect(),
            items,
            query: String::new(),
            chosen,
        });
    }

    fn picker_key(&mut self, press: KeyPress) {
        let Some(picker) = &mut self.picker else {
            return;
        };
        let count = picker.shown.len().max(1);
        match (press.key, press.ctrl) {
            (Key::Escape, _) | (Key::Char('c' | '['), true) => self.picker = None,
            (Key::Enter, _) => {
                let chosen = picker.shown.get(picker.chosen);
                let target = chosen.map(|item| picker.items[*item].1.clone());
                self.picker = None;
                match (target, &mut self.editor) {
                    (Some(Target::Offset(at)), Some(editor)) => {
                        editor.jump(at);
                        self.follow_cursor();
                    }
                    (Some(Target::File(path)), _) => self.open_path(&path, false),
                    (
                        Some(Target::Put {
                            text,
                            within,
                            typed,
                        }),
                        Some(editor),
                    ) => {
                        let press = editor.completed(typed, text, within);
                        self.editor_key(press);
                    }
                    _ => {}
                }
            }
            (Key::Down, _) | (Key::Char('n'), true) => {
                picker.chosen = (picker.chosen + 1) % count;
            }
            (Key::Up, _) | (Key::Char('p'), true) => {
                picker.chosen = (picker.chosen + count - 1) % count;
            }
            (Key::Backspace, _) => {
                picker.query.pop();
                picker.narrow();
            }
            (Key::Char(c), false) => {
                picker.query.push(c);
                picker.narrow();
            }
            _ => {}
        }
    }

    /// The lists and explanations on screen, over everything else.
    pub(super) fn assist_panels(&mut self, scene: &mut Scene, has: &dyn Fn(TextureId) -> bool) {
        let (width, height) = (f64::from(self.window.0), f64::from(self.window.1));
        let margin = super::PANEL_MARGIN * self.scale;
        let bottom = height - f64::from(self.status_height()) - margin;
        // Under the cursor's line; above it where there is no room.
        // Under the cursor's line; or, where there is no room, above
        // what is being typed, not over it.
        let anchor = match (self.caret_at, self.cursor_at) {
            (Some(caret), Some(site)) => Some((caret.0, site.1.min(caret.1), caret.2)),
            (caret, site) => caret.or(site),
        };
        let near = move |size: (f64, f64)| {
            let within = (margin, margin, width - margin, bottom);
            super::lens_place(anchor, size, within, 2.0)
        };
        if let Some(completion) = &self.completion {
            let first = completion
                .chosen
                .saturating_sub(LIST_ROWS / 2)
                .min(completion.candidates.len().saturating_sub(LIST_ROWS));
            let rows: Vec<(String, String)> = completion
                .candidates
                .iter()
                .skip(first)
                .take(LIST_ROWS)
                .map(|entry| {
                    let candidate = &entry.candidate;
                    (cut(&candidate.label, 40), candidate.kind.to_owned())
                })
                .collect();
            let detail = completion
                .candidates
                .get(completion.chosen)
                .and_then(|entry| entry.candidate.detail.as_deref())
                .map(|detail| cut(detail, 70));
            let markup =
                chrome::list_markup(None, &rows, completion.chosen - first, detail.as_deref());
            self.panel(scene, has, &markup, near);
        }
        if let Some(text) = &self.tooltip {
            let mut lines: Vec<String> = text
                .lines()
                .take(TOOLTIP_LINES)
                .map(|line| cut(line, TOOLTIP_COLUMNS))
                .collect();
            if text.lines().count() > lines.len() {
                lines.push("…".to_owned());
            }
            let markup = chrome::text_markup(&lines);
            self.panel(scene, has, &markup, near);
        }
        if let Some(picker) = &self.picker {
            let first = picker
                .chosen
                .saturating_sub(LIST_ROWS / 2)
                .min(picker.shown.len().saturating_sub(LIST_ROWS));
            let rows: Vec<(String, String)> = picker
                .shown
                .iter()
                .skip(first)
                .take(LIST_ROWS)
                .map(|item| (cut(&picker.items[*item].0, 70), String::new()))
                .collect();
            let title = format!("{}: {}\u{2588}", picker.title, picker.query);
            let chosen = picker.chosen.saturating_sub(first);
            let markup = chrome::list_markup(Some(&title), &rows, chosen, None);
            // In the middle, a third of the way down.
            self.panel(scene, has, &markup, |(panel_width, _)| {
                ((width - panel_width) / 2.0, (height / 4.0).max(margin))
            });
        }
    }

    /// Whether keys are for a list of places and not for the text.
    #[must_use]
    pub fn is_picking(&self) -> bool {
        self.picker.is_some()
    }
}

impl Picker {
    /// Narrows the list to what the query is found in, best first.
    fn narrow(&mut self) {
        let mut ranked: Vec<(i32, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, (label, _))| Some((score(label, &self.query)?, index)))
            .collect();
        ranked.sort_by_key(|(score, index)| (std::cmp::Reverse(*score), *index));
        self.shown = ranked.into_iter().map(|(_, index)| index).collect();
        self.chosen = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_is_found_in_order_and_ranked() {
        assert!(score("Introduction", "intro").is_some());
        assert!(score("Introduction", "itn").is_some());
        assert_eq!(score("Introduction", "x"), None);
        assert_eq!(score("Introduction", "ooo"), None);
        assert_eq!(score("Proof", "fp"), None);
        // The start of a word and letters in a row count for more.
        assert!(score("Main result", "res") > score("Preserved", "res"));
        assert!(score("Proof", "pro") > score("Approximation", "pro"));
        assert!(score("anything", "").is_some());
    }
}
