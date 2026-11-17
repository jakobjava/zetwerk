//! Typst packages: where they are kept on this machine, which ones a
//! document imports, and fetching one from Typst's registry.
//!
//! What was decided:
//!
//! - **Found where Typst finds them, and nowhere else.** A package is
//!   looked for among the writer's own first
//!   (`~/.local/share/typst/packages/<namespace>/<name>/<version>`, where
//!   `@local` ones are kept), then among those fetched before
//!   (`~/.cache/typst/packages/…`), as Typst's own program does (see
//!   [`find`]). A compile never asks the network for anything: a package
//!   in neither place is not found, and Typst says so.
//! - **Fetching is a step of its own,** which only the writer starts
//!   (`zetwerk-app`'s `viewer/packages.rs` says when). What goes over the
//!   network is a request for one package by name and version,
//!   `https://packages.typst.org/preview/<name>-<version>.tar.gz` (see
//!   [`archive_url`]), and, asked for by itself, the registry's list of
//!   packages, `…/preview/index.json` ([`index_url`]). Nothing of any
//!   document goes; the request says it comes from `zetwerk`, and no more.
//! - **Nothing half there.** What comes back is unpacked beside where it
//!   goes (a `.tmp-zetwerk-…` directory next to the package's versions),
//!   checked (its `typst.toml` names the package asked for, and its entry
//!   point is there) and renamed into place: a fetch that is abandoned,
//!   cut off or refused leaves nothing behind, and a compile meanwhile
//!   finds the package whole or not at all.
//! - **Nothing outside.** An archive that names a place outside its own
//!   directory (`../`, `/…`), holds anything but files and directories (a
//!   link could lead out), or is larger than [`ARCHIVE_LARGEST`] packed
//!   or [`UNPACKED_LARGEST`] unpacked is refused whole. Files are written
//!   new (never over one that is there) and with the usual permissions,
//!   not those the archive asks for.
//! - **What a package needs comes along.** A package may import others;
//!   those of `@preview` that are not on the machine are fetched with it
//!   (see [`fetch_with_needs`]), as Typst's own program would on the
//!   next compile.

use std::collections::BTreeSet;
use std::fmt;
use std::io::{self, Read, Write};
use std::ops::Range;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use typst::syntax::package::{PackageManifest, PackageSpec, PackageVersion};
use typst::syntax::{LinkedNode, SyntaxKind, SyntaxNode, ast};

#[cfg(any(test, feature = "stand-in"))]
pub mod stand_in;

/// Where Typst's registry is.
pub const REGISTRY: &str = "https://packages.typst.org";
/// The namespace whose packages the registry serves.
pub const NAMESPACE: &str = "preview";
/// The most a package's archive may be, in bytes. The largest packages
/// of the registry are a few megabytes.
pub const ARCHIVE_LARGEST: u64 = 64 << 20;
/// The most what it holds may be, unpacked.
pub const UNPACKED_LARGEST: u64 = 256 << 20;
/// The most files and directories it may hold.
const ENTRIES_MOST: usize = 20_000;
/// The most the registry's list of packages may be: it is some
/// megabytes.
pub const INDEX_LARGEST: u64 = 64 << 20;
/// How many packages one fetch brings at most, with those they need.
const NEEDS_MOST: usize = 24;
/// How long the registry has to answer.
const CONNECT: Duration = Duration::from_secs(15);
const ANSWER: Duration = Duration::from_secs(30);
/// How often those who wait for a fetch are told that more came.
const TICK: Duration = Duration::from_millis(200);
/// The most of a package's `typst.toml` and README that is read.
const MANIFEST_LONGEST: u64 = 256 << 10;
/// How many lines of a README are kept.
const README_LINES: usize = 4;
/// How many files of a package are looked through for what it exports
/// or imports.
const FILES_MOST: usize = 400;

/// Where packages are kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageDirs {
    /// The writer's own, `@local` among them: what is here is never
    /// changed by zetwerk.
    pub data: Option<PathBuf>,
    /// Those fetched: where fetching puts them, and what can be removed.
    pub cache: Option<PathBuf>,
    /// The registry's list of packages, as it was last asked for.
    pub index: Option<PathBuf>,
}

impl PackageDirs {
    /// Where Typst's own program keeps them: `$XDG_DATA_HOME/typst/packages`
    /// and `$XDG_CACHE_HOME/typst/packages` (`~/.local/share/…` and
    /// `~/.cache/…` where those say nothing); the list of packages in
    /// `$XDG_CACHE_HOME/zetwerk`.
    #[must_use]
    pub fn system() -> Self {
        use typst_kit::packages::FsPackages;
        let data = FsPackages::system_data().map(|packages| packages.path().to_owned());
        let cache = FsPackages::system_cache().map(|packages| packages.path().to_owned());
        // `<cache>/typst/packages`: two up is the cache of all programs.
        let index = cache
            .as_deref()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .map(|all| all.join("zetwerk/package-index.json"));
        Self { data, cache, index }
    }

    /// All in `directory`: `data`, `cache` and `index.json` in it. For
    /// tests, and for whoever wants packages of their own.
    #[must_use]
    pub fn within(directory: &Path) -> Self {
        Self {
            data: Some(directory.join("data")),
            cache: Some(directory.join("cache")),
            index: Some(directory.join("index.json")),
        }
    }
}

