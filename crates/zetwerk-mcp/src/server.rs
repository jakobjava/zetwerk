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

use crate::{
    Change, ChangeKind, Direction, ImageExport, ImageFormat, ImageOf, NewNote, NoteAnchor,
    NoteAnswer, Place, SharedEditor,
};

/// Longest side of a rendered page unless the caller asks otherwise. Large
/// enough to read body text, small enough for a model's image input.
const DEFAULT_MAX_PX: u32 = 1500;
const LIMIT_MAX_PX: u32 = 4000;

#[derive(Clone)]
struct Server {
    editor: SharedEditor,
    tool_router: ToolRouter<Self>,
    /// Whether there is no window: nobody but the agent saves then.
    headless: bool,
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
    /// Why you propose it, in a sentence or two: shown to the user with
    /// the proposal.
    reason: Option<String>,
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
struct AddSourceArgs {
    /// One BibTeX entry or several, as text:
    /// `@article{key, author = {Family, Given and …}, title = {…},
    /// journal = {…}, year = {2024}, doi = {…}}`. RIS and CSL-JSON
    /// are read too.
    bibtex: String,
    /// Add sources that are there already (the same DOI, the same title
    /// and year, or the same key and no title or DOI that differs) all
    /// the same, under keys of their own (default: false: they are
    /// reported in `there_already` and not added).
    #[serde(default)]
    anyway: bool,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct MoveSectionArgs {
    /// The line the section's heading is on, counting from 1, as
    /// `get_outline` gives it.
    line: usize,
    /// `up` puts the section before the section of its level that is
    /// before it under the same heading, `down` behind the one behind
    /// it; `deeper` and `shallower` change its level by one.
    direction: Direction,
    /// The `revision` you got the outline at. The move is refused if the
    /// text has changed since.
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
    id: Option<u64>,
    /// Several proposals at once, as `proposals` of `edit_source` or
    /// `move_section` gave them: all are taken back, or none where one
    /// of them is not there.
    ids: Option<Vec<u64>>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ListNotesArgs {
    /// Also the notes that are resolved (default false).
    include_resolved: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct AddNoteArgs {
    /// What the note says, to the user.
    note: String,
    /// The text the note is about, exactly as it is in the source; it has
    /// to be there exactly once. Give this or `line`.
    anchor_text: Option<String>,
    /// A line of the source, counting from 1: the note is about the
    /// paragraph (or heading, or item) on it.
    line: Option<usize>,
    /// The `revision` you read the source at. The note is refused if the
    /// text has changed since.
    expected_revision: Option<u64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct AnswerNoteArgs {
    /// The note's `id`, as `list_notes` gives it.
    id: u64,
    /// The answer, to the user.
    answer: String,
    /// Where a change is the answer: the text to replace, exactly as it is
    /// in the source and there once, as `edit_source` takes it. The change
    /// is proposed to the user with the answer, never made at once.
    old_str: Option<String>,
    /// What to put in its place.
    new_str: Option<String>,
    /// The `revision` you read the source at. The answer is refused if
    /// the text has changed since.
    expected_revision: Option<u64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct NoteArgs {
    /// The note's `id`, as `list_notes` gives it.
    id: u64,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ExportPdfArgs {
    /// Where to write the PDF: a name ending in `.pdf`, in the document's
    /// directory or below it. By default beside the document's main
    /// file, with its name.
    path: Option<String>,
    /// Which pages to write, counting from 1: `5`, `3-7`, `1,4-6`, `10-`
    /// (default: all).
    pages: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ExportImageArgs {
    /// `png` or `svg`.
    format: ImageFormat,
    /// Which pages to write, counting from 1: `5`, `3-7`, `1,4-6`, `10-`
    /// (default: all). Every page is a file of its own.
    pages: Option<String>,
    /// A line of the source, counting from 1, that is part of a figure
    /// (`#figure(…)`): only that figure is written, with its caption and
    /// without the rest of the page. Not together with `pages`.
    figure_at_line: Option<usize>,
    /// Where to write, as a name beside the document's main file ending
    /// in `.png` or `.svg`. By default the main file's name with that
    /// ending; of a figure, with the figure's label or number behind it.
    /// Several pages are numbered: `name-1.png`, `name-2.png`, ….
    path: Option<String>,
    /// Pixels to the inch of a PNG (default 144 for pages, 300 for a
    /// figure; 20 to 1200).
    dpi: Option<u32>,
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
    /// are made at once or wait as proposals, and how many wait; whether
    /// a `window` shows the document and who `saves` it; and how many
    /// questions the user asked you (`open_questions`) and notes of the
    /// user's (`unanswered_notes`) wait for an answer (`list_notes`).
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

    /// Write the compiled document as a PDF file: all of it, or the
    /// `pages` named. Fails if the document does not compile. Answers
    /// with the `path` written, and whether a file that was there was
    /// `replaced`.
    #[tool(annotations(title = "Export PDF", read_only_hint = false, idempotent_hint = true))]
    async fn export_pdf(
        &self,
        Parameters(args): Parameters<ExportPdfArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.value(move |editor| editor.export_pdf(args.path.as_deref(), args.pages.as_deref()))
            .await
    }

    /// Write pages of the compiled document as image files (`png` or
    /// `svg`), one file a page; or, with `figure_at_line`, one figure of
    /// it alone. Fails if the document does not compile, and never
    /// writes over a file the document is made from. Answers with the
    /// `files` written, each with its `path` and whether a file that
    /// was there was `replaced`.
    #[tool(annotations(title = "Export image", read_only_hint = false, idempotent_hint = true))]
    async fn export_image(
        &self,
        Parameters(args): Parameters<ExportImageArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let of = match (args.pages, args.figure_at_line) {
            (Some(_), Some(_)) => {
                return Ok(failure(
                    "give either `pages` or `figure_at_line`, not both".to_owned(),
                ));
            }
            (pages, None) => ImageOf::Pages(pages),
            (None, Some(line)) => ImageOf::FigureAtLine(line),
        };
        let image = ImageExport {
            format: args.format,
            of,
            dpi: args.dpi,
            path: args.path,
        };
        self.value(move |editor| editor.export_image(image)).await
    }

    /// The sources of the document: what its bibliography file (a BibTeX
    /// file beside it, or a Hayagriva `.yml` file) holds, each with its
    /// `key`, its `kind`, its fields and how often it is `cited` in the
    /// text (0: not at all), and the `file` they are in. `there_twice`
    /// pairs the keys of sources that seem to be one (the same DOI, or
    /// the same title and year). A source is cited in the text as
    /// `@key`.
    #[tool(annotations(title = "List sources", read_only_hint = true))]
    async fn list_sources(&self) -> Result<CallToolResult, ErrorData> {
        self.value(crate::Editor::sources).await
    }

    /// The figures of the file being edited, in the order they stand
    /// in: each with its `kind` (`image`, `table` or `other`), what it is
    /// `numbered` as ("Figure 2", "Table 1": as far as this file alone
    /// tells; null where it has no number), its `caption` as written,
    /// its `label` (refer to it as `@label`), the `line` its call begins
    /// on and, of an image, the `image` file. Tables and images that
    /// stand in no figure are listed too, with `in_figure` false. Change
    /// them with `edit_source`.
    #[tool(annotations(title = "List figures", read_only_hint = true))]
    async fn list_figures(&self) -> Result<CallToolResult, ErrorData> {
        self.value(crate::Editor::figures).await
    }

    /// What can be referred to: the sections, figures, tables and
    /// equations of the document and the sources of its bibliography.
    /// Each of `targets` has its `kind`, its `label` (null if it has
    /// none yet), its `number` as a reference shows it (null if the
    /// compiled document does not know it), its `title` (the title, the
    /// caption, the equation as written, or who wrote a source, when
    /// and what) and the `line` it begins on (with `file`, if that is
    /// not the file being edited). Refer to a thing as `@label`. One
    /// without a label needs one first: write `<label>` right behind it
    /// (at the end of a heading's line, behind the closing bracket of
    /// `#figure(…)`, behind an equation's last `$`), as `sec:…`,
    /// `fig:…`, `tab:…`, `eq:…`, and not one that is in use. A section
    /// or an equation whose `numbered` is false cannot be referred to
    /// by `@label` until it has a number.
    #[tool(annotations(title = "List targets", read_only_hint = true))]
    async fn list_targets(&self) -> Result<CallToolResult, ErrorData> {
        self.value(crate::Editor::targets).await
    }

    /// Check the spelling and the grammar of the running text of the
    /// file being edited (not its equations, code or raw text). All of
    /// it is checked, on the user's machine, as the window checks it:
    /// spelling by the Hunspell `dictionary` of the document's language
    /// and the user's own words, grammar by what `grammar` names (a
    /// `LanguageTool` server of the user's, or built-in rules for
    /// English); either is `null` where nothing checks. Answers with
    /// the `revision` checked and the `issues`: each with its `line`
    /// and `column` (from 1), the `text` it is about, its `kind`
    /// (`spelling` or `grammar`), a `message` and `suggestions` to
    /// write instead (an empty suggestion means: take the text out).
    /// `notes` say what could not be checked, such as a dictionary that
    /// is not installed. Fails if the user has checking switched off.
    #[tool(annotations(title = "Check language", read_only_hint = true))]
    async fn check_language(&self) -> Result<CallToolResult, ErrorData> {
        self.value(crate::Editor::check_language).await
    }
}

#[tool_router(router = editing_tools)]
impl Server {
    /// Add sources to the document's bibliography. They are given as
    /// BibTeX and put behind what the file holds; nothing in it is
    /// changed or taken out. Unlike the text, the file is written at
    /// once. Where the document does not list a bibliography yet, it is
    /// made to. Answers with the `keys` the sources are cited by, which
    /// are the ones given unless those were taken: cite with `@key` in
    /// the text (`edit_source`). A source that is there already (the
    /// same DOI, the same title and year, or the same key and nothing
    /// that tells them apart) is not added again: it is in
    /// `there_already`, with the key it is `cited_as`; cite that, or set
    /// `anyway`. The file may also be a Hayagriva `.yml` file: the
    /// sources are given as BibTeX all the same. zetwerk does not look
    /// up DOIs: resolve one yourself and hand over what it gives.
    /// Where the user wants changes proposed only, the sources are
    /// proposed: the answer has the `proposal`'s id and no `keys`, and
    /// nothing is written until the user adds them (`list_proposals`
    /// shows it with the `source` given; `withdraw_proposal` takes it
    /// back). Only a file in or below the document's directory is
    /// written to.
    #[tool(annotations(title = "Add source", read_only_hint = false))]
    async fn add_source(
        &self,
        Parameters(args): Parameters<AddSourceArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.value(move |editor| editor.add_source(&args.bibtex, args.anyway))
            .await
    }

    /// Change the document: replace `old_str` by `new_str`. `old_str` has
    /// to match the source exactly and, unless `replace_all` is set, be in
    /// it only once. The change is one undo step for the user and is not
    /// written to the file: the user saves. Answers with the new
    /// `revision`, the lines changed and the compile errors of the result
    /// (`diagnostics`; empty if it compiles) — or, for a proposal, with the
    /// ids of all it made as `proposals` (one for each place replaced),
    /// and the last of them as `proposal`.
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
            reason: args.reason,
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
            reason: None,
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
            reason: None,
        };
        self.value(move |editor| editor.change(change)).await
    }

    /// Rearrange the document by whole sections, without retyping them.
    /// A section is a heading and all up to the next heading of its
    /// level or above: its subsections go with it, and so do labels.
    /// `up` and `down` take it past the section of its level next to it
    /// under the same heading, and no further: it is refused where there
    /// is none. `deeper` and `shallower` change the level of the section
    /// and of all its subsections by one, between 1 and 6. Lines shift
    /// when a section moves: ask `get_outline` again before the next
    /// move. Answers like `edit_source`.
    #[tool(annotations(title = "Move section", read_only_hint = false))]
    async fn move_section(
        &self,
        Parameters(args): Parameters<MoveSectionArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let change = Change {
            what: ChangeKind::MoveSection {
                line: args.line,
                direction: args.direction,
            },
            expected_revision: args.expected_revision,
            propose: args.mode == Some(EditMode::Propose),
            reason: None,
        };
        self.value(move |editor| editor.change(change)).await
    }

    /// Take back a change you proposed, before the user has decided on
    /// it: give its `id`, or the `ids` of several (all the `proposals` a
    /// change made).
    #[tool(annotations(title = "Withdraw proposal", read_only_hint = false))]
    async fn withdraw_proposal(
        &self,
        Parameters(args): Parameters<ProposalArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        match (args.id, args.ids) {
            (Some(id), None) => self.value(move |editor| editor.withdraw_proposal(id)).await,
            (id, Some(mut ids)) => {
                ids.extend(id);
                self.value(move |editor| editor.withdraw_proposals(&ids))
                    .await
            }
            (None, None) => Ok(failure("give the `id` of a proposal, or `ids`".to_owned())),
        }
    }
}

#[tool_router(router = notes_tools)]
impl Server {
    /// The notes on the text: questions the user asks you about it
    /// (`to_agent` true), notes the user or an agent left on it, and what
    /// was answered. Each has its `id`, the `file`, `line` and `column`
    /// where the `text` it is about begins, that `text`, the `note`
    /// itself, who wrote it (`by`), the `answers` given since, whether it
    /// is `resolved`, whether its text is gone (`detached`), and the
    /// `proposals` made as answers to it that still wait. Answer a
    /// question with `answer_note`. Notes are not in the source, and not
    /// in what is exported.
    #[tool(annotations(title = "List notes", read_only_hint = true))]
    async fn list_notes(
        &self,
        Parameters(args): Parameters<ListNotesArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let all = args.include_resolved.unwrap_or(false);
        self.value(move |editor| editor.notes(all)).await
    }

    /// Leave a note for the user on the text: a remark, a doubt, a
    /// question. It is shown on the page by the text it is about, with
    /// `anchor_text` or `line`, and moves with that text as it is edited.
    /// Nothing in the text is changed. Answers with the note's `id`.
    #[tool(annotations(title = "Add note", read_only_hint = false))]
    async fn add_note(
        &self,
        Parameters(args): Parameters<AddNoteArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let anchor = match (args.anchor_text, args.line) {
            (Some(text), None) => NoteAnchor::Text(text),
            (None, Some(line)) => NoteAnchor::Line(line),
            _ => {
                return Ok(failure(
                    "give either `anchor_text` or `line`, not both".to_owned(),
                ));
            }
        };
        let note = NewNote {
            text: args.note,
            anchor,
            expected_revision: args.expected_revision,
        };
        self.value(move |editor| editor.add_note(note)).await
    }

    /// Answer a note: a question the user asked you, or a note left on
    /// the text. Where a change is the answer, give it as `old_str` and
    /// `new_str`: it is proposed to the user together with the answer,
    /// who sees it set on the page and accepts or rejects it. Answers with
    /// the `proposal`'s id, if one was made.
    #[tool(annotations(title = "Answer note", read_only_hint = false))]
    async fn answer_note(
        &self,
        Parameters(args): Parameters<AnswerNoteArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let change = match (args.old_str, args.new_str) {
            (Some(old), Some(new)) => Some((old, new)),
            (None, None) => None,
            _ => {
                return Ok(failure(
                    "give both `old_str` and `new_str`, or neither".to_owned(),
                ));
            }
        };
        let answer = NoteAnswer {
            id: args.id,
            text: args.answer,
            change,
            expected_revision: args.expected_revision,
        };
        self.value(move |editor| editor.answer_note(answer)).await
    }

    /// Mark a note resolved: what it asked is done. The user sees it
    /// resolved and may open it again.
    #[tool(annotations(title = "Resolve note", read_only_hint = false))]
    async fn resolve_note(
        &self,
        Parameters(args): Parameters<NoteArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.value(move |editor| editor.resolve_note(args.id)).await
    }
}

#[tool_router(router = saving_tools)]
impl Server {
    /// Write the text to the document's file. Refused if the file was
    /// changed by another program since it was read. Offered where the
    /// user lets agents save, and, unless the user says not, where there
    /// is no window (see `saves` in `get_state`).
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
        let instructions = if self.headless {
            HEADLESS
        } else {
            "zetwerk is a WYSIWYG editor for Typst documents. These tools act on the \
             document open in it, which the user may be looking at and typing into at \
             the same time. Read with read_source, change with edit_source (exact \
             text, replaced once), look at the result with render_page. Changes are \
             undo steps in the user's editor and are saved by the user, unless a save \
             tool is offered."
        };
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("zetwerk", env!("CARGO_PKG_VERSION")))
            .with_instructions(instructions)
    }
}

/// What an agent is told where no window shows the document.
const HEADLESS: &str = "zetwerk is a WYSIWYG editor for Typst documents. These tools act on a \
                        document that no window shows: nobody else sees it or types into it \
                        in this session. Read with read_source, change with edit_source \
                        (exact text, replaced once), look at the result with render_page. \
                        Nothing is written to the file unless you call save: the saves are \
                        yours to make (where the save tool is offered).";

/// What the save tool says of itself where no window shows the document.
const HEADLESS_SAVE: &str = "Write the text to the document's file. This session has no \
                             window: nobody else writes the text, and what you change is \
                             in the file only once you save. Refused if the file was \
                             changed by another program since it was read.";

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
    /// There is no window: the document is open for the agent alone.
    pub headless: bool,
}

impl Server {
    fn new(editor: SharedEditor, tools: Tools) -> Self {
        let mut tool_router =
            Self::document_tools() + Self::reading_tools() + Self::editing_tools();
        tool_router += Self::notes_tools();
        if tools.save {
            tool_router += Self::saving_tools();
            if let (true, Some(save)) = (tools.headless, tool_router.map.get_mut("save")) {
                save.attr.description = Some(HEADLESS_SAVE.into());
            }
        }
        if tools.development {
            tool_router += Self::development_tools();
        }
        Self {
            editor,
            tool_router,
            headless: tools.headless,
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
