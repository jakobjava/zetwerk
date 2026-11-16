//! The manager of suggestions: all of them in one list, beside the page.
//!
//! `gR` (or `:review`) puts it up at the right of the window: the
//! suggestions grouped by section (`s`: by category, agent, kind, state,
//! request), those that wait shown (Tab: waiting for the agent, answered
//! by it, decided, all), narrowed to the group of the chosen row with
//! `n`. `j` and `k` walk through them in the order of the text, and the
//! page follows; the keys that act on a suggestion on the page act on
//! the chosen one here without their `g` (`a`, `r`, `A`, `q`, `p`, `z`,
//! `!`), and on a group's row on all of its suggestions at once. Below
//! the list, the chosen one's thread. Escape puts it away.

use std::fmt::Write as _;

use super::super::assist::cut;
use super::super::notes::{literal, wrap};
use super::super::{PANEL_MARGIN, Quad, Scene, TextureId, Viewer};
use super::{Asking, Kind, Said, Suggestion, Who};
use crate::keys::{Key, KeyPress};

/// How many rows of the list are shown at once.
const ROWS: usize = 16;
/// How many characters wide a row is.
const COLUMNS: usize = 58;
/// How many steps of the thread are shown, and how many when asked.
const THREAD: usize = 4;
const THREAD_ALL: usize = 14;
const BACKGROUND: &str = "#282828";
const FOREGROUND: &str = "#ebdbb2";
const DIM: &str = "#928374";
const ACCENT: &str = "#fabd2f";
const CHOSEN: &str = "#504945";
const FONT: &str = "DejaVu Sans Mono";
const KEYS: &str = "j k walk · a accept · r reject (R why) · A rewrite · q answer · p wordings · \
                    z later · o open again · ! always · u undo · Tab states · s group · n narrow · \
                    t thread · b brief · w record · Esc";

/// What the suggestions are grouped by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grouping {
    Section,
    Category,
    Agent,
    Kind,
    State,
    Request,
}

impl Grouping {
    fn next(self) -> Self {
        match self {
            Self::Section => Self::Category,
            Self::Category => Self::Agent,
            Self::Agent => Self::Kind,
            Self::Kind => Self::State,
            Self::State => Self::Request,
            Self::Request => Self::Section,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Section => "section",
            Self::Category => "category",
            Self::Agent => "agent",
            Self::Kind => "kind",
            Self::State => "state",
            Self::Request => "request",
        }
    }

    fn named(name: &str) -> Option<Self> {
        [
            Self::Section,
            Self::Category,
            Self::Agent,
            Self::Kind,
            Self::State,
            Self::Request,
        ]
        .into_iter()
        .find(|grouping| grouping.name() == name)
    }

    /// The group `suggestion` is in.
    fn of(self, suggestion: &Suggestion) -> String {
        match self {
            Self::Section if suggestion.section.is_empty() => "before the first heading".to_owned(),
            Self::Section => suggestion.section.clone(),
            Self::Category => suggestion.category.clone(),
            Self::Agent => suggestion.by.clone(),
            Self::Kind => suggestion.kind.name().to_owned(),
            Self::State => suggestion.standing().to_owned(),
            Self::Request => suggestion
                .request
                .map_or_else(|| "not asked for".to_owned(), |id| format!("request {id}")),
        }
    }
}

/// Which suggestions are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Showing {
    /// Those not decided.
    Open,
    /// Those waiting for an agent.
    Waiting,
    /// Those an agent answered.
    Answered,
    Decided,
    All,
}

impl Showing {
    fn next(self) -> Self {
        match self {
            Self::Open => Self::Waiting,
            Self::Waiting => Self::Answered,
            Self::Answered => Self::Decided,
            Self::Decided => Self::All,
            Self::All => Self::Open,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Open => "undecided",
            Self::Waiting => "waiting for the agent",
            Self::Answered => "answered by the agent",
            Self::Decided => "decided",
            Self::All => "all",
        }
    }

    fn keeps(self, suggestion: &Suggestion) -> bool {
        match self {
            Self::Open => !suggestion.state.decided(),
            Self::Waiting => !suggestion.state.decided() && suggestion.waiting,
            Self::Answered => suggestion.standing() == "answered",
            Self::Decided => suggestion.state.decided(),
            Self::All => true,
        }
    }
}

/// A row of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Row {
    /// A group, and how many of its suggestions are shown.
    Group(String, usize),
    Item(u64),
}

/// What the manager keeps while it is up.
pub(in super::super) struct Manager {
    grouping: Grouping,
    showing: Showing,
    only: Option<(Grouping, String)>,
    thread: bool,
    rows: Vec<Row>,
    chosen: usize,
    /// The state of the suggestions the rows were made for.
    built: Option<u64>,
}

