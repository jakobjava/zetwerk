//! The user's settings: `$XDG_CONFIG_HOME/zetwerk/config.toml`, or
//! `~/.config/zetwerk/config.toml`.
//!
//! ```toml
//! [ai]
//! # "live": agents change the text (each change is one undo step).
//! # "propose": their changes wait for you to accept them.
//! edit_mode = "live"
//! # Whether agents may write the file. Without it, saving is yours in a
//! # window; with no window (`zetwerk mcp <file>`), the agent's.
//! save = false
//!
//! [review]
//! # How suggestions of agents are marked on the page: "all", only the
//! # one under the cursor ("current"), or "none" (]p still finds them).
//! marks = "all"
//! # Whom an instruction made with g! is for: "document" or "everywhere".
//! instructions = "document"
//!
//! [view]
//! layout = "split"    # or "source", "page": what the window shows
//! slides = false      # one page at a time, whole
//! ui_scale = 1.0      # how large everything that is not a page is
//! restore = true      # open a document where it was left
//! colours = "day"     # or "night", "sepia", "auto": the pages on screen
//! typewriter = false  # keep the line typed in the middle of the window
//! focus = "off"       # or "paragraph", "sentence": the rest faint
//!
//! [changes]
//! bars = true        # a bar beside every line that changed
//! since = "saved"     # what with: "saved", "opened", or a commit ("HEAD")
//! git = true          # ask git for the branch and older versions
//!
//! [power]
//! mode = "auto"       # or "saver", "performance"
//!
//! [present]
//! clock = false       # the time shows in a corner of the slides from the start (t)
//! duration = "20min"  # how long a talk is to take, unless :present says otherwise
//! [authors]
//! view = "off"        # or "origin", "age", "unread": how pages are tinted
//! hide_unknown = false # leave text of unknown origin untinted
//!
//! [language]
//! spelling = true     # words no dictionary knows are marked on the page
//! grammar = true      # and what a grammar checker finds
//! language = "nl"     # check as Dutch, whatever the document says it is
//! dictionaries = ["~/dictionaries"]  # more places to look for Hunspell's
//! # A LanguageTool server of your own checks the grammar of German,
//! # Dutch and English. Only this machine is asked, unless you say so.
//! languagetool = "http://localhost:8081"
//! allow_remote = false
//!
//! [packages]
//! # Whether Typst packages a document imports and this machine lacks are
//! # fetched from packages.typst.org: "ask" (g in :why, or :package get,
//! # each time), "always" (as soon as a document lacks one; not those an
//! # agent imports) or "never". What is sent is a request for the package
//! # by name and version, nothing of the document.
//! download = "ask"
//!
//! # Your own keys, as in Vim's nnoremap and inoremap.
//! [[insert]]            # your own things for the `#` menu
//! name = "diagram"
//! body = "#diagram(|)" # `|` is where the cursor goes
//! block = true         # it stands by itself, like a figure
//! [keys.normal]
//! ";" = ":"
//! "<C-s>" = ":w<CR>"
//! [keys.insert]
//! "<C-s>" = "<Esc>:w<CR>"
//! "jk" = "<Esc>"
//! [keys]
//! timeoutlen = 1000  # how long the first keys of a mapping wait, in ms
//!
//! [report]
//! keys = 500         # keys kept for a report (Ctrl-G, :report); 0 keeps none
//! [aids]
//! preset = "writing"  # or "quiet" (the usual), "everything"
//! numbers = "always"  # and every other option of `:set`
//! [typing]
//! tab_words = true    # Tab takes the faint rest of a word
//! ```
//!
//! Every option is in one table (`zetwerk_app::settings`), and `:set` in
//! the window changes them: what is changed there is written here (see
//! `config_write`). A key this does not know is said and left out; the
//! rest of the file holds all the same.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;
use zetwerk_app::power::PowerMode;
use zetwerk_app::settings::{Stored, Value, Written, table};
use zetwerk_app::viewer::agent::AgentEdits;
use zetwerk_app::viewer::{Keymap, Panes, comfort};

#[derive(Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub ai: Ai,
    #[serde(default)]
    pub view: View,
    #[serde(default)]
    pub review: Review,
    #[serde(default)]
    pub changes: Changes,
    #[serde(default)]
    pub power: Power,
    #[serde(default)]
    pub present: Present,
    #[serde(default)]
    pub authors: Authors,
    #[serde(default)]
    pub language: Checking,
    #[serde(default)]
    pub keys: Keys,
    /// The user's own things to put in.
    #[serde(default)]
    pub insert: Vec<Insert>,
    #[serde(default)]
    pub report: Report,
    /// The sections that only options of the table of settings are in:
    /// read by it (see [`explicit`]), whatever they say.
    #[serde(default)]
    pub aids: toml::Table,
    #[serde(default)]
    pub typing: toml::Table,
    /// Whether packages are fetched (`packages.download`).
    #[serde(default)]
    pub packages: toml::Table,
    /// What was left out, and why, each with its line.
    #[serde(skip)]
    pub ignored: Vec<String>,
}

