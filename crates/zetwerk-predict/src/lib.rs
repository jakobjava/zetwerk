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

use typst::layout::{Abs, Frame, FrameItem, GroupItem, Point, Rect};
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

/// Text of a source file replaced by other text: typed over, or with
/// nothing put in, deleted.
#[derive(Debug, Clone, Copy)]
pub struct Replacement<'a> {
    pub file: FileId,
    /// Byte offset in the source *before* the edit.
    pub offset: usize,
    /// How many bytes from there on are taken out.
    pub removed: usize,
    /// What is put in their place.
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
    /// The line would no longer fit its measure and has to be re-broken,
    /// over more lines than the predictor takes on.
    Overflow,
    /// The paragraph would need one more line than it has.
    Grows,
    /// A line would have to be broken where the predictor cannot break
    /// it: it ends in a hyphen or in something that is not plain text, is
    /// one long word, or the next line begins in another font.
    Break,
    /// The paragraph goes on in another column or on another page.
    Elsewhere,
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
    let replacement = Replacement {
        file: insertion.file,
        offset: insertion.offset,
        removed: 0,
        text: insertion.text,
    };
    predict(frame, map, replacement)
}

/// Predicts the page after `edit`: text taken out of one line, other text
/// put in its place, or both.
///
/// `frame` and `map` describe the page as last compiled, before the edit.
/// What is taken out has to be plain text on one line of the page.
pub fn predict(frame: &Frame, map: &PageMap, edit: Replacement) -> Result<Prediction, Decline> {
    let target = Target::find(map, edit).ok_or(Decline::NotOnPage)?;
    if !target.run.transform.is_only_translate() {
        return Err(Decline::Transformed);
    }
    let (&item_index, parent_path) = target.run.path.split_last().ok_or(Decline::Inconsistent)?;
    let parent = descend(frame, parent_path).ok_or(Decline::Inconsistent)?;
    let (item_pos, item) = match parent.items().nth(item_index) {
        Some((pos, FrameItem::Text(item))) => (*pos, item),
        _ => return Err(Decline::Inconsistent),
    };
    // Where in the item's own text the edit begins and ends.
    let within = |spot: Spot| {
        let glyph = item.glyphs.get(spot.glyph)?;
        Some(if spot.behind {
            glyph.range().end
        } else {
            glyph.range().start + spot.bytes
        })
    };
    let at = within(target.from).ok_or(Decline::Inconsistent)?;
    let until = match target.to {
        Some(spot) => within(spot).ok_or(Decline::Inconsistent)?,
        None => at,
    };
    if until < at {
        return Err(Decline::Inconsistent);
    }

    let mut line = Line::collect(parent, item_index)?;
    line.replace(item_index, at..until, edit.text)?;

    // The run's transform places the item's origin on the page; the item's
    // position in its parent tells where the parent's origin is.
    let to_page = Point::new(
        target.run.transform.tx - item_pos.x,
        target.run.transform.ty - item_pos.y,
    );
    match line.settle() {
        Ok(()) => {}
        // The line is too full: its last words go to the next line.
        Err(Decline::Overflow) => return reflow(frame, parent_path, parent, to_page, line),
        Err(other) => return Err(other),
    }
    // A line that lost text may have room for words of the next line.
    // If that cannot be worked out, the line is left short.
    if edit.removed > edit.text.len()
        && let Ok(prediction) = pull_up(frame, parent_path, parent, to_page, line.clone())
    {
        return Ok(prediction);
    }
    let cell = line.typed_cell().ok_or(Decline::Inconsistent)?;
    let typed = Rect::new(cell.min + to_page, cell.max + to_page);
    let frame = replace(frame, parent_path, line.apply(parent));
    Ok(Prediction { frame, typed })
}

