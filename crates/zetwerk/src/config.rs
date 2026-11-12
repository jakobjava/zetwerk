//! The user's settings: `$XDG_CONFIG_HOME/zetwerk/config.toml`, or
//! `~/.config/zetwerk/config.toml`.
//!
//! ```toml
//! [ai]
//! # "live": agents change the text (each change is one undo step).
//! # "propose": their changes wait for you to accept them.
//! edit_mode = "live"
//! # Whether agents may write the file. Without it, saving is yours.
//! save = false
//! ```

use std::path::PathBuf;

use serde::Deserialize;
use zetwerk_app::viewer::agent::AgentEdits;

#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub ai: Ai,
}

/// What agents may do.
#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ai {
    #[serde(default)]
    pub edit_mode: EditMode,
    #[serde(default)]
    pub save: bool,
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
                save: true,
            },
        };
        assert_eq!(config, Ok(expected));
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
}
