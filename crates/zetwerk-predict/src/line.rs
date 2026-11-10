//! One typeset line, taken apart so that it can be changed and put back.

use typst::layout::{Abs, Em, Frame, FrameItem, Point, Rect};
use typst::syntax::Span;
use typst::text::{Glyph, TextItem};

use crate::shape::{Shaped, reproduces, shape};
use crate::{Decline, map_items};

/// How far a justified line's spaces may shrink, as a fraction of their
/// natural width. Typst's default justification limit.
const SPACE_SHRINK: f64 = 1.0 / 3.0;

/// Lengths closer than this (in points) are equal. Typst positions items
/// with full `f64` precision, so this only has to absorb rounding noise.
const TOLERANCE_PT: f64 = 0.01;

/// Advances closer than this (in em) are equal.
const TOLERANCE_EM: f64 = 1e-7;

/// Items on one baseline belong to the same line if the gap between them is
/// smaller than this many font sizes. Neighbours on a line usually abut
/// exactly, but math leaves small gaps (italic corrections, thin spaces)
/// around itself. Column gutters are wider than this.
const MAX_GAP_EM: f64 = 0.75;

/// A line: the unbroken left-to-right chain of items around the edited one.
pub struct Line {
    members: Vec<Member>,
    baseline: Abs,
    /// Left and right edge of the chain before the edit.
    left: Abs,
    right: Abs,
    /// Whether the line's spaces carry justification.
    justified: bool,
    /// For a ragged line: how far right it may reach, or why that is unknown.
    limit: Result<Abs, Decline>,
}

/// One item of the line.
struct Member {
    /// Index in the parent frame.
    index: usize,
    x: Abs,
    width: Abs,
    /// Distance from the previous member's right edge. Kept as it is.
    gap: Abs,
    new_x: Abs,
    /// `Some` for text we can reshape. `None` for everything that is moved
    /// along but never changed: inline math, boxes, and text whose glyphs
    /// default shaping does not reproduce (math symbols, hyphenation marks).
    run: Option<Run>,
}

/// A text item with its glyphs as the shaper produced them.
struct Run {
    item: TextItem,
    natural: Vec<Shaped>,
    /// How many glyphs at the end are trailing whitespace that Typst gave
    /// zero width because the line breaks there.
    trimmed: usize,
    /// The bytes of the item's text that were typed in this prediction.
    typed: Option<std::ops::Range<usize>>,
}

impl Line {
    /// Finds the line containing item `item_index` of `parent`. The edited
    /// item itself must be reproducible by shaping; its neighbours are
    /// reshaped if they can be and otherwise kept rigid.
    pub fn collect(parent: &Frame, item_index: usize) -> Result<Self, Decline> {
        let items = parent.items().as_slice();
        let Some((origin, FrameItem::Text(edited))) = items.get(item_index) else {
            return Err(Decline::Inconsistent);
        };
        let baseline = origin.y;
        // Sub- and superscripts sit a fraction of the font size off the
        // baseline but are part of the line all the same.
        let reach = edited.size * 0.6;

        // Everything that sits on this baseline, left to right.
        let mut candidates: Vec<(usize, Abs, Abs)> = items
            .iter()
            .enumerate()
            .filter_map(|(index, (pos, item))| {
                let width = match item {
                    FrameItem::Text(text) if (pos.y - baseline).abs() <= reach => text.width(),
                    FrameItem::Group(group)
                        if pos.y <= baseline && baseline <= pos.y + group.frame.height() =>
                    {
                        group.frame.width()
                    }
                    _ => return None,
                };
                Some((index, pos.x, width))
            })
            .collect();
        candidates.sort_by_key(|&(_, x, _)| x);

        // Keep the stretch of candidates that follow each other closely
        // around the edited item. This is what separates a line from, say,
        // the line at the same height in the next column.
        let here = candidates
            .iter()
            .position(|&(index, ..)| index == item_index)
            .ok_or(Decline::Inconsistent)?;
        let max_gap = edited.size * MAX_GAP_EM;
        let abuts =
            |pair: &[(usize, Abs, Abs)]| (pair[1].1 - (pair[0].1 + pair[0].2)).abs() <= max_gap;
        let first = (0..here)
            .rev()
            .take_while(|&i| abuts(&candidates[i..=i + 1]))
            .last()
            .unwrap_or(here);
        let last = (here..candidates.len() - 1)
            .take_while(|&i| abuts(&candidates[i..=i + 1]))
            .last()
            .map_or(here, |i| i + 1);
        let chain = &candidates[first..=last];

        let mut previous_right = chain[0].1;
        let members = chain
            .iter()
            .map(|&(index, x, width)| {
                let run = match &items[index].1 {
                    FrameItem::Text(item) => Run::new(item),
                    _ => None,
                };
                if index == item_index && run.is_none() {
                    return Err(Decline::UnknownShaping);
                }
                let gap = x - previous_right;
                previous_right = x + width;
                Ok(Member {
                    index,
                    x,
                    width,
                    gap,
                    new_x: x,
                    run,
                })
            })
            .collect::<Result<Vec<_>, Decline>>()?;

        let left = chain[0].1;
        let right = chain[chain.len() - 1].1 + chain[chain.len() - 1].2;
        let mut line = Self {
            members,
            baseline,
            left,
            right,
            justified: false,
            limit: Err(Decline::Alignment),
        };
        line.justified = line.detect_justification();
        line.limit = line.ragged_limit(parent);
        Ok(line)
    }

