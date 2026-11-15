//! One typeset line, taken apart so that it can be changed and put back.

use typst::layout::{Abs, Em, Frame, FrameItem, GroupItem, Point, Rect, Size};
use typst::syntax::Span;
use typst::text::{Glyph, TextItem};

use crate::Decline;
use crate::shape::{Shaped, reproduces, shape};

/// How far a justified line's spaces may shrink, as a fraction of their
/// natural width. Typst's default justification limit.
const SPACE_SHRINK: f64 = 1.0 / 3.0;

/// How far a justified line's spaces are stretched, as a fraction of
/// their natural width, before the line is rather left short.
const SPACE_STRETCH: f64 = 1.5;

/// How far the blanks of a justified line are taken to give when words
/// are fitted into it, as a fraction of their natural width. Typst lets
/// them shrink by up to a third, but weighs that against the looks of the
/// whole paragraph; of the values tried, a fifth came closest to the
/// breaks it chooses.
const LINE_SHRINK: f64 = 0.2;

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
#[derive(Clone)]
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
    /// Items of the frame that were on the line and have left it for the
    /// next one.
    gone: Vec<usize>,
}

/// One item of the line.
#[derive(Clone)]
struct Member {
    /// Index in the parent frame; `None` for an item that came down from
    /// the line above and is not in the frame yet.
    index: Option<usize>,
    x: Abs,
    width: Abs,
    /// Distance from the previous member's right edge. Kept as it is.
    gap: Abs,
    new_x: Abs,
    /// How far below the line's baseline the item's origin is: nothing
    /// for text on the line, less for superscripts and for boxes, which
    /// hang from their top.
    drop: Abs,
    /// `Some` for text we can reshape. `None` for everything that is moved
    /// along but never changed: inline math, boxes, and text whose glyphs
    /// default shaping does not reproduce (math symbols).
    run: Option<Run>,
    /// The item itself, for those that are not reshaped.
    rigid: Option<FrameItem>,
}

/// A text item with its glyphs as the shaper produced them.
#[derive(Clone)]
struct Run {
    item: TextItem,
    natural: Vec<Shaped>,
    /// How many glyphs at the end are trailing whitespace that Typst gave
    /// zero width because the line breaks there.
    trimmed: usize,
    /// The bytes of the item's text that were typed in this prediction.
    typed: Option<std::ops::Range<usize>>,
    /// The hyphen Typst put behind the text because it broke the line
    /// inside a word.
    hyphen: Option<Glyph>,
}

/// What was taken off the end of a line that no longer holds it, on its
/// way to the start of the next line: words, and whatever stands among
/// them.
pub struct Carry {
    pieces: Vec<Member>,
    /// Whether the line it comes from was the last of its paragraph, so
    /// that it has to make a line of its own.
    pub ends_paragraph: bool,
}

impl Line {
    /// Where the line's baseline is, and its left and right edge as they
    /// were before the edit, in the coordinates of its frame.
    pub fn baseline(&self) -> Abs {
        self.baseline
    }

    pub fn span(&self) -> (Abs, Abs) {
        (self.left, self.right)
    }

    /// The largest font size on the line.
    pub fn font_size(&self) -> Abs {
        self.members
            .iter()
            .filter_map(|member| member.run.as_ref())
            .map(|run| run.item.size)
            .fold(Abs::zero(), Abs::max)
    }

    /// Whether `parent` is a frame Typst made for this line alone, as it
    /// does for lines of more than a few items. Short lines lie directly
    /// among their neighbours.
    pub fn has_own_frame(&self, parent: &Frame) -> bool {
        parent.kind().is_soft() && parent.height() <= self.font_size() * 2.0
    }

    /// Whether the paragraph goes on in a next line: this one ends in a
    /// blank that the line break swallowed, or in a hyphen.
    pub fn continues(&self) -> bool {
        self.members
            .last()
            .and_then(|member| member.run.as_ref())
            .is_some_and(|run| run.trimmed > 0 || run.hyphen.is_some())
    }

    /// How far right the line may reach, in the coordinates of its frame.
    pub fn right_limit(&self) -> Result<Abs, Decline> {
        if self.justified {
            Ok(self.right)
        } else {
            self.limit
        }
    }

