//! The figures of a file as they are written, and how what is said of
//! them is changed: a caption, a label, the file and the width of an
//! image, the columns, rows and looks of a table.
//!
//! Like its neighbour `typst_text`, all of it goes by the syntax tree,
//! and all of it is text in and text out: a change is the bytes to
//! replace and what with, so that the rest of a call stays as its
//! author wrote it.

use std::ops::Range;

use typst::syntax::{LinkedNode, Source, SyntaxKind};

use crate::typst_text::{self, Table};

/// The blanks within a line.
const BLANKS: [char; 2] = [' ', '\t'];

/// How many rows or columns a table may be asked to have at once.
const MOST: usize = 200;

/// What a figure shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An image, read from a file.
    Image,
    /// A table.
    Table,
    /// Anything else: code, a drawing, text.
    Other,
}

impl Kind {
    /// What it is called.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Table => "table",
            Self::Other => "other",
        }
    }
}

/// A figure as it is written: a call of `figure`, or a table or an
/// image that stands in no figure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Figure {
    /// The byte its call begins at, behind the hash where it has one.
    pub call: usize,
    /// The bytes of all of it: the call with its hash and its label.
    pub whole: Range<usize>,
    /// Whether it is a call of `figure`, and not a table or an image by
    /// itself.
    pub framed: bool,
    /// Whether it stands in running text, begun by a hash: there it can
    /// have a label, and be taken out without leaving a hole in code.
    pub marked: bool,
    /// What it shows.
    pub kind: Kind,
    /// What its number counts ("Figure", "Table", "Listing", or the
    /// kind it names itself) and the number, as far as this file tells:
    /// none for what is not a figure or is not numbered.
    pub number: Option<(String, usize)>,
    /// Its caption, as it is written.
    pub caption: Option<String>,
    /// The name of its label.
    pub label: Option<String>,
    /// The byte the call of the image it shows begins at.
    pub image: Option<usize>,
    /// The file of that image, where it is named by a string, and the
    /// bytes of the string.
    pub file: Option<(Range<usize>, String)>,
    /// The byte the call of the table (or grid) it shows begins at.
    pub table: Option<usize>,
    /// The line it begins on, counting from 1.
    pub line: usize,
}

impl Figure {
    /// What it is called in a list: "Figure 2", "Table 1"; or, of what
    /// has no number, what it is.
    #[must_use]
    pub fn name(&self) -> String {
        match (&self.number, self.framed, self.kind) {
            (Some((counted, number)), ..) => format!("{counted} {number}"),
            (None, true, _) => "figure".to_owned(),
            (None, false, kind) => kind.name().to_owned(),
        }
    }

    /// Whether byte `at` is in it.
    #[must_use]
    pub fn holds(&self, at: usize) -> bool {
        self.whole.start <= at && at < self.whole.end
    }
}

/// The name of the function `call` calls, as it is written.
fn callee<'a>(call: &LinkedNode, text: &'a str) -> Option<&'a str> {
    text.get(call.children().next()?.range())
}

/// The arguments of `call`, brackets and all.
fn args_of<'a>(call: &LinkedNode<'a>) -> Option<LinkedNode<'a>> {
    call.children().find(|part| part.kind() == SyntaxKind::Args)
}

/// Whether `part` of the arguments of a call is an argument, and not
/// what stands between arguments.
fn is_argument(part: &LinkedNode) -> bool {
    !matches!(
        part.kind(),
        SyntaxKind::LeftParen
            | SyntaxKind::RightParen
            | SyntaxKind::Comma
            | SyntaxKind::Space
            | SyntaxKind::LineComment
            | SyntaxKind::BlockComment
    )
}

/// The call of `source` that begins at byte `start`: the outermost,
/// where one begins with another.
fn call_at(source: &Source, start: usize) -> Option<LinkedNode<'_>> {
    let mut node = LinkedNode::new(source.root());
    loop {
        if node.kind() == SyntaxKind::FuncCall && node.offset() == start {
            return Some(node);
        }
        let inside = node
            .children()
            .find(|child| child.offset() <= start && start < child.offset() + child.len())?;
        node = inside;
    }
}

/// The argument `key` of the arguments `args`: all of it, name and
/// value.
fn named<'a>(args: &LinkedNode<'a>, text: &str, key: &str) -> Option<LinkedNode<'a>> {
    args.children()
        .filter(|arg| arg.kind() == SyntaxKind::Named)
        .find(|named| {
            let name = named.children().next();
            name.is_some_and(|name| text.get(name.range()) == Some(key))
        })
}

/// The bytes of the value the call of `source` that begins at byte
/// `call` gives its argument `key`, if it names it.
#[must_use]
pub fn named_value(source: &Source, call: usize, key: &str) -> Option<Range<usize>> {
    let args = args_of(&call_at(source, call)?)?;
    let named = named(&args, source.text(), key)?;
    named.children().next_back().map(|value| value.range())
}

/// What the call of `source` that begins at byte `call` gives its
/// argument `key`, as it is written.
#[must_use]
pub fn named_text<'a>(source: &'a Source, call: usize, key: &str) -> Option<&'a str> {
    source.text().get(named_value(source, call, key)?)
}

/// The change to `source` that makes the call that begins at byte
/// `call` give its argument `key` the `value`; or, with none, name it no
/// more. The bytes to replace, and what with. `None` if there is no
/// such call, or nothing to change.
///
/// An argument that is named already keeps its place. A new one goes
/// behind the last the call names; where it names none, behind the last
/// that is not written between square brackets (the cells of a table
/// and the body of a call stay last), and before all of them where
/// there is no such either. It gets a line of its own where the
/// argument it follows has one.
#[must_use]
pub fn set_named(
    source: &Source,
    call: usize,
    key: &str,
    value: Option<&str>,
) -> Option<(Range<usize>, String)> {
    let text = source.text();
    let args = args_of(&call_at(source, call)?)?;
    match (named(&args, text, key), value) {
        (Some(named), Some(value)) => {
            let old = named.children().next_back()?.range();
            Some((old, value.to_owned()))
        }
        (Some(named), None) => Some((with_comma(text, named.range()), String::new())),
        (None, Some(value)) => Some(new_named(text, &args, &format!("{key}: {value}"))),
        (None, None) => None,
    }
}

