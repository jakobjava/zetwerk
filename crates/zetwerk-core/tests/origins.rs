//! Where the text came from, against a model.
//!
//! Random edits by random origins, undo, redo, jumps, saves, changes on
//! disk, crashes (whole, torn and corrupted journals) and thinning. The
//! model keeps, for every state of the history, the attribution its text
//! had when the document was last in it. Whatever happens after (going
//! away and back, a crash and a reopen, the journal written anew,
//! thinning), the attribution of a state that is still there is the one
//! the model has: the attribution of a text never changes by the history
//! being kept. And what an edit puts in is credited to whoever made it,
//! but for the words it only kept.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a failed step should fail the case"
)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use proptest::prelude::*;
use zetwerk_core::{Agent, By, Credited, Document, Kind, Opened, Origin, Replace, StateId};

/// The origins edits are made by.
fn origin(who: u8) -> Origin {
    let agent = |name: &str| Agent {
        name: name.to_owned(),
        version: "1.0".to_owned(),
        session: "s".to_owned(),
    };
    match who % 6 {
        0 => Origin::Author,
        1 => Origin::Agent(agent("Claude Code")),
        2 => Origin::Accepted(agent("Claude Code")),
        3 => Origin::Agent(agent("Codex")),
        4 => Origin::Pasted,
        _ => Origin::Zetwerk("a tidy-up".to_owned()),
    }
}

/// A place in a list of any length, as `proptest::sample::Index` is, but
/// one a test can write down.
#[derive(Debug, Clone, Copy)]
struct Index(usize);

impl Index {
    fn index(self, size: usize) -> usize {
        ((size as u128 * self.0 as u128) >> usize::BITS) as usize
    }
}

fn index() -> impl Strategy<Value = Index> {
    any::<usize>().prop_map(Index)
}

#[derive(Debug, Clone)]
enum Op {
    Edit {
        at: Index,
        len: usize,
        text: String,
        join: bool,
        who: u8,
    },
    Undo,
    Redo,
    Goto(Index),
    Save,
    Reload,
    ExternalWrite(String),
    Crash,
    TornCrash(Index),
    Thin(usize),
}

fn text() -> impl Strategy<Value = String> {
    "[ab é\n]{0,5}"
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        12 => (index(), 0..4_usize, text(), any::<bool>(), any::<u8>())
            .prop_map(|(at, len, text, join, who)| Op::Edit { at, len, text, join, who }),
        4 => Just(Op::Undo),
        3 => Just(Op::Redo),
        2 => index().prop_map(Op::Goto),
        2 => Just(Op::Save),
        1 => Just(Op::Reload),
        1 => text().prop_map(Op::ExternalWrite),
        2 => Just(Op::Crash),
        1 => index().prop_map(Op::TornCrash),
        1 => (0..6_usize).prop_map(Op::Thin),
    ]
}

/// The attribution of a text: per byte, the origin of who wrote it and
/// of whose text it took the place of.
type Credits = Vec<(Origin, Option<Origin>)>;

fn credits_of(document: &Document<String>) -> Credits {
    let origins = document.origins();
    let mut credits = Vec::new();
    for (range, mark) in origins.map().iter() {
        let Credited { by, over } = origins.credited(mark);
        credits.extend(std::iter::repeat_n(
            (by.origin, over.map(|over| over.origin)),
            range.len(),
        ));
    }
    credits
}

struct Session {
    root: PathBuf,
    document: Option<Document<String>>,
    /// By state: its text and its attribution as last seen.
    states: Vec<(String, Option<Credits>)>,
    file: Option<String>,
    /// How long the journal was when it was last written anew: a crash
    /// can only cut off what was added to it since (it is written anew
    /// at once, see `Journal::create`).
    compacted: usize,
}

static CASE: AtomicUsize = AtomicUsize::new(0);

