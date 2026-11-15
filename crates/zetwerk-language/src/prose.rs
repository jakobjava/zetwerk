//! The running text of Typst source: what is read, without what is
//! written to make it look as it does.
//!
//! A checker of spelling or grammar must see words and sentences, not
//! markup. So the syntax tree is walked and the text of its text leaves
//! put together: an equation, a reference, a call of a function become a
//! word that stands for them (a sentence stays a sentence), code and raw
//! text are left out, and the text of a caption or a footnote is set
//! apart from the sentence it is written in. Every stretch of what comes
//! out knows the bytes of the source it was made of, so that what a
//! checker finds can be shown, and put right, where it is written.
//!
//! The source is checked in [`Unit`]s: its paragraphs. A unit is what is
//! looked up in the cache, by its text, so a change to one paragraph
//! costs the checking of one paragraph.

use std::borrow::Cow;
use std::ops::Range;

use typst_syntax::ast;
use typst_syntax::{SyntaxKind, SyntaxNode};

/// What stands between the pieces of a [`Unit`]'s key, where a part of
/// it that is checked by itself was taken out.
const HOLE: char = '\u{1}';

/// The functions whose content is part of the sentence they are called
/// in: they change how words look, not what is said.
const INLINE: &[&str] = &[
    "emph",
    "strong",
    "text",
    "underline",
    "overline",
    "strike",
    "highlight",
    "smallcaps",
    "sub",
    "super",
    "link",
    "box",
    "upper",
    "lower",
];

/// The functions that put nothing into the sentence they are called in.
const SILENT: &[&str] = &[
    "footnote",
    "h",
    "v",
    "pagebreak",
    "colbreak",
    "place",
    "hide",
    "metadata",
    "label",
];

/// How a stretch of the running text relates to the source it is from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shows {
    /// The very bytes of the source.
    Exact,
    /// What the source writes another way: an escape, a shorthand, a
    /// blank that is a line break there.
    Decoded,
    /// A word that stands for something that is no text: an equation, a
    /// reference, a call.
    Stands,
}

/// A stretch of the running text and the source it was made of.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Segment {
    /// Where it begins in the running text, and how long it is there.
    at: usize,
    len: usize,
    source: Range<usize>,
    shows: Shows,
}

/// The running text of a piece of source.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Prose {
    text: String,
    segments: Vec<Segment>,
}

impl Prose {
    /// The running text of `nodes`, which follow one another in a piece
    /// of markup, the first at byte `offset` of the source.
    #[must_use]
    pub fn of(nodes: &[SyntaxNode], offset: usize) -> Self {
        let mut walker = Walker {
            streams: vec![Prose::default()],
            open: vec![0],
            stand_ins: 0,
        };
        walker.markup(nodes, offset);
        let mut prose = Prose::default();
        for stream in walker.streams {
            let read = stream.text.trim_end();
            if read.is_empty() {
                continue;
            }
            if !prose.text.is_empty() {
                prose.text.push_str("\n\n");
            }
            let moved = prose.text.len();
            prose.text.push_str(read);
            let within = stream.segments.into_iter().filter(|s| s.at < read.len());
            prose.segments.extend(within.map(|segment| Segment {
                at: segment.at + moved,
                len: segment.len.min(read.len() - segment.at),
                ..segment
            }));
        }
        prose
    }

    /// The text: sentences as they are read, paragraphs set apart by
    /// empty lines.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The segments that bytes `range` of the text are of.
    fn segments_of(&self, range: &Range<usize>) -> &[Segment] {
        let first = self
            .segments
            .partition_point(|segment| segment.at + segment.len <= range.start);
        let behind = self
            .segments
            .partition_point(|segment| segment.at < range.end);
        self.segments.get(first..behind).unwrap_or(&[])
    }

    /// Whether bytes `range` of the text are, in part, a word that
    /// stands for something that is no text.
    #[must_use]
    pub fn stands_in(&self, range: &Range<usize>) -> bool {
        let segments = self.segments_of(range);
        segments
            .iter()
            .any(|segment| segment.shows == Shows::Stands)
    }

