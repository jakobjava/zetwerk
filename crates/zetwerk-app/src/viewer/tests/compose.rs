//! Typing what has no key: input methods (driven by winit's events made
//! up here, as no input method runs in a test), text that comes at once,
//! characters a reader takes for one, right-to-left text, digraphs,
//! characters by number and by name, and keys of other layouts.

use winit::event::Ime as ImeEvent;
use winit::keyboard::{Key as WinitKey, KeyCode, ModifiersState, NamedKey, PhysicalKey};

use super::*;
use crate::app::ime::{self, Call, Ime, Pressed};

const PAGE: &str = "#set page(width: 300pt, height: 300pt)\n";

/// A viewer that shows only pages of `body` (behind a small page), with
/// the cursor on the first `needle`.
fn on_page(body: &str, needle: &str) -> (Viewer, PathBuf) {
    let text = format!("{PAGE}{body}");
    let (mut viewer, directory) = editing(&text);
    press(&mut viewer, "gsgs");
    let at = text
        .find(needle)
        .unwrap_or_else(|| panic!("{needle} in {body}"));
    viewer.editor.as_mut().unwrap().set_cursor(at);
    (viewer, directory)
}

fn body(viewer: &Viewer) -> String {
    let text = viewer.text().unwrap();
    text.strip_prefix(PAGE).unwrap_or(text).to_owned()
}

fn mode(viewer: &Viewer) -> editor::Mode {
    viewer.editor.as_ref().unwrap().mode()
}

fn preedit(viewer: &mut Viewer, text: &str, cursor: Option<(usize, usize)>) {
    ime::event(viewer, ImeEvent::Preedit(text.to_owned(), cursor));
}

fn commit(viewer: &mut Viewer, text: &str) {
    ime::event(viewer, ImeEvent::Preedit(String::new(), None));
    ime::event(viewer, ImeEvent::Commit(text.to_owned()));
}

/// A key of the keyboard, typing `text`, at `code`.
fn keyboard(viewer: &mut Viewer, key: &WinitKey, code: KeyCode, text: Option<&str>) {
    let pressed = Pressed {
        key,
        code: PhysicalKey::Code(code),
        text,
        modifiers: ModifiersState::empty(),
    };
    ime::keyboard(viewer, &pressed);
}

fn escape_key(viewer: &mut Viewer) {
    keyboard(
        viewer,
        &WinitKey::Named(NamedKey::Escape),
        KeyCode::Escape,
        None,
    );
}

