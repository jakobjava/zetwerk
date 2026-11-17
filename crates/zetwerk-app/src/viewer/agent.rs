//! What an agent may ask of the document, and do to it.
//!
//! An agent's change goes the way of a key: through the editor, into the
//! journal, onto the undo history as one step. It is refused if it was
//! made for other text than there is (`expected_revision`), and it never
//! guesses: text to replace has to be there exactly once.
//!
//! Where the user asked for it (`:agents propose`), and whenever the agent
//! asks for it, a change is not made but put before the user as a
//! [`Proposal`], to accept or to reject.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::ops::Range;

use typst::World;
use typst::foundations::NativeElement;
use typst::model::HeadingElem;
use typst::syntax::{LinkedNode, Span, SyntaxKind};

use super::{Difference, Edited, PageMap, Viewer, difference};

/// More lines than this are not read at once.
const MAX_LINES: usize = 2000;
/// Selected text longer than this is cut where it is reported.
const MAX_SELECTED: usize = 4000;

/// Whether an agent's changes are made, or put before the user first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentEdits {
    /// Made at once, each as one undo step.
    #[default]
    Live,
    /// Shown as proposals, whatever the agent asks for.
    Propose,
}

/// A change an agent wants to make, waiting for the user's word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    pub id: u64,
    /// The file it is about.
    pub file: typst::syntax::FileId,
    /// The bytes of the text to replace.
    pub range: Range<usize>,
    /// What is there, and what should be.
    pub old: String,
    pub new: String,
    /// Whether the text it is about has been changed since: it can only
    /// be dropped then.
    pub stale: bool,
}

/// Why a stretch of the text is shown in a colour of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mark {
    /// An agent changed it last.
    Changed,
    /// An agent proposes to change it.
    Proposed,
    /// An agent proposed to change it, and it has changed since.
    Stale,
    /// The compiler says it is wrong, or may be.
    Error,
    Warning,
}

/// What became of a change an agent asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// It was made. `lines` are the first and the last line of the text
    /// that is different now, counting from 1.
    Changed {
        revision: u64,
        lines: (usize, usize),
    },
    /// It waits as proposal `id`.
    Proposed { id: u64, revision: u64 },
}

/// Lines of the source, for an agent to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lines {
    pub revision: u64,
    /// The first and the last line given, counting from 1, and how many
    /// the text has.
    pub first: usize,
    pub last: usize,
    pub total: usize,
    /// Each line as its number, a tab and its text.
    pub text: String,
}

/// A place a search found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub line: usize,
    pub column: usize,
    /// The whole line.
    pub text: String,
}

/// A heading of the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    pub level: usize,
    /// As written, markup and all.
    pub text: String,
    pub line: usize,
    /// The page it is on, counting from 1, if the pages are those of the
    /// text as it is.
    pub page: Option<usize>,
}

/// The headings and labels of the document.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Outline {
    pub headings: Vec<Heading>,
    /// Labels and the lines they are on.
    pub labels: Vec<(String, usize)>,
}

/// A compiled document, ready to be written as a file.
pub struct Export {
    pub document: std::sync::Arc<typst_layout::PagedDocument>,
    pub world: std::sync::Arc<zetwerk_typst::ProjectWorld>,
    /// Where the document's main file is; `None` if it is not on disk.
    pub main: Option<std::path::PathBuf>,
}

/// The cursor and the selection, for an agent that works on "this".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Focus {
    /// Line and column of the cursor, counting from 1.
    pub cursor: (usize, usize),
    pub selection: Option<Selected>,
}

/// What is selected in the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selected {
    /// From where (inclusive) to where (exclusive), as lines and columns
    /// counting from 1.
    pub from: (usize, usize),
    pub to: (usize, usize),
    /// The text; long text is cut.
    pub text: String,
}

/// The line and the column of byte `offset` of `text`, counting from 1;
/// the column counts characters.
#[must_use]
pub fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = &text[..offset];
    let start = before.rfind('\n').map_or(0, |at| at + 1);
    (
        before.matches('\n').count() + 1,
        before[start..].chars().count() + 1,
    )
}

