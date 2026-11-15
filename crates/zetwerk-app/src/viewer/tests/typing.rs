//! Everyday writing of prose on the page.
//!
//! What a writer does in the layout that shows only pages: Backspace at
//! the start of a paragraph and Delete at its end, deleting and changing
//! words in strong and emphasised text, in cells, captions and
//! footnotes. Each key is typeset before the next, as a writer at a
//! normal pace sees it. The text is shown with `|` where the cursor is.

use super::*;

fn cursor(viewer: &Viewer) -> usize {
    viewer.editor.as_ref().unwrap().cursor()
}

/// The text with `|` where the cursor is.
fn marked(viewer: &Viewer) -> String {
    let mut text = viewer.text().unwrap().to_owned();
    text.insert(cursor(viewer), '|');
    text
}

/// Presses `keys` one by one, typesetting after each.
fn paced(viewer: &mut Viewer, keys: &str) {
    for key in keys::parse(keys).unwrap() {
        viewer.key(key);
        compile(viewer);
    }
}

/// `keys` pressed on the pages of `text`; what the text is then, with
/// the cursor marked.
fn typed(text: &str, keys: &str) -> String {
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "gsgs");
    compile(&mut viewer);
    paced(&mut viewer, keys);
    let after = marked(&viewer);
    // And all of it is taken back by undo.
    paced(&mut viewer, "<Esc>u");
    assert_eq!(viewer.text(), Some(text), "after {keys:?} and u");
    let _ = std::fs::remove_dir_all(directory);
    after
}

#[test]
fn backspace_at_the_start_of_a_paragraph_joins_it_to_the_one_before() {
    // Not the last letter of the paragraph before: that was what went.
    let cases = [
        (
            "First para.\n\nSecond para.\n",
            "/Second<CR>i<BS>",
            "First para.|Second para.\n",
        ),
        (
            "Text before.\n\n= Heading\n\nText after.\n",
            "/Heading<CR>i<BS>",
            "Text before.|Heading\n\nText after.\n",
        ),
        (
            "- item one\n- item two\n",
            "/two<CR>0i<BS>",
            "- item one|item two\n",
        ),
        (
            "Para one.\nLine two.\n",
            "/Line<CR>i<BS>",
            "Para one.|Line two.\n",
        ),
        // The next Backspace takes what the page shows before it.
        (
            "First para.\n\nSecond para.\n",
            "/Second<CR>i<BS><BS>",
            "First para|Second para.\n",
        ),
    ];
    for (text, keys, expected) in cases {
        assert_eq!(typed(text, keys), expected, "{keys:?} in {text:?}");
    }
}

#[test]
fn delete_at_the_end_of_a_paragraph_joins_the_next_to_it() {
    let cases = [
        (
            "First para.\n\nSecond para.\n",
            "/First<CR>A<Del>",
            "First para.|Second para.\n",
        ),
        (
            "Text before.\n\n= Heading\n",
            "/before<CR>A<Del>",
            "Text before.|Heading\n",
        ),
        // A blank typed at the end stays: it parts the two.
        (
            "First para.\n\nSecond para.\n",
            "/First<CR>A <Del>",
            "First para. |Second para.\n",
        ),
        (
            "Some *bold* text here.\n\nNext one.\n",
            "/here<CR>A<Del>",
            "Some *bold* text here.|Next one.\n",
        ),
        // The next Delete takes what the page shows behind the cursor.
        (
            "First para.\n\nSecond para.\n",
            "/First<CR>A<Del><Del>",
            "First para.|econd para.\n",
        ),
    ];
    for (text, keys, expected) in cases {
        assert_eq!(typed(text, keys), expected, "{keys:?} in {text:?}");
    }
}

#[test]
fn backspace_and_delete_stay_in_their_cell_caption_or_footnote() {
    let table = "#table(columns: 2, [alpha], [beta], [gamma], [delta])\n";
    let footnote = "Some word#footnote[A note here.] and more.\n";
    let figure = "#figure(rect[x], caption: [The caption.])\n\nText after.\n";
    let quote = "#quote(block: true)[Quoted words.]\n\nPlain after.\n";
    let cases = [
        (table, "/beta<CR>i<BS>"),
        (table, "/alpha<CR>ea<Del>"),
        (table, "/gamma<CR>i<C-w>"),
        (footnote, "/here<CR>A<Del><Del>"),
        (figure, "/The<CR>i<BS>"),
        (figure, "/Text<CR>i<BS>"),
        (quote, "/Quoted<CR>i<BS>"),
        (quote, "/Plain<CR>i<BS>"),
        (quote, "/words<CR>A<Del>"),
    ];
    for (text, keys) in cases {
        let after = typed(text, keys);
        assert_eq!(after.replace('|', ""), text, "{keys:?} in {text:?}");
    }
}