/// What is kept for a report of what happened (Ctrl-G, `:report`).
#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    /// How many keys are kept; 0 keeps none. 500 unless this says.
    pub keys: Option<usize>,
}

/// The sections only options of the table are in (see [`Config::aids`]).
const OWN_SECTIONS: &[&str] = &["aids", "typing", "packages"];

/// What the settings file says that is no option of the table: the
/// user's keys, and things for the `#` menu.
#[cfg(test)]
const NO_OPTIONS: &[&str] = &["keys.normal", "keys.insert", "insert"];

/// Something the user wants in the menu of things to put into running
/// text (`#` while typing): for a package's diagrams, a letterhead, a
/// theorem.
#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Insert {
    /// What it is listed as and found by.
    pub name: String,
    /// What is put into the text; `|` is where the cursor goes.
    pub body: String,
    /// Whether it stands by itself, as a paragraph does.
    #[serde(default)]
    pub block: bool,
}

impl Config {
    /// The user's own things to put in, as the menu wants them. They
    /// are kept for as long as the program runs.
    #[must_use]
    pub fn inserts(&self) -> Vec<zetwerk_app::typst_text::Insert> {
        self.insert
            .iter()
            .map(|insert| zetwerk_app::typst_text::Insert {
                name: Box::leak(insert.name.to_lowercase().into_boxed_str()),
                short: "",
                body: Box::leak(insert.body.clone().into_boxed_str()),
                block: insert.block,
            })
            .collect()
    }
}

