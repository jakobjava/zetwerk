//! What a Typst text is read as: the words of its paragraphs without the
//! marks that set them off, the symbols of its equations as the symbols
//! they are, a blank where a line of the text breaks. A search on the
//! page goes over this (see [`zetwerk_vim::find`]), so that it finds what
//! the user sees and not what it is written with.
//!
//! Only what is sure to be read is read. What Typst makes of a call (a
//! reference's "Section 2", a heading's number) is not known before the
//! text is typeset and is read as nothing; what is given to a call as
//! content (a caption, a footnote) is read.

use std::cell::RefCell;
use std::ops::Range;
use std::sync::{Arc, LazyLock};

use typst::foundations::Value;
use typst::syntax::ast::{Escape, MathShorthand, Shorthand};
use typst::syntax::{LinkedNode, Source, SyntaxKind};
use typst::{Library, LibraryExt};
use zetwerk_vim::Reading;

/// The standard library, for what the names of symbols stand for.
static LIBRARY: LazyLock<Library> = LazyLock::new(<Library as LibraryExt>::default);

/// What the text of a file was last read as, kept until it changes: it
/// is asked for at every search and at every key while matches are
/// shown, and reading a long text is not free. When it changes, only
/// the paragraphs that changed are read again.
#[derive(Default)]
pub struct Cache(RefCell<Option<(u64, Source, Arc<Reading>)>>);

impl Cache {
    /// What `source`, the text at `revision`, is read as.
    pub fn get(&self, revision: u64, source: &Source) -> Arc<Reading> {
        let mut kept = self.0.borrow_mut();
        let fresh = match &*kept {
            Some((read, _, reading)) if *read == revision => return Arc::clone(reading),
            Some((_, old, reading)) => reading_again(reading, old, source),
            None => reading(source),
        };
        let fresh = Arc::new(fresh);
        *kept = Some((revision, source.clone(), Arc::clone(&fresh)));
        fresh
    }
}

/// Where `new` differs from `old`, in whole paragraphs: the bytes from
/// the start of the first paragraph that changed to the paragraph break
/// behind the last, in each. Before and behind them the two texts are
/// alike and taken apart alike, so read alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Paragraphs {
    pub start: usize,
    pub old_end: usize,
    pub new_end: usize,
    /// Which children of the root of `new` they are.
    first: usize,
    behind: usize,
}

/// A paragraph break among the children of the root: which child it is,
/// and its bytes.
type Break = (usize, Range<usize>);

/// The last paragraph break of `root` that ends by byte `before`, and
/// the first that begins at byte `after` or behind it.
fn breaks_around(
    root: &typst::syntax::SyntaxNode,
    before: usize,
    after: usize,
) -> (Option<Break>, Option<Break>) {
    let (mut last, mut next) = (None, None);
    let mut at = 0;
    for (index, child) in root.children().enumerate() {
        let range = at..at + child.len();
        at = range.end;
        if child.kind() != SyntaxKind::Parbreak {
            continue;
        }
        if range.end <= before {
            last = Some((index, range));
        } else if range.start >= after {
            next = Some((index, range));
            break;
        }
    }
    (last, next)
}

/// The paragraphs `new` changed of `old` (see [`Paragraphs`]). `None`
/// where the two are not taken apart alike before and behind them: the
/// same text may be, after an edit put something before it in another
/// light.
#[must_use]
pub fn paragraphs_changed(old: &Source, new: &Source) -> Option<Paragraphs> {
    // Asked twice after an edit, for what is read and for what is found:
    // the second time it is known. The texts kept keep their bytes
    // where they are, so where those are tells the texts apart.
    thread_local! {
        static LAST: RefCell<Option<(Source, Source, Option<Paragraphs>)>> =
            const { RefCell::new(None) };
    }
    let same = |a: &Source, b: &Source| std::ptr::eq(a.text(), b.text());
    let known = LAST.with_borrow(|last| match last {
        Some((was, now, changed)) if same(was, old) && same(now, new) => Some(*changed),
        _ => None,
    });
    if let Some(changed) = known {
        return changed;
    }
    let changed = paragraphs_between(old, new);
    LAST.set(Some((old.clone(), new.clone(), changed)));
    changed
}

