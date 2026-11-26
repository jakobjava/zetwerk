//! Styles: how headings, links, code, captions and the rest of a
//! document look, as `#show` rules written and read back here.
//!
//! A `#set` rule says what a function's arguments are; how a heading of
//! the first level or a link looks takes a `#show` rule, which nobody
//! should have to write. The styles chosen in the style manager are
//! written as a block of such rules near the top of the document, one
//! rule to a line, between two comments that mark it:
//!
//! ```typst
//! // zetwerk: styles (chosen with :styles; lines it does not know are left alone)
//! #show heading.where(level: 1): set text(size: 18pt)
//! #show link: set text(fill: rgb("#1f3a68"))
//! // zetwerk: end of styles
//! ```
//!
//! Theorems, lemmas, definitions and proofs are styles too: each is a
//! `#let` line in the block that defines it (as a figure of its own
//! kind, numbered and referred to as figures are), and a `#show` line
//! says how its kind looks.
//!
//! The file is all there is: what a style is now is read from the block
//! every time, and a change is the bytes to replace and what with. A
//! line of the block that is none of the styles (written by hand, or by
//! an agent) is left as it is and told of, never rewritten.

use crate::chrome::typst_string as quoted;
use std::collections::HashSet;
use std::ops::Range;

use typst::syntax::{LinkedNode, Source, SyntaxKind};

/// The line the block of styles begins with; any line comment that
/// begins so does.
pub const BEGIN: &str = "// zetwerk: styles";
/// What the line the block begins with says, as it is written.
const BEGIN_LINE: &str =
    "// zetwerk: styles (chosen with :styles; lines it does not know are left alone)";
/// The line the block ends with.
pub const END: &str = "// zetwerk: end of styles";

/// What may be typed for a style where its choices are not all there
/// is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Typed {
    /// Nothing: one of the choices it is.
    No,
    /// Nothing either: the style is on (its rule is written) or off.
    Switch,
    /// A length; a bare number is in this unit.
    Length(&'static str),
    /// A colour: as `#1f3a68`, or one of the colours Typst has a name
    /// for.
    Colour,
    /// The name of a font's family.
    Font,
    /// The name of the family of a font made for equations.
    MathFont,
    /// Words that are set as they are typed.
    Words,
}

/// Something about how a document looks that a `#show` rule decides.
pub struct Style {
    /// What it is listed as.
    pub name: &'static str,
    /// Its rule as it is written, before and behind its value; a switch
    /// has no value, and is all of its rule.
    pub rule: (&'static str, &'static str),
    /// What it can be: as listed, and as written in the rule.
    pub choices: &'static [(&'static str, &'static str)],
    /// What else may be typed for it.
    pub typed: Typed,
    /// The style that is written along with it, as its first choice,
    /// where nothing says yet what that one is: how a theorem looks,
    /// for the theorem.
    pub with: Option<&'static str>,
}

/// A style that is not one of a heading's.
const fn style(
    name: &'static str,
    rule: (&'static str, &'static str),
    choices: &'static [(&'static str, &'static str)],
    typed: Typed,
) -> Style {
    Style {
        name,
        rule,
        choices,
        typed,
        with: None,
    }
}

/// A style that is on or off.
const fn switch(name: &'static str, rule: &'static str) -> Style {
    style(name, (rule, ""), &[], Typed::Switch)
}

/// An environment such as a theorem: a switch whose rule defines it,
/// written along with how it looks (style `with`).
const fn environment(name: &'static str, rule: &'static str, with: &'static str) -> Style {
    Style {
        with: Some(with),
        ..switch(name, rule)
    }
}

/// The fonts that come with Typst, which every document has; the
/// others there are are offered beside them.
const FONTS: &[(&str, &str)] = &[
    ("Libertinus Serif", "\"Libertinus Serif\""),
    ("New Computer Modern", "\"New Computer Modern\""),
    ("DejaVu Sans Mono", "\"DejaVu Sans Mono\""),
];

const MATH_FONTS: &[(&str, &str)] = &[("New Computer Modern Math", "\"New Computer Modern Math\"")];

const CODE_FONTS: &[(&str, &str)] = &[("DejaVu Sans Mono", "\"DejaVu Sans Mono\"")];

const HEADING_SIZES: &[(&str, &str)] = &[
    ("11pt", "11pt"),
    ("12pt", "12pt"),
    ("14pt", "14pt"),
    ("16pt", "16pt"),
    ("18pt", "18pt"),
    ("20pt", "20pt"),
    ("24pt", "24pt"),
];

const TITLE_SIZES: &[(&str, &str)] = &[
    ("16pt", "16pt"),
    ("20pt", "20pt"),
    ("24pt", "24pt"),
    ("28pt", "28pt"),
    ("32pt", "32pt"),
];

/// Sizes of what is set smaller or larger than the text around it.
const RELATIVE_SIZES: &[(&str, &str)] = &[
    ("0.8em", "0.8em"),
    ("0.9em", "0.9em"),
    ("as the text", "1em"),
    ("1.1em", "1.1em"),
];

const FOOTNOTE_SIZES: &[(&str, &str)] = &[
    ("7pt", "7pt"),
    ("8pt", "8pt"),
    ("9pt", "9pt"),
    ("10pt", "10pt"),
];

const WEIGHTS: &[(&str, &str)] = &[
    ("light", "\"light\""),
    ("regular", "\"regular\""),
    ("medium", "\"medium\""),
    ("semibold", "\"semibold\""),
    ("bold", "\"bold\""),
];

const COLOURS: &[(&str, &str)] = &[
    ("black", "black"),
    ("dark grey", "rgb(\"#444444\")"),
    ("grey", "gray"),
    ("dark blue", "rgb(\"#1f3a68\")"),
    ("blue", "rgb(\"#2a5db0\")"),
    ("dark red", "rgb(\"#8b1e1e\")"),
    ("dark green", "rgb(\"#1e5631\")"),
    ("teal", "teal"),
    ("purple", "purple"),
];

/// Colours to lay under something.
const GROUNDS: &[(&str, &str)] = &[
    ("white", "white"),
    ("light grey", "luma(242)"),
    ("cream", "rgb(\"#fbf6e9\")"),
    ("light blue", "rgb(\"#eef3fa\")"),
    ("light yellow", "rgb(\"#fff8d6\")"),
];

const GAPS: &[(&str, &str)] = &[
    ("none", "0em"),
    ("0.5em", "0.5em"),
    ("1em", "1em"),
    ("1.5em", "1.5em"),
    ("2em", "2em"),
    ("3em", "3em"),
];

