//! What a key costs on a long document against a short one.
//!
//! A key is to cost about as much on a thesis as on a letter: what it
//! does is to be in proportion to what it changes, not to the document.
//! The same keys are pressed on the article of the corpus and on copies
//! of it, and what they take is compared: ten copies with every test
//! run, sixty by hand. The bounds are generous (other tests running
//! beside it); a key that typesets the whole document, or walks all its
//! pages, costs as many times more as the document is longer and breaks
//! them.
//!
//! `ZETWERK_SPEED=1 cargo test --workspace keys_cost_as_much -- --ignored --nocapture`
//! prints the numbers.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Instant;

use super::*;

const ARTICLE: &str = include_str!("../../../../zetwerk-corpus/docs/article.typ");

/// `copies` copies of the article's body behind its rules, with labels
/// and references of their own.
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

/// What is pressed, by kind: moving about, prose, an equation, a list,
/// the menu of what can be put in.
const KINDS: [(&str, &str); 5] = [
    ("moving", "wwwwwbbbbbjjjjkkkkllllhhhh"),
    ("prose", "ithe quick brown fox jumps over the lazy dog<Esc>"),
    ("equation", "a $x_i^2 + ff<Tab>a<Tab>b<Tab> = alpha$<Esc>"),
    ("list", "o- one<CR>two<CR><CR>And on.<Esc>"),
    ("menu", "A #figu<Esc><Esc>"),
];

