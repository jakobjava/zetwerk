//! What keys typed in an equation do for somebody who writes LaTeX.
//!
//! A backslash begins a LaTeX name. While it is typed it is text of the
//! equation, `"\al"`: the page shows it as it is typed, and Typst always
//! reads it (a backslash alone before the closing `$` would escape it).
//! When the name is over (a character that is no letter, a blank, `{`,
//! Tab, the cursor leaving) it becomes what it is in Typst: `\alpha`
//! becomes `alpha`, `\frac{` becomes `frac(□, □)` with the cursor in the
//! first place. A name nobody knows stays as typed, as text.
//!
//! Around that, what a LaTeX writer types goes where it is meant: `}`
//! leads to the next place of what was begun (`\frac{a}{b}`) or out of
//! it, `{` behind `^` and `_` groups (`x^{2n}` is `x^(2n)`) and is
//! nothing elsewhere, `&` and `\\` part the cells and rows of a matrix,
//! `\)` and `\]` leave the equation. In running text, `\(` and `\[`
//! begin one, and `$$…$$` makes what is between a displayed equation.
//!
//! All of it is on unless the settings say `[typing] latex = false`.

use std::ops::Range;

use typst::syntax::{LinkedNode, Side, Source, SyntaxKind};

use super::table::{self, Layout, Shape};
use crate::typst_text::{self, Ground};

/// What a key does: the bytes to replace, the text to put there, and how
/// far into it the cursor goes.
pub type Typed = (Range<usize>, String, usize);

/// A LaTeX name being typed in an equation, kept as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    /// The bytes of the text, with its quotation marks.
    pub range: Range<usize>,
    /// What is typed behind the backslash: `alpha`, `begin{pmatrix`.
    pub typed: String,
}

/// The commands whose arguments are typed into the text of the name, as
/// LaTeX, and made Typst together with it when they are over: those
/// whose Typst puts the arguments otherwise than they are typed.
const WHOLE: &[&str] = &[
    "begin",
    "end",
    "overset",
    "underset",
    "stackrel",
    "xrightarrow",
    "xleftarrow",
    "xLeftarrow",
    "xRightarrow",
    "xmapsto",
    "boxed",
    "phantom",
    "color",
    "textcolor",
    "label",
    "tag",
    "hspace",
    "vspace",
    "pmod",
    "pod",
];

/// Whether `typed` (what follows the backslash) is a LaTeX name being
/// typed: letters; `\left`, `\right` or `\not` and the backslash of what
/// they are said of; or a command of [`WHOLE`] and its arguments.
fn is_pending(typed: &str) -> bool {
    let letters = typed.len()
        - typed
            .trim_start_matches(|c: char| c.is_ascii_alphabetic())
            .len();
    let (name, rest) = typed.split_at(letters);
    if rest.is_empty() {
        return true;
    }
    if let Some(after) = rest.strip_prefix('\\') {
        return matches!(name, "left" | "right" | "not")
            && after.chars().all(|c| c.is_ascii_alphabetic());
    }
    WHOLE.contains(&name) && rest.starts_with('{')
}

/// What text in an equation reads, the node `node` being it.
fn string_at(source: &Source, node: &LinkedNode) -> Option<String> {
    if node.kind() != SyntaxKind::Str {
        return None;
    }
    let written = source.text().get(node.range())?;
    let inner = written.strip_prefix('"')?.strip_suffix('"')?;
    let mut read = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            read.push(chars.next()?);
        } else {
            read.push(c);
        }
    }
    Some(read)
}

/// The name being typed that byte `at` of `source` ends: right before the
/// quotation mark that closes it.
#[must_use]
pub fn pending_at(source: &Source, at: usize) -> Option<Pending> {
    let leaf = LinkedNode::new(source.root()).leaf_at(at, Side::After)?;
    if leaf.range().end != at + 1 || leaf.range().start >= at {
        return None;
    }
    pending_in(source, &leaf)
}

/// The name being typed whose text begins at byte `start` of `source`.
#[must_use]
pub fn pending_from(source: &Source, start: usize) -> Option<Pending> {
    let leaf = LinkedNode::new(source.root()).leaf_at(start + 1, Side::Before)?;
    if leaf.range().start != start {
        return None;
    }
    pending_in(source, &leaf)
}

fn pending_in(source: &Source, leaf: &LinkedNode) -> Option<Pending> {
    let read = string_at(source, leaf)?;
    let typed = read.strip_prefix('\\')?;
    if !is_pending(typed) || !typst_text::is_in_equation(source, leaf.range().start) {
        return None;
    }
    Some(Pending {
        range: leaf.range(),
        typed: typed.to_owned(),
    })
}

