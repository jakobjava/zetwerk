//! What agents are doing: every agent connected over MCP, what it is at
//! this moment, each step it took, and how far it is, in the writer's
//! words; with the means to stop it.
//!
//! Agents came in through MCP and worked unseen: the writer could not
//! tell whether one was reading, writing, stuck or gone. Now each tool
//! call an agent makes is told to the window as it begins and as it ends
//! (see `zetwerk_mcp::Activity`), and agents are asked to say what they
//! are doing (`report_progress`: a few words, and how many of how many
//! are done).
//!
//! - **The status line** says what the one at work is doing: what it
//!   said, or the step it is in ("agent: reading the source"), with its
//!   count; "agent waiting for you" while it waits for an answer;
//!   "agents paused" while they are.
//! - **`:agents`** lists each agent: who it is (its name and version),
//!   whether it is at work, waiting, done or gone, for how long, what it
//!   last said, and its last steps, newest first, each with its time, what
//!   it was about, how long it took and why it failed where it did. The
//!   list follows them as they work. From it the writer stops the agent
//!   the window called, pauses all agents or lets them go on, and sees
//!   what the one called wrote.
//! - **`:agents pause`** refuses every tool that would change anything,
//!   for every agent, until `:agents resume`: they may read, and are told
//!   why they may not write. Nothing they did is undone (that is `u`).
//!
//! Nothing of it is in the text, and nothing is kept past the window.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use super::Viewer;
use super::assist::Target;

/// Whether the writer paused agents: read by the MCP server's thread
/// before each call, without asking the window (one window a process).
pub static AGENTS_PAUSED: AtomicBool = AtomicBool::new(false);

/// How many steps of an agent are kept.
const STEPS_KEPT: usize = 200;
/// How many of them the list shows.
const STEPS_SHOWN: usize = 12;
/// How many agents are kept, the most recent; those gone longest go.
const AGENTS_KEPT: usize = 12;
/// How long what an agent said stands in the status line without a step.
const SAID_STANDS: Duration = Duration::from_secs(90);

/// What an agent did, as the MCP server tells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Connected {
        session: String,
        name: String,
        version: String,
    },
    Began {
        session: String,
        tool: String,
        about: String,
    },
    Ended {
        session: String,
        tool: String,
        millis: u64,
        error: Option<String>,
    },
    Said {
        session: String,
        message: String,
        done: Option<u32>,
        total: Option<u32>,
    },
    Left {
        session: String,
    },
}

/// One step of an agent: a tool it called.
#[derive(Debug, Clone)]
struct Step {
    at: SystemTime,
    tool: String,
    about: String,
    /// How long it took, once it ended, and why it failed if it did.
    took: Option<u64>,
    error: Option<String>,
}

/// What it said it is doing, and when.
#[derive(Debug, Clone)]
struct Said {
    message: String,
    done: Option<u32>,
    total: Option<u32>,
    at: Instant,
}

/// One agent: one connection of an MCP client.
#[derive(Debug, Clone)]
struct Agent {
    session: String,
    name: String,
    version: String,
    since: Instant,
    last: Instant,
    /// The step it is in, if one is under way.
    now: Option<Step>,
    said: Option<Said>,
    steps: VecDeque<Step>,
    calls: usize,
    failures: usize,
    left: Option<Instant>,
    /// Whether the window called it (see `on_call`).
    called: bool,
}

/// An agent as the AI pane shows it.
pub(super) struct Shown {
    pub(super) name: String,
    pub(super) state: String,
    /// What it said it is doing, and how many of how many are done.
    pub(super) said: Option<(String, Option<u32>, Option<u32>)>,
    /// Its last steps, newest first, in words.
    pub(super) steps: Vec<String>,
    pub(super) gone: bool,
}

/// What the viewer keeps of agents' doings.
#[derive(Default)]
pub(super) struct Activity {
    agents: Vec<Agent>,
    /// Whether the writer paused agents, as this window shows it; the
    /// server reads [`AGENTS_PAUSED`], which is set with it.
    paused: bool,
}

