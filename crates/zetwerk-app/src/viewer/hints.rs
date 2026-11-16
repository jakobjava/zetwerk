//! Aids that say what is coming: what can follow a key that waits for
//! another, and the rest of a name or a word being typed.
//!
//! After `g`, `z`, `]`, `[`, an operator, `"` or `i` behind an operator,
//! the keys that can come next are listed faintly above the status line,
//! each with what it does (see `zetwerk_vim::Following`), and with the
//! user's own keys that begin as typed. The list takes no key: the next
//! key does what it always does, and the list goes with it. It comes up
//! only where the keys pause ([`PAUSE`]), so that whoever types on
//! fluently never sees it. That pause is the one timer of the aids: set
//! by a key that leaves another awaited, gone with the next key, so that
//! nothing runs while the user does nothing (see [`Viewer::hints_at`]).
//! `:nextkeys` turns the list off.
//!
//! While a name is typed in an equation, a label behind `@` or a command
//! behind `:`, the rest of the best completion stands faintly behind the
//! cursor, in the line and in the font of the text there: `alp` shows
//! `ha` in grey. Tab takes it. It is the entry of the list of completions
//! that Tab takes in any case (the first, or the one chosen with Ctrl-N),
//! so Tab means one thing. Where such a rest is shown, it stands for the
//! list: the list itself comes up on Ctrl-N, and further Ctrl-N go down
//! it. Elsewhere (behind `#`, where the best entry is no continuation of
//! what is typed) the list comes up as it always did.
//!
//! In running text, where no list is made, a word of three letters or
//! more shows the rest of the nearest word in the text around the cursor
//! that begins so, as Vim's Ctrl-N would find it; Tab takes that too.
//! Nothing is shown where the name or word is whole already, or where the
//! text goes on behind the cursor. `:ghost` turns these rests off. They
//! are overlays only: nothing is in the text until Tab takes it.
//!
//! Neither costs a compile, nor a query of Typst of its own: the rest of
//! a name is read off the list of completions, which is made in any case,
//! and drawing it replaces drawing the list. The rest of a word is looked
//! for once a key, in at most [`WORD_REACH`] bytes before and behind the
//! cursor, however long the document.

use std::fmt::Write;
use std::time::{Duration, Instant};

use typst::utils::hash128;

use super::{PAGE_BAR, Panes, Quad, RowText, STATUS_HEIGHT, Scene, TextureId, Viewer};
use crate::editor;
use crate::keys::{Key, KeyPress};

/// The colours of the interface (as in `chrome`): its ground, its
/// dimmer text and its own text.
const GROUND: &str = "#282828";
const DIM: &str = "#928374";
const BRIGHT: &str = "#ebdbb2";
/// Its font, and the size of the status line's text.
const FONT: &str = "DejaVu Sans Mono";
const STATUS_SIZE: f64 = 10.0;
/// The size the source pane's font is measured at (see
/// `Chrome::cell_width`).
const SOURCE_SIZE: f64 = 10.5;
/// The width of the status line's margin and of the gap behind its mode,
/// in points; and how many characters its mode is long while a command
/// is typed (`COMMAND`).
const STATUS_LEFT: f64 = 8.0 + 10.0;
const COMMAND_MODE: usize = 7;
/// The grey of the rest of a name on the page.
const GHOST: &str = "luma(165)";
/// How much higher a line is than its font is large, about: for the
/// size of the rest of a name where nothing is known of the text there
/// but the height of the cursor.
const LINE_PER_SIZE: f64 = 1.2;

/// The text objects of the host's language, after `i` or `a` (see
/// `typst_text::object`).
const OBJECTS: &[(&str, &str)] = &[
    ("$ e", "an equation"),
    ("h", "a section"),
    ("l", "an item of a list"),
    ("a", "an argument"),
    ("f", "a call"),
    ("* _", "bold, emphasised text"),
];

/// How long the keys have to pause before what can follow them is
/// listed.
const PAUSE: Duration = Duration::from_millis(600);
/// How far before and behind the cursor a word is looked for, in bytes.
const WORD_REACH: usize = 16 * 1024;
/// How many characters of a word have to be typed before its rest is
/// shown, and how many at least that rest is long: less is not worth a
/// key.
const WORD_BEGUN: usize = 3;
const WORD_REST: usize = 2;

/// Whether each of the aids is shown, and where they are.
pub(super) struct Hints {
    next_keys: bool,
    ghost: bool,
    /// The list of what can follow the keys typed.
    list: List,
    /// The rest of what is being typed.
    rest: Ghost,
}

impl Default for Hints {
    fn default() -> Self {
        Self {
            next_keys: true,
            ghost: true,
            list: List::Away,
            rest: Ghost::default(),
        }
    }
}

/// Where the list of what can follow the keys typed is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum List {
    /// Not up, and not to come: no key waits for another.
    Away,
    /// To come up at this time, if the keys pause till then.
    Due(Instant),
    /// Up, until no key waits.
    Up,
}