/// How many bytes `a` and `b` begin with alike (or, `behind`, end with),
/// looked at many at a time.
fn alike(a: &[u8], b: &[u8], behind: bool) -> usize {
    const AT_ONCE: usize = 256;
    let most = a.len().min(b.len());
    let (a, b) = if behind {
        (&a[a.len() - most..], &b[b.len() - most..])
    } else {
        (&a[..most], &b[..most])
    };
    let mut same = 0;
    while same + AT_ONCE <= most {
        let (x, y) = if behind {
            (most - same - AT_ONCE, most - same)
        } else {
            (same, same + AT_ONCE)
        };
        if a[x..y] != b[x..y] {
            break;
        }
        same += AT_ONCE;
    }
    let rest = |(x, y): (&u8, &u8)| x == y;
    same + if behind {
        let (a, b) = (&a[..most - same], &b[..most - same]);
        a.iter()
            .rev()
            .zip(b.iter().rev())
            .take_while(|pair| rest(*pair))
            .count()
    } else {
        a[same..]
            .iter()
            .zip(&b[same..])
            .take_while(|pair| rest(*pair))
            .count()
    }
}

/// See [`paragraphs_changed`].
fn paragraphs_between(old: &Source, new: &Source) -> Option<Paragraphs> {
    let (was, now) = (old.text().as_bytes(), new.text().as_bytes());
    let same = alike(was, now, false);
    // Not into what is alike at the start.
    let same_behind = alike(&was[same..], &now[same..], true);
    let (old_end, new_end) = (was.len() - same_behind, now.len() - same_behind);
    let (before, behind) = breaks_around(new.root(), same, new_end);
    let start = before.as_ref().map_or(0, |(_, range)| range.end);
    let first = before.map_or(0, |(index, _)| index + 1);
    let (old_children, new_children) = (
        old.root().children().as_slice(),
        new.root().children().as_slice(),
    );
    let (behind, end) = behind.map_or((new_children.len(), now.len()), |(index, range)| {
        (index, range.start)
    });
    // The same nodes: most are those of the old text itself, kept by
    // the edit, and are told to be the same at once.
    let was_behind = (old_children.len() + behind).checked_sub(new_children.len())?;
    let alike = first <= was_behind
        && old_children.get(..first) == new_children.get(..first)
        && old_children[was_behind..] == new_children[behind..];
    alike.then_some(Paragraphs {
        start,
        old_end: end - new_end + old_end,
        new_end: end,
        first,
        behind,
    })
}

/// What `new` is read as, from what `old` was read as, `reading`: only
/// the paragraphs that changed are read.
fn reading_again(reading: &Reading, old: &Source, new: &Source) -> Reading {
    let Some(changed) = paragraphs_changed(old, new) else {
        return self::reading(new);
    };
    let mut between = Reading::new();
    let root = LinkedNode::new(new.root());
    let children = root.children().skip(changed.first);
    for child in children.take(changed.behind - changed.first) {
        read(&child, new.text(), Within::Markup, &mut between);
    }
    reading.splice(changed.start, changed.old_end, changed.new_end, &between)
}

/// The matches of `query` in `new`, from `found`, those in `old`: only
/// the paragraphs that changed are looked through (see
/// [`zetwerk_vim::find::find_within`]). `reading` is what `new` is read
/// as, if what is read is looked through. `None` where that cannot be
/// done, or where as many are found as are looked for at most.
#[must_use]
pub fn found_again(
    found: &[Range<usize>],
    old: &Source,
    new: &Source,
    reading: Option<&Reading>,
    query: &zetwerk_vim::Query,
) -> Option<Vec<Range<usize>>> {
    use zetwerk_vim::find::{MOST, find_within};
    if found.len() >= MOST {
        return None;
    }
    let changed = paragraphs_changed(old, new)?;
    let within = changed.start..changed.new_end;
    let (looked, between) = find_within(new.text(), reading, query, within)?;
    let behind = looked.end - changed.new_end + changed.old_end;
    let mut matches: Vec<Range<usize>> = found
        .iter()
        .take_while(|range| range.end <= looked.start)
        .cloned()
        .collect();
    matches.extend(between);
    let first_behind = found.partition_point(|range| range.start < behind);
    matches.extend(found[first_behind..].iter().map(|range| {
        range.start - changed.old_end + changed.new_end
            ..range.end - changed.old_end + changed.new_end
    }));
    (matches.len() < MOST).then_some(matches)
}

/// What a node is in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Within {
    Markup,
    Math,
    Code,
}

/// What `source` is read as.
#[must_use]
pub fn reading(source: &Source) -> Reading {
    let mut reading = Reading::new();
    let root = LinkedNode::new(source.root());
    read(&root, source.text(), Within::Markup, &mut reading);
    reading
}

