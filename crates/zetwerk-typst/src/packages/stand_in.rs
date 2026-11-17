//! A stand-in for Typst's package registry, for tests of fetching: it
//! serves the files it is given over plain HTTP on 127.0.0.1, and notes
//! what was asked of it. There is no network where zetwerk is tested,
//! and the real registry is never asked by a test.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

/// The stand-in, serving until it is dropped.
pub struct Registry {
    /// Where it is: `http://127.0.0.1:<port>`, what [`super::fetch`] takes
    /// in place of [`super::REGISTRY`].
    pub url: String,
    asked: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Registry {
    /// Serves `files`, by the path they are asked for at
    /// (`/preview/touying-0.8.0.tar.gz`); anything else is not found.
    /// With `slowly`, each file comes a kilobyte at a time, this far
    /// apart: for abandoning a fetch while it comes.
    pub fn serve(files: Vec<(String, Vec<u8>)>, slowly: Option<Duration>) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let url = format!("http://{}", listener.local_addr()?);
        let files: HashMap<String, Vec<u8>> = files.into_iter().collect();
        let asked = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (noted, stopping) = (Arc::clone(&asked), Arc::clone(&stop));
        let thread = std::thread::Builder::new()
            .name("zetwerk-stand-in".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    if stopping.load(Ordering::Acquire) {
                        break;
                    }
                    let Ok(stream) = stream else {
                        continue;
                    };
                    answer(stream, &files, &noted, slowly, &stopping);
                }
            })?;
        Ok(Self {
            url,
            asked,
            stop,
            thread: Some(thread),
        })
    }

    /// The paths asked for so far, in order, each with what came with
    /// it besides the request line's path: the request's other lines.
    pub fn asked(&self) -> Vec<String> {
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Wakes the thread that waits for a connection.
        let _ = TcpStream::connect(self.url.trim_start_matches("http://"));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Answers one request.
fn answer(
    stream: TcpStream,
    files: &HashMap<String, Vec<u8>>,
    asked: &Mutex<Vec<String>>,
    slowly: Option<Duration>,
    stop: &AtomicBool,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(stream);
    let mut request = Vec::new();
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) if line.trim().is_empty() => break,
            Ok(_) => request.push(line.trim_end().to_owned()),
        }
    }
    let Some(first) = request.first() else {
        return;
    };
    let path = first.split(' ').nth(1).unwrap_or("").to_owned();
    asked
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(request.join("\n"));
    let Some(body) = files.get(&path) else {
        let _ = writer
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    };
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    if writer.write_all(head.as_bytes()).is_err() {
        return;
    }
    match slowly {
        None => {
            let _ = writer.write_all(body);
        }
        Some(pause) => {
            for chunk in body.chunks(1024) {
                if stop.load(Ordering::Acquire) || writer.write_all(chunk).is_err() {
                    return;
                }
                let _ = writer.flush();
                std::thread::sleep(pause);
            }
        }
    }
    let _ = writer.flush();
}

/// A gzipped tar archive of `files`: their paths and what is in them.
pub fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    for (path, bytes) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        if builder.append_data(&mut header, path, *bytes).is_err() {
            return Vec::new();
        }
    }
    builder
        .into_inner()
        .and_then(flate2::write::GzEncoder::finish)
        .unwrap_or_default()
}

/// The `typst.toml` of a package `name` at `version` whose entry point
/// is `lib.typ`, and that says `more` besides.
pub fn manifest(name: &str, version: &str, more: &str) -> String {
    format!(
        "[package]\nname = \"{name}\"\nversion = \"{version}\"\nentrypoint = \"lib.typ\"\n\
         authors = [\"A. Writer\"]\nlicense = \"MIT\"\ndescription = \"The {name} package.\"\n{more}"
    )
}

/// The archive of a package `name` at `version` with `lib.typ` as given,
/// as the registry serves it, and the path it is served at.
pub fn package(name: &str, version: &str, lib: &str) -> (String, Vec<u8>) {
    let manifest = manifest(name, version, "");
    let bytes = archive(&[
        ("typst.toml", manifest.as_bytes()),
        ("lib.typ", lib.as_bytes()),
    ]);
    (format!("/preview/{name}-{version}.tar.gz"), bytes)
}
