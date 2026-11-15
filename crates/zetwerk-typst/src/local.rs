//! Typesetting a part of a document by itself.
//!
//! Typst has no way to typeset less than everything: a letter typed into
//! an equation costs a walk through every page, about a millisecond for
//! each. For a few pages that is nothing; for a thesis it is the third
//! of a second that makes typing feel wrong.
//!
//! What changed is small, though: a paragraph, an equation, a heading.
//! So the document is cut down to what is before everything (its rules),
//! the part that changed, and the paragraph on either side of it; that is
//! typeset — by Typst itself, so that it is what Typst would make of it
//! — and the part is put into the page in place of what stood there. The
//! paragraphs beside it are what it is placed by: they stand on the page
//! already, and how far the part is from them is as Typst set it. What
//! is below moves up or down. Numbers and page breaks are not right
//! until all of it has been typeset again, which happens when typing
//! pauses; everything else is.
//!
//! The parts of the text are told apart by the numbers Typst gives the
//! nodes of its syntax tree. They grow along the tree, and a node keeps
//! its number while it is not parsed anew: what the page shows of the
//! text that did not change is found in the changed text by the same
//! numbers, and what was typeset by itself fits among it.

use std::collections::HashSet;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

use typst::foundations::Label;
use typst::introspection::{Introspector, Location, Tag};
use typst::layout::{Abs, Frame, FrameItem, GroupItem, Point};
use typst::syntax::{FileId, Source, Span, SyntaxKind};
use typst::utils::PicoStr;
use typst::visualize::Geometry;
use typst_layout::{Page, PagedDocument};

/// What a lead puts in to find where the text of a page ends: placed at
/// the foot of the text, it takes no room and shows nothing.
pub const FOOT_MARK: &str = "#place(bottom)[#metadata(none) <zetwerk-foot>]\n";

/// How far down its page the text of `typeset` (a part with
/// [`FOOT_MARK`] in its lead) may reach.
#[must_use]
pub fn foot_of_text(typeset: &PagedDocument) -> Option<Abs> {
    let label = Label::new(PicoStr::intern("zetwerk-foot"))?;
    let introspector = typeset.introspector();
    let mark = introspector.query_label(label).ok()?.location()?;
    Some(introspector.position(mark)?.point.y)
}

/// The numbers of the nodes of a stretch of text: from the first on,
/// before the second.
type Numbers = (u64, u64);

/// The part of a text that changed, with all of the paragraphs (headings,
/// lists, …) the change touches, and the paragraph on either side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    /// The file it is of.
    pub file: FileId,
    /// Its bytes in the text as it is now.
    pub bytes: Range<usize>,
    /// The bytes of it and the paragraphs beside it.
    pub kept: Range<usize>,
    /// The numbers of what stood there before, and of what does now.
    was: Numbers,
    now: Numbers,
    /// The numbers of the paragraph before it and of the one behind it,
    /// where there are such: the same before and now.
    before: Option<Numbers>,
    behind: Option<Numbers>,
    /// Where what is before everything (the rules) ends, and the number
    /// below which a node is before all that is kept: what comes of the
    /// rules (a number before a heading, what a reference shows where
    /// it is typeset by itself) is of no paragraph.
    rules: usize,
    floor: u64,
}

/// Why a part could not be typeset by itself and put into a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The change is in what is before everything: that changes
    /// everything.
    InRules,
    /// Nothing that was there before is in the part: there is no place
    /// to put it.
    NothingToStandFor,
    /// The page shows nothing of the part.
    NotOnThisPage,
    /// Other things stand among the part on the page (a figure that
    /// floats, a footnote), or it was typeset onto several pages.
    NotInOnePiece,
    /// The part would reach into the margin below the text of the page:
    /// some of it goes to the next page, which is for all of it typeset
    /// at once to show.
    BreaksOtherwise,
    /// Cut away from the rest, the part is read otherwise than among it
    /// (a `*` not closed, a list marker on the line of a rule), or Typst
    /// read it anew and numbered its nodes otherwise: what is typeset of
    /// it cannot be told apart.
    ReadOtherwise,
}

/// A span's number within its file.
fn number(span: Span) -> u64 {
    span.into_raw().get() & ((1 << 48) - 1)
}

