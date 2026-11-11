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
use typst::layout::{Abs, Frame, FrameItem, Point, Rect, Transform};
use typst::syntax::{FileId, Source, Span, SyntaxKind, SyntaxNode};
use typst::text::TextItem;

use crate::ProjectWorld;

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
    /// Not editable on the page. Selected and handled as a unit.
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
    /// The glyph comes from syntax that is not plain text: a function call,
    /// a reference, a heading marker, a shorthand, an escape sequence.
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
        };
        builder.frame(frame, Transform::identity());
        Self {
            runs: builder.runs,
            glyphs: builder.glyphs,
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
        let mut behind = None;
        for glyph in &self.glyphs {
            let Some(range) = glyph.origin.range_in(file) else {
                continue;
            };
            if range.start == offset {
                // The glyph that begins here: nothing fits better.
                return Some(Caret {
                    cell: glyph.rect,
                    after: false,
                });
            }
            if range.contains(&offset) {
                inside.get_or_insert(glyph.rect);
            } else if range.end == offset {
                behind = Some(glyph.rect);
            }
        }
        inside
            .map(|cell| Caret { cell, after: false })
            .or(behind.map(|cell| Caret { cell, after: true }))
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
                Origin::Space { file, range } | Origin::Math { file, range } => {
                    (*file, range, false)
                }
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
            let cell = glyph.rect;
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
        rects
    }
}

/// Where a cursor is on a page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Caret {
    /// The cell of a glyph, in page coordinates.
    pub cell: Rect,
    /// Whether the cursor is behind that glyph (at the end of the text it
    /// shows) and not on it.
    pub after: bool,
}

impl Origin {
    /// The bytes of `file` this glyph stands for, if it stands for any.
    fn range_in(&self, file: FileId) -> Option<&Range<usize>> {
        match self {
            Self::Text { file: of, range }
            | Self::Space { file: of, range }
            | Self::Math { file: of, range }
                if *of == file =>
            {
                Some(range)
            }
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

impl SpanIndex {
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
    let in_equation = in_equation || node.kind() == SyntaxKind::Equation;
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
    indexes: HashMap<FileId, SpanIndex>,
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
    Space,
    Math,
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
                FrameItem::Shape(..)
                | FrameItem::Image(..)
                | FrameItem::Link(..)
                | FrameItem::Tag(_) => {}
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
            });
        }
        self.runs.push(TextRun {
            path: self.path.clone(),
            transform,
            glyphs: first..self.glyphs.len(),
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
            NodeKind::Text => {
                let Some(shown) = shown else {
                    return Origin::Atomic(Atomic::Transformed);
                };
                // The glyph is only editable if the source really holds, at
                // the position Typst reports, the text the glyph displays.
                // This also rejects offsets that saturated at `u16::MAX`.
                let start = node.start + usize::from(offset);
                let range = start..start + shown.len();
                if range.end <= node.end && source.text().get(range.clone()) == Some(shown) {
                    Origin::Text { file, range }
                } else {
                    Origin::Atomic(Atomic::Transformed)
                }
            }
        }
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
                entry.insert(SpanIndex::new(source))
            }
        };
        let Some((range, kind, in_equation)) = index.find(span) else {
            return Resolved::Unmappable(Atomic::Detached);
        };
        let kind = match kind {
            _ if in_equation => NodeKind::Math,
            SyntaxKind::Text => NodeKind::Text,
            SyntaxKind::Space => NodeKind::Space,
            other => return Resolved::Unmappable(Atomic::Markup(other)),
        };
        Resolved::Node {
            source: index.source.clone(),
            file,
            range,
            kind,
        }
    }
}

fn transform_rect(rect: Rect, transform: Transform) -> Rect {
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