impl Session {
    fn new(initial: &str) -> Self {
        let case = CASE.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("zetwerk-origins-{}-{case}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("work")).unwrap();
        std::fs::write(root.join("work/doc.typ"), initial).unwrap();
        let mut session = Self {
            root,
            document: None,
            states: Vec::new(),
            file: Some(initial.to_owned()),
            compacted: 0,
        };
        let (document, opened) = session.open();
        assert_eq!(opened, Opened::Clean);
        // What was in the file before is of no known origin.
        let credits = credits_of(&document);
        assert!(credits.iter().all(|(by, _)| *by == Origin::Unknown));
        session.states.push((initial.to_owned(), Some(credits)));
        session.document = Some(document);
        session
    }

    fn file(&self) -> PathBuf {
        self.root.join("work/doc.typ")
    }

    fn open(&mut self) -> (Document<String>, Opened) {
        let opened = Document::open(&self.file(), &self.root.join("state"), |text| text).unwrap();
        self.compacted = std::fs::read(self.journal()).map_or(0, |bytes| bytes.len());
        opened
    }

    fn journal(&self) -> PathBuf {
        std::fs::read_dir(self.root.join("state/journals"))
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|ext| ext == "journal"))
            .unwrap()
    }

    fn doc(&mut self) -> &mut Document<String> {
        self.document.as_mut().unwrap()
    }

    fn state(&mut self) -> usize {
        self.doc().state() as usize
    }

    fn apply(&mut self, op: Op) {
        match op {
            Op::Edit {
                at,
                len,
                text,
                join,
                who,
            } => self.edit(at, len, &text, join, &origin(who)),
            Op::Undo => {
                self.doc().undo().unwrap();
            }
            Op::Redo => {
                self.doc().redo().unwrap();
            }
            Op::Goto(index) => {
                let state = index.index(self.states.len()) as StateId;
                self.doc().goto(state).unwrap();
            }
            Op::Save => {
                if self.doc().save(true).is_ok() {
                    self.file = Some(self.doc().text().to_owned());
                }
            }
            Op::Reload => self.reload(),
            Op::ExternalWrite(text) => {
                std::fs::write(self.file(), &text).unwrap();
                self.file = Some(text);
            }
            Op::Crash => self.crash(),
            Op::TornCrash(cut) => self.torn(cut),
            Op::Thin(recent) => self.thin(recent),
        }
        self.check();
    }

    fn edit(&mut self, at: Index, len: usize, new: &str, join: bool, origin: &Origin) {
        let before = self.doc().text().to_owned();
        let old_credits = credits_of(self.doc());
        let boundaries: Vec<usize> = before
            .char_indices()
            .map(|(i, _)| i)
            .chain([before.len()])
            .collect();
        let first = at.index(boundaries.len());
        let last = (first + len).min(boundaries.len() - 1);
        let range = boundaries[first]..boundaries[last];
        let states = self.doc().history().len();
        let by = By::Origin(origin);
        self.doc()
            .edit_by(&[Replace::new(range.clone(), new)], join, &by)
            .unwrap();
        let text = self.doc().text().to_owned();
        let credits = credits_of(self.doc());
        assert_eq!(credits.len(), text.len(), "the attribution covers the text");
        // Before and behind the edit nothing changed.
        assert_eq!(credits[..range.start], old_credits[..range.start]);
        assert_eq!(
            credits[range.start + new.len()..],
            old_credits[range.end..],
            "behind the edit"
        );
        // What it put in is the origin's, or kept from what was there.
        let put = &credits[range.start..range.start + new.len()];
        let was = &old_credits[range.clone()];
        for credit in put {
            assert!(
                credit.0 == *origin || was.contains(credit),
                "{credit:?} put in by {origin:?} over {was:?}"
            );
        }
        if range.is_empty() {
            assert!(put.iter().all(|credit| credit.0 == *origin));
        }
        let state = self.state();
        if self.doc().history().len() > states {
            assert_eq!(state, self.states.len());
            self.states.push((text, Some(credits)));
        } else {
            self.states[state] = (text, Some(credits));
        }
    }

    fn reload(&mut self) {
        let old = credits_of(self.doc());
        let states = self.doc().history().len();
        if self.doc().reload().is_err() {
            return;
        }
        let text = self.doc().text().to_owned();
        let credits = credits_of(self.doc());
        assert_eq!(credits.len(), text.len());
        if self.doc().history().len() > states {
            // Taken in from the disk: outside, or kept.
            for credit in &credits {
                assert!(credit.0 == Origin::Outside || old.contains(credit));
            }
            self.states.push((text, Some(credits)));
        }
    }

    /// Ends the session without saving and opens the file again.
    fn crash(&mut self) {
        self.document = None;
        let states = self.states.len();
        let (document, opened) = self.open();
        self.document = Some(document);
        if !matches!(opened, Opened::Clean | Opened::Restored) {
            // The file's text on top: from outside.
            assert_eq!(self.doc().history().len(), states + 1);
            let text = self.doc().text().to_owned();
            let credits = credits_of(self.doc());
            assert!(credits.iter().all(|credit| {
                credit.0 == Origin::Outside
                    || credit.0 != Origin::Author
                    || self.states.iter().any(|(_, known)| {
                        known.as_ref().is_some_and(|known| known.contains(credit))
                    })
            }));
            self.states.push((text, Some(credits)));
        }
    }

    /// Ends the session, cuts the journal short anywhere, and opens again:
    /// what comes back has whatever attribution it had, as far as it
    /// came back; what does not is forgotten.
    fn torn(&mut self, cut: Index) {
        let appended = self.appended();
        self.torn_at(cut.index(appended + 1));
    }

    /// How many bytes were appended to the journal since it was last
    /// written whole.
    fn appended(&self) -> usize {
        std::fs::metadata(self.journal())
            .map_or(0, |meta| meta.len() as usize)
            .saturating_sub(self.compacted)
    }

    /// As [`Session::torn`], `cut` bytes of what was appended kept.
    fn torn_at(&mut self, cut: usize) {
        self.document = None;
        let journal = self.journal();
        let mut bytes = std::fs::read(&journal).unwrap();
        bytes.truncate(self.compacted + cut);
        std::fs::write(&journal, &bytes).unwrap();
        let (document, _) = self.open();
        self.document = Some(document);
        // The states that came back are those that were, as far as they
        // go; their attribution is checked as the model has it.
        let len = self.doc().history().len();
        let current = self.doc().state();
        let mut states = Vec::new();
        for state in 0..len as StateId {
            self.doc().goto(state).unwrap();
            let text = self.doc().text().to_owned();
            let credits = credits_of(self.doc());
            assert_eq!(credits.len(), text.len());
            // A state the journal kept has the credits it had. One the
            // tear lost, whose text the file still had, came back from
            // there: made from outside on what the journal still had, so
            // what is unchanged keeps the credits it had there (as a
            // reload does), and only what differs is from outside.
            if let Some((was, Some(known))) = self.states.get(state as usize)
                && *was == text
                && *known != credits
            {
                for credit in &credits {
                    assert!(
                        known.contains(credit)
                            || matches!(credit.0, Origin::Outside | Origin::Unknown)
                            || self.states.iter().any(|(_, earlier)| {
                                earlier
                                    .as_ref()
                                    .is_some_and(|earlier| earlier.contains(credit))
                            }),
                        "{credit:?} after a torn journal, in state {state}"
                    );
                }
            }
            states.push((text, Some(credits)));
        }
        self.doc().goto(current).unwrap();
        self.states = states;
    }

    fn thin(&mut self, recent: usize) {
        let map = self.doc().thin(recent).unwrap();
        let mut states = vec![(String::new(), None); self.doc().history().len()];
        for (old, new) in map.iter().enumerate() {
            if let Some(new) = new {
                states[*new as usize].clone_from(&self.states[old]);
            }
        }
        self.states = states;
        self.crash();
    }

    fn check(&mut self) {
        let state = self.state();
        assert_eq!(self.doc().history().len(), self.states.len());
        let text = self.doc().text().to_owned();
        let credits = credits_of(self.doc());
        assert_eq!(
            text, self.states[state].0,
            "state {state} has the wrong text"
        );
        assert_eq!(credits.len(), text.len(), "the attribution covers the text");
        match &self.states[state].1 {
            Some(known) => assert_eq!(&credits, known, "state {state} credited otherwise"),
            None => self.states[state].1 = Some(credits),
        }
    }

    /// Every state, gone to, has the attribution it had.
    fn finish(mut self) {
        for state in 0..self.states.len() {
            self.doc().goto(state as StateId).unwrap();
            self.check();
        }
        self.crash();
        self.check();
        for state in (0..self.states.len()).rev() {
            self.doc().goto(state as StateId).unwrap();
            self.check();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.document = None;
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 400, ..ProptestConfig::default() })]

    #[test]
    fn the_attribution_of_a_text_never_changes(
        initial in text(),
        ops in proptest::collection::vec(op(), 0..40),
    ) {
        let mut session = Session::new(&initial);
        for op in ops {
            session.apply(op);
        }
        session.finish();
    }
}

