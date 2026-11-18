//! Characters by name: the picker of the symbols Typst has names for
//! (`:chars`, `:char <words>`), that of the digraphs (`:digraphs`), what
//! the character under the cursor is (`:char`), and a character by its
//! number (`:char U+00E9`).
//!
//! Typst's `sym` and `emoji` modules name some two thousand characters
//! (`arrow.r.double`, `alpha`, `emoji.face.grin`): a table that is there
//! without a network and is searched as the other lists are. What is
//! taken is typed as the character into running text and as its name
//! into an equation, where names are how symbols are written.

use std::fmt::Write;
use std::sync::LazyLock;

use typst::foundations::Value;
use typst::{Library, LibraryExt};

use super::digraphs::{DIGRAPHS, digraph_of};
use crate::editor;
use crate::keys::{Key, KeyPress};
use crate::typst_text::equation_at;
use crate::viewer::Viewer;
use crate::viewer::assist::Target;

/// A character Typst has a name for: the name (with its module, but for
/// `sym`, whose names an equation knows as they are), and the character.
struct Named {
    name: String,
    value: String,
    /// Whether an equation knows it by its name.
    in_math: bool,
}

/// All of them, made the first time they are asked for.
static NAMED: LazyLock<Vec<Named>> = LazyLock::new(named);

fn named() -> Vec<Named> {
    let library = Library::default();
    let mut found = Vec::new();
    for (module, in_math) in [("sym", true), ("emoji", false)] {
        let Some(Value::Module(module_value)) = library
            .global
            .scope()
            .get(module)
            .map(typst::foundations::Binding::read)
        else {
            continue;
        };
        for (name, binding) in module_value.scope().iter() {
            let Value::Symbol(symbol) = binding.read() else {
                continue;
            };
            for (modifiers, value, deprecated) in symbol.variants() {
                if deprecated.is_some() {
                    continue;
                }
                let mut full = if in_math {
                    name.to_string()
                } else {
                    format!("{module}.{name}")
                };
                if !modifiers.is_empty() {
                    full.push('.');
                    full.push_str(modifiers.as_str());
                }
                found.push(Named {
                    name: full,
                    value: value.to_owned(),
                    in_math,
                });
            }
        }
    }
    // Of names alike but for case, the small letter first: `alpha` is
    // more often meant than `Alpha`.
    found.sort_by_key(|named| named.name.starts_with(char::is_uppercase));
    found
}

