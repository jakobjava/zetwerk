//! Where a document was left, found again when it is opened: the cursor
//! in every file of it that was open, which of them was being edited,
//! the view (scroll, zoom, layout, slides), the aids that were on, the
//! last search, the marks and the places jumped from; and the registers,
//! as Vim's shada keeps them.
//!
//! It is kept beside the journal, in the state directory: one file for
//! each document (named after its main file, as notes are), and one for
//! the registers, all in a directory only the user can enter, each
//! written whole or not at all. Nothing of it ever fails loudly: a place
//! that cannot be written or read is a document opened at its start.
//!
//! It is written when the window closes, loses the focus or goes to
//! another document, and while it is used every so often, on the events
//! that wake it anyway: never by a clock of its own. It is read once the
//! first pages are there, where the cursor's page can be known; and not
//! where the user is at work already. Where the text of a file changed
//! since (outside zetwerk), a place is found again by the text around it,
//! or else by the heading above it, or else it is the start.

use super::notes::{escape, unescape};
use crate::editor::floor_boundary as floor;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::comfort::{Colours, FocusBy};
use super::{Paging, Panes, Viewer};
use crate::editor::{self, Editor};

const HEADER: &str = "zetwerk place 1";
const REGISTERS_HEADER: &str = "zetwerk registers 1";
/// The directory of the stores, and the file of the registers in it.
const DIRECTORY: &str = "places";
const REGISTERS: &str = "registers";
/// How many bytes of the text before and after a place are kept to find
/// it by.
const CONTEXT: usize = 48;
/// How often, at most, the place is written while the window is used.
const KEEP_EVERY: Duration = Duration::from_secs(10);
/// A register longer than this is not kept (Vim's shada keeps 10 KiB).
const REGISTER_MOST: usize = 10 * 1024;

/// What this module keeps.
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent facts about the place, not states of one thing"
)]
#[derive(Default)]
pub(super) struct Place {
    /// Whether no place is found again nor kept (`[view] restore =
    /// false`), and whether it is not found again this time (`--fresh`).
    pub(super) never: bool,
    pub(super) fresh: bool,
    /// Whether the place was looked for: before that, nothing is
    /// written, or the place left would be lost to the start.
    tried: bool,
    /// Whether a key came before the place could be looked for.
    pub(super) touched: bool,
    /// What was read, for the files opened later.
    kept: Option<Kept>,
    /// What was written last, and when it was last looked at.
    written: Option<String>,
    registers_written: Option<String>,
    checked: Option<Instant>,
}

/// A place in a text, and what is around it to find it by where the
/// text has changed.
#[derive(Debug, Clone, Default, PartialEq)]
struct Spot {
    at: usize,
    before: String,
    after: String,
    heading: String,
}

/// What is kept of a file of the document.
#[derive(Debug, Clone, Default, PartialEq)]
struct File {
    path: PathBuf,
    /// The text it was kept for, by its hash.
    hash: u64,
    cursor: Spot,
    marks: Vec<(char, Spot)>,
    jumps: Vec<Spot>,
}

/// The view: the zoom in pixels per point at a display scale of one,
/// whether it follows the window's width, where its top and middle are
/// (in points), and at which height of it the cursor is.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct View {
    zoom: f64,
    fit: bool,
    top: f64,
    center: f64,
    cursor: Option<f64>,
}

/// What a store holds.
#[derive(Debug, Clone, Default, PartialEq)]
struct Kept {
    edited: Option<PathBuf>,
    panes: Option<Panes>,
    slide: Option<usize>,
    view: Option<View>,
    aids: Vec<(String, String)>,
    search: Option<(String, bool, bool, bool)>,
    files: Vec<File>,
}

