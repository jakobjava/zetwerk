//! Changes to text, in a form that can be checked and undone.

use std::fmt;
use std::ops::Range;

/// Text that can be edited in place. Offsets are bytes.
///
/// The editor keeps its text in Typst's own source type, which reparses
/// incrementally on every edit; tests use a plain `String`. This crate only
/// needs to read the text and to replace ranges of it.
pub trait Buffer {
    fn text(&self) -> &str;

    /// Replaces `range` with `with`. Only called with ranges that lie within
    /// the text on character boundaries.
    fn replace(&mut self, range: Range<usize>, with: &str);
}

impl Buffer for String {
    fn text(&self) -> &str {
        self
    }

    fn replace(&mut self, range: Range<usize>, with: &str) {
        self.replace_range(range, with);
    }
}

/// A request to replace part of the text: what callers ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replace {
    pub range: Range<usize>,
    pub text: String,
}

impl Replace {
    #[must_use]
    pub fn new(range: Range<usize>, text: impl Into<String>) -> Self {
        Self {
            range,
            text: text.into(),
        }
    }

    /// An insertion at `at`.
    #[must_use]
    pub fn insert(at: usize, text: impl Into<String>) -> Self {
        Self::new(at..at, text)
    }

    /// A deletion of `range`.
    #[must_use]
    pub fn delete(range: Range<usize>) -> Self {
        Self::new(range, "")
    }
}

/// A replacement that knows what it replaced: at byte `at`, `old` became
/// `new`.
///
/// Carrying the old text makes a change its own inverse, and makes applying
/// it to the wrong text an error instead of silent corruption: it only
/// applies where `old` (or, undoing, `new`) is really what stands there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub at: usize,
    pub old: String,
    pub new: String,
}

/// Why an edit was not applied. The text is as it was before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// The range reaches beyond the text or cuts through a character.
    InvalidRange { range: Range<usize>, len: usize },
    /// The text at this position is not what the change expects to replace.
    /// The change was made for a different state of the text.
    Mismatch { at: usize },
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRange { range, len } => write!(
                f,
                "bytes {}..{} are not a valid range of a text of {len} bytes",
                range.start, range.end
            ),
            Self::Mismatch { at } => {
                write!(
                    f,
                    "the text at byte {at} is not what the change was made for"
                )
            }
        }
    }
}

impl std::error::Error for EditError {}

/// Which way a change is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// `old` becomes `new`.
    Forward,
    /// `new` becomes `old`: undo.
    Backward,
}

impl Change {
    fn sides(&self, direction: Direction) -> (&str, &str) {
        match direction {
            Direction::Forward => (&self.old, &self.new),
            Direction::Backward => (&self.new, &self.old),
        }
    }

    /// Folds `next`, a change made right after this one, into this one if
    /// the two amount to a single replacement: typing on, deleting on, or
    /// deleting what was just typed. Returns whether it did.
    ///
    /// The merged change has the same effect in both directions as the two
    /// in sequence.
    pub fn absorb(&mut self, next: &Change) -> bool {
        let end = self.at + self.new.len();
        if next.old.is_empty() && next.at == end {
            // Typing on: inserted right after what this change put there.
            self.new.push_str(&next.new);
        } else if next.new.is_empty() && self.new.is_empty() && next.at == self.at {
            // Deleting forward from the same spot.
            self.old.push_str(&next.old);
        } else if next.new.is_empty() && self.new.is_empty() && next.at + next.old.len() == self.at
        {
            // Backspacing: deleted right before what this change deleted.
            self.old.insert_str(0, &next.old);
            self.at = next.at;
        } else if next.new.is_empty()
            && next.at >= self.at
            && next.at + next.old.len() == end
            && self.new.ends_with(next.old.as_str())
        {
            // Deleting the end of what this change put there.
            self.new.truncate(next.at - self.at);
        } else {
            return false;
        }
        true
    }

    /// Applies the change if the text it expects is there.
    fn apply(&self, buffer: &mut impl Buffer, direction: Direction) -> Result<(), EditError> {
        let (expected, replacement) = self.sides(direction);
        let range = self.at..self.at.saturating_add(expected.len());
        if buffer.text().get(range.clone()) != Some(expected) {
            return Err(EditError::Mismatch { at: self.at });
        }
        buffer.replace(range, replacement);
        Ok(())
    }
}

/// Turns `edits` into changes by applying them to `buffer` one after the
/// other; each range refers to the text as the edits before it left it.
/// Either all are applied or, on error, none.
pub fn perform(buffer: &mut impl Buffer, edits: &[Replace]) -> Result<Vec<Change>, EditError> {
    let mut changes: Vec<Change> = Vec::with_capacity(edits.len());
    for edit in edits {
        let text = buffer.text();
        let Some(old) = text.get(edit.range.clone()) else {
            let error = EditError::InvalidRange {
                range: edit.range.clone(),
                len: text.len(),
            };
            // Take back what the earlier edits of this batch did. They were
            // just applied, so reverting them cannot fail.
            let _ = apply(buffer, &changes, Direction::Backward);
            return Err(error);
        };
        let change = Change {
            at: edit.range.start,
            old: old.to_owned(),
            new: edit.text.clone(),
        };
        buffer.replace(edit.range.clone(), &edit.text);
        changes.push(change);
    }
    Ok(changes)
}

