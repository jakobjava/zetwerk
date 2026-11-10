//! The modal state machine: keys in, edits and cursor movements out.

use std::collections::HashMap;
use std::ops::Range;

use crate::keys::{Key, KeyPress};
use crate::motion::{self, Kind};
use crate::object::{self, Object};
use crate::text::Text;

/// Columns one `>` or `<` shifts by, and the width of a typed Tab.
const SHIFT_WIDTH: usize = 2;

/// A replacement of part of the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
}

/// The text the engine edits.
pub trait Host {
    fn text(&self) -> &str;

    /// Applies `edits` one after the other; each range refers to the text
    /// as the edits before it left it. With `join`, the edits become part
    /// of the previous undo step.
    fn edit(&mut self, edits: &[Edit], join: bool);

    /// Takes back one undo step. Returns the part of the text, as it is
    /// afterwards, that the step had changed; `None` if there was nothing
    /// to undo.
    fn undo(&mut self) -> Option<Range<usize>>;

    /// Makes one undone step again. Returns the part of the text, as it is
    /// afterwards, that the step changed.
    fn redo(&mut self) -> Option<Range<usize>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    /// Characterwise visual mode.
    Visual,
    /// Linewise visual mode.
    VisualLine,
    /// The command line is open.
    Command,
}

/// Something the engine asks its host to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Run this command-line command.
    Command(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operator {
    Delete,
    Change,
    Yank,
    Indent,
    Dedent,
    ToggleCase,
    Lower,
    Upper,
}

/// A key that the next key completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Awaiting {
    #[default]
    Nothing,
    /// After `g`.
    G,
    Find {
        forward: bool,
        till: bool,
    },
    /// After `r`.
    Replace,
    /// After `"`.
    Register,
    /// After `i` or `a` where a text object is expected.
    Object {
        around: bool,
    },
}

#[derive(Debug, Default)]
struct Pending {
    /// The count typed before the motion or command.
    count: Option<usize>,
    /// The count typed before the operator.
    operator_count: Option<usize>,
    register: Option<char>,
    operator: Option<Operator>,
    awaiting: Awaiting,
}

impl Pending {
    fn is_idle(&self) -> bool {
        self.count.is_none()
            && self.operator_count.is_none()
            && self.register.is_none()
            && self.operator.is_none()
            && self.awaiting == Awaiting::Nothing
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Register {
    text: String,
    linewise: bool,
}

/// Where a motion leads.
struct Moved {
    to: usize,
    kind: Kind,
    /// What the motion means for the column `j` and `k` aim at.
    column: Column,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Column {
    /// A horizontal move: aim at wherever the cursor ends up.
    Reset,
    /// A vertical move: keep aiming at the same column.
    Keep,
    /// `$`: aim at the end of every line.
    End,
    /// `|`: aim at this column, even if the line was too short for it.
    Exact(usize),
}

/// The last change, as the keys that made it.
#[derive(Debug, Clone)]
struct Recorded {
    count: Option<usize>,
    keys: Vec<KeyPress>,
}

/// A Vim.
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent facts about the command being typed"
)]
pub struct Engine {
    mode: Mode,
    cursor: usize,
    /// The screen column vertical motions aim at; `None` to take the
    /// cursor's.
    wanted_column: Option<usize>,
    pending: Pending,
    registers: HashMap<char, Register>,
    last_find: Option<(char, bool, bool)>,
    /// The column the cursor was aiming at before the motion of the
    /// operator being applied.
    column_before: Option<usize>,
    /// Whether the operator being applied got an exclusive motion that did
    /// not move: nothing at all to work on.
    nothing_selected: bool,
    /// Where undo should bring the cursor back to after the next change,
    /// if that is not simply where the cursor is when it is made.
    undo_cursor: Option<usize>,
    /// For each undo step made in this session, where the cursor was
    /// before it; and the same for steps that were undone.
    undo_cursors: Vec<usize>,
    redo_cursors: Vec<usize>,
    /// The other end of the visual selection.
    anchor: usize,
    /// Where this insert began. `Ctrl-W` and `Ctrl-U` stop here once before
    /// deleting text that was there before.
    insert_start: usize,
    /// How often the text typed in this insert is to be repeated on Escape.
    insert_count: usize,
    /// What was typed in this insert, if it was only typed (no deletions
    /// reaching before its start).
    insert_typed: Option<String>,
    command_line: String,
    /// The keys of the command being typed, counts excluded.
    record: Vec<KeyPress>,
    record_count: Option<usize>,
    /// Whether the command being typed changed the text.
    changed: bool,
    /// Whether an edit of this command has been made, so that further ones
    /// join its undo step.
    joined: bool,
    /// Whether `.` can repeat the command being typed.
    repeatable: bool,
    last_change: Option<Recorded>,
    effects: Vec<Effect>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    #[must_use]
    pub fn new() -> Self {
        Self {
            mode: Mode::Normal,
            cursor: 0,
            wanted_column: None,
            pending: Pending::default(),
            registers: HashMap::new(),
            last_find: None,
            column_before: None,
            nothing_selected: false,
            undo_cursor: None,
            undo_cursors: Vec::new(),
            redo_cursors: Vec::new(),
            anchor: 0,
            insert_start: 0,
            insert_count: 1,
            insert_typed: None,
            command_line: String::new(),
            record: Vec::new(),
            record_count: None,
            changed: false,
            joined: false,
            repeatable: true,
            last_change: None,
            effects: Vec::new(),
        }
    }

    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The cursor, as a byte offset into the host's text.
    #[must_use]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Puts the cursor at `pos`, or the nearest place it may be.
    pub fn set_cursor(&mut self, pos: usize, text: &str) {
        self.cursor = clamp(Text(text), pos, self.mode == Mode::Insert);
        self.wanted_column = None;
    }

    /// Whether an operator is waiting for its motion.
    #[must_use]
    pub fn operator_pending(&self) -> bool {
        self.pending.operator.is_some()
    }

    /// The text on the command line, while it is open.
    #[must_use]
    pub fn command_line(&self) -> Option<&str> {
        (self.mode == Mode::Command).then_some(self.command_line.as_str())
    }

    /// The visual selection, if one is active.
    #[must_use]
    pub fn selection(&self, text: &str) -> Option<Range<usize>> {
        let text = Text(text);
        let (low, high) = (self.anchor.min(self.cursor), self.anchor.max(self.cursor));
        match self.mode {
            Mode::Visual => {
                // The character under the later end is included, and on an
                // empty line that is its line break; except on the last
                // line, whose line break is not there to be taken.
                let on_last_break =
                    text.char_at(high) == Some('\n') && text.next_line(high).is_none();
                Some(low..if on_last_break { high } else { text.next(high) })
            }
            Mode::VisualLine => Some(text.line_start(low)..line_end_inclusive(text, high)),
            _ => None,
        }
    }

    /// The contents of a register and whether they are whole lines.
    #[must_use]
    pub fn register(&self, name: char) -> Option<(&str, bool)> {
        self.registers
            .get(&name)
            .map(|register| (register.text.as_str(), register.linewise))
    }

    /// Handles one key press.
    pub fn key(&mut self, press: KeyPress, host: &mut impl Host) -> Vec<Effect> {
        self.effects.clear();
        self.dispatch(press, host);
        std::mem::take(&mut self.effects)
    }

