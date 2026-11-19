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
pub mod authors;
mod buffers;
mod cells;
mod changes;
pub mod comfort;
mod compose;
mod deck;
pub mod export;
mod far;
mod figures;
mod find;
mod focus;
mod handout;
mod help;
mod hints;
mod history;
mod import_offer;
mod inplace;
mod jump;
mod keymap;
mod language;
mod latex;
mod local;
mod manager;
mod margin;
pub mod notes;
pub(crate) mod options;
pub mod packages;
mod pages;
mod palette;
mod paste;
mod place;
mod pointer;
mod present;
mod present_scene;
pub mod refer;
mod refused;
mod registers;
pub mod report;
pub mod review;
mod saving;
mod set;
mod shape;
mod slides;
mod sources;
mod sources_add;
mod sources_broken;
mod sources_look;
mod sources_proposed;
mod sources_undo;
pub mod start;
mod steady;
mod structure;
mod styles;
pub mod tutor;
mod vim;
mod waiting;
mod written;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use crate::typst_text::{EMPTY, EMPTY_ARGUMENT, Ground, empty_places, missing_arguments};
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
use zetwerk_predict::{Lines, Replacement, predict};
use zetwerk_render::{PixelRegion, Tile, TileKey, page_size_px, tile_origins, tiles};
use zetwerk_typst::{CompileOutput, Environment, PageLines, PageMap, Project, ProjectWorld};
use zetwerk_vim::{Effect, Seek, Shown as ShownLine};

use self::agent::{AgentEdits, Mark, Proposal};
pub use self::changes::{ChangeOptions, Since};
pub use self::compose::Typing;
pub use self::keymap::Keymap;
pub use self::language::{Check, Issue, Report};
pub use self::present::{Talk, parse_duration as present_duration};
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
/// The same in an equation: a symbol that takes no room, set where the
/// next symbol typed would be and as large (small in what is raised).
const MATH_CURSOR_MARK: &str = "#box(width: 0pt, clip: true)[|];";

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
    /// What became of the lines of the paragraph, and whether they are
    /// justified: what the random typist counts.
    #[cfg_attr(not(test), allow(dead_code, reason = "only counted in tests"))]
    lines: zetwerk_predict::Lines,
    #[cfg_attr(not(test), allow(dead_code, reason = "only counted in tests"))]
    justified: bool,
}

