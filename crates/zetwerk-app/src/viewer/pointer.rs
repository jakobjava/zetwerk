//! The pointer.
//!
//! zetwerk is worked by keys, and there is nothing in it that needs a
//! pointer. The pointer is a helper for the one thing a hand on it does
//! well, which is to say "there". What it does is what keys could have
//! done, so that the keys go on from where it left off:
//!
//! - A press puts the cursor on what is under the pointer and ends a
//!   selection. While typing, typing goes on there.
//! - Dragging selects from where the button went down to the pointer, as
//!   `v` and motions do, over lines and over pages. Held beyond the top
//!   or the bottom of the text, it moves the view along. Letting go
//!   leaves the selection to the keys (`d`, `y`, `gb`, Ctrl-B, …).
//! - A second press at once selects the word under the pointer, a third
//!   one the paragraph (on a page; a heading or an item of a list is one
//!   too) or the line (in the source). Dragging on from there selects by
//!   words, by paragraphs or by lines.
//! - With Shift, a press selects from the other end of the selection (or
//!   from the cursor, where there is none) to the pointer.
//! - Selecting ends typing, as Escape does: a selection is for commands.
//! - A press on an entry of a list on screen takes that entry. While a
//!   list of places is up (`go`, `g?`, …) the keys are for it and so is
//!   the pointer: a press beside it does nothing.
//! - While a command or a search is being typed, a press does nothing.
//!
//! On a page, what is under the pointer is not always text. What a press
//! does there:
//!
//! - What a reference, a citation or a call put on the page is one
//!   thing, wherever in it the pointer is: the cursor goes on it as keys
//!   bring it there, and a second press selects it whole.
//! - A symbol of an equation is the piece of the equation it was made
//!   of. The number of an equation stands for the equation: a second
//!   press on it selects all of the equation.
//! - A picture shows no text. Where it is the picture of a figure that
//!   the page shows as one thing ("Figure 1:" before its caption), the
//!   cursor goes on that: the figure, whole. Else it goes to the first
//!   thing after the picture that the page does show. Where source and
//!   page are side by side, it goes to the call that put the picture
//!   there. A second and a third press do no more.
//! - What has no place of its own in the text (the number of a heading,
//!   the marker of a list, a page number, the white of the page) leads
//!   to the nearest text of its line, or of the nearest line.
//! - Text of another file of the document: the press opens that file
//!   there. Dragging does not lead into another file.
//!
//! A page that does not say where its text is (it is behind the text by
//! more than one change, or shows what was typed as worked out, not as
//! compiled) is no place to point at: a press on it does nothing until
//! the text is compiled. In the source pane the pointer goes by rows
//! and cells, and the pages follow the cursor it puts there as they
//! follow the keys.
//!
//! A pointer that only moves costs nothing: nothing is looked up unless
//! a button is down (see [`Viewer::pointer_moved`]), and the view only
//! moves by itself while a button is held beyond its edge.

use std::ops::Range;
use std::time::{Duration, Instant};

use typst::WorldExt;
use typst::layout::{Abs, Frame, FrameItem, Point, Transform};
use typst::syntax::{FileId, LinkedNode, Side, Source, Span, SyntaxKind};
use zetwerk_typst::{MappedGlyph, Origin, PageMap, ProjectWorld};
use zetwerk_vim::Seek;

use super::{PageRows, Paging, Panes, Viewer};
use crate::editor::{self, Editor, Rows};
use crate::keys::{Key, KeyPress};

/// How soon a press has to follow another to be the second or the third
/// of them, and not a new one.
const AGAIN: Duration = Duration::from_millis(400);
/// How far the pointer may stray between such presses, and how far it
/// may move with the button down before that is dragging, in logical
/// pixels: a hand is never quite still.
const SLACK: f64 = 4.0;
/// How near the top or the bottom of the text a pointer that selects is
/// taken to want more of it, in logical pixels: a window that fills the
/// screen has nothing beyond its edge to go to.
const EDGE: f64 = 14.0;
/// How often the view moves while the pointer is held there.
const TICK: Duration = Duration::from_millis(30);
/// How far it moves each time, in logical pixels, and how much further
/// for every pixel the pointer is beyond the edge.
const STEP: f64 = 4.0;
const GAIN: f64 = 0.5;
/// The margin around a list and the room between two of its lines, in
/// points, as `chrome::list_markup` sets them.
const LIST_MARGIN: f64 = 8.0;
const LIST_SPACING: f64 = 6.0;

/// What the pointer is doing, from one of its events to the next.
#[derive(Default)]
pub(super) struct Pointer {
    /// The press the button is still down from.
    held: Option<Held>,
    /// The last press: when it was, where, and the how-manieth of
    /// presses in a row.
    last: Option<(Instant, (f64, f64), u8)>,
    /// The byte the pointer last put the cursor at in the source pane.
    /// While the cursor is there still, the pane keeps no rows between
    /// it and its edges: the text is not to move under the pointer.
    put: Option<usize>,
    /// Where the last panel was drawn, in device pixels: left, top,
    /// width and height.
    pub(super) placed: Option<(f64, f64, f64, f64)>,
    /// The lists on screen as last drawn: of what could be typed, and of
    /// places to go.
    completion: Option<Listed>,
    picker: Option<Listed>,
}

/// A press that the button is still down from.
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent facts about one press, not states of one thing"
)]
struct Held {
    /// Where the button went down and where the pointer is now, in
    /// device pixels.
    from: (f64, f64),
    at: (f64, f64),
    /// Whether that was in the source pane, not on a page.
    in_source: bool,
    /// What the press put the cursor on or selected: the end of the
    /// selection that stays where it is.
    origin: Stretch,
    /// What dragging selects by.
    unit: Unit,
    /// Whether what is selected is whole lines of the text.
    lines: bool,
    /// Whether the pointer has left the place of the press: from then
    /// on it selects.
    dragging: bool,
    /// When the view moves next, while the pointer is held beyond the
    /// text.
    due: Option<Instant>,
    /// Whether the view could not move any further when it last was to:
    /// it is not asked again until the pointer moves.
    stuck: bool,
    /// The file the press was in and how often its text had changed by
    /// then. A key that changes the text while the button is down ends
    /// the dragging: the places it goes by are places of the old text.
    text: (FileId, u64),
}

/// What a press selects, and dragging from it selects by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    /// What is under the pointer: a character, or all of one thing a
    /// page shows.
    Thing,
    Word,
    /// A paragraph on a page, a line in the source.
    Block,
}

/// A stretch of the text being edited.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stretch {
    /// Its bytes. Empty where it is a place between two characters: the
    /// end of a line, or where typing goes.
    range: Range<usize>,
    /// Whether a page shows all of it as one thing, which a cursor is on
    /// as a whole.
    whole: bool,
}

/// What the pointer is on.
enum Target {
    /// Something of the text being edited.
    Here(Spot),
    /// A byte of another file of the document.
    Elsewhere(FileId, usize),
}

/// A place in the text being edited that the pointer is on.
struct Spot {
    /// The bytes shown there: a character, or all that one thing on a
    /// page was made of. Empty behind the end of a line and for a
    /// picture, which shows none.
    range: Range<usize>,
    /// Whether they are shown as they are written, so that the text
    /// around them says what word they are part of.
    written: bool,
    /// Whether the pointer is on the later half of what is shown.
    behind: bool,
    /// The page it is on, if it is on one.
    page: Option<usize>,
}

/// A list on screen.
#[derive(Debug, Clone, Copy)]
struct Listed {
    /// Where its panel is, in device pixels: left, top, width, height.
    panel: (f64, f64, f64, f64),
    /// Which entry is in its first row, and how many rows of entries it
    /// has.
    first: usize,
    rows: usize,
    /// How many lines it has above the entries and below them.
    above: usize,
    below: usize,
}

impl Listed {
    /// The entry whose row `at` is on, if it is on one. `scale` is how
    /// many pixels a point of the list is.
    ///
    /// The lines of a list are all as high as each other and as far
    /// apart, so the height of the panel says where each of them is.
    fn entry(&self, at: (f64, f64), scale: f64) -> Option<usize> {
        let (left, top, width, height) = self.panel;
        if self.rows == 0 || at.0 < left || at.0 >= left + width {
            return None;
        }
        let lines = (self.above + self.rows + self.below) as f64;
        let (margin, spacing) = (LIST_MARGIN * scale, LIST_SPACING * scale);
        let pitch = (height - 2.0 * margin + spacing) / lines;
        // A row reaches half the room between two lines to either side.
        let line = ((at.1 - top - margin + spacing / 2.0) / pitch).floor();
        if line < self.above as f64 || line >= (self.above + self.rows) as f64 {
            return None;
        }
        Some(self.first + line as usize - self.above)
    }
}

impl Pointer {
    /// The first row the source pane shows of `editor`'s text, in
    /// `visible` rows of `columns` cells, so that the cursor is in it.
    /// For the keys it keeps `usual` rows between the cursor and its
    /// edges; none while the cursor is where the pointer put it.
    pub(super) fn top(
        &mut self,
        editor: &mut Editor,
        columns: usize,
        visible: usize,
        usual: usize,
    ) -> usize {
        let margin = if self.put == Some(editor.cursor()) {
            0
        } else {
            self.put = None;
            usual
        };
        editor.scroll_into_view(columns, visible, margin)
    }

    /// Forgets the lists that were on screen: a frame is being drawn.
    pub(super) fn forget_lists(&mut self) {
        (self.placed, self.completion, self.picker) = (None, None, None);
    }

    /// Notes that the panel just drawn is a list: of places to go if
    /// `places`, else of what could be typed. It shows `rows` entries
    /// from entry `first` on, with `above` lines over them and `below`
    /// lines under them.
    pub(super) fn listed(
        &mut self,
        places: bool,
        first: usize,
        rows: usize,
        above: usize,
        below: usize,
    ) {
        let listed = self.placed.take().map(|panel| Listed {
            panel,
            first,
            rows,
            above,
            below,
        });
        if places {
            self.picker = listed;
        } else {
            self.completion = listed;
        }
    }
}

