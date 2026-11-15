//! Checking a source, and remembering what was found.
//!
//! A source is checked a paragraph at a time (see [`crate::prose::Unit`])
//! and what is found in a paragraph is kept by the paragraph's text. So
//! a source that was checked and then changed costs the checking of the
//! paragraphs that changed, and undoing a change costs nothing.
//!
//! Not all of a source is checked because it is there. A [`Job`] says
//! how far to go ([`Scope`]): what a window shows, as far as the next
//! thing found, or all of it. What an answer holds is everything known
//! about the source by then, which is more than was asked for wherever
//! paragraphs were checked before.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use typst_syntax::Source;

use crate::dictionary::{self, Speller};
use crate::languagetool::{self, LanguageTool};
use crate::personal::listed_in;
use crate::prose::{self, Prose, Unit};
use crate::words::{self, Lexicon};
use crate::{Finding, Kind, Settings};

/// How many jobs what was found in a paragraph is kept for after the
/// paragraph was last seen: long enough for a change to be undone.
const KEPT: u64 = 64;

/// How long grammar is checked before what is known so far is handed
/// on, so that a long check shows as it goes.
const REPORT_EVERY: Duration = Duration::from_millis(250);

/// For how many different words of a source suggestions are worked out
/// when a job asks for them: each takes milliseconds.
const SUGGESTED: usize = 200;
/// How many suggestions a word gets then.
const SUGGESTIONS: usize = 5;

/// How much of a source to check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// The paragraphs these bytes are in: what a window shows.
    Window(Range<usize>),
    /// From byte `from` on, or back from it, as far as the next thing
    /// found, around the end of the source if need be. Afterwards the
    /// nearest finding in that direction is the nearest there is.
    Next { from: usize, forward: bool },
    /// All of it.
    All,
}

/// A source to check.
#[derive(Debug, Clone)]
pub struct Job {
    /// Told again in every answer to this job.
    pub serial: u64,
    /// Which text this is, as the asker tells texts apart: a job for
    /// another text makes one that is under way pointless.
    pub text: u64,
    pub source: Source,
    /// The language it is written in, as documents name languages:
    /// `en`, `de`, `nl`, `en-US`.
    pub language: String,
    pub scope: Scope,
    /// Whether what is misspelt is to come with what may have been
    /// meant. That takes time; a window asks when the user does.
    pub suggest: bool,
}

/// An answer to a [`Job`].
#[derive(Debug, Clone, Default)]
pub struct Checked {
    /// The job's `serial` and `text`.
    pub serial: u64,
    pub text: u64,
    /// Everything known to be wrong with the source, in the order of
    /// the text.
    pub findings: Vec<Finding>,
    /// Whether all of the source has been checked.
    pub complete: bool,
    /// Whether this is the last answer to the job; the ones before it
    /// tell what is known so far.
    pub done: bool,
    /// The dictionary spelling is checked with, if it is checked.
    pub dictionary: Option<String>,
    /// What checks the grammar, if something does.
    pub grammar: Option<String>,
    /// Things to tell the user, each of them once: that there is no
    /// dictionary, that the server does not answer.
    pub notes: Vec<String>,
}

/// Something found, as it is kept: its place is a place in the key of
/// its paragraph.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Local {
    range: Range<usize>,
    kind: Kind,
    message: String,
    replacements: Vec<String>,
}

/// What is known about a paragraph. `None` is not looked at yet.
#[derive(Debug, Default)]
struct Entry {
    spelling: Option<Arc<[Local]>>,
    grammar: Option<Arc<[Local]>>,
    /// The job that saw the paragraph last.
    seen: u64,
}

/// Something a grammar checker found in a text: where, what is wrong,
/// and what to write instead.
type Slip = (Range<usize>, String, Vec<String>);

/// What checks grammar.
enum Grammar {
    Nothing,
    Tool {
        tool: LanguageTool,
        /// The language, as LanguageTool names it.
        code: String,
    },
    #[cfg(feature = "harper")]
    Harper(Box<crate::harper::Harper>),
}

impl Grammar {
    fn name(&self) -> Option<String> {
        match self {
            Self::Nothing => None,
            Self::Tool { tool, code } => {
                Some(format!("LanguageTool ({code}) at {}", tool.endpoint()))
            }
            #[cfg(feature = "harper")]
            Self::Harper(_) => Some("Harper".to_owned()),
        }
    }

    /// What it finds in `text`.
    fn check(&mut self, text: &str) -> Result<Vec<Slip>, String> {
        match self {
            Self::Nothing => Ok(Vec::new()),
            Self::Tool { tool, code } => {
                let found = tool.check(text, code)?;
                let found = found.into_iter();
                Ok(found
                    .map(|found| (found.range, found.message, found.replacements))
                    .collect())
            }
            #[cfg(feature = "harper")]
            Self::Harper(harper) => {
                let found = harper.check(text).into_iter();
                Ok(found
                    .map(|found| (found.range, found.message, found.replacements))
                    .collect())
            }
        }
    }
}