/// Whether `prediction` breaks the lines as Typst does, in a document
/// whose paragraphs are `justified` or not: the error says why it may
/// not. The random typist (25 seeds of 150 keys on each document) found
/// a third of the lines worked out in justified text elsewhere than
/// Typst puts them, by up to 101 pt: Typst breaks justified text
/// weighing all of the paragraph, and a letter more on one line may
/// move a word on another (the last line of a paragraph, which is not
/// stretched, too). A part breaks them as Typst does. Ragged text Typst
/// breaks first fit, as the predictor does (all 81 lines broken anew in
/// the paper were right; in the document of chapters 50 of 158 were
/// not, which is still to be looked into).
fn as_typst_breaks(
    prediction: &zetwerk_predict::Prediction,
    justified: bool,
) -> Result<(), String> {
    // A line more or less moves all that is below it: what goes off a
    // full page or comes back onto it is known to a part, which knows
    // where the text of the page ends, and not here.
    if matches!(prediction.lines, Lines::More | Lines::Fewer) {
        return Err("the paragraph has a line more or less".to_owned());
    }
    if justified || prediction.justified {
        return Err("a line of justified text".to_owned());
    }
    Ok(())
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
    /// What of it is paper, and how that is coloured (see `comfort`).
    pub paper: crate::gpu::Paper,
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
    document: &'a local::Pages,
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
        if let Some(page) = pages::page_showing(self.document.pages(), self.world, self.file, then)
        {
            return Some(page);
        }
        let source = self.world.source(self.file).ok()?;
        let then = self.world.shown_at(self.file, then);
        zetwerk_typst::locate(self.document.pages(), &source, then, near)
            .map(|position| position.page)
    }

    /// Runs `ask` on the lines of page `index`.
    fn on<T>(&self, index: usize, ask: impl FnOnce(&PageLines) -> T) -> Option<T> {
        let page = self.document.pages().get(index)?;
        let mut lines = self.lines.borrow_mut();
        let lines = lines.entry(index).or_insert_with(|| {
            pages::lines_of(
                self.world,
                self.document.pages(),
                index,
                &page.frame,
                self.file,
            )
        });
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
    /// Whether an equation by itself stays where it began while it is
    /// typed into (see `steady`): off, unless turned on (`:set steady`);
    /// centred again only on the pause, it was found to lag behind.
    steady: bool,
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
    /// The pages of the last document that compiled, with a part typeset
    /// since put in (see `local`). Stay on screen while a newer revision
    /// has errors.
    document: Option<local::Pages>,
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
    /// What is being composed and is not yet typed (see [`compose`]).
    compose: compose::Compose,
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
    /// What pasting keeps between keys (see [`paste`]).
    paste: paste::Paste,
    /// What is known about the spelling and grammar of the text.
    language: language::Language,
    /// The notes on the text, and what reviewing keeps (see [`notes`]).
    notes: notes::Notes,
    /// Suggestions of agents and what is made of them (see [`review`]).
    review: review::Review,
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
    /// What changed since a version the user names (see [`changes`]).
    changes: changes::Changes,
    /// What the window waits for (see [`waiting`]).
    waiting: waiting::Waiting,
    /// What is kept of packages (see [`packages`]).
    packages: packages::Packages,
    /// What `i` in `:why` would import (see [`import_offer`]).
    import_offers: import_offer::ImportOffers,
    /// What the pages put in whole keep between keys (see [`pages`]).
    pages: pages::Pages,
    /// The talk being given, and what is known of the deck (see [`present`]).
    present: present::Present,
    /// What happened lately, for a report (see [`report`]).
    report: report::Recorder,
    /// The user's settings (see [`set`]).
    settings: set::Settings,
    /// Typewriter, focus, zen, night (see [`comfort`]), and where the
    /// document was left (see [`place`]).
    comfort: comfort::Comfort,
    place: place::Place,
    /// The lesson the window shows, if it does (see [`tutor`]).
    tutor: tutor::Tutor,
    pub stats: Stats,
    /// Who wrote what (see `authors`).
    authors: authors::Authors,
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
            steady: false,
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
            compose: compose::Compose::default(),
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
            paste: paste::Paste::default(),
            language: language::Language::default(),
            notes: notes::Notes::default(),
            review: review::Review::default(),
            margin: margin::Margin::default(),
            refused: refused::Refused::default(),
            caret_at: None,
            planned: HashMap::new(),
            shown: HashMap::new(),
            exports: export::Exports::default(),
            pointer: pointer::Pointer::default(),
            start: start::Start::default(),
            find: find::Find::default(),
            changes: changes::Changes::default(),
            waiting: waiting::Waiting::default(),
            packages: packages::Packages::default(),
            import_offers: import_offer::ImportOffers::default(),
            pages: pages::Pages::default(),
            present: present::Present::default(),
            report: report::Recorder::default(),
            settings: set::Settings::default(),
            comfort: comfort::Comfort::default(),
            place: place::Place::default(),
            tutor: tutor::Tutor::default(),
            stats: Stats::default(),
            authors: authors::Authors::default(),
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
        self.changes_opened();
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
        self.authors.configure(&options.authors);
        // The page is where a document is worked on; the source is there
        // for whoever wants to see it (`gs`).
        self.panes = options.layout.unwrap_or(Panes::Page);
        if options.slides {
            self.paging = Paging::Slides;
        }
        self.inserts.clone_from(&options.inserts);
        self.set_talk(options.talk.clone());
        self.report_configured(&options.report, options.font_set);
        self.set_language(&options.language);
        self.set_packages(&options.packages);
        self.set_review(options.review);
        self.changes_configured(&options.changes);
        self.comfort_configured(options.comfort);
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
        self.keep_review();
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
        if let Some(world) = self.proof_world() {
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
        // And the arguments a call cannot be without, behind those it has
        // (and the empty place of the last, which is put first).
        let missing = missing_arguments(source, &equation);
        put.extend(missing.into_iter().map(|place| (place, EMPTY_ARGUMENT)));
        // Behind a blank (out of what is raised, say) or a closing
        // bracket (behind `sqrt(2)`) nothing is typeset where the cursor
        // is, or something that stands for more than the place: the mark
        // is, as the next symbol would be. Not where what follows would
        // take it in (`^`, `_`, a bracket it is in, where it would be as
        // large as text and not as what is raised).
        let typing_here = editor.mode() == editor::Mode::Insert;
        let inside = equation.start < cursor && cursor < equation.end;
        let before = source.text()[..cursor].chars().next_back();
        let after = source.text()[cursor..].chars().next();
        let behind = before.is_some_and(|c| matches!(c, ' ' | '\n' | ')' | ']'));
        let free = after.is_some_and(|c| c.is_whitespace() || matches!(c, '$' | ',' | ';' | '&'));
        if typing_here && inside && behind && free {
            put.push((cursor, MATH_CURSOR_MARK));
        }
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
        self.shown_put
            .iter()
            .any(|(_, mark)| *mark != EMPTY && *mark != EMPTY_ARGUMENT)
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
        self.document.as_ref().map(local::Pages::document)
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
        let before = self.display;
        self.display = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        self.keep_size_on_screen(before);
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
    /// was that, or one of how the pages are looked at (see `comfort`).
    fn ui_command(&mut self, command: &str) -> bool {
        let Some(wanted) = command.strip_prefix("ui") else {
            return self.comfort_command(command);
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
        self.report_disk(path);
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
        if self.typewriter_follows() {
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
        let there = there.or_else(|| pages::shown_on(document.pages(), world, file, cursor));
        let shown = world.shown_at(file, cursor);
        let Some(position) =
            there.or_else(|| zetwerk_typst::locate(document.pages(), &source, shown, near))
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
        self.report_compiled(&output);
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
                self.document = Some(local::Pages::new(document, Arc::clone(&output.world)));
                self.arrange();
                // Now that it is known which files the document is made of.
                self.announce_unsaved_elsewhere();
            } else {
                self.document = Some(local::Pages::new(document, Arc::clone(&output.world)));
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
        self.place_compiled();
        self.tutor_compiled();
    }

    /// The user turned the wheel or dragged on the touchpad, by this many
    /// device pixels.
    pub fn scroll(&mut self, dx: f64, dy: f64) {
        if self.present_scroll(dy) {
            return;
        }
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
        if self.status_hidden() {
            return 0;
        }
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
        self.report_reached(press);
        // A page put in whole is not pasted into: its own keys say so.
        if self.page_here().is_none() && self.paste_key(press) {
            return;
        }
        self.message = None;
        self.place.touched = true;
        let press = self.shift_enter(press);
        let Some(press) = self.compose_key(press) else {
            return;
        };
        if self.stop_key(press)
            || self.set_key(press)
            || self.confirm_key(press)
            || self.cell_block_key(press)
        {
            return;
        }
        if self.present_key(press) {
            return;
        }
        if self.grid.is_some() {
            return self.grid_key(press);
        }
        if self.packages_key(press) {
            return;
        }
        if self.tutor_key(press) || self.help_key(press) {
            return;
        }
        if self.start_key(press)
            || self.refused_key(press)
            || self.margin_key(press)
            || self.pages_key(press)
        {
            return;
        }
        if self.changes_key(press) {
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
        let latex = latex::pending(editor);
        let typed = editor.key(press, &shown);
        let typing = editor.mode() == editor::Mode::Insert;
        // A paragraph is one line of the source and may fill pages: the
        // view follows the cursor, not the line it is in.
        let moved = editor.cursor() != cursor;
        if let Some(failure) = editor.take_failure() {
            self.alert(format!("not changed: {failure}"));
        }
        self.agent_mark = None;
        self.latex_told(latex);
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
        // Text typed at once is worked out once, at its last key.
        if self.compose.burst {
            return false;
        }
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
                    (document, Some(whole)) if document.is_whole() && Arc::ptr_eq(document.whole(), whole));
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
                document.pages(),
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
        let justified = crate::typst_text::rule_value(editor.source(), "par", "justify")
            .is_some_and(|value| value.trim() != "false");
        as_typst_breaks(&prediction, justified)?;
        self.planned.retain(|(planned, _, _), _| *planned != index);
        self.predicted = Some(Predicted {
            index,
            page: Page {
                frame: prediction.frame,
                ..page.clone()
            },
            typed: prediction.typed,
            end,
            lines: prediction.lines,
            justified: prediction.justified,
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
        if self.proof_shown() {
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
        if self.packages_command(command)
            || self.tutor_command(command)
            || self.help_command(command)
        {
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
            || self.present_command(command)
            || self.slide_command(command)
            || self.report_command(command)
            || self.paste_command(command)
            || self.changes_command(command)
            || self.compose_command(command)
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
            history if self.history_command(history) || self.authors_command(history) => {}
            find if self.find_command(find) => {}
            latex if self.latex_command(latex) => {}
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
        if let Some(scene) = self.present_scene(has) {
            return scene;
        }
        let mut scene = Scene::default();
        let (width, height) = (f64::from(self.window.0), f64::from(self.window.1));
        solid(&mut scene, has, BACKDROP, (0.0, 0.0, width, height));
        self.cursor_at = None;
        self.caret_at = None;
        self.equation_set = false;
        self.language_frame();
        self.changes_frame();
        if self.grid.is_some() {
            self.grid_scene(&mut scene, has);
            self.status_line(&mut scene, has);
            return scene;
        }
        self.pages(&mut scene, has);
        self.forget_far_pages();
        self.lens(&mut scene, has);
        // Drawn after the pages: a page zoomed wider than its pane must
        // not reach over the source.
        self.source_pane(&mut scene, has);
        self.composed_overlays(&mut scene, has);
        self.status_line(&mut scene, has);
        if self.performance_panel {
            self.performance(&mut scene, has);
        }
        self.proposal_panel(&mut scene, has);
        self.authors_panels(&mut scene, has);
        self.review_panels(&mut scene, has);
        self.hints(&mut scene, has);
        self.assist_panels(&mut scene, has);
        self.settings_panel(&mut scene, has);
        self.margin_panel(&mut scene, has);
        self.changes_panels(&mut scene, has);
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
            let paper = scene.quads.len();
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
            self.focus_veils(scene, has, visible.index, origin, page);
            self.line_numbers(scene, has, visible.index, origin);
            self.authors_page(scene, has, visible.index, origin);
            self.change_bars(scene, has, visible.index, origin);
            self.step_marks(scene, has, visible.index, origin);
            self.tutor_marks(scene, has, visible.index, origin);
            self.language_page(scene, has, visible.index, origin);
            self.find_marks(scene, has, visible.index, origin);
            self.margin_page(scene, has, visible.index, origin);
            self.page_cursor(scene, has, visible.index, origin);
            self.pages_marks(scene, has, visible.index, origin);
            self.review_page(scene, has, visible.index, origin);
            self.focus_marks(scene, has, visible.index, origin);
            // Last: the labels lie over all else.
            self.jump_labels(scene, has, visible.index, origin);
            self.paper_page(scene, paper, page, visible.index, origin);
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
        marked.extend(self.review_source_marks());
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
        let changes = self.changes_status();
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
            (_, Some((prompt, text))) => {
                Some(format!("{prompt}{text}{}\u{2588}", self.composed_inline()))
            }
            (Mode::Command(text), None) => {
                Some(format!(":{text}{}\u{2588}", self.composed_inline()))
            }
            _ => None,
        };
        let modified = state
            .editing
            .as_ref()
            .is_some_and(|editing| editing.modified);
        let name = self.file_label(modified);
        let name = self.authors_status().unwrap_or(name);
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
        let right = format!("{changes}{right}");
        let right = format!("{}{right}", self.tutor_status());
        let status = Status {
            mode,
            left,
            right: &right,
            alert,
        };
        let markup = chrome::status_markup(&status, f64::from(self.window.0) / self.scale);
        let y = f64::from(self.window.1) - f64::from(self.status_height());
        let Some(y) = self.zen_status(y, command.is_some() || self.message.is_some()) else {
            return;
        };
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
mod tests;
