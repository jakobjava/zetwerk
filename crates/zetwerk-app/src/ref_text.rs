//! What of a document can be referred to, and what refers to what: read
//! off the source as text, with nothing compiled.
//!
//! A reference (`@name`) needs a label (`<name>`) on the thing it is to
//! show the number of. Nobody wants to think of labels: here is what
//! finds the sections, figures, tables and equations of a text, with or
//! without one; what makes a label for one that has none and says where
//! it goes; and what finds the references that lead nowhere, the labels
//! nothing refers to and those that are there twice.
//!
//! What the things are numbered as is not known here: that is the
//! compiled document's to say (see `viewer::refer`).

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use typst::syntax::{LinkedNode, Side, Source, Span, SyntaxKind, SyntaxNode};

/// What kind of thing can be referred to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A heading and what is under it.
    Section,
    /// A figure that is no table and no listing.
    Figure,
    /// A figure that holds a table.
    Table,
    /// A figure that holds code.
    Listing,
    /// An equation that stands by itself.
    Equation,
    /// A source of the bibliography.
    Source,
}

impl Kind {
    /// What it is called in a list.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Section => "section",
            Self::Figure => "figure",
            Self::Table => "table",
            Self::Listing => "listing",
            Self::Equation => "equation",
            Self::Source => "source",
        }
    }

    /// What the labels made for things of this kind begin with.
    #[must_use]
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Section => "sec",
            Self::Figure => "fig",
            Self::Table => "tab",
            Self::Listing => "lst",
            Self::Equation => "eq",
            Self::Source => "src",
        }
    }
}

/// Something in the text that can be referred to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub kind: Kind,
    /// Its label, if it has one.
    pub label: Option<String>,
    /// What it is called: the title of a section, the caption of a
    /// figure (or else the name of its image), an equation as written.
    pub called: String,
    /// The bytes of it: the heading, the call of the figure, the
    /// equation. A label goes right behind them.
    pub range: Range<usize>,
    /// How deep a heading is, counting from 1; 0 for all else.
    pub level: usize,
    /// For an equation, whether it is written so that it has a number
    /// of its own (`Some(true)`), so that it has none (`Some(false)`), or
    /// as the document's rules have it (`None`, as for all else).
    pub numbers: Option<bool>,
    /// The node it is in the syntax tree: what the compiled document
    /// knows it by.
    pub span: Span,
}

/// What a text has to do with referring: what can be referred to, the
/// labels it gives and the labels it refers to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Marks {
    /// In the order of the text.
    pub targets: Vec<Target>,
    /// The labels put on things, and the bytes each is written at.
    pub defined: Vec<(String, Range<usize>)>,
    /// The labels referred to (by `@name`, or as `<name>` handed to a
    /// function), and the bytes each is written at.
    pub used: Vec<(String, Range<usize>)>,
    /// Whether a rule of this text says that headings are numbered, or
    /// that they are not; `None` if it has no such rule.
    pub sections_numbered: Option<bool>,
    /// The same for equations.
    pub equations_numbered: Option<bool>,
    /// Whether the text makes labels by code (`#label("name")`): there
    /// may be labels then that are written nowhere as such.
    pub makes_labels: bool,
}

/// What `source` has to do with referring. One walk over its syntax
/// tree: no more than a few milliseconds for a book.
#[must_use]
pub fn read(source: &Source) -> Marks {
    let mut marks = Marks::default();
    gather(source.root(), 0, source.text(), &mut marks);
    marks
}

/// The part of `text` that `node`, which begins at byte `at`, is.
fn written<'a>(text: &'a str, at: usize, node: &SyntaxNode) -> &'a str {
    text.get(at..at + node.len()).unwrap_or("")
}

