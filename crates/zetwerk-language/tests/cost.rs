//! What checking a long document costs, measured.
//!
//! Not a test of anything: it prints times. Run it with a document to
//! measure and the dictionaries of the machine:
//!
//! ```sh
//! ZETWERK_CORPUS=thesis.typ cargo test -p zetwerk-language --test cost -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use typst_syntax::Source;
use zetwerk_language::{Checked, Checker, Job, Scope, Settings, directories};

/// Has `checker` do `scope` of `text` and says how long it took.
fn timed(checker: &mut Checker, text: &str, scope: Scope, what: &str) -> Checked {
    let job = Job {
        serial: 0,
        text: 0,
        source: Source::detached(text),
        language: "en".to_owned(),
        scope,
        suggest: false,
    };
    let start = Instant::now();
    let mut last = Checked::default();
    let mut first: Option<Duration> = None;
    checker.check(&job, &|| false, &mut |checked| {
        first.get_or_insert(start.elapsed());
        last = checked;
    });
    println!(
        "{what}: {:.1?} (first answer after {:.1?}); {} found, complete: {}",
        start.elapsed(),
        first.unwrap_or_default(),
        last.findings.len(),
        last.complete
    );
    last
}

#[test]
#[ignore = "prints times for a document named by ZETWERK_CORPUS"]
fn what_checking_costs() {
    let Some(path) = std::env::var_os("ZETWERK_CORPUS") else {
        println!("ZETWERK_CORPUS names no document");
        return;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        println!("{} was not read", path.display());
        return;
    };
    let words = text.split_whitespace().count();
    println!(
        "{}: {} bytes, {words} words as written",
        path.display(),
        text.len()
    );
    let start = Instant::now();
    let source = Source::detached(text.as_str());
    println!(
        "parsed in {:.1?} (the editor has it parsed already)",
        start.elapsed()
    );
    let start = Instant::now();
    let units = zetwerk_language::units(source.root());
    println!(
        "{} paragraphs told apart in {:.1?}",
        units.len(),
        start.elapsed()
    );

    let spelling = Settings {
        spelling: true,
        directories: directories(&[]),
        ..Settings::default()
    };
    let mut checker = Checker::new(spelling.clone());
    let window = text.len() / 2..text.len() / 2 + 6000;
    let shown = timed(
        &mut checker,
        &text,
        Scope::Window(window.clone()),
        "spelling, a window of 6000 bytes, dictionary not yet read",
    );
    println!(
        "  notes: {:?}; dictionary: {:?}",
        shown.notes, shown.dictionary
    );
    timed(
        &mut checker,
        &text,
        Scope::Window(window.start + 6000..window.end + 6000),
        "spelling, the next window",
    );
    timed(
        &mut checker,
        &text,
        Scope::Next {
            from: 0,
            forward: true,
        },
        "spelling, to the next finding from the top",
    );
    timed(&mut checker, &text, Scope::All, "spelling, all of it");
    timed(
        &mut checker,
        &text,
        Scope::All,
        "spelling, all of it again (all known)",
    );
    let changed = text.replacen("theory", "theoyr", 1);
    timed(
        &mut checker,
        &changed,
        Scope::All,
        "spelling, all of it with one paragraph changed",
    );
    timed(
        &mut checker,
        &changed,
        Scope::Window(window.clone()),
        "spelling, a window of it, all known",
    );

    with_grammar(&text, spelling, window, words);
}

/// The same with grammar checked too, by what the machine has for it.
fn with_grammar(text: &str, spelling: Settings, window: std::ops::Range<usize>, words: usize) {
    let changed = text.replacen("theory", "theoyr", 1);
    let mut checker = Checker::new(Settings {
        grammar: true,
        ..spelling
    });
    timed(
        &mut checker,
        text,
        Scope::Window(window.clone()),
        "spelling and grammar, a window of 6000 bytes, nothing set up",
    );
    timed(
        &mut checker,
        text,
        Scope::Window(window.start + 6000..window.end + 6000),
        "spelling and grammar, the next window",
    );
    timed(
        &mut checker,
        &changed,
        Scope::Window(window),
        "spelling and grammar, the first window with a paragraph changed elsewhere",
    );
    if words < 20_000 {
        let all = timed(
            &mut checker,
            text,
            Scope::All,
            "spelling and grammar, all of it",
        );
        // What a short document is told, to judge it by.
        for finding in all.findings.iter().filter(|_| words < 2000) {
            let about = text.get(finding.range.clone()).unwrap_or("");
            println!(
                "  {about:?}: {} {:?}",
                finding.message, finding.replacements
            );
        }
    }
}
