//! What a talk looks like (see [`super::present`]): the audience's view,
//! the presenter's view, and the slides side by side.
//!
//! The audience's view is the slide and nothing else: one black quad
//! behind it and the tiles of its page, at the zoom that makes the page
//! as large as the window lets it be, placed on whole pixels. The
//! presenter's view has the slide on the left, the next one on the
//! right with the notes under it, and a line at the foot with the slide
//! and the step, the time of day, the time the talk has taken and, with
//! a plan, the time left and how far ahead or behind the talk is.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use typst_layout::PagedDocument;
use zetwerk_render::{PixelRegion, page_size_px, tiles};

use super::deck::Deck;
use super::present::{AHEAD, BLACK, lag, minutes, next_tick};
use super::{Quad, ROW_HEIGHT, RowText, Scene, TextureId, Viewer, row_images, solid, texture};

/// The colour behind the presenter's view, of the frame around the
/// slide picked out in the overview, and around the one shown.
const PRESENTER: [u8; 3] = [0x1d, 0x20, 0x21];
const CHOSEN: [u8; 3] = [0xfa, 0xbd, 0x2f];
const CURRENT: [u8; 3] = [0x83, 0xa5, 0x98];
/// How wide a slide is in the overview, about, in logical pixels, and
/// the room between two.
const THUMB_WIDTH: f64 = 300.0;
const THUMB_GAP: f64 = 24.0;
/// The room around the parts of the presenter's view, and the height of
/// its line at the foot, in logical pixels.
const PAD: f64 = 16.0;
const BAR: f64 = 40.0;
/// How much of the presenter's view the slide shown takes across.
const CURRENT_SHARE: f64 = 0.62;
/// The text of the presenter's view.
const FONT: &str = "DejaVu Sans Mono";
const TEXT: &str = "#ebdbb2";
const DIM: &str = "#928374";
const ACCENT: &str = "#fabd2f";
const ALERT: &str = "#fb4934";

/// `text` as a Typst string literal.
fn literal(text: &str) -> String {
    let mut literal = String::with_capacity(text.len() + 2);
    literal.push('"');
    for c in text.chars() {
        match c {
            '"' => literal.push_str("\\\""),
            '\\' => literal.push_str("\\\\"),
            '\n' => literal.push_str("\\n"),
            c => literal.push(c),
        }
    }
    literal.push('"');
    literal
}

/// The widest and the highest page of `document`, in points.
fn largest(document: &PagedDocument) -> (f64, f64) {
    let pages = document.pages();
    let widest = pages
        .iter()
        .map(|page| page.frame.width().to_pt())
        .fold(1.0, f64::max);
    let highest = pages
        .iter()
        .map(|page| page.frame.height().to_pt())
        .fold(1.0, f64::max);
    (widest, highest)
}

/// Where the slides are in the overview: how many side by side, the
/// size of the place each has, and the zoom they are drawn at.
struct Cells {
    columns: usize,
    cell: (f64, f64),
    zoom: f64,
}

impl Viewer {
    /// What the window shows while a talk is given; `None` while none
    /// is.
    pub(super) fn present_scene(&mut self, has: &dyn Fn(TextureId) -> bool) -> Option<Scene> {
        self.present.show.as_ref()?;
        let document = self.document.clone()?;
        self.talk_planned_for(&document);
        let mut scene = Scene::default();
        let size = (f64::from(self.window.0), f64::from(self.window.1));
        let pages = document.pages().len();
        let show = self.present.show.as_mut()?;
        show.page = show.page.min(pages.saturating_sub(1));
        let (blank, overview, presenter, clock_shown) =
            (show.blank, show.overview, show.presenter, show.clock);
        // With the audience's view in a window of its own, this one
        // keeps the presenter's view while that is blank.
        if let Some(colour) = blank.filter(|_| !show.apart) {
            show.tick = None;
            solid(&mut scene, has, colour, (0.0, 0.0, size.0, size.1));
            return Some(scene);
        }
        let deck = self.deck()?;
        if let Some(chosen) = overview {
            if let Some(show) = &mut self.present.show {
                show.tick = None;
            }
            self.talk_overview(&mut scene, has, &document, &deck, chosen);
        } else if presenter {
            self.presenter_view(&mut scene, has, &document, &deck, size);
        } else {
            self.audience_view(&mut scene, has, size);
            if clock_shown {
                self.audience_clock(&mut scene, has, size);
            }
        }
        let ticking = overview.is_none() && (presenter || clock_shown);
        if let Some(show) = &mut self.present.show {
            show.tick = ticking.then(|| next_tick(Instant::now(), show.began, SystemTime::now()));
        }
        Some(scene)
    }

