//! A disk that takes no more, and then does again.
//!
//! The test runs itself as a worker whose files may grow no larger than a
//! limit the operating system enforces (`ulimit -f`; the signal that would
//! end the worker is ignored, so writes beyond it fail as on a full disk).
//! The worker edits and saves until writing fails, and reports what it was
//! told: every edit that was accepted, every save that succeeded. Then the
//! limit is lifted (`prlimit`), as when room is made on the disk, and the
//! worker edits on. Then the document is opened again, and must hold every
//! edit that was accepted; the file must be the last save, whole.

#![cfg(unix)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a failed step should fail the test"
)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use zetwerk_core::{DiskError, Document, Opened, Replace};

const WORKER: &str = "ZETWERK_FULL_WORKER";
/// What the worker's files may grow to, in blocks of 512 bytes.
const LIMIT_BLOCKS: u32 = 24;

fn open(directory: &Path) -> (Document<String>, Opened) {
    Document::open(
        &directory.join("work/doc.typ"),
        &directory.join("state"),
        |text| text,
    )
    .expect("the document opens")
}

/// Edits and saves until the disk is full, waits for a line on standard
/// input (room was made), and edits on; says on standard output what was
/// accepted: `edit <length>` with the length of the text after an accepted
/// edit, `saved <length>` after a save that wrote the file, `full` when it
/// waits.
fn worker(directory: &Path) {
    let (mut document, _) = open(directory);
    let mut out = std::io::stdout().lock();
    let mut refused = 0;
    let mut after = None;
    for line in 0.. {
        let end = document.text().len();
        let edit = format!("line {line} of a document that grows\n");
        match document.edit(&[Replace::insert(end, edit)], false) {
            Ok(_) => writeln!(out, "edit {}", document.text().len()).unwrap(),
            Err(_) => {
                // Refused: the text is as it was.
                assert_eq!(document.text().len(), end);
                refused += 1;
            }
        }
        if line % 4 == 3 {
            match document.save(false) {
                // Written, though perhaps not noted in the journal.
                Ok(()) | Err(DiskError::Journal(_)) => {
                    writeln!(out, "saved {}", document.text().len()).unwrap();
                }
                Err(_) => writeln!(out, "unsaved").unwrap(),
            }
        }
        if refused == 5 && after.is_none() {
            writeln!(out, "full").unwrap();
            out.flush().unwrap();
            let mut go = String::new();
            std::io::stdin().read_line(&mut go).unwrap();
            after = Some(line);
        }
        if after.is_some_and(|after| line > after + 12) {
            break;
        }
    }
    out.flush().unwrap();
}

#[test]
fn a_full_disk_loses_nothing_that_was_accepted() {
    if let Some(directory) = std::env::var_os(WORKER) {
        worker(Path::new(&directory));
        return;
    }
    let directory: PathBuf =
        std::env::temp_dir().join(format!("zetwerk-full-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(directory.join("work")).unwrap();
    std::fs::write(directory.join("work/doc.typ"), "= A document\n").unwrap();
    // Opened once without the limit, as a document is before the disk
    // fills up.
    drop(open(&directory));

    let exe = std::env::current_exe().unwrap();
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "trap '' XFSZ; ulimit -S -f {LIMIT_BLOCKS}; exec \"$0\" --exact \
             a_full_disk_loses_nothing_that_was_accepted --nocapture --test-threads=1"
        ))
        .arg(&exe)
        .env(WORKER, &directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut go = child.stdin.take().unwrap();
    let mut accepted = 0;
    let mut saved = None;
    let mut full = false;
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        let line = line.unwrap();
        if let Some(length) = line.strip_prefix("edit ") {
            accepted = length.parse().unwrap();
        } else if let Some(length) = line.strip_prefix("saved ") {
            saved = Some(length.parse::<usize>().unwrap());
        } else if line == "full" {
            full = true;
            let lifted = Command::new("prlimit")
                .arg(format!("--pid={}", child.id()))
                .arg("--fsize=unlimited:")
                .status()
                .unwrap();
            assert!(lifted.success());
            writeln!(go, "go").unwrap();
        }
    }
    assert!(child.wait().unwrap().success(), "the worker failed");
    assert!(full, "the disk never filled up");
    assert!(accepted > 0, "nothing was accepted at all");

    // Without the limit: every accepted edit is back, and the file is the
    // last save, whole.
    let (document, opened) = open(&directory);
    assert!(
        matches!(opened, Opened::Clean | Opened::Restored),
        "{opened:?}"
    );
    assert_eq!(document.text().len(), accepted, "an accepted edit was lost");
    let file = std::fs::read_to_string(directory.join("work/doc.typ")).unwrap();
    if let Some(saved) = saved {
        assert_eq!(file.len(), saved, "the file is not the last save");
    }
    assert!(document.text().starts_with(&file));
    let _ = std::fs::remove_dir_all(directory);
}