    /// The bytes of the source that bytes `range` of the text were made
    /// of. `None` if they were made of none: they are what sets
    /// paragraphs apart.
    #[must_use]
    pub fn source(&self, range: &Range<usize>) -> Option<Range<usize>> {
        let segments = self.segments_of(range);
        let (first, last) = (segments.first()?, segments.last()?);
        let start = match first.shows {
            Shows::Exact => first.source.start + range.start.saturating_sub(first.at),
            Shows::Decoded | Shows::Stands => first.source.start,
        };
        let end = match last.shows {
            Shows::Exact => last.source.start + (range.end.min(last.at + last.len) - last.at),
            Shows::Decoded | Shows::Stands => last.source.end,
        };
        (start <= end).then_some(start..end)
    }

    /// Whether the text ends where a paragraph does (or has not begun).
    fn at_boundary(&self) -> bool {
        self.text.is_empty() || self.text.ends_with('\n')
    }

    /// Adds `text` as made of bytes `source` of the source.
    fn push(&mut self, text: &str, source: Range<usize>, shows: Shows) {
        if text.is_empty() {
            return;
        }
        let at = self.text.len();
        self.text.push_str(text);
        // Text that goes on, in the text and in the source, is one
        // stretch.
        if let Some(last) = self.segments.last_mut()
            && shows == Shows::Exact
            && last.shows == Shows::Exact
            && last.at + last.len == at
            && last.source.end == source.start
        {
            last.len += text.len();
            last.source.end = source.end;
            return;
        }
        self.segments.push(Segment {
            at,
            len: text.len(),
            source,
            shows,
        });
    }
}

/// Walks markup and puts its running text together.
struct Walker {
    /// The texts found: the first is that of the markup itself, the
    /// others are those set apart from it (a caption, a footnote).
    streams: Vec<Prose>,
    /// The texts being written, the innermost last.
    open: Vec<usize>,
    /// How many words that stand for something were put in.
    stand_ins: usize,
}

impl Walker {
    fn stream(&mut self) -> &mut Prose {
        let innermost = self.open.last().copied().unwrap_or(0);
        &mut self.streams[innermost]
    }

    /// Ends the paragraph being written.
    fn boundary(&mut self) {
        let stream = self.stream();
        let read = stream.text.trim_end_matches(' ').len();
        stream.text.truncate(read);
        while stream
            .segments
            .last()
            .is_some_and(|segment| segment.at >= read)
        {
            stream.segments.pop();
        }
        if !stream.at_boundary() {
            stream.text.push_str("\n\n");
        }
    }

    /// A blank between words, written as bytes `source`.
    fn blank(&mut self, source: Range<usize>) {
        let stream = self.stream();
        if !stream.at_boundary() && !stream.text.ends_with(' ') {
            stream.push(" ", source, Shows::Decoded);
        }
    }

    /// A word in place of bytes `source`, which are no text but stand
    /// in a sentence: `Dummy1`, as checkers of grammar are used to; or
    /// `Ina1` behind "an", which wants a vowel.
    fn stand_in(&mut self, source: Range<usize>) {
        self.stand_ins += 1;
        let count = self.stand_ins;
        let stream = self.stream();
        let before = stream.text.trim_end_matches(' ');
        let vowel = before.ends_with(" an") || before.ends_with(" An") || before == "An";
        let word = if vowel { "Ina" } else { "Dummy" };
        stream.push(&format!("{word}{count}"), source, Shows::Stands);
    }

    /// The nodes of a piece of markup, the first at byte `at`.
    fn markup(&mut self, nodes: &[SyntaxNode], mut at: usize) {
        let mut nodes = nodes.iter();
        while let Some(node) = nodes.next() {
            if node.kind() == SyntaxKind::Hash {
                at += node.len();
                if let Some(code) = nodes.next() {
                    self.code(code, at);
                    at += code.len();
                }
                continue;
            }
            self.element(node, at);
            at += node.len();
        }
    }

