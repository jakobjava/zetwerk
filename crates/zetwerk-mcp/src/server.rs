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

use crate::{Change, ChangeKind, Place, SharedEditor};

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
struct ReadSourceArgs {
    /// First line to read, counting from 1 (default: the first).
    from_line: Option<usize>,
    /// Last line to read (default: the last). At most 2000 lines are
    /// given at once.
    to_line: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct EditSourceArgs {
    /// The text to replace, exactly as it is in the source (without the
    /// line numbers `read_source` puts before the lines). It has to be
    /// in the source exactly once: include enough of the text around it.
    old_str: String,
    /// The text to put in its place.
    new_str: String,
    /// Replace every occurrence of `old_str` instead of requiring exactly
    /// one (default false).
    replace_all: Option<bool>,
    /// The `revision` you read the source at. The edit is refused if the
    /// text has changed since.
    expected_revision: Option<u64>,
    /// `apply` (default) makes the change; `propose` shows it to the user
    /// to accept or reject. Where the user asked for proposals, every
    /// change is a proposal.
    mode: Option<EditMode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
enum EditMode {
    Apply,
    Propose,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct TextArgs {
    /// The text to put in.
    text: String,
    /// The `revision` you last saw. The change is refused if the text has
    /// changed since.
    expected_revision: Option<u64>,
    /// `apply` (default) or `propose`; see `edit_source`.
    mode: Option<EditMode>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SearchArgs {
    /// The text to look for, literally (no patterns).
    query: String,
    /// Whether upper and lower case count as the same (default false).
    ignore_case: Option<bool>,
    /// How many places to report at most (default 50, at most 500).
    max_results: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct LocateArgs {
    /// A line of the source, counting from 1: asks where that place is on
    /// the pages. Give `line` (and `column`), or `page`, `x` and `y`.
    line: Option<usize>,
    /// The column in that line, counting characters from 1 (default 1).
    column: Option<usize>,
    /// A page, counting from 1: asks which place of the source is shown
    /// at the point `x`, `y` of it.
    page: Option<usize>,
    /// Points from the page's left edge.
    x: Option<f64>,
    /// Points from the page's top edge.
    y: Option<f64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ProposalArgs {
    /// The proposal's `id`, as `edit_source` or `list_proposals` gave it.
    id: u64,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ExportPdfArgs {
    /// Where to write the PDF. By default beside the document's main
    /// file, with its name and the ending `.pdf`.
    path: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SendKeysArgs {
    /// Keys in Vim notation, e.g. `jj<C-d>:perf<CR>`. A literal `<` is `<lt>`.
    keys: String,
}

#[tool_router(router = document_tools)]
impl Server {
    /// Report what the editor is showing and what the user is at: file,
    /// page count, current page, revision, mode, the cursor's line and
    /// column, the selected text, compile errors, whether your changes
    /// are made at once or wait as proposals, and how many wait.
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

impl Server {
    /// Runs an editor call that answers with a value.
    async fn value(
        &self,
        call: impl FnOnce(&dyn crate::Editor) -> Result<serde_json::Value, String> + Send + 'static,
    ) -> Result<CallToolResult, ErrorData> {
        let editor = Arc::clone(&self.editor);
        Ok(match blocking(move || call(&*editor)).await? {
            Ok(value) => CallToolResult::structured(value),
            Err(message) => failure(message),
        })
    }
}

#[tool_router(router = reading_tools)]
impl Server {
    /// Read the document's Typst source. Every line comes as its number, a
    /// tab and its text, after a first line that gives the `revision` of
    /// the text. Read before you edit; pass the revision to the editing
    /// tools as `expected_revision`.
    #[tool(annotations(title = "Read source", read_only_hint = true))]
    async fn read_source(
        &self,
        Parameters(args): Parameters<ReadSourceArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let editor = Arc::clone(&self.editor);
        let read = blocking(move || editor.read_source(args.from_line, args.to_line)).await?;
        Ok(match read {
            Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
            Err(message) => failure(message),
        })
    }

    /// The compile errors of the document as it is now (waits for the
    /// compile to finish). Each is `file:line:column: error: message`.
    #[tool(annotations(title = "Diagnostics", read_only_hint = true))]
    async fn get_diagnostics(&self) -> Result<CallToolResult, ErrorData> {
        self.value(crate::Editor::diagnostics).await
    }

    /// The headings of the document (level, text, line, and page once it
    /// is compiled) and its labels with their lines.
    #[tool(annotations(title = "Outline", read_only_hint = true))]
    async fn get_outline(&self) -> Result<CallToolResult, ErrorData> {
        self.value(crate::Editor::outline).await
    }

    /// Find text in the source. Reports line, column and the whole line of
    /// each place, and how many places there are in all.
    #[tool(annotations(title = "Search", read_only_hint = true))]
    async fn search(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let limit = args.max_results.unwrap_or(50).clamp(1, 500);
        let ignore_case = args.ignore_case.unwrap_or(false);
        self.value(move |editor| editor.search(&args.query, ignore_case, limit))
            .await
    }

    /// Relate the source to the pages: give `line` (and `column`) to learn
    /// the page and the point on it where that text is set, or `page`, `x`
    /// and `y` (in points from the page's top-left corner) to learn which
    /// line and column of the source is shown there. Use it with
    /// `render_page` to see what you are about to change.
    #[tool(annotations(title = "Locate", read_only_hint = true))]
    async fn locate(
        &self,
        Parameters(args): Parameters<LocateArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let place = match (args.line, args.page, args.x, args.y) {
            (Some(line), None, None, None) => Place::Source {
                line,
                column: args.column.unwrap_or(1),
            },
            (None, Some(page), Some(x), Some(y)) => Place::Page { page, x, y },
            _ => {
                return Ok(failure(
                    "give either `line` (and `column`), or `page`, `x` and `y`".to_owned(),
                ));
            }
        };
        self.value(move |editor| editor.locate(place)).await
    }

    /// The changes that were proposed and wait for the user to accept or
    /// reject them in the window.
    #[tool(annotations(title = "List proposals", read_only_hint = true))]
    async fn list_proposals(&self) -> Result<CallToolResult, ErrorData> {
        self.value(crate::Editor::proposals).await
    }

    /// Write the compiled document as a PDF file. Fails if the document
    /// does not compile.
    #[tool(annotations(title = "Export PDF", read_only_hint = false, idempotent_hint = true))]
    async fn export_pdf(
        &self,
        Parameters(args): Parameters<ExportPdfArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.value(move |editor| editor.export_pdf(args.path.as_deref()))
            .await
    }
}

#[tool_router(router = editing_tools)]
impl Server {
    /// Change the document: replace `old_str` by `new_str`. `old_str` has
    /// to match the source exactly and, unless `replace_all` is set, be in
    /// it only once. The change is one undo step for the user and is not
    /// written to the file: the user saves. Answers with the new
    /// `revision`, the lines changed and the compile errors of the result
    /// (`diagnostics`; empty if it compiles) — or, for a proposal, with its
    /// `id`.
    #[tool(annotations(title = "Edit source", read_only_hint = false))]
    async fn edit_source(
        &self,
        Parameters(args): Parameters<EditSourceArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let change = Change {
            what: ChangeKind::Replace {
                old: args.old_str,
                new: args.new_str,
                all: args.replace_all.unwrap_or(false),
            },
            expected_revision: args.expected_revision,
            propose: args.mode == Some(EditMode::Propose),
        };
        self.value(move |editor| editor.change(change)).await
    }

    /// Put text in at the user's cursor (see `get_state` for where it
    /// is). Answers like `edit_source`.
    #[tool(annotations(title = "Insert at cursor", read_only_hint = false))]
    async fn insert_at_cursor(
        &self,
        Parameters(args): Parameters<TextArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let change = Change {
            what: ChangeKind::Insert(args.text),
            expected_revision: args.expected_revision,
            propose: args.mode == Some(EditMode::Propose),
        };
        self.value(move |editor| editor.change(change)).await
    }

    /// Replace what the user has selected in the window (see `selection`
    /// in `get_state`) by other text. Answers like `edit_source`.
    #[tool(annotations(title = "Replace selection", read_only_hint = false))]
    async fn replace_selection(
        &self,
        Parameters(args): Parameters<TextArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let change = Change {
            what: ChangeKind::ReplaceSelection(args.text),
            expected_revision: args.expected_revision,
            propose: args.mode == Some(EditMode::Propose),
        };
        self.value(move |editor| editor.change(change)).await
    }

    /// Take back a change you proposed, before the user has decided on it.
    #[tool(annotations(title = "Withdraw proposal", read_only_hint = false))]
    async fn withdraw_proposal(
        &self,
        Parameters(args): Parameters<ProposalArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.value(move |editor| editor.withdraw_proposal(args.id))
            .await
    }
}

#[tool_router(router = saving_tools)]
impl Server {
    /// Write the text to the document's file. Refused if the file was
    /// changed by another program since it was read.
    #[tool(annotations(title = "Save", read_only_hint = false, idempotent_hint = true))]
    async fn save(&self) -> Result<CallToolResult, ErrorData> {
        self.value(crate::Editor::save).await
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
                 document open in it, which the user may be looking at and typing into at \
                 the same time. Read with read_source, change with edit_source (exact \
                 text, replaced once), look at the result with render_page. Changes are \
                 undo steps in the user's editor and are saved by the user, unless a save \
                 tool is offered.",
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

/// Which tools are offered besides those for reading and changing the
/// document.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tools {
    /// Writing the text to its file. The user's to do, unless they say
    /// otherwise or there is no window to do it in.
    pub save: bool,
    /// Driving and capturing the window itself, for developing zetwerk.
    pub development: bool,
}

impl Server {
    fn new(editor: SharedEditor, tools: Tools) -> Self {
        let mut tool_router =
            Self::document_tools() + Self::reading_tools() + Self::editing_tools();
        if tools.save {
            tool_router += Self::saving_tools();
        }
        if tools.development {
            tool_router += Self::development_tools();
        }
        Self {
            editor,
            tool_router,
        }
    }
}

/// Serves `editor` on standard input and output until the client hangs
/// up: for an agent that starts zetwerk itself, with no window.
pub fn serve_stdio(editor: SharedEditor, tools: Tools) -> std::io::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let connection = Server::new(editor, tools)
            .serve(rmcp::transport::stdio())
            .await
            .map_err(std::io::Error::other)?;
        connection.waiting().await.map_err(std::io::Error::other)?;
        Ok(())
    })
}

/// Starts serving `editor` on a Unix socket at `socket`, with `tools`
/// besides the ones for the document.
///
/// The server runs on one thread of its own, which sleeps while no client
/// is talking to it.
pub fn serve(socket: &Path, editor: SharedEditor, tools: Tools) -> std::io::Result<Running> {
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
    let server = Server::new(editor, tools);

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
