//! Where a figure is on the pages.
//!
//! Typst marks in the frames of the pages where every element it can find
//! again begins and where it ends. What is drawn between the two marks of
//! a figure is the figure: its body and its caption. A figure that floats
//! to the top or the bottom of a page is drawn elsewhere than its marks
//! are; what is drawn there says which element it belongs to, and that
//! element begins between the marks of the figure.
//!
//! Which figure is meant is said by a place in the source: the figure
//! whose call that place is in.

use std::collections::HashSet;
use std::ops::Range;

use typst::World;
use typst::introspection::{Location, Tag};
use typst::layout::{Frame, FrameItem, Point, Rect, Transform};
use typst::model::FigureElem;
use typst::syntax::{FileId, Source};
use typst_layout::PagedDocument;

use crate::ProjectWorld;
use crate::pagemap::transform_rect;

/// How many pages behind the one a figure is written on are looked
/// through for it: a figure that floats may be put off to a later page.
const PAGES_ON: usize = 4;

/// A figure of the document, found on its pages.
#[derive(Debug, Clone, PartialEq)]
pub struct FigureArea {
    /// The page it is on, counting from 0. Of a figure that goes over
    /// several pages, the first.
    pub page: usize,
    /// The part of that page the figure takes up, caption and all: the
    /// box around everything it draws there.
    pub area: Rect,
    /// The bytes of the file, as it is written, that are the figure:
    /// its call, and its label if one stands behind it.
    pub range: Range<usize>,
    /// The name of its label, if it has one.
    pub label: Option<String>,
    /// Which of the document's figures it is, counting from 1 in the
    /// order they are written in. Tables and figures in figures count
    /// too, so this is not the number printed in its caption.
    pub ordinal: usize,
}

/// The figure of `document` that bytes `wanted` of `file`, as it is
/// written, are part of: the figure whose call (with the `#` before it
/// and the label behind it) holds one of them or ends where they begin.
/// The innermost, if figures are in figures. `world` is what `document`
/// was compiled from.
///
/// `None` if the bytes are in no figure, or in one that draws nothing.
/// Only a figure written as a figure in `file` is found; one that a
/// function of the user's makes is written where that function is.
#[must_use]
pub fn figure_area(
    document: &PagedDocument,
    world: &ProjectWorld,
    file: FileId,
    wanted: &Range<usize>,
) -> Option<FigureArea> {
    let mut search = Search {
        world,
        shown: world.source(file).ok()?,
        written: world.written(file).ok()?,
        file,
        wanted: wanted.clone(),
        figures: 0,
        found: Vec::new(),
    };
    for (page, laid_out) in document.pages().iter().enumerate() {
        search.frame(&laid_out.frame, page);
    }
    // The innermost is the one written with the fewest bytes.
    let figure = search
        .found
        .into_iter()
        .min_by_key(|figure| figure.range.len())?;

    let mut measure = Measure {
        figure: figure.location,
        parts: &figure.parts,
        inside: false,
        bounds: None,
    };
    let pages = document.pages().iter().enumerate();
    for (page, laid_out) in pages.skip(figure.page).take(PAGES_ON + 1) {
        measure.frame(&laid_out.frame, Transform::identity(), None, false);
        if let Some(area) = measure.bounds {
            return Some(FigureArea {
                page,
                area,
                range: figure.range,
                label: figure.label,
                ordinal: figure.ordinal,
            });
        }
    }
    None
}

/// A figure whose call the wanted bytes are in.
struct Written {
    location: Location,
    /// The page its beginning is marked on.
    page: usize,
    range: Range<usize>,
    label: Option<String>,
    ordinal: usize,
    /// The figure and the elements that begin in it.
    parts: HashSet<Location>,
    /// Whether the walk is between its marks.
    open: bool,
}

/// The walk over the marks of the pages that finds the figures the
/// wanted bytes are in.
struct Search<'a> {
    world: &'a ProjectWorld,
    /// The file as Typst was given it, which is what its marks are of,
    /// and as it is written, which is what the wanted bytes are of.
    shown: Source,
    written: Source,
    file: FileId,
    wanted: Range<usize>,
    /// How many figures have begun so far.
    figures: usize,
    found: Vec<Written>,
}