/// The other way round from [`reflow`]: a line that has room takes words
/// from the start of the next line, which then has room itself, and so
/// on down the paragraph. If its last line is left with nothing, the
/// paragraph has a line less and what follows in its column moves up.
fn pull_up(
    page: &Frame,
    parent_path: &[usize],
    parent: &Frame,
    parent_origin: Point,
    mut line: Line,
) -> Result<Prediction, Decline> {
    let (container_path, mut place) = match parent_path.split_last() {
        Some((&index, rest)) if line.has_own_frame(parent) => (rest, Place::Group(index)),
        _ => (parent_path, Place::Loose),
    };
    let container = descend(page, container_path).ok_or(Decline::Inconsistent)?;
    let origin_of = |place: Place| match place {
        Place::Loose => Some(Point::zero()),
        Place::Group(index) => container.items().nth(index).map(|(pos, _)| *pos),
    };
    let container_origin = parent_origin - origin_of(place).ok_or(Decline::Inconsistent)?;
    let put = |out: &Frame, place: Place, line: &Line| match place {
        Place::Loose => line.apply(out),
        Place::Group(index) => map_items(out, |at, pos, item| match item {
            FrameItem::Group(group) if at == index => (
                pos,
                FrameItem::Group(GroupItem {
                    frame: line.apply(&group.frame),
                    ..group.clone()
                }),
            ),
            _ => (pos, item.clone()),
        }),
    };

    let mut out = container.clone();
    let mut typed = None;
    let mut column: Option<(Abs, Abs)> = None;
    for _ in 0..MAX_LINES {
        let origin = origin_of(place).ok_or(Decline::Inconsistent)?;
        let (left, right) = line.span();
        let below = Below {
            baseline: origin.y + line.baseline(),
            left: origin.x + left,
            right: origin.x + right,
            reach: line.font_size() * 2.5,
        };
        let limit = line.right_limit().map(|limit| origin.x + limit);
        column = Some(match (column, limit) {
            (Some((start, end)), limit) => {
                (start.min(below.left), end.max(limit.unwrap_or(below.right)))
            }
            (None, limit) => (below.left, limit.unwrap_or(below.right)),
        });
        // Words of the next line that fit behind this one's.
        let taken = line.room().and_then(|room| {
            let (next_place, mut next) = next_line(container, &below).ok()?;
            let last = !next.continues();
            let words = next.take_front(room, last)?;
            Some((next_place, next, words))
        });
        let mut following = None;
        if let Some((next_place, next, words)) = taken {
            line.append(words)?;
            line.settle()?;
            following = Some((next_place, next));
        }
        if let Some(cell) = line.typed_cell() {
            let to_page = container_origin + origin;
            typed = Some(Rect::new(cell.min + to_page, cell.max + to_page));
        }
        out = put(&out, place, &line);
        let Some((next_place, mut next)) = following else {
            break;
        };
        if next.is_empty() {
            // The paragraph's last line is gone: what follows moves up.
            let next_origin = origin_of(next_place).ok_or(Decline::Inconsistent)?;
            let gone = Below {
                baseline: next_origin.y + next.baseline(),
                ..below
            };
            out = put(&out, next_place, &next);
            let column = column.ok_or(Decline::Inconsistent)?;
            out = close_room(
                &out,
                &gone,
                column,
                gone.baseline - below.baseline,
                container.height(),
            );
            break;
        }
        next.settle()?;
        line = next;
        place = next_place;
    }
    let typed = typed.ok_or(Decline::Inconsistent)?;
    let frame = replace(page, container_path, out);
    Ok(Prediction { frame, typed })
}

