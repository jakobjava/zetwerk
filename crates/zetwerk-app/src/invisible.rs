//! What the text has that the page shows as nothing.
//!
//! A label, a comment, a forced page or column break, a space no line
//! may be broken at, a call that only makes room (`#v`, `#h`), a rule
//! set in the middle of the text and the `#include` of another file (whose
//! text shows, but not where it comes from) are all in the text and
//! nowhere on the page: there they can be neither seen nor reached. This finds them,
//! says what each is to be called beside the page, and which shown text
//! each goes with, so that the window can put a tag beside the line of
//! that text.
//!
//! All of it goes by the syntax tree of the text as it is written.

use std::ops::Range;

use typst::syntax::{LinkedNode, Source, SyntaxKind};

/// How many bytes at the end (or the beginning) of the shown text next
/// to a thing are looked for on the page.
const WINDOW: usize = 32;
/// How many characters of a comment or a rule a tag shows.
const SHOWN: usize = 24;

/// What a thing that shows as nothing is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// `<name>`: what references refer to.
    Label,
    /// `// …` or `/* … */`.
    Comment,
    /// `#pagebreak()`.
    PageBreak,
    /// `#colbreak()`.
    ColumnBreak,
    /// `~`, or the character itself: a blank no line is broken at.
    NoBreak,
    /// `#v(…)` or `#h(…)`: room and nothing in it.
    Spacing,
    /// `#set`, `#show`, `#let` or `#import` behind the first text.
    Rule,
    /// `#include "…"`: the text of another file put here.
    Include,
}

impl Kind {
    /// What the user calls it.
    #[must_use]
    pub fn called(self) -> &'static str {
        match self {
            Self::Label => "label",
            Self::Comment => "comment",
            Self::PageBreak => "page break",
            Self::ColumnBreak => "column break",
            Self::NoBreak => "no-break space",
            Self::Spacing => "spacing",
            Self::Rule => "rule",
            Self::Include => "include",
        }
    }
}

/// One thing in the text that the page shows as nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thing {
    /// Its bytes.
    pub range: Range<usize>,
    pub kind: Kind,
    /// What its tag beside the page says.
    pub name: String,
    /// The last bytes of the shown text nearest before it, and the first
    /// bytes of the nearest after it; `None` where there is none.
    pub before: Option<Range<usize>>,
    pub after: Option<Range<usize>>,
    /// Whether it goes with the text before it (a label with what it
    /// labels, a break with where it breaks) and not with what follows
    /// (a comment on a line of its own, a rule).
    pub with_before: bool,
}

/// The things of a text that show as nothing, and the stretches of it
/// that show.
#[derive(Debug, Default)]
pub struct Found {
    /// In the order of the text.
    pub things: Vec<Thing>,
    /// In the order of the text, none in another.
    shown: Vec<Range<usize>>,
    /// The code that shows text of its own (a call with content in
    /// brackets).
    calls: Vec<Range<usize>>,
}

impl Found {
    /// The last bytes of the shown text nearest before `range`, and the
    /// first bytes of the nearest after it.
    #[must_use]
    pub fn beside(&self, range: &Range<usize>) -> (Option<Range<usize>>, Option<Range<usize>>) {
        let behind = self.shown.partition_point(|shown| shown.end <= range.start);
        let before = behind
            .checked_sub(1)
            .and_then(|at| self.shown.get(at))
            .map(|shown| shown.end - (shown.end - shown.start).min(WINDOW)..shown.end);
        let ahead = self.shown.partition_point(|shown| shown.start < range.end);
        let after = self
            .shown
            .get(ahead)
            .map(|shown| shown.start..shown.start + (shown.end - shown.start).min(WINDOW));
        (before, after)
    }

    /// Whether what is at `range` (where the compiler complains) goes
    /// with the shown text before it: not where it is in code that shows
    /// text of its own, which is then the text `after` it.
    #[must_use]
    pub fn goes_before(&self, range: &Range<usize>, after: Option<&Range<usize>>) -> bool {
        let Some(after) = after else {
            return true;
        };
        !self
            .calls
            .iter()
            .any(|call| call.contains(&range.start) && call.contains(&after.start))
    }