/// The byte of `text` at `line` and `column`, counting from 1. A column
/// beyond the line's end is the line's end; a line beyond the text is
/// nothing.
#[must_use]
pub fn offset_of(text: &str, line: usize, column: usize) -> Option<usize> {
    let mut start = 0;
    for _ in 1..line {
        start += text[start..].find('\n')? + 1;
    }
    if line == 0 || (start == text.len() && line > 1 && !text.ends_with('\n')) {
        return None;
    }
    let rest = &text[start..];
    let rest = &rest[..rest.find('\n').unwrap_or(rest.len())];
    let within = rest
        .char_indices()
        .nth(column.saturating_sub(1))
        .map_or(rest.len(), |(at, _)| at);
    Some(start + within)
}

impl Viewer {
    /// Whether agents change the text or propose changes to it.
    #[must_use]
    pub fn agent_edits(&self) -> AgentEdits {
        self.agent_edits
    }

    pub fn set_agent_edits(&mut self, mode: AgentEdits) {
        self.agent_edits = mode;
    }

    /// Takes over from `before`, the viewer of the document this one
    /// takes the place of in the window, what agents were told: whether
    /// their changes are proposed only; and the revision and the numbers
    /// of proposals, which go on counting, so that what an agent read of
    /// the other document or proposed for it is never taken for this.
    pub fn take_over_from(&mut self, before: &Viewer) {
        self.agent_edits = before.agent_edits;
        self.revision = self.revision.max(before.revision + 1);
        self.next_proposal = self.next_proposal.max(before.next_proposal);
    }

    /// Whether the pages show another state of the text than the one it
    /// is in, as the user looks through its history: what the compiler
    /// says of them is not said of the text.
    #[must_use]
    pub fn pages_look_back(&self) -> bool {
        self.looks_back()
    }

    /// The changes agents have proposed to the file being edited, in
    /// the order of its text.
    #[must_use]
    pub fn proposals(&self) -> &[Proposal] {
        &self.proposals
    }

    /// How many changes agents have proposed to other open files.
    #[must_use]
    pub fn proposals_elsewhere(&self) -> usize {
        self.proposals_elsewhere.len()
    }

    /// Brings up the proposals for the file that is being edited now,
    /// and puts away those for the one that was.
    pub(super) fn swap_proposals(&mut self) {
        let file = self.file_id();
        self.proposals_elsewhere.append(&mut self.proposals);
        let (here, elsewhere) = std::mem::take(&mut self.proposals_elsewhere)
            .into_iter()
            .partition(|proposal| proposal.file == file);
        self.proposals = here;
        self.proposals_elsewhere = elsewhere;
        self.proposal_text = self.text().map(str::to_owned);
    }

    /// The cursor and the selection; `None` for a window that only shows
    /// pages.
    #[must_use]
    pub fn focus(&self) -> Option<Focus> {
        let editor = self.editor.as_ref()?;
        let text = editor.text();
        let selection = editor.selection().map(|range| {
            let mut end = range.end.min(range.start + MAX_SELECTED);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            let mut selected = text[range.start..end].to_owned();
            if end < range.end {
                selected.push('…');
            }
            Selected {
                from: line_column(text, range.start),
                to: line_column(text, range.end),
                text: selected,
            }
        });
        Some(Focus {
            cursor: editor.position(),
            selection,
        })
    }

    /// The lines `from..=to` of the source (all of it by default), each
    /// with its number.
    pub fn lines(&self, from: Option<usize>, to: Option<usize>) -> Result<Lines, String> {
        let text = self.text().ok_or(NOT_EDITING)?;
        let total = text.lines().count();
        let first = from.unwrap_or(1).max(1);
        let last = to.unwrap_or(total).min(total);
        if first > total.max(1) {
            return Err(format!("line {first} is beyond the text's {total} lines"));
        }
        if last < first && total > 0 {
            return Err(format!("no lines from {first} to {last}"));
        }
        if last + 1 - first > MAX_LINES {
            return Err(format!(
                "{} lines are too many to read at once; ask for at most {MAX_LINES}",
                last + 1 - first
            ));
        }
        let mut numbered = String::new();
        for (index, line) in text
            .lines()
            .enumerate()
            .skip(first - 1)
            .take(last + 1 - first)
        {
            let _ = writeln!(numbered, "{}\t{line}", index + 1);
        }
        Ok(Lines {
            revision: self.revision,
            first,
            last,
            total,
            text: numbered,
        })
    }