    /// Takes whole words off the end of the line until the rest fits, and
    /// hands them over for the next line. A word is what stands between
    /// two blanks, be it text in one font or several, or an equation.
    ///
    /// Declines if nothing of the line could stay.
    pub fn shed(&mut self) -> Result<Carry, Decline> {
        let available = if self.justified {
            self.right - self.left
        } else {
            self.limit? - self.left
        };
        let ends_paragraph = !self.continues();
        // A hyphen is there because the line was broken behind it. The
        // word it is in goes to the next line, where its other half is.
        if let Some(run) = self
            .members
            .last_mut()
            .and_then(|member| member.run.as_mut())
        {
            run.hyphen = None;
        }
        let mut pieces: Vec<Member> = Vec::new();
        loop {
            let (last, before) = self.members.split_last_mut().ok_or(Decline::Break)?;
            let mut fixed = last.gap;
            // How much the blanks that stay may give in a justified line.
            let mut give = Abs::zero();
            for member in before.iter() {
                fixed += member.gap + member.run.as_ref().map_or(member.width, Run::natural_width);
                if let Some(run) = member.run.as_ref().filter(|_| self.justified) {
                    give += run.space_width() * LINE_SHRINK;
                }
            }
            let room = Room {
                fixed,
                give,
                available,
                justified: self.justified,
            };
            let cut = last
                .run
                .as_ref()
                .and_then(|run| run.cut(pieces.is_empty(), before.is_empty(), &room));
            let Some((cut, body, natural)) = cut else {
                // Nothing of this item can stay: all of it goes.
                let mut member = self.members.pop().ok_or(Decline::Break)?;
                self.gone.extend(member.index.take());
                if let Some(run) = &mut member.run {
                    // Its last blank is no longer at the end of a line.
                    run.trimmed = 0;
                }
                pieces.push(member);
                continue;
            };
            let run = last.run.as_mut().ok_or(Decline::Inconsistent)?;
            let text = run.item.text.to_string();
            let typed = run.typed.take();
            let (stays, goes) = match typed {
                Some(typed) if typed.end > cut => {
                    (None, Some(typed.start.saturating_sub(cut)..typed.end - cut))
                }
                typed => (typed, None),
            };
            if cut < text.len() {
                let mut tail = Run {
                    item: run.item.clone(),
                    natural: shape(&run.item, &text[cut..]),
                    trimmed: 0,
                    typed: goes,
                    hyphen: None,
                };
                tail.item.text = text[cut..].into();
                pieces.push(Member {
                    index: None,
                    x: Abs::zero(),
                    width: tail.natural_width(),
                    gap: Abs::zero(),
                    new_x: Abs::zero(),
                    drop: last.drop,
                    run: Some(tail),
                    rigid: None,
                });
            }
            run.typed = stays;
            run.trimmed = natural
                .iter()
                .filter(|glyph| glyph.range.start >= body)
                .count();
            run.natural = natural;
            run.item.text = text[..cut].into();
            break;
        }
        pieces.reverse();
        if let Some(first) = pieces.first_mut() {
            first.gap = Abs::zero();
        }
        Ok(Carry {
            pieces,
            ends_paragraph,
        })
    }

    /// Puts what the line before could not hold at the start of this
    /// line.
    pub fn prepend(&mut self, carry: Carry) -> Result<(), Decline> {
        let mut pieces = carry.pieces;
        // Text that goes on in the same font becomes one with the text
        // it meets, as it was before a line break came between them.
        let joined = match (
            pieces.last().and_then(|piece| piece.run.as_ref()),
            self.members
                .first_mut()
                .and_then(|member| member.run.as_mut()),
        ) {
            (Some(tail), Some(run)) if same_style(&tail.item, &run.item) => {
                let mut text = tail.item.text.to_string();
                text.push_str(&run.item.text);
                if text.len() > usize::from(u16::MAX) {
                    return Err(Decline::Inconsistent);
                }
                let natural = shape(&run.item, &text);
                if natural.iter().any(|glyph| glyph.id == 0) {
                    return Err(Decline::MissingGlyph);
                }
                run.item.text = text.into();
                run.natural = natural;
                run.typed.clone_from(&tail.typed);
                true
            }
            _ => false,
        };
        if joined {
            pieces.pop();
        }
        self.members.splice(0..0, pieces);
        Ok(())
    }

