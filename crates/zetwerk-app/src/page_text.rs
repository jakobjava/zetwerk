//! Pages put into a document whole: a page of a PDF, an image as a page
//! of its own, a blank page. How they are written, how they are read
//! back from the text, and where in the text they can stand.
//!
//! Each is one line of ordinary Typst that anybody (or an agent editing
//! the source) could have written, and that the `typst` program
//! typesets the same:
//!
//! ```typst
//! #page(width: auto, height: auto, margin: 0pt, header: none, footer: none)[#image("cover.pdf", page: 1)]
//! #page(columns: 1, margin: 0pt, header: none, footer: none)[#image("scan.png", width: 100%, height: 100%, fit: "contain")]
//! #page(header: none, footer: none)[]
//! ```
//!
//! The first keeps the page as large as it is in its file (a landscape
//! slide stays landscape): a page as wide and as high as `auto` grows to
//! what it holds, and an image is as large as its file says. The second
//! puts it on the document's paper, scaled to fit, in the middle. Such a
//! page is a page of the document: it counts in the page numbers but
//! shows none (no header, no footer). One that is not to count has
//! `#counter(page).update(n => n - 1)` before its image. `#page(…)[…]`
//! is scoped: the `#set page` rules in force before it hold again after
//! it, columns, margins and numbering too.
//!
//! Several pages of one PDF are written one line each, not as a loop:
//! each page is then a thing of its own in the text, which `x` on the
//! page takes out by itself and which moves by itself, and an agent
//! reads which page of which file it is without working out a loop.
//!
//! What is read back: such lines, standing alone on their lines at the
//! top level of the text, and those written by hand like them (another
//! order of the arguments, the page's content in parentheses, an image
//! in an `align` or a `box`). A `#page` with anything else in it is "a
//! page", and is left alone. All here is about the text: where such a
//! page is on the pages is the viewer's business (see `viewer/pages`).

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use typst::syntax::ast;
use typst::syntax::{LinkedNode, Source, SyntaxKind};

use crate::figure_text::quoted;

/// What makes a page put in not count in the page numbers.
const UNCOUNTED: &str = "#counter(page).update(n => n - 1)";

/// The files a page can be made of, by the ends of their names: a PDF,
/// and the images Typst reads.
pub const PDF_ENDINGS: &[&str] = &["pdf"];
pub const IMAGE_ENDINGS: &[&str] = &["png", "jpg", "jpeg", "gif", "svg", "webp"];

/// How many pages are put in without asking first.
pub const MANY_PAGES: usize = 50;

/// What may stand in a page put in besides its image: what places it.
const AROUND_IMAGE: &[&str] = &["image", "align", "box", "block", "place", "counter"];

/// What a page put in shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shows {
    /// Page `page` (from 1) of the PDF `file`, as the text names it.
    Pdf { file: String, page: usize },
    /// The image `file`.
    Image { file: String },
    /// Nothing.
    Blank,
    /// Something else: a page written by hand.
    Other,
}

/// A page to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub shows: Shows,
    /// Whether it is put on the document's paper, scaled to fit, and not
    /// left as large as it is.
    pub fit: bool,
    /// Whether it counts in the page numbers.
    pub counted: bool,
}

/// A page put in, as the text has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inserted {
    /// Its bytes: from the `#` to the end of the call.
    pub range: Range<usize>,
    /// The line it begins on, counting from 1.
    pub line: usize,
    pub shows: Shows,
    /// `Some(true)` where it is written to fit the paper, `Some(false)`
    /// where it is as large as it is, `None` where it is written in
    /// another way.
    pub fit: Option<bool>,
    pub counted: bool,
}

impl Inserted {
    /// The page as it would be written anew: as large as it is unless it
    /// is written to fit.
    #[must_use]
    pub fn page(&self) -> Page {
        Page {
            shows: self.shows.clone(),
            fit: self.fit.unwrap_or(false),
            counted: self.counted,
        }
    }

    /// The file it shows, if it shows one.
    #[must_use]
    pub fn file(&self) -> Option<&str> {
        match &self.shows {
            Shows::Pdf { file, .. } | Shows::Image { file } => Some(file),
            Shows::Blank | Shows::Other => None,
        }
    }

    /// What it is, as the user is told: "page 2 of cover.pdf",
    /// "scan.png", "a blank page", "a page".
    #[must_use]
    pub fn called(&self) -> String {
        match &self.shows {
            Shows::Pdf { file, page } => format!("page {page} of {file}"),
            Shows::Image { file } => file.clone(),
            Shows::Blank => "a blank page".to_owned(),
            Shows::Other => "a page".to_owned(),
        }
    }

    /// Whether it is one this module writes and changes: not a page
    /// written by hand with more in it.
    #[must_use]
    pub fn is_known(&self) -> bool {
        self.shows != Shows::Other
    }
}

/// `page` as one line of Typst.
#[must_use]
pub fn written(page: &Page) -> String {
    let counter = if page.counted { "" } else { UNCOUNTED };
    let (file, number) = match &page.shows {
        Shows::Blank | Shows::Other => {
            return format!("#page(header: none, footer: none)[{counter}]");
        }
        Shows::Pdf { file, page } => (file, Some(page)),
        Shows::Image { file } => (file, None),
    };
    let number = number.map_or(String::new(), |page| format!(", page: {page}"));
    let (size, fit) = if page.fit {
        (
            "columns: 1, ",
            ", width: 100%, height: 100%, fit: \"contain\"",
        )
    } else {
        ("width: auto, height: auto, ", "")
    };
    format!(
        "#page({size}margin: 0pt, header: none, footer: none)[{counter}#image({}{number}{fit})]",
        quoted(file)
    )
}

/// `pages` as lines of Typst, one below the other.
#[must_use]
pub fn written_all(pages: &[Page]) -> String {
    let lines: Vec<String> = pages.iter().map(written).collect();
    lines.join("\n")
}