/// What a key of each of `kinds` took, the median of them, in
/// milliseconds, with the cursor put in the middle of the document
/// first. The median, so that a key the machine was busy elsewhere
/// during does not count.
fn costs(text: &str, kinds: &[(&'static str, &str)]) -> Vec<(&'static str, f64)> {
    let directory = directory(text);
    let (mut viewer, _) = open(&directory);
    press(&mut viewer, "gsgs");
    // A paragraph about halfway down.
    let middle = text.len() / 2;
    let at = text[middle..]
        .find("\n\nFor ")
        .or_else(|| text[middle..].find("\n\n"))
        .map_or(middle, |found| middle + found + 2);
    if let Some(editor) = viewer.editor.as_mut() {
        editor.set_cursor(at);
    }
    press(&mut viewer, "w");
    let mut costs = Vec::new();
    for &(name, keys) in kinds {
        let keys = keys::parse(keys).unwrap();
        let mut took: Vec<f64> = keys
            .iter()
            .map(|&key| {
                let started = Instant::now();
                viewer.key(key);
                started.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        took.sort_by(f64::total_cmp);
        costs.push((name, took[took.len() / 2]));
        // Typing pauses: all of it is typeset again.
        compile(&mut viewer);
    }
    let _ = std::fs::remove_dir_all(directory);
    costs
}

/// The kinds of keys that cost more on the long document than on the
/// short one: more than four times as much and some milliseconds over.
/// A key that does as many times the work as the document is longer
/// shows; one that does what the short document's does, with what any
/// long text costs to copy, does not.
fn slower(short: &[(&str, f64)], long: &[(&str, f64)], pages: usize) -> Vec<String> {
    let mut slower = Vec::new();
    for ((name, short), (_, long)) in short.iter().zip(long) {
        if std::env::var_os("ZETWERK_SPEED").is_some() {
            println!("{name:>9}: {short:.2} ms a key on 2 pages, {long:.2} ms on {pages}");
        }
        if *long > 4.0 * short + 8.0 {
            slower.push(format!("{name}: {short:.2} ms → {long:.2} ms"));
        }
    }
    slower
}

#[test]
fn a_key_costs_about_as_much_on_a_longer_document() {
    // Cheap enough for every run: a key that typesets all of twenty
    // pages, or walks them, costs tens of milliseconds.
    let kinds = &KINDS[..3];
    let short = costs(ARTICLE, kinds);
    let long = costs(&long(10), kinds);
    let slower = slower(&short, &long, 20);
    assert!(slower.is_empty(), "{slower:#?}");
}

#[test]
#[ignore = "a measurement: costs a few seconds of typesetting"]
fn keys_cost_as_much_on_a_long_document_as_on_a_short_one() {
    // Once, so that the fonts and caches are there for both.
    let _ = costs(ARTICLE, &KINDS);
    let short = costs(ARTICLE, &KINDS);
    let long = costs(&long(60), &KINDS);
    let slower = slower(&short, &long, 120);
    assert!(slower.is_empty(), "{slower:#?}");
}

/// How long `work` takes, on average of a few times, in milliseconds.
fn timed(mut work: impl FnMut()) -> f64 {
    const TIMES: u32 = 5;
    let started = Instant::now();
    for _ in 0..TIMES {
        work();
    }
    started.elapsed().as_secs_f64() * 1000.0 / f64::from(TIMES)
}

#[test]
#[ignore = "a measurement: what a key in an equation spends its time on"]
fn what_a_key_in_an_equation_costs_on_a_long_document() {
    for copies in [1, 60] {
        let text = long(copies);
        let directory = directory(&text);
        let (mut viewer, _) = open(&directory);
        press(&mut viewer, "gsgs");
        // Into an equation in a paragraph about halfway down, typing.
        let middle = text.len() / 2;
        let at = text[middle..]
            .find("$A$")
            .map_or(middle, |found| middle + found + 2);
        if let Some(editor) = viewer.editor.as_mut() {
            editor.set_cursor(at);
        }
        press(&mut viewer, "i+ x");
        let key = timed(|| press(&mut viewer, "y<BS>"));
        let snapshot = timed(|| {
            let _ = viewer.snapshot();
        });
        let left = timed(|| {
            let source = viewer.editor.as_ref().unwrap().source();
            let _ = crate::typst_text::left_empty_everywhere(source, None);
        });
        let completing = timed(|| {
            let source = viewer.editor.as_ref().unwrap().source().clone();
            let cursor = viewer.editor.as_ref().unwrap().cursor();
            let world = viewer.project.snapshot();
            let whole = viewer.whole.clone();
            let _ = zetwerk_typst::complete(&world, whole.as_deref(), &source, cursor, true);
        });
        // Each time from the pages of all of it, as after a pause.
        let (pages, world) = (viewer.document.clone(), viewer.compiled_world.clone());
        let local = timed(|| {
            viewer.document.clone_from(&pages);
            viewer.compiled_world.clone_from(&world);
            let _ = viewer.typeset_locally();
        });
        if std::env::var_os("ZETWERK_SPEED").is_some() {
            println!(
                "{:>4} pages: a key {:.2} ms (two keys {:.2}); snapshot {snapshot:.2}, \
                 places left empty {left:.2}, completion {completing:.2}, \
                 the part typeset {local:.2} ({:?})",
                viewer.state().pages,
                key / 2.0,
                key,
                viewer.declined
            );
        }
        let _ = std::fs::remove_dir_all(directory);
    }
}

/// What is pressed to measure a kind of key: the keys that lead up to
/// it, the keys measured, and those that end it.
const MEASURED: [(&str, &str, &str, &str); 6] = [
    ("prose", "i", "the quick brown fox jumps over", "<Esc>"),
    ("equation", "a $x_i^2 + ", "beta + gamma_j - alp", "$<Esc>"),
    (
        "backspace",
        "A",
        "<BS><BS><BS><BS><BS><BS><BS><BS>",
        "<Esc>",
    ),
    ("enter", "A", "<CR>a<CR>b<CR>c<CR>", "<Esc>"),
    ("motion", "", "wwwwwjjjjjkkkkkbbbbb", ""),
    ("undo", "xxxxxxxxxx", "uuuuuuuuuu", ""),
];

/// What can be switched off in turn, by the command that does it.
const OVERLAYS: [&str; 11] = [
    "",
    "numbers off",
    "hints off",
    "tags off",
    "errors off",
    "spell off",
    "grammar off",
    "hlsearch off",
    "nextkeys off",
    "ghost off",
    "parts off",
];

/// The median of `times`, in milliseconds.
fn median(mut times: Vec<f64>) -> f64 {
    times.sort_by(f64::total_cmp);
    times.get(times.len() / 2).copied().unwrap_or(0.0)
}

/// A viewer on `text` as a user has it: in a large window, spelling and
/// grammar checked, a search shown, a note on the text, the cursor in a
/// paragraph halfway down the page.
fn as_used(text: &str) -> (Viewer, PathBuf) {
    let directory = directory(text);
    let (mut viewer, _) = open(&directory);
    viewer.resize((1920, 1200), 1.25);
    viewer.set_language(&zetwerk_language::Settings {
        spelling: true,
        grammar: true,
        directories: zetwerk_language::directories(&[]),
        words: Some(directory.join("words.txt")),
        ..zetwerk_language::Settings::default()
    });
    compile(&mut viewer);
    viewer.language_settle();
    press(&mut viewer, "gsgs");
    let middle = text.len() / 2;
    let at = text[middle..]
        .find("\n\nFor ")
        .or_else(|| text[middle..].find("\n\n"))
        .map_or(middle, |found| middle + found + 2);
    if let Some(editor) = viewer.editor.as_mut() {
        editor.set_cursor(at);
    }
    press(&mut viewer, "/the<CR>");
    press(&mut viewer, "wvegncheck this<CR>w");
    (viewer, directory)
}

/// A clock in milliseconds: the time this thread has run with
/// `ZETWERK_CPU` set (which other work on the machine does not change),
/// else the time that went by.
fn clock() -> f64 {
    static STARTED: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);
    if std::env::var_os("ZETWERK_CPU").is_some() {
        let run = std::fs::read_to_string("/proc/thread-self/schedstat").unwrap_or_default();
        let nanoseconds: f64 = run
            .split(' ')
            .next()
            .and_then(|run| run.parse().ok())
            .unwrap_or(0.0);
        return nanoseconds / 1e6;
    }
    STARTED.elapsed().as_secs_f64() * 1000.0
}

/// Draws a frame as the window would: images it has are not made again.
fn frame(viewer: &mut Viewer, held: &mut HashSet<TextureId>) -> f64 {
    let started = clock();
    let scene = viewer.scene(&|id| held.contains(&id));
    let took = clock() - started;
    held.extend(scene.images.iter().map(|image| image.id));
    took
}

/// Compiles all of it, as a pause in typing does, and waits for the
/// spelling of what is shown.
fn settle(viewer: &mut Viewer, held: &mut HashSet<TextureId>) {
    compile(viewer);
    let _ = frame(viewer, held);
    viewer.language_settle();
    let _ = frame(viewer, held);
}

/// For each kind of key: the median time in `Viewer::key`, and in the
/// frame drawn after it; and the median of frames drawn with nothing
/// changed.
fn costs_of_keys(viewer: &mut Viewer, held: &mut HashSet<TextureId>) -> (Vec<(f64, f64)>, f64) {
    let mut costs = Vec::new();
    for (_, before, measured, after) in MEASURED {
        press(viewer, before);
        let (mut keys, mut frames) = (Vec::new(), Vec::new());
        for key in keys::parse(measured).unwrap() {
            let started = clock();
            viewer.key(key);
            keys.push(clock() - started);
            frames.push(frame(viewer, held));
            if std::env::var_os("ZETWERK_KEYS").is_some() {
                println!(
                    "    {key:?}: key {:.2} frame {:.2} {:?}",
                    keys[keys.len() - 1],
                    frames[frames.len() - 1],
                    viewer.declined
                );
            }
        }
        press(viewer, after);
        settle(viewer, held);
        costs.push((median(keys), median(frames)));
    }
    let idle = median((0..15).map(|_| frame(viewer, held)).collect());
    (costs, idle)
}

/// Prints what a key and a frame cost, with all on and with each of
/// what is drawn over the page switched off in turn:
/// `ZETWERK_SPEED=60 cargo test --release --workspace what_keys_and_frames_cost -- --ignored --nocapture`
/// for sixty copies of the article (one without the number);
/// `ZETWERK_CPU=1` for the time the thread ran rather than the time
/// that went by, which other builds on the machine do not change;
/// `ZETWERK_OVERLAYS=spell,numbers` for only some of the rows. Each row
/// begins anew from the same text and place, so that rows compare.
#[test]
#[ignore = "a measurement: prints a table"]
fn what_keys_and_frames_cost() {
    let copies = std::env::var("ZETWERK_SPEED")
        .ok()
        .and_then(|copies| copies.parse().ok())
        .unwrap_or(1);
    let only = std::env::var("ZETWERK_OVERLAYS").ok();
    let text = if copies == 1 {
        ARTICLE.to_owned()
    } else {
        long(copies)
    };
    let names: Vec<&str> = MEASURED.iter().map(|(name, ..)| *name).collect();
    // The first row once more first, unprinted, so that fonts and
    // caches are there.
    let rows = std::iter::once(None).chain(OVERLAYS.into_iter().map(Some));
    for overlay in rows {
        if let Some(overlay) = overlay
            && only.as_deref().is_some_and(|only| {
                !overlay.is_empty() && !only.contains(overlay.split(' ').next().unwrap_or(""))
            })
        {
            continue;
        }
        let (mut viewer, directory) = as_used(&text);
        let mut held = HashSet::new();
        if let Some(overlay) = overlay.filter(|overlay| !overlay.is_empty()) {
            press(&mut viewer, &format!(":{overlay}<CR>"));
        }
        settle(&mut viewer, &mut held);
        let (costs, idle) = costs_of_keys(&mut viewer, &mut held);
        let _ = std::fs::remove_dir_all(directory);
        let Some(overlay) = overlay else {
            println!(
                "{} pages; key / frame after it, in ms (medians)",
                viewer.state().pages
            );
            println!("{:>14} {:>17} idle", "off", names.join("       "));
            continue;
        };
        let mut row = format!(
            "{:>14}",
            overlay.split(' ').next().unwrap_or("").to_owned()
                + if overlay.is_empty() { "(all on)" } else { "" }
        );
        for (key, frame) in costs {
            let _ = write!(row, " {key:6.2}/{frame:5.2}");
        }
        let _ = write!(row, " {idle:5.2}");
        println!("{row}");
    }
}

/// What draws over a page: the viewer, the scene, the images the window
/// holds, the page, where it is.
type Overlay = fn(&mut Viewer, &mut Scene, &dyn Fn(TextureId) -> bool, usize, (f64, f64));

/// Where the time of a frame goes, part by part.
fn frame_parts(viewer: &mut Viewer, held: &mut HashSet<TextureId>) -> String {
    let visible: Vec<usize> = viewer
        .view
        .visible(&viewer.layout)
        .iter()
        .map(|visible| visible.index)
        .collect();
    let has = |id: TextureId| held.contains(&id);
    let mut scene = Scene::default();
    let mut parts = String::new();
    let mut time = |name: &str, work: &mut dyn FnMut(&mut Viewer, &mut Scene)| {
        let started = Instant::now();
        work(viewer, &mut scene);
        let _ = write!(
            parts,
            "{name} {:.2} ",
            started.elapsed().as_secs_f64() * 1000.0
        );
    };
    time("language", &mut |viewer, _| viewer.language_frame());
    let overlays: [(&str, Overlay); 8] = [
        ("numbers", Viewer::line_numbers),
        ("steps", Viewer::step_marks),
        ("spelling", Viewer::language_page),
        ("find", Viewer::find_marks),
        ("margin", Viewer::margin_page),
        ("cursor", Viewer::page_cursor),
        ("notes", Viewer::review_page),
        ("focus", Viewer::focus_marks),
    ];
    for (name, overlay) in overlays {
        time(name, &mut |viewer, scene| {
            for index in &visible {
                overlay(viewer, scene, &has, *index, (0.0, 0.0));
            }
        });
    }
    time("pages", &mut |viewer, scene| viewer.pages(scene, &has));
    time("lens", &mut |viewer, scene| viewer.lens(scene, &has));
    time("source", &mut |viewer, scene| {
        viewer.source_pane(scene, &has);
    });
    time("status", &mut |viewer, scene| {
        viewer.status_line(scene, &has);
    });
    time("review", &mut |viewer, scene| {
        viewer.review_panels(scene, &has);
    });
    time("hints", &mut |viewer, scene| viewer.hints(scene, &has));
    time("assist", &mut |viewer, scene| {
        viewer.assist_panels(scene, &has);
    });
    held.extend(scene.images.iter().map(|image| image.id));
    let _ = write!(parts, "images {}", scene.images.len());
    parts
}

/// What some of what a key or a frame asks for takes on the text of
/// `viewer`, in milliseconds: the state shown in the status line, the
/// equation the cursor is in, and the pages wrapped as a document again
/// (as a part typeset by itself is put in).
fn what_parts_of_a_key_ask(viewer: &Viewer) -> String {
    let timed_ms = |work: &dyn Fn()| {
        median(
            (0..9)
                .map(|_| {
                    let started = Instant::now();
                    work();
                    started.elapsed().as_secs_f64() * 1000.0
                })
                .collect(),
        )
    };
    let state = timed_ms(&|| {
        let _ = viewer.state();
    });
    let equation = timed_ms(&|| {
        if let Some(editor) = &viewer.editor {
            let _ = crate::typst_text::equation_at(editor.source(), editor.cursor());
        }
    });
    let wrapped = timed_ms(&|| {
        if let Some(document) = &viewer.document {
            let pages: typst::ecow::EcoVec<Page> = document.pages().iter().cloned().collect();
            let info = typst::model::Document::info(document.as_ref()).clone();
            let _ = PagedDocument::new(pages, info);
        }
    });
    format!(
        "the state: {state:.2} ms; the equation at the cursor: {equation:.2} ms; \
         the pages wrapped as a document again: {wrapped:.2} ms"
    )
}

#[test]
#[ignore = "a measurement: prints where frames go"]
fn where_a_frame_goes() {
    let copies = std::env::var("ZETWERK_SPEED")
        .ok()
        .and_then(|copies| copies.parse().ok())
        .unwrap_or(1);
    let text = if copies == 1 {
        ARTICLE.to_owned()
    } else {
        long(copies)
    };
    let (mut viewer, directory) = as_used(&text);
    let mut held = HashSet::new();
    settle(&mut viewer, &mut held);
    for _ in 0..3 {
        println!("idle: {}", frame_parts(&mut viewer, &mut held));
    }
    let forget = median(
        (0..9)
            .map(|_| {
                let started = Instant::now();
                zetwerk_typst::forget_unused(1_000_000);
                started.elapsed().as_secs_f64() * 1000.0
            })
            .collect(),
    );
    let status = median(
        (0..9)
            .map(|round| {
                let started = Instant::now();
                let markup = format!("#set page(width: 1500pt, height: 20pt)\n{round} of them");
                let _ = viewer.chrome.render(&markup, 1.25);
                started.elapsed().as_secs_f64() * 1000.0
            })
            .collect(),
    );
    println!("forgetting what is unused: {forget:.2} ms; a status line anew: {status:.2} ms");
    println!("{}", what_parts_of_a_key_ask(&viewer));
    let page = viewer.cursor_page.get();
    let (document, world) = (
        viewer.document.clone().unwrap(),
        viewer.compiled_world.clone().unwrap(),
    );
    let frame = &document.pages()[page].frame;
    let build = median(
        (0..9)
            .map(|_| {
                let started = Instant::now();
                let _ = PageMap::build(&world, frame);
                started.elapsed().as_secs_f64() * 1000.0
            })
            .collect(),
    );
    let map = PageMap::build(&world, frame);
    let lines = median(
        (0..9)
            .map(|_| {
                let started = Instant::now();
                let _ = map.lines();
                started.elapsed().as_secs_f64() * 1000.0
            })
            .collect(),
    );
    let away = median(
        (0..9)
            .map(|_| {
                viewer.page_lines.get_mut().clear();
                viewer.numbered = None;
                let started = Instant::now();
                viewer.line_numbers(&mut Scene::default(), &|_| true, page, (0.0, 0.0));
                started.elapsed().as_secs_f64() * 1000.0
            })
            .collect(),
    );
    println!(
        "a page's map: {build:.2} ms, its lines {lines:.2} ms; lines counted from the cursor \
         {away:.2} ms over {} pages",
        viewer.page_lines.borrow().len()
    );
    let keys = std::env::var("ZETWERK_FRAMES")
        .unwrap_or_else(|_| "A<BS><BS><CR>a<Esc>|u|jjww|xx".to_owned());
    for group in keys.split('|') {
        for key in keys::parse(group).unwrap() {
            let started = Instant::now();
            viewer.key(key);
            let took = started.elapsed().as_secs_f64() * 1000.0;
            println!("{key:?} {took:.2}: {}", frame_parts(&mut viewer, &mut held));
        }
        settle(&mut viewer, &mut held);
        println!("  idle: {}", frame_parts(&mut viewer, &mut held));
    }
    let _ = std::fs::remove_dir_all(directory);
}

/// Presses keys over and over for a profiler to look at:
/// `ZETWERK_SETUP` once, then `ZETWERK_LOOP`, each with a frame, for
/// `ZETWERK_SECONDS`.
#[test]
#[ignore = "a measurement: for a profiler"]
fn keys_in_a_loop() {
    let (Ok(setup), Ok(keys)) = (
        std::env::var("ZETWERK_SETUP"),
        std::env::var("ZETWERK_LOOP"),
    ) else {
        return;
    };
    let copies = std::env::var("ZETWERK_SPEED")
        .ok()
        .and_then(|copies| copies.parse().ok())
        .unwrap_or(60);
    let seconds: f64 = std::env::var("ZETWERK_SECONDS")
        .ok()
        .and_then(|seconds| seconds.parse().ok())
        .unwrap_or(20.0);
    let (mut viewer, directory) = as_used(&long(copies));
    let mut held = HashSet::new();
    settle(&mut viewer, &mut held);
    if std::env::var_os("ZETWERK_WARM").is_some() {
        let _ = costs_of_keys(&mut viewer, &mut held);
    }
    if let Ok(before) = std::env::var("ZETWERK_BEFORE") {
        press(&mut viewer, &before);
        settle(&mut viewer, &mut held);
    }
    press(&mut viewer, &setup);
    let _ = frame(&mut viewer, &mut held);
    let _ = viewer.snapshot();
    let text = viewer.text().unwrap_or("").to_owned();
    let cursor = viewer.editor.as_ref().map_or(0, Editor::cursor);
    for (at, put) in &viewer.shown_put {
        let near = text
            .get(at.saturating_sub(30)..(*at + 10).min(text.len()))
            .unwrap_or("");
        println!("put {put:?} at {at} (cursor {cursor}): {near:?}");
    }
    println!("looping");
    let started = Instant::now();
    let (mut rounds, mut in_keys, mut in_frames) = (0, 0.0, 0.0);
    while started.elapsed().as_secs_f64() < seconds {
        for key in keys::parse(&keys).unwrap() {
            let at = Instant::now();
            viewer.key(key);
            in_keys += at.elapsed().as_secs_f64() * 1000.0;
            in_frames += frame(&mut viewer, &mut held);
        }
        rounds += 1;
    }
    println!(
        "{rounds} rounds: {:.2} ms in keys, {:.2} ms in frames a round",
        in_keys / f64::from(rounds),
        in_frames / f64::from(rounds)
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_frame_of_what_the_window_holds_already_typesets_nothing() {
    let mut text = "#set page(width: 300pt, height: 1400pt)\n".to_owned();
    for line in 0..70 {
        let _ = write!(text, "Words of line {line}.\n\n");
    }
    let (mut viewer, directory) = editing(&text);
    press(&mut viewer, "gsgs");
    if let Some(editor) = viewer.editor.as_mut() {
        editor.set_cursor(text.find("line 2.").unwrap_or(0));
    }
    press(&mut viewer, "w");
    let mut held = HashSet::new();
    let _ = frame(&mut viewer, &mut held);
    // More numbers beside the lines than chrome keeps images: each of
    // them pushed the next out, and all were typeset again in every
    // frame.
    let zoom = viewer.view.zoom;
    let number = |away| Chrome::id(&crate::chrome::margin_number_markup(away, 6.5), zoom);
    let numbers: HashSet<usize> = (1..=99).collect();
    let scene = viewer.scene(&|id| held.contains(&id));
    let drawn: HashSet<usize> = numbers
        .into_iter()
        .filter(|away| scene.quads.iter().any(|quad| quad.texture == number(*away)))
        .collect();
    assert!(drawn.len() > 40, "{} numbers drawn", drawn.len());
    // Once the window holds them, a frame typesets nothing at all.
    let before = viewer.chrome.rasterized();
    let _ = frame(&mut viewer, &mut held);
    let _ = frame(&mut viewer, &mut held);
    assert_eq!(viewer.chrome.rasterized(), before);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_lines_of_a_page_cost_no_more_than_its_map() {
    // A full page of short lines: finding the places for the cursor on
    // it once looked through all places of the page for each glyph.
    let (viewer, directory) = editing(&long(2));
    let (document, world) = (
        viewer.document.clone().unwrap(),
        viewer.compiled_world.clone().unwrap(),
    );
    let frame = &document.pages()[0].frame;
    let map = PageMap::build(&world, frame);
    let mut built = Vec::new();
    let mut lined = Vec::new();
    for _ in 0..7 {
        let started = Instant::now();
        let map = PageMap::build(&world, frame);
        built.push(started.elapsed().as_secs_f64());
        let started = Instant::now();
        let _ = map.lines();
        lined.push(started.elapsed().as_secs_f64());
    }
    let (built, lined) = (median(built), median(lined));
    assert!(map.glyphs().len() > 1000, "{} glyphs", map.glyphs().len());
    assert!(lined < 2.5 * built, "lines {lined:.6} s, map {built:.6} s");
    let _ = std::fs::remove_dir_all(directory);
}

/// Random edits of `text`, `rounds` of them, as `(bytes, what is put
/// there)`, each in the text as the edits before it left it: much what
/// is typed into equations and what parses the rest anew, now and then
/// a paragraph of the article.
fn random_edits(text: &str, rounds: usize) -> Vec<(std::ops::Range<usize>, String)> {
    let typed = [
        "", "x", " ", "\n\n", "$", "^", "_", "/", "(", ")", ",", ";", "frac(", "$x^$", "#f[", "]",
        "\"", "vec(1, ", "*", "= ", "alpha", "\n", "sqrt(", "$ a/ $",
    ];
    let mut text = text.to_owned();
    let mut edits = Vec::new();
    let mut seed = 11_u64;
    for round in 0..rounds {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        // Mostly near an equation, where what is left empty is.
        let near = usize::try_from(seed >> 33).unwrap_or(0) % (text.len() + 1);
        let mut at = text[near..]
            .find('$')
            .filter(|_| round % 3 != 0)
            .map_or(near, |found| {
                near + found + usize::try_from(seed % 4).unwrap_or(0)
            });
        at = at.min(text.len());
        while !text.is_char_boundary(at) {
            at -= 1;
        }
        let most = if round % 7 == 0 { 400 } else { 4 };
        let mut end = (at + usize::try_from((seed >> 20) % most).unwrap_or(0)).min(text.len());
        while !text.is_char_boundary(end) {
            end += 1;
        }
        let pasted = usize::try_from(seed >> 40).unwrap_or(0) % (ARTICLE.len() - 300);
        let pasted = (pasted..pasted + 300)
            .find(|from| ARTICLE.is_char_boundary(*from))
            .unwrap_or(0);
        let mut until = pasted + 300;
        while !ARTICLE.is_char_boundary(until) {
            until += 1;
        }
        let pick = usize::try_from((seed >> 8) % 28).unwrap_or(0);
        let put = typed.get(pick).copied().unwrap_or(&ARTICLE[pasted..until]);
        text.replace_range(at..end, put);
        edits.push((at..end, put.to_owned()));
    }
    edits
}

#[test]
fn what_is_left_empty_everywhere_is_what_each_equation_lacks() {
    // What every key looks for, kept from key to key: after any edit it
    // is what each equation of the text lacks, one by one, but for the
    // one the cursor is in.
    use crate::typst_text::{equations, left_empty, left_empty_everywhere};
    let mut source = Source::detached(long(3));
    let edits = random_edits(source.text(), 400);
    for (round, (bytes, put)) in edits.into_iter().enumerate() {
        source.edit(bytes.clone(), &put);
        let all = equations(&source);
        let but = all.get(round % all.len().max(1)).filter(|_| round % 2 == 0);
        let mut one_by_one: Vec<usize> = all
            .iter()
            .filter(|equation| Some(*equation) != but)
            .flat_map(|equation| left_empty(&source, equation))
            .collect();
        one_by_one.sort_unstable();
        one_by_one.dedup();
        let everywhere = left_empty_everywhere(&source, but);
        assert_eq!(everywhere, one_by_one, "round {round}: {bytes:?} {put:?}");
        // Asked again of the same text, as the next frame does.
        assert!(left_empty_everywhere(&source, None).len() >= everywhere.len());
    }
}

#[test]
fn a_part_on_two_pages_is_refused_before_it_is_typeset() {
    // A paragraph that runs from one page onto the next: what is typed
    // into it is not typeset by itself, and that is known before the
    // part is typeset. Typing an equation once typeset the rest of the
    // document as the part, to refuse it then.
    let mut text = "#set page(width: 300pt, height: 200pt)\n\nFirst words.\n\n".to_owned();
    for word in 0..120 {
        let _ = write!(text, "word{word} ");
    }
    text.push_str("\n\nLast words.\n");
    let (mut viewer, directory) = editing(&text);
    press(&mut viewer, "gsgs");
    assert!(viewer.state().pages >= 2);
    if let Some(editor) = viewer.editor.as_mut() {
        editor.set_cursor(text.find("word3 ").unwrap_or(0));
    }
    press(&mut viewer, "i");
    let typeset = super::local::PARTS_TYPESET.get();
    press(&mut viewer, "$");
    let declined = viewer.declined.clone().unwrap_or_default();
    assert!(declined.contains("two pages"), "{declined}");
    assert_eq!(super::local::PARTS_TYPESET.get(), typeset);
    let _ = std::fs::remove_dir_all(directory);
}

/// Prints a digest of what tables and stand-ins are found over random
/// edits, to compare two versions of how they are found by:
/// `cargo test --workspace what_is_found_digest -- --ignored --nocapture`.
#[test]
#[ignore = "a comparison between versions, by hand"]
fn what_is_found_digest() {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let table = "\n\n#table(columns: 2, table.header([a], [b]), [x $y^2$], [z])\n\n\
                 - an item #footnote[a note] and\n  more\n\n#figure([x], caption: [it])\n\n";
    let mut text = long(3);
    text.insert_str(text.len() / 2, table);
    let mut source = Source::detached(text.clone());
    for (bytes, put) in random_edits(&text, 150) {
        source.edit(bytes, &put);
        let text = source.text();
        for at in (0..=text.len()).step_by(3) {
            format!("{:?}", crate::typst_text::enclosing(&source, at)).hash(&mut hasher);
            if let Some(table) = crate::typst_text::table_at(&source, at) {
                (table.whole, table.cells, table.columns, table.header).hash(&mut hasher);
                table.head.hash(&mut hasher);
            }
        }
        for start in (0..text.len()).step_by(41) {
            let end = (start + 700).min(text.len());
            if text.is_char_boundary(start) && text.is_char_boundary(end) {
                crate::typst_text::stand_ins(&source, &(start..end)).hash(&mut hasher);
            }
        }
    }
    println!("digest {:x}", hasher.finish());
}

/// What this process holds in memory, in megabytes.
fn resident() -> f64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|kb| kb.trim().trim_end_matches("kB").trim().parse::<f64>().ok())
        .map_or(0.0, |kb| kb / 1024.0)
}

/// Prints what opening a long document costs, to the first page drawn
/// and its tiles made, and what scrolling through all of it costs, with
/// what is held in memory at each step:
/// `ZETWERK_SPEED=350 cargo test --release --workspace what_opening_costs -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement: prints what opening and scrolling cost"]
fn what_opening_costs() {
    let copies = std::env::var("ZETWERK_SPEED")
        .ok()
        .and_then(|copies| copies.parse().ok())
        .unwrap_or(60);
    let text = long(copies);
    let directory = directory(&text);
    let path = directory.join("main.typ");
    let before = resident();
    let started = Instant::now();
    let ms = |since: Instant| since.elapsed().as_secs_f64() * 1000.0;
    let env = Environment::new(FontSet::Embedded);
    let project = Project::open(Arc::clone(&env), &path).unwrap();
    let (editor, opened) = Editor::open(&path, &directory.join("state"), project.main()).unwrap();
    let mut viewer = Viewer::new(env, project, (1920, 1200), 1.25)
        .unwrap_or_else(|| panic!("viewer sets up"))
        .editing(editor, &opened);
    let set_up = ms(started);
    let at = Instant::now();
    compile(&mut viewer);
    let compiled = ms(at);
    let after_compile = resident();
    let at = Instant::now();
    let mut held = HashSet::new();
    let scene = viewer.scene(&|id| held.contains(&id));
    held.extend(scene.images.iter().map(|image| image.id));
    let first_frame = ms(at);
    let at = Instant::now();
    let tiles = scene.wanted.len();
    for tile in &scene.wanted {
        let _ = tile.render();
    }
    let first_tiles = ms(at);
    println!(
        "{} pages: set up {set_up:.0} ms, compiled {compiled:.0} ms, first frame \
         {first_frame:.1} ms, its {tiles} tiles {first_tiles:.0} ms; to the first page \
         {:.0} ms. Memory: {before:.0} MB before, {after_compile:.0} MB compiled, {:.0} MB \
         drawn",
        viewer.state().pages,
        ms(started),
        resident()
    );
    // Through all of it a screen at a time, as fast as frames come:
    // what each frame asks to be made.
    let at = Instant::now();
    let (mut frames, mut made, mut rendering) = (0, 0, 0.0);
    let pages = viewer.state().pages;
    while viewer.state().page < pages && frames < 2000 {
        press(&mut viewer, "<C-f>");
        let scene = viewer.scene(&|id| held.contains(&id));
        held.extend(scene.images.iter().map(|image| image.id));
        let tiling = Instant::now();
        for tile in &scene.wanted {
            let _ = tile.render();
            made += 1;
        }
        rendering += ms(tiling);
        frames += 1;
    }
    println!(
        "scrolled through in {frames} screens: {:.0} ms, of it {rendering:.0} ms making \
         {made} tiles ({:.1} ms a tile); memory {:.0} MB",
        ms(at),
        rendering / f64::from(made.max(1)),
        resident()
    );
    let _ = std::fs::remove_dir_all(directory);
}

/// Prints what what a key asks of the text costs on a long one, by
/// itself, each a key after an edit (so that nothing kept for a text is
/// of that text yet): the place the cursor is in (`ground` twice, as the
/// shown text asks), the equation there, whether it is in a footnote,
/// the table there, what typing goes into, the stand-ins of the part,
/// and the places left empty everywhere. Medians, in milliseconds:
/// `ZETWERK_SPEED=350 cargo test --release --workspace what_the_text_asks_cost -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement: prints what a key asks of the text"]
fn what_the_text_asks_cost() {
    use crate::typst_text as text;
    let copies = std::env::var("ZETWERK_SPEED")
        .ok()
        .and_then(|copies| copies.parse().ok())
        .unwrap_or(60);
    let mut source = Source::detached(long(copies));
    let middle = source.text().len() / 2;
    let at = middle + source.text()[middle..].find("\n\nFor ").unwrap_or(0) + 6;
    let part = at.saturating_sub(300)..at + 300;
    let mut times: Vec<Vec<f64>> = vec![Vec::new(); 7];
    for round in 0..41 {
        if round % 2 == 0 {
            source.edit(at..at, "x");
        } else {
            source.edit(at..at + 1, "");
        }
        let mut time = |which: usize, work: &mut dyn FnMut()| {
            let started = Instant::now();
            work();
            times[which].push(started.elapsed().as_secs_f64() * 1000.0);
        };
        time(0, &mut || {
            let _ = (text::ground(&source, at), text::ground(&source, at + 1));
        });
        time(1, &mut || {
            let _ = (
                text::equation_at(&source, at),
                text::is_in_equation(&source, at),
            );
        });
        time(2, &mut || {
            let _ = text::is_in_call(&source, at, "footnote");
        });
        time(3, &mut || {
            let _ = text::table_at(&source, at);
        });
        time(4, &mut || {
            let _ = text::enclosing(&source, at);
        });
        time(5, &mut || {
            let _ = text::stand_ins(&source, &part);
        });
        time(6, &mut || {
            let _ = text::left_empty_everywhere(&source, None);
        });
    }
    let names = [
        "ground ×2",
        "equation ×2",
        "in a call",
        "table",
        "enclosing",
        "stand-ins",
        "left empty",
    ];
    let mut line = format!("{} bytes:", source.text().len());
    for (name, times) in names.iter().zip(times) {
        let _ = write!(line, " {name} {:.3}", median(times));
    }
    println!("{line}");
}
