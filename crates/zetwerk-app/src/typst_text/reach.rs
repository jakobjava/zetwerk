//! Reaching a place of a text without going through all of it.
//!
//! The root of a Typst text holds every word and blank of it that is not
//! inside something else: on a thesis, hundreds of thousands of nodes.
//! Going from the root to a place (`LinkedNode::leaf_at`, and with it
//! what the place is, the equation it is in) goes through them one by
//! one, and a key asks so a dozen times, at a millisecond each. Where
//! each child of the root begins is kept here for each text, so that the
//! child that holds a place is found at once; from there it is gone into
//! as it was, the child taken for a root of its own, and what is found
//! moved by where it begins.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use typst::syntax::{LinkedNode, Source, SyntaxNode};

/// How many texts the beginnings of the children are kept for: the one
/// being edited, and the one put together to show it.
const KEPT: usize = 2;

thread_local! {
    /// The texts last asked about, with where each child of their root
    /// begins. The texts are kept themselves: so the bytes they are at
    /// tell them apart.
    static STARTS: RefCell<Vec<(Source, Rc<[usize]>)>> = const { RefCell::new(Vec::new()) };
}

/// Where each child of the root of `source` begins, in bytes.
fn starts(source: &Source) -> Rc<[usize]> {
    let same = |kept: &Source| std::ptr::eq(kept.text(), source.text());
    if let Some(starts) = STARTS.with_borrow(|kept| {
        kept.iter()
            .find(|(text, _)| same(text))
            .map(|(_, starts)| Rc::clone(starts))
    }) {
        return starts;
    }
    let mut at = 0;
    let starts: Rc<[usize]> = source
        .root()
        .children()
        .map(|child| {
            let start = at;
            at += child.len();
            start
        })
        .collect();
    STARTS.with_borrow_mut(|kept| {
        if kept.len() >= KEPT {
            kept.remove(0);
        }
        kept.push((source.clone(), Rc::clone(&starts)));
    });
    starts
}

/// The child of the root of `source` that the last of those beginning
/// before byte `at` (`strictly`) or at it is, with where it begins, if
/// `at` is before its end. That is the child `LinkedNode::leaf_at` goes
/// into for `Side::After` (not `strictly`), or that holds `at` with
/// bytes before and behind it (`strictly`): children do not overlap,
/// and one of no bytes holds nothing.
fn child(source: &Source, at: usize, strictly: bool) -> Option<(&SyntaxNode, usize)> {
    let starts = starts(source);
    let behind = starts.partition_point(|start| *start < at || (!strictly && *start == at));
    let index = behind.checked_sub(1)?;
    let node = source.root().children().as_slice().get(index)?;
    let start = starts[index];
    (at < start + node.len()).then_some((node, start))
}

/// The child of the root of `source` that byte `at` is inside of (not at
/// an end of), with where it begins: what `containing` goes into.
pub(super) fn child_inside(source: &Source, at: usize) -> Option<(&SyntaxNode, usize)> {
    child(source, at, true)
}

/// The child of the root of `source` that holds byte `at`, with where it
/// begins: what `LinkedNode::leaf_at` goes into for `Side::After`. `None`
/// where that finds nothing.
pub(super) fn child_after(source: &Source, at: usize) -> Option<(&SyntaxNode, usize)> {
    if source.root().children().len() == 0 {
        return None;
    }
    child(source, at, false)
}

/// The leaf of `source` at byte `at`, the one behind it, and its
/// ancestors up to (not with) the root, from the inside out: as
/// `LinkedNode::leaf_at` with `Side::After` gives them, but each a node
/// of the child of the root it is in, which begins at the byte given.
pub(super) fn leaf_after(source: &Source, at: usize) -> Option<(LinkedNode<'_>, usize)> {
    let (child, start) = child_after(source, at)?;
    let leaf = LinkedNode::new(child).leaf_at(at - start, typst::syntax::Side::After)?;
    Some((leaf, start))
}

/// What `LinkedNode::leaf_at` with `Side::Before` finds at byte `at` of
/// `source`, as [`leaf_after`] gives it: a node of the child of the root
/// it is in, which begins at the byte given. That child is the one that
/// holds `at` with a byte before it, or else the last child if that
/// begins at `at`.
pub(super) fn leaf_before(source: &Source, at: usize) -> Option<(LinkedNode<'_>, usize)> {
    let children = source.root().children().as_slice();
    let starts = starts(source);
    let holds = starts.partition_point(|start| *start < at).checked_sub(1);
    let index = holds
        .filter(|index| at <= starts[*index] + children[*index].len())
        .or_else(|| {
            let last = children.len().checked_sub(1)?;
            (starts[last] == at).then_some(last)
        })?;
    let start = starts[index];
    let leaf =
        LinkedNode::new(&children[index]).leaf_at(at - start, typst::syntax::Side::Before)?;
    Some((leaf, start))
}

