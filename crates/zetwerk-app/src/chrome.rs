//! The editor's own interface: status line and overlays.
//!
//! Chrome is typeset by Typst, like the document. A status line is a tiny
//! Typst document compiled in memory and rasterized by the same renderer as
//! the pages, so there is one text stack in the program, not two.

use std::collections::HashMap;
use std::fmt::Write;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use tiny_skia::Pixmap;
use typst::utils::hash128;
use typst::{World, WorldExt};
use zetwerk_typst::{Environment, PageMap, Project};

use crate::gpu::TextureId;

/// Height of the status line in points (one point is one logical pixel).
pub const STATUS_HEIGHT: f64 = 20.0;

/// Height of a row of the source pane in points.
pub const ROW_HEIGHT: f64 = 17.0;
/// Size of the source pane's text in points.
const SOURCE_SIZE: f64 = 10.5;

const BACKGROUND: &str = "#282828";
const FOREGROUND: &str = "#ebdbb2";
const DIM: &str = "#928374";
const ACCENT: &str = "#fabd2f";
const ALERT: &str = "#fb4934";
const ADDED: &str = "#b8bb26";
const CHOSEN: &str = "#504945";
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

/// How many image sizes are remembered before starting over.
const SIZES_KEPT: usize = 20_000;

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
    /// The pixel sizes of images rendered without keeping them: enough to
    /// draw one that the compositor still holds.
    sizes: HashMap<TextureId, (u32, u32)>,
    /// The width of a cell of the source pane in pixels, at the scale
    /// (as bits) it was measured for.
    cell: Option<(u64, f64)>,
    /// Where the text of pieces set in place ends (see
    /// [`Chrome::render_in_place`]).
    ends: HashMap<TextureId, TextEnd>,
    /// Where cursors stand in pieces set in place (see
    /// [`Chrome::caret_in_place`]), by piece and byte.
    carets: HashMap<(TextureId, usize), Option<TextEnd>>,
}