impl Default for PackageDirs {
    fn default() -> Self {
        Self::system()
    }
}

/// Which of the two a package is kept in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Place {
    /// The writer's own ([`PackageDirs::data`]).
    Own,
    /// Fetched ([`PackageDirs::cache`]).
    Fetched,
}

impl Place {
    /// How the place is said.
    #[must_use]
    pub fn said(self) -> &'static str {
        match self {
            Self::Own => "your own",
            Self::Fetched => "fetched",
        }
    }
}

/// The directory of `spec` under `base`: `<namespace>/<name>/<version>`.
/// Namespaces and names are identifiers, versions numbers and dots, so
/// none of them leads anywhere else.
fn spec_dir(base: &Path, spec: &PackageSpec) -> PathBuf {
    base.join(spec.namespace.as_str())
        .join(spec.name.as_str())
        .join(spec.version.to_string())
}

/// Where `spec` is on this machine, if it is: among the writer's own
/// first, then among those fetched.
#[must_use]
pub fn find(dirs: &PackageDirs, spec: &PackageSpec) -> Option<(PathBuf, Place)> {
    let places = [(&dirs.data, Place::Own), (&dirs.cache, Place::Fetched)];
    places.into_iter().find_map(|(base, place)| {
        let dir = spec_dir(base.as_deref()?, spec);
        dir.is_dir().then_some((dir, place))
    })
}

/// `spec` as a writer reads it: `touying 0.8.0`, with the namespace where
/// it is not the registry's (`@local/mine 1.0.0`).
#[must_use]
pub fn said(spec: &PackageSpec) -> String {
    if spec.namespace == NAMESPACE {
        format!("{} {}", spec.name, spec.version)
    } else {
        format!("@{}/{} {}", spec.namespace, spec.name, spec.version)
    }
}

/// Reads what the writer typed for a package: `touying:0.8.0`,
/// `@preview/touying:0.8.0`, `@local/mine:1.0.0`. Without a namespace
/// it is the registry's.
pub fn parse_spec(text: &str) -> Result<PackageSpec, String> {
    let text = text.trim();
    let full = if text.starts_with('@') {
        text.to_owned()
    } else {
        format!("@{NAMESPACE}/{text}")
    };
    PackageSpec::from_str(&full).map_err(|why| why.to_string())
}

/// A package a file imports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    pub spec: PackageSpec,
    /// The bytes of the string that names it, quotes and all.
    pub literal: Range<usize>,
    /// What is taken of it.
    pub names: Imported,
}

/// What an import takes of a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Imported {
    /// The module, by this name: `#import "@preview/cetz:0.3.0"` binds
    /// `cetz`, and the document says `cetz.canvas`.
    Module(String),
    /// These, by the names the package gives them.
    Items(Vec<String>),
    /// All it exports: `: *`.
    All,
}

/// The packages the text whose syntax is `root` imports, in the order
/// of the text: wherever an import is, in code blocks and functions too.
#[must_use]
pub fn imports(root: &SyntaxNode) -> Vec<Import> {
    let mut found = Vec::new();
    let mut stack = vec![LinkedNode::new(root)];
    while let Some(node) = stack.pop() {
        if node.kind() == SyntaxKind::ModuleImport {
            found.extend(import_of(&node));
            continue;
        }
        // In reverse, so that they are taken in the order of the text.
        let children: Vec<LinkedNode> = node.children().collect();
        stack.extend(children.into_iter().rev());
    }
    found
}

/// What the import `node` takes, if it is of a package.
fn import_of(node: &LinkedNode) -> Option<Import> {
    let import = node.cast::<ast::ModuleImport>()?;
    let ast::Expr::Str(named) = import.source() else {
        return None;
    };
    let named = named.get();
    if !named.starts_with('@') {
        return None;
    }
    let spec = PackageSpec::from_str(&named).ok()?;
    let literal = node
        .children()
        .find(|child| child.kind() == SyntaxKind::Str)?;
    let names = match import.imports() {
        Some(ast::Imports::Wildcard) => Imported::All,
        Some(ast::Imports::Items(items)) => Imported::Items(
            items
                .iter()
                .map(|item| item.original_name().get().to_string())
                .collect(),
        ),
        None => Imported::Module(
            import
                .new_name()
                .map_or_else(|| spec.name.to_string(), |name| name.get().to_string()),
        ),
    };
    Some(Import {
        spec,
        literal: literal.range(),
        names,
    })
}

