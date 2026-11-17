//! The state of one zetwerk window, and what it should show.
//!
//! [`Viewer`] knows nothing about windows, GPUs or threads. It is told what
//! happened (a key, a finished compile, a resize) and asked what the frame
//! should look like. That keeps the whole behaviour of the program testable
//! without opening a window.

mod again;
pub mod agent;
mod aids;
mod assist;
mod buffers;
mod cells;
pub mod export;
mod figures;
mod find;
mod focus;
mod hints;
mod history;
mod inplace;
mod jump;
mod keymap;
mod language;
mod local;
mod manager;
mod margin;
pub mod notes;
pub(crate) mod options;
mod palette;
mod pointer;
pub mod refer;
mod refused;
mod registers;
mod saving;
mod set;
mod slides;
mod sources;
mod sources_add;
mod sources_broken;
mod sources_look;
mod sources_proposed;
mod sources_undo;
pub mod start;
mod structure;
mod styles;
mod vim;
mod waiting;
mod written;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use crate::typst_text::{EMPTY, Ground, empty_places};
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use tiny_skia::{Color, Pixmap};
use typst::World;
use typst::layout::Rect;
use typst::syntax::{FileId, LinkedNode, Side, Source, SyntaxKind};
use typst::utils::hash128;
use typst_layout::{Page, PagedDocument};
use zetwerk_core::{DiskError, DiskStatus, Opened};
use zetwerk_predict::{Replacement, predict};
use zetwerk_render::{PixelRegion, Tile, TileKey, page_size_px, tile_origins, tiles};
use zetwerk_typst::{CompileOutput, Environment, PageLines, PageMap, Project, ProjectWorld};
use zetwerk_vim::{Effect, Seek, Shown as ShownLine};

use self::agent::{AgentEdits, Mark, Proposal};
pub use self::keymap::Keymap;
pub use self::language::{Check, Issue, Report};
pub use self::slides::Paging;
use crate::chrome::{self, Chrome, Image, ROW_HEIGHT, RowText, STATUS_HEIGHT, Status};
use crate::editor::{self, Editor, Rows, Unseen};
use crate::gpu::{Quad, TextureId};
use crate::keys::{Key, KeyPress};
use crate::view::{Layout, MAX_ZOOM, MIN_ZOOM, Viewport};

/// The colour around the pages.
const BACKDROP: [u8; 3] = [0x1d, 0x20, 0x21];
/// What a page looks like before its tiles have been rasterized.
const PAPER: [u8; 3] = [0xff, 0xff, 0xff];

/// How far `j` and `k` scroll, in logical pixels.
const LINE_SCROLL: f64 = 60.0;
/// How much of the previous view stays visible when paging.
const PAGE_OVERLAP: f64 = 40.0;
/// Zoom change per `+` or `-`.
const ZOOM_STEP: f64 = 1.25;
/// Distance of overlay panels from the window edge, in logical pixels.
const PANEL_MARGIN: f64 = 12.0;

/// The colour behind the source text.
const PANE: [u8; 3] = [0x28, 0x28, 0x28];
/// The colour behind selected source text.
const SELECTION: [u8; 3] = [0x50, 0x49, 0x45];
/// The colour of the cursor.
const CURSOR: [u8; 3] = [0x7c, 0x6f, 0x64];
/// The cursor on a page: a dark block over the character it is on, and
/// in insert mode a bar before it. Colours are RGBA.
const PAGE_CURSOR: [u8; 4] = [0x28, 0x28, 0x28, 0x66];
const PAGE_BAR: [u8; 4] = [0x28, 0x28, 0x28, 0xff];
/// The colour laid over selected text on a page.
const PAGE_SELECTION: [u8; 4] = [0x45, 0x85, 0x88, 0x55];
/// How many rows stay visible beyond the cursor when the source scrolls.
const SCROLL_MARGIN: usize = 3;
/// How near the bottom of the view the top of the cursor may be put by
/// pages that come, in points, before the view goes after it: about the
/// height of a line.
const LANDED_MARGIN: f64 = 16.0;
/// A change an agent proposes, in the source and on the page; one whose
/// text has changed since; and what an agent changed last.
const PROPOSAL: [u8; 3] = [0x5c, 0x4a, 0x14];
const PAGE_PROPOSAL: [u8; 4] = [0xd7, 0x99, 0x21, 0x60];
const STALE_PROPOSAL: [u8; 3] = [0x5c, 0x26, 0x22];
const PAGE_STALE_PROPOSAL: [u8; 4] = [0xcc, 0x24, 0x1d, 0x45];
const AGENT_MARK: [u8; 3] = [0x3c, 0x42, 0x1c];
/// What the compiler complains about, in the source.
const ERROR_MARK: [u8; 3] = [0x6b, 0x1f, 0x1b];
const WARNING_MARK: [u8; 3] = [0x5a, 0x45, 0x10];
const PAGE_AGENT_MARK: [u8; 4] = [0x98, 0x97, 0x1a, 0x50];
/// How many lines of what goes and of what comes the panel of a proposal
/// shows, and how many characters of each.
const PROPOSAL_LINES: usize = 8;
const PROPOSAL_COLUMNS: usize = 90;
/// The lens: its edge, how many rows and cells of source it shows at
/// most, how many cells wide it is at least, and its padding and its
/// distance from the cursor's line in points.
const LENS_EDGE: [u8; 3] = [0x66, 0x5c, 0x54];
const LENS_ROWS: usize = 7;
const LENS_COLUMNS: usize = 72;
const LENS_MIN_CELLS: f64 = 12.0;
const LENS_PADDING: f64 = 5.0;
const LENS_GAP: f64 = 4.0;

/// What the window shows of a document that is being edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panes {
    /// The source on the left, the pages on the right.
    Split,
    Source,
    /// Only the pages. Keys then move through the pages, not the text.
    Page,
}

/// What a key that changed the text means for compiling it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edited {
    /// Text is being typed, and the page shows it already or a compile
    /// would take long: compile once typing pauses.
    Typing,
    /// Text is being typed that the page cannot show without a compile,
    /// and a compile is quick.
    Unpredicted,
    /// A command changed the text.
    Command,
    /// Typing ended with changes not yet compiled.
    LeftInsert,
    /// An agent changed the text, or a change it proposed was accepted.
    Agent,
}

/// Whether what is before byte `cursor` of `text` has just begun
/// something that stands by itself: a paragraph, a heading, an item of a
/// list, with not more than its first character there.
fn begins_block(text: &str, cursor: usize) -> bool {
    let Some(before) = text.get(..cursor) else {
        return false;
    };
    let line = before.rfind('\n').map_or(0, |at| at + 1);
    let begun = before[line..].trim_start_matches(['=', '-', '+', '/', ' ']);
    begun.chars().count() <= 1
}

/// How small and how large the interface can be made, in times its
/// usual size, and by how much `:ui +` makes it larger.
const UI_SMALLEST: f64 = 0.5;
const UI_LARGEST: f64 = 4.0;
const UI_STEP: f64 = 1.15;

/// What is typeset where the cursor is in a place with nothing in it:
/// a line of text as high as any, of no width, that shows nothing.
const CURSOR_MARK: &str = "#box(width: 0pt, clip: true)[|]";

/// How wide a blank is, in parts of the height of its line: a quarter
/// of the font's size, as in most fonts for text.
const BLANK: f64 = 0.21;

/// How many blanks stand right before byte `cursor` of `text`.
fn blanks_before(text: &str, cursor: usize) -> usize {
    let before = text.get(..cursor).unwrap_or("");
    before.len() - before.trim_end_matches(' ').len()
}

/// Where the cursor of `editor` stands on a page if blanks are before
/// it: behind what is before the blanks, and how many they are.
fn caret_before_blanks(
    map: &PageMap,
    change: &Change,
    editor: &Editor,
) -> Option<(zetwerk_typst::Caret, usize)> {
    let typed = blanks_before(editor.text(), editor.cursor());
    let at = change.back_or_edge(editor.cursor() - typed);
    let behind = map.caret(editor.file(), at);
    behind
        .filter(|caret| typed > 0 && caret.after)
        .map(|caret| (caret, typed))
}

/// How many words of equations are remembered as names or none.
const NAMES_KEPT: usize = 2000;

/// A compile shorter than this (in milliseconds) is run for every key
/// that the page cannot show without one.
const QUICK_COMPILE_MS: f64 = 40.0;

/// A page as it should look with what was typed since the last compile,
/// worked out without compiling. It is shown until the compile replaces it.
struct Predicted {
    /// Which page, counting from 0.
    index: usize,
    page: Page,
    /// The cell of the typed text on the page, and where in the source it
    /// ends: with the cursor there, the cursor is shown behind the cell.
    typed: Rect,
    end: usize,
}

/// How the text differs from the text the pages were compiled from.
enum Difference<'a> {
    Nothing,
    /// `removed` bytes from byte `offset` on were replaced by `text`, and
    /// nothing else changed.
    Replacement {
        offset: usize,
        removed: usize,
        text: &'a str,
    },
}

/// Compares `new` with `old`: what lies between the start they have in
/// common and the end they have in common.
fn difference<'a>(old: &str, new: &'a str) -> Difference<'a> {
    if old == new {
        return Difference::Nothing;
    }
    let mut prefix = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !(old.is_char_boundary(prefix) && new.is_char_boundary(prefix)) {
        prefix -= 1;
    }
    let (old_rest, new_rest) = (&old[prefix..], &new[prefix..]);
    let mut suffix = old_rest
        .bytes()
        .rev()
        .zip(new_rest.bytes().rev())
        .take_while(|(a, b)| a == b)
        .count();
    while !(old_rest.is_char_boundary(old_rest.len() - suffix)
        && new_rest.is_char_boundary(new_rest.len() - suffix))
    {
        suffix -= 1;
    }
    Difference::Replacement {
        offset: prefix,
        removed: old_rest.len() - suffix,
        text: &new_rest[..new_rest.len() - suffix],
    }
}

/// How the text differs from the text the pages were compiled from, if by
/// one replacement: what the places of the one are in the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Change {
    offset: usize,
    removed: usize,
    inserted: usize,
}

impl Change {
    /// No difference.
    const NONE: Self = Self {
        offset: 0,
        removed: 0,
        inserted: 0,
    };

    /// Where byte `now` of the text was in the compiled text. `None` if it
    /// was not: it is part of what was put in.
    fn back(self, now: usize) -> Option<usize> {
        if now < self.offset {
            Some(now)
        } else if now >= self.offset + self.inserted {
            Some(now - self.inserted + self.removed)
        } else {
            None
        }
    }

    /// Like [`Change::back`], with what was put in counting as the place
    /// it was put in at.
    fn back_or_edge(self, now: usize) -> usize {
        self.back(now).unwrap_or(self.offset)
    }

    /// Where byte `then` of the compiled text is now; what was taken out
    /// is where it was taken out.
    fn forth(self, then: usize) -> usize {
        if then < self.offset {
            then
        } else if then >= self.offset + self.removed {
            then - self.removed + self.inserted
        } else {
            self.offset
        }
    }
}

/// The equation byte `offset` of `source` is in, the outermost if there
/// are several, dollar signs and all.
fn equation_at(source: &Source, offset: usize) -> Option<Range<usize>> {
    let last = source.text().len().saturating_sub(1);
    let mut node = LinkedNode::new(source.root()).leaf_at(offset.min(last), Side::After);
    let mut outermost = None;
    while let Some(here) = node {
        if here.kind() == SyntaxKind::Equation {
            outermost = Some(here.range());
        }
        node = here.parent().cloned();
    }
    outermost
}

/// The part of `source` the lens shows for a cursor at `offset`: the whole
/// lines of the equation or raw block it is in, or else of the innermost
/// piece of code, or else its line.
fn lens_block(source: &Source, offset: usize) -> Range<usize> {
    let text = source.text();
    let mut block = offset..offset;
    let mut node = LinkedNode::new(source.root()).leaf_at(offset, Side::After);
    let mut code = None;
    while let Some(at) = node {
        match at.kind() {
            SyntaxKind::Equation | SyntaxKind::Raw => block = at.range(),
            SyntaxKind::FuncCall
            | SyntaxKind::SetRule
            | SyntaxKind::ShowRule
            | SyntaxKind::LetBinding
            | SyntaxKind::CodeBlock
                if code.is_none() =>
            {
                code = Some(at.range());
            }
            _ => {}
        }
        node = at.parent().cloned();
    }
    if block.is_empty() {
        block = code.unwrap_or(block);
    }
    let start = text[..block.start].rfind('\n').map_or(0, |at| at + 1);
    let end = text[block.end..]
        .find('\n')
        .map_or(text.len(), |at| block.end + at);
    start..end
}

/// Where to put a lens of `size` for a cursor at `anchor` (how far across
/// it is, its top and its bottom), inside `within` (left, top, right,
/// bottom): `gap` under the cursor's line; above it where there is no
/// room below; and in the bottom-left corner where the cursor is on
/// nothing the page shows.
fn lens_place(
    anchor: Option<(f64, f64, f64)>,
    (width, height): (f64, f64),
    (left, top, right, bottom): (f64, f64, f64, f64),
    gap: f64,
) -> (f64, f64) {
    let (x, y) = match anchor {
        Some((x, _, below)) if below + gap + height <= bottom => (x, below + gap),
        Some((x, above, _)) => (x, above - gap - height),
        None => (left, bottom - height),
    };
    (
        x.min(right - width).max(left).round(),
        y.min(bottom - height).max(top).round(),
    )
}

/// More than this many bytes taken out or put in since the last compile
/// are not worked out on the page: that is no longer typing.
const MAX_PREDICTED: usize = 400;

/// Whether bytes `offset..offset + len` of `source` are plain text of a
/// paragraph: nothing Typst reads as markup, math or code. Only such text
/// looks on the page like what was typed.
fn is_plain_text(source: &Source, offset: usize, len: usize) -> bool {
    LinkedNode::new(source.root())
        .leaf_at(offset, Side::After)
        .is_some_and(|leaf| {
            leaf.kind() == SyntaxKind::Text
                && leaf.range().start <= offset
                && leaf.range().end >= offset + len
        })
}

/// What the status line says instead of the file name.
struct Message {
    text: String,
    /// Whether it reports a problem.
    alert: bool,
}

/// A summary of the text being edited for the outside world.
#[derive(Debug, Clone, PartialEq)]
pub struct Editing {
    pub mode: editor::Mode,
    /// The cursor's line and column, counting from 1.
    pub line: usize,
    pub column: usize,
    /// Whether the text differs from the file.
    pub modified: bool,
    /// The text on the command line, while it is open, and what it is
    /// for: `:` for a command, `/` or `?` for a search.
    pub command: Option<String>,
    pub prompt: char,
    /// The register a macro is being recorded into, if one is.
    pub recording: Option<char>,
    /// Whether typing replaces the characters under the cursor (`R`).
    pub replacing: bool,
}

/// Whether keys are commands or text for the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    /// The command line is open and holds this text.
    Command(String),
}

/// Measurements shown in the performance panel.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Stats {
    /// Duration of the last compile.
    pub compile_ms: f64,
    pub compiles: u64,
    /// Tiles rasterized since the window opened.
    pub tiles: u64,
    /// Time spent building and submitting the last frame.
    pub frame_ms: f64,
    pub frames: u64,
    /// Time from the last input event to its frame being submitted.
    pub latency_ms: f64,
    pub texture_bytes: u64,
    /// Changes shown on the page without compiling, and changes that had
    /// to wait for a compile.
    pub predicted: u64,
    pub declined: u64,
    /// Changes shown by typesetting only the part that changed, and how
    /// long the last of them took, in milliseconds.
    pub local: u64,
    pub local_ms: f64,
}

/// A summary of the viewer for the outside world.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub file: String,
    pub pages: usize,
    /// The page at the middle of the view, counting from 1; 0 if there is
    /// no document.
    pub page: usize,
    /// Pixels per point.
    pub zoom: f64,
    pub revision: u64,
    /// The revision the pages on screen were compiled from.
    pub compiled_revision: Option<u64>,
    pub mode: Mode,
    /// Errors of the last compile; empty if it succeeded.
    pub problems: Vec<String>,
    pub performance_panel: bool,
    /// The text being edited; `None` for a window that only shows pages.
    pub editing: Option<Editing>,
}

/// What to draw, and what is still missing to draw it properly.
#[derive(Default)]
pub struct Scene {
    /// Images the compositor does not hold yet. Upload before drawing.
    pub images: Vec<Image>,
    pub quads: Vec<Quad>,
    /// Page tiles that are visible but not rasterized, most important first.
    pub wanted: Vec<Tile>,
}

/// The tiles last drawn completely for a page, kept as a stand-in while new
/// ones are rasterized after a zoom step or a recompile.
struct Shown {
    zoom: f64,
    tiles: Vec<(TileKey, PixelRegion)>,
}

/// The lines of the typeset pages, for a cursor that moves up and down on
/// them.
struct PageRows<'a> {
    document: &'a PagedDocument,
    world: &'a ProjectWorld,
    file: FileId,
    lines: &'a RefCell<HashMap<usize, PageLines>>,
    /// The page the cursor was last found on.
    cursor_page: &'a Cell<usize>,
    /// How the text differs from the text the pages were compiled from.
    change: Change,
    /// The page, the line, and the file and byte of the compiled text
    /// the cursor last came to here. What a call put on the page is many
    /// lines for one place in the source: the place alone does not say
    /// which.
    landed: &'a Cell<Option<Landed>>,
    /// Where a motion led that is in another file than the one edited.
    elsewhere: &'a Cell<Option<(FileId, usize)>>,
}

/// A page, a line of it, and the file and byte shown there.
type Landed = (usize, usize, FileId, usize);

/// How many pages on are looked at for the nearest place for the cursor.
const NEAR_PAGES: usize = 4;

/// How many pages either way of the cursor's are looked through for the
/// text a prediction is made for.
const PREDICTED_NEAR: usize = 2;

impl PageRows<'_> {
    /// The page that shows byte `then` of the compiled text of the file,
    /// or the text nearest to it.
    fn page_of(&self, then: usize) -> Option<usize> {
        // On the page it was last found on, if that shows it: the same
        // text may be on many pages (a heading and the table of
        // contents), and what a call made is found by no search. Then
        // the pages beside it: a cursor seldom goes far.
        let near = self.cursor_page.get();
        if self.on(near, |lines| lines.shows(self.file, then)) == Some(true) {
            return Some(near);
        }
        let beside = [Some(near), near.checked_add(1), near.checked_sub(1)];
        let covering = beside
            .into_iter()
            .flatten()
            .find(|page| self.on(*page, |lines| lines.covers(self.file, then)) == Some(true));
        if covering.is_some() {
            return covering;
        }
        let source = self.world.source(self.file).ok()?;
        let then = self.world.shown_at(self.file, then);
        zetwerk_typst::locate(self.document, &source, then, near).map(|position| position.page)
    }

    /// Runs `ask` on the lines of page `index`.
    fn on<T>(&self, index: usize, ask: impl FnOnce(&PageLines) -> T) -> Option<T> {
        let page = self.document.pages().get(index)?;
        let mut lines = self.lines.borrow_mut();
        let lines = lines
            .entry(index)
            .or_insert_with(|| PageMap::build(self.world, &page.frame).lines());
        Some(ask(lines))
    }
}

/// A line of a page and where on it a cursor aims.
struct Place {
    page: usize,
    line: usize,
    across: f64,
}

impl PageRows<'_> {
    /// Moves `place` a line down (up, if not `down`), to the next page
    /// with text on it where its page ends. Returns whether there was a
    /// line to go to.
    fn step(&self, place: &mut Place, down: bool) -> bool {
        let Place { page, line, across } = *place;
        if let Some(Some((line, across))) = self.on(page, |lines| lines.step(line, down, across)) {
            (place.line, place.across) = (line, across);
            return true;
        }
        let mut other = page;
        loop {
            other = if down {
                other + 1
            } else if let Some(before) = other.checked_sub(1) {
                before
            } else {
                return false;
            };
            match self.on(other, |lines| lines.enter(down)) {
                Some(Some(first)) => {
                    // From the last column of a page to the first of the
                    // next, the cursor stays as far into the column as it
                    // was; down a single column it goes straight on.
                    let extents = self
                        .on(page, |lines| lines.extent(line))
                        .flatten()
                        .zip(self.on(other, |lines| lines.extent(first)).flatten());
                    if let Some((old, new)) = extents
                        && (new.0 > old.1 || new.1 < old.0)
                    {
                        place.across += new.0 - old.0;
                    }
                    (place.page, place.line) = (other, first);
                    return true;
                }
                // A page with no text of the file on it.
                Some(None) => {}
                None => return false,
            }
        }
    }
}

