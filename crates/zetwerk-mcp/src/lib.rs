//! MCP server for a running zetwerk window.
//!
//! The window listens on a Unix socket in the user's runtime directory and
//! speaks MCP over it. Agents are configured to start `zetwerk mcp`, a
//! process that only forwards its standard input and output to that socket.
//! There is no network port, so nothing outside the user's session can reach
//! the editor.
//!
//! This crate knows the editor only through the [`Editor`] trait, so it does
//! not depend on windows or GPUs and can serve a headless editor later.

mod client;
mod review;
mod server;
mod socket;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

pub use client::{Output, call};
pub use review::{Caller, NewSuggestion, Revision, Suggest, caller};
pub use server::{Running, Tools, serve, serve_stdio};
pub use socket::{find_socket, find_socket_of, proxy_stdio, socket_of, socket_path};

/// What the server needs from the editor it exposes. Called on worker
/// threads; implementations block until they have the answer.
pub trait Editor: Send + Sync + 'static {
    fn state(&self) -> Result<EditorState, String>;

    /// One page of the compiled document as a PNG, scaled so that its longer
    /// side is at most `max_px` pixels. Pages count from 1.
    fn render_page(&self, page: usize, max_px: u32) -> Result<Vec<u8>, String>;

    /// Presses keys given in Vim notation, as if typed into the window.
    fn send_keys(&self, keys: &str) -> Result<(), String>;

    /// The window's contents as a PNG, once it has finished drawing.
    fn screenshot(&self) -> Result<Vec<u8>, String>;

    /// Lines `from..=to` of the source (counting from 1; all of it by
    /// default), each as its number, a tab and its text, under a line
    /// that says which revision of the text they are.
    fn read_source(&self, from: Option<usize>, to: Option<usize>) -> Result<String, String>;

    /// Makes `change`, or proposes it, and reports what became of it and
    /// what the compiler says to the text afterwards.
    fn change(&self, change: Change) -> Result<serde_json::Value, String>;

    /// The errors of the current revision, once it is compiled.
    fn diagnostics(&self) -> Result<serde_json::Value, String>;

    /// The headings and labels of the document.
    fn outline(&self) -> Result<serde_json::Value, String>;

    /// The places `query` is in the text.
    fn search(
        &self,
        query: &str,
        ignore_case: bool,
        limit: usize,
    ) -> Result<serde_json::Value, String>;

    /// Where a place of the source is on the pages, or the other way
    /// round.
    fn locate(&self, place: Place) -> Result<serde_json::Value, String>;

    /// The changes that wait for the user's word.
    fn proposals(&self) -> Result<serde_json::Value, String>;

    /// Takes back proposal `id`.
    fn withdraw_proposal(&self, id: u64) -> Result<serde_json::Value, String>;

    /// Takes back the proposals `ids`, all of them or, where one is not
    /// there, none.
    fn withdraw_proposals(&self, ids: &[u64]) -> Result<serde_json::Value, String> {
        let mut waiting = serde_json::Value::Null;
        for id in ids {
            waiting = self.withdraw_proposal(*id)?["waiting"].take();
        }
        Ok(serde_json::json!({ "withdrawn": ids, "waiting": waiting }))
    }

    /// Writes the document as a PDF to `path`, or beside its main file:
    /// all of it, or the pages `pages` names (`3-7`, `5`, `1,4-6`).
    fn export_pdf(
        &self,
        path: Option<&str>,
        pages: Option<&str>,
    ) -> Result<serde_json::Value, String>;

    /// Writes pages of the document, or one figure of it, as images.
    fn export_image(&self, image: ImageExport) -> Result<serde_json::Value, String>;

    /// Writes the text to its file.
    fn save(&self) -> Result<serde_json::Value, String>;

    /// The sources the document's bibliography file holds.
    fn sources(&self) -> Result<serde_json::Value, String>;

    /// What the document has that can be referred to: its sections,
    /// figures, tables and equations, and the sources of its
    /// bibliography.
    fn targets(&self) -> Result<serde_json::Value, String>;

    /// Adds the sources written in `bibtex` to that file, and reports
    /// the keys they can be cited by; those that are there already only
    /// if `anyway`, else it reports them.
    fn add_source(&self, bibtex: &str, anyway: bool) -> Result<serde_json::Value, String>;

    /// The figures of the file being edited, and the tables and images
    /// that stand in none.
    fn figures(&self) -> Result<serde_json::Value, String>;

    /// What is wrong with the spelling and the grammar of the text.
    fn check_language(&self) -> Result<serde_json::Value, String>;

    /// The notes on the text, the questions for agents among them: those
    /// not resolved, or all.
    fn notes(&self, include_resolved: bool) -> Result<serde_json::Value, String>;

    /// Leaves an agent's note on the text.
    fn add_note(&self, note: NewNote) -> Result<serde_json::Value, String>;

    /// An agent's answer to a note, with a change proposed as part of it
    /// where one is given.
    fn answer_note(&self, answer: NoteAnswer) -> Result<serde_json::Value, String>;

    /// Marks note `id` resolved.
    fn resolve_note(&self, id: u64) -> Result<serde_json::Value, String>;

    /// Makes the suggestions of `suggest`, as `caller` asks: each is
    /// answered (see `zetwerk_app::viewer::review`).
    fn suggest(&self, caller: &Caller, suggest: Suggest) -> Result<serde_json::Value, String> {
        let _ = (caller, suggest);
        Err(NO_REVIEW.to_owned())
    }

    /// The suggestions in `states` (all where none are named), of the
    /// agent named `by` only where it is given, changed since step
    /// `since`; with the reviews asked for and the brief.
    fn suggestions(
        &self,
        states: &[String],
        by: Option<&str>,
        since: Option<u64>,
    ) -> Result<serde_json::Value, String> {
        let _ = (states, by, since);
        Err(NO_REVIEW.to_owned())
    }

    /// A new version of a suggestion, other wordings, or a line to the
    /// author about it.
    fn revise_suggestion(
        &self,
        caller: &Caller,
        revision: Revision,
    ) -> Result<serde_json::Value, String> {
        let _ = (caller, revision);
        Err(NO_REVIEW.to_owned())
    }

    /// Takes back the suggestions `ids`: all, or none where one cannot be.
    fn withdraw_suggestions(
        &self,
        caller: &Caller,
        ids: &[u64],
    ) -> Result<serde_json::Value, String> {
        let _ = (caller, ids);
        Err(NO_REVIEW.to_owned())
    }

    /// The author's standing instructions to agents.
    fn brief(&self) -> Result<serde_json::Value, String> {
        Err(NO_REVIEW.to_owned())
    }

    /// The brief as lines of words, for the instructions an agent is
    /// given as it connects; `None` where there is none.
    fn brief_text(&self) -> Option<String> {
        None
    }

    /// What `get_state` says of reviewing: how many suggestions wait,
    /// requests, the brief.
    fn review_summary(&self) -> Option<serde_json::Value> {
        None
    }

    /// Waits until the author did something with suggestions, reviews
    /// or the brief since step `since` (by default: since now), at most
    /// `timeout` or until `cancelled`; then what it was.
    fn wait_for_review(
        &self,
        since: Option<u64>,
        timeout: Duration,
        cancelled: &AtomicBool,
    ) -> Result<serde_json::Value, String> {
        let _ = (since, timeout, cancelled);
        Err(NO_REVIEW.to_owned())
    }
}

