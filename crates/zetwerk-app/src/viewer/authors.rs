//! Who wrote what, on the page: the text tinted by where it came from,
//! the story of the sentence under the cursor, the shares of the
//! document, and a statement of them.
//!
//! `gH` tints the pages by origin: the author's own text stays as it is,
//! an agent's text is in a quiet colour (one hue per agent), agent text
//! the author has edited since is in a lighter mix of it, pasted text and
//! text changed outside zetwerk each in a colour of their own, text of
//! unknown origin faintly grey. A legend in the bottom-left corner says
//! which colour is what. Each glyph is tinted by the bytes it shows; what
//! the page shows as one thing (a reference, the part of an equation a
//! glyph stands for) by all of its bytes: mixed where an agent and the
//! author both wrote some. While the tint is on, the status line tells
//! the story of the sentence under the cursor ("Written by Claude Code
//! 2.1 on 15 Nov, accepted by you; you changed 3 words on 16 Nov"), and
//! `:authors step` opens the history (`gh`) at the step that put the text under the
//! cursor there. `:authors age` tints by how old the text is instead,
//! `:authors unread` only agents' text that the author has not taken in
//! (accepted, or edited in the same paragraph since), `:authors off`
//! (or `gH` again) takes the tint away.
//!
//! The tint of a page is worked out when the page, the text or the view
//! changes, for the pages in view, and kept: a frame only draws it.
//!
//! `:authors` puts up the shares of the document as it is (see
//! [`crate::authorship`] for what counts), as a whole, per section and
//! over the history; `:authors statement` writes them, with which agents
//! took part and when, beside the document as `<name>.authorship.md`,
//! worded for an "AI use" declaration; `:authors export` writes the raw
//! attribution beside it as `<name>.authorship.tsv`, for another machine
//! or a git repository, and `:authors import` reads it back.
//!
//! Agents are told the same through MCP (`get_authorship`); they cannot
//! change it: an agent's edit is an agent's, whatever it asks.

use crate::chrome::FONT;
use crate::chrome::typst_string as string;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::ops::Range;
use zetwerk_core::now;

use typst::layout::Rect;
use zetwerk_core::{Credit, Kind, Mark, Origin};
use zetwerk_typst::{Origin as Shows, PageMap};

use super::{Edited, Scene, TextureId, Viewer, tint};
use crate::authorship::{self, Counted, KINDS, kind_name};

pub use zetwerk_core::Agent;

/// What the user can set for this in the configuration (`[authors]`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// The tint the window opens with, if any.
    pub view: Option<Tint>,
    /// Whether text of unknown origin is left untinted.
    pub hide_unknown: bool,
}

/// How the pages are tinted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tint {
    /// By who wrote the text.
    Origin,
    /// By how old it is.
    Age,
    /// Only agents' text the author has not taken in.
    Unread,
}

/// The colours: an agent's text by agent, the same lighter where the
/// author edited it, and the others. RGBA, laid over the page.
const AGENT_HUES: [[u8; 3]; 4] = [
    [0x45, 0x85, 0x88],
    [0xb1, 0x62, 0x86],
    [0x68, 0x9d, 0x6a],
    [0xd6, 0x5d, 0x0e],
];
const AGENT_ALPHA: u8 = 0x40;
const EDITED_ALPHA: u8 = 0x1c;
const PASTED: [u8; 4] = [0xd7, 0x99, 0x21, 0x40];
const OUTSIDE: [u8; 4] = [0x83, 0xa5, 0x98, 0x48];
const UNKNOWN: [u8; 4] = [0x92, 0x83, 0x74, 0x1c];
/// Age, from the newest to the oldest: today, this week, this month,
/// this year, older.
const AGES: [[u8; 4]; 5] = [
    [0xfb, 0x49, 0x34, 0x48],
    [0xfe, 0x80, 0x19, 0x38],
    [0xfa, 0xbd, 0x2f, 0x30],
    [0x83, 0xa5, 0x98, 0x28],
    [0x92, 0x83, 0x74, 0x18],
];
const AGE_NAMES: [&str; 5] = [
    "today",
    "this week",
    "this month",
    "this year",
    "older or unknown",
];
const DAY: u64 = 86_400;

/// What the tint of a page was worked out for: the pages, the text and
/// how it is tinted.
type TintKey = (usize, u64, Tint, bool);

/// The cells of a page that are tinted, in points of the page, and
/// their colours.
type Cells = Vec<(Rect, [u8; 4])>;

/// How the agents' share moved over the history: by state on the line
/// of the history, how many steps in, the share as written and as
/// edited by the author since.
type Moved = Vec<(usize, f64, f64)>;

/// The shares of a revision of the text, and how they moved.
type Shared = (u64, Counted, Moved);

/// What is known of who wrote what, beside the text.
#[derive(Debug, Default)]
pub(super) struct Authors {
    tint: Option<Tint>,
    hide_unknown: bool,
    /// Whether the panel of shares is up.
    panel: bool,
    /// The agent whose request is being answered, if one's is.
    acting: Option<Agent>,
    /// The agent behind each proposal, by its number.
    proposed: HashMap<u64, Agent>,
    /// The tint of each page in view: cells in points of the page and
    /// their colours.
    pub(super) tinted: HashMap<usize, (TintKey, Cells)>,
    /// The colours on the pages tinted last, and what they stand for.
    legend: Vec<([u8; 4], String)>,
    /// The story of the sentence under the cursor, for the revision and
    /// the sentence it is of.
    story: Option<((u64, Range<usize>), String)>,
    /// The shares, for the revision they are of; and how they moved.
    counted: Option<Shared>,
}

