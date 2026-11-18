//! The Typst [`World`] zetwerk compiles against.
//!
//! Three layers, from long-lived to short-lived:
//!
//! - [`Environment`]: standard library, fonts and where packages are kept
//!   (see [`crate::packages`]: a compile never fetches one). Expensive to
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

use typst::diag::{FileError, FileResult, PackageError};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_kit::datetime::Time;
use typst_kit::files::FsRoot;
use typst_kit::fonts::{self, FontStore};

use crate::packages::PackageDirs;

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
    packages: PackageDirs,
}

impl Environment {
    /// Builds the environment. With [`FontSet::EmbeddedAndSystem`] this scans
    /// the system font directories, which takes a noticeable moment; build it
    /// once and share it.
    #[must_use]
    pub fn new(font_set: FontSet) -> Arc<Self> {
        Self::with_packages(font_set, PackageDirs::system())
    }

    /// The same, with packages found in `packages` and not where Typst's
    /// own program keeps them.
    #[must_use]
    pub fn with_packages(font_set: FontSet, packages: PackageDirs) -> Arc<Self> {
        let mut store = FontStore::new();
        store.extend(fonts::embedded());
        if font_set == FontSet::EmbeddedAndSystem {
            store.extend(fonts::system());
        }
        Arc::new(Self {
            library: LazyHash::new(Library::default()),
            fonts: store,
            packages,
        })
    }

    /// Where packages are kept.
    #[must_use]
    pub fn packages(&self) -> &PackageDirs {
        &self.packages
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

    /// Makes `source`, the text of the file at `path` in the project
    /// directory, what the editor owns in place of the file `old`, which
    /// is read from disk again from now on: for a text the editor goes on
    /// with under another name. If `old` was the main file, the new one
    /// is.
    pub fn renamed(&mut self, old: FileId, source: Source, path: &Path) {
        self.sources.remove(&old);
        self.external.forget(old);
        if old == self.main {
            self.main = source.id();
            self.main_path = std::path::absolute(path).ok();
        }
        self.sources.insert(source.id(), source);
    }

    /// Where the main file is on disk; `None` for an in-memory project.
    #[must_use]
    pub fn main_path(&self) -> Option<&Path> {
        self.main_path.as_deref()
    }

    /// The fonts, the library and the packages the project is compiled
    /// with: for another document to be compiled with them.
    #[must_use]
    pub fn env(&self) -> &Arc<Environment> {
        &self.env
    }

    /// The directory the project's files are in: what paths in the
    /// document are relative to.
    #[must_use]
    pub fn root_dir(&self) -> &Path {
        self.root.path()
    }

    /// The id of the project file at `path`, if `path` is in the project
    /// directory.
    #[must_use]
    pub fn file_id(&self, path: &Path) -> Option<FileId> {
        let absolute = std::path::absolute(path).ok()?;
        let vpath = VirtualPath::virtualize(self.root.path(), &absolute).ok()?;
        Some(RootedPath::new(VirtualRoot::Project, vpath).intern())
    }

    /// Where the project file `id` is on disk; `None` for the files of
    /// packages.
    #[must_use]
    pub fn path_of(&self, id: FileId) -> Option<PathBuf> {
        match id.root() {
            VirtualRoot::Project => self.root.resolve(id.vpath()).ok(),
            VirtualRoot::Package(_) => None,
        }
    }

    /// Where the packages the document imports are kept.
    #[must_use]
    pub fn packages(&self) -> &PackageDirs {
        self.env.packages()
    }

    /// Tells the project that packages were put in place or taken away:
    /// the next compile looks for them again.
    pub fn packages_changed(&mut self) {
        self.external.forget_packages();
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
        // And what is open for editing, whether a compile has read it
        // from disk before or not.
        let owned = self
            .sources
            .keys()
            .filter(|id| **id != self.main)
            .filter_map(|id| self.path_of(*id));
        let mut paths: Vec<PathBuf> = self
            .main_path
            .iter()
            .cloned()
            .chain(external)
            .chain(owned)
            .collect();
        paths.sort();
        paths.dedup();
        paths
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
            shown: None,
        }
    }