/// Whether the line byte `at` of `text` is on has nothing but blanks
/// before it, and is not the first.
fn begins_line(text: &str, at: usize) -> bool {
    text[..at]
        .rfind('\n')
        .is_some_and(|line| text[line + 1..at].trim_matches(BLANKS).is_empty())
}

/// The blanks the line byte `at` of `text` is on begins with.
fn indent_of(text: &str, at: usize) -> &str {
    let line = text[..at].rfind('\n').map_or(0, |found| found + 1);
    let rest = &text[line..at];
    &rest[..rest.len() - rest.trim_start_matches(BLANKS).len()]
}

/// The bytes that go with the argument at `range` of `text` when it is
/// taken out: its line, where it has one of its own; else the argument
/// with the comma behind it, or, of the last, the comma before it.
fn with_comma(text: &str, range: Range<usize>) -> Range<usize> {
    let behind = text[range.end..].trim_start_matches(BLANKS);
    if let Some(rest) = behind.strip_prefix(',') {
        let more = rest.trim_start_matches(BLANKS);
        let line = text[..range.start].rfind('\n').map_or(0, |found| found + 1);
        if begins_line(text, range.start) && more.starts_with('\n') {
            return line..text.len() - more.len() + 1;
        }
        return range.start..text.len() - more.len();
    }
    let before = text[..range.start].trim_end();
    if before.ends_with(',') {
        return before.len() - 1..range.end;
    }
    range
}

/// Where the argument `written` goes among the arguments `args` that do
/// not have it yet, and what is put there.
fn new_named(text: &str, args: &LinkedNode, written: &str) -> (Range<usize>, String) {
    let parts: Vec<LinkedNode> = args.children().collect();
    let open = parts
        .first()
        .filter(|part| part.kind() == SyntaxKind::LeftParen);
    let Some(open) = open else {
        // Only a body between square brackets: round ones before it.
        let at = args.offset();
        return (at..at, format!("({written})"));
    };
    let close = parts
        .iter()
        .position(|part| part.kind() == SyntaxKind::RightParen)
        .unwrap_or(parts.len());
    let inside: Vec<&LinkedNode> = parts
        .iter()
        .take(close)
        .filter(|part| is_argument(part))
        .collect();
    let last_named = inside
        .iter()
        .rev()
        .find(|arg| arg.kind() == SyntaxKind::Named);
    let last_plain = inside
        .iter()
        .rev()
        .find(|arg| arg.kind() != SyntaxKind::ContentBlock);
    if let Some(before) = last_named.or(last_plain) {
        let range = before.range();
        let own_line = begins_line(text, range.start);
        let indent = indent_of(text, range.start);
        let behind = text[range.end..].trim_start_matches(BLANKS);
        let comma = behind
            .starts_with(',')
            .then(|| text.len() - behind.len() + 1);
        return match (comma, own_line) {
            (Some(at), true) => (at..at, format!("\n{indent}{written},")),
            (Some(at), false) => (at..at, format!(" {written},")),
            (None, true) => (range.end..range.end, format!(",\n{indent}{written}")),
            (None, false) => (range.end..range.end, format!(", {written}")),
        };
    }
    let Some(first) = inside.first() else {
        let at = open.range().end;
        return (at..at, written.to_owned());
    };
    let at = first.offset();
    let lead = if begins_line(text, at) {
        format!("{written},\n{}", indent_of(text, at))
    } else {
        format!("{written}, ")
    };
    (at..at, lead)
}

/// A text as Typst writes a string.
#[must_use]
pub fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// What the string written at `range` of `text` says.
fn unquoted(text: &str, range: &Range<usize>) -> Option<String> {
    let inside = text.get(range.start + 1..range.end.checked_sub(1)?)?;
    Some(inside.replace("\\\"", "\"").replace("\\\\", "\\"))
}

/// The figures of `source` in the order they are written in: every call
/// of `figure`, and the tables and images that stand in none. Each
/// with what it shows, and with its number as far as it can be told
/// from this file alone: figures are counted by what they show, as
/// Typst counts them, and those that say they have no number are not.
#[must_use]
pub fn figures(source: &Source) -> Vec<Figure> {
    let text = source.text();
    let numbering = typst_text::rule_value(source, "figure", "numbering");
    let mut found = Vec::new();
    // What is counted, each with how many of it there are so far.
    let mut counts: Vec<(String, usize)> = Vec::new();
    let mut stack = vec![(LinkedNode::new(source.root()), false)];
    while let Some((node, within)) = stack.pop() {
        let called = (node.kind() == SyntaxKind::FuncCall)
            .then(|| callee(&node, text))
            .flatten();
        let mut within = within;
        match called {
            Some("figure") => {
                if let Some(mut figure) = read(&node, text, true) {
                    let unnumbered = named_text(source, figure.call, "numbering")
                        .or(numbering.as_deref())
                        == Some("none");
                    if let (Some((counted, number)), false) = (&mut figure.number, unnumbered) {
                        let place = counts.iter().position(|(name, _)| name == counted);
                        let place = place.unwrap_or_else(|| {
                            counts.push((counted.clone(), 0));
                            counts.len() - 1
                        });
                        if let Some((_, count)) = counts.get_mut(place) {
                            *count += 1;
                            *number = *count;
                        }
                    } else {
                        figure.number = None;
                    }
                    found.push(figure);
                }
                within = true;
            }
            Some("image" | "table") if !within => {
                found.extend(read(&node, text, false));
                continue;
            }
            _ => {}
        }
        // The first child on top: in the order of the text.
        let children: Vec<LinkedNode> = node.children().collect();
        stack.extend(children.into_iter().rev().map(|child| (child, within)));
    }
    found
}

