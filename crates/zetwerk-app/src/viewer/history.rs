//! What undo and redo did, shown where it happened; and the history of
//! the text as a list to look through and to go back into (`gh`).
//!
//! After `u` and Ctrl-R the pages show what the step changed, until the
//! next key: text that came back is tinted like an agent's change, and
//! where text went a bar stands at the place it went from. A step that
//! changed several places marks them all; the view goes to the first of
//! them if that is not in view, and the status line says how many there
//! are. While something is marked the view is about it, not about the
//! cursor, which stays where Vim puts it.
//!
//! The list has a line for every state the text has been in, the newest
//! first: what the change that led to the state did, in words of the
//! text and never in its markup. The history is a tree (text that was
//! undone and then changed differently is a branch of it), and the list
//! leaves nothing of it out: a state says whether it is the one the text
//! is in, one that Ctrl-R leads to, or one on another branch.
//!
//! Moving through the list shows the chosen state on the pages, with the
//! change that led to it marked. The text is not touched for that: the
//! state's text is worked out on a copy (`Document::text_at`) and typeset
//! in place of the text as it is. Looking writes nothing, not to the
//! text, not to its history, not to its journal; so Escape has nothing to
//! put back but the pages. Enter goes to the chosen state the way the
//! user would, by undoing and redoing (`Editor::travel`), so that `u` and
//! Ctrl-R lead back as after any undo.
//!
//! The journal keeps no times: the list cannot say when a step was made.

use std::ops::Range;

use typst::World;
use typst::syntax::{Source, SyntaxKind, SyntaxNode};
use zetwerk_core::{Change, Direction, History, ROOT, StateId, Step};
use zetwerk_typst::{PageMap, PagePosition, ProjectWorld};

use super::agent::Mark;
use super::assist::Target;
use super::{Edited, PAGE_AGENT_MARK, Paging, Panes, Scene, TextureId, Viewer, tint};
use crate::editor::{self, Editor};
use crate::keys::{Key, KeyPress};
use crate::typst_text::{self, EMPTY};

/// The bar at a place text went from: its colour, of the hue of what an
/// agent changed, and its width in points.
pub(super) const PAGE_STEP_PLACE: [u8; 4] = [0x98, 0x97, 0x1a, 0xd0];
const PLACE_WIDTH: f64 = 3.0;

/// How many characters of the text a step changed its line of the list
/// shows; of each side, where it replaced one text by another.
const WORDS: usize = 44;
const WORDS_EACH: usize = 22;
/// How many bytes of the changed text are read for those words.
const READ: usize = 240;

/// Where in the view something is brought that is not in view: it is
/// left alone between the first two (parts of the view's height from its
/// top), and else put at the third.
type Band = (f64, f64, f64);
/// As the cursor is kept in view.
const IN_VIEW: Band = (0.125, 0.875, 1.0 / 3.0);
/// While the list is up: above it, since it covers the middle.
const ABOVE_LIST: Band = (0.02, 0.18, 0.08);

/// The list of the history, while it is up.
pub(super) struct Panel {
    /// The state the pages show, or are being made to show, in place of
    /// the text as it is; `None` while they show that.
    shown: Option<Shown>,
    /// Where the view was when the list was put up, to put it back: how
    /// far down, in points, and the slide.
    top: f64,
    slide: usize,
}

/// A state of the history as the pages show it.
struct Shown {
    state: StateId,
    /// Its text, as the source of the file being edited.
    source: Source,
    /// What the change that led to it made different, in that text.
    ranges: Vec<Range<usize>>,
    /// The revision its pages were last asked for as, once they were.
    asked: Option<u64>,
}

/// The state an entry of the history's list leads to.
fn state_of(target: &Target) -> Option<StateId> {
    let Target::Command(command) = target else {
        return None;
    };
    command.strip_prefix("history go ")?.parse().ok()
}

/// What of `node` is read, not what is written to make it look as it
/// does: the words of running text and the letters of equations.
fn gather(node: &SyntaxNode, read: &mut String) {
    match node.kind() {
        SyntaxKind::Text
        | SyntaxKind::MathText
        | SyntaxKind::MathIdent
        | SyntaxKind::SmartQuote => {
            read.push_str(node.leaf_text());
        }
        SyntaxKind::Space | SyntaxKind::Parbreak | SyntaxKind::Linebreak => read.push(' '),
        _ => {
            for child in node.children() {
                gather(child, read);
            }
        }
    }
}

