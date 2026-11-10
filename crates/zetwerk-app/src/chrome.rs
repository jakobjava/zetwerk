//! The editor's own interface: status line and overlays.
//!
//! Chrome is typeset by Typst, like the document. A status line is a tiny
//! Typst document compiled in memory and rasterized by the same renderer as
//! the pages, so there is one text stack in the program, not two.

use std::collections::HashMap;
use std::fmt::Write;
use std::path::PathBuf;
use std::sync::Arc;

use tiny_skia::Pixmap;
use typst::utils::hash128;
use zetwerk_typst::{Environment, Project};

use crate::gpu::TextureId;

/// Height of the status line in points (one point is one logical pixel).
pub const STATUS_HEIGHT: f64 = 20.0;

const BACKGROUND: &str = "#282828";
const FOREGROUND: &str = "#ebdbb2";
const ACCENT: &str = "#fabd2f";
const ALERT: &str = "#fb4934";
const FONT: &str = "DejaVu Sans Mono";

/// How many chrome renders a result in Typst's cache may go unused before
/// it is dropped. The cache is shared with the document (see
/// [`zetwerk_typst::forget_unused`]): this has to be far more than the
/// status line changes between two compiles of the document, or scrolling
/// through a long document would cost it its incremental compile. It still
/// bounds what chrome leaves behind over a long session.
const UNUSED_ROUNDS: usize = 2000;

/// How many rendered pieces of chrome are kept. The status line changes
/// with every page scrolled past; a handful covers flipping back and forth.
const CACHE_SIZE: usize = 32;

/// A rendered piece of chrome.
#[derive(Clone)]
pub struct Image {
    pub id: TextureId,
    pub pixmap: Arc<Pixmap>,
}

/// Renders chrome markup to images.
pub struct Chrome {
    project: Project,
    cache: HashMap<TextureId, (u64, Arc<Pixmap>)>,
    /// Counts renders, to find the least recently used cache entry.
    clock: u64,
}

impl Chrome {
    /// Sets up chrome rendering with the fonts of `env`.
    #[must_use]
    pub fn new(env: Arc<Environment>) -> Option<Self> {
        // Chrome reads no files; the root only has to be a path.
        let project = Project::in_memory(env, PathBuf::from("/"), "chrome.typ", String::new())?;
        Some(Self {
            project,
            cache: HashMap::new(),
            clock: 0,
        })
    }

    /// Typesets `markup` and rasterizes its first page at `scale` pixels per
    /// point. `None` if the markup does not compile, which for the fixed
    /// templates in this module would be a bug.
    pub fn render(&mut self, markup: &str, scale: f64) -> Option<Image> {
        self.clock += 1;
        let id = TextureId(hash128(&(markup, scale.to_bits())));
        if let Some((used, pixmap)) = self.cache.get_mut(&id) {
            *used = self.clock;
            return Some(Image {
                id,
                pixmap: Arc::clone(pixmap),
            });
        }
        self.project.set_main_text(markup);
        let document = zetwerk_typst::compile(&self.project.snapshot()).document;
        zetwerk_typst::forget_unused(UNUSED_ROUNDS);
        let document = document?;
        let pixmap = Arc::new(zetwerk_render::render_page(
            document.pages().first()?,
            scale,
        ));
        if self.cache.len() >= CACHE_SIZE
            && let Some(oldest) = self
                .cache
                .iter()
                .min_by_key(|(_, (used, _))| *used)
                .map(|(id, _)| *id)
        {
            self.cache.remove(&oldest);
        }
        self.cache.insert(id, (self.clock, Arc::clone(&pixmap)));
        Some(Image { id, pixmap })
    }
}

/// What the status line shows.
pub struct Status<'a> {
    /// The mode tag at the left edge.
    pub mode: &'a str,
    /// File name, message or the command being typed.
    pub left: &'a str,
    /// Position and zoom at the right edge.
    pub right: &'a str,
    /// Whether `left` reports a problem.
    pub alert: bool,
}

/// Markup for a status line `width` points wide.
#[must_use]
pub fn status_markup(status: &Status, width: f64) -> String {
    let left_color = if status.alert { ALERT } else { FOREGROUND };
    format!(
        r#"#set page(width: {width}pt, height: {STATUS_HEIGHT}pt, margin: (x: 8pt, y: 0pt), fill: rgb("{BACKGROUND}"))
#set text(font: "{FONT}", size: 10pt, fill: rgb("{FOREGROUND}"))
#set align(horizon)
#grid(
  columns: (auto, 1fr, auto),
  column-gutter: 10pt,
  text(weight: "bold", fill: rgb("{ACCENT}"), {mode}),
  box(width: 100%, height: 12pt, clip: true, text(fill: rgb("{left_color}"), {left})),
  {right},
)"#,
        mode = string(status.mode),
        left = string(status.left),
        right = string(status.right),
    )
}