/// Moves what follows the line that was at `gone` in its column up by
/// `pitch`, into the room that line left. As in [`make_room`], what
/// follows is what comes below without a large gap, and what stands in
/// the margin below the column stays.
fn close_room(frame: &Frame, gone: &Below, column: (Abs, Abs), pitch: Abs, height: Abs) -> Frame {
    let top_of = |pos: Point, item: &FrameItem| match item {
        FrameItem::Text(text) => pos.y - text.size,
        _ => pos.y,
    };
    let bottom_of = |pos: Point, item: &FrameItem| match item {
        FrameItem::Text(text) => pos.y + text.size * 0.3,
        FrameItem::Group(group) => pos.y + group.frame.height(),
        FrameItem::Link(_, size) => pos.y + size.y,
        _ => pos.y,
    };
    let right_of = |pos: Point, item: &FrameItem| match item {
        FrameItem::Text(text) => pos.x + text.width(),
        FrameItem::Group(group) => pos.x + group.frame.width(),
        FrameItem::Link(_, size) => pos.x + size.x,
        _ => pos.x,
    };
    let top_margin = frame
        .items()
        .filter(|(pos, _)| pos.x <= column.1)
        .map(|(pos, item)| top_of(*pos, item))
        .fold(height, Abs::min)
        .max(Abs::zero());
    let bottom = height - top_margin;
    let mut following: Vec<(usize, Abs, Abs)> = frame
        .items()
        .enumerate()
        .filter_map(|(index, (pos, item))| {
            let top = top_of(*pos, item);
            let lower = match item {
                FrameItem::Text(_) => pos.y > gone.baseline + Abs::pt(1.0),
                _ => top > gone.baseline - Abs::pt(1.0),
            };
            let in_column = right_of(*pos, item) >= column.0 && pos.x <= column.1;
            let in_margin = top >= bottom - Abs::pt(1.0);
            (lower && in_column && !in_margin).then_some((index, top, bottom_of(*pos, item)))
        })
        .collect();
    following.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut moved = Vec::new();
    let mut flow_bottom = gone.baseline;
    for (index, top, bottom_edge) in following {
        if top - flow_bottom > pitch * 2.5 {
            break;
        }
        flow_bottom = flow_bottom.max(bottom_edge);
        moved.push(index);
    }
    map_items(frame, |index, pos, item| {
        if moved.contains(&index) {
            (Point::new(pos.x, pos.y - pitch), item.clone())
        } else {
            (pos, item.clone())
        }
    })
}

/// How many lines a paragraph may be re-broken over before the predictor
/// leaves it to the compiler.
const MAX_LINES: usize = 16;

/// Where a line is in the frame that holds its paragraph.
#[derive(Clone, Copy)]
enum Place {
    /// In a frame of its own, which is this item of the paragraph's frame.
    Group(usize),
    /// Its items lie directly in the paragraph's frame.
    Loose,
}

