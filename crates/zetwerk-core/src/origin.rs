//! Where the text came from: for every byte, who put it there and when.
//!
//! An edit is made by someone: the author typing, zetwerk doing something
//! for the author, text pasted from another program, an agent, a change
//! made to the file outside zetwerk. That is the edit's [`Origin`]; with
//! the time it was made it is a [`Credit`], and the credits are numbered
//! in the document's [`Attribution`] (a few dozen of them in a long life:
//! a credit is made when the origin changes or ten minutes have passed).
//!
//! The attribution is a second layer on the text, kept exactly as the
//! text is kept. Every change in the history carries, besides the text
//! it took out and the text it put in, the credits of both
//! ([`ChangeMarks`]): undo puts back what was taken out with the credits
//! it had, redo puts back what came in with its own. So the attribution
//! of any state of the history follows from that of the current state,
//! as its text does, and nothing that folds steps together (compaction,
//! thinning) can change the attribution of a text that stays reachable.
//! The current state's attribution ([`Runs`], runs of bytes and their
//! [`Mark`]) is kept beside the text and changed with it, at the cost of
//! a few runs per key.
//!
//! A byte's [`Mark`] says who wrote it (`by`) and, where it took the
//! place of someone else's text, whose (`over`): an agent's sentence of
//! which the author changed a word has that word marked "by the author,
//! over the agent": agent text the author has since edited. A change that
//! replaces text by text that is partly the same (an agent rewriting a
//! paragraph with a few words changed, a file changed on disk in two
//! places) only credits the words that are different: those that stay
//! keep their marks. That is an alignment of words, not a guess: a word
//! that is kept is the same word, in the same order.
//!
//! In the journal ([`crate::journal`]) the credits are records of their
//! own, the marks of a step go in a record just before the step, and a
//! snapshot of the text is followed by the attribution of that text. A
//! journal from before origins were recorded has none of these: its text
//! and its steps are of an unknown origin, and are said to be.

use std::ops::Range;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::change::{Change, Direction};
use crate::history::{History, StateId};
use crate::journal::Record;

/// Who made an edit.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Origin {
    /// Not known: text from before zetwerk recorded origins, or that was
    /// in the file when zetwerk first opened it.
    Unknown,
    /// Typed by the author.
    Author,
    /// Done by zetwerk for the author (a starter filled in, a manager's
    /// rule, a product spelled out in an equation, a tidy-up): the
    /// author's, and what did it.
    Zetwerk(String),
    /// Put in from the system clipboard: whatever its true source.
    Pasted,
    /// An agent changed the text, live.
    Agent(Agent),
    /// An agent proposed it, and the author accepted it.
    Accepted(Agent),
    /// The file was changed outside zetwerk (another editor, git, an
    /// agent that wrote the file), and zetwerk took the change in.
    Outside,
}

/// An agent, as it introduced itself.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Agent {
    /// The name and version of its MCP client.
    pub name: String,
    pub version: String,
    /// Which conversation of it: one connection to zetwerk.
    pub session: String,
}

/// An origin, and when (seconds since 1970) its edit was made, or for
/// [`Origin::Outside`] noticed; 0 where that is not known.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Credit {
    pub origin: Origin,
    pub when: u64,
}

impl Credit {
    /// Of text from before origins were recorded.
    #[must_use]
    pub fn unknown() -> Self {
        Self {
            origin: Origin::Unknown,
            when: 0,
        }
    }
}

/// A byte's credit, and that of the text it took the place of.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Credited {
    pub by: Credit,
    /// Whose text it was put in place of, where that was someone else.
    pub over: Option<Credit>,
}

/// What text is, for counting it and for showing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// The author's own: typed, or done by zetwerk for the author.
    Own,
    /// An agent's, as it wrote it (live or accepted).
    Agent,
    /// The author's, in the place of an agent's text: agent text the
    /// author has since edited.
    Edited,
    Pasted,
    Outside,
    Unknown,
}

/// The number of a credit in an [`Attribution`]. 0 is [`Credit::unknown`].
pub type CreditId = u32;

/// What a byte is marked with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Mark {
    pub by: CreditId,
    pub over: Option<CreditId>,
}

impl Mark {
    const UNKNOWN: Self = Self { by: 0, over: None };

    /// Whose text this was first: the one it was put in place of, or its
    /// own writer.
    fn writer(self) -> CreditId {
        self.over.unwrap_or(self.by)
    }
}

/// `len` bytes marked `mark`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    pub len: u32,
    pub mark: Mark,
}

/// The marks of the two sides of a change: of the text it took out and
/// of the text it put in.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChangeMarks {
    pub old: Vec<Run>,
    pub new: Vec<Run>,
}

/// How a change lies: where, and how long its two sides are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Shape {
    at: usize,
    old: usize,
    new: usize,
}

impl Shape {
    pub(crate) fn of(change: &Change) -> Self {
        Self {
            at: change.at,
            old: change.old.len(),
            new: change.new.len(),
        }
    }
}

impl ChangeMarks {
    /// The marks of a change of which nothing is known.
    fn unknown(change: &Change) -> Self {
        let mut marks = Self::default();
        push(&mut marks.old, change.old.len(), Mark::UNKNOWN);
        push(&mut marks.new, change.new.len(), Mark::UNKNOWN);
        marks
    }

    /// Whether these can be the marks of `change`.
    fn fit(&self, change: &Change) -> bool {
        total(&self.old) == change.old.len() && total(&self.new) == change.new.len()
    }
}

/// Appends `len` bytes marked `mark` to `runs`, as part of the last run
/// if it has that mark.
fn push(runs: &mut Vec<Run>, len: usize, mark: Mark) {
    if len == 0 {
        return;
    }
    match runs.last_mut() {
        Some(last) if last.mark == mark => last.len += len as u32,
        _ => runs.push(Run {
            len: len as u32,
            mark,
        }),
    }
}

fn append(runs: &mut Vec<Run>, more: &[Run]) {
    for run in more {
        push(runs, run.len as usize, run.mark);
    }
}

fn total(runs: &[Run]) -> usize {
    runs.iter().map(|run| run.len as usize).sum()
}

/// The marks of bytes `range` of what `runs` mark.
fn slice(runs: &[Run], range: Range<usize>) -> Vec<Run> {
    let mut out = Vec::new();
    let mut start = 0;
    for run in runs {
        let end = start + run.len as usize;
        let (from, to) = (start.max(range.start), end.min(range.end));
        if from < to {
            push(&mut out, to - from, run.mark);
        }
        if end >= range.end {
            break;
        }
        start = end;
    }
    out
}

/// `runs` with bytes `range` replaced by `with`.
fn spliced(runs: &[Run], range: Range<usize>, with: &[Run]) -> Vec<Run> {
    let mut out = slice(runs, 0..range.start);
    append(&mut out, with);
    append(&mut out, &slice(runs, range.end..total(runs)));
    out
}

/// How many runs a piece of [`Runs`] holds at most, and about how many
/// a piece holds after it is cut.
const PIECE: usize = 512;
const HALF_PIECE: usize = PIECE / 2;

