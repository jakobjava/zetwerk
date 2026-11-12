//! Editing the source text.
//!
//! An [`Editor`] is the text of one file as Typst parses it, the history
//! and journal that keep it safe ([`zetwerk_core::Document`]), and the Vim
//! that changes it ([`zetwerk_vim::Engine`]). Like the rest of the viewer it
//! knows nothing about windows: keys in, text and a cursor out.

use std::ops::Range;
use std::path::Path;
use std::time::Instant;

use typst::syntax::{FileId, Source};
use zetwerk_core::{Buffer, DiskError, DiskStatus, Document, OpenError, Opened, Replace};
use zetwerk_vim::{Edit, Effect, Engine, Host, Place};

use crate::keys::KeyPress;

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
}

/// Something that shows the text in rows other than the source pane's:
/// the lines of typeset pages.
pub trait Rows {
    /// See [`zetwerk_vim::Host::row`].
    fn row(&self, from: usize, rows: isize, across: Option<f64>) -> Option<(usize, f64)>;
}

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
        };
        Ok((editor, opened))
    }

    /// Handles a key press. `shown` is what the user sees the text on.
    pub fn key(&mut self, press: KeyPress, shown: &Shown) -> Typed {
        let before = self.document.revision();
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
        };
        let effects = self.engine.key(press, &mut host);
        if self.failure.is_some() {
            // The engine believes in an edit that was not made. The text is
            // what it was; all that can be off is the cursor.
            self.engine
                .set_cursor(self.engine.cursor(), self.document.text());
        }
        Typed {
            effects,
            changed: self.document.revision() != before,
        }
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
        (
            before.matches('\n').count() + 1,
            before[line_start..].chars().count() + 1,
        )
    }

    #[must_use]
    pub fn mode(&self) -> Mode {
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
}

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
        let replaces: Vec<Replace> = edits
            .iter()
            .map(|edit| Replace::new(edit.range.clone(), edit.text.as_str()))
            .collect();
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

    fn expand(&self, at: usize) -> Option<(Range<usize>, String, usize)> {
        crate::typst_text::snippet(&self.document.buffer().0, at)
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