/// What a tool is, as the writer would say it of an agent.
fn in_words(tool: &str) -> String {
    let said = match tool {
        "get_state" => "looked at the window",
        "read_source" => "read the source",
        "get_outline" => "read the outline",
        "render_page" => "looked at a page",
        "search" => "searched",
        "locate" => "found a place",
        "get_diagnostics" => "read what Typst refuses",
        "get_authorship" => "read who wrote what",
        "get_brief" => "read your brief",
        "list_notes" => "read the notes",
        "add_note" => "left a note",
        "answer_note" => "answered a note",
        "resolve_note" => "resolved a note",
        "suggest" => "made a suggestion",
        "list_suggestions" => "read the suggestions",
        "revise_suggestion" => "revised a suggestion",
        "withdraw_suggestion" => "withdrew a suggestion",
        "wait_for_review" => "waited for you",
        "list_proposals" => "read the proposals",
        "withdraw_proposal" => "withdrew a proposal",
        "edit_source" => "changed the text",
        "insert_at_cursor" => "typed at the cursor",
        "replace_selection" => "replaced the selection",
        "move_section" => "moved a section",
        "insert_pages" => "put in pages",
        "add_source" => "added a source",
        "list_sources" => "read the sources",
        "list_figures" => "read the figures",
        "list_packages" => "read the packages",
        "list_targets" => "read the labels",
        "check_language" => "checked the language",
        "export_pdf" => "exported a PDF",
        "export_image" => "exported an image",
        "save" => "saved the file",
        "send_keys" => "pressed keys",
        "screenshot" => "took a picture of the window",
        other => other,
    };
    said.to_owned()
}

/// The time of day of `at`, as `14:26:04`, in UTC where the local time
/// is not known.
fn clock(at: SystemTime) -> String {
    let seconds = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let offset = local_offset();
    let day = (seconds.cast_signed() + offset).rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", day / 3600, day % 3600 / 60, day % 60)
}

/// The offset of local time from UTC in seconds, as `date` tells it;
/// asked once.
fn local_offset() -> i64 {
    static OFFSET: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *OFFSET.get_or_init(|| {
        let output = std::process::Command::new("date").arg("+%z").output();
        let Ok(output) = output else {
            return 0;
        };
        let text = String::from_utf8_lossy(&output.stdout);
        let text = text.trim();
        let (sign, digits) = match text.as_bytes().first() {
            Some(b'-') => (-1, &text[1..]),
            Some(b'+') => (1, &text[1..]),
            _ => (1, text),
        };
        let hours: i64 = digits.get(..2).and_then(|h| h.parse().ok()).unwrap_or(0);
        let minutes: i64 = digits.get(2..4).and_then(|m| m.parse().ok()).unwrap_or(0);
        sign * (hours * 3600 + minutes * 60)
    })
}

/// How long `took` is, as people say it.
fn length(took: Duration) -> String {
    let seconds = took.as_secs();
    match seconds {
        0..60 => format!("{seconds} s"),
        60..3600 => format!("{} min", seconds / 60),
        _ => format!("{} h {} min", seconds / 3600, seconds % 3600 / 60),
    }
}

/// `count` of `total`, as the status line and the list say it.
fn counted(done: Option<u32>, total: Option<u32>) -> String {
    match (done, total) {
        (Some(done), Some(total)) => format!(" ({done}/{total})"),
        (Some(done), None) => format!(" ({done} done)"),
        _ => String::new(),
    }
}

impl Agent {
    /// What it is doing, in a line: at work, waiting, done, gone.
    fn state(&self) -> String {
        if let Some(left) = self.left {
            return format!("gone {} ago", length(left.elapsed()));
        }
        match &self.now {
            Some(step) if step.tool == "wait_for_review" => "waiting for you".to_owned(),
            Some(_) => format!("at work, {}", length(self.since.elapsed())),
            None if self.last.elapsed() < Duration::from_secs(30) => {
                format!("at work, {}", length(self.since.elapsed()))
            }
            None => format!("connected, quiet for {}", length(self.last.elapsed())),
        }
    }

    /// What it is doing now, in a few words, for the status line.
    fn doing(&self) -> Option<String> {
        if self.left.is_some() {
            return None;
        }
        let said = self
            .said
            .as_ref()
            .filter(|said| said.at.elapsed() < SAID_STANDS);
        match (&self.now, said) {
            (Some(step), _) if step.tool == "wait_for_review" => Some("waiting for you".to_owned()),
            (_, Some(said)) => Some(format!(
                "{}{}",
                said.message,
                counted(said.done, said.total)
            )),
            (Some(step), None) => Some(in_words(&step.tool)),
            (None, None) => None,
        }
    }
}

