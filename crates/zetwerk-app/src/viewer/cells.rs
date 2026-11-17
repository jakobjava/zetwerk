//! Ctrl-V on a page: a block of cells of a table.
//!
//! In the source a block is a rectangle of characters (see the engine).
//! On a page a rectangle of glyphs means nothing, except in a table, where
//! it is the natural way to take a block of cells: Ctrl-V in a cell
//! selects it, `h` `j` `k` `l` (and the arrow keys, `0`, `$`, `o`) make
//! the block larger or smaller, `d` empties its cells, `y` takes what
//! they hold, `c` empties them and what is typed into the first goes into
//! all of them, and `p` puts what was taken cell by cell, from the cell
//! of the cursor (or the block's first) on. Escape or Ctrl-V again ends
//! it. Elsewhere on a page Ctrl-V says why it does nothing.

use std::ops::Range;

use typst::syntax::Source;

use super::{Edited, Panes, Viewer};
use crate::editor;
use crate::keys::{Key, KeyPress};
use crate::typst_text::{Table, table_at};

/// What a block of cells keeps.
#[derive(Default)]
pub(super) struct Cells {
    /// The block selected: where its table begins, the cell it was begun
    /// in and the cell the cursor is in, counted in the order of the
    /// table.
    block: Option<Block>,
    /// What `y` took: as the unnamed register holds it (cells apart by
    /// tabs, rows by line breaks), and cell by cell.
    taken: Option<(String, Vec<Vec<String>>)>,
    /// After `c`: the table and the cells that get what is typed into the
    /// first, once typing is over.
    fill: Option<(usize, usize, Vec<usize>)>,
}

impl Cells {
    /// Whether a block of cells is selected.
    pub(super) fn has_block(&self) -> bool {
        self.block.is_some()
    }
}

#[derive(Clone, Copy)]
struct Block {
    table: usize,
    anchor: usize,
    cursor: usize,
}

/// What a cell holds: inside its brackets, where it is written with
/// them; else all of it.
fn inside(source: &Source, cell: &Range<usize>) -> Range<usize> {
    let text = source.text().get(cell.clone()).unwrap_or("");
    if text.starts_with('[') && text.ends_with(']') && text.len() >= 2 {
        cell.start + 1..cell.end - 1
    } else {
        cell.clone()
    }
}

/// What makes cell `cell` hold `with`: inside its brackets, or a cell in
/// brackets in its place.
fn holding(source: &Source, cell: &Range<usize>, with: &str) -> (Range<usize>, String) {
    let within = inside(source, cell);
    if within == *cell {
        (cell.clone(), format!("[{with}]"))
    } else {
        (within, with.to_owned())
    }
}

impl Block {
    /// The cells of the block, row by row, as indices of the table's.
    fn cells(&self, table: &Table) -> Vec<Vec<usize>> {
        let columns = table.columns.max(1);
        let (a, c) = (
            (self.anchor / columns, self.anchor % columns),
            (self.cursor / columns, self.cursor % columns),
        );
        (a.0.min(c.0)..=a.0.max(c.0))
            .map(|row| {
                (a.1.min(c.1)..=a.1.max(c.1))
                    .map(|column| row * columns + column)
                    .filter(|index| *index < table.cells.len())
                    .collect()
            })
            .collect()
    }
}

impl Viewer {
    /// The table that begins at byte `start` of the text being edited.
    fn table_from(&self, start: usize) -> Option<Table> {
        let editor = self.editor.as_ref()?;
        table_at(editor.source(), start + 1).filter(|table| table.whole.start == start)
    }

    /// What the block of cells selected covers, to be shown on the page.
    pub(super) fn cell_marks(&self) -> Vec<Range<usize>> {
        let (Some(block), Some(editor)) = (self.vim.cells.block, &self.editor) else {
            return Vec::new();
        };
        let Some(table) = self.table_from(block.table) else {
            return Vec::new();
        };
        block
            .cells(&table)
            .into_iter()
            .flatten()
            .filter_map(|index| table.cells.get(index))
            .map(|cell| inside(editor.source(), cell))
            .collect()
    }

