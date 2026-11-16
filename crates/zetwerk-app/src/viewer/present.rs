//! Giving the talk from the editor.
//!
//! `:present` (or `gP`, or F5 as presentation remotes send it) takes the
//! whole screen for the slide the cursor is on: the window goes
//! fullscreen on the display it is on, and nothing of the editor is
//! drawn, only the slide, as large as it goes, on black, rasterized for
//! the pixels the screen has. Leaving (`q` or Escape) brings the editor
//! back with the cursor on the slide the talk ended on.
//!
//! Keys, none of which needs the mouse:
//!
//! - next: Space, Enter, `j`, `l`, `n`, Right, Down, `PageDown` (and a
//!   click, a touch, the wheel); previous: Backspace, `k`, `h`, `N`,
//!   Left, Up, `PageUp`. They go page by page, so through the steps of an
//!   animated slide; the page before the first step of a slide is the
//!   last step of the slide before.
//! - first, last: `gg` or Home, `G` or End; a slide by its number: the
//!   number and then Enter or `G` (numbers mean slides, not pages, where
//!   the deck says which pages make a slide: see [`super::deck`]).
//! - back to where the last jump came from: `` ` ``, `'` or Ctrl-O.
//! - blank the screen: `b` or `.` black, `w` or `,` white; any key brings
//!   the slide back.
//! - `o` or Tab: the slides side by side, the current one marked, to
//!   pick one (arrows or `hjkl`, Enter; Escape leaves it).
//! - `t`: the time of day and the time the talk has taken, in a corner.
//! - `p`: the presenter's view in place of the audience's (the slide,
//!   the next one, the notes, the clock, how far the talk is): for
//!   rehearsing, or for a window on the laptop's screen.
//! - `q`, Escape: back to the editor. F5 does nothing while presenting.
//!
//! What was decided:
//!
//! - **Nothing runs between keys.** The clock shows minutes, and the
//!   window is woken when a minute it shows goes by: the time of day's
//!   and the elapsed time's, at most twice a minute; with the clock
//!   hidden and the audience's view up, never.
//! - **Instant slides.** The slide shown and the [`AHEAD`] pages behind
//!   it are asked of the raster threads at the size they are shown, the
//!   one shown first; those behind are not asked for again. Until the
//!   pages of a new slide are all there, the last slide stays up whole:
//!   the audience never sees half a slide.
//! - **Not remembered.** Presenting is no state of the window: nothing
//!   of it is kept, and leaving restores the view as a jump to the slide
//!   would. Only the timing of the talk is kept (see below).
//! - **Rehearsing.** `:present rehearse` notes how long each slide was
//!   up; leaving lists them, with the running total, and Enter on one
//!   goes there. `:present 20min` (also `1h`, `1h30`, `45m`, `90s`, or
//!   a number of minutes) says how long the talk is to take: the
//!   presenter's view then says how much time is left and how far ahead
//!   or behind the talk is at the slide shown, against the last
//!   rehearsal scaled to that length (or, never rehearsed, against the
//!   same time for every slide). Both are kept per document in the
//!   state directory, beside the journal; `:present times` lists the
//!   last rehearsal again.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use typst_layout::PagedDocument;
use zetwerk_render::Tile;

use super::Viewer;
use super::assist::Target;
use super::deck::Deck;
use crate::editor;
use crate::gpu::Quad;
use crate::keys::{Key, KeyPress};

/// How many pages behind the one shown are made ready.
pub const AHEAD: usize = 2;
/// How far the wheel has to be turned, in logical pixels, before the
/// slide changes.
const WHEEL_STEP: f64 = 50.0;
/// The most planned tiles kept before they are planned anew.
const PLANNED_KEPT: usize = 256;
/// What the screen is blanked with.
pub(super) const BLACK: [u8; 3] = [0, 0, 0];
pub(super) const WHITE: [u8; 3] = [0xff, 0xff, 0xff];

/// What the user's settings say about talks (`[present]` in the
/// settings).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Talk {
    /// Whether the clock is shown in the audience's view from the start.
    pub clock: bool,
    /// How long a talk is to take where its document does not say.
    pub duration: Option<Duration>,
}

