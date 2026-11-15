//! Beside the page: what it shows as nothing, and what is wrong with it.
//!
//! A label, a comment, a forced break, a space no line breaks at, room
//! made by `#v` or `#h`, a rule in the middle of the text, an `#include`:
//! the page shows none of them (see [`crate::invisible`]). Each gets a
//! small faint tag in the right margin, beside the line of the text it
//! goes with; the relative line numbers stand in the left one (see
//! `aids`). `]i` and
//! `[i` go to the next and the previous of them; on one, `x` (or `dd`)
//! takes it out, and Enter (or `c`) types a label, a comment or an amount
//! of room anew in a small prompt. `:invisibles` lists them all.
//!
//! What the compiler complains about is marked there too: a faint red bar
//! beside the line for an error, a yellow one for a warning, the message
//! beside it, shortened (whole in the status line while the cursor is on
//! that line), and a faint ground under the text it points at. Where that
//! text shows nothing, the nearest line that shows something is marked.
//!
//! All of it is worked out when a compile lands, of the pages that compile
//! made, and of each page the first time it is drawn after that: a frame
//! only draws it, and draws no image the compositor holds. `:tags` and
//! `:errors` turn either off; tags are shown only where the text is
//! edited, as only there keys reach what they name. None of it is part
//! of the document, of what is exported or of what an agent sees.

use std::collections::HashMap;
use std::fmt::Write;
use std::ops::Range;
use std::sync::Arc;

use typst::layout::{Frame, Rect};
use typst::syntax::FileId;
use typst_layout::PagedDocument;
use zetwerk_typst::{Origin, PageLines, PageMap, ProjectWorld};

use super::assist::Target;
use super::{
    Change, Difference, Edited, Message, Quad, Scene, TextureId, Viewer, difference, tint,
};
use crate::chrome::{self, Chrome};
use crate::editor;
use crate::invisible::{self, Kind, Thing};
use crate::keys::{Key, KeyPress};

/// How far right of the text of a page its marks stand, and how large
/// their text is, in points of the page.
const GAP: f64 = 6.0;
const SIZE: f64 = 5.5;
/// How many characters of a complaint are shown beside its line, and how
/// many tags a line has before the rest are only counted.
const MESSAGE: usize = 36;
const TAGS: usize = 3;
/// How many pages on a page with text is looked for.
const NEIGHBOURS: usize = 8;
/// How many image sizes are remembered before starting over.
const SIZES_KEPT: usize = 4000;
/// The font of the marks: the interface's.
const FONT: &str = "DejaVu Sans Mono";

/// The ground under text the compiler complains about, and the bar beside
/// its line, for errors and for warnings. Colours are RGBA.
pub(super) const PAGE_ERROR: [u8; 4] = [0xcc, 0x24, 0x1d, 0x2c];
pub(super) const PAGE_WARNING: [u8; 4] = [0xd7, 0x99, 0x21, 0x2c];
pub(super) const ERROR_BAR: [u8; 4] = [0xcc, 0x24, 0x1d, 0xa0];
pub(super) const WARNING_BAR: [u8; 4] = [0xd7, 0x99, 0x21, 0xa0];

/// The colours of the text of the marks: a tag, the tag the cursor is
/// on, an error and a warning.
const TAG: &str = "#928374";
const CHOSEN: &str = "#af3a03";
const ERROR: &str = "#cc241d";
const WARNING: &str = "#b57614";

/// What is known beside the pages.
#[derive(Default)]
pub(super) struct Margin {
    /// Whether the tags, and the complaints, are left out.
    hide_tags: bool,
    hide_errors: bool,
    /// The pages the marks are of.
    basis: Option<Basis>,
    /// When the last compile failed, the text it compiled: the bytes of
    /// its complaints are bytes of that.
    failed: Option<Arc<ProjectWorld>>,
    /// The things shown as nothing and the complaints, in bytes of the
    /// text of those pages.
    items: Vec<Item>,
    /// What each page drawn so far shows, and the lines on it that have
    /// marks.
    pages: HashMap<usize, Facts>,
    notes: HashMap<usize, Vec<Note>>,
    /// The sizes of the images of marks made, by their names.
    sizes: HashMap<TextureId, (u32, u32)>,
    /// The thing `]i` went to, while keys are for it.
    chosen: Option<Chosen>,
    /// A `d` waiting for its second.
    deleting: bool,
    /// Where the thing keys were for began, while the keys of `]i` or
    /// `[i` are typed: on the way the cursor is put on what is shown.
    from: Option<(u64, usize)>,
    /// What is being typed for a thing, while it is.
    prompt: Option<Chosen>,
    /// The line of each page the cursor is on, for the text, the cursor
    /// and the pages it was found for.
    cursor_on: Option<CursorOn>,
}

/// The line of each page asked about that the cursor is on, and the
/// revision of the text, the cursor and the revision of the pages that
/// is for.
type CursorOn = ((u64, usize, Option<u64>), HashMap<usize, Option<usize>>);

/// The pages marks are put beside: the document, the sources it was made
/// of, the file whose text is marked, and the revision of that text.
struct Basis {
    world: Arc<ProjectWorld>,
    document: Arc<PagedDocument>,
    file: FileId,
    revision: Option<u64>,
}

/// Something to put beside the line it belongs to.
struct Item {
    what: What,
    /// Its own bytes, if they may show (those of a complaint).
    own: Option<Range<usize>>,
    /// The ends of the shown text before and after it, and which of them
    /// it goes with (see [`Thing`]).
    before: Option<Range<usize>>,
    after: Option<Range<usize>>,
    with_before: bool,
}

/// What is put beside a line.
enum What {
    /// A thing the page shows as nothing: where it begins, and its tag.
    Thing { start: usize, name: String },
    /// A complaint of the compiler: an error or a warning, and what it
    /// says.
    Complaint { error: bool, message: String },
}

