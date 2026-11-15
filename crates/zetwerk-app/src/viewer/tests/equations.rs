//! Equations typed, changed and emptied on the page, as somebody types
//! the mathematics of a paper: every state typing reaches is typeset,
//! or shown as it is written in its place, and nothing the page does
//! not show is left in the text.

use super::*;

/// `text` edited on the page: the cursor put at the first `at`, then
/// `keys`.
fn typed(text: &str, at: &str, keys: &str) -> (Viewer, PathBuf) {
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "gsgs");
    if let Some(editor) = viewer.editor.as_mut() {
        editor.set_cursor(text.find(at).unwrap_or(0));
    }
    press(&mut viewer, keys);
    (viewer, directory)
}

/// Typesets all of it as when typing pauses, as often as the viewer
/// asks for it (an equation Typst refuses asks once more, to be shown
/// as written).
fn settle(viewer: &mut Viewer) {
    let _ = viewer.take_edited();
    for _ in 0..4 {
        compile(viewer);
        if viewer.take_edited().is_none() {
            break;
        }
    }
}

/// What Typst is given for the text as it is now.
fn shown_text(viewer: &mut Viewer) -> String {
    let (_, world) = viewer.snapshot();
    let file = viewer.editor.as_ref().map(Editor::file);
    file.and_then(|file| world.source(file).ok())
        .map(|source| source.text().to_owned())
        .unwrap_or_default()
}

