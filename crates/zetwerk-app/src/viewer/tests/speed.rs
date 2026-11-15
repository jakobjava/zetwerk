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
