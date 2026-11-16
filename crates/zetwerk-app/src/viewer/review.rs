//! Reviewing with agents: suggestions on the text, what the author makes
//! of them, the instructions that stand, and the record of it all.
//!
//! A [`Suggestion`] is the one thing an agent leaves on the text: a
//! change (to replace, put in, take out or move text) or only a remark
//! or a question, each with why (one sentence), a category in the
//! agent's own word, how sure and how important the agent thinks it is,
//! who made it and when, and a thread of what was said about it since.
//! The proposals of [`super::agent`] are what a change waits as: every
//! suggestion that would change the text has one, which moves with the
//! text, goes stale when its text is changed, is shown on the page as
//! it would be (`zp` flips it, see [`super::notes`]) and is made through
//! the editor as one undo step. Proposals made by the older tools
//! (`edit_source` with `mode: propose`, `answer_note` with a change,
//! `move_section`) become suggestions as they are made.
//!
//! The author answers each with one key: `ga` accepts, `gr` rejects
//! (`:reject why`), `gA` rewrites (the agent's text is put in, and the
//! cursor is in it typing), `gq` answers it in a line, `gp` asks for
//! other wordings (`zp` steps through them on the page), `gz` puts it
//! aside, `g!` makes what was said a standing instruction. `gR` is the
//! manager of all of them (see [`manager`]), `gQ` asks agents to review
//! what is selected or the section here, `:brief` lists the
//! instructions, `:review record` writes it all out (see [`record`]).
//! What changes the text is undone with `u`, and the suggestion is open
//! again; the rest with `:review undo`.
//!
//! All of it is kept beside the journal, in the state directory, one
//! store for each file as the notes are; the instructions for all of the
//! author's documents in one file there. Nothing of it is in the text or
//! in what is exported, and the program asks no model anything: agents
//! read and answer through the MCP tools (see [`agents`]), and wait for
//! the author on a [`Signal`].

mod act;
pub mod agents;
mod brief;
mod manager;
mod page;
mod record;
mod store;

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use typst::syntax::FileId;
use zetwerk_core::StateId;

use super::agent::Proposal;
use super::notes::moved_place;
use super::{Difference, Viewer, difference};

pub use agents::{Agent, Wanted};

/// What a suggestion would do to the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Replace,
    Insert,
    Delete,
    /// Text taken from where it is to another place.
    Move,
    /// A remark on the text: nothing in it is to change.
    Comment,
    /// A question to the author about the text.
    Question,
}

impl Kind {
    pub const ALL: [Self; 6] = [
        Self::Replace,
        Self::Insert,
        Self::Delete,
        Self::Move,
        Self::Comment,
        Self::Question,
    ];

    /// As agents and the store name it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Replace => "replace",
            Self::Insert => "insert",
            Self::Delete => "delete",
            Self::Move => "move",
            Self::Comment => "comment",
            Self::Question => "question",
        }
    }

    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    /// Whether it would change the text.
    #[must_use]
    pub fn changes(self) -> bool {
        !matches!(self, Self::Comment | Self::Question)
    }
}

/// How sure an agent is of a suggestion, or how much it matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub enum Level {
    Low,
    #[default]
    Medium,
    High,
}

impl Level {
    /// As how sure: `low`, `medium`, `high`.
    #[must_use]
    pub fn sure(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// As how important: `minor`, `normal`, `major`.
    #[must_use]
    pub fn weight(self) -> &'static str {
        match self {
            Self::Low => "minor",
            Self::Medium => "normal",
            Self::High => "major",
        }
    }

    /// From either way of naming it.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        match name.trim().to_lowercase().as_str() {
            "low" | "minor" | "unsure" => Some(Self::Low),
            "medium" | "normal" | "fair" => Some(Self::Medium),
            "high" | "major" | "sure" => Some(Self::High),
            _ => None,
        }
    }
}

/// What has become of a suggestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum State {
    /// Nobody has decided on it.
    Open,
    /// The author put it aside, to decide later.
    Later,
    /// Made as the agent offered it (one of its wordings).
    Accepted,
    /// Made, and changed by the author after.
    Rewritten,
    Rejected,
    /// Taken back by the agent that made it.
    Withdrawn,
}