impl Rows for PageRows<'_> {
    fn stop(&self, at: usize, seek: Seek) -> Result<Option<Range<usize>>, Unseen> {
        let file = self.file;
        let then = self.change.back(at).ok_or(Unseen)?;
        let Some(page) = self.page_of(then) else {
            return Ok(None);
        };
        let found = match seek {
            Seek::At => self.on(page, |lines| lines.stop_at(file, then)).flatten(),
            Seek::Next | Seek::Previous => {
                let forward = seek == Seek::Next;
                self.on(page, |lines| lines.stop_beside(file, then, forward))
                    .flatten()
            }
            Seek::After | Seek::Before => {
                // On that page, or else on the next ones that way.
                let forward = seek == Seek::After;
                let mut page = Some(page);
                let mut found = None;
                for _ in 0..NEAR_PAGES {
                    let Some(here) = page else {
                        break;
                    };
                    let Some(stop) = self.on(here, |lines| lines.stop_near(file, then, forward))
                    else {
                        break;
                    };
                    found = stop;
                    if found.is_some() {
                        break;
                    }
                    page = if forward {
                        here.checked_add(1)
                    } else {
                        here.checked_sub(1)
                    };
                }
                found
            }
        };
        // Where it is now: its start has moved with the change, its
        // length is its own.
        Ok(found.map(|stop| {
            let start = self.change.forth(stop.start);
            start..start + stop.len()
        }))
    }

    fn line(&self, at: usize) -> Option<ShownLine> {
        let then = self.change.back(at)?;
        let page = self.page_of(then)?;
        let (start, last, end) = self.on(page, |lines| lines.line_span(self.file, then))??;
        let forth = |then: usize| self.change.forth(then);
        let (start, last, end) = (forth(start), forth(last), forth(end));
        // The last place of a line may be an empty one, which shows no
        // bytes: it ends where it begins.
        (start <= last && last <= end).then_some(ShownLine { start, last, end })
    }

    fn row(&self, from: usize, rows: isize, across: Option<f64>) -> Option<(usize, f64)> {
        let file = self.file;
        let from = self.change.back_or_edge(from);
        let (page, line, shown_at) = match self.landed.get() {
            Some((page, line, of, at)) if (of, at) == (file, from) => (
                page,
                line,
                self.on(page, |lines| lines.across(line, file, from))??,
            ),
            _ => {
                let page = self.page_of(from)?;
                let (line, shown_at) = self.on(page, |lines| lines.find(file, from))??;
                (page, line, shown_at)
            }
        };
        let across = across.unwrap_or(shown_at);
        let down = rows > 0;
        let mut place = Place { page, line, across };
        let stay = Some((self.change.forth(from), across));
        for row in 0..rows.unsigned_abs() {
            if !self.step(&mut place, down) {
                if row == 0 {
                    // The first or the last line there is.
                    return stay;
                }
                break;
            }
        }
        // The cursor has to come to something else than it is on. Where a
        // line shows nothing else (it is more of what one call made, all
        // of which is one place in the source), it goes on.
        let (of, to) = loop {
            let Place { page, line, across } = place;
            if let Some(to) = self.on(page, |lines| lines.at(line, across, (file, from)))? {
                break to;
            }
            if !self.step(&mut place, down) {
                return stay;
            }
        };
        let Place { page, line, across } = place;
        self.cursor_page.set(page);
        self.landed.set(Some((page, line, of, to)));
        if of != file {
            // The line is of another file of the document (an included
            // chapter): the cursor cannot go there, the window can.
            self.elsewhere.set(Some((of, to)));
            return stay;
        }
        Some((self.change.forth(to), across))
    }
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "independent switches of the interface, not states of one thing"
)]
pub struct Viewer {
    chrome: Chrome,
    /// The equation that is shown as it is written, if one is (see
    /// `written`).
    written: Option<written::Written>,
    /// The revision of the text at which an equation was last shown as
    /// written because Typst refused it: once for each.
    written_tried: Option<u64>,
    /// Where the other equations begin that Typst refused and that are
    /// shown as written too: with two refused at once, showing one so
    /// leaves the document refused for the other.
    written_too: Vec<usize>,
    /// Whether the lines of the pages say how far they are from the
    /// cursor (see `aids`), and what they say, for as long as the cursor
    /// and the pages are the same.
    numbers: bool,
    numbered: Option<aids::NumberedFor>,
    /// The labels to jump by, while they are up (see `jump`).
    jump: jump::Jump,
    /// The frame around what typing goes into and the words for empty
    /// places (see `focus`): whether they are drawn, and what they are.
    focus: focus::Focus,
    /// Whether what changed is typeset by itself and put into the page
    /// (see `local`): on, unless the user turned it off (`:parts`).
    in_parts: bool,
    /// What the user wants to be able to put into running text, besides
    /// what everybody can (see `typst_text::TEXT_INSERTS`).
    inserts: Vec<crate::typst_text::Insert>,
    /// What was put into the text to show it, the last time it was
    /// handed over to be typeset (see [`Viewer::put_to_show`]).
    shown_put: Vec<(usize, &'static str)>,
    /// Whether words typed in equations name something, as far as that
    /// was asked.
    names: HashMap<String, bool>,
    project: Project,
    /// The main file's text, if the window edits it and does not merely
    /// show what is on disk.
    editor: Option<Editor>,
    panes: Panes,
    /// What the last keys mean for compiling, until the window asks.
    edited: Option<Edited>,
    /// The source pane's width in cells and height in rows, as last drawn.
    source_size: (usize, usize),
    /// The width of its line numbers, of a cell and the height of a row,
    /// in device pixels, as last drawn.
    source_metrics: (f64, f64, f64),
    file: String,
    revision: u64,
    compiled_revision: Option<u64>,
    /// The last document that compiled. Stays on screen while a newer
    /// revision has errors.
    document: Option<Arc<PagedDocument>>,
    /// The last document that was typeset whole. What is asked about
    /// the document as a whole (its labels, what a name stands for) is
    /// asked of it: `document` may be it with a part typeset anew, a
    /// new thing with every key, and asking that costs a look at all of
    /// its pages.
    whole: Option<Arc<PagedDocument>>,
    /// The sources that document was compiled from, and for the pages
    /// that have been asked about, where their glyphs come from in them.
    compiled_world: Option<Arc<ProjectWorld>>,
    maps: HashMap<usize, PageMap>,
    /// The lines of text of the pages the cursor has moved up or down on.
    page_lines: RefCell<HashMap<usize, PageLines>>,
    /// The page the cursor was last found on: where to look for it first.
    cursor_page: Cell<usize>,
    /// See [`PageRows::landed`] and [`PageRows::elsewhere`].
    landed: Cell<Option<Landed>>,
    elsewhere: Cell<Option<(FileId, usize)>>,
    /// The files that are open and not the one being edited, the one
    /// edited last first.
    others: Vec<Editor>,
    /// Where journals are kept, for opening more files.
    state_dir: Option<std::path::PathBuf>,
    /// The files whose text has changed since the last compile.
    stale_files: Vec<FileId>,
    /// The files that are open, in the order they were opened.
    opened: Vec<FileId>,
    /// Whether the pages are scrolled over or shown one at a time; with
    /// the latter, which one is shown, and how far the view was pushed
    /// towards the next.
    paging: Paging,
    slide: usize,
    pushed: f64,
    /// The overview of all pages, while it is up.
    grid: Option<slides::Grid>,
    /// Whether the page, as last drawn, shows the equation the cursor is
    /// in typeset, so that its source is not to be seen there.
    equation_set: bool,
    /// The user's own keys, and the keys typed that may yet become one
    /// of them.
    keymap: Keymap,
    typed: Vec<KeyPress>,
    /// When the keys typed are taken as they are, and how to take back
    /// those of them typed already (see `keymap`).
    mapping: keymap::Waiting,
    /// What Vim's keys that the window carries out keep (see `vim`).
    vim: vim::Vim,
    /// The revision that document was compiled from. Differs from
    /// `compiled_revision` while the newest text has errors.
    document_revision: Option<u64>,
    predicted: Option<Predicted>,
    /// How the text of a revision differs from the compiled text, where
    /// that is one replacement.
    changed: Option<(u64, FileId, Change)>,
    /// Where the cursor is on the pages as last drawn, in device pixels:
    /// how far across, and its top and bottom.
    cursor_at: Option<(f64, f64, f64)>,
    /// Where in the compiled text the last insertion was worked out for,
    /// and the page it is on.
    predicted_at: Option<(usize, usize)>,
    /// Why the last change could not be shown without compiling.
    declined: Option<String>,
    problems: Vec<String>,
    /// What the last compile complained about in the project's files: the
    /// file and the bytes (of the text it compiled), whether it is an
    /// error, and the complaint.
    diagnostics: Vec<(FileId, Range<usize>, bool, String)>,
    layout: Layout,
    view: Viewport,
    window: (u32, u32),
    /// Pixels per point of everything that is not a page: the status
    /// line, the lists, the source. It is `display` times `ui`.
    scale: f64,
    /// Device pixels per logical pixel, as the display has it.
    display: f64,
    /// How much larger than usual the user wants all that is not a page.
    ui: f64,
    /// Whether the zoom follows the window width.
    fit: bool,
    mode: Mode,
    /// A `g` waiting for its second key.
    pending_g: bool,
    message: Option<Message>,
    performance_panel: bool,
    quit: bool,
    /// Whether agents change the text or propose changes to it.
    agent_edits: AgentEdits,
    /// The changes agents have proposed, in the order of the text; the
    /// number the next one gets; and the text their places are places of.
    proposals: Vec<Proposal>,
    /// And those for open files that are not being edited.
    proposals_elsewhere: Vec<Proposal>,
    next_proposal: u64,
    proposal_text: Option<String>,
    /// What an agent changed last, until the next key: shown, so that the
    /// user sees what was done.
    agent_mark: Option<Range<usize>>,
    /// The list of what could be typed at the cursor, while it is up.
    completion: Option<assist::Completion>,
    /// What Typst says the thing under the cursor is, until the next key.
    tooltip: Option<String>,
    /// The aids that say what is coming (see [`hints`]).
    hints: hints::Hints,
    /// A list of places to go that keys are for, while it is up.
    picker: Option<assist::Picker>,
    /// What the list of the text's history is showing, while that is the
    /// list that is up.
    history: Option<history::Panel>,
    /// What the reference helper keeps between keys (see [`refer`]).
    refer: refer::Refer,
    /// What the bibliography manager keeps between commands.
    sources_kept: sources_look::Kept,
    /// The register `"+`, and the system clipboard it stands for.
    clipboard: registers::Clipboard,
    /// What is known about the spelling and grammar of the text.
    language: language::Language,
    /// The notes on the text, and what reviewing keeps (see [`notes`]).
    notes: notes::Notes,
    /// What is beside the pages: tags for what they show as nothing, and
    /// the compiler's complaints (see [`margin`]).
    margin: margin::Margin,
    /// What is kept of a text Typst refuses (see [`refused`]).
    refused: refused::Refused,
    /// Where the cursor is drawn in the source pane or the lens, in device
    /// pixels: how far across, and its top and bottom.
    caret_at: Option<(f64, f64, f64)>,
    /// Tiles of the current document at the current zoom, by page and tile
    /// origin, so that a frame does not crop and hash pages again.
    planned: HashMap<(usize, u32, u32), Tile>,
    shown: HashMap<usize, Shown>,
    /// The export that waits or is being written, and what writes it.
    exports: export::Exports,
    /// What the pointer is doing (see [`pointer`]).
    pointer: pointer::Pointer,
    /// What is kept about beginnings (see [`start`]).
    start: start::Start,
    /// The matches of a search and what keys took (see [`find`]).
    find: find::Find,
    /// What the window waits for (see [`waiting`]).
    waiting: waiting::Waiting,
    /// The user's settings (see [`set`]).
    settings: set::Settings,
    pub stats: Stats,
}

impl Viewer {
    /// A viewer for `project` in a window of `window` device pixels.
    #[must_use]
    #[allow(clippy::too_many_lines, reason = "a line for every field")]
    pub fn new(
        env: Arc<Environment>,
        project: Project,
        window: (u32, u32),
        scale: f64,
    ) -> Option<Self> {
        let file = project.main_path().and_then(Path::file_name).map_or_else(
            || "[no file]".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        let mut viewer = Self {
            chrome: Chrome::new(env)?,
            written: None,
            written_tried: None,
            written_too: Vec::new(),
            numbers: true,
            numbered: None,
            jump: jump::Jump::default(),
            focus: focus::Focus::default(),
            in_parts: true,
            inserts: Vec::new(),
            shown_put: Vec::new(),
            names: HashMap::new(),
            project,
            editor: None,
            panes: Panes::Page,
            edited: None,
            source_size: (80, 40),
            source_metrics: (0.0, 1.0, 1.0),
            file,
            revision: 0,
            compiled_revision: None,
            document: None,
            whole: None,
            compiled_world: None,
            maps: HashMap::new(),
            page_lines: RefCell::default(),
            cursor_page: Cell::new(0),
            landed: Cell::new(None),
            elsewhere: Cell::new(None),
            others: Vec::new(),
            state_dir: None,
            stale_files: Vec::new(),
            opened: Vec::new(),
            paging: Paging::default(),
            slide: 0,
            pushed: 0.0,
            grid: None,
            equation_set: false,
            keymap: Keymap::default(),
            typed: Vec::new(),
            mapping: keymap::Waiting::default(),
            vim: vim::Vim::default(),
            document_revision: None,
            predicted: None,
            changed: None,
            cursor_at: None,
            predicted_at: None,
            declined: None,
            problems: Vec::new(),
            diagnostics: Vec::new(),
            layout: Layout::default(),
            view: Viewport {
                zoom: scale,
                top: 0.0,
                center: 0.0,
                width: 1,
                height: 1,
            },
            window: (1, 1),
            scale: 1.0,
            display: 1.0,
            ui: 1.0,
            fit: true,
            mode: Mode::Normal,
            pending_g: false,
            message: None,
            performance_panel: false,
            quit: false,
            agent_edits: AgentEdits::default(),
            proposals: Vec::new(),
            proposals_elsewhere: Vec::new(),
            next_proposal: 1,
            proposal_text: None,
            agent_mark: None,
            completion: None,
            tooltip: None,
            hints: hints::Hints::default(),
            picker: None,
            history: None,
            refer: refer::Refer::default(),
            sources_kept: sources_look::Kept::default(),
            clipboard: registers::Clipboard::default(),
            language: language::Language::default(),
            notes: notes::Notes::default(),
            margin: margin::Margin::default(),
            refused: refused::Refused::default(),
            caret_at: None,
            planned: HashMap::new(),
            shown: HashMap::new(),
            exports: export::Exports::default(),
            pointer: pointer::Pointer::default(),
            start: start::Start::default(),
            find: find::Find::default(),
            waiting: waiting::Waiting::default(),
            settings: set::Settings::default(),
            stats: Stats::default(),
        };
        viewer.resize(window, scale);
        Some(viewer)
    }

    /// Makes this a window that edits the main file through `editor`, and
    /// says what opening the file found.
    #[must_use]
    pub fn editing(mut self, editor: Editor, opened: &Opened) -> Self {
        // The text to compile is the editor's from now on; it may hold
        // changes the file does not.
        self.project.set_source(editor.source().clone());
        self.file = editor
            .path()
            .strip_prefix(self.project.root_dir())
            .unwrap_or(editor.path())
            .to_string_lossy()
            .into_owned();
        self.opened.push(editor.file());
        let restored = editor.restored();
        let remarks = editor.remarks().to_vec();
        self.editor = Some(editor);
        self.panes = Panes::Split;
        self.announce(opened, restored);
        self.announce_remarks(&remarks);
        self.arrange();
        self
    }

    /// Takes from `options` what is the user's to say about the viewer.
    #[must_use]
    pub fn configured(mut self, options: &crate::app::Options) -> Self {
        self.agent_edits = options.agent_edits;
        self.state_dir = Some(options.state_dir.clone());
        self.keymap = options.keys.clone();
        // The page is where a document is worked on; the source is there
        // for whoever wants to see it (`gs`).
        self.panes = options.layout.unwrap_or(Panes::Page);
        if options.slides {
            self.paging = Paging::Slides;
        }
        self.inserts.clone_from(&options.inserts);
        self.set_language(&options.language);
        if let Some(ui) = options.ui_scale.filter(|ui| ui.is_finite()) {
            self.ui = ui.clamp(UI_SMALLEST, UI_LARGEST);
            self.scale = self.display * self.ui;
        }
        self.arrange();
        if self.text().is_some_and(|text| text.trim().is_empty()) {
            self.notice(
                "a new document: i to write, # for what can be put in, \
                 gD for how it looks, g? for all there is to do",
            );
        }
        self.configure_settings(&options.settings, options.complaint.is_some());
        if let Some(complaint) = &options.complaint {
            self.alert(complaint.clone());
        }
        self
    }

    fn alert(&mut self, text: impl Into<String>) {
        self.message = Some(Message {
            text: text.into(),
            alert: true,
        });
    }

    fn notice(&mut self, text: impl Into<String>) {
        self.message = Some(Message {
            text: text.into(),
            alert: false,
        });
    }

    /// The text being edited, if the window edits one.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        self.editor.as_ref().map(Editor::text)
    }

    /// What the window shows.
    #[must_use]
    pub fn panes(&self) -> Panes {
        self.panes
    }

    /// Whether the user asked to close the window.
    #[must_use]
    pub fn wants_quit(&self) -> bool {
        self.quit
    }

    /// What the keys since the last call mean for compiling.
    pub fn take_edited(&mut self) -> Option<Edited> {
        self.edited.take()
    }

    /// Since when there are changes that a power failure would lose.
    #[must_use]
    pub fn unsynced_since(&self) -> Option<Instant> {
        let editors = self.editor.iter().chain(&self.others);
        editors.filter_map(Editor::unsynced_since).min()
    }

    /// Forces the journal onto the disk.
    pub fn sync(&mut self) {
        let editors = self.editor.iter_mut().chain(&mut self.others);
        // All of them, whatever becomes of one.
        let failed: Vec<std::io::Error> =
            editors.filter_map(|editor| editor.sync().err()).collect();
        if let Some(err) = failed.first() {
            self.alert(format!("cannot write the journal: {err}"));
        }
        self.keep_notes();
    }

    /// The current revision and a snapshot of it to compile.
    #[must_use]
    pub fn snapshot(&mut self) -> (u64, ProjectWorld) {
        if let Some(editor) = &self.editor {
            self.project.set_source(editor.source().clone());
        }
        if let Some(world) = self.history_world() {
            return (self.revision, world);
        }
        let mut put = self.put_to_show();
        self.put_as_written(&mut put);
        self.shown_put.clone_from(&put);
        let world = match &self.editor {
            Some(editor) if !put.is_empty() => self.project.snapshot_showing(editor.file(), &put),
            _ => self.project.snapshot(),
        };
        (self.revision, world)
    }

    /// What is put into the text to show it, and where: into the
    /// equation the cursor is in, something in every place that is
    /// still empty (the parts of a fraction just begun), and marks
    /// around the word being typed while it names nothing. Half an
    /// equation is none to Typst; so it is one, and shows what it lacks.
    fn put_to_show(&mut self) -> Vec<(usize, &'static str)> {
        let Some(editor) = &self.editor else {
            return Vec::new();
        };
        let (source, cursor) = (editor.source(), editor.cursor());
        let own = equation_at(source, cursor);
        // What was left unfinished elsewhere shows what it lacks too:
        // the document is not to go for it.
        let left = crate::typst_text::left_empty_everywhere(source, own.as_ref());
        let mut put: Vec<(usize, &'static str)> =
            left.into_iter().map(|place| (place, EMPTY)).collect();
        let Some(equation) = own else {
            // Where nothing is written yet (a paragraph just begun, a
            // cell of a table, a caption), something that takes no room
            // but has a place on the page: the cursor's.
            let typing = editor.mode() == editor::Mode::Insert;
            if typing && crate::typst_text::is_empty_spot(source, cursor) {
                put.push((cursor, CURSOR_MARK));
            } else if typing && let Some(blank) = crate::typst_text::raw_blank(source, cursor) {
                // So in a block of code with nothing on the line yet;
                // code is shown as it is written, so it is a blank.
                put.push((cursor, blank));
            }
            put.sort_by_key(|(at, _)| *at);
            return put;
        };
        let places = empty_places(source, &equation);
        put.extend(places.into_iter().map(|place| (place, EMPTY)));
        // The word the cursor is typing: up to it, and what letters
        // stand right behind it (it was typed before them).
        let text = source.text();
        let typing = editor.mode() == editor::Mode::Insert;
        let before = text.get(..cursor).unwrap_or("");
        let named = before
            .trim_end_matches(|c: char| c.is_alphanumeric() || c == '.')
            .len();
        let behind = text[cursor..].len()
            - text[cursor..]
                .trim_start_matches(char::is_alphanumeric)
                .len();
        let begun = before[named..].trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
        let word = cursor - begun.len()..cursor + behind;
        let begins = text[..word.start].chars().next_back();
        let goes_on = text[word.end..].chars().next();
        let alone = !begins.is_some_and(|c| c.is_alphanumeric() || matches!(c, '#' | '"' | '.'))
            && !goes_on.is_some_and(char::is_alphanumeric);
        let in_math = crate::typst_text::ground(source, word.start + 1) == Ground::Math
            && crate::typst_text::ground(source, cursor) == Ground::Math;
        if typing && alone && in_math && text[word.clone()].chars().count() >= 2 {
            let name = text[word.clone()].to_owned();
            let known = if let Some(known) = self.names.get(&name) {
                *known
            } else {
                let known = self.chrome.typesets(&format!("${name}$"));
                if self.names.len() >= NAMES_KEPT {
                    self.names.clear();
                }
                self.names.insert(name, known);
                known
            };
            if !known {
                put.push((word.start, "\""));
                put.push((word.end, "\""));
            }
        }
        put.sort_by_key(|(at, _)| *at);
        put
    }

    /// Whether what was last put into the text to typeset it holds more
    /// than the empty places: marks that are there only while typing.
    fn shows_typing(&self) -> bool {
        self.shown_put.iter().any(|(_, mark)| *mark != EMPTY)
    }

    /// Where the document's main file is, if it is a file.
    #[must_use]
    pub fn main_path(&self) -> Option<std::path::PathBuf> {
        self.project.main_path().map(Path::to_owned)
    }

    /// Whether the pages on screen reflect the current revision.
    #[must_use]
    pub fn is_compiled(&self) -> bool {
        self.compiled_revision == Some(self.revision)
    }

    /// The document on screen, with the revision it was compiled from.
    #[must_use]
    pub fn document(&self) -> Option<&Arc<PagedDocument>> {
        self.document.as_ref()
    }

    /// The files whose changes on disk concern this window.
    #[must_use]
    pub fn dependencies(&self) -> Vec<std::path::PathBuf> {
        self.project.dependencies()
    }

    #[must_use]
    pub fn state(&self) -> State {
        State {
            file: self.file.clone(),
            pages: self
                .document
                .as_ref()
                .map_or(0, |document| document.pages().len()),
            page: self
                .view
                .current_page(&self.layout)
                .map_or(0, |index| index + 1),
            zoom: self.view.zoom,
            revision: self.revision,
            compiled_revision: self.compiled_revision,
            mode: self.mode.clone(),
            problems: self.problems.clone(),
            performance_panel: self.performance_panel,
            editing: self.editor.as_ref().map(|editor| {
                let (line, column) = editor.position();
                Editing {
                    mode: editor.mode(),
                    line,
                    column,
                    modified: editor.is_modified(),
                    command: editor.command_line().map(str::to_owned),
                    prompt: editor.prompt().unwrap_or(':'),
                    recording: editor.recording(),
                    replacing: editor.replacing(),
                }
            }),
        }
    }

    /// The width of the source pane and the left edge and width of the
    /// area pages are drawn in, in device pixels.
    /// What the window shows: what was asked for, but the source where
    /// there are no pages to show yet (the text has never compiled), so
    /// that what is wrong with it can be seen and mended.
    fn shown_panes(&self) -> Panes {
        if self.document.is_none() && self.editor.is_some() && self.compiled_revision.is_some() {
            Panes::Source
        } else {
            self.panes
        }
    }

    fn split(&self) -> (u32, u32, u32) {
        let width = self.window.0;
        match (&self.editor, self.shown_panes()) {
            (None, _) | (_, Panes::Page) => (0, 0, width),
            (_, Panes::Source) => (width, width, 0),
            (_, Panes::Split) => (width / 2, width / 2, width - width / 2),
        }
    }

    /// Sizes the page view for the panes shown.
    fn arrange(&mut self) {
        self.view.width = self.split().2.max(1);
        self.view.height = self.window.1.saturating_sub(self.status_height()).max(1);
        self.relayout();
    }

    fn cycle_panes(&mut self) {
        if self.editor.is_some() {
            self.panes = match self.panes {
                Panes::Split => Panes::Source,
                Panes::Source => Panes::Page,
                Panes::Page => Panes::Split,
            };
            self.arrange();
            self.follow_cursor();
        }
    }

    /// The window changed size or moved to a display with another scale.
    pub fn resize(&mut self, window: (u32, u32), scale: f64) {
        self.window = (window.0.max(1), window.1.max(1));
        self.display = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        self.scale = self.display * self.ui;
        self.arrange();
    }

    /// Makes everything that is not a page (the status line, the lists,
    /// the source) `ui` times its usual size.
    pub fn set_ui_scale(&mut self, ui: f64) {
        if !ui.is_finite() {
            return;
        }
        self.ui = ui.clamp(UI_SMALLEST, UI_LARGEST);
        self.scale = self.display * self.ui;
        self.arrange();
        self.follow_cursor();
    }

    /// `:ui`: says how large the interface is; with a number, makes it
    /// that many times its usual size (or that many percent of it); with
    /// `+` or `-`, a step larger or smaller. Returns whether `command`
    /// was that.
    fn ui_command(&mut self, command: &str) -> bool {
        let Some(wanted) = command.strip_prefix("ui") else {
            return false;
        };
        if !(wanted.is_empty() || wanted.starts_with([' ', '+', '-'])) {
            return false;
        }
        let wanted = wanted.trim();
        let ui = match wanted {
            "" => self.ui,
            "+" => self.ui * UI_STEP,
            "-" => self.ui / UI_STEP,
            number => match number.trim_end_matches('%').parse::<f64>() {
                // 150 and 1.5 are the same wish.
                Ok(percent) if percent >= 10.0 => percent / 100.0,
                Ok(times) if times > 0.0 => times,
                _ => {
                    self.alert(format!(
                        "not a size: {number} (try :ui 1.5, :ui + or :ui -)"
                    ));
                    return true;
                }
            },
        };
        self.set_ui_scale(ui);
        self.notice(format!(
            "interface at {:.0} %  (:ui + and :ui - change it; it is kept as \
             ui_scale under [view])",
            self.ui * 100.0
        ));
        true
    }

    /// A file changed on disk. Returns whether a compile is needed.
    pub fn file_changed(&mut self, path: &Path) -> bool {
        if self.is_edited(path) {
            return self.edited_file_changed();
        }
        if let Some(changed) = self.waiting_file_changed(path) {
            return changed;
        }
        match self.project.file_changed(path) {
            Ok(true) => {
                self.revision += 1;
                true
            }
            Ok(false) => false,
            Err(err) => {
                // Editors replace files by deleting and renaming; a file
                // that is briefly unreadable is not worth losing the page
                // over. Keep what is shown and say why it is stale.
                self.alert(err.to_string());
                false
            }
        }
    }

    /// The file being edited changed on disk, or seems to have: saving it
    /// from here looks the same. Without unsaved changes the new text is
    /// taken over (and undo brings the old one back); with them, neither
    /// side is touched and the user decides.
    fn edited_file_changed(&mut self) -> bool {
        let Some(editor) = &mut self.editor else {
            return false;
        };
        match editor.disk_status() {
            Ok(DiskStatus::Unchanged) => false,
            Ok(DiskStatus::Changed) if editor.is_modified() => {
                self.alert("the file changed on disk: :e! loads it, :w! overwrites it");
                false
            }
            Ok(DiskStatus::Changed) => match editor.reload() {
                Ok(()) => {
                    self.revision += 1;
                    self.notice("the file changed on disk and was loaded again");
                    true
                }
                Err(err) => {
                    self.alert(err.to_string());
                    false
                }
            },
            Ok(DiskStatus::Missing) => {
                self.alert("the file is gone from disk; :w writes it again");
                false
            }
            Err(err) => {
                self.alert(err.to_string());
                false
            }
        }
    }

    /// Scrolls the pages to where the text under the cursor is set, if the
    /// pages say where that is and it is out of view.
    fn follow_cursor(&mut self) {
        let margin = f64::from(self.view.height) / 8.0;
        self.follow_cursor_by(margin, margin);
    }

    /// Like [`Viewer::follow_cursor`], where the top of the cursor is
    /// nearer than `above` device pixels to the top of the view, or than
    /// `below` to its bottom.
    fn follow_cursor_by(&mut self, above: f64, below: f64) {
        if self.shown_panes() == Panes::Source || self.follows_step() {
            return;
        }
        let (Some(change), Some(editor), Some(document), Some(world)) = (
            self.places(),
            &self.editor,
            &self.document,
            &self.compiled_world,
        ) else {
            return;
        };
        let file = self.file_id();
        let Ok(source) = world.source(file) else {
            return;
        };
        let cursor = change.back_or_edge(editor.cursor());
        // Blanks right before it in the text the pages were made of.
        // Behind blanks at the end of what holds them (a footnote's
        // text, a caption) the cursor has no place of its own: it
        // stands behind what is before them. Not where the thing that
        // holds them begins, which may be far up the page.
        let blanks = world
            .written(file)
            .map_or(0, |written| blanks_before(written.text(), cursor));
        // Where it was last found, if it is there still: that page shows
        // it for certain, and may show it by something no search finds.
        let near = self.cursor_page.get();
        let there = document.pages().get(near).and_then(|page| {
            let map = self
                .maps
                .entry(near)
                .or_insert_with(|| PageMap::build(world, &page.frame));
            let caret = map
                .caret(file, cursor)
                .or_else(|| {
                    (blanks > 0)
                        .then(|| map.caret(file, cursor - blanks))
                        .flatten()
                })
                .map(|caret| caret.cell);
            let cell = caret.or_else(|| map.object(file, cursor).first().copied())?;
            Some(zetwerk_typst::PagePosition {
                page: near,
                x: cell.min.x.to_pt(),
                y: cell.min.y.to_pt(),
            })
        });
        let shown = world.shown_at(file, cursor);
        let Some(position) =
            there.or_else(|| zetwerk_typst::locate(document, &source, shown, near))
        else {
            return;
        };
        self.cursor_page.set(position.page);
        if self.paging == Paging::Slides {
            return self.show_slide(position.page);
        }
        let Some(page) = self.layout.pages.get(position.page) else {
            return;
        };
        let zoom = self.view.zoom;
        let y = f64::from(page.y) + position.y * zoom;
        let (top, height) = (self.view.top * zoom, f64::from(self.view.height));
        if y < top + above || y > top + height - below {
            self.view.top = (y - height / 3.0) / zoom;
            self.view.clamp(&self.layout);
        }
    }

    /// A compile finished.
    pub fn compiled(&mut self, output: CompileOutput) {
        if self
            .compiled_revision
            .is_some_and(|newest| output.revision < newest)
        {
            return;
        }
        self.compiled_revision = Some(output.revision);
        self.stats.compile_ms = output.duration.as_secs_f64() * 1000.0;
        self.stats.compiles += 1;
        self.whole_took(output.duration);
        self.problems = if output.compiled.panicked {
            vec!["the Typst compiler crashed on this document".to_owned()]
        } else {
            let world = &*output.world;
            let describe = |error| zetwerk_typst::describe(world, error);
            output.compiled.errors.iter().map(describe).collect()
        };
        let world = &*output.world;
        let compiled = &output.compiled;
        let errors = compiled.errors.iter().map(|error| (error, true));
        let warnings = compiled.warnings.iter().map(|warning| (warning, false));
        self.diagnostics = errors
            .chain(warnings)
            .filter_map(|(diagnostic, is_error)| {
                let (file, range) = zetwerk_typst::place(world, diagnostic)?;
                let range = world.written_at(file, range.start)..world.written_at(file, range.end);
                Some((file, range, is_error, diagnostic.message.to_string()))
            })
            .collect();
        self.diagnostics.sort_by_key(|(_, range, _, _)| range.start);
        // Pages a part of a newer text was put into stay until that text
        // is typeset whole: the older one would take back what was typed
        // since.
        let newer = self
            .document_revision
            .is_some_and(|shown| shown > output.revision);
        if let Some(document) = output.compiled.document {
            let document = Arc::new(document);
            self.whole = Some(Arc::clone(&document));
            if newer && self.document.is_some() {
                return;
            }
            if self.document.is_none() {
                // The first pages there are: until now the window showed
                // the source in their place.
                self.document = Some(document);
                self.arrange();
                // Now that it is known which files the document is made of.
                self.announce_unsaved_elsewhere();
            } else {
                self.document = Some(document);
            }
            self.compiled_world = Some(Arc::clone(&output.world));
            self.document_revision = Some(output.revision);
            self.predicted = None;
            self.predicted_at = None;
            self.maps.clear();
            self.page_lines.get_mut().clear();
            self.landed.set(None);
            self.stale_files.clear();
            self.planned.clear();
            if output.revision < self.revision {
                // Of an older text than the one typed: what was typed
                // since is worked into these pages as it was into the
                // last, or it would be gone from the page until the
                // next compile, and the cursor with it.
                self.predict();
            }
            self.relayout();
            // No key moved the cursor: the view goes after it only where
            // the pages put it out of the window (a paragraph begun at
            // its foot), and not because it is near an edge, a moment
            // after the key.
            self.follow_cursor_by(0.0, LANDED_MARGIN * self.view.zoom);
        }
        self.refer_problems();
        // An equation Typst refuses is shown as it is written, and the
        // pages are made again with it so: also where the pages were
        // carried over to this text without Typst (the text came back to
        // what they show, or a part was typeset alone), which refused it.
        let file = self.editor.as_ref().map(crate::editor::Editor::file);
        let refused = self
            .diagnostics
            .iter()
            .find(|(of, _, is_error, _)| *is_error && Some(*of) == file)
            .map(|(_, range, _, message)| (range.start, message.clone()));
        if output.revision == self.revision
            && let Some((at, why)) = refused
            && self.show_as_written(at, &why)
        {
            self.edited = Some(Edited::Command);
        }
        self.margin_compiled(&output.world);
        self.refused_compiled(&output.world);
    }

    /// The user turned the wheel or dragged on the touchpad, by this many
    /// device pixels.
    pub fn scroll(&mut self, dx: f64, dy: f64) {
        match self.paging {
            Paging::Scroll => self.view.scroll_by(dx, dy, &self.layout),
            Paging::Slides => self.push_slides(dy),
        }
    }

    /// Zooms by `factor`, keeping the document under `anchor` (a position in
    /// the window, in device pixels) where it is.
    pub fn zoom_by(&mut self, factor: f64, anchor: (f64, f64)) {
        self.set_zoom(self.view.zoom * factor, anchor);
    }

    fn set_zoom(&mut self, zoom: f64, anchor: (f64, f64)) {
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        if (zoom - self.view.zoom).abs() < f64::EPSILON {
            return;
        }
        self.fit = false;
        self.view.zoom_about(zoom, anchor);
        self.planned.clear();
        self.relayout();
    }

    /// Lays the document out at the current zoom, following the window
    /// width if the zoom is set to fit.
    fn relayout(&mut self) {
        let Some(document) = &self.document else {
            self.layout = Layout::default();
            return;
        };
        if self.paging == Paging::Slides {
            self.slide_layout();
            let pages = self.layout.pages.len();
            self.shown.retain(|index, _| *index < pages);
            return;
        }
        if self.fit {
            let widest = document
                .pages()
                .iter()
                .map(|page| page.frame.width().to_pt())
                .fold(0.0, f64::max);
            let zoom = self.view.fit_width(widest);
            if (zoom - self.view.zoom).abs() > f64::EPSILON {
                self.view.zoom = zoom;
                self.planned.clear();
            }
        }
        let zoom = self.view.zoom;
        self.layout = Layout::new(document.pages().iter().map(|p| page_size_px(p, zoom)), zoom);
        self.view.clamp(&self.layout);
        let pages = self.layout.pages.len();
        self.shown.retain(|index, _| *index < pages);
    }

    fn status_height(&self) -> u32 {
        (STATUS_HEIGHT * self.scale).round() as u32
    }

    /// A key, as it is: no mapping of the user's is applied to it.
    ///
    /// Who has a key is decided here, in this order: the overview of
    /// pages; the places to fill in of a starter (`start_key`); a thing
    /// that shows as nothing that `]i` went to, or the prompt it is typed
    /// anew in (`margin_key`); the labels of `gw`, a list of places to go,
    /// the aids and the list of completions (`assist_key`); and then Vim.
    ///
    /// Tab means many things, and so means the first of these that
    /// holds. While typing:
    ///
    /// 1. A list of completions is up (or of things to refer to): Tab
    ///    takes the chosen entry. (Where what is typed is that entry
    ///    already, Tab goes on down this list.)
    /// 2. The rest of a word shows faint behind the cursor (`:ghost`):
    ///    Tab takes it. What is shown wins over what is not.
    /// 3. The word before the cursor is the short name of a snippet: it
    ///    becomes the snippet.
    /// 4. A starter is being filled in: to its next place to fill in.
    /// 5. In an equation, in the arguments of a call: to the next place
    ///    to fill in. In a cell of a table: to the next cell; behind the
    ///    last one a row is made.
    /// 6. On a page nothing else: a tab shows as nothing there. In the
    ///    source, blanks to the next stop.
    ///
    /// In normal mode Tab goes to the next place to fill in of a starter
    /// where the text has one; where it has none, it is Vim's Ctrl-I:
    /// on again to where Ctrl-O came back from. On the command line it
    /// takes the rest of a command shown faint.
    fn plain_key(&mut self, press: KeyPress) {
        self.message = None;
        let press = self.shift_enter(press);
        if self.stop_key(press)
            || self.set_key(press)
            || self.confirm_key(press)
            || self.cell_block_key(press)
        {
            return;
        }
        if self.grid.is_some() {
            return self.grid_key(press);
        }
        if self.start_key(press) || self.refused_key(press) || self.margin_key(press) {
            return;
        }
        if self.bib_undo_key(press) {
            return;
        }
        if self.editor.is_some() {
            if !self.assist_key(press) {
                self.editor_key(press);
                self.complete(false);
            }
            return;
        }
        if let Mode::Command(text) = &mut self.mode {
            match press.key {
                Key::Escape => self.mode = Mode::Normal,
                Key::Enter => {
                    let command = std::mem::take(text);
                    self.mode = Mode::Normal;
                    self.execute(command.trim());
                }
                Key::Backspace if text.pop().is_none() => self.mode = Mode::Normal,
                Key::Char(c) if !press.ctrl && !press.alt => text.push(c),
                _ => {}
            }
            return;
        }

        let pending_g = std::mem::take(&mut self.pending_g);
        let line = LINE_SCROLL * self.scale;
        let view = f64::from(self.view.height);
        let page = (view - PAGE_OVERLAP * self.scale).max(line);
        let center = (f64::from(self.view.width) / 2.0, view / 2.0);
        match (press.key, press.ctrl) {
            (Key::Char('j') | Key::Down, false) => self.scroll(0.0, line),
            (Key::Char('k') | Key::Up, false) => self.scroll(0.0, -line),
            (Key::Char('l') | Key::Right, false) => self.scroll(line, 0.0),
            (Key::Char('h') | Key::Left, false) => self.scroll(-line, 0.0),
            (Key::Char('d'), true) => self.scroll(0.0, view / 2.0),
            (Key::Char('u'), true) => self.scroll(0.0, -view / 2.0),
            (Key::Char('f'), true) | (Key::PageDown | Key::Char(' '), false) => {
                self.scroll(0.0, page);
            }
            (Key::Char('b'), true) | (Key::PageUp, false) => self.scroll(0.0, -page),
            (Key::Char('g'), false) if pending_g => self.scroll(0.0, f64::NEG_INFINITY),
            (Key::Char('s'), false) if pending_g => self.cycle_panes(),
            (Key::Char('g'), false) => self.pending_g = true,
            (Key::Home, _) => self.scroll(0.0, f64::NEG_INFINITY),
            (Key::Char('G'), false) | (Key::End, _) => self.scroll(0.0, f64::INFINITY),
            (Key::Char('+'), false) => self.zoom_by(ZOOM_STEP, center),
            (Key::Char('-'), false) => self.zoom_by(1.0 / ZOOM_STEP, center),
            (Key::Char('='), false) => {
                self.fit = true;
                self.relayout();
            }
            (Key::Char(':'), false) => self.mode = Mode::Command(String::new()),
            _ => {}
        }
    }

    /// A key for the source text.
    fn editor_key(&mut self, press: KeyPress) {
        if (self.editor.as_ref())
            .is_some_and(|editor| registers::asks_clipboard(&mut self.clipboard, editor, press))
        {
            self.clipboard_to_register();
        }
        // What the pages say of where the text is holds also while one
        // of them is shown as worked out: only where things are drawn
        // differs then.
        let places = self.places_of_text();
        let layout = self.shown_panes();
        let Some(editor) = &mut self.editor else {
            return;
        };
        let was_typing = editor.mode() == editor::Mode::Insert;
        let cursor = editor.cursor();
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
        editor.set_rules(self.settings.rules());
        let typed = editor.key(press, &shown);
        let typing = editor.mode() == editor::Mode::Insert;
        // A paragraph is one line of the source and may fill pages: the
        // view follows the cursor, not the line it is in.
        let moved = editor.cursor() != cursor;
        if let Some(failure) = editor.take_failure() {
            self.alert(format!("not changed: {failure}"));
        }
        self.agent_mark = None;
        if typed.changed {
            self.revision += 1;
            self.touched();
            self.sync_proposals();
            self.try_as_equation();
            let shown = self.predict();
            // What begins a paragraph moves all that is below it: that
            // is for Typst to settle, at once.
            let begins = self
                .editor
                .as_ref()
                .is_some_and(|editor| begins_block(editor.text(), editor.cursor()));
            self.edited = Some(if !typing {
                Edited::Command
            } else if shown || !(begins || self.stats.compile_ms <= QUICK_COMPILE_MS) {
                Edited::Typing
            } else {
                Edited::Unpredicted
            });
            if typing {
                self.offer_files();
            }
        } else if was_typing && !typing && (!self.is_compiled() || self.shows_typing()) {
            // So too where the pages were typeset while typing, with the
            // marks of it in: where the cursor was, the quotes around a
            // word. They are not to stay once typing is over.
            self.edited = Some(Edited::LeftInsert);
        } else if typing && moved {
            // The cursor has come to a place with nothing in it (Tab
            // into a cell of a table): the page is to show it there.
            let put = self.put_to_show();
            if !put.is_empty() && put != self.shown_put {
                self.edited = Some(Edited::Unpredicted);
            }
        }
        for effect in typed.effects {
            match effect {
                Effect::Command(command) => self.execute(&command),
                Effect::Scroll(place) => {
                    let (columns, visible) = self.source_size;
                    if let Some(editor) = &mut self.editor {
                        editor.scroll_to(place, columns, visible);
                    }
                }
            }
        }
        self.stepped();
        self.register_to_clipboard();
        if moved {
            self.follow_cursor();
        }
        if let Some((file, offset)) = self.elsewhere.take() {
            self.open_file(file, Some(offset));
        }
        self.find_after_key(press);
    }

    /// Works out what the page looks like with the text as it is now, if
    /// that can be done without compiling: if all that happened since the
    /// last compile is plain text typed into one line of a paragraph.
    /// Returns whether the pages show the current text afterwards.
    fn predict(&mut self) -> bool {
        if let Some(old) = self.predicted.take() {
            self.planned.retain(|(index, _, _), _| *index != old.index);
        }
        match self.try_predict() {
            Ok(()) => {
                self.stats.predicted += 1;
                self.declined = None;
                true
            }
            // Not a word in a line: then the part that changed is
            // typeset, by itself.
            Err(why) => match self.typeset_locally_in_time() {
                Ok(()) => {
                    self.declined = None;
                    true
                }
                Err(alone) => {
                    self.stats.declined += 1;
                    self.declined = Some(format!("{why}; {alone}"));
                    false
                }
            },
        }
    }

    /// See [`Viewer::predict`]. The error says why the page cannot be
    /// worked out.
    fn try_predict(&mut self) -> Result<(), String> {
        let (Some(editor), Some(document), Some(world)) =
            (&self.editor, &self.document, &self.compiled_world)
        else {
            return Err("nothing compiled yet".to_owned());
        };
        let file = editor.file();
        let base = world
            .written(file)
            .map_err(|_| "the compiled source is gone".to_owned())?;
        let (offset, removed, text) = match difference(base.text(), editor.text()) {
            Difference::Nothing => {
                // Back at the text the pages show. Pages a part was put
                // into are not all of it typeset: that is still to come.
                let whole = matches!((document, &self.whole),
                    (document, Some(whole)) if Arc::ptr_eq(document, whole));
                self.document_revision = Some(self.revision);
                if whole {
                    self.compiled_revision = Some(self.revision);
                }
                return Ok(());
            }
            Difference::Replacement {
                offset,
                removed,
                text,
            } => (offset, removed, text),
        };
        let change = Change {
            offset,
            removed,
            inserted: text.len(),
        };
        self.changed = Some((self.revision, file, change));
        if removed > MAX_PREDICTED || text.len() > MAX_PREDICTED {
            return Err("too much changed".to_owned());
        }
        // Blanks typed at the end of a line show nowhere on the page.
        let end = offset + text.len();
        let line_ends = matches!(editor.text()[end..].chars().next(), None | Some('\n'));
        let text = if line_ends {
            text.trim_end_matches(' ')
        } else {
            text
        };
        // What is put in has to be plain text; and where text was taken
        // out, what now meets there has to be one piece of plain text,
        // not, say, two blanks or the two halves of a dash.
        let now = editor.text();
        let until = offset + text.len();
        let (mut from, mut to) = (offset, until);
        if removed > 0 {
            // One character further on either side, within the line.
            let before = now[..offset].chars().next_back().filter(|c| *c != '\n');
            let after = now[until..].chars().next().filter(|c| *c != '\n');
            from -= before.map_or(0, char::len_utf8);
            to += after.map_or(0, char::len_utf8);
        }
        if to > from && !is_plain_text(editor.source(), from, to - from) {
            return Err("not plain text of a paragraph".to_owned());
        }
        if world.shows_more() {
            return Err("the pages show more than is written".to_owned());
        }
        // The page is that of the text next to the insertion. While typing
        // goes on in one place it stays the same page, and is not looked
        // for again.
        let index = match self.predicted_at {
            Some((at, index)) if at == offset => index,
            // Near the cursor: a part typeset by itself is looked for
            // no further either.
            _ => zetwerk_typst::locate_near(
                document,
                &base,
                offset.saturating_sub(1),
                self.cursor_page.get(),
                PREDICTED_NEAR,
            )
            .map(|position| position.page)
            .ok_or("the text is on no page near the cursor")?,
        };
        self.predicted_at = Some((offset, index));
        let page = document
            .pages()
            .get(index)
            .ok_or("the text is on no page")?;
        let map = self
            .maps
            .entry(index)
            .or_insert_with(|| PageMap::build(world, &page.frame));
        let edit = Replacement {
            file,
            offset,
            removed,
            text,
        };
        let prediction = predict(&page.frame, map, edit).map_err(|why| format!("{why:?}"))?;
        self.planned.retain(|(planned, _, _), _| *planned != index);
        self.predicted = Some(Predicted {
            index,
            page: Page {
                frame: prediction.frame,
                ..page.clone()
            },
            typed: prediction.typed,
            end,
        });
        Ok(())
    }

    /// How places in the text relate to places in the text the pages on
    /// screen were compiled from. `None` if they do not in a simple way:
    /// nothing is compiled, more than one thing changed since, or a page
    /// is shown as worked out, not as compiled.
    fn places(&self) -> Option<Change> {
        if self.predicted.is_some() {
            return None;
        }
        self.places_of_text()
    }

    /// Like [`Viewer::places`], for asking where text is in the compiled
    /// document, whatever is on screen.
    fn places_of_text(&self) -> Option<Change> {
        self.document.as_ref()?;
        if self.looks_back() {
            return None;
        }
        if self.document_revision == Some(self.revision) {
            return Some(Change::NONE);
        }
        // One change, in one file: in the others nothing has moved.
        let (revision, file, change) = self.changed?;
        let only = self.stale_files.iter().all(|stale| *stale == file);
        if revision != self.revision || !only {
            return None;
        }
        Some(if file == self.file_id() {
            change
        } else {
            Change::NONE
        })
    }

    /// Whether the pages show text that has not been compiled, worked out
    /// from what was typed.
    #[must_use]
    pub fn is_predicted(&self) -> bool {
        self.predicted.is_some()
    }

    #[allow(clippy::too_many_lines, reason = "a line for every command")]
    fn execute(&mut self, command: &str) {
        self.settings_before(command);
        if self.start_command(command) {
            return;
        }
        let unwritten = self.unwritten();
        let modified = !unwritten.is_empty();
        if self.file_command(command)
            || self.set_command(command)
            || self.ui_command(command)
            || self.settings_command(command)
            || self.language_command(command)
            || self.notes_command(command)
            || self.refused_command(command)
            || self.vim_command(command)
        {
            return;
        }
        match command {
            "" => {}
            "q" | "quit" if modified => {
                self.alert(format!(
                    "unsaved changes in {}: :wq saves them, :q! drops them \
                     (undo brings them back when the file is opened again)",
                    unwritten.join(", ")
                ));
            }
            "q" | "quit" => self.quit = true,
            // What is not written is dropped, as whoever says `:q!`
            // means it: the files are as they were written last when
            // they are opened again. The history keeps what was dropped,
            // and undo brings it back then.
            "q!" | "quit!" => self.leave_dropping(),
            "w" | "write" => {
                self.save(false);
            }
            "w!" | "write!" => {
                self.save(true);
            }
            // With all that is open written, not only this file.
            "wq" | "x" | "xit" | "wqa" | "xa" => self.quit = !modified || self.save_all(),
            "wq!" => self.quit = self.save(true) && self.save_all(),
            "e!" | "edit!" => self.reload(),
            "perf" => self.performance_panel = !self.performance_panel,
            "slides" => {
                let paging = match self.paging {
                    Paging::Scroll => Paging::Slides,
                    Paging::Slides => Paging::Scroll,
                };
                self.set_paging(paging);
            }
            "grid" => self.open_grid(),
            "jump" => self.start_jump(),
            "page next" => self.step_page(true),
            "page previous" => self.step_page(false),
            "describe" => self.describe(),
            history if self.history_command(history) => {}
            find if self.find_command(find) => {}
            "definition" => self.goto_definition(),
            "outline" => self.pick(false),
            "labels" => self.pick(true),
            "references" | "refs" => self.references(),
            "diagnostic next" => self.goto_diagnostic(true),
            "diagnostic previous" => self.goto_diagnostic(false),
            "proposal next" => self.goto_proposal(true),
            "proposal previous" => self.goto_proposal(false),
            "accept" => {
                self.accept_proposals(false);
            }
            "accept all" => {
                self.accept_proposals(true);
            }
            "reject" => {
                self.reject_proposals(false);
            }
            "reject all" => {
                self.reject_proposals(true);
            }
            "agents" => {
                let mode = match self.agent_edits {
                    AgentEdits::Live => "agents change the text (:agents propose to review first)",
                    AgentEdits::Propose => "agents propose changes (:agents live to let them)",
                };
                self.notice(mode);
            }
            "agents live" => {
                self.agent_edits = AgentEdits::Live;
                self.notice("agents change the text from now on");
            }
            "agents propose" => {
                self.agent_edits = AgentEdits::Propose;
                self.notice("agents propose their changes from now on");
            }
            "view" => self.cycle_panes(),
            "view split" | "view source" | "view page" if self.editor.is_some() => {
                self.panes = match command {
                    "view split" => Panes::Split,
                    "view source" => Panes::Source,
                    _ => Panes::Page,
                };
                self.arrange();
                self.follow_cursor();
            }
            _ => match command.parse::<usize>() {
                Ok(page) if page >= 1 => self.goto_page(page - 1),
                _ => self.alert(format!("not a command: {command}")),
            },
        }
    }

    /// Puts the cursor on the next thing the compiler complained about
    /// (`]d`), or the one before (`[d`), and says what the complaint is.
    fn goto_diagnostic(&mut self, forward: bool) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let (cursor, file) = (editor.cursor(), editor.file());
        // Those of this file; the others are in the status line.
        let here: Vec<&(FileId, Range<usize>, bool, String)> = self
            .diagnostics
            .iter()
            .filter(|(of, ..)| *of == file)
            .collect();
        let starts = || here.iter().map(|(_, range, _, _)| range.start);
        let target = if forward {
            starts().position(|start| start > cursor).or(Some(0))
        } else {
            let before = starts().rposition(|start| start < cursor);
            before.or(here.len().checked_sub(1))
        };
        let Some((_, range, is_error, message)) = target.and_then(|at| here.get(at)) else {
            let elsewhere = self.diagnostics.len() - here.len();
            self.notice(match elsewhere {
                0 => "the compiler has no complaints".to_owned(),
                count => format!("no complaints about this file; {count} about others"),
            });
            return;
        };
        editor.set_cursor(range.start);
        let (is_error, message) = (*is_error, message.clone());
        if is_error {
            let said = refused::for_writer(&message);
            self.alert(format!("error: {said}  · :why"));
        } else {
            self.notice(format!("warning: {message}"));
        }
        self.follow_cursor();
    }

