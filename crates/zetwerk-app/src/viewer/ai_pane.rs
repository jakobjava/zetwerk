//! The AI pane: beside the page, what is asked of agents, what they
//! answer, and what they are doing.
//!
//! Research is written with agents now, and asking them, following them
//! and deciding on what they suggest had no place of its own: a question
//! was a key (`gq`), its answer a tag in the margin, what the agent did
//! a line in the status line. The pane (`gI`, `:ai`) stands to the right
//! of the page and holds all of it:
//!
//! - **What agents are doing,** at its top: each agent at work, its
//!   state, what it says it is doing with how far it is, its last steps
//!   (see `activity`).
//! - **Threads,** below: each question asked and each review asked for,
//!   newest and open first, with the text it is about, what was said in
//!   it, and the suggestions made for it with what became of them.
//! - **A line to ask in,** at its foot: what is typed there is asked of
//!   agents about what is selected, or the paragraph of the cursor
//!   (`:ask`); `/review clarity` (or any kind of review, with words after
//!   a comma) asks for a review of it (`:review this`); `/cite`,
//!   `/explain`, `/draft` and `/check maths` ask for those. Sending calls
//!   the agent on call, if one is named (see `on_call`).
//!
//! Keys, with the pane in front (`Ctrl-W l`, or as it opens; `Ctrl-W h`
//! goes back to the page): `j` and `k` go through the threads, Enter
//! goes to the text one is about, `a` accepts what waits in it and `r`
//! rejects it (`:review accept note 3`, one undo step), `i` (or `/`)
//! types in the line, Enter there sends and Escape leaves it; Escape
//! goes back to the page, `q` puts the pane away.
//!
//! The pane is typeset as the other panels are, and only when what it
//! shows changes: nothing is asked of the machine while nothing happens.

use std::fmt::Write as _;

use typst::syntax::FileId;

use super::notes::Author;
use super::review::State;
use super::{PANE, Quad, Scene, TextureId, Viewer, solid};
use crate::keys::{Key, KeyPress};

/// How much of the window the pane takes, and at least, in pixels.
const SHARE: f64 = 0.38;
const NARROWEST: u32 = 300;
/// How many threads above the chosen one stay in view.
const ABOVE: usize = 1;
/// How many characters of the text a thread is about are shown.
const QUOTED: usize = 110;
/// How many characters of a suggestion's old and new text are shown.
const CHANGED: usize = 140;

const FONT: &str = "DejaVu Sans Mono";
const TEXT: &str = "#ebdbb2";
const FAINT: &str = "#928374";
const ACCENT: &str = "#fabd2f";
const GOOD: &str = "#b8bb26";
const BAD: &str = "#fb4934";
const EDGE: &str = "#504945";

/// What is kept of the pane.
#[derive(Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each is a state of its own: shown, in front, typing, after Ctrl-W"
)]
pub(super) struct AiPane {
    pub(super) shown: bool,
    /// Whether keys go to the pane, not the page.
    focused: bool,
    /// Whether what is typed goes into the line to ask in.
    typing: bool,
    input: String,
    chosen: usize,
    /// `Ctrl-W` was pressed: the next key says which pane.
    window_key: bool,
}

/// A thread of the pane: what was asked, and what came of it.
pub(super) struct Thread {
    /// Typst markup for it, without its frame.
    pub(super) markup: String,
    /// Where the text it is about is.
    at: Option<(FileId, usize)>,
    /// What `a` and `r` take: `note 3`, `request 2`, if anything waits.
    waiting: Option<String>,
}

/// `text` as a Typst string literal.
fn literal(text: &str) -> String {
    let mut written = String::with_capacity(text.len() + 2);
    written.push('"');
    for c in text.chars() {
        match c {
            '\\' => written.push_str("\\\\"),
            '"' => written.push_str("\\\""),
            '\n' | '\r' | '\t' => written.push(' '),
            c => written.push(c),
        }
    }
    written.push('"');
    written
}

/// `text` in `colour`, as markup.
fn coloured(colour: &str, text: &str) -> String {
    format!("#text(fill: rgb(\"{colour}\"), {})", literal(text))
}

