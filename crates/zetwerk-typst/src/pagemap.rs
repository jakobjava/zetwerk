//! The map between glyphs on a page and bytes of source.
//!
//! Typst tags every glyph with the span of the syntax node it came from and
//! an offset into that node. That is enough to find *which node* produced a
//! glyph, but not to know whether typing at that spot is meaningful: the
//! glyph might come from a function's output, a package, a heading number, or
//! text that was transformed on the way to the page. So each glyph is
//! classified here, and only glyphs that verifiably show the exact source
//! bytes they point at count as editable.

use std::collections::HashMap;
use std::ops::Range;

use typst::World;
use typst::introspection::{Location, Tag};
use typst::layout::{Abs, Frame, FrameItem, Point, Rect, Transform};
use typst::pdf::ArtifactElem;
use typst::syntax::{FileId, Source, Span, SyntaxKind, SyntaxNode};
use typst::text::TextItem;

use crate::ProjectWorld;

/// How many bytes behind a glyph a place still counts as being by it.
const NEAR: usize = 2;

/// Where the glyphs of one page come from.
pub struct PageMap {
    runs: Vec<TextRun>,
    glyphs: Vec<MappedGlyph>,
}

/// One text item of the page frame: a run of glyphs in one font on one line.
#[derive(Debug, Clone)]
pub struct TextRun {
    /// How to reach the item from the page frame: the index of the child to
    /// descend into at each level, ending with the item's own index.
    pub path: Vec<usize>,
    /// Maps the item's coordinates (origin on the baseline at the first
    /// glyph's pen position) to page coordinates.
    pub transform: Transform,
    /// This run's glyphs within [`PageMap::glyphs`], in the same order as in
    /// the text item.
    pub glyphs: Range<usize>,
    /// The font it is set in: its family, its size in points, its weight
    /// (100 to 900) and whether it is italic.
    pub family: String,
    pub size: f64,
    pub weight: u16,
    pub italic: bool,
}

/// One glyph on the page.
#[derive(Debug, Clone)]
pub struct MappedGlyph {
    /// The glyph's cell in page coordinates: horizontally from its pen
    /// position across its advance, vertically from the font's ascender to
    /// its descender. Carets and selections are drawn from these.
    pub rect: Rect,
    /// What the glyph is, as far as editing is concerned.
    pub origin: Origin,
    /// Index of the glyph's run in [`PageMap::runs`].
    pub run: usize,
    /// Whether the glyph is part of what every page has around its text:
    /// a header, a footer, a background.
    pub furniture: bool,
    /// Whether the glyph shows nothing: a blank between words.
    pub blank: bool,
}

/// What a glyph on the page corresponds to in the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Prose the editor owns, shown exactly as written: `range` in `file`
    /// holds the very bytes this glyph displays.
    Text { file: FileId, range: Range<usize> },
    /// The space between two words. `range` is the whole whitespace run in
    /// the source, which may be several spaces or a single line break.
    Space { file: FileId, range: Range<usize> },
    /// Part of an equation the editor owns. `range` is the syntax node the
    /// glyph came from; math is edited through its source, not on the page.
    Math { file: FileId, range: Range<usize> },
    /// What a piece of markup or code put on the page: a reference, a
    /// citation, a quotation mark, the result of a call. Not the bytes of
    /// the source, but with a place in it: `range` is the syntax node, and
    /// all glyphs that came of it are handled as one.
    Object {
        file: FileId,
        range: Range<usize>,
        kind: SyntaxKind,
    },
    /// Not editable on the page, and with no place of its own in the
    /// source.
    Atomic(Atomic),
}

/// Why a glyph cannot be edited in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Atomic {
    /// Typst attached no source location: content synthesised by the
    /// compiler or by code.
    Detached,
    /// The glyph comes from a file the editor does not own, such as a
    /// package or template.
    ForeignFile,
    /// The glyph is an ornament of a piece of markup that holds text of
    /// its own: the number of a heading, the marker of a list item.
    Markup(SyntaxKind),
    /// The glyph points at a text node but does not show that node's bytes,
    /// because the text was transformed (case change, hyphenation, a `show`
    /// rule replacing it).
    Transformed,
}

impl PageMap {
    /// Maps every glyph of `frame`, the frame of one page.
    #[must_use]
    pub fn build(world: &ProjectWorld, frame: &Frame) -> Self {
        let mut builder = Builder {
            world,
            runs: Vec::new(),
            glyphs: Vec::new(),
            path: Vec::new(),
            resolved: None,
            indexes: HashMap::new(),
            open: Vec::new(),
            furniture: None,
            enclosing: Enclosing::Unknown,
        };
        builder.frame(frame, Transform::identity());
        let mut glyphs = builder.glyphs;
        if world.shows_more() {
            // In bytes of the files as they are written.
            for glyph in &mut glyphs {
                glyph.origin.as_written(world);
            }
        }
        Self {
            runs: builder.runs,
            glyphs,
        }
    }

    /// All text items of the page, in paint order.
    #[must_use]
    pub fn runs(&self) -> &[TextRun] {
        &self.runs
    }

    /// All glyphs of the page, in paint order.
    #[must_use]
    pub fn glyphs(&self) -> &[MappedGlyph] {
        &self.glyphs
    }