/// How far apart two places in the window are.
fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// The start of the last character of `range` of `text`. Of an empty
/// range, which is a place between two characters, the one before it.
fn last_of(text: &str, range: &Range<usize>) -> usize {
    let end = if range.is_empty() {
        range.start
    } else {
        range.end
    };
    let last = text.floor_char_boundary(end.min(text.len()).saturating_sub(1));
    if range.is_empty() {
        last
    } else {
        last.max(range.start.min(text.len()))
    }
}

/// The word of `text` that the character at byte `at` is part of: the
/// letters and digits around it, or the blanks around it. Any other
/// character is a word by itself: marks side by side seldom belong
/// together. Nothing, at the end of a line.
fn word(text: &str, at: usize) -> Range<usize> {
    let at = text.floor_char_boundary(at.min(text.len()));
    let Some(c) = text[at..].chars().next().filter(|c| *c != '\n') else {
        return at..at;
    };
    let blank = |c: char| c == ' ' || c == '\t';
    let alike: &dyn Fn(&char) -> bool = if c.is_alphanumeric() {
        &|other| other.is_alphanumeric()
    } else if blank(c) {
        &|other| blank(*other)
    } else {
        return at..at + c.len_utf8();
    };
    let before = text[..at].chars().rev().take_while(alike);
    let behind = text[at..].chars().take_while(alike);
    let (before, behind): (usize, usize) = (
        before.map(char::len_utf8).sum(),
        behind.map(char::len_utf8).sum(),
    );
    at - before..at + behind
}

/// The line of `text` that byte `at` is in, without its line break.
fn line(text: &str, at: usize) -> Range<usize> {
    let at = text.floor_char_boundary(at.min(text.len()));
    let start = text[..at].rfind('\n').map_or(0, |found| found + 1);
    let end = text[at..].find('\n').map_or(text.len(), |found| at + found);
    start..end
}

/// `range` of `text` without the blanks and line breaks at its ends.
fn trimmed(text: &str, range: Range<usize>) -> Range<usize> {
    let inside = &text[range.clone()];
    let start = range.start + (inside.len() - inside.trim_start().len());
    let end = range.end - (inside.len() - inside.trim_end().len());
    start..end.max(start)
}

/// The paragraph byte `at` of `source` is in, as a reader of the page
/// takes it: a heading or an item of a list by itself, or else what
/// stands between two things that end a paragraph (an empty line, a
/// heading, an item, a label, a rule) in the body the byte is in. That
/// body is the document, or the brackets of a caption or of a cell.
fn block(source: &Source, at: usize) -> Range<usize> {
    let text = source.text();
    let at = at.min(text.len().saturating_sub(1));
    let mut body = None;
    let mut node = LinkedNode::new(source.root()).leaf_at(at, Side::After);
    while let Some(here) = node {
        match here.kind() {
            SyntaxKind::Heading
            | SyntaxKind::ListItem
            | SyntaxKind::EnumItem
            | SyntaxKind::TermItem => return trimmed(text, here.range()),
            SyntaxKind::Markup => {
                body = Some(here);
                break;
            }
            _ => {}
        }
        node = here.parent().cloned();
    }
    let Some(body) = body else {
        return at..at;
    };
    let (mut start, mut end) = (body.range().start, body.range().end);
    // The `#` a rule begins with is a node before it.
    let mut hash = None;
    for child in body.children() {
        let before = hash.take();
        if child.kind() == SyntaxKind::Hash {
            hash = Some(child.range().start);
            continue;
        }
        let ends = matches!(
            child.kind(),
            SyntaxKind::Parbreak
                | SyntaxKind::Heading
                | SyntaxKind::ListItem
                | SyntaxKind::EnumItem
                | SyntaxKind::TermItem
                | SyntaxKind::Label
                | SyntaxKind::SetRule
                | SyntaxKind::ShowRule
                | SyntaxKind::LetBinding
                | SyntaxKind::ModuleImport
                | SyntaxKind::ModuleInclude
        );
        if !ends {
            continue;
        }
        let from = before.unwrap_or(child.range().start);
        if child.range().end <= at {
            start = child.range().end;
        } else if from > at {
            end = from;
            break;
        } else {
            return trimmed(text, from..child.range().end);
        }
    }
    trimmed(text, start..end)
}

/// The glyph of `map` that shows something of the text at `point` of
/// its page, or the nearest that does, and whether the point is on it.
/// Glyphs on the point's line count as nearer than any on another line.
fn glyph_at(map: &PageMap, point: Point) -> Option<(&MappedGlyph, bool)> {
    let mut best: Option<(f64, &MappedGlyph)> = None;
    for glyph in map.glyphs() {
        if matches!(glyph.origin, Origin::Atomic(_)) {
            continue;
        }
        let cell = glyph.rect;
        let beside = (cell.min.x - point.x)
            .max(point.x - cell.max.x)
            .max(Abs::zero());
        let above = (cell.min.y - point.y)
            .max(point.y - cell.max.y)
            .max(Abs::zero());
        let away = above.to_pt() * 1000.0 + beside.to_pt();
        if best.is_none_or(|(nearest, _)| away < nearest) {
            best = Some((away, glyph));
        }
    }
    best.map(|(away, glyph)| (glyph, away <= 0.0))
}

/// The picture drawn at `point` of a page whose frame (or a part of it,
/// reached by `transform`) is `frame`, as the span of what put it
/// there. Of several, the one drawn last, which is the one seen.
fn picture_at(frame: &Frame, transform: Transform, point: Point) -> Option<Span> {
    let mut found = None;
    for (position, item) in frame.items() {
        let transform = transform.pre_concat(Transform::translate(position.x, position.y));
        match item {
            FrameItem::Group(group) => {
                let transform = transform.pre_concat(group.transform);
                found = picture_at(&group.frame, transform, point).or(found);
            }
            FrameItem::Image(_, size, span) => {
                let corners = [
                    Point::zero(),
                    Point::new(size.x, Abs::zero()),
                    Point::new(Abs::zero(), size.y),
                    Point::new(size.x, size.y),
                ]
                .map(|corner| corner.transform(transform));
                let least = corners.into_iter().reduce(Point::min);
                let most = corners.into_iter().reduce(Point::max);
                if let (Some(least), Some(most)) = (least, most)
                    && least.x <= point.x
                    && point.x <= most.x
                    && least.y <= point.y
                    && point.y <= most.y
                {
                    found = Some(*span);
                }
            }
            _ => {}
        }
    }
    found
}

/// Where in the files of the project the picture at `point` of the page
/// with `frame` was put there: the file and the byte, as it is written.
fn picture_place(world: &ProjectWorld, frame: &Frame, point: Point) -> Option<(FileId, usize)> {
    let span = picture_at(frame, Transform::identity(), point)?;
    let file = span.id().filter(|file| world.is_editable(*file))?;
    let range = world.range(span)?;
    Some((file, world.written_at(file, range.start)))
}

impl Viewer {
    /// The button of the pointer went down at this place in the window,
    /// in device pixels, with Shift held or not. `now` tells whether it
    /// is the second or the third press in a row.
    pub fn pointer_pressed(&mut self, at: (f64, f64), shift: bool, now: Instant) {
        let slack = SLACK * self.display;
        let again = self.pointer.last.filter(|(then, place, _)| {
            now.saturating_duration_since(*then) <= AGAIN && distance(*place, at) <= slack
        });
        let count = again.map_or(1, |(_, _, count)| count % 3 + 1);
        self.pointer.last = Some((now, at, count));
        self.press(at, shift, count);
    }

    /// The button was pressed and let go at this place in the window, in
    /// device pixels: one click, whatever came before it.
    pub fn click(&mut self, at: (f64, f64)) {
        self.pointer.last = None;
        self.press(at, false, 1);
        self.pointer_released();
    }

    /// The pointer is at this place in the window now, in device pixels.
    /// Returns whether that changed anything.
    ///
    /// Unless a button is down it does not: this is called for every
    /// move of the pointer, and has to cost nothing then.
    pub fn pointer_moved(&mut self, at: (f64, f64)) -> bool {
        let Some(held) = &mut self.pointer.held else {
            return false;
        };
        held.at = at;
        held.stuck = false;
        if !held.dragging {
            if distance(held.from, at) <= SLACK * self.display {
                return false;
            }
            held.dragging = true;
        }
        self.drag();
        true
    }

    /// The button was let go. What is selected stays selected, for the
    /// keys.
    pub fn pointer_released(&mut self) {
        self.pointer.held = None;
    }

    /// Moves the view along with a pointer that is held beyond the top
    /// or the bottom of the text while it selects, if it is time to.
    /// Returns whether the view moved. See [`Viewer::autoscroll_at`].
    pub fn autoscroll(&mut self, now: Instant) -> bool {
        let beyond = self.beyond();
        let Some(held) = &mut self.pointer.held else {
            return false;
        };
        let Some(beyond) = beyond.filter(|_| !held.stuck) else {
            held.due = None;
            return false;
        };
        let due = held.due.is_some_and(|due| now >= due);
        if due || held.due.is_none() {
            held.due = Some(now + TICK);
        }
        if !due {
            return false;
        }
        let moved = if held.in_source {
            self.scroll_source(beyond)
        } else {
            self.scroll_pages(beyond)
        };
        if !moved && let Some(held) = &mut self.pointer.held {
            // The end of the text: nothing more to do until the pointer
            // moves.
            held.stuck = true;
            held.due = None;
        }
        moved
    }

    /// When [`Viewer::autoscroll`] wants to be called next. `None`
    /// unless the pointer is held beyond the text while it selects:
    /// there is nothing to wake up for then.
    #[must_use]
    pub fn autoscroll_at(&self) -> Option<Instant> {
        self.pointer.held.as_ref().and_then(|held| held.due)
    }

    /// Zooms by `factor`, keeping what is under the pointer, which is at
    /// `at` in the window (in device pixels), where it is. A pointer
    /// that is not over the pages zooms about their middle.
    pub fn zoom_at(&mut self, factor: f64, at: (f64, f64)) {
        let (_, left, width) = self.split();
        let across = at.0 - f64::from(left);
        let across = if across >= 0.0 && across < f64::from(width) {
            across
        } else {
            f64::from(self.view.width) / 2.0
        };
        self.zoom_by(factor, (across, at.1));
    }

