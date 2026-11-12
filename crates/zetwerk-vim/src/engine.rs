//! The modal state machine: keys in, edits and cursor movements out.

use std::collections::{HashMap, VecDeque};
use std::ops::Range;

use crate::keys::{Key, KeyPress};
use crate::motion::{self, Kind};
use crate::object::{self, Object};
use crate::text::{Class, Text};

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

    /// Where the cursor comes to when it goes `rows` rows down (up, if
    /// negative) from byte `from` on what the host shows: the rows of a
    /// wrapped line, or the lines of a typeset page. `across` is how far
    /// from the left the cursor is aiming, in the host's own measure; if
    /// it is `None`, that is where `from` is shown. Returns the byte and
    /// how far across the cursor aimed.
    ///
    /// `None` if the host shows no rows, or does not show `from`. `gj`
    /// and `gk` then go by lines of the text.
    fn row(&self, from: usize, rows: isize, across: Option<f64>) -> Option<(usize, f64)> {
        let _ = (from, rows, across);
        None
    }

    /// The edits that set bytes `range` of the text off as `format`, or
    /// that take that away again where it is there already. Their ranges
    /// are ranges of the text as it is, in its order and apart from each
    /// other. `None` if the host has no such thing or it cannot be done
    /// there.
    fn format(&self, format: Format, range: Range<usize>) -> Option<Vec<Edit>> {
        let _ = (format, range);
        None
    }

    /// What the word before byte `at` stands for, if it is the name of a
    /// snippet where it is: the bytes to replace, the text to put there,
    /// and how far into that text the cursor goes. Asked when Tab is
    /// pressed in insert mode.
    fn expand(&self, at: usize) -> Option<(Range<usize>, String, usize)> {
        let _ = at;
        None
    }

    /// A text object of the host's language around byte `at`, named by
    /// the key after `i` (the inside) or `a` (`around`: all of it). The
    /// host says which keys those are; `None` for all others, and where
    /// there is no such object.
    fn object(&self, name: char, at: usize, around: bool) -> Option<Range<usize>> {
        let _ = (name, at, around);
        None
    }
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
    /// Scroll the view so that the cursor's line is here in it.
    Scroll(Place),
}

/// Where in the view a line should be put.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Top,
    Middle,
    Bottom,
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
    /// Not Vim's: set the text off the way the host's language does.
    Format(Format),
}

/// A way to set text off that only the host knows how to write: the
/// engine knows keys and ranges, not markup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// `gb`: strong (bold).
    Strong,
    /// `gm`: emphasis.
    Emph,
    /// `gt`: raw (typewriter) text.
    Raw,
    /// `gc`: a comment, line by line.
    Comment,
    /// `gl`: items of a list, line by line.
    ListItem,
    /// `g+` and `g-`: a heading one level deeper or shallower.
    Deeper,
    Shallower,
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
    /// After `Z`.
    Z,
    /// After `z`.
    View,
    /// After `]` (forward) or `[`.
    Bracket {
        forward: bool,
    },
    /// After `q`, when nothing is being recorded.
    Record,
    /// After `m`.
    Mark,
    /// After `` ` `` or, to go to the line only, `'`.
    Goto {
        linewise: bool,
    },
    /// After `@`.
    Play,
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
    /// For a change made to a selection: how large the selection was. The
    /// change is repeated on as much text from the cursor on.
    shape: Option<Shape>,
}

/// A line (counted from 0) and a byte within it.
type LinePlace = (usize, usize);

/// How many keys one key press may set off through macros. A macro that
/// calls itself runs until something in it fails; one in which nothing
/// ever fails would run forever.
const MACRO_KEYS: usize = 200_000;

/// What `n` repeats.
#[derive(Debug, Clone)]
struct Search {
    pattern: String,
    forward: bool,
    /// Whether a match may not continue a word before it, and whether it
    /// may not be continued by one after it (`*` on a word: neither).
    word_start: bool,
    word_end: bool,
}

/// The size of a selection.
#[derive(Debug, Clone, Copy)]
struct Shape {
    linewise: bool,
    lines: usize,
    /// On one line, how many columns wide the selection was; over several
    /// lines, the column it ended in.
    columns: usize,
    /// Whether the selection reached to the end of its last line, however
    /// long.
    to_end: bool,
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
    /// Whether the operator being applied got a motion over characters
    /// that turned out to cover whole lines. Deleting them then leaves the
    /// cursor on the first non-blank instead of in its column.
    became_linewise: bool,
    /// Where undo should bring the cursor back to after the next change,
    /// if that is not simply where the cursor is when it is made.
    undo_cursor: Option<usize>,
    /// The lines (counted from 0) the operator being applied works on.
    /// Undo puts the cursor back where it was if that was in or next to
    /// them, whichever of them really changed.
    undo_block: Option<Range<usize>>,
    /// For each undo step made in this session, where the cursor was
    /// before it and the lines it worked on; and the same for steps that
    /// were undone. The cursor is kept as a line and a byte within it, and
    /// read as that in the text after a redo too, as Vim does.
    undo_cursors: Vec<(LinePlace, Option<Range<usize>>)>,
    redo_cursors: Vec<(LinePlace, Option<Range<usize>>)>,
    /// The other end of the visual selection.
    anchor: usize,
    /// Where this insert began, as a line (counted from 0) and a byte
    /// within it. `Ctrl-W` and `Ctrl-U` stop here once before deleting text
    /// that was there before. As in Vim it is a place, not a piece of text:
    /// it stays where it is when the text before it goes.
    insert_start: (usize, usize),
    /// How often the text typed in this insert is to be repeated on Escape.
    insert_count: usize,
    /// The keys pressed in this insert, which a count repeats on Escape.
    insert_keys: Vec<KeyPress>,
    /// Whether this insert began by opening a line (`o`, `O`), which a
    /// count repeats along with the keys.
    insert_opened: bool,
    /// Whether this insert replaces characters instead of pushing them
    /// along (`R`), and for each character typed so far what it replaced:
    /// `None` where it was added to the text. Backspace brings those back.
    replacing: bool,
    replaced: Vec<Option<String>>,
    /// Whether the cursor was moved in this insert and nothing typed since.
    /// That ends the insert as far as undo, count and `.` are concerned;
    /// typing on begins a new one.
    insert_moved: bool,
    /// Places in the text remembered by name with `m`, as byte offsets.
    /// Unlike Vim's, which are a line and a column, they stay with the
    /// text they were put on when the text around them changes.
    marks: HashMap<char, usize>,
    /// Where the cursor was before it last jumped (two backticks go back
    /// there), and the one before that while a command is still under way.
    jump: Option<usize>,
    prev_jump: Option<usize>,
    /// The two ends and the kind of the last selection, for `gv`, and the
    /// column the cursor was aiming at in it.
    last_visual: Option<(usize, usize, Mode)>,
    last_visual_column: Option<usize>,
    /// Where insert mode was last left, for `gi`.
    last_insert: Option<usize>,
    /// Whether the command just handled failed: asked for something that
    /// is not there. That ends a macro.
    failed: bool,
    /// The register a macro is being recorded into, and its keys so far.
    recording: Option<(char, Vec<KeyPress>)>,
    /// The register of the macro last played, for `@@`.
    last_macro: Option<char>,
    /// Keys of macros being played that are still to come.
    queue: VecDeque<KeyPress>,
    /// Whether the key being handled comes from a macro. All that a macro
    /// changes is one undo step, as if it were one command.
    playing: bool,
    command_line: String,
    /// What the command line is for: `:` for a command, `/` or `?` for a
    /// search.
    prompt: char,
    /// The mode a search prompt was opened from and goes back to.
    prompt_return: Mode,
    /// The last search: what for, which way, and whether only as a whole
    /// word (`*`, `#`).
    last_search: Option<Search>,
    /// Whether `j` and `k` go by the rows the host shows instead of by
    /// lines of the text, as `gj` and `gk` always do where it shows any.
    rows_first: bool,
    /// Whether a bracket typed brings its closing one along (not Vim's).
    pairs: bool,
    /// The texts that were put in whole (see [`Key::Text`]): how many
    /// characters before the cursor each replaced, the text, and how far
    /// into it the cursor went.
    texts: Vec<(usize, String, usize)>,
    /// How far across the cursor aimed when it last went by rows, and the
    /// column it came to: the aim holds while that is still the column
    /// the cursor is aiming at.
    row_goal: Option<(usize, f64)>,
    /// How many lines `Ctrl-F` moves; `Ctrl-D` moves half as many.
    page_lines: usize,
    /// The first and the last line the view shows, counted from 0, for
    /// `H`, `M` and `L`.
    view_lines: (usize, usize),
    /// The keys of the command being typed, counts excluded.
    record: Vec<KeyPress>,
    record_count: Option<usize>,
    /// In visual mode, where in `record` the command being typed begins:
    /// what came before only made the selection.
    command_start: usize,
    /// The selection the command being typed was applied to.
    record_shape: Option<Shape>,
    /// Whether the selection was made by `.` rather than by the user.
    reselected: bool,
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
            became_linewise: false,
            undo_cursor: None,
            undo_block: None,
            undo_cursors: Vec::new(),
            redo_cursors: Vec::new(),
            anchor: 0,
            insert_start: (0, 0),
            insert_count: 1,
            insert_keys: Vec::new(),
            insert_opened: false,
            replacing: false,
            replaced: Vec::new(),
            insert_moved: false,
            marks: HashMap::new(),
            jump: None,
            prev_jump: None,
            last_visual: None,
            last_visual_column: None,
            last_insert: None,
            failed: false,
            recording: None,
            last_macro: None,
            queue: VecDeque::new(),
            playing: false,
            command_line: String::new(),
            prompt: ':',
            prompt_return: Mode::Normal,
            last_search: None,
            rows_first: false,
            pairs: false,
            texts: Vec::new(),
            row_goal: None,
            page_lines: 40,
            view_lines: (0, 0),
            record: Vec::new(),
            record_count: None,
            command_start: 0,
            record_shape: None,
            reselected: false,
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

