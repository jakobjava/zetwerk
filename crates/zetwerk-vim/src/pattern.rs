//! Patterns of the command line (`:s`, `:g`): Vim's regular expressions,
//! as far as they are written alike in Rust's `regex`, and what replaces
//! a match (`\1`, `&`).
//!
//! What is read: Vim's 'magic' patterns, where `.`, `*`, `[…]`, `^` and
//! `$` are special and `\(…\)`, `\|`, `\+`, `\?`, `\=`, `\{n,m}`, `\<`,
//! `\>`, `\d`, `\w`, `\s`, `\a`, `\l`, `\u`, `\x` are; `\v` before a
//! pattern makes it 'very magic' (`(…)`, `|`, `+`, `?`, `{n,m}`, `<`, `>`
//! special without a backslash); `\c` and `\C` in it ignore case or not.
//! What is not (`\zs`, `\ze`, `\%[`, `\@`, `~`) is refused with a word, not
//! guessed at. Searches with `/` are not patterns: they look for what is
//! read on the page, as it is written (see [`crate::find`]).

use regex::{Captures, Regex, RegexBuilder};

/// A pattern of the command line, made ready to find with.
#[derive(Debug, Clone)]
pub struct Pattern {
    regex: Regex,
}

impl Pattern {
    /// Reads `written` as Vim reads a pattern; `Err` with why it cannot be
    /// read.
    pub fn new(written: &str) -> Result<Self, String> {
        Self::with_case(written, false)
    }

    /// Like [`Pattern::new`]; with `any_case` (the `i` flag of `:s`) case
    /// is ignored unless the pattern says `\C`.
    pub fn with_case(written: &str, any_case: bool) -> Result<Self, String> {
        let (translated, case) = translate(written)?;
        let regex = RegexBuilder::new(&translated)
            .case_insensitive(case.unwrap_or(any_case))
            .multi_line(true)
            .build()
            .map_err(|err| {
                format!(
                    "not a pattern: {written} ({})",
                    first_line(&err.to_string())
                )
            })?;
        Ok(Self { regex })
    }

    /// A pattern that finds `text` as it is written; `Err` where it is
    /// too long to be one.
    pub fn literal(text: &str, any_case: bool) -> Result<Self, String> {
        let regex = RegexBuilder::new(&regex::escape(text))
            .case_insensitive(any_case)
            .build()
            .map_err(|err| format!("not a pattern: {}", first_line(&err.to_string())))?;
        Ok(Self { regex })
    }

    /// Whether it is found in `text`.
    #[must_use]
    pub fn is_match(&self, text: &str) -> bool {
        self.regex.is_match(text)
    }

    /// The matches in `text`, in order and apart; each with its groups,
    /// for [`Replacement::expand`].
    pub fn captures<'t>(&self, text: &'t str) -> impl Iterator<Item = Captures<'t>> + use<'_, 't> {
        self.regex.captures_iter(text)
    }
}

/// What replaces a match of `:s`, as Vim writes it: `\1` to `\9` and `\0`
/// or `&` for what was matched, `\r` and `\n` for a line break, `\t` for
/// a tab, `\&` and `\\` for themselves.
#[derive(Debug, Clone)]
pub struct Replacement {
    template: String,
}

impl Replacement {
    /// Reads `written`; `Err` with why it cannot be.
    pub fn new(written: &str) -> Result<Self, String> {
        let mut template = String::new();
        let mut chars = written.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => match chars.next() {
                    Some(digit @ '0'..='9') => {
                        template.push_str("${");
                        template.push(digit);
                        template.push('}');
                    }
                    Some('r' | 'n') => template.push('\n'),
                    Some('t') => template.push('\t'),
                    Some(case @ ('u' | 'U' | 'l' | 'L' | 'e' | 'E')) => {
                        return Err(format!("\\{case} in what replaces is not done here"));
                    }
                    Some('$') => template.push_str("$$"),
                    Some(other) => template.push(other),
                    None => template.push('\\'),
                },
                '&' => template.push_str("${0}"),
                '~' => return Err("~ in what replaces is not done here: write it out".to_owned()),
                '$' => template.push_str("$$"),
                c => template.push(c),
            }
        }
        Ok(Self { template })
    }

    /// The text that replaces the match `captures`.
    #[must_use]
    pub fn expand(&self, captures: &Captures) -> String {
        let mut expanded = String::new();
        captures.expand(&self.template, &mut expanded);
        expanded
    }
}