    /// Whether spaces on the line were stretched or shrunk.
    fn detect_justification(&self) -> bool {
        self.members
            .iter()
            .filter_map(|member| member.run.as_ref())
            .any(|run| {
                let kept = run.natural.len() - run.trimmed;
                run.item
                    .glyphs
                    .iter()
                    .zip(&run.natural)
                    .take(kept)
                    .any(|(glyph, natural)| {
                        is_space(&run.item.text, natural) && deviates(glyph, natural)
                    })
            })
    }

    /// How far right a flush-left line may grow.
    fn ragged_limit(&self, parent: &Frame) -> Result<Abs, Decline> {
        let is_member = |index: usize| self.members.iter().any(|m| m.index == index);
        // A long line gets a frame of its own, as wide as the measure and
        // one line tall. Sub- and superscripts sit off the baseline, so the
        // test is the frame's height rather than "every item is a member".
        let line_height = self
            .members
            .iter()
            .filter_map(|member| member.run.as_ref())
            .map(|run| run.item.size)
            .fold(Abs::zero(), Abs::max);
        if parent.kind().is_soft() && parent.height() <= line_height * 2.0 {
            // If the text does not start at the frame's left edge, the line
            // is centred, right-aligned or indented, and we cannot tell
            // which.
            return if close(self.left, Abs::zero()) {
                Ok(parent.width())
            } else {
                Err(Decline::Alignment)
            };
        }
        // A short line is placed directly among its neighbours. Lines that
        // start at the same left edge belong to the same column; the widest
        // of them is the best available bound on the measure.
        parent
            .items()
            .enumerate()
            .filter(|(index, (pos, _))| !is_member(*index) && close(pos.x, self.left))
            .filter_map(|(_, (pos, item))| match item {
                FrameItem::Text(text) => Some(pos.x + text.width()),
                FrameItem::Group(group) if group.frame.kind().is_soft() => {
                    Some(pos.x + group.frame.width())
                }
                _ => None,
            })
            .reduce(Abs::max)
            .ok_or(Decline::Alignment)
    }

    /// Inserts `text` at byte `at` of the text item `item_index`.
    pub fn insert(&mut self, item_index: usize, at: usize, text: &str) -> Result<(), Decline> {
        let run = self
            .members
            .iter_mut()
            .find(|member| member.index == item_index)
            .and_then(|member| member.run.as_mut())
            .ok_or(Decline::Inconsistent)?;

        // Typing after trailing whitespace that was swallowed by the line
        // break puts the text on the next line, or changes the break.
        let kept = run.natural.len() - run.trimmed;
        if run
            .natural
            .get(kept)
            .is_some_and(|trimmed| at > trimmed.range.start)
        {
            return Err(Decline::Overflow);
        }

        let mut new_text = String::with_capacity(run.item.text.len() + text.len());
        new_text.push_str(run.item.text.get(..at).ok_or(Decline::Inconsistent)?);
        new_text.push_str(text);
        new_text.push_str(run.item.text.get(at..).ok_or(Decline::Inconsistent)?);
        if new_text.len() > usize::from(u16::MAX) {
            return Err(Decline::Inconsistent);
        }

        let natural = shape(&run.item, &new_text);
        if natural.iter().any(|glyph| glyph.id == 0) {
            return Err(Decline::MissingGlyph);
        }

        run.item.text = new_text.into();
        run.natural = natural;
        run.typed = Some(at..at + text.len());
        Ok(())
    }

