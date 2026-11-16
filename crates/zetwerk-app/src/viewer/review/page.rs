//! Suggestions on the page: their text marked faintly, in a colour of
//! their kind, and the one under the cursor with a slip beside it that
//! says what it is, why, and the keys.
//!
//! Where on a page the marks go is worked out once for each state of the
//! suggestions, of the pages and of the text (see [`Placed`]), as the
//! matches of a search are: a frame draws what was worked out, so that a
//! hundred suggestions cost a frame no more than one. The change itself
//! is shown typeset in place by the preview of proposals (see
//! [`super::super::notes`]), which this leaves to it.

use std::fmt::Write as _;

use typst::layout::Rect;
use zetwerk_typst::PageMap;

use super::super::assist::cut;
use super::super::notes::{literal, wrap};
use super::super::{Change, LENS_EDGE, PANEL_MARGIN, Quad, Scene, TextureId, Viewer, solid, tint};
use super::{Kind, Level, Marks, Said, State, Suggestion, Who, now};
use crate::editor;

/// The marks of suggestions by kind: faint over the text, and a line
/// under it; stronger for the one under the cursor.
const REPLACE: [u8; 3] = [0xd7, 0x99, 0x21];
const INSERT: [u8; 3] = [0x79, 0x74, 0x0e];
const DELETE: [u8; 3] = [0xcc, 0x24, 0x1d];
const MOVE: [u8; 3] = [0xb1, 0x62, 0x86];
const REMARK: [u8; 3] = [0x45, 0x85, 0x88];
const PUT_ASIDE: [u8; 3] = [0x92, 0x83, 0x74];
/// How strongly a mark is laid over the text, and its line, for the
/// others and for the one under the cursor.
const FILL: u8 = 0x2a;
const FILL_FRONT: u8 = 0x50;
const LINE: u8 = 0x80;
const LINE_FRONT: u8 = 0xe0;
/// The colours of the slip, as the panels of reviewing have them.
const BACKGROUND: &str = "#282828";
const FOREGROUND: &str = "#ebdbb2";
const DIM: &str = "#928374";
const ACCENT: &str = "#fabd2f";
const WARN: &str = "#fb4934";
const FONT: &str = "DejaVu Sans Mono";
/// How wide the slip is in characters, and how many lines of the reason
/// it shows.
const COLUMNS: usize = 62;
const REASON_LINES: usize = 4;
/// How far the cards of the suggestions behind the one in front stand
/// out from under it, in pixels at scale 1.
const STACKED: f64 = 4.0;

/// The marks on one page: the cells of the text of each suggestion, in
/// points, with its number and colour.
pub(in super::super) struct Placed {
    /// What they were worked out for: the suggestions, the document
    /// drawn, and how the text differs from the one it was compiled from.
    key: (u64, usize, Change),
    marks: Vec<(u64, [u8; 3], Rect)>,
}

/// The colour of a suggestion's mark.
pub(super) fn colour(suggestion: &Suggestion) -> [u8; 3] {
    if suggestion.state == State::Later || suggestion.stale {
        return PUT_ASIDE;
    }
    match suggestion.kind {
        Kind::Replace => REPLACE,
        Kind::Insert => INSERT,
        Kind::Delete => DELETE,
        Kind::Move => MOVE,
        Kind::Comment | Kind::Question => REMARK,
    }
}