    /// The thing whose bytes begin at `start`.
    #[must_use]
    pub fn at(&self, start: usize) -> Option<&Thing> {
        self.things.iter().find(|thing| thing.range.start == start)
    }
}

/// Finds what shows as nothing in `source`.
#[must_use]
pub fn find(source: &Source) -> Found {
    let text = source.text();
    let mut walk = Walk {
        text,
        found: Vec::new(),
        shown: Vec::new(),
        calls: Vec::new(),
    };
    walk.markup(&LinkedNode::new(source.root()));
    let mut found = Found {
        things: Vec::new(),
        shown: walk.shown,
        calls: walk.calls,
    };
    found.shown.sort_by_key(|shown| shown.start);
    let things: Vec<Thing> = walk
        .found
        .into_iter()
        .map(|(range, kind, name)| {
            let (before, after) = found.beside(&range);
            // What stands behind text on its line goes with that text; what
            // stands on a line of its own goes with what it comes before.
            let behind_text = before
                .as_ref()
                .is_some_and(|before| !text[before.end..range.start].contains('\n'));
            let with_before = match kind {
                Kind::Comment | Kind::Rule => behind_text || after.is_none(),
                _ => before.is_some() || after.is_none(),
            };
            Thing {
                range,
                kind,
                name,
                before,
                after,
                with_before,
            }
        })
        .collect();
    found.things = things;
    found.things.sort_by_key(|thing| thing.range.start);
    found
}

/// What the walk through the syntax tree has found so far.
struct Walk<'a> {
    text: &'a str,
    found: Vec<(Range<usize>, Kind, String)>,
    shown: Vec<Range<usize>>,
    calls: Vec<Range<usize>>,
}

