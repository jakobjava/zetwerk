//! Vim's keys on a page: they go by what the page shows.
//!
//! What `f`, `r`, `~`, `J` and counted `h` and `l` do where only pages
//! are shown, how the cursor comes to rest on a part of an equation that
//! is yet to be typed, and that it shows in a block of code with nothing
//! in it. The keys themselves are tested on a page made by hand in the
//! engine's own tests; here the page is Typst's.

use super::*;

/// What the viewer's pages say about where the cursor can be.
fn rows(viewer: &Viewer) -> PageRows<'_> {
    PageRows {
        document: viewer.document.as_ref().unwrap(),
        world: viewer.compiled_world.as_ref().unwrap(),
        file: viewer.file_id(),
        lines: &viewer.page_lines,
        cursor_page: &viewer.cursor_page,
        change: Change::NONE,
        landed: &viewer.landed,
        elsewhere: &viewer.elsewhere,
    }
}

fn cursor(viewer: &Viewer) -> usize {
    viewer.editor.as_ref().unwrap().cursor()
}

/// Whether the cursor is on something the page shows.
fn rests(viewer: &Viewer) -> bool {
    matches!(rows(viewer).stop(cursor(viewer), Seek::At), Ok(Some(_)))
}

/// Where `needle` begins in the text.
fn at(viewer: &Viewer, needle: &str) -> usize {
    viewer.text().unwrap().find(needle).unwrap()
}

/// Whether the pages are those of the text as it is, without complaint.
fn typeset(viewer: &mut Viewer) -> bool {
    compile(viewer);
    viewer.document_revision == Some(viewer.revision)
}

const PAGE: &str = "#set page(width: 300pt, height: 300pt)\n";

