//! An equation that stands by itself stays where it stood while it is
//! typed into.
//!
//! Such an equation is centred: each character typed at its end moved
//! all of it half a character to the left, what was typed already
//! sliding away under the cursor, key after key. Now, where a key is
//! typeset as a part (see `local.rs`), the equation is put back so that
//! it begins where it began on the page before the key: it grows to the
//! right, as a line of text does. Once typing pauses, the document is
//! typeset whole and the equation is centred again, in one step.
//!
//! Off unless asked for (`:set steady`): coming back to the middle only
//! once typing pauses, the equation was found to lag behind the keys.
//!
//! It stays only where it still fits there: between the left and the
//! right edge of the text of the page, and clear of what else stands in
//! its line (its number). Otherwise it is shown centred, as Typst sets
//! it, and stays from there on.
//!
//! What is moved: of the frame that holds all its glyphs (Typst puts
//! the items of an equation into the page itself, or a group of it),
//! the items they are in, and the lines drawn among them (a fraction's,
//! a root's).

use std::ops::Range;

use typst::layout::{Abs, Frame, FrameItem, Point};
use typst::syntax::FileId;
use zetwerk_typst::{Origin, PageMap, ProjectWorld};

use super::{Viewer, editor};
use crate::typst_text;

/// Where an equation is on a page.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Placed {
    /// Its glyphs' extent, in points of the page: across, and down.
    pub(super) left: f64,
    right: f64,
    top: f64,
    bottom: f64,
    /// The frame that holds all of them (a path of child indices, see
    /// [`zetwerk_typst::TextRun::path`]), and its items they are in.
    holder: Vec<usize>,
    items: Vec<usize>,
}

/// Where the equation at bytes `equation` of `file` is on the page of
/// `map`, if any of its glyphs is there.
pub(super) fn equation_on(map: &PageMap, file: FileId, equation: &Range<usize>) -> Option<Placed> {
    let mut extent = [
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ];
    let mut paths: Vec<&[usize]> = Vec::new();
    for glyph in map.glyphs() {
        if !within(&glyph.origin, file, equation) {
            continue;
        }
        let rect = glyph.rect;
        extent[0] = extent[0].min(rect.min.x.to_pt());
        extent[1] = extent[1].max(rect.max.x.to_pt());
        extent[2] = extent[2].min(rect.min.y.to_pt());
        extent[3] = extent[3].max(rect.max.y.to_pt());
        paths.push(&map.runs().get(glyph.run)?.path);
    }
    let first = *paths.first()?;
    // The frame all of them are in: the longest path all their frames'
    // paths begin with.
    let mut holder = first[..first.len().saturating_sub(1)].to_vec();
    for path in &paths {
        let within = &path[..path.len() - 1];
        let same = holder
            .iter()
            .zip(within)
            .take_while(|(a, b)| a == b)
            .count();
        holder.truncate(same);
    }
    let mut items: Vec<usize> = paths
        .iter()
        .filter_map(|path| path.get(holder.len()).copied())
        .collect();
    items.sort_unstable();
    items.dedup();
    let [left, right, top, bottom] = extent;
    Some(Placed {
        left,
        right,
        top,
        bottom,
        holder,
        items,
    })
}

/// Whether a glyph of `origin` is of what is within the dollar signs of
/// the equation at bytes `equation` of `file`: its symbols, and text in
/// it (a word being typed is shown as one). Its number is of all of it.
fn within(origin: &Origin, file: FileId, equation: &Range<usize>) -> bool {
    let (of, range) = match origin {
        Origin::Math { file, range }
        | Origin::Text { file, range }
        | Origin::Space { file, range }
        | Origin::Object { file, range, .. } => (file, range),
        Origin::Atomic(_) => return false,
    };
    *of == file && range.start > equation.start && range.end < equation.end
}

/// Whether the equation at bytes `equation` of `text` stands by itself:
/// with a blank inside each of its dollar signs.
fn stands_alone(text: &str, equation: &Range<usize>) -> bool {
    text.get(equation.clone()).is_some_and(|written| {
        written.len() > 2
            && written[1..].starts_with(char::is_whitespace)
            && written[..written.len() - 1].ends_with(char::is_whitespace)
    })
}