/// What is known of the rest of what is being typed.
#[derive(Default)]
struct Ghost {
    /// Whether the list of completions was asked for where the rest of
    /// its first entry stands for it.
    listed: bool,
    /// Whether a rest was drawn behind the cursor in the last frame.
    drawn: bool,
    /// The rest of the word being typed, as found after the last key:
    /// with the revision of the text and the cursor it holds for.
    word: Option<(u64, usize, Rest)>,
}

/// The rest of a name that is being typed, and what is known of where:
/// the byte the name begins at, and whether it is in an equation.
#[derive(Debug, Clone)]
struct Rest {
    text: String,
    from: usize,
    in_equation: bool,
}

/// Whether `c` may be part of a name or a label.
fn in_name(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '.' | '-' | ':')
}

/// `text` as a Typst string literal: shown, never read as markup.
fn string(text: &str) -> String {
    let mut literal = String::with_capacity(text.len() + 2);
    literal.push('"');
    for c in text.chars() {
        match c {
            '\\' | '"' => {
                literal.push('\\');
                literal.push(c);
            }
            '\n' => literal.push_str("\\n"),
            _ => literal.push(c),
        }
    }
    literal.push('"');
    literal
}

/// `keys` in Vim's notation, as a mapping is written.
fn notation(keys: &[KeyPress]) -> String {
    let mut written = String::new();
    for press in keys {
        let name = match press.key {
            Key::Char('<') => "lt".to_owned(),
            Key::Char(c) if !press.ctrl && !press.alt => {
                written.push(c);
                continue;
            }
            Key::Char(c) => c.to_string(),
            Key::Enter => "CR".to_owned(),
            Key::Escape => "Esc".to_owned(),
            Key::Backspace => "BS".to_owned(),
            Key::Delete => "Del".to_owned(),
            Key::Tab => "Tab".to_owned(),
            Key::BackTab => "S-Tab".to_owned(),
            Key::ShiftEnter => "S-CR".to_owned(),
            Key::Text(_) => continue,
            other => format!("{other:?}"),
        };
        let ctrl = if press.ctrl { "C-" } else { "" };
        let alt = if press.alt { "A-" } else { "" };
        let _ = write!(written, "<{ctrl}{alt}{name}>");
    }
    written
}

/// The rest of the word nearest to `from` in `text` that begins with
/// `typed`, the word typed at `from`, other than that one: looked for in
/// [`WORD_REACH`] bytes around it.
fn nearest_word<'a>(text: &'a str, from: usize, typed: &str) -> Option<&'a str> {
    let mut low = from.saturating_sub(WORD_REACH);
    while !text.is_char_boundary(low) {
        low -= 1;
    }
    let mut high = (from + typed.len() + WORD_REACH).min(text.len());
    while !text.is_char_boundary(high) {
        high += 1;
    }
    let near = &text[low..high];
    let mut best: Option<(usize, &str)> = None;
    let mut begun = None;
    let ends = std::iter::once((near.len(), ' '));
    for (at, c) in near.char_indices().chain(ends) {
        let in_word = c.is_alphanumeric();
        match begun {
            None if in_word => begun = Some(at),
            Some(start) if !in_word => {
                begun = None;
                // Not a word cut off at the edge of what is looked at.
                let cut = (start == 0 && low > 0) || (at == near.len() && high < text.len());
                let distance = (low + start).abs_diff(from);
                let rest = near[start..at]
                    .strip_prefix(typed)
                    .filter(|rest| rest.chars().count() >= WORD_REST);
                if let Some(rest) = rest.filter(|_| !cut && distance > 0)
                    && best.is_none_or(|(nearest, _)| distance < nearest)
                {
                    best = Some((distance, rest));
                }
            }
            _ => {}
        }
    }
    best.map(|(_, rest)| rest)
}

/// The longest beginning `a` and `b` have in common.
fn common<'a>(a: &'a str, b: &str) -> &'a str {
    let same = a
        .char_indices()
        .zip(b.chars())
        .find(|((_, x), y)| x != y)
        .map_or(a.len().min(b.len()), |((at, _), _)| at);
    &a[..same]
}

/// How wide a character of the list of what can follow is, in points
/// (the advance of its font at 9 points); and the room around its
/// columns and between them.
const LIST_CHARACTER: f64 = 5.42;
const LIST_EDGES: f64 = 16.0;
const LIST_GAPS: f64 = 6.0 + 18.0;

/// How many columns the list of `rows` is set in: as many as keep it low,
/// as far as they fit into `room` points.
fn columns_for(rows: &[(String, String)], room: f64) -> usize {
    let wanted = match rows.len() {
        0..=6 => 1,
        7..=16 => 2,
        _ => 3,
    };
    let width = |columns: usize| {
        let height = rows.len().div_ceil(columns).max(1);
        let widest = |column: &[(String, String)], pick: fn(&(String, String)) -> &String| {
            column
                .iter()
                .map(|row| pick(row).chars().count())
                .max()
                .unwrap_or(0)
        };
        let characters: usize = rows
            .chunks(height)
            .map(|column| widest(column, |row| &row.0) + widest(column, |row| &row.1))
            .sum();
        characters as f64 * LIST_CHARACTER + columns as f64 * LIST_GAPS + LIST_EDGES
    };
    (1..=wanted)
        .rev()
        .find(|columns| width(*columns) <= room)
        .unwrap_or(1)
}

