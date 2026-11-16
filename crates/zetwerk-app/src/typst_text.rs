//! What the Vim engine leaves to its host: the text objects of Typst (an
//! equation, a section, a list item, an argument, a call) and how Typst
//! writes strong, emphasised and raw text, comments, list items and
//! headings.
//!
//! All of it goes by the syntax tree, not by characters: a `$` in a
//! string is no equation, and bold in math is `bold(x)`, not `*x*`.

use std::fmt::Write;
use std::ops::Range;

use typst::syntax::{LinkedNode, Side, Source, SyntaxKind};
use zetwerk_vim::Format;

mod kept;
mod reach;
mod shown;
mod slots;

pub use shown::{breaks, cuts, join, letters, put_over, raw_blank};
pub use shown::{parted, pieces};
pub use slots::{Around, Enclosure, enclosing, has_rules, slot_name, spot_name};

/// The innermost node at byte `at` of `source` and its ancestors, from
/// the inside out.
fn ancestors(source: &Source, at: usize) -> Vec<LinkedNode<'_>> {
    let mut chain = Vec::new();
    let mut node = LinkedNode::new(source.root()).leaf_at(at, Side::After);
    while let Some(here) = node {
        node = here.parent().cloned();
        chain.push(here);
    }
    chain
}

/// What [`ancestors`] finds below the root of `source`: the nodes of
/// the child of the root that holds byte `at`, each a node of that child
/// taken for a root, which begins at the byte given (see [`reach`]). The
/// root is not gone through.
fn ancestors_below(source: &Source, at: usize) -> (Vec<LinkedNode<'_>>, usize) {
    let Some((leaf, start)) = reach::leaf_after(source, at) else {
        return (Vec::new(), 0);
    };
    let mut chain = Vec::new();
    let mut node = Some(leaf);
    while let Some(here) = node {
        node = here.parent().cloned();
        chain.push(here);
    }
    (chain, start)
}

/// `range` without the blanks and line breaks at its ends.
fn trimmed(text: &str, range: Range<usize>) -> Range<usize> {
    let inside = &text[range.clone()];
    let start = range.start + (inside.len() - inside.trim_start().len());
    let end = range.end - (inside.len() - inside.trim_end().len());
    start..end.max(start)
}

/// How long the mark is that closes `node` (an equation, text set off)
/// if one does, of the kind `mark` that also opens it: none where it is
/// not closed, and none where its opening mark is all of it.
fn closing(node: &LinkedNode, mark: SyntaxKind) -> usize {
    let mut children = node.children();
    let opened = children.next().is_some_and(|first| first.kind() == mark);
    match children.next_back() {
        Some(last) if opened && last.kind() == mark => last.range().len(),
        _ => 0,
    }
}

/// The text object named `name` around byte `at`: its inside, or with
/// `around` all of it.
///
/// - `$` or `e`: an equation, with or without its dollar signs
/// - `h`: a section, with or without its heading
/// - `l`: a list item, with or without its marker
/// - `a`: an argument of a call, with or without its comma
/// - `f`: a function call, or what is between its brackets
/// - `*` and `_`: strong and emphasised text, with or without the marks
#[must_use]
pub fn object(source: &Source, name: char, at: usize, around: bool) -> Option<Range<usize>> {
    let text = source.text();
    let chain = ancestors(source, at.min(text.len().saturating_sub(1)));
    let nearest = |kinds: &[SyntaxKind]| chain.iter().find(|node| kinds.contains(&node.kind()));
    match name {
        '$' | 'e' => {
            let node = nearest(&[SyntaxKind::Equation])?;
            let equation = node.range();
            if around {
                return Some(equation);
            }
            // An equation not closed runs to the end of the text, which
            // is its inside then.
            let inside = equation.start + 1..equation.end - closing(node, SyntaxKind::Dollar);
            (inside.start <= inside.end).then(|| trimmed(text, inside))
        }
        '*' | '_' => {
            let kind = if name == '*' {
                SyntaxKind::Strong
            } else {
                SyntaxKind::Emph
            };
            let node = nearest(&[kind])?;
            let marked = node.range();
            if around {
                return Some(marked);
            }
            // Text set off and not closed runs to where its paragraph
            // ends, which is its inside then.
            let mark = if name == '*' {
                SyntaxKind::Star
            } else {
                SyntaxKind::Underscore
            };
            Some(marked.start + 1..marked.end - closing(node, mark))
        }
        'l' => {
            let kinds = [
                SyntaxKind::ListItem,
                SyntaxKind::EnumItem,
                SyntaxKind::TermItem,
            ];
            let item = nearest(&kinds)?;
            if around {
                return Some(item.range());
            }
            // After the marker and the blanks behind it.
            let marker = item.children().next()?.range().end;
            let body = marker + text[marker..item.range().end].len()
                - text[marker..item.range().end].trim_start().len();
            Some(trimmed(text, body..item.range().end))
        }
        'a' => argument(text, &chain, around),
        'f' => {
            let call = nearest(&[SyntaxKind::FuncCall, SyntaxKind::MathCall])?;
            if around {
                // With the hash that begins it in markup.
                let hash = call
                    .prev_sibling()
                    .filter(|before| before.kind() == SyntaxKind::Hash);
                let start = hash.map_or(call.range().start, |hash| hash.range().start);
                return Some(start..call.range().end);
            }
            // Between the first brackets it has: round ones or square.
            let brackets = call.children().find_map(|child| match child.kind() {
                SyntaxKind::Args | SyntaxKind::MathArgs => child
                    .children()
                    .find(|part| part.kind() == SyntaxKind::LeftParen)
                    .map(|_| child.range())
                    .or_else(|| {
                        child
                            .children()
                            .find(|part| part.kind() == SyntaxKind::ContentBlock)
                            .map(|block| block.range())
                    }),
                _ => None,
            })?;
            let open = text[brackets.clone()].find(['(', '['])? + brackets.start;
            let close = matching(text, open)?;
            Some(open + 1..close)
        }
        'h' => section(source, at, around),
        _ => None,
    }
}

/// The bracket that closes the one at byte `open` of `text`, counting
/// brackets of its kind only (good enough inside one call).
fn matching(text: &str, open: usize) -> Option<usize> {
    let (opening, closing) = match text[open..].chars().next()? {
        '(' => ('(', ')'),
        '[' => ('[', ']'),
        _ => return None,
    };
    let mut depth = 0_usize;
    for (at, c) in text[open..].char_indices() {
        if c == opening {
            depth += 1;
        } else if c == closing {
            depth -= 1;
            if depth == 0 {
                return Some(open + at);
            }
        }
    }
    None
}

/// The argument the innermost node of `chain` is part of.
fn argument(text: &str, chain: &[LinkedNode], around: bool) -> Option<Range<usize>> {
    let is_argument = |node: &&LinkedNode| {
        matches!(
            node.parent_kind(),
            Some(SyntaxKind::Args | SyntaxKind::MathArgs)
        ) && !matches!(
            node.kind(),
            SyntaxKind::LeftParen
                | SyntaxKind::RightParen
                | SyntaxKind::Comma
                | SyntaxKind::Space
                | SyntaxKind::ContentBlock
        )
    };
    let argument = chain.iter().find(is_argument)?;
    let range = argument.range();
    if !around {
        return Some(range);
    }
    // With the comma behind it and the blanks behind that; for the last
    // argument, with the comma before it.
    let after = &text[range.end..];
    let behind = after.trim_start_matches([' ', '\t']);
    if let Some(rest) = behind.strip_prefix(',') {
        let blanks = rest.len() - rest.trim_start_matches([' ', '\t']).len();
        return Some(range.start..text.len() - rest.len() + blanks);
    }
    let before = text[..range.start].trim_end_matches([' ', '\t', '\n']);
    if before.ends_with(',') {
        return Some(before.len() - 1..range.end);
    }
    Some(range)
}

/// The section byte `at` is in: from its heading to the next heading of
/// its level or above; without `around`, what is below the heading.
fn section(source: &Source, at: usize, around: bool) -> Option<Range<usize>> {
    let text = source.text();
    // Every heading: where it starts and how deep it is.
    let mut headings: Vec<(Range<usize>, usize)> = Vec::new();
    let mut stack = vec![LinkedNode::new(source.root())];
    while let Some(node) = stack.pop() {
        if node.kind() == SyntaxKind::Heading {
            let level = text[node.range()].chars().take_while(|c| *c == '=').count();
            headings.push((node.range(), level));
            continue;
        }
        let children: Vec<LinkedNode> = node.children().collect();
        stack.extend(children.into_iter().rev());
    }
    let index = headings.iter().rposition(|(range, _)| range.start <= at)?;
    let (heading, level) = &headings[index];
    let end = headings[index + 1..]
        .iter()
        .find(|(_, other)| other <= level)
        .map_or(text.len(), |(range, _)| range.start);
    if around {
        return Some(heading.start..end);
    }
    let below = text[heading.end..end]
        .find('\n')
        .map_or(end, |at| heading.end + at + 1);
    Some(trimmed(text, below..end))
}

/// The edits that set bytes `range` of `source` off as `format`, or take
/// that away where it is there already. `None` if that cannot be done
/// there.
#[must_use]
pub fn format(
    source: &Source,
    format: Format,
    range: Range<usize>,
) -> Option<Vec<(Range<usize>, String)>> {
    let text = source.text();
    let range = range.start.min(text.len())..range.end.min(text.len());
    match format {
        Format::Strong | Format::Emph | Format::Raw => inline(source, format, range),
        Format::Comment => lines(text, &range, &|_| "// ".to_owned(), &|line| {
            line.strip_prefix("//")
                .map(|rest| rest.strip_prefix(' ').unwrap_or(rest).len())
        }),
        Format::ListItem => lines(text, &range, &|_| "- ".to_owned(), &|line| {
            line.strip_prefix("- ").map(str::len)
        }),
        Format::Deeper | Format::Shallower => headings(text, &range, format == Format::Deeper),
    }
}

/// The lines `range` touches: the bytes of each without its line break.
fn touched(text: &str, range: &Range<usize>) -> Vec<Range<usize>> {
    let first = text[..range.start].rfind('\n').map_or(0, |at| at + 1);
    let mut lines = Vec::new();
    let mut start = first;
    loop {
        let end = text[start..].find('\n').map_or(text.len(), |at| start + at);
        lines.push(start..end);
        // A range that ends where a line begins does not touch that line.
        if end + 1 >= range.end.max(range.start + 1) || end >= text.len() {
            break;
        }
        start = end + 1;
    }
    lines
}

/// Toggles a mark at the start of the lines `range` touches: `put` is
/// the mark for a line, `marked` tells how much of a line's text (after
/// its indent) is left once the mark it has is gone. If every line that
/// is not blank has the mark, it is taken from all; else it is put on
/// those that lack it, all at the same indent.
fn lines(
    text: &str,
    range: &Range<usize>,
    put: &dyn Fn(&str) -> String,
    marked: &dyn Fn(&str) -> Option<usize>,
) -> Option<Vec<(Range<usize>, String)>> {
    let lines: Vec<(usize, &str)> = touched(text, range)
        .into_iter()
        .map(|line| {
            let whole = &text[line.clone()];
            let body = whole.trim_start_matches([' ', '\t']);
            (line.end - body.len(), body)
        })
        .filter(|(_, body)| !body.is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }
    let all_marked = lines.iter().all(|(_, body)| marked(body).is_some());
    let mut edits = Vec::new();
    for (start, body) in lines {
        match (all_marked, marked(body)) {
            (true, Some(left)) => edits.push((start..start + body.len() - left, String::new())),
            (false, None) => edits.push((start..start, put(body))),
            _ => {}
        }
    }
    Some(edits)
}

/// Makes the headings among the lines `range` touches one level deeper
/// (and headings of the other lines) or shallower (and plain lines of
/// headings of the first level).
fn headings(text: &str, range: &Range<usize>, deeper: bool) -> Option<Vec<(Range<usize>, String)>> {
    let mut edits = Vec::new();
    for line in touched(text, range) {
        let whole = &text[line.clone()];
        let level = whole.chars().take_while(|c| *c == '=').count();
        let is_heading = level > 0 && whole[level..].starts_with(' ');
        match (is_heading, deeper) {
            (true, true) => edits.push((line.start..line.start, "=".to_owned())),
            (true, false) if level > 1 => edits.push((line.start..line.start + 1, String::new())),
            (true, false) => edits.push((line.start..line.start + 2, String::new())),
            (false, true) if !whole.trim().is_empty() => {
                edits.push((line.start..line.start, "= ".to_owned()));
            }
            (false, _) => {}
        }
    }
    (!edits.is_empty()).then_some(edits)
}

/// The item of a list that byte `at` is in: its indent and its marker.
fn list_item(source: &Source, at: usize) -> Option<(String, String)> {
    let text = source.text();
    let chain = ancestors(source, at.min(text.len().saturating_sub(1)));
    let item = chain
        .iter()
        .find(|node| matches!(node.kind(), SyntaxKind::ListItem | SyntaxKind::EnumItem))?;
    let line = text[..item.offset()].rfind('\n').map_or(0, |at| at + 1);
    let indent: String = text[line..item.offset()]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let marker = text[item.range()].chars().next()?;
    // A numbered item is followed by one that numbers itself.
    let marker = if marker == '-' { "-" } else { "+" };
    Some((indent, marker.to_owned()))
}

/// The paragraph (or heading, or item) byte `at` is in: the lines around
/// it up to the nearest empty ones, without their last line break.
fn block(text: &str, at: usize) -> Range<usize> {
    let at = at.min(text.len());
    let start = text[..at].rfind("\n\n").map_or(0, |found| found + 2);
    let end = text[at..].find("\n\n").map_or_else(
        || text.trim_end_matches('\n').len().max(at),
        |found| at + found,
    );
    start..end.max(start)
}

/// The paragraph byte `at` of `source` is in, as [`block`] has it, but
/// without the lines next to it that are things of their own however
/// close they stand (see [`own_line`]): a heading, an item, a rule.
fn paragraph(source: &Source, at: usize) -> Range<usize> {
    let text = source.text();
    let block = block(text, at);
    let line_start = |at: usize| text[..at].rfind('\n').map_or(0, |found| found + 1);
    let line_end = |at: usize| text[at..].find('\n').map_or(text.len(), |found| at + found);
    let here = line_start(at.min(text.len()));
    if here < block.start {
        return block;
    }
    if own_line(source, here) {
        // A heading is its line.
        return here..line_end(here).min(block.end).max(here);
    }
    let mut start = here;
    while start > block.start {
        let before = line_start(start - 1);
        if own_line(source, before) {
            break;
        }
        start = before;
    }
    let mut end = line_end(here);
    while end < block.end {
        if own_line(source, end + 1) {
            break;
        }
        end = line_end(end + 1);
    }
    start..end.min(block.end).max(start)
}

/// Whether the line of `source` that begins at byte `start` is a thing
/// of its own, not part of a paragraph it stands against: a heading, an
/// item of a list, a rule (`#set`, `#show`), a binding, an import, or a
/// line of one of these.
fn own_line(source: &Source, start: usize) -> bool {
    let Some(line) = source.text().get(start..) else {
        return false;
    };
    let mut first = start + (line.len() - line.trim_start_matches([' ', '\t']).len());
    // The hash that begins code is not part of what it begins.
    if source
        .text()
        .get(first..)
        .is_some_and(|rest| rest.starts_with('#'))
    {
        first += 1;
    }
    ancestors(source, first).iter().any(|node| {
        matches!(
            node.kind(),
            SyntaxKind::Heading
                | SyntaxKind::ListItem
                | SyntaxKind::EnumItem
                | SyntaxKind::TermItem
                | SyntaxKind::SetRule
                | SyntaxKind::ShowRule
                | SyntaxKind::LetBinding
                | SyntaxKind::ModuleImport
                | SyntaxKind::ModuleInclude
        )
    })
}

/// Where and what to put into `source` to begin a new paragraph below
/// the one byte `at` is in (or above it): the byte, the text, and how
/// far into that text the cursor goes. In a list, a new item.
#[must_use]
pub fn open(source: &Source, at: usize, below: bool) -> (usize, String, usize) {
    let text = source.text();
    if let Some((indent, marker)) = list_item(source, at) {
        let at = at.min(text.len());
        // Items are lines here: the line's end, or its start.
        if below {
            let end = text[at..].find('\n').map_or(text.len(), |found| at + found);
            let item = format!("\n{indent}{marker} ");
            let within = item.len();
            return (end, item, within);
        }
        let start = text[..at].rfind('\n').map_or(0, |found| found + 1);
        let item = format!("{indent}{marker} \n");
        let within = item.len() - 1;
        return (start, item, within);
    }
    let block = paragraph(source, at);
    if below {
        (block.end, "\n\n".to_owned(), 2)
    } else {
        (block.start, "\n\n".to_owned(), 0)
    }
}