/// What a page shows of the file marked.
struct Facts {
    /// The bytes its glyphs show and the lines they are on, by where
    /// they begin.
    spans: Vec<(usize, usize, usize)>,
    lines: PageLines,
    /// Where the text of the page ends on the right, in points.
    right: f64,
    /// The cells of what is complained about, and whether by an error.
    grounds: Vec<(Rect, bool)>,
}

/// A line of a page with marks: which line, from where to where down the
/// page it reaches, and what is beside it (indices of
/// [`Margin::items`]).
#[derive(Clone)]
struct Note {
    line: usize,
    top: f64,
    bottom: f64,
    items: Vec<usize>,
}

/// A thing of the text that keys are for: the revision of the text it is
/// in, its bytes, what it is, and, in a prompt, what is typed for it.
struct Chosen {
    revision: u64,
    range: Range<usize>,
    kind: Kind,
    typed: String,
}

/// The bytes of `file` a glyph shows, if it shows any of it.
fn shown_by(origin: &Origin, file: FileId) -> Option<&Range<usize>> {
    match origin {
        Origin::Text { file: of, range }
        | Origin::Math { file: of, range }
        | Origin::Object {
            file: of, range, ..
        } if *of == file => Some(range),
        _ => None,
    }
}

/// How `new` differs from `old`, as one replacement.
fn change_between(old: &str, new: &str) -> Change {
    match difference(old, new) {
        Difference::Nothing => Change::NONE,
        Difference::Replacement {
            offset,
            removed,
            text,
        } => Change {
            offset,
            removed,
            inserted: text.len(),
        },
    }
}

/// `text` cut to `most` characters.
fn cut(text: &str, most: usize) -> String {
    let mut shown: String = text.chars().take(most).collect();
    if shown.len() < text.len() {
        shown.push('\u{2026}');
    }
    shown.replace('\n', " ")
}

/// `text` as a Typst string literal: never read as markup.
fn literal(text: &str) -> String {
    let mut written = String::with_capacity(text.len() + 2);
    written.push('"');
    for c in text.chars() {
        match c {
            '\\' => written.push_str("\\\\"),
            '"' => written.push_str("\\\""),
            '\n' | '\r' | '\t' => written.push(' '),
            c => written.push(c),
        }
    }
    written.push('"');
    written
}

/// Markup for the marks beside one line: pieces of text, each in its
/// colour, set apart a little.
fn note_markup(pieces: &[(String, &str)]) -> String {
    let mut markup = format!(
        "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n\
         #set text(font: \"{FONT}\", size: {SIZE}pt, top-edge: \"ascender\", \
         bottom-edge: \"descender\")\n"
    );
    for (index, (text, colour)) in pieces.iter().enumerate() {
        if index > 0 {
            markup.push_str("#h(0.9em)");
        }
        let _ = write!(
            markup,
            "#text(fill: rgb(\"{colour}\").transparentize(20%), {})",
            literal(text)
        );
    }
    markup
}

/// What the status line says of `thing` when the cursor comes to it.
fn told(thing: &Thing) -> String {
    let what = match thing.kind {
        Kind::Label => format!("label {}: x takes it out, Enter names it anew", thing.name),
        Kind::Comment => format!("comment {}: x takes it out, Enter says it anew", thing.name),
        Kind::Spacing => format!("{}: x takes it out, Enter changes it", thing.name),
        Kind::NoBreak => "a space no line breaks at: x makes it a plain one".to_owned(),
        Kind::PageBreak | Kind::ColumnBreak | Kind::Include => {
            format!("{}: x takes it out", thing.name)
        }
        Kind::Rule => format!("rule {}: x takes it out", thing.name),
    };
    format!("{what}  (]i [i: the others)")
}

impl Facts {
    /// What the page whose frame is `frame` shows of `file`.
    fn of(world: &ProjectWorld, frame: &Frame, file: FileId, items: &[Item]) -> Self {
        let map = PageMap::build(world, frame);
        let lines = map.lines();
        let boxes: Vec<(f64, f64, f64, f64)> = (0..lines.len())
            .filter_map(|line| {
                let (top, bottom) = lines.band(line)?;
                let (left, right) = lines.extent(line)?;
                Some((top, bottom, left, right))
            })
            .collect();
        // The same bytes may be shown twice on a page (a heading and its
        // entry in the table of contents): each glyph with its line.
        let mut spans: Vec<(usize, usize, usize)> = Vec::new();
        for glyph in map.glyphs().iter().filter(|glyph| !glyph.furniture) {
            let Some(range) = shown_by(&glyph.origin, file).filter(|range| !range.is_empty())
            else {
                continue;
            };
            let x = (glyph.rect.min.x + glyph.rect.max.x).to_pt() / 2.0;
            let y = (glyph.rect.min.y + glyph.rect.max.y).to_pt() / 2.0;
            let line = boxes.iter().position(|(top, bottom, left, right)| {
                (*top..=*bottom).contains(&y) && (left - 0.5..=right + 0.5).contains(&x)
            });
            if let Some(line) = line {
                spans.push((range.start, range.end, line));
            }
        }
        spans.sort_unstable();
        spans.dedup();
        // As far right as the text may reach, if the margins are alike:
        // the marks are not to move along as a line typed grows longer.
        let left = boxes.iter().map(|(_, _, left, _)| *left).reduce(f64::min);
        let right = boxes
            .iter()
            .map(|(_, _, _, right)| *right)
            .chain(left.map(|left| frame.width().to_pt() - left))
            .reduce(f64::max)
            .unwrap_or(0.0);
        let mut grounds = Vec::new();
        for item in items {
            if let (What::Complaint { error, .. }, Some(own)) = (&item.what, &item.own) {
                let rects = map.selection(file, own);
                grounds.extend(rects.into_iter().map(|rect| (rect, *error)));
            }
        }
        Self {
            spans,
            lines,
            right,
            grounds,
        }
    }