/// How thick a line is.
const LINES: &[(&str, &str)] = &[
    ("thin", "0.5pt"),
    ("medium", "1pt"),
    ("thick", "2pt"),
    ("thin, grey", "0.5pt + gray"),
];

const SLANTS: &[(&str, &str)] = &[("upright", "\"normal\""), ("italic", "\"italic\"")];

const SIDES: &[(&str, &str)] = &[
    ("left", "left"),
    ("in the middle", "center"),
    ("right", "right"),
];

const INDENTS: &[(&str, &str)] = &[
    ("none", "0em"),
    ("0.5em", "0.5em"),
    ("1em", "1em"),
    ("1.5em", "1.5em"),
    ("2em", "2em"),
];

const ITEM_GAPS: &[(&str, &str)] = &[
    ("tight", "0.4em"),
    ("0.65em  as between lines", "0.65em"),
    ("1em", "1em"),
    ("1.2em  as between paragraphs", "1.2em"),
];

const TABLE_LINES: &[(&str, &str)] = &[
    ("thin", "0.5pt"),
    ("thin, grey", "0.5pt + gray"),
    ("between rows only", "(x: none, y: 0.5pt)"),
    ("none", "none"),
    ("thick", "1.5pt"),
];

const SEPARATORS: &[(&str, &str)] = &[
    (":", "[: ]"),
    (".", "[. ]"),
    ("—", "[ — ]"),
    ("–", "[ – ]"),
    ("a space", "[ ]"),
];

const HEADS: &[(&str, &str)] = &[
    (
        "title left, page number right",
        "context [#document.title #h(1fr) #counter(page).display()]",
    ),
    (
        "title left, page number right, a line under",
        "context block(width: 100%, stroke: (bottom: 0.5pt), inset: (bottom: 4pt))[#document.title #h(1fr) #counter(page).display()]",
    ),
    (
        "page number right",
        "context align(right, counter(page).display())",
    ),
    (
        "title in the middle",
        "context align(center, document.title)",
    ),
    ("nothing", "none"),
];

const FEET: &[(&str, &str)] = &[
    (
        "page number in the middle",
        "context align(center, counter(page).display())",
    ),
    (
        "page number right",
        "context align(right, counter(page).display())",
    ),
    (
        "page number of all, in the middle",
        "context align(center)[#counter(page).display() / #counter(page).final().first()]",
    ),
    (
        "a line, page number right",
        "context block(width: 100%, stroke: (top: 0.5pt), inset: (top: 4pt), align(right, counter(page).display()))",
    ),
    ("nothing", "none"),
];

/// How a theorem or a definition looks: its name and number between
/// `$open` and `$close` (as markup), then its words (`$body`), in a
/// block that is `$block` too.
macro_rules! look {
    ($block:literal, $open:literal, $close:literal, $body:literal) => {
        concat!(
            "it => block(width: 100%, above: 1.2em, below: 1.2em",
            $block,
            ")[",
            $open,
            "#it.supplement #context it.counter.display(it.numbering).",
            $close,
            " ",
            $body,
            "]"
        )
    };
}

/// How theorems, lemmas and the like can look; their words are in
/// italic, as is usual.
const THEOREM_LOOKS: &[(&str, &str)] = &[
    (
        "name in bold, words in italic",
        look!("", "*", "*", "#emph(it.body)"),
    ),
    (
        "name in bold, words upright",
        look!("", "*", "*", "#it.body"),
    ),
    (
        "name in small capitals, words in italic",
        look!("", "#smallcaps[", "]", "#emph(it.body)"),
    ),
    (
        "in a box",
        look!(
            ", inset: 0.8em, stroke: 0.5pt, radius: 3pt",
            "*",
            "*",
            "#emph(it.body)"
        ),
    ),
    (
        "on a grey ground",
        look!(
            ", inset: 0.8em, fill: luma(242), radius: 3pt",
            "*",
            "*",
            "#emph(it.body)"
        ),
    ),
];

/// How definitions, examples and remarks can look; their words are
/// upright, as is usual.
const DEFINITION_LOOKS: &[(&str, &str)] = &[
    (
        "name in bold, words upright",
        look!("", "*", "*", "#it.body"),
    ),
    (
        "name in italic, words upright",
        look!("", "_", "_", "#it.body"),
    ),
    (
        "name in bold, words in italic",
        look!("", "*", "*", "#emph(it.body)"),
    ),
    (
        "in a box",
        look!(
            ", inset: 0.8em, stroke: 0.5pt, radius: 3pt",
            "*",
            "*",
            "#it.body"
        ),
    ),
    (
        "on a grey ground",
        look!(
            ", inset: 0.8em, fill: luma(242), radius: 3pt",
            "*",
            "*",
            "#it.body"
        ),
    ),
];

/// What the theorems look like is listed as.
const THEOREMS_LOOK: &str = "environments: how theorems, lemmas, … look";
/// What the definitions look like is listed as.
const DEFINITIONS_LOOK: &str = "environments: how definitions, examples, … look";

/// A style of the headings of one level.
macro_rules! heading {
    ($level:literal, $what:literal, $rule:literal, $behind:literal, $choices:expr, $typed:expr) => {
        Style {
            name: concat!("heading ", $level, ": ", $what),
            rule: (
                concat!("#show heading.where(level: ", $level, "): ", $rule),
                $behind,
            ),
            choices: $choices,
            typed: $typed,
            with: None,
        }
    };
}

/// The styles: `before`, those of the headings of each of `levels`,
/// then `behind`.
macro_rules! styles {
    (before: [$($before:expr),* $(,)?], levels: [$($level:literal),*], behind: [$($behind:expr),* $(,)?]) => {
        &[
            $($before,)*
            $(
                heading!($level, "font", "set text(font: ", ")", FONTS, Typed::Font),
                heading!($level, "size", "set text(size: ", ")", HEADING_SIZES, Typed::Length("pt")),
                heading!($level, "weight", "set text(weight: ", ")", WEIGHTS, Typed::No),
                heading!($level, "colour", "set text(fill: ", ")", COLOURS, Typed::Colour),
                heading!($level, "space above", "set block(above: ", ")", GAPS, Typed::Length("em")),
                heading!($level, "space below", "set block(below: ", ")", GAPS, Typed::Length("em")),
                heading!($level, "small capitals", "smallcaps", "", &[], Typed::Switch),
                heading!(
                    $level,
                    "a line under",
                    "set block(stroke: (bottom: ",
                    "), inset: (bottom: 0.3em))",
                    LINES,
                    Typed::Length("pt")
                ),
            )*
            $($behind,)*
        ]
    };
}

