//! The lessons of `:tutor`, played: each lesson is opened as `:tutor`
//! opens it, and the keys of each of its steps are typed into it, with a
//! compile where the window would make one. Each step has to be undone
//! before its keys and done after them: a lesson that names a key that
//! no longer does what it says fails here. And `:help` and `K`.

use std::time::Instant;

use super::super::start::Opening;
use super::super::tutor::{PAUSE, lessons};
use super::*;

/// A viewer in a 1000×620 window editing the file at `path`, with its
/// state in `state`, as the window opens it.
fn open_at(path: &Path, state: &Path) -> Viewer {
    let env = Environment::new(FontSet::Embedded);
    let project = Project::open(Arc::clone(&env), path).unwrap();
    let (editor, opened) = Editor::open(path, state, project.main()).unwrap();
    let mut viewer = Viewer::new(env, project, (1000, 620), 1.0)
        .unwrap_or_else(|| panic!("viewer sets up"))
        .editing(editor, &opened);
    viewer.state_dir = Some(state.to_owned());
    viewer.panes = Panes::Page;
    viewer.arrange();
    compile(&mut viewer);
    viewer.welcome(Opening::Document, None);
    viewer
}

/// Lesson `number` opened by `:tutor <number>` in a window on an empty
/// document of `directory`: the viewer of the lesson's copy.
fn lesson(directory: &Path, number: usize) -> Viewer {
    let (mut viewer, _) = open(directory);
    viewer.execute(&format!("tutor {number}"));
    let (path, new) = viewer.take_wanted_document().unwrap_or_else(|| {
        panic!(
            "lesson {number} is opened: {:?}",
            viewer.message.as_ref().map(|m| &m.text)
        )
    });
    assert!(!new);
    drop(viewer);
    open_at(&path, &directory.join("state"))
}

/// Types `keys` as someone would: compiling where the window would at
/// once, and at the end, as at a pause.
fn play(viewer: &mut Viewer, keys: &str) {
    for chunk in keys.split(PAUSE) {
        for key in keys::parse(chunk).unwrap_or_else(|err| panic!("{chunk}: {err}")) {
            viewer.key(key);
            match viewer.take_edited() {
                None | Some(Edited::Typing) => {}
                Some(_) => compile(viewer),
            }
        }
        if !viewer.is_compiled() {
            compile(viewer);
        }
    }
    let _ = viewer.take_edited();
}

/// What is known of the viewer, for a step that failed.
fn situation(viewer: &Viewer) -> String {
    let editor = viewer.editor.as_ref().unwrap();
    let (text, cursor) = (editor.text(), editor.cursor());
    let mut from = cursor.saturating_sub(40);
    while !text.is_char_boundary(from) {
        from -= 1;
    }
    let mut to = (cursor + 40).min(text.len());
    while !text.is_char_boundary(to) {
        to += 1;
    }
    format!(
        "mode {:?}, message {:?}, problems {:?}, picker {:?}\naround the cursor: {:?}‸{:?}\ntext:\n{}",
        editor.mode(),
        viewer.message.as_ref().map(|message| &message.text),
        viewer.state().problems,
        viewer.picker.as_ref().map(|picker| picker.title()),
        &text[from..cursor],
        &text[cursor..to],
        text.split("/* tutor").next().unwrap_or(text)
    )
}

#[test]
fn every_lesson_is_done_by_the_keys_it_names() {
    let all = lessons("en");
    assert!(!all.is_empty());
    let mut failed = Vec::new();
    for (index, taught) in all.iter().enumerate() {
        let number = index + 1;
        let directory = directory("");
        let mut viewer = lesson(&directory, number);
        let problems = viewer.state().problems;
        if viewer.steps_done() != Some(0) || !problems.is_empty() {
            failed.push(format!(
                "{}: not at its first step, or not typeset: {problems:?}\n{}",
                taught.name,
                situation(&viewer)
            ));
            continue;
        }
        for (step, said) in taught.steps.iter().enumerate() {
            if viewer.steps_done() != Some(step) {
                failed.push(format!(
                    "{}: “{}” was done before its keys\n{}",
                    taught.name,
                    said.say,
                    situation(&viewer)
                ));
                break;
            }
            play(&mut viewer, &said.keys);
            if viewer.steps_done() != Some(step + 1) {
                failed.push(format!(
                    "{}: “{}” is not done by {}\n{}",
                    taught.name,
                    said.say,
                    said.keys,
                    situation(&viewer)
                ));
                break;
            }
        }
        let progress = std::fs::read_to_string(directory.join("state/tutor/progress"));
        if failed.is_empty() {
            assert!(
                progress.unwrap_or_default().contains(taught.name),
                "{} is noted as done",
                taught.name
            );
        }
        let _ = std::fs::remove_dir_all(directory);
    }
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}

