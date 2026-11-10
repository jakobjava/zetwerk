//! Cropping a page frame to a region, dropping everything outside it.

use typst::layout::{Abs, Frame, FrameItem, GroupItem, Point, Rect, Transform};

/// Slack in points added around every item's bounds before testing it
/// against the region. Covers anti-aliasing and glyph outlines that reach
/// slightly beyond their reported bounding box.
const MARGIN_PT: f64 = 1.0;

/// Returns a frame of `region`'s size holding only the items of `frame` that
/// can touch `region`, moved so that the region's corner is the origin.
///
/// Culling is conservative: an item is dropped only when its bounds, grown by
/// [`MARGIN_PT`], lie fully outside the region.
pub fn crop(frame: &Frame, region: Rect) -> Frame {
    let mut out = Frame::hard(region.size());
    collect(frame, Point::zero(), region, &mut out);
    out
}

/// The bounds of everything `frame` draws, in the frame's own coordinates.
/// `None` if it draws nothing. Content may lie outside the frame's nominal
/// size, which is why this exists.
#[must_use]
pub fn content_bounds(frame: &Frame) -> Option<Rect> {
    frame
        .items()
        .filter_map(|(pos, item)| item_bounds(*pos, item))
        .reduce(union)
}

fn collect(frame: &Frame, offset: Point, region: Rect, out: &mut Frame) {
    for (pos, item) in frame.items() {
        let pos = offset + *pos;
        if let FrameItem::Group(group) = item
            && is_plain(group)
        {
            // A group that neither transforms nor clips is just a container,
            // so look inside instead of keeping or dropping it wholesale.
            // Page content is mostly nested this way.
            collect(&group.frame, pos, region, out);
            continue;
        }
        if item_bounds(pos, item).is_some_and(|bounds| intersects(bounds, region)) {
            out.push(pos - region.min, item.clone());
        }
    }
}

fn is_plain(group: &GroupItem) -> bool {
    group.transform.is_identity() && group.clip.is_none()
}

/// Bounds of one item placed at `pos`. `None` for items that draw nothing.
fn item_bounds(pos: Point, item: &FrameItem) -> Option<Rect> {
    let local = match item {
        FrameItem::Text(text) => {
            let bbox = text.bbox();
            // An item without outlined glyphs (only spaces, say) reports an
            // infinite box.
            if !(bbox.min.x.to_raw().is_finite() && bbox.max.x.to_raw().is_finite()) {
                return None;
            }
            // Typst flips the box from the font's y-up system by negating
            // both corners, which leaves `min.y` below `max.y`.
            Rect::new(bbox.min.min(bbox.max), bbox.min.max(bbox.max))
        }
        FrameItem::Shape(shape, _) => shape.bbox(true),
        FrameItem::Image(_, size, _) => Rect::from_pos_size(Point::zero(), *size),
        FrameItem::Group(group) => transform_rect(content_bounds(&group.frame)?, group.transform),
        FrameItem::Link(..) | FrameItem::Tag(_) => return None,
    };
    Some(Rect::new(local.min + pos, local.max + pos))
}

/// The axis-aligned box around `rect` after `transform`.
fn transform_rect(rect: Rect, transform: Transform) -> Rect {
    if transform.is_identity() {
        return rect;
    }
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

fn union(a: Rect, b: Rect) -> Rect {
    Rect::new(a.min.min(b.min), a.max.max(b.max))
}

fn intersects(bounds: Rect, region: Rect) -> bool {
    let margin = Abs::pt(MARGIN_PT);
    bounds.min.x - margin <= region.max.x
        && bounds.max.x + margin >= region.min.x
        && bounds.min.y - margin <= region.max.y
        && bounds.max.y + margin >= region.min.y
}
