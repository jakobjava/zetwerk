//! Pasting what another program copied: text, an image, files, cells.
//!
//! The clipboard is read only when a key asks for it, and never on the
//! window's thread: the key that needs it (and those typed after it)
//! waits while it is read aside, bounded, and goes on when it is (see
//! [`Paste`]). Escape gives the wait up. In a test, and where nothing
//! wakes the window, it is read at once.
//!
//! What asks for it:
//!
//! - `"+p`, `"+P` (and `"*` for the primary selection), and Ctrl-R + or
//!   Ctrl-R * while typing, as in Vim. `"+y` and the like do not read it.
//! - `p` and `P` on the page, where another program copied something
//!   after the last thing zetwerk yanked or deleted (the window system
//!   tells of a copy as it happens, without anything being read), or
//!   zetwerk has nothing to put: what was copied last, wherever, is put.
//!   Where the window system does not tell (no native clipboard), `p`
//!   puts what zetwerk took, as Vim does.
//! - `:paste` (what the clipboard holds, as what it is), `:paste image`,
//!   `:paste table`, `:paste text`; `:paste primary` for the selection.
//!
//! What is put on the page is what was copied as what it is:
//!
//! - **An image** (a screenshot, a figure from a browser or a drawing
//!   program) is written into a file beside the document (see
//!   `image_file`: where its images are, named after the heading or
//!   caption above, never over another file) and a figure of it is put
//!   below the paragraph of the cursor (`P`: above), its caption empty
//!   and being typed. A screenshot larger than any page needs is offered
//!   smaller first. `u` takes the figure out and says the file stays.
//! - **Files** copied in a file manager: each image among them, as above;
//!   one beside the document already is named where it is, one from
//!   elsewhere is copied in. A PDF is for the pages of PDFs, not here.
//! - **Cells** (tab-separated, as every spreadsheet and browser copies
//!   them; comma-separated rows): a table (see `table_text::new_table`),
//!   or, in a cell of a table, the cells from there on, the table made
//!   larger to hold them; asked first where that writes over cells that
//!   hold something.
//! - **Text**: as `p` puts any text, through one way in
//!   ([`Viewer::put_pasted_text`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use typst::syntax::{LinkedNode, Side, SyntaxKind};

use super::assist::Target;
use super::{Edited, Panes, Viewer, editor};
use crate::clipboard::{Copied, News, Selection};
use crate::image_file;
use crate::keys::{Key, KeyPress};
use crate::table_text;

/// How long keys wait for the clipboard before the wait is given up:
/// more than a program that copied takes to answer (each kind of data
/// is waited for two seconds at most).
const GIVE_UP: Duration = Duration::from_secs(5);

/// The endings of the image files that are pasted as figures.
const IMAGE_ENDINGS: &[&str] = &["png", "jpg", "jpeg", "gif", "svg", "webp"];

/// What a paste wants of what was copied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    /// What it is.
    Any,
    Image,
    Table,
    Text,
}

/// What the clipboard is read for.
#[derive(Debug, Clone, Copy)]
enum For {
    /// A register put of Vim's (`"+p` in the source, Ctrl-R + while
    /// typing): the register gets the text, and the keys go on.
    Register(char),
    /// `p` or `P` on the page (`before`): put as what it is; `plain`
    /// where no register was named, and text is put as `p` puts it.
    Page {
        selection: Selection,
        before: bool,
        plain: bool,
    },
    /// `:paste`.
    Command(Selection, Want),
}

/// A read of the clipboard under way, and the keys that wait for it.
pub(super) struct Reading {
    result: mpsc::Receiver<Result<Copied, String>>,
    purpose: For,
    held: Vec<KeyPress>,
    since: Instant,
}

/// A large image that waits for the user to say how it is put in.
struct Large {
    ending: &'static str,
    bytes: Vec<u8>,
    stem: Option<String>,
    before: bool,
}

/// What reads a selection for a paste.
type Reader = fn(Selection) -> Result<Copied, String>;

/// What wakes the window, from any thread.
type Wake = Arc<std::sync::Mutex<Box<dyn Fn() + Send>>>;

/// Cells that wait for a yes to write over cells: of the table that
/// begins at that byte, from that row and column on.
type Waiting = (usize, (usize, usize), Vec<Vec<String>>);

