//! Keys of Vim that the engine had not, or had otherwise: Ctrl-H, Ctrl-R
//! while typing, a register set from outside, and whether a key is part
//! of a command begun. On plain text, as Vim has them.

#![allow(clippy::unwrap_used, reason = "a broken harness should fail loudly")]

use std::ops::Range;

use zetwerk_vim::keys;
use zetwerk_vim::{Edit, Engine, Host};

/// A text and nothing more.
struct Plain {
    text: String,
}

impl Host for Plain {
    fn text(&self) -> &str {
        &self.text
    }

    fn edit(&mut self, edits: &[Edit], _join: bool) {
        for edit in edits {
            self.text.replace_range(edit.range.clone(), &edit.text);
        }
    }

    fn undo(&mut self) -> Option<Range<usize>> {
        None
    }

    fn redo(&mut self) -> Option<Range<usize>> {
        None
    }
}

/// The text after `keys` typed on `text`, and the engine.
fn typed(text: &str, notation: &str) -> (String, Engine) {
    let mut host = Plain {
        text: text.to_owned(),
    };
    let mut engine = Engine::new();
    for press in keys::parse(notation).unwrap() {
        engine.key(press, &mut host);
    }
    (host.text, engine)
}

#[test]
fn ctrl_h_is_backspace_in_every_mode() {
    assert_eq!(typed("abc\n", "A<C-h><Esc>").0, "ab\n");
    assert_eq!(typed("abc\ndef\n", "j<C-h>x").0, "ab\ndef\n");
    let (_, engine) = typed("abc\n", ":ab<C-h><C-h><C-h>");
    assert_eq!(engine.command_line(), None, "an empty command line closes");
}

#[test]
fn ctrl_r_while_typing_puts_in_a_register() {
    assert_eq!(typed("one two\n", "yiwA <C-r>\"<Esc>").0, "one two one\n");
    assert_eq!(
        typed("one two\n", "w\"ayiw0i<C-r>a <Esc>").0,
        "two one two\n"
    );
    // A count types it as often; `.` again.
    assert_eq!(typed("ab\n", "yl3A<C-r>0<Esc>").0, "abaaa\n");
    assert_eq!(typed("ab\n", "ylA<C-r>\"<Esc>.").0, "abaa\n");
    // A register that holds nothing, nothing.
    assert_eq!(typed("ab\n", "A<C-r>zc<Esc>").0, "abc\n");
}

#[test]
fn a_register_is_set_from_outside() {
    let mut host = Plain {
        text: "one\n".to_owned(),
    };
    let mut engine = Engine::new();
    engine.set_register('+', "copied ");
    engine.set_register('*', "line\n");
    assert_eq!(engine.register('+'), Some(("copied ", false)));
    assert_eq!(engine.register('*'), Some(("line\n", true)));
    for press in keys::parse("\"+P\"*p").unwrap() {
        engine.key(press, &mut host);
    }
    assert_eq!(host.text, "copied one\nline\n");
}

#[test]
fn a_count_or_a_register_begins_no_part_of_a_command() {
    for (keys, part) in [
        ("3", false),
        ("\"a", false),
        ("\"a2", false),
        ("\"", true),
        ("d", true),
        ("2d", true),
        ("f", true),
        ("g", true),
        ("", false),
    ] {
        assert_eq!(
            typed("one two\n", keys).1.awaits_part(),
            part,
            "after {keys}"
        );
    }
}