/// The page break, as it is written.
pub const PAGE_BREAK: &str = "#pagebreak()";

/// Whether the name of `file` ends in one of `endings`, in any case.
#[must_use]
pub fn ends_in(file: &str, endings: &[&str]) -> bool {
    let ending = file.rsplit_once('.').map_or("", |(_, ending)| ending);
    endings
        .iter()
        .any(|wanted| wanted.eq_ignore_ascii_case(ending))
}

/// The name a call is made by, where it is a plain name.
fn callee(call: &LinkedNode) -> Option<String> {
    let first = call.children().next()?;
    match first.kind() {
        SyntaxKind::Ident => Some(first.leaf_text().to_string()),
        // `counter(page).update(…)`: the name of what it is called on.
        SyntaxKind::FieldAccess => first
            .children()
            .next()
            .filter(|target| target.kind() == SyntaxKind::FuncCall)
            .and_then(|target| callee(&target)),
        _ => None,
    }
}

/// The arguments of `call`: by name, the value of each named one as
/// written; and the others.
fn arguments<'a>(call: &LinkedNode<'a>) -> (Vec<(String, LinkedNode<'a>)>, Vec<LinkedNode<'a>>) {
    let mut named = Vec::new();
    let mut positional = Vec::new();
    let Some(args) = call
        .children()
        .find(|child| child.kind() == SyntaxKind::Args)
    else {
        return (named, positional);
    };
    for child in args.children() {
        match child.kind() {
            SyntaxKind::Named => {
                let mut parts = child.children();
                let name = parts.next().map(|name| name.leaf_text().to_string());
                let value = parts.find(|part| {
                    !matches!(part.kind(), SyntaxKind::Colon) && !part.kind().is_trivia()
                });
                if let (Some(name), Some(value)) = (name, value) {
                    named.push((name, value));
                }
            }
            SyntaxKind::LeftParen | SyntaxKind::RightParen | SyntaxKind::Comma => {}
            kind if kind.is_trivia() => {}
            _ => positional.push(child),
        }
    }
    (named, positional)
}

/// What `node` is written as, without blanks.
fn squeezed(node: &LinkedNode) -> String {
    node.get()
        .full_text()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// The value of the argument `name` among `named`, as written, without
/// blanks.
fn value_of(named: &[(String, LinkedNode)], name: &str) -> Option<String> {
    named
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| squeezed(value))
}

/// What is in a page: the calls of `image` in it, whether something
/// else is in it (text, or a call that is not one that places it), and
/// whether it takes itself out of the page numbers.
#[derive(Default)]
struct Inside<'a> {
    images: Vec<LinkedNode<'a>>,
    more: bool,
    uncounted: bool,
}

fn look_inside<'a>(node: &LinkedNode<'a>, inside: &mut Inside<'a>) {
    match node.kind() {
        SyntaxKind::Text | SyntaxKind::Strong | SyntaxKind::Emph | SyntaxKind::Equation
            if !node.get().full_text().trim().is_empty() =>
        {
            inside.more = true;
        }
        SyntaxKind::FuncCall => {
            let name = callee(node);
            match name.as_deref() {
                Some("image") => {
                    inside.images.push(node.clone());
                    return;
                }
                Some("counter") => {
                    let written = squeezed(node);
                    if written.starts_with("counter(page).update(") {
                        inside.uncounted = true;
                    } else {
                        inside.more = true;
                    }
                    return;
                }
                Some(name) if AROUND_IMAGE.contains(&name) => {}
                _ => {
                    inside.more = true;
                    return;
                }
            }
        }
        _ => {}
    }
    for child in node.children() {
        look_inside(&child, inside);
    }
}

/// What the `#page(…)` call `call` shows, whether it is written to fit
/// the paper, and whether it counts in the page numbers.
fn read_page(call: &LinkedNode) -> (Shows, Option<bool>, bool) {
    let (named, positional) = arguments(call);
    let mut inside = Inside::default();
    for content in &positional {
        look_inside(content, &mut inside);
    }
    let counted = !inside.uncounted;
    match inside.images.as_slice() {
        [] if !inside.more => return (Shows::Blank, None, counted),
        [image] if !inside.more => {
            let (image_named, image_positional) = arguments(image);
            let file = image_positional
                .first()
                .and_then(|file| file.get().cast::<ast::Str>())
                .map(|file| file.get().to_string());
            let Some(file) = file else {
                return (Shows::Other, None, counted);
            };
            let page = value_of(&image_named, "page").and_then(|page| page.parse().ok());
            let shows = if ends_in(&file, PDF_ENDINGS) {
                Shows::Pdf {
                    file,
                    page: page.unwrap_or(1),
                }
            } else {
                Shows::Image { file }
            };
            let page_value = |key: &str| value_of(&named, key);
            let image_value = |key: &str| value_of(&image_named, key);
            let fits = page_value("width").is_none()
                && page_value("height").is_none()
                && image_value("width").as_deref() == Some("100%")
                && image_value("height").as_deref() == Some("100%")
                && image_value("fit").as_deref() == Some("\"contain\"");
            let own = page_value("width").as_deref() == Some("auto")
                && page_value("height").as_deref() == Some("auto")
                && image_value("width").is_none()
                && image_value("height").is_none();
            let fit = if fits {
                Some(true)
            } else if own {
                Some(false)
            } else {
                None
            };
            return (shows, fit, counted);
        }
        _ => {}
    }
    (Shows::Other, None, counted)
}

/// Whether bytes `range` of `text` stand alone on their lines: nothing
/// but blanks before them on the first, nor behind them on the last.
fn alone(text: &str, range: &Range<usize>) -> bool {
    let line_start = text[..range.start].rfind('\n').map_or(0, |at| at + 1);
    let line_end = text[range.end..]
        .find('\n')
        .map_or(text.len(), |at| range.end + at);
    text[line_start..range.start].trim().is_empty() && text[range.end..line_end].trim().is_empty()
}

