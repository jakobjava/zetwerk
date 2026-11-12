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

/// Short names for longer things to type, in running text. In each, `|`
/// is where the cursor goes.
const MARKUP_SNIPPETS: &[(&str, &str)] = &[
    ("fig", "#figure(\n  image(\"|\"),\n  caption: [],\n)"),
    ("tab", "#table(\n  columns: 2,\n  [|], [],\n)"),
    ("img", "#image(\"|\")"),
    ("link", "#link(\"|\")[]"),
    ("fn", "#footnote[|]"),
    ("eq", "$ | $"),
    ("code", "```|\n```"),
    ("quote", "#quote[|]"),
];

/// And in equations.
const MATH_SNIPPETS: &[(&str, &str)] = &[
    ("ff", "frac(|, )"),
    ("sq", "sqrt(|)"),
    ("sum", "sum_(i = 1)^n |"),
    ("prod", "product_(i = 1)^n |"),
    ("int", "integral_a^b | dif x"),
    ("lim", "lim_(n -> oo) |"),
    ("vec", "vec(|, )"),
    ("mat", "mat(|, ; , )"),
    ("pd", "(partial |)/(partial x)"),
    ("bb", "bb(|)"),
    ("cal", "cal(|)"),
    ("case", "cases(|, )"),
];

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
        MATH_SNIPPETS
    } else if leaf.kind() == SyntaxKind::Text {
        MARKUP_SNIPPETS
    } else {
        return None;
    };
    let name = &text[start..at];
    let (_, template) = snippets.iter().find(|(short, _)| *short == name)?;
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
