//! Keys and commands of Vim that came late: the command-line history
//! (Up, Down, Ctrl-P, Ctrl-N, `@:`, `q:`), Ctrl-R on the command line,
//! the change list (`g;`, `g,`), Ctrl-A and Ctrl-X on numbers, and the
//! commands of the command line that are about lines of the text:
//! `:normal`, `:g` and `:v`, `:sort`.
//!
//! What they keep is in [`More`], one field of the engine; the engine's
//! own handlers call in here at one place each.

use std::ops::Range;

use super::{Effect, Engine, Host, Mode, clamp, first_non_blank_or_end};
use crate::keys::{Key, KeyPress};
use crate::pattern::Pattern;
use crate::text::{Class, Text};

/// How many command lines (and searches) are remembered, as Vim's
/// 'history'.
const HISTORY: usize = 200;

/// How many places of changes are remembered, as in Vim.
const CHANGES: usize = 100;

/// What the keys of this module keep between keys.
#[derive(Debug, Clone, Default)]
pub(super) struct More {
    /// The command lines entered, the oldest first, each with what it was
    /// for (`:`, or `/` and `?` for a search).
    history: Vec<(char, String)>,
    /// While the command line is open and Up or Down went through the
    /// history: which entry it shows, and what was typed before.
    browsing: Option<(usize, String)>,
    /// Whether Ctrl-R was typed on the command line, and waits for the
    /// name of a register (or Ctrl-W for the word under the cursor).
    register: bool,
    /// The places of the changes made, the oldest first, and which of
    /// them `g;` went to last: their number where it has not gone back.
    changes: Vec<usize>,
    changes_at: usize,
    /// The lines `:g` is still to work on, as bytes that stay with their
    /// text while it changes.
    lines: Vec<usize>,
    /// Whether the selection is a block (Ctrl-V, see `block`).
    pub(super) block: bool,
    /// The registers that hold a block, and the text they held then: a
    /// register that holds another since holds none.
    pub(super) blocks: Vec<(char, String)>,
    /// What `I`, `A` and `c` of a block put into its other lines on
    /// Escape, and for `I` and `A` the corner the cursor goes back to.
    pub(super) block_insert: Option<super::block::BlockInsert>,
    pub(super) insert_corner: Option<(usize, usize)>,
}

impl More {
    /// Moves what is remembered by position along with a change of the
    /// text: `start..old_end` became `start..new_end`.
    pub(super) fn shift(&mut self, start: usize, old_end: usize, new_end: usize) {
        let shift = |at: &mut usize| {
            if *at >= old_end {
                *at = *at - old_end + new_end;
            } else if *at > start {
                *at = start;
            }
        };
        self.changes.iter_mut().for_each(shift);
        self.lines.iter_mut().for_each(shift);
    }

    /// Remembers that a change is made at byte `at` of `text` (as it is
    /// before the change), as Vim keeps the change list: the first change
    /// of an undo step (`new`) is a place of its own, or takes the place
    /// of the last where it is near it (in its line, at most 79 columns
    /// from it); the changes after it in the step are not.
    pub(super) fn changed(&mut self, text: Text, at: usize, new: bool) {
        // Only the first change of a step counts.
        if !new && !self.changes.is_empty() {
            return;
        }
        let near = self.changes.last().is_some_and(|&last| {
            let last = last.min(text.len());
            text.line_index(last) == text.line_index(at)
                && text.column(last).abs_diff(text.column(at)) <= NEAR
        });
        if near {
            if let Some(last) = self.changes.last_mut() {
                *last = at;
            }
        } else {
            self.changes.push(at);
            if self.changes.len() > CHANGES {
                self.changes.remove(0);
            }
        }
        self.changes_at = self.changes.len();
    }
}

/// How many columns from the last place of a change one in the same line
/// is near it, as Vim has it where no 'textwidth' is set.
const NEAR: usize = 79;

impl Engine {
    /// The command lines entered for `prompt` (`:` for commands, `/` for
    /// searches either way), the oldest first.
    #[must_use]
    pub fn command_history(&self, prompt: char) -> Vec<&str> {
        let kind = |of: char| if of == ':' { ':' } else { '/' };
        self.more
            .history
            .iter()
            .filter(|(of, _)| kind(*of) == kind(prompt))
            .map(|(_, line)| line.as_str())
            .collect()
    }