/// What typing `c` at byte `at` of `source` does for a LaTeX writer;
/// `None` where it is nothing of LaTeX, and the key does what it always
/// does.
#[must_use]
pub fn typed(source: &Source, at: usize, c: char) -> Option<Typed> {
    if !super::typing_on() {
        return None;
    }
    if let Some(pending) = pending_at(source, at) {
        return go_on(source, &pending, c);
    }
    match typst_text::ground(source, at) {
        Ground::Math => in_math(source, at, c),
        Ground::Str => out_of_text(source, at, c),
        Ground::Markup => in_markup(source, at, c),
        _ => None,
    }
}

/// Tab at byte `at` of `source`, where a name is being typed: it is
/// over, as with `{` for a command with arguments.
#[must_use]
pub fn tab(source: &Source, at: usize) -> Option<Typed> {
    if !super::typing_on() {
        return None;
    }
    let pending = pending_at(source, at)?;
    match ended(source, &pending, true) {
        // A name: a blank behind it, for what is typed next.
        Some((range, mut with, within))
            if within == with.len() && with.ends_with(char::is_alphanumeric) =>
        {
            // Over the blank that stands there already.
            let range = if source.text()[range.end..].starts_with(' ') {
                range.start..range.end + 1
            } else {
                range
            };
            with.push(' ');
            let within = with.len();
            Some((range, with, within))
        }
        Some(done) => Some(done),
        // Nothing to make of it: the cursor goes behind it.
        None => Some((at..at + 1, "\"".to_owned(), 1)),
    }
}

/// Backspace at byte `at` of `source`: a name begun with nothing typed
/// behind its backslash goes whole, and a backslash written in a name
/// (`\left\`) goes with what escapes it. The bytes to take, and what is
/// left in their place.
#[must_use]
pub fn backspace(source: &Source, at: usize) -> Option<(Range<usize>, String)> {
    let pending = pending_at(source, at)?;
    if pending.typed.is_empty() {
        return Some((pending.range, String::new()));
    }
    let before = source.text().get(pending.range.start + 1..at)?;
    (before.ends_with("\\\\") || before.ends_with("\\\"")).then(|| (at - 2..at, String::new()))
}

/// The name being typed whose text begins at byte `start`, when the
/// cursor has left it: what it becomes, and where. `None` where it stays
/// text: a name nobody knows, an environment half named.
#[must_use]
pub fn left(source: &Source, start: usize) -> Option<(Range<usize>, String)> {
    let pending = pending_from(source, start)?;
    let (range, with, _) = ended(source, &pending, false)?;
    Some((range, with))
}

/// Whether `typed` (behind a backslash) is a name that nobody here
/// knows, which stays text.
#[must_use]
pub fn unknown(typed: &str) -> bool {
    !typed.is_empty()
        && typed.chars().all(|c| c.is_ascii_alphabetic())
        && meaning(typed) == Meaning::Unknown
}

/// What a LaTeX name is to the keys that follow it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Meaning {
    /// Something without arguments, as Typst writes it.
    Symbol(&'static str),
    /// Something with arguments typed as they are in Typst.
    Shaped(Shape),
    /// Something whose arguments are typed as LaTeX into its text.
    Whole,
    /// Something Typst needs not hear of (`\displaystyle`).
    Silent,
    Left,
    Right,
    Not,
    Unknown,
}

fn meaning(name: &str) -> Meaning {
    match name {
        "left" => Meaning::Left,
        "right" => Meaning::Right,
        "not" => Meaning::Not,
        _ if WHOLE.contains(&name) => Meaning::Whole,
        _ if let Some(typst) = table::symbol(name) => Meaning::Symbol(typst),
        _ if let Some(shape) = table::command(name) => match shape {
            Shape::Frac
            | Shape::Sqrt
            | Shape::Call(..)
            | Shape::Text
            | Shape::Operator
            | Shape::Brace(_) => Meaning::Shaped(shape),
            _ => Meaning::Whole,
        },
        _ if table::SILENT.contains(&name) => Meaning::Silent,
        _ => Meaning::Unknown,
    }
}

/// What is put in for a command with arguments, `|` where the cursor
/// goes: the first place.
fn template(shape: Shape) -> String {
    match shape {
        Shape::Frac => "frac(|, )".to_owned(),
        Shape::Sqrt => "sqrt(|)".to_owned(),
        Shape::Call(name, 1) => format!("{name}(|)"),
        Shape::Call(name, _) | Shape::Brace(name) => format!("{name}(|, )"),
        Shape::Text => "\"|\"".to_owned(),
        Shape::Operator => "op(\"|\")".to_owned(),
        _ => "|".to_owned(),
    }
}

/// `text` with the cursor at its `|`: the text without it, and where.
fn cursor_in(text: &str) -> (String, usize) {
    let within = text.find('|').unwrap_or(text.len());
    (text.replacen('|', "", 1), within)
}

/// Whether two characters side by side would be read as one thing by
/// Typst: a name (`alpha` and `x`), or a shorthand (`<` and `=`).
fn joins(a: char, b: char) -> bool {
    let word = char::is_alphanumeric;
    let shorthand = |c: char| "-:!.<>=|~*[]+/".contains(c);
    (word(a) && word(b)) || (a == '.' && word(b)) || (shorthand(a) && shorthand(b))
}