/// The marks of a whole text: runs, kept in pieces so that a change
/// touches one piece or two, however long the text.
#[derive(Debug, Clone, Default)]
pub struct Runs {
    pieces: Vec<Vec<Run>>,
    /// The bytes each piece marks.
    lens: Vec<usize>,
    len: usize,
}

impl Runs {
    fn of(runs: &[Run]) -> Self {
        let mut made = Self::default();
        made.replace(0..0, runs);
        made
    }

    /// The number of bytes marked.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The piece byte `at` is in, and where it begins: the last piece for
    /// the end of the text.
    fn piece_at(&self, at: usize) -> (usize, usize) {
        let mut start = 0;
        for (index, len) in self.lens.iter().enumerate() {
            if at < start + len || index + 1 == self.lens.len() {
                return (index, start);
            }
            start += len;
        }
        (0, 0)
    }

    /// Replaces the marks of bytes `range` by `with`.
    fn replace(&mut self, range: Range<usize>, with: &[Run]) {
        let added = total(with);
        if self.pieces.is_empty() {
            let mut runs = Vec::new();
            append(&mut runs, with);
            self.set_pieces(0..0, runs);
            self.len = added;
            return;
        }
        let (first, start) = self.piece_at(range.start);
        let (last, _) = self.piece_at(range.end.saturating_sub(1).max(range.start));
        let mut runs = Vec::new();
        for piece in &self.pieces[first..=last] {
            append(&mut runs, piece);
        }
        let runs = spliced(&runs, range.start - start..range.end - start, with);
        self.set_pieces(first..last + 1, runs);
        self.len = self.len - range.len() + added;
    }

    /// Puts `runs`, cut into pieces, in the place of pieces `which`.
    fn set_pieces(&mut self, which: Range<usize>, runs: Vec<Run>) {
        let mut pieces = Vec::new();
        if runs.len() <= PIECE {
            if !runs.is_empty() {
                pieces.push(runs);
            }
        } else {
            pieces.extend(runs.chunks(HALF_PIECE).map(<[Run]>::to_vec));
        }
        let lens: Vec<usize> = pieces.iter().map(|piece| total(piece)).collect();
        self.pieces.splice(which.clone(), pieces);
        self.lens.splice(which, lens);
    }

    /// The marks of bytes `range`.
    #[must_use]
    pub fn slice(&self, range: Range<usize>) -> Vec<Run> {
        let mut out = Vec::new();
        for (at, mark) in self.within(range) {
            push(&mut out, at.len(), mark);
        }
        out
    }

    /// The mark of byte `at`.
    #[must_use]
    pub fn at(&self, at: usize) -> Option<Mark> {
        self.within(at..at + 1).next().map(|(_, mark)| mark)
    }

    /// The runs that mark bytes of `range`, cut to it, in order.
    pub fn within(&self, range: Range<usize>) -> impl Iterator<Item = (Range<usize>, Mark)> + '_ {
        let (first, mut start) = if self.pieces.is_empty() {
            (0, 0)
        } else {
            self.piece_at(range.start)
        };
        self.pieces
            .iter()
            .skip(first)
            .flatten()
            .map_while(move |run| {
                let here = start..start + run.len as usize;
                start = here.end;
                (here.start < range.end).then_some((here, run.mark))
            })
            .filter_map(move |(here, mark)| {
                let cut = here.start.max(range.start)..here.end.min(range.end);
                (cut.start < cut.end).then_some((cut, mark))
            })
    }

    /// All runs, in order.
    pub fn iter(&self) -> impl Iterator<Item = (Range<usize>, Mark)> + '_ {
        self.within(0..self.len)
    }

    fn flat(&self) -> Vec<Run> {
        let mut out = Vec::new();
        for piece in &self.pieces {
            append(&mut out, piece);
        }
        out
    }
}

/// The current time, in seconds since 1970.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// How long a credit made for the edits of one origin is used for more
/// of them.
const CREDIT_SECONDS: u64 = 600;

/// Who is behind an edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Party<'a> {
    Author,
    Agent(&'a str),
    Pasted,
    Outside,
    Unknown,
}

fn party(origin: &Origin) -> Party<'_> {
    match origin {
        Origin::Author | Origin::Zetwerk(_) => Party::Author,
        Origin::Agent(agent) | Origin::Accepted(agent) => Party::Agent(&agent.name),
        Origin::Pasted => Party::Pasted,
        Origin::Outside => Party::Outside,
        Origin::Unknown => Party::Unknown,
    }
}

/// How the text of each edit is to be credited.
#[derive(Debug, Clone, Copy)]
pub enum By<'a> {
    /// To `origin`, but for the words an edit only kept.
    Origin(&'a Origin),
    /// For each edit, the credits of the text it puts in as they are
    /// given, carried from elsewhere (a register's text yanked from the
    /// document), stretch by stretch; a stretch credited `None`, an edit
    /// given `None` or credits that do not cover its text, as for
    /// `origin`.
    Given {
        origin: &'a Origin,
        given: &'a [Option<Given>],
    },
}

impl By<'_> {
    fn origin(&self) -> &Origin {
        match self {
            Self::Origin(origin) | Self::Given { origin, .. } => origin,
        }
    }
}

/// The credits of a text, stretch by stretch: lengths in bytes, and
/// whose they are (`None`: of whoever makes the edit).
pub type Given = Vec<(usize, Option<Credited>)>;

/// What an edit will do to the attribution, worked out before it is
/// written down.
pub(crate) struct Prepared {
    /// The marks of each change.
    pub(crate) marks: Vec<ChangeMarks>,
    shapes: Vec<Shape>,
}

/// What [`Attribution::before`] notes of the history before a record of
/// the journal is replayed.
pub(crate) struct Before {
    from: StateId,
    len: usize,
    /// The last change of the current state and its marks, and the
    /// marks of the changes before it: what an extension may be folded
    /// into.
    last: Option<(Change, ChangeMarks)>,
    node: Vec<ChangeMarks>,
}

/// The credits of a document, the marks of its text and those of the
/// changes in its history.
#[derive(Debug, Clone)]
pub struct Attribution {
    credits: Vec<Credit>,
    /// How many of the credits the journal has.
    journaled: usize,
    /// The marks of the text of the current state. While a journal is
    /// replayed and its text is not known yet, of nothing.
    map: Runs,
    known: bool,
    /// The marks of each state's changes, by state; missing for states
    /// of which they are not known.
    nodes: Vec<Vec<ChangeMarks>>,
    /// The credit last made for an edit.
    live: Option<CreditId>,
    /// Marks read in the journal for the record that follows them.
    pending: Option<(StateId, Vec<ChangeMarks>)>,
    /// A state that the salvage of a damaged journal made out of a
    /// difference, whose new text's marks the snapshot's attribution
    /// gives once it is read.
    diffed: Option<(StateId, usize)>,
    clock: fn() -> u64,
}