/// The line byte `at` of `text` is in, if all there is on it is the mark
/// of an item of a list or of a heading: the bytes of the line, and those
/// of the mark with the blanks behind it.
fn bare_mark(text: &str, at: usize) -> Option<(Range<usize>, Range<usize>)> {
    let start = text.get(..at)?.rfind('\n').map_or(0, |found| found + 1);
    let end = text[at..].find('\n').map_or(text.len(), |found| at + found);
    let line = &text[start..end];
    let mark = line.trim();
    let is_mark =
        matches!(mark, "-" | "+" | "/") || (!mark.is_empty() && mark.bytes().all(|b| b == b'='));
    let from = start + (line.len() - line.trim_start().len());
    (is_mark && at >= from + mark.len()).then_some((start..end, from..end))
}

/// What Enter typed at byte `at` of `source` does where only pages are
/// shown: the bytes it replaces and what with. It ends a paragraph or an
/// item of a list, or, in an equation or in code, a line; in an item
/// with nothing in it, it ends the list.
#[must_use]
pub fn line_break(source: &Source, at: usize) -> (Range<usize>, String) {
    let text = source.text();
    let put = |with: &str| (at..at, with.to_owned());
    if ground(source, at) != Ground::Markup {
        return put("\n");
    }
    // Out of what holds no paragraphs: the bytes up to there stay what
    // they are, and the cursor goes behind them.
    if let Some(to) = leave(source, at).filter(|to| *to > at)
        && let Some(between) = text.get(at..to)
    {
        return (at..to, between.to_owned());
    }
    if let Some((line, _)) = bare_mark(text, at) {
        // The mark goes; what follows is a paragraph of its own.
        let after_item = line.start > 0 && !text[..line.start - 1].ends_with('\n');
        return if after_item {
            (line.start - 1..line.end, "\n\n".to_owned())
        } else {
            (line, String::new())
        };
    }
    // Text set off ends with its paragraph: it is closed before the
    // break and begun again behind it.
    let (chain, _) = containing(source, at);
    // At the start of a heading the heading stays one, and moves down:
    // the break goes before its mark, not between it and its text.
    let line = text[..at].rfind('\n').map_or(0, |found| found + 1);
    let mark = text[line..at].trim();
    if chain.iter().any(|node| node.kind() == SyntaxKind::Heading)
        && !mark.is_empty()
        && mark.bytes().all(|b| b == b'=')
    {
        return (line..at, format!("\n\n{}", &text[line..at]));
    }
    let marks: String = chain
        .iter()
        .filter_map(|node| match node.kind() {
            SyntaxKind::Strong => Some('*'),
            SyntaxKind::Emph => Some('_'),
            _ => None,
        })
        .collect();
    let closing: String = marks.chars().rev().collect();
    match list_item(source, at.saturating_sub(1)) {
        Some((indent, marker)) => put(&format!("{closing}\n{indent}{marker} {marks}")),
        None => put(&format!("{closing}\n\n{marks}")),
    }
}

/// What a place in the text is, for what is typed there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ground {
    /// Running text: what is typed is what is to be read.
    Markup,
    /// An equation: what is typed is how Typst writes mathematics.
    Math,
    /// Code: the arguments of a call, a rule.
    Code,
    /// A string in code or in an equation.
    Str,
    /// A name: of a label, of what is referred to; a link.
    Name,
    /// Text that is shown as it is written, or not at all.
    Raw,
}

/// The nodes of `source` that byte `at` is inside of (not at an end of),
/// from the outside in, and with them what the place is.
fn containing(source: &Source, at: usize) -> (Vec<LinkedNode<'_>>, Ground) {
    containing_from(LinkedNode::new(source.root()), at, Ground::Markup)
}

/// What [`containing`] finds from `node` in, the place `node` is in
/// being `ground`.
fn containing_from(
    mut node: LinkedNode<'_>,
    at: usize,
    mut ground: Ground,
) -> (Vec<LinkedNode<'_>>, Ground) {
    let mut chain = Vec::new();
    loop {
        ground = ground_of(node.get(), ground);
        let inside = node
            .children()
            .find(|child| child.offset() < at && at < child.offset() + child.len());
        chain.push(node);
        match inside {
            Some(child) => node = child,
            None => return (chain, ground),
        }
    }
}

/// What [`containing`] finds below the root of `source`: the nodes of
/// the child of the root that byte `at` is inside of, from it in, each a
/// node of that child taken for a root, which begins at the byte given
/// (see [`reach`]); and what the place is. The root is not gone through.
fn containing_below(source: &Source, at: usize) -> (Vec<LinkedNode<'_>>, Ground, usize) {
    let ground = ground_of(source.root(), Ground::Markup);
    match reach::child_inside(source, at) {
        Some((child, start)) => {
            let (chain, ground) = containing_from(LinkedNode::new(child), at - start, ground);
            (chain, ground, start)
        }
        None => (Vec::new(), ground, 0),
    }
}

/// What the place in `node` is, the place `node` is in being `ground`.
fn ground_of(node: &typst::syntax::SyntaxNode, ground: Ground) -> Ground {
    match node.kind() {
        SyntaxKind::Markup
        | SyntaxKind::Strong
        | SyntaxKind::Emph
        | SyntaxKind::Heading
        | SyntaxKind::ListItem
        | SyntaxKind::EnumItem
        | SyntaxKind::TermItem
        | SyntaxKind::ContentBlock => Ground::Markup,
        SyntaxKind::Equation
        | SyntaxKind::Math
        | SyntaxKind::MathCall
        | SyntaxKind::MathArgs
        | SyntaxKind::MathDelimited
        | SyntaxKind::MathAttach
        | SyntaxKind::MathFrac
        | SyntaxKind::MathRoot
        | SyntaxKind::MathPrimes => Ground::Math,
        SyntaxKind::Str => Ground::Str,
        SyntaxKind::Raw | SyntaxKind::LineComment | SyntaxKind::BlockComment => Ground::Raw,
        SyntaxKind::Label | SyntaxKind::Ref | SyntaxKind::RefMarker | SyntaxKind::Link => {
            Ground::Name
        }
        SyntaxKind::Text
        | SyntaxKind::Space
        | SyntaxKind::Escape
        | SyntaxKind::Shorthand
        | SyntaxKind::SmartQuote
        | SyntaxKind::MathText
        | SyntaxKind::MathIdent
        | SyntaxKind::MathFieldAccess
        | SyntaxKind::MathShorthand
        | SyntaxKind::MathAlignPoint
        | SyntaxKind::Error => ground,
        // A blank, a break, a mark: part of what it stands in.
        _ if node.children().len() == 0 => ground,
        _ => Ground::Code,
    }
}

/// What the place at byte `at` of `source` is, for what is typed there.
#[must_use]
pub fn ground(source: &Source, at: usize) -> Ground {
    containing_below(source, at).1
}

/// Whether byte `at` of `text` begins a line (after its indent) that goes
/// on a paragraph begun on the line before it.
fn goes_on(text: &str, at: usize) -> bool {
    let line = text[..at].rfind('\n').map_or(0, |found| found + 1);
    if line == 0 || !text[line..at].trim().is_empty() {
        return false;
    }
    let before = text[..line - 1].rfind('\n').map_or(0, |found| found + 1);
    let before = text[before..line - 1].trim_start();
    !before.is_empty() && !before.starts_with(['=', '-', '+', '/', '#'])
}

/// What typing `c` at byte `at` of `source` does where only pages are
/// shown: the bytes to replace, the text to put there and how far into it
/// the cursor goes. `None` where it is just `c` that goes into the text.
///
/// In running text, what is typed is what is to be read: the characters
/// Typst gives a meaning to are written so that they have none. What
/// stays is what one types to say what one wants: `$` begins an equation,
/// `#` asks what to put in, `@` refers, and `=`, `-` and `+` before a
/// paragraph make it a heading or an item of a list.
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "one case after the other of what is typed where"
)]
pub fn typed(source: &Source, at: usize, c: char) -> Option<(Range<usize>, String, usize)> {
    let text = source.text();
    let plain = |with: String| {
        let within = with.len();
        Some((at..at, with, within))
    };
    let before = text.get(..at)?;
    let last = before.chars().next_back();
    let leaf = LinkedNode::new(source.root()).leaf_at(at, Side::Before);
    let behind = leaf.as_ref().filter(|leaf| leaf.range().end == at);
    let ends_text = || {
        LinkedNode::new(source.root())
            .leaf_at(at, Side::After)
            .is_some_and(|leaf| leaf.kind() == SyntaxKind::Str && leaf.range().end == at + 1)
    };
    match ground(source, at) {
        // Between an equation and its label, what is typed goes behind
        // the label, a blank too: before it, it would take the label from
        // the equation, and what refers to the equation would refer to it.
        Ground::Markup
            if c != '\n'
                && let Some(end) = label_behind(source, at) =>
        {
            let (range, with, within) =
                typed(source, end, c).unwrap_or((end..end, c.to_string(), c.len_utf8()));
            let label = text.get(at..range.start)?;
            let within = label.len() + within;
            return Some((at..range.end, format!("{label}{with}"), within));
        }
        Ground::Markup => {}
        // Over the quotation mark that ends text in an equation.
        Ground::Str if c == '"' && ends_text() && is_in_equation(source, at) => return None,
        Ground::Str if matches!(c, '"' | '\\') => return plain(format!("\\{c}")),
        Ground::Str | Ground::Name | Ground::Raw => return plain(c.to_string()),
        // A dollar sign typed in an equation ends it: the cursor goes
        // behind the one that closes it, as over one right behind it.
        Ground::Math if c == '$' => {
            let equation = equation_at(source, at)?;
            let rest = text.get(at..equation.end)?;
            let over = rest.len() > 1 && rest.ends_with('$');
            return (over && is_whole_equation(source, &equation))
                .then(|| (at..equation.end, rest.to_owned(), rest.len()));
        }
        // Two slashes, or one and a star, begin a comment, which would
        // take in the rest of the line and the dollar sign on it.
        Ground::Math
            if matches!(c, '/' | '*') && before.ends_with('/') && !before.ends_with("\\/") =>
        {
            return plain(format!("\\{c}"));
        }
        // Text in an equation is begun closed, before what follows it
        // too: one quotation mark alone takes in all behind it.
        Ground::Math if c == '"' && !text[at..].starts_with('"') => {
            return Some((at..at, "\"\"".to_owned(), 1));
        }
        Ground::Math | Ground::Code => return None,
    }
    let escaped = || plain(format!("\\{c}"));
    match behind.map(|leaf| leaf.kind()) {
        // After `#`, a name says what is meant; anything else, that the
        // sign itself is.
        Some(SyntaxKind::Hash) if !(c.is_alphabetic() || c == '_') => {
            let (_, with, within) =
                typed(source, at - 1, c).unwrap_or((0..0, c.to_string(), c.len_utf8()));
            let with = format!("\\#{with}");
            return Some((at - 1..at, with, within + 2));
        }
        Some(SyntaxKind::Hash) => return None,
        // The names of code and of what is referred to go on.
        Some(SyntaxKind::Ident) if matches!(c, '_' | '[' | '(') => return None,
        // Unless the name is that of a label there, and no other label's
        // begins with it and `c`: the reference is whole, and `c` is text
        // behind it.
        Some(SyntaxKind::RefMarker)
            if (c.is_alphanumeric() || matches!(c, '_' | '-'))
                && behind.is_some_and(|marker| {
                    let name = text[marker.range()].trim_start_matches('@');
                    text.contains(&format!("<{name}>")) && !text.contains(&format!("<{name}{c}"))
                }) =>
        {
            return escaped();
        }
        Some(SyntaxKind::RefMarker) if c == '_' => return None,
        // Behind a call (a footnote, a table), `(` would call what it
        // gives.
        Some(SyntaxKind::RightParen | SyntaxKind::RightBracket | SyntaxKind::RightBrace)
            if c == '(' =>
        {
            return escaped();
        }
        // Behind strong text a letter would make its last mark a
        // character like any other: the function says it, then.
        Some(SyntaxKind::Star | SyntaxKind::Underscore) if c.is_alphanumeric() => {
            let marked = behind.and_then(|mark| mark.parent()).filter(|marked| {
                matches!(marked.kind(), SyntaxKind::Strong | SyntaxKind::Emph)
                    && marked.range().end == at
            });
            if let Some(marked) = marked {
                let function = if marked.kind() == SyntaxKind::Strong {
                    "strong"
                } else {
                    "emph"
                };
                let range = marked.range();
                let body = text.get(range.start + 1..range.end - 1)?;
                let with = format!("#{function}[{body}]{c}");
                let within = with.len();
                return Some((range, with, within));
            }
        }
        _ => {}
    }
    let line = before.rfind('\n').map_or(0, |found| found + 1);
    let begins_line = before[line..].trim().is_empty();
    match c {
        // An equation begun is one closed: a dollar sign alone takes all
        // that follows it in its paragraph for mathematics.
        '$' if last != Some('\\') => Some((at..at, "$$".to_owned(), 1)),
        '*' | '_' | '`' | '<' | '[' | ']' | '\\' | '~' => escaped(),
        '@' if last.is_some_and(char::is_alphanumeric) => escaped(),
        // Two of them begin a comment; one before a line, a term.
        '/' if begins_line || (before.ends_with('/') && !before.ends_with("\\/")) => escaped(),
        '=' | '-' | '+' if goes_on(text, at) => escaped(),
        _ => None,
    }
}

/// Where the label ends that stands behind an equation, byte `at` of
/// `source` being between the two, where there is nothing but blanks:
/// `$ x $ <eq>`.
fn label_behind(source: &Source, at: usize) -> Option<usize> {
    let text = source.text();
    let end = text.get(..at)?.trim_end_matches(' ').len();
    let root = LinkedNode::new(source.root());
    let closing = root.leaf_at(end, Side::Before)?;
    let equation = closing.parent().filter(|equation| {
        equation.kind() == SyntaxKind::Equation && equation.range().end == end
    })?;
    let label = equation
        .next_sibling()
        .filter(|label| label.kind() == SyntaxKind::Label)?;
    let between = text.get(end..label.range().start)?;
    between.chars().all(|c| c == ' ').then(|| label.range().end)
}

/// What Ctrl-B and Ctrl-I do while typing at byte `at` of `source`: begin
/// text set off as `format` there, or, at the end of such text, leave it.
/// The bytes to replace, the text to put there, and how far into it the
/// cursor goes.
#[must_use]
pub fn set_off(
    source: &Source,
    at: usize,
    format: Format,
) -> Option<(Range<usize>, String, usize)> {
    let text = source.text();
    let (chain, ground) = containing(source, at);
    let (kind, mark, function, in_math) = match format {
        Format::Strong => (SyntaxKind::Strong, '*', "strong", "bold"),
        Format::Emph => (SyntaxKind::Emph, '_', "emph", "italic"),
        _ => return None,
    };
    // The text set off so that `at` is in: all of it, and what it holds.
    let around = chain.iter().rev().find_map(|node| {
        let range = node.range();
        let whole = text.get(range.clone())?;
        match node.kind() {
            found if found == kind => Some((range.clone(), range.start + 1..range.end - 1)),
            SyntaxKind::FuncCall if whole.starts_with(&format!("{function}[")) => {
                let hash = usize::from(text[..range.start].ends_with('#'));
                let inner = range.start + function.len() + 1..range.end - 1;
                Some((range.start - hash..range.end, inner))
            }
            SyntaxKind::MathCall if whole.starts_with(&format!("{in_math}(")) => Some((
                range.clone(),
                range.start + in_math.len() + 1..range.end - 1,
            )),
            _ => None,
        }
    });
    if let Some((outer, inner)) = around {
        return if text.get(inner.clone())?.trim().is_empty() {
            // Nothing was typed into it: it goes.
            Some((outer, String::new(), 0))
        } else if at == inner.end {
            Some((at..at, String::new(), outer.end - at))
        } else {
            None
        };
    }
    match ground {
        // Apart from a letter before it: one name with it, it would be
        // none Typst knows.
        Ground::Math if text[..at].ends_with(char::is_alphanumeric) => {
            Some((at..at, format!(" {in_math}()"), in_math.len() + 2))
        }
        Ground::Math => Some((at..at, format!("{in_math}()"), in_math.len() + 1)),
        Ground::Markup => {
            let letter = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
            let beside =
                letter(text[..at].chars().next_back()) || letter(text[at..].chars().next());
            if beside {
                Some((at..at, format!("#{function}[]"), function.len() + 2))
            } else {
                Some((at..at, format!("{mark}{mark}"), 1))
            }
        }
        _ => None,
    }
}

