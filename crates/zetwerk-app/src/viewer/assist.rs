//! Help with writing: what could be typed here (a list to pick from
//! while typing), what the thing under the cursor is (`K`), where it is
//! defined (`gd`), and a list of the document's headings or labels to go
//! to (`go`).
//!
//! What Typst knows comes from its own tooling (`zetwerk_typst::complete`
//! and its neighbours); this is the keys and the lists.

use zetwerk_typst::Candidate;

use super::{Scene, TextureId, Viewer, chrome};
use crate::editor;
use crate::keys::{Key, KeyPress};

/// How many rows of a list are shown at once.
const LIST_ROWS: usize = 9;
/// How many lines of an explanation are shown, and how long a line.
const TOOLTIP_LINES: usize = 14;
const TOOLTIP_COLUMNS: usize = 84;

/// The list of what could be typed at the cursor.
pub(super) struct Completion {
    /// The byte from which on a candidate replaces what was typed.
    from: usize,
    pub(super) candidates: Vec<Candidate>,
    chosen: usize,
}

/// A list of places to go, narrowed by typing.
pub(super) struct Picker {
    title: &'static str,
    /// What each is listed as and the byte it is at.
    items: Vec<(String, usize)>,
    query: String,
    /// The items the query leaves, best first.
    shown: Vec<usize>,
    chosen: usize,
}

/// How well `query` is found in `text`, all its characters in their
/// order: more for characters that follow each other and that begin
/// words. `None` if it is not found.
fn score(text: &str, query: &str) -> Option<i32> {
    let mut wanted = query.chars().flat_map(char::to_lowercase).peekable();
    let (mut score, mut run, mut before) = (0, 0, None::<char>);
    for c in text.chars() {
        let Some(next) = wanted.peek() else {
            break;
        };
        if c.to_lowercase().eq(std::iter::once(*next)) {
            wanted.next();
            run += 1;
            let begins = before.is_none_or(|before| !before.is_alphanumeric());
            score += run * 2 + if begins { 6 } else { 0 };
        } else {
            run = 0;
            score -= 1;
        }
        before = Some(c);
    }
    wanted.peek().is_none().then_some(score)
}

/// A line of `text` cut to `columns` characters.
fn cut(text: &str, columns: usize) -> String {
    let mut shown: String = text.chars().take(columns).collect();
    if shown.len() < text.len() {
        shown.push('…');
    }
    shown
}

