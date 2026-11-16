//! Editing the source text.
//!
//! An [`Editor`] is the text of one file as Typst parses it, the history
//! and journal that keep it safe ([`zetwerk_core::Document`]), and the Vim
//! that changes it ([`zetwerk_vim::Engine`]). Like the rest of the viewer it
//! knows nothing about windows: keys in, text and a cursor out.

use std::ops::Range;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use typst::syntax::{FileId, Source};
use zetwerk_core::{
    Buffer, DiskError, DiskStatus, Document, History, OpenError, Opened, Replace, StateId,
};
use zetwerk_vim::{Edit, Effect, Engine, Host, Place, Reading, Seek, Shown as ShownLine};

use crate::keys::{Key, KeyPress};

pub use zetwerk_vim::Mode;

/// How many cells of the source pane a tab takes.
pub const TAB_CELLS: usize = 4;

/// The text of a file, kept as the Typst source it is compiled from. An
/// edit reparses only what it touched.
pub struct SourceBuffer(Source);

impl Buffer for SourceBuffer {
    fn text(&self) -> &str {
        self.0.text()
    }

    fn replace(&mut self, range: Range<usize>, with: &str) {
        self.0.edit(range, with);
    }
}

/// One file being edited.
pub struct Editor {
    document: Document<SourceBuffer>,
    engine: Engine,
    /// Why the last edit did not happen, until someone asks.
    failure: Option<String>,
    /// The rows the text is shown in, for the text and pane width they
    /// were worked out for.
    rows: Option<(u64, usize, Vec<Row>)>,
    /// The first row shown in the source pane.
    top: usize,
    /// What the last key changed by walking the history (see
    /// [`Editor::stepped`]), and the revision of the text it is true of.
    stepped: Option<(u64, Vec<Range<usize>>)>,
    /// What the text is read as, for searching it on the page.
    read: crate::reading::Cache,
}

/// Something that shows the text in rows other than the source pane's:
/// the lines of typeset pages.
pub trait Rows {
    /// See [`zetwerk_vim::Host::row`].
    fn row(&self, from: usize, rows: isize, across: Option<f64>) -> Option<(usize, f64)>;

    /// See [`zetwerk_vim::Host::stop`]. `Err` if byte `at` is in text
    /// that is newer than what is shown.
    fn stop(&self, at: usize, seek: Seek) -> Result<Option<Range<usize>>, Unseen>;

    /// See [`zetwerk_vim::Host::shown_line`].
    fn line(&self, at: usize) -> Option<ShownLine>;
}

/// The place asked about is in text that nothing shows yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unseen;

/// What the cursor moves on when it moves by rows.
pub enum Shown<'a> {
    /// The source pane, whose rows are `columns` cells wide.
    Source { columns: usize },
    /// Typeset pages; `None` while they are not those of the text as it
    /// is.
    Pages(Option<&'a dyn Rows>),
}

/// What a key did.
pub struct Typed {
    /// Commands for the window: what was entered on the command line.
    pub effects: Vec<Effect>,
    /// Whether the text changed.
    pub changed: bool,
}

/// One row of the source pane: a line of the text, or part of one that is
/// too long for the pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The line, counting from 0.
    pub line: usize,
    /// The bytes of the text shown in this row, without the line break.
    pub range: Range<usize>,
    /// Whether this is the first row of its line.
    pub first: bool,
}

impl Editor {
    /// Opens the file at `path`, whose Typst file id is `id`, for editing.
    /// Journals are kept under `state_dir`.
    pub fn open(path: &Path, state_dir: &Path, id: FileId) -> Result<(Self, Opened), OpenError> {
        let (document, opened) =
            Document::open(path, state_dir, |text| SourceBuffer(Source::new(id, text)))?;
        let mut engine = Engine::new();
        engine.set_pairs(true);
        let editor = Self {
            document,
            engine,
            failure: None,
            rows: None,
            top: 0,
            stepped: None,
            read: crate::reading::Cache::default(),
        };
        Ok((editor, opened))
    }

    /// Handles a key press. `shown` is what the user sees the text on.
    pub fn key(&mut self, press: KeyPress, shown: &Shown) -> Typed {
        let before = self.document.revision();
        let typing = self.engine.mode() == Mode::Insert;
        let history = (self.document.state(), self.document.history().len());
        let here = matches!(shown, Shown::Pages(_))
            .then(|| self.equation_here())
            .flatten();
        let inside = matches!(shown, Shown::Pages(_))
            .then(|| self.equation_inside())
            .flatten();
        let cursor = self.engine.cursor();
        // Only pages take the place of lines for `j` and `k`: in the
        // source, a line of the text is what they have always gone by.
        self.engine.set_rows_first(matches!(shown, Shown::Pages(_)));
        let rows = match shown {
            Shown::Pages(pages) => RowsOf::Pages(*pages),
            Shown::Source { columns } => {
                self.rows(*columns);
                match &self.rows {
                    Some((_, _, rows)) => RowsOf::Source(rows),
                    None => RowsOf::Pages(None),
                }
            }
        };
        let mut host = DocumentHost {
            document: &mut self.document,
            failure: &mut self.failure,
            rows,
            shown_revision: before,
            read: &self.read,
            on_page: matches!(shown, Shown::Pages(_)),
        };
        let length = host.document.text().len();
        let effects = self.engine.key(press, &mut host);
        self.stepped = self.stepped_since(history);
        if self.failure.is_some() {
            // The engine believes in an edit that was not made. The text is
            // what it was; all that can be off is the cursor.
            self.engine
                .set_cursor(self.engine.cursor(), self.document.text());
        } else if matches!(shown, Shown::Pages(_)) && self.stepped.is_none() {
            // Not after an undo or a redo, which bring back what was
            // there: nothing is tidied away from that or made of it, or
            // redo would lead nowhere.
            if self.document.text().len() < length {
                self.tidy_equation();
            }
            if !typing && self.document.revision() != before {
                self.part_put();
            }
            let left = typing && self.engine.mode() != Mode::Insert;
            if left {
                self.clear_empty_text(here);
            }
            if self.document.text().len() < length || left {
                // A deletion from one equation into those behind it
                // empties them all: the next is where the one before
                // was.
                for _ in 0..TIDY_ROUNDS {
                    let before = self.document.text().len();
                    self.clear_empty_equation(here);
                    if self.document.text().len() == before {
                        break;
                    }
                }
            }
            // The equation typing took the cursor out of: what was typed
            // last in it is over.
            let out = inside.filter(|start| typing && self.equation_inside() != Some(*start));
            if let Some(start) = out {
                self.spell_out_names(Some(start));
            }
            // So is a word the cursor moved off while typing.
            let moved = typing && inside.is_some() && self.engine.cursor() != cursor;
            if self.document.revision() != before
                || typing != (self.engine.mode() == Mode::Insert)
                || moved
            {
                self.spell_out_names(None);
            }
        }
        Typed {
            effects,
            changed: self.document.revision() != before,
        }
    }