/// FNV-1a: the same in every run.
fn hash(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// Where the place of the document whose main file is at `main` is kept.
fn store_path(state_dir: &Path, main: &Path) -> PathBuf {
    let name = hash(main.as_os_str().as_encoded_bytes());
    state_dir.join(DIRECTORY).join(format!("{name:016x}.place"))
}

impl Spot {
    /// Byte `at` of `text`, with what is around it.
    fn of(text: &str, at: usize) -> Self {
        let at = floor(text, at);
        let start = floor(text, at.saturating_sub(CONTEXT));
        let end = floor(text, at + CONTEXT);
        let heading = text[..at]
            .lines()
            .rev()
            .find(|line| line.trim_start().starts_with('='))
            .unwrap_or("")
            .to_owned();
        Self {
            at,
            before: text[start..at].to_owned(),
            after: text[at..end].to_owned(),
            heading,
        }
    }

    /// Where it is in `text`: where it was if the text is the same
    /// (`same`), or else where what was around it is, the nearest to
    /// where it was; or the start of its heading; or `None`.
    fn find(&self, text: &str, same: bool) -> Option<usize> {
        if same {
            return Some(floor(text, self.at));
        }
        let nearest = |pattern: &str, shift: usize| {
            text.match_indices(pattern)
                .map(|(at, _)| at + shift)
                .min_by_key(|at| at.abs_diff(self.at))
        };
        let both = format!("{}{}", self.before, self.after);
        // Too little to be told from elsewhere: not looked for alone.
        let enough = |part: &str| part.trim().chars().count() >= 8;
        if enough(&both)
            && let Some(at) = nearest(&both, self.before.len())
        {
            return Some(at);
        }
        if enough(&self.after)
            && let Some(at) = nearest(&self.after, 0)
        {
            return Some(at);
        }
        if enough(&self.before)
            && let Some(at) = nearest(&self.before, self.before.len())
        {
            return Some(at);
        }
        if self.heading.is_empty() {
            return None;
        }
        let mut start = 0;
        for line in text.split_inclusive('\n') {
            if line.trim_end_matches(['\n', '\r']) == self.heading {
                return Some(start);
            }
            start += line.len();
        }
        None
    }

    fn write(&self, out: &mut String) {
        let _ = write!(
            out,
            "{}\t{}\t{}\t{}",
            self.at,
            escape(&self.before),
            escape(&self.after),
            escape(&self.heading)
        );
    }

    fn read(fields: &[&str]) -> Option<Self> {
        let [at, before, after, heading] = fields else {
            return None;
        };
        Some(Self {
            at: at.parse().ok()?,
            before: unescape(before),
            after: unescape(after),
            heading: unescape(heading),
        })
    }
}

impl File {
    fn of(editor: &Editor) -> Self {
        let text = editor.text();
        Self {
            path: editor.path().to_owned(),
            hash: hash(text.as_bytes()),
            cursor: Spot::of(text, editor.cursor()),
            marks: editor
                .marks_kept()
                .into_iter()
                .map(|(name, at)| (name, Spot::of(text, at)))
                .collect(),
            jumps: editor
                .jumps_kept()
                .iter()
                .map(|at| Spot::of(text, *at))
                .collect(),
        }
    }

    /// Gives `editor` what was kept of it: the cursor, the marks and the
    /// places jumped from, found again in its text as it is now.
    fn restore(&self, editor: &mut Editor) {
        let same = hash(editor.text().as_bytes()) == self.hash;
        let text = editor.text();
        let cursor = self.cursor.find(text, same).unwrap_or(0);
        let marks: Vec<(char, usize)> = self
            .marks
            .iter()
            .filter_map(|(name, spot)| Some((*name, spot.find(text, same)?)))
            .collect();
        let jumps: Vec<usize> = self
            .jumps
            .iter()
            .filter_map(|spot| spot.find(text, same))
            .collect();
        editor.set_cursor(cursor);
        for (name, at) in marks {
            editor.set_mark_kept(name, at);
        }
        editor.set_jumps_kept(&jumps);
    }
}

fn panes_name(panes: Panes) -> &'static str {
    match panes {
        Panes::Split => "split",
        Panes::Source => "source",
        Panes::Page => "page",
    }
}

fn flag(on: bool) -> &'static str {
    if on { "1" } else { "0" }
}