/// Looks through the children of `node`, which begins at byte `start`.
fn gather(node: &SyntaxNode, start: usize, text: &str, marks: &mut Marks) {
    let in_markup = node.kind() == SyntaxKind::Markup;
    // What a label written next would be the label of, if that is
    // something to refer to: a label goes on what stands before it.
    let mut before: Option<usize> = None;
    let mut at = start;
    for child in node.children() {
        let from = at;
        at += child.len();
        if child.kind() == SyntaxKind::FuncCall && written(text, from, child).starts_with("label(")
        {
            marks.makes_labels = true;
        }
        let found = match child.kind() {
            // Nothing a label could be on, and nothing between a thing
            // and its label either.
            SyntaxKind::Space
            | SyntaxKind::Parbreak
            | SyntaxKind::LineComment
            | SyntaxKind::BlockComment
            | SyntaxKind::Hash => continue,
            SyntaxKind::Label if in_markup => {
                let name = name_of(written(text, from, child));
                if let Some(target) = before.and_then(|index| marks.targets.get_mut(index)) {
                    target.label = Some(name.clone());
                }
                marks.defined.push((name, from..at));
                continue;
            }
            // Handed to a function: `#ref(<name>)`, `#cite(<name>)`.
            SyntaxKind::Label | SyntaxKind::RefMarker => {
                let name = name_of(written(text, from, child));
                marks.used.push((name, from..at));
                None
            }
            SyntaxKind::Heading if in_markup => {
                gather(child, from, text, marks);
                Some(heading(child, from, text))
            }
            SyntaxKind::Equation if in_markup => standing(child).then(|| Target {
                kind: Kind::Equation,
                label: None,
                called: one_line(written(text, from, child).trim_matches('$')),
                range: from..at,
                level: 0,
                numbers: None,
                span: child.span(),
            }),
            SyntaxKind::FuncCall if in_markup => {
                let found = call(child, from, text);
                // What is in an equation refers to nothing; a caption
                // may.
                if found
                    .as_ref()
                    .is_none_or(|found| found.kind != Kind::Equation)
                {
                    gather(child, from, text, marks);
                }
                found
            }
            SyntaxKind::SetRule => {
                rule(child, from, text, marks);
                None
            }
            // What refers to nothing and holds nothing that does.
            SyntaxKind::Text | SyntaxKind::Equation | SyntaxKind::Raw => None,
            _ => {
                gather(child, from, text, marks);
                None
            }
        };
        before = found.map(|target| {
            marks.targets.push(target);
            marks.targets.len() - 1
        });
    }
}

/// The name in a label or a reference as written: `<name>`, `@name`.
fn name_of(written: &str) -> String {
    written
        .trim_start_matches(['<', '@'])
        .trim_end_matches('>')
        .to_owned()
}

/// Whether an equation stands by itself, and is not part of a line.
fn standing(equation: &SyntaxNode) -> bool {
    let mut children = equation.children();
    let is_space =
        |node: Option<&SyntaxNode>| node.map(SyntaxNode::kind) == Some(SyntaxKind::Space);
    is_space(children.nth(1)) && is_space(children.nth_back(1))
}

/// The heading `node`, which begins at byte `at`, as something to refer
/// to.
fn heading(node: &SyntaxNode, at: usize, text: &str) -> Target {
    let mut level = 0;
    let mut called = String::new();
    let mut from = at;
    for child in node.children() {
        match child.kind() {
            SyntaxKind::HeadingMarker => level = child.len(),
            SyntaxKind::Markup => called = read_as_text(written(text, from, child)),
            _ => {}
        }
        from += child.len();
    }
    Target {
        kind: Kind::Section,
        label: None,
        called,
        range: at..at + node.len(),
        level,
        numbers: None,
        span: node.span(),
    }
}

/// The arguments of the call `node`, which begins at byte `at`: what it
/// is a call of, and each argument with its name (empty for one that
/// has none) and the byte it begins at.
fn arguments<'a>(
    node: &'a SyntaxNode,
    at: usize,
    text: &'a str,
) -> (&'a str, Vec<(&'a str, &'a SyntaxNode, usize)>) {
    let mut called = "";
    let mut given = Vec::new();
    let mut from = at;
    for child in node.children() {
        if child.kind() == SyntaxKind::Args {
            let mut within = from;
            for argument in child.children() {
                match argument.kind() {
                    SyntaxKind::Named => {
                        let mut parts = argument.children();
                        let name = parts.next().map_or("", |name| written(text, within, name));
                        let value = argument.children().next_back();
                        if let Some(value) = value {
                            let begins = within + argument.len() - value.len();
                            given.push((name, value, begins));
                        }
                    }
                    SyntaxKind::LeftParen
                    | SyntaxKind::RightParen
                    | SyntaxKind::Comma
                    | SyntaxKind::Space
                    | SyntaxKind::LineComment
                    | SyntaxKind::BlockComment => {}
                    _ => given.push(("", argument, within)),
                }
                within += argument.len();
            }
        } else if matches!(child.kind(), SyntaxKind::Ident | SyntaxKind::FieldAccess) {
            called = written(text, from, child);
        }
        from += child.len();
    }
    (called, given)
}

