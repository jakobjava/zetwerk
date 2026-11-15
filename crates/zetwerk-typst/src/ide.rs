//! What Typst itself knows about a place in the source: what could be
//! typed there, what the thing there is, and where it is defined.

use std::ops::Range;

use comemo::Track;
use typst::introspection::Introspector;
use typst::model::BibliographyElem;
use typst::syntax::{FileId, Side, Source};
use typst::{World, WorldExt};
use typst_ide::{CompletionKind, Definition, IdeWorld, Tooltip};
use typst_layout::PagedDocument;

use crate::ProjectWorld;

impl IdeWorld for ProjectWorld {
    fn upcast(&self) -> &dyn World {
        self
    }
}

/// Something that could be typed at a place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// What it is listed as.
    pub label: String,
    /// What kind of thing it is, in a word.
    pub kind: &'static str,
    /// A sentence about it, if there is one.
    pub detail: Option<String>,
    /// What to put into the text, and how far into that the cursor goes:
    /// between the brackets of a call, say.
    pub text: String,
    pub cursor: usize,
}

/// What could be typed at byte `cursor` of `source`: the byte from which
/// on the candidates replace what is there, and the candidates. With
/// `explicit` the user asked; otherwise only places are completed where a
/// list is likely to be wanted. `document` is the last compiled one, if
/// there is one: labels come from it.
#[must_use]
pub fn complete(
    world: &ProjectWorld,
    document: Option<&PagedDocument>,
    source: &Source,
    cursor: usize,
    explicit: bool,
) -> Option<(usize, Vec<Candidate>)> {
    let (from, completions) = typst_ide::autocomplete(world, document, source, cursor, explicit)?;
    let candidates = completions
        .into_iter()
        .map(|completion| {
            let apply = completion.apply.as_deref().unwrap_or(&completion.label);
            let (text, cursor) = plain(apply);
            Candidate {
                label: completion.label.to_string(),
                kind: match completion.kind {
                    CompletionKind::Syntax => "syntax",
                    CompletionKind::Func => "function",
                    CompletionKind::Type => "type",
                    CompletionKind::Param => "parameter",
                    CompletionKind::Constant => "value",
                    CompletionKind::Path => "file",
                    CompletionKind::Package => "package",
                    CompletionKind::Label => "label",
                    CompletionKind::Font => "font",
                    CompletionKind::Symbol(_) => "symbol",
                },
                detail: completion.detail.map(|detail| detail.to_string()),
                text,
                cursor,
            }
        })
        .collect();
    Some((from, candidates))
}

/// A completion written with placeholders (`frac(${num}, ${denom})`) as
/// the text to put in, and where the cursor goes in it: where the first
/// placeholder was, or else the end.
fn plain(apply: &str) -> (String, usize) {
    let mut text = String::with_capacity(apply.len());
    let mut cursor = None;
    let mut rest = apply;
    while let Some(start) = rest.find("${") {
        text.push_str(&rest[..start]);
        cursor.get_or_insert(text.len());
        rest = match rest[start..].find('}') {
            Some(end) => &rest[start + end + 1..],
            None => "",
        };
    }
    text.push_str(rest);
    let cursor = cursor.unwrap_or(text.len());
    (text, cursor)
}

/// What the thing at byte `cursor` of `source` is, in a sentence or as a
/// piece of code: the value of a variable, the documentation of a
/// function.
#[must_use]
pub fn describe_at(
    world: &ProjectWorld,
    document: Option<&PagedDocument>,
    source: &Source,
    cursor: usize,
) -> Option<String> {
    match typst_ide::tooltip(world, document, source, cursor, Side::After)? {
        Tooltip::Text(text) | Tooltip::Code(text) => Some(text.to_string()),
    }
}

/// Where the thing at byte `cursor` of `source` is defined: a file and
/// the bytes of it. `None` for what is defined nowhere in a file (it is
/// built in) and for what is no name of anything.
#[must_use]
pub fn definition_of(
    world: &ProjectWorld,
    document: Option<&PagedDocument>,
    source: &Source,
    cursor: usize,
) -> Option<(FileId, Range<usize>)> {
    match typst_ide::definition(world, document, source, cursor, Side::After)? {
        Definition::Span(span) => {
            let file = span.id()?;
            Some((file, world.range(span)?))
        }
        Definition::File(file) => Some((file, 0..0)),
        Definition::Std(_) => None,
    }
}

/// The keys of the sources that the bibliographies of `document` hold,
/// each with the title of its source, if it has one: what can be cited
/// whatever kind of file the sources are kept in.
#[must_use]
pub fn bibliography_keys(document: &PagedDocument) -> Vec<(String, Option<String>)> {
    let introspector: &dyn Introspector = document.introspector().as_ref();
    BibliographyElem::keys(introspector.track())
        .into_iter()
        .map(|(key, title)| {
            let title = title.map(|title| title.to_string());
            (key.resolve().as_str().to_owned(), title)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_become_a_place_for_the_cursor() {
        assert_eq!(plain("frac(${num}, ${denom})"), ("frac(, )".to_owned(), 5));
        assert_eq!(plain("pi"), ("pi".to_owned(), 2));
        assert_eq!(plain("#let ${name} = ${}"), ("#let  = ".to_owned(), 5));
        assert_eq!(plain("${"), (String::new(), 0));
    }
}
