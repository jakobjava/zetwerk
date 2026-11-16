//! Reviewing with the author: the tools by which an agent suggests
//! instead of editing, learns what became of its suggestions, waits for
//! the author's answers, and reads the author's standing instructions;
//! the prompts that start a review from the client; and the resources
//! an agent can read and be told of changes to.
//!
//! The working agreement every prompt and the server's instructions
//! state: read the brief first, read the text, suggest (do not edit),
//! one suggestion per thing, say why in one sentence, use the
//! categories, come back for the answers.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ErrorData, GetPromptResult, ListPromptsResult, ListResourcesResult, Prompt,
    PromptArgument, PromptMessage, ReadResourceResult, Resource, ResourceContents, Role,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, tool, tool_router};

use crate::server::{Server, blocking, failure};

/// Who calls a tool: the name the agent's client gave as it connected,
/// and the session (one connection).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Caller {
    pub name: String,
    pub session: String,
}

thread_local! {
    /// The caller of the tool being answered on this thread.
    static CALLER: RefCell<Option<Caller>> = const { RefCell::new(None) };
}

/// Who calls the tool being answered on this thread: for the older tools
/// (`edit_source` with `mode: propose`), whose suggestions are this
/// caller's.
#[must_use]
pub fn caller() -> Option<Caller> {
    CALLER.with(|caller| caller.borrow().clone())
}

/// Runs `call` with `caller` as the caller of this thread.
pub(crate) fn with_caller<T>(caller: Caller, call: impl FnOnce() -> T) -> T {
    CALLER.with(|kept| *kept.borrow_mut() = Some(caller));
    let answer = call();
    CALLER.with(|kept| *kept.borrow_mut() = None);
    answer
}

/// One connection of an agent.
#[derive(Debug)]
pub(crate) struct Connection {
    session: String,
    name: OnceLock<String>,
}

impl Connection {
    pub(crate) fn new() -> Arc<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let since = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let count = NEXT.fetch_add(1, Ordering::Relaxed);
        Arc::new(Self {
            session: format!("{since:x}-{count}"),
            name: OnceLock::new(),
        })
    }

    /// Notes the name the client gave.
    pub(crate) fn named(&self, name: &str) {
        let name = name.trim();
        let _ = self.name.set(if name.is_empty() {
            "agent".to_owned()
        } else {
            name.to_owned()
        });
    }

    pub(crate) fn caller(&self) -> Caller {
        Caller {
            name: self
                .name
                .get()
                .cloned()
                .unwrap_or_else(|| "agent".to_owned()),
            session: self.session.clone(),
        }
    }
}

/// A suggestion as an agent makes it.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize, schemars::JsonSchema)]
pub struct NewSuggestion {
    /// `replace` (`old_str` by `new_str`), `insert` (`new_str` put in after
    /// `old_str`, or before it with position `before`), `delete` (`old_str`
    /// taken out), `move` (`old_str` taken to `move_to`), `comment` (a remark
    /// on `old_str`, the paragraph on `line`, or the `section`) or
    /// `question` (a question to the author about it).
    pub kind: String,
    /// The text it is about, exactly as in the source (no line numbers),
    /// enough of it to be there once (within `section` where it is
    /// given). Not needed for a comment or question on a section or a
    /// line.
    pub old_str: Option<String>,
    /// The heading of the section (as `get_outline` gives it) to look for
    /// `old_str` in, or that a comment is about.
    pub section: Option<String>,
    /// A line of the source: the paragraph on it is what it is about, or
    /// where `old_str` is looked for.
    pub line: Option<usize>,
    /// What to put in place of `old_str` (`replace`), or to put in
    /// (`insert`).
    pub new_str: Option<String>,
    /// For `insert` and `move`: `after` (the default) or `before` the
    /// text it goes by.
    pub position: Option<String>,
    /// For `move`: the text, there once in the source, to put it by.
    pub move_to: Option<String>,
    /// Why, in one sentence the author understands. For a comment or a
    /// question: the comment or the question itself.
    pub reason: String,
    /// One word: spelling, grammar, clarity, concision, tone, structure,
    /// facts, citation, consistency, formatting, maths; or your own.
    pub category: Option<String>,
    /// How sure you are: `low`, `medium` (the default) or `high`. The
    /// author may accept all `high` ones at once.
    pub confidence: Option<String>,
    /// How much it matters: `minor`, `normal` (the default) or `major`.
    pub importance: Option<String>,
    /// Other wordings than `new_str`, best first: the author steps through
    /// them on the page and takes one.
    #[serde(default)]
    pub alternatives: Vec<String>,
    /// Make it even where it goes against an instruction of the brief
    /// (say why in reason): it is shown flagged. Default false.
    #[serde(default)]
    pub insist: bool,
}

