//! Writing the document out for others: as a PDF, as images of its
//! pages, and a figure of it alone.
//!
//! - `:pdf [pages] [file]` writes a PDF: of all pages, or of those named
//!   (`3-7`, `5`, `1,4-6`, `10-`, `-3`).
//! - `:png [pages] [300dpi] [file]` and `:svg [pages] [file]` write
//!   pages as images. One page gives one file; several give numbered
//!   ones, `name-01.png`, `name-02.png`, …, numbered as the pages of the
//!   document are and with as many digits as its last page has.
//! - `:figure png [300dpi] [file]` and `:figure svg [file]` write the
//!   figure the cursor is in, alone.
//!
//! A file is named from the document's directory. Without a name the
//! export goes beside the main file under its name: `thesis.pdf`,
//! `thesis-07.png` (numbered whenever the document has more than one
//! page, so that one page does not take the place of another),
//! `thesis-plot.svg` for the figure labelled `<plot>` and
//! `thesis-figure-3.svg` for the third figure of the document if it has
//! no label.
//!
//! What was decided:
//!
//! - **One revision.** An export is of the pages as they were compiled
//!   from one revision of the text: the job holds those pages and the
//!   sources they came from, and nothing typed afterwards reaches it.
//!   Asked for while the text is ahead of its pages, the export waits
//!   for the pages of the text as it is and starts when they are there.
//! - **Off the window's thread.** The files are made and written on a
//!   thread of their own (see [`Viewer::export_in_background`]), which
//!   wakes the window when it is done, as the compiler does. The status
//!   line says that the export began, and then where it went or what
//!   went wrong. Without a window there is nobody to keep waiting, and
//!   the export is done on the spot.
//! - **One at a time.** While an export is waiting or being written,
//!   another is refused and the status line says so. Queueing them
//!   would let a held key pile up minutes of work.
//! - **The figure.** Its area is the box around everything Typst draws
//!   for it — body and caption, wherever it floats to — found from the
//!   marks Typst leaves in the page (see [`zetwerk_typst::figure_area`]),
//!   and [`FIGURE_MARGIN`] points more on every side, as far as the page
//!   goes. The image is that part of the page: what the page shows
//!   there, on the page's background. The cursor is in a figure when it
//!   is in its call in the source (the `#` before it and the label
//!   behind it count), which is where it is when it stands on the
//!   figure's caption or on text of its body on the page; of figures in
//!   figures the innermost is meant. In no figure, nothing is written
//!   and the status line says so.
//! - **Nothing is lost.** Every file is written beside where it goes and
//!   renamed into place, so a file is never half there. A file that was
//!   there is replaced, and the status line says that it was. A file
//!   the document is made from (an image it shows, say) is never
//!   written over.

use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;

use typst::layout::{Abs, Point, Rect};
use typst_layout::PagedDocument;
use zetwerk_typst::{FigureArea, ProjectWorld};

use super::Viewer;
use super::agent::Export;

/// Pixels to the inch of a page written as a PNG, unless more or fewer
/// are asked for: two to the point, which reads well on a screen and is
/// not too large to send.
pub const PAGE_DPI: f64 = 144.0;
/// The same for a figure, which is smaller than a page and is looked at
/// more closely.
pub const FIGURE_DPI: f64 = 300.0;
/// The fewest and the most pixels to the inch that can be asked for.
pub const DPI_RANGE: std::ops::RangeInclusive<f64> = 20.0..=1200.0;
/// How much of the page around a figure goes along with it, in points:
/// enough that nothing drawn at its edge is cut, less than the space
/// Typst leaves between a figure and the text around it.
pub const FIGURE_MARGIN: f64 = 4.0;

/// How many threads an export has to itself: one that makes the files,
/// and one for what Typst's exporters do beside it.
const EXPORT_THREADS: usize = 2;

/// The kind of file an export writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Pdf,
    Png,
    Svg,
}

impl Format {
    /// What the name of such a file ends in.
    #[must_use]
    pub fn ending(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Png => "png",
            Self::Svg => "svg",
        }
    }
}

/// Pages named by the user: `3-7`, `5`, `1,4-6`, `10-`, `-3`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pages(Vec<(Option<usize>, Option<usize>)>);

impl Pages {
    /// Reads `text` as pages: numbers and stretches of them (`from-to`,
    /// either end of which may be left out), with commas between.
    /// `None` if it is something else, such as the name of a file.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let number = |text: &str| match text {
            "" => Some(None),
            digits if digits.bytes().all(|byte| byte.is_ascii_digit()) => {
                digits.parse().ok().filter(|page| *page > 0).map(Some)
            }
            _ => None,
        };
        let stretch = |part: &str| match part.split_once('-') {
            Some(("", "")) => None,
            Some((from, to)) => {
                let (from, to) = (number(from)?, number(to)?);
                let ordered = from.zip(to).is_none_or(|(from, to)| from <= to);
                ordered.then_some((from, to))
            }
            None => number(part)?.map(|page| (Some(page), Some(page))),
        };
        let stretches: Option<Vec<_>> = text.split(',').map(stretch).collect();
        stretches.filter(|found| !found.is_empty()).map(Self)
    }

    /// Which pages of a document of `count` pages these are: counting
    /// from 0, each once, in the order of the document. A page that is
    /// named and not there is an error.
    pub fn resolve(&self, count: usize) -> Result<Vec<usize>, String> {
        let mut chosen = Vec::new();
        for (from, to) in &self.0 {
            let (from, to) = (from.unwrap_or(1), to.unwrap_or(count));
            if let Some(missing) = [from, to].into_iter().find(|page| *page > count) {
                return Err(format!(
                    "page {missing} does not exist; the document has {count} page(s)"
                ));
            }
            chosen.extend((from..=to).map(|page| page - 1));
        }
        chosen.sort_unstable();
        chosen.dedup();
        if chosen.is_empty() {
            return Err("those are no pages of the document".to_owned());
        }
        Ok(chosen)
    }
}

