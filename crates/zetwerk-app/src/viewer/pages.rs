//! Pages put into the document whole: a page of a PDF (a cover made
//! elsewhere, a signed form, a paper as an appendix, slides), an image
//! as a page of its own, a blank page, a page break.
//!
//! `:pages` (or `gS`, for sheets and Seiten) lists the pages of the
//! document, each with what it is: its first words, "page 2 of
//! cover.pdf", "a blank page". Enter on one shows what can be done with
//! it: go there, put pages in before or behind it, and for a page put
//! in: take it out (alone, or with the pages of its file around it),
//! move it up or down past the text to the nearest page break, show
//! another file or other pages of it, leave it as large as it is or
//! fit it to the document's paper, count it in the page numbers or not.
//!
//! `:page add [file] [pages]` puts pages in: the file is chosen from
//! those beside the document or typed (from anywhere: it is copied in,
//! see [`page_text::brought_in`]), the pages of a PDF are asked for (the
//! prompt says how many it has), and then where: after the paragraph
//! the cursor is in (the text goes on on the next page), after or
//! before the page the cursor is on, at the start or at the end.
//!
//! On the page, a page put in is one thing: the cursor comes to rest on
//! it (`j`, `k`, `]]`), `x` or `dd` takes it out (`u` brings it back),
//! Enter shows what can be done with it, and what would type into it
//! does nothing. A small tag beside it says what it is.
//!
//! What is written, and how it is read back, is in [`page_text`]. Each
//! change is one step for `u`.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use typst::World;
use typst::layout::{Frame, FrameItem, Point, Rect};
use typst::syntax::{FileId, Source, Span};
use typst_layout::PagedDocument;
use zetwerk_typst::{Origin, PageLines, PageMap, PagePosition, ProjectWorld};

use super::agent::Outcome;
use super::assist::Target;
use super::{Edited, Panes, Quad, Scene, TextureId, Viewer, tint};
use crate::editor;
use crate::keys::{Key, KeyPress};
use crate::page_text::{self, Inserted, Made, Page, Shows};

/// What is said where no file is being edited.
const NOT_EDITING: &str = "no file is being edited";
/// What is said where the pages are not those of the text.
const NOT_TYPESET: &str = "the pages are not typeset yet: a moment, then again";
/// What is said of keys that would type into a page put in.
const ON_A_PAGE: &str = "x takes it out, Enter shows what can be done with it";
/// How many pages either way are looked at for what a page with
/// nothing on it is.
const NEIGHBOURS: usize = 12;
/// The colour of the tag beside a page put in, and of the frame around
/// it while the cursor is on it.
const TAG: &str = "#928374";
const FRAME: [u8; 4] = [0x45, 0x85, 0x88, 0xd0];
/// The size of the tag's text, in points of the page, and its font.
const TAG_SIZE: f64 = 6.5;
const FONT: &str = "DejaVu Sans Mono";

/// What is kept between keys.
#[derive(Default)]
pub(super) struct Pages {
    /// A `d` on a page put in, waiting for its second.
    deleting: bool,
    /// What the pages of the document on screen show of pages put in,
    /// as far as asked: for which document, and by page.
    seen: Option<(usize, HashMap<usize, Option<Seen>>)>,
}

/// A page put in, as a page of the document on screen shows it: its
/// bytes in the text that document was made of, and what it is.
#[derive(Debug, Clone)]
struct Seen {
    range: Range<usize>,
    called: String,
}

/// What a page of the document shows of the file edited: the bytes it
/// shows from first to last (of the text it was made of), the page put
/// in that it is (its index among those of that text), and its first
/// words.
#[derive(Debug, Clone, Default)]
struct Fact {
    shown: Option<(usize, usize)>,
    unit: Option<usize>,
    words: String,
}

/// A path as one word of a command, and back.
fn word_of(path: &str) -> String {
    path.replace('%', "%25").replace(' ', "%20")
}

fn path_of(word: &str) -> String {
    word.replace("%20", " ").replace("%25", "%")
}

/// The spans of the images on `frame`.
fn images_on(frame: &Frame, spans: &mut Vec<Span>) {
    for (_, item) in frame.items() {
        match item {
            FrameItem::Group(group) => images_on(&group.frame, spans),
            FrameItem::Image(_, _, span) => spans.push(*span),
            _ => {}
        }
    }
}

/// Whether `frame` shows nothing at all.
fn is_empty(frame: &Frame) -> bool {
    frame.items().all(|(_, item)| match item {
        FrameItem::Group(group) => is_empty(&group.frame),
        FrameItem::Tag(_) | FrameItem::Link(..) => true,
        FrameItem::Text(_) | FrameItem::Shape(..) | FrameItem::Image(..) => false,
    })
}

/// The text of `file` as written that `world` was made of, and the
/// pages put into it.
fn written_pages(world: &ProjectWorld, file: FileId) -> Option<(Source, Vec<Inserted>)> {
    let source = world.written(file).ok()?;
    let pages = page_text::inserted(&source);
    Some((source, pages))
}

/// Of `pages` (put into the written text of `world`), the one an image
/// on page `frame` was made by.
fn image_unit(
    world: &ProjectWorld,
    file: FileId,
    frame: &Frame,
    pages: &[Inserted],
) -> Option<usize> {
    let mut spans = Vec::new();
    images_on(frame, &mut spans);
    let shown = world.source(file).ok()?;
    spans.iter().find_map(|span| {
        if span.id() != Some(file) {
            return None;
        }
        let at = world.written_at(file, shown.find(*span)?.range().start);
        pages.iter().position(|page| page.range.contains(&at))
    })
}

/// The first and the last byte of `file` that page `index` shows, from
/// its glyphs, or from the page put in that it is.
fn shown_by(
    document: &PagedDocument,
    world: &ProjectWorld,
    file: FileId,
    index: usize,
    pages: &[Inserted],
) -> Option<(usize, usize)> {
    let page = document.pages().get(index)?;
    if let Some(unit) = image_unit(world, file, &page.frame, pages) {
        let range = &pages[unit].range;
        return Some((range.start, range.end));
    }
    let map = PageMap::build(world, &page.frame);
    let mut shown: Option<(usize, usize)> = None;
    for glyph in map.glyphs().iter().filter(|glyph| !glyph.furniture) {
        let range = match &glyph.origin {
            Origin::Text { file: of, range }
            | Origin::Space { file: of, range }
            | Origin::Math { file: of, range }
            | Origin::Object {
                file: of, range, ..
            } if *of == file => range,
            _ => continue,
        };
        shown = Some(shown.map_or((range.start, range.end), |(first, last)| {
            (first.min(range.start), last.max(range.end))
        }));
    }
    shown
}

/// Of the blank pages put in among `pages`, the one page `index` (which
/// shows nothing) is: the empty pages between the nearest pages that
/// show something are the blank pages put in between what those show,
/// one by one, where there are as many of either.
fn blank_unit(
    document: &PagedDocument,
    world: &ProjectWorld,
    file: FileId,
    index: usize,
    pages: &[Inserted],
) -> Option<usize> {
    let all = document.pages();
    let empty = |at: usize| all.get(at).is_some_and(|page| is_empty(&page.frame));
    let mut before = index;
    while before > 0 && empty(before - 1) && index - before < NEIGHBOURS {
        before -= 1;
    }
    let mut behind = index;
    while behind + 1 < all.len() && empty(behind + 1) && behind - index < NEIGHBOURS {
        behind += 1;
    }
    let from = match before.checked_sub(1) {
        Some(shown) => shown_by(document, world, file, shown, pages)?.1,
        None => 0,
    };
    let to = if behind + 1 < all.len() {
        shown_by(document, world, file, behind + 1, pages)?.0
    } else {
        usize::MAX
    };
    let blanks: Vec<usize> = (0..pages.len())
        .filter(|unit| {
            let page = &pages[*unit];
            page.shows == Shows::Blank && page.range.start >= from && page.range.end <= to
        })
        .collect();
    (blanks.len() == behind - before + 1).then(|| blanks[index - before])
}