    fn dispatch(&mut self, press: KeyPress, host: &mut impl Host) {
        if self.mode == Mode::Normal && self.pending.is_idle() {
            // A new command starts.
            self.undo_cursor = None;
            self.record.clear();
            self.record_count = None;
            self.changed = false;
            self.joined = false;
            self.repeatable = true;
        }
        self.record.push(press);
        match self.mode {
            Mode::Normal | Mode::Visual | Mode::VisualLine => self.normal_key(press, host),
            Mode::Insert => self.insert_key(press, host),
            Mode::Command => self.command_key(press, host),
        }
        if self.mode == Mode::Normal && self.pending.is_idle() && self.changed && self.repeatable {
            self.last_change = Some(Recorded {
                count: self.record_count,
                keys: std::mem::take(&mut self.record),
            });
            self.changed = false;
        }
    }

    // ---- Editing helpers -------------------------------------------------

    /// Replaces `range` with `text` as part of the current command.
    fn edit(&mut self, host: &mut impl Host, range: Range<usize>, text: &str) {
        if !self.joined {
            // A new undo step: remember where the cursor is as the change
            // is made, which is where undo will bring it back to.
            self.undo_cursors
                .push(self.undo_cursor.take().unwrap_or(self.cursor));
            self.redo_cursors.clear();
        }
        host.edit(
            &[Edit {
                range,
                text: text.to_owned(),
            }],
            self.joined,
        );
        self.joined = true;
        self.changed = true;
        // A text that is not empty ends with a line break, as Vim writes it.
        let now = host.text();
        if !now.is_empty() && !now.ends_with('\n') {
            let end = now.len();
            host.edit(
                &[Edit {
                    range: end..end,
                    text: "\n".to_owned(),
                }],
                true,
            );
        }
    }

    /// The count for the command at hand: the one before the operator times
    /// the one before the motion. Also whether any was typed.
    fn take_count(&mut self) -> (usize, bool) {
        let typed = self.pending.count.is_some() || self.pending.operator_count.is_some();
        let count = self.pending.operator_count.take().unwrap_or(1)
            * self.pending.count.take().unwrap_or(1);
        self.record_count = typed.then_some(count);
        (count.max(1), typed)
    }

    fn reset_pending(&mut self) {
        self.pending = Pending::default();
    }

    fn move_to(&mut self, text: Text, pos: usize, column: Column) {
        self.cursor = clamp(text, pos, self.mode != Mode::Normal);
        match column {
            Column::Reset => self.wanted_column = None,
            Column::Keep => {}
            Column::End => self.wanted_column = Some(usize::MAX),
            Column::Exact(column) => self.wanted_column = Some(column),
        }
    }

    fn store(&mut self, text: String, linewise: bool, deleted: bool) {
        let register = Register { text, linewise };
        match self.pending.register.take() {
            Some('_') => return,
            Some(name) if name.is_ascii_uppercase() => {
                let target = self.registers.entry(name.to_ascii_lowercase()).or_default();
                if register.linewise && !target.linewise && !target.text.is_empty() {
                    target.text.push('\n');
                }
                target.text.push_str(&register.text);
                target.linewise |= register.linewise;
                let combined = target.clone();
                self.registers.insert('"', combined);
                return;
            }
            Some(name) => {
                self.registers.insert(name, register.clone());
            }
            None if !deleted => {
                self.registers.insert('0', register.clone());
            }
            None if register.linewise || register.text.contains('\n') => {
                for number in (b'1'..b'9').rev() {
                    if let Some(older) = self.registers.remove(&char::from(number)) {
                        self.registers.insert(char::from(number + 1), older);
                    }
                }
                self.registers.insert('1', register.clone());
            }
            None => {
                self.registers.insert('-', register.clone());
            }
        }
        self.registers.insert('"', register);
    }

    // ---- Normal, visual and operator-pending mode -------------------------

    #[allow(clippy::too_many_lines, reason = "one arm per key of the grammar")]
    fn normal_key(&mut self, press: KeyPress, host: &mut impl Host) {
        let visual = matches!(self.mode, Mode::Visual | Mode::VisualLine);
        match std::mem::take(&mut self.pending.awaiting) {
            Awaiting::Nothing => {}
            Awaiting::G => return self.g_key(press, host),
            Awaiting::Register => {
                if let Key::Char(name) = press.key {
                    self.pending.register = Some(name);
                } else {
                    self.reset_pending();
                }
                return;
            }
            Awaiting::Find { forward, till } => {
                let Key::Char(target) = press.key else {
                    return self.reset_pending();
                };
                self.last_find = Some((target, forward, till));
                let (count, _) = self.take_count();
                let found =
                    motion::find(Text(host.text()), self.cursor, count, target, forward, till);
                let kind = if forward {
                    Kind::Inclusive
                } else {
                    Kind::Exclusive
                };
                let moved = found.map(|to| Moved {
                    to,
                    kind,
                    column: Column::Reset,
                });
                return self.finish_motion(moved, host);
            }
            Awaiting::Replace => return self.replace_chars(press, host),
            Awaiting::Object { around } => return self.object_key(press, around, host),
        }

        let Key::Char(c) = press.key else {
            match press.key {
                Key::Escape => {
                    self.reset_pending();
                    if visual {
                        self.mode = Mode::Normal;
                        self.cursor = clamp(Text(host.text()), self.cursor, false);
                    }
                }
                Key::Left | Key::Backspace => self.motion_key('h', host),
                Key::Right => self.motion_key('l', host),
                Key::Up => self.motion_key('k', host),
                Key::Down | Key::Enter => self.motion_key('j', host),
                Key::Home => self.motion_key('0', host),
                Key::End => self.motion_key('$', host),
                _ => self.reset_pending(),
            }
            return;
        };

        if press.ctrl {
            match c {
                'r' if !visual && self.pending.operator.is_none() => {
                    let (count, _) = self.take_count();
                    for _ in 0..count {
                        let Some(changed) = host.redo() else { break };
                        let before = self.redo_cursors.pop();
                        self.undo_cursors.push(before.unwrap_or(changed.start));
                        self.after_undo(&changed, before, host);
                    }
                }
                'c' | '[' => {
                    self.reset_pending();
                    self.mode = Mode::Normal;
                    self.cursor = clamp(Text(host.text()), self.cursor, false);
                }
                _ => self.reset_pending(),
            }
            return;
        }

        // Counts. `0` is a count digit only after another digit.
        if c.is_ascii_digit() && (c != '0' || self.pending.count.is_some()) {
            let digit = c as usize - '0' as usize;
            let count = self
                .pending
                .count
                .unwrap_or(0)
                .saturating_mul(10)
                .saturating_add(digit);
            self.pending.count = Some(count.min(999_999));
            self.record.pop();
            return;
        }

        let operator = match c {
            'd' => Some(Operator::Delete),
            'c' => Some(Operator::Change),
            'y' => Some(Operator::Yank),
            '>' => Some(Operator::Indent),
            '<' => Some(Operator::Dedent),
            _ => None,
        };
        if let Some(operator) = operator {
            return self.operator_key(operator, host);
        }

        if visual {
            let operator = match c {
                'x' | 'X' | 'D' => Some(Operator::Delete),
                's' | 'S' | 'C' => Some(Operator::Change),
                'Y' => Some(Operator::Yank),
                '~' => Some(Operator::ToggleCase),
                'u' => Some(Operator::Lower),
                'U' => Some(Operator::Upper),
                _ => None,
            };
            if let Some(operator) = operator {
                if matches!(c, 'X' | 'D' | 'S' | 'C' | 'Y') {
                    self.mode = Mode::VisualLine;
                }
                return self.operator_key(operator, host);
            }
        }

        if self.pending.operator.is_some() || visual {
            match c {
                'i' | 'a' => {
                    self.pending.awaiting = Awaiting::Object { around: c == 'a' };
                    return;
                }
                // The key that completes a doubled case operator: `guu`.
                'u' if self.pending.operator == Some(Operator::Lower) => {
                    return self.operator_key(Operator::Lower, host);
                }
                'U' if self.pending.operator == Some(Operator::Upper) => {
                    return self.operator_key(Operator::Upper, host);
                }
                '~' if self.pending.operator == Some(Operator::ToggleCase) => {
                    return self.operator_key(Operator::ToggleCase, host);
                }
                _ => {}
            }
        }

        match c {
            '"' => self.pending.awaiting = Awaiting::Register,
            'g' => self.pending.awaiting = Awaiting::G,
            'f' | 'F' | 't' | 'T' => {
                self.pending.awaiting = Awaiting::Find {
                    forward: c.is_lowercase(),
                    till: c.eq_ignore_ascii_case(&'t'),
                };
            }
            'h' | 'l' | 'j' | 'k' | '0' | '^' | '$' | 'w' | 'W' | 'b' | 'B' | 'e' | 'E' | 'G'
            | '{' | '}' | ';' | ',' | '%' | ' ' | '|' | '_' | '+' | '-' => self.motion_key(c, host),
            'v' | 'V' => self.visual_key(c, host),
            'o' if visual => std::mem::swap(&mut self.anchor, &mut self.cursor),
            'J' if visual => self.join_lines(host),
            'r' if visual => self.pending.awaiting = Awaiting::Replace,
            'p' | 'P' if visual => self.put_over_selection(host),
            ':' if !visual => {
                self.reset_pending();
                self.command_line.clear();
                self.mode = Mode::Command;
            }
            _ if visual || self.pending.operator.is_some() => self.reset_pending(),
            _ => self.command_key_normal(c, host),
        }
    }