/// A colour for an agent, the same each time for the same name.
fn hue(name: &str) -> [u8; 3] {
    let sum = name.bytes().fold(0_usize, |sum, byte| {
        sum.wrapping_mul(31).wrapping_add(byte.into())
    });
    AGENT_HUES[sum % AGENT_HUES.len()]
}

fn with_alpha([r, g, b]: [u8; 3], alpha: u8) -> [u8; 4] {
    [r, g, b, alpha]
}

fn rgba([r, g, b, a]: [u8; 4]) -> String {
    format!("rgb({r}, {g}, {b}, {a})")
}

/// The file the document's account is written to, beside it.
fn beside(path: &std::path::Path, ending: &str) -> std::path::PathBuf {
    let stem = path.file_stem().map_or_else(
        || "document".into(),
        |stem| stem.to_string_lossy().into_owned(),
    );
    path.with_file_name(format!("{stem}.{ending}"))
}

impl Authors {
    pub(super) fn configure(&mut self, settings: &Settings) {
        self.tint = settings.view;
        self.hide_unknown = settings.hide_unknown;
    }
}

impl Viewer {
    /// The commands of who wrote what. Returns whether `command` is one.
    pub(super) fn authors_command(&mut self, command: &str) -> bool {
        let Some(rest) = command
            .strip_prefix("authors")
            .filter(|rest| rest.is_empty() || rest.starts_with([' ', '!']))
        else {
            return false;
        };
        let (force, rest) = match rest.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, rest),
        };
        let (word, argument) = rest.trim().split_once(' ').unwrap_or((rest.trim(), ""));
        let (word, force) = match word.strip_suffix('!') {
            Some(word) => (word, true),
            None => (word, force),
        };
        match word {
            "" => {
                self.authors.panel = !self.authors.panel;
                self.authors.counted = None;
            }
            "view" | "origin" => self.toggle_tint(Tint::Origin),
            "age" => self.toggle_tint(Tint::Age),
            "unread" => self.toggle_tint(Tint::Unread),
            "off" => self.set_tint(None),
            "step" => self.step_of_text(),
            "statement" => self.write_statement(force),
            "export" => self.export_authorship(force),
            "import" => self.import_authorship(argument.trim()),
            other => self.alert(format!(
                "authors {other}? :authors, :authors view, age, unread, off, statement, \
                 export, import"
            )),
        }
        true
    }

    fn toggle_tint(&mut self, tint: Tint) {
        let next = (self.authors.tint != Some(tint)).then_some(tint);
        self.set_tint(next);
    }

    fn set_tint(&mut self, tint: Option<Tint>) {
        self.authors.tint = tint;
        self.authors.tinted.clear();
        self.authors.story = None;
        self.notice(match tint {
            Some(Tint::Origin) => {
                "tinted by who wrote it: gH again takes it away, :authors step to its step"
            }
            Some(Tint::Age) => "tinted by how old the text is: :authors off takes it away",
            Some(Tint::Unread) => "agents' text you have not taken in: :authors off",
            None => "the pages are as they are",
        });
    }

    /// Who an agent's change made now is by: the agent whose request it
    /// answers, or one that did not say who it is.
    pub(super) fn agent_origin(&self) -> Origin {
        Origin::Agent(self.authors.acting.clone().unwrap_or_else(|| Agent {
            name: "an agent".to_owned(),
            ..Agent::default()
        }))
    }

    /// Notes which agent proposal `id` is of.
    pub(super) fn noted_proposal(&mut self, id: u64) {
        let Origin::Agent(agent) = self.agent_origin() else {
            return;
        };
        self.authors.proposed.insert(id, agent);
    }

    /// Who the text of proposal `id` is by once the author accepts it.
    pub(super) fn accepted_origin(&self, id: u64) -> Origin {
        let agent = self.authors.proposed.get(&id).cloned();
        Origin::Accepted(agent.unwrap_or_else(|| Agent {
            name: "an agent".to_owned(),
            ..Agent::default()
        }))
    }

    /// Makes `edits` (see [`crate::editor::Editor::replace`]) as the
    /// author accepting what proposal or suggestion `id` of an agent asks
    /// for: its text is that agent's, accepted. The one call an accept
    /// path makes in place of `Editor::replace`.
    pub fn replace_accepted(
        &mut self,
        id: u64,
        edits: &[(Range<usize>, &str)],
    ) -> Result<(), String> {
        let origin = self.accepted_origin(id);
        let editor = self.editor.as_mut().ok_or(super::agent::NOT_EDITING)?;
        editor.replace_by(edits, &origin)
    }

    /// Does `task` as the agent `agent`: what it changes, live or by
    /// proposal, is that agent's.
    pub fn as_agent<T>(&mut self, agent: Option<Agent>, task: impl FnOnce(&mut Self) -> T) -> T {
        let before = std::mem::replace(&mut self.authors.acting, agent);
        let done = task(self);
        self.authors.acting = before;
        done
    }

    /// `:authors step`: the history, at the step that put the text under the cursor
    /// there.
    fn step_of_text(&mut self) {
        let Some(editor) = &self.editor else {
            return;
        };
        let cursor = editor.cursor();
        match editor.origins().introduced(editor.history(), cursor) {
            Some(state) => {
                let _ = self.history_command(&format!("history at {state}"));
            }
            None => self.notice("this text was there before its history began"),
        }
    }

    /// The story of the sentence under the cursor, for the status line,
    /// while the pages are tinted by origin.
    pub(super) fn authors_status(&mut self) -> Option<String> {
        self.authors.tint?;
        let editor = self.editor.as_ref()?;
        let text = editor.text();
        let sentence = authorship::sentence_at(text, editor.cursor());
        let key = (editor.revision(), sentence.clone());
        if let Some((of, told)) = &self.authors.story
            && *of == key
        {
            return Some(told.clone());
        }
        let told = authorship::story(text, sentence, editor.origins(), now());
        self.authors.story = Some((key, told.clone()));
        Some(told)
    }

    /// Tints page `index` (whose top-left corner is at `origin`) by who
    /// wrote its text.
    #[allow(
        clippy::too_many_lines,
        reason = "worked out and drawn in one, as the other marks on a page are"
    )]
    pub(super) fn authors_page(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        index: usize,
        origin: (f64, f64),
    ) {
        let Some(tint_by) = self.authors.tint else {
            return;
        };
        let (Some(document), Some(world), Some(editor)) =
            (&self.document, &self.compiled_world, &self.editor)
        else {
            return;
        };
        // Where the pages are not those of the text, nothing is said of
        // them: a stale tint is a wrong one.
        let Some(change) = self.places() else {
            return;
        };
        let key = (
            document.pages().as_ptr() as usize,
            editor.revision(),
            tint_by,
            self.authors.hide_unknown,
        );
        let fresh = self
            .authors
            .tinted
            .get(&index)
            .is_some_and(|(of, _)| *of == key);
        if !fresh {
            let Some(page) = document.pages().get(index) else {
                return;
            };
            let map = self
                .maps
                .entry(index)
                .or_insert_with(|| PageMap::build(world, &page.frame));
            let file = editor.file();
            let origins = editor.origins();
            let text = editor.text();
            let mut ranges: Vec<(Rect, Range<usize>, bool)> = Vec::new();
            for glyph in map.glyphs() {
                if glyph.furniture {
                    continue;
                }
                let (range, one) = match &glyph.origin {
                    Shows::Text { file: of, range } | Shows::Space { file: of, range }
                        if *of == file =>
                    {
                        (range, false)
                    }
                    Shows::Math { file: of, range }
                    | Shows::Object {
                        file: of, range, ..
                    } if *of == file => (range, true),
                    _ => continue,
                };
                let now = change.forth(range.start)..change.forth(range.end);
                if !now.is_empty() {
                    ranges.push((glyph.rect, now, one));
                }
            }
            let low = ranges
                .iter()
                .map(|(_, range, _)| range.start)
                .min()
                .unwrap_or(0);
            let high = ranges
                .iter()
                .map(|(_, range, _)| range.end)
                .max()
                .unwrap_or(0);
            let runs: Vec<(Range<usize>, Mark)> = origins.map().within(low..high).collect();
            let paint = Painter {
                origins,
                text,
                runs: &runs,
                tint: tint_by,
                hide_unknown: self.authors.hide_unknown,
                now: now(),
            };
            let mut cells: Cells = Vec::new();
            let mut legend: Vec<([u8; 4], String)> = Vec::new();
            for (rect, range, one) in ranges {
                let Some((colour, name)) = paint.colour(&range, one) else {
                    continue;
                };
                if !legend.iter().any(|(known, _)| *known == colour) {
                    legend.push((colour, name));
                }
                join(&mut cells, rect, colour);
            }
            for entry in legend {
                if !self
                    .authors
                    .legend
                    .iter()
                    .any(|(known, _)| *known == entry.0)
                {
                    self.authors.legend.push(entry);
                }
            }
            self.authors.tinted.insert(index, (key, cells));
        }
        let zoom = self.view.zoom;
        let Some((_, cells)) = self.authors.tinted.get(&index) else {
            return;
        };
        for (rect, colour) in cells {
            let place = (
                origin.0 + rect.min.x.to_pt() * zoom,
                origin.1 + rect.min.y.to_pt() * zoom,
                (rect.max.x - rect.min.x).to_pt() * zoom,
                (rect.max.y - rect.min.y).to_pt() * zoom,
            );
            tint(scene, has, *colour, place);
        }
    }

    /// The legend of the tint, in the bottom-left corner; and the panel
    /// of the shares, where it is up.
    pub(super) fn authors_panels(&mut self, scene: &mut Scene, has: &dyn Fn(TextureId) -> bool) {
        let margin = super::PANEL_MARGIN * self.scale;
        if self.authors.tint.is_some() {
            let mut rows = String::new();
            let title = match self.authors.tint {
                Some(Tint::Age) => "how old",
                Some(Tint::Unread) => "not taken in",
                _ => "who wrote it",
            };
            let _ = writeln!(rows, "  text(fill: rgb(\"#fabd2f\"), {}),", string(title));
            let _ = writeln!(
                rows,
                "  [#box(width: 9pt, height: 9pt, stroke: 0.5pt + rgb(\"#928374\")) yours],"
            );
            let mut legend = self.authors.legend.clone();
            legend.sort_by(|a, b| a.1.cmp(&b.1));
            for (colour, name) in legend.iter().take(8) {
                let solid = [colour[0], colour[1], colour[2], colour[3].saturating_mul(3)];
                let _ = writeln!(
                    rows,
                    "  [#box(width: 9pt, height: 9pt, fill: {}) #{}],",
                    rgba(solid),
                    string(name)
                );
            }
            let markup = format!(
                "#set page(width: auto, height: auto, margin: 7pt, fill: rgb(\"#282828\"))\n\
                 #set text(font: {FONT}, size: 8pt, fill: rgb(\"#ebdbb2\"))\n\
                 #stack(spacing: 4pt,\n{rows})"
            );
            let bottom = f64::from(self.window.1) - f64::from(self.status_height()) - margin;
            self.panel(scene, has, &markup, |(_, height)| (margin, bottom - height));
        }
        if self.authors.panel {
            let markup = self.shares_markup();
            let right = f64::from(self.window.0) - margin;
            self.panel(scene, has, &markup, |(width, _)| (right - width, margin));
        }
    }

    /// The shares of the document, counted anew where the text changed
    /// and is not being typed into; and how they moved over its history.
    fn shares(&mut self) -> Option<&Shared> {
        let editor = self.editor.as_ref()?;
        let typing = editor.mode() == crate::editor::Mode::Insert;
        let stale = self
            .authors
            .counted
            .as_ref()
            .is_none_or(|(of, ..)| *of != editor.revision());
        if stale && !(typing && self.authors.counted.is_some()) {
            let counted = authorship::count(editor.source(), editor.origins());
            let moved = moved(editor);
            self.authors.counted = Some((editor.revision(), counted, moved));
        }
        self.authors.counted.as_ref()
    }

    /// The panel of the shares, as markup.
    #[allow(clippy::too_many_lines, reason = "a part of the panel after the other")]
    fn shares_markup(&mut self) -> String {
        let Some((_, counted, moved)) = self.shares().cloned() else {
            return "#set page(width: auto, height: auto, margin: 8pt, fill: rgb(\"#282828\"))\n\
                    #text(fill: rgb(\"#ebdbb2\"))[no text is being edited]"
                .to_owned();
        };
        let colour = |kind: Kind| match kind {
            Kind::Own => [0xeb, 0xdb, 0xb2, 0xff],
            Kind::Agent => [0x45, 0x85, 0x88, 0xff],
            Kind::Edited => [0x8e, 0xc0, 0x7c, 0xff],
            Kind::Pasted => [0xd7, 0x99, 0x21, 0xff],
            Kind::Outside => [0x83, 0xa5, 0x98, 0xff],
            Kind::Unknown => [0x66, 0x5c, 0x54, 0xff],
        };
        let bar = |shares: &authorship::Shares, width: f64| {
            let mut cells = Vec::new();
            for kind in KINDS {
                let part = shares.percent(kind);
                if part > 0.0 {
                    cells.push(format!(
                        "box(width: {:.1}pt, height: 7pt, fill: {})",
                        width * part / 100.0,
                        rgba(colour(kind))
                    ));
                }
            }
            if cells.is_empty() {
                "box(width: 1pt, height: 7pt)".to_owned()
            } else {
                cells.join(" + h(0pt) + ")
            }
        };
        let whole = &counted.whole;
        let mut rows = String::new();
        let _ = writeln!(
            rows,
            "  text(fill: rgb(\"#fabd2f\"), {}),",
            string(&format!(
                "who wrote this document: {} characters, {} words",
                whole.total_chars(),
                whole.total_words()
            ))
        );
        let _ = writeln!(rows, "  {},", bar(whole, 260.0));
        let mut table = String::new();
        for kind in KINDS {
            if whole.chars.get(&kind).copied().unwrap_or(0) == 0 {
                continue;
            }
            let _ = writeln!(
                table,
                "  box(width: 8pt, height: 8pt, fill: {}), {}, align(right, {}), align(right, {}),",
                rgba(colour(kind)),
                string(kind_name(kind)),
                string(&format!("{:.1} %", whole.percent(kind))),
                string(&format!("{:.1} % of words", whole.word_percent(kind)))
            );
        }
        let _ = writeln!(
            rows,
            "  grid(columns: 4, column-gutter: 8pt, row-gutter: 4pt, align: horizon,\n{table}  ),"
        );
        if !whole.agents.is_empty() {
            let agents: Vec<String> = whole
                .agents
                .iter()
                .map(|(name, count)| {
                    format!(
                        "{name}: {:.1} %",
                        *count as f64 * 100.0 / whole.total_chars().max(1) as f64
                    )
                })
                .collect();
            let _ = writeln!(
                rows,
                "  text(fill: rgb(\"#928374\"), {}),",
                string(&format!("agents: {}", agents.join(", ")))
            );
        }
        if counted.sections.len() > 1 {
            let _ = writeln!(rows, "  text(fill: rgb(\"#fabd2f\"), \"by section\"),");
            let mut sections = String::new();
            for (title, _, _, shares) in counted.sections.iter().take(14) {
                let mut title = title
                    .clone()
                    .unwrap_or_else(|| "(before the first heading)".to_owned());
                if title.chars().count() > 28 {
                    title = title.chars().take(27).collect::<String>() + "…";
                }
                let _ = writeln!(
                    sections,
                    "  {}, {}, align(right, {}),",
                    string(&title),
                    bar(shares, 120.0),
                    string(&format!(
                        "{:.0} % agent",
                        shares.percent(Kind::Agent) + shares.percent(Kind::Edited)
                    ))
                );
            }
            let _ = writeln!(
                rows,
                "  grid(columns: 3, column-gutter: 8pt, row-gutter: 4pt, align: horizon,\n{sections}  ),"
            );
        }
        if moved.len() > 1 {
            let mut columns = Vec::new();
            for (_, agent, edited) in &moved {
                columns.push(format!(
                    "stack(dir: btt, box(width: 6pt, height: {:.1}pt, fill: {}), box(width: 6pt, height: {:.1}pt, fill: {}), box(width: 6pt, height: {:.1}pt, fill: rgb(\"#3c3836\")))",
                    agent * 0.3,
                    rgba(colour(Kind::Agent)),
                    edited * 0.3,
                    rgba(colour(Kind::Edited)),
                    (100.0 - agent - edited).max(0.0) * 0.3
                ));
            }
            let _ = writeln!(
                rows,
                "  text(fill: rgb(\"#fabd2f\"), \"over the history: the agents' share, step by step\"),"
            );
            let _ = writeln!(
                rows,
                "  stack(dir: ltr, spacing: 2pt, {}),",
                columns.join(", ")
            );
        }
        let _ = writeln!(
            rows,
            "  text(fill: rgb(\"#928374\"), size: 7pt, {}),",
            string(
                "counted: letters, digits and signs of the text; not markup, code, comments or \
                 blanks; an equation is one word and one character. Over the history: \
                 characters of the source. :authors statement writes it down"
            )
        );
        format!(
            "#set page(width: 330pt, height: auto, margin: 8pt, fill: rgb(\"#282828\"))\n\
             #set text(font: {FONT}, size: 8pt, fill: rgb(\"#ebdbb2\"))\n\
             #stack(spacing: 7pt,\n{rows})"
        )
    }

    /// `:authors statement`: the shares and who took part, worded for an
    /// "AI use" declaration, beside the document.
    fn write_statement(&mut self, force: bool) {
        let Some(path) = self.editor.as_ref().map(|editor| editor.path().to_owned()) else {
            return self.alert("there is no text being edited");
        };
        let to = beside(&path, "authorship.md");
        if to.exists() && !force {
            return self.alert(format!(
                "{} exists: :authors statement! writes over it",
                to.display()
            ));
        }
        let Some(statement) = self.statement() else {
            return;
        };
        match zetwerk_core::write_atomically(&to, statement.as_bytes()) {
            Ok(()) => self.notice(format!("written: {}", to.display())),
            Err(err) => self.alert(format!("not written: {err}")),
        }
    }

    /// The statement, as Markdown.
    #[must_use]
    #[allow(
        clippy::too_many_lines,
        reason = "a part of the statement after the other"
    )]
    pub fn statement(&mut self) -> Option<String> {
        let notes = self.notes_listed(true).len();
        let proposals = self.authors.proposed.len();
        let (_, counted, moved) = self.shares()?.clone();
        let editor = self.editor.as_ref()?;
        let name = editor
            .path()
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let origins = editor.origins();
        let now = now();
        let whole = &counted.whole;
        let mut out = String::new();
        let _ = writeln!(out, "# Authorship of {name}\n");
        let _ = writeln!(
            out,
            "*Generated by zetwerk on {} from the editing history of the document. It is only \
             as complete as that history: text typed in another program and pasted in counts \
             as pasted, whatever its true source; text that was in the file before zetwerk \
             kept where text came from is unknown.*\n",
            authorship::iso(now)
        );
        let _ = writeln!(out, "## Shares of the text as it is\n");
        let _ = writeln!(out, "| | characters | words |\n|---|---:|---:|");
        for kind in KINDS {
            if whole.chars.get(&kind).copied().unwrap_or(0) == 0 {
                continue;
            }
            let _ = writeln!(
                out,
                "| {} | {:.1} % | {:.1} % |",
                kind_name(kind),
                whole.percent(kind),
                whole.word_percent(kind)
            );
        }
        let _ = writeln!(
            out,
            "| all | {} | {} |\n",
            whole.total_chars(),
            whole.total_words()
        );
        let _ = writeln!(
            out,
            "Counted are the letters, digits and signs of the text (running text, headings, \
             captions, footnotes, code listings); not markup, code, comments, labels, \
             references or blanks. A word is a run of letters and digits. Each equation \
             counts as one word and one character, for whoever wrote most of it.\n"
        );
        // The agents: who, when, and in what way.
        let mut agents: Vec<(String, u64, u64, bool, bool)> = Vec::new();
        for credit in origins.credits() {
            let Some(agent) = authorship::agent_name(&credit.origin) else {
                continue;
            };
            let accepted = matches!(credit.origin, Origin::Accepted(_));
            match agents.iter_mut().find(|(known, ..)| *known == agent) {
                Some(entry) => {
                    entry.1 = entry.1.min(credit.when);
                    entry.2 = entry.2.max(credit.when);
                    entry.3 |= !accepted;
                    entry.4 |= accepted;
                }
                None => agents.push((agent, credit.when, credit.when, !accepted, accepted)),
            }
        }
        let _ = writeln!(out, "## Agents that took part\n");
        if agents.is_empty() {
            let _ = writeln!(
                out,
                "No agent changed the text, as far as the history tells.\n"
            );
        }
        for (agent, first, last, live, accepted) in &agents {
            let how = match (live, accepted) {
                (true, true) => {
                    "changed the text directly, and proposed changes the author accepted"
                }
                (true, false) => "changed the text directly",
                _ => "proposed changes, which the author accepted",
            };
            let share = whole.agents.get(agent).copied().unwrap_or(0);
            let _ = writeln!(
                out,
                "- **{agent}**, from {} to {}: {how}; of its text, {:.1} % of the characters of \
                 the document remain (as written or edited since).",
                authorship::day(*first, now),
                authorship::day(*last, now),
                share as f64 * 100.0 / whole.total_chars().max(1) as f64
            );
        }
        if proposals > 0 || notes > 0 {
            let _ = writeln!(
                out,
                "\nIn this session agents made {proposals} proposals; there are {notes} notes and \
                 questions on the text."
            );
        }
        let _ = writeln!(out, "\n## By section\n");
        let _ = writeln!(
            out,
            "| section | yours | agent | agent, edited by you | other |\n|---|---:|---:|---:|---:|"
        );
        for (title, _, line, shares) in &counted.sections {
            let other = shares.percent(Kind::Pasted)
                + shares.percent(Kind::Outside)
                + shares.percent(Kind::Unknown);
            let _ = writeln!(
                out,
                "| {} (line {line}) | {:.0} % | {:.0} % | {:.0} % | {:.0} % |",
                title.as_deref().unwrap_or("before the first heading"),
                shares.percent(Kind::Own),
                shares.percent(Kind::Agent),
                shares.percent(Kind::Edited),
                other
            );
        }
        if let (Some(first), Some(last)) = (moved.first(), moved.last()) {
            let _ = writeln!(
                out,
                "\nOver the {} steps of the history kept, the agents' share of the source went \
                 from {:.0} % to {:.0} % (as written) and from {:.0} % to {:.0} % (edited by \
                 the author since).",
                last.0, first.1, last.1, first.2, last.2
            );
        }
        let ai = whole.percent(Kind::Agent) + whole.percent(Kind::Edited);
        let names: Vec<&str> = agents.iter().map(|(name, ..)| name.as_str()).collect();
        let _ = writeln!(out, "\n## For a declaration of the use of AI\n");
        if names.is_empty() {
            let _ = writeln!(
                out,
                "> According to the editing history kept by zetwerk, no generative AI tool \
                 changed the text of this document. {:.0} % of its characters were written by \
                 the author, {:.0} % pasted from other programs and {:.0} % are of unknown \
                 origin.",
                whole.percent(Kind::Own),
                whole.percent(Kind::Pasted),
                whole.percent(Kind::Unknown) + whole.percent(Kind::Outside)
            );
        } else {
            let _ = writeln!(
                out,
                "> Generative AI tools ({}) were used in writing this document. According to the \
                 editing history kept by zetwerk, {:.0} % of the characters of the final text \
                 were written by these tools, of which {:.0} percentage points were edited by \
                 the author afterwards; {:.0} % were written by the author. Every change made by \
                 a tool was either reviewed and accepted by the author or made with the \
                 author's permission, and the author takes responsibility for the whole text. \
                 This account is generated from the editing history and is only as complete \
                 as that history: text pasted from other programs ({:.0} %) and text of \
                 unknown origin ({:.0} %) may have other sources.",
                names.join(", "),
                ai,
                whole.percent(Kind::Edited),
                whole.percent(Kind::Own),
                whole.percent(Kind::Pasted),
                whole.percent(Kind::Unknown) + whole.percent(Kind::Outside)
            );
        }
        Some(out)
    }

    /// `:authors export`: the raw attribution, beside the document.
    fn export_authorship(&mut self, force: bool) {
        let Some(editor) = &self.editor else {
            return self.alert("there is no text being edited");
        };
        let path = editor.path().to_owned();
        let to = beside(&path, "authorship.tsv");
        if to.exists() && !force {
            return self.alert(format!(
                "{} exists: :authors export! writes over it",
                to.display()
            ));
        }
        let name = path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let exported = authorship::export(&name, editor.text(), editor.origins());
        match zetwerk_core::write_atomically(&to, exported.as_bytes()) {
            Ok(()) => self.notice(format!("written: {}", to.display())),
            Err(err) => self.alert(format!("not written: {err}")),
        }
    }

    /// `:authors import [file]`: the attribution of a file written by
    /// `:authors export`, for the text as it is: one undo step that
    /// changes no text, only who it is credited to.
    fn import_authorship(&mut self, file: &str) {
        let Some(editor) = &mut self.editor else {
            return self.alert("there is no text being edited");
        };
        let from = if file.is_empty() {
            beside(editor.path(), "authorship.tsv")
        } else {
            editor
                .path()
                .parent()
                .map_or_else(|| file.into(), |directory| directory.join(file))
        };
        let read = match std::fs::read_to_string(&from) {
            Ok(read) => read,
            Err(err) => return self.alert(format!("{}: {err}", from.display())),
        };
        let text = editor.text().to_owned();
        let (given, found) = match authorship::import(&read, &text) {
            Ok(imported) => imported,
            Err(why) => return self.alert(why),
        };
        let origin = Origin::Zetwerk(format!("imported the attribution of {}", from.display()));
        match editor.recredit(&given, &origin) {
            Ok(()) => {
                self.text_changed(Edited::Command);
                self.authors.tinted.clear();
                self.notice(format!(
                    "{found} of {} bytes found and credited as {} says; the rest is unknown \
                     (u takes it back)",
                    text.len(),
                    from.display()
                ));
            }
            Err(why) => self.alert(format!("not imported: {why}")),
        }
    }

    /// Who wrote the document as it is, for an agent: the shares, and for
    /// lines `from..=to` (if asked) the stretches with who wrote them.
    pub fn authorship(
        &mut self,
        lines: Option<(usize, usize)>,
    ) -> Result<serde_json::Value, String> {
        let revision = self.revision;
        let (_, counted, _) = self.shares().cloned().ok_or(super::agent::NOT_EDITING)?;
        let editor = self.editor.as_ref().ok_or(super::agent::NOT_EDITING)?;
        let text = editor.text();
        let origins = editor.origins();
        let shares = |shares: &authorship::Shares| {
            let mut characters = serde_json::Map::new();
            let mut words = serde_json::Map::new();
            for kind in KINDS {
                characters.insert(
                    kind_name(kind).to_owned(),
                    round(shares.percent(kind)).into(),
                );
                words.insert(
                    kind_name(kind).to_owned(),
                    round(shares.word_percent(kind)).into(),
                );
            }
            serde_json::json!({
                "characters_percent": characters,
                "words_percent": words,
                "characters": shares.total_chars(),
                "words": shares.total_words(),
            })
        };
        let sections: Vec<serde_json::Value> = counted
            .sections
            .iter()
            .map(|(title, level, line, section)| {
                serde_json::json!({
                    "title": title, "level": level, "line": line, "shares": shares(section)
                })
            })
            .collect();
        let mut value = serde_json::json!({
            "revision": revision,
            "shares": shares(&counted.whole),
            "agents_characters": counted.whole.agents,
            "sections": sections,
            "counted": "letters, digits and signs of the text; not markup, code, comments, \
                        labels, references or blanks; an equation is one word and one character",
        });
        if let Some((from, to)) = lines {
            let start = super::agent::offset_of(text, from.max(1), 1).unwrap_or(text.len());
            let end = super::agent::offset_of(text, to.saturating_add(1), 1).unwrap_or(text.len());
            let mut runs = Vec::new();
            for (range, mark) in origins.map().within(start..end.max(start)) {
                if runs.len() >= 500 {
                    break;
                }
                let credited = origins.credited(mark);
                let (from_line, from_column) = super::agent::line_column(text, range.start);
                let (to_line, to_column) = super::agent::line_column(text, range.end);
                runs.push(serde_json::json!({
                    "from_line": from_line,
                    "from_column": from_column,
                    "to_line": to_line,
                    "to_column": to_column,
                    "kind": kind_name(origins.kind(mark)),
                    "by": authorship::who(&credited.by),
                    "accepted": matches!(credited.by.origin, Origin::Accepted(_)),
                    "when": (credited.by.when > 0).then(|| authorship::iso(credited.by.when)),
                    "over": credited.over.as_ref().map(authorship::who),
                    "text": text.get(range).unwrap_or_default(),
                }));
            }
            value["runs"] = serde_json::Value::Array(runs);
        }
        Ok(value)
    }
}

