//! The user's own keys: a sequence of keys that stands for another.
//!
//! As in Vim's `nnoremap` and `inoremap`: what a mapping stands for is
//! typed as it is, without being looked up again. A mapping may be
//! several keys long. While what was typed could still become one, it
//! waits for the next key, for as long as `timeoutlen` (a second, unless
//! the settings say otherwise); then it is taken as it is.
//!
//! In normal and visual mode the keys that wait do nothing yet. In insert
//! mode they are typed at once, as Vim does it with `inoremap jk <Esc>`:
//! typing must show as it is typed. The text, its history and the engine
//! are marked before the first of them ([`crate::editor::Mark`]); when
//! the mapping is complete, all that they did is taken back, so that it
//! is no undo step and not what `.` repeats. A pause, or a key that makes
//! no mapping of them, leaves them typed.

use std::time::{Duration, Instant};

use super::{Edited, Viewer};
use crate::editor;
use crate::keys::{self, KeyPress};

/// How long keys that may become a mapping wait for the next, unless the
/// settings say otherwise: Vim's `timeoutlen`.
pub const TIMEOUT: Duration = Duration::from_millis(1000);

/// What keys stand for, in normal (and visual) mode and in insert mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap {
    normal: Vec<(Vec<KeyPress>, Vec<KeyPress>)>,
    insert: Vec<(Vec<KeyPress>, Vec<KeyPress>)>,
    /// How long the keys of a mapping begun wait for the next.
    timeout: Duration,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            normal: Vec::new(),
            insert: Vec::new(),
            timeout: TIMEOUT,
        }
    }
}

impl Keymap {
    /// Reads mappings written in Vim's notation (`<C-s>`, `<CR>`), each
    /// as what is typed and what it stands for. Returns what could be
    /// read, and a complaint about the first that could not.
    #[must_use]
    pub fn parse<'a>(
        normal: impl IntoIterator<Item = (&'a str, &'a str)>,
        insert: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> (Self, Option<String>) {
        let mut complaint = None;
        let mut read = |pairs: &mut dyn Iterator<Item = (&'a str, &'a str)>| {
            let mut table = Vec::new();
            for (typed, stands_for) in pairs {
                let parsed = keys::parse(typed).and_then(|typed| {
                    let stands_for = keys::parse(stands_for)?;
                    if typed.is_empty() {
                        return Err("no key".to_owned());
                    }
                    Ok((typed, stands_for))
                });
                match parsed {
                    Ok(mapping) => table.push(mapping),
                    Err(why) => {
                        complaint
                            .get_or_insert(format!("the mapping of {typed} is ignored: {why}"));
                    }
                }
            }
            table
        };
        let normal = read(&mut normal.into_iter());
        let insert = read(&mut insert.into_iter());
        let keymap = Self {
            normal,
            insert,
            timeout: TIMEOUT,
        };
        (keymap, complaint)
    }

    /// The same mappings, whose keys begun wait `millis` milliseconds for
    /// the next (Vim's `timeoutlen`).
    #[must_use]
    pub fn with_timeout(mut self, millis: u64) -> Self {
        self.timeout = Duration::from_millis(millis);
        self
    }

    fn is_empty(&self) -> bool {
        self.normal.is_empty() && self.insert.is_empty()
    }

    /// The mappings of normal mode that begin with `typed`: what is
    /// typed for each, and what it stands for.
    pub(super) fn begun<'a>(
        &'a self,
        typed: &'a [KeyPress],
    ) -> impl Iterator<Item = (&'a [KeyPress], &'a [KeyPress])> {
        self.normal
            .iter()
            .filter(move |(keys, _)| keys.starts_with(typed))
            .map(|(keys, stands_for)| (keys.as_slice(), stands_for.as_slice()))
    }
}

/// Keys typed that wait for more to become a mapping.
#[derive(Default)]
pub(super) struct Waiting {
    /// When they are taken as they are.
    until: Option<Instant>,
    /// In insert mode, where they are typed already: the editor before
    /// each of them, and what was true of it after the last (its file,
    /// revision and cursor). Anything else that changed it since leaves
    /// them typed.
    marks: Vec<editor::Mark>,
    after: Option<(typst::syntax::FileId, u64, usize)>,
}

/// What a key means once the mappings have been looked at.
enum Mapped {
    /// These keys, as they are.
    Keys(Vec<KeyPress>),
    /// Nothing yet: more keys may make a mapping of it.
    Wait,
}

impl Viewer {
    /// Sets the user's mappings.
    pub fn set_keymap(&mut self, keymap: Keymap) {
        self.keymap = keymap;
        self.typed.clear();
        self.mapping = Waiting::default();
    }

    /// The keys that `press` stands for, given the mappings and what was
    /// typed before it.
    fn mapped(&mut self, press: KeyPress) -> Mapped {
        let Some(editor) = &self.editor else {
            return Mapped::Keys(vec![press]);
        };
        let typing = match editor.mode() {
            editor::Mode::Insert => true,
            editor::Mode::Command => return Mapped::Keys(vec![press]),
            _ => false,
        };
        if typing {
            return self.mapped_typing(press);
        }
        let table = &self.keymap.normal;
        // In the middle of a command a key is part of it: the `;` of
        // `f;` is a character to find. Not behind a count or the name
        // of a register: `3Y` is the mapping of `Y`, three times.
        if self.typed.is_empty() && editor.awaits_part() {
            return Mapped::Keys(vec![press]);
        }
        self.typed.push(press);
        let begun = || {
            table
                .iter()
                .filter(|(typed, _)| typed.starts_with(&self.typed))
        };
        if let Some((_, stands_for)) = begun().find(|(typed, _)| *typed == self.typed) {
            let keys = stands_for.clone();
            self.typed.clear();
            self.mapping.until = None;
            return Mapped::Keys(keys);
        }
        if begun().next().is_some() {
            self.mapping.until = Some(Instant::now() + self.keymap.timeout);
            return Mapped::Wait;
        }
        // No mapping after all: what was typed is what was meant.
        self.mapping.until = None;
        Mapped::Keys(std::mem::take(&mut self.typed))
    }

