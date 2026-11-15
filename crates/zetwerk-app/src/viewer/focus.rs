//! Where typing goes, and what an empty place is for: drawn over the
//! page, faintly, and no part of it. Nothing of it is in the document,
//! in what is exported or in what an agent is shown.
//!
//! On the page the source is never seen, so it is not seen either where
//! the text typed goes: into the equation or behind it, into the cell of
//! a table or the next one, into the caption or the text under the
//! figure. While typing, a thin frame stands around what the cursor is
//! in, where that is not running text: an equation, a cell, a caption,
//! the text of a footnote, what a call holds, an item of a list (typing
//! at its end goes on it, not under the list). A table drawn without
//! rules shows the edges of all its cells while the cursor is in it, so
//! that one sees which cell is which. The frame is where the pages on
//! screen have the thing: until they are typeset anew, of the text
//! before the last keys.
//!
//! In normal mode nothing is framed: nothing is typed, and the block
//! cursor says what the keys act on.
//!
//! A place of an equation that is still empty shows as a box. Beside it
//! stands, very small and fainter than the box, what goes there: the
//! numerator, the exponent, the radicand. So beside the cursor in an
//! empty caption, footnote or heading. These say what the place is for
//! only while the page shows the text as it is: a key that fills a place
//! takes its word away at once, and the page the compile then makes
//! shows the places that are still empty.
//!
//! What is drawn is found once for a text, a cursor and the pages, and
//! for each page only when that page is drawn; a frame only draws it.

use std::collections::HashMap;
use std::ops::Range;

use typst::layout::{Abs, Point, Rect};
use zetwerk_typst::{Origin, PageMap};

use super::{Quad, Scene, TextureId, Viewer, tint};
use crate::editor;
use crate::typst_text::{self, Around, EMPTY, Enclosure};

/// The frame around what typing goes into, and the edges of the cells
/// of a table without rules. Colours are RGBA.
const FRAME: [u8; 4] = [0x45, 0x85, 0x88, 0x80];
const EDGES: [u8; 4] = [0x92, 0x83, 0x74, 0x60];
/// How far the frame stands off what it frames, and how small it gets
/// at least, in points.
const MARGIN: f64 = 2.0;
const LEAST: f64 = 4.0;
/// How large the word for an empty place is, in points of the page; and
/// how far it stands off its box.
const HINT_SIZE: f64 = 4.5;
const HINT_GAP: f64 = 0.5;
/// The places whose word stands above their box: those above the rest
/// of what they are part of. All others have it below theirs.
const ABOVE: &[&str] = &["numerator", "exponent", "upper", "upper limit", "index"];

/// What the aids of this module keep between frames.
pub(super) struct Focus {
    /// Whether they are drawn (`:hints`).
    on: bool,
    /// What was found, and for which text, cursor and pages.
    found: Option<(Key, Found)>,
}

impl Default for Focus {
    fn default() -> Self {
        Self {
            on: true,
            found: None,
        }
    }
}

/// What what was found holds for: the revision of the text, the
/// cursor, whether it is typed at, the revision of the pages, and how
/// many times they were typeset whole and in parts.
type Key = (u64, usize, bool, Option<u64>, u64, u64);

/// What the cursor is in, and what is drawn of it on each page drawn.
#[derive(Default)]
struct Found {
    /// What typing goes into, if it is not running text; only while
    /// typing.
    around: Option<Around>,
    /// The table without rules the cursor is in, while typing: its cells
    /// and how many columns it has.
    table: Option<(Vec<Range<usize>>, usize)>,
    /// What the empty place the cursor is at, in running text, is for.
    spot: Option<&'static str>,
    /// Whether the pages show the text as it is.
    current: bool,
    pages: HashMap<usize, Marks>,
}

/// What is drawn on one page, in points from its top-left corner.
#[derive(Default)]
struct Marks {
    /// The frame around what typing goes into.
    frame: Option<Rect>,
    /// The edges of the cells of a table without rules.
    edges: Vec<(Point, Point)>,
    /// The boxes of empty places, and what each is for.
    hints: Vec<(Rect, Side, &'static str)>,
}

/// Where the word for an empty place stands of what it is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Above,
    Below,
    /// Behind the cursor, on its line.
    Behind,
}