    /// Where on the page the cursor at byte `offset` of `file` is: the
    /// cell of the glyph that shows the text there. `None` if no glyph on
    /// this page does, as for markup and code that set nothing themselves.
    #[must_use]
    pub fn caret(&self, file: FileId, offset: usize) -> Option<Caret> {
        let mut inside = None;
        let mut size = None;
        let mut behind = None;
        // The glyph that ends nearest before the place: where it ends,
        // and how many bytes it stands for.
        let mut near: Option<(usize, usize, Caret)> = None;
        for glyph in &self.glyphs {
            if matches!(glyph.origin, Origin::Object { .. }) {
                // Those are for `object` to find, whole.
                continue;
            }
            let Some(range) = glyph.origin.range_in(file) else {
                continue;
            };
            let caret = |after| Caret {
                cell: glyph.rect,
                after,
                run: glyph.run,
            };
            if range.start == offset {
                // The glyph that begins here: nothing fits better.
                return Some(caret(false));
            }
            if range.contains(&offset) {
                // Of several that the place is in (a symbol of an
                // equation, and the equation's number, which stands for
                // all of it), the one that is nearest to it.
                if size.is_none_or(|size| range.len() < size) {
                    size = Some(range.len());
                    inside = Some(caret(false));
                }
            } else if range.end == offset {
                behind = Some((range.len(), caret(true)));
            } else if range.end < offset
                && range.end + NEAR >= offset
                && near.as_ref().is_none_or(|(end, _, _)| range.end >= *end)
            {
                near = Some((range.end, range.len(), caret(true)));
            }
        }
        // A blank or two behind a symbol is by that symbol, not "in"
        // what stands for all of the equation (its number).
        if let (Some(size), None, Some((end, length, caret))) = (size, &behind, near)
            && size > length + (offset - end)
        {
            return Some(caret);
        }
        // Behind a glyph that shows a few bytes rather than "in" one
        // that stands for a whole equation.
        match (inside, size, behind) {
            (Some(_), Some(size), Some((length, behind))) if size > length => Some(behind),
            (Some(inside), ..) => Some(inside),
            (None, _, behind) => behind.map(|(_, caret)| caret),
        }
    }

    /// Where a cursor is that stands behind the last text of `file` shown
    /// on this page before byte `offset`: behind the glyph that ends
    /// nearest before it. For a place that shows as nothing itself, such
    /// as the end of a paragraph.
    #[must_use]
    pub fn caret_before(&self, file: FileId, offset: usize) -> Option<Caret> {
        let mut nearest: Option<(usize, Caret)> = None;
        for glyph in &self.glyphs {
            if matches!(glyph.origin, Origin::Object { .. }) {
                continue;
            }
            let Some(range) = glyph.origin.range_in(file) else {
                continue;
            };
            if range.end <= offset && nearest.is_none_or(|(end, _)| range.end > end) {
                let caret = Caret {
                    cell: glyph.rect,
                    after: true,
                    run: glyph.run,
                };
                nearest = Some((range.end, caret));
            }
        }
        nearest.map(|(_, caret)| caret)
    }

    /// The object that byte `offset` of `file` is in the source of, as
    /// the stretches of the page it takes up: of a reference, what it
    /// refers to by. The innermost, if objects are nested; nothing, if
    /// the byte is in no object that shows on this page.
    #[must_use]
    pub fn object(&self, file: FileId, offset: usize) -> Vec<Rect> {
        let objects = || {
            self.glyphs.iter().filter_map(|glyph| match &glyph.origin {
                Origin::Object {
                    file: of, range, ..
                } if *of == file && range.contains(&offset) => Some((range, glyph.rect)),
                _ => None,
            })
        };
        let Some(innermost) = objects()
            .map(|(range, _)| range)
            .min_by_key(|range| range.len())
        else {
            return Vec::new();
        };
        let mut rects = Vec::new();
        for (_, cell) in objects().filter(|(range, _)| *range == innermost) {
            join(&mut rects, cell);
        }
        rects
    }

    /// The place in the source that `point` on the page stands for: the
    /// file and byte of the nearest glyph that shows source text, before
    /// or behind it depending on which half of it the point is in. Glyphs
    /// on the point's line count as nearer than any on another line.
    #[must_use]
    pub fn source_at(&self, point: Point) -> Option<(FileId, usize)> {
        let mut best: Option<(f64, FileId, usize)> = None;
        for glyph in &self.glyphs {
            let (file, range, splits) = match &glyph.origin {
                Origin::Text { file, range } => (*file, range, true),
                Origin::Space { file, range }
                | Origin::Math { file, range }
                | Origin::Object { file, range, .. } => (*file, range, false),
                Origin::Atomic(_) => continue,
            };
            let cell = glyph.rect;
            let beside = (cell.min.x - point.x)
                .max(point.x - cell.max.x)
                .max(Abs::zero());
            let above = (cell.min.y - point.y)
                .max(point.y - cell.max.y)
                .max(Abs::zero());
            let distance = above.to_pt() * 1000.0 + beside.to_pt();
            if best.is_none_or(|(nearest, _, _)| distance < nearest) {
                let behind = splits && point.x > cell.min.x + (cell.max.x - cell.min.x) / 2.0;
                let offset = if behind { range.end } else { range.start };
                best = Some((distance, file, offset));
            }
        }
        best.map(|(_, file, offset)| (file, offset))
    }

