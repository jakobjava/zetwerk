//! Typing what a keyboard has no key for.
//!
//! - **Input methods** (Chinese, Japanese, Korean, emoji pickers, some
//!   dictation tools; on Wayland any tool that types through the
//!   compositor's text input): what the input method is composing (the
//!   pre-edit) is shown at the cursor and is nowhere else, not in the
//!   text, its history or the journal. What it commits is typed, as keys
//!   are ([`Viewer::text_typed`]). The window lets an input method work
//!   only where text is taken (insert mode, the command line, lists that
//!   are typed into, after `f`, `t` and `r`): in normal mode keys are
//!   commands, and `j` moves whatever the input method would make of it.
//! - **Digraphs** (Ctrl-K and two characters, Vim's table, see
//!   [`digraphs`]) and **characters by number** (Ctrl-V and a decimal
//!   number, or `u`, `U`, `x`, `o` and one in hex or octal), wherever text
//!   is taken.
//! - **The character picker** (`:chars`, `:char` and words, `:digraphs`;
//!   Ctrl-K Ctrl-K while typing), and `:char` alone for what the
//!   character under the cursor is (Vim's `ga`, which is taken here).
//!
//! The pre-edit is laid over the page at the cursor, set like the text it
//! stands in (the font, size, weight and slant of the run there),
//! underlined, the segment the input method is converting underlined
//! more strongly and its cursor drawn as the page's. It hides what
//! follows it while it is there: the line is not set anew for it (that
//! comes when it is committed). On the command line and in a list it is
//! shown in the line typed into.

mod chars;
mod digraphs;

use crate::chrome::typst_string as string;
use std::fmt::Write;

use typst::syntax::FileId;
use zetwerk_typst::PageMap;

use super::chrome::{self, InPlace};
use super::{Mode, PAGE_BAR, Panes, Quad, Scene, TextureId, Viewer, tint};
use crate::editor;
use crate::keys::{Key, KeyPress};

/// How the window takes typing, as the user wants it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Typing {
    /// Whether an input method may work in normal mode too, where what it
    /// commits is taken as keys.
    pub input_method_in_normal: bool,
    /// Whether, in normal mode, a key that types a letter of another
    /// script than the Latin (on a Russian or Greek layout) is taken as
    /// the key in its place on a US keyboard.
    pub by_position: bool,
}

impl Default for Typing {
    fn default() -> Self {
        Self {
            input_method_in_normal: false,
            by_position: true,
        }
    }
}

/// What is being composed, and what the input method is to be told.
#[derive(Default)]
pub(super) struct Compose {
    /// What an input method shows while it composes.
    preedit: Option<Preedit>,
    /// A digraph or a number begun with Ctrl-K or Ctrl-V.
    pending: Option<Pending>,
    /// While the keys of a text typed at once go in: the pages are worked
    /// out once, for the last of them.
    pub(super) burst: bool,
    /// Whether the input method is to drop what it composes.
    reset: bool,
}

/// The pre-edit of an input method.
struct Preedit {
    text: String,
    /// Where in it the cursor is, or which part of it is being converted:
    /// bytes of `text`.
    cursor: Option<(usize, usize)>,
    /// Where it was begun; somewhere else, it is gone.
    at: Place,
}

/// Where text is being typed: the file and the cursor, and what into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Place {
    file: Option<FileId>,
    cursor: usize,
    mode: Option<editor::Mode>,
    command: bool,
    picking: bool,
}

/// What Ctrl-K or Ctrl-V has begun.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pending {
    /// Ctrl-K, and the first character if it has come.
    Digraph(Option<char>),
    /// Ctrl-V, before it is known what follows.
    Literal,
    /// Ctrl-V and a number: its base, how many digits it has at most,
    /// what begun it (`u`, `x`, ...; nothing for a decimal one) and the
    /// digits so far.
    Number {
        radix: u32,
        most: usize,
        letter: Option<char>,
        digits: String,
    },
}

impl Pending {
    /// What stands at the cursor while it waits, as Vim shows it.
    fn shown(&self) -> String {
        match self {
            Self::Digraph(None) => "?".to_owned(),
            Self::Digraph(Some(first)) => first.to_string(),
            Self::Literal => "^".to_owned(),
            Self::Number { letter, digits, .. } => {
                format!("^{}{digits}", letter.map(String::from).unwrap_or_default())
            }
        }
    }
}