    /// Commands that only exist in normal mode proper.
    fn command_key_normal(&mut self, c: char, host: &mut impl Host) {
        match c {
            'x' => self.shorthand(Operator::Delete, 'l', host),
            'X' => self.shorthand(Operator::Delete, 'h', host),
            's' => self.shorthand(Operator::Change, 'l', host),
            'D' => self.shorthand(Operator::Delete, '$', host),
            'C' => self.shorthand(Operator::Change, '$', host),
            'S' => {
                self.pending.operator = Some(Operator::Change);
                self.operator_key(Operator::Change, host);
            }
            'Y' => {
                self.pending.operator = Some(Operator::Yank);
                self.operator_key(Operator::Yank, host);
            }
            'p' | 'P' => self.put(c == 'p', host),
            'J' => self.join_lines(host),
            'r' => self.pending.awaiting = Awaiting::Replace,
            '~' => self.toggle_case_chars(host),
            'i' | 'a' | 'I' | 'A' | 'o' | 'O' => self.start_insert(c, host),
            'u' => {
                let (count, _) = self.take_count();
                for _ in 0..count {
                    let Some(changed) = host.undo() else { break };
                    let before = self.undo_cursors.pop();
                    self.redo_cursors.push(before.unwrap_or(changed.start));
                    self.after_undo(&changed, before, host);
                }
            }
            '.' => self.repeat(host),
            _ => self.reset_pending(),
        }
    }

    /// Places the cursor after an undo or redo that changed `changed`.
    /// `before` is where the cursor was when the change was originally
    /// made, if that is known.
    ///
    /// As in Vim, the cursor goes back to that position if it lies among
    /// the changed lines or right next to them, and otherwise to the first
    /// changed line.
    fn after_undo(&mut self, changed: &Range<usize>, before: Option<usize>, host: &impl Host) {
        let text = Text(host.text());
        let first = text.line_start(clamp(text, changed.start, false));
        let last = text.line_start(clamp(text, changed.end.max(changed.start), false));
        let from = text.prev_line(first).unwrap_or(first);
        let to = text.next_line(last).unwrap_or(last);
        if let Some(before) = before.map(|before| clamp(text, before, false))
            && (from..=to).contains(&text.line_start(before))
        {
            self.cursor = before;
            self.wanted_column = None;
            return;
        }
        let column = self
            .wanted_column
            .unwrap_or_else(|| text.column(clamp(text, self.cursor, false)));
        self.cursor = column_in_line(text, first, column);
        self.wanted_column = None;
    }

    /// `x`, `D` and friends: an operator with a fixed motion.
    fn shorthand(&mut self, operator: Operator, motion: char, host: &mut impl Host) {
        self.pending.operator = Some(operator);
        self.motion_key(motion, host);
    }

    fn g_key(&mut self, press: KeyPress, host: &mut impl Host) {
        let Key::Char(c) = press.key else {
            return self.reset_pending();
        };
        match c {
            'g' | 'e' | 'E' | '_' => self.motion_key_g(c, host),
            '~' => self.operator_key(Operator::ToggleCase, host),
            'u' => self.operator_key(Operator::Lower, host),
            'U' => self.operator_key(Operator::Upper, host),
            'J' if self.pending.operator.is_none() => self.join_lines_raw(host),
            _ => self.reset_pending(),
        }
    }

    /// An operator key: starts the operator, or with the operator already
    /// pending (`dd`) or a selection active, applies it.
    fn operator_key(&mut self, operator: Operator, host: &mut impl Host) {
        if matches!(self.mode, Mode::Visual | Mode::VisualLine) {
            let Some(range) = self.selection(host.text()) else {
                return;
            };
            let linewise = self.mode == Mode::VisualLine;
            let (count, _) = self.take_count();
            self.repeatable = false;
            self.mode = Mode::Normal;
            self.pending.operator = None;
            let object = Object {
                start: range.start,
                end: range.end,
                linewise,
            };
            return self.operate(operator, object, count, range.start, host);
        }
        match self.pending.operator {
            None => {
                self.pending.operator = Some(operator);
                self.pending.operator_count = self.pending.count.take();
            }
            // Doubled (`dd`, `guu`): the operator with the motion `_`.
            Some(pending) if pending == operator => self.motion_key('_', host),
            Some(_) => self.reset_pending(),
        }
    }

    fn visual_key(&mut self, c: char, host: &impl Host) {
        let wanted = if c == 'v' {
            Mode::Visual
        } else {
            Mode::VisualLine
        };
        self.pending = Pending::default();
        if self.mode == wanted {
            self.mode = Mode::Normal;
            self.cursor = clamp(Text(host.text()), self.cursor, false);
        } else {
            if self.mode == Mode::Normal {
                self.anchor = self.cursor;
            }
            self.mode = wanted;
        }
    }

