//! Finding and forwarding to a window's socket.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
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
    let nothing = "no zetwerk window is running".to_owned();
    let directory = socket_directory().ok_or("XDG_RUNTIME_DIR is not set")?;
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Err(nothing);
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
        .find(is_alive)
        .ok_or(nothing)
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
