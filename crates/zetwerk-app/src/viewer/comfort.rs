//! Looking at the page for hours: the line being typed kept at one
//! height (typewriter), all but the paragraph worked on faint (focus),
//! nothing but the text (zen), and the pages light on dark (night) or on
//! sepia paper.
//!
//! None of it costs a frame more than a few quads, and none of it runs
//! by the clock. The typewriter moves the view once when the cursor's
//! line changes (a line break typed, `j`), never for a key within a line
//! and never for a compile that leaves the cursor on its line: it is
//! asked by [`Viewer::follow_cursor_by`] before anything else is. Focus
//! lays a veil of the paper's colour over what is not the paragraph (or
//! sentence) the cursor is in: up to four quads a page. Zen hides the
//! status line, unless it has something to say, and every aid over the
//! pages, and brings them back as they were when it ends. Night colours
//! are the compositor's (see `gpu::tone`): the viewer only says which
//! quads are on pages, where the pages are and where their pictures
//! are, which keep their colours. What is exported, and what agents are
//! shown, is never coloured so.
//!
//! An overlay drawn on a page among the others (between the paper and
//! the end of [`Viewer::pages`]'s loop) is given in the colours it has
//! on white paper: the compositor maps it with the page, and what is
//! readable on white is readable on the dark paper. Whatever is drawn on
//! a page outside that loop and should match it takes its colours from
//! [`Viewer::on_paper`].

use std::collections::HashMap;
use std::ops::Range;
use std::time::{Duration, Instant};

use typst::layout::Rect;
use typst_layout::Page;
use zetwerk_typst::PageMap;

use super::{Paging, Scene, TextureId, Viewer, tint};
use crate::chrome::STATUS_HEIGHT;
use crate::gpu::Tone;

/// How the pages are coloured, as the user asks for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Colours {
    #[default]
    Day,
    Night,
    Sepia,
    /// Night where the desktop prefers dark, or else between eight in
    /// the evening and seven in the morning.
    Auto,
}

/// What focus leaves clear: the paragraph the cursor is in, or its
/// sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusBy {
    #[default]
    Paragraph,
    Sentence,
}

/// What the user's settings say of all this, and of finding the place
/// left again (see `place`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// Whether a document is opened where it was left (`[view] restore`).
    pub restore: bool,
    /// Whether this time it is not (`--fresh`): the place is still kept.
    pub fresh: bool,
    pub colours: Colours,
    /// Whether the typewriter is on from the start, the line kept in the
    /// middle of the window.
    pub typewriter: bool,
    /// Whether focus is on from the start, and on what.
    pub focus: Option<FocusBy>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            restore: true,
            fresh: false,
            colours: Colours::Day,
            typewriter: false,
            focus: None,
        }
    }
}

/// What lies over a page outside focus: its paper, three fifths opaque.
const VEIL: [u8; 4] = [0xff, 0xff, 0xff, 0x99];
/// Where the typewriter keeps the line when the cursor is not in view,
/// or too near an edge of it, when it is turned on: in the middle.
const MIDDLE: f64 = 0.5;
const NEAR_EDGE: f64 = 0.15;
/// When the evening begins and the morning, for `Colours::Auto`.
const EVENING: u32 = 20;
const MORNING: u32 = 7;
/// How often the clock is looked at for that, at most.
const LOOK_AT_CLOCK: Duration = Duration::from_secs(60);

/// What zen hid, to be shown again when it ends.
#[allow(
    clippy::struct_excessive_bools,
    reason = "the switches of the aids, each its own"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Hidden {
    pub(super) numbers: bool,
    pub(super) hints: bool,
    pub(super) next_keys: bool,
    pub(super) tags: bool,
    pub(super) errors: bool,
    pub(super) perf: bool,
    pub(super) focus: Option<FocusBy>,
}

