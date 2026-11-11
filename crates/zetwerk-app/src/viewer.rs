//! The state of one zetwerk window, and what it should show.
//!
//! [`Viewer`] knows nothing about windows, GPUs or threads. It is told what
//! happened (a key, a finished compile, a resize) and asked what the frame
//! should look like. That keeps the whole behaviour of the program testable
//! without opening a window.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
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
use zetwerk_vim::Effect;

use crate::chrome::{self, Chrome, Image, ROW_HEIGHT, RowText, STATUS_HEIGHT, Status};
use crate::editor::{self, Editor, Rows};
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
}

/// A compile shorter than this (in milliseconds) is run for every key
/// that the page cannot show without one.
const QUICK_COMPILE_MS: f64 = 12.0;

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

/// Whether the page shows what is at byte `offset` of `source` the way it
/// is written: the words of a paragraph, a heading or a list item. Markup,
/// math and code show as something else or not at all.
fn is_prose(source: &Source, offset: usize) -> bool {
    let Some(leaf) = LinkedNode::new(source.root()).leaf_at(offset, Side::After) else {
        // Beyond the last leaf: the end of the text.
        return true;
    };
    match leaf.kind() {
        SyntaxKind::Text => true,
        SyntaxKind::Space
        | SyntaxKind::Parbreak
        | SyntaxKind::SmartQuote
        | SyntaxKind::Shorthand
        | SyntaxKind::Linebreak
        | SyntaxKind::Escape => leaf.parent_kind().is_some_and(|parent| {
            matches!(
                parent,
                SyntaxKind::Markup
                    | SyntaxKind::Strong
                    | SyntaxKind::Emph
                    | SyntaxKind::Heading
                    | SyntaxKind::ListItem
                    | SyntaxKind::EnumItem
                    | SyntaxKind::TermItem
            )
        }),
        _ => false,
    }
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
    /// The page, the line and the byte of the compiled text the cursor
    /// last came to here. What a call put on the page is many lines for
    /// one place in the source: the place alone does not say which.
    landed: &'a Cell<Option<(usize, usize, usize)>>,
}

