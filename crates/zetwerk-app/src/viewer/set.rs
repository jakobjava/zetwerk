//! `:set`: every option of the window, in a panel or on the command
//! line, as Vim users expect; the sets of aids over the page; and what
//! is changed kept in the settings file.
//!
//! `:set` (or `g?` and its first entry) puts up all options, grouped by
//! what they are about, each with its value and a mark where it is the
//! user's own and not what holds anyway. `j` and `k` go through them,
//! `/` narrows them by what is typed; Enter or Space changes the chosen
//! one: a switch at once, a choice or a number by asking. The page
//! behind the panel shows the change at once. `x` makes one what it
//! usually is, `u` takes back the last change, `a` goes to the next set
//! of aids. The user's own keys are listed among the keys: `x` takes one
//! away, and "a key of yours" asks for the keys to press and then for
//! the keys they stand for.
//!
//! On the command line: `:set numbers`, `:set nonumbers`, `:set
//! numbers!`, `:set numbers?`, `:set numbers&`, `:set pause=300`, `:set
//! pause+=100`, several at once; the faint completion behind the cursor
//! completes names and choices.
//!
//! The aids over the page come in three sets (see `options` for why each
//! is in which): quiet, the usual, shows the page and the cursor, and
//! besides only what is wrong and what a key asked for; writing adds
//! spelling, the rest of a word and the tags in the margin; everything
//! shows all. `za` goes round them. A set decides what an aid is unless
//! the user has said otherwise of it; choosing a set takes back what
//! was said of single aids, so that the set is what is seen (`u`
//! brings it back).
//!
//! What is changed is written into the settings file (see
//! [`crate::settings::Writer`]): only what is not what holds anyway, the
//! rest of the file as it was. So are changes made by the commands of
//! old (`:numbers`, `:spell`, `:ui 1.5`): a command's effect on the
//! options is looked at before and after it. Commands that change an
//! option for the time being only (`:slides`, `gs`) are left out.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use super::assist::Target;
use super::keymap::Keymap;
use super::{PANEL_MARGIN, Scene, TextureId, Viewer};
use crate::editor;
use crate::keys::{Key, KeyPress};
use crate::settings::{Edit, Group, Kind, Opt, Preset, Stored, Value, When, Writer, find, table};

/// How many rows of the panel are shown at once.
const ROWS: usize = 18;
/// How wide the panel is, in points.
const WIDTH: f64 = 340.0;
/// The colours of the interface (as in `chrome`).
const GROUND: &str = "#282828";
const TEXT: &str = "#ebdbb2";
const DIM: &str = "#928374";
const ACCENT: &str = "#fabd2f";
const CHOSEN: &str = "#504945";
const OWN: &str = "#83a598";
/// The file in the state directory that says the user was told that the
/// page has become quieter.
const TOLD: &str = "told-quieter";
/// What they are told, once.
pub(super) const QUIETER: &str =
    "the page is quieter now: :set aids=everything brings all aids back (:set lists all)";

/// What the viewer keeps of the settings.
pub(super) struct Settings {
    /// The set of aids that decides those the user said nothing of.
    pub(super) preset: Preset,
    /// Whether the numbers of the lines show while typing too.
    pub(super) numbers_typing: bool,
    /// How many lines up and down are numbered.
    pub(super) reach: usize,
    /// Whether empty places say what they are for.
    pub(super) place_words: bool,
    /// How long keys pause before what can follow them is listed.
    pub(super) pause: Duration,
    /// Whether the rest of a word in running text is shown.
    pub(super) ghost_words: bool,
    /// Whether Tab takes it.
    pub(super) tab_words: bool,
    /// Whether `$` begins an equation in running text.
    pub(super) dollar: bool,
    /// Whether equations are tidied after keys.
    pub(super) tidy: bool,
    /// How long keys of a mapping begun wait, in milliseconds.
    pub(super) timeout: u64,
    /// The values of options the viewer has no other place for.
    held: BTreeMap<&'static str, Value>,
    /// What the settings file says, as far as it differs from what holds
    /// anyway.
    explicit: BTreeMap<&'static str, Value>,
    /// The user's own keys, as written.
    pub(super) normal: Vec<(String, String)>,
    pub(super) insert: Vec<(String, String)>,
    writer: Option<Writer>,
    file: Option<PathBuf>,
    /// The command being run, and what all options were before it.
    before: Option<(String, Vec<Value>)>,
    /// What `u` in the panel takes back, the last last.
    undo: Vec<Undo>,
    /// The panel, while it is up.
    panel: Option<Panel>,
    /// The revision of the text when typing began, while it goes on.
    typing_from: Option<u64>,
    /// Whether it was said that Ctrl-Y takes the rest of a word.
    told_tab: bool,
}

impl Default for Settings {
    /// As the viewer was before settings were kept: all aids, Tab takes
    /// words. `configured` puts the user's settings in their place.
    fn default() -> Self {
        Self {
            preset: Preset::Everything,
            numbers_typing: true,
            reach: 99,
            place_words: true,
            pause: super::hints::PAUSE,
            ghost_words: true,
            tab_words: true,
            dollar: true,
            tidy: true,
            timeout: 1000,
            held: BTreeMap::new(),
            explicit: BTreeMap::new(),
            normal: Vec::new(),
            insert: Vec::new(),
            writer: None,
            file: None,
            before: None,
            undo: Vec::new(),
            panel: None,
            typing_from: None,
            told_tab: false,
        }
    }
}

impl Settings {
    pub(super) fn held(&self, name: &str) -> Option<&Value> {
        self.held.get(name)
    }

    pub(super) fn hold(&mut self, name: &'static str, value: Value) {
        self.held.insert(name, value);
    }

    /// What typing does of itself, as the user wants it.
    pub(super) fn rules(&self) -> editor::Rules {
        editor::Rules {
            dollar: self.dollar,
            tidy: self.tidy,
        }
    }
}

