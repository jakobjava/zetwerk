//! What agents ask of reviewing, and what they are told.
//!
//! An agent suggests many things at once (`suggest`): each is placed by
//! exact text (with a section to look in, where the text alone is in
//! more than one place) or by the paragraph on a line, and the batch is
//! checked whole before any of it is made: each item is answered, as
//! placed (`anchored`), `not_found`, `ambiguous`, `overlaps` another of
//! the batch, `contradicts` an instruction of the brief, or `invalid`.
//! What an agent is told of a suggestion is what became of it: its
//! state, the thread with the author's answers, what the author made of
//! a rewritten one; so that it learns what was done with its work.

use std::ops::Range;

use serde_json::{Value, json};
use typst::syntax::FileId;

use super::super::agent::{Outcome, Proposal, line_column, offset_of};
use super::super::assist::cut;
use super::super::notes::line_at;
use super::{Entry, Kind, Level, Request, Said, State, Suggestion, Viewer, Who, now};

/// How much of the text a suggestion is about an agent is given.
const MAX_QUOTE: usize = 2000;
/// How many bytes before a place to put text in are kept to find it
/// again.
const CONTEXT: usize = 40;

/// Who an agent is: the name its client gave, and its session.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Agent {
    pub name: String,
    pub session: String,
}

impl Agent {
    /// An agent that said nothing of itself.
    #[must_use]
    pub fn unknown() -> Self {
        Self {
            name: "agent".to_owned(),
            session: String::new(),
        }
    }
}

/// A suggestion as an agent asks for it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Wanted {
    /// `replace`, `insert`, `delete`, `move`, `comment` or `question`.
    pub kind: String,
    /// The text it is about, exactly as in the source.
    pub old_str: Option<String>,
    /// The heading of the section to look for `old_str` in, or that a
    /// remark is about.
    pub section: Option<String>,
    /// A line: the paragraph on it is what it is about, or where
    /// `old_str` is looked for.
    pub line: Option<usize>,
    /// What to put in place of `old_str`, or in beside it.
    pub new_str: Option<String>,
    /// For `insert` and `move`: `before` or `after` (the default) the
    /// text it is put by.
    pub position: Option<String>,
    /// For `move`: the text it is to be put by.
    pub move_to: Option<String>,
    /// Why, in a sentence; for a remark or a question, what it says.
    pub reason: String,
    pub category: Option<String>,
    pub confidence: Option<String>,
    pub importance: Option<String>,
    pub alternatives: Vec<String>,
    /// Made even where it goes against an instruction: flagged then.
    pub insist: bool,
}

/// Where a suggestion goes, worked out from what an agent asked.
struct Placed {
    kind: Kind,
    /// What its proposal replaces, or what it is about.
    range: Range<usize>,
    /// What would be there.
    new: String,
    context: String,
}

