//! What Vim's keys do to the text where only pages are shown.
//!
//! On a page the cursor stands on what is shown, and the keys that change
//! what it stands on (`r`, `~`, `J`) are about that: a character takes
//! the place of what is read there, the case of letters that are read is
//! switched, paragraphs are joined. What that is in the text, the syntax
//! tree says: a letter of a name is not a letter that is read.

use std::ops::Range;

use typst::syntax::{LinkedNode, Side, Source, SyntaxKind};

use super::{Ground, containing, equation_at, ground, typed};

/// The signs `r` puts into an equation besides letters and digits: those
/// that stand for themselves there. What would make another equation of
/// it (a bracket, what raises or lowers, a dollar sign) is typed.
const MATH_SIGNS: &str = "+-=<>!?*|.:'";

/// What the character `c` is written as where it takes the place of
/// what a page shows at bytes `stop` of `source`: the bytes to replace,
/// the text to put there, and how far into it what is shown of it
/// begins. `None` where no character can.
///
/// What is replaced is text that is read: characters of running text
/// (the blank between two words is one, an escaped sign, a dash written
/// with hyphens), and in an equation a letter, a number, or a symbol
/// written by its name, which goes whole. A reference, a citation, what
/// a call put on the page are no text, and stay. `stop` is empty where a
/// part of an equation is yet to be typed: `c` is then that part.
///
/// In running text `c` is written so that it is read as itself, as
/// when it is typed, and so are the signs that mean something when they
/// are typed: `$` begins no equation here. In an equation a blank parts
/// it from a letter beside it: together they would be a name.
#[must_use]
pub fn put_over(
    source: &Source,
    stop: &Range<usize>,
    c: char,
) -> Option<(Range<usize>, String, usize)> {
    let text = source.text();
    text.get(stop.clone())?;
    let in_equation = equation_at(source, stop.start)
        .is_some_and(|equation| equation.start < stop.start && stop.end < equation.end);
    if in_equation {
        if !(c.is_alphanumeric() || MATH_SIGNS.contains(c)) {
            return None;
        }
        if !(stop.is_empty() || is_read(source, stop, true)) {
            return None;
        }
        let before = text[..stop.start].chars().next_back();
        let behind = text[stop.end..].chars().next();
        let lead = c.is_alphanumeric() && before.is_some_and(char::is_alphabetic);
        let trail = c.is_alphabetic() && behind.is_some_and(char::is_alphanumeric);
        let mut with = String::new();
        if lead {
            with.push(' ');
        }
        with.push(c);
        if trail {
            with.push(' ');
        }
        return Some((stop.clone(), with, usize::from(lead)));
    }
    if stop.is_empty() || !is_read(source, stop, false) {
        return None;
    }
    if ground(source, stop.start) == Ground::Raw {
        // Shown as it is written.
        return Some((stop.clone(), c.to_string(), 0));
    }
    let line = text[..stop.start].rfind('\n').map_or(0, |found| found + 1);
    let begins_line = text[line..stop.start].trim().is_empty();
    // What typing it would take for a wish: here it is a character.
    if matches!(c, '$' | '#' | '@') || (begins_line && matches!(c, '=' | '-' | '+' | '/')) {
        return Some((stop.clone(), format!("\\{c}"), 0));
    }
    match typed(source, stop.start, c) {
        None => Some((stop.clone(), c.to_string(), 0)),
        Some((range, with, _)) if range.end == stop.start => {
            // The character is the last of what is put there, behind
            // the backslash if it has one.
            let shown = with.len().checked_sub(c.len_utf8())?;
            let escaped = with[..shown].ends_with('\\');
            let within = shown - usize::from(escaped);
            Some((range.start..stop.end, with, within))
        }
        Some(_) => None,
    }
}

/// Whether bytes `stop` of `source` are text that is read as what it
/// shows: characters of running text, or (`math`) of an equation, where
/// a symbol written by its name is read as one too.
fn is_read(source: &Source, stop: &Range<usize>, math: bool) -> bool {
    let Some(leaf) = LinkedNode::new(source.root()).leaf_at(stop.start, Side::After) else {
        return false;
    };
    let range = leaf.range();
    let within = range.start <= stop.start && stop.end <= range.end;
    let whole = range == *stop;
    match leaf.kind() {
        // A link written as it is shows its letters as written.
        SyntaxKind::Text | SyntaxKind::Space | SyntaxKind::Link => within && !math,
        SyntaxKind::Escape | SyntaxKind::Shorthand | SyntaxKind::SmartQuote => whole && !math,
        SyntaxKind::MathText => within && math,
        SyntaxKind::MathShorthand => whole && math,
        // A name, or a name with what it is the kind of: `arrow.r`.
        SyntaxKind::MathIdent if math => {
            let mut node = Some(leaf.clone());
            let mut named = false;
            while let Some(here) = node {
                let part = matches!(
                    here.kind(),
                    SyntaxKind::MathIdent | SyntaxKind::MathFieldAccess
                );
                if !part || here.range().start != stop.start {
                    break;
                }
                named |= here.range() == *stop;
                node = here.parent().cloned();
            }
            named
        }
        _ => false,
    }
}

