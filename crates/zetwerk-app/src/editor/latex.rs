//! A LaTeX name typed in an equation and left (see
//! [`crate::latex::typing`]): what the cursor leaves behind becomes Typst
//! as if a key had ended it, in the same undo step as the key.

use zetwerk_core::Replace;
use zetwerk_vim::Mode;

use super::Editor;

impl Editor {
    /// Where the LaTeX name begins that the cursor is typing at the end
    /// of, before a key on a page (`page`).
    pub(super) fn latex_pending(&self, page: bool) -> Option<usize> {
        let typing = page && self.engine.mode() == Mode::Insert;
        typing
            .then(|| {
                crate::latex::typing::pending_at(&self.document.buffer().0, self.engine.cursor())
            })
            .flatten()
            .map(|pending| pending.range.start)
    }

    /// After a key on a page: the name that began at `start` before it,
    /// if the cursor is no longer typing at its end, becomes what it is.
    pub(super) fn latex_left(&mut self, start: Option<usize>) {
        let Some(start) = start else {
            return;
        };
        let source = &self.document.buffer().0;
        let cursor = self.engine.cursor();
        let still = self.engine.mode() == Mode::Insert
            && crate::latex::typing::pending_at(source, cursor)
                .is_some_and(|pending| pending.range.start == start);
        if still {
            return;
        }
        let Some((range, with)) = crate::latex::typing::left(source, start) else {
            return;
        };
        let cursor = if cursor >= range.end {
            cursor - range.len() + with.len()
        } else if cursor > range.start {
            range.start + with.len().saturating_sub(1)
        } else {
            cursor
        };
        if let Err(err) = self
            .document
            .edit(&[Replace::new(range, with.as_str())], true)
        {
            self.failure = Some(err.to_string());
            return;
        }
        self.engine.set_cursor(cursor, self.document.text());
    }
}