/// The beginning of `texts`, one after the other with a blank between
/// them: as much as is read for a line of the list. `None` if they are
/// all empty.
fn beginning<'a>(texts: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut all: Option<String> = None;
    for text in texts.filter(|text| !text.is_empty()) {
        let all = all.get_or_insert_with(String::new);
        if all.len() >= READ {
            break;
        }
        if !all.is_empty() {
            all.push(' ');
        }
        let mut end = text.len().min(READ - all.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        all.push_str(&text[..end]);
    }
    all
}

/// What `text`, a piece of the source, is to someone who reads the
/// pages: its first words, `limit` characters of them at most; or, where
/// it has no words, what kind of thing it is.
fn told(text: &str, limit: usize) -> String {
    let mut read = String::new();
    gather(&typst::syntax::parse(text), &mut read);
    let words: Vec<&str> = read.split_whitespace().collect();
    if words.is_empty() {
        let what = if !text.trim().is_empty() {
            "formatting"
        } else if text.contains('\n') {
            "a break"
        } else {
            "a blank"
        };
        return what.to_owned();
    }
    let words = words.join(" ");
    let mut shown: String = words.chars().take(limit).collect();
    if shown.len() < words.len() {
        shown.push('…');
    }
    format!("“{shown}”")
}

/// What `changes`, the changes of one step, did, in a line: whether text
/// was added, deleted or replaced, and the first words of it. (Not
/// "typed": the history does not know whether a key, a put or an agent
/// added it.)
fn describe(changes: &[Change]) -> String {
    let gone = beginning(changes.iter().map(|change| change.old.as_str()));
    let come = beginning(changes.iter().map(|change| change.new.as_str()));
    let what = match (gone, come) {
        (None, None) => return "nothing changed".to_owned(),
        (None, Some(come)) => format!("added {}", told(&come, WORDS)),
        (Some(gone), None) => format!("deleted {}", told(&gone, WORDS)),
        (Some(gone), Some(come)) => {
            let (gone, come) = (told(&gone, WORDS_EACH), told(&come, WORDS_EACH));
            if gone == come && !gone.starts_with('“') {
                format!("changed {gone}")
            } else {
                format!("replaced {gone} with {come}")
            }
        }
    };
    let step = Step {
        changes,
        direction: Direction::Forward,
        to: ROOT,
    };
    match step.ranges().len() {
        0 | 1 => what,
        places => format!("{what}  ({places} places)"),
    }
}

/// The list: a line for every state of `history`, the newest first, each
/// leading to its state. Behind a tab, where the state is: `now` for the
/// one the text is in, nothing for those undo leads back to, `undone`
/// for those redo leads to, and for all others that they are on another
/// branch.
fn rows(history: &History) -> Vec<(String, Target)> {
    let line = history.line();
    // Where on the line each state is, if it is on it.
    let mut places: Vec<Option<usize>> = vec![None; history.len()];
    for (place, state) in line.iter().enumerate() {
        if let Some(of_state) = usize::try_from(*state)
            .ok()
            .and_then(|at| places.get_mut(at))
        {
            *of_state = Some(place);
        }
    }
    let now = usize::try_from(history.current())
        .ok()
        .and_then(|at| places.get(at).copied().flatten());
    (0..history.len())
        .rev()
        .map(|index| {
            let state = index as StateId;
            let what = match history.changes(state) {
                Some(changes) if state != ROOT => describe(changes),
                _ => "the text before all of this".to_owned(),
            };
            let place = match (places[index], now) {
                (Some(place), Some(now)) if place == now => "now",
                (Some(place), Some(now)) if place < now => "",
                (Some(_), _) => "undone",
                (None, _) => "undone, on another branch",
            };
            let target = Target::Command(format!("history go {state}"));
            (format!("{what}\t{place}"), target)
        })
        .collect()
}

/// How many of the steps from the state `history` is in to `state` lead
/// back, and how many forward.
fn way(history: &History, state: StateId) -> (usize, usize) {
    let steps = history.steps(history.current(), state).unwrap_or_default();
    let back = steps
        .iter()
        .filter(|step| step.direction == Direction::Backward)
        .count();
    (back, steps.len() - back)
}

/// `count` steps, in words.
fn steps(count: usize) -> String {
    if count == 1 {
        "1 step".to_owned()
    } else {
        format!("{count} steps")
    }
}

impl Viewer {
    /// The commands of the history's list. Returns whether `command` is
    /// one of them.
    pub(super) fn history_command(&mut self, command: &str) -> bool {
        match command {
            "history" => self.open_history(None),
            "history leave" => self.leave_history(),
            _ => match command.strip_prefix("history go ").map(str::parse) {
                Some(Ok(state)) => self.go_in_history(state),
                _ => match command.strip_prefix("history at ").map(str::parse) {
                    Some(Ok(state)) => self.open_history(Some(state)),
                    _ => return false,
                },
            },
        }
        true
    }