/// The parts of bytes `range` of `source` that are read as the letters
/// they are written with, in the order of the text: running text, raw
/// text, and in equations what is no name. The case of these can be
/// switched; a name in another case is another name, or none.
///
/// Not the text of a footnote that `range` reaches over: the page shows
/// that elsewhere, at the foot of the page, and `gUU` on a line with
/// the footnote's mark changed it unseen. Within the footnote, its own.
#[must_use]
pub fn letters(source: &Source, range: &Range<usize>) -> Vec<Range<usize>> {
    fn gather(node: &LinkedNode, range: &Range<usize>, found: &mut Vec<Range<usize>>) {
        let here = node.range();
        if here.end <= range.start || here.start >= range.end {
            return;
        }
        let note = node.kind() == SyntaxKind::FuncCall
            && node
                .children()
                .next()
                .is_some_and(|callee| callee.leaf_text() == "footnote");
        if note && !(here.start < range.start && range.end < here.end) {
            return;
        }
        if matches!(node.kind(), SyntaxKind::Text | SyntaxKind::MathText) {
            found.push(here.start.max(range.start)..here.end.min(range.end));
            return;
        }
        for child in node.children() {
            gather(&child, range, found);
        }
    }
    let mut found = Vec::new();
    gather(&LinkedNode::new(source.root()), range, &mut found);
    found
}

/// Whether the line of `source` that begins at byte `line` begins
/// something of its own: it has the mark of a heading, of an item of a
/// list or of a term. With it the bytes of the mark and of the blanks
/// behind it.
fn marked(source: &Source, line: usize) -> Option<Range<usize>> {
    let text = source.text();
    let rest = text.get(line..)?;
    let from = line + (rest.len() - rest.trim_start_matches([' ', '\t']).len());
    let leaf = LinkedNode::new(source.root()).leaf_at(from, Side::After)?;
    let is_mark = matches!(
        leaf.kind(),
        SyntaxKind::HeadingMarker
            | SyntaxKind::ListMarker
            | SyntaxKind::EnumMarker
            | SyntaxKind::TermMarker
    );
    if !is_mark || leaf.range().start != from {
        return None;
    }
    let behind = &text[leaf.range().end..];
    let blanks = behind.len() - behind.trim_start_matches([' ', '\t']).len();
    Some(from..leaf.range().end + blanks)
}

/// What `J` does at byte `at` of `source` where only pages are shown:
/// it joins what the cursor is in (a paragraph, a heading, an item of a
/// list) with what follows it. The bytes that part the two, and what
/// stands in their place: a blank if `spaced`, nothing for `gJ`. `None`
/// if nothing follows, or the cursor is in something that is not parted
/// from what follows by a line: code, a raw block.
///
/// The lines of the text are not what is joined: lines of one paragraph
/// are one paragraph on the page already. What follows loses the mark
/// that made it a heading or an item: it is text of this paragraph now.
#[must_use]
pub fn join(source: &Source, at: usize, spaced: bool) -> Option<(Range<usize>, String)> {
    let text = source.text();
    let mut line = text.get(..at)?.rfind('\n').map_or(0, |found| found + 1);
    // The end of the last line of what the cursor is in.
    let end = loop {
        let end = line + text[line..].find('\n')?;
        let next = end + 1;
        let next_end = text[next..]
            .find('\n')
            .map_or(text.len(), |found| next + found);
        let heading = marked(source, line).is_some_and(|mark| text[mark].starts_with('='));
        if heading || text[next..next_end].trim().is_empty() || marked(source, next).is_some() {
            break end;
        }
        line = next;
    };
    if ground(source, end) != Ground::Markup {
        return None;
    }
    let from = text[..end].trim_end_matches([' ', '\t']).len();
    // A label at the end would come to stand in the middle, and no
    // longer name what it named.
    let labelled = LinkedNode::new(source.root())
        .leaf_at(from, Side::Before)
        .is_some_and(|leaf| leaf.kind() == SyntaxKind::Label);
    let behind = text[end..].trim_start();
    if behind.is_empty() || labelled {
        return None;
    }
    let mut to = text.len() - behind.len();
    let to_line = text[..to].rfind('\n').map_or(0, |found| found + 1);
    if let Some(mark) = marked(source, to_line) {
        // A heading with a label is referred to as one: as text of this
        // paragraph it could not be.
        let line_end = text[to..].find('\n').map_or(text.len(), |found| to + found);
        if labels(&LinkedNode::new(source.root()), &(to..line_end)) {
            return None;
        }
        to = mark.end;
    }
    // Without a blank between, the mark of strong or emphasised text
    // would stand in a word, and be a character like any other.
    let mark = |c: Option<char>| matches!(c, Some('*' | '_'));
    let letter = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    let (left, right) = (text[..from].chars().next_back(), text[to..].chars().next());
    if !spaced && ((mark(left) && letter(right)) || (letter(left) && mark(right))) {
        return None;
    }
    // Behind a comment that ends the line, what follows would be part
    // of the comment: the line break that ends it stays, and in a
    // paragraph it is the blank between the two. A heading or an item
    // ends with its line, and gives no such way.
    let comment = LinkedNode::new(source.root())
        .leaf_at(from, Side::Before)
        .filter(|leaf| leaf.kind() == SyntaxKind::LineComment);
    if let Some(comment) = comment {
        let chain = super::ancestors(source, comment.range().start);
        let ends_with_line = marked(source, line).is_some()
            || chain.iter().any(|node| {
                matches!(
                    node.kind(),
                    SyntaxKind::Heading
                        | SyntaxKind::ListItem
                        | SyntaxKind::EnumItem
                        | SyntaxKind::TermItem
                )
            });
        return (spaced && !ends_with_line).then(|| (end..to, "\n".to_owned()));
    }
    let with = if spaced { " " } else { "" };
    // Nor where Typst could not read it then: what follows a line of
    // code (`#include "a.typ"`) would be read as more of it.
    if breaks(source, &[(from..to, with)]).is_some() {
        return None;
    }
    Some((from..to, with.to_owned()))
}