/// How reviewing with agents looks and acts.
#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Review {
    #[serde(default)]
    pub marks: ReviewMarks,
    #[serde(default)]
    pub instructions: Instructions,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewMarks {
    #[default]
    All,
    Current,
    None,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Instructions {
    #[default]
    Document,
    Everywhere,
}

impl Review {
    /// What the window is told.
    pub fn settings(&self) -> zetwerk_app::viewer::review::Settings {
        use zetwerk_app::viewer::review::Marks;
        zetwerk_app::viewer::review::Settings {
            marks: match self.marks {
                ReviewMarks::All => Marks::All,
                ReviewMarks::Current => Marks::Current,
                ReviewMarks::None => Marks::None,
            },
            everywhere: self.instructions == Instructions::Everywhere,
        }
    }
}

/// What the window shows to begin with.
#[derive(Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct View {
    pub layout: Option<Layout>,
    #[serde(default)]
    pub slides: bool,
    /// How many times its usual size the interface is.
    pub ui_scale: Option<f64>,
    /// Whether a document is opened where it was left (default: yes).
    pub restore: Option<bool>,
    /// How the pages are coloured on screen.
    pub colours: Option<Colours>,
    #[serde(default)]
    pub typewriter: bool,
    pub focus: Option<Focus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Colours {
    Day,
    Night,
    Sepia,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Focus {
    Off,
    Paragraph,
    Sentence,
}

impl View {
    /// What the viewer takes of typewriter, focus, colours and finding
    /// the place left; `fresh` where the command line says not to find
    /// it this time.
    #[must_use]
    pub fn comfort(&self, fresh: bool) -> comfort::Settings {
        comfort::Settings {
            restore: self.restore.unwrap_or(true),
            fresh,
            colours: match self.colours {
                None | Some(Colours::Day) => comfort::Colours::Day,
                Some(Colours::Night) => comfort::Colours::Night,
                Some(Colours::Sepia) => comfort::Colours::Sepia,
                Some(Colours::Auto) => comfort::Colours::Auto,
            },
            typewriter: self.typewriter,
            focus: match self.focus {
                None | Some(Focus::Off) => None,
                Some(Focus::Paragraph) => Some(comfort::FocusBy::Paragraph),
                Some(Focus::Sentence) => Some(comfort::FocusBy::Sentence),
            },
        }
    }
}

/// What is shown of what changed since a version.
#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Changes {
    /// Whether lines that changed have a bar beside them.
    pub bars: Option<bool>,
    /// What the text is compared with: `saved`, `opened`, or a revision
    /// of git.
    pub since: Option<String>,
    /// Whether git is asked at all.
    pub git: Option<bool>,
}

impl Changes {
    /// The settings as the viewer takes them.
    #[must_use]
    pub fn options(&self) -> zetwerk_app::viewer::ChangeOptions {
        let usual = zetwerk_app::viewer::ChangeOptions::default();
        zetwerk_app::viewer::ChangeOptions {
            bars: self.bars.unwrap_or(usual.bars),
            since: self.since.clone().unwrap_or(usual.since),
            git: self.git.unwrap_or(usual.git),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    Split,
    Source,
    Page,
}

impl From<Layout> for Panes {
    fn from(layout: Layout) -> Self {
        match layout {
            Layout::Split => Self::Split,
            Layout::Source => Self::Source,
            Layout::Page => Self::Page,
        }
    }
}

/// What talks given from the editor are like (`:present`).
#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Present {
    /// Whether the time shows in the corner of the slides from the start.
    #[serde(default)]
    pub clock: bool,
    /// How long a talk is to take: `20min`, `1h30`, `45`.
    pub duration: Option<String>,
}

impl Present {
    /// The settings as the viewer takes them. A duration that cannot be
    /// read is no duration.
    #[must_use]
    pub fn talk(&self) -> zetwerk_app::viewer::Talk {
        zetwerk_app::viewer::Talk {
            clock: self.clock,
            duration: self
                .duration
                .as_deref()
                .and_then(zetwerk_app::viewer::present_duration),
        }
    }
}

/// How who wrote what is shown (see `zetwerk_app::viewer::authors`).
#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authors {
    /// How the pages are tinted when the window opens.
    pub view: Option<AuthorsView>,
    /// Whether text of unknown origin is left untinted.
    #[serde(default)]
    pub hide_unknown: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthorsView {
    Off,
    Origin,
    Age,
    Unread,
}

impl Authors {
    #[must_use]
    pub fn settings(&self) -> zetwerk_app::viewer::authors::Settings {
        use zetwerk_app::viewer::authors::Tint;
        zetwerk_app::viewer::authors::Settings {
            view: match self.view {
                None | Some(AuthorsView::Off) => None,
                Some(AuthorsView::Origin) => Some(Tint::Origin),
                Some(AuthorsView::Age) => Some(Tint::Age),
                Some(AuthorsView::Unread) => Some(Tint::Unread),
            },
            hide_unknown: self.hide_unknown,
        }
    }
}

/// How much of the machine zetwerk may use.
#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Power {
    pub mode: Option<Mode>,
    /// Whether what changes is typeset by itself at once (`:parts`); read
    /// by the table of options.
    pub parts: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Auto,
    Saver,
    Performance,
}

impl From<Mode> for PowerMode {
    fn from(mode: Mode) -> Self {
        match mode {
            Mode::Auto => Self::Auto,
            Mode::Saver => Self::Saver,
            Mode::Performance => Self::Performance,
        }
    }
}

/// What is checked of the language of a document, and what with.
///
/// Spelling is checked with the Hunspell dictionaries installed on the
/// machine, in the language the document says it is written in
/// (`#set text(lang: "de")`; English where it says nothing, British
/// before American). Grammar is checked by the LanguageTool server
/// named here, or, without one, English alone by rules built in.
#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checking {
    /// Whether spelling is checked; it is unless this says no.
    pub spelling: Option<bool>,
    /// Whether grammar is; it is unless this says no.
    pub grammar: Option<bool>,
    /// The language to check in, whatever the document says: `de`,
    /// `nl`, `en-GB`.
    pub language: Option<String>,
    /// Directories to look for dictionaries in before the usual ones.
    #[serde(default)]
    pub dictionaries: Vec<PathBuf>,
    /// The address of a LanguageTool server: `http://localhost:8081`.
    pub languagetool: Option<String>,
    /// Whether that server may be another machine than this one: then
    /// the text of documents is sent there.
    #[serde(default)]
    pub allow_remote: bool,
}

impl Checking {
    /// The settings as the checker takes them: with the places
    /// dictionaries are looked for in, and the file the user's own
    /// words are kept in, `words.txt` beside the settings.
    #[must_use]
    pub fn settings(&self) -> zetwerk_language::Settings {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        // `~` is the shell's to resolve, and no shell reads this file.
        let resolved = |path: &PathBuf| match (path.strip_prefix("~"), &home) {
            (Ok(rest), Some(home)) => home.join(rest),
            _ => path.clone(),
        };
        let own: Vec<PathBuf> = self.dictionaries.iter().map(resolved).collect();
        zetwerk_language::Settings {
            spelling: self.spelling.unwrap_or(true),
            grammar: self.grammar.unwrap_or(true),
            language: self
                .language
                .clone()
                .filter(|language| !language.trim().is_empty()),
            directories: zetwerk_language::directories(&own),
            words: path().map(|settings| settings.with_file_name("words.txt")),
            languagetool: self
                .languagetool
                .as_ref()
                .map(|url| zetwerk_language::Server {
                    url: url.clone(),
                    remote: self.allow_remote,
                }),
        }
    }
}