    /// The lines of text on the page, as far as they show text of the
    /// project's files.
    #[must_use]
    pub fn lines(&self) -> PageLines {
        let mut lines: Vec<PageLine> = Vec::new();
        // Not what stands around the text on every page: a cursor that
        // goes down the text does not go through the running titles.
        for glyph in self.glyphs.iter().filter(|glyph| !glyph.furniture) {
            let cell = glyph.rect;
            let (left, right) = (cell.min.x.to_pt(), cell.max.x.to_pt());
            let (top, bottom) = (cell.min.y.to_pt(), cell.max.y.to_pt());
            // Text the document makes itself (a number, a reference) is
            // no place for the cursor, but part of its line: without it
            // the line would fall into pieces.
            // So is what was put in to show the text (see
            // `Project::snapshot_showing`): it stands for no bytes. Only
            // the box in a part of an equation that is yet to be typed
            // is a place for the cursor all the same: the place to type
            // that part at.
            let place = glyph
                .origin
                .place()
                .filter(|(_, range)| !range.is_empty() || glyph.origin.is_empty_place());
            let entry = place.map(|(file, range)| LineGlyph {
                left,
                right,
                file,
                offset: range.start,
                end: range.end,
                blank: glyph.blank || matches!(glyph.origin, Origin::Space { .. }),
            });
            match lines
                .iter_mut()
                .rev()
                .find(|line| line.takes(left, right, top, bottom))
            {
                Some(line) => {
                    line.left = line.left.min(left);
                    line.right = line.right.max(right);
                    line.glyphs.extend(entry);
                }
                None => lines.push(PageLine {
                    top,
                    bottom,
                    left,
                    right,
                    glyphs: entry.into_iter().collect(),
                }),
            }
        }
        lines.retain(|line| !line.glyphs.is_empty());
        // The places for a cursor: each glyph, but all the glyphs one
        // thing made (a reference, a call) as one.
        let mut stops: Vec<Stop> = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            let mut across: Vec<&LineGlyph> = line.glyphs.iter().collect();
            across.sort_by(|a, b| order(a.left, b.left));
            // The blank a line was broken at shows as nothing: it is no
            // place for a cursor.
            while across.len() > 1 && across.last().is_some_and(|glyph| glyph.blank) {
                across.pop();
            }
            // Only those of this line can be the same: looking through
            // all of the page's took as long as the square of its glyphs.
            let first = stops.len();
            for glyph in across {
                let known = stops[first..].iter().any(|stop| {
                    stop.line == index && (stop.file, stop.start, stop.end) == glyph.place()
                });
                if !known {
                    stops.push(Stop {
                        line: index,
                        file: glyph.file,
                        start: glyph.offset,
                        end: glyph.end,
                        blank: glyph.blank,
                    });
                }
            }
        }
        PageLines { lines, stops }
    }

    /// Makes the map one of the text of `file` with bytes `taken` taken
    /// out and `put` bytes put in their place, where the page is the same
    /// (a part was put into another page): the bytes behind them move.
    /// Returns whether it could be: not if a glyph shows any of the bytes
    /// taken out, or is of a node the bytes were put into, as a heading
    /// in a table of contents is. The map is then as it was.
    pub fn shift(&mut self, file: FileId, taken: &Range<usize>, put: usize) -> bool {
        let origins = || self.glyphs.iter().filter_map(|glyph| glyph.origin.place());
        if origins().any(|(of, range)| of == file && touches(range, taken)) {
            return false;
        }
        for glyph in &mut self.glyphs {
            if let Some(range) = glyph.origin.range_mut(file) {
                *range = moved(range, taken, put);
            }
        }
        true
    }

    /// The cells of the glyphs that show bytes `range` of `file`, joined
    /// where they stand side by side on a line.
    #[must_use]
    pub fn selection(&self, file: FileId, range: &Range<usize>) -> Vec<Rect> {
        let mut rects: Vec<Rect> = Vec::new();
        for glyph in &self.glyphs {
            let selected = glyph
                .origin
                .range_in(file)
                .is_some_and(|shown| shown.start < range.end && shown.end > range.start);
            if !selected {
                continue;
            }
            join(&mut rects, glyph.rect);
        }
        rects
    }
}

/// Whether bytes `range` of a text are, or hold, some of bytes `taken`,
/// or the place they were taken from: what is shown of them changes
/// with them.
fn touches(range: &Range<usize>, taken: &Range<usize>) -> bool {
    if taken.is_empty() {
        range.start < taken.start && taken.start < range.end
    } else {
        range.start < taken.end && taken.start < range.end
    }
}

/// Where bytes `range` of a text are after bytes `taken` of it were
/// replaced by `put` bytes, if they are none of them: what ends where
/// bytes were put in stays, what begins there moves.
fn moved(range: &Range<usize>, taken: &Range<usize>, put: usize) -> Range<usize> {
    let byte = |then: usize| {
        if then < taken.start {
            then
        } else {
            then.max(taken.end) - taken.len() + put
        }
    };
    let start = byte(range.start);
    if range.end <= taken.start && !range.is_empty() {
        start..range.end
    } else {
        start..byte(range.end).max(start)
    }
}

/// Adds `cell` to `rects`: to the last of them, if it goes on where that
/// ends.
fn join(rects: &mut Vec<Rect>, cell: Rect) {
    let slack = Abs::pt(0.5);
    match rects.last_mut() {
        Some(last)
            if (last.min.y - cell.min.y).abs() < slack
                && (last.max.y - cell.max.y).abs() < slack
                && cell.min.x >= last.min.x
                && cell.min.x <= last.max.x + slack =>
        {
            last.max.x = last.max.x.max(cell.max.x);
        }
        _ => rects.push(cell),
    }
}

/// The lines of text of a page, in the order the page has them (the
/// order of reading, but for what stands in the margins): what moving up
/// and down on a page goes by. All measures are points from the page's
/// top-left corner.
#[derive(Debug)]
pub struct PageLines {
    lines: Vec<PageLine>,
    /// The places for a cursor, line by line and in each from left to
    /// right.
    stops: Vec<Stop>,
}

/// A place for a cursor on a page: the bytes of a file that are shown
/// there, and the line it is in.
#[derive(Debug, Clone, Copy)]
struct Stop {
    line: usize,
    file: FileId,
    start: usize,
    end: usize,
    /// Whether it is the blank between two words: at the end of a line
    /// it shows as nothing.
    blank: bool,
}

/// A stretch of glyphs side by side with no gap to speak of. The lines of
/// two columns that stand at the same height are two of these, and so are
/// the cells of a table's row.
#[derive(Debug)]
struct PageLine {
    top: f64,
    bottom: f64,
    left: f64,
    right: f64,
    /// In the order the page has them.
    glyphs: Vec<LineGlyph>,
}

#[derive(Debug)]
struct LineGlyph {
    left: f64,
    right: f64,
    /// The file and the byte of it that the glyph begins to show, and
    /// the byte after the last it shows.
    file: FileId,
    offset: usize,
    end: usize,
    /// Whether it is the blank between two words.
    blank: bool,
}

impl LineGlyph {
    /// The middle of the glyph: an aim that does not flip to the glyph
    /// before it when it comes back a hair off.
    fn middle(&self) -> f64 {
        self.left + (self.right - self.left) / 2.0
    }

    fn place(&self) -> (FileId, usize, usize) {
        (self.file, self.offset, self.end)
    }

    /// Whether the glyph shows byte `offset` of `file`.
    fn shows(&self, file: FileId, offset: usize) -> bool {
        self.file == file && self.offset <= offset && offset < self.end
    }
}