/// The code points of `text`, as `U+00E9 U+0301`.
fn numbers(text: &str) -> String {
    text.chars()
        .map(|c| format!("U+{:04X}", u32::from(c)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The characters numbered in `words` (`U+00E9`, `u+e9`, `0xe9`), if all
/// of them are numbers of characters.
fn numbered(words: &str) -> Option<String> {
    words
        .split_whitespace()
        .map(|word| {
            let digits = word
                .strip_prefix("U+")
                .or_else(|| word.strip_prefix("u+"))
                .or_else(|| word.strip_prefix("0x"))?;
            u32::from_str_radix(digits, 16)
                .ok()
                .and_then(char::from_u32)
        })
        .collect::<Option<String>>()
        .filter(|text| !text.is_empty())
}

/// The name an equation knows `value` by, if it has one.
fn math_name(value: &str) -> Option<&'static str> {
    NAMED
        .iter()
        .find(|named| named.in_math && named.value == value)
        .map(|named| named.name.as_str())
}

impl Viewer {
    /// See [`Viewer::compose_command`].
    pub(super) fn chars_command(&mut self, command: &str) -> bool {
        let (name, rest) = command
            .split_once(' ')
            .map_or((command, ""), |(name, rest)| (name, rest.trim()));
        match (name, rest) {
            ("char", "") => self.tell_char(),
            ("char", words) => match numbered(words) {
                Some(text) => self.put_char(&text),
                None => self.open_chars(words),
            },
            ("chars", words) => self.open_chars(words),
            ("digraphs" | "digr", "") => self.open_digraphs(),
            _ => return false,
        }
        true
    }

    /// The list of the characters Typst has names for, with `words` typed
    /// into it.
    pub(super) fn open_chars(&mut self, words: &str) {
        let items = NAMED
            .iter()
            .map(|named| {
                let label = format!("{}  {}  {}", named.value, named.name, numbers(&named.value));
                let take = Target::Command(format!("char {}", numbers(&named.value)));
                (label, take)
            })
            .collect();
        self.open_picker("characters", items, 0);
        for c in words.chars() {
            self.key(KeyPress::plain(Key::Char(c)));
        }
    }

    /// The list of Vim's digraphs.
    fn open_digraphs(&mut self) {
        let items = DIGRAPHS
            .iter()
            .map(|(pair, c)| {
                let value = c.to_string();
                let name = math_name(&value).unwrap_or_default();
                let label = format!("{c}  {pair}  {}  {name}", numbers(&value));
                (label, Target::Command(format!("char {}", numbers(&value))))
            })
            .collect();
        self.open_picker("digraphs (Ctrl-K and the two)", items, 0);
    }

    /// Says what the character under the cursor is: its number, its
    /// digraph and its name in Typst, for each code point of it.
    fn tell_char(&mut self) {
        let Some(editor) = &self.editor else {
            self.alert("no text here");
            return;
        };
        let (text, at) = (editor.text(), editor.cursor());
        let end = zetwerk_vim::Text(text).next(at);
        let Some(shown) = text.get(at..end).filter(|shown| !shown.is_empty()) else {
            self.notice("NUL");
            return;
        };
        let told: Vec<String> = shown
            .chars()
            .map(|c| {
                let mut told = format!("<{c}> {}, U+{:04X}", u32::from(c), u32::from(c));
                if let Some(pair) = digraph_of(c) {
                    let _ = write!(told, ", digraph {pair}");
                }
                let value = c.to_string();
                if let Some(named) = NAMED.iter().find(|named| named.value == value) {
                    let _ = write!(told, ", Typst {}", named.name);
                }
                told
            })
            .collect();
        self.notice(told.join("  +  "));
    }

    /// Types `text` at the cursor: as itself into running text, as its
    /// name into an equation. In normal mode, as an insert of its own.
    fn put_char(&mut self, text: &str) {
        let Some(editor) = &self.editor else {
            self.alert("no text to put it into");
            return;
        };
        let normal = editor.mode() != editor::Mode::Insert;
        let in_math = equation_at(editor.source(), editor.cursor()).is_some();
        let name = in_math.then(|| math_name(text)).flatten();
        if normal {
            self.key(KeyPress::plain(Key::Char('i')));
        }
        match name {
            Some(name) => self.put_name(name),
            None => self.text_typed(text),
        }
        if normal {
            self.key(KeyPress::plain(Key::Escape));
        }
    }

    /// Puts `name` into an equation at the cursor, with blanks between it
    /// and letters around it, as one typed text.
    fn put_name(&mut self, name: &str) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let (text, at) = (editor.text(), editor.cursor());
        let joins = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '.');
        let before = joins(text.get(..at).and_then(|before| before.chars().next_back()));
        let after = joins(text.get(at..).and_then(|after| after.chars().next()));
        let mut put = String::new();
        if before {
            put.push(' ');
        }
        put.push_str(name);
        if after {
            put.push(' ');
        }
        let within = put.len();
        let press = editor.completed(0, put, within);
        self.editor_key(press);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typst_names_hundreds_of_characters() {
        assert!(NAMED.len() > 1000, "{}", NAMED.len());
        assert_eq!(math_name("\u{3b1}"), Some("alpha"));
        assert_eq!(math_name("\u{2192}"), Some("arrow.r"));
        assert!(NAMED.iter().any(|named| named.name.starts_with("emoji.")));
        assert_eq!(numbered("U+00E9 u+301"), Some("\u{e9}\u{301}".to_owned()));
        assert_eq!(numbered("alpha"), None);
        assert_eq!(numbers("\u{e9}"), "U+00E9");
    }
}