    /// Writes the text to its file. Returns whether it is saved now.
    fn save(&mut self, overwrite: bool) -> bool {
        let Some(editor) = &mut self.editor else {
            self.alert("this window only shows the file; there is nothing to write");
            return false;
        };
        match editor.save(overwrite) {
            Ok(()) => {
                let lines = editor.text().matches('\n').count();
                let how = if editor.wrote_in_place() {
                    ", over the file in place: its directory takes no new file"
                } else {
                    ""
                };
                self.notice(format!("{} written, {lines} lines{how}", self.file));
                true
            }
            Err(DiskError::ChangedOnDisk) => {
                self.alert("the file changed on disk: :w! overwrites it, :e! loads it");
                false
            }
            Err(err) => {
                self.alert(format!("not written: {err}"));
                false
            }
        }
    }

    /// Replaces the text with the file's. Undo brings the text back.
    fn reload(&mut self) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        match editor.reload() {
            Ok(()) => {
                self.text_changed(Edited::Command);
                self.notice("loaded from disk; undo brings the text back");
            }
            Err(err) => self.alert(format!("not loaded: {err}")),
        }
    }

    /// What the window should show right now. `has` tells which images the
    /// compositor already holds.
    pub fn scene(&mut self, has: &dyn Fn(TextureId) -> bool) -> Scene {
        let mut scene = Scene::default();
        let (width, height) = (f64::from(self.window.0), f64::from(self.window.1));
        solid(&mut scene, has, BACKDROP, (0.0, 0.0, width, height));
        self.cursor_at = None;
        self.caret_at = None;
        self.equation_set = false;
        self.language_frame();
        if self.grid.is_some() {
            self.grid_scene(&mut scene, has);
            self.status_line(&mut scene, has);
            return scene;
        }
        self.pages(&mut scene, has);
        self.lens(&mut scene, has);
        // Drawn after the pages: a page zoomed wider than its pane must
        // not reach over the source.
        self.source_pane(&mut scene, has);
        self.status_line(&mut scene, has);
        if self.performance_panel {
            self.performance(&mut scene, has);
        }
        self.proposal_panel(&mut scene, has);
        self.review_panels(&mut scene, has);
        self.hints(&mut scene, has);
        self.assist_panels(&mut scene, has);
        self.settings_panel(&mut scene, has);
        self.margin_panel(&mut scene, has);
        self.refused_panel(&mut scene, has);
        scene
    }

    fn pages(&mut self, scene: &mut Scene, has: &dyn Fn(TextureId) -> bool) {
        let Some(document) = self.document.clone() else {
            return;
        };
        if self.shows_no_document() {
            return;
        }
        let (_, left, pane_width) = self.split();
        if pane_width == 0 {
            return;
        }
        let zoom = self.view.zoom;
        for visible in self.view.visible(&self.layout) {
            let predicted = self
                .predicted
                .as_ref()
                .filter(|predicted| predicted.index == visible.index)
                .map(|predicted| predicted.page.clone());
            let (Some(page), Some(page_box)) = (
                predicted.as_ref().or(document.pages().get(visible.index)),
                self.layout.pages.get(visible.index),
            ) else {
                continue;
            };
            let origin = (f64::from(left) + visible.x as f64, visible.y as f64);
            let size = (f64::from(page_box.width), f64::from(page_box.height));
            solid(scene, has, PAPER, (origin.0, origin.1, size.0, size.1));

            let wanted = tiles_in(&mut self.planned, visible.index, visible.region, || {
                tiles(page, zoom, visible.region)
            });
            let complete = wanted.iter().all(|tile| has(texture(tile.key)));
            if !complete && let Some(shown) = self.shown.get(&visible.index) {
                // Stand in with what this page looked like a moment ago,
                // stretched to the new zoom, until the sharp tiles arrive.
                let stretch = zoom / shown.zoom;
                for (key, region) in &shown.tiles {
                    scene.quads.push(Quad {
                        texture: texture(*key),
                        x: (origin.0 + f64::from(region.x) * stretch) as f32,
                        y: (origin.1 + f64::from(region.y) * stretch) as f32,
                        width: (f64::from(region.width) * stretch) as f32,
                        height: (f64::from(region.height) * stretch) as f32,
                    });
                }
            }
            for tile in &wanted {
                if has(texture(tile.key)) {
                    scene.quads.push(Quad {
                        texture: texture(tile.key),
                        x: (origin.0 + f64::from(tile.region.x)) as f32,
                        y: (origin.1 + f64::from(tile.region.y)) as f32,
                        width: tile.region.width as f32,
                        height: tile.region.height as f32,
                    });
                } else {
                    scene.wanted.push(tile.clone());
                }
            }
            if complete {
                let tiles = wanted.iter().map(|tile| (tile.key, tile.region)).collect();
                self.shown.insert(visible.index, Shown { zoom, tiles });
            }
            self.line_numbers(scene, has, visible.index, origin);
            self.step_marks(scene, has, visible.index, origin);
            self.language_page(scene, has, visible.index, origin);
            self.find_marks(scene, has, visible.index, origin);
            self.margin_page(scene, has, visible.index, origin);
            self.page_cursor(scene, has, visible.index, origin);
            self.review_page(scene, has, visible.index, origin);
            self.focus_marks(scene, has, visible.index, origin);
            // Last: the labels lie over all else.
            self.jump_labels(scene, has, visible.index, origin);
        }
    }

    /// The cursor on a page that was worked out, not compiled: all that
    /// is known of such a page is where the typed text is. Returns
    /// whether the pages are of that kind.
    fn predicted_cursor(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        index: usize,
        origin: (f64, f64),
    ) -> bool {
        let (Some(predicted), Some(editor)) = (&self.predicted, &self.editor) else {
            return false;
        };
        let zoom = self.view.zoom;
        let bar = (1.5 * self.scale).round().max(1.0);
        let typing = editor.mode() == editor::Mode::Insert;
        let cursor = editor.cursor();
        if predicted.index == index && cursor == predicted.end && typing {
            let cell = predicted.typed;
            let y = origin.1 + cell.min.y.to_pt() * zoom;
            let height = (cell.max.y - cell.min.y).to_pt() * zoom;
            // Blanks typed at the end of a line show as nothing; the
            // cursor is behind them all the same. Elsewhere they are in
            // the cell already.
            let rest = editor.text().get(cursor..).unwrap_or_default();
            let line_ends = matches!(rest.chars().next(), None | Some('\n'));
            let blanks = if line_ends {
                blanks_before(editor.text(), cursor)
            } else {
                0
            };
            let x = origin.0 + cell.max.x.to_pt() * zoom + blanks as f64 * BLANK * height;
            tint(scene, has, PAGE_BAR, (x.round(), y, bar, height));
            self.cursor_at = Some((x, y, y + height));
        } else if predicted.index == index
            && let Some((offset, _)) = self.predicted_at
            && (offset..=predicted.end).contains(&cursor)
            && let Some(typed) = editor.text().get(offset..predicted.end)
        {
            // In what was typed, or on it after a key of normal mode
            // (`x`, `r`, `u`, `p`, Escape): the cursor is not to be gone,
            // nor a bar, until the page is typeset. Where in the typed
            // text it is, is told by how many of its characters are
            // before it; how wide the one it is on is, by how wide they
            // are on average, or a blank's width twice behind them.
            let cell = predicted.typed;
            let (left, right) = (cell.min.x.to_pt() * zoom, cell.max.x.to_pt() * zoom);
            let y = origin.1 + cell.min.y.to_pt() * zoom;
            let height = (cell.max.y - cell.min.y).to_pt() * zoom;
            let shown = typed.trim_end_matches(' ');
            let all = shown.chars().count();
            let before = shown
                .get(..cursor - offset)
                .map_or(all, |b| b.chars().count());
            let width = if before < all {
                (right - left) / all as f64
            } else {
                2.0 * BLANK * height
            };
            let x = origin.0 + left + (right - left) * before as f64 / all.max(1) as f64;
            if typing {
                tint(scene, has, PAGE_BAR, (x.round(), y, bar, height));
            } else {
                tint(scene, has, PAGE_CURSOR, (x, y, width.max(bar), height));
            }
            self.cursor_at = Some((x, y, y + height));
        }
        true
    }

    /// The cursor and the selection where the text they are on is set on
    /// page `index`, whose top-left corner is drawn at `origin`.
    ///
    /// Only while the pages say where their glyphs come from in the text
    /// as it is: see [`Viewer::places`].
    fn page_cursor(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        index: usize,
        origin: (f64, f64),
    ) {
        let zoom = self.view.zoom;
        let bar = (1.5 * self.scale).round().max(1.0);
        if self.in_place(scene, has, index, origin) {
            return;
        }
        if self.predicted_cursor(scene, has, index, origin) {
            return;
        }
        let Some(change) = self.places() else {
            return;
        };
        let (agent_marks, cells) = (self.agent_marks(), self.cell_marks());
        let (Some(editor), Some(document), Some(world)) =
            (&self.editor, &self.document, &self.compiled_world)
        else {
            return;
        };
        let Some(page) = document.pages().get(index) else {
            return;
        };
        let map = self
            .maps
            .entry(index)
            .or_insert_with(|| PageMap::build(world, &page.frame));
        let file = editor.file();
        let place = |rect: Rect| {
            (
                origin.0 + rect.min.x.to_pt() * zoom,
                origin.1 + rect.min.y.to_pt() * zoom,
                (rect.max.x - rect.min.x).to_pt() * zoom,
                (rect.max.y - rect.min.y).to_pt() * zoom,
            )
        };
        let mut marked: Vec<(Range<usize>, [u8; 4])> = Vec::new();
        for (range, mark) in agent_marks {
            let colour = match mark {
                Mark::Changed => PAGE_AGENT_MARK,
                Mark::Proposed => PAGE_PROPOSAL,
                Mark::Stale => PAGE_STALE_PROPOSAL,
                Mark::Error | Mark::Warning => continue,
            };
            marked.push((range, colour));
        }
        let selected = editor.selections().into_iter().chain(cells);
        marked.extend(selected.map(|range| (range, PAGE_SELECTION)));
        for (range, colour) in marked {
            let mut range = change.back_or_edge(range.start)..change.back_or_edge(range.end);
            if range.is_empty() {
                // Text to be put in: shown on what stands there now.
                range.end += 1;
            }
            for rect in map.selection(file, &range) {
                tint(scene, has, colour, place(rect));
            }
        }
        // Text put in since the compile is nowhere on the page; the place
        // it was put in at is.
        let cursor = change.back(editor.cursor());
        let typing = editor.mode() == editor::Mode::Insert;
        let mut caret = map.caret(file, change.back_or_edge(editor.cursor()));
        // Blanks at the end of a line show as nothing on the page. While
        // typing, the cursor is behind them all the same: as far behind
        // the word before them as they are wide.
        let mut blanks = 0;
        if typing
            && caret.as_ref().is_none_or(|caret| caret.after)
            && let Some((behind, typed)) = caret_before_blanks(map, &change, editor)
        {
            caret = Some(behind);
            blanks = typed;
        }
        if let Some(caret) = &caret {
            let (x, y, _, height) = place(caret.cell);
            self.cursor_at.get_or_insert((x, y, y + height));
        }
        // Typing in what was put in, the cursor is where it was put in:
        // not gone until the page has it (a blank typed beside another,
        // where nothing is typeset by itself). On it in normal mode, it
        // is on nothing the page shows.
        if cursor.is_none() && !typing {
            return;
        }
        if let Some(cursor) = cursor
            && caret.as_ref().is_none_or(|caret| caret.after && !typing)
        {
            // No text of the source is shown here; something made of it
            // may be, such as what a reference refers to by.
            let object = map.object(file, cursor);
            if let Some(first) = object.first() {
                let (x, y, _, height) = place(*first);
                self.cursor_at.get_or_insert((x, y, y + height));
                if let (true, Some(last)) = (typing, object.last()) {
                    // What is typed goes behind it.
                    let (x, y, width, height) = place(*last);
                    tint(scene, has, PAGE_BAR, ((x + width).round(), y, bar, height));
                    return;
                }
                for rect in object {
                    tint(scene, has, PAGE_CURSOR, place(rect));
                }
                return;
            }
        }
        if let Some(caret) = caret {
            let (x, y, width, height) = place(caret.cell);
            if editor.mode() == editor::Mode::Insert || caret.after {
                let x = if caret.after { x + width } else { x };
                let x = x + blanks as f64 * BLANK * height;
                tint(scene, has, PAGE_BAR, (x.round(), y, bar, height));
            } else {
                tint(scene, has, PAGE_CURSOR, (x, y, width.max(bar), height));
            }
        }
    }