impl State {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Later => "later",
            Self::Accepted => "accepted",
            Self::Rewritten => "rewritten",
            Self::Rejected => "rejected",
            Self::Withdrawn => "withdrawn",
        }
    }

    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        [
            Self::Open,
            Self::Later,
            Self::Accepted,
            Self::Rewritten,
            Self::Rejected,
            Self::Withdrawn,
        ]
        .into_iter()
        .find(|state| state.name() == name)
    }

    /// Whether it is over: nothing more is to be done with it.
    #[must_use]
    pub fn decided(self) -> bool {
        !matches!(self, Self::Open | Self::Later)
    }
}

/// Who said something in a thread.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Who {
    /// The one whose document it is.
    Author,
    /// An agent, by the name its client gave.
    Agent(String),
}

impl Who {
    /// As it is shown and given to agents: `author`, or the agent's name.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Author => "author",
            Self::Agent(name) => name,
        }
    }
}

/// What was said or done in a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Said {
    /// The suggestion was made.
    Made,
    /// A line said to the other side.
    Message(String),
    /// A new version of the change, and why.
    Version {
        text: String,
        reason: String,
    },
    /// Other wordings the agent offers.
    Alternatives(Vec<String>),
    /// The author asked for other wordings.
    AskedAlternatives,
    /// Made: the text before and after.
    Accepted {
        before: String,
        after: String,
    },
    /// Made, and changed by the author: the text before, what the agent
    /// offered, and what the author made of it.
    Rewritten {
        before: String,
        offered: String,
        after: String,
    },
    /// Rejected, and why if the author said.
    Rejected(String),
    Later,
    /// Open again: what was decided was taken back.
    Reopened,
    Withdrawn,
    /// What was said was made a standing instruction.
    Instruction(String),
}

impl Said {
    /// The word for it, as the store and agents have it.
    #[must_use]
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Made => "made",
            Self::Message(_) => "message",
            Self::Version { .. } => "revised",
            Self::Alternatives(_) => "alternatives",
            Self::AskedAlternatives => "asked_alternatives",
            Self::Accepted { .. } => "accepted",
            Self::Rewritten { .. } => "rewritten",
            Self::Rejected(_) => "rejected",
            Self::Later => "later",
            Self::Reopened => "reopened",
            Self::Withdrawn => "withdrawn",
            Self::Instruction(_) => "instruction",
        }
    }
}

/// One step of a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Counts all steps of all threads: what is newer than what an agent
    /// saw last.
    pub seq: u64,
    /// When, in seconds since 1970.
    pub at: u64,
    pub by: Who,
    pub said: Said,
}

/// Something an agent suggests about the text.
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent facts about a suggestion, not states of one thing"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub id: u64,
    pub file: FileId,
    pub kind: Kind,
    /// The bytes of the text it is about. For one that waits to change
    /// the text, its proposal's.
    pub range: Range<usize>,
    /// That text as it was when it was made.
    pub quote: String,
    /// What it would make of that text (the version shown, see `shown`).
    pub new: String,
    /// The text just before a place to put text in, to find the place
    /// again in a file changed while it was closed.
    pub context: String,
    /// Other wordings the agent offered, and which is shown: 0 for
    /// `offered`, then each of them.
    pub offered: String,
    pub alternatives: Vec<String>,
    pub shown: usize,
    /// Why, in a sentence; for a remark or a question, what it says.
    pub reason: String,
    pub category: String,
    pub confidence: Level,
    pub importance: Level,
    /// The agent that made it, and its session.
    pub by: String,
    pub session: String,
    pub made: u64,
    pub state: State,
    pub thread: Vec<Entry>,
    /// The proposal its change waits as.
    pub proposal: Option<u64>,
    /// The review the author asked for that it answers, and the note.
    pub request: Option<u64>,
    pub note: Option<u64>,
    /// The instructions it goes against.
    pub flagged: Vec<u64>,
    /// Whether its text has changed since it was made.
    pub stale: bool,
    /// Whether the author has had the last word, and waits for the
    /// agent's.
    pub waiting: bool,
    /// Whether the agent said something the author has not seen.
    pub unseen: bool,
    /// The heading of the section it is in, where it was last placed.
    pub section: String,
}