#[test]
fn the_tutor_goes_on_where_the_reader_left_off_and_never_touches_their_files() {
    let directory = directory("My own text.\n");
    let state = directory.join("state");
    let (mut viewer, _) = open(&directory);
    // The first lesson first.
    viewer.execute("tutor");
    let (first, _) = viewer.take_wanted_document().unwrap();
    assert!(first.starts_with(state.join("tutor")), "{first:?}");
    // A lesson done is not offered again.
    std::fs::write(
        state.join("tutor/progress"),
        format!("{}\n", lessons("en")[0].name),
    )
    .unwrap();
    viewer.execute("tutor");
    let (second, _) = viewer.take_wanted_document().unwrap();
    assert_ne!(first, second);
    // The list says which are done.
    viewer.execute("tutor list");
    let (title, listed, chosen) = viewer.picker.as_ref().unwrap().listing();
    assert_eq!(title, "lessons");
    assert!(listed[0].ends_with("\tdone"), "{listed:?}");
    assert_eq!(chosen, 1);
    press(&mut viewer, "<Esc>");
    // Nothing but the lessons' copies was made, and the document is as it was.
    assert_eq!(
        std::fs::read_to_string(directory.join("main.typ")).unwrap(),
        "My own text.\n"
    );
    // No such lesson.
    viewer.execute("tutor 999");
    assert!(viewer.message.as_ref().is_some_and(|message| message.alert));
    assert_eq!(viewer.take_wanted_document(), None);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_lesson_begun_again_is_its_text_as_it_came_and_the_hint_says_the_keys() {
    let directory = directory("");
    let mut viewer = lesson(&directory, 1);
    let first = lessons("en")[0].clone();
    assert!(viewer.tutor_status().contains("step 1/"));
    press(&mut viewer, "<C-t>");
    let said = viewer.message.as_ref().map(|message| message.text.clone());
    assert_eq!(
        said,
        Some(format!("{}: {}", first.steps[0].say, first.steps[0].hint))
    );
    // Something changed, then the lesson begun again: as it came.
    press(&mut viewer, "ggdd");
    assert_ne!(viewer.text(), Some(first.text.as_str()));
    viewer.execute("tutor 1");
    assert_eq!(viewer.take_wanted_document(), None, "it is here already");
    assert_eq!(viewer.text(), Some(first.text.as_str()));
    assert_eq!(viewer.steps_done(), Some(0));
    // Another lesson: this one's copy is saved first, so that nothing
    // stands in the way.
    press(&mut viewer, "x");
    viewer.execute("tutor 2");
    assert!(viewer.take_wanted_document().is_some());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_first_start_says_once_that_there_is_a_tutor() {
    let directory = directory("Text.\n");
    let state = directory.join("state");
    let _ = std::fs::remove_dir_all(&state);
    super::super::tutor::note_first_run(&state);
    let (mut viewer, _) = open(&directory);
    viewer.welcome(Opening::Document, None);
    let said = viewer
        .message
        .as_ref()
        .map(|message| message.text.clone())
        .unwrap_or_default();
    assert!(said.contains(":tutor"), "{said}");
    drop(viewer);
    // Never again: the state directory is there now.
    super::super::tutor::note_first_run(&state);
    let (mut viewer, _) = open(&directory);
    viewer.welcome(Opening::Document, None);
    let said = viewer
        .message
        .as_ref()
        .map(|message| message.text.clone())
        .unwrap_or_default();
    assert!(!said.contains(":tutor"), "{said}");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn help_says_what_a_key_does_and_enter_does_it() {
    let (mut viewer, directory) = editing("= Head\n\nSome words $x^2$ here.\n");
    press(&mut viewer, "gsgs");
    press(&mut viewer, ":help dd<CR>");
    let detail = viewer.help_detail().unwrap_or_default();
    assert!(detail.contains("paragraph"), "{detail}");
    let picker = viewer.picker.as_ref().unwrap();
    assert_eq!(picker.title(), "help");
    // Typing narrows it, as any list; Enter does what the entry does.
    press(&mut viewer, "word count<CR>");
    let said = viewer
        .message
        .as_ref()
        .map(|message| message.text.clone())
        .unwrap_or_default();
    assert!(said.contains("words"), "{said}");
    // `g?` is the same list.
    press(&mut viewer, "g?");
    assert_eq!(
        viewer.picker.as_ref().map(|picker| picker.title()),
        Some("help")
    );
    press(&mut viewer, "<Esc>");
    // A word nothing is about: the list all the same, and a word why.
    press(&mut viewer, ":help zzyzx<CR>");
    assert!(viewer.picker.is_some());
    press(&mut viewer, "<Esc>");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn question_mark_on_the_list_of_what_follows_gives_help_on_those_keys() {
    let (mut viewer, directory) = editing("Some words.\n");
    press(&mut viewer, "gsgs]");
    assert!(viewer.hints_due(Instant::now() + std::time::Duration::from_secs(2)));
    press(&mut viewer, "?");
    let (title, listed, _) = viewer.picker.as_ref().unwrap().listing();
    assert_eq!(title, "help");
    assert!(listed.iter().all(|entry| entry.contains(']')), "{listed:?}");
    press(&mut viewer, "<Esc>");
    // Behind an operator `?` searches backwards, as it always did.
    press(&mut viewer, "d");
    viewer.hints_due(Instant::now() + std::time::Duration::from_secs(2));
    press(&mut viewer, "?");
    assert!(viewer.picker.is_none());
    press(&mut viewer, "<Esc><Esc>");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn k_says_what_the_thing_under_the_cursor_is() {
    let text = "= Head\n\nWords $x^2$ and *bold* text#footnote[A note.].\n\n\
                #figure(rect(), caption: [A box.])\n\n#table(columns: 2, [a], [b])\n";
    let (mut viewer, directory) = editing(text);
    let mut said = |keys: &str| {
        press(&mut viewer, keys);
        let told = viewer.tooltip.clone().unwrap_or_default();
        press(&mut viewer, "<Esc>");
        told
    };
    assert!(said("gg/\\=x^2<CR>K").contains("equation"));
    assert!(said("gg/\\=Head<CR>K").contains("heading"));
    assert!(said("gg/\\=bold<CR>K").contains("bold"));
    assert!(said("gg/\\=A note<CR>K").contains("footnote"));
    assert!(said("gg/\\=A box<CR>K").contains("figure"));
    assert!(said("gg/\\=columns<CR>K").contains("table"));
    let _ = std::fs::remove_dir_all(directory);
}