/// Found by the property above (shrinking did not make it smaller).
#[test]
fn a_journal_torn_after_thinning_and_a_reload_keeps_the_credits() {
    let edit = |at: usize, len: usize, text: &str, join: bool, who: u8| Op::Edit {
        at: Index(at),
        len,
        text: text.to_owned(),
        join,
        who,
    };
    let ops = vec![
        edit(1_772_220_509_173_992_490, 2, "a\n", false, 228),
        edit(12_474_476_973_200_381_487, 0, "é", false, 95),
        edit(4_861_046_410_066_696_186, 1, "\nb ", true, 196),
        Op::Undo,
        Op::Crash,
        edit(16_457_741_024_356_015_075, 2, "b\nééb", false, 148),
        Op::Goto(Index(3_183_613_960_617_413_387)),
        edit(9_097_938_876_569_886_560, 2, "é", false, 127),
        Op::Save,
        Op::Thin(1),
        Op::Undo,
        Op::Save,
        Op::Redo,
        Op::Goto(Index(4_978_849_722_382_169_286)),
        Op::Redo,
        edit(1_666_559_089_877_734_644, 3, "\n  é\n", true, 209),
        edit(3_222_045_563_838_744_435, 2, "\nb ", true, 134),
        edit(14_233_461_027_875_061_158, 0, "\nb ", false, 166),
        edit(6_426_233_525_176_502_367, 3, " b\nb", true, 201),
        Op::Goto(Index(8_553_365_817_175_221_044)),
        Op::Undo,
        Op::ExternalWrite("\nb\né ".to_owned()),
        Op::Reload,
        edit(10_552_685_980_059_647_330, 2, "éaa\n\n", false, 108),
        edit(3_553_409_794_745_213_218, 3, "", false, 124),
        edit(1_978_494_229_444_479_524, 2, "", false, 169),
        Op::TornCrash(Index(10_369_457_656_408_705_370)),
    ];
    let mut session = Session::new("");
    for op in ops {
        session.apply(op);
    }
    session.finish();
}