impl PageLine {
    fn height(&self) -> f64 {
        self.bottom - self.top
    }

    fn middle(&self) -> f64 {
        self.top + self.height() / 2.0
    }

    /// Whether a glyph with this cell is one of this line's.
    fn takes(&self, left: f64, right: f64, top: f64, bottom: f64) -> bool {
        let middle = top + (bottom - top) / 2.0;
        if middle < self.top || middle > self.bottom {
            return false;
        }
        // Blanks are glyphs too, so what belongs together stands close:
        // closer than the columns of a page do.
        let gap = (left - self.right).max(self.left - right);
        gap <= self.height().max(bottom - top) * 0.75
    }

    /// Whether `other` stands above or below this line and not in a
    /// column next to it.
    fn in_column_with(&self, other: &PageLine) -> bool {
        let slack = self.height() / 2.0;
        other.left <= self.right + slack && other.right >= self.left - slack
    }

    /// How far `across` is from the line, sideways.
    fn aside(&self, across: f64) -> f64 {
        (self.left - across).max(across - self.right).max(0.0)
    }
}

impl PageLines {
    /// Like [`PageMap::shift`], for the lines of a page.
    pub fn shift(&mut self, file: FileId, taken: &Range<usize>, put: usize) -> bool {
        let glyphs = || self.lines.iter().flat_map(|line| &line.glyphs);
        if glyphs().any(|glyph| glyph.file == file && touches(&(glyph.offset..glyph.end), taken)) {
            return false;
        }
        for glyph in self.lines.iter_mut().flat_map(|line| &mut line.glyphs) {
            if glyph.file == file {
                let now = moved(&(glyph.offset..glyph.end), taken, put);
                (glyph.offset, glyph.end) = (now.start, now.end);
            }
        }
        for stop in &mut self.stops {
            if stop.file == file {
                let now = moved(&(stop.start..stop.end), taken, put);
                (stop.start, stop.end) = (now.start, now.end);
            }
        }
        true
    }

    /// How many lines of the page show text of the file.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// Whether the page shows no text of the file.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Whether a line of the page shows byte `offset` of `file`.
    #[must_use]
    pub fn shows(&self, file: FileId, offset: usize) -> bool {
        self.lines
            .iter()
            .flat_map(|line| &line.glyphs)
            .any(|glyph| glyph.shows(file, offset))
    }

    /// The line that shows byte `offset` of the source, or the text
    /// nearest before it (after it, if there is none before), and how far
    /// across the page the middle of that is.
    #[must_use]
    pub fn find(&self, file: FileId, offset: usize) -> Option<(usize, f64)> {
        let glyphs = || {
            self.lines
                .iter()
                .enumerate()
                .flat_map(|(index, line)| line.glyphs.iter().map(move |glyph| (index, glyph)))
                .filter(|(_, glyph)| glyph.file == file)
        };
        // Of several that begin at the same byte (all that one call put
        // on the page), the first.
        let mut before: Option<(usize, &LineGlyph)> = None;
        for (index, glyph) in glyphs().filter(|(_, glyph)| glyph.offset <= offset) {
            if before.is_none_or(|(_, best)| glyph.offset > best.offset) {
                before = Some((index, glyph));
            }
        }
        before
            .or_else(|| glyphs().min_by_key(|(_, glyph)| glyph.offset))
            .map(|(index, glyph)| (index, glyph.middle()))
    }

    /// How far across the page line `line` shows byte `offset`, or else
    /// begins.
    #[must_use]
    pub fn across(&self, line: usize, file: FileId, offset: usize) -> Option<f64> {
        let line = self.lines.get(line)?;
        let shown = line.glyphs.iter().find(|glyph| glyph.shows(file, offset));
        Some(shown.map_or(line.left, LineGlyph::middle))
    }

    /// The line below `line` (above it, if not `down`) for a cursor that
    /// aims `across`, and where the cursor aims from there on. `None` if
    /// the line is on another page.
    ///
    /// That is the line the page has next, if it lies below: so the
    /// cursor reads on, also where the next line stands to the side (a
    /// centred equation after a short line). Otherwise it is the nearest
    /// line straight below, or else the line the page has next if that
    /// begins a column further right; the cursor then aims as far into
    /// that column as it did into this one.
    #[must_use]
    pub fn step(&self, line: usize, down: bool, across: f64) -> Option<(usize, f64)> {
        let from = self.lines.get(line)?;
        // How far beyond `from` a line is, the way the cursor goes.
        let beyond = |other: &PageLine| {
            if down {
                other.middle() - from.bottom
            } else {
                from.top - other.middle()
            }
        };
        let others = || {
            self.lines
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != line)
        };
        let aimed_at = |a: &(usize, &PageLine), b: &(usize, &PageLine)| {
            order(a.1.aside(across), b.1.aside(across))
        };
        // The nearest line beyond `from`, of those that `count`; of
        // several at one height, the one the cursor aims at.
        let nearest = |count: &dyn Fn(&PageLine) -> bool| {
            let beyond_it = || others().filter(|(_, other)| count(other) && beyond(other) > 0.0);
            let least = beyond_it()
                .map(|(_, other)| beyond(other))
                .reduce(f64::min)?;
            beyond_it()
                .filter(|(_, other)| beyond(other) <= least + from.height() / 2.0)
                .min_by(aimed_at)
                .map(|(index, _)| index)
        };
        // The line the page has next, not counting what stands beside
        // this one (the number of an equation, the next cell of a row).
        let beside = |other: &PageLine| other.middle() >= from.top && other.middle() <= from.bottom;
        let next = if down {
            (line + 1..self.lines.len()).find(|index| !beside(&self.lines[*index]))
        } else {
            (0..line).rev().find(|index| !beside(&self.lines[*index]))
        };
        let next = next.and_then(|index| Some((index, self.lines.get(index)?)));