/// A child of the root: its number, its length and its kind.
type Child = (u64, usize, SyntaxKind);

/// The children of the root of `source`.
fn children(source: &Source) -> Vec<Child> {
    source
        .root()
        .children()
        .map(|child| (number(child.span()), child.len(), child.kind()))
        .collect()
}

/// Whether a child of the root is part of what is before everything:
/// rules, definitions, imports, and the blanks and comments among them.
fn is_rule(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::Hash
            | SyntaxKind::SetRule
            | SyntaxKind::ShowRule
            | SyntaxKind::LetBinding
            | SyntaxKind::ModuleImport
            | SyntaxKind::Space
            | SyntaxKind::Parbreak
            | SyntaxKind::LineComment
            | SyntaxKind::BlockComment
            | SyntaxKind::Semicolon
    )
}

/// How many children `before` and `now` begin with in common, and how
/// many of the rest they end with in common.
fn in_common(before: &[Child], now: &[Child]) -> (usize, usize) {
    let same = |(a, b): &(&Child, &Child)| a.0 == b.0 && a.1 == b.1;
    let head = before.iter().zip(now).take_while(same).count();
    let rest = before.len().min(now.len()) - head;
    let tail = before
        .iter()
        .rev()
        .zip(now.iter().rev())
        .take(rest)
        .take_while(same)
        .count();
    (head, tail)
}

/// The part of `new` that is not what `old` was, widened to whole
/// paragraphs. `changed` are the bytes of `new` that are known to be
/// new; the nodes Typst parsed anew are found by their numbers.
///
/// # Errors
///
/// If the change is in the rules before everything, or there is nothing
/// it could take the place of.
pub fn changed_region(
    old: &Source,
    new: &Source,
    changed: &Range<usize>,
) -> Result<Region, Refusal> {
    let (before, now) = (children(old), children(new));
    let (head, tail) = in_common(&before, &now);
    // Where each child begins.
    let mut starts = Vec::with_capacity(now.len() + 1);
    let mut at = 0;
    for child in &now {
        starts.push(at);
        at += child.1;
    }
    starts.push(at);
    // The children the changed bytes are in.
    let first = starts
        .partition_point(|start| *start <= changed.start)
        .saturating_sub(1);
    let last = starts
        .partition_point(|start| *start < changed.end.max(changed.start + 1))
        .min(now.len());
    let mut from = head.min(first);
    let mut to = (now.len() - tail).max(last).max(from);
    let is_break = |index: usize| now[index].2 == SyntaxKind::Parbreak;
    let rules = now
        .iter()
        .position(|child| !is_rule(child.2))
        .unwrap_or(now.len());
    // The hash before a call is of the call.
    let rules = rules - usize::from(rules > 0 && now[rules - 1].2 == SyntaxKind::Hash);
    if from < rules {
        return Err(Refusal::InRules);
    }
    // Whole paragraphs: out to the breaks between them.
    while from > rules && !is_break(from - 1) {
        from -= 1;
    }
    while to < now.len() && !is_break(to) {
        to += 1;
    }
    // What stood there before: the same children before and behind.
    let stood = from..before.len().saturating_sub(now.len() - to);
    if stood.is_empty() || from >= to {
        return Err(Refusal::NothingToStandFor);
    }
    let end = |index: usize| now.get(index).map_or(u64::MAX, |child| child.0);
    // The paragraph before, and the one behind.
    let mut earlier = from.saturating_sub(1);
    while earlier > rules && !is_break(earlier - 1) {
        earlier -= 1;
    }
    let has_before = from > rules && earlier < from - 1;
    let mut later = (to + 1).min(now.len());
    while later < now.len() && !is_break(later) {
        later += 1;
    }
    let has_behind = to + 1 < later;
    let kept_from = if has_before { earlier } else { from };
    // With the break before it, so that it begins as a paragraph does.
    let kept_from = if kept_from > rules {
        kept_from - 1
    } else {
        kept_from
    };
    Ok(Region {
        file: new.id(),
        bytes: starts[from]..starts[to],
        kept: starts[kept_from]..starts[if has_behind { later } else { to }],
        was: (before[from].0, end(to)),
        now: (now[from].0, end(to)),
        before: has_before.then(|| (now[earlier].0, now[from - 1].0)),
        behind: has_behind.then(|| (now[to + 1].0, end(later))),
        rules: starts[rules],
        floor: now[kept_from].0,
    })
}

