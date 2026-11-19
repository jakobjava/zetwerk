//! Lets the MCP server talk to the window.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zetwerk_app::app::{Handle, Request, Screenshot, Task};
use zetwerk_app::editor::Mode as EditMode;
use zetwerk_app::keys;
use zetwerk_app::sections::Way;
use zetwerk_app::viewer::agent::{AgentEdits, Outcome};
use zetwerk_app::viewer::export::{Exported, Format, Pages, Wanted, What};
use zetwerk_app::viewer::notes::{Author, Listed, NoteAnchor as Anchor};
use zetwerk_app::viewer::review::{Agent, Kind as Suggested, Wanted as WantedSuggestion};
use zetwerk_app::viewer::{Mode, Panes, Viewer};
use zetwerk_mcp::{
    Caller, Change, ChangeKind, Direction, Editor, EditorState, ImageExport, ImageFormat, ImageOf,
    NewNote, NewSuggestion, NoteAnchor, NoteAnswer, Place, Revision, Selection, Suggest,
};

/// How long the window may take to answer something it can answer at once.
const PROMPT: Duration = Duration::from_secs(5);
/// How long to wait for a compile or for the window to finish drawing.
const PATIENT: Duration = Duration::from_secs(60);

/// An [`Editor`] backed by a window on another thread.
pub struct Bridge {
    handle: Handle,
    /// Whether a window shows the document, and whether the agent is
    /// given `save`: what it is told of who saves.
    window: bool,
    save: bool,
}

/// What the window shows, that a request is for: the main file and the
/// file being edited.
type Shown = (Option<PathBuf>, Option<PathBuf>);

/// Why a request that waited is not answered: the user went to another
/// document meanwhile.
/// The agent whose request this thread answers, as it introduced
/// itself to the MCP server.
fn caller() -> Option<zetwerk_app::viewer::authors::Agent> {
    zetwerk_mcp::caller().map(|caller| zetwerk_app::viewer::authors::Agent {
        name: caller.name,
        version: caller.version,
        session: caller.session,
    })
}

const SWITCHED: &str = "the window shows another document now: what you asked of the one it \
                        showed was not done; see get_state for what it shows";

impl Bridge {
    pub fn new(handle: Handle) -> Self {
        Self {
            handle,
            window: true,
            save: false,
        }
    }

    /// The same, for a document that a window shows or not, with `save`
    /// offered to agents or not.
    #[must_use]
    pub fn offering(mut self, window: bool, save: bool) -> Self {
        self.window = window;
        self.save = save;
        self
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

    /// Does `task` with the viewer and gives what it gave.
    fn now<T: Send + 'static>(
        &self,
        task: impl FnOnce(&mut Viewer) -> T + Send + 'static,
    ) -> Result<T, String> {
        self.ask(
            |reply| {
                let task: Task = Box::new(move |viewer| {
                    viewer.agent_called(std::any::type_name_of_val(&task));
                    let _ = reply.send(task(viewer));
                });
                Request::Do(task)
            },
            PROMPT,
        )
    }

    /// What the window shows now.
    fn shown(&self) -> Result<Shown, String> {
        self.now(|viewer| viewer.shown_document())
    }

    /// Like [`Bridge::compiled`], for the document `shown`: where the
    /// window shows another by the time it is compiled, refused.
    fn compiled_for<T: Send + 'static>(
        &self,
        shown: Shown,
        task: impl FnOnce(&mut Viewer) -> T + Send + 'static,
    ) -> Result<T, String> {
        let answer = self.compiled(move |viewer| {
            let same = viewer.shown_document() == shown;
            same.then(|| task(viewer))
        })?;
        answer.ok_or_else(|| SWITCHED.to_owned())
    }

    /// Like [`Bridge::now`], once the text as it is has been compiled.
    fn compiled<T: Send + 'static>(
        &self,
        task: impl FnOnce(&mut Viewer) -> T + Send + 'static,
    ) -> Result<T, String> {
        self.ask(
            |reply| {
                let task: Task = Box::new(move |viewer| {
                    viewer.agent_called(std::any::type_name_of_val(&task));
                    let _ = reply.send(task(viewer));
                });
                Request::WhenCompiled(task)
            },
            PATIENT,
        )
    }
}

impl Bridge {
    /// Writes the export `wanted` of the text as it is, once that is
    /// compiled. The window only hands over its pages; the files are
    /// made and written here, on the thread of whoever asked.
    fn export(&self, wanted: Wanted) -> Result<Exported, String> {
        // Of the document shown when it was asked for, or of none.
        let shown = self.shown()?;
        let job = self.compiled_for(shown, move |viewer| viewer.plan_export(&wanted))??;
        job.run().map_err(|why| format!("not exported: {why}"))
    }
}

/// Makes the change `what`, or proposes it, as an agent asked.
fn made(
    viewer: &mut Viewer,
    what: ChangeKind,
    expected: Option<u64>,
    propose: bool,
) -> Result<Outcome, String> {
    match what {
        ChangeKind::Replace { old, new, all } => {
            viewer.agent_replace(&old, &new, all, expected, propose)
        }
        ChangeKind::Insert(text) => viewer.agent_insert(&text, expected, propose),
        ChangeKind::ReplaceSelection(text) => {
            viewer.agent_replace_selection(&text, expected, propose)
        }
        ChangeKind::MoveSection { line, direction } => {
            let way = match direction {
                Direction::Up => Way::Up,
                Direction::Down => Way::Down,
                Direction::Deeper => Way::Deeper,
                Direction::Shallower => Way::Shallower,
            };
            viewer.agent_move_section(line, way, expected, propose)
        }
        ChangeKind::InsertPages(wanted) => {
            let wanted = zetwerk_app::page_text::Wanted {
                file: inside(Some(&wanted.file))?.unwrap_or_default(),
                pages: wanted.pages,
                place: wanted.place,
                fit: wanted.fit,
                counted: wanted.counted,
            };
            viewer.agent_insert_pages(&wanted, expected, propose)
        }
    }
}

/// The agent `caller` is, as reviewing names it; the one calling on
/// this thread where none is given.
fn agent_of(caller: Option<&Caller>) -> Agent {
    let caller = caller.cloned().or_else(zetwerk_mcp::caller);
    caller.map_or_else(Agent::unknown, |caller| Agent {
        name: caller.name,
        session: caller.session,
    })
}

/// A suggestion as reviewing takes it.
fn wanted(suggestion: NewSuggestion) -> WantedSuggestion {
    WantedSuggestion {
        kind: suggestion.kind,
        old_str: suggestion.old_str,
        section: suggestion.section,
        line: suggestion.line,
        new_str: suggestion.new_str,
        position: suggestion.position,
        move_to: suggestion.move_to,
        reason: suggestion.reason,
        category: suggestion.category,
        confidence: suggestion.confidence,
        importance: suggestion.importance,
        alternatives: suggestion.alternatives,
        insist: suggestion.insist,
    }
}

/// A note as an agent is given it.
fn listed_note(note: Listed) -> Value {
    let said = |(by, text): (Author, String)| json!({ "by": by.name(), "text": text });
    json!({
        "id": note.id,
        "file": note.file,
        "line": note.line,
        "column": note.column,
        "text": note.text,
        "note": note.note,
        "by": note.by.name(),
        "to_agent": note.to_agent,
        "answers": note.answers.into_iter().map(said).collect::<Vec<Value>>(),
        "resolved": note.resolved,
        "detached": note.detached,
        "proposals": note.proposals,
    })
}

/// The file name an agent gave for an export, if it is one in or below
/// the document's directory: an agent writes nowhere else.
fn inside(name: Option<&str>) -> Result<Option<String>, String> {
    use std::path::{Component, Path};
    let Some(name) = name else {
        return Ok(None);
    };
    let within = Path::new(name)
        .components()
        .all(|part| matches!(part, Component::Normal(_) | Component::CurDir));
    if within {
        Ok(Some(name.to_owned()))
    } else {
        Err(format!(
            "{name} is not in the document's directory: give a name there, as `paper.pdf` \
             or `out/paper.pdf`"
        ))
    }
}

/// The path `path` comes to, `..` and `.` taken as they lead, and links
/// followed where it is there.
fn resolved(path: &Path) -> PathBuf {
    use std::path::Component;
    if let Ok(real) = path.canonicalize() {
        return real;
    }
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    match (out.parent(), out.file_name()) {
        (Some(folder), Some(name)) if folder.exists() => resolved(folder).join(name),
        _ => out,
    }
}

/// Whether an agent may write the file at `path` of the document whose
/// main file is `main`: only in or below the main file's directory, as
/// [`inside`] has it for what it names.
fn agent_may_write(path: &Path, main: &Path) -> Result<(), String> {
    let folder = resolved(main.parent().unwrap_or(Path::new(".")));
    let within = resolved(path)
        .strip_prefix(&folder)
        .map(|within| within.to_string_lossy().into_owned());
    match within {
        Ok(within) => inside(Some(&within)).map(|_| ()),
        Err(_) => Err(format!(
            "the document's bibliography is {}, outside the document's directory: an agent \
             writes nowhere else. Ask the user to add the source (:bib)",
            path.display()
        )),
    }
}

/// The pages `text` names, as an agent wrote them.
fn named_pages(text: &str) -> Result<Pages, String> {
    Pages::parse(text.trim()).ok_or_else(|| {
        format!("{text:?} names no pages; write them as `5`, `3-7`, `1,4-6` or `10-`")
    })
}

