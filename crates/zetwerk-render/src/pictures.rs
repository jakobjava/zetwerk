//! Where a page has pictures: raster images, which a colour transform
//! for reading at night must leave alone (a negative of a photograph is
//! no picture of anything), unlike text and drawings.

use typst::layout::{Frame, FrameItem, Point, Rect, Transform};

/// The boxes of the raster images on `frame`, in its coordinates, each
/// cut to what clips it. Vector graphics are no pictures in this sense:
/// their lines and text are best inverted with the rest of the page.
#[must_use]
pub fn pictures(frame: &Frame) -> Vec<Rect> {
    let mut found = Vec::new();
    collect(frame, Transform::identity(), None, &mut found);
    found
}

fn collect(frame: &Frame, transform: Transform, clip: Option<Rect>, found: &mut Vec<Rect>) {
    for (pos, item) in frame.items() {
        let here = transform.pre_concat(Transform::translate(pos.x, pos.y));
        match item {
            FrameItem::Group(group) => {
                let inner = here.pre_concat(group.transform);
                let clip = if group.clip.is_some() {
                    let own = Rect::from_pos_size(Point::zero(), group.frame.size());
                    let own = bounds(own, inner);
                    Some(clip.map_or(own, |outer| intersection(outer, own)))
                } else {
                    clip
                };
                collect(&group.frame, inner, clip, found);
            }
            FrameItem::Image(_, size, _) => {
                let rect = bounds(Rect::from_pos_size(Point::zero(), *size), here);
                let rect = clip.map_or(rect, |clip| intersection(clip, rect));
                if rect.max.x > rect.min.x && rect.max.y > rect.min.y {
                    found.push(rect);
                }
            }
            _ => {}
        }
    }
}

/// The axis-aligned box around `rect` after `transform`.
fn bounds(rect: Rect, transform: Transform) -> Rect {
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

fn intersection(a: Rect, b: Rect) -> Rect {
    let min = a.min.max(b.min);
    Rect::new(min, a.max.min(b.max).max(min))
}

#[cfg(test)]
mod tests {
    use super::*;
    use typst::layout::Abs;

    #[test]
    fn images_are_found_where_they_are_drawn_and_nothing_else() {
        let source = "#set page(width: 200pt, height: 200pt, margin: 10pt)\n\
             Text and a line.\n#line(length: 50pt)\n\
             #box(inset: 20pt, image(bytes(\"<svg xmlns='http://www.w3.org/2000/svg' \
             width='40' height='30'><rect width='40' height='30'/></svg>\"), width: 40pt))\n";
        let env = zetwerk_typst::Environment::new(zetwerk_typst::FontSet::Embedded);
        let project = zetwerk_typst::Project::in_memory(
            std::sync::Arc::clone(&env),
            std::env::temp_dir(),
            "main.typ",
            source.to_owned(),
        )
        .unwrap_or_else(|| panic!("valid main file name"));
        let document = zetwerk_typst::compile(&project.snapshot())
            .document
            .unwrap_or_else(|| panic!("it compiles"));
        let found = pictures(&document.pages()[0].frame);
        assert_eq!(found.len(), 1, "{found:?}");
        let rect = found[0];
        let width = (rect.max.x - rect.min.x).to_pt();
        assert!((width - 40.0).abs() < 0.01, "{rect:?}");
        assert!(rect.min.x > Abs::pt(25.0), "inside the inset: {rect:?}");
    }
}
