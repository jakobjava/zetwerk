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
//! [view]
//! layout = "split"    # or "source", "page": what the window shows
//! slides = false      # one page at a time, whole
//! ui_scale = 1.0      # how large everything that is not a page is
//!
//! [power]
//! mode = "auto"       # or "saver", "performance"
//!
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
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;
use zetwerk_app::power::PowerMode;
use zetwerk_app::viewer::agent::AgentEdits;
use zetwerk_app::viewer::{Keymap, Panes};

#[derive(Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub ai: Ai,
    #[serde(default)]
    pub view: View,
    #[serde(default)]
    pub power: Power,
    #[serde(default)]
    pub authors: Authors,
    #[serde(default)]
    pub language: Checking,
    #[serde(default)]
    pub keys: Keys,
    /// The user's own things to put in.
    #[serde(default)]
    pub insert: Vec<Insert>,
}

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

/// What the window shows to begin with.
#[derive(Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct View {
    pub layout: Option<Layout>,
    #[serde(default)]
    pub slides: bool,
    /// How many times its usual size the interface is.
    pub ui_scale: Option<f64>,
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

/// Reads the settings. A file that is not there is no settings; one that
/// cannot be read or understood is no settings and a complaint, to be
/// shown to the user: the document should open all the same.
pub fn load() -> (Config, Option<String>) {
    let Some(path) = path() else {
        return (Config::default(), None);
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return (Config::default(), None);
        }
        Err(err) => {
            let complaint = format!("{} was not read: {err}", path.display());
            return (Config::default(), Some(complaint));
        }
    };
    match parse(&text) {
        Ok(config) => (config, None),
        Err(why) => {
            let complaint = format!("{} is ignored: {why}", path.display());
            (Config::default(), Some(complaint))
        }
    }
}

fn parse(text: &str) -> Result<Config, String> {
    // The first line of the message says what is wrong; the rest draws it.
    toml::from_str(text).map_err(|err| {
        let message = err.message().to_owned();
        match err.span() {
            Some(span) => {
                let line = text[..span.start.min(text.len())].matches('\n').count() + 1;
                format!("line {line}: {message}")
            }
            None => message,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(config.keys.keymap().1, None);
        assert!(parse("[keys.normal]\n\"gx\" = 3\n").is_err());
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
        // A slip is said, with its line, and not guessed at.
        let unknown = parse("[ai]\nedit_mode = \"live\"\nsafe = true\n");
        assert!(unknown.is_err_and(|why| why.starts_with("line 3")));
        let wrong = parse("[ai]\nedit_mode = \"yolo\"\n");
        assert!(wrong.is_err_and(|why| why.starts_with("line 2")));
        assert!(parse("[ia]\n").is_err());
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
        assert!(parse("[language]\nspeling = true\n").is_err());
    }
}
