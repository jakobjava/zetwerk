//! Ctrl-V: a selection of a block, a rectangle of columns of lines, as
//! Vim has it: `d`, `x`, `y`, `c`, `s`, `I`, `A`, `~`, `u`, `U` on it,
//! `o` and `O` to its corners, `$` to the ends of its lines; and `p` and
//! `P` of a block yanked so, beside the cursor, line by line.
//!
//! A block is characterwise visual mode with a flag (in [`super::More`]):
//! the motions are those of a selection, and only the commands that work
//! on what is selected go by the rectangle. What a block is, is said in
//! columns as the text shows them (a tab is as wide as it reaches).
//!
//! Where the host shows the text as pages, a rectangle of glyphs means
//! nothing: the host has the key first (see `Viewer::block_key` in the
//! app), and the engine is not asked.

use std::ops::Range;

use super::{Awaiting, Engine, Host, Mode, clamp, lower, upper};
use crate::keys::{Key, KeyPress};
use crate::text::Text;

/// What `I`, `A` and `c` of a block keep while the first line is typed
/// into: the text is put into the others on Escape.
#[derive(Debug, Clone)]
pub(super) struct BlockInsert {
    /// The line typed into (counted from 0), the byte within it typing
    /// began at, and how long the line was then.
    line: usize,
    at: usize,
    length: usize,
    /// The other lines (counted from 0) and the column the text goes in
    /// at in each, and whether a line too short for it is filled up with
    /// blanks to it (`A`) or left as it is.
    others: Vec<usize>,
    column: usize,
    fill: bool,
    /// For `A` after `$`: at the end of each line.
    to_end: bool,
}

/// The columns of `line` (its start) that the block from column `left`
/// to `right` (both included; `None`: to the end of the line) covers, as
/// bytes. Where the line ends before `left`, none, at its end.
fn piece(text: Text, line: usize, left: usize, right: Option<usize>) -> Range<usize> {
    let end = text.line_end(line);
    let mut column = 0;
    let mut start = None;
    for (offset, c) in text.0[line..end].char_indices() {
        let at = line + offset;
        let width = if c == '\t' { 8 - column % 8 } else { 1 };
        if start.is_none() && column + width > left {
            start = Some(at);
        }
        if right.is_some_and(|right| column > right) {
            return start.unwrap_or(at)..at;
        }
        column += width;
    }
    start.unwrap_or(end)..end
}