impl Region {
    /// The region without the paragraphs beside it: for when those
    /// cannot be typeset by themselves.
    #[must_use]
    pub fn alone(mut self, new: &Source) -> Self {
        // The break before it stays with it.
        let text = new.text();
        let breaks = text[..self.bytes.start].len() - text[..self.bytes.start].trim_end().len();
        self.kept = (self.bytes.start - breaks).max(self.rules)..self.bytes.end;
        self.before = None;
        self.behind = None;
        self
    }

    /// The byte of the whole text that byte `at` of the text cut down
    /// to this region (with a lead of `lead` bytes) is; `None` if it is
    /// in the rules or the lead.
    #[must_use]
    pub fn in_whole(&self, at: usize, lead: usize) -> Option<usize> {
        // The lead is a paragraph of its own (see `cut_down`).
        let kept = at.checked_sub(self.rules + 2 + lead)?;
        Some(self.kept.start.max(self.rules) + kept)
    }

    /// Whether `span` is of a node with one of `numbers`.
    fn has(&self, numbers: Numbers, span: Span) -> Option<bool> {
        if span.is_detached() {
            return None;
        }
        let number = number(span);
        // What stood there before may be numbered below what is kept now:
        // Typst numbers what it reads anew between what it did not.
        if span.id() == Some(self.file) && number < self.floor.min(numbers.0) {
            return None;
        }
        Some(span.id() == Some(self.file) && numbers.0 <= number && number < numbers.1)
    }

    /// How much of what `item` shows comes of the nodes with `numbers`,
    /// and how much of other places of the text: glyphs, shapes and
    /// images, counted.
    fn count(&self, numbers: Numbers, item: &FrameItem, of: &mut (usize, usize)) {
        let mut see = |has: Option<bool>| match has {
            Some(true) => of.0 += 1,
            Some(false) => of.1 += 1,
            None => {}
        };
        match item {
            FrameItem::Group(group) => {
                for (_, inner) in group.frame.items() {
                    self.count(numbers, inner, of);
                }
            }
            FrameItem::Text(text) => {
                for glyph in &text.glyphs {
                    see(self.has(numbers, glyph.span.0));
                }
            }
            FrameItem::Shape(_, span) | FrameItem::Image(_, _, span) => {
                see(self.has(numbers, *span));
            }
            FrameItem::Link(..) | FrameItem::Tag(_) => {}
        }
    }

    /// Whether what `item` shows is of the nodes with `numbers`; `None`
    /// if it shows nothing that comes of the text.
    ///
    /// Most of it has to be: a line of a paragraph holds what comes of
    /// other places too (what a reference shows comes of the heading it
    /// refers to), and is a line of its paragraph all the same.
    fn shows(&self, numbers: Numbers, item: &FrameItem) -> Option<bool> {
        let mut of = (0, 0);
        self.count(numbers, item, &mut of);
        (of != (0, 0)).then_some(of.0 > of.1)
    }