    /// Replaces `old`, which has to be in the text exactly once (or, with
    /// `all`, at least once: then every one of them), by `new`.
    pub fn agent_replace(
        &mut self,
        old: &str,
        new: &str,
        all: bool,
        expected_revision: Option<u64>,
        propose: bool,
    ) -> Result<Outcome, String> {
        self.expect(expected_revision)?;
        if old.is_empty() {
            return Err("old_str is empty; to add text, replace text next to it".to_owned());
        }
        if old == new {
            return Err("old_str and new_str are the same".to_owned());
        }
        let text = self.text().ok_or(NOT_EDITING)?;
        let found: Vec<usize> = text.match_indices(old).map(|(at, _)| at).collect();
        match found.len() {
            0 => {
                return Err(
                    "old_str is not in the text; read the source again, it may have changed"
                        .to_owned(),
                );
            }
            1 => {}
            count if !all => {
                let lines: Vec<String> = found
                    .iter()
                    .take(8)
                    .map(|at| line_column(text, *at).0.to_string())
                    .collect();
                return Err(format!(
                    "old_str is in the text {count} times (lines {}); give more of the text \
                     around it, or set replace_all",
                    lines.join(", ")
                ));
            }
            _ => {}
        }
        let ranges: Vec<Range<usize>> = found.iter().map(|at| *at..*at + old.len()).collect();
        self.agent_change(&ranges, new, propose)
    }

    /// Puts `text` in at the cursor.
    pub fn agent_insert(
        &mut self,
        text: &str,
        expected_revision: Option<u64>,
        propose: bool,
    ) -> Result<Outcome, String> {
        self.expect(expected_revision)?;
        let at = self.editor.as_ref().ok_or(NOT_EDITING)?.cursor();
        if text.is_empty() {
            return Err("there is nothing to insert".to_owned());
        }
        let place = at..at;
        self.agent_change(std::slice::from_ref(&place), text, propose)
    }

    /// Replaces what is selected by `text`.
    pub fn agent_replace_selection(
        &mut self,
        text: &str,
        expected_revision: Option<u64>,
        propose: bool,
    ) -> Result<Outcome, String> {
        self.expect(expected_revision)?;
        let editor = self.editor.as_ref().ok_or(NOT_EDITING)?;
        let range = editor
            .selection()
            .ok_or("nothing is selected in the window")?;
        if &editor.text()[range.clone()] == text {
            return Err("the selection is that text already".to_owned());
        }
        self.agent_change(std::slice::from_ref(&range), text, propose)
    }

    pub(super) fn expect(&self, revision: Option<u64>) -> Result<(), String> {
        match revision {
            Some(expected) if expected != self.revision => Err(format!(
                "the text is at revision {}, not {expected}: it has changed since you read \
                 it; read it again",
                self.revision
            )),
            _ => Ok(()),
        }
    }

