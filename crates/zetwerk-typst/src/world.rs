//! The Typst [`World`] zetwerk compiles against.
//!
//! Three layers, from long-lived to short-lived:
//!
//! - [`Environment`]: standard library, fonts and package access. Expensive to
//!   build (system font scan), shared by every project and every compile.
//! - [`Project`]: one document on disk plus the sources the editor owns. The
//!   editor mutates sources here; they are authoritative over what is on disk.
//! - [`ProjectWorld`]: an immutable snapshot of a project at one revision,
//!   cheap to create and safe to hand to a compile thread.

use std::collections::HashMap;
use std::fmt;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_kit::datetime::Time;
use typst_kit::downloader::SystemDownloader;
use typst_kit::files::FsRoot;
use typst_kit::fonts::{self, FontStore};
use typst_kit::packages::SystemPackages;

/// Which fonts an [`Environment`] offers to documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontSet {
    /// Only the fonts shipped with Typst. Deterministic across machines, so
    /// this is what tests and benchmarks use.
    Embedded,
    /// Embedded fonts plus everything installed on the system.
    EmbeddedAndSystem,
}

/// Everything a compile needs that does not depend on the document.
pub struct Environment {
    library: LazyHash<Library>,
    fonts: FontStore,
    packages: SystemPackages,
}

impl Environment {
    /// Builds the environment. With [`FontSet::EmbeddedAndSystem`] this scans
    /// the system font directories, which takes a noticeable moment; build it
    /// once and share it.
    #[must_use]
    pub fn new(font_set: FontSet) -> Arc<Self> {
        let mut store = FontStore::new();
        store.extend(fonts::embedded());
        if font_set == FontSet::EmbeddedAndSystem {
            store.extend(fonts::system());
        }
        Arc::new(Self {
            library: LazyHash::new(Library::default()),
            fonts: store,
            packages: SystemPackages::new(SystemDownloader::new(concat!(
                "zetwerk/",
                env!("CARGO_PKG_VERSION")
            ))),
        })
    }
}

/// Why a project could not be opened.
#[derive(Debug)]
pub enum OpenError {
    /// The main file has no parent directory or file name to derive a project
    /// root from.
    InvalidPath(PathBuf),
    /// The main file could not be read.
    Io(PathBuf, std::io::Error),
    /// The main file is not valid UTF-8.
    NotUtf8(PathBuf),
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath(path) => {
                write!(f, "{} is not a usable file path", path.display())
            }
            Self::Io(path, err) => write!(f, "cannot read {}: {err}", path.display()),
            Self::NotUtf8(path) => write!(f, "{} is not valid UTF-8", path.display()),
        }
    }
}

impl std::error::Error for OpenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(_, err) => Some(err),
            Self::InvalidPath(_) | Self::NotUtf8(_) => None,
        }
    }
}

/// A document and the sources the editor owns for it.
pub struct Project {
    env: Arc<Environment>,
    root: FsRoot,
    main: FileId,
    /// Where the main file is on disk; `None` for an in-memory project.
    main_path: Option<PathBuf>,
    /// Sources owned by the editor. These win over the files on disk.
    sources: HashMap<FileId, Source>,
    /// Files the editor does not own: images, bibliographies, packages and
    /// project sources that have not been adopted for editing.
    external: Arc<ExternalFiles>,
}

impl Project {
    /// Opens the document whose main file is at `main_path`. The project root
    /// is the directory containing that file.
    pub fn open(env: Arc<Environment>, main_path: &Path) -> Result<Self, OpenError> {
        let invalid = || OpenError::InvalidPath(main_path.to_owned());
        let absolute = std::path::absolute(main_path)
            .map_err(|err| OpenError::Io(main_path.to_owned(), err))?;
        let root_dir = absolute.parent().ok_or_else(invalid)?;
        let name = absolute
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(invalid)?;
        let bytes = std::fs::read(&absolute).map_err(|err| OpenError::Io(absolute.clone(), err))?;
        let text = String::from_utf8(bytes).map_err(|_| OpenError::NotUtf8(absolute.clone()))?;
        let vpath = VirtualPath::new(name).map_err(|_| invalid())?;
        let mut project = Self::with_main(env, root_dir.to_owned(), &vpath, text);
        project.main_path = Some(absolute);
        Ok(project)
    }

