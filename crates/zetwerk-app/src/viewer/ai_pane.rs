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
//!
//! It is a window of its own, as a split is in Vim: lines of text, one
//! of them the cursor's line, threads folded to their first line but for
//! the one the cursor is on, `~` past the last line, a status line of
//! its own at its foot. Nothing is typed into the pane itself: `i` (or
//! `o`) opens the command line with `:ai ` on it, and what is typed
//! there is asked of agents about what is selected, or the paragraph of
//! the cursor (`:ask`); `:ai /review clarity` (or any kind of review,
//! with words after a comma) asks for a review of it (`:review this`);
//! `/cite`, `/explain`, `/draft` and `/check maths` ask for those.
//! Sending calls the agent on call, if one is named (see `on_call`).
//!
//! Keys, with the pane in front (`Ctrl-W l`, or as it opens; `Ctrl-W h`
//! goes back to the page): `j` and `k` go through the threads, `gg` and
//! `G` to the first and the last, Enter goes to the text one is about,
//! `a` accepts what waits in it and `r` rejects it (`:review accept note
//! 3`, one undo step), `zR` unfolds every thread and `zM` folds them
//! again (`za` switches); Escape goes back to the page, `q` puts the
//! pane away.
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

use crate::chrome::ACCENT;
use crate::chrome::ADDED as GOOD;
use crate::chrome::ALERT as BAD;
use crate::chrome::BACKGROUND;
use crate::chrome::DIM as FAINT;
use crate::chrome::FONT;
use crate::chrome::FOREGROUND as TEXT;
/// Vim's colours, as gruvbox has them: the cursor's line, the status
/// line of the window in front and of one that is not, the line between
/// windows, and the `~` past the last line.
const CURSOR_LINE: &str = "#3c3836";
use crate::chrome::CHOSEN as STATUS_IN_FRONT;
const STATUS_BEHIND: &str = "#32302f";
use crate::chrome::CHOSEN as SPLIT;
const NON_TEXT: &str = "#665c54";
/// The pane's margins, in points: the status line reaches across them.
const LEFT: f64 = 10.0;
const RIGHT: f64 = 8.0;

/// What is kept of the pane.
#[derive(Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each is a state of its own: shown, in front, asking, all unfolded, after Ctrl-W"
)]
pub(super) struct AiPane {
    pub(super) shown: bool,
    /// Whether keys go to the pane, not the page.
    focused: bool,
    /// The command line was opened from the pane (`i`): when it closes,
    /// the pane is in front again, as Vim goes back to the window a
    /// command was typed in.
    asking: bool,
    /// `zR`: every thread unfolded, not only the one the cursor is on.
    unfolded: bool,
    chosen: usize,
    /// The first key of two (`g`, `z`), waiting for the second.
    pending: Option<char>,
    /// `Ctrl-W` was pressed: the next key says which pane.
    window_key: bool,
}

