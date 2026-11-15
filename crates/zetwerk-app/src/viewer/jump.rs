//! Labels to jump by: on a typeset page `w` and `j` are hard to count,
//! so `gw` (or `:jump`) puts a letter or two on every word in view, and
//! typing what a word says puts the cursor there. It is the jump of
//! Vim's easymotion, hop and leap.
//!
//! The places are the beginnings of the words of the pages in view, of
//! all of them and not only the cursor's, and the first glyph of each
//! equation and of each thing the document makes of markup (a
//! reference, a citation): headings, captions and the first words of
//! lines are words too. Only what is set from the file being edited is
//! labelled: the cursor cannot go into another file by a jump.
//!
//! The labels are made once, when the key is pressed: which places are
//! in view, how near the cursor each is, what it is called. Drawing
//! them is a lookup. The places nearest the cursor have one letter,
//! the others two; no label is the beginning of another, so a label
//! is complete when it is typed. The letters are those under the
//! fingers first.
//!
//! While the labels are up, keys are theirs: a letter narrows them
//! (the labels it does not begin are gone, the others show what is
//! left of them), Backspace takes the letter back, Escape takes the
//! labels away, and a whole label moves the cursor and takes them
//! away. In visual mode the move extends the selection. With an
//! operator pending (`d` and then `gw`) there is no jump: the operator
//! waits for a motion of the text, which a label is not; `v`, the
//! jump and the operator do the same.
//!
//! Like the other aids (see `aids`), the labels are drawn over the
//! page and are no part of the document. They have the same size on
//! screen at any zoom: small, light letters on a ground that lets the
//! word under it show through.

use std::sync::Arc;

use typst::layout::Rect;
use typst::syntax::FileId;
use zetwerk_typst::{Origin, PageMap};

use super::{Change, Image, Panes, Quad, ROW_HEIGHT, RowText, Scene, TextureId, Viewer, tint};
use crate::keys::{Key, KeyPress};

/// The letters labels are made of: the home row first, then the row
/// above it, then the one below.
const KEYS: &str = "fjdkslaghrueiwoqptyvmcnbxz";

/// How high a label stands on screen, in pixels of the interface (see
/// `Viewer::scale`): the height of a row of the source pane its letters
/// are set as.
///
/// As large as the letters of the status line: seen in a window, smaller
/// ones could not be read, and a label that cannot be read at a glance
/// is slower than `w`.
const HEIGHT: f64 = 20.0;

/// The ground under a label: dark enough for its light letters to be
/// read at a glance. The beginning of the word under it is hidden for
/// as long as the labels are up; the rest of the word says which it is.
const GROUND: [u8; 4] = [0x28, 0x28, 0x28, 0xe6];

/// How far a label's ground reaches beyond its letters at either side,
/// and how much of the height of a letter's row it leaves out at the top
/// and at the bottom, in pixels of the interface.
const PAD: f64 = 1.5;
const INSET: f64 = 2.0;

/// How much more a line's distance counts than the same distance
/// across: the places of the lines next to the cursor's come before
/// those far along its own.
const DOWN_WEIGHT: f64 = 2.0;

/// The labels, while they are up, and the letters they are drawn with.
#[derive(Default)]
pub(super) struct Jump {
    labels: Option<Labels>,
    /// An image of each letter of [`KEYS`], in that order, and the scale
    /// (as bits) they were set at: set once, drawn as often as needed.
    letters: Option<(u64, Vec<Image>)>,
}

/// The labels that are up, and what of them has been typed.
struct Labels {
    all: Vec<Label>,
    typed: String,
    /// The revision of the text and of the pages the labels were made
    /// for: when either has changed, they say nothing true.
    made_for: (u64, Option<u64>),
}

/// A label: the place it stands at (a page, and the left edge and the
/// middle of the place's glyph, in points from the page's top-left
/// corner), what it is called, and the byte of the text it leads to.
struct Label {
    page: usize,
    left: f64,
    middle: f64,
    name: String,
    target: usize,
}

/// A place a label may be put on: where it is (on which page, the cell
/// of its first glyph), how far from the cursor it is on screen, and the
/// byte it leads to.
struct Candidate {
    page: usize,
    cell: Rect,
    distance: f64,
    target: usize,
}

impl Viewer {
    /// Whether labels are up, and keys go to them.
    #[must_use]
    pub fn is_jumping(&self) -> bool {
        self.jump.labels.is_some()
    }