    /// The audience's view in a window of its own of `size` device
    /// pixels, while the talk has one; `None` while no talk is given.
    pub fn audience_scene(
        &mut self,
        size: (u32, u32),
        has: &dyn Fn(TextureId) -> bool,
    ) -> Option<Scene> {
        let document = self.document.clone()?;
        self.talk_planned_for(&document);
        let show = self.present.show.as_ref()?;
        let (blank, clock_shown) = (show.blank, show.clock);
        let mut scene = Scene::default();
        let size = (f64::from(size.0), f64::from(size.1));
        if let Some(colour) = blank {
            solid(&mut scene, has, colour, (0.0, 0.0, size.0, size.1));
            return Some(scene);
        }
        self.audience_view(&mut scene, has, size);
        if clock_shown {
            self.audience_clock(&mut scene, has, size);
        }
        Some(scene)
    }

    /// The quads of page `index` fitted into `area` (left, top, width,
    /// height, in device pixels) on whole pixels, as far as its tiles
    /// are held; the others are asked for. Also whether all are held.
    fn slide_in(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        index: usize,
        (left, top, width, height): (f64, f64, f64, f64),
    ) -> (Vec<Quad>, bool) {
        let Some(document) = self.document.clone() else {
            return (Vec::new(), false);
        };
        let (Some(page), Some(show)) = (document.pages().get(index), &mut self.present.show) else {
            return (Vec::new(), false);
        };
        let (page_width, page_height) = (page.frame.width().to_pt(), page.frame.height().to_pt());
        if page_width <= 0.0 || page_height <= 0.0 || width < 1.0 || height < 1.0 {
            return (Vec::new(), false);
        }
        let zoom = (width / page_width).min(height / page_height);
        let (pixels_across, pixels_down) = page_size_px(page, zoom);
        let x = (left + (width - f64::from(pixels_across)) / 2.0).round();
        let y = (top + (height - f64::from(pixels_down)) / 2.0).round();
        let planned = show
            .planned
            .entry((index, zoom.to_bits()))
            .or_insert_with(|| {
                let whole = PixelRegion {
                    x: 0,
                    y: 0,
                    width: pixels_across,
                    height: pixels_down,
                };
                tiles(page, zoom, whole)
            });
        let mut quads = Vec::with_capacity(planned.len());
        let mut complete = true;
        for tile in planned.iter() {
            if has(texture(tile.key)) {
                quads.push(Quad {
                    texture: texture(tile.key),
                    x: (x + f64::from(tile.region.x)) as f32,
                    y: (y + f64::from(tile.region.y)) as f32,
                    width: tile.region.width as f32,
                    height: tile.region.height as f32,
                });
            } else {
                complete = false;
                scene.wanted.push(tile.clone());
            }
        }
        (quads, complete)
    }

    /// Asks for the pages behind `page`, fitted into `area` as it is.
    fn want_ahead(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        page: usize,
        area: (f64, f64, f64, f64),
    ) {
        for ahead in page + 1..=page + AHEAD {
            self.slide_in(scene, has, ahead, area);
        }
    }