/// Level B: re-breaks a paragraph from a line that has become too long.
///
/// The line's last words move to the start of the next line, and so on
/// down the paragraph until a line holds what it is given. If the last
/// line cannot, the paragraph gets one more line and what follows it in
/// its column moves down by a line. Lines are broken first-fit, at blanks.
/// That is how Typst breaks ragged text; in justified text it weighs the
/// whole paragraph and hyphenates, so there this is only close to what
/// the compile will show.
///
/// Declines if the paragraph goes on in another column or page, or if
/// there is no room for another line.
fn reflow(
    page: &Frame,
    parent_path: &[usize],
    parent: &Frame,
    parent_origin: Point,
    mut line: Line,
) -> Result<Prediction, Decline> {
    // The frame the paragraph's lines are in, and the first line's place
    // in it.
    let (container_path, mut place) = match parent_path.split_last() {
        Some((&index, rest)) if line.has_own_frame(parent) => (rest, Place::Group(index)),
        _ => (parent_path, Place::Loose),
    };
    let container = descend(page, container_path).ok_or(Decline::Inconsistent)?;
    let origin_of = |place: Place| match place {
        Place::Loose => Some(Point::zero()),
        Place::Group(index) => container.items().nth(index).map(|(pos, _)| *pos),
    };
    let container_origin = parent_origin - origin_of(place).ok_or(Decline::Inconsistent)?;

    let mut out = container.clone();
    let mut typed = None;
    // The column the paragraph is in, as far as its lines show it.
    let mut column: Option<(Abs, Abs)> = None;
    let mut carry = Some(line.shed()?);
    line.settle()?;
    for _ in 0..MAX_LINES {
        let origin = origin_of(place).ok_or(Decline::Inconsistent)?;
        if let Some(cell) = line.typed_cell() {
            let to_page = container_origin + origin;
            typed = Some(Rect::new(cell.min + to_page, cell.max + to_page));
        }
        let (left, right) = line.span();
        let below = Below {
            baseline: origin.y + line.baseline(),
            left: origin.x + left,
            right: origin.x + right,
            reach: line.font_size() * 2.5,
        };
        let limit = line.right_limit().map(|limit| origin.x + limit);
        column = Some(match (column, limit) {
            (Some((start, end)), limit) => {
                (start.min(below.left), end.max(limit.unwrap_or(below.right)))
            }
            (None, limit) => (below.left, limit.unwrap_or(below.right)),
        });
        out = match place {
            Place::Loose => line.apply(&out),
            Place::Group(index) => map_items(&out, |at, pos, item| match item {
                FrameItem::Group(group) if at == index => (
                    pos,
                    FrameItem::Group(GroupItem {
                        frame: line.apply(&group.frame),
                        ..group.clone()
                    }),
                ),
                _ => (pos, item.clone()),
            }),
        };
        let Some(words) = carry.take() else {
            break;
        };
        if words.ends_paragraph {
            // Nothing below to take the words: a line of their own.
            let column = column.ok_or(Decline::Inconsistent)?;
            let pitch = pitch(container, &below, line.font_size()).ok_or(Decline::Grows)?;
            let baseline = below.baseline + pitch;
            out = make_room(&out, &below, column, pitch, container.height())?;
            let mut last = Line::of(words, column.0, baseline, column.1 - column.0);
            last.settle()?;
            if let Some(cell) = last.typed_cell() {
                typed = Some(Rect::new(
                    cell.min + container_origin,
                    cell.max + container_origin,
                ));
            }
            last.put_new(&mut out);
            break;
        }
        let (next_place, mut next) = next_line(container, &below)?;
        next.prepend(words)?;
        match next.settle() {
            Ok(()) => {}
            Err(Decline::Overflow) => {
                carry = Some(next.shed()?);
                next.settle()?;
            }
            Err(other) => return Err(other),
        }
        line = next;
        place = next_place;
    }
    if carry.is_some() {
        return Err(Decline::Overflow);
    }
    let typed = typed.ok_or(Decline::Inconsistent)?;
    let frame = replace(page, container_path, out);
    Ok(Prediction { frame, typed })
}

/// What to look below of for the next line of a paragraph, in the
/// coordinates of the paragraph's frame.
struct Below {
    baseline: Abs,
    left: Abs,
    right: Abs,
    /// How far down the next line may be at most.
    reach: Abs,
}

/// A line found in a frame: where its baseline and its edges are, how it
/// is held, and the index of a text item of it within its own frame.
struct Found {
    baseline: Abs,
    left: Abs,
    right: Abs,
    place: Place,
    text_item: usize,
}

/// Everything in `container` that looks like a line of text, or a part
/// of one.
fn lines_in(container: &Frame) -> Vec<Found> {
    container
        .items()
        .enumerate()
        .filter_map(|(index, (pos, item))| match item {
            FrameItem::Text(text) => Some(Found {
                baseline: pos.y,
                left: pos.x,
                right: pos.x + text.width(),
                place: Place::Loose,
                text_item: index,
            }),
            FrameItem::Group(group) if group.frame.kind().is_soft() => {
                // A line in a frame of its own: its leftmost text.
                let (inner, (at, _)) = group
                    .frame
                    .items()
                    .enumerate()
                    .filter(|(_, (_, item))| matches!(item, FrameItem::Text(_)))
                    .min_by(|(_, (a, _)), (_, (b, _))| a.x.cmp(&b.x))?;
                Some(Found {
                    baseline: pos.y + at.y,
                    left: pos.x,
                    right: pos.x + group.frame.width(),
                    place: Place::Group(index),
                    text_item: inner,
                })
            }
            _ => None,
        })
        .collect()
}

/// Takes a found line apart.
fn collect(container: &Frame, found: &Found) -> Result<Line, Decline> {
    let frame = match found.place {
        Place::Loose => container,
        Place::Group(index) => match container.items().nth(index) {
            Some((_, FrameItem::Group(group))) => &group.frame,
            _ => return Err(Decline::Inconsistent),
        },
    };
    Line::collect(frame, found.text_item)
}