    /// Creates a project whose main source lives only in memory. Other files
    /// are still resolved relative to `root_dir`.
    #[must_use]
    pub fn in_memory(
        env: Arc<Environment>,
        root_dir: PathBuf,
        main_name: &str,
        text: String,
    ) -> Option<Self> {
        let vpath = VirtualPath::new(main_name).ok()?;
        Some(Self::with_main(env, root_dir, &vpath, text))
    }

    fn with_main(
        env: Arc<Environment>,
        root_dir: PathBuf,
        main_vpath: &VirtualPath,
        text: String,
    ) -> Self {
        // `intern` returns the same id for the same path, so a file id is
        // minted once per path for the lifetime of the process. Typst's
        // interner holds at most 2^16 ids; never create ids per compile.
        let main = RootedPath::new(VirtualRoot::Project, main_vpath.clone()).intern();
        let strip_bom = text.strip_prefix('\u{feff}').map(str::to_owned);
        let source = Source::new(main, strip_bom.unwrap_or(text));
        Self {
            env,
            root: FsRoot::new(root_dir),
            main,
            main_path: None,
            sources: HashMap::from([(main, source)]),
            external: Arc::default(),
        }
    }

    /// The id of the main source file.
    #[must_use]
    pub fn main(&self) -> FileId {
        self.main
    }

    /// A source owned by the editor.
    #[must_use]
    pub fn source(&self, id: FileId) -> Option<&Source> {
        self.sources.get(&id)
    }

    /// Replaces `range` (byte offsets) of an owned source with `text` and
    /// reparses it incrementally. Returns the range of the new text that was
    /// reparsed, or `None` if the editor does not own `id` or the range is
    /// not a valid slice of the source.
    pub fn edit(&mut self, id: FileId, range: Range<usize>, text: &str) -> Option<Range<usize>> {
        let source = self.sources.get_mut(&id)?;
        // `Source::edit` panics on out-of-bounds or non-boundary ranges.
        source.text().get(range.clone())?;
        Some(source.edit(range, text))
    }

    /// Makes `source` the text of its file from now on. This is how text
    /// that is edited elsewhere (with its own history) gets compiled: hand
    /// it over before taking a [`Project::snapshot`]. A `Source` is shared
    /// when cloned, so this copies nothing.
    pub fn set_source(&mut self, source: Source) {
        self.sources.insert(source.id(), source);
    }

    /// Where the main file is on disk; `None` for an in-memory project.
    #[must_use]
    pub fn main_path(&self) -> Option<&Path> {
        self.main_path.as_deref()
    }

    /// Replaces the whole text of the main source, reparsing only what
    /// differs.
    pub fn set_main_text(&mut self, text: &str) {
        if let Some(source) = self.sources.get_mut(&self.main) {
            source.replace(text);
        }
    }

    /// Every file on disk the document was built from so far: the main file
    /// and whatever the last compiles read from the project directory.
    /// Package files are not included; they do not change.
    #[must_use]
    pub fn dependencies(&self) -> Vec<PathBuf> {
        let external = self
            .external
            .ids()
            .into_iter()
            .filter_map(|id| match id.root() {
                VirtualRoot::Project => self.root.resolve(id.vpath()).ok(),
                VirtualRoot::Package(_) => None,
            });
        self.main_path.iter().cloned().chain(external).collect()
    }

    /// Tells the project that `path` changed on disk. Returns whether the
    /// document has to be compiled again.
    ///
    /// The main file is read again and replaces the text in memory. Any other
    /// file is simply forgotten, so the next compile reads it afresh.
    pub fn file_changed(&mut self, path: &Path) -> Result<bool, OpenError> {
        if self.main_path.as_deref() == Some(path) {
            let bytes = std::fs::read(path).map_err(|err| OpenError::Io(path.to_owned(), err))?;
            let text = String::from_utf8(bytes).map_err(|_| OpenError::NotUtf8(path.to_owned()))?;
            let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
            let unchanged = self
                .source(self.main)
                .is_some_and(|source| source.text() == text);
            if !unchanged {
                self.set_main_text(text);
            }
            return Ok(!unchanged);
        }
        let forgotten = VirtualPath::virtualize(self.root.path(), path).is_ok_and(|vpath| {
            let id = RootedPath::new(VirtualRoot::Project, vpath).intern();
            self.external.forget(id)
        });
        Ok(forgotten)
    }