/// The line byte `at` of `text` is on, counting from 1.
fn line_of(text: &str, at: usize) -> usize {
    text[..at].matches('\n').count() + 1
}

/// The calls of the top level of `source` made by the name `name` and
/// standing alone on their lines: each with its bytes, from the `#`.
fn calls_of<'a>(source: &'a Source, name: &str) -> Vec<(Range<usize>, LinkedNode<'a>)> {
    let root = LinkedNode::new(source.root());
    let children: Vec<LinkedNode> = root.children().collect();
    let mut found = Vec::new();
    for pair in children.windows(2) {
        let (hash, call) = (&pair[0], &pair[1]);
        if hash.kind() != SyntaxKind::Hash || call.kind() != SyntaxKind::FuncCall {
            continue;
        }
        if callee(call).as_deref() != Some(name) {
            continue;
        }
        let range = hash.range().start..call.range().end;
        if alone(source.text(), &range) {
            found.push((range, call.clone()));
        }
    }
    found
}

/// The pages put into `source`, in the order of the text.
#[must_use]
pub fn inserted(source: &Source) -> Vec<Inserted> {
    calls_of(source, "page")
        .into_iter()
        .map(|(range, call)| {
            let (shows, fit, counted) = read_page(&call);
            Inserted {
                line: line_of(source.text(), range.start),
                range,
                shows,
                fit,
                counted,
            }
        })
        .collect()
}

/// The page breaks of the top level of `source`, each alone on its
/// line.
#[must_use]
pub fn page_breaks(source: &Source) -> Vec<Range<usize>> {
    calls_of(source, "pagebreak")
        .into_iter()
        .map(|(range, _)| range)
        .collect()
}

/// Where what the document shows begins: behind the rules, the names
/// defined and the comments at its top.
#[must_use]
pub fn content_start(source: &Source) -> usize {
    let root = LinkedNode::new(source.root());
    let children: Vec<LinkedNode> = root.children().collect();
    let mut index = 0;
    while let Some(child) = children.get(index) {
        let rule = |node: Option<&LinkedNode>| {
            node.is_some_and(|node| {
                matches!(
                    node.kind(),
                    SyntaxKind::SetRule
                        | SyntaxKind::ShowRule
                        | SyntaxKind::LetBinding
                        | SyntaxKind::ModuleImport
                )
            })
        };
        match child.kind() {
            SyntaxKind::Space
            | SyntaxKind::Parbreak
            | SyntaxKind::LineComment
            | SyntaxKind::BlockComment => index += 1,
            SyntaxKind::Hash if rule(children.get(index + 1)) => index += 2,
            _ => return child.range().start,
        }
    }
    source.text().trim_end().len()
}

/// The end of what the document shows: before the blanks at its end.
#[must_use]
pub fn content_end(text: &str) -> usize {
    text.trim_end().len()
}

/// Where the top-level paragraph (or heading, list, figure) that byte
/// `at` of `source` is in ends, for what is to go behind it: where the
/// paragraph break behind it begins, or the page put in or the page
/// break behind it, or the end of the text.
#[must_use]
pub fn after_paragraph(source: &Source, at: usize) -> usize {
    let text = source.text();
    let blocks: Vec<usize> = inserted(source)
        .iter()
        .map(|page| page.range.start)
        .chain(page_breaks(source).iter().map(|range| range.start))
        .collect();
    let root = LinkedNode::new(source.root());
    let mut found = false;
    for child in root.children() {
        let range = child.range();
        if !found {
            if range.end <= at {
                continue;
            }
            found = true;
            // What the cursor is on is a page put in, or a break.
            if let Some(block) = blocks.iter().find(|start| **start == range.start) {
                let call = calls_end(source, *block);
                return call.unwrap_or(range.end);
            }
        }
        if child.kind() == SyntaxKind::Parbreak {
            return range.start;
        }
        if blocks.contains(&range.start) && range.start > at {
            return range.start;
        }
    }
    content_end(text)
}

/// Where the top-level paragraph (or heading, list, figure) that byte
/// `at` of `source` is in begins, for what is to go before it: behind
/// the paragraph break before it, or the page put in or the page break
/// before it, or where what the document shows begins.
#[must_use]
pub fn paragraph_start(source: &Source, at: usize) -> usize {
    let blocks: Vec<Range<usize>> = inserted(source)
        .iter()
        .map(|page| page.range.clone())
        .chain(page_breaks(source))
        .collect();
    let mut begin = content_start(source);
    let root = LinkedNode::new(source.root());
    for child in root.children() {
        let range = child.range();
        if range.start >= at {
            break;
        }
        if child.kind() == SyntaxKind::Parbreak && range.end <= at {
            begin = begin.max(range.end);
        }
        if let Some(block) = blocks.iter().find(|block| block.start == range.start)
            && block.end <= at
        {
            begin = begin.max(block.end);
        }
    }
    begin
}

/// What an agent asks to have put in (see `insert_pages`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Wanted {
    /// The file, as a path from the folder of the file edited.
    pub file: String,
    /// Which of its pages: `2`, `1-3`, `1,4-6`; all where none are named.
    pub pages: Option<String>,
    /// Where: `cursor` (after the paragraph the user's cursor is in, the
    /// default), `start`, `end`, `after:<line>`, `before:<line>`.
    pub place: Option<String>,
    /// Whether it is fit to the document's paper (default: a PDF's page
    /// is as large as it is, an image is fit).
    pub fit: Option<bool>,
    /// Whether it counts in the page numbers (default: it does).
    pub counted: Option<bool>,
}

/// Where the page put in or the page break that begins at `start` ends.
fn calls_end(source: &Source, start: usize) -> Option<usize> {
    inserted(source)
        .iter()
        .map(|page| page.range.clone())
        .chain(page_breaks(source))
        .find(|range| range.start == start)
        .map(|range| range.end)
}