/// Markup for the list of what can follow `typed`: the keys and what
/// they do, in `columns` columns, read down each column.
fn following_markup(typed: &str, rows: &[(String, String)], columns: usize) -> String {
    let columns = columns.max(1);
    let height = rows.len().div_ceil(columns);
    let mut cells = String::new();
    for row in 0..height {
        for column in 0..columns {
            match rows.get(column * height + row) {
                Some((keys, does)) => {
                    let _ = writeln!(
                        cells,
                        "  text(fill: rgb(\"{BRIGHT}\").transparentize(20%), {}), {},",
                        string(keys),
                        string(does)
                    );
                }
                None => cells.push_str("  [], [],\n"),
            }
        }
    }
    let gutters: Vec<&str> = (1..columns * 2)
        .map(|at| if at % 2 == 1 { "6pt" } else { "18pt" })
        .collect();
    format!(
        "#set page(width: auto, height: auto, margin: (x: 8pt, y: 6pt), \
         fill: rgb(\"{GROUND}\").transparentize(12%))\n\
         #set text(font: \"{FONT}\", size: 9pt, fill: rgb(\"{DIM}\"), ligatures: false, \
         hyphenate: false)\n\
         #grid(\n  columns: {count},\n  column-gutter: ({gutters},),\n  row-gutter: 5pt,\n  \
         grid.cell(colspan: {count}, text(fill: rgb(\"{BRIGHT}\").transparentize(20%), {title})),\n\
         {cells})",
        count = columns * 2,
        gutters = gutters.join(", "),
        title = string(&super::help::following_title(typed)),
    )
}

/// Markup for the rest of a name on the page: grey, in `family` (or, in
/// an equation, the font of equations) at `size` points, on nothing.
fn ghost_markup(rest: &str, family: Option<&str>, size: f64, in_equation: bool) -> String {
    let font = family.map_or_else(String::new, |family| format!("font: {}, ", string(family)));
    let body = if in_equation {
        format!("${}$", string(rest))
    } else {
        format!("#{}", string(rest))
    };
    format!(
        "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n\
         #set text({font}size: {size}pt, fill: {GHOST}, top-edge: \"ascender\", \
         bottom-edge: \"descender\")\n{body}"
    )
}

/// Markup for the rest of a command in the status line.
fn command_ghost_markup(rest: &str) -> String {
    format!(
        "#set page(width: auto, height: {STATUS_HEIGHT}pt, margin: 0pt, fill: none)\n\
         #set text(font: \"{FONT}\", size: {STATUS_SIZE}pt, fill: rgb(\"{DIM}\"))\n\
         #set align(horizon)\n#{}",
        string(rest)
    )
}

impl Viewer {
    /// `:nextkeys` and `:ghost`: turns an aid on or off, as `wanted`
    /// says (`on`, `off`), or to what it is not now.
    pub(super) fn following_command(&mut self, name: &str, wanted: &str) {
        let ghost = name == "ghost";
        let now = if ghost {
            &mut self.hints.ghost
        } else {
            &mut self.hints.next_keys
        };
        *now = match wanted {
            "on" => true,
            "off" => false,
            _ => !*now,
        };
        let told = match (ghost, *now) {
            (false, true) => "what can follow a key is shown",
            (false, false) => "what can follow a key is not shown",
            (true, true) => "the rest of a name or word shows behind the cursor; Tab takes it",
            (true, false) => "no rest of a name or word behind the cursor: the list comes up",
        };
        self.notice(told);
    }

    /// A key for the aids, before the list of completions has it:
    /// `Some` with whether it was taken, if it is for them.
    ///
    /// Tab on the command line takes the rest of the command. While
    /// typing, Ctrl-N (and Ctrl-P) bring up the list that the rest of
    /// its first entry stood for; the arrow keys, for which no list is
    /// up, are for the text.
    pub(super) fn hint_key(&mut self, press: KeyPress) -> Option<bool> {
        if self.completion.is_none() {
            self.hints.rest.listed = false;
        }
        let mode = self.editor.as_ref()?.mode();
        if mode == editor::Mode::Command && press.key == Key::Tab && !press.ctrl {
            let rest = self.command_rest()?;
            for c in rest.chars() {
                self.editor_key(KeyPress::plain(Key::Char(c)));
            }
            return Some(true);
        }
        if mode != editor::Mode::Insert {
            return None;
        }
        if press.key == Key::Tab
            && !press.ctrl
            && let Some(rest) = self.word_rest()
        {
            let editor = self.editor.as_mut()?;
            let within = rest.text.len();
            let press = editor.completed(0, rest.text, within);
            self.editor_key(press);
            return Some(true);
        }
        let asks = press.ctrl && matches!(press.key, Key::Char('n' | 'p' | ' '));
        let arrow = !press.ctrl && matches!(press.key, Key::Up | Key::Down);
        if !(asks || arrow) {
            return None;
        }
        if self.completion.is_none() {
            // Asked for where there was nothing: whatever comes up is
            // listed.
            self.hints.rest.listed = asks;
            return None;
        }
        if self.lists_completion() {
            return None;
        }
        if arrow {
            self.completion = None;
            return Some(false);
        }
        self.hints.rest.listed = true;
        Some(true)
    }