/// `with` put at bytes `range` of `source` in place of a name, with a
/// blank on either side where it would otherwise join what is there.
/// The text, and how far into it what was `with` begins.
fn apart(source: &Source, range: &Range<usize>, with: &str) -> (String, usize) {
    let text = source.text();
    let before = text[..range.start].chars().next_back();
    let after = text[range.end..].chars().next();
    let mut out = String::new();
    let mut lead = 0;
    if let (Some(before), Some(first)) = (before, with.chars().next())
        && joins(before, first)
    {
        out.push(' ');
        lead = 1;
    }
    out.push_str(with);
    if let (Some(last), Some(after)) = (with.chars().next_back(), after)
        && joins(last, after)
    {
        out.push(' ');
    }
    (out, lead)
}

/// What the name `pending` becomes when it is over; `typing` where a key
/// typed or Tab ended it, else the cursor left it. The bytes, the text,
/// and where in it the cursor goes. `None` where it stays text.
fn ended(source: &Source, pending: &Pending, typing: bool) -> Option<Typed> {
    let typed = pending.typed.as_str();
    let range = pending.range.clone();
    if typed.starts_with("left\\") || typed.starts_with("right\\") {
        return Some((range, String::new(), 0));
    }
    if typed.contains('{') || typed.contains('\\') {
        let converted = super::to_typst(&format!("\\{typed}"));
        if !converted.kept.is_empty() || unbalanced(typed) {
            return None;
        }
        let (with, lead) = apart(source, &range, &converted.text);
        let within = lead + converted.text.len();
        return Some((range, with, within));
    }
    match meaning(typed) {
        Meaning::Symbol(typst) => {
            let (with, lead) = apart(source, &range, typst);
            Some((range, with, lead + typst.len()))
        }
        Meaning::Shaped(shape) => {
            let (template, within) = cursor_in(&template(shape));
            let (with, lead) = apart(source, &range, &template);
            let within = if typing { lead + within } else { with.len() };
            Some((range, with, within))
        }
        Meaning::Silent | Meaning::Left | Meaning::Right | Meaning::Not => {
            Some((range, String::new(), 0))
        }
        _ if typed.is_empty() => Some((range, String::new(), 0)),
        Meaning::Whole | Meaning::Unknown => None,
    }
}

/// Whether the braces of `typed` are not all closed.
fn unbalanced(typed: &str) -> bool {
    let mut depth = 0_i32;
    let mut escaped = false;
    for c in typed.chars() {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
    }
    depth > 0
}

/// `c` written into the text of a name: as Typst writes text.
fn into_name(at: usize, c: char) -> Typed {
    let with = if matches!(c, '"' | '\\') {
        format!("\\{c}")
    } else {
        c.to_string()
    };
    let within = with.len();
    (at..at, with, within)
}

/// `c` typed at the end of the name `pending`.
fn go_on(source: &Source, pending: &Pending, c: char) -> Option<Typed> {
    let at = pending.range.end - 1;
    let typed = pending.typed.as_str();
    let range = pending.range.clone();
    // An environment's name, the arguments of a command typed whole.
    if typed.contains('{') {
        let open = unbalanced(typed);
        if c == '}' && open && !unbalanced(&format!("{typed}}}")) {
            let name = typed.split('{').next().unwrap_or("");
            let environment = &typed[name.len() + 1..];
            match name {
                "begin" => return Some(begin(source, &range, environment)),
                "end" => return Some(end(source, &range)),
                _ => {}
            }
        }
        if open || c == '{' || (c == '[' && typed.ends_with('}')) {
            return Some(into_name(at, c));
        }
        return Some(then(source, ended(source, pending, true), &range, c));
    }
    if c.is_ascii_alphabetic() {
        return Some(into_name(at, c));
    }
    // `\\`, `\,`, `\{` and their like: a backslash and one sign.
    if typed.is_empty() {
        return Some(sign(source, &range, c));
    }
    // What `\left`, `\right` and `\not` are said of.
    if let Some((name, of)) = typed.split_once('\\') {
        return said_of(source, &range, name, of, c);
    }
    match (meaning(typed), c) {
        (Meaning::Left | Meaning::Right | Meaning::Not, '\\') | (Meaning::Whole, '{' | '[') => {
            Some(into_name(at, c))
        }
        (Meaning::Left, _) => Some(left_delimiter(source, &range, &c.to_string())),
        (Meaning::Right, _) => right_delimiter(source, &range).or_else(|| {
            Some(then(
                source,
                Some((range.clone(), String::new(), 0)),
                &range,
                c,
            ))
        }),
        (Meaning::Not, '=') => {
            let (with, lead) = apart(source, &range, "!=");
            Some((range, with, lead + 2))
        }
        (Meaning::Shaped(Shape::Sqrt), '[') => {
            let (template, within) = cursor_in("root(|, )");
            let (with, lead) = apart(source, &range, &template);
            Some((range, with, lead + within))
        }
        // The arguments begin: the cursor goes into the first place.
        (Meaning::Shaped(_), '{' | ' ') => ended(source, pending, true),
        // `\frac1`: what is typed goes into the first place.
        (Meaning::Shaped(_), _) => {
            let done = ended(source, pending, true)?;
            Some(then_inside(source, done, c))
        }
        _ => Some(then(source, ended(source, pending, true), &range, c)),
    }
}

