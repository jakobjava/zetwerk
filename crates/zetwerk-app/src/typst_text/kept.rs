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
/// and the byte it begins at; it finds only within that node, and what
/// it finds there may depend only on that node and its text. What it gives with each is not moved: it is to be told
/// relative to the bytes it comes with.
pub fn found_again<T: Clone>(
    kept: Option<&(Source, Found<T>)>,
    source: &Source,
    find: &dyn Fn(&SyntaxNode, usize) -> Found<T>,
) -> Found<T> {
    let Some((old, found)) = kept else {
        return everywhere(source, find);
    };
    if std::ptr::eq(old.text(), source.text()) {
        return found.clone();
    }
    let Some(changed) = crate::reading::paragraphs_changed(old, source) else {
        return everywhere(source, find);
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

/// What `find` finds in each child of the root of `source`.
fn everywhere<T>(source: &Source, find: &dyn Fn(&SyntaxNode, usize) -> Found<T>) -> Found<T> {
    let mut found = Vec::new();
    let mut at = 0;
    for child in source.root().children() {
        found.extend(find(child, at));
        at += child.len();
    }
    found
}

#[cfg(test)]
mod tests {
    use typst::syntax::Source;

    use super::super::{bibliography_file, left_empty_everywhere};

    /// What is found from what was kept, after random edits, is what is
    /// found anew: on a thread of its own, which has nothing kept.
    #[test]
    fn what_is_found_again_is_what_is_found_anew() {
        let article = include_str!("../../../zetwerk-corpus/docs/article.typ");
        let mut text = article.repeat(2);
        text.insert_str(text.len() / 3, "\n\n#bibliography(\"one.bib\")\n\n");
        text.push_str("\n#bibliography((\"two.bib\", \"three.bib\"))\n");
        let mut source = Source::detached(text);
        let typed = [
            "",
            "x",
            " ",
            "\n\n",
            "$",
            "^",
            "/",
            "#",
            "[",
            "]",
            "\"",
            "#bibliography(\"four.bib\")",
            "$x^$",
            "#f[",
            "/* ",
            "`",
        ];
        let mut seed = 3_u64;
        for round in 0..300 {
            let anew = {
                let source = source.clone();
                std::thread::spawn(move || {
                    (
                        bibliography_file(&source),
                        left_empty_everywhere(&source, None),
                    )
                })
                .join()
                .unwrap_or_else(|_| panic!("round {round}"))
            };
            let again = (
                bibliography_file(&source),
                left_empty_everywhere(&source, None),
            );
            assert_eq!(again, anew, "round {round}");
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let text = source.text();
            let mut at = usize::try_from(seed >> 33).unwrap_or(0) % (text.len() + 1);
            while !text.is_char_boundary(at) {
                at -= 1;
            }
            let most = if round % 9 == 0 { 300 } else { 5 };
            let mut end = (at + usize::try_from((seed >> 20) % most).unwrap_or(0)).min(text.len());
            while !text.is_char_boundary(end) {
                end += 1;
            }
            let put = typed[usize::try_from(seed >> 8).unwrap_or(0) % typed.len()];
            source.edit(at..end, put);
        }
    }
}
