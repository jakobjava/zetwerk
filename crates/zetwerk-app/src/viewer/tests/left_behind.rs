//! What typing on the page leaves in the text that the page does not
//! show, and that keeps the document from being typeset: each kind the
//! random typist found (see `typist.rs`), typed by hand.
//!
//! Where only pages are shown, the user cannot see or delete what is not
//! on them. Each test types on the page, leaves what it typed as a user
//! would (Escape, or typing on elsewhere), and asks that all of the text
//! is still a document. Those that fail are marked: they are reports.

use super::*;

/// The problems of the text as it is, typeset whole.
fn problems(viewer: &mut Viewer) -> Vec<String> {
    compile(viewer);
    viewer.state().problems
}

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

#[test]
fn a_word_typed_into_an_equation_keeps_the_document() {
    // While the cursor is behind it, the word is shown as text (put
    // between quotes for Typst). Once a blank follows it, or the cursor
    // leaves, it was a name Typst does not know: an error, and the page
    // stayed as it was before the word. Letters typed together are a
    // product now (`x y`), as soon as typing them is over.
    let (mut viewer, directory) = typed("Some $x$ here.\n", "x$", "a + ab <Esc>");
    let text = viewer.text().unwrap_or_default().to_owned();
    let problems = problems(&mut viewer);
    assert!(problems.is_empty(), "{text:?}: {problems:?}");
    // Typeset as what it means, not shown as it is written.
    assert!(viewer.written.is_none(), "{text:?}");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_dollar_sign_typed_and_left_keeps_the_document() {
    // `$` begins an equation; before a word it came alone, and one left
    // unclosed runs to where the paragraph ends: all that text was read
    // as mathematics. It comes with the one that closes it now.
    let (mut viewer, directory) =
        typed("Some words here and more.\n\nNext.\n", "here", "i$x <Esc>");
    let text = viewer.text().unwrap_or_default().to_owned();
    let problems = problems(&mut viewer);
    assert!(problems.is_empty(), "{text:?}: {problems:?}");
    // Typeset as what it means, not shown as it is written.
    assert!(viewer.written.is_none(), "{text:?}");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
#[ignore = "shows a fault: a hash and a word that is nothing of the menu stay code"]
fn a_hash_typed_and_left_keeps_the_document() {
    // `#` asks what to put in; a word that names nothing there, and
    // Escape, leaves `#x` in the text: code, that names nothing.
    let (mut viewer, directory) = typed("Some words.\n", "words", "i#xyz <Esc>");
    let text = viewer.text().unwrap_or_default().to_owned();
    let problems = problems(&mut viewer);
    assert!(problems.is_empty(), "{text:?}: {problems:?}");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
#[ignore = "shows a fault: a reference begun from the menu and left takes the next word for its label"]
fn a_reference_begun_and_left_keeps_the_document() {
    // A reference asked for from the menu and not finished: `@` stays,
    // and the word behind it becomes the name of a label there is none
    // of.
    let text = "= Intro <intro>\n\nSome words here.\n";
    let (mut viewer, directory) = typed(text, "words", "i#ref<CR><Esc>");
    let text = viewer.text().unwrap_or_default().to_owned();
    let problems = problems(&mut viewer);
    assert!(problems.is_empty(), "{text:?}: {problems:?}");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_end_of_text_set_off_is_not_taken_by_backspace() {
    // `<C-i>` begins emphasised text; Right goes out of it, over its
    // closing mark, which the page does not show; Backspace then is to
    // take what the page shows before the cursor, not that mark.
    let (mut viewer, directory) = typed("Some words.\n", "words", "A <C-i>word<Right><BS><Esc>");
    let text = viewer.text().unwrap_or_default().to_owned();
    let problems = problems(&mut viewer);
    assert!(problems.is_empty(), "{text:?}: {problems:?}");
    let _ = std::fs::remove_dir_all(directory);
}