    /// After a key on a page: letters typed together in the equation the
    /// cursor is in that name nothing become the letters they are, each
    /// by itself (`xy` becomes `x y`). Typst reads them as one name and
    /// refuses the equation for it; whoever types them means a product.
    /// The word still being typed is left: it may become `alpha`. With
    /// `left`, in the equation that begins there, which the cursor left.
    fn spell_out_names(&mut self, left: Option<usize>) {
        let typing = left.is_none() && self.engine.mode() == Mode::Insert;
        let source = &self.document.buffer().0;
        let mut cursor = self.engine.cursor();
        let at = left.map_or(cursor.saturating_sub(1), |start| start + 1);
        let Some(equation) = crate::typst_text::equation_at(source, at)
            .filter(|equation| left.is_none_or(|start| equation.start == start))
        else {
            return;
        };
        // Only in an equation that is one. Where a `$` is missing or
        // one too many, Typst reads the text between equations as
        // equations, and the words there are no products of letters.
        if !crate::typst_text::is_whole_equation(source, &equation) {
            return;
        }
        let names = crate::typst_text::unknown_names(source, &equation, typing.then_some(cursor));
        if names.is_empty() {
            return;
        }
        let text = source.text();
        let mut replaces = Vec::new();
        // From the last: the bytes before it stay where they are.
        for name in names.into_iter().rev() {
            let Some(word) = text.get(name.clone()) else {
                continue;
            };
            let spelled = crate::typst_text::spelled_out(word);
            if name.end <= cursor {
                cursor += spelled.len() - word.len();
            } else if name.start < cursor {
                // Before the same letter as before.
                let letters = word[..cursor - name.start].chars().count();
                let mut shown = spelled.char_indices().filter(|(_, c)| *c != ' ');
                cursor = name.start + shown.nth(letters).map_or(0, |(at, _)| at);
            }
            replaces.push(Replace::new(name, spelled.as_str()));
        }
        if let Err(err) = self.document.edit(&replaces, true) {
            self.failure = Some(err.to_string());
            return;
        }
        self.engine.set_cursor(cursor, self.document.text());
    }

    /// After something was deleted on a page: takes from the equation
    /// the cursor is in the marks that raise, lower or divide by nothing
    /// now. They show as nothing, so nobody could delete them there; left
    /// in, they are no equation. The one being typed into stays.
    fn tidy_equation(&mut self) {
        let typing = self.engine.mode() == Mode::Insert;
        for _ in 0..TIDY_ROUNDS {
            let source = &self.document.buffer().0;
            let cursor = self.engine.cursor();
            // The one the cursor is in, or has just come out of at its
            // end: what was taken may have been the last thing in it.
            let behind = cursor
                .checked_sub(1)
                .and_then(|before| crate::typst_text::equation_at(source, before));
            let Some(equation) = crate::typst_text::equation_at(source, cursor).or(behind) else {
                return;
            };
            if cursor <= equation.start || !crate::typst_text::is_whole_equation(source, &equation)
            {
                return;
            }
            let mut left = crate::typst_text::empty_scripts(source, &equation);
            left.retain(|script| !(typing && script.start < cursor && cursor <= script.end));
            if left.is_empty() {
                return;
            }
            let mut cursor = cursor;
            let mut replaces = Vec::new();
            // From the last: the bytes before it stay where they are.
            for script in left.into_iter().rev() {
                if script.end <= cursor {
                    cursor -= script.len();
                } else if script.start < cursor {
                    cursor = script.start;
                }
                replaces.push(Replace::new(script, ""));
            }
            if let Err(err) = self.document.edit(&replaces, true) {
                self.failure = Some(err.to_string());
                return;
            }
            self.engine.set_cursor(cursor, self.document.text());
        }
    }

    /// After `p` or `P` on a page into an equation: what was put is
    /// parted by a blank from a letter it stands right against, or the
    /// two would be one name to Typst (`alpha` and `x`, `alphax`), and
    /// spelled out letter by letter.
    fn part_put(&mut self) {
        let Some(zetwerk_vim::Took::Put(range)) = self.engine.took().cloned() else {
            return;
        };
        let source = &self.document.buffer().0;
        let text = source.text();
        let within = crate::typst_text::equation_at(source, range.start)
            .is_some_and(|equation| equation.start < range.start && range.end < equation.end);
        let (Some(before), Some(behind)) = (text.get(..range.start), text.get(range.end..)) else {
            return;
        };
        if !within || range.is_empty() {
            return;
        }
        let letter = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
        let put = &text[range.clone()];
        let mut cursor = self.engine.cursor();
        let mut replaces = Vec::new();
        // From the last: the bytes before it stay where they are.
        if letter(put.chars().next_back()) && letter(behind.chars().next()) {
            replaces.push(Replace::new(range.end..range.end, " "));
            cursor += usize::from(cursor >= range.end);
        }
        if letter(before.chars().next_back()) && letter(put.chars().next()) {
            replaces.push(Replace::new(range.start..range.start, " "));
            cursor += usize::from(cursor >= range.start);
        }
        if replaces.is_empty() {
            return;
        }
        if let Err(err) = self.document.edit(&replaces, true) {
            self.failure = Some(err.to_string());
            return;
        }
        self.engine.set_cursor(cursor, self.document.text());
    }