impl Suggestion {
    /// What it is shown as in lists: `stale` and `waiting` and
    /// `answered` before what was decided.
    #[must_use]
    pub fn standing(&self) -> &'static str {
        match self.state {
            State::Open | State::Later if self.stale => "stale",
            State::Open | State::Later if self.waiting => "waiting",
            State::Open | State::Later if self.unseen && self.thread.len() > 1 => "answered",
            state => state.name(),
        }
    }

    /// The text of version `shown`.
    #[must_use]
    pub fn version(&self, shown: usize) -> &str {
        match shown {
            0 => &self.offered,
            n => self
                .alternatives
                .get(n - 1)
                .map_or(self.offered.as_str(), String::as_str),
        }
    }

    /// Whether the cursor at `cursor` is on it.
    fn under(&self, cursor: usize) -> bool {
        self.range.contains(&cursor) || self.range.start == cursor
    }

    /// Whether it is to be shown on the text: not decided, and its text
    /// is there.
    fn shown_on_text(&self) -> bool {
        !self.state.decided() && !(self.kind.changes() && self.proposal.is_none())
    }
}

/// A review the author asked agents for: of what is selected, or of a
/// section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub id: u64,
    pub file: FileId,
    pub range: Range<usize>,
    pub quote: String,
    /// The kind of review: `proofread`, `clarity`, …
    pub what: String,
    /// What the author said besides.
    pub words: String,
    pub at: u64,
    pub seq: u64,
    pub done: bool,
    pub section: String,
}

/// A standing instruction to agents: for this document, or for all the
/// author's documents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instruction {
    pub id: u64,
    pub text: String,
    pub everywhere: bool,
    /// The file it was made in, for one of this document.
    pub file: Option<FileId>,
    /// A category no suggestion is to be of.
    pub category: Option<String>,
    /// Text that is not to be changed: where, and what.
    pub protect: Option<(Range<usize>, String)>,
    /// A change that is not to be suggested again: from what to what.
    pub refused: Option<(String, String)>,
    /// The suggestion it was made of.
    pub from: Option<u64>,
    pub at: u64,
    pub seq: u64,
}

/// How much of the review the page shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Marks {
    /// Every suggestion is marked on its text.
    #[default]
    All,
    /// Only the one under the cursor.
    Current,
    /// None; `]p` still goes to them, the slip still shows.
    None,
}

/// What the user says of reviewing, in the settings (`[review]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Settings {
    pub marks: Marks,
    /// Whether instructions made with `g!` are for all documents unless
    /// said otherwise.
    pub everywhere: bool,
}

/// What agents waiting for the author wait on: the count of the
/// author's steps, which goes up when the author answers, decides, asks
/// for a review or changes the brief.
#[derive(Debug, Default)]
pub struct Signal {
    seq: Mutex<u64>,
    changed: Condvar,
}

impl Signal {
    /// The count now.
    pub fn now(&self) -> u64 {
        *self.seq.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn tell(&self, seq: u64) {
        let mut now = self.seq.lock().unwrap_or_else(PoisonError::into_inner);
        *now = (*now).max(seq);
        self.changed.notify_all();
    }

    /// Waits until the count is past `since`, at most `timeout`. Returns
    /// the count then.
    pub fn wait_past(&self, since: u64, timeout: Duration) -> u64 {
        let now = self.seq.lock().unwrap_or_else(PoisonError::into_inner);
        let (now, _) = self
            .changed
            .wait_timeout_while(now, timeout, |seq| *seq <= since)
            .unwrap_or_else(PoisonError::into_inner);
        *now
    }
}

/// A decision that changed the text: undone with `u`, the suggestions
/// are open again; done again, decided again.
struct Undo {
    file: FileId,
    /// The states of the history before and after.
    before: StateId,
    after: StateId,
    /// Whether the text is as the decision made it.
    applied: bool,
    /// The suggestions made, each with its range before.
    made: Vec<(u64, Range<usize>)>,
    /// Where everything else that keeps to the text was, before and
    /// after.
    anchors_before: Anchors,
    anchors_after: Anchors,
}

/// The places of what keeps to the text, in one state of it: proposals
/// by number, notes by number, suggestions, requests and instructions
/// that keep places of their own.
#[derive(Clone, Default)]
struct Anchors {
    proposals: Vec<(u64, Range<usize>, bool)>,
    notes: Vec<(u64, Range<usize>, bool)>,
    own: Vec<(Owner, Range<usize>)>,
}

/// What keeps a place of its own in the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    Suggestion(u64),
    Request(u64),
    Instruction(u64),
}

