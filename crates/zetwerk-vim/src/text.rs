//! Finding one's way around text: lines, characters, columns.
//!
//! All positions are byte offsets on character boundaries. A line owns the
//! line break that ends it: in `"a\nb\n"` there are two lines, and the final
//! break does not start a third. Empty text is one empty line.

/// How wide a tab stop is when counting screen columns.
const TAB_WIDTH: usize = 8;

/// Read-only helpers over a text.
#[derive(Clone, Copy)]
pub struct Text<'a>(pub &'a str);

impl<'a> Text<'a> {
    pub fn len(self) -> usize {
        self.0.len()
    }

    /// The character starting at `pos`, if `pos` is inside the text.
    pub fn char_at(self, pos: usize) -> Option<char> {
        self.0.get(pos..)?.chars().next()
    }

    /// The position after the character at `pos`.
    pub fn next(self, pos: usize) -> usize {
        self.char_at(pos).map_or(self.len(), |c| pos + c.len_utf8())
    }

    /// The start of the character before `pos`; `pos` itself at the start.
    pub fn prev(self, pos: usize) -> usize {
        self.0
            .get(..pos)
            .and_then(|before| before.chars().next_back())
            .map_or(pos, |c| pos - c.len_utf8())
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

    /// The screen column of `pos` within its line, counting from 0.
    pub fn column(self, pos: usize) -> usize {
        let start = self.line_start(pos);
        let before = self.0.get(start..pos).unwrap_or("");
        before
            .chars()
            .fold(0, |column, c| column + width(c, column))
    }

    /// The position in the line starting at `line_start` that covers screen
    /// column `column`, or the line's last character if it is shorter.
    pub fn at_column(self, line_start: usize, column: usize) -> usize {
        let line = self.line_text(line_start);
        let mut at = 0;
        for (offset, c) in line.char_indices() {
            let next = at + width(c, at);
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
        _ if big || c.is_alphanumeric() || c == '_' => Class::Word,
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
    }
}