/// The colour of a suggestion's kind, as markup.
fn stripe(suggestion: &Suggestion) -> String {
    let [r, g, b] = colour(suggestion);
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// How long ago `at` was, in words.
fn ago(at: u64) -> String {
    let seconds = now().saturating_sub(at);
    match seconds {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86_400 => format!("{} h ago", seconds / 3600),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

/// The keys for a suggestion of `kind`.
fn keys(kind: Kind, versions: bool) -> &'static str {
    match (kind.changes(), versions) {
        (false, _) => "ga done · gr dismiss · gq answer · gz later · g! always · gR all",
        (true, false) => {
            "ga accept · gr reject · gA rewrite · gq answer · gp other wordings · gz later · zp as it is"
        }
        (true, true) => {
            "zp next wording · ga accept this one · gr reject · gA rewrite · gq answer · gz later"
        }
    }
}

/// What the slip of `suggestion` says, line by line, each with whether it
/// warns.
fn slip_lines(
    suggestion: &Suggestion,
    flagged: &[(u64, String)],
    typeset: bool,
) -> Vec<(String, bool)> {
    let mut lines = Vec::new();
    for line in wrap(&suggestion.reason, COLUMNS, REASON_LINES) {
        if !line.is_empty() {
            lines.push((line, false));
        }
    }
    if suggestion.kind.changes() && !typeset {
        // Where the change cannot be set in place (a heading, an
        // equation), it is said in words.
        let change = match suggestion.kind {
            Kind::Insert => format!("puts in “{}”", cut(&suggestion.new, COLUMNS - 12)),
            Kind::Delete => format!("takes out “{}”", cut(&suggestion.quote, COLUMNS - 14)),
            _ => format!(
                "“{}” → “{}”",
                cut(&suggestion.quote, COLUMNS / 2 - 4),
                cut(&suggestion.new, COLUMNS / 2 - 4)
            ),
        };
        lines.push((change, false));
    }
    if suggestion.alternatives.len() + 1 > 1 {
        lines.push((
            format!(
                "wording {} of {} · zp the next",
                suggestion.shown + 1,
                suggestion.alternatives.len() + 1
            ),
            false,
        ));
    }
    let last = suggestion.thread.iter().rev().find(|entry| {
        matches!(
            entry.said,
            Said::Message(_)
                | Said::Version { .. }
                | Said::Alternatives(_)
                | Said::AskedAlternatives
        )
    });
    match (last, suggestion.waiting) {
        (Some(entry), true) if entry.by == Who::Author => {
            let said = match &entry.said {
                Said::Message(text) => format!("you said “{}”", cut(text, COLUMNS - 30)),
                _ => "you asked for other wordings".to_owned(),
            };
            lines.push((format!("waiting for {} — {said}", suggestion.by), false));
        }
        (Some(entry), _) if entry.by != Who::Author => {
            let said = match &entry.said {
                Said::Message(text) => format!("“{}”", cut(text, COLUMNS - 12)),
                Said::Version { .. } => "a new version".to_owned(),
                _ => "other wordings".to_owned(),
            };
            lines.push((format!("{}: {said}", suggestion.by), false));
        }
        _ => {}
    }
    for (id, words) in flagged {
        lines.push((
            format!(
                "goes against your instruction {id}: “{}”",
                cut(words, COLUMNS - 36)
            ),
            true,
        ));
    }
    if suggestion.stale {
        lines.push(("its text has changed since: gr drops it".to_owned(), true));
    }
    lines
}

/// Markup for the slip: a stripe of the kind's colour, a title, lines
/// (some warning), who and when, and the keys.
fn slip_markup(
    stripe: &str,
    title: &str,
    lines: &[(String, bool)],
    who: &str,
    hint: &str,
) -> String {
    let mut stacked = String::new();
    for (line, warns) in lines {
        let colour = if *warns { WARN } else { FOREGROUND };
        let _ = writeln!(
            stacked,
            "  text(fill: rgb(\"{colour}\"), {}) + h(0pt),",
            literal(line)
        );
    }
    format!(
        "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n\
         #set text(font: \"{FONT}\", size: 8.5pt, fill: rgb(\"{FOREGROUND}\"), ligatures: false, hyphenate: false)\n\
         #block(fill: rgb(\"{BACKGROUND}\"), stroke: (left: 2.5pt + rgb(\"{stripe}\")), inset: (left: 8pt, rest: 6pt), radius: (right: 2pt), \
         stack(\n  spacing: 5pt,\n  text(fill: rgb(\"{ACCENT}\"), {}),\n{stacked}  text(fill: rgb(\"{DIM}\"), {}),\n  text(fill: rgb(\"{DIM}\"), size: 7.5pt, {}),\n))",
        literal(title),
        literal(who),
        literal(hint)
    )
}

impl Viewer {
    /// Whether the suggestion under the cursor is shown by reviewing,
    /// with its slip: no other panel is wanted for its proposal.
    pub(in super::super) fn review_explains(&self) -> bool {
        self.review.manager.is_none()
            && self
                .editor
                .as_ref()
                .is_some_and(|e| e.mode() == editor::Mode::Normal)
            && self.suggestion_here().is_some()
    }

    /// Draws on page `index` (whose top-left corner is at `origin`) the
    /// marks of the suggestions on it.
    pub(in super::super) fn review_marks(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        index: usize,
        origin: (f64, f64),
    ) {
        self.review_ready();
        let file = self.file_id();
        if !self
            .review
            .all
            .iter()
            .any(|s| s.file == file && s.shown_on_text())
        {
            return;
        }
        let Some(change) = self.places_of_text() else {
            return;
        };
        let typed = self
            .predicted
            .as_ref()
            .filter(|predicted| predicted.index == index)
            .map(|predicted| predicted.typed);
        let (Some(document), Some(world)) = (&self.document, &self.compiled_world) else {
            return;
        };
        let Some(page) = document.pages().get(index) else {
            return;
        };
        let key = (
            self.review.generation,
            std::sync::Arc::as_ptr(document) as usize,
            change,
        );
        let stale = self
            .review
            .placed
            .get(&index)
            .is_none_or(|placed| placed.key != key);
        if stale {
            let map = self
                .maps
                .entry(index)
                .or_insert_with(|| PageMap::build(world, &page.frame));
            let mut marks = Vec::new();
            for suggestion in &self.review.all {
                if suggestion.file != file || !suggestion.shown_on_text() {
                    continue;
                }
                let mut range = change.back_or_edge(suggestion.range.start)
                    ..change.back_or_edge(suggestion.range.end);
                if range.is_empty() {
                    // Text to be put in: shown on what stands there now.
                    range.end += 1;
                }
                for rect in map.selection(file, &range) {
                    marks.push((suggestion.id, colour(suggestion), rect));
                }
            }
            self.review.placed.insert(index, Placed { key, marks });
        }
        let front = self.suggestion_here().map(|at| self.review.all[at].id);
        let only_front = self.review.settings.marks != Marks::All;
        let zoom = self.view.zoom;
        let line = (1.2 * self.scale).round().max(1.0);
        let Some(placed) = self.review.placed.get(&index) else {
            return;
        };
        let mut slip: Option<(f64, f64, f64, f64)> = None;
        for (id, [r, g, b], rect) in &placed.marks {
            let on = Some(*id) == front;
            if only_front && !on {
                continue;
            }
            if typed.is_some_and(|typed| rect.min.y < typed.max.y && rect.max.y > typed.min.y) {
                continue;
            }
            let (x, y, w, h) = (
                origin.0 + rect.min.x.to_pt() * zoom,
                origin.1 + rect.min.y.to_pt() * zoom,
                (rect.max.x - rect.min.x).to_pt() * zoom,
                (rect.max.y - rect.min.y).to_pt() * zoom,
            );
            let (fill, under) = if on {
                (FILL_FRONT, LINE_FRONT)
            } else {
                (FILL, LINE)
            };
            if self.review.settings.marks != Marks::None || on {
                tint(scene, has, [*r, *g, *b, fill], (x, y, w, h));
                tint(scene, has, [*r, *g, *b, under], (x, y + h - line, w, line));
            }
            if on {
                slip = Some(
                    slip.map_or((x, y, x + w, y + h), |(left, top, right, bottom)| {
                        (left.min(x), top.min(y), right.max(x + w), bottom.max(y + h))
                    }),
                );
            }
        }
        if slip.is_some() {
            self.review.slip_at = slip;
        }
    }

    /// The slip of the suggestion under the cursor, beside its text; and
    /// the manager, where it is up. Called before the other panels of
    /// reviewing, while it is still known whether the change was set in
    /// place.
    pub(in super::super) fn review_slip(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
    ) {
        let anchor = self.review.slip_at.take();
        if self.editor.is_none() {
            return;
        }
        if self.review.manager.is_some() {
            return self.review_manager_panel(scene, has);
        }
        let normal = self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.mode() == editor::Mode::Normal);
        if !normal || self.is_picking() {
            return;
        }
        let here = self.suggestions_here();
        let Some(&at) = here.first() else {
            return;
        };
        let typeset = self.notes.previewed;
        let suggestion = &self.review.all[at];
        let flagged: Vec<(u64, String)> = suggestion
            .flagged
            .iter()
            .filter_map(|id| {
                let instruction = self.review.brief.iter().find(|i| i.id == *id)?;
                Some((*id, instruction.text.clone()))
            })
            .collect();
        let mut title = format!("{} · {}", suggestion.category, suggestion.kind.name());
        if suggestion.confidence == Level::High {
            title.push_str(" · sure");
        } else if suggestion.confidence == Level::Low {
            title.push_str(" · unsure");
        }
        if suggestion.importance == Level::High {
            title.push_str(" · major");
        }
        if suggestion.state == State::Later {
            title.push_str(" · put aside");
        }
        if here.len() > 1 {
            let _ = write!(title, "   1 of {} here · ]p the next", here.len());
        }
        let lines = slip_lines(suggestion, &flagged, typeset);
        let mut who = format!("{} · {}", suggestion.by, ago(suggestion.made));
        if let Some(request) = suggestion.request {
            let _ = write!(who, " · for your review request {request}");
        }
        let markup = slip_markup(
            &stripe(suggestion),
            &title,
            &lines,
            &who,
            keys(suggestion.kind, !suggestion.alternatives.is_empty()),
        );
        let behind = here.len().saturating_sub(1).min(2);
        let scale = self.scale;
        let Some(image) = self.chrome.render(&markup, scale) else {
            return;
        };
        let (width, height) = (
            f64::from(image.pixmap.width()),
            f64::from(image.pixmap.height()),
        );
        let margin = PANEL_MARGIN * scale;
        let (window_width, window_height) = (f64::from(self.window.0), f64::from(self.window.1));
        let bottom = window_height - f64::from(self.status_height()) - margin;
        let gap = 6.0 * scale;
        let (x, y) = match anchor {
            Some((left, top, _, below)) => {
                let x = left.min(window_width - margin - width).max(margin);
                let y = if top - gap - height >= margin {
                    top - gap - height
                } else {
                    (below + gap).min(bottom - height)
                };
                (x.round(), y.round())
            }
            None => (margin.round(), (bottom - height).round()),
        };
        let step = (STACKED * scale).round();
        for card in (1..=behind).rev() {
            let shift = step * card as f64;
            solid(scene, has, LENS_EDGE, (x + shift, y - shift, width, height));
        }
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
        if let Some(suggestion) = self.review.all.get_mut(at)
            && suggestion.unseen
        {
            suggestion.unseen = false;
        }
    }
}