    /// The slide, as large as the window lets it be, on black. Until all
    /// of a new slide is there, the last one stays.
    fn audience_view(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        size: (f64, f64),
    ) {
        let Some(page) = self.presented_page() else {
            return;
        };
        solid(scene, has, BLACK, (0.0, 0.0, size.0, size.1));
        let area = (0.0, 0.0, size.0, size.1);
        let (quads, complete) = self.slide_in(scene, has, page, area);
        let Some(show) = &mut self.present.show else {
            return;
        };
        let held = |quads: &[Quad]| quads.iter().all(|quad| has(quad.texture));
        if complete {
            scene.quads.extend_from_slice(&quads);
            show.drawn = Some((page, quads));
        } else if let Some((_, before)) = show.drawn.as_ref().filter(|(_, before)| held(before)) {
            scene.quads.extend_from_slice(before);
        } else {
            scene.quads.extend(quads);
        }
        self.want_ahead(scene, has, page, area);
    }

    /// Draws chrome `markup` with its top left corner where `place` puts
    /// it, given its size in pixels; returns its size.
    fn talk_panel(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        markup: &str,
        place: impl FnOnce((f64, f64)) -> (f64, f64),
    ) -> (f64, f64) {
        let rendered = self.chrome.render(markup, self.scale);
        // The markup is made here: one that does not typeset is a bug.
        debug_assert!(rendered.is_some(), "markup that does not typeset: {markup}");
        let Some(image) = rendered else {
            return (0.0, 0.0);
        };
        let size = (
            f64::from(image.pixmap.width()),
            f64::from(image.pixmap.height()),
        );
        let (x, y) = place(size);
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
        size
    }

    /// The time of day and the time the talk has taken, in the corner of
    /// the audience's view.
    fn audience_clock(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        size: (f64, f64),
    ) {
        let Some(show) = &self.present.show else {
            return;
        };
        let said = format!("{}  ·  {}", time_of_day(), minutes(show.began.elapsed()));
        let markup = format!(
            "#set page(width: auto, height: auto, margin: (x: 7pt, y: 4pt), fill: rgb(\"#000000a0\"))\n\
             #set text(font: \"{FONT}\", size: 14pt, fill: rgb(\"{TEXT}\"))\n#{}",
            literal(&said)
        );
        let margin = 10.0 * self.scale;
        self.talk_panel(scene, has, &markup, |(width, height)| {
            (size.0 - width - margin, size.1 - height - margin)
        });
    }

