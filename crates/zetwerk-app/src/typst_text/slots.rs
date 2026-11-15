//! What the cursor is in where that is not running text, and what an
//! empty place is for.
//!
//! On a page one sees what is typed, not where it goes: into the
//! equation or behind it, into the caption or the text under the figure.
//! And a part of an equation that is yet to be typed shows as a box that
//! does not say which part it is. The syntax tree says both.

use std::ops::Range;

use typst::syntax::{LinkedNode, Side, Source, SyntaxKind};

use super::{Ground, containing, is_empty_spot, rule_value};

/// What typing goes into, where it is not running text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enclosure {
    Equation,
    /// A cell of a table or a grid.
    Cell,
    /// The caption of a figure.
    Caption,
    /// The text of a footnote.
    Footnote,
    /// What another call holds between square brackets.
    Block,
    /// An item of a list, of a numbered list or of a list of terms.
    Item,
}

/// What typing at a place goes into, and its bytes: of an equation with
/// its dollar signs, of what is between square brackets without them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Around {
    pub kind: Enclosure,
    pub range: Range<usize>,
}

/// The calls whose square brackets hold a cell.
const CELLS: &[&str] = &[
    "table",
    "grid",
    "table.cell",
    "grid.cell",
    "table.header",
    "grid.header",
    "table.footer",
    "grid.footer",
];

/// What is between the square brackets `block`, a content block of
/// `chain` (from the outside in, `block` at `depth`), is part of: a
/// cell, a caption, a footnote, another call. `None` where it is the
/// argument of no call.
fn block_kind(chain: &[LinkedNode], depth: usize, text: &str) -> Option<Enclosure> {
    let parent = chain.get(depth.checked_sub(1)?)?;
    let name_of = |call: &LinkedNode| {
        let callee = call.children().next()?;
        text.get(callee.range())
    };
    let (call, key) = match parent.kind() {
        SyntaxKind::Args => (chain.get(depth.checked_sub(2)?)?, None),
        SyntaxKind::Named => {
            let key = parent
                .children()
                .next()
                .and_then(|key| text.get(key.range()));
            (chain.get(depth.checked_sub(3)?)?, key)
        }
        _ => return None,
    };
    if call.kind() != SyntaxKind::FuncCall {
        return None;
    }
    let name = name_of(call)?;
    Some(match (name, key) {
        (_, Some("caption")) => Enclosure::Caption,
        ("footnote", None) => Enclosure::Footnote,
        (name, None) if CELLS.contains(&name) => Enclosure::Cell,
        _ => Enclosure::Block,
    })
}

/// Whether the square brackets `block` hold more than one paragraph:
/// then what is in them is running text, as much as any.
fn holds_paragraphs(block: &LinkedNode) -> bool {
    block
        .children()
        .filter(|child| child.kind() == SyntaxKind::Markup)
        .any(|markup| {
            markup
                .children()
                .any(|part| part.kind() == SyntaxKind::Parbreak)
        })
}

/// The innermost item of a list that byte `at` of `source` is in or at
/// the end of: what is typed at the end of an item goes on it.
fn item_at(source: &Source, at: usize) -> Option<Range<usize>> {
    // Blanks at the end of an item are not its own: what is typed
    // behind them goes on it all the same.
    let at = source.text().get(..at)?.trim_end_matches(' ').len();
    let mut node = LinkedNode::new(source.root()).leaf_at(at, Side::Before)?;
    loop {
        let item = matches!(
            node.kind(),
            SyntaxKind::ListItem | SyntaxKind::EnumItem | SyntaxKind::TermItem
        );
        if item && node.offset() < at {
            return Some(node.range());
        }
        node = node.parent()?.clone();
    }
}