/// A blank for bytes `range`, unless what is read so far ends in one or
/// nothing is read yet: what is read has one blank between words.
fn blank(reading: &mut Reading, range: Range<usize>) {
    if reading
        .text()
        .chars()
        .next_back()
        .is_some_and(|c| c != ' ' && c != '\n')
    {
        reading.push(" ", range);
    }
}

/// Reads `node`, which is `within` markup, an equation or code.
fn read(node: &LinkedNode, text: &str, within: Within, reading: &mut Reading) {
    match within {
        Within::Markup => markup(node, text, reading),
        Within::Math => math(node, text, reading),
        Within::Code => code(node, text, reading),
    }
}

/// Reads the children of `node`, all `within` the same.
fn children(node: &LinkedNode, text: &str, within: Within, reading: &mut Reading) {
    for child in node.children() {
        read(&child, text, within, reading);
    }
}

/// Reads a node of markup.
fn markup(node: &LinkedNode, text: &str, reading: &mut Reading) {
    let range = node.range();
    match node.kind() {
        SyntaxKind::Text | SyntaxKind::Link | SyntaxKind::SmartQuote => {
            reading.push_verbatim(node.leaf_text(), range.start);
        }
        SyntaxKind::Space | SyntaxKind::Linebreak => blank(reading, range),
        SyntaxKind::Parbreak => reading.push("\n", range),
        SyntaxKind::Escape => {
            if let Some(escape) = node.cast::<Escape>() {
                reading.push(escape.get().encode_utf8(&mut [0; 4]), range);
            }
        }
        SyntaxKind::Shorthand => {
            if let Some(shorthand) = node.cast::<Shorthand>() {
                reading.push(shorthand.get().encode_utf8(&mut [0; 4]), range);
            }
        }
        SyntaxKind::Raw => {
            for child in node.children() {
                match child.kind() {
                    SyntaxKind::Text => {
                        reading.push_verbatim(child.leaf_text(), child.range().start);
                    }
                    SyntaxKind::RawTrimmed if child.leaf_text().contains('\n') => {
                        blank(reading, child.range());
                    }
                    _ => {}
                }
            }
        }
        SyntaxKind::Equation => children(node, text, Within::Math, reading),
        SyntaxKind::Markup
        | SyntaxKind::Strong
        | SyntaxKind::Emph
        | SyntaxKind::Heading
        | SyntaxKind::ListItem
        | SyntaxKind::EnumItem
        | SyntaxKind::TermItem
        | SyntaxKind::Ref => children(node, text, Within::Markup, reading),
        _ if node.children().next().is_some() => code(node, text, reading),
        // Marks, labels, comments, the `#` before code: nothing.
        _ => {}
    }
}

/// Reads a node of code: only what is given to it as content is read,
/// and a symbol named by itself.
fn code(node: &LinkedNode, text: &str, reading: &mut Reading) {
    let range = node.range();
    match node.kind() {
        SyntaxKind::ContentBlock if apart(node) => {
            blank(reading, range.start..range.start);
            children(node, text, Within::Markup, reading);
            blank(reading, range.end..range.end);
        }
        SyntaxKind::ContentBlock => children(node, text, Within::Markup, reading),
        SyntaxKind::Markup => markup(node, text, reading),
        SyntaxKind::Ident | SyntaxKind::FieldAccess => {
            if let Some(symbol) = symbol(node, false) {
                reading.push(&symbol, range);
            }
        }
        // What only says how things look, or brings in what is read
        // elsewhere.
        SyntaxKind::LetBinding
        | SyntaxKind::SetRule
        | SyntaxKind::ShowRule
        | SyntaxKind::ModuleImport
        | SyntaxKind::ModuleInclude => {}
        _ => children(node, text, Within::Code, reading),
    }
}

/// Whether `node`, content given to a call, is a thing of its own and
/// not part of the words around it: given by name (a caption), or the
/// text of a footnote.
fn apart(node: &LinkedNode) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if parent.kind() == SyntaxKind::Named {
        return true;
    }
    let callee = parent.parent().and_then(|call| call.children().next());
    callee.is_some_and(|callee| callee.leaf_text() == "footnote")
}

