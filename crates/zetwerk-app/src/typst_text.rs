//! What the Vim engine leaves to its host: the text objects of Typst (an
//! equation, a section, a list item, an argument, a call) and how Typst
//! writes strong, emphasised and raw text, comments, list items and
//! headings.
//!
//! All of it goes by the syntax tree, not by characters: a `$` in a
//! string is no equation, and bold in math is `bold(x)`, not `*x*`.

use std::ops::Range;

use typst::syntax::{LinkedNode, Side, Source, SyntaxKind};
use zetwerk_vim::Format;

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

/// `range` without the blanks and line breaks at its ends.
fn trimmed(text: &str, range: Range<usize>) -> Range<usize> {
    let inside = &text[range.clone()];
    let start = range.start + (inside.len() - inside.trim_start().len());
    let end = range.end - (inside.len() - inside.trim_end().len());
    start..end.max(start)
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
            let equation = nearest(&[SyntaxKind::Equation])?.range();
            if around {
                return Some(equation);
            }
            let inside = equation.start + 1..equation.end.checked_sub(1)?;
            Some(trimmed(text, inside))
        }
        '*' | '_' => {
            let kind = if name == '*' {
                SyntaxKind::Strong
            } else {
                SyntaxKind::Emph
            };
            let marked = nearest(&[kind])?.range();
            if around {
                return Some(marked);
            }
            Some(marked.start + 1..marked.end.checked_sub(1)?)
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
    let block = block(text, at);
    if below {
        (block.end, "\n\n".to_owned(), 2)
    } else {
        (block.start, "\n\n".to_owned(), 0)
    }
}

