//! The MCP tools and the socket they are served on.

#![allow(
    clippy::unused_async_trait_impl,
    reason = "rmcp's tool_handler macro generates async functions that do not await"
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, ErrorData, Implementation, ServerCapabilities, ServerConfig,
};
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use tokio::net::UnixListener;
use tokio::sync::oneshot;

use crate::SharedEditor;

/// Longest side of a rendered page unless the caller asks otherwise. Large
/// enough to read body text, small enough for a model's image input.
const DEFAULT_MAX_PX: u32 = 1500;
const LIMIT_MAX_PX: u32 = 4000;

#[derive(Clone)]
struct Server {
    editor: SharedEditor,
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct RenderPageArgs {
    /// Page number, counting from 1.
    page: usize,
    /// Longest side of the image in pixels (default 1500, at most 4000).
    max_px: Option<u32>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SendKeysArgs {
    /// Keys in Vim notation, e.g. `jj<C-d>:perf<CR>`. A literal `<` is `<lt>`.
    keys: String,
}

#[tool_router(router = document_tools)]
impl Server {
    /// Report what the editor is showing: file, page count, current page,
    /// zoom, revision, mode and compile errors.
    #[tool(annotations(title = "Editor state", read_only_hint = true))]
    async fn get_state(&self) -> Result<CallToolResult, ErrorData> {
        let editor = Arc::clone(&self.editor);
        let state = blocking(move || editor.state()).await?;
        Ok(
            match state.and_then(|state| serde_json::to_value(state).map_err(|e| e.to_string())) {
                Ok(value) => CallToolResult::structured(value),
                Err(message) => failure(message),
            },
        )
    }

    /// Render one page of the compiled document as a PNG. This is the real
    /// typeset page at the document's current revision, independent of what
    /// part of it the window shows.
    #[tool(annotations(title = "Render page", read_only_hint = true))]
    async fn render_page(
        &self,
        Parameters(args): Parameters<RenderPageArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let editor = Arc::clone(&self.editor);
        let max_px = args
            .max_px
            .unwrap_or(DEFAULT_MAX_PX)
            .clamp(16, LIMIT_MAX_PX);
        Ok(image(
            blocking(move || editor.render_page(args.page, max_px)).await?,
        ))
    }
}

#[tool_router(router = development_tools)]
impl Server {
    /// Press keys in the editor window, as if typed. For testing zetwerk
    /// itself; only available when the window was started for development.
    #[tool(annotations(title = "Send keys", read_only_hint = false))]
    async fn send_keys(
        &self,
        Parameters(args): Parameters<SendKeysArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let editor = Arc::clone(&self.editor);
        Ok(
            match blocking(move || editor.send_keys(&args.keys)).await? {
                Ok(()) => CallToolResult::success(vec![ContentBlock::text("keys sent")]),
                Err(message) => failure(message),
            },
        )
    }

    /// Capture the editor window as a PNG once it has finished drawing. For
    /// testing zetwerk itself; only available when the window was started
    /// for development.
    #[tool(annotations(title = "Window screenshot", read_only_hint = true))]
    async fn screenshot(&self) -> Result<CallToolResult, ErrorData> {
        let editor = Arc::clone(&self.editor);
        Ok(image(blocking(move || editor.screenshot()).await?))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("zetwerk", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "zetwerk is a WYSIWYG editor for Typst documents. These tools act on the \
                 document open in the user's zetwerk window.",
            )
    }
}

/// Runs an editor call off the async thread; editor calls block.
async fn blocking<T: Send + 'static>(
    call: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ErrorData> {
    tokio::task::spawn_blocking(call)
        .await
        .map_err(|err| ErrorData::internal_error(format!("editor call failed: {err}"), None))
}

/// A tool result reporting that the tool could not do what was asked. The
/// calling model sees the message and can react to it.
fn failure(message: String) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message)])
}

fn image(png: Result<Vec<u8>, String>) -> CallToolResult {
    match png {
        Ok(png) => {
            let data = base64::engine::general_purpose::STANDARD.encode(png);
            CallToolResult::success(vec![ContentBlock::image(data, "image/png")])
        }
        Err(message) => failure(message),
    }
}

/// A running server. Dropping it stops the server and removes its socket.
pub struct Running {
    socket: PathBuf,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Running {
    /// The socket the server listens on.
    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.socket
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// Starts serving `editor` on a Unix socket at `socket`. With
/// `development_tools`, the tools for driving and capturing the window
/// itself are offered too.
///
/// The server runs on one thread of its own, which sleeps while no client
/// is talking to it.
pub fn serve(
    socket: &Path,
    editor: SharedEditor,
    development_tools: bool,
) -> std::io::Result<Running> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    if let Some(directory) = socket.parent() {
        // Only the user may reach the socket: whoever can connect can read
        // and, later, change the document.
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)?;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    }
    // A socket file left behind by a crashed process would make `bind` fail.
    let _ = std::fs::remove_file(socket);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let listener = {
        let _guard = runtime.enter();
        UnixListener::bind(socket)?
    };
    let mut tool_router = Server::document_tools();
    if development_tools {
        tool_router += Server::development_tools();
    }
    let server = Server {
        editor,
        tool_router,
    };

    let (stop, mut stopped) = oneshot::channel();
    let thread = std::thread::Builder::new()
        .name("zetwerk-mcp".into())
        .spawn(move || {
            runtime.block_on(async move {
                loop {
                    tokio::select! {
                        _ = &mut stopped => break,
                        accepted = listener.accept() => {
                            let Ok((stream, _)) = accepted else { break };
                            let server = server.clone();
                            tokio::spawn(async move {
                                // A client that speaks nonsense or hangs up only
                                // ends its own connection.
                                if let Ok(connection) = server.serve(stream).await {
                                    let _ = connection.waiting().await;
                                }
                            });
                        }
                    }
                }
            });
        })?;
    Ok(Running {
        socket: socket.to_owned(),
        stop: Some(stop),
        thread: Some(thread),
    })
}