/// The source pane's font and colours, for what is composed there.
/// (The source pane is set in the window's font, see [`crate::chrome::UiFont`].)
const SOURCE_SIZE: f64 = 10.5;
use crate::chrome::BACKGROUND as SOURCE_PAPER;
use crate::chrome::FOREGROUND as SOURCE_INK;

impl Viewer {
    /// Text came that was typed otherwise than by one key: committed by
    /// an input method, or several characters from one press. It is
    /// typed as its characters would be, one key each, so that it is part
    /// of the typing around it (one undo step, what `.` repeats); a line
    /// break is Enter, a tab Tab. This is the one way text that is not a
    /// key comes in.
    pub fn text_typed(&mut self, text: &str) {
        self.compose.preedit = None;
        // "\r\n" is one line break.
        let keys: Vec<Key> = text
            .replace("\r\n", "\n")
            .chars()
            .filter_map(|c| match c {
                '\n' | '\r' => Some(Key::Enter),
                '\t' => Some(Key::Tab),
                c if c.is_control() => None,
                c => Some(Key::Char(c)),
            })
            .collect();
        let last = keys.len().saturating_sub(1);
        for (index, key) in keys.into_iter().enumerate() {
            self.compose.burst = index < last && self.is_typing();
            self.key(KeyPress::plain(key));
        }
        self.compose.burst = false;
    }

    /// The input method shows `text` as what it composes, with its cursor
    /// (or the part being converted) at `cursor`; empty, it composes
    /// nothing any more.
    pub fn preedit(&mut self, text: &str, cursor: Option<(usize, usize)>) {
        if text.is_empty() {
            self.compose.preedit = None;
            return;
        }
        let fits = |(start, end): (usize, usize)| {
            start <= end
                && end <= text.len()
                && text.is_char_boundary(start)
                && text.is_char_boundary(end)
        };
        self.compose.preedit = Some(Preedit {
            text: text.to_owned(),
            cursor: cursor.filter(|cursor| fits(*cursor)),
            at: self.typing_place(),
        });
    }

    /// The input method has been turned on or off. Off, it composes
    /// nothing.
    pub fn input_method(&mut self, on: bool) {
        if !on {
            self.compose.preedit = None;
        }
    }

    /// Whether an input method is composing something that is shown.
    #[must_use]
    pub fn composing(&self) -> bool {
        self.live_preedit().is_some()
    }

    /// What is composed is given up: Escape while composing, a click, the
    /// window put aside. The input method is told to drop it too, if
    /// `reset`.
    pub fn end_composition(&mut self, reset: bool) {
        if self.compose.preedit.take().is_some() && reset {
            self.compose.reset = true;
        }
    }

    /// Whether the input method is to be made to drop what it composes,
    /// once: what it showed is gone here (the cursor moved, the mode
    /// changed, Escape).
    pub fn take_input_method_reset(&mut self) -> bool {
        if self
            .compose
            .preedit
            .as_ref()
            .is_some_and(|preedit| preedit.at != self.typing_place())
        {
            self.compose.preedit = None;
            self.compose.reset = true;
        }
        std::mem::take(&mut self.compose.reset)
    }

    /// Whether keys are typing text now (and not commands): in insert
    /// mode, on a command line, in a list typed into, and for the
    /// character that `f`, `t` or `r` waits for.
    #[must_use]
    pub fn takes_text(&self) -> bool {
        if self.picker.is_some() || matches!(self.mode, Mode::Command(_)) {
            return true;
        }
        if self.is_grid() || self.is_jumping() {
            return false;
        }
        self.editor.as_ref().is_some_and(|editor| {
            matches!(editor.mode(), editor::Mode::Insert | editor::Mode::Command)
                || editor.engine().awaits_char()
        })
    }

    /// Whether an input method is to be let work now: where text is
    /// typed, or everywhere if the settings say so.
    #[must_use]
    pub fn wants_input_method(&self) -> bool {
        self.takes_text() || (self.keymap.typing.input_method_in_normal && self.editor.is_some())
    }

    /// Whether a key pressed is to count as the key in its place on a US
    /// keyboard, where it types a letter of another script (see
    /// [`Typing::by_position`]): with Ctrl or Alt always, a command being
    /// nothing else; otherwise where keys are commands and the user has
    /// no mapping that begins with it.
    #[must_use]
    pub fn by_position(&self, press: KeyPress) -> bool {
        if !self.keymap.typing.by_position {
            return false;
        }
        if press.ctrl || press.alt {
            return true;
        }
        let mut typed = self.typed.clone();
        typed.push(press);
        !self.takes_text() && self.keymap.begun(&typed).next().is_none()
    }

