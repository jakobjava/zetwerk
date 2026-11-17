//! A document as a deck of slides: which pages make one slide, what the
//! speaker's notes are, and where each slide is written in the source.
//!
//! **Slides and pages.** Slide packages make each step of an animated
//! slide (`#pause`, `#uncover`, `#only`) a page of its own. A slide is
//! therefore a run of pages, found in this order:
//!
//! 1. *Markers.* touying and polylux put `<pdfpc>` metadata on every
//!    page for the pdfpc presenter: `(t: "LogicalSlide", v: 3)` says
//!    which slide the page belongs to, `(t: "Note", v: "…")` is a
//!    speaker's note. Pages with the same number in a row are one slide;
//!    a page without a marker is a slide by itself.
//! 2. *Page numbers.* Where there are no markers, pages in a row that
//!    carry the same page number (`counter(page)`, as decks that count
//!    slides keep it) are one slide.
//! 3. *Pages.* Otherwise every page is a slide.
//!
//! **Notes** are the `Note`s of the markers (touying's
//! `#speaker-note[…]` and polylux's `#pdfpc.speaker-note(…)` write
//! those, as long as touying's `enable-pdfpc` is left on), and for any
//! deck, with or without a package, metadata labelled `<speaker-note>`:
//! `#metadata("…") <speaker-note>`, `#metadata([…]) <speaker-note>` or
//! `#metadata((note: "…")) <speaker-note>`. A note belongs to the page
//! it is on.
//!
//! **Slides in the source.** To start a slide or move one, the source
//! has to say where slides are. Two ways are known: a top-level call per
//! slide (`#slide[…]`, `#focus-slide[…]`, `#polylux-slide[…]`: any call
//! whose name ends in `slide`), or a heading per slide (the starter's
//! `= Title`, touying's `== Title`). With headings, the slides are those
//! of the deepest level used, up to the second. A document that does not
//! look like a deck (no page wider than high, no slide package) or uses
//! neither way is told so and left alone.
//!
//! - `]]` and `[[` in the slides view go by slide, not by page.
//! - `:slide new` starts a slide behind the one the cursor is in, in the
//!   deck's own way, with places to fill in (Tab goes through them).
//! - `:slide up` and `:slide down` move the slide the cursor is in past
//!   its neighbour; in the overview (`:grid`), `H` and `L` (or `<` and
//!   `>`) move the slide picked out, and `n` starts one behind it. A
//!   slide is moved only where it is one block of the source.

use std::ops::Range;
use std::sync::Arc;

use typst::foundations::{Label, Selector, Value};
use typst::introspection::MetadataElem;
use typst::syntax::{LinkedNode, Source, SyntaxKind};
use typst::utils::PicoStr;
use typst_layout::PagedDocument;

use super::{Edited, Paging, Viewer};
use crate::keys::{Key, KeyPress};
use crate::sections::{self, Way};

/// How the slides of a deck were told apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    /// By the markers a slide package puts on its pages.
    Markers,
    /// By pages in a row that carry the same page number.
    Numbers,
    /// Not at all: every page is a slide.
    Pages,
}

/// A document as slides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deck {
    /// The pages of each slide, in order.
    pub slides: Vec<Range<usize>>,
    /// What each slide is called: the number the deck gives it.
    pub labels: Vec<String>,
    /// How the slides were found.
    pub found: Found,
    /// The speaker's notes of each page.
    pub notes: Vec<Vec<String>>,
}