/// A group of decisions that did not change the text, for `:review
/// undo`: each suggestion with what it was before.
enum Decided {
    States(Vec<(u64, State, bool)>),
    /// The decision of `undo` (its place among them) changed the text.
    Text,
}

/// The text being typed in place of an agent's, after `gA`.
struct Rewriting {
    id: u64,
    file: FileId,
    range: Range<usize>,
}

/// What a line typed on the command line is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Asking {
    Answer,
    Reject,
    Always,
}

/// What reviewing keeps between keys.
#[derive(Default)]
pub(super) struct Review {
    all: Vec<Suggestion>,
    requests: Vec<Request>,
    brief: Vec<Instruction>,
    /// The numbers the last suggestion, request and instruction got.
    last: u64,
    last_request: u64,
    last_instruction: u64,
    /// Counts the steps of all threads (see [`Entry::seq`]).
    seq: u64,
    /// The files whose stores have been read, and whether the brief for
    /// all documents has.
    loaded: Vec<FileId>,
    everywhere_loaded: bool,
    /// The files whose stores are to be written.
    unwritten: Vec<FileId>,
    everywhere_unwritten: bool,
    /// The text of the file being edited that the places kept here are
    /// places in.
    seen: Option<(FileId, String)>,
    /// Whether the text changed since the suggestions were last placed.
    dirty: bool,
    /// Counts the changes to what is shown: what was worked out for an
    /// older count is old.
    generation: u64,
    /// The number the next proposal had when the proposals were last
    /// looked through for ones to adopt.
    adopted: u64,
    /// The proposals that are suggestions.
    owned: HashSet<u64>,
    undo: Vec<Undo>,
    decided: Vec<Decided>,
    rewriting: Option<Rewriting>,
    /// What a line typed on the command line is for, about which
    /// suggestion, and the cursor then.
    asking: Option<(Asking, Option<u64>, usize)>,
    /// What a review asked for with `gQ` is to be of.
    requesting: Option<(FileId, Range<usize>)>,
    /// The suggestion decided on or answered last, the cursor then, and
    /// what the author said: what `g!` is about once it is gone from
    /// under the cursor.
    last_acted: Option<(u64, usize, String)>,
    /// The suggestion in front where several are on the same text.
    front: Option<u64>,
    manager: Option<manager::Manager>,
    /// Whether the manager is to come back once a line typed for it is.
    back_to_manager: bool,
    /// Where the marks of the suggestions are on the pages drawn.
    placed: HashMap<usize, page::Placed>,
    /// Where the suggestion in front is drawn in the frame being drawn.
    slip_at: Option<(f64, f64, f64, f64)>,
    signal: Arc<Signal>,
    settings: Settings,
}

/// Seconds since 1970.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Where byte `place` is after `edits` (each a range of bytes of the
/// text as it was, apart and in the order of the text, and how many
/// bytes came in its place): as the start of a stretch or as its end.
fn shifted(place: usize, edits: &[(Range<usize>, usize)], end: bool) -> usize {
    edits.iter().rev().fold(place, |place, (range, inserted)| {
        moved_place(place, (range.start, range.len(), *inserted), end)
    })
}

/// The bytes `range` of `text`, if they are bytes of it.
fn part<'a>(text: &'a str, range: &Range<usize>) -> Option<&'a str> {
    text.get(range.clone())
}

impl Viewer {
    /// Has reviewing ready for what is asked of it: the stores of the
    /// file being edited read, proposals made by other ways taken in,
    /// and the suggestions placed where the text has them now. Cheap
    /// where nothing changed.
    pub(super) fn review_ready(&mut self) {
        if self.editor.is_none() {
            return;
        }
        self.load_review();
        if self.next_proposal != self.review.adopted {
            self.adopt_proposals(None, None, None);
        }
        self.rewritten_done();
        self.settle_review();
    }