    /// Whether the list of completions, where there is one, is shown:
    /// not where the rest of its first entry stands for it, as drawn
    /// last. Where the rest cannot be drawn (no cursor is shown on the
    /// page, as while a reference is typed that does not hold yet), the
    /// list is.
    pub(super) fn lists_completion(&self) -> bool {
        self.hints.rest.listed || !self.hints.rest.drawn || self.listed_rest().is_none()
    }

    /// The rest of the name or word being typed, where it is to be shown
    /// behind the cursor.
    fn name_rest(&self) -> Option<Rest> {
        self.listed_rest().or_else(|| self.word_rest())
    }

    /// The rest of the name being typed that the chosen completion would
    /// put in, where it is to be shown behind the cursor.
    fn listed_rest(&self) -> Option<Rest> {
        if !self.hints.ghost {
            return None;
        }
        let editor = self.editor.as_ref()?;
        if editor.mode() != editor::Mode::Insert {
            return None;
        }
        let completion = self.completion.as_ref()?;
        let entry = completion.candidates.get(completion.chosen)?;
        if entry.insert.is_some() {
            return None;
        }
        let (text, cursor) = (editor.text(), editor.cursor());
        let typed = text
            .get(entry.from..cursor)
            .filter(|typed| !typed.is_empty())?;
        // Not where the name goes on behind the cursor already.
        if text[cursor..].chars().next().is_some_and(in_name) {
            return None;
        }
        let rest = entry.candidate.text.strip_prefix(typed)?;
        // Of what would be put in, the name: what stands around a call
        // is no part of the line yet.
        let rest = &rest[..rest.find(|c: char| !in_name(c)).unwrap_or(rest.len())];
        let in_equation = crate::typst_text::equation_at(editor.source(), cursor).is_some();
        let reference = text[..entry.from].ends_with('@');
        (!rest.is_empty() && (in_equation || reference)).then(|| Rest {
            text: rest.to_owned(),
            from: entry.from,
            in_equation,
        })
    }

    /// Whether the rest of a word shows behind the cursor, for Tab to
    /// take.
    pub(super) fn shows_word_rest(&self) -> bool {
        self.word_rest().is_some()
    }

    /// The rest of the word being typed in running text, as found after
    /// the last key, while it holds.
    fn word_rest(&self) -> Option<Rest> {
        let (revision, cursor, rest) = self.hints.rest.word.as_ref()?;
        let editor = self.editor.as_ref()?;
        let holds = self.hints.ghost
            && self.completion.is_none()
            && editor.mode() == editor::Mode::Insert
            && editor.revision() == *revision
            && editor.cursor() == *cursor;
        holds.then(|| rest.clone())
    }

    /// Looks for the rest of the word being typed in running text: where
    /// no list of completions is up, three letters or more are typed, and
    /// the text does not go on behind the cursor.
    fn find_word_rest(&self) -> Option<(u64, usize, Rest)> {
        if !self.hints.ghost || self.completion.is_some() {
            return None;
        }
        let editor = self.editor.as_ref()?;
        if editor.mode() != editor::Mode::Insert {
            return None;
        }
        let (text, cursor) = (editor.text(), editor.cursor());
        if text[cursor..].chars().next().is_some_and(in_name) {
            return None;
        }
        let from = text[..cursor].trim_end_matches(char::is_alphanumeric).len();
        let typed = &text[from..cursor];
        // Names of code, labels and what is escaped have lists of their
        // own, and so have equations.
        let named = text[..from].ends_with(['#', '@', '<', '\\', '_', '.']);
        if named
            || typed.chars().count() < WORD_BEGUN
            || crate::typst_text::is_in_equation(editor.source(), from.saturating_sub(1))
        {
            return None;
        }
        let rest = nearest_word(text, from, typed)?;
        let rest = Rest {
            text: rest.to_owned(),
            from,
            in_equation: false,
        };
        Some((editor.revision(), cursor, rest))
    }

    /// After every key: looks for the rest of a word, and sets or ends
    /// the pause after which what can follow the keys is listed.
    pub(super) fn hints_keyed(&mut self) {
        self.hints.rest.word = self.find_word_rest();
        if !self.waits_for_key() {
            self.hints.list = List::Away;
        } else if self.hints.list != List::Up {
            self.hints.list = List::Due(Instant::now() + PAUSE);
        }
    }

