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
