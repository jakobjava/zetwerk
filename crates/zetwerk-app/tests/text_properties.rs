//! What the text functions say holds of any text: the places they name
//! are in the text and on boundaries of its characters, the cursor they
//! put goes into what they put in, and what they say they change is what
//! changes.
//!
//! The texts are made by a seeded walk over pieces of Typst, whole and
//! broken: the same seeds give the same texts, and a failure names the
//! seed and the text.

#![allow(clippy::unwrap_used, reason = "a failed step should fail the test")]

use std::ops::Range;

use typst::syntax::Source;
use zetwerk_app::typst_text;
use zetwerk_app::{bib, figure_text, ref_text, sections};
use zetwerk_vim::Format;

/// A small generator of numbers, the same for the same seed.
struct Dice(u64);

impl Dice {
    fn next(&mut self) -> u64 {
        // SplitMix64.
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn pick<'a>(&mut self, of: &[&'a str]) -> &'a str {
        of[self.below(of.len())]
    }
}

/// Pieces of documents: whole things, and things half typed.
const PIECES: &[&str] = &[
    "word ",
    "Some words here. ",
    "é ü ✓ 漢字 ",
    "\n",
    "\n\n",
    "  ",
    "$x^2$",
    "$ frac(a, b) $",
    "$x^$",
    "$x_()$",
    "$frac(, )$",
    "$sqrt()$",
    "$ a / $",
    "$alpha + beta$",
    "$\"quoted\"$",
    "$",
    "$漢",
    "*漢字",
    "_é\n\n",
    "*strong*",
    "_emph_",
    "*",
    "_",
    "= Heading\n",
    "== Sub heading <sec>\n",
    "= ",
    "- item\n",
    "+ counted\n",
    "- ",
    "/ Term: description\n",
    "#table(\n  columns: 2,\n  [a], [b],\n  [c], [d],\n)",
    "#table(columns: (auto, 1fr), table.header([H], [I]), [x], [y])",
    "#grid(columns: 3, [1], [2])",
    "#table(columns: 1, [])",
    "#figure(image(\"a.png\"), caption: [A picture.]) <fig-a>",
    "#figure(table(columns: 1, [x]), caption: [])",
    "#footnote[a note]",
    "#strong[bold]",
    "#emph[]",
    "#set text(size: 10pt)\n",
    "#set page(columns: 2)\n",
    "#set text(",
    "#let x = 1\n",
    "#",
    "#f(",
    "[",
    "]",
    "(",
    ")",
    "{",
    "}",
    "\"",
    "`code`",
    "```rust\nfn main() {}\n```",
    "`",
    "<label>",
    "@label",
    "@",
    "\\",
    "\\*",
    "// a comment\n",
    "/* block */",
    "https://example.com ",
    "#bibliography(\"refs.bib\")\n",
    "#link(\"https://x.y\")[there]",
];

/// A text of `count` pieces.
fn text(dice: &mut Dice, count: usize) -> String {
    (0..count).map(|_| dice.pick(PIECES)).collect()
}

/// The places of `text` the functions are asked about: every boundary
/// of a character, and its end.
fn places(text: &str) -> Vec<usize> {
    let mut all: Vec<usize> = text.char_indices().map(|(at, _)| at).collect();
    all.push(text.len());
    all
}

/// Whether `range` is a range of `text` between characters.
fn fits(text: &str, range: &Range<usize>) -> bool {
    range.start <= range.end
        && range.end <= text.len()
        && text.is_char_boundary(range.start)
        && text.is_char_boundary(range.end)
}

/// A replacement of `range` by `with` with the cursor `within` it: all of
/// that fits, and the text that comes of it.
fn replaced(
    what: &str,
    text: &str,
    at: usize,
    (range, with, within): (Range<usize>, String, usize),
) -> String {
    assert!(
        fits(text, &range),
        "{what} at {at}: {range:?} is not in {text:?}"
    );
    let mut out = text.to_owned();
    let cursor = range.start + within;
    out.replace_range(range, &with);
    // The cursor goes into what was put in, or behind it (out of the
    // text set off that it ends): on a character of the new text.
    assert!(
        cursor <= out.len() && out.is_char_boundary(cursor),
        "{what} at {at}: the cursor {within} into {with:?} is off {out:?} ({text:?})"
    );
    out
}