impl Default for Attribution {
    fn default() -> Self {
        Self {
            credits: vec![Credit::unknown()],
            journaled: 1,
            map: Runs::default(),
            known: false,
            nodes: Vec::new(),
            live: None,
            pending: None,
            diffed: None,
            clock: now,
        }
    }
}

impl Attribution {
    /// The attribution of `len` bytes of text that zetwerk sees for the
    /// first time, with no history: not known, as of now.
    pub(crate) fn first_seen(len: usize) -> Self {
        let mut made = Self::default();
        let mark = if len == 0 {
            Mark::UNKNOWN
        } else {
            made.credits.push(Credit {
                origin: Origin::Unknown,
                when: now(),
            });
            Mark { by: 1, over: None }
        };
        made.journaled = made.credits.len();
        made.map = Runs::of(&[Run {
            len: len as u32,
            mark,
        }]);
        made.known = true;
        made
    }

    /// Makes the attribution read the time from `clock`: for tests.
    pub fn set_clock(&mut self, clock: fn() -> u64) {
        self.clock = clock;
    }

    /// The marks of the current text.
    #[must_use]
    pub fn map(&self) -> &Runs {
        &self.map
    }

    /// Credit `id`; an unknown one for a number never given.
    #[must_use]
    pub fn credit(&self, id: CreditId) -> &Credit {
        const UNKNOWN: Credit = Credit {
            origin: Origin::Unknown,
            when: 0,
        };
        self.credits.get(id as usize).unwrap_or(&UNKNOWN)
    }

    /// Every credit there is, by number.
    #[must_use]
    pub fn credits(&self) -> &[Credit] {
        &self.credits
    }

    /// What a byte marked `mark` is.
    #[must_use]
    pub fn kind(&self, mark: Mark) -> Kind {
        let by = &self.credit(mark.by).origin;
        match party(by) {
            Party::Author => {
                let over = mark.over.map(|over| party(&self.credit(over).origin));
                if matches!(over, Some(Party::Agent(_))) {
                    Kind::Edited
                } else {
                    Kind::Own
                }
            }
            Party::Agent(_) => Kind::Agent,
            Party::Pasted => Kind::Pasted,
            Party::Outside => Kind::Outside,
            Party::Unknown => Kind::Unknown,
        }
    }

    /// A mark as credits.
    #[must_use]
    pub fn credited(&self, mark: Mark) -> Credited {
        Credited {
            by: self.credit(mark.by).clone(),
            over: mark.over.map(|over| self.credit(over).clone()),
        }
    }

    /// Bytes `range` of the current text with their credits, to carry
    /// elsewhere: as lengths and credits.
    #[must_use]
    pub fn carried(&self, range: Range<usize>) -> Given {
        self.map
            .within(range)
            .map(|(at, mark)| (at.len(), Some(self.credited(mark))))
            .collect()
    }

    /// The marks of the changes of state `state`, if they are known.
    #[must_use]
    pub fn step_marks(&self, state: StateId) -> Option<&[ChangeMarks]> {
        self.nodes
            .get(usize::try_from(state).ok()?)
            .map(Vec::as_slice)
            .filter(|marks| !marks.is_empty())
    }

    /// The number of `credit`, made if there is none yet.
    fn number(&mut self, credit: &Credit) -> CreditId {
        if let Some(at) = self.credits.iter().position(|known| known == credit) {
            return at as CreditId;
        }
        self.credits.push(credit.clone());
        (self.credits.len() - 1) as CreditId
    }

    /// The credit for an edit by `origin` now.
    fn credit_now(&mut self, origin: &Origin) -> CreditId {
        let now = (self.clock)();
        if let Some(live) = self.live {
            let credit = self.credit(live);
            if credit.origin == *origin && now.saturating_sub(credit.when) < CREDIT_SECONDS {
                return live;
            }
        }
        self.credits.push(Credit {
            origin: origin.clone(),
            when: now,
        });
        let id = (self.credits.len() - 1) as CreditId;
        self.live = Some(id);
        id
    }

    /// Of `marks`, the first written by someone else than `by`'s party.
    fn other(&self, by: CreditId, marks: &[Run]) -> Option<CreditId> {
        let own = party(&self.credit(by).origin);
        marks
            .iter()
            .map(|run| run.mark.writer())
            .find(|writer| party(&self.credit(*writer).origin) != own)
    }

    /// Works out the marks of `changes`, just made to a text that is now
    /// `text`, and changes the marks of the text accordingly: they are
    /// to become the changes of `state`, joining the current state of
    /// `history` if `join`. Returns them, and the records that go into
    /// the journal before the edit's own. Until [`Attribution::commit`]
    /// or [`Attribution::rollback`] is called, the credits made for them
    /// are not in the journal.
    pub(crate) fn prepare(
        &mut self,
        changes: &[Change],
        by: &By,
        history: &History,
        join: bool,
        text: &str,
        state: StateId,
    ) -> (Prepared, Vec<Record>) {
        let current = history.current();
        let last = join
            .then(|| history.changes(current).and_then(<[Change]>::last))
            .flatten();
        let last_marks = last.and_then(|_| self.marks_of(history, current).pop());
        let last_marks = last_marks.as_ref();
        let origin = by.origin();
        let mut marks = Vec::with_capacity(changes.len());
        for (index, change) in changes.iter().enumerate() {
            let old = if self.known {
                self.map.slice(change.at..change.at + change.old.len())
            } else {
                let mut old = Vec::new();
                push(&mut old, change.old.len(), Mark::UNKNOWN);
                old
            };
            let given = match by {
                By::Given { given, .. } => given.get(index).cloned().flatten(),
                By::Origin(_) => None,
            };
            let given = given.filter(|given| {
                given.iter().map(|(len, _)| len).sum::<usize>() == change.new.len()
            });
            let new = if let Some(given) = given {
                let mut new = Vec::new();
                for (len, credited) in given {
                    let mark = match credited {
                        Some(credited) => Mark {
                            by: self.number(&credited.by),
                            over: credited.over.as_ref().map(|over| self.number(over)),
                        },
                        None => Mark {
                            by: self.credit_now(origin),
                            over: None,
                        },
                    };
                    push(&mut new, len, mark);
                }
                new
            } else if change.new.is_empty() {
                Vec::new()
            } else {
                let joins = (index == 0)
                    .then_some(())
                    .and(last.zip(last_marks))
                    .filter(|(last, _)| absorbs(last, change));
                let credit = self.credit_now(origin);
                self.new_marks(change, &old, credit, joins.map(|(_, marks)| marks), {
                    (changes.len() == 1).then_some(text)
                })
            };
            let change_marks = ChangeMarks { old, new };
            if self.known {
                self.map
                    .replace(change.at..change.at + change.old.len(), &change_marks.new);
            }
            marks.push(change_marks);
        }
        let mut records: Vec<Record> = (self.journaled..self.credits.len())
            .map(|id| Record::Credit {
                id: id as CreditId,
                credit: self.credits[id].clone(),
            })
            .collect();
        records.push(Record::Marks {
            state,
            marks: marks.clone(),
        });
        let prepared = Prepared {
            marks,
            shapes: changes.iter().map(Shape::of).collect(),
        };
        (prepared, records)
    }