    /// How high the part of the window is that shows text: all of it
    /// but the status line.
    fn text_height(&self) -> f64 {
        f64::from(self.window.1.saturating_sub(self.status_height()))
    }

    /// A press, the `count`th in a row.
    fn press(&mut self, at: (f64, f64), shift: bool, count: u8) {
        self.pointer.held = None;
        self.tooltip = None;
        if self.grid.is_some() {
            self.pointer.last = None;
            return self.grid_click(at);
        }
        if self.editor.is_none() {
            return;
        }
        if self.list_pressed(at) {
            self.pointer.last = None;
            return;
        }
        let Some(mode) = self.editor.as_ref().map(Editor::mode) else {
            return;
        };
        if at.1 >= self.text_height() || mode == editor::Mode::Command {
            return;
        }
        let in_source = at.0 < f64::from(self.split().0);
        let spot = match self.target(at, in_source, false) {
            Some(Target::Here(spot)) => spot,
            Some(Target::Elsewhere(file, offset)) => {
                self.pointer.last = None;
                return self.open_file(file, Some(offset));
            }
            None => return,
        };
        let held = if shift {
            self.pointer.last = None;
            self.extend(&spot, in_source)
        } else {
            self.begin(&spot, in_source, count)
        };
        let Some(text) = self.text_now() else {
            return;
        };
        self.pointer.held = held.map(|(origin, unit, lines)| Held {
            from: at,
            at,
            in_source,
            origin,
            unit,
            lines,
            // What is selected already grows with the least move.
            dragging: shift || unit != Unit::Thing,
            due: None,
            stuck: false,
            text,
        });
        self.pointed(in_source);
    }

    /// The file being edited and how often its text has changed.
    fn text_now(&self) -> Option<(FileId, u64)> {
        let editor = self.editor.as_ref()?;
        Some((editor.file(), editor.revision()))
    }

    /// A press on `spot`, the `count`th in a row: the cursor goes there,
    /// or the word or the paragraph there is selected. Returns what
    /// dragging on selects from, what it selects by, and whether that
    /// is whole lines.
    fn begin(&mut self, spot: &Spot, in_source: bool, count: u8) -> Option<(Stretch, Unit, bool)> {
        let picture = spot.page.is_some() && spot.range.is_empty();
        let unit = if picture || count == 1 {
            Unit::Thing
        } else if count == 2 {
            Unit::Word
        } else {
            Unit::Block
        };
        let lines = in_source && unit == Unit::Block;
        let thing = self.stretch(spot, Unit::Thing, in_source)?;
        if unit != Unit::Thing
            && let Some(chosen) = self.stretch(spot, unit, in_source)
            && (lines || !chosen.range.is_empty())
        {
            self.select(&chosen, &chosen, lines);
            return Some((chosen, unit, lines));
        }
        let typing = self.editor.as_ref()?.mode() == editor::Mode::Insert;
        // What is typed goes before a letter or behind it, whichever
        // is nearer.
        let mut offset = if typing && spot.written && spot.behind {
            spot.range.end
        } else {
            spot.range.start
        };
        if picture && let Some(shown) = self.shown_for(offset) {
            offset = shown;
        }
        let editor = self.editor.as_mut()?;
        editor.point(offset);
        let cursor = editor.cursor();
        let origin = if typing {
            // A place between two characters.
            cursor..cursor
        } else if picture {
            let shown = editor.text()[cursor..].chars().next();
            cursor..cursor + shown.map_or(0, char::len_utf8)
        } else {
            thing.range
        };
        let whole = spot.page.is_some() && !origin.is_empty();
        Some((
            Stretch {
                range: origin,
                whole,
            },
            Unit::Thing,
            false,
        ))
    }

    /// A press on `spot` with Shift held: selects from the other end of
    /// the selection, or from the cursor where there is none, to there.
    /// Returns what [`Viewer::begin`] does.
    fn extend(&mut self, spot: &Spot, in_source: bool) -> Option<(Stretch, Unit, bool)> {
        let lone = self.shown_panes() == Panes::Page;
        let editor = self.editor.as_mut()?;
        let typing = editor.mode() == editor::Mode::Insert;
        if typing {
            // What was typed so far is typed, as after a press: ending
            // it does not type it again for a count.
            editor.point(editor.cursor());
        }
        // Lines of a page are selected as characters: there, the
        // selection goes on as one of characters.
        let lines = editor.mode() == editor::Mode::VisualLine && !lone;
        let from = editor.anchor().unwrap_or(editor.cursor());
        let shown = editor.text()[from..].chars().next().filter(|_| !typing);
        let origin = Stretch {
            range: from..from + shown.map_or(0, char::len_utf8),
            whole: false,
        };
        let here = self.stretch(spot, Unit::Thing, in_source)?;
        self.select(&origin, &here, lines);
        Some((origin, Unit::Thing, lines))
    }

    /// What has to be done after the pointer moved the cursor in the
    /// source pane (`in_source`) or on a page.
    fn pointed(&mut self, in_source: bool) {
        if !in_source {
            return;
        }
        self.pointer.put = self.editor.as_ref().map(Editor::cursor);
        // The pages show where that is, as they do after a key.
        self.follow_cursor();
    }

    /// A press that may be for a list on screen. Returns whether it was.
    fn list_pressed(&mut self, at: (f64, f64)) -> bool {
        let scale = self.scale;
        let entry = |listed: Option<Listed>| listed.and_then(|listed| listed.entry(at, scale));
        if self.picker.is_some() {
            if let Some(index) = entry(self.pointer.picker) {
                self.take_listed(true, index);
            }
            return true;
        }
        if self.completion.is_some() {
            if let Some(index) = entry(self.pointer.completion) {
                self.take_listed(false, index);
                return true;
            }
            // The press is for the text: the list was for what was
            // being typed where the cursor was.
            self.completion = None;
        }
        false
    }

    /// Selects from `origin` to `here`, whichever of them comes first,
    /// or with `lines` the lines of the text they are in. Ends typing.
    fn select(&mut self, origin: &Stretch, here: &Stretch, lines: bool) {
        let typing = |viewer: &Self| {
            let editor = viewer.editor.as_ref();
            editor.is_some_and(|editor| editor.mode() == editor::Mode::Insert)
        };
        if typing(self) {
            self.plain_key(KeyPress::plain(Key::Escape));
        }
        // A thing a page shows as one is a single place for the cursor
        // where the cursor goes by the page.
        let lone = self.shown_panes() == Panes::Page;
        let Some(editor) = &self.editor else {
            return;
        };
        let text = editor.text();
        let last = |stretch: &Stretch| {
            if lone && stretch.whole {
                stretch.range.start
            } else {
                last_of(text, &stretch.range)
            }
        };
        let forward = here.range.start >= origin.range.start;
        let (from, to) = if lines {
            (origin.range.start, here.range.start)
        } else if forward {
            (origin.range.start, last(here))
        } else {
            (last(origin), here.range.start)
        };
        if forward && to < from && !lines {
            // Between two characters and back to there: nothing.
            if let Some(editor) = &mut self.editor {
                editor.point(from);
            }
            return;
        }
        self.with_editor(|editor, shown| editor.select(from, to, lines, shown));
    }

    /// Hands `ask` the text being edited and what the user sees it on,
    /// as a key is handed them.
    fn with_editor<T>(&mut self, ask: impl FnOnce(&mut Editor, &editor::Shown) -> T) -> Option<T> {
        let places = self.places_of_text();
        let layout = self.shown_panes();
        let editor = self.editor.as_mut()?;
        let pages = match (&self.document, &self.compiled_world, places) {
            (Some(document), Some(world), Some(change)) => Some(PageRows {
                document,
                world,
                file: editor.file(),
                lines: &self.page_lines,
                cursor_page: &self.cursor_page,
                change,
                landed: &self.landed,
                elsewhere: &self.elsewhere,
            }),
            _ => None,
        };
        let shown = match layout {
            Panes::Page => editor::Shown::Pages(pages.as_ref().map(|pages| pages as &dyn Rows)),
            Panes::Split | Panes::Source => editor::Shown::Source {
                columns: self.source_size.0,
            },
        };
        Some(ask(editor, &shown))
    }

    /// The place for a cursor that the pages show for byte `at` of the
    /// text, which shows as nothing itself: the thing shown that it is
    /// part of the source of, or else the first thing shown behind it.
    /// `None` unless only pages are shown: the source shows every byte.
    fn shown_for(&self, at: usize) -> Option<usize> {
        if self.shown_panes() != Panes::Page {
            return None;
        }
        let rows = PageRows {
            document: self.document.as_ref()?,
            world: self.compiled_world.as_ref()?,
            file: self.file_id(),
            lines: &self.page_lines,
            cursor_page: &self.cursor_page,
            change: self.places_of_text()?,
            landed: &self.landed,
            elsewhere: &self.elsewhere,
        };
        let around = rows.stop(at, Seek::Before).ok().flatten();
        if let Some(around) = around.filter(|around| around.contains(&at)) {
            return Some(around.start);
        }
        let after = rows.stop(at, Seek::After).ok().flatten()?;
        Some(after.start)
    }

    /// The stretch of the text that `unit` makes of `spot`.
    fn stretch(&self, spot: &Spot, unit: Unit, in_source: bool) -> Option<Stretch> {
        let editor = self.editor.as_ref()?;
        let text = editor.text();
        let start = text.floor_char_boundary(spot.range.start.min(text.len()));
        let end = text.floor_char_boundary(spot.range.end.min(text.len()));
        let whole = spot.page.is_some() && end > start;
        let thing = Stretch {
            range: start..end.max(start),
            whole,
        };
        let range = match unit {
            Unit::Thing => return Some(thing),
            // What is not shown as it is written is all the word there
            // is.
            Unit::Word if !spot.written => return Some(thing),
            Unit::Word => word(text, start),
            Unit::Block if in_source => line(text, start),
            Unit::Block => block(editor.source(), start),
        };
        Some(Stretch {
            range,
            whole: false,
        })
    }