/// Text an agent wrote and the author then changed a word of: that word
/// is the author's over the agent's, the rest stays the agent's; undo
/// brings the agent's word back as the agent's, and so does a reopen.
#[test]
fn a_word_the_author_changed_in_an_agents_sentence() {
    let mut session = Session::new("");
    let agent = origin(1);
    let sentence = "The cat sat on the mat.";
    session
        .doc()
        .edit_by(&[Replace::insert(0, sentence)], false, &By::Origin(&agent))
        .unwrap();
    // `cw` on "cat": deleted, then typed letter by letter, one step.
    session.doc().edit(&[Replace::delete(4..7)], false).unwrap();
    for (at, letter) in [(4, "d"), (5, "o"), (6, "g")] {
        session
            .doc()
            .edit(&[Replace::insert(at, letter)], true)
            .unwrap();
    }
    let kinds = |session: &mut Session| -> Vec<Kind> {
        let origins = session.doc().origins();
        origins
            .map()
            .iter()
            .flat_map(|(range, mark)| std::iter::repeat_n(origins.kind(mark), range.len()))
            .collect()
    };
    let now = kinds(&mut session);
    assert_eq!(&now[..4], &[Kind::Agent; 4]);
    assert_eq!(&now[4..7], &[Kind::Edited; 3]);
    assert_eq!(&now[7..], &[Kind::Agent; 16]);
    // An agent rewriting the sentence with one word different credits
    // only that word to itself.
    let rewritten = "The dog sat on the rug.";
    session
        .doc()
        .edit_by(
            &[Replace::new(0..sentence.len(), rewritten)],
            false,
            &By::Origin(&origin(3)),
        )
        .unwrap();
    let after = kinds(&mut session);
    assert_eq!(&after[4..7], &[Kind::Edited; 3], "the author's word stays");
    let credits = credits_of(session.doc());
    assert_eq!(
        credits[19].0,
        origin(3),
        "the new word is the other agent's"
    );
    assert_eq!(credits[0].0, agent);
    session.doc().undo().unwrap();
    session.doc().undo().unwrap();
    assert_eq!(kinds(&mut session), vec![Kind::Agent; sentence.len()]);
    session.crash();
    assert_eq!(kinds(&mut session), vec![Kind::Agent; sentence.len()]);
    session.doc().redo().unwrap();
    assert_eq!(&kinds(&mut session)[4..7], &[Kind::Edited; 3]);
}