/// The names the text whose syntax is `root` uses of what `import`
/// takes of its package: of a module, the fields taken of it
/// (`cetz.canvas`); of items, those named elsewhere in the text; of
/// all, those of `exports` (what the package has, see [`exports`]) that
/// the text names. As far as the text shows: a name passed on
/// elsewhere is not followed.
#[must_use]
pub fn used(root: &SyntaxNode, import: &Import, exports: &BTreeSet<String>) -> BTreeSet<String> {
    let mut named = BTreeSet::new();
    let mut fields = BTreeSet::new();
    let module = match &import.names {
        Imported::Module(name) => Some(name.as_str()),
        _ => None,
    };
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        match node.kind() {
            // What an import names is not a use of it.
            SyntaxKind::ModuleImport => continue,
            SyntaxKind::Ident | SyntaxKind::MathIdent => {
                named.insert(node.leaf_text().to_string());
            }
            SyntaxKind::FieldAccess => {
                if let Some(access) = node.cast::<ast::FieldAccess>()
                    && let ast::Expr::Ident(target) = access.target()
                    && Some(target.get().as_str()) == module
                {
                    fields.insert(access.field().get().to_string());
                }
            }
            _ => {}
        }
        stack.extend(node.children());
    }
    match &import.names {
        Imported::Module(_) => fields,
        Imported::Items(items) => items
            .iter()
            .filter(|item| named.contains(*item))
            .cloned()
            .collect(),
        Imported::All => exports.intersection(&named).cloned().collect(),
    }
}

/// The names the package in `dir` exports from its entry point
/// `entrypoint`: what is bound at the top of that file, and what it
/// imports there from its own files (all of one with `*`), as far as
/// [`FILES_MOST`] files go.
#[must_use]
pub fn exports(dir: &Path, entrypoint: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut read = 0;
    let Some(entry) = inside(Path::new(entrypoint)) else {
        return names;
    };
    let mut files = vec![entry];
    let mut seen = BTreeSet::new();
    while let Some(file) = files.pop() {
        if !seen.insert(file.clone()) || read >= FILES_MOST {
            continue;
        }
        read += 1;
        let Ok(text) = read_small(&dir.join(&file)) else {
            continue;
        };
        let root = typst::syntax::parse(&text);
        for node in root.children() {
            if let Some(binding) = node.cast::<ast::LetBinding>() {
                names.extend(
                    binding
                        .kind()
                        .bindings()
                        .iter()
                        .map(|id| id.get().to_string()),
                );
            }
            let Some(import) = node.cast::<ast::ModuleImport>() else {
                continue;
            };
            match import.imports() {
                Some(ast::Imports::Items(items)) => {
                    names.extend(items.iter().map(|item| item.bound_name().get().to_string()));
                }
                Some(ast::Imports::Wildcard) => {
                    let ast::Expr::Str(path) = import.source() else {
                        continue;
                    };
                    let path = path.get();
                    // Of the package itself: from its root, or from here.
                    let within = match path.strip_prefix('/') {
                        Some(rooted) => inside(Path::new(rooted)),
                        None => inside(&file.parent().unwrap_or(Path::new("")).join(path.as_str())),
                    };
                    files.extend(within);
                }
                None => {
                    if let Ok(name) = import.bare_name() {
                        names.insert(name.to_string());
                    }
                }
            }
        }
    }
    names
}

/// The packages a package in `dir` imports, in any of its files.
#[must_use]
pub fn needs(dir: &Path) -> Vec<PackageSpec> {
    let mut found: Vec<PackageSpec> = Vec::new();
    for file in typst_files(dir) {
        let Ok(text) = read_small(&file) else {
            continue;
        };
        for import in imports(&typst::syntax::parse(&text)) {
            if !found.contains(&import.spec) {
                found.push(import.spec);
            }
        }
    }
    found
}

/// The Typst files under `dir`, as far as [`FILES_MOST`] go; links are
/// not followed.
fn typst_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut directories = vec![dir.to_owned()];
    let mut seen = 0;
    while let Some(directory) = directories.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > FILES_MOST * 4 {
                return found;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                directories.push(path);
            } else if kind.is_file() && path.extension().is_some_and(|ending| ending == "typ") {
                found.push(path);
            }
        }
    }
    found.sort();
    found.truncate(FILES_MOST);
    found
}

/// Reads a text file that is not larger than [`MANIFEST_LONGEST`]
/// times four.
fn read_small(path: &Path) -> io::Result<String> {
    let mut text = String::new();
    std::fs::File::open(path)?
        .take(MANIFEST_LONGEST * 4)
        .read_to_string(&mut text)?;
    Ok(text)
}

/// `path` as a path inside a directory: its parts, without `.`; `None`
/// where it is empty or leads out (`..`, `/…`).
fn inside(path: &Path) -> Option<PathBuf> {
    let mut within = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => within.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!within.as_os_str().is_empty()).then_some(within)
}

/// The manifest of the package in `dir`, its `typst.toml`.
pub fn manifest(dir: &Path) -> Result<PackageManifest, String> {
    let path = dir.join("typst.toml");
    let mut text = String::new();
    std::fs::File::open(&path)
        .and_then(|file| file.take(MANIFEST_LONGEST).read_to_string(&mut text))
        .map_err(|err| format!("its typst.toml cannot be read: {err}"))?;
    toml::from_str(&text).map_err(|err| format!("its typst.toml is not as Typst wants it: {err}"))
}

/// What a package says of itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct About {
    pub description: Option<String>,
    pub authors: Vec<String>,
    pub license: Option<String>,
    pub entrypoint: String,
    pub homepage: Option<String>,
    /// Where its template is, and the file of it a document begins
    /// with, if it is one.
    pub template: Option<(String, String)>,
    /// The first lines of its README, where it has one: what it is, in
    /// its own words.
    pub readme: Vec<String>,
}