impl Deck {
    /// The slides and notes of `document`.
    #[must_use]
    pub fn of(document: &PagedDocument) -> Self {
        let pages = document.pages();
        let count = pages.len();
        let mut marked: Vec<Option<i64>> = vec![None; count];
        let mut notes: Vec<Vec<String>> = vec![Vec::new(); count];
        for (page, value) in labelled(document, "pdfpc") {
            let Value::Dict(dict) = &value else {
                continue;
            };
            let kind = dict.get("t").ok().and_then(|kind| match kind {
                Value::Str(kind) => Some(kind.as_str().to_owned()),
                _ => None,
            });
            match (kind.as_deref(), dict.get("v").ok()) {
                (Some("LogicalSlide"), Some(Value::Int(number))) => {
                    if let Some(slot) = marked.get_mut(page) {
                        *slot = Some(*number);
                    }
                }
                (Some("Note"), Some(note)) => {
                    if let (Some(slot), Some(note)) = (notes.get_mut(page), note_text(note)) {
                        slot.push(note);
                    }
                }
                _ => {}
            }
        }
        for (page, value) in labelled(document, "speaker-note") {
            let note = match &value {
                Value::Dict(dict) => dict.get("note").ok().and_then(note_text),
                other => note_text(other),
            };
            if let (Some(slot), Some(note)) = (notes.get_mut(page), note) {
                slot.push(note);
            }
        }
        let numbers: Vec<u64> = pages.iter().map(|page| page.number).collect();
        let (found, keys): (Found, Vec<Option<i64>>) = if marked.iter().any(Option::is_some) {
            (Found::Markers, marked)
        } else if numbers.windows(2).any(|pair| pair[0] == pair[1]) {
            let keys = numbers.iter().map(|number| i64::try_from(*number).ok());
            (Found::Numbers, keys.collect())
        } else {
            (Found::Pages, vec![None; count])
        };
        let mut slides: Vec<Range<usize>> = Vec::new();
        let mut labels = Vec::new();
        for (page, key) in keys.iter().enumerate() {
            let same = page > 0 && key.is_some() && keys[page - 1] == *key;
            match slides.last_mut() {
                Some(last) if same => last.end = page + 1,
                _ => {
                    slides.push(page..page + 1);
                    labels.push(
                        key.map_or_else(|| (slides.len()).to_string(), |key| key.to_string()),
                    );
                }
            }
        }
        if found == Found::Pages {
            labels = (1..=slides.len())
                .map(|number| number.to_string())
                .collect();
        }
        Self {
            slides,
            labels,
            found,
            notes,
        }
    }

    /// The slide page `page` belongs to; the last for a page behind all.
    #[must_use]
    pub fn slide_of(&self, page: usize) -> usize {
        let after = self.slides.partition_point(|slide| slide.start <= page);
        after.saturating_sub(1)
    }

    /// The slide a speaker means by `number`: the one the deck calls so,
    /// or else the `number`th.
    #[must_use]
    pub fn numbered(&self, number: usize) -> Option<usize> {
        let called = number.to_string();
        self.labels
            .iter()
            .position(|label| *label == called)
            .or_else(|| {
                (1..=self.slides.len())
                    .contains(&number)
                    .then(|| number - 1)
            })
    }

    /// The notes of page `page`, one after the other.
    #[must_use]
    pub fn notes_of(&self, page: usize) -> &[String] {
        self.notes.get(page).map_or(&[], Vec::as_slice)
    }
}

/// The values of the metadata labelled `name` in `document`, each with
/// the page it is on (counting from 0).
fn labelled(document: &PagedDocument, name: &str) -> Vec<(usize, Value)> {
    let Some(label) = Label::new(PicoStr::intern(name)) else {
        return Vec::new();
    };
    let introspector = document.introspector();
    let found = introspector.elements().query(&Selector::Label(label));
    found
        .iter()
        .filter_map(|element| {
            let metadata = element.to_packed::<MetadataElem>()?;
            let at = element.location()?;
            let page = introspector.position(at)?.page.get() - 1;
            Some((page, metadata.value.clone()))
        })
        .collect()
}

/// A note as text: a string as it is, content as the text it reads.
fn note_text(value: &Value) -> Option<String> {
    let text = match value {
        Value::Str(text) => text.as_str().to_owned(),
        Value::Content(content) => content.plain_text().to_string(),
        _ => return None,
    };
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// How a deck's source says where its slides are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Written {
    /// A top-level call for each slide; the name of the one to start new
    /// slides with.
    Calls(String),
    /// A heading of this level for each slide.
    Headings(usize),
}

/// What is said where a document's slides are not known in its source.
pub const UNKNOWN: &str = "zetwerk does not know how this deck makes its slides: \
                           neither a #slide[…] call nor a heading per slide";
/// What is said where a document is no deck.
pub const NO_DECK: &str = "this does not look like a deck of slides (no page is wider \
                           than high, and no slide package is used)";