/// `c` typed behind `\left\of`, `\right\of` or `\not\of` (the name
/// `name`), at `range`: with nothing behind the backslash, `c` is the
/// delimiter (`\left\{`).
fn said_of(source: &Source, range: &Range<usize>, name: &str, of: &str, c: char) -> Option<Typed> {
    let delimiter = if of.is_empty() {
        format!("\\{c}")
    } else {
        format!("\\{of}")
    };
    // A blank, or the sign that was the delimiter, is no more.
    let more = !of.is_empty() && c != ' ';
    match name {
        "left" => {
            let done = left_delimiter(source, range, &delimiter);
            if more {
                Some(then_inside(source, done, c))
            } else {
                Some(done)
            }
        }
        "right" => {
            let done = right_delimiter(source, range);
            if more {
                Some(then(source, done, range, c))
            } else {
                done.or(Some((range.clone(), String::new(), 0)))
            }
        }
        _ => {
            let converted = super::to_typst(&format!("\\not{delimiter}")).text;
            let (with, lead) = apart(source, range, &converted);
            let done = (range.clone(), with, lead + converted.len());
            if more {
                Some(then(source, Some(done), range, c))
            } else {
                Some(done)
            }
        }
    }
}

/// What `\` and the sign `c` are, typed in an equation: the name with
/// nothing in it at `range` goes for it.
fn sign(source: &Source, range: &Range<usize>, c: char) -> Typed {
    // A name with a blank behind it: what is typed next is no more of it.
    let put = |with: &str| {
        let (mut with, lead) = apart(source, range, with);
        let after = source.text()[range.end..].chars().next();
        if with.ends_with(char::is_alphanumeric) && after != Some(' ') {
            with.push(' ');
        }
        let within = with.len().max(lead);
        (range.clone(), with, within)
    };
    match c {
        '\\' => row(source, range),
        ',' => put("thin"),
        ':' | ';' | '>' => put("med"),
        ' ' => put("space"),
        '!' | '(' | '[' => (range.clone(), String::new(), 0),
        '{' => put("brace.l"),
        '}' => put("brace.r"),
        '|' => put("bar.v.double"),
        '%' => put("%"),
        // `\)` and `\]` end the equation: the cursor goes behind it.
        ')' | ']' => match typst_text::equation_at(source, range.start) {
            Some(equation) if equation.end > range.end => {
                let rest = source.text()[range.end..equation.end].to_owned();
                let within = rest.len();
                (range.start..equation.end, rest, within)
            }
            _ => (range.clone(), String::new(), 0),
        },
        c => put(&format!("\\{c}")),
    }
}

/// `\\` typed at `range` (where the name with nothing in it was): a new
/// row of a matrix, a new case, or a new line of the equation.
fn row(source: &Source, range: &Range<usize>) -> Typed {
    let with = match innermost_call(source, range.start).as_deref() {
        Some("mat") => "; ",
        Some("cases") => ", ",
        _ => " \\\n",
    };
    let text = source.text();
    let start = text[..range.start].trim_end_matches(' ').len();
    (start..range.end, with.to_owned(), with.len())
}

/// The name of the call whose arguments byte `at` of `source` is in,
/// directly: not in brackets within them.
fn innermost_call(source: &Source, at: usize) -> Option<String> {
    let mut node = LinkedNode::new(source.root()).leaf_at(at, Side::Before)?;
    loop {
        match node.kind() {
            SyntaxKind::MathArgs => {
                let call = node.parent()?;
                let callee = call.children().next()?;
                return source.text().get(callee.range()).map(str::to_owned);
            }
            SyntaxKind::MathDelimited | SyntaxKind::Equation => return None,
            _ => node = node.parent()?.clone(),
        }
    }
}

/// `\begin{environment}` typed whole, at `range`.
fn begin(source: &Source, range: &Range<usize>, environment: &str) -> Typed {
    let template = match table::environment(environment) {
        Some(Layout::Matrix(None)) => "mat(|)".to_owned(),
        Some(Layout::Matrix(Some(delimiter))) => format!("mat(delim: {delimiter}, |)"),
        Some(Layout::Cases) => "cases(|)".to_owned(),
        // Lines are lines of the equation itself.
        Some(Layout::Lines) => "|".to_owned(),
        // Nobody knows it: it stays as typed, and the cursor behind it.
        None => {
            let end = range.end;
            return (end - 1..end, "}\"".to_owned(), 2);
        }
    };
    let (template, within) = cursor_in(&template);
    let (with, lead) = apart(source, range, &template);
    (range.clone(), with, lead + within)
}