    /// The marks of what `change` puts in, made by `credit`: `old` are
    /// those of what it takes out, `joined` those of the change it is
    /// folded into, if it is; `text` the text after it, if it is the
    /// only change.
    fn new_marks(
        &self,
        change: &Change,
        old: &[Run],
        credit: CreditId,
        joined: Option<&ChangeMarks>,
        text: Option<&str>,
    ) -> Vec<Run> {
        let mut over = joined.and_then(|joined| {
            self.other(credit, &joined.old).or_else(|| {
                joined
                    .new
                    .last()
                    .filter(|run| run.mark.over.is_some())
                    .and_then(|run| self.other(credit, std::slice::from_ref(run)))
            })
        });
        if change.old.is_empty() {
            if over.is_none()
                && let Some(text) = text
            {
                over = self.within_a_word(change, credit, text);
            }
            let mut new = Vec::new();
            push(&mut new, change.new.len(), Mark { by: credit, over });
            return new;
        }
        // A replacement: the words it only kept keep their marks.
        let (from, to) = (tokens(&change.old), tokens(&change.new));
        let old_words: Vec<&str> = from.iter().map(|at| &change.old[at.clone()]).collect();
        let new_words: Vec<&str> = to.iter().map(|at| &change.new[at.clone()]).collect();
        let mut new = Vec::new();
        let (mut old_at, mut new_at) = (0, 0);
        // A blank or a sign is kept only beside a word that is kept: a
        // sentence rewritten is not the old one's for its full stop.
        let word = |token: &str| token.chars().any(char::is_alphanumeric);
        let aligned = aligned(&old_words, &new_words);
        let kept_words: std::collections::HashSet<usize> = aligned
            .iter()
            .filter(|(_, b)| word(new_words[*b]))
            .map(|(_, b)| *b)
            .collect();
        let kept: Vec<(usize, usize)> = aligned
            .into_iter()
            .filter(|(_, b)| {
                word(new_words[*b])
                    || b.checked_sub(1)
                        .is_some_and(|before| kept_words.contains(&before))
                    || kept_words.contains(&(b + 1))
            })
            .collect();
        let ends = kept.iter().copied().chain([(from.len(), to.len())]);
        for (index, (a, b)) in ends.enumerate() {
            if new_at < b {
                // Words put in the place of `old_at..a`.
                let taken = match (from.get(old_at), a.checked_sub(1).and_then(|l| from.get(l))) {
                    (Some(first), Some(last)) if old_at < a => slice(old, first.start..last.end),
                    _ => Vec::new(),
                };
                let mark = Mark {
                    by: credit,
                    over: self.other(credit, &taken).or(over),
                };
                push(&mut new, to[b - 1].end - to[new_at].start, mark);
            }
            if index < kept.len() {
                append(&mut new, &slice(old, from[a].clone()));
            }
            (old_at, new_at) = (a + 1, b + 1);
        }
        new
    }

    /// For an insertion into the middle of a word, the writer of that
    /// word if it is someone else.
    fn within_a_word(&self, change: &Change, credit: CreditId, text: &str) -> Option<CreditId> {
        let before = text.get(..change.at)?.chars().next_back()?;
        let after = text.get(change.at + change.new.len()..)?.chars().next()?;
        if !(before.is_alphanumeric() && after.is_alphanumeric()) {
            return None;
        }
        let left = self.map.at(change.at - before.len_utf8())?.writer();
        let right = self.map.at(change.at)?.writer();
        let parties = (
            party(&self.credit(left).origin),
            party(&self.credit(right).origin),
        );
        (parties.0 == parties.1 && parties.0 != party(&self.credit(credit).origin)).then_some(left)
    }

    /// The edit is in the journal: the credits made for it are too.
    pub(crate) fn commit(&mut self) {
        self.journaled = self.credits.len();
    }

    /// The edit could not be written down and was taken back: so is what
    /// it did to the marks.
    pub(crate) fn rollback(&mut self, prepared: &Prepared) {
        if self.known {
            for (shape, marks) in prepared.shapes.iter().zip(&prepared.marks).rev() {
                self.map.replace(shape.at..shape.at + shape.new, &marks.old);
            }
        }
        self.credits.truncate(self.journaled);
        if self
            .live
            .is_some_and(|live| live as usize >= self.credits.len())
        {
            self.live = None;
        }
    }

    fn node_mut(&mut self, state: StateId) -> &mut Vec<ChangeMarks> {
        let at = usize::try_from(state).unwrap_or(usize::MAX);
        if self.nodes.len() <= at {
            self.nodes.resize_with(at.saturating_add(1), Vec::new);
        }
        &mut self.nodes[at]
    }

    /// The marks of the changes of `state`, as many as it has changes,
    /// unknown where they are not known.
    fn marks_of(&self, history: &History, state: StateId) -> Vec<ChangeMarks> {
        let changes = history.changes(state).unwrap_or_default();
        let known = usize::try_from(state)
            .ok()
            .and_then(|at| self.nodes.get(at))
            .filter(|marks| {
                marks.len() == changes.len()
                    && marks
                        .iter()
                        .zip(changes)
                        .all(|(marks, change)| marks.fit(change))
            });
        match known {
            Some(marks) => marks.clone(),
            None => changes.iter().map(ChangeMarks::unknown).collect(),
        }
    }

    /// `state` was made with the changes of `prepared`.
    pub(crate) fn pushed(&mut self, state: StateId, prepared: Prepared) {
        *self.node_mut(state) = prepared.marks;
    }

    /// The marks of `state`'s changes, made to fit them where they do
    /// not (unknown): before changes are added to it.
    pub(crate) fn align(&mut self, history: &History, state: StateId) {
        let marks = self.marks_of(history, state);
        *self.node_mut(state) = marks;
    }

    /// A change of shape `shape` with `marks` was added to `state`,
    /// folded into its last change (of shape `last`) if `last` is given.
    pub(crate) fn extended(
        &mut self,
        state: StateId,
        last: Option<Shape>,
        shape: Shape,
        marks: ChangeMarks,
    ) {
        let node = self.node_mut(state);
        match (last, node.pop()) {
            (Some(last), Some(last_marks)) => {
                node.push(compose(last, &last_marks, shape, &marks));
            }
            (_, popped) => {
                node.extend(popped);
                node.push(marks);
            }
        }
    }

    /// The text went from state `from` to the current state of `history`
    /// the way the history leads: the marks go along.
    pub(crate) fn walk(&mut self, history: &History, from: StateId) {
        if !self.known {
            return;
        }
        let Some(path) = history.path(from, history.current()) else {
            return;
        };
        let mut at = from;
        for next in path {
            if history.parent(at) == Some(next) {
                self.step(history, at, Direction::Backward);
            } else {
                self.step(history, next, Direction::Forward);
            }
            at = next;
        }
    }

    /// Takes the changes of `state` in `direction` on the marks.
    fn step(&mut self, history: &History, state: StateId, direction: Direction) {
        let mut map = std::mem::take(&mut self.map);
        self.step_on(&mut map, history, state, direction);
        self.map = map;
    }