/// Whether there is a label in bytes `range` of `node`.
fn labels(node: &LinkedNode, range: &Range<usize>) -> bool {
    if node.kind() == SyntaxKind::Label {
        return range.contains(&node.offset());
    }
    node.children().any(|child| {
        let part = child.range();
        part.start < range.end && range.start < part.end && labels(&child, range)
    })
}

/// The functions whose content goes on in the text around them: what is
/// typed into it is of the same line as what stands before and behind.
const IN_LINE: &[&str] = &[
    "strong",
    "emph",
    "underline",
    "overline",
    "strike",
    "highlight",
    "text",
    "smallcaps",
    "sub",
    "super",
    "upper",
    "lower",
    "link",
];

/// Whether bytes `range` of `source` cross from one part of the page
/// that is typed into by itself to another: the cells of a table, a
/// caption and what it is the caption of, a footnote and the text it
/// is in, a quote and what is around it. Backspace and Delete do not
/// take what is on the other side of that. Text set off as strong or
/// emphasised (also with `#strong[…]` and its like) is no such part.
#[must_use]
pub fn parted(source: &Source, range: Range<usize>) -> bool {
    let text = source.text();
    let part = |at: usize| {
        let (chain, _) = containing(source, at.min(text.len()));
        chain
            .iter()
            .rfind(|node| {
                if node.kind() != SyntaxKind::ContentBlock {
                    return false;
                }
                let callee = node
                    .parent()
                    .filter(|args| args.kind() == SyntaxKind::Args)
                    .and_then(LinkedNode::parent)
                    .filter(|call| call.kind() == SyntaxKind::FuncCall)
                    .and_then(|call| call.children().next())
                    .filter(|callee| callee.kind() == SyntaxKind::Ident);
                !callee.is_some_and(|callee| IN_LINE.contains(&&text[callee.range()]))
            })
            .map(LinkedNode::range)
    };
    // Nor into a comment, nor across a line break onto the line of a
    // comment or of code: neither shows, and the text joined to it would
    // not show either, or would be code.
    let root = LinkedNode::new(source.root());
    let comment = |leaf: &LinkedNode| {
        matches!(
            leaf.kind(),
            SyntaxKind::LineComment | SyntaxKind::BlockComment
        )
    };
    let cuts = |at: usize| {
        root.leaf_at(at, Side::After)
            .is_some_and(|leaf| comment(&leaf) && leaf.range().start < at && at < leaf.range().end)
    };
    let breaks = text.get(range.clone()).is_some_and(|it| it.contains('\n'));
    let after_hidden = range.start.checked_sub(1).is_some_and(|before| {
        ground(source, before) == Ground::Code
            || root
                .leaf_at(range.start, Side::Before)
                .is_some_and(|leaf| comment(&leaf))
    });
    let before_hidden = root.leaf_at(range.end, Side::After).is_some_and(|leaf| {
        (comment(&leaf) || leaf.kind() == SyntaxKind::Hash) && leaf.range().start == range.end
    });
    cuts(range.start)
        || cuts(range.end)
        || (breaks && (after_hidden || before_hidden))
        || part(range.start) != part(range.end)
}