    /// Works out the glyph widths and item positions of the changed line.
    pub fn settle(&mut self) -> Result<(), Decline> {
        if self.justified {
            self.justify()?;
        } else {
            for run in self
                .members
                .iter_mut()
                .filter_map(|member| member.run.as_mut())
            {
                run.set_glyphs(|_| Em::zero());
            }
        }

        let mut x = self.left;
        for member in &mut self.members {
            x += member.gap;
            member.new_x = x;
            x += member
                .run
                .as_ref()
                .map_or(member.width, |run| run.item.width());
        }

        if !self.justified && x > self.limit? + Abs::pt(TOLERANCE_PT) {
            return Err(Decline::Overflow);
        }
        Ok(())
    }

    /// Spreads the line's slack over its spaces so that it keeps its width.
    fn justify(&mut self) -> Result<(), Decline> {
        let mut natural_width = Abs::zero();
        let mut space_width = Abs::zero();
        for member in &self.members {
            natural_width += member.gap;
            let Some(run) = &member.run else {
                natural_width += member.width;
                continue;
            };
            let kept = run.natural.len() - run.trimmed;
            for glyph in &run.natural[..kept] {
                let advance = glyph.x_advance.at(run.item.size);
                natural_width += advance;
                if is_space(&run.item.text, glyph) {
                    space_width += advance;
                }
            }
        }

        let slack = (self.right - self.left) - natural_width;
        if slack < -space_width * SPACE_SHRINK - Abs::pt(TOLERANCE_PT) {
            return Err(Decline::Overflow);
        }
        if space_width.to_pt() <= 0.0 {
            // Nothing to stretch: only fine if there is nothing to absorb.
            return if slack.to_pt().abs() <= TOLERANCE_PT {
                Ok(())
            } else {
                Err(Decline::Overflow)
            };
        }
        // Typst stretches and shrinks spaces in proportion to their width.
        let ratio = slack / space_width;
        for run in self
            .members
            .iter_mut()
            .filter_map(|member| member.run.as_mut())
        {
            let text = run.item.text.clone();
            run.set_glyphs(|glyph| {
                if is_space(&text, glyph) {
                    glyph.x_advance * ratio
                } else {
                    Em::zero()
                }
            });
        }
        Ok(())
    }

    /// The cell of the typed text after [`settle`](Self::settle), in the
    /// parent frame's coordinates: from the pen position before it to the
    /// pen position after it, and from the font's ascender to its descender.
    pub fn typed_cell(&self) -> Option<Rect> {
        let (member, run, typed) = self.members.iter().find_map(|member| {
            let run = member.run.as_ref()?;
            Some((member, run, run.typed.clone()?))
        })?;
        let size = run.item.size;
        let mut pen = member.new_x;
        let mut cell: Option<(Abs, Abs)> = None;
        // Where the typed text would start if it produced no glyph of its
        // own (nothing was typed): after everything that precedes it.
        let mut insertion_point = pen;
        for glyph in &run.item.glyphs {
            let next = pen + glyph.x_advance.at(size);
            // A typed character can merge into a ligature with its
            // neighbour; the ligature is then where the caret belongs.
            if glyph.range().start < typed.end && glyph.range().end > typed.start {
                cell = Some((cell.map_or(pen, |(start, _)| start), next));
            }
            if glyph.range().end <= typed.start {
                insertion_point = next;
            }
            pen = next;
        }
        let (start, end) = cell.unwrap_or((insertion_point, insertion_point));
        let metrics = run.item.font.metrics();
        Some(Rect::new(
            Point::new(start, self.baseline - metrics.ascender.at(size)),
            Point::new(end, self.baseline - metrics.descender.at(size)),
        ))
    }

