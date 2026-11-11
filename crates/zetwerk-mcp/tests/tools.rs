//! The server end to end: a real socket, a real client, a stand-in editor.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use zetwerk_mcp::{Editor, EditorState, Output, call, serve};

/// An editor that records the keys it was sent.
#[derive(Default)]
struct FakeEditor {
    keys: Mutex<Vec<String>>,
}

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
    let _server = serve(&socket, Arc::new(FakeEditor::default()), false)
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
    let _server = serve(&socket, Arc::new(FakeEditor::default()), false)
        .unwrap_or_else(|err| panic!("server starts: {err}"));
    let result = call(&socket, "render_page", arguments(&json!({ "page": 99 })));
    assert_eq!(result, Err("page 99 does not exist".to_owned()));
    // Arguments of the wrong shape are rejected before reaching the editor.
    assert!(call(&socket, "render_page", arguments(&json!({ "page": "two" }))).is_err());
}

#[test]
fn development_tools_are_only_offered_on_request() {
    let (_scratch_plain, plain) = socket("plain");
    let _server = serve(&plain, Arc::new(FakeEditor::default()), false)
        .unwrap_or_else(|err| panic!("server starts: {err}"));
    assert!(call(&plain, "send_keys", arguments(&json!({ "keys": "j" }))).is_err());
    assert!(call(&plain, "screenshot", Map::new()).is_err());

    let (_scratch_development, development) = socket("development");
    let editor = Arc::new(FakeEditor::default());
    let _server = serve(&development, Arc::clone(&editor) as Arc<dyn Editor>, true)
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
fn the_socket_is_private_and_removed_when_the_server_stops() {
    use std::os::unix::fs::PermissionsExt;

    let (_scratch_socket, socket) = socket("lifetime");
    let server = serve(&socket, Arc::new(FakeEditor::default()), false)
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
    let _server = serve(&socket, Arc::new(FakeEditor::default()), false)
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