/// The words spelling goes by: a dictionary's and the user's own.
struct Known<'a> {
    speller: &'a Speller,
    own: &'a BTreeSet<String>,
}

impl Lexicon for Known<'_> {
    fn knows(&self, word: &str) -> bool {
        self.speller.knows(word) || listed_in(self.own, word)
    }
}

/// Checks sources and remembers what it found.
pub struct Checker {
    settings: Settings,
    /// The user's own words.
    own: Arc<BTreeSet<String>>,
    /// The language a dictionary was last looked for, and the one found.
    speller: Option<(String, Option<Arc<Speller>>)>,
    /// What grammar was last checked by, and for which language.
    grammar: (String, Grammar),
    /// What is known, by the keys of paragraphs.
    cache: HashMap<Box<str>, Entry>,
    /// How many jobs there have been.
    round: u64,
    /// What the user was told, so as not to tell it again.
    told: HashSet<String>,
    notes: Vec<String>,
}

impl Checker {
    /// A checker that goes by `settings`.
    #[must_use]
    pub fn new(settings: Settings) -> Self {
        Self {
            settings,
            own: Arc::default(),
            speller: None,
            grammar: (String::new(), Grammar::Nothing),
            cache: HashMap::new(),
            round: 0,
            told: HashSet::new(),
            notes: Vec::new(),
        }
    }

    /// Goes by `settings` from now on. What was found by the old ones is
    /// forgotten.
    pub fn configure(&mut self, settings: Settings) {
        if settings == self.settings {
            return;
        }
        *self = Self {
            own: Arc::clone(&self.own),
            round: self.round,
            ..Self::new(settings)
        };
    }

    /// Takes `own` for the user's own words from now on.
    pub fn set_words(&mut self, own: Arc<BTreeSet<String>>) {
        let taken = self.own.iter().any(|word| !own.contains(word));
        self.own = own;
        for (key, entry) in &mut self.cache {
            let Some(spelling) = &entry.spelling else {
                continue;
            };
            if taken {
                // A word that was right may be wrong now: look again.
                entry.spelling = None;
                continue;
            }
            // What was wrong and is one of the words now is right.
            let lexicon = |word: &str| listed_in(&self.own, word);
            let still = |local: &&Local| {
                let word = key.get(local.range.clone()).unwrap_or("");
                !words::unknown(&lexicon, word, &(0..word.len())).is_empty()
            };
            if !spelling.iter().all(|local| still(&local)) {
                entry.spelling = Some(spelling.iter().filter(still).cloned().collect());
            }
        }
    }

    /// The dictionary spelling is checked with, once a job has looked
    /// for one.
    #[must_use]
    pub fn speller(&self) -> Option<Arc<Speller>> {
        self.speller
            .as_ref()
            .and_then(|(_, speller)| speller.clone())
    }

    /// Tells the user `note`, unless they were told.
    fn tell(&mut self, note: String) {
        if self.told.insert(note.clone()) {
            self.notes.push(note);
        }
    }

    /// Makes the dictionary for `language` the one spelling goes by.
    fn find_speller(&mut self, language: &str) {
        if self.speller.as_ref().is_some_and(|(of, _)| of == language) {
            return;
        }
        for entry in self.cache.values_mut() {
            entry.spelling = None;
        }
        let wanted = dictionary::wanted(language);
        let package = dictionary::package(language);
        let Some(found) = dictionary::find(language, &self.settings.directories) else {
            self.tell(format!(
                "no dictionary for {language}: spelling is not checked \
                 (Arch: pacman -S {package}; or put {wanted}.aff and {wanted}.dic \
                 into ~/.local/share/zetwerk/dictionaries)"
            ));
            self.speller = Some((language.to_owned(), None));
            return;
        };
        if found.name != wanted && found.name != language.trim() {
            self.tell(format!(
                "no {wanted} dictionary (Arch: pacman -S {package}): \
                 spelling is checked with {}",
                found.name
            ));
        }
        let speller = match Speller::load(&found) {
            Ok(speller) => Some(Arc::new(speller)),
            Err(why) => {
                self.tell(why);
                None
            }
        };
        self.speller = Some((language.to_owned(), speller));
    }

    /// Makes what can check the grammar of `language` what grammar is
    /// checked by: the LanguageTool server, if the settings name one;
    /// Harper, for English, if not.
    fn find_grammar(&mut self, language: &str) {
        let wanted = dictionary::wanted(language);
        let by = match &self.settings.languagetool {
            Some(server) => format!("{} {wanted}", server.url),
            None => wanted.clone(),
        };
        if self.grammar.0 == by {
            return;
        }
        for entry in self.cache.values_mut() {
            entry.grammar = None;
        }
        let grammar = match &self.settings.languagetool {
            Some(server) => match LanguageTool::new(server) {
                Ok(tool) => Grammar::Tool {
                    tool,
                    code: languagetool::code(&wanted),
                },
                Err(why) => {
                    self.tell(format!("LanguageTool is not used: {why}"));
                    Grammar::Nothing
                }
            },
            #[cfg(feature = "harper")]
            None if wanted.starts_with("en_") => {
                Grammar::Harper(Box::new(crate::harper::Harper::new(&wanted)))
            }
            None => Grammar::Nothing,
        };
        self.grammar = (by, grammar);
    }