/// The call `node`, which begins at byte `at`, as something to refer to,
/// if it is a figure or an equation written as a call.
fn call(node: &SyntaxNode, at: usize, text: &str) -> Option<Target> {
    let (function, given) = arguments(node, at, text);
    let named = |wanted: &str| {
        given
            .iter()
            .find(|(name, _, _)| *name == wanted)
            .map(|(_, value, begins)| written(text, *begins, value))
    };
    let whole = at..at + node.len();
    match function {
        "figure" => {
            // What it holds says what it is, unless it says so itself.
            let held = given.iter().find(|(name, _, _)| name.is_empty());
            let holds = held.map_or("", |(_, value, begins)| {
                let inner = written(text, *begins, value);
                match value.kind() {
                    SyntaxKind::Raw => "raw",
                    _ => inner.split(['(', '[']).next().unwrap_or(""),
                }
            });
            let kind = match named("kind").unwrap_or(holds).trim_matches('"') {
                "table" => Kind::Table,
                "raw" => Kind::Listing,
                _ => Kind::Figure,
            };
            let caption = named("caption").map(|caption| {
                let inner = caption
                    .strip_prefix('[')
                    .and_then(|rest| rest.strip_suffix(']'));
                read_as_text(inner.unwrap_or(caption).trim_matches('"'))
            });
            // Without a caption, the image's file is what it is known by.
            let file = || {
                let (_, value, begins) = held?;
                let inner = written(text, *begins, value);
                let path = inner.strip_prefix("image(\"")?.split('"').next()?;
                let name = path.rsplit('/').next()?;
                Some(
                    name.rsplit_once('.')
                        .map_or(name, |(stem, _)| stem)
                        .to_owned(),
                )
            };
            let called = caption
                .filter(|caption| !caption.is_empty())
                .or_else(file)
                .unwrap_or_default();
            Some(Target {
                kind,
                label: None,
                called,
                range: whole,
                level: 0,
                numbers: None,
                span: node.span(),
            })
        }
        "math.equation" if named("block") == Some("true") => {
            let held = given
                .iter()
                .find(|(name, value, _)| name.is_empty() && value.kind() == SyntaxKind::Equation);
            let called = held.map_or_else(String::new, |(_, value, begins)| {
                one_line(written(text, *begins, value).trim_matches('$'))
            });
            Some(Target {
                kind: Kind::Equation,
                label: None,
                called,
                range: whole,
                level: 0,
                numbers: Some(named("numbering").is_some_and(|numbering| numbering != "none")),
                span: node.span(),
            })
        }
        _ => None,
    }
}

/// Notes what the set rule `node`, which begins at byte `at`, says of
/// the numbers of headings and equations.
fn rule(node: &SyntaxNode, at: usize, text: &str, marks: &mut Marks) {
    // A set rule is written like a call behind its `set`.
    let (function, given) = arguments(node, at, text);
    let numbered = given
        .iter()
        .find(|(name, _, _)| *name == "numbering")
        .map(|(_, value, begins)| written(text, *begins, value) != "none");
    match (function, numbered) {
        ("heading", Some(numbered)) => marks.sections_numbered = Some(numbered),
        ("math.equation", Some(numbered)) => marks.equations_numbered = Some(numbered),
        _ => {}
    }
}