/// What this module keeps between keys and frames.
#[derive(Default)]
pub(super) struct Comfort {
    /// While the typewriter is on: at which height of the view the line
    /// is kept, from 0 (top) to 1.
    pub(super) typewriter: Option<f64>,
    /// The line it last kept there: its page, its top and its bottom in
    /// points.
    line: Option<(usize, f64, f64)>,
    pub(super) focus: Option<FocusBy>,
    /// While zen is on, what it hid.
    pub(super) zen: Option<Hidden>,
    pub(super) colours: Colours,
    /// Whether the colours were chosen by a command, not by the settings:
    /// then they are kept with the place the document was left at.
    pub(super) chosen: bool,
    /// Whether the desktop prefers dark, where that is known.
    desktop_dark: Option<bool>,
    /// When the clock was last looked at, and whether it was night.
    clock: Option<(Instant, bool)>,
    /// The pictures on pages, in points, for the document (by where it
    /// is in memory) and the page they are of.
    pictures: HashMap<(usize, usize), Vec<Rect>>,
    /// What focus leaves clear on each page, for what it was worked out
    /// for: the document, the bytes, and a page worked out from typing.
    clear: HashMap<usize, (ClearKey, Option<Clear>)>,
}

type ClearKey = (usize, Range<usize>, Option<[i64; 4]>);

/// What focus leaves clear on a page, in points: from the top of its
/// first line to the bottom of its last, and where on those two lines it
/// begins and ends.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Clear {
    top: f64,
    bottom: f64,
    first: (f64, f64, f64),
    last: (f64, f64, f64),
}

impl Clear {
    /// What `rects` cover, if anything.
    fn of(rects: &[Rect]) -> Option<Self> {
        let pt = |rect: &Rect| {
            (
                rect.min.x.to_pt(),
                rect.min.y.to_pt(),
                rect.max.x.to_pt(),
                rect.max.y.to_pt(),
            )
        };
        let first = rects.iter().map(pt).min_by(|a, b| {
            (a.1 + a.3)
                .total_cmp(&(b.1 + b.3))
                .then(a.0.total_cmp(&b.0))
        })?;
        let last = rects.iter().map(pt).max_by(|a, b| {
            (a.1 + a.3)
                .total_cmp(&(b.1 + b.3))
                .then(a.2.total_cmp(&b.2))
        })?;
        let top = rects
            .iter()
            .map(|rect| pt(rect).1)
            .fold(f64::INFINITY, f64::min);
        let bottom = rects.iter().map(|rect| pt(rect).3).fold(0.0, f64::max);
        Some(Self {
            top,
            bottom,
            first: (first.0, first.1, first.3),
            last: (last.2, last.1, last.3),
        })
    }

    /// The parts of a page `width` by `height` points that are not
    /// clear: above, below, before the start on the first line and
    /// behind the end on the last; as x, y, width, height in points.
    fn veiled(self, width: f64, height: f64) -> Vec<(f64, f64, f64, f64)> {
        let (start, first_top, first_bottom) = self.first;
        let (end, last_top, last_bottom) = self.last;
        let parts = [
            (0.0, 0.0, width, self.top),
            (0.0, self.bottom, width, height - self.bottom),
            (0.0, first_top, start, first_bottom - first_top),
            (end, last_top, width - end, last_bottom - last_top),
        ];
        parts
            .into_iter()
            .filter(|(_, _, w, h)| *w > 0.0 && *h > 0.0)
            .collect()
    }
}

/// The bytes of the paragraph of `text` that byte `at` is in: the lines
/// around it up to a blank one; a heading or an item of a list alone.
pub(super) fn paragraph(text: &str, at: usize) -> Range<usize> {
    let at = at.min(text.len());
    let line_start = |at: usize| text[..at].rfind('\n').map_or(0, |found| found + 1);
    let line_end = |at: usize| text[at..].find('\n').map_or(text.len(), |found| at + found);
    let alone = |line: &str| {
        let line = line.trim_start();
        line.starts_with('=') || line.starts_with("- ") || line.starts_with("+ ")
    };
    let (mut start, mut end) = (line_start(at), line_end(at));
    if alone(&text[start..end]) {
        return start..end;
    }
    while start > 0 {
        let before = line_start(start - 1);
        let line = &text[before..start - 1];
        if line.trim().is_empty() || alone(line) {
            break;
        }
        start = before;
    }
    while end < text.len() {
        let after = line_end(end + 1);
        let line = &text[end + 1..after];
        if line.trim().is_empty() || alone(line) {
            break;
        }
        end = after;
    }
    start..end
}