/// The slides as written in `source`: how they are written, and the
/// bytes of each, in order. `None` where neither way is used.
#[must_use]
pub fn blocks(source: &Source) -> Option<(Written, Vec<Range<usize>>)> {
    let text = source.text();
    let root = LinkedNode::new(source.root());
    let mut calls = Vec::new();
    let mut names = Vec::new();
    let mut hash: Option<usize> = None;
    for node in root.children() {
        match node.kind() {
            SyntaxKind::Hash => {
                hash = Some(node.offset());
                continue;
            }
            SyntaxKind::FuncCall => {
                let callee = node.children().next().map(|callee| &text[callee.range()]);
                let name = callee.map(|callee| callee.rsplit('.').next().unwrap_or(callee));
                if let (Some(name), Some(start)) = (name, hash)
                    && name.ends_with("slide")
                {
                    calls.push(line_start(text, start)..node.range().end);
                    names.push(name.to_owned());
                }
            }
            _ => {}
        }
        hash = None;
    }
    if !calls.is_empty() {
        let name = if names.iter().any(|name| name == "slide") {
            "slide".to_owned()
        } else {
            names.last().cloned().unwrap_or_default()
        };
        return Some((Written::Calls(name), calls));
    }
    let all = sections::sections(source);
    let level = all.iter().map(|section| section.level).max()?.min(2);
    let spans = all
        .iter()
        .filter(|section| section.level == level)
        .map(|section| section.span.clone())
        .collect();
    Some((Written::Headings(level), spans))
}

/// Where the line holding byte `at` begins, if only blanks are before it
/// there; else `at` itself.
fn line_start(text: &str, at: usize) -> usize {
    let start = text[..at].rfind('\n').map_or(0, |found| found + 1);
    if text[start..at].trim().is_empty() {
        start
    } else {
        at
    }
}

/// Whether `source` and its pages look like a deck of slides.
#[must_use]
pub fn is_deck(source: &Source, document: Option<&PagedDocument>) -> bool {
    let text = source.text();
    let package = ["@preview/touying", "@preview/polylux", "presentation-"]
        .iter()
        .any(|sign| text.contains(sign));
    let wide = document.is_some_and(|document| {
        let pages = document.pages();
        pages
            .iter()
            .any(|page| page.frame.width() > page.frame.height())
    });
    package || wide
}

/// Where a new slide behind the one at byte `at` goes in `source`, and
/// what is written there: the text, and the byte in it where its first
/// place to fill in is.
///
/// # Errors
///
/// Where the source does not say how its slides are written.
pub fn new_slide(source: &Source, at: usize) -> Result<(usize, String), String> {
    let text = source.text();
    let (way, blocks) = blocks(source).ok_or(UNKNOWN)?;
    match way {
        Written::Calls(name) => {
            let behind = blocks.iter().rev().find(|block| block.start <= at);
            if let Some(block) = behind {
                Ok((
                    block.end,
                    format!("\n\n#{name}[\n  ‹What this slide says.›\n]"),
                ))
            } else {
                let first = blocks.first().map_or(text.len(), |block| block.start);
                Ok((first, format!("#{name}[\n  ‹What this slide says.›\n]\n\n")))
            }
        }
        Written::Headings(level) => {
            // Before the next heading behind the cursor that is a slide
            // or a section of slides.
            let all = sections::sections(source);
            let next = all
                .iter()
                .find(|section| section.level <= level && section.span.start > at);
            let marks = "=".repeat(level);
            let slide = format!("{marks} ‹Title›\n\n‹What this slide says.›\n");
            if let Some(section) = next {
                Ok((section.span.start, format!("{slide}\n")))
            } else {
                let ending = if text.ends_with("\n\n") {
                    ""
                } else if text.ends_with('\n') {
                    "\n"
                } else {
                    "\n\n"
                };
                Ok((text.len(), format!("{ending}{slide}")))
            }
        }
    }
}

/// Edits of a text, each referring to the text as those before it left
/// it.
pub type Edits = Vec<(Range<usize>, String)>;

/// The edits that move the slide at byte `at` of `source` past the one
/// before it (`later` false) or behind it, and where the slide begins
/// then.
///
/// # Errors
///
/// Where the slide is not one block of the source, or has nowhere to go.
pub fn moved(source: &Source, at: usize, later: bool) -> Result<(Edits, usize), String> {
    let text = source.text();
    let (way, blocks) = blocks(source).ok_or(UNKNOWN)?;
    match way {
        Written::Calls(_) => {
            let index = blocks
                .iter()
                .position(|block| block.contains(&at))
                .ok_or("the cursor is in no slide: a slide is moved where it is one #slide[…]")?;
            let other = if later {
                index + 1
            } else {
                index.checked_sub(1).ok_or("this is the first slide")?
            };
            let other = blocks.get(other).ok_or("this is the last slide")?;
            let (first, second) = if later {
                (&blocks[index], other)
            } else {
                (other, &blocks[index])
            };
            let between = &text[first.end..second.start];
            if !between.trim().is_empty() {
                return Err("there is text between this slide and the next: \
                            it is not one block of the source"
                    .to_owned());
            }
            let swapped = format!("{}{between}{}", &text[second.clone()], &text[first.clone()]);
            let start = if later {
                first.start + second.len() + between.len()
            } else {
                first.start
            };
            Ok((vec![(first.start..second.end, swapped)], start))
        }
        Written::Headings(level) => {
            let all = sections::sections(source);
            let mut index = sections::at(&all, at).ok_or("the cursor is before the first slide")?;
            while all[index].level > level {
                index = all[index].parent.ok_or("the cursor is in no slide")?;
            }
            if all[index].level < level {
                return Err("the cursor is on a section of slides: gO moves sections".to_owned());
            }
            let way = if later { Way::Down } else { Way::Up };
            let change = sections::changed(source, index, way)?;
            Ok((change.edits, change.start))
        }
    }
}