        if let Some((index, next)) = next
            && beyond(next) > 0.0
        {
            let level =
                |other: &PageLine| (other.middle() - next.middle()).abs() <= next.height() / 2.0;
            return others()
                .filter(|(at, other)| *at == index || (level(other) && from.in_column_with(other)))
                .min_by(aimed_at)
                .map(|(index, _)| (index, across));
        }
        if let Some(index) = nearest(&|other| from.in_column_with(other)) {
            return Some((index, across));
        }
        if let Some((index, next)) = next {
            let slack = from.height() / 2.0;
            let further = if down {
                next.left > from.right
            } else {
                next.right < from.left
            };
            // A column begins level with the ones before it and ends
            // level with the ones after it. What stands above or below
            // all the text (a running title) is not one.
            let column = self.lines.iter().enumerate().any(|(at, other)| {
                if down {
                    at <= line && other.top <= next.top + slack
                } else {
                    at >= line && other.bottom >= next.bottom - slack
                }
            });
            if further && column {
                return Some((index, across - from.left + next.left));
            }
        }
        nearest(&|_| true).map(|index| (index, across))
    }

    /// Whether byte `offset` of `file` is within what the page shows of
    /// that file: not before the first byte it shows nor behind the last.
    #[must_use]
    pub fn covers(&self, file: FileId, offset: usize) -> bool {
        let of_file = || self.stops.iter().filter(|stop| stop.file == file);
        let first = of_file().map(|stop| stop.start).min();
        let last = of_file().map(|stop| stop.end).max();
        matches!((first, last), (Some(first), Some(last)) if first <= offset && offset <= last)
    }

    /// The place for a cursor that begins at byte `offset` of `file`, as
    /// the bytes shown there; `None` if none begins there.
    #[must_use]
    pub fn stop_at(&self, file: FileId, offset: usize) -> Option<Range<usize>> {
        let stop = self.stops.get(self.index_at(file, offset)?)?;
        Some(stop.start..stop.end)
    }

    /// Which of the places for a cursor begins at byte `offset` of
    /// `file`. Of two that do, the one that shows no bytes: an empty
    /// part of an equation stands at the byte of what follows it, and
    /// the cursor at that byte is in the empty part.
    fn index_at(&self, file: FileId, offset: usize) -> Option<usize> {
        let at = |(_, stop): &(usize, &Stop)| stop.file == file && stop.start == offset;
        let found = self.stops.iter().enumerate().filter(at);
        found
            .min_by_key(|(_, stop)| stop.end)
            .map(|(index, _)| index)
    }

    /// The first place for a cursor of `file` that begins at byte
    /// `offset` or behind it (`forward`), or the last that begins at it
    /// or before it.
    #[must_use]
    pub fn stop_near(&self, file: FileId, offset: usize, forward: bool) -> Option<Range<usize>> {
        let of_file = self.stops.iter().filter(|stop| stop.file == file);
        let found = if forward {
            of_file
                .filter(|stop| stop.start >= offset)
                .min_by_key(|stop| (stop.start, stop.end))
        } else {
            of_file
                .filter(|stop| stop.start <= offset)
                .max_by_key(|stop| (stop.start, std::cmp::Reverse(stop.end)))
        };
        found.map(|stop| stop.start..stop.end)
    }

    /// The place for a cursor after the one that begins at byte `offset`
    /// of `file` (`forward`), or before it, in its line and of its file.
    #[must_use]
    pub fn stop_beside(&self, file: FileId, offset: usize, forward: bool) -> Option<Range<usize>> {
        let at = self.index_at(file, offset)?;
        let line = self.stops[at].line;
        // Not what begins at the same byte: that is this place again.
        let in_line = |stop: &&Stop| stop.line == line && stop.file == file && stop.start != offset;
        let beside = if forward {
            self.stops[at + 1..].iter().find(in_line)
        } else {
            self.stops[..at].iter().rev().find(in_line)
        };
        beside.map(|stop| stop.start..stop.end)
    }

    /// The line that shows byte `offset` of `file`, as bytes of that
    /// file: the byte its first place for a cursor begins with, the byte
    /// its last one begins with, and the byte behind that one. The line
    /// is the one of the place that begins at the byte, or else of the
    /// nearest before it.
    #[must_use]
    pub fn line_span(&self, file: FileId, offset: usize) -> Option<(usize, usize, usize)> {
        let of_file = || self.stops.iter().filter(|stop| stop.file == file);
        let line = of_file()
            .filter(|stop| stop.start <= offset)
            .max_by_key(|stop| stop.start)
            .or_else(|| of_file().min_by_key(|stop| stop.start))?
            .line;
        let in_line = || of_file().filter(|stop| stop.line == line);
        let first = in_line().next()?;
        // The blank a line ends with is where it was broken: the line
        // ends before it.
        let last = in_line()
            .rev()
            .find(|stop| !stop.blank)
            .or_else(|| in_line().next_back())?;
        Some((first.start.min(last.start), last.start, last.end))
    }

    /// The file and the byte that line `line` begins with: those of its
    /// leftmost glyph.
    #[must_use]
    pub fn first(&self, line: usize) -> Option<(FileId, usize)> {
        let line = self.lines.get(line)?;
        line.glyphs
            .iter()
            .min_by(|a, b| order(a.left, b.left))
            .map(|glyph| (glyph.file, glyph.offset))
    }

    /// From where to where down the page line `line` reaches.
    #[must_use]
    pub fn band(&self, line: usize) -> Option<(f64, f64)> {
        self.lines.get(line).map(|line| (line.top, line.bottom))
    }

    /// From where to where across the page line `line` reaches.
    #[must_use]
    pub fn extent(&self, line: usize) -> Option<(f64, f64)> {
        self.lines.get(line).map(|line| (line.left, line.right))
    }

    /// The line a cursor comes to first when it enters the page from
    /// above (from below, if not `down`): the first the page has (the
    /// last, unless that stands above all the others, as a running title
    /// does: then the lowest).
    #[must_use]
    pub fn enter(&self, down: bool) -> Option<usize> {
        let last = self.lines.len().checked_sub(1)?;
        if down {
            return Some(0);
        }
        let top = self.lines.get(last)?.top;
        let above_all = self.lines.iter().take(last).all(|other| other.top > top);
        if last == 0 || !above_all {
            return Some(last);
        }
        self.lines
            .iter()
            .enumerate()
            .min_by(|a, b| order(b.1.bottom, a.1.bottom).then(order(b.1.right, a.1.right)))
            .map(|(index, _)| index)
    }

    /// The place in the sources shown `across` on line `line`: the file
    /// and the byte of the glyph there, or of the nearest one. Glyphs
    /// that show the place `not` do not count: `None` if the line has no
    /// others.
    #[must_use]
    pub fn at(&self, line: usize, across: f64, not: (FileId, usize)) -> Option<(FileId, usize)> {
        let aside = |glyph: &LineGlyph| {
            if across < glyph.left {
                glyph.left - across
            } else if across >= glyph.right {
                // Where two glyphs meet, the place belongs to the right
                // one.
                across - glyph.right + 0.001
            } else {
                0.0
            }
        };
        self.lines
            .get(line)?
            .glyphs
            .iter()
            .filter(|glyph| !glyph.shows(not.0, not.1))
            .min_by(|a, b| order(aside(a), aside(b)))
            .map(|glyph| (glyph.file, glyph.offset))
    }

    /// The lines that show some of the bytes `ranges` of `file` (in
    /// order, apart): each line with the index of the range it shows,
    /// once for each.
    #[must_use]
    pub fn showing(&self, file: FileId, ranges: &[Range<usize>]) -> Vec<(usize, usize)> {
        let mut found = Vec::new();
        for (index, line) in self.lines.iter().enumerate() {
            for glyph in line.glyphs.iter().filter(|glyph| glyph.file == file) {
                // From the first range that ends behind where the glyph
                // begins, those that begin before it ends.
                let first = ranges.partition_point(|range| range.end <= glyph.offset);
                let end = glyph.end.max(glyph.offset + 1);
                let shown = ranges[first..]
                    .iter()
                    .take_while(|range| range.start < end)
                    .count();
                found.extend((first..first + shown).map(|range| (index, range)));
            }
        }
        found.sort_unstable();
        found.dedup();
        found
    }
}