    fn object_key(&mut self, press: KeyPress, around: bool, host: &mut impl Host) {
        let Key::Char(c) = press.key else {
            return self.reset_pending();
        };
        let text = Text(host.text());
        let (count, _) = self.take_count();
        let pos = self.cursor;
        let visual = matches!(self.mode, Mode::Visual | Mode::VisualLine);
        if matches!(c, 'w' | 'W') && !visual {
            // Vim takes a word object like a motion from one of its ends
            // to the other, with all that follows from that.
            let Some((start, end, inclusive)) =
                motion::word_object(text, pos, count, around, c == 'W')
            else {
                return self.reset_pending();
            };
            let column = self.wanted_column.unwrap_or_else(|| text.column(pos));
            self.wanted_column = Some(column);
            self.cursor = start;
            let kind = if inclusive {
                Kind::Inclusive
            } else {
                Kind::Exclusive
            };
            let moved = Moved {
                to: end,
                kind,
                column: Column::Keep,
            };
            return self.finish_motion(Some(moved), host);
        }
        let found = match c {
            'w' => object::word(text, pos, around, false, count),
            'W' => object::word(text, pos, around, true, count),
            'p' => object::paragraph(text, pos, around, count),
            '(' | ')' | 'b' => object::bracket(text, pos, '(', ')', around, count),
            '[' | ']' => object::bracket(text, pos, '[', ']', around, count),
            '{' | '}' | 'B' => object::bracket(text, pos, '{', '}', around, count),
            '<' | '>' => object::bracket(text, pos, '<', '>', around, count),
            '"' | '\'' | '`' => object::quote(text, pos, c, around),
            _ => None,
        };
        let Some(found) = found else {
            return self.reset_pending();
        };
        if visual {
            if found.end > found.start {
                self.anchor = found.start;
                self.cursor = text.prev(found.end);
                if found.linewise {
                    self.mode = Mode::VisualLine;
                }
            }
            return;
        }
        let Some(operator) = self.pending.operator.take() else {
            return self.reset_pending();
        };
        let found = if operator == Operator::Delete {
            widen_delete(text, found)
        } else {
            found
        };
        self.operate(operator, found, 1, found.start, host);
    }

    // ---- Motions -----------------------------------------------------------

    fn motion_key(&mut self, c: char, host: &mut impl Host) {
        let moved = self.motion(c, host);
        self.finish_motion(moved, host);
    }

    fn motion_key_g(&mut self, c: char, host: &mut impl Host) {
        let text = Text(host.text());
        let (count, typed) = self.take_count();
        let operating = self.pending.operator.is_some();
        let moved = match c {
            'g' => {
                let line = text.line(if typed { count - 1 } else { 0 });
                Some(self.vertical(text, line))
            }
            'e' | 'E' => {
                let (to, ok) = motion::word_end_backward(text, self.cursor, count, c == 'E');
                self.partial(text, to, ok).map(|to| Moved {
                    to,
                    kind: Kind::Inclusive,
                    column: Column::Reset,
                })
            }
            '_' => match lines_below(text, self.cursor, count - 1) {
                None => {
                    self.wanted_column = Some(usize::MAX);
                    None
                }
                Some(line) => {
                    let trimmed = text.line_text(line).trim_end_matches([' ', '\t']);
                    let to = if trimmed.is_empty() {
                        line
                    } else {
                        text.prev(line + trimmed.len())
                    };
                    Some(Moved {
                        to,
                        kind: Kind::Inclusive,
                        column: if operating {
                            Column::End
                        } else {
                            Column::Reset
                        },
                    })
                }
            },
            _ => None,
        };
        self.finish_motion(moved, host);
    }

    #[allow(clippy::too_many_lines, reason = "one arm per motion")]
    fn motion(&mut self, c: char, host: &impl Host) -> Option<Moved> {
        let text = Text(host.text());
        let pos = self.cursor;
        let (count, typed) = self.take_count();
        let operating = self.pending.operator.is_some();
        let horizontal = |to, kind| {
            Some(Moved {
                to,
                kind,
                column: Column::Reset,
            })
        };
        match c {
            'h' => {
                let start = text.line_start(pos);
                let mut to = pos;
                for _ in 0..count {
                    if to == start {
                        break;
                    }
                    to = text.prev(to);
                }
                // With an operator, not moving is not a failure: the
                // operator gets nothing to work on, which for some
                // (`c`, `>`) still means something.
                (to != pos || operating).then_some(Moved {
                    to,
                    kind: Kind::Exclusive,
                    column: Column::Reset,
                })
            }
            'l' | ' ' => {
                // With an operator the motion may reach the end of the
                // line, so that `dl` deletes the last character.
                let limit = if operating || self.mode != Mode::Normal {
                    text.line_end(pos)
                } else {
                    text.line_last(pos)
                };
                let mut to = pos;
                for _ in 0..count {
                    if to >= limit {
                        break;
                    }
                    to = text.next(to);
                }
                (to != pos || operating).then_some(Moved {
                    to,
                    kind: Kind::Exclusive,
                    column: Column::Reset,
                })
            }
            'j' | 'k' | '+' | '-' | '_' => {
                let steps = if c == '_' { count - 1 } else { count };
                let line = if matches!(c, 'j' | '+' | '_') {
                    lines_below(text, pos, steps)?
                } else {
                    lines_above(text, pos, steps)?
                };
                // `_` puts the cursor on the first non-blank, except for the
                // operators that leave the column alone ('nostartofline').
                let keeps_column = matches!(
                    self.pending.operator,
                    Some(Operator::Yank | Operator::Delete | Operator::Indent | Operator::Dedent)
                );
                if c == '_' && keeps_column {
                    let mut moved = self.vertical(text, line);
                    moved.column = Column::Keep;
                    return Some(moved);
                }
                if matches!(c, '+' | '-' | '_') {
                    let to = text.first_non_blank(line);
                    return Some(Moved {
                        to,
                        kind: Kind::Linewise,
                        column: Column::Reset,
                    });
                }
                Some(self.vertical(text, line))
            }
            '0' => horizontal(text.line_start(pos), Kind::Exclusive),
            '^' => horizontal(text.first_non_blank(pos), Kind::Exclusive),
            '|' => Some(Moved {
                to: text.at_column(text.line_start(pos), count - 1),
                kind: Kind::Exclusive,
                column: Column::Exact(count - 1),
            }),
            '$' => {
                let Some(line) = lines_below(text, pos, count - 1) else {
                    // Even a `$` that cannot go down leaves the cursor
                    // aiming at line ends.
                    self.wanted_column = Some(usize::MAX);
                    return None;
                };
                // With an operator the motion reaches the end of the line,
                // where there is no character to include; without one the
                // cursor stops on the last character.
                let to = if operating {
                    text.line_end(line)
                } else {
                    text.line_last(line)
                };
                Some(Moved {
                    to,
                    kind: Kind::Inclusive,
                    column: Column::End,
                })
            }
            'w' | 'W' => {
                let big = c == 'W';
                if self.pending.operator == Some(Operator::Change)
                    && !matches!(text.char_at(pos), None | Some(' ' | '\t' | '\n'))
                {
                    // `cw` on a word changes to the end of the word, not up
                    // to the next one.
                    let (to, _) = motion::word_end(text, pos, count, big, true);
                    let to = if matches!(text.char_at(to), None | Some('\n')) {
                        text.line_last(to)
                    } else {
                        to
                    };
                    return horizontal(to, Kind::Inclusive);
                }
                let (to, ok) = motion::word_forward(text, pos, count, big, operating);
                if !ok && !operating && to == pos {
                    return None;
                }
                if matches!(text.char_at(to), None | Some('\n'))
                    && to > text.line_start(to)
                    && to > pos
                {
                    // Ended on the end of a line: step back onto its last
                    // character and include it.
                    return horizontal(text.prev(to), Kind::Inclusive);
                }
                horizontal(to, Kind::Exclusive)
            }
            'b' | 'B' => {
                let (to, ok) = motion::word_backward(text, pos, count, c == 'B');
                horizontal(self.partial(text, to, ok)?, Kind::Exclusive)
            }
            'e' | 'E' => {
                let (to, ok) = motion::word_end(text, pos, count, c == 'E', false);
                if operating {
                    // Unlike `b` and `ge`, a pending operator is applied to
                    // however far `e` got.
                    let to = if matches!(text.char_at(to), None | Some('\n')) {
                        text.line_last(to)
                    } else {
                        to
                    };
                    return horizontal(to, Kind::Inclusive);
                }
                horizontal(self.partial(text, to, ok)?, Kind::Inclusive)
            }
            'G' => {
                let line = if typed {
                    text.line(count - 1)
                } else {
                    text.last_line()
                };
                Some(self.vertical(text, line))
            }
            '{' | '}' => {
                let (to, inclusive) = motion::paragraph(text, pos, count, c == '}')?;
                horizontal(
                    to,
                    if inclusive {
                        Kind::Inclusive
                    } else {
                        Kind::Exclusive
                    },
                )
            }
            ';' | ',' => {
                let (target, forward, till) = self.last_find?;
                let forward = forward == (c == ';');
                // Repeating `t` from right before its target would not
                // move; Vim looks past that target instead.
                let mut from = pos;
                if till {
                    let step = if forward {
                        text.next(pos)
                    } else {
                        text.prev(pos)
                    };
                    if text.char_at(step) == Some(target) {
                        from = step;
                    }
                }
                let to = motion::find(text, from, count, target, forward, till)?;
                horizontal(
                    to,
                    if forward {
                        Kind::Inclusive
                    } else {
                        Kind::Exclusive
                    },
                )
            }
            '%' if typed => {
                // With a count, `%` goes to that percentage of the text.
                if count > 100 {
                    return None;
                }
                let line = (count * text.line_count()).div_ceil(100);
                Some(self.vertical(text, text.line(line.max(1) - 1)))
            }
            '%' => horizontal(motion::matching_bracket(text, pos)?, Kind::Inclusive),
            _ => None,
        }
    }

