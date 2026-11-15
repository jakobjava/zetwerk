//! A random typist on the page.
//!
//! A walk of keys, the same for the same seed, over example documents in
//! the layout that shows only pages: prose, equations, Enter, Backspace,
//! Delete, Tab, Escape, motions, operators, undo and redo, visual mode,
//! the `#` menu, lists, writes, and in a document of several files the
//! way from one to another. After every key the viewer has not panicked,
//! the cursor is on a character of the text, the text holds none of what
//! the editor puts in only to show it (the empty box, the cursor's mark),
//! the page shows no such mark of typing once typing is over, every file
//! without unwritten changes is on the disk as it is in the editor, and
//! what the page shows of a part typeset by itself is what all of it
//! typeset at once shows. Now and then typing pauses and all of it is
//! typeset; whether that worked, and if not why, is counted. In the end
//! all is written, and undoing every step in every file gives back the
//! text the walk began with.
//!
//! `a_random_typist_breaks_nothing` takes a few short walks with every
//! test run. `ZETWERK_TYPIST_SEEDS=n` walks `n` seeds instead of a few,
//! and `ZETWERK_TYPIST_KEYS=n` takes that many steps each:
//! `ZETWERK_TYPIST_SEEDS=40 cargo test --workspace random_typist -- --ignored --nocapture`

use std::collections::BTreeMap;

use super::*;

/// The paper of the corpus: headings, references, a footnote, lists.
const PAPER: &str = include_str!("../../../../zetwerk-corpus/docs/paper.typ");
/// The article of the corpus: justified, dense with equations, numbered.
const ARTICLE: &str = include_str!("../../../../zetwerk-corpus/docs/article.typ");

/// A document on one tall page with nothing numbered: what a part shows
/// typeset by itself has to be exactly what the whole shows.
const PLAIN: &str = "#set page(width: 320pt, height: 3000pt, margin: 20pt)\n\
    #set par(justify: true)\n\n\
    = Spectral estimates\n\n\
    Let $A$ be a self-adjoint operator on a Hilbert space $H$ with compact \
    resolvent, and denote its eigenvalues by $mu_1 <= mu_2 <= dots$, repeated \
    according to multiplicity.\n\n\
    $ N(mu) = (omega_d |Omega|) / (2 pi)^d mu^(d slash 2) + o(mu^(d slash 2)) $\n\n\
    where $omega_d$ is the volume of the unit ball. The leading term *depends* \
    on the domain only through its _volume_.\n\n\
    - first item with $x^2$\n- second item\n  - nested one\n\n\
    + counted\n+ again\n\n\
    #table(\n  columns: 2,\n  [a], [b],\n  [c], [d],\n)\n\n\
    Last paragraph, with a few more words in it to fill a line or two of the page.\n";

/// A document of three files: a main one that includes two chapters.
const CHAPTERS: &[(&str, &str)] = &[
    (
        "main.typ",
        "#set page(width: 300pt, height: 400pt)\n\
         = Thesis\n\n\
         Opening words with $x^2$ in them.\n\n\
         #include \"one.typ\"\n\
         #include \"parts/two.typ\"\n\n\
         Closing words.\n",
    ),
    (
        "one.typ",
        "== One\n\nFirst chapter text, with $a / b$ and *strong* words.\n",
    ),
    (
        "parts/two.typ",
        "== Two\n\nSecond chapter text.\n\n- an item\n- another\n",
    ),
];

/// The seeds of the walks, with a small generator of numbers.
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

    fn chance(&mut self, in_hundred: usize) -> bool {
        self.below(100) < in_hundred
    }
}

const WORDS: &[&str] = &[
    "the", "page", "word", "a", "of", "and", "spectral", "x", "Weyl", "é", "naïve", "über",
];
const MATH: &[&str] = &[
    "$", "^", "_", "/", "x", "2", "+", " ", "alpha", "pi", "ff<Tab>", "(", ")", "=", "sqrt",
    "<Tab>", "i", "n", "-", ",",
];
const NORMAL: &[&str] = &[
    "h", "j", "k", "l", "w", "b", "e", "0", "$", "x", "X", "dd", "dw", "D", "cw", "cc", "yy", "yw",
    "p", "P", "u", "u", "<C-r>", "J", "~", "rZ", "fe", "ta", ";", "<C-d>", "<C-u>", "G", "gg",
    ">>", "<lt><lt>", "*", "n", "%", "s", "S", "C", ".",
];
const INSERT_KEYS: &[&str] = &[
    "<CR>", "<BS>", "<BS>", "<Del>", "<Tab>", "<Left>", "<Right>", "<Up>", "<Down>", "<Home>",
    "<End>", "<C-b>", "<C-i>", "<C-w>",
];
const BEGIN_INSERT: &[&str] = &["i", "a", "A", "I", "o", "O"];
const VISUAL: &[&str] = &["v", "V"];
const VISUAL_OPS: &[&str] = &["d", "c", "y", "x", "<C-b>", "<Esc>", "~", "J", ">"];