/// Reads a node of an equation. Blanks are not read: an equation sets
/// its own spaces.
fn math(node: &LinkedNode, text: &str, reading: &mut Reading) {
    let range = node.range();
    match node.kind() {
        SyntaxKind::MathText | SyntaxKind::Text | SyntaxKind::MathPrimes | SyntaxKind::Root => {
            reading.push_verbatim(node.leaf_text(), range.start);
        }
        SyntaxKind::MathIdent | SyntaxKind::MathFieldAccess => match symbol(node, true) {
            Some(symbol) => reading.push(&symbol, range),
            None if node.kind() == SyntaxKind::MathIdent => {
                reading.push_verbatim(node.leaf_text(), range.start);
            }
            None => {}
        },
        SyntaxKind::MathShorthand => {
            if let Some(shorthand) = node.cast::<MathShorthand>() {
                reading.push(shorthand.get().encode_utf8(&mut [0; 4]), range);
            }
        }
        SyntaxKind::Escape => {
            if let Some(escape) = node.cast::<Escape>() {
                reading.push(escape.get().encode_utf8(&mut [0; 4]), range);
            }
        }
        SyntaxKind::Str => {
            let inner = range.start + 1..range.end.saturating_sub(1).max(range.start + 1);
            if let Some(inside) = text.get(inner.clone()) {
                reading.push_verbatim(inside, inner.start);
            }
        }
        SyntaxKind::MathCall => {
            // The name of what is called is not read, its arguments are.
            for child in node.children().filter(|c| c.kind() == SyntaxKind::MathArgs) {
                arguments(&child, text, reading);
            }
        }
        SyntaxKind::MathAttach | SyntaxKind::MathFrac => {
            // What is raised, lowered or divided loses its brackets.
            for child in node.children() {
                ungrouped(&child, text, reading);
            }
        }
        SyntaxKind::Linebreak => blank(reading, range),
        SyntaxKind::Math | SyntaxKind::MathDelimited | SyntaxKind::MathRoot => {
            children(node, text, Within::Math, reading);
        }
        _ if node.children().next().is_some() => code(node, text, reading),
        // Blanks, `^`, `_`, `/`, `&`, `#`, the dollar signs: nothing.
        _ => {}
    }
}

/// Reads the arguments of a call in an equation, without the brackets
/// and commas around them, nor what is given by name.
fn arguments(node: &LinkedNode, text: &str, reading: &mut Reading) {
    for child in node.children() {
        match child.kind() {
            SyntaxKind::LeftParen
            | SyntaxKind::RightParen
            | SyntaxKind::Comma
            | SyntaxKind::Semicolon
            | SyntaxKind::Named => {}
            _ => math(&child, text, reading),
        }
    }
}

/// Reads `node`, a part of an equation, without the round brackets that
/// only hold it together.
fn ungrouped(node: &LinkedNode, text: &str, reading: &mut Reading) {
    let round = node.kind() == SyntaxKind::MathDelimited
        && text
            .get(node.range())
            .is_some_and(|written| written.starts_with('(') && written.ends_with(')'));
    if round {
        for child in node.children().filter(|c| c.kind() == SyntaxKind::Math) {
            math(&child, text, reading);
        }
    } else {
        math(node, text, reading);
    }
}

/// The symbol `node` names, a name or a name with modifiers, if it names
/// one: in an equation, of the names there; elsewhere, of all names.
fn symbol(node: &LinkedNode, in_math: bool) -> Option<String> {
    let Value::Symbol(symbol) = value_of(node, in_math)? else {
        return None;
    };
    Some(symbol.get().to_owned())
}

