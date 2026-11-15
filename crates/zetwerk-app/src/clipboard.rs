//! The system clipboard, read: what the user copied in another program.
//!
//! It is read by the small programs made for that, `wl-paste` on Wayland
//! (from `wl-clipboard`, which every Wayland desktop has), `xclip` or
//! `xsel` on X11, `pbpaste` on macOS, and not by a library. A library
//! would have to speak the compositor's clipboard protocol on a
//! connection of its own (the window's is winit's, which offers no
//! clipboard), would bring a Wayland and an X11 client into the build,
//! and is no surer than the program the desktop itself ships. Reading
//! is done when the user asks for it, once; a program that does not
//! answer is given up on after a while, and where there is none the
//! user is told what to install.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long a program is waited for.
const PATIENCE: Duration = Duration::from_secs(2);
/// The most that is read: more than any list of sources anyone copies.
const MOST: u64 = 16 << 20;

/// A program that writes what the clipboard holds.
#[derive(Debug, Clone, Copy)]
pub struct Reader {
    pub program: &'static str,
    pub args: &'static [&'static str],
}

/// On Wayland: text, of whatever kind of text is offered.
const WAYLAND: Reader = Reader {
    program: "wl-paste",
    args: &["--no-newline", "--type", "text"],
};
/// On X11.
const X11: &[Reader] = &[
    Reader {
        program: "xclip",
        args: &["-selection", "clipboard", "-out"],
    },
    Reader {
        program: "xsel",
        args: &["--clipboard", "--output"],
    },
];
/// On macOS.
const MAC: Reader = Reader {
    program: "pbpaste",
    args: &[],
};

/// What the clipboard holds, as text.
///
/// # Errors
///
/// If there is no clipboard, nothing to read it with, it holds no text,
/// or it is not read in time.
pub fn text() -> Result<String, String> {
    // A test never reads the user's clipboard: it hands over its own.
    if cfg!(test) {
        return Err("no clipboard is read in a test".to_owned());
    }
    let set = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    let mut readers = Vec::new();
    if cfg!(target_os = "macos") {
        readers.push(MAC);
    }
    if set("WAYLAND_DISPLAY") {
        readers.push(WAYLAND);
    }
    if set("DISPLAY") {
        readers.extend_from_slice(X11);
    }
    if readers.is_empty() {
        return Err("there is no clipboard to read here: no Wayland or X11 display".to_owned());
    }
    read_with(&readers, PATIENCE)
}

/// The programs that put text into the clipboard, in the order they are
/// tried where they could be: on Wayland, on X11, on macOS.
const WRITERS: &[(&str, &[&str], &str)] = &[
    ("wl-copy", &[], "WAYLAND_DISPLAY"),
    ("xclip", &["-selection", "clipboard", "-in"], "DISPLAY"),
    ("xsel", &["--clipboard", "--input"], "DISPLAY"),
    ("pbcopy", &[], ""),
];

/// Puts `text` into the clipboard, for other programs to paste: by the
/// first program that is there to do it. It is handed over and not
/// waited for: such a program stays to hold the text until something
/// else is copied.
///
/// # Errors
///
/// If there is no clipboard, or nothing to write to it with.
pub fn write(text: &str) -> Result<(), String> {
    // A test never writes the user's clipboard: it hands over its own.
    if cfg!(test) {
        return Err("no clipboard is written in a test".to_owned());
    }
    let set = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    let mac = cfg!(target_os = "macos");
    let wanted = WRITERS.iter().filter(|(_, _, display)| {
        if display.is_empty() {
            mac
        } else {
            set(display)
        }
    });
    for (program, args, _) in wanted {
        let spawned = Command::new(program)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(format!("not copied: {program}: {err}")),
        };
        let mut input = child.stdin.take();
        let text = text.to_owned();
        // Written and waited for aside: nothing here waits on it.
        std::thread::spawn(move || {
            if let Some(input) = &mut input {
                let _ = std::io::Write::write_all(input, text.as_bytes());
            }
            drop(input);
            let _ = child.wait();
        });
        return Ok(());
    }
    Err("not copied: nothing to copy with (wl-clipboard on Wayland, xclip on X11)".to_owned())
}