/// The byte at which the first words of `figure` begin that are set on
/// the page as they are written (those of its caption, of a cell, of
/// its body): the page is surest to show the figure for a cursor there.
/// The byte its call begins at, where it has no words.
#[must_use]
pub fn words_of(source: &Source, figure: &Figure) -> usize {
    let mut stack: Vec<LinkedNode> = call_at(source, figure.call).into_iter().collect();
    while let Some(node) = stack.pop() {
        if node.kind() == SyntaxKind::Text {
            return node.offset();
        }
        // The first child on top: in the order of the text.
        let children: Vec<LinkedNode> = node.children().collect();
        stack.extend(children.into_iter().rev());
    }
    figure.call
}

/// The first image, table or piece of code in `node`, as Typst looks
/// for what a figure shows: what it is and the call it is written by.
fn shown<'a>(node: &LinkedNode<'a>, text: &str) -> Option<(&'static str, LinkedNode<'a>)> {
    match node.kind() {
        SyntaxKind::Raw => return Some(("raw", node.clone())),
        SyntaxKind::FuncCall => match callee(node, text) {
            Some("image") => return Some(("image", node.clone())),
            Some("table") => return Some(("table", node.clone())),
            Some("grid") => return Some(("grid", node.clone())),
            Some("raw") => return Some(("raw", node.clone())),
            _ => {}
        },
        // What is said of it is not what it shows.
        SyntaxKind::Named => return None,
        _ => {}
    }
    node.children().find_map(|child| shown(&child, text))
}

/// The label that stands behind `call` in running text.
fn label_behind<'a>(call: &LinkedNode<'a>) -> Option<LinkedNode<'a>> {
    let parent = call.parent()?;
    let mut behind = parent
        .children()
        .skip_while(|sibling| sibling.offset() < call.offset() + call.len())
        .skip_while(|sibling| sibling.kind() == SyntaxKind::Space);
    behind
        .next()
        .filter(|next| next.kind() == SyntaxKind::Label)
}

/// What a figure that shows `held` counts as, where it does not say so
/// itself by `kind`: as Typst names it in English.
fn counted(kind: Option<&str>, held: Option<&str>) -> String {
    match kind.filter(|kind| *kind != "auto").or(held) {
        Some("table") => "Table".to_owned(),
        Some("raw") => "Listing".to_owned(),
        Some("image" | "grid") | None => "Figure".to_owned(),
        Some(own) => own.trim_matches('"').to_owned(),
    }
}

/// The figure `call` is: a call of `figure` (`framed`), or a table or
/// an image by itself.
fn read(call: &LinkedNode, text: &str, framed: bool) -> Option<Figure> {
    let args = args_of(call)?;
    let value = |key: &str| {
        let value = named(&args, text, key)?.children().next_back()?;
        Some((value.kind(), value.range()))
    };
    let held = if framed {
        args.children().find_map(|arg| shown(&arg, text))
    } else {
        shown(call, text)
    };
    let what = held.as_ref().map(|(what, _)| *what);
    let at = held.as_ref().map(|(_, node)| node.offset());
    let file = held.as_ref().and_then(|(what, image)| {
        let file = args_of(image)
            .filter(|_| *what == "image")?
            .children()
            .find(|arg| arg.kind() == SyntaxKind::Str)?
            .range();
        Some((file.clone(), unquoted(text, &file)?))
    });
    let caption = value("caption").and_then(|(kind, range)| match kind {
        SyntaxKind::ContentBlock => text
            .get(range.start + 1..range.end.checked_sub(1)?)
            .map(|inside| inside.trim().to_owned()),
        SyntaxKind::Str => unquoted(text, &range),
        _ => text.get(range).map(str::to_owned),
    });
    let hash = call
        .prev_sibling_kind()
        .filter(|kind| *kind == SyntaxKind::Hash)
        .and(call.offset().checked_sub(1));
    let label = hash.and_then(|_| label_behind(call));
    let kind = value("kind").and_then(|(_, range)| text.get(range));
    let end = label
        .as_ref()
        .map_or(call.range().end, |label| label.range().end);
    Some(Figure {
        call: call.offset(),
        whole: hash.unwrap_or(call.offset())..end,
        framed,
        marked: hash.is_some(),
        kind: match what {
            Some("image") => Kind::Image,
            Some("table") => Kind::Table,
            _ => Kind::Other,
        },
        number: framed.then(|| (counted(kind, what), 0)),
        caption,
        label: label.and_then(|label| {
            let range = label.range();
            text.get(range.start + 1..range.end.checked_sub(1)?)
                .map(str::to_owned)
        }),
        image: at.filter(|_| what == Some("image")),
        file,
        table: at.filter(|_| matches!(what, Some("table" | "grid"))),
        line: text[..call.offset()].matches('\n').count() + 1,
    })
}

/// Words as running text says them: the marks Typst gives a meaning to
/// are written so that they have none, and what is typed is what is
/// read. What stays is what one types to say what one wants: an
/// equation between two dollar signs, and `@` before the name of what
/// is referred to.
#[must_use]
pub fn as_markup(words: &str) -> String {
    let words = words.trim();
    // A dollar sign without another is a dollar sign.
    let equations = words.matches('$').count().is_multiple_of(2);
    let mut written = String::with_capacity(words.len() + 8);
    let mut before = None::<char>;
    let mut chars = words.chars().peekable();
    while let Some(c) = chars.next() {
        let next = chars.peek().copied();
        let escaped = match c {
            '\\' | '[' | ']' | '#' | '*' | '_' | '`' | '<' | '~' => true,
            '$' => !equations,
            '@' => before.is_some_and(char::is_alphanumeric),
            // Two of them begin a comment; one before all else, a term.
            '/' => before.is_none() || matches!(next, Some('/' | '*')),
            // Before all else they begin a heading or an item of a list.
            '=' | '-' | '+' => before.is_none(),
            // As does a number with a full stop behind it.
            '.' => {
                written.chars().all(|c| c.is_ascii_digit())
                    && !written.is_empty()
                    && next.is_none_or(char::is_whitespace)
            }
            _ => false,
        };
        if escaped {
            written.push('\\');
        }
        written.push(c);
        before = Some(c);
    }
    written
}

