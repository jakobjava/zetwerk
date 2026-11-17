//! Where reviewing is kept: beside the journal, in the state directory.
//!
//! Each file of a document has a store of its own, as its notes do
//! (`review/<hash>.review`): its suggestions with their threads, the
//! reviews asked of it, and the instructions made for the document in
//! it. The instructions for all of the author's documents are in one
//! file there (`review/brief`). A store is lines of a field's name, a
//! tab and its value, with what would break it into lines written as
//! escapes; what a later version writes and this one does not know is
//! passed over.

use std::fmt::Write as _;
use std::ops::Range;
use std::path::{Path, PathBuf};

use typst::syntax::FileId;

use super::super::agent::Proposal;
use super::super::notes::{escape, unescape, write_store};
use super::{Entry, Instruction, Kind, Level, Request, Said, State, Suggestion, Viewer, Who, part};

/// What a store begins with, and the version of its form.
const HEADER: &str = "zetwerk review 1";
const BRIEF_HEADER: &str = "zetwerk brief 1";
/// How far from where it was a suggestion's text is looked for in a
/// file that changed while it was closed, in bytes.
const REACH: usize = 4000;

/// Where the store of the file at `path` is under `state_dir`.
pub(super) fn store_path(state_dir: &Path, path: &Path) -> PathBuf {
    // FNV-1a: a name that is the same in every run, for every path.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    state_dir.join("review").join(format!("{hash:016x}.review"))
}

/// Where the instructions for all documents are.
fn brief_path(state_dir: &Path) -> PathBuf {
    state_dir.join("review").join("brief")
}

fn flag(on: bool) -> &'static str {
    if on { "yes" } else { "no" }
}

/// Who as the store writes it.
fn who(by: &Who) -> String {
    match by {
        Who::Author => "author".to_owned(),
        Who::Agent(name) => format!("agent:{}", escape(name)),
    }
}

fn who_of(text: &str) -> Who {
    text.strip_prefix("agent:")
        .map_or(Who::Author, |name| Who::Agent(unescape(name)))
}

/// A step of a thread as one line: its count, when, who, what and up to
/// three texts.
fn entry_line(entry: &Entry) -> String {
    let texts: Vec<String> = match &entry.said {
        Said::Message(text) | Said::Rejected(text) | Said::Instruction(text) => {
            vec![text.clone()]
        }
        Said::Version { text, reason } => vec![text.clone(), reason.clone()],
        Said::Alternatives(all) => all.clone(),
        Said::Accepted { before, after } => vec![before.clone(), after.clone()],
        Said::Rewritten {
            before,
            offered,
            after,
        } => vec![before.clone(), offered.clone(), after.clone()],
        Said::Made | Said::AskedAlternatives | Said::Later | Said::Reopened | Said::Withdrawn => {
            Vec::new()
        }
    };
    let mut line = format!(
        "entry\t{}\t{}\t{}\t{}",
        entry.seq,
        entry.at,
        who(&entry.by),
        entry.said.tag()
    );
    for text in texts {
        let _ = write!(line, "\t{}", escape(&text));
    }
    line
}

fn entry_of(value: &str) -> Option<Entry> {
    let mut fields = value.split('\t');
    let seq = fields.next()?.parse().ok()?;
    let at = fields.next()?.parse().ok()?;
    let by = who_of(fields.next()?);
    let tag = fields.next()?;
    let texts: Vec<String> = fields.map(unescape).collect();
    let text = |index: usize| texts.get(index).cloned().unwrap_or_default();
    let said = match tag {
        "made" => Said::Made,
        "message" => Said::Message(text(0)),
        "revised" => Said::Version {
            text: text(0),
            reason: text(1),
        },
        "alternatives" => Said::Alternatives(texts.clone()),
        "asked_alternatives" => Said::AskedAlternatives,
        "accepted" => Said::Accepted {
            before: text(0),
            after: text(1),
        },
        "rewritten" => Said::Rewritten {
            before: text(0),
            offered: text(1),
            after: text(2),
        },
        "rejected" => Said::Rejected(text(0)),
        "later" => Said::Later,
        "reopened" => Said::Reopened,
        "withdrawn" => Said::Withdrawn,
        "instruction" => Said::Instruction(text(0)),
        _ => return None,
    };
    Some(Entry { seq, at, by, said })
}

/// What a store holds.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Stored {
    pub(super) seq: u64,
    pub(super) suggestions: Vec<Suggestion>,
    pub(super) requests: Vec<Request>,
    pub(super) brief: Vec<Instruction>,
}

