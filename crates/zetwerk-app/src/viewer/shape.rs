//! A table shaped from the page: a row or a column more, one taken
//! out, one moved, the first row made the header, the table copied as
//! cells for a spreadsheet.
//!
//! The keys follow `]` and `[` (where the cursor goes: below or right,
//! above or left), as the engine's tables say: `]r` `[r` a row below or
//! above, `]c` `[c` a column right or left, `]R` `[R` the row moved
//! down or up, `]C` `[C` the column moved right or left, `]x` `[x` the
//! row taken out, `]X` `[X` the column; `]h` the first row the header,
//! `[h` the header a row again; `]y` (or `[y`) the copy.
//! Each is a command too (`:table row add below`, …, see
//! [`Viewer::shape_command`]) and one step for undo, after which the
//! cursor is in the cell it was in, where that has gone the one next
//! to it, and in a new row or column its cell. The changes are those of
//! `table_text`: only a table of cells is changed; one with a cell over
//! several columns, a line or anything else written into it by hand is
//! refused with a word, not changed.

use typst::syntax::{LinkedNode, Side, SyntaxKind};

use super::{Edited, Viewer};
use crate::table_text::{Shape, Shaped};

/// What is said where the cursor is in no table.
const NO_TABLE: &str = "the cursor is in no table: ]r [r ]c [c and the like shape the one it is in";

/// What is said of a table that has more to it than cells.
const NOT_CELLS: &str = "this table has more to it than cells (a cell over several columns, a \
                         line, a footer, …): it is not changed from the page; gs shows how it \
                         is written";