    /// The places of the changes made, the oldest first (`:changes`).
    #[must_use]
    pub fn changes(&self) -> &[usize] {
        &self.more.changes
    }

    /// The places jumped from, the oldest first (`:jumps`).
    #[must_use]
    pub fn jumps(&self) -> &[usize] {
        &self.jumps
    }

    /// The marks set, by name (`:marks`).
    #[must_use]
    pub fn marks(&self) -> Vec<(char, usize)> {
        let mut marks: Vec<(char, usize)> = self.marks.iter().map(|(n, at)| (*n, *at)).collect();
        marks.sort_unstable();
        marks
    }

    /// The registers that hold something, by name (`:registers`): what
    /// each holds and whether it is whole lines.
    #[must_use]
    pub fn registers(&self) -> Vec<(char, &str, bool)> {
        let mut registers: Vec<(char, &str, bool)> = self
            .registers
            .iter()
            .map(|(name, register)| (*name, register.text.as_str(), register.linewise))
            .collect();
        // As Vim lists them: the unnamed one, the numbered, the named.
        let rank = |name: char| match name {
            '"' => 0,
            '0'..='9' => 1,
            'a'..='z' => 2,
            _ => 3,
        };
        registers.sort_unstable_by_key(|(name, ..)| (rank(*name), *name));
        registers
    }

    // ---- The command line ---------------------------------------------------

    /// A key on the command line that is for this module: Up, Down,
    /// Ctrl-P and Ctrl-N through the history, Ctrl-R, Ctrl-W and Ctrl-U;
    /// Enter, to remember the line and to run what is run here. Returns
    /// whether the key was taken.
    pub(super) fn command_line_key(&mut self, press: KeyPress, host: &mut impl Host) -> bool {
        if std::mem::take(&mut self.more.register) {
            self.command_line_register(press, host);
            return true;
        }
        match (press.key, press.ctrl) {
            (Key::Up, _) | (Key::Char('p'), true) => {
                self.browse(true, press.key == Key::Up);
                true
            }
            (Key::Down, _) | (Key::Char('n'), true) => {
                self.browse(false, press.key == Key::Down);
                true
            }
            (Key::Char('r'), true) => {
                self.more.register = true;
                true
            }
            (Key::Char('u'), true) => {
                self.command_line.clear();
                true
            }
            (Key::Char('w'), true) => {
                let line = &self.command_line;
                let class = |c: char| crate::text::class(c, false);
                let kept = line.trim_end_matches(|c| class(c) == Class::Blank);
                let last = kept.chars().next_back().map(class);
                let kept = kept.trim_end_matches(|c| Some(class(c)) == last);
                self.command_line.truncate(kept.len());
                true
            }
            (Key::Enter, _) => {
                self.more.browsing = None;
                let line = self.command_line.trim().to_owned();
                if line.is_empty() {
                    return false;
                }
                self.remember_command_line(self.prompt, &line);
                if self.prompt == ':' {
                    // The `:` register holds the last command line.
                    self.registers.insert(
                        ':',
                        super::Register {
                            text: line.clone(),
                            linewise: false,
                        },
                    );
                    if self.line_command(&line, host) {
                        self.command_line.clear();
                        return true;
                    }
                }
                false
            }
            (Key::Escape, _) => {
                self.more.browsing = None;
                false
            }
            _ => false,
        }
    }

    /// Puts `line`, entered for `prompt`, last in the history, and nowhere
    /// else in it.
    fn remember_command_line(&mut self, prompt: char, line: &str) {
        let kind = |of: char| of == ':';
        self.more
            .history
            .retain(|(of, entered)| kind(*of) != kind(prompt) || entered != line);
        self.more.history.push((prompt, line.to_owned()));
        if self.more.history.len() > HISTORY {
            self.more.history.remove(0);
        }
    }

