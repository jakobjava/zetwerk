//! What is being typed, shown where it is typed.
//!
//! The page is typeset by Typst, and Typst is not asked again for every
//! key. Words typed into a paragraph are worked into the page as it is
//! (see `zetwerk_predict`). Everything else that is typed — a new
//! paragraph, a heading, markup, an equation — is typeset by itself and
//! laid over the page at the place it is typed at, until the page has it:
//! as what it will look like where that can be told, as the characters
//! typed where it cannot (half an equation is no equation).
//!
//! An equation that is being changed is typeset anew over the old one,
//! with the cursor in it: what it still lacks (the two parts of a
//! fraction just begun) shows as empty boxes, and a word that is not yet
//! the name of a symbol as the word.

use std::ops::Range;

use typst::layout::Rect;
use zetwerk_typst::PageMap;

use super::chrome::{Image, InPlace, TextEnd};
use super::{PAGE_BAR, PAPER, Quad, Scene, TextureId, Viewer, chrome, equation_at, solid, tint};
use crate::editor;
use crate::typst_text::{EMPTY, empty_places};

/// How often what keeps a piece from being typeset is put right before
/// it is shown as it is written.
const TRIES: usize = 4;

/// A piece as it is laid over the page.
struct Piece {
    image: Image,
    /// Where its text ends.
    end: TextEnd,
    /// Where the cursor is in it, if that could be told.
    caret: Option<TextEnd>,
}

/// `source` with `EMPTY` put in at each of `places` (in order), and
/// where byte `at` of it is then: before what was put in there.
fn filled(source: &str, at: usize, places: &[usize]) -> (String, usize) {
    let mut text = String::with_capacity(source.len() + places.len() * EMPTY.len());
    let (mut from, mut cursor) = (0, at);
    for place in places
        .iter()
        .copied()
        .filter(|place| *place <= source.len())
    {
        text.push_str(&source[from..place]);
        text.push_str(EMPTY);
        from = place;
        if place < at {
            cursor += EMPTY.len();
        }
    }
    text.push_str(&source[from..]);
    (text, cursor)
}

/// Makes bytes `name` of `text` a word that is shown as it is, and moves
/// `cursor` with what it stood by.
fn quote(text: &mut String, cursor: &mut usize, name: &Range<usize>) {
    if !(text.is_char_boundary(name.start) && text.is_char_boundary(name.end)) {
        return;
    }
    text.insert(name.end, '"');
    text.insert(name.start, '"');
    *cursor += match *cursor {
        at if at >= name.end => 2,
        at if at > name.start => 1,
        _ => 0,
    };
}

/// How far under a line a paragraph begun behind it is shown, in parts
/// of that line's height.
const NEW_BLOCK: f64 = 0.9;
/// The room a piece set in place has at least, in points.
const LEAST_ROOM: f64 = 60.0;

/// Where on a page something is being typed.
#[derive(Clone, Copy)]
enum Site {
    /// Over the equation that stands there, which is being changed.
    Over(Rect),
    /// At a place in a line: its left edge and its top and bottom, in
    /// points; or, `below`, as a new paragraph under that line, at `left`.
    At {
        x: f64,
        top: f64,
        bottom: f64,
        left: f64,
        below: bool,
    },
}

/// What is being typed on a page: which bytes of the text, where they
/// belong on the page, and how the text there is set.
struct Typed {
    shown: Range<usize>,
    site: Site,
    family: String,
    size: f64,
    weight: u16,
    italic: bool,
    /// The room for it, in points.
    room: f64,
}

impl Typed {
    fn over(&self) -> bool {
        matches!(self.site, Site::Over(_))
    }

    /// How it is to be set: like the text it stands in; an equation in
    /// Typst's own way.
    fn style(&self) -> InPlace<'_> {
        InPlace {
            family: (!self.over()).then_some(self.family.as_str()),
            size: self.size,
            weight: self.weight.max(100),
            italic: self.italic && !self.over(),
            width: self.room,
        }
    }
}