    /// `gw`, `:jump`: labels on the places in view.
    pub(super) fn start_jump(&mut self) {
        self.jump.labels = None;
        if self.editor.is_none() {
            return;
        }
        if self.shown_panes() == Panes::Source {
            self.notice("jump labels are put on the pages: gs shows them");
            return;
        }
        let Some(change) = self.places() else {
            self.notice("the pages are being set: jump again in a moment");
            return;
        };
        let mut candidates = self.candidates(change);
        candidates.sort_by(|one, other| one.distance.total_cmp(&other.distance));
        // One label for each place on a page, the nearest: what one
        // piece of markup made may be set on several lines.
        let mut seen = std::collections::HashSet::new();
        candidates.retain(|candidate| seen.insert((candidate.page, candidate.target)));
        let room = KEYS.len() * KEYS.len();
        candidates.truncate(room);
        match candidates.as_slice() {
            [] => return self.notice("nothing to jump to in view"),
            [only] => return self.jump_to(only.page, only.target),
            _ => {}
        }
        let all = candidates
            .iter()
            .zip(names(candidates.len()))
            .map(|(candidate, name)| Label {
                page: candidate.page,
                left: candidate.cell.min.x.to_pt(),
                middle: ((candidate.cell.min.y + candidate.cell.max.y) / 2.0).to_pt(),
                name,
                target: candidate.target,
            })
            .collect();
        self.jump.labels = Some(Labels {
            all,
            typed: String::new(),
            made_for: (self.revision, self.document_revision),
        });
        self.set_letters();
        self.notice("jump: type a label  (Esc: stay)");
    }

    /// The places in view a label may be put on, and how far from the
    /// cursor each is. `change` is how the text differs from the text
    /// the pages were compiled from.
    fn candidates(&mut self, change: Change) -> Vec<Candidate> {
        let (Some(editor), Some(document), Some(world)) =
            (&self.editor, &self.document, &self.compiled_world)
        else {
            return Vec::new();
        };
        let (file, text) = (editor.file(), editor.text());
        let cursor = change.back_or_edge(editor.cursor());
        let zoom = self.view.zoom;
        let visible = self.view.visible(&self.layout);
        // Where the cursor is on screen; the middle of the view where no
        // page in view shows it.
        let mut from = (
            f64::from(self.view.width) / 2.0,
            f64::from(self.view.height) / 2.0,
        );
        let (mut found, mut at) = (Vec::new(), Vec::new());
        for page in &visible {
            let Some(frame) = document.pages().get(page.index).map(|page| &page.frame) else {
                continue;
            };
            let map = self
                .maps
                .entry(page.index)
                .or_insert_with(|| PageMap::build(world, frame));
            let screen = |x: f64, y: f64| (page.x as f64 + x * zoom, page.y as f64 + y * zoom);
            if page.index == self.cursor_page.get()
                && let Some(caret) = map.caret(file, cursor)
            {
                let cell = caret.cell;
                from = screen(
                    cell.min.x.to_pt(),
                    ((cell.min.y + cell.max.y) / 2.0).to_pt(),
                );
            }
            let region = page.region;
            let (left, top) = (f64::from(region.x) / zoom, f64::from(region.y) / zoom);
            let (right, bottom) = (
                left + f64::from(region.width) / zoom,
                top + f64::from(region.height) / zoom,
            );
            for (cell, then) in starts(map, file, text, change) {
                let (x, y) = (
                    cell.min.x.to_pt(),
                    ((cell.min.y + cell.max.y) / 2.0).to_pt(),
                );
                if x < left || x >= right || y < top || y >= bottom {
                    continue;
                }
                found.push(Candidate {
                    page: page.index,
                    cell,
                    distance: 0.0,
                    target: change.forth(then),
                });
                at.push(screen(x, y));
            }
        }
        for (candidate, (x, y)) in found.iter_mut().zip(at) {
            candidate.distance = (x - from.0).hypot((y - from.1) * DOWN_WEIGHT);
        }
        found
    }

    /// Sets the images of the letters labels are drawn with, unless they
    /// are set for the scale of the interface already.
    fn set_letters(&mut self) {
        let scale = self.scale * HEIGHT / ROW_HEIGHT;
        if self
            .jump
            .letters
            .as_ref()
            .is_some_and(|(at, _)| *at == scale.to_bits())
        {
            return;
        }
        let rows: Vec<RowText> = KEYS
            .chars()
            .map(|letter| RowText {
                text: letter.to_string(),
                dim: false,
            })
            .collect();
        let images = self.chrome.render_rows(&rows, scale);
        self.jump.letters = (images.len() == rows.len()).then_some((scale.to_bits(), images));
    }

