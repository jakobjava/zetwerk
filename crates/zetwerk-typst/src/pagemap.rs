//! The map between glyphs on a page and bytes of source.
//!
//! Typst tags every glyph with the span of the syntax node it came from and
//! an offset into that node. That is enough to find *which node* produced a
//! glyph, but not to know whether typing at that spot is meaningful: the
//! glyph might come from a function's output, a package, a heading number, or
//! text that was transformed on the way to the page. So each glyph is
//! classified here, and only glyphs that verifiably show the exact source
//! bytes they point at count as editable.

use std::ops::Range;

use typst::World;
use typst::layout::{Abs, Frame, FrameItem, Point, Rect, Transform};
use typst::syntax::{FileId, LinkedNode, Source, Span, SyntaxKind};
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

    fn resolve(&self, span: Span) -> Resolved {
        let Some(file) = span.id() else {
            return Resolved::Unmappable(Atomic::Detached);
        };
        if !self.world.is_editable(file) {
            return Resolved::Unmappable(Atomic::ForeignFile);
        }
        let Ok(source) = self.world.source(file) else {
            return Resolved::Unmappable(Atomic::ForeignFile);
        };
        let Some(node) = source.find(span) else {
            return Resolved::Unmappable(Atomic::Detached);
        };
        let kind = match node.kind() {
            _ if in_equation(&node) => NodeKind::Math,
            SyntaxKind::Text => NodeKind::Text,
            SyntaxKind::Space => NodeKind::Space,
            other => return Resolved::Unmappable(Atomic::Markup(other)),
        };
        let range = node.range();
        Resolved::Node {
            source: source.clone(),
            file,
            range,
            kind,
        }
    }
}

fn in_equation(node: &LinkedNode) -> bool {
    let mut current = Some(node);
    while let Some(node) = current {
        if node.kind() == SyntaxKind::Equation {
            return true;
        }
        current = node.parent();
    }
    false
}

/// The axis-aligned box around `rect` after `transform`.
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