/// The children of the root of `source` that have bytes in `range`, each
/// with where it begins; and a child before or behind them of no bytes.
pub(super) fn children_over<'a>(
    source: &'a Source,
    range: &Range<usize>,
) -> impl Iterator<Item = (&'a SyntaxNode, usize)> + use<'a> {
    let starts = starts(source);
    let first = starts
        .partition_point(|start| *start <= range.start)
        .saturating_sub(1);
    let children = source.root().children().as_slice();
    let end = range.end;
    (first..children.len())
        .map(move |index| (&children[index], starts[index]))
        .take_while(move |(_, start)| *start < end)
}

#[cfg(test)]
mod tests {
    use super::super::{ancestors, containing, equation_at, ground, is_in_call, is_in_equation};
    use super::super::{stand_ins, table_at};
    use typst::syntax::{LinkedNode, Source, SyntaxKind};

    const ARTICLE: &str = include_str!("../../../zetwerk-corpus/docs/article.typ");

    /// A text with all the kinds of places, edited at random: what is
    /// found below the root is what is found going from it.
    #[test]
    fn what_is_found_below_the_root_is_what_is_found_from_it() {
        let table = "\n\n#table(columns: 2, table.header([a], [b]), [x $y^2$], [z])\n\n\
                     Note#footnote[with $a$ and *b*] - item\n\n- one\n- two $c$ <l>\n\n";
        let mut text = ARTICLE.to_owned();
        text.insert_str(text.len() / 2, table);
        let mut source = Source::detached(text);
        let typed = [
            "",
            "x",
            " ",
            "\n\n",
            "$",
            "*",
            "#",
            "[",
            "]",
            "$x^$",
            "#table([a], [",
            "\"",
            "_",
            "#footnote[",
            "- ",
            "@eq-",
            "<l>",
            "/* ",
            "`",
        ];
        let mut seed = 5_u64;
        for round in 0..60 {
            let text = source.text();
            for at in 0..=text.len() {
                let (chain, expected) = containing(&source, at);
                assert_eq!(ground(&source, at), expected, "round {round}, at {at}");
                let called = |function: &str| {
                    chain.iter().any(|node| {
                        node.kind() == SyntaxKind::FuncCall
                            && node
                                .children()
                                .next()
                                .is_some_and(|name| text.get(name.range()) == Some(function))
                    })
                };
                for function in ["footnote", "table"] {
                    assert_eq!(is_in_call(&source, at, function), called(function));
                }
                let last = text.len().saturating_sub(1);
                let equation = ancestors(&source, at.min(last))
                    .iter()
                    .rfind(|node| node.kind() == SyntaxKind::Equation)
                    .map(LinkedNode::range);
                assert_eq!(equation_at(&source, at), equation, "round {round}, at {at}");
                assert_eq!(is_in_equation(&source, at), equation.is_some());
                if let Some(table) = table_at(&source, at) {
                    assert!(text[table.whole.clone()].starts_with("table("), "{at}");
                    assert!(
                        table
                            .cells
                            .iter()
                            .all(|cell| text[cell.clone()].starts_with('['))
                    );
                    assert!(table.whole.contains(&at));
                }
            }
            for start in (0..text.len()).step_by(97) {
                let end = (start + 400).min(text.len());
                if text.is_char_boundary(start) && text.is_char_boundary(end) {
                    let _ = stand_ins(&source, &(start..end));
                }
            }
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let mut at = usize::try_from(seed >> 33).unwrap_or(0) % (text.len() + 1);
            while !text.is_char_boundary(at) {
                at -= 1;
            }
            let mut end = (at + usize::try_from((seed >> 20) % 6).unwrap_or(0)).min(text.len());
            while !text.is_char_boundary(end) {
                end += 1;
            }
            let put = typed[usize::try_from(seed >> 8).unwrap_or(0) % typed.len()];
            source.edit(at..end, put);
        }
    }

    #[test]
    fn the_children_of_the_root_are_found_by_where_they_begin() {
        let source = Source::detached("a *b* $c$\n\nd");
        let root = source.root();
        let mut at = 0;
        for child in root.children() {
            for byte in at..at + child.len() {
                let (found, start) = super::child_after(&source, byte).unwrap_or_else(|| {
                    panic!("{byte}");
                });
                assert_eq!((found, start), (child, at));
            }
            at += child.len();
        }
        assert!(super::child_after(&source, at).is_none());
        assert!(super::child_inside(&source, 0).is_none());
        // Not at the end of one: inside it.
        assert_eq!(
            super::child_inside(&source, 3).map(|(_, start)| start),
            Some(2)
        );
        assert_eq!(super::child_inside(&source, 2), None);
        let over: Vec<usize> = super::children_over(&source, &(3..7))
            .map(|(_, start)| start)
            .collect();
        assert_eq!(over, [2, 5, 6]);
    }
}
