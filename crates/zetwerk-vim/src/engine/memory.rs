//! What the engine remembers besides the text and the cursor, written
//! down and taken back: for a replay of keys that begins in the middle
//! of a session (see the host's reports). The registers, the last
//! search, what `;` finds, what `.` repeats (unless it was a change of a
//! selection), the last macro, the marks, where typing was left, the
//! last selection and the jumps. Not the undo history: that is the
//! host's.
//!
//! One thing a line, `what: value`; text is escaped so that it stays on
//! its line. Lines that are not understood are passed over.

use std::fmt::Write as _;

use super::{Engine, Mode, Recorded, Register, Search};
use crate::keys::{Key, KeyPress};

impl Engine {
    /// What the engine remembers beyond the text and the cursor, as lines
    /// of text; [`Engine::recall`] takes it back.
    #[must_use]
    pub fn memory(&self) -> String {
        let mut text = String::new();
        let mut registers: Vec<_> = self.registers.iter().collect();
        registers.sort_by_key(|(name, _)| **name);
        for (name, register) in registers {
            let lines = u8::from(register.linewise);
            let _ = writeln!(
                text,
                "register: {} {lines} {}",
                escape(&name.to_string()),
                escape(&register.text)
            );
        }
        if let Some(search) = &self.last_search {
            let _ = writeln!(
                text,
                "search: {} {} {} {}",
                u8::from(search.forward),
                u8::from(search.word_start),
                u8::from(search.word_end),
                escape(&search.pattern)
            );
        }
        if let Some((c, a, b)) = self.last_find {
            let _ = writeln!(
                text,
                "find: {} {} {}",
                u8::from(a),
                u8::from(b),
                escape(&c.to_string())
            );
        }
        if let Some(change) = self
            .last_change
            .as_ref()
            .filter(|change| change.shape.is_none())
        {
            let count = change
                .count
                .map_or_else(|| "-".to_owned(), |count| count.to_string());
            let keys: Vec<String> = change.keys.iter().map(|press| key(*press)).collect();
            let _ = writeln!(text, "change: {count} {}", keys.join(","));
        }
        if let Some(name) = self.last_macro {
            let _ = writeln!(text, "macro: {}", escape(&name.to_string()));
        }
        let mut marks: Vec<_> = self.marks.iter().collect();
        marks.sort_by_key(|(name, _)| **name);
        for (name, at) in marks {
            let _ = writeln!(text, "mark: {at} {}", escape(&name.to_string()));
        }
        if let Some(at) = self.last_insert {
            let _ = writeln!(text, "insert: {at}");
        }
        if let Some((from, to, mode)) = self.last_visual {
            let lines = u8::from(mode == Mode::VisualLine);
            let _ = writeln!(text, "visual: {from} {to} {lines}");
        }
        if let Some(at) = self.jump {
            let _ = writeln!(text, "jump: {at}");
        }
        let jumps: Vec<String> = self.jumps.iter().map(ToString::to_string).collect();
        let _ = writeln!(text, "jumps: {} {}", self.jumps_at, jumps.join(","));
        for (number, put, within) in &self.texts {
            let _ = writeln!(text, "text: {number} {within} {}", escape(put));
        }
        text
    }