    /// Where on the window the input method's list of candidates belongs:
    /// at the cursor, as `(x, y, width, height)` in device pixels. Known
    /// once a frame has been drawn.
    #[must_use]
    pub fn input_area(&self) -> Option<(f64, f64, f64, f64)> {
        let (width, height) = (f64::from(self.window.0), f64::from(self.window.1));
        let status = f64::from(self.status_height());
        let commanding = matches!(self.mode, Mode::Command(_))
            || self
                .editor
                .as_ref()
                .is_some_and(|editor| editor.mode() == editor::Mode::Command);
        if self.picker.is_some() {
            // The line typed into stands at the top of the list, a third
            // of the way down.
            return Some(((width / 3.0).round(), (height / 4.0).round(), 1.0, status));
        }
        if commanding {
            // Behind what is typed on the status line; its characters
            // are about six tenths of the line high.
            let typed = self.command_typed().chars().count() as f64;
            let x = (8.0 + 70.0 + typed * 6.0) * self.scale;
            return Some((x.min(width - 1.0), height - status, 1.0, status));
        }
        let at = match self.shown_panes() {
            Panes::Page => self.cursor_at.or(self.caret_at),
            Panes::Split | Panes::Source => self.caret_at.or(self.cursor_at),
        }?;
        let (x, top, bottom) = at;
        Some((x.round(), top.round(), 1.0, (bottom - top).max(1.0).round()))
    }

    /// What is typed on the command line.
    fn command_typed(&self) -> String {
        match (&self.mode, &self.editor) {
            (Mode::Command(text), _) => format!(":{text}"),
            (_, Some(editor)) => format!(
                "{}{}",
                editor.prompt().unwrap_or(':'),
                editor.command_line().unwrap_or_default()
            ),
            _ => String::new(),
        }
    }

    /// Whether typing goes into a text of the editor now.
    fn is_typing(&self) -> bool {
        self.editor
            .as_ref()
            .is_some_and(|editor| editor.mode() == editor::Mode::Insert)
            && self.picker.is_none()
    }

    fn typing_place(&self) -> Place {
        Place {
            file: self.editor.as_ref().map(editor::Editor::file),
            cursor: self.editor.as_ref().map_or(0, editor::Editor::cursor),
            mode: self.editor.as_ref().map(editor::Editor::mode),
            command: matches!(self.mode, Mode::Command(_)),
            picking: self.picker.is_some(),
        }
    }

    fn live_preedit(&self) -> Option<&Preedit> {
        self.compose
            .preedit
            .as_ref()
            .filter(|preedit| preedit.at == self.typing_place())
    }

    /// What stands at the cursor that is not yet typed: the pre-edit, or
    /// what Ctrl-K or Ctrl-V wait with; and where in it the cursor is.
    pub(super) fn composed(&self) -> Option<(String, Option<(usize, usize)>)> {
        if let Some(preedit) = self.live_preedit() {
            return Some((preedit.text.clone(), preedit.cursor));
        }
        let pending = self.compose.pending.as_ref()?;
        let shown = pending.shown();
        let end = shown.len();
        Some((shown, Some((end, end))))
    }

    /// What is composed, as it is shown in a line of the status line or
    /// of a list: underlined, with a combining low line under each
    /// character. Empty where nothing is or it is shown elsewhere.
    pub(super) fn composed_inline(&self) -> String {
        let inline = self.picker.is_some()
            || matches!(self.mode, Mode::Command(_))
            || self
                .editor
                .as_ref()
                .is_some_and(|editor| editor.mode() == editor::Mode::Command);
        match self.composed() {
            Some((text, _)) if inline => text.chars().flat_map(|c| [c, '\u{332}']).collect(),
            _ => String::new(),
        }
    }