    /// When the list of what can follow the keys typed is to come up,
    /// while they wait for another: the one time the aids want the
    /// window woken up at.
    #[must_use]
    pub fn hints_at(&self) -> Option<Instant> {
        match self.hints.list {
            List::Due(at) => Some(at),
            List::Away | List::Up => None,
        }
    }

    /// Puts up the list of what can follow the keys typed, where they
    /// have paused long enough by `now`. Returns whether it came up, so
    /// that the window is drawn again.
    pub fn hints_due(&mut self, now: Instant) -> bool {
        if self.hints_at().is_none_or(|due| due > now) {
            return false;
        }
        let up = self.waits_for_key();
        self.hints.list = if up { List::Up } else { List::Away };
        up
    }

    /// The keys typed, where the list of what can follow them is up:
    /// what `?` gives help on then (see `help`).
    pub(super) fn following_listed(&self) -> Option<String> {
        if self.hints.list != List::Up || !self.typed.is_empty() {
            return None;
        }
        self.editor
            .as_ref()?
            .following()
            .map(|following| following.typed)
    }

    /// Whether the keys typed wait for another, and what can follow them
    /// is to be listed.
    fn waits_for_key(&self) -> bool {
        let Some(editor) = self.editor.as_ref().filter(|_| self.hints.next_keys) else {
            return false;
        };
        let mode = editor.mode();
        let commands = matches!(
            mode,
            editor::Mode::Normal | editor::Mode::Visual | editor::Mode::VisualLine
        );
        commands && (!self.typed.is_empty() || editor.following().is_some())
    }

    /// The rest of the command being typed behind `:`: what all the
    /// commands that begin as typed go on with alike.
    fn command_rest(&self) -> Option<String> {
        if !self.hints.ghost {
            return None;
        }
        let editor = self.editor.as_ref()?;
        let typed = editor
            .command_line()
            .filter(|_| editor.prompt() == Some(':'))?;
        if typed.trim().is_empty() {
            return None;
        }
        let mut shared: Option<&str> = None;
        for (keys, command) in super::palette::entries() {
            let Some(command) = command.filter(|_| keys.starts_with(':')) else {
                continue;
            };
            if command.starts_with(typed) {
                shared = Some(shared.map_or(command, |shared| common(shared, command)));
            }
        }
        let rest = shared?.get(typed.len()..)?;
        (!rest.trim().is_empty()).then(|| rest.to_owned())
    }

    /// Draws the aids that are due: over the page and the status line,
    /// under the lists.
    pub(super) fn hints(&mut self, scene: &mut Scene, has: &dyn Fn(TextureId) -> bool) {
        if let Some(markup) = self.following_now() {
            let margin = super::PANEL_MARGIN * self.scale;
            let bottom = f64::from(self.window.1) - f64::from(self.status_height()) - margin;
            self.panel(scene, has, &markup, |(_, height)| (margin, bottom - height));
        }
        self.hints.rest.drawn = false;
        if let Some(rest) = self.name_rest() {
            self.hints.rest.drawn = self.rest_on_page(scene, has, &rest);
            if self.shown_panes() != Panes::Page
                && let Some((x, top, _)) = self.caret_at
            {
                let row = RowText {
                    text: rest.text,
                    dim: true,
                };
                super::row_images(&mut self.chrome, scene, has, &[(row, (x, top))], self.scale);
                self.hints.rest.drawn = true;
            }
        }
        if let Some(rest) = self.command_rest() {
            self.rest_of_command(scene, has, &rest);
        }
    }

    /// Markup for what can follow the keys typed so far, where they wait
    /// for another.
    fn following_now(&self) -> Option<String> {
        if !self.hints.next_keys || self.hints.list != List::Up {
            return None;
        }
        let editor = self.editor.as_ref()?;
        let room = f64::from(self.window.0) / self.scale - 2.0 * super::PANEL_MARGIN;
        if !matches!(
            editor.mode(),
            editor::Mode::Normal | editor::Mode::Visual | editor::Mode::VisualLine
        ) {
            return None;
        }
        // Keys that may yet become one of the user's own.
        if !self.typed.is_empty() {
            let typed = self.typed.as_slice();
            let rows: Vec<(String, String)> = self
                .keymap
                .begun(typed)
                .map(|(keys, stands_for)| {
                    let own = format!("yours: {}", notation(stands_for));
                    (notation(&keys[typed.len()..]), own)
                })
                .collect();
            let columns = columns_for(&rows, room);
            return Some(following_markup(&notation(typed), &rows, columns));
        }
        let following = editor.following()?;
        let mut rows: Vec<(String, String)> = following
            .next
            .into_iter()
            .map(|(keys, does)| (keys, does.to_owned()))
            .collect();
        if following.objects {
            let own = OBJECTS.iter();
            rows.extend(own.map(|(keys, does)| ((*keys).to_owned(), (*does).to_owned())));
        }
        let columns = columns_for(&rows, room);
        Some(following_markup(&following.typed, &rows, columns))
    }