/// Applies `changes` in order (or, backward, undoes them last to first).
/// Either all are applied or, on error, none.
pub fn apply(
    buffer: &mut impl Buffer,
    changes: &[Change],
    direction: Direction,
) -> Result<(), EditError> {
    let ordered: Vec<&Change> = match direction {
        Direction::Forward => changes.iter().collect(),
        Direction::Backward => changes.iter().rev().collect(),
    };
    let opposite = match direction {
        Direction::Forward => Direction::Backward,
        Direction::Backward => Direction::Forward,
    };
    for (done, change) in ordered.iter().enumerate() {
        if let Err(error) = change.apply(buffer, direction) {
            for undone in ordered[..done].iter().rev() {
                // Each of these was just applied, so its other side is what
                // stands in the text now.
                let _ = undone.apply(buffer, opposite);
            }
            return Err(error);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_apply_in_sequence_and_record_what_they_replaced() {
        let mut text = String::from("hello world");
        let changes = perform(
            &mut text,
            &[
                Replace::new(0..5, "goodbye"),
                Replace::insert(13, "!"),
                Replace::delete(7..8),
            ],
        );
        assert_eq!(text, "goodbyeworld!");
        let changes = changes.unwrap_or_default();
        assert_eq!(
            changes[0],
            Change {
                at: 0,
                old: "hello".into(),
                new: "goodbye".into()
            }
        );
        assert_eq!(
            changes[2],
            Change {
                at: 7,
                old: " ".into(),
                new: String::new()
            }
        );

        assert_eq!(apply(&mut text, &changes, Direction::Backward), Ok(()));
        assert_eq!(text, "hello world");
        assert_eq!(apply(&mut text, &changes, Direction::Forward), Ok(()));
        assert_eq!(text, "goodbyeworld!");
    }

    #[test]
    fn an_invalid_edit_leaves_the_text_untouched() {
        let mut text = String::from("grüße");
        // Byte 3 is inside the two-byte `ü`.
        let cut = perform(&mut text, &[Replace::insert(0, "x"), Replace::delete(3..4)]);
        assert_eq!(
            cut,
            Err(EditError::InvalidRange {
                range: 3..4,
                len: 8
            })
        );
        assert_eq!(text, "grüße");
        let beyond = perform(&mut text, &[Replace::insert(99, "x")]);
        assert_eq!(
            beyond,
            Err(EditError::InvalidRange {
                range: 99..99,
                len: 7
            })
        );
        #[allow(
            clippy::reversed_empty_ranges,
            reason = "a caller's mistake, on purpose"
        )]
        let backwards = perform(&mut text, &[Replace::new(4..2, "x")]);
        assert!(backwards.is_err());
        assert_eq!(text, "grüße");
    }

    #[test]
    fn adjacent_changes_merge_without_changing_their_effect() {
        // (text before, first edit, second edit, whether they merge)
        let cases = [
            ("ab", Replace::insert(1, "x"), Replace::insert(2, "y"), true),
            (
                "ab",
                Replace::new(0..1, "xy"),
                Replace::insert(2, "z"),
                true,
            ),
            ("abcd", Replace::delete(1..2), Replace::delete(1..2), true),
            ("abcd", Replace::delete(2..3), Replace::delete(1..2), true),
            ("ab", Replace::insert(1, "xyz"), Replace::delete(3..4), true),
            ("ab", Replace::insert(1, "xyz"), Replace::delete(1..4), true),
            (
                "ab",
                Replace::insert(1, "x"),
                Replace::insert(0, "y"),
                false,
            ),
            ("abcd", Replace::delete(0..1), Replace::delete(2..3), false),
            (
                "ab",
                Replace::insert(1, "xyz"),
                Replace::delete(2..3),
                false,
            ),
            (
                "grüße",
                Replace::insert(2, "ü"),
                Replace::insert(4, "ß"),
                true,
            ),
        ];
        for (before, first, second, merges) in cases {
            let mut text = String::from(before);
            let changes = perform(&mut text, &[first.clone(), second.clone()]);
            let changes = changes.unwrap_or_else(|err| panic!("{err}"));
            let mut merged = changes[0].clone();
            assert_eq!(
                merged.absorb(&changes[1]),
                merges,
                "{first:?} then {second:?}"
            );
            if merges {
                let mut again = String::from(before);
                assert_eq!(
                    apply(&mut again, &[merged.clone()], Direction::Forward),
                    Ok(())
                );
                assert_eq!(again, text, "{first:?} then {second:?}");
                assert_eq!(apply(&mut again, &[merged], Direction::Backward), Ok(()));
                assert_eq!(again, before);
            }
        }
    }

    #[test]
    fn a_change_made_for_other_text_is_refused() {
        let mut text = String::from("abc");
        let changes = [
            Change {
                at: 0,
                old: "a".into(),
                new: "x".into(),
            },
            Change {
                at: 1,
                old: "WRONG".into(),
                new: "y".into(),
            },
        ];
        assert_eq!(
            apply(&mut text, &changes, Direction::Forward),
            Err(EditError::Mismatch { at: 1 })
        );
        // The first change was taken back.
        assert_eq!(text, "abc");
        assert_eq!(
            apply(&mut text, &changes[..1], Direction::Backward),
            Err(EditError::Mismatch { at: 0 })
        );
        assert_eq!(text, "abc");
    }
}
