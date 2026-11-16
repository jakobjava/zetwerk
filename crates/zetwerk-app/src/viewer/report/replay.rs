//! A report played again: read back, its files put in a scratch
//! directory, the stage set, the keys pressed.
//!
//! [`Recorded::read`] reads a report directory; [`Recorded::copy_to`]
//! makes the scratch copy of the document (the files of `before/`, and
//! what `needs.txt` names copied from where it is now: nothing of the
//! user's is written); [`Viewer::report_stage`] sets the stage as it was
//! at the first key. Then the steps are taken in order: keys pressed,
//! compiles begun and delivered where they were, texts put in where they
//! changed between keys, files written as they were when a key opened
//! them. The window's `--replay` does this with the recorded pauses (see
//! the `zetwerk` binary); [`replay`] does it in a [`Viewer`] on this
//! thread, compiling where compiles landed, and is what turns a report
//! into a test of the suite.
//!
//! What cannot be the same: the window's size in a window the replay
//! does not size (a viewer here is given it); compiles in a window land
//! when they land there, not where they did; what was typed into a
//! prompt that is not recorded; files changed on disk (only said); undo
//! beyond the first key, whose history the copy does not have; and the
//! files the document needs as they were then (they are copied as they
//! are now).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use zetwerk_typst::{CompileOutput, Environment, FontSet, Project};

use super::super::{Paging, Panes, Viewer, editor};
use crate::editor::Editor;
use crate::keys::{self, Key, KeyPress};

/// What comes before the settings file at the end of `state.txt`.
pub const CONFIG_MARK: &str = "----- config.toml -----";

/// What the window was like at the first key: enough to set it so again.
#[derive(Debug, Clone, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent facts about the window, not states of one thing"
)]
pub struct Stage {
    /// The file being edited, by its name in the document's directory,
    /// and the cursor in it (a byte); whether it was typing.
    pub file: String,
    pub cursor: usize,
    pub insert: bool,
    /// The files that were open, in the order they were opened.
    pub open: Vec<String>,
    pub panes: Panes,
    pub slides: bool,
    /// Whether the zoom follows the window width; the zoom; where the
    /// view is, in points.
    pub fit: bool,
    pub zoom: f64,
    pub top: f64,
    pub center: f64,
    /// The size of what is not a page, as the user set it.
    pub ui: f64,
    /// The window in device pixels, and the display's scale.
    pub window: (u32, u32),
    pub display: f64,
    pub parts: bool,
    pub numbers: bool,
    /// What the Vim of the file being edited remembered (registers, the
    /// last search, …: see `Engine::memory`); `vim.txt` of a report.
    pub memory: String,
    /// Whether nothing was under way then that a replay cannot set up
    /// (see `Viewer::at_rest`): where something was, it begins as near
    /// as it can, and may go otherwise.
    pub rest: bool,
}

impl Default for Stage {
    fn default() -> Self {
        Self {
            file: String::new(),
            cursor: 0,
            insert: false,
            open: Vec::new(),
            panes: Panes::Page,
            slides: false,
            fit: true,
            zoom: 1.0,
            top: 0.0,
            center: 0.0,
            ui: 1.0,
            window: (1000, 620),
            display: 1.0,
            parts: true,
            numbers: true,
            memory: String::new(),
            rest: true,
        }
    }
}

impl Stage {
    /// Writes the stage as lines of `state.txt`, `stage.<what>: <value>`.
    pub(super) fn write(&self, text: &mut String) {
        use std::fmt::Write as _;
        let yes = |on: bool| if on { "yes" } else { "no" };
        let panes = match self.panes {
            Panes::Page => "page",
            Panes::Source => "source",
            Panes::Split => "split",
        };
        let _ = writeln!(text, "stage.file: {}", self.file);
        let _ = writeln!(text, "stage.cursor: {}", self.cursor);
        let _ = writeln!(text, "stage.insert: {}", yes(self.insert));
        for open in &self.open {
            let _ = writeln!(text, "stage.open: {open}");
        }
        let _ = writeln!(text, "stage.layout: {panes}");
        let _ = writeln!(text, "stage.slides: {}", yes(self.slides));
        let _ = writeln!(text, "stage.fit: {}", yes(self.fit));
        let _ = writeln!(text, "stage.zoom: {}", self.zoom);
        let _ = writeln!(text, "stage.top: {}", self.top);
        let _ = writeln!(text, "stage.center: {}", self.center);
        let _ = writeln!(text, "stage.ui: {}", self.ui);
        let _ = writeln!(text, "stage.window: {} {}", self.window.0, self.window.1);
        let _ = writeln!(text, "stage.display: {}", self.display);
        let _ = writeln!(text, "stage.parts: {}", yes(self.parts));
        let _ = writeln!(text, "stage.numbers: {}", yes(self.numbers));
        let _ = writeln!(text, "stage.rest: {}", yes(self.rest));
    }