    /// The pointer moved with the button down, or the view moved under
    /// it: selects from where the button went down to where it is.
    fn drag(&mut self) {
        let Some(held) = &self.pointer.held else {
            return;
        };
        if self.text_now() != Some(held.text) {
            self.pointer.held = None;
            return;
        }
        let (at, in_source, unit, lines) = (held.at, held.in_source, held.unit, held.lines);
        let origin = held.origin.clone();
        let Some(Target::Here(spot)) = self.target(at, in_source, true) else {
            return;
        };
        let Some(here) = self.stretch(&spot, unit, in_source) else {
            return;
        };
        self.select(&origin, &here, lines);
        self.pointed(in_source);
    }

    /// How far beyond the text a pointer that selects is held, in device
    /// pixels: above its top if negative, else below its bottom. `None`
    /// if it is not.
    fn beyond(&self) -> Option<f64> {
        let held = self.pointer.held.as_ref().filter(|held| held.dragging)?;
        let height = self.text_height();
        let edge = EDGE * self.display;
        // A press near an edge is no wish to go beyond it: from there,
        // only leaving the text is one.
        let top = if held.from.1 < edge { 0.0 } else { edge };
        let bottom = if held.from.1 >= height - edge {
            height
        } else {
            height - edge
        };
        let y = held.at.1;
        if y < top {
            Some(y - top)
        } else if y >= bottom {
            Some(y - bottom + 1.0)
        } else {
            None
        }
    }

    /// How far the view moves at once for a pointer held `beyond` the
    /// text, in device pixels, the way it is beyond.
    fn step(&self, beyond: f64) -> f64 {
        let most = self.text_height() / 3.0;
        (STEP * self.display + GAIN * beyond.abs())
            .min(most)
            .copysign(beyond)
    }

    /// Moves the pages for a pointer held `beyond` them and selects up
    /// to what is under it then. Returns whether they moved.
    fn scroll_pages(&mut self, beyond: f64) -> bool {
        if self.paging != Paging::Scroll {
            return false;
        }
        let before = self.view.top;
        self.scroll(0.0, self.step(beyond));
        if (self.view.top - before).abs() < f64::EPSILON {
            return false;
        }
        self.drag();
        true
    }

    /// Selects up to a row beyond those the source pane shows, for a
    /// pointer held `beyond` them, and moves the pane to show it.
    /// Returns whether there was such a row.
    fn scroll_source(&mut self, beyond: f64) -> bool {
        let (_, _, row_height) = self.source_metrics;
        let (columns, visible) = self.source_size;
        let rows = (self.step(beyond).abs() / row_height).ceil().max(1.0) as isize;
        let row = if beyond < 0.0 {
            -rows
        } else {
            isize::try_from(visible).unwrap_or(isize::MAX) - 1 + rows
        };
        let Some(held) = &self.pointer.held else {
            return false;
        };
        if self.text_now() != Some(held.text) {
            self.pointer.held = None;
            return false;
        }
        let (across, unit, lines) = (held.at.0, held.unit, held.lines);
        let origin = held.origin.clone();
        let before = self.editor.as_ref().map(Editor::cursor);
        let Some(spot) = self.source_spot(across, row) else {
            return false;
        };
        let Some(here) = self.stretch(&spot, unit, true) else {
            return false;
        };
        self.select(&origin, &here, lines);
        if let Some(editor) = &mut self.editor {
            editor.scroll_into_view(columns, visible, 0);
        }
        self.pointed(true);
        self.editor.as_ref().map(Editor::cursor) != before
    }

    /// What the pointer is on at `at` in the window: in the source pane
    /// if `in_source`, else on a page. With `nearest`, what is nearest
    /// to it in that pane, wherever it is itself: a pointer that selects
    /// may be anywhere.
    fn target(&mut self, at: (f64, f64), in_source: bool, nearest: bool) -> Option<Target> {
        if !in_source {
            return self.page_target(at, nearest);
        }
        let (_, _, row_height) = self.source_metrics;
        let last = self.source_size.1.saturating_sub(1) as f64;
        let row = (at.1 / row_height).floor().max(0.0);
        let row = if nearest { row.min(last) } else { row };
        self.source_spot(at.0, row as isize).map(Target::Here)
    }

    /// What the source pane shows `across` the window in the row `row`
    /// rows below its first (above it, if negative).
    fn source_spot(&mut self, across: f64, row: isize) -> Option<Spot> {
        let (gutter, cell, _) = self.source_metrics;
        let columns = self.source_size.0;
        let editor = self.editor.as_mut()?;
        let cells = ((across - gutter) / cell).max(0.0);
        let offset = editor.offset_near(columns, row, cells.floor() as usize);
        let shown = editor.text()[offset..].chars().next();
        let shown = shown.filter(|c| *c != '\n');
        Some(Spot {
            range: offset..offset + shown.map_or(0, char::len_utf8),
            written: true,
            behind: cells.fract() >= 0.5,
            page: None,
        })
    }