/// What an agent is told by an editor that keeps no review.
const NO_REVIEW: &str = "this editor keeps no review";

/// A note an agent leaves on the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewNote {
    pub text: String,
    pub anchor: NoteAnchor,
    /// The revision the agent read the text at: the note is refused if
    /// the text has changed since.
    pub expected_revision: Option<u64>,
}

/// What a note is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteAnchor {
    /// This text, which is in the source exactly once.
    Text(String),
    /// The paragraph (or heading, or item) on this line, counting from 1.
    Line(usize),
}

/// An agent's answer to note `id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteAnswer {
    pub id: u64,
    pub text: String,
    /// Text to replace (once) and what to put there: proposed to the user
    /// as part of the answer.
    pub change: Option<(String, String)>,
    /// The revision the agent read the text at: the answer is refused if
    /// the text has changed since.
    pub expected_revision: Option<u64>,
}

/// A change an agent asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub what: ChangeKind,
    /// The revision the agent read; the change is refused if the text is
    /// at another one.
    pub expected_revision: Option<u64>,
    /// Whether to put it before the user instead of making it.
    pub propose: bool,
    /// Why, shown to the user with the proposal.
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeKind {
    /// Put `new` where `old` is; `old` has to be there exactly once, or,
    /// with `all`, at least once.
    Replace { old: String, new: String, all: bool },
    /// Put the text in at the cursor.
    Insert(String),
    /// Put the text where the selection is.
    ReplaceSelection(String),
    /// Take the section whose heading is on `line` (counting from 1)
    /// where `direction` says, with all its subsections.
    MoveSection { line: usize, direction: Direction },
}