    /// `gh`: puts up the history of the text being edited, with the
    /// state it is in chosen, or state `at`.
    fn open_history(&mut self, at: Option<StateId>) {
        let typing = |editor: &Editor| editor.mode() != editor::Mode::Normal;
        if self.editor.as_ref().is_some_and(typing) {
            // The list is for going through the history with `u` and
            // Ctrl-R: those are keys of normal mode.
            self.editor_key(KeyPress::plain(Key::Escape));
        }
        let Some(editor) = &self.editor else {
            return self.notice("there is no text being edited: no history");
        };
        let history = editor.history();
        if history.is_empty() {
            return self.notice("nothing was changed yet: the text has no history");
        }
        let items = rows(history);
        let current = usize::try_from(at.unwrap_or(history.current())).unwrap_or(0);
        let chosen = history.len().saturating_sub(current + 1);
        self.open_picker_for("history", items, chosen, "history leave".to_owned(), None);
        self.history = Some(Panel {
            shown: None,
            top: self.view.top,
            slide: self.slide,
        });
        if at.is_some() {
            self.history_looked();
        }
    }

    /// Escape: the list goes, and all is as it was when it came: the
    /// text was never anything else, the pages are made of it again and
    /// the view is where it was.
    fn leave_history(&mut self) {
        let Some(panel) = self.history.take() else {
            return;
        };
        if panel.shown.is_some() {
            self.text_changed(Edited::Command);
        }
        if self.paging == Paging::Slides {
            self.show_slide(panel.slide);
        } else {
            self.view.top = panel.top;
            self.view.clamp(&self.layout);
        }
    }

    /// After a key that was for a list: if the list is the history's,
    /// has the pages show the state that is chosen now.
    pub(super) fn history_looked(&mut self) {
        if self.history.is_none() {
            return;
        }
        let Some(picker) = &self.picker else {
            // The list went some other way than by its own commands.
            return self.leave_history();
        };
        // With nothing chosen (nothing is as typed), what was shown stays.
        if let Some(state) = picker.chosen().and_then(state_of) {
            self.look_at(state);
        }
    }

    /// Has the pages show state `state` of the history, and what the
    /// change that led to it made different. Only the pages: the text
    /// stays in the state it is in.
    fn look_at(&mut self, state: StateId) {
        let (Some(panel), Some(editor)) = (&self.history, &self.editor) else {
            return;
        };
        let now = editor.history().current();
        if state == panel.shown.as_ref().map_or(now, |shown| shown.state) {
            return;
        }
        let shown = if state == now {
            None
        } else {
            let Some(text) = editor.text_at(state) else {
                return;
            };
            let led = editor.history().changes(state).map(|changes| Step {
                changes,
                direction: Direction::Forward,
                to: state,
            });
            Some(Shown {
                state,
                source: Source::new(editor.file(), text),
                ranges: led.map(|step| step.ranges()).unwrap_or_default(),
                asked: None,
            })
        };
        if let Some(panel) = &mut self.history {
            panel.shown = shown;
        }
        // Other pages are wanted: as after a change of the text, though
        // there was none.
        self.text_changed(Edited::Command);
    }

    /// Whether the pages are those of another state of the history than
    /// the text is in, or are about to be: where things are on them says
    /// nothing about the text then.
    pub(super) fn looks_back(&self) -> bool {
        self.history
            .as_ref()
            .is_some_and(|panel| panel.shown.is_some())
    }