    /// A key for a block of cells, or Ctrl-V that begins one; `p` of what
    /// one took. Returns whether it was taken.
    pub(super) fn cell_block_key(&mut self, press: KeyPress) -> bool {
        if self.vim.cells.fill.is_some() {
            return self.cell_fill_key(press);
        }
        let on_page = self.shown_panes() == Panes::Page;
        let Some(editor) = &self.editor else {
            return false;
        };
        let mode = editor.mode();
        if !on_page || !matches!(mode, editor::Mode::Normal | editor::Mode::Visual) {
            self.vim.cells.block = None;
            return false;
        }
        let ctrl_v = press.ctrl && press.key == Key::Char('v');
        let Some(block) = self.vim.cells.block else {
            if ctrl_v {
                self.begin_cell_block();
                return true;
            }
            let puts = !press.ctrl && matches!(press.key, Key::Char('p' | 'P'));
            return puts && mode == editor::Mode::Normal && self.put_cells(None);
        };
        let Some(table) = self.table_from(block.table) else {
            self.vim.cells.block = None;
            return false;
        };
        let columns = table.columns.max(1);
        let count = table.cells.len();
        let (row, column) = (block.cursor / columns, block.cursor % columns);
        let mut moved = block;
        match (press.key, press.ctrl) {
            (Key::Escape, _) | (Key::Char('v' | '['), true) => {
                self.vim.cells.block = None;
                return true;
            }
            (Key::Char('h') | Key::Left, false) if column > 0 => moved.cursor -= 1,
            (Key::Char('l') | Key::Right, false) if column + 1 < columns => {
                moved.cursor = (moved.cursor + 1).min(count - 1);
            }
            (Key::Char('k') | Key::Up, false) if row > 0 => moved.cursor -= columns,
            (Key::Char('j') | Key::Down, false) if block.cursor + columns < count => {
                moved.cursor += columns;
            }
            (Key::Char('0' | '^') | Key::Home, false) => moved.cursor = row * columns,
            (Key::Char('$') | Key::End, false) => {
                moved.cursor = (row * columns + columns - 1).min(count - 1);
            }
            (Key::Char('o'), false) => (moved.anchor, moved.cursor) = (block.cursor, block.anchor),
            (Key::Char('d' | 'x'), false) => return self.empty_cells(&table, block, false),
            (Key::Char('c' | 's'), false) => return self.empty_cells(&table, block, true),
            (Key::Char('y'), false) => {
                self.take_cells(&table, block);
                self.vim.cells.block = None;
                return true;
            }
            (Key::Char('p' | 'P'), false) => {
                let first = block.anchor.min(block.cursor);
                self.vim.cells.block = None;
                if !self.put_cells(Some((block.table, first))) {
                    self.alert("nothing taken from cells to put: y in a block of cells first");
                }
                return true;
            }
            _ => {
                self.notice("a block of cells: h j k l to make it, d y c p, Escape to end");
                return true;
            }
        }
        self.vim.cells.block = Some(moved);
        // The cursor goes along, into the cell it is in.
        if let Some(cell) = table.cells.get(moved.cursor)
            && let Some(editor) = &mut self.editor
        {
            let at = inside(editor.source(), cell).start;
            editor.point(at);
            self.follow_cursor();
        }
        true
    }