    /// Where the glyphs that show some of `window` begin: the last of
    /// them (`last`) or the first.
    fn edge_in(&self, window: &Range<usize>, last: bool) -> Option<usize> {
        let ahead = self
            .spans
            .partition_point(|(start, ..)| *start < window.end);
        // Glyphs that begin before the window and reach into it are few
        // and near (a call, an equation): only those are looked at.
        let from = ahead.saturating_sub(64);
        let within = self.spans[from..ahead]
            .iter()
            .filter(|(_, end, _)| *end > window.start)
            .map(|(start, ..)| *start);
        if last { within.max() } else { within.min() }
    }

    fn shows(&self, window: &Range<usize>) -> bool {
        self.edge_in(window, true).is_some()
    }

    /// The line of the glyph that begins at `start`; of several, the one
    /// nearest to line `near`, or else the last: a table of contents
    /// comes before the text it lists.
    fn line_at(&self, start: usize, near: Option<usize>) -> Option<usize> {
        let first = self.spans.partition_point(|(of, ..)| *of < start);
        let lines = self.spans[first..]
            .iter()
            .take_while(|(of, ..)| *of == start)
            .map(|(.., line)| *line);
        match near {
            Some(near) => lines.min_by_key(|line| line.abs_diff(near)),
            None => lines.max(),
        }
    }

    /// The line of the last (`last`) or the first glyph showing some of
    /// `window`.
    fn line_in(&self, window: &Range<usize>, last: bool) -> Option<usize> {
        self.line_at(self.edge_in(window, last)?, None)
    }

    /// Whether `at` is where the last (`last`) or the first glyph of the
    /// page with text of the file begins.
    fn ends_at(&self, at: usize, last: bool) -> bool {
        let edge = if last {
            self.spans.last()
        } else {
            self.spans.first()
        };
        edge.map(|(start, ..)| *start) == Some(at)
    }
}

impl Margin {
    /// What page `index` shows, worked out the first time it is asked.
    fn facts(&mut self, index: usize) -> Option<&Facts> {
        if !self.pages.contains_key(&index) {
            let basis = self.basis.as_ref()?;
            let page = basis.document.pages().get(index)?;
            let facts = Facts::of(&basis.world, &page.frame, basis.file, &self.items);
            self.pages.insert(index, facts);
        }
        self.pages.get(&index)
    }

    /// The next page after `from` (before it, if not `forward`) with
    /// text of the file on it, near enough to look for.
    fn text_page(&mut self, from: usize, forward: bool) -> Option<usize> {
        let mut index = from;
        for _ in 0..NEIGHBOURS {
            index = if forward {
                index + 1
            } else {
                index.checked_sub(1)?
            };
            if !self.facts(index)?.spans.is_empty() {
                return Some(index);
            }
        }
        None
    }

    /// Whether the page with text next to page `from`, that way, shows
    /// some of `window`. `None` if there is no such page.
    fn beside_shows(&mut self, from: usize, forward: bool, window: &Range<usize>) -> Option<bool> {
        let other = self.text_page(from, forward)?;
        Some(self.pages.get(&other)?.shows(window))
    }

    /// The line of page `index` that item `item` is put beside. `None` if
    /// it belongs on another page.
    fn anchor(&mut self, index: usize, item: usize) -> Option<usize> {
        let Item {
            own,
            before,
            after,
            with_before,
            ..
        } = self.items.get(item)?;
        let (own, with_before) = (own.clone(), *with_before);
        let (primary, other) = if with_before {
            (before.clone(), after.clone())
        } else {
            (after.clone(), before.clone())
        };
        let facts = self.facts(index)?;
        if let Some(line) = own.and_then(|own| facts.line_in(&own, false)) {
            return Some(line);
        }
        let at = primary
            .as_ref()
            .and_then(|primary| facts.edge_in(primary, with_before));
        let Some(at) = at else {
            // What it goes with shows nothing (a figure that is a picture
            // alone): it stands by the text on its other side, unless
            // the page next to this one has what it goes with.
            let line = facts.line_in(other.as_ref()?, !with_before)?;
            if let Some(primary) = primary
                && self.beside_shows(index, !with_before, &primary) == Some(true)
            {
                return None;
            }
            return Some(line);
        };
        let Some(other) = other else {
            return facts.line_at(at, None);
        };
        if let Some(near) = facts.line_in(&other, !with_before) {
            return facts.line_at(at, Some(near));
        }
        // The text on its other side is not on this page. Either it is
        // on the next page (the previous one), and this one ends (begins)
        // with the text it goes with; or the text it goes with is shown
        // here once more, as in a table of contents.
        if !facts.ends_at(at, with_before) {
            return None;
        }
        let line = facts.line_at(at, None)?;
        let there = self.beside_shows(index, with_before, &other);
        there.unwrap_or(true).then_some(line)
    }

    /// The lines of page `index` that have marks, worked out the first
    /// time they are asked for.
    fn notes(&mut self, index: usize) -> Option<&[Note]> {
        if !self.notes.contains_key(&index) {
            self.facts(index)?;
            let mut notes: Vec<Note> = Vec::new();
            for item in 0..self.items.len() {
                let Some(line) = self.anchor(index, item) else {
                    continue;
                };
                if let Some(note) = notes.iter_mut().find(|note| note.line == line) {
                    note.items.push(item);
                    continue;
                }
                let lines = &self.pages.get(&index)?.lines;
                if let Some((top, bottom)) = lines.band(line) {
                    notes.push(Note {
                        line,
                        top,
                        bottom,
                        items: vec![item],
                    });
                }
            }
            self.notes.insert(index, notes);
        }
        self.notes.get(&index).map(Vec::as_slice)
    }