    /// Replaces each of `ranges` (in the order of the text, apart from
    /// each other) by `new`, or proposes to.
    pub(super) fn agent_change(
        &mut self,
        ranges: &[Range<usize>],
        new: &str,
        propose: bool,
    ) -> Result<Outcome, String> {
        if propose || self.agent_edits == AgentEdits::Propose {
            let text = self.text().ok_or(NOT_EDITING)?.to_owned();
            let mut id = 0;
            for range in ranges {
                id = self.next_proposal;
                self.next_proposal += 1;
                self.noted_proposal(id);
                self.proposals.push(Proposal {
                    id,
                    file: self.file_id(),
                    old: text[range.clone()].to_owned(),
                    range: range.clone(),
                    new: new.to_owned(),
                    stale: false,
                });
            }
            self.proposals.sort_by_key(|proposal| proposal.range.start);
            self.proposal_text = Some(text);
            let count = self.proposals.len();
            self.notice(format!(
                "an agent proposes a change ({count} waiting): ]p goes to it"
            ));
            return Ok(Outcome::Proposed {
                id,
                revision: self.revision,
            });
        }
        // From the last to the first, so that the ranges stay good.
        let edits: Vec<(Range<usize>, &str)> = ranges
            .iter()
            .rev()
            .map(|range| (range.clone(), new))
            .collect();
        let (Some(first), Some(last)) = (ranges.first(), ranges.last()) else {
            return Err("there is nothing to change".to_owned());
        };
        // From the first byte that is different to the last: the ranges
        // before the last one have each become `new`.
        let before_last: usize = ranges.iter().rev().skip(1).map(Range::len).sum();
        let changed = first.start..last.start + ranges.len() * new.len() - before_last;
        let origin = self.agent_origin();
        self.editor
            .as_mut()
            .ok_or(NOT_EDITING)?
            .replace_by(&edits, &origin)
            .map_err(|why| format!("not changed: {why}"))?;
        self.text_changed(Edited::Agent);
        let text = self.text().ok_or(NOT_EDITING)?;
        let end = changed.end.min(text.len());
        let lines = (
            line_column(text, changed.start).0,
            line_column(text, end.saturating_sub(1).max(changed.start)).0,
        );
        self.agent_mark = Some(changed.start..end);
        self.notice(if lines.0 == lines.1 {
            format!("an agent changed line {}; u takes it back", lines.0)
        } else {
            format!(
                "an agent changed lines {} to {}; u takes it back",
                lines.0, lines.1
            )
        });
        Ok(Outcome::Changed {
            revision: self.revision,
            lines,
        })
    }

    /// What follows a change of the text that no key made.
    pub(super) fn text_changed(&mut self, edited: Edited) {
        self.revision += 1;
        self.touched();
        if let Some(old) = self.predicted.take() {
            self.planned.retain(|(index, _, _), _| *index != old.index);
        }
        self.predicted_at = None;
        self.edited = Some(edited);
        self.sync_proposals();
    }

    /// Moves the proposals along with a change of the text; those whose
    /// text was changed are stale from then on.
    pub(super) fn sync_proposals(&mut self) {
        self.sync_notes();
        if self.proposals.is_empty() {
            self.proposal_text = None;
            return;
        }
        let (Some(editor), Some(before)) = (&self.editor, &self.proposal_text) else {
            return;
        };
        let now = editor.text();
        if let Difference::Replacement {
            offset,
            removed,
            text,
        } = difference(before, now)
        {
            // What was replaced, in the text as it was.
            let end = offset + removed;
            for proposal in &mut self.proposals {
                let (start, stop) = (proposal.range.start, proposal.range.end);
                if start >= end {
                    // Behind the change: it moves along.
                    proposal.range = start - removed + text.len()..stop - removed + text.len();
                } else if stop > offset {
                    // The change is in its text. It may be the very same
                    // text again (an undo and its redo): that is looked
                    // at below.
                    proposal.stale = true;
                }
            }
        }
        for proposal in &mut self.proposals {
            let there = now.get(proposal.range.clone()) == Some(proposal.old.as_str());
            if proposal.stale && there && !proposal.old.is_empty() {
                proposal.stale = false;
            } else if !there {
                proposal.stale = true;
            }
        }
        self.proposal_text = Some(now.to_owned());
    }

    /// What agents did and want to do, as stretches of the text to show:
    /// under the selection, in the source and on the pages.
    pub(super) fn agent_marks(&self) -> Vec<(Range<usize>, Mark)> {
        let mut marks = Vec::new();
        marks.extend(self.agent_mark.clone().map(|range| (range, Mark::Changed)));
        marks.extend(self.proposals.iter().map(|proposal| {
            let mark = if proposal.stale {
                Mark::Stale
            } else {
                Mark::Proposed
            };
            (proposal.range.clone(), mark)
        }));
        marks
    }

    /// The proposal the cursor is on.
    fn proposal_at_cursor(&self) -> Option<usize> {
        let cursor = self.editor.as_ref()?.cursor();
        let on = |proposal: &Proposal| {
            proposal.range.contains(&cursor) || proposal.range.start == cursor
        };
        self.proposals.iter().position(on)
    }