    /// Takes the changes of `state` in `direction` on `map`.
    fn step_on(&self, map: &mut Runs, history: &History, state: StateId, direction: Direction) {
        let changes = history.changes(state).unwrap_or_default();
        let marks = self.marks_of(history, state);
        match direction {
            Direction::Forward => {
                for (change, marks) in changes.iter().zip(&marks) {
                    map.replace(change.at..change.at + change.old.len(), &marks.new);
                }
            }
            Direction::Backward => {
                for (change, marks) in changes.iter().zip(&marks).rev() {
                    map.replace(change.at..change.at + change.new.len(), &marks.old);
                }
            }
        }
    }

    /// `map`, the marks of the text of state `from`, made those of state
    /// `to`: to look at another state without going there.
    pub fn walked(&self, history: &History, map: &mut Runs, from: StateId, to: StateId) {
        let Some(path) = history.path(from, to) else {
            return;
        };
        let mut at = from;
        for next in path {
            if history.parent(at) == Some(next) {
                self.step_on(map, history, at, Direction::Backward);
            } else {
                self.step_on(map, history, next, Direction::Forward);
            }
            at = next;
        }
    }

    /// The state whose step put byte `offset` of the current text there,
    /// with the mark it has: going back from the current state, the
    /// first step that brought it in and did not only keep it. `None` if
    /// it was there before the history began.
    #[must_use]
    pub fn introduced(&self, history: &History, offset: usize) -> Option<StateId> {
        let mark = self.map.at(offset)?;
        let mut offset = offset;
        let mut at = history.current();
        while let Some(parent) = history.parent(at) {
            let changes = history.changes(at).unwrap_or_default();
            let marks = self.marks_of(history, at);
            for (change, marks) in changes.iter().zip(&marks).rev() {
                let end = change.at + change.new.len();
                if offset < change.at {
                    continue;
                }
                if offset >= end {
                    offset = offset - change.new.len() + change.old.len();
                    continue;
                }
                let kept = marks.old.iter().any(|run| run.mark == mark);
                if !kept {
                    return Some(at);
                }
                // Only kept: where it was before, as near as it can be
                // told.
                offset = change.at + (offset - change.at).min(change.old.len().saturating_sub(1));
            }
            at = parent;
        }
        None
    }

    /// Notes what replaying `record` will need of the history as it is.
    pub(crate) fn before(&self, record: &Record, history: &History) -> Before {
        let current = history.current();
        let mut node = Vec::new();
        let mut last = None;
        if matches!(record, Record::Extend { .. }) {
            node = self.marks_of(history, current);
            let change = history.changes(current).and_then(<[Change]>::last).cloned();
            last = change.zip(node.pop());
        }
        Before {
            from: current,
            len: history.len(),
            last,
            node,
        }
    }

    /// Follows `record` of a journal, just replayed onto `history`; `text`
    /// is the text, if it is known yet.
    pub(crate) fn after(
        &mut self,
        before: Before,
        record: &Record,
        history: &History,
        text: Option<&str>,
    ) {
        let current = history.current();
        match record {
            Record::Credit { id, credit } => {
                let at = *id as usize;
                if self.credits.len() <= at {
                    self.credits.resize(at + 1, Credit::unknown());
                }
                self.credits[at] = credit.clone();
                self.journaled = self.credits.len();
            }
            Record::Marks { state, marks } => self.pending = Some((*state, marks.clone())),
            Record::Map {
                state: _,
                credits,
                runs,
            } => {
                for (id, credit) in credits {
                    let at = *id as usize;
                    if self.credits.len() <= at {
                        self.credits.resize(at + 1, Credit::unknown());
                    }
                    self.credits[at] = credit.clone();
                }
                self.journaled = self.credits.len();
                if text.is_some_and(|text| text.len() == total(runs)) {
                    self.map = Runs::of(runs);
                    self.known = true;
                    if let Some((state, start)) = self.diffed.take()
                        && state == current
                        && let Some(node) = self.nodes.get_mut(state as usize)
                        && let (Some(marks), Some(change)) = (
                            node.first_mut(),
                            history.changes(state).and_then(<[Change]>::first),
                        )
                    {
                        marks.new = slice(runs, start..start + change.new.len());
                    }
                }
            }
            Record::Push { id, changes, .. } | Record::Extend { state: id, changes } => {
                let given = self
                    .pending
                    .take()
                    .filter(|(state, _)| state == id)
                    .map(|(_, marks)| marks)
                    .filter(|marks| {
                        marks.len() == changes.len()
                            && marks
                                .iter()
                                .zip(changes)
                                .all(|(marks, change)| marks.fit(change))
                    });
                let marks: Vec<ChangeMarks> =
                    given.unwrap_or_else(|| changes.iter().map(ChangeMarks::unknown).collect());
                if self.known && text.is_some() {
                    for (change, marks) in changes.iter().zip(&marks) {
                        self.map
                            .replace(change.at..change.at + change.old.len(), &marks.new);
                    }
                }
                if history.len() > before.len {
                    *self.node_mut(current) = marks;
                } else {
                    *self.node_mut(current) = fold(before.node, before.last, changes, marks);
                }
            }
            Record::Goto { .. } => self.walk(history, before.from),
            Record::Snapshot { text: snapshot, .. } => {
                if self.known && text.is_some() {
                    self.walk(history, before.from);
                }
                if !self.known || self.map.len() != snapshot.len() {
                    self.map = Runs::of(&[Run {
                        len: snapshot.len() as u32,
                        mark: Mark::UNKNOWN,
                    }]);
                    self.known = true;
                }
            }
            Record::Prefer { .. } | Record::Saved { .. } => {}
        }
    }

    /// A damaged journal is salvaged from a snapshot of a text of `len`
    /// bytes; with `diffed`, the snapshot's text was made a step from
    /// the root (the file's text), whose change begins at that byte.
    pub(crate) fn salvaged(
        len: usize,
        diffed: Option<(StateId, usize)>,
        history: &History,
    ) -> Self {
        let mut made = Self {
            map: Runs::of(&[Run {
                len: len as u32,
                mark: Mark::UNKNOWN,
            }]),
            known: true,
            diffed,
            ..Self::default()
        };
        if let Some((state, _)) = diffed {
            *made.node_mut(state) = made.marks_of(history, state);
        }
        made
    }

