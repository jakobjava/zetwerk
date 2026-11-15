//! English grammar without a server, by the rules of `harper-core`.
//!
//! Harper is a checker of English only. It knows the slips that rules
//! can know (an article that does not fit, a word twice, `its` for
//! `it's`, a verb that does not agree with its pronoun) and does not
//! read a sentence as LanguageTool does. Its own spelling checker is
//! not asked: spelling is the dictionary's.

use std::ops::Range;
use std::sync::Arc;

use harper_core::linting::{LintGroup, LintKind, Linter, Suggestion};
use harper_core::spell::FstDictionary;
use harper_core::{Dialect, Document};

/// Something Harper found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// The bytes of the text it is about.
    pub range: Range<usize>,
    pub message: String,
    /// What could be written instead; nothing at all where the words
    /// are to go.
    pub replacements: Vec<String>,
}

/// Harper, set up for one kind of English.
pub struct Harper {
    linter: LintGroup,
    dictionary: Arc<FstDictionary>,
}

/// The English of the dictionary `wanted` (`en_GB`): British, unless
/// it is another country's.
fn dialect(wanted: &str) -> Dialect {
    match wanted.split_once('_').map_or("", |(_, region)| region) {
        "US" => Dialect::American,
        "AU" => Dialect::Australian,
        "CA" => Dialect::Canadian,
        "IN" => Dialect::Indian,
        _ => Dialect::British,
    }
}

impl Harper {
    /// Harper for the English of the dictionary `wanted`. Setting it up
    /// takes some hundred milliseconds.
    #[must_use]
    pub fn new(wanted: &str) -> Self {
        let dictionary = FstDictionary::curated();
        let mut linter = LintGroup::new_curated(Arc::clone(&dictionary), dialect(wanted));
        // Not asked: what a word is (that is the dictionary's), whether an
        // unknown word is two (it is a term more often), whether a
        // paragraph begins with a capital (one that goes on a sentence
        // after an equation set apart does not), and whether `min` is
        // short for a minute (in a paper it is a minimum).
        let unasked = [
            "SpellCheck",
            "SplitWords",
            "SentenceCapitalization",
            "ExpandTimeShorthands",
        ];
        for rule in unasked {
            linter.config.set_rule_enabled(rule, false);
        }
        Self { linter, dictionary }
    }

    /// What Harper finds in `text`.
    pub fn check(&mut self, text: &str) -> Vec<Match> {
        let document = Document::new_plain_english(text, &*self.dictionary);
        // Harper counts characters.
        let mut bytes: Vec<usize> = text.char_indices().map(|(at, _)| at).collect();
        bytes.push(text.len());
        let lints = self.linter.lint(&document);
        let kept = lints.into_iter().filter(|lint| {
            // How a sentence reads is the writer's business, and
            // spelling the dictionary's.
            !matches!(
                lint.lint_kind,
                LintKind::Spelling
                    | LintKind::Readability
                    | LintKind::Style
                    | LintKind::Enhancement
            )
        });
        let mut found: Vec<Match> = kept
            .filter_map(|lint| {
                let range = *bytes.get(lint.span.start)?..*bytes.get(lint.span.end)?;
                let written = text.get(range.clone())?;
                let replacements = lint.suggestions.iter().map(|suggestion| match suggestion {
                    Suggestion::ReplaceWith(with) => with.iter().collect(),
                    Suggestion::InsertAfter(more) => {
                        written.chars().chain(more.iter().copied()).collect()
                    }
                    Suggestion::Remove => String::new(),
                });
                Some(Match {
                    range,
                    message: lint.message,
                    replacements: replacements.collect(),
                })
            })
            .collect();
        found.sort_by_key(|found| (found.range.start, found.range.end));
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `harper` finds in `text`: the words each finding is about,
    /// and the first thing to write instead.
    fn found(harper: &mut Harper, text: &str) -> Vec<(String, String)> {
        let found = harper.check(text);
        let told = found.into_iter().map(|found| {
            let first = found.replacements.first().cloned().unwrap_or_default();
            (text[found.range].to_owned(), first)
        });
        told.collect()
    }

    #[test]
    fn slips_that_rules_know_are_found_with_what_to_write() {
        let mut harper = Harper::new("en_GB");
        let pair = |about: &str, instead: &str| (about.to_owned(), instead.to_owned());
        assert_eq!(
            found(&mut harper, "This is an test of the the checker."),
            [pair("an", "a"), pair("the the", "the")]
        );
        assert_eq!(
            found(&mut harper, "We shows that it holds, and its a long way."),
            [pair("shows", "show"), pair("its", "it's")]
        );
        // Characters are counted, bytes are given.
        assert_eq!(
            found(&mut harper, "Gödel’s proof is an thing of beauty."),
            [pair("an", "a")]
        );
    }

    #[test]
    fn good_english_and_what_stands_for_equations_is_let_be() {
        let mut harper = Harper::new("en_GB");
        for text in [
            "Averaging schemes appear throughout numerical analysis.",
            "Let Dummy1 be real numbers and fix a weight Dummy2 with Dummy3.",
            "This is an Ina1-gon and a Dummy2-gon.",
            "The colour of its neighbours is grey, and the centre is analysed.",
            "Hte speling is not Harper's to judge.",
            "by definition,\n\nDummy1\n\nand each difference on the right is bounded.",
            "The normed space has an adjoint and a min-max description.",
        ] {
            assert_eq!(found(&mut harper, text), [], "{text}");
        }
    }
}