/// `text` cut to `most` characters, with an ellipsis where it was cut,
/// on one line.
fn cut(text: &str, most: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= most {
        return flat;
    }
    let mut cut: String = flat.chars().take(most.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// What `source` (some of the text) reads as, roughly, for a line of
/// the pane: without the lines that are code (`#set …`), the marks of
/// strong and emphasised text and of raw text, and the equations' dollar
/// signs. The pane shows what is written as it reads, never its source.
pub(super) fn readable(source: &str) -> String {
    let lines: Vec<&str> = source
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#') && !line.starts_with("//"))
        .collect();
    let text: String = lines
        .join(" ")
        .chars()
        .filter(|c| !matches!(c, '*' | '_' | '`' | '$'))
        .collect();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    text.trim_start_matches(['=', '-', '+', ' '])
        .trim()
        .to_owned()
}

/// A bar of `done` of `total`, in blocks.
fn bar(done: u32, total: u32) -> String {
    let total = total.max(1);
    let done = done.min(total);
    let filled = (done * 10 / total) as usize;
    format!(
        "{}{} {done}/{total}",
        "▓".repeat(filled),
        "░".repeat(10 - filled)
    )
}

/// What a request typed into the line asks: the command it is, and how
/// it is said back.
pub(super) fn asked(typed: &str) -> Option<(String, String)> {
    let typed = typed.trim();
    if typed.is_empty() {
        return None;
    }
    let ask = |question: &str| Some((format!("ask {question}"), "asked".to_owned()));
    let Some(rest) = typed.strip_prefix('/') else {
        return ask(typed);
    };
    let (word, more) = rest.split_once(' ').unwrap_or((rest, ""));
    let more = more.trim();
    match word {
        "review" => {
            let what = if more.is_empty() { "anything" } else { more };
            Some((
                format!("review this {what}"),
                "a review asked for".to_owned(),
            ))
        }
        "check" => {
            let what = if more.is_empty() { "maths" } else { more };
            Some((
                format!("review this {what}"),
                "a check asked for".to_owned(),
            ))
        }
        "cite" => ask(&format!(
            "Find sources for what this says and suggest where to cite them. {more}"
        )),
        "explain" => ask(&format!("Explain this to me. {more}")),
        "draft" => ask(&format!(
            "Draft this from my notes, as suggestions I take one by one. {more}"
        )),
        _ => ask(typed),
    }
}

impl Viewer {
    /// How wide the pane is, in pixels: none where it is not shown.
    pub(super) fn ai_width(&self) -> u32 {
        if !self.ai.shown || self.editor.is_none() {
            return 0;
        }
        let width = self.window.0;
        let wanted = (f64::from(width) * SHARE).round() as u32;
        wanted
            .max(NARROWEST.min(width / 2))
            .min(width.saturating_sub(100))
    }

    /// Whether keys go to the pane.
    pub(super) fn ai_focused(&self) -> bool {
        self.ai.shown && self.ai.focused
    }

    /// `:ai`: shows the pane, in front, or puts it away; `:ai <words>`
    /// asks them as the line would.
    pub(super) fn ai_command(&mut self, command: &str) -> bool {
        let Some(rest) = command.strip_prefix("ai") else {
            return false;
        };
        if !rest.is_empty() && !rest.starts_with(' ') {
            return false;
        }
        let rest = rest.trim();
        if self.editor.is_none() {
            self.alert("the AI pane is beside a document: open one first");
            return true;
        }
        if rest.is_empty() {
            self.ai.shown = !self.ai.shown;
            self.ai.focused = self.ai.shown;
            self.ai.typing = false;
            self.arrange();
            if self.ai.shown {
                self.notice(
                    "the AI pane: i asks, j and k go through the threads, Enter goes to the text, \
                     Ctrl-W h back to the page",
                );
            }
        } else {
            self.ai_send(rest);
        }
        true
    }

    /// Sends `typed` as the line would: a question or a request.
    fn ai_send(&mut self, typed: &str) {
        let Some((command, _)) = asked(typed) else {
            return;
        };
        self.execute(&command);
        if !self.agent_on_call() {
            self.notice(
                "asked; no agent is on call: [ai] agent in the settings names one, or start one \
                 yourself (zetwerk mcp)",
            );
        }
        self.ai.chosen = 0;
    }

    /// A key, where the pane has it: `Ctrl-W` and what follows it, and
    /// every key while the pane is in front. Returns whether it was the
    /// pane's.
    pub(super) fn ai_pane_key(&mut self, press: KeyPress) -> bool {
        if !self.ai.shown || self.picker.is_some() {
            self.ai.window_key = false;
            return false;
        }
        let normal = self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.mode() == super::editor::Mode::Normal);
        if std::mem::take(&mut self.ai.window_key) {
            match press.key {
                Key::Char('l' | 'w') | Key::Right => self.ai.focused = true,
                Key::Char('h') | Key::Left => self.ai.focused = false,
                Key::Char('W') => self.ai.focused = !self.ai.focused,
                Key::Char('c' | 'q') => {
                    self.ai.shown = false;
                    self.ai.focused = false;
                    self.arrange();
                }
                _ => {}
            }
            return true;
        }
        let window_key = press.ctrl && press.key == Key::Char('w');
        if window_key && (normal || self.ai.focused) && !self.ai.typing {
            self.ai.window_key = true;
            return true;
        }
        if !self.ai.focused {
            return false;
        }
        if self.ai.typing {
            self.ai_typing_key(press);
            return true;
        }
        let threads = self.ai_threads().len();
        match press.key {
            Key::Char('j') | Key::Down => {
                self.ai.chosen = (self.ai.chosen + 1).min(threads.saturating_sub(1));
            }
            Key::Char('k') | Key::Up => self.ai.chosen = self.ai.chosen.saturating_sub(1),
            Key::Char('g') => self.ai.chosen = 0,
            Key::Char('G') => self.ai.chosen = threads.saturating_sub(1),
            Key::Char('i') => self.ai.typing = true,
            Key::Char('/') => {
                self.ai.typing = true;
                "/".clone_into(&mut self.ai.input);
            }
            Key::Char('a') => self.ai_decide(true),
            Key::Char('r') => self.ai_decide(false),
            Key::Enter => self.ai_go(),
            Key::Escape => self.ai.focused = false,
            Key::Char('q') => {
                self.ai.shown = false;
                self.ai.focused = false;
                self.arrange();
            }
            Key::Char(':') => {
                // The command line is the page's.
                self.ai.focused = false;
                return false;
            }
            _ => {}
        }
        true
    }

    /// A key while typing in the line to ask in.
    fn ai_typing_key(&mut self, press: KeyPress) {
        match press.key {
            Key::Escape => self.ai.typing = false,
            Key::Enter => {
                let typed = std::mem::take(&mut self.ai.input);
                self.ai.typing = false;
                self.ai_send(&typed);
            }
            Key::Backspace if press.ctrl => {
                let kept = self.ai.input.trim_end().rfind(' ').map_or(0, |at| at + 1);
                self.ai.input.truncate(kept);
            }
            Key::Backspace => {
                self.ai.input.pop();
            }
            Key::Char('w' | 'u') if press.ctrl => {
                let kept = if press.key == Key::Char('u') {
                    0
                } else {
                    self.ai.input.trim_end().rfind(' ').map_or(0, |at| at + 1)
                };
                self.ai.input.truncate(kept);
            }
            Key::Char(c) if !press.ctrl && !press.alt => self.ai.input.push(c),
            _ => {}
        }
    }

    /// Enter on a thread: the cursor to the text it is about, the page in
    /// front.
    fn ai_go(&mut self) {
        let threads = self.ai_threads();
        let Some((file, at)) = threads.get(self.ai.chosen).and_then(|thread| thread.at) else {
            return;
        };
        let Some(editor) = &mut self.editor else {
            return;
        };
        if editor.file() == file {
            editor.set_cursor(at.min(editor.text().len()));
            self.follow_cursor();
        } else {
            self.open_file(file, Some(at));
        }
        self.ai.focused = false;
    }

    /// `a` and `r` on a thread: what waits in it, accepted or rejected.
    fn ai_decide(&mut self, accept: bool) {
        let threads = self.ai_threads();
        let Some(which) = threads.get(self.ai.chosen).and_then(|t| t.waiting.clone()) else {
            self.notice("nothing waits in this thread to be accepted or rejected");
            return;
        };
        let verb = if accept { "accept" } else { "reject" };
        self.execute(&format!("review {verb} {which}"));
    }

    /// The threads of the pane, open first, newest first.
    pub(super) fn ai_threads(&self) -> Vec<Thread> {
        let Some(editor) = &self.editor else {
            return Vec::new();
        };
        let file = editor.file();
        let suggestions = self.suggestions();
        let mut threads: Vec<(bool, u64, Thread)> = Vec::new();
        for note in self.notes() {
            if !note.to_agent || note.by != Author::User {
                continue;
            }
            let answered = note
                .answers
                .last()
                .is_some_and(|(by, _)| *by == Author::Agent);
            let made: Vec<_> = suggestions
                .iter()
                .filter(|s| s.note == Some(note.id))
                .collect();
            let open = made.iter().any(|s| s.state == State::Open);
            let mut markup = String::new();
            let state = if note.resolved {
                coloured(FAINT, "resolved")
            } else if answered {
                coloured(GOOD, "answered")
            } else {
                coloured(ACCENT, "waiting for an agent")
            };
            let _ = writeln!(
                markup,
                "{} {} {state} \\",
                coloured(ACCENT, "You asked"),
                coloured(FAINT, "·")
            );
            let _ = writeln!(
                markup,
                "{} \\",
                coloured(FAINT, &format!("“{}”", cut(&readable(&note.quote), QUOTED)))
            );
            let _ = writeln!(markup, "{} \\", coloured(TEXT, &note.text));
            for (by, said) in &note.answers {
                let who = if *by == Author::Agent {
                    "Agent:"
                } else {
                    "You:"
                };
                let colour = if *by == Author::Agent { GOOD } else { ACCENT };
                let _ = writeln!(
                    markup,
                    "{} {} \\",
                    coloured(colour, who),
                    coloured(TEXT, said)
                );
            }
            push_suggestions(&mut markup, &made);
            let waiting = open.then(|| format!("note {}", note.id));
            let open_thread = !note.resolved && (!answered || open);
            threads.push((
                open_thread,
                note.id,
                Thread {
                    markup,
                    at: (note.file == file).then_some((note.file, note.range.start)),
                    waiting,
                },
            ));
        }
        threads.extend(self.request_threads(file));
        threads.sort_by_key(|(open, id, _)| (std::cmp::Reverse(*open), std::cmp::Reverse(*id)));
        threads.into_iter().map(|(_, _, thread)| thread).collect()
    }

    /// The threads of the review requests: whether each is open, its
    /// number, and it.
    fn request_threads(&self, file: FileId) -> Vec<(bool, u64, Thread)> {
        let suggestions = self.suggestions();
        let mut threads = Vec::new();
        for request in self.review_requests() {
            let made: Vec<_> = suggestions
                .iter()
                .filter(|s| s.request == Some(request.id))
                .collect();
            let open = made.iter().any(|s| s.state == State::Open);
            let mut markup = String::new();
            let state = if request.done {
                coloured(GOOD, "done")
            } else {
                coloured(ACCENT, "waiting for an agent")
            };
            let section = if request.section.is_empty() {
                String::new()
            } else {
                format!(" of {}", request.section)
            };
            let _ = writeln!(
                markup,
                "{} {} {state} \\",
                coloured(
                    ACCENT,
                    &format!("You asked for a review: {}{section}", request.what)
                ),
                coloured(FAINT, "·")
            );
            if !request.words.is_empty() {
                let _ = writeln!(markup, "{} \\", coloured(TEXT, &request.words));
            }
            let _ = writeln!(
                markup,
                "{} \\",
                coloured(
                    FAINT,
                    &format!("“{}”", cut(&readable(&request.quote), QUOTED))
                )
            );
            push_suggestions(&mut markup, &made);
            threads.push((
                !request.done || open,
                request.id,
                Thread {
                    markup,
                    at: (request.file == file).then_some((request.file, request.range.start)),
                    waiting: open.then(|| format!("request {}", request.id)),
                },
            ));
        }
        threads
    }

    /// The markup of the pane, `width` by `height` points.
    fn ai_markup(&self, width: f64, height: f64) -> String {
        let mut agents = String::new();
        let shown = self.agents_shown();
        if shown.is_empty() {
            let waiting = if self.agent_at_work().is_some() {
                "the agent the window called is starting"
            } else if self.agent_on_call() {
                "no agent at work: ask below, and the agent on call is called"
            } else {
                "no agent at work; [ai] agent in the settings lets the window call one"
            };
            let _ = writeln!(agents, "{} \\", coloured(FAINT, waiting));
        }
        for agent in &shown {
            let mark = if agent.gone { "○" } else { "●" };
            let colour = if agent.gone { FAINT } else { GOOD };
            let _ = writeln!(
                agents,
                "{} {} \\",
                coloured(colour, &format!("{mark} {}", agent.name)),
                coloured(FAINT, &agent.state)
            );
            if let Some((message, done, total)) = &agent.said {
                let _ = writeln!(
                    agents,
                    "{} \\",
                    coloured(TEXT, &format!("  “{}”", cut(message, 80)))
                );
                if let (Some(done), Some(total)) = (done, total) {
                    let _ = writeln!(
                        agents,
                        "{} \\",
                        coloured(ACCENT, &format!("  {}", bar(*done, *total)))
                    );
                }
            }
            for step in &agent.steps {
                let _ = writeln!(agents, "{} \\", coloured(FAINT, &format!("  {step}")));
            }
        }
        let paused = if self.agents_paused() {
            format!(
                "{} \\\n",
                coloured(BAD, "agents are paused (:agents resume)")
            )
        } else {
            String::new()
        };
        let threads = self.ai_threads();
        let top = self.ai.chosen.saturating_sub(ABOVE);
        let mut cards = String::new();
        if threads.is_empty() {
            cards.push_str(&coloured(
                FAINT,
                "nothing asked yet: i, then a question about the paragraph here or what is selected",
            ));
        }
        for (index, thread) in threads.iter().enumerate().skip(top) {
            let chosen = index == self.ai.chosen && self.ai_focused() && !self.ai.typing;
            let stroke = if chosen {
                format!("1pt + rgb(\"{ACCENT}\")")
            } else {
                format!("0.5pt + rgb(\"{EDGE}\")")
            };
            let _ = writeln!(
                cards,
                "#block(width: 100%, inset: 6pt, radius: 3pt, stroke: {stroke}, below: 6pt, breakable: false)[{}]",
                thread.markup
            );
        }
        let line = if self.ai.typing {
            format!(
                "{}{}",
                coloured(TEXT, &format!("> {}", self.ai.input)),
                coloured(ACCENT, "▌")
            )
        } else if self.ai_focused() {
            coloured(
                FAINT,
                "i ask · / review, cite, explain, draft · j k Enter a r · Ctrl-W h page",
            )
        } else {
            coloured(FAINT, "Ctrl-W l to the pane · gI hides it")
        };
        let frame = if self.ai_focused() { ACCENT } else { EDGE };
        format!(
            r##"#set page(width: {width}pt, height: {height}pt, margin: (x: 9pt, top: 8pt, bottom: 34pt), fill: rgb("#1d2021"),
  footer: block(width: 100%, inset: 6pt, radius: 3pt, stroke: 0.5pt + rgb("{frame}"), fill: rgb("#282828"))[{line}])
#set text(font: "{FONT}", size: 9pt, fill: rgb("{TEXT}"), hyphenate: false)
#set par(leading: 0.5em, justify: false)
{agents}{paused}#line(length: 100%, stroke: 0.5pt + rgb("{EDGE}"))
#v(2pt)
{cards}
"##
        )
    }

    /// Draws the pane at the right of the window.
    pub(super) fn ai_pane(&mut self, scene: &mut Scene, has: &dyn Fn(TextureId) -> bool) {
        let width = self.ai_width();
        if width == 0 {
            return;
        }
        let left = f64::from(self.window.0 - width);
        let height = f64::from(self.window.1.saturating_sub(self.status_height()));
        solid(scene, has, PANE, (left, 0.0, f64::from(width), height));
        let scale = self.scale.max(0.1);
        let markup = self.ai_markup(f64::from(width) / scale, height / scale);
        let Some(image) = self.chrome.render(&markup, scale) else {
            return;
        };
        scene.quads.push(Quad {
            texture: image.id,
            x: left as f32,
            y: 0.0,
            width: image.pixmap.width() as f32,
            height: image.pixmap.height() as f32,
        });
        if !has(image.id) {
            scene.images.push(image);
        }
    }
}