    /// Whether no command is under way: no count, no operator, no key
    /// that waits for another.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.pending.is_idle()
    }

    /// Whether an operator is waiting for its motion.
    #[must_use]
    pub fn operator_pending(&self) -> bool {
        self.pending.operator.is_some()
    }

    /// What the command line is for, while it is open: `:` for a command,
    /// `/` or `?` for a search.
    #[must_use]
    pub fn prompt(&self) -> Option<char> {
        (self.mode == Mode::Command).then_some(self.prompt)
    }

    /// Tells the engine how many lines the view shows, which is how far
    /// `Ctrl-F` and `Ctrl-B` move.
    pub fn set_page_lines(&mut self, lines: usize) {
        self.page_lines = lines.max(2);
    }

    /// The key that, pressed in insert mode, puts `text` in place of the
    /// `replaced` characters before the cursor and the cursor `within`
    /// bytes into it: for taking one of a list of completions. It is part
    /// of what is being typed, for undo and for `.`.
    pub fn completed(&mut self, replaced: usize, text: String, within: usize) -> KeyPress {
        let within = within.min(text.len());
        self.texts.push((replaced, text, within));
        let number = u32::try_from(self.texts.len() - 1).unwrap_or(u32::MAX);
        KeyPress::plain(Key::Text(number))
    }

    /// Takes over from `other`, the engine of another text, what a user
    /// expects to have in every text: the registers, what `.` repeats,
    /// what `n` and `;` look for, the last macro.
    pub fn share(&mut self, other: &Self) {
        self.registers.clone_from(&other.registers);
        self.last_change.clone_from(&other.last_change);
        self.last_search.clone_from(&other.last_search);
        self.last_find = other.last_find;
        self.last_macro = other.last_macro;
        self.texts.clone_from(&other.texts);
    }

    /// Puts the cursor at `pos` as a jump: `` ` ` `` leads back.
    pub fn jump(&mut self, pos: usize, text: &str) {
        self.set_jump();
        self.set_cursor(pos, text);
    }

    /// Makes brackets, quotation marks and dollar signs typed in insert
    /// mode come in pairs: the closing one is put in behind the cursor,
    /// typing it steps over it, and Backspace between the two takes both.
    pub fn set_pairs(&mut self, on: bool) {
        self.pairs = on;
    }

    /// Makes `j` and `k` go by the rows the host shows (see
    /// [`Host::row`]), as they should where only typeset pages are shown.
    pub fn set_rows_first(&mut self, on: bool) {
        self.rows_first = on;
    }

    /// Tells the engine which lines (counted from 0) the view shows, which
    /// is where `H`, `M` and `L` go.
    pub fn set_view_lines(&mut self, first: usize, last: usize) {
        self.view_lines = (first, last.max(first));
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
        // A search typed from a selection extends it; until it is entered
        // the selection is what it was.
        let mode = if self.mode == Mode::Command {
            self.prompt_return
        } else {
            self.mode
        };
        match mode {
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
        if let Some((_, keys)) = &mut self.recording {
            keys.push(press);
        }
        self.failed = false;
        self.dispatch(press, host);
        // The key may have started a macro: its keys follow as if typed,
        // up to the first command that fails.
        let mut budget = MACRO_KEYS;
        while let Some(next) = self.queue.pop_front() {
            if budget == 0 {
                self.queue.clear();
                break;
            }
            budget -= 1;
            self.failed = false;
            self.playing = true;
            self.dispatch(next, host);
            if self.failed {
                self.queue.clear();
            }
        }
        self.playing = false;
        std::mem::take(&mut self.effects)
    }

    /// Changes the text from outside the keyboard, as an agent does:
    /// each of `edits` replaces a range by a text, the ranges referring to
    /// the text as the edits before them left it. Together they are one
    /// undo step of their own, whatever is being typed; `.` still repeats
    /// what it did before; and the cursor, the selection and the marks
    /// stay with their text.
    pub fn replace(&mut self, host: &mut impl Host, edits: &[(Range<usize>, &str)]) {
        let (changed, undo_cursor) = (self.changed, self.undo_cursor.take());
        self.joined = false;
        for (range, text) in edits {
            let (start, old_end, new_end) = (range.start, range.end, range.start + text.len());
            self.edit(host, range.clone(), text);
            for at in [&mut self.cursor, &mut self.anchor] {
                if *at >= old_end {
                    *at = *at - old_end + new_end;
                } else if *at > start {
                    *at = start;
                }
            }
        }
        // What is typed next is a step of its own again.
        self.joined = false;
        self.changed = changed;
        self.undo_cursor = undo_cursor;
        let text = host.text();
        self.cursor = clamp(Text(text), self.cursor, self.mode != Mode::Normal);
        self.anchor = self.anchor.min(text.len());
        self.wanted_column = None;
        if self.mode == Mode::Insert {
            self.insert_start = place(text, self.cursor);
        }
    }

    /// Whether a macro is being recorded, and into which register.
    #[must_use]
    pub fn recording(&self) -> Option<char> {
        self.recording.as_ref().map(|(name, _)| *name)
    }

    fn dispatch(&mut self, press: KeyPress, host: &mut impl Host) {
        if self.mode == Mode::Normal && self.pending.is_idle() {
            // A new command starts.
            self.undo_cursor = None;
            self.record.clear();
            self.record_count = None;
            self.command_start = 0;
            self.record_shape = None;
            self.changed = false;
            self.joined = self.joined && self.playing;
            self.repeatable = true;
        } else if self.selecting() && self.pending.is_idle() {
            self.command_start = self.record.len();
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
                shape: self.record_shape.take(),
            });
            self.changed = false;
        }
        self.settle();
    }

    /// What is done after every key outside insert mode.
    fn settle(&mut self) {
        if self.selecting() {
            self.last_visual = Some((self.anchor, self.cursor, self.mode));
            self.last_visual_column = self.wanted_column;
        }
        if !matches!(self.mode, Mode::Insert | Mode::Command) {
            // A jump that led back to where it started is none: what was
            // remembered before it still is.
            if self.prev_jump.is_some() && self.jump == Some(self.cursor) {
                self.jump = self.prev_jump;
            }
            self.prev_jump = None;
        }
    }

    /// Remembers where the cursor is as the place to jump back to.
    fn set_jump(&mut self) {
        self.prev_jump = self.jump;
        self.jump = Some(self.cursor);
    }

    /// Moves what is remembered by position along with a change of the
    /// text: `start..old_end` became `start..new_end`.
    fn shift_marks(&mut self, start: usize, old_end: usize, new_end: usize) {
        let shift = |at: &mut usize| {
            if *at >= old_end {
                *at = *at - old_end + new_end;
            } else if *at > start {
                *at = start;
            }
        };
        self.marks.values_mut().for_each(shift);
        self.jump.iter_mut().for_each(shift);
        self.prev_jump.iter_mut().for_each(shift);
        self.last_insert.iter_mut().for_each(shift);
        if let Some((anchor, cursor, _)) = &mut self.last_visual {
            shift(anchor);
            shift(cursor);
        }
    }

    // ---- Editing helpers -------------------------------------------------

    /// Replaces `range` with `text` as part of the current command.
    fn edit(&mut self, host: &mut impl Host, range: Range<usize>, text: &str) {
        if !self.joined {
            // A new undo step: remember where the cursor is as the change
            // is made, which is where undo will bring it back to.
            let cursor = self.undo_cursor.take().unwrap_or(self.cursor);
            self.undo_cursors
                .push((place(host.text(), cursor), self.undo_block.take()));
            self.redo_cursors.clear();
        }
        self.shift_marks(range.start, range.end, range.start + text.len());
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

    /// Makes the current command an undo step even though it changed
    /// nothing. Vim does this for commands that find nothing to do only
    /// after they have begun, and `u` then takes back that nothing.
    fn touch(&mut self, host: &mut impl Host) {
        // Nothing changed, but this is still the command `.` repeats.
        self.changed = true;
        if !self.joined {
            let at = clamp(Text(host.text()), self.cursor, true);
            self.edit(host, at..at, "");
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

    /// The command being typed cannot be done.
    fn fail(&mut self) {
        self.failed = true;
        self.reset_pending();
    }

    /// Whether a selection is active. The cursor may then stand on the end
    /// of a line, behind its last character, and motions lead there.
    fn selecting(&self) -> bool {
        matches!(self.mode, Mode::Visual | Mode::VisualLine)
    }

    /// Where the cursor comes back to when an operator is applied to the
    /// selection: its earlier end, which for whole lines is the start of
    /// the line the selection was begun on.
    fn selection_start(&self, text: Text) -> usize {
        let anchor = if self.mode == Mode::VisualLine && !self.reselected {
            text.line_start(self.anchor)
        } else {
            self.anchor
        };
        anchor.min(self.cursor)
    }

    /// Notes that the command being typed is applied to the selection, so
    /// that `.` can apply it to as much text elsewhere.
    fn record_selection(&mut self, text: Text) {
        let (low, high) = (self.anchor.min(self.cursor), self.anchor.max(self.cursor));
        let lines = text.line_index(high) - text.line_index(low) + 1;
        self.record_shape = Some(Shape {
            linewise: self.mode == Mode::VisualLine,
            lines,
            columns: if lines == 1 {
                (text.column(high) + 1).saturating_sub(text.column(low))
            } else {
                text.column(high)
            },
            to_end: self.wanted_column == Some(usize::MAX),
        });
        let start = self.command_start.min(self.record.len());
        self.record.drain(..start);
        self.command_start = 0;
    }

    /// Selects as much text from the cursor on as `shape` describes.
    fn reselect(&mut self, shape: Shape, text: Text) {
        let start = clamp(text, self.cursor, false);
        let line = lines_below(text, start, shape.lines - 1).unwrap_or(text.line_start(start));
        let end = if shape.linewise {
            // Whatever the operator does with the column the cursor aims
            // at, it is the one from before the selection was made.
            self.wanted_column = self.wanted_column.or_else(|| Some(text.column(start)));
            if line == text.line_start(start) {
                start
            } else {
                line
            }
        } else {
            // The cursor goes to the end of the selection as if by a
            // motion, and from then on aims at that column.
            let column = if shape.to_end {
                usize::MAX
            } else if shape.lines == 1 {
                text.column(start).saturating_add(shape.columns - 1)
            } else {
                shape.columns
            };
            self.wanted_column = Some(column);
            column_in_line(text, line, column, true)
        };
        self.anchor = start;
        self.cursor = end;
        self.mode = if shape.linewise {
            Mode::VisualLine
        } else {
            Mode::Visual
        };
        self.reselected = true;
    }

    fn move_to(&mut self, text: Text, pos: usize, column: Column) {
        self.row_goal = None;
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
                // A register that holds nothing is still one line of
                // characters, unlike a register that was never set.
                let existed = self.registers.contains_key(&name.to_ascii_lowercase());
                let target = self.registers.entry(name.to_ascii_lowercase()).or_default();
                if register.linewise && !target.linewise && existed {
                    target.text.push('\n');
                }
                target.text.push_str(&register.text);
                if target.linewise && !register.linewise {
                    // Appended to whole lines, it becomes a line itself.
                    target.text.push('\n');
                }
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
                    self.fail();
                }
                return;
            }
            Awaiting::Mark => {
                self.reset_pending();
                match press.key {
                    Key::Char(name) if name.is_ascii_lowercase() => {
                        self.marks.insert(name, self.cursor);
                    }
                    Key::Char('\'' | '`') => self.jump = Some(self.cursor),
                    _ => self.fail(),
                }
                return;
            }
            Awaiting::Goto { linewise } => {
                let target = match press.key {
                    Key::Char('\'' | '`') => self.jump,
                    Key::Char(name) if name.is_ascii_lowercase() => self.marks.get(&name).copied(),
                    Key::Char('<') => self
                        .last_visual
                        .map(|(anchor, cursor, _)| anchor.min(cursor)),
                    Key::Char('>') => self
                        .last_visual
                        .map(|(anchor, cursor, _)| anchor.max(cursor)),
                    Key::Char('^') => self.last_insert,
                    _ => None,
                };
                let _ = self.take_count();
                let text = Text(host.text());
                let moved = target.map(|target| {
                    let target = clamp(text, target, false);
                    Moved {
                        to: if linewise {
                            text.first_non_blank(target)
                        } else {
                            target
                        },
                        kind: if linewise {
                            Kind::Linewise
                        } else {
                            Kind::Exclusive
                        },
                        column: Column::Reset,
                    }
                });
                if moved.is_some() {
                    self.set_jump();
                } else {
                    // Even a mark that is not set makes the cursor aim at
                    // the column it is in.
                    self.wanted_column = None;
                }
                return self.finish_motion(moved, host);
            }
            Awaiting::Record => {
                self.reset_pending();
                match press.key {
                    Key::Char(name) if name.is_ascii_alphanumeric() => {
                        self.recording = Some((name, Vec::new()));
                    }
                    _ => self.fail(),
                }
                return;
            }
            Awaiting::Play => {
                let (count, _) = self.take_count();
                self.reset_pending();
                let name = match press.key {
                    Key::Char('@') => self.last_macro,
                    Key::Char(name) => Some(name.to_ascii_lowercase()),
                    _ => None,
                };
                let keys: Vec<KeyPress> = name
                    .and_then(|name| self.registers.get(&name))
                    .map(|register| register.text.chars().map(key_of).collect())
                    .unwrap_or_default();
                if keys.is_empty() {
                    return self.fail();
                }
                self.last_macro = name;
                for _ in 0..count {
                    for &key in keys.iter().rev() {
                        self.queue.push_front(key);
                    }
                }
                return;
            }
            Awaiting::Find { forward, till } => {
                let Key::Char(target) = press.key else {
                    return self.fail();
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
            Awaiting::Bracket { forward } => {
                self.reset_pending();
                // The editor's own: to the next or the previous change an
                // agent proposes (`]p`).
                let what = match press.key {
                    // `]]` and `[[`: to the next and the previous page.
                    Key::Char(']') if !press.ctrl && forward => "page",
                    Key::Char('[') if !press.ctrl && !forward => "page",
                    Key::Char('p') if !press.ctrl => "proposal",
                    // And to what the compiler complains about.
                    Key::Char('d') if !press.ctrl => "diagnostic",
                    _ => return self.fail(),
                };
                let way = if forward { "next" } else { "previous" };
                self.effects.push(Effect::Command(format!("{what} {way}")));
                return;
            }
            Awaiting::View => {
                self.reset_pending();
                let place = match press.key {
                    Key::Char('z' | '.') => Place::Middle,
                    Key::Char('t') | Key::Enter => Place::Top,
                    Key::Char('b' | '-') => Place::Bottom,
                    _ => return self.fail(),
                };
                self.effects.push(Effect::Scroll(place));
                return;
            }
            Awaiting::Z => {
                self.reset_pending();
                match press.key {
                    Key::Char('Z') => self.effects.push(Effect::Command("x".to_owned())),
                    Key::Char('Q') => self.effects.push(Effect::Command("q!".to_owned())),
                    _ => {}
                }
                return;
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
                        self.wanted_column = None;
                    }
                }
                Key::Left => self.motion_key('h', host),
                Key::Backspace => self.motion_key('\u{8}', host),
                Key::Right => self.motion_key('l', host),
                Key::Up => self.motion_key('k', host),
                Key::Down => self.motion_key('j', host),
                Key::Enter => self.motion_key('+', host),
                Key::Home => self.motion_key('0', host),
                Key::End => self.motion_key('$', host),
                _ => self.fail(),
            }
            return;
        };

        if press.ctrl && c == '^' && self.pending.is_idle() && !self.selecting() {
            // To the file that was edited before this one.
            self.effects.push(Effect::Command("b#".to_owned()));
            return;
        }
        if press.ctrl {
            // Moving by pages: the cursor goes, and the view follows it.
            let page = match c {
                'd' => Some(('j', self.page_lines / 2)),
                'u' => Some(('k', self.page_lines / 2)),
                'f' => Some(('j', self.page_lines)),
                'b' => Some(('k', self.page_lines)),
                _ => None,
            };
            if let Some((direction, lines)) = page {
                let (count, _) = self.take_count();
                self.pending.count = Some(lines.max(1).saturating_mul(count));
                return self.motion_key(direction, host);
            }
            match c {
                'r' if !visual && self.pending.operator.is_none() => {
                    let (count, _) = self.take_count();
                    for done in 0..count {
                        self.note_column(host);
                        let length = host.text().len();
                        let Some(changed) = host.redo() else {
                            if done == 0 {
                                self.failed = true;
                            }
                            break;
                        };
                        let old_end = (changed.end + length).saturating_sub(host.text().len());
                        self.shift_marks(changed.start, old_end.max(changed.start), changed.end);
                        let before = self.redo_cursors.pop();
                        self.undo_cursors.push(
                            before
                                .clone()
                                .unwrap_or_else(|| (place(host.text(), changed.start), None)),
                        );
                        self.after_undo(&changed, before.map(|(cursor, _)| cursor), None, host);
                    }
                    self.wanted_column = None;
                    self.joined = false;
                }
                'c' | '[' => {
                    self.reset_pending();
                    if visual {
                        self.wanted_column = None;
                    }
                    self.mode = Mode::Normal;
                    self.cursor = clamp(Text(host.text()), self.cursor, false);
                }
                _ => self.fail(),
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
            // `gcc`: the comment operator doubled, on this line.
            'c' if self.pending.operator == Some(Operator::Format(Format::Comment)) => {
                Some(Operator::Format(Format::Comment))
            }
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

        if visual && matches!(c, 'I' | 'A') {
            // Insert before the selection or append after it. The
            // selection counts as whole lines, as Vim has it: from the
            // start of the line it was begun on to the cursor, whichever
            // comes first.
            let text = Text(host.text());
            let begun = text.line_start(self.anchor);
            let (start, end) = (begun.min(self.cursor), begun.max(self.cursor));
            let _ = self.take_count();
            self.mode = Mode::VisualLine;
            self.record_selection(text);
            self.mode = Mode::Normal;
            self.reset_pending();
            self.cursor = if c == 'I' {
                clamp(text, start, true)
            } else {
                // After the character the selection ends on; before it,
                // if it stands in the column the selection starts in.
                let end = clamp(text, end, false);
                let same_column = text.column(clamp(text, start, false)) == text.column(end);
                if text.line_is_empty(end) || same_column {
                    end
                } else {
                    text.next(end)
                }
            };
            return self.enter_insert(1, host.text());
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
            'm' if self.pending.operator.is_none() => self.pending.awaiting = Awaiting::Mark,
            '`' | '\'' => {
                self.pending.awaiting = Awaiting::Goto {
                    linewise: c == '\'',
                }
            }
            'q' if self.pending.operator.is_none() => {
                self.reset_pending();
                if let Some((name, mut keys)) = self.recording.take() {
                    // The `q` that ends the recording is not part of it.
                    keys.pop();
                    let text: String = keys.into_iter().map(char_of).collect();
                    if name.is_ascii_uppercase() {
                        let register = self.registers.entry(name.to_ascii_lowercase()).or_default();
                        register.text.push_str(&text);
                    } else {
                        let register = Register {
                            text,
                            linewise: false,
                        };
                        self.registers.insert(name, register);
                    }
                } else {
                    self.pending.awaiting = Awaiting::Record;
                }
            }
            '@' if self.pending.operator.is_none() => self.pending.awaiting = Awaiting::Play,
            'g' => self.pending.awaiting = Awaiting::G,
            'z' if self.pending.operator.is_none() => self.pending.awaiting = Awaiting::View,
            // What the thing under the cursor is: the host knows.
            'K' if self.pending.is_idle() && !self.selecting() => {
                self.effects.push(Effect::Command("describe".to_owned()));
            }
            ']' | '[' if self.pending.is_idle() && !self.selecting() => {
                self.pending.awaiting = Awaiting::Bracket { forward: c == ']' };
            }
            'H' | 'M' | 'L' => {
                // To the top, the middle or the bottom of what the view
                // shows; a count is that many lines from the edge.
                let (count, _) = self.take_count();
                let (first, last) = self.view_lines;
                let line = match c {
                    'H' => (first + count - 1).min(last),
                    'L' => last.saturating_sub(count - 1).max(first),
                    _ => first + (last - first) / 2,
                };
                let text = Text(host.text());
                self.set_jump();
                let moved = Moved {
                    to: text.first_non_blank(text.line(line)),
                    kind: Kind::Linewise,
                    column: Column::Reset,
                };
                self.finish_motion(Some(moved), host);
            }
            'f' | 'F' | 't' | 'T' => {
                self.pending.awaiting = Awaiting::Find {
                    forward: c.is_lowercase(),
                    till: c.eq_ignore_ascii_case(&'t'),
                };
            }
            'h' | 'l' | 'j' | 'k' | '0' | '^' | '$' | 'w' | 'W' | 'b' | 'B' | 'e' | 'E' | 'G'
            | '{' | '}' | ';' | ',' | '%' | ' ' | '|' | '_' | '+' | '-' => self.motion_key(c, host),
            '/' | '?' => {
                // The count and a pending operator wait for the search.
                self.prompt = c;
                self.prompt_return = self.mode;
                self.command_line.clear();
                self.mode = Mode::Command;
            }
            'n' | 'N' => {
                let forward = self
                    .last_search
                    .as_ref()
                    .is_some_and(|search| search.forward == (c == 'n'));
                self.search(forward, self.cursor, host);
            }
            '*' | '#' => self.search_word(c == '*', host),
            'v' | 'V' => self.visual_key(c, host),
            'o' if visual => {
                std::mem::swap(&mut self.anchor, &mut self.cursor);
                self.wanted_column = None;
                self.reset_pending();
            }
            'J' if visual => self.join_lines(host),
            'r' if visual => self.pending.awaiting = Awaiting::Replace,
            'p' | 'P' if visual => self.put_over_selection(c == 'p', host),
            ':' => {
                // From a selection too; commands do not take it as their
                // range yet, so it is given up.
                self.reset_pending();
                self.cursor = clamp(Text(host.text()), self.cursor, false);
                self.prompt = ':';
                self.prompt_return = Mode::Normal;
                self.command_line.clear();
                self.mode = Mode::Command;
            }
            _ if visual || self.pending.operator.is_some() => self.fail(),
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
            'i' | 'a' | 'I' | 'A' | 'o' | 'O' | 'R' => self.start_insert(c, host),
            'Z' => {
                self.reset_pending();
                self.pending.awaiting = Awaiting::Z;
            }
            'u' => {
                let (count, _) = self.take_count();
                for done in 0..count {
                    self.note_column(host);
                    let length = host.text().len();
                    let Some(changed) = host.undo() else {
                        if done == 0 {
                            self.failed = true;
                        }
                        break;
                    };
                    let old_end = (changed.end + length).saturating_sub(host.text().len());
                    self.shift_marks(changed.start, old_end.max(changed.start), changed.end);
                    let before = self.undo_cursors.pop();
                    self.redo_cursors.push(
                        before
                            .clone()
                            .unwrap_or_else(|| (place(host.text(), changed.start), None)),
                    );
                    let (cursor, block) = before.unzip();
                    self.after_undo(&changed, cursor, block.flatten(), host);
                }
                self.wanted_column = None;
                self.joined = false;
            }
            '.' => self.repeat(host),
            _ => self.fail(),
        }
    }

    /// Places the cursor after an undo or redo that changed `changed`.
    /// `before` is where the cursor was when the change was originally
    /// made, if that is known.
    ///
    /// As in Vim, the cursor goes back to that position if it lies among
    /// the changed lines or right next to them, and otherwise to the first
    /// changed line.
    /// Makes the column the cursor is in the one it aims at, unless it is
    /// aiming at one already. For before the text changes under it.
    fn note_column(&mut self, host: &impl Host) {
        let text = Text(host.text());
        let column = text.column(clamp(text, self.cursor, false));
        self.wanted_column = self.wanted_column.or(Some(column));
    }

    fn after_undo(
        &mut self,
        changed: &Range<usize>,
        before: Option<LinePlace>,
        block: Option<Range<usize>>,
        host: &impl Host,
    ) {
        let text = Text(host.text());
        let first = text.line_start(clamp(text, changed.start, false));
        let mut first_line = text.line_index(first);
        let mut last_line = text.line_index(clamp(text, changed.end.max(changed.start), false));
        // The lines the change worked on, if they are known, count as
        // changed for this even if they came out the same.
        if let Some(block) = block {
            first_line = first_line.min(block.start);
            last_line = last_line.max(block.end.saturating_sub(1));
        }
        if let Some((line, byte)) = before
            && (first_line.saturating_sub(1)..=last_line + 1).contains(&line)
            && line < text.line_count()
        {
            let start = text.line(line);
            self.cursor = clamp(text, (start + byte).min(text.line_end(start)), false);
            self.wanted_column = None;
            return;
        }
        let column = self
            .wanted_column
            .unwrap_or_else(|| text.column(clamp(text, self.cursor, false)));
        self.cursor = column_in_line(text, first, column, false);
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
            'j' | 'k' => self.row_key(c == 'j', host),
            '~' => self.operator_key(Operator::ToggleCase, host),
            'u' => self.operator_key(Operator::Lower, host),
            'U' => self.operator_key(Operator::Upper, host),
            // Not Vim's: the formats of the host's language, as operators.
            'b' => self.operator_key(Operator::Format(Format::Strong), host),
            'm' => self.operator_key(Operator::Format(Format::Emph), host),
            't' => self.operator_key(Operator::Format(Format::Raw), host),
            'c' => self.operator_key(Operator::Format(Format::Comment), host),
            'l' => self.operator_key(Operator::Format(Format::ListItem), host),
            '+' | '-' if self.pending.operator.is_none() => {
                let format = if c == '+' {
                    Format::Deeper
                } else {
                    Format::Shallower
                };
                if self.selecting() {
                    return self.operator_key(Operator::Format(format), host);
                }
                // This line, or with a count that many lines.
                let (count, _) = self.take_count();
                let text = Text(host.text());
                let start = text.line_start(self.cursor);
                let last = lines_below(text, self.cursor, count - 1).unwrap_or(start);
                let object = Object {
                    start,
                    end: text.line_end(last),
                    linewise: false,
                };
                self.reset_pending();
                self.operate(Operator::Format(format), object, 1, self.cursor, host);
            }
            'J' if self.pending.operator.is_none() => self.join_lines_raw(host),
            'v' if self.pending.is_idle() && self.mode == Mode::Normal => {
                // The last selection again.
                let Some((anchor, cursor, mode)) = self.last_visual else {
                    return self.fail();
                };
                let text = Text(host.text());
                self.anchor = clamp(text, anchor, true);
                self.cursor = clamp(text, cursor, true);
                self.mode = mode;
                self.wanted_column = self.last_visual_column;
            }
            'i' if self.pending.operator.is_none() && self.mode == Mode::Normal => {
                // Insert where insert mode was last left.
                if let Some(at) = self.last_insert {
                    self.cursor = clamp(Text(host.text()), at, true);
                }
                self.start_insert('i', host);
            }
            // Not Vim's `gs` (which sleeps): the editor's own key, for
            // switching what the window shows.
            's' if self.pending.is_idle() && !self.selecting() => {
                self.effects.push(Effect::Command("view".to_owned()));
            }
            // To where the name under the cursor is defined, as far as
            // the host can tell; and the host's list of places to go.
            'd' | 'o' | 'f' if self.pending.is_idle() && !self.selecting() => {
                let command = match c {
                    'd' => "definition",
                    'o' => "outline",
                    // The file named under the cursor.
                    _ => "file",
                };
                self.effects.push(Effect::Command(command.to_owned()));
            }
            // Nor Vim's `ga` and `gr`: to accept or reject the change an
            // agent proposes under the cursor.
            'a' | 'r' if self.pending.is_idle() && !self.selecting() => {
                let command = if c == 'a' { "accept" } else { "reject" };
                self.effects.push(Effect::Command(command.to_owned()));
            }
            _ => self.fail(),
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
            let landing = self.selection_start(Text(host.text()));
            let (count, _) = self.take_count();
            self.record_selection(Text(host.text()));
            self.mode = Mode::Normal;
            self.pending.operator = None;
            let object = Object {
                start: range.start,
                end: range.end,
                linewise,
            };
            self.nothing_selected = range.is_empty();
            return self.operate(operator, object, count, landing, host);
        }
        match self.pending.operator {
            None => {
                self.pending.operator = Some(operator);
                self.pending.operator_count = self.pending.count.take();
            }
            // Doubled (`dd`, `guu`): the operator with the motion `_`.
            Some(pending) if pending == operator => self.motion_key('_', host),
            Some(_) => self.fail(),
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
            // Unlike Escape, this leaves the cursor aiming at the column
            // it was in, even if that was behind the end of the line.
            let text = Text(host.text());
            self.wanted_column = self
                .wanted_column
                .or_else(|| Some(text.column(clamp(text, self.cursor, true))));
            self.mode = Mode::Normal;
            self.cursor = clamp(text, self.cursor, false);
        } else {
            if self.mode == Mode::Normal {
                self.anchor = self.cursor;
            }
            self.mode = wanted;
        }
    }

    fn object_key(&mut self, press: KeyPress, around: bool, host: &mut impl Host) {
        let Key::Char(c) = press.key else {
            return self.object_failed();
        };
        let text = Text(host.text());
        let (count, _) = self.take_count();
        let pos = self.cursor;
        let visual = self.selecting();
        // Words and brackets are found as Vim finds them, as a motion from
        // one end to the other, with all that follows from that for the
        // operator.
        if matches!(c, '(' | ')' | 'b' | '[' | ']' | '{' | '}' | 'B' | '<' | '>')
            && (!visual || self.anchor == self.cursor)
        {
            // Looking for a block is a jump, as it is in Vim.
            self.set_jump();
        }
        let as_motion = match c {
            'w' | 'W' if !visual => {
                match motion::word_object(text, pos, count, around, c == 'W') {
                    Ok(found) => Some(Some(found)),
                    Err(reached) => {
                        // Vim leaves the cursor where it gave up.
                        self.cursor = clamp(text, reached, false);
                        return self.object_failed();
                    }
                }
            }
            '(' | ')' | 'b' => Some(object::bracket(text, pos, '(', ')', around, count)),
            '[' | ']' => Some(object::bracket(text, pos, '[', ']', around, count)),
            '{' | '}' | 'B' => Some(object::bracket(text, pos, '{', '}', around, count)),
            '<' | '>' => Some(object::bracket(text, pos, '<', '>', around, count)),
            _ => None,
        };
        if let Some(found) = as_motion {
            let Some((start, end, inclusive)) = found else {
                return self.object_failed();
            };
            if visual {
                self.wanted_column = None;
                if inclusive || end > start {
                    self.anchor = start;
                    self.cursor = if inclusive { end } else { text.prev(end) };
                }
                return;
            }
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
            '"' | '\'' | '`' => object::quote(text, pos, c, around, count),
            // The host's own: an equation, a section, an argument.
            _ => host.object(c, pos, around).map(|range| Object {
                start: range.start,
                end: range.end,
                linewise: false,
            }),
        };
        let Some(found) = found else {
            return self.object_failed();
        };
        if visual {
            self.wanted_column = None;
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
            return self.object_failed();
        };
        let found = if operator == Operator::Delete {
            widen_delete(text, found)
        } else {
            found
        };
        self.operate(operator, found, 1, found.start, host);
    }

    /// A text object that is not there. Having looked for it still makes
    /// the cursor aim at the column it is in.
    fn object_failed(&mut self) {
        self.wanted_column = None;
        self.fail();
    }

    // ---- Motions -----------------------------------------------------------

    fn motion_key(&mut self, c: char, host: &mut impl Host) {
        if matches!(c, 'j' | 'k') && self.rows_first && self.pending.operator.is_none() {
            return self.row_key(c == 'j', host);
        }
        let moved = self.motion(c, host);
        self.finish_motion(moved, host);
    }

    /// Up and down by the rows the host shows: `gj` and `gk`, and `j` and
    /// `k` where rows come first. Where the host shows none, a row is a
    /// line of the text and these are `j` and `k`, as in Vim where lines
    /// do not wrap.
    fn row_key(&mut self, down: bool, host: &mut impl Host) {
        let count =
            (self.pending.operator_count.unwrap_or(1) * self.pending.count.unwrap_or(1)).max(1);
        let rows = isize::try_from(count).unwrap_or(isize::MAX);
        let goal = self
            .row_goal
            .filter(|(column, _)| self.wanted_column == Some(*column))
            .map(|(_, across)| across);
        let Some((to, across)) = host.row(self.cursor, if down { rows } else { -rows }, goal)
        else {
            let moved = self.motion(if down { 'j' } else { 'k' }, host);
            return self.finish_motion(moved, host);
        };
        self.take_count();
        let text = Text(host.text());
        let to = clamp(text, to, self.selecting());
        if to == self.cursor {
            return self.fail();
        }
        let column = text.column(to);
        let moved = Moved {
            to,
            kind: Kind::Exclusive,
            column: Column::Exact(column),
        };
        self.finish_motion(Some(moved), host);
        self.row_goal = Some((column, across));
    }

    fn motion_key_g(&mut self, c: char, host: &mut impl Host) {
        let text = Text(host.text());
        let (count, typed) = self.take_count();
        let operating = self.pending.operator.is_some();
        let moved = match c {
            'g' => {
                self.set_jump();
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
            // Backspace is `h` that goes on into the line before; with an
            // operator it stays in its line here.
            'h' | '\u{8}' => {
                let wraps = c != 'h' && !operating;
                let mut to = pos;
                for _ in 0..count {
                    if to > text.line_start(to) {
                        to = text.prev(to);
                    } else if let Some(line) = text.prev_line(to).filter(|_| wraps) {
                        to = if self.selecting() {
                            text.line_end(line)
                        } else {
                            text.line_last(line)
                        };
                    } else {
                        break;
                    }
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
                // Space is `l` that goes on into the next line; with an
                // operator it stays in its line here.
                let wraps = c == ' ' && !operating;
                let past_end = operating || self.mode != Mode::Normal;
                let mut to = pos;
                for _ in 0..count {
                    let limit = if past_end {
                        text.line_end(to)
                    } else {
                        text.line_last(to)
                    };
                    if to < limit {
                        to = text.next(to);
                    } else if let Some(line) = text.next_line(to).filter(|_| wraps) {
                        to = line;
                    } else {
                        break;
                    }
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
                to: column_in_line(text, text.line_start(pos), count - 1, self.selecting()),
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
                let to = if operating || self.selecting() {
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
                    self.wanted_column = None;
                    return None;
                }
                if !ok && !operating {
                    // As far as it goes, but not as far as asked: the
                    // cursor moves, and it still counts as failed.
                    self.failed = true;
                }
                if matches!(text.char_at(to), None | Some('\n'))
                    && to > text.line_start(to)
                    && to > pos
                    && !self.selecting()
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
                self.set_jump();
                let line = if typed {
                    text.line(count - 1)
                } else {
                    text.last_line()
                };
                Some(self.vertical(text, line))
            }
            '{' | '}' => {
                let Some((to, inclusive)) = motion::paragraph(text, pos, count, c == '}') else {
                    self.wanted_column = None;
                    return None;
                };
                self.set_jump();
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
                if till && count == 1 {
                    let step = if forward {
                        text.next(pos)
                    } else {
                        text.prev(pos)
                    };
                    if text.char_at(step) == Some(target) && pos < text.line_end(pos) {
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
                self.set_jump();
                let line = (count * text.line_count()).div_ceil(100);
                Some(self.vertical(text, text.line(line.max(1) - 1)))
            }
            '%' => {
                // From behind the last character, where a selection can
                // leave the cursor, that character is the one looked at.
                let from = if pos > text.line_start(pos) && pos == text.line_end(pos) {
                    text.prev(pos)
                } else {
                    pos
                };
                let to = motion::matching_bracket(text, from)?;
                self.set_jump();
                horizontal(to, Kind::Inclusive)
            }
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
        Moved {
            to: column_in_line(text, line, column, self.selecting()),
            kind: Kind::Linewise,
            column: Column::Keep,
        }
    }

    /// What to make of a word motion that got to `to`. If it got as far as
    /// asked, that is where it leads. If it ran into the end of the text,
    /// the cursor still goes there, but a pending operator is not applied.
    fn partial(&mut self, text: Text, to: usize, complete: bool) -> Option<usize> {
        if complete || self.pending.operator.is_none() {
            let to = if matches!(text.char_at(to), None | Some('\n')) && !self.selecting() {
                text.line_last(to)
            } else {
                clamp(text, to, true)
            };
            if to == self.cursor && !complete {
                // Even a word motion that leads nowhere makes the cursor
                // aim at the column it is in.
                self.wanted_column = None;
                return None;
            }
            if !complete {
                // Moved, but not as far as asked: that counts as failed.
                self.failed = true;
            }
            return Some(to);
        }
        self.cursor = clamp(text, to, false);
        self.wanted_column = None;
        None
    }

    /// Moves the cursor along a motion, or applies the pending operator
    /// over it.
    fn finish_motion(&mut self, moved: Option<Moved>, host: &mut impl Host) {
        let Some(moved) = moved else {
            return self.fail();
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
                    self.became_linewise = true;
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
        let became_linewise = std::mem::take(&mut self.became_linewise);
        if operator != Operator::Yank {
            // An operator that got as far as this is what `.` repeats, even
            // if it then finds nothing to do.
            self.changed = true;
            let text = Text(host.text());
            let last = if end > start { text.prev(end) } else { start };
            self.undo_block = Some(text.line_index(start)..text.line_index(last) + 1);
        }
        if operator != Operator::Yank {
            // Vim puts the cursor at the start of the text before it
            // changes it, and that is where undo returns to. One oddity is
            // kept: changing several whole lines is done from the second
            // of them.
            let text = Text(host.text());
            let unclamped = clamp(text, landing, true);
            let landing = clamp(text, landing, false);
            let second = text.next_line(start).filter(|&line| line < end);
            self.undo_cursor = Some(match second {
                Some(second) if linewise && operator == Operator::Change => {
                    // The same byte of that line, not the same column; and
                    // counted from where a selection left the cursor, which
                    // may be behind the end of its line.
                    let byte = unclamped - text.line_start(unclamped);
                    clamp(text, (second + byte).min(text.line_end(second)), true)
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
            }
            Operator::Delete => {
                if selected.is_empty() {
                    // Deleting nothing leaves even the registers alone.
                    if nothing_selected && !host.text().is_empty() {
                        self.touch(host);
                    }
                    // The cursor still goes to where the nothing is: in
                    // between a pair of quotes, say.
                    self.cursor = clamp(Text(host.text()), landing, false);
                    self.wanted_column = None;
                    self.column_before = None;
                    self.undo_block = None;
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
                    self.cursor = if became_linewise {
                        text.first_non_blank(line)
                    } else {
                        column_in_line(text, line, column, false)
                    };
                } else {
                    self.cursor = clamp(text, start, false);
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
                    if !host.text().is_empty() {
                        self.store(changed, true, true);
                    }
                    self.edit(host, start..content_end, "");
                } else {
                    let nothing = changed.is_empty() && nothing_selected;
                    if !nothing && !host.text().is_empty() {
                        self.store(changed, false, true);
                    }
                    self.edit(host, start..end, "");
                }
                self.cursor = start;
                self.enter_insert(1, host.text());
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
                self.cursor = column_in_line(Text(host.text()), first, column, false);
                self.touch(host);
                self.changed = true;
            }
            Operator::Format(format) => {
                let edits = host.format(format, start..end).unwrap_or_default();
                if edits.is_empty() {
                    self.failed = true;
                } else {
                    // From the last to the first: the ranges are those of
                    // the text as it was.
                    for edit in edits.iter().rev() {
                        self.edit(host, edit.range.clone(), &edit.text);
                    }
                }
                self.cursor = clamp(Text(host.text()), landing, false);
            }
            Operator::ToggleCase | Operator::Lower | Operator::Upper => {
                // What Vim makes of a motion that did not move, when the
                // cursor is at the start of a line: the whole line, or on
                // the first line one character. Hardly meant, but kept, so
                // that the engine can be held to Vim without exceptions.
                let text = Text(host.text());
                let (start, end) =
                    if selected.is_empty() && nothing_selected && start == text.line_start(start) {
                        let line_end = text.line_end(start);
                        if start > 0 {
                            (start, line_end)
                        } else {
                            (start, text.next(start).min(line_end))
                        }
                    } else {
                        (start, end)
                    };
                let selected = host.text().get(start..end).unwrap_or("").to_owned();
                let converted: String = selected
                    .chars()
                    .map(|c| match operator {
                        Operator::Lower => lower(c),
                        Operator::Upper => upper(c),
                        _ if c.is_lowercase() => upper(c),
                        _ => lower(c),
                    })
                    .collect();
                // Even if no letter changes, this is an undo step, as it is
                // in Vim.
                if selected.is_empty() {
                    self.touch(host);
                } else {
                    self.edit(host, start..end, &converted);
                }
                self.changed = true;
                self.cursor = clamp(Text(host.text()), landing, false);
            }
        }
        // A register named for this operator is used up, whether the
        // operator stored anything or not.
        self.pending.register = None;
        self.column_before = None;
        self.undo_block = None;
        // Whatever column the cursor was aiming at, from here on it aims at
        // the one it is in.
        if self.mode != Mode::Insert {
            self.wanted_column = None;
        }
    }

    // ---- Other normal-mode commands ---------------------------------------

    fn put(&mut self, after: bool, host: &mut impl Host) {
        let (count, _) = self.take_count();
        let name = self.pending.register.take().unwrap_or('"');
        self.reset_pending();
        // Even a put that finds nothing to put is what `.` repeats.
        self.changed = true;
        let register = self.registers.get(&name.to_ascii_lowercase()).cloned();
        let Some(register) = register.filter(|register| !register.text.is_empty()) else {
            self.touch(host);
            return self.fail();
        };
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

    /// `p` and `P` on a selection: what a register holds takes its place.
    /// With `keep`, as for `p`, the text that goes is kept in the unnamed
    /// register instead.
    fn put_over_selection(&mut self, keep: bool, host: &mut impl Host) {
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
        self.record_selection(Text(host.text()));
        self.mode = Mode::Normal;
        self.reset_pending();
        let replaced = host.text().get(range.clone()).unwrap_or("").to_owned();
        let mut inserted = register.text.clone();
        if linewise_selection && !register.linewise {
            inserted.push('\n');
        } else if !linewise_selection && register.linewise {
            inserted.insert(0, '\n');
        }
        self.edit(host, range.clone(), &inserted);
        if keep {
            self.store(replaced, linewise_selection, true);
        }
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
        if self.selecting() {
            let text = Text(host.text());
            let (low, high) = (self.anchor.min(self.cursor), self.anchor.max(self.cursor));
            let lines = text.line_index(high) - text.line_index(low);
            joins = lines.max(1);
            let start = self.selection_start(text);
            self.record_selection(text);
            self.mode = Mode::Normal;
            self.cursor = clamp(text, start, false);
            self.wanted_column = None;
            self.reset_pending();
            if text.next_line(self.cursor).is_none() {
                // Nothing to join, but still what `.` repeats.
                self.changed = true;
                return self.fail();
            }
        }
        self.reset_pending();
        let text = Text(host.text());
        let first = text.line_start(self.cursor);
        if self.record_shape.is_none() {
            // What `.` repeats is the join of as many lines as there were.
            let available = text.line_count() - text.line_index(first);
            self.record_count = Some(count.max(2).min(available));
        }
        if text.next_line(first).is_none() {
            if count > 2 {
                // Vim joins "as many lines as there are", which is none:
                // an undo step that changes nothing, with the cursor where
                // a join would have put it. An empty text comes out of
                // it as one empty line.
                if host.text().is_empty() {
                    self.edit(host, 0..0, "\n");
                } else {
                    self.touch(host);
                }
                self.cursor = Text(host.text()).line_start(self.cursor);
                self.wanted_column = None;
                return;
            }
            return self.fail();
        }
        // Each line is joined on as Vim does it: whether a space goes in
        // between depends on the line before as it was, not on what the
        // joining has made of it so far.
        let mut joined = text.line_text(first).to_owned();
        let mut ends_blank = joined.ends_with([' ', '\t']);
        let mut line = first;
        let mut seam = joined.len();
        for _ in 0..joins {
            let Some(next) = text.next_line(line) else {
                break;
            };
            line = next;
            seam = joined.len();
            if spaced {
                let rest = text.line_text(next).trim_start_matches([' ', '\t']);
                if !rest.is_empty() && !rest.starts_with(')') && !joined.is_empty() && !ends_blank {
                    joined.push(' ');
                }
                joined.push_str(rest);
                ends_blank = rest.ends_with([' ', '\t']);
            } else {
                joined.push_str(text.line_text(next));
            }
        }
        let end = text.line_end(line);
        self.edit(host, first..end, &joined);
        self.cursor = clamp(Text(host.text()), first + seam, false);
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
            // Undo comes back to where the selection started, which for
            // whole lines need not be the start of the line.
            self.undo_cursor = Some(clamp(text, self.selection_start(text), false));
            self.record_selection(text);
            self.record_count = None;
            self.mode = Mode::Normal;
            self.reset_pending();
            self.changed = true;
            if range.is_empty() {
                // Nothing selected (an empty last line): nothing to do,
                // not even an undo step.
                self.undo_cursor = None;
                self.cursor = clamp(text, range.start, false);
                self.wanted_column = None;
                return;
            }
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
                return self.fail();
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
            return self.fail();
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
        self.edit(host, self.cursor..end, &toggled);
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
        let mut count = if typed { Some(count) } else { recorded.count };
        // The repeated command is recorded afresh as it runs.
        self.record.clear();
        self.record_count = None;
        self.command_start = 0;
        self.record_shape = None;
        self.changed = false;
        self.joined = self.joined && self.playing;
        if let Some(shape) = recorded.shape {
            // A change to a selection is made to as much text again, and
            // stays what it was: a new count is not for it.
            self.reselect(shape, Text(host.text()));
            count = recorded.count;
        }
        if let Some(count) = count {
            for digit in count.to_string().chars() {
                self.normal_key(KeyPress::plain(Key::Char(digit)), host);
            }
        }
        let last = recorded.keys.len().saturating_sub(1);
        let mut complete = true;
        for (index, &press) in recorded.keys.iter().enumerate() {
            self.dispatch_recorded(press, host);
            if index < last && self.mode == Mode::Normal && self.pending.is_idle() {
                // The command failed before all its keys were used, such
                // as a change whose motion leads nowhere now. The rest of
                // the keys are not for anyone else.
                complete = false;
                break;
            }
        }
        if self.reselected {
            self.reselected = false;
            if self.selecting() {
                self.mode = Mode::Normal;
                self.cursor = clamp(Text(host.text()), self.cursor, false);
            }
            complete = false;
        }
        if !complete {
            // The change to repeat stays the one it was.
            self.last_change = Some(recorded);
            self.changed = false;
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
                shape: self.record_shape,
            });
        }
    }

    // ---- Insert mode -------------------------------------------------------

    fn enter_insert(&mut self, count: usize, text: &str) {
        self.mode = Mode::Insert;
        self.insert_start = place(text, self.cursor);
        self.insert_count = count;
        self.insert_keys.clear();
        self.insert_opened = false;
        self.insert_moved = false;
        self.replacing = false;
        self.replaced.clear();
        self.wanted_column = None;
        self.changed = true;
        if !self.joined {
            // Undo comes back to where the insert began, wherever in it
            // the first change is made.
            self.undo_cursor = Some(self.cursor);
        }
    }

    /// Whether typing replaces the characters under the cursor (`R`).
    #[must_use]
    pub fn replacing(&self) -> bool {
        self.mode == Mode::Insert && self.replacing
    }

    fn start_insert(&mut self, c: char, host: &mut impl Host) {
        let (count, _) = self.take_count();
        self.reset_pending();
        let text = Text(host.text());
        let column = self
            .wanted_column
            .unwrap_or_else(|| text.column(self.cursor));
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
        self.enter_insert(count, host.text());
        self.insert_opened = matches!(c, 'o' | 'O');
        self.replacing = c == 'R';
        if count > 1 {
            // An oddity of Vim's: while a counted insert is typed, the
            // cursor keeps aiming at the column it aimed at before.
            self.wanted_column = Some(column);
        }
    }

    fn insert_key(&mut self, press: KeyPress, host: &mut impl Host) {
        match (press.key, press.ctrl) {
            (Key::Escape, _) | (Key::Char('c' | '['), true) => self.leave_insert(host),
            (Key::Left | Key::Right | Key::Up | Key::Down | Key::Home | Key::End, _) => {
                let text = Text(host.text());
                let to =
                    match press.key {
                        Key::Left => (self.cursor > text.line_start(self.cursor))
                            .then(|| text.prev(self.cursor)),
                        Key::Right => (self.cursor < text.line_end(self.cursor))
                            .then(|| text.next(self.cursor)),
                        Key::Up | Key::Down => {
                            let line = if press.key == Key::Up {
                                text.prev_line(self.cursor)
                            } else {
                                text.next_line(self.cursor)
                            };
                            let column = self
                                .wanted_column
                                .unwrap_or_else(|| text.column(self.cursor));
                            line.map(|line| column_in_line(text, line, column, true))
                        }
                        Key::Home => Some(text.line_start(self.cursor)),
                        _ => Some(text.line_end(self.cursor)),
                    };
                let Some(to) = to else {
                    // A key that leads nowhere is as good as not pressed.
                    self.record.pop();
                    return;
                };
                self.wanted_column = match press.key {
                    Key::Up | Key::Down => self
                        .wanted_column
                        .or_else(|| Some(text.column(self.cursor))),
                    Key::End => Some(usize::MAX),
                    _ => None,
                };
                self.cursor = to;
                self.insert_count = 1;
                if !self.insert_moved {
                    // What was typed up to here is a complete insert: the
                    // one `.` repeats unless more is typed.
                    self.insert_moved = true;
                    if self.repeatable {
                        let mut keys = std::mem::take(&mut self.record);
                        keys.pop();
                        keys.push(KeyPress::plain(Key::Escape));
                        self.last_change = Some(Recorded {
                            count: self.record_count,
                            keys,
                            shape: self.record_shape.take(),
                        });
                    }
                }
            }
            _ => {
                if self.insert_moved {
                    // Typing after moving the cursor begins a new insert,
                    // with an undo step of its own.
                    self.insert_moved = false;
                    self.insert_start = place(host.text(), self.cursor);
                    self.insert_keys.clear();
                    self.insert_opened = false;
                    self.replaced.clear();
                    self.undo_cursor = None;
                    self.joined = self.joined && self.playing;
                    self.record = vec![KeyPress::plain(Key::Char('i')), press];
                    self.record_count = None;
                }
                if self.insert_count == 1 {
                    self.wanted_column = None;
                }
                if self.insert_apply(press, host) {
                    self.insert_keys.push(press);
                } else {
                    // A key that did nothing is as good as not pressed.
                    self.record.pop();
                }
            }
        }
    }

    /// What a key other than Escape and the cursor keys does in insert
    /// mode. Returns whether it did anything.
    fn insert_apply(&mut self, press: KeyPress, host: &mut impl Host) -> bool {
        let text = Text(host.text());
        match (press.key, press.ctrl) {
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
                let to = self.stop_at_insert_start(host.text(), start);
                self.insert_delete(to, host);
            }
            (Key::Char(c), false) if self.pairs && !self.replacing => self.insert_paired(c, host),
            (Key::Char(c), false) => self.insert_text(&c.to_string(), host),
            (Key::Enter, _) => self.insert_text("\n", host),
            (Key::Tab, _) => {
                // The name of a snippet becomes the snippet.
                if let Some((range, with, within)) = host.expand(self.cursor)
                    && range.end == self.cursor
                    && range.start <= range.end
                {
                    self.edit(host, range.clone(), &with);
                    self.cursor = range.start + within.min(with.len());
                    return true;
                }
                let column = text.column(self.cursor);
                self.insert_text(&" ".repeat(SHIFT_WIDTH - column % SHIFT_WIDTH), host);
            }
            (Key::Backspace, _) => return self.insert_backspace(host),
            (Key::Text(number), _) => {
                let Some((replaced, with, within)) = self.texts.get(number as usize).cloned()
                else {
                    return false;
                };
                // Back over that many characters, within the line.
                let mut from = self.cursor;
                for _ in 0..replaced {
                    if from == text.line_start(from) {
                        break;
                    }
                    from = text.prev(from);
                }
                self.edit(host, from..self.cursor, &with);
                self.cursor = from + within;
            }
            _ => return false,
        }
        true
    }

    fn insert_text(&mut self, typed: &str, host: &mut impl Host) {
        if typed.contains('\n') && host.text().is_empty() {
            // The one empty line of an empty text gets its line break, so
            // that the break typed makes another line.
            self.edit(host, 0..0, "\n");
        }
        let mut end = self.cursor;
        if self.replacing {
            // A character takes the place of the one under the cursor; at
            // the end of a line, and for a line break, there is none.
            let text = Text(host.text());
            if typed != "\n" && self.cursor < text.line_end(self.cursor) {
                end = text.next(self.cursor);
            }
            let original = host.text().get(self.cursor..end).unwrap_or("");
            self.replaced
                .push((end > self.cursor).then(|| original.to_owned()));
        }
        self.edit(host, self.cursor..end, typed);
        self.cursor += typed.len();
    }

    /// Types `c` where brackets come in pairs.
    fn insert_paired(&mut self, c: char, host: &mut impl Host) {
        let text = Text(host.text());
        let before = (self.cursor > 0)
            .then(|| text.char_at(text.prev(self.cursor)))
            .flatten();
        let after = text.char_at(self.cursor).filter(|c| *c != '\n');
        let closing = match c {
            '(' => Some(')'),
            '[' => Some(']'),
            '{' => Some('}'),
            '"' | '$' => Some(c),
            _ => None,
        };
        // The closing one where it stands already: over it.
        if matches!(c, ')' | ']' | '}' | '"' | '$') && after == Some(c) {
            self.cursor = text.next(self.cursor);
            return;
        }
        // A blank between two dollar signs is a displayed equation
        // begun: it needs the blank on the other side too.
        if c == ' ' && before == Some('$') && after == Some('$') {
            self.edit(host, self.cursor..self.cursor, "  ");
            self.cursor += 1;
            return;
        }
        // Before text an opening bracket is just that; and a quotation
        // mark or a dollar sign behind a letter or a digit closes
        // something, or is an apostrophe's cousin or a price.
        let room = after.is_none_or(|c| c.is_whitespace() || ")]}>,.;:$\"".contains(c));
        let opens = c != closing.unwrap_or(' ') || !before.is_some_and(char::is_alphanumeric);
        let escaped = before == Some('\\');
        match closing {
            Some(closing) if room && opens && !escaped => {
                let mut pair = c.to_string();
                pair.push(closing);
                self.edit(host, self.cursor..self.cursor, &pair);
                self.cursor += c.len_utf8();
            }
            _ => self.insert_text(&c.to_string(), host),
        }
    }

    /// Deletes the character before the cursor. Returns whether there was
    /// one.
    fn insert_backspace(&mut self, host: &mut impl Host) -> bool {
        if self.cursor == 0 {
            return false;
        }
        if self.pairs && !self.replacing {
            // Between a pair with nothing in it: both go.
            let text = Text(host.text());
            let before = text.char_at(text.prev(self.cursor));
            let after = text.char_at(self.cursor);
            let pair = matches!(
                (before, after),
                (Some('('), Some(')'))
                    | (Some('['), Some(']'))
                    | (Some('{'), Some('}'))
                    | (Some('"'), Some('"'))
                    | (Some('$'), Some('$'))
            );
            let from = text.prev(self.cursor);
            if pair && from >= self.stop_at_insert_start(host.text(), from) {
                let to = text.next(self.cursor);
                self.edit(host, self.cursor..to, "");
            }
        }
        if self.replacing {
            // Back over what was typed, bringing back what it replaced;
            // before that, only back, also onto the end of the line
            // before.
            let from = Text(host.text()).prev(self.cursor);
            if let Some(original) = self.replaced.pop() {
                let original = original.unwrap_or_default();
                self.edit(host, from..self.cursor, &original);
            }
            self.cursor = from;
            return true;
        }
        let text = Text(host.text());
        let joins_lines = text.line_start(self.cursor) == self.cursor;
        let to = text.prev(self.cursor);
        self.insert_delete(to, host);
        if joins_lines {
            // Backspacing onto the line before moves the start of the
            // insert there, as in Vim.
            self.insert_start = place(host.text(), self.cursor);
        }
        true
    }

    /// Where a deletion back to `to` really ends: at the start of the
    /// insert, if it would cross it. Once the cursor is at or before that
    /// start, nothing holds it back.
    fn stop_at_insert_start(&self, text: &str, to: usize) -> usize {
        // Vim deletes character by character and stops when it stands
        // exactly on the start; a start that earlier deletions left in
        // the middle of a character is stepped over.
        let (line, column) = self.insert_start;
        let lines = Text(text);
        if lines.line_index(self.cursor) != line {
            return to;
        }
        let start = lines.line_start(self.cursor) + column;
        if self.cursor > start && to < start && text.is_char_boundary(start) {
            start
        } else {
            to
        }
    }

    /// Deletes from `to` up to the cursor.
    fn insert_delete(&mut self, to: usize, host: &mut impl Host) {
        self.edit(host, to..self.cursor, "");
        self.cursor = to;
    }

    fn leave_insert(&mut self, host: &mut impl Host) {
        if self.insert_moved {
            // The insert `.` repeats was settled when the cursor moved.
            self.repeatable = false;
        } else {
            // A count types the same keys again, deletions and all.
            let keys = std::mem::take(&mut self.insert_keys);
            for _ in 1..self.insert_count {
                if self.insert_opened {
                    self.insert_text("\n", host);
                }
                for &press in &keys {
                    let _ = self.insert_apply(press, host);
                }
            }
        }
        self.insert_count = 1;
        self.mode = Mode::Normal;
        self.last_insert = Some(self.cursor);
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
        let cancel = |engine: &mut Self| {
            engine.mode = engine.prompt_return;
            engine.reset_pending();
        };
        match press.key {
            Key::Escape => cancel(self),
            Key::Char('c' | '[') if press.ctrl => cancel(self),
            Key::Backspace => {
                if self.command_line.pop().is_none() {
                    cancel(self);
                }
            }
            Key::Enter if self.prompt == ':' => {
                self.mode = Mode::Normal;
                let command = std::mem::take(&mut self.command_line);
                let command = command.trim();
                if let Ok(line) = command.parse::<usize>() {
                    self.set_jump();
                    let text = Text(host.text());
                    let start = text.line(line.saturating_sub(1));
                    self.cursor = text.first_non_blank(start);
                    self.wanted_column = None;
                } else if !command.is_empty() {
                    self.effects.push(Effect::Command(command.to_owned()));
                }
            }
            Key::Enter => {
                self.mode = self.prompt_return;
                let forward = self.prompt == '/';
                let typed = std::mem::take(&mut self.command_line);
                // Nothing typed: the last search again, this way.
                let pattern = if typed.is_empty() {
                    self.last_search.as_ref().map(|last| last.pattern.clone())
                } else {
                    Some(typed)
                };
                let Some(pattern) = pattern else {
                    return self.reset_pending();
                };
                self.last_search = Some(Search {
                    pattern,
                    forward,
                    word_start: false,
                    word_end: false,
                });
                self.search(forward, self.cursor, host);
            }
            Key::Char(c) if !press.ctrl && !press.alt => self.command_line.push(c),
            _ => {}
        }
    }

    /// Moves to a match of the last search, looking `forward` or backward
    /// from `from`, or applies the pending operator up to it.
    fn search(&mut self, forward: bool, from: usize, host: &mut impl Host) {
        let (count, _) = self.take_count();
        let found = self
            .last_search
            .as_ref()
            .and_then(|search| find_match(host.text(), from, search, forward, count));
        // Found or not, the cursor aims at the column it ends up in.
        self.wanted_column = None;
        if found.is_some() {
            // Where the cursor was is where two backticks go back to.
            self.set_jump();
        }
        let moved = found.map(|to| Moved {
            to,
            kind: Kind::Exclusive,
            column: Column::Reset,
        });
        self.finish_motion(moved, host);
    }

    /// `*` and `#`: searches for the word under the cursor, or the next one
    /// on the line; failing that, for what is under the cursor up to the
    /// next blank.
    fn search_word(&mut self, forward: bool, host: &mut impl Host) {
        let text = Text(host.text());
        let class_of = |c: char| crate::text::class(c, false);
        let line_end = text.line_end(self.cursor);
        let from = self.cursor.min(line_end);
        let rest = text.0.get(from..line_end).unwrap_or("");
        let word = rest.find(|c| class_of(c) == Class::Word);
        let Some(offset) = word.or_else(|| rest.find(|c| class_of(c) != Class::Blank)) else {
            return self.fail();
        };
        let at = from + offset;
        // Back to where the run of such characters begins.
        let class = text.char_at(at).map(class_of);
        let line_start = text.line_start(at);
        let before = text.0.get(line_start..at).unwrap_or("");
        let run = before.len()
            - before
                .trim_end_matches(|c| Some(class_of(c)) == class)
                .len();
        let start = at - run;
        // A word ends where it ends; anything else goes on to the next
        // blank.
        let tail = text.0.get(start..line_end).unwrap_or("");
        let rest_of = tail.trim_start_matches(|c| {
            if word.is_some() {
                class_of(c) == Class::Word
            } else {
                class_of(c) != Class::Blank
            }
        });
        let pattern = &tail[..tail.len() - rest_of.len()];
        let is_word = |c: Option<char>| c.map(class_of) == Some(Class::Word);
        self.last_search = Some(Search {
            pattern: pattern.to_owned(),
            forward,
            word_start: is_word(pattern.chars().next()),
            word_end: is_word(pattern.chars().next_back()),
        });
        self.search(forward, start, host);
    }
}

/// Where the `count`th match of `search` begins, looking from `from` and
/// going on at the other end of the text when its end is reached.
fn find_match(
    text: &str,
    from: usize,
    search: &Search,
    forward: bool,
    count: usize,
) -> Option<usize> {
    let pattern = search.pattern.as_str();
    if pattern.is_empty() {
        return None;
    }
    let is_word = |c: char| crate::text::class(c, false) == Class::Word;
    let mut matches = Vec::new();
    let mut at = 0;
    while let Some(offset) = text.get(at..).and_then(|rest| rest.find(pattern)) {
        let start = at + offset;
        let end = start + pattern.len();
        let continues_word =
            search.word_start && text[..start].chars().next_back().is_some_and(is_word);
        let goes_on = search.word_end && text[end..].chars().next().is_some_and(is_word);
        if !continues_word && !goes_on {
            matches.push(start);
        }
        // Matches may overlap: go on one character further, not behind
        // this match.
        at = start + text[start..].chars().next().map_or(1, char::len_utf8);
    }
    if matches.is_empty() {
        return None;
    }
    let count = count.max(1) - 1;
    let index = if forward {
        let next = matches.partition_point(|&start| start <= from);
        (next + count) % matches.len()
    } else {
        let before = matches.partition_point(|&start| start < from);
        let back = (count + 1) % matches.len();
        (before + matches.len() - back) % matches.len()
    };
    matches.get(index).copied()
}

/// The key a character of a register stands for when the register is
/// played as a macro.
fn key_of(c: char) -> KeyPress {
    let special = |key| KeyPress::plain(key);
    match c {
        '\u{1b}' => special(Key::Escape),
        '\r' | '\n' => special(Key::Enter),
        '\t' => special(Key::Tab),
        '\u{8}' | '\u{e000}' => special(Key::Backspace),
        '\u{e001}' => special(Key::Left),
        '\u{e002}' => special(Key::Right),
        '\u{e003}' => special(Key::Up),
        '\u{e004}' => special(Key::Down),
        '\u{e005}' => special(Key::Home),
        '\u{e006}' => special(Key::End),
        '\u{e007}' => special(Key::PageUp),
        '\u{e008}' => special(Key::PageDown),
        '\u{e100}'..='\u{f8ff}' => special(Key::Text(c as u32 - TEXT_KEYS)),
        '\u{1}'..='\u{1f}' => KeyPress {
            key: Key::Char(char::from(c as u8 | 0x60)),
            ctrl: true,
            alt: false,
        },
        c => special(Key::Char(c)),
    }
}

/// The character a recorded key is kept as in a register: itself, a
/// control character, or for keys that have none, one from Unicode's
/// private use area.
fn char_of(press: KeyPress) -> char {
    match press.key {
        Key::Char(c) if press.ctrl && c.is_ascii() => char::from(c as u8 & 0x1f),
        Key::Char(c) => c,
        Key::Escape => '\u{1b}',
        Key::Enter => '\r',
        Key::Tab => '\t',
        Key::Backspace => '\u{e000}',
        Key::Left => '\u{e001}',
        Key::Right => '\u{e002}',
        Key::Up => '\u{e003}',
        Key::Down => '\u{e004}',
        Key::Home => '\u{e005}',
        Key::End => '\u{e006}',
        Key::PageUp => '\u{e007}',
        Key::PageDown => '\u{e008}',
        // The rest of the private use area numbers the texts put in
        // whole; beyond that many, the last stands for all.
        Key::Text(number) => {
            char::from_u32(TEXT_KEYS + number.min(TEXT_KEYS_END - TEXT_KEYS)).unwrap_or('\u{f8ff}')
        }
    }
}

/// The characters that stand for [`Key::Text`] in a register.
const TEXT_KEYS: u32 = 0xe100;
const TEXT_KEYS_END: u32 = 0xf8ff;

/// `pos` as a line and a byte within it.
fn place(text: &str, pos: usize) -> LinePlace {
    let text = Text(text);
    let pos = pos.min(text.len());
    (text.line_index(pos), pos - text.line_start(pos))
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
///
/// With `past_end`, as in visual mode, a line too short for the column puts
/// the cursor on its end, behind the last character.
fn column_in_line(text: Text, line: usize, column: usize, past_end: bool) -> usize {
    let end = text.line_end(line);
    if past_end && (column == usize::MAX || column >= text.column(end)) {
        end
    } else if column == usize::MAX {
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