    /// Up or Down (`prefixed`: only lines that begin as what was typed),
    /// Ctrl-P or Ctrl-N: the line entered before the one shown, or after.
    fn browse(&mut self, back: bool, prefixed: bool) {
        let entries: Vec<String> = self
            .command_history(self.prompt)
            .into_iter()
            .map(str::to_owned)
            .collect();
        let (at, typed) = self
            .more
            .browsing
            .clone()
            .unwrap_or_else(|| (entries.len(), self.command_line.clone()));
        let fits = |line: &String| !prefixed || line.starts_with(&typed);
        let found = if back {
            (0..at).rev().find(|index| fits(&entries[*index]))
        } else {
            (at + 1..entries.len()).find(|index| fits(&entries[*index]))
        };
        match found {
            Some(index) => {
                self.command_line.clone_from(&entries[index]);
                self.more.browsing = Some((index, typed));
            }
            // Past the newest: what was typed, again.
            None if !back => {
                self.command_line.clone_from(&typed);
                self.more.browsing = None;
            }
            None => self.failed = true,
        }
    }

    /// The key after Ctrl-R on the command line: the name of a register,
    /// whose text is put in; Ctrl-W for the word under the cursor, Ctrl-A
    /// for the WORD.
    fn command_line_register(&mut self, press: KeyPress, host: &impl Host) {
        let text = Text(host.text());
        let put = match (press.key, press.ctrl) {
            (Key::Char(c @ ('w' | 'a')), true) => {
                let big = c == 'a';
                let at = clamp(text, self.cursor, false);
                word_at(text, at, big).map(|range| text.0[range].to_owned())
            }
            (Key::Char(name), false) => self
                .registers
                .get(&name.to_ascii_lowercase())
                .map(|register| register.text.clone()),
            _ => None,
        };
        match put {
            // A line break would end the line: Vim shows it as `^M`.
            Some(put) => self
                .command_line
                .push_str(put.trim_end_matches('\n').replace('\n', "\r").as_str()),
            None => self.failed = true,
        }
    }

    // ---- Keys of normal mode -------------------------------------------------

    /// `@:`: the last command line again, `count` times; `@@` after it too.
    /// Returns whether the key was `:` (or `@` after `@:`).
    pub(super) fn play_command_line(&mut self, press: KeyPress, count: usize) -> bool {
        let again = press.key == Key::Char('@') && self.last_macro == Some(':');
        if press.key != Key::Char(':') && !again {
            return false;
        }
        let Some(line) = self
            .registers
            .get(&':')
            .map(|register| register.text.clone())
        else {
            self.fail();
            return true;
        };
        self.last_macro = Some(':');
        let mut keys = Vec::new();
        for _ in 0..count {
            keys.push(KeyPress::plain(Key::Char(':')));
            keys.extend(line.chars().map(|c| KeyPress::plain(Key::Char(c))));
            keys.push(KeyPress::plain(Key::Enter));
        }
        for key in keys.into_iter().rev() {
            self.queue.push_front(key);
        }
        true
    }

    /// `q:`, `q/` and `q?`: the host's list of the command lines entered,
    /// or of the searches. Returns whether the key was one of those.
    pub(super) fn list_command_lines(&mut self, press: KeyPress) -> bool {
        let Key::Char(c @ (':' | '/' | '?')) = press.key else {
            return false;
        };
        let list = if c == ':' { "q:" } else { "q/" };
        self.effects.push(Effect::Command(list.to_owned()));
        true
    }

    /// `g;` (`back`) and `g,`: to the place of an older change, `count`
    /// of them, or of a newer one.
    pub(super) fn change_list(&mut self, back: bool, host: &impl Host) {
        let (count, _) = self.take_count();
        self.reset_pending();
        let at = self.more.changes_at;
        let to = if back {
            at.checked_sub(count)
        } else {
            Some(at.saturating_add(count))
        };
        let changes = self.more.changes.len();
        let to = match to {
            Some(to) if to < changes => Some(to),
            // As far as there are, as Vim goes; not at all from the end.
            None if at > 0 => Some(0),
            Some(_) if !back && at + 1 < changes => Some(changes - 1),
            _ => None,
        };
        let Some(to) = to else {
            return self.fail();
        };
        self.more.changes_at = to;
        let text = Text(host.text());
        let mode_end = self.mode != Mode::Normal;
        self.cursor = clamp(text, self.more.changes[to], mode_end);
        self.wanted_column = None;
    }