/// The user's own keys: what is typed, and what it stands for.
#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keys {
    #[serde(default)]
    pub normal: BTreeMap<String, String>,
    #[serde(default)]
    pub insert: BTreeMap<String, String>,
    /// How long, in milliseconds, keys that may begin a mapping wait for
    /// the next: Vim's `timeoutlen`.
    pub timeoutlen: Option<u64>,
}

impl Keys {
    /// The mappings as the viewer takes them, and a complaint about the
    /// first that makes no sense.
    pub fn keymap(&self) -> (Keymap, Option<String>) {
        fn pairs(table: &BTreeMap<String, String>) -> Vec<(&str, &str)> {
            table
                .iter()
                .map(|(typed, stands_for)| (typed.as_str(), stands_for.as_str()))
                .collect()
        }
        let (keymap, complaint) = Keymap::parse(pairs(&self.normal), pairs(&self.insert));
        match self.timeoutlen {
            Some(millis) => (keymap.with_timeout(millis), complaint),
            None => (keymap, complaint),
        }
    }
}

/// What agents may do.
#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ai {
    #[serde(default)]
    pub edit_mode: EditMode,
    /// Whether agents may write the file; where it is not set, they may
    /// only where no window shows the document.
    #[serde(default)]
    pub save: Option<bool>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EditMode {
    #[default]
    Live,
    Propose,
}

impl From<EditMode> for AgentEdits {
    fn from(mode: EditMode) -> Self {
        match mode {
            EditMode::Live => Self::Live,
            EditMode::Propose => Self::Propose,
        }
    }
}

/// Where the settings are, if the environment says where a home is.
fn path() -> Option<PathBuf> {
    let set = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    let base = match (set("XDG_CONFIG_HOME"), set("HOME")) {
        (Some(config), _) => PathBuf::from(config),
        (None, Some(home)) => PathBuf::from(home).join(".config"),
        (None, None) => return None,
    };
    Some(base.join("zetwerk/config.toml"))
}

/// Where the user's own starters for new documents are: the Typst files
/// in `templates` beside the settings.
pub fn templates() -> Option<PathBuf> {
    path().map(|settings| settings.with_file_name("templates"))
}

/// Reads the settings: as the program takes them, and as the table of
/// options does, with what writes changes back. A file that is not there
/// is no settings; one that cannot be read or understood is no settings
/// and a complaint, to be shown to the user: the document should open
/// all the same. What it says that is not understood is left out, and
/// said.
pub fn load() -> (Config, Stored, Option<String>) {
    let Some(path) = path() else {
        return (Config::default(), Stored::default(), None);
    };
    let mut stored = Stored {
        writer: Some(crate::config_write::writer(&path)),
        file: Some(path.clone()),
        ..Stored::default()
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return (Config::default(), stored, None);
        }
        Err(err) => {
            let complaint = format!("{} was not read: {err}", path.display());
            stored.writer = None;
            return (Config::default(), stored, Some(complaint));
        }
    };
    stored.existed = true;
    match parse(&text) {
        Ok(config) => {
            let (explicit, mut ignored) = explicit(&text);
            ignored.splice(0..0, config.ignored.iter().cloned());
            stored.explicit = explicit;
            stored.normal = pairs(&config.keys.normal);
            stored.insert = pairs(&config.keys.insert);
            let complaint = (!ignored.is_empty())
                .then(|| format!("{}: {}", path.display(), ignored.join("; ")));
            (config, stored, complaint)
        }
        Err(why) => {
            // Not to be written into: it would be the program's then.
            stored.writer = None;
            let complaint = format!("{} is ignored: {why}", path.display());
            (Config::default(), stored, Some(complaint))
        }
    }
}

/// The settings file as it is, if there is one: what a report says was
/// in force.
pub fn text() -> Option<String> {
    std::fs::read_to_string(path()?).ok()
}

/// What settings `text` (read as `config`) say, kept nowhere: for a
/// report played again, whose changes must not reach the user's file.
pub fn stored_of(text: &str, config: &Config) -> Stored {
    Stored {
        explicit: explicit(text).0,
        normal: pairs(&config.keys.normal),
        insert: pairs(&config.keys.insert),
        window: true,
        ..Stored::default()
    }
}

/// The user's keys as written.
fn pairs(table: &BTreeMap<String, String>) -> Vec<(String, String)> {
    table
        .iter()
        .map(|(typed, does)| (typed.clone(), does.clone()))
        .collect()
}