    /// A view of the project in which `file` is not quite as it is
    /// written: with each of `put` (a byte of the file and a text, in
    /// the order of the bytes) put into it. That is what Typst is given
    /// to typeset, so that something half written shows as what it is
    /// going to be; what is asked about the pages is answered in bytes
    /// of the file as it is written (see [`ProjectWorld::written_at`]).
    #[must_use]
    pub fn snapshot_showing(&self, file: FileId, put: &[(usize, &str)]) -> ProjectWorld {
        let mut world = self.snapshot();
        let Some(written) = self.sources.get(&file) else {
            return world;
        };
        let text = written.text();
        let in_order = put.windows(2).all(|pair| pair[0].0 <= pair[1].0);
        let in_text = put.iter().all(|(at, _)| text.is_char_boundary(*at));
        if put.is_empty() || !in_order || !in_text {
            return world;
        }
        // What is put in close together goes in at once: marks that
        // come in pairs (the quotes around a word) are never one without
        // the other, which Typst would read all that follows anew for.
        let mut shown = written.clone();
        let mut last = put.len();
        while last > 0 {
            let mut first = last - 1;
            while first > 0 && put[first].0 - put[first - 1].0 <= TOGETHER {
                first -= 1;
            }
            let bytes = put[first].0..put[last - 1].0;
            let mut with = String::new();
            let mut from = bytes.start;
            for (at, more) in &put[first..last] {
                with.push_str(&text[from..*at]);
                with.push_str(more);
                from = *at;
            }
            shown.edit(bytes, &with);
            last = first;
        }
        world.sources.insert(file, shown);
        world.shown = Some(Shown {
            file,
            written: written.clone(),
            put: put.iter().map(|(at, with)| (*at, with.len())).collect(),
        });
        world
    }
}

/// How many bytes apart two things put into a text to show it may be
/// to be put in as one change.
const TOGETHER: usize = 256;

/// How a file is shown otherwise than it is written.
struct Shown {
    file: FileId,
    /// The file as it is written.
    written: Source,
    /// What was put in to show it: at which byte of the file as written,
    /// and how many bytes.
    put: Vec<(usize, usize)>,
}

/// A project frozen at one revision. This is what Typst compiles.
pub struct ProjectWorld {
    env: Arc<Environment>,
    root: FsRoot,
    main: FileId,
    sources: HashMap<FileId, Source>,
    external: Arc<ExternalFiles>,
    time: Time,
    shown: Option<Shown>,
}

impl ProjectWorld {
    /// The source of `id` as it is written, which is not what Typst is
    /// given where something was put in to show it (see
    /// [`Project::snapshot_showing`]).
    ///
    /// # Errors
    ///
    /// As [`World::source`].
    pub fn written(&self, id: FileId) -> FileResult<Source> {
        match &self.shown {
            Some(shown) if shown.file == id => Ok(shown.written.clone()),
            _ => self.source(id),
        }
    }

    /// Whether a file is shown otherwise than it is written.
    #[must_use]
    pub fn shows_more(&self) -> bool {
        self.shown.is_some()
    }

    /// The byte of `id` as it is written that byte `offset` of what
    /// Typst was given is: itself, unless something was put in before
    /// it. What was put in is all at the byte it was put in at.
    #[must_use]
    pub fn written_at(&self, id: FileId, offset: usize) -> usize {
        let Some(shown) = self.shown.as_ref().filter(|shown| shown.file == id) else {
            return offset;
        };
        let mut more = 0;
        for (at, length) in &shown.put {
            if offset < at + more {
                break;
            }
            if offset < at + more + length {
                return *at;
            }
            more += length;
        }
        offset - more
    }

    /// The other way round: where in what Typst was given byte `offset`
    /// of `id` as it is written is. At a byte something was put in at,
    /// before that.
    #[must_use]
    pub fn shown_at(&self, id: FileId, offset: usize) -> usize {
        let Some(shown) = self.shown.as_ref().filter(|shown| shown.file == id) else {
            return offset;
        };
        let before = shown.put.iter().take_while(|(at, _)| *at < offset);
        offset + before.map(|(_, length)| length).sum::<usize>()
    }

    /// Whether edits to this file are possible at all: whether it is a
    /// file of the project. The files of packages are not; a file of the
    /// project that is not open for editing can be opened.
    #[must_use]
    pub fn is_editable(&self, id: FileId) -> bool {
        self.is_owned_or_of_project(id)
    }

    /// This view of the project with `source` as the text of its file,
    /// whatever that text was: for typesetting a part of a document by
    /// itself (see [`crate::local`]).
    #[must_use]
    pub fn with_text(&self, source: Source) -> Self {
        let mut sources = self.sources.clone();
        sources.insert(source.id(), source);
        Self {
            env: Arc::clone(&self.env),
            root: self.root.clone(),
            main: self.main,
            sources,
            external: Arc::clone(&self.external),
            time: Time::system(),
            shown: None,
        }
    }

    fn is_owned_or_of_project(&self, id: FileId) -> bool {
        self.sources.contains_key(&id) || matches!(id.root(), VirtualRoot::Project)
    }

