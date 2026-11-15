//! What a key did with text that goes into or comes out of a register,
//! for the host to show: a yank changes nothing to be seen, and what a
//! deletion took may have shown as nothing or been far from the cursor.

use std::ops::Range;

/// What the last key yanked, deleted or put (see [`crate::Engine::took`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Took {
    /// `y` and its like: these bytes of the text were yanked.
    Yanked(Range<usize>),
    /// `d`, `c`, `x` and their like: `text` went from byte `at`; whole
    /// lines of it, if `linewise`.
    Deleted {
        text: String,
        at: usize,
        linewise: bool,
    },
    /// `p` and `P`: these bytes of the text came in.
    Put(Range<usize>),
}