/// What presenting keeps.
#[derive(Default)]
pub(super) struct Present {
    /// The talk being given, while it is.
    pub(super) show: Option<Box<Show>>,
    /// The deck the last document typeset whole is, and that document.
    pub(super) deck: Option<(Arc<PagedDocument>, Arc<Deck>)>,
    /// What the window is to do about being fullscreen, until it asks.
    screen: Option<bool>,
    /// Whether it went fullscreen for the talk, and is to be a window
    /// again after it.
    took_screen: bool,
    pub(super) talk: Talk,
    /// The handout being written (see [`super::handout`]).
    pub(super) handouts: super::handout::Handouts,
}

/// A talk being given.
#[allow(
    clippy::struct_excessive_bools,
    reason = "switches of the talk's views, not states of one thing"
)]
pub(super) struct Show {
    /// The page shown.
    pub(super) page: usize,
    /// When the talk began, and when the page shown came up.
    pub(super) began: Instant,
    since: Instant,
    /// The digits of a slide's number typed so far.
    pub(super) number: String,
    /// The page shown before the last jump.
    back: Option<usize>,
    /// The colour the screen is blanked with, while it is.
    pub(super) blank: Option<[u8; 3]>,
    /// The slide picked out in the overview, while it is up.
    pub(super) overview: Option<usize>,
    /// Whether the clock is shown in the audience's view.
    pub(super) clock: bool,
    /// Whether the presenter's view is up instead of the audience's.
    pub(super) presenter: bool,
    /// Whether the audience's view may go to a screen of its own, and
    /// whether it is in a window of its own there.
    two: bool,
    pub(super) apart: bool,
    /// A `g` waiting for another.
    pending_g: bool,
    /// How far the wheel was turned towards the next slide.
    pushed: f64,
    /// When the clock next shows another minute, where it is shown.
    pub(super) tick: Option<Instant>,
    /// How long each slide was up, while rehearsing.
    rehearsal: Option<Vec<Duration>>,
    /// How long the talk is to take, and how long each slide took when
    /// it was rehearsed last.
    pub(super) plan: Plan,
    /// The audience's view as last drawn whole: the page and its quads.
    pub(super) drawn: Option<(usize, Vec<Quad>)>,
    /// The tiles of pages at a zoom, planned once.
    pub(super) planned: HashMap<(usize, u64), Vec<Tile>>,
    /// The document those tiles are of.
    pub(super) planned_of: Option<Arc<PagedDocument>>,
}

/// How long a talk is to take.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub duration: Option<Duration>,
    /// Each slide as it was rehearsed last: its label and its time.
    pub rehearsed: Vec<(String, Duration)>,
}

impl Plan {
    /// When slide `slide` of `count` is to come up, and when it is to be
    /// over, from the beginning of the talk; `None` without a duration.
    #[must_use]
    pub fn window(&self, slide: usize, count: usize) -> Option<(Duration, Duration)> {
        let duration = self.duration?;
        let total: Duration = self.rehearsed.iter().map(|(_, took)| *took).sum();
        if self.rehearsed.len() == count && !total.is_zero() {
            let share = duration.as_secs_f64() / total.as_secs_f64();
            let before: Duration = self.rehearsed[..slide].iter().map(|(_, took)| *took).sum();
            let took = self
                .rehearsed
                .get(slide)
                .map_or(Duration::ZERO, |(_, took)| *took);
            return Some((before.mul_f64(share), (before + took).mul_f64(share)));
        }
        let each = duration.checked_div(u32::try_from(count.max(1)).unwrap_or(u32::MAX))?;
        let slide = u32::try_from(slide).unwrap_or(u32::MAX);
        Some((each * slide, each * slide.saturating_add(1)))
    }
}

/// How far ahead (negative) or behind (positive) a talk that has taken
/// `elapsed` is at a slide that is to be up `from` `to`: nothing while it
/// is on time.
#[must_use]
pub fn lag((from, to): (Duration, Duration), elapsed: Duration) -> Option<(bool, Duration)> {
    if elapsed < from {
        Some((false, from.saturating_sub(elapsed)))
    } else if elapsed > to {
        Some((true, elapsed.saturating_sub(to)))
    } else {
        None
    }
}