/// What typing at byte `at` of `source` goes into, where that is not
/// running text: the equation, the cell of a table, the caption, the
/// text of a footnote, what a call holds, an item of a list. The
/// innermost. `None` in running text, in brackets that hold paragraphs,
/// and where what is typed is not shown at all (code, a name, a file
/// name).
#[must_use]
pub fn enclosing(source: &Source, at: usize) -> Option<Around> {
    let text = source.text();
    let (chain, ground) = containing(source, at);
    let in_equation = || chain.iter().any(|node| node.kind() == SyntaxKind::Equation);
    match ground {
        Ground::Markup | Ground::Math => {}
        Ground::Str if in_equation() => {}
        _ => return None,
    }
    // An item ends where its last letter does, so the cursor behind that
    // is in no item by `containing`; it is in the item here, and that
    // is inside what holds all of it.
    let item = item_at(source, at).map(|range| Around {
        kind: Enclosure::Item,
        range,
    });
    let inner = |node: &LinkedNode| {
        item.clone().filter(|item| {
            let range = node.range();
            !(item.range.start <= range.start && range.end <= item.range.end)
        })
    };
    for (depth, node) in chain.iter().enumerate().rev() {
        match node.kind() {
            SyntaxKind::Equation => {
                return inner(node).or(Some(Around {
                    kind: Enclosure::Equation,
                    range: node.range(),
                }));
            }
            SyntaxKind::ContentBlock => {
                let Some(kind) = block_kind(&chain, depth, text) else {
                    continue;
                };
                if let Some(item) = inner(node) {
                    return Some(item);
                }
                if holds_paragraphs(node) {
                    return None;
                }
                let range = node.range();
                return Some(Around {
                    kind,
                    range: range.start + 1..range.end.saturating_sub(1).max(range.start + 1),
                });
            }
            _ => {}
        }
    }
    item
}

/// What the arguments of the functions of equations are for, by their
/// order; with whether the last stands for all that follow it.
const ARGUMENTS: &[(&str, &[&str], bool)] = &[
    ("frac", &["numerator", "denominator"], false),
    ("binom", &["upper", "lower"], true),
    ("sqrt", &["radicand"], false),
    ("root", &["index", "radicand"], false),
    ("abs", &["value"], false),
    ("norm", &["value"], false),
    ("floor", &["value"], false),
    ("ceil", &["value"], false),
    ("round", &["value"], false),
    ("hat", &["base"], false),
    ("tilde", &["base"], false),
    ("dot", &["base"], false),
    ("ddot", &["base"], false),
    ("dddot", &["base"], false),
    ("arrow", &["base"], false),
    ("bar", &["base"], false),
    ("breve", &["base"], false),
    ("acute", &["base"], false),
    ("grave", &["base"], false),
    ("macron", &["base"], false),
    ("circle", &["base"], false),
    ("caron", &["base"], false),
    ("overline", &["body"], false),
    ("underline", &["body"], false),
    ("cancel", &["body"], false),
    ("overbrace", &["body", "annotation"], false),
    ("underbrace", &["body", "annotation"], false),
    ("overbracket", &["body", "annotation"], false),
    ("underbracket", &["body", "annotation"], false),
    ("overparen", &["body", "annotation"], false),
    ("underparen", &["body", "annotation"], false),
    ("accent", &["base", "accent"], false),
    ("attach", &["base"], false),
    ("bold", &["body"], false),
    ("italic", &["body"], false),
    ("upright", &["body"], false),
    ("bb", &["body"], false),
    ("cal", &["body"], false),
    ("frak", &["body"], false),
    ("mono", &["body"], false),
    ("sans", &["body"], false),
    ("serif", &["body"], false),
    ("op", &["text"], false),
    ("lr", &["body"], false),
    ("vec", &["entry"], true),
    ("mat", &["entry"], true),
    ("cases", &["case"], true),
];

/// What a sum, a product, an integral or a limit has below and above it
/// is not lowered or raised: it is where it runs from and to.
const LIMITS: &[&str] = &[
    "sum",
    "product",
    "prod",
    "integral",
    "int",
    "lim",
    "limsup",
    "liminf",
    "union",
    "sect",
    "max",
    "min",
    "sup",
    "inf",
    "coproduct",
    "integral.double",
    "integral.cont",
];

/// What the empty place behind the mark `mark` (`^`, `_` or `/`) of an
/// equation is for.
fn behind_mark(mark: &LinkedNode, text: &str) -> Option<&'static str> {
    let base = mark
        .parent()
        .filter(|parent| parent.kind() == SyntaxKind::MathAttach)
        .and_then(|attach| attach.children().next())
        .and_then(|base| text.get(base.range()));
    let limits = base.is_some_and(|base| LIMITS.contains(&base));
    match (mark.kind(), limits) {
        (SyntaxKind::Hat, false) => Some("exponent"),
        (SyntaxKind::Hat, true) => Some("upper limit"),
        (SyntaxKind::Underscore, false) => Some("subscript"),
        (SyntaxKind::Underscore, true) => Some("lower limit"),
        (SyntaxKind::Slash, _) => Some("denominator"),
        _ => None,
    }
}

