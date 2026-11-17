//! What the author does with suggestions: one key each, and many at
//! once by command.
//!
//! A change is made the way an agent's change is made (see
//! [`Viewer::make_suggested`]): through the editor, as one undo step, as
//! the agent's; many accepted at once are one step too. `u` takes such a
//! step back and the suggestions are open again (see `sync_review`).
//! What leaves the text as it is (rejecting, putting aside) is taken back
//! with `:review undo`.

use std::ops::Range;

use super::super::Edited;
use super::super::agent::Proposal;
use super::super::assist::Target;
use super::store::found_again;
use super::{
    Asking, Decided, Kind, Level, Rewriting, Said, State, Undo, Viewer, Who, part, shifted,
};
use crate::editor::{self, Shown};
use crate::keys::{Key, KeyPress};

/// The kinds of review the author can ask agents for (`gQ`), and what
/// each asks.
pub(super) const REVIEWS: &[(&str, &str)] = &[
    ("proofread", "spelling, grammar, punctuation, typos"),
    (
        "clarity",
        "what is hard to follow, and plainer ways to say it",
    ),
    ("concision", "what can go, or be said shorter"),
    (
        "tone",
        "what is too formal, too loose, or not like the rest",
    ),
    (
        "structure",
        "the order of paragraphs and sections, what is missing",
    ),
    ("facts", "claims that need a source, citations, numbers"),
    (
        "consistency",
        "terms, names, notation, spelling used two ways",
    ),
    ("maths", "equations, symbols and their definitions agree"),
    ("anything", "whatever a good reader would say"),
];

/// A test a suggestion passes to be chosen.
type Test<'a> = Box<dyn Fn(&super::Suggestion) -> bool + 'a>;

/// What `:review accept …` and the like are about.
struct Chosen {
    ids: Vec<u64>,
    /// What was asked for, in words.
    said: String,
}

impl Viewer {
    /// The commands of reviewing. Returns whether `command` was one;
    /// some (`accept`, `reject`, `ask`, `proposal …`) are only where the
    /// cursor is on a suggestion, and left to the older ways else.
    pub(in crate::viewer) fn review_command(&mut self, command: &str) -> bool {
        let (name, rest) = command
            .split_once(' ')
            .map_or((command, ""), |(name, rest)| (name, rest.trim()));
        let reviewing = matches!(
            name,
            "review"
                | "accept"
                | "reject"
                | "reject!"
                | "answer"
                | "answer!"
                | "always"
                | "always!"
                | "rewrite"
                | "alternatives"
                | "aside"
                | "request"
                | "brief"
                | "proposal"
                | "ask"
        );
        if !reviewing {
            return false;
        }
        if self.editor.is_none() {
            if matches!(name, "ask" | "proposal" | "accept" | "reject") {
                return false;
            }
            self.alert(super::super::agent::NOT_EDITING);
            return true;
        }
        self.review_ready();
        let here = self.suggestion_here();
        match (name, rest) {
            ("review", rest) => self.review_subcommand(rest),
            ("accept" | "reject", "all") => {
                let verb = if name == "accept" { "accept" } else { "reject" };
                self.review_many(verb, "all");
            }
            ("accept", "") if here.is_some() => self.accept_here(),
            ("reject", why)
                if here.is_some() || self.review_asked_about(Asking::Reject).is_some() =>
            {
                self.reject_here(why, false);
            }
            ("reject!", why) => self.reject_here(why, true),
            ("answer" | "answer!", "") => self.begin_answer(),
            ("answer" | "answer!", words) => self.answer_here(words, name == "answer!"),
            ("always" | "always!", words) => self.add_instruction(words, name == "always!"),
            ("rewrite", _) => self.rewrite_here(),
            ("alternatives", words) => self.ask_alternatives(words),
            ("aside", _) => self.later_here(),
            ("request", _) => self.begin_review_request(),
            ("brief", rest) => self.brief_command(rest),
            ("proposal", "next" | "previous") if self.has_suggestions() => {
                self.goto_suggestion(rest == "next");
            }
            ("proposal", "flip") if here.is_some_and(|at| self.has_versions(at)) => {
                self.next_wording();
            }
            ("ask", "") if here.is_some() => self.begin_answer(),
            _ => return false,
        }
        true
    }