/// How many bytes of blanks `text` begins with.
fn blanks(text: &str) -> usize {
    text.len() - text.trim_start_matches([' ', '\t', '\n']).len()
}

/// The places behind byte `at` that Tab leads to in `node` and what it
/// holds: where something is to be filled in, and, in an equation, out of
/// the brackets `at` is in.
fn fields(node: &LinkedNode, text: &str, at: usize, math: bool, found: &mut Vec<usize>) {
    let range = node.range();
    let holds = range.start < at && at < range.end;
    let math = match node.kind() {
        SyntaxKind::Equation => {
            found.push(range.start + 1 + blanks(&text[range.start + 1..range.end]));
            true
        }
        SyntaxKind::ContentBlock => {
            found.push(range.start + 1);
            false
        }
        SyntaxKind::Str => {
            found.push(range.start + 1);
            if math && holds {
                found.push(range.end);
            }
            math
        }
        SyntaxKind::MathArgs => {
            for child in node.children() {
                if matches!(
                    child.kind(),
                    SyntaxKind::LeftParen | SyntaxKind::Comma | SyntaxKind::Semicolon
                ) {
                    let end = child.range().end;
                    found.push(end + blanks(&text[end..range.end.saturating_sub(1).max(end)]));
                }
            }
            if holds {
                found.push(range.end);
            }
            true
        }
        SyntaxKind::MathDelimited => {
            if holds {
                found.push(range.end);
            }
            true
        }
        // The brackets around what is raised, lowered or divided by.
        SyntaxKind::Math if math && holds => {
            let closed = node.children().next_back();
            if closed.is_some_and(|last| last.kind() == SyntaxKind::RightParen) {
                found.push(range.end);
            }
            true
        }
        _ => math,
    };
    for child in node.children() {
        fields(&child, text, at, math, found);
    }
}

/// Where Tab leads from byte `at` of `source`: to the next place to fill
/// in of what `at` is in (an argument, the caption of a figure, a cell of
/// a table), and behind it where there is none left.
#[must_use]
pub fn next_field(source: &Source, at: usize) -> Option<usize> {
    // Out of the name of a label.
    let text = source.text();
    let name = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | ':' | '.');
    if text.get(at..)?.starts_with('>') && text[..at].trim_end_matches(name).ends_with('<') {
        return Some(at + 1);
    }
    let (chain, _) = containing(source, at);
    let stands = |node: &LinkedNode| {
        node.parent_kind() == Some(SyntaxKind::Markup)
            && matches!(
                node.kind(),
                SyntaxKind::Equation | SyntaxKind::FuncCall | SyntaxKind::Strong | SyntaxKind::Emph
            )
    };
    // The innermost thing that stands in running text and holds `at`;
    // or else the one that begins there: Tab leads into it.
    let begun = || {
        let text = source.text();
        let begins = at + usize::from(text.get(at..)?.starts_with('#'));
        let mut node = LinkedNode::new(source.root()).leaf_at(begins, Side::After);
        while let Some(here) = node {
            if here.offset() == begins && stands(&here) {
                return Some(here);
            }
            node = here.parent().cloned();
        }
        None
    };
    let whole = chain.iter().rev().find(|node| stands(node)).cloned();
    let whole = &whole.or_else(begun)?;
    let mut found = Vec::new();
    fields(whole, source.text(), at, false, &mut found);
    let next = found.into_iter().filter(|place| *place > at).min();
    Some(next.unwrap_or(whole.range().end))
}

/// The value a `#set` rule of `source` for `function` gives `key`, as it
/// is written: `rule_value(source, "text", "font")`. Only rules that
/// stand by themselves, not in a block or a call: those hold for all of
/// the file.
#[must_use]
pub fn rule_value(source: &Source, function: &str, key: &str) -> Option<String> {
    let (_, value) = rule_place(source, function, key)?;
    source.text().get(value?).map(str::to_owned)
}

/// Whether `code` is what Typst takes for a language: two or three
/// letters (ISO 639), as `de` or `nds`.
#[must_use]
pub fn is_language(code: &str) -> bool {
    (2..=3).contains(&code.len()) && code.chars().all(|c| c.is_ascii_alphabetic())
}

/// Where `source` sets `key` of `function` by a rule: the brackets of
/// the last rule for `function` that sets `key`, and in them its value;
/// where none does, the brackets of the first rule for `function`.
fn rule_place(
    source: &Source,
    function: &str,
    key: &str,
) -> Option<(Range<usize>, Option<Range<usize>>)> {
    let text = source.text();
    let root = LinkedNode::new(source.root());
    let rules = root
        .children()
        .filter(|node| node.kind() == SyntaxKind::SetRule);
    // The last rule that sets it is the one that holds: one written by
    // hand behind the first would undo a change made in that.
    let mut found = None;
    for rule in rules {
        let names = rule.children().any(|part| {
            matches!(part.kind(), SyntaxKind::Ident | SyntaxKind::FieldAccess)
                && text.get(part.range()) == Some(function)
        });
        let Some(args) = rule
            .children()
            .find(|part| part.kind() == SyntaxKind::Args)
            .filter(|_| names)
        else {
            continue;
        };
        let value = args
            .children()
            .filter(|arg| arg.kind() == SyntaxKind::Named)
            .find(|named| {
                let name = named.children().next();
                name.is_some_and(|name| text.get(name.range()) == Some(key))
            })
            .and_then(|named| named.children().next_back().map(|value| value.range()));
        match value {
            Some(_) => found = Some((args.range(), value)),
            None if found.is_none() => found = Some((args.range(), None)),
            None => {}
        }
    }
    found
}

/// The change to `source` that makes `key` of `function` be `value` from
/// here on: in the `#set` rule there is for `function`, or in a new one
/// before everything else (see `new_rule_place`). The bytes to replace,
/// and what with.
#[must_use]
pub fn set_rule(source: &Source, function: &str, key: &str, value: &str) -> (Range<usize>, String) {
    let text = source.text();
    match rule_place(source, function, key) {
        Some((_, Some(old))) => (old, value.to_owned()),
        Some((args, None))
            if text
                .get(args.clone())
                .is_some_and(|args| args.ends_with(')')) =>
        {
            let inside = text
                .get(args.start + 1..args.end.saturating_sub(1))
                .unwrap_or("");
            let written = inside.trim_end();
            let at = args.start + 1 + written.len();
            let between = match written {
                "" => "",
                written if written.ends_with(',') => " ",
                _ => ", ",
            };
            (at..at, format!("{between}{key}: {value}"))
        }
        // A new rule where there is none, or where its brackets are not
        // closed: behind its last argument would be in what follows.
        Some(_) | None => {
            let at = new_rule_place(source);
            let lead = if text[..at].ends_with('\n') || at == 0 {
                ""
            } else {
                "\n"
            };
            (at..at, format!("{lead}#set {function}({key}: {value})\n"))
        }
    }
}

/// Where a new rule goes: before everything else; but behind the last
/// rule that hands all that follows to a template (`#show: …`), which
/// sets its own rules, and over a rule before it.
fn new_rule_place(source: &Source) -> usize {
    let text = source.text();
    let root = LinkedNode::new(source.root());
    let mut at = 0;
    for rule in root.children() {
        if rule.kind() != SyntaxKind::ShowRule {
            continue;
        }
        // `show` and the colon with nothing between: of all that follows.
        let everything = rule
            .children()
            .filter(|part| !part.kind().is_trivia())
            .nth(1)
            .is_some_and(|part| part.kind() == SyntaxKind::Colon);
        if everything {
            let end = rule.range().end;
            at = text[end..]
                .find('\n')
                .map_or(text.len(), |newline| end + newline + 1);
        }
    }
    at
}

/// A table as it is written: a call of `table` (or `grid`) whose cells
/// are all written between square brackets.
pub struct Table {
    /// The bytes of the number of its columns, where that is written as
    /// a number.
    count: Option<Range<usize>>,
    /// How many columns it has.
    pub columns: usize,
    /// The bytes of its cells, brackets and all, row after row.
    pub cells: Vec<Range<usize>>,
    /// The bytes of the call.
    pub whole: Range<usize>,
    /// How many of its cells are those of its header (`table.header`):
    /// they come first, and are whole rows. None where it has no header.
    pub header: usize,
    /// The bytes of the header, where it has one: the call its cells
    /// are written in.
    pub head: Option<Range<usize>>,
}

impl Table {
    /// Which cell byte `at` is in (or at an end of).
    #[must_use]
    pub fn cell_at(&self, at: usize) -> Option<usize> {
        self.cells
            .iter()
            .position(|cell| cell.start < at && at < cell.end)
    }

    /// The cells of the row cell `index` is in.
    fn row_of(&self, index: usize) -> Range<usize> {
        let first = index - index % self.columns;
        first..(first + self.columns).min(self.cells.len())
    }

    /// The byte behind cell `index` and all that is written around it
    /// alone: behind the header, where the cell is the last of that.
    fn behind(&self, index: usize) -> Option<usize> {
        let cell = self.cells.get(index)?;
        match &self.head {
            Some(head) if index + 1 == self.header => Some(head.end),
            _ => Some(cell.end),
        }
    }
}

/// The cells of the header `call` of a table (`named`: `table` or
/// `grid`), if it is one and all it holds are cells written between
/// square brackets.
fn header_cells(call: &LinkedNode, text: &str, named: &str) -> Option<Vec<Range<usize>>> {
    let name = call.children().next()?;
    let is_header = text
        .get(name.range())
        .and_then(|name| name.strip_prefix(named))
        .is_some_and(|rest| rest == ".header");
    let args = call
        .children()
        .find(|part| part.kind() == SyntaxKind::Args)
        .filter(|_| is_header)?;
    let mut cells = Vec::new();
    let mut closed = false;
    for arg in args.children() {
        match arg.kind() {
            // Between the round brackets, not behind them.
            SyntaxKind::ContentBlock if !closed => cells.push(arg.range()),
            SyntaxKind::RightParen => closed = true,
            SyntaxKind::LeftParen | SyntaxKind::Comma | SyntaxKind::Space => {}
            _ => return None,
        }
    }
    (closed && !cells.is_empty()).then_some(cells)
}

/// The table byte `at` of `source` is in, the innermost.
#[must_use]
pub fn table_at(source: &Source, at: usize) -> Option<Table> {
    // Below the root, in the child of it the table is in: its bytes are
    // told from where that begins, and moved by it at the end.
    let (chain, _, start) = containing_below(source, at);
    let text = source.text().get(start..).unwrap_or("");
    let table = chain.iter().rev().find_map(|call| {
        if call.kind() != SyntaxKind::FuncCall {
            return None;
        }
        let name = call.children().next()?;
        let named = text.get(name.range())?;
        if !matches!(named, "table" | "grid") {
            return None;
        }
        let args = call
            .children()
            .find(|part| part.kind() == SyntaxKind::Args)?;
        let mut table = Table {
            count: None,
            columns: 1,
            cells: Vec::new(),
            whole: call.range(),
            header: 0,
            head: None,
        };
        let mut closed = false;
        for arg in args.children() {
            match arg.kind() {
                // A cell behind the round brackets: rows put behind it
                // would be outside the call.
                SyntaxKind::ContentBlock if closed => return None,
                SyntaxKind::ContentBlock => table.cells.push(arg.range()),
                SyntaxKind::RightParen => closed = true,
                // A header of cells, before all others: the first rows.
                SyntaxKind::FuncCall if table.cells.is_empty() => {
                    table.cells = header_cells(&arg, text, named)?;
                    table.header = table.cells.len();
                    table.head = Some(arg.range());
                }
                SyntaxKind::Named => {
                    let key = arg.children().next()?;
                    let value = arg.children().next_back()?;
                    if text.get(key.range()) != Some("columns") {
                        continue;
                    }
                    match value.kind() {
                        SyntaxKind::Int => {
                            table.columns = text.get(value.range())?.parse().ok()?;
                            table.count = Some(value.range());
                        }
                        SyntaxKind::Array => {
                            let item = |part: &LinkedNode| {
                                !matches!(
                                    part.kind(),
                                    SyntaxKind::LeftParen
                                        | SyntaxKind::RightParen
                                        | SyntaxKind::Comma
                                        | SyntaxKind::Space
                                )
                            };
                            table.columns = value.children().filter(item).count();
                        }
                        _ => return None,
                    }
                }
                SyntaxKind::LeftParen | SyntaxKind::Comma | SyntaxKind::Space => {}
                // A line, a footer, a cell that is a call: not for
                // changing blindly.
                _ => return None,
            }
        }
        let whole_rows = table.columns >= 1 && table.header.is_multiple_of(table.columns);
        (whole_rows && !table.cells.is_empty()).then_some(table)
    })?;
    let moved = |range: Range<usize>| range.start + start..range.end + start;
    Some(Table {
        count: table.count.map(moved),
        cells: table.cells.into_iter().map(moved).collect(),
        whole: moved(table.whole),
        head: table.head.map(moved),
        ..table
    })
}

/// The blanks the line byte `at` of `text` is on begins with.
fn indent_of(text: &str, at: usize) -> &str {
    let line = text[..at].rfind('\n').map_or(0, |found| found + 1);
    let rest = &text[line..at];
    &rest[..rest.len() - rest.trim_start_matches([' ', '\t']).len()]
}

/// A row of empty cells behind the last of `table`: where it goes, what
/// it is, and how far into that its first cell is.
#[must_use]
pub fn table_new_row(source: &Source, table: &Table) -> Option<(usize, String, usize)> {
    let text = source.text();
    let last = table.cells.last()?;
    let first = table.cells.get(table.row_of(table.cells.len() - 1).start)?;
    // A last row with cells missing is filled up first.
    let missing = (table.columns - table.cells.len() % table.columns) % table.columns;
    let fill = ", []".repeat(missing);
    let row = vec!["[]"; table.columns].join(", ");
    // Behind a header that is all there is, not in it.
    let (place, begins) = match &table.head {
        Some(head) if table.cells.len() == table.header => (head.end, head.start),
        _ => (last.end, first.start),
    };
    let lead = format!("{fill},\n{}", indent_of(text, begins));
    Some((place, format!("{lead}{row}"), lead.len() + 1))
}

/// The changes to `source` that give `table` one more column, behind
/// the others; from the last to the first, so that each leaves the
/// bytes of those that follow where they are.
#[must_use]
pub fn table_new_column(source: &Source, table: &Table) -> Vec<(Range<usize>, String)> {
    let text = source.text();
    let mut edits = Vec::new();
    for (index, cell) in table.cells.iter().enumerate().rev() {
        let ends_row = (index + 1) % table.columns == 0 || index + 1 == table.cells.len();
        if ends_row {
            edits.push((cell.end..cell.end, ", []".to_owned()));
        }
    }
    match &table.count {
        Some(count) => edits.push((count.clone(), (table.columns + 1).to_string())),
        None => {
            // `columns: (auto, 1fr)`: one more of the last kind.
            if let Some(((), Some(value))) = rule_free_columns(source, table) {
                let inside = text.get(value.start + 1..value.end - 1).unwrap_or("");
                let last = inside.rsplit(',').next().unwrap_or("auto").trim();
                let last = if last.is_empty() { "auto" } else { last };
                edits.push((value.end - 1..value.end - 1, format!(", {last}")));
            }
        }
    }
    edits
}

/// Where `table` says what its columns are, if it does so with a list.
fn rule_free_columns(source: &Source, table: &Table) -> Option<((), Option<Range<usize>>)> {
    let text = source.text();
    let call = LinkedNode::new(source.root()).leaf_at(table.whole.start, Side::After)?;
    let call = call.parent()?;
    let args = call
        .children()
        .find(|part| part.kind() == SyntaxKind::Args)?;
    let value = args
        .children()
        .filter(|arg| arg.kind() == SyntaxKind::Named)
        .find(|named| {
            let name = named.children().next();
            name.is_some_and(|name| text.get(name.range()) == Some("columns"))
        })
        .and_then(|named| named.children().next_back())
        .filter(|value| value.kind() == SyntaxKind::Array)
        .map(|value| value.range());
    Some(((), value))
}

