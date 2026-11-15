//! Random sessions against a model.
//!
//! Each case runs a random sequence of edits, undos, saves, crashes and
//! outside interference against a real document on disk, and after every
//! step checks it against a model that only knows what the text of each
//! state should be and what the file should hold.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a failed step should fail the case"
)]

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use proptest::prelude::*;
use proptest::sample::Index;
use zetwerk_core::{Change, Direction, DiskError, Document, Opened, ROOT, Replace, StateId, Step};

#[derive(Debug, Clone)]
enum Op {
    /// Replace `len` characters at a position with `text`.
    Edit {
        at: Index,
        len: usize,
        text: String,
        join: bool,
    },
    /// An edit whose range is not in the text.
    BadEdit,
    Undo,
    Redo,
    Goto(Index),
    /// Make a state the one redo leads to from the state before it.
    Prefer(Index),
    Save {
        overwrite: bool,
    },
    Reload,
    /// Another program writes the file.
    ExternalWrite(String),
    ExternalDelete,
    /// The process dies and the file is opened again.
    Crash,
    /// The process dies while writing the journal: the journal is cut off
    /// at an arbitrary byte.
    TornCrash(Index),
    /// The process dies and the disk returns the journal with one byte
    /// changed.
    CorruptCrash(Index, u8),
    /// The history is thinned, keeping this many of the newest states.
    Thin(usize),
}

fn text() -> impl Strategy<Value = String> {
    // Few distinct characters, so that edits collide; multi-byte ones, so
    // that byte offsets and characters differ.
    "[ab é\n]{0,4}"
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        10 => (any::<Index>(), 0..4_usize, text(), any::<bool>())
            .prop_map(|(at, len, text, join)| Op::Edit { at, len, text, join }),
        1 => Just(Op::BadEdit),
        4 => Just(Op::Undo),
        3 => Just(Op::Redo),
        2 => any::<Index>().prop_map(Op::Goto),
        2 => any::<Index>().prop_map(Op::Prefer),
        2 => any::<bool>().prop_map(|overwrite| Op::Save { overwrite }),
        1 => Just(Op::Reload),
        1 => text().prop_map(Op::ExternalWrite),
        1 => Just(Op::ExternalDelete),
        2 => Just(Op::Crash),
        1 => any::<Index>().prop_map(Op::TornCrash),
        1 => (any::<Index>(), 1..=255_u8).prop_map(|(at, flip)| Op::CorruptCrash(at, flip)),
        1 => (0..6_usize).prop_map(Op::Thin),
    ]
}

/// What the test expects, independently of the document.
struct Model {
    /// The text of every state, by state id.
    states: Vec<String>,
    /// What the file holds; `None` if it does not exist.
    file: Option<String>,
    /// What the file held when the document last read or wrote it.
    seen: Option<String>,
    /// Every text the document or the file has held: after a torn journal,
    /// whatever comes back must be one of these.
    ever: HashSet<String>,
}

struct Session {
    root: PathBuf,
    document: Option<Document<String>>,
    model: Model,
}

static CASE: AtomicUsize = AtomicUsize::new(0);