/// Applies `edits`, each of which refers to the text as the ones before
/// it left it (from the last to the first, as the table functions make
/// them).
fn applied(what: &str, text: &str, edits: &[(Range<usize>, String)]) -> String {
    let mut out = text.to_owned();
    for (range, with) in edits {
        assert!(fits(&out, range), "{what}: {range:?} is not in {out:?}");
        out.replace_range(range.clone(), with);
    }
    out
}

const TYPED: &[char] = &[
    'a', ' ', '*', '_', '$', '#', '@', '=', '-', '+', '/', '<', '[', ']', '\\', '"', '`', '~', 'é',
    '(', ')', '^',
];

/// Everything asked at one place of one text.
fn at_place(source: &Source, at: usize) {
    let text = source.text();
    for &c in TYPED {
        if let Some(change) = typst_text::typed(source, at, c) {
            let _ = replaced("typed", text, at, change);
        }
    }
    for format in [Format::Strong, Format::Emph] {
        if let Some(change) = typst_text::set_off(source, at, format) {
            let _ = replaced("set_off", text, at, change);
        }
    }
    if let Some((range, with)) = typst_text::unmake(source, at) {
        let _ = replaced("unmake", text, at, (range, with, 0));
        assert!(
            !typst_text::unmake(source, at).is_some_and(|(range, _)| range.is_empty()),
            "unmake at {at} takes nothing: {text:?}"
        );
    }
    let (range, with) = typst_text::line_break(source, at);
    let _ = replaced("line_break", text, at, (range, with, 0));
    if let Some(change) = typst_text::tab_key(source, at) {
        let _ = replaced("tab_key", text, at, change);
    }
    if let Some(next) = typst_text::next_field(source, at) {
        assert!(fits(text, &(next..next)), "next_field at {at}: {next}");
    }
    for below in [false, true] {
        let (place, with, within) = typst_text::open(source, at, below);
        let _ = replaced("open", text, at, (place..place, with, within));
    }
    if let Some(change) = typst_text::snippet(source, at) {
        let _ = replaced("snippet", text, at, change);
    }
    for name in ['$', 'e', 'h', 'l', 'a', 'f', '*', '_'] {
        for around in [false, true] {
            if let Some(range) = typst_text::object(source, name, at, around) {
                assert!(
                    fits(text, &range),
                    "object {name} at {at}: {range:?} {text:?}"
                );
            }
        }
    }
    for spaced in [false, true] {
        if let Some((range, with)) = typst_text::join(source, at, spaced) {
            let _ = replaced("join", text, at, (range, with, 0));
        }
    }
    if let Some(next) = text.get(at..).and_then(|rest| rest.chars().next()) {
        let stop = at..at + next.len_utf8();
        for c in ['x', '$', ' '] {
            if let Some(change) = typst_text::put_over(source, &stop, c) {
                let _ = replaced("put_over", text, at, change);
            }
        }
    }
    if let Some(table) = typst_text::table_at(source, at) {
        tables(source, at, &table);
    }
    let _ = typst_text::raw_blank(source, at);
    let _ = typst_text::is_empty_spot(source, at);
    let _ = typst_text::lead_for_part(source, at);
    if let Some(equation) = typst_text::equation_at(source, at) {
        assert!(fits(text, &equation), "equation_at {at}: {equation:?}");
        for place in typst_text::empty_places(source, &equation) {
            assert!(fits(text, &(place..place)), "empty place {place} {text:?}");
        }
        for script in typst_text::empty_scripts(source, &equation) {
            assert!(fits(text, &script), "empty script {script:?} {text:?}");
        }
    }
    let _ = ref_text::label_at(source, at);
}