/// The page put in that page `index` of `document` is, among those of
/// the written text of `world`: by its image, or, where it shows
/// nothing, by where it stands.
fn unit_on(
    document: &PagedDocument,
    world: &ProjectWorld,
    file: FileId,
    index: usize,
    pages: &[Inserted],
) -> Option<usize> {
    let page = document.pages().get(index)?;
    if let Some(unit) = image_unit(world, file, &page.frame, pages) {
        return Some(unit);
    }
    if is_empty(&page.frame) && pages.iter().any(|page| page.shows == Shows::Blank) {
        return blank_unit(document, world, file, index, pages);
    }
    None
}

/// The lines of page `index` of `document`, whose frame is `frame`, for
/// a cursor that moves on the pages: those of its text, or, on a page
/// put in, the page itself as one place, the bytes of all of its call.
pub(super) fn lines_of(
    world: &ProjectWorld,
    document: &PagedDocument,
    index: usize,
    frame: &Frame,
    file: FileId,
) -> PageLines {
    let lines = PageMap::build(world, frame).lines();
    if !lines.is_empty() {
        return lines;
    }
    let found = written_pages(world, file).and_then(|(_, pages)| {
        let unit = unit_on(document, world, file, index, &pages)?;
        Some(pages[unit].range.clone())
    });
    match found {
        Some(range) => {
            let whole = Rect::new(Point::zero(), Point::new(frame.width(), frame.height()));
            PageLines::of_one(file, range, whole)
        }
        None => lines,
    }
}

/// Whether byte `at` of `text` is on a line that begins a page put in:
/// a look that costs no more than the line, for what is asked of every
/// cursor.
fn on_page_line(text: &str, at: usize) -> bool {
    let at = at.min(text.len());
    let Some(before) = text.get(..at) else {
        return false;
    };
    let start = before.rfind('\n').map_or(0, |found| found + 1);
    text[start..].trim_start().starts_with("#page")
}

/// The page of `document` that shows the page put in at byte `then` of
/// the text `world` was made of (as written), if it is in one.
pub(super) fn page_showing(
    document: &PagedDocument,
    world: &ProjectWorld,
    file: FileId,
    then: usize,
) -> Option<usize> {
    let (source, pages) = written_pages(world, file)?;
    if !on_page_line(source.text(), then) {
        return None;
    }
    let unit = pages.iter().position(|page| page.range.contains(&then))?;
    (0..document.pages().len())
        .find(|index| unit_on(document, world, file, *index, &pages) == Some(unit))
}

/// Where the page put in at byte `then` of the text `world` was made of
/// is on the pages: its top-left corner.
pub(super) fn shown_on(
    document: &PagedDocument,
    world: &ProjectWorld,
    file: FileId,
    then: usize,
) -> Option<PagePosition> {
    page_showing(document, world, file, then).map(|page| PagePosition {
        page,
        x: 0.0,
        y: 0.0,
    })
}

/// `figures` without the images that are pages put in: those are no
/// figures, and are listed as pages (see `get_outline`).
pub(super) fn not_pages(
    source: &Source,
    mut figures: Vec<crate::figure_text::Figure>,
) -> Vec<crate::figure_text::Figure> {
    let pages = page_text::inserted(source);
    figures.retain(|figure| !pages.iter().any(|page| page.range.contains(&figure.call)));
    figures
}

/// The markup of the tag beside a page put in.
fn tag_markup(called: &str, size: f64) -> String {
    format!(
        "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n\
         #set text(font: \"{FONT}\", size: {size}pt, fill: rgb(\"{TAG}\").transparentize(15%))\n\
         {}",
        crate::figure_text::quoted(called)
            .strip_prefix('"')
            .and_then(|it| it.strip_suffix('"'))
            .map_or_else(String::new, |it| format!("#\"{it}\""))
    )
}

impl Viewer {
    /// The pages put into the file being edited.
    fn pages_put_in(&self) -> Result<Vec<Inserted>, String> {
        let editor = self.editor.as_ref().ok_or(NOT_EDITING)?;
        Ok(page_text::inserted(editor.source()))
    }

    /// The page put in at the cursor, if the cursor is on one: its index
    /// among them.
    pub(super) fn page_here(&self) -> Option<usize> {
        let editor = self.editor.as_ref()?;
        if !on_page_line(editor.text(), editor.cursor()) {
            return None;
        }
        let cursor = editor.cursor();
        let pages = page_text::inserted(editor.source());
        pages.iter().position(|page| page.range.contains(&cursor))
    }

    /// Whether the pages on screen are those of the text as it is.
    fn pages_current(&self) -> bool {
        self.document.is_some()
            && self.compiled_world.is_some()
            && self.document_revision == Some(self.revision)
            && self.predicted.is_none()
    }

    /// What each page of the document shows, where the pages are those
    /// of the text as it is.
    fn page_facts(&mut self) -> Option<Vec<Fact>> {
        if !self.pages_current() {
            return None;
        }
        let file = self.file_id();
        let document = self.document.clone()?;
        let world = self.compiled_world.clone()?;
        let (source, pages) = written_pages(&world, file)?;
        let text = source.text();
        let mut facts = Vec::new();
        for (index, page) in document.pages().iter().enumerate() {
            if let Some(unit) = unit_on(&document, &world, file, index, &pages) {
                let range = &pages[unit].range;
                facts.push(Fact {
                    shown: Some((range.start, range.end)),
                    unit: Some(unit),
                    words: String::new(),
                });
                continue;
            }
            let map = self
                .maps
                .entry(index)
                .or_insert_with(|| PageMap::build(&world, &page.frame));
            let mut fact = Fact::default();
            for glyph in map.glyphs().iter().filter(|glyph| !glyph.furniture) {
                let (range, words) = match &glyph.origin {
                    Origin::Text { file: of, range } if *of == file => (range, true),
                    Origin::Space { file: of, range } if *of == file => (range, true),
                    Origin::Math { file: of, range }
                    | Origin::Object {
                        file: of, range, ..
                    } if *of == file => (range, false),
                    _ => continue,
                };
                fact.shown = Some(fact.shown.map_or((range.start, range.end), |(a, b)| {
                    (a.min(range.start), b.max(range.end))
                }));
                if words && fact.words.chars().count() < 48 {
                    let piece = text.get(range.clone()).unwrap_or("");
                    let piece = if piece.trim().is_empty() { " " } else { piece };
                    if !(piece == " " && fact.words.ends_with(' ')) {
                        fact.words.push_str(piece);
                    }
                }
            }
            facts.push(fact);
        }
        Some(facts)
    }

    /// The page of the document the cursor is on, counting from 0.
    fn page_of_cursor(&self) -> usize {
        self.cursor_page.get()
    }

    /// The commands of the pages panel: `:pages …`, and `:page add …`.
    /// Returns whether `name` and `rest` were one of them.
    pub(super) fn pages_command(&mut self, name: &str, rest: &str) -> bool {
        let done = match (name, rest.split_once(' ').map_or(rest, |(verb, _)| verb)) {
            ("page", "add") => {
                let typed = rest.strip_prefix("add").unwrap_or("").trim();
                self.page_add(typed)
            }
            ("pages", _) => self.pages_panel(rest),
            _ => return false,
        };
        if let Err(why) = done {
            self.alert(why);
        }
        true
    }