    /// Ctrl-V on a page: a block of the cell the cursor is in, where it
    /// is in a table; elsewhere a word of why not.
    fn begin_cell_block(&mut self) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let cursor = editor.cursor();
        let found = table_at(editor.source(), cursor)
            .and_then(|table| Some((table.whole.start, table.cell_at(cursor)?)));
        let Some((table, cell)) = found else {
            return self.alert(
                "a block on a page is of cells of a table; here it would be of glyphs, \
                 which mean nothing together (in the source, Ctrl-V is Vim's)",
            );
        };
        // Out of a selection: the block is the selection now.
        if editor.mode() == editor::Mode::Visual {
            editor.point(cursor);
        }
        self.vim.cells.block = Some(Block {
            table,
            anchor: cell,
            cursor: cell,
        });
        self.notice("a block of cells: h j k l to make it, d y c p, Escape to end");
    }

    /// `y`: what the cells of the block hold, into the unnamed register.
    fn take_cells(&mut self, table: &Table, block: Block) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let source = editor.source();
        let grid: Vec<Vec<String>> = block
            .cells(table)
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .filter_map(|index| table.cells.get(index))
                    .map(|cell| {
                        let within = inside(source, cell);
                        source.text().get(within).unwrap_or("").trim().to_owned()
                    })
                    .collect()
            })
            .collect();
        let written: Vec<String> = grid.iter().map(|row| row.join("\t")).collect();
        let written = written.join("\n");
        editor.set_register('"', &written);
        editor.set_register('0', &written);
        let count: usize = grid.iter().map(Vec::len).sum();
        self.vim.cells.taken = Some((written, grid));
        self.notice(format!("{count} cells taken (p puts them, cell by cell)"));
    }

    /// `d` and `c`: the cells of the block emptied, as one undo step; with
    /// `typing`, typing begins in the first, and what is typed goes into
    /// the others once it is over.
    fn empty_cells(&mut self, table: &Table, block: Block, typing: bool) -> bool {
        self.take_cells(table, block);
        self.vim.cells.block = None;
        let Some(editor) = &mut self.editor else {
            return true;
        };
        let cells: Vec<usize> = block.cells(table).into_iter().flatten().collect();
        let edits: Vec<(Range<usize>, String)> = cells
            .iter()
            .rev()
            .filter_map(|index| table.cells.get(*index))
            .map(|cell| holding(editor.source(), cell, ""))
            .collect();
        let borrowed: Vec<(Range<usize>, &str)> = edits
            .iter()
            .map(|(range, with)| (range.clone(), with.as_str()))
            .collect();
        if let Err(why) = editor.replace(&borrowed) {
            self.alert(format!("not changed: {why}"));
            return true;
        }
        self.text_changed(Edited::Command);
        let Some(table) = self.table_from(block.table) else {
            return true;
        };
        let Some(first) = cells.first().and_then(|first| table.cells.get(*first)) else {
            return true;
        };
        if let Some(editor) = &mut self.editor {
            let at = inside(editor.source(), first).start;
            editor.point(at);
        }
        if typing {
            self.vim.cells.fill = Some((block.table, cells[0], cells[1..].to_vec()));
            self.editor_key(KeyPress::plain(Key::Char('i')));
        } else {
            let count = cells.len();
            self.notice(format!("{count} cells emptied  (u takes it back)"));
        }
        self.follow_cursor();
        true
    }

    /// A key while what is typed into the first cell of `c` waits to go
    /// into the others: they get it when typing is over.
    fn cell_fill_key(&mut self, press: KeyPress) -> bool {
        let typing = self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.mode() == editor::Mode::Insert);
        if !typing {
            self.vim.cells.fill = None;
            return false;
        }
        let leaves =
            press.key == Key::Escape || (press.ctrl && matches!(press.key, Key::Char('[' | 'c')));
        if !leaves {
            return false;
        }
        self.editor_key(press);
        let Some((start, first, others)) = self.vim.cells.fill.take() else {
            return true;
        };
        let Some(table) = self.table_from(start) else {
            return true;
        };
        let Some(editor) = &mut self.editor else {
            return true;
        };
        let source = editor.source();
        let Some(typed) = table
            .cells
            .get(first)
            .and_then(|cell| source.text().get(inside(source, cell)))
            .map(str::to_owned)
        else {
            return true;
        };
        let edits: Vec<(Range<usize>, String)> = others
            .iter()
            .rev()
            .filter_map(|index| table.cells.get(*index))
            .map(|cell| holding(source, cell, &typed))
            .collect();
        let borrowed: Vec<(Range<usize>, &str)> = edits
            .iter()
            .map(|(range, with)| (range.clone(), with.as_str()))
            .collect();
        let cursor = editor.cursor();
        match editor.replace(&borrowed) {
            Ok(()) => {
                editor.set_cursor(cursor);
                self.text_changed(Edited::Command);
            }
            Err(why) => self.alert(format!("not changed: {why}")),
        }
        true
    }

    /// `p` of cells taken: put cell by cell from cell `at` of the table
    /// beginning at byte `table` (or from the cell of the cursor), as many
    /// as fit. Returns whether there was something to put there.
    fn put_cells(&mut self, at: Option<(usize, usize)>) -> bool {
        let Some((written, grid)) = self.vim.cells.taken.clone() else {
            return false;
        };
        let Some(editor) = &self.editor else {
            return false;
        };
        // Only what the unnamed register still holds.
        if editor.engine().register('"').map(|(held, _)| held) != Some(written.as_str()) {
            return false;
        }
        let cursor = editor.cursor();
        let place = at.or_else(|| {
            let table = table_at(editor.source(), cursor)?;
            Some((table.whole.start, table.cell_at(cursor)?))
        });
        let Some((start, first)) = place else {
            return false;
        };
        let Some(table) = self.table_from(start) else {
            return false;
        };
        let columns = table.columns.max(1);
        let (row, column) = (first / columns, first % columns);
        let mut edits: Vec<(Range<usize>, String)> = Vec::new();
        for (down, cells) in grid.iter().enumerate() {
            for (across, with) in cells.iter().enumerate() {
                if column + across >= columns {
                    continue;
                }
                let index = (row + down) * columns + column + across;
                if let Some(cell) = table.cells.get(index) {
                    edits.push(holding(editor.source(), cell, with));
                }
            }
        }
        edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
        let borrowed: Vec<(Range<usize>, &str)> = edits
            .iter()
            .map(|(range, with)| (range.clone(), with.as_str()))
            .collect();
        let count = edits.len();
        let Some(editor) = &mut self.editor else {
            return false;
        };
        match editor.replace(&borrowed) {
            Ok(()) => {
                self.text_changed(Edited::Command);
                self.notice(format!("{count} cells put  (u takes it back)"));
            }
            Err(why) => self.alert(format!("not changed: {why}")),
        }
        true
    }
}
