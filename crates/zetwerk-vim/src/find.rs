//! Searching for what is read.
//!
//! Where the host shows the text as typeset pages, what the user sees is
//! what the text is read as, not what it is written with: a word set in
//! bold has no asterisks in it, `alpha` in an equation is α, a line break
//! of the text is a blank between two words. A search there goes over a
//! [`Reading`] of the text, which the host makes (see
//! [`crate::Host::reading`]), and what it finds are the bytes of the text
//! that show it.
//!
//! A pattern is text to be found as it is typed, as it always was for
//! the engine: there are no regular expressions. Vim's `\c` (anywhere in
//! the pattern) has case ignored and its `\C` has it count. Not Vim's: a
//! pattern that begins with `\=` is looked for in the text as written,
//! exactly, also on a page.
//!
//! In what is read, a pattern without capitals finds words with them too
//! (Vim's 'smartcase'), a blank finds any blank between words, and
//! straight quotation marks find the curly ones the page shows. In the
//! text as written, case counts unless `\c` says otherwise, as it does
//! in Vim by default.

use std::ops::Range;

use crate::text::{Class, class};

/// How many matches are looked for at most: enough for any search a user
/// steps through, few enough that a search for one letter in a long text
/// is soon over.
pub const MOST: usize = 100_000;

/// What a text is read as, and where each part of that is written in it.
#[derive(Debug, Clone, Default)]
pub struct Reading {
    text: String,
    /// The parts, in the order of the reading and of the text alike.
    pieces: Vec<Piece>,
}

/// A part of a [`Reading`].
#[derive(Debug, Clone)]
struct Piece {
    /// Where it begins in the reading.
    read: usize,
    /// The bytes of the text it is read from.
    from: Range<usize>,
    /// Whether it is those bytes themselves, byte for byte; otherwise all
    /// of it stands for all of them.
    verbatim: bool,
}

impl Reading {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `written`, which begins at byte `start` of the text, as read
    /// the way it is written.
    pub fn push_verbatim(&mut self, written: &str, start: usize) {
        if written.is_empty() {
            return;
        }
        let read = self.text.len();
        self.text.push_str(written);
        let end = start + written.len();
        if let Some(last) = self.pieces.last_mut()
            && last.verbatim
            && last.from.end == start
            && last.read + last.from.len() == read
        {
            last.from.end = end;
            return;
        }
        self.pieces.push(Piece {
            read,
            from: start..end,
            verbatim: true,
        });
    }

    /// Adds `read` as what bytes `from` of the text are read as, where
    /// that is not what they are written with: a symbol written by its
    /// name, a blank for a line break.
    pub fn push(&mut self, read: &str, from: Range<usize>) {
        if read.is_empty() {
            return;
        }
        self.pieces.push(Piece {
            read: self.text.len(),
            from,
            verbatim: false,
        });
        self.text.push_str(read);
    }

    /// All that is read.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The bytes of the text that bytes `range` of the reading are read
    /// from.
    #[must_use]
    pub fn written(&self, range: Range<usize>) -> Range<usize> {
        let piece = |at: usize| {
            let index = self.pieces.partition_point(|piece| piece.read <= at);
            self.pieces.get(index.checked_sub(1)?)
        };
        let Some(first) = piece(range.start) else {
            return 0..0;
        };
        let start = if first.verbatim {
            first.from.start + (range.start - first.read)
        } else {
            first.from.start
        };
        if range.is_empty() {
            return start..start;
        }
        let end = piece(range.end - 1).map_or(start, |last| {
            if last.verbatim {
                last.from.start + (range.end - last.read)
            } else {
                last.from.end
            }
        });
        start..end.max(start)
    }

    /// The first piece read from byte `at` of the text or after it, and
    /// where in the reading it begins.
    fn from(&self, at: usize) -> (usize, usize) {
        let index = self.pieces.partition_point(|piece| piece.from.start < at);
        let read = self
            .pieces
            .get(index)
            .map_or(self.text.len(), |piece| piece.read);
        (index, read)
    }

