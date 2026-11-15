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
    // What took the page's set-up along says so.
    after
        .strip_prefix(PAGE)
        .map_or_else(|| format!("(no set-up) {after}"), str::to_owned)
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
        // Typing there went before the mark of the heading, which made
        // it a paragraph.
        (
            "= Head\n\nText.\n",
            "= Head",
            "iX<Esc>",
            "= XHead\n\nText.\n",
        ),
        (
            "= Head\n\nText.\n",
            "= Head",
            "aX<Esc>",
            "= HXead\n\nText.\n",
        ),
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
    check(&[
        // Not against the star of strong text: Typst could not read it.
        ("One *two* x.\n", "x.", "yl0elp", "One *two* x.\n"),
        (
            "= H <h>\n\nSee @h now.\n",
            "now",
            "yiwF@p",
            "= H <h>\n\nSee @hnow now.\n",
        ),
    ]);
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
    // Ctrl-R + while typing puts in what the clipboard holds too.
    viewer.clipboard.read = || Ok("copied".to_owned());
    press(&mut viewer, "A <C-r>+<Esc>");
    assert_eq!(viewer.text(), Some("pasted one wo copied\n"));
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
        "x",
        "3x",
        "X",
        "2X",
        "dw",
        "d3w",
        "de",
        "db",
        "D",
        "dh",
        "d3l",
        "dfe",
        "dta",
        "J",
        "gJ",
        "3~",
        "rx",
        "ylp",
        "dd",
        "2dd",
        "C<Esc>",
        "s<Esc>",
        "v3ld",
        "cwz<Esc>",
        "daw",
        "diw",
        "Rxy<Esc>",
        "dap",
        "dip",
        "Vd",
        "yyP",
        "dj",
        "dk",
        "db",
        "dB",
        "de",
        "bx",
        "ex",
        "{x",
        "-x",
        "gUiw",
        "g~~",
        ">>",
        "<lt><lt>",
        "xp",
        "Vy<Esc>P",
        "cgnz<Esc>",
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
                // As many steps as the keys made (`xp` makes two).
                for _ in 0..keys.matches(['x', 'p', 'P']).count().clamp(1, 2) {
                    if viewer.text() != Some(&text) {
                        press(&mut viewer, "u");
                    }
                }
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
        // Where what is typed would stand against the star, so that the
        // page could not be read, it is not typed (and the user is told).
        ("a *bc* d.\n", "a *", "Rxyz<Esc>", "x *bc* d.\n"),
        ("a bc d.\n", "a bc", "Rxyzwv<Esc>", "xyzwvd.\n"),
        // Where nothing more is shown in the line, what is typed goes in
        // before the marks, as at the end of a line.
        (
            "T.\n\n#table(columns: 2, [a], [b])\n",
            "b]",
            "Rxy<Esc>",
            "T.\n\n#table(columns: 2, [a], [xy])\n",
        ),
        ("a $x + y$ d.\n", "a $", "Rxyz<Esc>", "xy$z + y$ d.\n"),
        // Backspace brings back what was typed over, marks and all.
        ("a *bc* d.\n", "a *", "Rx<BS><Esc>", "a *bc* d.\n"),
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
            "First.\n\nThird.\n",
            "Third",
        ),
        ("First.\n\nSecond.\n", "Second", "dd", "First.\n", "First"),
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

#[test]
fn lines_deleted_or_yanked_on_a_page_are_put_back_as_paragraphs() {
    check(&[
        // `ddp` swaps two paragraphs, as it swaps two lines in Vim; it
        // made `three` into `ttwo` and `hree`.
        (
            "one\n\ntwo\n\nthree\n",
            "two",
            "ddp",
            "one\n\nthree\n\ntwo\n",
        ),
        (
            "one\n\ntwo\n\nthree\n",
            "two",
            "ddP",
            "one\n\ntwo\n\nthree\n",
        ),
        (
            "one\n\ntwo\n\nthree\n",
            "one",
            "yyGp",
            "one\n\ntwo\n\nthree\n\none\n",
        ),
        (
            "one\n\ntwo\n\nthree\n",
            "two",
            "yapGp",
            "one\n\ntwo\n\nthree\n\ntwo\n",
        ),
        // In a list, lines are items.
        ("- a\n- b\n", "a", "ddp", "- b\n- a\n"),
        ("- a\n- b\n", "a", "yyp", "- a\n- a\n- b\n"),
    ]);
}

#[test]
fn a_paragraph_as_a_text_object_is_what_the_page_shows() {
    // The rules that set up the page stand right above the paragraph:
    // `dap` took them along, unseen.
    check(&[
        ("one\n\ntwo\n", "one", "dap", "two\n"),
        ("one\n\ntwo\n", "one", "dip", "\ntwo\n"),
        (
            "= Head\nText here.\n\nMore.\n",
            "here",
            "dap",
            "= Head\nMore.\n",
        ),
        ("one\n\ntwo\n\nthree\n", "two", "dap", "one\n\nthree\n"),
    ]);
}

#[test]
fn cgn_changes_the_next_match_and_dot_the_one_after() {
    // `gn` is a note where nothing is under way; behind an operator it
    // is Vim's. `cgnX` failed, and then `X` deleted the blank before.
    check(&[
        (
            "one two one two\n",
            "one",
            "/two<CR>cgnX<Esc>",
            "one X one two\n",
        ),
        (
            "one two one two\n",
            "one",
            "/two<CR>cgnX<Esc>.",
            "one X one X\n",
        ),
        ("one two one two\n", "one", "/two<CR>dgn", "one  one two\n"),
        (
            "one two one two\n",
            "one",
            "/two<CR>$cgNX<Esc>",
            "one two one X\n",
        ),
        // Nothing searched for: nothing changes.
        ("one two\n", "one", "cgn", "one two\n"),
    ]);
}

