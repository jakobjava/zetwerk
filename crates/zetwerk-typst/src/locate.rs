//! Where on the pages a place in the source ended up.
//!
//! This is the coarse direction of the page map: enough to bring the part
//! of the document the cursor is in into view. It looks for the text under
//! the cursor among the glyphs of all pages; if the cursor is in markup or
//! code that puts no glyph of its own on a page, the nearest text does.

use std::collections::HashMap;
use std::ops::Range;

use typst::layout::{Frame, FrameItem, Point, Transform};
use typst::syntax::{LinkedNode, Side, Source, Span};
use typst_layout::Page;

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

/// Where the text at byte `offset` of `source` is on `pages`, or the
/// text nearest to it that is on a page at all.
///
/// `source` has to be the very source the pages were compiled from: an
/// edit gives its syntax nodes new numbers, and the glyphs carry the old.
///
/// The search begins at page `near` and goes outwards from it, and ends
/// at the first page that shows the very place: a cursor is mostly near
/// where it was, and of text that is on many pages (a heading and its
/// running title) the nearest is the one meant.
#[must_use]
pub fn locate(pages: &[Page], source: &Source, offset: usize, near: usize) -> Option<PagePosition> {
    locate_near(pages, source, offset, near, usize::MAX)
}

/// [`locate`], looking no further than `reach` pages from page `near`
/// either way: what is typed is near the cursor, and text not on any
/// page (just typed) is not looked for through all of a long document.
#[must_use]
pub fn locate_near(
    pages: &[Page],
    source: &Source,
    offset: usize,
    near: usize,
    reach: usize,
) -> Option<PagePosition> {
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

    // The text under the cursor may be one leaf that is set on many lines
    // and pages (a paragraph of plain words is): of its pieces, the one
    // that shows the cursor's place, or the one nearest to that.
    let within = offset.saturating_sub(at.offset());
    let mut best: Option<((usize, usize), PagePosition)> = None;
    let near = near.min(pages.len().saturating_sub(1));
    // `near`, the page after it, the page before it, two after it, ...
    let outwards = (0..pages.len().min(reach.saturating_add(1))).flat_map(|distance| {
        let after = near
            .checked_add(distance)
            .filter(|page| *page < pages.len());
        let before = near.checked_sub(distance).filter(|_| distance > 0);
        after.into_iter().chain(before)
    });
    for page in outwards {
        let Some(laid_out) = pages.get(page) else {
            continue;
        };
        search(
            &laid_out.frame,
            Transform::identity(),
            &mut |span, shown: Range<usize>, point| {
                let Some(&rank) = wanted.get(&span) else {
                    return;
                };
                let miss = if rank > 0 {
                    0
                } else if within < shown.start {
                    shown.start - within
                } else {
                    within.saturating_sub(shown.end)
                };
                let rank = (rank, miss);
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
        if matches!(best, Some(((0, 0), _))) {
            break;
        }
    }
    best.map(|(_, position)| position)
}

/// Calls `found` for every stretch of glyphs from one syntax node in the
/// text items of `frame`: with the node's span, the bytes of the node the
/// stretch shows (from the first byte of its first glyph to that of its
/// last) and the page position of the item.
fn search(frame: &Frame, transform: Transform, found: &mut impl FnMut(Span, Range<usize>, Point)) {
    for (pos, item) in frame.items() {
        let transform = transform.pre_concat(Transform::translate(pos.x, pos.y));
        match item {
            FrameItem::Group(group) => {
                search(&group.frame, transform.pre_concat(group.transform), found);
            }
            FrameItem::Text(text) => {
                let origin = Point::zero().transform(transform);
                let mut stretch: Option<(Span, Range<usize>)> = None;
                for glyph in &text.glyphs {
                    let (span, at) = (glyph.span.0, usize::from(glyph.span.1));
                    match &mut stretch {
                        Some((of, shown)) if *of == span => {
                            *shown = shown.start.min(at)..shown.end.max(at);
                        }
                        _ => {
                            if let Some((of, shown)) = stretch.replace((span, at..at)) {
                                found(of, shown, origin);
                            }
                        }
                    }
                }
                if let Some((of, shown)) = stretch {
                    found(of, shown, origin);
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
            locate(document.pages(), source, offset, 0).map(|position| position.page)
        };
        assert_eq!(page_of("First"), Some(0));
        assert_eq!(page_of("page* here"), Some(1));
        assert_eq!(page_of("Third"), Some(2));
        // In code that sets no glyph: the nearest text.
        assert!(page_of("#pagebreak").is_some());
    }

    #[test]
    fn text_is_looked_for_no_further_than_asked() {
        let text = "First.\n#pagebreak()\nSecond.\n#pagebreak()\nThird.\n#pagebreak()\nFourth.\n";
        let env = Environment::new(FontSet::Embedded);
        let project = Project::in_memory(env, PathBuf::from("/"), "main.typ", text.to_owned())
            .expect("a valid name");
        let world = project.snapshot();
        let document = compile(&world).document.expect("it compiles");
        let source = project.source(project.main()).expect("the main source");
        let offset = text.find("Fourth").expect("the needle is in the text");
        let page = |reach| {
            locate_near(document.pages(), source, offset, 0, reach).map(|position| position.page)
        };
        assert_eq!(page(usize::MAX), Some(3));
        assert_eq!(page(3), Some(3));
        // Not that far: the nearest text on the pages looked at, or none.
        assert_ne!(page(1), Some(3));
        assert_eq!(
            locate(document.pages(), source, offset, 0).map(|position| position.page),
            Some(3)
        );
    }

    #[test]
    fn a_paragraph_over_several_pages_is_found_where_the_cursor_is() {
        // Plain words are one leaf of the syntax tree, however many pages
        // they fill.
        let words: Vec<String> = (0..200).map(|i| format!("w{i}")).collect();
        let text = format!(
            "#set page(width: 200pt, height: 100pt)\n{}\n",
            words.join(" ")
        );
        let env = Environment::new(FontSet::Embedded);
        let project = Project::in_memory(env, PathBuf::from("/"), "main.typ", text.clone())
            .expect("a valid name");
        let world = project.snapshot();
        let document = compile(&world).document.expect("it compiles");
        assert!(document.pages().len() > 3);
        let source = project.source(project.main()).expect("the main source");
        let mut last = (0, 0.0);
        for word in ["w0 ", "w40 ", "w80 ", "w120 ", "w160 ", "w199"] {
            let offset = text.find(word).expect("the word is in the text");
            // Wherever the search begins.
            let from_end = locate(document.pages(), source, offset, usize::MAX);
            let position = locate(document.pages(), source, offset, 0).expect("it is on a page");
            assert_eq!(from_end, Some(position));
            let now = (position.page, position.y);
            assert!(now > last, "{word} at {now:?} after {last:?}");
            last = now;
        }
        assert_eq!(last.0, document.pages().len() - 1);
    }
}