impl Viewer {
    /// The deck the document is, as far as its pages tell; worked out
    /// again only for new pages.
    pub(super) fn deck(&mut self) -> Option<Arc<Deck>> {
        let document = self.whole.clone().or_else(|| {
            self.document
                .as_ref()
                .map(|pages| std::sync::Arc::clone(pages.document()))
        })?;
        if let Some((of, deck)) = &self.present.deck
            && Arc::ptr_eq(of, &document)
        {
            return Some(Arc::clone(deck));
        }
        let deck = Arc::new(Deck::of(&document));
        self.present.deck = Some((document, Arc::clone(&deck)));
        Some(deck)
    }

    /// `]]` and `[[` in the slides view: to the first page of the next
    /// slide, or of this one or the one before. Returns whether it went
    /// so (elsewhere the pages go by one).
    pub(super) fn step_slide(&mut self, forward: bool) -> bool {
        if self.paging != Paging::Slides {
            return false;
        }
        let Some(deck) = self.deck().filter(|deck| deck.found != Found::Pages) else {
            return false;
        };
        let current = deck.slide_of(self.slide);
        let target = if forward {
            current + 1
        } else if self.slide > deck.slides[current].start {
            current
        } else {
            current.saturating_sub(1)
        };
        let Some(pages) = deck.slides.get(target) else {
            return true;
        };
        self.goto_page(pages.start);
        true
    }

    /// `:slide new`, `:slide up`, `:slide down`. Returns whether
    /// `command` was one of them.
    pub(super) fn slide_command(&mut self, command: &str) -> bool {
        let done = match command {
            "slide new" | "slide" => self.start_slide(),
            "slide up" => self.move_slide(false),
            "slide down" => self.move_slide(true),
            _ => return false,
        };
        if let Err(why) = done {
            self.alert(why);
        }
        true
    }

    /// Starts a slide behind the one the cursor is in.
    fn start_slide(&mut self) -> Result<(), String> {
        let editor = self.editor.as_ref().ok_or("no file is being edited")?;
        if !is_deck(
            editor.source(),
            self.document.as_ref().map(|pages| &**pages.document()),
        ) {
            return Err(NO_DECK.to_owned());
        }
        let (at, slide) = new_slide(editor.source(), editor.cursor())?;
        let editor = self.editor.as_mut().ok_or("no file is being edited")?;
        editor
            .replace(&[(at..at, slide.as_str())])
            .map_err(|why| format!("not changed: {why}"))?;
        self.text_changed(Edited::Command);
        if !self.go_to_blank(at) {
            self.notice("a new slide  (u takes it back)");
        }
        Ok(())
    }

    /// Moves the slide the cursor is in past its neighbour.
    fn move_slide(&mut self, later: bool) -> Result<(), String> {
        let editor = self.editor.as_mut().ok_or("no file is being edited")?;
        let (edits, start) = moved(editor.source(), editor.cursor(), later)?;
        let edits: Vec<(Range<usize>, &str)> = edits
            .iter()
            .map(|(range, with)| (range.clone(), with.as_str()))
            .collect();
        editor
            .replace(&edits)
            .map_err(|why| format!("not changed: {why}"))?;
        editor.jump(start);
        self.text_changed(Edited::Command);
        self.notice(if later {
            "the slide is behind the next one now  (u takes it back)"
        } else {
            "the slide is before the one before now  (u takes it back)"
        });
        Ok(())
    }

