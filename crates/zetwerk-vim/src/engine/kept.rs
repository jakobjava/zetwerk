//! What a user expects to find again when a text is opened anew, as Vim
//! keeps it in its shada file: the marks, the places jumped from, the
//! registers and the last search.
//!
//! The engine keeps none of it on disk itself. A host asks for it here
//! when it leaves a text and gives it back when the text is opened
//! again; where the text changed in between, the host says where the
//! places are now.

use super::{Engine, Register, Search, clamp};
use crate::text::Text;

impl Engine {
    /// The marks of the text (`ma` … `mz`), by name.
    #[must_use]
    pub fn marks_kept(&self) -> Vec<(char, usize)> {
        let mut marks: Vec<(char, usize)> = self
            .marks
            .iter()
            .filter(|(name, _)| name.is_ascii_lowercase())
            .map(|(name, at)| (*name, *at))
            .collect();
        marks.sort_unstable();
        marks
    }

    /// Puts mark `name` (a–z) at byte `at` of `text`, or as near as it
    /// may be.
    pub fn set_mark_kept(&mut self, name: char, at: usize, text: &str) {
        if name.is_ascii_lowercase() {
            self.marks.insert(name, clamp(Text(text), at, false));
        }
    }

    /// The places jumped from, the oldest first (Ctrl-O goes back
    /// through them).
    #[must_use]
    pub fn jumps_kept(&self) -> &[usize] {
        &self.jumps
    }

    /// Makes `jumps` (the oldest first) the places jumped from, with
    /// Ctrl-O going back from the newest. As many as Vim keeps, at most.
    pub fn set_jumps_kept(&mut self, jumps: &[usize], text: &str) {
        let first = jumps.len().saturating_sub(super::JUMPS);
        self.jumps = jumps[first..]
            .iter()
            .map(|at| clamp(Text(text), *at, false))
            .collect();
        self.jumps.dedup();
        self.jumps_at = self.jumps.len();
    }

    /// The registers that hold text (named, numbered, the usual one and
    /// the small delete), each with its text and whether it is whole
    /// lines; in the order of their names.
    #[must_use]
    pub fn registers_kept(&self) -> Vec<(char, &str, bool)> {
        let mut registers: Vec<(char, &str, bool)> = self
            .registers
            .iter()
            .filter(|(name, _)| {
                name.is_ascii_lowercase() || name.is_ascii_digit() || matches!(name, '"' | '-')
            })
            .map(|(name, register)| (*name, register.text.as_str(), register.linewise))
            .collect();
        registers.sort_unstable_by_key(|(name, _, _)| *name);
        registers
    }

    /// Makes register `name` hold `text`, whole lines or not, as it did
    /// when it was kept. Registers that are not kept are left alone.
    pub fn set_register_kept(&mut self, name: char, text: &str, linewise: bool) {
        if name.is_ascii_lowercase() || name.is_ascii_digit() || matches!(name, '"' | '-') {
            let register = Register {
                text: text.to_owned(),
                linewise,
            };
            self.registers.insert(name, register);
        }
    }

    /// The last search: what for, whether forward, and whether a match
    /// may not continue a word before it or be continued by one after it.
    #[must_use]
    pub fn search_kept(&self) -> Option<(&str, bool, bool, bool)> {
        self.last_search.as_ref().map(|search| {
            (
                search.pattern.as_str(),
                search.forward,
                search.word_start,
                search.word_end,
            )
        })
    }

    /// Makes the last search the one kept: `n` and `N` go on with it.
    /// It counts as no search made: nothing is to be shown of it.
    pub fn set_search_kept(&mut self, pattern: &str, forward: bool, words: (bool, bool)) {
        if pattern.is_empty() {
            return;
        }
        self.last_search = Some(Search {
            pattern: pattern.to_owned(),
            forward,
            word_start: words.0,
            word_end: words.1,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_kept_comes_back_as_it_was() {
        let text = "one two\nthree four\nfive\n";
        let mut engine = Engine::new();
        engine.set_mark_kept('a', 4, text);
        engine.set_mark_kept('A', 4, text);
        engine.set_mark_kept('q', 400, text);
        assert_eq!(engine.marks_kept(), [('a', 4), ('q', 19)]);
        engine.set_jumps_kept(&[0, 8, 8, 19], text);
        assert_eq!(engine.jumps_kept(), [0, 8, 19]);
        engine.set_register_kept('a', "two", false);
        engine.set_register_kept('"', "three four\n", true);
        engine.set_register_kept('+', "clipboard", false);
        assert_eq!(
            engine.registers_kept(),
            [('"', "three four\n", true), ('a', "two", false)]
        );
        assert_eq!(engine.search_kept(), None);
        engine.set_search_kept("four", false, (true, false));
        assert_eq!(engine.search_kept(), Some(("four", false, true, false)));
        assert_eq!(engine.searches(), 0, "a kept search is no search made");
    }
}