    /// When typing is over: text begun in the equation the cursor was in
    /// (`here`, see [`Editor::equation_here`]) and left with nothing in it,
    /// `""`, which shows as nothing, goes.
    fn clear_empty_text(&mut self, here: Option<(usize, bool)>) {
        let Some((start, _)) = here else {
            return;
        };
        let source = &self.document.buffer().0;
        let Some(equation) = crate::typst_text::equation_at(source, start + 1)
            .filter(|equation| equation.start == start)
            .filter(|equation| crate::typst_text::is_whole_equation(source, equation))
        else {
            return;
        };
        let text = source.text();
        let mut cursor = self.engine.cursor();
        let mut replaces = Vec::new();
        // From the last: the bytes before it stay where they are.
        for empty in crate::typst_text::empty_scripts(source, &equation)
            .into_iter()
            .rev()
        {
            if text.get(empty.clone()) != Some("\"\"") {
                continue;
            }
            if empty.end <= cursor {
                cursor -= empty.len();
            } else if empty.start < cursor {
                cursor = empty.start;
            }
            replaces.push(Replace::new(empty, ""));
        }
        if replaces.is_empty() {
            return;
        }
        if let Err(err) = self.document.edit(&replaces, true) {
            self.failure = Some(err.to_string());
            return;
        }
        self.engine.set_cursor(cursor, self.document.text());
    }

    /// Where the equation the cursor is in begins, if it is in one.
    fn equation_inside(&self) -> Option<usize> {
        let source = &self.document.buffer().0;
        let cursor = self.engine.cursor();
        crate::typst_text::equation_at(source, cursor)
            .filter(|equation| equation.start < cursor)
            .map(|equation| equation.start)
    }

    /// Where the equation the cursor is in (or is just behind) begins,
    /// and whether it stands in its line: with no blank on one side of
    /// what is in it.
    fn equation_here(&self) -> Option<(usize, bool)> {
        let source = &self.document.buffer().0;
        let cursor = self.engine.cursor();
        let equation = crate::typst_text::equation_at(source, cursor)
            .or_else(|| crate::typst_text::equation_at(source, cursor.saturating_sub(1)))?;
        let body = source
            .text()
            .get(equation.start + 1..equation.end.saturating_sub(1))?;
        let alone = body.starts_with(char::is_whitespace) && body.ends_with(char::is_whitespace);
        Some((equation.start, !alone))
    }

    /// After a deletion on a page, or when typing is over: the equation
    /// the cursor is in (or the one it was in before the key, `here`, see
    /// [`Editor::equation_here`]), with nothing left in it but blanks.
    /// While it is typed into, one that stood in its line is kept there
    /// (with blanks on both sides it would stand by itself). Else it
    /// goes, dollar signs and all: they show as nothing, and would stay
    /// in the text unseen. One with a label stays, for what refers to it,
    /// and so does one that is an argument of a call.
    fn clear_empty_equation(&mut self, here: Option<(usize, bool)>) {
        let typing = self.engine.mode() == Mode::Insert;
        let source = &self.document.buffer().0;
        let text = source.text();
        let blank = |equation: &Range<usize>| {
            text.get(equation.start + 1..equation.end.saturating_sub(1))
                .is_some_and(|body| body.trim().is_empty())
        };
        // After Escape the cursor rests on what is shown, which an empty
        // equation is not: the one it was in is looked for too.
        let was = here.and_then(|(start, _)| {
            crate::typst_text::equation_at(source, start + 1).filter(|found| found.start == start)
        });
        let Some(equation) = crate::typst_text::equation_at(source, self.engine.cursor())
            .into_iter()
            .chain(was)
            .find(blank)
        else {
            return;
        };
        if !crate::typst_text::is_whole_equation(source, &equation) {
            return;
        }
        let inside = equation.start + 1..equation.end.saturating_sub(1);
        let in_line = here.is_some_and(|(start, in_line)| start == equation.start && in_line);
        let labelled = text[equation.end..]
            .trim_start_matches(' ')
            .starts_with('<');
        // One given to a function (a numbered equation, a figure) stays
        // too: the function would lack it.
        let given =
            crate::typst_text::ground(source, equation.start) != crate::typst_text::Ground::Markup;
        let (range, cursor) = if typing {
            (inside, equation.start + 1)
        } else if labelled || given {
            return;
        } else {
            // With what set it apart: the line breaks of a line it stood
            // on alone (a paragraph's end once), or a blank beside it.
            let (before, after) = (&text[..equation.start], &text[equation.end..]);
            let breaks = after.len() - after.trim_start_matches('\n').len();
            let apart = if before.is_empty() || before.ends_with("\n\n") {
                breaks
            } else if before.ends_with('\n') {
                breaks.min(1)
            } else {
                usize::from(before.ends_with(' ') && after.starts_with(' '))
            };
            (equation.start..equation.end + apart, equation.start)
        };
        if range.is_empty() || (typing && !in_line) {
            return;
        }
        if let Err(err) = self.document.edit(&[Replace::new(range, "")], true) {
            self.failure = Some(err.to_string());
            return;
        }
        self.engine.set_cursor(cursor, self.document.text());
    }