/// The markup for the word that says what an empty place is for: small,
/// grey and half transparent, in letters that are not the page's, so
/// that it is never taken for text.
fn hint_markup(word: &str) -> String {
    format!(
        "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n\
         #set text(font: \"DejaVu Sans Mono\", size: {HINT_SIZE}pt, \
         fill: rgb(\"#83a598\").transparentize(45%), top-edge: \"ascender\", \
         bottom-edge: \"descender\")\n\
         {word}"
    )
}

/// The bytes of the file a glyph shows, if it is of `file`.
fn shown_of(origin: &Origin, file: typst::syntax::FileId) -> Option<&Range<usize>> {
    match origin {
        Origin::Text { file: of, range }
        | Origin::Space { file: of, range }
        | Origin::Math { file: of, range }
        | Origin::Object {
            file: of, range, ..
        } if *of == file => Some(range),
        _ => None,
    }
}

/// The box around `a` and `b`.
fn union(a: Rect, b: Rect) -> Rect {
    Rect::new(a.min.min(b.min), a.max.max(b.max))
}

/// `rect` grown by `by` points on every side, and to `LEAST` at least.
fn grown(rect: Rect, by: f64) -> Rect {
    let by = Abs::pt(by);
    let mut rect = Rect::new(
        Point::new(rect.min.x - by, rect.min.y - by),
        Point::new(rect.max.x + by, rect.max.y + by),
    );
    let least = Abs::pt(LEAST);
    for (min, max) in [
        (&mut rect.min.x, &mut rect.max.x),
        (&mut rect.min.y, &mut rect.max.y),
    ] {
        let lack = least - (*max - *min);
        if lack > Abs::zero() {
            *min -= lack / 2.0;
            *max += lack / 2.0;
        }
    }
    rect
}

/// The box around all that page `map` shows of bytes `range` of `file`:
/// the glyphs whose bytes are all in it. With `whole`, not those that
/// stand for all of it (the number of an equation).
fn bounds(
    map: &PageMap,
    file: typst::syntax::FileId,
    range: &Range<usize>,
    whole: bool,
) -> Option<Rect> {
    map.glyphs()
        .iter()
        .filter(|glyph| !glyph.furniture)
        .filter(|glyph| {
            shown_of(&glyph.origin, file).is_some_and(|shown| {
                range.start <= shown.start && shown.end <= range.end && !(whole && shown == range)
            })
        })
        .map(|glyph| glyph.rect)
        .reduce(union)
}

/// Where the edges between `extents` (of columns or of rows, by their
/// number, `None` where nothing of one is on the page) are: between two
/// halfway, and as far out at the ends as the narrowest gap between two
/// is half wide. With the numbers of the columns or rows that are there,
/// in order.
fn edges_between(extents: &[Option<(f64, f64)>]) -> (Vec<usize>, Vec<f64>) {
    let known: Vec<(usize, f64, f64)> = extents
        .iter()
        .enumerate()
        .filter_map(|(index, extent)| extent.map(|(from, to)| (index, from, to)))
        .collect();
    let gaps = known.windows(2).map(|pair| pair[1].1 - pair[0].2);
    let pad = gaps.fold(None, |least: Option<f64>, gap| {
        Some(least.map_or(gap, |least| least.min(gap)))
    });
    let pad = pad.map_or(3.0, |gap| (gap / 2.0).clamp(1.5, 5.0));
    let mut edges = Vec::with_capacity(known.len() + 1);
    if let (Some(first), Some(last)) = (known.first(), known.last()) {
        edges.push(first.1 - pad);
        edges.extend(
            known
                .windows(2)
                .map(|pair| f64::midpoint(pair[0].2, pair[1].1)),
        );
        edges.push(last.2 + pad);
    }
    (known.into_iter().map(|(index, ..)| index).collect(), edges)
}

