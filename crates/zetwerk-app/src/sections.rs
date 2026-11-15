//! The sections of a document: what a heading has under it, and how a
//! section is moved among its neighbours, made deeper or shallower, or
//! taken out, without anyone looking at the marks that say so.
//!
//! A section is its heading and all that follows up to the next heading
//! of its level or above: its subsections are part of it and go where it
//! goes. Only headings that stand in the text itself count. One inside a
//! block (`#box[= x]`) belongs to that block, which a section could not
//! be cut out of; one made by a call (`#heading[x]`) has no level to be
//! read off it.
//!
//! Everything here works out edits and makes none, so that it is tested
//! as text.

use std::ops::Range;

use typst::syntax::{LinkedNode, Source, SyntaxKind};

/// How deep a heading may get here. Typst itself takes any number of
/// `=`; six levels are what HTML and tagged PDF tell apart, and more
/// than a reader does.
pub const DEEPEST: usize = 6;

/// A section of a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// How deep its heading is: the number of its `=`.
    pub level: usize,
    /// Its heading as written, without the `=` and without its label.
    pub title: String,
    /// The byte its heading's `=` begin at.
    pub heading: usize,
    /// The line its heading is on, counting from 1.
    pub line: usize,
    /// All of it: from the start of its heading's line to the next
    /// heading of its level or above, or to the end of the text.
    pub span: Range<usize>,
    /// The section it is a subsection of, as its place among all.
    pub parent: Option<usize>,
}

/// What can be done to a section that keeps it whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way {
    /// Before the section of its level that is before it.
    Up,
    /// Behind the section of its level that is behind it.
    Down,
    /// One level deeper, with all its subsections.
    Deeper,
    /// One level shallower, with all its subsections.
    Shallower,
}

impl Way {
    /// The way called `name` in commands: `up`, `down`, `deeper` or
    /// `shallower`.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        match name {
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            "deeper" => Some(Self::Deeper),
            "shallower" => Some(Self::Shallower),
            _ => None,
        }
    }

    /// What it is called in commands.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::Deeper => "deeper",
            Self::Shallower => "shallower",
        }
    }

    /// What has become of a section it was done to, for the status line.
    #[must_use]
    pub fn told(self) -> &'static str {
        match self {
            Self::Up => "the section is one further up",
            Self::Down => "the section is one further down",
            Self::Deeper => "the section is one level deeper",
            Self::Shallower => "the section is one level shallower",
        }
    }
}

/// A change to the text of a section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The edits that make it, each referring to the text as the ones
    /// before it left it.
    pub edits: Vec<(Range<usize>, String)>,
    /// Where the section begins once they are made; for one that was
    /// taken out, where the text has closed over it.
    pub start: usize,
}

/// The sections of `source`, in the order of the text.
#[must_use]
pub fn sections(source: &Source) -> Vec<Section> {
    let text = source.text();
    let mut found: Vec<Section> = Vec::new();
    let (mut line, mut counted) = (1, 0);
    for node in LinkedNode::new(source.root()).children() {
        if node.kind() != SyntaxKind::Heading {
            continue;
        }
        let range = node.range();
        let written = &text[range.clone()];
        let level = written.chars().take_while(|c| *c == '=').count();
        line += text[counted..range.start].matches('\n').count();
        counted = range.start;
        // With the blanks its line begins with, if that is all there is
        // before it.
        let line_start = text[..range.start].rfind('\n').map_or(0, |at| at + 1);
        let start = if text[line_start..range.start].trim().is_empty() {
            line_start
        } else {
            range.start
        };
        let parent = found.iter().rposition(|before| before.level < level);
        found.push(Section {
            level,
            title: written[level..].trim().to_owned(),
            heading: range.start,
            line,
            span: start..text.len(),
            parent,
        });
    }
    for index in 0..found.len() {
        let level = found[index].level;
        let next = found[index + 1..].iter().find(|next| next.level <= level);
        if let Some(end) = next.map(|next| next.span.start) {
            found[index].span.end = end;
        }
    }
    found
}