/// The first line of `text`: `regex` draws where it went wrong below.
fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|line| line.starts_with("error"))
        .unwrap_or_else(|| text.lines().next().unwrap_or(text))
}

/// `written` as Rust's regular expression, and whether `\c` (`Some(true)`)
/// or `\C` asked for case to be ignored or not.
#[allow(
    clippy::too_many_lines,
    reason = "one arm for each mark of Vim's patterns"
)]
fn translate(written: &str) -> Result<(String, Option<bool>), String> {
    let mut out = String::new();
    let mut case = None;
    let (very, written) = match written.strip_prefix("\\v") {
        Some(rest) => (true, rest),
        None => (false, written.strip_prefix("\\m").unwrap_or(written)),
    };
    let mut chars = written.chars().peekable();
    // Whether what comes is the first of the pattern or of a branch, where
    // Vim takes `*` as itself.
    let mut first = true;
    while let Some(c) = chars.next() {
        let was_first = first;
        first = false;
        match c {
            '\\' => {
                let Some(next) = chars.next() else {
                    out.push_str("\\\\");
                    break;
                };
                match next {
                    'c' => case = Some(true),
                    'C' => case = Some(false),
                    '(' if !very => out.push('('),
                    ')' if !very => out.push(')'),
                    '|' if !very => {
                        out.push('|');
                        first = true;
                    }
                    '+' if !very => out.push('+'),
                    '?' | '=' if !very => out.push('?'),
                    '{' if !very => out.push_str(&braces(&mut chars)?),
                    '<' | '>' if !very => out.push_str("\\b"),
                    '%' if chars.peek() == Some(&'(') => {
                        chars.next();
                        out.push_str("(?:");
                    }
                    'd' | 'D' | 'w' | 'W' | 's' | 'S' => {
                        out.push('\\');
                        out.push(next);
                    }
                    'n' => out.push_str("\\n"),
                    't' => out.push_str("\\t"),
                    'a' => out.push_str("[A-Za-z]"),
                    'A' => out.push_str("[^A-Za-z]"),
                    'l' => out.push_str("[a-z]"),
                    'L' => out.push_str("[^a-z]"),
                    'u' => out.push_str("[A-Z]"),
                    'U' => out.push_str("[^A-Z]"),
                    'x' => out.push_str("[0-9A-Fa-f]"),
                    'X' => out.push_str("[^0-9A-Fa-f]"),
                    'z' | '@' | '%' => {
                        return Err(format!("\\{next} in a pattern is not done here"));
                    }
                    other => out.push_str(&regex::escape(&other.to_string())),
                }
            }
            '[' => out.push_str(&class(&mut chars)?),
            '*' if was_first => out.push_str("\\*"),
            '.' | '*' | '^' | '$' => out.push(c),
            '~' => return Err("~ in a pattern is not done here".to_owned()),
            '(' | ')' | '+' | '?' | '{' | '}' | '|' if very => {
                out.push(c);
                first = c == '(' || c == '|';
            }
            '=' if very => out.push('?'),
            '<' | '>' if very => out.push_str("\\b"),
            c => out.push_str(&regex::escape(&c.to_string())),
        }
    }
    Ok((out, case))
}