/// When the clock next shows another minute after `now`: of the time of
/// day (`wall`) or of the time since `began`, whichever comes first.
#[must_use]
pub fn next_tick(now: Instant, began: Instant, wall: SystemTime) -> Instant {
    let minute = Duration::from_mins(1);
    let rest = |gone: Duration| {
        minute.saturating_sub(Duration::from_nanos(
            (gone.as_nanos() % minute.as_nanos()) as u64,
        ))
    };
    let elapsed = now.saturating_duration_since(began);
    let of_day = wall.duration_since(UNIX_EPOCH).unwrap_or_default();
    now + rest(elapsed).min(rest(of_day))
}

/// `duration` as a clock shows it: `m:ss`, or `h:mm:ss` from an hour.
#[must_use]
pub fn clock(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// `duration` in whole minutes, as the live clock and the presenter's
/// view say it: `12 min`, `1 h 05 min`.
#[must_use]
pub fn minutes(duration: Duration) -> String {
    let minutes = duration.as_secs() / 60;
    if minutes >= 60 {
        format!("{} h {:02} min", minutes / 60, minutes % 60)
    } else {
        format!("{minutes} min")
    }
}

/// Reads how long a talk is to take: `20min`, `20m`, `20`, `1h`, `1h30`,
/// `1h30m`, `90s`.
#[must_use]
pub fn parse_duration(text: &str) -> Option<Duration> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if text.bytes().all(|byte| byte.is_ascii_digit()) {
        let minutes: u64 = text.parse().ok()?;
        return (minutes > 0).then(|| Duration::from_mins(minutes));
    }
    let mut total = 0u64;
    let mut rest = text;
    let mut last_unit = None;
    while !rest.is_empty() {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        let number: u64 = rest[..digits].parse().ok()?;
        rest = &rest[digits..];
        let unit = rest.bytes().take_while(u8::is_ascii_alphabetic).count();
        let (seconds, behind) = match &rest[..unit] {
            "h" | "hr" | "hour" | "hours" => (3600, Some(60)),
            "m" | "min" | "mins" | "minute" | "minutes" => (60, Some(1)),
            "s" | "sec" | "secs" | "second" | "seconds" => (1, None),
            // A number behind another without a unit: the next smaller.
            "" => (last_unit?, None),
            _ => return None,
        };
        total = total.checked_add(number.checked_mul(seconds)?)?;
        last_unit = behind;
        rest = rest[unit..].trim_start();
    }
    (total > 0).then(|| Duration::from_secs(total))
}

/// Where the timing of the talk of the document at `path` is kept under
/// `state_dir`.
fn store_path(state_dir: &Path, path: &Path) -> PathBuf {
    // FNV-1a, as the notes are named: the same in every run.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    state_dir.join("talks").join(format!("{hash:016x}.talk"))
}

/// The plan written as it is kept: a line for the duration, a line for
/// each slide rehearsed.
#[must_use]
pub fn written(plan: &Plan) -> String {
    let mut text = String::from("zetwerk talk 1\n");
    if let Some(duration) = plan.duration {
        let _ = writeln!(text, "duration\t{}", duration.as_secs());
    }
    for (label, took) in &plan.rehearsed {
        let label = label.replace(['\t', '\n'], " ");
        let _ = writeln!(text, "slide\t{label}\t{}", took.as_millis());
    }
    text
}

/// The plan read back from what [`written`] wrote; what cannot be read
/// is left out.
#[must_use]
pub fn read(text: &str) -> Plan {
    let mut plan = Plan::default();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.as_slice() {
            ["duration", seconds] => {
                plan.duration = seconds.parse().ok().map(Duration::from_secs);
            }
            ["slide", label, millis] => {
                if let Ok(millis) = millis.parse() {
                    plan.rehearsed
                        .push(((*label).to_owned(), Duration::from_millis(millis)));
                }
            }
            _ => {}
        }
    }
    plan
}

