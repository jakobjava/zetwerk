//! Spelling and grammar of the running text of a Typst document.
//!
//! Three things are done here, none of them on the thread that takes
//! the user's keys:
//!
//! - The running text is taken out of the source ([`Prose`]): words and
//!   sentences without markup, equations and code, each with the place
//!   it is written at.
//! - Its words are looked up in a Hunspell dictionary installed on the
//!   machine ([`Speller`], [`find`]) and in the user's own list
//!   ([`Personal`]). English (British first), German and Dutch are the
//!   languages this is made and tested for; any language with a
//!   dictionary in UTF-8 or Latin-1 works.
//! - Its sentences are checked for grammar: by a LanguageTool server
//!   the user runs on this machine, for any language that knows; or,
//!   without one, English by Harper (the `harper` feature).
//!
//! A [`Checker`] does this a paragraph at a time and remembers what it
//! found by the paragraph's text; a [`Service`] runs one on a thread of
//! its own. No text is sent anywhere but to a server the settings name,
//! which has to be this machine unless they say otherwise.

mod checker;
mod dictionary;
#[cfg(feature = "harper")]
mod harper;
mod languagetool;
mod personal;
mod prose;
mod service;
mod words;

use std::ops::Range;
use std::path::PathBuf;

pub use checker::{Checked, Checker, Job, Scope};
pub use dictionary::{Found, Speller, directories, find, installed, package, wanted};
pub use languagetool::code as languagetool_code;
pub use personal::Personal;
pub use prose::{Prose, Unit, units};
pub use service::{Handle, Service};
pub use words::{Lexicon, words};

/// What kind of thing was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A word no dictionary knows.
    Spelling,
    /// Words that do not go together as written.
    Grammar,
}

/// Something wrong with the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The bytes of the source it is about.
    pub range: Range<usize>,
    pub kind: Kind,
    /// What is wrong. Empty for a misspelt word: that it is one says
    /// it all.
    pub message: String,
    /// What could be written instead, the likeliest first. An empty
    /// text means the words are to go. For a misspelt word they are
    /// worked out only where a [`Job`] asks for them.
    pub replacements: Vec<String>,
}

/// A LanguageTool server to check grammar with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Server {
    /// Its address: `http://localhost:8081`.
    pub url: String,
    /// Whether it may be another machine. Text is sent to this machine
    /// only, unless this says otherwise.
    pub remote: bool,
}

/// What the user has to say about checking. By default nothing is
/// checked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// Whether spelling is checked.
    pub spelling: bool,
    /// Whether grammar is.
    pub grammar: bool,
    /// The language to check in, whatever the document says its
    /// language is.
    pub language: Option<String>,
    /// Where dictionaries are looked for, the first first (see
    /// [`directories`]).
    pub directories: Vec<PathBuf>,
    /// The file the user's own words are kept in.
    pub words: Option<PathBuf>,
    /// The server that checks grammar, if there is one.
    pub languagetool: Option<Server>,
}