impl Editor for Bridge {
    fn state(&self) -> Result<EditorState, String> {
        let (state, focus, panes, agent_edits, proposals, (questions, notes)) =
            self.now(|viewer| {
                (
                    viewer.state(),
                    viewer.focus(),
                    viewer.panes(),
                    viewer.agent_edits(),
                    viewer.proposals().len() + viewer.proposed_sources().len(),
                    viewer.notes_waiting(),
                )
            })?;
        let saves = match (self.window, self.save) {
            (true, false) => "user",
            (true, true) => "user or agent",
            (false, true) => "agent",
            (false, false) => "nobody",
        };
        let (mode, command) = match (state.mode, &state.editing) {
            (Mode::Command(text), _) => ("command", Some(text)),
            (Mode::Normal, Some(editing)) => {
                let mode = match editing.mode {
                    EditMode::Normal => "normal",
                    EditMode::Insert if editing.replacing => "replace",
                    EditMode::Insert => "insert",
                    EditMode::Visual => "visual",
                    EditMode::VisualLine => "visual-line",
                    EditMode::Command => "command",
                };
                (mode, editing.command.clone())
            }
            (Mode::Normal, None) => ("normal", None),
        };
        let (line, column, modified) = state.editing.as_ref().map_or((0, 0, false), |editing| {
            (editing.line, editing.column, editing.modified)
        });
        Ok(EditorState {
            file: state.file,
            pages: state.pages,
            page: state.page,
            zoom: state.zoom,
            revision: state.revision,
            compiled_revision: state.compiled_revision,
            mode: mode.to_owned(),
            line,
            column,
            modified,
            selection: focus
                .and_then(|focus| focus.selection)
                .map(|selected| Selection {
                    from_line: selected.from.0,
                    from_column: selected.from.1,
                    to_line: selected.to.0,
                    to_column: selected.to.1,
                    text: selected.text,
                }),
            layout: match panes {
                Panes::Split => "split",
                Panes::Source => "source",
                Panes::Page => "page",
            }
            .to_owned(),
            agent_edits: match agent_edits {
                AgentEdits::Live => "live",
                AgentEdits::Propose => "propose",
            }
            .to_owned(),
            proposals,
            command,
            problems: state.problems,
            window: self.window,
            saves: saves.to_owned(),
            open_questions: questions,
            unanswered_notes: notes,
        })
    }

    fn read_source(&self, from: Option<usize>, to: Option<usize>) -> Result<String, String> {
        let lines = self.now(move |viewer| viewer.lines(from, to))??;
        Ok(format!(
            "revision {}, lines {} to {} of {}\n{}",
            lines.revision, lines.first, lines.last, lines.total, lines.text
        ))
    }

    fn change(&self, change: Change) -> Result<Value, String> {
        let Change {
            what,
            expected_revision: expected,
            propose,
            reason,
        } = change;
        let agent = agent_of(None);
        let author = caller();
        let moving = matches!(what, ChangeKind::MoveSection { .. });
        let (outcome, made_ids, shown, suggestions) = self.now(move |viewer| {
            let first = viewer.next_proposal_id();
            let outcome = viewer.as_agent(author, |viewer| made(viewer, what, expected, propose));
            if let (Ok(Outcome::Proposed { .. }), Some(reason)) = (&outcome, &reason) {
                viewer.explain_proposals(first, reason);
            }
            // Every proposal it made: one for each place replaced, or for
            // each part of a section moved.
            let ids: Vec<u64> = (first..viewer.next_proposal_id())
                .filter(|id| viewer.proposals().iter().any(|proposal| proposal.id == *id))
                .collect();
            // What is proposed is a suggestion of this agent's.
            let kind = moving.then_some(Suggested::Move);
            let suggestions = match &outcome {
                Ok(outcome) => viewer.adopt_outcome(outcome, &agent, kind),
                Err(_) => Vec::new(),
            };
            outcome.map(|outcome| (outcome, ids, viewer.shown_document(), suggestions))
        })??;
        match outcome {
            Outcome::Proposed { id, revision } => Ok(json!({
                "applied": false,
                "proposal": id,
                "proposals": made_ids,
                "suggestions": suggestions,
                "revision": revision,
                "note": "shown to the user, who accepts or rejects it in the window; \
                         list_suggestions tells what became of it",
            })),
            Outcome::Changed { revision, lines } => {
                // What the compiler says to the text as it is by then, if
                // it is still the text of that document.
                let (state, now, back) = self.compiled_for(shown, |viewer| {
                    let state = viewer.state();
                    (state.clone(), state.revision, viewer.pages_look_back())
                })?;
                if back {
                    return Ok(json!({
                        "applied": true,
                        "revision": revision,
                        "first_line": lines.0,
                        "last_line": lines.1,
                        "diagnostics": null,
                        "note": LOOKING_BACK,
                    }));
                }
                let (diagnostics, more) = some_of(&state.problems);
                Ok(json!({
                    "applied": true,
                    "revision": revision,
                    "first_line": lines.0,
                    "last_line": lines.1,
                    "diagnostics": diagnostics,
                    "more_diagnostics": more,
                    "diagnostics_revision": now,
                    "pages": state.pages,
                }))
            }
        }
    }

    fn diagnostics(&self) -> Result<Value, String> {
        let shown = self.shown()?;
        let (state, back) =
            self.compiled_for(shown, |viewer| (viewer.state(), viewer.pages_look_back()))?;
        if back {
            return Err(LOOKING_BACK.to_owned());
        }
        let (diagnostics, more) = some_of(&state.problems);
        Ok(json!({
            "revision": state.revision,
            "diagnostics": diagnostics,
            "more_diagnostics": more,
            "pages": state.pages,
        }))
    }

    fn outline(&self) -> Result<Value, String> {
        let shown = self.shown()?;
        let (outline, revision, pages) = self.compiled_for(shown, |viewer| {
            (
                viewer.outline(),
                viewer.state().revision,
                viewer.agent_pages_put_in(),
            )
        })?;
        let outline = outline?;
        let headings: Vec<Value> = outline
            .headings
            .iter()
            .map(|heading| {
                json!({
                    "level": heading.level,
                    "text": heading.text,
                    "line": heading.line,
                    "page": heading.page,
                })
            })
            .collect();
        let labels: Vec<Value> = outline
            .labels
            .iter()
            .map(|(name, line)| json!({ "name": name, "line": line }))
            .collect();
        Ok(
            json!({ "revision": revision, "headings": headings, "labels": labels, "inserted_pages": pages }),
        )
    }

    fn search(&self, query: &str, ignore_case: bool, limit: usize) -> Result<Value, String> {
        let query = query.to_owned();
        let ((found, total), revision) = self.now(move |viewer| {
            viewer
                .search(&query, ignore_case, limit)
                .map(|found| (found, viewer.state().revision))
        })??;
        let matches: Vec<Value> = found
            .iter()
            .map(|at| json!({ "line": at.line, "column": at.column, "text": at.text }))
            .collect();
        Ok(json!({ "revision": revision, "total": total, "matches": matches }))
    }

    fn locate(&self, place: Place) -> Result<Value, String> {
        let shown = self.shown()?;
        match place {
            Place::Source { line, column } => {
                let (page, x, y) =
                    self.compiled_for(shown, move |viewer| viewer.page_of(line, column))??;
                Ok(json!({ "page": page, "x": round(x), "y": round(y) }))
            }
            Place::Page { page, x, y } => {
                let (file, line, column) =
                    self.compiled_for(shown, move |viewer| viewer.source_of(page, x, y))??;
                Ok(json!({ "file": file, "line": line, "column": column }))
            }
        }
    }

    fn proposals(&self) -> Result<Value, String> {
        let proposals = self.now(|viewer| {
            let text = viewer.text().unwrap_or_default();
            let line = |offset: usize| zetwerk_app::viewer::agent::line_column(text, offset).0;
            // Sources proposed for the bibliography, after the changes
            // of the text.
            let sources = viewer
                .proposed_sources()
                .into_iter()
                .map(|(id, given)| json!({ "id": id, "source": given }));
            let text_ones: Vec<Value> = viewer
                .proposals()
                .iter()
                .map(|proposal| {
                    let suggestion = viewer.suggestion_of_proposal(proposal.id).map(|s| s.id);
                    let mut listed = json!({
                        "id": proposal.id,
                        "suggestion": suggestion,
                        "line": line(proposal.range.start),
                        "old_str": proposal.old,
                        "new_str": proposal.new,
                        "stale": proposal.stale,
                    });
                    // Only what was given with it.
                    let (reason, note) = viewer.proposal_about(proposal.id);
                    if let Some(fields) = listed.as_object_mut() {
                        if let Some(reason) = reason {
                            fields.insert("reason".to_owned(), json!(reason));
                        }
                        if let Some(note) = note {
                            fields.insert("note".to_owned(), json!(note));
                        }
                    }
                    listed
                })
                .collect();
            text_ones.into_iter().chain(sources).collect::<Vec<Value>>()
        })?;
        Ok(json!({ "proposals": proposals }))
    }