    /// Finds the dictionary and the grammar checker for `language`, if
    /// they are not those already gone by. [`Checker::check`] does this
    /// first; done before, [`Checker::speller`] is the one it will use.
    pub fn prepare(&mut self, language: &str) {
        if self.settings.spelling {
            self.find_speller(language);
        } else {
            self.speller = None;
        }
        if self.settings.grammar {
            self.find_grammar(language);
        } else {
            self.grammar = (String::new(), Grammar::Nothing);
        }
    }

    /// Checks what `job` asks for. Every answer goes to `report`, the
    /// last of them with `done` set. `stop` is asked between paragraphs
    /// whether to go on: if not, there is no last answer.
    pub fn check(&mut self, job: &Job, stop: &dyn Fn() -> bool, report: &mut dyn FnMut(Checked)) {
        self.round += 1;
        self.prepare(&job.language);
        let text = job.source.text();
        let units = prose::units(job.source.root());
        let keys: Vec<Cow<str>> = units.iter().map(|unit| unit.key(text)).collect();
        for key in &keys {
            if let Some(entry) = self.cache.get_mut(key.as_ref()) {
                entry.seen = self.round;
            } else {
                let entry = Entry {
                    seen: self.round,
                    ..Entry::default()
                };
                self.cache.insert(key.as_ref().into(), entry);
            }
        }
        let mut run = Run {
            checker: self,
            job,
            units: &units,
            keys: &keys,
            stop,
            report,
            reported: Instant::now(),
            down: false,
        };
        let finished = match &job.scope {
            Scope::Window(window) => {
                let shown =
                    |unit: &Unit| unit.range.start < window.end && unit.range.end > window.start;
                let order: Vec<usize> = (0..units.len()).filter(|at| shown(&units[*at])).collect();
                run.in_passes(&order)
            }
            Scope::All => run.in_passes(&(0..units.len()).collect::<Vec<usize>>()),
            Scope::Next { from, forward } => run.as_far_as_found(*from, *forward),
        };
        if finished {
            run.answer(true);
        }
        let round = self.round;
        self.cache.retain(|_, entry| round - entry.seen <= KEPT);
    }
}

/// One job being done.
struct Run<'a> {
    checker: &'a mut Checker,
    job: &'a Job,
    units: &'a [Unit<'a>],
    keys: &'a [Cow<'a, str>],
    stop: &'a dyn Fn() -> bool,
    report: &'a mut dyn FnMut(Checked),
    /// When an answer was last given.
    reported: Instant,
    /// Whether what checks grammar has failed: it is not asked again
    /// for this job.
    down: bool,
}