impl Viewer {
    /// What is being typed on page `index`, if something is that the
    /// page may not show: an equation that stands on it and is being
    /// changed, or else what was put into the text since the page was
    /// made.
    fn typed_on(&mut self, index: usize) -> Option<Typed> {
        let change = self.places_of_text().filter(|_| self.predicted.is_none())?;
        let editor = self.editor.as_ref()?;
        let (document, world) = (self.document.as_ref()?, self.compiled_world.as_ref()?);
        if editor.mode() != editor::Mode::Insert {
            return None;
        }
        let page = document.pages().get(index)?;
        let (file, text, cursor) = (editor.file(), editor.text(), editor.cursor());
        let map = self
            .maps
            .entry(index)
            .or_insert_with(|| PageMap::build(world, &page.frame));
        let equation = equation_at(editor.source(), cursor);
        let standing = equation.as_ref().and_then(|equation| {
            let start = change.back(equation.start)?;
            let end = change.back_or_edge(equation.end).max(start + 1);
            let cells = map.selection(file, &(start..end));
            let around = |a: Rect, b: &Rect| Rect::new(a.min.min(b.min), a.max.max(b.max));
            let first = *cells.first()?;
            Some((
                cells.iter().fold(first, around),
                map.caret(file, start)?.run,
            ))
        });
        let (shown, site, run) = match (equation, standing) {
            (Some(equation), Some((rect, run))) => (equation, Site::Over(rect), run),
            _ if change.inserted > 0 => {
                // At the place it was put in at; or, where that shows as
                // nothing (the end of a paragraph), behind what is
                // before it.
                let at = map.caret(file, change.offset);
                let caret = at.or_else(|| map.caret_before(file, change.offset))?;
                let cell = caret.cell;
                let put_in = change.offset..change.offset + change.inserted;
                let below = text.get(put_in.clone())?.starts_with('\n');
                // Blanks at the end of what the page has show as nothing
                // there: they are shown with what follows them now.
                let before = text.get(..put_in.start)?;
                let blanks = before.len() - before.trim_end_matches(' ').len();
                let lead = if at.is_none() && !below { blanks } else { 0 };
                let put_in = put_in.start - lead..put_in.end;
                // The line it is in begins where its paragraph does.
                let lines = self.page_lines.get_mut();
                let lines = lines.entry(index).or_insert_with(|| map.lines());
                let left = lines
                    .find(file, change.offset.saturating_sub(1))
                    .and_then(|(line, _)| lines.extent(line))
                    .map_or(cell.min.x.to_pt(), |(left, _)| left);
                let site = Site::At {
                    x: if caret.after { cell.max.x } else { cell.min.x }.to_pt(),
                    top: cell.min.y.to_pt(),
                    bottom: cell.max.y.to_pt(),
                    left,
                    below,
                };
                (put_in, site, caret.run)
            }
            _ => return None,
        };
        // Without the line breaks around it: where it goes says that.
        let all = text.get(shown.clone())?;
        let lead = all.len() - all.trim_start_matches('\n').len();
        let length = all.trim_matches('\n').len();
        let shown = shown.start + lead..shown.start + lead + length;
        let (start, left) = match site {
            Site::Over(rect) => (rect.min.x.to_pt(), rect.min.x.to_pt()),
            Site::At { x, left, below, .. } => (if below { left } else { x }, left),
        };
        let run = map.runs().get(run);
        Some(Typed {
            shown,
            site,
            family: run.map_or_else(String::new, |run| run.family.clone()),
            size: run.map_or(11.0, |run| run.size),
            weight: run.map_or(400, |run| run.weight),
            italic: run.is_some_and(|run| run.italic),
            // As far as the text reaches on the other side of the page.
            room: (page.frame.width().to_pt() - left - start).max(LEAST_ROOM),
        })
    }

    /// Typesets `source` for its place, as it will look: an equation
    /// (`places` are where it is empty) with what it lacks shown as
    /// boxes, and words that name nothing as words. What cannot be
    /// typeset even so is shown as the characters it is. `at` is the byte
    /// of `source` the cursor is at.
    fn typeset_in_place(
        &mut self,
        typed: &Typed,
        source: &str,
        at: usize,
        places: Option<&[usize]>,
    ) -> Option<Piece> {
        let zoom = self.view.zoom;
        let style = typed.style();
        let (mut text, mut cursor) = filled(source, at, places.unwrap_or_default());
        if text.starts_with(' ') {
            // A blank that a piece begins with would not show: one that
            // does, as wide.
            text.replace_range(0..1, "\u{a0}");
            cursor += usize::from(at > 0);
        }
        for _ in 0..TRIES {
            let markup = chrome::in_place_markup(&text, &style, false);
            if let Some((image, end)) = self.chrome.render_in_place(&markup, zoom) {
                let caret = chrome::in_place_body(&markup)
                    .and_then(|body| self.chrome.caret_in_place(&markup, zoom, body + cursor));
                return Some(Piece { image, end, caret });
            }
            // A name that is none (yet): the word it is.
            let body = chrome::in_place_body(&markup)?;
            let mut names: Vec<Range<usize>> = self
                .chrome
                .unknown_in(&markup)
                .into_iter()
                .filter(|name| name.start >= body && name.end <= body + text.len())
                .map(|name| name.start - body..name.end - body)
                .collect();
            if names.is_empty() || places.is_none() {
                break;
            }
            names.sort_by_key(|name| std::cmp::Reverse(name.start));
            names.dedup();
            for name in &names {
                quote(&mut text, &mut cursor, name);
            }
        }
        if places.is_some() {
            // An equation that is none is not shown as what it is
            // written as: the page keeps it as it was, and the status
            // line says what is wrong with it.
            return None;
        }
        let markup = chrome::in_place_markup(source, &style, true);
        let (image, end) = self.chrome.render_in_place(&markup, zoom)?;
        // The cursor: behind the characters before it.
        let before = source.get(..at).unwrap_or(source);
        let markup = chrome::in_place_markup(before, &style, true);
        let caret = self.chrome.render_in_place(&markup, zoom);
        Some(Piece {
            image,
            end,
            caret: caret.map(|(_, end)| end).filter(|_| !before.is_empty()),
        })
    }