/// The bytes of the sentence of `text` that byte `at` is in, within
/// `within` (its paragraph): from behind the end of the one before to
/// its own end, closing marks and all.
pub(super) fn sentence(text: &str, at: usize, within: Range<usize>) -> Range<usize> {
    let ends = |c: char| matches!(c, '.' | '!' | '?');
    let closing = |c: char| matches!(c, '"' | '\'' | ')' | ']' | '”' | '’' | '»' | '*' | '_');
    let part = &text[within.clone()];
    let at = at.clamp(within.start, within.end) - within.start;
    // Where a sentence ends in the paragraph: behind its mark and what
    // closes, where a blank or the end follows.
    let mut boundaries = vec![0];
    let mut chars = part.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        if !ends(c) {
            continue;
        }
        let mut end = index + c.len_utf8();
        while let Some(&(next, c)) = chars.peek() {
            if closing(c) || ends(c) {
                end = next + c.len_utf8();
                chars.next();
            } else {
                break;
            }
        }
        if chars.peek().is_none_or(|(_, c)| c.is_whitespace()) {
            boundaries.push(end);
        }
    }
    boundaries.push(part.len());
    let start = boundaries
        .iter()
        .copied()
        .filter(|end| *end <= at)
        .max()
        .unwrap_or(0);
    let end = boundaries
        .iter()
        .copied()
        .find(|end| *end > at)
        .unwrap_or(part.len());
    // Not the blanks between it and the one before.
    let blanks = part[start..end].len() - part[start..end].trim_start().len();
    within.start + start + blanks..within.start + end
}

/// Whether it is night by the clock here.
fn night_by_the_clock() -> bool {
    use chrono::Timelike as _;
    let hour = chrono::Local::now().hour();
    !(MORNING..EVENING).contains(&hour)
}

/// Whether the desktop says it prefers dark, as GTK is told: by
/// `GTK_THEME` or GTK's settings. Sway itself has no say about it.
fn desktop_prefers_dark() -> Option<bool> {
    if let Ok(theme) = std::env::var("GTK_THEME") {
        return Some(theme.to_ascii_lowercase().ends_with(":dark"));
    }
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::Path::new(&home).join(".config"))
        })?;
    for version in ["gtk-4.0", "gtk-3.0"] {
        let Ok(text) = std::fs::read_to_string(config.join(version).join("settings.ini")) else {
            continue;
        };
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if key.trim() == "gtk-application-prefer-dark-theme" {
                let value = value.trim();
                return Some(value == "1" || value.eq_ignore_ascii_case("true"));
            }
        }
    }
    None
}

impl Viewer {
    /// Takes the user's settings for all this.
    pub(super) fn comfort_configured(&mut self, settings: Settings) {
        self.comfort.colours = settings.colours;
        if settings.colours == Colours::Auto {
            self.comfort.desktop_dark = self.comfort.desktop_dark.or_else(desktop_prefers_dark);
        }
        if settings.typewriter {
            self.comfort.typewriter = Some(MIDDLE);
        }
        self.comfort.focus = settings.focus;
        self.place.never = !settings.restore;
        self.place.fresh = settings.fresh;
    }

    /// What the window system says of the desktop: whether it prefers
    /// dark, where it says.
    pub fn desktop_theme(&mut self, dark: Option<bool>) {
        if dark.is_some() {
            self.comfort.desktop_dark = dark;
        }
    }

    /// How the pages are coloured now.
    #[must_use]
    pub fn page_tone(&self) -> Tone {
        match self.comfort.colours {
            Colours::Day => Tone::Day,
            Colours::Night => Tone::Night,
            Colours::Sepia => Tone::Sepia,
            Colours::Auto => {
                let dark = self.comfort.desktop_dark.unwrap_or_else(|| {
                    self.comfort
                        .clock
                        .map_or_else(night_by_the_clock, |(_, night)| night)
                });
                if dark { Tone::Night } else { Tone::Day }
            }
        }
    }

    /// `rgba` as it looks on a page now: for what is drawn on a page and
    /// is not among the quads the compositor colours with it (see the
    /// module's documentation). The one place where the colours of the
    /// day become those of the night.
    #[must_use]
    pub fn on_paper(&self, rgba: [u8; 4]) -> [u8; 4] {
        self.page_tone().apply(rgba)
    }

    /// Looks at the clock for `Colours::Auto`, at most once a minute.
    fn look_at_clock(&mut self) {
        if self.comfort.colours != Colours::Auto || self.comfort.desktop_dark.is_some() {
            return;
        }
        let now = Instant::now();
        if self
            .comfort
            .clock
            .is_none_or(|(at, _)| now.duration_since(at) >= LOOK_AT_CLOCK)
        {
            self.comfort.clock = Some((now, night_by_the_clock()));
        }
    }