/// A batch of suggestions.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Suggest {
    pub suggestions: Vec<NewSuggestion>,
    /// The review request they answer.
    pub request: Option<u64>,
    /// Whether that request is done with them.
    pub request_done: bool,
    pub expected_revision: Option<u64>,
    /// None made where one cannot be.
    pub all_or_nothing: bool,
}

/// A new version of a suggestion, other wordings, or a line about it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Revision {
    pub id: u64,
    pub new_str: Option<String>,
    pub reason: Option<String>,
    pub alternatives: Option<Vec<String>>,
    pub message: Option<String>,
    pub expected_revision: Option<u64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SuggestArgs {
    /// The suggestions, one for each thing: each is placed and answered
    /// in `results` (`anchored` with its `id`, `not_found`, `ambiguous`,
    /// `overlaps` another of the batch, `contradicts` an instruction of
    /// the brief, `invalid`).
    suggestions: Vec<NewSuggestion>,
    /// The id of the review request (see `list_suggestions`) they
    /// answer.
    request: Option<u64>,
    /// Whether that review request is done with these (default false).
    #[serde(default)]
    request_done: bool,
    /// The `revision` you read the source at: the batch is refused if the
    /// text has changed since.
    expected_revision: Option<u64>,
    /// Make none where one cannot be made (default false: the others are
    /// made).
    #[serde(default)]
    all_or_nothing: bool,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ListSuggestionsArgs {
    /// Only those in these states: `open`, `later` (put aside), `waiting`
    /// (for you: the author answered or asked for other wordings),
    /// `answered` (by an agent, not yet seen), `stale` (their text
    /// changed), `accepted`, `rewritten`, `rejected`, `withdrawn`,
    /// `undecided`, `decided`. Default: all.
    states: Option<Vec<String>>,
    /// Only yours (default false).
    #[serde(default)]
    mine_only: bool,
    /// Only those with something new since this `cursor` (from an
    /// earlier answer).
    since: Option<u64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ReviseArgs {
    /// The suggestion's `id`.
    id: u64,
    /// A new version of its change: the same thread, the author sees it
    /// in place of the old one.
    new_str: Option<String>,
    /// Why, anew.
    reason: Option<String>,
    /// Other wordings, best first (when the author asked for them).
    alternatives: Option<Vec<String>>,
    /// A line to the author in its thread: an answer to what they said.
    message: Option<String>,
    expected_revision: Option<u64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct WithdrawArgs {
    /// The suggestion's `id`.
    id: Option<u64>,
    /// Several at once: all are taken back, or none where one cannot be.
    ids: Option<Vec<u64>>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct WaitArgs {
    /// The `cursor` of an earlier answer: what happened since then is
    /// told at once. By default, what happens from now on.
    since: Option<u64>,
    /// How long to wait at most, in seconds (default 120, at most 600).
    timeout_seconds: Option<u64>,
}

/// The working agreement, as every prompt and the server's instructions
/// give it.
pub(crate) const AGREEMENT: &str = "Working agreement for reviewing in zetwerk: \
1. Read the author's brief first (get_brief) and follow it. \
2. Read the text (read_source, get_outline; render_page to see it). \
3. Suggest, do not edit: use suggest, never edit_source, unless the author asked you to make \
the change itself. The author decides on each suggestion in the window. \
4. One suggestion per thing; for several wordings of one thing, give alternatives. \
5. Say why in one sentence the author understands, and give a category (spelling, grammar, \
clarity, concision, tone, structure, facts, citation, consistency, formatting, maths), how sure \
you are and how much it matters. Leave alone what is fine. \
6. Come back for the answers: wait_for_review (or list_suggestions) tells you what the author \
accepted, rewrote, rejected and why, or asked of you; answer with revise_suggestion, and make \
nothing the author rejected again.";

/// An argument a prompt takes: its name and what it is.
type Argument = Option<(&'static str, &'static str)>;

/// The prompts: their names, what they are, and their argument if any.
const PROMPTS: &[(&str, &str, Argument)] = &[
    (
        "proofread",
        "Proofread the document: spelling, grammar, punctuation, typos",
        None,
    ),
    (
        "review_for_clarity",
        "Review the document for clarity: what is hard to follow, and plainer ways to say it",
        None,
    ),
    (
        "check_maths",
        "Check that the maths is consistent: notation, definitions, equations, numbers",
        None,
    ),
    (
        "review_section",
        "Review one section of the document",
        Some((
            "section",
            "The heading of the section, as the outline gives it",
        )),
    ),
    (
        "answer_questions",
        "Answer the author's open questions and review requests",
        None,
    ),
];

/// What prompt `name` asks, with `argument` where it takes one.
fn prompt_text(name: &str, argument: Option<&str>) -> Option<String> {
    let task = match name {
        "proofread" => {
            "Proofread the document open in zetwerk: spelling, grammar, punctuation and \
                        typos only (categories spelling, grammar, formatting). Do not rephrase \
                        what is correct. Suggest each fix with confidence high where it is \
                        plainly wrong."
                .to_owned()
        }
        "review_for_clarity" => "Review the document open in zetwerk for clarity: sentences that \
                                 are hard to follow, unclear references, jargon left unexplained, \
                                 paragraphs that wander. Suggest plainer wordings (category \
                                 clarity or concision), with alternatives where there are \
                                 several good ones; ask a question where you cannot tell what is \
                                 meant."
            .to_owned(),
        "check_maths" => "Check the maths of the document open in zetwerk: that symbols are \
                          defined before use and used for one thing, that equations agree with \
                          each other and with the text, that numbers add up. Suggest fixes \
                          (category maths), or ask a question where you are not sure. Do not \
                          change an equation the brief protects."
            .to_owned(),
        "review_section" => format!(
            "Review the section {:?} of the document open in zetwerk, as a careful editor \
             would: clarity, structure, tone, facts and citations. Pass section to suggest to \
             keep to it. Suggest nothing outside it.",
            argument.unwrap_or("the author names")
        ),
        "answer_questions" => "Answer the author: list_notes gives the questions they asked you \
                               (answer each with answer_note), list_suggestions the review \
                               requests waiting (requests) and the suggestions whose \
                               waiting_for is agent (answer with revise_suggestion: a message, \
                               a new version, or alternatives). Then wait_for_review for more."
            .to_owned(),
        _ => return None,
    };
    Some(format!("{task}\n\n{AGREEMENT}"))
}

/// The prompts there are.
pub(crate) fn prompts() -> ListPromptsResult {
    let prompts = PROMPTS
        .iter()
        .map(|(name, about, argument)| {
            let arguments = argument.map(|(name, about)| {
                vec![
                    PromptArgument::new(name)
                        .with_description(about)
                        .with_required(true),
                ]
            });
            Prompt::new(*name, Some(*about), arguments)
        })
        .collect();
    ListPromptsResult::with_all_items(prompts)
}

/// Prompt `name`, with what it was given.
pub(crate) fn prompt(
    name: &str,
    arguments: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Result<GetPromptResult, ErrorData> {
    let argument = arguments
        .and_then(|given| given.get("section"))
        .and_then(serde_json::Value::as_str);
    if name == "review_section" && argument.is_none_or(|section| section.trim().is_empty()) {
        return Err(ErrorData::invalid_params(
            "review_section needs the section's heading",
            None,
        ));
    }
    let text = prompt_text(name, argument)
        .ok_or_else(|| ErrorData::invalid_params(format!("there is no prompt {name}"), None))?;
    let about = PROMPTS
        .iter()
        .find(|(known, ..)| *known == name)
        .map_or("", |(_, about, _)| *about);
    Ok(
        GetPromptResult::new(vec![PromptMessage::new_text(Role::User, text)])
            .with_description(about),
    )
}

/// The resources an agent can read: the brief, the suggestions, the
/// review requests.
pub(crate) const BRIEF: &str = "zetwerk://review/brief";
pub(crate) const SUGGESTIONS: &str = "zetwerk://review/suggestions";

pub(crate) fn resources() -> ListResourcesResult {
    ListResourcesResult::with_all_items(vec![
        Resource::new(BRIEF, "brief")
            .with_description(
                "The author's standing instructions to agents: read before suggesting",
            )
            .with_mime_type("application/json"),
        Resource::new(SUGGESTIONS, "suggestions")
            .with_description(
                "All suggestions with their state and thread, the review requests and the \
                 brief. Subscribe to be told when the author answers.",
            )
            .with_mime_type("application/json"),
    ])
}

impl Server {
    /// Reads resource `uri`.
    pub(crate) async fn read_review_resource(
        &self,
        uri: &str,
    ) -> Result<ReadResourceResult, ErrorData> {
        let editor = Arc::clone(&self.editor);
        let value = match uri {
            BRIEF => blocking(move || editor.brief()).await?,
            SUGGESTIONS => blocking(move || editor.suggestions(&[], None, None)).await?,
            _ => {
                return Err(ErrorData::resource_not_found(
                    format!("there is no resource {uri}"),
                    None,
                ));
            }
        };
        let value = value.map_err(|why| ErrorData::internal_error(why, None))?;
        let text = serde_json::to_string_pretty(&value)
            .map_err(|err| ErrorData::internal_error(err.to_string(), None))?;
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(text, uri).with_mime_type("application/json"),
        ]))
    }

    /// Tells the client that subscribed to `uri` of every step of the
    /// author, until it goes.
    pub(crate) fn watch_review(&self, uri: String, context: &RequestContext<RoleServer>) {
        let editor = Arc::clone(&self.editor);
        let peer = context.peer.clone();
        let stop = Arc::new(AtomicBool::new(false));
        tokio::spawn(async move {
            let mut since: Option<u64> = None;
            while !peer.is_transport_closed() {
                let (waiter, flag) = (Arc::clone(&editor), Arc::clone(&stop));
                let answered = tokio::task::spawn_blocking(move || {
                    waiter.wait_for_review(since, Duration::from_secs(60), &flag)
                })
                .await;
                let Ok(Ok(news)) = answered else {
                    break;
                };
                since = news["cursor"].as_u64().or(since);
                let told = news["events"]
                    .as_array()
                    .is_some_and(|events| !events.is_empty())
                    || news["requests"]
                        .as_array()
                        .is_some_and(|requests| !requests.is_empty())
                    || news["brief_changed"] == serde_json::Value::Bool(true);
                if told {
                    let updated = rmcp::model::ResourceUpdatedNotificationParam::new(uri.clone());
                    if peer.notify_resource_updated(updated).await.is_err() {
                        break;
                    }
                }
            }
        });
    }
}

#[tool_router(router = review_tools, vis = "pub(crate)")]
impl Server {
    /// Suggest changes to the document, or remarks and questions about
    /// it, for the author to decide on in the window: the way to review
    /// (do not `edit_source` what you only suggest). Each suggestion is
    /// placed by its exact text (`old_str`) or by section or line, and
    /// shown on the page where it stands; the author accepts, rewrites,
    /// answers or rejects it. The batch is checked whole first: each
    /// item is answered in `results` with its `status` (`anchored` and
    /// its `id`; `not_found`, `ambiguous`, `overlaps`, `contradicts`
    /// with the `instruction` of the brief it goes against, `invalid`)
    /// and a `message` saying what to do. Read `get_brief` before.
    #[tool(annotations(title = "Suggest", read_only_hint = false))]
    async fn suggest(
        &self,
        Parameters(args): Parameters<SuggestArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = self.caller();
        let suggest = Suggest {
            suggestions: args.suggestions,
            request: args.request,
            request_done: args.request_done,
            expected_revision: args.expected_revision,
            all_or_nothing: args.all_or_nothing,
        };
        self.value(move |editor| editor.suggest(&caller, suggest))
            .await
    }

    /// What became of the suggestions: each with its `state` (open,
    /// later, waiting for you, answered, stale, accepted, rewritten,
    /// rejected, withdrawn), its `thread` (what the author said, the
    /// versions), its `outcome` (the text before and after; for a
    /// rewritten one what the author made of yours; for a rejected one
    /// why), and where it is. Also the review `requests` the author made
    /// and the `brief`, and a `cursor` for `wait_for_review`. Learn from
    /// it: make nothing again that was rejected.
    #[tool(annotations(title = "List suggestions", read_only_hint = true))]
    async fn list_suggestions(
        &self,
        Parameters(args): Parameters<ListSuggestionsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let by = args.mine_only.then(|| self.caller().name);
        let states = args.states.unwrap_or_default();
        self.value(move |editor| editor.suggestions(&states, by.as_deref(), args.since))
            .await
    }

    /// Answer the author on a suggestion of yours that is not decided:
    /// a new version of its change (`new_str`, the same thread), other
    /// wordings (`alternatives`, when the author asked for them), a
    /// `message` (an answer to what they said), or a new `reason`. It
    /// waits for the author again then.
    #[tool(annotations(title = "Revise suggestion", read_only_hint = false))]
    async fn revise_suggestion(
        &self,
        Parameters(args): Parameters<ReviseArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = self.caller();
        let revision = Revision {
            id: args.id,
            new_str: args.new_str,
            reason: args.reason,
            alternatives: args.alternatives,
            message: args.message,
            expected_revision: args.expected_revision,
        };
        self.value(move |editor| editor.revise_suggestion(&caller, revision))
            .await
    }

    /// Take back suggestions you made that the author has not decided
    /// on: give the `id`, or the `ids` of several.
    #[tool(annotations(title = "Withdraw suggestion", read_only_hint = false))]
    async fn withdraw_suggestion(
        &self,
        Parameters(args): Parameters<WithdrawArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let mut ids = args.ids.unwrap_or_default();
        ids.extend(args.id);
        if ids.is_empty() {
            return Ok(failure(
                "give the `id` of a suggestion, or `ids`".to_owned(),
            ));
        }
        let caller = self.caller();
        self.value(move |editor| editor.withdraw_suggestions(&caller, &ids))
            .await
    }

    /// The author's standing instructions to agents ("British spelling
    /// stays", "do not touch the abstract"): for this document and for
    /// all of theirs. Read them before you suggest, and follow them: a
    /// suggestion that goes against one (a text it keeps you off, a
    /// change it refuses, a category) is refused.
    #[tool(annotations(title = "Get brief", read_only_hint = true))]
    async fn get_brief(&self) -> Result<CallToolResult, ErrorData> {
        self.value(crate::Editor::brief).await
    }

    /// Wait for the author: returns as soon as they did something with
    /// suggestions (accepted, rewrote, rejected, answered, asked for
    /// other wordings), asked for a review, or changed the brief, since
    /// `since` (the `cursor` of an earlier answer; by default since now),
    /// or when `timeout_seconds` pass. Answers with the `events`, new
    /// review `requests`, the brief if it changed, and the `cursor` to
    /// wait from next. Use it instead of polling.
    #[tool(annotations(title = "Wait for review", read_only_hint = true))]
    async fn wait_for_review(
        &self,
        Parameters(args): Parameters<WaitArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let timeout = Duration::from_secs(args.timeout_seconds.unwrap_or(120).clamp(1, 600));
        let cancelled = Arc::new(AtomicBool::new(false));
        let (flag, editor) = (Arc::clone(&cancelled), Arc::clone(&self.editor));
        let waiting =
            tokio::task::spawn_blocking(move || editor.wait_for_review(args.since, timeout, &flag));
        let ct = context.ct.clone();
        let stop = Arc::clone(&cancelled);
        let cancel = tokio::spawn(async move {
            ct.cancelled().await;
            stop.store(true, Ordering::Relaxed);
        });
        let answer = waiting
            .await
            .map_err(|err| ErrorData::internal_error(format!("editor call failed: {err}"), None))?;
        cancel.abort();
        Ok(match answer {
            Ok(value) => CallToolResult::structured(value),
            Err(message) => failure(message),
        })
    }
}
