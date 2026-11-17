//! What is drawn over the pages to work on them by, and is no part of
//! them: nothing of it is in the document, in what is exported or in
//! what an agent is shown.
//!
//! The lines of a page have no numbers one could name them by, and
//! `5j` wants a five. So each line near the cursor says, faintly and in
//! the margin, how many lines away it is: the number to type before `j`
//! or `k` to get there. They are counted as those keys count, by going
//! from the cursor line by line, so what a number says is where the key
//! leads, across columns, tables and pages.

use zetwerk_typst::PageMap;

use super::{PageRows, Place, Quad, Scene, TextureId, Viewer, chrome};

/// How many lines up and down from the cursor are numbered.
const REACH: usize = 99;
/// How far left of its line a number stands, and how large it is, in
/// points of the page.
const GAP: f64 = 7.0;
const SIZE: f64 = 6.5;

/// The lines near the cursor: on which page, which line, how far away.
pub(super) type Numbered = Vec<(usize, usize, usize)>;

/// Those lines, and what they were counted for: the revision of the
/// text, the cursor, and the revision of the pages.
pub(super) type NumberedFor = ((u64, usize, Option<u64>), Numbered);

impl Viewer {
    /// How many lines each line near the cursor is away from it, as `j`
    /// and `k` count. `None` where the pages do not show where the
    /// cursor is.
    fn lines_away(&self) -> Option<Numbered> {
        let editor = self.editor.as_ref()?;
        let rows = PageRows {
            document: self.document.as_ref()?,
            world: self.compiled_world.as_ref()?,
            file: editor.file(),
            lines: &self.page_lines,
            cursor_page: &self.cursor_page,
            change: self.places()?,
            landed: &self.landed,
            elsewhere: &self.elsewhere,
        };
        let from = rows.change.back_or_edge(editor.cursor());
        let page = rows.page_of(from)?;
        let (line, across) = rows.on(page, |lines| lines.find(editor.file(), from))??;
        let mut numbered = vec![(page, line, 0)];
        for down in [true, false] {
            let mut place = Place { page, line, across };
            for away in 1..=REACH.min(self.settings.reach) {
                if !rows.step(&mut place, down) {
                    break;
                }
                numbered.push((place.page, place.line, away));
            }
        }
        Some(numbered)
    }

    /// Draws beside the lines of page `index` (whose top-left corner is
    /// at `origin`) how far each is from the cursor.
    pub(super) fn line_numbers(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        index: usize,
        origin: (f64, f64),
    ) {
        if !self.numbers || self.editor.is_none() || self.numbers_resting() {
            return;
        }
        // Counted once for all pages of a frame: they are the same
        // until the cursor or the pages change.
        let key = (
            self.revision,
            self.editor
                .as_ref()
                .map_or(0, crate::editor::Editor::cursor),
            self.document_revision,
        );
        // Counted anew, too, when the pages were made again of the same
        // text: where their lines are is forgotten then, and is found
        // again by counting.
        let forgotten = self.page_lines.borrow().is_empty();
        if forgotten || self.numbered.as_ref().is_none_or(|(of, _)| *of != key) {
            let typing = self
                .editor
                .as_ref()
                .is_some_and(|editor| editor.mode() == crate::editor::Mode::Insert);
            // The pages of this text are yet to come.
            let awaited = self.compiled_revision != Some(self.revision);
            match (self.lines_away(), &mut self.numbered) {
                (Some(numbered), _) => self.numbered = Some((key, numbered)),
                // After a key the pages are behind the text for a moment,
                // and the lines cannot be counted. They are where they
                // were, nearly always: the numbers stay, where otherwise
                // they would go and come with every key.
                (None, Some((of, _))) if typing || awaited => *of = key,
                (None, _) => self.numbered = Some((key, Vec::new())),
            }
        }
        let drawn = {
            let Some((_, numbered)) = &self.numbered else {
                return;
            };
            let (Some(document), Some(world)) = (&self.document, &self.compiled_world) else {
                return;
            };
            let page = document.pages().get(index);
            let Some(page) = page.filter(|_| numbered.iter().any(|(on, ..)| *on == index)) else {
                return;
            };
            // The lines of pages made while what was typed since is
            // worked into them (a compile of an older text) are not
            // counted: the numbers kept stand beside them.
            let mut all = self.page_lines.borrow_mut();
            let lines = all
                .entry(index)
                .or_insert_with(|| PageMap::build(world, &page.frame).lines());
            // The numbers stand in a column of their own: left of where
            // the lines of the page (or of its column) begin.
            let edges: Vec<f64> = (0..lines.len())
                .filter_map(|line| lines.extent(line).map(|(left, _)| left))
                .collect();
            let Some(page_edge) = edges.iter().copied().reduce(f64::min) else {
                return;
            };
            let widest = edges.iter().copied().fold(page_edge, f64::max);
            let middle = f64::midpoint(page_edge, widest);
            let column_edge = edges
                .iter()
                .copied()
                .filter(|edge| *edge > middle && widest - page_edge > 150.0)
                .reduce(f64::min);
            let mut drawn = Vec::new();
            for (page, line, away) in numbered {
                if *page != index || *away == 0 {
                    continue;
                }
                let (Some((left, _)), Some((top, bottom))) =
                    (lines.extent(*line), lines.band(*line))
                else {
                    continue;
                };
                let edge = match column_edge {
                    Some(column) if left >= column => column,
                    _ => page_edge,
                };
                drawn.push((*away, edge, f64::midpoint(top, bottom)));
            }
            drawn
        };
        let zoom = self.view.zoom;
        for (away, edge, middle) in drawn {
            let markup = chrome::margin_number_markup(away, SIZE);
            // Not typeset again while the compositor holds it: there are
            // more of them than chrome keeps.
            let Some((id, (width, height), image)) =
                self.chrome.render_unless_held(&markup, zoom, has)
            else {
                continue;
            };
            let (width, height) = (f64::from(width), f64::from(height));
            scene.quads.push(Quad {
                texture: id,
                x: (origin.0 + (edge - GAP) * zoom - width).round() as f32,
                y: (origin.1 + middle * zoom - height / 2.0).round() as f32,
                width: width as f32,
                height: height as f32,
            });
            if let Some(image) = image.filter(|_| !has(id)) {
                scene.images.push(image);
            }
        }
    }
}