    /// `:page add [file] [pages]`: what to put in, then where.
    fn page_add(&mut self, typed: &str) -> Result<(), String> {
        if typed.is_empty() {
            return self.pages_panel("choose ask");
        }
        let words: Vec<&str> = typed.split_whitespace().collect();
        let range_like = |word: &str| {
            word.eq_ignore_ascii_case("all")
                || word
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == ',' || c == '-')
        };
        let (file, pages) = match words.as_slice() {
            [.., last] if words.len() > 1 && range_like(last) => {
                let file = typed[..typed.len() - last.len()].trim();
                (file, *last)
            }
            _ => (typed, ""),
        };
        match (file, pages) {
            ("blank" | "break", _) => self.pages_panel(&format!("range-for ask :{file} all")),
            (file, "") => self.pages_panel(&format!("file-for ask {file}")),
            (file, pages) => {
                let file = self.resolve_file(file)?;
                self.pages_panel(&format!("range-for ask {} {pages}", word_of(&file)))
            }
        }
    }

    /// The commands of the panel, without their `pages`.
    fn pages_panel(&mut self, command: &str) -> Result<(), String> {
        let mut words = command.splitn(2, ' ');
        let verb = words.next().unwrap_or("").trim();
        let rest = words.next().unwrap_or("").trim();
        let mut parts = rest.splitn(3, ' ');
        let mut part = || parts.next().unwrap_or("").to_owned();
        match verb {
            "" => self.list_pages(None),
            "list" => self.list_pages(Some(rest)),
            "open" => self.open_page_entry(rest),
            "go" => self.go_to_page(rest),
            "choose" => self.choose_what(rest),
            "file-for" => {
                let (place, file) = rest.split_once(' ').unwrap_or((rest, ""));
                self.ask_pages_of(place, file.trim())
            }
            "range-for" | "range-for!" => {
                let (place, file, pages) = (part(), part(), part());
                self.ask_where(&place, &path_of(&file), &pages, verb.ends_with('!'))
            }
            "put" => {
                let (place, file, pages) = (part(), part(), part());
                self.put_pages(&place, &path_of(&file), &pages)
            }
            "remove" | "remove-run" => {
                let unit = unit_key(rest)?;
                self.remove_pages(unit, verb == "remove-run")
                    .and_then(|()| self.list_pages(None))
            }
            "up" | "down" | "up-run" | "down-run" => {
                let unit = unit_key(rest)?;
                let moved = self.move_pages(unit, verb.starts_with("up"), verb.ends_with("-run"));
                match moved {
                    Ok(now) => self.open_page_entry(&format!("u{now}")),
                    Err(why) => self.open_page_entry(rest).and(Err(why)),
                }
            }
            "fit" | "count" => {
                let unit = unit_key(rest)?;
                let switched = self.switch_page(unit, verb == "fit");
                let shown = self.open_page_entry(rest);
                switched.and(shown)
            }
            "file" => self.ask_other_file(&part()),
            "file!" => {
                let (key, file) = (part(), part());
                let unit = unit_key(&key)?;
                let done = self.other_file(unit, &path_of(&file));
                let shown = self.open_page_entry(&key);
                done.and(shown)
            }
            "range" => self.ask_other_range(&part()),
            "range!" => {
                let (key, pages) = (
                    part(),
                    rest.split_once(' ').map_or("", |(_, it)| it).to_owned(),
                );
                let unit = unit_key(&key)?;
                let done = self.other_range(unit, pages.trim());
                let shown = done
                    .as_ref()
                    .map_or(Ok(()), |now| self.open_page_entry(&format!("u{now}")));
                done.map(|_| ()).and(shown)
            }
            _ => Err(format!("not a command: pages {command}")),
        }
    }

    /// `:pages`: the pages of the document, each with what it is; the
    /// one `chosen` names (`u3`, `p5`), or the one the cursor is on,
    /// comes up chosen.
    fn list_pages(&mut self, chosen: Option<&str>) -> Result<(), String> {
        let pages = self.pages_put_in()?;
        let here = self.page_here();
        let mut items: Vec<(String, Target)> = Vec::new();
        let mut keys: Vec<String> = Vec::new();
        if let Some(facts) = self.page_facts() {
            for (index, fact) in facts.iter().enumerate() {
                let (key, what, more) = match fact
                    .unit
                    .and_then(|unit| pages.get(unit).map(|page| (unit, page)))
                {
                    Some((unit, page)) => (format!("u{unit}"), page.called(), described(page)),
                    None => {
                        let words = fact.words.trim();
                        let what = if words.is_empty() {
                            "(nothing of this file)".to_owned()
                        } else {
                            format!("{words}…")
                        };
                        (format!("p{index}"), what, String::new())
                    }
                };
                items.push((
                    format!("{:>3}  {what}\t{more}", index + 1),
                    Target::Command(format!("pages open {key}")),
                ));
                keys.push(key);
            }
        } else {
            // Not typeset as it is yet: the pages put in, and the text
            // between them, as the text has them.
            let editor = self.editor.as_ref().ok_or(NOT_EDITING)?;
            let source = editor.source();
            let text = source.text();
            let mut from = page_text::content_start(source);
            for (unit, page) in pages.iter().enumerate() {
                let between = text.get(from..page.range.start).unwrap_or("").trim();
                if !between.is_empty() {
                    let words: String = between.chars().take(48).collect();
                    items.push((
                        format!("     {}…", words.replace('\n', " ")),
                        Target::Offset(from),
                    ));
                    keys.push(String::new());
                }
                items.push((
                    format!("     {}\t{}", page.called(), described(page)),
                    Target::Command(format!("pages open u{unit}")),
                ));
                keys.push(format!("u{unit}"));
                from = page.range.end;
            }
            let rest = text.get(from..).unwrap_or("").trim();
            if !rest.is_empty() {
                let words: String = rest.chars().take(48).collect();
                items.push((
                    format!("     {}…", words.replace('\n', " ")),
                    Target::Offset(from),
                ));
                keys.push(String::new());
            }
        }
        if items.is_empty() {
            return Err("the document has no pages yet".to_owned());
        }
        let wanted = chosen
            .map(str::to_owned)
            .or_else(|| here.map(|unit| format!("u{unit}")))
            .unwrap_or_else(|| format!("p{}", self.page_of_cursor()));
        let chosen = keys.iter().position(|key| *key == wanted).unwrap_or(0);
        items.push((
            "put pages in: a PDF, an image, a blank page, a page break\t:page add".to_owned(),
            Target::Command("pages choose ask".to_owned()),
        ));
        self.open_picker("pages", items, chosen);
        Ok(())
    }

    /// What can be done with the page `key` names (`u3`: the fourth page
    /// put in; `p5`: the sixth page of the document).
    fn open_page_entry(&mut self, key: &str) -> Result<(), String> {
        let pages = self.pages_put_in()?;
        let editor = self.editor.as_ref().ok_or(NOT_EDITING)?;
        let mut items: Vec<(String, Target)> = Vec::new();
        let run = |unit: usize| page_text::run_of(editor.text(), &pages, unit);
        let command = |verb: &str| Target::Command(format!("pages {verb} {key}"));
        let title = if let Some(unit) = key.strip_prefix('u') {
            let unit: usize = unit.parse().map_err(|_| format!("no page {key}"))?;
            let page = pages.get(unit).ok_or("the page is not there any more")?;
            items.push((format!("go there\t{}", page.called()), command("go")));
            items.push((
                "put pages in before it".to_owned(),
                Target::Command(format!("pages choose before-u{unit}")),
            ));
            items.push((
                "put pages in behind it".to_owned(),
                Target::Command(format!("pages choose after-u{unit}")),
            ));
            if page.is_known() {
                let run = run(unit);
                items.push(("take it out of the document".to_owned(), command("remove")));
                items.push((
                    "move it up, before what is before it".to_owned(),
                    command("up"),
                ));
                items.push((
                    "move it down, behind what is behind it".to_owned(),
                    command("down"),
                ));
                if run.len() > 1 {
                    let file = page.file().unwrap_or("");
                    let count = run.len();
                    items.push((
                        format!("take out all {count} pages of {file} here"),
                        command("remove-run"),
                    ));
                    items.push((
                        format!("move the {count} pages of {file} up"),
                        command("up-run"),
                    ));
                    items.push((
                        format!("move the {count} pages of {file} down"),
                        command("down-run"),
                    ));
                }
                if let Some(file) = page.file() {
                    items.push((format!("file\t{file}"), command("file")));
                }
                if let Shows::Pdf { .. } = page.shows {
                    let numbers: Vec<usize> = pages[run]
                        .iter()
                        .filter_map(|page| match page.shows {
                            Shows::Pdf { page, .. } => Some(page),
                            _ => None,
                        })
                        .collect();
                    items.push((
                        format!("pages of the file\t{}", stretches(&numbers)),
                        command("range"),
                    ));
                }
                if page.shows != Shows::Blank {
                    let size = match page.fit {
                        Some(true) => "fit to the document's paper",
                        Some(false) => "as large as it is",
                        None => "as written by hand",
                    };
                    items.push((format!("size\t{size}"), command("fit")));
                }
                let counted = if page.counted { "yes" } else { "no" };
                items.push((
                    format!("counts in the page numbers\t{counted}"),
                    command("count"),
                ));
            }
            "page put in"
        } else if let Some(index) = key.strip_prefix('p') {
            let index: usize = index.parse().map_err(|_| format!("no page {key}"))?;
            items.push((format!("go to page {}", index + 1), command("go")));
            items.push((
                format!("put pages in before page {}", index + 1),
                Target::Command(format!("pages choose before-p{index}")),
            ));
            items.push((
                format!("put pages in behind page {}", index + 1),
                Target::Command(format!("pages choose after-p{index}")),
            ));
            "page"
        } else {
            return Err(format!("no page {key}"));
        };
        self.open_picker_for(title, items, 0, format!("pages list {key}"), None);
        Ok(())
    }

    /// Goes to the page `key` names: the cursor on it, where it is a
    /// page put in; else the page in view.
    fn go_to_page(&mut self, key: &str) -> Result<(), String> {
        if let Some(unit) = key.strip_prefix('u') {
            let unit: usize = unit.parse().map_err(|_| format!("no page {key}"))?;
            let pages = self.pages_put_in()?;
            let page = pages.get(unit).ok_or("the page is not there any more")?;
            let index = self
                .page_facts()
                .and_then(|facts| facts.iter().position(|fact| fact.unit == Some(unit)));
            let editor = self.editor.as_mut().ok_or(NOT_EDITING)?;
            editor.jump(page.range.start);
            if let Some(index) = index {
                self.cursor_page.set(index);
                self.show_page(index);
            } else {
                self.follow_cursor();
            }
            self.notice(format!("{}: {ON_A_PAGE}", page.called()));
            return Ok(());
        }
        let index: usize = key
            .strip_prefix('p')
            .and_then(|index| index.parse().ok())
            .ok_or_else(|| format!("no page {key}"))?;
        self.goto_page(index);
        Ok(())
    }

    /// Puts page `index` in view.
    fn show_page(&mut self, index: usize) {
        match self.paging {
            super::Paging::Slides => self.show_slide(index),
            super::Paging::Scroll => self.view.scroll_to_page(index, &self.layout),
        }
    }

    /// What is to be put in `place` (see [`Viewer::resolve_place`]): the
    /// files beside the document, a blank page, a page break, or a file
    /// typed.
    fn choose_what(&mut self, place: &str) -> Result<(), String> {
        let endings: Vec<&str> = page_text::PDF_ENDINGS
            .iter()
            .chain(page_text::IMAGE_ENDINGS)
            .copied()
            .collect();
        let mut files = self.files_beside(&endings);
        // PDF files first: what pages are mostly made of.
        files.sort_by_key(|file| !page_text::ends_in(file, page_text::PDF_ENDINGS));
        let mut items: Vec<(String, Target)> = files
            .into_iter()
            .map(|file| {
                let choose = Target::Command(format!("pages file-for {place} {file}"));
                (file, choose)
            })
            .collect();
        items.push((
            "a blank page".to_owned(),
            Target::Command(format!("pages range-for {place} :blank all")),
        ));
        items.push((
            "a page break: the text goes on on a new page".to_owned(),
            Target::Command(format!("pages range-for {place} :break all")),
        ));
        items.push((
            "or type the path of a PDF or an image, from anywhere".to_owned(),
            Target::Nothing,
        ));
        let free = Some(format!("pages file-for {place} "));
        self.open_picker_for("put in", items, 0, String::new(), free);
        self.picker_by_itself();
        Ok(())
    }

    /// Where the file `typed` is: as typed from the folder of the file
    /// edited, or from the home folder (`~/`), or from the root.
    fn resolve_file(&self, typed: &str) -> Result<String, String> {
        let editor = self.editor.as_ref().ok_or(NOT_EDITING)?;
        let typed = typed.trim().trim_matches('"');
        if typed.is_empty() {
            return Err("which file? :page add cover.pdf".to_owned());
        }
        let path = if let Some(home) = typed.strip_prefix("~/") {
            std::env::var_os("HOME")
                .map_or_else(|| PathBuf::from(typed), |dir| PathBuf::from(dir).join(home))
        } else {
            let path = PathBuf::from(typed);
            if path.is_absolute() {
                path
            } else {
                folder_of(editor.path()).join(path)
            }
        };
        if !path.is_file() {
            return Err(format!("there is no file {typed}"));
        }
        Ok(path.to_string_lossy().into_owned())
    }

    /// The pages of the file `typed` to put in `place`: asked for where
    /// it is a PDF of more than one page.
    fn ask_pages_of(&mut self, place: &str, typed: &str) -> Result<(), String> {
        let path = self.resolve_file(typed)?;
        let (made, name) = read_made(&path)?;
        let sizes = match made {
            Made::Pdf { sizes, .. } if sizes.len() > 1 => sizes,
            _ => return self.ask_where(place, &path, "all", false),
        };
        let count = sizes.len();
        let (width, height) = sizes[0];
        let set = format!("pages range-for {place} {} ", word_of(&path));
        let items = vec![
            (
                format!("all {count} pages"),
                Target::Command(format!("{set}all")),
            ),
            (
                "the first page".to_owned(),
                Target::Command(format!("{set}1")),
            ),
            (
                format!(
                    "or type the pages: 2, 1-3, 1,4-6\t{name}: {count} pages, {width:.0} × {height:.0} pt"
                ),
                Target::Nothing,
            ),
        ];
        self.open_picker_for(
            "which pages?",
            items,
            0,
            format!("pages choose {place}"),
            Some(set),
        );
        Ok(())
    }

    /// Where the pages `pages` of `file` are to go, where `place` is
    /// `ask`; else they go there. Many pages are asked about first,
    /// unless `sure`.
    fn ask_where(
        &mut self,
        place: &str,
        file: &str,
        pages: &str,
        sure: bool,
    ) -> Result<(), String> {
        let chosen = self.chosen_pages(file, pages)?;
        if !sure && chosen.len() > page_text::MANY_PAGES {
            let count = chosen.len();
            let file_word = word_of(file);
            let items = vec![
                (
                    format!("yes, all {count} pages"),
                    Target::Command(format!("pages range-for! {place} {file_word} {pages}")),
                ),
                (
                    "no, fewer".to_owned(),
                    Target::Command(format!("pages file-for {place} {file}")),
                ),
            ];
            self.open_picker_for(
                "so many pages?",
                items,
                1,
                format!("pages choose {place}"),
                None,
            );
            return Ok(());
        }
        if place != "ask" {
            return self.put_pages(place, file, pages);
        }
        let put =
            |place: &str| Target::Command(format!("pages put {place} {} {pages}", word_of(file)));
        let mut items = vec![(
            "after the paragraph the cursor is in: the text goes on on the next page".to_owned(),
            put("para"),
        )];
        if let Some(unit) = self.page_here() {
            items.push((
                "behind the page the cursor is on".to_owned(),
                put(&format!("after-u{unit}")),
            ));
            items.push((
                "before the page the cursor is on".to_owned(),
                put(&format!("before-u{unit}")),
            ));
        } else if self.pages_current() {
            let page = self.page_of_cursor();
            let number = page + 1;
            items.push((
                format!("behind page {number}, the page the cursor is on"),
                put(&format!("after-p{page}")),
            ));
            items.push((
                format!("before page {number}"),
                put(&format!("before-p{page}")),
            ));
        }
        items.push(("at the start of the document".to_owned(), put("start")));
        items.push(("at the end of the document".to_owned(), put("end")));
        self.open_picker("where?", items, 0);
        Ok(())
    }

    /// The pages of `file` that `pages` names, as they are to be
    /// written; one of a blank page or a page break for `:blank` and
    /// `:break`.
    fn chosen_pages(&self, file: &str, pages: &str) -> Result<Vec<Shows>, String> {
        match file {
            ":blank" => return Ok(vec![Shows::Blank]),
            ":break" => return Ok(vec![Shows::Other]),
            _ => {}
        }
        let (made, name) = read_made(file)?;
        Ok(match made {
            Made::Pdf { sizes, .. } => page_text::page_numbers(pages, sizes.len())?
                .into_iter()
                .map(|page| Shows::Pdf {
                    file: String::new(),
                    page,
                })
                .collect(),
            Made::Image if matches!(pages.trim(), "" | "all" | "1") => {
                vec![Shows::Image {
                    file: String::new(),
                }]
            }
            Made::Image => return Err(format!("{name} is an image: it is one page")),
        })
    }

    /// Where `place` is in the text, and how that is said: `para` after
    /// the paragraph the cursor is in, `start`, `end`, `before-u3` and
    /// `after-u3` beside the fourth page put in, `before-p5` and
    /// `after-p5` beside the sixth page of the document.
    fn resolve_place(&mut self, place: &str) -> Result<(usize, String), String> {
        let editor = self.editor.as_ref().ok_or(NOT_EDITING)?;
        let source = editor.source().clone();
        let pages = page_text::inserted(&source);
        match place {
            "para" | "ask" => Ok((
                page_text::after_paragraph(&source, editor.cursor()),
                "after the paragraph the cursor is in".to_owned(),
            )),
            "start" => Ok((page_text::content_start(&source), "at the start".to_owned())),
            "end" => Ok((
                page_text::content_end(source.text()),
                "at the end".to_owned(),
            )),
            _ => {
                let (side, key) = place
                    .split_once('-')
                    .ok_or_else(|| format!("no place {place}"))?;
                let after = side == "after";
                if let Some(unit) = key.strip_prefix('u') {
                    let page = unit
                        .parse::<usize>()
                        .ok()
                        .and_then(|unit| pages.get(unit))
                        .ok_or("that page is not there any more")?;
                    let at = if after {
                        page.range.end
                    } else {
                        page.range.start
                    };
                    let side = if after { "behind" } else { "before" };
                    return Ok((at, format!("{side} {}", page.called())));
                }
                let index: usize = key
                    .strip_prefix('p')
                    .and_then(|index| index.parse().ok())
                    .ok_or_else(|| format!("no place {place}"))?;
                let facts = self.page_facts().ok_or(NOT_TYPESET)?;
                let fact = facts.get(index).ok_or("there is no such page")?;
                let (first, last) = fact.shown.ok_or("that page shows nothing of this file")?;
                let number = index + 1;
                if after {
                    let at = page_text::after_paragraph(&source, last.saturating_sub(1));
                    let next = facts.get(index + 1).and_then(|fact| fact.shown);
                    let exact = next.is_none_or(|(first, _)| first >= at);
                    let told = if exact {
                        format!("behind page {number}")
                    } else {
                        format!(
                            "behind the paragraph page {number} ends in: no place is between \
                             page {number} and {} in the text, which runs on",
                            number + 1
                        )
                    };
                    Ok((at, told))
                } else {
                    let at = page_text::paragraph_start(&source, first);
                    let previous = index
                        .checked_sub(1)
                        .and_then(|before| facts.get(before)?.shown);
                    let exact = previous.is_none_or(|(_, last)| last <= at);
                    let told = if exact {
                        format!("before page {number}")
                    } else {
                        format!(
                            "before the paragraph page {number} begins with: no place is between \
                             page {} and {number} in the text, which runs on",
                            number - 1
                        )
                    };
                    Ok((at, told))
                }
            }
        }
    }

    /// Puts the pages `pages` of `file` (or a blank page, `:blank`, or
    /// a page break, `:break`) in at `place`, as one step: the file is
    /// brought in beside the document first, where it is not there.
    fn put_pages(&mut self, place: &str, file: &str, pages: &str) -> Result<(), String> {
        let shows = self.chosen_pages(file, pages)?;
        let (at, told) = self.resolve_place(place)?;
        let editor = self.editor.as_ref().ok_or(NOT_EDITING)?;
        let (block, what, copied) = match file {
            ":break" => (
                page_text::PAGE_BREAK.to_owned(),
                "a page break".to_owned(),
                None,
            ),
            ":blank" => (
                page_text::written(&Page {
                    shows: Shows::Blank,
                    fit: false,
                    counted: true,
                }),
                "a blank page".to_owned(),
                None,
            ),
            _ => {
                let folder = folder_of(editor.path());
                let into = self.folder_for_pages(&folder);
                let (name, copied) = page_text::brought_in(Path::new(file), &folder, &into)?;
                let written: Vec<Page> = shows
                    .iter()
                    .map(|shows| match shows {
                        Shows::Pdf { page, .. } => Page {
                            shows: Shows::Pdf {
                                file: name.clone(),
                                page: *page,
                            },
                            fit: false,
                            counted: true,
                        },
                        _ => Page {
                            shows: Shows::Image { file: name.clone() },
                            fit: true,
                            counted: true,
                        },
                    })
                    .collect();
                let what = match written.as_slice() {
                    [one] => Inserted::called_of(&one.shows),
                    _ => format!("{} pages of {name}", written.len()),
                };
                (page_text::written_all(&written), what, copied)
            }
        };
        let source = editor.source().clone();
        let units = page_text::inserted(&source);
        let (new, start) = page_text::with_put(source.text(), at, &block, &units);
        self.change_pages(&new, start)?;
        if let Some(path) = &copied {
            self.file_changed(path);
        }
        let copied = copied.map_or(String::new(), |path| {
            format!(
                "; {} copied to {}",
                Path::new(file)
                    .file_name()
                    .map_or(String::new(), |name| name.to_string_lossy().into_owned()),
                path.display()
            )
        });
        self.notice(format!("{what} put in {told}{copied}  (u takes it back)"));
        Ok(())
    }

    /// The folder pages from elsewhere are copied into: where the images
    /// beside the document are, if they are all in one folder below it;
    /// else beside the file edited, in `folder`.
    fn folder_for_pages(&self, folder: &Path) -> PathBuf {
        let images = self.images_beside();
        let mut folders = images
            .iter()
            .map(|image| image.rsplit_once('/').map_or("", |(dir, _)| dir));
        match folders.next() {
            Some(first) if !first.is_empty() && folders.all(|other| other == first) => {
                folder.join(first)
            }
            _ => folder.to_path_buf(),
        }
    }

    /// Makes the text `new`, as one step, and puts the cursor at byte
    /// `cursor` of it.
    fn change_pages(&mut self, new: &str, cursor: usize) -> Result<(), String> {
        let editor = self.editor.as_mut().ok_or(NOT_EDITING)?;
        let (range, with) = page_text::replacement(editor.text(), new);
        if range.is_empty() && with.is_empty() {
            return Ok(());
        }
        editor
            .replace(&[(range, with.as_str())])
            .map_err(|why| format!("not changed: {why}"))?;
        editor.jump(cursor.min(editor.text().len()));
        self.text_changed(Edited::Command);
        Ok(())
    }

    /// Takes out page put in `unit`, or all of the pages of its file
    /// that stand with it.
    fn remove_pages(&mut self, unit: usize, run: bool) -> Result<(), String> {
        let editor = self.editor.as_ref().ok_or(NOT_EDITING)?;
        let source = editor.source().clone();
        let pages = page_text::inserted(&source);
        let page = pages.get(unit).ok_or("the page is not there any more")?;
        if !page.is_known() {
            return Err("a page written by hand: not taken out here".to_owned());
        }
        let span = if run {
            page_text::run_of(source.text(), &pages, unit)
        } else {
            unit..unit + 1
        };
        let range = pages[span.start].range.start..pages[span.end - 1].range.end;
        let new = page_text::without(source.text(), range.clone(), &pages);
        let told = if span.len() > 1 {
            format!("{} pages of {}", span.len(), page.file().unwrap_or(""))
        } else {
            page.called()
        };
        let cursor = range.start.min(new.len());
        self.change_pages(&new, cursor)?;
        self.notice(format!("{told} taken out  (u brings it back)"));
        Ok(())
    }

    /// Moves page put in `unit` (or all of its file that stand with it)
    /// up or down; returns the index it has then.
    fn move_pages(&mut self, unit: usize, up: bool, run: bool) -> Result<usize, String> {
        let editor = self.editor.as_ref().ok_or(NOT_EDITING)?;
        let source = editor.source().clone();
        let pages = page_text::inserted(&source);
        let page = pages.get(unit).ok_or("the page is not there any more")?;
        if !page.is_known() {
            return Err("a page written by hand: not moved here".to_owned());
        }
        let span = if run {
            page_text::run_of(source.text(), &pages, unit)
        } else {
            unit..unit + 1
        };
        let (new, start, past_text) =
            page_text::moved(&source, &pages, span.start, span.end - 1, up)?;
        self.change_pages(&new, start)?;
        let now = page_text::inserted(&Source::detached(new.as_str()))
            .iter()
            .position(|page| page.range.start == start)
            .unwrap_or(0);
        let way = if up { "up" } else { "down" };
        let past = if past_text {
            "past the text, to the nearest page break: between two pages of running text \
             there is no place of its own"
        } else {
            "past the page put in next to it"
        };
        self.notice(format!(
            "{} moved {way}, {past}  (u takes it back)",
            page.called()
        ));
        Ok(now + (unit - span.start))
    }

    /// Switches page put in `unit` between as large as it is and fit to
    /// the paper (`fit`), or between counted and not.
    fn switch_page(&mut self, unit: usize, fit: bool) -> Result<(), String> {
        let editor = self.editor.as_ref().ok_or(NOT_EDITING)?;
        let source = editor.source().clone();
        let pages = page_text::inserted(&source);
        let page = pages.get(unit).ok_or("the page is not there any more")?;
        if !page.is_known() {
            return Err("a page written by hand: not changed here".to_owned());
        }
        let mut new = page.page();
        let told = if fit {
            new.fit = page.fit != Some(true);
            if new.fit {
                "fit to the document's paper"
            } else {
                "as large as it is"
            }
        } else {
            new.counted = !page.counted;
            if new.counted {
                "counted in the page numbers"
            } else {
                "not counted in the page numbers"
            }
        };
        let mut text = source.text().to_owned();
        text.replace_range(page.range.clone(), &page_text::written(&new));
        self.change_pages(&text, page.range.start)?;
        self.notice(format!("{}: {told}  (u takes it back)", page.called()));
        Ok(())
    }

    /// Asks which file page put in `key` (and those of its file beside
    /// it) is to show instead.
    fn ask_other_file(&mut self, key: &str) -> Result<(), String> {
        let endings: Vec<&str> = page_text::PDF_ENDINGS
            .iter()
            .chain(page_text::IMAGE_ENDINGS)
            .copied()
            .collect();
        let set = format!("pages file! {key} ");
        let mut items: Vec<(String, Target)> = self
            .files_beside(&endings)
            .into_iter()
            .map(|file| {
                (
                    file.clone(),
                    Target::Command(format!("{set}{}", word_of(&file))),
                )
            })
            .collect();
        items.push((
            "or type the path of a PDF or an image, from anywhere".to_owned(),
            Target::Nothing,
        ));
        self.open_picker_for("file", items, 0, format!("pages open {key}"), Some(set));
        Ok(())
    }

    /// Makes page put in `unit`, and those of its file beside it, show
    /// the file `typed`: the same pages of it, where it has them.
    fn other_file(&mut self, unit: usize, typed: &str) -> Result<(), String> {
        let path = self.resolve_file(typed)?;
        let (made, name) = read_made(&path)?;
        let editor = self.editor.as_ref().ok_or(NOT_EDITING)?;
        let source = editor.source().clone();
        let pages = page_text::inserted(&source);
        pages.get(unit).ok_or("the page is not there any more")?;
        let span = page_text::run_of(source.text(), &pages, unit);
        let folder = folder_of(editor.path());
        let into = self.folder_for_pages(&folder);
        let count = match &made {
            Made::Pdf { sizes, .. } => sizes.len(),
            Made::Image => 1,
        };
        let mut written = Vec::new();
        for page in &pages[span.clone()] {
            let mut new = page.page();
            new.shows = match (&made, &page.shows) {
                (Made::Pdf { .. }, Shows::Pdf { page, .. }) if *page > count => {
                    return Err(format!(
                        "{name} has {count} pages: there is no page {page} of it"
                    ));
                }
                (Made::Pdf { .. }, Shows::Pdf { page, .. }) => Shows::Pdf {
                    file: String::new(),
                    page: *page,
                },
                (Made::Pdf { .. }, _) => Shows::Pdf {
                    file: String::new(),
                    page: 1,
                },
                (Made::Image, _) => Shows::Image {
                    file: String::new(),
                },
            };
            written.push(new);
        }
        if matches!(made, Made::Image) {
            written.truncate(1);
        }
        let (named, copied) = page_text::brought_in(Path::new(&path), &folder, &into)?;
        for page in &mut written {
            page.shows = match &page.shows {
                Shows::Pdf { page, .. } => Shows::Pdf {
                    file: named.clone(),
                    page: *page,
                },
                _ => Shows::Image {
                    file: named.clone(),
                },
            };
        }
        let range = pages[span.start].range.start..pages[span.end - 1].range.end;
        let mut text = source.text().to_owned();
        text.replace_range(range.clone(), &page_text::written_all(&written));
        self.change_pages(&text, range.start)?;
        if let Some(path) = &copied {
            self.file_changed(path);
        }
        let copied = copied.map_or(String::new(), |path| {
            format!("; copied to {}", path.display())
        });
        self.notice(format!("now of {named}{copied}  (u takes it back)"));
        Ok(())
    }

    /// Asks which pages of its file page put in `key` and those of its
    /// file beside it are to be.
    fn ask_other_range(&mut self, key: &str) -> Result<(), String> {
        let unit = unit_key(key)?;
        let pages = self.pages_put_in()?;
        let page = pages.get(unit).ok_or("the page is not there any more")?;
        let Shows::Pdf { file, .. } = &page.shows else {
            return Err("only a page of a PDF has other pages".to_owned());
        };
        let editor = self.editor.as_ref().ok_or(NOT_EDITING)?;
        let path = folder_of(editor.path()).join(file);
        let (made, name) = read_made(&path.to_string_lossy())?;
        let count = match made {
            Made::Pdf { sizes, .. } => sizes.len(),
            Made::Image => 1,
        };
        let set = format!("pages range! {key} ");
        let items = vec![
            (
                format!("all {count} pages"),
                Target::Command(format!("{set}all")),
            ),
            (
                format!("or type the pages: 2, 1-3, 1,4-6\t{name}: {count} pages"),
                Target::Nothing,
            ),
        ];
        self.open_picker_for(
            "which pages?",
            items,
            0,
            format!("pages open {key}"),
            Some(set),
        );
        Ok(())
    }

    /// Makes page put in `unit`, with those of its file beside it, the
    /// pages `typed` of that file; returns the index of the first then.
    fn other_range(&mut self, unit: usize, typed: &str) -> Result<usize, String> {
        let editor = self.editor.as_ref().ok_or(NOT_EDITING)?;
        let source = editor.source().clone();
        let pages = page_text::inserted(&source);
        let page = pages.get(unit).ok_or("the page is not there any more")?;
        let Shows::Pdf { file, .. } = &page.shows else {
            return Err("only a page of a PDF has other pages".to_owned());
        };
        let path = folder_of(editor.path()).join(file);
        let (made, _) = read_made(&path.to_string_lossy())?;
        let Made::Pdf { sizes, .. } = made else {
            return Err(format!("{file} is no PDF any more"));
        };
        let numbers = page_text::page_numbers(typed, sizes.len())?;
        let span = page_text::run_of(source.text(), &pages, unit);
        let written: Vec<Page> = numbers
            .iter()
            .map(|number| Page {
                shows: Shows::Pdf {
                    file: file.clone(),
                    page: *number,
                },
                ..page.page()
            })
            .collect();
        let range = pages[span.start].range.start..pages[span.end - 1].range.end;
        let mut text = source.text().to_owned();
        text.replace_range(range.clone(), &page_text::written_all(&written));
        self.change_pages(&text, range.start)?;
        self.notice(format!(
            "pages {} of {file}  (u takes it back)",
            stretches(&numbers)
        ));
        Ok(span.start)
    }

    /// A key that may be for the page put in the cursor is on, in the
    /// page-only layout. Returns whether it was.
    pub(super) fn pages_key(&mut self, press: KeyPress) -> bool {
        let deleting = std::mem::take(&mut self.pages.deleting);
        let normal = self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.mode() == editor::Mode::Normal && editor.is_idle());
        if !normal || self.picker.is_some() || self.shown_panes() != Panes::Page || press.alt {
            return false;
        }
        let Some(unit) = self.page_here() else {
            return false;
        };
        let refused = |viewer: &mut Self| {
            let called = viewer
                .pages_put_in()
                .ok()
                .and_then(|pages| pages.get(unit).map(Inserted::called))
                .unwrap_or_default();
            viewer.notice(format!("{called}: a page put in whole: {ON_A_PAGE}"));
        };
        match (press.key, press.ctrl) {
            (Key::Char('x'), false) | (Key::Delete, _) => self.take_out_here(unit),
            (Key::Char('d'), false) if deleting => self.take_out_here(unit),
            (Key::Char('d'), false) => self.pages.deleting = true,
            (Key::Enter, _) => {
                if let Err(why) = self.open_page_entry(&format!("u{unit}")) {
                    self.alert(why);
                }
            }
            (
                Key::Char(
                    'i' | 'a' | 'I' | 'A' | 'o' | 'O' | 's' | 'S' | 'c' | 'C' | 'r' | 'R' | 'p'
                    | 'P' | '~' | 'J' | '.' | 'X' | 'D' | '>' | '<',
                ),
                false,
            ) => refused(self),
            _ => {
                if deleting {
                    // Not `dd` after all: the `d` is the text's.
                    self.editor_key(KeyPress::plain(Key::Char('d')));
                }
                return false;
            }
        }
        true
    }

    /// `x` or `dd` on a page put in: takes it out.
    fn take_out_here(&mut self, unit: usize) {
        if let Err(why) = self.remove_pages(unit, false) {
            self.alert(why);
        }
    }

    /// What is seen of pages put in on page `index` of the document on
    /// screen.
    fn seen_on(&mut self, index: usize) -> Option<Seen> {
        let document = self.document.clone()?;
        let world = self.compiled_world.clone()?;
        let key = std::sync::Arc::as_ptr(&document) as usize;
        if self.pages.seen.as_ref().is_none_or(|(of, _)| *of != key) {
            self.pages.seen = Some((key, HashMap::new()));
        }
        let file = self.file_id();
        let (_, seen) = self.pages.seen.as_mut()?;
        seen.entry(index)
            .or_insert_with(|| {
                let page = document.pages().get(index)?;
                let quick = is_empty(&page.frame) || {
                    let mut spans = Vec::new();
                    images_on(&page.frame, &mut spans);
                    !spans.is_empty()
                };
                if !quick {
                    return None;
                }
                let (_, pages) = written_pages(&world, file)?;
                let unit = unit_on(&document, &world, file, index, &pages)?;
                let page = &pages[unit];
                let mut called = page.called();
                if !page.counted {
                    called.push_str(", not counted");
                }
                Some(Seen {
                    range: page.range.clone(),
                    called,
                })
            })
            .clone()
    }

    /// What is drawn of a page put in, on page `index` drawn at
    /// `origin`: a tag beside it that says what it is, and a frame
    /// around it while the cursor is on it.
    pub(super) fn pages_marks(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        index: usize,
        origin: (f64, f64),
    ) {
        if self.editor.is_none() {
            return;
        }
        let Some(seen) = self.seen_on(index) else {
            return;
        };
        let Some(page_box) = self.layout.pages.get(index) else {
            return;
        };
        let (width, height) = (f64::from(page_box.width), f64::from(page_box.height));
        let zoom = self.view.zoom;
        // The tag: beside the page's top right corner, where there is
        // room; else on the page, in that corner.
        let markup = tag_markup(&seen.called, TAG_SIZE);
        if let Some((id, size, image)) = self.chrome.render_unless_held(&markup, zoom, has) {
            if let Some(image) = image
                && !scene.images.iter().any(|known| known.id == image.id)
            {
                scene.images.push(image);
            }
            let (tag_width, tag_height) = (f64::from(size.0), f64::from(size.1));
            let gap = 4.0 * zoom;
            let room = f64::from(self.window.0) - (origin.0 + width + gap);
            let x = if room >= tag_width {
                origin.0 + width + gap
            } else {
                origin.0 + width - tag_width - gap
            };
            scene.quads.push(Quad {
                texture: id,
                x: x.round() as f32,
                y: (origin.1 + gap).round() as f32,
                width: tag_width as f32,
                height: tag_height as f32,
            });
        }
        let on_it = self
            .places()
            .zip(self.editor.as_ref())
            .is_some_and(|(change, editor)| {
                editor.mode() == editor::Mode::Normal
                    && change
                        .back(editor.cursor())
                        .is_some_and(|then| seen.range.contains(&then))
            });
        if on_it {
            let bar = (3.0 * self.scale).round().max(2.0);
            for rect in [
                (origin.0, origin.1, width, bar),
                (origin.0, origin.1 + height - bar, width, bar),
                (origin.0, origin.1, bar, height),
                (origin.0 + width - bar, origin.1, bar, height),
            ] {
                tint(scene, has, FRAME, rect);
            }
            self.cursor_at
                .get_or_insert((origin.0, origin.1, origin.1 + height.min(40.0)));
        }
    }

    /// Puts pages of a file in for an agent, or proposes to.
    ///
    /// # Errors
    ///
    /// If the text is not at `expected_revision`, the file is not there
    /// or no PDF or image, the pages are not in it, or the place is
    /// none there is.
    pub fn agent_insert_pages(
        &mut self,
        wanted: &page_text::Wanted,
        expected_revision: Option<u64>,
        propose: bool,
    ) -> Result<Outcome, String> {
        self.expect(expected_revision)?;
        let editor = self.editor.as_ref().ok_or(NOT_EDITING)?;
        let folder = folder_of(editor.path());
        let path = folder.join(&wanted.file);
        if !path.is_file() {
            return Err(format!(
                "there is no file {} beside the document",
                wanted.file
            ));
        }
        let (made, name) = read_made(&path.to_string_lossy())?;
        let file = wanted.file.replace('\\', "/");
        let written: Vec<Page> = match made {
            Made::Pdf { sizes, .. } => {
                page_text::page_numbers(wanted.pages.as_deref().unwrap_or("all"), sizes.len())?
                    .into_iter()
                    .map(|page| Page {
                        shows: Shows::Pdf {
                            file: file.clone(),
                            page,
                        },
                        fit: wanted.fit.unwrap_or(false),
                        counted: wanted.counted.unwrap_or(true),
                    })
                    .collect()
            }
            Made::Image => {
                if wanted
                    .pages
                    .as_deref()
                    .is_some_and(|pages| !matches!(pages.trim(), "" | "all" | "1"))
                {
                    return Err(format!("{name} is an image: it is one page"));
                }
                vec![Page {
                    shows: Shows::Image { file },
                    fit: wanted.fit.unwrap_or(true),
                    counted: wanted.counted.unwrap_or(true),
                }]
            }
        };
        let source = editor.source().clone();
        let text = source.text();
        let place = wanted.place.as_deref().unwrap_or("cursor").trim();
        let line_start = |line: usize| -> Result<usize, String> {
            let lines = text.split_inclusive('\n').count().max(1);
            if line == 0 || line > lines {
                return Err(format!("there is no line {line}: the text has {lines}"));
            }
            Ok(text
                .split_inclusive('\n')
                .take(line - 1)
                .map(str::len)
                .sum())
        };
        let at = match place {
            "cursor" => page_text::after_paragraph(&source, editor.cursor()),
            "start" => page_text::content_start(&source),
            "end" => page_text::content_end(text),
            other => {
                let (side, line) = other.split_once(':').ok_or_else(|| {
                    format!(
                        "{other} is no place: cursor, start, end, after:<line> or before:<line>"
                    )
                })?;
                let line: usize = line
                    .trim()
                    .parse()
                    .map_err(|_| format!("{line} is no line number"))?;
                let start = line_start(line)?;
                match side.trim() {
                    "after" => page_text::after_paragraph(&source, start),
                    "before" => page_text::paragraph_start(&source, start),
                    _ => {
                        return Err(format!(
                            "{other} is no place: cursor, start, end, after:<line> or before:<line>"
                        ));
                    }
                }
            }
        };
        let units = page_text::inserted(&source);
        let (new, _) = page_text::with_put(text, at, &page_text::written_all(&written), &units);
        let (range, with) = page_text::replacement(text, &new);
        self.agent_change(&[range], &with, propose)
    }

    /// The pages put into the file edited, for an agent: each with its
    /// `line`, what it `shows` (`pdf`, `image`, `blank` or `other`), its
    /// `file` and `page`, whether it is `fit` to the paper, and whether it
    /// is `counted` in the page numbers.
    #[must_use]
    pub fn agent_pages_put_in(&self) -> Vec<serde_json::Value> {
        let Ok(pages) = self.pages_put_in() else {
            return Vec::new();
        };
        pages
            .iter()
            .map(|page| {
                let (shows, number) = match &page.shows {
                    Shows::Pdf { page, .. } => ("pdf", Some(*page)),
                    Shows::Image { .. } => ("image", None),
                    Shows::Blank => ("blank", None),
                    Shows::Other => ("other", None),
                };
                serde_json::json!({
                    "line": page.line,
                    "shows": shows,
                    "file": page.file(),
                    "page": number,
                    "fit": page.fit,
                    "counted": page.counted,
                })
            })
            .collect()
    }
}