/// `\end{…}` typed whole, at `range`: out of the matrix or the cases the
/// cursor is in.
fn end(source: &Source, range: &Range<usize>) -> Typed {
    let text = source.text();
    let call = matches!(
        innermost_call(source, range.start).as_deref(),
        Some("mat" | "cases")
    );
    let rest = &text[range.end..];
    let close = rest
        .find(')')
        .filter(|close| call && rest[..*close].trim().is_empty());
    let kept = text[..range.start].trim_end_matches(' ');
    match close {
        Some(close) => {
            let end = range.end + close + 1;
            (kept.len()..end, ")".to_owned(), 1)
        }
        None => (range.clone(), String::new(), 0),
    }
}

/// `\left` and the delimiter written `delimiter` (`(`, `\{`, `\langle`)
/// at `range`: both delimiters, made as large as what will be between
/// them, the cursor between.
fn left_delimiter(source: &Source, range: &Range<usize>, delimiter: &str) -> Typed {
    let pair = match delimiter {
        "(" => "(|)",
        "[" => "[|]",
        "\\{" | "\\lbrace" => "{|}",
        "|" | "\\vert" | "\\lvert" => "abs(|)",
        "\\|" | "\\Vert" | "\\lVert" => "norm(|)",
        "\\langle" => "lr(chevron.l | chevron.r)",
        "\\lfloor" => "floor(|)",
        "\\lceil" => "ceil(|)",
        "." => "lr(|)",
        _ => "|",
    };
    let (pair, within) = cursor_in(pair);
    let (with, lead) = apart(source, range, &pair);
    (range.clone(), with, lead + within)
}

/// `\right` at `range`: over the delimiter `\left` put in, where it
/// stands behind the cursor. `None` where none does.
fn right_delimiter(source: &Source, range: &Range<usize>) -> Option<Typed> {
    let text = source.text();
    let rest = &text[range.end..];
    let after = rest.trim_start_matches(' ');
    let blanks = rest.len() - after.len();
    let close = ["chevron.r)", ")", "]", "}"]
        .into_iter()
        .find(|close| after.starts_with(close))?;
    let end = range.end + blanks + close.len();
    Some((range.start..end, close.to_owned(), close.len()))
}

/// What `done` (the end of a name) leaves, and then `c` typed behind it,
/// as one change: `\alpha+` is `alpha+`, `\alpha$` leaves the equation.
fn then(source: &Source, done: Option<Typed>, range: &Range<usize>, c: char) -> Typed {
    // A name that stays text: `c` is typed behind it.
    let (first, mut with, _) = done.unwrap_or_else(|| {
        let kept = source.text()[range.clone()].to_owned();
        (range.clone(), kept, 0)
    });
    // `alpha2` would be a name, `alpha(` would call the symbol.
    let last = with.chars().next_back();
    let named = with.trim().chars().count() > 1;
    if last.is_some_and(|last| joins(last, c) || (c == '(' && named && last.is_alphabetic())) {
        with.push(' ');
    }
    let mut changed = source.clone();
    changed.edit(first.clone(), &with);
    let at = first.start + with.len();
    let second = typst_text::typed(&changed, at, c).unwrap_or_else(|| paired(&changed, at, c));
    compose(source.text(), (&first, &with), &second)
}

/// What `done` puts in, and `c` typed where its cursor is (the first
/// place of `frac(□, □)`).
fn then_inside(source: &Source, done: Typed, c: char) -> Typed {
    let (first, with, within) = done;
    let mut changed = source.clone();
    changed.edit(first.clone(), &with);
    let at = first.start + within;
    let second = typst_text::typed(&changed, at, c).unwrap_or_else(|| paired(&changed, at, c));
    compose(source.text(), (&first, &with), &second)
}

/// `c` typed at byte `at` of `source` where nothing has a say: as the
/// editor types it, brackets in pairs.
fn paired(source: &Source, at: usize, c: char) -> Typed {
    let after = source.text()[at..].chars().next();
    let room = after.is_none_or(|c| c.is_whitespace() || ")]},;$\"".contains(c));
    match c {
        // Over the closing one that stands there.
        ')' | ']' | '}' | '"' | '$' if after == Some(c) => (at..at + 1, c.to_string(), 1),
        '(' if room => (at..at, "()".to_owned(), 1),
        '[' if room => (at..at, "[]".to_owned(), 1),
        c => (at..at, c.to_string(), c.len_utf8()),
    }
}

