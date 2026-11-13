//! Key presses, independent of where they come from.
//!
//! The window turns keyboard events into [`KeyPress`]es; tests and the MCP
//! server write them in Vim's notation (`jj<C-d>:perf<CR>`). Both go through
//! the same handling.

/// A key, with Shift already applied to characters (`G`, not Shift+`g`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Escape,
    Backspace,
    /// The key that deletes forwards (`Entf` on a German keyboard).
    Delete,
    Tab,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    /// Not a key of any keyboard: a text put in whole in insert mode, as
    /// when one of a list of completions is taken. The number says which,
    /// to the engine that made it (see `Engine::completed`); it is what
    /// `.` and macros replay.
    Text(u32),
}

/// A key pressed together with modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyPress {
    pub key: Key,
    pub ctrl: bool,
    pub alt: bool,
}

impl KeyPress {
    #[must_use]
    pub fn plain(key: Key) -> Self {
        Self {
            key,
            ctrl: false,
            alt: false,
        }
    }
}

/// Parses keys written in Vim's notation: plain characters stand for
/// themselves, special keys and modifiers are written in angle brackets
/// (`<Esc>`, `<CR>`, `<C-d>`, `<A-x>`, `<C-A-Left>`), and a literal `<` is
/// `<lt>`.
pub fn parse(notation: &str) -> Result<Vec<KeyPress>, String> {
    let mut keys = Vec::new();
    let mut rest = notation;
    while let Some(c) = rest.chars().next() {
        if c != '<' {
            keys.push(KeyPress::plain(Key::Char(c)));
            rest = &rest[c.len_utf8()..];
            continue;
        }
        let end = rest
            .find('>')
            .ok_or_else(|| format!("unclosed '<' in {notation:?}"))?;
        keys.push(parse_special(&rest[1..end])?);
        rest = &rest[end + 1..];
    }
    Ok(keys)
}

fn parse_special(name: &str) -> Result<KeyPress, String> {
    let mut press = KeyPress::plain(Key::Escape);
    let mut name = name;
    loop {
        if let Some(rest) = name.strip_prefix("C-") {
            press.ctrl = true;
            name = rest;
        } else if let Some(rest) = name.strip_prefix("A-").or_else(|| name.strip_prefix("M-")) {
            press.alt = true;
            name = rest;
        } else {
            break;
        }
    }
    let mut chars = name.chars();
    press.key = match (
        name.to_ascii_lowercase().as_str(),
        chars.next(),
        chars.next(),
    ) {
        ("esc", ..) => Key::Escape,
        ("cr" | "enter" | "return", ..) => Key::Enter,
        ("bs" | "backspace", ..) => Key::Backspace,
        ("del" | "delete", ..) => Key::Delete,
        ("tab", ..) => Key::Tab,
        ("space", ..) => Key::Char(' '),
        ("lt", ..) => Key::Char('<'),
        ("bar", ..) => Key::Char('|'),
        ("up", ..) => Key::Up,
        ("down", ..) => Key::Down,
        ("left", ..) => Key::Left,
        ("right", ..) => Key::Right,
        ("pageup", ..) => Key::PageUp,
        ("pagedown", ..) => Key::PageDown,
        ("home", ..) => Key::Home,
        ("end", ..) => Key::End,
        (_, Some(c), None) => Key::Char(c),
        _ => return Err(format!("unknown key <{name}>")),
    };
    Ok(press)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_characters_are_themselves() {
        assert_eq!(
            parse("gG:é"),
            Ok(vec![
                KeyPress::plain(Key::Char('g')),
                KeyPress::plain(Key::Char('G')),
                KeyPress::plain(Key::Char(':')),
                KeyPress::plain(Key::Char('é')),
            ])
        );
    }

    #[test]
    fn special_keys_and_modifiers() {
        assert_eq!(
            parse("<C-d><Esc><CR><lt><C-A-Left><space>"),
            Ok(vec![
                KeyPress {
                    key: Key::Char('d'),
                    ctrl: true,
                    alt: false
                },
                KeyPress::plain(Key::Escape),
                KeyPress::plain(Key::Enter),
                KeyPress::plain(Key::Char('<')),
                KeyPress {
                    key: Key::Left,
                    ctrl: true,
                    alt: true
                },
                KeyPress::plain(Key::Char(' ')),
            ])
        );
    }

    #[test]
    fn malformed_notation_is_an_error() {
        assert!(parse("<C-d").is_err());
        assert!(parse("<Nope>").is_err());
        assert!(parse("<>").is_err());
    }
}