    /// Reads what [`Stage::write`] wrote, among other lines.
    fn read(text: &str) -> Result<Self, String> {
        let mut stage = Self::default();
        let mut found = false;
        for line in text.lines() {
            let Some((what, value)) = line
                .strip_prefix("stage.")
                .and_then(|rest| rest.split_once(": "))
            else {
                continue;
            };
            found = true;
            let bad = || format!("state.txt: cannot read stage.{what}: {value}");
            let yes = || match value {
                "yes" => Ok(true),
                "no" => Ok(false),
                _ => Err(bad()),
            };
            let number = || value.parse::<f64>().map_err(|_| bad());
            match what {
                "file" => value.clone_into(&mut stage.file),
                "cursor" => stage.cursor = value.parse().map_err(|_| bad())?,
                "insert" => stage.insert = yes()?,
                "open" => stage.open.push(value.to_owned()),
                "layout" => {
                    stage.panes = match value {
                        "page" => Panes::Page,
                        "source" => Panes::Source,
                        "split" => Panes::Split,
                        _ => return Err(bad()),
                    }
                }
                "slides" => stage.slides = yes()?,
                "fit" => stage.fit = yes()?,
                "zoom" => stage.zoom = number()?,
                "top" => stage.top = number()?,
                "center" => stage.center = number()?,
                "ui" => stage.ui = number()?,
                "window" => {
                    let (width, height) = value.split_once(' ').ok_or_else(bad)?;
                    stage.window = (
                        width.parse().map_err(|_| bad())?,
                        height.parse().map_err(|_| bad())?,
                    );
                }
                "display" => stage.display = number()?,
                "parts" => stage.parts = yes()?,
                "numbers" => stage.numbers = yes()?,
                "rest" => stage.rest = yes()?,
                _ => {}
            }
        }
        if !found || stage.file.is_empty() {
            return Err("state.txt says nothing of the stage: no key was kept".to_owned());
        }
        Ok(stage)
    }
}

/// One step of a replay, in the order they happened.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// A key, as it came from the keyboard, and how long before it the
    /// key before it came (or the first moment).
    Key { press: KeyPress, pause: Duration },
    /// The text as it is is handed over to be compiled: the revision it
    /// had where the report was written (a replay counts its own).
    Began { revision: u64 },
    /// The compile of that revision landed, and took so long.
    Compiled { revision: u64, millis: f64 },
    /// The open files' texts were changed between keys, to these, and
    /// the cursor was then at this byte.
    TextSet {
        files: Vec<(String, String)>,
        cursor: Option<usize>,
    },
    /// The next key opens this file, which was so then.
    Opened(String, String),
    /// The cursor was put at this byte without a key.
    Cursor(usize),
}

/// A report read back.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub directory: PathBuf,
    /// The document's directory and main file, as they were.
    pub root: PathBuf,
    pub main: String,
    pub stage: Stage,
    pub steps: Vec<Step>,
    /// The open files at the first key, and as they were at the end.
    pub before: Vec<(String, String)>,
    pub after: Vec<(String, String)>,
    /// What else the document needs, by its name in the document's
    /// directory (or where it is, if it is elsewhere).
    pub needs: Vec<String>,
    pub font_set: FontSet,
    /// The settings file that was in force, if there was one.
    pub config: Option<String>,
    /// Where the replay cannot be what happened, as far as the report
    /// tells.
    pub inexact: Vec<String>,
}