#[test]
#[ignore = "an exploration: run with --ignored --nocapture"]
#[allow(clippy::too_many_lines, reason = "one line per case")]
fn explore() {
    let cases: &[(&str, &str, &str)] = &[
        ("Some $x$ here.\n", "x$", "a + ab <Esc>"),
        ("Some words here and more.\n\nNext.\n", "here", "i$x <Esc>"),
        ("A $x + y$ b.\n", "x", "xxxxxxxxx"),
        (
            "A $x + y$ b.\n",
            "x",
            "i<Del><Del><Del><Del><Del><Del><Esc>",
        ),
        (
            "A $x + y$ b.\n",
            " b.",
            "i<BS><BS><BS><BS><BS><BS><BS><Esc>",
        ),
        ("A $x + y$ b.\n", "x", "dw"),
        ("A $x + y$ b.\n", "x", "dwdwdwdw"),
        ("A $x + y$ b.\n", "x", "vi$d"),
        ("Words.\n\n$ x + y $\n\nMore.\n", "x", "dd"),
        ("Words.\n\n$ x + y $\n\nMore.\n", "x", "xxxxxxx"),
        ("A $x$ b.\n", "$ b", "i\\<Esc>"),
        ("A $x$ b.\n", "$ b", "i\\"),
        ("A b.\n", "b", "i$x^2"),
        ("A b.\n", "b", "i$x^2<Esc>"),
        ("A b.\n", "b", "i$x^2$ <Esc>"),
        ("A b.\n", "b", "i$a/b$ <Esc>"),
        ("A b.\n", "b", "i$ff<Tab>a<Tab>b<Esc>"),
        ("A b.\n", "b", "i$ff<Tab><Esc>"),
        ("A b.\n", "b", "i$sum<Tab>x<Esc>"),
        ("A b.\n", "b", "i$\"word<Esc>"),
        ("A b.\n", "b", "i$x \"wo<Esc>"),
        ("A $x$ b.\n\nNext para $y$.\n", "x$", "a \"wo<Esc>"),
        ("A $x$ b.\n", "x$", "a_<Esc>"),
        ("A $x$ b.\n", "x$", "a^<Esc>"),
        ("A $x$ b.\n", "x$", "a/<Esc>"),
        ("A $x$ b.\n", "x$", "a(<Esc>"),
        ("A $x$ b.\n", "x$", "a[<Esc>"),
        ("A $x$ b.\n", "x$", "a{<Esc>"),
        ("A $x$ b.\n", "x$", "a\\<Esc>"),
        ("A $x$ b.\n", "x$", "a#<Esc>"),
        ("A $x$ b.\n", "x$", "a@<Esc>"),
        ("A $x$ b.\n", "x$", "a`<Esc>"),
        ("A $x$ b.\n", "x$", "a<lt><Esc>"),
        ("A $x$ b.\n", "x$", "a&<Esc>"),
        ("A $x$ b.\n", "x$", "a'<Esc>"),
        ("A $x$ b.\n", "x$", "a;<Esc>"),
        ("A $x$ b.\n", "x$", "a.<Esc>"),
        ("A $x$ b.\n", "x$", "a:<Esc>"),
        ("A $x$ b.\n", "x$", "a mat(1, 2; 3<Esc>"),
        ("A $x$ b.\n", "x$", "a lr(<Esc>"),
        ("A $x$ b.\n", "x$", "a sqrt(<Esc>"),
        ("A $x$ b.\n", "x$", "a alpha beta<Esc>"),
        ("A $x$ b.\n", "x$", "a sin x<Esc>"),
        ("A $x$ b.\n", "x$", "a xyz<Esc>"),
        ("A $x$ b.\n", "x$", "a abc <Esc>"),
        ("A $x$ b.\n", "x$", "a abc<Esc>"),
        ("A $x$ b.\n", "x$", "a $<Esc>"),
        ("A $x^2$ b.\n", "2", "x"),
        ("A $x^(2 n)$ b.\n", "2", "xxxx"),
        ("A $frac(a, b)$ b.\n", "a,", "x"),
        ("A $frac(a, b)$ b.\n", "b)", "x"),
        ("A $frac(a, b)$ b.\n", "a,", "xxxxxxx"),
        ("A $sqrt(a)$ b.\n", "a)", "x"),
        ("A $sqrt(a)$ b.\n", "a)", "xx"),
        ("A $vec(a, b)$ b.\n", "a,", "xx"),
        ("A $mat(a, b; c, d)$ b.\n", "b;", "xx"),
        ("A $cases(a, b)$ b.\n", "b)", "xx"),
        ("A $x$ b.\n", "x", "x"),
        ("A $x$ b.\n", "x", "xx"),
        ("A $x$ b.\n", "x", "xxx"),
        ("A $x$ b.\n", "x", "xu"),
        ("A $x$ b.\n", "x", "xxu"),
        ("A $x$ b.\n", "x", "s<Esc>"),
        ("Words.\n\n$ x $ <eq:a>\n\nSee @eq:a.\n", "x", "x"),
        ("Words.\n\n$ x $ <eq:a>\n\nSee @eq:a.\n", "x", "xx"),
        (
            "#set math.equation(numbering: \"(1)\")\nWords.\n\n$ x $ <eq:a>\n\nSee @eq:a.\n",
            "x",
            "a^<Esc>",
        ),
        (
            "#set math.equation(numbering: \"(1)\")\nWords.\n\n$ x $ <eq:a>\n\nSee @eq:a.\n",
            "x",
            "a \"t<Esc>",
        ),
        ("- item $x$ here\n- two\n", "x", "x"),
        ("- item $x$ here\n- two\n", "x", "a^<Esc>"),
        ("#table(columns: 2, [$x$], [b])\n", "x", "a^<Esc>"),
        ("#table(columns: 2, [$x$], [b])\n", "x", "x"),
        ("A#footnote[note $x$ here] b.\n", "x", "a^<Esc>"),
        ("A#footnote[note $x$ here] b.\n", "x", "x"),
        ("#figure(rect(), caption: [cap $x$ here])\n", "x", "a^<Esc>"),
        (
            "#figure(rect(), caption: [cap $x$ here])\n",
            "x",
            "a \"t<Esc>",
        ),
        ("A $x$ b.\n", "x", "a<CR>y<Esc>"),
        ("Words.\n\n$ x $\n\nMore.\n", "x", "a<CR>y<Esc>"),
        ("Words.\n\n$ x $\n\nMore.\n", "x", "o<Esc>"),
        ("A $x$ b.\n", "x", "ve<C-b>"),
        ("A $a+b$ b.\n", "+", "r-"),
        ("A $x$ b.\n", "x", "~"),
        ("A $x y$ b.\n", "x", "J"),
        ("A $x$ b.\n", "A", "ix$<Esc>"),
        ("A $x^$ b. C $y$ d.\n", "y$", "a ab <Esc>"),
        ("A $x$ b.\n", "x", "a<C-w><Esc>"),
        ("A $x + y$ b.\n", "y", "D"),
        ("A $x + y$ b.\n", " b", "a<C-w><C-w><Esc>"),
        ("A $x$ b.\n", "A", "i$<BS><Esc>"),
        ("A b.\n", "b", "i$ <BS><BS><Esc>"),
        ("A b.\n", "b", "i$ <BS><Esc>"),
        ("A b.\n", "b", "i#eq<CR>x<Esc>"),
        ("A b.\n", "b", "i#neq<CR>x<Esc>"),
        ("A b.\n", "b", "i#ieq<CR>x<Tab>y<Esc>"),
        ("A $x$ b.\n", "x$", "a ff<Tab>1<Tab>2<Tab>z<Esc>"),
        ("A $x$ b.\n", "x$", "a sum<Tab>k<Esc>"),
        ("A $x$ b.\n", "x$", "a int<Tab><Esc>"),
        ("A $x$ b.\n", "x$", "a mat<Tab>1<Tab>2<Tab>3<Tab>4<Esc>"),
        ("A $x$ b.\n", "x$", "a vec<Tab><Esc>"),
        ("A $x$ b.\n", "x$", "a case<Tab>1<Tab>2<Esc>"),
        ("A $x$ b.\n", "x$", "a lim<Tab>a<Esc>"),
        ("A $x$ b.\n", "x$", "a pd<Tab>f<Esc>"),
        ("A b.\n", "b", "i$x^2 + y$<Esc>uu<C-r><C-r>"),
        ("A b.\n", "b", "i$x^2 + y$<Esc>u"),
        ("A $x + y$ b.\n", "x", "cwz<Esc>"),
        ("A $x + y$ b.\n", "x", "dwu"),
        ("A $frac(a, b)$ c.\n", "a,", "dd"),
        ("A $x$ b.\n", "x", "a\\ <Esc>"),
        ("Words.\n\n$ x $\n\nMore.\n", "x", "xi<BS><Esc>"),
        ("A $x$ b.\n", "b", "i<BS><BS><BS><Esc>"),
        ("A $x$ b.\n", "x", "a<Right><Right>z<Esc>"),
        ("A $x_1$ b.\n", "1", "x"),
        ("Words.\n\n$ x\n  + y $\n\nMore.\n", "y", "dd"),
        ("A $\"text\" x$ b.\n", "text", "dw"),
        ("A $\"text\" x$ b.\n", "text", "xxxxx"),
        ("A $sqrt(x)$ b.\n", "x)", "a<BS><BS><Esc>"),
        ("A $x + y$ b.\n", "x", "v$<C-b>"),
        ("A $x + y$ b.\n", "A", "v/y<CR><C-b>"),
        ("A $x$ b.\n", "x", "a<CR><CR>y<Esc>"),
        ("A $x$ b.\n", "x", "yyp"),
        ("A $x$ b.\n", "x", "yi$P"),
        ("A $x$ b.\n", "x", "ya$$p"),
        ("A $alpha$ b.\n", "alpha", "x"),
        ("A $alpha$ b.\n", "alpha", "a<BS><Esc>"),
        ("A $alpha beta$ b.\n", "beta", "i<BS><Esc>"),
        ("A $x$ b.\n", "x", "rx"),
        ("A $x$ b.\n", "x", "R12<Esc>"),
        ("A $x$ b.\n", "x$", "a \"wo\"z<Esc>"),
        ("A $x$ b.\n", "x$", "a \"wo<Right>z<Esc>"),
        ("A $x + y$ b.\n", "+", "i$<Esc>"),
        ("A $x + y$ b.\n", "+", "i\"<Esc>"),
        ("A $x + y$ b.\n", "+", "a\"<Esc>"),
        ("A $x$ b.\n", "x$", "a$z<Esc>"),
    ];
    for (text, at, keys) in cases {
        let (mut viewer, directory) = typed(text, at, keys);
        let after = viewer.text().unwrap_or_default().to_owned();
        let shown = shown_text(&mut viewer);
        settle(&mut viewer);
        let problems = viewer.state().problems;
        let current = viewer.document_revision == Some(viewer.revision);
        let written = viewer.written.is_some();
        let plain = zetwerk_typst::compile(&viewer.project.snapshot());
        let plain = plain.errors.first().map(|error| error.message.to_string());
        let mode = viewer.state().editing.map(|editing| editing.mode);
        println!(
            "{text:?} at {at:?} {keys:?}\n    text  {after:?}\n    shown {shown:?}\n    \
             mode {mode:?} current {current} as-written {written} problems {problems:?}\n    \
             file alone: {plain:?}"
        );
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
#[ignore = "an exploration: run with --ignored --nocapture"]
#[allow(clippy::too_many_lines, reason = "one line per case")]
fn explore_motions() {
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "A $x^2 + y_1$ b.\n",
            "A",
            &["l", "l", "l", "l", "l", "l", "l", "l", "l"],
        ),
        (
            "A $x^2 + y_1$ b.\n",
            "b.",
            &["h", "h", "h", "h", "h", "h", "h", "h", "h"],
        ),
        (
            "A $frac(a, b) + sqrt(c)$ d.\n",
            "A",
            &["w", "w", "w", "w", "w", "w", "w"],
        ),
        (
            "A $frac(a, b) + sqrt(c)$ d.\n",
            "d.",
            &["b", "b", "b", "b", "b", "b", "b"],
        ),
        ("A $alpha + beta$ d.\n", "A", &["e", "e", "e", "e", "e"]),
        (
            "Words.\n\n$ sum_(i = 1)^n i = (n (n + 1)) / 2 $\n\nMore.\n",
            "Words",
            &["j", "l", "l", "l", "l", "l", "l", "l", "l", "l", "l", "j"],
        ),
        (
            "A $x^2$ b.\n",
            "A",
            &[
                "a", "<Right>", "<Right>", "<Right>", "<Right>", "<Right>", "z", "<Esc>",
            ],
        ),
        ("A $x$ b.\n", "x", &["a", "^", "<Tab>", "y", "<Esc>"]),
        (
            "A $x$ b.\n",
            "x",
            &["a", "ff<Tab>", "<Tab>", "<Tab>", "<Tab>", "z", "<Esc>"],
        ),
        (
            "A $x$ b.\n",
            "x",
            &[
                "a", "ff<Tab>", "1", "<Tab>", "2", "<BS>", "<BS>", "<BS>", "<BS>", "<BS>", "<Esc>",
            ],
        ),
        (
            "Words.\n\n$ x $\n\nMore.\n",
            "x",
            &["a", "<CR>", "y", "<BS>", "<BS>", "<BS>", "<Esc>"],
        ),
        (
            "A $x$ b.\n",
            "b",
            &["i", "<BS>", "<BS>", "<BS>", "<BS>", "<Esc>"],
        ),
        ("A $x + y$ b.\n", "x", &["v", "l", "l", "d"]),
        ("A $x + y$ b.\n", "A", &["v", "l", "l", "l", "d"]),
        ("A $x + y$ b.\n", "y", &["v", "l", "l", "d"]),
        ("A $x + y$ b. More $z$.\n", "y", &["J"]),
        (
            "A $x$ b.\n",
            "x",
            &["a", "<C-b>", "y", "<C-b>", "z", "<Esc>"],
        ),
        ("A $x$ b.\n", "x", &["a", "<C-i>", "<Esc>"]),
        ("A $x$ b.\n", "x", &["a", " in RR", "<Esc>"]),
        ("A $x$ b.\n", "x", &["a", " a.b", "<Esc>"]),
        ("A $x$ b.\n", "x", &["a", " sin(x", "<Esc>"]),
        ("A $x$ b.\n", "x", &["a", " f(x) = x", "<Esc>"]),
        ("A $x$ b.\n", "x", &["a", "<lt>= y", "<Esc>"]),
        ("A $x$ b.\n", "x", &["a", " &= y \\ z", "<Esc>"]),
        ("A $x$ b.\n", "x", &["a", " arrow.r", "<Esc>"]),
        ("A $x$ b.\n", "x", &["a", " arrow.", "<Esc>"]),
        ("A $x$ b.\n", "x", &["a", " dif", "<Esc>"]),
        ("A $x$ b.\n", "x", &["a", "'", "<Esc>"]),
        ("A $x$ b.\n", "x", &["a", "!", "<Esc>"]),
        ("A $x$ b.\n", "x", &["a", " |x|", "<Esc>"]),
        ("A $x$ b.\n", "x", &["a", " lr(", "<Esc>"]),
        ("A $x$ b.\n", "x", &["a", " 1.5", "<Esc>"]),
        ("A $x$ b.\n", "x", &["a", " #", "<Esc>"]),
        ("A $x$ b.\n", "x", &["a", " x_(i j)", "<Esc>"]),
        ("A $x$ b.\n", "x", &["a", " \"a\" b", "<Esc>"]),
        (
            "A $x$ b.\n",
            "x",
            &["a", " ab", "<Right>", "<Right>", "<Esc>"],
        ),
        (
            "A $x$ b.\n",
            "x",
            &["a", " ab", "<Left>", "<Left>", "<Left>", "<Esc>"],
        ),
    ];
    for (text, at, keys) in cases {
        let (mut viewer, directory) = typed(text, at, "");
        settle(&mut viewer);
        println!("{text:?} at {at:?}");
        for key in *keys {
            press(&mut viewer, key);
            let shown = shown_text(&mut viewer);
            settle(&mut viewer);
            let now = viewer.text().unwrap_or_default().to_owned();
            let cursor = viewer.editor.as_ref().map_or(0, Editor::cursor);
            let mode = viewer.state().editing.map(|editing| editing.mode);
            println!(
                "    {key:10} {:?} {mode:?} written {} current {} shown {shown:?}",
                format!("{}‸{}", &now[..cursor], &now[cursor..]),
                viewer.written.is_some(),
                viewer.document_revision == Some(viewer.revision),
            );
        }
        let _ = std::fs::remove_dir_all(directory);
    }
}