    /// Draws `rest` behind the cursor on the page, in the font the text
    /// there is set in, where the cursor is drawn. Returns whether it
    /// was: not where no cursor is drawn on the page.
    fn rest_on_page(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        rest: &Rest,
    ) -> bool {
        let bar_id = TextureId(hash128(&("tint", PAGE_BAR)));
        let Some(bar) = scene.quads.iter().rev().find(|quad| quad.texture == bar_id) else {
            return false;
        };
        let bar = *bar;
        let zoom = self.view.zoom;
        let (family, size) = self.style_at(&bar, rest).unwrap_or_else(|| {
            let size = f64::from(bar.height) / zoom / LINE_PER_SIZE;
            (None, size)
        });
        // In an equation, the font of equations; elsewhere the text's.
        let family = family.filter(|_| !rest.in_equation);
        let markup = ghost_markup(&rest.text, family.as_deref(), size, rest.in_equation);
        let Some(image) = self.chrome.render(&markup, zoom) else {
            return false;
        };
        let (width, height) = (image.pixmap.width() as f32, image.pixmap.height() as f32);
        scene.quads.push(Quad {
            texture: image.id,
            x: (bar.x + bar.width).round(),
            y: (bar.y + (bar.height - height) / 2.0).round(),
            width,
            height,
        });
        if !has(image.id) {
            scene.images.push(image);
        }
        true
    }

    /// The font family and size of the text at the start of what `rest`
    /// is the rest of (of the equation it is in), on the page the cursor
    /// `bar` is drawn on.
    fn style_at(&self, bar: &Quad, rest: &Rest) -> Option<(Option<String>, f64)> {
        let change = self.places_of_text()?;
        let editor = self.editor.as_ref()?;
        let (_, left, _) = self.split();
        let (x, y) = (f64::from(bar.x), f64::from(bar.y));
        let index = self
            .view
            .visible(&self.layout)
            .into_iter()
            .find_map(|page| {
                let size = self.layout.pages.get(page.index)?;
                let (left, top) = (f64::from(left) + page.x as f64, page.y as f64);
                let inside = (left..left + f64::from(size.width)).contains(&x)
                    && (top..top + f64::from(size.height)).contains(&y);
                inside.then_some(page.index)
            })?;
        let map = self.maps.get(&index)?;
        let start = match crate::typst_text::equation_at(editor.source(), editor.cursor()) {
            Some(equation) if rest.in_equation => equation.start,
            _ => rest.from.saturating_sub(1),
        };
        let (file, at) = (editor.file(), change.back_or_edge(start));
        let caret = map.caret(file, at).or_else(|| map.caret_before(file, at))?;
        let run = map.runs().get(caret.run)?;
        Some((Some(run.family.clone()), run.size))
    }