/// The bytes that go with cell `index` of `table` when it is taken out:
/// the cell and what stands between it and the next (or, of the last,
/// the one before).
fn cell_with_mark(table: &Table, index: usize) -> Option<Range<usize>> {
    let cell = table.cells.get(index)?;
    // The last of a row goes with what is before it: the row's end
    // stays where it is.
    let ends_row = (index + 1).is_multiple_of(table.columns) && table.columns > 1;
    match (table.cells.get(index + 1), index.checked_sub(1)) {
        (Some(next), _) if !ends_row => Some(cell.start..next.start),
        (_, Some(before)) => Some(table.behind(before)?..cell.end),
        (Some(next), None) => Some(cell.start..next.start),
        (None, None) => None,
    }
}

/// The changes that take the row cell `index` is in out of `table`, from
/// the last to the first. None if it is the only row.
#[must_use]
pub fn table_without_row(table: &Table, index: usize) -> Vec<(Range<usize>, String)> {
    if table.cells.len() <= table.columns {
        return Vec::new();
    }
    row_bytes(table, index)
        .map(|range| (range, String::new()))
        .into_iter()
        .collect()
}

/// The bytes that go with the row cell `index` of `table` is in when it
/// is taken out: its cells and what stands between them and the next
/// row (or, of the last row, the one before). A header goes whole with
/// its only row.
fn row_bytes(table: &Table, index: usize) -> Option<Range<usize>> {
    let row = table.row_of(index);
    let cells = &table.cells;
    let first = cells.get(row.start)?.start;
    let before = row.start.checked_sub(1);
    if row.end == table.header {
        // The last row of the header: what follows is not in it.
        return match (&table.head, before) {
            (Some(head), None) => Some(head.start..cells.get(row.end)?.start),
            (_, Some(before)) => Some(cells.get(before)?.end..cells.get(row.end - 1)?.end),
            (None, None) => None,
        };
    }
    match (cells.get(row.end), before) {
        (Some(next), _) => Some(first..next.start),
        (None, Some(before)) => Some(table.behind(before)?..cells.last()?.end),
        (None, None) => None,
    }
}

/// The changes that take the column cell `index` is in out of `table`,
/// from the last to the first. None if it is the only column, or the
/// number of columns is not written as a number.
#[must_use]
pub fn table_without_column(table: &Table, index: usize) -> Vec<(Range<usize>, String)> {
    let (Some(count), true) = (&table.count, table.columns > 1) else {
        return Vec::new();
    };
    let column = index % table.columns;
    let mut edits: Vec<(Range<usize>, String)> = (0..table.cells.len())
        .rev()
        .filter(|cell| cell % table.columns == column)
        .filter_map(|cell| Some((cell_with_mark(table, cell)?, String::new())))
        .collect();
    edits.push((count.clone(), (table.columns - 1).to_string()));
    edits
}

/// Whether byte `at` of `source` is a place in running text with nothing
/// written in it: a line with nothing on it (but the mark of an item or
/// a heading), or between brackets with nothing between them.
#[must_use]
pub fn is_empty_spot(source: &Source, at: usize) -> bool {
    let text = source.text();
    let (Some(before), Some(behind)) = (text.get(..at), text.get(at..)) else {
        return false;
    };
    if ground(source, at) != Ground::Markup {
        return false;
    }
    let line = before.rfind('\n').map_or(0, |found| found + 1);
    let begun = before[line..].trim_start_matches(['=', '-', '+', '/', ' ']);
    let opens = begun.is_empty() || before.ends_with('[');
    let rest = behind.trim_start_matches(' ');
    let closes = rest.is_empty() || rest.starts_with(['\n', ']']);
    // A mark needs its blank before anything can follow it.
    let marked = before[line..].trim_start().len() > begun.len();
    opens && closes && (!marked || before.ends_with(' ') || before.ends_with('['))
}

/// The equation byte `at` of `source` is in, the outermost: its bytes.
#[must_use]
pub fn equation_at(source: &Source, at: usize) -> Option<Range<usize>> {
    let last = source.text().len().saturating_sub(1);
    let (chain, start) = ancestors_below(source, at.min(last));
    chain
        .iter()
        .rfind(|node| node.kind() == SyntaxKind::Equation)
        .map(|equation| equation.range().start + start..equation.range().end + start)
}

/// The equation that begins at byte `start`, as whoever wrote it sees
/// it. That is as Typst reads it, unless something in it that is not
/// closed (a string, say) takes in what comes after its paragraph, to
/// the end of the document: then it reaches to the `$` that closes it
/// in its paragraph, or to that paragraph's end.
#[must_use]
pub fn equation_as_seen(source: &Source, start: usize) -> Option<Range<usize>> {
    let read = equation_at(source, start + 1).filter(|read| read.start == start)?;
    let body = source.text().get(read.clone())?;
    // Where its paragraph ends: before a line with nothing on it.
    let paragraph = body.match_indices('\n').find_map(|(at, _)| {
        let line = body[at + 1..].split('\n').next().unwrap_or("");
        line.trim().is_empty().then_some(at)
    });
    let Some(paragraph) = paragraph else {
        return Some(read);
    };
    let within = &body[..paragraph];
    let length = within
        .char_indices()
        .skip(1)
        .find(|(at, c)| *c == '$' && !within[..*at].ends_with('\\'))
        .map_or(within.trim_end().len(), |(at, _)| at + 1);
    Some(start..start + length)
}

/// Whether what Typst reads as the equation at bytes `equation` is one
/// to whoever wrote it: it ends in its paragraph, and no `$` is missing
/// or one too many anywhere (with one, the text between equations is
/// read as equations, to the end). Only then may it be changed for what
/// it is taken to mean.
#[must_use]
pub fn is_whole_equation(source: &Source, equation: &Range<usize>) -> bool {
    // Where something is wrong: an equation not closed. Other faults
    // (a mark that raises nothing, left elsewhere) leave equations be.
    fn unclosed(node: &LinkedNode) -> bool {
        if node.kind() == SyntaxKind::Equation && closing(node, SyntaxKind::Dollar) == 0 {
            return true;
        }
        node.children()
            .any(|child| child.get().diagnosis().errors && unclosed(&child))
    }
    let root = source.root();
    !(root.diagnosis().errors && unclosed(&LinkedNode::new(root)))
        && equation_as_seen(source, equation.start).as_ref() == Some(equation)
}

/// What a deletion of bytes `range` of `source` on a page takes: all of
/// it but the dollar signs of an equation it takes only a part of, the
/// quotation marks of text in one, and the brackets of content. Those
/// show as nothing; one taken
/// with what is shown beside it would leave the other to run on into
/// the text behind it. The parts that go, from the last; none if only
/// such marks were to go.
///
/// The ranges refer to the text as it is: they are apart, and the last
/// comes first so that each is still where it was when it is taken.
#[must_use]
pub fn deleted_on_page(source: &Source, range: &Range<usize>) -> Vec<Range<usize>> {
    fn kept(node: &LinkedNode, range: &Range<usize>, math: bool, found: &mut Vec<usize>) {
        let bytes = node.range();
        let apart = bytes.end <= range.start || range.end <= bytes.start;
        let all = range.start <= bytes.start && bytes.end <= range.end;
        if apart || all {
            return;
        }
        // So are the quotation marks of text in an equation, and the
        // brackets around what a call is given (a cell of a table, a
        // footnote, a caption) that an equation may begin or end.
        let closed = match node.kind() {
            SyntaxKind::Equation => closing(node, SyntaxKind::Dollar) > 0,
            SyntaxKind::Str => math && bytes.len() >= 2 && node.leaf_text().ends_with('"'),
            SyntaxKind::ContentBlock => {
                node.children().next_back().map(|last| last.kind())
                    == Some(SyntaxKind::RightBracket)
            }
            _ => false,
        };
        if closed {
            for mark in [bytes.start, bytes.end - 1] {
                if range.contains(&mark) {
                    found.push(mark);
                }
            }
        }
        let math = math || node.kind() == SyntaxKind::Equation;
        for child in node.children() {
            kept(&child, range, math, found);
        }
    }
    let mut found = Vec::new();
    kept(&LinkedNode::new(source.root()), range, false, &mut found);
    found.sort_unstable();
    let mut parts = Vec::new();
    let mut from = range.start;
    for mark in found {
        if from < mark {
            parts.push(from..mark);
        }
        from = from.max(mark + 1);
    }
    if from < range.end {
        parts.push(from..range.end);
    }
    parts.reverse();
    parts
}

/// The marks in the equation at bytes `equation` of `source` that raise,
/// lower or divide by nothing: each with the brackets it has, if those
/// are empty; and the quotation marks of text with nothing in them. They
/// are what is left when what they held is deleted.
#[must_use]
pub fn empty_scripts(source: &Source, equation: &Range<usize>) -> Vec<Range<usize>> {
    let text = source.text();
    let mut found = Vec::new();
    let Some(node) = LinkedNode::new(source.root()).leaf_at(equation.start, Side::After) else {
        return found;
    };
    let Some(node) = node
        .parent()
        .filter(|node| node.kind() == SyntaxKind::Equation)
    else {
        return found;
    };
    scripts_in(node, text, equation.end, &mut found);
    found
}

fn scripts_in(node: &LinkedNode, text: &str, end: usize, found: &mut Vec<Range<usize>>) {
    // Text with nothing in it shows as nothing too.
    if node.kind() == SyntaxKind::Str && node.leaf_text() == "\"\"" {
        found.push(node.range());
    }
    if matches!(
        node.kind(),
        SyntaxKind::Hat | SyntaxKind::Underscore | SyntaxKind::Slash
    ) {
        let range = node.range();
        let rest = text[range.end..end].trim_start();
        let from = end - rest.len();
        if let Some(inside) = rest.strip_prefix('(')
            && let Some(behind) = inside.trim_start().strip_prefix(')')
        {
            found.push(range.start..end - behind.len());
        } else if rest.is_empty() || rest.starts_with([',', ';', ')', '$', ']', '^', '_']) {
            found.push(range.start..if rest.is_empty() { from } else { range.end });
        } else if node.kind() != SyntaxKind::Slash && text[range.end..end].starts_with(' ') {
            // A blank behind the mark, where what it raised or lowered
            // was: left there, it would raise what follows the blank
            // (the `+` of `x^ + y`). A fraction is written with blanks.
            found.push(range);
        }
    }
    for child in node.children() {
        scripts_in(&child, text, end, found);
    }
}

/// What Backspace takes at byte `at` of `source` where it is in a place
/// of an equation that nothing was filled into: the mark that raises or
/// lowers nothing, with its brackets; a call all of whose arguments are
/// empty.
#[must_use]
pub fn unmake(source: &Source, at: usize) -> Option<(Range<usize>, String)> {
    let text = source.text();
    let gone = |range: Range<usize>| Some((range, String::new()));
    // The last row of a table, if nothing is in it: the cursor goes to
    // the end of the cell before it.
    if let Some(table) = table_at(source, at)
        && let Some(index) = table.cell_at(at)
    {
        let row = table.row_of(index);
        let blank = |cell: &Range<usize>| {
            text.get(cell.start + 1..cell.end - 1)
                .is_some_and(|inside| inside.trim().is_empty())
        };
        let last_row = row.end == table.cells.len() && row.start > 0;
        if last_row && table.cells[row.clone()].iter().all(blank) {
            let before = table.cells.get(row.start - 1)?;
            let last = table.cells.last()?;
            return Some((before.end - 1..last.end, "]".to_owned()));
        }
    }
    if ground(source, at) == Ground::Markup
        && let Some((line, mark)) = bare_mark(text, at)
    {
        // The mark of an item or a heading with nothing behind it; of an
        // item that follows another, the line too.
        let after_item = line.start > 0 && !text[..line.start - 1].ends_with('\n');
        let from = if after_item {
            line.start - 1
        } else {
            mark.start
        };
        return gone(from..at);
    }
    let equation = equation_at(source, at.saturating_sub(1))?;
    if at <= equation.start || at >= equation.end {
        return None;
    }
    let script = empty_scripts(source, &equation)
        .into_iter()
        .find(|script| script.start < at && at <= script.end);
    if let Some(script) = script {
        return gone(script);
    }
    let (chain, _) = containing(source, at);
    chain.iter().rev().find_map(|node| {
        let args = node
            .children()
            .find(|child| child.kind() == SyntaxKind::MathArgs)
            .filter(|_| node.kind() == SyntaxKind::MathCall)?;
        let inside = text.get(args.range().start + 1..args.range().end.checked_sub(1)?)?;
        let empty = inside
            .chars()
            .all(|c| c.is_whitespace() || matches!(c, ',' | ';'));
        (empty && args.range().start < at && at < args.range().end)
            .then(|| (node.range(), String::new()))
    })
}

/// What Tab does at byte `at` of `source`: leads to the next place to
/// fill in (see [`next_field`]), and behind the last cell of a table
/// makes a row for more. The bytes to replace, what with, and how far
/// into that the cursor goes.
#[must_use]
pub fn tab_key(source: &Source, at: usize) -> Option<(Range<usize>, String, usize)> {
    if let Some(table) = table_at(source, at)
        && table
            .cell_at(at)
            .is_some_and(|cell| cell + 1 == table.cells.len())
    {
        let (place, row, within) = table_new_row(source, &table)?;
        return Some((place..place, row, within));
    }
    next_field(source, at).map(|next| (next..next, String::new(), 0))
}

/// What is written between square brackets and is over with Enter: one
/// line of something, not paragraphs.
const ONE_LINE: &[&str] = &[
    "footnote",
    "link",
    "strong",
    "emph",
    "underline",
    "strike",
    "highlight",
    "super",
    "sub",
    "smallcaps",
];

/// Where Enter leads from byte `at` of `source` where it is in something
/// that holds no paragraphs: out of a cell of a table (to what follows
/// the table), out of a caption or a footnote.
fn leave(source: &Source, at: usize) -> Option<usize> {
    let text = source.text();
    if let Some(table) = table_at(source, at)
        && table.cell_at(at).is_some()
    {
        return Some(next_field(source, table.whole.end).unwrap_or(table.whole.end));
    }
    let (chain, _) = containing(source, at);
    let block = chain
        .iter()
        .rfind(|node| node.kind() == SyntaxKind::ContentBlock)?;
    let parent = block.parent()?;
    let call = parent.parent()?;
    let one_line = match parent.kind() {
        SyntaxKind::Named => true,
        SyntaxKind::Args => {
            let name = call.children().next()?;
            ONE_LINE.contains(&text.get(name.range())?)
        }
        _ => false,
    };
    one_line.then(|| next_field(source, at).unwrap_or(block.range().end))
}

/// What stands for a part of an equation that is yet to be typed.
pub const EMPTY: &str = "\u{25a1}";

/// The places of the equation at bytes `equation` of `source` that are
/// still empty (an argument, what is raised or lowered, what is divided
/// by): with nothing in them it would be no equation, or one that does
/// not show what it lacks. In order, by byte of `source`.
#[must_use]
pub fn empty_places(source: &Source, equation: &Range<usize>) -> Vec<usize> {
    lacking(source, equation, false)
}

/// How many words and characters of running text `source` has: what is
/// read, not what is written to make it look as it does. Equations and
/// code are not counted.
#[must_use]
pub fn count(source: &Source) -> (usize, usize) {
    fn gather(node: &LinkedNode, text: &str, read: &mut String) {
        match node.kind() {
            SyntaxKind::Text => read.push_str(text.get(node.range()).unwrap_or("")),
            SyntaxKind::Space
            | SyntaxKind::Parbreak
            | SyntaxKind::Linebreak
            | SyntaxKind::Equation
            | SyntaxKind::Raw => read.push(' '),
            _ => {
                for child in node.children() {
                    gather(&child, text, read);
                }
            }
        }
    }
    let mut read = String::new();
    gather(&LinkedNode::new(source.root()), source.text(), &mut read);
    let words = read.split_whitespace().count();
    let characters = read.chars().filter(|c| !c.is_whitespace()).count();
    (words, characters)
}

