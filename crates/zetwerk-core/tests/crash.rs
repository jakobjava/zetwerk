//! Killing the process for real.
//!
//! This test runs itself as a worker that edits and saves a document in a
//! loop, and kills that worker with SIGKILL at random moments: in the middle
//! of journal writes, of saves, of the journal being rewritten on open.
//! After every kill it opens the document and checks that
//!
//! - every edit the worker reported as made is there,
//! - the text is one the worker really produced, never a mixture,
//! - the file holds a complete earlier save, never part of one,
//! - recovery never reports a conflict or an unreadable journal,
//! - every line is credited to whoever the worker had write it: an
//!   agent wrote every fourth line, the author the others.
//!
//! The worker then carries on from what was recovered, so damage that
//! recovery introduced would show up in a later round.
//!
//! This covers the process dying. It cannot cover power loss, where data
//! the operating system had accepted but not yet written is lost too.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a failed step should fail the test"
)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use zetwerk_core::{Agent, By, Document, Opened, Origin, Replace};

const WORKER: &str = "ZETWERK_CRASH_WORKER";
const ROUNDS: usize = 60;

/// A line the worker writes and takes back again, to put undo, redo and
/// abandoned branches into the journal. It may be the last line of a
/// recovered text, never of a saved file.
const TRANSIENT: &str = "~\n";

fn main() {
    match std::env::var_os(WORKER) {
        Some(directory) => worker(Path::new(&directory)),
        None => parent(),
    }
}

fn open(directory: &Path) -> (Document<String>, Opened) {
    Document::open(
        &directory.join("work/doc.typ"),
        &directory.join("state"),
        |text| text,
    )
    .expect("the document opens")
}

/// The number of the last line of a text made of the lines `1`, `2`, `3`…,
/// allowing a trailing [`TRANSIENT`] line if `transient`. Panics on anything
/// else: such a text was never written.
fn last_line(text: &str, transient: bool) -> u64 {
    let lines = match text.strip_suffix(TRANSIENT) {
        Some(rest) if transient => rest,
        _ => text,
    };
    let mut expected = 0;
    for line in lines.split_inclusive('\n') {
        expected += 1;
        assert_eq!(line, format!("{expected}\n"), "line {expected} of {text:?}");
    }
    expected
}

/// Who the worker has write line `line`.
fn origin_of(line: u64) -> Origin {
    if line % 4 == 1 {
        Origin::Agent(Agent {
            name: "worker".to_owned(),
            ..Agent::default()
        })
    } else {
        Origin::Author
    }
}

/// Appends numbered lines forever, with saves, undos and abandoned branches
/// in between, reporting each line once it is made.
fn worker(directory: &Path) -> ! {
    let (mut document, _) = open(directory);
    // Carry on from whatever was recovered.
    if document.text().ends_with(TRANSIENT) {
        let end = document.text().len();
        document
            .edit(&[Replace::delete(end - TRANSIENT.len()..end)], false)
            .unwrap();
    }
    let mut line = last_line(document.text(), false);
    let mut stdout = std::io::stdout().lock();
    loop {
        line += 1;
        let end = document.text().len();
        if line % 11 == 5 {
            // Start a branch and abandon it.
            document
                .edit(&[Replace::insert(end, TRANSIENT)], false)
                .unwrap();
            assert!(document.undo().unwrap().is_some());
        }
        // A line that is about to be undone must be an undo step of its own:
        // joined to the line before, undoing it would take that line along,
        // and a kill at that moment would look like a reported line lost.
        let undo = line % 7 == 3;
        let join = line.is_multiple_of(3) && !undo;
        let by = origin_of(line);
        document
            .edit_by(
                &[Replace::insert(end, format!("{line}\n"))],
                join,
                &By::Origin(&by),
            )
            .unwrap();
        if undo {
            assert!(document.undo().unwrap().is_some());
            assert!(document.redo().unwrap().is_some());
        }
        // Reported only now: from here on the line must survive.
        writeln!(stdout, "{line}").unwrap();
        if line.is_multiple_of(5) {
            document.save(false).unwrap();
        }
        if line.is_multiple_of(13) {
            document.sync().unwrap();
        }
        if line % 97 == 50 {
            // The journal is written anew, thinned: a kill in the middle
            // leaves it as it was or thinned.
            document.thin(40).unwrap();
        }
    }
}

fn spawn_worker(directory: &Path) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .env(WORKER, directory)
        .stdout(Stdio::piped())
        .spawn()
        .expect("the worker starts")
}

/// Opens the document as the next session would and checks what came back.
/// Returns the number of the last line of the recovered text.
fn verify(directory: &Path, reported: u64, round: usize) -> u64 {
    let (document, opened) = open(directory);
    assert!(
        matches!(opened, Opened::Clean | Opened::Restored),
        "round {round}: recovery gave {opened:?}"
    );
    let recovered = last_line(document.text(), true);
    // Each line is its writer's, every byte of it.
    let origins = document.origins();
    let text = document.text();
    assert_eq!(origins.map().len(), text.len(), "round {round}");
    let mut start = 0;
    for (number, line) in text.split_inclusive('\n').enumerate() {
        let expected = match line {
            TRANSIENT => Origin::Author,
            _ => origin_of(number as u64 + 1),
        };
        for (_, mark) in origins.map().within(start..start + line.len()) {
            assert_eq!(
                origins.credit(mark.by).origin,
                expected,
                "round {round}: line {}",
                number + 1
            );
        }
        start += line.len();
    }
    assert!(
        recovered >= reported,
        "round {round}: line {reported} was reported but only {recovered} came back"
    );
    let file = directory.join("work/doc.typ");
    if let Ok(saved) = std::fs::read_to_string(&file) {
        let saved = last_line(&saved, false);
        assert!(
            saved <= recovered,
            "round {round}: the file is ahead of the journal"
        );
        assert_eq!(
            saved % 5,
            0,
            "round {round}: the file holds a state that was never saved"
        );
    }
    // Nothing but the document may be left in its directory.
    let strays: Vec<PathBuf> = std::fs::read_dir(directory.join("work"))
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| *path != file)
        .collect();
    assert!(strays.is_empty(), "round {round}: left behind {strays:?}");
    recovered
}

fn parent() {
    let directory = std::env::temp_dir().join(format!("zetwerk-crash-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(directory.join("work")).unwrap();

    let mut random = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = |bound: u64| {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        random % bound
    };

    let mut recovered = 0;
    for round in 0..ROUNDS {
        let mut child = spawn_worker(&directory);
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut reported = recovered;
        if round % 4 == 3 {
            // Kill by the clock, to also hit the worker while it opens the
            // document and rewrites the journal.
            std::thread::sleep(Duration::from_micros(next(3000)));
        } else {
            // Kill right after a random number of lines.
            for _ in 0..=next(400) {
                match lines.next() {
                    Some(Ok(line)) => reported = line.parse().unwrap(),
                    _ => break,
                }
            }
        }
        child.kill().unwrap();
        child.wait().unwrap();
        // Lines the worker reported before it died but we had not read yet.
        for line in lines.map_while(Result::ok) {
            reported = line.parse().unwrap_or(reported);
        }
        let now = verify(&directory, reported, round);
        assert!(
            now >= recovered,
            "round {round}: went back from {recovered} to {now}"
        );
        recovered = now;
    }
    assert!(recovered > 0, "the worker never got anything done");
    let _ = std::fs::remove_dir_all(&directory);
    println!("crash: {ROUNDS} kills survived, {recovered} lines intact");
}