    /// Draws `rest` behind the command being typed in the status line.
    fn rest_of_command(&mut self, scene: &mut Scene, has: &dyn Fn(TextureId) -> bool, rest: &str) {
        let Some(typed) = self
            .editor
            .as_ref()
            .and_then(editor::Editor::command_line)
            .map(|typed| typed.chars().count())
        else {
            return;
        };
        let scale = self.scale;
        let cell = self.chrome.cell_width(scale) * STATUS_SIZE / SOURCE_SIZE;
        // Behind the mode, the prompt, what is typed and the cursor.
        let x = STATUS_LEFT * scale + (COMMAND_MODE + 1 + typed + 1) as f64 * cell;
        let y = f64::from(self.window.1) - f64::from(self.status_height());
        let markup = command_ghost_markup(rest);
        self.panel(scene, has, &markup, |_| (x, y));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use std::time::Instant;

    use super::super::tests::{compile, editing, press, tints};
    use super::*;
    use crate::chrome::Chrome;

    /// Whether the scene of `viewer` draws the image `markup` makes.
    fn draws(viewer: &mut Viewer, markup: &str) -> bool {
        let id = Chrome::id(markup, viewer.scale);
        viewer
            .scene(&|_| false)
            .quads
            .iter()
            .any(|quad| quad.texture == id)
    }

    /// Lets the keys typed pause long enough for what can follow them to
    /// come up. Returns whether it came up.
    fn pause(viewer: &mut Viewer) -> bool {
        viewer.hints_due(Instant::now() + PAUSE)
    }

    #[test]
    fn what_can_follow_comes_up_only_when_the_keys_pause() {
        let (mut viewer, directory) = editing("Some text here.\n\nMore text.\n");
        press(&mut viewer, "gsgs");
        // Nothing waits: no time to wake up at.
        assert_eq!(viewer.hints_at(), None);
        assert!(!pause(&mut viewer));
        // Typed on fluently, the list is never drawn.
        press(&mut viewer, "g");
        let due = viewer.hints_at();
        assert!(due.is_some_and(|due| due > Instant::now()));
        assert!(!viewer.hints_due(Instant::now()));
        assert_eq!(viewer.following_now(), None);
        press(&mut viewer, "g");
        assert_eq!(viewer.hints_at(), None);
        // A pause puts it up, and asks for the window to be drawn.
        press(&mut viewer, "z");
        assert!(pause(&mut viewer));
        assert_eq!(viewer.hints_at(), None);
        assert!(viewer.following_now().is_some());
        // Escape ends the waiting, and with it the list and the timer.
        press(&mut viewer, "<Esc>");
        assert_eq!((viewer.hints_at(), viewer.following_now()), (None, None));
        // In insert mode nothing waits.
        press(&mut viewer, "i");
        assert_eq!(viewer.hints_at(), None);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn what_can_follow_a_key_is_shown_until_the_next_key() {
        let (mut viewer, directory) = editing("Some text here.\n\nMore text.\n");
        press(&mut viewer, "gsgs");
        assert_eq!(viewer.following_now(), None);
        press(&mut viewer, "g");
        pause(&mut viewer);
        let markup = viewer.following_now().unwrap_or_default();
        assert!(markup.contains("jump to a label in view"), "{markup}");
        assert!(markup.contains("go to a heading"), "{markup}");
        assert!(draws(&mut viewer, &markup));
        // The next key does what it does, and the list goes.
        press(&mut viewer, "g");
        assert_eq!(viewer.following_now(), None);
        // Behind an operator: the operator doubled, motions, and behind
        // `i` the objects of Typst too.
        press(&mut viewer, "d");
        pause(&mut viewer);
        let markup = viewer.following_now().unwrap_or_default();
        assert!(markup.contains("\"d …\"") && markup.contains("lines: this one"));
        // Once up, it follows the keys that wait on at once.
        press(&mut viewer, "i");
        assert_eq!(viewer.hints_at(), None);
        let markup = viewer.following_now().unwrap_or_default();
        assert!(markup.contains("\"di …\"") && markup.contains("an equation"));
        press(&mut viewer, "<Esc>z");
        pause(&mut viewer);
        assert!(
            viewer
                .following_now()
                .unwrap_or_default()
                .contains("spelling")
        );
        press(&mut viewer, "<Esc>");
        // Turned off, it is not shown, and no time is set for it.
        press(&mut viewer, ":nextkeys<CR>g");
        assert_eq!(viewer.hints_at(), None);
        assert!(!pause(&mut viewer));
        assert_eq!(viewer.following_now(), None);
        press(&mut viewer, "<Esc>:nextkeys on<CR>");
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_users_own_keys_are_among_what_can_follow() {
        let (mut viewer, directory) = editing("Some text.\n");
        let (keymap, _) = super::super::keymap::Keymap::parse([(",w", ":w<CR>")], []);
        viewer.set_keymap(keymap);
        press(&mut viewer, ",");
        assert!(pause(&mut viewer));
        let markup = viewer.following_now().unwrap_or_default();
        assert!(
            markup.contains("\"w\"") && markup.contains("yours: :w<CR>"),
            "{markup}"
        );
        press(&mut viewer, "x");
        assert_eq!(viewer.following_now(), None);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_rest_of_a_name_stands_behind_the_cursor_and_tab_takes_it() {
        let (mut viewer, directory) = editing("Some text here.\n\n");
        press(&mut viewer, "gsgs");
        let rest = |viewer: &Viewer| viewer.name_rest().map(|rest| rest.text);
        press(&mut viewer, "Go$alp");
        assert_eq!(rest(&viewer).as_deref(), Some("ha"));
        // It stands for the list, which is not shown, and agrees with it.
        let first = viewer
            .completion
            .as_ref()
            .map(|list| list.candidates[0].candidate.text.clone());
        assert_eq!(first.as_deref(), Some("alpha"));
        // On the page, behind the cursor.
        let bar = tints(&mut viewer, PAGE_BAR);
        assert_eq!(bar.len(), 1);
        let behind = (bar[0].x + bar[0].width).round();
        let quads = viewer.scene(&|_| false).quads;
        assert!(
            quads.iter().any(|quad| (quad.x - behind).abs() < 0.5),
            "{bar:?} {quads:?}"
        );
        assert!(!viewer.lists_completion());
        // Ctrl-N brings up the list, the first entry still chosen.
        press(&mut viewer, "<C-n>");
        assert!(viewer.lists_completion());
        assert_eq!(viewer.completion.as_ref().map(|list| list.chosen), Some(0));
        assert_eq!(rest(&viewer).as_deref(), Some("ha"));
        press(&mut viewer, "<Tab>");
        assert!(viewer.text().unwrap().contains("$alpha"));
        // Whole, it has no rest.
        assert_eq!(rest(&viewer), None);
        press(&mut viewer, "<Esc>");
        compile(&mut viewer);
        // In running text, nothing; behind `@`, the rest of a label.
        press(&mut viewer, "Goplain wor");
        assert_eq!(rest(&viewer), None);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_rest_of_a_label_and_of_a_command() {
        let text = "#set heading(numbering: \"1.\")\n= Start <start>\nText.\n\n";
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "gsgs");
        press(&mut viewer, "Go@st");
        assert_eq!(
            viewer.name_rest().map(|rest| rest.text).as_deref(),
            Some("art")
        );
        // Either the rest is drawn, or (where the page shows no cursor
        // behind a reference that does not hold yet) the list is.
        let _ = viewer.scene(&|_| false);
        assert_ne!(viewer.hints.rest.drawn, viewer.lists_completion());
        viewer.hints.rest.drawn = false;
        assert!(viewer.lists_completion());
        press(&mut viewer, "<Tab><Esc>");
        assert!(viewer.text().unwrap().contains("@start"));
        // Behind `:`, what all commands that begin so go on with.
        press(&mut viewer, ":gram");
        assert_eq!(viewer.command_rest().as_deref(), Some("mar"));
        assert!(draws(&mut viewer, &command_ghost_markup("mar")));
        press(&mut viewer, "<BS><BS><BS><BS>sec");
        assert_eq!(viewer.command_rest().as_deref(), Some("tion "));
        press(&mut viewer, "<Tab>");
        assert_eq!(
            viewer
                .editor
                .as_ref()
                .and_then(|editor| editor.command_line()),
            Some("section ")
        );
        press(&mut viewer, "<Esc>");
        // Turned off: no rest, and the list comes up.
        press(&mut viewer, ":ghost<CR>Go@st");
        assert_eq!(viewer.name_rest().map(|rest| rest.text), None);
        assert!(viewer.lists_completion());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_rest_of_a_word_in_running_text() {
        let text = "The integral of a function.\n\nMore text.\n";
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "gsgs");
        let rest = |viewer: &Viewer| viewer.name_rest().map(|rest| rest.text);
        let line = |viewer: &Viewer, line: &str| {
            let text = viewer.text().unwrap_or_default();
            text.lines().any(|there| there.trim_end() == line)
        };
        press(&mut viewer, "Goin");
        // Two letters are not enough.
        assert_eq!(rest(&viewer), None);
        press(&mut viewer, "t");
        assert_eq!(rest(&viewer).as_deref(), Some("egral"));
        // Only shown: the text has nothing of it yet, and no list is up.
        assert!(line(&viewer, "int"));
        assert!(viewer.completion.is_none());
        // On the page, behind the cursor.
        let bar = tints(&mut viewer, PAGE_BAR);
        assert_eq!(bar.len(), 1);
        let behind = (bar[0].x + bar[0].width).round();
        let quads = viewer.scene(&|_| false).quads;
        assert!(
            quads.iter().any(|quad| (quad.x - behind).abs() < 0.5),
            "{bar:?} {quads:?}"
        );
        // Typed on where it no longer fits, it goes.
        press(&mut viewer, "o");
        assert_eq!(rest(&viewer), None);
        press(&mut viewer, "<BS>");
        assert_eq!(rest(&viewer).as_deref(), Some("egral"));
        // Tab takes it, as part of what is typed.
        press(&mut viewer, "<Tab>");
        assert!(line(&viewer, "integral"));
        assert_eq!(rest(&viewer), None);
        press(&mut viewer, "<Esc>u");
        assert!(!line(&viewer, "integral"));
        // Not where the text goes on behind the cursor, nor behind `#`.
        press(&mut viewer, "ggifun");
        assert_eq!(rest(&viewer), None);
        press(&mut viewer, "<Esc>u");
        press(&mut viewer, "Go#fun");
        assert_eq!(
            viewer
                .hints
                .rest
                .word
                .as_ref()
                .map(|(_, _, rest)| rest.text.as_str()),
            None
        );
        press(&mut viewer, "<Esc>");
        // Moved away from, it holds no more; turned off, nothing.
        press(&mut viewer, "Gofun");
        assert_eq!(rest(&viewer).as_deref(), Some("ction"));
        press(&mut viewer, "<Esc>:ghost<CR>Gofun");
        assert_eq!(rest(&viewer), None);
        press(&mut viewer, "<Tab>");
        assert!(!line(&viewer, "function"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn small_parts() {
        let text = "alpha alphabet beta alphanumeric alp";
        let at = text.len() - 3;
        assert_eq!(nearest_word(text, at, "alp"), Some("hanumeric"));
        assert_eq!(nearest_word(text, 0, "alp"), Some("habet"));
        assert_eq!(nearest_word(text, at, "bet"), None);
        assert_eq!(nearest_word("über übermut", 0, "übe"), Some("rmut"));
        assert_eq!(common("section up", "section down"), "section ");
        assert_eq!(common("pdf", "pdf "), "pdf");
        assert_eq!(common("ab", "x"), "");
        let rows = vec![("k".to_owned(), "x".repeat(40)); 20];
        assert_eq!(columns_for(&rows, 1000.0), 3);
        assert_eq!(columns_for(&rows, 600.0), 2);
        assert_eq!(columns_for(&rows, 100.0), 1);
        let keys = crate::keys::parse("<C-s>a<lt><CR>").unwrap();
        assert_eq!(notation(&keys), "<C-s>a<lt><CR>");
        assert_eq!(string("a\"b\\"), "\"a\\\"b\\\\\"");
    }
}
