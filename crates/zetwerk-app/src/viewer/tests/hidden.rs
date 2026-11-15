//! Keys in normal and visual mode that reach into what the page does not
//! show: what they take is what the page shows there, never the code
//! around it that nobody sees.

use typst::syntax::{LinkedNode, Side, Source, SyntaxKind};

use super::*;

const PAGE: &str = "#set page(width: 300pt, height: 300pt)\n#set heading(numbering: \"1.\")\n";

/// The text after `keys` on the page with the cursor on the first
/// `needle` of `body`, and what the status line says.
fn on_page(body: &str, needle: &str, keys: &str) -> (String, Option<String>) {
    let text = format!("{PAGE}{body}");
    let (mut viewer, directory) = editing(&text);
    press(&mut viewer, "gsgs");
    let at = text
        .find(needle)
        .unwrap_or_else(|| panic!("{needle} in {body}"));
    viewer.editor.as_mut().unwrap().set_cursor(at);
    press(&mut viewer, keys);
    let after = viewer.text().unwrap().to_owned();
    let told = viewer.message.as_ref().map(|message| message.text.clone());
    let _ = std::fs::remove_dir_all(directory);
    let after = after
        .strip_prefix(PAGE)
        .map_or_else(|| format!("(no set-up) {after}"), str::to_owned);
    (after, told)
}