impl Viewer {
    /// What an agent did, as the MCP server tells it as it happens.
    pub fn agent_activity(&mut self, event: Event) {
        let now = Instant::now();
        let session = match &event {
            Event::Connected { session, .. }
            | Event::Began { session, .. }
            | Event::Ended { session, .. }
            | Event::Said { session, .. }
            | Event::Left { session } => session.clone(),
        };
        let called = self.agent_at_work().is_some();
        let index = self.agent_index(&session, called);
        let agent = &mut self.activity.agents[index];
        agent.last = now;
        match event {
            Event::Connected { name, version, .. } => {
                agent.name = name;
                agent.version = version;
                agent.called |= called;
            }
            Event::Began { tool, about, .. } => {
                if tool != "report_progress" {
                    agent.now = Some(Step {
                        at: SystemTime::now(),
                        tool,
                        about,
                        took: None,
                        error: None,
                    });
                }
            }
            Event::Ended {
                tool,
                millis,
                error,
                ..
            } => {
                if tool != "report_progress" {
                    let mut step = agent.now.take().unwrap_or(Step {
                        at: SystemTime::now(),
                        tool,
                        about: String::new(),
                        took: None,
                        error: None,
                    });
                    step.took = Some(millis);
                    agent.calls += 1;
                    if error.is_some() {
                        agent.failures += 1;
                    }
                    step.error = error;
                    agent.steps.push_front(step);
                    agent.steps.truncate(STEPS_KEPT);
                }
            }
            Event::Said {
                message,
                done,
                total,
                ..
            } => {
                agent.said = Some(Said {
                    message: message.trim().chars().take(120).collect(),
                    done,
                    total,
                    at: now,
                });
            }
            Event::Left { .. } => {
                agent.left = Some(now);
                agent.now = None;
            }
        }
        self.forget_agents();
        self.refresh_agents_list();
    }

    /// The agent of `session`, made where it is new (`called`: by the
    /// window).
    fn agent_index(&mut self, session: &str, called: bool) -> usize {
        if let Some(index) = self
            .activity
            .agents
            .iter()
            .position(|a| a.session == session)
        {
            return index;
        }
        let now = Instant::now();
        self.activity.agents.push(Agent {
            session: session.to_owned(),
            name: "agent".to_owned(),
            version: String::new(),
            since: now,
            last: now,
            now: None,
            said: None,
            steps: VecDeque::new(),
            calls: 0,
            failures: 0,
            left: None,
            called,
        });
        self.activity.agents.len() - 1
    }

    /// Of more agents than are kept, those gone longest go first.
    fn forget_agents(&mut self) {
        while self.activity.agents.len() > AGENTS_KEPT {
            let oldest = self
                .activity
                .agents
                .iter()
                .enumerate()
                .min_by_key(|(_, agent)| (agent.left.is_none(), agent.last))
                .map(|(index, _)| index);
            let Some(index) = oldest else {
                break;
            };
            self.activity.agents.remove(index);
        }
    }

    /// What the status line says of agents, if anything: what the one at
    /// work is doing, or that they are paused.
    pub(super) fn agents_status(&self) -> String {
        if self.activity.paused {
            return "agents paused  ".to_owned();
        }
        let doing = self
            .activity
            .agents
            .iter()
            .filter_map(Agent::doing)
            .next_back();
        match doing {
            Some(doing) => {
                let short: String = doing.chars().take(48).collect();
                let cut = if short.len() < doing.len() { "…" } else { "" };
                format!("agent: {short}{cut}  ")
            }
            // Called, and not connected yet: starting.
            None if self.agent_at_work().is_some() => "agent starting  ".to_owned(),
            None => String::new(),
        }
    }

