//! What the window waits for: said, bounded, and given up on.
//!
//! Typst cannot be stopped in the middle of a compile. What is done
//! instead:
//!
//! - **Compiles that run long** go on in the background (see
//!   [`zetwerk_typst::Compiler`]); typing never waits for them. One that
//!   has run [`LONG`] is said in the status line, with the seconds it has
//!   run, and Ctrl-C in normal mode gives it up: the page stays that of
//!   the text before, and the status line says how to go on (`u`,
//!   `:why`). A compile that newer text waits behind is given up on by
//!   itself after a while (see [`zetwerk_typst::Bounds`]); where too many
//!   are given up on already, the status line says what the text waits
//!   for, and how long it has.
//! - **Parts typeset by themselves** (see `local.rs`) are typeset on the
//!   window's thread, with every key that needs one. One that took longer
//!   than [`PART_BUDGET`] (a heavy rule before it, a big table) turns
//!   them off for the document: the page then waits for the whole, as it
//!   does where a part cannot be typeset, and keys do not. A whole
//!   compile within the budget says the cause is gone; else parts are
//!   tried again after [`PARTS_AGAIN`], twice as long after every part
//!   that was slow again.

use std::time::{Duration, Instant};

use super::{Mode, Viewer, editor};
use crate::keys::{Key, KeyPress};

/// A compile is said to run long, and can be stopped, once it has run
/// this long. A one-character change of a three-hundred-page document
/// compiles in 0.4 s, on battery in about twice that (see
/// `tests/waits.rs`): anything that is still at it after this has
/// something in it that takes long.
pub(super) const LONG: Duration = Duration::from_secs(2);

/// A part typeset by itself that takes longer than this costs every key
/// that needs one as much. A part of the article and of fifty pages of
/// it takes 8–30 ms, on battery and with the machine busy up to three
/// times that; with a loop of 200 000 rounds before the text 220 ms, of
/// two million 2.1 s (see `tests/waits.rs`).
pub(super) const PART_BUDGET: Duration = Duration::from_millis(150);

/// How long parts are left off after one was slow, the first time.
const PARTS_AGAIN: Duration = Duration::from_secs(60);

/// What the window waits for (see the module's documentation).
pub(super) struct Waiting {
    /// Since when the compile under way runs, if one does.
    compiling: Option<Instant>,
    /// Since when the text waits for a compile given up on to end.
    held: Option<Instant>,
    /// What the status line said of it when it was last drawn.
    said: Option<String>,
    /// Ctrl-C asked for the compile under way to be given up on.
    stop: bool,
    /// The revision whose compile was given up on at the user's word,
    /// and how long it had run.
    stopped: Option<(u64, Duration)>,
    /// What a part may take.
    pub(super) part_budget: Duration,
    /// The last part that took longer than that.
    pub(super) slow_part: Option<SlowPart>,
}

impl Default for Waiting {
    fn default() -> Self {
        Self {
            compiling: None,
            held: None,
            said: None,
            stop: false,
            stopped: None,
            part_budget: PART_BUDGET,
            slow_part: None,
        }
    }
}

/// A part that took longer than [`PART_BUDGET`]: how long, when, and how
/// many slow ones there were in a row.
pub(super) struct SlowPart {
    pub(super) took: Duration,
    pub(super) at: Instant,
    pub(super) times: u32,
}

impl SlowPart {
    /// Until when parts are left off for it.
    fn off_until(&self) -> Instant {
        self.at + PARTS_AGAIN * 2_u32.pow(self.times.saturating_sub(1).min(4))
    }
}

/// `duration` in whole seconds, as the status line says it.
fn seconds(duration: Duration) -> u64 {
    duration.as_secs()
}

impl Viewer {
    /// The window says what compiles: since when the compile under way
    /// runs, and since when the text waits for one given up on. Returns
    /// whether the status line is to say something else for it now.
    pub fn compiling(&mut self, since: Option<Instant>, held: Option<Instant>) -> bool {
        self.waiting.compiling = since;
        self.waiting.held = held;
        self.compile_words(Instant::now()) != self.waiting.said
    }

    /// When the status line is to say something else of what compiles,
    /// if it is: the window wakes then, and only while a compile runs.
    #[must_use]
    pub fn compiling_at(&self, now: Instant) -> Option<Instant> {
        let since = self.waiting.compiling.or(self.waiting.held)?;
        let ran = now.saturating_duration_since(since);
        Some(if ran < LONG {
            since + LONG
        } else {
            since + Duration::from_secs(seconds(ran) + 1)
        })
    }