    /// Ctrl-A (`by` 1) and Ctrl-X (`by` -1): the number under the cursor
    /// or the next one in the line made `count` times `by` larger, as Vim
    /// does it with 'nrformats' of `bin,hex`. Whether one was found.
    pub(super) fn add_to_number(&mut self, by: i64, host: &mut impl Host) {
        let (count, _) = self.take_count();
        self.reset_pending();
        if self.selecting() {
            return self.fail();
        }
        let text = Text(host.text());
        let Some(number) = number_at(text, self.cursor) else {
            return self.fail();
        };
        let amount = i128::from(by) * i128::try_from(count).unwrap_or(i128::MAX);
        let written = number.add(amount);
        let range = number.range.clone();
        self.edit(host, range.clone(), &written);
        self.cursor = range.start + written.len().saturating_sub(1);
        self.changed = true;
    }

    // ---- Commands of the command line ---------------------------------------

    /// Runs `line`, entered on the command line, if it is one of the
    /// commands this module carries out. Returns whether it was.
    fn line_command(&mut self, line: &str, host: &mut impl Host) -> bool {
        let (range, rest) = LineRange::read(line);
        let (name, argument) = split_name(rest);
        match name {
            "norm" | "norma" | "normal" | "norm!" | "norma!" | "normal!" => {
                if argument.is_empty() {
                    return false;
                }
                self.mode = Mode::Normal;
                let lines = range.lines(Text(host.text()), self.cursor);
                self.normal_on_lines(lines, argument, host);
                true
            }
            "g" | "gl" | "global" | "g!" | "global!" | "v" | "vg" | "vglobal" => {
                let inverse = name.starts_with('v') || name.ends_with('!');
                self.mode = Mode::Normal;
                let lines = range.or_all().lines(Text(host.text()), self.cursor);
                self.global(lines, argument, inverse, host);
                true
            }
            "sor" | "sort" | "sor!" | "sort!" => {
                self.mode = Mode::Normal;
                let lines = range.or_all().lines(Text(host.text()), self.cursor);
                self.sort(&lines, argument, name.ends_with('!'), host);
                true
            }
            _ => false,
        }
    }

    /// `:normal {keys}` on each of `lines` (starts of lines): the keys as
    /// if typed there, a command left unfinished ended with Escape. All of
    /// it is one undo step.
    fn normal_on_lines(&mut self, lines: Vec<usize>, keys: &str, host: &mut impl Host) {
        let keys: Vec<KeyPress> = keys
            .chars()
            .map(|c| KeyPress::plain(Key::Char(c)))
            .collect();
        let several = lines.len() > 1;
        self.more.lines = lines;
        self.joined = false;
        while !self.more.lines.is_empty() {
            let line = self.more.lines.remove(0);
            if several {
                let text = Text(host.text());
                self.cursor = clamp(text, line, false);
            }
            self.play_keys(&keys, host);
        }
    }

    /// Plays `keys` as `:normal` does: as if typed, as one undo step with
    /// what came before in this command, and up to the first that fails;
    /// what is left unfinished is ended with Escape.
    fn play_keys(&mut self, keys: &[KeyPress], host: &mut impl Host) {
        let playing = std::mem::replace(&mut self.playing, true);
        // Keys of a macro that are still to come are not for these.
        let waiting = std::mem::take(&mut self.queue);
        for &key in keys {
            self.failed = false;
            self.dispatch(key, host);
            // A macro the keys started plays too.
            while let Some(next) = self.queue.pop_front() {
                self.dispatch(next, host);
                if self.failed {
                    self.queue.clear();
                }
            }
            if self.failed {
                break;
            }
        }
        if self.mode != Mode::Normal || !self.pending.is_idle() {
            self.dispatch(KeyPress::plain(Key::Escape), host);
        }
        self.failed = false;
        self.playing = playing;
        self.queue = waiting;
    }