/// A file changed on disk in two places far apart is credited to the
/// outside in those places only.
#[test]
fn a_file_changed_on_disk_is_outside_only_where_it_changed() {
    let mut session = Session::new("");
    let text = "one two three four five six seven eight nine ten\n";
    session
        .doc()
        .edit_by(&[Replace::insert(0, text)], false, &By::Origin(&origin(1)))
        .unwrap();
    session.doc().save(false).unwrap();
    std::fs::write(
        session.file(),
        "one 2 three four five six seven eight nine 10\n",
    )
    .unwrap();
    session.doc().reload().unwrap();
    let credits = credits_of(session.doc());
    assert_eq!(credits[4].0, Origin::Outside);
    assert_eq!(credits[6].0, origin(1), "three stays the agent's");
    assert_eq!(credits[43].0, Origin::Outside);
    assert_eq!(credits[40].0, origin(1));
}

/// What keeping origins costs a key on a long text whose attribution is
/// in many pieces: typed letters, in an agent's text and in the author's.
/// `cargo test --release --workspace --test origins what_a_key_costs -- --ignored --nocapture`
#[test]
#[ignore = "a measurement: prints the time a key takes"]
fn what_a_key_costs_on_a_long_text() {
    let mut session = Session::new("");
    // About 3 MB, as 350 copies of a corpus article are, in 40 000
    // stretches by turns of the author and of agents.
    let paragraph = "Some words of a paragraph, typed by someone or other. ";
    let mut at = 0;
    for stretch in 0..40_000_u32 {
        let by = By::Origin(&origin((stretch % 4) as u8));
        let text = paragraph.repeat(1 + stretch as usize % 2);
        session
            .doc()
            .edit_by(&[Replace::insert(at, text.as_str())], false, &by)
            .unwrap();
        at += text.len();
    }
    let len = session.doc().text().len();
    let started = std::time::Instant::now();
    let keys = 2000;
    for key in 0..keys {
        let place = len / 2 + key as usize;
        session
            .doc()
            .edit(&[Replace::insert(place, "x")], key > 0)
            .unwrap();
    }
    let per_key = started.elapsed().as_secs_f64() * 1e6 / f64::from(keys);
    println!(
        "{len} bytes in {} runs: {per_key:.1} µs a key, journal and all",
        session.doc().origins().map().iter().count()
    );
}
