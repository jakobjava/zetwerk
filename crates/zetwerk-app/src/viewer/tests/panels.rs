//! The lists and panels as a user meets them: every entry of the list of
//! all there is to do, in documents where it applies and where it does
//! not; Escape and Enter in each list.

use super::*;

/// A document with a bit of everything the commands act on.
const RICH: &str = "#set page(width: 300pt, height: 300pt)\n\
= Introduction <intro>\n\
Some text with a citation @knuth1984 and a reference to @fig:box.\n\
\n\
== Aim\n\
$ x + y $ <eq:one>\n\
\n\
#figure(\n  rect(width: 40pt, height: 20pt),\n  caption: [A box],\n) <fig:box>\n\
\n\
#table(columns: 2, [a], [b], [c], [d])\n\
\n\
= Methods\n\
More text.\n\
\n\
#bibliography(\"refs.bib\")\n";

const BIB: &str = "@book{knuth1984,\n  author = {Donald E. Knuth},\n  title = {The TeXbook},\n  \
                   year = {1984},\n}\n";

/// The message the viewer shows, if any.
fn said(viewer: &Viewer) -> String {
    viewer
        .message
        .as_ref()
        .map_or_else(String::new, |message| message.text.clone())
}

/// The commands the list of all there is to do runs, or begins for the
/// user to finish.
fn run_commands() -> Vec<&'static str> {
    palette::entries()
        .filter_map(|(_, command)| command)
        .collect()
}

#[test]
#[ignore = "exploration: prints what each command says"]
fn walk_every_command_of_the_list() {
    for command in run_commands() {
        for (name, text, page) in [
            ("rich", RICH, false),
            ("empty", "", false),
            ("page", RICH, true),
        ] {
            let directory = directory(text);
            std::fs::write(directory.join("refs.bib"), BIB).unwrap();
            let (mut viewer, _) = open(&directory);
            if page {
                press(&mut viewer, "gsgs");
            }
            if let Some(editor) = viewer.editor.as_mut() {
                editor.set_cursor(text.find("box").unwrap_or(0));
            }
            let before = viewer.text().unwrap_or_default().to_owned();
            viewer.execute(command);
            let picker = viewer.picker.as_ref().map(|picker| {
                let (title, shown, _) = picker.listing();
                format!("[{title} x{}]", shown.len())
            });
            let changed = viewer.text().unwrap_or_default() != before;
            println!(
                "{command:24} {name:6} {} {} {}",
                picker.unwrap_or_default(),
                if changed { "CHANGED" } else { "" },
                said(&viewer)
            );
            press(&mut viewer, "<Esc><Esc>");
            let _ = std::fs::remove_dir_all(&directory);
        }
    }
}

#[test]
#[ignore = "exploration: escape and keys in every list"]
fn every_list_closes_with_escape_and_keeps_its_keys() {
    for command in run_commands() {
        for (name, text) in [("rich", RICH), ("empty", "")] {
            let directory = directory(text);
            std::fs::write(directory.join("refs.bib"), BIB).unwrap();
            let (mut viewer, _) = open(&directory);
            if let Some(editor) = viewer.editor.as_mut() {
                editor.set_cursor(text.find("box").unwrap_or(0));
            }
            viewer.execute(command);
            if viewer.picker.is_none() {
                let _ = std::fs::remove_dir_all(&directory);
                continue;
            }
            let before = viewer.text().unwrap_or_default().to_owned();
            // Typed into the list, not the text.
            press(&mut viewer, "ddxpu");
            press(&mut viewer, "<BS><BS><BS><BS><BS>");
            let mut escapes = 0;
            while viewer.picker.is_some() && escapes < 6 {
                press(&mut viewer, "<Esc>");
                escapes += 1;
            }
            let after = viewer.text().unwrap_or_default();
            println!(
                "{command:24} {name:6} escapes {escapes} {} {}",
                if viewer.picker.is_some() {
                    "STILL OPEN"
                } else {
                    ""
                },
                if after == before { "" } else { "TEXT CHANGED" },
            );
            let _ = std::fs::remove_dir_all(&directory);
        }
    }
}

#[test]
fn no_two_entries_of_the_list_are_for_the_same_keys() {
    let mut seen = std::collections::HashSet::new();
    for (keys, _) in palette::entries() {
        assert!(seen.insert(keys), "{keys} is in the list twice");
    }
}

#[test]
fn every_command_of_the_list_is_one() {
    // Those begun for the user to finish end in a blank.
    let run = run_commands().into_iter();
    for command in run.filter(|command| !command.is_empty() && !command.ends_with(' ')) {
        let directory = directory(RICH);
        std::fs::write(directory.join("refs.bib"), BIB).unwrap();
        let (mut viewer, _) = open(&directory);
        viewer.execute(command);
        let said = said(&viewer);
        assert!(!said.starts_with("not a command"), "{command}: {said}");
        let _ = std::fs::remove_dir_all(&directory);
    }
}

