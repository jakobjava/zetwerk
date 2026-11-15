//! Vim's keys and commands that are the window's to carry out, where
//! the engine only says what was asked: Shift-Enter on a page; `&` and
//! `:&&`; `:s` asking about each match (`c`); `q:` and `q/`; the lists
//! of `:registers`, `:marks`, `:jumps` and `:changes`; `:earlier` and
//! `:later`.

use std::ops::Range;

use super::assist::Target;
use super::{Panes, Viewer};
use crate::editor;
use crate::keys::{Key, KeyPress};

/// What these keep between keys.
#[derive(Default)]
pub(super) struct Vim {
    /// A `:s` with `c` that asks about each match.
    confirm: Option<Confirm>,
    /// A block of cells of a table on a page (see `cells`).
    pub(super) cells: super::cells::Cells,
}

/// The matches of a `:s` with `c`, with what replaces each; which is
/// asked about; those to be replaced; and the revision of the text they
/// are of.
struct Confirm {
    edits: Vec<(Range<usize>, String)>,
    at: usize,
    chosen: Vec<(Range<usize>, String)>,
    revision: u64,
}

/// How much of a register or a line a list shows.
const SHOWN: usize = 60;

impl Viewer {
    /// Shift-Enter while typing on a page: a line break that goes on in
    /// the same paragraph, as a word processor has it (Typst's `\`). It
    /// is typed as a text put in whole, so that it is part of what is
    /// typed for undo and `.`. Elsewhere it is Enter (see the engine).
    pub(super) fn shift_enter(&mut self, press: KeyPress) -> KeyPress {
        if press.key != Key::ShiftEnter || press.ctrl || press.alt {
            return press;
        }
        let on_page = self.shown_panes() == Panes::Page;
        let Some(editor) = self
            .editor
            .as_mut()
            .filter(|editor| on_page && editor.mode() == editor::Mode::Insert)
        else {
            return press;
        };
        match crate::typst_text::line_break_within(editor.source(), editor.cursor()) {
            Some(text) => {
                let within = text.len();
                editor.completed(0, text, within)
            }
            None => press,
        }
    }

    /// The commands of this module; whether `command` was one.
    pub(super) fn vim_command(&mut self, command: &str) -> bool {
        let (name, argument) = command.split_once(' ').unwrap_or((command, ""));
        let argument = argument.trim();
        match (name, argument) {
            ("&" | "s" | "substitute", "") => self.substitute_again(false, false),
            ("&&", "") => self.substitute_again(false, true),
            ("%&" | "%s", "") => self.substitute_again(true, false),
            ("%&&", "") => self.substitute_again(true, true),
            ("q:", "") => self.list_command_lines(':'),
            ("q/", "") => self.list_command_lines('/'),
            ("reg" | "registers" | "di" | "display", "") => self.list_registers(),
            ("marks", "") => self.list_places("marks"),
            ("ju" | "jumps", "") => self.list_places("jumps"),
            ("changes", "") => self.list_places("changes"),
            ("ea" | "earlier", steps) => self.step_in_time(steps, true),
            ("lat" | "later", steps) => self.step_in_time(steps, false),
            _ if command.starts_with(['/', '?']) && command.len() > 1 => {
                // A search chosen from `q/`: made again, as typed.
                for c in command.chars() {
                    self.editor_key(KeyPress::plain(Key::Char(c)));
                }
                self.editor_key(KeyPress::plain(Key::Enter));
            }
            _ => return false,
        }
        true
    }

    /// `q:` and `q/`: the command lines entered (or the searches), the
    /// newest first; one chosen is put on the command line to be run
    /// again, or changed first.
    fn list_command_lines(&mut self, prompt: char) {
        let Some(editor) = &self.editor else {
            return;
        };
        let items: Vec<(String, Target)> = editor
            .engine()
            .command_history(prompt)
            .into_iter()
            .rev()
            .map(|line| {
                let target = if prompt == ':' {
                    Target::Ask(line.to_owned())
                } else {
                    Target::Command(format!("/{line}"))
                };
                (format!("{prompt}{line}"), target)
            })
            .collect();
        let title = if prompt == ':' {
            "command lines entered"
        } else {
            "searches made"
        };
        if items.is_empty() {
            return self.notice(format!("no {title} yet"));
        }
        self.open_picker(title, items, 0);
    }

    /// `:registers`: what each register holds.
    fn list_registers(&mut self) {
        let Some(editor) = &self.editor else {
            return;
        };
        let items: Vec<(String, Target)> = editor
            .engine()
            .registers()
            .into_iter()
            .filter(|(_, text, _)| !text.is_empty())
            .map(|(name, text, linewise)| {
                let kind = if linewise { "lines" } else { "" };
                let row = format!("\"{name}  {}\t{kind}", shortened(text));
                (row, Target::Nothing)
            })
            .collect();
        if items.is_empty() {
            return self.notice("the registers hold nothing yet");
        }
        self.open_picker("registers", items, 0);
    }