impl Viewer {
    /// Whether a talk is being given.
    #[must_use]
    pub fn is_presenting(&self) -> bool {
        self.present.show.is_some()
    }

    /// Whether the presenter's view is up.
    #[must_use]
    pub fn is_presenter_view(&self) -> bool {
        self.present
            .show
            .as_ref()
            .is_some_and(|show| show.presenter)
    }

    /// The page a talk shows, counting from 0.
    #[must_use]
    pub fn presented_page(&self) -> Option<usize> {
        self.present.show.as_ref().map(|show| show.page)
    }

    /// What the window is to do for a talk, if anything has changed:
    /// whether one is given now, and whether to change being fullscreen
    /// for it: to go fullscreen as it begins, and back to a window as it
    /// ends where it went fullscreen for it.
    pub fn take_screen_wish(&mut self) -> Option<(bool, bool)> {
        let presenting = self.present.screen.take()?;
        let change = presenting || std::mem::take(&mut self.present.took_screen);
        Some((presenting, change))
    }

    /// The window was fullscreen already when the talk began: it stays
    /// so after it.
    pub fn screen_was_full(&mut self) {
        self.present.took_screen = false;
    }

    /// Whether the talk may have the audience's view on a screen of its
    /// own (`:present here` keeps it in this window).
    #[must_use]
    pub fn wants_two_screens(&self) -> bool {
        self.present.show.as_ref().is_some_and(|show| show.two)
    }

    /// The audience's view is in a window of its own now, or no more:
    /// this window has the presenter's view while it is.
    pub fn set_audience_apart(&mut self, apart: bool) {
        if let Some(show) = &mut self.present.show {
            show.apart = apart;
            show.presenter = apart;
            show.drawn = None;
        }
    }

    /// Takes what the user's settings say about talks.
    pub fn set_talk(&mut self, talk: Talk) {
        self.present.talk = talk;
    }

    /// The plan of the talk of this document, as kept.
    fn kept_plan(&self) -> Plan {
        let (Some(state_dir), Some(path)) = (&self.state_dir, self.main_path()) else {
            return Plan::default();
        };
        std::fs::read_to_string(store_path(state_dir, &path))
            .map(|text| read(&text))
            .unwrap_or_default()
    }