    /// The items of `frame` that show the nodes with `numbers`: from
    /// which to which, and the top and the bottom of what they take up.
    /// All between them have to be of them too.
    fn run(&self, numbers: Numbers, frame: &Frame) -> Result<Option<Run>, Refusal> {
        let mut run: Option<Run> = None;
        let mut others = 0;
        // The elements begun and not yet ended, innermost last, and
        // whether each is of the nodes: what shows nothing of the text
        // (the number of an item of a list, of a heading) is of the
        // element it is in.
        let mut open: Vec<(Location, bool)> = Vec::new();
        // The marks of the elements of the nodes that begin right before
        // the first item and end right behind the last go with them:
        // where the first of them is, how far the last reaches, and which
        // elements they are.
        let (mut lead, mut tail, mut ending) = (None, None, false);
        let mut inside: HashSet<Location> = HashSet::new();
        for (index, (position, item)) in frame.items().enumerate() {
            // What is below the line above the footnotes is not looked
            // at: the notes stay as they are.
            if is_rule_across(item) {
                break;
            }
            let shows = match item {
                FrameItem::Tag(_) => None,
                _ => self
                    .shows(numbers, item)
                    .or_else(|| open.last().is_some_and(|(_, of)| *of).then_some(true)),
            };
            let before = open.len();
            self.follow(numbers, item, &mut open);
            match item {
                FrameItem::Tag(Tag::Start(element, _)) if open.len() > before => {
                    let of = open.last().is_some_and(|(_, of)| *of);
                    if of {
                        inside.extend(element.location());
                        if run.is_none() {
                            lead.get_or_insert(index);
                        }
                    } else if run.is_none() {
                        (lead, inside) = (None, HashSet::new());
                    }
                }
                FrameItem::Tag(Tag::End(location, ..)) => {
                    if run.is_some() && ending && inside.contains(location) {
                        tail = Some(index + 1);
                    } else if run.is_none() && !inside.contains(location) {
                        (lead, inside) = (None, HashSet::new());
                    }
                }
                FrameItem::Tag(_) => {}
                _ if shows == Some(true) => ending = true,
                _ => {
                    ending = false;
                    if run.is_none() {
                        (lead, inside) = (None, HashSet::new());
                    }
                }
            }
            match shows {
                Some(true) => {
                    let (top, bottom) = extent(*position, item);
                    run = Some(match run {
                        None => Run {
                            items: lead.unwrap_or(index)..index + 1,
                            top,
                            bottom,
                        },
                        Some(_) if others > 0 => return Err(Refusal::NotInOnePiece),
                        Some(run) => Run {
                            items: run.items.start..index + 1,
                            top: run.top.min(top),
                            bottom: run.bottom.max(bottom),
                        },
                    });
                    tail = None;
                }
                Some(false) if run.is_some() => others += 1,
                _ => {}
            }
        }
        if let (Some(run), Some(tail)) = (run.as_mut(), tail) {
            run.items.end = tail;
        }
        Ok(run)
    }

    /// `open` (the elements begun and not ended, and whether each is of
    /// the nodes with `numbers`) after `item`, at any depth: an equation
    /// may begin on one line and end in the group of the next.
    fn follow(&self, numbers: Numbers, item: &FrameItem, open: &mut Vec<(Location, bool)>) {
        match item {
            FrameItem::Tag(Tag::Start(element, _)) => {
                if let Some(location) = element.location() {
                    // One of no place in the text (a mark for PDF readers)
                    // is of the element it is in.
                    let of = self
                        .has(numbers, element.span())
                        .unwrap_or_else(|| open.last().is_some_and(|(_, of)| *of));
                    open.push((location, of));
                }
            }
            FrameItem::Tag(Tag::End(location, ..)) => {
                if let Some(at) = open.iter().rposition(|(open, _)| open == location) {
                    open.remove(at);
                }
            }
            FrameItem::Group(group) => {
                for (_, inner) in group.frame.items() {
                    self.follow(numbers, inner, open);
                }
            }
            _ => {}
        }
    }

    /// Whether `frame` shows anything of what stood where the region is
    /// before it changed.
    #[must_use]
    pub fn is_on(&self, frame: &Frame) -> bool {
        !matches!(self.run(self.was, frame), Ok(None))
    }

    /// The item of `frame` that shows the paragraph before the region
    /// last (or, `behind`, the one that shows the paragraph behind it
    /// first), among the items `among`: which it is, and how far down.
    fn beside(&self, frame: &Frame, among: Range<usize>, behind: bool) -> Option<(usize, Abs)> {
        let numbers = if behind { self.behind? } else { self.before? };
        let mut found = None;
        let items = frame
            .items()
            .enumerate()
            .skip(among.start)
            .take(among.len());
        for (index, (position, item)) in items {
            if self.shows(numbers, item) == Some(true) {
                found = Some((index, position.y));
                if behind {
                    break;
                }
            }
        }
        found
    }
}

/// The items that show something, and how far down the page they reach.
struct Run {
    items: Range<usize>,
    top: f64,
    bottom: f64,
}