impl Session {
    fn new(initial: Option<String>) -> Self {
        let case = CASE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("zetwerk-prop-{}-{case}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("work")).unwrap();
        let mut session = Self {
            root,
            document: None,
            model: Model {
                states: vec![initial.clone().unwrap_or_default()],
                file: None,
                seen: None,
                ever: HashSet::from([String::new()]),
            },
        };
        if let Some(text) = initial {
            session.write_file(Some(&text));
        }
        let (document, opened) = session.open();
        assert_eq!(opened, Opened::Clean);
        session.document = Some(document);
        session.model.seen = session.model.file.clone();
        session
    }

    fn file(&self) -> PathBuf {
        self.root.join("work").join("doc.typ")
    }

    fn journal(&self) -> PathBuf {
        std::fs::read_dir(self.root.join("state").join("journals"))
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|ext| ext == "journal"))
            .unwrap()
    }

    fn open(&self) -> (Document<String>, Opened) {
        Document::open(&self.file(), &self.root.join("state"), |text| text).unwrap()
    }

    fn write_file(&mut self, text: Option<&str>) {
        match text {
            Some(text) => {
                std::fs::write(self.file(), text).unwrap();
                self.model.ever.insert(text.to_owned());
            }
            None => {
                let _ = std::fs::remove_file(self.file());
            }
        }
        self.model.file = text.map(str::to_owned);
    }

    fn doc(&mut self) -> &mut Document<String> {
        self.document.as_mut().unwrap()
    }

    fn text(&self) -> String {
        self.document.as_ref().unwrap().text().to_owned()
    }

    fn apply(&mut self, op: Op) {
        match op {
            Op::Edit {
                at,
                len,
                text,
                join,
            } => self.edit(at, len, &text, join),
            Op::BadEdit => {
                let (before, revision) = (self.text(), self.doc().revision());
                let beyond = before.len() + 1;
                assert!(
                    self.doc()
                        .edit(&[Replace::insert(beyond, "x")], false)
                        .is_err()
                );
                assert_eq!((self.text(), self.doc().revision()), (before, revision));
            }
            Op::Undo => {
                let from = self.doc().state();
                let parent = self.doc().history().parent(from);
                assert_eq!(self.doc().undo().unwrap(), parent);
                if parent.is_some() {
                    // Redo leads straight back.
                    let state = self.doc().state();
                    assert_eq!(self.doc().history().redo_target(state), Some(from));
                }
            }
            Op::Redo => {
                let from = self.doc().state();
                let reached = self.doc().redo().unwrap();
                if let Some(reached) = reached {
                    assert_eq!(self.doc().history().parent(reached), Some(from));
                }
            }
            Op::Goto(index) => {
                let state = index.index(self.model.states.len()) as StateId;
                self.doc().goto(state).unwrap();
                assert_eq!(self.doc().state(), state);
            }
            Op::Prefer(index) => {
                let state = index.index(self.model.states.len()) as StateId;
                let (text, revision) = (self.text(), self.doc().revision());
                let parent = self.doc().history().parent(state);
                assert_eq!(self.doc().prefer(state).unwrap(), parent.is_some());
                if let Some(parent) = parent {
                    assert_eq!(self.doc().history().redo_target(parent), Some(state));
                }
                // Where redo leads is all that changed.
                assert_eq!((self.text(), self.doc().revision()), (text, revision));
            }
            Op::Save { overwrite } => self.save(overwrite),
            Op::Reload => self.reload(),
            Op::ExternalWrite(text) => self.write_file(Some(&text)),
            Op::ExternalDelete => self.write_file(None),
            Op::Crash => self.crash(),
            Op::TornCrash(cut) => self.damaged_crash(|bytes| {
                bytes.truncate(cut.index(bytes.len() + 1));
            }),
            Op::CorruptCrash(at, flip) => self.damaged_crash(|bytes| {
                let at = at.index(bytes.len());
                bytes[at] ^= flip;
            }),
            Op::Thin(recent) => self.thin(recent),
        }
        self.check();
    }

    fn edit(&mut self, at: Index, len: usize, new: &str, join: bool) {
        let before = self.text();
        let boundaries: Vec<usize> = before
            .char_indices()
            .map(|(i, _)| i)
            .chain([before.len()])
            .collect();
        let first = at.index(boundaries.len());
        let last = (first + len).min(boundaries.len() - 1);
        let range = boundaries[first]..boundaries[last];
        let mut expected = before.clone();
        expected.replace_range(range.clone(), new);

        let states_before = self.doc().history().len();
        let state = self.doc().edit(&[Replace::new(range, new)], join).unwrap();
        if self.doc().history().len() == states_before {
            assert!(join, "an edit that was not to be joined made no new state");
            self.model.states[state as usize] = expected;
        } else {
            assert_eq!(state as usize, self.model.states.len());
            self.model.states.push(expected);
        }
    }

    fn save(&mut self, overwrite: bool) {
        let text = self.text();
        let foreign = self.model.file.is_some() && self.model.file != self.model.seen;
        let result = self.doc().save(overwrite);
        if foreign && !overwrite {
            assert!(
                matches!(result, Err(DiskError::ChangedOnDisk)),
                "{result:?}"
            );
        } else {
            result.unwrap();
            self.model.file = Some(text.clone());
            self.model.seen = Some(text);
            assert!(!self.doc().is_modified());
        }
    }

    fn reload(&mut self) {
        let result = self.doc().reload();
        match self.model.file.clone() {
            None => assert!(matches!(result, Err(DiskError::Missing)), "{result:?}"),
            Some(file) => {
                result.unwrap();
                if self.doc().history().len() > self.model.states.len() {
                    self.model.states.push(file.clone());
                }
                self.model.seen = Some(file);
                assert!(!self.doc().is_modified());
            }
        }
    }

    /// Thins the history: the states that stay keep their texts, the text
    /// and whether it is saved stay as they are, and so does the history
    /// after a crash.
    fn thin(&mut self, recent: usize) {
        let (text, modified) = (self.text(), self.doc().is_modified());
        let map = self.doc().thin(recent).unwrap();
        let mut states = vec![String::new(); self.doc().history().len()];
        for (old, new) in map.iter().enumerate() {
            if let Some(new) = new {
                states[*new as usize] = self.model.states[old].clone();
            }
        }
        self.model.states = states;
        assert_eq!(self.text(), text);
        assert_eq!(self.doc().is_modified(), modified);
        // The journal is thinned too: a crash brings back the same.
        self.crash();
    }

    /// Ends the session without saving and opens the file again.
    fn crash(&mut self) {
        let before = self.text();
        let was_modified = self.doc().is_modified();
        let history = self.doc().history().clone();
        self.document = None;
        let (document, opened) = self.open();

        let file = self.model.file.clone();
        let came_back = match (&file, &opened) {
            (Some(file), Opened::Clean) => *file == before,
            (None, Opened::Clean) => before.is_empty(),
            (Some(file), Opened::Restored) => *file != before && self.model.states.contains(file),
            (None, Opened::Restored) => !before.is_empty(),
            _ => false,
        };
        if came_back {
            assert_eq!(document.text(), before);
            assert_eq!(
                document.history(),
                &history,
                "the undo tree survives a crash"
            );
        } else {
            // The file holds something this history never produced.
            let file = file.expect("a missing file never conflicts");
            assert!(
                !self.model.states.contains(&file),
                "{opened:?} for a known text"
            );
            match &opened {
                Opened::Conflict { recovered, journal } => {
                    assert!(was_modified, "a conflict needs unsaved work");
                    assert_eq!(*recovered, before, "the unsaved text is handed over");
                    assert!(journal.exists());
                }
                Opened::ChangedWhileClosed => assert!(!was_modified, "unsaved work was dropped"),
                other => panic!("unexpected outcome {other:?}"),
            }
            assert_eq!(document.text(), file);
            // The file's text is a new state on top of the history: what
            // the document held is one undo away.
            assert_eq!(document.history().len(), history.len() + 1);
            assert_eq!(
                document.text_at(history.current()).as_deref(),
                Some(before.as_str()),
                "undo leads back to the text as it was"
            );
            self.model.states.push(file);
        }
        self.model.seen = self.model.file.clone();
        self.document = Some(document);
    }

    /// Ends the session, damages the journal, and opens again.
    fn damaged_crash(&mut self, damage: impl FnOnce(&mut Vec<u8>)) {
        self.document = None;
        let journal = self.journal();
        let mut bytes = std::fs::read(&journal).unwrap();
        damage(&mut bytes);
        std::fs::write(&journal, &bytes).unwrap();

        let (mut document, opened) = self.open();
        if let Opened::Conflict { recovered, .. } = &opened {
            assert!(
                self.model.ever.contains(recovered),
                "recovered text {recovered:?} never existed"
            );
        }
        // Whatever part of the history survived, every state in it is one
        // the document really was in.
        let current = document.state();
        let states: Vec<String> = (0..document.history().len() as StateId)
            .map(|state| {
                document.goto(state).unwrap();
                document.text().to_owned()
            })
            .collect();
        document.goto(current).unwrap();
        for state in &states {
            assert!(
                self.model.ever.contains(state),
                "state text {state:?} never existed"
            );
        }
        self.model.states = states;
        self.model.seen = self.model.file.clone();
        self.document = Some(document);
    }

    /// What must hold after every step.
    fn check(&mut self) {
        let text = self.text();
        self.model.ever.insert(text.clone());
        let state = self.doc().state() as usize;
        assert_eq!(self.doc().history().len(), self.model.states.len());
        assert_eq!(
            text, self.model.states[state],
            "state {state} has the wrong text"
        );
        let on_disk = std::fs::read_to_string(self.file()).ok();
        assert_eq!(
            on_disk, self.model.file,
            "the file is not what it should be"
        );
        if !self.doc().is_modified() {
            // Unmodified is relative to the file as the document last saw
            // it; what others did to it since is `disk_status`'s business.
            assert_eq!(self.model.seen.clone().unwrap_or_default(), text);
        }
        self.look();
    }

    /// Every state can be looked at from where the document is, and
    /// looking changes nothing: not the text, not the history.
    fn look(&mut self) {
        let before = (self.text(), self.doc().revision());
        let history = self.doc().history().clone();
        for (state, expected) in self.model.states.clone().iter().enumerate() {
            let seen = self.doc().text_at(state as StateId);
            assert_eq!(seen.as_ref(), Some(expected), "state {state} looked at");
        }
        let beyond = self.model.states.len() as StateId;
        assert_eq!(self.doc().text_at(beyond), None);
        assert_eq!((self.text(), self.doc().revision()), before);
        assert_eq!(self.doc().history(), &history, "looking moved something");
    }

    /// Checks made once, when the random steps are over.
    fn finish(mut self) {
        // Undoing everything leads back to the first text.
        while self.doc().undo().unwrap().is_some() {}
        assert_eq!(self.doc().state(), ROOT);
        assert_eq!(self.text(), self.model.states[0]);
        // And every state is still reachable and unchanged.
        for (state, expected) in self.model.states.clone().iter().enumerate() {
            self.doc().goto(state as StateId).unwrap();
            assert_eq!(self.text(), *expected);
        }
        // A last crash, to compare what is on disk with what is in memory.
        self.crash();
        self.check();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.document = None;
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, ..ProptestConfig::default() })]

    #[test]
    fn a_document_matches_its_model(
        initial in proptest::option::of(text()),
        ops in proptest::collection::vec(op(), 0..40),
    ) {
        let mut session = Session::new(initial);
        for op in ops {
            session.apply(op);
        }
        session.finish();
    }
}