    /// `:g/pattern/command` (or `:v`, with `inverse`, on the lines that do
    /// not match) on `lines`: `d` deletes them, `normal {keys}` types the
    /// keys on each; nothing prints. One undo step.
    fn global(&mut self, lines: Vec<usize>, argument: &str, inverse: bool, host: &mut impl Host) {
        let Some((pattern, command)) = split_pattern(argument) else {
            return self.fail();
        };
        let pattern = match self.pattern(&pattern) {
            Ok(pattern) => pattern,
            Err(_) => return self.fail(),
        };
        let text = Text(host.text());
        let matching = |line: usize| {
            let end = text.line_end(line);
            pattern.is_match(&text.0[line..end]) != inverse
        };
        let marked: Vec<usize> = lines.into_iter().filter(|line| matching(*line)).collect();
        if marked.is_empty() {
            return self.fail();
        }
        let (name, keys) = split_name(command.trim_start());
        let deletes = matches!(name, "d" | "de" | "del" | "dele" | "delet" | "delete");
        let normal = matches!(
            name,
            "norm" | "norma" | "normal" | "norm!" | "norma!" | "normal!"
        );
        if !((deletes && keys.is_empty()) || (normal && !keys.is_empty()) || name.is_empty()) {
            return self.fail();
        }
        if name.is_empty() {
            // Vim would print them: here the cursor goes to the last.
            if let Some(&last) = marked.last() {
                self.cursor = first_non_blank_or_end(Text(host.text()), last);
            }
            return;
        }
        self.joined = false;
        self.more.lines = marked;
        let keys: Vec<KeyPress> = if deletes {
            "dd".chars()
                .map(|c| KeyPress::plain(Key::Char(c)))
                .collect()
        } else {
            keys.chars()
                .map(|c| KeyPress::plain(Key::Char(c)))
                .collect()
        };
        let mut last = None;
        while !self.more.lines.is_empty() {
            let line = self.more.lines.remove(0);
            let text = Text(host.text());
            // A line the command before took away is not there any more:
            // its place is now the next line's, which may not match.
            if line > text.len() || text.line_start(line) != line {
                continue;
            }
            let end = text.line_end(line);
            if pattern.is_match(&text.0[line..end]) == inverse {
                continue;
            }
            self.cursor = clamp(text, line, false);
            self.play_keys(&keys, host);
            last = Some(self.cursor);
        }
        if let Some(last) = last {
            self.cursor = clamp(Text(host.text()), last, false);
        }
    }

    /// `:sort` of `lines`: by the text of each, `reverse` with `!`; with
    /// `i` ignoring case, `n` by the first number in each, `u` keeping one
    /// of lines that are alike.
    fn sort(&mut self, lines: &[usize], flags: &str, reverse: bool, host: &mut impl Host) {
        let (Some(&first), Some(&last)) = (lines.first(), lines.last()) else {
            return;
        };
        if flags.chars().any(|c| !matches!(c, 'i' | 'n' | 'u' | ' ')) {
            return self.fail();
        }
        let (any_case, numbers, unique) = (
            flags.contains('i'),
            flags.contains('n'),
            flags.contains('u'),
        );
        let text = Text(host.text());
        let end = text.line_end(last);
        let mut sorted: Vec<&str> = text.0[first..end].split('\n').collect();
        let key = |line: &str| -> (Option<i128>, String) {
            let number = numbers.then(|| first_number(line)).flatten();
            let line = if any_case {
                line.to_lowercase()
            } else {
                line.to_owned()
            };
            (number, if numbers { String::new() } else { line })
        };
        // Lines without a number come first, as in Vim.
        sorted.sort_by_key(|line| key(line));
        if reverse {
            sorted.reverse();
        }
        if unique {
            sorted.dedup_by(|a, b| key(a) == key(b));
        }
        let joined = sorted.join("\n");
        if joined != text.0[first..end] {
            self.joined = false;
            self.edit(host, first..end, &joined);
        }
        self.cursor = clamp(Text(host.text()), first, false);
        self.wanted_column = None;
    }

    /// `written` as a pattern of the command line, or why it is none.
    /// Nothing written: what the last search looked for.
    fn pattern(&self, written: &str) -> Result<Pattern, String> {
        if written.is_empty() {
            return self
                .last_search
                .as_ref()
                .ok_or_else(|| "no pattern".to_owned())
                .and_then(|search| Pattern::literal(&search.pattern, false));
        }
        Pattern::new(written)
    }
}