#[test]
fn finding_goes_by_the_line_of_the_page() {
    let text = format!(
        "{PAGE}One *two* $alpha + beta$ and \\* more words that go on \
         until the line is full and then some more of them.\n\nNext.\n"
    );
    let (mut viewer, directory) = editing(&text);
    press(&mut viewer, "gsgs/One<CR>");
    // A symbol is found by the letter its name begins with, and not by
    // the letters it is written with.
    press(&mut viewer, "fb");
    assert_eq!(cursor(&viewer), at(&viewer, "beta"));
    press(&mut viewer, "0fa");
    assert_eq!(cursor(&viewer), at(&viewer, "alpha"));
    press(&mut viewer, ";");
    assert_eq!(cursor(&viewer), at(&viewer, "and"), "not in a name");
    press(&mut viewer, ",");
    assert_eq!(cursor(&viewer), at(&viewer, "alpha"));
    // The marks of strong text show as nothing; the star that is read
    // is the one that is found.
    press(&mut viewer, "0f*");
    assert_eq!(cursor(&viewer), at(&viewer, "\\*"));
    assert_eq!(tints(&mut viewer, PAGE_CURSOR).len(), 1);
    press(&mut viewer, "Tt");
    assert_eq!(cursor(&viewer), at(&viewer, "wo*"), "behind it, on the way");
    // Nothing is found in the lines below, though they are of the same
    // line of the text.
    press(&mut viewer, "$");
    let last = cursor(&viewer);
    let line = rows(&viewer).line(last).unwrap();
    assert_eq!(line.last, last);
    assert!(text[line.end..].lines().next().unwrap().contains('e'));
    press(&mut viewer, "fe");
    assert_eq!(cursor(&viewer), last);
    // An operator takes what is found as the one thing it shows as.
    press(&mut viewer, "0fadfb");
    assert!(viewer.text().unwrap().contains("*two* $$ and"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn counts_count_what_the_page_shows() {
    let text = format!("{PAGE}One *two* $alpha + beta$ and more.\n\nNext one.\n");
    let (mut viewer, directory) = editing(&text);
    press(&mut viewer, "gsgs/One<CR>");
    let mut seen = Vec::new();
    for keys in ["3l", "4l", "2l", "l", "3h", "2h", "20h"] {
        press(&mut viewer, keys);
        seen.push(cursor(&viewer));
        assert!(rests(&viewer), "after {keys}");
        assert_eq!(tints(&mut viewer, PAGE_CURSOR).len(), 1, "after {keys}");
    }
    let places = [" *two", " $", "+", "beta", " $", "wo*", "One"];
    assert_eq!(seen, places.map(|needle| at(&viewer, needle)));
    // `l` ends with the paragraph; the space key goes on.
    press(&mut viewer, "99l");
    assert_eq!(cursor(&viewer), at(&viewer, ".\n\nNext"));
    press(&mut viewer, "<Space>");
    assert_eq!(cursor(&viewer), at(&viewer, "Next"));
    press(&mut viewer, "<BS>");
    assert_eq!(cursor(&viewer), at(&viewer, ".\n\nNext"));
    // As many are deleted, a symbol being one.
    press(&mut viewer, "0fa3x");
    assert!(viewer.text().unwrap().contains("*two* $$ and more."));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_character_is_put_over_what_the_page_shows() {
    let text = format!(
        "{PAGE}#set heading(numbering: \"1.\")\n= Start <start>\n\
         See @start and $alpha + 2x$ now.\n"
    );
    let (mut viewer, directory) = editing(&text);
    let line = |viewer: &Viewer| viewer.text().unwrap().lines().nth(3).unwrap().to_owned();
    press(&mut viewer, "gsgs/See<CR>rT");
    assert_eq!(line(&viewer), "Tee @start and $alpha + 2x$ now.");
    assert!(typeset(&mut viewer));
    // What a reference shows is no text to put a character over.
    press(&mut viewer, "f@rx");
    assert_eq!(cursor(&viewer), at(&viewer, "@start"));
    assert_eq!(line(&viewer), "Tee @start and $alpha + 2x$ now.");
    // A symbol goes whole, and what is put there is no part of a name.
    press(&mut viewer, "fa;rb");
    assert_eq!(line(&viewer), "Tee @start and $b + 2x$ now.");
    assert!(typeset(&mut viewer));
    press(&mut viewer, "f2ry");
    assert_eq!(line(&viewer), "Tee @start and $b + y x$ now.");
    assert!(typeset(&mut viewer));
    assert_eq!(cursor(&viewer), at(&viewer, "y x"));
    // In text it is read as it was typed.
    press(&mut viewer, "fn3r*");
    assert_eq!(line(&viewer), "Tee @start and $b + y x$ \\*\\*\\*.");
    assert!(typeset(&mut viewer));
    assert!(rests(&viewer));
    // In a selection, every character that is text.
    press(&mut viewer, "0vf@r-");
    // (A hyphen that begins a line would begin a list.)
    assert_eq!(line(&viewer), "\\----@start and $b + y x$ \\*\\*\\*.");
    assert!(typeset(&mut viewer));
    // Enter on a blank parts the paragraph there.
    press(&mut viewer, "0fdlr<CR>");
    assert!(viewer.text().unwrap().contains("@start and\n\n$b + y x$"));
    assert!(typeset(&mut viewer));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn case_is_switched_of_what_is_read() {
    let text = format!(
        "{PAGE}#set heading(numbering: \"1.\")\n= Start <start>\n\
         See @start and $alpha + x$ now.\n"
    );
    let (mut viewer, directory) = editing(&text);
    let line = |viewer: &Viewer| viewer.text().unwrap().lines().nth(3).unwrap().to_owned();
    press(&mut viewer, "gsgs/See<CR>3~");
    assert_eq!(line(&viewer), "sEE @start and $alpha + x$ now.");
    assert_eq!(cursor(&viewer), at(&viewer, " @start"));
    assert!(typeset(&mut viewer));
    // A name stays the name it is: on it, in a line, in a selection.
    press(&mut viewer, "fa;~");
    assert_eq!(line(&viewer), "sEE @start and $alpha + x$ now.");
    assert_eq!(cursor(&viewer), at(&viewer, "+"));
    press(&mut viewer, "g~~");
    assert_eq!(line(&viewer), "See @start AND $alpha + X$ NOW.");
    assert!(typeset(&mut viewer));
    press(&mut viewer, "0v$u");
    assert_eq!(line(&viewer), "see @start and $alpha + x$ now.");
    assert!(typeset(&mut viewer));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn paragraphs_are_joined_on_the_page() {
    let text = format!("{PAGE}One paragraph\nof two lines.\n\nSecond one.\n\n= Head\n\nLast.\n");
    let (mut viewer, directory) = editing(&text);
    press(&mut viewer, "gsgs/One<CR>J");
    assert_eq!(
        viewer.text().unwrap(),
        format!("{PAGE}One paragraph\nof two lines. Second one.\n\n= Head\n\nLast.\n")
    );
    assert_eq!(cursor(&viewer), at(&viewer, " Second"), "where they meet");
    assert!(typeset(&mut viewer));
    assert_eq!(tints(&mut viewer, PAGE_CURSOR).len(), 1);
    // A heading that is joined on is one no more; with a count, as
    // many as that are joined.
    press(&mut viewer, "3J");
    assert_eq!(
        viewer.text().unwrap(),
        format!("{PAGE}One paragraph\nof two lines. Second one. Head Last.\n")
    );
    assert!(typeset(&mut viewer));
    // Nothing follows: nothing happens.
    press(&mut viewer, "J");
    assert!(viewer.is_compiled());
    press(&mut viewer, "uu");
    assert_eq!(viewer.text().unwrap(), text);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_cursor_rests_on_a_part_yet_to_be_typed() {
    let text = format!("{PAGE}One $x + frac(a, )$ two.\n\nMore.\n");
    let (mut viewer, directory) = editing(&text);
    let (above, below) = (text.find("a, )").unwrap(), text.find(")$").unwrap());
    let word = text.find("frac").unwrap()..above;
    press(&mut viewer, "gsgs/x<CR>");
    // Through the fraction and back: on its parts, never in the word
    // that makes it one.
    let mut seen = Vec::new();
    let mut boxes = Vec::new();
    for keys in [
        "l", "l", "l", "l", "h", "h", "h", "$", "0", "w", "w", "w", "e", "b",
    ] {
        press(&mut viewer, keys);
        seen.push(cursor(&viewer));
        assert!(rests(&viewer), "after {keys}");
        assert!(!word.contains(&cursor(&viewer)), "after {keys}");
        let shown = tints(&mut viewer, PAGE_CURSOR);
        assert_eq!(shown.len(), 1, "after {keys}");
        boxes.push(shown[0]);
    }
    let plus = text.find('+').unwrap();
    assert_eq!(
        seen[..7],
        [plus, above, below, below + 2, below, above, plus]
    );
    // The empty part is below the bar, the other above it.
    assert!(boxes[2].y > boxes[1].y + 3.0, "{boxes:?}");
    assert!((boxes[2].x - boxes[1].x).abs() < 6.0, "{boxes:?}");
    // `x` there takes nothing: the fraction is not one without it.
    press(&mut viewer, "/a,<CR>lx");
    assert_eq!(cursor(&viewer), below);
    assert_eq!(viewer.text().unwrap(), text);
    // A selection that ends on it takes nothing of what follows.
    press(&mut viewer, "hvl");
    let selection = viewer.editor.as_ref().unwrap().selection();
    assert_eq!(selection, Some(above..below));
    press(&mut viewer, "<Esc>");
    // Typing begins in it.
    assert_eq!(cursor(&viewer), below);
    press(&mut viewer, "ib<Esc>");
    assert!(viewer.text().unwrap().contains("$x + frac(a, b)$"));
    assert!(typeset(&mut viewer));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn an_empty_part_goes_with_what_made_it() {
    let text = format!("{PAGE}One $x^$ and $y + frac(, )$ two.\n");
    let (mut viewer, directory) = editing(&text);
    let equations = |viewer: &Viewer| {
        let text = viewer.text().unwrap();
        text[text.find('$').unwrap()..=text.rfind('$').unwrap()].to_owned()
    };
    // What is raised: with `a` it is typed, with `x` the mark goes.
    press(&mut viewer, "gsgs/x<CR>l");
    assert_eq!(cursor(&viewer), at(&viewer, "$ and"));
    assert_eq!(tints(&mut viewer, PAGE_CURSOR).len(), 1);
    press(&mut viewer, "an<Esc>");
    assert_eq!(equations(&viewer), "$x^n$ and $y + frac(, )$");
    let _ = std::fs::remove_dir_all(directory);
    let (mut viewer, directory) = editing(&text);
    press(&mut viewer, "gsgs/x<CR>lx");
    assert_eq!(equations(&viewer), "$x$ and $y + frac(, )$");
    assert_eq!(cursor(&viewer), at(&viewer, "x$"), "on what it stood by");
    assert!(typeset(&mut viewer));
    // A fraction with no parts goes whole; with `r` a part is typed.
    press(&mut viewer, "/y<CR>ll");
    assert_eq!(cursor(&viewer), at(&viewer, ", )"));
    press(&mut viewer, "l");
    assert_eq!(cursor(&viewer), at(&viewer, ")$"));
    press(&mut viewer, "rb");
    assert_eq!(equations(&viewer), "$x$ and $y + frac(, b)$");
    assert!(typeset(&mut viewer));
    let _ = std::fs::remove_dir_all(directory);
    let (mut viewer, directory) = editing(&text);
    press(&mut viewer, "gsgs/y<CR>llx");
    assert_eq!(equations(&viewer), "$x^$ and $y + $");
    assert!(typeset(&mut viewer));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_cursor_shows_in_a_block_of_code_with_nothing_in_it() {
    let (mut viewer, directory) = editing(&format!("{PAGE}Some words.\n"));
    press(&mut viewer, "gsgs/words<CR>A<CR>#code<CR>");
    assert!(viewer.text().unwrap().contains("words.\n\n```\n```\n"));
    assert!(typeset(&mut viewer));
    let begun = tints(&mut viewer, PAGE_BAR);
    assert_eq!(begun.len(), 1, "behind the marks that open it");
    // With its language named, and on its first line.
    press(&mut viewer, "rust");
    assert!(typeset(&mut viewer));
    assert_eq!(tints(&mut viewer, PAGE_BAR).len(), 1);
    press(&mut viewer, "<CR>");
    assert!(typeset(&mut viewer));
    let first = tints(&mut viewer, PAGE_BAR);
    assert_eq!(first.len(), 1);
    // And on every line of it that is still empty.
    press(&mut viewer, "let x = 1;<CR>");
    assert!(
        viewer
            .text()
            .unwrap()
            .contains("```rust\nlet x = 1;\n\n```")
    );
    assert!(typeset(&mut viewer));
    let second = tints(&mut viewer, PAGE_BAR);
    assert_eq!(second.len(), 1);
    assert!(second[0].y > first[0].y + 5.0, "{first:?} {second:?}");
    assert!(
        (second[0].x - first[0].x).abs() < 1.0,
        "{first:?} {second:?}"
    );
    // Nothing of it is in the text.
    press(&mut viewer, "<Esc>");
    assert!(
        viewer
            .text()
            .unwrap()
            .contains("```rust\nlet x = 1;\n\n```\n")
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn an_empty_part_before_a_bracket_is_the_place_at_its_byte() {
    // The bracket is shown and begins at the byte the empty part is
    // typed at: there the cursor is in the empty part.
    let text = format!("{PAGE}One $(a^) + b$ two.\n");
    let (mut viewer, directory) = editing(&text);
    press(&mut viewer, "gsgs/One<CR>fa");
    let base = tints(&mut viewer, PAGE_CURSOR);
    press(&mut viewer, "l");
    assert_eq!(cursor(&viewer), at(&viewer, ") +"));
    let raised = tints(&mut viewer, PAGE_CURSOR);
    assert_eq!((base.len(), raised.len()), (1, 1));
    assert!(
        raised[0].y < base[0].y && raised[0].x > base[0].x,
        "{base:?} {raised:?}"
    );
    assert!(raised[0].height < base[0].height, "{base:?} {raised:?}");
    // On from it, and back to it.
    press(&mut viewer, "l");
    assert_eq!(cursor(&viewer), at(&viewer, "+"));
    press(&mut viewer, "h");
    assert_eq!(cursor(&viewer), at(&viewer, ") +"));
    press(&mut viewer, "i2<Esc>");
    assert!(viewer.text().unwrap().contains("$(a^2) + b$"));
    assert!(typeset(&mut viewer));
    let _ = std::fs::remove_dir_all(directory);
}