    /// Whether the pages alone do not show what the cursor is on: it is in
    /// markup, math or code, or in text typed since the last compile that
    /// no page shows yet.
    fn wants_lens(&self) -> bool {
        let Some(editor) = &self.editor else {
            return false;
        };
        // What is typed shows where it is typed (see `in_place`). Only
        // an equation needs more: it shows as what it means, and what is
        // typed is its source.
        self.shown_panes() == Panes::Page
            && editor.mode() == editor::Mode::Insert
            && self.equation_set
            && equation_at(editor.source(), editor.cursor()).is_some()
    }

    /// The source lens: where only pages are shown and they do not show
    /// what the cursor is on, the source around the cursor, in a small
    /// pane beside it.
    fn lens(&mut self, scene: &mut Scene, has: &dyn Fn(TextureId) -> bool) {
        if !self.wants_lens() {
            return;
        }
        let scale = self.scale;
        let (_, pane_left, pane_width) = self.split();
        let bottom = f64::from(self.window.1.saturating_sub(self.status_height()));
        let anchor = self.cursor_at;
        let Some(editor) = &self.editor else {
            return;
        };
        let cell = self.chrome.cell_width(scale).max(1.0);
        let row_height = (ROW_HEIGHT * scale).round().max(1.0);
        let margin = (PANEL_MARGIN * scale).round();
        let pad = (LENS_PADDING * scale).round();
        let room = f64::from(pane_width) - 2.0 * margin - 2.0 * pad;
        let columns = ((room / cell).floor().max(8.0) as usize).min(LENS_COLUMNS);

        let (cursor, selection, mode) = (editor.cursor(), editor.selection(), editor.mode());
        let text = editor.text();
        // The equation and no more: the line it is in is on the page.
        let block = equation_at(editor.source(), cursor)
            .unwrap_or_else(|| lens_block(editor.source(), cursor));
        let rows = editor::rows(&text[block.clone()], columns);
        let cursor_row = editor::row_of(&rows, cursor - block.start);
        let first = cursor_row
            .saturating_sub(LENS_ROWS / 2)
            .min(rows.len().saturating_sub(LENS_ROWS));
        let shown: Vec<Range<usize>> = rows
            .iter()
            .skip(first)
            .take(LENS_ROWS)
            .map(|row| block.start + row.range.start..block.start + row.range.end)
            .collect();
        let widest = shown
            .iter()
            .map(|row| editor::cells(&text[row.clone()]))
            .max()
            .unwrap_or(0);
        // One cell more: the cursor may stand behind the last character.
        let width = ((widest + 1) as f64 * cell).max(LENS_MIN_CELLS * cell) + 2.0 * pad;
        let height = shown.len() as f64 * row_height + 2.0 * pad;

        let gap = (LENS_GAP * scale).round();
        let left = f64::from(pane_left) + margin;
        let right = f64::from(pane_left) + f64::from(pane_width) - margin;
        let within = (left, margin, right, bottom - margin);
        let (x, y) = lens_place(anchor, (width, height), within, gap);

        let line = scale.round().max(1.0);
        solid(
            scene,
            has,
            LENS_EDGE,
            (x - line, y - line, width + 2.0 * line, height + 2.0 * line),
        );
        solid(scene, has, PANE, (x, y, width, height));
        let mut texts: Vec<(RowText, (f64, f64))> = Vec::new();
        let mut caret = None;
        for (index, row) in shown.iter().enumerate() {
            let top = y + pad + index as f64 * row_height;
            let at = |offset: usize| {
                let offset = offset.clamp(row.start, row.end);
                x + pad + editor::cells(&text[row.start..offset]) as f64 * cell
            };
            if let Some(selection) = &selection
                && selection.start <= row.end
                && selection.end > row.start
            {
                let (from, to) = (at(selection.start), at(selection.end));
                solid(scene, has, SELECTION, (from, top, to - from, row_height));
            }
            if first + index == cursor_row {
                let width = match mode {
                    editor::Mode::Insert => (2.0 * scale).round().max(1.0),
                    _ => cell,
                };
                solid(
                    scene,
                    has,
                    CURSOR,
                    (at(cursor).round(), top, width, row_height),
                );
                caret = Some((at(cursor).round(), top, top + row_height));
            }
            let content = &text[row.clone()];
            let body = content.trim_start_matches([' ', '\t']);
            let indent = editor::cells(&content[..content.len() - body.len()]);
            let body = body
                .trim_end()
                .replace('\t', &" ".repeat(editor::TAB_CELLS));
            if !body.is_empty() {
                let body = RowText {
                    text: body,
                    dim: false,
                };
                texts.push((body, (x + pad + indent as f64 * cell, top)));
            }
        }
        row_images(&mut self.chrome, scene, has, &texts, scale);
        self.caret_at = caret;
    }

    /// The stretches of the source shown in a colour of their own: what
    /// agents did and want to do, and what the compiler complained about,
    /// while the text is the text it complained about.
    fn source_marks(&self) -> Vec<(Range<usize>, Mark)> {
        let mut marked = self.agent_marks();
        marked.extend(self.step_stretches());
        if self.compiled_revision == Some(self.revision) {
            let file = self.file_id();
            let here = self.diagnostics.iter().filter(|(of, ..)| *of == file);
            marked.extend(here.map(|(_, range, is_error, _)| {
                let mark = if *is_error {
                    Mark::Error
                } else {
                    Mark::Warning
                };
                (range.clone(), mark)
            }));
        }
        marked
    }

    /// The source text: line numbers, rows of text, selection and cursor.
    ///
    /// Every row is an image of its own, named by its text. Typing changes
    /// one row, so one small image is made; moving the cursor, scrolling
    /// and selecting make none.
    fn source_pane(&mut self, scene: &mut Scene, has: &dyn Fn(TextureId) -> bool) {
        let width = self.split().0;
        let scale = self.scale;
        let height = f64::from(self.window.1.saturating_sub(self.status_height()));
        let marked = self.source_marks();
        let Some(editor) = &mut self.editor else {
            return;
        };
        if width == 0 {
            return;
        }
        let cell = self.chrome.cell_width(scale).max(1.0);
        let row_height = (ROW_HEIGHT * scale).round().max(1.0);
        solid(scene, has, PANE, (0.0, 0.0, f64::from(width), height));

        let lines = editor.text().matches('\n').count().max(1);
        let digits = lines.to_string().len().max(3);
        let gutter = (digits + 2) as f64 * cell;
        let columns = ((f64::from(width) - gutter - cell) / cell).floor().max(8.0) as usize;
        let visible = (height / row_height).floor().max(1.0) as usize;
        self.source_size = (columns, visible);
        self.source_metrics = (gutter, cell, row_height);
        let top = self.pointer.top(editor, columns, visible, SCROLL_MARGIN);
        editor.set_view(columns, visible);
        let (cursor, selections, mode) = (editor.cursor(), editor.selections(), editor.mode());
        let rows = editor.rows(columns);
        let shown: Vec<editor::Row> = rows.iter().skip(top).take(visible + 1).cloned().collect();
        let last_row_of_line: Vec<bool> = (top..top + shown.len())
            .map(|index| rows.get(index + 1).is_none_or(|next| next.first))
            .collect();
        let cursor_row = editor::row_of(rows, cursor);
        let text = editor.text();

        let mut texts: Vec<(RowText, (f64, f64))> = Vec::new();
        for (index, row) in shown.iter().enumerate() {
            let y = index as f64 * row_height;
            let at = |offset: usize| {
                let offset = offset.clamp(row.range.start, row.range.end);
                gutter + editor::cells(&text[row.range.start..offset]) as f64 * cell
            };
            for (range, mark) in &marked {
                let on_row = range.start <= row.range.end
                    && (range.end > row.range.start || range.start == row.range.start);
                if on_row {
                    let colour = match mark {
                        Mark::Changed => AGENT_MARK,
                        Mark::Proposed => PROPOSAL,
                        Mark::Stale => STALE_PROPOSAL,
                        Mark::Error => ERROR_MARK,
                        Mark::Warning => WARNING_MARK,
                    };
                    let (from, to) = (at(range.start), at(range.end));
                    // Text to be put in where there is none shows as a
                    // sliver.
                    let width = (to - from).max(cell / 3.0);
                    solid(scene, has, colour, (from, y, width, row_height));
                }
            }
            for selection in &selections {
                if !(selection.start <= row.range.end && selection.end > row.range.start) {
                    continue;
                }
                // A selection that goes on past the end of a line takes
                // its line break, shown as one more cell.
                let line_break = last_row_of_line[index] && selection.end > row.range.end;
                let (from, to) = (at(selection.start), at(selection.end));
                let to = if line_break { to + cell } else { to };
                solid(scene, has, SELECTION, (from, y, to - from, row_height));
            }
            if top + index == cursor_row {
                let width = match mode {
                    editor::Mode::Insert => (2.0 * scale).round().max(1.0),
                    _ => cell,
                };
                solid(
                    scene,
                    has,
                    CURSOR,
                    (at(cursor).round(), y, width, row_height),
                );
                self.caret_at = Some((at(cursor).round(), y, y + row_height));
            }
            if row.first {
                let number = (row.line + 1).to_string();
                let x = (digits - number.len().min(digits)) as f64 * cell + cell / 2.0;
                let number = RowText {
                    text: number,
                    dim: true,
                };
                texts.push((number, (x, y)));
            }
            let content = &text[row.range.clone()];
            let body = content.trim_start_matches([' ', '\t']);
            let indent = editor::cells(&content[..content.len() - body.len()]);
            let body = body
                .trim_end()
                .replace('\t', &" ".repeat(editor::TAB_CELLS));
            if !body.is_empty() {
                let x = gutter + indent as f64 * cell;
                let body = RowText {
                    text: body,
                    dim: false,
                };
                texts.push((body, (x, y)));
            }
        }
        row_images(&mut self.chrome, scene, has, &texts, scale);
    }

    fn status_line(&mut self, scene: &mut Scene, has: &dyn Fn(TextureId) -> bool) {
        let said_of_compiling = self.said_of_compiling();
        let state = self.state();
        let editing = state.editing.as_ref();
        let replacing = editing.is_some_and(|editing| editing.replacing);
        let mode = match (&self.mode, editing.map(|editing| editing.mode)) {
            (_, Some(editor::Mode::Insert)) if replacing => "REPLACE",
            (_, Some(editor::Mode::Insert)) => "INSERT",
            (_, Some(editor::Mode::Visual)) => "VISUAL",
            (_, Some(editor::Mode::VisualLine)) => "V-LINE",
            (_, Some(editor::Mode::Command)) | (Mode::Command(_), None) => "COMMAND",
            _ => "NORMAL",
        };
        let typed = editing.and_then(|editing| Some((editing.prompt, editing.command.as_ref()?)));
        let command = match (&self.mode, typed) {
            (_, Some((prompt, text))) => Some(format!("{prompt}{text}\u{2588}")),
            (Mode::Command(text), None) => Some(format!(":{text}\u{2588}")),
            _ => None,
        };
        let modified = state
            .editing
            .as_ref()
            .is_some_and(|editing| editing.modified);
        let name = self.file_label(modified);
        let problem = self
            .problems
            .first()
            .map(|first| match self.problems.len() {
                1 => first.clone(),
                count => format!("{first}  (+{} more)", count - 1),
            });
        // Said plainly where the text is refused: where, and that the
        // page is behind it.
        let problem = self.refused_status().or(problem);
        // What the compiler makes of half a word is not worth reading:
        // its complaints wait until typing is over. Not in an equation:
        // there the page shows what is typed only once it is one.
        let typing = editing.is_some_and(|editing| editing.mode == editor::Mode::Insert);
        // Asked only where it matters: it is looked for from the root.
        let in_equation = || {
            self.editor.as_ref().is_some_and(|editor| {
                crate::typst_text::equation_at(editor.source(), editor.cursor()).is_some()
            })
        };
        let problem = problem.filter(|_| !typing || in_equation());
        let (left, alert) = match (&command, &self.message, &problem) {
            (Some(command), ..) => (command.as_str(), false),
            (None, Some(message), _) => (message.text.as_str(), message.alert),
            (None, None, Some(problem)) => (problem.as_str(), true),
            (None, None, None) => (name.as_str(), false),
        };
        // Only while the pages are behind the text: where they show what
        // was typed (worked out, or a part typeset by itself), the word
        // would come and go with every key, and all beside it with it.
        let shown = self.is_predicted() || self.document_revision == Some(self.revision);
        let progress = if self.is_compiled() || shown {
            ""
        } else {
            "compiling  "
        };
        let progress = said_of_compiling.unwrap_or_else(|| progress.to_owned());
        let place = editing.map_or_else(String::new, |editing| {
            let recording = editing
                .recording
                .map_or_else(String::new, |register| format!("recording @{register}  "));
            format!("{recording}{}:{}  ", editing.line, editing.column)
        });
        // 100 % is the size at which a point is 1/72 inch on a 96 dpi
        // display, scaled like everything else on a high-density one.
        let percent = state.zoom / (self.display * 96.0 / 72.0) * 100.0;
        let proposals = match self.proposals.len() + self.proposals_elsewhere.len() {
            0 => String::new(),
            1 => "1 proposal ]p  ".to_owned(),
            count => format!("{count} proposals ]p  "),
        };
        let notes = self.notes_status();
        let right = format!(
            "{proposals}{notes}{progress}{place}{}/{}  {percent:.0}%",
            state.page, state.pages
        );
        let right = format!("{}{right}", self.search_count());
        let status = Status {
            mode,
            left,
            right: &right,
            alert,
        };
        let markup = chrome::status_markup(&status, f64::from(self.window.0) / self.scale);
        let y = f64::from(self.window.1) - f64::from(self.status_height());
        self.panel(scene, has, &markup, |_| (0.0, y));
    }

    /// The file's name as the status line shows it, `modified` or not.
    fn file_label(&self, modified: bool) -> String {
        let restored = self.editor.as_ref().and_then(Editor::restored);
        match restored {
            // Until written or dropped: brought back from an earlier
            // session, and perhaps forgotten.
            Some(then) if modified => format!(
                "{} [+ restored, last changed {}]",
                self.file,
                start::ago(then, std::time::SystemTime::now())
            ),
            _ if modified => format!("{} [+]", self.file),
            _ => self.file.clone(),
        }
    }

    fn performance(&mut self, scene: &mut Scene, has: &dyn Fn(TextureId) -> bool) {
        let stats = self.stats;
        let megabytes = stats.texture_bytes as f64 / (1024.0 * 1024.0);
        let rows = [
            ("key to frame", format!("{:.1} ms", stats.latency_ms)),
            ("frame", format!("{:.1} ms", stats.frame_ms)),
            ("frames", stats.frames.to_string()),
            ("compile", format!("{:.1} ms", stats.compile_ms)),
            ("compiles", stats.compiles.to_string()),
            ("tiles rasterized", stats.tiles.to_string()),
            (
                "shown uncompiled",
                format!(
                    "{} of {}",
                    stats.predicted + stats.local,
                    stats.predicted + stats.local + stats.declined
                ),
            ),
            (
                "typeset in parts",
                format!("{}, the last in {:.1} ms", stats.local, stats.local_ms),
            ),
            (
                "last not shown",
                self.declined.clone().unwrap_or_else(|| "–".to_owned()),
            ),
            ("tile memory", format!("{megabytes:.0} MB")),
        ];
        let margin = PANEL_MARGIN * self.scale;
        let right = f64::from(self.window.0) - margin;
        self.panel(scene, has, &chrome::panel_markup(&rows), |(width, _)| {
            (right - width, margin)
        });
    }

    /// The change an agent proposes for the text under the cursor: what
    /// would go and what would come, in the corner above the status line.
    fn proposal_panel(&mut self, scene: &mut Scene, has: &dyn Fn(TextureId) -> bool) {
        if self.proposal_explained() {
            return;
        }
        let Some(cursor) = self.editor.as_ref().map(Editor::cursor) else {
            return;
        };
        let on = |proposal: &&Proposal| {
            proposal.range.contains(&cursor) || proposal.range.start == cursor
        };
        let Some(proposal) = self.proposals.iter().find(on) else {
            return;
        };
        let number = self
            .proposals
            .iter()
            .position(|other| other.id == proposal.id)
            .map_or(0, |at| at + 1);
        let title = format!("proposal {number} of {}", self.proposals.len());
        let hint = if proposal.stale {
            "the text has changed since · gr drops it"
        } else {
            "ga accept · gr reject · ]p next"
        };
        let lines = |text: &str| -> Vec<String> {
            let mut lines: Vec<String> = text
                .lines()
                .take(PROPOSAL_LINES)
                .map(|line| {
                    let mut shown: String = line.chars().take(PROPOSAL_COLUMNS).collect();
                    if shown.len() < line.len() {
                        shown.push('…');
                    }
                    shown
                })
                .collect();
            let more = text.lines().count().saturating_sub(lines.len());
            if more > 0 {
                lines.push(format!("… {more} more"));
            }
            lines
        };
        let markup =
            chrome::proposal_markup(&title, hint, &lines(&proposal.old), &lines(&proposal.new));
        let margin = PANEL_MARGIN * self.scale;
        let right = f64::from(self.window.0) - margin;
        let bottom = f64::from(self.window.1) - f64::from(self.status_height()) - margin;
        let scale = self.scale;
        let Some(image) = self.chrome.render(&markup, scale) else {
            return;
        };
        let (width, height) = (
            f64::from(image.pixmap.width()),
            f64::from(image.pixmap.height()),
        );
        let (x, y) = ((right - width).round(), (bottom - height).round());
        let line = scale.round().max(1.0);
        solid(
            scene,
            has,
            LENS_EDGE,
            (x - line, y - line, width + 2.0 * line, height + 2.0 * line),
        );
        scene.quads.push(Quad {
            texture: image.id,
            x: x as f32,
            y: y as f32,
            width: width as f32,
            height: height as f32,
        });
        if !has(image.id) {
            scene.images.push(image);
        }
    }

    /// Renders chrome `markup` and places it where `position` says, given
    /// the rendered width and height in pixels.
    fn panel(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        markup: &str,
        position: impl FnOnce((f64, f64)) -> (f64, f64),
    ) {
        let Some(image) = self.chrome.render(markup, self.scale) else {
            return;
        };
        let (width, height) = (image.pixmap.width(), image.pixmap.height());
        let (x, y) = position((f64::from(width), f64::from(height)));
        self.pointer.placed = Some((x.round(), y.round(), f64::from(width), f64::from(height)));
        scene.quads.push(Quad {
            texture: image.id,
            x: x.round() as f32,
            y: y.round() as f32,
            width: width as f32,
            height: height as f32,
        });
        if !has(image.id) {
            scene.images.push(image);
        }
    }
}

/// The compositor's name for a page tile.
#[must_use]
pub fn texture(key: TileKey) -> TextureId {
    TextureId(hash128(&("tile", key)))
}

/// The tiles of page `index` covering `region`, taken from `planned` if all
/// of them are there and planned (and remembered) otherwise.
///
/// Planning walks the page frame once per tile to crop and hash it. That is
/// cheap next to rasterizing but not free, and scrolling within tiles that
/// are already known should not pay for it on every frame.
fn tiles_in(
    planned: &mut HashMap<(usize, u32, u32), Tile>,
    index: usize,
    region: PixelRegion,
    plan: impl FnOnce() -> Vec<Tile>,
) -> Vec<Tile> {
    let known: Option<Vec<Tile>> = tile_origins(region)
        .map(|(x, y)| planned.get(&(index, x, y)).cloned())
        .collect();
    known.unwrap_or_else(|| {
        let fresh = plan();
        for tile in &fresh {
            planned.insert((index, tile.region.x, tile.region.y), tile.clone());
        }
        fresh
    })
}

/// Draws the images of `rows` at their positions, making those the
/// compositor does not hold already, all at once.
fn row_images(
    chrome: &mut Chrome,
    scene: &mut Scene,
    has: &dyn Fn(TextureId) -> bool,
    rows: &[(RowText, (f64, f64))],
    scale: f64,
) {
    let mut missing: Vec<RowText> = Vec::new();
    for (row, _) in rows {
        let id = Chrome::row_id(row, scale);
        let known = chrome.size_of(id).is_some() && has(id);
        if !known && !missing.contains(row) {
            missing.push(row.clone());
        }
    }
    scene.images.extend(chrome.render_rows(&missing, scale));
    for (row, (x, y)) in rows {
        let id = Chrome::row_id(row, scale);
        let Some(size) = chrome.size_of(id) else {
            continue;
        };
        scene.quads.push(Quad {
            texture: id,
            x: x.round() as f32,
            y: y.round() as f32,
            width: size.0 as f32,
            height: size.1 as f32,
        });
    }
}

/// Lays a colour that may be partly transparent over a rectangle.
fn tint(
    scene: &mut Scene,
    has: &dyn Fn(TextureId) -> bool,
    rgba: [u8; 4],
    (x, y, width, height): (f64, f64, f64, f64),
) {
    let id = TextureId(hash128(&("tint", rgba)));
    if !has(id)
        && !scene.images.iter().any(|image| image.id == id)
        && let Some(mut pixmap) = Pixmap::new(1, 1)
    {
        pixmap.fill(Color::from_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]));
        scene.images.push(Image {
            id,
            pixmap: Arc::new(pixmap),
        });
    }
    scene.quads.push(Quad {
        texture: id,
        x: x as f32,
        y: y as f32,
        width: width as f32,
        height: height as f32,
    });
}