/// Why an item of a batch was not made: its status and what is wrong.
type Refused = (&'static str, String);

/// An item of a batch checked: where it goes and the instructions it
/// goes against, or why not and the instruction that refuses it.
type Checked = Result<(Placed, Vec<u64>), (Refused, Option<u64>)>;

/// The bytes of `text` that `old` is at, once, within `within`.
fn find_once(text: &str, old: &str, within: &Range<usize>) -> Result<Range<usize>, Refused> {
    let region = &text[within.clone()];
    let found: Vec<usize> = region
        .match_indices(old)
        .map(|(at, _)| within.start + at)
        .collect();
    match found.as_slice() {
        [] => Err((
            "not_found",
            "old_str is not in the text (there); read the source again, it may have changed"
                .to_owned(),
        )),
        [at] => Ok(*at..*at + old.len()),
        _ => {
            let lines: Vec<String> = found
                .iter()
                .take(8)
                .map(|at| line_column(text, *at).0.to_string())
                .collect();
            Err((
                "ambiguous",
                format!(
                    "old_str is in the text {} times (lines {}); give more of the text around it, \
                     or the section it is in",
                    found.len(),
                    lines.join(", ")
                ),
            ))
        }
    }
}

/// `text` as a value an agent is given: cut where it is long.
fn quoted(text: &str) -> String {
    cut(text, MAX_QUOTE)
}

/// An entry of a thread as an agent is given it.
fn entry_json(entry: &Entry) -> Value {
    let mut value = json!({
        "seq": entry.seq,
        "at": entry.at,
        "by": entry.by.name(),
        "what": entry.said.tag(),
    });
    let fields = match &entry.said {
        Said::Message(text) => json!({ "text": text }),
        Said::Version { text, reason } => json!({ "new_str": text, "reason": reason }),
        Said::Alternatives(all) => json!({ "alternatives": all }),
        Said::Accepted { before, after } => json!({ "before": before, "after": after }),
        Said::Rewritten {
            before,
            offered,
            after,
        } => json!({ "before": before, "offered": offered, "after": after }),
        Said::Rejected(why) if !why.is_empty() => json!({ "why": why }),
        Said::Instruction(text) => json!({ "instruction": text }),
        _ => json!({}),
    };
    if let (Some(value), Some(fields)) = (value.as_object_mut(), fields.as_object()) {
        value.extend(fields.clone());
    }
    value
}

impl Viewer {
    /// The section titled `title` (as written, or the same but for case)
    /// of the file being edited: its span.
    fn section_named(&self, title: &str) -> Result<Range<usize>, Refused> {
        let editor = self
            .editor
            .as_ref()
            .ok_or(("invalid", NOT_EDITING.to_owned()))?;
        let all = crate::sections::sections(editor.source());
        let wanted = title.trim().trim_start_matches('=').trim().to_lowercase();
        let found: Vec<&crate::sections::Section> = all
            .iter()
            .filter(|section| section.title.trim().to_lowercase() == wanted)
            .collect();
        match found.as_slice() {
            [section] => Ok(section.span.clone()),
            [] => Err((
                "not_found",
                format!("there is no section {title:?}; get_outline gives the headings"),
            )),
            _ => Err((
                "ambiguous",
                format!(
                    "{} sections are called {title:?}; give a line instead",
                    found.len()
                ),
            )),
        }
    }

    /// Where what `wanted` asks for goes in the text as it is.
    #[allow(
        clippy::too_many_lines,
        reason = "each kind of suggestion placed in its way"
    )]
    fn place_wanted(&self, wanted: &Wanted) -> Result<Placed, Refused> {
        let text = self.text().ok_or(("invalid", NOT_EDITING.to_owned()))?;
        let kind = Kind::named(wanted.kind.trim()).ok_or_else(|| {
            (
                "invalid",
                format!(
                    "kind {:?} is none of replace, insert, delete, move, comment, question",
                    wanted.kind
                ),
            )
        })?;
        let within = match (&wanted.section, wanted.line) {
            (Some(section), _) => self.section_named(section)?,
            (None, Some(line)) => {
                let at = offset_of(text, line, 1)
                    .ok_or_else(|| ("not_found", format!("the text has no line {line}")))?;
                line_at(text, at).ok_or_else(|| ("not_found", format!("line {line} is empty")))?
            }
            (None, None) => 0..text.len(),
        };
        let about = match wanted.old_str.as_deref() {
            Some("") => return Err(("invalid", "old_str is empty".to_owned())),
            Some(old) => find_once(text, old, &within)?,
            None if !kind.changes() || wanted.line.is_some() => {
                if wanted.section.is_some() && wanted.line.is_none() {
                    // A remark on a section: on its heading.
                    let end = text[within.start..]
                        .find('\n')
                        .map_or(within.end, |at| within.start + at);
                    within.start..end
                } else {
                    within
                }
            }
            None => {
                return Err((
                    "invalid",
                    "give old_str (the exact text), or a line for the paragraph".to_owned(),
                ));
            }
        };
        let new = wanted.new_str.clone().unwrap_or_default();
        let after = wanted
            .position
            .as_deref()
            .is_none_or(|position| position != "before");
        let placed = match kind {
            Kind::Replace => {
                if wanted.new_str.is_none() {
                    return Err(("invalid", "a replace needs new_str".to_owned()));
                }
                if new == text[about.clone()] {
                    return Err(("invalid", "new_str is what is there already".to_owned()));
                }
                Placed {
                    kind,
                    range: about,
                    new,
                    context: String::new(),
                }
            }
            Kind::Insert => {
                if new.is_empty() {
                    return Err(("invalid", "an insert needs new_str".to_owned()));
                }
                let at = if after { about.end } else { about.start };
                let mut from = at.saturating_sub(CONTEXT);
                while !text.is_char_boundary(from) {
                    from -= 1;
                }
                Placed {
                    kind,
                    range: at..at,
                    new,
                    context: text[from..at].to_owned(),
                }
            }
            Kind::Delete => Placed {
                kind,
                range: about,
                new: String::new(),
                context: String::new(),
            },
            Kind::Move => {
                let to = wanted
                    .move_to
                    .as_deref()
                    .filter(|to| !to.is_empty())
                    .ok_or((
                        "invalid",
                        "a move needs move_to: the text to put it by".to_owned(),
                    ))?;
                let target = find_once(text, to, &(0..text.len()))?;
                let at = if after { target.end } else { target.start };
                let moved = &text[about.clone()];
                if at > about.start && at < about.end {
                    return Err(("invalid", "move_to is in the text to move".to_owned()));
                }
                let (range, new) = if at <= about.start {
                    (at..about.end, format!("{moved}{}", &text[at..about.start]))
                } else {
                    (about.start..at, format!("{}{moved}", &text[about.end..at]))
                };
                Placed {
                    kind,
                    range,
                    new,
                    context: String::new(),
                }
            }
            Kind::Comment | Kind::Question => {
                if wanted.reason.trim().is_empty() {
                    return Err((
                        "invalid",
                        "a comment or question says what it is in reason".to_owned(),
                    ));
                }
                Placed {
                    kind,
                    range: about,
                    new: String::new(),
                    context: String::new(),
                }
            }
        };
        Ok(placed)
    }

    /// Makes suggestion `placed` as `agent` asked for it, answering
    /// `request`. Returns its number.
    fn make_suggestion(
        &mut self,
        placed: Placed,
        wanted: &Wanted,
        agent: &super::Agent,
        request: Option<u64>,
        flagged: Vec<u64>,
    ) -> u64 {
        let file = self.file_id();
        let text = self.text().unwrap_or_default();
        let quote = text[placed.range.clone()].to_owned();
        let proposal = placed.kind.changes().then(|| {
            let id = self.next_proposal;
            self.next_proposal += 1;
            id
        });
        let section = self.section_title_at(placed.range.start);
        self.review.last += 1;
        let id = self.review.last;
        if let Some(proposal) = proposal {
            let text = self.text().unwrap_or_default().to_owned();
            self.proposals.push(Proposal {
                id: proposal,
                file,
                range: placed.range.clone(),
                old: quote.clone(),
                new: placed.new.clone(),
                stale: false,
            });
            self.proposals.sort_by_key(|waiting| waiting.range.start);
            self.proposal_text = Some(text);
        }
        let category = wanted
            .category
            .as_deref()
            .map(|category| category.trim().to_lowercase())
            .filter(|category| !category.is_empty())
            .unwrap_or_else(|| default_category(placed.kind).to_owned());
        self.review.all.push(Suggestion {
            id,
            file,
            kind: placed.kind,
            range: placed.range,
            quote,
            new: placed.new.clone(),
            context: placed.context,
            offered: placed.new,
            alternatives: wanted.alternatives.clone(),
            shown: 0,
            reason: wanted.reason.trim().to_owned(),
            category,
            confidence: wanted
                .confidence
                .as_deref()
                .and_then(Level::named)
                .unwrap_or_default(),
            importance: wanted
                .importance
                .as_deref()
                .and_then(Level::named)
                .unwrap_or_default(),
            by: agent.name.clone(),
            session: agent.session.clone(),
            made: now(),
            state: State::Open,
            thread: Vec::new(),
            proposal,
            request,
            note: None,
            flagged,
            stale: false,
            waiting: false,
            unseen: true,
            section,
        });
        let at = self.review.all.len() - 1;
        self.review_said(at, Who::Agent(agent.name.clone()), Said::Made);
        if !wanted.alternatives.is_empty() {
            let said = Said::Alternatives(wanted.alternatives.clone());
            self.review_said(at, Who::Agent(agent.name.clone()), said);
        }
        id
    }

    /// Suggestions an agent makes, all checked before any is made: each
    /// answered. With `all_or_nothing`, none is made where one cannot be.
    #[allow(
        clippy::too_many_lines,
        reason = "one batch, checked and made in order"
    )]
    pub fn agent_suggest(
        &mut self,
        batch: &[Wanted],
        request: Option<u64>,
        request_done: bool,
        expected_revision: Option<u64>,
        all_or_nothing: bool,
        agent: &super::Agent,
    ) -> Result<Value, String> {
        self.expect(expected_revision)?;
        if self.editor.is_none() {
            return Err(NOT_EDITING.to_owned());
        }
        self.review_ready();
        if let Some(request) = request
            && !self.review.requests.iter().any(|asked| asked.id == request)
        {
            return Err(format!(
                "there is no review request {request}; list_suggestions gives them"
            ));
        }
        if batch.is_empty() {
            return Err("there is nothing to suggest: give suggestions".to_owned());
        }
        let file = self.file_id();
        let mut placed: Vec<Checked> = Vec::new();
        for wanted in batch {
            let result = self.place_wanted(wanted).map_err(|refused| (refused, None));
            let result = result.and_then(|at| {
                let text = self.text().unwrap_or_default();
                let old = &text[at.range.clone()];
                let against = self.contradicts(
                    file,
                    &at.range,
                    at.kind,
                    wanted.category.as_deref(),
                    old,
                    &at.new,
                );
                match against.first() {
                    Some(first) if !wanted.insist => {
                        let words = self
                            .review
                            .brief
                            .iter()
                            .find(|instruction| instruction.id == *first)
                            .map(|instruction| instruction.text.clone())
                            .unwrap_or_default();
                        Err((
                            (
                                "contradicts",
                                format!(
                                    "it goes against instruction {first} of the brief: “{words}”. \
                                     Leave it, or set insist with a reason the author will read"
                                ),
                            ),
                            Some(*first),
                        ))
                    }
                    _ => Ok((at, against)),
                }
            });
            placed.push(result);
        }
        // Changes of the batch on the same text: the later one is not
        // made, whatever it is.
        for index in 0..placed.len() {
            let Ok((this, _)) = &placed[index] else {
                continue;
            };
            if !this.kind.changes() {
                continue;
            }
            let range = this.range.clone();
            let overlapped = placed[..index].iter().position(|earlier| {
                earlier.as_ref().is_ok_and(|(earlier, _)| {
                    earlier.kind.changes()
                        && (earlier.range.start < range.end.max(range.start + 1)
                            && range.start < earlier.range.end.max(earlier.range.start + 1))
                })
            });
            if let Some(earlier) = overlapped {
                placed[index] = Err((
                    (
                        "overlaps",
                        format!(
                            "it changes the same text as item {earlier} of this batch: suggest one \
                             change there, with alternatives if there are several wordings"
                        ),
                    ),
                    None,
                ));
            }
        }
        let failed = placed.iter().filter(|result| result.is_err()).count();
        let make = !(all_or_nothing && failed > 0);
        let mut results = Vec::new();
        let mut count = 0;
        for (index, (result, wanted)) in placed.into_iter().zip(batch).enumerate() {
            match result {
                Ok((at, flagged)) if make => {
                    let line = line_column(self.text().unwrap_or_default(), at.range.start).0;
                    let id = self.make_suggestion(at, wanted, agent, request, flagged.clone());
                    count += 1;
                    let mut answer = json!({
                        "index": index, "status": "anchored", "id": id, "line": line,
                    });
                    if let (Some(fields), false) = (answer.as_object_mut(), flagged.is_empty()) {
                        fields.insert("against_instructions".to_owned(), json!(flagged));
                    }
                    results.push(answer);
                }
                Ok(_) => results.push(json!({
                    "index": index, "status": "not_made",
                    "message": "another item of the batch could not be made, and all_or_nothing is set",
                })),
                Err(((status, message), instruction)) => {
                    let mut answer = json!({ "index": index, "status": status, "message": message });
                    if let (Some(fields), Some(instruction)) = (answer.as_object_mut(), instruction) {
                        fields.insert("instruction".to_owned(), json!(instruction));
                    }
                    results.push(answer);
                }
            }
        }
        if let (Some(request), true) = (request, request_done)
            && let Some(asked) = self
                .review
                .requests
                .iter_mut()
                .find(|asked| asked.id == request)
        {
            asked.done = true;
        }
        if count > 0 {
            self.review_changed();
            let words = if count == 1 { "a change" } else { "changes" };
            self.notice(format!(
                "{} suggests {}{words}: ]p goes to them, gR lists them",
                agent.name,
                if count == 1 {
                    String::new()
                } else {
                    format!("{count} ")
                },
            ));
        }
        Ok(json!({
            "revision": self.revision,
            "made": count,
            "refused": failed,
            "results": results,
            "note": "shown on the page; the author accepts, rewrites, answers or rejects each. \
                     wait_for_review returns when the author did something",
        }))
    }

    /// The suggestions for an agent: those in `states` (all where none
    /// are named), by `by` only if given, changed since step `since`.
    pub fn suggestions_listed(
        &mut self,
        states: &[String],
        by: Option<&str>,
        since: Option<u64>,
    ) -> Result<Value, String> {
        if self.editor.is_none() {
            return Err(NOT_EDITING.to_owned());
        }
        self.review_ready();
        for state in states {
            let known = State::named(state).is_some()
                || ["stale", "waiting", "answered", "undecided", "decided"]
                    .contains(&state.as_str());
            if !known {
                return Err(format!(
                    "{state:?} is no state; give open, later, waiting, answered, stale, accepted, \
                     rewritten, rejected, withdrawn, undecided or decided"
                ));
            }
        }
        let keeps = |suggestion: &Suggestion| {
            let state_ok = states.is_empty()
                || states.iter().any(|state| match state.as_str() {
                    "undecided" => !suggestion.state.decided(),
                    "decided" => suggestion.state.decided(),
                    "stale" => suggestion.stale && !suggestion.state.decided(),
                    "waiting" | "answered" => suggestion.standing() == state,
                    name => suggestion.state.name() == name,
                });
            let by_ok = by.is_none_or(|by| suggestion.by == by);
            let since_ok =
                since.is_none_or(|since| suggestion.thread.iter().any(|entry| entry.seq > since));
            state_ok && by_ok && since_ok
        };
        let listed: Vec<Value> = self
            .review
            .all
            .iter()
            .filter(|suggestion| keeps(suggestion))
            .map(|suggestion| self.suggestion_json(suggestion))
            .collect();
        Ok(json!({
            "revision": self.revision,
            "cursor": self.review.seq,
            "suggestions": listed,
            "requests": self.requests_json(false),
            "brief": self.brief_json(),
        }))
    }

    /// A suggestion as an agent is given it.
    pub(super) fn suggestion_json(&self, suggestion: &Suggestion) -> Value {
        let text = self.text_of_file(suggestion.file).unwrap_or_default();
        let (line, column) = line_column(text, suggestion.range.start);
        let outcome = suggestion.thread.iter().rev().find(|entry| {
            matches!(
                entry.said,
                Said::Accepted { .. }
                    | Said::Rewritten { .. }
                    | Said::Rejected(_)
                    | Said::Withdrawn
            )
        });
        json!({
            "id": suggestion.id,
            "kind": suggestion.kind.name(),
            "state": suggestion.standing(),
            "decided": suggestion.state.decided(),
            "file": self.file_name_of_id(suggestion.file),
            "line": line,
            "column": column,
            "section": suggestion.section,
            "old_str": quoted(&suggestion.quote),
            "new_str": suggestion.offered,
            "alternatives": suggestion.alternatives,
            "reason": suggestion.reason,
            "category": suggestion.category,
            "confidence": suggestion.confidence.sure(),
            "importance": suggestion.importance.weight(),
            "by": suggestion.by,
            "session": suggestion.session,
            "made": suggestion.made,
            "stale": suggestion.stale,
            "waiting_for": if suggestion.state.decided() {
                Value::Null
            } else if suggestion.waiting {
                json!("agent")
            } else {
                json!("author")
            },
            "request": suggestion.request,
            "note": suggestion.note,
            "against_instructions": suggestion.flagged,
            "outcome": outcome.map(entry_json),
            "thread": suggestion.thread.iter().map(entry_json).collect::<Vec<Value>>(),
        })
    }

    /// The text of `file`, if it is open.
    fn text_of_file(&self, file: FileId) -> Option<&str> {
        let mut editors = self.editor.iter().chain(&self.others);
        editors
            .find(|e| e.file() == file)
            .map(crate::editor::Editor::text)
    }

    /// The name of `file`, from the document's directory.
    pub(in crate::viewer) fn file_name_of_id(&self, file: FileId) -> String {
        let editor = self
            .editor
            .iter()
            .chain(&self.others)
            .find(|e| e.file() == file);
        editor.map_or_else(
            || file.vpath().get_without_slash().to_owned(),
            |editor| {
                let path = editor.path();
                let shown = path.strip_prefix(self.project.root_dir()).unwrap_or(path);
                shown.to_string_lossy().into_owned()
            },
        )
    }

    /// The reviews asked for, for an agent: those not done, or all.
    fn requests_json(&self, all: bool) -> Vec<Value> {
        self.review
            .requests
            .iter()
            .filter(|request| all || !request.done)
            .map(|request| self.request_json(request))
            .collect()
    }

    fn request_json(&self, request: &Request) -> Value {
        let text = self.text_of_file(request.file).unwrap_or_default();
        let (from, _) = line_column(text, request.range.start);
        let (to, _) = line_column(
            text,
            request.range.end.saturating_sub(1).max(request.range.start),
        );
        let made: Vec<u64> = self
            .review
            .all
            .iter()
            .filter(|suggestion| suggestion.request == Some(request.id))
            .map(|suggestion| suggestion.id)
            .collect();
        json!({
            "id": request.id,
            "review": request.what,
            "words": request.words,
            "file": self.file_name_of_id(request.file),
            "from_line": from,
            "to_line": to,
            "section": request.section,
            "text": quoted(&request.quote),
            "done": request.done,
            "suggestions": made,
        })
    }

    /// The brief, for an agent.
    pub(super) fn brief_json(&self) -> Vec<Value> {
        self.review
            .brief
            .iter()
            .map(|instruction| {
                let mut value = json!({
                    "id": instruction.id,
                    "text": instruction.text,
                    "for": if instruction.everywhere { "all documents" } else { "this document" },
                });
                if let Some(fields) = value.as_object_mut() {
                    if let Some(category) = &instruction.category {
                        fields.insert("no_suggestions_of_category".to_owned(), json!(category));
                    }
                    if let Some((_, quote)) = &instruction.protect {
                        fields.insert("do_not_change".to_owned(), json!(quoted(quote)));
                    }
                    if let Some((old, new)) = &instruction.refused {
                        fields.insert(
                            "refused_change".to_owned(),
                            json!({ "old_str": old, "new_str": new }),
                        );
                    }
                }
                value
            })
            .collect()
    }

    /// The brief as words, for the instructions an agent is given when
    /// it connects: one instruction a line.
    pub fn brief_text(&mut self) -> String {
        self.review_ready();
        self.review
            .brief
            .iter()
            .map(|instruction| {
                let scope = if instruction.everywhere {
                    ""
                } else {
                    " (this document)"
                };
                format!("{}. {}{scope}", instruction.id, instruction.text)
            })
            .collect::<Vec<String>>()
            .join("\n")
    }

    /// The brief for an agent.
    pub fn brief_listed(&mut self) -> Value {
        self.review_ready();
        json!({
            "instructions": self.brief_json(),
            "text": self.brief_text(),
            "note": "standing instructions of the author: follow them. A suggestion that goes \
                     against one is refused unless you insist (and say why)",
        })
    }

    /// A new version of suggestion `id`, or other wordings for it, or a
    /// line to the author about it, from the agent that made it.
    #[allow(
        clippy::too_many_arguments,
        reason = "the parts of a revision, all optional"
    )]
    pub fn agent_revise(
        &mut self,
        id: u64,
        new_str: Option<&str>,
        reason: Option<&str>,
        alternatives: Option<Vec<String>>,
        message: Option<&str>,
        expected_revision: Option<u64>,
        agent: &super::Agent,
    ) -> Result<Value, String> {
        self.expect(expected_revision)?;
        self.review_ready();
        let at = self
            .suggestion_index(id)
            .ok_or_else(|| format!("there is no suggestion {id}; list_suggestions gives them"))?;
        let suggestion = &self.review.all[at];
        if suggestion.state.decided() {
            return Err(format!(
                "suggestion {id} is {} already: make a new suggestion instead",
                suggestion.state.name()
            ));
        }
        if new_str.is_none() && alternatives.is_none() && message.is_none() && reason.is_none() {
            return Err("give new_str, alternatives, reason or message".to_owned());
        }
        if new_str.is_some() && !suggestion.kind.changes() {
            return Err("a comment or question changes no text: give a message".to_owned());
        }
        if let Some(new) = new_str {
            let against = self.contradicts(
                suggestion.file,
                &suggestion.range,
                suggestion.kind,
                Some(&suggestion.category),
                &suggestion.quote,
                new,
            );
            if let Some(first) = against.first() {
                return Err(format!(
                    "the new version goes against instruction {first} of the brief (get_brief)"
                ));
            }
        }
        let by = Who::Agent(agent.name.clone());
        let proposal = suggestion.proposal;
        if let Some(new) = new_str {
            let reason = reason.unwrap_or_default().to_owned();
            let suggestion = &mut self.review.all[at];
            new.clone_into(&mut suggestion.new);
            new.clone_into(&mut suggestion.offered);
            suggestion.shown = 0;
            if !reason.is_empty() {
                reason.clone_into(&mut suggestion.reason);
            }
            if let Some(waiting) = self.proposals.iter_mut().find(|p| Some(p.id) == proposal) {
                new.clone_into(&mut waiting.new);
            }
            self.review_said(
                at,
                by.clone(),
                Said::Version {
                    text: new.to_owned(),
                    reason,
                },
            );
        } else if let Some(reason) = reason {
            reason.trim().clone_into(&mut self.review.all[at].reason);
        }
        if let Some(alternatives) = alternatives {
            let suggestion = &mut self.review.all[at];
            suggestion.alternatives.clone_from(&alternatives);
            suggestion.shown = 0;
            let offered = suggestion.offered.clone();
            suggestion.new.clone_from(&offered);
            if let Some(waiting) = self.proposals.iter_mut().find(|p| Some(p.id) == proposal) {
                waiting.new = offered;
            }
            self.review_said(at, by.clone(), Said::Alternatives(alternatives));
        }
        if let Some(message) = message.map(str::trim).filter(|m| !m.is_empty()) {
            self.review_said(at, by, Said::Message(message.to_owned()));
        }
        let suggestion = &mut self.review.all[at];
        suggestion.waiting = false;
        suggestion.unseen = true;
        self.review_changed();
        let line = self.text().map_or(0, |text| {
            line_column(text, self.review.all[at].range.start).0
        });
        self.notice(format!(
            "{} answered on the suggestion on line {line}: ]p goes to it",
            agent.name
        ));
        Ok(json!({ "revised": id, "state": self.review.all[at].standing() }))
    }

    /// Takes back the suggestions `ids` for the agent that made them: all
    /// of them, or none where one cannot be.
    pub fn agent_withdraw(&mut self, ids: &[u64], agent: &super::Agent) -> Result<Value, String> {
        self.review_ready();
        for id in ids {
            let at = self
                .suggestion_index(*id)
                .ok_or_else(|| format!("there is no suggestion {id}: none was taken back"))?;
            if self.review.all[at].state.decided() {
                return Err(format!(
                    "suggestion {id} is {} already: none was taken back",
                    self.review.all[at].state.name()
                ));
            }
        }
        for id in ids {
            let Some(at) = self.suggestion_index(*id) else {
                continue;
            };
            let suggestion = &mut self.review.all[at];
            suggestion.state = State::Withdrawn;
            let proposal = suggestion.proposal.take();
            self.proposals
                .retain(|waiting| Some(waiting.id) != proposal);
            self.proposals_elsewhere
                .retain(|waiting| Some(waiting.id) != proposal);
            self.review_said(at, Who::Agent(agent.name.clone()), Said::Withdrawn);
        }
        self.sync_proposals();
        self.review_changed();
        Ok(json!({ "withdrawn": ids }))
    }

    /// What the author did since step `since`: what agents that wait are
    /// told.
    pub fn review_news(&mut self, since: u64) -> Value {
        self.review_ready();
        let mut events = Vec::new();
        for suggestion in &self.review.all {
            for entry in suggestion.thread.iter().filter(|entry| entry.seq > since) {
                if entry.by == Who::Author {
                    let mut event = entry_json(entry);
                    if let Some(fields) = event.as_object_mut() {
                        fields.insert("suggestion".to_owned(), json!(suggestion.id));
                        fields.insert("state".to_owned(), json!(suggestion.standing()));
                    }
                    events.push(event);
                }
            }
        }
        let requests: Vec<Value> = self
            .review
            .requests
            .iter()
            .filter(|request| request.seq > since)
            .map(|request| self.request_json(request))
            .collect();
        let brief_changed = self
            .review
            .brief
            .iter()
            .any(|instruction| instruction.seq > since);
        events.sort_by_key(|event| event["seq"].as_u64().unwrap_or(0));
        json!({
            "cursor": self.review.seq,
            "events": events,
            "requests": requests,
            "brief_changed": brief_changed,
            "brief": if brief_changed { json!(self.brief_json()) } else { Value::Null },
        })
    }

    /// What `get_state` says of reviewing.
    pub fn review_summary(&mut self) -> Value {
        self.review_ready();
        let open = |suggestion: &&Suggestion| !suggestion.state.decided();
        let undecided: Vec<&Suggestion> = self.review.all.iter().filter(open).collect();
        let waiting_for_agents = undecided.iter().filter(|s| s.waiting).count();
        json!({
            "open_suggestions": undecided.len(),
            "waiting_for_agents": waiting_for_agents,
            "review_requests": self.review.requests.iter().filter(|r| !r.done).count(),
            "brief": self.brief_json().len(),
            "brief_text": self.brief_text(),
            "cursor": self.review.seq,
            "hint": "suggest changes with suggest (not edit_source) unless the author asked for \
                     edits; list_suggestions shows what became of them; wait_for_review waits \
                     for the author",
        })
    }

    /// Makes the proposals made by other ways than `suggest` (from
    /// `first` on, or all not yet taken in) suggestions of `agent`, of
    /// `kind` where it is known. Returns the numbers they got, by
    /// proposal.
    pub fn adopt_proposals(
        &mut self,
        agent: Option<&super::Agent>,
        kind: Option<Kind>,
        category: Option<&str>,
    ) -> Vec<(u64, u64)> {
        let from = self.review.adopted.min(self.next_proposal);
        self.review.adopted = self.next_proposal;
        let unknown = super::Agent::unknown();
        let agent = agent.unwrap_or(&unknown);
        let fresh: Vec<Proposal> = self
            .proposals
            .iter()
            .chain(&self.proposals_elsewhere)
            .filter(|proposal| proposal.id >= from)
            .filter(|proposal| {
                !self
                    .review
                    .all
                    .iter()
                    .any(|suggestion| suggestion.proposal == Some(proposal.id))
            })
            .cloned()
            .collect();
        let mut made = Vec::new();
        for proposal in fresh {
            let (reason, note) = self.proposal_about(proposal.id);
            let (reason, note) = (reason.unwrap_or_default().to_owned(), note);
            let kind = kind.unwrap_or(if proposal.old.is_empty() {
                Kind::Insert
            } else if proposal.new.is_empty() {
                Kind::Delete
            } else {
                Kind::Replace
            });
            let category = category.map_or_else(
                || if note.is_some() { "answer" } else { "edit" }.to_owned(),
                str::to_owned,
            );
            let flagged = self.contradicts(
                proposal.file,
                &proposal.range,
                kind,
                Some(&category),
                &proposal.old,
                &proposal.new,
            );
            let section = if proposal.file == self.file_id() {
                self.section_title_at(proposal.range.start)
            } else {
                String::new()
            };
            let context = if kind == Kind::Insert {
                let text = self.text_of_file(proposal.file).unwrap_or_default();
                let mut from = proposal.range.start.saturating_sub(CONTEXT);
                while !text.is_char_boundary(from) {
                    from -= 1;
                }
                text.get(from..proposal.range.start)
                    .unwrap_or_default()
                    .to_owned()
            } else {
                String::new()
            };
            self.review.last += 1;
            let id = self.review.last;
            self.review.all.push(Suggestion {
                id,
                file: proposal.file,
                kind,
                range: proposal.range.clone(),
                quote: proposal.old.clone(),
                new: proposal.new.clone(),
                context,
                offered: proposal.new.clone(),
                alternatives: Vec::new(),
                shown: 0,
                reason,
                category,
                confidence: Level::Medium,
                importance: Level::Medium,
                by: agent.name.clone(),
                session: agent.session.clone(),
                made: now(),
                state: State::Open,
                thread: Vec::new(),
                proposal: Some(proposal.id),
                request: None,
                note,
                flagged,
                stale: proposal.stale,
                waiting: false,
                unseen: true,
                section,
            });
            let at = self.review.all.len() - 1;
            self.review_said(at, Who::Agent(agent.name.clone()), Said::Made);
            made.push((proposal.id, id));
        }
        if !made.is_empty() {
            self.review_changed();
        }
        made
    }

    /// Makes a change as an agent asked with the older tools (see
    /// [`Viewer::agent_replace`]) and, where it was proposed, the
    /// suggestions it waits as; returns those with the outcome.
    pub fn adopt_outcome(
        &mut self,
        outcome: &Outcome,
        agent: &super::Agent,
        kind: Option<Kind>,
    ) -> Vec<u64> {
        if !matches!(outcome, Outcome::Proposed { .. }) {
            return Vec::new();
        }
        self.adopt_proposals(Some(agent), kind, None)
            .into_iter()
            .map(|(_, id)| id)
            .collect()
    }

    /// The suggestion that proposal `id` waits for, if it is one.
    #[must_use]
    pub fn suggestion_of_proposal(&self, id: u64) -> Option<&Suggestion> {
        self.review
            .all
            .iter()
            .find(|suggestion| suggestion.proposal == Some(id))
    }
}

/// The category a suggestion is of where its agent named none.
fn default_category(kind: Kind) -> &'static str {
    match kind {
        Kind::Comment => "comment",
        Kind::Question => "question",
        Kind::Move => "structure",
        _ => "edit",
    }
}

const NOT_EDITING: &str = super::super::agent::NOT_EDITING;