/// One action of the typist: the keys it presses. `several`: the
/// document is of several files, which it goes between.
fn action(dice: &mut Dice, typing: bool, several: bool) -> String {
    if typing {
        match dice.below(100) {
            0..=34 => {
                let word = dice.pick(WORDS);
                if dice.chance(50) {
                    format!("{word} ")
                } else {
                    word.to_owned()
                }
            }
            35..=59 => dice.pick(MATH).to_owned(),
            60..=81 => dice.pick(INSERT_KEYS).to_owned(),
            82..=86 => dice.pick(&["- ", "+ ", "= ", "* ", ". ", ", "]).to_owned(),
            87..=90 => {
                // The menu of what can be put in, and what is done in it.
                let name = dice.pick(&["fig", "tab", "eq", "foot", "li", "x", "ref"]);
                let then = dice.pick(&["<CR>", "<Esc>", "<Tab>", "<Down><CR>", ""]);
                format!("#{name}{then}")
            }
            91..=93 => dice
                .pick(&["@", "<lt>", "\\", "`", "\"", "'", "[", "]", "#"])
                .to_owned(),
            _ => "<Esc>".to_owned(),
        }
    } else {
        match dice.below(100) {
            0..=44 => dice.pick(NORMAL).to_owned(),
            45..=74 => dice.pick(BEGIN_INSERT).to_owned(),
            75..=84 => {
                let motion = dice.pick(&["l", "w", "e", "j", "$", "b", "ll", "iw", "ip", "i$"]);
                let op = dice.pick(VISUAL_OPS);
                format!("{}{motion}{op}", dice.pick(VISUAL))
            }
            85..=89 => format!("{}u", dice.below(4) + 1),
            90..=94 => format!("{}<C-r>", dice.below(4) + 1),
            95 | 96 => ":w<CR>".to_owned(),
            97 | 98 if several => dice
                .pick(&[":bn<CR>", ":bp<CR>", "<C-^>", ":wa<CR>"])
                .to_owned(),
            _ => "<Esc>".to_owned(),
        }
    }
}

/// Where a glyph stands, to a tenth of a point, and which byte it shows.
type Placed = (i64, i64, Option<usize>);

/// Where every glyph of `frame` stands, in order of the page.
fn placed(world: &ProjectWorld, frame: &typst::layout::Frame) -> Vec<Placed> {
    let map = PageMap::build(world, frame);
    let tenth = |length: typst::layout::Abs| (length.to_pt() * 10.0).round() as i64;
    let mut all: Vec<Placed> = map
        .glyphs()
        .iter()
        .map(|glyph| {
            let byte = match &glyph.origin {
                zetwerk_typst::Origin::Text { range, .. }
                | zetwerk_typst::Origin::Space { range, .. }
                | zetwerk_typst::Origin::Math { range, .. }
                | zetwerk_typst::Origin::Object { range, .. } => Some(range.start),
                zetwerk_typst::Origin::Atomic(_) => None,
            };
            (tenth(glyph.rect.min.x), tenth(glyph.rect.min.y), byte)
        })
        .collect();
    all.sort_unstable();
    all
}

/// What became of a walk.
#[derive(Default)]
struct Tally {
    keys: usize,
    local: usize,
    /// Keys after which a part typeset by itself showed what the whole
    /// does not: by kind, with the first walk that showed it.
    differs: BTreeMap<String, (usize, String)>,
    /// Pauses after which the whole could not be typeset: by the first
    /// thing Typst said, with the first walk that showed it.
    broken: BTreeMap<String, (usize, String)>,
    /// So, but only as the file is written: without what the editor puts
    /// in to show half-typed equations. What a save leaves on the disk.
    broken_written: BTreeMap<String, (usize, String)>,
    pauses: usize,
    undo_failures: Vec<String>,
    worst_key_ms: f64,
    local_ms: f64,
}

impl Tally {
    fn note(
        map: &mut BTreeMap<String, (usize, String)>,
        kind: String,
        case: impl FnOnce() -> String,
    ) {
        map.entry(kind)
            .and_modify(|(count, _)| *count += 1)
            .or_insert_with(|| (1, case()));
    }
}