    /// Places the suggestions of the file being edited where their
    /// proposals are, after the text changed.
    fn settle_review(&mut self) {
        if !std::mem::take(&mut self.review.dirty) {
            return;
        }
        let file = self.file_id();
        let text = self.text().unwrap_or_default().to_owned();
        let proposals = &self.proposals;
        let mut gone = Vec::new();
        for suggestion in &mut self.review.all {
            if suggestion.file != file || suggestion.state.decided() {
                continue;
            }
            if !suggestion.kind.changes() {
                let there = part(&text, &suggestion.range)
                    .is_some_and(|quote| quote == suggestion.quote || suggestion.quote.is_empty());
                suggestion.stale = !there;
                continue;
            }
            let Some(id) = suggestion.proposal else {
                suggestion.stale = true;
                continue;
            };
            match proposals.iter().find(|proposal| proposal.id == id) {
                Some(proposal) => {
                    suggestion.range = proposal.range.clone();
                    suggestion.stale = proposal.stale;
                }
                None => gone.push(suggestion.id),
            }
        }
        for id in gone {
            // Taken away by another way than reviewing: by the agent,
            // with the older tools.
            if let Some(at) = self.suggestion_index(id) {
                let by = Who::Agent(self.review.all[at].by.clone());
                let suggestion = &mut self.review.all[at];
                suggestion.proposal = None;
                suggestion.state = State::Withdrawn;
                self.review_said(at, by, Said::Withdrawn);
            }
        }
        self.review_shown_changed();
    }

    /// Notes that what is shown of reviewing changed: where the marks go
    /// is worked out anew.
    fn review_shown_changed(&mut self) {
        self.review.generation += 1;
        self.review.placed.clear();
        self.review.owned = self
            .review
            .all
            .iter()
            .filter(|suggestion| !suggestion.state.decided())
            .filter_map(|suggestion| suggestion.proposal)
            .collect();
    }

    /// Notes that reviewing changed: shown anew, and to be written; the
    /// text is noted that new places of its own are places in.
    fn review_changed(&mut self) {
        self.review_shown_changed();
        for file in self.review.loaded.clone() {
            if !self.review.unwritten.contains(&file) {
                self.review.unwritten.push(file);
            }
        }
        if let Some(editor) = &self.editor {
            let file = editor.file();
            let noted = self.review.seen.as_ref().is_some_and(|(of, _)| *of == file);
            if !noted && self.has_own_places(file) {
                self.review.seen = Some((file, editor.text().to_owned()));
            }
        }
    }

    /// Whether anything of reviewing keeps a place of its own in `file`.
    fn has_own_places(&self, file: FileId) -> bool {
        let review = &self.review;
        review
            .all
            .iter()
            .any(|suggestion| suggestion.file == file && !suggestion.kind.changes())
            || review.requests.iter().any(|request| request.file == file)
            || review
                .brief
                .iter()
                .any(|instruction| instruction.file == Some(file) && instruction.protect.is_some())
            || review
                .rewriting
                .as_ref()
                .is_some_and(|rewriting| rewriting.file == file)
    }

    /// Adds to the thread of suggestion `at` what `by` said or did, and
    /// tells agents that wait where it was the author.
    fn review_said(&mut self, at: usize, by: Who, said: Said) {
        self.review.seq += 1;
        let seq = self.review.seq;
        let author = by == Who::Author;
        let suggestion = &mut self.review.all[at];
        suggestion.thread.push(Entry {
            seq,
            at: now(),
            by,
            said,
        });
        if author {
            suggestion.unseen = false;
            self.review.signal.tell(seq);
        }
    }

    /// Tells agents that wait that the author did something not in a
    /// thread: asked for a review, changed the brief.
    fn review_told(&mut self) -> u64 {
        self.review.seq += 1;
        let seq = self.review.seq;
        self.review.signal.tell(seq);
        seq
    }

    /// The place of suggestion `id` among all.
    fn suggestion_index(&self, id: u64) -> Option<usize> {
        self.review
            .all
            .iter()
            .position(|suggestion| suggestion.id == id)
    }