/// Two changes one after the other as one: `first` of `text`, then
/// `second` of what that left.
fn compose(text: &str, first: (&Range<usize>, &str), second: &Typed) -> Typed {
    let (range, with) = first;
    let (second_range, second_with, within) = second;
    let mut last = text.to_owned();
    last.replace_range(range.clone(), with);
    last.replace_range(second_range.clone(), second_with);
    let start = range.start.min(second_range.start);
    // Where the second ends, in the bytes of `text`.
    let shown_end = range.start + with.len();
    let end = if second_range.end >= shown_end {
        second_range.end + range.len() - with.len()
    } else {
        range.end
    }
    .max(range.end);
    let tail = text.len() - end;
    let put = last[start..last.len() - tail].to_owned();
    (start..end, put, second_range.start + within - start)
}

/// How many bytes of blanks, a comma and blanks begin `text` with, where
/// it begins with blanks and a comma.
fn to_next_place(text: &str) -> Option<usize> {
    let rest = text.trim_start_matches(' ').strip_prefix(',')?;
    Some(text.len() - rest.trim_start_matches(' ').len())
}

/// What typing `c` at byte `at` of an equation of `source` does, where
/// no name is being typed.
fn in_math(source: &Source, at: usize, c: char) -> Option<Typed> {
    let text = source.text();
    let before = &text[..at];
    let after = &text[at..];
    let next = after.trim_start_matches(' ');
    let blanks = after.len() - next.len();
    let scripted = (before.ends_with('^') || before.ends_with('_')) && !before.ends_with("\\^");
    let nothing = || Some((at..at, String::new(), 0));
    let over = |length: usize| Some((at..at + length, text[at..at + length].to_owned(), length));
    match c {
        // A backslash begins a name.
        '\\' => Some((at..at, "\"\\\\\"".to_owned(), 3)),
        // Braces behind `^` and `_` group what is raised or lowered.
        '{' if scripted => Some((at..at, "()".to_owned(), 1)),
        // `}` closes what was begun: out of its brackets, or on to its
        // next place. Elsewhere braces show nothing in LaTeX, nor here.
        '}' if next.starts_with(')') => Some(close(source, at, blanks)),
        '}' if innermost_call(source, at).is_some() => {
            to_next_place(after).map_or_else(nothing, over)
        }
        '{' | '}' => nothing(),
        ']' if innermost_call(source, at).as_deref() == Some("root") => {
            to_next_place(after).and_then(over)
        }
        '&' if innermost_call(source, at).as_deref() == Some("mat") => {
            let kept = before.trim_end_matches(' ');
            Some((kept.len()..at, ", ".to_owned(), 2))
        }
        // Blanks mean nothing in an equation: one is enough.
        ' ' if before.ends_with([' ', '\n']) && !before.ends_with("\\ ") => nothing(),
        // The note of `\underbrace{…}_{…}` is its second place already.
        '^' | '_' if empty_second_of_brace(source, at) => nothing(),
        '$' => dollars(source, at),
        _ => None,
    }
}

/// Out of the brackets that stand `blanks` behind byte `at`: `x^{2n}`,
/// `\frac{a}{b}`. `bb(R)` closed so is `RR`.
fn close(source: &Source, at: usize, blanks: usize) -> Typed {
    let text = source.text();
    let end = at + blanks + 1;
    if let Some(open) = text[..at].rfind("bb(") {
        let inside = text[open + 3..at].trim();
        let letter = inside.len() == 1 && inside.chars().all(|c| c.is_ascii_uppercase());
        let alone = !text[..open].ends_with(|c: char| c.is_alphanumeric() || c == '.');
        if letter && alone {
            let double = format!("{inside}{inside}");
            let (with, _) = apart(source, &(open..end), &double);
            let within = with.len();
            return (open..end, with, within);
        }
    }
    let kept = text[at..end].to_owned();
    let within = kept.len();
    (at..end, kept, within)
}

/// Whether byte `at` of `source` is the empty second place of
/// `underbrace(…, )` or `overbrace(…, )`.
fn empty_second_of_brace(source: &Source, at: usize) -> bool {
    let text = source.text();
    let call = innermost_call(source, at);
    matches!(call.as_deref(), Some("underbrace" | "overbrace"))
        && text[..at].trim_end().ends_with(',')
        && text[at..].trim_start().starts_with(')')
}

/// `}` typed at the end of text in an equation, `\text{…}`: out of the
/// text, and out of `op("…")` that it is all of.
fn out_of_text(source: &Source, at: usize, c: char) -> Option<Typed> {
    if c != '}' || !typst_text::is_in_equation(source, at) {
        return None;
    }
    let leaf = LinkedNode::new(source.root()).leaf_at(at, Side::After)?;
    if leaf.kind() != SyntaxKind::Str || leaf.range().end != at + 1 {
        return None;
    }
    let text = source.text();
    let start = leaf.range().start;
    let whole_argument = text[..start].ends_with('(') && text[at + 1..].starts_with(')');
    let end = at + 1 + usize::from(whole_argument);
    Some((at..end, text[at..end].to_owned(), end - at))
}

