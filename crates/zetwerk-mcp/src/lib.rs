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
mod server;
mod socket;

use std::sync::Arc;

pub use client::{Output, call};
pub use server::{Running, Tools, serve, serve_stdio};
pub use socket::{find_socket, proxy_stdio, socket_path};

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

    /// Writes the document as a PDF to `path`, or beside its main file.
    fn export_pdf(&self, path: Option<&str>) -> Result<serde_json::Value, String>;

    /// Writes the text to its file.
    fn save(&self) -> Result<serde_json::Value, String>;
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
}