fn round(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// How the agents' share (as written, and edited by the author since)
/// of the source's characters moved over the line of the history the
/// text is on: up to 24 states, from the first to the current, each with
/// its number of steps from the first.
fn moved(editor: &crate::editor::Editor) -> Moved {
    let history = editor.history();
    let line: Vec<_> = {
        let mut line = Vec::new();
        let mut at = history.current();
        line.push(at);
        while let Some(parent) = history.parent(at) {
            line.push(parent);
            at = parent;
        }
        line.reverse();
        line
    };
    if line.len() < 2 {
        return Vec::new();
    }
    let wanted = 24.min(line.len());
    let picked: Vec<usize> = (0..wanted)
        .map(|step| step * (line.len() - 1) / (wanted - 1).max(1))
        .collect();
    let origins = editor.origins();
    let mut map = origins.map().clone();
    let mut text = editor.text().to_owned();
    let mut at = history.current();
    let mut shares = Vec::new();
    for &index in picked.iter().rev() {
        let state = line[index];
        for step in history.steps(at, state).unwrap_or_default() {
            let ordered: Vec<&zetwerk_core::Change> = match step.direction {
                zetwerk_core::Direction::Forward => step.changes.iter().collect(),
                zetwerk_core::Direction::Backward => step.changes.iter().rev().collect(),
            };
            for change in ordered {
                let (gone, come) = match step.direction {
                    zetwerk_core::Direction::Forward => (&change.old, &change.new),
                    zetwerk_core::Direction::Backward => (&change.new, &change.old),
                };
                if text.get(change.at..change.at + gone.len()) == Some(gone.as_str()) {
                    text.replace_range(change.at..change.at + gone.len(), come);
                }
            }
        }
        origins.walked(history, &mut map, at, state);
        at = state;
        let (mut agent, mut edited, mut all) = (0, 0, 0);
        for (range, mark) in map.iter() {
            let Some(bytes) = text.get(range) else {
                continue;
            };
            let count = bytes.chars().filter(|c| !c.is_whitespace()).count();
            all += count;
            match origins.kind(mark) {
                Kind::Agent => agent += count,
                Kind::Edited => edited += count,
                _ => {}
            }
        }
        let all = all.max(1) as f64;
        shares.push((
            index,
            agent as f64 * 100.0 / all,
            edited as f64 * 100.0 / all,
        ));
    }
    shares.reverse();
    shares
}

/// What colours the bytes of a page's glyphs.
struct Painter<'a> {
    origins: &'a zetwerk_core::Attribution,
    text: &'a str,
    runs: &'a [(Range<usize>, Mark)],
    tint: Tint,
    hide_unknown: bool,
    now: u64,
}