/// The first number in `line`, with its sign.
fn first_number(line: &str) -> Option<i128> {
    let start = line.find(|c: char| c.is_ascii_digit())?;
    let digits = &line[start..];
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    let value: i128 = digits[..end].parse().ok()?;
    let negative = line[..start].ends_with('-');
    Some(if negative { -value } else { value })
}

/// A number in the text, as Ctrl-A finds it.
struct Number {
    /// Its bytes, the sign and a `0x` or `0b` included.
    range: Range<usize>,
    value: i128,
    /// 10, 16 or 2.
    radix: u32,
    /// How many digits it is written with, and whether a hexadecimal one
    /// is written in capitals.
    digits: usize,
    capitals: bool,
}

impl Number {
    /// The number made `by` larger, written as this one is.
    fn add(&self, by: i128) -> String {
        let value = self.value.saturating_add(by);
        match self.radix {
            10 => {
                let width = if self.digits > 1 && self.leading_zero() {
                    self.digits
                } else {
                    0
                };
                let sign = if value < 0 { "-" } else { "" };
                format!("{sign}{:0width$}", value.unsigned_abs())
            }
            radix => {
                // Without a sign: as many bits as it has, around.
                let value = u64::try_from(value.rem_euclid(1 << 64)).unwrap_or(0);
                let digits = self.digits;
                let body = match (radix, self.capitals) {
                    (16, true) => format!("{value:0digits$X}"),
                    (16, false) => format!("{value:0digits$x}"),
                    _ => format!("{value:0digits$b}"),
                };
                format!("{}{body}", self.prefix())
            }
        }
    }

    fn leading_zero(&self) -> bool {
        self.radix == 10 && self.value != 0 && self.digits > digit_count(self.value)
            || self.value == 0 && self.digits > 1
    }

    fn prefix(&self) -> &'static str {
        match self.radix {
            16 => "0x",
            2 => "0b",
            _ => "",
        }
    }
}

/// How many decimal digits `value` is written with.
fn digit_count(value: i128) -> usize {
    value.unsigned_abs().to_string().len()
}

/// The number that Ctrl-A at byte `at` of `text` changes: the one under
/// the cursor, or else the next one in the line.
fn number_at(text: Text, at: usize) -> Option<Number> {
    let start = text.line_start(at);
    let end = text.line_end(at);
    let line = &text.0[start..end];
    let column = at - start;
    let bytes = line.as_bytes();
    // A hexadecimal or binary number the cursor is in, from its `0x`.
    let based = |from: usize| -> Option<Number> {
        let rest = line.get(from..)?;
        let lowered = rest.get(..2)?.to_ascii_lowercase();
        let (radix, valid): (u32, fn(char) -> bool) = match lowered.as_str() {
            "0x" => (16, |c| c.is_ascii_hexdigit()),
            "0b" => (2, |c| matches!(c, '0' | '1')),
            _ => return None,
        };
        let body = &rest[2..];
        let length = body.find(|c: char| !valid(c)).unwrap_or(body.len());
        if length == 0 {
            return None;
        }
        let digits = &body[..length];
        Some(Number {
            range: start + from..start + from + 2 + length,
            value: i128::from_str_radix(digits, radix).ok()?,
            radix,
            digits: length,
            capitals: digits.chars().any(|c| c.is_ascii_uppercase()),
        })
    };
    for from in (0..=column.min(line.len())).rev() {
        if let Some(number) = based(from).filter(|number| number.range.end > start + column) {
            return Some(number);
        }
        if !bytes.get(from).is_some_and(u8::is_ascii_alphanumeric) {
            break;
        }
    }
    // A decimal number: the one the cursor is on, else the next.
    let mut first = (column..line.len()).find(|index| bytes[*index].is_ascii_digit())?;
    while first > 0 && bytes[first - 1].is_ascii_digit() {
        first -= 1;
    }
    // From the next on: a `0x` there is a hexadecimal number.
    if first >= column
        && let Some(number) = based(first)
    {
        return Some(number);
    }
    let length = line[first..]
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(line.len() - first);
    let value: i128 = line[first..first + length].parse().ok()?;
    let negative = first > 0 && bytes[first - 1] == b'-';
    let from = if negative { first - 1 } else { first };
    Some(Number {
        range: start + from..start + first + length,
        value: if negative { -value } else { value },
        radix: 10,
        digits: length,
        capitals: false,
    })
}