    /// Where the view is: what has to change for a frame to differ from
    /// the last as far as scrolling and zooming go.
    #[must_use]
    pub fn where_shown(&self) -> (f64, f64, f64, usize) {
        (self.view.top, self.view.center, self.view.zoom, self.slide)
    }

    /// The display's scale went from `before` to what it is: a zoom the
    /// user chose shows the page as large on the screen as it was, not
    /// as many pixels large.
    pub(super) fn keep_size_on_screen(&mut self, before: f64) {
        if self.fit || before <= 0.0 || (before - self.display).abs() < f64::EPSILON {
            return;
        }
        let zoom = self.view.zoom * self.display / before;
        self.view.zoom = zoom.clamp(crate::view::MIN_ZOOM, crate::view::MAX_ZOOM);
        self.planned.clear();
    }

    /// `:typewriter`, `:focus`, `:zen`, `:night`, `:sepia`, `:colours`.
    /// Returns whether `command` was one of them.
    pub(super) fn comfort_command(&mut self, command: &str) -> bool {
        let (name, wanted) = match command.split_once(' ') {
            Some((name, wanted)) => (name, wanted.trim()),
            None => (command, ""),
        };
        let switch = |now: bool| match wanted {
            "on" => Some(true),
            "off" => Some(false),
            "" => Some(!now),
            _ => None,
        };
        match name {
            "typewriter" => match (wanted, switch(self.comfort.typewriter.is_some())) {
                ("middle", _) => self.typewriter_on(Some(MIDDLE)),
                (_, Some(true)) => self.typewriter_on(None),
                (_, Some(false)) => {
                    self.comfort.typewriter = None;
                    self.notice("the view follows the cursor as usual");
                }
                (_, None) => self.alert("on, off or middle, please: :typewriter middle"),
            },
            "focus" => {
                let by = match wanted {
                    "paragraph" => Some(Some(FocusBy::Paragraph)),
                    "sentence" => Some(Some(FocusBy::Sentence)),
                    _ => switch(self.comfort.focus.is_some())
                        .map(|on| on.then(|| self.comfort.focus.unwrap_or_default())),
                };
                match by {
                    Some(by) => self.set_focus(by),
                    None => self.alert("on, off, paragraph or sentence, please"),
                }
            }
            "zen" => match switch(self.comfort.zen.is_some()) {
                Some(on) => self.set_zen(on),
                None => self.alert("on or off, please: :zen"),
            },
            "night" | "sepia" => {
                let tone = if name == "night" {
                    Colours::Night
                } else {
                    Colours::Sepia
                };
                let now = self.page_tone()
                    == if name == "night" {
                        Tone::Night
                    } else {
                        Tone::Sepia
                    };
                match switch(now) {
                    Some(on) => self.set_colours(if on { tone } else { Colours::Day }),
                    None => self.alert(format!("on or off, please: :{name}")),
                }
            }
            "colours" | "colors" => match wanted {
                "day" => self.set_colours(Colours::Day),
                "night" => self.set_colours(Colours::Night),
                "sepia" => self.set_colours(Colours::Sepia),
                "auto" => self.set_colours(Colours::Auto),
                _ => self.alert("day, night, sepia or auto, please: :colours night"),
            },
            _ => return false,
        }
        true
    }

    /// Turns the typewriter on, keeping the line at `height` of the view,
    /// or else where the cursor's line is now.
    fn typewriter_on(&mut self, height: Option<f64>) {
        let here = self.cursor_line().and_then(|(page, top, bottom)| {
            let page = self.layout.pages.get(page)?;
            let middle = f64::from(page.y) + f64::midpoint(top, bottom) * self.view.zoom;
            let height = (middle - self.view.top * self.view.zoom) / f64::from(self.view.height);
            (NEAR_EDGE..=1.0 - NEAR_EDGE)
                .contains(&height)
                .then_some(height)
        });
        let height = height.or(here).unwrap_or(MIDDLE);
        self.comfort.typewriter = Some(height);
        self.comfort.line = None;
        self.typewriter_follows();
        self.notice(format!(
            "typewriter: the line typed stays {} of the window (:typewriter off)",
            if (height - MIDDLE).abs() < 0.01 {
                "in the middle".to_owned()
            } else {
                format!("at {:.0} %", height * 100.0)
            }
        ));
    }

