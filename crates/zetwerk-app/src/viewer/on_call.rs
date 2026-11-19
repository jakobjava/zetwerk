//! An agent on call: the writer's own agent, started by the window when
//! there is something for it, for that, and gone when it is done.
//!
//! The program asks no model anything, and that stays so: agents come in
//! through MCP. But an agent had to be running already, started from a
//! terminal and waiting, or a question asked in the window (`gq`, `:ask`)
//! and a review asked for (`gQ`) waited for nobody. With `[ai] agent` in
//! the settings — a command of the writer's own, for Claude Code say
//! `claude -p --allowedTools mcp__zetwerk` — the window starts it as soon
//! as something is asked of agents.
//!
//! - **What it is told.** The command is run by the shell with the task
//!   as its last argument (unless it names `$ZETWERK_TASK` itself, which
//!   holds the task too): answer the open questions and do the reviews
//!   asked for, with the window's tools, then stop. `ZETWERK_SOCKET` names
//!   this window's socket, and `zetwerk mcp` goes there first: the agent
//!   works in the window that called it, however many there are.
//! - **One at a time.** What is asked while one runs is left for it, and
//!   if anything still waits when it is done, it is called once more.
//! - **Nothing when nothing is asked.** No process waits in between; the
//!   window is woken when the agent is done, not asked how it goes.
//! - **The writer stops it** with `:agent stop`, which ends it and all it
//!   started. `:agent` calls it for what waits, `:agent log` says where
//!   what it wrote is (the state directory, `agent.log`, for the last
//!   call).
//!
//! What the agent does is as any agent's: its changes are proposals or
//! live as `[ai] edit_mode` says, it saves only where the settings let
//! agents save, and it never fetches a package.

use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use super::Viewer;

/// What the user's settings say of the agent on call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// The command that starts the writer's agent, as the shell reads it.
    pub command: Option<String>,
    /// This window's socket, for the agent to reach it by.
    pub socket: Option<PathBuf>,
}

/// The call under way.
struct Running {
    /// Its process, which leads a group of its own.
    group: u32,
    started: Instant,
    /// What it was called for, in words.
    what: String,
    done: Receiver<Result<ExitStatus, String>>,
    log: PathBuf,
}

/// What the viewer keeps of the agent on call.
#[derive(Default)]
pub(super) struct OnCall {
    settings: Settings,
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
    running: Option<Running>,
    /// Something was asked while it ran: once it is done, it is called
    /// again if anything still waits.
    again: bool,
}

impl Drop for OnCall {
    /// The window closes: the agent it called goes with it.
    fn drop(&mut self) {
        if let Some(running) = self.running.take() {
            end_group(running.group);
        }
    }
}

/// How long a call may take before the writer is told it is long.
const LONG: Duration = Duration::from_mins(10);

/// The task for an agent called for `questions` and `reviews`.
fn task(questions: usize, reviews: usize) -> String {
    let mut asked = Vec::new();
    if questions > 0 {
        asked.push(format!(
            "{questions} open question{} (list_notes gives them; answer each with answer_note, \
             with a proposal where a change is the answer)",
            if questions == 1 { "" } else { "s" }
        ));
    }
    if reviews > 0 {
        asked.push(format!(
            "{reviews} review request{} (list_suggestions gives them under requests; answer \
             with suggest, and with revise_suggestion where a suggestion waits for you)",
            if reviews == 1 { "" } else { "s" }
        ));
    }
    let asked = if asked.is_empty() {
        "whatever is waiting for an agent (get_state says)".to_owned()
    } else {
        asked.join(", and ")
    };
    format!(
        "The author is writing in the zetwerk window, whose tools you have as the MCP server \
         zetwerk, and has asked something of you: {asked}. Read the author's brief first \
         (get_brief) and follow it; read what is asked about (read_source, get_outline). Suggest, \
         do not edit, unless the author asked for the change itself. Do not save. When all of \
         it is answered, stop: do not wait for more."
    )
}