/// The word (or with `big` the WORD) at or behind byte `at` in its line.
fn word_at(text: Text, at: usize, big: bool) -> Option<Range<usize>> {
    let start = text.line_start(at);
    let end = text.line_end(at);
    let line = &text.0[start..end];
    let class = |c: char| crate::text::class(c, big);
    let column = at - start;
    let from = line[column..]
        .char_indices()
        .find(|(_, c)| class(*c) != Class::Blank)
        .map(|(offset, _)| column + offset)?;
    let kind = class(line[from..].chars().next()?);
    let before = line[..from]
        .char_indices()
        .rev()
        .take_while(|(_, c)| class(*c) == kind)
        .last()
        .map_or(from, |(offset, _)| offset);
    let after = line[from..]
        .char_indices()
        .find(|(_, c)| class(*c) != kind)
        .map_or(line.len(), |(offset, _)| from + offset);
    Some(start + before..start + after)
}

/// The lines a command of the command line is for, as written before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineRange {
    /// None written: the cursor's line (or for some, all).
    Unwritten,
    /// `%`.
    All,
    /// `.`
    Cursor,
    /// From and to, counted from 1.
    Counted(usize, usize),
}

impl LineRange {
    /// The range written at the start of `line`, and the rest.
    fn read(line: &str) -> (Self, &str) {
        if let Some(rest) = line.strip_prefix('%') {
            return (Self::All, rest);
        }
        if let Some(rest) = line.strip_prefix('.') {
            return (Self::Cursor, rest);
        }
        let numbers = line.len()
            - line
                .trim_start_matches(|c: char| c.is_ascii_digit() || c == ',')
                .len();
        let (written, rest) = line.split_at(numbers);
        if written.is_empty() {
            return (Self::Unwritten, line);
        }
        let (from, to) = written.split_once(',').unwrap_or((written, written));
        match (from.parse::<usize>(), to.parse::<usize>()) {
            (Ok(from), Ok(to)) => (Self::Counted(from.min(to).max(1), from.max(to)), rest),
            _ => (Self::Unwritten, line),
        }
    }

    /// All of the text where none was written.
    fn or_all(self) -> Self {
        if self == Self::Unwritten {
            Self::All
        } else {
            self
        }
    }

    /// The starts of the lines of `text`, with the cursor at `cursor`.
    fn lines(self, text: Text, cursor: usize) -> Vec<usize> {
        let count = text.line_count();
        let (from, to) = match self {
            Self::Unwritten | Self::Cursor => {
                let line = text.line_index(cursor);
                (line, line)
            }
            Self::All => (0, count.saturating_sub(1)),
            Self::Counted(from, to) => (from - 1, (to - 1).min(count.saturating_sub(1))),
        };
        (from..=to).map(|line| text.line(line)).collect()
    }
}

/// The name of a command and what follows it (`g/x/d` is `g` and `/x/d`,
/// `normal Ax` is `normal` and `Ax`, `sort! n` is `sort!` and `n`).
fn split_name(line: &str) -> (&str, &str) {
    let line = line.trim_start();
    let name = line
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(line.len());
    let name = if line[name..].starts_with('!') {
        name + 1
    } else {
        name
    };
    let (name, rest) = line.split_at(name);
    let rest = rest.strip_prefix(' ').unwrap_or(rest);
    (name, rest)
}

/// `/pattern/command` read: the pattern (a backslash lets the parting sign
/// stand for itself) and the command.
fn split_pattern(argument: &str) -> Option<(String, &str)> {
    let mut chars = argument.char_indices();
    let (_, parting) = chars.next()?;
    if parting.is_alphanumeric() || parting.is_whitespace() || parting == '\\' {
        return None;
    }
    let mut pattern = String::new();
    let mut escaped = false;
    for (index, c) in chars {
        match c {
            _ if escaped => {
                if c != parting {
                    pattern.push('\\');
                }
                pattern.push(c);
                escaped = false;
            }
            '\\' => escaped = true,
            _ if c == parting => return Some((pattern, &argument[index + c.len_utf8()..])),
            _ => pattern.push(c),
        }
    }
    Some((pattern, ""))
}
