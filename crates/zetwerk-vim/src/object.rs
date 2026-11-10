//! Text objects: the things `iw`, `ap`, `i(` and `a"` select.

use crate::text::{Class, Text, class};

/// A selected stretch of text, from `start` up to but not including `end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Object {
    pub start: usize,
    pub end: usize,
    pub linewise: bool,
}

fn class_at(text: Text, pos: usize, big: bool) -> Class {
    text.char_at(pos)
        .map_or(Class::LineBreak, |c| class(c, big))
}

/// The end of the run of one class starting at `pos`, within the line.
fn run_end(text: Text, mut pos: usize, big: bool) -> usize {
    let kind = class_at(text, pos, big);
    if kind == Class::LineBreak {
        return pos;
    }
    while class_at(text, pos, big) == kind {
        pos = text.next(pos);
    }
    pos
}

/// The start of the run of one class that `pos` is in, within the line.
fn run_start(text: Text, mut pos: usize, big: bool) -> usize {
    let kind = class_at(text, pos, big);
    let line_start = text.line_start(pos);
    while pos > line_start && class_at(text, text.prev(pos), big) == kind {
        pos = text.prev(pos);
    }
    pos
}

/// `iw`, `aw`, `iW` and `aW`.
pub fn word(text: Text, pos: usize, around: bool, big: bool, count: usize) -> Option<Object> {
    let line_end = text.line_end(pos);
    if pos >= line_end {
        // An empty line (or the end of one): nothing to select.
        return None;
    }
    let start = run_start(text, pos, big);
    let mut end = start;
    let started_on_blank = class_at(text, pos, big) == Class::Blank;
    if around {
        // Each count takes a word together with the blanks on one side of
        // it: the blanks before it if the cursor started on blanks, the
        // blanks after it otherwise.
        let mut trailing_blanks = false;
        for _ in 0..count {
            if end >= line_end {
                return None;
            }
            let first = run_end(text, end, big);
            end = if first < line_end {
                run_end(text, first, big)
            } else {
                first
            };
            trailing_blanks =
                !started_on_blank && first < end && class_at(text, first, big) == Class::Blank;
        }
        let mut start = start;
        if !started_on_blank && !trailing_blanks {
            // No blanks after the last word: take the ones before the
            // first, unless that would reach the start of the line.
            let line_start = text.line_start(pos);
            if start > line_start && class_at(text, text.prev(start), big) == Class::Blank {
                let before = run_start(text, text.prev(start), big);
                if before > line_start {
                    start = before;
                }
            }
        }
        Some(Object {
            start,
            end,
            linewise: false,
        })
    } else {
        for _ in 0..count {
            if end >= line_end {
                return None;
            }
            end = run_end(text, end, big);
        }
        Some(Object {
            start,
            end,
            linewise: false,
        })
    }
}

fn line_is_blank(text: Text, line: usize) -> bool {
    text.line_text(line).chars().all(|c| c == ' ' || c == '\t')
}

/// The start of the line after the run of lines of one kind (blank or not)
/// that begins at `line`; `None` if the run reaches the end of the text.
fn paragraph_end(text: Text, mut line: usize) -> Option<usize> {
    let blank = line_is_blank(text, line);
    loop {
        line = text.next_line(line)?;
        if line_is_blank(text, line) != blank {
            return Some(line);
        }
    }
}

/// `ip` and `ap`.
pub fn paragraph(text: Text, pos: usize, around: bool, count: usize) -> Option<Object> {
    let mut start = text.line_start(pos);
    let blank = line_is_blank(text, start);
    while let Some(previous) = text.prev_line(start) {
        if line_is_blank(text, previous) != blank {
            break;
        }
        start = previous;
    }
    // `ip` takes `count` runs of lines; `ap` takes pairs of runs.
    let runs = if around { count * 2 } else { count };
    let mut end = Some(start);
    let mut taken = 0;
    while taken < runs {
        let Some(from) = end else { break };
        end = paragraph_end(text, from);
        taken += 1;
    }
    if taken < runs {
        if !around || taken + 1 < runs {
            return None;
        }
        // A paragraph with nothing after it: `ap` takes the blank lines
        // before it instead, if the cursor started in the paragraph.
        if blank {
            return None;
        }
        while let Some(previous) = text.prev_line(start) {
            if !line_is_blank(text, previous) {
                break;
            }
            start = previous;
        }
    }
    let end = end.unwrap_or(text.len());
    Some(Object {
        start,
        end,
        linewise: true,
    })
}

