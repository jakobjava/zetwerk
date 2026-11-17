//! Searching on the page, and what a key took.
//!
//! While a search is typed (`/`, `?`) and after it, until `:noh`, Escape
//! or the next search (Vim's 'hlsearch'), every match on the pages in
//! view is tinted faintly, the one the cursor is on (or, while typing,
//! the one the search would go to) more strongly, and the status line
//! says which of how many it is. While the search is typed the view goes
//! to that match; Escape brings it back to where it was.
//!
//! On the page a search is for what is read (see
//! [`zetwerk_vim::find`]): the matches are found in a reading of the
//! text, made once for each state of the text that is searched, and
//! mapped to the glyphs of a page once for each page that is drawn with
//! them. A frame only draws what was mapped; while typing in insert mode
//! nothing is looked for at all.
//!
//! After `y` what was yanked is tinted, and after `p` what came in, for a
//! moment ([`FLASH`]) or until the next key: a key that changes nothing
//! that can be seen is shown to have done something. One timer takes the
//! tint away (see [`Viewer::flash_at`]); nothing runs while it is shown. A deletion of something that showed as nothing
//! (markup), of a thing (a figure, an equation) or of much is told in the
//! status line, since the page does not show what is gone.
//!
//! `:hlsearch` and `:feedback` turn each of the two off and on.

use std::collections::HashMap;
use std::ops::Range;
use std::time::{Duration, Instant};

use typst::World;
use typst::layout::{Abs, Rect};
use typst::syntax::{FileId, LinkedNode, Source, SyntaxKind};
use zetwerk_typst::{Origin, PageMap};
use zetwerk_vim::find::{MOST, find_all};
use zetwerk_vim::pattern::{Pattern, Replacement};
use zetwerk_vim::{Query, Took};

use super::{Change, Paging, Panes, Scene, TextureId, Viewer, tint};
use crate::editor;
use crate::keys::{Key, KeyPress};

/// A match of a search on the page, and the one the cursor is on.
const PAGE_MATCH: [u8; 4] = [0xfa, 0xbd, 0x2f, 0x48];
const PAGE_MATCH_CURRENT: [u8; 4] = [0xfe, 0x80, 0x19, 0x90];
/// What a key yanked or put.
const PAGE_TOOK: [u8; 4] = [0x83, 0xa5, 0x98, 0x60];

/// How long what a key yanked or put stays tinted.
const FLASH: Duration = Duration::from_millis(400);

/// How far from a match the cursor may be left by a search that went to
/// it, in bytes: the cursor rests on what is shown, and markup may stand
/// between that and the match.
const NEAR: usize = 16;

/// How much a deletion has to take to be told of: words of it, as read.
const MANY_WORDS: usize = 30;

/// What searching on the page keeps between keys.
pub(super) struct Find {
    /// Whether matches are shown at all (`:hlsearch`), and whether what
    /// keys took is (`:feedback`).
    pub(super) highlight: bool,
    pub(super) feedback: bool,
    /// Whether the matches of the last search are shown: from a search
    /// on, until `:noh` or Escape.
    pub(super) shown: bool,
    /// How many searches the engine of which file had made when last
    /// asked.
    searches: Option<(FileId, u64)>,
    /// The editor's mode after the last key: the mode the next key is
    /// pressed in.
    mode: Option<editor::Mode>,
    /// Where the view was when a search began to be typed (how far down,
    /// in points, and the slide): Escape brings it back there.
    before: Option<(f64, usize)>,
    /// While a search is typed, the match it would go to.
    target: Option<usize>,
    /// Where the last search left the cursor, and the match it went to:
    /// the cursor rests on what is shown, which may be just before it.
    landed: Option<(usize, usize)>,
    found: Option<Found>,
    /// How many times matches were looked for: what was mapped for older
    /// ones is known to be old, also when none were kept in between.
    generation: u64,
    /// Where the matches are on the pages that were drawn with them.
    placed: HashMap<usize, Placed>,
    /// What the last key yanked or put, the revision of the text that is
    /// true of, and until when it is shown.
    took: Option<(u64, Range<usize>, Instant)>,
    /// The last substitution, for `&` and `:&&`.
    substituted: Option<Substitute>,
}

impl Default for Find {
    fn default() -> Self {
        Self {
            highlight: true,
            feedback: true,
            shown: false,
            searches: None,
            mode: None,
            before: None,
            target: None,
            landed: None,
            found: None,
            generation: 0,
            placed: HashMap::new(),
            took: None,
            substituted: None,
        }
    }
}

/// The matches of a search in one state of the text.
struct Found {
    query: Query,
    /// The file they are in.
    file: FileId,
    /// Whether they were looked for in what is read.
    read: bool,
    revision: u64,
    /// The text they are in, to find them again in the next by.
    source: Source,
    /// In bytes of the text, in its order.
    matches: Vec<Range<usize>>,
    /// Counts the searches made, so that what was mapped for an older one
    /// is known to be old.
    generation: u64,
}

/// The matches on one page: the cells of their glyphs, in points, each
/// with the index of its match.
struct Placed {
    /// What they were mapped for: the matches, the document drawn, and
    /// how the text differs from the one it was compiled from; `None`
    /// before they are.
    key: Option<(u64, usize, Change)>,
    cells: Vec<(usize, Rect)>,
}