    /// The lines of the list of agents (`:agents`).
    fn agents_items(&self) -> Vec<(String, Target)> {
        let mut items: Vec<(String, Target)> = Vec::new();
        let nothing = |text: String| (text, Target::Nothing);
        if self.activity.agents.is_empty() {
            items.push(nothing(
                "no agent has connected since the window opened".to_owned(),
            ));
        }
        let mut agents: Vec<&Agent> = self.activity.agents.iter().collect();
        agents.sort_by_key(|agent| (agent.left.is_some(), std::cmp::Reverse(agent.last)));
        for agent in agents {
            let version = if agent.version.is_empty() {
                String::new()
            } else {
                format!(" {}", agent.version)
            };
            let called = if agent.called {
                " · called by the window"
            } else {
                ""
            };
            let failed = match agent.failures {
                0 => String::new(),
                1 => ", 1 failed".to_owned(),
                n => format!(", {n} failed"),
            };
            let mark = if agent.left.is_some() { "○" } else { "●" };
            items.push(nothing(format!(
                "{mark} {}{version}\t{} · {} calls{failed}{called}",
                agent.name,
                agent.state(),
                agent.calls
            )));
            if let Some(said) = &agent.said {
                items.push(nothing(format!(
                    "    “{}”{}\t{} ago",
                    said.message,
                    counted(said.done, said.total),
                    length(said.at.elapsed())
                )));
            }
            if let Some(step) = &agent.now {
                let about = if step.about.is_empty() {
                    String::new()
                } else {
                    format!("  {}", step.about)
                };
                items.push(nothing(format!(
                    "    {}  now: {}{about}",
                    clock(step.at),
                    in_words(&step.tool)
                )));
            }
            for step in agent.steps.iter().take(STEPS_SHOWN) {
                let about = if step.about.is_empty() {
                    String::new()
                } else {
                    format!("  {}", step.about)
                };
                let outcome = match (&step.error, step.took) {
                    (Some(error), _) => {
                        let error: String = error.chars().take(80).collect();
                        format!("\tfailed: {error}")
                    }
                    (None, Some(took)) => format!("\t{took} ms"),
                    (None, None) => String::new(),
                };
                let mark = if step.error.is_some() { "✗" } else { " " };
                items.push(nothing(format!(
                    "  {mark} {}  {}{about}{outcome}",
                    clock(step.at),
                    in_words(&step.tool)
                )));
            }
        }
        if let Some(what) = self.agent_at_work() {
            items.push((
                format!("stop the agent the window called\tit works on {what}"),
                Target::Command("agent stop".to_owned()),
            ));
        }
        if self.agent_on_call() {
            items.push((
                "what the agent the window called wrote last\tagent.log".to_owned(),
                Target::Command("agent log".to_owned()),
            ));
        }
        if self.activity.paused {
            items.push((
                "let agents go on\tthey are paused: they may read, not change".to_owned(),
                Target::Command("agents resume".to_owned()),
            ));
        } else {
            items.push((
                "pause all agents\tthey may read, and change nothing until you resume".to_owned(),
                Target::Command("agents pause".to_owned()),
            ));
        }
        items
    }

    /// `:agents` (the list), `:agents pause`, `:agents resume`. Returns
    /// whether `command` was one of them; `:agents live` and `:agents
    /// propose` are the option's (`ai.edit_mode`).
    pub(super) fn activity_command(&mut self, command: &str) -> bool {
        match command.trim() {
            "agents" => {
                let items = self.agents_items();
                self.open_picker("agents", items, 0);
            }
            "agents pause" => {
                self.activity.paused = true;
                AGENTS_PAUSED.store(true, Ordering::Relaxed);
                self.notice(
                    "agents are paused: they may read, and change nothing (:agents resume)",
                );
                self.refresh_agents_list();
            }
            "agents resume" => {
                self.activity.paused = false;
                AGENTS_PAUSED.store(false, Ordering::Relaxed);
                self.notice("agents may go on");
                self.refresh_agents_list();
            }
            _ => return false,
        }
        true
    }

    /// The agents at work or gone a short while ago, for the AI pane:
    /// who, in what state, what it said, and its last steps in words.
    pub(super) fn agents_shown(&self) -> Vec<Shown> {
        let mut agents: Vec<&Agent> = self
            .activity
            .agents
            .iter()
            .filter(|agent| {
                agent
                    .left
                    .is_none_or(|left| left.elapsed() < Duration::from_mins(2))
            })
            .collect();
        agents.sort_by_key(|agent| std::cmp::Reverse(agent.last));
        agents
            .into_iter()
            .map(|agent| Shown {
                name: agent.name.clone(),
                state: agent.state(),
                said: agent
                    .said
                    .as_ref()
                    .map(|said| (said.message.clone(), said.done, said.total)),
                steps: agent
                    .now
                    .iter()
                    .map(|step| format!("{}  now: {}", clock(step.at), in_words(&step.tool)))
                    .chain(agent.steps.iter().take(3).map(|step| {
                        let failed = if step.error.is_some() {
                            "  (failed)"
                        } else {
                            ""
                        };
                        format!("{}  {}{failed}", clock(step.at), in_words(&step.tool))
                    }))
                    .take(3)
                    .collect(),
                gone: agent.left.is_some(),
            })
            .collect()
    }

    /// Whether the writer paused agents.
    pub(super) fn agents_paused(&self) -> bool {
        self.activity.paused
    }

    /// The list of agents made anew where it is up, as they work.
    fn refresh_agents_list(&mut self) {
        if self.picker_title() == Some("agents") {
            let items = self.agents_items();
            self.replace_picker_items(items);
        }
    }
}
