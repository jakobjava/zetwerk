//! What happened lately, kept so that a moment that felt wrong can be
//! written down by one key and played again exactly.
//!
//! - **The recorder** is always on. It keeps the last keys as they came
//!   from the keyboard (before the user's mappings), each with the time
//!   since the moment before and what the window did with it: the mode,
//!   the cursor, the revision of the text, and whether the page was put
//!   right by the predictor, by a part typeset alone, or waited (and
//!   why). Between the keys it keeps what else happened: compiles begun
//!   and landed, what the status line said, lists opened and closed, the
//!   view scrolled or zoomed, the window resized, files changed on disk,
//!   tools agents called (a line each, no content). `[report] keys = 500`
//!   says how many keys; 0 keeps none.
//! - **The text is not copied per key.** Every so many keys, when
//!   nothing is under way (normal mode, no list up), the texts of the
//!   open files are kept as they are (a file that has not changed since
//!   the last time shares that copy), with the cursor and the view: a
//!   *start*. The keys after the oldest start are kept, and a start
//!   goes once the next one leaves enough keys. So the text at the first
//!   key kept is known exactly, and the keys from it lead to the text
//!   now. Where the text changed between keys (an agent, a file loaded
//!   again) the texts are kept again at that moment, and a replay puts
//!   them in there; a file opened by a key is kept as it was opened.
//! - **The report** (`g!`, Ctrl-G in any mode and over any list, or
//!   `:report [a few words]`) is a directory `reports/<date-time>/` under
//!   the state directory, only the user's: `keys.txt`, the moments in
//!   order, keys in the notation `--keys` takes; `before/`, the open
//!   files at the first key; `between/`, texts put in on the way;
//!   `after/`, the files as they are now, unsaved changes and all;
//!   `needs.txt`, the other files the document needs, named (not
//!   copied); `state.txt`, what sets the stage again; `window.png`, the
//!   window as it is; `note.txt`, the user's words. It is written on a
//!   thread of its own; the window only gathers what goes in. The keys
//!   that asked for it are marked and not played again.
//! - **Privacy.** A report holds the text and what was typed. It stays
//!   on this machine; nothing is sent anywhere. `:report` says so the
//!   first time, `:reports` lists them and deletes them. What is typed
//!   while a prompt is marked as not recorded
//!   ([`Viewer::typing_unrecorded`]) is kept as `hidden-key` only.
//!
//! Playing a report again is in [`replay`].

pub mod replay;

use std::collections::{HashSet, VecDeque};
use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant, SystemTime};

use typst::layout::{Frame, FrameItem};
use typst::syntax::FileId;
use typst_layout::PagedDocument;
use zetwerk_typst::{CompileOutput, FontSet};

use super::assist::Target;
use super::{Mode, Paging, Viewer, editor};
use crate::gpu::{Gpu, Quad};
use crate::keys::{Key, KeyPress};

/// How many keys are kept unless the settings say otherwise.
pub const DEFAULT_KEYS: usize = 500;
/// The key that writes a report from anywhere: Ctrl-G.
pub const REPORT_KEY: KeyPress = KeyPress {
    key: Key::Char('g'),
    ctrl: true,
    alt: false,
};
/// How many moments other than keys may be kept per key, at most:
/// a bound whatever happens between keys.
const MOMENTS_PER_KEY: usize = 4;
/// How long the thread that writes a report waits for the picture of the
/// window.
const PICTURE_PATIENCE: Duration = Duration::from_secs(3);
/// Where reports are kept under the state directory.
const REPORTS: &str = "reports";

/// What the user's settings say about reports, and what a report says of
/// the program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// How many keys are kept; 0 keeps none.
    pub keys: usize,
    /// The settings file as it was read, if there was one.
    pub config: Option<String>,
    /// zetwerk's version and the commit it was built from.
    pub built: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            keys: DEFAULT_KEYS,
            config: None,
            built: format!("zetwerk {}", env!("CARGO_PKG_VERSION")),
        }
    }
}

/// How the page came to show a key's change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    /// Nothing to show, or nothing was asked.
    Nothing,
    /// Worked out by the predictor.
    Predicted,
    /// A part typeset alone.
    Part,
    /// Neither: the page waits for a compile.
    Waited,
}

/// What is up over the page, as bits.
const LIST: u16 = 1;
const COMPLETION: u16 = 1 << 1;
const HISTORY: u16 = 1 << 2;
const GRID: u16 = 1 << 3;
const JUMP: u16 = 1 << 4;
const PROMPT: u16 = 1 << 5;
const COMMAND: u16 = 1 << 6;
const PERFORMANCE: u16 = 1 << 7;
const PANELS: [(u16, &str); 8] = [
    (LIST, "a list"),
    (COMPLETION, "completions"),
    (HISTORY, "the history"),
    (GRID, "the overview of pages"),
    (JUMP, "jump labels"),
    (PROMPT, "a prompt"),
    (COMMAND, "the command line"),
    (PERFORMANCE, "the performance panel"),
];

/// A key and what became of it.
#[derive(Debug, Clone)]
struct Keyed {
    press: KeyPress,
    /// The keys that reached the editor for it, after the mappings: the
    /// first few of them, and how many.
    reached: [KeyPress; 4],
    reached_count: u16,
    /// Whether it was handled at all after the mappings: `false` while
    /// the mappings wait for more keys.
    mode: editor::Mode,
    /// Whether nothing is under way after it: no count, operator or key
    /// waiting for another, no command line, no list.
    rest: bool,
    file: Option<FileId>,
    cursor: usize,
    revision: u64,
    shown: Shown,
    declined: Option<Arc<str>>,
    /// That it asked for a report (this one or one before).
    report: bool,
    /// That it was typed while typing was not to be recorded.
    hidden: bool,
}