    /// Where a vertical motion to the line at `line` ends: at the column
    /// the cursor has been aiming at.
    fn vertical(&mut self, text: Text, line: usize) -> Moved {
        let column = self
            .wanted_column
            .unwrap_or_else(|| text.column(self.cursor));
        self.wanted_column = Some(column);
        let to = if column == usize::MAX {
            text.line_last(line)
        } else {
            text.at_column(line, column)
        };
        Moved {
            to,
            kind: Kind::Linewise,
            column: Column::Keep,
        }
    }

    /// What to make of a word motion that got to `to`. If it got as far as
    /// asked, that is where it leads. If it ran into the end of the text,
    /// the cursor still goes there, but a pending operator is not applied.
    fn partial(&mut self, text: Text, to: usize, complete: bool) -> Option<usize> {
        if complete || self.pending.operator.is_none() {
            let to = if matches!(text.char_at(to), None | Some('\n')) {
                text.line_last(to)
            } else {
                to
            };
            return (to != self.cursor || complete).then_some(to);
        }
        self.cursor = clamp(text, to, false);
        self.wanted_column = None;
        None
    }

    /// Moves the cursor along a motion, or applies the pending operator
    /// over it.
    fn finish_motion(&mut self, moved: Option<Moved>, host: &mut impl Host) {
        let Some(moved) = moved else {
            return self.reset_pending();
        };
        let text = Text(host.text());
        let Some(operator) = self.pending.operator.take() else {
            self.reset_pending();
            return self.move_to(text, moved.to, moved.column);
        };
        // A linewise delete puts the cursor in the column it was aiming at
        // before the motion, whatever the motion did to that.
        self.column_before = self
            .wanted_column
            .or_else(|| Some(text.column(self.cursor)));
        // Motions that set the column to aim at do so before the operator
        // looks at it; the others leave it to be worked out afterwards.
        match moved.column {
            Column::End => self.wanted_column = Some(usize::MAX),
            Column::Exact(column) => self.wanted_column = Some(column),
            Column::Reset | Column::Keep => {}
        }
        let (mut start, mut end) = (self.cursor.min(moved.to), self.cursor.max(moved.to));
        let mut kind = moved.kind;
        self.nothing_selected = kind == Kind::Exclusive && start == end;
        // An exclusive motion that ends at the start of a line does not
        // reach into that line (`:help exclusive`).
        if kind == Kind::Exclusive && end > start && text.line_start(end) == end {
            let spans_lines = text.line_start(start) != end;
            if spans_lines {
                let indent_only = text
                    .0
                    .get(text.line_start(start)..start)
                    .is_some_and(|before| before.chars().all(|c| c == ' ' || c == '\t'));
                end -= 1;
                if indent_only {
                    kind = Kind::Linewise;
                }
            }
        }
        if kind == Kind::Inclusive && !matches!(text.char_at(end), None | Some('\n')) {
            // On the end of a line there is no character to include.
            end = text.next(end);
        }
        let linewise = kind == Kind::Linewise;
        if linewise {
            start = text.line_start(start);
            end = line_end_inclusive(text, end);
        }
        self.reset_pending_keep_register();
        let landing = self.cursor.min(moved.to);
        let mut object = Object {
            start,
            end,
            linewise,
        };
        if operator == Operator::Delete {
            object = widen_delete(text, object);
        }
        self.operate(operator, object, 1, landing, host);
    }

    fn reset_pending_keep_register(&mut self) {
        let register = self.pending.register.take();
        self.pending = Pending {
            register,
            ..Pending::default()
        };
    }

    // ---- Operators ---------------------------------------------------------