    /// Keys of the overview that change the deck: `H` and `L` (or `<`
    /// and `>`) move the slide picked out, `n` starts one behind it.
    /// Returns whether `press` was one of them.
    pub(super) fn grid_slide_key(&mut self, press: KeyPress) -> bool {
        let Some(chosen) = self.grid.as_ref().map(|grid| grid.chosen) else {
            return false;
        };
        let later = match (press.key, press.ctrl || press.alt) {
            (Key::Char('H' | '<'), false) => Some(false),
            (Key::Char('L' | '>'), false) => Some(true),
            (Key::Char('n'), false) => None,
            _ => return false,
        };
        let (Some(deck), Some(first)) = (self.deck(), self.first_offset(chosen)) else {
            self.alert("the slide picked out has no text of this file to find it by");
            return true;
        };
        if let Some(editor) = &mut self.editor {
            editor.set_cursor(first);
        }
        let Some(later) = later else {
            self.grid = None;
            if let Err(why) = self.start_slide() {
                self.alert(why);
            }
            return true;
        };
        match self.move_slide(later) {
            Ok(()) => {
                // Where the slide will be once the pages are made again.
                let slide = deck.slide_of(chosen);
                let neighbour = if later {
                    deck.slides.get(slide + 1)
                } else {
                    slide
                        .checked_sub(1)
                        .and_then(|before| deck.slides.get(before))
                };
                if let (Some(grid), Some(neighbour)) = (&mut self.grid, neighbour) {
                    let width = neighbour.len();
                    grid.chosen = if later {
                        chosen + width
                    } else {
                        chosen.saturating_sub(width)
                    };
                }
            }
            Err(why) => self.alert(why),
        }
        true
    }