/// What `node` stands for in the standard library, if it is a name or a
/// field of one.
fn value_of(node: &LinkedNode, in_math: bool) -> Option<Value> {
    match node.kind() {
        SyntaxKind::MathIdent | SyntaxKind::Ident => {
            let scope = if in_math {
                LIBRARY.math.scope()
            } else {
                LIBRARY.global.scope()
            };
            Some(scope.get(node.leaf_text())?.read().clone())
        }
        SyntaxKind::MathFieldAccess | SyntaxKind::FieldAccess => {
            let target = node.children().next()?;
            let field = node.children().next_back()?;
            value_of(&target, in_math)?
                .field(field.leaf_text(), ())
                .ok()
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_read_and_found_again_after_an_edit_is_what_is_read_and_found_anew() {
        const ROUNDS: usize = 500;
        use zetwerk_vim::Query;
        use zetwerk_vim::find::find_all;
        let article = include_str!("../../zetwerk-corpus/docs/article.typ");
        let mut source = Source::detached(article.repeat(2));
        let cache = Cache::default();
        let _ = cache.get(0, &source);
        let queries: Vec<Query> = ["the", "e", "\u{3b1}", "of the", "\\=$", "\\=the ", "a b"]
            .into_iter()
            .map(|pattern| Query {
                pattern: pattern.to_owned(),
                word_start: pattern == "e",
                word_end: false,
            })
            .collect();
        let mut found: Vec<[Vec<Range<usize>>; 2]> = queries
            .iter()
            .map(|query| {
                let read = reading(&source);
                [
                    find_all(source.text(), Some(&read), query),
                    find_all(source.text(), None, query),
                ]
            })
            .collect();
        // What is typed, and taken out: also what parses the rest anew.
        let typed = [
            "", "x", " ", "\n\n", "$", "*", "#", "[", "]", "$x$", "the ", "\n", "= ", "- ", "\"",
            "alpha", "_", "#strong[", "/* ", "`",
        ];
        let (mut seed, mut quick) = (7_u64, 0);
        for round in 1..ROUNDS {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let text = source.text();
            let mut at = usize::try_from(seed >> 33).unwrap_or(0) % (text.len() + 1);
            while !text.is_char_boundary(at) {
                at -= 1;
            }
            // Now and then much: paragraphs, and their breaks.
            let most = if round % 7 == 0 { 600 } else { 5 };
            let mut end = (at + usize::try_from((seed >> 20) % most).unwrap_or(0)).min(text.len());
            while !text.is_char_boundary(end) {
                end += 1;
            }
            // And now and then a paragraph or two of the article.
            let pasted = usize::try_from(seed >> 40).unwrap_or(0) % (article.len() - 300);
            let pasted = (pasted..pasted + 300)
                .find(|from| article.is_char_boundary(*from))
                .unwrap_or(0);
            let mut until = pasted + 300;
            while !article.is_char_boundary(until) {
                until += 1;
            }
            let put = match usize::try_from((seed >> 8) % 26).unwrap_or(0) {
                pick @ 0..20 => typed[pick],
                _ => &article[pasted..until],
            };
            let old = source.clone();
            source.edit(at..end, put);
            let anew = reading(&source);
            let again = cache.get(u64::try_from(round).unwrap_or(0), &source);
            assert_eq!(format!("{again:?}"), format!("{anew:?}"), "round {round}");
            for (query, found) in queries.iter().zip(&mut found) {
                for (kept, read) in found.iter_mut().zip([Some(&anew), None]) {
                    let all = find_all(source.text(), read, query);
                    let again = found_again(kept, &old, &source, read, query);
                    if let Some(again) = again {
                        assert_eq!(again, all, "round {round}, {query:?}");
                        quick += 1;
                    }
                    *kept = all;
                }
            }
        }
        // Nearly always only the paragraphs that changed are looked at.
        assert!(quick > (ROUNDS - 1) * 14 * 3 / 4, "{quick}");
    }

    fn read(text: &str) -> (Source, Reading) {
        let source = Source::detached(text);
        let reading = reading(&source);
        (source, reading)
    }

    fn text_of(text: &str) -> String {
        read(text).1.text().to_owned()
    }

    #[test]
    fn markup_is_not_read_and_line_breaks_are_blanks() {
        assert_eq!(
            text_of("One *two*\n_three_ fo#strong[u]r."),
            "One two three four."
        );
        assert_eq!(
            text_of("= Head\n\nA ~ b -- c \\#"),
            "Head\nA \u{a0} b \u{2013} c #"
        );
        assert_eq!(text_of("- one\n- two\n"), "one two ");
        assert_eq!(text_of("Raw `let x` here."), "Raw let x here.");
    }

    #[test]
    fn equations_read_as_their_symbols() {
        assert_eq!(text_of("If $alpha + beta$ then"), "If α+β then");
        assert_eq!(text_of("$x^(n+1) -> frac(a, b)$"), "xn+1→ab");
        assert_eq!(text_of("$arrow.r$ $sin x$"), "→ sinx");
    }

    #[test]
    fn code_is_read_for_the_content_given_to_it() {
        let text = "#set text(size: 11pt)\n#let x = [no]\nA#footnote[note] b.\n\
                    #figure(image(\"a.png\"), caption: [A cat])\n";
        assert_eq!(text_of(text), "A note b. A cat ");
        assert_eq!(text_of("See @intro and #sym.alpha."), "See and α.");
    }

    #[test]
    fn what_is_read_is_found_where_it_is_written() {
        let (source, reading) = read("A *bold*\nword and $alpha$.");
        let found = zetwerk_vim::find::find_all(
            source.text(),
            Some(&reading),
            &zetwerk_vim::Query {
                pattern: "bold word".to_owned(),
                word_start: false,
                word_end: false,
            },
        );
        assert_eq!(found.len(), 1);
        assert_eq!(&source.text()[found[0].clone()], "bold*\nword");
    }
}