    /// What [`History::thinned`] made of `history`, by `map`, for the
    /// marks: the changes it folds together, it folds their marks
    /// together too.
    pub(crate) fn thinned(&self, history: &History, map: &[Option<StateId>]) -> Self {
        let len = history.len();
        let mut carried: Vec<Vec<(Change, ChangeMarks)>> = vec![Vec::new(); len];
        let mut nodes = Vec::new();
        for id in 0..len {
            let state = id as StateId;
            let Some(parent) = history.parent(state).and_then(|p| usize::try_from(p).ok()) else {
                continue;
            };
            let mut changes = std::mem::take(&mut carried[parent]);
            let marks = self.marks_of(history, state);
            for (change, marks) in history.changes(state).unwrap_or_default().iter().zip(marks) {
                let merged = changes.last_mut().is_some_and(|(last, last_marks)| {
                    let shape = Shape::of(last);
                    let merged = last.absorb(change);
                    if merged {
                        *last_marks = compose(shape, last_marks, Shape::of(change), &marks);
                    }
                    merged
                });
                if !merged {
                    changes.push((change.clone(), marks));
                }
            }
            match map.get(id).copied().flatten() {
                Some(new) => {
                    let new = new as usize;
                    if nodes.len() <= new {
                        nodes.resize_with(new + 1, Vec::new);
                    }
                    nodes[new] = changes.into_iter().map(|(_, marks)| marks).collect();
                }
                None => carried[id] = changes,
            }
        }
        Self {
            nodes,
            pending: None,
            diffed: None,
            ..self.clone()
        }
    }

    /// The records of every credit, for a journal that is written anew:
    /// they go first, so that whatever part of it is read names no
    /// credit it has not read.
    pub(crate) fn credit_records(&self) -> impl Iterator<Item = Record> + '_ {
        self.credits
            .iter()
            .enumerate()
            .skip(1)
            .map(|(id, credit)| Record::Credit {
                id: id as CreditId,
                credit: credit.clone(),
            })
    }

    /// The record of the marks of `state`'s changes, for a journal that
    /// is written anew; it goes before the state's own.
    pub(crate) fn marks_record(&self, history: &History, state: StateId) -> Record {
        Record::Marks {
            state,
            marks: self.marks_of(history, state),
        }
    }

    /// The record of the attribution of the text of `state`, the current
    /// one, with every credit: it goes behind the snapshot of the text.
    pub(crate) fn map_record(&self, state: StateId) -> Record {
        Record::Map {
            state,
            credits: self
                .credits
                .iter()
                .enumerate()
                .skip(1)
                .map(|(id, credit)| (id as CreditId, credit.clone()))
                .collect(),
            runs: self.map.flat(),
        }
    }
}

/// The marks of a state whose changes had `node` as marks and then
/// `last`, when `changes` with `marks` are added to it: folded into its
/// last change as the history folds them.
fn fold(
    mut node: Vec<ChangeMarks>,
    mut last: Option<(Change, ChangeMarks)>,
    changes: &[Change],
    marks: Vec<ChangeMarks>,
) -> Vec<ChangeMarks> {
    for (change, marks) in changes.iter().zip(marks) {
        match &mut last {
            Some((folded, folded_marks)) => {
                let shape = Shape::of(folded);
                if folded.absorb(change) {
                    *folded_marks = compose(shape, folded_marks, Shape::of(change), &marks);
                } else {
                    node.push(std::mem::take(folded_marks));
                    last = Some((change.clone(), marks));
                }
            }
            None => last = Some((change.clone(), marks)),
        }
    }
    node.extend(last.map(|(_, marks)| marks));
    node
}

/// Whether [`Change::absorb`] would fold `next` into `last`, asked
/// without changing (or copying) either.
pub(crate) fn absorbs(last: &Change, next: &Change) -> bool {
    let end = last.at + last.new.len();
    (next.old.is_empty() && next.at == end)
        || (next.new.is_empty() && last.new.is_empty() && next.at == last.at)
        || (next.new.is_empty() && last.new.is_empty() && next.at + next.old.len() == last.at)
        || (next.new.is_empty()
            && next.at >= last.at
            && next.at + next.old.len() == end
            && last.new.ends_with(next.old.as_str()))
}

/// The marks of the change that `last` (with `last_marks`) and `next`,
/// made right after it, become when they are folded into one: they touch
/// or overlap, so in the text between them (after `last`, before `next`)
/// the two make up one stretch of bytes.
fn compose(
    last: Shape,
    last_marks: &ChangeMarks,
    next: Shape,
    next_marks: &ChangeMarks,
) -> ChangeMarks {
    let start = last.at.min(next.at);
    let end = (last.at + last.new).max(next.at + next.old);
    // The stretch in the text between the two changes.
    let mut between = Vec::new();
    if next.at > start {
        append(&mut between, &slice(&last_marks.new, 0..next.at - last.at));
    }
    append(&mut between, &next_marks.old);
    if end > next.at + next.old {
        let from = next.at + next.old - last.at;
        append(&mut between, &slice(&last_marks.new, from..last.new));
    }
    let old = spliced(
        &between,
        last.at - start..last.at - start + last.new,
        &last_marks.old,
    );
    let new = spliced(
        &between,
        next.at - start..next.at - start + next.old,
        &next_marks.new,
    );
    debug_assert_eq!(total(&between), end - start);
    ChangeMarks { old, new }
}

/// The words of `text`, each with the blanks and signs between them as
/// words of their own: runs of letters and digits, runs of blanks, and
/// every other character by itself.
fn tokens(text: &str) -> Vec<Range<usize>> {
    #[derive(PartialEq)]
    enum Class {
        Word,
        Blank,
        Sign,
    }
    let class = |c: char| {
        if c.is_alphanumeric() {
            Class::Word
        } else if c.is_whitespace() {
            Class::Blank
        } else {
            Class::Sign
        }
    };
    let mut tokens: Vec<Range<usize>> = Vec::new();
    let mut last = None;
    for (at, c) in text.char_indices() {
        let this = class(c);
        let goes_on = last.as_ref() == Some(&this) && this != Class::Sign;
        match tokens.last_mut() {
            Some(token) if goes_on => token.end = at + c.len_utf8(),
            _ => tokens.push(at..at + c.len_utf8()),
        }
        last = Some(this);
    }
    tokens
}

/// The most differences [`aligned`] looks for; beyond them only what two
/// texts have in common at their start and their end counts as kept.
const MAX_DIFFERENCES: usize = 1000;