/// What Enter typed at byte `at` of `source` puts into the text where
/// only pages are shown: the end of a paragraph, of an item of a list,
/// or, in an equation or in code, of a line.
#[must_use]
pub fn line_break(source: &Source, at: usize) -> String {
    let chain = ancestors(source, at.saturating_sub(1));
    let in_code = chain.iter().any(|node| {
        matches!(
            node.kind(),
            SyntaxKind::Equation | SyntaxKind::Raw | SyntaxKind::CodeBlock | SyntaxKind::Args
        )
    });
    if in_code {
        return "\n".to_owned();
    }
    match list_item(source, at.saturating_sub(1)) {
        Some((indent, marker)) => format!("\n{indent}{marker} "),
        None => "\n\n".to_owned(),
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
    let mut chain = Vec::new();
    let mut node = LinkedNode::new(source.root());
    let mut ground = Ground::Markup;
    loop {
        ground = match node.kind() {
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
            _ if node.get().children().len() == 0 => ground,
            _ => Ground::Code,
        };
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

/// What the place at byte `at` of `source` is, for what is typed there.
#[must_use]
pub fn ground(source: &Source, at: usize) -> Ground {
    containing(source, at).1
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
    match ground(source, at) {
        Ground::Markup => {}
        Ground::Str if matches!(c, '"' | '\\') => return plain(format!("\\{c}")),
        Ground::Str | Ground::Name | Ground::Raw => return plain(c.to_string()),
        Ground::Math | Ground::Code => return None,
    }
    let escaped = || plain(format!("\\{c}"));
    match behind.map(|leaf| leaf.kind()) {
        // After `#`, a name says what is meant; anything else, that the
        // sign itself is.
        Some(SyntaxKind::Hash) if !(c.is_alphabetic() || c == '_') => {
            let (_, with, _) = typed(source, at - 1, c).unwrap_or((0..0, c.to_string(), 0));
            let with = format!("\\#{with}");
            let within = with.len();
            return Some((at - 1..at, with, within));
        }
        Some(SyntaxKind::Hash) => return None,
        // The names of code and of what is referred to go on.
        Some(SyntaxKind::Ident) if matches!(c, '_' | '[' | '(') => return None,
        Some(SyntaxKind::RefMarker) if c == '_' => return None,
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
        '*' | '_' | '`' | '<' | '[' | ']' | '\\' | '~' => escaped(),
        '@' if last.is_some_and(char::is_alphanumeric) => escaped(),
        // Two of them begin a comment; one before a line, a term.
        '/' if begins_line || (before.ends_with('/') && !before.ends_with("\\/")) => escaped(),
        '=' | '-' | '+' if goes_on(text, at) => escaped(),
        _ => None,
    }
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

/// The equation byte `at` of `source` is in, the outermost: its bytes.
#[must_use]
pub fn equation_at(source: &Source, at: usize) -> Option<Range<usize>> {
    let last = source.text().len().saturating_sub(1);
    ancestors(source, at.min(last))
        .iter()
        .rfind(|node| node.kind() == SyntaxKind::Equation)
        .map(LinkedNode::range)
}

/// The marks in the equation at bytes `equation` of `source` that raise,
/// lower or divide by nothing: each with the brackets it has, if those
/// are empty. They are what is left when what they held is deleted.
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
        } else if rest.is_empty() || rest.starts_with([',', ';', ')', '$', ']']) {
            found.push(range.start..if rest.is_empty() { from } else { range.end });
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
pub fn unmake(source: &Source, at: usize) -> Option<Range<usize>> {
    let equation = equation_at(source, at.saturating_sub(1))?;
    if at <= equation.start || at >= equation.end {
        return None;
    }
    let script = empty_scripts(source, &equation)
        .into_iter()
        .find(|script| script.start < at && at <= script.end);
    if script.is_some() {
        return script;
    }
    let (chain, _) = containing(source, at);
    let text = source.text();
    chain.iter().rev().find_map(|node| {
        let args = node
            .children()
            .find(|child| child.kind() == SyntaxKind::MathArgs)
            .filter(|_| node.kind() == SyntaxKind::MathCall)?;
        let inside = text.get(args.range().start + 1..args.range().end.checked_sub(1)?)?;
        let empty = inside
            .chars()
            .all(|c| c.is_whitespace() || matches!(c, ',' | ';'));
        (empty && args.range().start < at && at < args.range().end).then(|| node.range())
    })
}

/// What stands for a part of an equation that is yet to be typed.
pub const EMPTY: &str = "square.stroked";

/// The places of the equation at bytes `equation` of `source` that are
/// still empty (an argument, what is raised or lowered, what is divided
/// by): with nothing in them it would be no equation, or one that does
/// not show what it lacks. In order, by byte of `source`.
#[must_use]
pub fn empty_places(source: &Source, equation: &Range<usize>) -> Vec<usize> {
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
        let nothing = rest.is_empty() || rest.starts_with([',', ';', ')', '$', ']']);
        nothing.then(|| equation.end - rest.len())
    };
    let inside = equation.start + 1..equation.end.saturating_sub(1);
    if text
        .get(inside.clone())
        .is_some_and(|body| body.trim().is_empty())
    {
        places.push(inside.start + usize::from(inside.len() > 1));
        return places;
    }
    empty_in(node, &ends, &mut places);
    places.sort_unstable();
    places.dedup();
    places
}

fn empty_in(node: &LinkedNode, ends: &dyn Fn(usize) -> Option<usize>, places: &mut Vec<usize>) {
    let end = node.range().end;
    let lacks = match node.kind() {
        SyntaxKind::Hat | SyntaxKind::Underscore | SyntaxKind::Slash => true,
        SyntaxKind::Comma | SyntaxKind::Semicolon => {
            node.parent_kind() == Some(SyntaxKind::MathArgs)
        }
        // Brackets with nothing in them: of a call, or of what is
        // raised, lowered or divided by.
        SyntaxKind::LeftParen => matches!(
            node.parent_kind(),
            Some(SyntaxKind::MathArgs | SyntaxKind::Math)
        ),
        _ => false,
    };
    if lacks {
        places.extend(ends(end));
    }
    for child in node.children() {
        empty_in(&child, ends, places);
    }
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
    ancestors(source, at)
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
        return in_equation(text, &chain, format, range);
    }
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
        assert_eq!(scripts("$ y_i = _ _() $"), ["_()"]);
        assert_eq!(scripts("$ y_i = _  $"), ["_"]);
        assert_eq!(scripts("$x_$"), ["_"]);
        assert_eq!(scripts("$x_( ) + a/$"), ["_( )", "/"]);
        assert_eq!(scripts("$x_i^2 + a/b + f()$"), Vec::<String>::new());
        // Backspace in a place with nothing in it takes the place.
        let back = |text: &str| {
            after(text, |source, at| {
                Some((unmake(source, at)?, String::new(), 0))
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
    fn what_an_equation_lacks_is_found() {
        assert_eq!(empties("$frac(, )$"), "$frac(?, ?)$");
        assert_eq!(empties("$frac(a, )$"), "$frac(a, ?)$");
        assert_eq!(empties("$sqrt()$"), "$sqrt(?)$");
        assert_eq!(empties("$x^$"), "$x^?$");
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
        assert_eq!(after("First line"), "\n\n");
        assert_eq!(after("- one"), "\n- ");
        assert_eq!(after("$ a"), "\n");
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
