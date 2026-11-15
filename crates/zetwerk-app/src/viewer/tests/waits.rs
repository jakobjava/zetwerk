//! What the window may wait for: compiles that run long, parts that are
//! slow to typeset by themselves, and the keys that stop them.
//!
//! `cargo test --workspace what_the_window_waits_for -- --ignored --nocapture`
//! prints what compiles and parts take on long and slow documents.

use std::time::Instant;

use super::*;

const ARTICLE: &str = include_str!("../../../../zetwerk-corpus/docs/article.typ");

/// `copies` copies of the article's body behind its rules, with labels
/// and references of their own: sixty are about three hundred pages.
fn long(copies: usize) -> String {
    let body = ARTICLE.find("\n= ").unwrap_or(0);
    let (rules, body) = ARTICLE.split_at(body);
    let mut text = rules.to_owned();
    for copy in 0..copies {
        text.push_str(
            &body
                .replace("<eq-", &format!("<eq{copy}-"))
                .replace("@eq-", &format!("@eq{copy}-")),
        );
    }
    text
}

/// A loop of `rounds` rounds that makes nothing to show.
fn counting(rounds: usize) -> String {
    format!(
        "#let total = 0\n#for i in range({rounds}) {{ total += calc.rem(i, 7) }}\n\nThe sum is #total.\n\nA paragraph.\n"
    )
}

