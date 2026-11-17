//! Finding one's way around text: lines, characters, columns.
//!
//! All positions are byte offsets on character boundaries; the cursor
//! steps by what a reader takes for one character (see [`Text::next`]).
//! A line owns the line break that ends it: in `"a\nb\n"` there are two
//! lines, and the final break does not start a third. Empty text is one
//! empty line.

use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};

/// How wide a tab stop is when counting screen columns.
const TAB_WIDTH: usize = 8;

/// Read-only helpers over a text.
#[derive(Clone, Copy)]
pub struct Text<'a>(pub &'a str);

impl<'a> Text<'a> {
    pub fn len(self) -> usize {
        self.0.len()
    }

    pub fn is_empty(self) -> bool {
        self.0.is_empty()
    }

    /// The character starting at `pos`, if `pos` is inside the text.
    pub fn char_at(self, pos: usize) -> Option<char> {
        self.0.get(pos..)?.chars().next()
    }

    /// The position after the character at `pos`. A character is what a
    /// reader takes for one (a grapheme cluster): a letter with the marks
    /// set on it, an emoji with its skin tone, a flag. Line breaks are
    /// characters of their own, `\r` of `\r\n` too.
    pub fn next(self, pos: usize) -> usize {
        let Some(c) = self.char_at(pos) else {
            return self.len();
        };
        let after = pos + c.len_utf8();
        // Nothing joins a line break, nor two plain letters.
        let plain = |at: usize| self.0.as_bytes().get(at).is_none_or(u8::is_ascii);
        if matches!(c, '\n' | '\r') || (c.is_ascii() && plain(after)) {
            return after;
        }
        let mut cursor = GraphemeCursor::new(pos, self.len(), true);
        match cursor.next_boundary(self.0, 0) {
            Ok(Some(end)) => end,
            _ => after,
        }
    }

    /// The start of the character before `pos`; `pos` itself at the start.
    /// See [`Text::next`] for what a character is.
    pub fn prev(self, pos: usize) -> usize {
        let Some(c) = self
            .0
            .get(..pos)
            .and_then(|before| before.chars().next_back())
        else {
            return pos;
        };
        let start = pos - c.len_utf8();
        let plain = |at: usize| self.0.as_bytes().get(at).is_none_or(u8::is_ascii);
        // A plain letter after another (or first) and before one is a
        // character by itself.
        let plain_before = start == 0 || plain(start - 1);
        if matches!(c, '\n' | '\r') || (c.is_ascii() && plain_before && plain(pos)) {
            return start;
        }
        let mut cursor = GraphemeCursor::new(pos, self.len(), true);
        match cursor.prev_boundary(self.0, 0) {
            Ok(Some(begin)) => begin,
            _ => start,
        }
    }

    /// `range` made to begin and end between characters (see
    /// [`Text::next`]): a glyph of a page may show only part of one, where
    /// its font has no glyph for the whole. An empty range stays as it is.
    pub fn whole(self, range: std::ops::Range<usize>) -> std::ops::Range<usize> {
        if range.is_empty() || range.end > self.len() {
            return range;
        }
        let start = self.prev(self.next(range.start)).min(range.start);
        let end = self.next(self.prev(range.end)).max(range.end);
        start..end
    }

    /// The start of the line containing `pos`.
    pub fn line_start(self, pos: usize) -> usize {
        self.0
            .get(..pos)
            .and_then(|before| before.rfind('\n'))
            .map_or(0, |at| at + 1)
    }

    /// The end of the line containing `pos`: where its line break is, or
    /// the end of the text.
    pub fn line_end(self, pos: usize) -> usize {
        self.0
            .get(pos..)
            .and_then(|rest| rest.find('\n'))
            .map_or(self.len(), |at| pos + at)
    }

    /// The start of the line after the one containing `pos`, if there is
    /// one.
    pub fn next_line(self, pos: usize) -> Option<usize> {
        let start = self.line_end(pos) + 1;
        (start < self.len()).then_some(start)
    }

    /// The start of the line before the one containing `pos`, if there is
    /// one.
    pub fn prev_line(self, pos: usize) -> Option<usize> {
        let start = self.line_start(pos);
        (start > 0).then(|| self.line_start(start - 1))
    }