/// Whether bytes `range` of `text` hold nothing but blanks.
fn blank(text: &str, range: Range<usize>) -> bool {
    text.get(range).is_some_and(|part| part.trim().is_empty())
}

/// How far from byte `at` of `text` the blanks before it begin, and
/// where those behind it end.
fn blanks_around(text: &str, at: usize) -> (usize, usize) {
    let before = text[..at].trim_end().len();
    let after = at + (text[at..].len() - text[at..].trim_start().len());
    (before, after)
}

/// The text `text` with `block` (lines of pages) put in at byte `at`, a
/// place between two things of the top level: apart from what is
/// around it by a blank line, from other pages put in by a line break.
/// Also where the block begins in the new text.
#[must_use]
pub fn with_put(text: &str, at: usize, block: &str, pages: &[Inserted]) -> (String, usize) {
    let (before, after) = blanks_around(text, at);
    let page_ends = |at: usize| pages.iter().any(|page| page.range.end == at);
    let page_starts = |at: usize| pages.iter().any(|page| page.range.start == at);
    let lead = if before == 0 {
        ""
    } else if page_ends(before) {
        "\n"
    } else {
        "\n\n"
    };
    let trail = if after == text.len() || page_starts(after) {
        "\n"
    } else {
        "\n\n"
    };
    let new = format!("{}{lead}{block}{trail}{}", &text[..before], &text[after..]);
    (new, before + lead.len())
}

/// The text `text` without bytes `range` (pages put in): what was
/// around them is apart as before, by a blank line, or by a line break
/// where pages put in stand on both sides.
#[must_use]
pub fn without(text: &str, range: Range<usize>, pages: &[Inserted]) -> String {
    let (before, _) = blanks_around(text, range.start);
    let (_, after) = blanks_around(text, range.end);
    let page_ends = |at: usize| pages.iter().any(|page| page.range.end == at);
    let page_starts = |at: usize| pages.iter().any(|page| page.range.start == at);
    let between = if before == 0 {
        ""
    } else if after == text.len() || (page_ends(before) && page_starts(after)) {
        "\n"
    } else {
        "\n\n"
    };
    format!("{}{between}{}", &text[..before], &text[after..])
}

/// The places pages put in can be moved to: the start and the end of
/// what is shown, behind each page break, and before and behind each
/// page put in.
fn stations(source: &Source, pages: &[Inserted]) -> Vec<usize> {
    let mut stations = vec![content_start(source), content_end(source.text())];
    stations.extend(page_breaks(source).iter().map(|range| range.end));
    for page in pages {
        stations.extend([page.range.start, page.range.end]);
    }
    stations.sort_unstable();
    stations.dedup();
    stations
}

/// The text with the pages put in `first..=last` of `pages` (those of
/// `source`, standing one behind the other) moved up (or down) past
/// what is next to them: the page put in next to them, or the text up
/// to the nearest page break, or the start or the end. Also where they
/// begin in the new text, and whether they went past text (and not
/// only past other pages put in).
///
/// # Errors
///
/// Where there is nothing that way to go past.
pub fn moved(
    source: &Source,
    pages: &[Inserted],
    first: usize,
    last: usize,
    up: bool,
) -> Result<(String, usize, bool), String> {
    let text = source.text();
    let (Some(from), Some(to)) = (pages.get(first), pages.get(last)) else {
        return Err("there is no such page".to_owned());
    };
    let block = from.range.start..to.range.end;
    let stations = stations(source, pages);
    let target = if up {
        stations
            .iter()
            .rev()
            .find(|at| **at < block.start && !blank(text, **at..block.start))
    } else {
        stations
            .iter()
            .find(|at| **at > block.end && !blank(text, block.end..**at))
    };
    let Some(&target) = target else {
        return Err(if up {
            "it is at the start already: nothing goes before it".to_owned()
        } else {
            "it is at the end already: nothing comes behind it".to_owned()
        });
    };
    let past_text = !pages
        .iter()
        .any(|page| (up && page.range.start == target) || (!up && page.range.end == target));
    let moving = &text[block.clone()];
    // The later place first, so that the earlier one stays where it is.
    if up {
        let removed = without(text, block, pages);
        let (new, start) = with_put(&removed, target, moving, pages);
        Ok((new, start, past_text))
    } else {
        let (put, start) = with_put(text, target, moving, pages);
        let removed = without(&put, block, pages);
        // Where it begins now: where it was put, less what went before.
        let start = start - (put.len() - removed.len());
        Ok((removed, start, past_text))
    }
}

/// The one replacement that makes `old` into `new`: the bytes of `old`
/// to replace, and what with. Nothing to replace where they are the
/// same.
#[must_use]
pub fn replacement(old: &str, new: &str) -> (Range<usize>, String) {
    let mut prefix = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !(old.is_char_boundary(prefix) && new.is_char_boundary(prefix)) {
        prefix -= 1;
    }
    let room = old.len().min(new.len()) - prefix;
    let mut suffix = old
        .bytes()
        .rev()
        .zip(new.bytes().rev())
        .take(room)
        .take_while(|(a, b)| a == b)
        .count();
    while !(old.is_char_boundary(old.len() - suffix) && new.is_char_boundary(new.len() - suffix)) {
        suffix -= 1;
    }
    (
        prefix..old.len() - suffix,
        new[prefix..new.len() - suffix].to_owned(),
    )
}

/// The pages of the pages put in `pages` standing one behind the other
/// with page `index`, the same PDF file shown by all: their indices.
#[must_use]
pub fn run_of(text: &str, pages: &[Inserted], index: usize) -> Range<usize> {
    let Some(page) = pages.get(index) else {
        return index..index;
    };
    let file = match &page.shows {
        Shows::Pdf { file, .. } => file.clone(),
        _ => return index..index + 1,
    };
    let same = |page: &Inserted| matches!(&page.shows, Shows::Pdf { file: of, .. } if *of == file);
    let mut first = index;
    while first > 0
        && same(&pages[first - 1])
        && blank(text, pages[first - 1].range.end..pages[first].range.start)
    {
        first -= 1;
    }
    let mut last = index;
    while last + 1 < pages.len()
        && same(&pages[last + 1])
        && blank(text, pages[last].range.end..pages[last + 1].range.start)
    {
        last += 1;
    }
    first..last + 1
}