/// The rest of `\{n,m}` after `\{`, as Rust writes it: `{n,m}`, and
/// `\{-n,m}` as the least that fits, `{n,m}?`.
fn braces(chars: &mut std::iter::Peekable<std::str::Chars>) -> Result<String, String> {
    let mut inside = String::new();
    loop {
        match chars.next() {
            Some('}') => break,
            Some('\\') if chars.peek() == Some(&'}') => {}
            Some(c) if c.is_ascii_digit() || c == ',' || c == '-' => inside.push(c),
            _ => return Err("\\{ without its }".to_owned()),
        }
    }
    let lazy = inside.starts_with('-');
    let counts = inside.trim_start_matches('-');
    let counts = if counts.is_empty() { "0," } else { counts };
    let counts = if counts.starts_with(',') {
        format!("0{counts}")
    } else {
        counts.to_owned()
    };
    Ok(format!("{{{counts}}}{}", if lazy { "?" } else { "" }))
}

/// The rest of a class `[…]` after its `[`, as Rust writes it.
fn class(chars: &mut std::iter::Peekable<std::str::Chars>) -> Result<String, String> {
    let mut out = String::from("[");
    if chars.peek() == Some(&'^') {
        chars.next();
        out.push('^');
    }
    // A `]` first is itself.
    if chars.peek() == Some(&']') {
        chars.next();
        out.push_str("\\]");
    }
    loop {
        match chars.next() {
            Some(']') => {
                out.push(']');
                return Ok(out);
            }
            Some('\\') => match chars.next() {
                Some(c @ ('d' | 'w' | 's' | 'n' | 't')) => {
                    out.push('\\');
                    out.push(c);
                }
                Some(c) => out.push_str(&regex::escape(&c.to_string())),
                None => return Err("[ without its ]".to_owned()),
            },
            Some(c @ ('[' | '&' | '~')) => {
                out.push('\\');
                out.push(c);
            }
            Some(c) => out.push(c),
            None => return Err("[ without its ]".to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replaced(pattern: &str, with: &str, text: &str) -> String {
        let pattern = Pattern::new(pattern).unwrap_or_else(|why| panic!("{why}"));
        let with = Replacement::new(with).unwrap_or_else(|why| panic!("{why}"));
        let mut out = String::new();
        let mut last = 0;
        for captures in pattern.captures(text) {
            let Some(whole) = captures.get(0) else {
                continue;
            };
            out.push_str(&text[last..whole.start()]);
            out.push_str(&with.expand(&captures));
            last = whole.end();
        }
        out.push_str(&text[last..]);
        out
    }

    #[test]
    fn vim_patterns_are_read_as_vim_reads_them() {
        assert_eq!(replaced("a.c", "X", "abc a.c"), "X X");
        assert_eq!(replaced("a\\.c", "X", "abc a.c"), "abc X");
        assert_eq!(
            replaced("\\(\\w\\+\\) \\(\\w\\+\\)", "\\2 \\1", "one two"),
            "two one"
        );
        assert_eq!(
            replaced("\\v(\\w+) (\\w+)", "\\2 \\1", "one two"),
            "two one"
        );
        assert_eq!(replaced("(x)", "[&]", "f(x)"), "f[(x)]");
        assert_eq!(replaced("\\<is\\>", "IS", "this is"), "this IS");
        assert_eq!(replaced("a\\{2}", "X", "aaa"), "Xa");
        assert_eq!(replaced("a\\{-1,}", "X", "aaa"), "XXX");
        assert_eq!(replaced("x\\|y", "-", "xzy"), "-z-");
        assert_eq!(replaced("[0-9]\\+", "#", "a12b3"), "a#b#");
        assert_eq!(replaced("^", "> ", "a\nb"), "> a\n> b");
        assert_eq!(replaced("\\cHELLO", "hi", "Hello"), "hi");
        assert_eq!(replaced("*", "x", "a*"), "ax");
        assert_eq!(replaced("$", "\\$", "a"), "a$");
        assert_eq!(replaced(" ", "\\r", "a b"), "a\nb");
        assert!(Pattern::new("a\\zsb").is_err());
        assert!(Pattern::new("[ab").is_err());
        assert!(Replacement::new("\\u&").is_err());
    }
}