/// Markup for a panel listing `rows` of label and value.
#[must_use]
pub fn panel_markup(rows: &[(&str, String)]) -> String {
    let mut cells = String::new();
    for (label, value) in rows {
        let _ = writeln!(
            cells,
            "  {}, align(right, {}),",
            string(label),
            string(value)
        );
    }
    format!(
        r#"#set page(width: auto, height: auto, margin: 8pt, fill: rgb("{BACKGROUND}"))
#set text(font: "{FONT}", size: 9pt, fill: rgb("{FOREGROUND}"))
#grid(
  columns: 2,
  column-gutter: 16pt,
  row-gutter: 5pt,
{cells})"#
    )
}

/// `text` as a Typst string literal. Shown through a string, text is never
/// interpreted as markup, whatever a file name or message contains.
fn string(text: &str) -> String {
    let mut literal = String::with_capacity(text.len() + 2);
    literal.push('"');
    for c in text.chars() {
        match c {
            '\\' => literal.push_str("\\\\"),
            '"' => literal.push_str("\\\""),
            '\n' => literal.push_str("\\n"),
            '\r' => literal.push_str("\\r"),
            '\t' => literal.push_str("\\t"),
            c => literal.push(c),
        }
    }
    literal.push('"');
    literal
}

#[cfg(test)]
mod tests {
    use zetwerk_typst::FontSet;

    use super::*;

    fn chrome() -> Chrome {
        Chrome::new(Environment::new(FontSet::Embedded)).unwrap_or_else(|| panic!("chrome sets up"))
    }

    #[test]
    fn status_line_has_the_requested_size() {
        let status = Status {
            mode: "NORMAL",
            left: "thesis.typ",
            right: "3/12",
            alert: false,
        };
        let image = chrome().render(&status_markup(&status, 640.0), 1.5);
        let pixmap = image
            .unwrap_or_else(|| panic!("status line compiles"))
            .pixmap;
        assert_eq!((pixmap.width(), pixmap.height()), (960, 30));
        // Text was drawn: not every pixel is background.
        let first = pixmap.pixels()[0];
        assert!(pixmap.pixels().iter().any(|pixel| *pixel != first));
    }

    #[test]
    fn hostile_text_is_shown_not_interpreted() {
        let mut chrome = chrome();
        for text in [
            "#panic()",
            "\"]) #pagebreak() #(\"",
            "a\\",
            "$x$ *b* \n = c",
            "</>",
        ] {
            let status = Status {
                mode: text,
                left: text,
                right: text,
                alert: true,
            };
            assert!(
                chrome.render(&status_markup(&status, 300.0), 1.0).is_some(),
                "status line with {text:?} did not compile"
            );
            assert!(
                chrome
                    .render(&panel_markup(&[(text, text.to_owned())]), 1.0)
                    .is_some()
            );
        }
    }

    #[test]
    fn long_text_does_not_grow_the_status_line() {
        let long = "word ".repeat(400);
        let status = Status {
            mode: "NORMAL",
            left: &long,
            right: "1/1",
            alert: false,
        };
        let image = chrome().render(&status_markup(&status, 300.0), 1.0);
        let pixmap = image
            .unwrap_or_else(|| panic!("status line compiles"))
            .pixmap;
        assert_eq!((pixmap.width(), pixmap.height()), (300, 20));
    }

    #[test]
    fn equal_markup_is_rendered_once() {
        let mut chrome = chrome();
        let markup = panel_markup(&[("compile", "1.5 ms".to_owned())]);
        let first = chrome
            .render(&markup, 2.0)
            .unwrap_or_else(|| panic!("panel compiles"));
        let again = chrome
            .render(&markup, 2.0)
            .unwrap_or_else(|| panic!("panel compiles"));
        assert_eq!(first.id, again.id);
        assert!(Arc::ptr_eq(&first.pixmap, &again.pixmap));
        let other = chrome
            .render(&markup, 1.0)
            .unwrap_or_else(|| panic!("panel compiles"));
        assert_ne!(first.id, other.id);
    }
}