#[test]
fn joining_paragraphs_keeps_strong_text_strong() {
    // Behind a letter the star would be a star, and all that follows
    // strong: nothing is joined then.
    let text = "Hello *world*\n\nNext para.\n";
    for keys in ["/world<CR>A<Del>", "/Next<CR>i<BS>"] {
        assert_eq!(typed(text, keys).replace('|', ""), text, "{keys:?}");
    }
}

#[test]
fn deleting_words_leaves_the_marks_of_text_set_off() {
    let cases = [
        (
            "Plain *bold words* after.\n",
            "/words<CR>D",
            "Plain *bold |*\n",
        ),
        (
            "Text with _emph words_ in it.\n",
            "/words<CR>de",
            "Text with _emph |_ in it.\n",
        ),
        (
            "Text with _emph words_ in it.\n",
            "/words<CR>cwX<Esc>",
            "Text with _emph |X_ in it.\n",
        ),
        (
            "Plain *bold words* after.\n",
            "/Plain<CR>vtwd",
            "|*words* after.\n",
        ),
        (
            "Some word#footnote[A note.] and more text.\n",
            "/and<CR>db",
            "Some word#footnote[A note|]and more text.\n",
        ),
        // Where the mark would come to stand in a word, nothing goes.
        (
            "Plain *bold words* after.\n",
            "/after<CR>db",
            "Plain *bold words* |after.\n",
        ),
    ];
    for (text, keys, expected) in cases {
        assert_eq!(typed(text, keys), expected, "{keys:?} in {text:?}");
    }
}