    /// Makes the change of the proposal under the cursor (`ga`), or of
    /// all of them. Returns how many were made.
    pub fn accept_proposals(&mut self, all: bool) -> usize {
        let chosen: Vec<u64> = if all {
            self.proposals.iter().map(|proposal| proposal.id).collect()
        } else {
            let at = self.proposal_at_cursor();
            at.map(|at| self.proposals[at].id).into_iter().collect()
        };
        if chosen.is_empty() {
            self.alert("the cursor is on no proposal; ]p goes to the next one");
            return 0;
        }
        let mut made = 0;
        // One by one: each moves the others along.
        for id in chosen {
            let Some(at) = self.proposals.iter().position(|proposal| proposal.id == id) else {
                continue;
            };
            if self.proposals[at].stale {
                continue;
            }
            let proposal = self.proposals.remove(at);
            let origin = self.accepted_origin(proposal.id);
            let Some(editor) = &mut self.editor else {
                break;
            };
            match editor.replace_by(&[(proposal.range.clone(), proposal.new.as_str())], &origin) {
                Ok(()) => {
                    editor.set_cursor(proposal.range.start);
                    made += 1;
                    self.text_changed(Edited::Agent);
                }
                Err(why) => {
                    self.alert(format!("not changed: {why}"));
                    break;
                }
            }
        }
        let stale = self
            .proposals
            .iter()
            .filter(|proposal| proposal.stale)
            .count();
        if made == 0 && stale > 0 {
            self.alert("the text of this proposal has changed since; gr drops it");
        } else if made > 0 {
            let left = self.proposals.len();
            self.notice(format!("accepted {made}; {left} waiting"));
        }
        made
    }

    /// Drops the proposal under the cursor (`gr`), or all of them.
    /// Returns how many were dropped.
    pub fn reject_proposals(&mut self, all: bool) -> usize {
        let before = self.proposals.len();
        if all {
            self.proposals.clear();
        } else if let Some(at) = self.proposal_at_cursor() {
            self.proposals.remove(at);
        } else {
            self.alert("the cursor is on no proposal; ]p goes to the next one");
            return 0;
        }
        let dropped = before - self.proposals.len();
        let left = self.proposals.len();
        self.notice(format!("rejected {dropped}; {left} waiting"));
        self.sync_proposals();
        dropped
    }

    /// Drops proposal `id`, for the agent that made it and thought better
    /// of it.
    pub fn withdraw_proposal(&mut self, id: u64) -> Result<(), String> {
        if self.withdraw_proposed_sources(id) {
            return Ok(());
        }
        let at = self
            .proposals
            .iter()
            .position(|proposal| proposal.id == id)
            .ok_or_else(|| format!("there is no proposal {id}"))?;
        self.proposals.remove(at);
        self.sync_proposals();
        Ok(())
    }