    /// Changes the text without keys, as an agent does: each of `edits`
    /// replaces a range of bytes by a text, the ranges referring to the
    /// text as the edits before them left it. All of them are one undo
    /// step. Nothing is changed if one of them cannot be made.
    pub fn replace(&mut self, edits: &[(Range<usize>, &str)]) -> Result<(), String> {
        // Tried on a copy of the ranges first: the engine has no way to
        // say no.
        let mut length = self.text().len();
        let mut text = std::borrow::Cow::Borrowed(self.text());
        for (range, with) in edits {
            let fits = range.start <= range.end
                && range.end <= length
                && text.is_char_boundary(range.start)
                && text.is_char_boundary(range.end);
            if !fits {
                return Err(format!(
                    "bytes {}..{} are not a part of the text",
                    range.start, range.end
                ));
            }
            if edits.len() > 1 {
                text.to_mut().replace_range(range.clone(), with);
            }
            length = length - range.len() + with.len();
        }
        drop(text);
        let before = self.document.revision();
        let mut host = DocumentHost {
            document: &mut self.document,
            failure: &mut self.failure,
            rows: RowsOf::Pages(None),
            shown_revision: before,
            read: &self.read,
            on_page: false,
        };
        self.engine.replace(&mut host, edits);
        match self.failure.take() {
            None => Ok(()),
            Some(failure) => {
                self.engine
                    .set_cursor(self.engine.cursor(), self.document.text());
                Err(failure)
            }
        }
    }

    /// The key that, given to [`Editor::key`] in insert mode, puts `text`
    /// in place of the `replaced` characters before the cursor, with the
    /// cursor `within` bytes into it. See [`Engine::completed`].
    pub fn completed(&mut self, replaced: usize, text: String, within: usize) -> KeyPress {
        self.engine.completed(replaced, text, within)
    }

    /// Puts the cursor at byte `offset` as a jump: `` ` ` `` leads back.
    pub fn jump(&mut self, offset: usize) {
        self.engine.jump(offset, self.document.text());
    }

    /// The file this is the text of, as Typst names it and as the disk
    /// does.
    #[must_use]
    pub fn file(&self) -> FileId {
        self.document.buffer().0.id()
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        self.document.path()
    }

    /// Takes over from the editor of another file what belongs to the
    /// user and not to a file: registers, the last search, what `.`
    /// repeats.
    pub fn share(&mut self, other: &Self) {
        self.engine.share(&other.engine);
    }

    /// The Vim that edits the text, to ask what it did.
    #[must_use]
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Takes back what the Vim of a text remembered (see
    /// [`Engine::recall`]), as a replay begins.
    pub fn recall(&mut self, memory: &str) {
        self.engine.recall(memory);
    }

    /// What the text is read as on a page (see [`crate::reading`]).
    #[must_use]
    pub fn reading(&self) -> Arc<Reading> {
        self.read
            .get(self.document.revision(), &self.document.buffer().0)
    }

    /// Whether no command is under way; see [`Engine::is_idle`].
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.engine.is_idle()
    }

    /// Makes register `name` hold `text`: whole lines if it ends with a
    /// line break. See [`Engine::set_register`].
    pub fn set_register(&mut self, name: char, text: &str) {
        self.engine.set_register(name, text);
    }

    /// Whether the next key is part of a command begun; see
    /// [`Engine::awaits_part`].
    #[must_use]
    pub fn awaits_part(&self) -> bool {
        self.engine.awaits_part()
    }

    /// What can follow the keys typed so far; see [`Engine::following`].
    #[must_use]
    pub fn following(&self) -> Option<zetwerk_vim::Following> {
        self.engine.following()
    }