    /// One node of markup, at byte `at`.
    fn element(&mut self, node: &SyntaxNode, at: usize) {
        let source = at..at + node.len();
        match node.kind() {
            SyntaxKind::Text | SyntaxKind::SmartQuote | SyntaxKind::Colon => {
                self.stream().push(node.leaf_text(), source, Shows::Exact);
            }
            SyntaxKind::Space | SyntaxKind::Linebreak => self.blank(source),
            SyntaxKind::Parbreak => self.boundary(),
            SyntaxKind::Escape => {
                if let Some(escape) = node.cast::<ast::Escape>() {
                    let shown = escape.get().to_string();
                    self.stream().push(&shown, source, Shows::Decoded);
                }
            }
            SyntaxKind::Shorthand => {
                if let Some(shorthand) = node.cast::<ast::Shorthand>() {
                    match shorthand.get() {
                        '\u{a0}' => self.blank(source),
                        shown => self
                            .stream()
                            .push(&shown.to_string(), source, Shows::Decoded),
                    }
                }
            }
            SyntaxKind::Strong | SyntaxKind::Emph | SyntaxKind::Markup => {
                self.markup(node.children().as_slice(), at);
            }
            SyntaxKind::Heading
            | SyntaxKind::ListItem
            | SyntaxKind::EnumItem
            | SyntaxKind::TermItem => {
                self.boundary();
                self.markup(node.children().as_slice(), at);
                self.boundary();
            }
            SyntaxKind::Equation | SyntaxKind::Ref | SyntaxKind::Link => self.stand_in(source),
            SyntaxKind::Raw => {
                if node.full_text().contains('\n') {
                    self.boundary();
                } else {
                    self.stand_in(source);
                }
            }
            // A label, a comment, the marks of markup: nothing is read.
            _ => {}
        }
    }

    /// What follows a `#` in markup, at byte `at`.
    fn code(&mut self, node: &SyntaxNode, at: usize) {
        let source = at..at + node.len();
        match node.kind() {
            SyntaxKind::ContentBlock => self.content(node, at, true),
            SyntaxKind::FuncCall => {
                let name = node.children().next().map(SyntaxNode::full_text);
                let name = name.as_ref().map_or("", |name| name.as_str());
                let inline = INLINE.contains(&name);
                if !inline && !SILENT.contains(&name) {
                    self.stand_in(source);
                }
                self.blocks_in(node, at, inline);
            }
            // A name, a number: what it stands for is shown.
            SyntaxKind::Ident
            | SyntaxKind::FieldAccess
            | SyntaxKind::Str
            | SyntaxKind::Int
            | SyntaxKind::Float => self.stand_in(source),
            // A rule, a binding, a loop: what text they hold is read by
            // itself.
            _ => self.blocks_in(node, at, false),
        }
    }

    /// The content written between square brackets in code `node`,
    /// which is at byte `at`: part of the sentence if `inline`.
    fn blocks_in(&mut self, node: &SyntaxNode, mut at: usize, inline: bool) {
        for child in node.children() {
            match child.kind() {
                SyntaxKind::ContentBlock => self.content(child, at, inline),
                SyntaxKind::Equation | SyntaxKind::Raw | SyntaxKind::Str => {}
                _ => self.blocks_in(child, at, inline),
            }
            at += child.len();
        }
    }

    /// Content between square brackets, at byte `at`.
    fn content(&mut self, block: &SyntaxNode, mut at: usize, inline: bool) {
        for child in block.children() {
            if child.kind() == SyntaxKind::Markup {
                if has_paragraphs(child) {
                    // Checked by itself: see `units`.
                } else if inline {
                    self.markup(child.children().as_slice(), at);
                } else {
                    self.streams.push(Prose::default());
                    self.open.push(self.streams.len() - 1);
                    self.markup(child.children().as_slice(), at);
                    self.open.pop();
                }
            }
            at += child.len();
        }
    }
}