/// `i(`, `a(` and their relatives: the innermost pair of `open` and `close`
/// around the cursor. If the cursor is in no pair, the search starts from
/// the next bracket after the cursor instead, on this line or a later one.
pub fn bracket(
    text: Text,
    pos: usize,
    open: char,
    close: char,
    around: bool,
    count: usize,
) -> Option<Object> {
    // Inside an opening bracket: its block, or nothing if it is never
    // closed. Not inside any: the block of the next bracket after the
    // cursor.
    let mut open_at = if let Some(open_at) = unmatched_open(text, pos, open, close) {
        open_at
    } else {
        let ahead = pos + text.0.get(pos..)?.find([open, close])?;
        unmatched_open(text, ahead, open, close)?
    };
    let mut close_at = matching_close(text, open_at, open, close)?;
    // A count asks for that many levels of nesting.
    for _ in 1..count {
        let outside = open_at.checked_sub(1)?;
        (open_at, close_at) = enclosing_from(text, outside, close_at, open, close)?;
    }
    bracket_object(text, open_at, close_at, around)
}

/// The innermost pair that contains the pair from `before + 1` to `inner_close`.
fn enclosing_from(
    text: Text,
    before: usize,
    inner_close: usize,
    open: char,
    close: char,
) -> Option<(usize, usize)> {
    let mut depth = 0_usize;
    let mut open_at = None;
    for (offset, c) in text.0.get(..=before)?.char_indices().rev() {
        if c == close {
            depth += 1;
        } else if c == open {
            if depth == 0 {
                open_at = Some(offset);
                break;
            }
            depth -= 1;
        }
    }
    let open_at = open_at?;
    let mut depth = 0_usize;
    let after = text.next(inner_close);
    for (offset, c) in text.0.get(after..)?.char_indices() {
        if c == open {
            depth += 1;
        } else if c == close {
            if depth == 0 {
                return Some((open_at, after + offset));
            }
            depth -= 1;
        }
    }
    None
}

/// The nearest `open` at or before `pos` that is not closed before `pos`.
fn unmatched_open(text: Text, pos: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0_usize;
    let upto = text.next(pos);
    for (offset, c) in text.0.get(..upto)?.char_indices().rev() {
        if c == close && offset != pos {
            depth += 1;
        } else if c == open {
            if depth == 0 {
                return Some(offset);
            }
            depth -= 1;
        }
    }
    None
}

/// The `close` that matches the `open` at `open_at`.
fn matching_close(text: Text, open_at: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0_usize;
    for (offset, c) in text.0.get(open_at..)?.char_indices() {
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(open_at + offset);
            }
        }
    }
    None
}

fn bracket_object(text: Text, open_at: usize, close_at: usize, around: bool) -> Option<Object> {
    if around {
        return Some(Object {
            start: open_at,
            end: text.next(close_at),
            linewise: false,
        });
    }
    let mut start = text.next(open_at);
    let mut end = close_at;
    // A block whose brackets stand on lines of their own is selected by
    // whole lines: the break after the opening bracket and the indentation
    // before the closing one do not belong to the inside.
    if text.char_at(start) == Some('\n') {
        let close_line = text.line_start(close_at);
        let only_indent = text
            .0
            .get(close_line..close_at)?
            .chars()
            .all(|c| c == ' ' || c == '\t');
        if only_indent && close_line > start {
            start += 1;
            end = close_line;
            return Some(Object {
                start,
                end,
                linewise: true,
            });
        }
        start += 1;
    }
    Some(Object {
        start,
        end,
        linewise: false,
    })
}