/// What of the document an export is of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum What {
    /// These pages; all of them where none are named.
    Pages(Option<Pages>),
    /// The figure the cursor is in.
    FigureAtCursor,
    /// The figure this line of the file being edited, counting from 1,
    /// is part of.
    FigureAtLine(usize),
}

/// An export as it is asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct Wanted {
    pub format: Format,
    pub what: What,
    /// Pixels to the inch of a PNG, if not the usual.
    pub dpi: Option<f64>,
    /// The name of the file, from the document's directory, if not the
    /// usual.
    pub file: Option<String>,
}

impl Wanted {
    /// Reads the command `name` (`pdf`, `png`, `svg` or `figure`) with
    /// `argument` behind it.
    pub fn parse(name: &str, argument: &str) -> Result<Self, String> {
        let (format, figure, mut rest) = match name {
            "pdf" => (Format::Pdf, false, argument),
            "png" => (Format::Png, false, argument),
            "svg" => (Format::Svg, false, argument),
            _ => match first_word(argument) {
                ("png", rest) => (Format::Png, true, rest),
                ("svg", rest) => (Format::Svg, true, rest),
                _ => return Err("as what, please: :figure png [file] or :figure svg [file]".into()),
            },
        };
        let (mut pages, mut dpi) = (None, None);
        loop {
            let (word, behind) = first_word(rest);
            let as_dpi = word
                .strip_suffix("dpi")
                .and_then(|number| number.parse::<f64>().ok());
            if let (false, None, Some(named)) = (figure, &pages, Pages::parse(word)) {
                pages = Some(named);
            } else if let (Format::Png, None, Some(number)) = (format, dpi, as_dpi) {
                dpi = Some(number);
            } else {
                break;
            }
            rest = behind;
        }
        let file = rest.trim();
        Ok(Self {
            format,
            what: if figure {
                What::FigureAtCursor
            } else {
                What::Pages(pages)
            },
            dpi,
            file: (!file.is_empty()).then(|| file.to_owned()),
        })
    }
}

/// The first word of `text` and what is behind it.
fn first_word(text: &str) -> (&str, &str) {
    let text = text.trim_start();
    text.split_once(char::is_whitespace).unwrap_or((text, ""))
}

/// The name of the file for page `number` of a document of `count`
/// pages, where pages go to files of their own: `path` with the number
/// before its ending, in as many digits as `count` has.
#[must_use]
pub fn numbered(path: &Path, number: usize, count: usize) -> PathBuf {
    let width = count.to_string().len();
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let ending = path.extension().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!("{stem}-{number:0width$}.{ending}"))
}

/// What the file of a figure is called after, where the user gives it
/// no name: its label, as far as that can be part of the name of a
/// file, or which figure of the document it is.
fn figure_name(figure: &FigureArea) -> String {
    let plain = |c: char| {
        if c.is_alphanumeric() || matches!(c, '-' | '_') {
            c
        } else {
            '-'
        }
    };
    match &figure.label {
        Some(label) if !label.is_empty() => label.chars().map(plain).collect(),
        _ => format!("figure-{}", figure.ordinal),
    }
}

/// The bytes of line `line` of `text`, counting from 1, without what
/// ends it.
fn line_bytes(text: &str, line: usize) -> Option<Range<usize>> {
    let mut start = 0;
    for (index, with_end) in text.split_inclusive('\n').enumerate() {
        if index + 1 == line {
            return Some(start..start + with_end.trim_end_matches(['\n', '\r']).len());
        }
        start += with_end.len();
    }
    None
}

/// One file an export wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub path: PathBuf,
    /// How large it is.
    pub bytes: usize,
    /// Whether a file was there before and is no more.
    pub replaced: bool,
}

/// What became of an export that went well.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exported {
    /// The files, in the order of the pages.
    pub files: Vec<Written>,
    /// How many pages of the document are in them, and how many it has.
    pub pages: usize,
    pub of: usize,
    /// What the status line says of it.
    pub told: String,
}

/// How pages become files.
enum Work {
    /// One PDF of these pages (counting from 0), or of all.
    Pdf {
        pages: Option<Vec<usize>>,
        path: PathBuf,
    },
    /// An image for each of these pages, or of `area` of the only one:
    /// PNGs of so many pixels to the point, or SVGs.
    Images {
        pixel_per_pt: Option<f64>,
        area: Option<Rect>,
        files: Vec<(usize, PathBuf)>,
    },
}

/// An export that is ready to be done: the pages of one revision of the
/// document and what to write of them where. Doing it needs nothing of
/// the window, so it can be done on any thread.
pub struct Job {
    document: Arc<PagedDocument>,
    world: Arc<ProjectWorld>,
    work: Work,
    /// What the files are called in the status line, and what is in
    /// them.
    name: String,
    holds: String,
}

impl Job {
    /// What the files are called in the status line.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Makes the files and writes them, each of them whole or not at
    /// all. The error says what went wrong, and how many files were
    /// written before it did.
    ///
    /// What Typst's exporters hand to other threads (a PDF's images are
    /// encoded that way) goes to threads that are there for this export
    /// only. Left to themselves they would take the one thread that
    /// typesets the status line and the lists, and the window would
    /// wait for the export after all.
    pub fn run(&self) -> Result<Exported, String> {
        let mut files = Vec::new();
        let threads = rayon::ThreadPoolBuilder::new()
            .num_threads(EXPORT_THREADS)
            .thread_name(|index| format!("zetwerk-export-{index}"))
            .build();
        let done = match threads {
            Ok(threads) => threads.install(|| self.write(&mut files)),
            Err(_) => self.write(&mut files),
        };
        let written = files.len();
        done.map_err(|why| match written {
            0 => why,
            _ => format!("{why}; {written} file(s) were written before that"),
        })?;
        let replaced = files.iter().filter(|file| file.replaced).count();
        let replaced = match (replaced, files.len()) {
            (0, _) => String::new(),
            (1, 1) => "; it replaced the one that was there".to_owned(),
            (count, _) => format!("; {count} replaced files that were there"),
        };
        let pages = match &self.work {
            Work::Pdf { pages, .. } => pages.as_ref().map_or(self.document.pages().len(), Vec::len),
            Work::Images { files, .. } => files.len(),
        };
        Ok(Exported {
            files,
            pages,
            of: self.document.pages().len(),
            told: format!("{} written, {}{replaced}", self.name, self.holds),
        })
    }