/// Whether `markup` is more than one paragraph.
fn has_paragraphs(markup: &SyntaxNode) -> bool {
    let mut children = markup.children();
    children.any(|child| child.kind() == SyntaxKind::Parbreak)
}

/// A paragraph of the source: what is checked, and looked up, as one.
///
/// Mostly what stands between two empty lines. Content of several
/// paragraphs written inside a call (the body handed to a template) is
/// units of its own, and not part of the unit the call is in: one
/// paragraph of it changing must not cost the checking of all of it.
#[derive(Debug, Clone)]
pub struct Unit<'a> {
    /// The bytes of the source it is.
    pub range: Range<usize>,
    nodes: &'a [SyntaxNode],
    /// The parts of it that are units of their own, in order.
    holes: Vec<Range<usize>>,
}

impl Unit<'_> {
    /// Its text, as what it is looked up by: `text` is all of the
    /// source. Two units with the same key are read the same.
    #[must_use]
    pub fn key<'t>(&self, text: &'t str) -> Cow<'t, str> {
        let whole = text.get(self.range.clone()).unwrap_or("");
        if self.holes.is_empty() {
            return Cow::Borrowed(whole);
        }
        let mut key = String::new();
        let mut from = self.range.start;
        for hole in &self.holes {
            key.push_str(text.get(from..hole.start).unwrap_or(""));
            key.push(HOLE);
            from = hole.end;
        }
        key.push_str(text.get(from..self.range.end).unwrap_or(""));
        Cow::Owned(key)
    }

    /// Where byte `at` of the source is in the key.
    #[must_use]
    pub fn in_key(&self, at: usize) -> usize {
        let mut cut = 0;
        for hole in &self.holes {
            if at >= hole.end {
                cut += hole.len() - HOLE.len_utf8();
            } else if at > hole.start {
                return hole.start - self.range.start - cut;
            }
        }
        at.saturating_sub(self.range.start + cut)
    }

    /// Where byte `at` of the key is in the source.
    #[must_use]
    pub fn in_source(&self, at: usize) -> usize {
        let mut place = self.range.start + at;
        for hole in &self.holes {
            if place > hole.start {
                place += hole.len() - HOLE.len_utf8();
            }
        }
        place
    }

    /// Its running text.
    #[must_use]
    pub fn prose(&self) -> Prose {
        Prose::of(self.nodes, self.range.start)
    }
}

/// The units of a source whose syntax tree is `root`, in no particular
/// order.
#[must_use]
pub fn units(root: &SyntaxNode) -> Vec<Unit<'_>> {
    let mut found = Vec::new();
    units_of(root, 0, &mut found);
    found
}

/// Adds the units of `markup`, which is at byte `at`, to `found`.
fn units_of<'a>(markup: &'a SyntaxNode, at: usize, found: &mut Vec<Unit<'a>>) {
    let nodes = markup.children().as_slice();
    let mut offset = at;
    // The paragraph being gone through: its first node and where that is.
    let mut begun: Option<(usize, usize)> = None;
    let mut holes = Vec::new();
    for (index, node) in nodes.iter().enumerate() {
        let kind = node.kind();
        let ends = kind == SyntaxKind::Parbreak;
        if !ends && kind != SyntaxKind::Space && begun.is_none() {
            begun = Some((index, offset));
        }
        if ends && let Some((first, start)) = begun.take() {
            found.push(Unit {
                range: start..offset,
                nodes: &nodes[first..index],
                holes: std::mem::take(&mut holes),
            });
        }
        if !ends && node.children().len() > 0 {
            holes_in(node, offset, &mut holes, found);
        }
        offset += node.len();
    }
    if let Some((first, start)) = begun {
        found.push(Unit {
            range: start..offset,
            nodes: &nodes[first..],
            holes,
        });
    }
}