    /// Ctrl-K and Ctrl-V and what follows them, before a key goes
    /// anywhere else. Returns the key to go on with: the character they
    /// make, the key itself, or `None` while they wait for more.
    pub(super) fn compose_key(&mut self, press: KeyPress) -> Option<KeyPress> {
        let Some(pending) = self.compose.pending.take() else {
            let begins = press.ctrl && !press.alt && self.takes_text();
            match press.key {
                Key::Char('k') if begins => self.compose.pending = Some(Pending::Digraph(None)),
                // Ctrl-V in normal mode selects a block.
                Key::Char('v') if begins => self.compose.pending = Some(Pending::Literal),
                _ => return Some(press),
            }
            return None;
        };
        let typed = |c: char| Some(KeyPress::plain(Key::Char(c)));
        let plain = match press.key {
            Key::Char(c) if !press.ctrl && !press.alt => Some(c),
            _ => None,
        };
        match (pending, plain) {
            (_, None) if press.key == Key::Escape => None,
            (Pending::Digraph(None), None) if press == ctrl_k() => {
                self.open_chars("");
                None
            }
            (Pending::Digraph(None), Some(first)) => {
                self.compose.pending = Some(Pending::Digraph(Some(first)));
                None
            }
            // As in Vim, a pair that is no digraph types its second.
            (Pending::Digraph(Some(first)), Some(second)) => {
                typed(digraphs::digraph(first, second).unwrap_or(second))
            }
            (Pending::Literal, Some(letter @ ('u' | 'U' | 'x' | 'X' | 'o' | 'O'))) => {
                let (radix, most) = match letter {
                    'u' => (16, 4),
                    'U' => (16, 8),
                    'x' | 'X' => (16, 2),
                    _ => (8, 3),
                };
                self.compose.pending = Some(Pending::Number {
                    radix,
                    most,
                    letter: Some(letter),
                    digits: String::new(),
                });
                None
            }
            (Pending::Literal, Some(digit)) if digit.is_ascii_digit() => {
                self.compose.pending = Some(Pending::Number {
                    radix: 10,
                    most: 3,
                    letter: None,
                    digits: digit.to_string(),
                });
                None
            }
            (Pending::Literal, Some(c)) => typed(c),
            (Pending::Literal, None) if press.key == Key::Tab => typed('\t'),
            (
                Pending::Number {
                    radix,
                    most,
                    letter,
                    mut digits,
                },
                Some(digit),
            ) if digit.is_digit(radix) => {
                digits.push(digit);
                if digits.len() < most {
                    self.compose.pending = Some(Pending::Number {
                        radix,
                        most,
                        letter,
                        digits,
                    });
                    return None;
                }
                self.numbered(radix, &digits).and_then(typed)
            }
            // A number cut short by another key: what it has so far, and
            // then the key. With no digits at all, the letter itself.
            (
                Pending::Number {
                    radix,
                    letter,
                    digits,
                    ..
                },
                _,
            ) => {
                let first = if digits.is_empty() {
                    letter
                } else {
                    self.numbered(radix, &digits)
                };
                if let Some(first) = first {
                    self.plain_key(KeyPress::plain(Key::Char(first)));
                }
                Some(press)
            }
            // Anything else ends what was begun, and is itself.
            _ => Some(press),
        }
    }

    /// The character numbered `digits` in base `radix`; `None`, and said
    /// so, where there is none.
    fn numbered(&mut self, radix: u32, digits: &str) -> Option<char> {
        let found = u32::from_str_radix(digits, radix)
            .ok()
            .and_then(char::from_u32)
            .filter(|c| *c != '\0');
        if found.is_none() {
            self.alert(format!("no character has the number {digits}"));
        }
        found
    }

    /// Draws what is composed where the cursor is: on the page in the
    /// font of the text there, in the source pane (or the lens) in its.
    pub(super) fn composed_overlays(&mut self, scene: &mut Scene, has: &dyn Fn(TextureId) -> bool) {
        if !self.composed_inline().is_empty() {
            return;
        }
        let Some((text, cursor)) = self.composed() else {
            return;
        };
        if let Some(at) = self.cursor_at {
            let (family, size, weight, italic) = self.style_at_cursor().unwrap_or_else(|| {
                // As high as the line the cursor stands in.
                let height = (at.2 - at.1) / self.view.zoom.max(f64::EPSILON);
                (None, (height * 0.8).clamp(4.0, 72.0), 400, false)
            });
            let style = InPlace {
                family: family.as_deref(),
                size,
                weight,
                italic,
                width: 10_000.0,
            };
            let zoom = self.view.zoom;
            self.composed_overlay(scene, has, at, (&style, None), zoom, (&text, cursor));
        }
        if let Some(at) = self.caret_at {
            let style = InPlace {
                family: Some(crate::chrome::FONT.family()),
                size: SOURCE_SIZE,
                weight: 400,
                italic: false,
                width: 10_000.0,
            };
            let colours = Some((SOURCE_PAPER, SOURCE_INK));
            let scale = self.scale;
            self.composed_overlay(scene, has, at, (&style, colours), scale, (&text, cursor));
        }
    }

