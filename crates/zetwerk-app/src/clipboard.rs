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
//!
//! Where the compositor offers it, the clipboard is the window system's
//! own instead, through a library (see `wayland`): no program is
//! started, a change of what is copied is known as it happens, and the
//! primary selection (`"*`) is there too. The programs stay for where it
//! is not (GNOME, X11, macOS) or fails. What a paste wants beyond text
//! (an image, files, the cells of a table) is read by [`copied`]; what
//! zetwerk copied is handed to `wl-copy` when the window closes, so that
//! it can still be pasted (see [`keep_after_exit`]).

#[cfg(all(unix, not(target_os = "macos")))]
mod wayland;

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
    if let Some(text) = native_text(Selection::Clipboard) {
        return text;
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
    if native_write(Selection::Clipboard, text) {
        return Ok(());
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
    let out = run_bytes(reader.program, reader.args, patience, MOST)?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// Reads what `program` with `args` writes, at most `most` bytes,
/// waiting at most `patience`.
fn run_bytes(
    program: &str,
    args: &[&str],
    patience: Duration,
    most: u64,
) -> Result<Vec<u8>, Failed> {
    let spawned = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Err(Failed::Missing),
        Err(err) => return Err(Failed::Said(format!("{program}: {err}"))),
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
        most,
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
            "the clipboard was not read: {program} did not answer in {} s",
            patience.as_secs_f32()
        )));
    };
    let out = out.join().unwrap_or_default();
    let err = err.join().unwrap_or_default();
    if status.success() {
        return Ok(out);
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
        format!("the clipboard was not read: {program}: {why}")
    }))
}

/// Which of the system's selections: the clipboard (`"+`, what is
/// copied), or the primary selection (`"*`, what was selected last, which
/// the middle button pastes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    Clipboard,
    Primary,
}

impl Selection {
    /// What it is called.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Clipboard => "clipboard",
            Self::Primary => "selection",
        }
    }
}

/// The most an image that is pasted may have.
const MOST_IMAGE: u64 = 64 << 20;

/// The kinds of images that are pasted as they are, the one preferred
/// first: a drawing stays a drawing.
pub const IMAGE_KINDS: &[(&str, &str)] = &[
    ("image/svg+xml", "svg"),
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/gif", "gif"),
    ("image/webp", "webp"),
];

/// The kinds that are text.
const TEXT_KINDS: &[&str] = &[
    "text/plain;charset=utf-8",
    "text/plain",
    "UTF8_STRING",
    "STRING",
    "TEXT",
];

/// What a selection holds, as far as a paste wants it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Copied {
    /// The kinds of data it is offered as.
    pub types: Vec<String>,
    /// As text, where it is offered so.
    pub text: Option<String>,
    /// As a web page, where it is offered so and the text may be cells.
    pub html: Option<String>,
    /// As an image: its kind and its bytes; where there is no text, or
    /// the text is no cells (a spreadsheet offers a picture of them too).
    pub image: Option<(String, Vec<u8>)>,
    /// The files copied in a file manager.
    pub files: Vec<std::path::PathBuf>,
}

impl Copied {
    /// Text and nothing more, as a test hands it over.
    #[must_use]
    pub fn text(text: &str) -> Self {
        Self {
            types: vec!["text/plain".to_owned()],
            text: Some(text.to_owned()),
            ..Self::default()
        }
    }
}

/// What can be known of a selection without reading it: the number of
/// its last change, and whether zetwerk copied what it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct News {
    pub serial: u64,
    pub ours: bool,
}

/// Sets the window system's own clipboard up, aside, if there is one:
/// from then on [`news`] knows of every change, as it happens.
pub fn start() {
    if cfg!(test) {
        return;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::thread::Builder::new()
        .name("zetwerk-clipboard-start".to_owned())
        .spawn(|| {
            let _ = wayland::native();
        });
}

/// What is known of the clipboard without reading it and without
/// waiting: `None` where the window system does not tell (no native
/// clipboard, or not set up yet), or it holds nothing.
#[must_use]
pub fn news() -> Option<News> {
    if cfg!(test) {
        return None;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let native = wayland::native_now().filter(|native| native.works())?;
        let (_, serial, ours) = native.offered(Selection::Clipboard)?;
        Some(News { serial, ours })
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    None
}

/// What `selection` holds as text, read by the window system's own
/// clipboard; `None` where there is none to read it with.
fn native_text(selection: Selection) -> Option<Result<String, String>> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let native = wayland::native().filter(|native| native.works())?;
        let Some((types, _, _)) = native.offered(selection) else {
            return Some(Err(format!("the {} holds no text", selection.name())));
        };
        let Some(kind) = TEXT_KINDS
            .iter()
            .find(|kind| types.iter().any(|t| t == *kind))
        else {
            return Some(Err(format!("the {} holds no text", selection.name())));
        };
        let read = native.read(selection, kind, PATIENCE, MOST);
        Some(read.map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        let _ = selection;
        None
    }
}

/// Puts `text` into `selection` by the window system's own clipboard.
/// Returns whether it could.
fn native_write(selection: Selection, text: &str) -> bool {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        wayland::native().is_some_and(|native| native.write(selection, text))
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        let _ = (selection, text);
        false
    }
}