    /// A key while labels are up. Returns whether it was theirs: all keys
    /// are, while there are labels.
    pub(super) fn jump_key(&mut self, press: KeyPress) -> bool {
        let Some(labels) = &mut self.jump.labels else {
            return false;
        };
        if labels.made_for != (self.revision, self.document_revision) {
            self.jump.labels = None;
            self.notice("the pages changed: gw puts the labels up again");
            return true;
        }
        match press.key {
            Key::Char(letter) if !press.ctrl && !press.alt => {
                labels.typed.push(letter.to_ascii_lowercase());
            }
            Key::Backspace => {
                if labels.typed.pop().is_none() {
                    self.jump.labels = None;
                    return true;
                }
            }
            // Escape, and any other key: no jump.
            _ => {
                self.jump.labels = None;
                return true;
            }
        }
        let typed = labels.typed.clone();
        let begun = labels
            .all
            .iter()
            .any(|label| label.name.starts_with(&typed));
        // No label begins another: one that is typed whole is the one.
        let whole = labels
            .all
            .iter()
            .find(|label| label.name == typed)
            .map(|label| (label.page, label.target));
        if let Some((page, target)) = whole {
            self.jump.labels = None;
            self.jump_to(page, target);
        } else if begun {
            self.notice(format!("jump: {typed}…  (Esc: stay)"));
        } else {
            self.jump.labels = None;
            self.notice(format!("no label {typed}: stayed"));
        }
        true
    }

    /// Puts the cursor at byte `target` of the text, which is set on page
    /// `page`: as a jump, so that `` ` ` `` leads back; in visual mode,
    /// the selection goes along.
    fn jump_to(&mut self, page: usize, target: usize) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        editor.jump(target);
        // The same text may be on several pages (a heading and the table
        // of contents): the cursor is where the label was.
        self.cursor_page.set(page);
        self.message = None;
        self.follow_cursor();
    }

    /// Draws the labels that stand on page `index` (whose top-left corner
    /// is at `origin`), each with what is left of it to type.
    pub(super) fn jump_labels(
        &self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        index: usize,
        origin: (f64, f64),
    ) {
        let (Some(labels), Some((_, letters))) = (&self.jump.labels, &self.jump.letters) else {
            return;
        };
        if labels.made_for != (self.revision, self.document_revision) {
            return;
        }
        let zoom = self.view.zoom;
        let (pad, inset) = (PAD * self.scale, INSET * self.scale);
        let typed = labels.typed.as_str();
        for label in &labels.all {
            if label.page != index || !label.name.starts_with(typed) {
                continue;
            }
            let shown: Vec<&Image> = label.name[typed.len()..]
                .chars()
                .filter_map(|letter| KEYS.find(letter).and_then(|at| letters.get(at)))
                .collect();
            let width: f64 = shown
                .iter()
                .map(|image| f64::from(image.pixmap.width()))
                .sum();
            let height = shown
                .iter()
                .map(|image| f64::from(image.pixmap.height()))
                .fold(0.0, f64::max);
            let x = (origin.0 + label.left * zoom).round();
            let y = (origin.1 + label.middle * zoom - height / 2.0).round();
            tint(
                scene,
                has,
                GROUND,
                (x - pad, y + inset, width + 2.0 * pad, height - 2.0 * inset),
            );
            let mut across = x;
            for image in shown {
                let size = (
                    f64::from(image.pixmap.width()),
                    f64::from(image.pixmap.height()),
                );
                scene.quads.push(Quad {
                    texture: image.id,
                    x: across as f32,
                    y: y as f32,
                    width: size.0 as f32,
                    height: size.1 as f32,
                });
                if !has(image.id) && !scene.images.iter().any(|held| held.id == image.id) {
                    scene.images.push(Image {
                        id: image.id,
                        pixmap: Arc::clone(&image.pixmap),
                    });
                }
                across += size.0;
            }
        }
    }
}