/// The line right below `below` in `container`.
fn next_line(container: &Frame, below: &Below) -> Result<(Place, Line), Decline> {
    let next = lines_in(container)
        .into_iter()
        .filter(|found| {
            found.baseline > below.baseline + Abs::pt(1.0)
                && found.baseline <= below.baseline + below.reach
                && found.right > below.left
                && found.left < below.right
        })
        .min_by(|a, b| {
            let (a, b) = ((a.baseline, a.left), (b.baseline, b.left));
            a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
        })
        .ok_or(Decline::Elsewhere)?;
    Ok((next.place, collect(container, &next)?))
}

/// The distance between the baselines of two lines of one paragraph set
/// in `size`: taken from the line above `below` if that is of the same
/// paragraph, and else from any two such lines in the frame.
fn pitch(container: &Frame, below: &Below, size: Abs) -> Option<Abs> {
    let mut lines = lines_in(container);
    lines.sort_by(|a, b| {
        a.baseline
            .partial_cmp(&b.baseline)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    // Two lines of one paragraph: the upper one ends in a blank that
    // its break swallowed.
    let apart = |upper: &Found, lower: &Found| {
        let apart = lower.baseline - upper.baseline;
        let stacked = apart > Abs::pt(1.0)
            && apart <= below.reach
            && upper.right > lower.left
            && upper.left < lower.right;
        if !stacked {
            return None;
        }
        let line = collect(container, upper).ok()?;
        (line.continues() && (line.font_size() - size).abs() < Abs::pt(0.01)).then_some(apart)
    };
    let here = Found {
        baseline: below.baseline,
        left: below.left,
        right: below.right,
        place: Place::Loose,
        text_item: 0,
    };
    let above = lines
        .iter()
        .rev()
        .filter(|line| line.baseline < below.baseline - Abs::pt(1.0))
        .find_map(|line| apart(line, &here));
    above.or_else(|| {
        lines
            .iter()
            .zip(lines.iter().skip(1))
            .find_map(|(upper, lower)| apart(upper, lower))
    })
}

/// Moves what follows the line at `below` in its column down by `pitch`,
/// to make room for one more line.
///
/// What follows is what comes below without a large gap: after such a
/// gap come things that keep their place on the page, like a footer. What
/// no longer fits the column is left out: it goes to the next column or
/// page, which the compile will show. How far down the column reaches is
/// not written on a page; it is taken to leave as much room at the bottom
/// as there is above the column's first line.
fn make_room(
    frame: &Frame,
    below: &Below,
    column: (Abs, Abs),
    pitch: Abs,
    height: Abs,
) -> Result<Frame, Decline> {
    let extent = |pos: Point, item: &FrameItem| match item {
        FrameItem::Text(text) => (
            pos.y - text.size,
            pos.y + text.size * 0.3,
            pos.x + text.width(),
        ),
        FrameItem::Group(group) => (
            pos.y,
            pos.y + group.frame.height(),
            pos.x + group.frame.width(),
        ),
        FrameItem::Link(_, size) => (pos.y, pos.y + size.y, pos.x + size.x),
        _ => (pos.y, pos.y, pos.x),
    };
    let top_margin = frame
        .items()
        .filter(|(pos, _)| pos.x <= column.1)
        .map(|(pos, item)| extent(*pos, item).0)
        .fold(height, Abs::min)
        .max(Abs::zero());
    let bottom = height - top_margin;
    if below.baseline + pitch > bottom {
        // The new line itself would be the one to go.
        return Err(Decline::Grows);
    }
    let mut following: Vec<(usize, Abs, Abs)> = frame
        .items()
        .enumerate()
        .filter_map(|(index, (pos, item))| {
            let (top, bottom_edge, right) = extent(*pos, item);
            let lower = match item {
                FrameItem::Text(_) => pos.y > below.baseline + Abs::pt(1.0),
                _ => top > below.baseline - Abs::pt(1.0),
            };
            let in_column = right >= column.0 && pos.x <= column.1;
            // What stands in the margin below the column stays there.
            let in_margin = top >= bottom - Abs::pt(1.0);
            (lower && in_column && !in_margin).then_some((index, top, bottom_edge))
        })
        .collect();
    following.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut moved = Vec::new();
    let mut gone = Vec::new();
    let mut flow_bottom = below.baseline;
    for (index, top, bottom_edge) in following {
        if top - flow_bottom > pitch * 2.5 {
            break;
        }
        flow_bottom = flow_bottom.max(bottom_edge);
        if bottom_edge + pitch > bottom + Abs::pt(0.5) {
            gone.push(index);
        } else {
            moved.push(index);
        }
    }
    let mut out = Frame::new(frame.size(), frame.kind());
    if frame.has_baseline() {
        out.set_baseline(frame.baseline());
    }
    for (index, (pos, item)) in frame.items().enumerate() {
        if gone.contains(&index) {
            continue;
        }
        let down = if moved.contains(&index) {
            pitch
        } else {
            Abs::zero()
        };
        out.push(Point::new(pos.x, pos.y + down), item.clone());
    }
    Ok(out)
}

/// Where an edit is on the page.
struct Target<'a> {
    /// The text item it is in.
    run: &'a TextRun,
    /// Where it begins and, if it takes text out, where that ends.
    from: Spot,
    to: Option<Spot>,
}

/// A place in a text item, given by a glyph of it.
#[derive(Clone, Copy)]
struct Spot {
    /// Index of the glyph within its text item.
    glyph: usize,
    /// Whether the place is behind all the glyph stands for, or else
    /// this many bytes into it.
    behind: bool,
    bytes: usize,
}

impl<'a> Target<'a> {
    fn find(map: &'a PageMap, edit: Replacement) -> Option<Self> {
        let glyphs = map.glyphs();
        let text_of = |glyph: &MappedGlyph| match &glyph.origin {
            Origin::Text { file, range } if *file == edit.file => Some(range.clone()),
            _ => None,
        };
        if edit.removed == 0 {
            // Typing continues the run of the character before the caret;
            // only at the very start of a run is there nothing before it.
            let ends_here =
                |glyph: &MappedGlyph| text_of(glyph).is_some_and(|range| range.end == edit.offset);
            let starts_here = |glyph: &MappedGlyph| {
                text_of(glyph).is_some_and(|range| range.start == edit.offset)
            };
            let (position, behind) = match glyphs.iter().rposition(ends_here) {
                Some(position) => (position, true),
                None => (glyphs.iter().position(starts_here)?, false),
            };
            let run = map.runs().get(glyphs.get(position)?.run)?;
            let from = Spot {
                glyph: position.checked_sub(run.glyphs.start)?,
                behind,
                bytes: 0,
            };
            return Some(Self {
                run,
                from,
                to: None,
            });
        }
        // What is taken out: glyphs of one text item that show the bytes
        // one after the other, with nothing else in between.
        let end = edit.offset + edit.removed;
        let first = glyphs
            .iter()
            .position(|glyph| text_of(glyph).is_some_and(|range| range.contains(&edit.offset)))?;
        let run_index = glyphs.get(first)?.run;
        let run = map.runs().get(run_index)?;
        let mut covered = text_of(&glyphs[first])?;
        let mut last = first;
        while covered.end < end {
            let next = glyphs
                .get(last + 1)
                .filter(|glyph| glyph.run == run_index)?;
            let range = text_of(next)?;
            if range.start != covered.end && range != covered {
                return None;
            }
            covered = range;
            last += 1;
        }
        let spot = |position: usize, byte: usize| {
            let range = text_of(&glyphs[position])?;
            Some(Spot {
                glyph: position.checked_sub(run.glyphs.start)?,
                behind: false,
                bytes: byte.checked_sub(range.start)?,
            })
        };
        Some(Self {
            run,
            from: spot(first, edit.offset)?,
            to: Some(spot(last, end)?),
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