/// The edges of the cells of a table on page `map`: of `cells` (bytes
/// of the text the page was made of) in `columns` columns. And the
/// rectangle of cell `cursor`, if it is on the page.
fn table_edges(
    map: &PageMap,
    file: typst::syntax::FileId,
    cells: &[Range<usize>],
    columns: usize,
    cursor: Option<usize>,
) -> (Vec<(Point, Point)>, Option<Rect>) {
    let columns = columns.max(1);
    // What each cell shows on the page: one walk over its glyphs.
    let mut shown: Vec<Option<Rect>> = vec![None; cells.len()];
    for glyph in map.glyphs() {
        let Some(range) = shown_of(&glyph.origin, file) else {
            continue;
        };
        let after = cells.partition_point(|cell| cell.start <= range.start);
        let Some(index) = after.checked_sub(1) else {
            continue;
        };
        if cells.get(index).is_some_and(|cell| range.end <= cell.end) {
            shown[index] = Some(shown[index].map_or(glyph.rect, |had| union(had, glyph.rect)));
        }
    }
    let rows = cells.len().div_ceil(columns);
    let mut across: Vec<Option<(f64, f64)>> = vec![None; columns];
    let mut down: Vec<Option<(f64, f64)>> = vec![None; rows];
    let widen = |extent: &mut Option<(f64, f64)>, from: Abs, to: Abs| {
        let (from, to) = (from.to_pt(), to.to_pt());
        *extent = Some(extent.map_or((from, to), |(a, b)| (a.min(from), b.max(to))));
    };
    for (index, rect) in shown.iter().enumerate() {
        if let Some(rect) = rect {
            widen(&mut across[index % columns], rect.min.x, rect.max.x);
            widen(&mut down[index / columns], rect.min.y, rect.max.y);
        }
    }
    let (columns_there, xs) = edges_between(&across);
    let (rows_there, ys) = edges_between(&down);
    let (Some(left), Some(right), Some(top), Some(bottom)) =
        (xs.first(), xs.last(), ys.first(), ys.last())
    else {
        return (Vec::new(), None);
    };
    let point = |x: f64, y: f64| Point::new(Abs::pt(x), Abs::pt(y));
    let mut edges: Vec<(Point, Point)> = xs
        .iter()
        .map(|x| (point(*x, *top), point(*x, *bottom)))
        .collect();
    edges.extend(ys.iter().map(|y| (point(*left, *y), point(*right, *y))));
    let cell = cursor.and_then(|cell| {
        let column = columns_there.iter().position(|at| *at == cell % columns)?;
        let row = rows_there.iter().position(|at| *at == cell / columns)?;
        Some(Rect::new(
            point(xs[column], ys[row]),
            point(*xs.get(column + 1)?, *ys.get(row + 1)?),
        ))
    });
    (edges, cell)
}

impl Viewer {
    /// What what is found now is for (see [`Key`]).
    fn focus_key(&self) -> Key {
        let (cursor, typing) = self.editor.as_ref().map_or((0, false), |editor| {
            (editor.cursor(), editor.mode() == editor::Mode::Insert)
        });
        (
            self.revision,
            cursor,
            typing,
            self.document_revision,
            self.stats.compiles,
            self.stats.local,
        )
    }

    /// What the cursor is in, by the text alone.
    fn find_focus(&self) -> Found {
        let Some(editor) = &self.editor else {
            return Found::default();
        };
        let (source, cursor) = (editor.source(), editor.cursor());
        let typing = editor.mode() == editor::Mode::Insert;
        let current = self.document_revision == Some(self.revision);
        let around = typing
            .then(|| typst_text::enclosing(source, cursor))
            .flatten();
        let table = typing
            .then(|| typst_text::table_at(source, cursor))
            .flatten()
            .filter(|table| !typst_text::has_rules(source, &table.whole))
            .map(|table| (table.cells, table.columns));
        let spot = (typing && current)
            .then(|| typst_text::spot_name(source, cursor))
            .flatten();
        Found {
            around,
            table,
            spot,
            current,
            pages: HashMap::new(),
        }
    }