/// Whether `name` can be the name of a label.
#[must_use]
pub fn is_label_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | ':' | '.'))
        && !name.ends_with(['.', ':'])
}

/// Where `source` uses the label `name`: the lines (counting from 1)
/// that put it on something, and the bytes of every reference to it
/// (`@name`, and `<name>` where it is handed to a function).
#[must_use]
pub fn label_uses(source: &Source, name: &str) -> (Vec<usize>, Vec<Range<usize>>) {
    let text = source.text();
    let (mut put, mut referred) = (Vec::new(), Vec::new());
    let mut stack = vec![LinkedNode::new(source.root())];
    while let Some(node) = stack.pop() {
        let written = text.get(node.range()).unwrap_or("");
        match node.kind() {
            SyntaxKind::Label if written.get(1..written.len().saturating_sub(1)) == Some(name) => {
                if node.parent_kind() == Some(SyntaxKind::Markup) {
                    put.push(text[..node.offset()].matches('\n').count() + 1);
                } else {
                    referred.push(node.range());
                }
            }
            SyntaxKind::RefMarker if written.strip_prefix('@') == Some(name) => {
                referred.push(node.range());
            }
            _ => stack.extend(node.children()),
        }
    }
    put.sort_unstable();
    referred.sort_by_key(|range| range.start);
    (put, referred)
}

/// The changes to `source` that give `figure` the label `name`, or with
/// none take its label away; with `references`, what refers to the
/// label it had refers to the new one. From the last to the first, so
/// that each leaves the bytes of those that follow where they are.
/// None if the figure cannot have a label where it stands.
#[must_use]
pub fn relabelled(
    source: &Source,
    figure: &Figure,
    name: Option<&str>,
    references: bool,
) -> Vec<(Range<usize>, String)> {
    if !figure.marked {
        return Vec::new();
    }
    let text = source.text();
    let end = figure.whole.end;
    let mut edits = Vec::new();
    match (&figure.label, name) {
        (Some(old), Some(name)) => {
            edits.push((end - old.len() - 2..end, format!("<{name}>")));
            let (_, referred) = label_uses(source, old);
            for range in referred.into_iter().filter(|_| references) {
                let with = if text[range.clone()].starts_with('@') {
                    format!("@{name}")
                } else {
                    format!("<{name}>")
                };
                edits.push((range, with));
            }
        }
        (Some(old), None) => {
            // With the blanks that set it off from the figure.
            let label = end - old.len() - 2;
            let start = text[..label].trim_end_matches(BLANKS).len();
            edits.push((start..end, String::new()));
        }
        (None, Some(name)) => edits.push((end..end, format!(" <{name}>"))),
        (None, None) => {}
    }
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    edits
}

/// The change to `source` that gives `figure` the caption `words`
/// (written as running text says them), or with none takes its caption
/// away. A table or an image that stands in no figure becomes one by
/// its caption: it is put into a call of `figure` as it is written.
#[must_use]
pub fn captioned(
    source: &Source,
    figure: &Figure,
    words: Option<&str>,
) -> Option<(Range<usize>, String)> {
    let caption = words.map(|words| format!("[{}]", as_markup(words)));
    if figure.framed {
        return set_named(source, figure.call, "caption", caption.as_deref());
    }
    let call = call_at(source, figure.call)?.range();
    let written = source.text().get(call.clone())?;
    Some((call, format!("figure(caption: {}, {written})", caption?)))
}

/// The bytes of `text` that go when `figure` is taken out: all of it,
/// with the lines it stands on where nothing else does, and with one of
/// the empty lines around it.
#[must_use]
pub fn removal(text: &str, figure: &Figure) -> Range<usize> {
    let Range { mut start, mut end } = figure.whole.clone();
    let line = text[..start].rfind('\n').map_or(0, |found| found + 1);
    let rest = text[end..]
        .find('\n')
        .map_or(text.len(), |found| end + found);
    let alone = text[line..start].trim().is_empty() && text[end..rest].trim().is_empty();
    if !alone {
        return start..end;
    }
    (start, end) = (line, (rest + 1).min(text.len()));
    let above = start == 0 || text[..start].ends_with("\n\n");
    if above && text[end..].starts_with('\n') {
        end += 1;
    }
    start..end
}

/// The table of `source` whose call begins at byte `start`.
#[must_use]
pub fn table_from(source: &Source, start: usize) -> Option<Table> {
    typst_text::table_at(source, start + 1).filter(|table| table.whole.start == start)
}