    /// Whether what is left on the line shows nothing: no text but
    /// blanks, and nothing else.
    pub fn shows_nothing(&self) -> bool {
        self.members.iter().all(|member| {
            member
                .run
                .as_ref()
                .is_some_and(|run| run.item.text.trim().is_empty())
        })
    }

    /// Whether nothing is left on the line.
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// How much more the line could hold if it did not end where it
    /// does: its measure less what is on it, counting the blank that the
    /// line break swallowed. `None` if it ends in a hyphen or in
    /// something that is not plain text: nothing can be put behind those.
    pub fn room(&self) -> Option<Abs> {
        let available = if self.justified {
            self.right - self.left
        } else {
            self.limit.ok()? - self.left
        };
        let last = self.members.last()?.run.as_ref()?;
        if last.hyphen.is_some() || last.trimmed == 0 {
            return None;
        }
        let kept = last.natural.len() - last.trimmed;
        let swallowed: Abs = last.natural[kept..]
            .iter()
            .map(|glyph| glyph.x_advance.at(last.item.size))
            .sum();
        let (mut used, mut blanks) = (swallowed, swallowed);
        for member in &self.members {
            used += member.gap + member.run.as_ref().map_or(member.width, Run::natural_width);
            blanks += member.run.as_ref().map_or(Abs::zero(), Run::space_width);
        }
        let give = if self.justified {
            blanks * LINE_SHRINK
        } else {
            Abs::zero()
        };
        Some(available - used + give)
    }

    /// Takes whole words off the start of the line, as many as fit into
    /// `room`, for the end of the line above. All of the line only if it
    /// `may_empty`: if the paragraph ends with it.
    pub fn take_front(&mut self, room: Abs, may_empty: bool) -> Option<Carry> {
        let mut used = Abs::zero();
        let mut pieces: Vec<Member> = Vec::new();
        loop {
            let only = self.members.len() == 1;
            let Some(first) = self.members.first_mut() else {
                break;
            };
            let gap = first.gap;
            let Some(run) = first.run.as_mut() else {
                break;
            };
            let text = run.item.text.to_string();
            let width = |upto: usize| -> Abs {
                shape(&run.item, &text[..upto])
                    .iter()
                    .map(|glyph| glyph.x_advance.at(run.item.size))
                    .sum()
            };
            // The furthest blank behind which the words before it still
            // fit; a blank at the end of the line above is swallowed.
            let mut cut = 0;
            let mut search = 0;
            while let Some(found) = text[search..].find(' ') {
                let behind = search + found + 1;
                search = behind;
                let body = text[..behind].trim_end_matches(' ').len();
                if body == 0 {
                    continue;
                }
                if used + gap + width(body) > room + Abs::pt(TOLERANCE_PT) {
                    break;
                }
                cut = behind;
            }
            // Or all of it, where that is all there is left of the line.
            let all_fits = used + gap + width(text.trim_end_matches(' ').len())
                <= room + Abs::pt(TOLERANCE_PT);
            if only && may_empty && all_fits && !text.trim_end_matches(' ').is_empty() {
                cut = text.len();
            }
            if cut == 0 || (cut == text.len() && only && !may_empty) {
                break;
            }
            if cut == text.len() {
                // The whole item goes up.
                used += gap + width(cut);
                let mut member = self.members.remove(0);
                self.gone.extend(member.index.take());
                if let Some(run) = &mut member.run {
                    run.trimmed = 0;
                }
                pieces.push(member);
                continue;
            }
            let mut head = Run {
                item: run.item.clone(),
                natural: shape(&run.item, &text[..cut]),
                trimmed: 0,
                typed: None,
                hyphen: None,
            };
            head.item.text = text[..cut].into();
            let piece = Member {
                index: None,
                x: Abs::zero(),
                width: head.natural_width(),
                gap,
                new_x: Abs::zero(),
                drop: first.drop,
                run: Some(head),
                rigid: None,
            };
            run.natural = shape(&run.item, &text[cut..]);
            run.item.text = text[cut..].into();
            first.gap = Abs::zero();
            pieces.push(piece);
            break;
        }
        (!pieces.is_empty()).then_some(Carry {
            pieces,
            ends_paragraph: false,
        })
    }