/// What deleting bytes `range` of `source` takes where only pages are
/// shown, with `with` put where the first part of it was: the parts of
/// it that go, in the order of the text and apart from each other. A mark that opens or closes something (strong or
/// emphasised text, the brackets of a cell or a footnote, the dollar
/// signs of an equation, parentheses) stays where the mark that belongs
/// with it is outside of `range`: that is not seen on the page, and
/// without it all that follows would be something else. `None` where
/// what is left would turn the mark of strong or emphasised text into
/// a character, standing in a word.
///
/// What shows as nothing and goes with what `range` takes all of goes
/// too, though it is not in `range` (see [`taken_along`]): the `#` before
/// a call, the call an equation is given to. The first piece then begins
/// before `range`.
#[must_use]
pub fn pieces(source: &Source, range: Range<usize>, with: &str) -> Option<Vec<Range<usize>>> {
    let text = source.text();
    if text.get(range.clone()).is_none() {
        return Some(vec![range]);
    }
    let range = taken_along(source, &range).unwrap_or(range);
    let mut kept = Vec::new();
    unpaired(&LinkedNode::new(source.root()), &range, &mut kept);
    kept.sort_by_key(|mark| mark.start);
    let mut pieces = Vec::new();
    let mut from = range.start;
    for mark in &kept {
        if mark.start > from {
            pieces.push(from..mark.start);
        }
        from = from.max(mark.end);
    }
    if range.end > from {
        pieces.push(from..range.end);
    }
    // What stands around what goes, as it will be: the marks of strong
    // or emphasised text are to be marks still.
    let set_off = |at: usize, side: Side| {
        LinkedNode::new(source.root())
            .leaf_at(at, side)
            .filter(|leaf| matches!(leaf.kind(), SyntaxKind::Star | SyntaxKind::Underscore))
            .filter(|leaf| {
                leaf.parent()
                    .is_some_and(|it| matches!(it.kind(), SyntaxKind::Strong | SyntaxKind::Emph))
            })
            .map(|leaf| leaf.range())
    };
    let mut left: Vec<(char, bool)> = text[..range.start]
        .chars()
        .rev()
        .take(2)
        .map(|c| (c, false))
        .collect();
    left.reverse();
    if let Some(last) = left.last_mut() {
        last.1 = set_off(range.start, Side::Before).is_some_and(|mark| mark.end == range.start);
    }
    let mut shown = left;
    // What is put in goes where the first piece was.
    let put = pieces.first().map_or(range.start, |piece| piece.start);
    let mut put_in = Some(with);
    for mark in &kept {
        if mark.start > put
            && let Some(with) = put_in.take()
        {
            shown.extend(with.chars().map(|c| (c, false)));
        }
        let is_set_off = set_off(mark.start, Side::After).is_some_and(|it| it == *mark);
        shown.extend(text[mark.clone()].chars().map(|c| (c, is_set_off)));
    }
    if let Some(with) = put_in {
        shown.extend(with.chars().map(|c| (c, false)));
    }
    for (index, c) in text[range.end..].chars().take(2).enumerate() {
        let is_set_off =
            index == 0 && set_off(range.end, Side::After).is_some_and(|it| it.start == range.end);
        shown.push((c, is_set_off));
    }
    let letter = |at: Option<usize>| {
        at.and_then(|at| shown.get(at))
            .is_some_and(|(c, _)| c.is_alphanumeric())
    };
    let glued =
        (0..shown.len()).any(|at| shown[at].1 && letter(at.checked_sub(1)) && letter(Some(at + 1)));
    // Nor is what follows a line of code (a rule, a `#let`) to come to
    // stand right behind it: it would be read as part of the code.
    let breaks = text[range.clone()].contains('\n');
    let follows = with
        .chars()
        .chain(text[range.end..].chars())
        .next()
        .is_some_and(|c| !c.is_whitespace());
    let onto_code = breaks && follows && ends_code(source, range.start);
    // Nor a paragraph break put into strong or emphasised text: it would
    // be left open where the paragraph ends.
    let joined: String = text[..range.start]
        .chars()
        .next_back()
        .into_iter()
        .chain(with.chars())
        .chain(text[range.end..].chars().next())
        .collect();
    let lines: Vec<&str> = joined.split('\n').collect();
    let blank = lines.len() > 2
        && lines[1..lines.len() - 1]
            .iter()
            .any(|l| l.trim().is_empty());
    let in_set_off = containing(source, range.start)
        .0
        .iter()
        .any(|node| matches!(node.kind(), SyntaxKind::Strong | SyntaxKind::Emph));
    let left_open = blank && in_set_off;
    (!glued && !onto_code && !left_open).then_some(pieces)
}

/// What deleting bytes `range` of `source` takes along that is not in
/// them, where the page shows it as nothing and it would be left alone:
/// the bytes to take instead. `None` where that is nothing.
///
/// - The `#` before code that `range` takes all of: a footnote's mark,
///   a symbol written by its name, a box. Left behind, it would make
///   code of what follows.
/// - The call an equation that `range` takes all of is given to, with
///   its `#`, where that call shows nothing else (a numbered equation, a
///   figure with no caption): the writer deletes the equation, and the
///   call would lack it. A figure with a caption is no such call.
///
/// A label right behind such a call goes with it: it would label what
/// stands before it then.
#[must_use]
pub fn taken_along(source: &Source, range: &Range<usize>) -> Option<Range<usize>> {
    let text = source.text();
    let taken = text.get(range.clone())?;
    if taken.trim().is_empty() {
        return None;
    }
    let root = LinkedNode::new(source.root());
    let start = range.start + (taken.len() - taken.trim_start().len());
    let end = range.end - (taken.len() - taken.trim_end().len());
    // The call an equation that is all of what is taken is given to.
    let given = root
        .leaf_at(start, Side::After)
        .and_then(|leaf| leaf.parent().cloned())
        .filter(|equation| equation.kind() == SyntaxKind::Equation)
        .filter(|equation| equation.range() == (start..end))
        .and_then(|equation| equation.parent().cloned())
        .filter(|args| args.kind() == SyntaxKind::Args)
        .filter(|args| {
            // Nothing else of it shows: no content given to it besides,
            // no caption, no text.
            let shows = |node: &LinkedNode| {
                matches!(
                    node.kind(),
                    SyntaxKind::ContentBlock | SyntaxKind::Equation | SyntaxKind::Str
                )
            };
            args.children().all(|arg| match arg.kind() {
                SyntaxKind::Named => {
                    let name = arg.children().next();
                    let caption = name.is_some_and(|name| {
                        matches!(name.leaf_text().as_str(), "caption" | "body" | "supplement")
                    });
                    !caption
                        && arg
                            .children()
                            .skip(1)
                            .all(|value| !shows(&value) || value.kind() == SyntaxKind::Str)
                }
                _ => arg.range() == (start..end) || !shows(&arg),
            })
        })
        .and_then(|args| args.parent().cloned())
        .filter(|call| call.kind() == SyntaxKind::FuncCall);
    let (code, mut whole) = match given {
        Some(call) => (call.range(), true),
        None => (start..end, false),
    };
    // The `#` before code taken whole.
    let hash = root
        .leaf_at(code.start, Side::Before)
        .filter(|hash| hash.kind() == SyntaxKind::Hash && hash.range().end == code.start)
        .filter(|hash| {
            hash.next_sibling().is_some_and(|after| {
                after.range().start == code.start && after.range().end <= code.end
            })
        });
    let from = match hash {
        Some(hash) => {
            whole = true;
            hash.range().start
        }
        None => code.start,
    };
    if !whole {
        return None;
    }
    // And its label.
    let behind = &text[code.end..];
    let blanks = behind.len() - behind.trim_start_matches([' ', '\t']).len();
    let to = root
        .leaf_at(code.end + blanks, Side::After)
        .filter(|label| {
            label.kind() == SyntaxKind::Label && label.range().start == code.end + blanks
        })
        .map_or(code.end, |label| label.range().end);
    let along = from.min(range.start)..to.max(range.end);
    (along != *range).then_some(along)
}