    /// Applies `operator` to `object`. `landing` is where the cursor goes
    /// after an operator that leaves lines in place: the earlier end of
    /// the motion.
    #[allow(clippy::too_many_lines, reason = "one arm per operator")]
    fn operate(
        &mut self,
        operator: Operator,
        object: Object,
        count: usize,
        landing: usize,
        host: &mut impl Host,
    ) {
        let Object {
            start,
            end,
            linewise,
        } = object;
        let selected = host.text().get(start..end).unwrap_or("").to_owned();
        // An exclusive motion that did not move gives the operator nothing;
        // an inclusive one at the end of a line gives it an empty text,
        // which deleting still puts into the register.
        let nothing_selected = std::mem::take(&mut self.nothing_selected);
        if operator != Operator::Yank {
            // Vim puts the cursor at the start of the text before it
            // changes it, and that is where undo returns to. One oddity is
            // kept: changing several whole lines is done from the second
            // of them.
            let text = Text(host.text());
            let landing = clamp(text, landing, false);
            let second = text.next_line(start).filter(|&line| line < end);
            self.undo_cursor = Some(match second {
                Some(second) if linewise && operator == Operator::Change => {
                    text.at_column(second, text.column(landing))
                }
                _ => landing,
            });
        }
        // After a linewise delete or shift the cursor keeps the column it
        // was aiming at ('nostartofline', Neovim's default).
        let column = self
            .wanted_column
            .unwrap_or_else(|| Text(host.text()).column(self.cursor));
        match operator {
            Operator::Yank => {
                let mut yanked = selected;
                if linewise && !yanked.ends_with('\n') {
                    yanked.push('\n');
                }
                self.store(yanked, linewise, false);
                self.cursor = clamp(Text(host.text()), landing, false);
                if !linewise {
                    self.wanted_column = None;
                }
            }
            Operator::Delete => {
                if selected.is_empty() {
                    // Deleting nothing leaves even the registers alone.
                    return self.reset_pending();
                }
                let mut deleted = selected;
                if linewise && !deleted.ends_with('\n') {
                    deleted.push('\n');
                }
                self.store(deleted, linewise, true);
                self.edit(host, start..end, "");
                let text = Text(host.text());
                if linewise {
                    let line = if start >= text.len() {
                        text.last_line()
                    } else {
                        start
                    };
                    let column = self.column_before.take().unwrap_or(column);
                    self.cursor = column_in_line(text, line, column);
                    self.wanted_column = Some(column);
                } else {
                    self.cursor = clamp(text, start, false);
                    self.wanted_column = None;
                }
            }
            Operator::Change => {
                let mut changed = selected;
                if linewise {
                    // The lines go, one empty line stays to type into.
                    if !changed.ends_with('\n') {
                        changed.push('\n');
                    }
                    let content_end = if host.text().get(..end).is_some_and(|t| t.ends_with('\n')) {
                        end - 1
                    } else {
                        end
                    };
                    self.store(changed, true, true);
                    self.edit(host, start..content_end, "");
                } else {
                    if !(changed.is_empty() && nothing_selected) {
                        self.store(changed, false, true);
                    }
                    self.edit(host, start..end, "");
                }
                self.cursor = start;
                self.enter_insert(1);
            }
            Operator::Indent | Operator::Dedent => {
                let first = Text(host.text()).line_start(start);
                let lines = selected
                    .strip_suffix('\n')
                    .unwrap_or(&selected)
                    .matches('\n')
                    .count()
                    + 1;
                let mut line = first;
                for index in 0..lines {
                    for _ in 0..count {
                        let line_text = Text(host.text()).line_text(line);
                        if operator == Operator::Indent {
                            if !line_text.is_empty() {
                                self.edit(host, line..line, &" ".repeat(SHIFT_WIDTH));
                            }
                        } else {
                            let indent = line_text
                                .chars()
                                .take(SHIFT_WIDTH)
                                .take_while(|c| *c == ' ')
                                .count();
                            if indent > 0 {
                                self.edit(host, line..line + indent, "");
                            }
                        }
                    }
                    if index + 1 < lines {
                        match Text(host.text()).next_line(line) {
                            Some(next) => line = next,
                            None => break,
                        }
                    }
                }
                // The cursor stays in the column it was aiming at, on the
                // first line shifted.
                self.cursor = column_in_line(Text(host.text()), first, column);
                self.wanted_column = Some(column);
                self.changed = true;
            }
            Operator::ToggleCase | Operator::Lower | Operator::Upper => {
                let converted: String = selected
                    .chars()
                    .map(|c| match operator {
                        Operator::Lower => lower(c),
                        Operator::Upper => upper(c),
                        _ if c.is_lowercase() => upper(c),
                        _ => lower(c),
                    })
                    .collect();
                if converted != selected {
                    self.edit(host, start..end, &converted);
                }
                self.changed = true;
                self.cursor = clamp(Text(host.text()), landing, false);
                if !linewise {
                    self.wanted_column = None;
                }
            }
        }
        // A register named for this operator is used up, whether the
        // operator stored anything or not.
        self.pending.register = None;
    }

    // ---- Other normal-mode commands ---------------------------------------

    fn put(&mut self, after: bool, host: &mut impl Host) {
        let (count, _) = self.take_count();
        let name = self.pending.register.take().unwrap_or('"');
        self.reset_pending();
        let Some(register) = self.registers.get(&name.to_ascii_lowercase()).cloned() else {
            return;
        };
        if register.text.is_empty() {
            return;
        }
        let inserted = register.text.repeat(count);
        if register.linewise {
            // An empty text is one empty line without its line break.
            if host.text().is_empty() {
                self.edit(host, 0..0, "\n");
            }
            let text = Text(host.text());
            let at = if after {
                (text.line_end(self.cursor) + 1).min(text.len())
            } else {
                text.line_start(self.cursor)
            };
            self.edit(host, at..at, &inserted);
            self.cursor = Text(host.text()).first_non_blank(at);
        } else {
            let text = Text(host.text());
            let at = if after && !text.line_is_empty(self.cursor) {
                text.next(self.cursor)
            } else {
                self.cursor
            };
            self.edit(host, at..at, &inserted);
            let text = Text(host.text());
            self.cursor = if inserted.contains('\n') {
                at
            } else {
                text.prev(at + inserted.len())
            };
            self.cursor = clamp(text, self.cursor, false);
        }
        self.wanted_column = None;
    }

    fn put_over_selection(&mut self, host: &mut impl Host) {
        let name = self.pending.register.take().unwrap_or('"');
        let Some(range) = self.selection(host.text()) else {
            return;
        };
        let linewise_selection = self.mode == Mode::VisualLine;
        let register = self
            .registers
            .get(&name.to_ascii_lowercase())
            .cloned()
            .unwrap_or_default();
        self.mode = Mode::Normal;
        self.repeatable = false;
        self.reset_pending();
        let replaced = host.text().get(range.clone()).unwrap_or("").to_owned();
        let mut inserted = register.text.clone();
        if linewise_selection && !register.linewise {
            inserted.push('\n');
        } else if !linewise_selection && register.linewise {
            inserted.insert(0, '\n');
        }
        self.edit(host, range.clone(), &inserted);
        self.store(replaced, linewise_selection, true);
        let text = Text(host.text());
        let landed = if register.linewise {
            let line = if linewise_selection {
                range.start
            } else {
                range.start + 1
            };
            text.first_non_blank(line.min(text.len()))
        } else if inserted.contains('\n') && !linewise_selection {
            range.start
        } else {
            text.prev(range.start + register.text.len())
                .max(range.start)
        };
        self.cursor = clamp(text, landed, false);
        self.wanted_column = None;
    }

    /// `J`: joins lines with a space between them.
    fn join_lines(&mut self, host: &mut impl Host) {
        self.join(true, host);
    }

    /// `gJ`: joins lines as they are.
    fn join_lines_raw(&mut self, host: &mut impl Host) {
        self.join(false, host);
    }

    fn join(&mut self, spaced: bool, host: &mut impl Host) {
        let (count, _) = self.take_count();
        let mut joins = count.max(2) - 1;
        if matches!(self.mode, Mode::Visual | Mode::VisualLine) {
            let text = Text(host.text());
            let (low, high) = (self.anchor.min(self.cursor), self.anchor.max(self.cursor));
            let lines = text.line_index(high) - text.line_index(low);
            joins = lines.max(1);
            self.cursor = low;
            self.mode = Mode::Normal;
            self.repeatable = false;
        }
        self.reset_pending();
        // Joining fewer lines than asked for is fine, as long as there
        // is at least one to join.
        if Text(host.text()).next_line(self.cursor).is_none() {
            if count > 2 {
                // Vim joins "as many lines as there are", which is none,
                // and still puts the cursor where a join would have.
                self.cursor = Text(host.text()).line_start(self.cursor);
                self.wanted_column = None;
            }
            return;
        }
        for _ in 0..joins {
            let text = Text(host.text());
            let end = text.line_end(self.cursor);
            let Some(next) = text.next_line(self.cursor) else {
                break;
            };
            if !spaced {
                self.edit(host, end..next, "");
                self.cursor = clamp(Text(host.text()), end, false);
                continue;
            }
            let next_text = text.line_text(next);
            let trimmed = next_text.trim_start_matches([' ', '\t']);
            let resume = next + (next_text.len() - trimmed.len());
            let current = text.line_text(self.cursor);
            let space = !(trimmed.is_empty()
                || trimmed.starts_with(')')
                || current.is_empty()
                || current.ends_with([' ', '\t']));
            self.edit(host, end..resume, if space { " " } else { "" });
            self.cursor = clamp(Text(host.text()), end, false);
        }
        self.wanted_column = None;
    }

