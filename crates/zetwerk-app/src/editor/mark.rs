//! Keys typed that are taken back: the first keys of a mapping of insert
//! mode (`jk` for Escape), typed as they come and taken back when the
//! mapping is complete, as Vim does.
//!
//! A [`Mark`] is the state of the text, its history and the engine before
//! the first of those keys; [`Editor::take_back`] brings all of them back
//! to it. What was typed then is as if never typed: no undo step, nothing
//! for `.` to repeat, nothing in a macro being recorded.

use zetwerk_core::{Replace, StateId};
use zetwerk_vim::Engine;

use super::Editor;

/// The editor as it was before keys that may be taken back.
#[derive(Clone)]
pub struct Mark {
    engine: Box<Engine>,
    text: String,
    /// The state of the history, how many states it held, and where redo
    /// led from that state.
    state: StateId,
    states: usize,
    redo: Option<StateId>,
}

impl Editor {
    /// The editor as it is now, to be brought back with
    /// [`Editor::take_back`].
    #[must_use]
    pub fn mark(&self) -> Mark {
        let history = self.document.history();
        Mark {
            engine: Box::new(self.engine.clone()),
            text: self.document.text().to_owned(),
            state: history.current(),
            states: history.len(),
            redo: history.redo_target(history.current()),
        }
    }

    /// Takes back all that keys did since `mark` was made, where only
    /// keys of this editor changed it since. Returns whether the text
    /// changed.
    pub fn take_back(&mut self, mark: Mark) -> bool {
        let history = self.document.history();
        let (current, states) = (history.current(), history.len());
        let changed = self.document.text() != mark.text;
        if states > mark.states {
            // The keys began an undo step of their own (or more): back to
            // before them, and redo leads where it led then.
            let ancestor = std::iter::successors(Some(current), |state| history.parent(*state))
                .any(|state| state == mark.state);
            if !ancestor || self.document.goto(mark.state).is_err() {
                return false;
            }
            if let Some(redo) = mark.redo {
                let _ = self.document.prefer(redo);
            }
        } else if current == mark.state && changed {
            // Joined to the step before: what they changed is changed back
            // within it, which leaves the step as it was.
            let now = self.document.text();
            let start = now
                .bytes()
                .zip(mark.text.bytes())
                .take_while(|(a, b)| a == b)
                .count();
            let tail = now[start..]
                .bytes()
                .rev()
                .zip(mark.text[start..].bytes().rev())
                .take_while(|(a, b)| a == b)
                .count();
            let (mut start, mut now_end, mut then_end) =
                (start, now.len() - tail, mark.text.len() - tail);
            while !(now.is_char_boundary(start) && mark.text.is_char_boundary(start)) {
                start -= 1;
            }
            while !(now.is_char_boundary(now_end) && mark.text.is_char_boundary(then_end)) {
                now_end += 1;
                then_end += 1;
            }
            let replace = Replace::new(start..now_end, &mark.text[start..then_end]);
            if self.document.edit(&[replace], true).is_err() {
                return false;
            }
        } else if current != mark.state {
            return false;
        }
        self.engine = *mark.engine;
        self.failure = None;
        changed
    }
}

impl Editor {
    /// What is selected, as parts of the text: the one range of a
    /// selection, or the range in each line of a block (Ctrl-V). Empty
    /// where nothing is.
    #[must_use]
    pub fn selections(&self) -> Vec<std::ops::Range<usize>> {
        let text = self.document.text();
        self.engine
            .block(text)
            .unwrap_or_else(|| self.engine.selection(text).into_iter().collect())
    }
}
