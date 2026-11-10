//! Cursor motions.
//!
//! The word motions are ports of Vim's own (`fwd_word`, `bck_word`,
//! `end_word`, `bckend_word`), because their behaviour at line ends, on
//! empty lines and at the ends of the text is too particular to rederive.
//! Vim's cursor can stand on the end-of-line position of a line; here that
//! position is the byte offset of the line break, so the ports walk the text
//! exactly as Vim walks its buffer.

use crate::text::{Class, Text, class};

/// How an operator treats the text between the cursor and where a motion
/// leads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Up to but not including the character at the later position.
    Exclusive,
    /// Including the character at the later position.
    Inclusive,
    /// All lines touched.
    Linewise,
}

/// A walker over the text with Vim's notion of stepping.
struct Walker<'a> {
    text: Text<'a>,
    pos: usize,
    big: bool,
}

/// What a step did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stepped {
    /// Moved within the line.
    Within,
    /// Moved onto the end-of-line position.
    OntoEnd,
    /// Moved onto another line.
    NewLine,
    /// At the end (or start) of the text; did not move.
    Blocked,
}

impl Walker<'_> {
    fn at_line_end(&self) -> bool {
        matches!(self.text.char_at(self.pos), None | Some('\n'))
    }

    /// The class of the character under the walker, with the end of a line
    /// counting as blank.
    fn class(&self) -> Class {
        match self.text.char_at(self.pos) {
            None | Some('\n') => Class::Blank,
            Some(c) => class(c, self.big),
        }
    }

    fn on_empty_line(&self) -> bool {
        self.text.line_start(self.pos) == self.pos && self.at_line_end()
    }

    fn on_last_line(&self) -> bool {
        self.text.next_line(self.pos).is_none()
    }

    /// Vim's `inc_cursor`.
    fn forward(&mut self) -> Stepped {
        if self.at_line_end() {
            match self.text.next_line(self.pos) {
                Some(next) => {
                    self.pos = next;
                    Stepped::NewLine
                }
                None => Stepped::Blocked,
            }
        } else {
            self.pos = self.text.next(self.pos);
            if self.at_line_end() {
                Stepped::OntoEnd
            } else {
                Stepped::Within
            }
        }
    }

    /// Vim's `dec_cursor`.
    fn backward(&mut self) -> Stepped {
        if self.pos == 0 {
            return Stepped::Blocked;
        }
        let was_line_start = self.text.line_start(self.pos) == self.pos;
        self.pos = self.text.prev(self.pos);
        if was_line_start {
            Stepped::NewLine
        } else {
            Stepped::Within
        }
    }

    fn column_is_zero(&self) -> bool {
        self.text.line_start(self.pos) == self.pos
    }

    /// Whether the walker is at the very end and cannot go on.
    fn at_text_end(&self) -> bool {
        self.at_line_end() && self.text.next_line(self.pos).is_none()
    }

    /// Vim's `back_in_line`: to the start of the word or the run of blanks
    /// under the walker.
    fn back_in_line(&mut self) {
        let class = self.class();
        while !self.column_is_zero() {
            let here = self.pos;
            self.backward();
            if self.class() != class {
                self.pos = here;
                break;
            }
        }
    }

    /// Vim's `decl`: one back, and from the start of a line onto the last
    /// character of the line before (not its end).
    fn back_over_line_end(&mut self) {
        if self.backward() == Stepped::NewLine && !self.column_is_zero() {
            self.backward();
        }
    }

    /// Steps while the class stays `class`. Returns `true` if it ran into
    /// the end of the text.
    fn skip(&mut self, class: Class, forward: bool) -> bool {
        while self.class() == class {
            let stepped = if forward {
                self.forward()
            } else {
                self.backward()
            };
            if stepped == Stepped::Blocked {
                return true;
            }
        }
        false
    }
}