/// `new` cut down to its rules and what is kept of `region`, with `lead`
/// (more rules, for this purpose) between them. The nodes that are kept
/// keep their numbers.
///
/// # Errors
///
/// If what is kept is not read as it is in `new`, node for node and
/// number for number: Typst reads a stretch anew where something in it
/// is not closed, and numbers it anew; or it is read otherwise by itself.
pub fn cut_down(new: &Source, region: &Region, lead: &str) -> Result<Source, Refusal> {
    let mut cut = new.clone();
    let length = cut.text().len();
    if region.kept.end < length {
        cut.edit(region.kept.end..length, "");
    }
    // The lead begins a line of its own: so its first node is never
    // taken by Typst for the first of what is kept, which would then be
    // numbered anew. And it ends a paragraph: what is kept begins one, as
    // it does behind the rules, and not behind a blank of the lead's.
    cut.edit(
        region.rules..region.kept.start.max(region.rules),
        &format!("\n{lead}\n"),
    );
    // The children of the root that are kept, from the first that is
    // not a break (the break before them joins the lead's last line).
    let mut kept = Vec::new();
    let mut at = 0;
    for child in new.root().children() {
        let end = at + child.len();
        if at >= region.kept.start.max(region.rules) && end <= region.kept.end {
            let blank = matches!(child.kind(), SyntaxKind::Parbreak | SyntaxKind::Space);
            if !(kept.is_empty() && blank) {
                kept.push((at, child));
            }
        }
        at = end;
    }
    let Some(&(first, _)) = kept.first() else {
        return Ok(cut);
    };
    // They end the cut, the same and at the same places.
    let from = cut.text().len() - (region.kept.end - first);
    let mut at = 0;
    let mut tail = Vec::with_capacity(kept.len());
    for child in cut.root().children() {
        if at >= from {
            tail.push((at - from + first, child));
        }
        at += child.len();
    }
    if tail != kept {
        return Err(Refusal::ReadOtherwise);
    }
    Ok(cut)
}

/// From where to where the box of `item` at `position` reaches down the
/// page, in points: of a line of text that stands by itself, from the
/// height of its capitals to its baseline, which is how Typst measures
/// lines unless a document says otherwise.
fn extent(position: Point, item: &FrameItem) -> (f64, f64) {
    let y = position.y.to_pt();
    match item {
        FrameItem::Group(group) => (y, y + group.frame.height().to_pt()),
        FrameItem::Text(text) => {
            let metrics = text.font.metrics();
            (y - metrics.cap_height.at(text.size).to_pt(), y)
        }
        FrameItem::Shape(shape, _) => {
            let bounds = shape.geometry.bbox(shape.stroke.as_ref());
            (y + bounds.min.y.to_pt(), y + bounds.max.y.to_pt())
        }
        FrameItem::Image(_, size, _) | FrameItem::Link(_, size) => (y, y + size.y.to_pt()),
        FrameItem::Tag(_) => (y, y),
    }
}

/// How far right of `item` the first text in it begins, at any depth.
fn first_text(item: &FrameItem) -> Option<Abs> {
    match item {
        FrameItem::Text(_) => Some(Abs::zero()),
        FrameItem::Group(group) => group
            .frame
            .items()
            .find_map(|(at, inner)| Some(at.x + first_text(inner)?)),
        _ => None,
    }
}

/// The element whose beginning or end `item` marks, if it marks one.
fn tagged(item: &FrameItem) -> Option<Location> {
    match item {
        FrameItem::Tag(Tag::Start(element, _)) => element.location(),
        FrameItem::Tag(Tag::End(location, ..)) => Some(*location),
        _ => None,
    }
}

/// `item` with the places of the elements it marks the beginning or the
/// end of (at any depth) told apart by `salt`, and without the marks of
/// those that are not `closed`; `None` if it is such a mark.
fn relocated(item: &FrameItem, salt: u128, closed: &HashSet<Location>) -> Option<FrameItem> {
    let moved = |location: Location| Location::new(location.hash() ^ salt);
    if tagged(item).is_some_and(|element| !closed.contains(&element)) {
        return None;
    }
    Some(match item {
        FrameItem::Tag(Tag::Start(element, flags)) => {
            let mut element = element.clone();
            if let Some(location) = element.location() {
                element.set_location(moved(location));
            }
            FrameItem::Tag(Tag::Start(element, *flags))
        }
        FrameItem::Tag(Tag::End(location, key, flags)) => {
            FrameItem::Tag(Tag::End(moved(*location), *key, *flags))
        }
        FrameItem::Group(group) => {
            let mut frame = Frame::new(group.frame.size(), group.frame.kind());
            if group.frame.has_baseline() {
                frame.set_baseline(group.frame.baseline());
            }
            for (position, inner) in group.frame.items() {
                if let Some(inner) = relocated(inner, salt, closed) {
                    frame.push(*position, inner);
                }
            }
            FrameItem::Group(GroupItem {
                frame,
                ..group.clone()
            })
        }
        other => other.clone(),
    })
}