impl Default for Manager {
    fn default() -> Self {
        Self {
            grouping: Grouping::Section,
            showing: Showing::Open,
            only: None,
            thread: false,
            rows: Vec::new(),
            chosen: 0,
            built: None,
        }
    }
}

/// The sign of a suggestion's kind in the list.
fn sign(kind: Kind) -> &'static str {
    match kind {
        Kind::Replace => "~",
        Kind::Insert => "+",
        Kind::Delete => "\u{2212}",
        Kind::Move => "\u{2195}",
        Kind::Comment => "\u{201d}",
        Kind::Question => "?",
    }
}

/// A step of a thread, in a line.
pub(super) fn step_words(by: &Who, said: &Said) -> String {
    let who = match by {
        Who::Author => "you".to_owned(),
        Who::Agent(name) => name.clone(),
    };
    let what = match said {
        Said::Made => "suggested it".to_owned(),
        Said::Message(text) => format!("“{text}”"),
        Said::Version { text, .. } => format!("revised it: “{}”", cut(text, 40)),
        Said::Alternatives(all) => format!("offered {} other wordings", all.len()),
        Said::AskedAlternatives => "asked for other wordings".to_owned(),
        Said::Accepted { .. } => "accepted it".to_owned(),
        Said::Rewritten { after, .. } => format!("rewrote it: “{}”", cut(after, 40)),
        Said::Rejected(why) if why.is_empty() => "rejected it".to_owned(),
        Said::Rejected(why) => format!("rejected it: “{why}”"),
        Said::Later => "put it aside".to_owned(),
        Said::Reopened => "opened it again".to_owned(),
        Said::Withdrawn => "took it back".to_owned(),
        Said::Instruction(text) => format!("made it an instruction: “{text}”"),
    };
    format!("{who}: {what}")
}

impl Viewer {
    /// Whether the manager is up, and keys are for it.
    #[must_use]
    pub fn is_reviewing(&self) -> bool {
        self.review.manager.is_some()
    }

    /// `gR`, `:review`: the manager put up, or away.
    pub(super) fn toggle_review_manager(&mut self) {
        if self.review.manager.take().is_none() {
            self.open_review_manager();
        }
    }

    pub(super) fn open_review_manager(&mut self) {
        self.review_ready();
        let here = self.suggestion_here().map(|at| self.review.all[at].id);
        self.review.manager = Some(Manager::default());
        self.build_review_rows();
        let Some(manager) = &mut self.review.manager else {
            return;
        };
        let item = |row: &Row| matches!(row, Row::Item(_));
        let at = here
            .and_then(|id| manager.rows.iter().position(|row| *row == Row::Item(id)))
            .or_else(|| manager.rows.iter().position(item));
        manager.chosen = at.unwrap_or(0);
    }

    /// The rows of the manager, made anew where the suggestions or what
    /// is shown of them changed.
    fn build_review_rows(&mut self) {
        let generation = self.review.generation;
        let Some(manager) = &self.review.manager else {
            return;
        };
        if manager.built == Some(generation) {
            return;
        }
        let current = self.file_id();
        let mut shown: Vec<&Suggestion> = self
            .review
            .all
            .iter()
            .filter(|suggestion| manager.showing.keeps(suggestion))
            .filter(|suggestion| {
                manager
                    .only
                    .as_ref()
                    .is_none_or(|(by, value)| by.of(suggestion) == *value)
            })
            .collect();
        shown.sort_by_key(|suggestion| (suggestion.file != current, suggestion.range.start));
        // Groups in the order their first suggestion comes in the text,
        // but for those named by a word, which go by it.
        let mut groups: Vec<(String, Vec<u64>)> = Vec::new();
        for suggestion in &shown {
            let group = manager.grouping.of(suggestion);
            match groups.iter_mut().find(|(name, _)| *name == group) {
                Some((_, ids)) => ids.push(suggestion.id),
                None => groups.push((group, vec![suggestion.id])),
            }
        }
        if manager.grouping != Grouping::Section {
            groups.sort_by(|a, b| a.0.cmp(&b.0));
        }
        let chosen = manager.rows.get(manager.chosen).cloned();
        let mut rows = Vec::new();
        for (name, ids) in groups {
            rows.push(Row::Group(name, ids.len()));
            rows.extend(ids.into_iter().map(Row::Item));
        }
        let Some(manager) = &mut self.review.manager else {
            return;
        };
        let again = chosen.and_then(|chosen| rows.iter().position(|row| *row == chosen));
        manager.chosen = again
            .unwrap_or(manager.chosen)
            .min(rows.len().saturating_sub(1));
        manager.rows = rows;
        manager.built = Some(generation);
    }

