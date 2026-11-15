//! The user's own keys: a sequence of keys that stands for another.
//!
//! As in Vim's `nnoremap` and `inoremap`: what a mapping stands for is
//! typed as it is, without being looked up again. In normal and visual
//! mode a mapping may be several keys long and is waited for while what
//! was typed could still become it; in insert mode it is one key, since
//! what is typed there has to show at once.

use super::Viewer;
use crate::editor;
use crate::keys::{self, KeyPress};

/// What keys stand for, in normal (and visual) mode and in insert mode.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Keymap {
    normal: Vec<(Vec<KeyPress>, Vec<KeyPress>)>,
    insert: Vec<(Vec<KeyPress>, Vec<KeyPress>)>,
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
        let mut read = |pairs: &mut dyn Iterator<Item = (&'a str, &'a str)>, one_key: bool| {
            let mut table = Vec::new();
            for (typed, stands_for) in pairs {
                let parsed = keys::parse(typed).and_then(|typed| {
                    let stands_for = keys::parse(stands_for)?;
                    match typed.len() {
                        0 => Err("no key".to_owned()),
                        1 => Ok((typed, stands_for)),
                        _ if one_key => Err("in insert mode one key only".to_owned()),
                        _ => Ok((typed, stands_for)),
                    }
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
        let normal = read(&mut normal.into_iter(), false);
        let insert = read(&mut insert.into_iter(), true);
        (Self { normal, insert }, complaint)
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
    }

    /// The keys that `press` stands for, given the mappings and what was
    /// typed before it.
    fn mapped(&mut self, press: KeyPress) -> Mapped {
        let Some(editor) = &self.editor else {
            return Mapped::Keys(vec![press]);
        };
        let table = match editor.mode() {
            editor::Mode::Insert => &self.keymap.insert,
            editor::Mode::Command => return Mapped::Keys(vec![press]),
            _ => &self.keymap.normal,
        };
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
            return Mapped::Keys(keys);
        }
        if begun().next().is_some() {
            return Mapped::Wait;
        }
        // No mapping after all: what was typed is what was meant.
        Mapped::Keys(std::mem::take(&mut self.typed))
    }

    /// A key was pressed: it, or what the user's mappings make of it.
    pub fn key(&mut self, press: KeyPress) {
        let plain = self.keymap.is_empty()
            || self.is_picking()
            || self.is_grid()
            || self.is_jumping()
            || self.is_retyping();
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
        }
        self.hints_keyed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mappings_are_read_and_checked() {
        let (keymap, complaint) = Keymap::parse(
            [(";", ":"), ("<C-s>", ":w<CR>")],
            [("<C-s>", "<Esc>:w<CR>")],
        );
        assert_eq!(complaint, None);
        assert_eq!((keymap.normal.len(), keymap.insert.len()), (2, 1));
        assert_eq!(keymap.normal[1].1.len(), 3);
        // What cannot be a mapping is said and left out.
        let (keymap, complaint) = Keymap::parse([("<Nope>", "x")], [("jk", "<Esc>")]);
        assert!(keymap.is_empty());
        assert!(complaint.is_some_and(|why| why.contains("<Nope>")));
        let (_, complaint) = Keymap::parse([], [("jk", "<Esc>")]);
        assert!(complaint.is_some_and(|why| why.contains("one key")));
    }
}