#[test]
#[ignore = "exploration: Enter on every row of every list, then Escape"]
fn enter_on_every_row_then_escape() {
    for command in [
        "doc",
        "styles",
        "bib",
        "bib view",
        "figures",
        "structure",
        "references",
        "invisibles",
        "outline",
        "labels",
        "files",
    ] {
        let mut row = 0;
        loop {
            let directory = directory(RICH);
            std::fs::write(directory.join("refs.bib"), BIB).unwrap();
            let (mut viewer, _) = open(&directory);
            viewer.execute(command);
            let Some(picker) = &viewer.picker else {
                break;
            };
            let (_, shown, chosen) = picker.listing();
            if row >= shown.len() {
                break;
            }
            let label = shown[row].to_owned();
            let before = viewer.text().unwrap_or_default().to_owned();
            // To the first row, then down to the one wanted.
            for _ in 0..chosen {
                press(&mut viewer, "<Up>");
            }
            for _ in 0..row {
                press(&mut viewer, "<Down>");
            }
            press(&mut viewer, "<CR>");
            let after_enter = viewer.text().unwrap_or_default() != before;
            let then = viewer
                .picker
                .as_ref()
                .map(|picker| picker.listing().0.to_owned());
            let said_then = said(&viewer);
            let mut escapes = 0;
            while viewer.picker.is_some() && escapes < 6 {
                press(&mut viewer, "<Esc>");
                escapes += 1;
            }
            let changed = viewer.text().unwrap_or_default() != before;
            println!(
                "{command:10} {row:2} {:50} -> [{}] {} esc {escapes}{} {}",
                label.replace('\t', " | "),
                then.unwrap_or_default(),
                if after_enter { "CHANGED" } else { "" },
                if changed { " KEPT" } else { "" },
                said_then,
            );
            let _ = std::fs::remove_dir_all(&directory);
            row += 1;
        }
    }
}

