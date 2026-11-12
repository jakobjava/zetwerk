//! Lets the MCP server talk to the window.

use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde_json::{Value, json};
use zetwerk_app::app::{Handle, Request, Screenshot, Task};
use zetwerk_app::editor::Mode as EditMode;
use zetwerk_app::keys;
use zetwerk_app::viewer::agent::{AgentEdits, Outcome};
use zetwerk_app::viewer::{Mode, Panes, Viewer};
use zetwerk_mcp::{Change, ChangeKind, Editor, EditorState, Place, Selection};

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

    /// Does `task` with the viewer and gives what it gave.
    fn now<T: Send + 'static>(
        &self,
        task: impl FnOnce(&mut Viewer) -> T + Send + 'static,
    ) -> Result<T, String> {
        self.ask(
            |reply| {
                let task: Task = Box::new(move |viewer| {
                    let _ = reply.send(task(viewer));
                });
                Request::Do(task)
            },
            PROMPT,
        )
    }

    /// Like [`Bridge::now`], once the text as it is has been compiled.
    fn compiled<T: Send + 'static>(
        &self,
        task: impl FnOnce(&mut Viewer) -> T + Send + 'static,
    ) -> Result<T, String> {
        self.ask(
            |reply| {
                let task: Task = Box::new(move |viewer| {
                    let _ = reply.send(task(viewer));
                });
                Request::WhenCompiled(task)
            },
            PATIENT,
        )
    }
}

impl Editor for Bridge {
    fn state(&self) -> Result<EditorState, String> {
        let (state, focus, panes, agent_edits, proposals) = self.now(|viewer| {
            (
                viewer.state(),
                viewer.focus(),
                viewer.panes(),
                viewer.agent_edits(),
                viewer.proposals().len(),
            )
        })?;
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
        } = change;
        let outcome = self.now(move |viewer| match what {
            ChangeKind::Replace { old, new, all } => {
                viewer.agent_replace(&old, &new, all, expected, propose)
            }
            ChangeKind::Insert(text) => viewer.agent_insert(&text, expected, propose),
            ChangeKind::ReplaceSelection(text) => {
                viewer.agent_replace_selection(&text, expected, propose)
            }
        })??;
        match outcome {
            Outcome::Proposed { id, revision } => Ok(json!({
                "applied": false,
                "proposal": id,
                "revision": revision,
                "note": "shown to the user, who accepts or rejects it in the window",
            })),
            Outcome::Changed { revision, lines } => {
                // What the compiler says to the text as it is by then.
                let (state, now) = self.compiled(|viewer| {
                    let state = viewer.state();
                    (state.clone(), state.revision)
                })?;
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
        let state = self.compiled(|viewer| viewer.state())?;
        let (diagnostics, more) = some_of(&state.problems);
        Ok(json!({
            "revision": state.revision,
            "diagnostics": diagnostics,
            "more_diagnostics": more,
            "pages": state.pages,
        }))
    }

    fn outline(&self) -> Result<Value, String> {
        let (outline, revision) =
            self.compiled(|viewer| (viewer.outline(), viewer.state().revision))?;
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
        Ok(json!({ "revision": revision, "headings": headings, "labels": labels }))
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
        match place {
            Place::Source { line, column } => {
                let (page, x, y) = self.compiled(move |viewer| viewer.page_of(line, column))??;
                Ok(json!({ "page": page, "x": round(x), "y": round(y) }))
            }
            Place::Page { page, x, y } => {
                let (file, line, column) =
                    self.compiled(move |viewer| viewer.source_of(page, x, y))??;
                Ok(json!({ "file": file, "line": line, "column": column }))
            }
        }
    }

    fn proposals(&self) -> Result<Value, String> {
        let proposals = self.now(|viewer| {
            let text = viewer.text().unwrap_or_default();
            let line = |offset: usize| zetwerk_app::viewer::agent::line_column(text, offset).0;
            viewer
                .proposals()
                .iter()
                .map(|proposal| {
                    json!({
                        "id": proposal.id,
                        "line": line(proposal.range.start),
                        "old_str": proposal.old,
                        "new_str": proposal.new,
                        "stale": proposal.stale,
                    })
                })
                .collect::<Vec<Value>>()
        })?;
        Ok(json!({ "proposals": proposals }))
    }

    fn withdraw_proposal(&self, id: u64) -> Result<Value, String> {
        let left = self.now(move |viewer| {
            viewer
                .withdraw_proposal(id)
                .map(|()| viewer.proposals().len())
        })??;
        Ok(json!({ "withdrawn": id, "waiting": left }))
    }

    fn export_pdf(&self, path: Option<&str>) -> Result<Value, String> {
        let export = self.compiled(|viewer| viewer.for_export())??;
        let beside = export.main.as_deref();
        let path = match (path, beside) {
            // A name is a name beside the document.
            (Some(path), Some(main)) => main
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join(path),
            (Some(path), None) => path.into(),
            (None, Some(main)) => main.with_extension("pdf"),
            (None, None) => return Err("the document is not a file; say where to write".into()),
        };
        if path.extension().is_none_or(|extension| extension != "pdf") {
            return Err(format!("{} does not end in .pdf", path.display()));
        }
        let bytes = zetwerk_typst::export_pdf(&export.document, &*export.world)
            .map_err(|errors| format!("not exported: {}", errors.join("; ")))?;
        std::fs::write(&path, &bytes)
            .map_err(|err| format!("cannot write {}: {err}", path.display()))?;
        Ok(json!({
            "path": path.display().to_string(),
            "bytes": bytes.len(),
            "pages": export.document.pages().len(),
        }))
    }

    fn save(&self) -> Result<Value, String> {
        let revision =
            self.now(|viewer| viewer.agent_save().map(|()| viewer.state().revision))??;
        Ok(json!({ "saved": true, "revision": revision }))
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
        let exported = bridge.export_pdf(None).unwrap();
        let pdf = std::fs::read(session.directory.join("main.pdf")).unwrap();
        assert!(pdf.starts_with(b"%PDF"));
        assert_eq!(exported["pages"], 2);
        assert!(bridge.export_pdf(Some("notes.txt")).is_err());
        bridge.export_pdf(Some("copy.pdf")).unwrap();
        assert!(session.directory.join("copy.pdf").is_file());
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
            json!([{ "id": id, "line": 6, "old_str": "lazy", "new_str": "busy", "stale": false }])
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
}