impl Viewer {
    /// A key that may be for a list on screen. Returns whether it was.
    pub(super) fn assist_key(&mut self, press: KeyPress) -> bool {
        self.tooltip = None;
        if self.picker.is_some() {
            self.picker_key(press);
            return true;
        }
        let typing = self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.mode() == editor::Mode::Insert);
        if !typing {
            self.completion = None;
            return false;
        }
        let next = matches!(
            (press.key, press.ctrl),
            (Key::Char('n'), true) | (Key::Down, _)
        );
        let previous = matches!(
            (press.key, press.ctrl),
            (Key::Char('p'), true) | (Key::Up, _)
        );
        let take = matches!(
            (press.key, press.ctrl),
            (Key::Tab, _) | (Key::Char('y'), true)
        );
        let asked = matches!((press.key, press.ctrl), (Key::Char('n' | ' '), true));
        let Some(completion) = &mut self.completion else {
            if asked {
                self.complete(true);
                if self.completion.is_none() {
                    self.notice("nothing to complete here");
                }
            }
            return asked;
        };
        let count = completion.candidates.len();
        if next {
            completion.chosen = (completion.chosen + 1) % count;
        } else if previous {
            completion.chosen = (completion.chosen + count - 1) % count;
        } else if take {
            self.take_completion();
        } else if matches!((press.key, press.ctrl), (Key::Char('e'), true)) {
            self.completion = None;
        } else {
            // Any other key is typed, and the list follows the typing.
            return false;
        }
        true
    }

    /// Brings the list of completions up to date with the text, after a
    /// key was typed: Typst is asked where a list is likely to be wanted
    /// (behind `#` and `@`), or wherever the user asked (`explicit`).
    pub(super) fn complete(&mut self, explicit: bool) {
        let was_open = self.completion.take().is_some();
        let Some(editor) = &self.editor else {
            return;
        };
        if editor.mode() != editor::Mode::Insert {
            return;
        }
        let (text, cursor) = (editor.text(), editor.cursor());
        // The word being typed, and what stands before it.
        let begun = text[..cursor]
            .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_' || c == '-')
            .len();
        let trigger = text[..begun].chars().next_back();
        if !(explicit || was_open || matches!(trigger, Some('#' | '@'))) {
            return;
        }
        let source = editor.source().clone();
        self.project.set_source(source.clone());
        let world = self.project.snapshot();
        let document = self.document.as_deref();
        let Some((from, mut candidates)) =
            zetwerk_typst::complete(&world, document, &source, cursor, explicit)
        else {
            return;
        };
        let Some(typed) = source.text().get(from..cursor) else {
            return;
        };
        // What begins with what was typed comes first, then what has it
        // in it; each label once.
        let mut ranked: Vec<(i32, Candidate)> = Vec::new();
        candidates.dedup_by(|a, b| a.label == b.label);
        for candidate in candidates {
            let label = candidate.label.trim_start_matches(['#', '@', '<']);
            let begins = label
                .to_lowercase()
                .starts_with(&typed.to_lowercase())
                .then_some(1000);
            if let Some(score) = begins.or_else(|| score(label, typed)) {
                ranked.push((score, candidate));
            }
        }
        ranked.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        if ranked.is_empty() {
            return;
        }
        self.completion = Some(Completion {
            from,
            candidates: ranked.into_iter().map(|(_, candidate)| candidate).collect(),
            chosen: 0,
        });
    }

    /// Puts the chosen completion into the text, as part of what is
    /// being typed.
    fn take_completion(&mut self) {
        let Some(completion) = self.completion.take() else {
            return;
        };
        let (Some(candidate), Some(editor)) = (
            completion.candidates.get(completion.chosen),
            &mut self.editor,
        ) else {
            return;
        };
        let typed = editor
            .text()
            .get(completion.from..editor.cursor())
            .map_or(0, |typed| typed.chars().count());
        let press = editor.completed(typed, candidate.text.clone(), candidate.cursor);
        self.editor_key(press);
    }

    /// `K`: what Typst says the thing under the cursor is.
    pub(super) fn describe(&mut self) {
        let Some(editor) = &self.editor else {
            return;
        };
        let (source, cursor) = (editor.source().clone(), editor.cursor());
        self.project.set_source(source.clone());
        let world = self.project.snapshot();
        let document = self.document.as_deref();
        match zetwerk_typst::describe_at(&world, document, &source, cursor) {
            Some(text) => self.tooltip = Some(text),
            None => self.notice("Typst has nothing to say about this"),
        }
    }

    /// `gd`: to where the name under the cursor is defined.
    pub(super) fn goto_definition(&mut self) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let (source, cursor) = (editor.source().clone(), editor.cursor());
        self.project.set_source(source.clone());
        let world = self.project.snapshot();
        let document = self.document.as_deref();
        match zetwerk_typst::definition_of(&world, document, &source, cursor) {
            Some((file, range)) if file == self.project.main() => {
                editor.jump(range.start);
                self.follow_cursor();
            }
            Some((file, _)) => {
                let path = file.vpath().get_without_slash().to_owned();
                self.notice(format!("defined in {path}, which is not open"));
            }
            None => self.notice("not defined in a file: built in, or no name"),
        }
    }

    /// Opens the list of the document's headings (`go`), or of its
    /// labels.
    pub(super) fn pick(&mut self, labels: bool) {
        let Ok(outline) = self.outline() else {
            return;
        };
        let Some(text) = self.text() else {
            return;
        };
        let start_of = |line: usize| {
            text.split_inclusive('\n')
                .take(line.saturating_sub(1))
                .map(str::len)
                .sum()
        };
        let items: Vec<(String, usize)> = if labels {
            let listed = |(name, line): &(String, usize)| (format!("<{name}>"), start_of(*line));
            outline.labels.iter().map(listed).collect()
        } else {
            let listed = |heading: &super::agent::Heading| {
                let indent = "  ".repeat(heading.level.saturating_sub(1));
                (format!("{indent}{}", heading.text), start_of(heading.line))
            };
            outline.headings.iter().map(listed).collect()
        };
        if items.is_empty() {
            let what = if labels { "labels" } else { "headings" };
            self.notice(format!("the document has no {what}"));
            return;
        }
        // The one the cursor is in or behind comes up chosen.
        let cursor = self
            .editor
            .as_ref()
            .map_or(0, crate::editor::Editor::cursor);
        let chosen = items.iter().rposition(|(_, at)| *at <= cursor).unwrap_or(0);
        self.picker = Some(Picker {
            title: if labels { "labels" } else { "headings" },
            shown: (0..items.len()).collect(),
            items,
            query: String::new(),
            chosen,
        });
    }

    fn picker_key(&mut self, press: KeyPress) {
        let Some(picker) = &mut self.picker else {
            return;
        };
        let count = picker.shown.len().max(1);
        match (press.key, press.ctrl) {
            (Key::Escape, _) | (Key::Char('c' | '['), true) => self.picker = None,
            (Key::Enter, _) => {
                let target = picker
                    .shown
                    .get(picker.chosen)
                    .map(|item| picker.items[*item].1);
                self.picker = None;
                if let (Some(target), Some(editor)) = (target, &mut self.editor) {
                    editor.jump(target);
                    self.follow_cursor();
                }
            }
            (Key::Down, _) | (Key::Char('n'), true) => {
                picker.chosen = (picker.chosen + 1) % count;
            }
            (Key::Up, _) | (Key::Char('p'), true) => {
                picker.chosen = (picker.chosen + count - 1) % count;
            }
            (Key::Backspace, _) => {
                picker.query.pop();
                picker.narrow();
            }
            (Key::Char(c), false) => {
                picker.query.push(c);
                picker.narrow();
            }
            _ => {}
        }
    }

    /// The lists and explanations on screen, over everything else.
    pub(super) fn assist_panels(&mut self, scene: &mut Scene, has: &dyn Fn(TextureId) -> bool) {
        let (width, height) = (f64::from(self.window.0), f64::from(self.window.1));
        let margin = super::PANEL_MARGIN * self.scale;
        let bottom = height - f64::from(self.status_height()) - margin;
        // Under the cursor's line; above it where there is no room.
        let anchor = self.caret_at.or(self.cursor_at);
        let near = move |size: (f64, f64)| {
            let within = (margin, margin, width - margin, bottom);
            super::lens_place(anchor, size, within, 2.0)
        };
        if let Some(completion) = &self.completion {
            let first = completion
                .chosen
                .saturating_sub(LIST_ROWS / 2)
                .min(completion.candidates.len().saturating_sub(LIST_ROWS));
            let rows: Vec<(String, String)> = completion
                .candidates
                .iter()
                .skip(first)
                .take(LIST_ROWS)
                .map(|candidate| (cut(&candidate.label, 40), candidate.kind.to_owned()))
                .collect();
            let detail = completion
                .candidates
                .get(completion.chosen)
                .and_then(|candidate| candidate.detail.as_deref())
                .map(|detail| cut(detail, 70));
            let markup =
                chrome::list_markup(None, &rows, completion.chosen - first, detail.as_deref());
            self.panel(scene, has, &markup, near);
        }
        if let Some(text) = &self.tooltip {
            let mut lines: Vec<String> = text
                .lines()
                .take(TOOLTIP_LINES)
                .map(|line| cut(line, TOOLTIP_COLUMNS))
                .collect();
            if text.lines().count() > lines.len() {
                lines.push("…".to_owned());
            }
            let markup = chrome::text_markup(&lines);
            self.panel(scene, has, &markup, near);
        }
        if let Some(picker) = &self.picker {
            let first = picker
                .chosen
                .saturating_sub(LIST_ROWS / 2)
                .min(picker.shown.len().saturating_sub(LIST_ROWS));
            let rows: Vec<(String, String)> = picker
                .shown
                .iter()
                .skip(first)
                .take(LIST_ROWS)
                .map(|item| (cut(&picker.items[*item].0, 70), String::new()))
                .collect();
            let title = format!("{}: {}\u{2588}", picker.title, picker.query);
            let chosen = picker.chosen.saturating_sub(first);
            let markup = chrome::list_markup(Some(&title), &rows, chosen, None);
            // In the middle, a third of the way down.
            self.panel(scene, has, &markup, |(panel_width, _)| {
                ((width - panel_width) / 2.0, (height / 4.0).max(margin))
            });
        }
    }

    /// Whether keys are for a list of places and not for the text.
    #[must_use]
    pub fn is_picking(&self) -> bool {
        self.picker.is_some()
    }
}

impl Picker {
    /// Narrows the list to what the query is found in, best first.
    fn narrow(&mut self) {
        let mut ranked: Vec<(i32, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, (label, _))| Some((score(label, &self.query)?, index)))
            .collect();
        ranked.sort_by_key(|(score, index)| (std::cmp::Reverse(*score), *index));
        self.shown = ranked.into_iter().map(|(_, index)| index).collect();
        self.chosen = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_is_found_in_order_and_ranked() {
        assert!(score("Introduction", "intro").is_some());
        assert!(score("Introduction", "itn").is_some());
        assert_eq!(score("Introduction", "x"), None);
        assert_eq!(score("Introduction", "ooo"), None);
        assert_eq!(score("Proof", "fp"), None);
        // The start of a word and letters in a row count for more.
        assert!(score("Main result", "res") > score("Preserved", "res"));
        assert!(score("Proof", "pro") > score("Approximation", "pro"));
        assert!(score("anything", "").is_some());
    }
}