/// Fills a rectangle with an opaque colour.
fn solid(
    scene: &mut Scene,
    has: &dyn Fn(TextureId) -> bool,
    rgb: [u8; 3],
    (x, y, width, height): (f64, f64, f64, f64),
) {
    let id = TextureId(hash128(&("solid", rgb)));
    if !has(id)
        && !scene.images.iter().any(|image| image.id == id)
        && let Some(mut pixmap) = Pixmap::new(1, 1)
    {
        pixmap.fill(Color::from_rgba8(rgb[0], rgb[1], rgb[2], 255));
        scene.images.push(Image {
            id,
            pixmap: Arc::new(pixmap),
        });
    }
    scene.quads.push(Quad {
        texture: id,
        x: x as f32,
        y: y as f32,
        width: width as f32,
        height: height as f32,
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::time::Duration;

    use zetwerk_typst::FontSet;

    use super::*;
    use crate::keys;

    mod cursor;
    mod equations;
    mod errors;
    mod files;
    mod frames;
    #[path = "keys.rs"]
    mod keyboard;
    mod left_behind;
    mod panels;
    mod parts;
    mod settings;
    mod speed;
    mod typing;
    mod typist;
    mod vim;
    mod waits;

    /// A viewer in an 800×620 window showing `pages` A4 pages.
    fn viewer(pages: usize) -> Viewer {
        let text = vec!["Page"; pages].join("\n#pagebreak()\n");
        let env = Environment::new(FontSet::Embedded);
        let project = Project::in_memory(Arc::clone(&env), PathBuf::from("/"), "main.typ", text)
            .unwrap_or_else(|| panic!("valid main file name"));
        let mut viewer =
            Viewer::new(env, project, (800, 620), 1.0).unwrap_or_else(|| panic!("viewer sets up"));
        compile(&mut viewer);
        viewer
    }

    /// Compiles the viewer's current revision on this thread.
    pub(super) fn compile(viewer: &mut Viewer) {
        let (revision, world) = viewer.snapshot();
        let compiled = zetwerk_typst::compile(&world);
        viewer.compiled(CompileOutput {
            revision,
            world: Arc::new(world),
            compiled,
            duration: Duration::from_millis(3),
        });
    }

    pub(super) fn press(viewer: &mut Viewer, notation: &str) {
        for key in keys::parse(notation).unwrap_or_else(|err| panic!("{err}")) {
            viewer.key(key);
        }
    }

    #[test]
    fn a_new_document_fits_the_window_width() {
        let viewer = viewer(3);
        let state = viewer.state();
        assert_eq!((state.pages, state.page), (3, 1));
        assert_eq!(state.compiled_revision, Some(0));
        assert!(viewer.is_compiled());
        // 800 px for 595.28 pt plus a 10 pt gap on each side.
        assert!(
            (state.zoom - 800.0 / 615.28).abs() < 1e-3,
            "zoom is {}",
            state.zoom
        );
    }

    #[test]
    fn keys_scroll_through_the_document() {
        let mut viewer = viewer(3);
        press(&mut viewer, "G");
        assert_eq!(viewer.state().page, 3);
        press(&mut viewer, "gg");
        assert_eq!(viewer.state().page, 1);
        let top = viewer.view.top;
        press(&mut viewer, "j");
        assert!(viewer.view.top > top);
        press(&mut viewer, "k");
        assert_eq!(viewer.view.top, top);
        press(&mut viewer, "<C-f><C-f>");
        assert_eq!(viewer.state().page, 2);
        // A lone `g` does nothing, and does not turn the next `g` into `gg`
        // after another key came in between.
        press(&mut viewer, "gjg");
        assert_eq!(viewer.state().page, 2);
    }

    #[test]
    fn zoom_keys_and_fit() {
        let mut viewer = viewer(1);
        let fitted = viewer.state().zoom;
        press(&mut viewer, "+");
        assert!((viewer.state().zoom - fitted * 1.25).abs() < 1e-9);
        // Once the user has chosen a zoom, resizing keeps it.
        viewer.resize((1200, 620), 1.0);
        assert!((viewer.state().zoom - fitted * 1.25).abs() < 1e-9);
        press(&mut viewer, "=");
        assert!((viewer.state().zoom - 1200.0 / 615.28).abs() < 1e-3);
        viewer.resize((800, 620), 1.0);
        assert!((viewer.state().zoom - fitted).abs() < 1e-9);
    }

    #[test]
    fn command_line() {
        let mut viewer = viewer(3);
        press(&mut viewer, ":3");
        assert_eq!(viewer.state().mode, Mode::Command("3".to_owned()));
        press(&mut viewer, "<CR>");
        assert_eq!(
            (viewer.state().mode, viewer.state().page),
            (Mode::Normal, 3)
        );

        press(&mut viewer, ":perf<CR>");
        assert!(viewer.state().performance_panel);
        press(&mut viewer, ":nonsense<CR>");
        let message = viewer.message.as_ref().map(|message| message.text.as_str());
        assert_eq!(message, Some("not a command: nonsense"));
        // Backspace on an empty command line closes it; Escape always does.
        press(&mut viewer, ":a<BS><BS>");
        assert_eq!(viewer.state().mode, Mode::Normal);
        press(&mut viewer, ":q<Esc>");
        assert!(!viewer.wants_quit());
        press(&mut viewer, ":q<CR>");
        assert!(viewer.wants_quit());
    }

    #[test]
    fn a_scene_asks_for_missing_tiles_and_draws_held_ones() {
        let mut viewer = viewer(2);
        let nothing = viewer.scene(&|_| false);
        // 800 px wide: two columns of 512 px tiles, two rows in view.
        assert_eq!(nothing.wanted.len(), 4);
        // Backdrop, paper and status line have images to upload.
        assert_eq!(nothing.images.len(), 3);

        let held: HashSet<TextureId> = nothing
            .wanted
            .iter()
            .map(|tile| texture(tile.key))
            .chain(nothing.images.iter().map(|image| image.id))
            .collect();
        let complete = viewer.scene(&|id| held.contains(&id));
        assert!(complete.wanted.is_empty() && complete.images.is_empty());
        // Backdrop, paper, four tiles, status line.
        assert_eq!(complete.quads.len(), 7);
        let last = complete.quads.last().unwrap_or_else(|| panic!("has quads"));
        assert_eq!(
            (last.x, last.y, last.width, last.height),
            (0.0, 600.0, 800.0, 20.0)
        );
    }

    #[test]
    fn old_tiles_stand_in_after_a_zoom_step() {
        let mut viewer = viewer(1);
        let first = viewer.scene(&|_| false);
        let held: HashSet<TextureId> = first.wanted.iter().map(|t| texture(t.key)).collect();
        // Draw once with everything held, so the viewer knows what it showed.
        let _ = viewer.scene(&|id| held.contains(&id));

        press(&mut viewer, "+");
        let zoomed = viewer.scene(&|id| held.contains(&id));
        assert!(!zoomed.wanted.is_empty(), "the new zoom needs new tiles");
        let stand_ins: Vec<&Quad> = zoomed
            .quads
            .iter()
            .filter(|quad| held.contains(&quad.texture))
            .collect();
        // One per tile of the old view; blank tiles share one texture.
        assert_eq!(stand_ins.len(), first.wanted.len());
        // Stretched by the zoom step: a 512 px tile is drawn 640 px wide.
        assert!(
            stand_ins
                .iter()
                .any(|quad| (quad.width - 640.0).abs() < 0.01)
        );
    }

    #[test]
    fn errors_keep_the_last_good_pages() {
        let mut viewer = viewer(2);
        viewer.project.set_main_text("#broken(");
        viewer.revision += 1;
        assert!(!viewer.is_compiled());
        compile(&mut viewer);
        let state = viewer.state();
        assert_eq!(state.pages, 2, "the old pages stay on screen");
        assert_eq!(state.compiled_revision, Some(1));
        assert_eq!(state.problems.len(), 1);
        assert!(
            state.problems[0].starts_with("main.typ:1:"),
            "{:?}",
            state.problems
        );
    }

    #[test]
    fn an_older_compile_never_replaces_a_newer_one() {
        let mut viewer = viewer(1);
        let (_, old_world) = viewer.snapshot();
        viewer.project.set_main_text("A\n#pagebreak()\nB");
        viewer.revision += 1;
        compile(&mut viewer);
        assert_eq!(viewer.state().pages, 2);

        let compiled = zetwerk_typst::compile(&old_world);
        viewer.compiled(CompileOutput {
            revision: 0,
            world: Arc::new(old_world),
            compiled,
            duration: Duration::ZERO,
        });
        assert_eq!(viewer.state().pages, 2);
        assert_eq!(viewer.state().compiled_revision, Some(1));
    }

    /// A fresh directory holding `main.typ` with `text`.
    pub(super) fn directory(text: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "zetwerk-viewer-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("main.typ"), text).unwrap();
        directory
    }

    /// A viewer in a 1000×620 window editing `main.typ` in `directory`,
    /// and what opening the file found.
    pub(super) fn open(directory: &Path) -> (Viewer, Opened) {
        let path = directory.join("main.typ");
        let env = Environment::new(FontSet::Embedded);
        let project = Project::open(Arc::clone(&env), &path).unwrap();
        let (editor, opened) =
            Editor::open(&path, &directory.join("state"), project.main()).unwrap();
        let mut viewer = Viewer::new(env, project, (1000, 620), 1.0)
            .unwrap_or_else(|| panic!("viewer sets up"))
            .editing(editor, &opened);
        viewer.state_dir = Some(directory.join("state"));
        compile(&mut viewer);
        (viewer, opened)
    }

    pub(super) fn editing(text: &str) -> (Viewer, PathBuf) {
        let directory = directory(text);
        (open(&directory).0, directory)
    }

    #[test]
    fn typing_changes_the_text_and_wants_a_compile() {
        let (mut viewer, directory) = editing("world\n");
        assert_eq!(viewer.panes(), Panes::Split);
        assert_eq!(viewer.take_edited(), None);

        press(&mut viewer, "iHello ");
        assert_eq!(viewer.text(), Some("Hello world\n"));
        // So short a text tells too little about its lines for the page
        // to be worked out; the paragraph is typeset by itself instead,
        // and all of it once typing pauses.
        assert_eq!(viewer.take_edited(), Some(Edited::Typing));
        assert!(!viewer.is_compiled());
        let editing = viewer.state().editing.unwrap();
        assert_eq!(editing.mode, editor::Mode::Insert);
        assert_eq!((editing.line, editing.column), (1, 7));
        assert!(editing.modified);

        // Leaving insert mode with changes not yet compiled asks again.
        press(&mut viewer, "<Esc>");
        assert_eq!(viewer.take_edited(), Some(Edited::LeftInsert));
        compile(&mut viewer);
        assert!(viewer.is_compiled());
        press(&mut viewer, "<Esc>");
        assert_eq!(viewer.take_edited(), None);

        press(&mut viewer, "dd");
        assert_eq!(viewer.text(), Some(""));
        assert_eq!(viewer.take_edited(), Some(Edited::Command));
        press(&mut viewer, "u");
        assert_eq!(viewer.text(), Some("Hello world\n"));
        // Nothing has touched the file.
        let file = std::fs::read_to_string(directory.join("main.typ")).unwrap();
        assert_eq!(file, "world\n");
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn writing_and_quitting() {
        let (mut viewer, directory) = editing("one\n");
        let file = || std::fs::read_to_string(directory.join("main.typ")).unwrap();
        press(&mut viewer, "Atwo<Esc>");
        press(&mut viewer, ":q<CR>");
        assert!(!viewer.wants_quit(), "unsaved changes keep the window");
        press(&mut viewer, ":w<CR>");
        assert_eq!(file(), "onetwo\n");
        assert!(!viewer.state().editing.unwrap().modified);
        press(&mut viewer, ":q<CR>");
        assert!(viewer.wants_quit());

        // `ZZ` saves what is unsaved and leaves.
        let (mut viewer, other) = editing("one\n");
        press(&mut viewer, "xZZ");
        assert!(viewer.wants_quit());
        assert_eq!(
            std::fs::read_to_string(other.join("main.typ")).unwrap(),
            "ne\n"
        );
        for directory in [directory, other] {
            let _ = std::fs::remove_dir_all(directory);
        }
    }

    #[test]
    fn unsaved_changes_come_back() {
        let (mut viewer, directory) = editing("one\n");
        // The window is closed, or the machine goes down: nobody said
        // that the changes were to go.
        press(&mut viewer, "otwo<Esc>");
        drop(viewer);
        assert_eq!(
            std::fs::read_to_string(directory.join("main.typ")).unwrap(),
            "one\n"
        );
        let (mut viewer, opened) = open(&directory);
        assert_eq!(opened, Opened::Restored);
        assert_eq!(viewer.text(), Some("one\ntwo\n"));
        // And can still be undone.
        press(&mut viewer, "u");
        assert_eq!(viewer.text(), Some("one\n"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_change_on_disk_never_costs_unsaved_text() {
        let (mut viewer, directory) = editing("one\n");
        let path = directory.join("main.typ");
        // Nothing unsaved: the new text is taken over, and can be undone.
        std::fs::write(&path, "from outside\n").unwrap();
        assert!(viewer.file_changed(&path));
        assert_eq!(viewer.text(), Some("from outside\n"));
        press(&mut viewer, "u");
        assert_eq!(viewer.text(), Some("one\n"));
        press(&mut viewer, "<C-r>");

        // Something unsaved: both sides stay as they are.
        press(&mut viewer, "x");
        std::fs::write(&path, "again from outside\n").unwrap();
        assert!(!viewer.file_changed(&path));
        assert_eq!(viewer.text(), Some("rom outside\n"));
        press(&mut viewer, ":w<CR>");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "again from outside\n"
        );
        press(&mut viewer, ":w!<CR>");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "rom outside\n");
        // Its own write is not a change from outside.
        assert!(!viewer.file_changed(&path));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn searching_moves_the_cursor() {
        let (mut viewer, directory) = editing("one two\nthree two\n");
        press(&mut viewer, "/two");
        let editing = viewer.state().editing.unwrap();
        assert_eq!(
            (editing.prompt, editing.command.as_deref()),
            ('/', Some("two"))
        );
        press(&mut viewer, "<CR>");
        let place = |viewer: &Viewer| {
            let editing = viewer.state().editing.unwrap();
            (editing.line, editing.column)
        };
        assert_eq!(place(&viewer), (1, 5));
        press(&mut viewer, "n");
        assert_eq!(place(&viewer), (2, 7));
        press(&mut viewer, "N");
        assert_eq!(place(&viewer), (1, 5));
        press(&mut viewer, "cwdrei<Esc>n.");
        assert_eq!(viewer.text(), Some("one drei\nthree drei\n"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_view_can_be_moved_and_moved_in() {
        let lines = "a line\n".repeat(200);
        let (mut viewer, directory) = editing(&lines);
        let held: HashSet<TextureId> = HashSet::new();
        let line = |viewer: &Viewer| viewer.state().editing.unwrap().line;
        let draw = |viewer: &mut Viewer| {
            viewer.scene(&|id| held.contains(&id));
        };
        draw(&mut viewer);
        press(&mut viewer, "L");
        let bottom = line(&viewer);
        assert!(bottom > 20, "the pane shows {bottom} lines");
        press(&mut viewer, "H");
        assert_eq!(line(&viewer), 1);
        press(&mut viewer, "M");
        assert_eq!(line(&viewer), 1 + (bottom - 1) / 2);
        // The cursor's line to the top; the lines kept in view around the
        // cursor stay above it.
        press(&mut viewer, "Lzt");
        draw(&mut viewer);
        press(&mut viewer, "H");
        assert_eq!(line(&viewer), bottom - SCROLL_MARGIN);
        // A page down and back.
        press(&mut viewer, "gg");
        draw(&mut viewer);
        press(&mut viewer, "<C-f>");
        assert_eq!(line(&viewer), bottom - 1);
        press(&mut viewer, "<C-b>");
        assert_eq!(line(&viewer), 1);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_cursor_is_shown_on_the_page() {
        let (mut viewer, directory) = editing("Hello *there* world.\n");
        let held: HashSet<TextureId> = HashSet::new();
        let count = |viewer: &mut Viewer, rgba: [u8; 4]| {
            let id = TextureId(hash128(&("tint", rgba)));
            let scene = viewer.scene(&|id| held.contains(&id));
            scene.quads.iter().filter(|quad| quad.texture == id).count()
        };
        assert_eq!(count(&mut viewer, PAGE_CURSOR), 1);
        assert_eq!(count(&mut viewer, PAGE_SELECTION), 0);
        // On markup that sets no glyph of its own there is no cursor to
        // show; on the text after it there is again.
        press(&mut viewer, "f*");
        assert_eq!(count(&mut viewer, PAGE_CURSOR), 0);
        press(&mut viewer, "l");
        assert_eq!(count(&mut viewer, PAGE_CURSOR), 1);
        // A selection over two words and the markup between them.
        press(&mut viewer, "0vee");
        assert!(count(&mut viewer, PAGE_SELECTION) >= 1);
        press(&mut viewer, "<Esc>");
        // After an edit the page is that of the text before it: the cursor
        // is shown where its text stands on that.
        press(&mut viewer, "0x");
        assert!(!viewer.is_compiled());
        assert_eq!(count(&mut viewer, PAGE_CURSOR), 1);
        compile(&mut viewer);
        assert_eq!(count(&mut viewer, PAGE_CURSOR), 1);
        // In insert mode the cursor is a bar.
        press(&mut viewer, "i");
        assert_eq!(count(&mut viewer, PAGE_BAR), 1);
        // Text that does not compile leaves the old page, with the cursor
        // on it as on any page that is behind.
        press(&mut viewer, "<Esc>f*x");
        compile(&mut viewer);
        assert!(viewer.is_compiled() && !viewer.state().problems.is_empty());
        assert_eq!(count(&mut viewer, PAGE_CURSOR), 1);
        // Two changes in two places: where things are is anyone's guess.
        press(&mut viewer, "$x");
        assert_eq!(count(&mut viewer, PAGE_CURSOR), 0);
        let _ = std::fs::remove_dir_all(directory);
    }

    /// The quads of one colour laid over the pages.
    pub(super) fn tints(viewer: &mut Viewer, rgba: [u8; 4]) -> Vec<Quad> {
        let id = TextureId(hash128(&("tint", rgba)));
        let scene = viewer.scene(&|_| false);
        let of_colour = |quad: &&Quad| quad.texture == id;
        scene.quads.iter().filter(of_colour).copied().collect()
    }

    #[test]
    fn what_a_reference_shows_is_one_thing_under_the_cursor() {
        let text = "#set heading(numbering: \"1.\")\n\
                    = Start <start>\n\
                    As @start says, more or less.\n";
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "/As<CR>");
        let letter = tints(&mut viewer, PAGE_CURSOR);
        assert_eq!(letter.len(), 1);
        // On the reference: all of "Section 1", wherever in it.
        press(&mut viewer, "/@<CR>");
        let whole = tints(&mut viewer, PAGE_CURSOR);
        assert_eq!(whole.len(), 1);
        assert!(whole[0].width > 4.0 * letter[0].width, "{whole:?}");
        press(&mut viewer, "lll");
        assert_eq!(tints(&mut viewer, PAGE_CURSOR), whole);
        // And past it, a letter again.
        press(&mut viewer, "w");
        let after = tints(&mut viewer, PAGE_CURSOR);
        assert!(after[0].x > whole[0].x + whole[0].width && after[0].width < whole[0].width);
        // A click on it goes to the reference.
        let (x, y) = (f64::from(whole[0].x) + 3.0, f64::from(whole[0].y) + 3.0);
        viewer.click((x, y));
        let cursor = viewer.editor.as_ref().unwrap().cursor();
        assert_eq!(&text[cursor..cursor + 6], "@start");
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_cursor_moves_on_a_page_that_is_behind() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    The quick brown fox jumps over the lazy dog and runs on \
                    through the wood until the line is full, and then some \
                    more of it follows here to make a few lines of text, \
                    the last of which is short.\n";
        let (mut viewer, directory) = editing(text);
        let cursor = |viewer: &Viewer| viewer.editor.as_ref().unwrap().cursor();
        // A star is no word of a line; with typesetting in parts off,
        // there is nothing to show without compiling.
        viewer.in_parts = false;
        press(&mut viewer, "gsgs/quick<CR>i*<Esc>");
        assert!(!viewer.is_compiled() && !viewer.is_predicted());
        // The star itself is on no page.
        assert_eq!(tints(&mut viewer, PAGE_CURSOR).len(), 0);
        press(&mut viewer, "l");
        let before = tints(&mut viewer, PAGE_CURSOR);
        assert_eq!(before.len(), 1, "the cursor is on the page as it was");
        let start = cursor(&viewer);
        press(&mut viewer, "j");
        assert!(cursor(&viewer) > start + 20, "down a line of the page");
        assert_eq!(viewer.editor.as_ref().unwrap().position().0, 2);
        let below = tints(&mut viewer, PAGE_CURSOR);
        assert!(below[0].y > before[0].y + 5.0);
        assert!(
            (below[0].x - before[0].x).abs() < 12.0,
            "{before:?} {below:?}"
        );
        press(&mut viewer, "k");
        assert_eq!(cursor(&viewer), start);
        // A click finds the text that was under it before the star.
        let (x, y) = (f64::from(below[0].x) + 1.0, f64::from(below[0].y) + 3.0);
        press(&mut viewer, "j");
        let below = cursor(&viewer);
        press(&mut viewer, "gg");
        viewer.click((x, y));
        assert_eq!(cursor(&viewer), below);
        compile(&mut viewer);
        assert_eq!(tints(&mut viewer, PAGE_CURSOR).len(), 1);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn what_is_typed_on_the_page_is_what_is_read() {
        let text = "#set page(width: 300pt, height: 300pt)\nSome words.\n\nMore.\n";
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "/words<CR>gsgs");
        press(&mut viewer, "A Use a_b * c [1] <lt>x> of me@x.de");
        let typed = viewer.text().unwrap();
        assert!(
            typed.contains("Use a\\_b \\* c \\[1\\] \\<x> of me\\@x.de"),
            "{typed:?}"
        );
        compile(&mut viewer);
        assert_eq!(viewer.document_revision, Some(viewer.revision));
        // Strong and emphasised text is begun and left by a key.
        press(
            &mut viewer,
            " and <C-b>bold<C-b> or <C-i>so<C-i>, b<C-b>ut<C-b>.",
        );
        let typed = viewer.text().unwrap();
        assert!(
            typed.contains(" and *bold* or _so_, b#strong[ut]."),
            "{typed:?}"
        );
        compile(&mut viewer);
        assert_eq!(viewer.document_revision, Some(viewer.revision));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn deleting_in_an_equation_leaves_an_equation() {
        let text = "#set page(width: 300pt, height: 300pt)\nBefore.\n\n\
                    $ y_i = lambda x_i + (1 - lambda) x_(i+1) $\n\nAfter.\n";
        let (mut viewer, directory) = editing(text);
        let equation = |viewer: &Viewer| {
            let text = viewer.text().unwrap_or_default().to_owned();
            let from = text.find('$').unwrap_or(0);
            let to = text.rfind('$').map_or(text.len(), |at| at + 1);
            text[from..to].to_owned()
        };
        // Backwards through all of it while typing: what is raised or
        // lowered goes, and with the last of it the mark that did it.
        press(&mut viewer, "/i+1<CR>gsgsA");
        let mut seen = Vec::new();
        for _ in 0..10 {
            press(&mut viewer, "<BS>");
            compile(&mut viewer);
            seen.push(equation(&viewer));
            assert_eq!(
                viewer.document_revision,
                Some(viewer.revision),
                "no equation: {seen:?}"
            );
        }
        assert!(
            seen.contains(&"$ y_i = lambda x_i + (1 - lambda) x_() $".to_owned()),
            "{seen:?}"
        );
        assert!(
            seen.contains(&"$ y_i = lambda x_i + (1 - lambda) x $".to_owned()),
            "{seen:?}"
        );
        // A symbol written with several letters goes as the one thing
        // it shows as.
        assert_eq!(
            equation(&viewer).trim_end_matches([' ', '$']),
            "$ y_i = lambda x_i +",
            "{seen:?}"
        );
        // And with `x` on the page: nothing is left of what it took.
        press(&mut viewer, "<Esc>u");
        compile(&mut viewer);
        press(&mut viewer, "/i+1<CR>");
        press(&mut viewer, "xxx");
        compile(&mut viewer);
        assert_eq!(equation(&viewer), "$ y_i = lambda x_i + (1 - lambda) x $");
        assert_eq!(viewer.document_revision, Some(viewer.revision));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn an_equation_that_stands_by_itself_stays_so_while_it_is_typed() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    #set math.equation(numbering: \"(1)\")\n\
                    Before.\n\n$ a + b $\n\nAfter.\n";
        let (mut viewer, directory) = editing(text);
        // Where the equation and its number stand, and where they stand
        // once all of it has been typeset again.
        press(&mut viewer, "gsgs/\\=+ b<CR>la + 2");
        let whole = |viewer: &mut Viewer| {
            compile(viewer);
            glyph_places(viewer, 0)
        };
        let before = glyph_places(&viewer, 0);
        assert_eq!(before, whole(&mut viewer));
        // What is raised is yet to come: the equation is as much in
        // the middle as before, and has its number.
        press(&mut viewer, "^");
        assert_eq!(
            viewer.document_revision,
            Some(viewer.revision),
            "{:?}",
            viewer.declined
        );
        let raised = glyph_places(&viewer, 0);
        assert_eq!(raised, whole(&mut viewer));
        assert_eq!(
            raised.len(),
            before.len() + 1,
            "the box for what is to come"
        );
        let number = |places: &[(i64, i64)]| places.iter().map(|place| place.0).max();
        assert_eq!(
            number(&raised),
            number(&before),
            "the number stands where it stood"
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn letters_typed_together_in_an_equation_are_what_is_meant() {
        let text = "#set page(width: 300pt, height: 300pt)\nSome words\n\nMore.\n";
        let (mut viewer, directory) = editing(text);
        // `xy` is no name: it is x times y, as whoever types it means.
        // A name is a name, and so is one that is still being typed.
        press(&mut viewer, "/words<CR>gsgsA and $xy + alpha = abc");
        assert!(
            viewer
                .text()
                .is_some_and(|text| text.contains("$x y + alpha = abc$")),
            "{:?}",
            viewer.text()
        );
        press(&mut viewer, "<Esc>");
        assert!(
            viewer
                .text()
                .is_some_and(|text| text.contains("$x y + alpha = a b c$")),
            "{:?}",
            viewer.text()
        );
        compile(&mut viewer);
        assert_eq!(viewer.document_revision, Some(viewer.revision));
        assert_eq!(viewer.problems, Vec::<String>::new());
        // All of it is one step to take back.
        press(&mut viewer, "u");
        assert!(viewer.text().is_some_and(|text| !text.contains('$')));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn an_equation_typst_refuses_is_shown_as_it_is_written() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    Before $a + \"b$ after.\n\nMore words.\n";
        let (mut viewer, directory) = editing(text);
        // Typst has no meaning for it. It is shown as its characters
        // then, and all the rest is typeset around it.
        compile(&mut viewer);
        assert!(viewer.written.is_some(), "{:?}", viewer.problems);
        compile(&mut viewer);
        assert_eq!(
            viewer.document_revision,
            Some(viewer.revision),
            "{:?}",
            viewer.problems
        );
        let shown = glyph_places(&viewer, 0).len();
        // Each of its characters is a place for the cursor: what is in
        // the way can be gone to and taken away, on the page.
        press(&mut viewer, "gsgs/\\=\"b<CR>");
        assert_eq!(tints(&mut viewer, PAGE_CURSOR).len(), 1);
        press(&mut viewer, "x");
        assert!(
            viewer
                .text()
                .is_some_and(|text| text.contains("Before $a + b$ after.")),
            "{:?}",
            viewer.text()
        );
        // It is an equation again, and shown as one, at once.
        assert_eq!(viewer.written, None);
        assert_eq!(
            viewer.document_revision,
            Some(viewer.revision),
            "{:?}",
            viewer.declined
        );
        let meant = glyph_places(&viewer, 0).len();
        assert!(meant < shown, "no marks of the equation are shown");
        // For the asking, an equation is shown as written too, while
        // the cursor is in it.
        press(&mut viewer, "gx");
        compile(&mut viewer);
        assert!(viewer.written.is_some());
        assert!(
            glyph_places(&viewer, 0).len() > meant + 3,
            "its marks and blanks are shown: {:?}",
            viewer.shown_put
        );
        press(&mut viewer, "gx");
        compile(&mut viewer);
        assert_eq!(viewer.written, None);
        press(&mut viewer, "gx/More<CR>");
        compile(&mut viewer);
        assert_eq!(viewer.written, None, "left behind, it is what it means");
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_cursor_is_behind_the_blank_that_was_typed() {
        let text = "#set page(width: 300pt, height: 300pt)\nSome words\n\nMore.\n";
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "/words<CR>gsgsA");
        let behind_word = tints(&mut viewer, PAGE_BAR);
        assert_eq!(behind_word.len(), 1);
        // A blank at the end of a line shows as nothing; the cursor
        // moves on by its width all the same, and stays to be seen.
        press(&mut viewer, " ");
        let behind_blank = tints(&mut viewer, PAGE_BAR);
        assert_eq!(behind_blank.len(), 1, "{:?}", viewer.declined);
        assert!(behind_blank[0].x > behind_word[0].x + 1.0);
        assert!(behind_blank[0].x < behind_word[0].x + 12.0);
        press(&mut viewer, " ");
        let further = tints(&mut viewer, PAGE_BAR);
        assert!(further[0].x > behind_blank[0].x + 1.0);
        // And once the page is that of the text, too.
        compile(&mut viewer);
        let compiled = tints(&mut viewer, PAGE_BAR);
        assert_eq!(compiled.len(), 1);
        assert!(
            (compiled[0].x - further[0].x).abs() < 2.0,
            "{compiled:?} {further:?}"
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_lines_say_how_far_they_are_from_the_cursor() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    One line.\n\nTwo lines.\n\nThree lines.\n\nFour lines.\n";
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "/Two<CR>gsgs");
        let numbered = |viewer: &mut Viewer| {
            let _ = viewer.scene(&|_| false);
            let (_, mut numbered) = viewer.numbered.clone().unwrap_or_default();
            numbered.sort_unstable();
            numbered
                .into_iter()
                .map(|(_, line, away)| (line, away))
                .collect::<Vec<_>>()
        };
        assert_eq!(numbered(&mut viewer), [(0, 1), (1, 0), (2, 1), (3, 2)]);
        // The number is the one `j` wants.
        press(&mut viewer, "2j");
        assert_eq!(numbered(&mut viewer), [(0, 3), (1, 2), (2, 1), (3, 0)]);
        press(&mut viewer, ":numbers<CR>");
        let before = viewer.scene(&|_| false).quads.len();
        press(&mut viewer, ":numbers<CR>");
        assert!(viewer.scene(&|_| false).quads.len() > before);
        // While something is typed the pages are behind it after each
        // key (here for good: nothing compiles them, and nothing is
        // typeset by itself). The numbers stay where they stood; they do
        // not go and come with every letter.
        let stood = numbered(&mut viewer);
        press(&mut viewer, ":parts<CR>A more");
        assert!(viewer.places().is_none(), "the pages are behind");
        assert_eq!(numbered(&mut viewer), stood);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_interface_is_as_large_as_asked() {
        let (mut viewer, directory) = editing("Some words.\n");
        let usual = viewer.status_height();
        press(&mut viewer, ":ui 2<CR>");
        assert_eq!(viewer.status_height(), 2 * usual);
        press(&mut viewer, ":ui 150%<CR>");
        assert!(viewer.status_height() > usual && viewer.status_height() < 2 * usual);
        press(&mut viewer, ":ui 1<CR>:ui +<CR>");
        assert!(viewer.status_height() > usual);
        press(&mut viewer, ":ui -<CR>");
        assert_eq!(viewer.status_height(), usual);
        // Another display's scale does not undo it.
        press(&mut viewer, ":ui 2<CR>");
        viewer.resize((1000, 800), 1.0);
        assert_eq!(viewer.status_height(), 2 * usual);
        press(&mut viewer, ":ui huge<CR>");
        assert_eq!(viewer.status_height(), 2 * usual);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_document_is_set_up_by_commands() {
        let (mut viewer, directory) = editing("Some words.\n");
        press(&mut viewer, ":fontsize 12<CR>");
        assert_eq!(viewer.text(), Some("#set text(size: 12pt)\nSome words.\n"));
        press(&mut viewer, ":justify<CR>:columns 2<CR>:justify<CR>");
        assert_eq!(
            viewer.text(),
            Some(
                "#set page(columns: 2)\n#set par(justify: false)\n\
                 #set text(size: 12pt)\nSome words.\n"
            )
        );
        press(&mut viewer, "u");
        assert!(
            viewer
                .text()
                .is_some_and(|text| text.contains("justify: true"))
        );
        compile(&mut viewer);
        assert_eq!(viewer.document_revision, Some(viewer.revision));
        // Everything there is to do is in a list; what needs more said
        // asks for it.
        press(&mut viewer, "g?");
        assert!(viewer.is_picking());
        press(&mut viewer, "margins<CR>");
        assert!(!viewer.is_picking());
        press(&mut viewer, "3<CR>");
        assert!(
            viewer
                .text()
                .is_some_and(|text| text.contains("#set page(columns: 2, margin: 3cm)")),
            "{:?}",
            viewer.text()
        );
        press(&mut viewer, ":font<CR>");
        assert!(viewer.is_picking());
        press(&mut viewer, "<Esc>:margin wide<CR>");
        assert!(
            viewer
                .text()
                .is_some_and(|text| text.contains("margin: 3cm"))
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_document_manager_decides_how_the_document_looks() {
        let (mut viewer, directory) = editing("Some words.\n");
        let has =
            |viewer: &Viewer, rule: &str| viewer.text().is_some_and(|text| text.contains(rule));
        // One of two is switched at once; the list stays up for more.
        press(&mut viewer, "gDjustified<CR>");
        assert!(
            has(&viewer, "#set par(justify: true)"),
            "{:?}",
            viewer.text()
        );
        assert!(viewer.is_picking());
        // What it can be is listed; anything else is typed.
        press(&mut viewer, "font size<CR>12<CR>");
        assert!(has(&viewer, "#set text(size: 12pt)"), "{:?}", viewer.text());
        press(&mut viewer, "font size<CR>13.5<CR>");
        assert!(
            has(&viewer, "#set text(size: 13.5pt)"),
            "{:?}",
            viewer.text()
        );
        press(&mut viewer, "numbers stand<CR>top, right<CR>");
        assert!(
            has(&viewer, "number-align: right + top"),
            "{:?}",
            viewer.text()
        );
        press(&mut viewer, "running<CR>My paper<CR>");
        assert!(has(&viewer, "header: [My paper]"), "{:?}", viewer.text());
        // Escape leads back to the list, then away.
        press(&mut viewer, "paper<CR>");
        assert!(viewer.is_picking());
        press(&mut viewer, "<Esc>");
        assert!(viewer.is_picking());
        press(&mut viewer, "<Esc>");
        assert!(!viewer.is_picking());
        compile(&mut viewer);
        assert_eq!(viewer.document_revision, Some(viewer.revision));
        // What cannot be is not written.
        press(&mut viewer, ":doc<CR>columns<CR>many<CR><Esc>");
        assert!(!has(&viewer, "columns"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn what_is_selected_on_a_page_is_made_strong_by_the_key_that_types_strong() {
        let (mut viewer, directory) = editing("Some plain words here.\n");
        press(&mut viewer, "/plain<CR>gsgsviw<C-b>");
        assert_eq!(viewer.text(), Some("Some *plain* words here.\n"));
        compile(&mut viewer);
        press(&mut viewer, "/words<CR>ve<C-i>");
        assert_eq!(viewer.text(), Some("Some *plain* _words_ here.\n"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_bibliography_manager_keeps_the_sources() {
        let (mut viewer, directory) = editing("Some words here.\n");
        // A source is added: its kind is chosen, what is known is typed.
        press(&mut viewer, "gB");
        assert!(viewer.is_picking());
        press(&mut viewer, "<CR>book<CR>");
        press(&mut viewer, "<CR>Donald Knuth<CR>");
        press(&mut viewer, "<CR>The TeXbook<CR>");
        press(&mut viewer, "year<CR>1984<CR>");
        press(&mut viewer, "<Esc><Esc>");
        assert!(!viewer.is_picking());
        assert_eq!(
            viewer.text(),
            Some("Some words here.\n\n#bibliography(\"refs.bib\")\n")
        );
        let sources = viewer.sources().unwrap();
        assert_eq!(sources.entries.len(), 1);
        let knuth = &sources.entries[0];
        assert_eq!(
            (knuth.key.as_str(), knuth.kind.as_str()),
            ("knuth1984", "book")
        );
        assert_eq!(knuth.field("title"), Some("The TeXbook"));
        // It is cited from the list, behind the word the cursor is on.
        press(&mut viewer, "/words<CR>gBknuth<CR>cite<CR>");
        assert!(
            viewer
                .text()
                .is_some_and(|text| text.starts_with("Some words @knuth1984 here.")),
            "{:?}",
            viewer.text()
        );
        compile(&mut viewer);
        assert_eq!(viewer.document_revision, Some(viewer.revision));
        // Agents add sources as text; a key that is taken is not kept.
        let entry = "@article{knuth1984, author = {Ada Lovelace}, title = {Notes}, year = {1843}}";
        assert_eq!(
            viewer.agent_add_sources(entry),
            Ok(vec!["lovelace1843".to_owned()])
        );
        assert!(viewer.agent_add_sources("no entry").is_err());
        assert_eq!(viewer.sources().unwrap().entries.len(), 2);
        // Where agents only propose, it is a proposal: nothing is added.
        viewer.agent_edits = AgentEdits::Propose;
        assert_eq!(viewer.agent_add_sources(entry), Ok(Vec::new()));
        assert_eq!(viewer.sources().unwrap().entries.len(), 2);
        let (proposed, _) = viewer.proposed_sources()[0].clone();
        viewer.execute(&format!("bib reject {proposed}"));
        press(&mut viewer, "<Esc>");
        // Taken out, after being asked.
        press(&mut viewer, "gBlovelace<CR>take<CR>yes<CR><Esc>");
        let left = viewer.sources().unwrap();
        assert_eq!(left.entries.len(), 1);
        assert_eq!(left.entries[0].key, "knuth1984");
        let _ = std::fs::remove_dir_all(directory);
    }

    /// Where every glyph of page `index` stands, to a tenth of a point.
    fn glyph_places(viewer: &Viewer, index: usize) -> Vec<(i64, i64)> {
        let (Some(document), Some(world)) = (&viewer.document, &viewer.compiled_world) else {
            return Vec::new();
        };
        let map = PageMap::build(world, &document.pages()[index].frame);
        let tenth = |length: typst::layout::Abs| (length.to_pt() * 10.0).round() as i64;
        map.glyphs()
            .iter()
            .map(|glyph| (tenth(glyph.rect.min.x), tenth(glyph.rect.min.y)))
            .collect()
    }

    #[test]
    fn what_changed_is_typeset_by_itself_as_the_whole_would_set_it() {
        let text = "#set page(width: 300pt, height: 400pt)\n\
                    = Title\n\
                    First paragraph with words that go on for more than one line of the page.\n\n\
                    Second $x^2$ one.\n\n\
                    - item one\n- item two\n\n\
                    Last paragraph.\n";
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "/x<CR>gsgs");
        // Each of these keys changes more than a word of a line: the
        // paragraph is typeset by itself, at once, and the page shows
        // the text as it is without the whole having been typeset.
        let keys = [
            "a", "_", "i", " ", "+", " ", "f", "f", "<Tab>", "a", "<Tab>", "b",
        ];
        for key in keys {
            press(&mut viewer, key);
            assert_eq!(
                viewer.document_revision,
                Some(viewer.revision),
                "after {key}: {:?}",
                viewer.declined
            );
        }
        assert!(!viewer.is_compiled());
        assert!(viewer.stats.local >= 8, "{}", viewer.stats.local);
        assert_eq!(tints(&mut viewer, PAGE_BAR).len(), 1);
        // And as the whole sets it: every glyph where it will be.
        let alone = glyph_places(&viewer, 0);
        compile(&mut viewer);
        assert_eq!(alone, glyph_places(&viewer, 0));
        // So with a paragraph begun behind another, and a heading.
        press(
            &mut viewer,
            "<Esc>/Second<CR>A<CR>A new paragraph, with *strong* words in it.<CR>== Part",
        );
        assert_eq!(
            viewer.document_revision,
            Some(viewer.revision),
            "{:?}",
            viewer.declined
        );
        let alone = glyph_places(&viewer, 0);
        compile(&mut viewer);
        assert_eq!(alone, glyph_places(&viewer, 0));
        // What is before everything changes everything: that waits.
        press(&mut viewer, "<Esc>:fontsize 9<CR>");
        assert_ne!(viewer.document_revision, Some(viewer.revision));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn tables_grow_and_shrink_by_keys() {
        let text = "#table(\n  columns: 2,\n  [a], [b],\n  [c], [d],\n)\n\nAfter.\n";
        let (mut viewer, directory) = editing(text);
        let table = |viewer: &Viewer| {
            let text = viewer.text().unwrap_or_default();
            text[..text.find("\n\nAfter").unwrap_or(text.len())].to_owned()
        };
        press(&mut viewer, "/d<CR>:table column<CR>");
        assert_eq!(
            table(&viewer),
            "#table(\n  columns: 3,\n  [a], [b], [],\n  [c], [d], [],\n)"
        );
        press(&mut viewer, "u/b]<CR>:table delete row<CR>");
        assert_eq!(table(&viewer), "#table(\n  columns: 2,\n  [c], [d],\n)");
        press(&mut viewer, ":table delete row<CR>");
        assert_eq!(table(&viewer), "#table(\n  columns: 2,\n  [c], [d],\n)");
        // While typing: Tab behind the last cell makes a row, Backspace
        // takes one nothing was typed into, Enter leaves the table.
        compile(&mut viewer);
        press(&mut viewer, "gsgs/d<CR>a<Tab>e<Tab>f<Tab>");
        assert_eq!(
            table(&viewer),
            "#table(\n  columns: 2,\n  [c], [d],\n  [e], [f],\n  [], [],\n)"
        );
        press(&mut viewer, "<BS>!<CR> Then");
        assert_eq!(
            viewer.text(),
            Some("#table(\n  columns: 2,\n  [c], [d],\n  [e], [f!],\n) Then\n\nAfter.\n")
        );
        press(&mut viewer, "/After<CR>:table row<CR>");
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn an_equation_left_unfinished_does_not_take_the_document_down() {
        let text = "#set page(width: 300pt, height: 300pt)\nA $x^2$ b.\n\nMore words.\n";
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "/2<CR>gsgsa + ff<Tab><Esc>");
        compile(&mut viewer);
        press(&mut viewer, "/More<CR>A and more<Esc>");
        compile(&mut viewer);
        assert!(
            viewer
                .text()
                .is_some_and(|text| text.contains("$x^2 + frac(, )$") && text.contains("and more")),
            "{:?}",
            viewer.text()
        );
        assert_eq!(viewer.document_revision, Some(viewer.revision));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn what_the_user_wants_to_put_in_is_in_the_menu() {
        let (mut viewer, directory) = editing("Some words.\n");
        viewer.inserts = vec![crate::typst_text::Insert {
            name: "diagram",
            short: "",
            body: "#diagram(|)",
            block: true,
        }];
        press(&mut viewer, "gsgsA<CR>#dia<CR>x");
        assert_eq!(viewer.text(), Some("Some words.\n\n#diagram(x)\n"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_cursor_keys_go_by_the_page_while_typing() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    One $alpha + beta$ two.\n\nSecond paragraph here.\n";
        let (mut viewer, directory) = editing(text);
        let cursor = |viewer: &Viewer| viewer.editor.as_ref().map_or(0, Editor::cursor);
        press(&mut viewer, "/One<CR>gsgsA");
        // A symbol written with several letters is one step.
        press(&mut viewer, "<Left><Left><Left><Left><Left><Left>");
        assert_eq!(cursor(&viewer), text.find("beta").unwrap_or(0));
        press(&mut viewer, "<Left><Left>");
        assert_eq!(cursor(&viewer), text.find("alpha").unwrap_or(0));
        press(&mut viewer, "<Right>");
        assert_eq!(cursor(&viewer), text.find(" + beta").unwrap_or(0));
        // To the ends of the typeset line, and down a line of the page.
        press(&mut viewer, "<Home>");
        assert_eq!(cursor(&viewer), text.find("One").unwrap_or(0));
        press(&mut viewer, "<End>");
        assert_eq!(cursor(&viewer), text.find("two.").unwrap_or(0) + 4);
        press(&mut viewer, "<Home><Down>!");
        assert!(
            viewer
                .text()
                .is_some_and(|text| text.contains("\n\n!Second paragraph here.")),
            "{:?}",
            viewer.text()
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn an_equation_is_typed_where_it_stands() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    Some words and $x^2 + y$ in a line.\n\n\
                    = A heading\n";
        let (mut viewer, directory) = editing(text);
        let lens = |viewer: &mut Viewer| {
            let edge = TextureId(hash128(&("solid", LENS_EDGE)));
            let scene = viewer.scene(&|_| false);
            let quads = scene.quads;
            quads.into_iter().find(|quad| quad.texture == edge)
        };
        // Its source is not shown: not for a cursor that stands on it,
        // and not while it is typed.
        press(&mut viewer, "/y<CR>gsgs");
        assert_eq!(lens(&mut viewer), None);
        assert_eq!(tints(&mut viewer, PAGE_CURSOR).len(), 1);
        // Half an equation is none to Typst. The page shows it all the
        // same, with a box for what it lacks, and the cursor in it.
        press(&mut viewer, "a^");
        compile(&mut viewer);
        assert_eq!(viewer.document_revision, Some(viewer.revision));
        assert!(viewer.text().unwrap().contains("$x^2 + y^$"));
        assert_eq!(lens(&mut viewer), None);
        assert_eq!(tints(&mut viewer, PAGE_BAR).len(), 1);
        // So with the parts of a fraction, and Tab leads through them.
        press(&mut viewer, "n + ff<Tab>");
        compile(&mut viewer);
        assert_eq!(viewer.document_revision, Some(viewer.revision));
        press(&mut viewer, "a<Tab>b<Tab><Tab> and");
        compile(&mut viewer);
        assert!(
            viewer
                .text()
                .unwrap()
                .contains("$x^2 + y^n + frac(a, b)$ and in a line."),
            "{:?}",
            viewer.text()
        );
        // A word that is not yet a name is shown as the word.
        press(&mut viewer, "<Esc>/b<CR>a + alp");
        compile(&mut viewer);
        assert_eq!(viewer.document_revision, Some(viewer.revision));
        assert_eq!(lens(&mut viewer), None);
        press(&mut viewer, "<Esc>");
        press(&mut viewer, "/words<CR>inew ");
        assert_eq!(lens(&mut viewer), None);
        // The cursor does not rest on code that shows nowhere.
        press(&mut viewer, "<Esc>");
        compile(&mut viewer);
        press(&mut viewer, "gg");
        assert_eq!(tints(&mut viewer, PAGE_CURSOR).len(), 1);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn typing_shows_on_the_page_without_a_compile() {
        // A paragraph of several lines tells how wide a line may get.
        let text = "#set page(width: 300pt, height: 200pt)\n\
                    The quick brown fox jumps.\n\n\
                    Second paragraph here, which goes on for long enough to \
                    be broken into more than one line of the page.\n";
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "/brown<CR>ivery ");
        assert!(viewer.is_predicted(), "{:?}", viewer.declined);
        assert_eq!(viewer.take_edited(), Some(Edited::Typing));
        // The cursor is shown behind what was typed.
        let held: HashSet<TextureId> = HashSet::new();
        let bar = TextureId(hash128(&("tint", PAGE_BAR)));
        let scene = viewer.scene(&|id| held.contains(&id));
        assert_eq!(
            scene
                .quads
                .iter()
                .filter(|quad| quad.texture == bar)
                .count(),
            1
        );
        // What is shown is what the compile then makes of the text.
        let predicted = viewer
            .predicted
            .as_ref()
            .map(|predicted| predicted.page.clone());
        let predicted = zetwerk_render::render_page(&predicted.unwrap(), 2.0);
        compile(&mut viewer);
        assert!(!viewer.is_predicted());
        let document = viewer.document().unwrap();
        let compiled = zetwerk_render::render_page(&document.pages()[0], 2.0);
        assert!(
            predicted.data() == compiled.data(),
            "the prediction differs from the compile"
        );

        // Markup is not text to be shown as typed: that takes a compile,
        // asked for at once where compiles are quick.
        press(&mut viewer, "*");
        assert!(!viewer.is_predicted());
        assert_eq!(viewer.take_edited(), Some(Edited::Unpredicted));
        // Taken back, the pages are those of the text again.
        press(&mut viewer, "<BS>");
        assert!(viewer.is_compiled());
        // Typing on, then back over part of it, is one insertion still.
        press(&mut viewer, "big<BS>");
        assert!(viewer.is_predicted());
        assert_eq!(
            viewer.text().map(|text| text.contains("very bibrown")),
            Some(true)
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_click_puts_the_cursor_on_the_text_under_it() {
        let (mut viewer, directory) = editing("Hello there world.\n\nSecond paragraph.\n");
        let held: HashSet<TextureId> = HashSet::new();
        // Find where the cursor is drawn on the page for a word, move
        // away, and click there.
        press(&mut viewer, "/there<CR>");
        let block = TextureId(hash128(&("tint", PAGE_CURSOR)));
        let scene = viewer.scene(&|id| held.contains(&id));
        let quad = scene
            .quads
            .iter()
            .find(|quad| quad.texture == block)
            .unwrap();
        let on_the_t = (
            f64::from(quad.x) + 1.0,
            f64::from(quad.y) + f64::from(quad.height) / 2.0,
        );
        press(&mut viewer, "G");
        assert_eq!(viewer.state().editing.unwrap().line, 3);
        viewer.click(on_the_t);
        let editing = viewer.state().editing.unwrap();
        assert_eq!((editing.line, editing.column), (1, 7));
        // And in the source: the third row, a few cells in.
        let (gutter, cell, row_height) = viewer.source_metrics;
        viewer.click((gutter + 7.5 * cell, 2.5 * row_height));
        let editing = viewer.state().editing.unwrap();
        assert_eq!((editing.line, editing.column), (3, 8));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn typing_past_the_end_of_a_line_moves_words_down() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    The quick brown fox jumps over the lazy dog and runs on \
                    through the wood until the line is full, and then some \
                    more of it follows here to make a few lines of text.\n\n\
                    Second paragraph.\n";
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "/brown<CR>i");
        // Far more than the first line has room for.
        let typed = "very very very very very ";
        let mut shown = 0;
        for key in keys::parse(typed).unwrap() {
            viewer.key(key);
            shown += usize::from(viewer.is_predicted());
        }
        assert_eq!(shown, typed.len(), "{:?}", viewer.declined);
        // In ragged text Typst breaks lines the same way: first fit.
        let predicted = viewer
            .predicted
            .as_ref()
            .map(|predicted| predicted.page.clone());
        let predicted = zetwerk_render::render_page(&predicted.unwrap(), 2.0);
        compile(&mut viewer);
        let document = viewer.document().unwrap();
        let compiled = zetwerk_render::render_page(&document.pages()[0], 2.0);
        assert!(
            predicted.data() == compiled.data(),
            "the prediction differs from the compile"
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn typing_at_the_end_of_a_paragraph_adds_lines() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    The quick brown fox jumps over the lazy dog and runs on \
                    through the wood until the line is full, and then some.\n\n\
                    Second paragraph, which is also long enough to be broken \
                    into two lines of the page.\n\n\
                    Third.\n";
        let (mut viewer, directory) = editing(text);
        // At the end of the first paragraph: enough for two more lines.
        press(&mut viewer, "/some<CR>A");
        let typed = " More and more words are typed here, and the paragraph grows by a line and by another.";
        let mut shown = 0;
        for key in keys::parse(typed).unwrap() {
            viewer.key(key);
            shown += usize::from(viewer.is_predicted());
        }
        assert_eq!(shown, typed.len(), "{:?}", viewer.declined);
        let predicted = viewer
            .predicted
            .as_ref()
            .map(|predicted| predicted.page.clone());
        let predicted = zetwerk_render::render_page(&predicted.unwrap(), 2.0);
        compile(&mut viewer);
        let document = viewer.document().unwrap();
        let compiled = zetwerk_render::render_page(&document.pages()[0], 2.0);
        assert!(
            predicted.data() == compiled.data(),
            "the prediction differs from the compile"
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn deleting_and_typing_over_show_on_the_page_too() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    The quick brown fox jumps over the lazy dog and runs on \
                    through the wood until the line is full, and then some.\n\n\
                    A short one here.\n";
        let (mut viewer, directory) = editing(text);
        let same_as_compiled = |viewer: &mut Viewer| {
            let predicted = viewer
                .predicted
                .as_ref()
                .map(|predicted| predicted.page.clone());
            let predicted = zetwerk_render::render_page(&predicted.unwrap(), 2.0);
            compile(viewer);
            let document = viewer.document().unwrap();
            let compiled = zetwerk_render::render_page(&document.pages()[0], 2.0);
            predicted.data() == compiled.data()
        };
        // A character out of the last paragraph.
        press(&mut viewer, "/short<CR>x");
        assert!(viewer.is_predicted(), "{:?}", viewer.declined);
        assert!(same_as_compiled(&mut viewer));
        // Backspace over text that was there before typing began.
        press(&mut viewer, "A<BS><BS><BS>");
        assert!(viewer.is_predicted(), "{:?}", viewer.declined);
        assert_eq!(
            viewer.text().map(|text| text.contains("A hort one he\n")),
            Some(true)
        );
        assert!(same_as_compiled(&mut viewer));
        // A word changed into another.
        press(&mut viewer, "<Esc>/one<CR>cwtwo");
        assert!(viewer.is_predicted(), "{:?}", viewer.declined);
        assert!(same_as_compiled(&mut viewer));
        // A word with the blank behind it.
        press(&mut viewer, "<Esc>/hort<CR>dw");
        assert!(viewer.is_predicted(), "{:?}", viewer.declined);
        assert!(same_as_compiled(&mut viewer));
        // A word from between two blanks leaves two blanks, which Typst
        // sets as one: that is for the compiler.
        press(&mut viewer, "/brown<CR>diw");
        assert!(!viewer.is_predicted());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn deleting_brings_words_up_from_the_next_line() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    The quick brown fox jumps over the lazy dog and runs on \
                    through the wood until the line is full, and then some \
                    more of it follows here to make a few lines of text, \
                    the last of which is short.\n\n\
                    Second paragraph.\n";
        let (mut viewer, directory) = editing(text);
        let same_as_compiled = |viewer: &mut Viewer| {
            let predicted = viewer
                .predicted
                .as_ref()
                .map(|predicted| predicted.page.clone());
            let predicted = zetwerk_render::render_page(&predicted.unwrap(), 2.0);
            compile(viewer);
            let document = viewer.document().unwrap();
            let compiled = zetwerk_render::render_page(&document.pages()[0], 2.0);
            if let Some(dir) = std::env::var_os("ZETWERK_DUMP").map(PathBuf::from) {
                let _ = predicted.save_png(dir.join("predicted.png"));
                let _ = compiled.save_png(dir.join("compiled.png"));
            }
            predicted.data() == compiled.data()
        };
        // Two words out of the first line: the lines below move up a bit.
        press(&mut viewer, "/quick<CR>dw");
        assert!(viewer.is_predicted(), "{:?}", viewer.declined);
        assert!(same_as_compiled(&mut viewer), "after one word");
        press(&mut viewer, "dw");
        assert!(viewer.is_predicted(), "{:?}", viewer.declined);
        assert!(same_as_compiled(&mut viewer), "after two words");
        // Enough out of it that the paragraph loses its last line.
        press(&mut viewer, "d4w");
        assert!(viewer.is_predicted(), "{:?}", viewer.declined);
        assert!(same_as_compiled(&mut viewer), "after a line's worth");
        let _ = std::fs::remove_dir_all(directory);
    }

    /// Where the cursor is drawn on the first page, in points.
    fn caret_on_page(viewer: &Viewer) -> (f64, f64) {
        let document = viewer.document.as_ref().unwrap();
        let world = viewer.compiled_world.as_ref().unwrap();
        let map = PageMap::build(world, &document.pages()[0].frame);
        let cursor = viewer.editor.as_ref().unwrap().cursor();
        let cell = map.caret(viewer.project.main(), cursor).unwrap().cell;
        (cell.min.x.to_pt(), cell.min.y.to_pt())
    }

    #[test]
    fn on_the_page_the_cursor_goes_up_and_down_by_its_lines() {
        // The paragraph is one line of the source and five on the page.
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    The quick brown fox jumps over the lazy dog and runs on \
                    through the wood until the line is full, and then some \
                    more of it follows here to make a few lines of text, \
                    the last of which is short.\n\n\
                    Second paragraph.\n";
        let (mut viewer, directory) = editing(text);
        let cursor = |viewer: &Viewer| viewer.editor.as_ref().unwrap().cursor();
        let line = |viewer: &Viewer| viewer.editor.as_ref().unwrap().position().0;
        press(&mut viewer, "gsgs/brown<CR>");
        assert_eq!(viewer.panes(), Panes::Page);
        let start = cursor(&viewer);
        let (x, y) = caret_on_page(&viewer);

        press(&mut viewer, "j");
        assert_eq!(line(&viewer), 2, "still in the paragraph's one line");
        assert!(cursor(&viewer) > start);
        let (below_x, below_y) = caret_on_page(&viewer);
        assert!(below_y > y + 5.0, "a line further down: {y} to {below_y}");
        assert!(
            (below_x - x).abs() < 8.0,
            "below where it was: {x} to {below_x}"
        );
        press(&mut viewer, "k");
        assert_eq!(cursor(&viewer), start);

        // A count, and the aim holds across a line too short for it.
        press(&mut viewer, "/short<CR>$");
        let (end_x, _) = caret_on_page(&viewer);
        press(&mut viewer, "/full<CR>");
        let (x, y) = caret_on_page(&viewer);
        assert!(x > end_x + 20.0, "beyond the end of the last line");
        press(&mut viewer, "2j");
        assert_eq!(line(&viewer), 2);
        let (_, last_y) = caret_on_page(&viewer);
        assert!(last_y > y + 15.0, "two lines down: {y} to {last_y}");
        press(&mut viewer, "j");
        assert_eq!(line(&viewer), 4, "into the next paragraph");
        press(&mut viewer, "3k");
        let (back_x, back_y) = caret_on_page(&viewer);
        assert!((back_y - y).abs() < 0.5 && (back_x - x).abs() < 8.0);

        // With an operator, `j` is still about lines of the text.
        press(&mut viewer, "dj");
        assert_eq!(
            viewer.text(),
            Some("#set page(width: 300pt, height: 300pt)\nSecond paragraph.\n")
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn on_the_page_the_cursor_goes_through_columns_and_pages_in_reading_order() {
        let words: Vec<String> = (0..150).map(|i| format!("w{i}")).collect();
        let text = format!(
            "#set page(width: 300pt, height: 120pt, columns: 2)\n{}\n",
            words.join(" ")
        );
        let (mut viewer, directory) = editing(&text);
        assert!(viewer.document().unwrap().pages().len() > 1);
        let cursor = |viewer: &Viewer| viewer.editor.as_ref().unwrap().cursor();
        press(&mut viewer, "gsgsjw");
        // Down every line: each is further on in the text, to the last.
        let mut places = vec![cursor(&viewer)];
        loop {
            press(&mut viewer, "j");
            let now = cursor(&viewer);
            if places.last() == Some(&now) {
                break;
            }
            assert!(places.last() < Some(&now), "{places:?} then {now}");
            places.push(now);
        }
        assert!(places.len() > 12, "{places:?}");
        assert!(
            text[cursor(&viewer)..].split(' ').count() < 8,
            "the last line"
        );
        // The cursor is as far into each column as it was into the first.
        let words_in = |place: &usize| text[..*place].matches(' ').count();
        let per_line = words_in(&places[1]) - words_in(&places[0]);
        assert!(places.windows(2).all(|pair| {
            let step = words_in(&pair[1]) - words_in(&pair[0]);
            step.abs_diff(per_line) <= 2
        }));
        // And back up the same way.
        for place in places.iter().rev().skip(1) {
            press(&mut viewer, "k");
            assert_eq!(cursor(&viewer), *place);
        }
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn on_the_page_no_line_is_passed_over() {
        // A centred equation under a short line, the cells of a table,
        // a line at the right margin: none straight below the other.
        let text = "#set page(width: 300pt, height: 400pt)\n\
                    Short.\n\n\
                    $ a + b = c $\n\n\
                    After it the text goes on.\n\n\
                    #table(columns: 2, [one], [two], [three], [four])\n\n\
                    #align(right)[End.]\n";
        let (mut viewer, directory) = editing(text);
        let word = |viewer: &Viewer| {
            let cursor = viewer.editor.as_ref().unwrap().cursor();
            let blank = |c: char| c.is_whitespace() || "[]$".contains(c);
            let start = text[..cursor].rfind(blank).map_or(0, |at| at + 1);
            let end = text[cursor..]
                .find(blank)
                .map_or(text.len(), |at| cursor + at);
            &text[start..end]
        };
        let walk = |viewer: &mut Viewer, key: &str| {
            let mut words = vec![word(viewer)];
            for _ in 0..12 {
                press(viewer, key);
                if words.last() == Some(&word(viewer)) {
                    break;
                }
                words.push(word(viewer));
            }
            words
        };
        press(&mut viewer, "gsgs/Short<CR>");
        assert_eq!(
            walk(&mut viewer, "j"),
            ["Short.", "a", "After", "one", "three", "End."]
        );
        // And up again along the right-hand side.
        press(&mut viewer, "$");
        assert_eq!(
            walk(&mut viewer, "k"),
            ["End.", "four", "two", "on.", "c", "Short."]
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn in_the_source_gj_goes_down_a_row_of_a_wrapped_line() {
        let words = "word ".repeat(40);
        let (mut viewer, directory) = editing(&format!("{words}\nshort\n"));
        let cursor = |viewer: &Viewer| viewer.editor.as_ref().unwrap().cursor();
        let columns = viewer.source_size.0;
        assert!(columns < 200);
        press(&mut viewer, "llgj");
        let row = columns / 5 * 5;
        assert_eq!(cursor(&viewer), row + 2, "the same place in the next row");
        press(&mut viewer, "gj");
        assert_eq!(cursor(&viewer), 2 * row + 2);
        press(&mut viewer, "2gk");
        assert_eq!(cursor(&viewer), 2);
        // `j` goes by lines of the text there.
        press(&mut viewer, "j");
        assert_eq!(cursor(&viewer), 203);
        // With an operator `gj` takes the text up to that place.
        press(&mut viewer, "ggdgj");
        assert_eq!(viewer.text().map(str::len), Some(200 - row + 7));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_panes_can_be_switched() {
        let (mut viewer, directory) = editing("one\ntwo\n");
        press(&mut viewer, "gs");
        assert_eq!(viewer.panes(), Panes::Source);
        press(&mut viewer, "gs");
        assert_eq!(viewer.panes(), Panes::Page);
        // With only pages shown, keys still edit: the cursor is on the
        // page.
        press(&mut viewer, "x");
        assert_eq!(viewer.text(), Some("ne\ntwo\n"));
        press(&mut viewer, "gs");
        assert_eq!(viewer.panes(), Panes::Split);
        press(&mut viewer, "x");
        assert_eq!(viewer.text(), Some("e\ntwo\n"));
        press(&mut viewer, ":view source<CR>");
        assert_eq!(viewer.panes(), Panes::Source);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_source_is_drawn_row_by_row() {
        let (mut viewer, directory) = editing("one\ntwo\n\nfour\n");
        let mut held = HashSet::new();
        let scene = viewer.scene(&|id| held.contains(&id));
        let images = scene.images.len();
        held.extend(scene.images.iter().map(|image| image.id));
        // Nothing new to make for the same frame again, or to move the
        // cursor.
        let scene = viewer.scene(&|id| held.contains(&id));
        assert!(scene.images.is_empty());
        press(&mut viewer, "j");
        let scene = viewer.scene(&|id| held.contains(&id));
        assert_eq!(scene.images.len(), 1, "only the status line changed");
        held.extend(scene.images.iter().map(|image| image.id));
        // Typing makes one row anew, and the status line.
        press(&mut viewer, "ix<Esc>");
        let scene = viewer.scene(&|id| held.contains(&id));
        assert_eq!(scene.images.len(), 2);
        assert!(images > 4, "rows, numbers and chrome: {images} images");
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn an_agent_reads_and_changes_the_text() {
        use super::agent::Outcome;
        let text = "= Title\n\nThe quick brown fox.\nThe lazy dog.\n";
        let (mut viewer, directory) = editing(text);
        let lines = viewer.lines(Some(3), None).unwrap();
        assert_eq!(lines.text, "3\tThe quick brown fox.\n4\tThe lazy dog.\n");
        assert_eq!((lines.first, lines.last, lines.total), (3, 4, 4));
        assert!(viewer.lines(Some(9), None).is_err());
        let revision = lines.revision;

        // The cursor is the user's: it stays on its word.
        press(&mut viewer, "/lazy<CR>");
        let outcome = viewer
            .agent_replace(
                "quick brown",
                "slow red and very tired",
                false,
                Some(revision),
                false,
            )
            .unwrap();
        assert_eq!(
            outcome,
            Outcome::Changed {
                revision: revision + 1,
                lines: (3, 3)
            }
        );
        assert_eq!(
            viewer.text(),
            Some("= Title\n\nThe slow red and very tired fox.\nThe lazy dog.\n")
        );
        assert_eq!(viewer.take_edited(), Some(Edited::Agent));
        let cursor = viewer.editor.as_ref().unwrap().cursor();
        assert!(viewer.text().unwrap()[cursor..].starts_with("lazy"));
        assert!(viewer.agent_mark.is_some());

        // Made for text that is no more; not there; there twice.
        let stale = viewer.agent_replace("fox", "cat", false, Some(revision), false);
        assert!(stale.unwrap_err().contains("revision"));
        let missing = viewer.agent_replace("quick", "x", false, None, false);
        assert!(missing.unwrap_err().contains("not in the text"));
        let twice = viewer.agent_replace("The", "A", false, None, false);
        assert!(twice.unwrap_err().contains("2 times (lines 3, 4)"));
        assert_eq!(viewer.state().revision, revision + 1);
        // All of them, if asked: one step.
        viewer.agent_replace("The", "A", true, None, false).unwrap();
        assert_eq!(
            viewer.text(),
            Some("= Title\n\nA slow red and very tired fox.\nA lazy dog.\n")
        );
        press(&mut viewer, "u");
        assert_eq!(
            viewer.text(),
            Some("= Title\n\nThe slow red and very tired fox.\nThe lazy dog.\n")
        );
        press(&mut viewer, "u");
        assert_eq!(viewer.text(), Some(text));
        assert!(viewer.agent_mark.is_none(), "a key ends the showing");

        // At the cursor, and in place of the selection.
        press(&mut viewer, "/dog<CR>");
        viewer.agent_insert("old ", None, false).unwrap();
        assert!(viewer.text().unwrap().contains("lazy old dog"));
        press(&mut viewer, "viw");
        let focus = viewer.focus().unwrap();
        let selected = focus.selection.unwrap();
        assert_eq!((selected.from, selected.to), ((4, 14), (4, 17)));
        assert_eq!(selected.text, "dog");
        viewer.agent_replace_selection("cat", None, false).unwrap();
        assert!(viewer.text().unwrap().contains("lazy old cat."));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn an_agents_change_is_a_step_of_its_own_while_the_user_types() {
        let (mut viewer, directory) = editing("one\ntwo\n");
        press(&mut viewer, "Aab");
        viewer
            .agent_replace("two", "three", false, None, false)
            .unwrap();
        press(&mut viewer, "cd<Esc>");
        assert_eq!(viewer.text(), Some("oneabcd\nthree\n"));
        press(&mut viewer, "u");
        assert_eq!(viewer.text(), Some("oneab\nthree\n"));
        press(&mut viewer, "u");
        assert_eq!(viewer.text(), Some("oneab\ntwo\n"));
        press(&mut viewer, "u");
        assert_eq!(viewer.text(), Some("one\ntwo\n"));
        press(&mut viewer, "<C-r><C-r><C-r>");
        assert_eq!(viewer.text(), Some("oneabcd\nthree\n"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn proposals_wait_for_the_user() {
        use super::agent::{AgentEdits, Outcome};
        let text = "The quick brown fox.\nThe lazy dog.\nThe end.\n";
        let (mut viewer, directory) = editing(text);
        // Asked for by the agent, or by the user for all agents.
        let first = viewer
            .agent_replace("quick", "slow", false, None, true)
            .unwrap();
        press(&mut viewer, ":agents propose<CR>");
        assert_eq!(viewer.agent_edits(), AgentEdits::Propose);
        let second = viewer
            .agent_replace("lazy dog", "busy cat", false, None, false)
            .unwrap();
        let third = viewer
            .agent_replace("end", "END", false, None, false)
            .unwrap();
        let revision = viewer.state().revision;
        assert_eq!(first, Outcome::Proposed { id: 1, revision });
        assert_eq!(second, Outcome::Proposed { id: 2, revision });
        assert_eq!(third, Outcome::Proposed { id: 3, revision });
        assert_eq!(viewer.text(), Some(text), "nothing is changed yet");
        assert_eq!(viewer.proposals().len(), 3);

        // On a proposal, the panel says what it would do.
        let panel = |viewer: &mut Viewer| {
            let edge = TextureId(hash128(&("solid", LENS_EDGE)));
            let scene = viewer.scene(&|_| false);
            scene.quads.iter().any(|quad| quad.texture == edge)
        };
        assert!(!panel(&mut viewer));
        press(&mut viewer, "]p");
        let cursor = |viewer: &Viewer| viewer.editor.as_ref().unwrap().cursor();
        assert_eq!(cursor(&viewer), text.find("quick").unwrap());
        assert!(panel(&mut viewer));
        press(&mut viewer, "]p");
        assert_eq!(cursor(&viewer), text.find("lazy").unwrap());
        press(&mut viewer, "[p[p");
        assert_eq!(
            cursor(&viewer),
            text.find("end").unwrap(),
            "around the ends"
        );

        // Accepting is a change like any other, and moves the rest along.
        press(&mut viewer, "]pga");
        assert_eq!(
            viewer.text(),
            Some("The slow brown fox.\nThe lazy dog.\nThe end.\n")
        );
        assert_eq!(viewer.take_edited(), Some(Edited::Agent));
        let now = viewer.text().unwrap().to_owned();
        let ranges: Vec<&str> = viewer
            .proposals()
            .iter()
            .map(|proposal| &now[proposal.range.clone()])
            .collect();
        assert_eq!(ranges, ["lazy dog", "end"]);
        press(&mut viewer, "u");
        assert_eq!(viewer.text(), Some(text));

        // Typing in the text of one makes it stale: it can only go.
        press(&mut viewer, "/dog<CR>x");
        assert!(viewer.proposals()[0].stale && !viewer.proposals()[1].stale);
        press(&mut viewer, "0]p");
        assert!(panel(&mut viewer));
        press(&mut viewer, "ga");
        assert!(viewer.text().unwrap().contains("lazy og"), "not made");
        assert_eq!(viewer.proposals().len(), 2);
        // Undoing the typing makes it good again.
        press(&mut viewer, "u");
        assert!(!viewer.proposals()[0].stale);
        press(&mut viewer, "gg]pgr");
        assert_eq!(viewer.proposals().len(), 1);
        press(&mut viewer, ":accept all<CR>");
        assert_eq!(
            viewer.text(),
            Some("The quick brown fox.\nThe lazy dog.\nThe END.\n")
        );
        assert_eq!(viewer.proposals(), []);
        // The agent may take one back itself.
        viewer
            .agent_replace("fox", "cat", false, None, false)
            .unwrap();
        let id = viewer.proposals()[0].id;
        assert!(viewer.withdraw_proposal(id + 7).is_err());
        viewer.withdraw_proposal(id).unwrap();
        assert_eq!(viewer.proposals(), []);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn an_agent_finds_its_way_in_the_document() {
        let text = "#set page(width: 300pt, height: 200pt)\n\
                    = First <one>\n\
                    Some words, and Words again.\n\
                    #pagebreak()\n\
                    == Second *part*\n\
                    More words here.\n";
        let (mut viewer, directory) = editing(text);
        let (found, total) = viewer.search("words", false, 10).unwrap();
        let places: Vec<(usize, usize)> = found.iter().map(|at| (at.line, at.column)).collect();
        assert_eq!((places, total), (vec![(3, 6), (6, 6)], 2));
        let (found, total) = viewer.search("WORDS", true, 2).unwrap();
        assert_eq!((found.len(), total), (2, 3));
        assert_eq!(found[1].text, "Some words, and Words again.");

        let outline = viewer.outline().unwrap();
        let headings: Vec<(usize, &str, usize, Option<usize>)> = outline
            .headings
            .iter()
            .map(|heading| {
                (
                    heading.level,
                    heading.text.as_str(),
                    heading.line,
                    heading.page,
                )
            })
            .collect();
        assert_eq!(
            headings,
            [(1, "First", 2, Some(1)), (2, "Second *part*", 5, Some(2))]
        );
        assert_eq!(outline.labels, [("one".to_owned(), 2)]);

        // From the text to the page and back.
        let (page, x, y) = viewer.page_of(6, 6).unwrap();
        assert_eq!(page, 2);
        assert_eq!(
            viewer.source_of(page, x + 1.0, y + 2.0),
            Ok(("main.typ".to_owned(), 6, 6))
        );
        assert!(viewer.source_of(3, 0.0, 0.0).is_err());
        assert!(viewer.page_of(40, 1).is_err());
        // Not for text the pages are not of.
        press(&mut viewer, "x");
        assert!(viewer.page_of(6, 6).unwrap_err().contains("compiled"));
        assert_eq!(viewer.outline().unwrap().headings[0].page, None);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn formats_are_operators_and_typst_has_text_objects() {
        let text = "= Title\nThe quick brown fox and $x + y$ here.\n\n== Sub\nMore text.\n";
        let (mut viewer, directory) = editing(text);
        let line = |viewer: &Viewer, number: usize| {
            viewer
                .text()
                .unwrap()
                .lines()
                .nth(number - 1)
                .unwrap()
                .to_owned()
        };
        // On a motion, on a text object, on a selection; and again with `.`.
        press(&mut viewer, "/quick<CR>gbiw");
        assert_eq!(line(&viewer, 2), "The *quick* brown fox and $x + y$ here.");
        press(&mut viewer, "/fox<CR>.");
        assert_eq!(
            line(&viewer, 2),
            "The *quick* brown *fox* and $x + y$ here."
        );
        press(&mut viewer, "/brown<CR>vegm");
        assert_eq!(
            line(&viewer, 2),
            "The *quick* _brown_ *fox* and $x + y$ here."
        );
        // Off again where it is on: one undo step each.
        press(&mut viewer, "/quick<CR>gbi*");
        assert_eq!(
            line(&viewer, 2),
            "The quick _brown_ *fox* and $x + y$ here."
        );
        press(&mut viewer, "u");
        assert_eq!(
            line(&viewer, 2),
            "The *quick* _brown_ *fox* and $x + y$ here."
        );
        // In the equation bold is a function; the equation is an object.
        press(&mut viewer, "/y<CR>gbl");
        assert!(line(&viewer, 2).ends_with("$x + bold(y)$ here."));
        press(&mut viewer, "ci$a = b<Esc>");
        assert!(line(&viewer, 2).ends_with("and $a = b$ here."));
        press(&mut viewer, "da$");
        assert!(line(&viewer, 2).ends_with("and  here."));

        // Lines: a comment, a list, a heading's level.
        press(&mut viewer, "Ggcc");
        assert_eq!(line(&viewer, 5), "// More text.");
        press(&mut viewer, "gcc");
        assert_eq!(line(&viewer, 5), "More text.");
        press(&mut viewer, "gll");
        assert_eq!(line(&viewer, 5), "- More text.");
        press(&mut viewer, "u4Gg+");
        assert_eq!(line(&viewer, 4), "=== Sub");
        press(&mut viewer, "g-g-");
        assert_eq!(line(&viewer, 4), "= Sub");
        // A section goes as a whole.
        press(&mut viewer, "Gdah");
        assert_eq!(viewer.text().unwrap().lines().count(), 3);
        assert!(!viewer.text().unwrap().contains("Sub"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_cursor_goes_to_what_the_compiler_complains_about() {
        // Slips of syntax: the compiler tells of all of them at once.
        let text = "Fine.\n#(1 +)\nFine too.\n#(2 *)\n";
        let (mut viewer, directory) = editing(text);
        assert_eq!(viewer.state().problems.len(), 2);
        let line = |viewer: &Viewer| viewer.editor.as_ref().unwrap().position().0;
        let message = |viewer: &Viewer| viewer.message.as_ref().unwrap().text.clone();
        press(&mut viewer, "]d");
        assert_eq!(line(&viewer), 2);
        assert!(
            message(&viewer).starts_with("error: "),
            "{}",
            message(&viewer)
        );
        press(&mut viewer, "]d");
        assert_eq!(line(&viewer), 4);
        press(&mut viewer, "]d");
        assert_eq!(line(&viewer), 2, "around the end");
        press(&mut viewer, "[d");
        assert_eq!(line(&viewer), 4);
        // They are marked in the source while the text is that text.
        let marks = |viewer: &mut Viewer| {
            let id = TextureId(hash128(&("solid", ERROR_MARK)));
            let scene = viewer.scene(&|_| false);
            scene.quads.iter().filter(|quad| quad.texture == id).count()
        };
        assert_eq!(marks(&mut viewer), 2);
        press(&mut viewer, "x");
        assert_eq!(marks(&mut viewer), 0);
        // With nothing wrong there is nowhere to go.
        press(&mut viewer, "ggjddjdd");
        compile(&mut viewer);
        press(&mut viewer, "]d");
        assert_eq!(message(&viewer), "the compiler has no complaints");
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn brackets_come_in_pairs_and_snippets_expand() {
        let (mut viewer, directory) = editing("\n");
        let text = |viewer: &Viewer| viewer.text().unwrap().trim_end().to_owned();
        // The closing one comes along, is stepped over, and goes with
        // its opening one.
        press(&mut viewer, "if(x<Esc>");
        assert_eq!(text(&viewer), "f(x)");
        press(&mut viewer, "ccf(x) and [y]<Esc>");
        assert_eq!(text(&viewer), "f(x) and [y]");
        press(&mut viewer, "cca(<BS>b<Esc>");
        assert_eq!(text(&viewer), "ab");
        // Before text, and for an apostrophe-like mark behind a word, no.
        press(&mut viewer, "0i(<Esc>");
        assert_eq!(text(&viewer), "(ab");
        press(&mut viewer, "ccsay \"hi\" and 5\" wide<Esc>");
        assert_eq!(text(&viewer), "say \"hi\" and 5\" wide");
        // An equation in its line, and one displayed.
        press(&mut viewer, "ccLet $x$ be.<Esc>");
        assert_eq!(text(&viewer), "Let $x$ be.");
        press(&mut viewer, "cc$ a + b<Esc>");
        assert_eq!(text(&viewer), "$ a + b $");
        // It is one undo step, and `.` types the same again.
        press(&mut viewer, "ccx<Esc>Ag(z)<Esc>.");
        assert_eq!(text(&viewer), "xg(z)g(z)");
        press(&mut viewer, "u");
        assert_eq!(text(&viewer), "xg(z)");

        // The name of a snippet and Tab.
        press(&mut viewer, "ccSee fn<Tab>the note<Esc>");
        assert_eq!(text(&viewer), "See #footnote[the note]");
        press(&mut viewer, "cc$ ff<Tab>a<Esc>");
        assert_eq!(text(&viewer), "$ frac(a, ) $");
        // Elsewhere Tab is a tab's worth of blanks.
        press(&mut viewer, "ccab<Tab>c<Esc>");
        assert_eq!(text(&viewer), "ab  c");
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn typing_brings_up_what_could_be_typed() {
        let text = "#let width = 3cm\n#set heading(numbering: \"1.\")\n= Start <start>\nText.\n\n";
        let (mut viewer, directory) = editing(text);
        let labels = |viewer: &Viewer| -> Vec<String> {
            let completion = viewer.completion.as_ref();
            completion.map_or_else(Vec::new, |completion| {
                completion
                    .candidates
                    .iter()
                    .map(|entry| entry.candidate.label.clone())
                    .collect()
            })
        };
        let last = |viewer: &Viewer| viewer.text().unwrap().lines().last().unwrap().to_owned();
        // Behind a hash: functions, and what the document defines.
        press(&mut viewer, "Go#");
        assert!(labels(&viewer).len() > 20, "{:?}", labels(&viewer));
        press(&mut viewer, "wi");
        assert_eq!(labels(&viewer).first().map(String::as_str), Some("width"));
        press(&mut viewer, "<Tab>");
        assert_eq!(last(&viewer), "#width");
        assert!(viewer.completion.is_none());
        // A function comes with its brackets, the cursor between them.
        press(&mut viewer, " and #ali");
        assert!(labels(&viewer).contains(&"align".to_owned()));
        press(&mut viewer, "<C-n><C-p><Tab>center<Esc>");
        assert_eq!(last(&viewer), "#width and #align(center)");
        // One undo step with the rest of what was typed, and `.` types
        // it again.
        press(&mut viewer, "u");
        assert_eq!(viewer.text(), Some(text));
        press(&mut viewer, ".");
        assert_eq!(last(&viewer), "#width and #align(center)");
        // In running text nothing comes up unasked; asked, it does; and
        // it goes away again.
        press(&mut viewer, "Goplain wor");
        assert_eq!(labels(&viewer), Vec::<String>::new());
        press(&mut viewer, "<Esc>o@st");
        assert_eq!(labels(&viewer), ["start"]);
        press(&mut viewer, "<C-e>");
        assert!(viewer.completion.is_none());
        press(&mut viewer, "<C-n>");
        assert_eq!(labels(&viewer), ["start"]);
        press(&mut viewer, "<C-y><Esc>");
        assert_eq!(last(&viewer), "@start");
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn typst_tells_what_a_thing_is_and_where_it_is_from() {
        let text = "#let width = 3cm\n= Start\n#rect(width: width)\n\n== Sub part\nEnd.\n";
        let (mut viewer, directory) = editing(text);
        let line = |viewer: &Viewer| viewer.editor.as_ref().unwrap().position().0;
        press(&mut viewer, "/rect<CR>K");
        let told = viewer.tooltip.clone().unwrap();
        assert!(told.to_lowercase().contains("rectangle"), "{told}");
        press(&mut viewer, "l");
        assert_eq!(viewer.tooltip, None, "until the next key");
        // To the definition, and back.
        press(&mut viewer, "$hgd");
        assert_eq!(viewer.editor.as_ref().unwrap().position(), (1, 6));
        press(&mut viewer, "``");
        assert_eq!(line(&viewer), 3);
        press(&mut viewer, "/rect<CR>gd");
        assert!(viewer.message.as_ref().unwrap().text.contains("built in"));

        // The list of headings: narrowed by typing, Enter goes there.
        press(&mut viewer, "go");
        assert!(viewer.is_picking());
        press(&mut viewer, "x");
        assert_eq!(viewer.text(), Some(text), "keys are for the list");
        press(&mut viewer, "<BS>sub<CR>");
        assert!(!viewer.is_picking());
        assert_eq!(line(&viewer), 5);
        press(&mut viewer, "go<Esc>");
        assert_eq!(line(&viewer), 5);
        press(&mut viewer, ":labels<CR>");
        assert!(!viewer.is_picking(), "there are none");
        let _ = std::fs::remove_dir_all(directory);
    }

    /// A document of three files: a main one that includes two chapters.
    fn chapters() -> (Viewer, PathBuf) {
        let main = "#set page(width: 300pt, height: 200pt)\n\
                    = Thesis\n\
                    Opening words.\n\
                    #include \"one.typ\"\n\
                    #include \"parts/two.typ\"\n\n\
                    Closing words.\n";
        let directory = directory(main);
        std::fs::create_dir_all(directory.join("parts")).unwrap();
        std::fs::write(directory.join("one.typ"), "== One\nFirst chapter text.\n").unwrap();
        let two = "== Two\nSecond chapter text.\n";
        std::fs::write(directory.join("parts/two.typ"), two).unwrap();
        (open(&directory).0, directory)
    }

    #[test]
    fn the_files_of_a_document_are_edited_together() {
        let (mut viewer, directory) = chapters();
        let name = |viewer: &Viewer| viewer.state().file;
        let message = |viewer: &Viewer| viewer.message.as_ref().unwrap().text.clone();
        assert_eq!(name(&viewer), "main.typ");
        // To the file named under the cursor, and back.
        press(&mut viewer, "/one<CR>gf");
        assert_eq!(name(&viewer), "one.typ");
        assert_eq!(viewer.text(), Some("== One\nFirst chapter text.\n"));
        press(&mut viewer, "<C-^>");
        assert_eq!(name(&viewer), "main.typ");
        assert_eq!(
            viewer.editor.as_ref().unwrap().position().0,
            4,
            "where it was"
        );
        // What is typed in one shows in the document, saved or not.
        press(&mut viewer, ":e parts/two.typ<CR>");
        assert_eq!(name(&viewer), "parts/two.typ");
        press(&mut viewer, "GAAdded.<Esc>");
        compile(&mut viewer);
        assert_eq!(viewer.state().problems, Vec::<String>::new());
        let on_disk = std::fs::read_to_string(directory.join("parts/two.typ")).unwrap();
        assert!(!on_disk.contains("Added"));
        // An agent asking what is at a place on the page is told the file.
        let (page, x, y) = viewer.page_of(2, 1).unwrap();
        let shown = viewer.source_of(page, x + 1.0, y + 2.0);
        assert_eq!(shown, Ok(("parts/two.typ".to_owned(), 2, 1)));
        // Registers go along from file to file.
        press(&mut viewer, "yiW:b main<CR>GP");
        assert!(viewer.text().unwrap().contains("Added."));
        assert_eq!(name(&viewer), "main.typ");
        // The list says what is open and what is unwritten.
        press(&mut viewer, ":ls<CR>");
        assert_eq!(
            message(&viewer),
            "1% main.typ [+]   2 one.typ   3 parts/two.typ [+]"
        );
        press(&mut viewer, ":q<CR>");
        assert!(!viewer.wants_quit());
        assert!(message(&viewer).contains("main.typ, parts/two.typ"));
        press(&mut viewer, ":wa<CR>");
        let on_disk = std::fs::read_to_string(directory.join("parts/two.typ")).unwrap();
        assert!(on_disk.contains("Added."));
        assert_eq!(viewer.unwritten(), Vec::<String>::new());
        // Round the open files, and to one by its number.
        press(&mut viewer, ":bn<CR>");
        assert_eq!(name(&viewer), "one.typ");
        press(&mut viewer, ":bp<CR>:bp<CR>");
        assert_eq!(name(&viewer), "parts/two.typ");
        press(&mut viewer, ":b 1<CR>");
        assert_eq!(name(&viewer), "main.typ");
        // What is not there is not made by accident.
        press(&mut viewer, ":e three.typ<CR>");
        assert!(message(&viewer).contains(":e! makes it"));
        press(&mut viewer, ":e! three.typ<CR>");
        assert_eq!(name(&viewer), "three.typ");
        assert!(directory.join("three.typ").is_file());
        press(&mut viewer, ":q<CR>");
        assert!(viewer.wants_quit());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_page_leads_into_the_file_its_text_is_from() {
        let (mut viewer, directory) = chapters();
        let name = |viewer: &Viewer| viewer.state().file;
        let word = |viewer: &Viewer| {
            let editor = viewer.editor.as_ref().unwrap();
            let rest = &editor.text()[editor.cursor()..];
            rest.split_whitespace().next().unwrap_or("").to_owned()
        };
        // Down the page from the main file: into the first chapter,
        // through the second and out again.
        press(&mut viewer, "gsgs/Opening<CR>");
        let mut walked = vec![(name(&viewer), word(&viewer))];
        for _ in 0..12 {
            press(&mut viewer, "j");
            let now = (name(&viewer), word(&viewer));
            if walked.last() == Some(&now) {
                break;
            }
            walked.push(now);
        }
        let walked: Vec<(&str, &str)> = walked
            .iter()
            .map(|(file, word)| (file.as_str(), word.as_str()))
            .collect();
        assert_eq!(
            walked,
            [
                ("main.typ", "Opening"),
                ("one.typ", "One"),
                ("one.typ", "First"),
                ("parts/two.typ", "Two"),
                ("parts/two.typ", "Second"),
                ("main.typ", "Closing"),
            ]
        );
        // And up again.
        press(&mut viewer, "kk");
        assert_eq!(
            (name(&viewer), word(&viewer)),
            ("parts/two.typ".to_owned(), "Two".to_owned())
        );
        // A click on text of another file goes to that file.
        press(&mut viewer, ":b main<CR>/Closing<CR>");
        let cursor = tints(&mut viewer, PAGE_CURSOR);
        assert_eq!(cursor.len(), 1);
        press(&mut viewer, "kk");
        let other = tints(&mut viewer, PAGE_CURSOR);
        assert_eq!(name(&viewer), "parts/two.typ");
        press(&mut viewer, ":b main<CR>");
        let (x, y) = (f64::from(other[0].x) + 2.0, f64::from(other[0].y) + 3.0);
        viewer.click((x, y));
        assert_eq!(
            (name(&viewer), word(&viewer)),
            ("parts/two.typ".to_owned(), "Two".to_owned())
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn pages_are_shown_as_slides_and_all_at_once() {
        let text = "#set page(width: 320pt, height: 180pt)\n\
                    = One\nFirst.\n#pagebreak()\n\
                    = Two\nSecond.\n#pagebreak()\n\
                    = Three\nThird.\n";
        let (mut viewer, directory) = editing(text);
        let line = |viewer: &Viewer| viewer.editor.as_ref().unwrap().position().0;
        let visible = |viewer: &Viewer| -> Vec<usize> {
            let pages = viewer.view.visible(&viewer.layout);
            pages.iter().map(|page| page.index).collect()
        };
        press(&mut viewer, "gsgs");
        assert!(visible(&viewer).len() > 1, "one under the other");
        // One at a time, whole, wherever the cursor is.
        press(&mut viewer, ":slides<CR>");
        assert_eq!(viewer.paging(), Paging::Slides);
        assert_eq!(visible(&viewer), [0]);
        let page = viewer.layout.pages[0];
        assert!(page.height <= viewer.view.height && page.width <= viewer.view.width);
        press(&mut viewer, "/Second<CR>");
        assert_eq!(visible(&viewer), [1]);
        // To the next page and the one before, the cursor along.
        press(&mut viewer, "]]");
        assert_eq!((visible(&viewer), line(&viewer)), (vec![2], 8));
        press(&mut viewer, "]]");
        assert_eq!(visible(&viewer), [2], "the last one");
        press(&mut viewer, "[[[[");
        assert_eq!((visible(&viewer), line(&viewer)), (vec![0], 2));
        // A push at the view is the next slide, not half of one.
        viewer.scroll(0.0, 30.0);
        assert_eq!(visible(&viewer), [0]);
        viewer.scroll(0.0, 30.0);
        assert_eq!(visible(&viewer), [1]);
        viewer.scroll(0.0, -80.0);
        assert_eq!(visible(&viewer), [0]);

        // The overview: keys pick a page, Enter goes to it.
        press(&mut viewer, ":grid<CR>");
        assert!(viewer.is_grid());
        let scene = viewer.scene(&|_| false);
        assert_eq!(scene.wanted.len(), 3, "all pages are asked for, small");
        assert!(scene.wanted.iter().all(|tile| tile.region.width < 400));
        press(&mut viewer, "x");
        assert_eq!(viewer.text(), Some(text), "keys are for the overview");
        press(&mut viewer, "ll<CR>");
        assert!(!viewer.is_grid());
        assert_eq!((visible(&viewer), line(&viewer)), (vec![2], 8));
        press(&mut viewer, ":grid<CR>h<Esc>");
        assert_eq!(visible(&viewer), [2], "Escape leaves things as they were");
        // And scrolling again.
        press(&mut viewer, ":slides<CR>");
        assert_eq!(viewer.paging(), Paging::Scroll);
        press(&mut viewer, "gg]]");
        assert_eq!(line(&viewer), 5);
        assert!(visible(&viewer).contains(&1));
        press(&mut viewer, ":pdf<CR>");
        let pdf = std::fs::read(directory.join("main.pdf")).unwrap();
        assert!(pdf.starts_with(b"%PDF"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_users_own_keys_stand_for_others() {
        let (mut viewer, directory) = editing("a;b and more\n");
        let (keymap, complaint) = Keymap::parse(
            [(";", ":"), (",w", ":w<CR>"), ("<C-s>", ":w<CR>")],
            [("<C-s>", "<Esc>:w<CR>")],
        );
        assert_eq!(complaint, None);
        viewer.set_keymap(keymap);
        let saved = |directory: &Path| std::fs::read_to_string(directory.join("main.typ")).unwrap();
        // One key for another.
        press(&mut viewer, ";");
        assert_eq!(viewer.state().editing.unwrap().mode, editor::Mode::Command);
        press(&mut viewer, "<Esc>");
        // In the middle of a command a key is what it is.
        press(&mut viewer, "f;");
        assert_eq!(viewer.editor.as_ref().unwrap().cursor(), 1);
        // Several keys: waited for, and typed as they are if they do not
        // become the mapping.
        press(&mut viewer, "x,");
        assert_eq!(viewer.text(), Some("ab and more\n"));
        press(&mut viewer, "w");
        assert_eq!(saved(&directory), "ab and more\n");
        press(&mut viewer, ",x");
        assert_eq!(viewer.text(), Some("a and more\n"));
        // While typing, at once.
        press(&mut viewer, "Anew<C-s>");
        assert_eq!(saved(&directory), "a and morenew\n");
        assert_eq!(viewer.state().editing.unwrap().mode, editor::Mode::Normal);
        // What a mapping stands for is not looked up again.
        let (keymap, _) = Keymap::parse([("x", "dd"), ("dd", "x")], []);
        viewer.set_keymap(keymap);
        press(&mut viewer, "x");
        assert_eq!(viewer.text(), Some(""));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    #[allow(clippy::too_many_lines, reason = "one walk over a page")]
    fn on_the_page_vim_goes_by_what_is_shown() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    #set heading(numbering: \"1.\")\n\
                    = Start <start>\n\
                    The *quick* brown fox jumps over the lazy dog and runs on \
                    through the wood, as @start says, until the line is full \
                    and then some more of it follows here.\n\n\
                    Second paragraph.\n";
        let (mut viewer, directory) = editing(text);
        let under = |viewer: &Viewer| {
            let editor = viewer.editor.as_ref().unwrap();
            editor.text()[editor.cursor()..].chars().next().unwrap()
        };
        let paragraph = |viewer: &Viewer| viewer.text().unwrap().lines().nth(3).unwrap().to_owned();
        press(&mut viewer, "gsgs/The<CR>");
        // Character by character: over the marks, which show as nothing,
        // and with the cursor on the page all the way.
        let mut seen = String::new();
        for _ in 0..12 {
            seen.push(under(&viewer));
            assert_eq!(tints(&mut viewer, PAGE_CURSOR).len(), 1, "after {seen:?}");
            press(&mut viewer, "l");
        }
        assert_eq!(seen, "The quick br");
        press(&mut viewer, "hhhhhhhhh");
        assert_eq!(under(&viewer), ' ', "and back over them");
        press(&mut viewer, "h");
        assert_eq!(under(&viewer), 'e');
        // Backspace takes what is shown before the cursor, not the mark.
        press(&mut viewer, "/brown<CR>hi<BS><Esc>");
        assert!(paragraph(&viewer).starts_with("The *quic* brown"));
        press(&mut viewer, "u");

        // What a reference shows is one thing: stepped over, deleted whole.
        press(&mut viewer, "/\\=@start<CR>");
        assert_eq!(under(&viewer), '@');
        press(&mut viewer, "l");
        assert_eq!(under(&viewer), ' ', "past all of it");
        press(&mut viewer, "hx");
        assert!(
            paragraph(&viewer).contains("as  says"),
            "{}",
            paragraph(&viewer)
        );
        press(&mut viewer, "u");
        compile(&mut viewer);

        // The start and the end of a line are those of the line shown.
        press(&mut viewer, "/jumps<CR>");
        let line = {
            let lines = PageRows {
                document: viewer.document.as_ref().unwrap(),
                world: viewer.compiled_world.as_ref().unwrap(),
                file: viewer.file_id(),
                lines: &viewer.page_lines,
                cursor_page: &viewer.cursor_page,
                change: Change::NONE,
                landed: &viewer.landed,
                elsewhere: &viewer.elsewhere,
            };
            let cursor = viewer.editor.as_ref().unwrap().cursor();
            lines.line(cursor).unwrap()
        };
        let shown = &viewer.text().unwrap()[line.start..line.end].to_owned();
        assert!(
            shown.starts_with("The *quick*") && shown.len() < 70,
            "{shown}"
        );
        let cursor = |viewer: &Viewer| viewer.editor.as_ref().unwrap().cursor();
        press(&mut viewer, "$");
        assert_eq!(cursor(&viewer), line.last);
        press(&mut viewer, "0");
        assert_eq!(cursor(&viewer), line.start);
        // `V` takes the paragraph: the lines the page sets it in are one
        // line of the text to whoever wrote it, as a line Vim wraps is
        // one to `V`. `j` takes the next paragraph with it.
        press(&mut viewer, "/jumps<CR>V");
        assert_eq!(
            viewer.editor.as_ref().unwrap().mode(),
            editor::Mode::VisualLine
        );
        let whole = viewer.editor.as_ref().unwrap().selection().unwrap();
        assert_eq!(whole.start, line.start);
        assert!(
            whole.end > line.end + 20 && whole.end <= text.find("\n\nSecond").unwrap(),
            "{whole:?}"
        );
        press(&mut viewer, "j");
        let two = viewer.editor.as_ref().unwrap().selection().unwrap();
        assert_eq!(two.start, whole.start);
        assert!(two.end > text.find("Second paragraph").unwrap(), "{two:?}");
        press(&mut viewer, "<Esc>");
        // `dd` takes the paragraph: that is one line to whoever wrote
        // it, as a line Vim wraps is one to `dd`. The row shown, taken
        // out of its paragraph, could be put back nowhere but into a
        // word.
        press(&mut viewer, "/jumps<CR>dd");
        let rest = viewer.text().unwrap().to_owned();
        assert!(
            !rest.contains("jumps")
                && !rest.contains("follows here.")
                && rest.contains("Second paragraph."),
            "{rest}"
        );
        press(&mut viewer, "u");
        compile(&mut viewer);
        // At its end and at its start.
        press(&mut viewer, "/jumps<CR>A!<Esc>");
        assert_eq!(&viewer.text().unwrap()[line.end..=line.end], "!");
        press(&mut viewer, "u");
        compile(&mut viewer);
        press(&mut viewer, "/jumps<CR>I>><Esc>");
        assert_eq!(&viewer.text().unwrap()[line.start..line.start + 2], ">>");
        press(&mut viewer, "u");
        compile(&mut viewer);

        // A new paragraph below, and Enter ends one.
        press(&mut viewer, "/jumps<CR>oNew one.<CR>And another.<Esc>");
        assert!(
            viewer
                .text()
                .unwrap()
                .contains("follows here.\n\nNew one.\n\nAnd another.\n\nSecond paragraph.")
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn things_are_put_in_from_a_menu() {
        let (mut viewer, directory) = editing("Some text here.\n\n");
        let labels = |viewer: &Viewer| -> Vec<String> {
            let completion = viewer.completion.as_ref();
            completion.map_or_else(Vec::new, |completion| {
                let label = |entry: &assist::Entry| entry.candidate.label.clone();
                completion.candidates.iter().map(label).collect()
            })
        };
        let lines = |viewer: &Viewer| -> Vec<String> {
            let text = viewer.text().unwrap();
            text.lines()
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect()
        };
        // Behind a hash: what can be put in, narrowed by what is typed.
        press(&mut viewer, "Go#");
        assert_eq!(
            labels(&viewer)[..3],
            ["equation", "numbered equation", "inline equation"]
        );
        press(&mut viewer, "num");
        assert_eq!(labels(&viewer)[0], "numbered equation");
        press(&mut viewer, "<CR>x^2<Esc>");
        assert_eq!(
            lines(&viewer)[1],
            "#math.equation(block: true, numbering: \"(1)\", $ x^2 $)"
        );
        compile(&mut viewer);
        assert_eq!(viewer.state().problems, Vec::<String>::new());
        // What stands by itself parts the paragraph it is put into.
        press(&mut viewer, "gg/text<CR>i#fig<Tab>cat.png<Esc>");
        assert_eq!(
            lines(&viewer)[..6],
            [
                "Some ",
                "#figure(",
                "  image(\"cat.png\"),",
                "  caption: [],",
                ")",
                "text here."
            ]
        );
        // What stands in a line does not.
        press(&mut viewer, "G$a#foot<CR>a note<Esc>");
        assert!(viewer.text().unwrap().contains("$)#footnote[a note]"));
        // In an equation the names come up as they are typed: no hash.
        press(&mut viewer, "Go$sq");
        assert_eq!(labels(&viewer)[0], "square root");
        press(&mut viewer, "<Tab>2<Esc>");
        assert_eq!(lines(&viewer).last().map(String::as_str), Some("$sqrt(2)$"));
        press(&mut viewer, "i + fra");
        assert_eq!(labels(&viewer)[0], "fraction");
        press(&mut viewer, "<C-e><Esc>");
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn what_is_typed_shows_where_it_is_typed() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    = Title\n\
                    Some words and $x^2 + y^2$ in a line.\n";
        let (mut viewer, directory) = editing(text);
        // What is laid over the page is for when the part that changed
        // cannot be typeset by itself and put into it. The numbers
        // beside the lines are laid over it too, and not meant here.
        viewer.in_parts = false;
        viewer.numbers = false;
        // The images laid over the page that are neither its tiles nor a
        // colour: what was typeset in place.
        let pieces = |viewer: &mut Viewer| -> Vec<Quad> {
            let scene = viewer.scene(&|_| false);
            let drawn = scene.images.iter().filter(|image| image.pixmap.width() > 1);
            let ours: HashSet<TextureId> = drawn.map(|image| image.id).collect();
            // Not the status line, which is as wide as the window.
            let large = |quad: &&Quad| ours.contains(&quad.texture) && quad.width < 900.0;
            scene.quads.iter().filter(large).copied().collect()
        };
        let bars = |viewer: &mut Viewer| tints(viewer, PAGE_BAR);
        // A document that takes long to typeset: one that does not is
        // typeset anew instead.
        viewer.stats.compile_ms = 10.0 * QUICK_COMPILE_MS;
        press(&mut viewer, "gsgs/Title<CR>");
        let heading = tints(&mut viewer, PAGE_CURSOR)[0];
        // Words typed into a heading are not worked into the page: they
        // are set by themselves and laid where they are typed, in the
        // heading's size, with the cursor behind them.
        press(&mut viewer, "A more");
        assert!(!viewer.is_predicted());
        let typed = pieces(&mut viewer);
        assert_eq!(typed.len(), 1, "{typed:?}");
        assert!(
            (typed[0].y - heading.y).abs() < 2.0,
            "on the heading's line"
        );
        assert!(
            (typed[0].height - heading.height).abs() < 2.0,
            "and as high"
        );
        assert!(typed[0].x > heading.x + 3.0 * heading.width);
        let bar = bars(&mut viewer);
        assert_eq!(bar.len(), 1);
        assert!(
            (bar[0].x - (typed[0].x + typed[0].width)).abs() < 3.0,
            "{bar:?}"
        );
        // Wider with every key.
        press(&mut viewer, " words");
        assert!(pieces(&mut viewer)[0].width > typed[0].width + 10.0);
        press(&mut viewer, "<Esc>");
        compile(&mut viewer);
        assert_eq!(pieces(&mut viewer), [], "the page has it now");

        // A new paragraph: under the one it follows, at its left edge.
        viewer.stats.compile_ms = 10.0 * QUICK_COMPILE_MS;
        press(&mut viewer, "/Some<CR>");
        let first = tints(&mut viewer, PAGE_CURSOR)[0];
        press(&mut viewer, "oNext *one*");
        let typed = pieces(&mut viewer);
        assert_eq!(typed.len(), 1);
        assert!((typed[0].x - first.x).abs() < 2.0, "{typed:?} {first:?}");
        assert!(typed[0].y > first.y + first.height);
        assert!(typed[0].y < first.y + 4.0 * first.height);
        // Half an equation is set as one, with what it lacks.
        press(&mut viewer, " and $a^");
        let half = pieces(&mut viewer);
        assert_eq!(half.len(), 1);
        assert!(half[0].width > typed[0].width);
        assert_eq!(bars(&mut viewer).len(), 1);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_delete_key_deletes_forwards() {
        let (mut viewer, directory) = editing("abc *def* ghi\njkl\n");
        // What is under the cursor; what is selected; while typing, what
        // is behind the cursor, the end of a line too.
        press(&mut viewer, "<Del>");
        assert_eq!(viewer.text(), Some("bc *def* ghi\njkl\n"));
        press(&mut viewer, "vl<Del>");
        assert_eq!(viewer.text(), Some(" *def* ghi\njkl\n"));
        press(&mut viewer, "A<Del>!<Esc>");
        assert_eq!(viewer.text(), Some(" *def* ghi!jkl\n"));
        press(&mut viewer, "0i<Del><Del><Esc>");
        assert_eq!(viewer.text(), Some("def* ghi!jkl\n"));
        // At the very end there is nothing to take.
        press(&mut viewer, "A<Del><Esc>u");
        assert_eq!(viewer.text(), Some(" *def* ghi!jkl\n"));
        // On the page, while typing: what is shown behind the cursor, not
        // a mark that shows as nothing.
        compile(&mut viewer);
        press(&mut viewer, "gsgs0i<Del><Esc>");
        assert_eq!(viewer.text(), Some(" *ef* ghi!jkl\n"));
        let _ = std::fs::remove_dir_all(directory);
    }

    /// Whether `j` and `k` get through a whole document on its pages:
    /// `ZETWERK_BENCH=file.typ cargo test --release -p zetwerk-app walking_the_pages -- --ignored --nocapture`
    #[test]
    #[ignore = "a measurement, not a test"]
    fn walking_the_pages() {
        let Ok(file) = std::env::var("ZETWERK_BENCH") else {
            return;
        };
        let directory = directory(&std::fs::read_to_string(file).unwrap());
        let (mut viewer, _) = open(&directory);
        let cursor = |viewer: &Viewer| viewer.editor.as_ref().unwrap().cursor();
        let lines: usize = {
            let document = viewer.document.as_ref().unwrap();
            let world = viewer.compiled_world.as_ref().unwrap();
            let count = |page: &Page| PageMap::build(world, &page.frame).lines().len();
            document.pages().iter().map(count).sum()
        };
        let pages = viewer.document().unwrap().pages().len();
        press(&mut viewer, "gsgs");
        for key in ["j", "k"] {
            let start = std::time::Instant::now();
            let mut seen = HashSet::from([cursor(&viewer)]);
            let (mut steps, mut again) = (0, 0);
            let mut trail = Vec::new();
            loop {
                let before = cursor(&viewer);
                press(&mut viewer, key);
                if cursor(&viewer) == before {
                    break;
                }
                steps += 1;
                again += usize::from(!seen.insert(cursor(&viewer)));
                trail.push((viewer.cursor_page.get() + 1, cursor(&viewer)));
                if steps > 4 * lines + 100 {
                    let text = viewer.text().unwrap();
                    for (page, at) in &trail[trail.len() - 40..] {
                        let shown: String = text[*at..].chars().take(30).collect();
                        println!("page {page}: {at} {shown:?}");
                    }
                    panic!("going round in circles");
                }
            }
            println!(
                "{key}: {steps} steps over {lines} lines, {again} to a place seen before, \
                 ends on page {} of {pages}, {:.3} ms a step",
                viewer.cursor_page.get() + 1,
                start.elapsed().as_secs_f64() * 1000.0 / steps.max(1) as f64
            );
        }
        let _ = std::fs::remove_dir_all(directory);
    }

    /// What a key costs that changes more than a word of a line (an
    /// equation, markup, a new paragraph), on a long document:
    /// `ZETWERK_BENCH=file.typ cargo test --release -p zetwerk-app typing_in_parts_cost -- --ignored --nocapture`
    #[test]
    #[ignore = "a measurement, not a test"]
    fn typing_in_parts_cost() {
        let Ok(file) = std::env::var("ZETWERK_BENCH") else {
            return;
        };
        let directory = directory(&std::fs::read_to_string(file).unwrap());
        let (mut viewer, _) = open(&directory);
        viewer.resize((1920, 1200), 1.25);
        let started = std::time::Instant::now();
        compile(&mut viewer);
        println!(
            "all of it, the first time: {:.1} ms",
            started.elapsed().as_secs_f64() * 1000.0
        );
        let mut held = HashSet::new();
        let mut frame = |viewer: &mut Viewer| {
            let scene = viewer.scene(&|id| held.contains(&id));
            held.extend(scene.images.iter().map(|image| image.id));
        };
        press(&mut viewer, "gsgs");
        let lines = viewer.text().map_or(0, |text| text.lines().count());
        press(&mut viewer, &format!("{}G", lines / 2));
        frame(&mut viewer);
        for (name, keys) in [
            (
                "an equation",
                "A Then $x_i^2 + ff<Tab>a<Tab>b<Tab> = alpha<Tab> holds.",
            ),
            ("strong words", " And <C-b>strong words<C-b> follow."),
            (
                "a paragraph",
                "<CR>A new paragraph begins here, with words.",
            ),
        ] {
            let keys = keys::parse(keys).unwrap();
            let (mut key_time, mut frame_time, mut worst) =
                (Duration::ZERO, Duration::ZERO, Duration::ZERO);
            let (local, declined) = (viewer.stats.local, viewer.stats.declined);
            for &key in &keys {
                let start = std::time::Instant::now();
                viewer.key(key);
                let took = start.elapsed();
                key_time += took;
                worst = worst.max(took);
                if std::env::var_os("ZETWERK_KEYS").is_some() {
                    println!(
                        "    {key:?}: {:.2} ms, {} ms in parts, {:?}",
                        took.as_secs_f64() * 1000.0,
                        viewer.stats.local_ms.round(),
                        viewer.declined
                    );
                }
                let start = std::time::Instant::now();
                frame(&mut viewer);
                frame_time += start.elapsed();
            }
            let per = |total: Duration| total.as_secs_f64() * 1000.0 / keys.len() as f64;
            println!(
                "{name:>14}: key {:.2} ms (worst {:.2}), frame {:.2} ms; {} keys, {} in parts, {} had to wait ({:?})",
                per(key_time),
                worst.as_secs_f64() * 1000.0,
                per(frame_time),
                keys.len(),
                viewer.stats.local - local,
                viewer.stats.declined - declined,
                viewer.declined
            );
        }
        let started = std::time::Instant::now();
        compile(&mut viewer);
        println!(
            "all of it, again: {:.1} ms",
            started.elapsed().as_secs_f64() * 1000.0
        );
    }

    /// What a keystroke costs before the compositor gets the frame:
    /// `ZETWERK_BENCH=file.typ cargo test --release -p zetwerk-app typing_cost -- --ignored --nocapture`
    #[test]
    #[ignore = "a measurement, not a test"]
    fn typing_cost() {
        let Ok(file) = std::env::var("ZETWERK_BENCH") else {
            return;
        };
        let directory = directory(&std::fs::read_to_string(file).unwrap());
        let (mut viewer, _) = open(&directory);
        viewer.resize((1920, 1200), 1.25);
        let mut held = HashSet::new();
        let mut frame = |viewer: &mut Viewer| {
            let scene = viewer.scene(&|id| held.contains(&id));
            held.extend(scene.images.iter().map(|image| image.id));
            scene.images.len()
        };
        press(&mut viewer, "300Gw");
        frame(&mut viewer);
        let mut report = |name: &str, keys: &str, viewer: &mut Viewer| {
            let keys = keys::parse(keys).unwrap();
            let (mut key_time, mut frame_time, mut images) = (Duration::ZERO, Duration::ZERO, 0);
            for &key in &keys {
                let start = std::time::Instant::now();
                viewer.key(key);
                key_time += start.elapsed();
                let start = std::time::Instant::now();
                images += frame(viewer);
                frame_time += start.elapsed();
            }
            let per = |total: Duration| total.as_secs_f64() * 1000.0 / keys.len() as f64;
            println!(
                "{name:>18}: key {:.3} ms, frame {:.3} ms, {:.1} images per key",
                per(key_time),
                per(frame_time),
                images as f64 / keys.len() as f64
            );
        };
        press(&mut viewer, "gsgs");
        report(
            "lines on the page",
            "jjjjjjjjjjkkkkkkkkkkjjjjjjjjjj",
            &mut viewer,
        );
        press(&mut viewer, "gs");
        report(
            "typing",
            "ithe quick brown fox jumps over the lazy dog and on",
            &mut viewer,
        );
        report("leaving insert", "<Esc>", &mut viewer);
        report(
            "moving in a line",
            "hhhhhhhhhhllllllllllwwwwwbbbbb",
            &mut viewer,
        );
        report(
            "moving by lines",
            "jjjjjjjjjjkkkkkkkkkkjjjjjjjjjj",
            &mut viewer,
        );
        report("with a list up", "A #figu<C-e><Esc>", &mut viewer);
        report("half a page down", "<C-d>", &mut viewer);
        report("deleting words", "dwdwdwdwdwdwdwdw", &mut viewer);
        report("undo", "uuuuuuuu", &mut viewer);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_string_left_open_in_an_equation_does_not_take_the_document_with_it() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    #set heading(numbering: \"1.\")\n\
                    Before.\n\nLet $x_0$ be real, see @later.\n\n= Later <later>\nWords.\n";
        let (mut viewer, directory) = editing(text);
        // The quote that closed the string is taken away: to Typst the
        // string then goes on to the end of the document, label and all.
        press(&mut viewer, "/Before<CR>A Then $a + \"<Del> b");
        assert!(
            viewer
                .text()
                .is_some_and(|text| text.contains("Then $a + \" b$\n\nLet $x_0$")),
            "{:?}",
            viewer.text()
        );
        compile(&mut viewer);
        compile(&mut viewer);
        // Only the equation is shown as it is written; what follows its
        // paragraph is typeset as ever.
        assert!(viewer.written.is_some(), "{:?}", viewer.problems);
        assert_eq!(
            viewer.document_revision,
            Some(viewer.revision),
            "{:?}",
            viewer.problems
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn an_equation_with_a_number_keeps_it_while_it_is_shown_as_written() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    #set math.equation(numbering: \"(1)\")\n\
                    Before.\n\n$ a + b $ <sum>\n\nSee @sum for it.\n";
        let (mut viewer, directory) = editing(text);
        let meant = glyph_places(&viewer, 0).len();
        press(&mut viewer, "gsgs/\\=a +<CR>gx");
        compile(&mut viewer);
        // What refers to it still finds it: the pages are made.
        assert!(viewer.written.is_some());
        assert_eq!(
            viewer.document_revision,
            Some(viewer.revision),
            "{:?}",
            viewer.problems
        );
        assert!(
            glyph_places(&viewer, 0).len() > meant + 3,
            "its marks are shown"
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn deleting_at_the_end_of_a_paragraph_takes_nothing_of_the_next() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    Before.\n\nLet $x_0$ be real.\n";
        let (mut viewer, directory) = editing(text);
        // In the string just begun nothing is shown behind the cursor
        // but the next paragraph, beyond marks that show as nothing.
        press(&mut viewer, "gsgs/Before<CR>A Then $a + \"");
        compile(&mut viewer);
        press(&mut viewer, "<Del>");
        assert!(
            viewer
                .text()
                .is_some_and(|text| text.contains("Then $a + \"\"$\n\nLet $x_0$")),
            "{:?}",
            viewer.text()
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_mark_that_takes_the_end_of_an_equation_with_it_ends_nothing_else() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    Before.\n\nLet $x_0$ be real, and $y$ too.\n\nAnd $z$ as well.\n";
        let (mut viewer, directory) = editing(text);
        // The backslash makes the `$` behind it a character: to Typst
        // the equation goes on to the next one's beginning, that one's
        // text is no equation, and what stands between them is one.
        press(&mut viewer, "gsgs/Before<CR>A Then $a + b \\");
        assert!(
            viewer
                .text()
                .is_some_and(|text| text.contains("Then $a + b \\$\n\nLet $x_0$")),
            "{:?}",
            viewer.text()
        );
        compile(&mut viewer);
        compile(&mut viewer);
        // The equation being typed is shown as written, whatever else
        // Typst complained of; all else is typeset as ever.
        assert!(viewer.written.is_some(), "{:?}", viewer.problems);
        assert_eq!(
            viewer.document_revision,
            Some(viewer.revision),
            "{:?}",
            viewer.problems
        );
        // With something behind it the mark is that, and the equation
        // an equation.
        press(&mut viewer, "a");
        compile(&mut viewer);
        assert_eq!(viewer.written, None);
        assert_eq!(
            viewer.document_revision,
            Some(viewer.revision),
            "{:?}",
            viewer.problems
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_blank_typed_in_a_footnote_leaves_the_view_where_it_is() {
        // The note's number stands at the top of a long page, its text
        // at the foot, out of the window while the top is shown.
        let text = "#set page(width: 300pt, height: 900pt)\n\
                    First words of the page.\n\nMore of them.\n";
        let (mut viewer, directory) = editing(text);
        let settle = |viewer: &mut Viewer| {
            compile(viewer);
            let _ = viewer.scene(&|_| false);
            viewer.view.top
        };
        // A note begun from the menu, and a word typed into it.
        press(&mut viewer, "gsgs/\\=words<CR>ea#foot<CR>");
        settle(&mut viewer);
        press(&mut viewer, "word");
        let top = settle(&mut viewer);
        assert!(
            viewer
                .text()
                .is_some_and(|text| text.contains("#footnote[word]")),
            "{:?}",
            viewer.text()
        );
        assert!(top > 100.0, "the foot of the page is shown: {top}");
        // Behind the blank the cursor has no place of its own in the
        // note. It is not looked for where the note's number stands.
        for key in [" ", "n", "e", "x", "t", " "] {
            press(&mut viewer, key);
            let _ = viewer.scene(&|_| false);
            let typed = viewer.view.top;
            let typeset = settle(&mut viewer);
            assert!(
                (typed - top).abs() < 1.0 && (typeset - top).abs() < 1.0,
                "after {key:?} the view is at {typed}, typeset at {typeset}; it was at {top}"
            );
        }
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_numbers_of_the_lines_stay_while_the_pages_are_made() {
        let text = "#set page(width: 300pt, height: 400pt)\n\
                    One line of words.\n\nTwo lines of them, the second.\n\n\
                    Three lines.\n\nFour lines.\n\nFive lines.\n";
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "gsgs/second<CR>");
        compile(&mut viewer);
        // How often the number 1 is drawn: beside the line above the
        // cursor's and the one below.
        let ones = |viewer: &mut Viewer| {
            let markup = crate::chrome::margin_number_markup(1, 6.5);
            let zoom = viewer.view.zoom;
            let id = viewer.chrome.render(&markup, zoom).map(|image| image.id);
            let scene = viewer.scene(&|_| false);
            let of_one = |quad: &&Quad| Some(quad.texture) == id;
            scene.quads.iter().filter(of_one).count()
        };
        assert_eq!(ones(&mut viewer), 2);
        // After every key, in the frames before its pages are there and
        // in the one in which they come: also where they are made a
        // second time of the same text, after what was typeset by
        // itself.
        for keys in [
            "A", " ", "w", "o", "r", "d", " ", "<Esc>", "x", "x", "u", "ia", "b", "<Esc>", "p", "u",
        ] {
            press(&mut viewer, keys);
            assert_eq!(ones(&mut viewer), 2, "after {keys}");
            assert_eq!(ones(&mut viewer), 2, "after {keys}, the next frame");
            compile(&mut viewer);
            assert_eq!(ones(&mut viewer), 2, "after {keys}, with its pages");
        }
        let _ = std::fs::remove_dir_all(directory);
    }
}