    /// What is drawn on page `index` for what was found.
    fn page_focus(&mut self, found: &Found, index: usize) -> Marks {
        let mut marks = Marks::default();
        // Only where boxes were put in to show empty places is there a
        // box to look for; with nothing to look for, no page is mapped.
        let boxes = found.current && self.shown_put.iter().any(|(_, put)| *put == EMPTY);
        if found.around.is_none() && found.table.is_none() && found.spot.is_none() && !boxes {
            return marks;
        }
        let Some(change) = self.places_of_text() else {
            return marks;
        };
        let (Some(editor), Some(document), Some(world)) =
            (&self.editor, &self.document, &self.compiled_world)
        else {
            return marks;
        };
        let Some(page) = document.pages().get(index) else {
            return marks;
        };
        let map = self
            .maps
            .entry(index)
            .or_insert_with(|| PageMap::build(world, &page.frame));
        let (file, source, cursor) = (editor.file(), editor.source(), editor.cursor());
        let back = |range: &Range<usize>| -> Range<usize> {
            change.back_or_edge(range.start)..change.back_or_edge(range.end)
        };
        if let Some(around) = &found.around {
            let whole = around.kind == Enclosure::Equation;
            marks.frame =
                bounds(map, file, &back(&around.range), whole).map(|rect| grown(rect, MARGIN));
        }
        if let Some((cells, columns)) = &found.table {
            let at = cells
                .iter()
                .position(|cell| cell.start < cursor && cursor < cell.end)
                .filter(|_| {
                    found
                        .around
                        .as_ref()
                        .is_some_and(|around| around.kind == Enclosure::Cell)
                });
            let cells: Vec<Range<usize>> = cells.iter().map(back).collect();
            let (edges, cell) = table_edges(map, file, &cells, *columns, at);
            marks.edges = edges;
            if let Some(cell) = cell {
                marks.frame = Some(grown(cell, -1.0));
            }
        }
        if found.current {
            if boxes {
                marks.hints = slot_hints(map, file, source);
            }
            if let (Some(word), Some(caret)) = (found.spot, map.caret(file, cursor)) {
                let x = if caret.after {
                    caret.cell.max.x
                } else {
                    caret.cell.min.x
                };
                let at = Rect::new(
                    Point::new(x, caret.cell.min.y),
                    Point::new(x, caret.cell.max.y),
                );
                marks.hints.push((at, Side::Behind, word));
            }
        }
        marks
    }

    /// Draws on page `index` (whose top-left corner is at `origin`) the
    /// frame around what typing goes into, the edges of the cells of a
    /// table without rules, and what the empty places are for.
    pub(super) fn focus_marks(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        index: usize,
        origin: (f64, f64),
    ) {
        if !self.focus.on || self.editor.is_none() {
            return;
        }
        let key = self.focus_key();
        let mut found = match self.focus.found.take() {
            Some((of, found)) if of == key => found,
            _ => self.find_focus(),
        };
        if !found.pages.contains_key(&index) {
            let marks = self.page_focus(&found, index);
            found.pages.insert(index, marks);
        }
        if let Some(marks) = found.pages.get(&index) {
            self.draw_focus(scene, has, origin, marks);
        }
        self.focus.found = Some((key, found));
    }