/// A change `u` in the panel takes back.
enum Undo {
    /// Options as they were: each with what the file said of it and its
    /// value; and the set of aids.
    Options(Vec<(&'static str, Option<Value>, Value)>, Preset),
    /// A key of the user's: of typing or not, the keys, and what they
    /// stood for before, if anything.
    Mapping {
        insert: bool,
        keys: String,
        before: Option<String>,
    },
}

/// The panel of all options.
#[derive(Default)]
struct Panel {
    /// What it is narrowed by, and whether that is being typed.
    query: String,
    typing: bool,
    /// The row chosen, of those shown.
    chosen: usize,
    /// A key of the user's being made.
    capture: Option<Capture>,
}

/// A key of the user's being made: for typing or not, the keys pressed
/// so far, the keys they are to stand for once those are done, and
/// whether the next key is taken as it is (after Ctrl-V).
struct Capture {
    insert: bool,
    pressed: Vec<KeyPress>,
    keys: Option<Vec<KeyPress>>,
    literal: bool,
}

/// A row of the panel.
#[derive(Clone)]
pub(super) enum Row {
    Heading(Group),
    Opt(&'static Opt),
    /// A key of the user's: of typing or not, the keys and what they
    /// stand for.
    Mapping {
        insert: bool,
        keys: String,
        does: String,
    },
    /// Makes a key of the user's.
    Add(bool),
    /// Leads to the document manager.
    Document(&'static str),
    /// Only says something.
    Info(String),
}

/// `text` as a Typst string literal.
fn literal(text: &str) -> String {
    let mut written = String::with_capacity(text.len() + 2);
    written.push('"');
    for c in text.chars() {
        match c {
            '\\' | '"' => {
                written.push('\\');
                written.push(c);
            }
            '\n' => written.push_str("\\n"),
            _ => written.push(c),
        }
    }
    written.push('"');
    written
}

/// What the keys of normal mode do by themselves, for saying what a
/// mapping of the user's hides.
const NORMAL_KEYS: &[(char, &str)] = &[
    ('h', "left"),
    ('j', "down"),
    ('k', "up"),
    ('l', "right"),
    ('w', "a word on"),
    ('b', "a word back"),
    ('e', "to the end of a word"),
    ('W', "a WORD on"),
    ('B', "a WORD back"),
    ('E', "to the end of a WORD"),
    ('0', "to the start of the line"),
    ('^', "to the first character"),
    ('$', "to the end of the line"),
    ('f', "to a character"),
    ('F', "back to a character"),
    ('t', "up to a character"),
    ('T', "back up to a character"),
    (';', "the last f or t again"),
    (',', "the last f or t back"),
    ('G', "to the last line"),
    ('x', "delete a character"),
    ('X', "delete the character before"),
    ('d', "delete"),
    ('D', "delete to the end of the line"),
    ('c', "change"),
    ('C', "change to the end of the line"),
    ('y', "yank"),
    ('Y', "yank the line"),
    ('p', "put behind"),
    ('P', "put before"),
    ('u', "undo"),
    ('U', "undo the line"),
    ('r', "replace a character"),
    ('R', "replace as typed"),
    ('s', "change a character"),
    ('S', "change the line"),
    ('i', "type before the cursor"),
    ('I', "type at the start of the line"),
    ('a', "type behind the cursor"),
    ('A', "type at the end of the line"),
    ('o', "type in a new line below"),
    ('O', "type in a new line above"),
    ('v', "select"),
    ('V', "select lines"),
    ('J', "join lines"),
    ('.', "the last change again"),
    ('n', "the next match"),
    ('N', "the match before"),
    ('*', "search for the word here"),
    ('#', "search back for the word here"),
    ('/', "search"),
    ('?', "search back"),
    (':', "a command"),
    ('%', "to the matching bracket"),
    ('m', "a mark"),
    ('`', "to a mark"),
    ('\'', "to the line of a mark"),
    ('q', "record keys"),
    ('@', "play keys"),
    ('~', "the other case"),
    ('<', "dedent"),
    ('>', "indent"),
    ('{', "a paragraph back"),
    ('}', "a paragraph on"),
    ('K', "what this is"),
    ('+', "zoom in"),
    ('-', "zoom out"),
    ('=', "zoom to the width"),
    ('"', "a register"),
    ('g', "the keys after g"),
    ('z', "the keys after z"),
    ('Z', "the keys after Z"),
    (']', "the keys after ]"),
    ('[', "the keys after ["),
];

/// Control keys of normal mode, and of typing, that do something.
const NORMAL_CONTROL: &[(char, &str)] = &[
    ('d', "half a page down"),
    ('u', "half a page up"),
    ('f', "a page down"),
    ('b', "a page up"),
    ('e', "a line down"),
    ('y', "a line up"),
    ('r', "redo"),
    ('o', "back to where a jump came from"),
    ('i', "on again"),
    ('a', "a number larger"),
    ('x', "a number smaller"),
    ('v', "select a block"),
    ('c', "stop"),
];
const INSERT_CONTROL: &[(char, &str)] = &[
    ('w', "delete the word before"),
    ('u', "delete to the start of the line"),
    ('n', "the next completion"),
    ('p', "the completion before"),
    ('y', "take the completion"),
    ('e', "no completion"),
    ('r', "put a register"),
    ('o', "one command"),
    ('b', "bold"),
    ('i', "italic"),
    ('h', "delete a character"),
    ('t', "indent"),
    ('d', "dedent"),
    ('v', "a key as it is"),
];

/// What the keys `keys` of a mapping hide of what the editor does by
/// itself, in normal mode or (`insert`) while typing; nothing where
/// they hide nothing.
fn hides(insert: bool, keys: &[KeyPress]) -> Option<String> {
    let first = keys.first()?;
    let Key::Char(c) = first.key else {
        return None;
    };
    if first.ctrl {
        let table = if insert {
            INSERT_CONTROL
        } else {
            NORMAL_CONTROL
        };
        let (_, does) = table
            .iter()
            .find(|(key, _)| *key == c.to_ascii_lowercase())?;
        let what = if keys.len() == 1 {
            format!("hides Ctrl-{c}: {does}")
        } else {
            format!("Ctrl-{c} ({does}) now waits for the next key")
        };
        return Some(what);
    }
    if insert {
        return Some(if keys.len() == 1 {
            format!("{c} is no longer typed as it is")
        } else {
            format!("typing {c} now waits for the next key before it shows")
        });
    }
    if keys.len() >= 2
        && matches!(c, 'g' | 'z' | 'Z' | ']' | '[')
        && let Key::Char(second) = keys[1].key
        && !keys[1].ctrl
    {
        let after = zetwerk_vim::keys_after(c);
        let found = after.iter().find(|(listed, _)| {
            listed
                .split(' ')
                .any(|key| key.chars().eq(std::iter::once(second)))
        });
        return found.map(|(_, does)| format!("hides {c}{second}: {does}"));
    }
    let (_, does) = NORMAL_KEYS.iter().find(|(key, _)| *key == c)?;
    Some(if keys.len() == 1 {
        format!("hides {c}: {does}")
    } else {
        format!("{c} ({does}) now waits for the next key")
    })
}

/// The arguments of `:set`, apart by blanks; a blank after a backslash
/// is part of one.
fn arguments(line: &str) -> Vec<String> {
    let mut all = Vec::new();
    let mut one = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&' ') => {
                one.push(' ');
                chars.next();
            }
            ' ' => {
                if !one.is_empty() {
                    all.push(std::mem::take(&mut one));
                }
            }
            _ => one.push(c),
        }
    }
    if !one.is_empty() {
        all.push(one);
    }
    all
}

/// `value` as `:set` takes it: blanks behind a backslash.
fn typed(value: &str) -> String {
    value.replace(' ', "\\ ")
}

/// The longest beginning `a` and `b` have in common.
fn common<'a>(a: &'a str, b: &str) -> &'a str {
    let same = a
        .char_indices()
        .zip(b.chars())
        .find(|((_, x), y)| x != y)
        .map_or(a.len().min(b.len()), |((at, _), _)| at);
    &a[..same]
}