/// Why a program gave nothing.
enum Failed {
    /// It is not installed.
    Missing,
    /// It said why, or did not answer.
    Said(String),
}

/// What the first of `readers` that can read the clipboard reads, each
/// given `patience` to answer.
///
/// # Errors
///
/// If none of them can: why the last one that is there could not, or
/// that none of them is installed.
pub fn read_with(readers: &[Reader], patience: Duration) -> Result<String, String> {
    let mut missing = Vec::new();
    let mut said = None;
    for reader in readers {
        match run(reader, patience) {
            Ok(text) => return Ok(text),
            Err(Failed::Missing) => missing.push(reader.program),
            Err(Failed::Said(why)) => said = Some(why),
        }
    }
    Err(said.unwrap_or_else(|| {
        format!(
            "cannot read the clipboard: {} is not installed (wl-clipboard on Wayland)",
            missing.join(" or ")
        )
    }))
}

/// Reads what `reader` writes, waiting at most `patience`.
fn run(reader: &Reader, patience: Duration) -> Result<String, Failed> {
    let spawned = Command::new(reader.program)
        .args(reader.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Err(Failed::Missing),
        Err(err) => return Err(Failed::Said(format!("{}: {err}", reader.program))),
    };
    // Read as it is written, so that a pipe that is full stops nobody.
    let drain = |pipe: Option<Box<dyn Read + Send>>, most: u64| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(pipe) = pipe {
                let _ = pipe.take(most).read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let out = drain(
        child
            .stdout
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
        MOST,
    );
    let err = drain(
        child
            .stderr
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
        4096,
    );
    let begun = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if begun.elapsed() < patience => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    // What did not answer may have left something behind that holds the
    // pipes open: its readers are left to end when that does.
    let Some(status) = status else {
        return Err(Failed::Said(format!(
            "the clipboard was not read: {} did not answer in {} s",
            reader.program,
            patience.as_secs_f32()
        )));
    };
    let out = out.join().unwrap_or_default();
    let err = err.join().unwrap_or_default();
    if status.success() {
        return Ok(String::from_utf8_lossy(&out).into_owned());
    }
    let why = String::from_utf8_lossy(&err);
    let why = why.lines().next().unwrap_or("").trim();
    let empty = [
        "Nothing is copied",
        "No suitable type",
        "target STRING not available",
    ]
    .iter()
    .any(|told| why.contains(told));
    Err(Failed::Said(if empty {
        "the clipboard holds no text".to_owned()
    } else {
        format!("the clipboard was not read: {}: {why}", reader.program)
    }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use super::*;

    const SAYS: Reader = Reader {
        program: "sh",
        args: &["-c", "printf '@book{a, title = {A}}'"],
    };
    const EMPTY: Reader = Reader {
        program: "sh",
        args: &["-c", "echo 'Nothing is copied' >&2; exit 1"],
    };
    const SLOW: Reader = Reader {
        program: "sh",
        args: &["-c", "sleep 5"],
    };
    const NONE: Reader = Reader {
        program: "zetwerk-no-such-program",
        args: &[],
    };

    #[test]
    fn the_clipboard_is_read_or_said_why_not() {
        let patience = Duration::from_millis(300);
        assert_eq!(
            read_with(&[NONE, SAYS], patience).as_deref(),
            Ok("@book{a, title = {A}}")
        );
        assert_eq!(
            read_with(&[EMPTY, NONE], patience),
            Err("the clipboard holds no text".to_owned())
        );
        let missing = read_with(&[NONE], patience).unwrap_err();
        assert!(
            missing.contains("zetwerk-no-such-program is not installed"),
            "{missing}"
        );
        let begun = Instant::now();
        let slow = read_with(&[SLOW], patience).unwrap_err();
        assert!(slow.contains("did not answer"), "{slow}");
        assert!(begun.elapsed() < Duration::from_secs(3));
    }
}