    /// Counts the changes made to the text.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.document.revision()
    }

    #[must_use]
    pub fn text(&self) -> &str {
        self.document.text()
    }

    /// The text as Typst source, to compile.
    #[must_use]
    pub fn source(&self) -> &Source {
        &self.document.buffer().0
    }

    /// The cursor, as a byte offset into the text.
    #[must_use]
    pub fn cursor(&self) -> usize {
        self.engine.cursor().min(self.text().len())
    }

    /// The cursor as line and column, both counting from 1; the column
    /// counts characters.
    #[must_use]
    pub fn position(&self) -> (usize, usize) {
        let before = &self.text()[..floor_boundary(self.text(), self.cursor())];
        let line_start = before.rfind('\n').map_or(0, |at| at + 1);
        // Asked for every frame (the status line), of a text that may be
        // a book: counted byte by byte, which goes many at a time.
        let lines = before.bytes().filter(|&byte| byte == b'\n').count();
        (lines + 1, before[line_start..].chars().count() + 1)
    }

    #[must_use]
    pub fn mode(&self) -> Mode {
        // Lines selected on a page are a selection of characters to the
        // engine; to the user they are lines.
        if self.engine.selects_lines() {
            return Mode::VisualLine;
        }
        self.engine.mode()
    }

    /// The selected bytes, in visual mode.
    #[must_use]
    pub fn selection(&self) -> Option<Range<usize>> {
        self.engine.selection(self.text())
    }

    /// The text on the command line, while it is open.
    #[must_use]
    pub fn command_line(&self) -> Option<&str> {
        self.engine.command_line()
    }

    /// What the command line is for, while it is open: `:` for a command,
    /// `/` or `?` for a search.
    #[must_use]
    pub fn prompt(&self) -> Option<char> {
        self.engine.prompt()
    }

    /// Scrolls a pane of `visible` rows, `columns` cells wide, so that the
    /// cursor's row is at `place` in it.
    pub fn scroll_to(&mut self, place: Place, columns: usize, visible: usize) {
        let cursor = self.cursor();
        let row = row_of(self.rows(columns), cursor);
        self.top = match place {
            Place::Top => row,
            Place::Middle => row.saturating_sub(visible / 2),
            Place::Bottom => (row + 1).saturating_sub(visible),
        };
    }

    /// Tells the editor which rows the pane shows: `visible` of them, in
    /// lines `columns` cells wide. `H`, `M`, `L` and the page keys go by it.
    pub fn set_view(&mut self, columns: usize, visible: usize) {
        let top = self.top;
        let rows = self.rows(columns);
        let line = |row: usize| {
            rows.get(row.min(rows.len().saturating_sub(1)))
                .map(|row| row.line)
        };
        let (first, last) = (line(top), line(top + visible.saturating_sub(1)));
        if let (Some(first), Some(last)) = (first, last) {
            self.engine.set_view_lines(first, last);
        }
        self.engine.set_page_lines(visible.saturating_sub(2));
    }

    /// Puts the cursor at byte `offset` of the text, or as near as it may
    /// be.
    pub fn set_cursor(&mut self, offset: usize) {
        self.engine.set_cursor(offset, self.document.text());
    }

    /// Runs `ask` with the engine and with the text as the engine sees
    /// it when `shown` is what the user sees the text on, without a key.
    fn with_host<T>(
        &mut self,
        shown: &Shown,
        ask: impl FnOnce(&mut Engine, &DocumentHost) -> T,
    ) -> T {
        let shown_revision = self.document.revision();
        self.engine.set_rows_first(matches!(shown, Shown::Pages(_)));
        let rows = match shown {
            Shown::Pages(pages) => RowsOf::Pages(*pages),
            Shown::Source { columns } => {
                self.rows(*columns);
                match &self.rows {
                    Some((_, _, rows)) => RowsOf::Source(rows),
                    None => RowsOf::Pages(None),
                }
            }
        };
        let host = DocumentHost {
            document: &mut self.document,
            failure: &mut self.failure,
            rows,
            shown_revision,
            read: &self.read,
            on_page: false,
        };
        ask(&mut self.engine, &host)
    }

    /// Puts the cursor at byte `offset` as a pointer does: a selection
    /// ends, typing goes on. See [`Engine::point`]. Returns whether the
    /// cursor was put there: it is not on the command line.
    pub fn point(&mut self, offset: usize) -> bool {
        self.engine.point(offset, self.document.text())
    }

    /// Selects from byte `anchor` to byte `cursor`, both included, or
    /// with `lines` the lines of the text they are in, as a pointer
    /// does. See [`Engine::select`]. Returns whether that was done: it
    /// is not while typing.
    pub fn select(&mut self, anchor: usize, cursor: usize, lines: bool, shown: &Shown) -> bool {
        self.with_host(shown, |engine, host| {
            engine.select(anchor, cursor, lines, host)
        })
    }

    /// The end of the selection that the cursor is not at, if there is a
    /// selection.
    #[must_use]
    pub fn anchor(&self) -> Option<usize> {
        self.engine.anchor()
    }

    /// The byte of the text a pointer is on at `cell` of a row of a pane
    /// `columns` cells wide. `row` counts from the first row the pane
    /// shows and may be negative, for a row above it. Past the end of a
    /// row that is the end of its line; where the line goes on in the
    /// next row, the row's last character, which is in this row still.
    pub fn offset_near(&mut self, columns: usize, row: isize, cell: usize) -> usize {
        let row = self.top.saturating_add_signed(row);
        let rows = self.rows(columns);
        let row = row.min(rows.len().saturating_sub(1));
        let Some(range) = rows.get(row).map(|row| row.range.clone()) else {
            return 0;
        };
        let goes_on = rows.get(row + 1).is_some_and(|next| !next.first);
        let mut cells_before = 0;
        let mut last = range.start;
        for (at, c) in self.text()[range.clone()].char_indices() {
            cells_before += cells_of(c);
            if cells_before > cell {
                return range.start + at;
            }
            last = range.start + at;
        }
        if goes_on { last } else { range.end }
    }

    /// The byte of the text shown at `cell` of row `row` of a pane
    /// `columns` cells wide; past the end of a row, the row's end.
    pub fn offset_at(&mut self, columns: usize, row: usize, cell: usize) -> usize {
        let row = self.top + row;
        let rows = self.rows(columns);
        let Some(range) = rows
            .get(row.min(rows.len().saturating_sub(1)))
            .map(|row| row.range.clone())
        else {
            return 0;
        };
        let text = &self.text()[range.clone()];
        let mut cells_before = 0;
        for (at, c) in text.char_indices() {
            cells_before += cells_of(c);
            if cells_before > cell {
                return range.start + at;
            }
        }
        range.end
    }

    /// Whether typing replaces the characters under the cursor (`R`).
    #[must_use]
    pub fn replacing(&self) -> bool {
        self.engine.replacing()
    }

    /// The register a macro is being recorded into, if one is.
    #[must_use]
    pub fn recording(&self) -> Option<char> {
        self.engine.recording()
    }

    /// Whether the text differs from the file.
    #[must_use]
    pub fn is_modified(&self) -> bool {
        self.document.is_modified()
    }

    /// When the unsaved changes brought back from the journal on opening
    /// were last made, until the text is saved or loaded; see
    /// [`Document::restored`].
    #[must_use]
    pub fn restored(&self) -> Option<std::time::SystemTime> {
        self.document.restored()
    }

    /// Why the last edit was refused, if one was. Asking forgets it.
    pub fn take_failure(&mut self) -> Option<String> {
        self.failure.take()
    }

    /// Writes the text to the file. See [`Document::save`].
    pub fn save(&mut self, overwrite: bool) -> Result<(), DiskError> {
        self.document.save(overwrite)
    }

    /// Replaces the text with the file's, as one undo step.
    pub fn reload(&mut self) -> Result<(), DiskError> {
        self.document.reload()?;
        self.engine
            .set_cursor(self.engine.cursor(), self.document.text());
        Ok(())
    }

    /// How the file relates to what was last read or written.
    pub fn disk_status(&self) -> Result<DiskStatus, DiskError> {
        self.document.disk_status()
    }

    /// Forces the journal onto the disk.
    pub fn sync(&mut self) -> std::io::Result<()> {
        self.document.sync()
    }

    /// Since when there are changes a power failure would lose.
    #[must_use]
    pub fn unsynced_since(&self) -> Option<Instant> {
        self.document.unsynced_since()
    }

    /// The undo history of the text: every state it has been in.
    #[must_use]
    pub fn history(&self) -> &History {
        self.document.history()
    }

    /// The text as it was in state `state` of the history; the text and
    /// its history stay as they are. See [`Document::text_at`].
    #[must_use]
    pub fn text_at(&self, state: StateId) -> Option<String> {
        self.document.text_at(state)
    }

    /// The stretches of the text that the last key made different by
    /// undoing or redoing, in the order of the text: what came back, and
    /// as empty stretches the places text went from (see
    /// [`zetwerk_core::Step::ranges`]). Of several steps taken at once,
    /// all they changed. `None` once another key was pressed or the text
    /// was changed in any other way.
    #[must_use]
    pub fn stepped(&self) -> Option<&[Range<usize>]> {
        let (revision, ranges) = self.stepped.as_ref()?;
        (*revision == self.document.revision()).then_some(ranges.as_slice())
    }

    /// What walking the history changed since it was at state `from`,
    /// when it had `known` states: `None` if the text is in the same
    /// state still, or in one that was made since (that is a change, not
    /// a walk).
    fn stepped_since(&self, (from, known): (StateId, usize)) -> Option<(u64, Vec<Range<usize>>)> {
        let history = self.document.history();
        let to = history.current();
        if to == from || usize::try_from(to).is_ok_and(|to| to >= known) {
            return None;
        }
        let mut ranges = Vec::new();
        for step in history.steps(from, to)? {
            ranges = step.ranges_with(ranges);
        }
        Some((self.document.revision(), ranges))
    }

    /// Goes to state `to` of the history the way the user would get
    /// there: by undoing as far as the last state the two have in common
    /// and redoing from there, as many steps as it takes. So the way
    /// back is `u` and Ctrl-R as after any undo, and nothing is done to
    /// the history that those keys could not do, but for this: where
    /// `to` is on a branch that redo has left, redo is made to lead down
    /// that branch (see [`Document::prefer`]).
    pub fn travel(&mut self, to: StateId) -> Result<(), String> {
        let history = self.document.history();
        let from = (history.current(), history.len());
        let path = history.path(from.0, to).ok_or("there is no such step")?;
        if self.engine.mode() != Mode::Normal || !self.engine.is_idle() {
            return Err("in the middle of something else".to_owned());
        }
        // The way leads up to the common state first, then down.
        let mut at = from.0;
        let up = path
            .iter()
            .take_while(|next| {
                let is_up = history.parent(at) == Some(**next);
                at = **next;
                is_up
            })
            .count();
        let down = &path[up..];
        self.press_times(up, KeyPress::plain(Key::Char('u')));
        for state in down {
            if self.failure.is_some() {
                break;
            }
            if let Err(err) = self.document.prefer(*state) {
                self.failure = Some(err.to_string());
            }
        }
        if self.failure.is_none() {
            let redo = KeyPress {
                ctrl: true,
                ..KeyPress::plain(Key::Char('r'))
            };
            self.press_times(down.len(), redo);
        }
        self.stepped = self.stepped_since(from);
        self.engine
            .set_cursor(self.engine.cursor(), self.document.text());
        match self.failure.take() {
            Some(failure) => Err(failure),
            None if self.document.state() == to => Ok(()),
            None => Err("the history does not lead there".to_owned()),
        }
    }

    /// Gives the engine `press` with the count `times`, as typed in
    /// normal mode; nothing if `times` is 0.
    fn press_times(&mut self, times: usize, press: KeyPress) {
        if times == 0 {
            return;
        }
        let count = if times > 1 {
            times.to_string()
        } else {
            String::new()
        };
        let keys = count.chars().map(|digit| KeyPress::plain(Key::Char(digit)));
        let shown_revision = self.document.revision();
        let mut host = DocumentHost {
            document: &mut self.document,
            failure: &mut self.failure,
            rows: RowsOf::Pages(None),
            shown_revision,
            read: &self.read,
            on_page: false,
        };
        for key in keys.chain([press]) {
            self.engine.key(key, &mut host);
        }
    }

    /// The rows the text takes in a pane `columns` cells wide.
    pub fn rows(&mut self, columns: usize) -> &[Row] {
        let revision = self.document.revision();
        let fresh = self
            .rows
            .as_ref()
            .is_some_and(|(for_revision, for_columns, _)| {
                (*for_revision, *for_columns) == (revision, columns)
            });
        if !fresh {
            self.rows = Some((revision, columns, rows(self.document.text(), columns)));
        }
        self.rows.as_ref().map_or(&[], |(_, _, rows)| rows)
    }

    /// The first row to show in a pane of `visible` rows so that the cursor
    /// is in view, moving the view as little as possible. `margin` rows are
    /// kept between the cursor and the pane's edges where there is text.
    pub fn scroll_into_view(&mut self, columns: usize, visible: usize, margin: usize) -> usize {
        let cursor = self.cursor();
        let rows = self.rows(columns);
        let row = row_of(rows, cursor);
        let count = rows.len();
        let margin = margin.min(visible.saturating_sub(1) / 2);
        let mut top = self.top;
        if row < top + margin {
            top = row.saturating_sub(margin);
        }
        if row + margin + 1 > top + visible {
            top = row + margin + 1 - visible;
        }
        self.top = top.min(count.saturating_sub(1));
        self.top
    }
}