    /// Where the typewriter is on, keeps the cursor's line where it is
    /// kept: the view moves when the line is another than the one it
    /// kept last, or is out of view; not for a key within the line, and
    /// not for pages that leave the cursor on its line. Returns whether
    /// it saw to the view (where it is off, or the cursor is on nothing
    /// a page shows, the view follows the cursor as usual).
    pub(super) fn typewriter_follows(&mut self) -> bool {
        let Some(height) = self.comfort.typewriter else {
            return false;
        };
        if self.paging == Paging::Slides {
            return false;
        }
        let Some((page, top, bottom)) = self.cursor_line() else {
            return false;
        };
        let Some(page_box) = self.layout.pages.get(page) else {
            return false;
        };
        let zoom = self.view.zoom;
        let middle = f64::from(page_box.y) + f64::midpoint(top, bottom) * zoom;
        let view_top = self.view.top * zoom;
        let in_view = middle >= view_top && middle <= view_top + f64::from(self.view.height);
        let same = self.comfort.line.is_some_and(|(was, was_top, was_bottom)| {
            let pitch = (bottom - top).max(was_bottom - was_top);
            was == page && (was_top - top).abs() < pitch / 2.0
        });
        self.comfort.line = Some((page, top, bottom));
        if !same || !in_view {
            self.view.top = (middle - height * f64::from(self.view.height)) / zoom;
            self.view.clamp(&self.layout);
        }
        true
    }

    /// The typeset line the cursor is on: its page, and its top and
    /// bottom in points. On a page worked out from typing, where the
    /// cursor is behind what was typed, the cell of that.
    pub(super) fn cursor_line(&self) -> Option<(usize, f64, f64)> {
        let editor = self.editor.as_ref()?;
        if let Some(predicted) = &self.predicted {
            let typed = predicted.typed;
            return (editor.cursor() == predicted.end)
                .then(|| (predicted.index, typed.min.y.to_pt(), typed.max.y.to_pt()));
        }
        let change = self.places()?;
        let (document, world) = (self.document.as_ref()?, self.compiled_world.as_ref()?);
        let file = editor.file();
        let then = change.back_or_edge(editor.cursor());
        let rows = super::PageRows {
            document,
            world,
            file,
            lines: &self.page_lines,
            cursor_page: &self.cursor_page,
            change,
            landed: &self.landed,
            elsewhere: &self.elsewhere,
        };
        let page = rows.page_of(then)?;
        let (top, bottom) = rows.on(page, |lines| {
            let (line, _) = lines.find(file, then)?;
            lines.band(line)
        })??;
        Some((page, top, bottom))
    }

    /// Turns focus on (on what `by` says) or off.
    fn set_focus(&mut self, by: Option<FocusBy>) {
        self.comfort.focus = by;
        self.comfort.clear.clear();
        self.notice(match by {
            Some(FocusBy::Paragraph) => "focus: all but this paragraph is faint (:focus sentence)",
            Some(FocusBy::Sentence) => "focus: all but this sentence is faint (:focus paragraph)",
            None => "focus off: all of the page as it is",
        });
    }

    /// Turns zen on (hiding the status line and the aids over the pages,
    /// with focus on) or off (bringing back what it hid).
    fn set_zen(&mut self, on: bool) {
        match (on, self.comfort.zen) {
            (true, None) => {
                self.comfort.zen = Some(Hidden {
                    numbers: self.numbers,
                    hints: self.focus.on,
                    next_keys: self.hints.next_keys,
                    tags: !self.margin.hide_tags,
                    errors: !self.margin.hide_errors,
                    perf: self.performance_panel,
                    focus: self.comfort.focus,
                });
                self.numbers = false;
                self.focus.on = false;
                self.hints.next_keys = false;
                self.margin.hide_tags = true;
                self.margin.hide_errors = true;
                self.performance_panel = false;
                if self.comfort.focus.is_none() {
                    self.comfort.focus = Some(FocusBy::Paragraph);
                }
                self.notice("zen: nothing but the text  (:zen again, or ze, brings all back)");
            }
            (false, Some(hidden)) => {
                self.comfort.zen = None;
                self.numbers = hidden.numbers;
                self.focus.on = hidden.hints;
                self.hints.next_keys = hidden.next_keys;
                self.margin.hide_tags = !hidden.tags;
                self.margin.hide_errors = !hidden.errors;
                self.performance_panel = hidden.perf;
                self.comfort.focus = hidden.focus;
                self.notice("zen off");
            }
            _ => {}
        }
        self.comfort.clear.clear();
        self.arrange();
        self.follow_cursor();
    }