    /// The row chosen, and the suggestions it stands for: one, or all of
    /// a group.
    fn review_chosen_row(&self) -> Option<(Row, Vec<u64>)> {
        let manager = self.review.manager.as_ref()?;
        let row = manager.rows.get(manager.chosen)?.clone();
        let ids = match &row {
            Row::Item(id) => vec![*id],
            Row::Group(..) => manager.rows[manager.chosen + 1..]
                .iter()
                .map_while(|row| match row {
                    Row::Item(id) => Some(*id),
                    Row::Group(..) => None,
                })
                .collect(),
        };
        Some((row, ids))
    }

    /// A key while the manager is up. Returns whether it was for it.
    pub(in super::super) fn review_key(&mut self, press: KeyPress) -> bool {
        if self.review.manager.is_none() {
            return false;
        }
        self.review_ready();
        self.build_review_rows();
        let Some((row, ids)) = self.review_chosen_row() else {
            if matches!(press.key, Key::Escape) {
                self.review.manager = None;
            } else if matches!(press.key, Key::Tab | Key::Char('s' | 'N' | 'b' | 'w' | 'u')) {
                self.review_manager_char(press, None, &[]);
            } else {
                self.review.manager = None;
                self.notice("no suggestions here: gQ asks agents for a review");
            }
            return true;
        };
        let item = match row {
            Row::Item(id) => Some(id),
            Row::Group(..) => None,
        };
        match (press.key, press.ctrl) {
            (Key::Escape, _) | (Key::Char('c' | '['), true) => self.review.manager = None,
            (Key::Down, _) | (Key::Char('j'), false) | (Key::Char('n'), true) => {
                self.review_manager_step(true);
            }
            (Key::Up, _) | (Key::Char('k'), false) | (Key::Char('p'), true) => {
                self.review_manager_step(false);
            }
            (Key::Enter, _) => match item {
                Some(id) => {
                    self.review.manager = None;
                    self.go_to_suggestion(id);
                }
                None => self.review_manager_narrow(),
            },
            (Key::Char(_) | Key::Tab, false) => self.review_manager_char(press, item, &ids),
            _ => {}
        }
        true
    }

    /// A key of the manager that acts.
    fn review_manager_char(&mut self, press: KeyPress, item: Option<u64>, ids: &[u64]) {
        let c = match press.key {
            Key::Tab => '\t',
            Key::Char(c) => c,
            _ => return,
        };
        let current = self.file_id();
        let here: Vec<u64> = ids
            .iter()
            .copied()
            .filter(|id| {
                self.suggestion_index(*id)
                    .is_some_and(|at| self.review.all[at].file == current)
            })
            .collect();
        match c {
            '\t' => {
                if let Some(manager) = &mut self.review.manager {
                    manager.showing = manager.showing.next();
                    manager.built = None;
                    manager.chosen = 0;
                }
            }
            's' => {
                if let Some(manager) = &mut self.review.manager {
                    manager.grouping = manager.grouping.next();
                    manager.built = None;
                }
            }
            'n' => self.review_manager_narrow(),
            'N' => {
                if let Some(manager) = &mut self.review.manager {
                    manager.only = None;
                    manager.built = None;
                }
            }
            't' => {
                if let Some(manager) = &mut self.review.manager {
                    manager.thread = !manager.thread;
                }
            }
            'a' | 'r' | 'z' if here.len() < ids.len() && item.is_none() => {
                self.notice("of those, only the ones of the file being edited are decided here");
                self.review_manager_many(c, &here);
            }
            'a' | 'r' | 'z' => {
                if let Some(id) = item
                    && !here.contains(&id)
                {
                    self.go_to_suggestion(id);
                }
                let all = if item.is_some() { ids.to_vec() } else { here };
                self.review_manager_many(c, &all);
            }
            'A' | 'q' | 'R' | 'p' | '!' => {
                let Some(id) = item else {
                    return self.notice("that is for one suggestion: j to one");
                };
                self.go_to_suggestion(id);
                match c {
                    'A' => {
                        self.review.manager = None;
                        self.rewrite_here();
                    }
                    'q' => self.begin_review_line(Asking::Answer, Some(id), "answer "),
                    'R' => self.begin_review_line(Asking::Reject, Some(id), "reject "),
                    'p' => self.ask_alternatives(""),
                    _ => {
                        self.review.requesting = None;
                        self.begin_review_line(Asking::Always, Some(id), "always ");
                    }
                }
            }
            'o' => self.reopen_suggestions(ids),
            'u' => self.execute("review undo"),
            'b' => {
                self.review.manager = None;
                self.brief_command("");
            }
            'w' => self.write_review_record(false),
            _ => {}
        }
        self.build_review_rows();
    }