    /// Puts words that came up from the line below at the end of this
    /// line.
    pub fn append(&mut self, carry: Carry) -> Result<(), Decline> {
        let mut pieces = carry.pieces.into_iter().peekable();
        let last = self
            .members
            .last_mut()
            .and_then(|member| member.run.as_mut())
            .ok_or(Decline::Break)?;
        // The blank the line ended in stands between words again.
        last.trimmed = 0;
        if let Some(head) = pieces
            .peek()
            .and_then(|piece| piece.run.as_ref())
            .filter(|head| same_style(&head.item, &last.item))
        {
            let mut text = last.item.text.to_string();
            text.push_str(&head.item.text);
            if text.len() > usize::from(u16::MAX) {
                return Err(Decline::Inconsistent);
            }
            let natural = shape(&last.item, &text);
            if natural.iter().any(|glyph| glyph.id == 0) {
                return Err(Decline::MissingGlyph);
            }
            last.item.text = text.into();
            last.natural = natural;
            pieces.next();
        }
        self.members.extend(pieces);
        // And the blanks the line ends in now are the ones swallowed.
        if let Some(run) = self
            .members
            .last_mut()
            .and_then(|member| member.run.as_mut())
        {
            let body = run.item.text.trim_end_matches(' ').len();
            run.trimmed = run
                .natural
                .iter()
                .filter(|glyph| glyph.range.start >= body)
                .count();
        }
        Ok(())
    }

    /// A line made of what the line before could not hold, with its left
    /// edge at `left` and its baseline at `baseline`, that may be `width`
    /// wide.
    pub fn of(carry: Carry, left: Abs, baseline: Abs, width: Abs) -> Self {
        Self {
            members: carry.pieces,
            baseline,
            left,
            right: left,
            justified: false,
            limit: Ok(left + width),
            gone: Vec::new(),
        }
    }

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
                let (pos, item) = &items[index];
                let run = match item {
                    FrameItem::Text(item) => Run::new(item),
                    _ => None,
                };
                if index == item_index && run.is_none() {
                    return Err(Decline::UnknownShaping);
                }
                let gap = x - previous_right;
                previous_right = x + width;
                Ok(Member {
                    index: Some(index),
                    x,
                    width,
                    gap,
                    new_x: x,
                    drop: pos.y - baseline,
                    rigid: run.is_none().then(|| item.clone()),
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
            gone: Vec::new(),
        };
        line.justified = line.detect_justification();
        line.limit = line.ragged_limit(parent);
        Ok(line)
    }

    /// Whether the line is justified: its spaces were stretched or shrunk.
    pub fn is_justified(&self) -> bool {
        self.justified
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
        let is_member = |index: usize| self.members.iter().any(|m| m.index == Some(index));
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
        // of them, or this line itself, is a lower bound on the measure.
        let widest = parent
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
            .ok_or(Decline::Alignment)?
            .max(self.right);
        // Ragged lines seldom reach the measure, so that bound is short
        // of it. On a page of one column the measure ends as far from the
        // right edge as the text begins from the left one, if the margins
        // are equal, which is the best guess there is. Several lines that
        // begin in the right half of the frame speak of a second column.
        let starts = || {
            parent.items().filter_map(|(pos, item)| match item {
                FrameItem::Text(_) => Some(pos.x),
                FrameItem::Group(group) if group.frame.kind().is_soft() => Some(pos.x),
                _ => None,
            })
        };
        let columns = starts().filter(|x| *x > parent.width() / 2.0).count() >= 3;
        let margin = starts().fold(self.left, Abs::min);
        let mirrored = parent.width() - margin;
        Ok(
            if !columns && !parent.kind().is_soft() && mirrored > widest {
                mirrored
            } else {
                widest
            },
        )
    }