impl Kept {
    fn write(&self, main: &Path) -> String {
        let mut out = format!("{HEADER}\ndocument\t{}\n", escape(&main.to_string_lossy()));
        if let Some(edited) = &self.edited {
            let _ = writeln!(out, "edited\t{}", escape(&edited.to_string_lossy()));
        }
        if let Some(panes) = self.panes {
            let _ = writeln!(out, "layout\t{}", panes_name(panes));
        }
        if let Some(slide) = self.slide {
            let _ = writeln!(out, "slides\t{slide}");
        }
        if let Some(view) = self.view {
            let cursor = view
                .cursor
                .map_or_else(String::new, |at| format!("{at:.4}"));
            let _ = writeln!(
                out,
                "view\t{:.6}\t{}\t{:.3}\t{:.3}\t{cursor}",
                view.zoom,
                flag(view.fit),
                view.top,
                view.center
            );
        }
        for (name, value) in &self.aids {
            let _ = writeln!(out, "aid\t{name}\t{value}");
        }
        if let Some((pattern, forward, start, end)) = &self.search {
            let _ = writeln!(
                out,
                "search\t{}\t{}\t{}\t{}",
                flag(*forward),
                flag(*start),
                flag(*end),
                escape(pattern)
            );
        }
        for file in &self.files {
            let _ = writeln!(out, "file\t{}", escape(&file.path.to_string_lossy()));
            let _ = writeln!(out, "hash\t{:016x}", file.hash);
            out.push_str("cursor\t");
            file.cursor.write(&mut out);
            out.push('\n');
            for (name, spot) in &file.marks {
                let _ = write!(out, "mark\t{name}\t");
                spot.write(&mut out);
                out.push('\n');
            }
            for spot in &file.jumps {
                out.push_str("jump\t");
                spot.write(&mut out);
                out.push('\n');
            }
        }
        out
    }

    /// What `text` keeps; `None` if it is not a store of this kind.
    fn read(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        if lines.next() != Some(HEADER) {
            return None;
        }
        let mut kept = Self::default();
        let on = |field: &str| field == "1";
        for line in lines {
            let fields: Vec<&str> = line.split('\t').collect();
            let Some((&what, rest)) = fields.split_first() else {
                continue;
            };
            let file = kept.files.last_mut();
            match (what, rest, file) {
                ("edited", [path], _) => kept.edited = Some(PathBuf::from(unescape(path))),
                ("layout", [name], _) => {
                    kept.panes = match *name {
                        "split" => Some(Panes::Split),
                        "source" => Some(Panes::Source),
                        "page" => Some(Panes::Page),
                        _ => None,
                    };
                }
                ("slides", [slide], _) => kept.slide = slide.parse().ok(),
                ("view", [zoom, fit, top, center, cursor], _) => {
                    kept.view = (|| {
                        Some(View {
                            zoom: zoom.parse().ok().filter(|zoom: &f64| zoom.is_finite())?,
                            fit: on(fit),
                            top: top.parse().ok().filter(|top: &f64| top.is_finite())?,
                            center: center.parse().ok().filter(|at: &f64| at.is_finite())?,
                            cursor: cursor.parse().ok().filter(|at: &f64| at.is_finite()),
                        })
                    })();
                }
                ("aid", [name, value], _) => {
                    kept.aids.push(((*name).to_owned(), (*value).to_owned()));
                }
                ("search", [forward, start, end, pattern], _) => {
                    kept.search = Some((unescape(pattern), on(forward), on(start), on(end)));
                }
                ("file", [path], _) => kept.files.push(File {
                    path: PathBuf::from(unescape(path)),
                    ..File::default()
                }),
                ("hash", [hash], Some(file)) => {
                    file.hash = u64::from_str_radix(hash, 16).unwrap_or_default();
                }
                ("cursor", spot, Some(file)) => file.cursor = Spot::read(spot)?,
                ("mark", [name, spot @ ..], Some(file)) => {
                    let name = name.chars().next()?;
                    file.marks.push((name, Spot::read(spot)?));
                }
                ("jump", spot, Some(file)) => file.jumps.push(Spot::read(spot)?),
                // Of a later zetwerk: left out.
                _ => {}
            }
        }
        Some(kept)
    }
}

/// Writes `contents` to `store`, in a directory only the user can enter.
/// Not being able to is said to nobody: a place is no work lost.
fn write_store(store: &Path, contents: &str) -> bool {
    if let Some(directory) = store.parent() {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        if builder.create(directory).is_err() {
            return false;
        }
    }
    zetwerk_core::write_atomically(store, contents.as_bytes()).is_ok()
}

/// The registers as their store writes them.
fn registers_text(editor: &Editor) -> String {
    let mut out = format!("{REGISTERS_HEADER}\n");
    for (name, text, linewise) in editor.registers_kept() {
        if text.len() <= REGISTER_MOST {
            let _ = writeln!(
                out,
                "register\t{name}\t{}\t{}",
                flag(linewise),
                escape(text)
            );
        }
    }
    out
}