/// A thread of the pane: what was asked, and what came of it.
pub(super) struct Thread {
    /// Markup for its first line: what it is, and its state at the right.
    pub(super) head: String,
    /// Markup for the lines under it, shown when it is unfolded.
    pub(super) body: Vec<String>,
    /// Whether it is still open: something to wait for, or to decide.
    pub(super) open: bool,
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

/// One line of the pane, as markup: `content` across its width, on the
/// cursor's line colour where it is that line.
fn line(content: &str, cursor: bool) -> String {
    if cursor {
        format!(
            "#block(width: 100%, fill: rgb(\"{CURSOR_LINE}\"), outset: (left: {LEFT}pt, right: {RIGHT}pt, y: 1.5pt))[{content}]\n"
        )
    } else {
        format!("#block(width: 100%)[{content}]\n")
    }
}

/// A line under a thread's first one, indented as Vim indents a fold's
/// insides.
fn under(content: &str) -> String {
    format!("#block(width: 100%, inset: (left: 1.2em))[{content}]\n")
}

/// A heading of the pane: `title` and how many are under it.
fn heading(title: &str, count: usize) -> String {
    format!(
        "#block(width: 100%, above: 0.9em)[#text(weight: \"bold\", fill: rgb(\"{ACCENT}\"), {}) {}]\n",
        literal(title),
        coloured(FAINT, &format!("({count})"))
    )
}

/// The first line of a thread: its mark, what it is, and its state at
/// the right.
fn head(mark: &str, title: &str, state: &str) -> String {
    format!(
        "{} {} #h(1fr) {state}",
        coloured(FAINT, mark),
        coloured(TEXT, &cut(title, 60))
    )
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

    /// Whether the pane is the window in front: it has the keys, or the
    /// command line was opened from it.
    fn ai_in_front(&self) -> bool {
        self.ai.shown && (self.ai.focused || self.ai.asking)
    }

    /// `:ai`: shows the pane, in front, or puts it away; `:ai <words>`
    /// asks them.
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
            self.ai.asking = false;
            self.arrange();
            if self.ai.shown {
                self.notice(
                    "the AI pane: i asks, j and k go through the threads, Enter goes to the text, \
                     Ctrl-W h back to the page",
                );
            }
        } else {
            self.ai_send(rest);
            if std::mem::take(&mut self.ai.asking) {
                self.ai.focused = self.ai.shown;
            }
        }
        true
    }

    /// Sends `typed` as a question or a request.
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

    /// `i` in the pane: the command line, with `:ai ` on it.
    fn ai_ask(&mut self) {
        self.ai.focused = false;
        self.plain_key(KeyPress::plain(Key::Char(':')));
        self.ai.asking = true;
        for c in "ai ".chars() {
            self.plain_key(KeyPress::plain(Key::Char(c)));
        }
    }

    /// A key, where the pane has it: `Ctrl-W` and what follows it, and
    /// every key while the pane is in front. Returns whether it was the
    /// pane's.
    pub(super) fn ai_pane_key(&mut self, press: KeyPress) -> bool {
        if !self.ai.shown || self.picker.is_some() {
            self.ai.window_key = false;
            return false;
        }
        if self.ai.asking
            && self
                .editor
                .as_ref()
                .is_none_or(|editor| editor.command_line().is_none())
        {
            // The command line opened from the pane was left: back to it.
            self.ai.asking = false;
            self.ai.focused = true;
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
                Key::Char('c' | 'q') => self.ai_close(),
                _ => {}
            }
            return true;
        }
        let window_key = press.ctrl && press.key == Key::Char('w');
        if window_key && (normal || self.ai.focused) {
            self.ai.window_key = true;
            return true;
        }
        if !self.ai.focused {
            return false;
        }
        let threads = self.ai_threads().len();
        let last = threads.saturating_sub(1);
        if let Some(first) = self.ai.pending.take() {
            match (first, press.key) {
                ('g', Key::Char('g')) => self.ai.chosen = 0,
                ('g', Key::Char('I')) => self.ai_close(),
                ('z', Key::Char('R' | 'o' | 'O')) => self.ai.unfolded = true,
                ('z', Key::Char('M' | 'c' | 'C')) => self.ai.unfolded = false,
                ('z', Key::Char('a' | 'A')) => self.ai.unfolded = !self.ai.unfolded,
                _ => {}
            }
            return true;
        }
        match press.key {
            Key::Char('j') | Key::Down => self.ai.chosen = (self.ai.chosen + 1).min(last),
            Key::Char('k') | Key::Up => self.ai.chosen = self.ai.chosen.saturating_sub(1),
            Key::Char(c @ ('g' | 'z')) if !press.ctrl => self.ai.pending = Some(c),
            Key::Char('G') => self.ai.chosen = last,
            Key::Char('i' | 'o') if !press.ctrl => self.ai_ask(),
            Key::Char('a') => self.ai_decide(true),
            Key::Char('r') => self.ai_decide(false),
            Key::Enter => self.ai_go(),
            Key::Escape => self.ai.focused = false,
            Key::Char('q') => self.ai_close(),
            Key::Char(':') => {
                // The command line is the page's.
                self.ai.focused = false;
                return false;
            }
            _ => {}
        }
        true
    }

    /// Puts the pane away: the page has the window again.
    fn ai_close(&mut self) {
        self.ai.shown = false;
        self.ai.focused = false;
        self.ai.asking = false;
        self.arrange();
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
        let mut threads: Vec<(u64, Thread)> = Vec::new();
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
            let waits = made.iter().any(|s| s.state == State::Open);
            let state = if waits {
                coloured(ACCENT, "waits for you")
            } else if note.resolved {
                coloured(FAINT, "resolved")
            } else if answered {
                coloured(GOOD, "answered")
            } else {
                coloured(ACCENT, "waiting for an agent")
            };
            let mut body = Vec::new();
            if note.text.chars().count() > 60 {
                body.push(coloured(TEXT, &note.text));
            }
            push_quote(&mut body, &note.quote);
            for (by, said) in &note.answers {
                let (who, colour) = if *by == Author::Agent {
                    ("agent ", GOOD)
                } else {
                    ("you   ", ACCENT)
                };
                body.push(format!(
                    "{} {}",
                    coloured(colour, who),
                    coloured(TEXT, said)
                ));
            }
            push_suggestions(&mut body, &made);
            threads.push((
                note.id,
                Thread {
                    head: head("?", &note.text, &state),
                    body,
                    open: !note.resolved && (!answered || waits),
                    at: (note.file == file).then_some((note.file, note.range.start)),
                    waiting: waits.then(|| format!("note {}", note.id)),
                },
            ));
        }
        threads.extend(self.request_threads(file));
        threads
            .sort_by_key(|(id, thread)| (std::cmp::Reverse(thread.open), std::cmp::Reverse(*id)));
        threads.into_iter().map(|(_, thread)| thread).collect()
    }

    /// The threads of the review requests, with their numbers.
    fn request_threads(&self, file: FileId) -> Vec<(u64, Thread)> {
        let suggestions = self.suggestions();
        let mut threads = Vec::new();
        for request in self.review_requests() {
            let made: Vec<_> = suggestions
                .iter()
                .filter(|s| s.request == Some(request.id))
                .collect();
            let waits = made.iter().any(|s| s.state == State::Open);
            let state = if waits {
                coloured(ACCENT, "waits for you")
            } else if request.done {
                coloured(GOOD, "done")
            } else {
                coloured(ACCENT, "waiting for an agent")
            };
            let section = if request.section.is_empty() {
                String::new()
            } else {
                format!(" of {}", request.section)
            };
            let mut body = Vec::new();
            if !request.words.is_empty() {
                body.push(coloured(TEXT, &request.words));
            }
            push_quote(&mut body, &request.quote);
            push_suggestions(&mut body, &made);
            threads.push((
                request.id,
                Thread {
                    head: head("R", &format!("review: {}{section}", request.what), &state),
                    body,
                    open: !request.done || waits,
                    at: (request.file == file).then_some((request.file, request.range.start)),
                    waiting: waits.then(|| format!("request {}", request.id)),
                },
            ));
        }
        threads
    }

    /// The lines on what agents are doing, under their heading.
    fn ai_agents_markup(&self) -> String {
        let shown = self.agents_shown();
        let mut agents = heading("Agents", shown.len());
        if shown.is_empty() {
            let waiting = if self.agent_at_work().is_some() {
                "the agent the window called is starting"
            } else if self.agent_on_call() {
                "none at work; asking calls the agent on call"
            } else {
                "none at work; [ai] agent in the settings lets the window call one"
            };
            agents.push_str(&line(&coloured(FAINT, waiting), false));
        }
        for agent in &shown {
            let (mark, colour) = if agent.gone {
                ("○", FAINT)
            } else {
                ("●", GOOD)
            };
            agents.push_str(&line(
                &format!(
                    "{} {} #h(1fr) {}",
                    coloured(colour, mark),
                    coloured(TEXT, &agent.name),
                    coloured(FAINT, &agent.state)
                ),
                false,
            ));
            if let Some((message, done, total)) = &agent.said {
                let progress = match (done, total) {
                    (Some(done), Some(total)) => {
                        format!(" #h(1fr) {}", coloured(ACCENT, &bar(*done, *total)))
                    }
                    _ => String::new(),
                };
                agents.push_str(&under(&format!(
                    "{}{progress}",
                    coloured(TEXT, &cut(message, 80))
                )));
            }
            for step in &agent.steps {
                agents.push_str(&under(&coloured(FAINT, step)));
            }
        }
        if self.agents_paused() {
            agents.push_str(&line(
                &coloured(BAD, "paused: they change nothing until :agents resume"),
                false,
            ));
        }
        agents
    }

    /// The markup of the pane, `width` by `height` points.
    fn ai_markup(&self, width: f64, height: f64) -> String {
        let threads = self.ai_threads();
        let in_front = self.ai_in_front();
        let chosen = self.ai.chosen.min(threads.len().saturating_sub(1));
        let open = threads.iter().filter(|thread| thread.open).count();
        let waits = threads.iter().filter(|t| t.waiting.is_some()).count();
        let top = chosen.saturating_sub(ABOVE);
        let mut list = String::new();
        if threads.is_empty() {
            list.push_str(&heading("Threads", 0));
            list.push_str(&line(
                &coloured(
                    FAINT,
                    "nothing asked yet: i asks about the paragraph here or what is selected",
                ),
                false,
            ));
        }
        for (index, thread) in threads.iter().enumerate() {
            if index < top {
                continue;
            }
            if index == 0 && thread.open {
                list.push_str(&heading("Open", open));
            }
            if index == open {
                list.push_str(&heading("Done", threads.len() - open));
            }
            let cursor = index == chosen;
            list.push_str(&line(&thread.head, cursor && in_front));
            if cursor || self.ai.unfolded {
                for body in &thread.body {
                    list.push_str(&under(body));
                }
            }
        }
        let (status_fill, status_ink) = if in_front {
            (STATUS_IN_FRONT, TEXT)
        } else {
            (STATUS_BEHIND, FAINT)
        };
        let mut said = format!("{open} open");
        if waits > 0 {
            let _ = write!(said, " · {waits} wait for you");
        }
        if self.agents_paused() {
            said.push_str(" · agents paused");
        }
        let place = if threads.is_empty() {
            String::new()
        } else {
            format!("{}/{}", chosen + 1, threads.len())
        };
        let agents = self.ai_agents_markup();
        format!(
            r#"#set page(width: {width}pt, height: {height}pt, margin: (left: {LEFT}pt, right: {RIGHT}pt, top: 4pt, bottom: 17pt), fill: rgb("{BACKGROUND}"),
  background: place(left, rect(width: 1pt, height: 100%, fill: rgb("{SPLIT}"))),
  footer-descent: 3pt,
  footer: block(width: 100%, outset: (left: {LEFT}pt - 1pt, right: {RIGHT}pt), inset: (y: 3pt), fill: rgb("{status_fill}"),
    text(fill: rgb("{status_ink}"))[#text(weight: "bold", "ai") #h(8pt) #{said} #h(1fr) #{place}]))
#set text(font: {FONT}, size: 9pt, fill: rgb("{TEXT}"), ligatures: false, hyphenate: false)
#set par(leading: 0.45em, justify: false, spacing: 0.45em)
#set block(spacing: 0.45em)
{agents}{list}#block(width: 100%, height: 1fr, clip: true, above: 0.45em, text(fill: rgb("{NON_TEXT}"), for _ in range(160) {{ "~"; linebreak() }}))
"#,
            said = literal(&said),
            place = literal(&place),
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

/// The text a thread is about, as it reads, if it reads as anything.
fn push_quote(lines: &mut Vec<String>, quote: &str) {
    let quote = cut(&readable(quote), QUOTED);
    if !quote.is_empty() {
        lines.push(coloured(FAINT, &format!("“{quote}”")));
    }
}

/// The suggestions `made` for a thread, as lines under it: each with its
/// kind, what became of it, and its change.
fn push_suggestions(lines: &mut Vec<String>, made: &[&super::review::Suggestion]) {
    for suggestion in made {
        let (word, colour) = match suggestion.state {
            State::Open => ("a accepts, r rejects", ACCENT),
            State::Later => ("put aside", FAINT),
            State::Accepted => ("accepted", GOOD),
            State::Rewritten => ("rewritten by you", GOOD),
            State::Rejected => ("rejected", BAD),
            State::Withdrawn => ("withdrawn", FAINT),
        };
        let category = if suggestion.category.is_empty() {
            "suggestion"
        } else {
            &suggestion.category
        };
        lines.push(format!(
            "{} {} #h(1fr) {}",
            coloured(FAINT, "▸"),
            coloured(TEXT, category),
            coloured(colour, word)
        ));
        if !suggestion.reason.is_empty() {
            lines.push(format!(
                "#h(1.2em){}",
                coloured(FAINT, &cut(&suggestion.reason, 120))
            ));
        }
        if !suggestion.quote.is_empty() {
            lines.push(format!(
                "#h(1.2em){}",
                coloured(
                    BAD,
                    &format!("- {}", cut(&readable(&suggestion.quote), CHANGED))
                )
            ));
        }
        if !suggestion.new.is_empty() {
            lines.push(format!(
                "#h(1.2em){}",
                coloured(
                    GOOD,
                    &format!("+ {}", cut(&readable(&suggestion.new), CHANGED))
                )
            ));
        }
    }
}