    /// The suggestions of the file being edited that the cursor is on and
    /// that are shown, the one in front first.
    fn suggestions_here(&self) -> Vec<usize> {
        let Some(editor) = &self.editor else {
            return Vec::new();
        };
        let (file, cursor) = (editor.file(), editor.cursor());
        let mut here: Vec<usize> = (0..self.review.all.len())
            .filter(|at| {
                let suggestion = &self.review.all[*at];
                suggestion.file == file && suggestion.shown_on_text() && suggestion.under(cursor)
            })
            .collect();
        // The one in front, then the narrowest: the most particular.
        here.sort_by_key(|at| {
            let suggestion = &self.review.all[*at];
            (
                Some(suggestion.id) != self.review.front,
                suggestion.state == State::Later,
                suggestion.range.len(),
                suggestion.id,
            )
        });
        here
    }

    /// The suggestion the cursor is on, the one in front.
    fn suggestion_here(&self) -> Option<usize> {
        self.suggestions_here().first().copied()
    }

    /// The heading of the section byte `at` of the file being edited is
    /// in, or nothing before the first heading.
    fn section_title_at(&self, at: usize) -> String {
        let Some(editor) = &self.editor else {
            return String::new();
        };
        let all = crate::sections::sections(editor.source());
        crate::sections::at(&all, at).map_or_else(String::new, |index| all[index].title.clone())
    }

    /// The hook of [`Viewer::sync_proposals`]: the text changed. Moves
    /// the places reviewing keeps of its own along with it, and where
    /// the change takes back a decision (`u`) or makes it again, the
    /// suggestions are open, or decided, again.
    pub(super) fn sync_review(&mut self) {
        self.review.dirty = true;
        let Some(editor) = &self.editor else {
            return;
        };
        let (file, state) = (editor.file(), editor.history().current());
        let undone = self
            .review
            .undo
            .iter()
            .position(|undo| undo.file == file && undo.applied && undo.before == state);
        let redone = self
            .review
            .undo
            .iter()
            .position(|undo| undo.file == file && !undo.applied && undo.after == state);
        if let Some(at) = undone {
            return self.review_take_back(at);
        }
        if let Some(at) = redone {
            return self.review_make_again(at);
        }
        self.move_own_places();
    }

    /// Moves what keeps a place of its own (remarks, requests, the text
    /// an instruction protects, what is being rewritten) along with the
    /// change of the text since it was last seen.
    fn move_own_places(&mut self) {
        let Some(editor) = &self.editor else {
            return;
        };
        let file = editor.file();
        let has = self.has_own_places(file);
        let Some(editor) = &self.editor else {
            return;
        };
        let now = editor.text();
        let review = &mut self.review;
        if !has {
            review.seen = None;
            return;
        }
        let Some((_, before)) = review.seen.as_mut().filter(|(of, _)| *of == file) else {
            review.seen = Some((file, now.to_owned()));
            return;
        };
        let Difference::Replacement {
            offset,
            removed,
            text,
        } = difference(before, now)
        else {
            return;
        };
        let change = [(offset..offset + removed, text.len())];
        let moved = |range: &mut Range<usize>| {
            let start = shifted(range.start, &change, false);
            *range = start..shifted(range.end, &change, true).max(start);
        };
        for suggestion in &mut review.all {
            if suggestion.file == file && !suggestion.kind.changes() {
                moved(&mut suggestion.range);
            }
        }
        for request in &mut review.requests {
            if request.file == file {
                moved(&mut request.range);
            }
        }
        for instruction in &mut review.brief {
            if let (Some((range, _)), true) =
                (&mut instruction.protect, instruction.file == Some(file))
            {
                moved(range);
            }
        }
        if let Some(rewriting) = &mut review.rewriting
            && rewriting.file == file
        {
            // What is typed at either end of it is part of it.
            let range = &mut rewriting.range;
            let (start, end) = (range.start, range.end);
            let start = if start == offset {
                start
            } else {
                shifted(start, &change, false)
            };
            let end = if removed == 0 && end == offset {
                end + text.len()
            } else {
                shifted(end, &change, true)
            };
            *range = start..end.max(start);
        }
        before.clear();
        before.push_str(now);
    }