/// Where a section is to go: past the section of its level before it
/// or behind it under the same heading, or a level deeper or shallower.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Up,
    Down,
    Deeper,
    Shallower,
}

/// An image, or several, that an agent asks to have written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageExport {
    pub format: ImageFormat,
    pub of: ImageOf,
    /// Pixels to the inch of a PNG, if not the usual.
    pub dpi: Option<u32>,
    /// Where to write, if not beside the main file under its name.
    pub path: Option<String>,
}

/// The kind of image to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    Png,
    Svg,
}

/// What an image is to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageOf {
    /// Pages, as `3-7`, `5` or `1,4-6` name them; all where none are
    /// named. Every page is a file of its own.
    Pages(Option<String>),
    /// The figure that this line of the source, counting from 1, is
    /// part of: that figure alone.
    FigureAtLine(usize),
}

/// A place to look up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Place {
    /// A line and a column of the source, counting from 1.
    Source { line: usize, column: usize },
    /// A page, counting from 1, and a point on it in points from its
    /// top-left corner.
    Page { page: usize, x: f64, y: f64 },
}

/// A shared [`Editor`].
pub type SharedEditor = Arc<dyn Editor>;

/// The selection in the window. Lines and columns count from 1; `to` is
/// the place after the last selected character.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Selection {
    pub from_line: usize,
    pub from_column: usize,
    pub to_line: usize,
    pub to_column: usize,
    /// The selected text; cut if it is very long.
    pub text: String,
}

/// What `get_state` reports.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct EditorState {
    /// Name of the document's main file.
    pub file: String,
    /// Number of pages in the compiled document.
    pub pages: usize,
    /// The page at the middle of the window, counting from 1; 0 if the
    /// document has never compiled.
    pub page: usize,
    /// Zoom in pixels per typographic point.
    pub zoom: f64,
    /// Counts changes to the document.
    pub revision: u64,
    /// The revision the pages on screen were compiled from. Differs from
    /// `revision` while a compile is under way.
    pub compiled_revision: Option<u64>,
    /// `normal`, `insert`, `replace`, `visual`, `visual-line` or `command`.
    pub mode: String,
    /// The cursor's line in the source, counting from 1; 0 if the window
    /// only shows the document and does not edit it.
    pub line: usize,
    /// The cursor's column in its line, counting characters from 1.
    pub column: usize,
    /// Whether the text has changes that are not in the file.
    pub modified: bool,
    /// What is selected in the window, if anything.
    pub selection: Option<Selection>,
    /// What the window shows: `split` (source and pages), `source` or
    /// `page`.
    pub layout: String,
    /// `live` if changes by agents are made at once, `propose` if they
    /// wait for the user to accept them.
    pub agent_edits: String,
    /// How many proposed changes wait for the user.
    pub proposals: usize,
    /// The text on the command line, while it is open.
    pub command: Option<String>,
    /// Errors of the last compile, as `file:line:column: error: message`.
    /// Empty if it succeeded. While there are errors, the pages shown are
    /// those of the last revision that compiled.
    pub problems: Vec<String>,
    /// Whether a window shows the document to the user. Without one
    /// (`zetwerk mcp <file>` with no window open on it) nobody sees the
    /// pages or types into them.
    #[serde(default = "shown")]
    pub window: bool,
    /// Who writes the text to its file: `user` (with `:w` in the window),
    /// `user or agent` (the user lets agents `save` too), `agent` (there
    /// is no window: nothing is written unless you call `save`), or
    /// `nobody` (no window, and the user does not let agents save: what
    /// is changed is kept in zetwerk's journal, and comes back when the
    /// document is opened next).
    #[serde(default)]
    pub saves: String,
    /// Questions the user asked you about the text that wait for your
    /// answer (`list_notes`, `answer_note`).
    #[serde(default)]
    pub open_questions: usize,
    /// Notes the user left on the text that nobody has answered or
    /// resolved yet.
    #[serde(default)]
    pub unanswered_notes: usize,
}

/// A window, where nothing says otherwise.
const fn shown() -> bool {
    true
}