    /// Puts `text` in the place of bytes `range` of the text item
    /// `item_index`.
    pub fn replace(
        &mut self,
        item_index: usize,
        range: std::ops::Range<usize>,
        text: &str,
    ) -> Result<(), Decline> {
        let run = self
            .members
            .iter_mut()
            .find(|member| member.index == Some(item_index))
            .and_then(|member| member.run.as_mut())
            .ok_or(Decline::Inconsistent)?;

        // Typing after trailing whitespace that was swallowed by the line
        // break puts the text on the next line, or changes the break; and
        // taking that whitespace out joins two words across the break.
        let kept = run.natural.len() - run.trimmed;
        if run
            .natural
            .get(kept)
            .is_some_and(|trimmed| range.end > trimmed.range.start)
        {
            return Err(Decline::Overflow);
        }

        let old = &run.item.text;
        let mut new_text = String::with_capacity(old.len() + text.len());
        new_text.push_str(old.get(..range.start).ok_or(Decline::Inconsistent)?);
        new_text.push_str(text);
        new_text.push_str(old.get(range.end..).ok_or(Decline::Inconsistent)?);
        if new_text.len() > usize::from(u16::MAX) {
            return Err(Decline::Inconsistent);
        }

        let natural = shape(&run.item, &new_text);
        if natural.iter().any(|glyph| glyph.id == 0) {
            return Err(Decline::MissingGlyph);
        }

        run.item.text = new_text.into();
        run.natural = natural;
        run.typed = Some(range.start..range.start + text.len());
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
            natural_width += run.natural_width();
            space_width += run.space_width();
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
        if slack > space_width * SPACE_STRETCH {
            // Too little left on the line to spread it over the measure:
            // Typst would bring words up from the next line. Until the
            // compile shows that, the line is set as it comes.
            for run in self
                .members
                .iter_mut()
                .filter_map(|member| member.run.as_mut())
            {
                run.set_glyphs(|_| Em::zero());
            }
            return Ok(());
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

    /// A copy of `parent` with this line's items replaced and repositioned,
    /// those that left the line taken out, and those that came to it put
    /// in.
    pub fn apply(&self, parent: &Frame) -> Frame {
        let shift_at = |x: Abs| {
            // Decorations and link areas follow the item they sit on.
            self.members
                .iter()
                .rev()
                .find(|member| member.index.is_some() && member.x <= x + Abs::pt(TOLERANCE_PT))
                .map_or(Abs::zero(), |member| member.new_x - member.x)
        };
        let on_line = |pos: Point| {
            pos.x >= self.left - Abs::pt(TOLERANCE_PT)
                && pos.x <= self.right + Abs::pt(TOLERANCE_PT)
        };
        let mut out = Frame::new(parent.size(), parent.kind());
        if parent.has_baseline() {
            out.set_baseline(parent.baseline());
        }
        for (index, (pos, item)) in parent.items().enumerate() {
            if self.gone.contains(&index) {
                // Nothing in its place, but something: the lines after
                // this one know their items by their numbers.
                let nothing = GroupItem::new(Frame::soft(Size::zero()));
                out.push(*pos, FrameItem::Group(nothing));
                continue;
            }
            if let Some(member) = self.members.iter().find(|m| m.index == Some(index)) {
                let pos = Point::new(member.new_x, pos.y);
                match &member.run {
                    Some(run) => out.push(pos, FrameItem::Text(run.item.clone())),
                    None => out.push(pos, item.clone()),
                }
                continue;
            }
            let attached = match item {
                FrameItem::Link(_, size) => {
                    pos.y <= self.baseline && self.baseline <= pos.y + size.y
                }
                FrameItem::Shape(..) => (pos.y - self.baseline).to_pt().abs() < 4.0,
                _ => false,
            };
            if attached && on_line(*pos) {
                out.push(Point::new(pos.x + shift_at(pos.x), pos.y), item.clone());
            } else {
                out.push(*pos, item.clone());
            }
        }
        self.put_new(&mut out);
        out
    }

    /// Puts the items that came to this line from the one above into
    /// `frame`.
    pub fn put_new(&self, frame: &mut Frame) {
        for member in self.members.iter().filter(|member| member.index.is_none()) {
            let pos = Point::new(member.new_x, self.baseline + member.drop);
            match (&member.run, &member.rigid) {
                (Some(run), _) => frame.push(pos, FrameItem::Text(run.item.clone())),
                (None, Some(item)) => frame.push(pos, item.clone()),
                (None, None) => {}
            }
        }
    }
}

/// Whether two text items are set alike, so that text of one can go on
/// in the other.
fn same_style(a: &TextItem, b: &TextItem) -> bool {
    a.font == b.font
        && a.size == b.size
        && a.fill == b.fill
        && a.stroke == b.stroke
        && a.lang == b.lang
        && a.region == b.region
}

/// What a line has room for when words are fitted into it.
struct Room {
    /// The width of everything on the line before the text looked at.
    fixed: Abs,
    /// How much the blanks before it may give.
    give: Abs,
    /// The width the line may have.
    available: Abs,
    justified: bool,
}

impl Run {
    /// The latest place in the run's text behind which a line can end so
    /// that what stays fits `room`: behind a blank. Returns the byte the
    /// text is cut at, where what stays ends without its last blanks,
    /// and what stays as shaped.
    ///
    /// For the `first` piece looked at, the blanks at its very end do not
    /// count: ending behind them would move nothing. If the run is
    /// `alone` on its line, something of it has to stay.
    fn cut(&self, first: bool, alone: bool, room: &Room) -> Option<(usize, usize, Vec<Shaped>)> {
        let text = self.item.text.as_str();
        let mut body = if first {
            text.trim_end_matches(' ').len()
        } else {
            text.len()
        };
        loop {
            let cut = text[..body].rfind(' ')? + 1;
            body = text[..cut].trim_end_matches(' ').len();
            if body == 0 && alone {
                return None;
            }
            let natural = shape(&self.item, &text[..cut]);
            let kept = || natural.iter().filter(|glyph| glyph.range.start < body);
            let width: Abs = kept().map(|glyph| glyph.x_advance.at(self.item.size)).sum();
            let spaces: Abs = kept()
                .filter(|glyph| is_space(&text[..cut], glyph))
                .map(|glyph| glyph.x_advance.at(self.item.size))
                .sum();
            let give = if room.justified {
                room.give + spaces * LINE_SHRINK
            } else {
                Abs::zero()
            };
            if room.fixed + width - give <= room.available + Abs::pt(TOLERANCE_PT) {
                return Some((cut, body, natural));
            }
        }
    }

    /// The width of the run's blanks as shaped, without what the line
    /// break swallowed.
    fn space_width(&self) -> Abs {
        let kept = self.natural.len() - self.trimmed;
        self.natural[..kept]
            .iter()
            .filter(|glyph| is_space(&self.item.text, glyph))
            .map(|glyph| glyph.x_advance.at(self.item.size))
            .sum()
    }

    /// The width of the run as shaped, without what the line break
    /// swallowed.
    fn natural_width(&self) -> Abs {
        let kept = self.natural.len() - self.trimmed;
        let hyphen = self
            .hyphen
            .as_ref()
            .map_or(Em::zero(), |glyph| glyph.x_advance);
        self.natural[..kept]
            .iter()
            .map(|glyph| glyph.x_advance.at(self.item.size))
            .sum::<Abs>()
            + hyphen.at(self.item.size)
    }

    /// `None` if shaping the item's text does not give back its glyphs, or
    /// if anything but a space has a different width than shaped: then
    /// tracking, an italic correction or a justification mode we do not
    /// model is in play.
    fn new(item: &TextItem) -> Option<Self> {
        let natural = shape(item, &item.text);
        // One glyph more than the text gives, standing behind all of it:
        // the hyphen of a word broken at the end of its line.
        let hyphen = (item.glyphs.len() == natural.len() + 1)
            .then(|| item.glyphs.last())
            .flatten()
            .filter(|glyph| glyph.range().start >= item.text.trim_end().len())
            .cloned();
        let text_glyphs = item.glyphs.len() - usize::from(hyphen.is_some());
        if !reproduces(&item.glyphs[..text_glyphs], &natural) {
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
            hyphen,
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
            .chain(self.hyphen.clone())
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