impl Run<'_> {
    /// Whether spelling is checked, and grammar.
    fn checks(&self) -> (bool, bool) {
        let spelling = self.checker.speller().is_some();
        let grammar = !matches!(self.checker.grammar.1, Grammar::Nothing) && !self.down;
        (spelling, grammar)
    }

    /// Checks the spelling of unit `at`, unless it is known. Returns
    /// whether it was checked.
    fn spell(&mut self, at: usize, prose: &mut Option<Prose>) -> bool {
        let Some(speller) = self.checker.speller() else {
            return false;
        };
        let key = self.keys[at].as_ref();
        if self
            .checker
            .cache
            .get(key)
            .is_none_or(|entry| entry.spelling.is_some())
        {
            return false;
        }
        let unit = &self.units[at];
        let prose = prose.get_or_insert_with(|| unit.prose());
        let lexicon = Known {
            speller: &speller,
            own: &self.checker.own,
        };
        let text = prose.text();
        let mut found = Vec::new();
        for word in words::words(text).filter(|word| !prose.stands_in(word)) {
            for wrong in words::unknown(&lexicon, text, &word) {
                if let Some(source) = prose.source(&wrong) {
                    found.push(Local {
                        range: unit.in_key(source.start)..unit.in_key(source.end),
                        kind: Kind::Spelling,
                        message: String::new(),
                        replacements: Vec::new(),
                    });
                }
            }
        }
        if let Some(entry) = self.checker.cache.get_mut(key) {
            entry.spelling = Some(found.into());
        }
        true
    }

    /// Checks the grammar of unit `at`, unless it is known. Returns
    /// whether it was checked.
    fn grammar(&mut self, at: usize, prose: &mut Option<Prose>) -> bool {
        let key = self.keys[at].as_ref();
        let known = |entry: &Entry| entry.grammar.is_some();
        if !self.checks().1 || self.checker.cache.get(key).is_none_or(known) {
            return false;
        }
        let unit = &self.units[at];
        let prose = prose.get_or_insert_with(|| unit.prose());
        let text = prose.text();
        // A sentence has words; what has none is a figure, a rule.
        let read = words::words(text).filter(|word| !prose.stands_in(word));
        let found = if read.take(2).count() < 2 {
            Vec::new()
        } else {
            match self.checker.grammar.1.check(text) {
                Ok(found) => found,
                Err(why) => {
                    let by = self.checker.grammar.1.name().unwrap_or_default();
                    self.checker.tell(format!(
                        "{by} does not answer ({why}): grammar is not checked"
                    ));
                    self.down = true;
                    return false;
                }
            }
        };
        let found = found
            .into_iter()
            .filter(|(range, _, _)| !prose.stands_in(range));
        let found = found.filter_map(|(range, message, replacements)| {
            let source = prose.source(&range)?;
            Some(Local {
                range: unit.in_key(source.start)..unit.in_key(source.end),
                kind: Kind::Grammar,
                message,
                replacements,
            })
        });
        let found: Arc<[Local]> = found.collect();
        if let Some(entry) = self.checker.cache.get_mut(key) {
            entry.grammar = Some(found);
        }
        true
    }

    /// Checks the units `order` lists: the spelling of all of them
    /// first, which is quick and is told at once, then the grammar.
    /// Returns whether it got through.
    fn in_passes(&mut self, order: &[usize]) -> bool {
        let mut spelt = false;
        for at in order {
            if (self.stop)() {
                return false;
            }
            spelt |= self.spell(*at, &mut None);
        }
        let waits = |at: &usize| {
            let entry = self.checker.cache.get(self.keys[*at].as_ref());
            entry.is_some_and(|entry| entry.grammar.is_none())
        };
        if spelt && self.checks().1 && order.iter().any(waits) {
            self.answer(false);
        }
        for at in order {
            if (self.stop)() {
                return false;
            }
            if self.grammar(*at, &mut None) && self.reported.elapsed() >= REPORT_EVERY {
                self.answer(false);
            }
        }
        true
    }

    /// Where the nearest thing found in unit `at` is: the first that
    /// begins behind `from`, or the last that begins before it.
    fn nearest(&self, at: usize, from: Option<usize>, forward: bool) -> Option<usize> {
        let entry = self.checker.cache.get(self.keys[at].as_ref())?;
        let unit = &self.units[at];
        let both = entry.spelling.iter().chain(&entry.grammar);
        let starts =
            both.flat_map(|found| found.iter().map(|local| unit.in_source(local.range.start)));
        if forward {
            starts
                .filter(|start| from.is_none_or(|from| *start > from))
                .min()
        } else {
            starts
                .filter(|start| from.is_none_or(|from| *start < from))
                .max()
        }
    }

    /// Checks from byte `from` on (or back from it) until what is next
    /// is known. Returns whether it got through.
    fn as_far_as_found(&mut self, from: usize, forward: bool) -> bool {
        let units = self.units;
        let mut order: Vec<usize> = (0..units.len()).collect();
        if forward {
            order.sort_by_key(|at| units[*at].range.start);
        } else {
            order.sort_by_key(|at| std::cmp::Reverse(units[*at].range.end));
        }
        // What lies ahead first; then, around the end, the rest.
        let ahead = |at: &usize| {
            if forward {
                units[*at].range.end > from
            } else {
                units[*at].range.start < from
            }
        };
        let first: Vec<usize> = order.iter().copied().filter(ahead).collect();
        let around: Vec<usize> = order.iter().copied().filter(|at| !ahead(at)).collect();
        for (lap, bound) in [(first, Some(from)), (around, None)] {
            let mut best: Option<usize> = None;
            for at in lap {
                // Nothing in a unit that lies behind what was found is
                // nearer than that.
                let behind = best.is_some_and(|best| {
                    if forward {
                        units[at].range.start >= best
                    } else {
                        units[at].range.end <= best
                    }
                });
                if behind {
                    break;
                }
                if (self.stop)() {
                    return false;
                }
                let mut prose = None;
                self.spell(at, &mut prose);
                self.grammar(at, &mut prose);
                let nearest = self.nearest(at, bound, forward);
                best = match (best, nearest) {
                    (Some(best), Some(nearest)) if forward => Some(best.min(nearest)),
                    (Some(best), Some(nearest)) => Some(best.max(nearest)),
                    (best, nearest) => best.or(nearest),
                };
            }
            if best.is_some() {
                break;
            }
        }
        true
    }

    /// Hands on what is known by now.
    fn answer(&mut self, done: bool) {
        let (spelling, grammar) = self.checks();
        let mut findings = Vec::new();
        let mut complete = true;
        for (unit, key) in self.units.iter().zip(self.keys) {
            let Some(entry) = self.checker.cache.get(key.as_ref()) else {
                continue;
            };
            complete &=
                (entry.spelling.is_some() || !spelling) && (entry.grammar.is_some() || !grammar);
            let both = entry.spelling.iter().chain(&entry.grammar);
            findings.extend(both.flat_map(|found| found.iter()).map(|local| Finding {
                range: unit.in_source(local.range.start)..unit.in_source(local.range.end),
                kind: local.kind,
                message: local.message.clone(),
                replacements: local.replacements.clone(),
            }));
        }
        findings.sort_by_key(|finding| (finding.range.start, finding.range.end));
        if let (true, Some(speller)) = (self.job.suggest, self.checker.speller()) {
            suggest(&speller, self.job.source.text(), &mut findings);
        }
        self.reported = Instant::now();
        (self.report)(Checked {
            serial: self.job.serial,
            text: self.job.text,
            findings,
            complete,
            done,
            dictionary: self
                .checker
                .speller()
                .map(|speller| speller.name().to_owned()),
            grammar: self.checker.grammar.1.name().filter(|_| !self.down),
            notes: std::mem::take(&mut self.checker.notes),
        });
    }
}

