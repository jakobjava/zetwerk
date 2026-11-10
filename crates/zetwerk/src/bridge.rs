//! Lets the MCP server talk to the window.

use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use zetwerk_app::app::{Handle, Request, Screenshot};
use zetwerk_app::keys;
use zetwerk_app::viewer::Mode;
use zetwerk_mcp::{Editor, EditorState};

/// How long the window may take to answer something it can answer at once.
const PROMPT: Duration = Duration::from_secs(5);
/// How long to wait for a compile or for the window to finish drawing.
const PATIENT: Duration = Duration::from_secs(60);

/// An [`Editor`] backed by a window on another thread.
pub struct Bridge {
    handle: Handle,
}

impl Bridge {
    pub fn new(handle: Handle) -> Self {
        Self { handle }
    }

    /// Sends a request built around a reply channel and waits for the reply.
    fn ask<T>(
        &self,
        request: impl FnOnce(mpsc::Sender<T>) -> Request,
        patience: Duration,
    ) -> Result<T, String> {
        let (reply, answer): (_, Receiver<T>) = mpsc::channel();
        if !self.handle.send(request(reply)) {
            return Err("the zetwerk window has closed".to_owned());
        }
        answer
            .recv_timeout(patience)
            .map_err(|_| format!("the zetwerk window did not answer within {patience:?}"))
    }
}

impl Editor for Bridge {
    fn state(&self) -> Result<EditorState, String> {
        let state = self.ask(Request::State, PROMPT)?;
        let (mode, command) = match state.mode {
            Mode::Normal => ("normal", None),
            Mode::Command(text) => ("command", Some(text)),
        };
        Ok(EditorState {
            file: state.file,
            pages: state.pages,
            page: state.page,
            zoom: state.zoom,
            revision: state.revision,
            compiled_revision: state.compiled_revision,
            mode: mode.to_owned(),
            command,
            problems: state.problems,
        })
    }

    fn render_page(&self, page: usize, max_px: u32) -> Result<Vec<u8>, String> {
        let document = self
            .ask(Request::Document, PATIENT)?
            .ok_or("the document has not compiled yet; see `problems` in get_state")?;
        let pages = document.pages();
        let page = page
            .checked_sub(1)
            .and_then(|index| pages.get(index))
            .ok_or_else(|| {
                format!(
                    "page {page} does not exist; the document has {} page(s)",
                    pages.len()
                )
            })?;
        let longest = page.frame.width().max(page.frame.height()).to_pt().max(1.0);
        zetwerk_render::render_page(page, f64::from(max_px) / longest)
            .encode_png()
            .map_err(|err| format!("cannot encode the page: {err}"))
    }

    fn send_keys(&self, keys: &str) -> Result<(), String> {
        let keys = keys::parse(keys)?;
        if self.handle.send(Request::Keys(keys)) {
            Ok(())
        } else {
            Err("the zetwerk window has closed".to_owned())
        }
    }

    fn screenshot(&self) -> Result<Vec<u8>, String> {
        encode(self.ask(Request::Screenshot, PATIENT)??)
    }
}

/// A window capture as a PNG file.
pub fn encode(screenshot: Screenshot) -> Result<Vec<u8>, String> {
    let Screenshot {
        width,
        height,
        rgba,
    } = screenshot;
    tiny_skia::IntSize::from_wh(width, height)
        .and_then(|size| tiny_skia::Pixmap::from_vec(rgba, size))
        .ok_or("the screenshot has no pixels")?
        .encode_png()
        .map_err(|err| format!("cannot encode the screenshot: {err}"))
}