/// `written` on one line, with one blank wherever it has blanks.
fn one_line(written: &str) -> String {
    written.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Markup as it is read on the page, as far as that can be told without
/// typesetting it: without the marks of bold and italic, of code and of
/// equations, and without the backslashes that take a character's
/// meaning away.
fn read_as_text(markup: &str) -> String {
    let mut read = String::new();
    let mut characters = markup.chars();
    while let Some(c) = characters.next() {
        match c {
            '\\' => read.extend(characters.next()),
            '*' | '_' | '`' | '$' => {}
            _ => read.push(c),
        }
    }
    one_line(&read)
}

/// Words that say nothing of what a title is about.
const SMALL_WORDS: &[&str] = &[
    "a", "an", "the", "of", "and", "or", "for", "in", "on", "to", "with", "by", "at", "as", "is",
    "are", "from", "its", "their", "this", "that", "der", "die", "das", "und", "von", "de", "het",
    "een", "en", "van",
];

/// Names in equations that say how something is set, not what it is.
const MATH_WORDS: &[&str] = &[
    "frac",
    "sqrt",
    "root",
    "upright",
    "bold",
    "italic",
    "cal",
    "bb",
    "text",
    "lr",
    "mat",
    "vec",
    "cases",
    "dif",
    "quad",
    "wide",
    "thin",
    "med",
    "thick",
    "limits",
    "scripts",
    "op",
    "attach",
    "binom",
    "cancel",
    "overline",
    "underline",
    "hat",
    "tilde",
    "dot",
    "ddot",
    "bar",
    "arrow",
    "left",
    "right",
    "mid",
    "display",
    "inline",
];

/// How many words of a title go into a label.
const LABEL_WORDS: usize = 3;

/// A label for a thing of `kind` that is called `called` and has none:
/// its kind and the first words of what it is called that say something
/// (`sec:main-result`, `fig:plot`), or a number where there are no such
/// words. `taken` says whether a label is in use: the one made is not.
#[must_use]
pub fn label_for(kind: Kind, called: &str, taken: &dyn Fn(&str) -> bool) -> String {
    let lowered = called.to_lowercase();
    let all: Vec<&str> = lowered
        .split(|c: char| !(c.is_alphabetic() || c.is_ascii_digit()))
        .filter(|word| !word.is_empty())
        .collect();
    let dull: &[&str] = if kind == Kind::Equation {
        MATH_WORDS
    } else {
        SMALL_WORDS
    };
    let mut words: Vec<&str> = all
        .iter()
        .copied()
        .filter(|word| !dull.contains(word))
        .collect();
    if words.is_empty() && kind != Kind::Equation {
        words = all;
    }
    words.truncate(LABEL_WORDS);
    // Its words; with a number behind them, if another has them; a
    // number alone, if it has none.
    let stem = format!("{}:{}", kind.prefix(), words.join("-"));
    if !words.is_empty() && !taken(&stem) {
        return stem;
    }
    let mut count = if words.is_empty() { 1 } else { 2 };
    loop {
        let label = match words.len() {
            0 => format!("{stem}{count}"),
            _ => format!("{stem}-{count}"),
        };
        if !taken(&label) {
            return label;
        }
        count += 1;
    }
}

/// The change that makes `target` something a reference can be to: the
/// bytes to replace and what to put there. With `label` it gets that
/// label, right behind it (the line of a heading, the call of a figure,
/// an equation's last dollar sign). With `number` an equation written
/// between dollar signs is made one that has a number, as it has to
/// have one to be referred to. `None` if there is nothing to change.
#[must_use]
pub fn prepared(
    text: &str,
    target: &Target,
    label: Option<&str>,
    number: bool,
) -> Option<(Range<usize>, String)> {
    let end = target.range.end;
    let label = label.map(|label| format!(" <{label}>")).unwrap_or_default();
    let bare = target.kind == Kind::Equation && target.numbers.is_none();
    match text.get(target.range.clone()) {
        Some(equation) if number && bare => Some((
            target.range.clone(),
            format!("#math.equation(block: true, numbering: \"(1)\", {equation}){label}"),
        )),
        _ if label.is_empty() => None,
        _ => Some((end..end, label)),
    }
}

/// What is written to refer to the thing with `label`: a reference,
/// which shows its number; or, for a section where sections have no
/// numbers (`numbered` is false), a link that reads as its title.
#[must_use]
pub fn reference(kind: Kind, label: &str, called: &str, numbered: bool) -> String {
    if numbered || kind != Kind::Section || called.is_empty() {
        return format!("@{label}");
    }
    let mut title = String::new();
    for c in called.chars() {
        if matches!(
            c,
            '\\' | '*' | '_' | '`' | '#' | '@' | '<' | '[' | ']' | '$' | '~' | '/'
        ) {
            title.push('\\');
        }
        title.push(c);
    }
    format!("#link(<{label}>)[{title}]")
}

/// The label that is written at byte `at` of `source`: its name, the
/// bytes it is written at, and whether it is put on something there
/// (`<name>` in running text) or referred to (`@name`, `#ref(<name>)`).
#[must_use]
pub fn label_at(source: &Source, at: usize) -> Option<(String, Range<usize>, bool)> {
    let leaf = LinkedNode::new(source.root()).leaf_at(at, Side::After)?;
    let defines = match leaf.kind() {
        SyntaxKind::RefMarker => false,
        SyntaxKind::Label => leaf
            .parent()
            .is_some_and(|parent| parent.kind() == SyntaxKind::Markup),
        _ => return None,
    };
    let name = name_of(source.text().get(leaf.range())?);
    Some((name, leaf.range(), defines))
}

/// What is wrong with a label or a reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    /// A reference to a label that is nowhere; and the label there is
    /// that is most like it, if one is.
    Missing(Option<String>),
    /// A label that is put on more than one thing: a reference to it
    /// cannot tell which is meant.
    Twice,
    /// A label nothing refers to.
    Unused,
}