/// What is done to a table at a place of it.
fn tables(source: &Source, at: usize, table: &typst_text::Table) {
    let text = source.text();
    assert!(fits(text, &table.whole));
    for cell in &table.cells {
        assert!(fits(text, cell) && table.whole.start <= cell.start && cell.end <= table.whole.end);
    }
    if let Some((place, row, within)) = typst_text::table_new_row(source, table) {
        let grown = replaced("table_new_row", text, at, (place..place, row, within));
        // One row more, of the same columns.
        let grown = Source::detached(grown);
        let again = typst_text::table_at(&grown, table.whole.start + 1);
        if let Some(again) = again {
            let rows = table.cells.len().div_ceil(table.columns);
            assert_eq!(
                again.cells.len(),
                (rows + 1) * table.columns,
                "a new row of {text:?} gives {:?}",
                grown.text()
            );
        }
    }
    let wider = applied(
        "table_new_column",
        text,
        &typst_text::table_new_column(source, table),
    );
    if let Some(index) = table.cell_at(at) {
        let fewer = applied(
            "table_without_row",
            text,
            &typst_text::table_without_row(table, index),
        );
        let narrower = applied(
            "table_without_column",
            text,
            &typst_text::table_without_column(table, index),
        );
        for (what, changed) in [("row", &fewer), ("column", &narrower), ("wider", &wider)] {
            // Still a table where it was, or no table: never one that
            // has more cells than it had where fewer were asked for.
            let changed = Source::detached(changed.clone());
            if let Some(after) = typst_text::table_at(&changed, table.whole.start + 1)
                && what != "wider"
            {
                assert!(
                    after.cells.len() <= table.cells.len(),
                    "{what}: {text:?} became {:?}",
                    changed.text()
                );
            }
        }
    }
}

#[test]
fn the_text_functions_stay_in_the_text() {
    let seeds = std::env::var("ZETWERK_TEXT_SEEDS")
        .ok()
        .and_then(|seeds| seeds.parse().ok())
        .unwrap_or(150_u64);
    for seed in 0..seeds {
        let mut dice = Dice(seed);
        let count = 1 + dice.below(8);
        let text = text(&mut dice, count);
        let source = Source::detached(text.clone());
        let result = std::panic::catch_unwind(|| {
            for at in places(&text) {
                at_place(&source, at);
            }
            let _ = typst_text::left_empty_everywhere(&source, None);
            let _ = typst_text::count(&source);
            let _ = typst_text::bibliography_file(&source);
            let _ = ref_text::read(&source);
            let _ = sections::sections(&source);
            let _ = figure_text::figures(&source);
        });
        if let Err(panic) = result {
            let said = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_default();
            panic!("seed {seed}, text {text:?}: {said}");
        }
    }
}

#[test]
fn a_rule_that_is_set_reads_back() {
    for seed in 0..300 {
        let mut dice = Dice(seed);
        let count = dice.below(6);
        let text = text(&mut dice, count);
        let source = Source::detached(text.clone());
        let value = dice.pick(&["11pt", "\"Libertinus Serif\"", "(x: 2cm)", "true"]);
        let (function, key) = *[("text", "size"), ("page", "margin"), ("par", "justify")]
            .get(dice.below(3))
            .unwrap();
        let (range, with) = typst_text::set_rule(&source, function, key, value);
        let changed = replaced("set_rule", &text, 0, (range, with, 0));
        let changed = Source::detached(changed);
        assert_eq!(
            typst_text::rule_value(&changed, function, key).as_deref(),
            Some(value),
            "seed {seed}: {function}.{key} = {value} in {text:?} gives {:?}",
            changed.text()
        );
    }
}

/// Entries of BibTeX, whole and broken, and what is between them.
const BIB_PIECES: &[&str] = &[
    "@article{knuth1984,\n  author = {Knuth, Donald},\n  title = {Breaking {Paragraphs}},\n  year = 1981,\n}\n",
    "@Book(lamport94,\n  author = \"Leslie Lamport\",\n  title = {LaTeX},\n  year = {1994}\n)\n",
    "@misc{web, url = {https://x.org/a@b}, note = \"say \\\"hi\\\"\"}\n",
    "@online{mail, author = {Ann {de} Vries and Bob}, title = {Mail me at a@b.c}}\n",
    "@inproceedings{p1, title = {Unicode: über ✓}, pages = {1--2}}\n",
    "@string{jmlr = \"JMLR\"}\n",
    "@comment{ignored @article{x, y = z} }\n",
    "% a comment with @ in it\n",
    "\n\n",
    "@article{broken, title = {never closed\n",
    "@article{nokey}\n",
    "stray text\n",
];

/// An entry as it reads: its key, its kind and its fields.
type Read = (String, String, Vec<(String, String)>);

/// The entries of `text` other than the one at `except`, as they read.
fn entries_but(text: &str, except: Option<&str>) -> Vec<Read> {
    bib::parse(text)
        .into_iter()
        .filter(|entry| Some(entry.key.as_str()) != except)
        .map(|entry| (entry.key, entry.kind, entry.fields))
        .collect()
}