/// What `:set` makes of one argument.
enum Wish {
    /// The name alone: on, or what it is.
    Bare,
    Ask,
    To(String),
    Off,
    Toggle,
    Usual,
    Add(f64),
}

impl Viewer {
    /// Takes the user's settings from what the settings file said, and
    /// puts them into the viewer: what it says, and what holds where it
    /// says nothing. `complained` if something else is to be told first.
    pub(super) fn configure_settings(&mut self, stored: &Stored, complained: bool) {
        let message = self.message.take();
        self.settings.writer.clone_from(&stored.writer);
        self.settings.file.clone_from(&stored.file);
        self.settings.normal.clone_from(&stored.normal);
        self.settings.insert.clone_from(&stored.insert);
        for (name, value) in &stored.explicit {
            if let Some(opt) = find(name) {
                self.settings.explicit.insert(opt.name, value.clone());
            }
        }
        let preset = self
            .settings
            .explicit
            .get("aids.preset")
            .and_then(|value| Preset::named(value.text()))
            .unwrap_or(Preset::Quiet);
        self.settings.preset = preset;
        for opt in table().iter().filter(|opt| opt.name != "aids.preset") {
            let wanted = self
                .settings
                .explicit
                .get(opt.name)
                .cloned()
                .unwrap_or_else(|| opt.usual_for(preset));
            if (opt.get)(self) != wanted {
                (opt.set)(self, &wanted);
            }
        }
        self.message = message;
        self.settings.before = None;
        self.tell_quieter(stored, complained);
    }

    /// Says once, to whoever had settings before the page became quiet,
    /// that it did.
    fn tell_quieter(&mut self, stored: &Stored, complained: bool) {
        let shown = self.text().is_some_and(|text| !text.trim().is_empty());
        let Some(state) = self.state_dir.clone().filter(|_| stored.window && shown) else {
            return;
        };
        let told = state.join(TOLD);
        if complained || told.exists() {
            return;
        }
        let _ = std::fs::create_dir_all(&state);
        if std::fs::write(&told, "").is_err() {
            return;
        }
        if stored.existed && !self.settings.explicit.contains_key("aids.preset") {
            self.notice(QUIETER);
        }
    }

    /// Makes `preset` the set of aids, and each aid the user said nothing
    /// of what it is in that set.
    pub(super) fn apply_preset(&mut self, preset: Preset) {
        self.settings.preset = preset;
        for opt in table().iter().filter(|opt| opt.by_preset()) {
            if self.settings.explicit.contains_key(opt.name) {
                continue;
            }
            let wanted = opt.usual_for(preset);
            if (opt.get)(self) != wanted {
                (opt.set)(self, &wanted);
            }
        }
    }

    /// Whether the numbers of the lines rest: they show in normal mode,
    /// and are not drawn while typing once a key has been typed.
    pub(super) fn numbers_resting(&self) -> bool {
        let typed = |from: u64| {
            self.editor
                .as_ref()
                .is_some_and(|editor| editor.revision() != from)
        };
        !self.settings.numbers_typing && self.settings.typing_from.is_some_and(typed)
    }

    /// Before command `command` runs: what the options are, to keep what
    /// it changes of them.
    pub(super) fn settings_before(&mut self, command: &str) {
        if self.settings.before.is_some() {
            return;
        }
        let values = table().iter().map(|opt| (opt.get)(self)).collect();
        self.settings.before = Some((command.to_owned(), values));
    }

    /// After every key: when typing began, and what the commands it ran
    /// changed of the options, to be kept.
    pub(super) fn settings_keyed(&mut self) {
        let typing = self
            .editor
            .as_ref()
            .filter(|editor| editor.mode() == editor::Mode::Insert)
            .map(editor::Editor::revision);
        match (typing, self.settings.typing_from) {
            (Some(revision), None) => self.settings.typing_from = Some(revision),
            (None, _) => self.settings.typing_from = None,
            (Some(_), Some(_)) => {}
        }
        if self.numbers_resting() {
            // Counted anew once typing is over: what was counted before
            // it is not where the lines are now.
            self.numbered = None;
        }
        let Some((command, values)) = self.settings.before.take() else {
            return;
        };
        let name = command.split(' ').next().unwrap_or_default();
        let mut changed = Vec::new();
        let mut before = Vec::new();
        for (opt, was) in table().iter().zip(values) {
            let now = (opt.get)(self);
            let session = opt.session.contains(&name) || opt.session.contains(&command.as_str());
            if now != was && !session {
                before.push((opt.name, self.settings.explicit.get(opt.name).cloned(), was));
                changed.push((opt, now));
            }
        }
        if !changed.is_empty() {
            self.keep(&changed);
            let preset = self.settings.preset;
            self.settings.undo.push(Undo::Options(before, preset));
        }
    }