impl Viewer {
    /// The table the cursor is in, where it is one that can be shaped:
    /// where it begins, and the cell of the cursor. `Err` says why not.
    pub(super) fn table_here(&self) -> Result<(usize, usize), &'static str> {
        let editor = self.editor.as_ref().ok_or("no file is being edited")?;
        let (source, cursor) = (editor.source(), editor.cursor());
        // The cursor stands on a character of a cell: behind its start.
        let found = crate::typst_text::table_at(source, cursor + 1).and_then(|table| {
            let cell = table
                .cell_at(cursor + 1)
                .or_else(|| table.cell_at(cursor))?;
            Some((table.whole.start, cell))
        });
        if let Some(found) = found {
            return Ok(found);
        }
        // In a call of `table` all the same: one of more than cells.
        let leaf = LinkedNode::new(source.root()).leaf_at(cursor + 1, Side::Before);
        let in_table = std::iter::successors(leaf, |node| node.parent().cloned()).any(|node| {
            node.kind() == SyntaxKind::FuncCall
                && node
                    .children()
                    .next()
                    .and_then(|name| source.text().get(name.range()))
                    .is_some_and(|name| matches!(name, "table" | "grid"))
        });
        Err(if in_table { NOT_CELLS } else { NO_TABLE })
    }

    /// `:table` with what this module does: `row add [below|above]`,
    /// `row delete [above]`, `row move up|down`, `column add
    /// [right|left]`, `column delete [left]`, `column move right|left`,
    /// `header [on|off]`, `copy`. Whether `what` was one of them.
    pub(super) fn shape_command(&mut self, what: &str) -> bool {
        let words: Vec<&str> = what.split_whitespace().collect();
        let (thing, rest) = match words.as_slice() {
            ["row" | "rows", rest @ ..] if !rest.is_empty() => ("row", rest),
            ["column" | "col" | "columns", rest @ ..] if !rest.is_empty() => ("column", rest),
            ["header", rest @ ..] => ("header", rest),
            ["copy" | "cells" | "tsv"] => (words[0], &[][..]),
            _ => return false,
        };
        let known = matches!(
            (thing, rest),
            ("row", ["add"] | ["add", "below" | "above"])
                | (
                    "row",
                    ["delete" | "remove"] | ["delete" | "remove", "above"]
                )
                | ("row", ["move", "up" | "down"] | ["up" | "down"])
                | ("column", ["add"] | ["add", "right" | "left"])
                | (
                    "column",
                    ["delete" | "remove"] | ["delete" | "remove", "left"]
                )
                | ("column", ["move", "right" | "left"] | ["right" | "left"])
                | ("header", [] | ["on" | "off"])
                | ("copy" | "cells" | "tsv", [])
        );
        if !known {
            return false;
        }
        if let Err(why) = self.shape(thing, rest) {
            self.alert(why);
        }
        true
    }

    /// Carries out a command of [`Viewer::shape_command`].
    fn shape(&mut self, thing: &str, rest: &[&str]) -> Result<(), String> {
        let (start, cell) = self.table_here()?;
        let editor = self.editor.as_ref().ok_or("no file is being edited")?;
        let shape = Shape::new(editor.source(), start).ok_or(NOT_CELLS)?;
        let (row, column) = shape.place(cell);
        let last = rest.last().copied().unwrap_or("");
        let (shaped, told): (Option<Shaped>, String) = match (thing, rest.first().copied()) {
            ("row", Some("add")) => (
                shape.row_added(row, last != "above"),
                format!(
                    "a row more {}",
                    if last == "above" { "above" } else { "below" }
                ),
            ),
            ("row", Some("delete" | "remove")) => {
                let mut shaped = shape.row_removed(row, column);
                if last == "above"
                    && let Some(shaped) = &mut shaped
                {
                    shaped.cell.0 = row.saturating_sub(1);
                }
                (shaped, "the row is out".to_owned())
            }
            ("row", _) => (
                Some(shape.row_moved(row, column, last == "down")?),
                format!("the row moved {last}"),
            ),
            ("column", Some("add")) => (
                shape.column_added(row, column, last != "left"),
                format!(
                    "a column more {}",
                    if last == "left" { "left" } else { "right" }
                ),
            ),
            ("column", Some("delete" | "remove")) => {
                let mut shaped = shape.column_removed(row, column);
                if last == "left"
                    && let Some(shaped) = &mut shaped
                {
                    shaped.cell.1 = column.saturating_sub(1);
                }
                (shaped, "the column is out".to_owned())
            }
            ("column", _) => (
                Some(shape.column_moved(row, column, last == "right")?),
                format!("the column moved {last}"),
            ),
            ("header", _) => {
                let has = shape.table.header > 0;
                if (last == "on" && has) || (last == "off" && !has) {
                    let now = if has { "has a header" } else { "has no header" };
                    return Err(format!("the table {now} already ([h ]h switch it)"));
                }
                let told = if has {
                    "the header is a row like the others"
                } else {
                    "the first row is the header"
                };
                (shape.header_switched(row, column), told.to_owned())
            }
            _ => {
                let cells = shape.as_cells();
                let (rows, columns) = (shape.rows(), shape.table.columns);
                self.copy_cells(&cells, rows, columns);
                return Ok(());
            }
        };
        let shaped = shaped.ok_or("that cannot be done to this table: it is all it has")?;
        self.reshape(start, shaped, &told)
    }

    /// Makes the changes of `shaped` to the table that begins at byte
    /// `start` as one step, puts the cursor into its cell and tells of
    /// it.
    fn reshape(&mut self, start: usize, shaped: Shaped, told: &str) -> Result<(), String> {
        let editor = self.editor.as_mut().ok_or("no file is being edited")?;
        let edits: Vec<(std::ops::Range<usize>, &str)> = shaped
            .edits
            .iter()
            .map(|(range, with)| (range.clone(), with.as_str()))
            .collect();
        editor
            .replace(&edits)
            .map_err(|why| format!("not changed: {why}"))?;
        self.text_changed(Edited::Command);
        self.cursor_to_cell(start, shaped.cell);
        self.notice(format!("{told}  (u takes it back)"));
        Ok(())
    }

    /// Puts the cursor into the cell at `row` and `column` of the table
    /// that begins at byte `start` (or the last there is of them).
    pub(super) fn cursor_to_cell(&mut self, start: usize, (row, column): (usize, usize)) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let Some(shape) = Shape::new(editor.source(), start) else {
            return;
        };
        let cells = &shape.table.cells;
        let index = (row * shape.table.columns + column).min(cells.len().saturating_sub(1));
        if let Some(cell) = cells.get(index) {
            let inside = editor
                .text()
                .get(cell.clone())
                .is_some_and(|written| written.starts_with('['));
            editor.point(cell.start + usize::from(inside));
            self.follow_cursor();
        }
    }

    /// `]y`: `cells` of a table into the clipboard, for a spreadsheet;
    /// into the register `"+` too.
    fn copy_cells(&mut self, cells: &str, rows: usize, columns: usize) {
        let written = (self.clipboard.write)(cells);
        if let Some(editor) = &mut self.editor {
            editor.set_register('+', cells);
        }
        self.clipboard_known(cells);
        match written {
            Ok(()) => self.notice(format!(
                "the table is in the clipboard as cells, {rows} rows of {columns}: \
                 it pastes into a spreadsheet"
            )),
            Err(why) => self.alert(why),
        }
    }
}