/// The innermost of `sections` that byte `at` is in: its place among
/// them. `None` before the first heading.
#[must_use]
pub fn at(sections: &[Section], at: usize) -> Option<usize> {
    sections
        .iter()
        .rposition(|section| section.span.start <= at)
}

/// How many of `sections` from `index` on are that section and what is
/// under it.
fn family(sections: &[Section], index: usize) -> usize {
    let Some(section) = sections.get(index) else {
        return 0;
    };
    let under = sections[index + 1..]
        .iter()
        .take_while(|below| below.level > section.level);
    1 + under.count()
}

/// The edits that take section `index` of `source` the `way` asked for.
///
/// A section moves past the one of its level that is next to it under
/// the same heading, and no further: it does not leave what it is a
/// subsection of. The blank lines between the two stay between them,
/// and those behind the pair stay behind it.
///
/// A level changes for the section and all under it. Labels stay where
/// they are: only the `=` of the headings are touched. A section made
/// shallower takes in what followed it at its old level, as headings
/// have it.
///
/// # Errors
///
/// If there is no such section, it has nowhere to go that way, it is of
/// the first level already, or something under it is as deep as
/// headings get. The error says which, for the status line.
pub fn changed(source: &Source, index: usize, way: Way) -> Result<Change, String> {
    let all = sections(source);
    let section = all.get(index).ok_or("the document has no such section")?;
    match way {
        Way::Up | Way::Down => {
            let down = way == Way::Down;
            let same_parent = |other: &&Section| other.parent == section.parent;
            let neighbour = if down {
                all[index + 1..].iter().find(same_parent)
            } else {
                all[..index].iter().rfind(same_parent)
            };
            // One of another level is no neighbour: past it, the section
            // would come under another heading, or it under the section.
            let Some(neighbour) = neighbour.filter(|other| other.level == section.level) else {
                return Err(format!(
                    "the section is the {} of its level here: it goes no further {}",
                    if down { "last" } else { "first" },
                    way.name()
                ));
            };
            let (first, second) = if down {
                (section, neighbour)
            } else {
                (neighbour, section)
            };
            Ok(swapped(source.text(), &first.span, &second.span, down))
        }
        Way::Deeper | Way::Shallower => {
            let deeper = way == Way::Deeper;
            let family = &all[index..index + family(&all, index)];
            if !deeper && section.level <= 1 {
                return Err("the section is of the first level: none is shallower".to_owned());
            }
            if deeper && family.iter().any(|member| member.level >= DEEPEST) {
                return Err(format!(
                    "a heading of it is {DEEPEST} levels deep: none goes deeper"
                ));
            }
            // From the last to the first, so that the places stay good.
            let edits = family.iter().rev().map(|member| {
                if deeper {
                    (member.heading..member.heading, "=".to_owned())
                } else {
                    (member.heading..member.heading + 1, String::new())
                }
            });
            Ok(Change {
                edits: edits.collect(),
                start: section.span.start,
            })
        }
    }
}

/// `text[range]` as what is written there and the blanks and line
/// breaks behind it.
fn body_and_gap<'a>(text: &'a str, range: &Range<usize>) -> (&'a str, &'a str) {
    let whole = &text[range.clone()];
    whole.split_at(whole.trim_end().len())
}

/// The edit that makes two spans of `text` that follow each other change
/// places, and where the one that was moved (the first, if it went
/// `down`) begins then.
fn swapped(text: &str, first: &Range<usize>, second: &Range<usize>, down: bool) -> Change {
    let (first_body, between) = body_and_gap(text, first);
    let (second_body, behind) = body_and_gap(text, second);
    // What begins a line goes on beginning one.
    let between = if between.contains('\n') {
        between
    } else {
        "\n\n"
    };
    let with = format!("{second_body}{between}{first_body}{behind}");
    let start = if down {
        first.start + second_body.len() + between.len()
    } else {
        first.start
    };
    Change {
        edits: vec![(first.start..second.end, with)],
        start,
    }
}

