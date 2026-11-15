//! What is found in a text, kept from one key to the next.
//!
//! Some of what every key asks of the text goes through all of it: on a
//! thesis that is much of what a key costs. What is found in each child
//! of the root is found again only in the children an edit changed (see
//! [`crate::reading::paragraphs_changed`]); before and behind them the
//! old and the new text are the same nodes, and what is found there is
//! what was found, moved by as many bytes as the edit put in or took.

use std::ops::Range;

use typst::syntax::{Source, SyntaxNode};

/// What was found in a text: each with the bytes of the node it was found
/// in, which is within one child of the root, in the order of the text.
pub type Found<T> = Vec<(Range<usize>, T)>;

/// What `find` finds in all of `source`, from what it found in `old`
/// (`kept`) where the two are alike. `find` is given a child of the root
/// (or the root) and the byte it begins at; it finds only within that
/// node, and what it finds there may depend only on that node and its
/// text. What it gives with each is not moved: it is to be told
/// relative to the bytes it comes with.
pub fn found_again<T: Clone>(
    kept: Option<&(Source, Found<T>)>,
    source: &Source,
    find: &dyn Fn(&SyntaxNode, usize) -> Found<T>,
) -> Found<T> {
    let Some((old, found)) = kept else {
        return find(source.root(), 0);
    };
    if std::ptr::eq(old.text(), source.text()) {
        return found.clone();
    }
    let Some(changed) = crate::reading::paragraphs_changed(old, source) else {
        return find(source.root(), 0);
    };
    let mut again: Found<T> = found
        .iter()
        .take_while(|(bytes, _)| bytes.end <= changed.start)
        .cloned()
        .collect();
    let mut at = changed.start;
    let children = source.root().children().as_slice();
    for child in children.get(changed.children()).unwrap_or_default() {
        again.extend(find(child, at));
        at += child.len();
    }
    let behind = found.partition_point(|(bytes, _)| bytes.start < changed.old_end);
    let moved = |at: usize| at - changed.old_end + changed.new_end;
    again.extend(
        found[behind..]
            .iter()
            .map(|(bytes, what)| (moved(bytes.start)..moved(bytes.end), what.clone())),
    );
    again
}
