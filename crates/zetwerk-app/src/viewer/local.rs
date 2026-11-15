//! What changed is typeset by itself and put into the page.
//!
//! A key that changes more than a word of a paragraph (see
//! `zetwerk_predict`) used to wait for all of the document to be
//! typeset again: a third of a second for a thesis. Now the part that
//! changed is typeset alone (see [`zetwerk_typst::local`]), which takes
//! as long for a thesis as for a letter, and the page it is on shows it
//! at once, as Typst sets it. The whole follows when typing pauses, and
//! puts right what a part cannot know: numbers, and where pages break.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use typst::ecow::EcoVec;
use typst::model::Document;
use typst::{World, WorldExt};
use typst_layout::{Page, PagedDocument};
use zetwerk_typst::{PageMap, ProjectWorld, local};

use super::{Difference, Viewer, difference};
use crate::typst_text;

/// What a part of a document may not hold if it is to be typeset by
/// itself: things that are what they are only among all the rest, or
/// that stand elsewhere on the page than where they are written.
const NOT_ALONE: &[&str] = &[
    "#bibliography",
    "#outline",
    "#include",
    "#pagebreak",
    "#colbreak",
    "#place",
];

#[cfg(test)]
thread_local! {
    /// How many parts were typeset on this thread, for tests.
    pub(super) static PARTS_TYPESET: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl Viewer {
    /// Typesets what changed since the pages on screen were made, by
    /// itself, and puts it into the page it is on. Afterwards the pages
    /// show the text as it is.
    ///
    /// The error says why that could not be done; the text then waits
    /// for all of it to be typeset.
    pub(super) fn typeset_locally(&mut self) -> Result<(), String> {
        let started = Instant::now();
        let (Some(document), Some(before)) = (self.document.clone(), self.compiled_world.clone())
        else {
            return Err("nothing compiled yet".to_owned());
        };
        // The page: where the cursor was last found, or one beside it.
        let near = self.cursor_page.get();
        let pages = document.pages();
        let page_of = |region: &local::Region| {
            let shows = |index: &usize| {
                let page = pages.get(*index);
                page.is_some_and(|page| region.is_on(&page.frame))
            };
            let index = [Some(near), Some(near + 1), near.checked_sub(1)]
                .into_iter()
                .flatten()
                .find(shows)
                .ok_or("the part is on no page near the cursor")?;
            if [index.checked_sub(1), Some(index + 1)]
                .iter()
                .flatten()
                .any(shows)
            {
                return Err("the part is on two pages");
            }
            Ok(index)
        };
        let (part, index) = self.typeset_part(&before, &page_of)?;
        let page = &pages[index];
        // What every page has around its text stays where it is.
        let map = self
            .maps
            .entry(index)
            .or_insert_with(|| PageMap::build(&before, &page.frame));
        let stays: HashSet<usize> = map
            .glyphs()
            .iter()
            .filter(|glyph| glyph.furniture)
            .filter_map(|glyph| map.runs().get(glyph.run)?.path.first().copied())
            .collect();
        let frame = local::splice(&page.frame, part.typeset.pages(), &part.region, &|item| {
            stays.contains(&item)
        })
        .map_err(|why| format!("{why:?}"))?;

        let mut patched: EcoVec<Page> = pages.iter().cloned().collect();
        if let Some(slot) = patched.make_mut().get_mut(index) {
            slot.frame = frame;
        }
        let info = document.info().clone();
        self.document = Some(Arc::new(PagedDocument::new(patched, info)));
        self.compiled_world = Some(Arc::new(part.world));
        self.document_revision = Some(part.revision);
        self.predicted = None;
        self.predicted_at = None;
        self.maps.clear();
        self.page_lines.get_mut().clear();
        self.landed.set(None);
        self.planned.retain(|(planned, _, _), _| *planned != index);
        self.stats.local += 1;
        self.stats.local_ms = started.elapsed().as_secs_f64() * 1000.0;
        Ok(())
    }

    /// Finds what changed since `before` (what the pages on screen were
    /// made of) and typesets it by itself, with the paragraphs beside it,
    /// if `page_of` finds the one page it is on: which is returned with
    /// it. Asked before the part is typeset: a part that runs on over
    /// the rest of the document (a `$` not yet closed) takes as long to
    /// typeset as the rest, and is on no one page.
    fn typeset_part(
        &mut self,
        before: &ProjectWorld,
        page_of: &dyn Fn(&local::Region) -> Result<usize, &'static str>,
    ) -> Result<(Part, usize), String> {
        if !self.in_parts {
            return Err("typesetting in parts is off".to_owned());
        }
        let editor = self.editor.as_ref().ok_or("no file is being edited")?;
        let file = editor.file();
        if file != self.project.main() {
            return Err("only the main file is typeset in parts".to_owned());
        }
        let columns = typst_text::rule_value(editor.source(), "page", "columns");
        if columns.is_some_and(|columns| columns.trim() != "1") {
            return Err("pages with columns are typeset whole".to_owned());
        }
        let gone = |_| "the compiled source is gone".to_owned();
        let written = before.written(file).map_err(gone)?;
        let Difference::Replacement { offset, text, .. } =
            difference(written.text(), editor.text())
        else {
            return Err("nothing changed".to_owned());
        };
        let put_in = offset..offset + text.len();
        // The text of a footnote stands at the foot of the page, which
        // is not where the paragraph it belongs to is put.
        if typst_text::is_in_call(editor.source(), put_in.start, "footnote") {
            return Err("a footnote is typeset with all of the page".to_owned());
        }
        let (revision, world) = self.snapshot();
        let old = before.source(file).map_err(gone)?;
        let new = world.source(file).map_err(gone)?;
        let changed = world.shown_at(file, put_in.start)..world.shown_at(file, put_in.end);
        let region =
            local::changed_region(&old, &new, &changed).map_err(|why| format!("{why:?}"))?;
        let holds = |bytes: &std::ops::Range<usize>| {
            let part = new.text().get(bytes.clone()).unwrap_or("");
            NOT_ALONE.iter().find(|word| part.contains(**word))
        };
        if let Some(word) = holds(&region.bytes) {
            return Err(format!("{word} is not typeset by itself"));
        }
        // The paragraphs beside it are typeset with it, to place it by,
        // unless they are of the kind that cannot be.
        let region = if holds(&region.kept).is_some() {
            region.alone(&new)
        } else {
            region
        };
        let index = page_of(&region)?;
        let lead = typst_text::lead_for_part(&new, region.kept.start);
        let alone = world.with_text(local::cut_down(&new, &region, &lead));
        #[cfg(test)]
        PARTS_TYPESET.set(PARTS_TYPESET.get() + 1);
        let compiled = zetwerk_typst::compile(&alone);
        let Some(typeset) = compiled.document else {
            let first = compiled.errors.first();
            // An equation Typst refuses is shown as it is written; with
            // it so, the part is typeset after all.
            let refused = first.and_then(|error| {
                let at = alone.range(error.span)?.start;
                let whole = region.in_whole(at, lead.len())?;
                Some((world.written_at(file, whole), error.message.to_string()))
            });
            if let Some((at, why)) = refused
                && self.show_as_written(at, &why)
            {
                return self.typeset_part(before, page_of);
            }
            let why = first.map(|error| zetwerk_typst::describe(&alone, error));
            return Err(format!(
                "the part cannot be typeset by itself: {}",
                why.unwrap_or_default()
            ));
        };
        let part = Part {
            revision,
            world,
            region,
            typeset,
        };
        Ok((part, index))
    }
}

/// A part of the text typeset by itself: of which revision of the text,
/// the whole text it is a part of, which part, and the pages made of it.
struct Part {
    revision: u64,
    world: ProjectWorld,
    region: local::Region,
    typeset: PagedDocument,
}