/// The edit that takes section `index` out of `source`, with all under
/// it. Taken from the end of the text, it takes the blank lines before
/// it along: the text ends as it ended.
///
/// # Errors
///
/// If there is no such section.
pub fn removed(source: &Source, index: usize) -> Result<Change, String> {
    let text = source.text();
    let all = sections(source);
    let section = all.get(index).ok_or("the document has no such section")?;
    let span = section.span.clone();
    if span.end < text.len() {
        return Ok(Change {
            edits: vec![(span.clone(), String::new())],
            start: span.start,
        });
    }
    let before = text[..span.start].trim_end();
    let (_, behind) = body_and_gap(text, &span);
    let with = if before.is_empty() { "" } else { behind };
    Ok(Change {
        edits: vec![(before.len()..span.end, with.to_owned())],
        start: before.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAPER: &str = "\
#set text(size: 11pt)

Before everything.

= Introduction <intro>
Why.

== Aim
What for.

== Scope <scope>
How far.

=== Limits
Not this.

= Methods
How.

= Results
What.
";

    fn source(text: &str) -> Source {
        Source::detached(text)
    }

    /// The place of the section called `title` among those of `text`.
    fn named(text: &str, title: &str) -> usize {
        sections(&source(text))
            .iter()
            .position(|section| section.title == title)
            .unwrap_or_else(|| panic!("no section {title} in {text:?}"))
    }

    /// `text` after `change`.
    fn made(text: &str, change: &Change) -> String {
        let mut text = text.to_owned();
        for (range, with) in &change.edits {
            text.replace_range(range.clone(), with);
        }
        text
    }

    /// `text` with its section `title` taken `way`; the error if that
    /// cannot be. The section has to begin where the change says.
    fn taken(text: &str, title: &str, way: Way) -> Result<String, String> {
        let change = changed(&source(text), named(text, title), way)?;
        let after = made(text, &change);
        let now = sections(&source(&after));
        let there = at(&now, change.start).map(|index| now[index].title.as_str());
        assert_eq!(there, Some(title), "{after:?}");
        Ok(after)
    }

    /// The headings of `text` as written, one after the other.
    fn outline(text: &str) -> String {
        let headings: Vec<String> = sections(&source(text))
            .iter()
            .map(|section| format!("{} {}", "=".repeat(section.level), section.title))
            .collect();
        headings.join(" | ")
    }

    #[test]
    fn a_section_spans_its_subsections_up_to_the_next_of_its_level() {
        let all = sections(&source(PAPER));
        let spanned = |title: &str| {
            let section = &all[named(PAPER, title)];
            &PAPER[section.span.clone()]
        };
        assert_eq!(
            outline(PAPER),
            "= Introduction | == Aim | == Scope | === Limits | = Methods | = Results"
        );
        assert!(spanned("Introduction").starts_with("= Introduction <intro>\nWhy."));
        assert!(spanned("Introduction").ends_with("Not this.\n\n"));
        assert_eq!(spanned("Aim"), "== Aim\nWhat for.\n\n");
        assert_eq!(
            spanned("Scope"),
            "== Scope <scope>\nHow far.\n\n=== Limits\nNot this.\n\n"
        );
        assert_eq!(spanned("Results"), "= Results\nWhat.\n");
        // Who is under whom, and on which line.
        let parents: Vec<Option<usize>> = all.iter().map(|section| section.parent).collect();
        assert_eq!(parents, [None, Some(0), Some(0), Some(2), None, None]);
        let lines: Vec<usize> = all.iter().map(|section| section.line).collect();
        assert_eq!(lines, [5, 8, 11, 14, 17, 20]);
        // The innermost section a place is in; none before the first.
        assert_eq!(at(&all, 0), None);
        let limits = PAPER.find("Not this").unwrap_or_default();
        assert_eq!(at(&all, limits), Some(named(PAPER, "Limits")));
        let how = PAPER.find("How far").unwrap_or_default();
        assert_eq!(at(&all, how), Some(named(PAPER, "Scope")));
    }

    #[test]
    fn only_headings_of_the_text_itself_are_sections() {
        let text = "= One\n#box[\n= Boxed\n]\nText with = in it.\n  = Two\n== = odd\n";
        assert_eq!(outline(text), "= One | = Two | == = odd");
        let all = sections(&source(text));
        // A heading's line is its own from its first blank on.
        assert_eq!(&text[all[1].span.clone()], "  = Two\n== = odd\n");
        assert_eq!(all[1].heading, all[1].span.start + 2);
        assert_eq!(sections(&source("No heading.\n")), []);
    }

    #[test]
    fn a_section_moves_past_its_neighbour_with_all_under_it() {
        let up = taken(PAPER, "Methods", Way::Up).unwrap_or_default();
        assert_eq!(
            outline(&up),
            "= Methods | = Introduction | == Aim | == Scope | === Limits | = Results"
        );
        assert!(up.starts_with("#set text(size: 11pt)\n\nBefore everything.\n\n= Methods\nHow.\n\n= Introduction <intro>\nWhy.\n\n== Aim"));
        assert!(up.ends_with("Not this.\n\n= Results\nWhat.\n"));
        // Down past it again is the text as it was.
        assert_eq!(taken(&up, "Methods", Way::Down).as_deref(), Ok(PAPER));
        // Subsections move among those of their section, labels and all.
        let scope = taken(PAPER, "Scope", Way::Up).unwrap_or_default();
        assert_eq!(
            outline(&scope),
            "= Introduction | == Scope | === Limits | == Aim | = Methods | = Results"
        );
        assert!(scope.contains("Why.\n\n== Scope <scope>\nHow far.\n\n=== Limits\nNot this.\n\n== Aim\nWhat for.\n\n= Methods"));
        assert_eq!(
            taken(PAPER, "Aim", Way::Down).as_deref(),
            Ok(scope.as_str())
        );
    }

    #[test]
    fn a_section_does_not_leave_what_it_is_under() {
        let no = |title: &str, way: Way| taken(PAPER, title, way).err().unwrap_or_default();
        assert!(no("Introduction", Way::Up).contains("first of its level"));
        assert!(no("Results", Way::Down).contains("last of its level"));
        assert!(no("Aim", Way::Up).contains("first"));
        assert!(no("Scope", Way::Down).contains("last"));
        assert!(no("Limits", Way::Up).contains("first"));
        assert!(no("Limits", Way::Down).contains("last"));
        // A deeper heading before one of the same section is no
        // neighbour of it: past it, one would come under the other.
        let skipped = "= A\n=== Deep\n== Flat\n== Flatter\n";
        assert!(taken(skipped, "Flat", Way::Up).is_err());
        assert!(taken(skipped, "Deep", Way::Down).is_err());
        assert_eq!(
            taken(skipped, "Flat", Way::Down).as_deref(),
            Ok("= A\n=== Deep\n== Flatter\n== Flat\n")
        );
        assert!(changed(&source(PAPER), 99, Way::Up).is_err());
    }

    #[test]
    fn blank_lines_stay_where_they_were_when_sections_change_places() {
        // The last section may end without a line break: so does the
        // text then, whichever section is last.
        let text = "= A\na\n\n\n= B\nb";
        assert_eq!(
            taken(text, "B", Way::Up).as_deref(),
            Ok("= B\nb\n\n\n= A\na")
        );
        assert_eq!(
            taken(text, "A", Way::Down).as_deref(),
            Ok("= B\nb\n\n\n= A\na")
        );
        // Sections that follow each other closely go on doing so.
        let close = "= A\na\n= B\nb\n\n= C\n";
        assert_eq!(
            taken(close, "A", Way::Down).as_deref(),
            Ok("= B\nb\n= A\na\n\n= C\n")
        );
        assert_eq!(
            taken(close, "C", Way::Up).as_deref(),
            Ok("= A\na\n= C\n\n= B\nb\n")
        );
        // A heading behind a comment in its line gets a line of its own.
        let odd = "= A\na\n/* x */ = B\nb\n";
        assert_eq!(
            taken(odd, "B", Way::Up).as_deref(),
            Ok("= B\nb\n\n= A\na\n/* x */\n")
        );
    }

    #[test]
    fn a_level_changes_for_a_section_and_all_under_it() {
        let deeper = taken(PAPER, "Scope", Way::Deeper).unwrap_or_default();
        assert_eq!(
            outline(&deeper),
            "= Introduction | == Aim | === Scope | ==== Limits | = Methods | = Results"
        );
        // Labels are where they were, and so is everything else.
        assert!(
            deeper.contains("=== Scope <scope>\nHow far.\n\n==== Limits\nNot this.\n\n= Methods")
        );
        assert_eq!(
            taken(&deeper, "Scope", Way::Shallower).as_deref(),
            Ok(PAPER)
        );
        let shallower = taken(PAPER, "Scope", Way::Shallower).unwrap_or_default();
        assert_eq!(
            outline(&shallower),
            "= Introduction | == Aim | = Scope | == Limits | = Methods | = Results"
        );
        assert!(shallower.contains("\n= Scope <scope>\n"));
        let all = taken(PAPER, "Introduction", Way::Deeper).unwrap_or_default();
        assert_eq!(
            outline(&all),
            "== Introduction | === Aim | === Scope | ==== Limits | = Methods | = Results"
        );
        assert!(all.contains("== Introduction <intro>\nWhy."));
    }

    #[test]
    fn levels_stay_between_the_first_and_the_deepest() {
        let no =
            |text: &str, title: &str, way: Way| taken(text, title, way).err().unwrap_or_default();
        assert!(no(PAPER, "Introduction", Way::Shallower).contains("first level"));
        assert!(no(PAPER, "Methods", Way::Shallower).contains("first level"));
        // Six deep is as deep as it gets, for what is under it too.
        let deep = "= A\n== B\n====== F\n";
        assert!(no(deep, "F", Way::Deeper).contains("6 levels"));
        assert!(no(deep, "B", Way::Deeper).contains("6 levels"));
        assert!(no(deep, "A", Way::Deeper).contains("6 levels"));
        assert_eq!(
            taken(deep, "F", Way::Shallower).as_deref(),
            Ok("= A\n== B\n===== F\n")
        );
        let room = "= A\n== B\n===== E\n";
        assert_eq!(
            taken(room, "A", Way::Deeper).as_deref(),
            Ok("== A\n=== B\n====== E\n")
        );
    }

    #[test]
    fn a_section_is_taken_out_with_all_under_it() {
        let without = |text: &str, title: &str| {
            let change =
                removed(&source(text), named(text, title)).unwrap_or_else(|why| panic!("{why}"));
            (made(text, &change), change.start)
        };
        let (text, start) = without(PAPER, "Scope");
        assert_eq!(
            outline(&text),
            "= Introduction | == Aim | = Methods | = Results"
        );
        assert!(text.contains("What for.\n\n= Methods\nHow."));
        assert!(text[start..].starts_with("= Methods"));
        // From the end, the text ends as it did.
        let (text, start) = without(PAPER, "Results");
        assert!(text.ends_with("= Methods\nHow.\n"));
        assert_eq!(start, text.len() - 1);
        assert_eq!(without("= A\na\n\n= B\nb", "B").0, "= A\na");
        assert_eq!(without("\n= A\na\n", "A").0, "");
        assert_eq!(without("Text.\n\n= A\n", "A").0, "Text.\n");
        assert!(removed(&source(PAPER), 99).is_err());
    }

    #[test]
    fn ways_are_called_what_commands_call_them() {
        for way in [Way::Up, Way::Down, Way::Deeper, Way::Shallower] {
            assert_eq!(Way::named(way.name()), Some(way));
            assert!(way.told().starts_with("the section is"));
        }
        assert_eq!(Way::named("sideways"), None);
    }
}