/// What the empty argument behind `mark` (the bracket that opens the
/// arguments, or a comma between two) is for.
fn argument(mark: &LinkedNode, text: &str) -> Option<&'static str> {
    let args = mark.parent()?;
    let callee = args.parent()?.children().next()?;
    let name = text.get(callee.range())?;
    let (_, names, rest) = ARGUMENTS
        .iter()
        .find(|(function, ..)| *function == name)
        .or_else(|| {
            // `arrow.r` is an arrow as `arrow` is.
            let (first, _) = name.split_once('.')?;
            ARGUMENTS.iter().find(|(function, ..)| *function == first)
        })?;
    let index = args
        .children()
        .take_while(|part| part.offset() < mark.range().end)
        .filter(|part| matches!(part.kind(), SyntaxKind::Comma | SyntaxKind::Semicolon))
        .count();
    names
        .get(index)
        .or_else(|| rest.then(|| names.last()).flatten())
        .copied()
}

/// The sibling of `node` before it (`forward` false) or behind it that
/// is not a blank.
fn beside<'a>(node: &LinkedNode<'a>, forward: bool) -> Option<LinkedNode<'a>> {
    let parent = node.parent()?;
    let index = node.index();
    let mut siblings: Vec<LinkedNode<'a>> = parent.children().collect();
    if forward {
        siblings.drain(..=index);
    } else {
        siblings.truncate(index);
        siblings.reverse();
    }
    siblings
        .into_iter()
        .find(|sibling| sibling.kind() != SyntaxKind::Space && !sibling.is_empty())
}

/// The leaf of `node` that ends at byte `at` and is not empty: the mark
/// a place stands behind.
fn leaf_ending_at(node: LinkedNode<'_>, at: usize) -> Option<LinkedNode<'_>> {
    if node.get().children().len() == 0 {
        return (!node.is_empty() && node.range().end == at).then_some(node);
    }
    node.children()
        .filter(|child| child.offset() < at && at <= child.range().end)
        .find_map(|child| leaf_ending_at(child, at))
}

/// What the empty place at byte `at` of an equation of `source` is for:
/// the numerator, the denominator, the exponent, an argument of a
/// function by its name. `None` where that cannot be said. `at` is a
/// place [`super::empty_places`] finds.
#[must_use]
pub fn slot_name(source: &Source, at: usize) -> Option<&'static str> {
    let text = source.text();
    let before = text.get(..at)?.trim_end();
    let mark = leaf_ending_at(LinkedNode::new(source.root()), before.len())?;
    match mark.kind() {
        SyntaxKind::Hat | SyntaxKind::Underscore | SyntaxKind::Slash => behind_mark(&mark, text),
        SyntaxKind::LeftParen | SyntaxKind::Comma | SyntaxKind::Semicolon
            if mark.parent_kind() == Some(SyntaxKind::MathArgs) =>
        {
            argument(&mark, text)
        }
        // Brackets around nothing: of what is raised, lowered or divided
        // by, or of what is divided.
        SyntaxKind::LeftParen => {
            let group = mark.parent()?;
            if let Some(before) = beside(group, false)
                && matches!(
                    before.kind(),
                    SyntaxKind::Hat | SyntaxKind::Underscore | SyntaxKind::Slash
                )
            {
                return behind_mark(&before, text);
            }
            beside(group, true)
                .filter(|behind| behind.kind() == SyntaxKind::Slash)
                .map(|_| "numerator")
        }
        SyntaxKind::Dollar => Some("equation"),
        _ => None,
    }
}

/// What the empty place in running text at byte `at` of `source` is for,
/// where it is one that has a meaning: the caption of a figure, the text
/// of a footnote, a heading. Not a cell of a table: the cell says that.
#[must_use]
pub fn spot_name(source: &Source, at: usize) -> Option<&'static str> {
    if !is_empty_spot(source, at) {
        return None;
    }
    let text = source.text();
    let before = text.get(..at)?;
    let line = &before[before.rfind('\n').map_or(0, |found| found + 1)..];
    let marks = line.trim_start().trim_end_matches(' ');
    if !marks.is_empty() && marks.chars().all(|c| c == '=') && line.ends_with(' ') {
        return Some("heading");
    }
    match enclosing(source, at)?.kind {
        Enclosure::Caption => Some("caption"),
        Enclosure::Footnote => Some("footnote"),
        _ => None,
    }
}