/// The message of the first error of `compiled`, without what is
/// particular to the place: what kind of failure it is.
fn first_error(world: &ProjectWorld, compiled: &zetwerk_typst::Compiled) -> Option<String> {
    if compiled.panicked {
        return Some("the compiler panicked".to_owned());
    }
    let error = compiled.errors.first()?;
    let message = error.message.to_string();
    // What is quoted differs from case to case.
    let kind: String = message
        .split('`')
        .enumerate()
        .map(|(index, part)| if index % 2 == 1 { "…" } else { part })
        .collect::<Vec<_>>()
        .join("`");
    let at = zetwerk_typst::place(world, error).and_then(|(file, range)| {
        let source = world.source(file).ok()?;
        let text = source.text();
        let from = text.floor_char_boundary(range.start.saturating_sub(30));
        let to = text.ceil_char_boundary((range.end + 30).min(text.len()));
        Some(text[from..to].to_owned())
    });
    Some(format!("{kind} — near {at:?}"))
}

/// The pages of the viewer as they are, compared with what all of the
/// text typeset at once makes of them.
fn compare_with_whole(viewer: &mut Viewer, tally: &mut Tally, case: &dyn Fn() -> String) {
    let Some(shown) = viewer.document.clone() else {
        return;
    };
    let Some(shown_world) = viewer.compiled_world.clone() else {
        return;
    };
    let (_, world) = snapshot_aside(viewer);
    let compiled = zetwerk_typst::compile(&world);
    let Some(whole) = compiled.document else {
        // A part shows a reference to what it does not hold by its name;
        // the whole that cannot be typeset is counted at the pause.
        return;
    };
    if whole.pages().len() != shown.pages().len() {
        // Where pages break is put right by the whole: expected.
        return;
    }
    let before = viewer.whole.clone();
    for (index, (page, full)) in shown.pages().iter().zip(whole.pages()).enumerate() {
        let local = placed(&shown_world, &page.frame);
        let right = placed(&world, &full.frame);
        if local == right {
            continue;
        }
        // A page that was not typeset anew: what moved onto it or off it
        // is where pages break, which the whole puts right.
        let untouched = before.as_ref().and_then(|before| before.pages().get(index));
        if untouched.is_some_and(|old| hash128(&old.frame) == hash128(&page.frame)) {
            continue;
        }
        let ends = (index > 0, index + 1 < whole.pages().len());
        let near = local.len() == right.len()
            && local
                .iter()
                .zip(&right)
                .all(|(a, b)| (a.0 - b.0).abs() <= 1 && (a.1 - b.1).abs() <= 1 && a.2 == b.2);
        let kind = if near {
            "glyphs stand a tenth of a point apart".to_owned()
        } else if breaks_elsewhere(&local, &right, ends) {
            "the page breaks elsewhere".to_owned()
        } else if local.len() == right.len() {
            let moved = local.iter().zip(&right).filter(|(a, b)| a != b).count();
            let numbers = local
                .iter()
                .zip(&right)
                .filter(|(a, b)| a != b)
                .all(|(a, b)| a.2.is_none() || b.2.is_none());
            if numbers {
                format!("only numbers differ ({moved} glyphs)")
            } else {
                "glyphs stand elsewhere".to_owned()
            }
        } else {
            "a different number of glyphs".to_owned()
        };
        let detail = || {
            let first = local
                .iter()
                .zip(&right)
                .position(|(a, b)| a != b)
                .unwrap_or(local.len().min(right.len()));
            format!(
                "{} on page {index}: {} glyphs here, {} in the whole; first difference at {first}: \
                 {:?} against {:?}",
                case(),
                local.len(),
                right.len(),
                local.get(first),
                right.get(first)
            )
        };
        Tally::note(&mut tally.differs, kind, detail);
    }
}