    fn write(&self, written: &mut Vec<Written>) -> Result<(), String> {
        match &self.work {
            Work::Pdf { pages, path } => {
                let bytes =
                    zetwerk_typst::export_pdf_pages(&self.document, &*self.world, pages.as_deref())
                        .map_err(|errors| errors.join("; "))?;
                written.push(write(path, &bytes)?);
            }
            Work::Images {
                pixel_per_pt,
                area,
                files,
            } => {
                for (index, path) in files {
                    let page = self
                        .document
                        .pages()
                        .get(*index)
                        .ok_or_else(|| format!("page {} is gone", index + 1))?;
                    let bytes = match pixel_per_pt {
                        Some(scale) => zetwerk_render::export::png(page, *area, *scale),
                        None => zetwerk_render::export::svg(page, *area).map(String::into_bytes),
                    }
                    .map_err(|why| format!("page {}: {why}", index + 1))?;
                    written.push(write(path, &bytes)?);
                }
            }
        }
        Ok(())
    }
}

/// Puts `bytes` at `path`, whole or not at all.
fn write(path: &Path, bytes: &[u8]) -> Result<Written, String> {
    let replaced = path.exists();
    if let Err(err) = zetwerk_core::write_atomically(path, bytes) {
        // What could not be moved into place is not left lying beside
        // it.
        for unfinished in zetwerk_core::leftovers(path) {
            let _ = std::fs::remove_file(unfinished);
        }
        return Err(format!("cannot write {}: {err}", path.display()));
    }
    Ok(Written {
        path: path.to_owned(),
        bytes: bytes.len(),
        replaced,
    })
}