impl Painter<'_> {
    /// The marks of bytes `range`, with how many bytes each.
    fn marks(&self, range: &Range<usize>) -> Vec<(Mark, usize)> {
        let first = self.runs.partition_point(|(run, _)| run.end <= range.start);
        self.runs[first..]
            .iter()
            .take_while(|(run, _)| run.start < range.end)
            .map(|(run, mark)| (*mark, run.end.min(range.end) - run.start.max(range.start)))
            .collect()
    }

    /// The colour of a glyph showing bytes `range` (all of a thing shown
    /// as one, if `one`), and what it stands for.
    fn colour(&self, range: &Range<usize>, one: bool) -> Option<([u8; 4], String)> {
        let marks = self.marks(range);
        let (mark, _) = *marks.first()?;
        let mut kinds = std::collections::BTreeMap::new();
        for (mark, count) in &marks {
            *kinds.entry(self.origins.kind(*mark)).or_insert(0) += count;
        }
        let kind = if one {
            let has = |kind| kinds.contains_key(&kind);
            if kinds.len() > 1
                && (has(Kind::Agent) || has(Kind::Edited))
                && (has(Kind::Own) || has(Kind::Edited))
            {
                Kind::Edited
            } else {
                *kinds.iter().max_by_key(|(_, count)| **count)?.0
            }
        } else {
            self.origins.kind(mark)
        };
        let agent = marks.iter().find_map(|(mark, _)| {
            [Some(mark.by), mark.over]
                .into_iter()
                .flatten()
                .find_map(|id| authorship::agent_name(&self.origins.credit(id).origin))
        });
        match self.tint {
            Tint::Origin => self.by_origin(kind, agent),
            Tint::Unread => {
                let credit = self.origins.credit(mark.by);
                let live = matches!(credit.origin, Origin::Agent(_)) && kind == Kind::Agent;
                if !live || self.taken_in(range.start, credit) {
                    return None;
                }
                self.by_origin(kind, agent)
            }
            Tint::Age => {
                let when = self.origins.credit(mark.by).when;
                let age = self.now.saturating_sub(when);
                let bucket = if when == 0 {
                    4
                } else if age < DAY {
                    0
                } else if age < 7 * DAY {
                    1
                } else if age < 31 * DAY {
                    2
                } else if age < 365 * DAY {
                    3
                } else {
                    4
                };
                Some((AGES[bucket], AGE_NAMES[bucket].to_owned()))
            }
        }
    }

    fn by_origin(&self, kind: Kind, agent: Option<String>) -> Option<([u8; 4], String)> {
        let agent_name = agent.unwrap_or_else(|| "an agent".to_owned());
        match kind {
            Kind::Own => None,
            Kind::Agent => Some((with_alpha(hue(&agent_name), AGENT_ALPHA), agent_name)),
            Kind::Edited => Some((
                with_alpha(hue(&agent_name), EDITED_ALPHA),
                format!("{agent_name}, edited by you"),
            )),
            Kind::Pasted => Some((PASTED, "pasted".to_owned())),
            Kind::Outside => Some((OUTSIDE, "changed outside zetwerk".to_owned())),
            Kind::Unknown if self.hide_unknown => None,
            Kind::Unknown => Some((UNKNOWN, "unknown".to_owned())),
        }
    }

    /// Whether the author took in the agent's text at byte `at`, written
    /// with `credit`: by editing its paragraph since it was written.
    fn taken_in(&self, at: usize, credit: &Credit) -> bool {
        let before = self.text.get(..at).unwrap_or_default();
        let start = before.rfind("\n\n").map_or(0, |found| found + 2);
        let end = self
            .text
            .get(at..)
            .and_then(|after| after.find("\n\n"))
            .map_or(self.text.len(), |found| at + found);
        self.origins.map().within(start..end).any(|(_, mark)| {
            let kind = self.origins.kind(mark);
            matches!(kind, Kind::Own | Kind::Edited)
                && self.origins.credit(mark.by).when >= credit.when
        })
    }
}

/// Adds `cell` in `colour` to `cells`: to the last of them, if it goes on
/// where that ends in the same colour.
fn join(cells: &mut Vec<(Rect, [u8; 4])>, cell: Rect, colour: [u8; 4]) {
    let slack = typst::layout::Abs::pt(0.5);
    match cells.last_mut() {
        Some((last, of))
            if *of == colour
                && (last.min.y - cell.min.y).abs() < slack
                && (last.max.y - cell.max.y).abs() < slack
                && cell.min.x >= last.min.x - slack
                && cell.min.x <= last.max.x + slack =>
        {
            last.max.x = last.max.x.max(cell.max.x);
        }
        _ => cells.push((cell, colour)),
    }
}