/// The document as the Vim engine sees it.
struct DocumentHost<'a> {
    document: &'a mut Document<SourceBuffer>,
    failure: &'a mut Option<String>,
    rows: RowsOf<'a>,
    /// The revision of the text those rows are of.
    shown_revision: u64,
    /// What the text is read as, kept between keys.
    read: &'a crate::reading::Cache,
    /// Whether the edits are made by keys on a page, not by an agent.
    on_page: bool,
}

/// How often what a deletion left in an equation is cleared away: what
/// is cleared may leave something of the kind itself.
const TIDY_ROUNDS: usize = 4;

/// The rows the cursor moves by.
enum RowsOf<'a> {
    Source(&'a [Row]),
    Pages(Option<&'a dyn Rows>),
}

impl Host for DocumentHost<'_> {
    fn text(&self) -> &str {
        self.document.text()
    }

    fn edit(&mut self, edits: &[Edit], join: bool) {
        let mut replaces: Vec<Replace> = edits
            .iter()
            .map(|edit| Replace::new(edit.range.clone(), edit.text.as_str()))
            .collect();
        // On a page, a deletion keeps the dollar signs of an equation it
        // takes only a part of: they show as nothing.
        if self.on_page
            && let [edit] = edits
            && edit.text.is_empty()
        {
            let source = &self.document.buffer().0;
            let parts = crate::typst_text::deleted_on_page(source, &edit.range);
            if parts.is_empty() {
                *self.failure = Some("the marks of an equation stay".to_owned());
                return;
            }
            replaces = parts
                .into_iter()
                .map(|part| Replace::new(part, ""))
                .collect();
        }
        if let Err(err) = self.document.edit(&replaces, join) {
            *self.failure = Some(err.to_string());
        }
    }

    fn undo(&mut self) -> Option<Range<usize>> {
        let extent = self.document.history().undo_step()?.extent()?;
        match self.document.undo() {
            Ok(done) => done.map(|_| extent),
            Err(err) => {
                *self.failure = Some(err.to_string());
                None
            }
        }
    }

    fn row(&self, from: usize, rows: isize, across: Option<f64>) -> Option<(usize, f64)> {
        if self.document.revision() != self.shown_revision {
            // The key has changed the text: the rows are those of what it
            // was before.
            return None;
        }
        let shown = match &self.rows {
            RowsOf::Pages(pages) => return pages.as_ref()?.row(from, rows, across),
            RowsOf::Source(shown) => shown,
        };
        let text = self.document.text();
        let here = row_of(shown, from);
        let row = shown.get(here)?;
        let across =
            across.unwrap_or_else(|| cells(text.get(row.range.start..from).unwrap_or("")) as f64);
        let target = here
            .saturating_add_signed(rows)
            .min(shown.len().saturating_sub(1));
        let range = shown.get(target)?.range.clone();
        let mut before = 0.0;
        for (at, c) in text.get(range.clone())?.char_indices() {
            before += cells_of(c) as f64;
            if before > across {
                return Some((range.start + at, across));
            }
        }
        Some((range.end, across))
    }

    fn format(&self, format: zetwerk_vim::Format, range: Range<usize>) -> Option<Vec<Edit>> {
        let source = &self.document.buffer().0;
        let edits = crate::typst_text::format(source, format, range)?;
        let edit = |(range, text)| Edit { range, text };
        Some(edits.into_iter().map(edit).collect())
    }

    fn object(&self, name: char, at: usize, around: bool) -> Option<Range<usize>> {
        crate::typst_text::object(&self.document.buffer().0, name, at, around)
    }

    fn reading(&self) -> Option<Arc<Reading>> {
        let source = &self.document.buffer().0;
        matches!(self.rows, RowsOf::Pages(_))
            .then(|| self.read.get(self.document.revision(), source))
    }

    fn expand(&self, at: usize) -> Option<(Range<usize>, String, usize)> {
        crate::typst_text::snippet(&self.document.buffer().0, at)
    }

    fn typed(&self, at: usize, c: char) -> Option<(Range<usize>, String, usize)> {
        crate::typst_text::typed(&self.document.buffer().0, at, c)
    }

    fn set_off(
        &self,
        at: usize,
        format: zetwerk_vim::Format,
    ) -> Option<(Range<usize>, String, usize)> {
        crate::typst_text::set_off(&self.document.buffer().0, at, format)
    }

    fn unmake(&self, at: usize) -> Option<(Range<usize>, String)> {
        crate::typst_text::unmake(&self.document.buffer().0, at)
    }

    fn put_over(&self, stop: Range<usize>, c: char) -> Option<(Range<usize>, String, usize)> {
        crate::typst_text::put_over(&self.document.buffer().0, &stop, c)
    }

    fn letters(&self, range: Range<usize>) -> Option<Vec<Range<usize>>> {
        matches!(self.rows, RowsOf::Pages(_))
            .then(|| crate::typst_text::letters(&self.document.buffer().0, &range))
    }

    fn refuses(&mut self, edits: &[Edit]) -> bool {
        if !matches!(self.rows, RowsOf::Pages(_)) {
            return false;
        }
        let edits: Vec<(Range<usize>, &str)> = edits
            .iter()
            .map(|edit| (edit.range.clone(), edit.text.as_str()))
            .collect();
        let why = crate::typst_text::breaks(&self.document.buffer().0, &edits);
        if let Some(why) = &why {
            *self.failure = Some(format!("it would leave {why}"));
        }
        why.is_some()
    }

    fn cuts(&self, range: Range<usize>) -> bool {
        matches!(self.rows, RowsOf::Pages(_))
            && crate::typst_text::cuts(&self.document.buffer().0, &range)
    }

    fn join(&self, at: usize, spaced: bool) -> Option<(Range<usize>, String)> {
        // Where nothing follows, nothing is joined: not lines either.
        let nothing = || (at..at, String::new());
        matches!(self.rows, RowsOf::Pages(_)).then(|| {
            crate::typst_text::join(&self.document.buffer().0, at, spaced).unwrap_or_else(nothing)
        })
    }

    fn parted(&self, range: Range<usize>) -> bool {
        matches!(self.rows, RowsOf::Pages(_))
            && crate::typst_text::parted(&self.document.buffer().0, range)
    }

    fn pieces(&self, range: Range<usize>, with: &str) -> Option<Vec<Range<usize>>> {
        if !matches!(self.rows, RowsOf::Pages(_)) {
            return Some(vec![range]);
        }
        crate::typst_text::pieces(&self.document.buffer().0, range, with)
    }

    fn next_field(&self, at: usize) -> Option<(Range<usize>, String, usize)> {
        matches!(self.rows, RowsOf::Pages(_))
            .then(|| crate::typst_text::tab_key(&self.document.buffer().0, at))?
    }

    fn stop(&self, at: usize, seek: Seek) -> Option<Range<usize>> {
        // The dollar sign that closes an equation with nothing in it is
        // the place to fill in, which is no bytes: also before the pages
        // show it (it was just emptied).
        if seek == Seek::At && matches!(self.rows, RowsOf::Pages(_)) {
            let source = &self.document.buffer().0;
            let text = source.text();
            let empty = crate::typst_text::equation_at(source, at).is_some_and(|equation| {
                equation.end == at + 1
                    && text
                        .get(equation.start + 1..at)
                        .is_some_and(|body| body.trim().is_empty())
            });
            if empty {
                return Some(at..at);
            }
        }
        let RowsOf::Pages(Some(pages)) = &self.rows else {
            return None;
        };
        if self.document.revision() != self.shown_revision {
            return None;
        }
        match pages.stop(at, seek) {
            Ok(stop) => stop,
            // Text typed since the pages were made: it will show as it
            // was typed, character by character.
            Err(Unseen) if matches!(seek, Seek::At | Seek::After | Seek::Before) => {
                let text = self.document.text();
                let c = text.get(at..)?.chars().next().filter(|c| *c != '\n')?;
                Some(at..at + c.len_utf8())
            }
            Err(Unseen) => None,
        }
    }

    fn shown_line(&self, at: usize) -> Option<ShownLine> {
        let RowsOf::Pages(Some(pages)) = &self.rows else {
            return None;
        };
        if self.document.revision() != self.shown_revision {
            return None;
        }
        pages.line(at)
    }

    fn open(&self, at: usize, below: bool) -> Option<(usize, String, usize)> {
        matches!(self.rows, RowsOf::Pages(_))
            .then(|| crate::typst_text::open(&self.document.buffer().0, at, below))
    }

    fn line_break(&self, at: usize) -> Option<(Range<usize>, String)> {
        matches!(self.rows, RowsOf::Pages(_))
            .then(|| crate::typst_text::line_break(&self.document.buffer().0, at))
    }

    fn redo(&mut self) -> Option<Range<usize>> {
        let extent = self.document.history().redo_step()?.extent()?;
        match self.document.redo() {
            Ok(done) => done.map(|_| extent),
            Err(err) => {
                *self.failure = Some(err.to_string());
                None
            }
        }
    }
}