    /// Where the first text of page `index` is in the file being edited,
    /// if it is of that file.
    pub(super) fn first_offset(&self, index: usize) -> Option<usize> {
        let (document, world, change) = (
            self.document.as_ref()?,
            self.compiled_world.as_ref()?,
            self.places()?,
        );
        let page = document.pages().get(index)?;
        let lines = zetwerk_typst::PageMap::build(world, &page.frame).lines();
        let line = lines.enter(true)?;
        let (file, offset) = lines.first(line)?;
        (file == self.file_id()).then(|| change.forth(offset))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use std::sync::Arc;

    use zetwerk_typst::{Environment, FontSet, Project};

    use super::*;

    fn source(text: &str) -> Source {
        Source::detached(text)
    }

    fn compiled(text: &str) -> PagedDocument {
        let env = Environment::new(FontSet::Embedded);
        let project = Project::in_memory(
            Arc::clone(&env),
            std::path::PathBuf::from("/"),
            "main.typ",
            text.to_owned(),
        )
        .unwrap();
        let compiled = zetwerk_typst::compile(&project.snapshot());
        compiled.document.unwrap()
    }

    #[test]
    fn slides_are_runs_of_pages_by_markers_numbers_or_pages() {
        // As touying marks its pages: the second slide in three steps.
        let marked = "#set page(width: 200pt, height: 100pt)\n\
            #let mark(n) = [#metadata((t: \"NewSlide\")) <pdfpc>#metadata((t: \"LogicalSlide\", v: n)) <pdfpc>]\n\
            #mark(1) One\n#pagebreak()\n\
            #mark(2) Two#metadata((t: \"Note\", v: \"say it slowly\")) <pdfpc>\n#pagebreak()\n\
            #mark(2) Two, more\n#pagebreak()\n#mark(2) Two, all\n#pagebreak()\n\
            #mark(3) Three #metadata((note: \"the last\")) <speaker-note>\n";
        let deck = Deck::of(&compiled(marked));
        assert_eq!(deck.found, Found::Markers);
        assert_eq!(deck.slides, [0..1, 1..4, 4..5]);
        assert_eq!(deck.labels, ["1", "2", "3"]);
        assert_eq!(deck.notes_of(1), ["say it slowly"]);
        assert_eq!(deck.notes_of(4), ["the last"]);
        assert_eq!(
            (deck.slide_of(0), deck.slide_of(3), deck.slide_of(9)),
            (0, 1, 2)
        );
        assert_eq!(deck.numbered(2), Some(1));
        // By page numbers that stay the same over the steps of a slide.
        let numbered = "#set page(width: 200pt, height: 100pt, numbering: \"1\")\n\
            One\n#pagebreak()\nTwo\n#pagebreak()\n#counter(page).update(n => n - 1)Two, more\n\
            #pagebreak()\nThree #metadata([Just *this*.]) <speaker-note>\n";
        let deck = Deck::of(&compiled(numbered));
        assert_eq!(deck.found, Found::Numbers);
        assert_eq!(deck.slides, [0..1, 1..3, 3..4]);
        assert_eq!(deck.labels, ["1", "2", "3"]);
        assert_eq!(deck.notes_of(3), ["Just this."]);
        // Otherwise a page is a slide.
        let plain = "#set page(width: 200pt, height: 100pt)\nOne\n#pagebreak()\nTwo\n";
        let deck = Deck::of(&compiled(plain));
        assert_eq!(
            (deck.found, deck.slides.clone()),
            (Found::Pages, vec![0..1, 1..2])
        );
        assert_eq!(deck.numbered(2), Some(1));
        assert_eq!(deck.numbered(3), None);
    }

    #[test]
    fn slides_are_found_in_the_source_by_calls_or_headings() {
        let calls = source(
            "#import \"x.typ\": *\n#slide[One]\n\n#focus-slide[Two]\nloose\n#slide[Three]\n",
        );
        let (way, found) = blocks(&calls).unwrap();
        assert_eq!(way, Written::Calls("slide".to_owned()));
        let text = calls.text();
        let written: Vec<&str> = found.iter().map(|block| &text[block.clone()]).collect();
        assert_eq!(
            written,
            ["#slide[One]", "#focus-slide[Two]", "#slide[Three]"]
        );
        // The starter's: a heading of the first level each.
        let starter = source(include_str!("../../templates/slides.typ"));
        assert!(
            matches!(blocks(&starter), Some((Written::Headings(1), found)) if found.len() == 3)
        );
        // touying's: sections of the first level, slides of the second.
        let touying = source("= Part\n\n== One\n\nA\n\n== Two\n\nB\n\n= Next\n\n== Three\n");
        assert!(
            matches!(blocks(&touying), Some((Written::Headings(2), found)) if found.len() == 3)
        );
        assert_eq!(blocks(&source("Just text.\n")), None);
    }

    #[test]
    fn a_new_slide_goes_behind_the_one_the_cursor_is_in() {
        let deck =
            source("#set page(paper: \"presentation-16-9\")\nTitle\n\n= One\n\nA\n\n= Two\n\nB\n");
        let text = deck.text();
        // In the first slide: before the second.
        let (at, slide) = new_slide(&deck, text.find('A').unwrap()).unwrap();
        assert_eq!(at, text.find("= Two").unwrap());
        assert_eq!(slide, "= ‹Title›\n\n‹What this slide says.›\n\n");
        // On the title, before any heading: before the first.
        let (at, _) = new_slide(&deck, text.find("Title").unwrap()).unwrap();
        assert_eq!(at, text.find("= One").unwrap());
        // In the last: at the end.
        let (at, slide) = new_slide(&deck, text.find('B').unwrap()).unwrap();
        assert_eq!((at, slide.starts_with("\n= ‹Title›")), (text.len(), true));
        // A call for each slide: another such call behind it.
        let calls = source("#slide[One]\n\n#slide[Two]\n");
        let (at, slide) = new_slide(&calls, 3).unwrap();
        assert_eq!(at, "#slide[One]".len());
        assert_eq!(slide, "\n\n#slide[\n  ‹What this slide says.›\n]");
        assert_eq!(new_slide(&source("Text.\n"), 0), Err(UNKNOWN.to_owned()));
    }

    #[test]
    fn a_slide_moves_past_its_neighbour_as_one_block() {
        let calls = source("#slide[One]\n\n#slide[Two]\n\n#slide[Three]\n");
        let (edits, start) = moved(&calls, 2, true).unwrap();
        let mut text = calls.text().to_owned();
        for (range, with) in &edits {
            text.replace_range(range.clone(), with);
        }
        assert_eq!(text, "#slide[Two]\n\n#slide[One]\n\n#slide[Three]\n");
        assert_eq!(&text[start..start + 11], "#slide[One]");
        assert!(
            moved(&calls, 2, false).is_err(),
            "the first has nothing before it"
        );
        let loose = source("#slide[One]\nloose words\n#slide[Two]\n");
        assert!(
            moved(&loose, 2, true)
                .unwrap_err()
                .contains("not one block")
        );
        let headings = source("= One\n\nA\n\n= Two\n\nB\n");
        let (edits, _) = moved(&headings, headings.text().find('B').unwrap(), false).unwrap();
        let mut text = headings.text().to_owned();
        for (range, with) in &edits {
            text.replace_range(range.clone(), with);
        }
        assert!(text.starts_with("= Two\n\nB\n"), "{text}");
    }
}