/// Whether the table or grid that is the call at bytes `call` of
/// `source` is drawn with rules: by what it is told, or else by what the
/// rules of the file say; a table has them unless told otherwise, a grid
/// none.
#[must_use]
pub fn has_rules(source: &Source, call: &Range<usize>) -> bool {
    let text = source.text();
    let mut node = LinkedNode::new(source.root()).leaf_at(call.start, Side::After);
    while let Some(here) = &node {
        if here.kind() == SyntaxKind::FuncCall && here.range() == *call {
            break;
        }
        node = here.parent().cloned();
    }
    let Some(call) = node else {
        return false;
    };
    let Some(name) = call
        .children()
        .next()
        .and_then(|callee| text.get(callee.range()))
    else {
        return false;
    };
    let own = call
        .children()
        .find(|part| part.kind() == SyntaxKind::Args)
        .and_then(|args| {
            args.children().find(|arg| {
                arg.kind() == SyntaxKind::Named
                    && arg
                        .children()
                        .next()
                        .is_some_and(|key| text.get(key.range()) == Some("stroke"))
            })
        })
        .and_then(|named| named.children().next_back())
        .and_then(|value| text.get(value.range()).map(str::to_owned));
    match own.or_else(|| rule_value(source, name, "stroke")) {
        Some(stroke) => stroke.trim() != "none",
        None => name == "table",
    }
}

#[cfg(test)]
mod tests {
    use super::super::{empty_places, equations};
    use super::*;