impl Viewer {
    /// What is kept of the place the document is at now.
    fn kept_now(&self) -> Option<Kept> {
        let editor = self.editor.as_ref()?;
        let mut files = vec![File::of(editor)];
        files.extend(self.others.iter().map(File::of));
        let cursor = self.cursor_at.and_then(|(_, top, _)| {
            let height = f64::from(self.view.height);
            (top >= 0.0 && top < height).then(|| top / height)
        });
        let view = View {
            zoom: self.view.zoom / self.display,
            fit: self.fit,
            top: self.view.top,
            center: self.view.center,
            cursor,
        };
        Some(Kept {
            edited: Some(editor.path().to_owned()),
            panes: Some(self.panes),
            slide: (self.paging == Paging::Slides).then_some(self.slide),
            view: Some(view),
            aids: self.aids_now(),
            search: editor
                .search_kept()
                .map(|(pattern, forward, start, end)| (pattern.to_owned(), forward, start, end)),
            files,
        })
    }

    /// Which aids are on, by name: those over the pages, and those of
    /// `comfort`. In zen, those it hid as they were before it.
    fn aids_now(&self) -> Vec<(String, String)> {
        let hidden = self.comfort.zen;
        let on = |name: &str, on: bool| (name.to_owned(), flag(on).to_owned());
        let mut aids = vec![
            on("numbers", hidden.map_or(self.numbers, |was| was.numbers)),
            on("hints", hidden.map_or(self.focus.on, |was| was.hints)),
            on(
                "nextkeys",
                hidden.map_or(self.hints.next_keys, |was| was.next_keys),
            ),
            on("ghost", self.hints.ghost),
            on(
                "tags",
                hidden.map_or(!self.margin.hide_tags, |was| was.tags),
            ),
            on(
                "errors",
                hidden.map_or(!self.margin.hide_errors, |was| was.errors),
            ),
            on(
                "perf",
                hidden.map_or(self.performance_panel, |was| was.perf),
            ),
            on("zen", hidden.is_some()),
        ];
        let focus = hidden.map_or(self.comfort.focus, |was| was.focus);
        let focus = match focus {
            None => "off",
            Some(FocusBy::Paragraph) => "paragraph",
            Some(FocusBy::Sentence) => "sentence",
        };
        aids.push(("focus".to_owned(), focus.to_owned()));
        let typewriter = self
            .comfort
            .typewriter
            .map_or_else(|| "off".to_owned(), |height| format!("{height:.4}"));
        aids.push(("typewriter".to_owned(), typewriter));
        if self.comfort.chosen {
            let colours = match self.comfort.colours {
                Colours::Day => "day",
                Colours::Night => "night",
                Colours::Sepia => "sepia",
                Colours::Auto => "auto",
            };
            aids.push(("colours".to_owned(), colours.to_owned()));
        }
        aids
    }

    /// Writes where the document is now, and the registers, where that
    /// differs from what was written last.
    fn keep_place(&mut self) {
        if self.place.never || !self.place.tried {
            return;
        }
        let (Some(state_dir), Some(main)) = (self.state_dir.clone(), self.main_path()) else {
            return;
        };
        if let Some(kept) = self.kept_now() {
            let text = kept.write(&main);
            if self.place.written.as_ref() != Some(&text)
                && write_store(&store_path(&state_dir, &main), &text)
            {
                self.place.written = Some(text);
            }
        }
        if let Some(editor) = &self.editor {
            let text = registers_text(editor);
            let store = state_dir.join(DIRECTORY).join(REGISTERS);
            if self.place.registers_written.as_ref() != Some(&text) && write_store(&store, &text) {
                self.place.registers_written = Some(text);
            }
        }
    }

    /// The window closes, loses the focus or goes to another document:
    /// where the document is now is kept.
    pub fn leaving(&mut self) {
        self.keep_place();
    }

    /// Something woke the window: where the document is now is kept, if
    /// that was not looked at for a while.
    pub fn keep_place_if_due(&mut self, now: Instant) {
        if self
            .place
            .checked
            .is_some_and(|at| now.duration_since(at) < KEEP_EVERY)
        {
            return;
        }
        if self.place.tried {
            self.place.checked = Some(now);
        }
        self.keep_place();
    }