    /// Takes back what [`Engine::memory`] wrote, in place of what it
    /// remembers of those things now.
    #[allow(clippy::too_many_lines, reason = "an arm for each thing remembered")]
    pub fn recall(&mut self, memory: &str) {
        self.registers.clear();
        self.last_search = None;
        self.last_find = None;
        self.last_change = None;
        self.last_macro = None;
        self.marks.clear();
        self.last_insert = None;
        self.last_visual = None;
        self.jump = None;
        self.texts.clear();
        for line in memory.lines() {
            let Some((what, value)) = line.split_once(": ") else {
                continue;
            };
            let words: Vec<&str> = value.splitn(4, ' ').collect();
            let number = |at: usize| words.get(at).and_then(|word| word.parse::<usize>().ok());
            let flag = |at: usize| words.get(at).map(|word| *word == "1");
            let char_at = |at: usize| {
                let text = unescape(words.get(at)?);
                let mut chars = text.chars();
                chars.next().filter(|_| chars.next().is_none())
            };
            match what {
                "register" => {
                    let (Some(name), Some(linewise)) = (char_at(0), flag(1)) else {
                        continue;
                    };
                    let text = value
                        .splitn(3, ' ')
                        .nth(2)
                        .map(unescape)
                        .unwrap_or_default();
                    self.registers.insert(name, Register { text, linewise });
                }
                "search" => {
                    let (Some(forward), Some(word_start), Some(word_end)) =
                        (flag(0), flag(1), flag(2))
                    else {
                        continue;
                    };
                    self.last_search = Some(Search {
                        pattern: words.get(3).map(|text| unescape(text)).unwrap_or_default(),
                        forward,
                        word_start,
                        word_end,
                    });
                }
                "find" => {
                    if let (Some(a), Some(b), Some(c)) = (flag(0), flag(1), char_at(2)) {
                        self.last_find = Some((c, a, b));
                    }
                }
                "change" => {
                    let mut parts = value.splitn(2, ' ');
                    let count = parts.next().and_then(|count| count.parse().ok());
                    let keys = parts.next().unwrap_or("");
                    let keys: Option<Vec<KeyPress>> = keys
                        .split(',')
                        .filter(|key| !key.is_empty())
                        .map(unkey)
                        .collect();
                    if let Some(keys) = keys {
                        self.last_change = Some(Recorded {
                            count,
                            keys,
                            shape: None,
                        });
                    }
                }
                "macro" => self.last_macro = char_at(0),
                "mark" => {
                    if let (Some(at), Some(name)) = (number(0), char_at(1)) {
                        self.marks.insert(name, at);
                    }
                }
                "insert" => self.last_insert = number(0),
                "visual" => {
                    if let (Some(from), Some(to), Some(lines)) = (number(0), number(1), flag(2)) {
                        let mode = if lines {
                            Mode::VisualLine
                        } else {
                            Mode::Visual
                        };
                        self.last_visual = Some((from, to, mode));
                    }
                }
                "jump" => self.jump = number(0),
                "jumps" => {
                    let mut parts = value.splitn(2, ' ');
                    self.jumps_at = parts.next().and_then(|at| at.parse().ok()).unwrap_or(0);
                    self.jumps = parts
                        .next()
                        .unwrap_or("")
                        .split(',')
                        .filter_map(|at| at.parse().ok())
                        .collect();
                    self.jumps_at = self.jumps_at.min(self.jumps.len());
                }
                "text" => {
                    if let (Some(number), Some(within)) = (number(0), number(1)) {
                        let put = value
                            .splitn(3, ' ')
                            .nth(2)
                            .map(unescape)
                            .unwrap_or_default();
                        self.texts.push((number, put, within));
                    }
                }
                _ => {}
            }
        }
    }
}

/// `text` with what would end its line, or be taken for something else,
/// escaped.
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            ' ' => escaped.push_str("\\s"),
            c => escaped.push(c),
        }
    }
    escaped
}

fn unescape(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            plain.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => plain.push('\n'),
            Some('r') => plain.push('\r'),
            Some('s') => plain.push(' '),
            Some(other) => plain.push(other),
            None => {}
        }
    }
    plain
}

/// A key as `memory` writes it: what kind, a number, and the modifiers.
fn key(press: KeyPress) -> String {
    let (kind, number) = match press.key {
        Key::Char(c) => ('c', u32::from(c)),
        Key::Text(number) => ('t', number),
        other => (
            'k',
            NAMED
                .iter()
                .position(|named| *named == other)
                .map_or(0, |at| at as u32),
        ),
    };
    format!(
        "{kind}{number}.{}{}",
        u8::from(press.ctrl),
        u8::from(press.alt)
    )
}

/// The keys that are no character, in the order `key` numbers them.
const NAMED: [Key; 13] = [
    Key::Enter,
    Key::Escape,
    Key::Backspace,
    Key::Delete,
    Key::Tab,
    Key::Up,
    Key::Down,
    Key::Left,
    Key::Right,
    Key::PageUp,
    Key::PageDown,
    Key::Home,
    Key::End,
];

fn unkey(written: &str) -> Option<KeyPress> {
    let (body, modifiers) = written.split_once('.')?;
    let kind = body.chars().next()?;
    let number: u32 = body[kind.len_utf8()..].parse().ok()?;
    let key = match kind {
        'c' => Key::Char(char::from_u32(number)?),
        't' => Key::Text(number),
        'k' => *NAMED.get(number as usize)?,
        _ => return None,
    };
    let mut flags = modifiers.chars();
    Some(KeyPress {
        key,
        ctrl: flags.next() == Some('1'),
        alt: flags.next() == Some('1'),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_remembered_is_taken_back_whole() {
        let mut engine = Engine::new();
        engine.set_register('a', "one two\n");
        engine.set_register('"', "a\\b c");
        engine.last_find = Some(('x', true, false));
        engine.marks.insert('q', 12);
        engine.last_change = Some(Recorded {
            count: Some(3),
            keys: vec![
                KeyPress::plain(Key::Char('x')),
                KeyPress::plain(Key::Escape),
            ],
            shape: None,
        });
        let memory = engine.memory();
        assert!(memory.contains("register: a 1 one\\stwo\\n"), "{memory}");
        let mut other = Engine::new();
        other.recall(&memory);
        assert_eq!(other.memory(), memory);
        for text in ["a b\\c\nd\r", "", " "] {
            assert_eq!(unescape(&escape(text)), text);
        }
        let press = KeyPress {
            key: Key::Home,
            ctrl: true,
            alt: false,
        };
        assert_eq!(unkey(&key(press)), Some(press));
    }
}