/// The nearest character boundary at or before `at`.
fn floor_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// How many cells of the pane `c` takes.
fn cells_of(c: char) -> usize {
    if c == '\t' { TAB_CELLS } else { 1 }
}

/// How many cells `text` takes.
#[must_use]
pub fn cells(text: &str) -> usize {
    text.chars().map(cells_of).sum()
}

/// Breaks `text` into rows of at most `columns` cells. A line that is too
/// long continues in the next row, broken after a blank where there is one.
#[must_use]
pub fn rows(text: &str, columns: usize) -> Vec<Row> {
    let columns = columns.max(1);
    let mut rows = Vec::new();
    let body = text.strip_suffix('\n').unwrap_or(text);
    let mut line_start = 0;
    for (line, content) in body.split('\n').enumerate() {
        let line_end = line_start + content.len();
        let mut start = line_start;
        loop {
            let mut used = 0;
            let mut end = start;
            let mut after_blank = None;
            for (offset, c) in text[start..line_end].char_indices() {
                if used + cells_of(c) > columns && offset > 0 {
                    break;
                }
                used += cells_of(c);
                end = start + offset + c.len_utf8();
                if c == ' ' {
                    after_blank = Some(end);
                }
            }
            let first = start == line_start;
            if end == line_end {
                rows.push(Row {
                    line,
                    range: start..line_end,
                    first,
                });
                break;
            }
            let cut = after_blank.unwrap_or(end);
            rows.push(Row {
                line,
                range: start..cut,
                first,
            });
            start = cut;
        }
        line_start = line_end + 1;
    }
    rows
}