/// `w` and `W`. With `stop_at_line_end`, as when an operator is pending,
/// the last word of a line ends at the line's end instead of at the next
/// line's first word. Returns where the motion got to and whether it got
/// as far as asked.
pub fn word_forward(
    text: Text,
    pos: usize,
    count: usize,
    big: bool,
    stop_at_line_end: bool,
) -> (usize, bool) {
    let mut walker = Walker { text, pos, big };
    for remaining in (0..count).rev() {
        let last = remaining == 0;
        let start_class = walker.class();
        let on_last_line = walker.on_last_line();
        let stepped = walker.forward();
        if stepped == Stepped::Blocked || (stepped == Stepped::NewLine && on_last_line) {
            return (walker.pos, false);
        }
        if stepped != Stepped::Within && stop_at_line_end && last {
            return (walker.pos, true);
        }
        if start_class != Class::Blank {
            while walker.class() == start_class {
                let stepped = walker.forward();
                if stepped == Stepped::Blocked
                    || (stepped != Stepped::Within && stop_at_line_end && last)
                {
                    return (walker.pos, true);
                }
            }
        }
        while walker.class() == Class::Blank {
            if walker.on_empty_line() {
                break;
            }
            let on_last_line = walker.on_last_line();
            let stepped = walker.forward();
            if stepped == Stepped::Blocked || (stepped == Stepped::NewLine && on_last_line) {
                return (walker.pos, true);
            }
        }
    }
    (walker.pos, true)
}

/// `b` and `B`. Returns where the motion got to and whether it got as far
/// as asked; like Vim, it moves as far as it can either way.
pub fn word_backward(text: Text, pos: usize, count: usize, big: bool) -> (usize, bool) {
    let mut walker = Walker { text, pos, big };
    for _ in 0..count {
        if walker.backward() == Stepped::Blocked {
            return (walker.pos, false);
        }
        let mut overshot = true;
        while walker.class() == Class::Blank {
            if walker.on_empty_line() {
                overshot = false;
                break;
            }
            if walker.backward() == Stepped::Blocked {
                return (walker.pos, true);
            }
        }
        if overshot {
            if walker.skip(walker.class(), false) {
                return (walker.pos, true);
            }
            walker.forward();
        }
    }
    (walker.pos, true)
}

/// `e` and `E`. With `stop`, as for `cw`, a cursor already at the end of a
/// word stays there for the first count. Returns where the motion got to
/// and whether it got as far as asked.
pub fn word_end(text: Text, pos: usize, count: usize, big: bool, stop: bool) -> (usize, bool) {
    word_end_or_empty_line(text, pos, count, big, stop, false)
}

/// [`word_end`], optionally also stopping on an empty line, as the word
/// objects need.
fn word_end_or_empty_line(
    text: Text,
    pos: usize,
    count: usize,
    big: bool,
    mut stop: bool,
    empty_lines: bool,
) -> (usize, bool) {
    let mut walker = Walker { text, pos, big };
    for _ in 0..count {
        let start_class = walker.class();
        if walker.forward() == Stepped::Blocked {
            return (walker.pos, false);
        }
        let mut overshot = true;
        if walker.class() == start_class && start_class != Class::Blank {
            if walker.skip(start_class, true) {
                return (walker.pos, false);
            }
        } else if !stop || start_class == Class::Blank {
            while walker.class() == Class::Blank {
                if empty_lines && walker.on_empty_line() {
                    overshot = false;
                    break;
                }
                if walker.forward() == Stepped::Blocked {
                    return (walker.pos, false);
                }
            }
            if overshot && walker.skip(walker.class(), true) {
                return (walker.pos, false);
            }
        }
        if overshot {
            walker.backward();
        }
        stop = false;
    }
    (walker.pos, true)
}

/// `iw`, `aw`, `iW` and `aW`: a port of Vim's `current_word`. Returns the
/// two ends of the object (the second may come before the first) and
/// whether the second end is included. `None` if there are not `count`
/// words to take.
pub fn word_object(
    text: Text,
    pos: usize,
    count: usize,
    around: bool,
    big: bool,
) -> Option<(usize, usize, bool)> {
    let mut walker = Walker { text, pos, big };
    let mut inclusive = true;
    let mut include_white = false;
    walker.back_in_line();
    let mut start = walker.pos;
    if (walker.class() == Class::Blank) == around {
        // On blanks and they are wanted, or on a word and they are not:
        // go to the end of the word.
        let (to, ok) = word_end_or_empty_line(text, walker.pos, 1, big, true, true);
        if !ok {
            return None;
        }
        walker.pos = to;
    } else {
        // Otherwise up to where the next word starts, and one back.
        walker.pos = word_forward(text, walker.pos, 1, big, true).0;
        if walker.column_is_zero() {
            walker.back_over_line_end();
        } else {
            walker.backward();
        }
        include_white = around;
    }
    for _ in 1..count {
        inclusive = true;
        // One on, over a line end if that is what comes.
        if walker.forward() == Stepped::OntoEnd {
            if walker.forward() == Stepped::Blocked {
                return None;
            }
        } else if walker.at_text_end() {
            return None;
        }
        if around {
            let (to, ok) = word_forward(text, walker.pos, 1, big, true);
            if !ok {
                return None;
            }
            walker.pos = to;
            if walker.column_is_zero() {
                inclusive = false;
            } else {
                walker.backward();
            }
        } else {
            let (to, ok) = word_end_or_empty_line(text, walker.pos, 1, big, true, true);
            if !ok {
                return None;
            }
            walker.pos = to;
        }
    }
    if include_white && (walker.class() != Class::Blank || (walker.column_is_zero() && !inclusive))
    {
        // No blanks were taken at the end: take the ones before the
        // start instead, unless they are the line's indent.
        let mut before = Walker {
            text,
            pos: start,
            big,
        };
        if !before.column_is_zero() {
            before.backward();
            before.back_in_line();
            if before.class() == Class::Blank && !before.column_is_zero() {
                start = before.pos;
            }
        }
    }
    Some((start, walker.pos, inclusive))
}