    /// [`Viewer::mapped`] while typing: the key is typed at once, and the
    /// keys typed before it that were waiting are taken back if it makes
    /// a mapping of them. Where it makes none, the first of them stays
    /// typed and the rest may still make one, as in Vim.
    fn mapped_typing(&mut self, press: KeyPress) -> Mapped {
        let Some(editor) = &self.editor else {
            return Mapped::Keys(vec![press]);
        };
        // What was typed is still what it was typed into, as it was left.
        let now = (editor.file(), editor.revision(), editor.cursor());
        if self.mapping.after != Some(now) {
            self.typed.clear();
            self.mapping = Waiting::default();
        }
        self.typed.push(press);
        let table = &self.keymap.insert;
        loop {
            let complete = table.iter().find(|(typed, _)| *typed == self.typed);
            if let Some((_, stands_for)) = complete {
                let keys = stands_for.clone();
                self.typed.clear();
                let mark = std::mem::take(&mut self.mapping).marks.into_iter().next();
                if let (Some(mark), Some(editor)) = (mark, &mut self.editor)
                    && editor.take_back(mark)
                {
                    self.text_changed(Edited::Typing);
                    self.completion = None;
                }
                return Mapped::Keys(keys);
            }
            let longer = table
                .iter()
                .any(|(typed, _)| typed.starts_with(&self.typed));
            if longer {
                // Typed now; marked before, and what it was typed into
                // noted after.
                self.mapping.marks.push(editor.mark());
                self.mapping.until = Some(Instant::now() + self.keymap.timeout);
                return Mapped::Keys(vec![press]);
            }
            // No mapping of these: the first stays typed, and those
            // after it may begin one.
            self.typed.remove(0);
            if self.mapping.marks.is_empty() {
                self.mapping = Waiting::default();
                return Mapped::Keys(vec![press]);
            }
            self.mapping.marks.remove(0);
        }
    }

    /// A key was pressed: it, or what the user's mappings make of it.
    pub fn key(&mut self, press: KeyPress) {
        if self.report_key(press) {
            return;
        }
        let plain = self.keymap.is_empty()
            || self.is_picking()
            || self.is_reviewing()
            || self.is_grid()
            || self.is_presenting()
            || self.is_jumping()
            || self.is_retyping()
            || self.is_setting();
        if plain {
            self.plain_key(press);
        } else {
            match self.mapped(press) {
                Mapped::Wait => self.message = None,
                Mapped::Keys(keys) => {
                    for key in keys {
                        self.plain_key(key);
                    }
                }
            }
            self.note_typed();
        }
        self.hints_keyed();
        self.report_keyed();
        self.tutor_keyed();
    }

    /// Notes what keys typed while a mapping may be begun were typed into,
    /// once they are.
    fn note_typed(&mut self) {
        let typing = self
            .editor
            .as_ref()
            .filter(|editor| editor.mode() == editor::Mode::Insert);
        match typing {
            Some(editor) if !self.mapping.marks.is_empty() && !self.typed.is_empty() => {
                self.mapping.after = Some((editor.file(), editor.revision(), editor.cursor()));
            }
            // Out of typing by them: nothing to take back any more.
            _ if !self.mapping.marks.is_empty() => {
                self.typed.clear();
                self.mapping = Waiting::default();
            }
            _ => {}
        }
    }

    /// When the keys typed of a mapping are to be taken as they are, if
    /// any wait: the one time the mappings want the window woken up at.
    #[must_use]
    pub fn keys_at(&self) -> Option<Instant> {
        self.mapping.until.filter(|_| !self.typed.is_empty())
    }

    /// Takes the keys typed of a mapping as they are, where they have
    /// waited long enough by `now`: those that waited untyped do now
    /// what they do by themselves; those typed already stay so. Returns
    /// whether that changed anything to be drawn.
    pub fn keys_due(&mut self, now: Instant) -> bool {
        if self.keys_at().is_none_or(|due| due > now) {
            return false;
        }
        self.mapping = Waiting::default();
        let typing = self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.mode() == editor::Mode::Insert);
        let keys = std::mem::take(&mut self.typed);
        if typing {
            return false;
        }
        for key in keys {
            self.plain_key(key);
        }
        self.hints_keyed();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mappings_are_read_and_checked() {
        let (keymap, complaint) = Keymap::parse(
            [(";", ":"), ("<C-s>", ":w<CR>")],
            [("<C-s>", "<Esc>:w<CR>"), ("jk", "<Esc>")],
        );
        assert_eq!(complaint, None);
        assert_eq!((keymap.normal.len(), keymap.insert.len()), (2, 2));
        assert_eq!(keymap.normal[1].1.len(), 3);
        assert_eq!(keymap.timeout, TIMEOUT);
        assert_eq!(keymap.with_timeout(300).timeout, Duration::from_millis(300));
        // What cannot be a mapping is said and left out.
        let (keymap, complaint) = Keymap::parse([("<Nope>", "x")], [("", "<Esc>")]);
        assert!(keymap.is_empty());
        assert!(complaint.is_some_and(|why| why.contains("<Nope>")));
        let (_, complaint) = Keymap::parse([], [("", "<Esc>")]);
        assert!(complaint.is_some_and(|why| why.contains("no key")));
    }
}