    /// The start of the last line.
    pub fn last_line(self) -> usize {
        let body = self.0.strip_suffix('\n').unwrap_or(self.0);
        self.line_start(body.len())
    }

    /// Number of lines; at least one.
    pub fn line_count(self) -> usize {
        let body = self.0.strip_suffix('\n').unwrap_or(self.0);
        body.bytes().filter(|&byte| byte == b'\n').count() + 1
    }

    /// The start of line number `index` (counting from 0), or of the last
    /// line if there are fewer.
    pub fn line(self, index: usize) -> usize {
        let mut start = 0;
        for _ in 0..index {
            match self.next_line(start) {
                Some(next) => start = next,
                None => break,
            }
        }
        start
    }

    /// Which line `pos` is on, counting from 0.
    pub fn line_index(self, pos: usize) -> usize {
        self.0
            .get(..self.line_start(pos))
            .map_or(0, |before| before.bytes().filter(|&b| b == b'\n').count())
    }

    /// The line containing `pos`, without its line break.
    pub fn line_text(self, pos: usize) -> &'a str {
        self.0
            .get(self.line_start(pos)..self.line_end(pos))
            .unwrap_or("")
    }

    /// Whether the line containing `pos` has no characters.
    pub fn line_is_empty(self, pos: usize) -> bool {
        self.line_start(pos) == self.line_end(pos)
    }

    /// The start of the last character of the line containing `pos`; the
    /// line start if the line is empty. This is as far right as the cursor
    /// goes in normal mode.
    pub fn line_last(self, pos: usize) -> usize {
        let (start, end) = (self.line_start(pos), self.line_end(pos));
        if start == end { start } else { self.prev(end) }
    }

    /// The first character of the line containing `pos` that is not a blank;
    /// the last character if the line is all blanks.
    pub fn first_non_blank(self, pos: usize) -> usize {
        let start = self.line_start(pos);
        let line = self.line_text(pos);
        match line.find(|c: char| c != ' ' && c != '\t') {
            Some(offset) => start + offset,
            None => self.line_last(pos),
        }
    }

    /// The screen column of `pos` within its line, counting from 0. A
    /// character (see [`Text::next`]) is one column, a tab up to the next
    /// stop.
    pub fn column(self, pos: usize) -> usize {
        let start = self.line_start(pos);
        let before = self.0.get(start..pos).unwrap_or("");
        if before.is_ascii() {
            return before
                .chars()
                .fold(0, |column, c| column + width(c, column));
        }
        before
            .graphemes(true)
            .fold(0, |column, cluster| column + cluster_width(cluster, column))
    }

    /// The position in the line starting at `line_start` that covers screen
    /// column `column`, or the line's last character if it is shorter.
    pub fn at_column(self, line_start: usize, column: usize) -> usize {
        let line = self.line_text(line_start);
        let mut at = 0;
        for (offset, cluster) in line.grapheme_indices(true) {
            let next = at + cluster_width(cluster, at);
            if column < next {
                return line_start + offset;
            }
            at = next;
        }
        self.line_last(line_start)
    }
}

fn width(c: char, column: usize) -> usize {
    if c == '\t' {
        TAB_WIDTH - column % TAB_WIDTH
    } else {
        1
    }
}

fn cluster_width(cluster: &str, column: usize) -> usize {
    cluster.chars().next().map_or(0, |c| width(c, column))
}

/// Whether `c` is a mark set on the letter before it: an accent written
/// apart (U+0301), as some keyboards and input methods type them.
pub fn is_mark(c: char) -> bool {
    matches!(
        c,
        '\u{300}'..='\u{36f}'
            | '\u{1ab0}'..='\u{1aff}'
            | '\u{1dc0}'..='\u{1dff}'
            | '\u{20d0}'..='\u{20ff}'
            | '\u{fe20}'..='\u{fe2f}'
    )
}

/// What kind of character something is, for word motions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Blank,
    /// Letters, digits and the underscore.
    Word,
    /// Everything else that is visible.
    Punctuation,
    LineBreak,
}