impl Recorded {
    /// Reads the report in `directory`.
    pub fn read(directory: &Path) -> Result<Self, String> {
        let read = |name: &Path| {
            std::fs::read_to_string(directory.join(name))
                .map_err(|err| format!("cannot read {}: {err}", directory.join(name).display()))
        };
        let written = read(Path::new("state.txt"))?;
        let (written, config) = match written.split_once(CONFIG_MARK) {
            Some((before_mark, config)) => (
                before_mark.to_owned(),
                Some(config.strip_prefix('\n').unwrap_or(config).to_owned()),
            ),
            None => (written, None),
        };
        let mut stage = Stage::read(&written)?;
        stage.memory = read(Path::new("vim.txt")).unwrap_or_default();
        let field = |name: &str| {
            written
                .lines()
                .find_map(|line| line.strip_prefix(name)?.strip_prefix(": "))
                .map(str::to_owned)
        };
        let root = PathBuf::from(field("root").ok_or("state.txt does not say root")?);
        let main = field("main").ok_or("state.txt does not say main")?;
        let font_set = match field("font set").as_deref() {
            Some("EmbeddedAndSystem") => FontSet::EmbeddedAndSystem,
            _ => FontSet::Embedded,
        };
        let mut before = Vec::new();
        for name in &stage.open {
            before.push((name.clone(), read(&Path::new("before").join(name))?));
        }
        let mut after = Vec::new();
        for line in written.lines() {
            if let Some(open) = line.strip_prefix("open: ") {
                let name = open.trim_end_matches(" (changes not written)");
                after.push((name.to_owned(), read(&Path::new("after").join(name))?));
            }
        }
        let needs = read(Path::new("needs.txt"))
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
        let keys = read(Path::new("keys.txt"))?;
        let mut inexact = Vec::new();
        if !stage.rest {
            inexact.push(
                "the first key kept came while something was under way (typing, a list, \
                 pages Typst refused): the replay begins as near to it as it can"
                    .to_owned(),
            );
        }
        let steps = steps(&keys, directory, &mut inexact)?;
        Ok(Self {
            directory: directory.to_owned(),
            root,
            main,
            stage,
            steps,
            before,
            after,
            needs,
            font_set,
            config,
            inexact,
        })
    }

    /// Puts the document as it was at the first key into `directory`:
    /// the open files as they were, and what else it needs as it is now
    /// (copied; nothing of the original is changed).
    pub fn copy_to(&self, directory: &Path) -> Result<(), String> {
        for (name, text) in &self.before {
            write_inside(directory, name, text.as_bytes())?;
        }
        for need in &self.needs {
            let path = Path::new(need);
            let (from, name) = if path.is_absolute() {
                // Outside the document's directory: it cannot be opened
                // from the copy anyway.
                continue;
            } else {
                (self.root.join(path), need.as_str())
            };
            if self.before.iter().any(|(open, _)| open == name) {
                continue;
            }
            if let Ok(bytes) = std::fs::read(&from) {
                write_inside(directory, name, &bytes)?;
            }
        }
        Ok(())
    }