/// An instruction's lines.
fn instruction_lines(out: &mut String, instruction: &Instruction) {
    let _ = writeln!(out, "instruction\t{}", instruction.id);
    let _ = writeln!(out, "text\t{}", escape(&instruction.text));
    let _ = writeln!(out, "everywhere\t{}", flag(instruction.everywhere));
    if let Some(category) = &instruction.category {
        let _ = writeln!(out, "category\t{}", escape(category));
    }
    if let Some((range, quote)) = &instruction.protect {
        let _ = writeln!(
            out,
            "protect\t{}\t{}\t{}",
            range.start,
            range.end,
            escape(quote)
        );
    }
    if let Some((old, new)) = &instruction.refused {
        let _ = writeln!(out, "refused\t{}\t{}", escape(old), escape(new));
    }
    if let Some(from) = instruction.from {
        let _ = writeln!(out, "from\t{from}");
    }
    let _ = writeln!(out, "made\t{}", instruction.at);
    let _ = writeln!(out, "seq\t{}", instruction.seq);
    let _ = writeln!(out, "end");
}

/// The store of the file at `path` as it is written.
pub(super) fn stored(path: &Path, seq: u64, stored: &Stored) -> String {
    let mut out = format!(
        "{HEADER}\npath\t{}\nseq\t{seq}\n",
        escape(&path.to_string_lossy())
    );
    for suggestion in &stored.suggestions {
        let _ = writeln!(out, "suggestion\t{}", suggestion.id);
        let _ = writeln!(out, "kind\t{}", suggestion.kind.name());
        let _ = writeln!(
            out,
            "range\t{}\t{}",
            suggestion.range.start, suggestion.range.end
        );
        for (name, value) in [
            ("quote", &suggestion.quote),
            ("new", &suggestion.new),
            ("offered", &suggestion.offered),
            ("context", &suggestion.context),
            ("reason", &suggestion.reason),
            ("category", &suggestion.category),
            ("by", &suggestion.by),
            ("session", &suggestion.session),
            ("section", &suggestion.section),
        ] {
            let _ = writeln!(out, "{name}\t{}", escape(value));
        }
        for alternative in &suggestion.alternatives {
            let _ = writeln!(out, "alternative\t{}", escape(alternative));
        }
        let _ = writeln!(out, "shown\t{}", suggestion.shown);
        let _ = writeln!(out, "confidence\t{}", suggestion.confidence.sure());
        let _ = writeln!(out, "importance\t{}", suggestion.importance.weight());
        let _ = writeln!(out, "made\t{}", suggestion.made);
        let _ = writeln!(out, "state\t{}", suggestion.state.name());
        let _ = writeln!(out, "waiting\t{}", flag(suggestion.waiting));
        let _ = writeln!(out, "unseen\t{}", flag(suggestion.unseen));
        if let Some(request) = suggestion.request {
            let _ = writeln!(out, "request\t{request}");
        }
        if let Some(note) = suggestion.note {
            let _ = writeln!(out, "note\t{note}");
        }
        for entry in &suggestion.thread {
            let _ = writeln!(out, "{}", entry_line(entry));
        }
        let _ = writeln!(out, "end");
    }
    for request in &stored.requests {
        let _ = writeln!(out, "request\t{}", request.id);
        let _ = writeln!(out, "range\t{}\t{}", request.range.start, request.range.end);
        let _ = writeln!(out, "quote\t{}", escape(&request.quote));
        let _ = writeln!(out, "what\t{}", escape(&request.what));
        let _ = writeln!(out, "words\t{}", escape(&request.words));
        let _ = writeln!(out, "section\t{}", escape(&request.section));
        let _ = writeln!(out, "made\t{}", request.at);
        let _ = writeln!(out, "seq\t{}", request.seq);
        let _ = writeln!(out, "done\t{}", flag(request.done));
        let _ = writeln!(out, "end");
    }
    for instruction in &stored.brief {
        instruction_lines(&mut out, instruction);
    }
    out
}

/// The instructions for all documents as their file is written.
pub(super) fn stored_brief(brief: &[&Instruction]) -> String {
    let mut out = format!("{BRIEF_HEADER}\n");
    for instruction in brief {
        instruction_lines(&mut out, instruction);
    }
    out
}

/// What is being read: the thing begun last.
enum Reading {
    Nothing,
    Suggestion(Box<Suggestion>),
    Request(Request),
    Instruction(Instruction),
}