/// The pages `typed` names of a file of `count` pages: `all` (or
/// nothing), numbers and stretches of them, `2`, `1-3`, `1,4-6`, `3-`,
/// in the order they are named.
///
/// # Errors
///
/// Where `typed` names no pages, or names one the file does not have.
pub fn page_numbers(typed: &str, count: usize) -> Result<Vec<usize>, String> {
    let typed = typed.trim();
    if typed.is_empty() || typed.eq_ignore_ascii_case("all") {
        return Ok((1..=count).collect());
    }
    let wrong = || format!("{typed} names no pages: write them as 2, 1-3 or 1,4-6 (or all)");
    let number = |part: &str, missing: usize| -> Result<usize, String> {
        match part.trim() {
            "" => Ok(missing),
            digits => digits
                .parse::<usize>()
                .ok()
                .filter(|page| *page > 0)
                .ok_or_else(wrong),
        }
    };
    let mut pages = Vec::new();
    for part in typed.split(',') {
        let (from, to) = match part.split_once('-') {
            Some((from, to)) if !(from.trim().is_empty() && to.trim().is_empty()) => {
                (number(from, 1)?, number(to, count)?)
            }
            Some(_) => return Err(wrong()),
            None => {
                let page = number(part, 0)?;
                if page == 0 {
                    return Err(wrong());
                }
                (page, page)
            }
        };
        if from > to {
            return Err(wrong());
        }
        if let Some(beyond) = [from, to].into_iter().find(|page| *page > count) {
            let s = if count == 1 { "" } else { "s" };
            return Err(format!(
                "there is no page {beyond}: the file has {count} page{s}"
            ));
        }
        pages.extend(from..=to);
    }
    Ok(pages)
}

/// What a file that is to become pages is.
#[derive(Debug, Clone, PartialEq)]
pub enum Made {
    /// A PDF: the width and the height of each of its pages in points,
    /// and whether it is newer than the PDF files Typst writes take.
    Pdf { sizes: Vec<(f64, f64)>, newer: bool },
    /// An image Typst reads.
    Image,
}