/// `i"`, `a"` and the same for `'` and `` ` ``: a quoted string on the
/// cursor's line.
pub fn quote(text: Text, pos: usize, quote: char, around: bool) -> Option<Object> {
    let (line_start, line_end) = (text.line_start(pos), text.line_end(pos));
    let line = text.0.get(line_start..line_end)?;
    let mut quotes = Vec::new();
    let mut escaped = false;
    for (offset, c) in line.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == quote {
            quotes.push(line_start + offset);
        }
    }
    // On a quote, the quotes pair up from the start of the line and the
    // string is the pair this quote belongs to. Elsewhere it is whatever
    // lies between the quote before the cursor and the one after it; with
    // no quote before the cursor, the first string after it.
    let (open, close) = match quotes.iter().position(|&at| at == pos) {
        Some(index) if index % 2 == 0 => (pos, *quotes.get(index + 1)?),
        Some(index) => (quotes[index - 1], pos),
        None => {
            let after = quotes.partition_point(|&at| at < pos);
            match after {
                0 => (*quotes.first()?, *quotes.get(1)?),
                after => (quotes[after - 1], *quotes.get(after)?),
            }
        }
    };
    if !around {
        return Some(Object {
            start: text.next(open),
            end: close,
            linewise: false,
        });
    }
    let mut start = open;
    let mut end = text.next(close);
    let after = run_end_of_blanks(text, end, line_end);
    if after > end {
        end = after;
    } else {
        while start > line_start && matches!(text.char_at(text.prev(start)), Some(' ' | '\t')) {
            start = text.prev(start);
        }
    }
    Some(Object {
        start,
        end,
        linewise: false,
    })
}

fn run_end_of_blanks(text: Text, mut pos: usize, line_end: usize) -> usize {
    while pos < line_end && matches!(text.char_at(pos), Some(' ' | '\t')) {
        pos = text.next(pos);
    }
    pos
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slice(text: &str, object: Option<Object>) -> Option<&str> {
        object.and_then(|object| text.get(object.start..object.end))
    }

    #[test]
    fn words() {
        let text = "foo  bar.baz qux\n";
        let get = |pos, around| slice(text, word(Text(text), pos, around, false, 1));
        assert_eq!(get(1, false), Some("foo"));
        assert_eq!(get(3, false), Some("  "));
        assert_eq!(get(1, true), Some("foo  "));
        assert_eq!(get(3, true), Some("  bar"));
        assert_eq!(get(8, false), Some("."));
        assert_eq!(
            get(14, true),
            Some(" qux"),
            "no blanks after: take those before"
        );
        assert_eq!(
            slice(text, word(Text(text), 5, false, true, 1)),
            Some("bar.baz")
        );
        assert_eq!(word(Text("\n"), 0, false, false, 1), None);
    }

    #[test]
    fn paragraphs() {
        let text = "a\nb\n\n\nc\n";
        let get = |pos, around| slice(text, paragraph(Text(text), pos, around, 1));
        assert_eq!(get(0, false), Some("a\nb\n"));
        assert_eq!(get(2, true), Some("a\nb\n\n\n"));
        assert_eq!(get(4, false), Some("\n\n"));
        assert_eq!(get(4, true), Some("\n\nc\n"));
        assert_eq!(
            get(6, true),
            Some("\n\nc\n"),
            "nothing after: take the blanks before"
        );
    }

    #[test]
    fn brackets() {
        let text = "f(a, (b), c)\n";
        let get = |pos, around| slice(text, bracket(Text(text), pos, '(', ')', around, 1));
        assert_eq!(get(3, false), Some("a, (b), c"));
        assert_eq!(get(6, false), Some("b"));
        assert_eq!(get(6, true), Some("(b)"));
        assert_eq!(get(1, true), Some("(a, (b), c)"));
        assert_eq!(get(11, false), Some("a, (b), c"));
        assert_eq!(
            get(0, false),
            Some("a, (b), c"),
            "outside any pair: the next one"
        );
        assert_eq!(
            slice(
                "a ) (b)\n",
                bracket(Text("a ) (b)\n"), 0, '(', ')', false, 1)
            ),
            None
        );
        let block = "{\n  x\n}\n";
        let inner = bracket(Text(block), 4, '{', '}', false, 1);
        assert_eq!(slice(block, inner), Some("  x\n"));
        assert_eq!(inner.map(|object| object.linewise), Some(true));
    }

    #[test]
    fn quotes() {
        let text = "say \"hi \\\" there\" now 'x'\n";
        let get = |pos, quote_char, around| slice(text, quote(Text(text), pos, quote_char, around));
        assert_eq!(get(6, '"', false), Some("hi \\\" there"));
        assert_eq!(
            get(0, '"', false),
            Some("hi \\\" there"),
            "before the string: the next one"
        );
        assert_eq!(get(6, '"', true), Some("\"hi \\\" there\" "));
        assert_eq!(get(22, '\'', true), Some(" 'x'"));
        assert_eq!(get(0, '`', false), None);
    }
}