/// All the styles, in the order they are listed and written in. A style
/// for all headings comes before those of one level, which go further.
pub const STYLES: &[Style] = styles!(
    before: [
        style("headings: font", ("#show heading: set text(font: ", ")"), FONTS, Typed::Font),
    ],
    levels: [1, 2, 3],
    behind: [
        switch(
            "heading 1: begins a new page",
            "#show heading.where(level: 1): it => pagebreak(weak: true) + it",
        ),
        style("title: font", ("#show title: set text(font: ", ")"), FONTS, Typed::Font),
        style(
            "title: size",
            ("#show title: set text(size: ", ")"),
            TITLE_SIZES,
            Typed::Length("pt"),
        ),
        style("title: weight", ("#show title: set text(weight: ", ")"), WEIGHTS, Typed::No),
        style("title: colour", ("#show title: set text(fill: ", ")"), COLOURS, Typed::Colour),
        style("title: stands", ("#show title: set align(", ")"), SIDES, Typed::No),
        style("links: colour", ("#show link: set text(fill: ", ")"), COLOURS, Typed::Colour),
        switch("links: underlined", "#show link: underline"),
        style(
            "references: colour",
            ("#show ref: set text(fill: ", ")"),
            COLOURS,
            Typed::Colour,
        ),
        style(
            "equations: font",
            ("#show math.equation: set text(font: ", ")"),
            MATH_FONTS,
            Typed::MathFont,
        ),
        style("code: font", ("#show raw: set text(font: ", ")"), CODE_FONTS, Typed::Font),
        style(
            "code: size",
            ("#show raw: set text(size: ", ")"),
            RELATIVE_SIZES,
            Typed::Length("pt"),
        ),
        style(
            "code blocks: ground",
            (
                "#show raw.where(block: true): block.with(fill: ",
                ", inset: 0.8em, radius: 3pt, width: 100%)",
            ),
            GROUNDS,
            Typed::Colour,
        ),
        style("emphasis: colour", ("#show emph: set text(fill: ", ")"), COLOURS, Typed::Colour),
        style("strong: colour", ("#show strong: set text(fill: ", ")"), COLOURS, Typed::Colour),
        switch(
            "block quotes: italic",
            "#show quote.where(block: true): set text(style: \"italic\")",
        ),
        style(
            "block quotes: size",
            ("#show quote.where(block: true): set text(size: ", ")"),
            RELATIVE_SIZES,
            Typed::Length("pt"),
        ),
        style(
            "block quotes: a line beside",
            (
                "#show quote.where(block: true): block.with(stroke: (left: ",
                "), inset: (left: 1em, y: 0.4em))",
            ),
            LINES,
            Typed::Length("pt"),
        ),
        style(
            "captions: size",
            ("#show figure.caption: set text(size: ", ")"),
            RELATIVE_SIZES,
            Typed::Length("pt"),
        ),
        style(
            "captions: slant",
            ("#show figure.caption: set text(style: ", ")"),
            SLANTS,
            Typed::No,
        ),
        style(
            "captions: after the number",
            ("#show: set figure.caption(separator: ", ")"),
            SEPARATORS,
            Typed::Words,
        ),
        switch(
            "captions: number in bold",
            "#show figure.caption: it => context [#strong[#it.supplement #it.counter.display(it.numbering)]#it.separator#it.body]",
        ),
        style(
            "tables: lines",
            ("#show: set table(stroke: ", ")"),
            TABLE_LINES,
            Typed::Length("pt"),
        ),
        style(
            "tables: first row",
            ("#show table.cell.where(y: 0): set text(weight: ", ")"),
            WEIGHTS,
            Typed::No,
        ),
        style(
            "tables: every other row shaded",
            ("#show: set table(fill: (_, y) => if calc.odd(y) { ", " })"),
            GROUNDS,
            Typed::Colour,
        ),
        style(
            "bullet lists: indent",
            ("#show: set list(indent: ", ")"),
            INDENTS,
            Typed::Length("em"),
        ),
        style(
            "bullet lists: between items",
            ("#show: set list(spacing: ", ")"),
            ITEM_GAPS,
            Typed::Length("em"),
        ),
        style(
            "numbered lists: indent",
            ("#show: set enum(indent: ", ")"),
            INDENTS,
            Typed::Length("em"),
        ),
        style(
            "numbered lists: between items",
            ("#show: set enum(spacing: ", ")"),
            ITEM_GAPS,
            Typed::Length("em"),
        ),
        style(
            "footnotes: size",
            ("#show footnote.entry: set text(size: ", ")"),
            FOOTNOTE_SIZES,
            Typed::Length("pt"),
        ),
        style("page: head", ("#show: set page(header: ", ")"), HEADS, Typed::Words),
        style("page: foot", ("#show: set page(footer: ", ")"), FEET, Typed::Words),
        style("text: colour", ("#show: set text(fill: ", ")"), COLOURS, Typed::Colour),
        style("page: ground", ("#show: set page(fill: ", ")"), GROUNDS, Typed::Colour),
        // Theorems and their kin are figures of a kind of their own: they
        // are numbered (all of a kind together) and referred to as figures
        // are, and look as the rule of their kind says.
        environment(
            "environments: theorem",
            "#let theorem = figure.with(kind: \"theorem\", supplement: [Theorem])",
            THEOREMS_LOOK,
        ),
        environment(
            "environments: lemma",
            "#let lemma = figure.with(kind: \"theorem\", supplement: [Lemma])",
            THEOREMS_LOOK,
        ),
        environment(
            "environments: corollary",
            "#let corollary = figure.with(kind: \"theorem\", supplement: [Corollary])",
            THEOREMS_LOOK,
        ),
        environment(
            "environments: proposition",
            "#let proposition = figure.with(kind: \"theorem\", supplement: [Proposition])",
            THEOREMS_LOOK,
        ),
        environment(
            "environments: definition",
            "#let definition = figure.with(kind: \"definition\", supplement: [Definition])",
            DEFINITIONS_LOOK,
        ),
        environment(
            "environments: example",
            "#let example = figure.with(kind: \"definition\", supplement: [Example])",
            DEFINITIONS_LOOK,
        ),
        environment(
            "environments: remark",
            "#let remark = figure.with(kind: \"definition\", supplement: [Remark])",
            DEFINITIONS_LOOK,
        ),
        switch(
            "environments: proof",
            "#let proof(body) = block(width: 100%)[_Proof._ #body #h(1fr) $square$]",
        ),
        style(
            THEOREMS_LOOK,
            ("#show figure.where(kind: \"theorem\"): ", ""),
            THEOREM_LOOKS,
            Typed::No,
        ),
        style(
            DEFINITIONS_LOOK,
            ("#show figure.where(kind: \"definition\"): ", ""),
            DEFINITION_LOOKS,
            Typed::No,
        ),
    ]
);

