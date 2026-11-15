//! Vim's keys that were missing: mappings of several keys while typing,
//! Shift-Tab, block selections, and the smaller keys a Vim user reaches
//! for.

use std::time::{Duration, Instant};

use super::*;

const PAGE: &str = "#set page(width: 300pt, height: 300pt)\n";

/// A viewer editing `text`, with `jk` mapped to Escape while typing.
fn with_jk(text: &str) -> (Viewer, PathBuf) {
    let (mut viewer, directory) = editing(text);
    viewer.set_keymap(Keymap::parse([], [("jk", "<Esc>"), ("jjx", "X")]).0);
    (viewer, directory)
}

fn mode(viewer: &Viewer) -> editor::Mode {
    viewer.editor.as_ref().unwrap().mode()
}

#[test]
fn a_mapping_of_two_keys_while_typing_takes_back_the_first() {
    let (mut viewer, directory) = with_jk("one\n");
    press(&mut viewer, "Atwo");
    // The first key shows as it is typed.
    press(&mut viewer, "j");
    assert_eq!(viewer.text(), Some("onetwoj\n"));
    press(&mut viewer, "k");
    assert_eq!(viewer.text(), Some("onetwo\n"));
    assert_eq!(mode(&viewer), editor::Mode::Normal);
    // What was typed is one undo step, and `.` types it again: not the
    // `j` taken back.
    press(&mut viewer, ".");
    assert_eq!(viewer.text(), Some("onetwotwo\n"));
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some("onetwo\n"));
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some("one\n"));
    // Right after beginning to type: nothing typed, nothing to undo.
    press(&mut viewer, "<C-r>ijk");
    assert_eq!(viewer.text(), Some("onetwo\n"));
    assert_eq!(mode(&viewer), editor::Mode::Normal);
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some("one\n"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn keys_that_make_no_mapping_stay_typed() {
    let (mut viewer, directory) = with_jk("\n");
    // Another key after the first: both typed, as they came.
    press(&mut viewer, "ijxj");
    assert_eq!(viewer.text(), Some("jxj\n"));
    // A key that begins a mapping itself after one that ended none.
    press(&mut viewer, "jk");
    assert_eq!(viewer.text(), Some("jxj\n"));
    assert_eq!(mode(&viewer), editor::Mode::Normal);
    // Three keys long: the two typed before are taken back.
    press(&mut viewer, "ojjx");
    assert_eq!(viewer.text(), Some("jxj\nX\n"));
    press(&mut viewer, "<Esc>");
    // A pause: what was typed stays, and the next key is a key.
    press(&mut viewer, "oj");
    assert!(viewer.keys_at().is_some());
    assert!(!viewer.keys_due(Instant::now()));
    assert!(!viewer.keys_due(Instant::now() + Duration::from_secs(2)));
    assert_eq!(viewer.keys_at(), None);
    press(&mut viewer, "k");
    assert_eq!(viewer.text(), Some("jxj\nX\njk\n"));
    assert_eq!(mode(&viewer), editor::Mode::Insert);
    // As does the cursor moved in between.
    press(&mut viewer, "j<Left>k");
    assert_eq!(viewer.text(), Some("jxj\nX\njkkj\n"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_mapping_while_typing_on_the_page_takes_back_what_the_page_made() {
    let text = format!("{PAGE}Some $x$ text.\n");
    let (mut viewer, directory) = with_jk(&text);
    press(&mut viewer, "gsgs");
    let at = text.find("x$").unwrap() + 1;
    viewer.editor.as_mut().unwrap().set_cursor(at);
    press(&mut viewer, "i");
    let before = viewer.text().unwrap().to_owned();
    press(&mut viewer, "y");
    assert!(viewer.text().unwrap().contains("$xy$"));
    // In an equation letters typed together make a product once the
    // word is over: what `j` makes of the equation is taken back whole.
    press(&mut viewer, "j");
    assert!(viewer.text().unwrap().contains('j'));
    press(&mut viewer, "k");
    // As Escape leaves it after `y`.
    assert!(viewer.text().unwrap().contains("Some $x y$ text."));
    assert_eq!(mode(&viewer), editor::Mode::Normal);
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some(before.as_str()));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn keys_of_a_normal_mapping_begun_are_taken_as_they_are_after_a_pause() {
    let (mut viewer, directory) = editing("one\ntwo\nthree\n");
    viewer.set_keymap(Keymap::parse([("gx", "dd")], []).0);
    press(&mut viewer, "G");
    // `g` waits for the `x` of the mapping, and nothing happens yet.
    press(&mut viewer, "g");
    assert!(viewer.keys_at().is_some());
    assert!(!viewer.keys_due(Instant::now()));
    // Then it is Vim's `g`, which waits for its own next key.
    assert!(viewer.keys_due(Instant::now() + Duration::from_secs(2)));
    assert_eq!(viewer.keys_at(), None);
    press(&mut viewer, "g");
    assert_eq!(viewer.editor.as_ref().unwrap().cursor(), 0);
    // Before a pause the mapping holds; a key that makes none is Vim's.
    press(&mut viewer, "gxgj");
    assert_eq!(viewer.text(), Some("two\nthree\n"));
    assert_eq!(viewer.editor.as_ref().unwrap().cursor(), 4);
    let _ = std::fs::remove_dir_all(directory);
}

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
    after
        .strip_prefix(PAGE)
        .map_or_else(|| format!("(no set-up) {after}"), str::to_owned)
}

#[test]
fn shift_tab_goes_back_where_tab_goes_on() {
    let cases = [
        // The argument before in an equation; from the first, to the
        // start of the equation.
        (
            "A $frac(a, b)$ z.\n",
            "b)",
            "i<S-Tab>x",
            "A $frac(xa, b)$ z.\n",
        ),
        (
            "A $frac(a, b)$ z.\n",
            "a,",
            "i<S-Tab>x",
            "A $xfrac(a, b)$ z.\n",
        ),
        // From behind it, into its last.
        (
            "A $frac(a, b)$ z.\n",
            " z",
            "i<S-Tab>x",
            "A $frac(a, xb)$ z.\n",
        ),
        // The cell before in a table; from the first, before the table.
        (
            "#table(columns: 2, [a], [b], [c], [d])\n",
            "c]",
            "i<S-Tab>x",
            "#table(columns: 2, [a], [xb], [c], [d])\n",
        ),
        // Tab and back again comes home.
        (
            "#table(columns: 2, [a], [b], [c], [d])\n",
            "b]",
            "i<Tab><S-Tab>x",
            "#table(columns: 2, [a], [xb], [c], [d])\n",
        ),
    ];
    let mut wrong = Vec::new();
    for (body, needle, keys, expected) in cases {
        let after = on_page(body, needle, keys);
        if after != expected {
            wrong.push(format!("{body:?} on {needle:?}, {keys}: {after:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn shift_tab_goes_to_the_place_to_fill_in_before() {
    let text = "Title: \u{2039}title\u{203a}\nBy: \u{2039}name\u{203a}\n";
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "<Tab><Tab><S-Tab>X<Esc>");
    assert_eq!(viewer.text(), Some("Title: X\nBy: \u{2039}name\u{203a}\n"));
    // In normal mode too, and from the first to the last.
    press(&mut viewer, "gg<S-Tab>Y<Esc>");
    assert_eq!(viewer.text(), Some("Title: X\nBy: Y\n"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn shift_enter_breaks_the_line_and_not_the_paragraph_on_a_page() {
    assert_eq!(
        on_page("Some text.\n", " text", "i<S-CR><Esc>"),
        "Some \\ text.\n"
    );
    assert_eq!(
        on_page("Some text.\n", "text", "i<S-CR>"),
        "Some \\ text.\n"
    );
    // In an equation, as Enter.
    let typed = on_page("A $x$ b.\n", "x$", "a<S-CR>");
    assert_eq!(typed, on_page("A $x$ b.\n", "x$", "a<CR>"));
    // In the source, Enter.
    let (mut viewer, directory) = editing("one two\n");
    press(&mut viewer, "wi<S-CR><Esc>");
    assert_eq!(viewer.text(), Some("one \ntwo\n"));
    let _ = std::fs::remove_dir_all(directory);
}

fn cursor(viewer: &Viewer) -> usize {
    viewer.editor.as_ref().unwrap().cursor()
}

fn told(viewer: &Viewer) -> String {
    viewer
        .message
        .as_ref()
        .map(|message| message.text.clone())
        .unwrap_or_default()
}

#[test]
fn substitute_takes_vim_patterns_and_is_made_again() {
    let (mut viewer, directory) = editing("one two\ntwo one\nthree one\n");
    // Groups and what they found.
    press(&mut viewer, ":s/\\(\\w\\+\\) \\(\\w\\+\\)/\\2 \\1/<CR>");
    assert_eq!(viewer.text(), Some("two one\ntwo one\nthree one\n"));
    // Counted, not made.
    press(&mut viewer, ":%s/one//gn<CR>");
    assert_eq!(viewer.text(), Some("two one\ntwo one\nthree one\n"));
    assert_eq!(told(&viewer), "3 matches on 3 lines");
    // `&` again on the cursor's line, `g&` on all with the flags.
    press(&mut viewer, ":s/o/0/g<CR>j&");
    assert_eq!(viewer.text(), Some("tw0 0ne\ntw0 one\nthree one\n"));
    press(&mut viewer, "g&");
    assert_eq!(viewer.text(), Some("tw0 0ne\ntw0 0ne\nthree 0ne\n"));
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some("tw0 0ne\ntw0 one\nthree one\n"));
    // `:&&` keeps the flags, `:&` does not.
    press(&mut viewer, "G:&&<CR>");
    assert_eq!(viewer.text(), Some("tw0 0ne\ntw0 one\nthree 0ne\n"));
    // `@:` runs the last command line again.
    press(&mut viewer, "k:s/w/W/<CR>gg@:");
    assert_eq!(viewer.text(), Some("tW0 0ne\ntW0 one\nthree 0ne\n"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn substitute_with_c_asks_about_each_match() {
    let (mut viewer, directory) = editing("a a a a\nb\n");
    press(&mut viewer, ":s/a/X/gc<CR>");
    assert!(told(&viewer).contains("replace with"), "{}", told(&viewer));
    // The match asked about is selected, and nothing changed yet.
    assert_eq!(viewer.editor.as_ref().unwrap().selection(), Some(0..1));
    press(&mut viewer, "yn");
    assert_eq!(viewer.editor.as_ref().unwrap().selection(), Some(4..5));
    press(&mut viewer, "l");
    assert_eq!(viewer.text(), Some("X a X a\nb\n"));
    assert_eq!(mode(&viewer), editor::Mode::Normal);
    // One undo step.
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some("a a a a\nb\n"));
    // q stops with nothing made; a makes all that are left.
    press(&mut viewer, ":s/a/X/gc<CR>q");
    assert_eq!(viewer.text(), Some("a a a a\nb\n"));
    press(&mut viewer, ":s/a/X/gc<CR>na");
    assert_eq!(viewer.text(), Some("a X X X\nb\n"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn lists_of_places_lead_there() {
    let (mut viewer, directory) = editing("one\ntwo\nthree\nfour\n");
    press(&mut viewer, "jjmagg:marks<CR><CR>");
    assert_eq!(cursor(&viewer), 8);
    press(&mut viewer, "Gxgg:changes<CR><CR>");
    assert_eq!(cursor(&viewer), 14);
    press(&mut viewer, "gg:jumps<CR><CR>");
    assert_eq!(cursor(&viewer), 14);
    // The command lines entered, to run one again.
    press(&mut viewer, ":s/o/0/<CR>q:<CR>");
    assert_eq!(
        viewer.editor.as_ref().unwrap().command_line(),
        Some("s/o/0/")
    );
    press(&mut viewer, "<Esc>:registers<CR><Esc>");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn earlier_and_later_go_through_the_history_in_order() {
    let (mut viewer, directory) = editing("a\n");
    press(&mut viewer, "xib<Esc>xic<Esc>");
    assert_eq!(viewer.text(), Some("c\n"));
    press(&mut viewer, ":earlier 2<CR>");
    assert_eq!(viewer.text(), Some("b\n"));
    press(&mut viewer, ":later<CR>");
    assert_eq!(viewer.text(), Some("\n"));
    press(&mut viewer, ":earlier 9<CR>");
    assert_eq!(viewer.text(), Some("a\n"));
    let _ = std::fs::remove_dir_all(directory);
}

const TABLE: &str = "#table(columns: 3, [a], [b], [c], [d], [e], [f], [g], [h], [i])\n";

#[test]
fn a_block_on_the_page_is_of_cells_of_a_table() {
    // `d`: the cells of the block emptied, as one undo step.
    assert_eq!(
        on_page(TABLE, "b]", "<C-v>jld"),
        "#table(columns: 3, [a], [], [], [d], [], [], [g], [h], [i])\n"
    );
    assert_eq!(on_page(TABLE, "b]", "<C-v>jldu"), TABLE);
    // `y` and `p`: cell by cell, from the cell of the cursor; as many as
    // fit.
    let text = format!("{PAGE}{TABLE}");
    let (mut viewer, directory) = editing(&text);
    press(&mut viewer, "gsgs");
    let at = |needle: &str| text.find(needle).unwrap();
    viewer.editor.as_mut().unwrap().set_cursor(at("a]"));
    press(&mut viewer, "<C-v>ljy");
    viewer.editor.as_mut().unwrap().set_cursor(at("h]"));
    press(&mut viewer, "p");
    assert_eq!(
        viewer.text().unwrap().strip_prefix(PAGE),
        Some("#table(columns: 3, [a], [b], [c], [d], [e], [f], [g], [a], [b])\n")
    );
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some(text.as_str()));
    let _ = std::fs::remove_dir_all(directory);
    // `c`: what is typed into the first goes into all.
    assert_eq!(
        on_page(TABLE, "e]", "<C-v>jlcX<Esc>"),
        "#table(columns: 3, [a], [b], [c], [d], [X], [X], [g], [X], [X])\n"
    );
    // Escape ends it; the keys are Vim's again.
    assert_eq!(
        on_page(TABLE, "a]", "<C-v>l<Esc>x"),
        on_page(TABLE, "b]", "x")
    );
}

#[test]
fn a_block_elsewhere_on_the_page_says_why_not_and_in_the_source_is_vims() {
    let text = format!("{PAGE}One two.\nThree four.\n");
    let (mut viewer, directory) = editing(&text);
    press(&mut viewer, "gsgs");
    let at = text.find("two").unwrap();
    viewer.editor.as_mut().unwrap().set_cursor(at);
    press(&mut viewer, "<C-v>");
    assert!(
        told(&viewer).contains("cells of a table"),
        "{}",
        told(&viewer)
    );
    assert_eq!(mode(&viewer), editor::Mode::Normal);
    let _ = std::fs::remove_dir_all(directory);
    // In the source: Vim's block, drawn line by line.
    let (mut viewer, directory) = editing("abc\nabc\n");
    press(&mut viewer, "l<C-v>j");
    assert_eq!(
        viewer.editor.as_ref().unwrap().selections(),
        vec![1..2, 5..6]
    );
    press(&mut viewer, "d");
    assert_eq!(viewer.text(), Some("ac\nac\n"));
    let _ = std::fs::remove_dir_all(directory);
}