/// Something wrong with a label or a reference, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub fault: Fault,
    /// The label it is about.
    pub label: String,
    /// The bytes it is written at.
    pub range: Range<usize>,
}

impl Problem {
    /// What is wrong, in a line.
    #[must_use]
    pub fn told(&self) -> String {
        let label = &self.label;
        match &self.fault {
            Fault::Missing(Some(nearest)) => {
                format!(
                    "@{label} refers to nothing: there is no such label (but there is {nearest})"
                )
            }
            Fault::Missing(None) => format!("@{label} refers to nothing: there is no such label"),
            Fault::Twice => format!("<{label}> is the label of more than one thing"),
            Fault::Unused => format!("<{label}> is referred to nowhere"),
        }
    }
}

/// What is wrong with the labels and references of a text: references
/// to labels that do not exist, labels that are put on two things, and
/// labels nothing refers to. First what breaks the document, then what
/// may; each in the order of the text.
///
/// A document may be more than this text: `elsewhere` are the labels
/// that exist outside it (in other files, as keys of the bibliography),
/// `used_elsewhere` says whether a label is referred to outside it.
#[must_use]
pub fn check(
    marks: &Marks,
    elsewhere: &[String],
    used_elsewhere: &dyn Fn(&str) -> bool,
) -> Vec<Problem> {
    let mut defined: HashMap<&str, usize> = HashMap::new();
    for (name, _) in &marks.defined {
        *defined.entry(name).or_default() += 1;
    }
    let used: HashSet<&str> = marks.used.iter().map(|(name, _)| name.as_str()).collect();
    let elsewhere: HashSet<&str> = elsewhere.iter().map(String::as_str).collect();
    let mut problems = Vec::new();
    for (name, range) in &marks.used {
        if !defined.contains_key(name.as_str()) && !elsewhere.contains(name.as_str()) {
            let known = defined.keys().chain(&elsewhere).copied();
            problems.push(Problem {
                fault: Fault::Missing(nearest(name, known).map(str::to_owned)),
                label: name.clone(),
                range: range.clone(),
            });
        }
    }
    for (name, range) in &marks.defined {
        if defined.get(name.as_str()) > Some(&1) || elsewhere.contains(name.as_str()) {
            problems.push(Problem {
                fault: Fault::Twice,
                label: name.clone(),
                range: range.clone(),
            });
        }
    }
    let mut seen = HashSet::new();
    for (name, range) in &marks.defined {
        let first = seen.insert(name.as_str());
        if first && !used.contains(name.as_str()) && !used_elsewhere(name) {
            problems.push(Problem {
                fault: Fault::Unused,
                label: name.clone(),
                range: range.clone(),
            });
        }
    }
    problems
}

/// The one of `labels` that is most like `name`, if one is like it at
/// all: what was probably meant by a reference that leads nowhere.
#[must_use]
pub fn nearest<'a>(name: &str, labels: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    // Without what says the kind: `plot` is like `fig:plot`.
    let tail = |label: &'a str| label.rsplit(':').next().unwrap_or(label);
    let own_tail = name.rsplit(':').next().unwrap_or(name);
    labels
        .filter(|label| *label != name)
        .filter_map(|label| {
            let whole = distance(name, label);
            let tails = distance(own_tail, tail(label)) + 1;
            let apart = whole.min(tails);
            let longer = name.chars().count().max(label.chars().count());
            (apart <= (longer / 3).max(2)).then_some((apart, label))
        })
        .min()
        .map(|(_, label)| label)
}