/// Looks through `node`, which is at byte `at`, for content of several
/// paragraphs: where each is goes to `holes`, its units to `found`.
fn holes_in<'a>(
    node: &'a SyntaxNode,
    mut at: usize,
    holes: &mut Vec<Range<usize>>,
    found: &mut Vec<Unit<'a>>,
) {
    let within_block = node.kind() == SyntaxKind::ContentBlock;
    for child in node.children() {
        match child.kind() {
            SyntaxKind::Equation | SyntaxKind::Raw => {}
            SyntaxKind::Markup if within_block && has_paragraphs(child) => {
                holes.push(at..at + child.len());
                units_of(child, at, found);
            }
            _ if child.children().len() > 0 => holes_in(child, at, holes, found),
            _ => {}
        }
        at += child.len();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use typst_syntax::Source;

    use super::*;

    /// The running text of `text`, read as one piece of markup.
    fn read(text: &str) -> (Source, Prose) {
        let source = Source::detached(text);
        let prose = Prose::of(source.root().children().as_slice(), 0);
        (source, prose)
    }

    /// What `text` is read as.
    fn prose(text: &str) -> String {
        read(text).1.text().to_owned()
    }

    /// The source that `word` of what `text` is read as is written by.
    fn written(text: &str, word: &str) -> String {
        let (source, prose) = read(text);
        let at = prose.text().find(word).unwrap();
        let range = prose.source(&(at..at + word.len())).unwrap();
        source.text()[range].to_owned()
    }

    #[test]
    fn markup_is_read_as_the_words_it_shows() {
        assert_eq!(
            prose("Hello *bold* and _slanted_ world."),
            "Hello bold and slanted world."
        );
        assert_eq!(prose("one\ntwo  three"), "one two three");
        assert_eq!(prose("don't \"quote\" it"), "don't \"quote\" it");
        assert_eq!(prose("a // not this\nb"), "a b");
        assert_eq!(prose("a /* nor this */ b"), "a b");
        assert_eq!(prose("Name <label> goes on"), "Name goes on");
        assert_eq!(prose("5 \\$ and A~B and x -- y"), "5 $ and A B and x – y");
    }

    #[test]
    fn what_is_no_text_is_a_word_that_stands_for_it() {
        assert_eq!(
            prose("Let $x$ be as in @eq."),
            "Let Dummy1 be as in Dummy2."
        );
        assert_eq!(prose("This is an $n$-gon."), "This is an Ina1-gon.");
        assert_eq!(prose("See https://example.org now."), "See Dummy1 now.");
        assert_eq!(prose("Call `main` then."), "Call Dummy1 then.");
        assert_eq!(
            prose("It is #calc.pi here, #x there."),
            "It is Dummy1 here, Dummy2 there."
        );
        assert_eq!(
            prose("Before\n```rust\nlet x = 1;\n```\nafter"),
            "Before\n\nafter"
        );
    }

    #[test]
    fn headings_lists_and_terms_are_paragraphs_of_their_own() {
        assert_eq!(prose("= A Title\nText below."), "A Title\n\nText below.");
        assert_eq!(
            prose("- one thing\n- another\n+ third"),
            "one thing\n\nanother\n\nthird"
        );
        assert_eq!(prose("/ Term: What it is."), "Term: What it is.");
    }

    #[test]
    fn code_is_left_out_and_content_in_it_is_read() {
        assert_eq!(prose("#set text(lang: \"de\", font: \"Libertinus\")"), "");
        assert_eq!(prose("#let wrong = \"speling\""), "");
        assert_eq!(prose("#import \"@preview/thing:1.0.0\": stuff"), "");
        assert_eq!(
            prose("A #emph[fine] day, #text(red)[really]."),
            "A fine day, really."
        );
        assert_eq!(
            prose("#link(\"https://x.y\")[the site] is up"),
            "the site is up"
        );
        // A footnote is not part of the sentence it is written in.
        assert_eq!(
            prose("An end#footnote[Said in passing.] of it."),
            "An end of it.\n\nSaid in passing."
        );
        assert_eq!(
            prose("#figure(image(\"plot.png\"), caption: [A plot of it.])"),
            "Dummy1\n\nA plot of it."
        );
        assert_eq!(
            prose("#table(columns: 2, [left], [right])"),
            "Dummy1\n\nleft\n\nright"
        );
    }

    #[test]
    fn what_is_read_knows_where_it_is_written() {
        assert_eq!(written("Hello *bold* world", "bold"), "bold");
        assert_eq!(written("so they don't go", "don't"), "don't");
        assert_eq!(
            written("a hyphen-?ated word", "hyphen\u{ad}ated"),
            "hyphen-?ated"
        );
        assert_eq!(written("#figure(caption: [A plott.])", "plott"), "plott");
        let (_, prose) = read("Let $x$ be.");
        let at = prose.text().find("Dummy1").unwrap();
        assert!(prose.stands_in(&(at..at + 6)));
        assert!(!prose.stands_in(&(0..3)));
        assert_eq!(prose.source(&(at..at + 6)), Some(4..7));
        // What sets paragraphs apart is written nowhere.
        let (_, prose) = read("= Title\nText");
        assert_eq!(prose.source(&(5..7)), None);
    }

    /// The units of `text`, as the source they are.
    fn unit_texts(text: &str) -> Vec<String> {
        let source = Source::detached(text);
        let mut found: Vec<String> = units(source.root())
            .iter()
            .map(|unit| text[unit.range.clone()].to_owned())
            .collect();
        found.sort();
        found
    }

    #[test]
    fn units_are_paragraphs() {
        assert_eq!(
            unit_texts("one\nstill one\n\ntwo\n\n\n\nthree\n"),
            ["one\nstill one", "three\n", "two"]
        );
        assert_eq!(unit_texts("\n\n"), Vec::<String>::new());
        // Empty lines inside a call or raw text end nothing.
        let call = "#figure(\n  image(\"a.png\"),\n\n  caption: [It],\n)";
        assert_eq!(unit_texts(call), [call]);
        let raw = "```\na\n\nb\n```";
        assert_eq!(unit_texts(raw), [raw]);
    }

    #[test]
    fn a_body_of_paragraphs_in_a_call_is_units_of_its_own() {
        let text = "#paper(title: [On Things])[\n  First one.\n\n  Second one.\n]\n\nAfter it.";
        let source = Source::detached(text);
        let found = units(source.root());
        let mut read: Vec<String> = found
            .iter()
            .map(|unit| unit.prose().text().to_owned())
            .collect();
        read.sort();
        assert_eq!(
            read,
            [
                "After it.",
                "Dummy1\n\nOn Things",
                "First one.",
                "Second one."
            ]
        );
        // The call is looked up by what it is without its body, so a
        // change to the body does not change it.
        let shell = found.iter().find(|unit| unit.range.start == 0).unwrap();
        let key = shell.key(text);
        assert_eq!(key, "#paper(title: [On Things])[\u{1}]");
        let other = text.replace("Second", "Another");
        let other_source = Source::detached(other.as_str());
        let other_units = units(other_source.root());
        let other_shell = other_units
            .iter()
            .find(|unit| unit.range.start == 0)
            .unwrap();
        assert_eq!(other_shell.key(&other), key);
        // Places in the key are places in the source, and back.
        let title = text.find("Things").unwrap();
        assert_eq!(shell.in_source(shell.in_key(title)), title);
        let end = shell.range.end;
        assert_eq!(shell.in_key(end), key.len());
        assert_eq!(shell.in_source(key.len()), end);
        let inner = found
            .iter()
            .find(|unit| text[unit.range.clone()].starts_with("Second"))
            .unwrap();
        assert_eq!(
            inner.in_source(inner.in_key(inner.range.start + 3)),
            inner.range.start + 3
        );
    }
}