#[test]
fn back_a_paragraph_is_to_its_start() {
    // `{` came to rest on the end of the paragraph before, as the empty
    // line it goes to shows nothing; `{x` took the last letter there.
    check(&[
        (
            "one\n\ntwo three\n\nfour\n",
            "three",
            "{x",
            "one\n\nwo three\n\nfour\n",
        ),
        (
            "one\n\ntwo\n\nthree\n",
            "three",
            "{x",
            "one\n\nwo\n\nthree\n",
        ),
        ("one\n\ntwo\n\nthree\n", "two", "{x", "ne\n\ntwo\n\nthree\n"),
        (
            "one\n\ntwo\n\nthree\n",
            "hree",
            "2{x",
            "one\n\nwo\n\nthree\n",
        ),
        ("one\n\ntwo\n\nthree\n", "one", "}x", "one\n\nwo\n\nthree\n"),
        // `-` and `+` by the lines the page shows, to their start.
        (
            "one\n\ntwo\n\nthree\n",
            "hree",
            "-x",
            "one\n\nwo\n\nthree\n",
        ),
        ("one\n\ntwo\n\nthree\n", "ne", "+x", "one\n\nwo\n\nthree\n"),
    ]);
}

#[test]
fn whole_lines_on_a_page_are_lines_the_page_shows() {
    check(&[
        // `V` and an operator: lines, put back as paragraphs. `VGy` and
        // `p` put the lines into the middle of a word.
        (
            "One two.\n\nThree.\n",
            "One",
            "VGy<Esc>Gp",
            "One two.\n\nThree.\n\nOne two.\n\nThree.\n",
        ),
        (
            "One two.\n\nThree.\n",
            "One",
            "VdGp",
            "\nThree.\n\nOne two.\n",
        ),
        // `dgg` took the rules that set up the page along (they stay
        // before what is left).
        ("One two.\n\nThree.\n\nFour.\n", "Three", "dgg", "\nFour.\n"),
        // `dj` above a figure took the first line of it and left the
        // rest as text; it takes the figure, one line of the page.
        (
            "Some words.\n#figure(\n  rect(),\n  caption: [A box.],\n)\n\nMore.\n",
            "words",
            "dj",
            "\nMore.\n",
        ),
    ]);
}

#[test]
fn a_cursor_in_the_middle_of_a_thing_shown_as_one_rests_on_it() {
    // A search led into a link; `diw` took the blank behind it.
    check(&[(
        "One https://example.com three.\n",
        "example",
        "diw",
        "One ://example.com three.\n",
    )]);
}

#[test]
fn tab_does_what_is_written_in_plain_key_where_it_is() {
    // The order is written down at `Viewer::plain_key`; here each place
    // Tab means something different, one by one.
    check(&[
        // A list is up: the chosen entry.
        (
            "= Head <head>\n\nOne two.\n",
            "two",
            "A @he<Tab><Esc>",
            "= Head <head>\n\nOne two. @head\n",
        ),
        // The short name of a snippet.
        (
            "One two.\n",
            "two",
            "A fn<Tab>X<Esc>",
            "One two. #footnote[X]\n",
        ),
        // In an equation, the next place to fill in.
        (
            "One $x$ two.\n",
            "x$",
            "a + ff<Tab>1<Tab>2<Esc>",
            "One $x + frac(1, 2)$ two.\n",
        ),
        // In a table, the next cell; behind the last one, a new row.
        (
            "Text.\n\n#table(columns: 2, [a], [b])\n",
            "a]",
            "a<Tab>X<Esc>",
            "Text.\n\n#table(columns: 2, [a], [Xb])\n",
        ),
        (
            "Text.\n\n#table(columns: 2, [a], [b])\n",
            "b]",
            "a<Tab>X<Esc>",
            "Text.\n\n#table(columns: 2, [a], [b],\n[X], [])\n",
        ),
        // Elsewhere on a page, nothing: a tab shows as nothing.
        ("One two.\n", "two", "A<Tab>X<Esc>", "One two.X\n"),
        // In normal mode with no place to fill in: Ctrl-I, on through
        // the places jumped from, and nowhere where there is none.
        ("One two.\n", "two", "<Tab>x", "One wo.\n"),
    ]);
}

#[test]
fn words_are_the_words_the_page_reads() {
    // The stars of strong text were words of their own: `b` came to
    // rest on the blank before them, `w` behind them.
    let body = "Third line *with bold* here.\n";
    check(&[
        (body, "with", "bx", "Third ine *with bold* here.\n"),
        (body, "here", "bx", "Third line *with old* here.\n"),
        (body, "line", "eex", "Third line *wit bold* here.\n"),
        (body, "bold", "wx", "Third line *with bold* ere.\n"),
        (body, "Third", "3wx", "Third line *with old* here.\n"),
        (body, "here", "2Bx", "Third line *ith bold* here.\n"),
        // Behind an operator: what is shown goes, the stars stay.
        (body, "here", "db", "Third line *with *here.\n"),
        (body, "line", "de", "Third  *with bold* here.\n"),
    ]);
}

#[test]
fn what_is_taken_out_of_an_exponent_takes_its_mark_along() {
    // The mark that raised it shows as nothing: left behind, nobody
    // could take it away on the page, and it is no equation with it.
    check(&[
        ("One $x^2 + alpha$ see.\n", "2", "3x", "One $x$ see.\n"),
        ("One $x^2 + alpha$ see.\n", "^", "3x", "One $x$ see.\n"),
        ("One $x^2 + alpha$ see.\n", "2", "d3w", "One $x$ see.\n"),
        ("One $x^2$ see.\n", "2", "x", "One $x$ see.\n"),
    ]);
}