/// What pasting keeps between keys.
pub(super) struct Paste {
    /// What reads a selection: the system's; `None` in a test, where
    /// the text of `Clipboard::read` stands for the clipboard.
    pub(super) read: Option<Reader>,
    /// What tells of the clipboard without reading it.
    pub(super) news: fn() -> Option<News>,
    /// What writes the primary selection.
    pub(super) write_primary: fn(&str) -> Result<(), String>,
    /// What wakes the window when a read is over; none in a test.
    wake: Option<Wake>,
    pub(super) reading: Option<Reading>,
    /// The change of the clipboard that what zetwerk yanked or deleted
    /// since is newer than.
    seen: Option<u64>,
    /// The register `"+` or `"*` was named: a put may follow.
    armed: Option<char>,
    /// The last key was Ctrl-R while typing.
    ctrl_r: bool,
    /// The register was just filled from the clipboard: the key that
    /// comes again is not to read it again.
    fresh: bool,
    /// The image file last written, by its bytes.
    written: Option<(u64, PathBuf)>,
    /// The call of the image last pasted, how often the text has it
    /// since, and its file: for when it is taken out again.
    pasted: Option<(String, usize, PathBuf)>,
    large: Option<Large>,
    /// Cells that wait for a yes to write over cells: of the table
    /// beginning at that byte, from that row and column.
    cells: Option<Waiting>,
    /// What the primary selection was last given.
    pub(super) known_primary: Option<String>,
}

impl Default for Paste {
    fn default() -> Self {
        Self {
            read: (!cfg!(test)).then_some(crate::clipboard::copied as Reader),
            news: crate::clipboard::news,
            write_primary: |text| crate::clipboard::write_to(Selection::Primary, text),
            wake: None,
            reading: None,
            seen: None,
            armed: None,
            ctrl_r: false,
            fresh: false,
            written: None,
            pasted: None,
            large: None,
            cells: None,
            known_primary: None,
        }
    }
}

/// The selection register `name` stands for.
fn selection_of(name: char) -> Option<Selection> {
    match name {
        '+' => Some(Selection::Clipboard),
        '*' => Some(Selection::Primary),
        _ => None,
    }
}

