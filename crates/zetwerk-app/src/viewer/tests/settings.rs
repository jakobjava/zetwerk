//! The settings as a user meets them: every option set and read back,
//! `:set` in its forms, the panel, what each set of aids draws, what is
//! kept in the settings file, and the three behaviours of typing that
//! are the user's to choose. No test reads or writes the user's own
//! settings: what would be written is recorded.

use std::sync::{Arc, Mutex};

use super::*;
use crate::settings::{Edit, Kind, Preset, Stored, Value, Written, table};

/// The message the viewer shows, if any.
fn said(viewer: &Viewer) -> String {
    viewer
        .message
        .as_ref()
        .map_or_else(String::new, |message| message.text.clone())
}

/// Settings as a file with `explicit` in it says them, and what they
/// write, recorded.
fn stored(explicit: &[(&'static str, Value)]) -> (Stored, Arc<Mutex<Vec<Edit>>>) {
    let edits = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&edits);
    let stored = Stored {
        explicit: explicit.to_vec(),
        writer: Some(Arc::new(move |new: &[Edit]| {
            record.lock().unwrap().extend_from_slice(new);
            Ok(())
        }) as crate::settings::Writer),
        file: Some(PathBuf::from("/nowhere/config.toml")),
        ..Stored::default()
    };
    (stored, edits)
}

/// A viewer editing `text`, configured as the window is with `explicit`
/// in the settings file; and what it writes there.
fn configured(
    text: &str,
    explicit: &[(&'static str, Value)],
) -> (Viewer, PathBuf, Arc<Mutex<Vec<Edit>>>) {
    let (mut viewer, directory) = editing(text);
    let (stored, edits) = stored(explicit);
    viewer.configure_settings(&stored, false);
    press(&mut viewer, "gsgs");
    (viewer, directory, edits)
}

/// The value of option `name`.
fn value(viewer: &Viewer, name: &str) -> Value {
    let opt = crate::settings::find(name).unwrap();
    (opt.get)(viewer)
}

/// What was written last of `name` (`aids.numbers`): `Some(None)` where
/// it was taken out.
#[allow(clippy::option_option, reason = "taken out is not never written")]
fn last_written(edits: &Mutex<Vec<Edit>>, name: &str) -> Option<Option<Written>> {
    let path: Vec<String> = name.split('.').map(str::to_owned).collect();
    let edits = edits.lock().unwrap();
    edits
        .iter()
        .rev()
        .find(|edit| edit.path == path)
        .map(|edit| edit.value.clone())
}

const TEXT: &str = "= A heading <start>\n\
                    One line of text that is long enough to wrap, perhaps.\n\n\
                    Two lines of text.\n\n\
                    Three: $x + y$ and more words here.\n\n\
                    Four lines.\n";

#[test]
fn every_option_can_be_set_and_read_back_and_is_listed() {
    let (mut viewer, directory, _) = configured(TEXT, &[]);
    for opt in table() {
        let values: Vec<String> = match opt.kind {
            Kind::Switch => vec!["on".into(), "off".into()],
            Kind::Choice(_) | Kind::Maybe => {
                opt.choices().iter().map(|c| (*c).to_owned()).collect()
            }
            Kind::Number { least, most, .. } => vec![least.to_string(), most.to_string()],
            Kind::Text => vec!["nl".into(), String::new()],
            Kind::List => vec!["/one, /two".into(), String::new()],
        };
        for typed in values {
            let wanted = opt.parse(&typed).unwrap();
            let typed = match opt.kind {
                Kind::Choice(_) | Kind::Maybe => typed.replace(' ', "\\ "),
                _ => typed,
            };
            let command = format!(":set {}={typed}<CR>", opt.name);
            press(&mut viewer, &command);
            assert_eq!(
                value(&viewer, opt.name),
                wanted,
                "{command}: {}",
                said(&viewer)
            );
            // The panel shows it.
            press(&mut viewer, &format!(":set panel {}<CR>", opt.name));
            let markup = viewer.settings_markup();
            assert!(
                markup.contains(&format!("\"{}\"", wanted.shown())),
                "{}: {markup}",
                opt.name
            );
            press(&mut viewer, "<Esc>");
        }
        // And back to what it usually is.
        press(&mut viewer, &format!(":set {}&<CR>", opt.name));
    }
    // The usual of each is something it can be.
    for opt in table() {
        for preset in Preset::ALL {
            let usual = opt.usual_for(preset);
            let typed = usual.shown();
            let typed = if typed == "(none)" {
                String::new()
            } else {
                typed
            };
            assert_eq!(opt.parse(&typed), Ok(usual), "{}", opt.name);
        }
        assert!(!opt.about.is_empty() && opt.name.contains('.'));
    }
    let _ = std::fs::remove_dir_all(directory);
}

/// Commands that turn rules of the document on or off: the document
/// keeps them, not the settings (`gD`).
const DOCUMENT_RULES: &[&str] = &["justify", "indent", "numbering", "pagenumbers"];

#[test]
fn every_command_that_turns_something_on_or_off_is_an_option() {
    let (mut viewer, directory, _) = configured(TEXT, &[]);
    viewer.open_palette();
    let labels: Vec<String> = viewer
        .picker
        .as_ref()
        .map(|picker| {
            picker
                .listing()
                .1
                .iter()
                .map(|label| (*label).to_owned())
                .collect()
        })
        .unwrap_or_default();
    viewer.picker = None;
    let commands: Vec<Option<&str>> = palette::entries().map(|(_, command)| command).collect();
    assert_eq!(labels.len(), commands.len());
    for (label, command) in labels.iter().zip(commands) {
        let Some(command) = command else {
            continue;
        };
        let name = command.split(' ').next().unwrap_or_default();
        let what = label.split('\t').next().unwrap_or_default();
        let toggles =
            what.ends_with("on or off") || what.ends_with("or not") || what.ends_with("or do not");
        if toggles && !DOCUMENT_RULES.contains(&name) {
            assert!(
                table()
                    .iter()
                    .any(|opt| opt.names.contains(&name) || opt.session.contains(&name)),
                ":{name} turns something on or off ({what}), but no option of the table \
                 (viewer/options.rs) names it"
            );
        }
    }
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn set_takes_the_forms_vim_users_know() {
    let (mut viewer, directory, edits) = configured(TEXT, &[]);
    assert_eq!(value(&viewer, "aids.numbers"), Value::Text("normal".into()));
    press(&mut viewer, ":set nonumbers<CR>");
    assert_eq!(value(&viewer, "aids.numbers"), Value::Text("off".into()));
    assert!(said(&viewer).contains("numbers: off"), "{}", said(&viewer));
    assert_eq!(
        last_written(&edits, "aids.numbers"),
        Some(Some(Written::Text("off".into())))
    );
    press(&mut viewer, ":set numbers!<CR>");
    assert_eq!(value(&viewer, "aids.numbers"), Value::Text("normal".into()));
    // As usual again: taken out of the file.
    assert_eq!(last_written(&edits, "aids.numbers"), Some(None));
    press(&mut viewer, ":set numbers?<CR>");
    assert_eq!(said(&viewer), "numbers=normal, as usual");
    press(&mut viewer, ":set pause=300 hlsearch nospell<CR>");
    assert_eq!(value(&viewer, "aids.pause"), Value::Number(300.0));
    assert_eq!(viewer.settings.pause, std::time::Duration::from_millis(300));
    press(&mut viewer, ":set pause+=100<CR>");
    assert_eq!(value(&viewer, "aids.pause"), Value::Number(400.0));
    assert_eq!(
        last_written(&edits, "aids.pause"),
        Some(Some(Written::Integer(400)))
    );
    press(&mut viewer, ":set pause&<CR>");
    assert_eq!(value(&viewer, "aids.pause"), Value::Number(1500.0));
    press(&mut viewer, ":set pause=lots<CR>");
    assert!(
        viewer.message.as_ref().is_some_and(|m| m.alert),
        "{}",
        said(&viewer)
    );
    press(&mut viewer, ":set nonsense<CR>");
    assert!(said(&viewer).starts_with("no option nonsense"));
    // A choice by as much of it as tells it apart; a text with blanks.
    press(&mut viewer, ":set ghost=w<CR>");
    assert_eq!(value(&viewer, "aids.ghost"), Value::Text("words".into()));
    press(&mut viewer, ":set ai.save=not\\ said<CR>");
    assert_eq!(value(&viewer, "ai.save"), Value::Text("not said".into()));
    assert!(
        said(&viewer).contains("as usual, from the next start"),
        "{}",
        said(&viewer)
    );
    press(&mut viewer, ":set languagetool=http://localhost:8081<CR>");
    assert!(
        viewer
            .language
            .settings
            .languagetool
            .as_ref()
            .is_some_and(|server| server.url == "http://localhost:8081" && !server.remote)
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_commands_of_old_go_through_the_settings() {
    let (mut viewer, directory, edits) = configured(TEXT, &[]);
    press(&mut viewer, ":tags<CR>");
    assert_eq!(value(&viewer, "aids.tags"), Value::Switch(true));
    assert_eq!(
        last_written(&edits, "aids.tags"),
        Some(Some(Written::Bool(true)))
    );
    // The command's own words are kept.
    assert!(
        said(&viewer).contains("tag beside the page"),
        "{}",
        said(&viewer)
    );
    press(&mut viewer, ":tags off<CR>");
    assert_eq!(last_written(&edits, "aids.tags"), Some(None));
    press(&mut viewer, ":ui 1.5<CR>");
    assert_eq!(
        last_written(&edits, "view.ui_scale"),
        Some(Some(Written::Float(1.5)))
    );
    // What changes something for now only is not kept.
    press(&mut viewer, ":slides<CR>");
    assert_eq!(last_written(&edits, "view.slides"), None);
    press(&mut viewer, ":slides<CR>gs");
    assert_eq!(last_written(&edits, "view.layout"), None);
    press(&mut viewer, "gsgs:agents propose<CR>");
    assert_eq!(
        last_written(&edits, "ai.edit_mode"),
        Some(Some(Written::Text("propose".into())))
    );
    // `u` in the panel takes it back, in the file too.
    press(&mut viewer, ":set<CR>u<Esc>");
    assert_eq!(value(&viewer, "ai.edit_mode"), Value::Text("live".into()));
    assert_eq!(last_written(&edits, "ai.edit_mode"), Some(None));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_panel_is_worked_by_keys() {
    let (mut viewer, directory, edits) = configured(TEXT, &[]);
    let without = drawn(&mut viewer);
    press(&mut viewer, ":set<CR>");
    assert!(viewer.is_setting());
    // Drawn: its markup is set by Typst.
    assert!(drawn(&mut viewer) > without);
    let markup = viewer.settings_markup();
    assert!(viewer.chrome.render(&markup, 1.0).is_some(), "{markup}");
    // Narrowed by typing behind `/`; a switch changes at once.
    press(&mut viewer, "/feedback<CR>");
    let rows = viewer.setting_rows();
    assert!(
        rows.iter()
            .any(|row| matches!(row, SetRow::Opt(opt) if opt.name == "aids.feedback"))
    );
    assert!(
        !rows
            .iter()
            .any(|row| matches!(row, SetRow::Opt(opt) if opt.name == "aids.numbers"))
    );
    press(&mut viewer, "<Space>");
    assert_eq!(value(&viewer, "aids.feedback"), Value::Switch(false));
    assert!(viewer.is_setting(), "the panel stays");
    press(&mut viewer, "x");
    assert_eq!(value(&viewer, "aids.feedback"), Value::Switch(true));
    press(&mut viewer, "u");
    assert_eq!(value(&viewer, "aids.feedback"), Value::Switch(false));
    // A choice is asked: a list of what it can be, and back to the panel.
    press(&mut viewer, "<Esc>/numbers<CR><CR>");
    assert!(viewer.picker.is_some() && !viewer.is_setting());
    let listed = viewer
        .picker
        .as_ref()
        .map(|picker| picker.listing().1.len());
    assert_eq!(listed, Some(3));
    press(&mut viewer, "<C-n><CR>");
    assert!(viewer.is_setting(), "the panel is back");
    assert_eq!(value(&viewer, "aids.numbers"), Value::Text("always".into()));
    // A number is typed.
    press(&mut viewer, "<Esc>/reach<CR><CR>40<CR>");
    assert_eq!(value(&viewer, "aids.reach"), Value::Number(40.0));
    assert_eq!(
        last_written(&edits, "aids.reach"),
        Some(Some(Written::Integer(40)))
    );
    // `a` goes to the next set of aids; Escape leaves.
    press(&mut viewer, "a");
    assert_eq!(viewer.settings.preset, Preset::Writing);
    press(&mut viewer, "<Esc><Esc>");
    assert!(!viewer.is_setting());
    let _ = std::fs::remove_dir_all(directory);
}

use super::super::set::Row as SetRow;

#[test]
fn a_key_of_ones_own_is_made_by_pressing_it() {
    let (mut viewer, directory, edits) = configured(TEXT, &[]);
    press(&mut viewer, ":set<CR>/a key of yours<CR>");
    // The first such row is for normal mode.
    press(&mut viewer, "<CR>,w<CR>:count<C-v><CR><CR>");
    assert_eq!(
        viewer.settings.normal,
        [(",w".to_owned(), ":count<CR>".to_owned())]
    );
    assert!(
        said(&viewer).contains(", (the last f or t back) now waits"),
        "{}",
        said(&viewer)
    );
    assert_eq!(
        edits.lock().unwrap().last().map(|edit| edit.path.join(".")),
        Some("keys.normal.,w".to_owned())
    );
    press(&mut viewer, "<Esc><Esc>,w");
    assert!(said(&viewer).contains("words"), "{}", said(&viewer));
    // Listed, and taken away with `x`.
    press(&mut viewer, ":set<CR>/:count<CR>x");
    assert_eq!(viewer.settings.normal.len(), 0);
    assert_eq!(
        edits.lock().unwrap().last().map(|edit| edit.value.clone()),
        Some(None)
    );
    let _ = std::fs::remove_dir_all(directory);
}

/// How many things a frame draws: the pages, and all over and beside
/// them.
fn drawn(viewer: &mut Viewer) -> usize {
    viewer.scene(&|_| false).quads.len()
}

#[test]
fn each_set_of_aids_draws_its_own() {
    let mut counts = Vec::new();
    for preset in Preset::ALL {
        let explicit = [("aids.preset", Value::Text(preset.name().into()))];
        let (mut viewer, directory, _) = configured(TEXT, &explicit);
        press(&mut viewer, "/Two<CR>");
        let normal = drawn(&mut viewer);
        let numbers = viewer
            .numbered
            .as_ref()
            .map_or(0, |(_, numbered)| numbered.len());
        // Typing in a paragraph.
        press(&mut viewer, "A an");
        let typing = drawn(&mut viewer);
        let numbers_typing = !viewer.numbers_resting();
        press(&mut viewer, "<Esc>");
        compile(&mut viewer);
        let back = drawn(&mut viewer);
        // Typing in an equation: the frame stands in all.
        press(&mut viewer, "Go$a");
        compile(&mut viewer);
        let tints = tints(&mut viewer, [0x45, 0x85, 0x88, 0x80]);
        counts.push((
            preset,
            normal,
            numbers,
            typing,
            numbers_typing,
            back,
            tints.len(),
        ));
        press(&mut viewer, "<Esc>");
        let _ = std::fs::remove_dir_all(directory);
    }
    let [quiet, writing, everything] = [counts[0], counts[1], counts[2]];
    // Numbers in normal mode in all; while typing only with everything.
    assert!(
        quiet.2 > 0 && writing.2 > 0 && everything.2 > 0,
        "{counts:?}"
    );
    assert!(!quiet.4 && !writing.4 && everything.4, "{counts:?}");
    // Quiet draws least, then writing (the tag of the label), then all.
    assert!(quiet.1 < writing.1, "{counts:?}");
    assert!(writing.1 <= everything.1, "{counts:?}");
    // While typing, quiet draws fewer than in normal mode: the numbers rest.
    assert!(quiet.3 < quiet.1, "{counts:?}");
    // And they are back once typing is over.
    assert!(quiet.5 >= quiet.1, "{counts:?}");
    // The frame of what typing goes into, in every set.
    assert!(
        quiet.6 >= 4 && writing.6 >= 4 && everything.6 >= 4,
        "{counts:?}"
    );
}

#[test]
fn the_numbers_rest_while_typing_and_come_back_without_a_flicker() {
    let (mut viewer, directory, _) = configured(TEXT, &[]);
    press(&mut viewer, "/Two<CR>");
    let numbered = |viewer: &mut Viewer| {
        let _ = viewer.scene(&|_| false);
        viewer
            .numbered
            .as_ref()
            .map_or(0, |(_, numbered)| numbered.len())
    };
    assert!(numbered(&mut viewer) > 1);
    // `A` alone takes nothing away: nothing is typed yet.
    press(&mut viewer, "A");
    assert!(!viewer.numbers_resting() && numbered(&mut viewer) > 1);
    press(&mut viewer, " more");
    assert!(viewer.numbers_resting());
    let before = drawn(&mut viewer);
    press(&mut viewer, " words");
    assert_eq!(
        drawn(&mut viewer),
        before,
        "nothing comes and goes while typing"
    );
    // Escape: not drawn where they would be the old ones; once the pages
    // come, counted anew.
    press(&mut viewer, "<Esc>");
    assert!(!viewer.numbers_resting());
    compile(&mut viewer);
    assert!(numbered(&mut viewer) > 1);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_three_behaviours_of_typing() {
    let text = "The integral of a function.\n\nMore text.\n";
    let (mut viewer, directory, _) = configured(text, &[]);
    let line = |viewer: &Viewer, line: &str| {
        let text = viewer.text().unwrap_or_default();
        text.lines().any(|there| there.trim_end() == line)
    };
    // The rest of a word is not shown quietly; with words, Tab does not
    // take it, Ctrl-Y does.
    press(&mut viewer, ":set ghost=words<CR>Goint<Tab>");
    assert!(line(&viewer, "int"), "{:?}", viewer.text());
    assert!(said(&viewer).contains("Ctrl-Y"), "{}", said(&viewer));
    press(&mut viewer, "<C-y>");
    assert!(line(&viewer, "integral"), "{:?}", viewer.text());
    press(&mut viewer, "<Esc>:set tabwords<CR>Gofun<Tab>");
    assert!(line(&viewer, "function"), "{:?}", viewer.text());
    press(&mut viewer, "<Esc>");
    // `$` is a dollar sign where the user wants it so.
    press(&mut viewer, ":set nodollar<CR>Go5$<Esc>");
    assert!(line(&viewer, "5\\$"), "{:?}", viewer.text());
    press(&mut viewer, ":set dollar<CR>Gox $y<Esc>");
    compile(&mut viewer);
    assert!(line(&viewer, "x $y$"), "{:?}", viewer.text());
    // Letters typed together stay so where equations are not tidied.
    press(&mut viewer, ":set notidy<CR>Go$ab<Esc>");
    compile(&mut viewer);
    assert!(
        viewer.text().unwrap_or_default().contains("$ab$"),
        "{:?}",
        viewer.text()
    );
    press(&mut viewer, ":set tidy<CR>Go$cd<Esc>");
    assert!(
        viewer.text().unwrap_or_default().contains("$c d$"),
        "{:?}",
        viewer.text()
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_set_of_aids_takes_back_what_was_said_of_single_aids() {
    let explicit = [("aids.tags", Value::Switch(true))];
    let (mut viewer, directory, edits) = configured(TEXT, &explicit);
    assert_eq!(value(&viewer, "aids.tags"), Value::Switch(true));
    press(&mut viewer, "za");
    assert_eq!(viewer.settings.preset, Preset::Writing);
    assert!(
        said(&viewer).contains("one aid gave way"),
        "{}",
        said(&viewer)
    );
    assert_eq!(
        last_written(&edits, "aids.preset"),
        Some(Some(Written::Text("writing".into())))
    );
    assert_eq!(value(&viewer, "language.spelling"), Value::Switch(true));
    press(&mut viewer, ":set aids=quiet<CR>");
    assert_eq!(value(&viewer, "aids.tags"), Value::Switch(false));
    assert_eq!(last_written(&edits, "aids.preset"), Some(None));
    // Taken back: writing again, and before it the tags of the user's.
    press(&mut viewer, ":set<CR>uu<Esc>");
    assert_eq!(viewer.settings.preset, Preset::Quiet);
    assert_eq!(value(&viewer, "aids.tags"), Value::Switch(true));
    assert_eq!(
        last_written(&edits, "aids.tags"),
        Some(Some(Written::Bool(true)))
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn whoever_had_settings_is_told_once_that_the_page_is_quieter() {
    let (mut viewer, directory) = editing(TEXT);
    let state = directory.join("told");
    viewer.state_dir = Some(state.clone());
    let (mut stored, _) = stored(&[("view.ui_scale", Value::Number(1.25))]);
    stored.existed = true;
    stored.window = true;
    viewer.configure_settings(&stored, false);
    assert_eq!(said(&viewer), super::super::set::QUIETER);
    let (mut again, _) = editing(TEXT);
    again.state_dir = Some(state);
    again.configure_settings(&stored, false);
    assert_eq!(said(&again), "");
    // Who has none is not told; who chose a set of aids neither.
    let (mut new, other) = editing(TEXT);
    new.state_dir = Some(other.join("state-new"));
    let (mut fresh, _) = self::stored(&[]);
    fresh.window = true;
    new.configure_settings(&fresh, false);
    assert_eq!(said(&new), "");
    let _ = std::fs::remove_dir_all(directory);
    let _ = std::fs::remove_dir_all(other);
}

#[test]
fn nothing_comes_and_goes_within_a_key_in_the_quiet_set() {
    let (mut viewer, directory, _) = configured(TEXT, &[]);
    press(&mut viewer, "/Two<CR>");
    compile(&mut viewer);
    for key in ["A", " ", "m", "o", "<BS>", "<Esc>", "j", "k"] {
        let before = viewer.snapshot();
        press(&mut viewer, key);
        let mut counts = vec![drawn(&mut viewer), drawn(&mut viewer)];
        // A compile of the text before the key, landing late; then its
        // own, twice.
        let (revision, world) = before;
        let compiled = zetwerk_typst::compile(&world);
        viewer.compiled(CompileOutput {
            revision,
            world: Arc::new(world),
            compiled,
            duration: std::time::Duration::from_millis(3),
        });
        counts.push(drawn(&mut viewer));
        compile(&mut viewer);
        counts.push(drawn(&mut viewer));
        compile(&mut viewer);
        counts.push(drawn(&mut viewer));
        // What is drawn may come with the pages, or go with the key, but
        // nothing goes and comes back, or comes and goes.
        let up = counts.windows(2).all(|pair| pair[0] <= pair[1]);
        let down = counts.windows(2).all(|pair| pair[0] >= pair[1]);
        assert!(up || down, "{key}: {counts:?}");
        assert_eq!(counts[0], counts[1], "{key}: {counts:?}");
    }
    let _ = std::fs::remove_dir_all(directory);
}