/// The suggestions `made` for a thread, as lines of `markup`: each with
/// its kind, what became of it, and its change.
fn push_suggestions(markup: &mut String, made: &[&super::review::Suggestion]) {
    for suggestion in made {
        let (word, colour) = match suggestion.state {
            State::Open => ("waits: a accepts, r rejects", ACCENT),
            State::Later => ("put aside", FAINT),
            State::Accepted => ("accepted", GOOD),
            State::Rewritten => ("rewritten by you", GOOD),
            State::Rejected => ("rejected", BAD),
            State::Withdrawn => ("withdrawn", FAINT),
        };
        let category = if suggestion.category.is_empty() {
            "suggestion".to_owned()
        } else {
            suggestion.category.clone()
        };
        let _ = writeln!(
            markup,
            "{} {} \\",
            coloured(TEXT, &format!("▸ {category}")),
            coloured(colour, word)
        );
        if !suggestion.reason.is_empty() {
            let _ = writeln!(
                markup,
                "{} \\",
                coloured(FAINT, &format!("  {}", cut(&suggestion.reason, 120)))
            );
        }
        if !suggestion.quote.is_empty() {
            let _ = writeln!(
                markup,
                "{} \\",
                coloured(
                    BAD,
                    &format!("  − {}", cut(&readable(&suggestion.quote), CHANGED))
                )
            );
        }
        if !suggestion.new.is_empty() {
            let _ = writeln!(
                markup,
                "{} \\",
                coloured(
                    GOOD,
                    &format!("  + {}", cut(&readable(&suggestion.new), CHANGED))
                )
            );
        }
    }
}