    /// This reading, with what bytes `start..old_end` of the text were
    /// read as taken out and `between` put in, what bytes
    /// `start..new_end` of the text are now read as; what is read of the
    /// bytes behind is moved along. The bytes are to be read alike in
    /// the old text and the new from `start` on and from `old_end` and
    /// `new_end` on, and `between` to be read by itself as all that is
    /// read before it would read it: from paragraph break to paragraph
    /// break (see `zetwerk_app::reading`).
    #[must_use]
    pub fn splice(&self, start: usize, old_end: usize, new_end: usize, between: &Reading) -> Self {
        let (first, read_from) = self.from(start);
        let (behind, read_behind) = self.from(old_end);
        let mut text = String::with_capacity(self.text.len() + between.text.len());
        text.push_str(&self.text[..read_from]);
        text.push_str(&between.text);
        let moved = text.len();
        text.push_str(&self.text[read_behind..]);
        let mut pieces = Vec::with_capacity(self.pieces.len() + between.pieces.len());
        pieces.extend_from_slice(&self.pieces[..first]);
        pieces.extend(between.pieces.iter().map(|piece| Piece {
            read: piece.read + read_from,
            ..piece.clone()
        }));
        pieces.extend(self.pieces[behind..].iter().map(|piece| Piece {
            read: piece.read - read_behind + moved,
            from: piece.from.start - old_end + new_end..piece.from.end - old_end + new_end,
            verbatim: piece.verbatim,
        }));
        Self { text, pieces }
    }

    /// Where in the reading byte `at` of the text is read, or what is
    /// read next after it if it is not read at all (markup).
    #[must_use]
    pub fn read_at(&self, at: usize) -> usize {
        let index = self.pieces.partition_point(|piece| piece.from.end <= at);
        match self.pieces.get(index) {
            Some(piece) if piece.verbatim && piece.from.start <= at => {
                piece.read + (at - piece.from.start)
            }
            Some(piece) => piece.read,
            None => self.text.len(),
        }
    }
}

/// What to look for: the pattern as typed, and whether a match may not
/// continue a word before it and may not be continued by one after it
/// (`*` and `#`: neither).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub pattern: String,
    pub word_start: bool,
    pub word_end: bool,
}

/// A pattern taken apart: the text to look for, and how.
struct Wanted {
    needle: String,
    ignore_case: bool,
    /// Whether to look in the text as written, though there is a reading.
    as_written: bool,
}

/// Takes the flags out of `pattern`. `reading` says whether it is looked
/// for in what is read, where case counts only if the pattern has a
/// capital.
fn wanted(pattern: &str, reading: bool) -> Wanted {
    let (as_written, rest) = match pattern.strip_prefix("\\=") {
        Some(rest) => (true, rest),
        None => (false, pattern),
    };
    let mut needle = String::with_capacity(rest.len());
    let mut case = None;
    let mut chars = rest.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, chars.peek()) {
            ('\\', Some('c')) => {
                case = Some(false);
                chars.next();
            }
            ('\\', Some('C')) => {
                case = Some(true);
                chars.next();
            }
            _ => needle.push(c),
        }
    }
    let smart = reading && !as_written && !needle.chars().any(char::is_uppercase);
    Wanted {
        ignore_case: case.map_or(smart, |counts| !counts),
        needle,
        as_written,
    }
}

/// Every match of `query` in `text`, as bytes of it, in the order of the
/// text; at most [`MOST`]. With a `reading`, what is looked through is
/// that, unless the pattern asks for the text as written.
#[must_use]
pub fn find_all(text: &str, reading: Option<&Reading>, query: &Query) -> Vec<Range<usize>> {
    let wanted = wanted(&query.pattern, reading.is_some());
    if wanted.needle.is_empty() {
        return Vec::new();
    }
    let whole = (query.word_start, query.word_end);
    match reading.filter(|_| !wanted.as_written) {
        Some(reading) => {
            let mut needle = String::with_capacity(wanted.needle.len());
            // What is read has one blank between words.
            for c in wanted.needle.chars() {
                if !(c == ' ' && needle.ends_with(' ')) {
                    needle.push(c);
                }
            }
            let found = occurrences(reading.text(), &needle, wanted.ignore_case, true, whole);
            found
                .into_iter()
                .map(|range| reading.written(range))
                .collect()
        }
        None => occurrences(text, &wanted.needle, wanted.ignore_case, false, whole),
    }
}