/// What the package in `dir` says of itself.
pub fn about(dir: &Path) -> Result<About, String> {
    let manifest = manifest(dir)?;
    let package = manifest.package;
    Ok(About {
        description: package.description.map(|said| said.to_string()),
        authors: package.authors.iter().map(ToString::to_string).collect(),
        license: package.license.map(|said| said.to_string()),
        entrypoint: package.entrypoint.to_string(),
        homepage: package
            .homepage
            .or(package.repository)
            .map(|link| link.to_string()),
        template: manifest
            .template
            .map(|template| (template.path.to_string(), template.entrypoint.to_string())),
        readme: readme(dir),
    })
}

/// The first lines of prose of the README in `dir`: no headings' marks,
/// pictures, badges, markup or code.
fn readme(dir: &Path) -> Vec<String> {
    let Ok(text) = read_small(&dir.join("README.md")) else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    let mut code = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("```") {
            code = !code;
            continue;
        }
        let picture = ["![", "[![", "<", "|", "---", "==="]
            .iter()
            .any(|begins| line.starts_with(begins));
        if code || line.is_empty() || picture {
            continue;
        }
        let line = line.trim_start_matches('#').trim();
        if !line.is_empty() {
            lines.push(line.to_owned());
        }
        if lines.len() >= README_LINES {
            break;
        }
    }
    lines
}

/// A package on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kept {
    pub spec: PackageSpec,
    pub dir: PathBuf,
    pub place: Place,
}

/// The packages on this machine: the writer's own, then those fetched;
/// by namespace and name, the newest version first.
#[must_use]
pub fn on_machine(dirs: &PackageDirs) -> Vec<Kept> {
    let mut found = Vec::new();
    let places = [(&dirs.data, Place::Own), (&dirs.cache, Place::Fetched)];
    for (base, place) in places {
        let Some(base) = base else {
            continue;
        };
        let names = |dir: &Path| -> Vec<(String, PathBuf)> {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return Vec::new();
            };
            entries
                .flatten()
                .take(FILES_MOST * 10)
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .filter_map(|entry| Some((entry.file_name().into_string().ok()?, entry.path())))
                .filter(|(name, _)| !name.starts_with('.'))
                .collect()
        };
        let mut here = Vec::new();
        for (namespace, within) in names(base) {
            for (name, versions) in names(&within) {
                for (version, dir) in names(&versions) {
                    let spec = format!("@{namespace}/{name}:{version}");
                    if let Ok(spec) = PackageSpec::from_str(&spec) {
                        here.push(Kept { spec, dir, place });
                    }
                }
            }
        }
        here.sort_by(|a, b| {
            (&a.spec.namespace, &a.spec.name, b.spec.version).cmp(&(
                &b.spec.namespace,
                &b.spec.name,
                a.spec.version,
            ))
        });
        found.extend(here);
    }
    found
}

/// Takes `spec` off the machine: only from the packages fetched, never
/// the writer's own, and only what is in that directory itself. It is
/// first renamed, so that it is never found half gone. Returns where it
/// was.
pub fn remove(dirs: &PackageDirs, spec: &PackageSpec) -> Result<PathBuf, String> {
    let cache = dirs
        .cache
        .as_ref()
        .ok_or("there is no directory of fetched packages")?;
    let dir = spec_dir(cache, spec);
    let not_there = || format!("{} is not among the fetched packages", said(spec));
    let meta = std::fs::symlink_metadata(&dir).map_err(|_| not_there())?;
    if !meta.is_dir() {
        return Err(not_there());
    }
    // In the cache, three below it, whatever links on the way lead to.
    let real = |path: &Path| path.canonicalize().map_err(|err| err.to_string());
    let (real_cache, real_dir) = (real(cache)?, real(&dir)?);
    let below = real_dir
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent);
    if below != Some(real_cache.as_path()) {
        return Err(format!(
            "{} is not inside {}: left as it is",
            dir.display(),
            cache.display()
        ));
    }
    let versions = dir.parent().ok_or_else(not_there)?;
    let going = versions.join(format!(
        ".tmp-zetwerk-removing-{}-{}-{}",
        spec.version,
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::rename(&dir, &going).map_err(|err| format!("cannot remove it: {err}"))?;
    std::fs::remove_dir_all(&going).map_err(|err| format!("not all of it is removed: {err}"))?;
    // The package's directory, where no version is left in it.
    let _ = std::fs::remove_dir(versions);
    Ok(dir)
}

/// Tells unpacking directories of one process apart.
static NEXT: AtomicUsize = AtomicUsize::new(0);

/// Where the archive of `spec` is in the registry at `base`: only
/// packages of its namespace are there.
#[must_use]
pub fn archive_url(base: &str, spec: &PackageSpec) -> Option<String> {
    let plain = |text: &str| {
        text.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_".contains(c))
    };
    (spec.namespace == NAMESPACE && plain(&spec.name)).then(|| {
        format!(
            "{}/{NAMESPACE}/{}-{}.tar.gz",
            base.trim_end_matches('/'),
            spec.name,
            spec.version
        )
    })
}

/// Where the registry at `base` lists its packages.
#[must_use]
pub fn index_url(base: &str) -> String {
    format!("{}/{NAMESPACE}/index.json", base.trim_end_matches('/'))
}

/// How far a fetch has come, and the word to abandon it: shared between
/// the thread that fetches and whoever waits for it.
pub struct Progress {
    got: AtomicU64,
    /// How much is coming, plus one; 0 where that is not known.
    total: AtomicU64,
    stop: AtomicBool,
    /// Called now and then while bytes come in: at most every [`TICK`].
    tick: Option<Box<dyn Fn() + Send + Sync>>,
    ticked: Mutex<Option<Instant>>,
}

impl Default for Progress {
    fn default() -> Self {
        Self {
            got: AtomicU64::new(0),
            total: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            tick: None,
            ticked: Mutex::new(None),
        }
    }
}

impl Progress {
    /// Progress that calls `tick` now and then while bytes come in.
    #[must_use]
    pub fn new(tick: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            tick: Some(Box::new(tick)),
            ..Self::default()
        }
    }

    /// Asks the fetch to stop: it does, before it reads or writes more.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    /// Whether it was asked to stop.
    #[must_use]
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    /// How many bytes came of what is being fetched, and how many are
    /// coming, where that is known.
    #[must_use]
    pub fn got(&self) -> (u64, Option<u64>) {
        let total = self.total.load(Ordering::Acquire);
        (self.got.load(Ordering::Acquire), total.checked_sub(1))
    }

    fn begin(&self, total: Option<u64>) {
        self.got.store(0, Ordering::Release);
        self.total
            .store(total.map_or(0, |total| total + 1), Ordering::Release);
        self.ticked();
    }

    fn add(&self, bytes: usize) {
        self.got.fetch_add(bytes as u64, Ordering::AcqRel);
        let mut ticked = self.ticked.lock().unwrap_or_else(PoisonError::into_inner);
        if ticked.is_none_or(|at| at.elapsed() >= TICK) {
            *ticked = Some(Instant::now());
            drop(ticked);
            self.ticked();
        }
    }

    fn ticked(&self) {
        if let Some(tick) = &self.tick {
            tick();
        }
    }
}

