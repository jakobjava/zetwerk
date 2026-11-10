//! Predicting what a page looks like after a simple edit, without compiling.
//!
//! Typst has no partial compile: any edit re-runs layout over the whole
//! document, which on a long document costs more than a hundred milliseconds
//! of CPU per keystroke. For the edits that make up most typing — a
//! character inserted into running prose — the visible change is confined to
//! one line. The predictor patches that line in a copy of the page frame, so
//! the editor can show the keystroke immediately and compile only on a pause.
//!
//! A prediction is display-only and may be wrong; the next real compile
//! replaces it. The predictor therefore *declines* whenever it cannot verify
//! that it understands the line it is asked to change.
//!
//! This is level A of the design: the line keeps its breaks. A line that no
//! longer fits is declined, not re-broken.

mod line;
mod shape;

use typst::layout::{Frame, FrameItem, GroupItem, Point, Rect};
use typst::syntax::FileId;
use zetwerk_typst::{MappedGlyph, Origin, PageMap, TextRun};

use crate::line::Line;

/// Text typed at one position of a source file.
#[derive(Debug, Clone, Copy)]
pub struct Insertion<'a> {
    pub file: FileId,
    /// Byte offset in the source *before* the edit.
    pub offset: usize,
    pub text: &'a str,
}

/// Why no prediction was made. Each of these means "compile instead".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Decline {
    /// No glyph on this page shows the text next to the insertion point.
    NotOnPage,
    /// The line is rotated, scaled or otherwise transformed.
    Transformed,
    /// Shaping the edited text with default settings does not reproduce its
    /// glyphs, so it was set with features, tracking or scripts we cannot
    /// see.
    UnknownShaping,
    /// The font of the run has no glyph for the typed text.
    MissingGlyph,
    /// The line would no longer fit its measure and has to be re-broken.
    Overflow,
    /// The line is neither justified nor recognisably flush left, so its
    /// position depends on its width in a way that is not modelled.
    Alignment,
    /// The frame does not have the structure the page map described.
    Inconsistent,
}

/// A predicted page.
pub struct Prediction {
    /// The page frame as it should look after the edit.
    ///
    /// Glyphs on the changed line carry no source location; see
    /// [`Prediction::typed`] for where the caret goes.
    pub frame: Frame,
    /// The cell of the typed text in page coordinates. The caret belongs at
    /// its right edge.
    pub typed: Rect,
}

/// Predicts the page after `insertion`.
///
/// `frame` and `map` describe the page as last compiled, before the edit.
pub fn predict_insertion(
    frame: &Frame,
    map: &PageMap,
    insertion: Insertion,
) -> Result<Prediction, Decline> {
    let target = Target::find(map, insertion).ok_or(Decline::NotOnPage)?;
    if !target.run.transform.is_only_translate() {
        return Err(Decline::Transformed);
    }
    let (&item_index, parent_path) = target.run.path.split_last().ok_or(Decline::Inconsistent)?;
    let parent = descend(frame, parent_path).ok_or(Decline::Inconsistent)?;
    let (item_pos, neighbour) = match parent.items().nth(item_index) {
        Some((pos, FrameItem::Text(item))) => (
            *pos,
            item.glyphs.get(target.glyph).ok_or(Decline::Inconsistent)?,
        ),
        _ => return Err(Decline::Inconsistent),
    };
    let at = if target.after {
        neighbour.range().end
    } else {
        neighbour.range().start
    };

    let mut line = Line::collect(parent, item_index)?;
    line.insert(item_index, at, insertion.text)?;
    line.settle()?;

    // The run's transform places the item's origin on the page; the item's
    // position in its parent tells where the parent's origin is.
    let to_page = Point::new(
        target.run.transform.tx - item_pos.x,
        target.run.transform.ty - item_pos.y,
    );
    let cell = line.typed_cell().ok_or(Decline::Inconsistent)?;
    let typed = Rect::new(cell.min + to_page, cell.max + to_page);
    let frame = replace(frame, parent_path, line.apply(parent));
    Ok(Prediction { frame, typed })
}

/// The glyph next to the insertion point.
struct Target<'a> {
    run: &'a TextRun,
    /// Index of the neighbouring glyph within its text item.
    glyph: usize,
    /// Whether the new text goes after that glyph (else before it).
    after: bool,
}

impl<'a> Target<'a> {
    fn find(map: &'a PageMap, insertion: Insertion) -> Option<Self> {
        let glyphs = map.glyphs();
        let ends_at = |glyph: &MappedGlyph| match &glyph.origin {
            Origin::Text { file, range } => {
                *file == insertion.file && range.end == insertion.offset
            }
            _ => false,
        };
        let starts_at = |glyph: &MappedGlyph| match &glyph.origin {
            Origin::Text { file, range } => {
                *file == insertion.file && range.start == insertion.offset
            }
            _ => false,
        };
        // Typing continues the run of the character before the caret; only
        // at the very start of a run is there nothing before it.
        let (position, after) = match glyphs.iter().rposition(ends_at) {
            Some(position) => (position, true),
            None => (glyphs.iter().position(starts_at)?, false),
        };
        let run = map.runs().get(glyphs.get(position)?.run)?;
        Some(Self {
            run,
            glyph: position.checked_sub(run.glyphs.start)?,
            after,
        })
    }
}

/// Follows `path` (child indices through nested groups) down from `frame`.
fn descend<'a>(frame: &'a Frame, path: &[usize]) -> Option<&'a Frame> {
    path.iter()
        .try_fold(frame, |frame, &index| match frame.items().nth(index) {
            Some((_, FrameItem::Group(group))) => Some(&group.frame),
            _ => None,
        })
}

/// A copy of `frame` in which the frame reached by `path` is `replacement`.
fn replace(frame: &Frame, path: &[usize], replacement: Frame) -> Frame {
    let Some((&index, rest)) = path.split_first() else {
        return replacement;
    };
    let mut replacement = Some(replacement);
    map_items(frame, |at, pos, item| match (item, at == index) {
        (FrameItem::Group(group), true) => {
            let inner = replacement.take().map_or_else(
                || group.frame.clone(),
                |replacement| replace(&group.frame, rest, replacement),
            );
            (
                pos,
                FrameItem::Group(GroupItem {
                    frame: inner,
                    ..group.clone()
                }),
            )
        }
        _ => (pos, item.clone()),
    })
}

/// A copy of `frame` with every item passed through `f`.
fn map_items(
    frame: &Frame,
    mut f: impl FnMut(usize, Point, &FrameItem) -> (Point, FrameItem),
) -> Frame {
    let mut out = Frame::new(frame.size(), frame.kind());
    if frame.has_baseline() {
        out.set_baseline(frame.baseline());
    }
    for (index, (pos, item)) in frame.items().enumerate() {
        let (pos, item) = f(index, *pos, item);
        out.push(pos, item);
    }
    out
}