/// Whether the glyphs `here` (as [`placed`]) differ from those `there`
/// only as a page that breaks elsewhere does, which the whole puts right:
/// lines come or go at its top or foot, and all between them is the same,
/// moved up or down by as much. What comes or goes is of no place of the
/// text that the page shows elsewhere. `ends` says whether a page comes
/// before this one and whether one comes after it: lines come or go only
/// at an end that meets another page.
fn breaks_elsewhere(
    here: &[(i64, i64, Option<usize>)],
    there: &[(i64, i64, Option<usize>)],
    ends: (bool, bool),
) -> bool {
    use std::collections::{BTreeSet, HashMap};
    let mut shifts: HashMap<i64, usize> = HashMap::new();
    let wheres: HashMap<usize, i64> = there
        .iter()
        .filter_map(|(_, y, byte)| Some(((*byte)?, *y)))
        .collect();
    for (_, y, byte) in here {
        if let Some(there) = byte.and_then(|byte| wheres.get(&byte)) {
            *shifts.entry(y - there).or_default() += 1;
        }
    }
    let Some((&shift, _)) = shifts
        .iter()
        .max_by_key(|(shift, count)| (**count, -shift.abs()))
    else {
        return false;
    };
    let moved: BTreeSet<(i64, i64, Option<usize>)> = there
        .iter()
        .map(|(x, y, byte)| (*x, y + shift, *byte))
        .collect();
    let here_set: BTreeSet<(i64, i64, Option<usize>)> = here.iter().copied().collect();
    let same: Vec<_> = here_set.intersection(&moved).collect();
    let (Some(top), Some(foot)) = (
        same.iter().map(|glyph| glyph.1).min(),
        same.iter().map(|glyph| glyph.1).max(),
    ) else {
        return false;
    };
    let (gone, come): (Vec<_>, Vec<_>) = (
        here_set.difference(&moved).collect(),
        moved.difference(&here_set).collect(),
    );
    let outside = |glyph: &&(i64, i64, Option<usize>)| {
        (ends.0 && glyph.1 < top) || (ends.1 && glyph.1 > foot)
    };
    let bytes = |glyphs: &[&(i64, i64, Option<usize>)]| -> BTreeSet<usize> {
        glyphs.iter().filter_map(|glyph| glyph.2).collect()
    };
    gone.iter().all(outside) && come.iter().all(outside) && bytes(&gone).is_disjoint(&bytes(&come))
}

/// A snapshot of the text as it is, to compare with: what the pages on
/// screen were made with is left as it was.
fn snapshot_aside(viewer: &mut Viewer) -> (u64, ProjectWorld) {
    let shown = viewer.shown_put.clone();
    let snapshot = viewer.snapshot();
    viewer.shown_put = shown;
    snapshot
}

/// The page worked out from what was typed, if one is shown, compared
/// with what all of the text typeset at once makes of it: where every
/// glyph stands, to a tenth of a point.
fn compare_prediction(viewer: &mut Viewer, tally: &mut Tally, case: &dyn Fn() -> String) {
    let Some((index, page)) =
        (viewer.predicted.as_ref()).map(|shown| (shown.index, shown.page.clone()))
    else {
        return;
    };
    let Some(shown_world) = viewer.compiled_world.clone() else {
        return;
    };
    let (_, world) = snapshot_aside(viewer);
    let Some(whole) = zetwerk_typst::compile(&world).document else {
        return;
    };
    let Some(full) = whole.pages().get(index) else {
        return;
    };
    // By lines, then along them.
    let places = |world: &ProjectWorld, frame| {
        let mut all: Vec<(i64, i64)> = placed(world, frame)
            .into_iter()
            .map(|(x, y, _)| (y, x))
            .collect();
        all.sort_unstable();
        all
    };
    let (here, there) = (
        places(&shown_world, &page.frame),
        places(&world, &full.frame),
    );
    if here == there {
        return;
    }
    let kind = if here.len() == there.len() {
        let worst = here
            .iter()
            .zip(&there)
            .map(|(a, b)| (a.0 - b.0).abs().max((a.1 - b.1).abs()))
            .max()
            .unwrap_or(0);
        format!(
            "predicted glyphs stand elsewhere (by {} tenths of a point or less)",
            if worst <= 5 {
                5
            } else {
                worst.min(1000) / 10 * 10 + 10
            }
        )
    } else {
        "the prediction shows a different number of glyphs".to_owned()
    };
    let detail = || {
        let first = here
            .iter()
            .zip(&there)
            .position(|(a, b)| a != b)
            .unwrap_or(here.len().min(there.len()));
        format!(
            "{} on page {index}: {} glyphs here, {} in the whole; first difference at {first}: \
             {:?} against {:?}",
            case(),
            here.len(),
            there.len(),
            here.get(first),
            there.get(first)
        )
    };
    Tally::note(&mut tally.differs, kind, detail);
}

/// Typesets all of it, as when typing pauses, and counts what failed.
fn pause(viewer: &mut Viewer, tally: &mut Tally, case: &dyn Fn() -> String) {
    tally.pauses += 1;
    let (revision, world) = viewer.snapshot();
    let compiled = zetwerk_typst::compile(&world);
    if let Some(kind) = first_error(&world, &compiled) {
        Tally::note(&mut tally.broken, kind, case);
    } else {
        // The text as it is written, as it would be saved.
        let written = viewer.project.snapshot();
        let plain = zetwerk_typst::compile(&written);
        if let Some(kind) = first_error(&written, &plain) {
            Tally::note(&mut tally.broken_written, kind, case);
        }
    }
    viewer.compiled(CompileOutput {
        revision,
        world: Arc::new(world),
        compiled,
        duration: Duration::from_millis(3),
    });
}