#[test]
fn what_is_yanked_on_the_page_has_no_marks_without_their_partners() {
    let (mut viewer, directory) = editing("Text with _emph words_ in it.\n\nEnd.\n");
    press(&mut viewer, "gsgs");
    compile(&mut viewer);
    paced(&mut viewer, "/words<CR>yeG$p");
    assert_eq!(
        viewer.text(),
        Some("Text with _emph words_ in it.\n\nEnd.words\n")
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn put_goes_behind_or_before_what_the_cursor_is_on() {
    // Not between the backslash and the star, nor into the name of what
    // a reference refers to: both would change what the page shows.
    let text = "#set heading(numbering: \"1.\")\n= Intro <intro>\n\n\
                A star \\* here and @intro there.\n\nSecond para.\n";
    let cases = [
        ("/Second<CR>yiw/star<CR>wp", "\\*Second"),
        ("/Second<CR>yiw/star<CR>wP", "Second\\*"),
        ("/Second<CR>yiw/\\=intro there<CR>p", "@introSecond"),
        ("/Second<CR>yiw/\\=intro there<CR>P", "Second@intro"),
    ];
    for (keys, expected) in cases {
        let after = typed(text, keys).replace('|', "");
        assert!(after.contains(expected), "{keys:?} gave {after:?}");
    }
}

#[test]
fn backspace_takes_a_blank_typed_at_the_end_first() {
    // The blank shows as nothing at the end of the paragraph; the letter
    // before it went instead, and the blank stayed.
    let text = "Write some words.\n\nNext.\n";
    assert_eq!(
        typed(text, "/words<CR>A more<BS><BS><BS><BS><BS>"),
        "Write some words.|\n\nNext.\n"
    );
    assert_eq!(
        typed(text, "/words<CR>A <BS><BS>"),
        "Write some words|\n\nNext.\n"
    );
}

#[test]
fn joining_and_putting_keep_strong_text_strong() {
    let cases = [
        // `gJ` would put the star between two letters.
        (
            "Hello *world*\n\nNext para.\n",
            "/world<CR>gJ",
            "Hello *|world*\n\nNext para.\n",
        ),
        (
            "Hello *world*\n\nNext para.\n",
            "/world<CR>J",
            "Hello *world*| Next para.\n",
        ),
        // What is put over a selection leaves the star that closes.
        (
            "Plain *bold words* after.\n",
            "/Plain<CR>yiw/bold<CR>vt.p",
            "Plain *Plai|n*.\n",
        ),
        // Unless what is put in would come to stand in a word with it.
        (
            "Plain *bold words* after.\n",
            "/Plain<CR>yiw/bold<CR>vtap",
            "Plain *bold words*| after.\n",
        ),
    ];
    for (text, keys, expected) in cases {
        assert_eq!(typed(text, keys), expected, "{keys:?} in {text:?}");
    }
}

#[test]
fn enter_in_strong_text_closes_it_and_begins_it_again() {
    // A paragraph's end ends strong text: left open, it was an error and
    // the page stopped changing.
    let cases = [
        (
            "Some *bold words here* end.\n",
            "/words<CR>i<CR>",
            "Some *bold *\n\n*|words here* end.\n",
        ),
        (
            "Some _emph words_ end.\n",
            "/words<CR>i<CR>",
            "Some _emph _\n\n_|words_ end.\n",
        ),
        (
            "- item *bold words*\n",
            "/words<CR>i<CR>",
            "- item *bold *\n- *|words*\n",
        ),
    ];
    for (text, keys, expected) in cases {
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "gsgs");
        compile(&mut viewer);
        paced(&mut viewer, keys);
        assert_eq!(marked(&viewer), expected, "{keys:?} in {text:?}");
        assert!(viewer.state().problems.is_empty(), "{keys:?}");
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn backspace_leaves_labels_and_code_before_a_paragraph_alone() {
    // The heading's label in the middle of a heading names nothing that
    // can be referred to; text behind `#let x = 1` is read as part of it.
    let cases = [
        "#set heading(numbering: \"1.\")\n= Intro <intro>\n\nText after @intro.\n",
        "#let x = 1\n\nText after.\n",
        "#let x = 1\nText after.\n",
    ];
    for text in cases {
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "gsgs");
        compile(&mut viewer);
        paced(&mut viewer, "/Text<CR>i<BS><BS><BS>");
        assert!(viewer.state().problems.is_empty(), "{:?}", viewer.text());
        assert!(
            viewer.text().unwrap().contains("Text after"),
            "{:?}",
            viewer.text()
        );
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn backspace_and_delete_take_nothing_into_a_comment() {
    // The paragraph behind a comment went into it, and from the page;
    // the next Backspace took a letter of the comment.
    let cases = [
        ("// note\nText here.\n", "/Text<CR>i<BS><BS>"),
        ("/* note */\nText here.\n", "/Text<CR>i<BS><BS>"),
        ("Text here.\n// note\n", "/here<CR>A<Del><Del>"),
        ("Text here.\n/* note */\n", "/here<CR>A<Del><Del>"),
    ];
    for (text, keys) in cases {
        let after = typed(text, keys);
        assert_eq!(after.replace('|', ""), text, "{keys:?} in {text:?}");
    }
    // Nor, typing faster than the page is typeset, into code.
    for (text, keys) in [
        ("#let x = 1\nText after.\n", "/Text<CR>ix<BS><BS><BS><Esc>"),
        (
            "Text after.\n#let x = 1\n",
            "/after<CR>Ax<BS><Del><Del><Esc>",
        ),
    ] {
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "gsgs");
        compile(&mut viewer);
        press(&mut viewer, keys);
        assert_eq!(viewer.text(), Some(text), "{keys:?}");
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn a_heading_with_a_label_is_not_joined_to_the_paragraph_before() {
    // As text of the paragraph its label named text, and the reference
    // to it was an error.
    let text = "#set heading(numbering: \"1.\")\nBefore.\n\n= Intro <intro>\n\nSee @intro.\n";
    for keys in ["/Before<CR>A<Del>", "/Intro<CR>i<BS>", "/Before<CR>J"] {
        assert_eq!(typed(text, keys).replace('|', ""), text, "{keys:?}");
    }
}

#[test]
fn enter_at_the_start_of_a_heading_keeps_it_a_heading() {
    // The mark stayed behind as an empty heading, and the heading's text
    // became a paragraph.
    let text = "Before.\n\n= Head one\n\nText.\n";
    assert_eq!(
        typed(text, "/Head<CR>i<CR>"),
        "Before.\n\n\n\n= |Head one\n\nText.\n"
    );
    assert_eq!(
        typed(text, "/Head<CR>i<CR>x"),
        "Before.\n\n\n\n= x|Head one\n\nText.\n"
    );
}

#[test]
fn a_parenthesis_typed_behind_a_footnote_or_a_table_is_one() {
    // It called what the footnote or the table gives: an error, and the
    // page stopped changing.
    let cases = [
        ("Word#footnote[Note.] after.\n", "/after<CR>hi(x)"),
        (
            "#table(columns: 2, [a1], [b1])\n\nAfter.\n",
            "/b1<CR>A<CR>(see)",
        ),
    ];
    for (text, keys) in cases {
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "gsgs");
        compile(&mut viewer);
        paced(&mut viewer, keys);
        assert!(viewer.state().problems.is_empty(), "{:?}", viewer.text());
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
#[ignore = "probe"]
fn probe() {
    let cases: Vec<(String, String)> =
        std::fs::read_to_string(std::env::var("PROBE").unwrap_or_else(|_| "/dev/null".to_owned()))
            .unwrap()
            .split("\n=====\n")
            .filter_map(|case| {
                let (doc, keys) = case.split_once("\n-----\n")?;
                Some((doc.to_owned() + "\n", keys.trim_end().to_owned()))
            })
            .collect();
    for (doc, keys) in cases {
        let (mut viewer, directory) = editing(&doc);
        press(&mut viewer, "gsgs");
        compile(&mut viewer);
        for line in keys.lines() {
            let fast = line.starts_with('!');
            let line = line.trim_start_matches('!');
            if fast {
                press(&mut viewer, line);
                compile(&mut viewer);
            } else {
                paced(&mut viewer, line);
            }
            let state = viewer.state();
            let mode = state.editing.map(|e| e.mode);
            println!(
                "--- {line:?} {mode:?} {:?}\n{}",
                state.problems,
                marked(&viewer)
            );
        }
        println!("=================");
        let _ = std::fs::remove_dir_all(directory);
    }
}
