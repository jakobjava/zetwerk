//! What is typed as a name or next to one on the page: `#` and what
//! follows it, `@` and a reference, and the edges of things the page
//! shows as one (a symbol written with a name, strong text, a label, an
//! equation). What the page shows is what was typed, and the document
//! is typeset all the time.
//!
//! The rules: `#` and `@` ask what is meant (a list comes up); what is
//! typed behind them is shown as typed (`\#xyz`, `\@word`) until
//! something is chosen from the list, and stays so where nothing is. A
//! reference typed out to a name something is labelled with refers to
//! it once a blank, a mark or Escape ends it. What is typed against
//! something that stood there (a name, a reference, strong text) is
//! never read as more of it. `$` begins an equation; `$` again right
//! after it makes it a dollar sign.

use super::*;

/// The text with `|` where the cursor is.
fn marked(viewer: &Viewer) -> String {
    let mut text = viewer.text().unwrap_or_default().to_owned();
    let cursor = viewer.editor.as_ref().map_or(0, Editor::cursor);
    text.insert(cursor.min(text.len()), '|');
    text
}

/// Presses `keys` one by one, typesetting after each.
fn paced(viewer: &mut Viewer, keys: &str) {
    for key in keys::parse(keys).unwrap_or_else(|err| panic!("{err}")) {
        viewer.key(key);
        compile(viewer);
    }
}

/// All the text the pages show, in the order it was laid out.
fn page_text(viewer: &Viewer) -> String {
    use typst::layout::{Frame, FrameItem};
    fn walk(frame: &Frame, into: &mut String) {
        for (_, item) in frame.items() {
            match item {
                FrameItem::Group(group) => walk(&group.frame, into),
                FrameItem::Text(text) => into.push_str(&text.text),
                _ => {}
            }
        }
    }
    let mut text = String::new();
    for page in viewer
        .document
        .iter()
        .flat_map(super::super::local::Pages::pages)
    {
        walk(&page.frame, &mut text);
    }
    text
}

/// What came of keys typed on a page.
struct Typed {
    /// The text then.
    text: String,
    /// What Typst says is wrong with it, typeset whole.
    problems: Vec<String>,
    /// What the pages show then.
    page: String,
}

/// `keys` typed on the pages of `text` with the cursor put at the first
/// `at`, each typeset before the next (`pace`) or all at once.
fn typed(text: &str, at: &str, keys: &str, pace: bool) -> Typed {
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "gsgs");
    compile(&mut viewer);
    let cursor = text
        .find(at)
        .unwrap_or_else(|| panic!("{at:?} in {text:?}"));
    if let Some(editor) = viewer.editor.as_mut() {
        editor.set_cursor(cursor);
    }
    if pace {
        paced(&mut viewer, keys);
    } else {
        press(&mut viewer, keys);
    }
    compile(&mut viewer);
    let typed = Typed {
        text: viewer.text().unwrap_or_default().to_owned(),
        problems: viewer.state().problems,
        page: page_text(&viewer),
    };
    let _ = std::fs::remove_dir_all(directory);
    typed
}

