//! Vim's keys on a page, and the keys of the editor's own features
//! living together: what a key does to the text, against what Vim would
//! do to the text as the page shows it.

use std::ops::Not;

use super::*;

const PAGE: &str = "#set page(width: 300pt, height: 300pt)\n#set heading(numbering: \"1.\")\n";

/// The text after `keys`, typed on the page with the cursor put on the
/// first `needle` of `body` (which follows a page set-up).
fn on_page(body: &str, needle: &str, keys: &str) -> String {
    let text = format!("{PAGE}{body}");
    let (mut viewer, directory) = editing(&text);
    press(&mut viewer, "gsgs");
    let at = text
        .find(needle)
        .unwrap_or_else(|| panic!("{needle} in {body}"));
    viewer.editor.as_mut().unwrap().set_cursor(at);
    press(&mut viewer, keys);
    let after = viewer.text().unwrap().to_owned();
    let _ = std::fs::remove_dir_all(directory);
    after.strip_prefix(PAGE).unwrap_or(&after).to_owned()
}

/// Runs each of `cases` (a text, where the cursor is put, the keys, the
/// text expected) and says which came out otherwise.
fn check(cases: &[(&str, &str, &str, &str)]) {
    let mut wrong = Vec::new();
    for (body, needle, keys, expected) in cases {
        let after = on_page(body, needle, keys);
        if after != *expected {
            wrong.push(format!(
                "{body:?} on {needle:?}, {keys}: {after:?}, not {expected:?}"
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn deleting_on_a_page_never_cuts_markup_in_two() {
    // What Vim would do to the text as the page shows it, as nearly as
    // the marks between allow: they stay.
    check(&[
        ("One $x + 2$ two.\n", "e $", "3x", "On$ + 2$ two.\n"),
        ("a *bc* d e.\n", "c*", "3x", "a *b* e.\n"),
        ("ab $x$ e.\n", "x$", "d2h", "a$x$ e.\n"),
        ("ab $x$ e.\n", "x$", "X", "ab$x$ e.\n"),
        ("one *two* three.\n", "one", "dft", "*wo* three.\n"),
        ("one *two* three.\n", "one", "ctwX<Esc>", "X*wo* three.\n"),
        ("one *two* three.\n", "one", "vfwd", "*o* three.\n"),
        ("a $x + y$ b.\n", "a $", "dfx", "$ + y$ b.\n"),
        ("Hello *world* foo.\n", "world", "d2w", "Hello **foo.\n"),
        // Where what is shown cannot be written so, nothing changes (and
        // the user is told why): a word against the star of strong text
        // leaves it unclosed.
        ("a *bc* d.\n", "c*", "2x", "a *bc* d.\n"),
        ("a *bc* d.\n", "bc", "X", "a *bc* d.\n"),
        ("a *bc* d.\n", "a *", "lrx", "a *bc* d.\n"),
        ("One $x^2$ two.\n", "e $", "3x", "One $x^2$ two.\n"),
        // What goes whole, goes whole as before.
        ("a *bc* d.\n", "a *", "4x", "** d.\n"),
        ("x $alpha + beta$ y.\n", "alpha", "3x", "x $$ y.\n"),
        ("Hello *world* foo.\n", "Hello", "dw", "*world* foo.\n"),
    ]);
}

#[test]
fn joining_behind_a_comment_keeps_the_comment_to_itself() {
    check(&[
        // The paragraph that follows was made part of the comment.
        ("One // c\n\nTwo.\n", "One", "J", "One // c\nTwo.\n"),
        // A heading ends with its line: there is no joining it so.
        ("= One // c\n\nTwo.\n", "One", "J", "= One // c\n\nTwo.\n"),
        ("One // c\n\nTwo.\n", "One", "gJ", "One // c\n\nTwo.\n"),
        ("One\n\nTwo.\n", "One", "J", "One Two.\n"),
    ]);
}

#[test]
fn deleting_the_line_of_an_item_takes_its_marker() {
    check(&[
        ("- one\n- two\n", "one", "dd", "- two\n"),
        ("- one\n- two\n- three\n", "one", "2dd", "- three\n"),
        ("+ one\n+ two\n", "one", "dd", "+ two\n"),
        ("= Head\n\nText.\n", "Head", "dd", "\nText.\n"),
        // Written anew, the marker stays.
        ("- one\n- two\n", "one", "ccnew<Esc>", "- new\n- two\n"),
    ]);
}

#[test]
fn a_cursor_put_on_what_shows_as_nothing_deletes_nothing_hidden() {
    // A list of headings, a label, `]i` put the cursor on the marks
    // themselves: keys there go by what is shown next to them.
    check(&[
        ("= Head\n\nText.\n", "= Head", "x", "= ead\n\nText.\n"),
        ("= Head\n\nText.\n", "= Head", "dw", "= \n\nText.\n"),
        ("Text <lab> more.\n", "<lab>", "x", "Text <lab> ore.\n"),
        ("Text <lab> more.\n", "Text", "]i2x", "Text <lab> re.\n"),
        ("Text <lab> more.\n", "Text", "]i\"_x", "Text <lab> ore.\n"),
    ]);
}

#[test]
fn ctrl_h_is_backspace() {
    check(&[
        ("abc\n", "abc", "A<C-h><Esc>", "ab\n"),
        ("abc def\n", "def", "<C-h>x", "abcdef\n"),
    ]);
}

#[test]
fn ctrl_w_and_ctrl_u_while_typing_take_what_is_shown() {
    check(&[
        ("One two three.\n", "three", "i<C-w><Esc>", "One three.\n"),
        (
            "One *two* three.\n",
            "three",
            "i<C-w><Esc>",
            "One **three.\n",
        ),
        ("One $x + y$ three.\n", "three", "i<C-u><Esc>", "three.\n"),
        ("One $x + y$ three.\n", "y$", "a<C-u><Esc>", "$$ three.\n"),
        ("One two three.\n", "three", "i<C-u><Esc>", "three.\n"),
    ]);
}

#[test]
fn keys_typed_into_the_prompt_of_a_thing_are_not_mapped() {
    let text = format!("{PAGE}Text <lab> more.\n");
    let (mut viewer, directory) = editing(&text);
    viewer.set_keymap(Keymap::parse([("n", "x")], []).0);
    press(&mut viewer, "gsgs");
    let at = text.find("Text").unwrap();
    viewer.editor.as_mut().unwrap().set_cursor(at);
    press(&mut viewer, "]ic<C-u>new<CR>");
    assert!(viewer.text().unwrap().contains("Text <new> more."));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_mapping_takes_the_count_and_the_register_typed_before_it() {
    let (mut viewer, directory) = editing("one two three four\n");
    viewer.set_keymap(Keymap::parse([("X", "dw")], []).0);
    // As Vim's `nmap X dw`: `2X` is `2dw`, `"aX` is `"adw`.
    press(&mut viewer, "2X");
    assert_eq!(viewer.text(), Some("three four\n"));
    press(&mut viewer, "\"aX$\"ap");
    assert_eq!(viewer.text(), Some("fourthree \n"));
    // Behind an operator a key is still the operator's: `dX` is no
    // `ddw`.
    press(&mut viewer, "0dX");
    assert_eq!(viewer.text(), Some("fourthree \n"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn putting_behind_a_thing_shown_as_one_puts_behind_all_of_it() {
    check(&[(
        "= H <h>\n\nSee @h now.\n",
        "now",
        "yiwF@p",
        "= H <h>\n\nSee @hnow now.\n",
    )]);
}

#[test]
fn ctrl_r_while_typing_puts_in_what_a_register_holds() {
    check(&[
        (
            "Hello world.\n",
            "world",
            "yiwI<C-r>\" <Esc>",
            "world Hello world.\n",
        ),
        (
            "Hello world.\n",
            "world",
            "\"ayiwI<C-r>a <Esc>",
            "world Hello world.\n",
        ),
        // A register that holds nothing puts in nothing.
        ("Hello world.\n", "world", "I<C-r>q<Esc>", "Hello world.\n"),
    ]);
}

#[test]
fn colon_s_replaces_what_is_written() {
    check(&[
        (
            "a world.\n\nworld world.\n",
            "a w",
            ":s/world/there/<CR>",
            "a there.\n\nworld world.\n",
        ),
        (
            "a world.\n\nworld world.\n",
            "a w",
            ":%s/world/there/g<CR>",
            "a there.\n\nthere there.\n",
        ),
        (
            "a world.\n\nworld world.\n",
            "a w",
            ":%s/World/x/i<CR>",
            "a x.\n\nx world.\n",
        ),
        ("a/b c.\n", "a/b", ":s/a\\/b/c/<CR>", "c c.\n"),
        // Not found: nothing changes.
        ("a world.\n", "a w", ":s/nothing/there/<CR>", "a world.\n"),
    ]);
}

/// What a test's clipboard was given, in place of the system's.
static COPIED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

#[test]
fn the_register_plus_is_the_clipboard() {
    let (mut viewer, directory) = editing("one two\n");
    viewer.clipboard.read = || Ok("pasted ".to_owned());
    viewer.clipboard.write = |text| {
        COPIED.lock().unwrap().push(text.to_owned());
        Ok(())
    };
    // Yanked into it: in the clipboard, once.
    press(&mut viewer, "\"+yiwwx");
    assert_eq!(*COPIED.lock().unwrap(), ["one"]);
    // Put from it: what the clipboard holds, though another program
    // copied it.
    press(&mut viewer, "0\"+P");
    assert_eq!(viewer.text(), Some("pasted one wo\n"));
    // What is put is not copied back; other registers are not copied.
    press(&mut viewer, "yiw\"ayiw");
    assert_eq!(*COPIED.lock().unwrap(), ["one"]);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn tab_takes_the_rest_of_a_word_shown_also_while_filling_in_a_starter() {
    let text = format!("{PAGE}‹Title›\n\nThe integral of it.\n\n‹Body›\n");
    let (mut viewer, directory) = editing(&text);
    // From the first place, the cursor on it, to the next.
    press(&mut viewer, "gsgsgg<Tab>");
    assert_eq!(viewer.editor.as_ref().unwrap().mode(), editor::Mode::Insert);
    press(&mut viewer, "int");
    assert!(viewer.shows_word_rest());
    press(&mut viewer, "<Tab>");
    assert!(
        viewer.text().unwrap().ends_with("\n\nintegral\n"),
        "{:?}",
        viewer.text()
    );
    // With nothing shown behind the cursor, on to the next place.
    press(&mut viewer, "<Tab>X<Esc>");
    let filled = format!("{PAGE}X\n\nThe integral of it.\n\nintegral\n");
    assert_eq!(viewer.text(), Some(filled.as_str()));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn escape_leaves_every_panel_and_list_and_the_next_key_is_vims() {
    // Whatever a key put up, Escape takes it down again, and `x` is
    // then Vim's `x` on the text.
    let body = "= Head <head>\n\nHello $x$ and @head.\n\n#figure(rect(), caption: [A box.])\n";
    let mut wrong = Vec::new();
    for keys in [
        "g?",
        "gO",
        "gh",
        "gD",
        "gB",
        "gF",
        "go",
        "gw",
        "gx",
        "gn",
        "gq",
        "z=",
        "K",
        "zp",
        ":labels<CR>",
        ":notes<CR>",
        ":invisibles<CR>",
        ":figure<CR>",
        ":styles<CR>",
        ":issues<CR>",
        ":files<CR>",
        ":recent<CR>",
        ":grid<CR>",
    ] {
        let after = on_page(body, "Hello", &format!("{keys}<Esc>x"));
        if !after.contains("\nello $x$") {
            wrong.push(format!("after {keys} and Escape, x: {after:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn every_command_the_list_of_all_runs_is_one() {
    let body = "= Head <head>\n\nHello $x$ and @head.\n\n#figure(rect(), caption: [A box.])\n";
    let text = format!("{PAGE}{body}");
    let (mut viewer, directory) = editing(&text);
    press(&mut viewer, "gsgs/Hello<CR>");
    // Not those that leave, write or export, nor those only begun for
    // the user to finish.
    let leaves = ["x", "q", "w", "pdf", "figure png", "figure svg"];
    let begun = |command: &str| command.is_empty() || command.ends_with([' ', '/']);
    for (keys, command) in super::super::palette::entries() {
        let Some(command) = command.filter(|command| !leaves.contains(command) && !begun(command))
        else {
            continue;
        };
        viewer.execute(command);
        let told = viewer.message.as_ref().map(|message| message.text.clone());
        assert!(
            !told
                .as_deref()
                .is_some_and(|told| told.starts_with("not a command")),
            "{keys}: {told:?}"
        );
        press(&mut viewer, "<Esc><Esc>");
        viewer.message = None;
    }
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn no_key_on_a_page_breaks_the_markup() {
    // From every place shown, keys that delete, change, join and put:
    // what Typst could read before, it reads after.
    let body = "= Head <head>\n\nOne *two* and _three_ $x^2 + alpha$ see @head.\n\
                - item *bold* end\n- next `raw` x\n\n\
                #table(columns: 2, [a], [b])\n\nWord#footnote[note] after // c\n\nLast.\n";
    let text = format!("{PAGE}{body}");
    let (mut viewer, directory) = editing(&text);
    press(&mut viewer, "gsgs");
    let broken = |viewer: &Viewer| {
        typst::syntax::Source::detached(viewer.text().unwrap())
            .root()
            .errors_and_warnings()
            .0
            .is_empty()
            .not()
    };
    assert!(!broken(&viewer));
    let keys = [
        "x", "3x", "X", "2X", "dw", "d3w", "de", "db", "D", "dh", "d3l", "dfe", "dta", "J", "gJ",
        "3~", "rx", "ylp", "dd", "2dd", "C<Esc>", "s<Esc>", "v3ld", "cwz<Esc>", "daw", "diw",
    ];
    let mut wrong = Vec::new();
    for at in (PAGE.len()..text.len()).step_by(3) {
        for keys in keys {
            viewer.editor.as_mut().unwrap().set_cursor(at);
            press(&mut viewer, &format!("<Esc>{keys}<Esc>"));
            if broken(&viewer) {
                let after = viewer.text().unwrap().strip_prefix(PAGE).unwrap_or("");
                let why = typst::syntax::Source::detached(viewer.text().unwrap())
                    .root()
                    .errors_and_warnings()
                    .0
                    .first()
                    .map(|error| error.message.to_string());
                wrong.push(format!(
                    "at {}, {keys}: {after:?} ({why:?})",
                    at - PAGE.len()
                ));
            }
            if viewer.text() != Some(&text) {
                press(&mut viewer, "u");
                assert_eq!(viewer.text(), Some(text.as_str()), "{keys} undone");
                compile(&mut viewer);
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{} broken:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn replacing_on_a_page_goes_over_what_is_shown_never_the_marks() {
    check(&[
        // The star that begins strong text stays, and what is shown is
        // typed over: `xyzc d.` as Vim would have `a bc d.` after `Rxyz`.
        ("a *bc* d.\n", "a *", "Rxyz<Esc>", "xy*zc* d.\n"),
        ("a *bc* d.\n", "a *", "Rxyzwv<Esc>", "xy*zw*vd.\n"),
        ("a $x + y$ d.\n", "a $", "Rxyz<Esc>", "xy$z + y$ d.\n"),
        // Backspace brings back what was typed over, marks and all.
        ("a *bc* d.\n", "a *", "Rxyz<BS><BS><BS><Esc>", "a *bc* d.\n"),
        (
            "a $x + y$ d.\n",
            "a $",
            "Rxyz<BS><BS><Esc>",
            "x $x + y$ d.\n",
        ),
        ("One two.\n", "two", "Rxyz<Esc>", "One xyz.\n"),
    ]);
}

#[test]
fn repeating_from_where_nothing_is_shown_goes_by_what_is() {
    // The first `x` leaves the cursor on a dollar sign; `.` takes what
    // is shown there, as `x` would.
    check(&[("abc $x$ d.\n", "c $", "x..", "ab$$ d.\n")]);
}

#[test]
fn opening_a_paragraph_keeps_to_the_paragraph_not_what_stands_against_it() {
    check(&[
        // Not above the rules that come before it: what is typed there
        // would be typeset without them.
        (
            "#set text(size: 10pt)\nOne two.\n",
            "two",
            "Onew<Esc>",
            "#set text(size: 10pt)\nnew\n\nOne two.\n",
        ),
        (
            "= Head\nText.\n",
            "Text",
            "Onew<Esc>",
            "= Head\nnew\n\nText.\n",
        ),
        (
            "= Head\nText.\n",
            "Head",
            "onew<Esc>",
            "= Head\n\nnew\nText.\n",
        ),
        ("One.\n- a\n", "One", "onew<Esc>", "One.\n\nnew\n- a\n"),
        // Paragraphs apart as they were.
        (
            "One.\n\nTwo.\n",
            "Two",
            "Onew<Esc>",
            "One.\n\nnew\n\nTwo.\n",
        ),
        (
            "One\ntwo.\n\nThree.\n",
            "One",
            "onew<Esc>",
            "One\ntwo.\n\nnew\n\nThree.\n",
        ),
    ]);
}

#[test]
fn deleting_a_paragraph_leaves_the_cursor_on_the_next() {
    for (body, needle, keys, after, on) in [
        (
            "First words.\n\nSecond words.\n",
            "First",
            "dd",
            "\nSecond words.\n",
            "Second",
        ),
        (
            "First.\n\nSecond.\n\nThird.\n",
            "Second",
            "dd",
            "First.\n\n\nThird.\n",
            "Third",
        ),
        ("First.\n\nSecond.\n", "Second", "dd", "First.\n\n", "First"),
        ("= Head\n\nText.\n", "Head", "dd", "\nText.\n", "Text"),
    ] {
        let text = format!("{PAGE}{body}");
        let (mut viewer, directory) = editing(&text);
        press(&mut viewer, "gsgs");
        let at = text.find(needle).unwrap();
        viewer.editor.as_mut().unwrap().set_cursor(at);
        press(&mut viewer, keys);
        compile(&mut viewer);
        let now = viewer.text().unwrap().to_owned();
        assert_eq!(now.strip_prefix(PAGE), Some(after), "{body:?}, {keys}");
        let cursor = viewer.editor.as_ref().unwrap().cursor();
        assert_eq!(Some(cursor), now.find(on), "{body:?}, {keys}: the cursor");
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn ctrl_o_goes_back_where_a_jump_came_from_and_tab_on_again() {
    check(&[
        (
            "One two.\n\nThree four.\n",
            "One",
            "/four<CR><C-o>x",
            "ne two.\n\nThree four.\n",
        ),
        (
            "One two.\n\nThree four.\n",
            "One",
            "/four<CR><C-o><Tab>x",
            "One two.\n\nThree our.\n",
        ),
        (
            "One two.\n\nThree four.\n",
            "One",
            "/four<CR><C-o><C-i>x",
            "One two.\n\nThree our.\n",
        ),
        (
            "One two.\n\nThree four.\n",
            "two",
            "G<C-o>x",
            "One wo.\n\nThree four.\n",
        ),
    ]);
}

#[test]
fn ctrl_o_while_typing_runs_one_command_and_typing_goes_on() {
    check(&[
        ("One two.\n", "two", "iX<C-o>xY<Esc>", "One XYwo.\n"),
        ("One two.\n", "two", "AX<C-o>0Y<Esc>", "YOne two.X\n"),
    ]);
}