/// How many characters have to be put in, taken out or exchanged to
/// make `a` of `b`, capitals counting as small letters.
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().flat_map(char::to_lowercase).collect();
    let b: Vec<char> = b.chars().flat_map(char::to_lowercase).collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let exchanged = diagonal + usize::from(x != y);
            diagonal = row[j + 1];
            row[j + 1] = exchanged.min(row[j] + 1).min(diagonal + 1);
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(text: &str) -> Source {
        Source::detached(text)
    }

    /// The things of `text` that can be referred to, each as its kind,
    /// its label and what it is called.
    fn targets(text: &str) -> Vec<(Kind, Option<String>, String)> {
        read(&source(text))
            .targets
            .into_iter()
            .map(|target| (target.kind, target.label, target.called))
            .collect()
    }

    #[allow(clippy::unnecessary_wraps, reason = "what a label is compared with")]
    fn some(label: &str) -> Option<String> {
        Some(label.to_owned())
    }

    const PAPER: &str = "#set heading(numbering: \"1.1\")\n\
        = Introduction <intro>\n\
        As @fig:plot and @eq:energy show, see also @knuth1984[p.~3].\n\n\
        == The _main_ result\n\
        #figure(\n  image(\"plots/energy.png\"),\n  caption: [A plot of the *energy*],\n) <fig:plot>\n\n\
        #figure(table(columns: 2, [a], [b]), caption: [Two cells])\n\n\
        #figure(image(\"sketch.svg\"))\n\n\
        $ E = m c^2 $ <eq:energy>\n\n\
        Inline $x^2$ is nothing to refer to, #math.equation(block: true, numbering: \"(1)\", $ a + b $) is.\n\n\
        ```rust\nlet x = \"<no-label> @no-ref\";\n```\n\
        #ref(<intro>) and #cite(<knuth1984>, form: \"prose\").\n";

    #[test]
    fn what_can_be_referred_to_is_found() {
        assert_eq!(
            targets(PAPER),
            [
                (Kind::Section, some("intro"), "Introduction".to_owned()),
                (Kind::Section, None, "The main result".to_owned()),
                (
                    Kind::Figure,
                    some("fig:plot"),
                    "A plot of the energy".to_owned()
                ),
                (Kind::Table, None, "Two cells".to_owned()),
                (Kind::Figure, None, "sketch".to_owned()),
                (Kind::Equation, some("eq:energy"), "E = m c^2".to_owned()),
                (Kind::Equation, None, "a + b".to_owned()),
            ]
        );
        let marks = read(&source(PAPER));
        assert_eq!(marks.sections_numbered, Some(true));
        assert_eq!(marks.equations_numbered, None);
        assert_eq!(marks.targets[1].level, 2);
        // Only the equation written as a call says of itself that it
        // has a number.
        let numbers: Vec<Option<bool>> =
            marks.targets.iter().map(|target| target.numbers).collect();
        assert_eq!(numbers[5..], [None, Some(true)]);
        let names = |marked: &[(String, Range<usize>)]| -> Vec<String> {
            marked.iter().map(|(name, _)| name.clone()).collect()
        };
        assert_eq!(names(&marks.defined), ["intro", "fig:plot", "eq:energy"]);
        assert_eq!(
            names(&marks.used),
            ["fig:plot", "eq:energy", "knuth1984", "intro", "knuth1984"]
        );
        // Each is where it is written.
        for (name, range) in marks.defined.iter().chain(&marks.used) {
            let written = &PAPER[range.clone()];
            assert!(
                written == format!("<{name}>") || written == format!("@{name}"),
                "{written}"
            );
        }
    }

    #[test]
    fn a_label_is_on_what_stands_before_it() {
        // On its own line, and after a blank line.
        assert_eq!(targets("= One\n<a>\n")[0].1, some("a"));
        assert_eq!(targets("$ x $\n\n<b>\n")[0].1, some("b"));
        // Not across other text, a rule, or into what holds the thing.
        assert_eq!(targets("= One\nText <a>\n")[0].1, None);
        assert_eq!(targets("$ x $\n#set text(red)\n<a>\n")[0].1, None);
        assert_eq!(targets("#box[= One] <a>\n")[0].1, None);
        // Inside what holds it, it is.
        assert_eq!(
            targets("#align(center)[#figure(image(\"a.png\")) <a>]")[0].1,
            some("a")
        );
        // The last of two labels is the one that counts.
        assert_eq!(targets("= One <a> <b>\n")[0].1, some("b"));
        // What a figure is can be said outright.
        assert_eq!(targets("#figure(kind: table, [x])")[0].0, Kind::Table);
        assert_eq!(targets("#figure(```rust\nx\n```)")[0].0, Kind::Listing);
        // Rules that take numbers away are heard too.
        let marks = read(&source(
            "#set heading(numbering: none)\n#set math.equation(numbering: \"(1)\")\n",
        ));
        assert_eq!(marks.sections_numbered, Some(false));
        assert_eq!(marks.equations_numbered, Some(true));
        // And labels made by code are known to be about.
        assert!(!marks.makes_labels);
        assert!(read(&source("Text #label(\"a\")\n")).makes_labels);
        assert!(read(&source("#box[Text #label(\"a\")]\n")).makes_labels);
    }

    #[test]
    fn labels_are_made_of_what_a_thing_is_called() {
        let free = |_: &str| false;
        assert_eq!(
            label_for(Kind::Section, "Main result", &free),
            "sec:main-result"
        );
        assert_eq!(label_for(Kind::Figure, "A plot", &free), "fig:plot");
        assert_eq!(
            label_for(Kind::Table, "Rates of the three schemes compared", &free),
            "tab:rates-three-schemes"
        );
        assert_eq!(
            label_for(Kind::Section, "Über die Größe", &free),
            "sec:über-größe"
        );
        assert_eq!(label_for(Kind::Equation, "E = m c^2", &free), "eq:e-m-c");
        assert_eq!(
            label_for(Kind::Equation, "frac(energy, 2)", &free),
            "eq:energy-2"
        );
        // Nothing but small words is better than nothing.
        assert_eq!(label_for(Kind::Section, "To be or", &free), "sec:be");
        assert_eq!(label_for(Kind::Section, "Of the", &free), "sec:of-the");
        // No words at all: a number.
        assert_eq!(label_for(Kind::Figure, "", &free), "fig:1");
        assert_eq!(label_for(Kind::Equation, "1 + 1", &free), "eq:1-1");
        assert_eq!(label_for(Kind::Equation, "sqrt(+)", &free), "eq:1");
        // And never one that is taken.
        let taken = |label: &str| matches!(label, "fig:plot" | "fig:plot-2" | "fig:1");
        assert_eq!(label_for(Kind::Figure, "The plot", &taken), "fig:plot-3");
        assert_eq!(label_for(Kind::Figure, "", &taken), "fig:2");
    }

    /// `text` after the `index`th thing of it got `label`.
    fn with_label(text: &str, index: usize, label: &str, number: bool) -> String {
        let marks = read(&source(text));
        let label = (!label.is_empty()).then_some(label);
        let mut changed = text.to_owned();
        if let Some((range, with)) = prepared(text, &marks.targets[index], label, number) {
            changed.replace_range(range, &with);
        }
        changed
    }

    #[test]
    fn a_label_goes_right_behind_its_thing() {
        assert_eq!(
            with_label("= One\nText.\n", 0, "sec:one", false),
            "= One <sec:one>\nText.\n"
        );
        assert_eq!(
            with_label(
                "#figure(\n  image(\"a.png\"),\n  caption: [A],\n)\n\nMore.",
                0,
                "fig:a",
                false
            ),
            "#figure(\n  image(\"a.png\"),\n  caption: [A],\n) <fig:a>\n\nMore."
        );
        assert_eq!(
            with_label("So $ x $, where", 0, "eq:x", false),
            "So $ x $ <eq:x>, where"
        );
        // An equation that has no number gets one with its label.
        assert_eq!(
            with_label("$ x $\n", 0, "eq:x", true),
            "#math.equation(block: true, numbering: \"(1)\", $ x $) <eq:x>\n"
        );
        let call = "#math.equation(block: true, numbering: \"(1)\", $ x $)";
        assert_eq!(with_label(call, 0, "eq:x", true), format!("{call} <eq:x>"));
        // One that has a label and no number gets the number alone;
        // and where nothing is wanted, nothing is changed.
        assert_eq!(
            with_label("$ x $ <eq:x>\n", 0, "", true),
            "#math.equation(block: true, numbering: \"(1)\", $ x $) <eq:x>\n"
        );
        assert_eq!(with_label("= One\n", 0, "", true), "= One\n");
        // What was labelled is found labelled.
        let text = with_label(PAPER, 3, "tab:cells", false);
        assert_eq!(targets(&text)[3].1, some("tab:cells"));
    }

    #[test]
    fn a_section_without_a_number_is_referred_to_by_its_title() {
        assert_eq!(
            reference(Kind::Section, "sec:a", "A [b] #c", true),
            "@sec:a"
        );
        assert_eq!(
            reference(Kind::Section, "sec:a", "A [b] #c", false),
            "#link(<sec:a>)[A \\[b\\] \\#c]"
        );
        assert_eq!(reference(Kind::Figure, "fig:a", "A", false), "@fig:a");
        assert_eq!(reference(Kind::Section, "sec:a", "", false), "@sec:a");
    }

    #[test]
    fn the_label_under_the_cursor_is_known() {
        let text = "= One <one>\nSee @one and #ref(<one>), not one.\n";
        let at = |needle: &str| {
            let source = source(text);
            label_at(&source, text.find(needle).unwrap_or(usize::MAX))
                .map(|(name, range, defines)| (name, text[range].to_owned(), defines))
        };
        assert_eq!(
            at("<one>"),
            Some(("one".to_owned(), "<one>".to_owned(), true))
        );
        assert_eq!(
            at("@one"),
            Some(("one".to_owned(), "@one".to_owned(), false))
        );
        assert_eq!(
            at("one and"),
            Some(("one".to_owned(), "@one".to_owned(), false))
        );
        assert_eq!(
            at("<one>)"),
            Some(("one".to_owned(), "<one>".to_owned(), false))
        );
        assert_eq!(at("not one"), None);
    }

    #[test]
    fn what_is_wrong_with_labels_and_references_is_found() {
        let text = "= One <one>\n= Two <two>\n= Again <two>\n$ x $ <eq:x>\n\
                    See @one, @eq:y, @knuth and @elsewhere; @two.\n";
        let marks = read(&source(text));
        let elsewhere = ["knuth".to_owned(), "elsewhere".to_owned()];
        let problems = check(&marks, &elsewhere, &|_| false);
        let told: Vec<(Fault, &str, &str)> = problems
            .iter()
            .map(|problem| {
                (
                    problem.fault.clone(),
                    problem.label.as_str(),
                    &text[problem.range.clone()],
                )
            })
            .collect();
        assert_eq!(
            told,
            [
                (Fault::Missing(some("eq:x")), "eq:y", "@eq:y"),
                (Fault::Twice, "two", "<two>"),
                (Fault::Twice, "two", "<two>"),
                (Fault::Unused, "eq:x", "<eq:x>"),
            ]
        );
        assert_eq!(
            problems[0].told(),
            "@eq:y refers to nothing: there is no such label (but there is eq:x)"
        );
        // What another file refers to is used; what another file has
        // too is there twice.
        let problems = check(&marks, &["one".to_owned()], &|label| label == "eq:x");
        let faults: Vec<(&Fault, &str)> = problems
            .iter()
            .map(|problem| (&problem.fault, problem.label.as_str()))
            .collect();
        assert!(faults.contains(&(&Fault::Twice, "one")), "{faults:?}");
        assert!(!faults.contains(&(&Fault::Unused, "eq:x")), "{faults:?}");
        // A text with nothing wrong has no problems.
        assert_eq!(
            check(&read(&source("= One <one>\nSee @one.\n")), &[], &|_| false),
            []
        );
    }

    #[test]
    #[ignore = "prints how long a text takes to read; the text is the file named by MEASURE"]
    fn measure() {
        let path = std::env::var("MEASURE").unwrap_or_default();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let source = source(&text);
        let begun = std::time::Instant::now();
        let marks = read(&source);
        let read_in = begun.elapsed();
        let begun = std::time::Instant::now();
        let problems = check(&marks, &[], &|_| false);
        eprintln!(
            "{} bytes: {} things, {} labels, {} references read in {read_in:?}; \
             {} problems found in {:?}",
            text.len(),
            marks.targets.len(),
            marks.defined.len(),
            marks.used.len(),
            problems.len(),
            begun.elapsed()
        );
    }

    #[test]
    fn the_label_that_was_meant_is_guessed() {
        let labels = ["fig:plot", "sec:intro", "eq:energy", "tab:rates"];
        let guess = |name: &str| nearest(name, labels.iter().copied());
        assert_eq!(guess("fig:plto"), Some("fig:plot"));
        assert_eq!(guess("plot"), Some("fig:plot"));
        assert_eq!(guess("sec:energy"), Some("eq:energy"));
        assert_eq!(guess("Intro"), Some("sec:intro"));
        assert_eq!(guess("conclusion"), None);
        assert_eq!(distance("kitten", "sitting"), 3);
        assert_eq!(distance("", "abc"), 3);
    }
}
