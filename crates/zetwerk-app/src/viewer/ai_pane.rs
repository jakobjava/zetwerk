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
//! It is a window of its own, as a split is in Vim, and a buffer one can
//! only read: lines of text, wrapped to its width here (not by Typst),
//! so that each line on the screen is one the cursor can be on. Threads
//! are folded to their first line but for the one the cursor is in; `~`
//! stands past the last line, and a status line of its own at its foot.
//! Nothing is typed into the pane itself: `i` (or `o`) opens the command
//! line with `:ai ` on it, and what is typed there is asked of agents
//! about what is selected, or the paragraph of the cursor (`:ask`), or
//! the place of the cursor on an empty line; `:ai /review clarity` (or
//! any kind of review, with words after a comma) asks for a review of it
//! (`:review this`); `/cite`, `/explain`, `/draft` and `/check maths` ask
//! for those. Sending calls the agent on call, if one is named.
//!
//! Keys, with the pane in front (`Ctrl-W l`, or as it opens; `Ctrl-W h`
//! goes back to the page): `j` `k` `h` `l` `w` `b` `0` `^` `$` `gg` `G`
//! `Ctrl-D` `Ctrl-U` move as in Vim, `]]` and `[[` (or `}` and `{`) to
//! the next and the last thread; `v` and `V` select, `y` copies what is
//! selected (`yy` and `Y` the line) into the unnamed register and the
//! clipboard; Enter goes to the text a thread is about, `a` accepts what
//! waits in it and `r` rejects it; `zR` unfolds every thread and `zM`
//! folds them again (`za` switches); Escape leaves a selection, then
//! goes back to the page; `q` puts the pane away.
//!
//! The pane is typeset as the other panels are, and only when what it
//! shows changes: nothing is asked of the machine while nothing happens.

use std::fmt::Write as _;

use typst::syntax::FileId;

use super::notes::Author;
use super::review::State;
use super::{PANE, Quad, Scene, TextureId, Viewer, solid};
use crate::chrome::{FONT, typst_string as literal};
use crate::keys::{Key, KeyPress};

/// How much of the window the pane takes, and at least, in pixels.
const SHARE: f64 = 0.38;
const NARROWEST: u32 = 300;
/// How many characters of the text a thread is about are shown.
const QUOTED: usize = 200;
/// How many characters of a suggestion's old and new text are shown.
const CHANGED: usize = 400;
/// The size of the pane's text, and the height of a line, in points.
const SIZE: f64 = 9.0;
const ROW: f64 = 12.5;
/// How many lines stay in view above and below the cursor.
const SCROLL_OFF: usize = 2;

use crate::chrome::{
    ACCENT, ADDED as GOOD, ALERT as BAD, BACKGROUND, CHOSEN as STATUS_IN_FRONT, DIM as FAINT,
    FOREGROUND as TEXT,
};
/// Vim's colours, as gruvbox has them: the cursor's line, the status
/// line of a window not in front, the line between windows, what is
/// selected, and the `~` past the last line.
const CURSOR_LINE: &str = "#3c3836";
const STATUS_BEHIND: &str = "#32302f";
const SPLIT: &str = "#504945";
const VISUAL: &str = "#665c54";
const NON_TEXT: &str = "#665c54";
/// The pane's margins, in points: the status line and the cursor's line
/// reach across them.
const LEFT: f64 = 10.0;
const RIGHT: f64 = 8.0;
const TOP: f64 = 4.0;
const BOTTOM: f64 = 18.0;

/// A stretch of a line in one colour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Span {
    pub(super) text: String,
    colour: &'static str,
    bold: bool,
}

/// `text` in `colour`.
fn span(colour: &'static str, text: &str) -> Span {
    Span {
        text: text.replace('\t', "    "),
        colour,
        bold: false,
    }
}

/// A line of the pane before it is wrapped: its stretches, and how far
/// it and the lines it is wrapped into are indented.
#[derive(Debug, Clone)]
pub(super) struct Line {
    pub(super) spans: Vec<Span>,
    indent: usize,
}

impl Line {
    fn new(indent: usize, spans: Vec<Span>) -> Self {
        Self { spans, indent }
    }

    /// Its text, without its colours.
    #[cfg(test)]
    pub(super) fn plain(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }
}

/// `text` as lines of `colour`, one for each of its own lines, indented
/// by `indent`.
fn lines(indent: usize, colour: &'static str, text: &str) -> Vec<Line> {
    text.trim_end()
        .lines()
        .map(|line| Line::new(indent, vec![span(colour, line.trim_end())]))
        .collect()
}