/// The text of the block as a register holds it: the pieces line by
/// line. A line that ends before the block holds a blank where the
/// block begins behind its end, as Vim yanks it.
fn block_text(text: Text, pieces: &[Range<usize>], left: usize) -> String {
    pieces
        .iter()
        .map(|piece| {
            if piece.is_empty() && width(text, text.line_start(piece.start)) < left {
                " ".to_owned()
            } else {
                text.0[piece.clone()].to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// How many columns `line` shows.
fn width(text: Text, line: usize) -> usize {
    text.column(text.line_end(line))
}

impl Engine {
    /// Whether a block is selected.
    #[must_use]
    pub fn selects_block(&self) -> bool {
        self.mode == Mode::Visual && self.more.block
    }

    /// The parts of the text the block selected covers, a range in each of
    /// its lines, from the top; `None` where no block is selected.
    #[must_use]
    pub fn block(&self, text: &str) -> Option<Vec<Range<usize>>> {
        if !self.selects_block() {
            return None;
        }
        let text = Text(text);
        let (top, bottom, left, right) = self.corners(text);
        let mut pieces = Vec::new();
        let mut line = Some(top);
        while let Some(at) = line.filter(|at| *at <= bottom) {
            pieces.push(piece(text, at, left, right));
            line = text.next_line(at);
        }
        Some(pieces)
    }

    /// The first and the last line of the block (their starts), and its
    /// leftmost and rightmost column (`None`: to the ends of the lines).
    fn corners(&self, text: Text) -> (usize, usize, usize, Option<usize>) {
        let (anchor, cursor) = (self.anchor.min(text.len()), self.cursor.min(text.len()));
        let (a, c) = (text.column(anchor), text.column(cursor));
        let top = text.line_start(anchor.min(cursor));
        let bottom = text.line_start(anchor.max(cursor));
        let right = (self.wanted_column != Some(usize::MAX)).then_some(a.max(c));
        (top, bottom, a.min(c), right)
    }

    /// Ctrl-V: a block begins, or a selection becomes one; in a block it
    /// ends.
    pub(super) fn block_toggle(&mut self, host: &impl Host) {
        let _ = self.take_count();
        self.reset_pending();
        if self.selects_block() {
            self.more.block = false;
            self.mode = Mode::Normal;
            self.cursor = clamp(Text(host.text()), self.cursor, false);
            return;
        }
        if self.mode == Mode::Normal {
            self.anchor = self.cursor;
            // A new command starts, as with `v`.
            self.record.clear();
            self.command_start = 0;
        }
        self.mode = Mode::Visual;
        self.more.block = true;
        self.line_select = false;
    }

    /// A key while a block is selected that works on the block. Returns
    /// whether it was one: the others (motions, Escape, `:`) are those of
    /// any selection.
    pub(super) fn block_key(&mut self, press: KeyPress, host: &mut impl Host) -> bool {
        if self.mode != Mode::Visual {
            // Left otherwise than by these keys: no block any more.
            self.more.block = false;
            return false;
        }
        if self.pending.awaiting != Awaiting::Nothing || self.pending.operator.is_some() {
            return false;
        }
        let Key::Char(c) = press.key else {
            return false;
        };
        if press.ctrl || press.alt {
            return false;
        }
        match c {
            'v' => {
                self.more.block = false;
                self.reset_pending();
            }
            'V' => {
                self.more.block = false;
                return false;
            }
            'O' => {
                let text = Text(host.text());
                let (a, c) = (text.column(self.anchor), text.column(self.cursor));
                let anchor_line = text.line_start(self.anchor);
                let cursor_line = text.line_start(self.cursor);
                self.anchor = text.at_column(anchor_line, c);
                self.cursor = text.at_column(cursor_line, a);
                self.wanted_column = None;
                self.reset_pending();
            }
            'd' | 'x' | 'X' | 'D' => self.block_delete(matches!(c, 'X' | 'D'), host),
            'y' | 'Y' => self.block_yank(c == 'Y', host),
            'c' | 's' | 'C' => self.block_change(c == 'C', host),
            'I' | 'A' => self.block_insert(c == 'A', host),
            '~' | 'u' | 'U' => self.block_case(c, host),
            // Not done on a block: said by failing, as Vim beeps.
            'p' | 'P' | 'r' | '>' | '<' | 'S' | 'R' | 'g' | '=' => self.fail_block(),
            _ => return false,
        }
        true
    }

    fn fail_block(&mut self) {
        self.fail();
        self.more.block = false;
        self.mode = Mode::Normal;
    }

    /// Ends the block, the cursor at its top left, and keeps its text.
    fn block_take(
        &mut self,
        to_end: bool,
        deleted: bool,
        host: &impl Host,
    ) -> Option<(Vec<Range<usize>>, usize)> {
        if to_end {
            self.wanted_column = Some(usize::MAX);
        }
        let pieces = self.block(host.text())?;
        let text = Text(host.text());
        let (top, _, left, _) = self.corners(text);
        let taken = block_text(text, &pieces, left);
        let _ = self.take_count();
        self.store(taken.clone(), false, deleted);
        self.more.blocks.retain(|(_, held)| *held != taken);
        let names: Vec<char> = self
            .registers
            .iter()
            .filter(|(_, register)| register.text == taken)
            .map(|(name, _)| *name)
            .collect();
        for name in names {
            self.more.blocks.push((name, taken.clone()));
        }
        self.reset_pending();
        self.mode = Mode::Normal;
        self.more.block = false;
        self.repeatable = false;
        self.cursor = clamp(text, text.at_column(top, left).max(top), false);
        self.wanted_column = None;
        Some((pieces, top))
    }

    fn block_yank(&mut self, to_end: bool, host: &mut impl Host) {
        let _ = self.block_take(to_end, false, host);
    }

    fn block_delete(&mut self, to_end: bool, host: &mut impl Host) {
        let Some((pieces, _)) = self.block_take(to_end, true, host) else {
            return;
        };
        for piece in pieces.iter().rev().filter(|piece| !piece.is_empty()) {
            self.edit(host, piece.clone(), "");
        }
        self.cursor = clamp(Text(host.text()), self.cursor, false);
    }

    /// `c`: the block goes, and what is typed in its first line goes into
    /// the others where it was.
    fn block_change(&mut self, to_end: bool, host: &mut impl Host) {
        let text = Text(host.text());
        let (top, _, left, _) = self.corners(text);
        let Some((pieces, _)) = self.block_take(to_end, true, host) else {
            return;
        };
        let reached: Vec<bool> = pieces
            .iter()
            .map(|piece| width(text, text.line_start(piece.start)) > left)
            .collect();
        for piece in pieces.iter().rev().filter(|piece| !piece.is_empty()) {
            self.edit(host, piece.clone(), "");
        }
        let text = Text(host.text());
        let line = text.line_index(top);
        let others = (1..pieces.len())
            .filter(|index| reached[*index])
            .map(|index| line + index)
            .collect();
        self.cursor = pieces.first().map_or(top, |piece| piece.start);
        self.begin_block_insert(host, line, others, left, false, false);
    }

    /// `I` (before the block) and `A` (`after` it): what is typed in its
    /// first line goes into the others there.
    fn block_insert(&mut self, after: bool, host: &mut impl Host) {
        let text = Text(host.text());
        let (top, bottom, left, right) = self.corners(text);
        let to_end = right.is_none();
        let column = match right {
            Some(right) if after => right + 1,
            None if after => usize::MAX,
            _ => left,
        };
        let line = text.line_index(top);
        let last = text.line_index(bottom);
        let _ = self.take_count();
        self.reset_pending();
        self.mode = Mode::Normal;
        self.more.block = false;
        self.repeatable = false;
        // Into the lines that reach the block; with `A`, into all.
        let reaches = |index: usize| after || width(text, text.line(index)) > left;
        let others: Vec<usize> = (line + 1..=last).filter(|index| reaches(*index)).collect();
        self.cursor = if to_end {
            text.line_end(top)
        } else {
            self.fill_to(host, top, column)
        };
        self.begin_block_insert(host, line, others, column, after, to_end);
        // Escape brings the cursor back to the corner, as in Vim.
        self.more.insert_corner = Some((line, left));
    }

    /// The byte of the line beginning at `line` at `column`, the line
    /// filled up with blanks to it where it is shorter.
    fn fill_to(&mut self, host: &mut impl Host, line: usize, column: usize) -> usize {
        let text = Text(host.text());
        let shown = width(text, line);
        let end = text.line_end(line);
        if shown < column {
            self.edit(host, end..end, &" ".repeat(column - shown));
            return Text(host.text()).line_end(line);
        }
        let mut at = line;
        while at < end && text.column(at) < column {
            at = text.next(at);
        }
        at
    }

    fn begin_block_insert(
        &mut self,
        host: &impl Host,
        line: usize,
        others: Vec<usize>,
        column: usize,
        fill: bool,
        to_end: bool,
    ) {
        let text = Text(host.text());
        let start = text.line(line);
        let length = text.line_end(start) - start;
        self.more.block_insert = Some(BlockInsert {
            line,
            at: self.cursor - start,
            length,
            others,
            column,
            fill,
            to_end,
        });
        self.more.insert_corner = None;
        self.enter_insert(1, host.text());
    }

    /// On Escape after `I`, `A` or `c` of a block: what was typed in its
    /// first line goes into the others, as one undo step with it.
    pub(super) fn end_block_insert(&mut self, host: &mut impl Host) {
        let Some(insert) = self.more.block_insert.take() else {
            return;
        };
        let corner = self.more.insert_corner.take();
        let text = Text(host.text());
        if text.line_count() <= insert.line {
            return;
        }
        let start = text.line(insert.line);
        let length = text.line_end(start) - start;
        // Several lines typed, or less than there was: nothing goes on.
        let grown = length.checked_sub(insert.length);
        let typed = grown
            .and_then(|grown| text.0.get(start + insert.at..start + insert.at + grown))
            .filter(|typed| !typed.is_empty() && !typed.contains('\n'))
            .map(str::to_owned);
        if let Some(typed) = typed {
            for &line in insert.others.iter().rev() {
                let text = Text(host.text());
                if line >= text.line_count() {
                    continue;
                }
                let at = text.line(line);
                let place = if insert.to_end {
                    text.line_end(at)
                } else if insert.fill {
                    self.fill_to(host, at, insert.column)
                } else {
                    let shown = width(text, at);
                    if shown < insert.column {
                        continue;
                    }
                    self.fill_to(host, at, insert.column)
                };
                self.edit(host, place..place, &typed);
            }
        }
        if let Some((line, left)) = corner {
            let text = Text(host.text());
            let at = text.line(line);
            // One more to the right: leaving insert mode steps back.
            let corner = text.at_column(at, left);
            self.cursor = if text.line_is_empty(at) {
                at
            } else {
                text.next(corner)
            };
        }
    }

    /// `~`, `u` and `U` on the block.
    fn block_case(&mut self, c: char, host: &mut impl Host) {
        let Some(pieces) = self.block(host.text()) else {
            return;
        };
        let text = Text(host.text());
        let (top, _, left, _) = self.corners(text);
        let _ = self.take_count();
        self.reset_pending();
        self.mode = Mode::Normal;
        self.more.block = false;
        self.repeatable = false;
        for piece in pieces.into_iter().rev() {
            let now = host.text()[piece.clone()].to_owned();
            let changed: String = now
                .chars()
                .map(|ch| match c {
                    'u' => lower(ch),
                    'U' => upper(ch),
                    _ if ch.is_uppercase() => lower(ch),
                    _ => upper(ch),
                })
                .collect();
            if changed != now {
                self.edit(host, piece, &changed);
            }
        }
        let text = Text(host.text());
        self.cursor = clamp(text, text.at_column(top, left).max(top), false);
        self.wanted_column = None;
    }

    /// `p` and `P` of a register that holds a block: its lines put beside
    /// the cursor, one in each line from the cursor's down, behind it or
    /// before it, `count` times; lines filled up with blanks to where it
    /// goes in, and more lines made where the text has too few. Returns
    /// whether the register held a block.
    pub(super) fn put_block(&mut self, after: bool, host: &mut impl Host) -> bool {
        let name = self.pending.register.unwrap_or('"').to_ascii_lowercase();
        let Some(held) = self
            .registers
            .get(&name)
            .map(|register| register.text.clone())
        else {
            return false;
        };
        if !self
            .more
            .blocks
            .iter()
            .any(|(of, text)| *of == name && *text == held)
        {
            return false;
        }
        let (count, _) = self.take_count();
        self.reset_pending();
        let pieces: Vec<&str> = held.split('\n').collect();
        let widest = pieces
            .iter()
            .map(|piece| piece.chars().count())
            .max()
            .unwrap_or(0);
        let text = Text(host.text());
        let line = text.line_start(self.cursor);
        let empty = text.line_is_empty(self.cursor);
        let column = text.column(self.cursor) + usize::from(after && !empty);
        let first = text.line_index(line);
        for (index, piece) in pieces.iter().enumerate() {
            let text = Text(host.text());
            let wanted = first + index;
            if wanted >= text.line_count() {
                // A line more at the end.
                let end = text
                    .len()
                    .saturating_sub(usize::from(text.0.ends_with('\n')));
                self.edit(host, end..end, "\n");
            }
            let text = Text(host.text());
            let at = text.line(wanted);
            let place = self.fill_to(host, at, column);
            let text = Text(host.text());
            let more = place < text.line_end(at);
            let padded = format!("{piece:<widest$}");
            let mut put = String::new();
            for time in 0..count {
                let last = time + 1 == count;
                put.push_str(if last && !more { piece } else { &padded });
            }
            self.edit(host, place..place, &put);
        }
        let text = Text(host.text());
        let start = text.line(first);
        let mut at = start;
        while at < text.line_end(start) && text.column(at) < column {
            at = text.next(at);
        }
        self.cursor = clamp(text, at, false);
        self.wanted_column = None;
        true
    }
}
