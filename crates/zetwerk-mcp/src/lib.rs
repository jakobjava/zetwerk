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
pub use server::{Running, serve};
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
}

/// A shared [`Editor`].
pub type SharedEditor = Arc<dyn Editor>;

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
    /// The text on the command line, while it is open.
    pub command: Option<String>,
    /// Errors of the last compile, as `file:line:column: error: message`.
    /// Empty if it succeeded. While there are errors, the pages shown are
    /// those of the last revision that compiled.
    pub problems: Vec<String>,
}