/// Whether the viewer's text and cursor are as they must be, and its
/// pages, if `settled`: if nothing is waiting to be typeset.
fn sound(viewer: &Viewer, settled: bool) -> Result<(), String> {
    let editor = viewer.editor.as_ref().ok_or("no editor")?;
    let text = editor.text();
    let cursor = editor.cursor();
    if cursor > text.len() || !text.is_char_boundary(cursor) {
        return Err(format!("the cursor is at {cursor}, off the text"));
    }
    if let Some(selection) = editor.selection()
        && (selection.end > text.len()
            || !text.is_char_boundary(selection.start)
            || !text.is_char_boundary(selection.end))
    {
        return Err(format!("the selection {selection:?} is off the text"));
    }
    if viewer.shown_panes() != Panes::Page {
        return Err("the page is no longer all that is shown".to_owned());
    }
    // What is put in only to be shown is never in the text: it would be
    // written, and the page shows it as if it were nothing.
    for mark in [EMPTY, CURSOR_MARK] {
        if text.contains(mark) {
            return Err(format!("the text holds the editor's mark {mark:?}"));
        }
    }
    // Typing over, and nothing more to typeset, the page shows no mark
    // of it: neither where the cursor was nor the quotes around a word
    // being typed.
    if settled && editor.mode() != editor::Mode::Insert {
        let left: Vec<String> = marks_on_page(viewer)
            .into_iter()
            .filter(|mark| !mark.replace(EMPTY, "").is_empty())
            .collect();
        if !left.is_empty() {
            return Err(format!("typing is over and the page shows {left:?}"));
        }
    }
    // A file without unwritten changes is on the disk as it is here.
    for open in viewer.editor.iter().chain(&viewer.others) {
        if !open.is_modified() {
            let on_disk = std::fs::read_to_string(open.path()).map_err(|err| err.to_string())?;
            if on_disk != open.text() {
                return Err(format!(
                    "{} is said to be written, and the disk has {}",
                    open.path().display(),
                    first_difference(open.text(), &on_disk)
                ));
            }
        }
    }
    Ok(())
}

/// What was put into the text of the file being edited to typeset the
/// pages on screen, piece by piece; nothing where the pages are not all
/// of the text typeset at once (a part typeset by itself is cut from
/// it).
fn marks_on_page(viewer: &Viewer) -> Vec<String> {
    let (Some(editor), Some(world)) = (viewer.editor.as_ref(), viewer.compiled_world.as_ref())
    else {
        return Vec::new();
    };
    let whole = matches!((&viewer.document, &viewer.whole),
        (Some(document), Some(whole)) if Arc::ptr_eq(document, whole));
    let file = editor.file();
    let (Ok(shown), Ok(written)) = (world.source(file), world.written(file)) else {
        return Vec::new();
    };
    if !whole || !world.shows_more() {
        return Vec::new();
    }
    // Byte `w` of what is written is given to Typst behind all that was
    // put in up to it, at it included.
    let (shown, written) = (shown.text(), written.text());
    let mut is_written = vec![false; shown.len()];
    for at in 0..written.len() {
        if let Some(slot) = is_written.get_mut(world.shown_at(file, at + 1) - 1) {
            *slot = true;
        }
    }
    let mut pieces = Vec::new();
    let mut piece = String::new();
    for (at, c) in shown.char_indices() {
        if is_written[at] {
            if !piece.is_empty() {
                pieces.push(std::mem::take(&mut piece));
            }
        } else {
            piece.push(c);
        }
    }
    if !piece.is_empty() {
        pieces.push(piece);
    }
    pieces
}