    /// Keeps the plan of the talk of this document.
    fn keep_plan(&mut self, plan: &Plan) {
        let (Some(state_dir), Some(path)) = (&self.state_dir, self.main_path()) else {
            return;
        };
        let store = store_path(state_dir, &path);
        let made = store
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| zetwerk_core::write_atomically(&store, written(plan).as_bytes()));
        if let Err(err) = made {
            self.alert(format!("the timing of the talk is not kept: {err}"));
        }
    }

    /// `:present`, with what was typed behind it. Returns whether
    /// `command` was that.
    pub(super) fn present_command(&mut self, command: &str) -> bool {
        let Some(rest) = command.strip_prefix("present") else {
            return false;
        };
        if !(rest.is_empty() || rest.starts_with(' ')) {
            return false;
        }
        let (mut rehearse, mut duration, mut two) = (false, None, true);
        for word in rest.split_whitespace() {
            match word {
                "rehearse" | "rehearsal" => rehearse = true,
                "here" | "one" => two = false,
                "times" | "timing" => {
                    self.list_rehearsal(&self.kept_plan());
                    return true;
                }
                other => {
                    duration = parse_duration(other);
                    if duration.is_none() {
                        self.alert(format!(
                            "not something to present with: {other} (:present, :present \
                             rehearse, :present 20min, :present times)"
                        ));
                        return true;
                    }
                }
            }
        }
        self.begin_talk(rehearse, duration);
        if let Some(show) = &mut self.present.show {
            show.two = two;
        }
        true
    }

    /// Begins the talk on the slide the cursor is on.
    pub(super) fn begin_talk(&mut self, rehearse: bool, duration: Option<Duration>) {
        let Some(pages) = self
            .document
            .as_ref()
            .map(|document| document.pages().len())
        else {
            return self.notice("there are no pages to present yet");
        };
        if pages == 0 {
            return self.notice("there are no pages to present yet");
        }
        let mut plan = self.kept_plan();
        if let Some(duration) = duration {
            plan.duration = Some(duration);
            self.keep_plan(&plan);
        }
        if plan.duration.is_none() {
            plan.duration = self.present.talk.duration;
        }
        let page = match self.paging {
            super::Paging::Slides => self.slide,
            super::Paging::Scroll if self.editor.is_some() => self.cursor_page.get(),
            super::Paging::Scroll => self.view.current_page(&self.layout).unwrap_or(0),
        };
        let slides = self.deck().map_or(pages, |deck| deck.slides.len());
        let now = Instant::now();
        // Nothing of the editor stays up over the talk.
        self.grid = None;
        self.picker = None;
        self.completion = None;
        self.present.show = Some(Box::new(Show {
            page: page.min(pages - 1),
            began: now,
            since: now,
            number: String::new(),
            back: None,
            blank: None,
            overview: None,
            clock: self.present.talk.clock,
            presenter: false,
            two: true,
            apart: false,
            pending_g: false,
            pushed: 0.0,
            tick: None,
            rehearsal: rehearse.then(|| vec![Duration::ZERO; slides]),
            plan,
            drawn: None,
            planned: HashMap::new(),
            planned_of: None,
        }));
        self.present.screen = Some(true);
        self.present.took_screen = true;
    }

    /// Ends the talk: the editor is back, the cursor on the slide shown.
    pub(super) fn end_talk(&mut self) {
        let Some(mut show) = self.present.show.take() else {
            return;
        };
        self.present.screen = Some(false);
        self.note_time(&mut show, Instant::now());
        let page = show.page;
        self.goto_page(page);
        if let Some(spent) = show.rehearsal.take() {
            let labels = self
                .deck()
                .map(|deck| deck.labels.clone())
                .unwrap_or_default();
            let rehearsed: Vec<(String, Duration)> = spent
                .into_iter()
                .enumerate()
                .map(|(index, took)| {
                    let label = labels
                        .get(index)
                        .cloned()
                        .unwrap_or_else(|| (index + 1).to_string());
                    (label, took)
                })
                .collect();
            let plan = Plan {
                duration: show.plan.duration,
                rehearsed,
            };
            self.keep_plan(&plan);
            self.list_rehearsal(&plan);
        }
    }

    /// Adds the time the page shown has been up to its slide's, while
    /// rehearsing, and starts counting anew.
    fn note_time(&mut self, show: &mut Show, now: Instant) {
        let slide = self
            .deck()
            .map_or(show.page, |deck| deck.slide_of(show.page));
        if let Some(spent) = &mut show.rehearsal
            && let Some(slot) = spent.get_mut(slide)
        {
            *slot += now.saturating_duration_since(show.since);
        }
        show.since = now;
    }

    /// Lists how long each slide took when the talk was rehearsed, with
    /// the running total; Enter goes to the slide.
    fn list_rehearsal(&mut self, plan: &Plan) {
        if plan.rehearsed.is_empty() {
            return self.notice("the talk has not been rehearsed: :present rehearse");
        }
        let deck = self.deck();
        let count = plan.rehearsed.len();
        let mut total = Duration::ZERO;
        let mut items = Vec::new();
        for (index, (label, took)) in plan.rehearsed.iter().enumerate() {
            total += *took;
            let mut row = format!(
                "slide {label:>3}   {:>7}   total {:>7}",
                clock(*took),
                clock(total)
            );
            // Against the same share of the planned length for each.
            if let Some(duration) = plan.duration {
                let due = duration.mul_f64((index + 1) as f64 / count as f64);
                match lag((due, due), total) {
                    Some((true, by)) => {
                        let _ = write!(row, "   {} over the plan", clock(by));
                    }
                    Some((false, by)) => {
                        let _ = write!(row, "   {} under the plan", clock(by));
                    }
                    None => {}
                }
            }
            let page = deck
                .as_ref()
                .and_then(|deck| deck.slides.get(index))
                .map_or(index, |pages| pages.start);
            items.push((row, Target::Command((page + 1).to_string())));
        }
        let length = plan
            .duration
            .map(|duration| format!(" of {} planned", clock(duration)))
            .unwrap_or_default();
        self.open_picker("slides as rehearsed (Enter: go there)", items, 0);
        self.notice(format!("the talk took {}{length}", clock(total)));
    }

    /// A key while a talk is given, or F5 to give one. Returns whether
    /// the key was the talk's.
    pub(super) fn present_key(&mut self, press: KeyPress) -> bool {
        let Some(show) = &mut self.present.show else {
            let normal = self.editor.as_ref().is_none_or(|editor| {
                editor.mode() == editor::Mode::Normal && editor.command_line().is_none()
            });
            if press.key == Key::F(5) && normal && self.picker.is_none() {
                self.begin_talk(false, None);
                return true;
            }
            return false;
        };
        if show.blank.take().is_some() {
            return true;
        }
        if show.overview.is_some() {
            self.overview_key(press);
            return true;
        }
        let pending_g = std::mem::take(&mut show.pending_g);
        let number = std::mem::take(&mut show.number);
        let plain = !press.ctrl && !press.alt;
        match (press.key, plain) {
            (Key::Char(digit @ '0'..='9'), true) => {
                show.number = number;
                show.number.push(digit);
            }
            (Key::Enter | Key::Char('G'), true) if !number.is_empty() => {
                if let Ok(wanted) = number.parse::<usize>() {
                    self.talk_to_slide(wanted);
                }
            }
            (Key::Backspace, _) if !number.is_empty() => {
                show.number = number;
                show.number.pop();
            }
            (Key::Escape, _) if !number.is_empty() => {}
            (Key::Char('g'), true) if pending_g => self.talk_jump(0),
            (Key::Char('g'), true) => show.pending_g = true,
            (Key::Home, _) => self.talk_jump(0),
            (Key::Char('G'), true) | (Key::End, _) => self.talk_jump(usize::MAX),
            (Key::Char(' ' | 'j' | 'l' | 'n') | Key::Enter, true)
            | (Key::Right | Key::Down | Key::PageDown, _) => self.talk_step(true),
            (Key::Char('k' | 'h' | 'N'), true)
            | (Key::Backspace | Key::Left | Key::Up | Key::PageUp, _) => self.talk_step(false),
            (Key::Char('`' | '\''), true) | (Key::Char('o'), false) if press.ctrl || plain => {
                if let Some(back) = show.back {
                    self.talk_jump(back);
                }
            }
            (Key::Char('b' | '.'), true) => show.blank = Some(BLACK),
            (Key::Char('w' | ','), true) => show.blank = Some(WHITE),
            (Key::Char('o') | Key::Tab, true) => {
                let page = show.page;
                let slide = self.deck().map_or(page, |deck| deck.slide_of(page));
                if let Some(show) = &mut self.present.show {
                    show.overview = Some(slide);
                }
            }
            (Key::Char('t'), true) => show.clock = !show.clock,
            (Key::Char('p'), true) => {
                show.presenter = !show.presenter;
                show.drawn = None;
            }
            (Key::Char('q') | Key::Escape, _) | (Key::Char('c' | '['), false) => self.end_talk(),
            _ => {}
        }
        true
    }

    /// The page after the one shown, or the one before.
    fn talk_step(&mut self, forward: bool) {
        let Some(show) = &self.present.show else {
            return;
        };
        let page = if forward {
            show.page + 1
        } else {
            show.page.saturating_sub(1)
        };
        self.talk_show(page);
    }

    /// Shows page `page` (the last for one behind all) and notes where
    /// the jump came from.
    fn talk_jump(&mut self, page: usize) {
        if let Some(show) = &mut self.present.show {
            show.back = Some(show.page);
        }
        self.talk_show(page);
    }

    /// Shows slide `number` as the speaker means it, from its first step.
    fn talk_to_slide(&mut self, number: usize) {
        let Some(deck) = self.deck() else {
            return;
        };
        if let Some(slide) = deck.numbered(number) {
            self.talk_jump(deck.slides[slide].start);
        }
    }

    /// Shows page `page`, or the last there is.
    pub(super) fn talk_show(&mut self, page: usize) {
        let pages = self
            .document
            .as_ref()
            .map_or(0, |document| document.pages().len());
        let Some(mut show) = self.present.show.take() else {
            return;
        };
        let page = page.min(pages.saturating_sub(1));
        if page != show.page {
            self.note_time(&mut show, Instant::now());
            show.page = page;
        }
        self.present.show = Some(show);
    }

    /// A key while the overview of the talk's slides is up.
    fn overview_key(&mut self, press: KeyPress) {
        let Some(deck) = self.deck() else {
            return;
        };
        let columns = self.overview_columns();
        let count = deck.slides.len().max(1);
        let Some(show) = &mut self.present.show else {
            return;
        };
        let Some(chosen) = show.overview else {
            return;
        };
        let number = std::mem::take(&mut show.number);
        let chosen = match (press.key, press.ctrl) {
            (Key::Escape | Key::Tab | Key::Char('o' | 'q'), false)
            | (Key::Char('c' | '['), true) => {
                show.overview = None;
                return;
            }
            (Key::Char(digit @ '0'..='9'), false) => {
                show.number = number;
                show.number.push(digit);
                return;
            }
            (Key::Enter | Key::Char(' '), false) => {
                show.overview = None;
                let slide = number
                    .parse::<usize>()
                    .ok()
                    .and_then(|wanted| deck.numbered(wanted))
                    .unwrap_or(chosen);
                return self.talk_jump(deck.slides[slide.min(count - 1)].start);
            }
            (Key::Left | Key::Char('h'), false) => chosen.saturating_sub(1),
            (Key::Right | Key::Char('l'), false) => chosen + 1,
            (Key::Up | Key::Char('k'), false) => chosen.checked_sub(columns).unwrap_or(chosen),
            (Key::Down | Key::Char('j'), false) if chosen + columns < count => chosen + columns,
            (Key::Home | Key::Char('g'), false) => 0,
            (Key::End | Key::Char('G'), false) => count - 1,
            _ => chosen,
        };
        show.overview = Some(chosen.min(count - 1));
    }

    /// A click or a touch while a talk is given: the next page; in the
    /// overview, the slide under it. Returns whether a talk is given.
    pub fn present_click(&mut self, at: (f64, f64)) -> bool {
        let Some(show) = &mut self.present.show else {
            return false;
        };
        if show.blank.take().is_some() {
            return true;
        }
        if show.overview.is_some() {
            if let Some(slide) = self.overview_at(at)
                && let Some(deck) = self.deck()
                && let Some(pages) = deck.slides.get(slide)
            {
                if let Some(show) = &mut self.present.show {
                    show.overview = None;
                }
                self.talk_jump(pages.start);
            }
            return true;
        }
        self.talk_step(true);
        true
    }

    /// The wheel while a talk is given: enough of it, and the next page
    /// comes, or the one before. Returns whether a talk is given.
    pub(super) fn present_scroll(&mut self, dy: f64) -> bool {
        let step = WHEEL_STEP * self.scale;
        let Some(show) = &mut self.present.show else {
            return false;
        };
        if show.overview.is_some() || show.blank.is_some() {
            return true;
        }
        show.pushed += dy;
        if show.pushed.abs() >= step {
            let forward = show.pushed > 0.0;
            show.pushed = 0.0;
            self.talk_step(forward);
        }
        true
    }

    /// When the window is to be woken for the clock: when it next shows
    /// another minute, and only where it is shown.
    #[must_use]
    pub fn present_at(&self) -> Option<Instant> {
        self.present.show.as_ref().and_then(|show| show.tick)
    }

    /// Whether the clock shows another minute by `now`, so that the
    /// window is drawn again.
    pub fn present_due(&mut self, now: Instant) -> bool {
        let Some(show) = &mut self.present.show else {
            return false;
        };
        if show.tick.is_some_and(|tick| tick <= now) {
            show.tick = None;
            return true;
        }
        false
    }

    /// Plans tiles anew where the document changed, and keeps no more
    /// than [`PLANNED_KEPT`] pages' worth.
    pub(super) fn talk_planned_for(&mut self, document: &Arc<PagedDocument>) {
        let Some(show) = &mut self.present.show else {
            return;
        };
        let same = show
            .planned_of
            .as_ref()
            .is_some_and(|of| Arc::ptr_eq(of, document));
        if !same || show.planned.len() > PLANNED_KEPT {
            show.planned.clear();
            show.planned_of = Some(Arc::clone(document));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_are_read_as_people_write_them() {
        assert_eq!(parse_duration("20min"), Some(Duration::from_mins(20)));
        assert_eq!(parse_duration("20"), Some(Duration::from_mins(20)));
        assert_eq!(parse_duration("1h30"), Some(Duration::from_mins(90)));
        assert_eq!(parse_duration("1h30m"), Some(Duration::from_mins(90)));
        assert_eq!(parse_duration("45m"), Some(Duration::from_mins(45)));
        assert_eq!(parse_duration("90s"), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration("rehearse"), None);
        assert_eq!(parse_duration("0"), None);
        assert_eq!(clock(Duration::from_secs(65)), "1:05");
        assert_eq!(clock(Duration::from_secs(3725)), "1:02:05");
        assert_eq!(minutes(Duration::from_secs(3725)), "1 h 02 min");
        assert_eq!(minutes(Duration::from_secs(119)), "1 min");
    }

    #[test]
    fn the_plan_is_kept_as_written() {
        let plan = Plan {
            duration: Some(Duration::from_mins(20)),
            rehearsed: vec![
                ("1".to_owned(), Duration::from_millis(30_500)),
                ("2\tb".to_owned(), Duration::from_secs(90)),
            ],
        };
        let back = read(&written(&plan));
        assert_eq!(back.duration, plan.duration);
        assert_eq!(back.rehearsed[0], plan.rehearsed[0]);
        assert_eq!(back.rehearsed[1].0, "2 b");
        assert_eq!(read("nonsense\nslide\tx\ty\n"), Plan::default());
    }

    #[test]
    fn a_slide_is_due_as_rehearsed_scaled_to_the_length_of_the_talk() {
        let minute = Duration::from_mins(1);
        let mut plan = Plan {
            duration: Some(minute * 10),
            rehearsed: Vec::new(),
        };
        // Never rehearsed: the same for each.
        assert_eq!(plan.window(1, 5), Some((minute * 2, minute * 4)));
        // Rehearsed in 5 minutes, one slide taking 3 of them: in a talk
        // of 10 it is to take 6.
        plan.rehearsed = vec![
            ("1".to_owned(), minute),
            ("2".to_owned(), minute * 3),
            ("3".to_owned(), minute),
        ];
        assert_eq!(plan.window(1, 3), Some((minute * 2, minute * 8)));
        assert_eq!(lag((minute * 2, minute * 8), minute), Some((false, minute)));
        assert_eq!(lag((minute * 2, minute * 8), minute * 5), None);
        assert_eq!(
            lag((minute * 2, minute * 8), minute * 9),
            Some((true, minute))
        );
        assert_eq!(Plan::default().window(0, 3), None);
    }

    #[test]
    fn the_clock_wakes_the_window_when_a_minute_it_shows_goes_by() {
        let began = Instant::now();
        let wall = UNIX_EPOCH + Duration::from_secs(1_000_000 * 60 + 15);
        // Begun 10 s ago: its minute is over in 50 s, the time of day's
        // in 45.
        let now = began + Duration::from_secs(10);
        assert_eq!(next_tick(now, began, wall), now + Duration::from_secs(45));
        let now = began + Duration::from_secs(20);
        assert_eq!(next_tick(now, began, wall), now + Duration::from_secs(40));
        // Never more than a minute away, never now.
        for offset in 0..120 {
            let now = began + Duration::from_secs(offset);
            let tick = next_tick(now, began, wall);
            assert!(tick > now && tick <= now + Duration::from_mins(1));
        }
    }
}