    /// An immutable view of the project as it is right now.
    #[must_use]
    pub fn snapshot(&self) -> ProjectWorld {
        ProjectWorld {
            env: Arc::clone(&self.env),
            root: self.root.clone(),
            main: self.main,
            sources: self.sources.clone(),
            external: Arc::clone(&self.external),
            time: Time::system(),
        }
    }
}

/// A project frozen at one revision. This is what Typst compiles.
pub struct ProjectWorld {
    env: Arc<Environment>,
    root: FsRoot,
    main: FileId,
    sources: HashMap<FileId, Source>,
    external: Arc<ExternalFiles>,
    time: Time,
}

impl ProjectWorld {
    /// Whether the editor owns this file, i.e. whether edits to it are
    /// possible at all. Package files and unadopted project files are not.
    #[must_use]
    pub fn is_editable(&self, id: FileId) -> bool {
        self.sources.contains_key(&id)
    }

    fn load_external(&self, id: FileId) -> FileResult<Bytes> {
        match id.root() {
            VirtualRoot::Project => self.root.load(id.vpath()),
            VirtualRoot::Package(spec) => self.env.packages.obtain(spec)?.load(id.vpath()),
        }
    }
}

impl World for ProjectWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.env.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        self.env.fonts.book()
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        match self.sources.get(&id) {
            Some(source) => Ok(source.clone()),
            None => self.external.source(id, || self.load_external(id)),
        }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        match self.sources.get(&id) {
            Some(source) => Ok(Bytes::from_string(source.clone())),
            None => self.external.file(id, || self.load_external(id)),
        }
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.env.fonts.font(index)
    }

    fn today(&self, offset: Option<Duration>) -> Option<Datetime> {
        self.time.today(offset)
    }
}

/// Cache for files the editor does not own. Entries stay until invalidated,
/// so repeated compiles never touch the disk for unchanged files.
#[derive(Default)]
struct ExternalFiles {
    slots: Mutex<HashMap<FileId, ExternalSlot>>,
}

struct ExternalSlot {
    bytes: FileResult<Bytes>,
    /// Parsed lazily: most external files are images or data, not sources.
    source: Option<FileResult<Source>>,
}

impl ExternalFiles {
    /// Every file that has been asked for, found or not.
    fn ids(&self) -> Vec<FileId> {
        self.lock().keys().copied().collect()
    }

    /// Drops what is cached for `id`. Returns whether anything was.
    fn forget(&self, id: FileId) -> bool {
        self.lock().remove(&id).is_some()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<FileId, ExternalSlot>> {
        // A panic while holding the lock can only leave a fully written slot
        // or none behind, so a poisoned map is still consistent.
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn file(&self, id: FileId, load: impl FnOnce() -> FileResult<Bytes>) -> FileResult<Bytes> {
        self.with_slot(id, load, |slot| slot.bytes.clone())
    }

    fn source(&self, id: FileId, load: impl FnOnce() -> FileResult<Bytes>) -> FileResult<Source> {
        self.with_slot(id, load, |slot| {
            let bytes = slot.bytes.clone()?;
            slot.source
                .get_or_insert_with(|| {
                    let text = std::str::from_utf8(&bytes).map_err(|_| FileError::InvalidUtf8)?;
                    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
                    Ok(Source::new(id, text.to_owned()))
                })
                .clone()
        })
    }

    fn with_slot<T>(
        &self,
        id: FileId,
        load: impl FnOnce() -> FileResult<Bytes>,
        f: impl FnOnce(&mut ExternalSlot) -> T,
    ) -> T {
        let mut slots = self.lock();
        let slot = slots.entry(id).or_insert_with(|| ExternalSlot {
            bytes: load(),
            source: None,
        });
        f(slot)
    }
}