/// The line byte `at` of `text` is on, counted from 1.
fn line_of(text: &str, at: usize) -> usize {
    text[..at.min(text.len())].matches('\n').count() + 1
}

/// How many slips of a file are left out at most, one after the other.
const SLIPS: usize = 64;

/// Reads the settings in `text`. What it does not understand (a key it
/// does not know, a value a key cannot have) is left out and said in
/// [`Config::ignored`]; only a file that is no TOML at all is refused.
pub fn parse(text: &str) -> Result<Config, String> {
    let original = toml_edit::Document::parse(text.to_owned()).ok();
    let mut current = text.to_owned();
    let mut ignored = Vec::new();
    for _ in 0..SLIPS {
        let err = match toml::from_str::<Config>(&current) {
            Ok(mut config) => {
                config.ignored = ignored;
                return Ok(config);
            }
            Err(err) => err,
        };
        // The first line of the message says what is wrong; the rest draws it.
        let message = err.message().to_owned();
        let Some(span) = err.span() else {
            return Err(message);
        };
        let path = toml_edit::Document::parse(current.clone())
            .ok()
            .and_then(|document| path_at(document.as_table(), span.start));
        let mut document: toml_edit::DocumentMut = match (&path, current.parse()) {
            (Some(_), Ok(document)) => document,
            _ => return Err(format!("line {}: {message}", line_of(&current, span.start))),
        };
        let path = path.unwrap_or_default();
        // Where it is in the file as written, not as left.
        let at = original
            .as_ref()
            .and_then(|original| span_of(original.as_table(), &path))
            .map_or_else(|| line_of(&current, span.start), |at| line_of(text, at));
        remove(document.as_table_mut(), &path);
        current = document.to_string();
        ignored.push(format!("line {at}: {message}, left out"));
    }
    Err("too much in it is not understood".to_owned())
}

/// A step of the way to a value in a TOML document.
#[derive(Debug, Clone, PartialEq)]
enum Step {
    Key(String),
    Index(usize),
}

/// The way to the innermost key or value of `table` that byte `at` of its
/// text is in.
fn path_at(table: &dyn toml_edit::TableLike, at: usize) -> Option<Vec<Step>> {
    let inside = |span: Option<std::ops::Range<usize>>| span.is_some_and(|span| span.contains(&at));
    for (name, item) in table.iter() {
        let key = table.key(name).and_then(toml_edit::Key::span);
        let deeper = match item {
            toml_edit::Item::Table(inner) => path_at(inner, at),
            toml_edit::Item::Value(toml_edit::Value::InlineTable(inner)) => path_at(inner, at),
            toml_edit::Item::ArrayOfTables(array) => {
                array.iter().enumerate().find_map(|(index, inner)| {
                    let mut path = vec![Step::Index(index)];
                    path.extend(
                        path_at(inner, at).or_else(|| inside(inner.span()).then(Vec::new))?,
                    );
                    Some(path)
                })
            }
            _ => None,
        };
        if let Some(deeper) = deeper {
            let mut path = vec![Step::Key(name.to_owned())];
            path.extend(deeper);
            return Some(path);
        }
        if inside(key) || inside(item.span()) {
            return Some(vec![Step::Key(name.to_owned())]);
        }
    }
    None
}

/// Where in its text the key at `path` of `table` begins.
fn span_of(table: &dyn toml_edit::TableLike, path: &[Step]) -> Option<usize> {
    match path {
        [Step::Key(name)] => table
            .key(name)
            .and_then(toml_edit::Key::span)
            .or_else(|| table.get(name)?.span())
            .map(|span| span.start),
        [Step::Key(name), Step::Index(index), rest @ ..] => {
            let inner = table.get(name)?.as_array_of_tables()?.get(*index)?;
            if rest.is_empty() {
                return inner.span().map(|span| span.start);
            }
            span_of(inner, rest)
        }
        [Step::Key(name), rest @ ..] => span_of(table.get(name)?.as_table_like()?, rest),
        _ => None,
    }
}

/// Takes out what is at `path` of `table`.
fn remove(table: &mut dyn toml_edit::TableLike, path: &[Step]) {
    match path {
        [Step::Key(name)] => {
            table.remove(name);
        }
        [Step::Key(name), Step::Index(index), rest @ ..] => {
            let Some(array) = table
                .get_mut(name)
                .and_then(toml_edit::Item::as_array_of_tables_mut)
            else {
                return;
            };
            if rest.is_empty() {
                array.remove(*index);
            } else if let Some(inner) = array.get_mut(*index) {
                remove(inner, rest);
            }
        }
        [Step::Key(name), rest @ ..] => {
            if let Some(inner) = table
                .get_mut(name)
                .and_then(toml_edit::Item::as_table_like_mut)
            {
                remove(inner, rest);
            }
        }
        _ => {}
    }
}