/// The class of `c`. With `big`, for the motions on WORDs, everything
/// visible is one class.
pub fn class(c: char, big: bool) -> Class {
    match c {
        '\n' => Class::LineBreak,
        ' ' | '\t' => Class::Blank,
        // A mark set on a letter is of the letter's word.
        _ if big || c.is_alphanumeric() || c == '_' || is_mark(c) => Class::Word,
        _ => Class::Punctuation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_own_their_line_break() {
        let text = Text("ab\n\ncd\n");
        assert_eq!(text.line_count(), 3);
        assert_eq!((text.line_start(4), text.line_end(4)), (4, 6));
        assert_eq!((text.line_start(3), text.line_end(3)), (3, 3));
        assert_eq!(text.next_line(0), Some(3));
        assert_eq!(text.next_line(4), None, "the final break starts no line");
        assert_eq!(text.prev_line(4), Some(3));
        assert_eq!(text.prev_line(1), None);
        assert_eq!(text.last_line(), 4);
        assert_eq!((text.line(0), text.line(2), text.line(9)), (0, 4, 4));
        assert_eq!(
            (text.line_index(0), text.line_index(3), text.line_index(5)),
            (0, 1, 2)
        );
    }

    #[test]
    fn empty_text_and_missing_final_break() {
        let empty = Text("");
        assert_eq!(
            (empty.line_count(), empty.last_line(), empty.line_last(0)),
            (1, 0, 0)
        );
        let open = Text("a\nbc");
        assert_eq!(
            (open.line_count(), open.last_line(), open.line_end(2)),
            (2, 2, 4)
        );
        let blank_last = Text("a\n\n");
        assert_eq!((blank_last.line_count(), blank_last.last_line()), (2, 2));
    }

    #[test]
    fn characters_and_columns() {
        let text = Text("  é\tx\n");
        assert_eq!((text.next(2), text.prev(4), text.prev(0)), (4, 2, 0));
        assert_eq!(text.first_non_blank(0), 2);
        assert_eq!(text.line_last(0), 5);
        // The tab reaches from column 3 to the next multiple of 8.
        assert_eq!((text.column(4), text.column(5)), (3, 8));
        assert_eq!(
            (
                text.at_column(0, 5),
                text.at_column(0, 8),
                text.at_column(0, 99)
            ),
            (4, 5, 5)
        );
        assert_eq!(Text("   \n").first_non_blank(0), 2);
    }

    #[test]
    fn classes() {
        assert_eq!(class('é', false), Class::Word);
        assert_eq!(class('_', false), Class::Word);
        assert_eq!(class('-', false), Class::Punctuation);
        assert_eq!(class('-', true), Class::Word);
        assert_eq!(class('\t', true), Class::Blank);
        // An accent written apart is of the word of its letter.
        assert_eq!(class('\u{301}', false), Class::Word);
    }

    #[test]
    fn a_character_is_what_a_reader_takes_for_one() {
        // A letter with an accent written apart, an emoji with a skin
        // tone, a flag, a family joined by joiners: one step each.
        let text =
            Text("e\u{301}x\u{1f44d}\u{1f3fd}\u{1f1e9}\u{1f1ea}\u{1f468}\u{200d}\u{1f469}!\n");
        let mut stops = vec![0];
        while *stops.last().unwrap_or(&0) < text.len() {
            stops.push(text.next(*stops.last().unwrap_or(&0)));
        }
        assert_eq!(stops, vec![0, 3, 4, 12, 20, 31, 32, 33]);
        let mut back = vec![text.len()];
        while *back.last().unwrap_or(&0) > 0 {
            back.push(text.prev(*back.last().unwrap_or(&0)));
        }
        back.reverse();
        assert_eq!(back, stops);
        // One column each, and a column finds the whole of it.
        assert_eq!(
            (text.column(3), text.column(12), text.column(31)),
            (1, 3, 5)
        );
        assert_eq!((text.at_column(0, 1), text.at_column(0, 3)), (3, 12));
        assert_eq!(text.line_last(0), 31);
        // What shows part of one is made to take all of it.
        assert_eq!((text.whole(4..8), text.whole(8..12)), (4..12, 4..12));
        assert_eq!((text.whole(0..1), text.whole(3..4)), (0..3, 3..4));
        assert_eq!(text.whole(5..5), 5..5);
        // A line break stays a character of its own, also after `\r`.
        let crlf = Text("a\r\nb");
        assert_eq!(
            (crlf.next(1), crlf.next(2), crlf.prev(4), crlf.prev(3)),
            (2, 3, 3, 2)
        );
    }
}