/// Why a fetch did not bring what was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The writer abandoned it.
    Stopped,
    /// The registry has no such package, or no such version of it.
    NotThere,
    /// Only packages of `@preview` are in the registry.
    NotFetchable,
    /// The registry could not be asked, or did not answer as it should.
    Network(String),
    /// It is larger than is taken.
    TooLarge,
    /// What came is no package that can be unpacked safely.
    Archive(String),
    /// It could not be written.
    Disk(String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stopped => write!(f, "abandoned"),
            Self::NotThere => write!(f, "packages.typst.org has no such package or version"),
            Self::NotFetchable => write!(
                f,
                "only packages of @preview are in the registry; others are your own to put in place"
            ),
            Self::Network(why) => write!(f, "packages.typst.org could not be asked: {why}"),
            Self::TooLarge => write!(
                f,
                "it is larger than zetwerk takes ({} MiB)",
                ARCHIVE_LARGEST >> 20
            ),
            Self::Archive(why) => write!(
                f,
                "what came is not a package that can be unpacked safely: {why}"
            ),
            Self::Disk(why) => write!(f, "it could not be written: {why}"),
        }
    }
}

fn disk(err: io::Error) -> FetchError {
    FetchError::Disk(err.to_string())
}

fn malformed(err: impl fmt::Display) -> FetchError {
    FetchError::Archive(err.to_string())
}

/// The proxy the environment names for `url`, as Typst's own program
/// takes it: `https_proxy` (or `http_proxy` for a plain one), then
/// `all_proxy`, in small letters or capitals; none for this machine, or
/// where `no_proxy` names the host.
fn proxy_for(url: &str) -> Option<ureq::Proxy> {
    let set = |name: &str| {
        [name.to_owned(), name.to_uppercase()]
            .into_iter()
            .find_map(|name| {
                std::env::var(name)
                    .ok()
                    .filter(|value| !value.trim().is_empty())
            })
    };
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let host = rest.split(['/', ':']).next().unwrap_or("");
    if matches!(host, "127.0.0.1" | "localhost" | "::1" | "[") {
        return None;
    }
    let excepted = set("no_proxy").is_some_and(|names| {
        names.split(',').map(str::trim).any(|name| {
            let name = name.trim_start_matches('.');
            name == "*"
                || (!name.is_empty() && (host == name || host.ends_with(&format!(".{name}"))))
        })
    });
    if excepted {
        return None;
    }
    let scheme = if url.starts_with("https:") {
        "https_proxy"
    } else {
        "http_proxy"
    };
    let named = set(scheme).or_else(|| set("all_proxy"))?;
    ureq::Proxy::new(named).ok()
}

/// What asks: with a time to answer, TLS where the address says so,
/// and never from secure to plain.
fn agent(url: &str) -> Result<ureq::Agent, FetchError> {
    let secure = url.starts_with("https:");
    let mut agent = ureq::AgentBuilder::new()
        .timeout_connect(CONNECT)
        .timeout_read(ANSWER)
        .timeout_write(ANSWER)
        .redirects(4)
        .https_only(secure)
        .user_agent(concat!("zetwerk/", env!("CARGO_PKG_VERSION")));
    if secure {
        let connector = native_tls::TlsConnector::new()
            .map_err(|err| FetchError::Network(format!("no TLS: {err}")))?;
        agent = agent.tls_connector(std::sync::Arc::new(connector));
    }
    if let Some(proxy) = proxy_for(url) {
        agent = agent.proxy(proxy);
    }
    Ok(agent.build())
}