/// The elements that begin and end among `items`, at any depth: an
/// equation may begin on one line and end in the group of the next.
fn closed_among<'a>(items: impl Iterator<Item = &'a FrameItem>) -> HashSet<Location> {
    fn walk(item: &FrameItem, begun: &mut HashSet<Location>, closed: &mut HashSet<Location>) {
        match item {
            FrameItem::Tag(Tag::Start(element, _)) => {
                begun.extend(element.location());
            }
            FrameItem::Tag(Tag::End(location, ..)) if begun.contains(location) => {
                closed.insert(*location);
            }
            FrameItem::Group(group) => {
                for (_, inner) in group.frame.items() {
                    walk(inner, begun, closed);
                }
            }
            _ => {}
        }
    }
    let (mut begun, mut closed) = (HashSet::new(), HashSet::new());
    for item in items {
        walk(item, &mut begun, &mut closed);
    }
    closed
}

/// The marks among `item` at `position` (at any depth) of the elements
/// that are not `closed`, where they are on the page.
fn open_marks(
    position: Point,
    item: &FrameItem,
    closed: &HashSet<Location>,
    out: &mut Vec<(Point, FrameItem)>,
) {
    match item {
        FrameItem::Tag(_) if tagged(item).is_some_and(|element| !closed.contains(&element)) => {
            out.push((position, item.clone()));
        }
        FrameItem::Group(group) if group.transform.is_identity() => {
            for (inner, item) in group.frame.items() {
                open_marks(position + *inner, item, closed, out);
            }
        }
        FrameItem::Group(group) => {
            for (_, item) in group.frame.items() {
                open_marks(position, item, closed, out);
            }
        }
        _ => {}
    }
}

/// Whether `item` is a line across the page: what Typst draws above the
/// footnotes, which stay where they are.
fn is_rule_across(item: &FrameItem) -> bool {
    // Typst's own line, not one the document draws: it comes of no
    // place in the text.
    matches!(item, FrameItem::Shape(shape, span)
        if span.is_detached()
            && matches!(shape.geometry, Geometry::Line(to) if to.y.to_pt().abs() < 0.01))
}

/// How near the top of something has to be to the bottom of the part
/// to count as below it, in points: lines of a page overlap a little.
const BELOW: f64 = 1.0;

/// How many parts have been put into pages: what tells the places of the
/// elements of each apart.
static SPLICES: AtomicU64 = AtomicU64::new(1);