fn range_of(value: &str) -> Range<usize> {
    let mut fields = value.split('\t');
    let start = fields.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    let end = fields.next().and_then(|n| n.parse().ok()).unwrap_or(start);
    start..end.max(start)
}

/// What the store `text` holds, for `file`. An error if it is not a
/// store of reviewing (or of the brief, with `brief`).
#[allow(
    clippy::too_many_lines,
    reason = "every field of every thing a store holds"
)]
pub(super) fn parse(text: &str, file: Option<FileId>, brief: bool) -> Result<Stored, String> {
    let mut lines = text.lines();
    let header = if brief { BRIEF_HEADER } else { HEADER };
    if lines.next() != Some(header) {
        return Err("it is not a store of reviewing this zetwerk can read".to_owned());
    }
    let mut stored = Stored::default();
    let mut reading = Reading::Nothing;
    for line in lines {
        let (key, value) = line.split_once('\t').unwrap_or((line, ""));
        let number = value.parse::<u64>().ok();
        match (key, &mut reading, file) {
            ("seq", Reading::Nothing, _) => stored.seq = number.unwrap_or(0),
            ("suggestion", _, Some(file)) => {
                reading = Reading::Suggestion(Box::new(Suggestion {
                    id: number.unwrap_or(0),
                    file,
                    kind: Kind::Comment,
                    range: 0..0,
                    quote: String::new(),
                    new: String::new(),
                    context: String::new(),
                    offered: String::new(),
                    alternatives: Vec::new(),
                    shown: 0,
                    reason: String::new(),
                    category: String::new(),
                    confidence: Level::Medium,
                    importance: Level::Medium,
                    by: String::new(),
                    session: String::new(),
                    made: 0,
                    state: State::Open,
                    thread: Vec::new(),
                    proposal: None,
                    request: None,
                    note: None,
                    flagged: Vec::new(),
                    stale: false,
                    waiting: false,
                    unseen: false,
                    section: String::new(),
                }));
            }
            ("request", Reading::Nothing, Some(file)) => {
                reading = Reading::Request(Request {
                    id: number.unwrap_or(0),
                    file,
                    range: 0..0,
                    quote: String::new(),
                    what: String::new(),
                    words: String::new(),
                    at: 0,
                    seq: 0,
                    done: false,
                    section: String::new(),
                });
            }
            ("instruction", Reading::Nothing, _) => {
                reading = Reading::Instruction(Instruction {
                    id: number.unwrap_or(0),
                    text: String::new(),
                    everywhere: brief,
                    file,
                    category: None,
                    protect: None,
                    refused: None,
                    from: None,
                    at: 0,
                    seq: 0,
                });
            }
            ("end", ..) => match std::mem::replace(&mut reading, Reading::Nothing) {
                Reading::Suggestion(suggestion) => stored.suggestions.push(*suggestion),
                Reading::Request(request) => stored.requests.push(request),
                Reading::Instruction(instruction) => stored.brief.push(instruction),
                Reading::Nothing => {}
            },
            (key, Reading::Suggestion(suggestion), _) => {
                let text = unescape(value);
                match key {
                    "kind" => suggestion.kind = Kind::named(value).unwrap_or(Kind::Comment),
                    "range" => suggestion.range = range_of(value),
                    "quote" => suggestion.quote = text,
                    "new" => suggestion.new = text,
                    "offered" => suggestion.offered = text,
                    "context" => suggestion.context = text,
                    "reason" => suggestion.reason = text,
                    "category" => suggestion.category = text,
                    "by" => suggestion.by = text,
                    "session" => suggestion.session = text,
                    "section" => suggestion.section = text,
                    "alternative" => suggestion.alternatives.push(text),
                    "shown" => suggestion.shown = value.parse().unwrap_or(0),
                    "confidence" => suggestion.confidence = Level::named(value).unwrap_or_default(),
                    "importance" => suggestion.importance = Level::named(value).unwrap_or_default(),
                    "made" => suggestion.made = number.unwrap_or(0),
                    "state" => suggestion.state = State::named(value).unwrap_or(State::Open),
                    "waiting" => suggestion.waiting = value == "yes",
                    "unseen" => suggestion.unseen = value == "yes",
                    "request" => suggestion.request = number,
                    "note" => suggestion.note = number,
                    "entry" => suggestion.thread.extend(entry_of(value)),
                    _ => {}
                }
            }
            (key, Reading::Request(request), _) => match key {
                "range" => request.range = range_of(value),
                "quote" => request.quote = unescape(value),
                "what" => request.what = unescape(value),
                "words" => request.words = unescape(value),
                "section" => request.section = unescape(value),
                "made" => request.at = number.unwrap_or(0),
                "seq" => request.seq = number.unwrap_or(0),
                "done" => request.done = value == "yes",
                _ => {}
            },
            (key, Reading::Instruction(instruction), _) => match key {
                "text" => instruction.text = unescape(value),
                "everywhere" => instruction.everywhere = value == "yes",
                "category" => instruction.category = Some(unescape(value)),
                "protect" => {
                    let mut fields = value.splitn(3, '\t');
                    let start = fields.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                    let end: usize = fields.next().and_then(|n| n.parse().ok()).unwrap_or(start);
                    let quote = unescape(fields.next().unwrap_or_default());
                    instruction.protect = Some((start..end.max(start), quote));
                }
                "refused" => {
                    let (old, new) = value.split_once('\t').unwrap_or((value, ""));
                    instruction.refused = Some((unescape(old), unescape(new)));
                }
                "from" => instruction.from = number,
                "made" => instruction.at = number.unwrap_or(0),
                "seq" => instruction.seq = number.unwrap_or(0),
                _ => {}
            },
            _ => {}
        }
    }
    Ok(stored)
}