/// The colours Typst has names for.
pub const NAMED_COLOURS: &[&str] = &[
    "black", "gray", "silver", "white", "navy", "blue", "aqua", "teal", "eastern", "purple",
    "fuchsia", "maroon", "red", "orange", "yellow", "olive", "green", "lime",
];

/// What a line between the marks of the block is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Is {
    /// Nothing but blanks.
    Blank,
    /// A comment.
    Comment,
    /// Style `index` of [`STYLES`]; its value is these bytes.
    Style { index: usize, value: Range<usize> },
    /// Something else: written by hand, and left as it is.
    Unknown,
}

/// A line between the marks of the block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// Its bytes, with its line break.
    pub whole: Range<usize>,
    /// What it is.
    pub is: Is,
}

/// The block of styles of a document, as it is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// Its bytes, the lines that mark it and their line breaks included.
    pub whole: Range<usize>,
    /// The byte its last line (the mark of its end) begins at.
    pub end: usize,
    /// Its lines between the marks.
    pub lines: Vec<Line>,
}

impl Block {
    /// The lines of style `index`, in their order.
    fn of(&self, index: usize) -> impl Iterator<Item = &Line> {
        self.lines
            .iter()
            .filter(move |line| matches!(line.is, Is::Style { index: of, .. } if of == index))
    }
}

/// The byte the line `at` is in begins at.
fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |found| found + 1)
}

/// The byte behind the line break that ends the line `at` is in, or the
/// end of the text.
fn line_end(text: &str, at: usize) -> usize {
    text[at..]
        .find('\n')
        .map_or(text.len(), |found| at + found + 1)
}

/// Whether `value` is one thing as Typst writes it, and no more: what a
/// line of the block holds for a style.
fn is_one_value(value: &str) -> bool {
    let code = typst::syntax::parse_code(value);
    let things = code
        .children()
        .filter(|child| !child.kind().is_trivia())
        .count();
    things == 1 && !code.diagnosis().errors
}

/// Which style the rule `line` is (trimmed), and the bytes of its value
/// in it.
fn style_of(line: &str) -> Option<(usize, Range<usize>)> {
    STYLES.iter().enumerate().find_map(|(index, style)| {
        let (before, behind) = style.rule;
        let value = line.strip_prefix(before)?.strip_suffix(behind)?;
        let fits = match style.typed {
            Typed::Switch => value.is_empty(),
            _ => is_one_value(value),
        };
        fits.then(|| (index, before.len()..before.len() + value.len()))
    })
}

/// The block of styles of `source`, if it has one. Only comments that
/// stand by themselves at the top of the file mark it.
///
/// # Errors
///
/// Where its beginning is marked and its end is not.
pub fn block(source: &Source) -> Result<Option<Block>, String> {
    let text = source.text();
    let root = LinkedNode::new(source.root());
    let mut begun = None;
    let mut ended = None;
    // The rules (and definitions) that stand by themselves: from their
    // hash to their end.
    let mut rules = HashSet::new();
    let mut hash = None;
    for child in root.children() {
        let kind = child.kind();
        let said = text.get(child.range()).unwrap_or("").trim_end();
        if kind == SyntaxKind::LineComment && begun.is_none() && said.starts_with(BEGIN) {
            begun = Some(child.range());
        } else if kind == SyntaxKind::LineComment && begun.is_some() && said == END {
            ended = Some(child.range());
            break;
        }
        if matches!(kind, SyntaxKind::ShowRule | SyntaxKind::LetBinding)
            && let Some(hash) = hash
        {
            rules.insert((hash, child.range().end));
        }
        hash = (kind == SyntaxKind::Hash).then_some(child.offset());
    }
    let Some(begun) = begun else {
        return Ok(None);
    };
    let Some(ended) = ended else {
        return Err("the styles have lost the line that ends them".to_owned());
    };
    let end = line_start(text, ended.start);
    let mut lines = Vec::new();
    let mut at = line_end(text, begun.end);
    while at < end {
        let next = line_end(text, at).min(end);
        let written = &text[at..next];
        let trimmed = written.trim();
        let from = at + (written.len() - written.trim_start().len());
        let is = if trimmed.is_empty() {
            Is::Blank
        } else if trimmed.starts_with("//") {
            Is::Comment
        } else {
            let alone = rules.contains(&(from, from + trimmed.len()));
            match style_of(trimmed).filter(|_| alone) {
                Some((index, value)) => Is::Style {
                    index,
                    value: from + value.start..from + value.end,
                },
                None => Is::Unknown,
            }
        };
        lines.push(Line {
            whole: at..next,
            is,
        });
        at = next;
    }
    Ok(Some(Block {
        whole: line_start(text, begun.start)..line_end(text, ended.end),
        end,
        lines,
    }))
}

/// What style `index` is in `source`, as written: `None` where Typst
/// decides (no line says), the empty text for a switch that is on.
/// Where two lines say, the last is what holds.
#[must_use]
pub fn style_value(source: &Source, index: usize) -> Option<String> {
    let block = block(source).ok()??;
    let line = block.of(index).last()?;
    let Is::Style { value, .. } = &line.is else {
        return None;
    };
    source.text().get(value.clone()).map(str::to_owned)
}

/// What each style is in `source`, in the order of [`STYLES`], as
/// [`style_value`] says: the block is read once for all of them.
#[must_use]
pub fn style_values(source: &Source) -> Vec<Option<String>> {
    let mut values = vec![None; STYLES.len()];
    if let Ok(Some(block)) = block(source) {
        for line in &block.lines {
            if let Is::Style { index, value } = &line.is
                && let Some(slot) = values.get_mut(*index)
            {
                *slot = source.text().get(value.clone()).map(str::to_owned);
            }
        }
    }
    values
}

/// The lines of the block of `source` that are none of the styles: left
/// as they are, and to be told of.
#[must_use]
pub fn not_understood(source: &Source) -> Vec<String> {
    let Ok(Some(block)) = block(source) else {
        return Vec::new();
    };
    block
        .lines
        .iter()
        .filter(|line| line.is == Is::Unknown)
        .filter_map(|line| source.text().get(line.whole.clone()))
        .map(|line| line.trim().to_owned())
        .collect()
}

