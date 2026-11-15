//! Entries of a BibTeX file that cannot be read: a brace left open, a
//! key left out. They are listed as such (not left out of the list
//! unsaid), to be closed, written anew or taken out; nothing else of the
//! file is touched.

use std::ops::Range;

/// Whether the text of an entry runs on into another: a line of it
/// begins with `@`, as an entry does that a brace left open swallowed.
#[must_use]
pub fn runs_on(entry: &str) -> bool {
    entry
        .lines()
        .skip(1)
        .any(|line| line.trim_start().starts_with('@'))
}

/// The entries of `text` that cannot be read: each begins with `@` at
/// the start of a line, outside every entry that can be read, and runs
/// to the next line that begins so, or to the end.
#[must_use]
pub fn unreadable(text: &str) -> Vec<Range<usize>> {
    let read: Vec<Range<usize>> = super::parse(text)
        .into_iter()
        .map(|entry| entry.range)
        .collect();
    let mut starts = Vec::new();
    let mut line = 0;
    for part in text.split_inclusive('\n') {
        let blank = part.len() - part.trim_start().len();
        if part.trim_start().starts_with('@') {
            starts.push(line + blank);
        }
        line += part.len();
    }
    let mut found = Vec::new();
    for (at, start) in starts.iter().enumerate() {
        if read.iter().any(|range| range.contains(start)) {
            continue;
        }
        let head = &text[start + 1..];
        let kind = &head[..head
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(head.len())];
        let kind = kind.to_lowercase();
        if kind.is_empty() || matches!(kind.as_str(), "comment" | "string" | "preamble") {
            continue;
        }
        let end = starts.get(at + 1).map_or(text.len(), |next| {
            // Back to the end of the line before the next one.
            text[..*next].rfind('\n').map_or(*next, |line| line + 1)
        });
        let end = start + text[*start..end].trim_end().len();
        found.push(*start..end);
    }
    found
}

/// How many closing braces `entry` lacks.
#[must_use]
pub fn unclosed(entry: &str) -> usize {
    let mut depth = 0_usize;
    let mut escaped = false;
    for c in entry.chars() {
        match c {
            '\\' => {
                escaped = !escaped;
                continue;
            }
            '{' if !escaped => depth += 1,
            '}' if !escaped => depth = depth.saturating_sub(1),
            _ => {}
        }
        escaped = false;
    }
    depth
}

/// The first words of `entry`, on one line, as it is listed.
#[must_use]
pub fn first_words(entry: &str) -> String {
    const LONGEST: usize = 50;
    let words: Vec<&str> = entry.split_whitespace().collect();
    let line = words.join(" ");
    let mut shown: String = line.chars().take(LONGEST).collect();
    if line.chars().count() > LONGEST {
        shown.push('…');
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_left_open_is_found_and_the_next_is_read() {
        let text = "@book{a,\n  title = {Open,\n}\n\n@book{b, title = {Fine}}\n\
                    @misc{title = {no key}}\n@comment{whatever\n";
        let read: Vec<String> = super::super::parse(text)
            .into_iter()
            .map(|entry| entry.key)
            .collect();
        assert_eq!(read, ["b"]);
        let broken = unreadable(text);
        let shown: Vec<&str> = broken.iter().map(|range| &text[range.clone()]).collect();
        assert_eq!(
            shown,
            ["@book{a,\n  title = {Open,\n}", "@misc{title = {no key}}"]
        );
        assert_eq!(unclosed(shown[0]), 1);
        assert_eq!(unclosed(shown[1]), 0);
        assert_eq!(first_words(shown[0]), "@book{a, title = {Open, }");
    }
}
