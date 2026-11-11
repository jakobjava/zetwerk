//! Where on the pages a place in the source ended up.
//!
//! This is the coarse direction of the page map: enough to bring the part
//! of the document the cursor is in into view. It looks for the text under
//! the cursor among the glyphs of all pages; if the cursor is in markup or
//! code that puts no glyph of its own on a page, the nearest text does.

use std::collections::HashMap;

use typst::layout::{Frame, FrameItem, Point, Transform};
use typst::syntax::{LinkedNode, Side, Source, Span};
use typst_layout::PagedDocument;

/// How many syntax leaves on either side of the cursor are tried.
const REACH: usize = 48;

/// A point on a page, in points from its top-left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PagePosition {
    /// The page, counting from 0.
    pub page: usize,
    pub x: f64,
    pub y: f64,
}

/// Where the text at byte `offset` of `source` is on the pages of
/// `document`, or the text nearest to it that is on a page at all.
///
/// `source` has to be the very source the document was compiled from: an
/// edit gives its syntax nodes new numbers, and the glyphs carry the old.
#[must_use]
pub fn locate(document: &PagedDocument, source: &Source, offset: usize) -> Option<PagePosition> {
    // The leaves around the cursor, the nearest first.
    let root = LinkedNode::new(source.root());
    let at = root.leaf_at(offset.min(source.text().len()), Side::After)?;
    let mut wanted: HashMap<Span, usize> = HashMap::new();
    wanted.insert(at.span(), 0);
    let (mut after, mut before) = (at.next_leaf(), at.prev_leaf());
    for distance in 1..=REACH {
        if let Some(node) = &after {
            wanted.entry(node.span()).or_insert(distance * 2 - 1);
            after = node.next_leaf();
        }
        if let Some(node) = &before {
            wanted.entry(node.span()).or_insert(distance * 2);
            before = node.prev_leaf();
        }
    }

    let mut best: Option<(usize, PagePosition)> = None;
    for (page, laid_out) in document.pages().iter().enumerate() {
        search(
            &laid_out.frame,
            Transform::identity(),
            &mut |span, point| {
                let Some(&rank) = wanted.get(&span) else {
                    return;
                };
                if best.is_none_or(|(found, _)| rank < found) {
                    let position = PagePosition {
                        page,
                        x: point.x.to_pt(),
                        y: point.y.to_pt(),
                    };
                    best = Some((rank, position));
                }
            },
        );
        if matches!(best, Some((0, _))) {
            break;
        }
    }
    best.map(|(_, position)| position)
}

/// Calls `found` with the span and the page position of the first glyph of
/// every text item in `frame`.
fn search(frame: &Frame, transform: Transform, found: &mut impl FnMut(Span, Point)) {
    for (pos, item) in frame.items() {
        let transform = transform.pre_concat(Transform::translate(pos.x, pos.y));
        match item {
            FrameItem::Group(group) => {
                search(&group.frame, transform.pre_concat(group.transform), found);
            }
            FrameItem::Text(text) => {
                let origin = Point::zero().transform(transform);
                let mut last = None;
                for glyph in &text.glyphs {
                    // One report per run of glyphs from the same node.
                    if last != Some(glyph.span.0) {
                        found(glyph.span.0, origin);
                        last = Some(glyph.span.0);
                    }
                }
            }
            FrameItem::Shape(..)
            | FrameItem::Image(..)
            | FrameItem::Link(..)
            | FrameItem::Tag(_) => {}
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "a failed step should fail the test")]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::{Environment, FontSet, Project, compile};

    #[test]
    fn text_is_found_on_the_page_it_is_set_on() {
        let text = "First page.\n#pagebreak()\nSecond *page* here.\n#pagebreak()\n= Third\n";
        let env = Environment::new(FontSet::Embedded);
        let project = Project::in_memory(env, PathBuf::from("/"), "main.typ", text.to_owned())
            .expect("a valid name");
        let world = project.snapshot();
        let document = compile(&world).document.expect("it compiles");
        let source = project.source(project.main()).expect("the main source");
        let page_of = |needle: &str| {
            let offset = text.find(needle).expect("the needle is in the text");
            locate(&document, source, offset).map(|position| position.page)
        };
        assert_eq!(page_of("First"), Some(0));
        assert_eq!(page_of("page* here"), Some(1));
        assert_eq!(page_of("Third"), Some(2));
        // In code that sets no glyph: the nearest text.
        assert!(page_of("#pagebreak").is_some());
    }
}