/// The places on a page a label may be put on: the cell of the first
/// glyph of each, and the byte of the compiled text it leads to. `text`
/// is the text as it is now, which differs from the compiled one by
/// `change`.
///
/// A place is the beginning of a word of `file` (a letter or a digit
/// after something that is neither), the first glyph of an equation,
/// and the first glyph of what one piece of markup made.
fn starts(map: &PageMap, file: FileId, text: &str, change: Change) -> Vec<(Rect, usize)> {
    let mut found = Vec::new();
    let mut before: Option<&Origin> = None;
    for glyph in map.glyphs().iter().filter(|glyph| !glyph.furniture) {
        let origin = &glyph.origin;
        let start = match origin {
            Origin::Text { file: of, range } if *of == file && !glyph.blank => {
                begins_word(text, change.forth(range.start)).then_some(range.start)
            }
            Origin::Math { file: of, range } if *of == file => {
                let goes_on = matches!(before, Some(Origin::Math { file: was, .. }) if was == of);
                (!goes_on).then_some(range.start)
            }
            Origin::Object {
                file: of, range, ..
            } if *of == file => {
                let goes_on = matches!(
                    before,
                    Some(Origin::Object { file: was, range: made, .. }) if was == of && made == range
                );
                (!goes_on).then_some(range.start)
            }
            _ => None,
        };
        if let Some(start) = start {
            found.push((glyph.rect, start));
        }
        before = Some(origin);
    }
    found
}

/// Whether a word begins at byte `at` of `text`: a letter or a digit is
/// there, and before it is neither, nor an apostrophe (of "don't").
fn begins_word(text: &str, at: usize) -> bool {
    let first = text.get(at..).and_then(|rest| rest.chars().next());
    if !first.is_some_and(char::is_alphanumeric) {
        return false;
    }
    let before = text.get(..at).and_then(|rest| rest.chars().next_back());
    before.is_none_or(|before| !(before.is_alphanumeric() || matches!(before, '\'' | '’')))
}