/// What a whole compile of `text` takes, cold and then after a change of
/// one character, in milliseconds; and the keys of `keys` that typeset a
/// part, with what the slowest of them took.
fn measure(name: &str, text: &str, keys: &str) {
    zetwerk_typst::forget_unused(0);
    let directory = directory(text);
    let started = Instant::now();
    let (mut viewer, _) = open(&directory);
    let cold = started.elapsed().as_secs_f64() * 1000.0;
    let pages = viewer.state().pages;
    // A paragraph about halfway down.
    let middle = text.len() / 2;
    let at = text[middle..]
        .find("\n\n")
        .map_or(middle, |found| middle + found + 2);
    if let Some(editor) = viewer.editor.as_mut() {
        editor.set_cursor(at.min(text.len().saturating_sub(1)));
    }
    press(&mut viewer, "w");
    let mut slowest: f64 = 0.0;
    let mut parts = 0;
    for key in keys::parse(keys).unwrap() {
        let before = viewer.stats.local;
        viewer.key(key);
        if viewer.stats.local > before {
            parts += 1;
            slowest = slowest.max(viewer.stats.local_ms);
        }
    }
    let started = Instant::now();
    let (revision, world) = viewer.snapshot();
    let compiled = zetwerk_typst::compile(&world);
    let warm = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    viewer.compiled(CompileOutput {
        revision,
        world: Arc::new(world),
        compiled,
        duration: Duration::from_secs_f64(warm / 1000.0),
    });
    let taken = started.elapsed().as_secs_f64() * 1000.0;
    // A frame with nothing held by the compositor, and one after it.
    let started = Instant::now();
    let _ = viewer.scene(&|_| false);
    let first = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let _ = viewer.scene(&|_| true);
    let next = started.elapsed().as_secs_f64() * 1000.0;
    println!(
        "{name:<22} {pages:>4} pages  cold {cold:>8.0} ms  warm {warm:>7.0} ms  \
         taken up {taken:>6.1} ms  {parts:>2} parts, slowest {slowest:>6.1} ms  \
         frames {first:>6.1} / {next:>4.1} ms"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
#[ignore = "measures; run by hand"]
fn what_the_window_waits_for() {
    let keys = "o- one<CR>two<CR><CR>And on.<Esc>";
    measure("article", ARTICLE, keys);
    measure("article x10", &long(10), keys);
    measure("article x60", &long(60), keys);
    measure("article x330", &long(330), keys);
    measure("loop 200 000", &counting(200_000), keys);
    measure("loop 2 000 000", &counting(2_000_000), keys);
    let lead = format!(
        "#let heavy = range(300000).map(i => calc.rem(i * 7, 13)).sum()\n\n{}",
        long(2)
    );
    measure("heavy rule + x2", &lead, keys);
    let table = format!(
        "{}\n\n#table(columns: 8, ..range(2400).map(i => [#i]))\n\nAfter.\n",
        long(1)
    );
    measure("big table", &table, keys);
}

/// `seconds` ago, or now on a machine that has not run that long.
fn ago(seconds: u64) -> Instant {
    let now = Instant::now();
    now.checked_sub(Duration::from_secs(seconds)).unwrap_or(now)
}

#[test]
fn ctrl_c_stops_a_compile_that_runs_long_and_says_how_to_go_on() {
    let (mut viewer, directory) = editing("Some text.\n");
    press(&mut viewer, "x");
    assert_eq!(viewer.text(), Some("ome text.\n"));
    assert!(viewer.take_edited().is_some());
    // The window says a compile of it has run for nine seconds.
    assert!(viewer.compiling(Some(ago(9)), None));
    let words = viewer.compile_words(Instant::now()).unwrap_or_default();
    assert!(words.contains("typesetting 9 s, Ctrl-C stops"), "{words}");
    press(&mut viewer, ":why<CR>");
    let said = viewer.message.as_ref().map(|message| message.text.clone());
    assert!(
        said.as_deref()
            .is_some_and(|said| said.contains("Ctrl-C stops it")),
        "{said:?}"
    );
    press(&mut viewer, "<C-c>");
    assert!(viewer.take_stop());
    assert!(!viewer.take_stop());
    // The window gives it up, and says how long it had run.
    viewer.stopped(Some(Duration::from_secs(9)));
    let said = viewer.message.as_ref().map(|message| message.text.clone());
    assert!(
        said.as_deref()
            .is_some_and(|said| said.contains("after 9 s") && said.contains("u undoes")),
        "{said:?}"
    );
    // The page stays that of the text before; the text is as it was.
    assert_eq!(viewer.state().compiled_revision, Some(0));
    assert_eq!(viewer.text(), Some("ome text.\n"));
    assert_eq!(
        viewer.compile_words(Instant::now()).as_deref(),
        Some("stopped  ")
    );
    press(&mut viewer, ":why<CR>");
    let said = viewer.message.as_ref().map(|message| message.text.clone());
    assert!(
        said.as_deref()
            .is_some_and(|said| said.contains("stopped after 9 s")),
        "{said:?}"
    );
    // And `u` takes back what made it long, to be typeset as ever.
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some("Some text.\n"));
    assert_eq!(viewer.compile_words(Instant::now()), None);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn ctrl_c_is_vims_where_no_compile_runs_long() {
    let (mut viewer, directory) = editing("Some text.\n");
    // A compile that has only just begun is not stopped.
    viewer.compiling(Some(Instant::now()), None);
    press(&mut viewer, "<C-c>");
    assert!(!viewer.take_stop());
    // Nor is one while typing: there Ctrl-C leaves insert mode.
    viewer.compiling(Some(ago(9)), None);
    press(&mut viewer, "i<C-c>");
    assert!(!viewer.take_stop());
    assert_eq!(
        viewer.state().editing.map(|editing| editing.mode),
        Some(editor::Mode::Normal)
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_status_line_counts_the_seconds_only_while_a_compile_runs() {
    let (mut viewer, directory) = editing("Some text.\n");
    let now = Instant::now();
    // Nothing compiles: nothing to wake the window for.
    assert!(!viewer.compiling(None, None));
    assert_eq!(viewer.compiling_at(now), None);
    // Just begun: said once it has run long.
    viewer.compiling(Some(now), None);
    assert_eq!(viewer.compiling_at(now), Some(now + waiting::LONG));
    assert_eq!(viewer.compile_words(now), None);
    // Running long: the next whole second, and only then.
    let since = ago(9);
    assert!(viewer.compiling(Some(since), None));
    let at = viewer.compiling_at(Instant::now());
    assert!(at.is_some_and(|at| at > Instant::now() && at <= since + Duration::from_secs(11)));
    let _ = viewer.scene(&|_| false);
    assert!(!viewer.compiling(Some(since), None));
    // A text that waits for a compile given up on says so.
    assert!(viewer.compiling(None, Some(since)));
    let words = viewer.compile_words(Instant::now()).unwrap_or_default();
    assert!(words.starts_with("waiting 9 s"), "{words}");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_document_that_always_takes_long_is_said_to_only_beyond_its_usual() {
    let (mut viewer, directory) = editing("Some text.\n");
    viewer.stats.compile_ms = 3000.0;
    viewer.compiling(Some(ago(4)), None);
    assert_eq!(viewer.compile_words(Instant::now()), None);
    press(&mut viewer, "<C-c>");
    assert!(!viewer.take_stop());
    viewer.compiling(Some(ago(7)), None);
    let words = viewer.compile_words(Instant::now()).unwrap_or_default();
    assert!(words.starts_with("typesetting 7 s"), "{words}");
    press(&mut viewer, "<C-c>");
    assert!(viewer.take_stop());
    let _ = std::fs::remove_dir_all(directory);
}

/// How many parts were typeset, or begun to be, on this thread.
fn parts_begun() -> usize {
    super::local::PARTS_TYPESET.get()
}

#[test]
fn a_slow_part_is_not_waited_for_and_turns_parts_off_until_a_whole_compile_says_otherwise() {
    // Typed into a heading: the page is not worked out, a part is
    // typeset.
    let (mut viewer, directory) = editing("= Title\n\nMore.\n");
    // No part is in time by this budget.
    viewer.waiting.part_budget = Duration::ZERO;
    let begun = parts_begun();
    press(&mut viewer, "AA");
    assert_eq!((parts_begun() - begun, viewer.stats.local), (1, 0));
    let declined = viewer.declined.clone().unwrap_or_default();
    assert!(declined.contains("takes longer than 0 ms"), "{declined}");
    // The next key does not begin one, and says why the page waits.
    press(&mut viewer, "B");
    assert_eq!(parts_begun() - begun, 1);
    let declined = viewer.declined.clone().unwrap_or_default();
    assert!(
        declined.contains("to typeset by itself: the page waits"),
        "{declined}"
    );
    // A whole compile within the budget: the cause is gone.
    viewer.waiting.part_budget = Duration::from_secs(10);
    compile(&mut viewer);
    press(&mut viewer, "C");
    assert_eq!((parts_begun() - begun, viewer.stats.local), (2, 1));
    // Slow again, and the whole not within the budget: off, and tried
    // again later.
    viewer.waiting.part_budget = Duration::ZERO;
    press(&mut viewer, "D");
    assert_eq!((parts_begun() - begun, viewer.stats.local), (3, 1));
    let (revision, world) = viewer.snapshot();
    let compiled = zetwerk_typst::compile(&world);
    viewer.compiled(CompileOutput {
        revision,
        world: Arc::new(world),
        compiled,
        duration: Duration::from_secs(5),
    });
    press(&mut viewer, "E");
    assert_eq!(parts_begun() - begun, 3);
    if let Some(slow) = viewer.waiting.slow_part.as_mut() {
        slow.at = ago(3600);
    }
    viewer.waiting.part_budget = Duration::from_secs(10);
    press(&mut viewer, "F");
    assert_eq!((parts_begun() - begun, viewer.stats.local), (4, 2));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_key_that_makes_a_long_loop_does_not_wait_for_it() {
    // Before, the part was typeset on the window's thread: the key that
    // made the loop long waited for all of it (2.1 s for two million
    // rounds). Now it waits no longer than the budget.
    let text = "= Sum\n\nZeros 000000 here.\n\nThe sum is #{let t = 0; for i in range(3) { t += i }; t}.\n";
    let (mut viewer, directory) = editing(text);
    viewer.waiting.part_budget = Duration::from_millis(100);
    if let Some(editor) = viewer.editor.as_mut() {
        editor.set_cursor(text.find("000000").unwrap_or(0));
    }
    press(&mut viewer, "yiw");
    if let Some(editor) = viewer.editor.as_mut() {
        editor.set_cursor(text.find("(3)").unwrap_or(0) + 1);
    }
    let started = Instant::now();
    press(&mut viewer, "p");
    let took = started.elapsed();
    assert!(
        viewer
            .text()
            .is_some_and(|text| text.contains("range(3000000)")),
        "{:?}",
        viewer.text()
    );
    assert!(took < Duration::from_millis(1000), "the key took {took:?}");
    let declined = viewer.declined.clone().unwrap_or_default();
    assert!(declined.contains("takes longer than 100 ms"), "{declined}");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn ctrl_c_stops_an_export_before_its_next_file() {
    let pages = 40;
    let text = vec!["Page"; pages].join("\n#pagebreak()\n");
    let (mut viewer, directory) = editing(&text);
    let (wake, woken) = std::sync::mpsc::channel();
    viewer.export_in_background(move || {
        let _ = wake.send(());
    });
    press(&mut viewer, ":png<CR>");
    assert!(viewer.is_exporting());
    press(&mut viewer, "<C-c>");
    let said = viewer.message.as_ref().map(|message| message.text.clone());
    assert!(
        said.as_deref()
            .is_some_and(|said| said.starts_with("stopping main-")
                && said.ends_with("after the file being written …")),
        "{said:?}"
    );
    woken
        .recv_timeout(Duration::from_secs(60))
        .unwrap_or_else(|err| panic!("the export never ended: {err}"));
    viewer.export_progress();
    assert!(!viewer.is_exporting());
    let said = viewer.message.as_ref().map(|message| message.text.clone());
    assert!(
        said.as_deref()
            .is_some_and(|said| said.starts_with("not exported: stopped")),
        "{said:?}"
    );
    let written = std::fs::read_dir(&directory)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|ending| ending == "png")
        })
        .count();
    assert!(written < pages, "{written} written");
    let _ = std::fs::remove_dir_all(directory);
}