/// What the file `bytes` (named `name`) is, as far as pages can be made
/// of it.
///
/// # Errors
///
/// Where it is neither a PDF nor an image, or a PDF that is locked or
/// cannot be read.
pub fn made_of(name: &str, bytes: &[u8]) -> Result<Made, String> {
    let head = &bytes[..bytes.len().min(1024)];
    let is_pdf = head.windows(5).any(|window| window == b"%PDF-");
    if is_pdf {
        let pdf = hayro_syntax::Pdf::new(Arc::new(bytes.to_vec())).map_err(|why| match why {
            hayro_syntax::LoadPdfError::Decryption(_) => {
                format!("{name} is locked by a password: Typst cannot read it; unlock it first")
            }
            hayro_syntax::LoadPdfError::Invalid => {
                format!("{name} is a broken PDF: it cannot be read")
            }
        })?;
        let sizes: Vec<(f64, f64)> = pdf
            .pages()
            .iter()
            .map(|page| {
                let (width, height) = page.render_dimensions();
                (f64::from(width), f64::from(height))
            })
            .collect();
        if sizes.is_empty() {
            return Err(format!("{name} is a PDF without pages"));
        }
        let newer = matches!(pdf.version(), hayro_syntax::PdfVersion::Pdf20);
        return Ok(Made::Pdf { sizes, newer });
    }
    let image = head.starts_with(b"\x89PNG")
        || head.starts_with(b"\xff\xd8\xff")
        || head.starts_with(b"GIF8")
        || (head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP"))
        || (ends_in(name, &["svg"]) && String::from_utf8_lossy(head).contains("<svg"));
    if image {
        return Ok(Made::Image);
    }
    Err(format!(
        "{name} is neither a PDF nor an image (png, jpg, gif, svg, webp): no page can be made of it"
    ))
}

/// Where the file at `from` is, for a document in `folder` to name it:
/// a path from `folder` where it is in or below it. Else it is copied
/// into `into` (in or below `folder`): under its own name, or, where a
/// file of that name and other bytes is there, under another; one with
/// the same bytes is taken as it is. Nothing is ever written over, and
/// `from` is left as it is. Returns the path as the document names it,
/// and where it was copied to, if it was.
///
/// # Errors
///
/// Where `from` cannot be read, or `into` cannot be written.
pub fn brought_in(
    from: &Path,
    folder: &Path,
    into: &Path,
) -> Result<(String, Option<PathBuf>), String> {
    let shown = from.display();
    let absolute = std::path::absolute(from).map_err(|why| format!("{shown}: {why}"))?;
    let folder = std::path::absolute(folder).map_err(|why| format!("{why}"))?;
    if !absolute.is_file() {
        return Err(format!("there is no file {shown}"));
    }
    let named = |path: &Path, base: &Path| -> Option<String> {
        let within = path.strip_prefix(base).ok()?;
        let plain = within
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)));
        plain.then(|| within.to_string_lossy().replace('\\', "/"))
    };
    if let Some(name) = named(&absolute, &folder) {
        return Ok((name, None));
    }
    let bytes = std::fs::read(&absolute).map_err(|why| format!("{shown} cannot be read: {why}"))?;
    let into = std::path::absolute(into).map_err(|why| format!("{why}"))?;
    let stem = absolute.file_stem().map_or_else(
        || "page".to_owned(),
        |stem| stem.to_string_lossy().into_owned(),
    );
    let ending = absolute
        .extension()
        .map(|ending| format!(".{}", ending.to_string_lossy()))
        .unwrap_or_default();
    for number in 1..1000 {
        let name = if number == 1 {
            format!("{stem}{ending}")
        } else {
            format!("{stem}-{number}{ending}")
        };
        let path = into.join(&name);
        match std::fs::read(&path) {
            Ok(there) if there == bytes => {
                let name = named(&path, &folder).unwrap_or(name);
                return Ok((name, None));
            }
            Err(why) if why.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) | Err(_) => continue,
        }
        let written = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .and_then(|mut file| std::io::Write::write_all(&mut file, &bytes));
        return match written {
            Ok(()) => Ok((named(&path, &folder).unwrap_or(name), Some(path))),
            Err(why) if why.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(why) => Err(format!(
                "the document's folder {} cannot be written: {why}",
                into.display()
            )),
        };
    }
    Err(format!(
        "no free name for {stem}{ending} in {}",
        into.display()
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
pub(crate) mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use typst::layout::{Frame, FrameItem};
    use typst_layout::PagedDocument;
    use zetwerk_typst::{Environment, FontSet, Project};

    use super::*;

    /// A fresh directory for one test.
    fn directory(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("zetwerk-page-text-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    /// The pages `text` is typeset to, as `main.typ` in `directory`.
    fn typeset(directory: &Path, text: &str) -> Result<PagedDocument, String> {
        let path = directory.join("main.typ");
        std::fs::write(&path, text).unwrap();
        let env = Environment::new(FontSet::Embedded);
        let project = Project::open(Arc::clone(&env), &path).unwrap();
        let world = project.snapshot();
        let compiled = zetwerk_typst::compile(&world);
        compiled.document.ok_or_else(|| {
            let errors: Vec<String> = compiled
                .errors
                .iter()
                .map(|error| error.message.to_string())
                .collect();
            errors.join("; ")
        })
    }

    /// The bytes of a PDF of three landscape pages, 300 by 200 points,
    /// that say "One", "Two", "Three": made by Typst itself.
    pub(crate) fn slides_pdf() -> Vec<u8> {
        let env = Environment::new(FontSet::Embedded);
        let text =
            "#set page(width: 300pt, height: 200pt)\nOne\n#pagebreak()\nTwo\n#pagebreak()\nThree\n";
        let project =
            Project::in_memory(env, PathBuf::from("/"), "main.typ", text.to_owned()).unwrap();
        let world = project.snapshot();
        let document = zetwerk_typst::compile(&world).document.unwrap();
        zetwerk_typst::export_pdf(&document, &world).unwrap()
    }

    /// The sizes of the pages of `document`, in whole points.
    fn sizes(document: &PagedDocument) -> Vec<(i64, i64)> {
        document
            .pages()
            .iter()
            .map(|page| {
                let size = page.frame.size();
                (size.x.to_pt().round() as i64, size.y.to_pt().round() as i64)
            })
            .collect()
    }

    /// The text set on `frame`, and where its images are, in whole
    /// points: left, top, width, height.
    fn contents(frame: &Frame, at: (f64, f64), text: &mut String, images: &mut Vec<[i64; 4]>) {
        for (point, item) in frame.items() {
            let here = (at.0 + point.x.to_pt(), at.1 + point.y.to_pt());
            match item {
                FrameItem::Group(group) => contents(&group.frame, here, text, images),
                FrameItem::Text(item) => text.push_str(&item.text),
                FrameItem::Image(_, size, _) => images.push([
                    here.0.round() as i64,
                    here.1.round() as i64,
                    size.x.to_pt().round() as i64,
                    size.y.to_pt().round() as i64,
                ]),
                _ => {}
            }
        }
    }

    fn page_contents(document: &PagedDocument, index: usize) -> (String, Vec<[i64; 4]>) {
        let (mut text, mut images) = (String::new(), Vec::new());
        contents(
            &document.pages()[index].frame,
            (0.0, 0.0),
            &mut text,
            &mut images,
        );
        (text, images)
    }

    fn pdf(page: usize, fit: bool, counted: bool) -> Page {
        Page {
            shows: Shows::Pdf {
                file: "slides.pdf".to_owned(),
                page,
            },
            fit,
            counted,
        }
    }

    #[test]
    fn every_form_written_typesets_to_the_pages_meant() {
        let directory = directory("forms");
        std::fs::write(directory.join("slides.pdf"), slides_pdf()).unwrap();
        // A tall image of 100 by 400 pixels: 75 by 300 points, as Typst
        // takes a pixel of an SVG file.
        let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100\" height=\"400\"/>";
        std::fs::write(directory.join("tall.svg"), svg).unwrap();
        let tall = |fit: bool, counted: bool| Page {
            shows: Shows::Image {
                file: "tall.svg".to_owned(),
            },
            fit,
            counted,
        };
        let blank = |counted: bool| Page {
            shows: Shows::Blank,
            fit: false,
            counted,
        };
        // A document that sets its paper, margins, columns and numbers:
        // what comes after the pages put in has all that again.
        let rules = "#set page(paper: \"a5\", margin: 1cm, columns: 2, numbering: \"1\")\n";
        let block = written_all(&[
            pdf(2, false, true),
            pdf(1, true, true),
            tall(true, true),
            tall(false, false),
            blank(true),
            blank(false),
        ]);
        let text =
            format!("{rules}Before.\n\n{block}\n\nAfter #context counter(page).get().first().\n");
        let document = typeset(&directory, &text).unwrap();
        let a5 = (420, 595);
        assert_eq!(
            sizes(&document),
            [a5, (300, 200), a5, a5, (75, 300), a5, a5, a5]
        );
        // As large as it is: the page is the slide.
        assert_eq!(
            page_contents(&document, 1),
            (String::new(), vec![[0, 0, 300, 200]])
        );
        // Fit to the paper: as wide as the paper, in the middle of it.
        assert_eq!(page_contents(&document, 2).1, vec![[0, 158, 420, 280]]);
        assert_eq!(page_contents(&document, 3).1, vec![[135, 0, 149, 595]]);
        // No number on a page put in, nor on a blank one; the last page
        // is the eighth, less the two that do not count.
        assert_eq!(page_contents(&document, 0).0, "Before.1");
        assert_eq!(page_contents(&document, 4).0, "");
        assert_eq!(page_contents(&document, 5).0, "");
        assert_eq!(page_contents(&document, 7).0, "After 6.6");
        // Each form is read back as written.
        let source = Source::detached(text.as_str());
        let read: Vec<Page> = inserted(&source).iter().map(Inserted::page).collect();
        assert_eq!(
            read,
            [
                pdf(2, false, true),
                pdf(1, true, true),
                tall(true, true),
                tall(false, false),
                blank(true),
                blank(false),
            ]
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_page_put_in_at_the_start_makes_no_empty_page_and_breaks_add_none() {
        let directory = directory("start");
        std::fs::write(directory.join("slides.pdf"), slides_pdf()).unwrap();
        let one = written(&pdf(3, false, true));
        for (text, pages) in [
            (format!("{one}\n\nText.\n"), 2),
            (format!("#set text(size: 12pt)\n{one}\n"), 1),
            (format!("Text.\n\n#pagebreak()\n{one}\n\nMore.\n"), 3),
            (format!("Text.\n\n{one}\n#pagebreak()\nMore.\n"), 3),
            ("Text.\n\n#page[]\n\nMore.\n".to_owned(), 3),
        ] {
            let document = typeset(&directory, &text).unwrap();
            assert_eq!(document.pages().len(), pages, "{text}");
        }
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn pages_written_by_hand_are_read_as_far_as_they_go() {
        let text = "\
#set page(numbering: \"1\")
Words.

#page(margin: 0pt, footer: none, header: none, height: auto, width: auto)[
  #image(\"a.pdf\", page: 4)
]
#page(margin: 1cm, align(center + horizon, image(\"scan.jpg\", width: 80%)))
#page[Some words of its own.]
#page[ ]
Inline #page[] is no page of its own line.
#page(image(\"x\" + \".pdf\"))
";
        let source = Source::detached(text);
        let found = inserted(&source);
        let read: Vec<(usize, Shows, Option<bool>)> = found
            .iter()
            .map(|page| (page.line, page.shows.clone(), page.fit))
            .collect();
        assert_eq!(
            read,
            [
                (
                    4,
                    Shows::Pdf {
                        file: "a.pdf".to_owned(),
                        page: 4
                    },
                    Some(false)
                ),
                (
                    7,
                    Shows::Image {
                        file: "scan.jpg".to_owned()
                    },
                    None
                ),
                (8, Shows::Other, None),
                (9, Shows::Blank, None),
                (11, Shows::Other, None),
            ]
        );
        assert_eq!(found[0].called(), "page 4 of a.pdf");
        assert_eq!(&text[found[3].range.clone()], "#page[ ]");
    }

    #[test]
    fn pages_go_after_the_paragraph_and_come_out_leaving_paragraphs_apart() {
        let text = "#set page(paper: \"a5\")\n\nOne one.\nStill one.\n\nTwo.\n";
        let source = Source::detached(text);
        assert_eq!(content_start(&source), text.find("One").unwrap());
        let cursor = text.find("Still").unwrap();
        let at = after_paragraph(&source, cursor);
        let block = written(&pdf(1, false, true));
        let (put, start) = with_put(text, at, &block, &[]);
        assert_eq!(
            put,
            format!("#set page(paper: \"a5\")\n\nOne one.\nStill one.\n\n{block}\n\nTwo.\n")
        );
        assert_eq!(&put[start..start + 6], "#page(");
        // A second behind it is on the next line; the end is the end.
        let source = Source::detached(put.as_str());
        let pages = inserted(&source);
        let behind = after_paragraph(&source, start);
        assert_eq!(behind, pages[0].range.end);
        let (two, _) = with_put(&put, behind, "#page[]", &pages);
        assert!(two.contains(&format!("{block}\n#page[]\n\nTwo.")), "{two}");
        let (end, _) = with_put(&two, content_end(&two), "#page[]", &pages);
        assert!(end.ends_with("Two.\n\n#page[]\n"), "{end}");
        let (start, _) = with_put(text, content_start(&Source::detached(text)), "#page[]", &[]);
        assert!(
            start.starts_with("#set page(paper: \"a5\")\n\n#page[]\n\nOne one."),
            "{start}"
        );
        // Taken out again, the text is as it was; a page between two
        // lines of one paragraph leaves them two paragraphs.
        let source = Source::detached(put.as_str());
        let pages = inserted(&source);
        assert_eq!(without(&put, pages[0].range.clone(), &pages), text);
        let tight = format!("A.\n{block}\nB.\n");
        let source = Source::detached(tight.as_str());
        let pages = inserted(&source);
        assert_eq!(
            without(&tight, pages[0].range.clone(), &pages),
            "A.\n\nB.\n"
        );
        let first = format!("{block}\n\nB.\n");
        let source = Source::detached(first.as_str());
        let pages = inserted(&source);
        assert_eq!(without(&first, pages[0].range.clone(), &pages), "B.\n");
    }

    #[test]
    fn pages_move_past_the_text_to_the_nearest_page_break() {
        let page = |n: usize| written(&pdf(n, false, true));
        let text = format!(
            "#set text(size: 11pt)\nIntro.\n\n#pagebreak()\nOne.\n\nTwo.\n\n{}\n{}\n\nThree.\n",
            page(1),
            page(2)
        );
        let source = Source::detached(text.as_str());
        let pages = inserted(&source);
        assert_eq!(run_of(&text, &pages, 1), 0..2);
        // Both up: past the text, to behind the page break.
        let (up, start, past) = moved(&source, &pages, 0, 1, true).unwrap();
        assert!(past);
        assert_eq!(
            up,
            format!(
                "#set text(size: 11pt)\nIntro.\n\n#pagebreak()\n\n{}\n{}\n\nOne.\n\nTwo.\n\nThree.\n",
                page(1),
                page(2)
            )
        );
        assert_eq!(&up[start..start + page(1).len()], page(1));
        // Up again: to the start of what is shown.
        let source = Source::detached(up.as_str());
        let pages = inserted(&source);
        let (top, _, _) = moved(&source, &pages, 0, 1, true).unwrap();
        assert!(
            top.starts_with(&format!(
                "#set text(size: 11pt)\n\n{}\n{}\n\nIntro.",
                page(1),
                page(2)
            )),
            "{top}"
        );
        let source = Source::detached(top.as_str());
        let pages = inserted(&source);
        assert!(moved(&source, &pages, 0, 1, true).is_err());
        // One page down past the other: they change places, past no
        // text.
        let (swapped, start, past) = moved(&source, &pages, 0, 0, false).unwrap();
        assert!(!past);
        assert!(
            swapped.contains(&format!("{}\n{}\n\nIntro.", page(2), page(1))),
            "{swapped}"
        );
        assert_eq!(&swapped[start..start + page(1).len()], page(1));
        // Down from the first place to the end, past all text.
        let (down, start, _) = moved(
            &Source::detached(text.as_str()),
            &inserted(&Source::detached(text.as_str())),
            0,
            1,
            false,
        )
        .unwrap();
        assert!(
            down.ends_with(&format!("Three.\n\n{}\n{}\n", page(1), page(2))),
            "{down}"
        );
        assert_eq!(&down[start..start + page(1).len()], page(1));
        let (range, with) = replacement(&text, &down);
        let mut again = text.clone();
        again.replace_range(range, &with);
        assert_eq!(again, down);
    }

    #[test]
    fn pages_of_a_file_are_named_as_in_a_print_dialog() {
        assert_eq!(page_numbers("", 3), Ok(vec![1, 2, 3]));
        assert_eq!(page_numbers("all", 3), Ok(vec![1, 2, 3]));
        assert_eq!(page_numbers("2", 3), Ok(vec![2]));
        assert_eq!(page_numbers("1-3", 3), Ok(vec![1, 2, 3]));
        assert_eq!(page_numbers("1, 4-6", 9), Ok(vec![1, 4, 5, 6]));
        assert_eq!(page_numbers("3,1", 3), Ok(vec![3, 1]));
        assert_eq!(page_numbers("8-", 9), Ok(vec![8, 9]));
        assert_eq!(
            page_numbers("4", 3),
            Err("there is no page 4: the file has 3 pages".to_owned())
        );
        for wrong in ["0", "a", "3-1", "-", "1,,2"] {
            assert!(page_numbers(wrong, 3).is_err(), "{wrong}");
        }
    }

    #[test]
    fn files_are_told_apart_by_what_they_hold() {
        let slides = slides_pdf();
        let Ok(Made::Pdf { sizes, newer }) = made_of("slides.pdf", &slides) else {
            panic!("a PDF");
        };
        assert_eq!(sizes.len(), 3);
        assert!((sizes[0].0 - 300.0).abs() < 0.5 && (sizes[0].1 - 200.0).abs() < 0.5);
        assert!(!newer);
        let broken = made_of("broken.pdf", &slides[..slides.len() / 3]);
        assert!(broken.is_err(), "{broken:?}");
        assert!(made_of("x.png", b"\x89PNG\r\n\x1a\n....").is_ok());
        assert!(made_of("x.svg", b"<?xml version=\"1.0\"?><svg/>").is_ok());
        let text = made_of("notes.txt", b"Hello");
        assert!(text.is_err_and(|why| why.contains("neither a PDF nor an image")));
    }

    #[test]
    fn a_file_from_elsewhere_is_copied_in_and_never_over_another() {
        let directory = directory("copy");
        let elsewhere = directory.join("elsewhere");
        let document = directory.join("document");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::create_dir_all(document.join("figures")).unwrap();
        std::fs::write(elsewhere.join("cover.pdf"), b"one").unwrap();
        // In the document's folder already: named as it is.
        std::fs::write(document.join("figures/in.png"), b"in").unwrap();
        assert_eq!(
            brought_in(&document.join("figures/in.png"), &document, &document),
            Ok(("figures/in.png".to_owned(), None))
        );
        // From elsewhere: copied, then taken as it is.
        let copied = brought_in(&elsewhere.join("cover.pdf"), &document, &document).unwrap();
        assert_eq!(
            copied,
            ("cover.pdf".to_owned(), Some(document.join("cover.pdf")))
        );
        assert_eq!(
            brought_in(&elsewhere.join("cover.pdf"), &document, &document),
            Ok(("cover.pdf".to_owned(), None))
        );
        // Another file of that name gets a name of its own.
        std::fs::create_dir_all(elsewhere.join("b")).unwrap();
        std::fs::write(elsewhere.join("b/cover.pdf"), b"two").unwrap();
        let other = brought_in(
            &elsewhere.join("b/cover.pdf"),
            &document,
            &document.join("figures"),
        );
        assert_eq!(
            other,
            Ok((
                "figures/cover.pdf".to_owned(),
                Some(document.join("figures/cover.pdf"))
            ))
        );
        std::fs::write(elsewhere.join("b/cover.pdf"), b"three").unwrap();
        let third = brought_in(&elsewhere.join("b/cover.pdf"), &document, &document);
        assert_eq!(
            third,
            Ok(("cover-2.pdf".to_owned(), Some(document.join("cover-2.pdf"))))
        );
        assert_eq!(std::fs::read(document.join("cover.pdf")).unwrap(), b"one");
        assert_eq!(
            std::fs::read(elsewhere.join("b/cover.pdf")).unwrap(),
            b"three"
        );
        let nowhere = brought_in(&elsewhere.join("none.pdf"), &document, &document);
        assert!(nowhere.is_err_and(|why| why.contains("no file")));
        let _ = std::fs::remove_dir_all(directory);
    }
}