/// Which words of `a` are kept in `b`, as pairs of their places, in
/// order: a longest common sequence of words (Myers' way), or where the
/// two differ too much, what they have in common at start and end.
fn aligned(a: &[&str], b: &[&str]) -> Vec<(usize, usize)> {
    let head = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let tail = a[head..]
        .iter()
        .rev()
        .zip(b[head..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (mid_a, mid_b) = (&a[head..a.len() - tail], &b[head..b.len() - tail]);
    let mut pairs: Vec<(usize, usize)> = (0..head).map(|at| (at, at)).collect();
    if let Some(middle) = myers(mid_a, mid_b) {
        pairs.extend(middle.into_iter().map(|(x, y)| (x + head, y + head)));
    }
    pairs.extend((0..tail).map(|back| (a.len() - tail + back, b.len() - tail + back)));
    pairs
}

/// The pairs of places of a longest common sequence of `a` and `b`, if
/// they have one within [`MAX_DIFFERENCES`] of each other.
#[allow(
    clippy::many_single_char_names,
    clippy::cast_possible_wrap,
    reason = "the names of Myers' paper; counts of words are far below isize::MAX"
)]
fn myers(a: &[&str], b: &[&str]) -> Option<Vec<(usize, usize)>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    if n == 0 || m == 0 {
        return Some(Vec::new());
    }
    let max = (MAX_DIFFERENCES as isize).min(n + m);
    let offset = max + 1;
    let mut v = vec![0_isize; 2 * offset as usize + 1];
    // For each number of differences, the furthest place on each
    // diagonal from -d to d.
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut reached = None;
    'search: for d in 0..=max {
        for k in (-d..=d).step_by(2) {
            let index = (k + offset) as usize;
            let mut x = if k == -d || (k != d && v[index - 1] < v[index + 1]) {
                v[index + 1]
            } else {
                v[index - 1] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[index] = x;
            if x >= n && y >= m {
                trace.push(v[(offset - d) as usize..=(offset + d) as usize].to_vec());
                reached = Some(d);
                break 'search;
            }
        }
        trace.push(v[(offset - d) as usize..=(offset + d) as usize].to_vec());
    }
    let reached = reached?;
    // Back from the end, the diagonal steps are the words kept.
    let mut pairs = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (1..=reached).rev() {
        let previous = &trace[d as usize - 1];
        let at = |k: isize| previous[(k + d - 1) as usize];
        let k = x - y;
        let down = k == -d || (k != d && at(k - 1) < at(k + 1));
        let prev_k = if down { k + 1 } else { k - 1 };
        let prev_x = at(prev_k);
        let prev_y = prev_x - prev_k;
        let (start_x, start_y) = if down {
            (prev_x, prev_y + 1)
        } else {
            (prev_x + 1, prev_y)
        };
        while x > start_x && y > start_y {
            x -= 1;
            y -= 1;
            pairs.push((x as usize, y as usize));
        }
        (x, y) = (prev_x, prev_y);
    }
    while x > 0 && y > 0 {
        x -= 1;
        y -= 1;
        pairs.push((x as usize, y as usize));
    }
    pairs.reverse();
    Some(pairs)
}

/// Puts `changes`, with the marks `prepared` for them, into `history`:
/// as a new state, or with `join` into the current one. Returns the
/// state they are in.
pub(crate) fn record(
    history: &mut History,
    origins: &mut Attribution,
    changes: Vec<Change>,
    prepared: Prepared,
    join: bool,
) -> StateId {
    if !join {
        let state = history.push(changes);
        origins.pushed(state, prepared);
        return state;
    }
    let state = history.current();
    origins.align(history, state);
    for (change, marks) in changes.into_iter().zip(prepared.marks) {
        let before = history.changes(state).unwrap_or_default();
        let (count, last) = (before.len(), before.last().map(Shape::of));
        let shape = Shape::of(&change);
        history.extend(vec![change]);
        let folded = history.changes(state).map_or(0, <[Change]>::len) == count;
        origins.extended(state, last.filter(|_| folded), shape, marks);
    }
    state
}

// ---- In the journal --------------------------------------------------------

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_str(out: &mut Vec<u8>, text: &str) {
    put_u32(out, text.len() as u32);
    out.extend_from_slice(text.as_bytes());
}

fn take<'a>(bytes: &mut &'a [u8], count: usize) -> Option<&'a [u8]> {
    let (taken, rest) = bytes.split_at_checked(count)?;
    *bytes = rest;
    Some(taken)
}

fn take_u32(bytes: &mut &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(take(bytes, 4)?.try_into().ok()?))
}

fn take_u64(bytes: &mut &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(take(bytes, 8)?.try_into().ok()?))
}

fn take_str(bytes: &mut &[u8]) -> Option<String> {
    let len = take_u32(bytes)? as usize;
    String::from_utf8(take(bytes, len)?.to_vec()).ok()
}

/// Writes `credit` for the journal.
pub(crate) fn put_credit(out: &mut Vec<u8>, credit: &Credit) {
    out.extend_from_slice(&credit.when.to_le_bytes());
    let agent = |out: &mut Vec<u8>, tag: u8, agent: &Agent| {
        out.push(tag);
        put_str(out, &agent.name);
        put_str(out, &agent.version);
        put_str(out, &agent.session);
    };
    match &credit.origin {
        Origin::Unknown => out.push(0),
        Origin::Author => out.push(1),
        Origin::Zetwerk(what) => {
            out.push(2);
            put_str(out, what);
        }
        Origin::Pasted => out.push(3),
        Origin::Agent(who) => agent(out, 4, who),
        Origin::Accepted(who) => agent(out, 5, who),
        Origin::Outside => out.push(6),
    }
}

pub(crate) fn take_credit(bytes: &mut &[u8]) -> Option<Credit> {
    let when = take_u64(bytes)?;
    let tag = take(bytes, 1)?[0];
    let mut agent = || -> Option<Agent> {
        Some(Agent {
            name: take_str(bytes)?,
            version: take_str(bytes)?,
            session: take_str(bytes)?,
        })
    };
    let origin = match tag {
        0 => Origin::Unknown,
        1 => Origin::Author,
        2 => Origin::Zetwerk(take_str(bytes)?),
        3 => Origin::Pasted,
        4 => Origin::Agent(agent()?),
        5 => Origin::Accepted(agent()?),
        6 => Origin::Outside,
        _ => return None,
    };
    Some(Credit { origin, when })
}

/// Writes `runs` for the journal: per run its length, its writer and
/// whose text it took the place of (`u32::MAX` for no one).
pub(crate) fn put_runs(out: &mut Vec<u8>, runs: &[Run]) {
    put_u32(out, runs.len() as u32);
    for run in runs {
        put_u32(out, run.len);
        put_u32(out, run.mark.by);
        put_u32(out, run.mark.over.unwrap_or(u32::MAX));
    }
}

pub(crate) fn take_runs(bytes: &mut &[u8]) -> Option<Vec<Run>> {
    let count = take_u32(bytes)? as usize;
    // Each run takes 12 bytes: a count beyond what is left is damage.
    if count > bytes.len() / 12 {
        return None;
    }
    (0..count)
        .map(|_| {
            let len = take_u32(bytes)?;
            let by = take_u32(bytes)?;
            let over = take_u32(bytes)?;
            Some(Run {
                len,
                mark: Mark {
                    by,
                    over: (over != u32::MAX).then_some(over),
                },
            })
        })
        .collect()
}

pub(crate) fn put_marks(out: &mut Vec<u8>, marks: &[ChangeMarks]) {
    put_u32(out, marks.len() as u32);
    for marks in marks {
        put_runs(out, &marks.old);
        put_runs(out, &marks.new);
    }
}

pub(crate) fn take_marks(bytes: &mut &[u8]) -> Option<Vec<ChangeMarks>> {
    let count = take_u32(bytes)? as usize;
    if count > bytes.len() / 8 {
        return None;
    }
    (0..count)
        .map(|_| {
            Some(ChangeMarks {
                old: take_runs(bytes)?,
                new: take_runs(bytes)?,
            })
        })
        .collect()
}