/// The index of the row that shows byte `offset`.
#[must_use]
pub fn row_of(rows: &[Row], offset: usize) -> usize {
    rows.partition_point(|row| row.range.start <= offset)
        .saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranges(text: &str, columns: usize) -> Vec<&str> {
        rows(text, columns)
            .into_iter()
            .map(|row| &text[row.range])
            .collect()
    }

    #[test]
    fn short_lines_are_one_row_each() {
        let text = "one\n\nthree\n";
        assert_eq!(ranges(text, 80), ["one", "", "three"]);
        let rows = rows(text, 80);
        assert_eq!(rows[2].line, 2);
        assert!(rows.iter().all(|row| row.first));
        // An empty text is one empty line.
        assert_eq!(ranges("", 80), [""]);
        assert_eq!(ranges("no line break", 80), ["no line break"]);
    }

    #[test]
    fn long_lines_break_after_blanks() {
        let text = "the quick brown fox\n";
        assert_eq!(ranges(text, 10), ["the quick ", "brown fox"]);
        let rows = rows(text, 10);
        assert!(rows[0].first && !rows[1].first);
        assert_eq!((rows[0].line, rows[1].line), (0, 0));
        // Without a blank the row is simply full.
        assert_eq!(ranges("abcdefghij\n", 4), ["abcd", "efgh", "ij"]);
        // Characters of several bytes are not cut.
        assert_eq!(ranges("ééééé\n", 2), ["éé", "éé", "é"]);
    }

    #[test]
    fn every_byte_has_its_row() {
        let text = "the quick brown fox\nnext\n";
        let rows = rows(text, 10);
        assert_eq!(row_of(&rows, 0), 0);
        assert_eq!(row_of(&rows, 9), 0);
        // The start of the second row of the line belongs to that row.
        assert_eq!(row_of(&rows, 10), 1);
        // The end of the line (where its line break is) is in its last row.
        assert_eq!(row_of(&rows, 19), 1);
        assert_eq!(row_of(&rows, 20), 2);
        assert_eq!(row_of(&rows, 999), 2);
    }

    #[test]
    fn tabs_take_several_cells() {
        assert_eq!(cells("a\tb"), 2 + TAB_CELLS);
        assert_eq!(ranges("\t\tab\n", TAB_CELLS * 2), ["\t\t", "ab"]);
    }
}