/// A change: the bytes to replace and what with. Where there are more,
/// each is of the text the ones before it left: they go from the back.
pub type Edits = Vec<(Range<usize>, String)>;

/// Where a block of styles goes in `source` that has none: behind the
/// rules, imports, definitions and comments it begins with, on a line
/// of its own. The byte, and whether a line break must come first.
fn block_place(source: &Source) -> (usize, bool) {
    let text = source.text();
    let mut behind = 0;
    for child in LinkedNode::new(source.root()).children() {
        match child.kind() {
            SyntaxKind::SetRule
            | SyntaxKind::ShowRule
            | SyntaxKind::LetBinding
            | SyntaxKind::ModuleImport
            | SyntaxKind::LineComment
            | SyntaxKind::BlockComment => behind = child.range().end,
            SyntaxKind::Hash | SyntaxKind::Space | SyntaxKind::Parbreak | SyntaxKind::Semicolon => {
            }
            _ => break,
        }
    }
    if behind == 0 {
        return (0, false);
    }
    let rest = &text[behind..];
    let blanks = rest.len() - rest.trim_start_matches([' ', '\t', '\r']).len();
    if rest[blanks..].starts_with('\n') {
        (behind + blanks + 1, false)
    } else {
        (behind, true)
    }
}

/// The change to `source` that makes style `index` be `value` (as
/// written in its rule; nothing for a switch, which this turns on): its
/// line is changed, or a new one is written where its place in the
/// block is, or the block is written with it.
///
/// # Errors
///
/// Where there is no such style, or the block cannot be read.
pub fn set_style(source: &Source, index: usize, value: &str) -> Result<Edits, String> {
    let style = STYLES.get(index).ok_or("there is no such style")?;
    let with = style.with.and_then(|name| {
        let at = STYLES.iter().position(|style| style.name == name)?;
        let first = STYLES[at].choices.first()?.1;
        style_value(source, at).is_none().then_some((at, first))
    });
    let edits = set_one(source, style, index, value)?;
    let Some((at, first)) = with.filter(|_| !edits.is_empty()) else {
        return Ok(edits);
    };
    // Both, as one change: what is between the bytes that stay.
    let mut text = source.text().to_owned();
    apply(&mut text, &edits);
    let more = set_one(&Source::detached(text.clone()), &STYLES[at], at, first)?;
    apply(&mut text, &more);
    Ok(vec![between(source.text(), &text)])
}

/// `text` after `edits`.
fn apply(text: &mut String, edits: &Edits) {
    for (range, with) in edits {
        text.replace_range(range.clone(), with);
    }
}

/// The one change that makes `old` be `new`: the bytes between what both
/// begin and end with.
fn between(old: &str, new: &str) -> (Range<usize>, String) {
    let mut front = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(front) || !new.is_char_boundary(front) {
        front -= 1;
    }
    let most = old.len().min(new.len()) - front;
    let mut back = old
        .bytes()
        .rev()
        .zip(new.bytes().rev())
        .take(most)
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(old.len() - back) || !new.is_char_boundary(new.len() - back) {
        back -= 1;
    }
    (
        front..old.len() - back,
        new[front..new.len() - back].to_owned(),
    )
}

/// [`set_style`], of `style` (`index`) alone.
fn set_one(source: &Source, style: &Style, index: usize, value: &str) -> Result<Edits, String> {
    let line = format!("{}{value}{}", style.rule.0, style.rule.1);
    let Some(block) = block(source)? else {
        let (at, broken) = block_place(source);
        let first = if broken { "\n" } else { "" };
        return Ok(vec![(
            at..at,
            format!("{first}{BEGIN_LINE}\n{line}\n{END}\n"),
        )]);
    };
    if let Some(Line {
        is: Is::Style { value: old, .. },
        ..
    }) = block.of(index).last()
    {
        let same = source.text().get(old.clone()) == Some(value);
        return Ok(if same {
            Vec::new()
        } else {
            vec![(old.clone(), value.to_owned())]
        });
    }
    // Before the first of the styles that come behind it.
    let behind = block.lines.iter().find(|line| match line.is {
        Is::Style { index: of, .. } => of > index,
        _ => false,
    });
    let at = behind.map_or(block.end, |line| line.whole.start);
    Ok(vec![(at..at, format!("{line}\n"))])
}

/// The change to `source` that leaves style `index` to Typst again: its
/// lines are taken out, and the block too where nothing else is left in
/// it.
///
/// # Errors
///
/// Where the block cannot be read.
pub fn unset_style(source: &Source, index: usize) -> Result<Edits, String> {
    let Some(block) = block(source)? else {
        return Ok(Vec::new());
    };
    let gone: Vec<&Line> = block.of(index).collect();
    if gone.is_empty() {
        return Ok(Vec::new());
    }
    let left = block
        .lines
        .iter()
        .any(|line| line.is != Is::Blank && !gone.contains(&line));
    if !left {
        return Ok(vec![(block.whole, String::new())]);
    }
    Ok(gone
        .into_iter()
        .rev()
        .map(|line| (line.whole.clone(), String::new()))
        .collect())
}

/// `typed` as a colour Typst knows: a name it has, or six (or three, or
/// eight) hexadecimal digits.
fn colour(typed: &str) -> Option<String> {
    let lower = typed.to_lowercase();
    let lower = if lower == "grey" {
        "gray".to_owned()
    } else {
        lower
    };
    if NAMED_COLOURS.contains(&lower.as_str()) {
        return Some(lower);
    }
    let digits = lower.strip_prefix('#').unwrap_or(&lower);
    let hex = digits.chars().all(|c| c.is_ascii_hexdigit());
    (hex && matches!(digits.len(), 3 | 6 | 8)).then(|| format!("rgb(\"#{digits}\")"))
}

/// What was typed for a style of kind `typed`, as it is written in its
/// rule; `None` if it is nothing the style can be.
#[must_use]
pub fn typed_value(typed: Typed, text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    match typed {
        Typed::No | Typed::Switch => None,
        Typed::Length(unit) => {
            let number = text.trim_end_matches(|c: char| c.is_ascii_alphabetic());
            let own = &text[number.len()..];
            let fine = number.parse::<f64>().is_ok_and(|number| number >= 0.0);
            let known = matches!(own, "" | "pt" | "mm" | "cm" | "in" | "em");
            (fine && known).then(|| format!("{number}{}", if own.is_empty() { unit } else { own }))
        }
        Typed::Colour => colour(text),
        Typed::Font | Typed::MathFont => Some(quoted(text)),
        // Between brackets, with all that would be markup made plain.
        Typed::Words => {
            let mut written = String::from("[");
            for c in text.chars() {
                if "\\[]#$*_@<>`~/-+=".contains(c) {
                    written.push('\\');
                }
                written.push(c);
            }
            written.push(']');
            Some(written)
        }
    }
}