#[test]
fn a_setting_of_two_is_turned_to_the_other_at_the_first_enter() {
    for (row, rule) in [
        ("orientation", "#set page(flipped: true)"),
        ("captions stand", "#set figure.caption(position: top)"),
        ("justified lines", "#set par(justify: true)"),
    ] {
        let (mut viewer, directory) = editing("Words.\n");
        press(&mut viewer, &format!("gD{row}<CR>"));
        let text = viewer.text().unwrap_or_default();
        assert!(text.contains(rule), "{row}: {text:?}");
        // And back, at the next.
        press(&mut viewer, &format!("{row}<CR>"));
        assert!(!viewer.text().unwrap_or_default().contains(rule), "{row}");
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn an_agent_never_changes_the_next_document_by_a_revision_of_the_last() {
    use super::super::agent::AgentEdits;
    let (mut before, first) = editing("The old text.\n");
    let read = before.state().revision;
    before.set_agent_edits(AgentEdits::Propose);
    // The window goes to another document, whose text the agent has
    // never read.
    let (mut next, second) = editing("The new text.\n");
    next.take_over_from(&before);
    assert_eq!(next.agent_edits(), AgentEdits::Propose);
    next.set_agent_edits(AgentEdits::Live);
    let refused = next.agent_replace("The", "A", false, Some(read), false);
    assert!(refused.unwrap_err().contains("revision"));
    assert_eq!(next.text(), Some("The new text.\n"));
    // Its own revision is good.
    let now = next.state().revision;
    next.agent_replace("The", "A", false, Some(now), false)
        .unwrap();
    let _ = std::fs::remove_dir_all(first);
    let _ = std::fs::remove_dir_all(second);
}

#[test]
#[ignore = "exploration: where the lists stand in small windows"]
fn lists_in_small_windows() {
    for (ui, window) in [
        (1.0, (1000, 620)),
        (1.6, (1000, 620)),
        (1.6, (700, 500)),
        (1.0, (500, 400)),
    ] {
        for command in ["help", "styles", "doc", "bib", "recent"] {
            let (mut viewer, directory) = editing(RICH);
            std::fs::write(directory.join("refs.bib"), BIB).unwrap();
            viewer.resize(window, 1.0);
            viewer.ui = ui;
            viewer.scale = viewer.display * ui;
            viewer.arrange();
            viewer.execute(command);
            let scene = viewer.scene(&|_| false);
            let top = (f64::from(window.1) / 4.0).round() as f32;
            let placed: Vec<(f32, f32, f32, f32)> = scene
                .quads
                .iter()
                .filter(|quad| (quad.y - top).abs() < 1.0)
                .map(|quad| (quad.x, quad.y, quad.width, quad.height))
                .collect();
            println!("ui {ui} {window:?} {command:8} {placed:?}");
            let _ = std::fs::remove_dir_all(directory);
        }
    }
}

#[test]
fn the_list_of_all_there_is_to_do_shows_every_entrys_keys_whole() {
    let (mut viewer, directory) = editing("Words.\n");
    press(&mut viewer, "g?");
    let Some(picker) = &viewer.picker else {
        panic!("the list is up");
    };
    let (_, shown, _) = picker.listing();
    let keys: Vec<&str> = palette::entries().map(|(keys, _)| keys).collect();
    assert_eq!(shown.len(), keys.len());
    for (label, keys) in shown.iter().zip(keys) {
        // What it is, cut where long, and its keys in a column of their
        // own, which is not.
        let (what, listed) = label.split_once('\t').unwrap_or((label, ""));
        assert_eq!(listed, keys, "{label}");
        assert!(what.chars().count() <= 70, "{what} is cut");
        assert!(keys.chars().count() <= 40, "{keys} is cut");
    }
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_list_wider_than_the_window_begins_in_it() {
    let (mut viewer, directory) = editing(RICH);
    viewer.resize((500, 400), 1.0);
    viewer.ui = 1.6;
    viewer.scale = viewer.display * viewer.ui;
    viewer.arrange();
    press(&mut viewer, "g?");
    let scene = viewer.scene(&|_| false);
    let list = scene
        .quads
        .iter()
        .find(|quad| (quad.y - 100.0).abs() < 1.0)
        .unwrap_or_else(|| panic!("the list is drawn a quarter down"));
    assert!(list.width > 500.0, "the list is wider than the window");
    assert!(list.x >= 0.0, "it begins at {}", list.x);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
#[ignore = "exploration: the bibliography manager on a broken file"]
fn sources_of_a_broken_file() {
    for bib in [
        "@book{a,\n  title = {Unclosed\n",
        "@book{a, title = {One}, year = {2000}}\n@misc{b, title = {Two\n\n@article{c, title = {Three}, year = 2001}\n",
        "garbage only\n",
        "@book{a, title = {One}, year = {2000}}\n@book{a, title = {Same key}, year = {2001}}\n",
    ] {
        let directory = directory("Text @a.\n#bibliography(\"refs.bib\")\n");
        std::fs::write(directory.join("refs.bib"), bib).unwrap();
        let (mut viewer, _) = open(&directory);
        viewer.execute("bib");
        let listed = viewer
            .picker
            .as_ref()
            .map(|picker| picker.listing().1.join(" / "));
        println!("{bib:?}\n  listed: {listed:?} said: {}", said(&viewer));
        press(&mut viewer, "<Esc>");
        viewer.execute("bib new book");
        let after = std::fs::read_to_string(directory.join("refs.bib")).unwrap();
        println!("  said: {}\n  file now: {after:?}", said(&viewer));
        let entries = crate::bib::parse(&after);
        let keys: Vec<&str> = entries.iter().map(|entry| entry.key.as_str()).collect();
        println!("  parsed keys: {keys:?}");
        let _ = std::fs::remove_dir_all(&directory);
    }
}

#[test]
fn a_list_of_files_and_the_completions_are_not_up_at_once() {
    let directory = directory("Words.\n");
    std::fs::write(directory.join("plot.png"), b"").unwrap();
    std::fs::write(directory.join("other.png"), b"").unwrap();
    let (mut viewer, _) = open(&directory);
    press(&mut viewer, "Go#image(\"");
    let completion = viewer.completion.is_some();
    let picker = viewer
        .picker
        .as_ref()
        .map(|picker| picker.listing().1.len());
    assert_eq!(picker, Some(2), "the files are offered");
    assert!(!completion, "and nothing else");
    // Escape leaves the list and nothing else.
    press(&mut viewer, "<Esc>");
    assert!(viewer.picker.is_none());
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
#[ignore = "exploration: an agent's change while the user looks through the history"]
fn an_agent_changes_the_text_while_the_user_looks_back() {
    let (mut viewer, directory) = editing("Broken #nope(\n");
    press(&mut viewer, "ccFixed text.<Esc>");
    compile(&mut viewer);
    println!(
        "now: {:?} problems {:?}",
        viewer.text(),
        viewer.state().problems
    );
    press(&mut viewer, "gh<Down>");
    compile(&mut viewer);
    println!(
        "looking back: problems {:?} looks_back {}",
        viewer.state().problems,
        viewer.looks_back()
    );
    viewer
        .agent_replace("Fixed", "Fine", false, None, false)
        .unwrap();
    compile(&mut viewer);
    let state = viewer.state();
    println!(
        "after the agent: text {:?} revision {} compiled {:?} problems {:?} is_compiled {}",
        viewer.text(),
        state.revision,
        state.compiled_revision,
        state.problems,
        viewer.is_compiled()
    );
    press(&mut viewer, "<Esc>");
    compile(&mut viewer);
    println!(
        "after escape: {:?} problems {:?}",
        viewer.text(),
        viewer.state().problems
    );
    let _ = std::fs::remove_dir_all(&directory);
}
