//! Pages as slides: one at a time, filling the window (`:slides`), and
//! all of them small, side by side, to find one among them (`:grid`).

use zetwerk_render::{PixelRegion, page_size_px, tiles};

use super::{PAPER, Quad, RowText, Scene, TextureId, Viewer, row_images, solid, texture};
use crate::keys::{Key, KeyPress};
use crate::view::Layout;

/// How the pages are laid out in the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Paging {
    /// One under the other; the view moves over them freely.
    #[default]
    Scroll,
    /// One at a time, whole.
    Slides,
}

/// The overview of all pages, and the one picked out in it.
pub(super) struct Grid {
    chosen: usize,
}

/// How wide a page is in the overview, about, in logical pixels, and the
/// room between two of them and around a number.
const THUMB_WIDTH: f64 = 230.0;
const THUMB_GAP: f64 = 18.0;
/// How far the view has to be pushed, in logical pixels, before slides
/// change.
const FLIP_AFTER: f64 = 50.0;
/// The frame around the page picked out in the overview.
const CHOSEN: [u8; 3] = [0xfa, 0xbd, 0x2f];

/// Where the pages are in the overview: how many side by side, the size
/// of the place each has, and the zoom they are drawn at.
struct Cells {
    columns: usize,
    cell: (f64, f64),
    zoom: f64,
}

impl Viewer {
    /// How the pages are laid out.
    #[must_use]
    pub fn paging(&self) -> Paging {
        self.paging
    }

    /// Lays the pages out as slides or to scroll over.
    pub fn set_paging(&mut self, paging: Paging) {
        self.paging = paging;
        self.fit = true;
        self.planned.clear();
        self.slide = self.view.current_page(&self.layout).unwrap_or(0);
        self.relayout();
        self.follow_cursor();
    }

    /// The layout for slides: every page whole, at one zoom for all.
    pub(super) fn slide_layout(&mut self) {
        let Some(document) = &self.document else {
            return;
        };
        if self.fit {
            let widest = |length: fn(&typst_layout::Page) -> f64| {
                document.pages().iter().map(length).fold(0.0, f64::max)
            };
            let width = widest(|page| page.frame.width().to_pt());
            let height = widest(|page| page.frame.height().to_pt());
            let zoom = self.view.fit_page(width, height);
            if (zoom - self.view.zoom).abs() > f64::EPSILON {
                self.view.zoom = zoom;
                self.planned.clear();
            }
        }
        let zoom = self.view.zoom;
        let sizes = document.pages().iter().map(|page| page_size_px(page, zoom));
        self.layout = Layout::slides(sizes, zoom, self.view.height);
        self.slide = self.slide.min(self.layout.pages.len().saturating_sub(1));
        self.view.center_page(self.slide, &self.layout);
    }

    /// Shows slide `index`.
    pub(super) fn show_slide(&mut self, index: usize) {
        self.slide = index.min(self.layout.pages.len().saturating_sub(1));
        self.view.center_page(self.slide, &self.layout);
    }

    /// A push at the view while slides are shown: enough of it, and the
    /// next slide comes, or the one before.
    pub(super) fn push_slides(&mut self, dy: f64) {
        self.pushed += dy;
        if self.pushed.abs() < FLIP_AFTER * self.scale {
            return;
        }
        let forward = self.pushed > 0.0;
        self.pushed = 0.0;
        let next = if forward {
            self.slide + 1
        } else {
            self.slide.saturating_sub(1)
        };
        self.show_slide(next);
    }

    /// Puts the cursor on the first text of page `index`, in whichever
    /// file that is, and brings the page into view.
    pub(super) fn goto_page(&mut self, index: usize) {
        let pages = self.layout.pages.len();
        if pages == 0 {
            return;
        }
        let index = index.min(pages - 1);
        let first = match (&self.document, &self.compiled_world, self.places()) {
            (Some(document), Some(world), Some(change)) => {
                document.pages().get(index).and_then(|page| {
                    let lines = zetwerk_typst::PageMap::build(world, &page.frame).lines();
                    let line = lines.enter(true)?;
                    let (file, offset) = lines.first(line)?;
                    let here = file == self.file_id();
                    Some((file, if here { change.forth(offset) } else { offset }))
                })
            }
            _ => None,
        };
        self.cursor_page.set(index);
        match first {
            Some((file, offset)) if self.editor.is_some() => self.open_file(file, Some(offset)),
            _ => {}
        }
        match self.paging {
            Paging::Slides => self.show_slide(index),
            Paging::Scroll => self.view.scroll_to_page(index, &self.layout),
        }
    }

    /// `]]` and `[[`: to the page after the one the cursor is on, or the
    /// one before.
    pub(super) fn step_page(&mut self, forward: bool) {
        let current = match self.paging {
            Paging::Slides => self.slide,
            Paging::Scroll => self.cursor_page.get(),
        };
        let target = if forward {
            current + 1
        } else {
            current.saturating_sub(1)
        };
        self.goto_page(target);
    }

    /// Opens the overview of all pages, with the page in view picked out.
    pub(super) fn open_grid(&mut self) {
        if self.document.is_none() {
            self.notice("there are no pages yet");
            return;
        }
        let chosen = match self.paging {
            Paging::Slides => self.slide,
            Paging::Scroll => self.view.current_page(&self.layout).unwrap_or(0),
        };
        self.grid = Some(Grid { chosen });
    }

    /// Whether the overview of all pages is up.
    #[must_use]
    pub fn is_grid(&self) -> bool {
        self.grid.is_some()
    }