impl PageLines {
    /// The lines of a page that shows no text, only one thing made of
    /// bytes `range` of `file` (a page put in whole, from a PDF or an
    /// image): one line across `rect`, with that thing as its one place
    /// for a cursor. What [`PageMap::lines`] finds of glyphs alone.
    #[must_use]
    pub fn of_one(file: FileId, range: Range<usize>, rect: Rect) -> Self {
        let (left, right) = (rect.min.x.to_pt(), rect.max.x.to_pt());
        let (top, bottom) = (rect.min.y.to_pt(), rect.max.y.to_pt());
        let glyph = LineGlyph {
            left,
            right,
            file,
            offset: range.start,
            end: range.end,
            blank: false,
        };
        let stop = Stop {
            line: 0,
            file,
            start: range.start,
            end: range.end,
            blank: false,
        };
        Self {
            lines: vec![PageLine {
                top,
                bottom,
                left,
                right,
                glyphs: vec![glyph],
            }],
            stops: vec![stop],
        }
    }
}

fn order(a: f64, b: f64) -> std::cmp::Ordering {
    a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
}

/// Where a cursor is on a page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Caret {
    /// The cell of a glyph, in page coordinates.
    pub cell: Rect,
    /// Whether the cursor is behind that glyph (at the end of the text it
    /// shows) and not on it.
    pub after: bool,
    /// The run the glyph is of, in [`PageMap::runs`].
    pub run: usize,
}

impl Origin {
    /// The file and the bytes of it this glyph stands for, if it stands
    /// for any.
    fn place(&self) -> Option<(FileId, &Range<usize>)> {
        match self {
            Self::Text { file, range }
            | Self::Space { file, range }
            | Self::Math { file, range }
            | Self::Object { file, range, .. } => Some((*file, range)),
            Self::Atomic(_) => None,
        }
    }

    /// Makes the bytes this glyph stands for those of the file as it is
    /// written, from those of what Typst was given.
    fn as_written(&mut self, world: &ProjectWorld) {
        if let Self::Text { file, range }
        | Self::Space { file, range }
        | Self::Math { file, range }
        | Self::Object { file, range, .. } = self
        {
            *range = world.written_at(*file, range.start)..world.written_at(*file, range.end);
        }
    }

    /// Whether this glyph is the box in a part of an equation that is
    /// yet to be typed: it was put in to show the equation, at the byte
    /// that part is typed at.
    fn is_empty_place(&self) -> bool {
        matches!(self, Self::Math { range, .. } if range.is_empty())
    }

    /// The bytes of `file` this glyph stands for, to be changed, if it
    /// stands for any.
    fn range_mut(&mut self, file: FileId) -> Option<&mut Range<usize>> {
        match self {
            Self::Text { file: of, range }
            | Self::Space { file: of, range }
            | Self::Math { file: of, range }
            | Self::Object {
                file: of, range, ..
            } if *of == file => Some(range),
            _ => None,
        }
    }

    /// The bytes of `file` this glyph stands for, if it stands for any.
    fn range_in(&self, file: FileId) -> Option<&Range<usize>> {
        match self {
            Self::Text { file: of, range }
            | Self::Space { file: of, range }
            | Self::Math { file: of, range }
            | Self::Object {
                file: of, range, ..
            } if *of == file => Some(range),
            _ => None,
        }
    }
}

/// Finds the syntax node a span belongs to.
///
/// `Source::find` walks the children of the root one by one, which for a
/// long file with thousands of paragraphs costs milliseconds per page.
/// Span numbers grow along the tree, so the child of the root to look in
/// can be found by bisection; below it the trees are small.
struct SpanIndex {
    source: Source,
    /// The span number and the byte offset of every child of the root.
    children: Vec<(u64, usize)>,
}

/// What a span was found to be: its bytes, its kind, and whether it is
/// part of an equation.
type Found = (Range<usize>, SyntaxKind, bool);