/// Puts `text` into `selection`, for other programs to paste.
///
/// # Errors
///
/// If there is no such selection, or nothing to write to it with.
pub fn write_to(selection: Selection, text: &str) -> Result<(), String> {
    match selection {
        Selection::Clipboard => write(text),
        Selection::Primary => {
            if cfg!(test) {
                return Err("no selection is written in a test".to_owned());
            }
            if native_write(selection, text) {
                return Ok(());
            }
            let set = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
            let writers: &[(&str, &[&str], &str)] = &[
                ("wl-copy", &["--primary"], "WAYLAND_DISPLAY"),
                ("xclip", &["-selection", "primary", "-in"], "DISPLAY"),
                ("xsel", &["--primary", "--input"], "DISPLAY"),
            ];
            for (program, args, _) in writers.iter().filter(|(_, _, display)| set(display)) {
                if hand_to(program, args, text, None).is_ok() {
                    return Ok(());
                }
            }
            Err("not copied: there is no selection to copy into here".to_owned())
        }
    }
}

/// Starts `program` with `args` and hands it `text`; with `wait`, waits
/// that long at most for it to have taken it.
fn hand_to(program: &str, args: &[&str], text: &str, wait: Option<Duration>) -> Result<(), String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("{program}: {err}"))?;
    let mut input = child.stdin.take();
    let text = text.to_owned();
    let handed = std::thread::spawn(move || {
        if let Some(input) = &mut input {
            let _ = std::io::Write::write_all(input, text.as_bytes());
        }
        drop(input);
        let _ = child.wait();
    });
    if let Some(wait) = wait {
        let begun = Instant::now();
        while !handed.is_finished() && begun.elapsed() < wait {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    Ok(())
}

/// When the window closes: what zetwerk copied and holds itself is given
/// to `wl-copy`, which stays to hold it (it leaves itself running in the
/// background) until something else is copied. Where the programs
/// copied it, they hold it already; a clipboard manager would have taken
/// it too.
pub fn keep_after_exit() {
    if cfg!(test) {
        return;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Some(native) = wayland::native_now() {
        for (selection, text) in native.held_texts() {
            let args: &[&str] = match selection {
                Selection::Clipboard => &[],
                Selection::Primary => &["--primary"],
            };
            let _ = hand_to("wl-copy", args, &text, Some(Duration::from_secs(1)));
        }
    }
}

/// What `selection` holds, for a paste: its kinds, and of them what a
/// paste can take (see [`Copied`]). Reads by the window system's own
/// clipboard where there is one, else by the programs; waits for the
/// program that copied it (each kind at most [`PATIENCE`]): never to be
/// called on the window's thread.
///
/// # Errors
///
/// If it cannot be read, or holds nothing.
pub fn copied(selection: Selection) -> Result<Copied, String> {
    if cfg!(test) {
        return Err("no clipboard is read in a test".to_owned());
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Some(native) = wayland::native().filter(|native| native.works()) {
        let Some((types, _, _)) = native.offered(selection) else {
            return Err(format!("the {} holds nothing", selection.name()));
        };
        return copied_by(&types, &|kind, most| {
            native.read(selection, kind, PATIENCE, most)
        });
    }
    let set = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    let primary = selection == Selection::Primary;
    if set("WAYLAND_DISPLAY") {
        let mut list = vec!["--list-types"];
        if primary {
            list.push("--primary");
        }
        if let Ok(listed) = run_bytes("wl-paste", &list, PATIENCE, MOST) {
            let types: Vec<String> = String::from_utf8_lossy(&listed)
                .lines()
                .map(str::to_owned)
                .collect();
            return copied_by(&types, &|kind, most| {
                let mut args = vec!["--no-newline", "--type", kind];
                if primary {
                    args.push("--primary");
                }
                run_bytes("wl-paste", &args, PATIENCE, most).map_err(|failed| match failed {
                    Failed::Missing => "wl-paste is not installed".to_owned(),
                    Failed::Said(why) => why,
                })
            });
        }
    }
    if set("DISPLAY") {
        let which = if primary { "primary" } else { "clipboard" };
        let list = ["-selection", which, "-t", "TARGETS", "-out"];
        if let Ok(listed) = run_bytes("xclip", &list, PATIENCE, MOST) {
            let types: Vec<String> = String::from_utf8_lossy(&listed)
                .lines()
                .map(str::to_owned)
                .collect();
            return copied_by(&types, &|kind, most| {
                let args = ["-selection", which, "-t", kind, "-out"];
                run_bytes("xclip", &args, PATIENCE, most).map_err(|failed| match failed {
                    Failed::Missing => "xclip is not installed".to_owned(),
                    Failed::Said(why) => why,
                })
            });
        }
    }
    // Text, as it has always been read.
    if primary {
        return Err("there is no selection to read here".to_owned());
    }
    text().map(|text| Copied::text(&text))
}

/// What a selection offered as `types` holds for a paste, each kind read
/// by `read` (given the kind and the most bytes to take).
/// What reads one kind of data of a selection, at most so many bytes.
type ReadKind<'a> = &'a dyn Fn(&str, u64) -> Result<Vec<u8>, String>;

fn copied_by(types: &[String], read: ReadKind) -> Result<Copied, String> {
    let has = |kind: &str| types.iter().any(|t| t == kind);
    let mut copied = Copied {
        types: types.to_vec(),
        ..Copied::default()
    };
    if has("text/uri-list") {
        let list = read("text/uri-list", MOST)?;
        copied.files = files_in(&String::from_utf8_lossy(&list));
    }
    if let Some(kind) = TEXT_KINDS.iter().find(|kind| has(kind)) {
        let text = read(kind, MOST)?;
        copied.text = Some(String::from_utf8_lossy(&text).into_owned());
    }
    let cells = copied
        .text
        .as_deref()
        .is_some_and(|text| crate::table_text::cells_in(text, has("text/csv")).is_some());
    if has("text/html") && (copied.text.is_none() || cells) {
        let html = read("text/html", MOST)?;
        copied.html = Some(String::from_utf8_lossy(&html).into_owned());
    }
    let image = IMAGE_KINDS.iter().find(|(kind, _)| has(kind));
    if let Some((kind, _)) = image.filter(|_| !cells && copied.files.is_empty()) {
        copied.image = Some(((*kind).to_owned(), read(kind, MOST_IMAGE)?));
    }
    if copied.text.is_none()
        && copied.image.is_none()
        && copied.files.is_empty()
        && copied.html.is_none()
    {
        return Err("the clipboard holds nothing that can be pasted here".to_owned());
    }
    Ok(copied)
}

/// The files a list of URIs names (`text/uri-list`): those on this
/// machine, `file://` with what is written as `%xx` undone.
#[must_use]
pub fn files_in(list: &str) -> Vec<std::path::PathBuf> {
    list.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let path = line.strip_prefix("file://")?;
            // `file://host/path`: only this machine's.
            let path = &path[path.find('/')?..];
            let mut bytes = Vec::with_capacity(path.len());
            let mut rest = path.as_bytes();
            while let Some((&byte, after)) = rest.split_first() {
                let hex = after
                    .get(..2)
                    .and_then(|hex| std::str::from_utf8(hex).ok())
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok());
                if let (b'%', Some(decoded)) = (byte, hex) {
                    bytes.push(decoded);
                    rest = &after[2..];
                } else {
                    bytes.push(byte);
                    rest = after;
                }
            }
            Some(std::path::PathBuf::from(String::from_utf8(bytes).ok()?))
        })
        .collect()
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

    #[test]
    fn files_copied_are_read_from_their_list() {
        let list = "# from a file manager\r\nfile:///home/me/a%20b.png\r\nfile://localhost/c.svg\nhttps://x.org/d.png\n";
        assert_eq!(
            files_in(list),
            [
                std::path::PathBuf::from("/home/me/a b.png"),
                std::path::PathBuf::from("/c.svg")
            ]
        );
    }

    #[test]
    fn what_a_paste_reads_depends_on_what_is_offered() {
        let types = |kinds: &[&str]| {
            kinds
                .iter()
                .map(|kind| (*kind).to_owned())
                .collect::<Vec<_>>()
        };
        let read = |kind: &str, _: u64| -> Result<Vec<u8>, String> {
            Ok(match kind {
                "text/plain" => b"a\tb\n1\t2\n".to_vec(),
                "UTF8_STRING" => b"just words".to_vec(),
                "text/html" => b"<table></table>".to_vec(),
                "image/png" => b"PNG".to_vec(),
                "image/svg+xml" => b"<svg/>".to_vec(),
                "text/uri-list" => b"file:///a.png\n".to_vec(),
                _ => return Err(format!("{kind} was not asked for")),
            })
        };
        // A spreadsheet: cells, as text and a page; its picture is not read.
        let sheet = copied_by(&types(&["image/png", "text/html", "text/plain"]), &read).unwrap();
        assert_eq!(sheet.text.as_deref(), Some("a\tb\n1\t2\n"));
        assert!(sheet.html.is_some() && sheet.image.is_none());
        // A drawing: as a drawing rather than a picture.
        let drawing = copied_by(&types(&["image/png", "image/svg+xml"]), &read).unwrap();
        assert_eq!(
            drawing.image,
            Some(("image/svg+xml".to_owned(), b"<svg/>".to_vec()))
        );
        // Words and a picture (a browser's image): the picture; no page.
        let both = copied_by(&types(&["UTF8_STRING", "image/png"]), &read).unwrap();
        assert_eq!(
            both.image.map(|(kind, _)| kind).as_deref(),
            Some("image/png")
        );
        assert_eq!(both.html, None);
        // Files.
        let files = copied_by(&types(&["text/uri-list", "image/png"]), &read).unwrap();
        assert_eq!(files.files, [std::path::PathBuf::from("/a.png")]);
        assert_eq!(files.image, None);
        assert!(copied_by(&types(&["application/x-other"]), &read).is_err());
    }
}