#[test]
fn a_change_to_one_source_leaves_the_others_as_they_were() {
    for seed in 0..400 {
        let mut dice = Dice(seed);
        let count = dice.below(7);
        let text: String = (0..count).map(|_| dice.pick(BIB_PIECES)).collect();
        let entries = bib::parse(&text);
        for entry in &entries {
            assert!(fits(&text, &entry.range), "seed {seed}: {entry:?}");
        }
        // Added: all that was there is still there, and the new one.
        let mut new = bib::Entry {
            key: "added2024".to_owned(),
            kind: "book".to_owned(),
            fields: Vec::new(),
            range: 0..0,
        };
        let value = dice.pick(&[
            "Plain",
            "a {brace",
            "close} first",
            "back\\",
            "\"quoted\"",
            "@x",
        ]);
        new.set("title", value);
        new.set("author", "Doe, Jane");
        let added = bib::with_added(&text, &new);
        let mut after = entries_but(&added, None);
        let last = after.pop();
        assert_eq!(
            after,
            entries_but(&text, None),
            "seed {seed}: adding to {text:?} gives {added:?}"
        );
        assert_eq!(
            last.map(|(key, _, _)| key).as_deref(),
            Some("added2024"),
            "seed {seed}: {added:?}"
        );
        // Each one changed or taken out: the others stay.
        let keys: Vec<&str> = entries.iter().map(|entry| entry.key.as_str()).collect();
        let unique = |key: &str| keys.iter().filter(|other| **other == key).count() == 1;
        for entry in entries.iter().filter(|entry| unique(&entry.key)) {
            let gone = bib::with_replaced(&text, &entry.range, None);
            assert_eq!(
                entries_but(&gone, None),
                entries_but(&text, Some(&entry.key)),
                "seed {seed}: taking {} out of {text:?} gives {gone:?}",
                entry.key
            );
            let mut changed = entry.clone();
            changed.set("note", value);
            let rewritten = bib::with_replaced(&text, &entry.range, Some(&changed));
            assert_eq!(
                entries_but(&rewritten, Some(&entry.key)),
                entries_but(&text, Some(&entry.key)),
                "seed {seed}: changing {} in {text:?} gives {rewritten:?}",
                entry.key
            );
        }
    }
}

#[test]
fn what_is_begun_and_not_closed_has_no_inside() {
    // `di$` on a dollar sign that ends the text took the editor down:
    // the equation is the sign alone, and its inside ran backwards.
    for text in [
        "Text $",
        "$$x^$",
        "Text *",
        "Text _",
        "$ x 漢",
        "*漢字",
        "_é\n\nNext",
    ] {
        let source = Source::detached(text);
        for at in places(text) {
            for name in ['$', '*', '_'] {
                if let Some(range) = typst_text::object(&source, name, at, false) {
                    assert!(fits(text, &range), "{name} at {at} of {text:?}: {range:?}");
                }
            }
        }
    }
    let source = Source::detached("A $$ b");
    assert_eq!(typst_text::object(&source, '$', 3, false), Some(3..3));
}

#[test]
fn a_table_with_a_cell_behind_its_brackets_is_not_changed_blindly() {
    // Rows were put behind the cell behind the brackets: outside the
    // call, as text of the page.
    let text = "#table(columns: 2, [a], [b])[c]";
    let source = Source::detached(text);
    assert!(typst_text::table_at(&source, text.find('a').unwrap()).is_none());
    assert!(
        typst_text::tab_key(&source, text.find('c').unwrap())
            .is_none_or(|(range, with, _)| { with.is_empty() && range.is_empty() })
    );
}

/// What sections are made of.
const SECTION_PIECES: &[&str] = &[
    "= One\n",
    "== Two <two>\n",
    "=== Three\n",
    "Words of a paragraph.\n",
    "\n",
    "$ x^2 $\n",
    "- item\n",
    "// = not a heading\n",
    "#figure([x], caption: [= no])\n",
    "é ü 漢字\n",
];

/// `text` with `edits` made, each referring to the text the ones before
/// it left.
fn with_edits(text: &str, edits: &[(Range<usize>, String)]) -> String {
    applied("section", text, edits)
}