/// The file `source` names as its bibliography, if it names one: the
/// first, where it names several.
#[must_use]
pub fn bibliography_file(source: &Source) -> Option<String> {
    fn find(node: &LinkedNode, text: &str) -> Option<String> {
        if node.kind() == SyntaxKind::FuncCall {
            let name = node.children().next()?;
            if text.get(name.range()) == Some("bibliography") {
                let args = node
                    .children()
                    .find(|part| part.kind() == SyntaxKind::Args)?;
                // The file, or the first of a list of files.
                let first = args.children().find_map(|arg| match arg.kind() {
                    SyntaxKind::Str => Some(arg.range()),
                    SyntaxKind::Array => arg
                        .children()
                        .find(|item| item.kind() == SyntaxKind::Str)
                        .map(|item| item.range()),
                    _ => None,
                })?;
                return text
                    .get(first.start + 1..first.end - 1)
                    .map(|named| named.replace("\\\"", "\"").replace("\\\\", "\\"));
            }
        }
        node.children().find_map(|child| find(&child, text))
    }
    thread_local! {
        /// The children of the root of the text last asked about that
        /// name a file, with the file. Asked after every compile: a walk
        /// through all of a long text, unless kept.
        static KEPT: std::cell::RefCell<Option<(Source, kept::Found<String>)>> =
            const { std::cell::RefCell::new(None) };
    }
    let text = source.text();
    let named = |node: &typst::syntax::SyntaxNode, start: usize| {
        let within = text.get(start..start + node.len()).unwrap_or("");
        find(&LinkedNode::new(node), within)
            .map(|named| (start..start + node.len(), named))
            .into_iter()
            .collect()
    };
    let found = KEPT.with_borrow(|kept| kept::found_again(kept.as_ref(), source, &named));
    let first = found.first().map(|(_, named)| named.clone());
    KEPT.set(Some((source.clone(), found)));
    first
}

/// What a part of `source` that begins at byte `at` needs before it to
/// be typeset by itself as it is in the whole: the numbers the headings,
/// equations and figures before it have counted up to, and a rule for
/// references to what is not in the part (they show what they refer to
/// by its name, for want of the thing).
#[must_use]
pub fn lead_for_part(source: &Source, at: usize) -> String {
    let text = source.text();
    let mut levels: Vec<usize> = Vec::new();
    let (mut equations, mut images, mut tables) = (0, 0, 0);
    // The rules, definitions and imports among the text before the part:
    // the part is cut down to the rules before all of the text.
    let (mut begun, mut among) = (false, String::new());
    let mut offset = 0;
    for child in source.root().children() {
        if offset >= at {
            break;
        }
        let written = text.get(offset..offset + child.len()).unwrap_or("");
        let rule = matches!(
            child.kind(),
            SyntaxKind::SetRule
                | SyntaxKind::ShowRule
                | SyntaxKind::LetBinding
                | SyntaxKind::ModuleImport
        );
        if rule && begun {
            let _ = writeln!(among, "#{written}");
        }
        begun |= !rule
            && !matches!(
                child.kind(),
                SyntaxKind::Hash
                    | SyntaxKind::Space
                    | SyntaxKind::Parbreak
                    | SyntaxKind::LineComment
                    | SyntaxKind::BlockComment
                    | SyntaxKind::Semicolon
            );
        match child.kind() {
            SyntaxKind::Heading => {
                let level = written
                    .bytes()
                    .take_while(|byte| *byte == b'=')
                    .count()
                    .max(1);
                levels.truncate(level);
                levels.resize(level, 0);
                levels[level - 1] += 1;
            }
            SyntaxKind::Equation if written.starts_with("$ ") || written.starts_with("$\n") => {
                equations += 1;
            }
            SyntaxKind::FuncCall if written.starts_with("figure(") => {
                if written.contains("table(") {
                    tables += 1;
                } else if written.contains("image(") {
                    images += 1;
                }
            }
            _ => {}
        }
        offset += child.len();
    }
    // On a page of the size of the document's, which Typst's margins are
    // a share of; with a mark of where its text ends.
    let mut lead = among;
    lead.push_str(zetwerk_typst::local::FOOT_MARK);
    lead.push_str(
        "#show ref: it => context { if query(it.target).len() > 0 { it } \
         else { text(str(it.target)) } }\n",
    );
    let notes = text
        .get(..at)
        .map_or(0, |before| before.matches("#footnote").count());
    if notes > 0 {
        let _ = writeln!(lead, "#counter(footnote).update({notes})");
    }
    if !levels.is_empty() {
        let counted: Vec<String> = levels.iter().map(usize::to_string).collect();
        let _ = writeln!(lead, "#counter(heading).update(({},))", counted.join(", "));
    }
    for (what, count) in [
        ("math.equation", equations),
        ("figure.where(kind: image)", images),
        ("figure.where(kind: table)", tables),
    ] {
        if count > 0 {
            let _ = writeln!(lead, "#counter({what}).update({count})");
        }
    }
    lead
}

/// What a part of `source` (the bytes `kept`) needs before it so that
/// its references to what is not in it show what they show in the whole:
/// the equation, heading or figure referred to, hidden and taking no
/// room, with the numbers counted up to where it stands. `None` where one
/// of them cannot be put so (a source of a bibliography, an element in
/// another file or not at the top of the text).
#[must_use]
pub fn stand_ins(source: &Source, kept: &Range<usize>) -> Option<String> {
    fn labels(node: &LinkedNode, kept: &Range<usize>, out: &mut Vec<String>) {
        let range = node.range();
        if range.end <= kept.start || range.start >= kept.end {
            return;
        }
        if node.kind() == SyntaxKind::RefMarker {
            out.push(node.get().leaf_text().trim_start_matches('@').to_owned());
        }
        for child in node.children() {
            labels(&child, kept, out);
        }
    }
    let text = source.text();
    let part = text.get(kept.clone())?;
    let mut referred = Vec::new();
    if part.contains('@') {
        // Only the children of the root the part is in are gone into.
        for (child, start) in reach::children_over(source, kept) {
            let kept = kept.start.saturating_sub(start)..kept.end - start;
            labels(&LinkedNode::new(child), &kept, &mut referred);
        }
    }
    referred.sort();
    referred.dedup();
    let mut out = String::new();
    for label in referred {
        let mark = format!("<{label}>");
        if part.contains(&mark) {
            continue;
        }
        // The child of the root it is the label of: a heading may hold
        // it; an equation, a figure or a heading has it behind it.
        let (mut offset, mut start) = (0, None);
        let mut found = None;
        for child in source.root().children() {
            let written = text.get(offset..offset + child.len()).unwrap_or("");
            match child.kind() {
                SyntaxKind::Heading if written.contains(&mark) => {
                    found = Some(offset..offset + child.len());
                }
                SyntaxKind::Equation | SyntaxKind::Heading | SyntaxKind::Hash => {
                    start = Some(offset);
                }
                SyntaxKind::FuncCall
                    if written.starts_with("figure(")
                        && (written.contains("image(") || written.contains("table(")) => {}
                SyntaxKind::Space => {}
                SyntaxKind::Label if written == mark => {
                    found = start.map(|start| start..offset + child.len());
                }
                _ => start = None,
            }
            if found.is_some() {
                break;
            }
            offset += child.len();
        }
        let element = found?;
        let written = text.get(element.clone())?;
        let figure = written.starts_with("#figure(");
        if !(figure || written.starts_with('$') || written.starts_with('=')) {
            return None;
        }
        // The numbers before it, as for a part that begins there.
        for line in lead_for_part(source, element.start).lines() {
            if line.starts_with("#counter(") {
                let _ = writeln!(out, "{line}");
            }
        }
        let _ = writeln!(out, "#place(hide[{written}])");
    }
    Some(out)
}

/// Whether `name` is something in an equation: a symbol, a function or
/// an operator Typst knows there, or something `source` defines or
/// brings in itself.
fn is_named(source: &Source, name: &str) -> bool {
    use std::sync::OnceLock;
    use typst::{Library, LibraryExt};
    static LIBRARY: OnceLock<Library> = OnceLock::new();
    let library = LIBRARY.get_or_init(Library::default);
    if library.math.scope().get(name).is_some() || library.global.scope().get(name).is_some() {
        return true;
    }
    // What the text says it is: `#let name`, or a name it imports. One
    // that imports all of something may have any name from there.
    let text = source.text();
    let defined = text.match_indices("let ").any(|(at, _)| {
        let rest = &text[at + 4..];
        rest.strip_prefix(name)
            .is_some_and(|behind| !behind.starts_with(|c: char| c.is_alphanumeric() || c == '-'))
    });
    let imports = || {
        text.lines()
            .filter(|line| line.contains("import "))
            .any(|line| line.contains('*') || line.contains(name))
    };
    defined || imports()
}

/// The words of the equation at bytes `equation` of `source` that name
/// nothing, each as its bytes: `xy`, where `x y` is meant. Typst reads
/// letters that stand together as one name and refuses an equation for
/// a name it does not know. `but` is a byte a word is being typed at,
/// if one is: that word may yet become a name.
#[must_use]
pub fn unknown_names(
    source: &Source,
    equation: &Range<usize>,
    but: Option<usize>,
) -> Vec<Range<usize>> {
    fn gather(node: &LinkedNode, found: &mut Vec<Range<usize>>) {
        if node.kind() == SyntaxKind::MathIdent
            && node.parent_kind() != Some(SyntaxKind::MathFieldAccess)
        {
            found.push(node.range());
        }
        for child in node.children() {
            gather(&child, found);
        }
    }
    let text = source.text();
    let mut found = Vec::new();
    let leaf = LinkedNode::new(source.root()).leaf_at(equation.start, Side::After);
    if let Some(equation) = leaf
        .as_ref()
        .and_then(LinkedNode::parent)
        .filter(|node| node.kind() == SyntaxKind::Equation)
    {
        gather(equation, &mut found);
    }
    found.retain(|name| {
        !but.is_some_and(|but| name.start < but && but <= name.end)
            && text.get(name.clone()).is_some_and(|word| {
                word.chars().all(char::is_alphanumeric) && !is_named(source, word)
            })
    });
    found
}

/// `word` as the letters it is made of, each by itself: what somebody
/// who types `xy` in an equation means. A number in it stays one
/// (`a12` is `a 12`).
#[must_use]
pub fn spelled_out(word: &str) -> String {
    let mut spelled = String::with_capacity(2 * word.len());
    let mut last = None;
    for c in word.chars() {
        let number = last.is_some_and(|last: char| last.is_ascii_digit()) && c.is_ascii_digit();
        if last.is_some() && !number {
            spelled.push(' ');
        }
        spelled.push(c);
        last = Some(c);
    }
    spelled
}

/// The equations of `source`, the outermost: their bytes.
#[must_use]
pub fn equations(source: &Source) -> Vec<Range<usize>> {
    fn gather(node: &LinkedNode, found: &mut Vec<Range<usize>>) {
        if node.kind() == SyntaxKind::Equation {
            found.push(node.range());
            return;
        }
        for child in node.children() {
            gather(&child, found);
        }
    }
    let mut found = Vec::new();
    gather(&LinkedNode::new(source.root()), &mut found);
    found
}

/// The places of an equation that is not being typed which lack what
/// they cannot be without: what was begun there and left. Fewer than
/// [`empty_places`] finds: only those Typst would refuse the equation
/// for, so that no equation that is one is shown otherwise.
#[must_use]
pub fn left_empty(source: &Source, equation: &Range<usize>) -> Vec<usize> {
    lacking(source, equation, true)
}

/// The places of all equations of `source` but the one at bytes `but`
/// that lack what they cannot be without (see [`left_empty`]), in the
/// order of the text. One walk through the text, for every key: nothing
/// in it looks anything up. Only through the paragraphs a key changed:
/// what was found in the others is kept (see [`kept`]).
#[must_use]
pub fn left_empty_everywhere(source: &Source, but: Option<&Range<usize>>) -> Vec<usize> {
    use typst::syntax::SyntaxNode;
    /// What the walk carries along: the places of the equation it is
    /// in, and those of the equations before, each with its bytes and
    /// its places told from its start.
    struct Walk<'a> {
        text: &'a str,
        places: Vec<usize>,
        equations: kept::Found<Vec<usize>>,
    }
    /// The places of the equation `node` (at byte `start`) holds.
    fn gaps(
        walk: &mut Walk,
        node: &SyntaxNode,
        start: usize,
        within: (SyntaxKind, bool),
        end: usize,
    ) {
        let (parent, needs_all) = within;
        let lacks = match node.kind() {
            SyntaxKind::Hat | SyntaxKind::Underscore | SyntaxKind::Slash => true,
            SyntaxKind::Comma | SyntaxKind::Semicolon => {
                parent == SyntaxKind::MathArgs && needs_all
            }
            SyntaxKind::LeftParen => match parent {
                SyntaxKind::MathArgs => needs_all,
                SyntaxKind::Math => true,
                _ => false,
            },
            _ => false,
        };
        if lacks {
            let rest = walk.text[start + node.len()..end].trim_start();
            if rest.is_empty() || rest.starts_with('$') {
                walk.places.push(start + node.len());
            } else if rest.starts_with([',', ';', ')', ']', '^', '_']) {
                walk.places.push(end - rest.len());
            }
        }
        if lacks_base(node.kind(), node.leaf_text()) {
            walk.places.push(start);
        }
        // The arguments of a call are told what the call is.
        let needs_all = match node.kind() {
            SyntaxKind::MathCall => node
                .children()
                .next()
                .and_then(|name| walk.text.get(start..start + name.len()))
                .is_some_and(|name| NEEDS_ALL.contains(&name)),
            SyntaxKind::MathArgs => needs_all,
            _ => false,
        };
        let mut at = start;
        for child in node.children() {
            gaps(walk, child, at, (node.kind(), needs_all), end);
            at += child.len();
        }
    }
    /// Through all of `node` (at byte `start`), down to its equations.
    fn through(walk: &mut Walk, node: &SyntaxNode, start: usize) {
        if node.kind() == SyntaxKind::Equation {
            let bytes = start..start + node.len();
            let empty = walk
                .text
                .get(bytes.start + 1..bytes.end.saturating_sub(1))
                .is_none_or(|body| body.trim().is_empty());
            if !empty {
                gaps(walk, node, start, (SyntaxKind::Markup, false), bytes.end);
            }
            if !walk.places.is_empty() {
                let places = std::mem::take(&mut walk.places);
                let places = places.into_iter().map(|at| at - bytes.start).collect();
                walk.equations.push((bytes, places));
            }
            return;
        }
        let mut at = start;
        for child in node.children() {
            if child.children().len() > 0 {
                through(walk, child, at);
            }
            at += child.len();
        }
    }
    thread_local! {
        /// The equations with places of the text last asked about.
        static KEPT: std::cell::RefCell<Option<(Source, kept::Found<Vec<usize>>)>> =
            const { std::cell::RefCell::new(None) };
    }
    let find = |node: &SyntaxNode, start: usize| {
        let mut walk = Walk {
            text: source.text(),
            places: Vec::new(),
            equations: Vec::new(),
        };
        through(&mut walk, node, start);
        walk.equations
    };
    let equations = KEPT.with_borrow(|kept| kept::found_again(kept.as_ref(), source, &find));
    let mut places: Vec<usize> = equations
        .iter()
        .filter(|(bytes, _)| but != Some(bytes))
        .flat_map(|(bytes, places)| places.iter().map(|at| bytes.start + at))
        .collect();
    KEPT.set(Some((source.clone(), equations)));
    places.sort_unstable();
    places.dedup();
    places
}

/// Whether byte `at` of `source` is in what a call of `function` holds:
/// in a footnote, say.
#[must_use]
pub fn is_in_call(source: &Source, at: usize, function: &str) -> bool {
    let (chain, _, start) = containing_below(source, at);
    let text = source.text().get(start..).unwrap_or("");
    chain.iter().any(|node| {
        node.kind() == SyntaxKind::FuncCall
            && node
                .children()
                .next()
                .is_some_and(|name| text.get(name.range()) == Some(function))
    })
}

/// The functions of equations that are nothing without each of their
/// arguments.
const NEEDS_ALL: &[&str] = &[
    "frac",
    "sqrt",
    "root",
    "binom",
    "bold",
    "italic",
    "upright",
    "bb",
    "cal",
    "frak",
    "mono",
    "sans",
    "serif",
    "hat",
    "tilde",
    "overline",
    "underline",
    "dot",
    "ddot",
    "arrow",
    "norm",
    "abs",
    "floor",
    "ceil",
    "round",
    "underbrace",
    "overbrace",
    "cancel",
    "op",
    "accent",
];