    /// Colours the pages as `colours` says.
    fn set_colours(&mut self, colours: Colours) {
        self.comfort.colours = colours;
        self.comfort.chosen = true;
        if colours == Colours::Auto {
            self.comfort.desktop_dark = self.comfort.desktop_dark.or_else(desktop_prefers_dark);
            self.comfort.clock = None;
            self.look_at_clock();
        }
        self.notice(match (colours, self.page_tone()) {
            (Colours::Day, _) => "the pages as they are",
            (Colours::Night, _) => "night: the pages light on dark (exports stay as they are)",
            (Colours::Sepia, _) => "sepia: the pages on warm paper (exports stay as they are)",
            (Colours::Auto, Tone::Night) => "colours follow the desktop or the hour: night now",
            (Colours::Auto, _) => "colours follow the desktop or the hour: day now",
        });
    }

    /// The height of the status line: none in zen.
    pub(super) fn status_hidden(&self) -> bool {
        self.comfort.zen.is_some()
    }

    /// Where the status line goes, at `y` as usual: in zen only when it
    /// has something to `say` (a command being typed, a message), over
    /// the foot of the page; not at all otherwise.
    pub(super) fn zen_status(&self, y: f64, say: bool) -> Option<f64> {
        match (self.comfort.zen, say) {
            (None, _) => Some(y),
            (Some(_), true) => {
                Some((f64::from(self.window.1) - STATUS_HEIGHT * self.scale).round())
            }
            (Some(_), false) => None,
        }
    }

    /// The bytes focus leaves clear, in the text of the file edited.
    fn focus_range(&self) -> Option<Range<usize>> {
        let by = self.comfort.focus?;
        let editor = self.editor.as_ref()?;
        let (text, cursor) = (editor.text(), editor.cursor());
        let paragraph = paragraph(text, cursor);
        Some(match by {
            FocusBy::Paragraph => paragraph,
            FocusBy::Sentence => sentence(text, cursor, paragraph),
        })
    }

    /// Lays the veil of focus over page `index` (drawn as `page`, at
    /// `origin`) where it is not the paragraph or sentence worked on.
    pub(super) fn focus_veils(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        index: usize,
        origin: (f64, f64),
        page: &Page,
    ) {
        let Some(range) = self.focus_range() else {
            return;
        };
        let (Some(change), Some(page_box)) = (self.places_of_text(), self.layout.pages.get(index))
        else {
            return;
        };
        let file = self.file_id();
        let then = change.back_or_edge(range.start)..change.back_or_edge(range.end);
        let typed = self
            .predicted
            .as_ref()
            .filter(|predicted| predicted.index == index)
            .map(|predicted| predicted.typed);
        let document = self
            .document
            .as_ref()
            .map_or(0, |document| document.pages().as_ptr() as usize);
        let bits = typed.map(|rect| {
            [rect.min.x, rect.min.y, rect.max.x, rect.max.y].map(|at| at.to_raw().round() as i64)
        });
        let key: ClearKey = (document, then.clone(), bits);
        let clear = match self.comfort.clear.get(&index) {
            Some((was, clear)) if *was == key => *clear,
            _ => {
                let Some(world) = self.compiled_world.clone() else {
                    return;
                };
                // The map of the page as compiled: one worked out from
                // typing has none of its own, and the typed text is
                // where its cell is.
                let mut rects = match self.maps.get(&index) {
                    Some(map) => map.selection(file, &then),
                    None if typed.is_none() => {
                        let map = PageMap::build(&world, &page.frame);
                        let rects = map.selection(file, &then);
                        self.maps.insert(index, map);
                        rects
                    }
                    None => Vec::new(),
                };
                rects.extend(typed.filter(|_| range.contains(&self.editor_cursor())));
                let clear = Clear::of(&rects);
                self.comfort.clear.insert(index, (key, clear));
                clear
            }
        };
        let zoom = self.view.zoom;
        let (width, height) = (
            f64::from(page_box.width) / zoom,
            f64::from(page_box.height) / zoom,
        );
        let parts = match clear {
            Some(clear) => clear.veiled(width, height),
            // A page the cursor is not on, without any of it: all faint.
            None if index != self.cursor_page.get() => vec![(0.0, 0.0, width, height)],
            None => Vec::new(),
        };
        for (x, y, w, h) in parts {
            let at = |v: f64| v * zoom;
            tint(
                scene,
                has,
                VEIL,
                (origin.0 + at(x), origin.1 + at(y), at(w), at(h)),
            );
        }
    }