    /// Draws on page `index` (whose top-left corner is at `origin`) what
    /// is being typed, where it is typed, if that is on this page and
    /// the page does not show it. Returns whether it did: the page's own
    /// cursor has no say then.
    pub(super) fn in_place(
        &mut self,
        scene: &mut Scene,
        has: &dyn Fn(TextureId) -> bool,
        index: usize,
        origin: (f64, f64),
    ) -> bool {
        let zoom = self.view.zoom;
        if self.document_revision == Some(self.revision) {
            // The page shows what was typed, where it belongs.
            return false;
        }
        if !self.is_compiled() && self.stats.compile_ms <= super::QUICK_COMPILE_MS {
            // It is about to: nothing is shown in between that would
            // only be in the way a moment later.
            return false;
        }
        let Some(typed) = self.typed_on(index) else {
            return false;
        };
        let window = |x: f64, y: f64| (origin.0 + x * zoom, origin.1 + y * zoom);
        let Some(editor) = &self.editor else {
            return false;
        };
        let cursor = editor.cursor();
        let shown = typed.shown.clone();
        let Some(source) = editor.text().get(shown.clone()).map(str::to_owned) else {
            return false;
        };
        // The equation the cursor is in, and where it is empty.
        let places = equation_at(editor.source(), cursor)
            .filter(|equation| shown.start <= equation.start && equation.end <= shown.end)
            .map(|equation| {
                empty_places(editor.source(), &equation)
                    .into_iter()
                    .map(|place| place - shown.start)
                    .collect::<Vec<usize>>()
            });
        let at = cursor.saturating_sub(shown.start).min(source.len());
        let (Some(piece), place) = (
            self.typeset_in_place(&typed, &source, at, places.as_deref()),
            typed.site,
        ) else {
            return false;
        };
        let Piece { image, end, caret } = piece;
        self.equation_set = false;
        let size = (
            f64::from(image.pixmap.width()),
            f64::from(image.pixmap.height()),
        );
        let (x, y, line) = match place {
            Site::Over(rect) => {
                // Over the old one, which goes; in its middle.
                let (left, top) = window(rect.min.x.to_pt(), rect.min.y.to_pt());
                let (right, bottom) = window(rect.max.x.to_pt(), rect.max.y.to_pt());
                let old = (
                    left - 2.0,
                    top - 2.0,
                    right - left + 4.0,
                    bottom - top + 4.0,
                );
                solid(scene, has, PAPER, old);
                let x = (left + right - size.0) / 2.0;
                (x, (top + bottom - size.1) / 2.0, bottom - top)
            }
            Site::At {
                x,
                top,
                bottom,
                left,
                below,
            } => {
                let height = bottom - top;
                let (x, y) = if below {
                    window(left, bottom + NEW_BLOCK * height)
                } else {
                    window(x, top)
                };
                if below {
                    // A paragraph of its own has its lines to itself:
                    // what the page has there (the paragraph that will
                    // be moved down) is not shown through it.
                    let room = (typed.room * zoom, size.1.max(height * zoom) + 2.0);
                    solid(scene, has, PAPER, (x, y - 1.0, room.0, room.1));
                }
                (x, y, height * zoom)
            }
        };
        scene.quads.push(Quad {
            texture: image.id,
            x: x.round() as f32,
            y: y.round() as f32,
            width: size.0 as f32,
            height: size.1 as f32,
        });
        if !has(image.id) {
            scene.images.push(image);
        }
        // The cursor: where it is in the piece; behind all of it, if
        // that cannot be told and it is behind what was typed.
        let caret = caret.or_else(|| (cursor >= shown.end).then_some(end));
        let (across, top, height) = match caret {
            Some(caret) if caret.bottom > caret.top => {
                (caret.x, caret.top, caret.bottom - caret.top)
            }
            _ => (0.0, 0.0, line),
        };
        let bar = (1.5 * self.scale).round().max(1.0);
        tint(
            scene,
            has,
            PAGE_BAR,
            ((x + across).round(), y + top, bar, height),
        );
        self.cursor_at = Some((x, y, y + size.1.max(line)));
        true
    }
}