    /// Where everything that keeps to the text of `file` is now.
    fn review_anchors(&self, file: FileId) -> Anchors {
        let proposals = self
            .proposals
            .iter()
            .filter(|proposal| proposal.file == file)
            .map(|proposal| (proposal.id, proposal.range.clone(), proposal.stale))
            .collect();
        let notes = self
            .notes
            .all
            .iter()
            .filter(|note| note.file == file)
            .map(|note| (note.id, note.range.clone(), note.detached))
            .collect();
        let mut own = Vec::new();
        for suggestion in &self.review.all {
            if suggestion.file == file && !suggestion.kind.changes() {
                own.push((Owner::Suggestion(suggestion.id), suggestion.range.clone()));
            }
        }
        for request in &self.review.requests {
            if request.file == file {
                own.push((Owner::Request(request.id), request.range.clone()));
            }
        }
        for instruction in &self.review.brief {
            if let (Some((range, _)), true) = (&instruction.protect, instruction.file == Some(file))
            {
                own.push((Owner::Instruction(instruction.id), range.clone()));
            }
        }
        Anchors {
            proposals,
            notes,
            own,
        }
    }

    /// Puts everything that keeps to the text where `anchors` say, and
    /// notes the text as it is now as what they are places in: what
    /// moves them by the difference of two texts then finds none.
    fn restore_review_anchors(&mut self, anchors: &Anchors) {
        let Some(editor) = &self.editor else {
            return;
        };
        let file = editor.file();
        let now = editor.text().to_owned();
        for proposal in &mut self.proposals {
            if let Some((_, range, stale)) =
                anchors.proposals.iter().find(|(id, ..)| *id == proposal.id)
            {
                proposal.range = range.clone();
                proposal.stale = *stale || part(&now, range) != Some(proposal.old.as_str());
            }
        }
        for note in &mut self.notes.all {
            if let Some((_, range, detached)) = anchors.notes.iter().find(|(id, ..)| *id == note.id)
            {
                note.range = range.clone();
                note.detached = *detached;
            }
        }
        for (owner, range) in &anchors.own {
            match owner {
                Owner::Suggestion(id) => {
                    if let Some(suggestion) = self.review.all.iter_mut().find(|s| s.id == *id) {
                        suggestion.range = range.clone();
                    }
                }
                Owner::Request(id) => {
                    if let Some(request) = self.review.requests.iter_mut().find(|r| r.id == *id) {
                        request.range = range.clone();
                    }
                }
                Owner::Instruction(id) => {
                    let found = self.review.brief.iter_mut().find(|i| i.id == *id);
                    if let Some((kept, _)) = found.and_then(|i| i.protect.as_mut()) {
                        *kept = range.clone();
                    }
                }
            }
        }
        if !self.proposals.is_empty() {
            self.proposal_text = Some(now.clone());
        }
        if self.notes.seen.as_ref().is_some_and(|(of, _)| *of == file) {
            self.notes.seen = Some((file, now.clone()));
        }
        self.review.seen = Some((file, now));
        self.review.dirty = true;
    }

    /// `u` took back decision `at`: its suggestions are open again, and
    /// all else is where it was before it.
    fn review_take_back(&mut self, at: usize) {
        let undo = &mut self.review.undo[at];
        undo.applied = false;
        let (made, anchors) = (undo.made.clone(), undo.anchors_before.clone());
        let file = undo.file;
        let text = self.text().unwrap_or_default().to_owned();
        for (id, range) in &made {
            let Some(index) = self.suggestion_index(*id) else {
                continue;
            };
            let proposal = self.next_proposal;
            self.next_proposal += 1;
            let suggestion = &mut self.review.all[index];
            suggestion.state = State::Open;
            suggestion.range = range.clone();
            suggestion.proposal = Some(proposal);
            let old = part(&text, range).unwrap_or_default().to_owned();
            self.proposals.push(Proposal {
                id: proposal,
                file,
                range: range.clone(),
                stale: old != suggestion.quote,
                old,
                new: suggestion.new.clone(),
            });
            self.review_said(index, Who::Author, Said::Reopened);
        }
        self.proposals.sort_by_key(|proposal| proposal.range.start);
        if self
            .review
            .rewriting
            .as_ref()
            .is_some_and(|rewriting| made.iter().any(|(id, _)| *id == rewriting.id))
        {
            self.review.rewriting = None;
        }
        self.restore_review_anchors(&anchors);
        self.review_changed();
        let count = made.len();
        self.notice(if count == 1 {
            "taken back: the suggestion is open again".to_owned()
        } else {
            format!("taken back: {count} suggestions are open again")
        });
    }

