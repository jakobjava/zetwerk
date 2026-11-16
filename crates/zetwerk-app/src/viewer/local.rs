//! What changed is typeset by itself and put into the page.
//!
//! A key that changes more than a word of a paragraph (see
//! `zetwerk_predict`) used to wait for all of the document to be
//! typeset again: a third of a second for a thesis. Now the part that
//! changed is typeset alone (see [`zetwerk_typst::local`]), which takes
//! as long for a thesis as for a letter, and the page it is on shows it
//! at once, as Typst sets it. The whole follows when typing pauses, and
//! puts right what a part cannot know: numbers, and where pages break.

use std::cell::{OnceCell, RefCell};
use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

use typst::ecow::EcoVec;
use typst::model::Document;
use typst::syntax::{LinkedNode, Source, SyntaxKind};
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
    // A figure that floats to the top or the foot of the page.
    "placement:",
    // What is cited or referred to shows by its name in a part without
    // it, in another width than in the whole.
    "#cite",
    "#ref(",
];

#[cfg(test)]
thread_local! {
    /// How many parts were typeset on this thread, for tests.
    pub(super) static PARTS_TYPESET: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The pages on screen: those of a document typeset whole, or those with
/// a part typeset since put into one of them.
///
/// What draws a page or finds the text on it asks [`Pages::pages`]. What
/// asks about the document as a whole (what can be referred to, what a
/// name stands for) asks [`Pages::whole`]: the pages with a part put in,
/// wrapped as a document again, cost Typst a look at all of them (15 ms
/// on 312 pages, a third of a key there), with every key.
#[derive(Clone)]
pub(super) struct Pages {
    /// The document typeset whole, and what it was typeset of.
    whole: Arc<PagedDocument>,
    world: Arc<ProjectWorld>,
    /// Its pages with a part put into one of them, and which, if one is.
    spliced: Option<(usize, EcoVec<Page>)>,
    /// Those as a document, made when it is first asked for.
    wrapped: OnceCell<Arc<PagedDocument>>,
    /// The items of a page of `whole` that stand around its text, and
    /// which page, as last asked for (see [`Pages::furniture`]).
    furniture: RefCell<Option<(usize, Arc<HashSet<usize>>)>>,
}

impl Pages {
    /// The pages of `whole`, typeset of `world`, as they are.
    pub(super) fn new(whole: Arc<PagedDocument>, world: Arc<ProjectWorld>) -> Self {
        Self {
            whole,
            world,
            spliced: None,
            wrapped: OnceCell::new(),
            furniture: RefCell::new(None),
        }
    }

    /// The page a part was put into, if one was.
    pub(super) fn spliced_at(&self) -> Option<usize> {
        self.spliced.as_ref().map(|(index, _)| *index)
    }

    /// The items of page `index` of the document typeset whole that stand
    /// around the text of every page (a running head, the page number):
    /// what a part put in does not move. Found once for a page that parts
    /// are put into key after key.
    fn furniture(&self, index: usize) -> Arc<HashSet<usize>> {
        if let Some((of, items)) = &*self.furniture.borrow()
            && *of == index
        {
            return Arc::clone(items);
        }
        let items = self
            .whole
            .pages()
            .get(index)
            .map_or_else(HashSet::new, |page| {
                let map = PageMap::build(&self.world, &page.frame);
                map.glyphs()
                    .iter()
                    .filter(|glyph| glyph.furniture)
                    .filter_map(|glyph| map.runs().get(glyph.run)?.path.first().copied())
                    .collect()
            });
        let items = Arc::new(items);
        *self.furniture.borrow_mut() = Some((index, Arc::clone(&items)));
        items
    }

    /// The pages, as they are shown.
    pub(super) fn pages(&self) -> &[Page] {
        self.spliced
            .as_ref()
            .map_or(self.whole.pages(), |(_, pages)| pages)
    }

    /// The document typeset whole that the pages are of.
    pub(super) fn whole(&self) -> &Arc<PagedDocument> {
        &self.whole
    }

    /// Whether the pages are those of the document typeset whole, with
    /// no part put in.
    pub(super) fn is_whole(&self) -> bool {
        self.spliced.is_none()
    }

    /// The pages as a document, for what is to be made of them as they
    /// are shown (a PDF of the text as it is): wrapped the first time it
    /// is asked for, where a part was put in.
    pub(super) fn document(&self) -> &Arc<PagedDocument> {
        let Some((_, pages)) = &self.spliced else {
            return &self.whole;
        };
        self.wrapped.get_or_init(|| {
            let info = self.whole.info().clone();
            Arc::new(PagedDocument::new(pages.clone(), info))
        })
    }
}

impl Viewer {
    /// Typesets what changed since the pages on screen were made, by
    /// itself, and puts it into the page it is on. Afterwards the pages
    /// show the text as it is.
    ///
    /// The error says why that could not be done; the text then waits
    /// for all of it to be typeset.
    pub(super) fn typeset_locally(&mut self) -> Result<(), String> {
        // What is put into the text to show it stays that of the pages on
        // screen where they stay: typing over, they are to be made again
        // without the marks of it.
        let shown_put = self.shown_put.clone();
        let done = self.put_part_in();
        if done.is_err() {
            self.shown_put = shown_put;
        }
        done
    }

    /// See [`Viewer::typeset_locally`].
    fn put_part_in(&mut self) -> Result<(), String> {
        let started = Instant::now();
        let (Some(on_screen), Some(was)) = (self.document.clone(), self.compiled_world.clone())
        else {
            return Err("nothing compiled yet".to_owned());
        };
        // The part holds all that changed since the pages were typeset
        // whole, and goes into the page as it was then: what a part before
        // it pushed off the foot of the page comes back where this one is
        // shorter.
        let before = Arc::clone(&on_screen.world);
        let pages = on_screen.whole.pages();
        if self.back_at_whole(&on_screen) {
            return Ok(());
        }
        // The page: where the cursor was last found, or one beside it.
        let near = self.cursor_page.get();
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
        // Margins of two sides are swapped on every other page; a part is
        // typeset on a first page.
        let text = self.editor.as_ref().map_or("", |editor| editor.text());
        if index % 2 == 1 && (text.contains("inside:") || text.contains("outside:")) {
            return Err("a left page with margins of two sides is typeset whole".to_owned());
        }
        // What every page has around its text stays where it is.
        let stays = on_screen.furniture(index);
        let foot = local::foot_of_text(&part.typeset).ok_or("the foot of the text is not known")?;
        let frame = local::splice(
            &page.frame,
            part.typeset.pages(),
            &part.region,
            foot,
            &|item| stays.contains(&item),
        )
        .map_err(|why| format!("{why:?}"))?;

        let mut patched: EcoVec<Page> = pages.iter().cloned().collect();
        if let Some(slot) = patched.make_mut().get_mut(index) {
            slot.frame = frame;
        }
        let other = on_screen.spliced_at().filter(|other| *other != index);
        let world = Arc::new(part.world);
        // The page put in is mapped anew, and the one the part before was
        // put in, if that is another (it is as typeset whole again); the
        // others stand as they stood, the text they show moved with what
        // changed.
        self.maps_moved(&was, &world, &[Some(index), other]);
        self.document = Some(Pages {
            spliced: Some((index, patched)),
            wrapped: OnceCell::new(),
            ..on_screen
        });
        self.compiled_world = Some(world);
        self.document_revision = Some(part.revision);
        self.predicted = None;
        self.predicted_at = None;
        self.landed.set(None);
        self.planned
            .retain(|(planned, _, _), _| *planned != index && Some(*planned) != other);
        self.margin_pages_changed(&[Some(index), other]);
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
        if columns.is_some_and(|columns| columns.trim() != "1")
            || editor.text().contains("columns(")
            || editor.text().contains("columns.with(")
        {
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
        // A footnote put in or taken out changes the foot of the page,
        // which is not put in with the part. What stood where the part
        // is: all before and behind it is as it was.
        let notes = |text: &str| text.matches("#footnote").count();
        let was = (region.bytes.end + old.text().len())
            .checked_sub(new.text().len())
            .and_then(|end| old.text().get(region.bytes.start..end));
        let now = new.text().get(region.bytes.clone()).unwrap_or("");
        if was.is_none_or(|was| notes(was) != notes(now)) {
            return Err(
                "a footnote put in or taken out is typeset with all of the page".to_owned(),
            );
        }
        // What is referred to and not in the part stands in its lead.
        let stand_ins = typst_text::stand_ins(&new, &region.kept);
        if stand_ins.is_none() && refers_beyond(&new, &region.bytes) {
            return Err(
                "a reference to what is not in the part is not typeset by itself".to_owned(),
            );
        }
        // The paragraphs beside it are typeset with it, to place it by,
        // unless they are of the kind that cannot be.
        let alone = stand_ins.is_none() && refers_beyond(&new, &region.kept);
        let region = if holds(&region.kept).is_some() || alone {
            // Then it is the first paragraph of what is typeset, which
            // Typst does not indent as it does one behind another.
            if new.text().contains("first-line-indent") {
                return Err("a paragraph by itself is indented otherwise".to_owned());
            }
            region.alone(&new)
        } else {
            region
        };
        let index = page_of(&region)?;
        let mut lead = typst_text::lead_for_part(&new, region.kept.start);
        // Before the numbers of the part are set: the stand-ins count too.
        let numbers = lead.find("\n#counter(").map_or(lead.len(), |at| at + 1);
        lead.insert_str(numbers, &stand_ins.unwrap_or_default());
        let cut = local::cut_down(&new, &region, &lead).map_err(|why| format!("{why:?}"))?;
        let alone = world.with_text(cut);
        #[cfg(test)]
        PARTS_TYPESET.set(PARTS_TYPESET.get() + 1);
        let compiled = self.compile_part(&alone)?;
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

impl Viewer {
    /// Shows the pages typeset whole again where the text is what they
    /// were typeset of (all typed since was taken back), in place of
    /// those with a part: returns whether it is.
    fn back_at_whole(&mut self, shown: &Pages) -> bool {
        let Some(editor) = &self.editor else {
            return false;
        };
        let file = editor.file();
        let same = shown
            .world
            .written(file)
            .is_ok_and(|written| written.text() == editor.text());
        // Not where they show what typing put in to be seen (a refused
        // equation as it is written, the place of the cursor): that is
        // not to stay once typing is over.
        if shown.is_whole()
            || !same
            || shown.world.shows_more()
            || self.stale_files.iter().any(|stale| *stale != file)
        {
            return false;
        }
        let world = Arc::clone(&shown.world);
        let other = shown.spliced_at();
        if let Some(was) = self.compiled_world.clone() {
            self.maps_moved(&was, &world, &[other]);
        }
        self.document = Some(Pages {
            spliced: None,
            wrapped: OnceCell::new(),
            ..shown.clone()
        });
        self.compiled_world = Some(world);
        self.document_revision = Some(self.revision);
        self.predicted = None;
        self.predicted_at = None;
        self.landed.set(None);
        self.planned
            .retain(|(planned, _, _), _| Some(*planned) != other);
        self.margin_pages_changed(&[other]);
        true
    }

    /// The maps and the lines of the pages, which were of the text of
    /// `was`, made those of the text of `now`: where a page stays as it
    /// was, the text it shows only moved with what changed (as places do,
    /// see [`super::Change`]). Those of the pages `renew`, and those that
    /// show any of what changed, are made anew when they are asked for.
    fn maps_moved(&mut self, was: &ProjectWorld, now: &ProjectWorld, renew: &[Option<usize>]) {
        let file = self.file_id();
        let (Ok(was), Ok(now)) = (was.written(file), now.written(file)) else {
            self.maps.clear();
            self.page_lines.get_mut().clear();
            return;
        };
        let changed = match difference(was.text(), now.text()) {
            Difference::Nothing => None,
            Difference::Replacement {
                offset,
                removed,
                text,
            } => Some((offset..offset + removed, text.len())),
        };
        let kept = |index: &usize| !renew.contains(&Some(*index));
        self.maps.retain(|index, map| {
            kept(index)
                && changed
                    .as_ref()
                    .is_none_or(|(taken, put)| map.shift(file, taken, *put))
        });
        self.page_lines.get_mut().retain(|index, lines| {
            kept(index)
                && changed
                    .as_ref()
                    .is_none_or(|(taken, put)| lines.shift(file, taken, *put))
        });
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

/// Whether the text of `source` within `bytes` refers to a label it does
/// not hold: a part typeset by itself shows such a reference by the name
/// of the label (see [`typst_text::lead_for_part`]), the whole by what
/// it refers to.
fn refers_beyond(source: &Source, bytes: &Range<usize>) -> bool {
    fn beyond(node: &LinkedNode, bytes: &Range<usize>, text: &str) -> bool {
        let range = node.range();
        if range.end <= bytes.start || range.start >= bytes.end {
            return false;
        }
        if node.kind() == SyntaxKind::Ref {
            let marker = node
                .children()
                .find(|child| child.kind() == SyntaxKind::RefMarker);
            let label =
                marker.map(|marker| marker.get().leaf_text().trim_start_matches('@').to_owned());
            return label.is_none_or(|label| !text.contains(&format!("<{label}>")));
        }
        node.children().any(|child| beyond(&child, bytes, text))
    }
    let Some(text) = source.text().get(bytes.clone()) else {
        return true;
    };
    text.contains('@') && beyond(&LinkedNode::new(source.root()), bytes, text)
}