/// How many indexes of sources are kept for the next map: those of the
/// files on the pages shown, as they are now.
const INDEXES_KEPT: usize = 4;

impl SpanIndex {
    /// The index of `source`: made once for all the pages mapped of the
    /// same text, not for each (it goes through all of the root's
    /// children, which in a long document are many).
    fn of(source: Source) -> std::rc::Rc<Self> {
        thread_local! {
            static KEPT: std::cell::RefCell<Vec<std::rc::Rc<SpanIndex>>> =
                const { std::cell::RefCell::new(Vec::new()) };
        }
        // An index keeps its source, and with it its bytes where they
        // are: where those are tells the sources apart (not where there
        // are none: all empty texts are in the same place).
        let same = |index: &&std::rc::Rc<Self>| {
            index.source.id() == source.id()
                && !source.text().is_empty()
                && std::ptr::eq(index.source.text(), source.text())
        };
        let known = KEPT.with_borrow(|kept| kept.iter().find(same).cloned());
        known.unwrap_or_else(|| {
            let index = std::rc::Rc::new(Self::new(source));
            KEPT.with_borrow_mut(|kept| {
                if kept.len() >= INDEXES_KEPT {
                    kept.remove(0);
                }
                kept.push(std::rc::Rc::clone(&index));
            });
            index
        })
    }

    fn new(source: Source) -> Self {
        let mut offset = 0;
        let children = source
            .root()
            .children()
            .map(|child| {
                let entry = (number(child.span()), offset);
                offset += child.len();
                entry
            })
            .collect();
        Self { source, children }
    }

    fn find(&self, span: Span) -> Option<Found> {
        let root = self.source.root();
        if root.span() == span {
            return Some((0..root.len(), root.kind(), false));
        }
        let target = number(span);
        let index = self
            .children
            .partition_point(|(first, _)| *first <= target)
            .checked_sub(1)?;
        let child = root.children().as_slice().get(index)?;
        descend(child, self.children[index].1, span, target, false)
    }
}

/// A span's number within its file.
fn number(span: Span) -> u64 {
    span.into_raw().get() & ((1 << 48) - 1)
}

fn descend(
    node: &SyntaxNode,
    mut offset: usize,
    span: Span,
    target: u64,
    in_equation: bool,
) -> Option<Found> {
    // What is written as it stands is that wherever it is: its
    // characters are text, each with its place, in an equation too
    // (where it can only be in something that is put into it).
    let in_equation =
        (in_equation || node.kind() == SyntaxKind::Equation) && node.kind() != SyntaxKind::Raw;
    if node.span() == span {
        return Some((offset..offset + node.len(), node.kind(), in_equation));
    }
    // A node has a smaller number than everything below it, and all of a
    // child's subtree has smaller numbers than the next child.
    if target < number(node.span()) {
        return None;
    }
    let mut children = node.children().peekable();
    while let Some(child) = children.next() {
        if children
            .peek()
            .is_none_or(|next| number(next.span()) > target)
            && let Some(found) = descend(child, offset, span, target, in_equation)
        {
            return Some(found);
        }
        offset += child.len();
    }
    None
}

struct Builder<'a> {
    world: &'a ProjectWorld,
    runs: Vec<TextRun>,
    glyphs: Vec<MappedGlyph>,
    /// Child indices from the page frame down to the item being visited.
    path: Vec<usize>,
    /// The node the previous glyph resolved to. Neighbouring glyphs nearly
    /// always share a span, and finding a node means a tree search.
    resolved: Option<(Span, Resolved)>,
    indexes: HashMap<FileId, std::rc::Rc<SpanIndex>>,
    /// The elements that have begun and not ended where the walk is, the
    /// innermost last: what the glyphs there are part of.
    open: Vec<(Location, Span)>,
    /// Where among them the page's furniture begins, if the walk is in
    /// it: Typst marks headers, footers and backgrounds as artifacts.
    furniture: Option<usize>,
    /// What a glyph with no place of its own in the source is taken to
    /// come from there.
    enclosing: Enclosing,
}

/// What the glyphs at one place of the walk are part of.
enum Enclosing {
    /// Not asked for yet since an element began or ended.
    Unknown,
    /// This, or nothing that has a place in the source.
    Known(Option<Origin>),
}

/// What a span points at.
#[derive(Clone)]
enum Resolved {
    Unmappable(Atomic),
    Node {
        source: Source,
        file: FileId,
        range: Range<usize>,
        kind: NodeKind,
    },
}

#[derive(Clone, Copy)]
enum NodeKind {
    Text,
    /// A link written as it is (`https://…`): shown as written, letter
    /// by letter, like text; else as one thing.
    Link,
    Space,
    Math,
    Object(SyntaxKind),
}