    /// The decision `at` was taken back and is done again (a redo): its
    /// suggestions are decided again.
    fn review_make_again(&mut self, at: usize) {
        let undo = &mut self.review.undo[at];
        undo.applied = true;
        let (made, anchors) = (undo.made.clone(), undo.anchors_after.clone());
        for (id, _) in &made {
            let Some(index) = self.suggestion_index(*id) else {
                continue;
            };
            let suggestion = &mut self.review.all[index];
            let proposal = suggestion.proposal.take();
            suggestion.state = State::Accepted;
            self.proposals
                .retain(|waiting| Some(waiting.id) != proposal);
            let said = Said::Accepted {
                before: self.review.all[index].quote.clone(),
                after: self.review.all[index].new.clone(),
            };
            self.review_said(index, Who::Author, said);
        }
        self.restore_review_anchors(&anchors);
        self.review_changed();
    }

    /// Whether proposal `id` is shown by reviewing (in a colour of its
    /// kind), and not by the agents' marks.
    pub(super) fn review_marks_proposal(&self, id: u64) -> bool {
        self.review.owned.contains(&id)
    }

    /// The marks of suggestions in the source, as the agents' marks.
    pub(super) fn review_source_marks(&self) -> Vec<(Range<usize>, super::agent::Mark)> {
        let file = self.file_id();
        self.review
            .all
            .iter()
            .filter(|suggestion| suggestion.file == file && suggestion.shown_on_text())
            .map(|suggestion| {
                let mark = if suggestion.stale {
                    super::agent::Mark::Stale
                } else {
                    super::agent::Mark::Proposed
                };
                (suggestion.range.clone(), mark)
            })
            .collect()
    }

    /// The suggestions of all open files.
    #[must_use]
    pub fn suggestions(&self) -> &[Suggestion] {
        &self.review.all
    }

    /// The instructions that stand: those for all documents first.
    #[must_use]
    pub fn brief(&self) -> &[Instruction] {
        &self.review.brief
    }

    /// The reviews the author asked for.
    #[must_use]
    pub fn review_requests(&self) -> &[Request] {
        &self.review.requests
    }

    /// What agents waiting for the author wait on.
    #[must_use]
    pub fn review_signal(&self) -> Arc<Signal> {
        Arc::clone(&self.review.signal)
    }

    /// Takes what the user says of reviewing.
    pub fn set_review(&mut self, settings: Settings) {
        self.review.settings = settings;
        self.review.generation += 1;
    }

    /// Takes over from `before`, the viewer this one takes the place of
    /// in the window, what agents wait on: an agent waiting goes on
    /// waiting for the author in the document shown now.
    pub(super) fn review_take_over(&mut self, before: &Viewer) {
        self.review.signal = Arc::clone(&before.review.signal);
        self.review.seq = self.review.seq.max(before.review.seq);
        self.review.settings = before.review.settings;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use super::*;

    #[test]
    fn places_move_with_several_edits_at_once() {
        // "one two three four": "two" becomes "2", "four" goes.
        let edits = [(4..7, 1), (14..18, 0)];
        assert_eq!(shifted(0, &edits, false), 0);
        assert_eq!(shifted(8, &edits, false), 6, "three moves back");
        assert_eq!(shifted(13, &edits, true), 11);
        assert_eq!(shifted(18, &edits, true), 12);
    }

    #[test]
    fn the_signal_wakes_those_waiting_past_a_count() {
        let signal = Arc::new(Signal::default());
        assert_eq!(signal.wait_past(0, Duration::from_millis(10)), 0);
        let told = Arc::clone(&signal);
        let thread = std::thread::spawn(move || told.wait_past(0, Duration::from_secs(30)));
        std::thread::sleep(Duration::from_millis(20));
        signal.tell(3);
        assert_eq!(thread.join().unwrap(), 3);
        assert_eq!(signal.now(), 3);
    }

    #[test]
    fn names_go_both_ways() {
        for kind in Kind::ALL {
            assert_eq!(Kind::named(kind.name()), Some(kind));
        }
        assert_eq!(Level::named("sure"), Some(Level::High));
        assert_eq!(Level::named("minor"), Some(Level::Low));
        assert!(State::Rejected.decided() && !State::Later.decided());
    }
}