    /// Draws `marks` on the page whose top-left corner is at `origin`.
    fn draw_focus(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        origin: (f64, f64),
        marks: &Marks,
    ) {
        let zoom = self.view.zoom;
        let thin = self.scale.round().max(1.0);
        let at = |point: Point| {
            (
                origin.0 + point.x.to_pt() * zoom,
                origin.1 + point.y.to_pt() * zoom,
            )
        };
        if let Some(rect) = marks.frame {
            let ((left, top), (right, bottom)) = (at(rect.min), at(rect.max));
            let (x, y, width, height) = (left.round(), top.round(), right - left, bottom - top);
            tint(scene, has, FRAME, (x, y, width, thin));
            tint(scene, has, FRAME, (x, y + height, width + thin, thin));
            tint(scene, has, FRAME, (x, y, thin, height));
            tint(scene, has, FRAME, (x + width, y, thin, height));
        }
        for (from, to) in &marks.edges {
            let ((x0, y0), (x1, y1)) = (at(*from), at(*to));
            let (x0, y0) = (x0.round(), y0.round());
            tint(
                scene,
                has,
                EDGES,
                (x0, y0, (x1 - x0).max(thin), (y1 - y0).max(thin)),
            );
        }
        for (rect, side, word) in &marks.hints {
            let Some(image) = self.chrome.render(&hint_markup(word), zoom) else {
                continue;
            };
            let (width, height) = (
                f64::from(image.pixmap.width()),
                f64::from(image.pixmap.height()),
            );
            let ((left, top), (right, bottom)) = (at(rect.min), at(rect.max));
            let gap = HINT_GAP * zoom;
            let middle = f64::midpoint(left, right) - width / 2.0;
            let (x, y) = match side {
                Side::Above => (middle, top - height - gap),
                Side::Below => (middle, bottom + gap),
                Side::Behind => (right + 2.0 * gap, f64::midpoint(top, bottom) - height / 2.0),
            };
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

    /// `:hints`, `:hints on`, `:hints off`: whether what typing goes
    /// into is framed and empty places say what they are for.
    pub(super) fn hints_command(&mut self, wanted: &str) {
        self.focus.on = match wanted {
            "on" => true,
            "off" => false,
            _ => !self.focus.on,
        };
        self.notice(if self.focus.on {
            "what typing goes into is framed; empty places say what they are for"
        } else {
            "no frames, no words for empty places"
        });
    }
}

/// The boxes of the empty places of equations on page `map`, each with
/// what it is for and where that is written.
fn slot_hints(
    map: &PageMap,
    file: typst::syntax::FileId,
    source: &typst::syntax::Source,
) -> Vec<(Rect, Side, &'static str)> {
    let mut hints = Vec::new();
    for glyph in map.glyphs() {
        let Origin::Math { file: of, range } = &glyph.origin else {
            continue;
        };
        if *of != file || !range.is_empty() {
            continue;
        }
        if let Some(word) = typst_text::slot_name(source, range.start) {
            let side = if ABOVE.contains(&word) {
                Side::Above
            } else {
                Side::Below
            };
            hints.push((glyph.rect, side, word));
        }
    }
    hints
}

#[cfg(test)]
mod tests {
    use super::super::tests::{compile, editing, press, tints};
    use super::*;
    use crate::chrome::Chrome;

    /// How many words for empty places that say `word` are drawn.
    fn hinted(viewer: &mut Viewer, word: &str) -> usize {
        let id = Chrome::id(&hint_markup(word), viewer.view.zoom);
        let scene = viewer.scene(&|_| false);
        scene.quads.iter().filter(|quad| quad.texture == id).count()
    }

    /// Puts the cursor at the first `needle` of the text, and begins to
    /// type there.
    fn type_at(viewer: &mut Viewer, needle: &str) {
        let at = viewer
            .text()
            .and_then(|text| text.find(needle))
            .unwrap_or_else(|| panic!("{needle} is in the text"));
        if let Some(editor) = &mut viewer.editor {
            editor.set_cursor(at);
        }
        press(viewer, "i");
    }

    #[test]
    fn what_typing_goes_into_is_framed() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    Some words and $x^2 + y$ in a line.\n";
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "gsgs");
        // In running text typing goes into the line: nothing is framed.
        type_at(&mut viewer, "words");
        assert_eq!(tints(&mut viewer, FRAME).len(), 0);
        // In normal mode nothing is typed.
        press(&mut viewer, "<Esc>");
        type_at(&mut viewer, "y$");
        press(&mut viewer, "<Esc>");
        assert_eq!(tints(&mut viewer, FRAME).len(), 0);
        // Typing in an equation goes into it: it is framed, with the
        // cursor inside the frame.
        press(&mut viewer, "i");
        let frame = tints(&mut viewer, FRAME);
        assert_eq!(frame.len(), 4);
        let bar = tints(&mut viewer, super::super::PAGE_BAR);
        let (top, left) = (frame[0], frame[2]);
        assert!(
            bar.iter()
                .any(|bar| bar.x > top.x && bar.x < top.x + top.width && bar.y >= left.y),
            "{bar:?} in {frame:?}"
        );
        // So while it is typed, and when it is typeset.
        press(&mut viewer, "z + ");
        assert_eq!(tints(&mut viewer, FRAME).len(), 4);
        compile(&mut viewer);
        assert_eq!(tints(&mut viewer, FRAME).len(), 4);
        // Out of it, nothing again.
        press(&mut viewer, "<Esc>");
        type_at(&mut viewer, " in a");
        assert_eq!(tints(&mut viewer, FRAME).len(), 0);
        // And nothing at all when it is not wanted.
        press(&mut viewer, "<Esc>:hints off<CR>");
        type_at(&mut viewer, "y$");
        assert_eq!(tints(&mut viewer, FRAME).len(), 0);
        press(&mut viewer, "<Esc>:hints<CR>i");
        assert_eq!(tints(&mut viewer, FRAME).len(), 4);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn typing_at_the_end_of_an_item_goes_on_it() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    - One item.\n\
                    - Another\n\n\
                    After the list.\n";
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "gsgs");
        type_at(&mut viewer, "\n\nAfter");
        let frame = tints(&mut viewer, FRAME);
        assert_eq!(frame.len(), 4);
        // The frame is around the second item: the first has its own.
        press(&mut viewer, "<Esc>");
        type_at(&mut viewer, "item.");
        let first = tints(&mut viewer, FRAME);
        assert_eq!(first.len(), 4);
        assert!(first[0].y < frame[0].y, "{first:?} above {frame:?}");
        // While typed on, and when typeset.
        press(&mut viewer, "<Esc>");
        type_at(&mut viewer, "\n\nAfter");
        press(&mut viewer, " one");
        assert_eq!(tints(&mut viewer, FRAME).len(), 4);
        compile(&mut viewer);
        assert_eq!(tints(&mut viewer, FRAME).len(), 4);
        // Under the list, typing is running text.
        press(&mut viewer, "<Esc>");
        type_at(&mut viewer, "the list");
        assert_eq!(tints(&mut viewer, FRAME).len(), 0);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_parts_of_a_fraction_say_what_they_are_until_filled() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    Some words and $x$ in a line.\n";
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "gsgs");
        type_at(&mut viewer, "$ in");
        press(&mut viewer, " + ff<Tab>");
        compile(&mut viewer);
        assert!(
            viewer
                .text()
                .is_some_and(|text| text.contains("$x + frac(, )$"))
        );
        assert_eq!(hinted(&mut viewer, "numerator"), 1);
        assert_eq!(hinted(&mut viewer, "denominator"), 1);
        // A key that fills a place takes its word away at once.
        press(&mut viewer, "a");
        assert_eq!(hinted(&mut viewer, "numerator"), 0);
        compile(&mut viewer);
        assert_eq!(hinted(&mut viewer, "numerator"), 0);
        assert_eq!(hinted(&mut viewer, "denominator"), 1);
        press(&mut viewer, "<Tab>b");
        compile(&mut viewer);
        assert!(
            viewer
                .text()
                .is_some_and(|text| text.contains("frac(a, b)"))
        );
        assert_eq!(hinted(&mut viewer, "denominator"), 0);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn an_empty_caption_says_it_is_one() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    Words.\n\n\
                    #figure(rect(width: 40pt, height: 20pt), caption: [])\n";
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "gsgs");
        type_at(&mut viewer, "])");
        compile(&mut viewer);
        assert_eq!(hinted(&mut viewer, "caption"), 1);
        // The caption is what typing goes into.
        assert_eq!(tints(&mut viewer, FRAME).len(), 4);
        press(&mut viewer, "A");
        assert_eq!(hinted(&mut viewer, "caption"), 0);
        compile(&mut viewer);
        assert_eq!(hinted(&mut viewer, "caption"), 0);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_table_without_rules_shows_its_cells_while_typing_in_it() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    #grid(columns: 2, gutter: 8pt, [one], [two], [three], [four])\n\n\
                    #table(columns: 2, [five], [six])\n";
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "gsgs");
        type_at(&mut viewer, "two");
        // Three edges across and three down, and the cell typed in.
        assert_eq!(tints(&mut viewer, EDGES).len(), 6);
        assert_eq!(tints(&mut viewer, FRAME).len(), 4);
        // A table with rules shows them itself.
        press(&mut viewer, "<Esc>");
        type_at(&mut viewer, "six");
        assert_eq!(tints(&mut viewer, EDGES).len(), 0);
        assert_eq!(tints(&mut viewer, FRAME).len(), 4);
        press(&mut viewer, "<Esc>");
        assert_eq!(tints(&mut viewer, FRAME).len(), 0);
        let _ = std::fs::remove_dir_all(directory);
    }
}
