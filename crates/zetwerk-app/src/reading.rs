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
/// shown, and reading a long text is not free.
#[derive(Default)]
pub struct Cache(RefCell<Option<(u64, Arc<Reading>)>>);

impl Cache {
    /// What `source`, the text at `revision`, is read as.
    pub fn get(&self, revision: u64, source: &Source) -> Arc<Reading> {
        let mut kept = self.0.borrow_mut();
        match &*kept {
            Some((read, reading)) if *read == revision => Arc::clone(reading),
            _ => {
                let fresh = Arc::new(reading(source));
                *kept = Some((revision, Arc::clone(&fresh)));
                fresh
            }
        }
    }
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