impl Inserted {
    /// What a page that shows `shows` is called.
    fn called_of(shows: &Shows) -> String {
        Inserted {
            range: 0..0,
            line: 0,
            shows: shows.clone(),
            fit: None,
            counted: true,
        }
        .called()
    }
}

/// The index of a page put in that `key` (`u3`) names.
fn unit_key(key: &str) -> Result<usize, String> {
    key.trim()
        .strip_prefix('u')
        .and_then(|unit| unit.parse().ok())
        .ok_or_else(|| format!("no page put in {key}"))
}

/// How a page put in is listed besides what it is.
fn described(page: &Inserted) -> String {
    let mut said = match (&page.shows, page.fit) {
        (Shows::Blank, _) => String::new(),
        (Shows::Other, _) => "written by hand".to_owned(),
        (_, Some(true)) => "fit to the paper".to_owned(),
        (_, Some(false)) => "as large as it is".to_owned(),
        (_, None) => "its own way".to_owned(),
    };
    if !page.counted {
        if !said.is_empty() {
            said.push_str(", ");
        }
        said.push_str("not counted");
    }
    said
}

/// Page numbers as stretches: `1-3, 5`.
fn stretches(numbers: &[usize]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut index = 0;
    while index < numbers.len() {
        let mut last = index;
        while last + 1 < numbers.len() && numbers[last + 1] == numbers[last] + 1 {
            last += 1;
        }
        parts.push(if last > index {
            format!("{}-{}", numbers[index], numbers[last])
        } else {
            numbers[index].to_string()
        });
        index = last + 1;
    }
    parts.join(", ")
}

/// The folder the file at `path` is in.
fn folder_of(path: &Path) -> PathBuf {
    match path.parent() {
        Some(folder) if !folder.as_os_str().is_empty() => folder.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// What the file at `path` is, and its name.
fn read_made(path: &str) -> Result<(Made, String), String> {
    let name = Path::new(path).file_name().map_or_else(
        || path.to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let bytes = std::fs::read(path).map_err(|why| match why.kind() {
        std::io::ErrorKind::NotFound => format!("there is no file {path}"),
        _ => format!("{name} cannot be read: {why}"),
    })?;
    let made = page_text::made_of(&name, &bytes)?;
    Ok((made, name))
}