/// One walk of `steps` actions with `seed` over the document of `files`
/// (names and texts; the first is the main file).
fn walk(files: &[(&str, &str)], seed: u64, steps: usize, tally: &mut Tally) {
    let text = files.first().map_or("", |(_, text)| *text);
    let directory = directory(text);
    for (name, text) in files.iter().skip(1) {
        let path = directory.join(name);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(path, text).unwrap_or_else(|err| panic!("{name}: {err}"));
    }
    let (mut viewer, _) = open(&directory);
    let several = files.len() > 1;
    press(&mut viewer, "gsgs");
    let mut dice = Dice(seed);
    // Somewhere in the text behind its rules.
    let body = text.find("\n\n").unwrap_or(0);
    let mut start = body + dice.below(text.len() - body);
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    if let Some(editor) = viewer.editor.as_mut() {
        editor.set_cursor(start);
    }
    let mut trail = String::new();
    // Whether the viewer has asked for all of it to be typeset since it
    // last was (in the program, that is done when typing pauses).
    let mut asked = false;
    for _ in 0..steps {
        let typing = viewer
            .editor
            .as_ref()
            .is_some_and(|editor| editor.mode() == editor::Mode::Insert);
        let keys = action(&mut dice, typing, several);
        trail.push_str(&keys);
        for key in keys::parse(&keys).unwrap_or_else(|err| panic!("{keys}: {err}")) {
            let started = std::time::Instant::now();
            let local = viewer.stats.local;
            viewer.key(key);
            let took = started.elapsed().as_secs_f64() * 1000.0;
            asked |= viewer.take_edited().is_some();
            tally.keys += 1;
            tally.worst_key_ms = tally.worst_key_ms.max(took);
            // What is drawn of it: the cursor, what is typed in place,
            // the numbers of the lines.
            if tally.keys.is_multiple_of(3) {
                let _ = viewer.scene(&|_| true);
            }
            let case = || {
                format!(
                    "seed {seed} at {start}: {trail:?}, in {} near the cursor {:?}",
                    viewer.state().file,
                    near_cursor(&viewer)
                )
            };
            if let Err(why) = sound(&viewer, !asked) {
                panic!("{why}: {}", case());
            }
            if !compiled_is_whole(&viewer) {
                Tally::note(
                    &mut tally.differs,
                    "said to be typeset whole while a part is put in".to_owned(),
                    case,
                );
            }
            let case = case();
            if viewer.stats.local > local {
                tally.local += 1;
                tally.local_ms += viewer.stats.local_ms;
                compare_with_whole(&mut viewer, tally, &|| case.clone());
            }
            if viewer.is_predicted() {
                compare_prediction(&mut viewer, tally, &|| case.clone());
            }
        }
        if dice.chance(15) {
            let case = format!("seed {seed} at {start}: {trail:?}");
            pause(&mut viewer, tally, &|| case.clone());
            asked = false;
        }
    }
    // All of it written: on the disk as in the editor.
    press(&mut viewer, "<Esc><Esc>:wa<CR>");
    if let Err(why) = sound(&viewer, false) {
        panic!("{why}: seed {seed} at {start}: {trail:?}, and :wa");
    }
    // Every file undone to where it began.
    let names: Vec<String> = viewer
        .buffers()
        .into_iter()
        .map(|(name, ..)| name)
        .collect();
    for (index, name) in names.iter().enumerate() {
        press(&mut viewer, &format!(":b {}<CR>", index + 1));
        let original = files
            .iter()
            .find(|(file, _)| file == name)
            .map_or("", |(_, text)| *text);
        if let Some(wrong) = undo_all(&mut viewer, original) {
            tally.undo_failures.push(format!(
                "seed {seed} at {start}: {trail:?}: {name}: {wrong}"
            ));
        }
    }
    let _ = std::fs::remove_dir_all(directory);
}

/// Undoes every step there is, by `u`; what is wrong then, if anything:
/// a `u` that does nothing short of the first state, or a text that is
/// not the one the walk began with.
fn undo_all(viewer: &mut Viewer, original: &str) -> Option<String> {
    press(viewer, "<Esc><Esc>");
    let state = |viewer: &Viewer| viewer.editor.as_ref().map(|e| e.history().current());
    let mut stuck = String::new();
    for _ in 0..10_000 {
        if state(viewer) == Some(zetwerk_core::ROOT) {
            break;
        }
        let before = (state(viewer), viewer.text().map(str::to_owned));
        press(viewer, "u");
        if (state(viewer), viewer.text().map(str::to_owned)) == before {
            stuck = format!(
                "u does nothing in state {:?} (mode {:?}, a list up: {}); ",
                before.0,
                viewer.state().editing.map(|editing| editing.mode),
                viewer.is_picking()
            );
            break;
        }
    }
    let text = viewer.text().unwrap_or_default();
    let first = viewer
        .editor
        .as_ref()
        .and_then(|editor| editor.text_at(zetwerk_core::ROOT));
    (text != original || !stuck.is_empty()).then(|| {
        format!(
            "{stuck}undone leaves {}; the first state of the history is the text: {:?}",
            first_difference(original, text),
            first.map(|first| first == original)
        )
    })
}