    fn withdraw_proposals(&self, ids: &[u64]) -> Result<Value, String> {
        let wanted = ids.to_vec();
        let left = self.now(move |viewer| {
            viewer
                .withdraw_proposals(&wanted)
                .map(|()| viewer.proposals().len())
        })??;
        Ok(json!({ "withdrawn": ids, "waiting": left }))
    }

    fn withdraw_proposal(&self, id: u64) -> Result<Value, String> {
        let left = self.now(move |viewer| {
            viewer
                .withdraw_proposal(id)
                .map(|()| viewer.proposals().len())
        })??;
        Ok(json!({ "withdrawn": id, "waiting": left }))
    }

    fn export_pdf(&self, path: Option<&str>, pages: Option<&str>) -> Result<Value, String> {
        let outcome = self.export(Wanted {
            format: Format::Pdf,
            what: What::Pages(pages.map(named_pages).transpose()?),
            dpi: None,
            file: inside(path)?,
            replace: false,
        })?;
        let [file] = outcome.files.as_slice() else {
            return Err("the PDF did not come out as one file".to_owned());
        };
        Ok(json!({
            "path": file.path.display().to_string(),
            "bytes": file.bytes,
            "replaced": file.replaced,
            "pages": outcome.pages,
            "of": outcome.of,
        }))
    }

    fn export_image(&self, image: ImageExport) -> Result<Value, String> {
        let outcome = self.export(Wanted {
            format: match image.format {
                ImageFormat::Png => Format::Png,
                ImageFormat::Svg => Format::Svg,
            },
            what: match image.of {
                ImageOf::Pages(pages) => {
                    What::Pages(pages.as_deref().map(named_pages).transpose()?)
                }
                ImageOf::FigureAtLine(line) => What::FigureAtLine(line),
            },
            dpi: image.dpi.map(f64::from),
            file: inside(image.path.as_deref())?,
            replace: false,
        })?;
        let files: Vec<Value> = outcome
            .files
            .iter()
            .map(|file| {
                json!({
                    "path": file.path.display().to_string(),
                    "bytes": file.bytes,
                    "replaced": file.replaced,
                })
            })
            .collect();
        Ok(json!({ "files": files, "pages": outcome.pages, "of": outcome.of }))
    }

    fn sources(&self) -> Result<Value, String> {
        let (sources, cited) = self.now(|viewer| {
            let sources = viewer.sources()?;
            let cited = viewer.citation_counts()?;
            Ok::<_, String>((sources, cited))
        })??;
        let entries: Vec<Value> = sources
            .entries
            .iter()
            .map(|entry| {
                let fields: serde_json::Map<String, Value> = entry
                    .fields
                    .iter()
                    .map(|(name, value)| (name.clone(), json!(value)))
                    .collect();
                let times = cited.get(&entry.key).copied().unwrap_or(0);
                json!({ "key": entry.key, "kind": entry.kind, "fields": fields, "cited": times })
            })
            .collect();
        let twice: Vec<Value> = zetwerk_app::bib::same::pairs(&sources.entries)
            .into_iter()
            .map(|(first, second)| json!([sources.entries[first].key, sources.entries[second].key]))
            .collect();
        Ok(json!({
            "file": sources.path.display().to_string(),
            "listed_by_the_document": sources.listed,
            "format": if sources.hayagriva { "hayagriva" } else { "bibtex" },
            "sources": entries,
            "there_twice": twice,
        }))
    }

    fn figures(&self) -> Result<Value, String> {
        let figures = self.now(|viewer| viewer.figures())??;
        let listed: Vec<Value> = figures
            .iter()
            .map(|figure| {
                let numbered = figure.number.as_ref().map(|_| figure.name());
                json!({
                    "kind": figure.kind.name(),
                    "in_figure": figure.framed,
                    "numbered": numbered,
                    "caption": figure.caption,
                    "label": figure.label,
                    "line": figure.line,
                    "image": figure.file.as_ref().map(|(_, file)| file),
                })
            })
            .collect();
        Ok(json!({ "figures": listed }))
    }

    fn packages(&self) -> Result<Value, String> {
        self.now(|viewer| viewer.packages_listed())
    }

    fn targets(&self) -> Result<Value, String> {
        let shown = self.shown()?;
        let (targets, revision) = self.compiled_for(shown, |viewer| {
            (viewer.referables(), viewer.state().revision)
        })?;
        let targets: Vec<Value> = targets?
            .into_iter()
            .map(|target| {
                let mut listed = json!({
                    "kind": target.kind,
                    "label": target.label,
                    "number": target.number,
                    "numbered": target.numbered,
                    "title": target.called,
                    "line": target.line,
                });
                // Only what is in another file says which.
                if let (Some(file), Some(fields)) = (target.file, listed.as_object_mut()) {
                    fields.insert("file".to_owned(), json!(file));
                }
                listed
            })
            .collect();
        Ok(json!({ "revision": revision, "targets": targets }))
    }

    fn check_language(&self) -> Result<Value, String> {
        // The window hands out the check; the waiting is done here.
        let check = self.now(Viewer::language_check)??;
        let report = check()?;
        let issues: Vec<Value> = report
            .issues
            .iter()
            .map(|issue| {
                json!({
                    "line": issue.line,
                    "column": issue.column,
                    "text": issue.text,
                    "kind": if issue.grammar { "grammar" } else { "spelling" },
                    "message": issue.message,
                    "suggestions": issue.suggestions,
                })
            })
            .collect();
        Ok(json!({
            "revision": report.revision,
            "dictionary": report.dictionary,
            "grammar": report.grammar,
            "issues": issues,
            "notes": report.notes,
        }))
    }

    fn add_source(&self, bibtex: &str, anyway: bool) -> Result<Value, String> {
        let bibtex = bibtex.to_owned();
        let (added, revision) = self.now(move |viewer| {
            // The user's `:bib` writes where the document points; an
            // agent only in or below the document's directory.
            let path = viewer.sources()?.path;
            let main = viewer.main_path().or_else(|| viewer.shown_document().1);
            if let Some(main) = main {
                agent_may_write(&path, &main)?;
            }
            viewer
                .agent_add_sources_checked(&bibtex, anyway)
                .map(|added| (added, viewer.state().revision))
        })??;
        if let Some(id) = added.proposal {
            return Ok(json!({
                "applied": false,
                "proposal": id,
                "keys": [],
                "revision": revision,
                "note": "proposed: the user adds it in the window (:bib), or not; nothing \
                         is written before",
            }));
        }
        let there: Vec<Value> = added
            .there
            .iter()
            .map(|there| {
                json!({ "given": there.given, "source": there.line, "cited_as": there.cited_as })
            })
            .collect();
        Ok(json!({ "keys": added.keys, "there_already": there, "revision": revision }))
    }

    fn save(&self) -> Result<Value, String> {
        let revision =
            self.now(|viewer| viewer.agent_save().map(|()| viewer.state().revision))??;
        Ok(json!({ "saved": true, "revision": revision }))
    }

    fn render_page(&self, page: usize, max_px: u32) -> Result<Vec<u8>, String> {
        let shown = self.shown()?;
        let document = self
            .compiled_for(shown, |viewer| viewer.document().cloned())?
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
        // An agent's keys fetch no package (see `viewer::packages`).
        let task: Task = Box::new(move |viewer| viewer.keys_of_agent(&keys));
        if self.handle.send(Request::Do(task)) {
            Ok(())
        } else {
            Err("the zetwerk window has closed".to_owned())
        }
    }

    fn screenshot(&self) -> Result<Vec<u8>, String> {
        encode(self.ask(Request::Screenshot, PATIENT)??)
    }

    fn notes(&self, include_resolved: bool) -> Result<Value, String> {
        let (notes, revision) = self.now(move |viewer| {
            (
                viewer.notes_listed(include_resolved),
                viewer.state().revision,
            )
        })?;
        let notes: Vec<Value> = notes.into_iter().map(listed_note).collect();
        Ok(json!({ "revision": revision, "notes": notes }))
    }

    fn add_note(&self, note: NewNote) -> Result<Value, String> {
        let (id, line) = self.now(move |viewer| {
            let anchor = match &note.anchor {
                NoteAnchor::Text(text) => Anchor::Text(text),
                NoteAnchor::Line(line) => Anchor::Line(*line),
            };
            viewer.agent_add_note(&note.text, anchor, note.expected_revision)
        })??;
        Ok(json!({ "id": id, "line": line }))
    }

    fn answer_note(&self, answer: NoteAnswer) -> Result<Value, String> {
        let id = answer.id;
        let agent = agent_of(None);
        let author = caller();
        let (proposal, suggestion) = self.now(move |viewer| {
            let change = answer
                .change
                .as_ref()
                .map(|(old, new)| (old.as_str(), new.as_str()));
            let proposal = viewer.as_agent(author, |viewer| {
                viewer.agent_answer_note(answer.id, &answer.text, change, answer.expected_revision)
            });
            let made = viewer.adopt_proposals(Some(&agent), None, None);
            proposal.map(|proposal| (proposal, made.first().map(|(_, id)| *id)))
        })??;
        Ok(json!({ "answered": id, "proposal": proposal, "suggestion": suggestion }))
    }

    fn resolve_note(&self, id: u64) -> Result<Value, String> {
        self.now(move |viewer| viewer.agent_resolve_note(id))??;
        Ok(json!({ "resolved": id }))
    }