/// What `text` says of the options of the table of settings, each as
/// it reads; and what of the sections only they are in it does not
/// understand. (What it does not understand elsewhere [`parse`] says.)
pub fn explicit(text: &str) -> (Vec<(&'static str, Value)>, Vec<String>) {
    let mut found = Vec::new();
    let mut ignored = Vec::new();
    let Ok(document) = toml_edit::Document::parse(text.to_owned()) else {
        return (found, ignored);
    };
    let root = document.as_table();
    let line = |item: &toml_edit::Item| item.span().map_or(0, |span| line_of(text, span.start));
    for opt in table() {
        let (section, key) = opt.place();
        let Some(item) = root
            .get(section)
            .and_then(toml_edit::Item::as_table_like)
            .and_then(|section| section.get(key))
        else {
            continue;
        };
        let read = written(item)
            .ok_or_else(|| format!("{} cannot be read", opt.name))
            .and_then(|written| opt.read(&written));
        match read {
            Ok(value) => found.push((opt.name, value)),
            Err(why) if OWN_SECTIONS.contains(&section) => {
                ignored.push(format!("line {}: {why}, left out", line(item)));
            }
            // Said by `parse` already.
            Err(_) => {}
        }
    }
    for section in OWN_SECTIONS {
        let Some(keys) = root.get(section).and_then(toml_edit::Item::as_table_like) else {
            continue;
        };
        for (key, item) in keys.iter() {
            let name = format!("{section}.{key}");
            if !table().iter().any(|opt| opt.name == name) {
                ignored.push(format!(
                    "line {}: no option {name} (:set lists them), left out",
                    line(item)
                ));
            }
        }
    }
    (found, ignored)
}

/// A value of a TOML document as the table of settings takes it.
fn written(item: &toml_edit::Item) -> Option<Written> {
    let value = item.as_value()?;
    Some(match value {
        toml_edit::Value::Boolean(on) => Written::Bool(*on.value()),
        toml_edit::Value::Integer(number) => Written::Integer(*number.value()),
        toml_edit::Value::Float(number) => Written::Float(*number.value()),
        toml_edit::Value::String(text) => Written::Text(text.value().clone()),
        toml_edit::Value::Array(array) => Written::List(
            array
                .iter()
                .map(|value| value.as_str().map(str::to_owned))
                .collect::<Option<_>>()?,
        ),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_changed_is_shown_unless_the_settings_say_otherwise() {
        let usual = parse("").unwrap_or_default().changes.options();
        assert_eq!(usual, zetwerk_app::viewer::ChangeOptions::default());
        assert!(usual.bars && usual.git && usual.since == "saved");
        let own = parse(
            "[changes]
bars = false
since = \"HEAD\"
git = false
",
        )
        .unwrap_or_default()
        .changes
        .options();
        assert!(!own.bars && !own.git && own.since == "HEAD");
        // A key it does not know is left out, and said.
        assert!(
            parse(
                "[changes]
colour = 1
"
            )
            .is_ok_and(|config| config.ignored.len() == 1)
        );
    }

    #[test]
    fn settings_are_optional_and_checked() {
        assert_eq!(parse(""), Ok(Config::default()));
        let config = parse("[ai]\nedit_mode = \"propose\"\nsave = true\n");
        let expected = Config {
            ai: Ai {
                edit_mode: EditMode::Propose,
                save: Some(true),
            },
            ..Config::default()
        };
        assert_eq!(config, Ok(expected));
        let config = parse(
            "[view]\nlayout = \"page\"\nslides = true\nui_scale = 1.5\n[power]\nmode = \"saver\"\n\
             [keys.normal]\n\";\" = \":\"\n[keys.insert]\n\"<C-s>\" = \"<Esc>:w<CR>\"\n",
        )
        .unwrap_or_default();
        assert_eq!(
            (config.view.layout, config.view.slides),
            (Some(Layout::Page), true)
        );
        assert!(
            config
                .view
                .ui_scale
                .is_some_and(|ui| (ui - 1.5).abs() < 1e-9)
        );
        assert_eq!(config.power.mode, Some(Mode::Saver));
        let view = parse("[view]\nrestore = false\ncolours = \"sepia\"\nfocus = \"sentence\"\n")
            .unwrap_or_default()
            .view;
        let settings = view.comfort(false);
        assert!(!settings.restore && !settings.typewriter);
        assert_eq!(settings.colours, comfort::Colours::Sepia);
        assert_eq!(settings.focus, Some(comfort::FocusBy::Sentence));
        assert!(Config::default().view.comfort(true).restore);
        assert!(
            parse("[view]\ncolours = \"purple\"\n").is_ok_and(|config| config.ignored.len() == 1)
        );
        assert_eq!(config.keys.keymap().1, None);
        let slip = |text: &str| parse(text).map(|config| config.ignored).unwrap_or_default();
        assert_eq!(slip("[keys.normal]\n\"gx\" = 3\n").len(), 1);
        // Several keys in insert mode too, as Vim's `inoremap jk <Esc>`;
        // and how long they wait for each other.
        let jk = parse("[keys]\ntimeoutlen = 300\n[keys.insert]\n\"jk\" = \"<Esc>\"\n");
        let jk = jk.unwrap_or_default();
        assert_eq!(jk.keys.keymap().1, None);
        assert_eq!(jk.keys.timeoutlen, Some(300));
        let odd = parse("[keys.insert]\n\"\" = \"<Esc>\"\n").unwrap_or_default();
        assert!(odd.keys.keymap().1.is_some());
        assert_eq!(
            parse("[ai]\nsave = true\n").map(|config| config.ai.edit_mode),
            Ok(EditMode::Live)
        );
        // A slip is said, with its line, and not guessed at; the rest
        // holds.
        let unknown = parse("[ai]\nedit_mode = \"propose\"\nsafe = true\n").unwrap_or_default();
        assert_eq!(unknown.ai.edit_mode, EditMode::Propose);
        assert!(
            unknown.ignored.len() == 1 && unknown.ignored[0].starts_with("line 3"),
            "{:?}",
            unknown.ignored
        );
        let wrong = slip("# a comment\n[ai]\nedit_mode = \"yolo\"\n");
        assert!(
            wrong.len() == 1 && wrong[0].starts_with("line 3"),
            "{wrong:?}"
        );
        let section = slip("[view]\nui_scale = 1.5\n[ia]\nx = 1\n[power]\nmode = \"saver\"\n");
        assert!(
            section.len() == 1 && section[0].starts_with("line 3"),
            "{section:?}"
        );
        let both = parse("[view]\nui_scale = 1.5\n[ia]\nx = 1\n[power]\nmode = \"saver\"\n");
        assert_eq!(both.map(|config| config.power.mode), Ok(Some(Mode::Saver)));
        let insert = slip("[[insert]]\nname = \"a\"\nbody = \"b\"\nblok = true\n");
        assert!(
            insert.len() == 1 && insert[0].starts_with("line 4"),
            "{insert:?}"
        );
        // What is no TOML at all is refused, with its line.
        assert!(parse("[ai]\nedit_mode = \n").is_err_and(|why| why.starts_with("line 2")));
    }

    fn slip_of(text: &str) -> usize {
        parse(text).map_or(0, |config| config.ignored.len())
    }

    #[test]
    fn packages_are_fetched_when_asked_unless_the_settings_say_otherwise() {
        for said in ["ask", "always", "never"] {
            let (found, ignored) = explicit(&format!("[packages]\ndownload = \"{said}\"\n"));
            assert_eq!(found, [("packages.download", Value::Text(said.into()))]);
            assert!(ignored.is_empty(), "{ignored:?}");
        }
        let (found, ignored) = explicit("[packages]\ndownload = \"sometimes\"\nfetch = 1\n");
        assert_eq!(found, Vec::new());
        assert_eq!(ignored.len(), 2, "{ignored:?}");
    }

    #[test]
    fn language_is_checked_unless_the_settings_say_no() {
        let settings = Config::default().language.settings();
        assert!(settings.spelling && settings.grammar);
        assert_eq!((settings.language, settings.languagetool), (None, None));
        // Where packages put dictionaries is always looked in.
        let system = std::path::Path::new("/usr/share/hunspell");
        assert!(
            settings
                .directories
                .iter()
                .any(|directory| directory == system)
        );

        let config = parse(
            "[language]\nspelling = false\ngrammar = false\nlanguage = \"nl\"\n\
             dictionaries = [\"/opt/dictionaries\"]\n\
             languagetool = \"http://localhost:8081\"\n",
        )
        .unwrap_or_default();
        let settings = config.language.settings();
        assert!(!settings.spelling && !settings.grammar);
        assert_eq!(settings.language.as_deref(), Some("nl"));
        // The user's own directories come first.
        let own = std::path::Path::new("/opt/dictionaries");
        assert_eq!(
            settings.directories.first().map(PathBuf::as_path),
            Some(own)
        );
        // A server is this machine's unless the settings say otherwise.
        let server = settings.languagetool.unwrap_or_default();
        assert_eq!(
            (server.url.as_str(), server.remote),
            ("http://localhost:8081", false)
        );
        let remote =
            parse("[language]\nlanguagetool = \"https://lt.example.org\"\nallow_remote = true\n");
        let remote = remote.unwrap_or_default().language.settings().languagetool;
        assert!(remote.is_some_and(|server| server.remote));
        assert_eq!(slip_of("[language]\nspeling = true\n"), 1);
    }

    #[test]
    fn options_are_read_by_the_table_and_slips_said() {
        let text = "[view]\nui_scale = 1.25\nlayout = \"split\"\n[aids]\npreset = \"writing\"\n\
                    numbers = \"always\"\npause = 300\nghosts = 1\nreach = 500\n\
                    [typing]\ntab_words = true\n[language]\ndictionaries = [\"~/d\"]\n";
        let config = parse(text).unwrap_or_default();
        assert_eq!(config.ignored, Vec::<String>::new());
        let (found, ignored) = explicit(text);
        let value = |name: &str| {
            found
                .iter()
                .find(|(of, _)| *of == name)
                .map(|(_, value)| value.clone())
        };
        assert_eq!(value("view.ui_scale"), Some(Value::Number(1.25)));
        assert_eq!(value("view.layout"), Some(Value::Text("split".into())));
        assert_eq!(value("aids.preset"), Some(Value::Text("writing".into())));
        assert_eq!(value("aids.pause"), Some(Value::Number(300.0)));
        assert_eq!(value("typing.tab_words"), Some(Value::Switch(true)));
        assert_eq!(
            value("language.dictionaries"),
            Some(Value::List(vec!["~/d".into()]))
        );
        assert_eq!(value("aids.reach"), None);
        // A key no option is for, a value it cannot have: said, with
        // their lines, and the rest holds.
        assert_eq!(ignored.len(), 2, "{ignored:?}");
        assert!(ignored[0].starts_with("line 9") && ignored[0].contains("aids.reach"));
        assert!(ignored[1].starts_with("line 8") && ignored[1].contains("aids.ghosts"));
        // Files as they were read as before: nothing of the new is said.
        assert_eq!(explicit("").0, Vec::new());
        assert_eq!(
            parse("[view]\nui_scale = 1.5\n").map(|config| config.ignored),
            Ok(Vec::new())
        );
    }

    /// The fields of the settings that a section of them has, as the
    /// reader of the file names them where one is not known.
    fn fields(text: &str) -> Vec<String> {
        let Err(err) = toml::from_str::<Config>(text) else {
            return Vec::new();
        };
        let message = err.message().to_owned();
        let Some((_, expected)) = message.split_once("expected ") else {
            return Vec::new();
        };
        let expected = expected.trim_start_matches("one of ");
        expected
            .split(", ")
            .flat_map(|fields| fields.split(" or "))
            .map(|field| field.trim().trim_matches('`').to_owned())
            .collect()
    }

    /// For whoever merges an option of another branch: every key the
    /// settings file is read for is an option of the table of settings
    /// (`zetwerk_app::viewer::options`), or is said here to be none; and
    /// every option of the table is a key the file may have.
    #[test]
    fn every_key_of_the_settings_file_is_an_option() {
        let sections = fields("zetwerk_no_such_section = 1\n");
        assert!(sections.contains(&"view".to_owned()), "{sections:?}");
        for section in &sections {
            if OWN_SECTIONS.contains(&section.as_str()) || NO_OPTIONS.contains(&section.as_str()) {
                continue;
            }
            let keys = fields(&format!("[{section}]\nzetwerk_no_such_key = 1\n"));
            assert!(!keys.is_empty(), "the keys of [{section}] are not said");
            for key in keys {
                let name = format!("{section}.{key}");
                assert!(
                    table().iter().any(|opt| opt.name == name)
                        || NO_OPTIONS.contains(&name.as_str()),
                    "{name} is read from the settings file, but no option of the table is for it: \
                     add it to TABLE in viewer/options.rs"
                );
            }
        }
        for opt in table() {
            let (section, key) = opt.place();
            assert!(
                sections.iter().any(|known| known == section),
                "no section [{section}] for {}",
                opt.name
            );
            if !OWN_SECTIONS.contains(&section) {
                let keys = fields(&format!("[{section}]\nzetwerk_no_such_key = 1\n"));
                assert!(
                    keys.iter().any(|known| known == key),
                    "{} is no key of [{section}]: {keys:?}",
                    opt.name
                );
            }
        }
    }
}