/// `(` or `[` typed in running text right behind a backslash (written
/// `\\` there): `\(` begins an equation in the line, `\[` one that
/// stands by itself.
fn in_markup(source: &Source, at: usize, c: char) -> Option<Typed> {
    if !matches!(c, '(' | '[') {
        return None;
    }
    let leaf = LinkedNode::new(source.root()).leaf_at(at, Side::Before)?;
    if leaf.kind() != SyntaxKind::Escape || leaf.range().end != at || leaf.leaf_text() != "\\\\" {
        return None;
    }
    let (with, within) = if c == '(' { ("$$", 1) } else { ("$  $", 2) };
    Some((leaf.range(), with.to_owned(), within))
}

/// `$` typed in an equation with nothing in it. The sign itself, as
/// without LaTeX (`$$5` is "$5"); but where a dollar sign so written
/// stands before in the paragraph with mathematics after it, the two
/// `$$` were LaTeX's: what is between is a displayed equation.
fn dollars(source: &Source, at: usize) -> Option<Typed> {
    let equation = typst_text::equation_at(source, at)?;
    if equation != (at.saturating_sub(1)..at + 1) {
        return None;
    }
    let text = source.text();
    let paragraph = text[..equation.start]
        .rfind("\n\n")
        .map_or(0, |found| found + 2);
    let before = &text[paragraph..equation.start];
    let sign = before
        .match_indices("\\$")
        .map(|(at, _)| at)
        .filter(|at| {
            let run = before[..*at].len() - before[..*at].trim_end_matches('\\').len();
            run.is_multiple_of(2)
        })
        .last();
    if let Some(sign) = sign {
        let opening = paragraph + sign;
        let read = unescaped(&text[opening + 2..equation.start]);
        let price = read.trim_start().starts_with(|c: char| c.is_ascii_digit());
        let mathematics =
            super::looks_like_latex(&read) || read.contains(['^', '_', '=', '<', '>', '+', '{']);
        if mathematics && !price && !read.trim().is_empty() && !read.contains('$') {
            let converted = super::to_typst(read.trim()).text;
            let body = if converted.trim().is_empty() {
                "\"\"".to_owned()
            } else {
                converted
            };
            let with = format!("$ {body} $");
            let within = with.len();
            return Some((opening..equation.end, with, within));
        }
    }
    Some((equation, "\\$".to_owned(), 2))
}