pub(crate) fn put_credits(out: &mut Vec<u8>, credits: &[(CreditId, Credit)]) {
    put_u32(out, credits.len() as u32);
    for (id, credit) in credits {
        put_u32(out, *id);
        put_credit(out, credit);
    }
}

pub(crate) fn take_credits(bytes: &mut &[u8]) -> Option<Vec<(CreditId, Credit)>> {
    let count = take_u32(bytes)? as usize;
    if count > bytes.len() / 13 {
        return None;
    }
    (0..count)
        .map(|_| Some((take_u32(bytes)?, take_credit(bytes)?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(len: u32, by: CreditId) -> Run {
        Run {
            len,
            mark: Mark { by, over: None },
        }
    }

    #[test]
    fn runs_in_pieces_are_runs() {
        let mut runs = Runs::default();
        let mut flat: Vec<Run> = Vec::new();
        // Enough different runs for many pieces.
        for at in 0..3000_u32 {
            let place = (at as usize * 7919) % (runs.len() + 1);
            let with = [run(1 + at % 3, at % 5)];
            runs.replace(place..place, &with);
            flat = spliced(&flat, place..place, &with);
            if at % 7 == 0 {
                let end = (place + 4).min(runs.len());
                runs.replace(place..end, &[]);
                flat = spliced(&flat, place..end, &[]);
            }
        }
        assert!(runs.pieces.len() > 4);
        assert_eq!(runs.flat(), flat);
        assert_eq!(runs.len(), total(&flat));
        assert_eq!(runs.slice(100..900), slice(&flat, 100..900));
        let seen: Vec<Run> = runs
            .iter()
            .map(|(at, mark)| Run {
                len: at.len() as u32,
                mark,
            })
            .collect();
        let mut merged = Vec::new();
        append(&mut merged, &seen);
        assert_eq!(merged, flat);
    }

    #[test]
    fn words_that_stay_keep_their_marks() {
        let a = ["the", " ", "cat", " ", "sat", " ", "here"];
        let b = ["the", " ", "dog", " ", "sat", " ", "there", "!"];
        assert_eq!(
            aligned(&a, &b),
            vec![(0, 0), (1, 1), (3, 3), (4, 4), (5, 5)]
        );
        let many: Vec<String> = (0..5000).map(|n| n.to_string()).collect();
        let other: Vec<String> = (0..5000).map(|n| (n + 1).to_string()).collect();
        let (many, other): (Vec<&str>, Vec<&str>) = (
            many.iter().map(String::as_str).collect(),
            other.iter().map(String::as_str).collect(),
        );
        assert_eq!(aligned(&many, &other).len(), 4999);
        assert_eq!(
            tokens("a cat, sat.")
                .into_iter()
                .map(|r| &"a cat, sat."[r])
                .collect::<Vec<_>>(),
            ["a", " ", "cat", ",", " ", "sat", "."]
        );
    }

    #[test]
    fn what_folds_is_known_before_folding() {
        let texts = ["", "a", "ab", "é", "ba"];
        let mut checked = 0;
        for base in ["abcab", "ééab", ""] {
            for at in 0..=base.len() {
                for old in texts {
                    for new in texts {
                        for next_at in 0..=base.len() + 2 {
                            for next_old in texts {
                                for next_new in texts {
                                    let last = Change {
                                        at,
                                        old: old.to_owned(),
                                        new: new.to_owned(),
                                    };
                                    let next = Change {
                                        at: next_at,
                                        old: next_old.to_owned(),
                                        new: next_new.to_owned(),
                                    };
                                    let mut folded = last.clone();
                                    assert_eq!(absorbs(&last, &next), folded.absorb(&next));
                                    checked += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(checked > 10_000);
    }

    #[test]
    fn a_journal_from_before_origins_reads_as_before() {
        use crate::journal::Record;
        let root = std::env::temp_dir().join(format!("zetwerk-old-journal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("work")).unwrap_or_else(|err| panic!("{err}"));
        let file = root.join("work/doc.typ");
        std::fs::write(&file, "ab").unwrap_or_else(|err| panic!("{err}"));
        // A journal as zetwerk wrote it before origins were recorded:
        // two steps on an empty text, the second unsaved.
        let push = |id, text: &str, at| Record::Push {
            id,
            parent: id - 1,
            changes: vec![Change {
                at,
                old: String::new(),
                new: text.to_owned(),
            }],
        };
        let records = [
            push(1, "a", 0),
            push(2, "b", 1),
            Record::Snapshot {
                state: 2,
                text: "ab".to_owned(),
            },
        ];
        let state = root.join("state");
        let (_, _) = crate::Document::open(&root.join("work/other.typ"), &state, |text| text)
            .unwrap_or_else(|err| panic!("{err}"));
        let journals = state.join("journals");
        let name = blake3::hash(
            std::fs::canonicalize(&file)
                .unwrap_or_else(|err| panic!("{err}"))
                .as_os_str()
                .as_encoded_bytes(),
        )
        .to_hex();
        let mut bytes = b"ZWJOURN1".to_vec();
        for record in &records {
            bytes.extend_from_slice(&record.encode());
        }
        std::fs::write(journals.join(format!("{}.journal", &name[..32])), bytes)
            .unwrap_or_else(|err| panic!("{err}"));
        let (mut document, _) =
            crate::Document::open(&file, &state, |text| text).unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(document.text(), "ab");
        assert_eq!(document.history().len(), 3, "the history is all there");
        let unknown = |document: &crate::Document<String>| {
            let origins = document.origins();
            origins.map().iter().all(|(_, mark)| {
                origins.kind(mark) == Kind::Unknown && origins.credit(mark.by).when == 0
            })
        };
        assert!(unknown(&document), "nothing is said of what is not known");
        assert_eq!(document.origins().map().len(), 2);
        document.undo().unwrap_or_else(|err| panic!("{err}"));
        assert!(unknown(&document) && document.origins().map().len() == 1);
        document.redo().unwrap_or_else(|err| panic!("{err}"));
        document
            .edit(&[crate::Replace::insert(2, "c")], false)
            .unwrap_or_else(|err| panic!("{err}"));
        let origins = document.origins();
        assert_eq!(
            origins.map().at(2).map(|mark| origins.kind(mark)),
            Some(Kind::Own)
        );
        drop(document);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn credits_and_marks_survive_the_journal() {
        let credit = Credit {
            origin: Origin::Accepted(Agent {
                name: "Claude Code".into(),
                version: "2.1".into(),
                session: "s1".into(),
            }),
            when: 1_763_200_000,
        };
        let mut bytes = Vec::new();
        put_credit(&mut bytes, &credit);
        let marks = vec![ChangeMarks {
            old: vec![run(3, 1)],
            new: vec![Run {
                len: 2,
                mark: Mark {
                    by: 2,
                    over: Some(1),
                },
            }],
        }];
        put_marks(&mut bytes, &marks);
        let mut read = bytes.as_slice();
        assert_eq!(take_credit(&mut read), Some(credit));
        assert_eq!(take_marks(&mut read), Some(marks));
        assert_eq!(read, &[] as &[u8]);
    }
}
