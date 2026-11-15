//! `*` and `#` where the host shows the text as something to be read
//! (see [`Host::reading`]): they look for the word as it is read, not
//! for what it is written with, and find it wherever it is read so.

use super::{Engine, Host, Search};
use crate::find;
use crate::text::{Class, class};

impl Engine {
    /// `*` and `#` on what is read: the word at the cursor or the next
    /// one in the paragraph, as a whole word, whatever its case (Vim's
    /// `*` with 'ignorecase'). Returns whether it did its work: not where
    /// the host has no reading, or the paragraph has nothing more.
    pub(super) fn shown_search_word(&mut self, forward: bool, host: &mut impl Host) -> bool {
        let Some(reading) = host.reading() else {
            return false;
        };
        let Some((word, start)) = find::word_read(&reading, self.cursor) else {
            return false;
        };
        let is_word = |c: Option<char>| c.is_some_and(|c| class(c, false) == Class::Word);
        self.last_search = Some(Search {
            word_start: is_word(word.chars().next()),
            word_end: is_word(word.chars().next_back()),
            pattern: format!("\\c{word}"),
            forward,
        });
        self.search(forward, start, host);
        true
    }
}