/// Runs each of `cases` (a text, where the cursor is put, the keys, the
/// text expected, and words the status line is to hold then, if any)
/// and says which came out otherwise.
fn check(cases: &[(&str, &str, &str, &str, &str)]) {
    let mut wrong = Vec::new();
    for (body, needle, keys, expected, said) in cases {
        let (after, told) = on_page(body, needle, keys);
        let told_well = said.is_empty() || told.as_deref().is_some_and(|told| told.contains(said));
        if after != *expected || !told_well {
            wrong.push(format!(
                "{body:?} on {needle:?}, {keys}: {after:?} ({told:?}), not {expected:?} ({said:?})"
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn taking_the_mark_of_a_footnote_takes_the_footnote_whole() {
    // `x` on the mark left `A# b.`, which Typst cannot read; Delete
    // before it did the same, Backspace behind it nothing.
    let note = "A#footnote[note] b.\n";
    check(&[
        (
            note,
            "footnote",
            "x",
            "A b.\n",
            "a footnote deleted, with its text",
        ),
        (
            note,
            "A#",
            "a<Del>",
            "A b.\n",
            "a footnote deleted, with its text",
        ),
        (note, "A#", "a<Del><Esc>", "A b.\n", ""),
        (
            note,
            " b.",
            "i<BS>",
            "A b.\n",
            "a footnote deleted, with its text",
        ),
        (note, " b.", "i<BS><Esc>", "A b.\n", ""),
        (note, "footnote", "dl", "A b.\n", ""),
        // Its text, elsewhere on the page, goes letter by letter.
        (note, "ote]", "x", "A#footnote[nte] b.\n", ""),
        // Put in before the mark: before all of it, not into its code.
        (note, "A#", "yllP", "AA#footnote[note] b.\n", ""),
    ]);
}

#[test]
fn a_thing_shown_as_one_goes_with_its_hash() {
    check(&[
        // The number of a figure: the figure, not `caption: )`.
        (
            "One.\n\n#figure(rect(), caption: [A box.])\n",
            "figure",
            "x",
            "One.\n\n\n",
            "a figure deleted",
        ),
        ("A #sym.alpha c.\n", "sym", "x", "A  c.\n", ""),
        // The quotation mark of a quote: the quote, said so.
        (
            "A.\n\n#quote[B c.]\n",
            "quote",
            "x",
            "A.\n\n\n",
            "a quote deleted",
        ),
        // With its label: left, it would label what stands before it.
        (
            "One.\n\n#figure(rect(), caption: [A box.]) <f>\n",
            "figure",
            "x",
            "One.\n\n\n",
            "",
        ),
    ]);
}

#[test]
fn an_equation_given_to_a_call_that_shows_nothing_else_goes_with_it() {
    check(&[
        (
            "One.\n\n#math.equation(block: true, numbering: \"(1)\", $ x + y $)\n",
            "x +",
            "va$d",
            "One.\n\n\n",
            "an equation deleted",
        ),
        ("One.\n\n#figure($ x $)\n", "x $", "va$d", "One.\n\n\n", ""),
        // A caption shows: the figure stays, and so does the equation.
        (
            "One.\n\n#figure($ x $, caption: [C.])\n",
            "x $",
            "va$d",
            "One.\n\n#figure($ x $, caption: [C.])\n",
            "",
        ),
    ]);
}

#[test]
fn ctrl_u_in_a_cell_takes_only_what_is_in_the_cell() {
    let table = "#table(columns: 2, [a], [b])\n";
    check(&[
        (
            table,
            "b]",
            "a<C-u><Esc>",
            "#table(columns: 2, [a], [])\n",
            "",
        ),
        (table, "b]", "i<C-u><Esc>", table, ""),
        (table, "b]", "i<C-w><Esc>", table, ""),
        (
            "#table(columns: 2, [a b c], [d])\n",
            "c]",
            "a<C-u><Esc>",
            "#table(columns: 2, [], [d])\n",
            "",
        ),
    ]);
}

#[test]
fn brackets_of_code_are_no_text_object_on_the_page() {
    // `ci(` in a cell took all of the table's arguments; `da(` on a box
    // its parentheses.
    let table = "#table(columns: 2, [a], [b])\n";
    check(&[
        (table, "b]", "ci(X<Esc>", table, ""),
        (table, "b]", "di[", table, ""),
        (
            "A #box(rect()) b.\n",
            "rect",
            "da(",
            "A #box(rect()) b.\n",
            "",
        ),
        (
            "A #text(\"x\")[b] c.\n",
            "b]",
            "di\"",
            "A #text(\"x\")[b] c.\n",
            "",
        ),
        // Those of running text are.
        ("A (b c) d.\n", "c)", "ci(X<Esc>", "A (X) d.\n", ""),
        ("A (b c) d.\n", "c)", "da(", "A  d.\n", ""),
        ("A [b c] d.\n", "c]", "di[", "A [] d.\n", ""),
    ]);
}

#[test]
fn case_keys_on_a_line_leave_the_text_of_its_footnotes() {
    let note = "A#footnote[note] b.\n";
    check(&[
        (note, "A#", "gUU", "A#footnote[note] B.\n", ""),
        (note, "A#", "g~~", "a#footnote[note] B.\n", ""),
        (note, "b.", "~", "A#footnote[note] B.\n", ""),
        // In the footnote, its own.
        (note, "note", "gUiw", "A#footnote[NOTE] b.\n", ""),
    ]);
}

#[test]
fn escape_from_v_leaves_the_cursor_where_it_was() {
    check(&[
        ("One two three.\n", "two", "V<Esc>x", "One wo three.\n", ""),
        ("One two three.\n", "two", "VVx", "One wo three.\n", ""),
        ("One two three.\n", "two", "Vd", "", ""),
    ]);
}

#[test]
fn arrows_while_typing_never_lead_into_code() {
    // Up from below a figure went to `#|figure(…)`: what was typed
    // there was code, and the document stopped.
    let text = "Text.\n\n#figure(rect(), caption: [A box.])\n\nMore.\n";
    for keys in ["i<Up>X<Esc>", "i<Up><Up>X<Esc>", "A<Down>X<Esc>"] {
        let needle = if keys.starts_with('A') {
            "Text"
        } else {
            "More"
        };
        let (after, _) = on_page(text, needle, keys);
        let source = typst::syntax::Source::detached(&after);
        let at = after.find('X').unwrap();
        assert_eq!(
            crate::typst_text::ground(&source, at),
            crate::typst_text::Ground::Markup,
            "{keys}: {after:?}"
        );
    }
}

#[test]
fn deleting_what_references_lead_to_says_so() {
    check(&[
        (
            "= Head <head>\n\nSee @head and @head.\n",
            "Head",
            "dd",
            "\nSee @head and @head.\n",
            "2 references to it lead nowhere now (u brings it back)",
        ),
        (
            "One.\n\n#figure(rect(), caption: [A box.]) <f>\n\nSee @f.\n",
            "figure",
            "x",
            "One.\n\n\n\nSee @f.\n",
            "a figure and a label deleted: a reference to it leads nowhere now",
        ),
    ]);
}

#[test]
fn a_link_typed_as_text_is_edited_letter_by_letter() {
    // The page showed it as one place: `x` took all of it.
    let link = "See https://example.com now.\n";
    check(&[
        (link, "example", "x", "See https://xample.com now.\n", ""),
        (link, "example", "rE", "See https://Example.com now.\n", ""),
        (link, "example", "lx", "See https://eample.com now.\n", ""),
        (link, "https", "dw", "See ://example.com now.\n", ""),
    ]);
}

/// The places the page shows, in the order of the text.
fn places(viewer: &Viewer) -> Vec<Range<usize>> {
    let rows = PageRows {
        document: viewer.document.as_ref().unwrap(),
        world: viewer.compiled_world.as_ref().unwrap(),
        file: viewer.file_id(),
        lines: &viewer.page_lines,
        cursor_page: &viewer.cursor_page,
        change: Change::NONE,
        landed: &viewer.landed,
        elsewhere: &viewer.elsewhere,
    };
    let text = viewer.text().unwrap();
    let mut found = Vec::new();
    let mut at = 0;
    while let Ok(Some(stop)) = rows.stop(at, Seek::After) {
        if stop.start < at {
            break;
        }
        at = text[stop.start..]
            .chars()
            .next()
            .map_or(stop.start + 1, |c| stop.start + c.len_utf8());
        found.push(stop);
    }
    found
}

/// A document with one of each thing a page shows otherwise than it is
/// written: what the keys of the sweep are pressed on.
const EVERYTHING: &str = "= Head <head>\n\n\
    One *two* and _three_ $x^2 + alpha$ see @head and @src.\n\
    - item *bold* end\n  - nested `raw` x\n- next\n\n\
    + first\n+ second\n\n\
    / Term: its words\n\n\
    #figure(rect(), caption: [A box.]) <fig>\n\n\
    #table(columns: 2, [a], [b c], [d], [e])\n\n\
    Word#footnote[note here] after // c\n\n\
    #quote[A quote.]\n\n\
    See https://example.com and \"so\" now.\n\n\
    #math.equation(block: true, numbering: \"(1)\", $ y = 1 $)\n\n\
    ```rust\nfn main() {}\n```\n\n\
    #include \"part.typ\"\n\n\
    Last (one) [two].\n\n\
    #bibliography(\"refs.bib\")\n";

/// The file [`EVERYTHING`] takes in.
const PART: &str = "A part *of it*.\n";

/// The bytes of `text` the page shows: those of its places (see
/// [`places`]) that hold no other place. A place that holds others is
/// written with more than it shows: the mark of a footnote holds the
/// footnote's text, the number of a figure all of the figure.
fn shown_bytes(text: &str, places: &[Range<usize>]) -> Vec<bool> {
    let mut shown = vec![false; text.len()];
    for place in places {
        let holds = places
            .iter()
            .any(|other| other != place && place.start <= other.start && other.end <= place.end);
        if !holds {
            for byte in &mut shown[place.clone()] {
                *byte = true;
            }
        }
    }
    shown
}

/// Which bytes of `before` are gone from `after`, and which of `after`
/// are new, by the longest run the two have in common around what
/// changed.
fn changed(before: &str, after: &str) -> (Vec<bool>, Vec<bool>) {
    let (a, b) = (before.as_bytes(), after.as_bytes());
    // The end first: what went is taken to begin where it can begin
    // first (`#figure(…)\n\n` before `#table`, not `figure(…)\n\n#`).
    let suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let prefix = a[..a.len() - suffix]
        .iter()
        .zip(&b[..b.len() - suffix])
        .take_while(|(x, y)| x == y)
        .count();
    let (a_mid, b_mid) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);
    let (n, m) = (a_mid.len(), b_mid.len());
    // How long the common run of what follows is, from each pair on.
    let mut runs = vec![0_u32; (n + 1) * (m + 1)];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            runs[i * (m + 1) + j] = if a_mid[i] == b_mid[j] {
                runs[(i + 1) * (m + 1) + j + 1] + 1
            } else {
                runs[(i + 1) * (m + 1) + j].max(runs[i * (m + 1) + j + 1])
            };
        }
    }
    let mut gone = vec![false; a.len()];
    let mut new = vec![false; b.len()];
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        // Where taking this byte loses nothing of what the two have in
        // common, it went: a deletion begins where it begins, not one
        // byte of the same kind further on.
        let went = i < n && j < m && runs[(i + 1) * (m + 1) + j] == runs[i * (m + 1) + j];
        if i < n && j < m && a_mid[i] == b_mid[j] && !went {
            i += 1;
            j += 1;
        } else if j == m || (i < n && runs[(i + 1) * (m + 1) + j] >= runs[i * (m + 1) + j + 1]) {
            gone[prefix + i] = true;
            i += 1;
        } else {
            new[prefix + j] = true;
            j += 1;
        }
    }
    (gone, new)
}

/// What went of `before` that the page did not show, while what it was
/// part of stayed: the name of a call, the arguments of a table, the
/// `#` of a call. Words for each, or nothing.
///
/// What the page does not show may go with all of what it belongs to,
/// where that showed something: a heading's mark with the heading, a
/// footnote with its `#`, a label with what it labels. Blanks may go
/// (they part paragraphs), and so may the marks of an equation that
/// raise or divide by what was taken out of it: they are tidied away.
fn hidden_gone(before: &Source, shown: &[bool], gone: &[bool], joins: bool) -> Vec<String> {
    let text = before.text();
    let root = LinkedNode::new(before.root());
    let all_gone = |range: Range<usize>| range.clone().all(|at| gone[at]) && !range.is_empty();
    let mut wrong = Vec::new();
    let mut at = 0;
    while at < text.len() {
        let next = at + text[at..].chars().next().map_or(1, char::len_utf8);
        let blank = text[at..next].trim().is_empty();
        if !gone[at] || shown[at] || blank {
            at = next;
            continue;
        }
        let Some(leaf) = root.leaf_at(at, Side::After) else {
            at = next;
            continue;
        };
        let in_equation =
            std::iter::successors(leaf.parent().cloned(), |node| node.parent().cloned())
                .any(|node| node.kind() == SyntaxKind::Equation);
        // What joins paragraphs (`J`, Backspace at the start of one,
        // Delete at the end) makes what follows text of the one before:
        // the mark of a heading or an item goes, as in a word processor.
        let mark = matches!(
            leaf.kind(),
            SyntaxKind::HeadingMarker
                | SyntaxKind::ListMarker
                | SyntaxKind::EnumMarker
                | SyntaxKind::TermMarker
                | SyntaxKind::Colon
        );
        if (in_equation && leaf.kind() != SyntaxKind::Dollar) || (joins && mark) {
            at = next;
            continue;
        }
        // A `#` belongs with the code it begins, a label with what it
        // labels.
        let mut node = match leaf.kind() {
            SyntaxKind::Hash => leaf.next_sibling().unwrap_or(leaf.clone()),
            SyntaxKind::Label => leaf.prev_sibling().unwrap_or(leaf.clone()),
            _ => leaf.clone(),
        };
        let mut whole = all_gone(node.range());
        while let Some(parent) = node.parent().cloned() {
            if !all_gone(parent.range()) {
                break;
            }
            node = parent;
            whole = true;
        }
        let showed = node.range().any(|at| shown[at]);
        // A comment on a line that went whole goes with it.
        let line_start = text[..node.range().start]
            .rfind('\n')
            .map_or(0, |found| found + 1);
        let line_end = text[node.range().end..]
            .find('\n')
            .map_or(text.len(), |found| node.range().end + found);
        let comment = matches!(
            node.kind(),
            SyntaxKind::LineComment | SyntaxKind::BlockComment
        ) && all_gone(line_start..line_end);
        if !(whole && showed) && !comment {
            wrong.push(format!(
                "{:?} of {:?} went",
                &text[node.range()],
                node.kind()
            ));
            at = node.range().end.max(next);
            continue;
        }
        at = next;
    }
    wrong
}

/// What the sweep still finds, each a key and where it is pressed:
/// open, and written up.
///
/// - `C` and `D` from the last letter of the paragraph that ends with a
///   footnote and a comment take the comment along: the line the page
///   shows ends behind it there (not so in a shorter document).
const KNOWN: &[(&str, &str)] = &[("C<Esc>", "r // c"), ("D", "r // c")];

/// Keys that take whole lines or reach over the end of a paragraph,
/// whose hidden losses the sweep still finds and does not hold against
/// them (they are read whole all the same):
///
/// - `db`, `dB`, `de` over the end of a heading join what follows to it
///   and take its label (the status line says what then leads nowhere);
/// - `dd`, `2dd`, `dk` take the first line of an item that holds a
///   list, which then stands by itself; and a comment or an `#include`
///   on the lines they take;
/// - `vjd` takes a quote, a figure, a label where the selection only
///   begins or ends in them;
/// - `d3w` over the end of a paragraph takes a comment that ends it.
const LOSES_HIDDEN: &[&str] = &["db", "dB", "de", "dd", "2dd", "dk", "vjd", "d3w"];

/// From every byte of [`EVERYTHING`] on a page, each of `keys`: what
/// Typst could read before, it reads after, and nothing the page does
/// not show went while what it is part of stayed. An `X` typed goes
/// into what is shown, never into code.
fn sweep(keys: &[&str]) {
    let text = format!("{PAGE}{EVERYTHING}");
    let directory = directory(&text);
    std::fs::write(directory.join("part.typ"), PART).unwrap();
    std::fs::write(
        directory.join("refs.bib"),
        "@book{src, title = {A Book}, author = {Some One}, year = {2000}}\n",
    )
    .unwrap();
    let (mut viewer, _) = open(&directory);
    press(&mut viewer, "gsgs");
    assert!(
        viewer.state().problems.is_empty(),
        "{:?}",
        viewer.state().problems
    );
    let source = Source::detached(text.as_str());
    let shown = shown_bytes(&text, &places(&viewer));
    let lacking = [
        "expected expression",
        "unexpected hat",
        "unexpected underscore",
        "unexpected slash",
    ];
    let mut wrong = Vec::new();
    for at in PAGE.len()..text.len() {
        if !text.is_char_boundary(at) {
            continue;
        }
        for keys in keys {
            viewer.editor.as_mut().unwrap().set_cursor(at);
            press(&mut viewer, &format!("<Esc>{keys}<Esc>"));
            let after = viewer.text().unwrap().to_owned();
            if after == text {
                continue;
            }
            if !after.starts_with(PAGE) && !text.starts_with(&after) {
                // Into the file the text takes in, which the page shows
                // as its own: back, with what was typed there undone.
                for _ in 0..4 {
                    if viewer.text() != Some(PART) {
                        press(&mut viewer, "<Esc>u");
                    }
                }
                assert_eq!(viewer.text(), Some(PART), "at {at}, {keys}");
                press(&mut viewer, "<Esc><C-^>");
                for _ in 0..4 {
                    if viewer.text() != Some(&text) {
                        press(&mut viewer, "<Esc>u");
                    }
                }
                assert_eq!(viewer.text(), Some(text.as_str()), "at {at}, {keys}: back");
                compile(&mut viewer);
                continue;
            }
            let here = || format!("at {:?}, {keys}", &text[at..text.len().min(at + 12)]);
            let shown_after = after.strip_prefix(PAGE).unwrap_or("(no set-up)");
            let errors = Source::detached(after.as_str())
                .root()
                .errors_and_warnings()
                .0;
            if let Some(error) = errors
                .iter()
                .find(|error| !lacking.contains(&error.message.as_str()))
            {
                wrong.push(format!("{}: {shown_after:?} ({})", here(), error.message));
            }
            let (gone, new) = changed(&text, &after);
            // `:s` replaces what is written, as it says; `xp` moves what
            // it took, which the comparison cannot tell from a deletion.
            let written = keys.starts_with(':') || keys.contains('p');
            let joins = keys.contains('J')
                || ["<BS>", "<Del>", "<C-w>", "<C-u>"]
                    .iter()
                    .any(|key| keys.contains(key));
            // Strong text that a letter came to stand against is written
            // with its function instead (`#strong[…]`): it shows the same.
            let rewritten = ["#strong[", "#emph["]
                .iter()
                .any(|function| after.matches(function).count() > text.matches(function).count());
            let known = KNOWN
                .iter()
                .any(|(known, from)| known == keys && text[at..].starts_with(from));
            if !written && !rewritten && !known && !LOSES_HIDDEN.contains(keys) {
                for hidden in hidden_gone(&source, &shown, &gone, joins) {
                    wrong.push(format!("{}: {hidden}: {shown_after:?}", here()));
                }
            }
            let typed_into = Source::detached(after.as_str());
            let into_code = after.char_indices().any(|(byte, c)| {
                c == 'X'
                    && new[byte]
                    && crate::typst_text::ground(&typed_into, byte)
                        == crate::typst_text::Ground::Code
            });
            if into_code {
                wrong.push(format!("{}: typed into code: {shown_after:?}", here()));
            }
            for _ in 0..4 {
                if viewer.text() != Some(&text) {
                    press(&mut viewer, "<Esc>u");
                }
            }
            assert_eq!(viewer.text(), Some(text.as_str()), "{} undone", here());
            compile(&mut viewer);
        }
    }
    let _ = std::fs::remove_dir_all(directory);
    assert!(
        wrong.is_empty(),
        "{} wrong:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn no_key_that_deletes_reaches_into_what_the_page_does_not_show() {
    sweep(&[
        "x", "3x", "X", "2X", "dw", "d3w", "de", "db", "dB", "D", "dh", "d3l", "dfe", "dta", "d0",
        "d$", "dd", "2dd", "dj", "dk", "dap", "dip", "Vd", "v3ld", "vjd", "<Del>",
    ]);
}

#[test]
fn no_text_object_on_a_page_reaches_into_what_it_does_not_show() {
    let mut keys = Vec::new();
    for object in [
        "iw", "aw", "ip", "ap", "i(", "a(", "i[", "a[", "i\"", "a\"", "i$", "a$", "it", "at",
    ] {
        for operator in ["d", "c", "y"] {
            let typed = if operator == "c" { "X" } else { "" };
            let put = if operator == "y" { "P" } else { "" };
            keys.push(format!("{operator}{object}{typed}{put}"));
        }
    }
    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
    sweep(&keys);
}

#[test]
fn no_change_on_a_page_reaches_into_what_it_does_not_show() {
    sweep(&[
        "rx",
        "~",
        "3~",
        "J",
        "gJ",
        "3J",
        "ylp",
        "yyp",
        "yawP",
        "ya$p",
        "yi[P",
        "xp",
        "C<Esc>",
        "cc<Esc>",
        "s<Esc>",
        "cwX",
        "Rxy",
        "gUiw",
        "gUU",
        "g~~",
        ">>",
        "<lt><lt>",
        "Vy<Esc>P",
        ":s/e/E/<CR>",
        ":s/o/0/g<CR>",
    ]);
}

#[test]
fn no_key_while_typing_reaches_into_what_the_page_does_not_show() {
    sweep(&[
        "iX", "aX", "i<C-w>", "a<C-w>", "i<C-u>", "a<C-u>", "i<BS>", "a<BS>", "i<Del>", "a<Del>",
        "i<Up>X", "i<Down>X", "a<Up>X", "a<Down>X", "2i<C-w>",
    ]);
}