    /// What to typeset in place of the text as it is, while the history's
    /// list has another state chosen: the project with that state's text
    /// as the file being edited. The project itself is left as it is.
    pub(super) fn history_world(&mut self) -> Option<ProjectWorld> {
        let revision = self.revision;
        let file = self.editor.as_ref().map(Editor::file)?;
        let shown = self.history.as_mut()?.shown.as_mut()?;
        if shown.source.id() != file {
            return None;
        }
        shown.asked = Some(revision);
        let source = shown.source.clone();
        let written = self.project.source(file)?.clone();
        // What an equation that was left unfinished lacks is put in, as
        // for the text as it is: half an equation is none to Typst, and
        // many a state of the history ends in the middle of something.
        let put: Vec<(usize, &'static str)> = typst_text::equations(&source)
            .iter()
            .flat_map(|equation| typst_text::left_empty(&source, equation))
            .map(|place| (place, EMPTY))
            .collect();
        self.project.set_source(source);
        let world = if put.is_empty() {
            self.project.snapshot()
        } else {
            self.project.snapshot_showing(file, &put)
        };
        self.project.set_source(written);
        Some(world)
    }

    /// Enter: goes to state `state` of the history, by as many undos and
    /// redos as it takes, and shows what that changed.
    fn go_in_history(&mut self, state: StateId) {
        let looked = self
            .history
            .take()
            .is_some_and(|panel| panel.shown.is_some());
        let Some(editor) = &mut self.editor else {
            return;
        };
        let from = editor.history().current();
        let (back, forth) = way(editor.history(), state);
        let (state_is_behind, state_is_ahead) = (forth == 0, back == 0);
        let went = editor.travel(state);
        if looked || editor.history().current() != from {
            self.text_changed(Edited::Command);
        }
        let (back, forth) = (steps(back), steps(forth));
        match went {
            Err(why) => self.alert(format!("not changed: {why}")),
            Ok(()) if from == state => {}
            Ok(()) if state_is_behind => {
                self.notice(format!("{back} back; Ctrl-R leads forward again"));
            }
            Ok(()) if state_is_ahead => self.notice(format!("{forth} forward; u leads back")),
            Ok(()) => self.notice(format!(
                "{back} back and {forth} forward on another branch; u leads back"
            )),
        }
        // To what that changed, which is marked; else to the cursor.
        self.follow_cursor();
    }

    /// After a key for the text: if it undid or redid something, brings
    /// what that changed into view, and says how many places it is where
    /// it is more than one.
    pub(super) fn stepped(&mut self) {
        let stepped = self.editor.as_ref().and_then(Editor::stepped);
        let Some(places) = stepped.map(<[Range<usize>]>::len) else {
            return;
        };
        if places > 1 {
            self.notice(format!(
                "{places} places changed: all are marked, the first is in view"
            ));
        }
        self.follows_step();
    }

    /// What the last step changed, as stretches of the source to show in
    /// the source pane; a place text went from is an empty one.
    pub(super) fn step_stretches(&self) -> Vec<(Range<usize>, Mark)> {
        if self.history.is_some() {
            return Vec::new();
        }
        let stepped = self.editor.as_ref().and_then(Editor::stepped);
        let changed = |range: &Range<usize>| (range.clone(), Mark::Changed);
        stepped.unwrap_or_default().iter().map(changed).collect()
    }

    /// What is to be marked on the pages as changed by a step, in bytes
    /// of the text the pages were compiled from: what the last undo or
    /// redo changed, or, while the history's list is up, what the change
    /// that led to the state it shows did. `None` while the pages cannot
    /// tell where that is.
    fn marked(&self) -> Option<Vec<Range<usize>>> {
        if let Some(panel) = &self.history {
            let shown = panel.shown.as_ref()?;
            let typeset = shown.asked.is_some() && self.document_revision == shown.asked;
            return typeset.then(|| shown.ranges.clone());
        }
        let stepped = self.editor.as_ref()?.stepped()?;
        let change = self.places()?;
        let then =
            |range: &Range<usize>| change.back_or_edge(range.start)..change.back_or_edge(range.end);
        Some(stepped.iter().map(then).collect())
    }

    /// Brings the first of what a step changed into view, if the pages
    /// mark it (see [`Viewer::marked`]) and it is not in view. Returns
    /// whether there is such a thing: the view is about it then, not
    /// about the cursor.
    pub(super) fn follows_step(&mut self) -> bool {
        if self.shown_panes() == Panes::Source {
            return false;
        }
        let Some(first) = self.marked().and_then(|marked| marked.into_iter().next()) else {
            return false;
        };
        let (Some(document), Some(world)) = (&self.document, &self.compiled_world) else {
            return false;
        };
        let file = self.file_id();
        let Ok(source) = world.source(file) else {
            return false;
        };
        let at = world.shown_at(file, first.start);
        let near = self.cursor_page.get();
        let Some(position) = zetwerk_typst::locate(document, &source, at, near) else {
            return false;
        };
        let band = if self.history.is_some() {
            ABOVE_LIST
        } else {
            IN_VIEW
        };
        self.show_place(position, band);
        true
    }

    /// Scrolls so that `position` on the pages is in view: not at all if
    /// it is within `band` already.
    fn show_place(&mut self, position: PagePosition, (from, until, to): Band) {
        if self.paging == Paging::Slides {
            return self.show_slide(position.page);
        }
        let Some(page) = self.layout.pages.get(position.page) else {
            return;
        };
        let zoom = self.view.zoom;
        let y = f64::from(page.y) + position.y * zoom;
        let (top, height) = (self.view.top * zoom, f64::from(self.view.height));
        if y < top + height * from || y > top + height * until {
            self.view.top = (y - height * to) / zoom;
            self.view.clamp(&self.layout);
        }
    }

    /// Marks on page `index`, whose top-left corner is drawn at `origin`,
    /// what a step changed (see [`Viewer::marked`]): a tint over text the
    /// step put there, a bar at a place it took text from, and a bar too
    /// where what it put there shows as nothing.
    pub(super) fn step_marks(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        index: usize,
        origin: (f64, f64),
    ) {
        let Some(marked) = self.marked() else {
            return;
        };
        let file = self.file_id();
        let zoom = self.view.zoom;
        let bar = (PLACE_WIDTH * self.scale).round().max(2.0);
        let (Some(document), Some(world)) = (&self.document, &self.compiled_world) else {
            return;
        };
        let Some(page) = document.pages().get(index) else {
            return;
        };
        let map = self
            .maps
            .entry(index)
            .or_insert_with(|| PageMap::build(world, &page.frame));
        let place = |rect: typst::layout::Rect| {
            (
                origin.0 + rect.min.x.to_pt() * zoom,
                origin.1 + rect.min.y.to_pt() * zoom,
                (rect.max.x - rect.min.x).to_pt() * zoom,
                (rect.max.y - rect.min.y).to_pt() * zoom,
            )
        };
        for range in marked {
            let set = if range.is_empty() {
                Vec::new()
            } else {
                map.selection(file, &range)
            };
            for rect in &set {
                tint(scene, has, PAGE_AGENT_MARK, place(*rect));
            }
            if set.is_empty()
                && let Some(caret) = map.caret(file, range.start)
            {
                let (x, y, width, height) = place(caret.cell);
                let x = if caret.after { x + width } else { x };
                let bar = ((x - bar / 2.0).round(), y, bar, height);
                tint(scene, has, PAGE_STEP_PLACE, bar);
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use std::path::Path;

    use super::super::tests::{compile, editing, open, press, tints};
    use super::*;

    /// The lines of the history's list.
    fn listed(viewer: &Viewer) -> Vec<String> {
        let history = viewer.editor.as_ref().unwrap().history();
        rows(history).into_iter().map(|(line, _)| line).collect()
    }

    /// What the status line says.
    fn said(viewer: &Viewer) -> &str {
        viewer.message.as_ref().map_or("", |message| &message.text)
    }

    /// The text the pages on screen were typeset from.
    fn typeset(viewer: &Viewer) -> String {
        let world = viewer.compiled_world.as_ref().unwrap();
        world.source(viewer.file_id()).unwrap().text().to_owned()
    }

    /// The bytes of the journal kept for the document in `directory`.
    fn journal(directory: &Path) -> Vec<u8> {
        let journals = directory.join("state").join("journals");
        let journal = std::fs::read_dir(journals)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|ending| ending == "journal"));
        std::fs::read(journal.unwrap()).unwrap()
    }

    #[test]
    fn undo_and_redo_show_what_they_changed_until_the_next_key() {
        let (mut viewer, directory) = editing("The quick brown fox.\n\nA second paragraph.\n");
        press(&mut viewer, "wdw");
        assert_eq!(
            viewer.text(),
            Some("The brown fox.\n\nA second paragraph.\n")
        );
        compile(&mut viewer);
        assert_eq!(tints(&mut viewer, PAGE_AGENT_MARK).len(), 0);
        assert_eq!(tints(&mut viewer, PAGE_STEP_PLACE).len(), 0);

        // The word comes back, and is tinted where it stands.
        press(&mut viewer, "u");
        assert_eq!(
            viewer.text(),
            Some("The quick brown fox.\n\nA second paragraph.\n")
        );
        compile(&mut viewer);
        let back = tints(&mut viewer, PAGE_AGENT_MARK);
        assert_eq!(back.len(), 1, "one word on one line");
        assert_eq!(tints(&mut viewer, PAGE_STEP_PLACE).len(), 0);
        // In the source too.
        let stretches = viewer.step_stretches();
        assert_eq!(stretches, vec![(4..10, Mark::Changed)]);

        // It goes again: a bar where it went from, at the left edge of
        // what stood behind it.
        press(&mut viewer, "<C-r>");
        compile(&mut viewer);
        assert_eq!(tints(&mut viewer, PAGE_AGENT_MARK).len(), 0);
        let gone = tints(&mut viewer, PAGE_STEP_PLACE);
        assert_eq!(gone.len(), 1);
        assert!((gone[0].x + gone[0].width / 2.0 - back[0].x).abs() < 1.5);
        assert_eq!(viewer.step_stretches(), vec![(4..4, Mark::Changed)]);

        // Any key ends the showing, also one that changes nothing.
        press(&mut viewer, "l");
        assert_eq!(tints(&mut viewer, PAGE_AGENT_MARK).len(), 0);
        assert_eq!(tints(&mut viewer, PAGE_STEP_PLACE).len(), 0);
        assert_eq!(viewer.step_stretches().len(), 0);
        // And so does a change no key made.
        press(&mut viewer, "u");
        assert_eq!(viewer.step_stretches().len(), 1);
        viewer.agent_insert("x", None, false).unwrap();
        assert_eq!(viewer.step_stretches().len(), 0);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_step_that_changed_several_places_marks_them_all() {
        let (mut viewer, directory) = editing("The fox.\n\nThe dog.\n\nThe end.\n");
        viewer.agent_replace("The", "A", true, None, false).unwrap();
        assert_eq!(viewer.text(), Some("A fox.\n\nA dog.\n\nA end.\n"));
        compile(&mut viewer);
        press(&mut viewer, "u");
        assert_eq!(viewer.text(), Some("The fox.\n\nThe dog.\n\nThe end.\n"));
        assert!(
            said(&viewer).starts_with("3 places changed"),
            "{}",
            said(&viewer)
        );
        compile(&mut viewer);
        assert_eq!(tints(&mut viewer, PAGE_AGENT_MARK).len(), 3);
        // Two steps undone by one key: what both changed.
        press(&mut viewer, "<C-r>");
        press(&mut viewer, "A More.<Esc>");
        compile(&mut viewer);
        press(&mut viewer, "2u");
        assert_eq!(viewer.text(), Some("The fox.\n\nThe dog.\n\nThe end.\n"));
        compile(&mut viewer);
        // The three words that are back, and where " More." went.
        assert_eq!(tints(&mut viewer, PAGE_AGENT_MARK).len(), 3);
        assert_eq!(tints(&mut viewer, PAGE_STEP_PLACE).len(), 1);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_view_goes_to_what_a_step_changed() {
        // One paragraph over several pages: a line of the source, so the
        // cursor has no reason to leave its end.
        let long = "word ".repeat(2500);
        let (mut viewer, directory) = editing(&format!("Start {long}end.\n"));
        assert!(viewer.state().pages > 2);
        press(&mut viewer, "dw");
        drop(viewer);
        // Another day: nobody remembers where the cursor was then.
        let (mut viewer, _) = open(&directory);
        press(&mut viewer, "$");
        let end = viewer.view.top;
        assert!(end > 1000.0, "the view is at the paragraph's end");
        press(&mut viewer, "u");
        let cursor = viewer.editor.as_ref().unwrap().cursor();
        assert!(cursor > 12000, "the cursor is still at the end");
        compile(&mut viewer);
        assert!(
            viewer.view.top < 100.0,
            "the view is where the word came back"
        );
        assert_eq!(tints(&mut viewer, PAGE_AGENT_MARK).len(), 1);
        // The next key is about the cursor again.
        press(&mut viewer, "h");
        assert!(viewer.view.top > 1000.0);
        let _ = std::fs::remove_dir_all(directory);
    }

    /// A text with four steps of history: the states 1 to 4.
    fn with_history() -> (Viewer, std::path::PathBuf) {
        let (mut viewer, directory) = editing("One two three.\n\nA *second* paragraph.\n");
        press(&mut viewer, "A Four.<Esc>");
        press(&mut viewer, "0dw");
        press(&mut viewer, "/second<CR>cwthird<Esc>");
        press(&mut viewer, "gg0vegb");
        assert_eq!(
            viewer.text(),
            Some("*two* three. Four.\n\nA *third* paragraph.\n")
        );
        compile(&mut viewer);
        viewer.take_edited();
        (viewer, directory)
    }

    #[test]
    fn the_list_tells_every_step_in_words_of_the_text() {
        let (mut viewer, directory) = with_history();
        assert_eq!(
            listed(&viewer),
            [
                "added formatting  (2 places)\tnow",
                "replaced “second” with “third”\t",
                "deleted “One”\t",
                "added “Four.”\t",
                "the text before all of this\t",
            ]
        );
        // What was undone is listed as that, and is not gone.
        press(&mut viewer, "2u");
        assert_eq!(
            listed(&viewer)[..3],
            [
                "added formatting  (2 places)\tundone",
                "replaced “second” with “third”\tundone",
                "deleted “One”\tnow",
            ]
        );
        // The history is the journal's: it is there another day.
        drop(viewer);
        let (mut viewer, _) = open(&directory);
        assert_eq!(listed(&viewer).len(), 5);
        assert_eq!(listed(&viewer)[2], "deleted “One”\tnow");

        // No source in the list: what has no words is told as what it is.
        assert_eq!(
            told("#set text(font: \"Libertinus Serif\")\n", 44),
            "formatting"
        );
        assert_eq!(told("*", 44), "formatting");
        assert_eq!(told("\n\n", 44), "a break");
        assert_eq!(told(" ", 44), "a blank");
        assert_eq!(told("= A _fine_ heading\n", 44), "“A fine heading”");
        assert_eq!(
            told("#figure(image(\"cat.png\"), caption: [A cat])", 44),
            "“A cat”"
        );
        assert_eq!(told("one two three four five six", 11), "“one two thr…”");
        let nothing = Change {
            at: 3,
            old: String::new(),
            new: String::new(),
        };
        assert_eq!(describe(&[nothing]), "nothing changed");

        // A text nothing was done to has no history to show.
        let (mut fresh, other) = editing("Untouched.\n");
        press(&mut fresh, "gh");
        assert!(!fresh.is_picking());
        assert!(said(&fresh).contains("no history"));
        // The list of all there is to do has the key.
        press(&mut viewer, "g?history");
        let chosen = viewer.picker.as_ref().and_then(|picker| picker.chosen());
        assert_eq!(chosen, Some(&Target::Command("history".to_owned())));
        let _ = std::fs::remove_dir_all(directory);
        let _ = std::fs::remove_dir_all(other);
    }

    #[test]
    fn looking_at_the_history_shows_it_and_changes_nothing() {
        let (mut viewer, directory) = with_history();
        let text = viewer.text().unwrap().to_owned();
        let history = viewer.editor.as_ref().unwrap().history().clone();
        let written = journal(&directory);
        let cursor = viewer.editor.as_ref().unwrap().cursor();

        press(&mut viewer, "gh");
        assert!(viewer.is_picking());
        // The state the text is in comes up chosen: the pages stay.
        assert_eq!(viewer.take_edited(), None);
        assert!(!viewer.looks_back());

        // One down, one step back: the pages are made of that state.
        press(&mut viewer, "<Down>");
        assert_eq!(viewer.take_edited(), Some(Edited::Command));
        assert!(!viewer.is_compiled());
        compile(&mut viewer);
        assert_eq!(
            typeset(&viewer),
            "two three. Four.\n\nA *third* paragraph.\n"
        );
        // What led to it is marked: the word that replaced another.
        assert_eq!(tints(&mut viewer, PAGE_AGENT_MARK).len(), 1);
        // The text itself is where it was, and has no cursor to show on
        // pages that are not its own.
        assert_eq!(viewer.text(), Some(text.as_str()));
        assert!(viewer.places().is_none());

        // Typing narrows the list, and the pages follow what is chosen.
        for key in "deleted".chars() {
            viewer.key(KeyPress::plain(Key::Char(key)));
        }
        compile(&mut viewer);
        assert_eq!(
            typeset(&viewer),
            "two three. Four.\n\nA *second* paragraph.\n"
        );
        assert_eq!(tints(&mut viewer, PAGE_AGENT_MARK).len(), 0);
        assert_eq!(tints(&mut viewer, PAGE_STEP_PLACE).len(), 1);
        // Back at the state the text is in: its own pages.
        press(&mut viewer, "<BS><BS><BS><BS><BS><BS><BS>");
        compile(&mut viewer);
        assert_eq!(typeset(&viewer), text);
        assert!(!viewer.looks_back() && viewer.places().is_some());
        // The beginning of it all.
        press(&mut viewer, "<Up>");
        compile(&mut viewer);
        assert_eq!(
            typeset(&viewer),
            "One two three.\n\nA *second* paragraph.\n"
        );

        // Escape: all is as it was, and nothing was ever written.
        press(&mut viewer, "<Esc>");
        assert!(!viewer.is_picking() && viewer.history.is_none());
        assert_eq!(viewer.take_edited(), Some(Edited::Command));
        compile(&mut viewer);
        assert_eq!(typeset(&viewer), text);
        assert_eq!(viewer.text(), Some(text.as_str()));
        let editor = viewer.editor.as_ref().unwrap();
        assert_eq!(editor.history(), &history);
        assert_eq!(editor.cursor(), cursor);
        assert_eq!(journal(&directory), written, "looking wrote to the journal");
        assert!(viewer.places().is_some());
        // Undo is what it would have been.
        press(&mut viewer, "u");
        assert_eq!(
            viewer.text(),
            Some("two three. Four.\n\nA *third* paragraph.\n")
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn enter_goes_to_the_chosen_step_by_undoing() {
        let (mut viewer, directory) = with_history();
        let text = viewer.text().unwrap().to_owned();
        press(&mut viewer, "gh<Down><Down><Down>");
        compile(&mut viewer);
        press(&mut viewer, "<CR>");
        assert!(!viewer.is_picking() && viewer.history.is_none());
        assert_eq!(
            viewer.text(),
            Some("One two three. Four.\n\nA *second* paragraph.\n")
        );
        assert_eq!(said(&viewer), "3 steps back; Ctrl-R leads forward again");
        assert_eq!(viewer.take_edited(), Some(Edited::Command));
        compile(&mut viewer);
        assert_eq!(typeset(&viewer), viewer.text().unwrap());
        // All that the three steps changed is marked: the word that is
        // back at the start, the one that is back in the second
        // paragraph, and the two places the formatting went from, of
        // which the first is where the word came back.
        assert_eq!(tints(&mut viewer, PAGE_AGENT_MARK).len(), 2);
        assert_eq!(tints(&mut viewer, PAGE_STEP_PLACE).len(), 1);
        // No state was made or lost, and the keys lead on as ever.
        assert_eq!(viewer.editor.as_ref().unwrap().history().len(), 5);
        press(&mut viewer, "<C-r>");
        assert_eq!(
            viewer.text(),
            Some("two three. Four.\n\nA *second* paragraph.\n")
        );
        press(&mut viewer, "2<C-r>");
        assert_eq!(viewer.text(), Some(text.as_str()));
        press(&mut viewer, "u");
        // Forward too, and to where the text is already.
        press(&mut viewer, "gh<Up><CR>");
        assert_eq!(viewer.text(), Some(text.as_str()));
        assert_eq!(said(&viewer), "1 step forward; u leads back");
        press(&mut viewer, "gh<CR>");
        assert_eq!(viewer.text(), Some(text.as_str()));
        assert_eq!(viewer.take_edited(), Some(Edited::Command));
        press(&mut viewer, "gh<CR>");
        assert_eq!(viewer.take_edited(), None, "nothing to typeset anew");
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn branches_are_listed_and_can_be_gone_to() {
        let (mut viewer, directory) = editing("base\n");
        press(&mut viewer, "A one<Esc>A two<Esc>uA three<Esc>");
        assert_eq!(viewer.text(), Some("base one three\n"));
        assert_eq!(
            listed(&viewer),
            [
                "added “three”\tnow",
                "added “two”\tundone, on another branch",
                "added “one”\t",
                "the text before all of this\t",
            ]
        );
        // Looking at the branch, then going there.
        press(&mut viewer, "gh<Down>");
        compile(&mut viewer);
        assert_eq!(typeset(&viewer), "base one two\n");
        assert_eq!(viewer.text(), Some("base one three\n"));
        press(&mut viewer, "<CR>");
        assert_eq!(viewer.text(), Some("base one two\n"));
        assert_eq!(
            said(&viewer),
            "1 step back and 1 step forward on another branch; u leads back"
        );
        assert_eq!(
            listed(&viewer)[..2],
            [
                "added “three”\tundone, on another branch",
                "added “two”\tnow",
            ]
        );
        // Redo leads down the branch that was gone to.
        press(&mut viewer, "u");
        assert_eq!(viewer.text(), Some("base one\n"));
        press(&mut viewer, "<C-r>");
        assert_eq!(viewer.text(), Some("base one two\n"));
        // And the other is as near as it was: nothing is out of reach.
        press(&mut viewer, "gh<Up><CR>");
        assert_eq!(viewer.text(), Some("base one three\n"));
        assert_eq!(viewer.editor.as_ref().unwrap().history().len(), 4);
        // Where redo leads is in the journal: it is so another day.
        press(&mut viewer, "u");
        drop(viewer);
        let (mut viewer, _) = open(&directory);
        assert_eq!(viewer.text(), Some("base one\n"));
        press(&mut viewer, "<C-r>");
        assert_eq!(viewer.text(), Some("base one three\n"));
        let _ = std::fs::remove_dir_all(directory);
    }
}