    /// The presenter's view: the slide, the next, the notes, the time.
    fn presenter_view(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        document: &Arc<PagedDocument>,
        deck: &Deck,
        (width, height): (f64, f64),
    ) {
        let Some(show) = &self.present.show else {
            return;
        };
        let page = show.page;
        let pages = document.pages().len();
        let s = self.scale;
        let (pad, bar) = (PAD * s, BAR * s);
        solid(scene, has, PRESENTER, (0.0, 0.0, width, height));
        let split = (width * CURRENT_SHARE).round();
        let current = (pad, pad, split - 1.5 * pad, height - bar - 2.0 * pad);
        let (quads, _) = self.slide_in(scene, has, page, current);
        scene.quads.extend(quads);

        // The next page, and what it is.
        let right = split + pad / 2.0;
        let column = (width - right - pad).max(1.0);
        let slide = deck.slide_of(page);
        let steps = deck.slides.get(slide).cloned().unwrap_or(page..page + 1);
        let next_said = if page + 1 >= pages {
            "the end of the talk".to_owned()
        } else if steps.contains(&(page + 1)) {
            format!(
                "next: step {} of {}",
                page + 1 - steps.start + 1,
                steps.len()
            )
        } else {
            let label = deck.labels.get(slide + 1).map_or("", String::as_str);
            format!("next: slide {label}")
        };
        let label = format!(
            "#set page(width: auto, height: auto, margin: (x: 0pt, y: 2pt), fill: none)\n\
             #set text(font: \"{FONT}\", size: 12pt, fill: rgb(\"{DIM}\"))\n#{}",
            literal(&next_said)
        );
        let (_, label_height) = self.talk_panel(scene, has, &label, |_| (right, pad));
        let (widest, highest) = largest(document);
        let next_height = (column * highest / widest).min((height - bar) * 0.45);
        let next_top = pad + label_height + 6.0 * s;
        let next_area = (right, next_top, column, next_height);
        if page + 1 < pages {
            let (quads, _) = self.slide_in(scene, has, page + 1, next_area);
            scene.quads.extend(quads);
        }

        // The notes of the page shown.
        let notes_top = next_top + next_height + pad;
        let notes_height = (height - bar - pad - notes_top).max(1.0);
        let notes = deck.notes_of(page);
        let mut markup = format!(
            "#set page(width: {:.1}pt, height: {:.1}pt, margin: 0pt, fill: none)\n\
             #set text(font: \"{FONT}\", size: 15pt, fill: rgb(\"{TEXT}\"), hyphenate: false)\n\
             #set par(spacing: 0.9em)\n",
            column / s,
            notes_height / s
        );
        if notes.is_empty() {
            let _ = writeln!(
                markup,
                "#text(fill: rgb(\"{DIM}\"), \"no notes for this slide\")"
            );
        }
        for note in notes {
            let _ = writeln!(markup, "#{}\n", literal(note));
        }
        self.talk_panel(scene, has, &markup, |_| (right, notes_top));

        // The line at the foot.
        let foot = self.presenter_foot(deck, pages);
        let markup = format!(
            "#set page(width: {:.1}pt, height: {:.1}pt, margin: (x: {PAD}pt), fill: none)\n\
             #set text(font: \"{FONT}\", size: 14pt, fill: rgb(\"{TEXT}\"))\n\
             #set align(horizon)\n{foot}",
            width / s,
            bar / s
        );
        self.talk_panel(scene, has, &markup, |_| (0.0, height - bar));
        // The page shown next, in the place it will have.
        self.want_ahead(scene, has, page, current);
    }

    /// What the line at the foot of the presenter's view says, as markup.
    pub(super) fn presenter_foot(&self, deck: &Deck, pages: usize) -> String {
        let Some(show) = &self.present.show else {
            return String::new();
        };
        let page = show.page;
        let slide = deck.slide_of(page);
        let count = deck.slides.len();
        let label = deck.labels.get(slide).map_or("", String::as_str);
        let mut left = format!("slide {label}  ({} of {count})", slide + 1);
        if let Some(steps) = deck.slides.get(slide).filter(|steps| steps.len() > 1) {
            let _ = write!(left, "  step {} of {}", page - steps.start + 1, steps.len());
        }
        if deck.slides.len() != pages {
            let _ = write!(left, "  page {} of {pages}", page + 1);
        }
        if !show.number.is_empty() {
            let _ = write!(left, "  → {}", show.number);
        }
        let elapsed = show.began.elapsed();
        let mut right = format!(
            "text(fill: rgb(\"{ACCENT}\"), {}) + {}",
            literal(&time_of_day()),
            literal(&format!("  {} gone", minutes(elapsed)))
        );
        if let Some(duration) = show.plan.duration {
            let left_over = duration.saturating_sub(elapsed);
            let _ = write!(
                right,
                " + {}",
                literal(&format!("  {} left", minutes(left_over)))
            );
            if let Some(due) = show.plan.window(slide, count) {
                match lag(due, elapsed).filter(|(_, by)| by.as_secs() >= 60) {
                    Some((true, by)) => {
                        let _ = write!(
                            right,
                            " + text(fill: rgb(\"{ALERT}\"), {})",
                            literal(&format!("  {} behind", minutes(by)))
                        );
                    }
                    Some((false, by)) => {
                        let _ =
                            write!(right, " + {}", literal(&format!("  {} ahead", minutes(by))));
                    }
                    None => {
                        let _ = write!(right, " + {}", literal("  on time"));
                    }
                }
            }
        }
        format!("#grid(columns: (1fr, auto), {}, {right})", literal(&left))
    }