    /// The text of the marks beside a line: the first complaint, then
    /// the tags if `tags_wanted`. The one of the thing that begins at
    /// `chosen` stands out.
    fn pieces(
        &self,
        note: &Note,
        chosen: Option<usize>,
        tags_wanted: bool,
    ) -> Vec<(String, &'static str)> {
        let mut pieces = Vec::new();
        let mut complaint = None;
        let mut complaints = 0_usize;
        let mut tags = Vec::new();
        for item in note.items.iter().filter_map(|item| self.items.get(*item)) {
            match &item.what {
                What::Complaint { error, message } if !self.hide_errors => {
                    complaints += 1;
                    complaint = complaint.or(Some((*error, message)));
                }
                What::Thing { start, name } if tags_wanted => {
                    let colour = if chosen == Some(*start) { CHOSEN } else { TAG };
                    tags.push((name.clone(), colour));
                }
                _ => {}
            }
        }
        if let Some((error, message)) = complaint {
            let more = match complaints {
                1 => String::new(),
                count => format!(" (+{})", count - 1),
            };
            let colour = if error { ERROR } else { WARNING };
            let said = super::refused::for_writer(message);
            pieces.push((format!("{}{more}", cut(&said, MESSAGE)), colour));
        }
        let hidden = tags.len().saturating_sub(TAGS);
        pieces.extend(tags.into_iter().take(TAGS));
        if hidden > 0 {
            pieces.push((format!("+{hidden}"), TAG));
        }
        pieces
    }
}

impl Viewer {
    /// A compile landed, made of `world`: what is marked is worked out
    /// anew, of its pages if it made any, else of the last that did.
    pub(super) fn margin_compiled(&mut self, world: &Arc<ProjectWorld>) {
        let succeeded = self.document_revision == self.compiled_revision;
        if succeeded && let Some(document) = &self.whole {
            self.margin.basis = Some(Basis {
                world: Arc::clone(world),
                document: Arc::clone(document),
                file: self.file_id(),
                revision: self.document_revision,
            });
        }
        self.margin.failed = (!succeeded).then(|| Arc::clone(world));
        self.margin_rebuild();
    }

    /// Finds what is to be marked beside the pages of the basis, for the
    /// file being edited.
    fn margin_rebuild(&mut self) {
        let file = self.file_id();
        let margin = &mut self.margin;
        margin.items.clear();
        margin.pages.clear();
        margin.notes.clear();
        margin.cursor_on = None;
        let Some(basis) = &mut margin.basis else {
            return;
        };
        basis.file = file;
        let Ok(source) = basis.world.written(file) else {
            return;
        };
        let found = invisible::find(&source);
        // The complaints are of the text compiled last; where that did
        // not compile, the pages are of an older one.
        let failed = margin
            .failed
            .as_ref()
            .and_then(|world| world.written(file).ok());
        let change = failed.map_or(Change::NONE, |failed| {
            change_between(source.text(), failed.text())
        });
        for (of, range, error, message) in &self.diagnostics {
            if *of != file {
                continue;
            }
            let start = change.back_or_edge(range.start);
            let end = change.back_or_edge(range.end).max(start + 1);
            let own = start..end.min(source.text().len()).max(start);
            let (before, after) = found.beside(&own);
            let with_before = found.goes_before(&own, after.as_ref());
            margin.items.push(Item {
                what: What::Complaint {
                    error: *error,
                    message: message.clone(),
                },
                own: Some(own),
                before,
                after,
                with_before,
            });
        }
        for thing in found.things {
            margin.items.push(Item {
                what: What::Thing {
                    start: thing.range.start,
                    name: thing.name,
                },
                own: None,
                before: thing.before,
                after: thing.after,
                with_before: thing.with_before,
            });
        }
    }

    /// Where the thing keys are for begins in the text of the pages
    /// marked, if they show it.
    fn chosen_in_basis(&self) -> Option<usize> {
        let chosen = self.margin.chosen.as_ref()?;
        let basis = self.margin.basis.as_ref()?;
        if chosen.revision != self.revision || basis.revision != self.document_revision {
            return None;
        }
        self.places_of_text()?.back(chosen.range.start)
    }

    /// The line of page `index` the cursor is on, if it is on one there.
    fn margin_cursor_line(&mut self, index: usize) -> Option<usize> {
        let cursor = self.editor.as_ref()?.cursor();
        let key = (self.revision, cursor, self.document_revision);
        if self
            .margin
            .cursor_on
            .as_ref()
            .is_none_or(|(of, _)| *of != key)
        {
            self.margin.cursor_on = Some((key, HashMap::new()));
        }
        if let Some((_, known)) = &self.margin.cursor_on
            && let Some(line) = known.get(&index)
        {
            return *line;
        }
        let basis = self.margin.basis.as_ref()?;
        let then = (basis.revision == self.document_revision)
            .then(|| self.places_of_text())
            .flatten()
            .and_then(|change| change.back(cursor));
        let line = then.and_then(|then| self.margin.facts(index)?.line_in(&(then..then + 1), true));
        if let Some((_, known)) = &mut self.margin.cursor_on {
            known.insert(index, line);
        }
        line
    }