    /// A copy of `parent` with this line's items replaced and repositioned.
    pub fn apply(self, parent: &Frame) -> Frame {
        let shift_at = |x: Abs| {
            // Decorations and link areas follow the item they sit on.
            self.members
                .iter()
                .rev()
                .find(|member| member.x <= x + Abs::pt(TOLERANCE_PT))
                .map_or(Abs::zero(), |member| member.new_x - member.x)
        };
        let on_line = |pos: Point| {
            pos.x >= self.left - Abs::pt(TOLERANCE_PT)
                && pos.x <= self.right + Abs::pt(TOLERANCE_PT)
        };
        map_items(parent, |index, pos, item| {
            if let Some(member) = self.members.iter().find(|m| m.index == index) {
                let pos = Point::new(member.new_x, pos.y);
                return match &member.run {
                    Some(run) => (pos, FrameItem::Text(run.item.clone())),
                    None => (pos, item.clone()),
                };
            }
            let attached = match item {
                FrameItem::Link(_, size) => {
                    pos.y <= self.baseline && self.baseline <= pos.y + size.y
                }
                FrameItem::Shape(..) => (pos.y - self.baseline).to_pt().abs() < 4.0,
                _ => false,
            };
            if attached && on_line(pos) {
                (Point::new(pos.x + shift_at(pos.x), pos.y), item.clone())
            } else {
                (pos, item.clone())
            }
        })
    }
}

impl Run {
    /// `None` if shaping the item's text does not give back its glyphs, or
    /// if anything but a space has a different width than shaped: then
    /// tracking, an italic correction or a justification mode we do not
    /// model is in play.
    fn new(item: &TextItem) -> Option<Self> {
        let natural = shape(item, &item.text);
        if !reproduces(item, &natural) {
            return None;
        }
        let odd_width = |(glyph, natural): (&Glyph, &Shaped)| {
            !is_space(&item.text, natural)
                && glyph.x_advance.get() != 0.0
                && deviates(glyph, natural)
        };
        if item.glyphs.iter().zip(&natural).any(odd_width) {
            return None;
        }
        let trimmed = item
            .glyphs
            .iter()
            .zip(&natural)
            .rev()
            .take_while(|(glyph, natural)| {
                glyph.x_advance.get() == 0.0 && natural.x_advance.get() > 0.0
            })
            .count();
        Some(Self {
            item: item.clone(),
            natural,
            trimmed,
            typed: None,
        })
    }

    /// Rebuilds the item's glyphs from the shaped ones, widening each by
    /// `extra`. Trimmed trailing whitespace stays at zero width.
    ///
    /// The glyphs carry no source location: Typst renumbers the spans of
    /// every syntax node it reparses, so the spans of the old frame mean
    /// nothing in the edited source. Where predicted glyphs sit in the
    /// source is tracked by the page map, not by the frame.
    fn set_glyphs(&mut self, extra: impl Fn(&Shaped) -> Em) {
        let kept = self.natural.len() - self.trimmed;
        let span = (Span::detached(), 0);
        self.item.glyphs = self
            .natural
            .iter()
            .enumerate()
            .map(|(index, shaped)| {
                let (x_advance, x_offset) = if index < kept {
                    (shaped.x_advance + extra(shaped), shaped.x_offset)
                } else {
                    (Em::zero(), Em::zero())
                };
                Glyph {
                    id: shaped.id,
                    x_advance,
                    x_offset,
                    y_advance: Em::zero(),
                    y_offset: shaped.y_offset,
                    range: saturate(shaped.range.start)..saturate(shaped.range.end),
                    span,
                }
            })
            .collect();
    }
}

/// Whether `glyph` is a space that justification may stretch.
fn is_space(text: &str, glyph: &Shaped) -> bool {
    matches!(text.get(glyph.range.clone()), Some(" " | "\u{a0}"))
}

/// Whether a glyph on the page is wider or narrower than shaped.
fn deviates(glyph: &Glyph, natural: &Shaped) -> bool {
    (glyph.x_advance - natural.x_advance).get().abs() > TOLERANCE_EM
}

fn close(a: Abs, b: Abs) -> bool {
    (a - b).to_pt().abs() < TOLERANCE_PT
}

fn saturate(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}