impl Search<'_> {
    fn frame(&mut self, frame: &Frame, page: usize) {
        for (_, item) in frame.items() {
            match item {
                FrameItem::Group(group) => self.frame(&group.frame, page),
                FrameItem::Tag(Tag::Start(element, _)) => {
                    let Some(location) = element.location() else {
                        continue;
                    };
                    for figure in self.found.iter_mut().filter(|figure| figure.open) {
                        figure.parts.insert(location);
                    }
                    if element.is::<FigureElem>() {
                        self.figures += 1;
                        self.begin(element, location, page);
                    }
                }
                FrameItem::Tag(Tag::End(location, ..)) => {
                    for figure in &mut self.found {
                        figure.open &= figure.location != *location;
                    }
                }
                FrameItem::Text(..)
                | FrameItem::Shape(..)
                | FrameItem::Image(..)
                | FrameItem::Link(..) => {}
            }
        }
    }

    /// Notes the figure `element` that begins on `page`, if the wanted
    /// bytes are in it.
    fn begin(&mut self, element: &typst::foundations::Content, location: Location, page: usize) {
        let span = element.span();
        if span.id() != Some(self.file) {
            return;
        }
        let Some(call) = self.shown.find(span).map(|node| node.range()) else {
            return;
        };
        let from = self.world.written_at(self.file, call.start);
        let to = self.world.written_at(self.file, call.end);
        let text = self.written.text();
        // The `#` the call is written with, and the label behind it.
        let from = from - usize::from(text.get(..from).is_some_and(|before| before.ends_with('#')));
        let behind = text.get(to..).unwrap_or_default();
        let label = behind.trim_start_matches([' ', '\t']);
        let name = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | ':' | '.');
        let labelled = label
            .strip_prefix('<')
            .map(|label| label.trim_start_matches(name))
            .filter(|rest| rest.starts_with('>'))
            .map(|rest| text.len() - rest.len() + 1);
        let range = from..labelled.unwrap_or(to);
        if range.start > self.wanted.end || self.wanted.start > range.end {
            return;
        }
        self.found.push(Written {
            location,
            page,
            range,
            label: element
                .label()
                .map(|label| label.resolve().as_str().to_owned()),
            ordinal: self.figures,
            parts: HashSet::from([location]),
            open: true,
        });
    }
}

/// The walk over what the pages draw that finds how much of a page one
/// figure takes up. It goes page by page, from the one the figure begins
/// on; `bounds` is of the pages walked so far.
struct Measure<'a> {
    figure: Location,
    parts: &'a HashSet<Location>,
    /// Whether the walk is between the marks of the figure.
    inside: bool,
    bounds: Option<Rect>,
}

impl Measure<'_> {
    /// Walks `frame`, whose corner `transform` puts on the page. Only
    /// what is in `clip` shows; `adopted` says that the frame is drawn
    /// for the figure wherever its marks are.
    fn frame(&mut self, frame: &Frame, transform: Transform, clip: Option<Rect>, adopted: bool) {
        for (pos, item) in frame.items() {
            let transform = transform.pre_concat(Transform::translate(pos.x, pos.y));
            let local = match item {
                FrameItem::Group(group) => {
                    let transform = transform.pre_concat(group.transform);
                    let clip = if group.clip.is_some() {
                        let own = Rect::from_pos_size(Point::zero(), group.frame.size());
                        match intersection(clip, transform_rect(own, transform)) {
                            Some(clip) => Some(clip),
                            // Nothing of it shows.
                            None => continue,
                        }
                    } else {
                        clip
                    };
                    let adopted = adopted
                        || group
                            .parent
                            .is_some_and(|parent| self.parts.contains(&parent.location));
                    self.frame(&group.frame, transform, clip, adopted);
                    continue;
                }
                FrameItem::Tag(Tag::Start(element, _)) => {
                    if element.location() == Some(self.figure) {
                        self.inside = true;
                    }
                    continue;
                }
                FrameItem::Tag(Tag::End(location, ..)) => {
                    if *location == self.figure {
                        self.inside = false;
                    }
                    continue;
                }
                FrameItem::Link(..) => continue,
                FrameItem::Text(text) => {
                    // The line the text is on, and what its glyphs draw
                    // beyond that.
                    let metrics = text.font.metrics();
                    let line = Rect::new(
                        Point::new(typst::layout::Abs::zero(), -metrics.ascender.at(text.size)),
                        Point::new(text.width(), -metrics.descender.at(text.size)),
                    );
                    let ink = text.bbox();
                    let finite = [ink.min.x, ink.min.y, ink.max.x, ink.max.y]
                        .iter()
                        .all(|length| length.to_raw().is_finite());
                    if finite {
                        // Typst leaves the corners the wrong way up.
                        union(line, Rect::new(ink.min.min(ink.max), ink.min.max(ink.max)))
                    } else {
                        line
                    }
                }
                FrameItem::Shape(shape, _) => shape.bbox(true),
                FrameItem::Image(_, size, _) => Rect::from_pos_size(Point::zero(), *size),
            };
            if !(self.inside || adopted) {
                continue;
            }
            let on_page = transform_rect(local, transform);
            let Some(shows) = intersection(clip, on_page) else {
                continue;
            };
            self.bounds = Some(self.bounds.map_or(shows, |bounds| union(bounds, shows)));
        }
    }
}

/// The box around `a` and `b`.
fn union(a: Rect, b: Rect) -> Rect {
    Rect::new(a.min.min(b.min), a.max.max(b.max))
}