/// The text of the viewer once all of it is typeset, and that it is
/// typeset as it is, nothing shown as it is written.
fn settled(viewer: &mut Viewer) -> String {
    settle(viewer);
    let problems = viewer.state().problems;
    assert!(problems.is_empty(), "{:?}: {problems:?}", viewer.text());
    assert!(
        viewer.written.is_none(),
        "{:?} is shown as it is written",
        viewer.text()
    );
    assert_eq!(viewer.document_revision, Some(viewer.revision));
    viewer.text().unwrap_or_default().to_owned()
}

#[test]
fn a_word_deleted_at_the_end_of_an_equation_leaves_its_dollar_sign() {
    // `dw` on the last word of an equation went on over the dollar sign
    // that closes it, which shows as nothing: the equation ran on into
    // the rest of the paragraph, shown in red as it is written.
    for (at, keys, left) in [
        ("y", "dw", "A $x + $ b.\n"),
        ("y", "D", "A $x + $\n"),
        ("x", "dwdwdw", "A b.\n"),
    ] {
        let (mut viewer, directory) = typed("A $x + y$ b.\n", at, keys);
        assert_eq!(viewer.text(), Some(left), "{keys}");
        settled(&mut viewer);
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn deleting_back_over_the_end_of_an_equation_leaves_its_dollar_sign() {
    // Ctrl-W behind an equation took its last letter and the dollar sign
    // behind it.
    let (mut viewer, directory) = typed("A $x + y$ b.\n", " b", "a<C-w><C-w><Esc>");
    let text = viewer.text().unwrap_or_default().to_owned();
    assert_eq!(text.matches('$').count() % 2, 0, "{text:?}");
    settled(&mut viewer);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn an_equation_emptied_goes_with_its_dollar_signs() {
    // What is left of an equation all of whose symbols were deleted is
    // its dollar signs (and the blanks between), which show as nothing.
    for (text, at, keys, left) in [
        ("A $x + y$ b.\n", "x", "vi$d", "A b.\n"),
        ("A $x$ b.\n", "x", "x", "A b.\n"),
        ("A $x$ b.\n", "x", "s<Esc>", "A b.\n"),
        ("A $x$ b.\n", "x", "a<C-w><Esc>", "A b.\n"),
        ("A $sqrt(a)$ b.\n", "a)", "xx", "A b.\n"),
        ("A $sqrt(x)$ b.\n", "x)", "a<BS><BS><Esc>", "A b.\n"),
        (
            "Words.\n\n$ x + y $\n\nMore.\n",
            "x",
            "dd",
            "Words.\n\nMore.\n",
        ),
        ("Words.\n\n$ x $\n\nMore.\n", "x", "x", "Words.\n\nMore.\n"),
        ("A b.\n", "b", "i$<Esc>", "A b.\n"),
        ("A b.\n", "b", "i$ <Esc>", "A b.\n"),
    ] {
        let (mut viewer, directory) = typed(text, at, keys);
        assert_eq!(viewer.text(), Some(left), "{text:?} {keys}");
        settled(&mut viewer);
        let _ = std::fs::remove_dir_all(directory);
    }
    // It goes in the same step as what was deleted last.
    let (mut viewer, directory) = typed("A $x$ b.\n", "x", "x");
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some("A $x$ b.\n"));
    press(&mut viewer, "<C-r>");
    assert_eq!(viewer.text(), Some("A b.\n"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn an_equation_emptied_while_typing_stays_in_its_line() {
    // Delete took the symbols of an equation in a line one after the
    // other and left the blanks between them: `$  $`, an equation that
    // stands by itself, and the paragraph broke in two around it.
    let (mut viewer, directory) = typed("A $x + y$ b.\n", "x", "i<Del><Del><Del>");
    assert_eq!(viewer.text(), Some("A $$ b.\n"));
    press(&mut viewer, "z");
    assert_eq!(viewer.text(), Some("A $z$ b.\n"));
    press(&mut viewer, "<BS><Esc>");
    assert_eq!(viewer.text(), Some("A b.\n"));
    let _ = std::fs::remove_dir_all(directory);
    // One that stands by itself stays so.
    let text = "Words.\n\n$ x $\n\nMore.\n";
    let (viewer, directory) = typed(text, "x", "i<Del>");
    assert_eq!(viewer.text(), Some("Words.\n\n$  $\n\nMore.\n"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn an_empty_pair_of_dollar_signs_goes_with_backspace() {
    let (mut viewer, directory) = typed("A b.\n", "b", "i$");
    assert_eq!(viewer.text(), Some("A $$b.\n"));
    press(&mut viewer, "<BS>");
    assert_eq!(viewer.text(), Some("A b.\n"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_dollar_sign_typed_in_running_text_begins_an_equation_that_ends() {
    // Before a word, or behind one, a dollar sign came alone: all that
    // followed it in its paragraph was read as mathematics.
    for (text, at, keys, left) in [
        ("A b.\n", "b", "i$x^2", "A $x^2$b.\n"),
        ("A b.\n", "b", "i$x^2$ ", "A $x^2$ b.\n"),
        ("A $x$ b.\n", "A", "ix$y", "x$y$A $x$ b.\n"),
    ] {
        let (mut viewer, directory) = typed(text, at, keys);
        assert_eq!(viewer.text(), Some(left), "{text:?} {keys}");
        press(&mut viewer, "<Esc>");
        settled(&mut viewer);
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn bold_on_a_selection_out_of_an_equation_stays_in_it() {
    // Ctrl-B put `bold(` before the selection and `)` behind it, behind
    // the dollar sign that closes the equation.
    let (mut viewer, directory) = typed("A $x + y$ b.\n", "x", "v$<C-b>");
    let text = viewer.text().unwrap_or_default().to_owned();
    assert_eq!(text, "A $bold(x + y)$ b.\n");
    settled(&mut viewer);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_search_finds_an_equation_written_with_blanks() {
    // An equation is read without its blanks: `/a + b` found nothing in
    // `$a + b$`.
    let text = "Some words, then $a + b$ and more.\n";
    for pattern in ["a + b", "a+b"] {
        let (viewer, directory) = typed(text, "Some", &format!("/{pattern}<CR>"));
        let cursor = viewer.editor.as_ref().map(Editor::cursor);
        assert_eq!(cursor, text.find("a + b"), "/{pattern}");
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn an_empty_pair_of_dollar_signs_before_text_goes_with_backspace() {
    // Backspace took the closing sign first, then the opening one, which
    // was then the opening sign of an equation up to the next one.
    let (mut viewer, directory) = typed("A $x$ b.\n", "A", "i$");
    assert_eq!(viewer.text(), Some("$$A $x$ b.\n"));
    press(&mut viewer, "<BS>");
    assert_eq!(viewer.text(), Some("A $x$ b.\n"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn text_in_an_equation_keeps_its_quotation_marks() {
    // Deleting the letters of text in an equation one by one went on to
    // its closing quotation mark, and the text ran on to the paragraph's
    // end; or left the two marks, which show as nothing.
    let (mut viewer, directory) = typed("A $\"text\" x$ b.\n", "text", "xxxxx");
    assert_eq!(viewer.text(), Some("A $x$ b.\n"));
    settled(&mut viewer);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_mark_left_in_one_equation_does_not_stop_letters_becoming_a_product_in_another() {
    // A mark that raises nothing anywhere in the text made every
    // equation be taken for one whose dollar signs are wrong: `ab` was
    // left a name Typst refuses.
    let (mut viewer, directory) = typed("A $x^$ b. C $y$ d.\n", "y$", "a ab <Esc>");
    assert_eq!(viewer.text(), Some("A $x^$ b. C $y a b $ d.\n"));
    settled(&mut viewer);
    let _ = std::fs::remove_dir_all(directory);
}

/// A small generator of numbers, the same for the same seed.
struct Dice(u64);

impl Dice {
    fn below(&mut self, n: usize) -> usize {
        // SplitMix64.
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) % n.max(1) as u64) as usize
    }

    fn pick<'a>(&mut self, of: &[&'a str]) -> &'a str {
        of[self.below(of.len())]
    }
}

/// What is typed into an equation, and what is done to one.
const TYPED: &[&str] = &[
    "x", "y", "2", "n", "a", "b", " ", " ", "^", "_", "/", "+", "=", "(", ")", ",", "alpha", "pi",
    "ff<Tab>", "sq<Tab>", "vec<Tab>", "sum<Tab>", "<Tab>", "<BS>", "<BS>", "<BS>", "<Del>",
    "<Left>", "<Right>", "\"", "$", "&", ";", "<Esc>", "<C-w>",
];
const DONE: &[&str] = &[
    "x", "x", "X", "dw", "db", "D", "s", "cw", "h", "l", "w", "b", "e", "a", "a", "i", "i", "u",
    "<C-r>", "vi$d", "vlld", "vhd", ".", "~", "J", "ci(", "da(",
];

/// Typing in equations, at random: after every pause all of it is
/// typeset as it is, or the equation is shown as it is written; never
/// pages that stay behind the text. What went wrong, by kind, with the
/// first walk that showed it.
fn equation_walks(seeds: std::ops::Range<u64>, steps: usize) -> Vec<String> {
    let text = "#set page(width: 300pt, height: 400pt)\n\
                Some text $x + y$ and more.\n\n\
                $ a^2 + b^2 = c^2 $\n\n\
                - an item $frac(1, 2)$\n\n\
                End $z$.\n";
    let mut wrong = std::collections::BTreeMap::new();
    for seed in seeds {
        let mut dice = Dice(seed);
        let at = ["x +", "a^2", "1, 2", "z$"][dice.below(4)];
        let (mut viewer, directory) = typed(text, at, "i");
        let mut trail = format!("i at {at:?}: ");
        for step in 0..steps {
            let typing = viewer
                .editor
                .as_ref()
                .is_some_and(|editor| editor.mode() == editor::Mode::Insert);
            let keys = if typing {
                dice.pick(TYPED)
            } else {
                dice.pick(DONE)
            };
            trail.push_str(keys);
            press(&mut viewer, keys);
            let now = viewer.text().unwrap_or_default().to_owned();
            if now.contains(EMPTY) || now.contains(CURSOR_MARK) {
                wrong
                    .entry("a mark of the editor in the text".to_owned())
                    .or_insert_with(|| format!("seed {seed}: {trail:?}: {now:?}"));
            }
            if step % 4 == 3 || keys == "<Esc>" {
                settle(&mut viewer);
                let behind = viewer.document_revision != Some(viewer.revision);
                let problems = viewer.state().problems;
                if behind || !problems.is_empty() {
                    let first = problems.first().cloned().unwrap_or_default();
                    let kind = first.split(": ").skip(2).collect::<Vec<_>>().join(": ");
                    let detail = format!(
                        "written {:?} tried {:?} revision {} cursor {:?}",
                        viewer.written,
                        viewer.written_tried,
                        viewer.revision,
                        viewer.editor.as_ref().map(Editor::cursor)
                    );
                    wrong
                        .entry(format!("not typeset: {kind}"))
                        .or_insert_with(|| format!("seed {seed}: {trail:?}: {now:?} {detail}"));
                }
            }
        }
        press(&mut viewer, "<Esc><Esc>");
        settle(&mut viewer);
        let now = viewer.text().unwrap_or_default().to_owned();
        if viewer.written.is_some() {
            wrong
                .entry("typing over, an equation is shown as written".to_owned())
                .or_insert_with(|| format!("seed {seed}: {trail:?}: {now:?}"));
        }
        let _ = std::fs::remove_dir_all(directory);
    }
    wrong
        .into_iter()
        .map(|(kind, first)| format!("{kind}\n    first: {first}"))
        .collect()
}

#[test]
#[ignore = "a long walk: run with --ignored --nocapture"]
fn explore_random_equations() {
    let seeds = std::env::var("ZETWERK_EQUATION_SEEDS")
        .ok()
        .and_then(|seeds| seeds.parse().ok())
        .unwrap_or(200);
    let from = std::env::var("ZETWERK_EQUATION_FROM")
        .ok()
        .and_then(|from| from.parse().ok())
        .unwrap_or(0);
    for line in equation_walks(from..seeds, 40) {
        println!("{line}");
    }
}

#[test]
fn letters_typed_together_become_a_product_when_the_cursor_leaves_them() {
    // Tab out of the equation right behind `yz`: the word was being
    // typed when the cursor left, and nothing looked at it again; the
    // equation stayed red, a name Typst does not know.
    for (keys, left) in [
        ("a yz<Tab>w", "A $x y z$w b.\n"),
        ("a yz<Right>w", "A $x y z$ wb.\n"),
    ] {
        let (mut viewer, directory) = typed("A $x$ b.\n", "x$", keys);
        assert_eq!(viewer.text(), Some(left), "{keys}");
        settle(&mut viewer);
        assert!(viewer.written.is_none(), "{keys}");
        assert_eq!(viewer.document_revision, Some(viewer.revision));
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn a_dollar_sign_typed_in_an_equation_ends_it() {
    // In the middle of an equation a dollar sign closed it there, and
    // the closing one opened another, up to the next equation: all the
    // text between was taken for mathematics.
    let (mut viewer, directory) = typed("A $x + y$ b. C $z$.\n", "+", "i$w");
    assert_eq!(viewer.text(), Some("A $x + y$w b. C $z$.\n"));
    press(&mut viewer, "<Esc>");
    settled(&mut viewer);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn letters_typed_before_a_letter_stay_in_their_order() {
    // Typed right before a letter of an equation, each letter made a
    // name with it that was spelled out at once, and the cursor was put
    // before the name: `yz` typed before `x` came out as `z y x`.
    let (mut viewer, directory) = typed("A $x$ b.\n", "x", "iyz");
    assert_eq!(viewer.text(), Some("A $yzx$ b.\n"));
    // While it is typed, the word is shown as text.
    assert_eq!(shown_text(&mut viewer), "A $\"yzx\"$ b.\n");
    press(&mut viewer, "<Esc>");
    assert_eq!(viewer.text(), Some("A $y z x$ b.\n"));
    settled(&mut viewer);
    let _ = std::fs::remove_dir_all(directory);
    // A name typed so is the name, once a blank parts it.
    let (mut viewer, directory) = typed("A $a$ b.\n", "a$", "ialpha <Esc>");
    assert_eq!(viewer.text(), Some("A $alpha a$ b.\n"));
    settled(&mut viewer);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn text_in_an_equation_is_closed_by_its_quotation_mark() {
    // A quotation mark typed at the end of text in an equation was put
    // into it as one of its characters; one typed before a symbol came
    // alone, and the text took in the rest of the paragraph.
    for (at, keys, left) in [
        ("x ", "a \"wo\"z", "A $x \"wo\"z + y$ b.\n"),
        ("+", "i\"t", "A $x \"t\"+ y$ b.\n"),
    ] {
        let (mut viewer, directory) = typed("A $x + y$ b.\n", at, keys);
        assert_eq!(viewer.text(), Some(left), "{keys}");
        press(&mut viewer, "<Esc>");
        settled(&mut viewer);
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
#[ignore = "an exploration: run with --ignored --nocapture"]
fn explore_states() {
    let head = "#set page(width: 300pt, height: 400pt)\n";
    let states = [
        "- an item $frac(n,_x_2x^&1, 2)$+ =asu$$\n\nEnd $z$.\n",
        "End $,2z vec(sqrt(frac)2 vec(=,sum_(i = 1)^n _=2^))$.\n",
        "Some text $x + y$ and more.\n\n- an item $frac(;+a+;p i x a v e c 1, n 2)frac(x, )$22sum<&2x($2$)>\n",
        "Some text $x + y$pisq<x()> and more.\n",
        "A $x$ b $frac(a, b, c)$ c $y$.\n",
    ];
    for state in states {
        let text = format!("{head}{state}");
        for at in ["Some", "End", "- an", "A $"] {
            if !text.contains(at) {
                continue;
            }
            let (mut viewer, directory) = typed(&text, at, "");
            settle(&mut viewer);
            let shown = shown_text(&mut viewer);
            println!(
                "{state:?} cursor at {at:?}: written {:?} current {} problems {:?}\n    shown {shown:?}",
                viewer.written,
                viewer.document_revision == Some(viewer.revision),
                viewer.state().problems,
            );
            let _ = std::fs::remove_dir_all(directory);
        }
    }
    for (text, at, keys) in [
        (
            "End $/\\/frac(a^=^;y(a s q) sum_(i = 1)^n &, )z$.\n",
            "z$",
            "i",
        ),
        (
            "End $/\\/frac(a^=^;y(a s q) sum_(i = 1)^n &, )z$.\n",
            "z$",
            "",
        ),
        ("- an item $frac(a=)$\n\nEnd $z$.\n", "=)", "a"),
        ("- an item $frac(a=)$\n\nEnd $z$.\n", "=)", ""),
        ("- an item $frac(a)$\n\nEnd $z$.\n", "a)", "a"),
    ] {
        let (mut viewer, directory) = typed(text, at, keys);
        let shown = shown_text(&mut viewer);
        compile(&mut viewer);
        let once = (viewer.written.clone(), viewer.state().problems);
        settle(&mut viewer);
        let shown_after = shown_text(&mut viewer);
        println!(
            "{text:?} {at:?} {keys:?}: first {once:?}\n    then written {:?} current {} problems {:?}\n    shown {shown:?}\n    then {shown_after:?}",
            viewer.written,
            viewer.document_revision == Some(viewer.revision),
            viewer.state().problems,
        );
        let _ = std::fs::remove_dir_all(directory);
    }
    let (mut viewer, directory) = typed("Some text and more.\n", "and", "ipisq<Tab>^<BS>(<Left>x");
    println!("Tab in text: {:?}", viewer.text());
    settle(&mut viewer);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_letter_and_a_digit_typed_together_become_a_product() {
    // `a2` is one name to Typst, which it does not know.
    let (mut viewer, directory) = typed("A $x$ b.\n", "x$", "a + a2 + pi2<Esc>");
    assert_eq!(viewer.text(), Some("A $x + a 2 + p i 2$ b.\n"));
    settled(&mut viewer);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn two_slashes_typed_in_an_equation_are_no_comment() {
    // `//` began a comment that took in the dollar sign closing the
    // equation; the rest of the document became mathematics.
    for keys in ["a//y<Esc>", "a/*y<Esc>"] {
        let (mut viewer, directory) = typed("A $x$ b.\n\nNext $z$.\n", "x$", keys);
        let text = viewer.text().unwrap_or_default().to_owned();
        assert!(text.starts_with("A $x/\\"), "{keys}: {text:?}");
        settled(&mut viewer);
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn an_equation_typst_refuses_is_shown_as_written_where_the_cursor_is_not() {
    // With the cursor in another equation, that one was shown as it is
    // written, not the one Typst refused: the page stayed behind.
    let text = "A $x$ b $frac(a, b, c)$ c.\n";
    let (mut viewer, directory) = typed(text, "x$", "ay");
    settle(&mut viewer);
    assert_eq!(viewer.document_revision, Some(viewer.revision));
    assert_eq!(viewer.state().problems, Vec::<String>::new());
    let shown = shown_text(&mut viewer);
    assert!(shown.contains("[`$frac(a, b, c)$`]"), "{shown:?}");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn an_equation_given_to_a_function_stays_when_emptied() {
    // An emptied equation went, dollar signs and all, also out of the
    // call it was given to: a numbered equation inserted and left at
    // once lacked its body, and the document was not typeset again.
    for (text, at, keys, left) in [
        (
            "A b.\n",
            "b",
            "i#neq<CR><Esc>",
            "A \n\n#math.equation(block: true, numbering: \"(1)\", $  $)\n\nb.\n",
        ),
        (
            "#figure($ x $, caption: [c])\n",
            "x",
            "x",
            "#figure($  $, caption: [c])\n",
        ),
        ("#align(center, $x$)\n", "x", "x", "#align(center, $$)\n"),
    ] {
        let (mut viewer, directory) = typed(text, at, keys);
        assert_eq!(viewer.text(), Some(left), "{text:?} {keys}");
        settled(&mut viewer);
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn a_mark_whose_base_is_deleted_shows_an_empty_place() {
    // Deleting `x` of `x^2` left `^2`, which Typst refuses: the equation
    // was shown in red as it is written. It shows the place of what was
    // deleted now, to be typed into.
    for (text, keys, left) in [
        ("A $x^2$ b.\n", "x", "A $^2$ b.\n"),
        ("A $x_1$ b.\n", "x", "A $_1$ b.\n"),
        ("A $x/y$ b.\n", "x", "A $/y$ b.\n"),
    ] {
        let (mut viewer, directory) = typed(text, "x", keys);
        assert_eq!(viewer.text(), Some(left), "{text:?}");
        settled(&mut viewer);
        assert!(shown_text(&mut viewer).contains(&format!("${EMPTY}")));
        press(&mut viewer, "iz<Esc>");
        assert_eq!(viewer.text(), Some(text.replace('x', "z").as_str()));
        settled(&mut viewer);
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn a_deletion_over_two_equations_leaves_neither() {
    // `dfy` from the first equation into the second kept the dollar
    // sign of each that it took only a part of; one of the two equations
    // left empty stayed, unseen, as `$$`.
    for (text, keys, left) in [
        ("A $x$ b. $y$ c.\n", "dfy", "A c.\n"),
        ("A $x$ b. $y$ c. $z$ d.\n", "dfz", "A d.\n"),
    ] {
        let (mut viewer, directory) = typed(text, "x", keys);
        assert_eq!(viewer.text(), Some(left), "{keys}");
        settled(&mut viewer);
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn an_equation_shown_as_written_stays_so_when_the_cursor_leaves_it() {
    // `$` typed in an equation Typst refuses (here a fraction without
    // what it divides by) took the cursor out of it, the text as it was:
    // the pages were taken for those of that text, and as the equation
    // was tried again and refused, it was not shown as written again.
    // Typesetting stopped there, with a complaint and nothing changing.
    let (mut viewer, directory) = typed("A $x$ b.\n", "x$", "a frac(a");
    settle(&mut viewer);
    assert!(viewer.written.is_some());
    for keys in ["$", " c"] {
        press(&mut viewer, keys);
        settle(&mut viewer);
        assert_eq!(viewer.state().problems, Vec::<String>::new(), "{keys}");
        assert!(viewer.written.is_some(), "{keys}");
        assert_eq!(viewer.document_revision, Some(viewer.revision), "{keys}");
    }
    assert_eq!(viewer.text(), Some("A $x frac(a)$ c b.\n"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn two_equations_typst_refuses_are_both_shown_as_written() {
    // With one equation left refused, typing another into a state Typst
    // refuses showed only that one as written: the first was refused
    // again, and the document was not typeset until the second was an
    // equation again, the page standing still while it was typed.
    let text = "A $frac(a)$ b.\n\nC $x$ d.\n";
    let (mut viewer, directory) = typed(text, "x$", "");
    settle(&mut viewer);
    assert!(viewer.written.is_some());
    press(&mut viewer, "a frac(b");
    settle(&mut viewer);
    assert_eq!(viewer.state().problems, Vec::<String>::new());
    assert_eq!(viewer.document_revision, Some(viewer.revision));
    let shown = shown_text(&mut viewer);
    assert_eq!(shown.matches("[`$").count(), 2, "{shown:?}");
    // Each is shown as what it means once it is an equation again.
    press(&mut viewer, ", c)<Esc>");
    settle(&mut viewer);
    assert_eq!(viewer.state().problems, Vec::<String>::new());
    let shown = shown_text(&mut viewer);
    assert_eq!(shown.matches("[`$").count(), 1, "{shown:?}");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_word_typed_right_behind_an_equation_is_offered_nothing_of_equations() {
    // The dollar sign before the word is the equation's: its things were
    // offered, and Tab put the template of a sum into running text, its
    // underscore beginning emphasis that ran on to the end.
    let (mut viewer, directory) = typed("A $x$ and more.\n\nNext.\n", " and", "isum");
    assert!(viewer.completion.is_none());
    press(&mut viewer, "<Tab><Esc>");
    assert!(!viewer.text().unwrap_or_default().contains('_'));
    settled(&mut viewer);
    let _ = std::fs::remove_dir_all(directory);
    // In an equation, at its end too, they are.
    let (viewer, directory) = typed("A $x$ and more.\n", "$ and", "i su");
    assert!(viewer.completion.is_some());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn what_is_typed_behind_a_labelled_equation_leaves_it_its_label() {
    // `$` or Tab out of an equation with a label put the cursor between
    // the equation and the label; what was typed there took the label,
    // and what referred to the equation referred to text, which Typst
    // refuses: the document was not typeset.
    let text = "#set math.equation(numbering: \"(1)\")\nWords.\n\n$ x $ <eq:a>\n\nSee @eq:a.\n";
    for keys in ["a<Tab>b<Esc>", "a$b<Esc>"] {
        let (mut viewer, directory) = typed(text, "x", keys);
        let left = text.replace("<eq:a>\n", "<eq:a>b\n");
        assert_eq!(viewer.text(), Some(left.as_str()), "{keys}");
        settled(&mut viewer);
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn deleting_back_from_an_equation_keeps_the_brackets_around_it() {
    // An equation at the start of a cell of a table, a footnote or a
    // caption: deleting back from it took the bracket the cell begins
    // with, which shows as nothing, and the document was not typeset.
    for (text, keys) in [
        ("#table(columns: 2, [$p_1$], [$q$])\n", "i<C-w><Esc>"),
        ("#table(columns: 2, [$p_1$], [$q$])\n", "i<BS><BS><Esc>"),
        ("#table(columns: 2, [$p_1$], [$q$])\n", "db"),
        ("A#footnote[$s$ in it] b.\n", "i<C-w><Esc>"),
        ("#figure(rect(), caption: [$r^2$ end])\n", "d0"),
    ] {
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, "gsgs");
        if let Some(editor) = viewer.editor.as_mut() {
            editor.set_cursor(text.find('$').unwrap_or(0) + 1);
        }
        press(&mut viewer, keys);
        assert_eq!(viewer.text(), Some(text), "{keys}");
        settled(&mut viewer);
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn what_is_put_into_an_equation_keeps_the_symbols_beside_it() {
    // `p` on a symbol written with several letters put what it put after
    // the first letter (`a x lpha`), and `P` before one made one word of
    // the two (`alphaalpha`): either was then spelled out letter by
    // letter, and the symbol was gone without a word.
    let text = "A $x + y$ b. C $alpha + z$.\n";
    for (keys, left) in [
        ("yl/\\=alpha<CR>p", "A $x + y$ b. C $alpha x + z$.\n"),
        ("yl/\\=alpha<CR>P", "A $x + y$ b. C $x alpha + z$.\n"),
    ] {
        let (mut viewer, directory) = typed(text, "x", keys);
        assert_eq!(viewer.text(), Some(left), "{keys}");
        settled(&mut viewer);
        let _ = std::fs::remove_dir_all(directory);
    }
    // Into an equation emptied, not behind it (between it and its label).
    let text = "#set math.equation(numbering: \"(1)\")\nA b.\n\n$ x + y $ <eq:a>\n\nSee @eq:a.\n";
    let (mut viewer, directory) = typed(text, "x", "ddp");
    assert_eq!(viewer.text(), Some(text));
    settled(&mut viewer);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn an_equation_selected_whole_is_all_that_goes() {
    // `va$` ended the selection on the dollar sign that closes the
    // equation, which shows as nothing; it was moved on to what is shown
    // next, the first letter of the next paragraph, and `d` took that,
    // the label and the bracket of the call between.
    for (text, left) in [
        ("A b.\n\n$ x + y $\n\nC d.\n", "A b.\n\n\n\nC d.\n"),
        (
            "A b.\n\n#math.equation(block: true, numbering: \"(1)\", $ x + y $)\n\nC d.\n",
            "A b.\n\n#math.equation(block: true, numbering: \"(1)\", )\n\nC d.\n",
        ),
    ] {
        let (viewer, directory) = typed(text, "x", "va$d");
        assert_eq!(viewer.text(), Some(left), "{text:?}");
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn bold_begun_right_behind_a_letter_stays_bold() {
    // Ctrl-B behind `x` in an equation made `xbold()`, one name Typst does
    // not know, which was spelled out letter by letter: `x b o l d()`.
    let (mut viewer, directory) = typed("A $x$ b.\n", "x", "a<C-b>y<Esc>");
    assert_eq!(viewer.text(), Some("A $x bold(y)$ b.\n"));
    settled(&mut viewer);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn text_begun_in_an_equation_and_left_empty_goes() {
    // A quotation mark typed in an equation comes with the one that
    // closes it; left so, the two stayed, showing as nothing.
    for (keys, left) in [("a\"<Esc>", "A $x$ b.\n"), ("a \"\"<Esc>", "A $x $ b.\n")] {
        let (mut viewer, directory) = typed("A $x$ b.\n", "x", keys);
        assert_eq!(viewer.text(), Some(left), "{keys}");
        settled(&mut viewer);
        let _ = std::fs::remove_dir_all(directory);
    }
    // Text with something in it stays, and so does an equation of it.
    let (viewer, directory) = typed("A $x$ b.\n", "x", "a \"if\"<Esc>");
    assert_eq!(viewer.text(), Some("A $x \"if\"$ b.\n"));
    let _ = std::fs::remove_dir_all(directory);
}