    /// The keymap of the settings in force.
    #[must_use]
    pub fn keymap(&self) -> super::super::Keymap {
        let Some(config) = &self.config else {
            return super::super::Keymap::default();
        };
        let Ok(table) = config.parse::<toml::Table>() else {
            return super::super::Keymap::default();
        };
        let pairs = |mode: &str| -> Vec<(String, String)> {
            let keys = table.get("keys").and_then(|keys| keys.get(mode));
            keys.and_then(toml::Value::as_table)
                .map(|keys| {
                    keys.iter()
                        .filter_map(|(typed, means)| {
                            Some((typed.clone(), means.as_str()?.to_owned()))
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let (normal, insert) = (pairs("normal"), pairs("insert"));
        let normal = normal.iter().map(|(a, b)| (a.as_str(), b.as_str()));
        let insert = insert.iter().map(|(a, b)| (a.as_str(), b.as_str()));
        super::super::Keymap::parse(normal, insert).0
    }
}

/// Writes `bytes` to `name` under `directory`, if `name` stays inside it.
pub fn write_inside(directory: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    let relative = Path::new(name);
    let inside = relative
        .components()
        .all(|part| matches!(part, std::path::Component::Normal(_)));
    if !inside {
        return Err(format!("{name} is not a name inside the document"));
    }
    let path = directory.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("cannot make {}: {err}", parent.display()))?;
    }
    std::fs::write(&path, bytes).map_err(|err| format!("cannot write {}: {err}", path.display()))
}

/// The steps `keys.txt` holds; the texts put in on the way are read from
/// `directory`. What a replay cannot do as it was goes to `inexact`.
fn steps(keys: &str, directory: &Path, inexact: &mut Vec<String>) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    let mut last_key = None;
    for (number, line) in keys.lines().enumerate() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let bad = |why: &str| format!("keys.txt, line {}: {why}: {line}", number + 1);
        let mut words = line.split_whitespace();
        let time: f64 = words
            .next()
            .and_then(|time| time.parse().ok())
            .ok_or_else(|| bad("no time"))?;
        let _gap = words.next();
        let kind = words.next().ok_or_else(|| bad("nothing happened"))?;
        // What follows the kind, as it is written.
        let rest = line
            .split_once(&format!(" {kind} "))
            .map_or("", |(_, rest)| rest);
        match kind {
            "key" => {
                let token = if rest.starts_with('<') {
                    rest.find('>').map(|end| &rest[..=end])
                } else {
                    rest.chars().next().map(|c| &rest[..c.len_utf8()])
                };
                let token = token.ok_or_else(|| bad("no key"))?;
                let press = *keys::parse(token)
                    .map_err(|why| bad(&why))?
                    .first()
                    .ok_or_else(|| bad("no key"))?;
                let since = last_key.map_or(time, |last| time - last);
                last_key = Some(time);
                steps.push(Step::Key {
                    press,
                    pause: Duration::from_secs_f64(since.max(0.0)),
                });
            }
            "report-key" => last_key = Some(time),
            "hidden-key" => {
                last_key = Some(time);
                inexact.push("a key typed into a prompt that is not recorded".to_owned());
            }
            "compile" => {
                let revision = rest
                    .split_whitespace()
                    .next()
                    .and_then(|word| word.strip_prefix('r')?.parse().ok())
                    .ok_or_else(|| bad("no revision"))?;
                steps.push(Step::Began { revision });
            }
            "compiled" => {
                let revision = rest
                    .split_whitespace()
                    .next()
                    .and_then(|word| word.strip_prefix('r')?.parse().ok())
                    .ok_or_else(|| bad("no revision"))?;
                let millis = rest
                    .split_whitespace()
                    .zip(rest.split_whitespace().skip(1))
                    .find_map(|(number, unit)| {
                        (unit.starts_with("ms")).then(|| number.parse().ok())?
                    })
                    .unwrap_or(3.0);
                steps.push(Step::Compiled { revision, millis });
            }
            "text-set" => {
                let mut words = rest.split_whitespace();
                let at = words.next().ok_or_else(|| bad("no number"))?;
                let cursor = words
                    .next()
                    .and_then(|word| word.strip_prefix('@')?.parse().ok());
                let files = files_under(&directory.join("between").join(at))?;
                steps.push(Step::TextSet { files, cursor });
            }
            "opened" => {
                let mut words = rest.split_whitespace();
                let at = words.next().ok_or_else(|| bad("no number"))?;
                let name = words.next().ok_or_else(|| bad("no file"))?;
                let path = directory.join("between").join(at).join(name);
                let text = std::fs::read_to_string(&path)
                    .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
                steps.push(Step::Opened(name.to_owned(), text));
            }
            "cursor" => {
                let cursor = rest
                    .split_whitespace()
                    .next()
                    .and_then(|word| word.strip_prefix('@')?.parse().ok())
                    .ok_or_else(|| bad("no place"))?;
                steps.push(Step::Cursor(cursor));
            }
            "disk" => inexact.push(format!("a file changed on disk: {rest}")),
            _ => {}
        }
    }
    Ok(steps)
}

/// The files under `directory`, by their names in it.
fn files_under(directory: &Path) -> Result<Vec<(String, String)>, String> {
    let mut found = Vec::new();
    let mut waiting = vec![directory.to_owned()];
    while let Some(next) = waiting.pop() {
        let entries = std::fs::read_dir(&next)
            .map_err(|err| format!("cannot read {}: {err}", next.display()))?;
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                waiting.push(path);
            } else if let Ok(text) = std::fs::read_to_string(&path) {
                let name = path.strip_prefix(directory).unwrap_or(&path);
                found.push((name.to_string_lossy().into_owned(), text));
            }
        }
    }
    found.sort();
    Ok(found)
}

/// What a replay in a viewer led to.
#[derive(Debug, Clone)]
pub struct Replayed {
    /// The open files and their texts, at the end of the replay, and as
    /// they were at the end of what the report recorded.
    pub texts: Vec<(String, String)>,
    pub expected: Vec<(String, String)>,
    /// Where it could not be what happened.
    pub inexact: Vec<String>,
}

impl Replayed {
    /// How the texts differ from what they were, if they do: a line for
    /// each file.
    #[must_use]
    pub fn differences(&self) -> Vec<String> {
        differences(&self.texts, &self.expected)
    }
}