/// One thing that happened.
#[derive(Debug, Clone)]
enum What {
    Key(Keyed),
    /// The text of this revision was handed over to be compiled.
    Began(u64),
    Compiled {
        revision: u64,
        problem: Option<Arc<str>>,
        millis: f64,
    },
    Said {
        text: Arc<str>,
        alert: bool,
    },
    Opened(&'static str),
    Closed(&'static str),
    Scrolled {
        top: f64,
        zoom: f64,
    },
    Window {
        size: (u32, u32),
        display: f64,
        ui: f64,
    },
    Layout(super::Panes, Paging),
    Disk(Arc<Path>),
    Agent(&'static str),
    /// The texts were set to these between keys.
    TextSet(Arc<Kept>),
    /// A file was opened by the key after this, with this text.
    FileOpened(FileId, Arc<str>),
    /// The cursor was put here between keys (a click, an agent).
    Moved(usize),
}

#[derive(Debug, Clone)]
struct Moment {
    at: Instant,
    what: What,
}

/// The texts of the open files at a moment, with what sets the stage.
#[derive(Debug)]
pub(super) struct Kept {
    /// Every open file and its text, in the order they were opened.
    files: Vec<(FileId, Arc<str>)>,
    stage: replay::Stage,
}

/// A moment a replay can begin at.
#[derive(Debug, Clone)]
struct Start {
    /// The number of the first moment after it.
    moment: u64,
    /// How many keys were recorded before it.
    keyed: u64,
    kept: Arc<Kept>,
}

/// The key being handled, until it is over.
#[derive(Debug, Clone)]
struct Open {
    press: KeyPress,
    at: Instant,
    reached: [KeyPress; 4],
    reached_count: u16,
    /// The counts of `Stats` before it: predicted, typeset in part,
    /// declined.
    counts: (u64, u64, u64),
    files: usize,
}

/// What was last recorded of what changes between moments.
#[derive(Debug, Clone, Default)]
struct Seen {
    revision: u64,
    said: Option<Arc<str>>,
    panels: u16,
    view: (u64, u64),
    window: (u32, u32, u64, u64),
    layout: Option<(super::Panes, Paging)>,
    declined: Option<Arc<str>>,
    files: Vec<FileId>,
    /// The open files and their editors' revisions, after the last key.
    texts: Vec<(FileId, u64)>,
    /// The file being edited and the cursor in it, after the last key.
    cursor: Option<(FileId, usize)>,
}

/// The thread that writes reports, one after the other.
struct Worker {
    jobs: Option<Sender<(Job, PictureTaken)>>,
    done: Receiver<Result<String, String>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Worker {
    /// Starts the thread; `wake` is called from it whenever a report is
    /// written, or could not be.
    fn spawn(wake: Box<dyn Fn() + Send>) -> std::io::Result<Self> {
        let (jobs, waiting) = mpsc::channel::<(Job, PictureTaken)>();
        let (finished, done) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("zetwerk-report".into())
            .spawn(move || {
                for (job, picture) in waiting {
                    // Whatever goes wrong with one report, the thread is
                    // there for the next, and the window hears of it.
                    let outcome = catch_unwind(AssertUnwindSafe(|| job.run(Some(&picture))))
                        .unwrap_or_else(|_| Err("writing it crashed".to_owned()));
                    if finished.send(outcome).is_err() {
                        break;
                    }
                    wake();
                }
            })?;
        Ok(Self {
            jobs: Some(jobs),
            done,
            thread: Some(thread),
        })
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // A report begun is finished before the window goes.
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What the recorder keeps.
pub(super) struct Recorder {
    settings: Settings,
    font_set: Option<FontSet>,
    moments: VecDeque<Moment>,
    /// The number of `moments[0]`.
    first: u64,
    /// How many keys were recorded.
    keyed: u64,
    starts: VecDeque<Start>,
    /// Whether a start is wanted at the next key, whatever is under way.
    want_start: bool,
    /// The texts last kept, with the revision of the editor they are of:
    /// what is the same is shared.
    shared: Vec<(FileId, u64, Arc<str>)>,
    seen: Seen,
    open: Option<Open>,
    /// A report asked for during a key: written when it is over.
    wanted: Option<(String, bool)>,
    hidden: bool,
    /// The thread that writes a report waits for the window's picture.
    /// (Before the worker: dropped first, it lets the thread go on.)
    picture: Option<Sender<Result<Picture, String>>>,
    /// How to tell the window a report is written, until the thread that
    /// writes them has it; with neither, reports are written on the spot.
    wake: Option<Box<dyn Fn() + Send>>,
    worker: Option<Worker>,
    /// What the report being written is said to be, after it is.
    writing: Option<String>,
}

/// The pixels of the window: width, height, and rows of RGBA.
type Picture = (u32, u32, Vec<u8>);
/// What the window gives the thread that writes a report: the picture.
type PictureTaken = Receiver<Result<Picture, String>>;
/// Files by their names in the document's directory, and their texts.
type Files = Vec<(String, Arc<str>)>;

impl Default for Recorder {
    fn default() -> Self {
        Self::new(Settings::default())
    }
}

impl Recorder {
    fn new(settings: Settings) -> Self {
        let keys = settings.keys;
        Self {
            settings,
            font_set: None,
            moments: VecDeque::with_capacity(keys.min(10_000) * 2),
            first: 0,
            keyed: 0,
            starts: VecDeque::new(),
            want_start: true,
            shared: Vec::new(),
            seen: Seen::default(),
            open: None,
            wanted: None,
            hidden: false,
            picture: None,
            wake: None,
            worker: None,
            writing: None,
        }
    }

    fn on(&self) -> bool {
        self.settings.keys > 0
    }

    /// How many keys apart starts are kept.
    fn every(&self) -> u64 {
        (self.settings.keys / 4).max(8) as u64
    }

    fn push(&mut self, at: Instant, what: What) {
        self.moments.push_back(Moment { at, what });
    }

    /// Puts a moment that says where the view or the window now is: in
    /// place of the last one if that said so too, so that a held key or
    /// a turned wheel is one moment.
    fn push_or_replace(&mut self, at: Instant, what: What) {
        let same_kind = self.moments.back().is_some_and(|last| {
            matches!(
                (&last.what, &what),
                (What::Scrolled { .. }, What::Scrolled { .. })
                    | (What::Window { .. }, What::Window { .. })
            )
        });
        if same_kind && let Some(last) = self.moments.back_mut() {
            *last = Moment { at, what };
            return;
        }
        self.push(at, what);
    }

    /// Lets go of what no replay needs: starts that a later one makes
    /// unnecessary, and the moments before the oldest start.
    fn trim(&mut self) {
        let keys = self.settings.keys as u64;
        while self.starts.len() >= 2 && self.keyed - self.starts[1].keyed >= keys {
            self.starts.pop_front();
        }
        let end = self.first + self.moments.len() as u64;
        let from = self.starts.front().map_or(end, |start| start.moment);
        while self.first < from && self.moments.pop_front().is_some() {
            self.first += 1;
        }
        // Whatever happens, no more than this; a replay then begins
        // where a start is again.
        let most = self.settings.keys.saturating_mul(MOMENTS_PER_KEY).max(64);
        while self.moments.len() > most && self.moments.pop_front().is_some() {
            self.first += 1;
        }
        while self
            .starts
            .front()
            .is_some_and(|start| start.moment < self.first)
        {
            self.starts.pop_front();
        }
        if self.starts.is_empty() {
            self.want_start = true;
        }
    }
}

impl Viewer {
    /// Takes the report settings: how many keys to keep, the settings
    /// file, how the program was built; and which fonts it offers.
    pub(super) fn report_configured(&mut self, settings: &Settings, font_set: FontSet) {
        let mut recorder = Recorder::new(settings.clone());
        recorder.wake = self.report.wake.take();
        recorder.worker = self.report.worker.take();
        recorder.font_set = Some(font_set);
        self.report = recorder;
    }

    /// Has reports written on a thread of their own from now on, and the
    /// window's picture taken for them; `wake` is called from that thread
    /// when a report is written. See [`Viewer::report_frame`].
    pub fn report_in_background(&mut self, wake: impl Fn() + Send + 'static) {
        self.report.wake = Some(Box::new(wake));
    }

    /// Marks what is typed from now on, until called with `false`, as
    /// not to be recorded: for a prompt that may be given a secret. Such
    /// keys are kept as `hidden-key` only.
    pub fn typing_unrecorded(&mut self, hidden: bool) {
        self.report.hidden = hidden;
    }

    /// A key came from the keyboard, before the mappings. Returns whether
    /// it is taken here: the key that writes a report.
    pub(super) fn report_key(&mut self, press: KeyPress) -> bool {
        let chord = press == REPORT_KEY;
        if !self.report.on() {
            if chord {
                self.message = None;
                self.write_report(String::new(), true);
            }
            return chord;
        }
        let at = Instant::now();
        self.report_changes(at);
        let quiet = self.at_rest();
        let due = self
            .report
            .starts
            .back()
            .is_none_or(|last| self.report.keyed - last.keyed >= self.report.every());
        // Typing that goes on and on is begun from where it is, in the
        // end: the keys are not to pile up.
        let long = self.report.starts.back().is_some_and(|last| {
            self.report.keyed - last.keyed >= 2 * self.report.settings.keys as u64
        });
        if self.report.want_start || (due && quiet) || long {
            self.report_start(None);
        }
        self.report.open = Some(Open {
            press,
            at,
            reached: [press; 4],
            reached_count: 0,
            counts: (self.stats.predicted, self.stats.local, self.stats.declined),
            files: self.editor.iter().count() + self.others.len(),
        });
        if chord {
            self.message = None;
            self.report_keyed();
            self.write_report(String::new(), true);
        }
        chord
    }

    /// A key reached the editor, after the mappings.
    pub(super) fn report_reached(&mut self, press: KeyPress) {
        if let Some(open) = &mut self.report.open {
            if let Some(slot) = open.reached.get_mut(usize::from(open.reached_count)) {
                *slot = press;
            }
            open.reached_count = open.reached_count.saturating_add(1);
        }
    }

    /// The key is over: what it led to is recorded, and a report it
    /// asked for written.
    pub(super) fn report_keyed(&mut self) {
        let Some(open) = self.report.open.take() else {
            return;
        };
        // Files the key opened, as they were when it opened them: a
        // replay finds them so.
        if self.editor.iter().count() + self.others.len() != open.files {
            let known: HashSet<FileId> = self.report.seen.files.iter().copied().collect();
            let opened: Vec<(FileId, Arc<str>)> = self
                .editor
                .iter()
                .chain(&self.others)
                .filter(|editor| !known.contains(&editor.file()))
                .map(|editor| (editor.file(), Arc::from(editor.text())))
                .collect();
            for (file, text) in opened {
                self.report.push(open.at, What::FileOpened(file, text));
            }
        }
        let (predicted, local, declined) = open.counts;
        let shown = if self.stats.local > local {
            Shown::Part
        } else if self.stats.predicted > predicted {
            Shown::Predicted
        } else if self.stats.declined > declined {
            Shown::Waited
        } else {
            Shown::Nothing
        };
        let reason = if shown == Shown::Waited {
            match (&self.declined, &self.report.seen.declined) {
                (Some(now), Some(seen)) if now.as_str() == &**seen => Some(Arc::clone(seen)),
                (Some(now), _) => {
                    let reason: Arc<str> = Arc::from(now.as_str());
                    self.report.seen.declined = Some(Arc::clone(&reason));
                    Some(reason)
                }
                (None, _) => None,
            }
        } else {
            None
        };
        let editor = self.editor.as_ref();
        let mode = editor.map_or(editor::Mode::Normal, editor::Editor::mode);
        let rest = self.panels() == 0
            && self.typed.is_empty()
            && editor.is_none_or(|editor| editor.is_idle() && mode != editor::Mode::Command);
        let keyed = Keyed {
            press: if self.report.hidden {
                KeyPress::plain(Key::Char('*'))
            } else {
                open.press
            },
            reached: if self.report.hidden {
                [KeyPress::plain(Key::Char('*')); 4]
            } else {
                open.reached
            },
            reached_count: open.reached_count,
            mode,
            rest,
            file: editor.map(editor::Editor::file),
            cursor: editor.map_or(0, editor::Editor::cursor),
            revision: self.revision,
            shown,
            declined: reason,
            report: open.press == REPORT_KEY,
            hidden: self.report.hidden,
        };
        self.report.push(open.at, What::Key(keyed));
        self.report.keyed += 1;
        self.report_seen_texts();
        self.report_changes(Instant::now());
        self.report.trim();
        if let Some((note, chord)) = self.report.wanted.take() {
            self.write_report(note, chord);
        }
    }

    /// The text of the current revision is about to be handed over to be
    /// compiled (not `snapshot` itself: parts typeset alone take one too).
    pub fn report_began(&mut self) {
        if self.report.on() {
            self.report.push(Instant::now(), What::Began(self.revision));
        }
    }

    /// A compile landed (whether or not it is taken).
    pub(super) fn report_compiled(&mut self, output: &CompileOutput) {
        if !self.report.on() {
            return;
        }
        let problem = if output.compiled.panicked {
            Some(Arc::from("the Typst compiler crashed"))
        } else {
            output
                .compiled
                .errors
                .first()
                .map(|error| Arc::from(zetwerk_typst::describe(&*output.world, error)))
        };
        self.report.push(
            Instant::now(),
            What::Compiled {
                revision: output.revision,
                problem,
                millis: output.duration.as_secs_f64() * 1000.0,
            },
        );
    }

    /// A file the window depends on changed on disk.
    pub(super) fn report_disk(&mut self, path: &Path) {
        if self.report.on() {
            self.report
                .push(Instant::now(), What::Disk(Arc::from(path)));
        }
    }

    /// An agent called a tool. `name` is that of the function that asked
    /// the window for it (see the bridge); what is after its last `::`
    /// and before any closure is kept.
    pub fn agent_called(&mut self, name: &'static str) {
        if !self.report.on() {
            return;
        }
        let mut name = name;
        while let Some(outer) = name.strip_suffix("::{{closure}}") {
            name = outer;
        }
        let name = name.rsplit("::").next().unwrap_or(name);
        self.report.push(Instant::now(), What::Agent(name));
    }

    /// Records what changed since the last moment that keys are not the
    /// cause of, or not alone: what the status line says, what is up,
    /// where the view is, the window, the layout, and texts that were
    /// changed between keys.
    fn report_changes(&mut self, at: Instant) {
        // Changed between keys: kept as it is now, for a replay to put in.
        let open = self.report.open.is_some();
        if !open && self.revision != self.report.seen.revision {
            self.report.seen.revision = self.revision;
            let editors = self.editor.iter().chain(&self.others);
            let texts_changed = editors
                .map(|editor| (editor.file(), editor.revision()))
                .ne(self.report.seen.texts.iter().copied());
            if texts_changed && !self.report.starts.is_empty() {
                self.report_start(Some(at));
            }
        }
        let cursor = (self.editor.as_ref()).map(|editor| (editor.file(), editor.cursor()));
        if !open && cursor != self.report.seen.cursor {
            self.report.seen.cursor = cursor;
            if let Some((_, cursor)) = cursor {
                self.report.push(at, What::Moved(cursor));
            }
        }
        let said = self.message.as_ref().map(|message| message.text.as_str());
        if said != self.report.seen.said.as_deref() {
            self.report.seen.said = None;
            if let Some(message) = &self.message {
                let text: Arc<str> = Arc::from(message.text.as_str());
                self.report.seen.said = Some(Arc::clone(&text));
                let alert = message.alert;
                self.report.push(at, What::Said { text, alert });
            }
        }
        let panels = self.panels();
        if panels != self.report.seen.panels {
            for (bit, name) in PANELS {
                let (was, is) = (self.report.seen.panels & bit != 0, panels & bit != 0);
                if is && !was {
                    self.report.push(at, What::Opened(name));
                } else if was && !is {
                    self.report.push(at, What::Closed(name));
                }
            }
            self.report.seen.panels = panels;
        }
        let view = (self.view.top.to_bits(), self.view.zoom.to_bits());
        if view != self.report.seen.view {
            self.report.seen.view = view;
            let (top, zoom) = (self.view.top, self.view.zoom);
            self.report
                .push_or_replace(at, What::Scrolled { top, zoom });
        }
        let window = (
            self.window.0,
            self.window.1,
            self.display.to_bits(),
            self.ui.to_bits(),
        );
        if window != self.report.seen.window {
            self.report.seen.window = window;
            let what = What::Window {
                size: self.window,
                display: self.display,
                ui: self.ui,
            };
            self.report.push_or_replace(at, what);
        }
        let layout = Some((self.panes, self.paging));
        if layout != self.report.seen.layout {
            self.report.seen.layout = layout;
            self.report.push(at, What::Layout(self.panes, self.paging));
        }
        if self.report.seen.files.len() != self.editor.iter().count() + self.others.len() {
            self.report.seen.files = self
                .editor
                .iter()
                .chain(&self.others)
                .map(editor::Editor::file)
                .collect();
        }
    }

    /// Notes the revisions of the texts as they are: what changes them
    /// later and is no key is kept.
    fn report_seen_texts(&mut self) {
        self.report.seen.revision = self.revision;
        let texts = &mut self.report.seen.texts;
        texts.clear();
        let editors = self.editor.iter().chain(&self.others);
        texts.extend(editors.map(|editor| (editor.file(), editor.revision())));
        self.report.seen.cursor =
            (self.editor.as_ref()).map(|editor| (editor.file(), editor.cursor()));
    }

    /// Whether nothing is under way that a replay could not set up as it
    /// is: normal mode, no command begun, nothing up over the page, and
    /// the pages those of the text, typeset without complaint (a replay
    /// has no older pages to show in their place).
    fn at_rest(&self) -> bool {
        self.panels() == 0
            && self.typed.is_empty()
            && self.is_compiled()
            && self.problems.is_empty()
            && self.document.is_some()
            && (self.editor.as_ref())
                .is_some_and(|editor| editor.mode() == editor::Mode::Normal && editor.is_idle())
    }

    /// Whether the oldest start kept is at the first key: a replay from
    /// it has the whole history of the text, undo and all.
    #[cfg(test)]
    pub(super) fn report_from_first_key(&self) -> bool {
        self.report
            .starts
            .front()
            .is_some_and(|start| start.keyed == 0)
    }

    /// What is up over the page, as bits.
    fn panels(&self) -> u16 {
        let mut panels = 0;
        let mut set = |bit: u16, on: bool| {
            if on {
                panels |= bit;
            }
        };
        set(LIST, self.picker.is_some());
        set(COMPLETION, self.completion.is_some());
        set(HISTORY, self.history.is_some());
        set(GRID, self.grid.is_some());
        set(JUMP, self.is_jumping());
        set(PROMPT, self.is_retyping());
        let command = matches!(self.mode, Mode::Command(_))
            || (self.editor.as_ref()).is_some_and(|editor| editor.mode() == editor::Mode::Command);
        set(COMMAND, command);
        set(PERFORMANCE, self.performance_panel);
        panels
    }

    /// Keeps the texts of the open files as they are, with the stage: a
    /// start a replay can begin at. `between`: the texts changed between
    /// keys, and a replay is to put them in at this moment.
    fn report_start(&mut self, between: Option<Instant>) {
        let kept = Arc::new(self.report_kept());
        let moment = self.report.first + self.report.moments.len() as u64;
        if let Some(at) = between {
            self.report.push(at, What::TextSet(Arc::clone(&kept)));
        }
        self.report.starts.push_back(Start {
            moment,
            keyed: self.report.keyed,
            kept,
        });
        self.report.want_start = false;
        self.report_seen_texts();
        self.report.seen.files = self
            .editor
            .iter()
            .chain(&self.others)
            .map(editor::Editor::file)
            .collect();
        self.report.trim();
    }

    /// The texts of the open files and the stage, as they are: a text that
    /// has not changed since it was last kept is shared.
    fn report_kept(&mut self) -> Kept {
        let mut files = Vec::new();
        let mut shared = Vec::new();
        // In the order they were opened.
        for file in &self.opened {
            let Some(editor) = self
                .editor
                .iter()
                .chain(&self.others)
                .find(|editor| editor.file() == *file)
            else {
                continue;
            };
            let revision = editor.revision();
            let text = self
                .report
                .shared
                .iter()
                .find(|(of, at, _)| of == file && *at == revision)
                .map_or_else(|| Arc::from(editor.text()), |(_, _, text)| Arc::clone(text));
            shared.push((*file, revision, Arc::clone(&text)));
            files.push((*file, text));
        }
        self.report.shared = shared;
        Kept {
            files,
            stage: self.stage(),
        }
    }

    /// `:report [words]` and `:reports [delete <name>]`. Returns whether
    /// `command` was one of them.
    pub(super) fn report_command(&mut self, command: &str) -> bool {
        let (name, argument) = command
            .split_once(' ')
            .map_or((command, ""), |(name, rest)| (name, rest.trim()));
        match name {
            "report" => {
                let note = argument.to_owned();
                if self.report.open.is_some() {
                    // Once the key that asked is over and recorded.
                    self.report.wanted = Some((note, false));
                } else {
                    self.write_report(note, false);
                }
                true
            }
            "reports" => {
                match argument.split_once(' ') {
                    Some(("delete", name)) => self.delete_report(name.trim()),
                    _ if argument.is_empty() => self.list_reports(),
                    _ => self.alert("try :reports, or :reports delete <name>"),
                }
                true
            }
            _ => false,
        }
    }

    /// Where reports are kept.
    fn reports_dir(&self) -> Option<PathBuf> {
        self.state_dir.as_ref().map(|state| state.join(REPORTS))
    }

    /// Writes a report of what happened since the first key kept, with
    /// the user's `note`. `chord`: the report key asked, not a command.
    fn write_report(&mut self, note: String, chord: bool) {
        if self.report.writing.is_some() {
            return self.alert("a report is still being written: one at a time");
        }
        let Some(reports) = self.reports_dir() else {
            return self.alert("no report: there is no state directory to keep it in");
        };
        let first_time = !reports.exists();
        self.mark_report_keys(chord);
        let job = self.report_job(&reports, note);
        let first_word = if first_time {
            " — a report holds the text and what was typed; it stays on this machine and \
             is never sent anywhere (:reports lists them, and deletes them)"
        } else {
            " (:reports lists them)"
        };
        let background = self.report.wake.is_some() || self.report.worker.is_some();
        if !background {
            // No window: written on the spot, without a picture.
            return match job.run(None) {
                Ok(told) => self.notice(format!("{told}{first_word}")),
                Err(why) => self.alert(format!("the report could not be written: {why}")),
            };
        }
        if self.report.worker.is_none()
            && let Some(wake) = self.report.wake.take()
        {
            match Worker::spawn(wake) {
                Ok(worker) => self.report.worker = Some(worker),
                Err(err) => {
                    return self
                        .alert(format!("the report could not be written: no thread: {err}"));
                }
            }
        }
        let (picture, taken) = mpsc::channel();
        let jobs = (self.report.worker.as_ref()).and_then(|worker| worker.jobs.as_ref());
        if jobs.is_some_and(|jobs| jobs.send((job, taken)).is_ok()) {
            self.report.picture = Some(picture);
            // Said once the window is pictured: not in the picture.
            self.report.writing = Some(first_word.to_owned());
        } else {
            self.report.worker = None;
            self.alert("the report could not be written: the thread that writes them is gone");
        }
    }

    /// Marks the keys that asked for the report about to be written, so
    /// that a replay does not press them: the report key; or the last key
    /// and those before it that left something under way (the `g` of
    /// `g!`, `:report …` on the command line, a list it was chosen from).
    fn mark_report_keys(&mut self, chord: bool) {
        let mut keys = self
            .report
            .moments
            .iter_mut()
            .rev()
            .filter_map(|moment| match &mut moment.what {
                What::Key(keyed) => Some(keyed),
                _ => None,
            });
        let Some(last) = keys.next() else {
            return;
        };
        last.report = true;
        if chord {
            return;
        }
        for keyed in keys {
            if keyed.rest {
                break;
            }
            keyed.report = true;
        }
    }

    /// Gathers what goes into a report: what is quick to copy is copied
    /// here, the rest is done by the thread that writes it.
    fn report_job(&mut self, reports: &Path, note: String) -> Job {
        let now = SystemTime::now();
        let stamp = stamp(now);
        let mut directory = reports.join(&stamp);
        let mut again = 2;
        while directory.exists() {
            directory = reports.join(format!("{stamp}-{again}"));
            again += 1;
        }
        let root = self.project.root_dir().to_owned();
        let name = |file: FileId| file.vpath().get_without_slash().to_owned();
        let start = self.report.starts.front().cloned();
        let from = start
            .as_ref()
            .map_or(self.report.first, |start| start.moment);
        let moments: Vec<Moment> = self
            .report
            .moments
            .iter()
            .skip(usize::try_from(from.saturating_sub(self.report.first)).unwrap_or(0))
            .cloned()
            .collect();
        let mut between = Vec::new();
        let keys = keys_text(&moments, &name, &mut between);
        let before = start.as_ref().map(|start| {
            let files = start.kept.files.iter();
            files
                .map(|(file, text)| (name(*file), Arc::clone(text)))
                .collect()
        });
        let after: Vec<(String, Arc<str>)> = self
            .opened
            .iter()
            .filter_map(|file| {
                let editor = self
                    .editor
                    .iter()
                    .chain(&self.others)
                    .find(|e| e.file() == *file)?;
                Some((name(*file), Arc::from(editor.text())))
            })
            .collect();
        let open: HashSet<PathBuf> = self
            .editor
            .iter()
            .chain(&self.others)
            .map(|editor| editor.path().to_owned())
            .collect();
        let needs: Vec<PathBuf> = self
            .project
            .dependencies()
            .into_iter()
            .filter(|path| !open.contains(path))
            .collect();
        let state = self.state_text(now, &note, start.as_ref().map(|start| &*start.kept), &name);
        Job {
            directory,
            root,
            note,
            keys,
            before,
            memory: start.as_ref().map(|start| start.kept.stage.memory.clone()),
            between,
            after,
            needs,
            state,
            config: self.report.settings.config.clone(),
            document: self.document.clone(),
        }
    }

    /// What `state.txt` says, but for the fonts the document uses (the
    /// thread looks for those).
    fn state_text(
        &self,
        now: SystemTime,
        note: &str,
        start: Option<&Kept>,
        name: &dyn Fn(FileId) -> String,
    ) -> String {
        let mut text = String::new();
        let yes = |on: bool| if on { "yes" } else { "no" };
        let _ = writeln!(text, "zetwerk report, written {} (UTC)", stamp(now));
        let _ = writeln!(text, "note: {note}");
        let _ = writeln!(
            text,
            "built: {}; Typst {}",
            self.report.settings.built,
            typst::utils::version().raw()
        );
        let _ = writeln!(
            text,
            "window: {}x{} device pixels, display scale {}, ui_scale {}",
            self.window.0, self.window.1, self.display, self.ui
        );
        let _ = writeln!(
            text,
            "view: zoom {} (fits the width: {}), top {} pt, centre {} pt",
            self.view.zoom,
            yes(self.fit),
            self.view.top,
            self.view.center
        );
        let _ = writeln!(
            text,
            "layout: {:?}, shown: {:?}, {:?}",
            self.panes,
            self.shown_panes(),
            self.paging
        );
        let _ = writeln!(
            text,
            "aids: line numbers {}, parts typeset alone {}, performance panel {}, \
             what is up {:?}",
            yes(self.numbers),
            yes(self.in_parts),
            yes(self.performance_panel),
            PANELS
                .iter()
                .filter(|(bit, _)| self.panels() & bit != 0)
                .map(|(_, name)| *name)
                .collect::<Vec<_>>()
        );
        let _ = writeln!(text, "root: {}", self.project.root_dir().display());
        let _ = writeln!(text, "main: {}", name(self.project.main()));
        if let Some(editor) = &self.editor {
            let (line, column) = editor.position();
            let _ = writeln!(
                text,
                "now: {} at line {line}, column {column} (byte {}), {:?} mode",
                name(editor.file()),
                editor.cursor(),
                editor.mode()
            );
        }
        for editor in self.editor.iter().chain(&self.others) {
            let _ = writeln!(
                text,
                "open: {}{}",
                name(editor.file()),
                if editor.is_modified() {
                    " (changes not written)"
                } else {
                    ""
                }
            );
        }
        let _ = writeln!(
            text,
            "revision: {}, compiled: {:?}, pages of: {:?}",
            self.revision, self.compiled_revision, self.document_revision
        );
        let pages = self
            .document
            .as_ref()
            .map_or(0, |document| document.pages().len());
        let _ = writeln!(text, "pages: {pages}");
        if self.problems.is_empty() {
            let _ = writeln!(text, "problems: none");
        }
        for problem in &self.problems {
            let _ = writeln!(text, "problem: {problem}");
        }
        let _ = writeln!(text, "stats: {:?}", self.stats);
        let kept = (self.report.starts.front()).map_or(0, |first| self.report.keyed - first.keyed);
        let _ = writeln!(
            text,
            "recording: {kept} keys kept, {} asked for ([report] keys)",
            self.report.settings.keys
        );
        if let Some(font_set) = self.report.font_set {
            let _ = writeln!(text, "font set: {font_set:?}");
        }
        if let Some(start) = start {
            text.push_str("\nThe stage at the first key, for a replay:\n");
            start.stage.write(&mut text);
        }
        text
    }

    /// Takes up what the thread that writes a report has to say, and
    /// gives it the picture of the window it waits for. Costs nothing
    /// where no report is under way; the window calls it after every
    /// frame. Also records what changed since the last moment.
    pub fn report_frame(
        &mut self,
        gpu: &mut Gpu,
        config: &wgpu::SurfaceConfiguration,
        quads: &[Quad],
    ) {
        if let Some(picture) = self.report.picture.take() {
            let size = (config.width, config.height);
            let taken = gpu
                .capture(size, quads)
                .map(|rgba| (size.0, size.1, rgba))
                .map_err(|err| err.to_string());
            let _ = picture.send(taken);
            self.notice("writing a report …");
        }
        if self.report.writing.is_some() {
            let said = (self.report.worker.as_ref()).map(|worker| worker.done.try_recv());
            match said {
                Some(Err(TryRecvError::Empty)) => {}
                Some(Ok(Ok(told))) => {
                    let first_word = self.report.writing.take().unwrap_or_default();
                    self.notice(format!("{told}{first_word}"));
                }
                Some(Ok(Err(why))) => {
                    self.report.writing = None;
                    self.alert(format!("the report could not be written: {why}"));
                }
                Some(Err(TryRecvError::Disconnected)) | None => {
                    self.report.writing = None;
                    self.report.worker = None;
                    self.alert(
                        "the report could not be written: the thread that writes them is gone",
                    );
                }
            }
        }
        if self.report.on() && self.report.open.is_none() {
            self.report_changes(Instant::now());
        }
    }

    /// `:reports`: the reports there are, newest first, with their notes;
    /// Enter on one asks whether to delete it.
    fn list_reports(&mut self) {
        let Some(reports) = self.reports_dir() else {
            return self.alert("there is no state directory, and so no reports");
        };
        let mut names: Vec<String> = std::fs::read_dir(&reports)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        if names.is_empty() {
            return self.notice("no reports: g!, Ctrl-G or :report writes one");
        }
        names.sort_unstable_by(|a, b| b.cmp(a));
        let mut items = vec![(
            format!(
                "in {} — they stay on this machine; Enter on one deletes it",
                reports.display()
            ),
            Target::Nothing,
        )];
        for name in names {
            let note =
                std::fs::read_to_string(reports.join(&name).join("note.txt")).unwrap_or_default();
            let note = note.lines().next().unwrap_or("").trim();
            let label = if note.is_empty() {
                name.clone()
            } else {
                format!("{name}  {note}")
            };
            items.push((label, Target::Ask(format!("reports delete {name}"))));
        }
        self.open_picker("reports", items, 1);
    }

    /// `:reports delete <name>`.
    fn delete_report(&mut self, name: &str) {
        let Some(reports) = self.reports_dir() else {
            return self.alert("there is no state directory, and so no reports");
        };
        let plain = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            && !name.starts_with('.');
        let directory = reports.join(name);
        if !plain || !directory.is_dir() {
            return self.alert(format!("there is no report {name} (:reports lists them)"));
        }
        match std::fs::remove_dir_all(&directory) {
            Ok(()) => self.notice(format!("report {name} deleted")),
            Err(err) => self.alert(format!("report {name} not deleted: {err}")),
        }
    }
}

/// Everything a report is made of, to be written by a thread of its own.
struct Job {
    directory: PathBuf,
    /// The document's directory, which the names of files are of.
    root: PathBuf,
    note: String,
    keys: String,
    /// The open files at the first key, if a key was kept.
    before: Option<Vec<(String, Arc<str>)>>,
    /// What Vim remembered then (see `Engine::memory`).
    memory: Option<String>,
    /// Texts put in on the way: the number of the moment, and the files.
    between: Vec<(usize, Files)>,
    after: Files,
    needs: Vec<PathBuf>,
    state: String,
    config: Option<String>,
    document: Option<Arc<PagedDocument>>,
}

impl Job {
    /// Writes the report, with the picture of the window if `picture`
    /// brings one. Says where it went.
    fn run(self, picture: Option<&PictureTaken>) -> Result<String, String> {
        let directory = &self.directory;
        let private = |path: &Path| {
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            builder
                .create(path)
                .map_err(|err| format!("cannot make {}: {err}", path.display()))
        };
        let write = |path: &Path, bytes: &[u8]| {
            if let Some(parent) = path.parent() {
                private(parent)?;
            }
            std::fs::write(path, bytes)
                .map_err(|err| format!("cannot write {}: {err}", path.display()))
        };
        // Only names that stay inside the report.
        let inside = |base: &Path, name: &str| -> Option<PathBuf> {
            let relative = Path::new(name);
            relative
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
                .then(|| base.join(relative))
        };
        let write_files = |base: &Path, files: &[(String, Arc<str>)]| -> Result<(), String> {
            for (name, text) in files {
                if let Some(path) = inside(base, name) {
                    write(&path, text.as_bytes())?;
                }
            }
            Ok(())
        };
        private(directory)?;
        write(
            &directory.join("note.txt"),
            format!("{}\n", self.note).as_bytes(),
        )?;
        write(&directory.join("keys.txt"), self.keys.as_bytes())?;
        if let Some(before) = &self.before {
            write_files(&directory.join("before"), before)?;
        }
        if let Some(memory) = &self.memory {
            write(&directory.join("vim.txt"), memory.as_bytes())?;
        }
        for (number, files) in &self.between {
            write_files(&directory.join("between").join(number.to_string()), files)?;
        }
        write_files(&directory.join("after"), &self.after)?;
        let mut needs = String::new();
        for path in &self.needs {
            let shown = path.strip_prefix(&self.root).unwrap_or(path);
            let _ = writeln!(needs, "{}", shown.display());
        }
        write(&directory.join("needs.txt"), needs.as_bytes())?;
        let mut state = self.state;
        let fonts = self.document.as_deref().map(fonts_of).unwrap_or_default();
        state.push_str("\nFonts the pages use:\n");
        for font in fonts {
            let _ = writeln!(state, "font: {font}");
        }
        // Last: everything behind the mark is the file.
        if let Some(config) = &self.config {
            state.push_str("\nThe settings in force (config.toml):\n");
            state.push_str(replay::CONFIG_MARK);
            state.push('\n');
            state.push_str(config);
        }
        write(&directory.join("state.txt"), state.as_bytes())?;
        let pictured = match picture.map(|taken| taken.recv_timeout(PICTURE_PATIENCE)) {
            None => Err("no window".to_owned()),
            Some(Ok(Ok((width, height, rgba)))) => tiny_skia::IntSize::from_wh(width, height)
                .and_then(|size| tiny_skia::Pixmap::from_vec(rgba, size))
                .ok_or_else(|| "the picture has no pixels".to_owned())
                .and_then(|pixmap| pixmap.encode_png().map_err(|err| err.to_string()))
                .and_then(|png| write(&directory.join("window.png"), &png)),
            Some(Ok(Err(why))) => Err(why),
            Some(Err(_)) => Err("the window gave no picture".to_owned()),
        };
        let without = match pictured {
            Ok(()) => String::new(),
            Err(why) => format!(" (no picture: {why})"),
        };
        Ok(format!(
            "report written to {}{without}",
            directory.display()
        ))
    }
}

/// The fonts the pages of `document` use, each once.
fn fonts_of(document: &PagedDocument) -> Vec<String> {
    fn walk(frame: &Frame, found: &mut Vec<typst::text::Font>) {
        for (_, item) in frame.items() {
            match item {
                FrameItem::Group(group) => walk(&group.frame, found),
                FrameItem::Text(text) => {
                    let font = text.font.font();
                    if !found.contains(font) {
                        found.push(font.clone());
                    }
                }
                _ => {}
            }
        }
    }
    let mut found = Vec::new();
    for page in document.pages() {
        walk(&page.frame, &mut found);
    }
    let mut fonts: Vec<String> = found
        .iter()
        .map(|font| {
            let info = font.info();
            format!("{} {:?}", info.family, info.variant)
        })
        .collect();
    fonts.sort_unstable();
    fonts
}

/// The moments as `keys.txt` has them; the texts set on the way go to
/// `between`, numbered.
#[allow(clippy::too_many_lines, reason = "a line for each thing that happens")]
fn keys_text(
    moments: &[Moment],
    name: &dyn Fn(FileId) -> String,
    between: &mut Vec<(usize, Files)>,
) -> String {
    let mut text = String::new();
    let keys: String = moments
        .iter()
        .filter_map(|moment| match &moment.what {
            What::Key(keyed) if !keyed.report && !keyed.hidden => Some(notation(keyed.press)),
            _ => None,
        })
        .collect();
    text.push_str(
        "# zetwerk report: what happened, from the first key kept. Replay it with\n\
         # `zetwerk --replay <this directory>`. Each line: seconds since the first\n\
         # moment, + seconds since the one before, what happened. A key is written\n\
         # as --keys takes it; behind it what the window did: the mode, the cursor\n\
         # (@byte of the file), the revision of the text, how the page showed it.\n\
         # report-key: a key that asked for a report; not played again.\n",
    );
    let _ = writeln!(text, "# all keys: {keys}");
    let Some(first) = moments.first().map(|moment| moment.at) else {
        return text;
    };
    let mut last = first;
    let mut file = None;
    for (index, moment) in moments.iter().enumerate() {
        let time = moment.at.saturating_duration_since(first).as_secs_f64();
        let gap = moment.at.saturating_duration_since(last).as_secs_f64();
        last = moment.at;
        let _ = write!(text, "{time:9.3} {:>+8}  ", format!("+{gap:.3}"));
        match &moment.what {
            What::Key(keyed) => {
                let kind = if keyed.hidden {
                    "hidden-key"
                } else if keyed.report {
                    "report-key"
                } else {
                    "key"
                };
                let written = if keyed.hidden {
                    "*".to_owned()
                } else {
                    notation(keyed.press)
                };
                let _ = write!(
                    text,
                    "{kind} {written:<6} · {} @{} r{}",
                    mode_name(keyed.mode),
                    keyed.cursor,
                    keyed.revision
                );
                if keyed.file != file {
                    file = keyed.file;
                    if let Some(file) = keyed.file {
                        let _ = write!(text, " in {}", name(file));
                    }
                }
                match keyed.shown {
                    Shown::Nothing => {}
                    Shown::Predicted => text.push_str(" · predicted"),
                    Shown::Part => text.push_str(" · typeset in part"),
                    Shown::Waited => {
                        let _ = write!(
                            text,
                            " · waits for a compile: {}",
                            keyed.declined.as_deref().unwrap_or("why is not known")
                        );
                    }
                }
                let reached = &keyed.reached[..usize::from(keyed.reached_count).min(4)];
                if !keyed.hidden && reached != [keyed.press] {
                    if keyed.reached_count == 0 {
                        text.push_str(" · waits: it may be the beginning of a mapping");
                    } else {
                        let keys: String = reached.iter().map(|press| notation(*press)).collect();
                        let more = if keyed.reached_count > 4 { "…" } else { "" };
                        let _ = write!(text, " · mapped: the editor got {keys}{more}");
                    }
                }
            }
            What::Began(revision) => {
                let _ = write!(text, "compile r{revision}");
            }
            What::Compiled {
                revision,
                problem,
                millis,
            } => match problem {
                None => {
                    let _ = write!(text, "compiled r{revision} ok {millis:.0} ms");
                }
                Some(problem) => {
                    let _ = write!(
                        text,
                        "compiled r{revision} refused {millis:.0} ms: {}",
                        one_line(problem)
                    );
                }
            },
            What::Said { text: said, alert } => {
                let kind = if *alert { "alerts" } else { "says" };
                let _ = write!(text, "{kind} {}", one_line(said));
            }
            What::Opened(panel) => {
                let _ = write!(text, "opens {panel}");
            }
            What::Closed(panel) => {
                let _ = write!(text, "closes {panel}");
            }
            What::Scrolled { top, zoom } => {
                let _ = write!(text, "view top {top:.1} pt zoom {zoom:.3}");
            }
            What::Window { size, display, ui } => {
                let _ = write!(
                    text,
                    "window {}x{} display scale {display} ui {ui}",
                    size.0, size.1
                );
            }
            What::Layout(panes, paging) => {
                let _ = write!(text, "layout {panes:?} {paging:?}");
            }
            What::Disk(path) => {
                let _ = write!(text, "disk changed {}", path.display());
            }
            What::Agent(tool) => {
                let _ = write!(text, "agent {tool}");
            }
            What::TextSet(kept) => {
                let files = kept.files.iter();
                between.push((
                    index,
                    files
                        .map(|(file, text)| (name(*file), Arc::clone(text)))
                        .collect(),
                ));
                let _ = write!(
                    text,
                    "text-set {index} @{} (the texts changed between keys: between/{index}/)",
                    kept.stage.cursor
                );
            }
            What::FileOpened(file, opened) => {
                between.push((index, vec![(name(*file), Arc::clone(opened))]));
                let _ = write!(
                    text,
                    "opened {index} {} (as it was opened: between/{index}/)",
                    name(*file)
                );
            }
            What::Moved(cursor) => {
                let _ = write!(text, "cursor @{cursor} (put there without a key)");
            }
        }
        text.push('\n');
    }
    text
}

/// `text` on one line.
fn one_line(text: &str) -> String {
    text.replace('\n', " ⏎ ")
}

/// How a mode is written in `keys.txt`.
fn mode_name(mode: editor::Mode) -> &'static str {
    match mode {
        editor::Mode::Normal => "normal",
        editor::Mode::Insert => "insert",
        editor::Mode::Visual => "visual",
        editor::Mode::VisualLine => "visual-line",
        editor::Mode::Command => "command",
    }
}

/// A key in the notation `--keys` takes (see [`crate::keys::parse`]).
#[must_use]
pub fn notation(press: KeyPress) -> String {
    let special = match press.key {
        Key::Char(' ') => Some("space".to_owned()),
        Key::Char('<') => Some("lt".to_owned()),
        Key::Char(c) if c.is_whitespace() || c.is_control() => Some(c.to_string()),
        Key::Char(c) if press.ctrl || press.alt => Some(c.to_string()),
        Key::Char(_) => None,
        Key::Enter => Some("CR".to_owned()),
        Key::Escape => Some("Esc".to_owned()),
        Key::Backspace => Some("BS".to_owned()),
        Key::Delete => Some("Del".to_owned()),
        Key::Tab => Some("Tab".to_owned()),
        Key::Up => Some("Up".to_owned()),
        Key::Down => Some("Down".to_owned()),
        Key::Left => Some("Left".to_owned()),
        Key::Right => Some("Right".to_owned()),
        Key::PageUp => Some("PageUp".to_owned()),
        Key::PageDown => Some("PageDown".to_owned()),
        Key::Home => Some("Home".to_owned()),
        Key::End => Some("End".to_owned()),
        Key::Text(number) => Some(format!("Text{number}")),
    };
    match (special, press.key) {
        (None, Key::Char(c)) => c.to_string(),
        (special, _) => {
            let mut written = String::from("<");
            if press.ctrl {
                written.push_str("C-");
            }
            if press.alt {
                written.push_str("A-");
            }
            written.push_str(special.as_deref().unwrap_or(""));
            written.push('>');
            written
        }
    }
}

/// The time `now` as a name: `2026-11-15T14-03-22`, in UTC.
fn stamp(now: SystemTime) -> String {
    let seconds = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    // Howard Hinnant's days from the civil calendar, backwards.
    let z = i64::try_from(days).unwrap_or(0) + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}-{:02}-{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_written_as_keys_takes_them() {
        let all = "ab é|<lt><space><CR><Esc><BS><Del><Tab><Up><Down><Left><Right>\
                   <PageUp><PageDown><Home><End><C-g><C-A-x><A-Left><C-lt><C-space>\u{a0}";
        let keys = crate::keys::parse(all).unwrap_or_default();
        assert_eq!(keys.len(), 26);
        let written: String = keys.iter().map(|press| notation(*press)).collect();
        assert_eq!(crate::keys::parse(&written), Ok(keys));
        // No blank, tab or line break: a line of keys.txt is read so.
        assert!(!written.contains([' ', '\t', '\n']));
    }

    #[test]
    fn a_time_is_a_name() {
        let at = |seconds| SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
        assert_eq!(stamp(at(0)), "1970-01-01T00-00-00");
        assert_eq!(stamp(at(951_782_400)), "2000-02-29T00-00-00");
        assert_eq!(stamp(at(1_794_750_202)), "2026-11-15T13-43-22");
    }

    #[test]
    fn the_ring_keeps_enough_keys_and_no_more() {
        let mut recorder = Recorder::new(Settings {
            keys: 20,
            ..Settings::default()
        });
        let kept = Arc::new(Kept {
            files: Vec::new(),
            stage: replay::Stage::default(),
        });
        let now = Instant::now();
        for key in 0..1000_u64 {
            // A start every so often, as when nothing is under way.
            if recorder.starts.is_empty()
                || recorder.keyed - recorder.starts[recorder.starts.len() - 1].keyed
                    >= recorder.every()
            {
                let moment = recorder.first + recorder.moments.len() as u64;
                recorder.starts.push_back(Start {
                    moment,
                    keyed: recorder.keyed,
                    kept: Arc::clone(&kept),
                });
            }
            recorder.push(now, What::Began(key));
            recorder.push(
                now,
                What::Key(Keyed {
                    press: KeyPress::plain(Key::Char('x')),
                    reached: [KeyPress::plain(Key::Char('x')); 4],
                    reached_count: 1,
                    mode: editor::Mode::Normal,
                    rest: true,
                    file: None,
                    cursor: 0,
                    revision: key,
                    shown: Shown::Nothing,
                    declined: None,
                    report: false,
                    hidden: false,
                }),
            );
            recorder.keyed += 1;
            recorder.trim();
            let keys = recorder
                .moments
                .iter()
                .filter(|moment| matches!(moment.what, What::Key(_)))
                .count();
            // From the oldest start: at least as many keys as asked, once
            // there were as many, and never many more.
            let start = &recorder.starts[0];
            assert_eq!(start.moment, recorder.first);
            assert!(keys >= 20.min(key as usize + 1), "{keys} keys after {key}");
            assert!(keys <= 20 + 8, "{keys} keys after {key}");
        }
    }
}