fn lacking(source: &Source, equation: &Range<usize>, sure: bool) -> Vec<usize> {
    let text = source.text();
    let mut places = Vec::new();
    let Some(node) = LinkedNode::new(source.root()).leaf_at(equation.start, Side::After) else {
        return places;
    };
    let Some(node) = node
        .parent()
        .filter(|node| node.kind() == SyntaxKind::Equation)
    else {
        return places;
    };
    // Where nothing stands behind byte `at`: the place for something.
    let ends = |at: usize| {
        let rest = text[at..equation.end].trim_start();
        let nothing = rest.is_empty() || rest.starts_with([',', ';', ')', '$', ']', '^', '_']);
        // Before the blank an equation ends with, not behind it: with
        // no blank before its last `$`, an equation that stood by
        // itself would stand in its line.
        let closes = rest.is_empty() || rest.starts_with('$');
        nothing.then(|| {
            if closes {
                at
            } else {
                equation.end - rest.len()
            }
        })
    };
    let inside = equation.start + 1..equation.end.saturating_sub(1);
    if text
        .get(inside.clone())
        .is_some_and(|body| body.trim().is_empty())
    {
        if !sure {
            places.push(inside.start + usize::from(inside.len() > 1));
        }
        return places;
    }
    empty_in(node, &ends, sure.then_some(text), &mut places);
    places.sort_unstable();
    places.dedup();
    places
}

/// With `sure` (the text), only what Typst would refuse: of calls, only
/// those that need all their arguments.
fn empty_in(
    node: &LinkedNode,
    ends: &dyn Fn(usize) -> Option<usize>,
    sure: Option<&str>,
    places: &mut Vec<usize>,
) {
    let end = node.range().end;
    // Whether the call these are the brackets of may lack an argument.
    let argument = || {
        let Some(text) = sure else {
            return true;
        };
        let name = node
            .parent()
            .and_then(LinkedNode::parent)
            .and_then(|call| call.children().next())
            .and_then(|name| text.get(name.range()));
        name.is_some_and(|name| NEEDS_ALL.contains(&name))
    };
    let lacks = match node.kind() {
        SyntaxKind::Hat | SyntaxKind::Underscore | SyntaxKind::Slash => true,
        SyntaxKind::Comma | SyntaxKind::Semicolon => {
            node.parent_kind() == Some(SyntaxKind::MathArgs) && argument()
        }
        // Brackets with nothing in them: of a call, or of what is
        // raised, lowered or divided by.
        SyntaxKind::LeftParen => match node.parent_kind() {
            Some(SyntaxKind::MathArgs) => argument(),
            Some(SyntaxKind::Math) => true,
            _ => false,
        },
        _ => false,
    };
    if lacks {
        places.extend(ends(end));
    }
    if lacks_base(node.kind(), node.leaf_text()) {
        places.push(node.range().start);
    }
    for child in node.children() {
        empty_in(&child, ends, sure, places);
    }
}

/// Whether a node of `kind` with the text `text` is a mark that raises,
/// lowers or divides with nothing before it, which Typst refuses: what
/// is left of `x^2` when `x` is deleted.
fn lacks_base(kind: SyntaxKind, text: &str) -> bool {
    kind == SyntaxKind::Error && matches!(text, "^" | "_" | "/")
}

/// Something to put into a document from the insert menu, or by its
/// short name and Tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Insert {
    /// What it is listed as and found by.
    pub name: &'static str,
    /// A short name of lower-case letters that Tab expands, if it has one.
    pub short: &'static str,
    /// What is put into the text; `|` is where the cursor goes.
    pub body: &'static str,
    /// Whether it stands by itself, as a paragraph does, and not in a
    /// line of text.
    pub block: bool,
}

const fn block_of(name: &'static str, short: &'static str, body: &'static str) -> Insert {
    Insert {
        name,
        short,
        body,
        block: true,
    }
}

const fn in_line(name: &'static str, short: &'static str, body: &'static str) -> Insert {
    Insert {
        name,
        short,
        body,
        block: false,
    }
}

/// What can be put into running text.
pub const TEXT_INSERTS: &[Insert] = &[
    block_of("equation", "eq", "$ | $"),
    block_of(
        "numbered equation",
        "neq",
        "#math.equation(block: true, numbering: \"(1)\", $ | $)",
    ),
    in_line("inline equation", "ieq", "$|$"),
    block_of(
        "figure",
        "fig",
        "#figure(\n  image(\"|\"),\n  caption: [],\n)",
    ),
    block_of(
        "table",
        "tab",
        "#figure(\n  table(\n    columns: 2,\n    [|], [],\n    [], [],\n  ),\n  caption: [],\n)",
    ),
    block_of("image", "img", "#image(\"|\")"),
    block_of("heading", "", "= |"),
    block_of("subheading", "", "== |"),
    block_of("subsubheading", "", "=== |"),
    block_of("list", "", "- |"),
    block_of("numbered list", "", "+ |"),
    block_of("terms", "", "/ |: "),
    in_line("footnote", "fn", "#footnote[|]"),
    in_line("link", "link", "#link(\"|\")[]"),
    in_line("reference", "", "@|"),
    in_line("label", "", "<|>"),
    in_line("citation", "", "#cite(<|>)"),
    block_of("code", "code", "```|\n```"),
    block_of("quote", "quote", "#quote(block: true)[|]"),
    block_of("page break", "", "#pagebreak()|"),
    block_of("contents", "", "#outline()|"),
    block_of("bibliography", "", "#bibliography(\"|\")"),
    block_of("columns", "", "#columns(2)[|]"),
    block_of("centred", "", "#align(center)[|]"),
    block_of("space", "", "#v(1em)|"),
    block_of("rule", "", "#line(length: 100%)|"),
    in_line("bold", "", "*|*"),
    in_line("emphasis", "", "_|_"),
    in_line("underline", "", "#underline[|]"),
    in_line("strikethrough", "", "#strike[|]"),
    in_line("highlight", "", "#highlight[|]"),
    in_line("superscript", "", "#super[|]"),
    in_line("subscript", "", "#sub[|]"),
    in_line("small capitals", "", "#smallcaps[|]"),
    in_line("date", "", "#datetime.today().display()|"),
    in_line("dollar sign", "", "\\$|"),
    in_line("hash sign", "", "\\#|"),
    in_line("at sign", "", "\\@|"),
    in_line("non-breaking space", "", "~|"),
];

/// And into equations.
pub const MATH_INSERTS: &[Insert] = &[
    in_line("fraction", "ff", "frac(|, )"),
    in_line("square root", "sq", "sqrt(|)"),
    in_line("root", "", "root(|, )"),
    in_line("sum", "sum", "sum_(i = 1)^n |"),
    in_line("product", "prod", "product_(i = 1)^n |"),
    in_line("integral", "int", "integral_a^b | dif x"),
    in_line("limit", "lim", "lim_(n -> oo) |"),
    in_line("vector", "vec", "vec(|, )"),
    in_line("matrix", "mat", "mat(|, ; , )"),
    in_line("cases", "case", "cases(|, )"),
    in_line("partial derivative", "pd", "(partial |)/(partial x)"),
    in_line("binomial", "", "binom(|, )"),
    in_line("text", "", "\"|\""),
    in_line("aligned", "", "&= |"),
    in_line("bold", "", "bold(|)"),
    in_line("upright", "", "upright(|)"),
    in_line("blackboard bold", "bb", "bb(|)"),
    in_line("calligraphic", "cal", "cal(|)"),
    in_line("hat", "", "hat(|)"),
    in_line("tilde", "", "tilde(|)"),
    in_line("bar", "", "overline(|)"),
    in_line("dot", "", "dot(|)"),
    in_line("arrow", "", "arrow(|)"),
    in_line("norm", "", "norm(|)"),
    in_line("absolute value", "", "abs(|)"),
    in_line("underbrace", "", "underbrace(|, )"),
    in_line("overbrace", "", "overbrace(|, )"),
];

/// Whether byte `at` of `source` is in an equation.
#[must_use]
pub fn is_in_equation(source: &Source, at: usize) -> bool {
    let at = at.min(source.text().len().saturating_sub(1));
    ancestors_below(source, at)
        .0
        .iter()
        .any(|node| node.kind() == SyntaxKind::Equation)
}

/// `body` of an [`Insert`] as the text to put in and how far into it the
/// cursor goes. One that stands by itself gets a paragraph of its own:
/// where text of its paragraph is before the place it goes to (`before`)
/// or behind it, the paragraph is ended there.
#[must_use]
pub fn insert_text(insert: &Insert, before: bool, behind: bool) -> (String, usize) {
    let lead = if insert.block && before { "\n\n" } else { "" };
    let trail = if insert.block && behind { "\n\n" } else { "" };
    let cursor = lead.len() + insert.body.find('|').unwrap_or(insert.body.len());
    let text = format!("{lead}{}{trail}", insert.body.replacen('|', "", 1));
    (text, cursor)
}

/// Whether the paragraph of byte `at` of `text` has anything but blanks
/// before byte `from`, and behind byte `at`.
#[must_use]
pub fn around(text: &str, from: usize, at: usize) -> (bool, bool) {
    let block = block(text, from.min(at));
    let before = text
        .get(block.start..from)
        .is_some_and(|before| !before.trim().is_empty());
    let behind = text
        .get(at..block.end.max(at))
        .is_some_and(|behind| !behind.trim().is_empty());
    (before, behind)
}

/// What the word that ends at byte `at` of `source` stands for, if it is
/// the name of a snippet where it stands (in running text or in an
/// equation; code has none): the bytes of the word, the text to put in
/// their place, and how far into that text the cursor goes.
#[must_use]
pub fn snippet(source: &Source, at: usize) -> Option<(Range<usize>, String, usize)> {
    let text = source.text();
    let before = text.get(..at)?;
    let word = before.len()
        - before
            .trim_end_matches(|c: char| c.is_ascii_lowercase())
            .len();
    let start = at - word;
    if word == 0
        || before[..start]
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric)
    {
        return None;
    }
    let chain = ancestors(source, start);
    let leaf = chain.first()?;
    let snippets = if chain.iter().any(|node| node.kind() == SyntaxKind::Equation) {
        MATH_INSERTS
    } else if leaf.kind() == SyntaxKind::Text {
        TEXT_INSERTS
    } else {
        return None;
    };
    let name = &text[start..at];
    let template = snippets.iter().find(|insert| insert.short == name)?.body;
    // In the indent of the line it is on.
    let line = text[..start].rfind('\n').map_or(0, |at| at + 1);
    let indent: String = text[line..start]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let expanded = template.replace('\n', &format!("\n{indent}"));
    let cursor = expanded.find('|')?;
    Some((start..at, expanded.replacen('|', "", 1), cursor))
}

/// Strong, emphasised or raw text: marks around `range`, or none where
/// there are some.
fn inline(
    source: &Source,
    format: Format,
    range: Range<usize>,
) -> Option<Vec<(Range<usize>, String)>> {
    let text = source.text();
    let range = trimmed(text, range);
    if range.is_empty() || text[range.clone()].contains("\n\n") {
        return None;
    }
    let chain = ancestors(source, range.start);
    let in_math = chain.iter().any(|node| node.kind() == SyntaxKind::Equation);
    if in_math {
        // Not beyond the equation: its dollar sign would be set off too.
        let equation = chain
            .iter()
            .find(|node| node.kind() == SyntaxKind::Equation)?;
        let end = equation.range().end - closing(equation, SyntaxKind::Dollar);
        let range = trimmed(text, range.start..range.end.min(end).max(range.start));
        if range.is_empty() {
            return None;
        }
        return in_equation(text, &chain, format, range);
    }
    // An equation the range ends in is set off whole.
    let range = match equation_at(source, range.end - 1) {
        Some(equation) if equation.start > range.start && equation.end > range.end => {
            range.start..equation.end
        }
        _ => range,
    };
    // Marks are markup: they mean something else, or nothing, in code.
    let in_markup = chain.first().is_some_and(|leaf| {
        matches!(
            leaf.parent_kind(),
            Some(
                SyntaxKind::Markup
                    | SyntaxKind::Strong
                    | SyntaxKind::Emph
                    | SyntaxKind::Heading
                    | SyntaxKind::ListItem
                    | SyntaxKind::EnumItem
                    | SyntaxKind::TermItem
            )
        )
    });
    if !in_markup {
        return None;
    }
    let (kind, mark, function) = match format {
        Format::Strong => (SyntaxKind::Strong, '*', "strong"),
        Format::Emph => (SyntaxKind::Emph, '_', "emph"),
        _ => (SyntaxKind::Raw, '`', "raw"),
    };
    // There already, as exactly this text: off with the marks.
    for node in chain.iter().filter(|node| node.kind() == kind) {
        let outer = node.range();
        let inner = outer.start + 1..outer.end.saturating_sub(1);
        if range == outer || range == inner {
            return Some(vec![
                (outer.start..outer.start + 1, String::new()),
                (outer.end - 1..outer.end, String::new()),
            ]);
        }
    }
    let selected = &text[range.clone()];
    if format == Format::Raw {
        if selected.contains('`') {
            return None;
        }
        return Some(vec![
            (range.start..range.start, "`".to_owned()),
            (range.end..range.end, "`".to_owned()),
        ]);
    }
    // Between two letters a mark is a character like any other: there,
    // the function says it.
    let letter = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    let before = text[..range.start].chars().next_back();
    let after = text[range.end..].chars().next();
    let within_word = (letter(before) && letter(selected.chars().next()))
        || (letter(selected.chars().next_back()) && letter(after));
    let (open, close) = if within_word {
        (format!("#{function}["), "]".to_owned())
    } else {
        (mark.to_string(), mark.to_string())
    };
    Some(vec![
        (range.start..range.start, open),
        (range.end..range.end, close),
    ])
}

