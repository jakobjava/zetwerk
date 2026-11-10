//! The state of one zetwerk window, and what it should show.
//!
//! [`Viewer`] knows nothing about windows, GPUs or threads. It is told what
//! happened (a key, a finished compile, a resize) and asked what the frame
//! should look like. That keeps the whole behaviour of the program testable
//! without opening a window.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use tiny_skia::{Color, Pixmap};
use typst::utils::hash128;
use typst_layout::PagedDocument;
use zetwerk_render::{PixelRegion, Tile, TileKey, page_size_px, tile_origins, tiles};
use zetwerk_typst::{CompileOutput, Environment, Project, ProjectWorld};

use crate::chrome::{self, Chrome, Image, STATUS_HEIGHT, Status};
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

#[allow(
    clippy::struct_excessive_bools,
    reason = "independent switches of the interface, not states of one thing"
)]
pub struct Viewer {
    chrome: Chrome,
    project: Project,
    file: String,
    revision: u64,
    compiled_revision: Option<u64>,
    /// The last document that compiled. Stays on screen while a newer
    /// revision has errors.
    document: Option<Arc<PagedDocument>>,
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
    message: Option<String>,
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
            file,
            revision: 0,
            compiled_revision: None,
            document: None,
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

    /// Whether the user asked to close the window.
    #[must_use]
    pub fn wants_quit(&self) -> bool {
        self.quit
    }

    /// The current revision and a snapshot of it to compile.
    #[must_use]
    pub fn snapshot(&self) -> (u64, ProjectWorld) {
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
        self.view.width = self.window.0;
        self.view.height = self.window.1.saturating_sub(self.status_height()).max(1);
        self.relayout();
    }

    /// A file changed on disk. Returns whether a compile is needed.
    pub fn file_changed(&mut self, path: &Path) -> bool {
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
                self.message = Some(err.to_string());
                false
            }
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
            self.planned.clear();
            self.relayout();
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

    fn execute(&mut self, command: &str) {
        match command {
            "" => {}
            "q" | "quit" => self.quit = true,
            "perf" => self.performance_panel = !self.performance_panel,
            _ => match command.parse::<usize>() {
                Ok(page) if page >= 1 => self.view.scroll_to_page(page - 1, &self.layout),
                _ => self.message = Some(format!("not a command: {command}")),
            },
        }
    }

    /// What the window should show right now. `has` tells which images the
    /// compositor already holds.
    pub fn scene(&mut self, has: &dyn Fn(TextureId) -> bool) -> Scene {
        let mut scene = Scene::default();
        let (width, height) = (f64::from(self.window.0), f64::from(self.window.1));
        solid(&mut scene, has, BACKDROP, (0.0, 0.0, width, height));
        self.pages(&mut scene, has);
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
        let zoom = self.view.zoom;
        for visible in self.view.visible(&self.layout) {
            let (Some(page), Some(page_box)) = (
                document.pages().get(visible.index),
                self.layout.pages.get(visible.index),
            ) else {
                continue;
            };
            let origin = (visible.x as f64, visible.y as f64);
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
        }
    }

    fn status_line(&mut self, scene: &mut Scene, has: &dyn Fn(TextureId) -> bool) {
        let state = self.state();
        let command;
        let problem;
        let (mode, left, alert) = match (&self.mode, &self.message, self.problems.first()) {
            (Mode::Command(text), ..) => {
                command = format!(":{text}\u{2588}");
                ("COMMAND", command.as_str(), false)
            }
            (Mode::Normal, Some(message), _) => ("NORMAL", message.as_str(), true),
            (Mode::Normal, None, Some(first)) => {
                problem = match self.problems.len() {
                    1 => first.clone(),
                    count => format!("{first}  (+{} more)", count - 1),
                };
                ("NORMAL", problem.as_str(), true)
            }
            (Mode::Normal, None, None) => ("NORMAL", self.file.as_str(), false),
        };
        let progress = if self.is_compiled() {
            ""
        } else {
            "compiling  "
        };
        // 100 % is the size at which a point is 1/72 inch on a 96 dpi
        // display, scaled like everything else on a high-density one.
        let percent = state.zoom / (self.scale * 96.0 / 72.0) * 100.0;
        let right = format!("{progress}{}/{}  {percent:.0}%", state.page, state.pages);
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
        assert_eq!(viewer.message.as_deref(), Some("not a command: nonsense"));
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
}