/// How `texts` differ from `expected`: a line for each file that does.
#[must_use]
pub fn differences(texts: &[(String, String)], expected: &[(String, String)]) -> Vec<String> {
    let mut differ = Vec::new();
    for (name, want) in expected {
        match texts.iter().find(|(open, _)| open == name) {
            None => differ.push(format!("{name} is not open")),
            Some((_, text)) if text != want => {
                let at = text
                    .char_indices()
                    .zip(want.chars())
                    .find(|((_, a), b)| a != b)
                    .map_or(text.len().min(want.len()), |((at, _), _)| at);
                let line = text[..at].matches('\n').count() + 1;
                let near = |text: &str| {
                    let from = text.floor_char_boundary(at.saturating_sub(20));
                    let to = text.ceil_char_boundary((at + 20).min(text.len()));
                    text.get(from..to).unwrap_or("").to_owned()
                };
                differ.push(format!(
                    "{name} differs from line {line} (byte {at}): {:?} where it was {:?}",
                    near(text),
                    near(want)
                ));
            }
            Some(_) => {}
        }
    }
    differ
}

/// Plays the report in `report` again in a viewer on this thread, in a
/// copy of the document under `scratch` (with a state directory of its
/// own there): sets the stage, presses the keys, compiles where compiles
/// were begun and delivers them where they landed. For tests: a report
/// that the user wrote becomes a test by asserting that
/// [`Replayed::differences`] is empty (or what it should be once the bug
/// is mended).
pub fn replay(report: &Path, scratch: &Path) -> Result<Replayed, String> {
    replay_with(report, scratch, |_, _| {})
}

/// Like [`replay`], with `each` told of every step once it is taken, and
/// of the viewer after it: to find the key a replay goes otherwise at.
pub fn replay_with(
    report: &Path,
    scratch: &Path,
    mut each: impl FnMut(&Step, &Viewer),
) -> Result<Replayed, String> {
    let recorded = Recorded::read(report)?;
    let document = scratch.join("document");
    let journals = scratch.join("state");
    let _ = std::fs::remove_dir_all(&document);
    let _ = std::fs::remove_dir_all(&journals);
    recorded.copy_to(&document)?;
    let env = Environment::new(recorded.font_set);
    let main = document.join(&recorded.main);
    // The file opened first, as it was; the stage opens the others.
    let first = recorded.stage.open.first().unwrap_or(&recorded.stage.file);
    let edited = document.join(first);
    let project = Project::open(Arc::clone(&env), &main).map_err(|err| err.to_string())?;
    let file = project
        .file_id(&edited)
        .ok_or_else(|| format!("{first} is not in the document"))?;
    let (editor, opened) = Editor::open(&edited, &journals, file).map_err(|err| err.to_string())?;
    let stage = &recorded.stage;
    let mut viewer = Viewer::new(env, project, stage.window, stage.display)
        .ok_or("no usable fonts")?
        .editing(editor, &opened);
    viewer.state_dir = Some(journals);
    viewer.set_keymap(recorded.keymap());
    // The document had been typeset before the first key: so here.
    compile(&mut viewer, None, 3.0);
    viewer.report_stage(stage);
    compile(&mut viewer, None, 3.0);
    let mut begun = Vec::new();
    for step in &recorded.steps {
        match step {
            Step::Key { press, .. } => viewer.key(*press),
            Step::Began { revision } => {
                viewer.report_began();
                begun.push((*revision, viewer.snapshot()));
            }
            Step::Compiled { revision, millis } => {
                // Of what was handed over then; or, where that was not
                // recorded (a test compiles as it pleases), of the text
                // as it is.
                let at = begun.iter().rposition(|(of, _)| of == revision);
                let snapshot = at.map(|at| begun.swap_remove(at).1);
                begun.clear();
                compile(&mut viewer, snapshot, *millis);
            }
            Step::TextSet { files, cursor } => viewer.report_set_texts(files, *cursor),
            Step::Cursor(cursor) => viewer.report_set_texts(&[], Some(*cursor)),
            Step::Opened(name, text) => write_inside(&document, name, text.as_bytes())?,
        }
        each(step, &viewer);
    }
    Ok(Replayed {
        texts: viewer.report_texts(),
        expected: recorded.after.clone(),
        inexact: recorded.inexact,
    })
}

/// Compiles `begun` (or the text as it is) and gives the viewer what came
/// of it, as if it took `millis`.
fn compile(viewer: &mut Viewer, begun: Option<(u64, zetwerk_typst::ProjectWorld)>, millis: f64) {
    let (revision, world) = begun.unwrap_or_else(|| viewer.snapshot());
    let compiled = zetwerk_typst::compile(&world);
    viewer.compiled(CompileOutput {
        revision,
        world: Arc::new(world),
        compiled,
        duration: Duration::from_secs_f64(millis.max(0.0) / 1000.0),
    });
}