    /// Whether Ctrl-C asked for the compile under way to be given up on;
    /// once.
    pub fn take_stop(&mut self) -> bool {
        std::mem::take(&mut self.waiting.stop)
    }

    /// The compile under way was given up on, after it had run `ran`; or
    /// there was none.
    pub fn stopped(&mut self, ran: Option<Duration>) {
        self.waiting.compiling = None;
        self.waiting.held = None;
        let Some(ran) = ran else {
            return self.notice("nothing is being typeset");
        };
        self.waiting.stopped = Some((self.revision, ran));
        self.alert(format!(
            "typesetting stopped after {} s: the page is of the text before; \
             u undoes the change, :why says more",
            seconds(ran)
        ));
    }

    /// Ctrl-C in normal mode while a compile runs long: it is to be
    /// given up on; and an export being written stops. Returns whether
    /// the key was taken for that.
    pub(super) fn stop_key(&mut self, press: KeyPress) -> bool {
        if press.key != Key::Char('c') || !press.ctrl || press.alt {
            return false;
        }
        let normal = match &self.editor {
            Some(editor) => editor.mode() == editor::Mode::Normal,
            None => self.mode == Mode::Normal,
        };
        let now = Instant::now();
        let long = |since: Option<Instant>| {
            since.is_some_and(|since| now.saturating_duration_since(since) >= LONG)
        };
        if !normal {
            return false;
        }
        let export = self.stop_export();
        if !(long(self.waiting.compiling) || self.waiting.held.is_some()) {
            return export;
        }
        self.waiting.stop = true;
        true
    }

    /// What the status line says of what compiles, at `now`, where it
    /// says more than that the page is behind.
    pub(super) fn compile_words(&self, now: Instant) -> Option<String> {
        let ran = |since: Instant| seconds(now.saturating_duration_since(since));
        if let Some(held) = self.waiting.held {
            return Some(format!(
                "waiting {} s for a compile given up on, Ctrl-C stops  ",
                ran(held)
            ));
        }
        if let Some(since) = self.waiting.compiling
            && now.saturating_duration_since(since) >= LONG
        {
            return Some(format!("typesetting {} s, Ctrl-C stops  ", ran(since)));
        }
        let stopped = self.waiting.stopped.map(|(revision, _)| revision);
        (stopped == Some(self.revision) && !self.is_compiled()).then(|| "stopped  ".to_owned())
    }

    /// What the status line says now of what compiles, and remembers that
    /// it did.
    pub(super) fn said_of_compiling(&mut self) -> Option<String> {
        let words = self.compile_words(Instant::now());
        self.waiting.said.clone_from(&words);
        words
    }

    /// What `:why` says while the page is behind for what the window
    /// waits for, if it is.
    pub(super) fn why_waiting(&self) -> Option<String> {
        if let Some((revision, ran)) = self.waiting.stopped
            && revision == self.revision
            && !self.is_compiled()
        {
            return Some(format!(
                "typesetting this text was stopped after {} s: something in it takes \
                 long (a long loop, deep recursion, a heavy package); u undoes the \
                 change, and any change typesets it again",
                seconds(ran)
            ));
        }
        let since = self.waiting.compiling.or(self.waiting.held)?;
        let ran = Instant::now().saturating_duration_since(since);
        (ran >= LONG).then(|| {
            format!(
                "the text has been typesetting for {} s: something in it takes long; \
                 Ctrl-C stops it, and the page stays as it is",
                seconds(ran)
            )
        })
    }

    /// [`Viewer::typeset_locally`], unless a part was slow for this
    /// document lately; a part slower than the budget turns parts off.
    pub(super) fn typeset_locally_in_time(&mut self) -> Result<(), String> {
        let now = Instant::now();
        if let Some(slow) = &self.waiting.slow_part
            && now < slow.off_until()
        {
            return Err(format!(
                "a part took {} ms to typeset by itself: the page waits for all of it",
                slow.took.as_millis()
            ));
        }
        let done = self.typeset_locally();
        let took = now.elapsed();
        if took > self.waiting.part_budget {
            let times = self.waiting.slow_part.as_ref().map_or(0, |slow| slow.times) + 1;
            self.waiting.slow_part = Some(SlowPart {
                took,
                at: Instant::now(),
                times,
            });
        } else if done.is_ok() {
            self.waiting.slow_part = None;
        }
        done
    }

    /// A whole compile took `took`: within what a part may take, no part
    /// of it can take longer, and what made one slow is gone. (Not where
    /// it is only quicker than the slow part: a loop before the text
    /// takes as long in either.)
    pub(super) fn whole_took(&mut self, took: Duration) {
        if took <= self.waiting.part_budget {
            self.waiting.slow_part = None;
        }
    }
}
