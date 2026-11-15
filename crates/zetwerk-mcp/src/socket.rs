//! Finding and forwarding to a window's socket.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The directory windows put their sockets in, if the session has a runtime
/// directory.
fn socket_directory() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty())?;
    Some(PathBuf::from(runtime).join("zetwerk"))
}

/// The socket a window in this process should listen on.
#[must_use]
pub fn socket_path() -> Option<PathBuf> {
    Some(socket_directory()?.join(format!("{}.sock", std::process::id())))
}

/// The socket of a running window: the most recently started one that
/// accepts connections.
pub fn find_socket() -> Result<PathBuf, String> {
    let directory = socket_directory().ok_or("XDG_RUNTIME_DIR is not set")?;
    live_sockets(&directory)
        .into_iter()
        .next()
        .ok_or_else(|| "no zetwerk window is running".to_owned())
}

/// The socket of the running window that has the document of `file`
/// open. With several windows, the one that shows that file: an agent
/// asked to work on one document is never put to work on another.
pub fn find_socket_of(file: &Path) -> Result<PathBuf, String> {
    let directory = socket_directory().ok_or("XDG_RUNTIME_DIR is not set")?;
    socket_of(&directory, file)
}

/// The socket in `directory` of the window that has the document of
/// `file` open: the only window there is, or the one that shows `file`.
pub fn socket_of(directory: &Path, file: &Path) -> Result<PathBuf, String> {
    let sockets = live_sockets(directory);
    if let [only] = sockets.as_slice() {
        return Ok(only.clone());
    }
    let file = std::path::absolute(file).unwrap_or_else(|_| file.to_owned());
    let shows = |socket: &PathBuf| {
        let Ok(outputs) = crate::call(socket, "get_state", serde_json::Map::new()) else {
            return false;
        };
        let [crate::Output::Text(text)] = outputs.as_slice() else {
            return false;
        };
        serde_json::from_str::<crate::EditorState>(text)
            .is_ok_and(|state| !state.file.is_empty() && file.ends_with(&state.file))
    };
    sockets.into_iter().find(shows).ok_or_else(|| {
        format!(
            "{} is open in a zetwerk window that does not show it now",
            file.display()
        )
    })
}

/// The sockets of the windows running in `directory`, the most recently
/// started first.
fn live_sockets(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut sockets: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "sock")
        })
        .filter_map(|path| Some((path.metadata().ok()?.modified().ok()?, path)))
        .collect();
    sockets.sort();
    sockets
        .into_iter()
        .rev()
        .map(|(_, path)| path)
        .filter(is_alive)
        .collect()
}

/// Whether a window is listening on `socket`. A socket nobody listens on
/// was left behind by a window that was killed, and is removed.
fn is_alive(socket: &PathBuf) -> bool {
    match UnixStream::connect(socket) {
        Ok(_) => true,
        Err(err) => {
            if err.kind() == io::ErrorKind::ConnectionRefused {
                let _ = std::fs::remove_file(socket);
            }
            false
        }
    }
}

/// Forwards this process's standard input to the socket and the socket's
/// replies to standard output, until either side closes. MCP's stdio
/// framing is the same over a socket, so no message is looked at.
pub fn proxy_stdio(socket: &std::path::Path) -> io::Result<()> {
    let stream = UnixStream::connect(socket)?;
    let mut to_socket = stream.try_clone()?;
    let upstream = std::thread::spawn(move || {
        let result = copy(&mut io::stdin().lock(), &mut to_socket);
        // The agent closed its end: tell the window, which ends the session.
        let _ = to_socket.shutdown(std::net::Shutdown::Write);
        result
    });
    let mut from_socket = stream;
    copy(&mut from_socket, &mut io::stdout().lock())?;
    // The window closed the connection. The thread reading standard input
    // may be blocked there for good; the caller exits the process.
    drop(upstream);
    Ok(())
}

/// Copies until end of input, passing every chunk on at once: messages are
/// lines, and a reply held back in a buffer would stall the conversation.
fn copy(from: &mut impl Read, to: &mut impl Write) -> io::Result<()> {
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        match from.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => {
                to.write_all(&buffer[..count])?;
                to.flush()?;
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
}