/// Asks for `url` and takes what comes, no more than `largest` bytes,
/// saying how far it came in `progress` and stopping where that says.
pub fn download(url: &str, progress: &Progress, largest: u64) -> Result<Vec<u8>, FetchError> {
    let response = match agent(url)?.get(url).call() {
        Ok(response) => response,
        Err(ureq::Error::Status(404 | 410, _)) => return Err(FetchError::NotThere),
        Err(ureq::Error::Status(status, _)) => {
            return Err(FetchError::Network(format!("it answered {status}")));
        }
        Err(ureq::Error::Transport(err)) => return Err(FetchError::Network(err.to_string())),
    };
    let total = response
        .header("Content-Length")
        .and_then(|length| length.parse::<u64>().ok());
    // Where it comes compressed, the length is not that of what is read.
    let plain = response.header("Content-Encoding").is_none();
    if total.is_some_and(|total| total > largest) {
        return Err(FetchError::TooLarge);
    }
    progress.begin(total);
    let mut reader = response.into_reader();
    let capacity = usize::try_from(total.unwrap_or(0).min(largest)).unwrap_or(0);
    let mut data = Vec::with_capacity(capacity);
    let mut buffer = vec![0; 64 << 10];
    loop {
        if progress.stopped() {
            return Err(FetchError::Stopped);
        }
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(FetchError::Network(err.to_string())),
        };
        if (data.len() + read) as u64 > largest {
            return Err(FetchError::TooLarge);
        }
        data.extend_from_slice(&buffer[..read]);
        progress.add(read);
    }
    if plain && total.is_some_and(|total| total != data.len() as u64) {
        return Err(FetchError::Network("the answer was cut off".to_owned()));
    }
    Ok(data)
}

/// What makes the files of a package: a writer for a new file at a
/// path. That the disk may be full is tested with one that fails.
pub type Creator<'a> = dyn FnMut(&Path) -> io::Result<Box<dyn Write>> + 'a;

/// A new file at `path`, never one that is there; flushing it puts it
/// on the disk.
pub fn create_file(path: &Path) -> io::Result<Box<dyn Write>> {
    struct Synced(std::fs::File);
    impl Write for Synced {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.write(bytes)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.0.sync_all()
        }
    }
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    Ok(Box::new(Synced(file)))
}

/// Unpacks the gzipped tar archive `archive` into the directory `into`
/// (which is there, and empty), with `create` making the files. Refuses
/// it whole (with what was made left for the caller to take away) where
/// anything in it would go outside `into`, is no file or directory, or
/// it is too large; stops where `stop` says so.
pub fn unpack(
    archive: impl Read,
    into: &Path,
    stop: &AtomicBool,
    create: &mut Creator<'_>,
) -> Result<(), FetchError> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let entries = archive.entries().map_err(malformed)?;
    let (mut count, mut size) = (0, 0_u64);
    let mut buffer = vec![0; 64 << 10];
    for entry in entries {
        if stop.load(Ordering::Acquire) {
            return Err(FetchError::Stopped);
        }
        let mut entry = entry.map_err(malformed)?;
        count += 1;
        if count > ENTRIES_MOST {
            return Err(FetchError::Archive(format!(
                "it holds more than {ENTRIES_MOST} files"
            )));
        }
        let named = entry.path().map_err(malformed)?.into_owned();
        let kind = entry.header().entry_type();
        if matches!(
            kind,
            tar::EntryType::XGlobalHeader | tar::EntryType::XHeader
        ) {
            continue;
        }
        let Some(within) = inside(&named) else {
            if named.components().all(|part| part == Component::CurDir) {
                continue;
            }
            return Err(FetchError::Archive(format!(
                "“{}” would be put outside the package's directory",
                named.display()
            )));
        };
        let path = into.join(&within);
        match kind {
            tar::EntryType::Directory => std::fs::create_dir_all(&path).map_err(disk)?,
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                size += entry.size();
                if size > UNPACKED_LARGEST {
                    return Err(FetchError::TooLarge);
                }
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(disk)?;
                }
                let mut file = create(&path).map_err(disk)?;
                loop {
                    let read = match entry.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(read) => read,
                        Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                        Err(err) => return Err(malformed(err)),
                    };
                    file.write_all(&buffer[..read]).map_err(disk)?;
                }
                file.flush().map_err(disk)?;
            }
            other => {
                return Err(FetchError::Archive(format!(
                    "“{}” is a {}, not a file or a directory",
                    named.display(),
                    match other {
                        tar::EntryType::Symlink => "link",
                        tar::EntryType::Link => "hard link",
                        _ => "special file",
                    }
                )));
            }
        }
    }
    // The rest, to its end: where the archive is cut off or damaged,
    // what gzip says of its length and sum is not so.
    let mut rest = archive.into_inner();
    io::copy(&mut rest, &mut io::sink()).map_err(malformed)?;
    Ok(())
}

/// A directory that goes again unless it is kept.
struct Beside {
    path: PathBuf,
    kept: bool,
}

