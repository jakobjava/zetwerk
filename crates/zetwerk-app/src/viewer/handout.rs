//! Handouts of a deck: `:pdf handout` writes `name-handout.pdf` beside
//! the main file, six slides to an A4 page; `:pdf handout 1` (2, 3, 4,
//! 6, 9) puts that many on a page. Of an animated slide only its last
//! step is in it, which shows all of it.
//!
//! The user's document is not touched for it. The slides are written as
//! a PDF to a folder of zetwerk's own in the state directory, and a small
//! document that places its pages as images (`image("deck.pdf", page:
//! n)`, which Typst reads as vector graphics) is typeset with the same
//! fonts and written out. That is done on a thread of its own, which
//! wakes the window when it is over, as exports are; without a window,
//! on the spot.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};

use typst_layout::PagedDocument;
use zetwerk_typst::{Environment, Project, ProjectWorld};

use super::Viewer;

/// How many slides go on a page where nothing else is asked for.
const PER_PAGE: usize = 6;

/// What wakes the window when a handout is written.
pub(super) type Wake = Arc<Mutex<Box<dyn Fn() + Send>>>;

/// How `count` slides go on a page: whether the page lies on its side,
/// how many go across and how many down. `None` for a count that does
/// not make a good page.
#[must_use]
pub fn arranged(count: usize) -> Option<(bool, usize, usize)> {
    Some(match count {
        1 => (true, 1, 1),
        2 => (false, 1, 2),
        3 => (false, 1, 3),
        4 => (true, 2, 2),
        6 => (false, 2, 3),
        9 => (false, 3, 3),
        _ => return None,
    })
}

/// The document that places `slides` pages of `deck.pdf`, `count` to a
/// page.
#[must_use]
pub fn wrapper(slides: usize, count: usize) -> Option<String> {
    let (flipped, across, down) = arranged(count)?;
    let mut text = format!(
        "#set page(paper: \"a4\", flipped: {flipped}, margin: 1.5cm)\n\
         #let slide(n) = align(center + horizon, box(stroke: 0.5pt + luma(170), \
         image(\"deck.pdf\", page: n, width: 100%, height: 100%, fit: \"contain\")))\n"
    );
    let numbers: Vec<usize> = (1..=slides).collect();
    for (index, chunk) in numbers.chunks(count).enumerate() {
        if index > 0 {
            text.push_str("#pagebreak()\n");
        }
        let cells: Vec<String> = chunk.iter().map(|n| format!("slide({n})")).collect();
        text.push_str(&format!(
            "#grid(columns: (1fr,) * {across}, rows: (1fr,) * {down}, gutter: 8mm, {})\n",
            cells.join(", ")
        ));
    }
    Some(text)
}

/// Writes the handout of `pages` of `document`, `count` to a page, to
/// `target`, by way of `scratch`. Says what was written, or why not.
///
/// # Errors
///
/// Where a PDF cannot be made or a file cannot be written.
pub fn write(
    env: Arc<Environment>,
    document: &PagedDocument,
    world: &ProjectWorld,
    pages: &[usize],
    count: usize,
    scratch: &Path,
    target: &Path,
) -> Result<String, String> {
    let deck = zetwerk_typst::export_pdf_pages(document, world, Some(pages))
        .map_err(|errors| errors.join("; "))?;
    std::fs::create_dir_all(scratch)
        .map_err(|err| format!("cannot make {}: {err}", scratch.display()))?;
    let placed = scratch.join("deck.pdf");
    std::fs::write(&placed, &deck)
        .map_err(|err| format!("cannot write {}: {err}", placed.display()))?;
    let text = wrapper(pages.len(), count).ok_or("not so many to a page")?;
    let project = Project::in_memory(env, scratch.to_owned(), "handout.typ", text)
        .ok_or("cannot set up the handout")?;
    let handout_world = project.snapshot();
    let compiled = zetwerk_typst::compile(&handout_world);
    let _ = std::fs::remove_file(&placed);
    let Some(handout) = compiled.document else {
        let why: Vec<String> = compiled
            .errors
            .iter()
            .map(|error| zetwerk_typst::describe(&handout_world, error))
            .collect();
        return Err(format!("the handout was not typeset: {}", why.join("; ")));
    };
    let bytes = zetwerk_typst::export_pdf_pages(&handout, &handout_world, None)
        .map_err(|errors| errors.join("; "))?;
    zetwerk_core::write_atomically(target, &bytes)
        .map_err(|err| format!("cannot write {}: {err}", target.display()))?;
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    Ok(format!(
        "{name} written: {} slides, {count} to a page, on {} page(s)",
        pages.len(),
        handout.pages().len()
    ))
}