    /// `r`: replaces characters with the next key typed.
    fn replace_chars(&mut self, press: KeyPress, host: &mut impl Host) {
        let replacement = match press.key {
            Key::Char(c) if !press.ctrl => c.to_string(),
            Key::Enter => "\n".to_owned(),
            _ => return self.reset_pending(),
        };
        let text = Text(host.text());
        if matches!(self.mode, Mode::Visual | Mode::VisualLine) {
            let Some(range) = self.selection(host.text()) else {
                return;
            };
            let replaced: String = host
                .text()
                .get(range.clone())
                .unwrap_or("")
                .chars()
                .map(|c| {
                    if c == '\n' {
                        "\n".to_owned()
                    } else {
                        replacement.clone()
                    }
                })
                .collect();
            self.mode = Mode::Normal;
            self.repeatable = false;
            self.reset_pending();
            self.edit(host, range.clone(), &replaced);
            self.cursor = clamp(Text(host.text()), range.start, false);
            self.wanted_column = None;
            return;
        }
        let (count, _) = self.take_count();
        self.reset_pending();
        let line_end = text.line_end(self.cursor);
        let mut end = self.cursor;
        for _ in 0..count {
            if end >= line_end {
                // Not enough characters: nothing is replaced.
                return;
            }
            end = text.next(end);
        }
        if replacement == "\n" {
            self.edit(host, self.cursor..end, "\n");
            self.cursor = clamp(Text(host.text()), self.cursor + 1, false);
        } else {
            let inserted = replacement.repeat(count);
            self.edit(host, self.cursor..end, &inserted);
            let text = Text(host.text());
            self.cursor = text.prev(self.cursor + inserted.len());
        }
        self.wanted_column = None;
    }

    /// `~`: switches the case of characters and moves over them.
    fn toggle_case_chars(&mut self, host: &mut impl Host) {
        let (count, _) = self.take_count();
        self.reset_pending();
        let text = Text(host.text());
        let line_end = text.line_end(self.cursor);
        if self.cursor >= line_end {
            return;
        }
        let mut end = self.cursor;
        for _ in 0..count {
            if end >= line_end {
                break;
            }
            end = text.next(end);
        }
        let original = host.text().get(self.cursor..end).unwrap_or("").to_owned();
        let toggled: String = original
            .chars()
            .map(|c| if c.is_lowercase() { upper(c) } else { lower(c) })
            .collect();
        if toggled != original {
            self.edit(host, self.cursor..end, &toggled);
        }
        self.changed = true;
        let text = Text(host.text());
        self.cursor = clamp(text, self.cursor + toggled.len(), false);
        self.wanted_column = None;
    }

    /// `.`: repeats the last change, with a new count if one was typed.
    fn repeat(&mut self, host: &mut impl Host) {
        let (count, typed) = self.take_count();
        self.reset_pending();
        let Some(recorded) = self.last_change.clone() else {
            return;
        };
        let count = if typed { Some(count) } else { recorded.count };
        // The repeated command is recorded afresh as it runs.
        self.record.clear();
        self.record_count = None;
        self.changed = false;
        self.joined = false;
        if let Some(count) = count {
            for digit in count.to_string().chars() {
                self.normal_key(KeyPress::plain(Key::Char(digit)), host);
            }
        }
        for press in recorded.keys {
            self.dispatch_recorded(press, host);
        }
    }

    /// Like [`Engine::dispatch`], for keys that are replayed rather than
    /// typed.
    fn dispatch_recorded(&mut self, press: KeyPress, host: &mut impl Host) {
        self.record.push(press);
        match self.mode {
            Mode::Normal | Mode::Visual | Mode::VisualLine => self.normal_key(press, host),
            Mode::Insert => self.insert_key(press, host),
            Mode::Command => self.command_key(press, host),
        }
        if self.mode == Mode::Normal && self.pending.is_idle() && self.changed && self.repeatable {
            self.last_change = Some(Recorded {
                count: self.record_count,
                keys: self.record.clone(),
            });
        }
    }

    // ---- Insert mode -------------------------------------------------------

    fn enter_insert(&mut self, count: usize) {
        self.mode = Mode::Insert;
        self.insert_start = self.cursor;
        self.insert_count = count;
        self.insert_typed = Some(String::new());
        self.wanted_column = None;
        self.changed = true;
    }

    fn start_insert(&mut self, c: char, host: &mut impl Host) {
        let (count, _) = self.take_count();
        self.reset_pending();
        let text = Text(host.text());
        match c {
            'a' => {
                if !text.line_is_empty(self.cursor) {
                    self.cursor = text.next(self.cursor);
                }
            }
            'I' => self.cursor = first_non_blank_or_end(text, self.cursor),
            'A' => self.cursor = text.line_end(self.cursor),
            'o' => {
                if host.text().is_empty() {
                    self.edit(host, 0..0, "\n");
                }
                let end = Text(host.text()).line_end(self.cursor);
                self.edit(host, end..end, "\n");
                self.cursor = end + 1;
            }
            'O' => {
                if host.text().is_empty() {
                    // The one empty line gets its line break, so that the
                    // new line is another one.
                    self.edit(host, 0..0, "\n");
                }
                let start = Text(host.text()).line_start(self.cursor);
                self.edit(host, start..start, "\n");
                self.cursor = start;
            }
            _ => {}
        }
        // Opening a line is repeated by repeating the whole command, which
        // this does not do; a count on `o` and `O` opens one line.
        self.enter_insert(if matches!(c, 'o' | 'O') { 1 } else { count });
    }

    fn insert_key(&mut self, press: KeyPress, host: &mut impl Host) {
        let text = Text(host.text());
        match (press.key, press.ctrl) {
            (Key::Escape, _) | (Key::Char('c' | '['), true) => self.leave_insert(host),
            (Key::Char('w'), true) => {
                let start = text.line_start(self.cursor);
                if self.cursor == start {
                    return self.insert_backspace(host);
                }
                // Back over blanks, then over one word or one run of
                // punctuation.
                let mut to = self.cursor;
                while to > start && matches!(text.char_at(text.prev(to)), Some(' ' | '\t')) {
                    to = text.prev(to);
                }
                if to > start {
                    to = motion::word_backward(text, to, 1, false).0.max(start);
                }
                let to = self.stop_at_insert_start(host.text(), to);
                self.insert_delete(to, host);
            }
            (Key::Char('u'), true) => {
                let start = text.line_start(self.cursor);
                if self.cursor == start {
                    return self.insert_backspace(host);
                }
                let indent = text.first_non_blank(self.cursor);
                let to = if indent < self.cursor && !text.line_text(self.cursor).trim().is_empty() {
                    indent
                } else {
                    start
                };
                let to = self.stop_at_insert_start(host.text(), to);
                self.insert_delete(to, host);
            }
            (Key::Char(c), false) => self.insert_text(&c.to_string(), host),
            (Key::Enter, _) => self.insert_text("\n", host),
            (Key::Tab, _) => {
                let column = text.column(self.cursor);
                self.insert_text(&" ".repeat(SHIFT_WIDTH - column % SHIFT_WIDTH), host);
            }
            (Key::Backspace, _) => self.insert_backspace(host),
            (Key::Left | Key::Right | Key::Up | Key::Down | Key::Home | Key::End, _) => {
                // Moving the cursor ends what one undo, one repeat and one
                // count apply to.
                let to = match press.key {
                    Key::Left if self.cursor > text.line_start(self.cursor) => {
                        text.prev(self.cursor)
                    }
                    Key::Right if self.cursor < text.line_end(self.cursor) => {
                        text.next(self.cursor)
                    }
                    Key::Up => text.prev_line(self.cursor).map_or(self.cursor, |line| {
                        insert_column(text, line, text.column(self.cursor))
                    }),
                    Key::Down => text.next_line(self.cursor).map_or(self.cursor, |line| {
                        insert_column(text, line, text.column(self.cursor))
                    }),
                    Key::Home => text.line_start(self.cursor),
                    Key::End => text.line_end(self.cursor),
                    _ => self.cursor,
                };
                self.cursor = to;
                self.joined = false;
                self.insert_typed = None;
                self.insert_count = 1;
                self.repeatable = false;
            }
            _ => {}
        }
    }