/// Running text as it reads: its escapes are the signs.
fn unescaped(text: &str) -> String {
    let mut read = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                read.push(next);
            }
        } else {
            read.push(c);
        }
    }
    read
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `text` with `keys` typed at its `|`, each through the editor's
    /// own `typed`, as the page types them: the text with `|` where the
    /// cursor is. After every key the text is read by Typst.
    fn typing(text: &str, keys: &str) -> String {
        let at = text.find('|').unwrap_or(text.len());
        let mut text = text.replacen('|', "", 1);
        let mut cursor = at;
        for c in keys.chars() {
            let source = Source::detached(text.clone());
            let (range, with, within) =
                typst_text::typed(&source, cursor, c).unwrap_or_else(|| paired(&source, cursor, c));
            text.replace_range(range.clone(), &with);
            cursor = range.start + within.min(with.len());
            // A place left empty is shown filled by the page.
            let source = Source::detached(text.clone());
            let mut errors = source.root().errors_and_warnings().0;
            errors.retain(|error| error.message != "expected expression");
            assert!(errors.is_empty(), "{c:?} left {text:?}: {errors:?}");
        }
        text.insert(cursor, '|');
        text
    }

    #[test]
    fn a_name_is_text_until_it_is_over() {
        assert_eq!(typing("$|$", "\\al"), "$\"\\\\al|\"$");
        assert_eq!(typing("$|$", "\\alpha "), "$alpha |$");
        assert_eq!(typing("$|$", "\\alpha+"), "$alpha+|$");
        assert_eq!(typing("$|$", "\\alpha2"), "$alpha 2|$");
        assert_eq!(typing("$|$", "\\alpha("), "$alpha (|)$");
        assert_eq!(typing("$x|$", "\\alpha"), "$x\"\\\\alpha|\"$");
        assert_eq!(typing("$x|$", "\\alpha "), "$x alpha |$");
        assert_eq!(typing("$|$", "\\leq "), "$<= |$");
        assert_eq!(typing("$a |$", "\\leq b"), "$a <= b|$");
        assert_eq!(typing("$|$", "\\mathbb{R}"), "$RR|$");
        assert_eq!(typing("$|$", "\\foo "), "$\"\\\\foo\" |$");
        assert_eq!(typing("$|$", "\\alpha\\beta "), "$alpha beta |$");
        // An equation by itself, as asked: it keeps its blank.
        assert_eq!(typing("$|$", "\\displaystyle x"), "$ x| $");
    }

    #[test]
    fn a_backslash_before_the_closing_dollar_breaks_nothing() {
        assert_eq!(typing("A $x|$ b", "\\"), "A $x\"\\\\|\"$ b");
        assert_eq!(typing("A $x|$ b", "\\$"), "A $x\\$|$ b");
        assert_eq!(typing("A $x|$ b", "\\,y"), "A $x thin y|$ b");
        assert_eq!(typing("A $x|$ b", "\\quad\\,y"), "A $x quad thin y|$ b");
    }

    #[test]
    fn a_fraction_is_typed_as_in_latex() {
        assert_eq!(typing("$|$", "\\frac{"), "$frac(|, )$");
        assert_eq!(typing("$|$", "\\frac{a"), "$frac(a|, )$");
        assert_eq!(typing("$|$", "\\frac{a}"), "$frac(a, |)$");
        assert_eq!(typing("$|$", "\\frac{a}{"), "$frac(a, |)$");
        assert_eq!(typing("$|$", "\\frac{a}{b}"), "$frac(a, b)|$");
        assert_eq!(typing("$|$", "\\frac{a}{b} + 1"), "$frac(a, b) + 1|$");
        assert_eq!(typing("$|$", "\\frac1"), "$frac(1|, )$");
        assert_eq!(typing("$|$", "\\sqrt[3]{x}"), "$root(3, x)|$");
        assert_eq!(typing("$|$", "\\sqrt{x}"), "$sqrt(x)|$");
        assert_eq!(typing("$|$", "\\text{if}"), "$\"if\"|$");
        assert_eq!(typing("$|$", "\\operatorname{tr}"), "$op(\"tr\")|$");
        assert_eq!(typing("$|$", "\\underbrace{a}_{n}"), "$underbrace(a, n)|$");
    }

    #[test]
    fn braces_behind_a_script_group_it() {
        assert_eq!(typing("$x|$", "^{2n}"), "$x^(2n)|$");
        assert_eq!(typing("$x|$", "_{i}^{2}"), "$x_(i)^(2)|$");
        assert_eq!(typing("$|$", "{a}"), "$a|$");
    }

    #[test]
    fn matrices_and_cases_take_their_cells() {
        assert_eq!(
            typing("$|$", "\\begin{pmatrix}a & b \\\\ c & d\\end{pmatrix}"),
            "$mat(a, b; c, d)|$"
        );
        assert_eq!(
            typing("$|$", "\\begin{bmatrix}1\\end{bmatrix}"),
            "$mat(delim: \"[\", 1)|$"
        );
        assert_eq!(
            typing("$|$", "\\begin{cases}1 & x \\\\ 0\\end{cases}"),
            "$cases(1 & x, 0)|$"
        );
        assert_eq!(typing("$ | $", "a \\\\ b"), "$ a \\\nb| $");
        assert_eq!(typing("$|$", "\\begin{weird}"), "$\"\\\\begin{weird}\"|$");
    }

    #[test]
    fn delimiters_are_made_large() {
        assert_eq!(typing("$|$", "\\left( x \\right)"), "$( x )|$");
        assert_eq!(typing("$|$", "\\left\\{ x \\right\\}"), "${ x }|$");
        assert_eq!(typing("$|$", "\\left| x \\right|"), "$abs( x )|$");
        assert_eq!(
            typing("$|$", "\\left\\langle x \\right\\rangle "),
            "$lr(chevron.l x chevron.r)|$"
        );
        assert_eq!(typing("$|$", "\\not="), "$!=|$");
        assert_eq!(typing("$|$", "\\not\\in "), "$in.not|$");
    }

    #[test]
    fn whole_commands_are_made_when_they_are_over() {
        assert_eq!(typing("$|$", "\\overset{!}{=} "), "$limits(=)^(!) |$");
        assert_eq!(typing("$|$", "\\pmod{n}"), "$\"\\\\pmod{n}|\"$");
        assert_eq!(typing("$|$", "\\pmod{n}+"), "$(mod n)+|$");
    }

    #[test]
    fn running_text_begins_equations_as_latex_does() {
        assert_eq!(typing("A |", "\\(x\\)"), "A $x$|");
        assert_eq!(typing("A |", "\\[x\\]"), "A $ x $|");
        assert_eq!(typing("A |", "$$x^2 = y$$"), "A $ x^2 = y $|");
        assert_eq!(typing("A |", "$$5"), "A \\$5|");
    }

    #[test]
    fn what_backspace_and_leaving_do() {
        let source = Source::detached("$x\"\\\\\"$");
        assert_eq!(backspace(&source, 5), Some((2..6, String::new())));
        let source = Source::detached("$x\"\\\\alpha\"$");
        assert_eq!(left(&source, 2), Some((2..11, " alpha".to_owned())));
        let source = Source::detached("$\"\\\\frac\"$");
        assert_eq!(left(&source, 1), Some((1..9, "frac(, )".to_owned())));
        let source = Source::detached("$\"\\\\foo\"$");
        assert_eq!(left(&source, 1), None);
        assert!(unknown("foo") && !unknown("alpha") && !unknown("frac"));
    }
}