    fn suggest(&self, caller: &Caller, suggest: Suggest) -> Result<Value, String> {
        let agent = agent_of(Some(caller));
        let batch: Vec<WantedSuggestion> = suggest.suggestions.into_iter().map(wanted).collect();
        self.now(move |viewer| {
            viewer.agent_suggest(
                &batch,
                suggest.request,
                suggest.request_done,
                suggest.expected_revision,
                suggest.all_or_nothing,
                &agent,
            )
        })?
    }

    fn suggestions(
        &self,
        states: &[String],
        by: Option<&str>,
        since: Option<u64>,
    ) -> Result<Value, String> {
        let (states, by) = (states.to_vec(), by.map(str::to_owned));
        self.now(move |viewer| viewer.suggestions_listed(&states, by.as_deref(), since))?
    }

    fn revise_suggestion(&self, caller: &Caller, revision: Revision) -> Result<Value, String> {
        let agent = agent_of(Some(caller));
        self.now(move |viewer| {
            viewer.agent_revise(
                revision.id,
                revision.new_str.as_deref(),
                revision.reason.as_deref(),
                revision.alternatives,
                revision.message.as_deref(),
                revision.expected_revision,
                &agent,
            )
        })?
    }

    fn withdraw_suggestions(&self, caller: &Caller, ids: &[u64]) -> Result<Value, String> {
        let (agent, ids) = (agent_of(Some(caller)), ids.to_vec());
        self.now(move |viewer| viewer.agent_withdraw(&ids, &agent))?
    }

    fn brief(&self) -> Result<Value, String> {
        self.now(Viewer::brief_listed)
    }

    fn brief_text(&self) -> Option<String> {
        self.now(Viewer::brief_text).ok()
    }

    fn activity(&self, activity: zetwerk_mcp::Activity) {
        use zetwerk_app::viewer::activity::Event;
        use zetwerk_mcp::Activity as Told;
        let event = match activity {
            Told::Connected {
                session,
                name,
                version,
            } => Event::Connected {
                session,
                name,
                version,
            },
            Told::Began {
                session,
                tool,
                about,
            } => Event::Began {
                session,
                tool,
                about,
            },
            Told::Ended {
                session,
                tool,
                millis,
                error,
            } => Event::Ended {
                session,
                tool,
                millis,
                error,
            },
            Told::Said {
                session,
                message,
                done,
                total,
            } => Event::Said {
                session,
                message,
                done,
                total,
            },
            Told::Left { session } => Event::Left { session },
        };
        // Handed over, not waited for: the server's thread goes on.
        let task: Task = Box::new(move |viewer| viewer.agent_activity(event));
        let _ = self.handle.send(Request::Do(task));
    }

    fn agents_paused(&self) -> bool {
        zetwerk_app::viewer::activity::AGENTS_PAUSED.load(Ordering::Relaxed)
    }

    fn review_summary(&self) -> Option<Value> {
        self.now(Viewer::review_summary).ok()
    }

    fn wait_for_review(
        &self,
        since: Option<u64>,
        timeout: Duration,
        cancelled: &AtomicBool,
    ) -> Result<Value, String> {
        // Waited for here, on the thread of the agent's call: the window
        // only bumps a count as the author acts.
        let signal = self.now(|viewer| viewer.review_signal())?;
        let since = since.unwrap_or_else(|| signal.now());
        let until = Instant::now() + timeout;
        while signal.now() <= since {
            let left = until.saturating_duration_since(Instant::now());
            if cancelled.load(Ordering::Relaxed) || left.is_zero() {
                return Ok(json!({
                    "cursor": since,
                    "events": [],
                    "requests": [],
                    "brief_changed": false,
                    "timed_out": left.is_zero(),
                }));
            }
            signal.wait_past(since, left.min(Duration::from_millis(500)));
        }
        self.now(move |viewer| viewer.review_news(since))
    }