    /// Notes that options are now `values`: in what the file says, and
    /// in the file, only where they are not what holds anyway.
    fn keep(&mut self, values: &[(&'static Opt, Value)]) {
        let preset = self.settings.preset;
        let mut edits = Vec::new();
        for (opt, value) in values {
            let path = opt.name.split('.').map(str::to_owned).collect();
            let written = opt.written(value);
            if *value == opt.usual_for(preset) || written.is_none() {
                self.settings.explicit.remove(opt.name);
                edits.push(Edit { path, value: None });
            } else {
                self.settings.explicit.insert(opt.name, value.clone());
                edits.push(Edit {
                    path,
                    value: written,
                });
            }
        }
        self.write_settings(&edits);
    }

    /// Writes `edits` into the settings file, if there is one to write.
    fn write_settings(&mut self, edits: &[Edit]) {
        let Some(writer) = &self.settings.writer else {
            return;
        };
        if let Err(why) = writer(edits) {
            self.alert(format!("the settings are not kept: {why}"));
        }
    }

    /// Makes `opt` be `value`, as the user asked: in the viewer, in the
    /// settings file; `u` in the panel takes it back.
    fn change(&mut self, opt: &'static Opt, value: &Value) {
        if opt.name == "aids.preset" {
            if let Some(preset) = Preset::named(value.text()) {
                self.change_preset(preset);
            }
            return;
        }
        let before = vec![(
            opt.name,
            self.settings.explicit.get(opt.name).cloned(),
            (opt.get)(self),
        )];
        (opt.set)(self, value);
        let now = (opt.get)(self);
        self.keep(&[(opt, now.clone())]);
        let preset = self.settings.preset;
        self.settings.undo.push(Undo::Options(before, preset));
        self.tell(opt, &now);
    }

    /// Makes `preset` the set of aids: what was said of single aids goes,
    /// so that the set is what is seen.
    fn change_preset(&mut self, preset: Preset) {
        let mut before = Vec::new();
        let mut gone = 0;
        let mut edits = Vec::new();
        for opt in table()
            .iter()
            .filter(|opt| opt.by_preset() || opt.name == "aids.preset")
        {
            let said = self.settings.explicit.remove(opt.name);
            if said.is_some() && opt.name != "aids.preset" {
                gone += 1;
            }
            before.push((opt.name, said, (opt.get)(self)));
            edits.push(Edit {
                path: opt.name.split('.').map(str::to_owned).collect(),
                value: None,
            });
        }
        let was = self.settings.preset;
        self.apply_preset(preset);
        if preset != Preset::Quiet {
            let value = Value::Text(preset.name().to_owned());
            self.settings.explicit.insert("aids.preset", value.clone());
            if let Some(last) = edits
                .iter_mut()
                .find(|edit| edit.path == ["aids", "preset"])
            {
                last.value = Some(crate::settings::Written::Text(preset.name().to_owned()));
            }
        }
        self.write_settings(&edits);
        self.settings.undo.push(Undo::Options(before, was));
        let what = match preset {
            Preset::Quiet => "aids: quiet — the page, and what is wrong or asked for",
            Preset::Writing => "aids: writing — and spelling, word endings, tags in the margin",
            Preset::Everything => "aids: everything",
        };
        let gone = match gone {
            0 => String::new(),
            1 => "; what you said of one aid gave way (u in :set brings it back)".to_owned(),
            n => format!("; what you said of {n} aids gave way (u in :set brings it back)"),
        };
        self.notice(format!("{what}  (za: the next){gone}"));
    }

    /// Says what `opt` is now, and where that is kept.
    fn tell(&mut self, opt: &Opt, value: &Value) {
        let kept = self.kept(opt);
        self.notice(format!("{}: {}  ({kept})", opt.short(), value.shown()));
    }

    /// Where what `opt` is now is kept, in words.
    fn kept(&self, opt: &Opt) -> String {
        let (section, key) = opt.place();
        let next = if opt.when == When::NextStart {
            ", from the next start"
        } else {
            ""
        };
        if !self.settings.explicit.contains_key(opt.name) {
            return format!("as usual{next}");
        }
        match &self.settings.file {
            Some(file) if self.settings.writer.is_some() => {
                let name = file.file_name().map_or_else(
                    || file.display().to_string(),
                    |name| name.to_string_lossy().into_owned(),
                );
                format!("kept in {name}, [{section}] {key}{next}")
            }
            _ => format!("for now: there is no settings file{next}"),
        }
    }

    /// `:set` and its forms. Returns whether `command` was one.
    pub(super) fn set_command(&mut self, command: &str) -> bool {
        let rest = match command.split_once(' ') {
            Some(("set" | "se", rest)) => rest.trim(),
            None if matches!(command, "set" | "se") => "",
            _ => return false,
        };
        // Its own: the panel at an option, after a list of choices.
        if let Some(name) = rest.strip_prefix("panel ") {
            self.open_settings(Some(name.trim()));
            return true;
        }
        if rest.is_empty() || rest == "all" {
            self.open_settings(None);
            self.settings.before = None;
            return true;
        }
        // A text is all that is behind its `=`, blanks and all.
        let single = rest.split_once('=').and_then(|(name, value)| {
            let opt = find(name.trim())?;
            matches!(opt.kind, Kind::Text | Kind::List).then(|| (opt, value.trim().to_owned()))
        });
        let wishes = match single {
            Some((opt, value)) => vec![(opt.name.to_owned(), Some((opt, Wish::To(value))))],
            None => arguments(rest)
                .into_iter()
                .map(|argument| {
                    let read = Self::wish(&argument);
                    (argument, read)
                })
                .collect(),
        };
        let mut told = Vec::new();
        for (argument, wish) in wishes {
            let Some((opt, wish)) = wish else {
                let name = argument.trim_end_matches(['?', '!', '&']);
                let name = name.split(['=', ':', '+', '-']).next().unwrap_or(name);
                self.alert(format!("no option {name} (:set lists them all)"));
                self.settings.before = None;
                return true;
            };
            match self.grant(opt, wish) {
                Ok(Some(said)) => told.push(said),
                Ok(None) => {}
                Err(why) => {
                    self.alert(why);
                    self.settings.before = None;
                    return true;
                }
            }
        }
        if !told.is_empty() {
            self.notice(told.join("   "));
        }
        self.settings.before = None;
        true
    }

    /// The option an argument of `:set` is about, and what is wished of
    /// it.
    fn wish(argument: &str) -> Option<(&'static Opt, Wish)> {
        let named = |name: &str| find(name);
        for (mark, wish) in [('?', Wish::Ask), ('!', Wish::Toggle), ('&', Wish::Usual)] {
            if let Some(name) = argument.strip_suffix(mark) {
                return named(name).map(|opt| (opt, wish));
            }
        }
        for (sign, by) in [("+=", 1.0), ("-=", -1.0)] {
            if let Some((name, number)) = argument.split_once(sign) {
                let number = number.trim().parse::<f64>().ok()?;
                return named(name).map(|opt| (opt, Wish::Add(by * number)));
            }
        }
        if let Some((name, value)) = argument.split_once(['=', ':']) {
            return named(name).map(|opt| (opt, Wish::To(value.to_owned())));
        }
        if let Some(opt) = named(argument) {
            return Some((opt, Wish::Bare));
        }
        if let Some(opt) = argument.strip_prefix("no").and_then(named) {
            return Some((opt, Wish::Off));
        }
        argument
            .strip_prefix("inv")
            .and_then(named)
            .map(|opt| (opt, Wish::Toggle))
    }

    /// Does what was wished of `opt`. Returns what is to be said, or why
    /// it cannot be.
    fn grant(&mut self, opt: &'static Opt, wish: Wish) -> Result<Option<String>, String> {
        let now = (opt.get)(self);
        let off = || match opt.kind {
            Kind::Switch => Ok(Value::Switch(false)),
            Kind::Choice(choices) if choices.contains(&"off") => Ok(Value::Text("off".to_owned())),
            _ => Err(format!("{} cannot be off: it is one of its own", opt.name)),
        };
        let on = || match opt.kind {
            Kind::Switch => Some(Value::Switch(true)),
            Kind::Choice(_) | Kind::Maybe if !opt.on.is_empty() => {
                Some(Value::Text(opt.on.to_owned()))
            }
            _ => None,
        };
        let value = match wish {
            Wish::Ask => {
                let usual = if now == opt.usual_for(self.settings.preset) {
                    ", as usual"
                } else {
                    ""
                };
                return Ok(Some(format!("{}={}{usual}", opt.short(), now.shown())));
            }
            Wish::Bare => match on() {
                Some(value) => value,
                None => return Ok(Some(format!("{}={}", opt.short(), now.shown()))),
            },
            Wish::To(typed) if opt.name == "aids.preset" && typed == "next" => {
                Value::Text(self.settings.preset.next().name().to_owned())
            }
            Wish::To(typed) => opt.parse(&typed)?,
            Wish::Off => off()?,
            Wish::Usual => opt.usual_for(self.settings.preset),
            Wish::Add(by) => match opt.kind {
                Kind::Number { least, most, .. } => {
                    Value::Number((now.number() + by).clamp(least, most))
                }
                _ => return Err(format!("{} is no number", opt.name)),
            },
            Wish::Toggle => match (opt.kind, &now) {
                (Kind::Switch, _) => Value::Switch(!now.is_on()),
                (Kind::Choice(_) | Kind::Maybe, Value::Text(text)) => {
                    // Off and on again, where it can be off; else the
                    // next of its choices.
                    let choices = opt.choices();
                    let target = match (choices.contains(&"off"), text.as_str()) {
                        (true, "off") if !opt.on.is_empty() => opt.on,
                        (true, _) => "off",
                        (false, _) => {
                            let at = choices.iter().position(|choice| choice == text);
                            choices[at.map_or(0, |at| (at + 1) % choices.len())]
                        }
                    };
                    Value::Text(target.to_owned())
                }
                _ => {
                    return Err(format!(
                        "{} cannot be switched: :set {}=…",
                        opt.name,
                        opt.short()
                    ));
                }
            },
        };
        self.change(opt, &value);
        let said = self.message.take().map(|message| message.text);
        Ok(said)
    }

    /// The rest of a `:set` being typed, as the faint completion behind
    /// it: of a name, or of a choice behind `=`.
    pub(super) fn set_rest(typed: &str) -> Option<String> {
        let line = typed
            .strip_prefix("set ")
            .or_else(|| typed.strip_prefix("se "))?;
        let word = line.rsplit(' ').next().unwrap_or(line);
        if word.is_empty() {
            return None;
        }
        let mut shared: Option<&str> = None;
        let mut consider = |candidate: &'static str, begun: &str| {
            if let Some(rest) = candidate.strip_prefix(begun) {
                shared = Some(shared.map_or(rest, |shared| common(shared, rest)));
            }
        };
        if let Some((name, begun)) = word.split_once('=') {
            let opt = find(name)?;
            for choice in opt.choices() {
                consider(choice, begun);
            }
        } else {
            let begun = ["no", "inv"]
                .iter()
                .find_map(|prefix| {
                    let rest = word.strip_prefix(prefix)?;
                    let names = table().iter().any(|opt| opt.short().starts_with(rest));
                    (names && !table().iter().any(|opt| opt.short().starts_with(word)))
                        .then_some(rest)
                })
                .unwrap_or(word);
            for opt in table() {
                consider(opt.short(), begun);
                consider(opt.name, begun);
            }
        }
        shared.filter(|rest| !rest.is_empty()).map(str::to_owned)
    }

    /// Has the language checker ask the server the settings name, as far
    /// as they name one, and send it text only as they allow.
    pub(super) fn language_server_from_settings(&mut self) {
        let url = self
            .settings
            .held("language.languagetool")
            .map(|value| value.text().trim().to_owned())
            .unwrap_or_default();
        let remote = self
            .settings
            .held("language.allow_remote")
            .is_some_and(Value::is_on);
        self.language.settings.languagetool =
            (!url.is_empty()).then_some(zetwerk_language::Server { url, remote });
        self.reconfigure_language();
    }

    /// Whether `press` takes the rest of a word in running text, where
    /// one shows: Ctrl-Y always, Tab where the user wants it to. Tab
    /// that does not says once what does.
    pub(super) fn takes_word(&mut self, press: KeyPress) -> bool {
        let tab = press.key == Key::Tab && !press.ctrl && !press.alt;
        let yes = press.key == Key::Char('y') && press.ctrl;
        if tab && !self.settings.tab_words {
            if !self.settings.told_tab {
                self.settings.told_tab = true;
                self.notice("Ctrl-Y takes the faint rest of the word (Tab too: :set tabwords)");
            }
            return false;
        }
        tab || yes
    }

    /// Whether keys are for the panel of settings.
    #[must_use]
    pub fn is_setting(&self) -> bool {
        self.settings.panel.is_some() && self.picker.is_none()
    }

    /// `:set`: the panel of all options, at `name` if one is named.
    fn open_settings(&mut self, name: Option<&str>) {
        let mut panel = self.settings.panel.take().unwrap_or_default();
        panel.capture = None;
        panel.typing = false;
        self.settings.panel = Some(panel);
        if let Some(opt) = name.and_then(find) {
            let rows = self.setting_rows();
            let at = rows
                .iter()
                .position(|row| matches!(row, Row::Opt(of) if of.name == opt.name));
            if let (Some(at), Some(panel)) = (at, &mut self.settings.panel) {
                panel.chosen = at;
            }
        }
        self.fix_chosen(true);
    }

    /// The rows of the panel, as narrowed.
    pub(super) fn setting_rows(&self) -> Vec<Row> {
        let query = self
            .settings
            .panel
            .as_ref()
            .map(|panel| panel.query.to_lowercase())
            .unwrap_or_default();
        let fits = |text: &str| query.is_empty() || text.to_lowercase().contains(&query);
        let mut rows = Vec::new();
        for group in Group::ALL {
            let mut these: Vec<Row> = table()
                .iter()
                .filter(|opt| opt.group == group)
                .filter(|opt| fits(&format!("{} {} {}", opt.name, opt.about, group.title())))
                .map(Row::Opt)
                .collect();
            match group {
                Group::Keys => {
                    for (insert, mappings) in [
                        (false, &self.settings.normal),
                        (true, &self.settings.insert),
                    ] {
                        for (keys, does) in mappings {
                            let row = Row::Mapping {
                                insert,
                                keys: keys.clone(),
                                does: does.clone(),
                            };
                            if fits(&format!("{keys} {does} mapping key yours")) {
                                these.push(row);
                            }
                        }
                    }
                    for insert in [false, true] {
                        if fits("a key of yours mapping add map") {
                            these.push(Row::Add(insert));
                        }
                    }
                }
                Group::Language if fits("the document's own language gd") => {
                    these.push(Row::Document("the document's own language"));
                }
                Group::Window if fits("how the document looks paper font margins gd") => {
                    these.push(Row::Document("how the document looks: paper, font, …"));
                }
                Group::Files => {
                    let words =
                        self.language.settings.words.as_ref().map_or_else(
                            || "nowhere".to_owned(),
                            |path| path.display().to_string(),
                        );
                    let infos = [
                        format!("the # menu: {} of yours ([[insert]])", self.inserts.len()),
                        format!("your words: {words}"),
                    ];
                    for info in infos {
                        if fits(&info) {
                            these.push(Row::Info(info));
                        }
                    }
                }
                _ => {}
            }
            if !these.is_empty() {
                rows.push(Row::Heading(group));
                rows.extend(these);
            }
        }
        rows
    }

    /// Keeps the chosen row one that can be chosen: the next one that
    /// can, down or (not `down`) up.
    fn fix_chosen(&mut self, down: bool) {
        let rows = self.setting_rows();
        let Some(panel) = &mut self.settings.panel else {
            return;
        };
        if rows.is_empty() {
            panel.chosen = 0;
            return;
        }
        let mut at = panel.chosen.min(rows.len() - 1);
        for _ in 0..rows.len() {
            if !matches!(rows[at], Row::Heading(_)) {
                break;
            }
            at = if down {
                (at + 1) % rows.len()
            } else {
                (at + rows.len() - 1) % rows.len()
            };
        }
        panel.chosen = at;
    }

    /// A key for the panel of settings. Returns whether it was.
    pub(super) fn set_key(&mut self, press: KeyPress) -> bool {
        if !self.is_setting() {
            return false;
        }
        if self
            .settings
            .panel
            .as_ref()
            .is_some_and(|panel| panel.capture.is_some())
        {
            self.capture_key(press);
            return true;
        }
        let Some(panel) = &mut self.settings.panel else {
            return false;
        };
        if panel.typing {
            match (press.key, press.ctrl) {
                (Key::Escape, _) => {
                    panel.query.clear();
                    panel.typing = false;
                }
                (Key::Enter, _) => panel.typing = false,
                (Key::Backspace, _) => {
                    if panel.query.pop().is_none() {
                        panel.typing = false;
                    }
                }
                (Key::Char(c), false) => panel.query.push(c),
                _ => {}
            }
            panel.chosen = 0;
            self.fix_chosen(true);
            return true;
        }
        let rows = self.setting_rows();
        let Some(panel) = &mut self.settings.panel else {
            return false;
        };
        let chosen = rows.get(panel.chosen).cloned();
        match (press.key, press.ctrl) {
            (Key::Escape, _) | (Key::Char('c' | '['), true) | (Key::Char('q'), false) => {
                if panel.query.is_empty() {
                    self.settings.panel = None;
                } else {
                    panel.query.clear();
                    panel.chosen = 0;
                    self.fix_chosen(true);
                }
            }
            (Key::Char('j') | Key::Down, false) | (Key::Char('n'), true) => {
                panel.chosen = (panel.chosen + 1) % rows.len().max(1);
                self.fix_chosen(true);
            }
            (Key::Char('k') | Key::Up, false) | (Key::Char('p'), true) => {
                panel.chosen = (panel.chosen + rows.len().max(1) - 1) % rows.len().max(1);
                self.fix_chosen(false);
            }
            (Key::Char('g'), false) => {
                panel.chosen = 0;
                self.fix_chosen(true);
            }
            (Key::Char('G'), false) => {
                panel.chosen = rows.len().saturating_sub(1);
                self.fix_chosen(false);
            }
            (Key::Char('/'), false) => {
                panel.typing = true;
                panel.query.clear();
            }
            (Key::Char('u'), false) => self.undo_setting(),
            (Key::Char('a'), false) => {
                let next = self.settings.preset.next();
                self.change_preset(next);
            }
            (Key::Char('x' | '&'), false) => match chosen {
                Some(Row::Opt(opt)) => {
                    let usual = opt.usual_for(self.settings.preset);
                    self.change(opt, &usual);
                }
                Some(Row::Mapping { insert, keys, .. }) => self.map(insert, &keys, None),
                _ => {}
            },
            (Key::Enter | Key::Char(' '), false) => match chosen {
                Some(Row::Opt(opt)) => self.change_chosen(opt),
                Some(Row::Add(insert)) => {
                    panel.capture = Some(Capture {
                        insert,
                        pressed: Vec::new(),
                        keys: None,
                        literal: false,
                    });
                }
                Some(Row::Mapping { keys, does, .. }) => {
                    self.notice(format!("{keys} stands for {does}: x takes it away"));
                }
                Some(Row::Document(_)) => {
                    self.settings.panel = None;
                    self.execute("doc");
                }
                _ => {}
            },
            _ => {}
        }
        true
    }

    /// Enter on an option: a switch changes at once; what has choices or
    /// is typed is asked for.
    fn change_chosen(&mut self, opt: &'static Opt) {
        let now = (opt.get)(self);
        if opt.kind == Kind::Switch && !opt.asks {
            self.change(opt, &Value::Switch(!now.is_on()));
            return;
        }
        let usual = opt.usual_for(self.settings.preset);
        let mark = |value: &Value| match (*value == now, *value == usual) {
            (true, true) => "now, as usual",
            (true, false) => "now",
            (false, true) => "as usual",
            (false, false) => "",
        };
        let values: Vec<Value> = match opt.kind {
            Kind::Switch => vec![Value::Switch(false), Value::Switch(true)],
            Kind::Choice(_) | Kind::Maybe => opt
                .choices()
                .into_iter()
                .map(|choice| Value::Text(choice.to_owned()))
                .collect(),
            Kind::Number { .. } => {
                let mut values: Vec<Value> = std::iter::once(now.clone())
                    .chain(std::iter::once(usual.clone()))
                    .chain(crate::settings::Preset::ALL.map(|preset| opt.usual_for(preset)))
                    .collect();
                values.sort_by(|a, b| a.number().total_cmp(&b.number()));
                values.dedup();
                values
            }
            Kind::Text | Kind::List => vec![now.clone()],
        };
        let chosen = values.iter().position(|value| *value == now).unwrap_or(0);
        let items = values
            .iter()
            .map(|value| {
                let label = match (opt.name, value) {
                    ("language.allow_remote", Value::Switch(true)) => {
                        "on: the text of documents is sent to that server".to_owned()
                    }
                    ("language.allow_remote", Value::Switch(false)) => {
                        "off: only this machine is sent text".to_owned()
                    }
                    _ => value.shown(),
                };
                let command = format!("set {}={}", opt.name, typed(&value.shown()));
                let command = match value {
                    Value::Text(text) if text.is_empty() => format!("set {}=", opt.name),
                    Value::List(texts) if texts.is_empty() => format!("set {}=", opt.name),
                    Value::Switch(on) => {
                        format!("set {}={}", opt.name, if *on { "on" } else { "off" })
                    }
                    _ => command,
                };
                (
                    format!("{label}\t{}", mark(value)),
                    Target::Command(command),
                )
            })
            .collect();
        let free = matches!(opt.kind, Kind::Number { .. } | Kind::Text | Kind::List)
            .then(|| format!("set {}=", opt.name));
        let back = format!("set panel {}", opt.name);
        self.open_picker_for(opt.short(), items, chosen, back, free);
        if matches!(opt.kind, Kind::Number { .. }) {
            self.picker_accepts(move |typed| opt.parse(typed).is_ok());
        }
    }

    /// A key while a key of the user's is being made.
    fn capture_key(&mut self, press: KeyPress) {
        let Some(capture) = self
            .settings
            .panel
            .as_mut()
            .and_then(|panel| panel.capture.as_mut())
        else {
            return;
        };
        if capture.literal {
            capture.literal = false;
            capture.pressed.push(press);
            return;
        }
        match (press.key, press.ctrl) {
            (Key::Escape, _) => {
                if let Some(panel) = &mut self.settings.panel {
                    panel.capture = None;
                }
                self.notice("no key made");
            }
            (Key::Char('v'), true) => capture.literal = true,
            (Key::Backspace, _) => {
                capture.pressed.pop();
            }
            (Key::Enter, _) if capture.pressed.is_empty() => {}
            (Key::Enter, _) => {
                let pressed = std::mem::take(&mut capture.pressed);
                match capture.keys.take() {
                    None => capture.keys = Some(pressed),
                    Some(keys) => {
                        let insert = capture.insert;
                        if let Some(panel) = &mut self.settings.panel {
                            panel.capture = None;
                        }
                        let (keys, does) = (
                            super::hints::notation(&keys),
                            super::hints::notation(&pressed),
                        );
                        self.map(insert, &keys, Some(&does));
                    }
                }
            }
            _ => capture.pressed.push(press),
        }
    }

    /// Makes `keys` stand for `does` (or for nothing more), in normal mode
    /// or while typing (`insert`): in the viewer and in the settings file.
    fn map(&mut self, insert: bool, keys: &str, does: Option<&str>) {
        let mappings = if insert {
            &mut self.settings.insert
        } else {
            &mut self.settings.normal
        };
        let at = mappings.iter().position(|(typed, _)| typed == keys);
        let before = at.map(|at| mappings.remove(at).1);
        if let Some(does) = does {
            mappings.push((keys.to_owned(), does.to_owned()));
        }
        self.settings.undo.push(Undo::Mapping {
            insert,
            keys: keys.to_owned(),
            before,
        });
        self.remap(insert, keys, does);
        match does {
            Some(does) => {
                let parsed = crate::keys::parse(keys).unwrap_or_default();
                let hidden = hides(insert, &parsed)
                    .map_or_else(String::new, |hidden| format!("; it {hidden}"));
                let mode = if insert {
                    "while typing"
                } else {
                    "in normal mode"
                };
                self.notice(format!("{keys} now does {does} {mode}{hidden}"));
            }
            None => self.notice(format!("{keys} does what it did before (u brings it back)")),
        }
    }

    /// Puts the user's keys as they are now into the viewer, and the one
    /// at `keys` (now `does`, or gone) into the settings file.
    fn remap(&mut self, insert: bool, keys: &str, does: Option<&str>) {
        let pairs = |mappings: &[(String, String)]| -> Vec<(String, String)> { mappings.to_vec() };
        let (normal, typing) = (pairs(&self.settings.normal), pairs(&self.settings.insert));
        let (keymap, complaint) = Keymap::parse(
            normal.iter().map(|(a, b)| (a.as_str(), b.as_str())),
            typing.iter().map(|(a, b)| (a.as_str(), b.as_str())),
        );
        self.set_keymap(keymap.with_timeout(self.settings.timeout));
        let mode = if insert { "insert" } else { "normal" };
        let edit = Edit {
            path: vec!["keys".to_owned(), mode.to_owned(), keys.to_owned()],
            value: does.map(|does| crate::settings::Written::Text(does.to_owned())),
        };
        self.write_settings(&[edit]);
        if let Some(complaint) = complaint {
            self.alert(complaint);
        }
    }

    /// `u` in the panel: takes back the last change.
    fn undo_setting(&mut self) {
        let Some(undo) = self.settings.undo.pop() else {
            self.notice("nothing to take back");
            return;
        };
        match undo {
            Undo::Options(before, preset) => {
                self.settings.preset = preset;
                let mut edits = Vec::new();
                for (name, said, value) in before {
                    let Some(opt) = find(name) else {
                        continue;
                    };
                    let path: Vec<String> = name.split('.').map(str::to_owned).collect();
                    match said {
                        Some(said) => {
                            edits.push(Edit {
                                path,
                                value: opt.written(&said),
                            });
                            self.settings.explicit.insert(opt.name, said);
                        }
                        None => {
                            edits.push(Edit { path, value: None });
                            self.settings.explicit.remove(opt.name);
                        }
                    }
                    if name != "aids.preset" && (opt.get)(self) != value {
                        (opt.set)(self, &value);
                    }
                }
                self.write_settings(&edits);
                self.notice("taken back");
            }
            Undo::Mapping {
                insert,
                keys,
                before,
            } => {
                let mappings = if insert {
                    &mut self.settings.insert
                } else {
                    &mut self.settings.normal
                };
                mappings.retain(|(typed, _)| *typed != keys);
                if let Some(before) = &before {
                    mappings.push((keys.clone(), before.clone()));
                }
                self.remap(insert, &keys, before.as_deref());
                self.notice(format!("{keys}: taken back"));
            }
        }
    }

    /// Draws the panel of settings, while it is up and no list of its
    /// own is over it.
    pub(super) fn settings_panel(&mut self, scene: &mut Scene, has: &dyn Fn(TextureId) -> bool) {
        if !self.is_setting() {
            return;
        }
        let markup = self.settings_markup();
        let (width, _) = (f64::from(self.window.0), f64::from(self.window.1));
        let margin = PANEL_MARGIN * self.scale;
        self.panel(scene, has, &markup, |(panel_width, _)| {
            ((width - panel_width - margin).max(margin), margin)
        });
    }

    /// The markup of the panel of settings.
    pub(super) fn settings_markup(&self) -> String {
        let rows = self.setting_rows();
        let Some(panel) = &self.settings.panel else {
            return String::new();
        };
        let preset = self.settings.preset;
        let mut cells = String::new();
        let first = panel
            .chosen
            .saturating_sub(ROWS / 2)
            .min(rows.len().saturating_sub(ROWS));
        let cell = |text: &str, fill: &str, chosen: bool| {
            let ground = if chosen {
                format!("fill: rgb(\"{CHOSEN}\"), ")
            } else {
                String::new()
            };
            format!(
                "  grid.cell({ground}inset: (x: 3pt, y: 2pt), text(fill: rgb(\"{fill}\"), {})),\n",
                literal(text)
            )
        };
        if first > 0 {
            cells.push_str(&format!(
                "  grid.cell(colspan: 3, text(fill: rgb(\"{DIM}\"), \"…\")),\n"
            ));
        }
        for (at, row) in rows.iter().enumerate().skip(first).take(ROWS) {
            let chosen = at == panel.chosen;
            match row {
                Row::Heading(group) => {
                    let _ = writeln!(
                        cells,
                        "  grid.cell(colspan: 3, inset: (top: 5pt, x: 3pt), text(fill: rgb(\"{ACCENT}\"), size: 8pt, {})),",
                        literal(group.title())
                    );
                }
                Row::Opt(opt) => {
                    let value = (opt.get)(self);
                    let own = self.settings.explicit.contains_key(opt.name)
                        && value != opt.usual_for(preset);
                    let mark = if own { "yours" } else { "" };
                    cells.push_str(&cell(opt.short(), TEXT, chosen));
                    cells.push_str(&cell(&value.shown(), if own { OWN } else { TEXT }, chosen));
                    cells.push_str(&cell(mark, OWN, chosen));
                }
                Row::Mapping { insert, keys, does } => {
                    let mode = if *insert { "typing " } else { "" };
                    cells.push_str(&cell(&format!("{mode}{keys}"), TEXT, chosen));
                    cells.push_str(&cell(does, OWN, chosen));
                    cells.push_str(&cell("yours", OWN, chosen));
                }
                Row::Add(insert) => {
                    let what = if *insert {
                        "a key of yours while typing…"
                    } else {
                        "a key of yours…"
                    };
                    cells.push_str(&cell(what, DIM, chosen));
                    cells.push_str(&cell("", DIM, chosen));
                    cells.push_str(&cell("", DIM, chosen));
                }
                Row::Document(what) => {
                    cells.push_str(&cell(what, DIM, chosen));
                    cells.push_str(&cell("gD", DIM, chosen));
                    cells.push_str(&cell("", DIM, chosen));
                }
                Row::Info(what) => {
                    let _ = writeln!(
                        cells,
                        "  grid.cell(colspan: 3, {}inset: (x: 3pt, y: 2pt), text(fill: rgb(\"{DIM}\"), {})),",
                        if chosen {
                            format!("fill: rgb(\"{CHOSEN}\"), ")
                        } else {
                            String::new()
                        },
                        literal(what)
                    );
                }
            }
        }
        if first + ROWS < rows.len() {
            cells.push_str(&format!(
                "  grid.cell(colspan: 3, text(fill: rgb(\"{DIM}\"), \"…\")),\n"
            ));
        }
        let title = match (&panel.query, panel.typing) {
            (query, true) => format!("settings  /{query}\u{2588}"),
            (query, false) if !query.is_empty() => format!("settings  /{query}"),
            _ => format!("settings  ·  aids: {}", preset.name()),
        };
        let (about, help) = self.settings_detail(rows.get(panel.chosen), panel);
        format!(
            "#set page(width: auto, height: auto, margin: 9pt, fill: rgb(\"{GROUND}\"))\n\
             #set text(font: \"DejaVu Sans Mono\", size: 9pt, fill: rgb(\"{TEXT}\"), \
             ligatures: false, hyphenate: false)\n\
             #block(width: {WIDTH}pt)[\n\
             #text(fill: rgb(\"{ACCENT}\"), {title})\n\
             #v(2pt)\n\
             #grid(columns: (1fr, auto, auto), column-gutter: 4pt, row-gutter: 1pt,\n{cells})\n\
             #v(4pt)\n\
             #text(fill: rgb(\"{TEXT}\"), size: 8pt, {about})\n\
             #v(1pt)\n\
             #text(fill: rgb(\"{DIM}\"), size: 7.5pt, {help})\n]",
            title = literal(&title),
            about = literal(&about),
            help = literal(&help),
        )
    }

    /// What is said under the rows of the panel: of the chosen row, and
    /// of the keys.
    fn settings_detail(&self, row: Option<&Row>, panel: &Panel) -> (String, String) {
        if let Some(capture) = &panel.capture {
            let pressed = super::hints::notation(&capture.pressed);
            let mode = if capture.insert {
                "while typing"
            } else {
                "in normal mode"
            };
            let (about, help) = match &capture.keys {
                None => (
                    format!("press the keys to map {mode}: {pressed}\u{2588}"),
                    "Enter: those · Ctrl-V: the next key as it is · Esc: none".to_owned(),
                ),
                Some(keys) => {
                    let keys = super::hints::notation(keys);
                    let hidden = crate::keys::parse(&keys)
                        .ok()
                        .and_then(|parsed| hides(capture.insert, &parsed))
                        .map_or_else(String::new, |hidden| format!(" ({hidden})"));
                    (
                        format!("{keys}{hidden}: now the keys it should do: {pressed}\u{2588}"),
                        "Enter: done · Ctrl-V Enter: an Enter of its own · Esc: none".to_owned(),
                    )
                }
            };
            return (about, help);
        }
        let help = if panel.typing {
            "type to narrow · Enter: done · Esc: all again".to_owned()
        } else {
            "Enter: change · x: as usual · u: take back · a: next aids · /: narrow · Esc".to_owned()
        };
        let about = match row {
            Some(Row::Opt(opt)) => format!("{}. {}", opt.about, self.kept(opt)),
            Some(Row::Mapping { insert, keys, does }) => {
                let parsed = crate::keys::parse(keys).unwrap_or_default();
                let hidden = hides(*insert, &parsed)
                    .map_or_else(String::new, |hidden| format!("; it {hidden}"));
                format!("{keys} stands for {does}{hidden}. x takes it away")
            }
            Some(Row::Add(_)) => "Enter, then the keys, then what they are to do".to_owned(),
            Some(Row::Document(_)) => {
                "kept in the document itself, as its rules: the document manager (gD)".to_owned()
            }
            Some(Row::Info(_)) => "kept in config.toml and beside it".to_owned(),
            Some(Row::Heading(_)) | None => String::new(),
        };
        (about, help)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_apart_by_blanks() {
        assert_eq!(arguments("nonumbers  hls"), ["nonumbers", "hls"]);
        assert_eq!(arguments("save=not\\ said x"), ["save=not said", "x"]);
        assert_eq!(typed("not said"), "not\\ said");
    }

    #[test]
    fn a_mapping_says_what_it_hides() {
        let keys = |notation: &str| crate::keys::parse(notation).unwrap_or_default();
        assert_eq!(
            hides(false, &keys(";")).as_deref(),
            Some("hides ;: the last f or t again")
        );
        assert!(hides(false, &keys("gw")).is_some_and(|hidden| hidden.contains("jump")));
        assert_eq!(hides(false, &keys("gy")), None);
        assert_eq!(hides(false, &keys("<C-s>")), None);
        assert!(hides(false, &keys(",w")).is_some_and(|hidden| hidden.contains("waits")));
        assert!(hides(true, &keys("jk")).is_some_and(|hidden| hidden.contains("waits")));
        assert!(hides(true, &keys("<C-w>")).is_some_and(|hidden| hidden.contains("word")));
    }

    #[test]
    fn set_completes_names_and_choices() {
        assert_eq!(Viewer::set_rest("set numb").as_deref(), Some("ers"));
        assert_eq!(Viewer::set_rest("set nonumb").as_deref(), Some("ers"));
        assert_eq!(Viewer::set_rest("set aids=wr").as_deref(), Some("iting"));
        assert_eq!(
            Viewer::set_rest("set hlsearch nu").as_deref(),
            Some("mbers")
        );
        assert_eq!(Viewer::set_rest("set "), None);
        assert_eq!(Viewer::set_rest("sort"), None);
    }
}