/// What a value written in the rule of `style` is listed as: the choice
/// it is, or else as plainly as it can be said.
#[must_use]
pub fn listed(style: &Style, written: &str) -> String {
    if style.typed == Typed::Switch {
        return "on".to_owned();
    }
    if let Some((label, _)) = style.choices.iter().find(|(_, value)| *value == written) {
        return (*label).to_owned();
    }
    if let Some(hex) = written
        .strip_prefix("rgb(\"")
        .and_then(|rest| rest.strip_suffix("\")"))
    {
        return hex.to_owned();
    }
    if let Some(inside) = written
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        return inside.replace("\\\"", "\"").replace("\\\\", "\\");
    }
    if let Some(inside) = written
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    {
        let mut plain = String::new();
        let mut escaped = false;
        for c in inside.chars() {
            if c == '\\' && !escaped {
                escaped = true;
                continue;
            }
            escaped = false;
            plain.push(c);
        }
        return plain;
    }
    let simple = written
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '.' | ' ' | '+' | '%'));
    if simple {
        written.to_owned()
    } else {
        "set by hand".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, OnceLock};

    use zetwerk_typst::{Environment, FontSet, Project};

    use super::*;

    fn source(text: &str) -> Source {
        Source::detached(text)
    }

    /// `text` after `edits`.
    fn edited(text: &str, edits: &Edits) -> String {
        let mut text = text.to_owned();
        for (range, with) in edits {
            text.replace_range(range.clone(), with);
        }
        text
    }

    /// `text` with style `name` made `value`.
    fn set(text: &str, name: &str, value: &str) -> String {
        let edits = set_style(&source(text), index(name), value)
            .unwrap_or_else(|why| panic!("{name}: {why}"));
        edited(text, &edits)
    }

    /// `text` with style `name` left to Typst.
    fn unset(text: &str, name: &str) -> String {
        let edits = unset_style(&source(text), index(name)).unwrap_or_else(|why| panic!("{why}"));
        edited(text, &edits)
    }

    fn index(name: &str) -> usize {
        STYLES
            .iter()
            .position(|style| style.name == name)
            .unwrap_or_else(|| panic!("no style {name}"))
    }

    fn value(text: &str, name: &str) -> Option<String> {
        style_value(&source(text), index(name))
    }

    #[test]
    fn a_style_is_written_in_a_block_of_its_own_behind_the_rules() {
        let text = "#set text(size: 12pt)\n#set page(paper: \"a5\")\n\n= Hello\n";
        let written = set(text, "heading 1: size", "18pt");
        assert_eq!(
            written,
            format!(
                "#set text(size: 12pt)\n#set page(paper: \"a5\")\n{BEGIN_LINE}\n\
                 #show heading.where(level: 1): set text(size: 18pt)\n{END}\n\n= Hello\n"
            )
        );
        assert_eq!(value(&written, "heading 1: size").as_deref(), Some("18pt"));
        assert_eq!(value(&written, "heading 2: size"), None);
        // Without rules, at the very top.
        let written = set("Words.\n", "links: underlined", "");
        assert_eq!(
            written,
            format!("{BEGIN_LINE}\n#show link: underline\n{END}\nWords.\n")
        );
        assert_eq!(value(&written, "links: underlined").as_deref(), Some(""));
        // Behind a rule that ends the file.
        let written = set("#set par(justify: true)", "text: colour", "gray");
        assert_eq!(
            written,
            format!("#set par(justify: true)\n{BEGIN_LINE}\n#show: set text(fill: gray)\n{END}\n")
        );
    }

    #[test]
    fn a_style_is_changed_where_it_is_and_others_go_in_their_place() {
        let text = set("Words.\n", "links: colour", "teal");
        let changed = set(&text, "links: colour", "purple");
        assert_eq!(changed, text.replace("teal", "purple"));
        // The same again changes nothing.
        let edits = set_style(&source(&changed), index("links: colour"), "purple");
        assert_eq!(edits, Ok(Vec::new()));
        // A heading's goes before, the page's behind: in the order they
        // are listed in.
        let more = set(&changed, "heading 2: weight", "\"bold\"");
        let more = set(&more, "page: ground", "white");
        let lines: Vec<&str> = more.lines().collect();
        assert_eq!(
            lines,
            [
                BEGIN_LINE,
                "#show heading.where(level: 2): set text(weight: \"bold\")",
                "#show link: set text(fill: purple)",
                "#show: set page(fill: white)",
                END,
                "Words.",
            ]
        );
    }

    #[test]
    fn a_style_left_to_typst_is_gone_and_the_last_takes_the_block_along() {
        let text = "#set text(size: 12pt)\n\nWords.\n";
        let one = set(text, "links: colour", "teal");
        let two = set(&one, "code: size", "0.9em");
        assert_eq!(unset(&two, "code: size"), one);
        assert_eq!(unset(&one, "links: colour"), text);
        // What is not there is not taken out.
        assert_eq!(unset(text, "links: colour"), text);
        assert_eq!(unset(&one, "code: size"), one);
    }

    #[test]
    fn every_style_and_choice_is_read_back_as_written() {
        for (at, style) in STYLES.iter().enumerate() {
            let values: Vec<&str> = match style.typed {
                Typed::Switch => vec![""],
                _ => style.choices.iter().map(|(_, value)| *value).collect(),
            };
            assert!(!values.is_empty(), "{} can be nothing", style.name);
            for written in values {
                let text = set("Words.\n", style.name, written);
                let found = block(&source(&text))
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| panic!("{}: no block", style.name));
                let is: Vec<&Is> = found.lines.iter().map(|line| &line.is).collect();
                // With the style that is written along with it, if any.
                let with = style.with.map(index);
                let read = match (&is[..], with) {
                    ([Is::Style { index, .. }], None) => *index == at,
                    ([Is::Style { index: a, .. }, Is::Style { index: b, .. }], Some(with)) => {
                        *a == at && *b == with
                    }
                    _ => false,
                };
                assert!(read, "{}: {written} is read as {is:?}", style.name);
                assert_eq!(value(&text, style.name).as_deref(), Some(written));
                let alone = with.map_or_else(
                    || "Words.\n".to_owned(),
                    |with| set("Words.\n", STYLES[with].name, STYLES[with].choices[0].1),
                );
                assert_eq!(unset(&text, style.name), alone);
                if style.typed != Typed::Switch {
                    let label = style.choices.iter().find(|(_, value)| *value == written);
                    assert_eq!(
                        Some(listed(style, written).as_str()),
                        label.map(|(l, _)| *l)
                    );
                }
            }
        }
    }

    #[test]
    fn what_is_written_by_hand_in_the_block_is_left_alone_and_told_of() {
        let text = format!(
            "#set text(size: 12pt)\n{BEGIN}, by hand\n\
             #show link: set text(fill: red)\n\
             // my own\n\
             #show heading: set text(size: 20pt, fill: red)\n\
             \n\
             #show heading.where(level: 1): set text(size: 30pt)\n\
             #show heading.where(level: 1): set text(size: 31pt)\n\
             {END}\nWords.\n"
        );
        // Understood: the link's colour, and the last of two sizes.
        assert_eq!(value(&text, "links: colour").as_deref(), Some("red"));
        assert_eq!(value(&text, "heading 1: size").as_deref(), Some("31pt"));
        assert_eq!(
            not_understood(&source(&text)),
            ["#show heading: set text(size: 20pt, fill: red)"]
        );
        // A change leaves the rest as it was written.
        let changed = set(&text, "heading 1: size", "18pt");
        assert_eq!(changed, text.replace("31pt", "18pt"));
        let added = set(&text, "page: ground", "white");
        assert_eq!(
            added,
            text.replace(
                &format!("{END}\n"),
                &format!("#show: set page(fill: white)\n{END}\n")
            )
        );
        // Taken out: all that says it; the block stays for the rest.
        let gone = unset(&text, "heading 1: size");
        assert!(!gone.contains("30pt") && !gone.contains("31pt"));
        assert!(gone.contains("20pt, fill: red") && gone.contains("// my own"));
        let gone = unset(&gone, "links: colour");
        assert!(gone.contains(BEGIN) && gone.contains(END));
        // A block that has lost its end is not written into.
        let broken = format!("{BEGIN_LINE}\n#show link: underline\nWords.\n");
        assert!(set_style(&source(&broken), index("links: colour"), "red").is_err());
        assert_eq!(not_understood(&source(&broken)), Vec::<String>::new());
    }

    #[test]
    fn an_environment_is_written_with_how_it_looks_unless_that_is_said() {
        let text = "#set text(size: 12pt)\n\nWords.\n";
        let looks = "environments: how theorems, lemmas, … look";
        // The first: how theorems look comes along, as one change.
        let edits = set_style(&source(text), index("environments: lemma"), "")
            .unwrap_or_else(|why| panic!("{why}"));
        assert_eq!(edits.len(), 1);
        let one = edited(text, &edits);
        assert_eq!(value(&one, "environments: lemma").as_deref(), Some(""));
        assert_eq!(
            value(&one, looks).as_deref(),
            Some(THEOREM_LOOKS[0].1),
            "{one}"
        );
        // How they look is said now: it is not written again, nor changed.
        let boxed = set(&one, looks, THEOREM_LOOKS[3].1);
        let two = set(&boxed, "environments: theorem", "");
        assert_eq!(value(&two, looks).as_deref(), Some(THEOREM_LOOKS[3].1));
        assert_eq!(two.matches("#show figure").count(), 1);
        // The definitions have a look of their own; the proof has none.
        let three = set(&two, "environments: definition", "");
        assert_eq!(
            value(&three, "environments: how definitions, examples, … look").as_deref(),
            Some(DEFINITION_LOOKS[0].1)
        );
        let four = set(&three, "environments: proof", "");
        assert_eq!(four.matches("#show figure").count(), 2);
        // Definitions are read back as styles, not as lines by hand.
        assert_eq!(not_understood(&source(&four)), Vec::<String>::new());
        // A definition written by hand among them is left alone.
        let by_hand = four.replace(END, &format!("#let lemma = [mine]\n{END}"));
        assert_eq!(not_understood(&source(&by_hand)), ["#let lemma = [mine]"]);
    }

    /// The text set in `frame`, in its order.
    fn said_on(frame: &typst::layout::Frame) -> String {
        let mut said = String::new();
        for (_, item) in frame.items() {
            match item {
                typst::layout::FrameItem::Group(group) => said += &said_on(&group.frame),
                typst::layout::FrameItem::Text(text) => said += text.text.as_str(),
                _ => {}
            }
        }
        said
    }

    #[test]
    fn theorems_are_numbered_and_referred_to_in_every_look() {
        let mut text = "Words.\n".to_owned();
        for name in [
            "environments: theorem",
            "environments: lemma",
            "environments: corollary",
            "environments: proposition",
            "environments: definition",
            "environments: example",
            "environments: remark",
            "environments: proof",
        ] {
            text = set(&text, name, "");
        }
        let used = "#theorem[All is well.] <t>\n\
            #lemma[Some is.]\n\
            #corollary[Most is.]\n\
            #proposition[One is.]\n\
            #definition[A word.] <d>\n\
            #example[An instance.]\n\
            #remark[By the way.]\n\
            #proof[As @t and @d say.]\n";
        let pairs = [
            ("environments: how theorems, lemmas, … look", THEOREM_LOOKS),
            (
                "environments: how definitions, examples, … look",
                DEFINITION_LOOKS,
            ),
        ];
        for (looks, choices) in pairs {
            for (label, written) in choices {
                let text = format!("{}{used}", set(&text, looks, written));
                let (said, document) = compiled(&text);
                assert!(
                    said.is_empty() && document.is_some(),
                    "{label}: {said:?}\n{text}"
                );
                let document = document.unwrap_or_else(|| panic!("compiles"));
                let words = said_on(&document.pages()[0].frame);
                // Numbered, all theorems together: the proposition is the
                // fourth; the remark the third definition.
                for told in ["Theorem 1", "Proposition 4", "Remark 3", "Proof"] {
                    assert!(words.contains(told), "{label}: no {told}");
                }
            }
        }
    }

    #[test]
    fn rules_within_something_else_are_not_styles() {
        // In a block of code, a line that looks like a style is none.
        let text = format!("{BEGIN_LINE}\n#{{\n#show link: underline\n}}\n{END}\nWords.\n");
        assert_eq!(value(&text, "links: underlined"), None);
        assert_eq!(not_understood(&source(&text)).len(), 3);
        // A comment that only says the same deep down marks nothing.
        let text = format!("#[\n{BEGIN_LINE}\n#show link: underline\n{END}\n]\n");
        assert_eq!(block(&source(&text)), Ok(None));
    }

    #[test]
    fn what_is_typed_for_a_style_is_written_as_typst_wants_it() {
        assert_eq!(
            typed_value(Typed::Length("pt"), "13").as_deref(),
            Some("13pt")
        );
        assert_eq!(
            typed_value(Typed::Length("pt"), "1.2em").as_deref(),
            Some("1.2em")
        );
        assert_eq!(typed_value(Typed::Length("pt"), "50%"), None);
        assert_eq!(typed_value(Typed::Length("pt"), "big"), None);
        assert_eq!(
            typed_value(Typed::Colour, "#1F3A68").as_deref(),
            Some("rgb(\"#1f3a68\")")
        );
        assert_eq!(
            typed_value(Typed::Colour, "abc").as_deref(),
            Some("rgb(\"#abc\")")
        );
        assert_eq!(typed_value(Typed::Colour, "Grey").as_deref(), Some("gray"));
        assert_eq!(typed_value(Typed::Colour, "sky"), None);
        assert_eq!(
            typed_value(Typed::Font, "Noto \"Serif\"").as_deref(),
            Some("\"Noto \\\"Serif\\\"\"")
        );
        assert_eq!(
            typed_value(Typed::Words, "Draft #1").as_deref(),
            Some("[Draft \\#1]")
        );
        assert_eq!(typed_value(Typed::No, "bold"), None);
        assert_eq!(typed_value(Typed::Switch, "on"), None);
        // And listed as it was typed.
        let colour = &STYLES[index("links: colour")];
        assert_eq!(listed(colour, "rgb(\"#1f3a68\")"), "dark blue");
        assert_eq!(listed(colour, "rgb(\"#abcdef\")"), "#abcdef");
        assert_eq!(listed(colour, "red"), "red");
        assert_eq!(listed(colour, "red.darken(20%)"), "set by hand");
        let head = &STYLES[index("page: head")];
        let words = typed_value(Typed::Words, "Draft #1 [x]").unwrap_or_default();
        assert_eq!(listed(head, &words), "Draft #1 [x]");
        let font = &STYLES[index("code: font")];
        assert_eq!(listed(font, "\"Fira Code\""), "Fira Code");
    }

    /// A document with something of everything that is styled.
    const SAMPLE: &str = "#set document(title: [A Title])\n\
        #set page(paper: \"a6\")\n\
        #title()\n\
        = Chapter\n\
        Some _emphasis_ and *strong* words, #link(\"https://typst.app\")[a link], \
        a reference to @fig and a note#footnote[A note.].\n\
        == Section\n\
        === Part\n\
        $ x^2 + y^2 = z^2 $\n\
        Code `inline` and\n\
        ```rust\nfn main() {}\n```\n\
        #quote(block: true)[Said once.]\n\
        #figure(table(columns: 2, [A], [B], [1], [2], [3], [4]), caption: [Numbers.]) <fig>\n\
        - one\n- two\n\
        + first\n+ second\n\
        #pagebreak()\n\
        = Another\n";

    /// Typst, with the fonts it comes with, for all the tests here.
    fn environment() -> Arc<Environment> {
        static ENVIRONMENT: OnceLock<Arc<Environment>> = OnceLock::new();
        Arc::clone(ENVIRONMENT.get_or_init(|| Environment::new(FontSet::Embedded)))
    }

    /// What is wrong with `text` as Typst compiles it: its errors and
    /// warnings. With them, its pages if it has any.
    fn compiled(text: &str) -> (Vec<String>, Option<typst_layout::PagedDocument>) {
        let project = Project::in_memory(
            environment(),
            PathBuf::from("/"),
            "main.typ",
            text.to_owned(),
        )
        .unwrap_or_else(|| panic!("a project"));
        let compiled = zetwerk_typst::compile(&project.snapshot());
        let said = compiled
            .errors
            .iter()
            .chain(&compiled.warnings)
            .map(|said| said.message.to_string())
            .collect();
        (said, compiled.document)
    }

    #[test]
    fn the_document_compiles_with_every_style_as_offered() {
        let (said, _) = compiled(SAMPLE);
        assert!(said.is_empty(), "the sample itself: {said:?}");
        let mut wrong = Vec::new();
        for style in STYLES {
            let values: Vec<String> = match style.typed {
                Typed::Switch => vec![String::new()],
                typed => {
                    let mut values: Vec<String> = style
                        .choices
                        .iter()
                        .map(|(_, value)| (*value).to_owned())
                        .collect();
                    // And something typed.
                    let typed = match typed {
                        Typed::Length(_) => typed_value(typed, "3"),
                        Typed::Colour => typed_value(typed, "#2a5db0"),
                        Typed::Font => typed_value(typed, "DejaVu Sans Mono"),
                        Typed::MathFont => typed_value(typed, "New Computer Modern Math"),
                        Typed::Words => {
                            typed_value(typed, "Draft #1 $5 *x* _y_ @z <w> `v` ~ // - + =")
                        }
                        Typed::No | Typed::Switch => None,
                    };
                    values.extend(typed);
                    values
                }
            };
            for written in values {
                let text = set(SAMPLE, style.name, &written);
                let (said, document) = compiled(&text);
                if !said.is_empty() || document.is_none() {
                    wrong.push(format!("{} as {written}: {said:?}", style.name));
                }
            }
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    #[test]
    fn styles_show_on_the_page() {
        // The ground of the page, for one.
        let (_, plain) = compiled(SAMPLE);
        let plain = plain.unwrap_or_else(|| panic!("compiles"));
        let text = set(SAMPLE, "page: ground", "rgb(\"#fbf6e9\")");
        let (_, document) = compiled(&text);
        let document = document.unwrap_or_else(|| panic!("compiles"));
        let fill =
            |document: &typst_layout::PagedDocument| format!("{:?}", document.pages()[0].fill);
        assert_ne!(fill(&document), fill(&plain));
        // A new page for each chapter: the sample's two become three.
        let (_, before) = compiled(SAMPLE);
        let pages = |document: Option<typst_layout::PagedDocument>| {
            document.map_or(0, |document| document.pages().len())
        };
        let before = pages(before);
        let text = set(SAMPLE, "heading 1: begins a new page", "");
        assert!(pages(compiled(&text).1) > before);
    }
}