    fn authorship(&self, lines: Option<(usize, usize)>) -> Result<Value, String> {
        self.now(move |viewer| viewer.authorship(lines))?
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

/// Why what the compiler says is not known: the pages are of another
/// state of the text.
const LOOKING_BACK: &str = "the user is looking through the history of the text: the pages \
                            show another state of it, and what the compiler says of the \
                            text is not known; ask get_diagnostics again in a moment";

/// More errors than this are not listed: one slip (an unclosed bracket)
/// makes hundreds, and the first ones say what it is.
const MAX_DIAGNOSTICS: usize = 20;

/// The first of `problems`, and how many more there are.
fn some_of(problems: &[String]) -> (&[String], usize) {
    let shown = problems.len().min(MAX_DIAGNOSTICS);
    (&problems[..shown], problems.len() - shown)
}

/// A length in points, to the hundredth: more is noise in an answer.
fn round(points: f64) -> f64 {
    (points * 100.0).round() / 100.0
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use zetwerk_app::app::Options;
    use zetwerk_app::headless::{self, OpenError};
    use zetwerk_app::power::PowerMode;
    use zetwerk_typst::FontSet;

    use super::*;

    /// A document in a directory of its own, opened without a window.
    struct Session {
        directory: PathBuf,
        bridge: Bridge,
        handle: Handle,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    fn options(directory: &Path, agent_edits: AgentEdits) -> Options {
        Options {
            main: None,
            path: directory.join("main.typ"),
            font_set: FontSet::Embedded,
            power: PowerMode::default(),
            state_dir: directory.join("state"),
            agent_edits,
            complaint: None,
            keys: zetwerk_app::viewer::Keymap::default(),
            layout: None,
            slides: false,
            ui_scale: None,
            inserts: Vec::new(),
            language: zetwerk_language::Settings::default(),
            templates: None,
            packages: zetwerk_app::viewer::packages::Settings::default(),
            agent: zetwerk_app::viewer::on_call::Settings::default(),
            review: zetwerk_app::viewer::review::Settings::default(),
            talk: zetwerk_app::viewer::Talk::default(),
            report: zetwerk_app::viewer::report::Settings::default(),
            authors: zetwerk_app::viewer::authors::Settings::default(),
            settings: zetwerk_app::settings::Stored::default(),
            changes: zetwerk_app::viewer::ChangeOptions {
                git: false,
                ..zetwerk_app::viewer::ChangeOptions::default()
            },
            comfort: zetwerk_app::viewer::comfort::Settings::default(),
        }
    }

    fn session(text: &str, agent_edits: AgentEdits) -> Session {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "zetwerk-headless-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("main.typ"), text).unwrap();
        let (handle, thread) = headless::spawn(&options(&directory, agent_edits))
            .unwrap_or_else(|err| panic!("{err:?}"));
        Session {
            directory,
            bridge: Bridge::new(handle.clone()),
            handle,
            thread: Some(thread),
        }
    }

    /// Like [`session`], with the spelling of the document checked by
    /// the small dictionaries kept for tests.
    fn checking_session(text: &str) -> Session {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "zetwerk-headless-language-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("main.typ"), text).unwrap();
        let fixtures = concat!(env!("CARGO_MANIFEST_DIR"), "/../zetwerk-language/fixtures");
        let options = Options {
            language: zetwerk_language::Settings {
                spelling: true,
                directories: vec![PathBuf::from(fixtures)],
                ..zetwerk_language::Settings::default()
            },
            ..options(&directory, AgentEdits::Live)
        };
        let (handle, thread) = headless::spawn(&options).unwrap_or_else(|err| panic!("{err:?}"));
        Session {
            directory,
            bridge: Bridge::new(handle.clone()),
            handle,
            thread: Some(thread),
        }
    }

    #[test]
    fn an_agent_is_told_what_is_misspelt_in_each_of_the_three_languages() {
        for (language, text, wrong, meant) in [
            (
                "en",
                "A good word.\n\nHe said helo to $x$.",
                "helo",
                "hello",
            ),
            (
                "de",
                "Das Haus.\n\nDie Strasse ist kurz.",
                "Strasse",
                "Straße",
            ),
            ("nl", "Het huis.\n\nDe fiest is goed.", "fiest", "fiets"),
        ] {
            let text = format!("#set text(lang: \"{language}\")\n{text}\n");
            let session = checking_session(&text);
            let found = session.bridge.check_language().unwrap();
            let issues = found["issues"].as_array().unwrap();
            assert_eq!(issues.len(), 1, "{found}");
            let issue = &issues[0];
            assert_eq!(
                (&issue["line"], &issue["text"]),
                (&json!(4), &json!(wrong)),
                "{found}"
            );
            assert_eq!(issue["kind"], json!("spelling"));
            let suggestions = issue["suggestions"].as_array().unwrap();
            assert!(suggestions.contains(&json!(meant)), "{found}");
            assert_eq!(found["revision"], json!(0));
            assert!(
                found["dictionary"].as_str().unwrap().starts_with(language),
                "{found}"
            );
        }
        // Where the user has it switched off, an agent is told so.
        let off = session("A wrnog word.\n", AgentEdits::Live);
        let refused = off.bridge.check_language().unwrap_err();
        assert!(refused.contains("switched off"), "{refused}");
    }

    impl Drop for Session {
        fn drop(&mut self) {
            self.handle.send(Request::Quit);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    fn replace(old: &str, new: &str, expected_revision: Option<u64>) -> Change {
        Change {
            what: ChangeKind::Replace {
                old: old.to_owned(),
                new: new.to_owned(),
                all: false,
            },
            expected_revision,
            propose: false,
            reason: None,
        }
    }

    const TEXT: &str = "#set page(width: 200pt, height: 120pt)\n\
                        = First\n\
                        The quick brown fox.\n\
                        #pagebreak()\n\
                        = Second <two>\n\
                        The lazy dog.\n";

    #[test]
    fn a_document_is_read_changed_and_written_without_a_window() {
        let session = session(TEXT, AgentEdits::Live);
        let bridge = &session.bridge;
        let state = bridge.state().unwrap();
        assert_eq!(
            (state.file.as_str(), state.revision, state.modified),
            ("main.typ", 0, false)
        );
        assert_eq!(state.agent_edits, "live");
        let read = bridge.read_source(Some(3), Some(3)).unwrap();
        assert_eq!(
            read,
            "revision 0, lines 3 to 3 of 6\n3\tThe quick brown fox.\n"
        );

        let changed = bridge
            .change(replace("quick brown", "slow red", Some(0)))
            .unwrap();
        assert_eq!(changed["applied"], true);
        assert_eq!(changed["revision"], 1);
        assert_eq!(changed["diagnostics"], json!([]));
        assert_eq!(changed["pages"], 2);
        // A change for text that is no more; and one that breaks it.
        let refused = bridge.change(replace("fox", "cat", Some(0)));
        assert!(refused.unwrap_err().contains("revision 1"));
        let broken = bridge
            .change(replace("= First", "= First #nope(", None))
            .unwrap();
        assert_eq!(broken["applied"], true);
        let diagnostics = broken["diagnostics"].as_array().unwrap();
        assert!(diagnostics[0].as_str().unwrap().contains("main.typ:2:"));
        assert!(bridge.locate(Place::Source { line: 3, column: 1 }).is_err());
        bridge.change(replace(" #nope(", "", None)).unwrap();
        assert_eq!(bridge.diagnostics().unwrap()["diagnostics"], json!([]));

        // Where things are.
        let outline = bridge.outline().unwrap();
        assert_eq!(
            outline["headings"][1],
            json!({ "level": 1, "text": "Second", "line": 5, "page": 2 })
        );
        assert_eq!(outline["labels"], json!([{ "name": "two", "line": 5 }]));
        let found = bridge.search("the", true, 10).unwrap();
        assert_eq!(found["total"], 2);
        let place = bridge.locate(Place::Source { line: 6, column: 5 }).unwrap();
        assert_eq!(place["page"], 2);
        let (x, y) = (place["x"].as_f64().unwrap(), place["y"].as_f64().unwrap());
        let back = bridge
            .locate(Place::Page {
                page: 2,
                x: x + 1.0,
                y: y + 2.0,
            })
            .unwrap();
        assert_eq!(back, json!({ "file": "main.typ", "line": 6, "column": 5 }));
        let png = bridge.render_page(2, 400).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        assert!(bridge.render_page(3, 400).is_err());
        assert!(bridge.screenshot().is_err(), "there is no window");

        // Onto the disk: the text only when asked, the PDF beside it.
        let file = session.directory.join("main.typ");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), TEXT);
        assert!(bridge.state().unwrap().modified);
        bridge.save().unwrap();
        let written = std::fs::read_to_string(&file).unwrap();
        assert!(written.contains("The slow red fox."));
        assert!(!bridge.state().unwrap().modified);
        let exported = bridge.export_pdf(None, None).unwrap();
        let pdf = std::fs::read(session.directory.join("main.pdf")).unwrap();
        assert!(pdf.starts_with(b"%PDF"));
        assert_eq!(exported["pages"], 2);
        assert_eq!(
            (&exported["of"], &exported["replaced"]),
            (&json!(2), &json!(false))
        );
        assert!(bridge.export_pdf(Some("notes.txt"), None).is_err());
        bridge.export_pdf(Some("copy.pdf"), None).unwrap();
        assert!(session.directory.join("copy.pdf").is_file());
    }

    #[test]
    fn an_agent_is_not_told_the_errors_of_a_state_the_user_looks_back_at() {
        let session = session("Broken #nope(\n", AgentEdits::Live);
        let bridge = &session.bridge;
        bridge.send_keys("ccFixed text.<Esc>").unwrap();
        assert_eq!(bridge.diagnostics().unwrap()["diagnostics"], json!([]));
        // The user looks at the text as it was, which did not compile.
        bridge.send_keys("gh<Down>").unwrap();
        let changed = bridge.change(replace("Fixed", "Fine", None)).unwrap();
        assert_eq!(changed["applied"], true);
        assert!(
            !changed.to_string().contains("unclosed"),
            "the old state's errors are not the change's: {changed}"
        );
        assert!(bridge.diagnostics().is_err());
        let headings = bridge.outline().unwrap();
        assert_eq!(headings["headings"], json!([]));
        // Back at the text: what is said is of it.
        bridge.send_keys("<Esc>").unwrap();
        assert_eq!(bridge.diagnostics().unwrap()["diagnostics"], json!([]));
    }

    #[test]
    fn arguments_out_of_range_are_refused_and_the_window_lives_on() {
        let session = session(TEXT, AgentEdits::Live);
        let bridge = &session.bridge;
        assert!(bridge.read_source(Some(5), Some(2)).is_err());
        assert!(bridge.read_source(Some(99), None).is_err());
        assert!(bridge.read_source(Some(0), Some(0)).is_err());
        assert!(bridge.change(replace("", "x", None)).is_err());
        assert!(bridge.change(replace("lazy", "lazy", None)).is_err());
        assert!(bridge.change(replace("nowhere", "x", Some(99))).is_err());
        assert!(bridge.search("", false, 10).is_err());
        for place in [
            Place::Source { line: 0, column: 0 },
            Place::Source {
                line: 99,
                column: 1,
            },
            Place::Page {
                page: 0,
                x: 1.0,
                y: 1.0,
            },
            Place::Page {
                page: 9,
                x: -5.0,
                y: f64::MAX,
            },
        ] {
            assert!(bridge.locate(place).is_err());
        }
        assert!(bridge.render_page(0, 100).is_err());
        let moved = Change {
            what: ChangeKind::MoveSection {
                line: 99,
                direction: Direction::Up,
            },
            expected_revision: None,
            propose: false,
            reason: None,
        };
        assert!(bridge.change(moved).is_err());
        let note = NewNote {
            text: "x".to_owned(),
            anchor: NoteAnchor::Line(0),
            expected_revision: None,
        };
        assert!(bridge.add_note(note).is_err());
        let image = ImageExport {
            format: ImageFormat::Png,
            of: ImageOf::FigureAtLine(0),
            dpi: Some(0),
            path: None,
        };
        assert!(bridge.export_image(image).is_err());
        assert!(bridge.withdraw_proposal(u64::MAX).is_err());
        assert!(bridge.resolve_note(u64::MAX).is_err());
        // Still there, and the text as it was.
        assert_eq!(bridge.state().unwrap().revision, 0);
        assert!(bridge.read_source(None, None).unwrap().contains("lazy dog"));
    }

    #[test]
    fn an_agent_exports_nowhere_but_in_the_documents_directory() {
        let session = session(TEXT, AgentEdits::Live);
        let (bridge, directory) = (&session.bridge, &session.directory);
        let outside = directory.with_file_name(format!(
            "{}-outside",
            directory.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(&outside).unwrap();
        let name = outside.file_name().unwrap().to_string_lossy().into_owned();
        let up = format!("../{name}/up.pdf");
        let absolute = outside.join("absolute.pdf");
        let refused = bridge.export_pdf(Some(&up), None).unwrap_err();
        assert!(
            refused.contains("not in the document's directory"),
            "{refused}"
        );
        assert!(
            bridge
                .export_pdf(Some(absolute.to_str().unwrap()), None)
                .is_err()
        );
        let image = ImageExport {
            format: ImageFormat::Svg,
            of: ImageOf::Pages(None),
            dpi: None,
            path: Some(format!("../{name}/up.svg")),
        };
        assert!(bridge.export_image(image).is_err());
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&outside);
        // Below it is in it.
        std::fs::create_dir(directory.join("out")).unwrap();
        bridge.export_pdf(Some("./out/paper.pdf"), None).unwrap();
        assert!(directory.join("out/paper.pdf").is_file());
    }

    #[test]
    fn proposals_stay_proposals_without_a_window() {
        let session = session(TEXT, AgentEdits::Propose);
        let bridge = &session.bridge;
        let proposed = bridge.change(replace("lazy", "busy", None)).unwrap();
        assert_eq!(proposed["applied"], false);
        let id = proposed["proposal"].as_u64().unwrap();
        let listed = bridge.proposals().unwrap();
        assert_eq!(
            listed["proposals"],
            json!([{
                "id": id, "line": 6, "old_str": "lazy", "new_str": "busy", "stale": false,
                "suggestion": 1,
            }])
        );
        assert_eq!(bridge.state().unwrap().proposals, 1);
        assert_eq!(
            bridge.read_source(Some(6), None).unwrap(),
            "revision 0, lines 6 to 6 of 6\n6\tThe lazy dog.\n"
        );
        bridge.withdraw_proposal(id).unwrap();
        assert_eq!(bridge.proposals().unwrap()["proposals"], json!([]));
    }

    #[test]
    fn notes_change_no_text_where_agents_only_propose() {
        let session = session(TEXT, AgentEdits::Propose);
        let bridge = &session.bridge;
        bridge.send_keys(":6<CR>gqShorter?<CR>").unwrap();
        let listed = bridge.notes(false).unwrap();
        let revision = listed["revision"].as_u64().unwrap();
        let id = listed["notes"][0]["id"].as_u64().unwrap();
        let answer = |change: Option<(&str, &str)>, expected| NoteAnswer {
            id,
            text: "Yes.".to_owned(),
            change: change.map(|(old, new)| (old.to_owned(), new.to_owned())),
            expected_revision: expected,
        };
        // For a text there no longer is: refused.
        let late = answer(Some(("lazy", "idle")), Some(revision + 1));
        assert!(bridge.answer_note(late).is_err());
        let note = NewNote {
            text: "x".to_owned(),
            anchor: NoteAnchor::Text("lazy".to_owned()),
            expected_revision: Some(revision + 1),
        };
        assert!(bridge.add_note(note).is_err());
        assert_eq!(
            bridge.notes(false).unwrap()["notes"][0]["answers"],
            json!([])
        );

        let answered = bridge
            .answer_note(answer(Some(("lazy", "idle")), Some(revision)))
            .unwrap();
        let proposal = answered["proposal"].as_u64().unwrap();
        assert_eq!(bridge.state().unwrap().revision, revision, "nothing made");
        assert!(bridge.read_source(None, None).unwrap().contains("lazy dog"));
        // Its text changed by the user, the proposal is stale: it can only
        // be dropped.
        bridge.send_keys("0wcwsleepy<Esc>").unwrap();
        let proposals = bridge.proposals().unwrap();
        assert_eq!(proposals["proposals"][0]["id"], json!(proposal));
        assert_eq!(proposals["proposals"][0]["stale"], json!(true));
        assert!(
            bridge
                .read_source(None, None)
                .unwrap()
                .contains("sleepy dog")
        );
    }

    #[test]
    fn notes_are_asked_and_answered_without_a_window() {
        let session = session(TEXT, AgentEdits::Live);
        let bridge = &session.bridge;
        // The user asks about the third line.
        bridge.send_keys(":3<CR>gqIs this right?<CR>").unwrap();
        let listed = bridge.notes(false).unwrap();
        let [question] = listed["notes"].as_array().unwrap().as_slice() else {
            panic!("one note: {listed}");
        };
        assert_eq!(
            (
                &question["line"],
                &question["text"],
                &question["note"],
                &question["to_agent"],
                &question["by"]
            ),
            (
                &json!(3),
                &json!("The quick brown fox."),
                &json!("Is this right?"),
                &json!(true),
                &json!("user")
            )
        );
        let id = question["id"].as_u64().unwrap();

        // Answered with a change, which waits for the user, with why.
        let answered = bridge
            .answer_note(NoteAnswer {
                id,
                text: "Foxes are not that quick.".to_owned(),
                change: Some(("quick".to_owned(), "swift".to_owned())),
                expected_revision: None,
            })
            .unwrap();
        let proposal = answered["proposal"].as_u64().unwrap();
        let proposals = bridge.proposals().unwrap();
        assert_eq!(proposals["proposals"][0]["note"], json!(id));
        assert_eq!(
            proposals["proposals"][0]["reason"],
            json!("Foxes are not that quick.")
        );
        assert_eq!(bridge.state().unwrap().proposals, 1);
        let listed = bridge.notes(false).unwrap();
        assert_eq!(listed["notes"][0]["proposals"], json!([proposal]));
        assert_eq!(
            listed["notes"][0]["answers"],
            json!([{ "by": "agent", "text": "Foxes are not that quick." }])
        );
        assert!(
            bridge
                .read_source(Some(3), Some(3))
                .unwrap()
                .contains("quick")
        );

        // A reason given with a proposal of the agent's own.
        let mut change = replace("lazy", "busy", None);
        change.propose = true;
        change.reason = Some("livelier".to_owned());
        let other = bridge.change(change).unwrap()["proposal"].as_u64().unwrap();
        let proposals = bridge.proposals().unwrap();
        let reasons: Vec<&Value> = proposals["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|listed| &listed["reason"])
            .collect();
        assert_eq!(
            reasons,
            [&json!("Foxes are not that quick."), &json!("livelier")]
        );
        bridge.withdraw_proposal(other).unwrap();

        // Notes of the agent's own; resolved, they are listed on request.
        let left = bridge
            .add_note(NewNote {
                text: "A source?".to_owned(),
                anchor: NoteAnchor::Text("lazy dog".to_owned()),
                expected_revision: None,
            })
            .unwrap();
        assert_eq!(left["line"], json!(6));
        let on_line = NewNote {
            text: "x".to_owned(),
            anchor: NoteAnchor::Line(40),
            expected_revision: None,
        };
        assert!(bridge.add_note(on_line).is_err());
        bridge.resolve_note(id).unwrap();
        assert!(bridge.resolve_note(99).is_err());
        let open = bridge.notes(false).unwrap();
        assert_eq!(open["notes"].as_array().unwrap().len(), 1);
        let all = bridge.notes(true).unwrap();
        assert_eq!(all["notes"].as_array().unwrap().len(), 2);
        // None of it is in the text.
        assert!(!bridge.read_source(None, None).unwrap().contains("source?"));
    }

    /// Three small pages, the second of which is a figure.
    const FIGURES: &str = "#set page(width: 200pt, height: 120pt, margin: 10pt)\n\
                           First page.\n\
                           #pagebreak()\n\
                           #figure(\n  rect(width: 80pt, height: 40pt),\n  caption: [A box],\n) <box>\n\
                           #pagebreak()\n\
                           Third page.\n";

    fn size_of(png: &Path) -> (u32, u32) {
        let pixmap = tiny_skia::Pixmap::decode_png(&std::fs::read(png).unwrap()).unwrap();
        (pixmap.width(), pixmap.height())
    }

    #[test]
    fn pages_and_figures_are_exported_without_a_window() {
        let session = session(FIGURES, AgentEdits::Live);
        let (bridge, directory) = (&session.bridge, &session.directory);
        let image = |format, of, dpi, path: Option<&str>| {
            bridge.export_image(ImageExport {
                format,
                of,
                dpi,
                path: path.map(str::to_owned),
            })
        };
        let pages = |pages: &str| ImageOf::Pages(Some(pages.to_owned()));

        // Some pages as a PDF.
        let part = bridge.export_pdf(Some("part.pdf"), Some("2-3")).unwrap();
        assert_eq!((&part["pages"], &part["of"]), (&json!(2), &json!(3)));
        assert!(directory.join("part.pdf").is_file());
        let missing = bridge.export_pdf(None, Some("4")).unwrap_err();
        assert!(missing.contains("page 4 does not exist"), "{missing}");
        assert!(bridge.export_pdf(None, Some("two")).is_err());
        assert!(!directory.join("main.pdf").exists());

        // Pages as images, each a file of its own.
        let images = image(ImageFormat::Png, pages("1,3"), Some(72), None).unwrap();
        let written: Vec<&str> = images["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file["path"].as_str().unwrap())
            .collect();
        let expected = [directory.join("main-1.png"), directory.join("main-3.png")];
        assert_eq!(
            written,
            expected.each_ref().map(|path| path.to_str().unwrap())
        );
        assert_eq!((&images["pages"], &images["of"]), (&json!(2), &json!(3)));
        assert_eq!(size_of(&expected[1]), (200, 120));
        let again = image(ImageFormat::Png, pages("3"), None, None).unwrap();
        assert_eq!(again["files"][0]["replaced"], true);
        assert_eq!(size_of(&expected[1]), (400, 240));
        let all = image(
            ImageFormat::Svg,
            ImageOf::Pages(None),
            None,
            Some("all.svg"),
        )
        .unwrap();
        assert_eq!(all["files"].as_array().unwrap().len(), 3);
        assert!(directory.join("all-2.svg").is_file());

        // The figure a line is part of, alone.
        let figure = image(ImageFormat::Png, ImageOf::FigureAtLine(6), Some(72), None).unwrap();
        assert_eq!(
            figure["files"][0]["path"],
            directory.join("main-box.png").to_str().unwrap()
        );
        let (width, height) = size_of(&directory.join("main-box.png"));
        assert!(
            (88..110).contains(&width) && (55..80).contains(&height),
            "{width}×{height}"
        );
        let nowhere = image(ImageFormat::Svg, ImageOf::FigureAtLine(2), None, None).unwrap_err();
        assert_eq!(nowhere, "line 2 is in no figure");

        // What cannot be: a resolution of an SVG, a name of another kind.
        assert!(image(ImageFormat::Svg, pages("1"), Some(300), None).is_err());
        assert!(image(ImageFormat::Png, pages("1"), None, Some("page.jpg")).is_err());
        assert!(image(ImageFormat::Png, pages("0"), None, None).is_err());

        // An export is of the text as it is, changed a moment ago or not.
        bridge
            .change(replace("#pagebreak()\nThird page.\n", "", None))
            .unwrap();
        let fewer = bridge.export_pdf(None, None).unwrap();
        assert_eq!((&fewer["pages"], &fewer["of"]), (&json!(2), &json!(2)));
        // And with nobody to keep waiting, keys export on the spot.
        bridge.send_keys(":png 2 keys.png<CR>").unwrap();
        bridge.state().unwrap();
        assert_eq!(size_of(&directory.join("keys.png")), (400, 240));
    }

    #[test]
    fn a_chapter_is_opened_as_part_of_its_document() {
        let session = session("= Book\n#include \"part.typ\"\n", AgentEdits::Live);
        let part = session.directory.join("part.typ");
        std::fs::write(&part, "== Part\nPart text.\n").unwrap();
        // The main file is found from the part.
        assert_eq!(
            zetwerk_typst::find_main(&part).canonicalize().unwrap(),
            session.directory.join("main.typ").canonicalize().unwrap()
        );
        let mut options = options(&session.directory, AgentEdits::Live);
        options.main = Some(std::mem::replace(&mut options.path, part));
        // The main file is open in the first session: a second one may
        // have the part.
        let (handle, thread) = headless::spawn(&options).unwrap_or_else(|err| panic!("{err:?}"));
        let bridge = Bridge::new(handle.clone());
        assert_eq!(bridge.state().unwrap().file, "part.typ");
        assert_eq!(bridge.read_source(None, None).unwrap().lines().count(), 3);
        let outline = bridge.outline().unwrap();
        assert_eq!(outline["headings"][0]["text"], "Part");
        // The whole document is what is compiled and shown.
        let changed = bridge
            .change(replace("Part text.", "Other text.", None))
            .unwrap();
        assert_eq!(changed["diagnostics"], json!([]));
        let place = bridge.locate(Place::Source { line: 2, column: 1 }).unwrap();
        assert_eq!(place["page"], 1);
        let (x, y) = (place["x"].as_f64().unwrap(), place["y"].as_f64().unwrap());
        let back = bridge
            .locate(Place::Page {
                page: 1,
                x: x + 1.0,
                y: y + 2.0,
            })
            .unwrap();
        assert_eq!(back, json!({ "file": "part.typ", "line": 2, "column": 1 }));
        handle.send(Request::Quit);
        let _ = thread.join();
    }

    #[test]
    fn a_document_is_open_once() {
        let session = session(TEXT, AgentEdits::Live);
        let again = headless::spawn(&options(&session.directory, AgentEdits::Live));
        assert!(matches!(again, Err(OpenError::AlreadyOpen)));
    }

    #[test]
    fn what_was_changed_and_not_written_comes_back() {
        let session = session(TEXT, AgentEdits::Live);
        session
            .bridge
            .change(replace("lazy", "busy", None))
            .unwrap();
        let directory = session.directory.clone();
        // Leave without deleting the directory: only the session ends.
        let Session { handle, thread, .. } = &session;
        handle.send(Request::Quit);
        let _ = thread.as_ref().map(|thread| thread.thread().id());
        std::mem::forget(session);
        // The journal lock goes with the thread; wait for it.
        let reopened = (0..200).find_map(|_| {
            std::thread::sleep(std::time::Duration::from_millis(10));
            headless::spawn(&options(&directory, AgentEdits::Live)).ok()
        });
        let (handle, thread) = reopened.unwrap();
        let bridge = Bridge::new(handle.clone());
        assert!(
            bridge
                .read_source(Some(6), None)
                .unwrap()
                .contains("The busy dog.")
        );
        assert!(bridge.state().unwrap().modified);
        handle.send(Request::Quit);
        let _ = thread.join();
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_figures_of_a_document_are_listed() {
        let text = "= Results\n\
                    #figure(\n  image(\"plot.svg\"),\n  caption: [A plot],\n) <fig:plot>\n\n\
                    #table(columns: 1, [a])\n";
        let session = session(text, AgentEdits::Live);
        let listed = session.bridge.figures().unwrap();
        assert_eq!(
            listed,
            json!({ "figures": [
                {
                    "kind": "image",
                    "in_figure": true,
                    "numbered": "Figure 1",
                    "caption": "A plot",
                    "label": "fig:plot",
                    "line": 2,
                    "image": "plot.svg",
                },
                {
                    "kind": "table",
                    "in_figure": false,
                    "numbered": null,
                    "caption": null,
                    "label": null,
                    "line": 7,
                    "image": null,
                },
            ] })
        );
    }

    #[test]
    fn every_proposal_of_a_change_is_told_and_taken_back_at_once() {
        let session = session("A cat, a cat, a dog.\n", AgentEdits::Propose);
        let bridge = &session.bridge;
        let change = Change {
            what: ChangeKind::Replace {
                old: "cat".to_owned(),
                new: "cow".to_owned(),
                all: true,
            },
            expected_revision: None,
            propose: false,
            reason: None,
        };
        let proposed = bridge.change(change).unwrap();
        let ids: Vec<u64> = proposed["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| id.as_u64().unwrap())
            .collect();
        assert_eq!(ids.len(), 2, "{proposed}");
        // The field there was, for those who read it.
        assert_eq!(proposed["proposal"], json!(ids[1]));
        assert_eq!(bridge.state().unwrap().proposals, 2);
        // One that is not there: none goes.
        assert!(bridge.withdraw_proposals(&[ids[0], 999]).is_err());
        assert_eq!(bridge.state().unwrap().proposals, 2);
        let withdrawn = bridge.withdraw_proposals(&ids).unwrap();
        assert_eq!(withdrawn["waiting"], json!(0));
        assert_eq!(bridge.proposals().unwrap()["proposals"], json!([]));
    }

    #[test]
    fn an_agent_is_told_who_saves() {
        let session = session(TEXT, AgentEdits::Live);
        let state = session.bridge.state().unwrap();
        assert_eq!((state.window, state.saves.as_str()), (true, "user"));
        for (save, saves) in [(true, "agent"), (false, "nobody")] {
            let alone = Bridge::new(session.handle.clone()).offering(false, save);
            let state = alone.state().unwrap();
            assert_eq!((state.window, state.saves.as_str()), (false, saves));
        }
    }

    #[test]
    fn an_agent_adds_sources_only_in_the_documents_directory() {
        let bibtex = "@book{knuth, author = {Knuth, Donald}, title = {The TeXbook}, year = {1984}}";
        let outside = session("Words.\n#bibliography(\"../x.bib\")\n", AgentEdits::Live);
        let refused = outside.bridge.add_source(bibtex, false).unwrap_err();
        assert!(
            refused.contains("outside the document's directory"),
            "{refused}"
        );
        assert!(!outside.directory.with_file_name("x.bib").exists());
        // Below it is in it.
        let below = session(
            "Words.\n#bibliography(\"./sub/../refs.bib\")\n",
            AgentEdits::Live,
        );
        std::fs::create_dir(below.directory.join("sub")).unwrap();
        let added = below.bridge.add_source(bibtex, false).unwrap();
        assert_eq!(added["keys"], json!(["knuth"]));
        assert!(below.directory.join("refs.bib").is_file());
    }

    #[test]
    fn a_request_for_a_document_the_window_no_longer_shows_is_refused() {
        let session = session(TEXT, AgentEdits::Live);
        let bridge = &session.bridge;
        let shown = bridge.shown().unwrap();
        assert!(bridge.compiled_for(shown, |_| ()).is_ok());
        // Asked while another was shown: not answered from this one.
        let other = (Some(PathBuf::from("/elsewhere/other.typ")), None);
        let refused = bridge.compiled_for(other, |_| ()).unwrap_err();
        assert!(refused.contains("another document"), "{refused}");
    }

    #[test]
    fn an_agent_is_told_of_questions_and_notes_that_wait() {
        let session = session(TEXT, AgentEdits::Live);
        let bridge = &session.bridge;
        let state = bridge.state().unwrap();
        assert_eq!((state.open_questions, state.unanswered_notes), (0, 0));
        bridge.send_keys(":3<CR>gqIs this right?<CR>").unwrap();
        bridge.send_keys(":6<CR>gnLater.<CR>").unwrap();
        let state = bridge.state().unwrap();
        assert_eq!((state.open_questions, state.unanswered_notes), (1, 1));
        let id = bridge.notes(false).unwrap()["notes"][0]["id"]
            .as_u64()
            .unwrap();
        bridge
            .answer_note(NoteAnswer {
                id,
                text: "Yes.".to_owned(),
                change: None,
                expected_revision: None,
            })
            .unwrap();
        assert_eq!(bridge.state().unwrap().open_questions, 0);
    }

    /// The bytes of a PDF of `count` pages of 300 by 200 points.
    fn landscape_pdf(count: usize) -> Vec<u8> {
        let env = zetwerk_typst::Environment::new(FontSet::Embedded);
        let text = format!(
            "#set page(width: 300pt, height: 200pt)\n{}",
            vec!["Slide"; count].join("\n#pagebreak()\n")
        );
        let project =
            zetwerk_typst::Project::in_memory(env, PathBuf::from("/"), "main.typ", text).unwrap();
        let world = project.snapshot();
        let document = zetwerk_typst::compile(&world).document.unwrap();
        zetwerk_typst::export_pdf(&document, &world).unwrap()
    }

    #[test]
    fn an_agent_puts_in_pages_of_a_file_beside_the_document_only() {
        let live = session(TEXT, AgentEdits::Live);
        std::fs::write(live.directory.join("slides.pdf"), landscape_pdf(3)).unwrap();
        let bridge = &live.bridge;
        let insert = |file: &str, pages: Option<&str>, place: &str, expected: Option<u64>| Change {
            what: ChangeKind::InsertPages(zetwerk_mcp::PagesWanted {
                file: file.to_owned(),
                pages: pages.map(str::to_owned),
                place: Some(place.to_owned()),
                fit: None,
                counted: None,
            }),
            expected_revision: expected,
            propose: false,
            reason: None,
        };
        let put = bridge
            .change(insert("slides.pdf", Some("2-3"), "after:3", Some(0)))
            .unwrap();
        assert_eq!(put["applied"], json!(true), "{put}");
        assert_eq!(put["pages"], json!(4), "{put}");
        assert_eq!(put["diagnostics"], json!([]), "{put}");
        let outline = bridge.outline().unwrap();
        let pages = outline["inserted_pages"].as_array().unwrap();
        assert_eq!(pages.len(), 2, "{outline}");
        assert_eq!(
            pages[0],
            json!({ "line": 5, "shows": "pdf", "file": "slides.pdf", "page": 2,
                    "fit": false, "counted": true })
        );
        // They are pages, not figures.
        assert_eq!(bridge.figures().unwrap(), json!({ "figures": [] }));
        // Not from outside the directory, not a page the file lacks,
        // not a file that is none.
        for (file, pages) in [
            ("../slides.pdf", None),
            ("/etc/passwd", None),
            ("slides.pdf", Some("4")),
            ("nowhere.pdf", None),
            ("main.typ", None),
        ] {
            let refused = bridge.change(insert(file, pages, "end", None));
            assert!(refused.is_err(), "{file}: {refused:?}");
        }
        assert!(
            bridge
                .change(insert("slides.pdf", None, "sideways", None))
                .is_err()
        );
        // Where the user reviews changes, it is a proposal.
        let proposing = session(TEXT, AgentEdits::Propose);
        std::fs::write(proposing.directory.join("slides.pdf"), landscape_pdf(1)).unwrap();
        let proposed = proposing
            .bridge
            .change(insert("slides.pdf", None, "end", None))
            .unwrap();
        assert_eq!(proposed["applied"], json!(false), "{proposed}");
        let read = proposing.bridge.read_source(None, None).unwrap();
        assert!(!read.contains("#page("), "{read}");
    }

    /// Keys the author types, in the window the session has none of.
    fn author(bridge: &Bridge, typed: &str) {
        let typed = typed.to_owned();
        bridge
            .now(move |viewer| {
                for key in keys::parse(&typed).unwrap() {
                    viewer.key(key);
                }
            })
            .unwrap();
    }

    fn suggestion(kind: &str, old: &str, new: Option<&str>, category: &str) -> NewSuggestion {
        NewSuggestion {
            kind: kind.to_owned(),
            old_str: Some(old.to_owned()),
            new_str: new.map(str::to_owned),
            reason: format!("Better for {category}."),
            category: Some(category.to_owned()),
            ..NewSuggestion::default()
        }
    }

    #[test]
    #[allow(clippy::too_many_lines, reason = "one round of a review, step by step")]
    fn a_whole_review_round_is_suggested_decided_answered_and_recorded() {
        let text = "= Abstract\nWe study the colour of foxes. Our results are new.\n\n\
                    = Methods\nThe quick brown fox jumps over the lazy dog. We recieve data \
                    and analyse it carefuly.\n\nResults are good. This is very very important.\n";
        let session = session(text, AgentEdits::Propose);
        let bridge = &session.bridge;
        let claude = Caller {
            name: "claude-code".to_owned(),
            session: "s7".to_owned(),
            ..Caller::default()
        };
        let batch = vec![
            NewSuggestion {
                confidence: Some("high".to_owned()),
                ..suggestion("replace", "recieve", Some("receive"), "spelling")
            },
            suggestion("replace", "carefuly", Some("carefully"), "spelling"),
            suggestion("replace", "very very", Some("very"), "concision"),
            suggestion(
                "replace",
                "Results are good.",
                Some("The results are good."),
                "grammar",
            ),
            suggestion("replace", "quick brown", Some("fast brown"), "tone"),
            suggestion("replace", "lazy", Some("sleepy"), "tone"),
            NewSuggestion {
                kind: "question".to_owned(),
                old_str: Some("analyse".to_owned()),
                reason: "Which method?".to_owned(),
                ..NewSuggestion::default()
            },
            suggestion("replace", "colour", Some("color"), "spelling"),
            suggestion(
                "replace",
                "Our results are new.",
                Some("Our results are novel."),
                "tone",
            ),
            NewSuggestion {
                kind: "comment".to_owned(),
                old_str: None,
                section: Some("Methods".to_owned()),
                reason: "Too short.".to_owned(),
                ..NewSuggestion::default()
            },
        ];
        let suggest = Suggest {
            suggestions: batch,
            expected_revision: Some(0),
            ..Suggest::default()
        };
        let answer = bridge.suggest(&claude, suggest).unwrap();
        assert_eq!(answer["made"], 10, "{answer}");
        let id = |index: usize| answer["results"][index]["id"].as_u64().unwrap();
        // Four rejected, one of them made an instruction.
        author(
            bridge,
            &format!(
                ":review go {}<CR>:reject British spelling stays<CR>g!<CR>",
                id(7)
            ),
        );
        author(bridge, &format!(":review go {}<CR>gr", id(8)));
        author(bridge, &format!(":review go {}<CR>gr", id(9)));
        author(
            bridge,
            &format!(":review go {}<CR>:reject keep the emphasis<CR>", id(2)),
        );
        // Three accepted, one rewritten, two answered.
        author(bridge, ":review accept spelling<CR>");
        author(bridge, &format!(":review go {}<CR>ga", id(3)));
        author(
            bridge,
            &format!(":review go {}<CR>gA<BS><BS><BS><BS><BS>grey<Esc>", id(4)),
        );
        author(bridge, &format!(":review go {}<CR>gqtoo cute<CR>", id(5)));
        author(
            bridge,
            &format!(":review go {}<CR>gqthe usual one<CR>", id(6)),
        );
        let now = bridge.read_source(None, None).unwrap();
        assert!(now.contains("The fast grey fox"), "{now}");
        assert!(
            now.contains("We receive data and analyse it carefully."),
            "{now}"
        );
        assert!(now.contains("The results are good."), "{now}");
        assert!(now.contains("the colour of"), "{now}");

        // The agent learns what became of each.
        let listed = bridge.suggestions(&[], Some("claude-code"), None).unwrap();
        let state = |index: usize| {
            let all = listed["suggestions"].as_array().unwrap();
            let found = all
                .iter()
                .find(|s| s["id"].as_u64() == Some(id(index)))
                .unwrap();
            found["state"].as_str().unwrap().to_owned()
        };
        let states: Vec<String> = (0..10).map(state).collect();
        assert_eq!(
            states,
            [
                "accepted",
                "accepted",
                "rejected",
                "accepted",
                "rewritten",
                "waiting",
                "waiting",
                "rejected",
                "rejected",
                "rejected"
            ],
            "{listed}"
        );
        assert_eq!(
            listed["brief"][0]["text"], "British spelling stays",
            "{listed}"
        );
        let rewritten = &listed["suggestions"].as_array().unwrap()[4];
        assert_eq!(rewritten["outcome"]["after"], "fast grey", "{listed}");
        // Told at once what happened since the start; and nothing more
        // after the cursor it was given.
        let news = bridge
            .wait_for_review(Some(0), Duration::from_secs(5), &AtomicBool::new(false))
            .unwrap();
        assert!(news.to_string().contains("too cute"), "{news}");
        let cursor = news["cursor"].as_u64().unwrap();
        let quiet = bridge
            .wait_for_review(
                Some(cursor),
                Duration::from_millis(50),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(quiet["timed_out"], true, "{quiet}");
        // It answers the author with a new version; and is refused when
        // it suggests what the author refused for good.
        let revised = bridge
            .revise_suggestion(
                &claude,
                Revision {
                    id: id(5),
                    new_str: Some("drowsy".to_owned()),
                    message: Some("Plainer, then.".to_owned()),
                    ..Revision::default()
                },
            )
            .unwrap();
        assert_eq!(revised["state"], "answered", "{revised}");
        let again = bridge
            .suggest(
                &claude,
                Suggest {
                    suggestions: vec![suggestion("replace", "colour", Some("color"), "spelling")],
                    ..Suggest::default()
                },
            )
            .unwrap();
        assert_eq!(again["results"][0]["status"], "contradicts", "{again}");
        assert!(
            bridge
                .brief_text()
                .unwrap()
                .contains("British spelling stays")
        );
        assert_eq!(bridge.review_summary().unwrap()["open_suggestions"], 2);
        // The record says all of it.
        author(bridge, ":review record<CR>");
        let record = std::fs::read_to_string(session.directory.join("main.review.md")).unwrap();
        for said in [
            "rejected it: `British spelling stays`",
            "made it a standing instruction: `British spelling stays`",
            "accepted it: `recieve` became `receive`",
            "rewrote it: `quick brown` became `fast grey`",
            "said: `too cute`",
            "revised it to `drowsy`",
            "by claude-code (session s7)",
            "1. British spelling stays",
        ] {
            assert!(record.contains(said), "{said:?} not in\n{record}");
        }
        // Withdrawn by the agent: decided ones are not.
        assert!(bridge.withdraw_suggestions(&claude, &[id(0)]).is_err());
        assert!(bridge.withdraw_suggestions(&claude, &[id(5)]).is_ok());
    }
}