/// What of `rect` is in `clip`, if any of it is; all of it where
/// nothing clips.
fn intersection(clip: Option<Rect>, rect: Rect) -> Option<Rect> {
    let Some(clip) = clip else {
        return Some(rect);
    };
    let (min, max) = (rect.min.max(clip.min), rect.max.min(clip.max));
    (min.x <= max.x && min.y <= max.y).then(|| Rect::new(min, max))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::{Environment, FontSet, Project, compile};

    /// The pages of `text` and what they were compiled from.
    fn compiled(text: &str) -> (PagedDocument, ProjectWorld, FileId) {
        let env = Environment::new(FontSet::Embedded);
        let project = Project::in_memory(env, PathBuf::from("/"), "main.typ", text.to_owned())
            .unwrap_or_else(|| panic!("valid main file name"));
        let world = project.snapshot();
        let document = compile(&world)
            .document
            .unwrap_or_else(|| panic!("the document compiles"));
        (document, world, project.main())
    }

    /// The figure at the first `needle` in `text`.
    fn at(text: &str, needle: &str) -> Option<FigureArea> {
        let (document, world, file) = compiled(text);
        let offset = text
            .find(needle)
            .unwrap_or_else(|| panic!("{needle} is in the text"));
        figure_area(&document, &world, file, &(offset..offset))
    }

    fn size(figure: &FigureArea) -> (f64, f64) {
        let size = figure.area.size();
        (size.x.to_pt(), size.y.to_pt())
    }

    const TEXT: &str = "#set page(width: 300pt, height: 400pt, margin: 20pt)\n\
                        Before the figure.\n\n\
                        #figure(\n  rect(width: 120pt, height: 60pt),\n  caption: [A box],\n) <fig:box>\n\n\
                        After the figure.\n\
                        #pagebreak()\n\
                        #figure(rect(width: 40pt, height: 30pt))\n\
                        Last words.\n";

    #[test]
    fn a_figure_is_found_with_its_caption_and_nothing_else() {
        for needle in ["#figure(\n", "120pt", "A box", "<fig:box>", "ig:box>"] {
            let figure = at(TEXT, needle).unwrap_or_else(|| panic!("a figure at {needle}"));
            assert_eq!((figure.page, figure.ordinal), (0, 1), "at {needle}");
            assert_eq!(figure.label.as_deref(), Some("fig:box"));
            let (width, height) = size(&figure);
            // The box, and the caption below it.
            assert!((120.0..140.0).contains(&width), "{width} wide at {needle}");
            assert!((70.0..100.0).contains(&height), "{height} high at {needle}");
            // Neither the line before it nor the line behind it.
            let top = figure.area.min.y.to_pt();
            assert!(top > 30.0 && top + height < 150.0, "from {top}");
        }
        let written = &TEXT[at(TEXT, "A box")
            .map(|figure| figure.range)
            .unwrap_or_default()];
        assert!(written.starts_with("#figure(") && written.ends_with("<fig:box>"));
    }

    #[test]
    fn text_around_a_figure_is_in_none() {
        assert_eq!(at(TEXT, "Before"), None);
        assert_eq!(at(TEXT, "fter the figure"), None);
        assert_eq!(at(TEXT, "Last"), None);
    }

    #[test]
    fn a_figure_on_a_later_page_is_found_there() {
        let figure = at(TEXT, "40pt").unwrap_or_else(|| panic!("the second figure"));
        assert_eq!((figure.page, figure.ordinal, &figure.label), (1, 2, &None));
        let (width, height) = size(&figure);
        assert!((40.0..42.0).contains(&width) && (30.0..32.0).contains(&height));
    }

    #[test]
    fn of_figures_in_figures_the_innermost_is_meant() {
        let text = "#set page(width: 300pt, height: 400pt)\n\
                    #figure(\n  grid(columns: 2, gutter: 10pt,\n    \
                    figure(rect(width: 50pt, height: 20pt), caption: [left]),\n    \
                    figure(rect(width: 60pt, height: 20pt), caption: [right]),\n  ),\n  \
                    caption: [Both of them],\n)\n";
        let both = at(text, "Both").unwrap_or_else(|| panic!("the outer figure"));
        let right = at(text, "60pt").unwrap_or_else(|| panic!("the inner figure"));
        assert_eq!((both.ordinal, right.ordinal), (1, 3));
        assert!(size(&right).0 < 70.0, "only the right one");
        assert!(size(&both).0 > 115.0, "both and the gutter");
        assert!(both.area.min.x <= right.area.min.x && both.area.max.y >= right.area.max.y);
    }

    #[test]
    fn a_figure_that_floats_is_found_where_it_is_drawn() {
        let text = "#set page(width: 300pt, height: 400pt, margin: 20pt)\n\
                    First words.\n\n\
                    #figure(\n  rect(width: 100pt, height: 50pt),\n  \
                    caption: [Up there],\n  placement: bottom,\n)\n\n\
                    More words.\n";
        let figure = at(text, "Up there").unwrap_or_else(|| panic!("the floating figure"));
        assert_eq!(figure.page, 0);
        let (width, height) = size(&figure);
        assert!((100.0..130.0).contains(&width), "{width} wide");
        assert!((60.0..90.0).contains(&height), "{height} high");
        assert!(
            figure.area.min.y.to_pt() > 250.0,
            "at the bottom of the page"
        );
    }
}
