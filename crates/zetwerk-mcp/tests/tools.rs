//! The server end to end: a real socket, a real client, a stand-in editor.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use zetwerk_mcp::{
    Change, ChangeKind, Editor, EditorState, Output, Place, Selection, Tools, call, serve,
};

/// An editor that records the keys it was sent and the changes it was
/// asked for.
#[derive(Default)]
struct FakeEditor {
    keys: Mutex<Vec<String>>,
    changes: Mutex<Vec<Change>>,
}

const PLAIN: Tools = Tools {
    save: false,
    development: false,
};

const PNG: &[u8] = b"\x89PNG not really";

impl Editor for FakeEditor {
    fn state(&self) -> Result<EditorState, String> {
        Ok(EditorState {
            file: "thesis.typ".to_owned(),
            pages: 12,
            page: 3,
            zoom: 1.5,
            revision: 7,
            compiled_revision: Some(6),
            mode: "normal".to_owned(),
            line: 4,
            column: 2,
            modified: true,
            selection: Some(Selection {
                from_line: 4,
                from_column: 2,
                to_line: 4,
                to_column: 7,
                text: "words".to_owned(),
            }),
            layout: "split".to_owned(),
            agent_edits: "live".to_owned(),
            proposals: 0,
            command: None,
            problems: vec!["thesis.typ:1:1: error: nope".to_owned()],
        })
    }

    fn render_page(&self, page: usize, max_px: u32) -> Result<Vec<u8>, String> {
        if page > 12 {
            return Err(format!("page {page} does not exist"));
        }
        Ok([PNG, max_px.to_string().as_bytes()].concat())
    }

    fn send_keys(&self, keys: &str) -> Result<(), String> {
        self.keys
            .lock()
            .map_err(|err| err.to_string())?
            .push(keys.to_owned());
        Ok(())
    }

    fn screenshot(&self) -> Result<Vec<u8>, String> {
        Ok(PNG.to_vec())
    }

    fn read_source(&self, from: Option<usize>, to: Option<usize>) -> Result<String, String> {
        Ok(format!(
            "revision 7, lines {from:?} to {to:?}\n1\t= Title\n"
        ))
    }

    fn change(&self, change: Change) -> Result<Value, String> {
        if change
            .expected_revision
            .is_some_and(|expected| expected != 7)
        {
            return Err("the text is at revision 7".to_owned());
        }
        let proposed = change.propose;
        self.changes
            .lock()
            .map_err(|err| err.to_string())?
            .push(change);
        Ok(json!({ "applied": !proposed, "revision": 8 }))
    }

    fn diagnostics(&self) -> Result<Value, String> {
        Ok(json!({ "diagnostics": ["thesis.typ:1:1: error: nope"] }))
    }

    fn outline(&self) -> Result<Value, String> {
        Ok(json!({ "headings": [{ "level": 1, "text": "Title", "line": 1, "page": 1 }] }))
    }

    fn search(&self, query: &str, ignore_case: bool, limit: usize) -> Result<Value, String> {
        Ok(json!({ "query": query, "ignore_case": ignore_case, "limit": limit }))
    }

    fn locate(&self, place: Place) -> Result<Value, String> {
        Ok(match place {
            Place::Source { line, column } => json!({ "page": 1, "from": [line, column] }),
            Place::Page { page, x, y } => json!({ "line": 1, "from": [page, x, y] }),
        })
    }

    fn proposals(&self) -> Result<Value, String> {
        Ok(json!({ "proposals": [] }))
    }

    fn withdraw_proposal(&self, id: u64) -> Result<Value, String> {
        Err(format!("there is no proposal {id}"))
    }

    fn export_pdf(&self, path: Option<&str>) -> Result<Value, String> {
        Ok(json!({ "path": path.unwrap_or("thesis.pdf") }))
    }

    fn save(&self) -> Result<Value, String> {
        Ok(json!({ "saved": true }))
    }
}

/// What a tool answered, as JSON.
fn answer(socket: &std::path::Path, tool: &str, with: &Value) -> Result<Value, String> {
    let outputs = call(socket, tool, arguments(with))?;
    let [Output::Text(text)] = outputs.as_slice() else {
        return Err(format!("expected text, got {outputs:?}"));
    };
    serde_json::from_str(text).map_err(|err| format!("{err}: {text}"))
}

/// A directory that is removed again when the test is done.
struct Scratch(std::path::PathBuf);