impl PageRows<'_> {
    /// Runs `ask` on the lines of page `index`.
    fn on<T>(&self, index: usize, ask: impl FnOnce(&PageLines) -> T) -> Option<T> {
        let page = self.document.pages().get(index)?;
        let mut lines = self.lines.borrow_mut();
        let lines = lines
            .entry(index)
            .or_insert_with(|| PageMap::build(self.world, &page.frame).lines(self.file));
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
    fn row(&self, from: usize, rows: isize, across: Option<f64>) -> Option<(usize, f64)> {
        let source = self.world.source(self.file).ok()?;
        let from = self.change.back_or_edge(from);
        let (page, line, shown_at) = match self.landed.get() {
            Some((page, line, at)) if at == from => (
                page,
                line,
                self.on(page, |lines| lines.across(line, from))??,
            ),
            _ => {
                // On the page it was last found on, if that shows it:
                // the same text may be on many pages (a heading and the
                // table of contents), and what a call made is found by
                // no search.
                let near = self.cursor_page.get();
                let page = match self.on(near, |lines| lines.shows(from)) {
                    Some(true) => near,
                    _ => zetwerk_typst::locate(self.document, &source, from, near)?.page,
                };
                let (line, shown_at) = self.on(page, |lines| lines.find(from))??;
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
        let to = loop {
            let Place { page, line, across } = place;
            if let Some(to) = self.on(page, |lines| lines.at(line, across, from))? {
                break to;
            }
            if !self.step(&mut place, down) {
                return stay;
            }
        };
        let Place { page, line, across } = place;
        self.cursor_page.set(page);
        self.landed.set(Some((page, line, to)));
        Some((self.change.forth(to), across))
    }
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "independent switches of the interface, not states of one thing"
)]
pub struct Viewer {
    chrome: Chrome,
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
    /// The sources that document was compiled from, and for the pages
    /// that have been asked about, where their glyphs come from in them.
    compiled_world: Option<Arc<ProjectWorld>>,
    maps: HashMap<usize, PageMap>,
    /// The lines of text of the pages the cursor has moved up or down on.
    page_lines: RefCell<HashMap<usize, PageLines>>,
    /// The page the cursor was last found on: where to look for it first.
    cursor_page: Cell<usize>,
    /// See [`PageRows::landed`].
    landed: Cell<Option<(usize, usize, usize)>>,
    /// The revision that document was compiled from. Differs from
    /// `compiled_revision` while the newest text has errors.
    document_revision: Option<u64>,
    predicted: Option<Predicted>,
    /// How the text of a revision differs from the compiled text, where
    /// that is one replacement.
    changed: Option<(u64, Change)>,
    /// Where the cursor is on the pages as last drawn, in device pixels:
    /// how far across, and its top and bottom.
    cursor_at: Option<(f64, f64, f64)>,
    /// Where in the compiled text the last insertion was worked out for,
    /// and the page it is on.
    predicted_at: Option<(usize, usize)>,
    /// Why the last change could not be shown without compiling.
    declined: Option<String>,
    problems: Vec<String>,
    layout: Layout,
    view: Viewport,
    window: (u32, u32),
    /// Device pixels per logical pixel.
    scale: f64,
    /// Whether the zoom follows the window width.
    fit: bool,
    mode: Mode,
    /// A `g` waiting for its second key.
    pending_g: bool,
    message: Option<Message>,
    performance_panel: bool,
    quit: bool,
    /// Tiles of the current document at the current zoom, by page and tile
    /// origin, so that a frame does not crop and hash pages again.
    planned: HashMap<(usize, u32, u32), Tile>,
    shown: HashMap<usize, Shown>,
    pub stats: Stats,
}

impl Viewer {
    /// A viewer for `project` in a window of `window` device pixels.
    #[must_use]
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
            compiled_world: None,
            maps: HashMap::new(),
            page_lines: RefCell::default(),
            cursor_page: Cell::new(0),
            landed: Cell::new(None),
            document_revision: None,
            predicted: None,
            changed: None,
            cursor_at: None,
            predicted_at: None,
            declined: None,
            problems: Vec::new(),
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
            fit: true,
            mode: Mode::Normal,
            pending_g: false,
            message: None,
            performance_panel: false,
            quit: false,
            planned: HashMap::new(),
            shown: HashMap::new(),
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
        self.editor = Some(editor);
        self.panes = Panes::Split;
        match opened {
            Opened::Clean => {}
            Opened::Restored => self.notice("unsaved changes from the last session are back"),
            Opened::ChangedWhileClosed => {
                self.notice("the file changed since the last session; its undo history is gone");
            }
            Opened::Conflict { journal, .. } => self.alert(format!(
                "the file changed while unsaved changes were pending; they are kept in {}",
                journal.display()
            )),
            Opened::JournalUnusable { journal } => self.alert(format!(
                "the journal could not be read and was set aside as {}",
                journal.display()
            )),
        }
        self.arrange();
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
        self.editor.as_ref().and_then(Editor::unsynced_since)
    }

    /// Forces the journal onto the disk.
    pub fn sync(&mut self) {
        if let Some(editor) = &mut self.editor
            && let Err(err) = editor.sync()
        {
            self.alert(format!("cannot write the journal: {err}"));
        }
    }

    /// The current revision and a snapshot of it to compile.
    #[must_use]
    pub fn snapshot(&mut self) -> (u64, ProjectWorld) {
        if let Some(editor) = &self.editor {
            self.project.set_source(editor.source().clone());
        }
        (self.revision, self.project.snapshot())
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
    fn split(&self) -> (u32, u32, u32) {
        let width = self.window.0;
        match (&self.editor, self.panes) {
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
        self.scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        self.arrange();
    }

    /// A file changed on disk. Returns whether a compile is needed.
    pub fn file_changed(&mut self, path: &Path) -> bool {
        if self.editor.is_some() && self.project.main_path() == Some(path) {
            return self.edited_file_changed();
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
        if self.panes == Panes::Source {
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
        let Ok(source) = world.source(self.project.main()) else {
            return;
        };
        let cursor = change.back_or_edge(editor.cursor());
        // Where it was last found, if it is there still: that page shows
        // it for certain, and may show it by something no search finds.
        let near = self.cursor_page.get();
        let file = self.project.main();
        let there = document.pages().get(near).and_then(|page| {
            let map = self
                .maps
                .entry(near)
                .or_insert_with(|| PageMap::build(world, &page.frame));
            let caret = map.caret(file, cursor).map(|caret| caret.cell);
            let cell = caret.or_else(|| map.object(file, cursor).first().copied())?;
            Some(zetwerk_typst::PagePosition {
                page: near,
                x: cell.min.x.to_pt(),
                y: cell.min.y.to_pt(),
            })
        });
        let Some(position) =
            there.or_else(|| zetwerk_typst::locate(document, &source, cursor, near))
        else {
            return;
        };
        self.cursor_page.set(position.page);
        let Some(page) = self.layout.pages.get(position.page) else {
            return;
        };
        let zoom = self.view.zoom;
        let y = f64::from(page.y) + position.y * zoom;
        let (top, height) = (self.view.top * zoom, f64::from(self.view.height));
        let margin = height / 8.0;
        if y < top + margin || y > top + height - margin {
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
        self.problems = if output.compiled.panicked {
            vec!["the Typst compiler crashed on this document".to_owned()]
        } else {
            let world = &*output.world;
            let describe = |error| zetwerk_typst::describe(world, error);
            output.compiled.errors.iter().map(describe).collect()
        };
        if let Some(document) = output.compiled.document {
            self.document = Some(Arc::new(document));
            self.compiled_world = Some(Arc::clone(&output.world));
            self.document_revision = Some(output.revision);
            self.predicted = None;
            self.predicted_at = None;
            self.maps.clear();
            self.page_lines.get_mut().clear();
            self.landed.set(None);
            self.planned.clear();
            self.relayout();
            self.follow_cursor();
        }
    }

    /// The user turned the wheel or dragged on the touchpad, by this many
    /// device pixels.
    pub fn scroll(&mut self, dx: f64, dy: f64) {
        self.view.scroll_by(dx, dy, &self.layout);
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

    /// A key was pressed.
    pub fn key(&mut self, press: KeyPress) {
        self.message = None;
        if self.editor.is_some() {
            return self.editor_key(press);
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

    /// The pointer was pressed at this place in the window, in device
    /// pixels: the cursor goes to the text there, in the source or on a
    /// page.
    pub fn click(&mut self, (x, y): (f64, f64)) {
        let (source_width, page_left, _) = self.split();
        let height = f64::from(self.window.1.saturating_sub(self.status_height()));
        if self.editor.is_none() || y >= height {
            return;
        }
        if x < f64::from(source_width) {
            let (gutter, cell, row_height) = self.source_metrics;
            let columns = self.source_size.0;
            let row = (y / row_height).floor().max(0.0) as usize;
            let across = ((x - gutter) / cell).floor().max(0.0) as usize;
            if let Some(editor) = &mut self.editor {
                let offset = editor.offset_at(columns, row, across);
                editor.set_cursor(offset);
            }
        } else if let Some(change) = self.places() {
            let (Some(document), Some(world)) = (&self.document, &self.compiled_world) else {
                return;
            };
            let zoom = self.view.zoom;
            let file = self.project.main();
            for visible in self.view.visible(&self.layout) {
                let (Some(page), Some(page_box)) = (
                    document.pages().get(visible.index),
                    self.layout.pages.get(visible.index),
                ) else {
                    continue;
                };
                let left = f64::from(page_left) + visible.x as f64;
                let top = visible.y as f64;
                let inside = x >= left
                    && x < left + f64::from(page_box.width)
                    && y >= top
                    && y < top + f64::from(page_box.height);
                if !inside {
                    continue;
                }
                let map = self
                    .maps
                    .entry(visible.index)
                    .or_insert_with(|| PageMap::build(world, &page.frame));
                let point = typst::layout::Point::new(
                    typst::layout::Abs::pt((x - left) / zoom),
                    typst::layout::Abs::pt((y - top) / zoom),
                );
                if let (Some((found, offset)), Some(editor)) =
                    (map.source_at(point), &mut self.editor)
                    && found == file
                {
                    editor.set_cursor(change.forth(offset));
                }
                return;
            }
        }
    }

    /// A key for the source text.
    fn editor_key(&mut self, press: KeyPress) {
        let places = self.places();
        let Some(editor) = &mut self.editor else {
            return;
        };
        let was_typing = editor.mode() == editor::Mode::Insert;
        let cursor = editor.cursor();
        let pages = match (&self.document, &self.compiled_world, places) {
            (Some(document), Some(world), Some(change)) => Some(PageRows {
                document,
                world,
                file: self.project.main(),
                lines: &self.page_lines,
                cursor_page: &self.cursor_page,
                change,
                landed: &self.landed,
            }),
            _ => None,
        };
        let shown = match self.panes {
            Panes::Page => editor::Shown::Pages(pages.as_ref().map(|pages| pages as &dyn Rows)),
            Panes::Split | Panes::Source => editor::Shown::Source {
                columns: self.source_size.0,
            },
        };
        let typed = editor.key(press, &shown);
        let typing = editor.mode() == editor::Mode::Insert;
        // A paragraph is one line of the source and may fill pages: the
        // view follows the cursor, not the line it is in.
        let moved = editor.cursor() != cursor;
        if let Some(failure) = editor.take_failure() {
            self.alert(format!("not changed: {failure}"));
        }
        if typed.changed {
            self.revision += 1;
            let shown = self.predict();
            self.edited = Some(if !typing {
                Edited::Command
            } else if shown || self.stats.compile_ms > QUICK_COMPILE_MS {
                Edited::Typing
            } else {
                Edited::Unpredicted
            });
        } else if was_typing && !typing && !self.is_compiled() {
            self.edited = Some(Edited::LeftInsert);
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
        if moved {
            self.follow_cursor();
        }
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
            Err(why) => {
                self.stats.declined += 1;
                self.declined = Some(why);
                false
            }
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
        let file = self.project.main();
        let base = world
            .source(file)
            .map_err(|_| "the compiled source is gone".to_owned())?;
        let (offset, removed, text) = match difference(base.text(), editor.text()) {
            Difference::Nothing => {
                // Back at the text the pages show.
                self.document_revision = Some(self.revision);
                self.compiled_revision = Some(self.revision);
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
        self.changed = Some((self.revision, change));
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
        // The page is that of the text next to the insertion. While typing
        // goes on in one place it stays the same page, and is not looked
        // for again.
        let index = match self.predicted_at {
            Some((at, index)) if at == offset => index,
            _ => zetwerk_typst::locate(
                document,
                &base,
                offset.saturating_sub(1),
                self.cursor_page.get(),
            )
            .map(|position| position.page)
            .ok_or("the text is on no page")?,
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
        if self.predicted.is_some() || self.document.is_none() {
            return None;
        }
        if self.document_revision == Some(self.revision) {
            return Some(Change::NONE);
        }
        self.changed
            .filter(|(revision, _)| *revision == self.revision)
            .map(|(_, change)| change)
    }

    /// Whether the pages show text that has not been compiled, worked out
    /// from what was typed.
    #[must_use]
    pub fn is_predicted(&self) -> bool {
        self.predicted.is_some()
    }

    fn execute(&mut self, command: &str) {
        let modified = self.editor.as_ref().is_some_and(Editor::is_modified);
        match command {
            "" => {}
            "q" | "quit" if modified => {
                self.alert("unsaved changes: :wq saves them, :q! leaves them unsaved");
            }
            // Unsaved changes stay in the journal: they are back the next
            // time the file is opened, and can be undone then.
            "q" | "quit" | "q!" | "quit!" => self.quit = true,
            "w" | "write" => {
                self.save(false);
            }
            "w!" | "write!" => {
                self.save(true);
            }
            "wq" => self.quit = self.save(false),
            "wq!" => self.quit = self.save(true),
            "x" | "xit" => self.quit = !modified || self.save(false),
            "e!" | "edit!" => self.reload(),
            "perf" => self.performance_panel = !self.performance_panel,
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
                Ok(page) if page >= 1 => self.view.scroll_to_page(page - 1, &self.layout),
                _ => self.alert(format!("not a command: {command}")),
            },
        }
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
                self.notice(format!("{} written, {lines} lines", self.file));
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
                self.revision += 1;
                self.edited = Some(Edited::Command);
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
        self.pages(&mut scene, has);
        self.lens(&mut scene, has);
        // Drawn after the pages: a page zoomed wider than its pane must
        // not reach over the source.
        self.source_pane(&mut scene, has);
        self.status_line(&mut scene, has);
        if self.performance_panel {
            self.performance(&mut scene, has);
        }
        scene
    }

    fn pages(&mut self, scene: &mut Scene, has: &dyn Fn(TextureId) -> bool) {
        let Some(document) = self.document.clone() else {
            return;
        };
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
            self.page_cursor(scene, has, visible.index, origin);
        }
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
        if let (Some(predicted), Some(editor)) = (&self.predicted, &self.editor) {
            // The page is a predicted one: all that is known of it is
            // where the typed text is.
            if predicted.index == index && editor.cursor() == predicted.end {
                let cell = predicted.typed;
                let x = origin.0 + cell.max.x.to_pt() * zoom;
                let y = origin.1 + cell.min.y.to_pt() * zoom;
                let height = (cell.max.y - cell.min.y).to_pt() * zoom;
                tint(scene, has, PAGE_BAR, (x.round(), y, bar, height));
                self.cursor_at = Some((x, y, y + height));
            }
            return;
        }
        let Some(change) = self.places() else {
            return;
        };
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
        let file = self.project.main();
        let place = |rect: Rect| {
            (
                origin.0 + rect.min.x.to_pt() * zoom,
                origin.1 + rect.min.y.to_pt() * zoom,
                (rect.max.x - rect.min.x).to_pt() * zoom,
                (rect.max.y - rect.min.y).to_pt() * zoom,
            )
        };
        if let Some(selection) = editor.selection() {
            let selection =
                change.back_or_edge(selection.start)..change.back_or_edge(selection.end);
            for rect in map.selection(file, &selection) {
                tint(scene, has, PAGE_SELECTION, place(rect));
            }
        }
        // Text put in since the compile is nowhere on the page; the place
        // it was put in at is.
        let cursor = change.back(editor.cursor());
        let caret = map.caret(file, change.back_or_edge(editor.cursor()));
        if let Some(caret) = &caret {
            let (x, y, _, height) = place(caret.cell);
            self.cursor_at = Some((x, y, y + height));
        }
        let Some(cursor) = cursor else {
            return;
        };
        if caret.as_ref().is_none_or(|caret| caret.after) {
            // No text of the source is shown here; something made of it
            // may be, such as what a reference refers to by.
            let object = map.object(file, cursor);
            if let Some(first) = object.first() {
                let (x, y, _, height) = place(*first);
                self.cursor_at = Some((x, y, y + height));
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
        if self.panes != Panes::Page {
            return false;
        }
        let unseen = match self.places() {
            Some(change) => change.back(editor.cursor()).is_none(),
            None => self.predicted.is_none(),
        };
        let typing_unseen = editor.mode() == editor::Mode::Insert
            && self.predicted.is_none()
            && self.document_revision != Some(self.revision);
        unseen || typing_unseen || !is_prose(editor.source(), editor.cursor())
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
        let block = lens_block(editor.source(), cursor);
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
        let top = editor.scroll_into_view(columns, visible, SCROLL_MARGIN);
        editor.set_view(columns, visible);
        let (cursor, selection, mode) = (editor.cursor(), editor.selection(), editor.mode());
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
            if let Some(selection) = &selection
                && selection.start <= row.range.end
                && selection.end > row.range.start
            {
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
        let name = if modified {
            format!("{} [+]", self.file)
        } else {
            self.file.clone()
        };
        let problem = self
            .problems
            .first()
            .map(|first| match self.problems.len() {
                1 => first.clone(),
                count => format!("{first}  (+{} more)", count - 1),
            });
        let (left, alert) = match (&command, &self.message, &problem) {
            (Some(command), ..) => (command.as_str(), false),
            (None, Some(message), _) => (message.text.as_str(), message.alert),
            (None, None, Some(problem)) => (problem.as_str(), true),
            (None, None, None) => (name.as_str(), false),
        };
        let progress = if self.is_compiled() {
            ""
        } else {
            "compiling  "
        };
        let place = editing.map_or_else(String::new, |editing| {
            let recording = editing
                .recording
                .map_or_else(String::new, |register| format!("recording @{register}  "));
            format!("{recording}{}:{}  ", editing.line, editing.column)
        });
        // 100 % is the size at which a point is 1/72 inch on a 96 dpi
        // display, scaled like everything else on a high-density one.
        let percent = state.zoom / (self.scale * 96.0 / 72.0) * 100.0;
        let right = format!(
            "{progress}{place}{}/{}  {percent:.0}%",
            state.page, state.pages
        );
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
                    stats.predicted,
                    stats.predicted + stats.declined
                ),
            ),
            (
                "last not shown",
                self.declined.clone().unwrap_or_else(|| "–".to_owned()),
            ),
            ("tile memory", format!("{megabytes:.0} MB")),
        ];
        let margin = PANEL_MARGIN * self.scale;
        let right = f64::from(self.window.0) - margin;
        self.panel(scene, has, &chrome::panel_markup(&rows), |width| {
            (right - width, margin)
        });
    }

    /// Renders chrome `markup` and places it where `position` says, given
    /// the rendered width in pixels.
    fn panel(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        markup: &str,
        position: impl FnOnce(f64) -> (f64, f64),
    ) {
        let Some(image) = self.chrome.render(markup, self.scale) else {
            return;
        };
        let (width, height) = (image.pixmap.width(), image.pixmap.height());
        let (x, y) = position(f64::from(width));
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
    fn compile(viewer: &mut Viewer) {
        let (revision, world) = viewer.snapshot();
        let compiled = zetwerk_typst::compile(&world);
        viewer.compiled(CompileOutput {
            revision,
            world: Arc::new(world),
            compiled,
            duration: Duration::from_millis(3),
        });
    }

    fn press(viewer: &mut Viewer, notation: &str) {
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
    fn directory(text: &str) -> PathBuf {
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
    fn open(directory: &Path) -> (Viewer, Opened) {
        let path = directory.join("main.typ");
        let env = Environment::new(FontSet::Embedded);
        let project = Project::open(Arc::clone(&env), &path).unwrap();
        let (editor, opened) =
            Editor::open(&path, &directory.join("state"), project.main()).unwrap();
        let mut viewer = Viewer::new(env, project, (1000, 620), 1.0)
            .unwrap_or_else(|| panic!("viewer sets up"))
            .editing(editor, &opened);
        compile(&mut viewer);
        (viewer, opened)
    }

    fn editing(text: &str) -> (Viewer, PathBuf) {
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
        // to be worked out; it is quick to compile instead.
        assert_eq!(viewer.take_edited(), Some(Edited::Unpredicted));
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
        press(&mut viewer, "otwo<Esc>:q!<CR>");
        assert!(viewer.wants_quit());
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
    fn tints(viewer: &mut Viewer, rgba: [u8; 4]) -> Vec<Quad> {
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
        // A star makes the rest of the paragraph bold: nothing to work out
        // without compiling.
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
    fn the_lens_shows_the_source_where_the_page_does_not() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    Some words and $x^2 + y^2$ in a line.\n\n\
                    = A heading\n";
        let (mut viewer, directory) = editing(text);
        let lens = |viewer: &mut Viewer| {
            let edge = TextureId(hash128(&("solid", LENS_EDGE)));
            let scene = viewer.scene(&|_| false);
            let quads = scene.quads;
            quads.into_iter().find(|quad| quad.texture == edge)
        };
        // Beside the source it has nothing to add.
        press(&mut viewer, "/x<CR>");
        assert_eq!(lens(&mut viewer), None);
        // With only pages: none on the words of a paragraph, one on math.
        press(&mut viewer, "gsgs/words<CR>");
        assert_eq!(lens(&mut viewer), None);
        press(&mut viewer, "/x<CR>");
        let on_math = lens(&mut viewer).unwrap();
        let cursor = tints(&mut viewer, PAGE_CURSOR);
        assert_eq!(cursor.len(), 1);
        assert!(on_math.y > cursor[0].y, "under the cursor's line");
        let (width, height) = viewer.window;
        assert!(on_math.x >= 0.0 && on_math.x + on_math.width <= width as f32);
        assert!(on_math.y + on_math.height <= height as f32);
        // On code that shows nowhere, in the corner.
        press(&mut viewer, "gg");
        assert!(lens(&mut viewer).is_some());
        // While typing what the page cannot show yet, and no longer once
        // it does.
        press(&mut viewer, "/heading<CR>");
        assert_eq!(lens(&mut viewer), None);
        press(&mut viewer, "inew ");
        assert!(!viewer.is_predicted(), "a heading is not worked out");
        assert!(lens(&mut viewer).is_some());
        press(&mut viewer, "<Esc>");
        compile(&mut viewer);
        assert_eq!(lens(&mut viewer), None);
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
        press(&mut viewer, "$");
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
            let file = viewer.project.main();
            let count = |page: &Page| PageMap::build(world, &page.frame).lines(file).len();
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
        report("half a page down", "<C-d>", &mut viewer);
        report("deleting words", "dwdwdwdwdwdwdwdw", &mut viewer);
        report("undo", "uuuuuuuu", &mut viewer);
        let _ = std::fs::remove_dir_all(directory);
    }
}