impl Builder<'_> {
    fn frame(&mut self, frame: &Frame, transform: Transform) {
        for (index, (pos, item)) in frame.items().enumerate() {
            let transform = transform.pre_concat(Transform::translate(pos.x, pos.y));
            self.path.push(index);
            match item {
                FrameItem::Group(group) => {
                    self.frame(&group.frame, transform.pre_concat(group.transform));
                }
                FrameItem::Text(text) => self.text(text, transform),
                FrameItem::Tag(Tag::Start(element, _)) => {
                    if let Some(location) = element.location() {
                        if self.furniture.is_none() && element.is::<ArtifactElem>() {
                            self.furniture = Some(self.open.len());
                        }
                        self.open.push((location, element.span()));
                        self.enclosing = Enclosing::Unknown;
                    }
                }
                FrameItem::Tag(Tag::End(location, ..)) => {
                    // An element that began on an earlier page is not
                    // among them.
                    if let Some(at) = self.open.iter().rposition(|(open, _)| open == location) {
                        self.open.remove(at);
                        self.enclosing = Enclosing::Unknown;
                        match self.furniture {
                            Some(first) if first == at => self.furniture = None,
                            Some(first) if first > at => self.furniture = Some(first - 1),
                            _ => {}
                        }
                    }
                }
                FrameItem::Shape(..) | FrameItem::Image(..) | FrameItem::Link(..) => {}
            }
            self.path.pop();
        }
    }

    fn text(&mut self, text: &TextItem, transform: Transform) {
        let metrics = text.font.metrics();
        let top = -metrics.ascender.at(text.size);
        let bottom = -metrics.descender.at(text.size);
        let mut pen = Abs::zero();
        let run = self.runs.len();
        let first = self.glyphs.len();
        for glyph in &text.glyphs {
            let advance = glyph.x_advance.at(text.size);
            let cell = Rect::new(Point::new(pen, top), Point::new(pen + advance, bottom));
            pen += advance;

            let shown = text.text.get(glyph.range());
            let origin = self.origin(glyph.span, shown);
            self.glyphs.push(MappedGlyph {
                rect: transform_rect(cell, transform),
                origin,
                run,
                furniture: self.furniture.is_some(),
                blank: shown.is_some_and(|shown| shown.trim().is_empty()),
            });
        }
        let info = text.font.info();
        self.runs.push(TextRun {
            path: self.path.clone(),
            transform,
            glyphs: first..self.glyphs.len(),
            family: info.family.clone(),
            size: text.size.to_pt(),
            weight: info.variant.weight.to_number(),
            italic: info.variant.style != typst::text::FontStyle::Normal,
        });
    }

    fn origin(&mut self, (span, offset): (Span, u16), shown: Option<&str>) -> Origin {
        let resolved = match &self.resolved {
            Some((cached, resolved)) if *cached == span => resolved.clone(),
            _ => {
                let resolved = self.resolve(span);
                self.resolved = Some((span, resolved.clone()));
                resolved
            }
        };
        let (source, file, node, kind) = match resolved {
            Resolved::Unmappable(reason @ (Atomic::Detached | Atomic::ForeignFile)) => {
                return self.enclosing(reason);
            }
            Resolved::Unmappable(reason) => return Origin::Atomic(reason),
            Resolved::Node {
                source,
                file,
                range,
                kind,
            } => (source, file, range, kind),
        };
        match kind {
            NodeKind::Space => Origin::Space { file, range: node },
            NodeKind::Math => Origin::Math { file, range: node },
            NodeKind::Object(kind) => Origin::Object {
                file,
                range: node,
                kind,
            },
            NodeKind::Text | NodeKind::Link => {
                let whole = Origin::Object {
                    file,
                    range: node.clone(),
                    kind: SyntaxKind::Link,
                };
                let link = matches!(kind, NodeKind::Link);
                let Some(shown) = shown else {
                    return if link {
                        whole
                    } else {
                        Origin::Atomic(Atomic::Transformed)
                    };
                };
                // The glyph is only editable if the source really holds, at
                // the position Typst reports, the text the glyph displays.
                // This also rejects offsets that saturated at `u16::MAX`.
                let start = node.start + usize::from(offset);
                let range = start..start + shown.len();
                if range.end <= node.end && source.text().get(range.clone()) == Some(shown) {
                    Origin::Text { file, range }
                } else if link {
                    whole
                } else {
                    Origin::Atomic(Atomic::Transformed)
                }
            }
        }
    }

    /// What a glyph that says nothing of where it comes from is part of:
    /// the innermost element around it that is written in a file of the
    /// project. The text a reference shows has no place in the source,
    /// but the reference has.
    fn enclosing(&mut self, reason: Atomic) -> Origin {
        if let Enclosing::Known(found) = &self.enclosing {
            return found.clone().unwrap_or(Origin::Atomic(reason));
        }
        let mut origin = None;
        for at in (0..self.open.len()).rev() {
            let Resolved::Node {
                file, range, kind, ..
            } = self.resolve(self.open[at].1)
            else {
                continue;
            };
            match kind {
                NodeKind::Object(kind) => origin = Some(Origin::Object { file, range, kind }),
                NodeKind::Link => {
                    let kind = SyntaxKind::Link;
                    origin = Some(Origin::Object { file, range, kind });
                }
                NodeKind::Math => origin = Some(Origin::Math { file, range }),
                NodeKind::Text | NodeKind::Space => continue,
            }
            break;
        }
        self.enclosing = Enclosing::Known(origin.clone());
        origin.unwrap_or(Origin::Atomic(reason))
    }

    fn resolve(&mut self, span: Span) -> Resolved {
        let Some(file) = span.id() else {
            return Resolved::Unmappable(Atomic::Detached);
        };
        if !self.world.is_editable(file) {
            return Resolved::Unmappable(Atomic::ForeignFile);
        }
        let index = match self.indexes.entry(file) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let Ok(source) = self.world.source(file) else {
                    return Resolved::Unmappable(Atomic::ForeignFile);
                };
                entry.insert(SpanIndex::of(source))
            }
        };
        let Some((range, kind, in_equation)) = index.find(span) else {
            return Resolved::Unmappable(Atomic::Detached);
        };
        let kind = match kind {
            _ if in_equation => NodeKind::Math,
            SyntaxKind::Text => NodeKind::Text,
            SyntaxKind::Space => NodeKind::Space,
            SyntaxKind::Link => NodeKind::Link,
            // What these put on the page besides their text is a number
            // or a marker; the cursor has no business there.
            other @ (SyntaxKind::Markup
            | SyntaxKind::Heading
            | SyntaxKind::ListItem
            | SyntaxKind::EnumItem
            | SyntaxKind::TermItem) => return Resolved::Unmappable(Atomic::Markup(other)),
            other => NodeKind::Object(other),
        };
        Resolved::Node {
            source: index.source.clone(),
            file,
            range,
            kind,
        }
    }
}

pub(crate) fn transform_rect(rect: Rect, transform: Transform) -> Rect {
    let corners = [
        rect.min,
        Point::new(rect.max.x, rect.min.y),
        Point::new(rect.min.x, rect.max.y),
        rect.max,
    ]
    .map(|corner| corner.transform(transform));
    let min = corners.into_iter().reduce(Point::min).unwrap_or(rect.min);
    let max = corners.into_iter().reduce(Point::max).unwrap_or(rect.max);
    Rect::new(min, max)
}