/// Gives what is misspelt in `text` what may have been meant.
fn suggest(speller: &Speller, text: &str, findings: &mut [Finding]) {
    let mut meant: HashMap<&str, Vec<String>> = HashMap::new();
    for finding in findings
        .iter_mut()
        .filter(|finding| finding.kind == Kind::Spelling)
    {
        let Some(word) = text.get(finding.range.clone()) else {
            continue;
        };
        if !meant.contains_key(word) && meant.len() >= SUGGESTED {
            continue;
        }
        let suggested = meant
            .entry(word)
            .or_insert_with(|| speller.suggest(word, SUGGESTIONS));
        finding.replacements.clone_from(suggested);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::Server;
    use crate::dictionary::tests::{fixtures, scratch};
    use crate::languagetool::tests::serve;

    /// Settings that check spelling with the dictionaries kept for tests.
    fn spelling() -> Settings {
        Settings {
            spelling: true,
            directories: vec![fixtures()],
            ..Settings::default()
        }
    }

    fn job(text: &str, language: &str, scope: Scope) -> Job {
        Job {
            serial: 7,
            text: 1,
            source: Source::detached(text),
            language: language.to_owned(),
            scope,
            suggest: false,
        }
    }

    /// The last answer to `job`.
    fn checked(checker: &mut Checker, job: &Job) -> Checked {
        let mut last = None;
        checker.check(job, &|| false, &mut |answer| last = Some(answer));
        let last = last.unwrap();
        assert!(last.done);
        last
    }

    /// What is found in all of `text`: the words each finding is about.
    fn found(checker: &mut Checker, text: &str, language: &str) -> Vec<String> {
        let answer = checked(checker, &job(text, language, Scope::All));
        assert!(answer.complete);
        let about = |finding: &Finding| text[finding.range.clone()].to_owned();
        answer.findings.iter().map(about).collect()
    }

    #[test]
    fn misspelt_words_of_running_text_are_found_where_they_are_written() {
        let mut checker = Checker::new(spelling());
        let text = "= A Tset <tset>\n\
                    This is a *wrnog* word, and #emph[anothr].\n\n\
                    #let helo = \"wrold\"\n\
                    The cat's box is $x_1 + helo$ and `wrold` in @tset, see https://wrold.example.\n\n\
                    #figure(image(\"wrold.png\"), caption: [A good exampel.]) <fig-wrold>\n\n\
                    ```\nwrold of code\n```\n";
        assert_eq!(
            found(&mut checker, text, "en"),
            ["Tset", "wrnog", "anothr", "exampel"]
        );
        // Nothing else is known about it.
        let answer = checked(&mut checker, &job(text, "en", Scope::All));
        assert_eq!(answer.dictionary.as_deref(), Some("en_GB"));
        assert!(
            answer.notes.is_empty() && answer.findings.iter().all(|f| f.kind == Kind::Spelling)
        );
    }

    #[test]
    fn the_three_languages_are_checked_by_their_dictionaries() {
        let mut checker = Checker::new(spelling());
        assert_eq!(
            found(
                &mut checker,
                "The colour is good, the color is wrnog.",
                "en"
            ),
            ["color", "wrnog"]
        );
        let german =
            "Das Haus und die Haustür sind hier. Die Strasse ist nicht kurz, das haus ist falsch.";
        assert_eq!(found(&mut checker, german, "de"), ["Strasse", "haus"]);
        let dutch = "Dit is het woordenboek. IJs is goed, Ijsland is fout, de fietsen en de huizen niet, de fiest wel.";
        assert_eq!(found(&mut checker, dutch, "nl"), ["Ijsland", "fiest"]);
        // The dictionary a document's language means is the one asked.
        assert_eq!(found(&mut checker, "Das Haus ist good.", "de-DE"), ["good"]);
        assert_eq!(
            found(&mut checker, "Das Haus ist good.", "en-GB"),
            ["Das", "Haus", "ist"]
        );
    }

    #[test]
    fn a_missing_dictionary_is_told_once_with_the_package_that_brings_it() {
        let mut checker = Checker::new(Settings {
            directories: vec![scratch("none")],
            ..spelling()
        });
        for (language, package, wanted) in [
            ("en", "hunspell-en_gb", "en_GB"),
            ("de", "hunspell-de", "de_DE"),
            ("nl", "hunspell-nl", "nl_NL"),
        ] {
            let first = checked(&mut checker, &job("Some wrnog text.", language, Scope::All));
            assert_eq!(
                (first.findings.len(), first.dictionary.as_deref()),
                (0, None)
            );
            assert_eq!(first.notes.len(), 1);
            assert!(
                first.notes[0].contains(package) && first.notes[0].contains(wanted),
                "{:?}",
                first.notes
            );
            let again = checked(&mut checker, &job("Some othr text.", language, Scope::All));
            assert!(again.notes.is_empty() && again.complete);
        }
    }

    #[test]
    fn english_without_a_british_dictionary_is_american_and_says_so() {
        let directory = scratch("american");
        std::fs::copy(fixtures().join("en_GB.aff"), directory.join("en_US.aff")).unwrap();
        std::fs::copy(fixtures().join("en_GB.dic"), directory.join("en_US.dic")).unwrap();
        let mut checker = Checker::new(Settings {
            directories: vec![directory.clone()],
            ..spelling()
        });
        let first = checked(&mut checker, &job("A wrnog word.", "en", Scope::All));
        assert_eq!(first.dictionary.as_deref(), Some("en_US"));
        assert_eq!(first.findings.len(), 1);
        assert_eq!(first.notes.len(), 1, "{:?}", first.notes);
        assert!(first.notes[0].contains("en_GB") && first.notes[0].contains("hunspell-en_gb"));
        assert!(first.notes[0].contains("en_US"));
        // Asked for by name, it is what was wanted.
        let mut checker = Checker::new(Settings {
            directories: vec![directory.clone()],
            ..spelling()
        });
        let named = checked(&mut checker, &job("A word.", "en-US", Scope::All));
        assert_eq!(named.notes, Vec::<String>::new());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn only_paragraphs_that_changed_are_checked_again() {
        let mut checker = Checker::new(spelling());
        let text = "One wrnog word.\n\nA good paragraph.\n\nTwo mroe wrods.";
        assert_eq!(found(&mut checker, text, "en"), ["wrnog", "mroe", "wrods"]);
        // What is known is kept by the text of the paragraph. Were a
        // paragraph looked at again, this would be forgotten.
        let planted = Local {
            range: 2..6,
            kind: Kind::Spelling,
            message: "planted".to_owned(),
            replacements: Vec::new(),
        };
        checker.cache.get_mut("A good paragraph.").unwrap().spelling = Some([planted].into());
        let changed =
            "One right word.\n\nNew here: a wrnog one.\n\nA good paragraph.\n\nTwo mroe wrods.";
        assert_eq!(
            found(&mut checker, changed, "en"),
            ["wrnog", "good", "mroe", "wrods"]
        );
        // Undoing the change costs nothing: the old paragraph is known.
        checker.cache.get_mut("One wrnog word.").unwrap().spelling = Some([].into());
        assert_eq!(found(&mut checker, text, "en"), ["good", "mroe", "wrods"]);
        // Paragraphs long gone are forgotten.
        for _ in 0..=KEPT {
            found(&mut checker, "Another text.", "en");
        }
        assert_eq!(checker.cache.len(), 1);
    }

    #[test]
    fn a_window_is_checked_and_the_rest_is_not() {
        let mut checker = Checker::new(spelling());
        let text = "Frist paragraph.\n\nSecnod paragraph.\n\nThrid paragraph.";
        let second = text.find("Secnod").unwrap();
        let window = checked(
            &mut checker,
            &job(text, "en", Scope::Window(second + 2..second + 3)),
        );
        assert!(!window.complete);
        assert_eq!(window.findings.len(), 1);
        assert_eq!(&text[window.findings[0].range.clone()], "Secnod");
        // What was checked before is told with whatever is asked next.
        let third = text.find("Thrid").unwrap();
        let next = checked(
            &mut checker,
            &job(text, "en", Scope::Window(third..text.len())),
        );
        assert_eq!((next.findings.len(), next.complete), (2, false));
        let all = checked(&mut checker, &job(text, "en", Scope::All));
        assert_eq!((all.findings.len(), all.complete), (3, true));
        // And a window of a text that is known whole is whole.
        let again = checked(&mut checker, &job(text, "en", Scope::Window(0..1)));
        assert!(again.complete);
    }

    #[test]
    fn the_next_finding_is_looked_for_no_further_than_it_is() {
        let text = "Good one.\n\nA wrnog one.\n\nGood two.\n\nA secnod one.\n\nGood three.";
        let places = |checked: &Checked| -> Vec<String> {
            let about = |finding: &Finding| text[finding.range.clone()].to_owned();
            checked.findings.iter().map(about).collect()
        };
        let mut checker = Checker::new(spelling());
        let from = text.find("Good two").unwrap();
        let forward = Scope::Next {
            from,
            forward: true,
        };
        let next = checked(&mut checker, &job(text, "en", forward));
        assert_eq!(
            (places(&next), next.complete),
            (vec!["secnod".to_owned()], false)
        );
        // Back from the same place.
        let mut checker = Checker::new(spelling());
        let back = Scope::Next {
            from,
            forward: false,
        };
        let next = checked(&mut checker, &job(text, "en", back));
        assert_eq!(
            (places(&next), next.complete),
            (vec!["wrnog".to_owned()], false)
        );
        // Around the end of the text, where nothing lies ahead.
        let mut checker = Checker::new(spelling());
        let from = text.find("Good three").unwrap();
        let next = checked(
            &mut checker,
            &job(
                text,
                "en",
                Scope::Next {
                    from,
                    forward: true,
                },
            ),
        );
        assert_eq!(places(&next), ["wrnog"]);
        // From a finding on, the next is another.
        let mut checker = Checker::new(spelling());
        let from = text.find("wrnog").unwrap();
        let next = checked(
            &mut checker,
            &job(
                text,
                "en",
                Scope::Next {
                    from,
                    forward: true,
                },
            ),
        );
        assert!(places(&next).contains(&"secnod".to_owned()));
        // Where nothing is wrong, all of it was looked at.
        let mut checker = Checker::new(spelling());
        let clean = checked(
            &mut checker,
            &job(
                "Good.\n\nWell.",
                "en",
                Scope::Next {
                    from: 0,
                    forward: true,
                },
            ),
        );
        assert!(clean.complete && clean.findings.is_empty());
    }

    #[test]
    fn the_users_own_words_are_words() {
        let mut checker = Checker::new(spelling());
        let text = "Typst is good. Typst's paper is zetwerk.\n\nA wrnog Zetwerk.";
        assert_eq!(
            found(&mut checker, text, "en"),
            ["Typst", "Typst's", "zetwerk", "wrnog", "Zetwerk"]
        );
        let own = |words: &[&str]| Arc::new(words.iter().map(|word| (*word).to_owned()).collect());
        checker.set_words(own(&["Typst", "zetwerk"]));
        assert_eq!(found(&mut checker, text, "en"), ["wrnog"]);
        // Taken back, they are wrong again.
        checker.set_words(own(&["zetwerk"]));
        assert_eq!(
            found(&mut checker, text, "en"),
            ["Typst", "Typst's", "wrnog"]
        );
    }

    #[test]
    fn suggestions_come_when_they_are_asked_for() {
        let mut checker = Checker::new(spelling());
        let mut asked = job("He said helo and helo.", "en", Scope::All);
        assert!(
            checked(&mut checker, &asked)
                .findings
                .iter()
                .all(|f| f.replacements.is_empty())
        );
        asked.suggest = true;
        let with = checked(&mut checker, &asked);
        assert_eq!(with.findings.len(), 2);
        assert!(
            with.findings
                .iter()
                .all(|f| f.replacements.contains(&"hello".to_owned()))
        );
    }

    #[test]
    fn a_job_that_is_stopped_gives_no_last_answer_and_loses_nothing() {
        let mut checker = Checker::new(spelling());
        let text = "A wrnog one.\n\nA secnod one.";
        let asked = Cell::new(0);
        let stop = || {
            asked.set(asked.get() + 1);
            asked.get() > 1
        };
        let mut answers = 0;
        checker.check(&job(text, "en", Scope::All), &stop, &mut |_| answers += 1);
        assert_eq!(answers, 0);
        // What was checked before it was stopped is known.
        let window = checked(&mut checker, &job(text, "en", Scope::Window(0..0)));
        assert_eq!(window.findings.len(), 1);
    }

    #[test]
    fn switched_off_nothing_is_checked() {
        let mut checker = Checker::new(Settings::default());
        let off = checked(&mut checker, &job("A wrnog word.", "en", Scope::All));
        assert!(off.findings.is_empty() && off.complete && off.notes.is_empty());
        assert_eq!((off.dictionary, off.grammar), (None, None));
        checker.configure(spelling());
        assert_eq!(found(&mut checker, "A wrnog word.", "en"), ["wrnog"]);
    }

    /// Settings that check grammar by the server at `url` and nothing
    /// else.
    fn by_server(url: &str) -> Settings {
        Settings {
            grammar: true,
            languagetool: Some(Server {
                url: url.to_owned(),
                remote: false,
            }),
            ..Settings::default()
        }
    }

    /// A LanguageTool answer that finds something at `offset`.
    fn one_match(offset: usize, length: usize, message: &str, instead: &str) -> String {
        serde_json::json!({ "matches": [{
            "message": message, "offset": offset, "length": length,
            "replacements": [{ "value": instead }],
            "rule": { "id": "SOME_RULE" },
        }]})
        .to_string()
    }

    #[test]
    fn a_languagetool_server_checks_grammar_in_the_three_languages() {
        // The server finds the second word of whatever it is sent.
        let (address, requests) = serve("200 OK", one_match(4, 5, "Hier fehlt etwas.", "Katze,"));
        let mut checker = Checker::new(by_server(&address));
        let text = "Die Katze die $x$ mag schläft.\n\n#image(\"a.png\")";
        let german = checked(&mut checker, &job(text, "de", Scope::All));
        assert_eq!(german.findings.len(), 1);
        let finding = &german.findings[0];
        assert_eq!(
            (&text[finding.range.clone()], finding.kind),
            ("Katze", Kind::Grammar)
        );
        assert_eq!(
            (finding.message.as_str(), finding.replacements.as_slice()),
            ("Hier fehlt etwas.", ["Katze,".to_owned()].as_slice())
        );
        assert!(
            german
                .grammar
                .as_deref()
                .is_some_and(|by| by.contains("de-DE")),
            "{:?}",
            german.grammar
        );
        let asked = requests.recv().unwrap();
        assert!(asked.contains("language=de-DE"), "{asked}");
        // What was sent is what is read: no markup, a word for the equation.
        assert!(
            asked.contains("text=Die+Katze+die+Dummy1+mag+schl%C3%A4ft."),
            "{asked}"
        );
        // The paragraph without a sentence in it was not sent.
        assert!(requests.try_recv().is_err());

        checked(&mut checker, &job("Een korte zin.", "nl", Scope::All));
        assert!(requests.recv().unwrap().ends_with("language=nl"));
        checked(&mut checker, &job("A short sentence.", "en", Scope::All));
        let english = requests.recv().unwrap();
        assert!(english.contains("language=en-GB"), "{english}");
        // A paragraph that was checked is not sent again.
        checked(
            &mut checker,
            &job("A short sentence.\n\nAnother short one.", "en", Scope::All),
        );
        assert!(requests.recv().unwrap().contains("Another"));
        assert!(requests.try_recv().is_err());
    }

    #[test]
    fn dutch_is_sent_to_the_server_as_dutch() {
        let (address, requests) = serve("200 OK", "{\"matches\": []}".to_owned());
        let mut checker = Checker::new(by_server(&address));
        let dutch = checked(
            &mut checker,
            &job("Dit is een korte zin.", "nl", Scope::All),
        );
        assert!(dutch.findings.is_empty() && dutch.complete);
        let asked = requests.recv().unwrap();
        assert!(
            asked.contains("language=nl") && !asked.contains("language=nl-"),
            "{asked}"
        );
    }

    #[test]
    fn what_the_server_finds_about_a_stand_in_is_not_told() {
        // "Dummy1" of "Let Dummy1 be": the server knows no better.
        let (address, _requests) = serve("200 OK", one_match(4, 6, "Unknown word.", "Dummy"));
        let mut checker = Checker::new(by_server(&address));
        let answer = checked(&mut checker, &job("Let $x$ be a number.", "en", Scope::All));
        assert!(answer.findings.is_empty() && answer.complete);
    }

    #[test]
    fn a_server_that_does_not_answer_is_told_once_and_spelling_goes_on() {
        let unused = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", listener.local_addr().unwrap())
        };
        let mut checker = Checker::new(Settings {
            spelling: true,
            directories: vec![fixtures()],
            ..by_server(&unused)
        });
        let text = "Das ist ein Fehlr hier.\n\nUnd das ist ein Satz.";
        let first = checked(&mut checker, &job(text, "de", Scope::All));
        assert_eq!(first.findings.len(), 1);
        assert_eq!(first.notes.len(), 1, "{:?}", first.notes);
        assert!(
            first.notes[0].contains("does not answer"),
            "{:?}",
            first.notes
        );
        assert!(first.complete && first.grammar.is_none());
        let again = checked(&mut checker, &job(text, "de", Scope::All));
        assert!(again.notes.is_empty() && again.findings.len() == 1);
    }

    #[test]
    fn a_server_elsewhere_is_not_asked() {
        let mut checker = Checker::new(by_server("https://api.languagetool.org"));
        let answer = checked(&mut checker, &job("This are a sentence.", "en", Scope::All));
        assert_eq!(answer.notes.len(), 1);
        assert!(
            answer.notes[0].contains("not this machine"),
            "{:?}",
            answer.notes
        );
        assert!(answer.findings.is_empty() && answer.grammar.is_none() && answer.complete);
    }

    #[cfg(feature = "harper")]
    #[test]
    fn english_grammar_is_checked_without_a_server_and_german_is_not() {
        let mut checker = Checker::new(Settings {
            grammar: true,
            ..spelling()
        });
        let text = "This is an *test* of the the checker, with $x$ in it.\n\nA good paragraph.";
        let english = checked(&mut checker, &job(text, "en", Scope::All));
        let told: Vec<(&str, Kind)> = english
            .findings
            .iter()
            .map(|f| (&text[f.range.clone()], f.kind))
            .collect();
        assert_eq!(told, [("an", Kind::Grammar), ("the the", Kind::Grammar)]);
        assert_eq!(english.findings[0].replacements, ["a"]);
        assert_eq!(english.grammar.as_deref(), Some("Harper"));
        let german = checked(
            &mut checker,
            &job("Das ist ein ein Haus.", "de", Scope::All),
        );
        assert!(german.findings.is_empty() && german.grammar.is_none() && german.complete);
    }
}