    /// Where the slides are in the overview of a talk.
    fn overview_cells(&self) -> Option<Cells> {
        let document = self.document.as_ref()?;
        let (widest, highest) = largest(document);
        let gap = THUMB_GAP * self.scale;
        let room = f64::from(self.window.0) - gap;
        let columns = ((room / (THUMB_WIDTH * self.scale + gap)).floor() as usize).clamp(1, 8);
        let across = room / columns as f64;
        let zoom = (across - gap) / widest;
        Some(Cells {
            columns,
            cell: (across, highest * zoom + gap + ROW_HEIGHT * self.scale),
            zoom,
        })
    }

    /// How many slides are side by side in the overview of a talk.
    pub(super) fn overview_columns(&self) -> usize {
        self.overview_cells().map_or(1, |cells| cells.columns)
    }

    /// How far down the overview the window begins, so that the row of
    /// the slide picked out is near its middle.
    fn overview_top(&self, cells: &Cells, chosen: usize) -> f64 {
        let row = (chosen / cells.columns) as f64;
        let height = f64::from(self.window.1);
        (row * cells.cell.1 - (height - cells.cell.1) / 2.0).max(0.0)
    }

    /// The slide of the overview at `at` in the window, if one is there.
    pub(super) fn overview_at(&mut self, (x, y): (f64, f64)) -> Option<usize> {
        let cells = self.overview_cells()?;
        let chosen = self.present.show.as_ref()?.overview?;
        let top = self.overview_top(&cells, chosen);
        let column = ((x / cells.cell.0).floor().max(0.0) as usize).min(cells.columns - 1);
        let row = ((y + top) / cells.cell.1).floor().max(0.0) as usize;
        let slide = row * cells.columns + column;
        let count = self.deck()?.slides.len();
        (slide < count).then_some(slide)
    }

    /// The slides side by side on black, each as its last step shows it,
    /// the one picked out and the one shown framed, numbered as the deck
    /// numbers them.
    fn talk_overview(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        document: &Arc<PagedDocument>,
        deck: &Deck,
        chosen: usize,
    ) {
        let (width, height) = (f64::from(self.window.0), f64::from(self.window.1));
        solid(scene, has, BLACK, (0.0, 0.0, width, height));
        let Some(cells) = self.overview_cells() else {
            return;
        };
        let shown = self
            .present
            .show
            .as_ref()
            .map_or(0, |show| deck.slide_of(show.page));
        let top = self.overview_top(&cells, chosen);
        let gap = THUMB_GAP * self.scale;
        let frame = (3.0 * self.scale).round();
        let mut numbers: Vec<(RowText, (f64, f64))> = Vec::new();
        for (slide, pages) in deck.slides.iter().enumerate() {
            let (column, row) = (slide % cells.columns, slide / cells.columns);
            let y = row as f64 * cells.cell.1 + gap - top;
            if y > height || y + cells.cell.1 < 0.0 {
                continue;
            }
            let last = pages.end.saturating_sub(1);
            let Some(page) = document.pages().get(last) else {
                continue;
            };
            let (across, down) = page_size_px(page, cells.zoom);
            let (across, down) = (f64::from(across), f64::from(down));
            let x =
                (column as f64 * cells.cell.0 + gap / 2.0 + (cells.cell.0 - gap - across) / 2.0)
                    .round();
            let y = y.round();
            for (marked, colour) in [(slide == shown, CURRENT), (slide == chosen, CHOSEN)] {
                if marked {
                    let around = (
                        x - frame,
                        y - frame,
                        across + 2.0 * frame,
                        down + 2.0 * frame,
                    );
                    solid(scene, has, colour, around);
                }
            }
            let (quads, _) = self.slide_in(scene, has, last, (x, y, across, down));
            scene.quads.extend(quads);
            let number = RowText {
                text: deck.labels.get(slide).cloned().unwrap_or_default(),
                dim: slide != chosen,
            };
            numbers.push((number, (x, y + down + 4.0 * self.scale)));
        }
        row_images(&mut self.chrome, scene, has, &numbers, self.scale);
    }
}

/// The time of day as the clock shows it.
fn time_of_day() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}