/// How many rows `table` has, those of its header among them.
#[must_use]
pub fn rows_of(table: &Table) -> usize {
    table.cells.len().div_ceil(table.columns)
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

/// The change to `source` that gives the table whose call begins at
/// byte `start` so many `columns` and `rows` (where one is not given,
/// as many as it has): columns and rows come and go at the end, new
/// cells are empty. The bytes of the table, and the table anew. `None`
/// if there is no such table, or it cannot be given that many: a table
/// keeps a row and a column, and its header.
#[must_use]
pub fn table_resized(
    source: &Source,
    start: usize,
    columns: Option<usize>,
    rows: Option<usize>,
) -> Option<(Range<usize>, String)> {
    let old = table_from(source, start)?;
    let fine = |wanted: Option<usize>| wanted.is_none_or(|wanted| (1..=MOST).contains(&wanted));
    if !fine(columns) || !fine(rows) {
        return None;
    }
    let mut text = source.text().to_owned();
    // What the table is to be after the step taken last.
    let mut next = None::<(usize, usize)>;
    loop {
        let source = Source::detached(text.as_str());
        let table = table_from(&source, start)?;
        let (wide, high) = (table.columns, rows_of(&table));
        // A step that did not do what it was for is the last.
        if next.is_some_and(|next| next != (wide, high)) {
            return None;
        }
        let last = table.cells.len() - 1;
        let more = |wanted: Option<usize>, has: usize| wanted.is_some_and(|wanted| wanted > has);
        let fewer = |wanted: Option<usize>, has: usize| wanted.is_some_and(|wanted| wanted < has);
        let mut edits = if more(columns, wide) {
            next = Some((wide + 1, high));
            let mut edits = typst_text::table_new_column(&source, &table);
            // A table that does not say how many columns it has, has
            // one: now it says so.
            if named_value(&source, start, "columns").is_none() {
                edits.push(set_named(&source, start, "columns", Some("2"))?);
            }
            edits
        } else if fewer(columns, wide) {
            next = Some((wide - 1, high));
            typst_text::table_without_column(&table, wide - 1)
        } else if more(rows, high) {
            next = Some((wide, high + 1));
            let (place, row, _) = typst_text::table_new_row(&source, &table)?;
            vec![(place..place, row)]
        } else if fewer(rows, high) && last >= table.header.max(1) {
            next = Some((wide, high - 1));
            typst_text::table_without_row(&table, last)
        } else if fewer(rows, high) {
            Vec::new()
        } else {
            let new = text.get(table.whole)?.to_owned();
            return Some((old.whole, new));
        };
        if edits.is_empty() {
            return None;
        }
        edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
        text = edited(&text, edits);
    }
}

/// The change to `source` that gives `table` a header where it has
/// none (its first row becomes it), or makes the header it has a row
/// like the others. `None` for a table without a whole first row.
#[must_use]
pub fn header_switched(source: &Source, table: &Table) -> Option<(Range<usize>, String)> {
    let text = source.text();
    let named = text.get(table.whole.clone())?;
    let named = &named[..named.find(['(', '['])?];
    if let Some(head) = &table.head {
        let (first, last) = (table.cells.first()?, table.cells.get(table.header - 1)?);
        return Some((head.clone(), text.get(first.start..last.end)?.to_owned()));
    }
    let (first, last) = (table.cells.first()?, table.cells.get(table.columns - 1)?);
    let row = text.get(first.start..last.end)?;
    Some((
        first.start..last.end,
        format!("{}.header({row})", named.trim()),
    ))
}

/// How the columns of a table are aligned, as typed: a word for each
/// column, or one for all (`left`, `center`, `right`, or their first
/// letters). As it is written for `align`; `None` if it is none of
/// that.
#[must_use]
pub fn alignment(typed: &str) -> Option<String> {
    let words: Option<Vec<&str>> = typed
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|word| !word.is_empty())
        .map(|word| match word.to_lowercase().as_str() {
            "l" | "left" => Some("left"),
            "c" | "center" | "centre" | "centred" | "centered" | "middle" => Some("center"),
            "r" | "right" => Some("right"),
            _ => None,
        })
        .collect();
    match words?.as_slice() {
        [] => None,
        [one] => Some((*one).to_owned()),
        several => Some(format!("({})", several.join(", "))),
    }
}