/// Where `quote` is in `text`: at `range` if it is there, else where it
/// is nearest to it within reach.
pub(super) fn found_again(text: &str, quote: &str, range: &Range<usize>) -> Option<Range<usize>> {
    if quote.is_empty() {
        return None;
    }
    if part(text, range) == Some(quote) {
        return Some(range.clone());
    }
    super::super::notes::near(text, quote, range.start, REACH).map(|at| at..at + quote.len())
}

/// Where the text `context` ends in `text`, nearest to `at`: the place
/// to put text in at.
fn found_after(text: &str, context: &str, at: usize) -> Option<usize> {
    if context.is_empty() {
        return (at <= text.len() && text.is_char_boundary(at)).then_some(at);
    }
    let before = at.saturating_sub(context.len());
    let range = before..at;
    found_again(text, context, &range).map(|found| found.end)
}

impl Viewer {
    /// Reads the store of the file being edited, and the brief for all
    /// documents, the first time they are wanted.
    pub(super) fn load_review(&mut self) {
        let (Some(state_dir), true) = (self.state_dir.clone(), self.editor.is_some()) else {
            return;
        };
        if !self.review.everywhere_loaded {
            self.review.everywhere_loaded = true;
            let path = brief_path(&state_dir);
            match std::fs::read_to_string(&path) {
                Ok(text) => match parse(&text, None, true) {
                    Ok(stored) => {
                        for mut instruction in stored.brief {
                            self.review.last_instruction += 1;
                            instruction.id = self.review.last_instruction;
                            instruction.file = None;
                            instruction.everywhere = true;
                            self.review.brief.push(instruction);
                        }
                    }
                    Err(why) => self.alert(format!("the brief could not be read: {why}")),
                },
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => self.alert(format!("the brief could not be read: {err}")),
            }
        }
        let Some(editor) = &self.editor else {
            return;
        };
        let file = editor.file();
        if self.review.loaded.contains(&file) {
            return;
        }
        self.review.loaded.push(file);
        let store = store_path(&state_dir, editor.path());
        let read = match std::fs::read_to_string(&store) {
            Ok(text) => parse(&text, Some(file), false),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                Ok(super::store::Stored::default())
            }
            Err(err) => Err(err.to_string()),
        };
        let stored = match read {
            Ok(stored) => stored,
            Err(why) => {
                // Kept, not written over: it may be of a later zetwerk.
                let aside = store.with_extension("unreadable");
                let _ = std::fs::rename(&store, &aside);
                self.alert(format!(
                    "the review of this file could not be read ({why}); it is kept in {}",
                    aside.display()
                ));
                return;
            }
        };
        self.take_stored(file, stored);
    }

    /// Takes in what the store of `file` held: each suggestion found
    /// again in the text as it is, and those that wait for the author
    /// waiting as proposals again.
    fn take_stored(&mut self, file: FileId, stored: Stored) {
        let text = self.text().unwrap_or_default().to_owned();
        self.review.seq = self.review.seq.max(stored.seq);
        for mut suggestion in stored.suggestions {
            if self
                .review
                .all
                .iter()
                .any(|other| other.id == suggestion.id)
                || suggestion.id == 0
            {
                suggestion.id = self.review.last + 1;
            }
            self.review.last = self.review.last.max(suggestion.id);
            for entry in &suggestion.thread {
                self.review.seq = self.review.seq.max(entry.seq);
            }
            if !suggestion.state.decided() {
                let range = match suggestion.kind {
                    Kind::Insert => found_after(&text, &suggestion.context, suggestion.range.start)
                        .map(|at| at..at),
                    _ => found_again(&text, &suggestion.quote, &suggestion.range),
                };
                suggestion.stale = range.is_none();
                if let Some(range) = range {
                    suggestion.range = range;
                }
                if suggestion.kind.changes() && !suggestion.stale {
                    let id = self.next_proposal;
                    self.next_proposal += 1;
                    suggestion.proposal = Some(id);
                    self.proposals.push(Proposal {
                        id,
                        file,
                        range: suggestion.range.clone(),
                        old: suggestion.quote.clone(),
                        new: suggestion.new.clone(),
                        stale: false,
                    });
                }
            }
            self.review.all.push(suggestion);
        }
        self.proposals.sort_by_key(|proposal| proposal.range.start);
        if !self.proposals.is_empty() {
            self.proposal_text = Some(text.clone());
        }
        for mut request in stored.requests {
            self.review.last_request += 1;
            request.id = self.review.last_request;
            if let Some(range) = found_again(&text, &request.quote, &request.range) {
                request.range = range;
            }
            self.review.requests.push(request);
        }
        for mut instruction in stored.brief {
            self.review.last_instruction += 1;
            instruction.id = self.review.last_instruction;
            instruction.file = Some(file);
            instruction.everywhere = false;
            if let Some((range, quote)) = &mut instruction.protect
                && let Some(found) = found_again(&text, quote, range)
            {
                *range = found;
            }
            self.review.brief.push(instruction);
        }
        self.review
            .all
            .sort_by_key(|suggestion| (suggestion.file != file, suggestion.range.start));
        self.review.seen = None;
        self.reflag_all();
        self.review.signal.tell(self.review.seq);
        self.review.dirty = true;
        self.review_changed();
        self.review.unwritten.retain(|written| *written != file);
    }

    /// Writes what changed of reviewing since it was written: when the
    /// journal is forced to disk.
    pub(in crate::viewer) fn keep_review(&mut self) {
        let files = std::mem::take(&mut self.review.unwritten);
        for file in files {
            self.save_review(file);
        }
        if std::mem::take(&mut self.review.everywhere_unwritten) {
            self.save_brief();
        }
    }

    /// Writes the store of `file`, with the text of what is kept as it
    /// is now.
    pub(super) fn save_review(&mut self, file: FileId) {
        let Some(state_dir) = &self.state_dir else {
            return;
        };
        let Some(editor) = self
            .editor
            .iter()
            .chain(&self.others)
            .find(|editor| editor.file() == file)
        else {
            return;
        };
        let store = store_path(state_dir, editor.path());
        let kept = Stored {
            seq: self.review.seq,
            suggestions: self
                .review
                .all
                .iter()
                .filter(|suggestion| suggestion.file == file)
                .cloned()
                .collect(),
            requests: self
                .review
                .requests
                .iter()
                .filter(|request| request.file == file)
                .cloned()
                .collect(),
            brief: self
                .review
                .brief
                .iter()
                .filter(|instruction| instruction.file == Some(file) && !instruction.everywhere)
                .cloned()
                .collect(),
        };
        if kept.suggestions.is_empty()
            && kept.requests.is_empty()
            && kept.brief.is_empty()
            && !store.exists()
        {
            return;
        }
        let path = editor.path().to_owned();
        if let Err(err) = write_store(&store, &stored(&path, self.review.seq, &kept)) {
            self.alert(format!("the review could not be kept: {err}"));
        }
    }

    /// Writes the instructions for all documents.
    fn save_brief(&mut self) {
        let Some(state_dir) = &self.state_dir else {
            return;
        };
        let everywhere: Vec<&Instruction> = self
            .review
            .brief
            .iter()
            .filter(|instruction| instruction.everywhere)
            .collect();
        let path = brief_path(state_dir);
        if everywhere.is_empty() && !path.exists() {
            return;
        }
        if let Err(err) = write_store(&path, &stored_brief(&everywhere)) {
            self.alert(format!("the brief could not be kept: {err}"));
        }
    }

    /// The file `old` goes on as `new` (`:saveas`, `:mv`): its review is
    /// the new file's, and kept under its name.
    pub(in crate::viewer) fn review_renamed(&mut self, old: FileId, new: FileId) {
        let loaded = self.review.loaded.contains(&old);
        for suggestion in self.review.all.iter_mut().filter(|s| s.file == old) {
            suggestion.file = new;
        }
        for request in self.review.requests.iter_mut().filter(|r| r.file == old) {
            request.file = new;
        }
        for instruction in &mut self.review.brief {
            if instruction.file == Some(old) {
                instruction.file = Some(new);
            }
        }
        for undo in &mut self.review.undo {
            if undo.file == old {
                undo.file = new;
            }
        }
        if let Some((file, _)) = &mut self.review.seen
            && *file == old
        {
            *file = new;
        }
        if loaded {
            self.review.loaded.push(new);
            self.save_review(new);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use super::*;

    fn file() -> FileId {
        typst::syntax::RootedPath::new(
            typst::syntax::VirtualRoot::Project,
            typst::syntax::VirtualPath::new("main.typ").unwrap(),
        )
        .intern()
    }

    #[test]
    fn a_store_gives_back_what_it_was_given() {
        let suggestion = Suggestion {
            id: 4,
            file: file(),
            kind: Kind::Replace,
            range: 3..9,
            quote: "quick\tfox".to_owned(),
            new: "slow\nfox".to_owned(),
            context: String::new(),
            offered: "slow\nfox".to_owned(),
            alternatives: vec!["sly fox".to_owned(), "red \\ fox".to_owned()],
            shown: 1,
            reason: "Plainer.".to_owned(),
            category: "clarity".to_owned(),
            confidence: Level::High,
            importance: Level::Low,
            by: "claude-code".to_owned(),
            session: "s1".to_owned(),
            made: 1_700_000_000,
            state: State::Rewritten,
            thread: vec![
                Entry {
                    seq: 1,
                    at: 1_700_000_000,
                    by: Who::Agent("claude-code".to_owned()),
                    said: Said::Made,
                },
                Entry {
                    seq: 2,
                    at: 1_700_000_100,
                    by: Who::Author,
                    said: Said::Rewritten {
                        before: "quick fox".to_owned(),
                        offered: "slow fox".to_owned(),
                        after: "slower fox".to_owned(),
                    },
                },
                Entry {
                    seq: 3,
                    at: 1_700_000_200,
                    by: Who::Author,
                    said: Said::Alternatives(vec!["a".to_owned(), "b\tc".to_owned()]),
                },
            ],
            proposal: None,
            request: Some(2),
            note: None,
            flagged: Vec::new(),
            stale: false,
            waiting: true,
            unseen: false,
            section: "Methods".to_owned(),
        };
        let request = Request {
            id: 2,
            file: file(),
            range: 0..20,
            quote: "The quick".to_owned(),
            what: "clarity".to_owned(),
            words: "be brief".to_owned(),
            at: 5,
            seq: 1,
            done: true,
            section: "Intro".to_owned(),
        };
        let instruction = Instruction {
            id: 1,
            text: "do not touch the abstract".to_owned(),
            everywhere: false,
            file: Some(file()),
            category: Some("tone".to_owned()),
            protect: Some((10..20, "abstract\ttext".to_owned())),
            refused: Some(("color".to_owned(), "colour".to_owned())),
            from: Some(4),
            at: 7,
            seq: 3,
        };
        let kept = Stored {
            seq: 3,
            suggestions: vec![suggestion],
            requests: vec![request],
            brief: vec![instruction.clone()],
        };
        let text = stored(Path::new("/doc/main.typ"), 3, &kept);
        assert_eq!(parse(&text, Some(file()), false).unwrap(), kept);
        assert!(parse("something", Some(file()), false).is_err());
        let everywhere = Instruction {
            everywhere: true,
            file: None,
            ..instruction
        };
        let brief = parse(&stored_brief(&[&everywhere]), None, true).unwrap();
        assert_eq!(brief.brief, vec![everywhere]);
    }

    #[test]
    fn text_is_found_again_where_it_went() {
        let text = "Before. The quick fox, the quick dog.";
        assert_eq!(found_again(text, "quick", &(9..14)), Some(12..17));
        assert_eq!(found_again(text, "quick", &(28..33)), Some(27..32));
        assert_eq!(found_again(text, "slow", &(9..13)), None);
        assert_eq!(found_after(text, "Before.", 7), Some(7));
        assert_eq!(found_after("xx Before. y", "Before.", 7), Some(10));
    }
}