/// Where what goes in before the place a page shows at bytes `stop` of
/// `source` goes: before the `#` that begins the code the place is
/// written with (a footnote's mark, a figure, a box), else at its start.
#[must_use]
pub fn front(source: &Source, stop: &Range<usize>) -> usize {
    LinkedNode::new(source.root())
        .leaf_at(stop.start, Side::Before)
        .filter(|hash| hash.kind() == SyntaxKind::Hash && hash.range().end == stop.start)
        .filter(|hash| {
            hash.next_sibling()
                .is_some_and(|code| code.range().start == stop.start)
        })
        .map_or(stop.start, |hash| hash.range().start)
}

/// Whether byte `at` of `source` ends code put into running text with
/// `#`, or is in it.
fn ends_code(source: &Source, at: usize) -> bool {
    let Some(mut node) = LinkedNode::new(source.root()).leaf_at(at, Side::Before) else {
        return false;
    };
    while let Some(parent) = node.parent().cloned() {
        if parent.kind() == SyntaxKind::Markup {
            return node
                .prev_sibling()
                .is_some_and(|before| before.kind() == SyntaxKind::Hash)
                || node.kind() == SyntaxKind::Hash;
        }
        node = parent;
    }
    false
}

/// Collects into `kept` the marks of `node` and of what is in it that
/// open or close something in bytes `range` of the text whose other
/// mark is outside of `range`.
fn unpaired(node: &LinkedNode, range: &Range<usize>, kept: &mut Vec<Range<usize>>) {
    let inside = |part: &Range<usize>| range.start <= part.start && part.end <= range.end;
    let children: Vec<LinkedNode> = node.children().collect();
    let mut keep = |open: &LinkedNode, close: &LinkedNode| {
        let (open, close) = (open.range(), close.range());
        match (inside(&open), inside(&close)) {
            (true, false) => kept.push(open),
            (false, true) => kept.push(close),
            _ => {}
        }
    };
    // Text set off, an equation, raw text: the first and the last.
    if let [first, .., last] = children.as_slice()
        && first.kind() == last.kind()
        && matches!(
            first.kind(),
            SyntaxKind::Star | SyntaxKind::Underscore | SyntaxKind::Dollar | SyntaxKind::RawDelim
        )
    {
        keep(first, last);
    }
    // Brackets, parentheses and braces, each with the one it closes.
    // In running text a bracket is text, but the one that closes it is
    // read as closing it: without it, Typst cannot read the other.
    let text_bracket = |child: &LinkedNode, mark: &str| {
        node.kind() == SyntaxKind::Markup
            && child.kind() == SyntaxKind::Text
            && child.leaf_text() == mark
    };
    let mut open: Vec<&LinkedNode> = Vec::new();
    for child in &children {
        match child.kind() {
            SyntaxKind::LeftBracket | SyntaxKind::LeftParen | SyntaxKind::LeftBrace => {
                open.push(child);
            }
            SyntaxKind::RightBracket | SyntaxKind::RightParen | SyntaxKind::RightBrace => {
                if let Some(opened) = open.pop() {
                    keep(opened, child);
                }
            }
            _ if text_bracket(child, "[") => open.push(child),
            _ if text_bracket(child, "]") => {
                if let Some(opened) = open.pop() {
                    keep(opened, child);
                }
            }
            _ => {}
        }
    }
    // Of a call that does not go whole (a table, a footnote, `#strong`)
    // what makes it one stays, and the `#` before it: its name, the
    // names of its arguments and the commas between them, the brackets
    // of what it holds. What that holds goes; cells stay cells.
    if matches!(
        node.kind(),
        SyntaxKind::FuncCall | SyntaxKind::Args | SyntaxKind::Named
    ) {
        for child in children.iter().filter(|child| inside(&child.range())) {
            if child.kind() == SyntaxKind::ContentBlock {
                let marks: Vec<LinkedNode> = child.children().collect();
                if let [first, .., last] = marks.as_slice() {
                    kept.extend([first.range(), last.range()]);
                }
            } else {
                kept.push(child.range());
            }
        }
    }
    let cut_into = |part: &Range<usize>| {
        (part.start < range.start && range.start < part.end)
            || (part.start < range.end && range.end < part.end)
    };
    for pair in children.windows(2) {
        if pair[0].kind() == SyntaxKind::Hash
            && inside(&pair[0].range())
            && cut_into(&pair[1].range())
        {
            kept.push(pair[0].range());
        }
    }
    // Into what is cut, not into what goes whole or stays whole.
    for child in &children {
        let part = child.range();
        let cut = (part.start < range.start && range.start < part.end)
            || (part.start < range.end && range.end < part.end);
        if cut {
            unpaired(child, range, kept);
        }
    }
}