/// Where the text of a typeset piece ends: how far across the piece the
/// last glyph ends, and the top and the bottom of its line, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextEnd {
    pub x: f64,
    pub top: f64,
    pub bottom: f64,
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
            sizes: HashMap::new(),
            cell: None,
            ends: HashMap::new(),
            carets: HashMap::new(),
        })
    }

    /// The name of the image [`Chrome::render`] makes of `markup`.
    #[must_use]
    pub fn id(markup: &str, scale: f64) -> TextureId {
        TextureId(hash128(&(markup, scale.to_bits())))
    }

    /// Renders `markup` as [`Chrome::render`] does, and says where its
    /// text ends: where a cursor behind it belongs.
    pub fn render_in_place(&mut self, markup: &str, scale: f64) -> Option<(Image, TextEnd)> {
        let id = Self::id(markup, scale);
        if let (Some(end), true) = (self.ends.get(&id).copied(), self.cache.contains_key(&id)) {
            return Some((self.render(markup, scale)?, end));
        }
        self.project.set_main_text(markup);
        let document = zetwerk_typst::compile(&self.project.snapshot()).document?;
        let page = document.pages().first()?;
        let mut end = None;
        last_text(&page.frame, typst::layout::Point::zero(), &mut end);
        let (x, top, bottom) = end.unwrap_or_default();
        let end = TextEnd {
            x: x * scale,
            top: top * scale,
            bottom: bottom * scale,
        };
        if self.ends.len() >= SIZES_KEPT {
            self.ends.clear();
        }
        self.ends.insert(id, end);
        Some((self.render(markup, scale)?, end))
    }

    /// Where in the piece typeset of `markup` a cursor at byte `at` of
    /// `markup` stands: how far across, and the top and the bottom of
    /// what it stands by, in pixels. `None` if nothing of the piece shows
    /// that place or what is before it.
    pub fn caret_in_place(&mut self, markup: &str, scale: f64, at: usize) -> Option<TextEnd> {
        let key = (Self::id(markup, scale), at);
        if let Some(known) = self.carets.get(&key) {
            return *known;
        }
        let caret = self.find_caret(markup, scale, at);
        if self.carets.len() >= SIZES_KEPT {
            self.carets.clear();
        }
        self.carets.insert(key, caret);
        caret
    }

    fn find_caret(&mut self, markup: &str, scale: f64, at: usize) -> Option<TextEnd> {
        self.project.set_main_text(markup);
        let world = self.project.snapshot();
        let document = zetwerk_typst::compile(&world).document?;
        let map = PageMap::build(&world, &document.pages().first()?.frame);
        let file = world.main();
        let caret = map.caret(file, at).or_else(|| map.caret_before(file, at))?;
        let x = if caret.after {
            caret.cell.max.x
        } else {
            caret.cell.min.x
        };
        Some(TextEnd {
            x: x.to_pt() * scale,
            top: caret.cell.min.y.to_pt() * scale,
            bottom: caret.cell.max.y.to_pt() * scale,
        })
    }

    /// Whether `markup` can be typeset at all.
    pub fn typesets(&mut self, markup: &str) -> bool {
        self.project.set_main_text(markup);
        zetwerk_typst::compile(&self.project.snapshot())
            .document
            .is_some()
    }

    /// The bytes of `markup` that keep it from being typeset because
    /// they are names of nothing: a word of an equation that is not, or
    /// not yet, the name of a symbol.
    pub fn unknown_in(&mut self, markup: &str) -> Vec<Range<usize>> {
        self.project.set_main_text(markup);
        let world = self.project.snapshot();
        let compiled = zetwerk_typst::compile(&world);
        compiled
            .errors
            .iter()
            .filter(|error| error.message.starts_with("unknown variable"))
            .filter_map(|error| world.range(error.span))
            .collect()
    }

    fn rasterize(&mut self, markup: &str, scale: f64) -> Option<Pixmap> {
        self.project.set_main_text(markup);
        let document = zetwerk_typst::compile(&self.project.snapshot()).document;
        zetwerk_typst::forget_unused(UNUSED_ROUNDS);
        Some(zetwerk_render::render_page(
            document?.pages().first()?,
            scale,
        ))
    }

    /// The name of the image of a row of the source pane.
    #[must_use]
    pub fn row_id(row: &RowText, scale: f64) -> TextureId {
        TextureId(hash128(&("row", &row.text, row.dim, scale.to_bits())))
    }

    /// Images of rows of the source pane, one for each of `rows`, typeset
    /// together: a screenful of rows costs one compile, not one each.
    ///
    /// The images are not kept here. The rows are many, and the compositor
    /// keeps them for as long as they are drawn; only their sizes are
    /// remembered.
    pub fn render_rows(&mut self, rows: &[RowText], scale: f64) -> Vec<Image> {
        if rows.is_empty() {
            return Vec::new();
        }
        let mut markup = row_preamble(false);
        for (index, row) in rows.iter().enumerate() {
            if index > 0 {
                markup.push_str("#pagebreak()\n");
            }
            let color = if row.dim { DIM } else { FOREGROUND };
            let _ = writeln!(
                markup,
                "#text(fill: rgb(\"{color}\"), {})",
                string(&row.text)
            );
        }
        self.project.set_main_text(&markup);
        let document = zetwerk_typst::compile(&self.project.snapshot()).document;
        zetwerk_typst::forget_unused(UNUSED_ROUNDS);
        let Some(document) = document else {
            return Vec::new();
        };
        if self.sizes.len() >= SIZES_KEPT {
            self.sizes.clear();
        }
        rows.iter()
            .zip(document.pages())
            .map(|(row, page)| {
                let id = Self::row_id(row, scale);
                let pixmap = Arc::new(zetwerk_render::render_page(page, scale));
                self.sizes.insert(id, (pixmap.width(), pixmap.height()));
                Image { id, pixmap }
            })
            .collect()
    }

    /// The size in pixels of an image made by [`Chrome::render_rows`], if
    /// it is still known.
    #[must_use]
    pub fn size_of(&self, id: TextureId) -> Option<(u32, u32)> {
        self.sizes.get(&id).copied()
    }

    /// The width in pixels of one cell of the source pane: the advance of
    /// its monospaced font, measured by setting a row of characters.
    pub fn cell_width(&mut self, scale: f64) -> f64 {
        /// How many characters are set to measure one.
        const SAMPLE: usize = 64;
        if let Some((for_scale, width)) = self.cell
            && for_scale == scale.to_bits()
        {
            return width;
        }
        let markup = format!("{}#{}", row_preamble(false), string(&"M".repeat(SAMPLE)));
        let width = self.rasterize(&markup, scale).map_or(
            // Should the sample not render, a usual width for such a font.
            SOURCE_SIZE * 0.6 * scale,
            |pixmap| f64::from(pixmap.width()) / SAMPLE as f64,
        );
        self.cell = Some((scale.to_bits(), width));
        width
    }

    /// Typesets `markup` and rasterizes its first page at `scale` pixels per
    /// point. `None` if the markup does not compile, which for the fixed
    /// templates in this module would be a bug.
    pub fn render(&mut self, markup: &str, scale: f64) -> Option<Image> {
        self.clock += 1;
        let id = Self::id(markup, scale);
        if let Some((used, pixmap)) = self.cache.get_mut(&id) {
            *used = self.clock;
            return Some(Image {
                id,
                pixmap: Arc::clone(pixmap),
            });
        }
        let pixmap = Arc::new(self.rasterize(markup, scale)?);
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

/// Finds the text item of `frame` that comes last, and notes where it
/// ends and from where to where its line reaches down, in points.
fn last_text(
    frame: &typst::layout::Frame,
    origin: typst::layout::Point,
    end: &mut Option<(f64, f64, f64)>,
) {
    for (position, item) in frame.items() {
        let at = origin + *position;
        match item {
            typst::layout::FrameItem::Group(group) => last_text(&group.frame, at, end),
            typst::layout::FrameItem::Text(text) => {
                let metrics = text.font.metrics();
                let width: f64 = text
                    .glyphs
                    .iter()
                    .map(|glyph| glyph.x_advance.at(text.size).to_pt())
                    .sum();
                *end = Some((
                    at.x.to_pt() + width,
                    (at.y - metrics.ascender.at(text.size)).to_pt(),
                    (at.y - metrics.descender.at(text.size)).to_pt(),
                ));
            }
            _ => {}
        }
    }
}

/// How a piece set in place is to look: like the text around it.
#[derive(Debug, Clone, PartialEq)]
pub struct InPlace<'a> {
    /// The font of the text around it; `None` to leave it to Typst, as
    /// for an equation.
    pub family: Option<&'a str>,
    /// Its size in points, its weight (100 to 900), whether it is italic.
    pub size: f64,
    pub weight: u16,
    pub italic: bool,
    /// The room there is for it, in points, before it has to be broken
    /// into lines.
    pub width: f64,
}