/// The cells of the glyphs of `map` that show any of `matches` (bytes of
/// `file` as compiled, in the order of their beginnings), joined where
/// they stand side by side, each with the index of its match.
fn place(map: &PageMap, file: FileId, matches: &[Range<usize>]) -> Vec<(usize, Rect)> {
    let slack = Abs::pt(0.5);
    let mut cells: Vec<(usize, Rect)> = Vec::new();
    for glyph in map.glyphs() {
        let shown = match &glyph.origin {
            Origin::Text { file: of, range }
            | Origin::Space { file: of, range }
            | Origin::Math { file: of, range }
            | Origin::Object {
                file: of, range, ..
            } if *of == file => range,
            _ => continue,
        };
        // The last match that begins before the glyph ends, and those
        // before it that reach into the glyph: matches seldom overlap.
        let mut index = matches.partition_point(|found| found.start < shown.end);
        while let Some(before) = index.checked_sub(1) {
            index = before;
            let Some(found) = matches.get(index) else {
                break;
            };
            if found.end <= shown.start {
                break;
            }
            let cell = glyph.rect;
            match cells.last_mut() {
                Some((of, last))
                    if *of == index
                        && (last.min.y - cell.min.y).abs() < slack
                        && (last.max.y - cell.max.y).abs() < slack
                        && cell.min.x >= last.min.x
                        && cell.min.x <= last.max.x + slack =>
                {
                    last.max.x = last.max.x.max(cell.max.x);
                }
                _ => cells.push((index, cell)),
            }
        }
    }
    cells
}