    fn cells(&self) -> Option<Cells> {
        let document = self.document.as_ref()?;
        let widest = |length: fn(&typst_layout::Page) -> f64| {
            document.pages().iter().map(length).fold(1.0, f64::max)
        };
        let (width, height) = (
            widest(|page| page.frame.width().to_pt()),
            widest(|page| page.frame.height().to_pt()),
        );
        let gap = THUMB_GAP * self.scale;
        let room = f64::from(self.window.0) - gap;
        let columns = ((room / (THUMB_WIDTH * self.scale + gap)).floor() as usize).clamp(1, 12);
        let cell_width = room / columns as f64;
        let zoom = (cell_width - gap) / width;
        Some(Cells {
            columns,
            cell: (
                cell_width,
                height * zoom + gap + super::ROW_HEIGHT * self.scale,
            ),
            zoom,
        })
    }

    /// A key while the overview is up: the arrows and `hjkl` pick a page,
    /// Enter goes to it, Escape leaves things as they were.
    pub(super) fn grid_key(&mut self, press: KeyPress) {
        let (Some(cells), Some(grid)) = (self.cells(), &mut self.grid) else {
            self.grid = None;
            return;
        };
        let pages = self.layout.pages.len().max(1);
        let chosen = grid.chosen;
        let moved = match (press.key, press.ctrl) {
            (Key::Escape | Key::Char('q'), _) | (Key::Char('c' | '['), true) => {
                self.grid = None;
                return;
            }
            (Key::Enter, _) => {
                self.grid = None;
                return self.goto_page(chosen);
            }
            (Key::Left | Key::Char('h'), false) => chosen.saturating_sub(1),
            (Key::Right | Key::Char('l'), false) => chosen + 1,
            (Key::Up | Key::Char('k'), false) => {
                if chosen >= cells.columns {
                    chosen - cells.columns
                } else {
                    chosen
                }
            }
            (Key::Down | Key::Char('j'), false) => {
                if chosen + cells.columns < pages {
                    chosen + cells.columns
                } else {
                    chosen
                }
            }
            (Key::Home | Key::Char('g'), false) => 0,
            (Key::End | Key::Char('G'), false) => pages - 1,
            _ => chosen,
        };
        grid.chosen = moved.min(pages - 1);
    }

    /// A click in the overview: on a page, it goes there.
    pub(super) fn grid_click(&mut self, (x, y): (f64, f64)) {
        let (Some(cells), Some(grid)) = (self.cells(), &self.grid) else {
            return;
        };
        let top = Self::grid_top(&cells, grid.chosen, self.grid_height());
        let column = (x / cells.cell.0).floor().max(0.0) as usize;
        let row = ((y + top) / cells.cell.1).floor().max(0.0) as usize;
        let index = row * cells.columns + column.min(cells.columns - 1);
        if index < self.layout.pages.len() {
            self.grid = None;
            self.goto_page(index);
        }
    }

    fn grid_height(&self) -> f64 {
        f64::from(self.window.1.saturating_sub(self.status_height()))
    }

    /// How far down the overview the window begins, so that the row of
    /// the page picked out is in it: the rows are a window high together
    /// or more, and that row is kept near the middle.
    fn grid_top(cells: &Cells, chosen: usize, height: f64) -> f64 {
        let row = (chosen / cells.columns) as f64;
        (row * cells.cell.1 - (height - cells.cell.1) / 2.0).max(0.0)
    }

    /// Draws the overview in place of the panes.
    pub(super) fn grid_scene(&mut self, scene: &mut Scene, has: &dyn Fn(TextureId) -> bool) {
        let (Some(cells), Some(grid), Some(document)) =
            (self.cells(), &self.grid, self.document.clone())
        else {
            return;
        };
        let height = self.grid_height();
        let top = Self::grid_top(&cells, grid.chosen, height);
        let gap = THUMB_GAP * self.scale;
        let frame = (3.0 * self.scale).round();
        let chosen = grid.chosen;
        let mut numbers: Vec<(RowText, (f64, f64))> = Vec::new();
        for (index, page) in document.pages().iter().enumerate() {
            let (column, row) = (index % cells.columns, index / cells.columns);
            let y = row as f64 * cells.cell.1 + gap - top;
            if y > height || y + cells.cell.1 < 0.0 {
                continue;
            }
            let (width, tall) = page_size_px(page, cells.zoom);
            let x = (column as f64 * cells.cell.0
                + gap / 2.0
                + (cells.cell.0 - f64::from(width)) / 2.0)
                .round();
            let y = y.round();
            let size = (f64::from(width), f64::from(tall));
            if index == chosen {
                let around = (
                    x - frame,
                    y - frame,
                    size.0 + 2.0 * frame,
                    size.1 + 2.0 * frame,
                );
                solid(scene, has, CHOSEN, around);
            }
            solid(scene, has, PAPER, (x, y, size.0, size.1));
            let whole = PixelRegion {
                x: 0,
                y: 0,
                width,
                height: tall,
            };
            for tile in tiles(page, cells.zoom, whole) {
                if has(texture(tile.key)) {
                    scene.quads.push(Quad {
                        texture: texture(tile.key),
                        x: (x + f64::from(tile.region.x)) as f32,
                        y: (y + f64::from(tile.region.y)) as f32,
                        width: tile.region.width as f32,
                        height: tile.region.height as f32,
                    });
                } else {
                    scene.wanted.push(tile);
                }
            }
            let number = RowText {
                text: (index + 1).to_string(),
                dim: index != chosen,
            };
            numbers.push((number, (x, y + size.1 + 2.0 * self.scale)));
        }
        row_images(&mut self.chrome, scene, has, &numbers, self.scale);
    }
}