    /// Draws beside the lines of page `index` (whose top-left corner is
    /// at `origin`) what it shows as nothing and what is wrong there.
    pub(super) fn margin_page(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        index: usize,
        origin: (f64, f64),
    ) {
        // Tags are of text that keys can reach: only where it is edited.
        let tags_wanted = !self.margin.hide_tags && self.editor.is_some();
        if !tags_wanted && self.margin.hide_errors {
            return;
        }
        let file = self.file_id();
        if self
            .margin
            .basis
            .as_ref()
            .is_some_and(|basis| basis.file != file)
        {
            self.margin_rebuild();
        }
        let chosen = self.chosen_in_basis();
        let cursor_line = self.margin_cursor_line(index);
        let Some(notes) = self.margin.notes(index).map(<[Note]>::to_vec) else {
            return;
        };
        let Some((right, grounds)) = self
            .margin
            .pages
            .get(&index)
            .map(|facts| (facts.right, facts.grounds.clone()))
        else {
            return;
        };
        let zoom = self.view.zoom;
        let place = |rect: &Rect| {
            (
                origin.0 + rect.min.x.to_pt() * zoom,
                origin.1 + rect.min.y.to_pt() * zoom,
                (rect.max.x - rect.min.x).to_pt() * zoom,
                (rect.max.y - rect.min.y).to_pt() * zoom,
            )
        };
        let typed_on = self.margin_typed_on(&notes);
        let grounds: Vec<_> = grounds
            .into_iter()
            .filter(|(rect, _)| {
                typed_on.is_none_or(|(_, top, bottom)| {
                    rect.max.y.to_pt() <= top || rect.min.y.to_pt() >= bottom
                })
            })
            .collect();
        if !self.margin.hide_errors {
            for (rect, error) in &grounds {
                let colour = if *error { PAGE_ERROR } else { PAGE_WARNING };
                tint(scene, has, colour, place(rect));
            }
        }
        let bar = (1.5 * self.scale).round().max(1.0);
        for note in &notes {
            let mut pieces = self.margin.pieces(note, chosen, tags_wanted);
            let mut complaint = self.margin_complaint(note);
            if typed_on.is_some_and(|(line, ..)| line == note.line) && complaint.take().is_some() {
                pieces.remove(0);
            }
            if pieces.is_empty() {
                continue;
            }
            let (top, height) = (origin.1 + note.top * zoom, (note.bottom - note.top) * zoom);
            if let Some((error, message)) = &complaint {
                let colour = if *error { ERROR_BAR } else { WARNING_BAR };
                let x = origin.0 + (right + GAP * 0.4) * zoom;
                tint(scene, has, colour, (x.round(), top, bar, height));
                let typing = self
                    .editor
                    .as_ref()
                    .is_some_and(|editor| editor.mode() == editor::Mode::Insert);
                if cursor_line == Some(note.line) && self.message.is_none() && !typing {
                    let kind = if *error { "error" } else { "warning" };
                    self.message = Some(Message {
                        text: format!("{kind}: {message}"),
                        alert: *error,
                    });
                }
            }
            let markup = note_markup(&pieces);
            let Some((id, width, image_height)) = self.margin_image(scene, has, &markup, zoom)
            else {
                continue;
            };
            scene.quads.push(Quad {
                texture: id,
                x: (origin.0 + (right + GAP) * zoom).round() as f32,
                y: (top + height / 2.0 - image_height / 2.0).round() as f32,
                width: width as f32,
                height: image_height as f32,
            });
        }
    }

    /// Of `notes`, the one beside the line typed on, while what is typed
    /// is complained of: its line, top and bottom. What the compiler
    /// makes of half a word is not worth reading; as in the status line,
    /// it waits until typing is over. The complaint is that of what
    /// begins before the cursor on its line. Not in an equation.
    fn margin_typed_on(&self, notes: &[Note]) -> Option<(usize, f64, f64)> {
        let editor = self.editor.as_ref()?;
        let (text, cursor, file) = (editor.text(), editor.cursor(), editor.file());
        if editor.mode() != editor::Mode::Insert
            || crate::typst_text::equation_at(editor.source(), cursor).is_some()
        {
            return None;
        }
        let (.., typed) = self.diagnostics.iter().find(|(of, range, ..)| {
            *of == file
                && text
                    .get(range.start..cursor)
                    .is_some_and(|between| !between.contains('\n'))
        })?;
        notes
            .iter()
            .find(|note| {
                self.margin_complaint(note)
                    .is_some_and(|(_, message)| message == *typed)
            })
            .map(|note| (note.line, note.top, note.bottom))
    }

    /// The text of the marks beside the lines of page `index`, as last
    /// drawn.
    #[cfg(test)]
    pub(super) fn margin_texts(&self, index: usize) -> Vec<String> {
        let notes = self.margin.notes.get(&index).map_or(&[][..], Vec::as_slice);
        notes
            .iter()
            .flat_map(|note| self.margin.pieces(note, None, true))
            .map(|(text, _)| text)
            .collect()
    }

    /// The first complaint beside `note`, if complaints are shown: whether
    /// it is an error, and what it says.
    fn margin_complaint(&self, note: &Note) -> Option<(bool, String)> {
        if self.margin.hide_errors {
            return None;
        }
        note.items
            .iter()
            .find_map(|item| match &self.margin.items.get(*item)?.what {
                What::Complaint { error, message } => Some((*error, message.clone())),
                What::Thing { .. } => None,
            })
    }

    /// The image of `markup` at `zoom`: its name and size, made only if
    /// the compositor does not hold it.
    fn margin_image(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        markup: &str,
        zoom: f64,
    ) -> Option<(TextureId, f64, f64)> {
        let id = Chrome::id(markup, zoom);
        if has(id)
            && let Some((width, height)) = self.margin.sizes.get(&id)
        {
            return Some((id, f64::from(*width), f64::from(*height)));
        }
        let image = self.chrome.render(markup, zoom)?;
        let size = (image.pixmap.width(), image.pixmap.height());
        if self.margin.sizes.len() >= SIZES_KEPT {
            self.margin.sizes.clear();
        }
        self.margin.sizes.insert(id, size);
        if !has(id) && !scene.images.iter().any(|known| known.id == id) {
            scene.images.push(image);
        }
        Some((id, f64::from(size.0), f64::from(size.1)))
    }

    /// The prompt in which a thing is typed anew, over everything else.
    pub(super) fn margin_panel(&mut self, scene: &mut Scene, has: &dyn Fn(TextureId) -> bool) {
        let Some(prompt) = &self.margin.prompt else {
            return;
        };
        let lines = [
            format!("{}: {}\u{2588}", prompt.kind.called(), prompt.typed),
            "Enter: so it is   Ctrl-U: all of it anew   Esc: leave it".to_owned(),
        ];
        let markup = chrome::text_markup(&lines);
        let (width, height) = (f64::from(self.window.0), f64::from(self.window.1));
        let margin = super::PANEL_MARGIN * self.scale;
        self.panel(scene, has, &markup, |(panel_width, _)| {
            ((width - panel_width) / 2.0, (height / 4.0).max(margin))
        });
    }