impl Drop for Beside {
    fn drop(&mut self) {
        if !self.kept {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Puts the package `spec`, whose archive is `archive`, among those
/// fetched in `cache`: unpacked beside where it goes, checked, renamed
/// into place. Whatever goes wrong, nothing of it is left. Returns
/// where it is; where it is there already, that.
pub fn install(
    cache: &Path,
    spec: &PackageSpec,
    archive: &[u8],
    stop: &AtomicBool,
    create: &mut Creator<'_>,
) -> Result<PathBuf, FetchError> {
    let target = spec_dir(cache, spec);
    let versions = target
        .parent()
        .ok_or_else(|| FetchError::Disk("no place for it".to_owned()))?;
    if target.is_dir() {
        return Ok(target);
    }
    std::fs::create_dir_all(versions).map_err(disk)?;
    let path = versions.join(format!(
        ".tmp-zetwerk-{}-{}-{}",
        spec.version,
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&path).map_err(disk)?;
    let mut beside = Beside { path, kept: false };
    unpack(archive, &beside.path, stop, create)?;
    check(&beside.path, spec)?;
    if stop.load(Ordering::Acquire) {
        return Err(FetchError::Stopped);
    }
    match std::fs::rename(&beside.path, &target) {
        Ok(()) => {
            beside.kept = true;
            Ok(target)
        }
        // Another fetch of it was quicker.
        Err(_) if target.is_dir() => Ok(target),
        Err(err) => Err(disk(err)),
    }
}

/// Whether what was unpacked in `dir` is the package `spec`: its
/// manifest says so, and its entry point is there.
fn check(dir: &Path, spec: &PackageSpec) -> Result<(), FetchError> {
    let manifest = manifest(dir).map_err(FetchError::Archive)?;
    let package = &manifest.package;
    if package.name != spec.name || package.version != spec.version {
        return Err(FetchError::Archive(format!(
            "it holds {} {}, not {}",
            package.name,
            package.version,
            said(spec)
        )));
    }
    let entry = inside(Path::new(package.entrypoint.as_str()));
    if !entry.is_some_and(|entry| dir.join(entry).is_file()) {
        return Err(FetchError::Archive(format!(
            "its entry point {} is not in it",
            package.entrypoint
        )));
    }
    Ok(())
}

/// Fetches `spec` from the registry at `base` (always [`REGISTRY`] but
/// in tests) into `cache`. Returns where it is now.
pub fn fetch(
    base: &str,
    spec: &PackageSpec,
    cache: &Path,
    progress: &Progress,
) -> Result<PathBuf, FetchError> {
    let url = archive_url(base, spec).ok_or(FetchError::NotFetchable)?;
    let archive = download(&url, progress, ARCHIVE_LARGEST)?;
    install(cache, spec, &archive, &progress.stop, &mut create_file)
}

/// Fetches `spec`, and the packages of the registry it needs that are
/// not on the machine, and those they need, as far as [`NEEDS_MOST`]
/// go. `now` is told of each before it is asked for. Returns what was
/// fetched, `spec` first; or what could not be, and why.
pub fn fetch_with_needs(
    base: &str,
    spec: &PackageSpec,
    dirs: &PackageDirs,
    progress: &Progress,
    now: &dyn Fn(&PackageSpec),
) -> Result<Vec<PackageSpec>, (PackageSpec, FetchError)> {
    let cache = dirs.cache.as_ref().ok_or_else(|| {
        (
            spec.clone(),
            FetchError::Disk("there is no cache directory".to_owned()),
        )
    })?;
    let mut fetched: Vec<PackageSpec> = Vec::new();
    let mut waiting = vec![spec.clone()];
    while let Some(next) = waiting.pop() {
        if fetched.len() >= NEEDS_MOST || find(dirs, &next).is_some() {
            continue;
        }
        now(&next);
        let dir = fetch(base, &next, cache, progress).map_err(|why| (next.clone(), why))?;
        fetched.push(next);
        for needed in needs(&dir) {
            let missing = needed.namespace == NAMESPACE && find(dirs, &needed).is_none();
            if missing && !fetched.contains(&needed) && !waiting.contains(&needed) {
                waiting.push(needed);
            }
        }
    }
    Ok(fetched)
}

/// A package as the registry lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub name: String,
    pub version: PackageVersion,
    pub description: String,
    pub keywords: Vec<String>,
}

/// The registry's list of packages, and when it was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Index {
    pub listed: Vec<Listed>,
    pub when: SystemTime,
}

/// Reads the registry's list: every package that this version of Typst
/// can read the entry of; the others are left out.
pub fn parse_index(bytes: &[u8]) -> Result<Vec<Listed>, String> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|err| format!("the list cannot be read: {err}"))?;
    let entries = value.as_array().ok_or("the list is no list")?;
    let text = |entry: &serde_json::Value, field: &str| {
        entry
            .get(field)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    Ok(entries
        .iter()
        .filter_map(|entry| {
            let name = text(entry, "name")?;
            let version = text(entry, "version")?.parse().ok()?;
            let keywords = entry
                .get("keywords")
                .and_then(serde_json::Value::as_array)
                .map(|words| {
                    words
                        .iter()
                        .filter_map(|word| word.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            Some(Listed {
                name,
                version,
                description: text(entry, "description").unwrap_or_default(),
                keywords,
            })
        })
        .collect())
}

/// The list as it was last asked for and kept at `path`, if it is.
#[must_use]
pub fn read_index(path: &Path) -> Option<Index> {
    let when = std::fs::metadata(path).ok()?.modified().ok()?;
    let bytes = std::fs::read(path).ok()?;
    Some(Index {
        listed: parse_index(&bytes).ok()?,
        when,
    })
}

/// Asks the registry at `base` for its list, and keeps it at `path`.
pub fn fetch_index(
    base: &str,
    path: Option<&Path>,
    progress: &Progress,
) -> Result<Index, FetchError> {
    let bytes = download(&index_url(base), progress, INDEX_LARGEST)?;
    let listed = parse_index(&bytes).map_err(FetchError::Network)?;
    if let Some(path) = path {
        // Kept whole or not at all; not keeping it takes nothing from
        // the answer.
        let beside = path.with_extension(format!("tmp-{}", std::process::id()));
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&beside, &bytes))
            .and_then(|()| std::fs::rename(&beside, path));
        if written.is_err() {
            let _ = std::fs::remove_file(&beside);
        }
    }
    Ok(Index {
        listed,
        when: SystemTime::now(),
    })
}

impl Index {
    /// The newest version of the package `name`.
    #[must_use]
    pub fn latest(&self, name: &str) -> Option<PackageVersion> {
        self.listed
            .iter()
            .filter(|listed| listed.name == name)
            .map(|listed| listed.version)
            .max()
    }

    /// The packages whose name, description or keywords hold all of
    /// `words`, each in its newest version: those whose name holds them
    /// first, then by name.
    #[must_use]
    pub fn search(&self, words: &str) -> Vec<&Listed> {
        let words: Vec<String> = words.split_whitespace().map(str::to_lowercase).collect();
        let mut found: Vec<&Listed> = Vec::new();
        for listed in &self.listed {
            let about = format!(
                "{} {} {}",
                listed.name,
                listed.description,
                listed.keywords.join(" ")
            )
            .to_lowercase();
            if !words.iter().all(|word| about.contains(word.as_str())) {
                continue;
            }
            match found.iter_mut().find(|other| other.name == listed.name) {
                Some(other) if other.version < listed.version => *other = listed,
                Some(_) => {}
                None => found.push(listed),
            }
        }
        let in_name = |listed: &Listed| {
            !words
                .iter()
                .all(|word| listed.name.to_lowercase().contains(word.as_str()))
        };
        found.sort_by(|a, b| (in_name(a), &a.name).cmp(&(in_name(b), &b.name)));
        found
    }
}

/// What starting a document from a template made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Copied {
    /// The text of the file a document of it begins with.
    pub text: String,
    /// The files made beside the document, and those left as they were
    /// because a file of that name was there.
    pub made: Vec<PathBuf>,
    pub kept: Vec<PathBuf>,
}

/// Copies the template of the package in `package` into `into`: all its
/// files but the one a document begins with, whose text is given back.
/// No file that is there is written over; links are not followed.
pub fn copy_template(package: &Path, into: &Path) -> Result<Copied, String> {
    let template = manifest(package)?
        .template
        .ok_or("this package is no template")?;
    let unsafe_path = || "its template names a place outside the package".to_owned();
    let from = package.join(inside(Path::new(template.path.as_str())).ok_or_else(unsafe_path)?);
    let entry = inside(Path::new(template.entrypoint.as_str())).ok_or_else(unsafe_path)?;
    let real = from
        .canonicalize()
        .map_err(|err| format!("its template is not there: {err}"))?;
    let package_real = package.canonicalize().map_err(|err| err.to_string())?;
    if !real.starts_with(&package_real) {
        return Err(unsafe_path());
    }
    let mut copied = Copied {
        text: String::new(),
        made: Vec::new(),
        kept: Vec::new(),
    };
    let mut entry_found = false;
    let mut directories = vec![PathBuf::new()];
    let mut seen = 0;
    while let Some(within) = directories.pop() {
        let entries = std::fs::read_dir(from.join(&within)).map_err(|err| err.to_string())?;
        for file in entries.flatten() {
            seen += 1;
            if seen > FILES_MOST * 4 {
                return Err("its template has too many files".to_owned());
            }
            let Ok(kind) = file.file_type() else {
                continue;
            };
            let name = within.join(file.file_name());
            if kind.is_dir() {
                directories.push(name);
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            if name == entry {
                copied.text = read_small(&file.path()).map_err(|err| err.to_string())?;
                entry_found = true;
                continue;
            }
            let target = into.join(&name);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
            }
            let made = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target);
            match made {
                Ok(mut made) => {
                    let mut source =
                        std::fs::File::open(file.path()).map_err(|err| err.to_string())?;
                    io::copy(&mut source, &mut made)
                        .map_err(|err| format!("cannot write {}: {err}", target.display()))?;
                    copied.made.push(target);
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => copied.kept.push(target),
                Err(err) => return Err(format!("cannot make {}: {err}", target.display())),
            }
        }
    }
    if !entry_found {
        return Err(format!("its template has no {}", template.entrypoint));
    }
    copied.made.sort();
    copied.kept.sort();
    Ok(copied)
}

#[cfg(test)]
#[path = "packages/tests.rs"]
mod tests;