/// A line on the screen: what it shows, where it belongs, and what
/// stands at its right.
#[derive(Debug, Clone)]
struct Row {
    /// Which piece of the pane it is (`agents`, a heading, a thread's
    /// key) and which of its lines: where the cursor stays when threads
    /// unfold and fold around it.
    key: (String, usize),
    /// The thread it is a line of.
    thread: Option<usize>,
    /// Its characters, with their colours, indent included.
    cells: Vec<(char, &'static str, bool)>,
    /// Shown at its right, not a place for the cursor.
    right: Option<Span>,
    /// It goes on the line before it, wrapped: copied, it is joined to it.
    wrapped: bool,
    /// How far it is indented, in characters.
    indent: usize,
}

impl Row {
    fn text(&self) -> String {
        self.cells.iter().map(|(c, _, _)| c).collect()
    }
}

/// A selection begun with `v` or `V`: where it began.
#[derive(Debug, Clone)]
struct Visual {
    key: (String, usize),
    column: usize,
    lines: bool,
}

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
    /// `zR`: every thread unfolded, not only the one the cursor is in.
    unfolded: bool,
    /// The line of the cursor (see [`Row::key`]); none for the first
    /// thread's first line.
    cursor: Option<(String, usize)>,
    column: usize,
    visual: Option<Visual>,
    /// The first line shown.
    top: usize,
    /// The first key of two (`g`, `z`, `y`, `]`, `[`), waiting for the
    /// second.
    pending: Option<char>,
    /// `Ctrl-W` was pressed: the next key says which pane.
    window_key: bool,
    /// How many characters a line has room for, and how many lines are
    /// shown, as last drawn.
    columns: usize,
    height: usize,
}

/// A thread of the pane: what was asked, and what came of it.
pub(super) struct Thread {
    /// What names it while it lasts: `n3` for note 3, `r2` for request 2.
    key: String,
    /// `?` for a question, `R` for a review asked for.
    mark: &'static str,
    /// Its first line: what was asked, in short.
    pub(super) title: String,
    /// What became of it, at the right of its first line.
    pub(super) state: Span,
    /// The lines under its first one, shown when it is unfolded.
    pub(super) body: Vec<Line>,
    /// Whether it is still open: something to wait for, or to decide.
    pub(super) open: bool,
    /// Where the text it is about is.
    at: Option<(FileId, usize)>,
    /// What `a` and `r` take: `note 3`, `request 2`, if anything waits.
    waiting: Option<String>,
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
    let text = without_calls(&lines.join(" "));
    let text: String = text
        .chars()
        .filter(|c| !matches!(c, '*' | '_' | '`' | '$' | '[' | ']'))
        .collect();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    text.trim_start_matches(['=', '-', '+', ' '])
        .trim()
        .to_owned()
}

