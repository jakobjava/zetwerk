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
fn every_list_closes_with_escape_and_keeps_its_keys() {
    for command in run_commands() {
        let directory = directory(RICH);
        std::fs::write(directory.join("refs.bib"), BIB).unwrap();
        let (mut viewer, _) = open(&directory);
        viewer.execute(command);
        if viewer.picker.is_none() {
            let _ = std::fs::remove_dir_all(&directory);
            continue;
        }
        let before = viewer.text().unwrap_or_default().to_owned();
        // Typed into the list, not the text.
        press(&mut viewer, "ddxpu<BS><BS><BS><BS><BS>");
        // Escape leads back to the list it came of, and away at last.
        for _ in 0..3 {
            press(&mut viewer, "<Esc>");
        }
        assert!(viewer.picker.is_none(), "{command}: the list stays");
        assert_eq!(viewer.text(), Some(before.as_str()), "{command}");
        let _ = std::fs::remove_dir_all(&directory);
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

/// What the list that is up shows, as the query leaves it.
fn listed(viewer: &Viewer) -> Vec<String> {
    let picker = viewer.picker.as_ref();
    picker.map_or_else(Vec::new, |picker| {
        picker.listing().1.into_iter().map(str::to_owned).collect()
    })
}

#[test]
fn a_paper_typst_does_not_know_is_not_written_and_the_nearest_is_offered() {
    let (mut viewer, directory) = editing("Words.\n");
    // Typed in the list: what is no paper is not offered as itself; the
    // paper it is part of is.
    press(&mut viewer, "gDpaper<CR>letter");
    assert_eq!(
        listed(&viewer).first().map(String::as_str),
        Some("us-letter")
    );
    press(&mut viewer, "<CR>");
    let text = viewer.text().unwrap_or_default().to_owned();
    assert!(text.contains("paper: \"us-letter\""), "{text:?}");
    // As a command: refused, said why, and the nearest are listed.
    press(&mut viewer, "<Esc>");
    viewer.execute("doc set 4 a44");
    assert_eq!(viewer.text(), Some(text.as_str()));
    assert!(said(&viewer).contains("no paper"), "{}", said(&viewer));
    assert_eq!(listed(&viewer).first().map(String::as_str), Some("a4"));
    press(&mut viewer, "<CR>");
    assert!(
        viewer
            .text()
            .is_some_and(|text| text.contains("paper: \"a4\""))
    );
    // One Typst knows that is not in the short list is taken, as Typst
    // writes it.
    viewer.execute("doc set 4 ISO-B5");
    assert!(
        viewer
            .text()
            .is_some_and(|text| text.contains("paper: \"iso-b5\"")),
        "{:?}",
        viewer.text()
    );
    compile(&mut viewer);
    assert_eq!(viewer.document_revision, Some(viewer.revision));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_font_that_is_not_installed_is_said_to_be_so_with_the_near_ones() {
    let (mut viewer, directory) = editing("Words.\n");
    viewer.execute("doc set 0 Libertinus");
    assert_eq!(viewer.text(), Some("Words.\n"));
    assert!(said(&viewer).contains("no font"), "{}", said(&viewer));
    assert!(
        listed(&viewer).contains(&"Libertinus Serif".to_owned()),
        "{:?}",
        listed(&viewer)
    );
    // One that is there, in any capitals.
    viewer.execute("doc set 0 dejavu sans mono");
    assert!(
        viewer
            .text()
            .is_some_and(|text| text.contains("font: \"dejavu sans mono\"")),
        "{:?}",
        viewer.text()
    );
    // A length without a unit gets the row's; a word is no length.
    viewer.execute("doc set 6 wide");
    assert!(
        said(&viewer).contains("a number and a unit"),
        "{}",
        said(&viewer)
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_colour_that_is_none_offers_the_near_ones() {
    let (mut viewer, directory) = editing("Words.\n");
    press(&mut viewer, ":styles<CR>links: colour<CR>tael<CR>");
    assert!(!viewer.text().unwrap_or_default().contains("#show link"));
    assert!(said(&viewer).contains("no colour"), "{}", said(&viewer));
    assert_eq!(listed(&viewer).first().map(String::as_str), Some("teal"));
    press(&mut viewer, "<CR>");
    assert!(
        viewer
            .text()
            .is_some_and(|text| text.contains("#show link: set text(fill: teal)")),
        "{:?}",
        viewer.text()
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn what_is_typed_is_taken_as_itself_before_an_item_like_it() {
    for (typed, written) in [("no", "header: [no]"), ("on", "header: [on]")] {
        let (mut viewer, directory) = editing("Words.\n");
        press(&mut viewer, &format!("gDrunning head<CR>{typed}"));
        let first = listed(&viewer).first().cloned().unwrap_or_default();
        assert_eq!(first, format!("use \u{201c}{typed}\u{201d}"));
        press(&mut viewer, "<CR>");
        let text = viewer.text().unwrap_or_default();
        assert!(text.contains(written), "{typed}: {text:?}");
        let _ = std::fs::remove_dir_all(directory);
    }
    // What is an item exactly is that item.
    let (mut viewer, folder) = editing("Words.\n");
    press(&mut viewer, "gDrunning head<CR>none");
    assert_eq!(listed(&viewer), ["none"]);
    let _ = std::fs::remove_dir_all(folder);
    // A new file is not one like it that is there.
    let folder = directory("#figure(image(\"network.svg\"), caption: [A net])\n");
    let image = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"40\" height=\"20\"/>";
    std::fs::write(folder.join("network.svg"), image).unwrap();
    let (mut viewer, _) = open(&folder);
    press(&mut viewer, "gF<CR>file<CR>new.svg");
    let first = listed(&viewer).first().cloned().unwrap_or_default();
    assert_eq!(first, "use \u{201c}new.svg\u{201d}");
    press(&mut viewer, "<CR>");
    assert!(
        said(&viewer).contains("no file new.svg"),
        "{}",
        said(&viewer)
    );
    assert!(
        viewer
            .text()
            .is_some_and(|text| text.contains("network.svg"))
    );
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn escape_from_the_styles_leads_back_to_their_row() {
    let (mut viewer, directory) = editing("Words.\n");
    press(&mut viewer, "gDstyles<CR><Esc>");
    let chosen = viewer
        .picker
        .as_ref()
        .and_then(|picker| picker.chosen().cloned());
    assert_eq!(
        chosen,
        Some(super::super::assist::Target::Command("styles 0".to_owned()))
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_source_that_cannot_be_read_is_listed_and_mended() {
    let folder = directory("Words @b.\n#bibliography(\"refs.bib\")\n");
    let bib = "@book{a,\n  title = {Open,\n}\n\n@book{b, title = {Fine}}\n";
    std::fs::write(folder.join("refs.bib"), bib).unwrap();
    let (mut viewer, _) = open(&folder);
    press(&mut viewer, "gB");
    let rows = listed(&viewer);
    let broken = rows.iter().find(|row| row.starts_with("cannot be read"));
    assert_eq!(
        broken.map(String::as_str),
        Some("cannot be read: @book{a, title = {Open, }\tline 1"),
        "{rows:?}"
    );
    assert!(rows.iter().any(|row| row.starts_with("b · 1×")), "{rows:?}");
    press(&mut viewer, "cannot<CR>");
    assert_eq!(
        listed(&viewer)[0],
        "close it: add the } it lacks at its end"
    );
    press(&mut viewer, "<CR>");
    let mended = std::fs::read_to_string(folder.join("refs.bib")).unwrap();
    assert_eq!(
        mended,
        "@book{a,\n  title = {Open,\n}}\n\n@book{b, title = {Fine}}\n"
    );
    // Taken out, the rest of the file stays as it was.
    std::fs::write(folder.join("refs.bib"), bib).unwrap();
    viewer.execute("bib drop 0");
    let dropped = std::fs::read_to_string(folder.join("refs.bib")).unwrap();
    assert_eq!(dropped, "@book{b, title = {Fine}}\n");
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn sources_cited_by_one_key_are_said_to_clash_and_one_is_renamed() {
    let folder = directory("Words @k.\n#bibliography(\"refs.bib\")\n");
    let bib = "@book{k, author = {Ann Ash}, title = {One}, year = {2001}}\n\n\
               @book{k, author = {Bo Birch}, title = {Two}, year = {2002}}\n";
    std::fs::write(folder.join("refs.bib"), bib).unwrap();
    let (mut viewer, _) = open(&folder);
    press(&mut viewer, "gB");
    let rows = listed(&viewer);
    let clashes = rows
        .iter()
        .filter(|row| row.contains("2 sources clash"))
        .count();
    assert_eq!(clashes, 2, "{rows:?}");
    press(&mut viewer, "k · 2<CR>");
    assert!(listed(&viewer)[0].starts_with("2 sources are cited as k"));
    press(&mut viewer, "<CR>");
    let renamed = std::fs::read_to_string(folder.join("refs.bib")).unwrap();
    assert_eq!(
        renamed,
        bib.replacen("@book{k, author = {Bo", "@book{birch2002, author = {Bo", 1)
    );
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn a_source_an_agent_proposes_is_added_only_when_the_user_takes_it() {
    use super::super::agent::AgentEdits;
    let folder = directory("Words.\n#bibliography(\"refs.bib\")\n");
    std::fs::write(folder.join("refs.bib"), BIB).unwrap();
    let (mut viewer, _) = open(&folder);
    viewer.set_agent_edits(AgentEdits::Propose);
    let entry = "@book{lamport1994, author = {Leslie Lamport}, title = {LaTeX}, year = {1994}}";
    let added = viewer.agent_add_sources_checked(entry, false).unwrap();
    let id = added.proposal.unwrap_or_else(|| panic!("proposed"));
    assert!(added.keys.is_empty());
    assert_eq!(
        std::fs::read_to_string(folder.join("refs.bib")).unwrap(),
        BIB
    );
    // Withdrawn by the agent as any proposal.
    viewer.withdraw_proposal(id).unwrap();
    assert!(viewer.proposed_sources().is_empty());
    // Shown first in the list; Enter, and it is added.
    viewer.agent_add_sources_checked(entry, false).unwrap();
    press(&mut viewer, "gB");
    let rows = listed(&viewer);
    assert!(
        rows[4].starts_with("an agent proposes: lamport1994"),
        "{rows:?}"
    );
    viewer.execute(&format!("bib proposed {}", viewer.proposed_sources()[0].0));
    press(&mut viewer, "<CR>");
    let written = std::fs::read_to_string(folder.join("refs.bib")).unwrap();
    assert!(written.contains("lamport1994"), "{written}");
    assert!(viewer.proposed_sources().is_empty());
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn u_right_after_a_change_of_the_bibliography_says_how_that_is_undone() {
    let folder = directory("Words.\n#bibliography(\"refs.bib\")\n");
    std::fs::write(folder.join("refs.bib"), BIB).unwrap();
    let (mut viewer, _) = open(&folder);
    press(&mut viewer, "xu");
    assert_eq!(viewer.text(), Some("Words.\n#bibliography(\"refs.bib\")\n"));
    press(&mut viewer, "x");
    viewer.execute("bib remove! knuth1984");
    press(&mut viewer, "<Esc>u");
    assert!(said(&viewer).contains(":bib undo"), "{}", said(&viewer));
    assert_eq!(viewer.text(), Some("ords.\n#bibliography(\"refs.bib\")\n"));
    // Once: then u is the text's again.
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some("Words.\n#bibliography(\"refs.bib\")\n"));
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn what_a_template_sets_is_said_in_the_document_manager_and_can_be_changed() {
    let text = "#let conf(doc) = {\n  set text(size: 14pt)\n  set par(justify: true)\n  doc\n}\n\
                #show: conf\nWords.\n";
    let (mut viewer, folder) = editing(text);
    press(&mut viewer, "gD");
    let rows = listed(&viewer);
    let row = |name: &str| {
        let prefix = format!("{name}\t");
        rows.iter()
            .find(|row| row.starts_with(&prefix))
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(
        row("font size"),
        "font size\t14pt  (set by the template)",
        "{rows:?}"
    );
    assert_eq!(
        row("justified lines"),
        "justified lines\ton  (set by the template)"
    );
    assert_eq!(row("font"), "font\tLibertinus Serif");
    // Changed here, the rule stands behind the template's and holds.
    press(&mut viewer, "font size<CR>12pt<CR>");
    let now = viewer.text().unwrap_or_default();
    let (template, rule) = (now.find("#show: conf"), now.find("#set text(size: 12pt)"));
    assert!(template < rule && rule.is_some(), "{now}");
    press(&mut viewer, "justified<CR>");
    assert!(
        viewer.text().unwrap_or_default().contains("justify: false"),
        "off, from on"
    );
    compile(&mut viewer);
    assert_eq!(viewer.document_revision, Some(viewer.revision));
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn what_an_agent_changes_while_the_user_types_is_a_step_of_its_own() {
    let (mut viewer, folder) = editing("One two.\n\nThree four.\n");
    press(&mut viewer, "Aabc");
    viewer
        .agent_replace("Three", "Five", false, None, false)
        .unwrap();
    press(&mut viewer, "def<Esc>");
    assert_eq!(viewer.text(), Some("One two.abcdef\n\nFive four.\n"));
    // Back, the last first: the agent's change is never taken back with
    // the user's typing, nor lost by it.
    press(&mut viewer, "u");
    let first = viewer.text().unwrap_or_default().to_owned();
    assert!(
        first.contains("Five"),
        "the agent's change stays: {first:?}"
    );
    assert!(!first.contains("def"), "{first:?}");
    press(&mut viewer, "u");
    let second = viewer.text().unwrap_or_default().to_owned();
    assert!(second.contains("Three"), "{second:?}");
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some("One two.\n\nThree four.\n"));
    // And forth again, all of it.
    press(&mut viewer, "<C-r><C-r><C-r>");
    assert_eq!(viewer.text(), Some("One two.abcdef\n\nFive four.\n"));
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn every_insert_of_the_menu_is_put_in_whole_by_enter() {
    use crate::typst_text::TEXT_INSERTS;
    let mut wrong = Vec::new();
    for insert in TEXT_INSERTS {
        let (mut viewer, folder) = editing("Words.\n");
        let word = insert.name.split(' ').next().unwrap_or_default();
        press(&mut viewer, &format!("Go#{word}"));
        // Down the list to it, as a user would.
        let chosen = |viewer: &Viewer| {
            viewer.completion.as_ref().and_then(|completion| {
                let entry = completion.candidates.get(completion.chosen)?;
                Some(entry.candidate.label.clone())
            })
        };
        for _ in 0..30 {
            if chosen(&viewer).as_deref() == Some(insert.name) {
                break;
            }
            press(&mut viewer, "<C-n>");
        }
        let taken = chosen(&viewer);
        press(&mut viewer, "<CR>");
        // A list of files may be up: it is left.
        press(&mut viewer, "<Esc><Esc>");
        let text = viewer.text().unwrap_or_default().to_owned();
        let left = text.contains(&format!("#{word}\n"));
        if taken.as_deref() != Some(insert.name) || left {
            wrong.push(format!("{}: chosen {taken:?}, text {text:?}", insert.name));
        }
        let _ = std::fs::remove_dir_all(folder);
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