/// The matches [`find_all`] finds that begin in bytes `within` of
/// `text`, with the bytes they were looked for in, which hold `within`.
/// With a `reading`, `within` is to begin after a paragraph break of the
/// text and end where one begins (or at the start and end of the text):
/// no match reaches across one. `None` where that cannot be told: a
/// pattern with a line break in it, or `within` not between breaks.
#[must_use]
pub fn find_within(
    text: &str,
    reading: Option<&Reading>,
    query: &Query,
    within: Range<usize>,
) -> Option<(Range<usize>, Vec<Range<usize>>)> {
    let wanted = wanted(&query.pattern, reading.is_some());
    if wanted.needle.contains('\n') {
        return None;
    }
    if wanted.needle.is_empty() {
        return Some((within, Vec::new()));
    }
    let whole = (query.word_start, query.word_end);
    if let Some(reading) = reading.filter(|_| !wanted.as_written) {
        let ((_, from), (_, to)) = (reading.from(within.start), reading.from(within.end));
        let read = reading.text();
        // A paragraph break is read as a line break: none on the edges
        // of `within`, none in the pattern, no match across.
        let broken = |at: usize| at == 0 || at >= read.len() || read[..at].ends_with('\n');
        if !broken(from) || !(to >= read.len() || read[to..].starts_with('\n')) {
            return None;
        }
        let mut needle = String::with_capacity(wanted.needle.len());
        for c in wanted.needle.chars() {
            if !(c == ' ' && needle.ends_with(' ')) {
                needle.push(c);
            }
        }
        let found = occurrences(&read[from..to], &needle, wanted.ignore_case, true, whole);
        let found = found
            .into_iter()
            .map(|range| reading.written(range.start + from..range.end + from))
            .collect();
        return Some((within, found));
    }
    // Lines: no match reaches across their ends.
    let start = text[..within.start].rfind('\n').map_or(0, |at| at + 1);
    let end = text[within.end..]
        .find('\n')
        .map_or(text.len(), |at| within.end + at);
    let found = occurrences(
        &text[start..end],
        &wanted.needle,
        wanted.ignore_case,
        false,
        whole,
    );
    let found = found
        .into_iter()
        .map(|range| range.start + start..range.end + start)
        .collect();
    Some((start..end, found))
}

/// Whether `c` belongs to a word.
fn is_word(c: char) -> bool {
    class(c, false) == Class::Word
}

/// Where `needle` is in `hay`. Matches may overlap: each is looked for
/// one character behind where the one before began. With `fold`, case is
/// ignored; with `loose`, a blank finds any blank but a paragraph's end,
/// and straight quotation marks the curly ones.
fn occurrences(
    hay: &str,
    needle: &str,
    fold: bool,
    loose: bool,
    (word_start, word_end): (bool, bool),
) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let Some(first) = needle.chars().next() else {
        return found;
    };
    let exact = !fold && !loose;
    let mut at = 0;
    while found.len() < MOST {
        let start = if exact {
            match hay.get(at..).and_then(|rest| rest.find(needle)) {
                Some(offset) => at + offset,
                None => break,
            }
        } else {
            let next = hay.get(at..).and_then(|rest| {
                rest.char_indices()
                    .find(|&(_, c)| same(first, c, fold, loose))
                    .map(|(offset, _)| at + offset)
            });
            match next {
                Some(start) => start,
                None => break,
            }
        };
        let end = if exact {
            Some(start + needle.len())
        } else {
            match_at(hay, start, needle, fold, loose)
        };
        if let Some(end) = end {
            let continues_word =
                word_start && hay[..start].chars().next_back().is_some_and(is_word);
            let goes_on = word_end && hay[end..].chars().next().is_some_and(is_word);
            if !continues_word && !goes_on {
                found.push(start..end);
            }
        }
        at = start + hay[start..].chars().next().map_or(1, char::len_utf8);
    }
    found
}

/// Where a match of `needle` that begins at byte `start` of `hay` ends,
/// if one does.
fn match_at(hay: &str, start: usize, needle: &str, fold: bool, loose: bool) -> Option<usize> {
    let mut read = hay.get(start..)?.char_indices();
    let mut end = start;
    for wanted in needle.chars() {
        let (offset, c) = read.next()?;
        if !same(wanted, c, fold, loose) {
            return None;
        }
        end = start + offset + c.len_utf8();
    }
    Some(end)
}

/// Whether `found` in the text is a match for `wanted` in the pattern.
fn same(wanted: char, found: char, fold: bool, loose: bool) -> bool {
    if wanted == found {
        return true;
    }
    if loose {
        let matched = match wanted {
            ' ' => found != '\n' && found.is_whitespace(),
            '"' => matches!(
                found,
                '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{ab}' | '\u{bb}'
            ),
            '\'' => matches!(found, '\u{2018}' | '\u{2019}' | '\u{201a}'),
            _ => false,
        };
        if matched {
            return true;
        }
    }
    fold && lower(wanted) == lower(found)
}

/// `c` in lower case, as far as that is one character.
fn lower(c: char) -> char {
    // Most of a text is ASCII, and this is asked of every character of
    // it while matches are shown: without the general tables.
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(one), None) => one,
        _ => c,
    }
}