/// `page` with what it shows of `region` replaced by what `typeset`
/// (the pages made of the text cut down to the region) shows of it. What
/// is below it on the page moves by as much as Typst set it further
/// down or up; `stays(index)` says of an item of the page that it does
/// not move (the page number, a running head). What is pushed below
/// `foot`, where the text of the page ends, is left out: it goes to the
/// next page.
///
/// # Errors
///
/// If the page shows nothing of the region, or it is not in one piece
/// there or in `typeset`; or if it would reach below `foot` itself.
pub fn splice(
    page: &Frame,
    typeset: &[Page],
    region: &Region,
    foot: Abs,
    stays: &dyn Fn(usize) -> bool,
) -> Result<Frame, Refusal> {
    let old = region
        .run(region.was, page)?
        .ok_or(Refusal::NotOnThisPage)?;
    // What was typeset, on the one page it is on.
    let mut made = None;
    for typeset in typeset {
        if let Some(run) = region.run(region.now, &typeset.frame)? {
            if made.is_some() {
                return Err(Refusal::NotInOnePiece);
            }
            made = Some((&typeset.frame, run));
        }
    }
    let (from, new) = made.ok_or(Refusal::NotInOnePiece)?;
    let count = page.items().count();

    // How far down what was typeset goes on the page: as far from the
    // paragraph before it as it was typeset; or, where there is none on
    // this page, to where its top was.
    let before = region
        .beside(page, 0..old.items.start, false)
        .zip(region.beside(from, 0..new.items.start, false));
    let down = match before {
        Some(((_, on_page), (_, typeset))) => on_page - typeset,
        None => Abs::pt(old.top - new.top),
    };
    // And what is below it moves with the paragraph behind it; or by as
    // much as the part grew.
    let behind = region
        .beside(page, old.items.end..count, true)
        .zip(region.beside(from, new.items.end..usize::MAX, true));
    // The paragraph behind begins where it did: what it follows may
    // change how it is set (indented behind a paragraph, not behind a
    // list), and it is not put in.
    if let Some(((on_page, _), (typeset, _))) = behind {
        let x = |frame: &Frame, index: usize| {
            let (at, item) = frame.items().nth(index)?;
            Some(at.x + first_text(item)?)
        };
        if x(page, on_page) != x(from, typeset) {
            return Err(Refusal::ReadOtherwise);
        }
    }
    let (moved_from, moved_by) = if let Some(((index, on_page), (_, typeset))) = behind {
        (Some(index), typeset + down - on_page)
    } else {
        let grown = (new.bottom - new.top) - (old.bottom - old.top);
        let top_moved = down.to_pt() - (old.top - new.top);
        (None, Abs::pt(grown + top_moved))
    };

    let mut out = Frame::new(page.size(), page.kind());
    if page.has_baseline() {
        out.set_baseline(page.baseline());
    }
    // The marks of where elements begin and end that are put in or
    // taken out are those of elements that begin and end among them:
    // one left open would hold all that follows on the page. Those put
    // in are told apart from those of the page: Typst gave them their
    // places in another document, and so the same ones.
    let salt = u128::from(SPLICES.fetch_add(1, Ordering::Relaxed))
        .wrapping_mul(0x9E37_79B9_7F4A_7C15_F39C_C060_5CED_C835);
    let made = from.items().skip(new.items.start).take(new.items.len());
    let made_closed = closed_among(made.clone().map(|(_, item)| item));
    let gone = page.items().skip(old.items.start).take(old.items.len());
    let gone_closed = closed_among(gone.map(|(_, item)| item));
    let lowest = foot.to_pt() + BELOW;
    // The part itself does not go below it: the line typed in may be the
    // one that goes to the next page.
    if new.bottom + down.to_pt() > lowest {
        return Err(Refusal::BreaksOtherwise);
    }
    let mut moving = true;
    for (index, (position, item)) in page.items().enumerate() {
        if index == old.items.start {
            for (position, item) in made.clone() {
                if let Some(item) = relocated(item, salt, &made_closed) {
                    out.push(Point::new(position.x, position.y + down), item);
                }
            }
        }
        if old.items.contains(&index) {
            let mut open = Vec::new();
            open_marks(*position, item, &gone_closed, &mut open);
            for (position, mark) in open {
                out.push(position, mark);
            }
            continue;
        }
        let mut position = *position;
        if index >= old.items.end {
            // The footnotes, and what is below them, stay.
            moving = moving && !is_rule_across(item);
            let below = match moved_from {
                Some(first) => index >= first,
                None => extent(position, item).0 >= old.bottom - BELOW,
            };
            if moving && below && !stays(index) && !matches!(item, FrameItem::Tag(_)) {
                // What is pushed below the text goes to the next page
                // when all of it is typeset (and maybe more with it: a
                // heading keeps to what follows it): until then it is not
                // shown, rather than over the foot of the page. Where
                // elements begin and end in it stays.
                position.y += moved_by;
                if extent(position, item).1 > lowest {
                    let mut marks = Vec::new();
                    open_marks(position, item, &HashSet::new(), &mut marks);
                    for (position, mark) in marks {
                        out.push(position, mark);
                    }
                    continue;
                }
            }
        }
        out.push(position, item.clone());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(text: &str) -> Source {
        Source::detached(text)
    }

    /// The region `typed` put in at `at` of `text` makes.
    fn region(text: &str, at: usize, typed: &str) -> Result<(Source, Region), Refusal> {
        let old = source(text);
        let mut new = old.clone();
        new.edit(at..at, typed);
        let region = changed_region(&old, &new, &(at..at + typed.len()))?;
        Ok((new, region))
    }

    /// That region as text, and with the paragraphs beside it.
    fn parts(text: &str, at: usize, typed: &str) -> Result<(String, String), Refusal> {
        let (new, region) = region(text, at, typed)?;
        let text = new.text();
        Ok((text[region.bytes].to_owned(), text[region.kept].to_owned()))
    }

    const TEXT: &str = "#set text(size: 11pt)\n\n= Title\nFirst paragraph\nof two lines.\n\n\
                        Second $x^2$ one.\n\n- item\n- more\n\nLast.\n";

    #[test]
    fn what_changed_is_found_with_its_whole_paragraph() {
        let at = TEXT.find("of two").unwrap_or(0);
        let (part, kept) = parts(TEXT, at, "x").unwrap_or_default();
        assert_eq!(part, "= Title\nFirst paragraph\nxof two lines.");
        assert_eq!(
            kept,
            "= Title\nFirst paragraph\nxof two lines.\n\nSecond $x^2$ one."
        );
        let at = TEXT.find("^2").unwrap_or(0);
        let (part, kept) = parts(TEXT, at, "_i").unwrap_or_default();
        assert_eq!(part, "Second $x_i^2$ one.");
        assert_eq!(
            kept,
            "= Title\nFirst paragraph\nof two lines.\n\nSecond $x_i^2$ one.\n\n- item\n- more"
        );
        let at = TEXT.find("- more").unwrap_or(0) + 6;
        assert_eq!(
            parts(TEXT, at, " things").unwrap_or_default().0,
            "- item\n- more things"
        );
        // A paragraph begun behind another: both, and whatever else
        // Typst read anew with them.
        let at = TEXT.find(" one.").unwrap_or(0) + 5;
        let both = parts(TEXT, at, "\n\nNew").unwrap_or_default().0;
        assert!(both.starts_with("Second $x^2$ one.\n\nNew"), "{both:?}");
        assert!(!both.contains("Last."), "{both:?}");
        // In the rules: everything changes.
        assert_eq!(parts(TEXT, 10, "x"), Err(Refusal::InRules));
    }

    /// The numbers of `node` and all below it.
    fn numbers(node: &typst::syntax::SyntaxNode, out: &mut Vec<u64>) {
        out.push(number(node.span()));
        for child in node.children() {
            numbers(child, out);
        }
    }

    #[test]
    fn the_part_keeps_its_numbers_when_all_else_is_cut_away() {
        let at = TEXT.find("^2").unwrap_or(0);
        let (new, region) = region(TEXT, at, "_i").unwrap_or_else(|why| panic!("{why:?}"));
        let alone = region.clone().alone(&new);
        let cut = cut_down(&new, &alone, "#set par(justify: true)\n")
            .unwrap_or_else(|why| panic!("{why:?}"));
        assert_eq!(
            cut.text(),
            "#set text(size: 11pt)\n\n\n#set par(justify: true)\n\n\n\nSecond $x_i^2$ one."
        );
        let with_both = cut_down(&new, &region, "").unwrap_or_else(|why| panic!("{why:?}"));
        assert!(
            with_both
                .text()
                .contains("of two lines.\n\nSecond $x_i^2$ one.\n\n- item")
        );
        assert!(!with_both.text().contains("Last."));
        // Every node of the part is found in what was cut down by the
        // number it has in the whole.
        let of_region = |source: &Source, range: Numbers| -> Vec<u64> {
            let mut all = Vec::new();
            numbers(source.root(), &mut all);
            all.into_iter()
                .filter(|number| range.0 <= *number && *number < range.1)
                .collect()
        };
        for cut in [&cut, &with_both] {
            assert_ne!(of_region(&new, region.now), Vec::<u64>::new());
            assert_eq!(of_region(&new, region.now), of_region(cut, region.now));
        }
        // And so are those of the paragraphs beside it.
        for beside in [region.before, region.behind].into_iter().flatten() {
            assert_ne!(of_region(&new, beside), Vec::<u64>::new());
            assert_eq!(of_region(&new, beside), of_region(&with_both, beside));
        }
    }
}