    /// A key that may be for a thing `]i` went to, or for the prompt in
    /// which one is typed anew. Returns whether it was.
    pub(super) fn margin_key(&mut self, press: KeyPress) -> bool {
        if !matches!(press.key, Key::Char(']' | '[' | 'i')) {
            self.margin.from = None;
        }
        if self.margin.prompt.is_some() {
            self.prompt_key(press);
            return true;
        }
        if self.margin.chosen.is_none() {
            return false;
        }
        let normal = self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.mode() == editor::Mode::Normal);
        // Keys are for the thing while the cursor is still on it.
        let cursor = self.editor.as_ref().map(editor::Editor::cursor);
        let current = self.margin.chosen.as_ref().is_some_and(|chosen| {
            chosen.revision == self.revision && cursor == Some(chosen.range.start)
        });
        if !normal || !current || self.picker.is_some() {
            self.margin.chosen = None;
            self.margin.deleting = false;
            return false;
        }
        let deleting = std::mem::take(&mut self.margin.deleting);
        match (press.key, press.ctrl) {
            (Key::Char('x'), false) | (Key::Delete, _) => self.take_out(),
            (Key::Char('d'), false) if deleting => self.take_out(),
            (Key::Char('d'), false) => self.margin.deleting = true,
            (Key::Enter, _) | (Key::Char('c'), false) => self.retype(),
            (Key::Escape, _) => self.margin.chosen = None,
            _ => {
                let chosen = self.margin.chosen.take();
                if matches!(press.key, Key::Char(']' | '[')) {
                    self.margin.from = chosen.map(|chosen| (chosen.revision, chosen.range.start));
                }
                if deleting {
                    // Not `dd` after all: the `d` is the text's.
                    self.editor_key(KeyPress::plain(Key::Char('d')));
                }
                return false;
            }
        }
        true
    }

    /// A key for the prompt in which a thing is typed anew.
    fn prompt_key(&mut self, press: KeyPress) {
        let Some(prompt) = &mut self.margin.prompt else {
            return;
        };
        match (press.key, press.ctrl) {
            (Key::Escape, _) | (Key::Char('c' | '['), true) => {
                self.margin.prompt = None;
                self.notice("left as it was");
            }
            (Key::Enter, _) => self.take_typed(),
            (Key::Backspace, _) => {
                prompt.typed.pop();
            }
            (Key::Char('u' | 'U'), true) => prompt.typed.clear(),
            (Key::Char(c), false) => prompt.typed.push(c),
            _ => {}
        }
    }

    /// The thing of the current text that `chosen` is, if it still is.
    fn thing_of(&self, chosen: &Chosen) -> Option<Thing> {
        let editor = self.editor.as_ref()?;
        if chosen.revision != self.revision {
            return None;
        }
        let found = invisible::find(editor.source());
        found
            .at(chosen.range.start)
            .filter(|thing| thing.range == chosen.range)
            .cloned()
    }

    /// `x` or `dd` on a thing: takes it out of the text.
    fn take_out(&mut self) {
        let Some(chosen) = self.margin.chosen.take() else {
            return;
        };
        let (Some(thing), Some(editor)) = (self.thing_of(&chosen), &mut self.editor) else {
            return;
        };
        let (range, with) = invisible::removal(editor.text(), &thing);
        match editor.replace(&[(range.clone(), with)]) {
            Ok(()) => {
                editor.set_cursor(range.start.min(editor.text().len()));
                self.text_changed(Edited::Command);
                let what = match thing.kind {
                    Kind::Label => format!("label {}", thing.name),
                    _ => thing.name.clone(),
                };
                self.notice(format!("{what} taken out  (u brings it back)"));
            }
            Err(why) => self.alert(format!("not changed: {why}")),
        }
    }

    /// Enter or `c` on a thing: puts up the prompt to type it anew in.
    fn retype(&mut self) {
        let Some(chosen) = self.margin.chosen.take() else {
            return;
        };
        let Some(thing) = self.thing_of(&chosen) else {
            return;
        };
        let typed = self
            .editor
            .as_ref()
            .and_then(|editor| invisible::retypable(editor.text(), &thing));
        if let Some(typed) = typed {
            self.margin.prompt = Some(Chosen { typed, ..chosen });
        } else {
            self.margin.chosen = Some(chosen);
            self.notice(format!(
                "a {} is what it is: x takes it out",
                thing.kind.called()
            ));
        }
    }

    /// Enter in the prompt: writes the thing as typed.
    fn take_typed(&mut self) {
        let Some(prompt) = self.margin.prompt.take() else {
            return;
        };
        let Some(thing) = self.thing_of(&prompt) else {
            self.alert("the text has changed: not written");
            return;
        };
        let Some(editor) = &mut self.editor else {
            return;
        };
        let (range, with) = match invisible::retyped(editor.text(), &thing, &prompt.typed) {
            Ok(edit) => edit,
            Err(why) => {
                self.alert(why);
                self.margin.prompt = Some(prompt);
                return;
            }
        };
        match editor.replace(&[(range, with.as_str())]) {
            Ok(()) => {
                editor.set_cursor(thing.range.start);
                self.text_changed(Edited::Command);
                self.notice(format!(
                    "{} is now {}  (u takes it back)",
                    thing.kind.called(),
                    prompt.typed.trim()
                ));
            }
            Err(why) => self.alert(format!("not changed: {why}")),
        }
    }

    /// Puts the cursor on `thing` and keys on it, and says what it is.
    fn choose_thing(&mut self, thing: &Thing) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        editor.set_cursor(thing.range.start);
        self.margin.chosen = Some(Chosen {
            revision: self.revision,
            range: thing.range.clone(),
            kind: thing.kind,
            typed: String::new(),
        });
        self.notice(told(thing));
        self.follow_cursor();
    }

    /// `]i` and `[i`: to the next thing the page shows as nothing, or
    /// the one before.
    fn goto_invisible(&mut self, forward: bool) {
        let Some(editor) = &self.editor else {
            return;
        };
        let cursor = match self.margin.from.take() {
            Some((revision, from)) if revision == self.revision => from,
            _ => editor.cursor(),
        };
        let found = invisible::find(editor.source());
        let things = &found.things;
        let target = if forward {
            let next = things.iter().find(|thing| thing.range.start > cursor);
            next.or(things.first())
        } else {
            let before = things.iter().rev().find(|thing| thing.range.start < cursor);
            before.or(things.last())
        };
        match target.cloned() {
            Some(thing) => self.choose_thing(&thing),
            None => self.notice("nothing in this text shows as nothing on the page"),
        }
    }

    /// `:invisibles`: all the things the page shows as nothing, to go to.
    fn list_invisibles(&mut self) {
        let Some(editor) = &self.editor else {
            return;
        };
        let cursor = editor.cursor();
        let found = invisible::find(editor.source());
        let chosen = found
            .things
            .iter()
            .position(|thing| thing.range.start >= cursor)
            .unwrap_or(0);
        let items = found
            .things
            .iter()
            .map(|thing| {
                let go = Target::Command(format!("invisible at {}", thing.range.start));
                (format!("{}\t{}", thing.name, thing.kind.called()), go)
            })
            .collect();
        self.open_picker("things that show as nothing", items, chosen);
    }

    /// The commands of the marks beside the page: `:tags`, `:errors`,
    /// `:invisibles`, and `invisible next`, `invisible previous` and
    /// `invisible at <byte>`, which keys and lists run.
    pub(super) fn margin_command(&mut self, name: &str, wanted: &str) {
        let switch = |now: bool| match wanted {
            "on" => Some(true),
            "off" => Some(false),
            "" => Some(!now),
            _ => None,
        };
        match (name, wanted) {
            ("tags", _) => match switch(!self.margin.hide_tags) {
                Some(on) => {
                    self.margin.hide_tags = !on;
                    self.notice(if on {
                        "what shows as nothing has a tag beside the page (]i goes to it)"
                    } else {
                        "no tags beside the page"
                    });
                }
                None => self.alert("on or off, please: :tags off"),
            },
            ("errors", _) => match switch(!self.margin.hide_errors) {
                Some(on) => {
                    self.margin.hide_errors = !on;
                    self.notice(if on {
                        "what the compiler complains about is marked beside the page"
                    } else {
                        "complaints only in the status line (]d goes to them)"
                    });
                }
                None => self.alert("on or off, please: :errors off"),
            },
            ("invisible", "next") => self.goto_invisible(true),
            ("invisible", "previous") => self.goto_invisible(false),
            ("invisibles", "") => self.list_invisibles(),
            ("invisible", at) if at.starts_with("at ") => {
                let start = at.trim_start_matches("at ").trim().parse::<usize>();
                let thing = start.ok().and_then(|start| {
                    let editor = self.editor.as_ref()?;
                    invisible::find(editor.source()).at(start).cloned()
                });
                match thing {
                    Some(thing) => self.choose_thing(&thing),
                    None => self.alert("that is not there any more"),
                }
            }
            _ => self.alert(format!("not a command: {name} {wanted}")),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use super::super::tests::{compile, directory, editing, open, press, tints};
    use super::*;

    /// The line of page 0 that item `name` (a tag's text, or the start
    /// of a complaint) is beside, and that note's items.
    fn note_of(viewer: &mut Viewer, name: &str) -> Option<Note> {
        let _ = viewer.scene(&|_| false);
        let item = viewer
            .margin
            .items
            .iter()
            .position(|item| match &item.what {
                What::Thing { name: of, .. } => of == name,
                What::Complaint { message, .. } => message.starts_with(name),
            })?;
        let notes = viewer.margin.notes.get(&0)?;
        notes
            .iter()
            .find(|note| note.items.contains(&item))
            .cloned()
    }

    /// The line of page 0 that shows byte `at`.
    fn line_of(viewer: &Viewer, at: usize) -> Option<usize> {
        viewer.margin.pages.get(&0)?.line_in(&(at..at + 1), true)
    }

    #[test]
    fn a_label_has_a_tag_beside_its_heading_and_keys_reach_it() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    Before it.\n\n\
                    = Intro <sec:intro>\n\
                    Words of the section.\n";
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "gsgs");
        let note = note_of(&mut viewer, "sec:intro").unwrap();
        let heading = line_of(&viewer, text.find("Intro").unwrap());
        assert_eq!(Some(note.line), heading);
        assert_ne!(heading, line_of(&viewer, text.find("Words").unwrap()));
        // Drawn right of the text, unless tags are not wanted.
        let tag = |viewer: &mut Viewer| {
            let markup = note_markup(&[("sec:intro".to_owned(), TAG)]);
            let id = Chrome::id(&markup, viewer.view.zoom);
            let scene = viewer.scene(&|_| false);
            scene.quads.iter().find(|quad| quad.texture == id).copied()
        };
        let drawn = tag(&mut viewer).unwrap();
        let right = viewer.margin.pages.get(&0).unwrap().right;
        let (_, left, _) = viewer.split();
        assert!(f64::from(drawn.x) > f64::from(left) + right * viewer.view.zoom);
        press(&mut viewer, ":tags<CR>");
        assert!(tag(&mut viewer).is_none());
        press(&mut viewer, ":tags<CR>");
        assert!(tag(&mut viewer).is_some());

        // `]i` goes to it; Enter names it anew.
        press(&mut viewer, "gg]i");
        let cursor = |viewer: &Viewer| viewer.editor.as_ref().unwrap().cursor();
        assert_eq!(cursor(&viewer), text.find("<sec").unwrap());
        assert!(
            viewer
                .message
                .as_ref()
                .unwrap()
                .text
                .starts_with("label sec:intro")
        );
        press(&mut viewer, "<CR>");
        assert_eq!(viewer.margin.prompt.as_ref().unwrap().typed, "sec:intro");
        press(&mut viewer, "<C-U>sec:start<CR>");
        assert!(viewer.text().unwrap().contains("= Intro <sec:start>\n"));
        compile(&mut viewer);
        assert!(note_of(&mut viewer, "sec:start").is_some());
        // And `x` takes it out.
        press(&mut viewer, "gg]ix");
        assert!(viewer.text().unwrap().contains("= Intro\nWords"));
        compile(&mut viewer);
        assert!(note_of(&mut viewer, "sec:start").is_none());
        press(&mut viewer, "u");
        assert!(viewer.text().unwrap().contains("<sec:start>"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_comment_goes_beside_what_follows_it_and_dd_takes_it_out() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    First paragraph.\n\n\
                    // to be looked at\n\
                    Second paragraph.\n\n\
                    #pagebreak()\n\
                    Third.\n";
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "gsgs");
        let note = note_of(&mut viewer, "\u{201c}to be looked at\u{201d}").unwrap();
        assert_eq!(
            Some(note.line),
            line_of(&viewer, text.find("Second").unwrap())
        );
        let note = note_of(&mut viewer, "page break").unwrap();
        assert_eq!(
            Some(note.line),
            line_of(&viewer, text.find("Second").unwrap())
        );
        // `[i` from the end goes to the break, and on to the comment.
        press(&mut viewer, "G[i[i");
        press(&mut viewer, "dd");
        assert!(
            viewer
                .text()
                .unwrap()
                .contains("First paragraph.\n\nSecond")
        );
        // A page break is no thing to type anew.
        press(&mut viewer, "]i<CR>");
        assert!(viewer.margin.prompt.is_none());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn an_error_is_marked_beside_its_line_until_the_text_compiles() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    First line.\n\n\
                    Second line.\n";
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "gsgs");
        assert_eq!(tints(&mut viewer, ERROR_BAR).len(), 0);
        viewer
            .agent_replace("Second line.", "Second line. #(1 +)", false, None, false)
            .unwrap();
        compile(&mut viewer);
        assert_ne!(viewer.state().problems.len(), 0);
        assert_eq!(tints(&mut viewer, ERROR_BAR).len(), 1);
        let note = note_of(&mut viewer, "expected").unwrap();
        assert_eq!(
            Some(note.line),
            line_of(&viewer, text.find("Second").unwrap())
        );
        press(&mut viewer, ":errors<CR>");
        assert_eq!(tints(&mut viewer, ERROR_BAR).len(), 0);
        press(&mut viewer, ":errors<CR>");
        // Put right, and compiled: the mark is gone.
        viewer
            .agent_replace(" #(1 +)", "", false, None, false)
            .unwrap();
        compile(&mut viewer);
        assert_eq!(tints(&mut viewer, ERROR_BAR).len(), 0);
        assert!(note_of(&mut viewer, "expected").is_none());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_warning_says_all_it_says_where_the_cursor_is_on_its_line() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    Plain words.\n\n\
                    #text(font: \"No Such Font\")[Odd words.]\n";
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "gsgs");
        assert_eq!(tints(&mut viewer, WARNING_BAR).len(), 1);
        press(&mut viewer, "/Plain<CR>");
        let _ = viewer.scene(&|_| false);
        assert!(viewer.message.is_none());
        press(&mut viewer, "/Odd<CR>");
        let _ = viewer.scene(&|_| false);
        let told = &viewer.message.as_ref().unwrap().text;
        assert!(told.starts_with("warning: unknown font family"), "{told}");
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn an_include_has_a_tag_where_the_other_files_text_begins() {
        let text = "#set page(width: 300pt, height: 300pt)\n\
                    Before it.\n\n\
                    #include \"parts/part.typ\"\n\n\
                    After it.\n";
        let directory = directory(text);
        std::fs::create_dir_all(directory.join("parts")).unwrap();
        std::fs::write(directory.join("parts/part.typ"), "Included words.\n").unwrap();
        let (mut viewer, _) = open(&directory);
        press(&mut viewer, "gsgs");
        let note = note_of(&mut viewer, "include part.typ").unwrap();
        assert_eq!(
            Some(note.line),
            line_of(&viewer, text.find("Before").unwrap())
        );
        // Not shown as source: the page has the other file's words, and
        // the tag only names it.
        press(&mut viewer, "gg]i");
        let told = &viewer.message.as_ref().unwrap().text;
        assert!(
            told.starts_with("include part.typ: x takes it out"),
            "{told}"
        );
        // Keys are for it only while the cursor stays on it.
        press(&mut viewer, "l");
        assert!(viewer.margin.chosen.is_none());
        press(&mut viewer, "gg]ix");
        assert!(viewer.text().unwrap().contains("Before it.\n\n\nAfter it."));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_table_of_contents_has_no_tags_of_the_headings_it_lists() {
        let text = "#set page(width: 300pt, height: 400pt)\n\
                    #outline()\n\
                    = Intro <intro>\n\
                    Words.\n\
                    = Next <next>\n\
                    More words.\n";
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "gsgs");
        let _ = viewer.scene(&|_| false);
        let notes = viewer.margin.notes.get(&0).cloned().unwrap_or_default();
        let tagged: usize = notes.iter().map(|note| note.items.len()).sum();
        assert_eq!(tagged, 2, "one tag for each label");
        let intro = note_of(&mut viewer, "intro").unwrap();
        let next = note_of(&mut viewer, "next").unwrap();
        assert!(intro.top < next.top);
        // Each beside its heading, which comes after the outline.
        let words = line_of(&viewer, text.find("Words").unwrap()).unwrap();
        assert_eq!(intro.line + 1, words);
        let _ = std::fs::remove_dir_all(directory);
    }
}