    /// Accepts (`a`), rejects (`r`) or puts aside (`z`) `ids`.
    fn review_manager_many(&mut self, c: char, ids: &[u64]) {
        let (done, how) = match c {
            'a' => (self.accept_suggestions(ids), "accepted"),
            'r' => (self.reject_suggestions(ids, ""), "rejected"),
            _ => (self.put_aside(ids), "put aside"),
        };
        self.notice(format!("{how} {done} · u takes it back"));
        self.build_review_rows();
    }

    /// `n`, Enter on a group: only the chosen row's group shown, or all
    /// again.
    fn review_manager_narrow(&mut self) {
        let group = self.review_chosen_row().and_then(|(row, ids)| match row {
            Row::Group(name, _) => Some(name),
            Row::Item(_) => ids.first().and_then(|id| {
                let at = self.suggestion_index(*id)?;
                let grouping = self.review.manager.as_ref()?.grouping;
                Some(grouping.of(&self.review.all[at]))
            }),
        });
        if let Some(manager) = &mut self.review.manager {
            manager.only = match (&manager.only, group) {
                (Some(_), _) | (None, None) => None,
                (None, Some(group)) => Some((manager.grouping, group)),
            };
            manager.built = None;
            manager.chosen = 0;
        }
    }

    /// `j`, `k`: to the next row, and the page to its suggestion.
    fn review_manager_step(&mut self, forward: bool) {
        let Some(manager) = &mut self.review.manager else {
            return;
        };
        let count = manager.rows.len().max(1);
        manager.chosen = if forward {
            (manager.chosen + 1) % count
        } else {
            (manager.chosen + count - 1) % count
        };
        if let Some(Row::Item(id)) = manager.rows.get(manager.chosen).cloned() {
            let in_file = self
                .suggestion_index(id)
                .is_some_and(|at| self.review.all[at].file == self.file_id());
            if in_file {
                self.go_to_suggestion(id);
            }
        }
    }

    /// `:review only category spelling`, `:review all`: what the manager
    /// shows.
    pub(super) fn review_manager_command(&mut self, verb: &str, more: &str) {
        if self.review.manager.is_none() {
            self.open_review_manager();
        }
        let (by, value) = more.split_once(' ').unwrap_or((more, ""));
        let Some(manager) = &mut self.review.manager else {
            return;
        };
        match (verb, Grouping::named(by)) {
            ("all", _) => {
                manager.only = None;
                manager.showing = Showing::All;
            }
            (_, Some(grouping)) if !value.is_empty() => {
                manager.grouping = grouping;
                manager.only = Some((grouping, value.trim().to_owned()));
            }
            _ => {
                return self.alert(
                    "only what? :review only category spelling (section, agent, kind, state, request)",
                );
            }
        }
        manager.built = None;
        manager.chosen = 0;
        self.build_review_rows();
    }

    /// Draws the manager at the right of the window.
    pub(super) fn review_manager_panel(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
    ) {
        self.build_review_rows();
        let markup = self.review_manager_markup();
        let scale = self.scale;
        let Some(image) = self.chrome.render(&markup, scale) else {
            return;
        };
        let (width, height) = (
            f64::from(image.pixmap.width()),
            f64::from(image.pixmap.height()),
        );
        let margin = PANEL_MARGIN * scale;
        let x = (f64::from(self.window.0) - margin - width)
            .max(margin)
            .round();
        let y = margin.round();
        let line = scale.round().max(1.0);
        super::super::solid(
            scene,
            has,
            super::super::LENS_EDGE,
            (x - line, y - line, width + 2.0 * line, height + 2.0 * line),
        );
        scene.quads.push(Quad {
            texture: image.id,
            x: x as f32,
            y: y as f32,
            width: width as f32,
            height: height as f32,
        });
        if !has(image.id) {
            scene.images.push(image);
        }
    }