/// Markup that sets `source` (Typst markup) by itself the way the text
/// around it is set, on a slip of paper as high as its lines; or, with
/// `literal`, sets the characters of `source` as they are.
#[must_use]
pub fn in_place_markup(source: &str, style: &InPlace, literal: bool) -> String {
    let font = style
        .family
        .map_or_else(String::new, |family| format!("font: {}, ", string(family)));
    let slant = if style.italic { "italic" } else { "normal" };
    let body = if literal {
        format!("#{}", string(source))
    } else {
        source.to_owned()
    };
    // As wide as its text, but no wider than the room there is: then it
    // is broken into lines.
    format!(
        "#set page(width: auto, height: auto, margin: 0pt, fill: white)\n\
         #set text({font}size: {size}pt, weight: {weight}, style: \"{slant}\", \
         top-edge: \"ascender\", bottom-edge: \"descender\")\n\
         #set par(justify: false)\n\
         #context {{\n  let piece = [{body}]\n  \
         if measure(piece).width > {width}pt {{ block(width: {width}pt, piece) }} else {{ piece }}\n}}",
        size = style.size,
        weight = style.weight,
        width = style.width.max(20.0),
    )
}

/// Markup for the number beside a line of a page that says how far the
/// line is from the cursor: faint, small, on nothing.
#[must_use]
pub fn margin_number_markup(number: usize, size: f64) -> String {
    format!(
        "#set page(width: auto, height: auto, margin: 0pt, fill: none)\n\
         #set text(font: \"{FONT}\", size: {size}pt, fill: rgb(\"{DIM}\").transparentize(25%))\n\
         {number}"
    )
}

/// Where in markup made by [`in_place_markup`] the piece itself begins.
#[must_use]
pub fn in_place_body(markup: &str) -> Option<usize> {
    markup.find(PIECE).map(|at| at + PIECE.len())
}

/// What the piece of [`in_place_markup`] follows.
const PIECE: &str = "let piece = [";

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