    /// What the empty places of the equations of `text` are for, in
    /// order; `?` where that is not said.
    fn hints(text: &str) -> Vec<&'static str> {
        let source = Source::detached(text);
        equations(&source)
            .iter()
            .flat_map(|equation| empty_places(&source, equation))
            .map(|place| slot_name(&source, place).unwrap_or("?"))
            .collect()
    }

    /// What typing at the `|` of `text` goes into: its kind, and its
    /// bytes.
    fn around(text: &str) -> Option<(Enclosure, String)> {
        let at = text.find('|')?;
        let text = text.replacen('|', "", 1);
        let found = enclosing(&Source::detached(&text), at)?;
        Some((found.kind, text[found.range].to_owned()))
    }

    #[test]
    fn the_parts_of_a_fraction_and_a_root_are_named() {
        assert_eq!(hints("$frac(, )$"), ["numerator", "denominator"]);
        assert_eq!(hints("$frac(a, )$"), ["denominator"]);
        assert_eq!(hints("$frac(a, b)$"), Vec::<&str>::new());
        assert_eq!(hints("$sqrt()$"), ["radicand"]);
        assert_eq!(hints("$root(, )$"), ["index", "radicand"]);
        assert_eq!(hints("$binom(, )$"), ["upper", "lower"]);
        assert_eq!(hints("$abs()$ $underbrace(x, )$"), ["value", "annotation"]);
        assert_eq!(hints("$arrow.r()$"), ["base"]);
        assert_eq!(hints("$vec(1, , )$"), ["entry", "entry"]);
    }

    #[test]
    fn what_is_raised_lowered_and_divided_by_is_named() {
        assert_eq!(hints("$x^$ and $y_$"), ["exponent", "subscript"]);
        assert_eq!(hints("$x^() + a/$"), ["exponent", "denominator"]);
        assert_eq!(hints("$sum_()^()$"), ["lower limit", "upper limit"]);
        assert_eq!(hints("$ $"), ["equation"]);
    }

    #[test]
    fn what_typing_goes_into_is_found() {
        assert_eq!(
            around("A $x^|2$ b."),
            Some((Enclosure::Equation, "$x^2$".to_owned()))
        );
        assert_eq!(
            around("#table(columns: 2, [a|b], [])"),
            Some((Enclosure::Cell, "ab".to_owned()))
        );
        assert_eq!(
            around("#table(columns: 2, [a], [|])"),
            Some((Enclosure::Cell, String::new()))
        );
        assert_eq!(
            around("#figure(rect(), caption: [A b|ox])"),
            Some((Enclosure::Caption, "A box".to_owned()))
        );
        assert_eq!(
            around("Text#footnote[Se|e there.] on."),
            Some((Enclosure::Footnote, "See there.".to_owned()))
        );
        assert_eq!(
            around("#underline[und|er]"),
            Some((Enclosure::Block, "under".to_owned()))
        );
        // The innermost: an equation in a cell.
        assert_eq!(
            around("#table([$x|$])"),
            Some((Enclosure::Equation, "$x$".to_owned()))
        );
    }

    #[test]
    fn typing_in_an_item_of_a_list_goes_into_it() {
        assert_eq!(
            around("- One\n- Tw|o\n- Three\n"),
            Some((Enclosure::Item, "- Two".to_owned()))
        );
        // At its end too, also at the end of the text.
        assert_eq!(
            around("- One\n- Two|\n"),
            Some((Enclosure::Item, "- Two".to_owned()))
        );
        assert_eq!(
            around("+ One|"),
            Some((Enclosure::Item, "+ One".to_owned()))
        );
        assert_eq!(
            around("/ Term: what it me|ans\n"),
            Some((Enclosure::Item, "/ Term: what it means".to_owned()))
        );
        // The innermost: an item in an item, an item in a cell, an
        // equation in an item; behind the equation, the item.
        assert_eq!(
            around("- Outer\n  - inn|er\n"),
            Some((Enclosure::Item, "- inner".to_owned()))
        );
        assert_eq!(
            around("#table([- a|], [b])"),
            Some((Enclosure::Item, "- a".to_owned()))
        );
        assert_eq!(
            around("- An $x|$ in it\n"),
            Some((Enclosure::Equation, "$x$".to_owned()))
        );
        assert_eq!(
            around("- An $x$|\n"),
            Some((Enclosure::Item, "- An $x$".to_owned()))
        );
        assert_eq!(
            around("#align(center)[One.\n\n- Tw|o.]"),
            Some((Enclosure::Item, "- Two.".to_owned()))
        );
        // Not after the list, nor before the mark of an item.
        assert_eq!(around("- One\n\nAft|er.\n"), None);
        assert_eq!(around("- One\n|- Two\n"), None);
        assert_eq!(around("- #raw(\"c|\")\n"), None);
    }

    #[test]
    fn running_text_and_what_is_not_shown_go_into_nothing() {
        assert_eq!(around("Plain te|xt."), None);
        assert_eq!(around("Some *str|ong* text."), None);
        assert_eq!(around("= A head|ing"), None);
        // Behind an equation, typing goes behind it.
        assert_eq!(around("A $x$| b."), None);
        assert_eq!(around("#image(\"a|.png\")"), None);
        assert_eq!(around("#align(center)[One.\n\nTw|o.]"), None);
    }

    #[test]
    fn empty_places_of_running_text_are_named() {
        let name = |text: &str| {
            let at = text.find('|')?;
            spot_name(&Source::detached(text.replacen('|', "", 1)), at)
        };
        assert_eq!(name("#figure(rect(), caption: [|])"), Some("caption"));
        assert_eq!(name("A#footnote[|] b."), Some("footnote"));
        assert_eq!(name("Text.\n\n= |"), Some("heading"));
        assert_eq!(name("Text.\n\n== |\n"), Some("heading"));
        assert_eq!(name("#table([|])"), None);
        assert_eq!(name("#figure(rect(), caption: [A|])"), None);
        assert_eq!(name("Text.\n\n|"), None);
    }

    #[test]
    fn tables_have_rules_and_grids_none_unless_told() {
        let rules = |text: &str| {
            let source = Source::detached(text);
            let at = text.find("[a").unwrap_or(0) + 1;
            let table = super::super::table_at(&source, at).unwrap_or_else(|| panic!("a table"));
            has_rules(&source, &table.whole)
        };
        assert!(rules("#table(columns: 2, [a], [b])"));
        assert!(!rules("#table(columns: 2, stroke: none, [a], [b])"));
        assert!(!rules(
            "#set table(stroke: none)\n#table(columns: 2, [a], [b])"
        ));
        assert!(!rules("#grid(columns: 2, [a], [b])"));
        assert!(rules("#grid(columns: 2, stroke: 1pt, [a], [b])"));
    }
}