    /// The first pages are there: the document is put where it was
    /// left, unless the user is at work in it already.
    pub(super) fn place_compiled(&mut self) {
        if self.place.tried || self.document.is_none() || self.editor.is_none() {
            return;
        }
        self.place.tried = true;
        if self.place.never || self.place.fresh {
            return;
        }
        let (Some(state_dir), Some(main)) = (self.state_dir.clone(), self.main_path()) else {
            return;
        };
        self.restore_registers(&state_dir);
        let Some(kept) = std::fs::read_to_string(store_path(&state_dir, &main))
            .ok()
            .and_then(|text| Kept::read(&text))
        else {
            return;
        };
        self.place.kept = Some(kept.clone());
        let at_work = self.place.touched
            || self.editor.as_ref().is_some_and(|editor| {
                editor.cursor() != 0 || editor.mode() != editor::Mode::Normal
            });
        if at_work {
            return;
        }
        self.restore_place(&kept, &main);
    }

    /// Puts the document where `kept` says it was left.
    fn restore_place(&mut self, kept: &Kept, main: &Path) {
        // The file edited then, where the document was opened by its
        // main file: one named to open is the one wanted.
        let opened_main = self
            .editor
            .as_ref()
            .is_some_and(|editor| super::buffers::same_file(editor.path(), main));
        let elsewhere = kept.edited.as_ref().filter(|edited| {
            opened_main
                && edited.is_file()
                && !self
                    .editor
                    .as_ref()
                    .is_some_and(|editor| super::buffers::same_file(editor.path(), edited))
        });
        self.place_file_opened(true);
        if let Some(file) = elsewhere.and_then(|path| self.project.file_id(path)) {
            // Its place is given to it as it is opened.
            self.open_file(file, None);
        }
        let Some(editor) = &mut self.editor else {
            return;
        };
        if let Some((pattern, forward, start, end)) = &kept.search {
            editor.set_search_kept(pattern, *forward, (*start, *end));
        }
        for (name, value) in &kept.aids {
            self.restore_aid(name, value);
        }
        if let Some(panes) = kept.panes {
            self.panes = panes;
        }
        self.arrange();
        if let Some(slide) = kept.slide {
            self.set_paging(Paging::Slides);
            self.show_slide(slide);
        } else if let Some(view) = kept.view {
            self.restore_view(view, kept);
        }
        // Not to be said as if a key had done it.
        self.message = self.message.take().filter(|message| message.alert);
    }

    /// Sets the zoom and the view as `view` has them: exactly where the
    /// text is as it was, or else with the cursor at the height it was.
    fn restore_view(&mut self, view: View, kept: &Kept) {
        self.fit = view.fit;
        if !view.fit && view.zoom > 0.0 {
            self.view.zoom =
                (view.zoom * self.display).clamp(crate::view::MIN_ZOOM, crate::view::MAX_ZOOM);
        }
        self.planned.clear();
        self.relayout();
        let unchanged = self.editor.as_ref().is_some_and(|editor| {
            kept.files.iter().any(|file| {
                super::buffers::same_file(&file.path, editor.path())
                    && file.hash == hash(editor.text().as_bytes())
            })
        });
        if unchanged {
            self.view.top = view.top;
            self.view.center = view.center;
            self.view.clamp(&self.layout);
            return;
        }
        let height = view.cursor.unwrap_or(1.0 / 3.0);
        let line = self.cursor_line();
        if let Some((page, top, _)) = line
            && let Some(page) = self.layout.pages.get(page)
        {
            let zoom = self.view.zoom;
            let y = f64::from(page.y) + top * zoom;
            self.view.top = (y - height * f64::from(self.view.height)) / zoom;
            self.view.center = view.center;
            self.view.clamp(&self.layout);
        } else {
            self.follow_cursor();
        }
    }

    /// Turns the aid `name` on or off as `value` says.
    fn restore_aid(&mut self, name: &str, value: &str) {
        let on = value == "1";
        match name {
            "numbers" => self.numbers = on,
            "hints" => self.focus.on = on,
            "nextkeys" => self.hints.next_keys = on,
            "ghost" => self.hints.ghost = on,
            "tags" => self.margin.hide_tags = !on,
            "errors" => self.margin.hide_errors = !on,
            "perf" => self.performance_panel = on,
            "focus" => {
                self.comfort.focus = match value {
                    "paragraph" => Some(FocusBy::Paragraph),
                    "sentence" => Some(FocusBy::Sentence),
                    _ => None,
                };
            }
            "typewriter" => {
                self.comfort.typewriter = value
                    .parse()
                    .ok()
                    .filter(|at: &f64| (0.0..=1.0).contains(at));
            }
            "colours" => {
                let colours = match value {
                    "night" => Colours::Night,
                    "sepia" => Colours::Sepia,
                    "auto" => Colours::Auto,
                    _ => Colours::Day,
                };
                self.comfort.colours = colours;
                self.comfort.chosen = true;
            }
            // Last of the aids written: what it hid is restored by now.
            "zen" if on => {
                let _ = self.comfort_command("zen on");
            }
            _ => {}
        }
    }