/// What `*` and `#` look for where the cursor is at byte `at` of the
/// text, as read: the word there, or the next one in the paragraph;
/// failing that, what is there up to the next blank. Returns it and the
/// byte of the text it begins at.
#[must_use]
pub fn word_read(reading: &Reading, at: usize) -> Option<(String, usize)> {
    let text = reading.text();
    let from = reading.read_at(at).min(text.len());
    let line_end = text[from..].find('\n').map_or(text.len(), |end| from + end);
    let line_start = text[..from].rfind('\n').map_or(0, |start| start + 1);
    let rest = &text[from..line_end];
    let word = rest.find(is_word);
    let offset = word.or_else(|| rest.find(|c: char| !c.is_whitespace()))?;
    let run_of = |c: char| {
        if word.is_some() {
            is_word(c)
        } else {
            !c.is_whitespace()
        }
    };
    let inside = from + offset;
    let before = &text[line_start..inside];
    let start = line_start + before.trim_end_matches(run_of).len();
    let tail = &text[start..line_end];
    let length = tail.len() - tail.trim_start_matches(run_of).len();
    let found = &text[start..start + length];
    Some((
        found.to_owned(),
        reading.written(start..start + length).start,
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use super::*;

    /// "One *two* $alpha$." as read: "One two α."
    fn reading() -> (String, Reading) {
        let text = "One *two*\n$alpha$.".to_owned();
        let mut reading = Reading::new();
        reading.push_verbatim("One", 0);
        reading.push(" ", 3..4);
        reading.push_verbatim("two", 5);
        reading.push(" ", 9..10);
        reading.push("α", 11..16);
        reading.push_verbatim(".", 17);
        (text, reading)
    }

    fn query(pattern: &str) -> Query {
        Query {
            pattern: pattern.to_owned(),
            word_start: false,
            word_end: false,
        }
    }

    #[test]
    fn what_is_read_maps_to_what_is_written() {
        let (text, reading) = reading();
        assert_eq!(reading.text(), "One two α.");
        // "two" is where it is written, inside its asterisks.
        let two = reading.text().find("two").unwrap();
        assert_eq!(&text[reading.written(two..two + 3)], "two");
        assert_eq!(&text[reading.written(two + 1..two + 2)], "w");
        // A symbol is all of its name.
        let alpha = reading.text().find('α').unwrap();
        assert_eq!(&text[reading.written(alpha..alpha + 2)], "alpha");
        // Markup is read where what follows it is.
        assert_eq!(reading.read_at(4), two);
        assert_eq!(reading.read_at(6), two + 1);
        assert_eq!(reading.read_at(text.len()), reading.text().len());
    }

    #[test]
    fn a_search_finds_what_is_read() {
        let (text, reading) = reading();
        let found = |pattern: &str| -> Vec<&str> {
            find_all(&text, Some(&reading), &query(pattern))
                .into_iter()
                .map(|range| &text[range])
                .collect()
        };
        // Across markup and a line break of the text.
        assert_eq!(found("one two"), ["One *two"]);
        assert_eq!(found("e  t"), ["e *t"]);
        assert_eq!(found("α"), ["alpha"]);
        assert_eq!(found("alpha").len(), 0);
        // Smart case; `\C` and `\c`.
        assert_eq!(found("ONE").len(), 0);
        assert_eq!(found("one"), ["One"]);
        assert_eq!(found("one\\C").len(), 0);
        assert_eq!(found("\\cTWO"), ["two"]);
        // As written, exactly.
        assert_eq!(found("\\=*two*"), ["*two*"]);
        assert_eq!(found("\\=one").len(), 0);
    }

    #[test]
    fn as_written_case_counts_unless_asked() {
        let text = "Foo foo";
        let all = |pattern: &str| -> Vec<&str> {
            find_all(text, None, &query(pattern))
                .into_iter()
                .map(|range| &text[range])
                .collect()
        };
        assert_eq!(all("foo"), ["foo"]);
        assert_eq!(all("\\cfoo"), ["Foo", "foo"]);
        assert_eq!(all("Fo\\Co"), ["Foo"]);
    }

    #[test]
    fn quotation_marks_and_blanks_are_found_by_the_plain_ones() {
        let text = "a\u{201c}b\u{201d}\u{a0}c";
        let mut reading = Reading::new();
        reading.push_verbatim(text, 0);
        let found = find_all(text, Some(&reading), &query("\"b\" c"));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0], 1..text.len());
    }

    #[test]
    fn the_word_under_the_cursor_is_the_word_as_read() {
        let (text, reading) = reading();
        // On the asterisk before "two": the word behind it.
        assert_eq!(word_read(&reading, 4), Some(("two".to_owned(), 5)));
        assert_eq!(word_read(&reading, 7), Some(("two".to_owned(), 5)));
        assert_eq!(word_read(&reading, 12), Some(("α".to_owned(), 11)));
        assert_eq!(word_read(&reading, text.len()), None);
    }
}