/// Runs each of `cases` (a text, where the cursor is put, the keys, the
/// text expected, what the pages are to show of it) at a writer's pace
/// and as fast as keys come, and says which came out otherwise or left
/// a text Typst refuses.
fn check(cases: &[(&str, &str, &str, &str, &str)]) {
    let mut wrong = Vec::new();
    for (text, at, keys, expected, shown) in cases {
        for pace in [true, false] {
            let typed = typed(text, at, keys, pace);
            let case = format!("{text:?} at {at:?}, {keys} (paced: {pace})");
            if typed.text != *expected {
                wrong.push(format!("{case}: {:?}, not {expected:?}", typed.text));
            }
            if !typed.problems.is_empty() {
                wrong.push(format!(
                    "{case}: {:?} refused: {:?}",
                    typed.text, typed.problems
                ));
            }
            if !typed.page.contains(shown) {
                wrong.push(format!(
                    "{case}: the page shows {:?}, not {shown:?}",
                    typed.page
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn a_hash_left_without_a_choice_stays_what_was_typed() {
    let text = "Some words.\n";
    check(&[
        (text, "words", "i#xyz <Esc>", "Some \\#xyz words.\n", "#xyz"),
        (
            text,
            "words",
            "i#xyz<Esc>",
            "Some \\#xyzwords.\n",
            "#xyzwords",
        ),
        (text, "words", "A #<Esc>", "Some words. \\#\n", "#"),
        (
            text,
            "words",
            "A #sym.<Esc>",
            "Some words. \\#sym.\n",
            "#sym.",
        ),
        (text, "words", "A #fo<Esc>", "Some words. \\#fo\n", "#fo"),
        // A name that only begins one of the menu: as typed.
        (
            text,
            "words",
            "A #foot more<Esc>",
            "Some words. \\#foot more\n",
            "#foot",
        ),
        // Backspace back over the sign takes it.
        (text, "words", "A #x<BS><BS>.<Esc>", "Some words. .\n", "."),
    ]);
}

#[test]
fn a_name_being_typed_behind_a_hash_shows_as_typed_meanwhile() {
    // Key by key: the page shows what was typed, the list follows it,
    // and nothing says the text is refused, nor waits to say it.
    let (mut viewer, directory) = editing("Some words.\n");
    press(&mut viewer, "gsgs");
    compile(&mut viewer);
    press(&mut viewer, "A");
    for key in keys::parse(" #fo").unwrap_or_default() {
        viewer.key(key);
        compile(&mut viewer);
        let text = viewer.text().unwrap_or_default().to_owned();
        assert!(viewer.state().problems.is_empty(), "{text:?}");
        assert!(viewer.is_compiled(), "{text:?}");
        assert_eq!(viewer.refused_status(), None, "{text:?}");
        assert!(!viewer.refused_up(), "{text:?}");
        let _ = viewer.scene(&|_| false);
        assert!(viewer.margin_texts(0).is_empty(), "{text:?}");
    }
    assert!(page_text(&viewer).ends_with("words. #fo"));
    assert_eq!(marked(&viewer), "Some words. \\#fo|\n");
    let listed = viewer.completion.as_ref().and_then(|completion| {
        let chosen = completion.candidates.get(completion.chosen)?;
        Some(chosen.candidate.label.clone())
    });
    assert_eq!(listed.as_deref(), Some("footnote"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_name_chosen_after_a_hash_is_put_in() {
    let text = "Some words.\n";
    check(&[
        (
            text,
            "words",
            "A #sym.alp<Tab> b<Esc>",
            "Some words. #sym.alpha b\n",
            "α",
        ),
        // What is typed right behind it is text, not more of its name.
        (
            text,
            "words",
            "A #sym.alp<Tab>b<Esc>",
            "Some words. #sym.alpha;b\n",
            "αb",
        ),
        (
            text,
            "words",
            "i#sym.alp<Tab><Esc>",
            "Some #sym.alpha;words.\n",
            "αwords",
        ),
        (
            text,
            "words",
            "A #foot<CR>note<Esc>",
            "Some words. #footnote[note]\n",
            "note",
        ),
        (
            text,
            "words",
            "A #eq<CR>x<Esc>",
            "Some words. \n\n$ x $\n",
            "𝑥",
        ),
        (
            text,
            "words",
            "A #hash<CR> sign<Esc>",
            "Some words. \\# sign\n",
            "# sign",
        ),
    ]);
}

#[test]
fn a_reference_typed_out_refers_where_something_is_labelled_so() {
    let text = "#set heading(numbering: \"1.\")\n= Intro <intro>\n\nSome words here.\n";
    let then = |rest: &str| text.replace("here.\n", rest);
    let words = |with: &str| text.replace("words", with);
    check(&[
        (text, "here", "A @intro<Esc>", &then("here. @intro\n"), "1"),
        (
            text,
            "here",
            "A @intro.<Esc>",
            &then("here. @intro.\n"),
            "1.",
        ),
        (
            text,
            "here",
            "A @intro, more<Esc>",
            &then("here. @intro, more\n"),
            "1, more",
        ),
        (
            text,
            "here",
            "A @in<Tab>s<Esc>",
            &then("here. @intro\\s\n"),
            "1s",
        ),
        // Nothing is labelled so: it is what was typed.
        (
            text,
            "here",
            "A @nowhere <Esc>",
            &then("here. \\@nowhere \n"),
            "@nowhere",
        ),
        (text, "here", "A @<Esc>", &then("here. \\@\n"), "@"),
        // Before a word, the word stays out of its name.
        (
            text,
            "words",
            "i@intro<Esc>",
            &words("@intro\\words"),
            "1words",
        ),
        // Asked for from the menu and left: the sign, as it showed.
        (
            text,
            "words",
            "i#ref<CR><Esc>",
            &words("\\@words"),
            "@words",
        ),
    ]);
}

#[test]
fn what_is_typed_against_a_name_stays_out_of_it() {
    let reference = "#set heading(numbering: \"1.\")\n= Intro <intro>\n\nSee @intro.\n";
    let symbol = "Some #sym.alpha words.\n";
    check(&[
        (
            symbol,
            " words",
            "ib<Esc>",
            "Some #sym.alpha;b words.\n",
            "αb",
        ),
        (
            symbol,
            " words",
            "i.b<Esc>",
            "Some #sym.alpha;.b words.\n",
            "α.b",
        ),
        (
            reference,
            ".\n",
            "ix<Esc>",
            &reference.replace("@intro.", "@intro\\x."),
            "1x.",
        ),
        (
            reference,
            ".\n",
            "a<BS>.x<Esc>",
            &reference.replace("@intro.", "@intro.\\x"),
            "1.x",
        ),
        // A label put in from the menu: a blank ends it.
        (
            "One word.\n",
            "word",
            "A #label<CR>x more<Esc>",
            "One word. <x> more\n",
            "more",
        ),
        (
            "One word.\n",
            "word",
            "A #label<CR> more<Esc>",
            "One word.  more\n",
            "more",
        ),
        // A letter before strong text keeps its marks marks.
        (
            "One *two* three.\n",
            "*two",
            "iX<Esc>",
            "One X#strong[two] three.\n",
            "Xtwo",
        ),
        (
            "One _two_ three.\n",
            " _two",
            "aX<Esc>",
            "One X#emph[two] three.\n",
            "Xtwo",
        ),
    ]);
}

#[test]
fn a_dollar_sign_is_typed_twice() {
    let text = "One word.\n";
    check(&[
        (
            text,
            "word",
            "A costs $$5<Esc>",
            "One word. costs \\$5\n",
            "costs $5",
        ),
        (
            text,
            "word",
            "A costs $$ more<Esc>",
            "One word. costs \\$ more\n",
            "$ more",
        ),
        // In an equation with something in it, it ends it, as before.
        (
            text,
            "word",
            "A $x$ more<Esc>",
            "One word. $x$ more\n",
            "more",
        ),
    ]);
    // The status line says so when `$` is typed; one step undoes it.
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "gsgsA $");
    let said = viewer.message.as_ref().map(|message| message.text.clone());
    assert!(
        said.as_deref().is_some_and(|said| said.contains("$ again")),
        "{said:?}"
    );
    press(&mut viewer, "$5<Esc>u");
    assert_eq!(viewer.text(), Some(text));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn typing_at_an_end_of_a_line_goes_beside_its_equation() {
    check(&[
        ("One $x$\n", "One", "A new<Esc>", "One $x$ new\n", "new"),
        ("$x$ one\n", "one", "Inew <Esc>", "new $x$ one\n", "new"),
    ]);
    // Not where the equation is all of the line: it is typed into.
    let typed = typed("One\n\n$ x $\n\nTwo\n", "x", "A + y<Esc>", true);
    assert_eq!(typed.text, "One\n\n$ x + y $\n\nTwo\n");
    let (mut viewer, directory) = editing("One $x$\n");
    press(&mut viewer, "gsgs");
    compile(&mut viewer);
    press(&mut viewer, "A");
    assert_eq!(marked(&viewer), "One $x$|\n");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn every_mark_typed_anywhere_shows_as_typed() {
    // A character Typst gives a meaning to, typed alone at every kind of
    // place: the document typesets, and the page shows it.
    let places: &[(&str, &str)] = &[
        ("Some words.\n", "Some"),
        ("Some words.\n", " words"),
        ("Some words.\n", ".\n"),
        ("= A heading\n\nText.\n", " heading"),
        ("- an item\n- another\n", " item"),
        ("+ an item\n+ another\n", "another"),
        ("#table(columns: 2, [a cell], [b])\n", " cell"),
        ("#figure(rect(), caption: [A caption])\n", " caption]"),
        ("Text#footnote[a note] more.\n", " note"),
        ("#quote(block: true)[A quote]\n", " quote"),
        ("A $x$ b.\n", " b."),
        (
            "#set heading(numbering: \"1.\")\n= Intro <intro>\n\nSee @intro b.\n",
            " b.",
        ),
        ("Some *strong* words.\n", " words"),
    ];
    let marks = "*_`<>[](){}\\/~-+=.:\"'@#%";
    let mut wrong = Vec::new();
    for (text, at) in places {
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "gsgs");
        compile(&mut viewer);
        let before = page_text(&viewer);
        let place = text.find(at).unwrap_or(0);
        for mark in marks.chars() {
            if let Some(editor) = viewer.editor.as_mut() {
                editor.set_cursor(place);
            }
            let keys = match mark {
                '<' => "i<lt><Esc>".to_owned(),
                mark => format!("i{mark}<Esc>"),
            };
            press(&mut viewer, &keys);
            compile(&mut viewer);
            let after = viewer.text().unwrap_or_default().to_owned();
            let problems = viewer.state().problems;
            let page = page_text(&viewer);
            let more = |c: char| page.matches(c).count() > before.matches(c).count();
            let shown = match mark {
                // Typst's quotation marks, and behind a number its marks
                // of feet and inches.
                '\'' => more('\'') || more('’') || more('‘') || more('′'),
                '"' => more('"') || more('“') || more('”') || more('″'),
                mark => more(mark),
            };
            if !problems.is_empty() || !shown {
                wrong.push(format!(
                    "{mark:?} at {at:?} in {text:?}: {after:?} {problems:?}, page {page:?}"
                ));
            }
            press(&mut viewer, "u");
            compile(&mut viewer);
        }
        let _ = std::fs::remove_dir_all(directory);
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