/// The text around the cursor.
fn near_cursor(viewer: &Viewer) -> String {
    let (Some(text), Some(editor)) = (viewer.text(), viewer.editor.as_ref()) else {
        return String::new();
    };
    let cursor = editor.cursor();
    let from = text.floor_char_boundary(cursor.saturating_sub(25));
    let to = text.ceil_char_boundary((cursor + 25).min(text.len()));
    format!("{}‸{}", &text[from..cursor], &text[cursor..to])
}

/// Where `a` and `b` part, shown.
fn first_difference(a: &str, b: &str) -> String {
    let at = a
        .char_indices()
        .zip(b.chars())
        .find(|((_, x), y)| x != y)
        .map_or(a.len().min(b.len()), |((at, _), _)| at);
    let at = a.floor_char_boundary(at);
    let from = a.floor_char_boundary(at.saturating_sub(20));
    let shown = |text: &str| {
        let to = text.ceil_char_boundary((at + 30).min(text.len()));
        text.get(from..to).unwrap_or("").to_owned()
    };
    format!("{:?} where it was {:?}", shown(b), shown(a))
}

fn report(name: &str, tally: &Tally) {
    println!(
        "{name}: {} keys, {} typeset in parts ({:.1} ms each), worst key {:.1} ms, {} pauses",
        tally.keys,
        tally.local,
        tally.local_ms / tally.local.max(1) as f64,
        tally.worst_key_ms,
        tally.pauses
    );
    for (title, map) in [
        ("part and whole differ", &tally.differs),
        ("the whole cannot be typeset", &tally.broken),
        ("the written text cannot be typeset", &tally.broken_written),
    ] {
        for (kind, (count, case)) in map {
            println!("  {title}: {count}× {kind}\n      first: {case}");
        }
    }
    for failure in &tally.undo_failures {
        println!("  undo: {failure}");
    }
}