/// Bold and upright-or-italic in an equation: `bold(x)` and `italic(x)`.
fn in_equation(
    text: &str,
    chain: &[LinkedNode],
    format: Format,
    range: Range<usize>,
) -> Option<Vec<(Range<usize>, String)>> {
    let function = match format {
        Format::Strong => "bold",
        Format::Emph => "italic",
        _ => return None,
    };
    // There already, around exactly this: off with it.
    for call in chain
        .iter()
        .filter(|node| matches!(node.kind(), SyntaxKind::FuncCall | SyntaxKind::MathCall))
    {
        let outer = call.range();
        let name = format!("{function}(");
        if !text[outer.clone()].starts_with(&name) || !text[outer.clone()].ends_with(')') {
            continue;
        }
        let inner = outer.start + name.len()..outer.end - 1;
        if range == outer || range == inner {
            return Some(vec![
                (outer.start..inner.start, String::new()),
                (inner.end..outer.end, String::new()),
            ]);
        }
    }
    Some(vec![
        (range.start..range.start, format!("{function}(")),
        (range.end..range.end, ")".to_owned()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(text: &str) -> Source {
        Source::detached(text)
    }

    /// The text object `name` around the first `needle` in `text`.
    fn found<'a>(text: &'a str, needle: &str, name: char, around: bool) -> Option<&'a str> {
        let at = text.find(needle)?;
        object(&source(text), name, at, around).map(|range| &text[range])
    }

    /// `text` with `format` put on its first `needle`.
    fn formatted(text: &str, needle: &str, format: Format) -> Option<String> {
        let at = text.find(needle)?;
        let edits = super::format(&source(text), format, at..at + needle.len())?;
        let mut text = text.to_owned();
        for (range, with) in edits.into_iter().rev() {
            text.replace_range(range, &with);
        }
        Some(text)
    }

    /// `text` (its `|` the cursor) after `change` there, the cursor a
    /// `|` again.
    fn after(
        text: &str,
        change: impl Fn(&Source, usize) -> Option<(Range<usize>, String, usize)>,
    ) -> Option<String> {
        let at = text.find('|')?;
        let source = source(&text.replacen('|', "", 1));
        let (range, with, within) = change(&source, at)?;
        let mut text = source.text().to_owned();
        text.replace_range(range.clone(), &with);
        text.insert(range.start + within, '|');
        Some(text)
    }

    fn typing(text: &str, c: char) -> Option<String> {
        after(text, |source, at| typed(source, at, c))
    }

    #[test]
    fn what_is_typed_in_text_is_what_is_read() {
        assert_eq!(typing("a |", '*').as_deref(), Some("a \\*|"));
        assert_eq!(typing("a |\n\nb", '*').as_deref(), Some("a \\*|\n\nb"));
        assert_eq!(typing("snake|", '_').as_deref(), Some("snake\\_|"));
        assert_eq!(typing("a |", '[').as_deref(), Some("a \\[|"));
        assert_eq!(typing("a |", '<').as_deref(), Some("a \\<|"));
        assert_eq!(typing("a |", '~').as_deref(), Some("a \\~|"));
        // Letters are letters.
        assert_eq!(typing("a |", 'b'), None);
        // An address is no reference; a reference is one.
        assert_eq!(typing("me|", '@').as_deref(), Some("me\\@|"));
        assert_eq!(typing("see |", '@'), None);
        assert_eq!(typing("see @fig|", '_'), None);
        // No comments by accident.
        assert_eq!(typing("a /|", '/').as_deref(), Some("a /\\/|"));
        assert_eq!(typing("a |", '/'), None);
        // A heading where a paragraph begins, not in the middle of one.
        assert_eq!(typing("one\n\n|", '='), None);
        assert_eq!(typing("one\n|two", '=').as_deref(), Some("one\n\\=|two"));
        assert_eq!(typing("- one\n|", '-'), None);
    }

    #[test]
    fn the_hash_asks_or_is_itself() {
        assert_eq!(typing("a |", '#'), None);
        assert_eq!(typing("a #|", 'f'), None);
        assert_eq!(typing("number #|", '1').as_deref(), Some("number \\#1|"));
        assert_eq!(typing("a #|", ' ').as_deref(), Some("a \\# |"));
        assert_eq!(typing("a #|", '*').as_deref(), Some("a \\#\\*|"));
        // What the sign begins is begun behind it.
        assert_eq!(typing("a #|", '$').as_deref(), Some("a \\#$|$"));
        assert_eq!(typing("#my|", '_'), None);
    }

    #[test]
    fn equations_code_and_strings_are_typed_as_they_are_written() {
        assert_eq!(typing("$x|$", '_'), None);
        assert_eq!(typing("$x|$", '*'), None);
        assert_eq!(
            typing("#image(\"a|\")", '_').as_deref(),
            Some("#image(\"a_|\")")
        );
        assert_eq!(
            typing("#image(\"a|\")", '"').as_deref(),
            Some("#image(\"a\\\"|\")")
        );
        assert_eq!(typing("#f(|)", '*'), None);
        assert_eq!(typing("#f[a |]", '*').as_deref(), Some("#f[a \\*|]"));
        assert_eq!(typing("$x$|", '*').as_deref(), Some("$x$\\*|"));
        assert_eq!(typing("see <a|>", '_').as_deref(), Some("see <a_|>"));
    }

    #[test]
    fn what_is_typed_behind_an_equation_leaves_it_its_label() {
        assert_eq!(
            typing("$ x $| <eq>\n", 'a').as_deref(),
            Some("$ x $ <eq>a|\n")
        );
        assert_eq!(
            typing("$ x $| <eq>\n", '*').as_deref(),
            Some("$ x $ <eq>\\*|\n")
        );
        // A blank too, and what follows it.
        assert_eq!(
            typing("$ x $| <eq>\n", ' ').as_deref(),
            Some("$ x $ <eq> |\n")
        );
        assert_eq!(
            typing("$ x $ | <eq>\n", 'a').as_deref(),
            Some("$ x $  <eq>a|\n")
        );
        assert_eq!(typing("$ x $|\n\n<eq>\n", 'a'), None);
    }

    #[test]
    fn a_letter_behind_strong_text_keeps_it_strong() {
        assert_eq!(typing("a *b*|", 'c').as_deref(), Some("a #strong[b]c|"));
        assert_eq!(typing("a _b_|", 'c').as_deref(), Some("a #emph[b]c|"));
        assert_eq!(typing("a *b*|", ' '), None);
    }

    fn set(text: &str, format: Format) -> Option<String> {
        after(text, |source, at| set_off(source, at, format))
    }

    #[test]
    fn strong_text_is_begun_and_left_while_typing() {
        assert_eq!(set("a |", Format::Strong).as_deref(), Some("a *|*"));
        assert_eq!(set("a |", Format::Emph).as_deref(), Some("a _|_"));
        assert_eq!(set("a|", Format::Strong).as_deref(), Some("a#strong[|]"));
        // At its end, out of it; with nothing in it, away with it.
        assert_eq!(set("a *b|*", Format::Strong).as_deref(), Some("a *b*|"));
        assert_eq!(set("a *|*", Format::Strong).as_deref(), Some("a |"));
        assert_eq!(
            set("a#strong[b|]", Format::Strong).as_deref(),
            Some("a#strong[b]|")
        );
        assert_eq!(set("a#strong[|]", Format::Strong).as_deref(), Some("a|"));
        // One in the other.
        assert_eq!(set("a *b |*", Format::Emph).as_deref(), Some("a *b _|_*"));
        // In equations.
        assert_eq!(
            set("$x + |$", Format::Strong).as_deref(),
            Some("$x + bold(|)$")
        );
        assert_eq!(
            set("$bold(x|)$", Format::Strong).as_deref(),
            Some("$bold(x)|$")
        );
        // Not one name with the letter before.
        assert_eq!(set("$x|$", Format::Strong).as_deref(), Some("$x bold(|)$"));
        assert_eq!(set("$x|$", Format::Emph).as_deref(), Some("$x italic(|)$"));
        assert_eq!(set("#image(\"|\")", Format::Strong), None);
    }

    fn tab(text: &str) -> Option<String> {
        after(text, |source, at| {
            next_field(source, at).map(|next| (next..next, String::new(), 0))
        })
    }

    #[test]
    fn tab_leads_to_what_is_to_be_filled_in_next() {
        assert_eq!(tab("$frac(a|, )$").as_deref(), Some("$frac(a, |)$"));
        assert_eq!(tab("$frac(a, b|)$").as_deref(), Some("$frac(a, b)|$"));
        assert_eq!(tab("$frac(a, b)|$").as_deref(), Some("$frac(a, b)$|"));
        assert_eq!(
            tab("$ x^(a + b|) + 1 $").as_deref(),
            Some("$ x^(a + b)| + 1 $")
        );
        assert_eq!(tab("$mat(a|, ; , )$").as_deref(), Some("$mat(a, |; , )$"));
        assert_eq!(
            tab("#figure(\n  image(\"a.png|\"),\n  caption: [],\n)").as_deref(),
            Some("#figure(\n  image(\"a.png\"),\n  caption: [|],\n)")
        );
        assert_eq!(
            tab("#figure(\n  image(\"a.png\"),\n  caption: [The |],\n) more").as_deref(),
            Some("#figure(\n  image(\"a.png\"),\n  caption: [The ],\n)| more")
        );
        assert_eq!(
            tab("#table([a|], [], [])").as_deref(),
            Some("#table([a], [|], [])")
        );
        assert_eq!(tab("#link(\"x|\")[]").as_deref(), Some("#link(\"x\")[|]"));
        assert_eq!(tab("a *b|* c").as_deref(), Some("a *b*| c"));
        assert_eq!(
            tab("#footnote[see $x|$ there]").as_deref(),
            Some("#footnote[see $x$| there]")
        );
        assert_eq!(tab("plain| text"), None);
        // Out of a label.
        assert_eq!(
            tab("= Title <sec:one|>").as_deref(),
            Some("= Title <sec:one>|")
        );
        assert_eq!(tab("#figure([a]) <|>").as_deref(), Some("#figure([a]) <>|"));
        // Into what begins at the cursor.
        assert_eq!(tab("a |$frac(, )$").as_deref(), Some("a $|frac(, )$"));
        assert_eq!(
            tab("|#figure(image(\"a.png\"), caption: [])").as_deref(),
            Some("#figure(image(\"|a.png\"), caption: [])")
        );
    }

    #[test]
    #[ignore = "prints a syntax tree"]
    fn tree() {
        fn show(node: &LinkedNode, depth: usize) {
            eprintln!("{}{:?} {:?}", "  ".repeat(depth), node.kind(), node.range());
            for child in node.children() {
                show(&child, depth + 1);
            }
        }
        let text = std::env::var("TREE").unwrap_or_default();
        show(&LinkedNode::new(source(&text).root()), 0);
    }

    /// `text` after `edits`, each of which refers to the text the ones
    /// before it left.
    fn edited(text: &str, edits: Vec<(Range<usize>, String)>) -> String {
        let mut text = text.to_owned();
        for (range, with) in edits {
            text.replace_range(range, &with);
        }
        text
    }

    #[test]
    fn rules_are_set_where_they_are_or_before_everything() {
        let set = |text: &str, function: &str, key: &str, value: &str| {
            edited(text, vec![set_rule(&source(text), function, key, value)])
        };
        assert_eq!(
            set("Text.\n", "text", "size", "12pt"),
            "#set text(size: 12pt)\nText.\n"
        );
        assert_eq!(
            set("#set text(size: 12pt)\nText.\n", "text", "size", "10pt"),
            "#set text(size: 10pt)\nText.\n"
        );
        assert_eq!(
            set(
                "#set text(size: 12pt)\nText.\n",
                "text",
                "font",
                "\"Libertinus Serif\""
            ),
            "#set text(size: 12pt, font: \"Libertinus Serif\")\nText.\n"
        );
        assert_eq!(
            set("#set page()\n", "page", "paper", "\"a5\""),
            "#set page(paper: \"a5\")\n"
        );
        assert_eq!(
            set("#set par(justify: true)\n", "page", "columns", "2"),
            "#set page(columns: 2)\n#set par(justify: true)\n"
        );
        let text = "#set par(justify: true)\n";
        assert_eq!(
            rule_value(&source(text), "par", "justify").as_deref(),
            Some("true")
        );
        assert_eq!(rule_value(&source(text), "par", "leading"), None);
    }

    #[test]
    fn the_rule_that_holds_is_the_one_changed() {
        let set = |text: &str, function: &str, key: &str, value: &str| {
            edited(text, vec![set_rule(&source(text), function, key, value)])
        };
        // Rules written by hand, one behind the other: the last that
        // sets a thing is what holds, and what is listed and changed.
        let text = "#set text(font: \"New Computer Modern\")\n#set text(size: 12pt)\nText.\n";
        assert_eq!(
            rule_value(&source(text), "text", "size").as_deref(),
            Some("12pt")
        );
        assert_eq!(
            set(text, "text", "size", "14pt"),
            "#set text(font: \"New Computer Modern\")\n#set text(size: 14pt)\nText.\n"
        );
        let twice = "#set text(size: 10pt)\n#set text(size: 12pt)\n";
        assert_eq!(
            set(twice, "text", "size", "9pt"),
            "#set text(size: 10pt)\n#set text(size: 9pt)\n"
        );
        // A new rule goes behind a template that all is handed to: one
        // before it would be set over by the template's own.
        let template = "#import \"t.typ\": conf\n#show: conf.with(title: [X])\nBody.\n";
        assert_eq!(
            set(template, "text", "size", "12pt"),
            "#import \"t.typ\": conf\n#show: conf.with(title: [X])\n#set text(size: 12pt)\nBody.\n"
        );
        assert_eq!(
            set("#show: conf", "page", "columns", "2"),
            "#show: conf\n#set page(columns: 2)\n"
        );
        assert_eq!(
            set(
                "#show heading: set text(red)\nBody.\n",
                "text",
                "size",
                "9pt"
            ),
            "#set text(size: 9pt)\n#show heading: set text(red)\nBody.\n"
        );
        // What no rule sets yet goes into the first.
        assert_eq!(
            set(text, "text", "lang", "\"de\""),
            "#set text(font: \"New Computer Modern\", lang: \"de\")\n#set text(size: 12pt)\nText.\n"
        );
    }

    #[test]
    fn tables_get_and_lose_rows_and_columns() {
        let text = "#table(\n  columns: 2,\n  [a], [b],\n  [c], [d],\n)";
        let source = source(text);
        let at = text.find("[d").unwrap_or(0) + 1;
        let Some(table) = table_at(&source, at) else {
            panic!("no table");
        };
        assert_eq!(
            (table.columns, table.cells.len(), table.cell_at(at)),
            (2, 4, Some(3))
        );
        let Some((place, row, within)) = table_new_row(&source, &table) else {
            panic!("no row");
        };
        let mut longer = edited(text, vec![(place..place, row)]);
        longer.insert(place + within, '|');
        assert_eq!(
            longer,
            "#table(\n  columns: 2,\n  [a], [b],\n  [c], [d],\n  [|], [],\n)"
        );
        assert_eq!(
            edited(text, table_new_column(&source, &table)),
            "#table(\n  columns: 3,\n  [a], [b], [],\n  [c], [d], [],\n)"
        );
        assert_eq!(
            edited(text, table_without_row(&table, 3)),
            "#table(\n  columns: 2,\n  [a], [b],\n)"
        );
        assert_eq!(
            edited(text, table_without_row(&table, 0)),
            "#table(\n  columns: 2,\n  [c], [d],\n)"
        );
        assert_eq!(
            edited(text, table_without_column(&table, 0)),
            "#table(\n  columns: 1,\n  [b],\n  [d],\n)"
        );
        assert_eq!(
            edited(text, table_without_column(&table, 3)),
            "#table(\n  columns: 1,\n  [a],\n  [c],\n)"
        );
        // Not one with cells that are calls.
        let odd = "#table(columns: 2, table.cell(colspan: 2)[a], [c], [d])";
        assert!(table_at(&super::tests::source(odd), odd.len() - 3).is_none());
    }

    #[test]
    fn places_with_nothing_in_them_are_known() {
        let empty = |text: &str| {
            let at = text.find('|').unwrap_or(0);
            is_empty_spot(&source(&text.replacen('|', "", 1)), at)
        };
        assert!(empty("One.\n\n|"));
        assert!(empty("One.\n\n|\n\nTwo."));
        assert!(empty("|"));
        assert!(empty("- one\n- |"));
        assert!(empty("= |"));
        assert!(empty("#table([a], [|])"));
        assert!(empty("#figure(image(\"a.png\"), caption: [|])"));
        assert!(!empty("One.|"));
        assert!(!empty("One.\n\n|Two."));
        assert!(!empty("=|"));
        assert!(!empty("$|$"));
        assert!(!empty("#image(\"|\")"));
        assert!(!empty("#f(|)"));
    }

    #[test]
    fn what_is_left_of_what_was_deleted_goes_too() {
        let scripts = |text: &str| {
            let source = source(text);
            let equation = equation_at(&source, 1).unwrap_or(0..0);
            let found = empty_scripts(&source, &equation);
            found
                .into_iter()
                .map(|range| text[range].to_owned())
                .collect::<Vec<String>>()
        };
        assert_eq!(scripts("$ y_i = _ _() $"), ["_", "_()"]);
        assert_eq!(scripts("$ y_i = _  $"), ["_"]);
        assert_eq!(scripts("$x_$"), ["_"]);
        assert_eq!(scripts("$x_( ) + a/$"), ["_( )", "/"]);
        assert_eq!(scripts("$x_i^2 + a/b + f()$"), Vec::<String>::new());
        // Backspace in a place with nothing in it takes the place.
        let back = |text: &str| {
            after(text, |source, at| {
                let (range, left) = unmake(source, at)?;
                Some((range, left, 0))
            })
        };
        assert_eq!(back("$x_(|)$").as_deref(), Some("$x|$"));
        assert_eq!(back("$x^|$").as_deref(), Some("$x|$"));
        assert_eq!(back("$a + frac(|, )$").as_deref(), Some("$a + |$"));
        assert_eq!(back("$a + frac(, |)$").as_deref(), Some("$a + |$"));
        assert_eq!(back("$frac(a, |)$"), None);
        assert_eq!(back("$x_(i|)$"), None);
        assert_eq!(back("a_|b"), None);
    }

    fn empties(text: &str) -> String {
        let source = source(text);
        let start = text.find('$').unwrap_or(0);
        let end = text.rfind('$').map_or(text.len(), |at| at + 1);
        let mut shown = text.to_owned();
        for place in empty_places(&source, &(start..end)).into_iter().rev() {
            shown.insert(place, '?');
        }
        shown
    }

    #[test]
    fn the_file_of_the_sources_is_found() {
        let named = |text: &str| bibliography_file(&source(text));
        assert_eq!(
            named("Text.\n\n#bibliography(\"refs.bib\")\n").as_deref(),
            Some("refs.bib")
        );
        assert_eq!(
            named("#bibliography((\"a.bib\", \"b.yml\"), style: \"apa\")").as_deref(),
            Some("a.bib")
        );
        assert_eq!(named("Text about a bibliography.\n"), None);
    }

    #[test]
    fn an_equation_ends_where_its_writer_sees_it_end() {
        let seen = |text: &str| {
            let source = Source::detached(text);
            let start = text.find('$').unwrap_or(0);
            equation_as_seen(&source, start).and_then(|seen| text.get(seen).map(str::to_owned))
        };
        assert_eq!(seen("A $a + b$ c.\n\nMore."), Some("$a + b$".into()));
        // A string that is not closed goes on to the end for Typst.
        assert_eq!(
            seen("A $a + \" b$ c.\n\nLet $x$ be."),
            Some("$a + \" b$".into())
        );
        assert_eq!(seen("A $a + \" b\n\nLet."), Some("$a + \" b".into()));
        // One that stands by itself over several lines is all of it.
        assert_eq!(seen("$\n  a + b\n$\n\nMore."), Some("$\n  a + b\n$".into()));
    }

    #[test]
    fn letters_that_name_nothing_are_found() {
        let unknown = |text: &str, but: Option<usize>| {
            let source = source(text);
            let equation = equation_at(&source, text.find('$').unwrap_or(0) + 1).unwrap_or(0..0);
            unknown_names(&source, &equation, but)
                .into_iter()
                .map(|name| text[name].to_owned())
                .collect::<Vec<String>>()
        };
        assert_eq!(unknown("$xy + alpha + sin(ab) + x$", None), ["xy", "ab"]);
        // Names Typst knows, and names with something to them, stay.
        assert_eq!(
            unknown("$arrow.r + frac(a, b) + \"xy\" + 2x$", None),
            Vec::<String>::new()
        );
        // What the text defines or brings in is a name.
        assert_eq!(unknown("#let foo = 1\n$foo + qux$", None), ["qux"]);
        assert_eq!(
            unknown("#import \"a.typ\": *\n$foo$", None),
            Vec::<String>::new()
        );
        // The word being typed may yet become one.
        let text = "$xy + alp$";
        assert_eq!(unknown(text, Some(text.len() - 1)), ["xy"]);
        assert_eq!(spelled_out("xyz"), "x y z");
        assert_eq!(spelled_out("a12b"), "a 12 b");
    }

    #[test]
    fn words_are_counted_as_they_are_read() {
        let text = "#set text(size: 11pt)\n= A title\n\nSome *strong* words, $x^2$ and \
                    #footnote[a note].\n";
        assert_eq!(count(&source(text)).0, 8);
    }

    #[test]
    fn what_was_left_unfinished_is_found_and_nothing_else() {
        let left = |text: &str| {
            let source = source(text);
            let mut shown = text.to_owned();
            let mut places: Vec<usize> = equations(&source)
                .iter()
                .flat_map(|equation| left_empty(&source, equation))
                .collect();
            places.sort_unstable();
            for place in places.into_iter().rev() {
                shown.insert(place, '?');
            }
            shown
        };
        assert_eq!(
            left("a $x^$ b $frac(a, )$ c $y_$"),
            "a $x^?$ b $frac(a, ?)$ c $y_?$"
        );
        // What is an equation as it is stays as it is.
        assert_eq!(
            left("$vec(1, 2, )$ $f()$ $$ $mat(1, ; 2, )$"),
            "$vec(1, 2, )$ $f()$ $$ $mat(1, ; 2, )$"
        );
        assert_eq!(left("$sqrt()$"), "$sqrt(?)$");
        // A mark whose base was deleted lacks it.
        assert_eq!(left("a $^2$ b $(_1)$ c $/y$"), "a $?^2$ b $(?_1)$ c $?/y$");
        // All at once, as it is done for every key: the same places.
        for text in [
            "a $^2$ b $frac(_1, a)$ c $/y$",
            "a $x^$ b $frac(a, )$ c $y_$",
            "$vec(1, 2, )$ $f()$ $$ $mat(1, ; 2, )$ and #f[$a/$] - $x^()$",
        ] {
            let source = source(text);
            let mut one_by_one: Vec<usize> = equations(&source)
                .iter()
                .flat_map(|equation| left_empty(&source, equation))
                .collect();
            one_by_one.sort_unstable();
            assert_eq!(left_empty_everywhere(&source, None), one_by_one, "{text}");
        }
        let text = "a $x^$ b $y_$";
        let first = 2..6;
        assert_eq!(left_empty_everywhere(&source(text), Some(&first)), [12]);
    }

    #[test]
    fn what_an_equation_lacks_is_found() {
        assert_eq!(empties("$frac(, )$"), "$frac(?, ?)$");
        assert_eq!(empties("$frac(a, )$"), "$frac(a, ?)$");
        assert_eq!(empties("$sqrt()$"), "$sqrt(?)$");
        assert_eq!(empties("$ ^2 + x$"), "$ ?^2 + x$");
        assert_eq!(empties("$x^$"), "$x^?$");
        // An equation that stands by itself keeps the blank it ends
        // with: without it, it would stand in its line.
        assert_eq!(empties("$ 2^ $"), "$ 2^? $");
        assert_eq!(empties("$ a/ $"), "$ a/? $");
        assert_eq!(empties("$x_ + 1$"), "$x_ + 1$");
        assert_eq!(empties("$a/$"), "$a/?$");
        assert_eq!(empties("$x^()$"), "$x^(?)$");
        assert_eq!(empties("$$"), "$?$");
        assert_eq!(empties("$  $"), "$ ? $");
        assert_eq!(empties("a $mat(1, 2; 3, 4)$ b"), "a $mat(1, 2; 3, 4)$ b");
        assert_eq!(empties("$f(x) + (a)$"), "$f(x) + (a)$");
    }

    #[test]
    fn equations_and_marked_text_are_objects() {
        let text = "Let $ x^2 + y $ be, and *very strong* and _so_ on. \"$5\"\n";
        assert_eq!(found(text, "x^2", '$', false), Some("x^2 + y"));
        assert_eq!(found(text, "y", 'e', true), Some("$ x^2 + y $"));
        assert_eq!(found(text, "Let", '$', true), None);
        assert_eq!(found(text, "strong", '*', false), Some("very strong"));
        assert_eq!(found(text, "strong", '*', true), Some("*very strong*"));
        assert_eq!(found(text, "so", '_', true), Some("_so_"));
        assert_eq!(found(text, "very", '_', true), None);
    }

    #[test]
    fn calls_and_their_arguments_are_objects() {
        let text = "#figure(image(\"a.png\", width: 50%), caption: [A cat]) and #lorem(5).\n";
        assert_eq!(found(text, "a.png", 'a', false), Some("\"a.png\""));
        assert_eq!(found(text, "a.png", 'a', true), Some("\"a.png\", "));
        assert_eq!(found(text, "50%", 'a', false), Some("width: 50%"));
        assert_eq!(found(text, "50%", 'a', true), Some(", width: 50%"));
        assert_eq!(
            found(text, "50%", 'f', true),
            Some("image(\"a.png\", width: 50%)")
        );
        assert_eq!(
            found(text, "50%", 'f', false),
            Some("\"a.png\", width: 50%")
        );
        assert_eq!(found(text, "caption", 'a', false), Some("caption: [A cat]"));
        assert_eq!(found(text, "caption", 'f', true).map(str::len), Some(53));
        assert_eq!(found(text, "lorem", 'f', true), Some("#lorem(5)"));
        assert_eq!(found(text, "and", 'f', true), None);
    }

    #[test]
    fn sections_and_list_items_are_objects() {
        let text = "= One\nIntro.\n\n== Sub\nBody\n- first item\n- second\n\n= Two\nEnd.\n";
        assert_eq!(
            found(text, "Body", 'h', false),
            Some("Body\n- first item\n- second")
        );
        assert_eq!(
            found(text, "Body", 'h', true),
            Some("== Sub\nBody\n- first item\n- second\n\n")
        );
        assert_eq!(
            found(text, "Intro", 'h', true).map(str::len),
            Some(text.find("= Two").unwrap_or(0))
        );
        assert_eq!(found(text, "End", 'h', false), Some("End."));
        assert_eq!(found(text, "first", 'l', false), Some("first item"));
        assert_eq!(found(text, "second", 'l', true), Some("- second"));
        assert_eq!(found(text, "Intro", 'l', true), None);
    }

    #[test]
    fn marks_go_on_and_come_off() {
        let text = "The quick brown fox.\n";
        let bold = formatted(text, "quick brown", Format::Strong);
        assert_eq!(bold.as_deref(), Some("The *quick brown* fox.\n"));
        // Off again, from the inside or with the marks.
        let bold = bold.unwrap_or_default();
        assert_eq!(
            formatted(&bold, "quick brown", Format::Strong).as_deref(),
            Some(text)
        );
        assert_eq!(
            formatted(&bold, "*quick brown*", Format::Strong).as_deref(),
            Some(text)
        );
        assert_eq!(
            formatted(text, "fox", Format::Emph).as_deref(),
            Some("The quick brown _fox_.\n")
        );
        assert_eq!(
            formatted(text, "brown", Format::Raw).as_deref(),
            Some("The quick `brown` fox.\n")
        );
        // In a word the marks would be characters: the function is used.
        assert_eq!(
            formatted(text, "uic", Format::Strong).as_deref(),
            Some("The q#strong[uic]k brown fox.\n")
        );
        // Blanks at the ends stay outside.
        assert_eq!(
            formatted(text, " quick ", Format::Emph).as_deref(),
            Some("The _quick_ brown fox.\n")
        );
        // Not over a paragraph break, and not in code.
        assert_eq!(
            formatted("one\n\ntwo\n", "one\n\ntwo", Format::Strong),
            None
        );
        assert_eq!(formatted("#let x = 12\n", "12", Format::Strong), None);
        assert_eq!(
            formatted("#align(center)[Title]\n", "Title", Format::Strong).as_deref(),
            Some("#align(center)[*Title*]\n")
        );
    }

    #[test]
    fn new_paragraphs_and_items_begin_where_they_belong() {
        let text = "= Title\n\nFirst line\nof a paragraph.\n\n- one\n- two\n\n$ a\n+ b $\n";
        let source = source(text);
        let opened = |needle: &str, below: bool| {
            let (at, with, within) = open(&source, text.find(needle).unwrap_or(0), below);
            let mut text = text.to_owned();
            text.insert_str(at, &with);
            text.insert(at + within, '|');
            text
        };
        assert!(opened("First", true).contains("of a paragraph.\n\n|\n\n- one"));
        assert!(opened("of a", false).contains("= Title\n\n|\n\nFirst line"));
        assert!(opened("Title", true).starts_with("= Title\n\n|\n\nFirst"));
        assert!(opened("one", true).contains("- one\n- |\n- two"));
        assert!(opened("two", false).contains("- one\n- |\n- two"));
        // Enter: a paragraph, an item, or in an equation a line.
        let after =
            |needle: &str| line_break(&source, text.find(needle).unwrap_or(0) + needle.len());
        assert_eq!(after("First line").1, "\n\n");
        assert_eq!(after("- one").1, "\n- ");
        assert_eq!(after("$ a").1, "\n");
        // Enter in an item with nothing in it ends the list; Backspace
        // there takes the item.
        let enter = |text: &str| {
            super::tests::after(text, |source, at| {
                let (range, with) = line_break(source, at);
                let within = with.len();
                Some((range, with, within))
            })
        };
        assert_eq!(
            enter("- one\n- |\n\nNext").as_deref(),
            Some("- one\n\n|\n\nNext")
        );
        assert_eq!(
            enter("#figure([a], caption: [b])|").as_deref(),
            Some("#figure([a], caption: [b])\n\n|")
        );
        let back = |text: &str| {
            super::tests::after(text, |source, at| {
                let (range, left) = unmake(source, at)?;
                Some((range, left, 0))
            })
        };
        // Enter leaves what holds one line; Tab behind the last cell of
        // a table makes a row, Backspace in an empty last row takes it.
        let table = "#figure(\n  table(\n    columns: 2,\n    [a], [b|],\n  ),\n  caption: [],\n)";
        assert_eq!(
            enter(table).as_deref(),
            Some("#figure(\n  table(\n    columns: 2,\n    [a], [b],\n  ),\n  caption: [|],\n)")
        );
        assert_eq!(
            enter("#figure([x], caption: [The |]) more").as_deref(),
            Some("#figure([x], caption: [The ])| more")
        );
        assert_eq!(
            enter("a#footnote[note|] b").as_deref(),
            Some("a#footnote[note]| b")
        );
        assert_eq!(
            enter("#quote(block: true)[one|]").as_deref(),
            Some("#quote(block: true)[one\n\n|]")
        );
        let tabbed = |text: &str| super::tests::after(text, tab_key);
        assert_eq!(
            tabbed("#table(\n  columns: 2,\n  [a], [b|],\n)").as_deref(),
            Some("#table(\n  columns: 2,\n  [a], [b],\n  [|], [],\n)")
        );
        assert_eq!(
            back("#table(\n  columns: 2,\n  [a], [b],\n  [|], [],\n)").as_deref(),
            Some("#table(\n  columns: 2,\n  [a], [b|],\n)")
        );
        assert_eq!(back("#table(\n  columns: 2,\n  [|], [],\n)"), None);
        assert_eq!(back("- one\n- |").as_deref(), Some("- one|"));
        assert_eq!(back("Text.\n\n== |").as_deref(), Some("Text.\n\n|"));
        assert_eq!(back("Text.\n\n- a|"), None);
    }

    #[test]
    fn snippets_know_where_they_are() {
        let expand = |text: &str| {
            let at = text.find('|')?;
            let text = text.replace('|', "");
            let (range, with, cursor) = snippet(&source(&text), at)?;
            let mut text = text.clone();
            text.replace_range(range.clone(), &with);
            text.insert(range.start + cursor, '|');
            Some(text)
        };
        assert_eq!(expand("A fn| here").as_deref(), Some("A #footnote[|] here"));
        assert_eq!(expand("$ a + ff| $").as_deref(), Some("$ a + frac(|, ) $"));
        assert_eq!(
            expand("  fig|\n").as_deref(),
            Some("  #figure(\n    image(\"|\"),\n    caption: [],\n  )\n")
        );
        // Not the end of another word, not where it means nothing, and
        // each kind only where it belongs.
        assert_eq!(expand("A elfn| here"), None);
        assert_eq!(expand("A ff| here"), None);
        assert_eq!(expand("$ fn| $"), None);
        assert_eq!(expand("#let fn| = 1"), None);
        assert_eq!(expand("A | here"), None);
    }

    #[test]
    fn math_has_its_own_bold() {
        let text = "$ A x = b $\n";
        let bold = formatted(text, "A", Format::Strong);
        assert_eq!(bold.as_deref(), Some("$ bold(A) x = b $\n"));
        let bold = bold.unwrap_or_default();
        assert_eq!(formatted(&bold, "A", Format::Strong).as_deref(), Some(text));
        assert_eq!(
            formatted(&bold, "bold(A)", Format::Strong).as_deref(),
            Some(text)
        );
        assert_eq!(
            formatted(text, "b", Format::Emph).as_deref(),
            Some("$ A x = italic(b) $\n")
        );
        assert_eq!(formatted(text, "x", Format::Raw), None);
    }

    #[test]
    fn lines_are_commented_listed_and_made_headings() {
        let text = "one\n  two\n\nthree\n";
        let all = "one\n  two\n\nthree";
        let commented = formatted(text, all, Format::Comment);
        assert_eq!(commented.as_deref(), Some("// one\n  // two\n\n// three\n"));
        let commented = commented.unwrap_or_default();
        assert_eq!(
            formatted(&commented, "// one\n  // two", Format::Comment).as_deref(),
            Some("one\n  two\n\n// three\n")
        );
        // Some with, some without: all get it.
        assert_eq!(
            formatted("// one\ntwo\n", "// one\ntwo", Format::Comment).as_deref(),
            Some("// one\n// two\n")
        );
        assert_eq!(
            formatted(text, "one\n  two", Format::ListItem).as_deref(),
            Some("- one\n  - two\n\nthree\n")
        );
        assert_eq!(
            formatted("- one\n- two\n", "- one\n- two", Format::ListItem).as_deref(),
            Some("one\ntwo\n")
        );
        assert_eq!(formatted("\n\n", "\n", Format::Comment), None);

        assert_eq!(
            formatted(text, "one", Format::Deeper).as_deref(),
            Some("= one\n  two\n\nthree\n")
        );
        assert_eq!(
            formatted("= one\n", "one", Format::Deeper).as_deref(),
            Some("== one\n")
        );
        assert_eq!(
            formatted("== one\n", "one", Format::Shallower).as_deref(),
            Some("= one\n")
        );
        assert_eq!(
            formatted("= one\n", "one", Format::Shallower).as_deref(),
            Some("one\n")
        );
        assert_eq!(formatted("one\n", "one", Format::Shallower), None);
    }
}