#[test]
fn a_section_moved_and_moved_back_is_where_it_was() {
    for seed in 0..600 {
        let mut dice = Dice(seed);
        let count = 2 + dice.below(8);
        let mut text: String = (0..count).map(|_| dice.pick(SECTION_PIECES)).collect();
        if dice.below(2) == 0 {
            text.pop();
        }
        let source = Source::detached(text.clone());
        for (index, _) in sections::sections(&source).iter().enumerate() {
            for (way, back) in [
                (sections::Way::Up, sections::Way::Down),
                (sections::Way::Down, sections::Way::Up),
                (sections::Way::Deeper, sections::Way::Shallower),
            ] {
                let Ok(change) = sections::changed(&source, index, way) else {
                    continue;
                };
                let moved = with_edits(&text, &change.edits);
                let sorted = |text: &str| {
                    let mut chars: Vec<char> =
                        text.chars().filter(|c| !c.is_whitespace()).collect();
                    chars.sort_unstable();
                    chars
                };
                if way != sections::Way::Deeper {
                    assert_eq!(
                        sorted(&moved),
                        sorted(&text),
                        "seed {seed}: section {index} {way:?} in {text:?} gives {moved:?}"
                    );
                }
                let moved_source = Source::detached(moved.clone());
                let now = sections::at(&sections::sections(&moved_source), change.start)
                    .unwrap_or(usize::MAX);
                let Ok(again) = sections::changed(&moved_source, now, back) else {
                    panic!(
                        "seed {seed}: section {index} {way:?} in {text:?} gives {moved:?}, \
                         which does not go back {back:?} (as section {now})"
                    );
                };
                let returned = with_edits(&moved, &again.edits);
                assert_eq!(
                    returned.trim_end(),
                    text.trim_end(),
                    "seed {seed}: section {index} {way:?} and {back:?} in {text:?} via {moved:?}"
                );
            }
            if let Ok(change) = sections::removed(&source, index) {
                let gone = with_edits(&text, &change.edits);
                assert!(gone.len() <= text.len(), "seed {seed}: {gone:?}");
            }
        }
    }
}

/// What documents with figures and references to them are made of.
const FIGURE_PIECES: &[&str] = &[
    "#figure(image(\"a.png\"), caption: [A picture.]) <fig-a>\n\n",
    "#figure(table(columns: 1, [x]), caption: [See @fig-a.]) <tab-ü>\n\n",
    "#figure(rect[], caption: [None])\n\n",
    "#figure([x])  <fig-b>\n\n",
    "#table(columns: 1, [y])\n\n",
    "As @fig-a shows, and @tab-ü too, and @fig-b[Figure].\n\n",
    "#ref(<fig-a>) and <fig-b>\n\n",
    "Words.\n\n",
];

#[test]
fn a_figure_renamed_and_renamed_back_is_as_it_was() {
    for seed in 0..400 {
        let mut dice = Dice(seed);
        let count = 1 + dice.below(6);
        let text: String = (0..count).map(|_| dice.pick(FIGURE_PIECES)).collect();
        let source = Source::detached(text.clone());
        let figures = figure_text::figures(&source);
        for (index, figure) in figures.iter().enumerate() {
            assert!(fits(&text, &figure.whole), "seed {seed}: {figure:?}");
            let Some(old) = figure.label.clone() else {
                continue;
            };
            let edits = figure_text::relabelled(&source, figure, Some("renamed-é"), true);
            let renamed = applied("relabelled", &text, &edits);
            let renamed_source = Source::detached(renamed.clone());
            let again = figure_text::figures(&renamed_source);
            let Some(figure) = again.get(index) else {
                panic!("seed {seed}: no figure {index} in {renamed:?}");
            };
            assert_eq!(
                figure.label.as_deref(),
                Some("renamed-é"),
                "seed {seed}: {renamed:?}"
            );
            let edits = figure_text::relabelled(&renamed_source, figure, Some(&old), true);
            let back = applied("relabelled back", &renamed, &edits);
            // What referred to another figure of the same label, if two
            // have it, refers to this one now: only where each label is
            // the figure's own is the text the same again.
            let once = figures
                .iter()
                .filter(|other| other.label.as_ref() == Some(&old))
                .count();
            if once == 1 {
                assert_eq!(back, text, "seed {seed}: renamed {old} via {renamed:?}");
            }
        }
    }
}