/// The text of one row of the source pane. Blanks at either end may get
/// lost in its image; the caller places the image where the first other
/// character goes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RowText {
    pub text: String,
    /// Whether it is set in the dimmer colour, as line numbers are.
    pub dim: bool,
}

/// What the markup for rows of the source pane begins with: a page as wide
/// as its text and one row high, on nothing.
fn row_preamble(dim: bool) -> String {
    let color = if dim { DIM } else { FOREGROUND };
    format!(
        r#"#set page(width: auto, height: {ROW_HEIGHT}pt, margin: 0pt, fill: none)
#set text(font: "{FONT}", size: {SOURCE_SIZE}pt, fill: rgb("{color}"), top-edge: "ascender", bottom-edge: "descender", ligatures: false, kerning: false, hyphenate: false)
#set align(horizon)
"#
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

/// Markup for the panel of a proposed change: a `title`, a `hint` at what
/// can be done with it, and the lines to go and the lines to come.
#[must_use]
pub fn proposal_markup(title: &str, hint: &str, old: &[String], new: &[String]) -> String {
    let mut lines = String::new();
    for (sign, colour, texts) in [("- ", ALERT, old), ("+ ", ADDED, new)] {
        for text in texts {
            let _ = writeln!(
                lines,
                "  text(fill: rgb(\"{colour}\"), {}),",
                string(&format!("{sign}{text}"))
            );
        }
    }
    format!(
        r#"#set page(width: auto, height: auto, margin: 8pt, fill: rgb("{BACKGROUND}"))
#set text(font: "{FONT}", size: 9pt, fill: rgb("{FOREGROUND}"), ligatures: false, hyphenate: false)
#stack(
  spacing: 6pt,
  text(fill: rgb("{ACCENT}"), {title}) + h(24pt) + text(fill: rgb("{DIM}"), {hint}),
{lines})"#,
        title = string(title),
        hint = string(hint),
    )
}

/// Markup for a list to choose from: `rows` of a name and a word about
/// it, the `chosen` one set off; over it a `title`, under it a line of
/// `detail` about the chosen one, where there are any.
#[must_use]
pub fn list_markup(
    title: Option<&str>,
    rows: &[(String, String)],
    chosen: usize,
    detail: Option<&str>,
) -> String {
    let mut lines = String::new();
    if let Some(title) = title {
        let _ = writeln!(lines, "  text(fill: rgb(\"{ACCENT}\"), {}),", string(title));
    }
    for (index, (name, about)) in rows.iter().enumerate() {
        let (name, about) = (string(name), string(about));
        let about = format!("h(16pt) + text(fill: rgb(\"{DIM}\"), {about})");
        if index == chosen {
            let _ = writeln!(
                lines,
                "  block(fill: rgb(\"{CHOSEN}\"), outset: (x: 4pt, y: 2.5pt), radius: 2pt, {name} + {about}),"
            );
        } else {
            let _ = writeln!(lines, "  {name} + {about},");
        }
    }
    if let Some(detail) = detail {
        let _ = writeln!(lines, "  text(fill: rgb(\"{DIM}\"), {}),", string(detail));
    }
    format!(
        r#"#set page(width: auto, height: auto, margin: 8pt, fill: rgb("{BACKGROUND}"))
#set text(font: "{FONT}", size: 9pt, fill: rgb("{FOREGROUND}"), ligatures: false, hyphenate: false)
#stack(
  spacing: 6pt,
{lines})"#
    )
}

/// Markup that sets `equation` (Typst source, dollar signs and all) by
/// itself on a slip of paper: what it will look like on the page.
#[must_use]
pub fn equation_markup(equation: &str) -> String {
    format!(
        "#set page(width: auto, height: auto, margin: 7pt, fill: white)\n\
         #set text(size: 13pt)\n{equation}"
    )
}

/// Markup for a few lines of text in a panel.
#[must_use]
pub fn text_markup(lines: &[String]) -> String {
    let mut stacked = String::new();
    for line in lines {
        // An empty line still takes its room.
        let _ = writeln!(stacked, "  {} + h(0pt),", string(line));
    }
    format!(
        r#"#set page(width: auto, height: auto, margin: 8pt, fill: rgb("{BACKGROUND}"))
#set text(font: "{FONT}", size: 9pt, fill: rgb("{FOREGROUND}"), ligatures: false, hyphenate: false)
#stack(
  spacing: 6pt,
{stacked})"#
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