    /// What the pages show at `at` in the window. Nothing where the
    /// pages are not those of the text as it is (see [`Viewer::places`])
    /// or where the pointer is on no page, unless `nearest`: then, what
    /// the nearest page shows nearest to it.
    fn page_target(&mut self, (x, y): (f64, f64), nearest: bool) -> Option<Target> {
        let change = self.places()?;
        let (_, pane_left, _) = self.split();
        let zoom = self.view.zoom;
        let edited = self.file_id();
        // The page, how far from it the pointer is, and where on it (or
        // on its edge) that is.
        let mut best: Option<(f64, usize, (f64, f64))> = None;
        for visible in self.view.visible(&self.layout) {
            let Some(page_box) = self.layout.pages.get(visible.index) else {
                continue;
            };
            let (left, top) = (f64::from(pane_left) + visible.x as f64, visible.y as f64);
            let (width, height) = (f64::from(page_box.width), f64::from(page_box.height));
            let beside = (left - x).max(x - (left + width - 1.0)).max(0.0);
            let above = (top - y).max(y - (top + height - 1.0)).max(0.0);
            if best.is_none_or(|(away, _, _)| beside + above < away) {
                let within = (
                    (x - left).clamp(0.0, (width - 1.0).max(0.0)),
                    (y - top).clamp(0.0, (height - 1.0).max(0.0)),
                );
                best = Some((beside + above, visible.index, within));
            }
        }
        let (away, index, within) = best?;
        if away > 0.0 && !nearest {
            return None;
        }
        let (document, world) = (self.document.as_ref()?, self.compiled_world.as_ref()?);
        let page = document.pages().get(index)?;
        let map = self
            .maps
            .entry(index)
            .or_insert_with(|| PageMap::build(world, &page.frame));
        let point = Point::new(Abs::pt(within.0 / zoom), Abs::pt(within.1 / zoom));
        let glyph = glyph_at(map, point);
        // A picture, where the pointer is on no text: dragging goes by
        // the text alone.
        let on_text = glyph.is_some_and(|(_, on)| on);
        if !nearest
            && !on_text
            && let Some((file, offset)) = picture_place(world, &page.frame, point)
        {
            if file != edited {
                return Some(Target::Elsewhere(file, offset));
            }
            let offset = change.forth(offset);
            self.cursor_page.set(index);
            self.landed.set(None);
            return Some(Target::Here(Spot {
                range: offset..offset,
                written: false,
                behind: false,
                page: Some(index),
            }));
        }
        let (glyph, _) = glyph?;
        let (file, range, written) = match &glyph.origin {
            Origin::Text { file, range } => (*file, range, true),
            Origin::Space { file, range }
            | Origin::Math { file, range }
            | Origin::Object { file, range, .. } => (*file, range, false),
            Origin::Atomic(_) => return None,
        };
        if file != edited {
            return Some(Target::Elsewhere(file, range.start));
        }
        let cell = glyph.rect;
        let start = change.forth(range.start);
        // Where the keys look for the cursor first.
        self.cursor_page.set(index);
        self.landed.set(None);
        Some(Target::Here(Spot {
            range: start..start + range.len(),
            written,
            behind: point.x > cell.min.x + (cell.max.x - cell.min.x) / 2.0,
            page: Some(index),
        }))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use std::fmt::Write as _;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::super::tests::{compile, directory, editing, open, press, tints};
    use super::super::{PAGE_BAR, PAGE_CURSOR, PAGE_SELECTION};
    use super::*;
    use crate::gpu::Quad;

    /// A page narrow enough to break a paragraph into lines.
    const NARROW: &str = "#set page(width: 300pt, height: 300pt)\n";
    /// A paragraph that takes a few of its lines.
    const FOX: &str = "The quick brown fox jumps over the lazy dog and runs on \
                       through the wood until the line is full and then some more \
                       of it follows here.\n";

    /// A time long after every press there was: presses are in a row
    /// where a test makes them so, however fast it runs.
    fn later() -> Instant {
        static PRESSES: AtomicU32 = AtomicU32::new(1);
        Instant::now() + Duration::from_secs(10) * PRESSES.fetch_add(1, Ordering::Relaxed)
    }

    fn editor(viewer: &Viewer) -> &Editor {
        viewer.editor.as_ref().unwrap()
    }

    fn mode(viewer: &Viewer) -> editor::Mode {
        editor(viewer).mode()
    }

    /// What is selected.
    fn selected(viewer: &Viewer) -> Option<String> {
        let range = editor(viewer).selection()?;
        Some(editor(viewer).text()[range].to_owned())
    }

    /// The text from the cursor on.
    fn from_cursor(viewer: &Viewer) -> &str {
        &editor(viewer).text()[editor(viewer).cursor()..]
    }

    /// Where in the window a page shows the first glyph whose origin is
    /// `wanted`: a place in the left half of it, or with `behind` in the
    /// right half.
    fn glyph_place(viewer: &Viewer, behind: bool, wanted: impl Fn(&Origin) -> bool) -> (f64, f64) {
        let world = viewer.compiled_world.clone().unwrap();
        let document = viewer.document.clone().unwrap();
        let (_, pane_left, _) = viewer.split();
        let zoom = viewer.view.zoom;
        for visible in viewer.view.visible(&viewer.layout) {
            let map = PageMap::build(&world, &document.pages()[visible.index].frame);
            if let Some(glyph) = map.glyphs().iter().find(|glyph| wanted(&glyph.origin)) {
                let cell = glyph.rect;
                let part = if behind { 0.75 } else { 0.25 };
                let x = cell.min.x.to_pt() + (cell.max.x - cell.min.x).to_pt() * part;
                let y = f64::midpoint(cell.min.y.to_pt(), cell.max.y.to_pt());
                return (
                    f64::from(pane_left) + visible.x as f64 + x * zoom,
                    visible.y as f64 + y * zoom,
                );
            }
        }
        panic!("no page in view shows that");
    }

    /// Where in the window a page shows what begins at byte `offset` of
    /// the text it was compiled from.
    fn shown_at(viewer: &Viewer, offset: usize) -> (f64, f64) {
        glyph_place(viewer, false, |origin| match origin {
            Origin::Text { range, .. }
            | Origin::Space { range, .. }
            | Origin::Math { range, .. }
            | Origin::Object { range, .. } => range.start == offset,
            Origin::Atomic(_) => false,
        })
    }

    /// Where in the window a page shows the first of `words` in the
    /// text.
    fn word_at(viewer: &Viewer, words: &str) -> (f64, f64) {
        shown_at(viewer, viewer.text().unwrap().find(words).unwrap())
    }

    /// A place in the source pane: `cell` cells into row `row`, both
    /// counted from 0 and into the middle of them.
    fn in_source(viewer: &Viewer, row: usize, cell: usize) -> (f64, f64) {
        let (gutter, cell_width, row_height) = viewer.source_metrics;
        (
            gutter + (cell as f64 + 0.25) * cell_width,
            (row as f64 + 0.5) * row_height,
        )
    }

    /// Shows the pages alone and draws a frame.
    fn pages_alone(viewer: &mut Viewer) {
        press(viewer, "gsgs");
        assert_eq!(viewer.shown_panes(), Panes::Page);
        let _ = tints(viewer, PAGE_CURSOR);
    }

    /// Presses at `from`, moves to `to` and lets go.
    fn drag(viewer: &mut Viewer, from: (f64, f64), to: (f64, f64)) {
        viewer.pointer_pressed(from, false, later());
        assert!(viewer.pointer_moved(to));
        viewer.pointer_released();
    }

    fn done(directory: PathBuf) {
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_pointer_that_only_moves_does_nothing() {
        let (mut viewer, directory) = editing(&format!("{NARROW}{FOX}"));
        pages_alone(&mut viewer);
        let cursor = editor(&viewer).cursor();
        assert!(!viewer.pointer_moved(word_at(&viewer, "wood")));
        assert!(!viewer.autoscroll(later()));
        assert_eq!(viewer.autoscroll_at(), None);
        assert_eq!(editor(&viewer).cursor(), cursor);
        assert_eq!(mode(&viewer), editor::Mode::Normal);
        // Nor does one that trembles while the button is down.
        let quick = word_at(&viewer, "quick");
        viewer.pointer_pressed(quick, false, later());
        assert!(!viewer.pointer_moved((quick.0 + 1.5, quick.1 - 1.0)));
        viewer.pointer_released();
        assert_eq!(mode(&viewer), editor::Mode::Normal);
        assert!(from_cursor(&viewer).starts_with("quick"));
        done(directory);
    }

    #[test]
    fn dragging_selects_over_the_lines_of_a_page() {
        let text = format!("{NARROW}{FOX}\nSecond paragraph.\n");
        let (mut viewer, directory) = editing(&text);
        pages_alone(&mut viewer);
        let (quick, wood) = (word_at(&viewer, "quick"), word_at(&viewer, "d until"));
        assert!(wood.1 > quick.1 + 10.0, "on a later line");
        // The press puts the cursor there; the move selects.
        viewer.pointer_pressed(quick, false, later());
        assert_eq!(mode(&viewer), editor::Mode::Normal);
        assert!(from_cursor(&viewer).starts_with("quick"));
        assert!(viewer.pointer_moved(word_at(&viewer, "fox")));
        assert_eq!(selected(&viewer).as_deref(), Some("quick brown f"));
        assert!(viewer.pointer_moved(wood));
        let whole = "quick brown fox jumps over the lazy dog and runs on through the wood";
        assert_eq!(selected(&viewer).as_deref(), Some(whole));
        // Back before where the button went down: that end stays.
        assert!(viewer.pointer_moved(word_at(&viewer, "The")));
        assert_eq!(selected(&viewer).as_deref(), Some("The q"));
        assert!(viewer.pointer_moved(wood));
        viewer.pointer_released();
        // Letting go leaves it for the keys, and on the page.
        assert_eq!(mode(&viewer), editor::Mode::Visual);
        assert_eq!(selected(&viewer).as_deref(), Some(whole));
        let shown = tints(&mut viewer, PAGE_SELECTION);
        assert!(shown.len() >= 2, "a stretch for each line: {shown:?}");
        press(&mut viewer, "d");
        assert_eq!(mode(&viewer), editor::Mode::Normal);
        assert!(viewer.text().unwrap().contains("The  until the line"));
        press(&mut viewer, "u");
        assert_eq!(viewer.text(), Some(text.as_str()));
        compile(&mut viewer);
        // From the later end to the earlier one is the same.
        drag(&mut viewer, wood, quick);
        assert_eq!(selected(&viewer).as_deref(), Some(whole));
        // `y` and what is selected is in the register; a press ends
        // the selection.
        press(&mut viewer, "y");
        viewer.click(word_at(&viewer, "Second"));
        assert_eq!(mode(&viewer), editor::Mode::Normal);
        press(&mut viewer, "P");
        assert!(viewer.text().unwrap().contains(&format!("{whole}Second")));
        done(directory);
    }

    #[test]
    fn a_key_that_changes_the_text_ends_the_dragging() {
        let (mut viewer, directory) = editing(&format!("{NARROW}{FOX}"));
        pages_alone(&mut viewer);
        let (quick, fox, jumps) = (
            word_at(&viewer, "quick"),
            word_at(&viewer, "fox"),
            word_at(&viewer, "jumps"),
        );
        viewer.pointer_pressed(quick, false, later());
        assert!(viewer.pointer_moved(fox));
        // A key that changes nothing leaves it going.
        press(&mut viewer, "o");
        assert!(viewer.pointer_moved(jumps));
        assert_eq!(selected(&viewer).as_deref(), Some("quick brown fox j"));
        press(&mut viewer, "d");
        assert!(viewer.text().unwrap().contains("The umps over"));
        assert!(viewer.pointer_moved(fox));
        assert_eq!(mode(&viewer), editor::Mode::Normal);
        assert_eq!(selected(&viewer), None);
        assert!(!viewer.pointer_moved(quick));
        done(directory);
    }

    #[test]
    fn the_source_does_not_move_under_the_pointer() {
        let mut text = String::new();
        for line in 1..=120 {
            let _ = writeln!(text, "line {line}");
        }
        let (mut viewer, directory) = editing(&text);
        let _ = tints(&mut viewer, PAGE_CURSOR);
        let last = in_source(&viewer, viewer.source_size.1 - 1, 5);
        let line = |viewer: &mut Viewer| {
            let _ = tints(viewer, PAGE_CURSOR);
            viewer.state().editing.unwrap().line
        };
        // The keys keep a few rows between the cursor and the edge of
        // the pane; under the pointer that would move the row away
        // that the second press is meant for.
        viewer.click(last);
        let pressed = line(&mut viewer);
        assert_eq!(pressed, viewer.source_size.1);
        viewer.click(last);
        assert_eq!(line(&mut viewer), pressed);
        let now = later();
        viewer.pointer_pressed(last, false, now);
        viewer.pointer_released();
        let _ = line(&mut viewer);
        viewer.pointer_pressed(last, false, now + AGAIN / 2);
        viewer.pointer_released();
        assert_eq!(selected(&viewer), Some(format!("{pressed}")));
        // With the next key that moves the cursor the rows are back.
        press(&mut viewer, "<Esc>j");
        assert_eq!(line(&mut viewer), pressed + 1);
        press(&mut viewer, "k");
        assert_eq!(line(&mut viewer), pressed);
        viewer.click(last);
        assert!(line(&mut viewer) > pressed);
        done(directory);
    }

    #[test]
    fn dragging_selects_from_one_page_to_the_next() {
        let text = "#set page(width: 300pt, height: 100pt, margin: 20pt)\n\
                    Alpha is on the first page.\n\
                    #pagebreak()\n\
                    Omega is on the second page.\n";
        let (mut viewer, directory) = editing(text);
        pages_alone(&mut viewer);
        assert_eq!(viewer.state().pages, 2);
        let (first, second) = (
            word_at(&viewer, "first"),
            word_at(&viewer, "is on the second"),
        );
        drag(&mut viewer, first, second);
        let between = "first page.\n#pagebreak()\nOmega i";
        assert_eq!(selected(&viewer).as_deref(), Some(between));
        // Both pages show their part of it.
        let shown = tints(&mut viewer, PAGE_SELECTION);
        let page_break = f64::midpoint(first.1, second.1);
        assert!(shown.iter().any(|quad| f64::from(quad.y) < page_break));
        assert!(shown.iter().any(|quad| f64::from(quad.y) > page_break));
        // The white between two pages is no place: the nearest page is
        // asked.
        viewer.pointer_pressed(first, false, later());
        assert!(viewer.pointer_moved((second.0, second.1 + 40.0)));
        assert!(selected(&viewer).unwrap().starts_with("first page."));
        viewer.pointer_released();
        press(&mut viewer, "<Esc>");
        drag(&mut viewer, first, second);
        press(&mut viewer, "d");
        assert!(
            viewer
                .text()
                .unwrap()
                .contains("Alpha is on the s on the second page."),
            "{:?} {:?}",
            viewer.text(),
            selected(&viewer)
        );
        done(directory);
    }

    #[test]
    fn dragging_beyond_the_pages_moves_them_along() {
        let mut text = "#set page(width: 300pt, height: 200pt, margin: 20pt)\n".to_owned();
        for page in 1..=5 {
            let _ = writeln!(text, "Page {page} begins here and goes on.");
            if page < 5 {
                text.push_str("#pagebreak()\n");
            }
        }
        let (mut viewer, directory) = editing(&text);
        pages_alone(&mut viewer);
        let (begins, page) = (word_at(&viewer, "begins"), word_at(&viewer, "Page 1"));
        let now = later();
        viewer.pointer_pressed(begins, false, now);
        // In the status line, below the text.
        assert!(viewer.pointer_moved((begins.0, viewer.text_height() + 8.0)));
        assert_eq!(viewer.view.top, 0.0);
        // The first call sets the time; the view moves when it has come.
        assert!(!viewer.autoscroll(now));
        assert_eq!(viewer.autoscroll_at(), Some(now + TICK));
        assert!(!viewer.autoscroll(now + TICK / 2));
        assert!(viewer.autoscroll(now + TICK));
        let moved = viewer.view.top;
        assert!(moved > 0.0);
        assert_eq!(viewer.autoscroll_at(), Some(now + 2 * TICK));
        // On to the end of the document, where it stops asking.
        let mut time = now + TICK;
        let mut steps = 0;
        while viewer.autoscroll_at().is_some() {
            time += TICK;
            viewer.autoscroll(time);
            steps += 1;
            assert!(steps < 5000, "the document has an end");
        }
        assert!(viewer.view.top > moved);
        assert!(!viewer.autoscroll(time + TICK));
        let all = selected(&viewer).unwrap();
        assert!(
            all.starts_with("begins here") && all.ends_with("Page 5 b"),
            "{all}"
        );
        // Back up, beyond the top, to the first page.
        assert!(viewer.pointer_moved((page.0, -30.0)));
        assert!(!viewer.autoscroll(time));
        let mut steps = 0;
        while viewer.autoscroll_at().is_some() {
            time += TICK;
            viewer.autoscroll(time);
            steps += 1;
            assert!(steps < 5000, "the document has a beginning");
        }
        assert_eq!(viewer.view.top, 0.0);
        let all = selected(&viewer).unwrap();
        assert_eq!(all, "Page 1 b");
        // Inside the text again, and let go: nothing more to wait for.
        assert!(viewer.pointer_moved(begins));
        assert!(!viewer.autoscroll(time));
        assert_eq!(viewer.autoscroll_at(), None);
        assert!(viewer.pointer_moved((begins.0, -30.0)));
        viewer.pointer_released();
        assert!(!viewer.autoscroll(time + TICK));
        assert_eq!(viewer.autoscroll_at(), None);
        done(directory);
    }

    #[test]
    fn dragging_selects_in_the_source() {
        let text = "one two three\nfour five six\nseven eight nine\n";
        let (mut viewer, directory) = editing(text);
        let _ = tints(&mut viewer, PAGE_CURSOR);
        // From the `t` of two to the `e` of eight.
        let (two, eight) = (in_source(&viewer, 0, 4), in_source(&viewer, 2, 6));
        viewer.pointer_pressed(two, false, later());
        assert_eq!(mode(&viewer), editor::Mode::Normal);
        assert!(from_cursor(&viewer).starts_with("two"));
        assert!(viewer.pointer_moved(eight));
        viewer.pointer_released();
        assert_eq!(mode(&viewer), editor::Mode::Visual);
        let between = "two three\nfour five six\nseven e";
        assert_eq!(selected(&viewer).as_deref(), Some(between));
        // Behind the end of a line is its end, without the line break.
        let behind = (in_source(&viewer, 1, 40).0, in_source(&viewer, 1, 0).1);
        drag(&mut viewer, two, behind);
        assert_eq!(
            selected(&viewer).as_deref(),
            Some("two three\nfour five six")
        );
        // A pointer that leaves the pane sideways still selects in it.
        drag(&mut viewer, two, (900.0, eight.1));
        assert_eq!(
            selected(&viewer).as_deref(),
            Some("two three\nfour five six\nseven eight nine")
        );
        drag(&mut viewer, two, eight);
        press(&mut viewer, "d");
        assert_eq!(viewer.text(), Some("one ight nine\n"));
        done(directory);
    }

    #[test]
    fn dragging_beyond_the_source_moves_it_along() {
        let mut text = String::new();
        for line in 1..=120 {
            let _ = writeln!(text, "line {line}");
        }
        let (mut viewer, directory) = editing(&text);
        let _ = tints(&mut viewer, PAGE_CURSOR);
        let visible = viewer.source_size.1;
        assert!(visible < 60);
        let now = later();
        let from = in_source(&viewer, 2, 0);
        viewer.pointer_pressed(from, false, now);
        // Within the pane the rows are where they are: the last row is
        // the last row, however near the edge.
        assert!(viewer.pointer_moved(in_source(&viewer, visible - 1, 0)));
        let _ = tints(&mut viewer, PAGE_CURSOR);
        assert_eq!(viewer.state().editing.unwrap().line, visible);
        assert!(viewer.pointer_moved((from.0, viewer.text_height() + 5.0)));
        assert!(!viewer.autoscroll(now));
        assert!(viewer.autoscroll(now + TICK));
        let _ = tints(&mut viewer, PAGE_CURSOR);
        let line = viewer.state().editing.unwrap().line;
        assert!(line > visible, "{line}");
        let mut time = now + TICK;
        while viewer.autoscroll_at().is_some() {
            time += TICK;
            viewer.autoscroll(time);
            let _ = tints(&mut viewer, PAGE_CURSOR);
        }
        assert_eq!(viewer.state().editing.unwrap().line, 120);
        let all = selected(&viewer).unwrap();
        assert!(
            all.starts_with("line 3\n") && all.ends_with("line 119\nl"),
            "{all}"
        );
        // And up again.
        assert!(viewer.pointer_moved((from.0, -20.0)));
        assert!(!viewer.autoscroll(time));
        while viewer.autoscroll_at().is_some() {
            time += TICK;
            viewer.autoscroll(time);
            let _ = tints(&mut viewer, PAGE_CURSOR);
        }
        assert_eq!(viewer.state().editing.unwrap().line, 1);
        viewer.pointer_released();
        assert_eq!(selected(&viewer).as_deref(), Some("line 1\nline 2\nl"));
        done(directory);
    }

    #[test]
    fn a_second_press_selects_the_word_and_a_third_the_paragraph() {
        let text = format!(
            "{NARROW}#set heading(numbering: \"1.\")\n= Start <start>\n\
             Some plain words here, and *strong* ones.\n\nSecond paragraph.\n"
        );
        let (mut viewer, directory) = editing(&text);
        pages_alone(&mut viewer);
        let now = later();
        let plain = word_at(&viewer, "ain words");
        viewer.pointer_pressed(plain, false, now);
        viewer.pointer_released();
        assert_eq!(selected(&viewer), None);
        viewer.pointer_pressed(plain, false, now + AGAIN / 2);
        assert_eq!(selected(&viewer).as_deref(), Some("plain"));
        // Dragging on from there selects by words.
        assert!(viewer.pointer_moved(word_at(&viewer, "re, and")));
        assert_eq!(selected(&viewer).as_deref(), Some("plain words here"));
        assert!(viewer.pointer_moved(word_at(&viewer, "me plain")));
        assert_eq!(selected(&viewer).as_deref(), Some("Some plain"));
        assert!(viewer.pointer_moved(plain));
        viewer.pointer_released();
        assert_eq!(selected(&viewer).as_deref(), Some("plain"));
        // The third press: the paragraph, and no more of the text than
        // the page shows of it.
        viewer.pointer_pressed(plain, false, now + AGAIN);
        viewer.pointer_released();
        assert_eq!(
            selected(&viewer).as_deref(),
            Some("Some plain words here, and *strong* ones.")
        );
        // A fourth is a first again; and so is one that comes late.
        viewer.pointer_pressed(plain, false, now + AGAIN * 3 / 2);
        viewer.pointer_released();
        assert_eq!(selected(&viewer), None);
        viewer.pointer_pressed(plain, false, now + AGAIN * 4);
        viewer.pointer_released();
        assert_eq!(selected(&viewer), None);
        assert!(from_cursor(&viewer).starts_with("ain words"));
        // What is selected is for the keys.
        viewer.pointer_pressed(plain, false, now + AGAIN * 9 / 2);
        viewer.pointer_released();
        press(&mut viewer, "<C-b>");
        assert!(viewer.text().unwrap().contains("Some *plain* words"));
        press(&mut viewer, "u");
        compile(&mut viewer);
        // A heading is a paragraph by itself.
        let start = word_at(&viewer, "tart <");
        for press in 0..3 {
            viewer.pointer_pressed(start, false, now + AGAIN * 10 + AGAIN * press / 3);
            viewer.pointer_released();
        }
        assert_eq!(selected(&viewer).as_deref(), Some("Start"));
        done(directory);
    }

    #[test]
    fn in_the_source_a_third_press_selects_the_line() {
        let text = "one two three\nfour five six\n\nseven eight nine\n";
        let (mut viewer, directory) = editing(text);
        let _ = tints(&mut viewer, PAGE_CURSOR);
        let now = later();
        let five = in_source(&viewer, 1, 6);
        viewer.pointer_pressed(five, false, now);
        viewer.pointer_released();
        viewer.pointer_pressed(five, false, now + AGAIN / 3);
        assert_eq!(selected(&viewer).as_deref(), Some("five"));
        assert_eq!(mode(&viewer), editor::Mode::Visual);
        viewer.pointer_released();
        viewer.pointer_pressed(five, false, now + AGAIN * 2 / 3);
        assert_eq!(mode(&viewer), editor::Mode::VisualLine);
        assert_eq!(selected(&viewer).as_deref(), Some("four five six\n"));
        // Dragging on selects by lines.
        assert!(viewer.pointer_moved(in_source(&viewer, 3, 2)));
        viewer.pointer_released();
        assert_eq!(
            selected(&viewer).as_deref(),
            Some("four five six\n\nseven eight nine\n")
        );
        press(&mut viewer, "d");
        assert_eq!(viewer.text(), Some("one two three\n"));
        done(directory);
    }

    /// A document with things on its page that are not its text.
    fn things() -> (Viewer, PathBuf, String) {
        let text = "#set math.equation(numbering: \"(1)\")\n\
                    #set heading(numbering: \"1.\")\n\
                    = Start <start>\n\
                    Before the figure.\n\n\
                    #figure(\n  image(\"pic.svg\", width: 80pt),\n  caption: [A cat.],\n)\n\n\
                    After it, as @start says.\n\n\
                    $ a^2 + b^2 = c^2 $\n\n\
                    - one item\n- two items\n";
        let directory = directory(text);
        let picture = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100\" height=\"50\">\
                       <rect width=\"100\" height=\"50\" fill=\"red\"/></svg>";
        std::fs::write(directory.join("pic.svg"), picture).unwrap();
        let (mut viewer, _) = open(&directory);
        // Again, now that the picture is there.
        compile(&mut viewer);
        assert_eq!(viewer.state().problems, Vec::<String>::new());
        (viewer, directory, text.to_owned())
    }

    /// Presses at `at` once, twice or three times in a row.
    fn presses(viewer: &mut Viewer, at: (f64, f64), times: u32) {
        let now = later();
        for press in 0..times {
            viewer.pointer_pressed(at, false, now + AGAIN * press / 4);
            viewer.pointer_released();
        }
    }

    #[test]
    fn what_a_page_shows_that_is_not_text_has_its_place() {
        let (mut viewer, directory, text) = things();
        pages_alone(&mut viewer);
        // What a reference shows is one thing.
        let reference = text.find("@start").unwrap();
        let section = glyph_place(
            &viewer,
            true,
            |origin| matches!(origin, Origin::Object { range, .. } if range.start == reference),
        );
        presses(&mut viewer, section, 1);
        assert_eq!(editor(&viewer).cursor(), reference);
        presses(&mut viewer, section, 2);
        assert_eq!(selected(&viewer).as_deref(), Some("@start"));
        // Dragging over it takes all of it.
        let (as_says, says) = (word_at(&viewer, "as @"), word_at(&viewer, "says"));
        drag(&mut viewer, as_says, section);
        assert_eq!(selected(&viewer).as_deref(), Some("as @start"));
        drag(&mut viewer, says, section);
        assert_eq!(selected(&viewer).as_deref(), Some("@start s"));
        press(&mut viewer, "<Esc>");
        // The number of an equation stands for the equation; a symbol
        // is a piece of it.
        let equation = text.find("$ a^2").unwrap();
        let (number, symbol) = (shown_at(&viewer, equation), word_at(&viewer, "b^2"));
        presses(&mut viewer, number, 2);
        assert_eq!(selected(&viewer).as_deref(), Some("$ a^2 + b^2 = c^2 $"));
        presses(&mut viewer, symbol, 1);
        assert!(from_cursor(&viewer).starts_with("b^2 = c^2"));
        presses(&mut viewer, symbol, 2);
        assert_eq!(selected(&viewer).as_deref(), Some("b"));
        press(&mut viewer, "<Esc>");
        // The number of a heading and the marker of an item have no
        // place of their own: the text beside them has.
        let zoom = viewer.view.zoom;
        let heading = word_at(&viewer, "Start");
        presses(&mut viewer, (heading.0 - 14.0 * zoom, heading.1), 1);
        assert!(from_cursor(&viewer).starts_with("Start"));
        let item = word_at(&viewer, "two items");
        presses(&mut viewer, (item.0 - 8.0 * zoom, item.1), 1);
        assert!(from_cursor(&viewer).starts_with("two items"));
        // And so has the white of the page: beside a line, its end.
        let says = word_at(&viewer, "says");
        presses(&mut viewer, (says.0 + 150.0 * zoom, says.1), 1);
        assert!(from_cursor(&viewer).starts_with(".\n"));
        // A picture: the figure it is the picture of, which the page
        // shows as one thing.
        let (before, caption) = (word_at(&viewer, "Before"), word_at(&viewer, "A cat"));
        let picture = (caption.0, f64::midpoint(before.1, caption.1));
        for times in 1..=3 {
            presses(&mut viewer, picture, times);
            assert_eq!(mode(&viewer), editor::Mode::Normal);
            assert!(from_cursor(&viewer).starts_with("figure(\n  image("));
        }
        let on_figure = tints(&mut viewer, PAGE_CURSOR);
        assert_eq!(on_figure.len(), 1);
        assert!(f64::from(on_figure[0].x) < caption.0);
        done(directory);
    }

    #[test]
    fn beside_the_source_a_picture_leads_to_its_call() {
        let (mut viewer, directory, text) = things();
        let _ = tints(&mut viewer, PAGE_CURSOR);
        assert_eq!(viewer.shown_panes(), Panes::Split);
        let (before, caption) = (word_at(&viewer, "Before"), word_at(&viewer, "A cat"));
        presses(
            &mut viewer,
            (caption.0, f64::midpoint(before.1, caption.1)),
            1,
        );
        assert!(from_cursor(&viewer).starts_with("image(\"pic.svg\""));
        // What a reference shows is all of it there too.
        let reference = text.find("@start").unwrap();
        let section = glyph_place(
            &viewer,
            true,
            |origin| matches!(origin, Origin::Object { range, .. } if range.start == reference),
        );
        presses(&mut viewer, section, 2);
        assert_eq!(selected(&viewer).as_deref(), Some("@start"));
        let as_says = word_at(&viewer, "as @");
        drag(&mut viewer, as_says, section);
        assert_eq!(selected(&viewer).as_deref(), Some("as @start"));
        // A third press on the page is the paragraph, as it is written.
        presses(&mut viewer, section, 3);
        assert_eq!(
            selected(&viewer).as_deref(),
            Some("After it, as @start says.")
        );
        assert_eq!(mode(&viewer), editor::Mode::Visual);
        done(directory);
    }

    #[test]
    fn with_shift_a_press_selects_up_to_the_pointer() {
        let (mut viewer, directory) = editing(&format!("{NARROW}{FOX}"));
        pages_alone(&mut viewer);
        let now = later();
        let (quick, fox, the) = (
            word_at(&viewer, "quick"),
            word_at(&viewer, "fox"),
            word_at(&viewer, "The"),
        );
        // From the cursor, where nothing is selected.
        viewer.click(quick);
        viewer.pointer_pressed(fox, true, now);
        viewer.pointer_released();
        assert_eq!(mode(&viewer), editor::Mode::Visual);
        assert_eq!(selected(&viewer).as_deref(), Some("quick brown f"));
        // The end it began at stays, whichever side the pointer is on.
        viewer.pointer_pressed(the, true, now);
        assert_eq!(selected(&viewer).as_deref(), Some("The q"));
        // And dragging on from there goes on selecting.
        assert!(viewer.pointer_moved(word_at(&viewer, "jumps")));
        viewer.pointer_released();
        assert_eq!(selected(&viewer).as_deref(), Some("quick brown fox j"));
        // What the keys selected is taken further.
        press(&mut viewer, "<Esc>");
        viewer.click(quick);
        press(&mut viewer, "ve");
        assert_eq!(selected(&viewer).as_deref(), Some("quick"));
        viewer.pointer_pressed(fox, true, now);
        viewer.pointer_released();
        assert_eq!(selected(&viewer).as_deref(), Some("quick brown f"));
        press(&mut viewer, "d");
        assert!(viewer.text().unwrap().contains("The ox jumps"));
        done(directory);
    }

    #[test]
    fn with_shift_lines_of_the_source_stay_lines() {
        let text = "one two three\nfour five six\nseven eight nine\nten\n";
        let (mut viewer, directory) = editing(text);
        let _ = tints(&mut viewer, PAGE_CURSOR);
        press(&mut viewer, "V");
        viewer.pointer_pressed(in_source(&viewer, 2, 3), true, later());
        viewer.pointer_released();
        assert_eq!(mode(&viewer), editor::Mode::VisualLine);
        assert_eq!(
            selected(&viewer).as_deref(),
            Some("one two three\nfour five six\nseven eight nine\n")
        );
        press(&mut viewer, "<Esc>");
        // From where typing was going on: that ends it.
        viewer.click(in_source(&viewer, 1, 5));
        press(&mut viewer, "i");
        viewer.pointer_pressed(in_source(&viewer, 1, 9), true, later());
        viewer.pointer_released();
        assert_eq!(mode(&viewer), editor::Mode::Visual);
        assert_eq!(selected(&viewer).as_deref(), Some("five "));
        press(&mut viewer, "<Esc>i");
        viewer.pointer_pressed(in_source(&viewer, 1, 0), true, later());
        viewer.pointer_released();
        assert_eq!(selected(&viewer).as_deref(), Some("four five"));
        done(directory);
    }

    /// Where in the window `quad` shows byte `offset`, about.
    fn near(quad: &Quad, at: (f64, f64), by: f64) -> bool {
        let (x, y) = (f64::from(quad.x), f64::from(quad.y));
        let (width, height) = (f64::from(quad.width), f64::from(quad.height));
        at.0 >= x - by && at.0 <= x + width + by && at.1 >= y - by && at.1 <= y + height + by
    }

    #[test]
    fn the_wheel_zooms_where_the_pointer_is_and_scrolls_by_pixels() {
        let mut text = "#set page(width: 300pt, height: 300pt)\n".to_owned();
        for page in 1..=4 {
            let _ = write!(text, "Page {page} begins here and goes on.\n\n{FOX}");
            if page < 4 {
                text.push_str("#pagebreak()\n");
            }
        }
        let (mut viewer, directory) = editing(&text);
        // Beside the source, where the pages do not begin at the left of
        // the window, and alone.
        for alone in [false, true] {
            if alone {
                press(&mut viewer, "gsgs");
            }
            // On the second page: the view has room to move every way.
            press(&mut viewer, "gg/wood<CR>n");
            let cursor = tints(&mut viewer, PAGE_CURSOR);
            let pointer = (
                f64::from(cursor[0].x) + f64::from(cursor[0].width) / 2.0,
                f64::from(cursor[0].y) + f64::from(cursor[0].height) / 2.0,
            );
            let zoom = viewer.view.zoom;
            for factor in [1.1, 1.1, 1.1, 1.0 / 1.1, 1.0 / 1.1] {
                viewer.zoom_at(factor, pointer);
                let cursor = tints(&mut viewer, PAGE_CURSOR);
                assert!(near(&cursor[0], pointer, 1.5), "{cursor:?} {pointer:?}");
            }
            assert!(viewer.view.zoom > zoom * 1.05);
            // Pages smaller than the window stay in its middle; up and
            // down, what is under the pointer still stays there.
            viewer.zoom_at(0.5, pointer);
            let cursor = tints(&mut viewer, PAGE_CURSOR);
            let middle = f64::from(cursor[0].y) + f64::from(cursor[0].height) / 2.0;
            assert!((middle - pointer.1).abs() < 3.0, "{cursor:?} {pointer:?}");
            viewer.zoom_at(2.0, pointer);
            // A touchpad scrolls by a pixel or less at a time: it adds
            // up.
            let before = tints(&mut viewer, PAGE_CURSOR)[0];
            for _ in 0..40 {
                viewer.scroll(0.0, 0.25);
            }
            let after = tints(&mut viewer, PAGE_CURSOR)[0];
            assert!((f64::from(before.y - after.y) - 10.0).abs() <= 1.0);
            press(&mut viewer, "=");
        }
        // Over the source the pages are zoomed about their middle.
        press(&mut viewer, "gs");
        assert_eq!(viewer.shown_panes(), Panes::Split);
        let before = viewer.view;
        viewer.zoom_at(1.1, (20.0, 0.0));
        assert!(viewer.view.zoom > before.zoom);
        assert_eq!(viewer.view.center, before.center);
        done(directory);
    }

    #[test]
    fn a_press_while_typing_moves_the_cursor_and_typing_goes_on() {
        let (mut viewer, directory) = editing("Hello there world.\n\nSecond paragraph.\n");
        pages_alone(&mut viewer);
        press(&mut viewer, "iX");
        compile(&mut viewer);
        // The left half of a letter: before it.
        viewer.click(word_at(&viewer, "world"));
        assert_eq!(mode(&viewer), editor::Mode::Insert);
        assert!(from_cursor(&viewer).starts_with("world"));
        assert_eq!(tints(&mut viewer, PAGE_BAR).len(), 1);
        press(&mut viewer, "big ");
        assert_eq!(
            viewer.text(),
            Some("XHello there big world.\n\nSecond paragraph.\n")
        );
        compile(&mut viewer);
        // The right half: behind it.
        let there = viewer.text().unwrap().find("e big").unwrap();
        let behind = glyph_place(
            &viewer,
            true,
            |origin| matches!(origin, Origin::Text { range, .. } if range.start == there),
        );
        viewer.click(behind);
        assert_eq!(mode(&viewer), editor::Mode::Insert);
        press(&mut viewer, "!");
        assert!(viewer.text().unwrap().contains("Hello there! big world."));
        // Each stretch typed is a step of its own for undo.
        press(&mut viewer, "<Esc>u");
        assert!(viewer.text().unwrap().contains("XHello there big world."));
        press(&mut viewer, "u");
        assert!(viewer.text().unwrap().contains("XHello there world."));
        press(&mut viewer, "u");
        assert!(viewer.text().unwrap().starts_with("Hello there world."));
        compile(&mut viewer);
        // Dragging ends typing: what is selected is for commands.
        press(&mut viewer, "i");
        let (there, world) = (word_at(&viewer, "there"), word_at(&viewer, "world"));
        drag(&mut viewer, there, world);
        assert_eq!(mode(&viewer), editor::Mode::Visual);
        assert_eq!(selected(&viewer).as_deref(), Some("there w"));
        press(&mut viewer, "<Esc>");
        // In the source too.
        press(&mut viewer, "gs");
        let _ = tints(&mut viewer, PAGE_CURSOR);
        press(&mut viewer, "i");
        viewer.click(in_source(&viewer, 2, 7));
        assert_eq!(mode(&viewer), editor::Mode::Insert);
        press(&mut viewer, "new ");
        assert!(viewer.text().unwrap().contains("Second new paragraph."));
        done(directory);
    }

    #[test]
    fn a_press_does_nothing_while_a_command_is_typed() {
        let (mut viewer, directory) = editing(&format!("{NARROW}{FOX}"));
        pages_alone(&mut viewer);
        let cursor = editor(&viewer).cursor();
        press(&mut viewer, ":coun");
        let (quick, fox) = (word_at(&viewer, "quick"), word_at(&viewer, "fox"));
        viewer.pointer_pressed(quick, false, later());
        assert!(!viewer.pointer_moved(fox));
        viewer.pointer_released();
        assert_eq!(mode(&viewer), editor::Mode::Command);
        assert_eq!(editor(&viewer).command_line(), Some("coun"));
        assert_eq!(editor(&viewer).cursor(), cursor);
        // A command half typed in normal mode is forgotten: the `d` is
        // not waiting for the `w` any more.
        press(&mut viewer, "<Esc>d");
        viewer.click(word_at(&viewer, "quick"));
        press(&mut viewer, "w");
        assert!(viewer.text().unwrap().contains("The quick brown fox"));
        assert!(from_cursor(&viewer).starts_with("brown"));
        done(directory);
    }

    /// Where in the window the list drawn last sets off the entry that
    /// is picked out in it: the middle of that row, found in the pixels
    /// of its panel.
    fn picked_row(viewer: &mut Viewer) -> (f64, f64) {
        let scene = viewer.scene(&|_| false);
        let panel = *scene.quads.last().unwrap();
        let image = scene.images.iter().find(|image| image.id == panel.texture);
        let pixmap = &image.unwrap().pixmap;
        // The colour `chrome::list_markup` lays under the row.
        let set_off = |x: u32, y: u32| {
            let pixel = pixmap.pixel(x, y).unwrap();
            (pixel.red(), pixel.green(), pixel.blue()) == (0x50, 0x49, 0x45)
        };
        let rows: Vec<u32> = (0..pixmap.height())
            .filter(|y| (0..pixmap.width()).any(|x| set_off(x, *y)))
            .collect();
        let (first, last) = (*rows.first().unwrap(), *rows.last().unwrap());
        assert!(last - first > 8, "a row of text: {first} to {last}");
        (
            f64::from(panel.x) + f64::from(panel.width) / 2.0,
            f64::from(panel.y) + f64::from(first + last) / 2.0,
        )
    }

    #[test]
    fn a_press_on_an_entry_of_a_list_of_places_goes_there() {
        let mut text = String::new();
        for heading in 1..=14 {
            let _ = write!(text, "= Heading {heading}\n\nText of {heading}.\n\n");
        }
        let (mut viewer, directory) = editing(&text);
        let cursor = editor(&viewer).cursor();
        press(&mut viewer, "go<Down><Down>");
        assert!(viewer.is_picking());
        let third = picked_row(&mut viewer);
        press(&mut viewer, "<Down><Down>");
        let fifth = picked_row(&mut viewer);
        let pitch = (fifth.1 - third.1) / 2.0;
        assert!(pitch > 10.0);
        // Beside the list, on its title and under its last row: nothing,
        // the list is still there and waits for keys.
        for beside in [
            (5.0, third.1),
            (third.0, third.1 - 3.0 * pitch),
            (third.0, third.1 + 7.0 * pitch),
        ] {
            viewer.click(beside);
            assert!(viewer.is_picking(), "{beside:?}");
            assert_eq!(editor(&viewer).cursor(), cursor);
        }
        // On the row of the third entry, with the fifth picked out.
        viewer.click(third);
        assert!(!viewer.is_picking());
        assert!(from_cursor(&viewer).starts_with("= Heading 3\n"));
        // The list comes up with the heading the cursor is under picked
        // out. Further down, its rows show other entries: the one in
        // the middle is the ninth, the one two above it the seventh.
        press(
            &mut viewer,
            "gggo<Down><Down><Down><Down><Down><Down><Down><Down>",
        );
        let middle = picked_row(&mut viewer);
        assert!((middle.1 - fifth.1).abs() < 1.0);
        viewer.click((middle.0, middle.1 - 2.0 * pitch));
        assert!(!viewer.is_picking());
        assert!(from_cursor(&viewer).starts_with("= Heading 7\n"));
        // The very edges of a row are the row still.
        press(&mut viewer, "gggo");
        let first = picked_row(&mut viewer);
        viewer.click((first.0, first.1 + pitch * 1.45));
        assert!(from_cursor(&viewer).starts_with("= Heading 2\n"));
        press(&mut viewer, "gggo");
        let _ = picked_row(&mut viewer);
        viewer.click((first.0, first.1 + pitch * 1.55));
        assert!(from_cursor(&viewer).starts_with("= Heading 3\n"));
        done(directory);
    }

    #[test]
    fn a_press_on_what_could_be_typed_types_it() {
        // By the keys, for what it should come to.
        let (mut viewer, directory) = editing("Some text here.\n\n");
        press(&mut viewer, "Go#<C-n><C-n><Tab>");
        let by_keys = viewer.text().unwrap().to_owned();
        assert!(by_keys.contains('$'), "{by_keys}");
        done(directory);

        let (mut viewer, directory) = editing("Some text here.\n\n");
        press(&mut viewer, "Go#<C-n><C-n>");
        let third = picked_row(&mut viewer);
        press(&mut viewer, "<C-p><C-p>");
        let first = picked_row(&mut viewer);
        assert!(first.1 < third.1);
        let _ = viewer.take_edited();
        viewer.click(third);
        assert_eq!(viewer.text(), Some(by_keys.as_str()));
        assert_eq!(mode(&viewer), editor::Mode::Insert);
        assert!(viewer.completion.is_none());
        // The text changed: the window has to have it compiled.
        assert!(viewer.take_edited().is_some());
        // A press beside the list is for the text, and the list goes.
        press(&mut viewer, "<Esc>Go#");
        let _ = picked_row(&mut viewer);
        assert!(viewer.completion.is_some());
        viewer.click(in_source(&viewer, 0, 5));
        assert!(viewer.completion.is_none());
        assert_eq!(mode(&viewer), editor::Mode::Insert);
        assert!(from_cursor(&viewer).starts_with("text here."));
        done(directory);
    }
}