    /// `:review …`.
    fn review_subcommand(&mut self, rest: &str) {
        let (verb, more) = rest
            .split_once(' ')
            .map_or((rest, ""), |(verb, more)| (verb, more.trim()));
        match verb {
            "" => self.toggle_review_manager(),
            "accept" | "reject" | "later" => self.review_many(verb, more),
            "undo" => self.review_undo(),
            "record" | "record!" => self.write_review_record(verb == "record!"),
            "this" => self.make_review_request(more),
            "next" | "previous" => self.goto_suggestion(verb == "next"),
            "go" => match more.parse::<u64>() {
                Ok(id) => self.go_to_suggestion(id),
                Err(_) => self.alert("go to which suggestion? :review go 3"),
            },
            "reopen" => match more.parse::<u64>() {
                Ok(id) => self.reopen_suggestions(&[id]),
                Err(_) => self.alert("open which suggestion again? :review reopen 3"),
            },
            "only" | "all" => self.review_manager_command(verb, more),
            "brief" => self.brief_command(more),
            _ => self.alert(
                "not a review command: :review [accept|reject|later what] [undo] [record] \
                 [this kind] [brief]",
            ),
        }
    }

    /// Whether the file being edited has suggestions to go to.
    fn has_suggestions(&self) -> bool {
        let file = self.file_id();
        self.review
            .all
            .iter()
            .any(|suggestion| suggestion.file == file && suggestion.shown_on_text())
    }

    /// Whether suggestion `at` has other wordings to step through.
    fn has_versions(&self, at: usize) -> bool {
        !self.review.all[at].alternatives.is_empty()
    }

    /// The suggestion a line typed for `what` is about, if the cursor is
    /// still where it was when it was begun.
    fn review_asked_about(&self, what: Asking) -> Option<u64> {
        let cursor = self.editor.as_ref()?.cursor();
        match &self.review.asking {
            Some((asked, Some(id), then)) if *asked == what && *then == cursor => Some(*id),
            _ => None,
        }
    }

    /// The suggestion a line typed for `what` is about, or the one here.
    fn review_about(&mut self, what: Asking) -> Option<usize> {
        let asked = self.review_asked_about(what);
        self.review.asking = None;
        asked
            .and_then(|id| self.suggestion_index(id))
            .or_else(|| self.suggestion_here())
    }

    /// Opens the command line with `begun` on it, for a line about
    /// suggestion `about`.
    pub(super) fn begin_review_line(&mut self, what: Asking, about: Option<u64>, begun: &str) {
        if self.review.manager.take().is_some() {
            self.review.back_to_manager = true;
        }
        let Some(editor) = &mut self.editor else {
            return;
        };
        for c in format!(":{begun}").chars() {
            editor.key(KeyPress::plain(Key::Char(c)), &Shown::Pages(None));
        }
        self.review.asking = Some((what, about, editor.cursor()));
    }

    /// After a line typed for the manager, it is back.
    fn back_to_review_manager(&mut self) {
        if std::mem::take(&mut self.review.back_to_manager) {
            self.open_review_manager();
        }
    }

    /// `ga` on a suggestion: made as it is shown; a remark taken as read.
    fn accept_here(&mut self) {
        let Some(at) = self.suggestion_here() else {
            return;
        };
        let id = self.review.all[at].id;
        let accepted = self.accept_suggestions(&[id]);
        if accepted == 0 && self.review.all[at].stale {
            self.alert("its text has changed since it was made: gr drops it, gq asks again");
        } else if accepted > 0 {
            let left = self.open_left();
            self.notice(format!("accepted; {left} open · u takes it back · ]p next"));
        }
    }

    /// How many suggestions of the file being edited wait.
    fn open_left(&self) -> usize {
        let file = self.file_id();
        self.review
            .all
            .iter()
            .filter(|suggestion| suggestion.file == file && suggestion.state == State::Open)
            .count()
    }

    /// Accepts the suggestions `ids` of the file being edited: their
    /// changes made as one undo step. Returns how many.
    pub(super) fn accept_suggestions(&mut self, ids: &[u64]) -> usize {
        let remarks: Vec<usize> = ids
            .iter()
            .filter_map(|id| self.suggestion_index(*id))
            .filter(|at| {
                let suggestion = &self.review.all[*at];
                !suggestion.kind.changes() && !suggestion.state.decided()
            })
            .collect();
        let mut states = Vec::new();
        for at in &remarks {
            let suggestion = &mut self.review.all[*at];
            states.push((suggestion.id, suggestion.state, suggestion.waiting));
            suggestion.state = State::Accepted;
            suggestion.waiting = false;
            let said = Said::Accepted {
                before: String::new(),
                after: String::new(),
            };
            self.review_said(*at, Who::Author, said);
        }
        if !states.is_empty() {
            self.review.decided.push(Decided::States(states));
        }
        let made = self.make_suggested(ids, false);
        if let Some(last) = ids.last() {
            let cursor = self.editor.as_ref().map_or(0, editor::Editor::cursor);
            self.review.last_acted = Some((*last, cursor, String::new()));
        }
        self.review_changed();
        made.len() + remarks.len()
    }