    /// Drops the proposals `ids`, for the agent that made them: all of
    /// them, or none where one is not there.
    pub fn withdraw_proposals(&mut self, ids: &[u64]) -> Result<(), String> {
        let sources = self.proposed_sources();
        let missing: Vec<String> = ids
            .iter()
            .filter(|id| !self.proposals.iter().any(|proposal| proposal.id == **id))
            .filter(|id| !sources.iter().any(|(proposed, _)| proposed == *id))
            .map(u64::to_string)
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "there is no proposal {}: none was taken back",
                missing.join(", ")
            ));
        }
        self.proposals
            .retain(|proposal| !ids.contains(&proposal.id));
        for id in ids {
            self.withdraw_proposed_sources(*id);
        }
        self.sync_proposals();
        Ok(())
    }

    /// What the window shows, that an agent's request is for: the main
    /// file of the document and the file being edited. What was asked
    /// of one is not answered from another the window shows by then.
    #[must_use]
    pub fn shown_document(&self) -> (Option<std::path::PathBuf>, Option<std::path::PathBuf>) {
        let editing = self.editor.as_ref().map(|editor| editor.path().to_owned());
        (self.main_path(), editing)
    }

    /// Puts the cursor on the next proposal after it (`]p`), or the one
    /// before it (`[p`).
    pub fn goto_proposal(&mut self, forward: bool) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let cursor = editor.cursor();
        let starts = self.proposals.iter().map(|proposal| proposal.range.start);
        let target = if forward {
            starts
                .clone()
                .find(|start| *start > cursor)
                .or(starts.clone().next())
        } else {
            let before = starts.clone().rfind(|start| *start < cursor);
            before.or(starts.clone().next_back())
        };
        match target {
            Some(start) => {
                editor.set_cursor(start);
                self.follow_cursor();
            }
            None => self.alert("no proposals"),
        }
    }

    /// Every place `query` is in the text, as written or without regard
    /// to case, up to `limit` of them; and how many there are in all.
    pub fn search(
        &self,
        query: &str,
        ignore_case: bool,
        limit: usize,
    ) -> Result<(Vec<Found>, usize), String> {
        let text = self.text().ok_or(NOT_EDITING)?;
        if query.is_empty() {
            return Err("there is nothing to search for".to_owned());
        }
        let mut found = Vec::new();
        let mut total = 0;
        let needle = query.to_lowercase();
        for (index, line) in text.lines().enumerate() {
            let hits: Vec<usize> = if ignore_case {
                // Lower case may be longer or shorter in bytes: columns
                // are counted in the lowered line, which has as many
                // characters for all but a handful of letters.
                let lowered = line.to_lowercase();
                lowered
                    .match_indices(&needle)
                    .map(|(at, _)| lowered[..at].chars().count() + 1)
                    .collect()
            } else {
                line.match_indices(query)
                    .map(|(at, _)| line[..at].chars().count() + 1)
                    .collect()
            };
            for column in hits {
                total += 1;
                if found.len() < limit {
                    found.push(Found {
                        line: index + 1,
                        column,
                        text: line.to_owned(),
                    });
                }
            }
        }
        Ok((found, total))
    }

    /// The headings and labels of the text as it is. Pages are given
    /// where the pages on screen are those of this text.
    pub fn outline(&self) -> Result<Outline, String> {
        let editor = self.editor.as_ref().ok_or(NOT_EDITING)?;
        let (source, text) = (editor.source(), editor.text());
        // Where the compiled headings are, by the syntax node each came
        // from. Good only while the nodes are those of this text.
        let mut pages: HashMap<Span, usize> = HashMap::new();
        if let (Some(document), true) = (
            &self.document,
            self.document_revision == Some(self.revision) && !self.looks_back(),
        ) {
            let introspector = document.introspector();
            let headings = introspector.elements().query(&HeadingElem::ELEM.select());
            for heading in &headings {
                let position = heading
                    .location()
                    .and_then(|location| introspector.position(location));
                if let Some(position) = position {
                    pages.entry(heading.span()).or_insert(position.page.get());
                }
            }
        }
        let mut outline = Outline::default();
        let mut stack = vec![LinkedNode::new(source.root())];
        while let Some(node) = stack.pop() {
            match node.kind() {
                SyntaxKind::Heading => {
                    let written = &text[node.range()];
                    let level = written.chars().take_while(|c| *c == '=').count();
                    outline.headings.push(Heading {
                        level,
                        text: written[level..].trim().to_owned(),
                        line: line_column(text, node.offset()).0,
                        page: pages.get(&node.span()).copied(),
                    });
                }
                SyntaxKind::Label => {
                    let name = text[node.range()]
                        .trim_start_matches('<')
                        .trim_end_matches('>');
                    outline
                        .labels
                        .push((name.to_owned(), line_column(text, node.offset()).0));
                }
                _ => {}
            }
            // In the order of the text: the first child is looked at next.
            let children: Vec<LinkedNode> = node.children().collect();
            stack.extend(children.into_iter().rev());
        }
        Ok(outline)
    }

    /// Where on the pages the text at `line` and `column` is: the page,
    /// counting from 1, and how far across and down it the top-left
    /// corner of its first character is, in points. For a place that
    /// shows as nothing itself, the text nearest to it.
    pub fn page_of(&mut self, line: usize, column: usize) -> Result<(usize, f64, f64), String> {
        let (editor, document, world) = self.current_pages()?;
        let offset = offset_of(editor.text(), line, column)
            .ok_or_else(|| format!("the text has no line {line}"))?;
        let (document, world) = (document.clone(), world.clone());
        let file = self.file_id();
        let source = world
            .source(file)
            .map_err(|_| "the compiled source is gone".to_owned())?;
        let near = self.cursor_page.get();
        let shown = world.shown_at(file, offset);
        let position = zetwerk_typst::locate(&document, &source, shown, near)
            .ok_or("nothing of that place or near it is on a page")?;
        // That is the line; the page map knows the character.
        let exact = document.pages().get(position.page).and_then(|page| {
            let map = self
                .maps
                .entry(position.page)
                .or_insert_with(|| PageMap::build(&world, &page.frame));
            let caret = map.caret(file, offset).map(|caret| caret.cell);
            caret.or_else(|| map.object(file, offset).first().copied())
        });
        Ok(match exact {
            Some(cell) => (position.page + 1, cell.min.x.to_pt(), cell.min.y.to_pt()),
            None => (position.page + 1, position.x, position.y),
        })
    }

    /// The file, line and column of the text that is shown on `page`
    /// (counting from 1) at the point `x` across and `y` down it, in
    /// points, or nearest to it.
    pub fn source_of(
        &mut self,
        page: usize,
        x: f64,
        y: f64,
    ) -> Result<(String, usize, usize), String> {
        let (_, document, world) = self.current_pages()?;
        let (document, world) = (document.clone(), world.clone());
        let pages = document.pages().len();
        let index = page
            .checked_sub(1)
            .filter(|index| *index < pages)
            .ok_or_else(|| {
                format!("page {page} does not exist; the document has {pages} page(s)")
            })?;
        let map = self
            .maps
            .entry(index)
            .or_insert_with(|| PageMap::build(&world, &document.pages()[index].frame));
        let point = typst::layout::Point::new(typst::layout::Abs::pt(x), typst::layout::Abs::pt(y));
        let (found, offset) = map
            .source_at(point)
            .ok_or("that page shows no text of the document's own")?;
        // The file may be another than the one being edited: a chapter
        // the main file includes.
        let source = world
            .written(found)
            .map_err(|_| "the file of the text there is gone".to_owned())?;
        let name = found.vpath().get_without_slash().to_owned();
        let (line, column) = line_column(source.text(), offset);
        Ok((name, line, column))
    }

    /// Writes the text to its file, as `:w` does. The error says why it
    /// could not.
    pub fn agent_save(&mut self) -> Result<(), String> {
        if self.editor.is_none() {
            return Err(NOT_EDITING.to_owned());
        }
        if self.save(false) {
            return Ok(());
        }
        let why = self.message.as_ref().map(|message| message.text.clone());
        Err(why.unwrap_or_else(|| "not written".to_owned()))
    }

    /// What it takes to write the document as a PDF: its pages, if they
    /// are those of the text as it is, the sources they were made from,
    /// and where its main file is.
    pub fn for_export(&self) -> Result<Export, String> {
        let (_, document, world) = self.current_pages()?;
        Ok(Export {
            document: document.clone(),
            world: world.clone(),
            main: self.project.main_path().map(std::path::Path::to_owned),
        })
    }

    /// The editor and the compiled pages, if they are those of the text
    /// as it is.
    #[allow(clippy::type_complexity, reason = "three things that go together")]
    fn current_pages(
        &self,
    ) -> Result<
        (
            &super::Editor,
            &std::sync::Arc<typst_layout::PagedDocument>,
            &std::sync::Arc<zetwerk_typst::ProjectWorld>,
        ),
        String,
    > {
        let editor = self.editor.as_ref().ok_or(NOT_EDITING)?;
        if self.looks_back() {
            // The pages are those of another state of the history: of
            // a text that is not the text, which nothing is to be told
            // of or written from.
            return Err(
                "the user is looking through the history of the text: the pages \
                 show another state of it; ask again in a moment"
                    .to_owned(),
            );
        }
        match (&self.document, &self.compiled_world) {
            (Some(document), Some(world)) if self.document_revision == Some(self.revision) => {
                Ok((editor, document, world))
            }
            _ if self.problems.is_empty() => {
                Err("the document is being compiled; ask again in a moment".to_owned())
            }
            _ => Err(format!(
                "the text does not compile, so its pages are not known: {}",
                self.problems.join("; ")
            )),
        }
    }
}

/// What an agent is told where the window has no document: a window
/// without one is at its start screen, where the user chooses one.
pub(super) const NOT_EDITING: &str = "no document is open: the window is at its start screen, \
                                      where the user chooses one. Ask the user to open it \
                                      (:open <file>, :recent), or work on a file without the \
                                      window: zetwerk mcp <file.typ>";
