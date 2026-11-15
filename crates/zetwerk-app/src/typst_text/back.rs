//! Shift-Tab and Shift-Enter: back to the place to fill in before, and a
//! line break that does not end the paragraph.

use typst::syntax::{LinkedNode, Side, Source, SyntaxKind};

use super::{Ground, containing, fields, next_field, table_at};

/// Where Shift-Tab leads from byte `at` of `source`: back to the place to
/// fill in before the one `at` is in, as Tab leads to the next (see
/// [`next_field`]): the cell before in a table, the argument before in
/// a call, the empty place before in an equation. From the first, to
/// before what they are the places of.
#[must_use]
pub fn prev_field(source: &Source, at: usize) -> Option<usize> {
    if let Some(table) = table_at(source, at)
        && let Some(cell) = table.cell_at(at)
    {
        let Some(before) = cell.checked_sub(1).and_then(|cell| table.cells.get(cell)) else {
            return Some(table.whole.start);
        };
        // Where Tab comes into that cell.
        let into = next_field(source, before.start)
            .filter(|into| before.start < *into && *into < before.end)
            .unwrap_or(before.start + 1);
        return Some(into);
    }
    let (chain, _) = containing(source, at);
    let stands = |node: &LinkedNode| {
        node.parent_kind() == Some(SyntaxKind::Markup)
            && matches!(
                node.kind(),
                SyntaxKind::Equation | SyntaxKind::FuncCall | SyntaxKind::Strong | SyntaxKind::Emph
            )
    };
    // The innermost thing that stands in running text and holds `at`; or
    // else the one that ends there: Shift-Tab leads back into it.
    let ended = || {
        let mut node = LinkedNode::new(source.root()).leaf_at(at, Side::Before);
        while let Some(here) = node {
            if here.range().end == at && stands(&here) {
                return Some(here);
            }
            node = here.parent().cloned();
        }
        None
    };
    let whole = chain.iter().rev().find(|node| stands(node)).cloned();
    let whole = &whole.or_else(ended)?;
    let mut found = Vec::new();
    fields(whole, source.text(), at, false, &mut found);
    found.retain(|place| *place <= at);
    found.sort_unstable();
    found.dedup();
    // Not the place `at` is in: the one before it. From behind the
    // whole, the last of them.
    if whole.range().end != at {
        found.pop();
    }
    Some(found.pop().unwrap_or(whole.range().start))
}

/// What Shift-Enter puts in at byte `at` of `source`, where a page is
/// typed on: a line break that goes on in the same paragraph, in running
/// text. `None` where it is not running text: Enter does there what it
/// does.
#[must_use]
pub fn line_break_within(source: &Source, at: usize) -> Option<String> {
    let (_, ground) = containing(source, at);
    if ground != Ground::Markup {
        return None;
    }
    // A backslash before a blank breaks the line; before anything else
    // it would make that a character as it is written. Behind a word it
    // is a mark of its own.
    let text = source.text();
    let blank = |c: Option<char>| c.is_none_or(char::is_whitespace);
    let before = blank(text.get(..at)?.chars().next_back());
    let after = blank(text.get(at..)?.chars().next());
    let mut written = String::new();
    if !before {
        written.push(' ');
    }
    written.push('\\');
    if !after {
        written.push(' ');
    }
    Some(written)
}
