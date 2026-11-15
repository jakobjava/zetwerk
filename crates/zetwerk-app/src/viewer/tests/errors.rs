//! A text Typst refuses: what the user is told, and the ways out from
//! the page (see `viewer/refused.rs`).

use super::*;

/// Compiles the text as the window does: what asked for the compile
/// is taken first.
fn landed(viewer: &mut Viewer) {
    viewer.take_edited();
    compile(viewer);
}

/// The text, typeset whole: its problems.
fn problems(viewer: &mut Viewer) -> Vec<String> {
    landed(viewer);
    viewer.state().problems
}

/// What the status line says of the text Typst refuses.
fn said(viewer: &Viewer) -> String {
    viewer.refused_status().unwrap_or_default()
}

/// Changes the text being edited as an agent or another program would:
/// `old` becomes `new`.
fn change(viewer: &mut Viewer, old: &str, new: &str) {
    let editor = viewer.editor.as_mut().unwrap();
    let at = editor.text().find(old).unwrap();
    editor.replace(&[(at..at + old.len(), new)]).unwrap();
    viewer.text_changed(Edited::Command);
}

#[test]
fn a_text_refused_from_the_start_says_why_and_is_got_out_of_from_there() {
    // The case of a journal brought back with letters typed into the
    // first line: no page has ever been made of the text.
    let text = "#set pagsgse(paper: \"a4\", margin: 2cm)\n= Intro\n\nText.\n";
    let (mut viewer, directory) = editing(text);
    assert!(viewer.document.is_none());
    let status = said(&viewer);
    assert!(status.starts_with("no page yet: line 1"), "{status}");
    assert!(status.contains("#set pagsgse(paper"), "{status}");
    assert!(
        status.contains("Typst knows nothing called “pagsgse”"),
        "{status}"
    );
    // The panel is up by itself, and says how out.
    assert!(viewer.refused_up());
    // `x` takes out the line Typst refuses; the page comes.
    press(&mut viewer, "x");
    assert_eq!(viewer.text(), Some("= Intro\n\nText.\n"));
    assert_eq!(problems(&mut viewer), Vec::<String>::new());
    assert!(viewer.document.is_some() && !viewer.refused_up());
    assert_eq!(viewer.refused_status(), None);
    // And undo brings it back.
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some(text));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_panel_put_away_stays_away_until_asked_for() {
    let text = "#set pagsgse(paper: \"a4\")\nText.\n";
    let (mut viewer, directory) = editing(text);
    assert!(viewer.refused_up());
    press(&mut viewer, "<Esc>");
    assert!(!viewer.refused_up());
    assert_eq!(viewer.text(), Some(text));
    // Typing on in the source, still refused: it does not come again.
    press(&mut viewer, "GoMore.<Esc>");
    landed(&mut viewer);
    assert!(!viewer.refused_up());
    press(&mut viewer, ":why<CR>");
    assert!(viewer.refused_up());
    // Enter goes to what is refused, and says what it is.
    press(&mut viewer, "<CR>");
    assert!(!viewer.refused_up());
    assert_eq!(
        viewer.editor.as_ref().unwrap().position().0,
        1,
        "lines count from 1"
    );
    let message = viewer.message.as_ref().unwrap();
    assert!(
        message.alert && message.text.contains("pagsgse"),
        "{}",
        message.text
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_rule_broken_with_pages_on_screen_says_the_page_is_behind() {
    let text = "#set text(size: 11pt)\n= Intro\n\nSome words.\n";
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "gsgs");
    assert!(viewer.document.is_some());
    change(&mut viewer, "size", "sise");
    landed(&mut viewer);
    let status = said(&viewer);
    assert!(
        status.starts_with("the page is behind the text: line 1 “#set text(sise: 11pt)”"),
        "{status}"
    );
    // Not up by itself: the pages are there, and the status line says.
    assert!(!viewer.refused_up());
    press(&mut viewer, ":why<CR>");
    assert!(viewer.refused_up());
    // `u`: back to the text the pages show.
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some(text));
    assert_eq!(problems(&mut viewer), Vec::<String>::new());
    assert_eq!(viewer.refused_status(), None);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_name_typed_behind_a_hash_and_left_is_taken_out_from_the_page() {
    let (mut viewer, directory) = editing("Some words.\n");
    press(&mut viewer, "gsgs");
    viewer.editor.as_mut().unwrap().set_cursor(5);
    press(&mut viewer, "i#xyz <Esc>");
    assert_ne!(problems(&mut viewer), Vec::<String>::new());
    let status = said(&viewer);
    assert!(
        status.contains("Typst knows nothing called “xyz”"),
        "{status}"
    );
    press(&mut viewer, ":why<CR>x");
    assert_eq!(viewer.text(), Some("Some  words.\n"));
    assert_eq!(problems(&mut viewer), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_reference_to_nothing_is_said_so_and_taken_out() {
    let (mut viewer, directory) = editing("= Intro\n\nSee @nowhere here.\n");
    let status = said(&viewer);
    assert!(status.contains("nothing is labelled “nowhere”"), "{status}");
    press(&mut viewer, "x");
    assert_eq!(viewer.text(), Some("= Intro\n\nSee nowhere here.\n"));
    assert_eq!(problems(&mut viewer), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_mark_left_open_is_what_is_taken_out() {
    let (mut viewer, directory) = editing("Text *bold and on.\n\nMore.\n");
    press(&mut viewer, "x");
    assert_eq!(viewer.text(), Some("Text bold and on.\n\nMore.\n"));
    assert_eq!(problems(&mut viewer), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn an_image_that_is_not_there_is_named_and_taken_out() {
    let (mut viewer, directory) = editing("= Intro\n\n#image(\"missing.png\")\n\nText.\n");
    let status = said(&viewer);
    assert!(
        status.contains("there is no file “missing.png”"),
        "{status}"
    );
    press(&mut viewer, "x");
    assert_eq!(viewer.text(), Some("= Intro\n\n\nText.\n"));
    assert_eq!(problems(&mut viewer), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn unsaved_changes_that_are_refused_can_be_let_go() {
    let (mut viewer, directory) = editing("Some words.\n");
    change(&mut viewer, "Some", "#xyz Some");
    landed(&mut viewer);
    press(&mut viewer, ":why<CR>e");
    assert_eq!(viewer.text(), Some("Some words.\n"));
    assert_eq!(problems(&mut viewer), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn any_other_key_puts_the_panel_away_and_does_what_it_does() {
    let (mut viewer, directory) = editing("#set pagsgse()\nOne\nTwo\n");
    assert!(viewer.refused_up());
    press(&mut viewer, "j");
    assert!(!viewer.refused_up());
    assert_eq!(viewer.editor.as_ref().unwrap().position().0, 2);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn an_error_in_another_file_leads_into_it() {
    let directory = directory("= Thesis\n#include \"one.typ\"\n");
    std::fs::write(directory.join("one.typ"), "== One\n#xyz text.\n").unwrap();
    let (mut viewer, _) = open(&directory);
    let status = said(&viewer);
    assert!(status.contains("one.typ, line 2 “#xyz text.”"), "{status}");
    press(&mut viewer, "<CR>");
    assert_eq!(viewer.state().file, "one.typ");
    assert_eq!(viewer.editor.as_ref().unwrap().position().0, 2);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn after_a_way_out_the_next_refusal_is_said_too() {
    let (mut viewer, directory) = editing("#set pagsgse()\n#set tixt()\nText.\n");
    assert!(viewer.refused_up());
    press(&mut viewer, "x");
    assert_eq!(viewer.text(), Some("#set tixt()\nText.\n"));
    landed(&mut viewer);
    assert!(viewer.refused_up(), "{}", said(&viewer));
    assert!(said(&viewer).contains("“tixt”"), "{}", said(&viewer));
    press(&mut viewer, "x");
    assert_eq!(problems(&mut viewer), Vec::<String>::new());
    assert!(!viewer.refused_up());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_bracket_left_open_in_code_is_closed_where_its_paragraph_ends() {
    let text = "#table(columns: 2, [a], [b]\n\nMore.\n";
    let (mut viewer, directory) = editing(text);
    let status = said(&viewer);
    assert!(
        status.contains("opened here and never closed (+2 more)"),
        "{status}"
    );
    press(&mut viewer, "x");
    assert_eq!(
        viewer.text(),
        Some("#table(columns: 2, [a], [b])\n\nMore.\n")
    );
    assert_eq!(problems(&mut viewer), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn marks_closed_and_never_opened_or_raw_text_left_open_are_taken_out() {
    for (text, then) in [
        ("Text ] here.\n", "Text  here.\n"),
        ("Text `raw here.\n\nMore.\n", "Text raw here.\n\nMore.\n"),
    ] {
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "x");
        assert_eq!(viewer.text(), Some(then));
        assert_eq!(problems(&mut viewer), Vec::<String>::new(), "{text:?}");
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn what_typst_says_is_said_for_a_writer() {
    for (message, said) in [
        (
            "unknown variable: pagsgse",
            "Typst knows nothing called “pagsgse”",
        ),
        (
            "label `<intro>` does not exist in the document",
            "nothing is labelled “intro”: the reference leads nowhere",
        ),
        (
            "file not found (searched at /home/someone/paper/missing.png)",
            "there is no file “missing.png”",
        ),
        (
            "cannot reference heading without numbering",
            "a heading is referred to, but headings are not numbered (gD numbers them)",
        ),
        (
            "unclosed delimiter",
            "something is opened here and never closed",
        ),
        ("condition is always true", "this loop would never end"),
        ("expected comma", "expected comma"),
    ] {
        assert_eq!(refused::for_writer(message), said);
    }
}

#[test]
fn turning_heading_numbers_off_where_sections_are_referred_to_says_why() {
    // The page stops changing; the status line says why, and `u` in
    // the panel takes the change back.
    let text = "#set heading(numbering: \"1.\")\n= Intro <intro>\n\nSee @intro.\n";
    let (mut viewer, directory) = editing(text);
    assert!(viewer.document.is_some());
    change(&mut viewer, "#set heading(numbering: \"1.\")\n", "");
    landed(&mut viewer);
    let status = said(&viewer);
    assert!(
        status.starts_with(
            "the page is behind the text: line 3 “See @intro.”: a heading is referred to"
        ),
        "{status}"
    );
    press(&mut viewer, ":why<CR>u");
    assert_eq!(viewer.text(), Some(text));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn an_agents_change_that_is_refused_is_said_so_on_the_page() {
    let (mut viewer, directory) = editing("Some words.\n");
    viewer
        .agent_replace("words", "#wrds(", false, None, false)
        .unwrap();
    landed(&mut viewer);
    let status = said(&viewer);
    assert!(
        status.starts_with("the page is behind the text: line 1"),
        "{status}"
    );
    press(&mut viewer, ":why<CR>u");
    assert_eq!(viewer.text(), Some("Some words.\n"));
    assert_eq!(problems(&mut viewer), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn in_a_long_paragraph_the_words_around_the_place_are_named() {
    let text = "= Intro <intro>\n\nThe argument is elementary and relies only on a contraction \
                estimate, which we state in @nowhere and prove at length later on.\n";
    let (viewer, directory) = editing(text);
    let status = said(&viewer);
    assert!(
        status.contains("line 3 “…we state in @nowhere and prove at…”"),
        "{status}"
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_panel_is_drawn_while_it_is_up_and_not_after() {
    let (mut viewer, directory) = editing("#set pagsgse()\nText.\n");
    let drawn = |viewer: &mut Viewer| {
        let scene = viewer.scene(&|_| false);
        scene
            .quads
            .iter()
            .filter(|quad| quad.texture == TextureId(hash128(&("solid", ERROR_MARK))))
            .count()
    };
    let with = drawn(&mut viewer);
    press(&mut viewer, "<Esc>");
    let without = drawn(&mut viewer);
    assert_eq!(with, without + 1, "the panel's red edge");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_reference_begun_from_the_menu_and_left_is_got_out_of_from_the_page() {
    // See `left_behind.rs`: the word behind the `@` became the name of
    // a label there is none of. The panel takes the `@` out; the word
    // stays.
    let text = "= Intro <intro>\n\nSome words here.\n";
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "gsgs");
    viewer
        .editor
        .as_mut()
        .unwrap()
        .set_cursor(text.find("words").unwrap());
    press(&mut viewer, "i#ref<CR><Esc>");
    let left = viewer.text().unwrap_or_default().to_owned();
    if problems(&mut viewer).is_empty() {
        // Mended where it was made: nothing to get out of.
        let _ = std::fs::remove_dir_all(directory);
        return;
    }
    press(&mut viewer, ":why<CR>x");
    assert_eq!(
        problems(&mut viewer),
        Vec::<String>::new(),
        "{:?}",
        viewer.text()
    );
    let then = viewer.text().unwrap_or_default();
    assert!(
        then.contains("words"),
        "{left:?} became {then:?}: {}",
        said(&viewer)
    );
    let _ = std::fs::remove_dir_all(directory);
}