fn number_from_env(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

/// Walks every document with `seeds` seeds of `steps` actions, and
/// asks what holds of every walk: undoing gives back what was there;
/// in the documents of one file, a part shows what the whole shows,
/// glyph for glyph; and nowhere are pages a part was put into said to
/// be all of it typeset.
fn walks(seeds: usize, steps: usize) {
    let documents: [(&str, &[(&str, &str)]); 5] = [
        ("empty", &[("main.typ", "")]),
        ("plain", &[("main.typ", PLAIN)]),
        ("article", &[("main.typ", ARTICLE)]),
        ("paper", &[("main.typ", PAPER)]),
        ("chapters", CHAPTERS),
    ];
    let mut wrong = Vec::new();
    for (index, (name, files)) in documents.into_iter().enumerate() {
        let mut tally = Tally::default();
        // Other keys on each document.
        for seed in (0..seeds as u64).map(|seed| seed + 1000 * index as u64) {
            walk(files, seed, steps, &mut tally);
        }
        report(name, &tally);
        wrong.extend(
            tally
                .undo_failures
                .iter()
                .map(|failure| format!("{name}: {failure}")),
        );
        for (kind, (count, case)) in &tally.differs {
            // What a part shows has to be what the whole shows; what is
            // worked out on a line, and where pages break, is only told
            // of (see `report`).
            let part = !kind.contains("predict")
                && !kind.contains("breaks elsewhere")
                && !kind.contains("a tenth of a point apart");
            let strict = name != "chapters" && part;
            if strict || kind.starts_with("said to be") {
                wrong.push(format!("{name}: {count}× {kind}; first: {case}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn a_random_typist_breaks_nothing() {
    walks(2, 60);
}

#[test]
#[ignore = "a long walk: run with --ignored"]
fn random_typist() {
    walks(
        number_from_env("ZETWERK_TYPIST_SEEDS", 3),
        number_from_env("ZETWERK_TYPIST_KEYS", 150),
    );
}

#[test]
fn an_equation_not_closed_at_the_end_of_the_text_is_no_crash() {
    // `di$` in an equation that runs to the end of the text, which ends
    // in a character of more than one byte: its inside was cut through
    // that character, and the window went down.
    for (text, keys) in [
        ("Words $ x 漢", "$di$"),
        ("Words $", "$di$"),
        ("A *漢", "$di*"),
    ] {
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, keys);
        assert!(sound_text(&viewer), "{text:?} {keys}: {:?}", viewer.text());
        press(&mut viewer, "u");
        assert_eq!(viewer.text(), Some(text));
        let _ = std::fs::remove_dir_all(directory);
    }
}

/// Whether the cursor is on a character of the text.
fn sound_text(viewer: &Viewer) -> bool {
    let (Some(text), Some(editor)) = (viewer.text(), viewer.editor.as_ref()) else {
        return false;
    };
    text.is_char_boundary(editor.cursor())
}

#[test]
fn undoing_on_the_page_gives_back_what_was_there() {
    // A deletion on the page takes from the equation what raises or
    // lowers nothing; an undo that deletes is no deletion, and what it
    // brings back is to be what was there, to be redone after.
    let text = "Some $x^$ here, and more words.\n";
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "gsgs");
    if let Some(editor) = viewer.editor.as_mut() {
        editor.set_cursor(text.find('x').unwrap_or(0));
    }
    press(&mut viewer, "ay<Esc>");
    let typed = viewer.text().unwrap_or_default().to_owned();
    assert_eq!(typed, "Some $xy^$ here, and more words.\n");
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some(text));
    press(&mut viewer, "<C-r>");
    assert_eq!(viewer.text(), Some(typed.as_str()));
    let _ = std::fs::remove_dir_all(directory);
}

/// Whether the pages the viewer says are those of all of the text
/// typeset at once are: not a page a part was put into.
fn compiled_is_whole(viewer: &Viewer) -> bool {
    // Where the text cannot be typeset, the pages are the last that
    // could be, whatever they are made of.
    !viewer.is_compiled()
        || !viewer.problems.is_empty()
        || match (&viewer.document, &viewer.whole) {
            (Some(document), Some(whole)) => Arc::ptr_eq(document, whole),
            (None, _) => true,
            (Some(_), None) => false,
        }
}

#[test]
fn coming_back_to_the_text_of_a_part_is_not_having_typeset_all_of_it() {
    // A part is typeset by itself; a letter is typed and taken back: the
    // text is that of the part again. The pages are still those with a
    // part put in (numbers and page breaks not yet right), and all of
    // it is still to be typeset.
    let text = "#set page(width: 300pt, height: 400pt)\n\
                #set heading(numbering: \"1.\")\n\
                = One\n\nFirst paragraph here.\n\n= Two\n\nSecond paragraph here.\n";
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "/First<CR>gsgsA<CR>= Mid");
    assert_eq!(viewer.document_revision, Some(viewer.revision));
    assert!(!viewer.is_compiled());
    press(&mut viewer, "<Esc>/Second<CR>ix<BS>");
    assert!(
        compiled_is_whole(&viewer),
        "the viewer says all of it was typeset; the page has a part put in"
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_mark_of_the_cursor_goes_from_the_page_when_typing_is_over() {
    // A line opened, all of it typeset while typing (the cursor's mark
    // holds the empty line open), then Escape: the text is typeset as
    // it is, or the empty line stays on the page and the mark with it.
    // The typist found it: seed 3000, `yyo <Esc>` in the paper.
    let text = "Words of the first paragraph.\n\nSecond.\n";
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "gsgso");
    let _ = viewer.take_edited();
    compile(&mut viewer);
    assert!(viewer.is_compiled());
    assert_eq!(marks_on_page(&viewer), [CURSOR_MARK]);
    press(&mut viewer, "<Esc>");
    assert_eq!(viewer.take_edited(), Some(Edited::LeftInsert));
    compile(&mut viewer);
    assert_eq!(marks_on_page(&viewer), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_compile_of_an_older_text_does_not_take_back_what_a_part_shows() {
    // Typing pauses, and all of it is being typeset (the snapshot). The
    // typing goes on, and each key is typeset in part and shown. Then
    // the compile of the older text is done: the page goes back to the
    // text of the pause, and what was typed since is gone from it until
    // the next pause has been typeset. On a thesis, a third of a second
    // in which letters vanish and come back.
    let text = "#set page(width: 300pt, height: 400pt)\n\
                = Title\n\nFirst paragraph with $x^2$ in it.\n\nLast.\n";
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "/x<CR>gsgsa");
    let (revision, world) = viewer.snapshot();
    press(&mut viewer, "_i");
    let shown = viewer.document_revision;
    assert_eq!(shown, Some(viewer.revision), "{:?}", viewer.declined);
    let compiled = zetwerk_typst::compile(&world);
    viewer.compiled(CompileOutput {
        revision,
        world: Arc::new(world),
        compiled,
        duration: Duration::from_millis(3),
    });
    assert_eq!(
        viewer.document_revision, shown,
        "the page went back from the text as it is to an older one"
    );
    assert!(!viewer.is_compiled());
    // The text as it is, typeset whole, takes the place of the part.
    compile(&mut viewer);
    assert!(viewer.is_compiled() && compiled_is_whole(&viewer));
    let _ = std::fs::remove_dir_all(directory);
}
