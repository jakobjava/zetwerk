//! What of the editor is kept for when the text is opened again (see
//! `viewer::place`): the marks, the places jumped from, the registers
//! and the last search, as the engine has them; and given back to it.

use super::Editor;

impl Editor {
    /// The marks of the text (a–z), by name. See
    /// [`zetwerk_vim::Engine::marks_kept`].
    #[must_use]
    pub fn marks_kept(&self) -> Vec<(char, usize)> {
        self.engine.marks_kept()
    }

    /// Puts mark `name` at byte `at`, or as near as it may be.
    pub fn set_mark_kept(&mut self, name: char, at: usize) {
        self.engine.set_mark_kept(name, at, self.document.text());
    }

    /// The places jumped from, the oldest first.
    #[must_use]
    pub fn jumps_kept(&self) -> &[usize] {
        self.engine.jumps_kept()
    }

    /// Makes `jumps` (the oldest first) the places jumped from.
    pub fn set_jumps_kept(&mut self, jumps: &[usize]) {
        self.engine.set_jumps_kept(jumps, self.document.text());
    }

    /// The registers that hold text; see
    /// [`zetwerk_vim::Engine::registers_kept`].
    #[must_use]
    pub fn registers_kept(&self) -> Vec<(char, &str, bool)> {
        self.engine.registers_kept()
    }

    /// Makes register `name` hold `text` as it did when it was kept.
    pub fn set_register_kept(&mut self, name: char, text: &str, linewise: bool) {
        self.engine.set_register_kept(name, text, linewise);
    }

    /// The last search: what for, whether forward, and whether only at
    /// the start and the end of words.
    #[must_use]
    pub fn search_kept(&self) -> Option<(&str, bool, bool, bool)> {
        self.engine.search_kept()
    }

    /// Makes the search kept the last search.
    pub fn set_search_kept(&mut self, pattern: &str, forward: bool, words: (bool, bool)) {
        self.engine.set_search_kept(pattern, forward, words);
    }
}