#[test]
fn what_an_input_method_composes_is_shown_and_not_typed_until_committed() {
    let (mut viewer, directory) = on_page("Hello world.\n", "world");
    let mut window = Ime::default();
    // In normal mode, no input method: keys are commands.
    assert_eq!(window.sync(&mut viewer), vec![Call::Allow(false)]);
    press(&mut viewer, "i");
    viewer.scene(&|_| false);
    let calls = window.sync(&mut viewer);
    assert_eq!(calls.first(), Some(&Call::Allow(true)));
    // And its list of candidates goes where the cursor is drawn.
    let (x, top, bottom) = viewer.cursor_at.unwrap();
    assert!(
        calls.iter().any(|call| matches!(call,
            Call::Area(ax, ay, _, h) if (ax - x.round()).abs() < 1.0
                && (ay - top.round()).abs() < 1.0
                && (h - (bottom - top)).abs() < 2.0)),
        "{calls:?}"
    );
    assert_eq!(window.sync(&mut viewer), vec![], "nothing changed");

    let revision = viewer.editor.as_ref().unwrap().revision();
    let _ = viewer.take_edited();
    let quads = viewer.scene(&|_| false).quads.len();
    // Composed, changed on the way, a part of it being converted: shown,
    // and nothing of it in the text, its history or what is compiled.
    preedit(&mut viewer, "n", Some((1, 1)));
    preedit(&mut viewer, "ni", Some((2, 2)));
    preedit(&mut viewer, "\u{4f60}\u{597d}", Some((0, 3)));
    assert!(viewer.composing());
    assert_eq!(viewer.editor.as_ref().unwrap().revision(), revision);
    assert_eq!(viewer.take_edited(), None);
    assert_eq!(body(&viewer), "Hello world.\n");
    // It is laid over the page at the cursor.
    let scene = viewer.scene(&|_| false);
    assert!(scene.quads.len() > quads, "{} {quads}", scene.quads.len());
    assert!(
        scene
            .quads
            .iter()
            .any(|quad| (f64::from(quad.x) - x.round()).abs() < 1.0
                && f64::from(quad.width) > 10.0),
        "a slip at the cursor"
    );
    // Committed: typed, as one with the typing around it.
    commit(&mut viewer, "\u{4f60}\u{597d}");
    assert!(!viewer.composing());
    press(&mut viewer, "x");
    commit(&mut viewer, "\u{1f44d}");
    press(&mut viewer, "<Esc>");
    assert_eq!(body(&viewer), "Hello \u{4f60}\u{597d}x\u{1f44d}world.\n");
    assert_eq!(mode(&viewer), editor::Mode::Normal);
    // Normal mode again: no input method.
    assert_eq!(window.sync(&mut viewer).first(), Some(&Call::Allow(false)));
    // `.` types it again, `u` takes each back whole.
    press(&mut viewer, "0.");
    assert_eq!(
        body(&viewer),
        "\u{4f60}\u{597d}x\u{1f44d}Hello \u{4f60}\u{597d}x\u{1f44d}world.\n"
    );
    press(&mut viewer, "uu");
    assert_eq!(body(&viewer), "Hello world.\n");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn escape_while_composing_gives_up_the_composition_not_insert_mode() {
    let (mut viewer, directory) = on_page("Hello world.\n", "world");
    let mut window = Ime::default();
    press(&mut viewer, "i");
    viewer.scene(&|_| false);
    let _ = window.sync(&mut viewer);
    preedit(&mut viewer, "ka", Some((2, 2)));
    escape_key(&mut viewer);
    assert!(!viewer.composing());
    assert_eq!(mode(&viewer), editor::Mode::Insert);
    // The input method is made to drop it too: off and on again.
    assert_eq!(
        window.sync(&mut viewer)[..2],
        [Call::Allow(false), Call::Allow(true)]
    );
    // Given up by the input method itself: nothing to do.
    preedit(&mut viewer, "ka", None);
    preedit(&mut viewer, "", None);
    assert!(!viewer.composing());
    assert_eq!(window.sync(&mut viewer), vec![]);
    // Escape with nothing composed leaves insert mode, as ever.
    escape_key(&mut viewer);
    assert_eq!(mode(&viewer), editor::Mode::Normal);
    assert_eq!(body(&viewer), "Hello world.\n");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_composition_interrupted_is_given_up_where_it_was() {
    let (mut viewer, directory) = on_page("Hello world.\n", "world");
    let mut window = Ime::default();
    press(&mut viewer, "i");
    let _ = window.sync(&mut viewer);
    // By a click: given up, and the input method told; what it commits
    // after all is typed where the cursor is then.
    preedit(&mut viewer, "ni", None);
    viewer.end_composition(true);
    assert!(!viewer.composing());
    assert_eq!(
        window.sync(&mut viewer)[..2],
        [Call::Allow(false), Call::Allow(true)]
    );
    press(&mut viewer, "<Left>");
    commit(&mut viewer, "\u{4f60}");
    assert_eq!(body(&viewer), "Hello\u{4f60} world.\n");
    // By the window put aside: given up here; the input method knows.
    preedit(&mut viewer, "ni", None);
    viewer.end_composition(false);
    assert!(!viewer.composing());
    assert_eq!(window.sync(&mut viewer), vec![]);
    // By the cursor moving or the mode changing otherwise than by the
    // keyboard (keys sent by an agent): gone, and the input method off.
    preedit(&mut viewer, "ni", None);
    press(&mut viewer, "<Esc>");
    assert!(!viewer.composing());
    let calls = window.sync(&mut viewer);
    assert_eq!(calls.first(), Some(&Call::Allow(false)));
    // What it commits late, after typing is over, is no command.
    commit(&mut viewer, "dd");
    assert_eq!(body(&viewer), "Hello\u{4f60} world.\n");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn an_input_method_works_where_text_is_typed() {
    let (mut viewer, directory) = on_page("Hello world.\n", "world");
    assert!(!viewer.wants_input_method());
    // After `f`, `t` and `r` the key is a character of the text.
    press(&mut viewer, "f");
    assert!(viewer.wants_input_method());
    commit(&mut viewer, "d");
    assert_eq!(viewer.editor.as_ref().unwrap().cursor(), PAGE.len() + 10);
    // On the command line, what is composed is shown in it, underlined;
    // what is committed is typed into it.
    press(&mut viewer, ":");
    assert!(viewer.wants_input_method());
    preedit(&mut viewer, "ch", None);
    assert_eq!(viewer.composed_inline(), "c\u{332}h\u{332}");
    commit(&mut viewer, "char");
    assert_eq!(viewer.editor.as_ref().unwrap().command_line(), Some("char"));
    press(&mut viewer, "<CR>");
    // `:char` says what the character under the cursor is.
    let told = viewer.message.as_ref().unwrap().text.clone();
    assert!(told.starts_with("<d> 100, U+0064"), "{told}");
    // With the settings' leave, in normal mode too: what it commits is
    // keys there.
    let keymap = Keymap::default().with_typing(Typing {
        input_method_in_normal: true,
        by_position: true,
    });
    viewer.set_keymap(keymap);
    assert!(viewer.wants_input_method());
    commit(&mut viewer, "x");
    assert_eq!(body(&viewer), "Hello worl.\n");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn several_characters_from_one_key_are_typed_all() {
    let (mut viewer, directory) = on_page("Hello world.\n", "world");
    press(&mut viewer, "i");
    // A compose sequence that makes two, AltGr, a dead key and a plain
    // key: one way in.
    let character = |c: &str| WinitKey::Character(c.into());
    keyboard(
        &mut viewer,
        &character("f"),
        KeyCode::KeyF,
        Some("\u{fb00}x"),
    );
    keyboard(&mut viewer, &character("@"), KeyCode::KeyQ, Some("@"));
    keyboard(&mut viewer, &character("e"), KeyCode::KeyE, Some("\u{ea}"));
    keyboard(&mut viewer, &character("a"), KeyCode::KeyA, Some("a"));
    assert_eq!(body(&viewer), "Hello \u{fb00}x\\@\u{ea}aworld.\n");
    // Text that comes at once is typed as its keys would be: written to
    // be read, a line break a new paragraph.
    viewer.text_typed(" *one*\r\n");
    assert_eq!(
        body(&viewer),
        "Hello \u{fb00}x\\@\u{ea}a \\*one\\*\n\nworld.\n"
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn keys_of_another_script_are_vims_by_their_places_in_normal_mode() {
    let (mut viewer, directory) = on_page("One.\n\nTwo.\n\nThree.\n", "One");
    let russian = |c: &str| WinitKey::Character(c.into());
    let cursor = |viewer: &Viewer| viewer.editor.as_ref().unwrap().cursor();
    // `о` is on the key of `j`, `Л` (Shift) on that of `L`.
    keyboard(
        &mut viewer,
        &russian("\u{43e}"),
        KeyCode::KeyJ,
        Some("\u{43e}"),
    );
    assert_eq!(cursor(&viewer), PAGE.len() + 6, "down a line");
    // Typing, it is the letter.
    press(&mut viewer, "i");
    keyboard(
        &mut viewer,
        &russian("\u{43e}"),
        KeyCode::KeyJ,
        Some("\u{43e}"),
    );
    press(&mut viewer, "<Esc>");
    assert_eq!(body(&viewer), "One.\n\n\u{43e}Two.\n\nThree.\n");
    // As is what `f` looks for.
    press(&mut viewer, "0");
    keyboard(
        &mut viewer,
        &russian("\u{430}"),
        KeyCode::KeyF,
        Some("\u{430}"),
    );
    keyboard(
        &mut viewer,
        &russian("\u{43e}"),
        KeyCode::KeyJ,
        Some("\u{43e}"),
    );
    assert_eq!(cursor(&viewer), PAGE.len() + 6);
    // A Latin letter is never changed: `ö` is on the key of `;` on a
    // German keyboard, and no command.
    let before = viewer.text().unwrap().to_owned();
    keyboard(
        &mut viewer,
        &russian("\u{f6}"),
        KeyCode::Semicolon,
        Some("\u{f6}"),
    );
    assert_eq!(viewer.text().unwrap(), before);
    // Shift (or Caps Lock) shows in the letter: `Д` on the key of `d`
    // is `D`.
    let by = |viewer: &Viewer, c: char, code: KeyCode| {
        let press = KeyPress::plain(Key::Char(c));
        ime::by_position(viewer, press, PhysicalKey::Code(code)).key
    };
    assert_eq!(by(&viewer, '\u{414}', KeyCode::KeyD), Key::Char('D'));
    assert_eq!(
        by(&viewer, '\u{3be}', KeyCode::KeyJ),
        Key::Char('j'),
        "Greek"
    );
    assert_eq!(by(&viewer, '\u{436}', KeyCode::Semicolon), Key::Char(';'));
    // With Ctrl, in every mode.
    press(&mut viewer, "i");
    let ctrl = KeyPress {
        key: Key::Char('\u{446}'),
        ctrl: true,
        alt: false,
    };
    assert_eq!(
        ime::by_position(&viewer, ctrl, PhysicalKey::Code(KeyCode::KeyW)).key,
        Key::Char('w')
    );
    press(&mut viewer, "<Esc>");
    // The user's own mapping of the letter wins; and the settings may
    // say no.
    let (keymap, _) = Keymap::parse([("\u{43e}", "k")], []);
    viewer.set_keymap(keymap.clone());
    assert_eq!(by(&viewer, '\u{43e}', KeyCode::KeyJ), Key::Char('\u{43e}'));
    viewer.set_keymap(keymap.with_typing(Typing {
        input_method_in_normal: false,
        by_position: false,
    }));
    assert_eq!(by(&viewer, '\u{434}', KeyCode::KeyD), Key::Char('\u{434}'));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn digraphs_and_characters_by_number_are_typed() {
    let (mut viewer, directory) = on_page("Hello world.\n", "world");
    press(&mut viewer, "i<C-k>e:<C-k>a*<C-k>:e<C-k>qq");
    assert_eq!(body(&viewer), "Hello \u{eb}\u{3b1}\u{eb}qworld.\n");
    // While it waits, what it waits with is shown at the cursor.
    press(&mut viewer, "<C-k>");
    assert_eq!(viewer.composed(), Some(("?".to_owned(), Some((1, 1)))));
    press(&mut viewer, "<Esc>");
    assert_eq!(
        mode(&viewer),
        editor::Mode::Insert,
        "Escape ends the digraph"
    );
    // By number: Vim's ways, and a number cut short by another key.
    press(
        &mut viewer,
        "<C-v>u00e9<C-v>233<C-v>x41<C-v>U0001f600<C-v>ue9!",
    );
    assert_eq!(
        body(&viewer),
        "Hello \u{eb}\u{3b1}\u{eb}q\u{e9}\u{e9}A\u{1f600}\u{e9}!world.\n"
    );
    // One insert: one undo step, and `.` types it again.
    press(&mut viewer, "<Esc>u");
    assert_eq!(body(&viewer), "Hello world.\n");
    // After `r` too; and in normal mode Ctrl-V still selects a block.
    press(&mut viewer, "0r<C-k>a*");
    assert_eq!(body(&viewer), "\u{3b1}ello world.\n");
    press(&mut viewer, "<C-v>");
    assert_eq!(viewer.composed(), None);
    press(&mut viewer, "<Esc>");
    // On the command line too.
    press(&mut viewer, ":<C-k>e:");
    assert_eq!(
        viewer.editor.as_ref().unwrap().command_line(),
        Some("\u{eb}")
    );
    press(&mut viewer, "<Esc><Esc>");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn characters_are_picked_by_their_names() {
    let (mut viewer, directory) = on_page("Hello world.\n", "world");
    // In normal mode: an insert of its own.
    press(&mut viewer, ":chars<CR>");
    assert!(viewer.is_picking());
    commit(&mut viewer, "arrow.r.double");
    press(&mut viewer, "<CR>");
    assert!(!viewer.is_picking());
    assert_eq!(body(&viewer), "Hello \u{21d2}world.\n");
    assert_eq!(mode(&viewer), editor::Mode::Normal);
    // Typing, from Ctrl-K Ctrl-K; a sign of Typst's in text is written
    // to be read.
    press(&mut viewer, "A<C-k><C-k>");
    assert!(viewer.is_picking());
    press(&mut viewer, "ast.op<CR>");
    assert_eq!(body(&viewer), "Hello \u{21d2}world.\u{2217}\n");
    assert_eq!(mode(&viewer), editor::Mode::Insert);
    // In an equation, by its name.
    press(&mut viewer, " $");
    press(&mut viewer, "<C-k><C-k>alpha<CR>");
    assert!(body(&viewer).contains("$alpha$"), "{}", body(&viewer));
    press(&mut viewer, "<Esc>");
    // `:char U+…` puts a character by number; `:digraphs` lists them.
    press(&mut viewer, "0:char U+00E9<CR>");
    assert!(
        body(&viewer).starts_with("\u{e9}Hello"),
        "{}",
        body(&viewer)
    );
    press(&mut viewer, ":digraphs<CR>");
    assert!(viewer.is_picking());
    press(&mut viewer, "ss<CR>");
    assert!(
        body(&viewer).starts_with("\u{df}\u{e9}"),
        "{}",
        body(&viewer)
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_character_is_what_a_reader_takes_for_one() {
    // A letter with an accent written apart, an emoji with a skin tone,
    // a flag: `l`, `x` and Backspace take each whole, in the source and
    // on the page.
    let text = "ae\u{301}b\u{1f44d}\u{1f3fd}c\u{1f1e9}\u{1f1ea}d.\n";
    for page in [false, true] {
        let (mut viewer, directory) = editing(&format!("{PAGE}{text}"));
        if page {
            press(&mut viewer, "gsgs");
        }
        let start = PAGE.len();
        viewer.editor.as_mut().unwrap().set_cursor(start);
        let mut stops = Vec::new();
        for _ in 0..8 {
            press(&mut viewer, "l");
            stops.push(viewer.editor.as_ref().unwrap().cursor() - start);
        }
        assert_eq!(stops, vec![1, 4, 5, 13, 14, 22, 23, 23], "page {page}");
        press(&mut viewer, "0lx");
        assert_eq!(body(&viewer), text.replace("e\u{301}", ""), "page {page}");
        press(&mut viewer, "lx");
        assert_eq!(body(&viewer), "abc\u{1f1e9}\u{1f1ea}d.\n", "page {page}");
        press(&mut viewer, "A<BS><BS><BS>");
        assert_eq!(body(&viewer), "abc\n", "page {page}");
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn right_to_left_text_is_shown_and_gone_through_in_its_order() {
    let (mut viewer, directory) = on_page("Hello world.\n", "world.");
    let predicted = viewer.stats.predicted;
    press(&mut viewer, "i\u{5e9}\u{5dc}\u{5d5}\u{5dd} ");
    // Not worked out as left-to-right text: set by itself, as Typst
    // sets it, until the page is compiled.
    assert_eq!(viewer.stats.predicted, predicted, "{:?}", viewer.declined);
    assert_eq!(viewer.document_revision, Some(viewer.revision));
    press(&mut viewer, "<Esc>");
    compile(&mut viewer);
    assert_eq!(body(&viewer), "Hello \u{5e9}\u{5dc}\u{5d5}\u{5dd} world.\n");
    // `l` goes through it in the order it is written (and read), which
    // on the page is from right to left: one letter a key, to the end.
    press(&mut viewer, "0");
    let mut at = vec![viewer.editor.as_ref().unwrap().cursor() - PAGE.len()];
    for _ in 0..12 {
        press(&mut viewer, "l");
        at.push(viewer.editor.as_ref().unwrap().cursor() - PAGE.len());
    }
    assert!(at.windows(2).all(|pair| pair[0] < pair[1]), "{at:?}");
    assert!(
        at.contains(&6) && at.contains(&8) && at.contains(&12),
        "{at:?}"
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn letters_of_other_scripts_are_text_in_an_equation() {
    let (mut viewer, directory) = on_page("Hello $x$ world.\n", "x$");
    press(&mut viewer, "a");
    commit(&mut viewer, "\u{4e2d}\u{6587}");
    press(&mut viewer, "+y\u{3b1}<Esc>");
    assert_eq!(
        body(&viewer),
        "Hello $x\"\u{4e2d}\u{6587}\"+y \u{3b1}$ world.\n"
    );
    compile(&mut viewer);
    assert!(viewer.problems.is_empty(), "{:?}", viewer.problems);
    let _ = std::fs::remove_dir_all(directory);
}