    fn insert_text(&mut self, typed: &str, host: &mut impl Host) {
        self.edit(host, self.cursor..self.cursor, typed);
        self.cursor += typed.len();
        if let Some(so_far) = &mut self.insert_typed {
            so_far.push_str(typed);
        }
    }

    fn insert_backspace(&mut self, host: &mut impl Host) {
        if self.cursor == 0 {
            return;
        }
        let text = Text(host.text());
        let joins_lines = text.line_start(self.cursor) == self.cursor;
        let to = text.prev(self.cursor);
        self.insert_delete(to, host);
        if joins_lines {
            // Backspacing onto the line before moves the start of the
            // insert there, as in Vim.
            self.insert_start = self.cursor;
        }
    }

    /// Where a deletion back to `to` really ends: at the start of the
    /// insert, if it would cross it. Once the cursor is at or before that
    /// start, nothing holds it back.
    fn stop_at_insert_start(&self, text: &str, to: usize) -> usize {
        // Vim deletes character by character and stops when it stands
        // exactly on the start; a start that earlier deletions left in
        // the middle of a character is stepped over.
        if self.cursor > self.insert_start
            && to < self.insert_start
            && text.is_char_boundary(self.insert_start)
        {
            self.insert_start
        } else {
            to
        }
    }

    /// Deletes from `to` up to the cursor.
    fn insert_delete(&mut self, to: usize, host: &mut impl Host) {
        let removed = self.cursor - to;
        match &mut self.insert_typed {
            Some(typed) if typed.len() >= removed => typed.truncate(typed.len() - removed),
            // Deleted text that was there before the insert began: the
            // insert is no longer just typed text.
            _ => self.insert_typed = None,
        }
        self.edit(host, to..self.cursor, "");
        self.cursor = to;
    }

    fn leave_insert(&mut self, host: &mut impl Host) {
        if self.insert_count > 1
            && let Some(typed) = self.insert_typed.take()
            && !typed.is_empty()
        {
            let repeated = typed.repeat(self.insert_count - 1);
            self.edit(host, self.cursor..self.cursor, &repeated);
            self.cursor += repeated.len();
        }
        self.mode = Mode::Normal;
        self.reset_pending();
        let text = Text(host.text());
        if self.cursor > text.line_start(self.cursor) {
            self.cursor = text.prev(self.cursor);
        }
        self.cursor = clamp(text, self.cursor, false);
        self.wanted_column = None;
    }

    // ---- Command line ------------------------------------------------------

    fn command_key(&mut self, press: KeyPress, host: &mut impl Host) {
        match press.key {
            Key::Escape => self.mode = Mode::Normal,
            Key::Char('c' | '[') if press.ctrl => self.mode = Mode::Normal,
            Key::Backspace => {
                if self.command_line.pop().is_none() {
                    self.mode = Mode::Normal;
                }
            }
            Key::Enter => {
                self.mode = Mode::Normal;
                let command = std::mem::take(&mut self.command_line);
                let command = command.trim();
                if let Ok(line) = command.parse::<usize>() {
                    let text = Text(host.text());
                    let start = text.line(line.saturating_sub(1));
                    self.cursor = text.first_non_blank(start);
                    self.wanted_column = None;
                } else if !command.is_empty() {
                    self.effects.push(Effect::Command(command.to_owned()));
                }
            }
            Key::Char(c) if !press.ctrl && !press.alt => self.command_line.push(c),
            _ => {}
        }
    }
}

/// The nearest position to `pos` the cursor may be at: in insert and visual
/// mode anywhere on a line including its end, in normal mode on a
/// character.
fn clamp(text: Text, pos: usize, allow_line_end: bool) -> usize {
    let mut pos = pos.min(text.len());
    while !text.0.is_char_boundary(pos) {
        pos -= 1;
    }
    if pos == text.len() && pos > 0 && text.0.ends_with('\n') {
        // Past the final line break is not a line.
        pos = text.last_line();
    }
    if allow_line_end {
        pos
    } else {
        pos.min(text.line_last(pos))
    }
}

/// The end of the line containing `pos`, with its line break.
fn line_end_inclusive(text: Text, pos: usize) -> usize {
    (text.line_end(pos) + 1).min(text.len())
}

/// Where `I` starts inserting: the first non-blank, or the end of a line of
/// blanks.
fn first_non_blank_or_end(text: Text, pos: usize) -> usize {
    let start = text.line_start(pos);
    let line = text.line_text(pos);
    start
        + line
            .find(|c: char| c != ' ' && c != '\t')
            .unwrap_or(line.len())
}

/// The position at screen column `column` of the line at `line`, where the
/// end of the line is a valid position.
fn insert_column(text: Text, line: usize, column: usize) -> usize {
    let at = text.at_column(line, column);
    if text.column(at) < column {
        text.line_end(line)
    } else {
        at
    }
}

fn upper(c: char) -> char {
    let mut upper = c.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(single), None) => single,
        // `ß` and its like have no single uppercase character; Vim leaves
        // them alone.
        _ => c,
    }
}

fn lower(c: char) -> char {
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(single), None) => single,
        _ => c,
    }
}

/// The start of the line `steps` lines below the one containing `pos`, or
/// of the last line if there are fewer. `None` if a move was asked for and
/// `pos` is on the last line already: Vim moves as far as it can, but not
/// moving at all is a failure.
fn lines_below(text: Text, pos: usize, steps: usize) -> Option<usize> {
    let mut line = text.line_start(pos);
    for step in 0..steps {
        match text.next_line(line) {
            Some(next) => line = next,
            None if step == 0 => return None,
            None => break,
        }
    }
    Some(line)
}

/// Like [`lines_below`], upwards.
fn lines_above(text: Text, pos: usize, steps: usize) -> Option<usize> {
    let mut line = text.line_start(pos);
    for step in 0..steps {
        match text.prev_line(line) {
            Some(previous) => line = previous,
            None if step == 0 => return None,
            None => break,
        }
    }
    Some(line)
}

/// The position in the line at `line` for a cursor aiming at screen column
/// `column`, where `usize::MAX` aims at the end.
fn column_in_line(text: Text, line: usize, column: usize) -> usize {
    if column == usize::MAX {
        text.line_last(line)
    } else {
        text.at_column(line, column)
    }
}

/// Vim's rule for a characterwise delete over several lines: if only blanks
/// stand before it on its first line and after it on its last, it takes the
/// whole lines.
fn widen_delete(text: Text, object: Object) -> Object {
    let blank = |range: Range<usize>| {
        text.0
            .get(range)
            .is_some_and(|slice| slice.chars().all(|c| c == ' ' || c == '\t'))
    };
    let Object {
        start,
        end,
        linewise,
    } = object;
    let Some(selected) = text.0.get(start..end) else {
        return object;
    };
    if linewise || !selected.contains('\n') || !blank(text.line_start(start)..start) {
        return object;
    }
    if !blank(end..text.line_end(end)) {
        return object;
    }
    let end = line_end_inclusive(text, end);
    Object {
        start: text.line_start(start),
        end,
        linewise: true,
    }
}