impl Walk<'_> {
    /// The children of a piece of markup.
    fn markup(&mut self, node: &LinkedNode) {
        let children: Vec<LinkedNode> = node.children().collect();
        let mut index = 0;
        while let Some(child) = children.get(index) {
            index += 1;
            // Code in markup is a hash and what follows it.
            if child.kind() == SyntaxKind::Hash
                && let Some(code) = children.get(index)
            {
                index += 1;
                self.code(child.offset(), code);
                continue;
            }
            self.piece(child);
        }
    }

    /// A child of a piece of markup that is not code.
    fn piece(&mut self, node: &LinkedNode) {
        let range = node.range();
        match node.kind() {
            SyntaxKind::Label => {
                let name = self.text[range.clone()]
                    .trim_start_matches('<')
                    .trim_end_matches('>');
                self.found.push((range, Kind::Label, name.to_owned()));
            }
            SyntaxKind::LineComment | SyntaxKind::BlockComment => {
                let name = match comment_text(&self.text[range.clone()]).as_str() {
                    "" => "comment".to_owned(),
                    words => format!("\u{201c}{}\u{201d}", cut(words, SHOWN)),
                };
                self.found.push((range, Kind::Comment, name));
            }
            SyntaxKind::Shorthand if &self.text[range.clone()] == "~" => {
                self.found
                    .push((range, Kind::NoBreak, "no break".to_owned()));
            }
            SyntaxKind::Text => self.text(range),
            SyntaxKind::Heading
            | SyntaxKind::Strong
            | SyntaxKind::Emph
            | SyntaxKind::ListItem
            | SyntaxKind::EnumItem
            | SyntaxKind::TermItem => {
                for child in node.children() {
                    if child.kind() == SyntaxKind::Markup {
                        self.markup(&child);
                    }
                }
            }
            SyntaxKind::Markup => self.markup(node),
            // Nothing shows of these that the cursor could be on.
            SyntaxKind::Space
            | SyntaxKind::Parbreak
            | SyntaxKind::Linebreak
            | SyntaxKind::Error => {}
            _ => self.shown.push(range),
        }
    }

    /// Plain text: shown, but for the blanks in it no line is broken at.
    fn text(&mut self, range: Range<usize>) {
        let mut from = range.start;
        for (at, _) in self.text[range.clone()].match_indices('\u{a0}') {
            let at = range.start + at;
            if at > from {
                self.shown.push(from..at);
            }
            let end = at + '\u{a0}'.len_utf8();
            self.found
                .push((at..end, Kind::NoBreak, "no break".to_owned()));
            from = end;
        }
        if range.end > from {
            self.shown.push(from..range.end);
        }
    }

    /// Code put into markup behind a hash at byte `hash`.
    fn code(&mut self, hash: usize, node: &LinkedNode) {
        let range = hash..node.range().end;
        let whole = &self.text[range.clone()];
        match node.kind() {
            SyntaxKind::FuncCall => {
                let callee = node.children().next();
                let name = callee
                    .filter(|callee| callee.kind() == SyntaxKind::Ident)
                    .map(|callee| self.text[callee.range()].to_owned());
                match name.as_deref() {
                    Some("pagebreak") => {
                        self.found
                            .push((range, Kind::PageBreak, "page break".to_owned()));
                    }
                    Some("colbreak") => {
                        self.found
                            .push((range, Kind::ColumnBreak, "column break".to_owned()));
                    }
                    Some(call @ ("v" | "h")) => {
                        let way = if call == "v" { '\u{2195}' } else { '\u{2194}' };
                        let amount = spacing_amount(whole).map_or("", |amount| &whole[amount]);
                        let name = format!("space {way} {}", cut(amount.trim(), SHOWN));
                        self.found.push((range, Kind::Spacing, name));
                    }
                    _ => self.shown_code(range, node),
                }
            }
            // Where another file's text is put is shown by none of it.
            SyntaxKind::ModuleInclude => {
                let path = node
                    .children()
                    .find(|child| child.kind() == SyntaxKind::Str)
                    .map_or("", |path| self.text[path.range()].trim_matches('"'));
                let path = path.rsplit('/').next().unwrap_or(path);
                let name = format!("include {}", cut(path, SHOWN));
                self.found.push((range, Kind::Include, name));
            }
            // Before the first text a rule says how all of it looks: that
            // is the document's settings, not something in the text.
            SyntaxKind::SetRule
            | SyntaxKind::ShowRule
            | SyntaxKind::LetBinding
            | SyntaxKind::ModuleImport => {
                if !self.shown.is_empty() {
                    self.found.push((range, Kind::Rule, rule_name(whole)));
                }
            }
            _ => self.shown_code(range, node),
        }
    }

    /// Code that may show something: what its content blocks hold, or
    /// else, as one, what it makes.
    fn shown_code(&mut self, range: Range<usize>, node: &LinkedNode) {
        let known = self.shown.len();
        self.contents(node);
        if self.shown.len() == known {
            self.shown.push(range);
        } else {
            self.calls.push(range);
        }
    }

    /// The content blocks in code, as markup.
    fn contents(&mut self, node: &LinkedNode) {
        for child in node.children() {
            match child.kind() {
                SyntaxKind::ContentBlock => {
                    for inner in child.children() {
                        if inner.kind() == SyntaxKind::Markup {
                            self.markup(&inner);
                        }
                    }
                }
                // What a function makes shows where it is called.
                SyntaxKind::Closure | SyntaxKind::LetBinding => {}
                _ => self.contents(&child),
            }
        }
    }
}

/// `text` cut to `most` characters, with an ellipsis if it was longer.
fn cut(text: &str, most: usize) -> String {
    let mut shown: String = text.chars().take(most).collect();
    if shown.len() < text.len() {
        shown.push('\u{2026}');
    }
    shown
}