    /// `:marks`, `:jumps` and `:changes`: the places, each with its line;
    /// one chosen is gone to.
    fn list_places(&mut self, what: &'static str) {
        let Some(editor) = &self.editor else {
            return;
        };
        let engine = editor.engine();
        let named: Vec<(String, usize)> = match what {
            "marks" => engine
                .marks()
                .into_iter()
                .map(|(name, at)| (format!("'{name}"), at))
                .collect(),
            "jumps" => engine
                .jumps()
                .iter()
                .rev()
                .enumerate()
                .map(|(back, at)| (format!("{}", back + 1), *at))
                .collect(),
            _ => engine
                .changes()
                .iter()
                .rev()
                .enumerate()
                .map(|(back, at)| (format!("{}", back + 1), *at))
                .collect(),
        };
        let text = editor.text();
        let items: Vec<(String, Target)> = named
            .into_iter()
            .filter(|(_, at)| *at <= text.len())
            .map(|(name, at)| {
                let line = text[..at].matches('\n').count() + 1;
                let start = text[..at].rfind('\n').map_or(0, |found| found + 1);
                let end = text[at..].find('\n').map_or(text.len(), |found| at + found);
                let row = format!("{name}  {}\tline {line}", shortened(&text[start..end]));
                (row, Target::Offset(at))
            })
            .collect();
        if items.is_empty() {
            return self.notice(format!("no {what} yet"));
        }
        self.open_picker(what, items, 0);
    }

    /// `:earlier {n}` and `:later {n}`: to the state of the text made `n`
    /// steps before this one (or after), as Vim counts them: in the order
    /// they were made, across the branches of the history.
    fn step_in_time(&mut self, steps: &str, back: bool) {
        let steps = if steps.is_empty() { "1" } else { steps };
        let Ok(steps) = steps.parse::<u64>() else {
            return self.alert(
                "steps of the history, please: :earlier 3 (times are not kept here; gh lists them)",
            );
        };
        let Some(editor) = &self.editor else {
            return;
        };
        let history = editor.history();
        let current = history.current();
        let newest = u64::try_from(history.len())
            .unwrap_or(u64::MAX)
            .saturating_sub(1);
        let to = if back {
            current.saturating_sub(steps)
        } else {
            current.saturating_add(steps).min(newest)
        };
        if to == current {
            let end = if back { "oldest" } else { "newest" };
            return self.notice(format!("at the {end} state of the text"));
        }
        self.execute(&format!("history go {to}"));
    }

    /// `:s` with `c`: asks about each of `edits` in turn, the match shown
    /// selected; y makes it, n leaves it, a makes it and all after it, l
    /// makes it and stops, q and Escape stop. What is made is made at the
    /// end, as one undo step.
    pub(super) fn confirm_substitute(&mut self, edits: Vec<(Range<usize>, String)>) {
        let Some(editor) = &self.editor else {
            return;
        };
        self.vim.confirm = Some(Confirm {
            edits,
            at: 0,
            chosen: Vec::new(),
            revision: editor.revision(),
        });
        self.ask_about_match();
    }

    /// Shows the match asked about, and asks.
    fn ask_about_match(&mut self) {
        let (Some(confirm), Some(editor)) = (&self.vim.confirm, &mut self.editor) else {
            return;
        };
        let Some((range, with)) = confirm.edits.get(confirm.at) else {
            return self.end_confirm();
        };
        let text = editor.text();
        let last = text[..range.end.max(range.start + 1).min(text.len())]
            .char_indices()
            .next_back()
            .map_or(range.start, |(at, _)| at.max(range.start));
        let source = editor::Shown::Source {
            columns: self.source_size.0,
        };
        editor.select(range.start, last, false, &source);
        let left = confirm.edits.len() - confirm.at;
        let message = format!(
            "replace with \u{201c}{with}\u{201d}? y yes, n no, a all {left}, l this and stop, q stop"
        );
        self.follow_cursor();
        self.notice(message);
    }

    /// A key while `:s` asks about a match; whether it was taken.
    pub(super) fn confirm_key(&mut self, press: KeyPress) -> bool {
        let Some(confirm) = &mut self.vim.confirm else {
            return false;
        };
        let changed = self
            .editor
            .as_ref()
            .is_none_or(|editor| editor.revision() != confirm.revision);
        if changed {
            self.vim.confirm = None;
            return false;
        }
        let this = confirm.edits.get(confirm.at).cloned();
        match (press.key, press.ctrl) {
            (Key::Char('y'), false) => {
                confirm.chosen.extend(this);
                confirm.at += 1;
            }
            (Key::Char('n'), false) => confirm.at += 1,
            (Key::Char('a'), false) => {
                let rest = confirm.edits.split_off(confirm.at);
                confirm.chosen.extend(rest);
                confirm.at = confirm.edits.len();
            }
            (Key::Char('l'), false) => {
                confirm.chosen.extend(this);
                confirm.at = confirm.edits.len();
            }
            (Key::Char('q'), false) | (Key::Escape, _) | (Key::Char('['), true) => {
                confirm.at = confirm.edits.len();
            }
            // Nothing else: the question stands.
            _ => {}
        }
        self.ask_about_match();
        true
    }

    /// The questions are over: what was chosen is made.
    fn end_confirm(&mut self) {
        let Some(confirm) = self.vim.confirm.take() else {
            return;
        };
        if let Some(editor) = &mut self.editor {
            let at = confirm
                .chosen
                .last()
                .map_or(editor.cursor(), |(range, _)| range.start);
            editor.point(at);
        }
        if confirm.chosen.is_empty() {
            return self.notice("nothing replaced");
        }
        self.make_substitute(&confirm.chosen);
    }
}

/// `text` in one line, cut short where it is long.
fn shortened(text: &str) -> String {
    let one_line = text.replace('\n', "\u{21b5}");
    if one_line.chars().count() <= SHOWN {
        return one_line;
    }
    let cut: String = one_line.chars().take(SHOWN).collect();
    format!("{cut}\u{2026}")
}