    /// Makes the changes of the suggestions `ids` of the file being
    /// edited that wait and whose text is as it was: one undo step, as an
    /// agent's change is made (the change an agent proposed, accepted by
    /// the author: [`Edited::Agent`], as `accept_proposals` makes it).
    /// Everything else that keeps to the text is moved by each change
    /// exactly. Unless `rewriting`, each is recorded as accepted. Returns
    /// those made.
    #[allow(
        clippy::too_many_lines,
        reason = "the changes made, and all that keeps to the text moved with them"
    )]
    pub(super) fn make_suggested(&mut self, ids: &[u64], rewriting: bool) -> Vec<u64> {
        let file = self.file_id();
        let mut chosen: Vec<(u64, u64, Range<usize>, String)> = ids
            .iter()
            .filter_map(|id| {
                let suggestion = self.review.all.iter().find(|s| s.id == *id)?;
                if suggestion.file != file
                    || suggestion.state.decided()
                    || !suggestion.kind.changes()
                {
                    return None;
                }
                let proposal = self
                    .proposals
                    .iter()
                    .find(|proposal| Some(proposal.id) == suggestion.proposal)?;
                (!proposal.stale).then(|| {
                    (
                        *id,
                        proposal.id,
                        proposal.range.clone(),
                        proposal.new.clone(),
                    )
                })
            })
            .collect();
        chosen.sort_by_key(|(_, _, range, _)| (range.start, range.end));
        // Of changes to the same text, the first.
        let mut end = 0;
        chosen.retain(|(_, _, range, _)| {
            let apart = range.start >= end || (range.is_empty() && range.start > end);
            if apart {
                end = range.end.max(range.start + usize::from(range.is_empty()));
            }
            apart
        });
        if chosen.is_empty() {
            return Vec::new();
        }
        let Some(editor) = &self.editor else {
            return Vec::new();
        };
        let before = editor.history().current();
        let anchors_before = self.review_anchors(file);
        let text = editor.text().to_owned();
        let edits: Vec<(Range<usize>, &str)> = chosen
            .iter()
            .rev()
            .map(|(_, _, range, new)| (range.clone(), new.as_str()))
            .collect();
        let Some(editor) = &mut self.editor else {
            return Vec::new();
        };
        if let Err(why) = editor.replace(&edits) {
            self.alert(format!("not changed: {why}"));
            return Vec::new();
        }
        let first = chosen[0].2.start;
        editor.set_cursor(first);
        let now = editor.text().to_owned();
        let edits_made: Vec<(Range<usize>, usize)> = chosen
            .iter()
            .map(|(_, _, range, new)| (range.clone(), new.len()))
            .collect();
        let gone: Vec<u64> = chosen.iter().map(|(_, proposal, ..)| *proposal).collect();
        self.proposals
            .retain(|proposal| !gone.contains(&proposal.id));
        let moved = |range: &Range<usize>| {
            let from = shifted(range.start, &edits_made, false);
            from..shifted(range.end, &edits_made, true).max(from)
        };
        for proposal in self.proposals.iter_mut().filter(|p| p.file == file) {
            proposal.range = moved(&proposal.range);
            proposal.stale = proposal.stale || part(&now, &proposal.range) != Some(&proposal.old);
        }
        for note in self.notes.all.iter_mut().filter(|n| n.file == file) {
            note.range = moved(&note.range);
            note.detached = note.range.is_empty() && !note.quote.is_empty();
        }
        for suggestion in &mut self.review.all {
            if suggestion.file == file && !suggestion.kind.changes() {
                suggestion.range = moved(&suggestion.range);
            }
        }
        for request in self.review.requests.iter_mut().filter(|r| r.file == file) {
            request.range = moved(&request.range);
        }
        for instruction in &mut self.review.brief {
            if let (Some((range, _)), true) =
                (&mut instruction.protect, instruction.file == Some(file))
            {
                *range = moved(range);
            }
        }
        if !self.proposals.is_empty() {
            self.proposal_text = Some(now.clone());
        }
        if self.notes.seen.as_ref().is_some_and(|(of, _)| *of == file) {
            self.notes.seen = Some((file, now.clone()));
        }
        self.review.seen = Some((file, now));
        self.text_changed(Edited::Agent);
        let after = self
            .editor
            .as_ref()
            .map_or(before, |editor| editor.history().current());
        let anchors_after = self.review_anchors(file);
        let mut made = Vec::new();
        for (id, _, range, new) in &chosen {
            let Some(at) = self.suggestion_index(*id) else {
                continue;
            };
            let suggestion = &mut self.review.all[at];
            suggestion.proposal = None;
            suggestion.state = State::Accepted;
            suggestion.waiting = false;
            suggestion.new.clone_from(new);
            made.push((*id, range.clone()));
            if !rewriting {
                let said = Said::Accepted {
                    before: text[range.clone()].to_owned(),
                    after: new.clone(),
                };
                self.review_said(at, Who::Author, said);
            }
        }
        self.review.undo.push(Undo {
            file,
            before,
            after,
            applied: true,
            made: made.clone(),
            anchors_before,
            anchors_after,
        });
        if self.review.undo.len() > 64 {
            self.review.undo.remove(0);
        }
        self.review.decided.push(Decided::Text);
        self.review_changed();
        made.into_iter().map(|(id, _)| id).collect()
    }

    /// `gr`, `:reject why`: the suggestion here rejected, and why if the
    /// author said; with `always`, what was said made an instruction.
    fn reject_here(&mut self, why: &str, always: bool) {
        let Some(at) = self.review_about(Asking::Reject) else {
            return self.alert("the cursor is on no suggestion; ]p goes to the next one");
        };
        let id = self.review.all[at].id;
        self.reject_suggestions(&[id], why);
        let cursor = self.editor.as_ref().map_or(0, editor::Editor::cursor);
        self.review.last_acted = Some((id, cursor, why.to_owned()));
        if always {
            self.review.asking = Some((Asking::Always, Some(id), cursor));
            self.add_instruction(why, false);
        } else {
            let left = self.open_left();
            self.notice(if why.is_empty() {
                format!("rejected; {left} open · g! makes it an instruction · :review undo")
            } else {
                format!(
                    "rejected, and the agent is told why; {left} open · g! makes it an instruction"
                )
            });
        }
        self.back_to_review_manager();
    }

    /// Rejects the suggestions `ids`. Returns how many.
    pub(super) fn reject_suggestions(&mut self, ids: &[u64], why: &str) -> usize {
        let mut states = Vec::new();
        for id in ids {
            let Some(at) = self.suggestion_index(*id) else {
                continue;
            };
            let suggestion = &mut self.review.all[at];
            if suggestion.state.decided() {
                continue;
            }
            states.push((suggestion.id, suggestion.state, suggestion.waiting));
            suggestion.state = State::Rejected;
            suggestion.waiting = false;
            let proposal = suggestion.proposal.take();
            self.proposals
                .retain(|waiting| Some(waiting.id) != proposal);
            self.proposals_elsewhere
                .retain(|waiting| Some(waiting.id) != proposal);
            self.review_said(at, Who::Author, Said::Rejected(why.trim().to_owned()));
        }
        let count = states.len();
        if count > 0 {
            self.review.decided.push(Decided::States(states));
        }
        self.review_changed();
        count
    }

    /// `gz`: the suggestion here put aside, to decide on later.
    fn later_here(&mut self) {
        let Some(at) = self.suggestion_here() else {
            return self.alert("the cursor is on no suggestion; ]p goes to the next one");
        };
        let id = self.review.all[at].id;
        self.put_aside(&[id]);
        self.notice("put aside: gR lists it under later · ]p next");
    }

    pub(super) fn put_aside(&mut self, ids: &[u64]) -> usize {
        let mut states = Vec::new();
        for id in ids {
            let Some(at) = self.suggestion_index(*id) else {
                continue;
            };
            let suggestion = &mut self.review.all[at];
            if suggestion.state != State::Open {
                continue;
            }
            states.push((suggestion.id, suggestion.state, suggestion.waiting));
            suggestion.state = State::Later;
            self.review_said(at, Who::Author, Said::Later);
        }
        let count = states.len();
        if count > 0 {
            self.review.decided.push(Decided::States(states));
        }
        self.review_changed();
        count
    }

    /// Opens the suggestions `ids` again, whatever was decided on them
    /// without changing the text.
    pub(super) fn reopen_suggestions(&mut self, ids: &[u64]) {
        let mut opened = 0;
        for id in ids {
            let Some(at) = self.suggestion_index(*id) else {
                continue;
            };
            if matches!(
                self.review.all[at].state,
                State::Accepted | State::Rewritten
            ) && self.review.all[at].kind.changes()
            {
                self.alert("its change is made: u takes it back");
                continue;
            }
            self.open_again(at, State::Open, false);
            opened += 1;
        }
        if opened > 0 {
            self.review_changed();
            self.notice(format!("open again: {opened}"));
        }
    }

    /// Suggestion `at` is `state` again, and waits as a proposal again
    /// where its text is there.
    fn open_again(&mut self, at: usize, state: State, waiting: bool) {
        let text = self.text().unwrap_or_default().to_owned();
        let file = self.file_id();
        let suggestion = &mut self.review.all[at];
        suggestion.state = state;
        suggestion.waiting = waiting;
        if suggestion.kind.changes() && suggestion.proposal.is_none() && suggestion.file == file {
            let range = if suggestion.quote.is_empty() {
                Some(suggestion.range.clone()).filter(|range| range.end <= text.len())
            } else {
                found_again(&text, &suggestion.quote, &suggestion.range)
            };
            if let Some(range) = range {
                let id = self.next_proposal;
                self.next_proposal += 1;
                suggestion.range = range.clone();
                suggestion.proposal = Some(id);
                suggestion.stale = false;
                self.proposals.push(Proposal {
                    id,
                    file,
                    range,
                    old: suggestion.quote.clone(),
                    new: suggestion.new.clone(),
                    stale: false,
                });
                self.proposals.sort_by_key(|proposal| proposal.range.start);
                self.proposal_text = Some(text);
            } else {
                suggestion.stale = true;
            }
        }
        self.review_said(at, Who::Author, Said::Reopened);
    }

    /// `:review undo`: the last decision taken back.
    fn review_undo(&mut self) {
        match self.review.decided.pop() {
            None => self.notice("nothing decided to take back"),
            Some(Decided::Text) => {
                let file = self.file_id();
                let state = self
                    .editor
                    .as_ref()
                    .map(|editor| editor.history().current());
                let last = self
                    .review
                    .undo
                    .iter()
                    .rev()
                    .find(|undo| undo.file == file && undo.applied);
                if last.is_some_and(|undo| Some(undo.after) == state) {
                    self.editor_key(KeyPress::plain(Key::Char('u')));
                } else {
                    self.alert("the text has changed since: u steps back to it, or gR reopens it");
                }
            }
            Some(Decided::States(states)) => {
                for (id, state, waiting) in &states {
                    if let Some(at) = self.suggestion_index(*id) {
                        self.open_again(at, *state, *waiting);
                    }
                }
                self.review_changed();
                self.notice(format!("taken back: {} open again", states.len()));
            }
        }
    }

    /// `gA`: the agent's text put in, the cursor in it typing; what the
    /// author makes of it is recorded once typing ends.
    pub(super) fn rewrite_here(&mut self) {
        let Some(at) = self.suggestion_here() else {
            return self.alert("the cursor is on no suggestion; ]p goes to the next one");
        };
        let suggestion = &self.review.all[at];
        if !suggestion.kind.changes() {
            return self.alert("a remark changes no text: gq answers it, ga takes it as read");
        }
        let (id, new) = (suggestion.id, suggestion.new.clone());
        let Some(range) = self
            .proposals
            .iter()
            .find(|proposal| Some(proposal.id) == suggestion.proposal)
            .map(|proposal| proposal.range.clone())
        else {
            return;
        };
        if self.make_suggested(&[id], true).is_empty() {
            return self.alert("its text has changed since it was made: gr drops it");
        }
        let file = self.file_id();
        let written = range.start..range.start + new.len();
        self.review.rewriting = Some(Rewriting {
            id,
            file,
            range: written.clone(),
        });
        // What is typed from now on moves it: the text it is a place in.
        let now = self.text().unwrap_or_default().to_owned();
        self.review.seen = Some((file, now));
        let Some(editor) = &mut self.editor else {
            return;
        };
        let last = new.chars().next_back().map_or(0, char::len_utf8);
        if new.is_empty() || new.ends_with('\n') {
            editor.set_cursor(written.end);
            self.editor_key(KeyPress::plain(Key::Char('i')));
        } else {
            editor.set_cursor(written.end - last);
            self.editor_key(KeyPress::plain(Key::Char('a')));
        }
        self.notice("rewrite it as you like; Esc when done (u twice takes all of it back)");
    }

    /// Records what the author made of a rewritten suggestion, once
    /// typing has ended.
    pub(super) fn rewritten_done(&mut self) {
        let Some(rewriting) = &self.review.rewriting else {
            return;
        };
        let typing = self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.mode() == editor::Mode::Insert);
        if typing || self.file_id() != rewriting.file {
            return;
        }
        let Some(rewriting) = self.review.rewriting.take() else {
            return;
        };
        let Some(at) = self.suggestion_index(rewriting.id) else {
            return;
        };
        let after = self
            .text()
            .and_then(|text| text.get(rewriting.range.clone()))
            .unwrap_or_default()
            .to_owned();
        let suggestion = &mut self.review.all[at];
        let (before, offered) = (suggestion.quote.clone(), suggestion.new.clone());
        let said = if after == offered {
            Said::Accepted {
                before,
                after: offered,
            }
        } else {
            suggestion.state = State::Rewritten;
            Said::Rewritten {
                before,
                offered,
                after,
            }
        };
        let cursor = self.editor.as_ref().map_or(0, editor::Editor::cursor);
        self.review.last_acted = Some((rewriting.id, cursor, String::new()));
        self.review_said(at, Who::Author, said);
        self.review_changed();
    }

    /// `gq` on a suggestion: the command line opened for a line to the
    /// agent about it.
    fn begin_answer(&mut self) {
        let Some(at) = self.suggestion_here() else {
            return self.alert("the cursor is on no suggestion; ]p goes to the next one");
        };
        let id = self.review.all[at].id;
        self.begin_review_line(Asking::Answer, Some(id), "answer ");
    }

    /// `:answer words`: said to the agent on the suggestion here; it
    /// waits for the agent then.
    fn answer_here(&mut self, words: &str, always: bool) {
        let Some(at) = self.review_about(Asking::Answer) else {
            return self.alert("the cursor is on no suggestion; ]p goes to the next one");
        };
        let id = self.review.all[at].id;
        self.review.all[at].waiting = true;
        self.review_said(at, Who::Author, Said::Message(words.to_owned()));
        self.review_changed();
        let cursor = self.editor.as_ref().map_or(0, editor::Editor::cursor);
        self.review.last_acted = Some((id, cursor, words.to_owned()));
        if always {
            self.review.asking = Some((Asking::Always, Some(id), cursor));
            self.add_instruction(words, false);
        } else {
            self.notice("said; it waits for the agent (it sees it) · g! makes it an instruction");
        }
        self.back_to_review_manager();
    }

    /// `gp`: other wordings asked for; `zp` steps through them once the
    /// agent gave them.
    pub(super) fn ask_alternatives(&mut self, words: &str) {
        let Some(at) = self.suggestion_here() else {
            return self.alert("the cursor is on no suggestion; ]p goes to the next one");
        };
        if !self.review.all[at].kind.changes() {
            return self.alert("a remark has no wording to change: gq answers it");
        }
        self.review.all[at].waiting = true;
        self.review_said(at, Who::Author, Said::AskedAlternatives);
        if !words.trim().is_empty() {
            self.review_said(at, Who::Author, Said::Message(words.trim().to_owned()));
        }
        self.review_changed();
        self.notice("other wordings asked for; when they come, zp steps through them");
    }

    /// `zp` on a suggestion with other wordings: the next of them typeset
    /// in place, then the text as it is, then the first again.
    fn next_wording(&mut self) {
        let Some(at) = self.suggestion_here() else {
            return;
        };
        let suggestion = &self.review.all[at];
        let (proposal, count) = (suggestion.proposal, suggestion.alternatives.len() + 1);
        let Some(proposal) = proposal else {
            return;
        };
        let as_it_is = self.notes.before == Some(proposal);
        let shown = if as_it_is {
            self.notes.before = None;
            0
        } else if suggestion.shown + 1 < count {
            suggestion.shown + 1
        } else {
            self.notes.before = Some(proposal);
            0
        };
        let suggestion = &mut self.review.all[at];
        suggestion.shown = shown;
        let text = suggestion.version(shown).to_owned();
        suggestion.new.clone_from(&text);
        if let Some(waiting) = self.proposals.iter_mut().find(|p| p.id == proposal) {
            waiting.new = text;
        }
        self.review.generation += 1;
        self.notice(if self.notes.before == Some(proposal) {
            "as it is · zp the first wording again".to_owned()
        } else if shown == 0 {
            format!("the wording suggested first, of {count} · zp the next · ga takes it")
        } else {
            format!(
                "wording {} of {count} · zp the next · ga takes it",
                shown + 1
            )
        });
    }

    /// `]p`, `[p`: to the next suggestion of the file being edited that
    /// waits, in the order of the text; where several begin at one place,
    /// each in front in turn.
    fn goto_suggestion(&mut self, forward: bool) {
        let file = self.file_id();
        let mut order: Vec<(usize, u64)> = self
            .review
            .all
            .iter()
            .filter(|s| s.file == file && s.shown_on_text() && s.state == State::Open)
            .map(|s| (s.range.start, s.id))
            .collect();
        order.sort_unstable();
        let Some(cursor) = self.editor.as_ref().map(editor::Editor::cursor) else {
            return;
        };
        if order.is_empty() {
            return self.notice("no suggestions wait here: gQ asks agents for a review");
        }
        let now = order
            .iter()
            .position(|(start, id)| *start == cursor && Some(*id) == self.review.front);
        let target = match (now, forward) {
            (Some(at), true) => order.get(at + 1).or(order.first()),
            (Some(at), false) => at
                .checked_sub(1)
                .and_then(|at| order.get(at))
                .or(order.last()),
            (None, true) => order
                .iter()
                .find(|(start, _)| *start > cursor)
                .or(order.first()),
            (None, false) => order
                .iter()
                .rfind(|(start, _)| *start < cursor)
                .or(order.last()),
        };
        let Some((start, id)) = target.copied() else {
            return;
        };
        self.bring_to_front(id);
        if let Some(editor) = &mut self.editor {
            editor.jump(start);
        }
        self.follow_cursor();
        let place = order
            .iter()
            .position(|(_, of)| *of == id)
            .map_or(0, |at| at + 1);
        self.notice(format!("suggestion {place} of {}", order.len()));
    }

    /// Puts suggestion `id` in front of those on the same text: its
    /// proposal first, for it to be the one shown as it would be.
    pub(super) fn bring_to_front(&mut self, id: u64) {
        self.review.front = Some(id);
        let proposal = self
            .suggestion_index(id)
            .and_then(|at| self.review.all[at].proposal);
        if let Some(at) = self.proposals.iter().position(|p| Some(p.id) == proposal) {
            let moved = self.proposals.remove(at);
            let before = self
                .proposals
                .iter()
                .position(|p| p.range.end > moved.range.start || p.range.start >= moved.range.start)
                .unwrap_or(self.proposals.len());
            self.proposals.insert(before, moved);
        }
        self.review.generation += 1;
    }

    /// Puts the cursor on suggestion `id`, in its file.
    pub(super) fn go_to_suggestion(&mut self, id: u64) {
        let Some(at) = self.suggestion_index(id) else {
            return self.alert(format!("there is no suggestion {id}"));
        };
        let suggestion = &mut self.review.all[at];
        suggestion.unseen = false;
        let (file, start) = (suggestion.file, suggestion.range.start);
        self.bring_to_front(id);
        if file == self.file_id() {
            if let Some(editor) = &mut self.editor {
                editor.jump(start);
            }
            self.follow_cursor();
        } else {
            self.open_file(file, Some(start));
        }
    }

    /// The suggestions `words` name, of those that wait in the file being
    /// edited: `all`, `sure`, `here` (the section the cursor is in), a
    /// kind, a category, `by` an agent, `request` 2, `later` (those put
    /// aside too); all the words hold.
    fn suggestions_chosen_by(&self, words: &str) -> Result<Chosen, String> {
        let file = self.file_id();
        let cursor = self.editor.as_ref().map_or(0, editor::Editor::cursor);
        let section = self.editor.as_ref().and_then(|editor| {
            let all = crate::sections::sections(editor.source());
            crate::sections::at(&all, cursor).map(|index| all[index].span.clone())
        });
        let agents: Vec<&str> = self.review.all.iter().map(|s| s.by.as_str()).collect();
        let mut tests: Vec<Test<'_>> = Vec::new();
        let mut later = false;
        let mut said = Vec::new();
        let mut words = words.split_whitespace().peekable();
        while let Some(word) = words.next() {
            let word = word.to_lowercase();
            match word.as_str() {
                "all" => said.push("all".to_owned()),
                "later" => {
                    later = true;
                    said.push("put aside".to_owned());
                }
                "sure" => {
                    tests.push(Box::new(|s| s.confidence == Level::High));
                    said.push("sure".to_owned());
                }
                "here" | "section" => {
                    let Some(span) = section.clone() else {
                        return Err(
                            "the cursor is in no section: no heading is before it".to_owned()
                        );
                    };
                    tests.push(Box::new(move |s| span.contains(&s.range.start)));
                    said.push("in this section".to_owned());
                }
                "by" | "agent" => {
                    let name = words.next().unwrap_or_default().to_owned();
                    said.push(format!("by {name}"));
                    tests.push(Box::new(move |s| s.by.eq_ignore_ascii_case(&name)));
                }
                "request" => {
                    let number = words.next().and_then(|n| n.parse::<u64>().ok());
                    let Some(number) = number else {
                        return Err("which request? :review accept request 2".to_owned());
                    };
                    said.push(format!("of request {number}"));
                    tests.push(Box::new(move |s| s.request == Some(number)));
                }
                kind if Kind::named(kind).is_some() => {
                    let kind = Kind::named(kind).unwrap_or(Kind::Replace);
                    said.push(kind.name().to_owned());
                    tests.push(Box::new(move |s| s.kind == kind));
                }
                name if agents.iter().any(|agent| agent.eq_ignore_ascii_case(name)) => {
                    let name = name.to_owned();
                    said.push(format!("by {name}"));
                    tests.push(Box::new(move |s| s.by.eq_ignore_ascii_case(&name)));
                }
                category => {
                    let category = category.to_owned();
                    said.push(category.clone());
                    tests.push(Box::new(move |s| {
                        s.category.eq_ignore_ascii_case(&category)
                    }));
                }
            }
        }
        if said.is_empty() {
            return Err(
                "which? :review accept all, sure, here, spelling (a category), by claude, \
                 request 2, delete (a kind)"
                    .to_owned(),
            );
        }
        let ids = self
            .review
            .all
            .iter()
            .filter(|s| {
                s.file == file && (s.state == State::Open || later && s.state == State::Later)
            })
            .filter(|s| tests.iter().all(|test| test(s)))
            .map(|s| s.id)
            .collect();
        Ok(Chosen {
            ids,
            said: said.join(" "),
        })
    }

    /// `:review accept spelling` and the like: many at once, with how
    /// many; one step for `u` (changes) or `:review undo` (the rest).
    pub(super) fn review_many(&mut self, verb: &str, words: &str) {
        let chosen = match self.suggestions_chosen_by(words) {
            Ok(chosen) => chosen,
            Err(why) => return self.alert(why),
        };
        if chosen.ids.is_empty() {
            return self.notice(format!("no suggestion waits that is {}", chosen.said));
        }
        let (done, how) = match verb {
            "accept" => (self.accept_suggestions(&chosen.ids), "accepted"),
            "reject" => (self.reject_suggestions(&chosen.ids, ""), "rejected"),
            _ => (self.put_aside(&chosen.ids), "put aside"),
        };
        let left = chosen.ids.len() - done;
        let back = if verb == "accept" {
            "u takes them back"
        } else {
            ":review undo takes it back"
        };
        let stale = if left > 0 {
            format!("; {left} not: their text changed, or they change the same text")
        } else {
            String::new()
        };
        self.notice(format!("{how} {done} ({}){stale} · {back}", chosen.said));
    }

    /// `gQ`: what a review is asked for is taken (what is selected, or
    /// the section here), and the kinds of review are put up.
    fn begin_review_request(&mut self) {
        let Some(editor) = &self.editor else {
            return;
        };
        let cursor = editor.cursor();
        let range = editor
            .selection()
            .filter(|range| !range.is_empty())
            .or_else(|| {
                let all = crate::sections::sections(editor.source());
                crate::sections::at(&all, cursor).map(|index| all[index].span.clone())
            })
            .unwrap_or(0..editor.text().len());
        let file = editor.file();
        self.review.requesting = Some((file, range));
        let items = REVIEWS
            .iter()
            .map(|(name, does)| {
                (
                    format!("{name}\t{does}"),
                    Target::Command(format!("review this {name}")),
                )
            })
            .collect();
        self.open_picker_for(
            "review this for",
            items,
            0,
            String::new(),
            Some("review this ".to_owned()),
        );
        self.picker_by_itself();
    }

    /// `:review this clarity, and keep it short`: a review asked of
    /// agents, of what `gQ` took or the section here.
    fn make_review_request(&mut self, words: &str) {
        let words = words.trim();
        let (what, rest) = words
            .split_once(|c: char| c.is_whitespace() || c == ',')
            .map_or((words, ""), |(what, rest)| {
                (what, rest.trim_start_matches(',').trim())
            });
        let what = if what.is_empty() { "anything" } else { what };
        let Some(editor) = &self.editor else {
            return;
        };
        let (file, range) = self.review.requesting.take().unwrap_or_else(|| {
            let cursor = editor.cursor();
            let all = crate::sections::sections(editor.source());
            let span = crate::sections::at(&all, cursor)
                .map_or(0..editor.text().len(), |index| all[index].span.clone());
            (editor.file(), span)
        });
        if file != editor.file() {
            return self.alert("the review was begun in another file");
        }
        let quote = editor
            .text()
            .get(range.clone())
            .unwrap_or_default()
            .to_owned();
        if let Some(editor) = &mut self.editor
            && editor.selection().is_some()
        {
            let cursor = editor.cursor();
            editor.point(cursor);
        }
        let section = self.section_title_at(range.start);
        self.review.last_request += 1;
        let id = self.review.last_request;
        let seq = self.review_told();
        self.review.requests.push(super::Request {
            id,
            file,
            range,
            quote,
            what: what.to_lowercase(),
            words: rest.to_owned(),
            at: super::now(),
            seq,
            done: false,
            section,
        });
        self.review.seen = None;
        self.review_changed();
        self.notice(format!(
            "asked: agents see review request {id} ({what}); what they suggest comes back to it"
        ));
    }
}