/// Counts what a piece of Typst text has of things that show as more
/// than text: figures, tables, equations and the like.
fn things(text: &str) -> Vec<(usize, &'static str)> {
    fn gather(node: &LinkedNode, counts: &mut [(usize, &'static str)]) {
        let call = |name: &str| {
            node.kind() == SyntaxKind::FuncCall
                && node
                    .children()
                    .next()
                    .is_some_and(|callee| callee.leaf_text() == name)
        };
        let index = match node.kind() {
            _ if call("figure") => Some(0),
            _ if call("table") => Some(1),
            _ if call("image") => Some(2),
            SyntaxKind::Equation => Some(3),
            SyntaxKind::Heading => Some(4),
            _ if call("footnote") => Some(5),
            SyntaxKind::Ref => Some(6),
            SyntaxKind::Label => Some(7),
            SyntaxKind::SetRule | SyntaxKind::ShowRule => Some(8),
            SyntaxKind::LineComment | SyntaxKind::BlockComment => Some(9),
            _ if call("quote") => Some(10),
            _ => None,
        };
        if let Some(index) = index
            && let Some((count, _)) = counts.get_mut(index)
        {
            *count += 1;
            // A figure's image is part of the figure.
            if index == 0 {
                return;
            }
        }
        for child in node.children() {
            gather(&child, counts);
        }
    }
    let mut counts = [
        (0, "figure"),
        (0, "table"),
        (0, "image"),
        (0, "equation"),
        (0, "heading"),
        (0, "footnote"),
        (0, "reference"),
        (0, "label"),
        (0, "rule of how the document looks"),
        (0, "comment"),
        (0, "quote"),
    ];
    let source = Source::detached(text);
    gather(&LinkedNode::new(source.root()), &mut counts);
    counts.into_iter().filter(|(count, _)| *count > 0).collect()
}

/// `count` of `name`, in words: "a figure", "2 equations".
fn counted(count: usize, name: &str) -> String {
    match count {
        1 if name.starts_with(['a', 'e', 'i', 'o', 'u']) => format!("an {name}"),
        1 => format!("a {name}"),
        _ => match name.split_once(' ') {
            Some((first, rest)) => format!("{count} {first}s {rest}"),
            None => format!("{count} {name}s"),
        },
    }
}

/// What the status line says of `text` having been deleted, if it is
/// worth saying: what the page cannot show is gone.
fn told(text: &str, linewise: bool) -> Option<String> {
    let found = things(text);
    if !found.is_empty() {
        let mut named: Vec<String> = found
            .iter()
            .map(|(count, name)| counted(*count, name))
            .collect();
        let last = named.pop()?;
        let all = if named.is_empty() {
            last
        } else {
            format!("{} and {last}", named.join(", "))
        };
        // The text of a footnote is elsewhere on the page: it went too.
        let notes = found.iter().find(|(_, name)| *name == "footnote");
        let along = match notes.map(|(count, _)| *count) {
            None => "",
            Some(1) => ", with its text",
            Some(_) => ", with their text",
        };
        return Some(format!("{all} deleted{along}"));
    }
    let read = crate::reading::reading(&Source::detached(text));
    let read = read.text();
    let paragraphs = read.trim().matches('\n').count() + 1;
    let lines = text.trim_end_matches('\n').matches('\n').count() + 1;
    let words = read.split_whitespace().count();
    if paragraphs >= 2 {
        Some(format!("{paragraphs} paragraphs deleted ({words} words)"))
    } else if linewise && lines >= 3 {
        Some(format!("{lines} lines deleted"))
    } else if words >= MANY_WORDS {
        Some(format!("{words} words deleted"))
    } else if read.trim().is_empty() && !text.trim().is_empty() {
        Some("markup deleted: it showed as nothing".to_owned())
    } else {
        None
    }
}

/// How many references in `source` lead to the labels that `deleted`
/// had, now that it is gone from it: `None` if none do.
fn lost_references(deleted: &str, source: &Source) -> Option<usize> {
    if !deleted.contains('<') {
        return None;
    }
    let gone = crate::ref_text::read(&Source::detached(deleted)).defined;
    if gone.is_empty() {
        return None;
    }
    let left = crate::ref_text::read(source);
    let lost = left
        .used
        .iter()
        .filter(|(name, _)| gone.iter().any(|(label, _)| label == name))
        .filter(|(name, _)| !left.defined.iter().any(|(label, _)| label == name))
        .count();
    (lost > 0).then_some(lost)
}

impl Viewer {
    /// After a key for the text: notes what it searched for and what it
    /// took, finds the matches to show, and while a search is typed
    /// brings the match it would go to into view.
    pub(super) fn find_after_key(&mut self, press: KeyPress) {
        let Some(editor) = &self.editor else {
            return;
        };
        let mode = editor.mode();
        let before = self.find.mode.replace(mode);
        let engine = editor.engine();
        let took = engine.took().cloned();
        let (file, total) = (editor.file(), engine.searches());
        // Another file's engine has made searches of its own.
        let searched = self
            .find
            .searches
            .replace((file, total))
            .map_or(total > 0, |(then, count)| then == file && count != total);
        if searched {
            self.find.shown = true;
        }
        let pressed_in_normal = before == Some(editor::Mode::Normal);
        if press.key == Key::Escape && pressed_in_normal && mode == editor::Mode::Normal {
            self.find.shown = false;
        }
        let typing = matches!(editor.prompt(), Some('/' | '?'));
        self.find.took = None;
        if let Some(took) = took {
            self.took(took);
        }
        self.refind();
        self.find.target = None;
        self.find.landed = None;
        if searched {
            self.land();
        }
        if typing {
            if self.find.before.is_none() {
                self.find.before = Some((self.view.top, self.slide));
            }
            self.incremental();
        } else if let Some((top, slide)) = self.find.before.take()
            && press.key != Key::Enter
        {
            // The search was not made: the view goes back.
            if self.paging == Paging::Slides {
                self.show_slide(slide);
            } else {
                self.view.top = top;
                self.view.clamp(&self.layout);
            }
        }
    }

    /// After a search: notes the match nearest to the cursor as the one
    /// it went to, for as long as the cursor stays where it was left.
    fn land(&mut self) {
        let (Some(editor), Some(found)) = (&self.editor, self.found()) else {
            return;
        };
        let cursor = editor.cursor();
        let distance = |found: &Range<usize>| {
            found
                .start
                .saturating_sub(cursor)
                .max(cursor.saturating_sub(found.end))
        };
        let nearest = found
            .matches
            .iter()
            .enumerate()
            .min_by_key(|(_, found)| distance(found))
            .filter(|(_, found)| distance(found) <= NEAR)
            .map(|(index, _)| index);
        self.find.landed = nearest.map(|index| (cursor, index));
    }

    /// Notes what a key yanked, deleted or put: the first two to be
    /// marked until the next key, the last to be told of.
    fn took(&mut self, took: Took) {
        let Some(editor) = &self.editor else {
            return;
        };
        // What leads nowhere now is said whatever else is: the whole
        // document stops for it.
        if let Took::Deleted { text, linewise, .. } = &took
            && let Some(lost) = lost_references(text, editor.source())
        {
            let said = told(text, *linewise).unwrap_or_else(|| "a label deleted".to_owned());
            let lost = match lost {
                1 => "a reference to it leads".to_owned(),
                lost => format!("{lost} references to it lead"),
            };
            self.alert(format!("{said}: {lost} nowhere now (u brings it back)"));
            return;
        }
        if !self.find.feedback {
            return;
        }
        match took {
            Took::Yanked(range) | Took::Put(range) => {
                let until = Instant::now() + FLASH;
                self.find.took = Some((editor.revision(), range, until));
            }
            Took::Deleted { text, linewise, .. } => {
                // While typing, only what went unseen with a key that
                // takes what is shown: a footnote, with its text.
                let typing = editor.mode() == editor::Mode::Insert;
                if typing && things(&text).is_empty() {
                    return;
                }
                if let Some(said) = told(&text, linewise) {
                    self.notice(said);
                }
            }
        }
    }

    /// What is searched for now: what is being typed for a search, else
    /// the last search while its matches are shown.
    fn wanted(&self) -> Option<Query> {
        let editor = self.editor.as_ref()?;
        if matches!(editor.prompt(), Some('/' | '?')) {
            let pattern = editor.command_line()?.to_owned();
            if pattern.is_empty() {
                return None;
            }
            return Some(Query {
                pattern,
                word_start: false,
                word_end: false,
            });
        }
        if !self.find.shown || !self.find.highlight {
            return None;
        }
        editor.engine().last_search()
    }

    /// Finds the matches of what is searched for, unless they are known
    /// for the text as it is. Not while typing: what is typed would be
    /// looked for in all the text at every key.
    fn refind(&mut self) {
        let Some(editor) = &self.editor else {
            return;
        };
        let Some(query) = self
            .wanted()
            .filter(|_| editor.mode() != editor::Mode::Insert)
        else {
            self.find.found = None;
            return;
        };
        let read = self.shown_panes() == Panes::Page;
        let (file, revision) = (editor.file(), editor.revision());
        let known = self.find.found.as_ref().is_some_and(|found| {
            found.query == query
                && found.read == read
                && found.file == file
                && found.revision == revision
        });
        if known {
            return;
        }
        let reading = read.then(|| editor.reading());
        // After an edit, in the paragraphs it changed: a long text is
        // not to be looked through at every key.
        let again = self
            .find
            .found
            .as_ref()
            .filter(|found| found.query == query && found.read == read && found.file == file);
        let again = again.and_then(|found| {
            let source = editor.source();
            crate::reading::found_again(
                &found.matches,
                &found.source,
                source,
                reading.as_deref(),
                &query,
            )
        });
        let matches = again.unwrap_or_else(|| find_all(editor.text(), reading.as_deref(), &query));
        self.find.generation += 1;
        let generation = self.find.generation;
        self.find.found = Some(Found {
            query,
            file,
            read,
            revision,
            source: editor.source().clone(),
            matches,
            generation,
        });
    }

    /// The matches found, if they are of the text as it is.
    fn found(&self) -> Option<&Found> {
        let editor = self.editor.as_ref()?;
        let found = self.find.found.as_ref()?;
        (found.file == editor.file() && found.revision == editor.revision()).then_some(found)
    }

    /// When what a key yanked or put is to stop being shown, if it is
    /// shown: the one time [`Viewer::flash_over`] wants to be called.
    #[must_use]
    pub fn flash_at(&self) -> Option<Instant> {
        self.find.took.as_ref().map(|(_, _, until)| *until)
    }

    /// Stops showing what a key yanked or put, if it is time to. Returns
    /// whether the pages are to be drawn again.
    pub fn flash_over(&mut self, now: Instant) -> bool {
        let over = self.flash_at().is_some_and(|until| now >= until);
        if over {
            self.find.took = None;
        }
        over
    }

    /// While a search is typed: the match it would go to (the first behind
    /// the cursor, or before it for `?`), brought into view.
    fn incremental(&mut self) {
        let (Some(editor), Some(found)) = (&self.editor, self.found()) else {
            return;
        };
        let cursor = editor.cursor();
        let matches = &found.matches;
        let target = if editor.prompt() == Some('?') {
            let before = matches.partition_point(|found| found.start < cursor);
            before.checked_sub(1).or(matches.len().checked_sub(1))
        } else {
            let behind = matches.partition_point(|found| found.start <= cursor);
            (behind < matches.len())
                .then_some(behind)
                .or((!matches.is_empty()).then_some(0))
        };
        let at = target
            .and_then(|index| matches.get(index))
            .map(|found| found.start);
        self.find.target = target;
        if let Some(at) = at {
            self.bring_into_view(at);
        }
    }

    /// Scrolls the pages so that byte `at` of the text is in view, if it
    /// is not.
    fn bring_into_view(&mut self, at: usize) {
        if self.shown_panes() == Panes::Source {
            return;
        }
        let (Some(change), Some(document), Some(world)) =
            (self.places(), &self.document, &self.compiled_world)
        else {
            return;
        };
        let file = self.file_id();
        let Ok(source) = world.source(file) else {
            return;
        };
        let shown = world.shown_at(file, change.back_or_edge(at));
        let near = self.cursor_page.get();
        let Some(position) = zetwerk_typst::locate(document.pages(), &source, shown, near) else {
            return;
        };
        if self.paging == Paging::Slides {
            return self.show_slide(position.page);
        }
        let Some(page) = self.layout.pages.get(position.page) else {
            return;
        };
        let zoom = self.view.zoom;
        let y = f64::from(page.y) + position.y * zoom;
        let (top, height) = (self.view.top * zoom, f64::from(self.view.height));
        if y < top + height / 8.0 || y > top + height * 7.0 / 8.0 {
            self.view.top = (y - height / 3.0) / zoom;
            self.view.clamp(&self.layout);
        }
    }

    /// Which of the matches is the one to show more strongly: while a
    /// search is typed, the one it would go to; else the one the cursor
    /// is on.
    fn current(&self) -> Option<usize> {
        if self.find.target.is_some() {
            return self.find.target;
        }
        let (editor, found) = (self.editor.as_ref()?, self.found()?);
        let cursor = editor.cursor();
        if let Some((at, index)) = self.find.landed
            && at == cursor
        {
            return Some(index);
        }
        let index = found
            .matches
            .partition_point(|found| found.start <= cursor)
            .checked_sub(1)?;
        let on = found.matches.get(index)?;
        (cursor < on.end || cursor == on.start).then_some(index)
    }

    /// What the status line says of the matches: which of how many the
    /// cursor is on, how many there are, or that there are none. Empty
    /// while none are shown.
    pub(super) fn search_count(&self) -> String {
        let Some(found) = self.found() else {
            return String::new();
        };
        let total = found.matches.len();
        let more = if total >= MOST { "+" } else { "" };
        match (total, self.current()) {
            (0, _) => "no match  ".to_owned(),
            (_, Some(index)) => format!("{} of {total}{more}  ", index + 1),
            (1, None) => "1 match  ".to_owned(),
            (_, None) => format!("{total}{more} matches  "),
        }
    }

    /// `:noh`, `:hlsearch` and `:feedback`; whether `command` is one of
    /// them.
    pub(super) fn find_command(&mut self, command: &str) -> bool {
        if let Some(substitute) = Substitute::read(command) {
            self.substitute(substitute);
            return true;
        }
        let (name, wanted) = command.split_once(' ').unwrap_or((command, ""));
        let on = |now: bool| match wanted {
            "on" => Some(true),
            "off" => Some(false),
            "" => Some(!now),
            _ => None,
        };
        match name {
            "noh" | "nohlsearch" if wanted.is_empty() => {
                self.find.shown = false;
                self.find.found = None;
            }
            "hlsearch" | "hls" => {
                let Some(highlight) = on(self.find.highlight) else {
                    return false;
                };
                self.find.highlight = highlight;
                self.find.shown = highlight;
                self.refind();
                self.notice(if highlight {
                    "the matches of a search are shown until :noh or Escape"
                } else {
                    "the matches of a search are not shown"
                });
            }
            "feedback" => {
                let Some(feedback) = on(self.find.feedback) else {
                    return false;
                };
                self.find.feedback = feedback;
                self.notice(if feedback {
                    "what is yanked and put is shown, what goes unseen is told"
                } else {
                    "yanking, putting and deleting say nothing"
                });
            }
            _ => return false,
        }
        true
    }

    /// `:s/this/that/` and its like (see [`Substitute`]).
    fn substitute(&mut self, substitute: Result<Substitute, String>) {
        let mut substitute = match substitute {
            Ok(substitute) => substitute,
            Err(why) => return self.alert(why),
        };
        // Nothing to look for: what the last search looked for, as it is
        // written.
        if substitute.pattern.is_empty() {
            let searched = (self.editor.as_ref()).and_then(|editor| editor.engine().last_search());
            let Some(searched) = searched else {
                return self.alert("nothing to look for: :s/this/that/");
            };
            substitute.pattern = searched.pattern;
            substitute.literal = true;
        }
        let Some(editor) = &self.editor else {
            return;
        };
        let edits = match substitute.edits(editor.text(), editor.cursor()) {
            Ok(edits) => edits,
            Err(why) => return self.alert(why),
        };
        let (count_only, confirm) = (substitute.count_only, substitute.confirm);
        let pattern = substitute.pattern.clone();
        self.find.substituted = Some(substitute);
        if edits.is_empty() {
            return self.alert(format!("not found: {pattern}"));
        }
        if count_only {
            let text = editor.text();
            let mut lines: Vec<usize> = edits
                .iter()
                .map(|(range, _)| text[..range.start].matches('\n').count())
                .collect();
            lines.dedup();
            let count = edits.len();
            let matches = if count == 1 { "match" } else { "matches" };
            let on = if lines.len() == 1 { "line" } else { "lines" };
            return self.notice(format!("{count} {matches} on {} {on}", lines.len()));
        }
        if confirm {
            return self.confirm_substitute(edits);
        }
        self.make_substitute(&edits);
    }

    /// Makes the replacements `edits` (in the order of the text, apart),
    /// as one undo step, and says how many.
    pub(super) fn make_substitute(&mut self, edits: &[(Range<usize>, String)]) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let Some((last, _)) = edits.last() else {
            return;
        };
        // From the last: the ranges before it stay those of the text.
        let backwards: Vec<(Range<usize>, &str)> = edits
            .iter()
            .rev()
            .map(|(range, with)| (range.clone(), with.as_str()))
            .collect();
        match editor.replace(&backwards) {
            Ok(()) => {
                // To the start of the last line that changed, as in Vim:
                // where it is now that those before it changed.
                let before = &edits[..edits.len() - 1];
                let put: usize = before.iter().map(|(_, with)| with.len()).sum();
                let taken: usize = before.iter().map(|(range, _)| range.len()).sum();
                let last = (last.start + put).saturating_sub(taken);
                let text = editor.text();
                let line = text[..last.min(text.len())]
                    .rfind('\n')
                    .map_or(0, |found| found + 1);
                let blanks =
                    text[line..].len() - text[line..].trim_start_matches([' ', '\t']).len();
                editor.set_cursor(line + blanks);
                self.text_changed(super::Edited::Command);
                let count = edits.len();
                let what = if count == 1 { "change" } else { "changes" };
                self.notice(format!("{count} {what}  (u takes them back)"));
            }
            Err(why) => self.alert(format!("not changed: {why}")),
        }
    }

    /// `&`, `:&&`, `g&` and their like: the last substitution again, in
    /// the line of the cursor or in `all`; with its flags if `keep`.
    pub(super) fn substitute_again(&mut self, all: bool, keep: bool) {
        let Some(mut again) = self.find.substituted.clone() else {
            return self.alert("no substitution to make again: :s/this/that/ first");
        };
        again.lines = if all { Lines::All } else { Lines::Cursor };
        if !keep {
            again.all = false;
            again.any_case = false;
            again.count_only = false;
            again.confirm = false;
        }
        self.substitute(Ok(again));
    }

    /// Tints on page `index`, whose top-left corner is drawn at `origin`,
    /// the matches of the search and what the last key yanked or put.
    pub(super) fn find_marks(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        index: usize,
        origin: (f64, f64),
    ) {
        let Some(change) = self.places() else {
            return;
        };
        let current = self.current();
        let took = self.find.took.clone().filter(|(revision, ..)| {
            self.editor
                .as_ref()
                .is_some_and(|editor| editor.revision() == *revision)
        });
        let file = self.file_id();
        let (Some(document), Some(world)) = (&self.document, &self.compiled_world) else {
            return;
        };
        // Field by field, not by `found`: the maps are borrowed below.
        let found = self.find.found.as_ref().filter(|found| {
            self.editor.as_ref().is_some_and(|editor| {
                editor.file() == found.file && editor.revision() == found.revision
            })
        });
        if found.is_none_or(|found| found.matches.is_empty()) && took.is_none() {
            return;
        }
        let Some(page) = document.pages().get(index) else {
            return;
        };
        let map = self
            .maps
            .entry(index)
            .or_insert_with(|| PageMap::build(world, &page.frame));
        let zoom = self.view.zoom;
        let place_on = |rect: Rect| {
            (
                origin.0 + rect.min.x.to_pt() * zoom,
                origin.1 + rect.min.y.to_pt() * zoom,
                (rect.max.x - rect.min.x).to_pt() * zoom,
                (rect.max.y - rect.min.y).to_pt() * zoom,
            )
        };
        if let Some(found) = found {
            let key = (found.generation, document.pages().as_ptr() as usize, change);
            let placed = self.find.placed.entry(index).or_insert_with(|| Placed {
                key: None,
                cells: Vec::new(),
            });
            if placed.key != Some(key) {
                let then: Vec<Range<usize>> = found
                    .matches
                    .iter()
                    .map(|found| change.back_or_edge(found.start)..change.back_or_edge(found.end))
                    .collect();
                placed.key = Some(key);
                placed.cells = place(map, file, &then);
            }
            for (of, cell) in &placed.cells {
                let colour = if Some(*of) == current {
                    PAGE_MATCH_CURRENT
                } else {
                    PAGE_MATCH
                };
                tint(scene, has, colour, place_on(*cell));
            }
        }
        if let Some((_, range, _)) = took {
            let then = change.back_or_edge(range.start)..change.back_or_edge(range.end);
            for rect in map.selection(file, &then) {
                tint(scene, has, PAGE_TOOK, place_on(rect));
            }
        }
    }
}

/// `:s/this/that/`, `:%s/this/that/g` and `:3,7s/this/that/`: what
/// `pattern` finds becomes `with`, in the line of the text the cursor is
/// in, in all of them (`%`) or in those counted; the first in each line,
/// or with `g` all; with `i` in any case (`I`: not); with `n` they are
/// only counted, with `c` each is asked about. The pattern is Vim's
/// regular expression, and `with` may hold `\1` and `&` (see
/// [`zetwerk_vim::pattern`]); a backslash before the sign that parts the
/// three lets it stand for itself. A pattern left empty is what the
/// last search looked for, as it is written.
#[derive(Clone)]
#[allow(clippy::struct_excessive_bools, reason = "the flags of :s")]
struct Substitute {
    lines: Lines,
    pattern: String,
    /// Whether the pattern is text as it is written, not a pattern.
    literal: bool,
    with: String,
    all: bool,
    any_case: bool,
    count_only: bool,
    confirm: bool,
}

/// The lines a substitution is made in.
#[derive(Clone, Copy)]
enum Lines {
    Cursor,
    All,
    /// From and to, counted from 1.
    Counted(usize, usize),
}

impl Substitute {
    /// `command` read as a substitution: `None` if it is none, `Err`
    /// with why if it is one that cannot be made.
    fn read(command: &str) -> Option<Result<Self, String>> {
        let (lines, rest) = if let Some(rest) = command.strip_prefix('%') {
            (Lines::All, rest)
        } else {
            let counted = command.len()
                - command
                    .trim_start_matches(|c: char| c.is_ascii_digit() || c == ',')
                    .len();
            let (numbers, rest) = command.split_at(counted);
            let lines = if numbers.is_empty() {
                Lines::Cursor
            } else {
                let (from, to) = numbers.split_once(',').unwrap_or((numbers, numbers));
                let (Ok(from), Ok(to)) = (from.parse::<usize>(), to.parse::<usize>()) else {
                    return None;
                };
                Lines::Counted(from.min(to).max(1), from.max(to))
            };
            (lines, rest)
        };
        let rest = rest
            .strip_prefix("substitute")
            .or_else(|| rest.strip_prefix('s'))?;
        let mut chars = rest.chars();
        let parting = chars.next()?;
        if parting.is_alphanumeric() || parting.is_whitespace() || parting == '\\' {
            return None;
        }
        let mut parts = vec![String::new()];
        let mut escaped = false;
        for c in chars {
            match c {
                _ if escaped => {
                    if c != parting {
                        parts.last_mut()?.push('\\');
                    }
                    parts.last_mut()?.push(c);
                    escaped = false;
                }
                '\\' => escaped = true,
                _ if c == parting && parts.len() < 3 => parts.push(String::new()),
                _ => parts.last_mut()?.push(c),
            }
        }
        let mut parts = parts.into_iter();
        let pattern = parts.next().unwrap_or_default();
        let with = parts.next().unwrap_or_default();
        let flags = parts.next().unwrap_or_default();
        let flags = flags.trim_end();
        if let Some(flag) = flags
            .chars()
            .find(|flag| !matches!(flag, 'g' | 'i' | 'I' | 'n' | 'c'))
        {
            return Some(Err(format!("not a flag of :s here: {flag}")));
        }
        Some(Ok(Self {
            lines,
            pattern,
            literal: false,
            with,
            all: flags.contains('g'),
            any_case: flags.contains('i'),
            count_only: flags.contains('n'),
            confirm: flags.contains('c'),
        }))
    }

    /// Where in `text`, with the cursor at byte `cursor`, the pattern is
    /// found to be replaced, in order, and what with; or why it cannot be
    /// looked for.
    fn edits(&self, text: &str, cursor: usize) -> Result<Vec<(Range<usize>, String)>, String> {
        let pattern = if self.literal {
            Pattern::literal(&self.pattern, self.any_case)?
        } else {
            Pattern::with_case(&self.pattern, self.any_case)?
        };
        let with = Replacement::new(&self.with)?;
        let mut found = Vec::new();
        let mut start = 0;
        for (index, line) in text.split_inclusive('\n').enumerate() {
            let range = start..start + line.len();
            start = range.end;
            let wanted = match self.lines {
                Lines::Cursor => {
                    range.contains(&cursor) || (cursor == text.len() && range.end == cursor)
                }
                Lines::All => true,
                Lines::Counted(from, to) => (from..=to).contains(&(index + 1)),
            };
            if !wanted {
                continue;
            }
            // Within the line, without its line break: `$` is its end.
            let body = line.strip_suffix('\n').unwrap_or(line);
            for captures in pattern.captures(body) {
                let Some(whole) = captures.get(0) else {
                    continue;
                };
                let at = range.start + whole.start()..range.start + whole.end();
                found.push((at, with.expand(&captures)));
                if !self.all {
                    break;
                }
            }
        }
        Ok(found)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use super::super::tests::{compile, editing, press, tints};
    use super::*;

    /// A document on the page alone, compiled.
    fn on_page(text: &str) -> (Viewer, std::path::PathBuf) {
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "gsgs");
        assert_eq!(viewer.shown_panes(), Panes::Page);
        compile(&mut viewer);
        (viewer, directory)
    }

    #[test]
    fn matches_are_shown_while_typing_and_after_until_escape() {
        let text = "One cat, two cats.\n\nA dog and a #strong[ca]t.\n";
        let (mut viewer, _directory) = on_page(text);
        assert_eq!(tints(&mut viewer, PAGE_MATCH).len(), 0);
        press(&mut viewer, "/ca");
        // While typing: all three, the first behind the cursor strongest.
        assert_eq!(tints(&mut viewer, PAGE_MATCH).len(), 2);
        assert_eq!(tints(&mut viewer, PAGE_MATCH_CURRENT).len(), 1);
        assert_eq!(viewer.search_count(), "1 of 3  ");
        press(&mut viewer, "t<CR>");
        let cursor = viewer.editor.as_ref().unwrap().cursor();
        assert_eq!(cursor, text.find("cat").unwrap());
        assert_eq!(viewer.search_count(), "1 of 3  ");
        press(&mut viewer, "n");
        assert_eq!(viewer.search_count(), "2 of 3  ");
        // The third is set off by markup in the middle: found all the same.
        press(&mut viewer, "n");
        assert_eq!(viewer.search_count(), "3 of 3  ");
        let cursor = viewer.editor.as_ref().unwrap().cursor();
        assert_eq!(cursor, text.find("ca]t").unwrap());
        assert_eq!(tints(&mut viewer, PAGE_MATCH).len(), 2);
        // Away from the matches they are still shown.
        press(&mut viewer, "gg");
        assert_eq!(viewer.search_count(), "3 matches  ");
        assert!(tints(&mut viewer, PAGE_MATCH).len() >= 3);
        // Escape hides them; `n` shows them again; `:noh` hides them.
        press(&mut viewer, "<Esc>");
        assert_eq!(tints(&mut viewer, PAGE_MATCH).len(), 0);
        assert_eq!(viewer.search_count(), "");
        press(&mut viewer, "n");
        assert!(viewer.search_count().ends_with(" of 3  "));
        press(&mut viewer, ":noh<CR>");
        assert_eq!(tints(&mut viewer, PAGE_MATCH).len(), 0);
        assert_eq!(tints(&mut viewer, PAGE_MATCH_CURRENT).len(), 0);
        // Turned off, a search shows none.
        press(&mut viewer, ":hlsearch<CR>/cat<CR>");
        assert_eq!(tints(&mut viewer, PAGE_MATCH).len(), 0);
        assert_eq!(viewer.search_count(), "");
    }

    #[test]
    fn a_search_finds_what_is_read_on_the_page() {
        let text = "The *quick*\nbrown fox: $alpha$.\n";
        let (mut viewer, _directory) = on_page(text);
        let cursor = |viewer: &Viewer| viewer.editor.as_ref().unwrap().cursor();
        // Across a line break of the text and markup, whatever the case.
        press(&mut viewer, "/QUICK brown<CR>");
        assert_eq!(viewer.search_count(), "no match  ");
        press(&mut viewer, "/quick brown<CR>");
        assert_eq!(cursor(&viewer), text.find("quick").unwrap());
        assert_eq!(viewer.search_count(), "1 of 1  ");
        // A symbol by what it is, not by its name.
        press(&mut viewer, "gg/α<CR>");
        assert_eq!(cursor(&viewer), text.find("alpha").unwrap());
        press(&mut viewer, "gg/alpha<CR>");
        assert_eq!(viewer.search_count(), "no match  ");
        // As written, with `\=`.
        press(&mut viewer, "gg/\\=*quick*<CR>");
        assert_eq!(viewer.search_count(), "1 of 1  ");
        press(&mut viewer, "gg/\\=alpha<CR>");
        assert_eq!(cursor(&viewer), text.find("alpha").unwrap());
        // `*` takes the word as shown.
        press(&mut viewer, "gg*");
        assert_eq!(viewer.search_count(), "1 of 1  ");
    }

    #[test]
    fn escape_brings_the_view_back_from_where_the_search_went() {
        let mut text = String::from("Start.\n\n");
        for _ in 0..12 {
            text.push_str("#lorem(120)\n\n");
        }
        text.push_str("Faraway.\n");
        let (mut viewer, _directory) = on_page(&text);
        let top = viewer.view.top;
        press(&mut viewer, "/Faraway");
        assert!(viewer.view.top > top);
        assert_eq!(viewer.editor.as_ref().unwrap().cursor(), 0);
        press(&mut viewer, "<Esc>");
        assert!((viewer.view.top - top).abs() < 1e-9);
        press(&mut viewer, "/Faraway<CR>");
        assert!(viewer.view.top > top);
        assert_eq!(
            viewer.editor.as_ref().unwrap().cursor(),
            text.find("Faraway").unwrap()
        );
    }

    #[test]
    fn yanking_and_putting_show_what_they_took_for_a_moment() {
        let (mut viewer, _directory) = on_page("Some words here.\n");
        assert_eq!(viewer.flash_at(), None);
        press(&mut viewer, "yw");
        assert_eq!(tints(&mut viewer, PAGE_TOOK).len(), 1);
        // One timer takes it away, and then there is none.
        let until = viewer.flash_at().unwrap();
        assert!(!viewer.flash_over(until.checked_sub(FLASH / 2).unwrap()));
        assert_eq!(tints(&mut viewer, PAGE_TOOK).len(), 1);
        assert!(viewer.flash_over(until));
        assert_eq!(tints(&mut viewer, PAGE_TOOK).len(), 0);
        assert_eq!(viewer.flash_at(), None);
        assert!(!viewer.flash_over(until + FLASH));
        // The next key takes it away sooner.
        press(&mut viewer, "yw");
        assert_eq!(tints(&mut viewer, PAGE_TOOK).len(), 1);
        press(&mut viewer, "w");
        assert_eq!(tints(&mut viewer, PAGE_TOOK).len(), 0);
        press(&mut viewer, "P");
        compile(&mut viewer);
        assert_eq!(viewer.text(), Some("Some Some words here.\n"));
        assert_eq!(tints(&mut viewer, PAGE_TOOK).len(), 1);
        press(&mut viewer, "l");
        assert_eq!(tints(&mut viewer, PAGE_TOOK).len(), 0);
        press(&mut viewer, ":feedback<CR>yw");
        assert_eq!(tints(&mut viewer, PAGE_TOOK).len(), 0);
    }

    #[test]
    fn what_goes_unseen_is_told() {
        assert_eq!(
            told("#figure(image(\"a.png\"), caption: [A])\n", true).as_deref(),
            Some("a figure deleted")
        );
        assert_eq!(
            told("$x$ and $y$ <eq>", false).as_deref(),
            Some("2 equations and a label deleted")
        );
        assert_eq!(
            told("one\ntwo\nthree\n", true).as_deref(),
            Some("3 lines deleted")
        );
        assert_eq!(
            told("One.\n\nTwo.\n\nThree.", false).as_deref(),
            Some("3 paragraphs deleted (3 words)")
        );
        assert_eq!(
            told("*", false).as_deref(),
            Some("markup deleted: it showed as nothing")
        );
        assert_eq!(told("word", false), None);
        assert_eq!(
            told("#set text(size: 12pt)", false).as_deref(),
            Some("a rule of how the document looks deleted")
        );
    }

    #[test]
    fn deleting_a_figure_on_the_page_says_so() {
        let text = "Before.\n\n#figure(rect(), caption: [A box]) <box>\n\nAfter.\n";
        let (mut viewer, _directory) = on_page(text);
        press(&mut viewer, "/box<CR>");
        let at = viewer.editor.as_ref().unwrap().cursor();
        assert!(at > text.find("figure").unwrap(), "on the caption");
        press(&mut viewer, "dap");
        let message = viewer.message.as_ref().map(|message| message.text.clone());
        assert!(
            message
                .as_deref()
                .is_some_and(|said| said.contains("figure")),
            "{message:?}, {:?}",
            viewer.text()
        );
    }
}