/// `ge` and `gE`. Returns where the motion got to and whether it got as far
/// as asked.
pub fn word_end_backward(text: Text, pos: usize, count: usize, big: bool) -> (usize, bool) {
    let mut walker = Walker { text, pos, big };
    for _ in 0..count {
        let start_class = walker.class();
        if walker.backward() == Stepped::Blocked {
            return (walker.pos, false);
        }
        if start_class != Class::Blank {
            while walker.class() == start_class {
                if walker.backward() == Stepped::Blocked {
                    return (walker.pos, true);
                }
            }
        }
        while walker.class() == Class::Blank {
            if walker.on_empty_line() {
                break;
            }
            if walker.backward() == Stepped::Blocked {
                return (walker.pos, true);
            }
        }
    }
    (walker.pos, true)
}

/// `}` (forward) and `{`: to the next boundary between paragraphs, which is
/// an empty line. Returns the position and whether the motion became
/// inclusive (it does at the end of the text). `None` if there are fewer
/// than `count` paragraphs to cross.
pub fn paragraph(text: Text, pos: usize, count: usize, forward: bool) -> Option<(usize, bool)> {
    let mut line = text.line_start(pos);
    for remaining in (0..count).rev() {
        let mut seen_text = false;
        let mut first = true;
        loop {
            if !text.line_is_empty(line) {
                seen_text = true;
            }
            if !first && seen_text && text.line_is_empty(line) {
                break;
            }
            first = false;
            let next = if forward {
                text.next_line(line)
            } else {
                text.prev_line(line)
            };
            match next {
                Some(next) => line = next,
                None if remaining > 0 => return None,
                None => break,
            }
        }
    }
    if forward && text.next_line(line).is_none() && !text.line_is_empty(line) {
        // On the last line the motion reaches the last character and
        // includes it.
        return Some((text.line_last(line), true));
    }
    Some((line, false))
}

/// `f`, `F`, `t` and `T`: to the `count`-th `target` on the line, or just
/// before it with `till`. `None` if there are not that many.
pub fn find(
    text: Text,
    pos: usize,
    count: usize,
    target: char,
    forward: bool,
    till: bool,
) -> Option<usize> {
    let (start, end) = (text.line_start(pos), text.line_end(pos));
    let mut found = pos;
    if forward {
        let mut from = text.next(pos).min(end);
        for _ in 0..count {
            let offset = text.0.get(from..end)?.find(target)?;
            found = from + offset;
            from = text.next(found);
        }
        Some(if till { text.prev(found) } else { found })
    } else {
        let mut until = pos;
        for _ in 0..count {
            found = start + text.0.get(start..until)?.rfind(target)?;
            until = found;
        }
        Some(if till { text.next(found) } else { found })
    }
}