    /// The manager as markup: a title, the rows in view, the chosen
    /// suggestion and its thread, and the keys.
    #[allow(
        clippy::too_many_lines,
        reason = "the list, the chosen one and the keys, in one markup"
    )]
    fn review_manager_markup(&self) -> String {
        let Some(manager) = &self.review.manager else {
            return String::new();
        };
        let undecided = self
            .review
            .all
            .iter()
            .filter(|s| !s.state.decided())
            .count();
        let waiting = self
            .review
            .all
            .iter()
            .filter(|s| !s.state.decided() && s.waiting)
            .count();
        let mut title = format!(
            "review · {undecided} undecided, {waiting} waiting for agents · {} · by {}",
            manager.showing.name(),
            manager.grouping.name()
        );
        if let Some((by, value)) = &manager.only {
            let _ = write!(title, " · only {} {}", by.name(), cut(value, 20));
        }
        let first = manager
            .chosen
            .saturating_sub(ROWS / 2)
            .min(manager.rows.len().saturating_sub(ROWS));
        let mut body = String::new();
        if manager.rows.is_empty() {
            let _ = writeln!(
                body,
                "  text(fill: rgb(\"{DIM}\"), {}),",
                literal("nothing here · Tab shows others · gQ asks agents for a review")
            );
        }
        for (index, row) in manager.rows.iter().enumerate().skip(first).take(ROWS) {
            let content = match row {
                Row::Group(name, count) => format!(
                    "text(fill: rgb(\"{ACCENT}\"), {})",
                    literal(&format!("{} ({count})", cut(name, COLUMNS - 6)))
                ),
                Row::Item(id) => {
                    let Some(suggestion) = self.review.all.iter().find(|s| s.id == *id) else {
                        continue;
                    };
                    let [r, g, b] = super::page::colour(suggestion);
                    let words = if suggestion.reason.is_empty() {
                        cut(&suggestion.offered, 30)
                    } else {
                        cut(&suggestion.reason, 30)
                    };
                    format!(
                        "text(fill: rgb(\"#{r:02x}{g:02x}{b:02x}\"), {}) + h(4pt) + {} + h(6pt) + text(fill: rgb(\"{DIM}\"), {})",
                        literal(sign(suggestion.kind)),
                        literal(&format!("{:<12} {words}", cut(&suggestion.category, 12))),
                        literal(&format!(
                            "{} · {}",
                            suggestion.standing(),
                            cut(&suggestion.by, 12)
                        ))
                    )
                }
            };
            if index == manager.chosen {
                let _ = writeln!(
                    body,
                    "  block(fill: rgb(\"{CHOSEN}\"), outset: (x: 3pt, y: 2pt), radius: 2pt, {content}),"
                );
            } else {
                let _ = writeln!(body, "  {content},");
            }
        }
        let mut detail = Vec::new();
        if let Some(Row::Item(id)) = manager.rows.get(manager.chosen)
            && let Some(suggestion) = self.review.all.iter().find(|s| s.id == *id)
        {
            let text = self
                .editor
                .iter()
                .chain(&self.others)
                .find(|e| e.file() == suggestion.file)
                .map(crate::editor::Editor::text)
                .unwrap_or_default();
            let line = super::super::agent::line_column(text, suggestion.range.start).0;
            detail.push(format!(
                "{} {} · {} line {line} · {}",
                suggestion.id,
                suggestion.kind.name(),
                self.file_name_of_id(suggestion.file),
                suggestion.state.name()
            ));
            detail.extend(wrap(&suggestion.reason, COLUMNS, 3));
            if suggestion.kind.changes() {
                detail.push(format!(
                    "“{}” → “{}”",
                    cut(&suggestion.quote, COLUMNS / 2 - 4),
                    cut(&suggestion.new, COLUMNS / 2 - 4)
                ));
            }
            let most = if manager.thread { THREAD_ALL } else { THREAD };
            let skip = suggestion.thread.len().saturating_sub(most);
            for step in suggestion.thread.iter().skip(skip) {
                detail.push(cut(&step_words(&step.by, &step.said), COLUMNS));
            }
        }
        let mut lines = String::new();
        for line in &detail {
            let _ = writeln!(lines, "  text(fill: rgb(\"{DIM}\"), {}),", literal(line));
        }
        format!(
            "#set page(width: auto, height: auto, margin: 8pt, fill: rgb(\"{BACKGROUND}\"))\n\
             #set text(font: \"{FONT}\", size: 8.5pt, fill: rgb(\"{FOREGROUND}\"), ligatures: false, hyphenate: false)\n\
             #stack(\n  spacing: 5pt,\n  text(fill: rgb(\"{ACCENT}\"), {}),\n{body}  line(length: 100%, stroke: 0.5pt + rgb(\"{DIM}\")),\n{lines}  block(width: {}em, text(fill: rgb(\"{DIM}\"), size: 7.5pt, {})),\n)",
            literal(&title),
            COLUMNS * 6 / 10,
            literal(KEYS)
        )
    }
}