/// The thread that does exports, one after the other.
struct Worker {
    jobs: Option<Sender<Job>>,
    done: Receiver<Result<Exported, String>>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    /// Starts the thread. `wake` is called from it whenever an export is
    /// over and what became of it can be collected.
    fn spawn(wake: Box<dyn Fn() + Send>) -> std::io::Result<Self> {
        let (jobs, waiting) = mpsc::channel::<Job>();
        let (finished, done) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("zetwerk-export".into())
            .spawn(move || {
                for job in waiting {
                    // Whatever goes wrong with one export, the thread is
                    // there for the next.
                    let outcome = catch_unwind(AssertUnwindSafe(|| job.run()))
                        .unwrap_or_else(|_| Err("the export crashed".to_owned()));
                    if finished.send(outcome).is_err() {
                        break;
                    }
                    wake();
                }
            })?;
        Ok(Self {
            jobs: Some(jobs),
            done,
            thread: Some(thread),
        })
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Closing the channel ends the thread's loop after the export in
        // progress; wait for it, so that leaving does not cut short a
        // file the user asked for.
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The exports of one window: the one that waits for its pages or is
/// being written, and the thread that writes.
#[derive(Default)]
pub(super) struct Exports {
    /// How to tell the window that an export is over, until the thread
    /// that does them has it. With neither, exports are done on the
    /// spot.
    wake: Option<Box<dyn Fn() + Send>>,
    worker: Option<Worker>,
    /// What the export being written is called.
    running: Option<String>,
    /// The export that waits for the text to be compiled.
    waiting: Option<Wanted>,
}

impl Viewer {
    /// Has exports done on a thread of their own from now on. `wake` is
    /// called from that thread when one is over; whoever it wakes calls
    /// [`Viewer::export_progress`], which is also what starts an export
    /// that waits for its pages.
    pub fn export_in_background(&mut self, wake: impl Fn() + Send + 'static) {
        self.exports.wake = Some(Box::new(wake));
    }

    /// Whether an export waits for its pages or is being written.
    #[must_use]
    pub fn is_exporting(&self) -> bool {
        self.exports.running.is_some() || self.exports.waiting.is_some()
    }

    /// `:pdf`, `:png`, `:svg` and `:figure`, with what was typed behind
    /// them.
    pub(super) fn export_command(&mut self, name: &str, argument: &str) {
        match Wanted::parse(name, argument) {
            Ok(wanted) => self.begin_export(wanted),
            Err(why) => self.alert(why),
        }
    }

    /// Starts the export `wanted`, or says why not.
    fn begin_export(&mut self, wanted: Wanted) {
        if let Some(name) = &self.exports.running {
            let told = format!("{name} is still being written: one export at a time");
            return self.alert(told);
        }
        if self.exports.waiting.is_some() {
            return self.alert("an export is waiting for the pages already: one at a time");
        }
        let background = self.exports.wake.is_some() || self.exports.worker.is_some();
        if background && !self.is_compiled() {
            self.exports.waiting = Some(wanted);
            return self.notice("exporting as soon as the pages are those of the text …");
        }
        let job = match self.plan_export(&wanted) {
            Ok(job) => job,
            Err(why) => return self.alert(format!("not exported: {why}")),
        };
        if !background {
            return self.exported(job.run());
        }
        if self.exports.worker.is_none()
            && let Some(wake) = self.exports.wake.take()
        {
            match Worker::spawn(wake) {
                Ok(worker) => self.exports.worker = Some(worker),
                Err(err) => return self.alert(format!("not exported: no thread for it: {err}")),
            }
        }
        let name = job.name().to_owned();
        let jobs = self
            .exports
            .worker
            .as_ref()
            .and_then(|worker| worker.jobs.as_ref());
        if jobs.is_some_and(|jobs| jobs.send(job).is_ok()) {
            self.notice(format!("writing {name} …"));
            self.exports.running = Some(name);
        } else {
            self.exports.worker = None;
            self.alert("not exported: the thread that writes exports is gone; ask again");
        }
    }

    /// Says what became of an export.
    fn exported(&mut self, outcome: Result<Exported, String>) {
        match outcome {
            Ok(outcome) => self.notice(outcome.told),
            Err(why) => self.alert(format!("not exported: {why}")),
        }
    }

    /// Takes up what the thread that writes exports has to say, and
    /// starts the export that waited for the text to be compiled if it
    /// is. Costs nothing where no export is under way; the window calls
    /// it whenever it was woken.
    pub fn export_progress(&mut self) {
        if self.exports.running.is_some() {
            let said = self
                .exports
                .worker
                .as_ref()
                .map(|worker| worker.done.try_recv());
            match said {
                Some(Err(TryRecvError::Empty)) => {}
                Some(Ok(outcome)) => {
                    self.exports.running = None;
                    self.exported(outcome);
                }
                Some(Err(TryRecvError::Disconnected)) | None => {
                    self.exports.running = None;
                    self.exports.worker = None;
                    self.alert("not exported: the thread that writes exports is gone");
                }
            }
        }
        if self.exports.waiting.is_some()
            && self.is_compiled()
            && let Some(wanted) = self.exports.waiting.take()
        {
            self.begin_export(wanted);
        }
    }

    /// The export `wanted`, ready to be done: the pages as they are, if
    /// they are those of the text as it is, and where to write what.
    /// Nothing is written yet. The error says why it cannot be done.
    pub fn plan_export(&self, wanted: &Wanted) -> Result<Job, String> {
        let export = self.for_export()?;
        let count = export.document.pages().len();
        let figure = match &wanted.what {
            What::Pages(_) => None,
            what => Some(self.figure_wanted(&export, what)?),
        };
        let chosen = match &wanted.what {
            What::Pages(Some(pages)) => pages.resolve(count)?,
            _ if count == 0 => return Err("the document has no pages".to_owned()),
            _ => (0..count).collect(),
        };
        let pixel_per_pt = pixels_per_point(wanted, figure.is_some())?;
        let path = self.export_path(wanted, &export, figure.as_ref())?;
        let all = chosen.len() == count;
        let some = format!("{} of {count} pages", chosen.len());
        let (work, holds) = match (wanted.format, &figure) {
            (Format::Pdf, Some(_)) => return Err("a figure is written as png or svg".to_owned()),
            (Format::Pdf, None) => {
                let holds = if all {
                    format!("{count} page(s)")
                } else {
                    some
                };
                let pages = (!all).then_some(chosen);
                (Work::Pdf { pages, path }, holds)
            }
            (_, Some(figure)) => {
                let page = &export.document.pages()[figure.page];
                let area = Some(with_margin(figure.area, page.frame.size()));
                let files = vec![(figure.page, path)];
                let holds = format!("the figure on page {}", figure.page + 1);
                (
                    Work::Images {
                        pixel_per_pt,
                        area,
                        files,
                    },
                    holds,
                )
            }
            (_, None) => {
                let files = page_files(&path, &chosen, count, wanted.file.is_some());
                let holds = match (all, files.as_slice()) {
                    (true, _) => format!("{count} page(s)"),
                    (false, [(index, _)]) => format!("page {} of {count}", index + 1),
                    (false, _) => some,
                };
                let area = None;
                (
                    Work::Images {
                        pixel_per_pt,
                        area,
                        files,
                    },
                    holds,
                )
            }
        };
        let name = self.export_name(&work)?;
        Ok(Job {
            document: export.document,
            world: export.world,
            work,
            name,
            holds,
        })
    }

    /// The figure an export is to be of.
    fn figure_wanted(&self, export: &Export, what: &What) -> Result<FigureArea, String> {
        let editor = self.editor.as_ref().ok_or("no text is being edited")?;
        let text = editor.text();
        let (bytes, nowhere) = if let What::FigureAtLine(line) = what {
            let bytes = line_bytes(text, *line).ok_or_else(|| {
                format!(
                    "line {line} does not exist; the file has {} line(s)",
                    text.lines().count()
                )
            })?;
            (bytes, format!("line {line} is in no figure"))
        } else {
            let cursor = editor.cursor();
            let nowhere = "the cursor is in no figure: put it on the caption of one";
            (cursor..cursor, nowhere.to_owned())
        };
        zetwerk_typst::figure_area(&export.document, &export.world, editor.file(), &bytes)
            .ok_or(nowhere)
    }

    /// Where the export `wanted` goes: the file the user named, or the
    /// usual one beside the main file. Of pages that go to files of
    /// their own, the name the numbers are put into.
    fn export_path(
        &self,
        wanted: &Wanted,
        export: &Export,
        figure: Option<&FigureArea>,
    ) -> Result<PathBuf, String> {
        let ending = wanted.format.ending();
        let path = match (&wanted.file, &export.main) {
            (Some(name), _) => self.project.root_dir().join(name),
            (None, Some(main)) => match figure {
                Some(figure) => {
                    let stem = main.file_stem().unwrap_or_default().to_string_lossy();
                    main.with_file_name(format!("{stem}-{}.{ending}", figure_name(figure)))
                }
                None => main.with_extension(ending),
            },
            (None, None) => return Err("the document is not a file: say what to call it".into()),
        };
        if path.extension().is_none_or(|found| found != ending) {
            return Err(format!("{} does not end in .{ending}", path.display()));
        }
        Ok(path)
    }

    /// What the files of `work` are called in the status line: their
    /// names from the document's directory. An error if one of them is
    /// a file the document is made from.
    fn export_name(&self, work: &Work) -> Result<String, String> {
        let paths: Vec<&PathBuf> = match work {
            Work::Pdf { path, .. } => vec![path],
            Work::Images { files, .. } => files.iter().map(|(_, path)| path).collect(),
        };
        let root = self.project.root_dir();
        let name = |path: &Path| {
            let shown = path.strip_prefix(root).unwrap_or(path);
            shown.to_string_lossy().into_owned()
        };
        let made_of = self.project.dependencies();
        let same = |a: &Path, b: &Path| {
            a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
        };
        for path in &paths {
            if made_of.iter().any(|part| same(part, path)) {
                return Err(format!(
                    "{} is a file the document is made from; another name, please",
                    name(path)
                ));
            }
        }
        match paths.as_slice() {
            [] => Err("there is nothing to write".to_owned()),
            [only] => Ok(name(only)),
            [first, .., last] => Ok(format!("{} … {}", name(first), name(last))),
        }
    }
}

/// How many pixels to the point the PNG `wanted` is to have; `None` for
/// what is not a PNG, which has none to choose.
fn pixels_per_point(wanted: &Wanted, figure: bool) -> Result<Option<f64>, String> {
    match (wanted.format, wanted.dpi) {
        (Format::Png, dpi) => {
            let dpi = dpi.unwrap_or(if figure { FIGURE_DPI } else { PAGE_DPI });
            if !DPI_RANGE.contains(&dpi) {
                let (least, most) = (DPI_RANGE.start(), DPI_RANGE.end());
                return Err(format!("between {least} and {most} dpi, please"));
            }
            Ok(Some(dpi / 72.0))
        }
        (_, Some(_)) => Err("only a PNG has pixels to the inch".to_owned()),
        (_, None) => Ok(None),
    }
}

/// The files the pages `chosen` (counting from 0) of a document of
/// `count` pages go to as images, where `path` is the name asked for.
/// A name the user gave (`named`) is the name of the file if there is
/// one file; the usual name is numbered whenever the document has pages
/// to tell apart.
fn page_files(path: &Path, chosen: &[usize], count: usize, named: bool) -> Vec<(usize, PathBuf)> {
    let apart = if named { chosen.len() > 1 } else { count > 1 };
    let file = |index: &usize| {
        if apart {
            (*index, numbered(path, index + 1, count))
        } else {
            (*index, path.to_owned())
        }
    };
    chosen.iter().map(file).collect()
}

/// `area` with [`FIGURE_MARGIN`] more on every side, as far as a page
/// of `size` goes.
fn with_margin(area: Rect, size: typst::layout::Size) -> Rect {
    let margin = Point::splat(Abs::pt(FIGURE_MARGIN));
    let min = (area.min - margin).max(Point::zero());
    let max = (area.max + margin).min(size.to_point());
    // A figure that is drawn off the page altogether keeps its place.
    if min.x < max.x && min.y < max.y {
        Rect::new(min, max)
    } else {
        area
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use tiny_skia::Pixmap;
    use zetwerk_typst::{CompileOutput, Environment, FontSet, Project};

    use super::*;
    use crate::editor::Editor;
    use crate::keys;

    /// Three small pages, the second of which is a figure.
    const TEXT: &str = "#set page(width: 200pt, height: 120pt, margin: 10pt)\n\
                        First page.\n\
                        #pagebreak()\n\
                        #figure(\n  rect(width: 80pt, height: 40pt),\n  caption: [A box],\n) <fig:box>\n\
                        #pagebreak()\n\
                        Third page.\n";

    const PICTURE: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\">\
                           <rect width=\"10\" height=\"10\"/></svg>";

    /// A viewer editing `main.typ` with `text` in a fresh directory that
    /// also holds `files`, compiled; and the directory.
    fn editing(text: &str, files: &[(&str, &str)]) -> (Viewer, PathBuf) {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "zetwerk-export-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("main.typ");
        std::fs::write(&path, text).unwrap();
        for (name, content) in files {
            std::fs::write(directory.join(name), content).unwrap();
        }
        let env = Environment::new(FontSet::Embedded);
        let project = Project::open(Arc::clone(&env), &path).unwrap();
        let (editor, opened) =
            Editor::open(&path, &directory.join("state"), project.main()).unwrap();
        let mut viewer = Viewer::new(env, project, (1000, 620), 1.0)
            .unwrap()
            .editing(editor, &opened);
        viewer.state_dir = Some(directory.join("state"));
        compile(&mut viewer);
        (viewer, directory)
    }

    /// Compiles the viewer's current revision on this thread.
    fn compile(viewer: &mut Viewer) {
        let (revision, world) = viewer.snapshot();
        let compiled = zetwerk_typst::compile(&world);
        viewer.compiled(CompileOutput {
            revision,
            world: Arc::new(world),
            compiled,
            duration: Duration::from_millis(3),
        });
    }

    fn press(viewer: &mut Viewer, notation: &str) {
        for key in keys::parse(notation).unwrap() {
            viewer.key(key);
        }
    }

    /// What the status line says, and whether it says it as a problem.
    fn said(viewer: &Viewer) -> (&str, bool) {
        let message = viewer.message.as_ref();
        message.map_or(("", false), |message| {
            (message.text.as_str(), message.alert)
        })
    }

    /// The names of the files in `directory` that end in `ending`.
    fn files(directory: &Path, ending: &str) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(ending))
            .collect();
        names.sort();
        names
    }

    fn size_of(png: &Path) -> (u32, u32) {
        let pixmap = Pixmap::decode_png(&std::fs::read(png).unwrap()).unwrap();
        (pixmap.width(), pixmap.height())
    }

    #[test]
    fn pages_are_read_as_pages_and_names_as_names() {
        let pages = |text: &str, count: usize| Pages::parse(text).map(|pages| pages.resolve(count));
        assert_eq!(pages("5", 9), Some(Ok(vec![4])));
        assert_eq!(pages("3-7", 9), Some(Ok(vec![2, 3, 4, 5, 6])));
        assert_eq!(pages("8-", 9), Some(Ok(vec![7, 8])));
        assert_eq!(pages("-2", 9), Some(Ok(vec![0, 1])));
        assert_eq!(pages("4-6,1,5", 9), Some(Ok(vec![0, 3, 4, 5])));
        assert_eq!(pages("3-3", 3), Some(Ok(vec![2])));
        let missing = pages("2-12", 9).unwrap().unwrap_err();
        assert_eq!(
            missing,
            "page 12 does not exist; the document has 9 page(s)"
        );
        assert!(pages("10", 9).unwrap().is_err());
        assert!(pages("10-", 9).unwrap().is_err());
        for no_pages in [
            "", "0", "7-3", "-", "1,,2", "1-2-3", "2024.pdf", "out.png", "a-b", "+3", "3 ", "1,",
        ] {
            assert_eq!(Pages::parse(no_pages), None, "{no_pages:?}");
        }
    }

    #[test]
    fn commands_say_what_to_write_and_where() {
        let parsed = |name: &str, argument: &str| Wanted::parse(name, argument).unwrap();
        let pages = |text: &str| What::Pages(Pages::parse(text));
        let whole = parsed("pdf", "");
        assert_eq!(
            (whole.format, &whole.what, &whole.file),
            (Format::Pdf, &What::Pages(None), &None)
        );
        let some = parsed("pdf", "3-7 out.pdf");
        assert_eq!(
            (some.what, some.file.as_deref()),
            (pages("3-7"), Some("out.pdf"))
        );
        let one = parsed("pdf", "5");
        assert_eq!((one.what, one.file), (pages("5"), None));
        let named = parsed("pdf", "2024.pdf");
        assert_eq!(
            (named.what, named.file.as_deref()),
            (What::Pages(None), Some("2024.pdf"))
        );

        let image = parsed("png", "2 300dpi my page.png");
        assert_eq!(image.format, Format::Png);
        assert_eq!((image.what, image.dpi), (pages("2"), Some(300.0)));
        assert_eq!(image.file.as_deref(), Some("my page.png"));
        let turned = parsed("png", "  96dpi   1,3 ");
        assert_eq!(
            (turned.what, turned.dpi, turned.file),
            (pages("1,3"), Some(96.0), None)
        );
        // Only a PNG has a resolution: elsewhere the word is a name.
        assert_eq!(parsed("svg", "300dpi").file.as_deref(), Some("300dpi"));

        let figure = parsed("figure", "svg plot.svg");
        assert_eq!(
            (figure.format, &figure.what),
            (Format::Svg, &What::FigureAtCursor)
        );
        assert_eq!(figure.file.as_deref(), Some("plot.svg"));
        let sharp = parsed("figure", "png 600dpi");
        assert_eq!(
            (sharp.format, sharp.dpi, sharp.file),
            (Format::Png, Some(600.0), None)
        );
        // A figure has no pages to choose: the word is a name.
        assert_eq!(parsed("figure", "png 3").file.as_deref(), Some("3"));
        assert!(Wanted::parse("figure", "").is_err());
        assert!(Wanted::parse("figure", "pdf").is_err());
    }

    #[test]
    fn pages_that_go_to_files_of_their_own_are_numbered() {
        let path = Path::new("/doc/talk.png");
        assert_eq!(numbered(path, 3, 9), Path::new("/doc/talk-3.png"));
        assert_eq!(numbered(path, 3, 12), Path::new("/doc/talk-03.png"));
        assert_eq!(numbered(path, 120, 120), Path::new("/doc/talk-120.png"));
        let names = |chosen: &[usize], count: usize, named: bool| -> Vec<String> {
            page_files(path, chosen, count, named)
                .iter()
                .map(|(_, path)| path.file_name().unwrap().to_string_lossy().into_owned())
                .collect()
        };
        // The usual name is numbered as soon as the document has pages
        // to tell apart; a name that was given, when it is for several.
        assert_eq!(names(&[0], 1, false), ["talk.png"]);
        assert_eq!(names(&[4], 12, false), ["talk-05.png"]);
        assert_eq!(names(&[4], 12, true), ["talk.png"]);
        assert_eq!(names(&[0, 9], 12, true), ["talk-01.png", "talk-10.png"]);
        assert_eq!(line_bytes("one\ntwo\r\nthree", 2), Some(4..7));
        assert_eq!(line_bytes("one\ntwo\r\nthree", 3), Some(9..14));
        assert_eq!(line_bytes("one\n", 2), None);
    }

    #[test]
    fn a_pdf_is_written_of_all_pages_or_of_some() {
        let (mut viewer, directory) = editing(TEXT, &[]);
        press(&mut viewer, ":pdf<CR>");
        assert_eq!(said(&viewer), ("main.pdf written, 3 page(s)", false));
        let whole = std::fs::read(directory.join("main.pdf")).unwrap();
        assert!(whole.starts_with(b"%PDF"));

        press(&mut viewer, ":pdf 2-3 part.pdf<CR>");
        assert_eq!(said(&viewer), ("part.pdf written, 2 of 3 pages", false));
        let part = std::fs::read(directory.join("part.pdf")).unwrap();
        assert!(part.starts_with(b"%PDF") && part.len() < whole.len());
        press(&mut viewer, ":pdf 3<CR>");
        let told = "main.pdf written, 1 of 3 pages; it replaced the one that was there";
        assert_eq!(said(&viewer), (told, false));
        let last = std::fs::read(directory.join("main.pdf")).unwrap();
        assert!(last.starts_with(b"%PDF") && last.len() < part.len());

        // What cannot be done is said, and nothing is written.
        press(&mut viewer, ":pdf 4 none.pdf<CR>");
        let refused = "not exported: page 4 does not exist; the document has 3 page(s)";
        assert_eq!(said(&viewer), (refused, true));
        press(&mut viewer, ":pdf notes.txt<CR>");
        assert!(said(&viewer).0.ends_with("notes.txt does not end in .pdf"));
        assert_eq!(files(&directory, ".pdf"), ["main.pdf", "part.pdf"]);
        assert_eq!(files(&directory, ".txt"), [""; 0]);
        assert!(!viewer.is_exporting());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn pages_are_written_as_images() {
        let (mut viewer, directory) = editing(TEXT, &[]);
        press(&mut viewer, ":png<CR>");
        assert_eq!(
            said(&viewer),
            ("main-1.png … main-3.png written, 3 page(s)", false)
        );
        assert_eq!(
            files(&directory, ".png"),
            ["main-1.png", "main-2.png", "main-3.png"]
        );
        // Two pixels to the point, unless asked otherwise.
        assert_eq!(size_of(&directory.join("main-2.png")), (400, 240));
        press(&mut viewer, ":png 2 72dpi two.png<CR>");
        assert_eq!(said(&viewer), ("two.png written, page 2 of 3", false));
        assert_eq!(size_of(&directory.join("two.png")), (200, 120));
        press(&mut viewer, ":png 3<CR>");
        let told = "main-3.png written, page 3 of 3; it replaced the one that was there";
        assert_eq!(said(&viewer), (told, false));
        press(&mut viewer, ":png 5dpi<CR>");
        assert_eq!(
            said(&viewer),
            ("not exported: between 20 and 1200 dpi, please", true)
        );

        press(&mut viewer, ":svg 1,3 slides/out.svg<CR>");
        assert!(
            said(&viewer).0.contains("cannot write"),
            "{:?}",
            said(&viewer)
        );
        press(&mut viewer, ":svg 1,3 out.svg<CR>");
        assert_eq!(
            said(&viewer),
            ("out-1.svg … out-3.svg written, 2 of 3 pages", false)
        );
        press(&mut viewer, ":svg 2<CR>");
        assert_eq!(
            files(&directory, ".svg"),
            ["main-2.svg", "out-1.svg", "out-3.svg"]
        );
        let svg = std::fs::read_to_string(directory.join("main-2.svg")).unwrap();
        assert!(svg.starts_with("<svg") && svg.contains("viewBox=\"0 0 200 120\""));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_figure_the_cursor_is_in_is_written_alone() {
        let (mut viewer, directory) = editing(TEXT, &[]);
        // On the first page the cursor is in no figure.
        press(&mut viewer, ":figure png<CR>");
        assert!(said(&viewer).1, "{:?}", said(&viewer));
        assert!(said(&viewer).0.contains("the cursor is in no figure"));
        assert_eq!(files(&directory, ".png"), [""; 0]);

        let caption = TEXT.find("A box").unwrap();
        viewer.editor.as_mut().unwrap().jump(caption);
        press(&mut viewer, ":figure png<CR>");
        let told = "main-fig-box.png written, the figure on page 2";
        assert_eq!(said(&viewer), (told, false));
        // The box of 80 by 40 points, its caption below it and four
        // points around them, at 300 pixels to the inch: not the page,
        // which would be 833 by 500.
        let (width, height) = size_of(&directory.join("main-fig-box.png"));
        assert!((360..460).contains(&width), "{width} wide");
        assert!((230..330).contains(&height), "{height} high");

        press(&mut viewer, ":figure svg 72dpi<CR>");
        // Only a PNG has a resolution: here the word is a name.
        assert!(said(&viewer).0.ends_with("72dpi does not end in .svg"));
        press(&mut viewer, ":figure svg box.svg<CR>");
        assert_eq!(
            said(&viewer),
            ("box.svg written, the figure on page 2", false)
        );
        let svg = std::fs::read_to_string(directory.join("box.svg")).unwrap();
        let whole = "viewBox=\"0 0 200 120\"";
        assert!(
            svg.starts_with("<svg") && !svg.contains(whole),
            "{}",
            &svg[..200]
        );

        // And for an agent, the figure a line is part of.
        let mut wanted = Wanted::parse("figure", "png line.png").unwrap();
        wanted.what = What::FigureAtLine(5);
        let outcome = viewer.plan_export(&wanted).unwrap().run().unwrap();
        assert_eq!((outcome.pages, outcome.of, outcome.files.len()), (1, 3, 1));
        assert_eq!(size_of(&directory.join("line.png")), (width, height));
        wanted.what = What::FigureAtLine(2);
        let nowhere = viewer.plan_export(&wanted).err().unwrap();
        assert_eq!(nowhere, "line 2 is in no figure");
        wanted.what = What::FigureAtLine(40);
        assert!(viewer.plan_export(&wanted).is_err());
        let _ = std::fs::remove_dir_all(directory);
    }

    /// A viewer that exports on a thread of its own, and what that
    /// thread wakes.
    fn in_background(text: &str) -> (Viewer, PathBuf, mpsc::Receiver<()>) {
        let (mut viewer, directory) = editing(text, &[]);
        let (wake, woken) = mpsc::channel();
        viewer.export_in_background(move || {
            let _ = wake.send(());
        });
        (viewer, directory, woken)
    }

    const PATIENCE: Duration = Duration::from_secs(60);

    #[test]
    fn an_export_is_written_on_a_thread_of_its_own_from_one_revision() {
        let (mut viewer, directory, woken) = in_background(TEXT);
        let before = Arc::clone(viewer.document().unwrap());
        press(&mut viewer, ":svg 1<CR>");
        assert_eq!(said(&viewer), ("writing main-1.svg …", false));
        assert!(viewer.is_exporting());
        // Another is refused while it is under way, whatever it is.
        press(&mut viewer, ":pdf<CR>");
        let refused = "main-1.svg is still being written: one export at a time";
        assert_eq!(said(&viewer), (refused, true));

        // Typing goes on meanwhile and does not reach what is written.
        let start = TEXT.find("First").unwrap();
        viewer.editor.as_mut().unwrap().jump(start);
        press(&mut viewer, "cwOther<Esc>");
        compile(&mut viewer);
        assert!(viewer.text().unwrap().contains("Other page."));
        let after = Arc::clone(viewer.document().unwrap());

        woken.recv_timeout(PATIENCE).unwrap();
        // Nothing is said until the window looks.
        assert!(viewer.is_exporting());
        viewer.export_progress();
        assert_eq!(said(&viewer), ("main-1.svg written, page 1 of 3", false));
        assert!(!viewer.is_exporting());
        let written = std::fs::read_to_string(directory.join("main-1.svg")).unwrap();
        let of = |document: &PagedDocument| {
            zetwerk_render::export::svg(&document.pages()[0], None).unwrap()
        };
        assert_eq!(written, of(&before));
        assert_ne!(written, of(&after));
        assert!(woken.try_recv().is_err(), "woken once for one export");

        // The thread is there for the next one, which fails and says so.
        press(&mut viewer, ":pdf nowhere/out.pdf<CR>");
        assert_eq!(said(&viewer), ("writing nowhere/out.pdf …", false));
        woken.recv_timeout(PATIENCE).unwrap();
        viewer.export_progress();
        assert!(said(&viewer).1 && said(&viewer).0.starts_with("not exported: cannot write"));
        press(&mut viewer, ":pdf<CR>");
        woken.recv_timeout(PATIENCE).unwrap();
        viewer.export_progress();
        assert_eq!(said(&viewer), ("main.pdf written, 3 page(s)", false));
        // Looking again when nothing is under way changes nothing.
        viewer.export_progress();
        assert_eq!(said(&viewer), ("main.pdf written, 3 page(s)", false));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn an_export_waits_for_the_pages_of_the_text_as_it_is() {
        let (mut viewer, directory, woken) = in_background(TEXT);
        let start = TEXT.find("First").unwrap();
        viewer.editor.as_mut().unwrap().jump(start);
        press(&mut viewer, "cwOther<Esc>");
        assert!(!viewer.is_compiled());
        press(&mut viewer, ":png 1 one.png<CR>");
        let waits = "exporting as soon as the pages are those of the text …";
        assert_eq!(said(&viewer), (waits, false));
        press(&mut viewer, ":pdf<CR>");
        assert!(said(&viewer).1 && said(&viewer).0.contains("one at a time"));
        // Looking does not start it before the pages are there.
        viewer.export_progress();
        assert!(viewer.is_exporting() && files(&directory, ".png").is_empty());
        assert!(woken.try_recv().is_err());

        compile(&mut viewer);
        viewer.export_progress();
        assert_eq!(said(&viewer), ("writing one.png …", false));
        woken.recv_timeout(PATIENCE).unwrap();
        viewer.export_progress();
        assert_eq!(said(&viewer), ("one.png written, page 1 of 3", false));
        let written = std::fs::read(directory.join("one.png")).unwrap();
        let page = &viewer.document().unwrap().pages()[0];
        assert_eq!(
            written,
            zetwerk_render::export::png(page, None, 2.0).unwrap()
        );

        // Text that does not compile has no pages to wait for.
        press(&mut viewer, "A#nope(<Esc>:pdf<CR>");
        compile(&mut viewer);
        viewer.export_progress();
        let (told, alert) = said(&viewer);
        assert!(alert && told.starts_with("not exported: the text does not compile"));
        assert!(!viewer.is_exporting() && files(&directory, ".pdf").is_empty());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn files_are_whole_or_not_there_and_the_documents_own_are_kept() {
        let text = format!("{TEXT}#image(\"plot.svg\")\n");
        let (mut viewer, directory) = editing(&text, &[("plot.svg", PICTURE)]);
        // A file the document shows is not written over, under whatever
        // name it comes to that.
        for command in [":svg 1 plot.svg<CR>", ":svg 1 ./plot.svg<CR>"] {
            press(&mut viewer, command);
            let (told, alert) = said(&viewer);
            assert!(
                alert && told.contains("a file the document is made from"),
                "{told}"
            );
        }
        assert_eq!(
            std::fs::read_to_string(directory.join("plot.svg")).unwrap(),
            PICTURE
        );

        // A file that was there is replaced whole, and that is said.
        std::fs::write(directory.join("old.png"), "not a picture").unwrap();
        press(&mut viewer, ":png 1 old.png<CR>");
        let told = "old.png written, page 1 of 3; it replaced the one that was there";
        assert_eq!(said(&viewer), (told, false));
        assert_eq!(size_of(&directory.join("old.png")), (400, 240));

        // Where the last of several cannot be written, the others are
        // whole and the count is said.
        std::fs::create_dir(directory.join("out-3.svg")).unwrap();
        press(&mut viewer, ":svg out.svg<CR>");
        let (told, alert) = said(&viewer);
        assert!(alert && told.contains("cannot write"), "{told}");
        assert!(
            told.ends_with("2 file(s) were written before that"),
            "{told}"
        );
        assert!(
            std::fs::read_to_string(directory.join("out-2.svg"))
                .unwrap()
                .ends_with("</svg>")
        );

        // Nothing else is left behind: no file that was written through.
        let mut left: Vec<String> = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        let expected = [
            "main.typ",
            "old.png",
            "out-1.svg",
            "out-2.svg",
            "out-3.svg",
            "plot.svg",
            "state",
        ];
        assert_eq!(left, expected);
        let _ = std::fs::remove_dir_all(directory);
    }
}