/// `%`: the bracket matching the first bracket at or after the cursor on
/// its line.
pub fn matching_bracket(text: Text, pos: usize) -> Option<usize> {
    const PAIRS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];
    let end = text.line_end(pos);
    let (at, bracket) = text
        .0
        .get(pos..end)?
        .char_indices()
        .find(|(_, c)| PAIRS.iter().any(|(open, close)| c == open || c == close))
        .map(|(offset, c)| (pos + offset, c))?;
    let (open, close) = *PAIRS
        .iter()
        .find(|(open, close)| bracket == *open || bracket == *close)?;
    let mut depth = 0_usize;
    if bracket == open {
        for (offset, c) in text.0.get(at..)?.char_indices() {
            if c == open {
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    return Some(at + offset);
                }
            }
        }
    } else {
        let upto = text.next(at);
        for (offset, c) in text.0.get(..upto)?.char_indices().rev() {
            if c == close {
                depth += 1;
            } else if c == open {
                depth -= 1;
                if depth == 0 {
                    return Some(offset);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(text: &str, pos: usize) -> usize {
        word_forward(Text(text), pos, 1, false, false).0
    }

    #[test]
    fn word_forward_stops_at_words_punctuation_and_empty_lines() {
        let text = "foo.bar  baz\n\nqux\n";
        assert_eq!(w(text, 0), 3);
        assert_eq!(w(text, 3), 4);
        assert_eq!(w(text, 4), 9);
        assert_eq!(w(text, 9), 13, "an empty line is a word");
        assert_eq!(w(text, 13), 14);
        // At the last word the cursor ends on the end of the line.
        assert_eq!(word_forward(Text(text), 14, 1, false, false), (17, true));
        assert_eq!(word_forward(Text(text), 0, 1, true, false).0, 9);
    }

    #[test]
    fn word_forward_for_an_operator_stays_on_the_line() {
        let text = "foo bar\nbaz\n";
        assert_eq!(word_forward(Text(text), 4, 1, false, true), (7, true));
        assert_eq!(word_forward(Text(text), 4, 2, false, true), (11, true));
    }

    #[test]
    fn word_backward_and_ends() {
        let text = Text("foo.bar  baz\n\nqux\n");
        assert_eq!(word_backward(text, 14, 1, false), (13, true));
        assert_eq!(word_backward(text, 13, 1, false), (9, true));
        assert_eq!(word_backward(text, 9, 1, false), (4, true));
        assert_eq!(word_backward(text, 9, 1, true), (0, true));
        assert_eq!(word_backward(text, 0, 1, false), (0, false));
        // Asked for more words than there are: as far as it goes.
        assert_eq!(word_backward(text, 4, 9, false), (0, true));
        assert_eq!(word_end(text, 0, 1, false, false), (2, true));
        assert_eq!(word_end(text, 2, 1, false, false), (3, true));
        assert_eq!(word_end(text, 6, 1, false, false), (11, true));
        assert_eq!(word_end(text, 11, 1, false, false), (16, true));
        assert!(!word_end(text, 16, 1, false, false).1);
        assert_eq!(word_end_backward(text, 9, 1, false), (6, true));
        assert_eq!(word_end_backward(text, 14, 1, false), (13, true));
        assert_eq!(word_end_backward(text, 4, 1, true).0, 0);
    }

    #[test]
    fn paragraphs() {
        let text = Text("a\nb\n\n\nc\nd\n");
        assert_eq!(paragraph(text, 0, 1, true), Some((4, false)));
        assert_eq!(
            paragraph(text, 4, 1, true),
            Some((8, true)),
            "last line: inclusive"
        );
        assert_eq!(paragraph(text, 8, 1, false), Some((5, false)));
        assert_eq!(paragraph(text, 5, 1, false), Some((0, false)));
        assert_eq!(paragraph(text, 0, 3, true), None);
    }

    #[test]
    fn finding_characters() {
        let text = Text("a,b,c\nd,\n");
        assert_eq!(find(text, 0, 1, ',', true, false), Some(1));
        assert_eq!(find(text, 0, 2, ',', true, false), Some(3));
        assert_eq!(find(text, 0, 2, ',', true, true), Some(2));
        assert_eq!(
            find(text, 0, 3, ',', true, false),
            None,
            "never past the line"
        );
        assert_eq!(find(text, 4, 1, ',', false, false), Some(3));
        assert_eq!(find(text, 4, 2, ',', false, true), Some(2));
        assert_eq!(find(text, 0, 1, ',', false, false), None);
    }

    #[test]
    fn brackets() {
        let text = Text("f(a[1], {b})\n");
        assert_eq!(matching_bracket(text, 0), Some(11));
        assert_eq!(matching_bracket(text, 11), Some(1));
        assert_eq!(matching_bracket(text, 3), Some(5));
        assert_eq!(matching_bracket(text, 5), Some(3));
        assert_eq!(matching_bracket(Text("(\n"), 0), None);
        assert_eq!(matching_bracket(Text("abc\n"), 0), None);
    }
}
