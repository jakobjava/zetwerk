//! Reports of what happened: written by one key from anywhere, and played
//! again to the same text.

use std::path::Path;
use std::time::Instant;

use super::typist::Dice;
use super::*;
use crate::viewer::report::Settings;
use crate::viewer::report::replay::{Recorded, Step, replay, replay_with};

/// The reports written under the state directory of `directory`.
fn reports(directory: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(directory.join("state/reports"))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

fn said(viewer: &Viewer) -> String {
    viewer
        .message
        .as_ref()
        .map_or_else(String::new, |message| message.text.clone())
}

/// Plays `report` again in a viewer, in a scratch directory beside
/// `directory`, and says how the texts differ from what they were.
fn replayed(report: &Path, directory: &Path) -> Vec<String> {
    let scratch = directory.with_extension("replay");
    let replayed = replay(report, &scratch).unwrap_or_else(|why| panic!("{why}"));
    let _ = std::fs::remove_dir_all(&scratch);
    replayed.differences()
}

const TEXT: &str = "= A heading\n\nThe first paragraph, with $x^2$ in it and a few more words.\n\n\
                    The second paragraph.\n\n- one\n- two\n";

#[test]
fn a_report_holds_what_happened_and_plays_again_to_the_same_text() {
    let (mut viewer, directory) = editing(TEXT);
    press(&mut viewer, "gsgs/first<CR>");
    press(&mut viewer, "ea not<Esc>");
    compile(&mut viewer);
    press(&mut viewer, "jjA And on.<Esc>dd");
    press(&mut viewer, "u/two<CR>");
    // A compile begun, keys typed while it runs, and then it lands (as
    // the window does it).
    viewer.report_began();
    let (revision, world) = viewer.snapshot();
    press(&mut viewer, "ciwthree<Esc>");
    let compiled = zetwerk_typst::compile(&world);
    viewer.compiled(CompileOutput {
        revision,
        world: Arc::new(world),
        compiled,
        duration: Duration::from_millis(40),
    });
    press(&mut viewer, "ggx");
    press(&mut viewer, ":report the x went<CR>");
    let told = said(&viewer);
    assert!(told.starts_with("report written to"), "{told}");
    // The first time, what a report holds and that it stays here.
    assert!(told.contains("never sent anywhere"), "{told}");
    let found = reports(&directory);
    assert_eq!(found.len(), 1);
    let report = &found[0];
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(report).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
    let read = |name: &str| std::fs::read_to_string(report.join(name)).unwrap();
    assert_eq!(read("note.txt"), "the x went\n");
    assert_eq!(read("before/main.typ"), TEXT);
    assert_eq!(read("after/main.typ"), viewer.text().unwrap());
    let keys = read("keys.txt");
    for wanted in [
        "key g ",
        "key <Esc>",
        "compiled r",
        "ok 40 ms",
        "· insert @",
        "report-key :",
        "report-key <CR>",
        "opens the command line",
    ] {
        assert!(keys.contains(wanted), "{wanted} in\n{keys}");
    }
    let state = read("state.txt");
    for wanted in [
        "stage.file: main.typ",
        // At the first key, the `g` of `gsgs`.
        "stage.layout: split",
        "Typst 0.",
        "pages: 1",
    ] {
        assert!(state.contains(wanted), "{wanted} in\n{state}");
    }
    assert!(!report.join("window.png").exists(), "no window, no picture");
    // The keys from the first, as --keys takes them, lead to the text.
    let recorded = Recorded::read(report).unwrap();
    assert_eq!(recorded.stage.file, "main.typ");
    assert!(
        recorded
            .steps
            .iter()
            .any(|step| matches!(step, Step::Key { .. }))
    );
    assert_eq!(replayed(report, &directory), Vec::<String>::new());
    // Said once; and nothing was typed into the text by it.
    press(&mut viewer, ":report<CR>");
    assert!(!said(&viewer).contains("never sent"), "{}", said(&viewer));
    assert_eq!(reports(&directory).len(), 2);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_report_key_works_while_typing_and_over_a_list() {
    let (mut viewer, directory) = editing(TEXT);
    press(&mut viewer, "gsgs/second<CR>iword <C-g>");
    // Still typing, and nothing typed by it.
    let editor = viewer.editor.as_ref().unwrap();
    assert_eq!(editor.mode(), editor::Mode::Insert);
    assert!(viewer.text().unwrap().contains("word second"));
    assert!(
        said(&viewer).starts_with("report written"),
        "{}",
        said(&viewer)
    );
    press(&mut viewer, "more<Esc>g?");
    assert!(viewer.picker.is_some());
    press(&mut viewer, "<C-g>");
    assert!(viewer.picker.is_some(), "the list stays");
    assert_eq!(reports(&directory).len(), 2);
    press(&mut viewer, "<Esc>:report<CR>");
    assert_eq!(reports(&directory).len(), 3);
    // Each plays again to what it was.
    for report in reports(&directory) {
        assert_eq!(replayed(&report, &directory), Vec::<String>::new());
    }
    let last = &reports(&directory)[2];
    let keys = std::fs::read_to_string(last.join("keys.txt")).unwrap();
    assert!(keys.contains("report-key <C-g>"), "{keys}");
    assert!(keys.contains("report-key :"), "{keys}");
    // The list of them, and deleting one.
    press(&mut viewer, ":reports<CR>");
    let listed = viewer.picker.as_ref().unwrap().listing().1.len();
    assert_eq!(listed, 4, "where they are, and each");
    press(&mut viewer, "<CR>");
    let name = last.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(
        viewer.editor.as_ref().unwrap().command_line(),
        Some(format!("reports delete {name}").as_str())
    );
    press(&mut viewer, "<CR>");
    assert_eq!(reports(&directory).len(), 2, "{}", said(&viewer));
    press(&mut viewer, ":reports delete ../state<CR>");
    assert!(said(&viewer).starts_with("there is no report"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_users_mappings_are_kept_as_typed_and_said() {
    let (mut viewer, directory) = editing(TEXT);
    let config = "[keys.normal]\n\"Q\" = \"ddp\"\n";
    let settings = Settings {
        config: Some(config.to_owned()),
        ..Settings::default()
    };
    viewer.report_configured(&settings, FontSet::Embedded);
    let (keymap, _) = Keymap::parse([("Q", "ddp")], []);
    viewer.set_keymap(keymap);
    press(&mut viewer, "Q:report<CR>");
    let report = &reports(&directory)[0];
    let keys = std::fs::read_to_string(report.join("keys.txt")).unwrap();
    assert!(
        keys.contains("key Q ") && keys.contains("mapped: the editor got ddp"),
        "{keys}"
    );
    // The replay maps it as the settings in force did.
    assert_eq!(replayed(report, &directory), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn text_changed_between_keys_is_put_in_where_it_was() {
    let (mut viewer, directory) = editing(TEXT);
    press(&mut viewer, "gsgsjjx");
    // As an agent does: no key.
    let _ = viewer.agent_insert("AGENT ", None, false);
    viewer.agent_called("zetwerk::bridge::Bridge::insert::{{closure}}");
    press(&mut viewer, "wx:report<CR>");
    let report = &reports(&directory)[0];
    let keys = std::fs::read_to_string(report.join("keys.txt")).unwrap();
    assert!(keys.contains("text-set"), "{keys}");
    assert!(keys.contains("agent insert"), "{keys}");
    assert_eq!(replayed(report, &directory), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn with_no_keys_kept_a_report_is_still_written() {
    let (mut viewer, directory) = editing(TEXT);
    let settings = Settings {
        keys: 0,
        ..Settings::default()
    };
    viewer.report_configured(&settings, FontSet::Embedded);
    press(&mut viewer, "x<C-g>");
    let report = &reports(&directory)[0];
    assert!(report.join("after/main.typ").exists());
    assert!(!report.join("before").exists());
    let _ = std::fs::remove_dir_all(directory);
}

/// For the random typist: a report written now plays again to the text
/// as it is, where what is kept begins at the first key (from a later
/// one, undo cannot reach what was before it).
pub(super) fn replays_the_same(viewer: &mut Viewer, case: &str) {
    if !viewer.report_from_first_key() {
        return;
    }
    let Some(directory) = viewer.state_dir.as_deref().and_then(Path::parent) else {
        return;
    };
    let directory = directory.to_owned();
    press(viewer, "<C-g>");
    let report = reports(&directory)
        .pop()
        .unwrap_or_else(|| panic!("no report: {}", said(viewer)));
    let differences = replayed(&report, &directory);
    assert!(
        differences.is_empty(),
        "{case}: the report does not play again to the same text: {differences:#?}"
    );
    let _ = std::fs::remove_dir_all(&report);
}

/// What a typist of prose does: words typed, and in normal mode moving,
/// deleting, yanking and putting (registers named too), `.`, searching
/// and `;`, but no undo (a replay that begins after the first key has
/// not the history before it) and nothing that keeps Typst from setting
/// the text, so that there are moments at rest to begin at.
fn prose(dice: &mut Dice, typing: bool) -> String {
    const WORDS: &[&str] = &[
        "the", "page", "word", "a", "of", "and", "naïve", "über", "Weyl",
    ];
    const NORMAL: &[&str] = &[
        "h",
        "j",
        "k",
        "l",
        "w",
        "b",
        "e",
        "0",
        "$",
        "x",
        "dw",
        "D",
        "cwnew<Esc>",
        "yy",
        "yw",
        "\"ayw",
        "\"ap",
        "p",
        "P",
        "J",
        "~",
        "rZ",
        "fe",
        ";",
        ",",
        "*",
        "n",
        "N",
        ".",
        "ma",
        "`a",
        "/the<CR>",
        "gg",
        "G",
        "dd",
        ">>",
    ];
    if typing {
        match dice.below(10) {
            0..=6 => format!("{} ", WORDS[dice.below(WORDS.len())]),
            7 => "<BS>".to_owned(),
            _ => "<Esc>".to_owned(),
        }
    } else {
        match dice.below(10) {
            0..=6 => NORMAL[dice.below(NORMAL.len())].to_owned(),
            _ => ["i", "a", "A", "o", "O"][dice.below(5)].to_owned(),
        }
    }
}

/// The documents of the random typist that a walk here goes over.
const PLAIN: &str = "#set page(width: 320pt, height: 3000pt, margin: 20pt)\n\n\
    = Spectral estimates\n\n\
    Let $A$ be a self-adjoint operator with eigenvalues $mu_1 <= mu_2$.\n\n\
    $ N(mu) = (omega_d |Omega|) / (2 pi)^d $\n\n\
    - first item with $x^2$\n- second item\n\n\
    #table(\n  columns: 2,\n  [a], [b],\n)\n\n\
    Last paragraph, with a few more words in it to fill a line.\n";

/// Walks the random typist with a ring of few keys, so that what is kept
/// moves on many times, pausing now and then as typing does; then a
/// report is written and played again: the keys from the first kept,
/// from the text kept there, give the text as it is.
/// Returns whether the replay was asked to be exact: where the first
/// key kept came while nothing was under way.
fn walk_and_replay(text: &str, seed: u64, steps: usize, keys: usize) -> bool {
    let (mut viewer, directory) = editing(text);
    viewer.report_configured(
        &Settings {
            keys,
            ..Settings::default()
        },
        FontSet::Embedded,
    );
    press(&mut viewer, "gsgs");
    let mut dice = Dice(seed);
    let mut trail = String::new();
    // The text after every key, to say where a replay goes otherwise.
    let mut texts: Vec<(KeyPress, String)> = Vec::new();
    for _ in 0..steps {
        let typing = viewer
            .editor
            .as_ref()
            .is_some_and(|editor| editor.mode() == editor::Mode::Insert);
        let keys = prose(&mut dice, typing);
        trail.push_str(&keys);
        for key in keys::parse(&keys).unwrap() {
            viewer.key(key);
            texts.push((key, viewer.text().unwrap_or_default().to_owned()));
        }
        if dice.chance(15) {
            compile(&mut viewer);
        }
    }
    for key in keys::parse("<Esc><Esc>").unwrap() {
        viewer.key(key);
        texts.push((key, viewer.text().unwrap_or_default().to_owned()));
    }
    press(&mut viewer, "<C-g>");
    let report = reports(&directory)
        .pop()
        .unwrap_or_else(|| panic!("no report: {}", said(&viewer)));
    let exact = Recorded::read(&report).unwrap().stage.rest;
    let differences = replayed(&report, &directory);
    if exact && !differences.is_empty() {
        // Where it went otherwise: the first key after which the texts
        // differ, counted from the last.
        let scratch = directory.with_extension("replay");
        let mut after = Vec::new();
        let _ = replay_with(&report, &scratch, |step, viewer| {
            if let Step::Key { press, .. } = step {
                after.push((*press, viewer.text().unwrap_or_default().to_owned()));
            }
        });
        let skip = texts.len().saturating_sub(after.len());
        let first = texts[skip..]
            .iter()
            .zip(&after)
            .position(|(was, is)| was != is);
        let context = first.map(|at| {
            let from = at.saturating_sub(6);
            let keys: String = after[from..=at]
                .iter()
                .map(|(press, _)| crate::viewer::report::notation(*press))
                .collect();
            let near = |text: &str| text.chars().take(400).collect::<String>();
            format!(
                "key {at} of {} replayed: {keys}\nreplayed: {:?}\nwas:      {:?}",
                after.len(),
                near(&after[at].1),
                near(&texts[skip + at].1)
            )
        });
        panic!(
            "seed {seed}, {keys} keys kept: {differences:#?}\n{context:?}\nkeys {trail:?}\nreport {}",
            report.display()
        );
    }
    let _ = std::fs::remove_dir_all(directory);
    exact
}

#[test]
fn the_keys_kept_from_the_text_kept_lead_to_the_text_now() {
    let prose = "= Spectral estimates\n\nLet the operator be self-adjoint, with a page of \
                 words about it and some more words.\n\n- first item\n- second item\n\n\
                 The last paragraph, with a few more words in it to fill a line or two.\n";
    let mut exact = 0;
    for seed in 0..4 {
        exact += usize::from(walk_and_replay(prose, 7000 + seed, 150, 40));
    }
    exact += usize::from(walk_and_replay("", 7100, 100, 25));
    // Most walks keep a start where nothing is under way.
    assert!(exact >= 3, "only {exact} replays were from a start at rest");
}

#[test]
#[ignore = "a long walk: run with --ignored"]
fn random_typist_replays() {
    let mut exact = 0;
    for seed in 0..20 {
        exact += usize::from(walk_and_replay(PLAIN, 8000 + seed, 300, 60));
    }
    println!("{exact} of 20 replays from a start at rest, all of them exact");
}

#[test]
fn recording_costs_little_beside_a_key() {
    let (mut viewer, directory) = editing(&TEXT.repeat(40));
    press(&mut viewer, "gsgs");
    let keys = keys::parse("lhjkwb").unwrap();
    // What a cheap key costs, the median of many.
    let mut took: Vec<f64> = (0..300)
        .map(|index| {
            let key = keys[index % keys.len()];
            let started = Instant::now();
            viewer.key(key);
            started.elapsed().as_secs_f64() * 1e6
        })
        .collect();
    took.sort_by(f64::total_cmp);
    let key = took[took.len() / 2];
    // What the recorder alone costs for a key, starts included (a copy of
    // the text every so many keys).
    let rounds = 5000;
    let started = Instant::now();
    for index in 0..rounds {
        let press = keys[index % keys.len()];
        viewer.report_key(press);
        viewer.report_reached(press);
        viewer.report_keyed();
    }
    let recording = started.elapsed().as_secs_f64() * 1e6 / rounds as f64;
    if std::env::var_os("ZETWERK_SPEED").is_some() {
        println!("recording: {recording:.2} µs a key; a cheap key: {key:.1} µs");
    }
    assert!(
        recording < key / 5.0 && recording < 50.0,
        "recording {recording:.2} µs a key, a cheap key {key:.1} µs"
    );
    let _ = std::fs::remove_dir_all(directory);
}