    /// The font of the text at the cursor on the page: its family, size,
    /// weight and whether it is italic.
    fn style_at_cursor(&mut self) -> Option<(Option<String>, f64, u16, bool)> {
        let change = self.places_of_text()?;
        let (editor, document, world) = (
            self.editor.as_ref()?,
            self.document.as_ref()?,
            self.compiled_world.as_ref()?,
        );
        let index = self.cursor_page.get();
        let page = document.pages().get(index)?;
        let map = self
            .maps
            .entry(index)
            .or_insert_with(|| PageMap::build(world, &page.frame));
        let (file, at) = (editor.file(), change.back_or_edge(editor.cursor()));
        let caret = map.caret(file, at).or_else(|| map.caret_before(file, at))?;
        let run = map.runs().get(caret.run)?;
        Some((
            Some(run.family.clone()),
            run.size,
            run.weight.max(100),
            run.italic,
        ))
    }

    /// Lays `composed` (its text and its cursor) over the window at `at`
    /// (across, top and bottom of the cursor), set in `style` at `scale`
    /// pixels a point, on the page's paper or on `colours`.
    fn composed_overlay(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        (x, top, bottom): (f64, f64, f64),
        (style, colours): (&InPlace, Option<(&str, &str)>),
        scale: f64,
        (text, cursor): (&str, Option<(usize, usize)>),
    ) {
        let markup = composed_markup(text, cursor, style, colours);
        let Some((image, end)) = self.chrome.render_in_place(&markup, scale) else {
            return;
        };
        let size = (
            f64::from(image.pixmap.width()),
            f64::from(image.pixmap.height()),
        );
        // Its baseline on the line's: the slip is as high as the font's
        // ascender and descender, as the cell of a glyph is.
        let y = top.max(bottom - size.1);
        scene.quads.push(Quad {
            texture: image.id,
            x: x.round() as f32,
            y: y.round() as f32,
            width: size.0 as f32,
            height: size.1 as f32,
        });
        if !has(image.id) {
            scene.images.push(image);
        }
        // The input method's cursor, where it has one and converts
        // nothing: behind what is before it.
        let Some((start, _)) = cursor.filter(|(start, stop)| start == stop) else {
            return;
        };
        let across = if start >= text.len() {
            Some(end)
        } else if start == 0 {
            None
        } else {
            let before = composed_markup(text.get(..start).unwrap_or(text), None, style, colours);
            self.chrome
                .render_in_place(&before, scale)
                .map(|(_, end)| end)
        };
        let across = across.map_or(0.0, |end| end.x);
        let bar = (1.5 * self.scale).round().max(1.0);
        tint(
            scene,
            has,
            PAGE_BAR,
            ((x + across).round(), y, bar, size.1.max(bottom - top)),
        );
    }

    /// `:char`, `:chars`, `:digraphs`. Returns whether `command` was one.
    pub(super) fn compose_command(&mut self, command: &str) -> bool {
        self.chars_command(command)
    }
}

fn ctrl_k() -> KeyPress {
    KeyPress {
        key: Key::Char('k'),
        ctrl: true,
        alt: false,
    }
}

/// Markup that sets `text` as it is composed: underlined, the part being
/// converted (`cursor`, where it is not empty) more strongly; on paper,
/// or in `colours` (the paper and the ink).
fn composed_markup(
    text: &str,
    cursor: Option<(usize, usize)>,
    style: &InPlace,
    colours: Option<(&str, &str)>,
) -> String {
    let (start, end) = cursor
        .filter(|(start, end)| start < end)
        .unwrap_or((text.len(), text.len()));
    let mut body = String::new();
    for (part, strong) in [
        (text.get(..start), false),
        (text.get(start..end), true),
        (text.get(end..), false),
    ] {
        let Some(part) = part.filter(|part| !part.is_empty()) else {
            continue;
        };
        let stroke = if strong { "0.12em" } else { "0.05em" };
        let _ = write!(
            body,
            "#underline(stroke: {stroke}, offset: 0.15em, {})",
            string(part)
        );
    }
    let markup = chrome::in_place_markup(&body, style, false);
    match colours {
        Some((paper, ink)) => markup
            .replacen("fill: white", &format!("fill: rgb(\"{paper}\")"), 1)
            .replacen(
                "#set text(",
                &format!("#set text(fill: rgb(\"{ink}\"), "),
                1,
            ),
        None => markup,
    }
}