/// The handout being written, and what wakes the window after it.
#[derive(Default)]
pub(super) struct Handouts {
    pub(super) wake: Option<Wake>,
    running: Option<Receiver<Result<String, String>>>,
}

impl Viewer {
    /// Has handouts written on a thread of their own, `wake` called from
    /// it when one is over.
    pub fn wake_for_handouts(&mut self, wake: impl Fn() + Send + 'static) {
        self.present.handouts.wake = Some(Arc::new(Mutex::new(Box::new(wake))));
    }

    /// `:pdf handout`, with what is behind `pdf`. Returns whether it was
    /// that.
    pub(super) fn handout_command(&mut self, argument: &str) -> bool {
        let mut words = argument.split_whitespace();
        if words.next() != Some("handout") {
            return false;
        }
        let count = match words.next().map(str::parse::<usize>) {
            None => PER_PAGE,
            Some(Ok(count)) if arranged(count).is_some() => count,
            Some(_) => {
                self.alert("so many to a page, please: :pdf handout 1, 2, 3, 4, 6 or 9");
                return true;
            }
        };
        if let Err(why) = self.begin_handout(count) {
            self.alert(format!("no handout: {why}"));
        }
        true
    }

    fn begin_handout(&mut self, count: usize) -> Result<(), String> {
        if self.present.handouts.running.is_some() {
            return Err("one is being written still".to_owned());
        }
        let export = self.for_export()?;
        let main = export.main.clone().ok_or("the document is not a file")?;
        let deck = self.deck().ok_or("there are no pages")?;
        let pages: Vec<usize> = deck.slides.iter().map(|slide| slide.end - 1).collect();
        let state_dir = self
            .state_dir
            .clone()
            .ok_or("there is no state directory")?;
        let stem = main
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let target = main.with_file_name(format!("{stem}-handout.pdf"));
        let scratch: PathBuf = state_dir
            .join("handout")
            .join(std::process::id().to_string());
        let env = Arc::clone(self.project.env());
        let job = move || {
            write(
                env,
                &export.document,
                &export.world,
                &pages,
                count,
                &scratch,
                &target,
            )
        };
        let Some(wake) = self.present.handouts.wake.clone() else {
            match job() {
                Ok(told) => self.notice(told),
                Err(why) => self.alert(format!("no handout: {why}")),
            }
            return Ok(());
        };
        let (done, running) = mpsc::channel();
        std::thread::Builder::new()
            .name("zetwerk-handout".into())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job))
                    .unwrap_or_else(|_| Err("the handout crashed".to_owned()));
                let _ = done.send(outcome);
                (wake.lock().unwrap_or_else(PoisonError::into_inner))();
            })
            .map_err(|err| format!("no thread for it: {err}"))?;
        self.present.handouts.running = Some(running);
        self.notice(format!("writing {stem}-handout.pdf …"));
        Ok(())
    }

    /// Takes up what the thread that writes a handout has to say.
    pub(super) fn handout_progress(&mut self) {
        let Some(running) = &self.present.handouts.running else {
            return;
        };
        match running.try_recv() {
            Err(TryRecvError::Empty) => {}
            Ok(Ok(told)) => {
                self.present.handouts.running = None;
                self.notice(told);
            }
            Ok(Err(why)) => {
                self.present.handouts.running = None;
                self.alert(format!("no handout: {why}"));
            }
            Err(TryRecvError::Disconnected) => {
                self.present.handouts.running = None;
                self.alert("no handout: the thread that wrote it is gone");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slides_are_placed_so_many_to_a_page() {
        assert_eq!(arranged(6), Some((false, 2, 3)));
        assert_eq!(arranged(5), None);
        let text = wrapper(7, 6).unwrap_or_default();
        assert_eq!(text.matches("#pagebreak()").count(), 1);
        assert!(text.contains("slide(7)") && !text.contains("slide(8)"));
        assert!(wrapper(3, 7).is_none());
    }
}