/// The parts of `text` outside `ranges`.
fn cut(text: &str, ranges: &[std::ops::Range<usize>]) -> Vec<String> {
    let mut outside = Vec::new();
    let mut from = 0;
    for range in ranges {
        outside.push(text[from..range.start].to_owned());
        from = range.end;
    }
    outside.push(text[from..].to_owned());
    outside
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, ..ProptestConfig::default() })]

    /// A step marks all it changed: the text outside its stretches is the
    /// same before and after, piece by piece, whichever way the step is
    /// taken; and the stretches of several steps are those of all their
    /// changes as one step.
    #[test]
    fn a_step_marks_all_it_changed(
        initial in "[ab é\n]{0,12}",
        edits in proptest::collection::vec((any::<Index>(), 0..4_usize, text()), 0..8),
        split in any::<Index>(),
    ) {
        let mut now = initial.clone();
        let mut changes = Vec::new();
        for (at, len, new) in edits {
            let boundaries: Vec<usize> =
                now.char_indices().map(|(i, _)| i).chain([now.len()]).collect();
            let first = at.index(boundaries.len());
            let last = (first + len).min(boundaries.len() - 1);
            let range = boundaries[first]..boundaries[last];
            changes.push(Change { at: range.start, old: now[range.clone()].to_owned(), new: new.clone() });
            now.replace_range(range, &new);
        }
        let step = |changes, direction| Step { changes, direction, to: ROOT };
        let made = step(&changes, Direction::Forward).ranges();
        let undone = step(&changes, Direction::Backward).ranges();

        for (ranges, text) in [(&made, &now), (&undone, &initial)] {
            let mut end = None;
            for range in ranges {
                prop_assert!(range.start <= range.end && range.end <= text.len());
                prop_assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
                // In order, and apart: what touches is one stretch.
                prop_assert!(end.is_none_or(|end| end < range.start), "{ranges:?}");
                end = Some(range.end);
            }
        }
        // (A stretch may read the same before and after: what was taken
        // out and typed again was touched all the same.)
        prop_assert_eq!(cut(&now, &made), cut(&initial, &undone), "{:?} {:?}", &made, &undone);
        // Within the part of the text the step says it replaced.
        let extent = step(&changes, Direction::Forward).extent();
        if let (Some(first), Some(last), Some(extent)) = (made.first(), made.last(), extent) {
            prop_assert!(extent.start <= first.start && last.end <= extent.end);
        }

        // Taken as two steps, one after the other, it marks the same.
        let (first, second) = changes.split_at(split.index(changes.len() + 1));
        let earlier = step(first, Direction::Forward).ranges();
        prop_assert_eq!(&step(second, Direction::Forward).ranges_with(earlier), &made);
        let later = step(second, Direction::Backward).ranges();
        prop_assert_eq!(&step(first, Direction::Backward).ranges_with(later), &undone);
    }
}