/// What the agent wrote last, for the status line: its last line that
/// says something.
fn last_line(log: &PathBuf) -> Option<String> {
    let text = std::fs::read_to_string(log).ok()?;
    let line = text.lines().rev().find(|line| !line.trim().is_empty())?;
    Some(line.trim().chars().take(100).collect())
}

/// Ends the group of processes `group` leads: the shell, the agent, and
/// what the agent started.
fn end_group(group: u32) {
    let _ = Command::new("kill")
        .args(["-TERM", "--", &format!("-{group}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

impl Viewer {
    /// Takes what the user's settings say of the agent on call.
    pub fn set_agent_on_call(&mut self, settings: &Settings) {
        self.on_call.settings = settings.clone();
    }

    /// The command that starts the agent on call, if one is named.
    pub(super) fn agent_named(&self) -> Option<&str> {
        self.on_call.settings.command.as_deref()
    }

    /// Names the command that starts the agent on call (`:set ai.agent`);
    /// nothing: none.
    pub(super) fn set_agent_command(&mut self, command: &str) {
        self.on_call.settings.command = (!command.is_empty()).then(|| command.to_owned());
    }

    /// Wakes the window with `wake` when a call of the agent is done.
    pub fn agent_in_background(&mut self, wake: impl Fn() + Send + Sync + 'static) {
        self.on_call.wake = Some(Arc::new(wake));
    }

    /// Whether an agent is on call: the settings name one.
    pub(super) fn agent_on_call(&self) -> bool {
        self.on_call
            .settings
            .command
            .as_deref()
            .is_some_and(|command| !command.trim().is_empty())
    }

    /// Something was asked of agents: the agent on call is called for
    /// it, if there is one and it is not at work already. Returns
    /// whether it was called (or is at work).
    pub(super) fn call_agent(&mut self) -> bool {
        if !self.agent_on_call() {
            return false;
        }
        if self.on_call.running.is_some() {
            self.on_call.again = true;
            return true;
        }
        let (questions, _) = self.notes_waiting();
        let reviews = self.review_requests().iter().filter(|r| !r.done).count();
        match self.start_agent(questions, reviews) {
            Ok(what) => {
                self.notice(format!(
                    "the agent is called for {what}  (:agent stop ends it)"
                ));
                true
            }
            Err(why) => {
                self.alert(format!("the agent could not be called: {why}"));
                false
            }
        }
    }

    /// Starts the agent for `questions` and `reviews`. Returns what it
    /// was called for, in words.
    fn start_agent(&mut self, questions: usize, reviews: usize) -> Result<String, String> {
        let command = self
            .on_call
            .settings
            .command
            .clone()
            .ok_or("no agent is named in the settings ([ai] agent)")?;
        let what = match (questions, reviews) {
            (0, 0) => "what waits".to_owned(),
            (q, 0) => format!("{q} question{}", if q == 1 { "" } else { "s" }),
            (0, r) => format!("{r} review{}", if r == 1 { "" } else { "s" }),
            (q, r) => format!("{q} question{} and {r} review{}", plural(q), plural(r)),
        };
        let directory = self
            .state_dir
            .clone()
            .ok_or("there is no state directory to keep what it says in")?;
        std::fs::create_dir_all(&directory).map_err(|err| err.to_string())?;
        let log = directory.join("agent.log");
        let out = std::fs::File::create(&log).map_err(|err| format!("{}: {err}", log.display()))?;
        let err = out.try_clone().map_err(|err| err.to_string())?;
        let script = if command.contains("ZETWERK_TASK") {
            command
        } else {
            format!("{command} \"$ZETWERK_TASK\"")
        };
        let mut shell = Command::new("sh");
        shell
            .arg("-c")
            .arg(&script)
            .env("ZETWERK_TASK", task(questions, reviews))
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err);
        if let Some(socket) = &self.on_call.settings.socket {
            shell.env("ZETWERK_SOCKET", socket);
        }
        if let Some(root) = self.project.main_path().and_then(|main| main.parent()) {
            shell.current_dir(root);
        }
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut shell, 0);
        let mut child = shell.spawn().map_err(|err| err.to_string())?;
        let group = child.id();
        let (done, waited) = mpsc::channel();
        let wake = self.on_call.wake.clone();
        std::thread::spawn(move || {
            let status = child.wait().map_err(|err| err.to_string());
            let _ = done.send(status);
            if let Some(wake) = wake {
                wake();
            }
        });
        self.on_call.again = false;
        self.on_call.running = Some(Running {
            group,
            started: Instant::now(),
            what: what.clone(),
            done: waited,
            log,
        });
        Ok(what)
    }

    /// What became of the agent's call, if it is over: said in the status
    /// line, and the agent called again where something was asked
    /// meanwhile and still waits. Costs nothing while nothing runs; the
    /// window calls it whenever it was woken. Returns whether the call
    /// ended.
    pub fn agent_progress(&mut self) -> bool {
        let Some(running) = &self.on_call.running else {
            return false;
        };
        let status = match running.done.try_recv() {
            Ok(status) => status,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => Err("it was lost".to_owned()),
        };
        let Some(running) = self.on_call.running.take() else {
            return false;
        };
        let took = running.started.elapsed().as_secs();
        match status {
            Ok(status) if status.success() => {
                self.notice(format!(
                    "the agent is done with {} ({took} s): what it suggests is on the page, :notes and gR list it",
                    running.what
                ));
            }
            Ok(status) => {
                let said = last_line(&running.log).unwrap_or_default();
                let code = status.code().map_or_else(
                    || "it was stopped".to_owned(),
                    |code| format!("it ended with {code}"),
                );
                self.alert(format!(
                    "the agent did not finish: {code}; {said}  (:agent log)"
                ));
            }
            Err(why) => self.alert(format!("the agent did not finish: {why}")),
        }
        if std::mem::take(&mut self.on_call.again) {
            let (questions, _) = self.notes_waiting();
            let reviews = self.review_requests().iter().filter(|r| !r.done).count();
            if questions + reviews > 0 {
                self.call_agent();
            }
        }
        true
    }

    /// `:agent`, `:agent stop`, `:agent log`. Returns whether `command`
    /// was one of them.
    pub(super) fn agent_command(&mut self, command: &str) -> bool {
        let Some(rest) = command.strip_prefix("agent") else {
            return false;
        };
        if !rest.is_empty() && !rest.starts_with(' ') {
            return false;
        }
        match rest.trim() {
            "" if !self.agent_on_call() => self.alert(
                "no agent is on call: [ai] agent = \"claude -p --allowedTools mcp__zetwerk\" \
                 in the settings names one",
            ),
            "" => {
                if let Some(running) = &self.on_call.running {
                    let took = running.started.elapsed().as_secs();
                    let what = running.what.clone();
                    let long = if running.started.elapsed() > LONG {
                        ", for long"
                    } else {
                        ""
                    };
                    self.notice(format!(
                        "the agent is at work on {what}, {took} s{long}  (:agent stop ends it)"
                    ));
                } else {
                    self.call_agent();
                }
            }
            "stop" => match self.on_call.running.take() {
                Some(running) => {
                    end_group(running.group);
                    self.on_call.again = false;
                    self.notice(format!("the agent was stopped ({})", running.what));
                }
                None => self.notice("the agent is not at work"),
            },
            "log" => match &self.state_dir {
                Some(directory) => {
                    let log = directory.join("agent.log");
                    self.notice(format!("what the agent wrote last: {}", log.display()));
                }
                None => self.notice("there is no state directory, and no log"),
            },
            other => self.alert(format!(
                "no :agent {other}: :agent, :agent stop, :agent log"
            )),
        }
        true
    }

    /// Whether the agent on call is at work, for the status line.
    pub(super) fn agent_at_work(&self) -> Option<&str> {
        self.on_call
            .running
            .as_ref()
            .map(|running| running.what.as_str())
    }
}

/// The ending of a count's noun.
fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}