impl Viewer {
    /// The stage as it is now (see [`Stage`]).
    pub(super) fn stage(&self) -> Stage {
        let name = |file: typst::syntax::FileId| file.vpath().get_without_slash().to_owned();
        let editor = self.editor.as_ref();
        Stage {
            file: editor.map(|editor| name(editor.file())).unwrap_or_default(),
            cursor: editor.map_or(0, Editor::cursor),
            insert: editor.is_some_and(|editor| editor.mode() == editor::Mode::Insert),
            open: self.opened.iter().map(|file| name(*file)).collect(),
            panes: self.panes,
            slides: self.paging == Paging::Slides,
            fit: self.fit,
            zoom: self.view.zoom,
            top: self.view.top,
            center: self.view.center,
            ui: self.ui,
            window: self.window,
            display: self.display,
            parts: self.in_parts,
            numbers: self.numbers,
            memory: editor
                .map(|editor| editor.engine().memory())
                .unwrap_or_default(),
            rest: self.at_rest(),
        }
    }

    /// Sets the stage as it was at the first key of a report: the files
    /// open, the one edited, the cursor (typing, if it was), the layout,
    /// the zoom and the view. The window's size is the window's.
    pub fn report_stage(&mut self, stage: &Stage) {
        let root = self.project.root_dir().to_owned();
        for name in &stage.open {
            self.open_path(&root.join(name), false);
        }
        self.open_path(&root.join(&stage.file), false);
        if self.editor.is_some() {
            self.panes = stage.panes;
        }
        self.in_parts = stage.parts;
        self.numbers = stage.numbers;
        let paging = if stage.slides {
            Paging::Slides
        } else {
            Paging::Scroll
        };
        if paging != self.paging {
            self.set_paging(paging);
        }
        self.set_ui_scale(stage.ui);
        self.fit = stage.fit;
        if !stage.fit {
            self.view.zoom = stage.zoom;
            self.planned.clear();
        }
        self.arrange();
        self.view.top = stage.top;
        self.view.center = stage.center;
        self.view.clamp(&self.layout);
        let Some(editor) = &mut self.editor else {
            return;
        };
        if !stage.memory.is_empty() {
            editor.recall(&stage.memory);
        }
        let text = editor.text();
        let cursor = text.floor_char_boundary(stage.cursor.min(text.len()));
        if !stage.insert {
            editor.set_cursor(cursor);
            return;
        }
        // Typing at the end of a line is typing behind its last
        // character: `a` there, `i` anywhere else.
        let line_ends = matches!(text[cursor..].chars().next(), None | Some('\n'));
        let before = text[..cursor].chars().next_back().filter(|c| *c != '\n');
        let (at, key) = match before {
            Some(c) if line_ends => (cursor - c.len_utf8(), 'a'),
            _ => (cursor, 'i'),
        };
        editor.set_cursor(at);
        self.plain_key(KeyPress::plain(Key::Char(key)));
    }

    /// The open files, by their names in the document's directory, and
    /// their texts.
    #[must_use]
    pub fn report_texts(&self) -> Vec<(String, String)> {
        let editors = self.editor.iter().chain(&self.others);
        let mut texts: Vec<(String, String)> = editors
            .map(|editor| {
                let name = editor.file().vpath().get_without_slash().to_owned();
                (name, editor.text().to_owned())
            })
            .collect();
        texts.sort();
        texts
    }

    /// Puts `files` (names and texts) into the open files of those names
    /// that differ, and the cursor at byte `cursor` of the file being
    /// edited: as texts changed between keys are put in by a replay.
    pub fn report_set_texts(&mut self, files: &[(String, String)], cursor: Option<usize>) {
        let mut changed = false;
        let editors = self.editor.iter_mut().chain(&mut self.others);
        for editor in editors {
            let name = editor.file().vpath().get_without_slash().to_owned();
            let Some((_, text)) = files.iter().find(|(file, _)| *file == name) else {
                continue;
            };
            if editor.text() != text {
                let whole = 0..editor.text().len();
                changed |= editor.replace(&[(whole, text)]).is_ok();
            }
        }
        if changed {
            self.text_changed(super::super::Edited::Command);
        }
        if let (Some(cursor), Some(editor)) = (cursor, &mut self.editor) {
            let text = editor.text();
            editor.set_cursor(text.floor_char_boundary(cursor.min(text.len())));
        }
    }
}