/// What a comment says: without its marks, on one line.
fn comment_text(comment: &str) -> String {
    let inside = match comment.strip_prefix("/*") {
        Some(block) => block.strip_suffix("*/").unwrap_or(block),
        None => comment.trim_start_matches('/'),
    };
    inside.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What a rule is called: its keyword and what it is about, as far as
/// the first bracket, colon or equals sign.
fn rule_name(rule: &str) -> String {
    let rule = rule.trim_start_matches('#');
    let end = rule
        .find(['(', ':', '=', '[', '{', '\n'])
        .unwrap_or(rule.len());
    let words: Vec<&str> = rule[..end].split_whitespace().collect();
    cut(&words.join(" "), SHOWN)
}

/// The bytes of the amount in a call that makes room (`#v(1cm)`), as
/// bytes of `call`: what stands between its opening bracket and the
/// first comma or the closing bracket.
fn spacing_amount(call: &str) -> Option<Range<usize>> {
    let open = call.find('(')? + 1;
    let close = call[open..].find([',', ')'])? + open;
    Some(open..close)
}

/// What to take out of `text` to take `thing` out, and what to put in
/// its place: a line it stands on alone goes with it, and so do the
/// blanks it stands behind at the end of a line. A space no line is
/// broken at becomes a plain one.
#[must_use]
pub fn removal(text: &str, thing: &Thing) -> (Range<usize>, &'static str) {
    let range = thing.range.clone();
    if thing.kind == Kind::NoBreak {
        return (range, " ");
    }
    let line_start = text[..range.start].rfind('\n').map_or(0, |at| at + 1);
    let line_end = text[range.end..]
        .find('\n')
        .map_or(text.len(), |at| range.end + at);
    let blank = |part: &str| part.trim().is_empty();
    let alone_before = blank(&text[line_start..range.start]);
    let alone_after = blank(&text[range.end..line_end]);
    if alone_before && alone_after {
        return if line_end < text.len() {
            (line_start..line_end + 1, "")
        } else {
            (line_start.saturating_sub(1)..line_end, "")
        };
    }
    if alone_after {
        let start = text[..range.start].trim_end_matches([' ', '\t']).len();
        return (start..line_end, "");
    }
    if text[..range.start].ends_with(' ') && text[range.end..].starts_with(' ') {
        return (range.start - 1..range.end, "");
    }
    (range, "")
}

/// What of `thing` the user may type anew, as it is now: a label's name,
/// what a comment says, the amount of room made. `None` for the things
/// that are what they are.
#[must_use]
pub fn retypable(text: &str, thing: &Thing) -> Option<String> {
    let written = &text[thing.range.clone()];
    match thing.kind {
        Kind::Label => Some(thing.name.clone()),
        Kind::Comment => Some(comment_text(written)),
        Kind::Spacing => spacing_amount(written).map(|amount| written[amount].trim().to_owned()),
        _ => None,
    }
}

/// What to write in place of which bytes of `text` for `thing` to be
/// `typed`, or why that cannot be.
///
/// # Errors
///
/// Where `typed` is not what such a thing can be: a label of characters
/// no label has, a comment that would end early, an empty amount.
pub fn retyped(
    text: &str,
    thing: &Thing,
    typed: &str,
) -> Result<(Range<usize>, String), &'static str> {
    let typed = typed.trim();
    let written = &text[thing.range.clone()];
    match thing.kind {
        Kind::Label => {
            let fits = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':');
            if typed.is_empty() || !typed.chars().all(fits) {
                return Err("a label is letters, digits and _ - . : only");
            }
            Ok((thing.range.clone(), format!("<{typed}>")))
        }
        Kind::Comment if written.starts_with("/*") => {
            if typed.contains("*/") {
                return Err("a comment of this kind cannot say */");
            }
            Ok((thing.range.clone(), format!("/* {typed} */")))
        }
        Kind::Comment => Ok((thing.range.clone(), format!("// {typed}"))),
        Kind::Spacing => {
            let fine = !typed.is_empty() && !typed.contains([',', '(', ')', '\n']);
            match spacing_amount(written) {
                Some(amount) if fine => Ok((
                    thing.range.start + amount.start..thing.range.start + amount.end,
                    typed.to_owned(),
                )),
                _ => Err("an amount, please: 1cm, 2em, 1fr"),
            }
        }
        _ => Err("there is nothing to type for it"),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use super::*;

    fn found(text: &str) -> Found {
        find(&Source::detached(text))
    }

    /// What is found, as kinds and names.
    fn named(text: &str) -> Vec<(Kind, String)> {
        found(text)
            .things
            .into_iter()
            .map(|thing| (thing.kind, thing.name))
            .collect()
    }

    /// The text of the windows of the thing at `start`.
    fn windows(text: &str, start: &str) -> (Option<String>, Option<String>, bool) {
        let found = found(text);
        let at = text
            .find(start)
            .unwrap_or_else(|| panic!("{start} is in the text"));
        let thing = found.at(at).unwrap_or_else(|| panic!("a thing at {start}"));
        let of = |range: &Option<Range<usize>>| range.clone().map(|range| text[range].to_owned());
        (of(&thing.before), of(&thing.after), thing.with_before)
    }

    #[test]
    fn what_shows_as_nothing_is_found_and_named() {
        let text = "#set text(size: 11pt)\n\
                    // the beginning\n\
                    = Title <sec:intro>\n\
                    Some words~here.\n\
                    #pagebreak()\n\
                    /* a longer remark that goes on and on and on */\n\
                    #v(1cm)\n\
                    #h(2em, weak: true)\n\
                    #colbreak()\n\
                    #set par(justify: true)\n\
                    #show heading: it => it\n\
                    #let total = 3\n\
                    #include \"chapters/one.typ\"\n\
                    End\u{a0}here.\n";
        assert_eq!(
            named(text),
            [
                (Kind::Comment, "\u{201c}the beginning\u{201d}".to_owned()),
                (Kind::Label, "sec:intro".to_owned()),
                (Kind::NoBreak, "no break".to_owned()),
                (Kind::PageBreak, "page break".to_owned()),
                (
                    Kind::Comment,
                    "\u{201c}a longer remark that goe\u{2026}\u{201d}".to_owned()
                ),
                (Kind::Spacing, "space \u{2195} 1cm".to_owned()),
                (Kind::Spacing, "space \u{2194} 2em".to_owned()),
                (Kind::ColumnBreak, "column break".to_owned()),
                (Kind::Rule, "set par".to_owned()),
                (Kind::Rule, "show heading".to_owned()),
                (Kind::Rule, "let total".to_owned()),
                (Kind::Include, "include one.typ".to_owned()),
                (Kind::NoBreak, "no break".to_owned()),
            ]
        );
    }

    #[test]
    fn rules_before_the_text_are_the_documents_settings() {
        let text = "#set page(paper: \"a5\")\n#show: it => it\n#let x = 1\n\nText.\n";
        assert_eq!(named(text), []);
    }

    #[test]
    fn what_shows_inside_calls_and_blocks_is_looked_into() {
        let text = "#figure(image(\"a.png\"), caption: [A plot // of it\n]) <fig:plot>\n\
                    #block[Inside <inner>]\n";
        assert_eq!(
            named(text),
            [
                (Kind::Comment, "\u{201c}of it\u{201d}".to_owned()),
                (Kind::Label, "fig:plot".to_owned()),
                (Kind::Label, "inner".to_owned()),
            ]
        );
        // A label goes with what it labels: the caption.
        let (before, after, with_before) = windows(text, "<fig:plot>");
        assert_eq!(before.as_deref(), Some("A plot"));
        assert_eq!(after.as_deref(), Some("Inside"));
        assert!(with_before);
    }

    #[test]
    fn a_thing_goes_with_the_text_before_it_or_after_it() {
        let first = "First paragraph, which is long enough to be cut.";
        let text = format!(
            "{first}\n\n\
             // about the next one\n\
             = Next <next>\n\
             Behind words // a remark\n"
        );
        let text = text.as_str();
        let (before, after, with_before) = windows(text, "// about");
        assert_eq!(before.as_deref(), Some(&first[first.len() - WINDOW..]));
        assert_eq!(after.as_deref(), Some("Next"));
        assert!(
            !with_before,
            "a comment on a line of its own goes with what follows"
        );
        let (before, _, with_before) = windows(text, "<next>");
        assert_eq!(before.as_deref(), Some("Next"));
        assert!(with_before);
        let (before, after, with_before) = windows(text, "// a remark");
        assert_eq!(before.as_deref(), Some("Behind words"));
        assert_eq!(after, None);
        assert!(with_before, "a comment behind text goes with it");
    }

    #[test]
    fn a_complaint_in_code_goes_with_the_text_the_code_shows() {
        let text = "Plain.\n\n#text(font: \"X\")[Odd words.]\n#(1 +)\n";
        let found = found(text);
        let at = |part: &str| {
            let at = text.find(part).unwrap();
            at..at + part.len()
        };
        let font = at("\"X\"");
        let (_, after) = found.beside(&font);
        assert_eq!(after.clone().map(|after| &text[after]), Some("Odd words."));
        assert!(!found.goes_before(&font, after.as_ref()));
        let sum = at("+");
        let (_, after) = found.beside(&sum);
        assert!(found.goes_before(&sum, after.as_ref()));
    }

    #[test]
    fn taking_a_thing_out_takes_its_line_or_its_blank() {
        let text = "= Title <sec:intro>\nWords~more.\n// alone\nText\n#pagebreak()";
        let found = found(text);
        let out = |start: &str| {
            let at = text.find(start).unwrap_or_else(|| panic!("{start}"));
            let (range, with) = removal(text, found.at(at).unwrap());
            format!("{}{with}{}", &text[..range.start], &text[range.end..])
        };
        assert_eq!(
            out("<sec"),
            "= Title\nWords~more.\n// alone\nText\n#pagebreak()"
        );
        assert_eq!(
            out("~"),
            "= Title <sec:intro>\nWords more.\n// alone\nText\n#pagebreak()"
        );
        assert_eq!(
            out("// alone"),
            "= Title <sec:intro>\nWords~more.\nText\n#pagebreak()"
        );
        assert_eq!(
            out("#pagebreak"),
            "= Title <sec:intro>\nWords~more.\n// alone\nText"
        );
    }

    #[test]
    fn labels_comments_and_room_are_typed_anew() {
        let text = "= Title <sec:intro>\n// old words\n/* block */\nText\n#v(1cm, weak: true)\n";
        let found = found(text);
        let thing = |start: &str| found.at(text.find(start).unwrap()).unwrap();
        let anew = |start: &str, typed: &str| {
            retyped(text, thing(start), typed).map(|(range, with)| {
                format!("{}{with}{}", &text[..range.start], &text[range.end..])
            })
        };
        assert_eq!(retypable(text, thing("<sec")).as_deref(), Some("sec:intro"));
        assert_eq!(
            retypable(text, thing("// old")).as_deref(),
            Some("old words")
        );
        assert_eq!(retypable(text, thing("/*")).as_deref(), Some("block"));
        assert_eq!(retypable(text, thing("#v")).as_deref(), Some("1cm"));
        assert!(
            anew("<sec", "sec:start")
                .unwrap()
                .starts_with("= Title <sec:start>\n")
        );
        assert!(anew("<sec", "no spaces").is_err());
        assert!(
            anew("// old", "new words")
                .unwrap()
                .contains("\n// new words\n")
        );
        assert!(anew("/*", "other").unwrap().contains("\n/* other */\n"));
        assert!(anew("/*", "ends */ early").is_err());
        assert!(
            anew("#v", "2em")
                .unwrap()
                .ends_with("#v(2em, weak: true)\n")
        );
        assert!(anew("#v", "").is_err());
    }
}