    fn load_external(&self, id: FileId) -> FileResult<Bytes> {
        match id.root() {
            VirtualRoot::Project => self.root.load(id.vpath()),
            // Never fetched here: see `crate::packages`.
            VirtualRoot::Package(spec) => match crate::packages::find(&self.env.packages, spec) {
                Some((dir, _)) => FsRoot::new(dir).load(id.vpath()),
                None => Err(FileError::Package(PackageError::NotFound(spec.clone()))),
            },
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

    /// Drops what is cached of the files of packages.
    fn forget_packages(&self) {
        self.lock()
            .retain(|id, _| !matches!(id.root(), VirtualRoot::Package(_)));
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

/// The main file of the document that the file at `path` is part of: the
/// file that includes it, or includes the file that does, as far up as
/// that goes. `path` itself if nothing includes it.
///
/// Typst has no place where a document says which file is its main one,
/// so it is looked for: in the directory of the file and those above it
/// (up to the one that holds `typst.toml` or `.git`), among the Typst
/// files there, `main.typ` first.
#[must_use]
pub fn find_main(path: &Path) -> PathBuf {
    /// How many directories up a main file is looked for.
    const LEVELS: usize = 6;
    let Ok(mut part) = std::path::absolute(path) else {
        return path.to_owned();
    };
    let mut directory = part.parent().map(Path::to_owned);
    let mut levels = 0;
    while let Some(here) = directory {
        let mut candidates: Vec<PathBuf> = std::fs::read_dir(&here)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|file| file.extension().is_some_and(|ending| ending == "typ") && *file != part)
            .collect();
        // `main.typ` before the others, and those in a fixed order.
        candidates
            .sort_by_key(|file| (file.file_name() != Some("main.typ".as_ref()), file.clone()));
        let includer = candidates
            .into_iter()
            .find(|candidate| includes(candidate, &here, &part, 0));
        let top = here.join("typst.toml").exists() || here.join(".git").exists();
        match includer {
            // It may be a part of something itself: on from there, in
            // the same directory first.
            Some(includer) => {
                part = includer;
                directory = Some(here);
            }
            None if top || levels >= LEVELS => break,
            None => {
                levels += 1;
                directory = here.parent().map(Path::to_owned);
            }
        }
    }
    part
}

/// Whether the Typst file `file`, as a file of a project in `root`,
/// includes `wanted`, itself or through files it includes.
fn includes(file: &Path, root: &Path, wanted: &Path, depth: usize) -> bool {
    /// How deep includes are followed.
    const DEPTH: usize = 8;
    let Ok(text) = std::fs::read_to_string(file) else {
        return false;
    };
    let mut stack = vec![typst::syntax::parse(&text)];
    let mut included = Vec::new();
    while let Some(node) = stack.pop() {
        if node.kind() == typst::syntax::SyntaxKind::ModuleInclude {
            let literal = node
                .children()
                .find(|child| child.kind() == typst::syntax::SyntaxKind::Str);
            if let Some(literal) = literal {
                let name = literal.leaf_text().trim_matches('"');
                // From the project directory, or from this file's.
                let target = match name.strip_prefix('/') {
                    Some(rooted) => root.join(rooted),
                    None => file.parent().unwrap_or(root).join(name),
                };
                included.push(target);
            }
        }
        stack.extend(node.children().cloned());
    }
    let same = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    included.iter().any(|target| same(target, wanted))
        || (depth < DEPTH
            && included
                .iter()
                .any(|target| includes(target, root, wanted, depth + 1)))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use super::*;

    #[test]
    fn the_main_file_is_the_one_that_includes_the_others() {
        let root = std::env::temp_dir().join(format!("zetwerk-main-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("chapters/deep")).unwrap();
        std::fs::write(root.join("typst.toml"), "").unwrap();
        let write = |name: &str, text: &str| std::fs::write(root.join(name), text).unwrap();
        write("thesis.typ", "= Thesis\n#include \"chapters/one.typ\"\n");
        write("notes.typ", "Nothing to do with it.\n");
        write("chapters/one.typ", "== One\n#include \"deep/part.typ\"\n");
        write("chapters/deep/part.typ", "Part.\n");
        write("chapters/alone.typ", "Alone.\n");
        let main = |name: &str| find_main(&root.join(name));
        let same =
            |a: PathBuf, b: &str| a.canonicalize().unwrap() == root.join(b).canonicalize().unwrap();
        // Up through two includes; from the middle; and the top itself.
        assert!(same(main("chapters/deep/part.typ"), "thesis.typ"));
        assert!(same(main("chapters/one.typ"), "thesis.typ"));
        assert!(same(main("thesis.typ"), "thesis.typ"));
        // What nothing includes is its own document.
        assert!(same(main("chapters/alone.typ"), "chapters/alone.typ"));
        assert!(same(main("notes.typ"), "notes.typ"));
        let _ = std::fs::remove_dir_all(root);
    }
}