/// A width as typed: a number of percent of the line, or a number with
/// a unit of its own. As it is written; `None` if it is no width.
#[must_use]
pub fn width(typed: &str) -> Option<String> {
    let typed = typed.trim();
    let number = typed.trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == '%');
    let unit = &typed[number.len()..];
    let fine = number.parse::<f64>().is_ok_and(|number| number > 0.0);
    let known = matches!(unit, "" | "pt" | "mm" | "cm" | "in" | "em" | "%");
    (fine && known).then(|| format!("{number}{}", if unit.is_empty() { "%" } else { unit }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(text: &str) -> Source {
        Source::detached(text)
    }

    /// `text` with the argument `key` of its first call of `function`
    /// made `value`.
    fn set(text: &str, function: &str, key: &str, value: Option<&str>) -> Option<String> {
        let call = text.find(&format!("{function}("))?;
        let edit = set_named(&source(text), call, key, value)?;
        Some(edited(text, vec![edit]))
    }

    const PLOT: &str =
        "#figure(\n  image(\"plot.png\", width: 80%),\n  caption: [A _plot_],\n) <fig:plot>\n";

    #[test]
    fn a_named_argument_is_read_where_it_is() {
        let source = source(PLOT);
        assert_eq!(named_text(&source, 1, "caption"), Some("[A _plot_]"));
        assert_eq!(named_text(&source, 1, "placement"), None);
        let image = PLOT.find("image").unwrap_or(0);
        assert_eq!(named_text(&source, image, "width"), Some("80%"));
        // Not of a call that is none, nor of one within the call.
        assert_eq!(named_text(&source, 0, "caption"), None);
        assert_eq!(named_text(&source, 1, "width"), None);
    }

    #[test]
    fn a_named_argument_is_replaced_and_added() {
        // Replaced where it is.
        assert_eq!(
            set(PLOT, "image", "width", Some("50%")).as_deref(),
            Some(
                "#figure(\n  image(\"plot.png\", width: 50%),\n  caption: [A _plot_],\n) <fig:plot>\n"
            )
        );
        // Added behind the last that is named: on a line of its own
        // where that has one, in the line where it has not.
        assert_eq!(
            set(PLOT, "figure", "placement", Some("top")).as_deref(),
            Some(
                "#figure(\n  image(\"plot.png\", width: 80%),\n  caption: [A _plot_],\n  \
                 placement: top,\n) <fig:plot>\n"
            )
        );
        assert_eq!(
            set(PLOT, "image", "height", Some("3cm")).as_deref(),
            Some(
                "#figure(\n  image(\"plot.png\", width: 80%, height: 3cm),\n  \
                 caption: [A _plot_],\n) <fig:plot>\n"
            )
        );
        assert_eq!(
            set(
                "#figure(\n  image(\"a.png\"),\n  caption: []\n)",
                "figure",
                "placement",
                Some("auto")
            )
            .as_deref(),
            Some("#figure(\n  image(\"a.png\"),\n  caption: [],\n  placement: auto\n)")
        );
        assert_eq!(
            set("#image(\"a.png\")", "image", "width", Some("80%")).as_deref(),
            Some("#image(\"a.png\", width: 80%)")
        );
        assert_eq!(
            set("#image(\"a.png\",)", "image", "width", Some("80%")).as_deref(),
            Some("#image(\"a.png\", width: 80%,)")
        );
        // Before the cells of a table, which stay last.
        assert_eq!(
            set(
                "#table(columns: 2, [a], [b])",
                "table",
                "align",
                Some("center")
            )
            .as_deref(),
            Some("#table(columns: 2, align: center, [a], [b])")
        );
        assert_eq!(
            set(
                "#table(\n  columns: 2,\n  [a], [b],\n)",
                "table",
                "stroke",
                Some("none")
            )
            .as_deref(),
            Some("#table(\n  columns: 2,\n  stroke: none,\n  [a], [b],\n)")
        );
        assert_eq!(
            set("#table([a], [b])", "table", "stroke", Some("none")).as_deref(),
            Some("#table(stroke: none, [a], [b])")
        );
        assert_eq!(
            set(
                "#table(\n  [a],\n  [b],\n)",
                "table",
                "stroke",
                Some("none")
            )
            .as_deref(),
            Some("#table(\n  stroke: none,\n  [a],\n  [b],\n)")
        );
        // Where there is nothing, and where there are no brackets for it.
        assert_eq!(
            set("#figure()", "figure", "caption", Some("[x]")).as_deref(),
            Some("#figure(caption: [x])")
        );
        let body = "#figure[Words.]";
        let edit = set_named(&source(body), 1, "caption", Some("[x]"));
        assert_eq!(
            edit.map(|edit| edited(body, vec![edit])).as_deref(),
            Some("#figure(caption: [x])[Words.]")
        );
    }

    #[test]
    fn a_named_argument_is_taken_out_with_its_line_or_its_comma() {
        assert_eq!(
            set(PLOT, "figure", "caption", None).as_deref(),
            Some("#figure(\n  image(\"plot.png\", width: 80%),\n) <fig:plot>\n")
        );
        assert_eq!(
            set(PLOT, "image", "width", None).as_deref(),
            Some("#figure(\n  image(\"plot.png\"),\n  caption: [A _plot_],\n) <fig:plot>\n")
        );
        assert_eq!(
            set(
                "#table(stroke: none, columns: 2, [a])",
                "table",
                "stroke",
                None
            )
            .as_deref(),
            Some("#table(columns: 2, [a])")
        );
        assert_eq!(
            set(
                "#figure(\n  image(\"a.png\"),\n  caption: [x]\n)",
                "figure",
                "caption",
                None
            )
            .as_deref(),
            Some("#figure(\n  image(\"a.png\")\n)")
        );
        assert_eq!(
            set("#image(width: 80%)", "image", "width", None).as_deref(),
            Some("#image()")
        );
        // What is not there is not taken out.
        assert_eq!(set(PLOT, "figure", "placement", None), None);
    }

    const PAPER: &str = "= Results\n\
        #figure(\n  image(\"plot.png\", width: 80%),\n  caption: [A _plot_],\n) <fig:plot>\n\n\
        See @fig:plot and #ref(<fig:plot>), not @fig:plots.\n\n\
        #figure(\n  table(\n    columns: 2,\n    [a], [b],\n    [c], [d],\n  ),\n  caption: [Numbers],\n)\n\n\
        #figure(caption: [Code], numbering: none)[```rust\nfn main() {}\n```]\n\n\
        #figure(rect[Drawn], caption: \"A box\") <box>\n\n\
        #image(\"logo.svg\")\n\n\
        #align(center, table(columns: 1, [x]))\n\n\
        #figure(align(center, table(columns: 1, [y])))\n";

    #[test]
    fn the_figures_of_a_file_are_found_with_what_they_hold() {
        let found = figures(&source(PAPER));
        let told: Vec<String> = found
            .iter()
            .map(|figure| {
                format!(
                    "{}, {:?}, {:?}, {:?}, line {}",
                    figure.name(),
                    figure.caption.as_deref().unwrap_or("-"),
                    figure.label.as_deref().unwrap_or("-"),
                    figure.file.as_ref().map_or("-", |(_, file)| file.as_str()),
                    figure.line,
                )
            })
            .collect();
        assert_eq!(
            told,
            [
                "Figure 1, \"A _plot_\", \"fig:plot\", \"plot.png\", line 2",
                "Table 1, \"Numbers\", \"-\", \"-\", line 9",
                "figure, \"Code\", \"-\", \"-\", line 18",
                "Figure 2, \"A box\", \"box\", \"-\", line 22",
                "image, \"-\", \"-\", \"logo.svg\", line 24",
                "table, \"-\", \"-\", \"-\", line 26",
                "Table 2, \"-\", \"-\", \"-\", line 28",
            ]
        );
        let kinds: Vec<Kind> = found.iter().map(|figure| figure.kind).collect();
        assert_eq!(
            kinds,
            vec![
                Kind::Image,
                Kind::Table,
                Kind::Other,
                Kind::Other,
                Kind::Image,
                Kind::Table,
                Kind::Table
            ]
        );
        // All of it, hash and label; what stands in code has neither.
        assert!(PAPER[found[0].whole.clone()].starts_with("#figure("));
        assert!(PAPER[found[0].whole.clone()].ends_with("<fig:plot>"));
        assert!(found[0].marked && found[0].framed);
        assert!(!found[5].marked && !found[5].framed);
        assert!(PAPER[found[5].whole.clone()].starts_with("table("));
        assert!(found[1].table.is_some() && found[6].table.is_some());
        // Where its first words are: of the caption, a cell, the body.
        let words: Vec<&str> = found
            .iter()
            .map(|figure| &PAPER[words_of(&source(PAPER), figure)..][..4])
            .collect();
        assert_eq!(
            words,
            ["A _p", "a], ", "Code", "Draw", "imag", "x]))", "y]))"]
        );
        assert!(found[0].holds(PAPER.find("plot.png").unwrap_or(0)));
        assert!(!found[0].holds(PAPER.find("See").unwrap_or(0)));
    }

    #[test]
    fn figures_are_counted_as_they_say() {
        let text = "#set figure(numbering: none)\n#figure(image(\"a.png\"))\n";
        assert_eq!(figures(&source(text))[0].number, None);
        let text = "#figure(image(\"a.png\"), kind: table)\n\
                    #figure(table([a]), kind: \"plate\")\n\
                    #figure(figure(image(\"b.png\")), caption: [Both])\n";
        let names: Vec<String> = figures(&source(text)).iter().map(Figure::name).collect();
        assert_eq!(names, ["Table 1", "plate 1", "Figure 1", "Figure 2"]);
    }

    #[test]
    fn captions_are_written_as_running_text() {
        assert_eq!(as_markup(" Plain words. "), "Plain words.");
        assert_eq!(
            as_markup("a*b_c [1] #2 <x>"),
            "a\\*b\\_c \\[1\\] \\#2 \\<x>"
        );
        assert_eq!(as_markup("Costs $5"), "Costs \\$5");
        assert_eq!(as_markup("Where $x^2$ grows"), "Where $x^2$ grows");
        assert_eq!(
            as_markup("As in @fig:a, by a@b.c"),
            "As in @fig:a, by a\\@b.c"
        );
        assert_eq!(as_markup("= not a heading"), "\\= not a heading");
        assert_eq!(as_markup("- a - b"), "\\- a - b");
        assert_eq!(as_markup("1. first"), "1\\. first");
        assert_eq!(as_markup("In 2024. Then"), "In 2024. Then");
        assert_eq!(as_markup("a // b, c/d"), "a \\// b, c/d");
        let caption = |text: &str, words: Option<&str>| {
            let source = source(text);
            let figure = figures(&source).into_iter().next()?;
            Some(edited(text, vec![captioned(&source, &figure, words)?]))
        };
        assert_eq!(
            caption(PLOT, Some("New *words*")).as_deref(),
            Some(
                "#figure(\n  image(\"plot.png\", width: 80%),\n  caption: [New \\*words\\*],\n) <fig:plot>\n"
            )
        );
        assert_eq!(
            caption("#figure(image(\"a.png\"))", Some("One")).as_deref(),
            Some("#figure(image(\"a.png\"), caption: [One])")
        );
        assert_eq!(
            caption("#figure(image(\"a.png\"), caption: [One])", None).as_deref(),
            Some("#figure(image(\"a.png\"))")
        );
        // A table by itself becomes a figure by its caption.
        assert_eq!(
            caption("#table(\n  columns: 1,\n  [a],\n) <t>", Some("One")).as_deref(),
            Some("#figure(caption: [One], table(\n  columns: 1,\n  [a],\n)) <t>")
        );
        assert_eq!(caption("#image(\"a.png\")", None), None);
    }

    #[test]
    fn labels_are_given_changed_and_followed() {
        assert!(is_label_name("fig:plot-2_a.b"));
        assert!(!is_label_name(""));
        assert!(!is_label_name("two words"));
        assert!(!is_label_name("fig:"));
        let source = source(PAPER);
        let (put, referred) = label_uses(&source, "fig:plot");
        assert_eq!(put, [5]);
        let referred: Vec<&str> = referred.into_iter().map(|range| &PAPER[range]).collect();
        assert_eq!(referred, ["@fig:plot", "<fig:plot>"]);
        let found = figures(&source);
        let renamed = edited(PAPER, relabelled(&source, &found[0], Some("plot"), true));
        assert!(renamed.contains(") <plot>\n\nSee @plot and #ref(<plot>), not @fig:plots."));
        let alone = edited(PAPER, relabelled(&source, &found[0], Some("plot"), false));
        assert!(alone.contains(") <plot>\n\nSee @fig:plot and #ref(<fig:plot>)"));
        let none = edited(PAPER, relabelled(&source, &found[0], None, true));
        assert!(none.contains("caption: [A _plot_],\n)\n\nSee @fig:plot"));
        let new = edited(PAPER, relabelled(&source, &found[1], Some("tab:n"), true));
        assert!(new.contains("caption: [Numbers],\n) <tab:n>\n"));
        // Not where it stands in code.
        assert_eq!(relabelled(&source, &found[5], Some("t"), true), Vec::new());
    }

    #[test]
    fn a_figure_goes_with_its_lines() {
        let without = |text: &str, which: usize| {
            let figure = figures(&source(text)).into_iter().nth(which);
            figure.map(|figure| edited(text, vec![(removal(text, &figure), String::new())]))
        };
        assert_eq!(
            without("Before.\n\n#figure(image(\"a.png\")) <a>\n\nAfter.\n", 0).as_deref(),
            Some("Before.\n\nAfter.\n")
        );
        assert_eq!(
            without("#image(\"a.png\")\n\nAfter.\n", 0).as_deref(),
            Some("After.\n")
        );
        assert_eq!(
            without("Before.\n#image(\"a.png\")\nAfter.\n", 0).as_deref(),
            Some("Before.\nAfter.\n")
        );
        assert_eq!(
            without("Before #image(\"a.png\") after.\n", 0).as_deref(),
            Some("Before  after.\n")
        );
        assert_eq!(
            without("Before.\n\n#image(\"a.png\")", 0).as_deref(),
            Some("Before.\n\n")
        );
    }

    const TABLE: &str =
        "#figure(\n  table(\n    columns: 2,\n    [a], [b],\n    [c], [d],\n  ),\n)";

    /// `text` with its first table given so many columns and rows.
    fn resized(text: &str, columns: Option<usize>, rows: Option<usize>) -> Option<String> {
        let source = source(text);
        let start = text.find("table(")?;
        Some(edited(
            text,
            vec![table_resized(&source, start, columns, rows)?],
        ))
    }

    #[test]
    fn a_table_is_given_columns_and_rows_at_its_end() {
        assert_eq!(
            resized(TABLE, Some(3), None).as_deref(),
            Some(
                "#figure(\n  table(\n    columns: 3,\n    [a], [b], [],\n    [c], [d], [],\n  ),\n)"
            )
        );
        assert_eq!(
            resized(TABLE, Some(1), Some(4)).as_deref(),
            Some(
                "#figure(\n  table(\n    columns: 1,\n    [a],\n    [c],\n    [],\n    [],\n  ),\n)"
            )
        );
        assert_eq!(
            resized(TABLE, None, Some(1)).as_deref(),
            Some("#figure(\n  table(\n    columns: 2,\n    [a], [b],\n  ),\n)")
        );
        assert_eq!(resized(TABLE, Some(2), Some(2)).as_deref(), Some(TABLE));
        assert_eq!(resized(TABLE, Some(0), None), None);
        assert_eq!(resized(TABLE, None, Some(0)), None);
        assert_eq!(resized(TABLE, None, Some(MOST + 1)), None);
        // A table that does not say its columns has one.
        assert_eq!(
            resized("#table([a], [b])", Some(2), None).as_deref(),
            Some("#table(columns: 2, [a], [], [b], [])")
        );
        // Columns that are told by a list cannot go.
        let listed = "#table(columns: (auto, 1fr), [a], [b])";
        assert_eq!(resized(listed, Some(1), None), None);
        assert_eq!(
            resized(listed, Some(3), None).as_deref(),
            Some("#table(columns: (auto, 1fr, 1fr), [a], [b], [])")
        );
    }

    /// `text` with the header of its first table switched.
    fn switched(text: &str) -> Option<String> {
        let source = source(text);
        let start = text.find("table(").or_else(|| text.find("grid("))?;
        let table = table_from(&source, start)?;
        Some(edited(text, vec![header_switched(&source, &table)?]))
    }

    #[test]
    fn a_table_has_a_header_or_has_none() {
        let headed = "#figure(\n  table(\n    columns: 2,\n    table.header([a], [b]),\n    [c], [d],\n  ),\n)";
        assert_eq!(switched(TABLE).as_deref(), Some(headed));
        assert_eq!(switched(headed).as_deref(), Some(TABLE));
        assert_eq!(
            switched("#grid(columns: 2, [a], [b])").as_deref(),
            Some("#grid(columns: 2, grid.header([a], [b]))")
        );
        // Not of a row that is not whole.
        assert_eq!(switched("#table(columns: 2, [a])"), None);
    }

    #[test]
    fn a_table_with_a_header_gets_and_loses_rows_and_columns() {
        let headed = "#table(\n  columns: 2,\n  table.header([a], [b]),\n  [c], [d],\n)";
        let source = source(headed);
        let Some(table) = table_from(&source, 1) else {
            panic!("no table");
        };
        assert_eq!((table.columns, table.cells.len(), table.header), (2, 4, 2));
        assert_eq!(
            resized(headed, Some(3), Some(3)).as_deref(),
            Some(
                "#table(\n  columns: 3,\n  table.header([a], [b], []),\n  [c], [d], [],\n  \
                 [], [], [],\n)"
            )
        );
        assert_eq!(
            resized(headed, Some(1), None).as_deref(),
            Some("#table(\n  columns: 1,\n  table.header([a]),\n  [c],\n)")
        );
        // The header stays when the rows go; a row comes behind it.
        let bare = "#table(\n  columns: 2,\n  table.header([a], [b]),\n)";
        assert_eq!(resized(headed, None, Some(1)).as_deref(), Some(bare));
        assert_eq!(resized(bare, None, Some(1)).as_deref(), Some(bare));
        assert_eq!(
            resized(bare, None, Some(2)).as_deref(),
            Some("#table(\n  columns: 2,\n  table.header([a], [b]),\n  [], [],\n)")
        );
        // The row of the header goes with the header.
        assert_eq!(
            edited(headed, typst_text::table_without_row(&table, 0)),
            "#table(\n  columns: 2,\n  [c], [d],\n)"
        );
        assert_eq!(
            edited(headed, typst_text::table_without_column(&table, 0)),
            "#table(\n  columns: 1,\n  table.header([b]),\n  [d],\n)"
        );
        // A header of two rows loses either.
        let two = "#table(columns: 1, table.header([a], [b]), [c])";
        let source = super::tests::source(two);
        let Some(table) = table_from(&source, 1) else {
            panic!("no table");
        };
        assert_eq!(
            edited(two, typst_text::table_without_row(&table, 0)),
            "#table(columns: 1, table.header([b]), [c])"
        );
        assert_eq!(
            edited(two, typst_text::table_without_row(&table, 1)),
            "#table(columns: 1, table.header([a]), [c])"
        );
        // A header that is not whole rows, or not cells, is left alone.
        let odd = "#table(columns: 2, table.header([a]), [c], [d])";
        assert!(table_from(&super::tests::source(odd), 1).is_none());
        let odd = "#table(columns: 2, table.header(table.cell(colspan: 2)[a]), [c], [d])";
        assert!(table_from(&super::tests::source(odd), 1).is_none());
    }

    #[test]
    fn what_is_typed_for_a_table_or_an_image_is_written_as_typst_wants_it() {
        assert_eq!(alignment("l c r").as_deref(), Some("(left, center, right)"));
        assert_eq!(alignment("left, Centre").as_deref(), Some("(left, center)"));
        assert_eq!(alignment("right").as_deref(), Some("right"));
        assert_eq!(alignment("up"), None);
        assert_eq!(alignment(""), None);
        assert_eq!(width("70").as_deref(), Some("70%"));
        assert_eq!(width("7.5cm").as_deref(), Some("7.5cm"));
        assert_eq!(width("wide"), None);
        assert_eq!(width("0"), None);
    }
}