/// What `edits` (each a range of `source` as it is and what to put
/// there, in order and apart) would break that is whole now: what Typst
/// could not read then, as words for a person (strong text unclosed).
/// `None` where nothing would break, and where something is broken
/// already.
#[must_use]
pub fn breaks(source: &Source, edits: &[(Range<usize>, &str)]) -> Option<String> {
    // What an equation lacks after something in it went (a mark that
    // raises nothing, a fraction without what it divides by) is nothing
    // broken: the page shows an empty place there, and what is left
    // over is tidied away (see `lacking` and `Editor::tidy_equation`).
    // Else nothing in an exponent could be taken out to type it anew.
    // That holds for what goes within one equation, where the cursor
    // stays in it; what reaches out of it leaves it behind as it is.
    fn first(node: &LinkedNode, lenient: bool, in_equation: bool) -> Option<String> {
        let in_equation = in_equation || node.kind() == SyntaxKind::Equation;
        if node.kind() == SyntaxKind::Error {
            let (errors, _) = node.get().errors_and_warnings();
            let message = errors.first().map(|error| error.message.to_string())?;
            // Nothing behind a mark that raises, lowers or divides, or
            // nothing before it.
            let lacking = [
                "expected expression",
                "unexpected hat",
                "unexpected underscore",
                "unexpected slash",
            ];
            let lacks = lenient && in_equation && lacking.contains(&message.as_str());
            return (!lacks).then_some(message);
        }
        if !node.get().diagnosis().errors {
            return None;
        }
        node.children()
            .find_map(|child| first(&child, lenient, in_equation))
    }
    if source.root().diagnosis().errors {
        return None;
    }
    let mut after = source.clone();
    for (range, with) in edits.iter().rev() {
        source.text().get(range.clone())?;
        after.edit(range.clone(), with);
    }
    if !after.root().diagnosis().errors {
        return None;
    }
    let within = edits.iter().all(|(range, _)| {
        equation_at(source, range.start)
            .is_some_and(|equation| equation.start < range.start && range.end < equation.end)
    });
    let message = first(&LinkedNode::new(after.root()), within, false)?;
    let message = message.as_str();
    Some(match message {
        "unclosed delimiter" => "a mark unclosed: strong or emphasised text, say".to_owned(),
        "expected expression" => "a part of an equation with nothing in it".to_owned(),
        _ => format!("what Typst cannot read ({message})"),
    })
}

/// Whether taking bytes `range` out of `source` would take a mark that
/// opens or closes something (a dollar sign, a star, a bracket, the
/// marker of a heading) and leave that something: half an equation, a
/// strong text that is never closed. What a page shows, and things that
/// go whole (a symbol, a label, a comment), may be taken by themselves;
/// running text may be cut anywhere.
#[must_use]
pub fn cuts(source: &Source, range: &Range<usize>) -> bool {
    /// Leaves that may be taken whole without what they are in.
    fn by_itself(kind: SyntaxKind) -> bool {
        matches!(
            kind,
            SyntaxKind::Text
                | SyntaxKind::Space
                | SyntaxKind::Parbreak
                | SyntaxKind::Linebreak
                | SyntaxKind::Escape
                | SyntaxKind::Shorthand
                | SyntaxKind::SmartQuote
                | SyntaxKind::Link
                | SyntaxKind::Label
                | SyntaxKind::RefMarker
                | SyntaxKind::LineComment
                | SyntaxKind::BlockComment
                | SyntaxKind::RawTrimmed
                | SyntaxKind::MathText
                | SyntaxKind::MathIdent
                | SyntaxKind::MathShorthand
        )
    }
    fn walk(node: &LinkedNode, range: &Range<usize>) -> bool {
        let here = node.range();
        let apart = here.end <= range.start || here.start >= range.end;
        let within = range.start <= here.start && here.end <= range.end;
        if apart || within {
            return false;
        }
        if node.children().len() == 0 {
            // Cut in two: only running text and blanks may be.
            return !matches!(
                node.kind(),
                SyntaxKind::Text | SyntaxKind::Space | SyntaxKind::Parbreak | SyntaxKind::Link
            );
        }
        let children: Vec<LinkedNode> = node.children().collect();
        children.iter().enumerate().any(|(index, child)| {
            let part = child.range();
            let taken = range.start <= part.start && part.end <= range.end && !part.is_empty();
            if taken && child.children().len() == 0 && !by_itself(child.kind()) {
                // A hash goes with the code it begins.
                let code = children.get(index + 1).map(LinkedNode::range);
                let with_code = child.kind() == SyntaxKind::Hash
                    && code.is_some_and(|code| code.end <= range.end);
                return !with_code;
            }
            walk(child, range)
        })
    }
    walk(&LinkedNode::new(source.root()), range)
}

