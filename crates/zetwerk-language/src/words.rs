//! The words of running text, and which of them a dictionary does not
//! know.
//!
//! A word is letters, with the apostrophes and hyphens between them.
//! What a dictionary cannot be asked about is let be: anything with a
//! digit in it, single letters, and words all in capitals, which are
//! names of things more often than they are words.

use std::ops::Range;

/// Something that knows words.
pub trait Lexicon {
    /// Whether `word` is one.
    fn knows(&self, word: &str) -> bool;
}

impl<F: Fn(&str) -> bool> Lexicon for F {
    fn knows(&self, word: &str) -> bool {
        self(word)
    }
}

/// A hyphen that is only there where a line breaks.
const SOFT_HYPHEN: char = '\u{ad}';

/// Whether `c` is part of a word wherever it stands.
fn in_word(c: char) -> bool {
    c.is_alphanumeric() || c == SOFT_HYPHEN || ('\u{300}'..='\u{36f}').contains(&c)
}

/// Whether `c` is part of a word where it stands between two parts of
/// one.
fn joins(c: char) -> bool {
    matches!(c, '\'' | '’' | '-')
}

/// The words of `text`: the bytes each is.
pub fn words(text: &str) -> impl Iterator<Item = Range<usize>> + '_ {
    let mut chars = text.char_indices().peekable();
    std::iter::from_fn(move || {
        let (start, first) = chars.by_ref().find(|(_, c)| in_word(*c))?;
        let mut end = start + first.len_utf8();
        while let Some((at, c)) = chars.peek().copied() {
            let goes_on = in_word(c)
                || (joins(c)
                    && text[at + c.len_utf8()..]
                        .chars()
                        .next()
                        .is_some_and(in_word));
            if !goes_on {
                break;
            }
            end = at + c.len_utf8();
            chars.next();
        }
        Some(start..end)
    })
}

/// Whether `word` is one a dictionary can be asked about.
fn is_checked(word: &str) -> bool {
    let mut letters = word.chars().filter(|c| c.is_alphabetic());
    let (Some(_), Some(_)) = (letters.next(), letters.next()) else {
        return false;
    };
    let digit = word.chars().any(char::is_numeric);
    let shouted = !word.chars().any(char::is_lowercase);
    !digit && !shouted
}

/// Whether `lexicon` knows `word`, as it is or as what it is a form of:
/// with the full stop of an abbreviation, if one follows it, or without
/// an `'s`.
fn known(lexicon: &impl Lexicon, word: &str, stop_follows: bool) -> bool {
    if !is_checked(word) {
        return true;
    }
    // As dictionaries write it.
    let plain: String = word
        .chars()
        .filter(|c| *c != SOFT_HYPHEN)
        .map(|c| if c == '’' { '\'' } else { c })
        .collect();
    if lexicon.knows(&plain) {
        return true;
    }
    if stop_follows && lexicon.knows(&format!("{plain}.")) {
        return true;
    }
    // Dutch begins a sentence with both letters of its IJ in capitals,
    // which to a dictionary is a word in mixed case.
    if let Some(rest) = plain.strip_prefix("IJ")
        && rest.chars().all(char::is_lowercase)
        && lexicon.knows(&format!("ij{rest}"))
    {
        return true;
    }
    let owner = plain.strip_suffix("'s").or_else(|| plain.strip_suffix("'"));
    owner.is_some_and(|owner| !is_checked(owner) || lexicon.knows(owner))
}

/// The parts of the word at bytes `range` of `text` that `lexicon` does
/// not know: all of it, or of a word put together with hyphens, the
/// parts that are no words.
pub fn unknown(lexicon: &impl Lexicon, text: &str, range: &Range<usize>) -> Vec<Range<usize>> {
    let Some(word) = text.get(range.clone()) else {
        return Vec::new();
    };
    let stop_follows = text[range.end..].starts_with('.');
    if known(lexicon, word, stop_follows) {
        return Vec::new();
    }
    if !word.contains('-') {
        return vec![range.clone()];
    }
    let mut found = Vec::new();
    let mut at = range.start;
    for part in word.split('-') {
        let end = at + part.len();
        if !known(lexicon, part, stop_follows && end == range.end) {
            found.push(at..end);
        }
        at = end + 1;
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(text: &str) -> Vec<&str> {
        words(text).map(|range| &text[range]).collect()
    }

    #[test]
    fn words_are_letters_and_what_joins_them() {
        assert_eq!(split("Hello, world!"), ["Hello", "world"]);
        assert_eq!(
            split("don't 'quoted' dogs' it’s"),
            ["don't", "quoted", "dogs", "it’s"]
        );
        assert_eq!(
            split("well-known – a dash - here"),
            ["well-known", "a", "dash", "here"]
        );
        assert_eq!(split("Straße über naïve"), ["Straße", "über", "naïve"]);
        assert_eq!(split("x1 and 3D, 42"), ["x1", "and", "3D", "42"]);
        assert_eq!(split("soft\u{ad}hyphen"), ["soft\u{ad}hyphen"]);
        assert_eq!(split("  "), Vec::<&str>::new());
    }

    /// The unknown parts of the words of `text`, for a dictionary of
    /// `known` words.
    fn wrong<'a>(text: &'a str, known: &[&str]) -> Vec<&'a str> {
        let lexicon = |word: &str| known.contains(&word);
        words(text)
            .flat_map(|range| unknown(&lexicon, text, &range))
            .map(|range| &text[range])
            .collect()
    }

    #[test]
    fn what_no_dictionary_can_know_is_let_be() {
        assert_eq!(wrong("a b x1 3D NASA H2O", &[]), Vec::<&str>::new());
        assert_eq!(wrong("teh cat", &["cat"]), ["teh"]);
    }

    #[test]
    fn forms_of_known_words_are_known() {
        assert_eq!(wrong("etc. and etc", &["etc.", "and"]), ["etc"]);
        assert_eq!(
            wrong("Typst's dogs' it’s", &["Typst", "dogs", "it's"]),
            Vec::<&str>::new()
        );
        assert_eq!(
            wrong("soft\u{ad}hyphen", &["softhyphen"]),
            Vec::<&str>::new()
        );
        // Dutch: IJs begins a sentence about ijs; Ijs begins none.
        assert_eq!(wrong("IJs Ijs auto's", &["ijs", "auto"]), ["Ijs"]);
    }

    #[test]
    fn of_a_hyphenated_word_the_unknown_parts_are_wrong() {
        assert_eq!(wrong("well-known", &["well", "known"]), Vec::<&str>::new());
        assert_eq!(wrong("well-knwon fact", &["well", "fact"]), ["knwon"]);
        assert_eq!(wrong("E-Mail", &["E-Mail"]), Vec::<&str>::new());
        assert_eq!(wrong("x-axis", &["axis"]), Vec::<&str>::new());
        assert_eq!(wrong("abc-def", &[]), ["abc", "def"]);
    }
}