/// Where the frame `path` leads to from `frame` is on the page, if no
/// group on the way is turned or scaled.
fn offset_of(frame: &Frame, path: &[usize]) -> Option<Point> {
    let mut at = Point::zero();
    let mut frame = frame;
    for &index in path {
        let (pos, FrameItem::Group(group)) = frame.items().nth(index)? else {
            return None;
        };
        if !group.transform.is_identity() {
            return None;
        }
        at += *pos;
        frame = &group.frame;
    }
    Some(at)
}

impl Viewer {
    /// `frame`, the page `index` with a part put in, of `world`, with the
    /// equation the cursor is typing in where it stood on that page as
    /// shown before (`shown`, of `was`), where it stands by itself and
    /// still fits there. See the module's comment.
    pub(super) fn steady_equation(
        &mut self,
        index: usize,
        shown: &Frame,
        was: &ProjectWorld,
        frame: Frame,
        world: &ProjectWorld,
    ) -> Frame {
        self.steadied(index, shown, was, &frame, world)
            .unwrap_or(frame)
    }

    fn steadied(
        &mut self,
        index: usize,
        shown: &Frame,
        was: &ProjectWorld,
        frame: &Frame,
        world: &ProjectWorld,
    ) -> Option<Frame> {
        let editor = self.editor.as_ref()?;
        if !self.steady || editor.mode() != editor::Mode::Insert {
            return None;
        }
        let file = editor.file();
        let now = world.written(file).ok()?;
        // Typed into: between its dollar signs, not behind them.
        let cursor = editor.cursor();
        let equation = typst_text::equation_at(&now, cursor)
            .filter(|equation| equation.start < cursor && cursor < equation.end)?;
        if !stands_alone(now.text(), &equation) {
            return None;
        }
        // The same equation in the text the page was shown of: what was
        // typed is in it, so it begins where it began.
        let before = was.written(file).ok()?;
        let then = typst_text::equation_at(&before, equation.start + 1)
            .filter(|then| then.start == equation.start)?;
        let began = match self.maps.get(&index) {
            Some(map) => equation_on(map, file, &then)?,
            None => equation_on(&PageMap::build(was, shown), file, &then)?,
        };
        let map = PageMap::build(world, frame);
        let placed = equation_on(&map, file, &equation)?;
        let dx = began.left - placed.left;
        if dx.abs() < 0.01 {
            return None;
        }
        // Room: within the text of the page, and left of what else is in
        // its line.
        let line = |rect: &typst::layout::Rect| {
            rect.max.y.to_pt() > placed.top && rect.min.y.to_pt() < placed.bottom
        };
        let others = || {
            map.glyphs()
                .iter()
                .filter(|glyph| !within(&glyph.origin, file, &equation))
        };
        let text = others().filter(|glyph| matches!(glyph.origin, Origin::Text { .. }));
        let text_left = text
            .clone()
            .map(|glyph| glyph.rect.min.x.to_pt())
            .fold(f64::INFINITY, f64::min);
        // The right edge of the text: of its longest line, or of what
        // stands at the end of the equation's line (its number).
        let in_line = || others().filter(|glyph| line(&glyph.rect));
        let text_right = text
            .chain(in_line())
            .map(|glyph| glyph.rect.max.x.to_pt())
            .fold(f64::NEG_INFINITY, f64::max);
        let beside = in_line()
            .map(|glyph| glyph.rect.min.x.to_pt())
            .filter(|x| *x >= placed.right)
            .fold(f64::INFINITY, f64::min);
        let (left, right) = (placed.left + dx, placed.right + dx);
        let fits = text_left.is_finite()
            && left >= text_left - 0.01
            && right <= text_right + 0.01
            && right <= beside;
        if !fits {
            return None;
        }
        // The lines drawn among its glyphs go with them.
        let holder = zetwerk_predict::frame_at(frame, &placed.holder)?;
        let origin = offset_of(frame, &placed.holder)?;
        let mut items = placed.items.clone();
        for (at, (pos, item)) in holder.items().enumerate() {
            let x = (origin.x + pos.x).to_pt();
            let y = (origin.y + pos.y).to_pt();
            let among = (placed.left - 1.0..=placed.right + 1.0).contains(&x)
                && (placed.top - 1.0..=placed.bottom + 1.0).contains(&y);
            if matches!(item, FrameItem::Shape(..)) && among {
                items.push(at);
            }
        }
        zetwerk_predict::moved_across(frame, &placed.holder, &items, Abs::pt(dx))
    }
}