    fn editor_cursor(&self) -> usize {
        self.editor
            .as_ref()
            .map_or(0, crate::editor::Editor::cursor)
    }

    /// Notes that what was drawn of page `index` (as `page`, at
    /// `origin`) since quad `first` is on paper, for the compositor to
    /// colour (see `gpu::tone`), with where its pictures are.
    pub(super) fn paper_page(
        &mut self,
        scene: &mut Scene,
        first: usize,
        page: &Page,
        index: usize,
        origin: (f64, f64),
    ) {
        self.look_at_clock();
        let tone = self.page_tone();
        scene.paper.tone = tone;
        if tone == Tone::Day {
            return;
        }
        let Some(page_box) = self.layout.pages.get(index) else {
            return;
        };
        scene.paper.quads.push(first..scene.quads.len());
        let (width, height) = (f64::from(page_box.width), f64::from(page_box.height));
        scene
            .paper
            .pages
            .push([origin.0, origin.1, width, height].map(|v| v as f32));
        // Of the page drawn: one worked out from typing is another than
        // the document's, but the same pictures are on it.
        let document = self
            .document
            .as_ref()
            .map_or(0, |document| document.pages().as_ptr() as usize);
        if self.comfort.pictures.keys().any(|(of, _)| *of != document) {
            self.comfort.pictures.clear();
        }
        let pictures = self
            .comfort
            .pictures
            .entry((document, index))
            .or_insert_with(|| zetwerk_render::pictures(&page.frame));
        let zoom = self.view.zoom;
        for rect in pictures.iter() {
            let (x, y) = (rect.min.x.to_pt() * zoom, rect.min.y.to_pt() * zoom);
            let (w, h) = (
                (rect.max.x - rect.min.x).to_pt() * zoom,
                (rect.max.y - rect.min.y).to_pt() * zoom,
            );
            scene
                .paper
                .pictures
                .push([origin.0 + x, origin.1 + y, w, h].map(|v| v as f32));
        }
    }
}

/// A quad's rectangle, for tests that look for veils.
#[cfg(test)]
pub(super) fn veils(scene: &Scene) -> Vec<[f32; 4]> {
    let veil = TextureId(typst::utils::hash128(&("tint", VEIL)));
    scene
        .quads
        .iter()
        .filter(|quad| quad.texture == veil)
        .map(|quad| [quad.x, quad.y, quad.width, quad.height])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_paragraph_is_its_lines_up_to_a_blank_one() {
        let text = "= Title\nIntro line one\nline two.\n\n- item\n- other\nAfter.\n";
        let at = text.find("one").unwrap_or(0);
        assert_eq!(&text[paragraph(text, at)], "Intro line one\nline two.");
        let at = text.find("item").unwrap_or(0);
        assert_eq!(&text[paragraph(text, at)], "- item");
        assert_eq!(&text[paragraph(text, 2)], "= Title");
        let at = text.find("After").unwrap_or(0);
        assert_eq!(&text[paragraph(text, at)], "After.");
    }

    #[test]
    fn a_sentence_ends_at_its_mark_and_what_closes_it() {
        let text = "One two. \"Three?\" Four e.g.five! Six";
        let all = 0..text.len();
        let at = |word: &str| text.find(word).unwrap_or(0);
        assert_eq!(&text[sentence(text, at("two"), all.clone())], "One two.");
        assert_eq!(
            &text[sentence(text, at("Three"), all.clone())],
            "\"Three?\""
        );
        // A dot with no blank behind it ends nothing.
        assert_eq!(
            &text[sentence(text, at("five"), all.clone())],
            "Four e.g.five!"
        );
        assert_eq!(&text[sentence(text, at("Six"), all)], "Six");
    }
}