/// `text` without the calls of code in it (`#h(2em)`, `#v(1em)`): a name
/// after `#`, and its arguments in parentheses. What a call has in
/// brackets is text, and stays.
fn without_calls(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '#' {
            kept.push(c);
            continue;
        }
        while chars
            .peek()
            .is_some_and(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            chars.next();
        }
        if chars.peek() == Some(&'(') {
            let mut depth = 0usize;
            for c in chars.by_ref() {
                match c {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                if depth == 0 {
                    break;
                }
            }
        }
    }
    kept
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

/// `cells` broken into lines of at most `columns`, at blanks where it
/// can be: the first as it is, the others indented by `indent` again.
fn wrap(
    cells: Vec<(char, &'static str, bool)>,
    indent: usize,
    columns: usize,
) -> Vec<Vec<(char, &'static str, bool)>> {
    let columns = columns.max(indent + 12);
    let mut rows = Vec::new();
    let mut rest = cells;
    while rest.len() > columns {
        let room = &rest[..=columns];
        let at = room
            .iter()
            .rposition(|(c, _, _)| *c == ' ')
            .filter(|&at| at > indent)
            .unwrap_or(columns);
        let mut next: Vec<_> = rest.split_off(at);
        while next.first().is_some_and(|(c, _, _)| *c == ' ') {
            next.remove(0);
        }
        rows.push(rest);
        let mut indented = vec![(' ', TEXT, false); indent];
        indented.extend(next);
        rest = indented;
    }
    rows.push(rest);
    rows
}

/// The cells of `line`, its indent first.
fn cells(line: &Line) -> Vec<(char, &'static str, bool)> {
    let mut cells = vec![(' ', TEXT, false); line.indent];
    for span in &line.spans {
        cells.extend(span.text.chars().map(|c| (c, span.colour, span.bold)));
    }
    cells
}

/// Puts `line` into `rows` as one or more lines, the lines of `key`
/// counted on from `next`.
fn push_rows(
    rows: &mut Vec<Row>,
    key: &str,
    next: &mut usize,
    thread: Option<usize>,
    line: &Line,
    columns: usize,
) {
    for (index, cells) in wrap(cells(line), line.indent, columns)
        .into_iter()
        .enumerate()
    {
        rows.push(Row {
            key: (key.to_owned(), *next),
            thread,
            cells,
            right: None,
            wrapped: index > 0,
            indent: line.indent,
        });
        *next += 1;
    }
}

/// A heading of the pane, `title (count)`, after an empty line.
fn push_heading(rows: &mut Vec<Row>, key: &str, title: &str, count: usize) {
    if !rows.is_empty() {
        rows.push(Row {
            key: (key.to_owned(), 0),
            thread: None,
            cells: Vec::new(),
            right: None,
            wrapped: false,
            indent: 0,
        });
    }
    let mut cells: Vec<_> = title.chars().map(|c| (c, ACCENT, true)).collect();
    cells.extend(format!(" ({count})").chars().map(|c| (c, FAINT, false)));
    rows.push(Row {
        key: (key.to_owned(), 1),
        thread: None,
        cells,
        right: None,
        wrapped: false,
        indent: 0,
    });
}

/// The first line of `thread`: its mark and title, cut to leave room
/// for its state at the right.
fn head_row(thread: &Thread, index: usize, columns: usize) -> Row {
    let room = columns.saturating_sub(thread.state.text.chars().count() + 4);
    let mut cells = vec![(thread.mark.chars().next().unwrap_or('?'), FAINT, false)];
    cells.push((' ', TEXT, false));
    cells.extend(cut(&thread.title, room).chars().map(|c| (c, TEXT, false)));
    Row {
        key: (thread.key.clone(), 0),
        thread: Some(index),
        cells,
        right: Some(thread.state.clone()),
        wrapped: false,
        indent: 0,
    }
}

/// The text of `rows` from `from` (line, column) to `to`, both in, as
/// it is copied: lines wrapped joined again, the pane's indent left out;
/// whole lines with `lines`.
fn copied(rows: &[Row], from: (usize, usize), to: (usize, usize), lines: bool) -> String {
    let mut text = String::new();
    for (index, row) in rows.iter().enumerate().take(to.0 + 1).skip(from.0) {
        let chars: Vec<char> = row.text().chars().collect();
        let start = if lines || index != from.0 {
            row.indent.min(chars.len())
        } else {
            from.1.min(chars.len())
        };
        let end = if lines || index != to.0 {
            chars.len()
        } else {
            (to.1 + 1).min(chars.len())
        };
        if index != from.0 {
            text.push(if row.wrapped { ' ' } else { '\n' });
        }
        text.extend(&chars[start.min(end)..end]);
    }
    if lines {
        text.push('\n');
    }
    text
}

/// Where the next word begins after column `at` of `text`, or the last
/// one before it.
fn word(text: &[char], at: usize, forward: bool) -> usize {
    let blank = |i: usize| text.get(i).is_none_or(|c| c.is_whitespace());
    if forward {
        let mut i = at;
        while i < text.len() && !blank(i) {
            i += 1;
        }
        while i < text.len() && blank(i) {
            i += 1;
        }
        i.min(text.len().saturating_sub(1))
    } else {
        let mut i = at;
        while i > 0 && blank(i - 1) {
            i -= 1;
        }
        while i > 0 && !blank(i - 1) {
            i -= 1;
        }
        i
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
                    "the AI pane: i asks, j and k move, v and V select, y copies, Enter goes to \
                     the text, Ctrl-W h back to the page",
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
        // To what was just asked: the first thread.
        self.ai.cursor = None;
        self.ai.column = 0;
        self.ai.visual = None;
    }

    /// `i` in the pane: the command line, with `:ai ` on it.
    fn ai_ask(&mut self) {
        self.ai.focused = false;
        self.ai.visual = None;
        self.plain_key(KeyPress::plain(Key::Char(':')));
        self.ai.asking = true;
        for c in "ai ".chars() {
            self.plain_key(KeyPress::plain(Key::Char(c)));
        }
    }

    /// The threads and the lines of the pane as they are now, and the
    /// line of the cursor.
    fn ai_layout(&self) -> (Vec<Thread>, Vec<Row>, usize) {
        let threads = self.ai_threads();
        let columns = if self.ai.columns == 0 {
            60
        } else {
            self.ai.columns
        };
        // The thread the cursor is in (or a selection began in) unfolds.
        let held = self
            .ai
            .visual
            .as_ref()
            .map(|visual| &visual.key)
            .or(self.ai.cursor.as_ref());
        let open_one = match held {
            Some(key) => threads.iter().position(|thread| thread.key == key.0),
            None => (!threads.is_empty()).then_some(0),
        };
        let mut rows = self.ai_agent_rows(columns);
        let open = threads.iter().filter(|thread| thread.open).count();
        if threads.is_empty() {
            push_heading(&mut rows, "threads", "Threads", 0);
            let hint = Line::new(
                0,
                vec![span(
                    FAINT,
                    "nothing asked yet: i asks about the paragraph here or what is selected",
                )],
            );
            push_rows(&mut rows, "threads", &mut 2, None, &hint, columns);
        }
        for (index, thread) in threads.iter().enumerate() {
            if index == 0 && thread.open {
                push_heading(&mut rows, "open", "Open", open);
            }
            if index == open {
                push_heading(&mut rows, "done", "Done", threads.len() - open);
            }
            rows.push(head_row(thread, index, columns));
            if self.ai.unfolded || open_one == Some(index) {
                let mut next = 1;
                for line in &thread.body {
                    push_rows(
                        &mut rows,
                        &thread.key,
                        &mut next,
                        Some(index),
                        line,
                        columns,
                    );
                }
            }
        }
        let cursor = match &self.ai.cursor {
            None => rows.iter().position(|row| row.thread.is_some()),
            Some(key) => rows
                .iter()
                .position(|row| &row.key == key)
                .or_else(|| {
                    rows.iter()
                        .rposition(|row| row.key.0 == key.0 && row.key.1 <= key.1)
                })
                .or_else(|| rows.iter().position(|row| row.thread.is_some())),
        };
        (threads, rows, cursor.unwrap_or(0))
    }

    /// The lines on what agents are doing, under their heading.
    fn ai_agent_rows(&self, columns: usize) -> Vec<Row> {
        let shown = self.agents_shown();
        let mut rows = Vec::new();
        push_heading(&mut rows, "agents", "Agents", shown.len());
        let mut next = 2;
        let mut push = |rows: &mut Vec<Row>, line: Line, right: Option<Span>| {
            push_rows(rows, "agents", &mut next, None, &line, columns);
            if let (Some(right), Some(row)) = (right, rows.last_mut()) {
                row.right = Some(right);
            }
        };
        if shown.is_empty() {
            let waiting = if self.agent_at_work().is_some() {
                "the agent the window called is starting"
            } else if self.agent_on_call() {
                "none at work; asking calls the agent on call"
            } else {
                "none at work; [ai] agent in the settings lets the window call one"
            };
            push(&mut rows, Line::new(0, vec![span(FAINT, waiting)]), None);
        }
        for agent in &shown {
            let (mark, colour) = if agent.gone {
                ("○ ", FAINT)
            } else {
                ("● ", GOOD)
            };
            push(
                &mut rows,
                Line::new(0, vec![span(colour, mark), span(TEXT, &agent.name)]),
                Some(span(FAINT, &agent.state)),
            );
            if let Some((message, done, total)) = &agent.said {
                let progress = match (done, total) {
                    (Some(done), Some(total)) => Some(span(ACCENT, &bar(*done, *total))),
                    _ => None,
                };
                push(
                    &mut rows,
                    Line::new(2, vec![span(TEXT, &cut(message, 200))]),
                    progress,
                );
            }
            for step in &agent.steps {
                push(&mut rows, Line::new(2, vec![span(FAINT, step)]), None);
            }
        }
        if self.agents_paused() {
            push(
                &mut rows,
                Line::new(
                    0,
                    vec![span(
                        BAD,
                        "paused: they change nothing until :agents resume",
                    )],
                ),
                None,
            );
        }
        rows
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
        if press.ctrl && press.key == Key::Char('w') && (normal || self.ai.focused) {
            self.ai.window_key = true;
            return true;
        }
        if !self.ai.focused {
            return false;
        }
        if let Some(first) = self.ai.pending.take() {
            self.ai_second_key(first, press);
            return true;
        }
        let (_, rows, at) = self.ai_layout();
        let last = rows.len().saturating_sub(1);
        let width = rows.get(at).map_or(0, |row| row.cells.len());
        let half = (self.ai.height / 2).max(1);
        match press.key {
            Key::Char('d') if press.ctrl => self.ai_to_row(&rows, (at + half).min(last)),
            Key::Char('u') if press.ctrl => self.ai_to_row(&rows, at.saturating_sub(half)),
            _ if press.ctrl || press.alt => {}
            Key::Char('j') | Key::Down => self.ai_to_row(&rows, (at + 1).min(last)),
            Key::Char('k') | Key::Up => self.ai_to_row(&rows, at.saturating_sub(1)),
            Key::Char('G') => self.ai_to_row(&rows, last),
            Key::Char('}') => self.ai_to_thread(&rows, at, true),
            Key::Char('{') => self.ai_to_thread(&rows, at, false),
            Key::Char('h') | Key::Left => self.ai.column = self.ai.column.saturating_sub(1),
            Key::Char('l') | Key::Right => {
                self.ai.column = (self.ai.column + 1).min(width.saturating_sub(1));
            }
            Key::Char('0') => self.ai.column = 0,
            Key::Char('^') => self.ai.column = rows.get(at).map_or(0, |row| row.indent),
            Key::Char('$') => self.ai.column = width.saturating_sub(1),
            Key::Char(c @ ('w' | 'b')) => {
                let text: Vec<char> = rows
                    .get(at)
                    .map(Row::text)
                    .unwrap_or_default()
                    .chars()
                    .collect();
                self.ai.column = word(&text, self.ai.column, c == 'w');
            }
            Key::Char(c @ ('v' | 'V')) => self.ai_select(&rows, at, c == 'V'),
            Key::Char('y') if self.ai.visual.is_some() => self.ai_copy(&rows, at),
            Key::Char('Y') => self.ai_copy_line(&rows, at),
            Key::Char(c @ ('g' | 'z' | 'y' | ']' | '[')) => self.ai.pending = Some(c),
            Key::Char('i' | 'o') => self.ai_ask(),
            Key::Char('a') => self.ai_decide(true),
            Key::Char('r') => self.ai_decide(false),
            Key::Enter => self.ai_go(),
            Key::Escape if self.ai.visual.is_some() => self.ai.visual = None,
            Key::Escape => self.ai.focused = false,
            Key::Char('q') => self.ai_close(),
            Key::Char(':') => {
                // The command line is the page's.
                self.ai.focused = false;
                self.ai.visual = None;
                return false;
            }
            _ => {}
        }
        true
    }

    /// The second key of two: `gg`, `gI`, `yy`, `]]`, `[[`, the folds.
    fn ai_second_key(&mut self, first: char, press: KeyPress) {
        let (_, rows, at) = self.ai_layout();
        match (first, press.key) {
            ('g', Key::Char('g')) => self.ai_to_row(&rows, 0),
            ('g', Key::Char('I')) => self.ai_close(),
            ('y', Key::Char('y')) => self.ai_copy_line(&rows, at),
            (']', Key::Char(']')) => self.ai_to_thread(&rows, at, true),
            ('[', Key::Char('[')) => self.ai_to_thread(&rows, at, false),
            ('z', Key::Char('R' | 'o' | 'O')) => self.ai.unfolded = true,
            ('z', Key::Char('M' | 'c' | 'C')) => self.ai.unfolded = false,
            ('z', Key::Char('a' | 'A')) => self.ai.unfolded = !self.ai.unfolded,
            _ => {}
        }
    }

    /// The cursor to line `index` of `rows`, in the column it was in
    /// where that line is as long.
    fn ai_to_row(&mut self, rows: &[Row], index: usize) {
        if let Some(row) = rows.get(index) {
            self.ai.cursor = Some(row.key.clone());
            self.ai.column = self.ai.column.min(row.cells.len().saturating_sub(1));
        }
    }

    /// The cursor to the first line of the next thread after line `at`,
    /// or of the one before.
    fn ai_to_thread(&mut self, rows: &[Row], at: usize, forward: bool) {
        let first = |row: &Row| row.thread.is_some() && row.key.1 == 0;
        let found = if forward {
            rows.iter()
                .enumerate()
                .skip(at + 1)
                .find(|(_, row)| first(row))
        } else {
            rows.iter()
                .enumerate()
                .take(at)
                .rev()
                .find(|(_, row)| first(row))
        };
        if let Some((index, _)) = found {
            self.ai_to_row(rows, index);
            self.ai.column = 0;
        }
    }

    /// `v`, `V`: a selection begins at the cursor, by character or by
    /// line; the same key again ends it.
    fn ai_select(&mut self, rows: &[Row], at: usize, lines: bool) {
        self.ai.visual = match &self.ai.visual {
            Some(visual) if visual.lines == lines => None,
            _ => Some(Visual {
                key: rows.get(at).map(|row| row.key.clone()).unwrap_or_default(),
                column: self.ai.column,
                lines,
            }),
        };
    }

    /// `yy`, `Y`: the line of the cursor copied.
    fn ai_copy_line(&mut self, rows: &[Row], at: usize) {
        self.ai.visual = None;
        self.ai_select(rows, at, true);
        self.ai_copy(rows, at);
    }

    /// `y`: what is selected, into the unnamed register and the
    /// clipboard; the selection is over.
    fn ai_copy(&mut self, rows: &[Row], at: usize) {
        let Some(visual) = self.ai.visual.take() else {
            return;
        };
        let began = rows
            .iter()
            .position(|row| row.key == visual.key)
            .unwrap_or(at);
        let (from, to) = if (began, visual.column) <= (at, self.ai.column) {
            ((began, visual.column), (at, self.ai.column))
        } else {
            ((at, self.ai.column), (began, visual.column))
        };
        let text = copied(rows, from, to, visual.lines);
        if let Some(editor) = &mut self.editor {
            editor.set_register('"', &text);
            editor.set_register('0', &text);
        }
        // The cursor goes to where the selection began, as in Vim.
        self.ai.cursor = rows.get(from.0).map(|row| row.key.clone());
        self.ai.column = if visual.lines { self.ai.column } else { from.1 };
        let lines = text.trim_end_matches('\n').lines().count();
        let told = match crate::clipboard::write(&text) {
            Ok(()) => format!(
                "copied {lines} line{} (into \" and the clipboard)",
                if lines == 1 { "" } else { "s" }
            ),
            Err(_) => format!(
                "copied {lines} line{} into \"",
                if lines == 1 { "" } else { "s" }
            ),
        };
        self.notice(&told);
    }

    /// Puts the pane away: the page has the window again.
    fn ai_close(&mut self) {
        self.ai.shown = false;
        self.ai.focused = false;
        self.ai.asking = false;
        self.ai.visual = None;
        self.arrange();
    }

    /// The thread the cursor is in.
    fn ai_thread_here(&self) -> Option<Thread> {
        let (threads, rows, at) = self.ai_layout();
        let index = rows.get(at)?.thread?;
        threads.into_iter().nth(index)
    }

    /// Enter on a thread: the cursor to the text it is about, the page in
    /// front.
    fn ai_go(&mut self) {
        let Some((file, at)) = self.ai_thread_here().and_then(|thread| thread.at) else {
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
        self.ai.visual = None;
    }

    /// `a` and `r` on a thread: what waits in it, accepted or rejected.
    fn ai_decide(&mut self, accept: bool) {
        let Some(which) = self.ai_thread_here().and_then(|thread| thread.waiting) else {
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
                span(ACCENT, "waits for you")
            } else if note.resolved {
                span(FAINT, "resolved")
            } else if answered {
                span(GOOD, "answered")
            } else {
                span(ACCENT, "waiting for an agent")
            };
            // The question again in full, where its first line had to cut it.
            let mut body = if note.text.chars().count() > 48 || note.text.contains('\n') {
                lines(2, TEXT, &note.text)
            } else {
                Vec::new()
            };
            push_quote(&mut body, &note.quote);
            for (by, said) in &note.answers {
                let (who, colour) = if *by == Author::Agent {
                    ("agent", GOOD)
                } else {
                    ("you", ACCENT)
                };
                body.push(Line::new(2, vec![span(colour, who)]));
                body.extend(lines(4, TEXT, said));
            }
            push_suggestions(&mut body, &made);
            threads.push((
                note.id,
                Thread {
                    key: format!("n{}", note.id),
                    mark: "?",
                    title: note.text.clone(),
                    state,
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
                span(ACCENT, "waits for you")
            } else if request.done {
                span(GOOD, "done")
            } else {
                span(ACCENT, "waiting for an agent")
            };
            let section = if request.section.is_empty() {
                String::new()
            } else {
                format!(" of {}", request.section)
            };
            let mut body = lines(2, TEXT, &request.words);
            push_quote(&mut body, &request.quote);
            push_suggestions(&mut body, &made);
            threads.push((
                request.id,
                Thread {
                    key: format!("r{}", request.id),
                    mark: "R",
                    title: format!("review: {}{section}", request.what),
                    state,
                    body,
                    open: !request.done || waits,
                    at: (request.file == file).then_some((request.file, request.range.start)),
                    waiting: waits.then(|| format!("request {}", request.id)),
                },
            ));
        }
        threads
    }

    /// The markup of the pane, `width` by `height` points, its
    /// characters `advance` points apart; keeps how many lines it shows
    /// and how long they are, and scrolls to the cursor.
    fn ai_markup(&mut self, width: f64, height: f64, advance: f64) -> String {
        self.ai.columns = (((width - LEFT - RIGHT) / advance).floor() as usize).max(20);
        self.ai.height = (((height - TOP - BOTTOM) / ROW).floor() as usize).max(3);
        let (threads, rows, at) = self.ai_layout();
        let shown = self.ai.height;
        let off = SCROLL_OFF.min(shown / 2);
        if at < self.ai.top + off {
            self.ai.top = at.saturating_sub(off);
        } else if at + off >= self.ai.top + shown {
            self.ai.top = at + off + 1 - shown;
        }
        self.ai.top = self.ai.top.min(rows.len().saturating_sub(1));
        let in_front = self.ai_in_front();
        let cursor_shown = self.ai_focused() && !self.ai.asking;
        let selected = self.ai.visual.as_ref().map(|visual| {
            let began = rows
                .iter()
                .position(|row| row.key == visual.key)
                .unwrap_or(at);
            let (from, to) = if (began, visual.column) <= (at, self.ai.column) {
                ((began, visual.column), (at, self.ai.column))
            } else {
                ((at, self.ai.column), (began, visual.column))
            };
            (from, to, visual.lines)
        });
        let mut list = String::new();
        for (index, row) in rows.iter().enumerate().skip(self.ai.top).take(shown) {
            let selection = selected.and_then(|(from, to, lines)| {
                if index < from.0 || index > to.0 {
                    return None;
                }
                let length = row.cells.len().max(1);
                Some(if lines {
                    0..length
                } else {
                    let start = if index == from.0 { from.1 } else { 0 };
                    let end = if index == to.0 { to.1 + 1 } else { length };
                    start..end.max(start + 1)
                })
            });
            let cursor = (cursor_shown && index == at).then_some(self.ai.column);
            list.push_str(&row_markup(
                row,
                in_front && index == at,
                cursor,
                selection.as_ref(),
            ));
        }
        for _ in rows.len().saturating_sub(self.ai.top)..shown {
            let _ = writeln!(
                list,
                "#block(width: 100%, height: {ROW}pt, text(fill: rgb(\"{NON_TEXT}\"), \"~\"))"
            );
        }
        let (status_fill, status_ink) = if in_front {
            (STATUS_IN_FRONT, TEXT)
        } else {
            (STATUS_BEHIND, FAINT)
        };
        let open = threads.iter().filter(|thread| thread.open).count();
        let waits = threads.iter().filter(|t| t.waiting.is_some()).count();
        let mut said = match &self.ai.visual {
            Some(visual) if visual.lines => "VISUAL LINE   ".to_owned(),
            Some(_) => "VISUAL   ".to_owned(),
            None => String::new(),
        };
        let _ = write!(said, "{open} open");
        if waits > 0 {
            let _ = write!(said, " · {waits} wait for you");
        }
        if self.agents_paused() {
            said.push_str(" · agents paused");
        }
        let place = format!("{}/{}", at + 1, rows.len().max(1));
        format!(
            r#"#set page(width: {width}pt, height: {height}pt, margin: (left: {LEFT}pt, right: {RIGHT}pt, top: {TOP}pt, bottom: {BOTTOM}pt), fill: rgb("{BACKGROUND}"),
  background: place(left, rect(width: 1pt, height: 100%, fill: rgb("{SPLIT}"))),
  footer-descent: 3pt,
  footer: block(width: 100%, outset: (left: {LEFT}pt - 1pt, right: {RIGHT}pt), inset: (y: 3pt), fill: rgb("{status_fill}"),
    text(fill: rgb("{status_ink}"))[#text(weight: "bold", "ai") #h(8pt) #{said} #h(1fr) #{place}]))
#set text(font: {FONT}, size: {SIZE}pt, fill: rgb("{TEXT}"), ligatures: false, kerning: false, hyphenate: false, top-edge: "ascender", bottom-edge: "descender")
#set par(justify: false, spacing: 0pt, leading: 0pt)
#set block(spacing: 0pt)
{list}"#,
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
        // The pane's characters are as far apart as the source pane's,
        // at its size: one font, one advance for all of them.
        let advance = self.chrome.cell_width(scale) / scale * SIZE / crate::chrome::SOURCE_SIZE;
        let markup = self.ai_markup(f64::from(width) / scale, height / scale, advance);
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

/// The markup of one line of the pane: on the cursor's line colour where
/// it is that line, with the cursor at `cursor` and what is selected set
/// off.
fn row_markup(
    row: &Row,
    cursor_line: bool,
    cursor: Option<usize>,
    selection: Option<&std::ops::Range<usize>>,
) -> String {
    let mut cells = row.cells.clone();
    let reach = cursor
        .map(|at| at + 1)
        .into_iter()
        .chain(selection.map(|range| range.end))
        .max()
        .unwrap_or(0);
    while cells.len() < reach {
        cells.push((' ', TEXT, false));
    }
    // Runs of cells of one look: colour, weight, and what is behind them.
    let mut runs: Vec<(String, &str, bool, Option<&str>)> = Vec::new();
    for (index, (c, colour, bold)) in cells.into_iter().enumerate() {
        let (ink, ground) = if cursor == Some(index) {
            (BACKGROUND, Some(TEXT))
        } else if selection
            .as_ref()
            .is_some_and(|range| range.contains(&index))
        {
            (colour, Some(VISUAL))
        } else {
            (colour, None)
        };
        match runs.last_mut() {
            Some((text, i, b, g)) if *i == ink && *b == bold && *g == ground => text.push(c),
            _ => runs.push((c.to_string(), ink, bold, ground)),
        }
    }
    let mut content = String::new();
    for (text, ink, bold, ground) in runs {
        let weight = if bold { ", weight: \"bold\"" } else { "" };
        let set = format!("text(fill: rgb(\"{ink}\"){weight}, {})", literal(&text));
        let _ = match ground {
            Some(ground) => write!(
                content,
                "#highlight(fill: rgb(\"{ground}\"), top-edge: \"ascender\", bottom-edge: \"descender\", {set})"
            ),
            None => write!(content, "#{set}"),
        };
    }
    if let Some(right) = &row.right {
        let _ = write!(
            content,
            "#h(1fr)#text(fill: rgb(\"{}\"), {})",
            right.colour,
            literal(&right.text)
        );
    }
    let fill = if cursor_line {
        format!(", fill: rgb(\"{CURSOR_LINE}\"), outset: (left: {LEFT}pt - 1pt, right: {RIGHT}pt)")
    } else {
        String::new()
    };
    format!("#block(width: 100%, height: {ROW}pt, clip: true{fill}, align(horizon)[{content}])\n")
}

/// The text a thread is about, as it reads, if it reads as anything.
fn push_quote(body: &mut Vec<Line>, quote: &str) {
    let quote = cut(&readable(quote), QUOTED);
    if !quote.is_empty() {
        body.push(Line::new(2, vec![span(FAINT, &format!("“{quote}”"))]));
    }
}

/// The suggestions `made` for a thread, as lines under it: each with its
/// kind, what became of it, and its change.
fn push_suggestions(body: &mut Vec<Line>, made: &[&super::review::Suggestion]) {
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
            "suggestion"
        } else {
            &suggestion.category
        };
        body.push(Line::new(
            2,
            vec![
                span(FAINT, "▸ "),
                span(TEXT, category),
                span(FAINT, " · "),
                span(colour, word),
            ],
        ));
        if !suggestion.reason.is_empty() {
            body.extend(lines(4, FAINT, &suggestion.reason));
        }
        let old = cut(&readable(&suggestion.quote), CHANGED);
        if !old.is_empty() {
            body.push(Line::new(4, vec![span(BAD, &format!("- {old}"))]));
        }
        for line in suggestion.new.trim_end().lines().take(40) {
            body.push(Line::new(4, vec![span(GOOD, &format!("+ {line}"))]));
        }
    }
}