/// A hash of `bytes`.
fn hash_of(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// Where a thing that stands by itself goes into `source` for the
/// cursor at `at`: below the paragraph of the cursor (or above it,
/// `before`), in what holds that paragraph, and never into a call or an
/// equation. The byte, and the blank lines to put before and after it.
fn block_place(
    source: &typst::syntax::Source,
    at: usize,
    before: bool,
) -> (usize, &'static str, &'static str) {
    let text = source.text();
    let root = LinkedNode::new(source.root());
    let leaf = root
        .leaf_at(at.min(text.len()), Side::After)
        .or_else(|| root.leaf_at(at.min(text.len()), Side::Before));
    let Some(leaf) = leaf else {
        return (
            text.len(),
            if text.trim().is_empty() { "" } else { "\n\n" },
            "",
        );
    };
    // The paragraph's part that the cursor is in, in the nearest markup
    // that holds paragraphs.
    let mut part = leaf;
    while let Some(parent) = part.parent().cloned() {
        let holds = parent.kind() == SyntaxKind::Markup
            && (parent.parent().is_none()
                || parent
                    .children()
                    .any(|child| child.kind() == SyntaxKind::Parbreak));
        if holds {
            break;
        }
        part = parent;
    }
    let Some(markup) = part.parent().cloned() else {
        return (text.len(), "\n\n", "");
    };
    let siblings: Vec<LinkedNode> = markup.children().collect();
    let index = siblings
        .iter()
        .position(|sibling| sibling.range() == part.range())
        .unwrap_or(0);
    let content_end = |nodes: &[LinkedNode]| {
        nodes
            .iter()
            .rev()
            .find(|node| !matches!(node.kind(), SyntaxKind::Space | SyntaxKind::Parbreak))
            .map(|node| node.range().end)
    };
    if before {
        let start = siblings[..=index]
            .iter()
            .rev()
            .find(|node| node.kind() == SyntaxKind::Parbreak && node.range().start < at)
            .map_or(markup.range().start, |parbreak| parbreak.range().end);
        let blank = if text[..start].trim().is_empty() || text[..start].ends_with('\n') {
            ""
        } else {
            "\n\n"
        };
        return (start, blank, "\n\n");
    }
    let end = siblings[index..]
        .iter()
        .find(|node| node.kind() == SyntaxKind::Parbreak)
        .map(|parbreak| parbreak.range().start)
        .or_else(|| content_end(&siblings))
        .unwrap_or(markup.range().end);
    let after = if text[..end].trim().is_empty() {
        ""
    } else {
        "\n\n"
    };
    (end, after, "")
}

impl Viewer {
    /// Has the clipboard read on a thread of its own from now on;
    /// `wake` is called from it when a read is over (whoever it wakes
    /// calls [`Viewer::paste_progress`]). Sets the window system's own
    /// clipboard up, so that it tells of copies as they happen.
    pub fn paste_in_background(&mut self, wake: impl Fn() + Send + 'static) {
        self.paste.wake = Some(Arc::new(std::sync::Mutex::new(Box::new(wake))));
        crate::clipboard::start();
    }

    /// Takes what a read of the clipboard brought, if one is over, and
    /// goes on with the keys that waited. Returns whether anything was
    /// done.
    pub fn paste_progress(&mut self) -> bool {
        let Some(reading) = &self.paste.reading else {
            return false;
        };
        let result = match reading.result.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => Err("the clipboard was not read".to_owned()),
        };
        let Some(reading) = self.paste.reading.take() else {
            return false;
        };
        self.message = None;
        self.pasted_now(result, reading.purpose, reading.held);
        true
    }

    /// A key for pasting: one that waits for the clipboard, or one that
    /// asks for it. Returns whether it was taken.
    pub(super) fn paste_key(&mut self, press: KeyPress) -> bool {
        if let Some(reading) = &mut self.paste.reading {
            // Escape right behind the key that waits gives the wait up
            // (one typed further ahead waits with the others); Ctrl-C
            // always does.
            let escape = press.key == Key::Escape || (press.ctrl && press.key == Key::Char('['));
            let stop =
                (escape && reading.held.len() == 1) || (press.ctrl && press.key == Key::Char('c'));
            if stop || reading.since.elapsed() > GIVE_UP {
                let reading = self.paste.reading.take();
                self.clear_pending();
                self.alert(if stop {
                    "the paste is given up"
                } else {
                    "the paste is given up: the clipboard did not answer"
                });
                // The keys after the one that asked go on.
                for key in reading.into_iter().flat_map(|reading| reading.held).skip(1) {
                    self.plain_key(key);
                }
                return true;
            }
            reading.held.push(press);
            return true;
        }
        let Some(editor) = &self.editor else {
            return false;
        };
        // A list, the overview of pages, a block of cells: their keys.
        if self.picker.is_some() || self.grid.is_some() || self.vim.cells.has_block() {
            return false;
        }
        if std::mem::take(&mut self.paste.fresh) {
            return false;
        }
        let mode = editor.mode();
        let typing = mode == editor::Mode::Insert;
        let ctrl_r = std::mem::replace(
            &mut self.paste.ctrl_r,
            typing && press.ctrl && press.key == Key::Char('r'),
        );
        let named = match press.key {
            Key::Char(name @ ('+' | '*')) if !press.ctrl && !press.alt => Some(name),
            _ => None,
        };
        // Ctrl-R + while typing: the register is filled first.
        if let (true, true, Some(name)) = (ctrl_r, typing, named) {
            self.read_clipboard(For::Register(name), vec![press]);
            return true;
        }
        // `"+`: a put may follow.
        if named.is_some()
            && editor
                .following()
                .is_some_and(|following| following.typed == "\"")
        {
            self.paste.armed = named;
            return false;
        }
        let puts = !press.ctrl && !press.alt && matches!(press.key, Key::Char('p' | 'P'));
        let before = press.key == Key::Char('P');
        let on_page = self.shown_panes() == Panes::Page && mode == editor::Mode::Normal;
        if let Some(name) = self.paste.armed {
            if matches!(press.key, Key::Char('0'..='9')) && !press.ctrl {
                return false;
            }
            self.paste.armed = None;
            if !puts {
                return false;
            }
            let purpose = match selection_of(name) {
                Some(selection) if on_page => For::Page {
                    selection,
                    before,
                    plain: false,
                },
                _ => For::Register(name),
            };
            self.read_clipboard(purpose, vec![press]);
            return true;
        }
        if !puts || !on_page || !editor.is_idle() {
            return false;
        }
        let nothing_taken = editor
            .engine()
            .register('"')
            .is_none_or(|(text, _)| text.is_empty());
        let news = (self.paste.news)();
        let newer = news.is_some_and(|news| !news.ours && Some(news.serial) != self.paste.seen);
        if !(newer || nothing_taken) {
            return false;
        }
        let purpose = For::Page {
            selection: Selection::Clipboard,
            before,
            plain: true,
        };
        self.read_clipboard(purpose, vec![press]);
        true
    }

    /// Reads the clipboard for `purpose`, aside where something wakes the
    /// window when it is done (`held` waits for it); else at once.
    fn read_clipboard(&mut self, purpose: For, held: Vec<KeyPress>) {
        let selection = match purpose {
            For::Register(name) => selection_of(name).unwrap_or(Selection::Clipboard),
            For::Page { selection, .. } | For::Command(selection, _) => selection,
        };
        // In a test, the text a test gives stands for the clipboard.
        let text = self.clipboard.read;
        let read = self.paste.read;
        let read = move || match read {
            Some(read) => read(selection),
            None if selection == Selection::Clipboard => text().map(|text| Copied::text(&text)),
            None => Err("there is no selection to read here".to_owned()),
        };
        let Some(wake) = self.paste.wake.clone() else {
            return self.pasted_now(read(), purpose, held);
        };
        let (done, result) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("zetwerk-paste".to_owned())
            .spawn(move || {
                let _ = done.send(read());
                if let Ok(wake) = wake.lock() {
                    wake();
                }
            });
        if let Err(err) = spawned {
            return self.pasted_now(
                Err(format!("the clipboard was not read: {err}")),
                purpose,
                held,
            );
        }
        self.paste.reading = Some(Reading {
            result,
            purpose,
            held,
            since: Instant::now(),
        });
        self.notice("reading the clipboard…  (Escape gives it up)");
    }

    /// Ends what the engine waits for (a register named): what it was
    /// for was done otherwise.
    fn clear_pending(&mut self) {
        let columns = self.source_size.0;
        if let Some(editor) = &mut self.editor
            && !editor.is_idle()
            && editor.mode() == editor::Mode::Normal
        {
            let _ = editor.key(
                KeyPress::plain(Key::Escape),
                &editor::Shown::Source { columns },
            );
        }
    }

    /// What was read for `purpose` is there: it is put where it goes,
    /// and the keys that waited go on.
    fn pasted_now(&mut self, result: Result<Copied, String>, purpose: For, held: Vec<KeyPress>) {
        let mut held = held.into_iter();
        let first = held.next();
        match purpose {
            For::Register(name) => {
                let text = match result {
                    Ok(Copied {
                        text: Some(text), ..
                    }) => text,
                    Ok(copied) => {
                        let what = if copied.image.is_some() || !copied.files.is_empty() {
                            "an image or files: on the page, p or :paste puts them in"
                        } else {
                            "no text"
                        };
                        self.alert(format!("the clipboard holds {what}"));
                        String::new()
                    }
                    Err(why) => {
                        self.alert(why);
                        String::new()
                    }
                };
                if let Some(first) = first {
                    self.put_pasted_text(&text, name, first);
                }
            }
            For::Page {
                selection,
                before,
                plain,
            } => match result {
                Ok(copied) => {
                    let serial = (self.paste.news)().map(|news| news.serial);
                    let name = if plain {
                        '"'
                    } else if selection == Selection::Primary {
                        '*'
                    } else {
                        '+'
                    };
                    let rest = self.paste_copied(copied, before, Want::Any);
                    match (rest, first) {
                        (Some(text), Some(first)) => {
                            if plain {
                                // As Vim's `p` puts it, and again.
                                self.paste.seen = serial;
                            }
                            self.put_pasted_text(&text, name, first);
                        }
                        _ => self.clear_pending(),
                    }
                }
                Err(why) => {
                    // `p` puts what zetwerk took, where it has some.
                    let taken = self.editor.as_ref().is_some_and(|editor| {
                        editor
                            .engine()
                            .register('"')
                            .is_some_and(|(text, _)| !text.is_empty())
                    });
                    if plain && taken {
                        if let Some(first) = first {
                            self.paste.fresh = true;
                            self.plain_key(first);
                        }
                    } else {
                        self.clear_pending();
                        self.alert(why);
                    }
                }
            },
            For::Command(_, want) => match result {
                Ok(copied) => {
                    if let Some(text) = self.paste_copied(copied, false, want) {
                        self.put_pasted_text(&text, '+', KeyPress::plain(Key::Char('p')));
                    }
                }
                Err(why) => self.alert(why),
            },
        }
        for key in held {
            self.plain_key(key);
        }
    }

    /// Text from the clipboard put in: the one way in for pasted text.
    /// Register `name` gets `text`, and `key` (the put that asked for it,
    /// or the register's name after Ctrl-R) is carried out with it.
    pub(super) fn put_pasted_text(&mut self, text: &str, name: char, key: KeyPress) {
        if let Some(editor) = &mut self.editor {
            editor.set_register(name, text);
            if name == '"' {
                editor.set_register('+', text);
            }
        }
        if name == '+' || name == '"' {
            self.clipboard_known(text);
        }
        if name == '*' {
            self.paste.known_primary = Some(text.to_owned());
        }
        self.paste.fresh = true;
        self.plain_key(key);
        self.paste.fresh = false;
    }

    /// Puts `copied` on the page as what it is (or as `want` says), the
    /// cursor's paragraph before it (or after it, `before`). Returns the
    /// text to put as text, where it is that.
    fn paste_copied(&mut self, copied: Copied, before: bool, want: Want) -> Option<String> {
        let images: Vec<&PathBuf> = copied
            .files
            .iter()
            .filter(|file| {
                file.extension()
                    .and_then(|ending| ending.to_str())
                    .is_some_and(|ending| IMAGE_ENDINGS.contains(&ending.to_lowercase().as_str()))
            })
            .collect();
        if matches!(want, Want::Any | Want::Image) && !images.is_empty() {
            let images: Vec<PathBuf> = images.into_iter().cloned().collect();
            self.paste_files(&images, before);
            return None;
        }
        if matches!(want, Want::Any | Want::Image) && !copied.files.is_empty() {
            let pdf = copied.files.iter().any(|file| {
                file.extension()
                    .is_some_and(|ending| ending.eq_ignore_ascii_case("pdf"))
            });
            self.alert(if pdf {
                "the clipboard holds a PDF: it is no image to paste as a figure"
            } else {
                "the clipboard holds files, and none of them is an image"
            });
            return None;
        }
        if matches!(want, Want::Any | Want::Image)
            && let Some((kind, bytes)) = copied.image.clone()
        {
            let ending = crate::clipboard::IMAGE_KINDS
                .iter()
                .find(|(known, _)| *known == kind)
                .map_or("png", |(_, ending)| *ending);
            self.paste_image(ending, bytes, None, before, None);
            return None;
        }
        if want == Want::Image {
            self.alert("the clipboard holds no image");
            return None;
        }
        let csv = copied.types.iter().any(|kind| kind == "text/csv");
        let cells = (want != Want::Text)
            .then(|| {
                let html = copied.html.as_deref().and_then(table_text::cells_in_html);
                html.or_else(|| {
                    let text = copied.text.as_deref()?;
                    table_text::cells_in(text, csv || want == Want::Table)
                })
            })
            .flatten();
        if let Some(cells) = cells {
            self.paste_cells(cells, before);
            return None;
        }
        if want == Want::Table {
            self.alert("the clipboard holds no cells: copy them from a spreadsheet or a table");
            return None;
        }
        let text = copied.text;
        if text.is_none() {
            self.alert("the clipboard holds nothing that can be put in here");
        }
        text
    }

    /// The image files `files` as figures: those beside the document
    /// where they are, others copied in.
    fn paste_files(&mut self, files: &[PathBuf], before: bool) {
        let Some(folder) = self.document_folder() else {
            return;
        };
        for file in files.iter().rev() {
            let ending = file
                .extension()
                .and_then(|ending| ending.to_str())
                .map(str::to_lowercase)
                .unwrap_or_default();
            let ending = IMAGE_ENDINGS
                .iter()
                .find(|known| **known == ending)
                .copied()
                .unwrap_or("png");
            let beside = file
                .strip_prefix(&folder)
                .ok()
                .map(|from| from.to_string_lossy().replace('\\', "/"));
            if let Some(beside) = beside {
                self.put_figure(
                    &beside,
                    file,
                    before,
                    &format!("{beside}: it is beside the document already"),
                );
                continue;
            }
            match std::fs::read(file) {
                Ok(bytes) => {
                    let stem = file
                        .file_stem()
                        .map(|stem| image_file::slug(&stem.to_string_lossy()))
                        .filter(|stem| !stem.is_empty());
                    self.paste_image(ending, bytes, stem, before, Some(file));
                }
                Err(err) => self.alert(format!("{} was not read: {err}", file.display())),
            }
        }
    }

    /// The folder of the file being edited.
    fn document_folder(&self) -> Option<PathBuf> {
        let editor = self.editor.as_ref()?;
        let folder = editor.path().parent()?;
        Some(if folder.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            folder.to_path_buf()
        })
    }

    /// An image of `bytes` (a file that ends in `ending`) written beside
    /// the document, called `stem` if that is given, and put in as a
    /// figure; a large one is offered smaller first.
    fn paste_image(
        &mut self,
        ending: &'static str,
        bytes: Vec<u8>,
        stem: Option<String>,
        before: bool,
        from: Option<&Path>,
    ) {
        let size = image_file::dimensions(&bytes);
        if let Some(size) = size.filter(|size| image_file::is_large(*size)) {
            let (wide, high) = size;
            let longest = image_file::SMALLER;
            let items = vec![
                (
                    format!(
                        "smaller: {longest} pixels on its longest side, as much as a page needs"
                    ),
                    Target::Command("paste image smaller".to_owned()),
                ),
                (
                    format!(
                        "as it is: {wide}×{high} pixels, {}",
                        image_file::size(bytes.len())
                    ),
                    Target::Command("paste image as is".to_owned()),
                ),
            ];
            self.paste.large = Some(Large {
                ending,
                bytes,
                stem,
                before,
            });
            self.open_picker("a large image", items, 0);
            return;
        }
        self.write_image(ending, &bytes, stem, before, from);
    }

    /// Writes the image of `bytes` beside the document and puts it in.
    fn write_image(
        &mut self,
        ending: &str,
        bytes: &[u8],
        stem: Option<String>,
        before: bool,
        from: Option<&Path>,
    ) {
        let (Some(folder), Some(editor)) = (self.document_folder(), &self.editor) else {
            return;
        };
        let (text, cursor) = (editor.text(), editor.cursor());
        let images = image_file::images_folder(text, &folder);
        let stem = stem
            .or_else(|| image_file::name_near(text, cursor))
            .unwrap_or_else(|| image_file::stamp(std::time::SystemTime::now()));
        let hash = hash_of(bytes);
        // The same image again: the file written for it is taken again.
        let again = self.paste.written.as_ref().filter(|(written, path)| {
            *written == hash
                && std::fs::metadata(path).is_ok_and(|meta| meta.len() == bytes.len() as u64)
        });
        let path = if let Some((_, path)) = again {
            path.clone()
        } else {
            let path = image_file::free_path(&folder.join(&images), &stem, ending);
            let written = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .and_then(|mut file| std::io::Write::write_all(&mut file, bytes));
            if let Err(err) = written {
                self.alert(format!(
                    "the image was not written: {}: {err}",
                    path.display()
                ));
                return;
            }
            self.paste.written = Some((hash, path.clone()));
            path
        };
        let name = path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let named = images.join(&name).to_string_lossy().replace('\\', "/");
        let size = image_file::dimensions(bytes)
            .map(|(wide, high)| format!("{wide}×{high} pixels, "))
            .unwrap_or_default();
        let whence = from.map_or_else(String::new, |from| format!(" from {}", from.display()));
        let told = format!(
            "{named} written{whence} ({size}{})",
            image_file::size(bytes.len())
        );
        self.file_changed(&path);
        self.put_figure(&named, &path, before, &told);
    }

    /// A figure of the image file `named` (as the document names it; it
    /// is `path`) put in, its caption to be typed.
    fn put_figure(&mut self, named: &str, path: &Path, before: bool, told: &str) {
        let call = format!("image(\"{}\")", named.replace('"', "\\\""));
        let block = format!("#figure(\n  {call},\n  caption: [],\n)");
        let caption = block
            .find("caption: [")
            .map_or(block.len(), |at| at + "caption: [".len());
        if self.put_block(&block, caption, before) {
            let count = self
                .editor
                .as_ref()
                .map_or(0, |editor| editor.text().matches(call.as_str()).count());
            self.paste.pasted = Some((call, count, path.to_path_buf()));
            self.notice(format!(
                "{told}: type its caption  (u takes the figure out, the file stays; \
                 :figure for the rest)"
            ));
        }
    }

    /// `block` put in as a thing by itself near the cursor (see
    /// [`block_place`]), as one step; typing begins `caption` bytes into
    /// it. Returns whether it was put in.
    fn put_block(&mut self, block: &str, caption: usize, before: bool) -> bool {
        let Some(editor) = &mut self.editor else {
            return false;
        };
        let (at, lead, trail) = block_place(editor.source(), editor.cursor(), before);
        let with = format!("{lead}{block}{trail}");
        if let Err(why) = editor.replace(&[(at..at, with.as_str())]) {
            self.alert(format!("not put in: {why}"));
            return false;
        }
        self.text_changed(Edited::Command);
        if let Some(editor) = &mut self.editor {
            editor.point(at + lead.len() + caption);
        }
        self.editor_key(KeyPress::plain(Key::Char('i')));
        self.follow_cursor();
        true
    }

    /// Cells from the clipboard: into the table the cursor is in, from
    /// its cell on (asked first where cells that hold something would be
    /// written over); else as a table of their own.
    fn paste_cells(&mut self, cells: Vec<Vec<String>>, before: bool) {
        if let Ok((start, cell)) = self.table_here() {
            let Some(editor) = &self.editor else {
                return;
            };
            let Some(shape) = table_text::Shape::new(editor.source(), start) else {
                return;
            };
            let place = shape.place(cell);
            let filled = shape.filled_under(place.0, place.1, &cells);
            if filled > 0 {
                let items = vec![
                    (
                        format!("yes: write over the {filled} cells that hold something"),
                        Target::Command("paste cells over".to_owned()),
                    ),
                    (
                        "no: leave the table as it is".to_owned(),
                        Target::Command("paste cells no".to_owned()),
                    ),
                ];
                self.paste.cells = Some((start, place, cells));
                self.open_picker("cells written over?", items, 1);
                return;
            }
            return self.fill_cells(start, place, &cells);
        }
        let (rows, columns) = (cells.len(), cells.iter().map(Vec::len).max().unwrap_or(0));
        let (block, caption) = table_text::new_table(&cells);
        if self.put_block(&block, caption, before) {
            self.notice(format!(
                "a table of {rows} rows and {columns} columns: type its caption  \
                 (u takes it back; ]r ]c ]h and the like shape it)"
            ));
        }
    }

    /// Puts `cells` into the table beginning at byte `start` from the
    /// cell at `place` on, as one step.
    fn fill_cells(&mut self, start: usize, place: (usize, usize), cells: &[Vec<String>]) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let Some((range, table)) = table_text::filled(editor.source(), start, place, cells) else {
            return self.alert("the cells do not fit into this table");
        };
        if let Err(why) = editor.replace(&[(range, table.as_str())]) {
            return self.alert(format!("not changed: {why}"));
        }
        self.text_changed(Edited::Command);
        self.cursor_to_cell(start, place);
        let count: usize = cells.iter().map(Vec::len).sum();
        self.notice(format!("{count} cells put in from here  (u takes it back)"));
    }

    /// `:paste` and what its lists run. Whether `command` was one.
    pub(super) fn paste_command(&mut self, command: &str) -> bool {
        let Some(rest) = command.strip_prefix("paste") else {
            return false;
        };
        let rest = rest.trim();
        let (selection, rest) = match rest.strip_prefix("primary") {
            Some(rest) => (Selection::Primary, rest.trim()),
            None => (Selection::Clipboard, rest),
        };
        let want = match rest {
            "" => Want::Any,
            "image" => Want::Image,
            "table" | "cells" => Want::Table,
            "text" => Want::Text,
            "image smaller" | "image as is" => {
                let Some(large) = self.paste.large.take() else {
                    return true;
                };
                let bytes = if rest == "image smaller" {
                    match image_file::smaller(&large.bytes) {
                        Ok(bytes) => bytes,
                        Err(why) => {
                            self.alert(format!("it was not made smaller: {why}"));
                            large.bytes
                        }
                    }
                } else {
                    large.bytes
                };
                self.write_image(large.ending, &bytes, large.stem, large.before, None);
                return true;
            }
            "cells over" | "cells no" => {
                if let Some((start, place, cells)) = self.paste.cells.take()
                    && rest == "cells over"
                {
                    self.fill_cells(start, place, &cells);
                }
                return true;
            }
            _ => return false,
        };
        if self.editor.is_none() {
            self.alert("no file is being edited");
            return true;
        }
        self.read_clipboard(For::Command(selection, want), Vec::new());
        true
    }

    /// After a key: what zetwerk yanked or deleted is newer than what
    /// the clipboard holds; a figure pasted and taken out again leaves
    /// its file, which is said.
    pub(super) fn paste_after_key(&mut self) {
        let Some(editor) = &self.editor else {
            return;
        };
        if matches!(
            editor.engine().took(),
            Some(zetwerk_vim::Took::Yanked(_) | zetwerk_vim::Took::Deleted { .. })
        ) {
            self.paste.seen = (self.paste.news)().map(|news| news.serial);
        }
        let gone =
            self.paste.pasted.as_ref().is_some_and(|(call, count, _)| {
                editor.text().matches(call.as_str()).count() < *count
            });
        if gone && let Some((_, _, path)) = self.paste.pasted.take() {
            let name = path
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            self.notice(format!(
                "the figure is out; {name} stays beside the document (delete the file \
                 yourself if it is not wanted)"
            ));
        }
    }

    /// After a key: what it put into `"*` goes into the primary
    /// selection.
    pub(super) fn register_to_primary(&mut self) {
        let Some(editor) = &self.editor else {
            return;
        };
        let Some((text, _)) = editor.engine().register('*') else {
            return;
        };
        if self.paste.known_primary.as_deref() == Some(text) {
            return;
        }
        let text = text.to_owned();
        let written = (self.paste.write_primary)(&text);
        self.paste.known_primary = Some(text);
        if let Err(why) = written {
            self.alert(why);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use super::*;
    use typst::syntax::Source;

    #[test]
    fn a_thing_by_itself_goes_below_the_paragraph_of_the_cursor() {
        // The text, where the cursor is (`|`), and where the thing goes
        // (`^`) below and above.
        let cases = [
            (
                "One |two.\n\nThree.\n",
                "One two.^\n\nThree.\n",
                "^One two.\n\nThree.\n",
            ),
            (
                "One.\n\nTwo |three.",
                "One.\n\nTwo three.^",
                "One.\n\n^Two three.",
            ),
            (
                "A #footnote[in |it] b.\n\nC.\n",
                "A #footnote[in it] b.^\n\nC.\n",
                "^A #footnote[in it] b.\n\nC.\n",
            ),
            (
                "= Head\n\nA $x + |y$ b.\n",
                "= Head\n\nA $x + y$ b.^\n",
                "= Head\n\n^A $x + y$ b.\n",
            ),
            ("|", "^", "^"),
        ];
        for (text, below, above) in cases {
            let at = text.find('|').unwrap();
            let text = text.replace('|', "");
            let source = Source::detached(text.as_str());
            for (before, wanted) in [(false, below), (true, above)] {
                let (place, _, _) = block_place(&source, at, before);
                let mut shown = text.clone();
                shown.insert(place, '^');
                assert_eq!(shown, wanted, "{text:?} before: {before}");
            }
        }
    }
}