impl std::ops::Deref for Scratch {
    type Target = std::path::Path;

    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A socket path in a directory of its own.
fn socket(name: &str) -> (Scratch, PathBuf) {
    let directory =
        std::env::temp_dir().join(format!("zetwerk-mcp-test-{name}-{}", std::process::id()));
    let socket = directory.join("window.sock");
    (Scratch(directory), socket)
}

fn arguments(value: &Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

#[test]
fn document_tools_answer() {
    let (_scratch_socket, socket) = socket("document");
    let _server = serve(&socket, Arc::new(FakeEditor::default()), PLAIN)
        .unwrap_or_else(|err| panic!("server starts: {err}"));

    let state = call(&socket, "get_state", Map::new()).unwrap_or_else(|err| panic!("{err}"));
    let [Output::Text(text)] = state.as_slice() else {
        panic!("expected text, got {state:?}")
    };
    let reported: EditorState = serde_json::from_str(text).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(
        (reported.pages, reported.page, reported.revision),
        (12, 3, 7)
    );
    assert_eq!(reported.compiled_revision, Some(6));
    assert_eq!(reported.problems.len(), 1);

    // The default size applies, and an oversized request is limited.
    let page = call(&socket, "render_page", arguments(&json!({ "page": 2 })));
    let expected = Output::Image {
        mime_type: "image/png".to_owned(),
        data: [PNG, b"1500"].concat(),
    };
    assert_eq!(page, Ok(vec![expected]));
    let huge = call(
        &socket,
        "render_page",
        arguments(&json!({ "page": 2, "max_px": 99999 })),
    );
    let Ok([Output::Image { data, .. }]) = huge.as_deref() else {
        panic!("expected an image")
    };
    assert!(data.ends_with(b"4000"));
}

#[test]
fn a_failing_tool_reports_its_message() {
    let (_scratch_socket, socket) = socket("failure");
    let _server = serve(&socket, Arc::new(FakeEditor::default()), PLAIN)
        .unwrap_or_else(|err| panic!("server starts: {err}"));
    let result = call(&socket, "render_page", arguments(&json!({ "page": 99 })));
    assert_eq!(result, Err("page 99 does not exist".to_owned()));
    // Arguments of the wrong shape are rejected before reaching the editor.
    assert!(call(&socket, "render_page", arguments(&json!({ "page": "two" }))).is_err());
}

#[test]
fn development_tools_are_only_offered_on_request() {
    let (_scratch_plain, plain) = socket("plain");
    let _server = serve(&plain, Arc::new(FakeEditor::default()), PLAIN)
        .unwrap_or_else(|err| panic!("server starts: {err}"));
    assert!(call(&plain, "send_keys", arguments(&json!({ "keys": "j" }))).is_err());
    assert!(call(&plain, "screenshot", Map::new()).is_err());

    let (_scratch_development, development) = socket("development");
    let editor = Arc::new(FakeEditor::default());
    let tools = Tools {
        save: false,
        development: true,
    };
    let _server = serve(&development, Arc::clone(&editor) as Arc<dyn Editor>, tools)
        .unwrap_or_else(|err| panic!("server starts: {err}"));
    let sent = call(
        &development,
        "send_keys",
        arguments(&json!({ "keys": "gg:q<CR>" })),
    );
    assert_eq!(sent, Ok(vec![Output::Text("keys sent".to_owned())]));
    assert_eq!(
        editor.keys.lock().map(|keys| keys.clone()).ok(),
        Some(vec!["gg:q<CR>".into()])
    );
    let shot = call(&development, "screenshot", Map::new());
    let image = Output::Image {
        mime_type: "image/png".to_owned(),
        data: PNG.to_vec(),
    };
    assert_eq!(shot, Ok(vec![image]));
}

#[test]
fn reading_tools_pass_on_what_was_asked() {
    let (_scratch_socket, socket) = socket("reading");
    let _server = serve(&socket, Arc::new(FakeEditor::default()), PLAIN)
        .unwrap_or_else(|err| panic!("server starts: {err}"));
    let read = call(
        &socket,
        "read_source",
        arguments(&json!({ "from_line": 3 })),
    );
    let expected = "revision 7, lines Some(3) to None\n1\t= Title\n".to_owned();
    assert_eq!(read, Ok(vec![Output::Text(expected)]));

    let found = answer(&socket, "search", &json!({ "query": "fox" }));
    assert_eq!(
        found,
        Ok(json!({ "query": "fox", "ignore_case": false, "limit": 50 }))
    );
    let many = answer(
        &socket,
        "search",
        &json!({ "query": "fox", "ignore_case": true, "max_results": 100_000 }),
    );
    assert_eq!(
        many,
        Ok(json!({ "query": "fox", "ignore_case": true, "limit": 500 }))
    );

    // A place of the source, or a place on a page; nothing in between.
    let on_page = answer(&socket, "locate", &json!({ "line": 12 }));
    assert_eq!(on_page, Ok(json!({ "page": 1, "from": [12, 1] })));
    let in_source = answer(
        &socket,
        "locate",
        &json!({ "page": 2, "x": 100.5, "y": 30 }),
    );
    assert_eq!(
        in_source,
        Ok(json!({ "line": 1, "from": [2, 100.5, 30.0] }))
    );
    for mixed in [
        json!({}),
        json!({ "page": 2 }),
        json!({ "line": 1, "page": 2 }),
    ] {
        assert!(answer(&socket, "locate", &mixed).is_err(), "{mixed}");
    }

    assert!(answer(&socket, "get_diagnostics", &json!({})).is_ok());
    assert!(answer(&socket, "get_outline", &json!({})).is_ok());
    assert_eq!(
        answer(&socket, "list_proposals", &json!({})),
        Ok(json!({ "proposals": [] }))
    );
    assert_eq!(
        answer(&socket, "export_pdf", &json!({ "path": "out.pdf" })),
        Ok(json!({ "path": "out.pdf" }))
    );
}

#[test]
fn editing_tools_say_what_to_change_and_how() {
    let (_scratch_socket, socket) = socket("editing");
    let editor = Arc::new(FakeEditor::default());
    let _server = serve(&socket, Arc::clone(&editor) as Arc<dyn Editor>, PLAIN)
        .unwrap_or_else(|err| panic!("server starts: {err}"));
    let edit = json!({ "old_str": "fox", "new_str": "cat", "expected_revision": 7 });
    assert_eq!(
        answer(&socket, "edit_source", &edit),
        Ok(json!({ "applied": true, "revision": 8 }))
    );
    let proposal =
        json!({ "old_str": "a", "new_str": "b", "replace_all": true, "mode": "propose" });
    assert_eq!(
        answer(&socket, "edit_source", &proposal),
        Ok(json!({ "applied": false, "revision": 8 }))
    );
    assert!(answer(&socket, "insert_at_cursor", &json!({ "text": "x" })).is_ok());
    let selection = json!({ "text": "y", "mode": "apply" });
    assert!(answer(&socket, "replace_selection", &selection).is_ok());
    let replace = |old: &str, new: &str, all: bool| ChangeKind::Replace {
        old: old.to_owned(),
        new: new.to_owned(),
        all,
    };
    let change = |what: ChangeKind, expected_revision: Option<u64>, propose: bool| Change {
        what,
        expected_revision,
        propose,
    };
    assert_eq!(
        editor.changes.lock().map(|changes| changes.clone()).ok(),
        Some(vec![
            change(replace("fox", "cat", false), Some(7), false),
            change(replace("a", "b", true), None, true),
            change(ChangeKind::Insert("x".to_owned()), None, false),
            change(ChangeKind::ReplaceSelection("y".to_owned()), None, false),
        ])
    );

    // The editor's refusal reaches the agent; so does a missing argument
    // and a mode there is not.
    let stale = json!({ "old_str": "fox", "new_str": "cat", "expected_revision": 3 });
    assert_eq!(
        answer(&socket, "edit_source", &stale),
        Err("the text is at revision 7".to_owned())
    );
    assert!(answer(&socket, "edit_source", &json!({ "old_str": "fox" })).is_err());
    let odd = json!({ "old_str": "a", "new_str": "b", "mode": "force" });
    assert!(answer(&socket, "edit_source", &odd).is_err());
    assert_eq!(
        answer(&socket, "withdraw_proposal", &json!({ "id": 4 })),
        Err("there is no proposal 4".to_owned())
    );
}

#[test]
fn saving_is_only_offered_on_request() {
    let (_scratch_plain, plain) = socket("nosave");
    let _server = serve(&plain, Arc::new(FakeEditor::default()), PLAIN)
        .unwrap_or_else(|err| panic!("server starts: {err}"));
    assert!(answer(&plain, "save", &json!({})).is_err());

    let (_scratch_saving, saving) = socket("save");
    let tools = Tools {
        save: true,
        development: false,
    };
    let _server = serve(&saving, Arc::new(FakeEditor::default()), tools)
        .unwrap_or_else(|err| panic!("server starts: {err}"));
    assert_eq!(
        answer(&saving, "save", &json!({})),
        Ok(json!({ "saved": true }))
    );
}

#[test]
fn the_socket_is_private_and_removed_when_the_server_stops() {
    use std::os::unix::fs::PermissionsExt;

    let (_scratch_socket, socket) = socket("lifetime");
    let server = serve(&socket, Arc::new(FakeEditor::default()), PLAIN)
        .unwrap_or_else(|err| panic!("server starts: {err}"));
    let directory = socket
        .parent()
        .unwrap_or_else(|| panic!("socket has a directory"));
    let mode = std::fs::metadata(directory).map(|meta| meta.permissions().mode() & 0o777);
    assert_eq!(mode.ok(), Some(0o700));
    assert!(socket.exists());

    drop(server);
    assert!(!socket.exists());
    assert!(call(&socket, "get_state", Map::new()).is_err());
}

#[test]
fn several_clients_can_talk_at_once() {
    let (_scratch_socket, socket) = socket("concurrent");
    let _server = serve(&socket, Arc::new(FakeEditor::default()), PLAIN)
        .unwrap_or_else(|err| panic!("server starts: {err}"));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let socket = socket.clone();
            std::thread::spawn(move || call(&socket, "get_state", Map::new()).is_ok())
        })
        .collect();
    for thread in threads {
        assert_eq!(thread.join().ok(), Some(true));
    }
}