/// Blanks, for [`raw_blank`] to take as many of as it needs.
const BLANKS: &str = "\n                                                                ";

/// What to typeset at byte `at` of `source` so that a cursor there has
/// a place on the page, where it is in raw text on a line with nothing
/// on it: a blank, which shows as nothing and still is a glyph in that
/// line. `None` where it is not in such a place.
///
/// A block of code just begun has no line yet: the cursor stands behind
/// the marks that open it (and the name of the language, if one is
/// typed). There it is a line break and a blank, the line that is
/// about to be typed. As many blanks are given as Typst takes off every
/// line of the block as its indent, and one more.
#[must_use]
pub fn raw_blank(source: &Source, at: usize) -> Option<&'static str> {
    let text = source.text();
    let (chain, _) = containing(source, at);
    let raw = chain.iter().rfind(|node| node.kind() == SyntaxKind::Raw)?;
    let open = raw.children().next()?.range();
    let close = raw.children().next_back()?.range();
    if at < open.end || at > close.start {
        return None;
    }
    let line = text[..at].rfind('\n').map_or(0, |found| found + 1);
    let line_end = text[at..].find('\n').map_or(text.len(), |found| at + found);
    if open.len() < 3 {
        // Raw text in a line: only where there is none of it yet.
        let nothing = open.end == close.start;
        return nothing.then(|| &BLANKS[1..2]);
    }
    // The blanks before the marks that close the block are the most
    // that is taken off its lines.
    let last = text[..close.start].rfind('\n').map_or(0, |found| found + 1);
    let indent = text[last..close.start].chars().count();
    let blanks = (indent + 1).min(BLANKS.len() - 1);
    if line <= open.start {
        // The line that opens the block: behind all that is on it,
        // while the block has no line with something on it.
        let shows = raw
            .children()
            .any(|part| part.kind() == SyntaxKind::Text && !text[part.range()].trim().is_empty());
        let at_end = at == line_end && line_end < close.start;
        return (at_end && !shows).then(|| &BLANKS[..=blanks]);
    }
    let empty = text[line..line_end].trim().is_empty() && line_end < close.start;
    empty.then(|| &BLANKS[1..=blanks])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(text: &str) -> Source {
        Source::detached(text)
    }

    /// `text` after `c` took the place of its first `needle`, with a
    /// `|` where the cursor goes.
    fn over(text: &str, needle: &str, c: char) -> Option<String> {
        let at = text.find(needle)?;
        let (range, with, within) = put_over(&source(text), &(at..at + needle.len()), c)?;
        let mut text = text.to_owned();
        text.replace_range(range.clone(), &with);
        text.insert(range.start + within, '|');
        Some(text)
    }

    #[test]
    fn a_character_takes_the_place_of_text_that_is_read() {
        assert_eq!(over("One two.", "t", 'T').as_deref(), Some("One |Two."));
        // Written so that it is read as itself.
        assert_eq!(over("One two.", "t", '*').as_deref(), Some("One |\\*wo."));
        assert_eq!(over("One two.", "t", '$').as_deref(), Some("One |\\$wo."));
        assert_eq!(over("One two.", "t", '#').as_deref(), Some("One |\\#wo."));
        assert_eq!(over("a b", "a", '-').as_deref(), Some("|\\- b"));
        assert_eq!(over("a - b", "-", '+').as_deref(), Some("a |+ b"));
        // An escaped sign and a dash are characters on the page.
        assert_eq!(over("a \\* b", "\\*", 'x').as_deref(), Some("a |x b"));
        assert_eq!(over("a -- b", "--", '-').as_deref(), Some("a |- b"));
        // The blank between two words, however it is written.
        assert_eq!(over("One two.", " ", '-').as_deref(), Some("One|-two."));
        assert_eq!(over("One\ntwo.", "\n", '-').as_deref(), Some("One|-two."));
        // Raw text is shown as it is written.
        assert_eq!(over("`a*b`", "a", '*').as_deref(), Some("`|**b`"));
        // What is no text stays.
        assert_eq!(over("See @ref now.", "@ref", 'x'), None);
        assert_eq!(over("A #lorem(3) b.", "#lorem(3)", 'x'), None);
        assert_eq!(over("A *b* c", "*", 'x'), None);
        assert_eq!(over("A $x$ c", "$x$", 'y'), None);
    }

    #[test]
    fn in_an_equation_a_symbol_goes_whole() {
        assert_eq!(
            over("$alpha + x$", "alpha", 'a').as_deref(),
            Some("$|a + x$")
        );
        assert_eq!(over("$a -> b$", "->", '=').as_deref(), Some("$a |= b$"));
        assert_eq!(
            over("$a arrow.r b$", "arrow.r", '<').as_deref(),
            Some("$a |< b$")
        );
        assert_eq!(over("$x + 12$", "12", '3').as_deref(), Some("$x + |3$"));
        // A letter beside a letter would be a name: a blank parts them.
        assert_eq!(over("$2x$", "2", 'a').as_deref(), Some("$|a x$"));
        assert_eq!(over("$x y$", "y", '2').as_deref(), Some("$x |2$"));
        assert_eq!(over("$a 2$", "2", 'b').as_deref(), Some("$a |b$"));
        assert_eq!(over("$x1$", "1", 'y'), None, "part of a name");
        assert_eq!(over("$a/2$", "2", 'b').as_deref(), Some("$a/|b$"));
        // What would make another equation of it is not put there.
        assert_eq!(over("$a + b$", "+", '^'), None);
        assert_eq!(over("$a + b$", "b", '$'), None);
        assert_eq!(over("$a + b$", "b", '('), None);
        // Part of a name is not the name; a call is no symbol.
        assert_eq!(over("$alpha$", "al", 'x'), None);
        assert_eq!(over("$frac(a, b)$", "frac(a, b)", 'x'), None);
        // An empty part is typed by it.
        let source = source("$frac(a, )$");
        assert_eq!(
            put_over(&source, &(9..9), 'b'),
            Some((9..9, "b".to_owned(), 0))
        );
    }

    #[test]
    fn letters_are_what_is_read_as_them() {
        let read = |text: &str| {
            let source = source(text);
            let parts = letters(&source, &(0..text.len()));
            let parts: Vec<&str> = parts.into_iter().map(|part| &text[part]).collect();
            parts.join("|")
        };
        assert_eq!(read("One *two*."), "One|two|.");
        assert_eq!(read("A @ref and #lorem(3)."), "A|and|.");
        // Of an equation, what is no name; a sign has no case to switch.
        assert_eq!(read("$alpha + x_i$"), "+|x|i");
        assert_eq!(read("= Title <label>"), "Title");
        assert_eq!(read("#set page(width: 3cm)\nText"), "Text");
        // Only what is in the bytes asked for.
        let source = source("One two.");
        assert_eq!(letters(&source, &(2..5)).first(), Some(&(2..5)));
    }

    /// `text` with what its `|` is in joined with what follows.
    fn joined(text: &str) -> Option<String> {
        let at = text.find('|')?;
        let source = source(&text.replacen('|', "", 1));
        let (range, with) = join(&source, at, true)?;
        let mut text = source.text().to_owned();
        text.replace_range(range, &with);
        Some(text)
    }

    #[test]
    fn paragraphs_are_joined_with_one_blank_between() {
        assert_eq!(joined("O|ne.\n\nTwo.\n").as_deref(), Some("One. Two.\n"));
        // From any line of the paragraph, and over more than one empty
        // line.
        assert_eq!(
            joined("O|ne\nmore. \n\n\n  Two.\n").as_deref(),
            Some("One\nmore. Two.\n")
        );
        assert_eq!(
            joined("One.\n\nT|wo.\n\nThree.\n").as_deref(),
            Some("One.\n\nTwo. Three.\n")
        );
        // A heading and an item end with their line; what follows loses
        // its mark.
        assert_eq!(
            joined("= He|ad\nText.\n").as_deref(),
            Some("= Head Text.\n")
        );
        assert_eq!(joined("- o|ne\n- two\n").as_deref(), Some("- one two\n"));
        assert_eq!(
            joined("Te|xt.\n\n= Head\n").as_deref(),
            Some("Text. Head\n")
        );
        assert_eq!(joined("Te|xt.\n+ item\n").as_deref(), Some("Text. item\n"));
        // An equation that stands by itself, with its lines.
        assert_eq!(
            joined("$ a|\n  + b $\n\nTwo.\n").as_deref(),
            Some("$ a\n  + b $ Two.\n")
        );
        // Nothing follows; and code has lines of its own.
        assert_eq!(joined("One.\n\nT|wo.\n"), None);
        assert_eq!(joined("One.\n\nT|wo.\n\n\n"), None);
        assert_eq!(joined("```\na|\n\nb\n```\n\nTwo.\n"), None);
        let source = source("One.\n\nTwo.\n");
        assert_eq!(join(&source, 1, false), Some((4..6, String::new())));
    }

    #[test]
    fn an_empty_line_of_raw_text_gets_a_blank() {
        let blank = |text: &str| {
            let at = text.find('|')?;
            raw_blank(&source(&text.replacen('|', "", 1)), at)
        };
        // A block just begun, with and without its language.
        assert_eq!(blank("```|\n```"), Some("\n "));
        assert_eq!(blank("```rust|\n```"), Some("\n "));
        assert_eq!(blank("```\n|\n```"), Some(" "));
        // A line with nothing on it between others, and an indented
        // block.
        assert_eq!(blank("```\na\n|\nb\n```"), Some(" "));
        assert_eq!(blank("  ```\n  a\n|\n  ```"), Some("   "));
        // Raw text in a line that has none yet.
        assert_eq!(blank("a `|` b"), Some(" "));
        assert_eq!(blank("a `b|` c"), None);
        // Not where something is shown already, nor outside raw text.
        assert_eq!(blank("```\na|\n```"), None);
        assert_eq!(blank("```ru|st\n```"), None);
        assert_eq!(blank("```rust|\na\n```"), None);
        assert_eq!(blank("One.\n\n|\n"), None);
        assert_eq!(blank("// |\n"), None);
    }
}
