//! The modal state machine: keys in, edits and cursor movements out.

use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::sync::Arc;

use crate::find::{self, Query, Reading};
use crate::keys::{Key, KeyPress};
use crate::motion::{self, Kind};
use crate::object::{self, Object};
use crate::text::{Class, Text};
use crate::took::Took;

mod follow;
mod read;
mod shown;

pub use follow::Following;

/// Columns one `>` or `<` shifts by, and the width of a typed Tab.
const SHIFT_WIDTH: usize = 2;

/// How many places jumped from are remembered, as in Vim.
const JUMPS: usize = 100;

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

    /// A place the cursor can be on what the host shows in place of the
    /// text's characters (a typeset page), as the bytes of the text that
    /// are shown there: a letter, or all that a reference turned into;
    /// no bytes at all, where it shows a part that is yet to be typed.
    /// Which one is asked for, `seek` says. `None` where there is none,
    /// and always where the host shows the text as it is: then the
    /// cursor goes by characters.
    fn stop(&self, at: usize, seek: Seek) -> Option<Range<usize>> {
        let _ = (at, seek);
        None
    }

    /// The line byte `at` is shown in, where the host shows the text in
    /// lines of its own making. `0`, `$`, `dd` and `V` go by it.
    fn shown_line(&self, at: usize) -> Option<Shown> {
        let _ = at;
        None
    }

    /// Where and what to put into the text to begin a new paragraph (or
    /// item of a list) below the one byte `at` is in, or above it: the
    /// byte, the text, and how far into that text the cursor goes. `o`
    /// and `O` ask where lines are not the host's to show.
    fn open(&self, at: usize, below: bool) -> Option<(usize, String, usize)> {
        let _ = (at, below);
        None
    }

    /// What Enter typed at byte `at` does to the text, where that is not
    /// simply putting a line break there: the bytes it replaces, and
    /// what with. The cursor goes behind that.
    fn line_break(&self, at: usize) -> Option<(Range<usize>, String)> {
        let _ = at;
        None
    }

    /// What typing the character `c` at byte `at` does to the text, where
    /// that is not simply putting `c` there: the bytes to replace, the
    /// text to put there, and how far into it the cursor goes. A host
    /// that shows the text as a page takes what is typed for what is
    /// meant to be read, and writes it so that it is.
    fn typed(&self, at: usize, c: char) -> Option<(Range<usize>, String, usize)> {
        let _ = (at, c);
        None
    }

    /// What to do at byte `at` to begin text set off as `format` there
    /// (or to end it, where the cursor is at the end of such text): the
    /// bytes to replace, the text to put there, and how far into it the
    /// cursor goes. Asked for Ctrl-B and Ctrl-I in insert mode.
    fn set_off(&self, at: usize, format: Format) -> Option<(Range<usize>, String, usize)> {
        let _ = (at, format);
        None
    }

    /// What Backspace takes at byte `at` where that is not what stands
    /// before it: a place to fill in that nothing was filled into (what
    /// was to be raised or lowered, a fraction with no parts) goes as a
    /// whole, with the marks that made it.
    ///
    /// The bytes that go, and what is left in their place; the cursor
    /// goes to where they began.
    fn unmake(&self, at: usize) -> Option<(Range<usize>, String)> {
        let _ = at;
        None
    }

    /// Where Tab leads from byte `at`, where the text has places to fill
    /// in one after the other: the arguments of a call, the cells of a
    /// table.
    ///
    /// The bytes to replace on the way (none, as a rule; behind the last
    /// cell of a table Tab makes a row), what with, and how far into
    /// that the cursor goes.
    fn next_field(&self, at: usize) -> Option<(Range<usize>, String, usize)> {
        let _ = at;
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

    /// What the character `c` is written as where it takes the place of
    /// what the host shows at bytes `stop` (see [`Host::stop`]; they are
    /// none for a part of an equation that is yet to be typed, which `c`
    /// then is): the bytes to replace, the text to put there, and how
    /// far into it what is shown of it begins. `None` where what is
    /// shown there is not text that a character can take the place of,
    /// or `c` is not for that place. Asked for `r`.
    fn put_over(&self, stop: Range<usize>, c: char) -> Option<(Range<usize>, String, usize)> {
        let _ = (stop, c);
        None
    }

    /// The parts of bytes `range` of the text that are read as the
    /// letters they are written with, in the order of the text and apart
    /// from each other: what `~` and its like may change the case of.
    /// `None` where the host shows the text as it is: there every letter
    /// is one.
    fn letters(&self, range: Range<usize>) -> Option<Vec<Range<usize>>> {
        let _ = range;
        None
    }

    /// Whether taking bytes `range` out of the text would take a mark
    /// that what the host shows is made with, and leave what it marks:
    /// one of the dollar signs of an equation, say. An operator then
    /// takes only what is shown there (see [`Host::stop`]). `false`
    /// where the host shows the text as it is.
    fn cuts(&self, range: Range<usize>) -> bool {
        let _ = range;
        false
    }

    /// Whether the host refuses `edits` (of the text as it is, in its
    /// order and apart), where it shows the text as something else:
    /// what it shows would not be read at all then (a word that comes to
    /// stand against the star of strong text leaves that unclosed). The
    /// host says why, as it says why an edit was not made; the command
    /// then changes nothing.
    fn refuses(&mut self, edits: &[Edit]) -> bool {
        let _ = edits;
        false
    }

    /// What joining the paragraph byte `at` is in with what follows it
    /// does to the text: the bytes that part the two, and what stands
    /// in their place (a blank if `spaced`, nothing for `gJ`). Bytes
    /// that are none, with nothing to put there, say that nothing
    /// follows. `None` where lines are not the host's to show: `J` then
    /// joins lines of the text.
    fn join(&self, at: usize, spaced: bool) -> Option<(Range<usize>, String)> {
        let _ = (at, spaced);
        None
    }

    /// Whether bytes `range` of the text go from one part of what the
    /// host shows that is typed into by itself to another: from a cell
    /// of a table to the next, from a footnote to the text it is in.
    /// Backspace and Delete in insert mode take nothing across that.
    fn parted(&self, range: Range<usize>) -> bool {
        let _ = range;
        false
    }

    /// What deleting bytes `range` of the text takes, `with` put where
    /// the first of it was: the parts of it that go, in the order of the
    /// text and apart from each other. A host that shows pages keeps the
    /// marks whose partners stay (the one that closes strong text, say),
    /// which would otherwise make all that follows something else.
    /// `None` where nothing may go.
    fn pieces(&self, range: Range<usize>, with: &str) -> Option<Vec<Range<usize>>> {
        let _ = with;
        Some(vec![range])
    }

    /// A text object of the host's language around byte `at`, named by
    /// the key after `i` (the inside) or `a` (`around`: all of it). The
    /// host says which keys those are; `None` for all others, and where
    /// there is no such object.
    fn object(&self, name: char, at: usize, around: bool) -> Option<Range<usize>> {
        let _ = (name, at, around);
        None
    }

    /// What the text is read as, where the host shows it as something to
    /// be read and not as it is written: a search goes over that (see
    /// [`crate::find`]). `None` where the text is shown as it is.
    fn reading(&self) -> Option<Arc<Reading>> {
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

/// Which of the places the cursor can be on (see [`Host::stop`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seek {
    /// The one that begins at the byte.
    At,
    /// The first that begins at the byte or behind it.
    After,
    /// The last that begins at the byte or before it.
    Before,
    /// The one after the one at the byte, in the same line.
    Next,
    /// The one before it, in the same line.
    Previous,
}

/// A line of what the host shows (see [`Host::shown_line`]): the byte it
/// begins with, the byte its last place for the cursor begins with, and
/// the byte behind what it shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shown {
    pub start: usize,
    pub last: usize,
    pub end: usize,
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
    /// along (`R`), and for each character typed so far where it stands
    /// and what it replaced: `None` where it was added to the text.
    /// Backspace brings those back.
    replacing: bool,
    replaced: Vec<(Range<usize>, Option<String>)>,
    /// Whether Ctrl-R was typed in this insert and waits for the name of
    /// the register to put in.
    insert_register: bool,
    /// After Ctrl-O while typing: typing goes on once the one command
    /// that follows is done. Whether the cursor was at the end of its
    /// line then, and where leaving insert mode put it.
    resume: Option<(bool, usize)>,
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
    /// The places jumped from, the oldest first, for Ctrl-O to go back
    /// to and Ctrl-I (Tab) forward again; and which of them Ctrl-O went
    /// to last: their number where it has not gone back.
    jumps: Vec<usize>,
    jumps_at: usize,
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
    /// How many searches were made: a host that shows their matches shows
    /// them again after each.
    searches: u64,
    /// What the last key yanked, deleted or put.
    took: Option<Took>,
    /// Whether the last key selected a text object, whose ends are where
    /// they are: not moved to rest on what the host shows.
    selected_object: bool,
    /// Whether `j` and `k` go by the rows the host shows instead of by
    /// lines of the text, as `gj` and `gk` always do where it shows any.
    rows_first: bool,
    /// Whether a bracket typed brings its closing one along (not Vim's).
    pairs: bool,
    /// Whether a selection begun with `V` is of lines the host shows:
    /// then it is a selection of characters that is kept to whole lines.
    line_select: bool,
    /// Where what is shown under the cursor ends in the text, and what
    /// is shown under the other end of a selection, where the host shows
    /// other things than characters.
    extent: Option<(usize, usize)>,
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
            insert_register: false,
            resume: None,
            replaced: Vec::new(),
            insert_moved: false,
            marks: HashMap::new(),
            jump: None,
            prev_jump: None,
            jumps: Vec::new(),
            jumps_at: 0,
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
            searches: 0,
            took: None,
            selected_object: false,
            rows_first: false,
            pairs: false,
            line_select: false,
            extent: None,
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

    /// Whether the next key is part of a command begun: what an operator
    /// goes over, the character `f` looks for, the name of a register.
    /// A count or a register named before a command begins none: the
    /// command is yet to come.
    #[must_use]
    pub fn awaits_part(&self) -> bool {
        self.pending.operator.is_some() || self.pending.awaiting != Awaiting::Nothing
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
                let end = if on_last_break { high } else { text.next(high) };
                // What is shown at the later end may be more than one
                // character of the text.
                let shown = self.extent.map_or(end, |(cursor, anchor)| {
                    if self.cursor >= self.anchor {
                        cursor
                    } else {
                        anchor
                    }
                });
                if self.extent.is_some() && shown == high && high > 0 {
                    // A place that shows no bytes (a part of an equation
                    // that is yet to be typed) has nothing to take.
                    return Some(low..high);
                }
                Some(low..end.max(shown.min(text.len())))
            }
            Mode::VisualLine => Some(text.line_start(low)..line_end_inclusive(text, high)),
            _ => None,
        }
    }

    /// Whether the selection is one of whole lines: `V`, also where the
    /// lines are the host's.
    #[must_use]
    pub fn selects_lines(&self) -> bool {
        self.mode == Mode::VisualLine || (self.mode == Mode::Visual && self.line_select)
    }

    /// The contents of a register and whether they are whole lines.
    #[must_use]
    pub fn register(&self, name: char) -> Option<(&str, bool)> {
        self.registers
            .get(&name)
            .map(|register| (register.text.as_str(), register.linewise))
    }

    /// Makes register `name` hold `text`, as what another program put
    /// there does (the system clipboard): whole lines if it ends with a
    /// line break.
    pub fn set_register(&mut self, name: char, text: &str) {
        let register = Register {
            text: text.to_owned(),
            linewise: text.ends_with('\n'),
        };
        self.registers.insert(name, register);
    }

    /// What the last search was for, if there was one.
    #[must_use]
    pub fn last_search(&self) -> Option<Query> {
        self.last_search.as_ref().map(|search| Query {
            pattern: search.pattern.clone(),
            word_start: search.word_start,
            word_end: search.word_end,
        })
    }

    /// How many searches have been made (`/`, `n`, `*` and their like).
    #[must_use]
    pub fn searches(&self) -> u64 {
        self.searches
    }

    /// What the last key yanked, deleted or put, if it did.
    #[must_use]
    pub fn took(&self) -> Option<&Took> {
        self.took.as_ref()
    }

    /// Handles one key press.
    pub fn key(&mut self, press: KeyPress, host: &mut impl Host) -> Vec<Effect> {
        // Ctrl-O while typing: one command of normal mode, and then on
        // typing, as in Vim.
        let once = self.mode == Mode::Insert
            && press.ctrl
            && press.key == Key::Char('o')
            && !self.insert_register;
        let at_end = once && {
            let text = Text(host.text());
            self.cursor == text.line_end(self.cursor) && !text.line_is_empty(self.cursor)
        };
        let press = if once {
            KeyPress::plain(Key::Escape)
        } else {
            press
        };
        self.rest_before_key(press, host);
        let before = self.cursor;
        self.took = None;
        self.selected_object = false;
        let mut effects = self.key_as_typed(press, host);
        if once {
            // The cursor stays where it was, but behind the end of a
            // line: there is nothing to stand on there.
            if !at_end {
                self.cursor = clamp(Text(host.text()), before, false);
            }
            self.resume = Some((at_end, self.cursor));
        } else if let Some((at_end, left)) = self.resume {
            match self.mode {
                Mode::Normal if self.pending.is_idle() => {
                    self.resume = None;
                    // Behind the last character again, where it was (as
                    // Vim does) or where `$` aimed.
                    let text = Text(host.text());
                    let last = !text.line_is_empty(self.cursor)
                        && text.next(self.cursor) == text.line_end(self.cursor);
                    let same_line = text.line_start(self.cursor) == text.line_start(left);
                    // So too where what the command deleted went up to
                    // the end of the line.
                    let deleted_end = matches!(&self.took, Some(Took::Deleted { at, .. })
                        if *at == text.line_end(self.cursor));
                    let behind = (at_end && same_line)
                        || deleted_end
                        || self.wanted_column == Some(usize::MAX);
                    let c = if last && behind { 'a' } else { 'i' };
                    effects.extend(self.key_as_typed(KeyPress::plain(Key::Char(c)), host));
                }
                Mode::Normal | Mode::Command => {}
                _ => self.resume = None,
            }
        }
        self.rest_on_shown(before, host);
        effects
    }

    /// Brings the cursor to rest where the host shows something, after a
    /// key: a host that shows the text as a page has places for the
    /// cursor that are not the characters of the text. Markup that shows
    /// as nothing is none; what a reference shows is one.
    fn rest_on_shown(&mut self, before: usize, host: &impl Host) {
        if self.mode != Mode::Visual {
            self.line_select = false;
        }
        self.extent = None;
        if !self.rows_first || !matches!(self.mode, Mode::Normal | Mode::Visual) {
            return;
        }
        // A text object selected ends where it ends: moved on to what is
        // shown next (behind the dollar sign that closes an equation, the
        // next paragraph) the selection would take more than it.
        if self.selected_object {
            self.extent = None;
            return;
        }
        // Whole lines deleted, and the cursor left on an empty one: a
        // page shows nothing there (it parts paragraphs), and is not yet
        // one of the text as it is. To the paragraph that follows, as
        // Vim goes to the line that follows, or else the one before.
        let lines_went = matches!(&self.took, Some(Took::Deleted { text, .. })
                if text.ends_with('\n') || text.starts_with('\n'));
        if lines_went && Text(host.text()).line_is_empty(self.cursor) {
            let text = Text(host.text());
            let filled = |mut line: Option<usize>, down: bool| {
                while let Some(at) = line {
                    if !text.line_is_empty(at) {
                        return Some(at);
                    }
                    line = if down {
                        text.next_line(at)
                    } else {
                        text.prev_line(at)
                    };
                }
                None
            };
            let found = filled(text.next_line(self.cursor), true)
                .or_else(|| filled(text.prev_line(self.cursor), false));
            if let Some(line) = found {
                self.cursor = first_non_blank_or_end(text, line);
                self.wanted_column = None;
            }
        }
        let rest = |at: usize, forward: bool| {
            if host.stop(at, Seek::At).is_some() {
                return at;
            }
            let (first, then) = if forward {
                (Seek::After, Seek::Before)
            } else {
                (Seek::Before, Seek::After)
            };
            let stop = host.stop(at, first).or_else(|| host.stop(at, then));
            stop.map_or(at, |stop| stop.start)
        };
        let moved = rest(self.cursor, self.cursor >= before);
        if moved != self.cursor {
            self.cursor = moved;
            self.wanted_column = None;
        }
        if self.mode == Mode::Visual && self.line_select {
            // Both ends at the ends of their lines, the lines between
            // and the two themselves taken whole.
            let lines = (host.shown_line(self.anchor), host.shown_line(self.cursor));
            let lines = (
                lines.0.map(|row| Self::rows_of_paragraph(row, host)),
                lines.1.map(|row| Self::rows_of_paragraph(row, host)),
            );
            if let (Some(anchor), Some(cursor)) = lines {
                if self.cursor >= self.anchor {
                    (self.anchor, self.cursor) = (anchor.start, cursor.last);
                } else {
                    (self.anchor, self.cursor) = (anchor.last, cursor.start);
                }
            }
        }
        let end = |at: usize| host.stop(at, Seek::At).map(|stop| stop.end);
        // The other end only where there is one: asking about a place
        // far from the cursor is not free.
        let anchor = if self.mode == Mode::Visual {
            end(self.anchor)
        } else {
            None
        };
        self.extent = end(self.cursor).zip(anchor.or(Some(0)));
    }

    /// All the rows of the paragraph that `row` (a line the host shows)
    /// is one of, as one line: a paragraph the page sets in several
    /// lines is one line to `V`, as it is to `dd` (see
    /// [`Engine::shown_lines`]) and as a line Vim wraps is one to both.
    /// `row` itself where it is no paragraph of running text (a heading,
    /// an item of a list), or where the host cannot say.
    fn rows_of_paragraph(row: Shown, host: &impl Host) -> Shown {
        let (Some((above, before, _)), Some((below, after, _))) =
            (host.open(row.start, false), host.open(row.start, true))
        else {
            return row;
        };
        if before != "\n\n" || after != "\n\n" {
            return row;
        }
        let text = Text(host.text());
        let end = text.line_end(below);
        if above > row.start || end < row.end || end <= above {
            return row;
        }
        let first = host
            .shown_line(above)
            .filter(|first| first.start <= row.start);
        let last = host
            .shown_line(text.prev(end))
            .filter(|last| last.last >= row.last);
        match (first, last) {
            (Some(first), Some(last)) => Shown {
                start: first.start.max(above),
                last: last.last,
                end: last.end,
            },
            _ => row,
        }
    }

    fn key_as_typed(&mut self, press: KeyPress, host: &mut impl Host) -> Vec<Effect> {
        // Outside insert mode the Delete key is `x`, as in Vim: what is
        // under the cursor, or what is selected.
        let deletes =
            press.key == Key::Delete && !matches!(self.mode, Mode::Insert | Mode::Command);
        let press = if deletes {
            KeyPress::plain(Key::Char('x'))
        } else if press.ctrl && press.key == Key::Char('h') {
            // Ctrl-H is Backspace, as it is in Vim: in every mode.
            KeyPress::plain(Key::Backspace)
        } else {
            press
        };
        // Where the host shows pages, Ctrl-B and Ctrl-I make what is
        // selected strong or emphasised, as they do while typing: `gb`
        // and `gm` by other keys.
        let sets_off = self.rows_first
            && self.mode == Mode::Visual
            && press.ctrl
            && matches!(press.key, Key::Char('b' | 'i'));
        let press = if sets_off {
            let operator = if press.key == Key::Char('b') {
                'b'
            } else {
                'm'
            };
            self.queue.push_back(KeyPress::plain(Key::Char(operator)));
            KeyPress::plain(Key::Char('g'))
        } else {
            press
        };
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

    /// Puts the cursor at `pos`, or the nearest place it may be, as a
    /// pointer does. No key asked for it: a command half typed is
    /// forgotten and a selection ends, as Escape ends them. While
    /// typing, typing goes on at the new place as it does after a cursor
    /// key: what was typed up to here is an insert of its own for undo
    /// and `.`. Nothing happens on the command line. Returns whether the
    /// cursor was put there.
    pub fn point(&mut self, pos: usize, text: &str) -> bool {
        let text = Text(text);
        match self.mode {
            Mode::Command => return false,
            Mode::Insert => {
                self.cursor = clamp(text, pos, true);
                self.insert_count = 1;
                if !self.insert_moved {
                    self.insert_moved = true;
                    if self.repeatable {
                        let mut keys = std::mem::take(&mut self.record);
                        keys.push(KeyPress::plain(Key::Escape));
                        self.last_change = Some(Recorded {
                            count: self.record_count,
                            keys,
                            shape: self.record_shape.take(),
                        });
                    }
                }
            }
            Mode::Normal | Mode::Visual | Mode::VisualLine => {
                self.reset_pending();
                self.mode = Mode::Normal;
                self.line_select = false;
                self.extent = None;
                self.cursor = clamp(text, pos, false);
            }
        }
        self.wanted_column = None;
        true
    }

    /// Selects from `anchor` to `cursor`, both of them included, as `v`
    /// at the one and a motion to the other do; with `lines`, the lines
    /// of the text the two are in, as `V` does. This is for a pointer,
    /// as [`Engine::point`] is: a command half typed is forgotten. Where
    /// the host shows other things than characters, each end comes to
    /// rest on what is shown at it, or else on the nearest thing shown
    /// between the two. Nothing happens while typing or on the command
    /// line. Returns whether the selection was made.
    pub fn select(&mut self, anchor: usize, cursor: usize, lines: bool, host: &impl Host) -> bool {
        if matches!(self.mode, Mode::Insert | Mode::Command) {
            return false;
        }
        let text = Text(host.text());
        self.reset_pending();
        if self.mode == Mode::Normal {
            // A new command starts, as it does with `v`.
            self.undo_cursor = None;
            self.record.clear();
            self.record_count = None;
            self.command_start = 0;
            self.record_shape = None;
            self.changed = false;
            self.joined = false;
            self.repeatable = true;
        }
        self.mode = if lines {
            Mode::VisualLine
        } else {
            Mode::Visual
        };
        self.line_select = false;
        self.reselected = false;
        self.anchor = clamp(text, anchor, true);
        self.cursor = clamp(text, cursor, true);
        self.wanted_column = None;
        self.extent = None;
        if self.rows_first && !lines {
            let forward = self.cursor >= self.anchor;
            let rest = |at: usize, forward: bool| {
                if host.stop(at, Seek::At).is_some() {
                    return at;
                }
                let seek = if forward { Seek::After } else { Seek::Before };
                host.stop(at, seek).map_or(at, |stop| stop.start)
            };
            let (from, to) = (rest(self.anchor, forward), rest(self.cursor, !forward));
            // Not past each other: then nothing is shown between them.
            if (forward && from <= to) || (!forward && from >= to) {
                (self.anchor, self.cursor) = (from, to);
            }
            let end = |at: usize| host.stop(at, Seek::At).map(|stop| stop.end);
            self.extent = end(self.cursor).zip(end(self.anchor).or(Some(0)));
        }
        self.settle();
        true
    }

    /// The end of the selection that the cursor is not at, if there is a
    /// selection.
    #[must_use]
    pub fn anchor(&self) -> Option<usize> {
        self.selecting().then_some(self.anchor)
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
        self.remember_jump();
    }

    /// Puts where the cursor is at the end of the places jumped from,
    /// and nowhere else in them.
    fn remember_jump(&mut self) {
        let at = self.cursor;
        self.jumps.retain(|jumped| *jumped != at);
        self.jumps.push(at);
        if self.jumps.len() > JUMPS {
            self.jumps.remove(0);
        }
        self.jumps_at = self.jumps.len();
    }

    /// Ctrl-O (`back`) and Ctrl-I or Tab: back through the places jumped
    /// from, `count` of them, and forward again, as in Vim. Going back
    /// from where the last jump led remembers that place too, for Ctrl-I
    /// to come back to.
    fn jump_list(&mut self, back: bool, host: &impl Host) {
        let (count, _) = self.take_count();
        if back && self.jumps_at >= self.jumps.len() {
            self.remember_jump();
            self.jumps_at = self.jumps.len() - 1;
        }
        let to = if back {
            self.jumps_at.checked_sub(count)
        } else {
            Some(self.jumps_at.saturating_add(count))
        };
        let Some((to, &at)) = to.and_then(|to| Some((to, self.jumps.get(to)?))) else {
            return self.fail();
        };
        self.jumps_at = to;
        self.cursor = clamp(Text(host.text()), at, false);
        self.wanted_column = None;
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
        self.jumps.iter_mut().for_each(shift);
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
        // A text that is not empty ends with a line break, as Vim writes it:
        // of the kind its lines end with.
        let now = host.text();
        if !now.is_empty() && !now.ends_with('\n') {
            let end = now.len();
            let ending = if now.contains("\r\n") { "\r\n" } else { "\n" };
            host.edit(
                &[Edit {
                    range: end..end,
                    text: ending.to_owned(),
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
                    Key::Char('\'' | '`') => {
                        self.jump = Some(self.cursor);
                        self.remember_jump();
                    }
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
                if self.shown_find((target, forward, till), false, host) {
                    return;
                }
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
                    // And to what is misspelt, as in Vim.
                    Key::Char('s') if !press.ctrl => "issue",
                    // And to the notes on the text.
                    Key::Char('n') if !press.ctrl => "notes",
                    // And to what the page shows as nothing.
                    Key::Char('i') if !press.ctrl => "invisible",
                    _ => return self.fail(),
                };
                let way = if forward { "next" } else { "previous" };
                self.effects.push(Effect::Command(format!("{what} {way}")));
                return;
            }
            Awaiting::View => {
                self.reset_pending();
                // Vim's keys for spelling are the host's to act on.
                let spelling = match press.key {
                    Key::Char('=') => Some("suggest"),
                    Key::Char('g') => Some("word good"),
                    Key::Char('w') => Some("word wrong"),
                    // Not Vim's: a proposed change shown on the page as
                    // it is, or as it would be.
                    Key::Char('p') => Some("proposal flip"),
                    _ => None,
                };
                if let Some(command) = spelling {
                    self.effects.push(Effect::Command(command.to_owned()));
                    return;
                }
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
                // Tab is Ctrl-I, as in Vim.
                Key::Tab if !visual && self.pending.operator.is_none() => {
                    self.jump_list(false, host);
                }
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
                'o' | 'i' if !visual && self.pending.operator.is_none() => {
                    self.jump_list(c == 'o', host);
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
            'x' | 'X' | 's' if self.shown_shorthand(c, host) => {}
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
            // As Vim's `gO` lists the headings of a help file: the host's
            // list of the document's sections, to rearrange them.
            'O' if self.pending.is_idle() && !self.selecting() => {
                self.effects.push(Effect::Command("structure".to_owned()));
            }
            // The host's list of all there is to do, and its list of
            // what there is to decide about the document.
            '?' | 'D' | 'B' if self.pending.is_idle() && !self.selecting() => {
                let command = match c {
                    '?' => "help",
                    'D' => "doc",
                    _ => "bib",
                };
                self.effects.push(Effect::Command(command.to_owned()));
            }
            // Not Vim's `gx` (which opens what is under the cursor
            // elsewhere): the equation there as it is written.
            'x' if self.pending.is_idle() && !self.selecting() => {
                self.effects.push(Effect::Command("written".to_owned()));
            }
            // Not Vim's: the host's list of the pages (sheets, Seiten).
            'S' if self.pending.is_idle() && !self.selecting() => {
                self.effects.push(Effect::Command("pages".to_owned()));
            }
            // Nor Vim's `gF`: the host's list of the figures and tables.
            'F' if self.pending.is_idle() && !self.selecting() => {
                self.effects.push(Effect::Command("figures".to_owned()));
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
            // Behind an operator, Vim's `gn` and `gN`: the next match of
            // the search, or the one before (`cgn` and then `.`).
            'n' | 'N' if self.pending.operator.is_some() => self.search_object(c == 'n', host),
            // Nor Vim's `gn` and `gq`: a note on what is selected or under
            // the cursor, and a question about it for an agent, whose text
            // the host asks for.
            'n' | 'q' if self.pending.is_idle() => {
                let command = if c == 'n' { "note" } else { "ask" };
                self.effects.push(Effect::Command(command.to_owned()));
            }
            // Nor Vim's `gh` (which selects): the host's list of the steps
            // of the undo history, to look at and to go back to.
            'h' if self.pending.is_idle() && !self.selecting() => {
                self.effects.push(Effect::Command("history".to_owned()));
            }
            // Nor Vim's `gw` (which formats): the host's labels on the
            // words in view, to jump to one by typing its label. In
            // visual mode too, to extend the selection.
            'w' if self.pending.is_idle() => {
                self.effects.push(Effect::Command("jump".to_owned()));
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
            let mut linewise = self.mode == Mode::VisualLine;
            let mut range = range;
            // Lines a page shows, selected with `V`, that are all there
            // is of lines of the text: deleted or yanked as lines, as
            // they are in Vim, for `p` to put back as lines.
            let text = Text(host.text());
            let whole = self.line_select
                && matches!(operator, Operator::Delete | Operator::Yank)
                && range.start == text.line_start(range.start)
                && range.end == text.line_end(range.end)
                && range.start < range.end;
            if whole {
                linewise = true;
                range.end = line_end_inclusive(text, range.end);
            }
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
            Some(pending) if pending == operator => {
                if let Some(lines) = self.shown_lines(operator, host) {
                    self.pending.operator = None;
                    return self.operate(operator, lines, 1, lines.start, host);
                }
                self.motion_key('_', host);
            }
            Some(_) => self.fail(),
        }
    }

    /// The lines the host shows that a doubled operator with a count is
    /// about, from the cursor's on: as characters, since they are not
    /// lines of the text. `None` where the host shows the text's own
    /// lines, and for operators that are about those in any case.
    fn shown_lines(&mut self, operator: Operator, host: &impl Host) -> Option<Object> {
        if !self.rows_first || matches!(operator, Operator::Indent | Operator::Dedent) {
            return None;
        }
        let first = host.shown_line(self.cursor)?;
        let (count, _) = self.take_count();
        let last = if count > 1 {
            let rows = isize::try_from(count - 1).unwrap_or(isize::MAX);
            let (below, _) = host.row(self.cursor, rows, None)?;
            host.shown_line(below)?
        } else {
            first
        };
        let (mut start, mut end) = (first.start.min(last.start), first.end.max(last.end));
        let text = Text(host.text());
        // A paragraph the page sets in several lines is one line to
        // whoever wrote it, as a line Vim wraps is one to `dd`: all of
        // it, not the row of it the cursor is in. Taken by the row, the
        // rest would close up and `p` would put the row into a word.
        if let (Some((above, before, _)), Some((below, after, _))) =
            (host.open(start, false), host.open(start, true))
            && before == "\n\n"
            && after == "\n\n"
            && above <= start
            && end <= text.line_end(below)
        {
            start = above;
            end = end.max(text.line_end(below));
        }
        // Lines that show all there is of the lines of the text they are
        // in (an item of a list, a heading) go with those lines whole:
        // with the marks that made them what they were, which would else
        // make something of what follows.
        let (from, to) = (text.line_start(start), text.line_end(end));
        let shown = |at: usize, until: usize| {
            host.stop(at, Seek::After)
                .is_some_and(|stop| stop.start < until && !stop.is_empty())
        };
        let whole = matches!(operator, Operator::Delete | Operator::Yank);
        if whole && !shown(from, start) && !shown(end, to) {
            // Whole lines, as Vim's `dd` and `yy` take them: `p` puts
            // them back as lines, paragraphs.
            let (mut from, mut to) = (from, line_end_inclusive(text, to));
            // A paragraph with empty lines on both sides takes one of them
            // along, else they pile up where it was.
            let apart_before = from == 0 || text.line_is_empty(text.prev(from));
            let apart_after = to >= text.len() || text.line_is_empty(to);
            if operator == Operator::Delete && apart_before && apart_after {
                if to < text.len() {
                    to = line_end_inclusive(text, to);
                } else if from > 0 {
                    from = text.line_start(text.prev(from));
                }
            }
            return Some(Object {
                start: from,
                end: to,
                linewise: true,
            });
        }
        // With the blank that parts them from what follows, unless the
        // lines are to be written anew.
        let follows = (text.char_at(end), text.char_at(text.next(end)));
        let blank = matches!(follows, (Some(' '), _))
            || matches!(follows, (Some('\n'), next) if next != Some('\n'));
        if blank && operator != Operator::Change {
            end = text.next(end);
        }
        Some(Object {
            start,
            end,
            linewise: false,
        })
    }

    /// `x`, `X` and `s` on what the host shows under the cursor (or, for
    /// `X`, before it), where that is not a character of the text.
    /// Returns whether it was done.
    fn shown_shorthand(&mut self, c: char, host: &mut impl Host) -> bool {
        if !self.rows_first || self.mode != Mode::Normal {
            return false;
        }
        let Some(mut stop) = host.stop(self.cursor, Seek::At) else {
            return false;
        };
        let (count, _) = self.take_count();
        if stop.is_empty() && c != 'X' {
            // Nothing is there to take: see `shown_empty`.
            self.shown_empty(c, stop.start, host);
            return true;
        }
        if c == 'X' {
            // With a count, that many of them.
            let before = Self::shown_in_line(host, &stop, false, count);
            let Some(nearest) = before.first() else {
                self.fail();
                return true;
            };
            stop = before.iter().fold(nearest.clone(), |all, one| {
                all.start.min(one.start)..all.end.max(one.end)
            });
        } else {
            for next in Self::shown_in_line(host, &stop, true, count - 1) {
                stop = stop.start.min(next.start)..stop.end.max(next.end);
            }
        }
        let operator = if c == 's' {
            Operator::Change
        } else {
            Operator::Delete
        };
        let object = Object {
            start: stop.start,
            end: stop.end,
            linewise: false,
        };
        self.reset_pending();
        self.operate(operator, object, 1, object.start, host);
        true
    }

    fn visual_key(&mut self, c: char, host: &impl Host) {
        // Lines the host shows are selected as characters, kept to whole
        // lines.
        if self.rows_first && host.shown_line(self.cursor).is_some() {
            self.pending = Pending::default();
            let lines = c == 'V';
            if self.mode == Mode::Visual && self.line_select == lines {
                self.mode = Mode::Normal;
                self.line_select = false;
            } else {
                if self.mode == Mode::Normal {
                    self.anchor = self.cursor;
                }
                self.mode = Mode::Visual;
                self.line_select = lines;
            }
            return;
        }
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
            'p' => object::paragraph(text, pos, around, count)
                .map(|found| self.shown_paragraph(found, pos, host)),
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
                self.selected_object = true;
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
        // `+` (Enter) and `-` likewise, to the start of that row: by the
        // lines of the text, `-` went to the empty line between two
        // paragraphs and came to rest on the end of the one before.
        if matches!(c, '+' | '-') && self.rows_first && self.pending.operator.is_none() {
            self.row_key(c == '+', host);
            if let Some(line) = host.shown_line(self.cursor) {
                self.cursor = line.start;
                self.wanted_column = None;
            }
            return;
        }
        // Sideways, and to what was last looked for in the line: by
        // what the host shows, where that is not the text.
        if matches!(c, 'h' | 'l' | ' ' | '\u{8}') && self.shown_sideways(c, host) {
            return;
        }
        if let (';' | ',', Some((target, forward, till))) = (c, self.last_find)
            && self.shown_find((target, forward == (c == ';'), till), true, host)
        {
            return;
        }
        // Words as the page reads them: the star of strong text is no
        // word there, and `b` came to rest on the blank before it.
        // Behind an operator `b` and `e` too (`db`); `w` there has rules
        // of its own (`cw`, the end of a line).
        let words = if self.pending.operator.is_none() {
            "wWbBeE"
        } else {
            "bBeE"
        };
        if words.contains(c)
            && self.rows_first
            && let Some(moved) = self.shown_word(c, host)
        {
            return self.finish_motion(Some(moved), host);
        }
        let back = c == '{' && self.rows_first && self.mode == Mode::Normal;
        let back = back && self.pending.operator.is_none();
        let before = self.cursor;
        let moved = self.motion(c, host);
        self.finish_motion(moved, host);
        if back {
            self.shown_paragraph_start(before, host);
        }
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
        // The start and the end of a line are those of the line the host
        // shows, where it shows lines of its own.
        if self.rows_first
            && matches!(c, '0' | '^' | '$')
            && count == 1
            && let Some(line) = host.shown_line(pos)
        {
            return match c {
                '$' if operating => horizontal(line.end, Kind::Exclusive),
                '$' => horizontal(line.last, Kind::Inclusive),
                _ => horizontal(line.start, Kind::Exclusive),
            };
        }
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
                let to = self.past_unshown(pos, to, host, |at| {
                    motion::word_forward(text, at, 1, big, false).0
                });
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
                let to = self.past_unshown(pos, to, host, |at| {
                    motion::word_backward(text, at, 1, c == 'B').0
                });
                horizontal(self.partial(text, to, ok)?, Kind::Exclusive)
            }
            'e' | 'E' => {
                let (to, ok) = motion::word_end(text, pos, count, c == 'E', false);
                let to = self.past_unshown(pos, to, host, |at| {
                    motion::word_end(text, at, 1, c == 'E', false).0
                });
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

    /// Where a word motion from byte `from` that got to `to` leads where
    /// the host shows pages: on past the marks of strong or emphasised
    /// text that show as nothing (the star that closes it), which are no
    /// word there, by `step` from each. Only for the cursor itself, and
    /// where it stands on what the host shows: then the host knows what
    /// that is.
    fn past_unshown(
        &self,
        from: usize,
        to: usize,
        host: &impl Host,
        step: impl Fn(usize) -> usize,
    ) -> usize {
        let cursor_only = self.pending.operator.is_none() && !self.selecting();
        if !self.rows_first || !cursor_only || host.stop(from, Seek::At).is_none() {
            return to;
        }
        let text = Text(host.text());
        let mut to = to;
        for _ in 0..8 {
            let mark = matches!(text.char_at(to), Some('*' | '_'));
            let next = step(to);
            if !mark || host.stop(to, Seek::At).is_some() || next == to {
                break;
            }
            to = next;
        }
        to
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
            (start, end) = self.shown_lines_within(start, end, host);
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
        // On a page, what would cut what it shows in two is taken as the
        // places shown in it, and the marks between them stay.
        let parts = self.shown_parts(operator, &object, host);
        if self.shown_refused(operator, &object, parts.as_deref(), host) {
            return;
        }
        match operator {
            Operator::Delete | Operator::Change if parts.is_some() => {
                self.delete_parts(operator, parts.as_deref().unwrap_or_default(), host);
            }
            Operator::Yank if parts.is_some() => {
                // What is shown, and not half of the marks around it:
                // put elsewhere, it would make something else of what
                // follows there.
                let text = host.text();
                let shown: String = parts
                    .iter()
                    .flatten()
                    .filter_map(|part| text.get(part.clone()))
                    .collect();
                self.store(shown, false, false);
                self.took = Some(Took::Yanked(start..end));
                self.cursor = clamp(Text(host.text()), landing, false);
            }
            Operator::Yank => {
                let mut yanked = selected;
                if !linewise && let Some(pieces) = host.pieces(start..end, "") {
                    // Not the marks whose partners are not taken along.
                    yanked = pieces
                        .into_iter()
                        .filter_map(|piece| host.text().get(piece))
                        .collect();
                }
                if linewise && !yanked.ends_with('\n') {
                    yanked.push('\n');
                }
                self.store(yanked, linewise, false);
                self.took = Some(Took::Yanked(start..end));
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
                let whole = start..end;
                let pieces = if linewise {
                    Some(vec![whole])
                } else {
                    host.pieces(start..end, "")
                };
                let Some(pieces) = pieces.filter(|pieces| !pieces.is_empty()) else {
                    // It would leave the text other than it shows.
                    (self.undo_cursor, self.undo_block) = (None, None);
                    return self.fail();
                };
                let mut deleted: String = pieces
                    .iter()
                    .filter_map(|piece| host.text().get(piece.clone()))
                    .collect();
                if linewise && !deleted.ends_with('\n') {
                    deleted.push('\n');
                }
                self.took = Some(Took::Deleted {
                    text: deleted.clone(),
                    at: start,
                    linewise,
                });
                self.store(deleted, linewise, true);
                for piece in pieces.into_iter().rev() {
                    self.edit(host, piece, "");
                }
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
                self.took = Some(Took::Deleted {
                    text: selected.clone(),
                    at: start,
                    linewise,
                });
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
                    let Some(pieces) = host.pieces(start..end, "") else {
                        (self.undo_cursor, self.undo_block) = (None, None);
                        return self.fail();
                    };
                    if pieces.len() != 1 || pieces[0] != (start..end) {
                        changed = pieces
                            .iter()
                            .filter_map(|piece| host.text().get(piece.clone()))
                            .collect();
                    }
                    let nothing = changed.is_empty() && nothing_selected;
                    if !nothing && !host.text().is_empty() {
                        self.store(changed, false, true);
                    }
                    for piece in pieces.into_iter().rev() {
                        self.edit(host, piece, "");
                    }
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
                } else if !self.shown_case(operator, start..end, host) {
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
            // On a page lines are put as paragraphs of their own, beside
            // the one the cursor is in: put into its lines, they would
            // be part of it. (In a list they are lines, items.)
            let paragraph = host
                .open(self.cursor, after)
                .filter(|(_, with, _)| self.rows_first && with == "\n\n");
            if let Some((at, ..)) = paragraph {
                let body = inserted.trim_end_matches('\n');
                let (put, start) = if after {
                    (format!("\n\n{body}"), at + 2)
                } else {
                    (format!("{body}\n\n"), at)
                };
                self.edit(host, at..at, &put);
                self.took = Some(Took::Put(start..start + body.len()));
                self.cursor = Text(host.text()).first_non_blank(start);
                self.wanted_column = None;
                return;
            }
            let text = Text(host.text());
            let at = if after {
                (text.line_end(self.cursor) + 1).min(text.len())
            } else {
                text.line_start(self.cursor)
            };
            self.edit(host, at..at, &inserted);
            self.took = Some(Took::Put(at..at + inserted.len()));
            self.cursor = Text(host.text()).first_non_blank(at);
        } else {
            let text = Text(host.text());
            // On a page, behind or before what the cursor is on there:
            // not into the middle of an escaped sign or a reference.
            let shown = host
                .stop(self.cursor, Seek::Before)
                .filter(|stop| stop.start <= self.cursor && self.cursor < stop.end);
            // Or into the place to fill in that a page shows there.
            let empty = (self.rows_first && after)
                .then(|| host.stop(self.cursor, Seek::At))
                .flatten()
                .filter(|stop| stop.start == self.cursor && stop.is_empty());
            let at = match shown {
                _ if empty.is_some() => self.cursor,
                Some(stop) if self.rows_first => {
                    if after {
                        stop.end
                    } else {
                        stop.start
                    }
                }
                _ if after && !text.line_is_empty(self.cursor) => text.next(self.cursor),
                _ => self.cursor,
            };
            // Not where it would leave the text other than it shows
            // (see `Host::pieces`): strong text left open, say. Nor
            // where the page could not be read then: a letter put
            // against the star of strong text leaves it unclosed.
            let edit = Edit {
                range: at..at,
                text: inserted.clone(),
            };
            if self.rows_first
                && (host.pieces(at..at, &inserted).is_none()
                    || host.refuses(std::slice::from_ref(&edit)))
            {
                return self.fail();
            }
            self.edit(host, at..at, &inserted);
            self.took = Some(Took::Put(at..at + inserted.len()));
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
        let mut inserted = register.text.clone();
        if linewise_selection && !register.linewise {
            inserted.push('\n');
        } else if !linewise_selection && register.linewise {
            inserted.insert(0, '\n');
        }
        // Marks whose partners are not selected stay (see `Host::pieces`).
        let pieces = if linewise_selection || register.linewise {
            Some(vec![range.clone()])
        } else {
            host.pieces(range.clone(), &inserted)
        };
        let Some(pieces) = pieces else {
            return self.fail();
        };
        let replaced: String = pieces
            .iter()
            .filter_map(|piece| host.text().get(piece.clone()))
            .collect();
        let start = pieces.first().map_or(range.start, |piece| piece.start);
        for piece in pieces.iter().skip(1).rev() {
            self.edit(host, piece.clone(), "");
        }
        let first = pieces.first().cloned().unwrap_or(start..start);
        self.edit(host, first, &inserted);
        self.took = Some(Took::Put(start..start + inserted.len()));
        if keep {
            self.store(replaced, linewise_selection, true);
        }
        let text = Text(host.text());
        let landed = if register.linewise {
            let line = if linewise_selection { start } else { start + 1 };
            text.first_non_blank(line.min(text.len()))
        } else if inserted.contains('\n') && !linewise_selection {
            start
        } else {
            text.prev(start + register.text.len()).max(start)
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
        if self.shown_join(spaced, host) {
            return;
        }
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
        if self.shown_replace(press, host) {
            return;
        }
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
        if self.shown_toggle_case(host) {
            return;
        }
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
        self.insert_register = false;
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

    /// `a`, `A`, `I`, `o` and `O` where the host shows other things than
    /// the text's characters and lines: they go by what it shows. Returns
    /// `i` once the cursor is where typing begins, the key itself where
    /// there is nothing of the host's to go by.
    fn start_insert_shown(&mut self, c: char, host: &mut impl Host) -> char {
        if !self.rows_first {
            return c;
        }
        let Some(line) = host.shown_line(self.cursor) else {
            return c;
        };
        match c {
            'a' => {
                if let Some(stop) = host.stop(self.cursor, Seek::At) {
                    self.cursor = stop.end;
                }
            }
            'I' => self.cursor = line.start,
            'A' => self.cursor = line.end,
            'o' | 'O' => {
                let Some((at, with, within)) = host.open(self.cursor, c == 'o') else {
                    return c;
                };
                self.edit(host, at..at, &with);
                self.cursor = at + within.min(with.len());
            }
            _ => return c,
        }
        'i'
    }

    fn start_insert(&mut self, c: char, host: &mut impl Host) {
        let (count, _) = self.take_count();
        self.reset_pending();
        let c = self.start_insert_shown(c, host);
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
                // Where the host shows the text as pages, by what is
                // on them: symbols and typeset lines.
                let to = self.insert_arrow_on_page(press.key, host).or(to);
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

    /// Where a cursor key leads while typing where the host shows the
    /// text as pages: left and right by what is shown (a symbol written
    /// with several letters is one step), up and down by typeset lines,
    /// Home and End to the ends of the typeset line. `None` where the
    /// host cannot tell.
    fn insert_arrow_on_page(&mut self, key: Key, host: &impl Host) -> Option<usize> {
        if !self.rows_first {
            return None;
        }
        let text = Text(host.text());
        match key {
            Key::Left => {
                let before = text.prev(self.cursor);
                let stop = host.stop(before, Seek::Before)?;
                (stop.end <= self.cursor && stop.start < self.cursor).then_some(stop.start)
            }
            Key::Right => {
                let stop = host.stop(self.cursor, Seek::After)?;
                if stop.is_empty() && stop.start == self.cursor {
                    // Out of a place with nothing in it yet: to before
                    // what is shown next, not into the markup between.
                    let next = host.stop(text.next(self.cursor), Seek::After)?;
                    return (next.start > self.cursor).then_some(next.start);
                }
                (stop.end > self.cursor).then_some(stop.end)
            }
            Key::Up | Key::Down => {
                let goal = self
                    .row_goal
                    .filter(|(column, _)| self.wanted_column == Some(*column))
                    .map(|(_, across)| across);
                let rows = if key == Key::Down { 1 } else { -1 };
                let (to, across) = host.row(self.cursor, rows, goal)?;
                let column = self
                    .wanted_column
                    .unwrap_or_else(|| text.column(self.cursor));
                self.row_goal = Some((column, across));
                Some(to)
            }
            Key::Home => host.shown_line(self.cursor).map(|line| line.start),
            Key::End => host.shown_line(self.cursor).map(|line| line.end),
            _ => None,
        }
    }

    /// Enter in insert mode: a line break, or what the host makes of it
    /// where it shows the text as pages.
    fn insert_enter(&mut self, host: &mut impl Host) {
        let typed = if self.rows_first {
            host.line_break(self.cursor)
        } else {
            None
        };
        match typed {
            Some((range, with)) if !range.is_empty() && range.end <= host.text().len() => {
                self.edit(host, range.clone(), &with);
                self.cursor = range.start + with.len();
            }
            Some((_, with)) => self.insert_text(&with, host),
            None => self.insert_text("\n", host),
        }
    }

    /// Ctrl-B and Ctrl-I where the host shows the text as pages: begins
    /// strong (or emphasised) text at the cursor, or leaves it. Returns
    /// whether it did anything.
    fn insert_set_off(&mut self, strong: bool, host: &mut impl Host) -> bool {
        let format = if strong { Format::Strong } else { Format::Emph };
        let Some((range, with, within)) = host.set_off(self.cursor, format) else {
            return false;
        };
        if !(range.is_empty() && with.is_empty()) {
            self.edit(host, range.clone(), &with);
        }
        self.cursor = (range.start + within).min(host.text().len());
        true
    }

    /// Types `c` where the host shows the text as pages: as what the
    /// host takes it to mean, if it has a say.
    fn insert_as_meant(&mut self, c: char, host: &mut impl Host) {
        match host.typed(self.cursor, c) {
            Some((range, with, within)) => {
                self.edit(host, range.clone(), &with);
                self.cursor = range.start + within.min(with.len());
            }
            None if self.pairs => self.insert_paired(c, host),
            None => self.insert_text(&c.to_string(), host),
        }
    }

    /// What a key other than Escape and the cursor keys does in insert
    /// mode. Returns whether it did anything.
    #[allow(clippy::too_many_lines, reason = "one arm per key")]
    fn insert_apply(&mut self, press: KeyPress, host: &mut impl Host) -> bool {
        if let Some(done) = self.insert_aside(press, host) {
            return done;
        }
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
                if host.parted(to..self.cursor) {
                    return false;
                }
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
            (Key::Char(c @ ('b' | 'i')), true) if self.rows_first => {
                return self.insert_set_off(c == 'b', host);
            }
            (Key::Char(c), false) if self.rows_first && !self.replacing => {
                self.insert_as_meant(c, host);
            }
            (Key::Char(c), false) if self.pairs && !self.replacing => self.insert_paired(c, host),
            (Key::Char(c), false) => self.insert_text(&c.to_string(), host),
            (Key::Enter, _) => self.insert_enter(host),
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
                // To the next place to fill in, where there is one.
                if let Some((range, with, within)) = host.next_field(self.cursor) {
                    if !(range.is_empty() && with.is_empty()) {
                        self.edit(host, range.clone(), &with);
                    }
                    self.cursor = (range.start + within).min(host.text().len());
                    return true;
                }
                // Blanks set nothing apart on a page.
                if self.rows_first {
                    return false;
                }
                let column = text.column(self.cursor);
                self.insert_text(&" ".repeat(SHIFT_WIDTH - column % SHIFT_WIDTH), host);
            }
            (Key::Backspace, _) => return self.insert_backspace(host),
            (Key::Delete, _) => {
                // What is behind the cursor: what the host shows there,
                // or else the next character, the end of the line too.
                let shown = if self.rows_first {
                    host.stop(self.cursor, Seek::After)
                } else {
                    None
                };
                // What is shown behind the end of a line is not behind
                // the cursor: it is of the next paragraph, and what
                // lies between (marks that show as nothing) hid that.
                let apart = |shown: &Range<usize>| {
                    let between = text.0.get(self.cursor..shown.start);
                    between.is_some_and(|between| between.contains('\n'))
                };
                if let Some(shown) = shown.as_ref().filter(|shown| apart(shown)) {
                    // At the end of a paragraph: the next one is joined
                    // to it, as a word processor does.
                    return self
                        .join_apart(self.cursor, self.cursor, shown.start, host)
                        .is_some();
                }
                let next = text.next(self.cursor);
                let behind = shown.unwrap_or(self.cursor..next);
                // Not the line break the text ends with.
                if behind.is_empty() || behind.end > text.len() || next >= text.len() {
                    return false;
                }
                // Nor what is in another cell, or out of a footnote.
                if host.parted(self.cursor..behind.start) || host.parted(behind.clone()) {
                    return false;
                }
                if self.delete_pieces(host, behind).is_none() {
                    return false;
                }
            }
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

    /// Keys while typing that are not typed: Ctrl-R and the name of a
    /// register, which puts in what that holds, as typed; and Ctrl-W and
    /// Ctrl-U where the host shows the text as pages (see
    /// [`Engine::shown_erase`]). Whether the key did anything, if it was
    /// one of these.
    fn insert_aside(&mut self, press: KeyPress, host: &mut impl Host) -> Option<bool> {
        if std::mem::take(&mut self.insert_register) {
            let Key::Char(name) = press.key else {
                return Some(false);
            };
            let held = self.registers.get(&name.to_ascii_lowercase());
            let Some(held) = held.map(|register| register.text.clone()) else {
                return Some(false);
            };
            self.insert_text(&held, host);
            return Some(true);
        }
        match (press.key, press.ctrl) {
            (Key::Char('r'), true) => {
                self.insert_register = true;
                Some(true)
            }
            (Key::Char(c @ ('w' | 'u')), true) => {
                let text = Text(host.text());
                if self.cursor == text.line_start(self.cursor) {
                    return None;
                }
                self.shown_erase(c == 'w', host)
            }
            _ => None,
        }
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
            let line_end = text.line_end(self.cursor);
            if typed != "\n" && self.cursor < line_end {
                end = text.next(self.cursor);
            }
            // On a page, of what is shown there: over the marks it is
            // made with (the star that begins strong text), never them;
            // where nothing more is shown in the line, nothing is typed
            // over (the bracket that closes a cell). And not where what
            // is shown would not be read then: a letter against the
            // star of strong text leaves it unclosed.
            if typed != "\n" && self.rows_first {
                let shown = host
                    .stop(self.cursor, Seek::After)
                    .filter(|stop| stop.start < line_end);
                (self.cursor, end) = match shown {
                    Some(stop) => (stop.start, stop.end.max(stop.start)),
                    None => (self.cursor, self.cursor),
                };
                let edit = Edit {
                    range: self.cursor..end,
                    text: typed.to_owned(),
                };
                if host.refuses(std::slice::from_ref(&edit)) {
                    self.failed = true;
                    return;
                }
            }
            let original = host.text().get(self.cursor..end).unwrap_or("");
            let at = self.cursor..self.cursor + typed.len();
            self.replaced
                .push((at, (end > self.cursor).then(|| original.to_owned())));
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
    #[allow(
        clippy::too_many_lines,
        reason = "one case after the other of what stands before the cursor"
    )]
    fn insert_backspace(&mut self, host: &mut impl Host) -> bool {
        if self.cursor == 0 {
            return false;
        }
        if self.rows_first && !self.replacing {
            // An empty place to fill in goes itself.
            if let Some((place, left)) = host.unmake(self.cursor)
                && place.start < place.end
                && place.end <= host.text().len()
            {
                self.edit(host, place.clone(), &left);
                self.cursor = place.start;
                return true;
            }
            // What the host shows before the cursor goes, not the markup
            // that may stand between it and the cursor.
            let before = Text(host.text()).prev(self.cursor);
            // A blank typed at the end of a line shows as nothing there;
            // it is still what goes first.
            let text = Text(host.text());
            let blank = matches!(text.char_at(before), Some(' ' | '\t'))
                && text
                    .0
                    .get(self.cursor..text.line_end(self.cursor))
                    .is_some_and(|rest| rest.trim().is_empty());
            if !blank
                && let Some(stop) = host.stop(before, Seek::Before)
                && stop.end < self.cursor
            {
                let between = host.text().get(stop.end..self.cursor).unwrap_or("");
                if between.contains('\n') {
                    // At the start of a paragraph: it is joined to the
                    // one before, as a word processor does. What that
                    // one shows last is not taken.
                    let Some(gone) = self.join_apart(stop.start, stop.end, self.cursor, host)
                    else {
                        return false;
                    };
                    self.cursor -= gone.len().min(self.cursor);
                    self.insert_start = place(host.text(), self.cursor);
                    return true;
                }
                if host.parted(stop.end..self.cursor) || host.parted(stop) {
                    return false;
                }
            }
            // So does a symbol written with several letters that stood
            // there before typing began: it shows as one thing. What
            // was just typed goes letter by letter, as it came.
            if !blank
                && let Some(stop) = host.stop(before, Seek::Before)
                && (stop.end < self.cursor
                    || (stop.end == self.cursor && self.stood_before_insert(host.text(), &stop)))
            {
                let to = self.stop_at_insert_start(host.text(), stop.start);
                if to == stop.start && stop.is_empty() {
                    // A place with nothing in it yet: into it. The next
                    // Backspace is in it, and takes what that takes.
                    self.cursor = stop.start;
                    return true;
                }
                if to == stop.start {
                    if self.delete_pieces(host, stop.clone()).is_none() {
                        return false;
                    }
                    self.cursor = stop.start;
                    return true;
                }
            }
        }
        // Not the mark that opens a cell or a footnote, from inside it.
        let text = Text(host.text());
        if !self.replacing && host.parted(text.prev(self.cursor)..self.cursor) {
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
                // Both in one: a host may keep one of a pair taken alone
                // (the dollar sign that closes an equation).
                let to = text.next(self.cursor);
                self.edit(host, from..to, "");
                self.cursor = from;
                return true;
            }
        }
        if self.replacing {
            // Back over what was typed, bringing back what it replaced;
            // before that, only back, also onto the end of the line
            // before.
            let mut from = Text(host.text()).prev(self.cursor);
            if let Some((typed, original)) = self.replaced.pop() {
                let original = original.unwrap_or_default();
                // Where it was typed: on a page, marks it went over may
                // stand between it and what was typed before it.
                let typed = if typed.start < self.cursor && typed.end <= self.cursor {
                    typed
                } else {
                    from..self.cursor
                };
                from = typed.start;
                self.edit(host, typed, &original);
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

    /// Delete at the end of a paragraph, Backspace at the start of one,
    /// where the host shows pages: the paragraph byte `at` is in and the
    /// next are joined, where what is shown last of the one ends at
    /// `end`, what is shown first of the other begins at `next`, and
    /// between those and where the two are parted stand only marks that
    /// show as nothing. Lines of one paragraph lose the line break that
    /// parts them. Nothing is joined that would turn the marks of strong
    /// or emphasised text into characters. Returns the bytes that went.
    fn join_apart(
        &mut self,
        at: usize,
        end: usize,
        next: usize,
        host: &mut impl Host,
    ) -> Option<Range<usize>> {
        let text = host.text();
        let between = text.get(end..next)?;
        let quiet = |from: usize, to: usize| {
            // Not out of an equation or a string either.
            let shows_nothing = text.get(from..to).is_some_and(|between| {
                !between.contains(['\n', '$', '"'])
                    && !between.contains("//")
                    && !between.contains("/*")
            });
            from <= to && shows_nothing && !host.parted(from..to)
        };
        let joined = host.join(at, false).and_then(|(range, with)| {
            // Blanks typed behind the end of the paragraph stay: they
            // were typed to part it from what is joined to it.
            let typed = range.start < end && text.get(range.start..end)?.trim().is_empty();
            let start = if typed { end } else { range.start };
            let fits = with.is_empty() && start < range.end;
            (fits && quiet(end, start) && quiet(range.end, next)).then_some(start..range.end)
        });
        let lines = between.trim().is_empty() && between.matches('\n').count() == 1;
        let gone = joined.or_else(|| (lines && !host.parted(end..next)).then_some(end..next))?;
        let left = text[..gone.start].chars().next_back();
        let right = text[gone.end..].chars().next();
        let mark = |c: Option<char>| matches!(c, Some('*' | '_'));
        let letter = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
        if (mark(left) && letter(right)) || (letter(left) && mark(right)) {
            return None;
        }
        self.edit(host, gone.clone(), "");
        Some(gone)
    }

    /// Whether bytes `stop` of `text`, which end at the cursor, are more
    /// than one character and were there before this insert began.
    fn stood_before_insert(&self, text: &str, stop: &Range<usize>) -> bool {
        let (line, column) = self.insert_start;
        let lines = Text(text);
        let several = text
            .get(stop.clone())
            .is_some_and(|shown| shown.chars().count() > 1);
        several
            && lines.line_index(self.cursor) == line
            && stop.end <= lines.line_start(self.cursor) + column
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
        if self.delete_pieces(host, to..self.cursor).is_some() {
            self.cursor = to;
        }
    }

    /// Deletes bytes `range` of the text as the host says it goes (see
    /// [`Host::pieces`]). Returns the text that went, or `None` if
    /// nothing could.
    fn delete_pieces(&mut self, host: &mut impl Host, range: Range<usize>) -> Option<String> {
        let pieces = host.pieces(range, "")?;
        let taken = pieces
            .iter()
            .filter_map(|piece| host.text().get(piece.clone()))
            .collect();
        for piece in pieces.into_iter().rev() {
            self.edit(host, piece, "");
        }
        Some(taken)
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
        self.searches += 1;
        let reading = host.reading();
        let found = self.last_search.as_ref().and_then(|search| {
            find_match(
                host.text(),
                reading.as_deref(),
                from,
                search,
                forward,
                count,
            )
        });
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

    /// `gn` (`forward`) and `gN` behind an operator: it goes over the
    /// match of the last search the cursor is on, or else the next one
    /// (the one before), as in Vim.
    fn search_object(&mut self, forward: bool, host: &mut impl Host) {
        let reading = host.reading();
        let found = self.last_search.as_ref().map(|search| {
            let query = Query {
                pattern: search.pattern.clone(),
                word_start: search.word_start,
                word_end: search.word_end,
            };
            find::find_all(host.text(), reading.as_deref(), &query)
        });
        let found = found.unwrap_or_default();
        let cursor = self.cursor;
        let pick = if forward {
            let next = found.iter().find(|range| range.end > cursor);
            next.or_else(|| found.first())
        } else {
            let before = found.iter().rev().find(|range| range.start <= cursor);
            before.or_else(|| found.last())
        };
        let (Some(range), Some(operator)) = (pick.cloned(), self.pending.operator) else {
            self.reset_pending();
            return self.fail();
        };
        self.searches += 1;
        self.reset_pending_keep_register();
        let object = Object {
            start: range.start,
            end: range.end,
            linewise: false,
        };
        self.operate(operator, object, 1, range.start, host);
    }

    /// `*` and `#`: searches for the word under the cursor, or the next one
    /// on the line; failing that, for what is under the cursor up to the
    /// next blank.
    fn search_word(&mut self, forward: bool, host: &mut impl Host) {
        if self.shown_search_word(forward, host) {
            return;
        }
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
    reading: Option<&Reading>,
    from: usize,
    search: &Search,
    forward: bool,
    count: usize,
) -> Option<usize> {
    let query = Query {
        pattern: search.pattern.clone(),
        word_start: search.word_start,
        word_end: search.word_end,
    };
    let found = find::find_all(text, reading, &query);
    let mut matches: Vec<usize> = found.into_iter().map(|range| range.start).collect();
    matches.dedup();
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
        '\u{e009}' => special(Key::Delete),
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
        Key::Delete => '\u{e009}',
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