/// Names for `count` places, the nearest first: one letter for as many
/// as can have one while the others still find a name of two, beginning
/// with one of the letters left. None is the beginning of another. At
/// most as many as there are names of two letters.
fn names(count: usize) -> Vec<String> {
    let keys: Vec<char> = KEYS.chars().collect();
    let room = keys.len() * keys.len();
    let count = count.min(room);
    let single = if count <= keys.len() {
        count
    } else {
        (room - count) / (keys.len() - 1)
    };
    let mut names: Vec<String> = keys[..single].iter().map(char::to_string).collect();
    for first in &keys[single..] {
        for second in &keys {
            if names.len() == count {
                return names;
            }
            names.push(format!("{first}{second}"));
        }
    }
    names
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use std::fmt::Write;

    use super::super::tests::{compile, editing, press, tints};
    use super::*;

    /// Many words, two pages, and a word to find at the very end.
    fn words() -> String {
        let mut text = String::from("= Heading\n\nFirst words here, and $x^2$ too.\n\n");
        for paragraph in 0..6 {
            for word in 0..30 {
                let _ = write!(text, "w{paragraph}x{word} ");
            }
            text.push_str("\n\n");
        }
        text.push_str("#pagebreak()\nFarAway\n");
        text
    }

    /// The labels that are up and begin with what was typed, as name
    /// and the byte each leads to.
    fn up(viewer: &Viewer) -> Vec<(String, usize)> {
        viewer.jump.labels.as_ref().map_or_else(Vec::new, |labels| {
            labels
                .all
                .iter()
                .filter(|label| label.name.starts_with(&labels.typed))
                .map(|label| (label.name.clone(), label.target))
                .collect()
        })
    }

    fn cursor(viewer: &Viewer) -> usize {
        viewer.editor.as_ref().unwrap().cursor()
    }

    #[test]
    fn names_are_short_near_and_none_begins_another() {
        assert_eq!(names(3), ["f", "j", "d"]);
        for count in [1, 25, 26, 27, 60, 300, 675, 676, 1000] {
            let names = names(count);
            assert_eq!(names.len(), count.min(676));
            for one in &names {
                for other in &names {
                    assert!(
                        one == other || !other.starts_with(one.as_str()),
                        "{one} {other}"
                    );
                }
            }
            // The nearer, the shorter.
            assert!(names.windows(2).all(|pair| pair[0].len() <= pair[1].len()));
        }
        assert_eq!(names(27).iter().filter(|name| name.len() == 1).count(), 25);
    }

    #[test]
    fn labels_are_put_on_the_words_in_view() {
        let text = words();
        let (mut viewer, _) = editing(&text);
        press(&mut viewer, "gsgs");
        press(&mut viewer, "gw");
        assert!(viewer.is_jumping());
        let labels = up(&viewer);
        assert!(labels.len() > KEYS.len(), "{}", labels.len());
        for (_, target) in &labels {
            assert!(begins_word(&text, *target) || text[*target..].starts_with("x^2"));
        }
        let at = |word: &str| text.find(word).unwrap();
        let targets: Vec<usize> = labels.iter().map(|(_, target)| *target).collect();
        for word in ["Heading", "First", "words", "and", "w0x0", "w0x29"] {
            assert!(targets.contains(&at(word)), "{word}");
        }
        // Not in the middle of a word, not on a page out of view.
        assert!(!targets.contains(&(at("First") + 1)));
        assert!(!targets.contains(&at("FarAway")));
        // The nearest place, the cursor's own word, has a letter.
        assert_eq!(labels[0], ("f".to_owned(), 2));
        assert!(labels.iter().all(|(one, _)| {
            labels
                .iter()
                .all(|(other, _)| one == other || !other.starts_with(one.as_str()))
        }));
        // Each drawn on a ground of its own; and gone once Escape is typed,
        // with the cursor where it was.
        assert_eq!(tints(&mut viewer, GROUND).len(), labels.len());
        press(&mut viewer, "<Esc>");
        assert!(!viewer.is_jumping());
        assert_eq!(tints(&mut viewer, GROUND).len(), 0);
        assert_eq!(cursor(&viewer), 2);
    }

    #[test]
    fn typing_a_label_moves_the_cursor_there() {
        let text = words();
        let (mut viewer, _) = editing(&text);
        press(&mut viewer, "gsgs");
        press(&mut viewer, ":jump<CR>");
        let target = text.find("w3x7").unwrap();
        let (name, _) = up(&viewer)
            .into_iter()
            .find(|(_, at)| *at == target)
            .unwrap();
        // A letter narrows the labels to those it begins.
        let first = &name[..1];
        press(&mut viewer, first);
        let left = up(&viewer);
        assert!(viewer.is_jumping() || name.len() == 1);
        assert!(
            left.iter().all(|(other, _)| other.starts_with(first)),
            "{left:?}"
        );
        press(&mut viewer, &name[1..]);
        assert!(!viewer.is_jumping());
        assert_eq!(cursor(&viewer), target);
        // A jump: `` ` ` `` leads back.
        press(&mut viewer, "``");
        assert_eq!(cursor(&viewer), 2);
        // Keys are the text's again.
        press(&mut viewer, "w");
        assert_eq!(cursor(&viewer), text.find("First").unwrap());
    }

    #[test]
    fn a_label_extends_the_selection_and_a_wrong_one_stays() {
        let text = words();
        let (mut viewer, _) = editing(&text);
        press(&mut viewer, "gsgs");
        press(&mut viewer, "/First<CR>vgw");
        assert!(viewer.is_jumping());
        let target = text.find("and").unwrap();
        let (name, _) = up(&viewer)
            .into_iter()
            .find(|(_, at)| *at == target)
            .unwrap();
        press(&mut viewer, &name);
        let editor = viewer.editor.as_ref().unwrap();
        let selected = editor.selection().unwrap();
        assert_eq!(selected.start, text.find("First").unwrap());
        assert_eq!(selected.end, target + 1);
        // With an operator pending there are no labels; nor once a key
        // that is no label's is typed.
        press(&mut viewer, "<Esc>dgw");
        assert!(!viewer.is_jumping());
        press(&mut viewer, "<Esc>gw;");
        assert!(!viewer.is_jumping());
        assert_eq!(viewer.text(), Some(text.as_str()));
    }

    #[test]
    fn labels_follow_the_view() {
        let text = words();
        let (mut viewer, _) = editing(&text);
        press(&mut viewer, "gsgs");
        press(&mut viewer, "G");
        compile(&mut viewer);
        press(&mut viewer, "gw");
        let target = text.find("FarAway").unwrap();
        let labels = up(&viewer);
        if labels.is_empty() {
            // The only place in view: the cursor went there at once.
            assert_eq!(cursor(&viewer), target);
        } else {
            assert!(labels.iter().any(|(_, at)| *at == target), "{labels:?}");
            assert!(!labels.iter().any(|(_, at)| *at == 2));
        }
    }
}