    /// The file being edited was just opened (`fresh`: not one that was
    /// open and waiting, nor with a place to go to): it is given the
    /// cursor, the marks and the places jumped from it was left with.
    pub(super) fn place_file_opened(&mut self, fresh: bool) {
        let (Some(kept), Some(editor)) = (&self.place.kept, &mut self.editor) else {
            return;
        };
        if !fresh {
            return;
        }
        let path = editor.path().to_owned();
        if let Some(file) = kept
            .files
            .iter()
            .find(|file| super::buffers::same_file(&file.path, &path))
        {
            file.restore(editor);
            self.follow_cursor();
        }
    }

    /// The registers kept, where the editor has none of that name yet.
    fn restore_registers(&mut self, state_dir: &Path) {
        let Ok(text) = std::fs::read_to_string(state_dir.join(DIRECTORY).join(REGISTERS)) else {
            return;
        };
        let mut lines = text.lines();
        let (Some(REGISTERS_HEADER), Some(editor)) = (lines.next(), &mut self.editor) else {
            return;
        };
        for line in lines {
            let Some(("register", rest)) = line.split_once('\t') else {
                continue;
            };
            let fields: Vec<&str> = rest.splitn(3, '\t').collect();
            let [name, linewise, text] = fields[..] else {
                continue;
            };
            let Some(name) = name.chars().next() else {
                continue;
            };
            if editor.engine().register(name).is_none() {
                editor.set_register_kept(name, &unescape(text), linewise == "1");
            }
        }
        self.place.registers_written = Some(text);
    }

    /// The file `old` was moved to `to` (`:mv`, `:saveas`): where it was
    /// the main file, its place goes along.
    pub(super) fn place_renamed(&mut self, old: typst::syntax::FileId, to: &Path) {
        if old != self.project.main() {
            return;
        }
        let (Some(state_dir), Some(from)) = (&self.state_dir, self.project.path_of(old)) else {
            return;
        };
        let (from, to) = (store_path(state_dir, &from), store_path(state_dir, to));
        if from != to && from.exists() {
            let _ = std::fs::rename(from, to);
        }
        self.place.written = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_store_reads_back_what_was_written() {
        let text = "= One\n\nSome words here and there.\n\n= Two\n\nMore.\n";
        let spot = Spot::of(text, text.find("here").unwrap_or(0));
        let kept = Kept {
            edited: Some(PathBuf::from("/a/b c.typ")),
            panes: Some(Panes::Page),
            slide: None,
            view: Some(View {
                zoom: 1.5,
                fit: false,
                top: 120.25,
                center: -3.0,
                cursor: Some(0.4),
            }),
            aids: vec![("numbers".to_owned(), "1".to_owned())],
            search: Some(("tab\there".to_owned(), true, false, true)),
            files: vec![File {
                path: PathBuf::from("/a/b c.typ"),
                hash: hash(text.as_bytes()),
                cursor: spot.clone(),
                marks: vec![('a', spot.clone())],
                jumps: vec![spot],
            }],
        };
        let written = kept.write(Path::new("/a/main.typ"));
        assert_eq!(Kept::read(&written), Some(kept));
        assert_eq!(Kept::read("something else\n"), None);
    }

    #[test]
    fn a_place_is_found_by_what_is_around_it_or_its_heading() {
        let text = "= One\n\nSome words here and there.\n\n= Two\n\nMore words follow here.\n";
        let at = text.find("follow").unwrap_or(0);
        let spot = Spot::of(text, at);
        assert_eq!(spot.find(text, true), Some(at));
        // Text put in before it: found where it went.
        let changed = format!("A new first line.\n{text}");
        assert_eq!(spot.find(&changed, false), Some(at + 18));
        // Its own line rewritten: under its heading.
        let rewritten = text.replace("More words follow here.", "Something else entirely.");
        let heading = rewritten.find("= Two").unwrap_or(0);
        assert_eq!(spot.find(&rewritten, false), Some(heading));
        // Nothing of it left: nowhere.
        assert_eq!(spot.find("Other text altogether.\n", false), None);
    }
}
