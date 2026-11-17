//! Typst packages, without a terminal.
//!
//! A document that imports a package this machine does not have is
//! refused by Typst, and `:why` says so in a writer's words: "this
//! document uses touying 0.8.0, which is not on this machine". The panel
//! offers `g` to fetch it, and the status line says how the fetch goes;
//! Ctrl-C abandons it. Once it is there, the document is typeset again
//! by itself. The packages it needs come along (see
//! [`zetwerk_typst::packages::fetch_with_needs`]).
//!
//! - **Nothing is fetched unless the writer says so**: with `g` in the
//!   panel, a command (`:package get`, `:package add`, `:package use`,
//!   `:package updates`, `:package search`, `:package index`), or the
//!   standing word of the settings, `[packages] download = "always"`,
//!   which fetches what a document lacks as soon as Typst says it does.
//!   `"never"` turns all of it off; `"ask"`, the default, is the key or
//!   the command each time. What goes over the network is a request to
//!   `packages.typst.org` for a package by name and version, or for its
//!   list of packages; nothing of the document.
//! - **Agents never fetch.** Keys an agent sends (`send_keys`) cannot
//!   start a fetch, and a package an agent's change imports is not
//!   fetched by the standing word either: the writer is asked as for any
//!   other (see [`Viewer::keys_of_agent`], [`Viewer::packages_by_agent`]).
//! - **One fetch at a time**, on a thread of its own, which wakes the
//!   window now and then to say how far it came. Without a window it is
//!   done on the spot.
//! - `:packages` lists the packages the document imports (in all of its
//!   files), whether each is on this machine and where, and the other
//!   packages that are; Enter on one says what it is (its `typst.toml`,
//!   the first lines of its README, what of it the document uses) and
//!   offers to fetch it or to remove a fetched version (asked first, and
//!   only among those fetched: the writer's own, `@local` among them, are
//!   never touched). None of that asks the network.
//! - `:package updates` asks for the registry's list (kept, with its
//!   date, and asked for again after [`INDEX_FRESH`]) and offers the
//!   newer versions of the packages used: choosing one changes the import
//!   in one undo step and fetches it; the old version stays. `:package
//!   search <words>` lists what the registry has; choosing one writes its
//!   `#import` among the document's imports and fetches it.
//! - Packages that are templates are offered by `:new` in an empty
//!   document (see `start.rs`): their files are copied beside it, none
//!   written over, and the document begins with the template's text.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use typst::syntax::SyntaxNode;
use typst::syntax::package::{PackageSpec, PackageVersion};
use zetwerk_typst::packages::{
    self as typst_packages, FetchError, Index, NAMESPACE, PackageDirs, Place, Progress, REGISTRY,
    said,
};

use super::assist::Target;
use super::{Edited, Mode, Viewer, editor};
use crate::keys::{Key, KeyPress};

/// How long the registry's list is taken as it was kept before it is
/// asked for again.
pub const INDEX_FRESH: Duration = Duration::from_secs(24 * 60 * 60);
/// How many lines of what a package says of itself the list shows.
const ABOUT_LINES: usize = 4;
/// How many packages a search lists.
const FOUND_MOST: usize = 60;

/// Whether packages a document lacks are fetched.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Download {
    /// When the writer says so, each time: a key or a command.
    #[default]
    Ask,
    /// As soon as Typst says a document lacks one, but one an agent
    /// imported.
    Always,
    /// Never: nothing is asked of the network.
    Never,
}

/// What the user's settings say of packages.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Settings {
    pub download: Download,
    /// Where they are kept: where Typst's own program keeps them.
    pub dirs: PackageDirs,
}

/// What one fetch is to bring.
enum Job {
    Packages(Vec<PackageSpec>),
    Index,
}

/// What it brought.
enum Done {
    Packages(Result<Vec<PackageSpec>, (PackageSpec, FetchError)>),
    Index(Result<Index, FetchError>),
}

/// What is done once the registry's list is there.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Then {
    Nothing,
    Updates,
    Search(String),
    Add(String),
    Get(String),
}

/// The fetch under way.
struct Fetch {
    /// What it is said to fetch.
    what: String,
    /// The package coming now.
    now: Arc<Mutex<String>>,
    progress: Arc<Progress>,
    done: Receiver<Done>,
    then: Then,
}

/// What the viewer keeps of packages.
#[derive(Default)]
pub(super) struct Packages {
    download: Download,
    /// The registry asked, where it is not [`REGISTRY`]: a stand-in, in
    /// tests.
    registry: Option<String>,
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
    fetch: Option<Fetch>,
    /// Keys an agent sent are being pressed.
    by_agent: bool,
    /// Packages not on the machine that an agent's change imported.
    agents: HashSet<PackageSpec>,
    /// Packages fetched by the standing word already, or tried: once.
    tried: HashSet<PackageSpec>,
    index: Option<Index>,
    /// What the status line said of the fetch last.
    said: Option<String>,
}

/// The package Typst says it did not find, in `message`.
pub(super) fn missing_spec(message: &str) -> Option<PackageSpec> {
    let (_, rest) = message.split_once("package not found (searched for ")?;
    rest.split(')').next()?.parse().ok()
}

/// What `:why` says of a package that is not found.
pub(super) fn missing_said(message: &str) -> String {
    match missing_spec(message) {
        Some(spec) => format!(
            "this document uses {}, which is not on this machine",
            said(&spec)
        ),
        None => format!("{message} (:packages says which are on this machine)"),
    }
}

/// `path`, from the home directory where it is in it.
fn home(path: &Path) -> String {
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty());
    match home.and_then(|home| path.strip_prefix(home).ok().map(Path::to_owned)) {
        Some(within) => format!("~/{}", within.display()),
        None => path.display().to_string(),
    }
}

/// How much `bytes` is, as people say it.
fn amount(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{} KiB", bytes / 1024),
        _ => format!("{:.1} MiB", bytes as f64 / 1_048_576.0),
    }
}

/// Packages named one after the other: `a`, `a and b`, `a, b and c`.
fn listed(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// What a fetch does, on its thread or on the spot.
fn run(
    base: &str,
    job: &Job,
    dirs: &PackageDirs,
    progress: &Progress,
    now: &Mutex<String>,
) -> Done {
    match job {
        Job::Packages(specs) => {
            let told = |spec: &PackageSpec| {
                *now.lock().unwrap_or_else(PoisonError::into_inner) = said(spec);
            };
            let mut all = Vec::new();
            for spec in specs {
                match typst_packages::fetch_with_needs(base, spec, dirs, progress, &told) {
                    Ok(fetched) => all.extend(fetched),
                    Err(failed) => return Done::Packages(Err(failed)),
                }
            }
            Done::Packages(Ok(all))
        }
        Job::Index => Done::Index(typst_packages::fetch_index(
            base,
            dirs.index.as_deref(),
            progress,
        )),
    }
}

/// A package the document imports, and where.
struct Used {
    spec: PackageSpec,
    /// The files that import it, by their names in the project.
    files: Vec<String>,
    /// What of it they name.
    uses: BTreeSet<String>,
}

impl Viewer {
    /// Takes what the settings say of packages.
    pub(super) fn set_packages(&mut self, settings: &Settings) {
        self.packages.download = settings.download;
    }

    /// Has fetches done on a thread of their own from now on; `wake` is
    /// called from it when one is over and now and then while it comes.
    /// Whoever it wakes calls [`Viewer::packages_progress`].
    pub fn packages_in_background(&mut self, wake: impl Fn() + Send + Sync + 'static) {
        self.packages.wake = Some(Arc::new(wake));
    }

    /// Has fetches ask `url` in place of the registry: a stand-in.
    #[cfg(test)]
    pub(super) fn packages_from(&mut self, url: &str) {
        self.packages.registry = Some(url.to_owned());
    }

    /// The keys an agent sends: what they would fetch is refused, and
    /// what they import is the writer's to fetch.
    pub fn keys_of_agent(&mut self, keys: &[KeyPress]) {
        self.packages.by_agent = true;
        for key in keys {
            self.key(*key);
        }
        self.packages.by_agent = false;
        self.packages_by_agent();
    }

    /// An agent changed the text: the packages it imports that are not
    /// on the machine are not fetched by the standing word.
    pub(super) fn packages_by_agent(&mut self) {
        let Some(editor) = &self.editor else {
            return;
        };
        let dirs = self.project.packages();
        let missing = typst_packages::imports(editor.source().root())
            .into_iter()
            .map(|import| import.spec)
            .filter(|spec| typst_packages::find(dirs, spec).is_none());
        self.packages.agents.extend(missing);
    }

    /// The packages Typst said it did not find, in the last compile, in
    /// the order it said them.
    pub(super) fn missing(&self) -> Vec<PackageSpec> {
        let mut found: Vec<PackageSpec> = Vec::new();
        let said = self
            .diagnostics
            .iter()
            .filter(|(_, _, error, _)| *error)
            .map(|(.., message)| message.as_str())
            .chain(self.problems.iter().map(String::as_str));
        for spec in said.filter_map(missing_spec) {
            if !found.contains(&spec) {
                found.push(spec);
            }
        }
        found
    }

    /// Of those, the ones that can be fetched: of the registry, and not
    /// there by now.
    fn missing_fetchable(&self) -> Vec<PackageSpec> {
        let dirs = self.project.packages();
        self.missing()
            .into_iter()
            .filter(|spec| spec.namespace == NAMESPACE)
            .filter(|spec| typst_packages::find(dirs, spec).is_none())
            .collect()
    }

    /// The lines of the panel of `:why` about the packages the document
    /// lacks: what `g` fetches, or why nothing does.
    pub(super) fn package_lines(&self) -> Vec<String> {
        let missing = self.missing();
        if missing.is_empty() {
            return Vec::new();
        }
        if let Some(fetch) = &self.packages.fetch {
            return vec![format!(
                "       fetching {} … (Ctrl-C abandons)",
                fetch.what
            )];
        }
        let mut lines = Vec::new();
        let dirs = self.project.packages();
        for own in missing.iter().filter(|spec| spec.namespace != NAMESPACE) {
            let place = dirs.data.as_ref().map_or_else(
                || "the directory of your own packages".to_owned(),
                |data| {
                    let dir = data.join(own.namespace.as_str()).join(own.name.as_str());
                    home(&dir.join(own.version.to_string()))
                },
            );
            lines.push(format!(
                "       {} is one of your own: Typst looks for it in {place}",
                said(own)
            ));
        }
        let fetchable = self.missing_fetchable();
        if fetchable.is_empty() {
            return lines;
        }
        let names: Vec<String> = fetchable.iter().map(said).collect();
        if self.packages.download == Download::Never {
            lines.push(
                "       fetching is off ([packages] download = \"never\" in the settings)"
                    .to_owned(),
            );
            return lines;
        }
        let them = if names.len() == 1 { "it" } else { "them" };
        lines.push(format!(
            "g      fetch {} from packages.typst.org",
            listed(&names)
        ));
        lines.push(format!(
            "       (asks for {them} by name and version; nothing of the document is sent)"
        ));
        if fetchable
            .iter()
            .any(|spec| self.packages.agents.contains(spec))
        {
            lines.push(
                "       an agent wrote this import: it is fetched only if you say so".to_owned(),
            );
        }
        lines
    }

    /// Whether `g` in the panel of `:why` fetches something.
    pub(super) fn package_offered(&self) -> bool {
        self.packages.download != Download::Never
            && self.packages.fetch.is_none()
            && !self.missing_fetchable().is_empty()
    }

    /// `g` in the panel: fetches what the document lacks.
    pub(super) fn fetch_missing(&mut self) {
        let missing = self.missing_fetchable();
        if self.begin_fetch(Job::Packages(missing), Then::Nothing) {
            self.edited = Some(Edited::Command);
        }
    }

    /// A key that is for packages: Ctrl-C while a fetch is under way
    /// abandons it. Returns whether the key was taken.
    pub(super) fn packages_key(&mut self, press: KeyPress) -> bool {
        if press.key != Key::Char('c') || !press.ctrl || press.alt || self.packages.fetch.is_none()
        {
            return false;
        }
        let normal = match &self.editor {
            Some(editor) => editor.mode() == editor::Mode::Normal,
            None => self.mode == Mode::Normal,
        };
        if !normal {
            return false;
        }
        if let Some(fetch) = self.packages.fetch.take() {
            fetch.progress.stop();
            self.packages.said = None;
            self.notice(format!(
                "fetching {} abandoned: nothing half fetched is kept",
                fetch.what
            ));
        }
        true
    }

    /// What the status line says of the fetch under way.
    fn fetch_words(&self) -> Option<String> {
        let fetch = self.packages.fetch.as_ref()?;
        let now = fetch
            .now
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let now = if now.is_empty() {
            fetch.what.clone()
        } else {
            now
        };
        let came = match fetch.progress.got() {
            (0, None) => "asking packages.typst.org".to_owned(),
            (got, Some(total)) if total > 0 => format!(
                "{} of {} ({} %)",
                amount(got),
                amount(total),
                got * 100 / total
            ),
            (got, _) => amount(got),
        };
        Some(format!("fetching {now}: {came} · Ctrl-C abandons"))
    }

    /// Takes up what the fetch under way has to say, and starts one that
    /// the standing word of the settings wants. Returns whether the
    /// document is to be typeset again now. Costs nothing where nothing
    /// is fetched; the window calls it whenever it was woken.
    pub fn packages_progress(&mut self) -> bool {
        let said = self
            .packages
            .fetch
            .as_ref()
            .map(|fetch| fetch.done.try_recv());
        let compile = match said {
            None => false,
            Some(Ok(done)) => {
                let then = self
                    .packages
                    .fetch
                    .take()
                    .map_or(Then::Nothing, |fetch| fetch.then);
                self.packages.said = None;
                self.fetched(done, then)
            }
            Some(Err(TryRecvError::Empty)) => {
                let words = self.fetch_words();
                if words != self.packages.said {
                    self.packages.said.clone_from(&words);
                    if let Some(words) = words {
                        self.notice(words);
                    }
                }
                false
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.packages.fetch = None;
                self.packages.said = None;
                self.alert("the fetch ended without a word: nothing was fetched");
                false
            }
        };
        compile || self.fetch_by_standing()
    }

    /// With `download = "always"`: fetches what the document lacks, once
    /// for each package, but what an agent imported. Returns whether the
    /// document is to be typeset again (where it was done on the spot).
    fn fetch_by_standing(&mut self) -> bool {
        let at_rest = self.packages.download == Download::Always
            && self.packages.fetch.is_none()
            && self.compiled_revision == Some(self.revision)
            && !self.problems.is_empty();
        if !at_rest {
            return false;
        }
        let wanted: Vec<PackageSpec> = self
            .missing_fetchable()
            .into_iter()
            .filter(|spec| !self.packages.tried.contains(spec))
            .filter(|spec| !self.packages.agents.contains(spec))
            .collect();
        if wanted.is_empty() {
            return false;
        }
        self.packages.tried.extend(wanted.iter().cloned());
        self.begin_fetch(Job::Packages(wanted), Then::Nothing)
    }

    /// Starts fetching `job`, and does `then` once it is done. Without a
    /// window it is done on the spot. Returns whether the document is to
    /// be typeset again now.
    fn begin_fetch(&mut self, job: Job, then: Then) -> bool {
        if self.packages.by_agent {
            self.alert("an agent cannot fetch packages: the writer is asked (:why, :package get)");
            return false;
        }
        if self.packages.download == Download::Never {
            self.alert(
                "nothing is fetched: [packages] download = \"never\" in the settings \
                 (\"ask\" fetches when you say so)",
            );
            return false;
        }
        if let Some(fetch) = &self.packages.fetch {
            let told = format!(
                "{} is being fetched: one fetch at a time (Ctrl-C abandons it)",
                fetch.what
            );
            self.alert(told);
            return false;
        }
        let what = match &job {
            Job::Packages(specs) if specs.is_empty() => {
                self.notice("every package this document uses is on this machine");
                return false;
            }
            Job::Packages(specs) => listed(&specs.iter().map(said).collect::<Vec<_>>()),
            Job::Index => "the list of packages".to_owned(),
        };
        let base = self
            .packages
            .registry
            .clone()
            .unwrap_or_else(|| REGISTRY.to_owned());
        let dirs = self.project.packages().clone();
        let now = Arc::new(Mutex::new(String::new()));
        let wake = self.packages.wake.clone();
        let progress = Arc::new(match &wake {
            Some(wake) => {
                let wake = Arc::clone(wake);
                Progress::new(move || wake())
            }
            None => Progress::default(),
        });
        let work = {
            let (now, progress) = (Arc::clone(&now), Arc::clone(&progress));
            move || run(&base, &job, &dirs, &progress, &now)
        };
        let Some(wake) = wake else {
            return self.fetched(work(), then);
        };
        let (send, done) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("zetwerk-packages".into())
            .spawn(move || {
                let _ = send.send(work());
                wake();
            });
        if let Err(err) = thread {
            self.alert(format!("nothing is fetched: no thread for it: {err}"));
            return false;
        }
        self.notice(format!(
            "fetching {what} from packages.typst.org … (Ctrl-C abandons)"
        ));
        self.packages.fetch = Some(Fetch {
            what,
            now,
            progress,
            done,
            then,
        });
        false
    }

    /// Says what a fetch brought, and goes on with what it was for.
    /// Returns whether the document is to be typeset again now.
    fn fetched(&mut self, done: Done, then: Then) -> bool {
        match done {
            Done::Packages(Ok(fetched)) => {
                self.project.packages_changed();
                self.revision += 1;
                let names: Vec<String> = fetched.iter().map(said).collect();
                let place = self
                    .project
                    .packages()
                    .cache
                    .as_deref()
                    .map(home)
                    .unwrap_or_default();
                let told = match names.as_slice() {
                    [] => "it is on this machine already".to_owned(),
                    [one] => format!("{one} fetched into {place}"),
                    [first, needed @ ..] => format!(
                        "{first} fetched into {place}, with {} that it needs",
                        listed(needed)
                    ),
                };
                self.notice(format!("{told}: typesetting again"));
                true
            }
            Done::Packages(Err((spec, why))) => {
                // What it needed and got is there all the same.
                self.project.packages_changed();
                self.alert(format!("{} not fetched: {why}", said(&spec)));
                false
            }
            Done::Index(Ok(index)) => {
                self.packages.index = Some(index);
                self.with_index_now(then)
            }
            Done::Index(Err(why)) => {
                self.alert(format!("the list of packages was not fetched: {why}"));
                false
            }
        }
    }

    /// Does `then` with the registry's list: the one kept, where it is
    /// fresh, else one asked for now; with fetching off, whatever is
    /// kept. Returns whether the document is to be typeset again.
    fn with_index(&mut self, then: Then) -> bool {
        let fresh = |index: &Index| {
            SystemTime::now()
                .duration_since(index.when)
                .is_ok_and(|age| age < INDEX_FRESH)
        };
        if self
            .packages
            .index
            .as_ref()
            .is_none_or(|index| !fresh(index))
            && let Some(kept) = self
                .project
                .packages()
                .index
                .as_deref()
                .and_then(typst_packages::read_index)
        {
            self.packages.index = Some(kept);
        }
        let usable = match &self.packages.index {
            Some(index) => fresh(index) || self.packages.download == Download::Never,
            None => false,
        };
        if usable && then != Then::Nothing {
            return self.with_index_now(then);
        }
        self.begin_fetch(Job::Index, then)
    }

    /// How old the list is, as it is said.
    fn index_age(&self) -> String {
        self.packages
            .index
            .as_ref()
            .map_or_else(String::new, |index| {
                super::start::ago(index.when, SystemTime::now())
            })
    }

    /// Does `then`, the list being there.
    fn with_index_now(&mut self, then: Then) -> bool {
        match then {
            Then::Nothing => {
                let count = self
                    .packages
                    .index
                    .as_ref()
                    .map_or(0, |index| index.listed.len());
                self.notice(format!(
                    "packages.typst.org lists {count} packages (asked {})",
                    self.index_age()
                ));
                false
            }
            Then::Updates => {
                self.show_updates();
                false
            }
            Then::Search(words) => {
                self.show_found(&words);
                false
            }
            Then::Add(name) => self.add_package(&name),
            Then::Get(name) => self.get_named(&name),
        }
    }

    /// `:packages` and `:package …`. Returns whether `command` was one.
    pub(super) fn packages_command(&mut self, command: &str) -> bool {
        let (name, rest) = command.split_once(' ').unwrap_or((command, ""));
        if name == "packages" && rest.trim().is_empty() {
            self.open_packages();
            return true;
        }
        if name != "package" {
            return false;
        }
        let (what, argument) = rest.trim().split_once(' ').unwrap_or((rest.trim(), ""));
        let argument = argument.trim();
        let compile = match what {
            "" | "list" => {
                self.open_packages();
                false
            }
            "get" | "fetch" => self.get_named(argument),
            "about" => {
                self.about_package(argument);
                false
            }
            "remove" => {
                self.ask_remove(argument);
                false
            }
            "remove!" => self.remove_package(argument),
            "updates" => self.with_index(Then::Updates),
            "search" if !argument.is_empty() => self.with_index(Then::Search(argument.to_owned())),
            "add" if !argument.is_empty() => self.add_or_index(argument),
            "use" => self.use_version(argument),
            "index" => self.begin_fetch(Job::Index, Then::Nothing),
            _ => {
                self.alert(
                    ":package get [name[:version]], about, remove, updates, search <words>, \
                     add <name>, use <name:version>, index; :packages lists them",
                );
                false
            }
        };
        if compile {
            self.edited = Some(Edited::Command);
        }
        true
    }

    /// The packages the document imports, in all its files: the one
    /// being edited, the others open, and those the last compile read.
    fn used(&self) -> Vec<Used> {
        let root = self.project.root_dir().to_owned();
        let name_of = |path: &Path| {
            path.strip_prefix(&root)
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned()
        };
        let mut texts: Vec<(String, SyntaxNode)> = Vec::new();
        let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
        for editor in self.editor.iter().chain(&self.others) {
            let path =
                std::path::absolute(editor.path()).unwrap_or_else(|_| editor.path().to_owned());
            seen.insert(path.clone());
            texts.push((name_of(&path), editor.source().root().clone()));
        }
        for path in self.project.dependencies() {
            let typst_file = path.extension().is_some_and(|ending| ending == "typ");
            if !typst_file || seen.contains(&path) {
                continue;
            }
            let small = std::fs::metadata(&path).is_ok_and(|meta| meta.len() <= 1 << 20);
            if let Some(text) = small.then(|| std::fs::read_to_string(&path).ok()).flatten() {
                texts.push((name_of(&path), typst::syntax::parse(&text)));
            }
        }
        let dirs = self.project.packages();
        let mut found: BTreeMap<String, Used> = BTreeMap::new();
        for (file, root) in &texts {
            for import in typst_packages::imports(root) {
                let exports = match typst_packages::find(dirs, &import.spec) {
                    Some((dir, _)) => typst_packages::about(&dir)
                        .map(|about| typst_packages::exports(&dir, &about.entrypoint))
                        .unwrap_or_default(),
                    None => BTreeSet::new(),
                };
                let uses = typst_packages::used(root, &import, &exports);
                let used = found
                    .entry(import.spec.to_string())
                    .or_insert_with(|| Used {
                        spec: import.spec.clone(),
                        files: Vec::new(),
                        uses: BTreeSet::new(),
                    });
                if !used.files.contains(file) {
                    used.files.push(file.clone());
                }
                used.uses.extend(uses);
            }
        }
        found.into_values().collect()
    }

    /// `:packages`: those the document uses and those on the machine.
    fn open_packages(&mut self) {
        let dirs = self.project.packages().clone();
        let used = self.used();
        let mut items: Vec<(String, Target)> = Vec::new();
        for used in &used {
            let place = match typst_packages::find(&dirs, &used.spec) {
                Some((_, place)) => place.said().to_owned(),
                None => "not on this machine".to_owned(),
            };
            items.push((
                format!("{}\tused here · {place}", said(&used.spec)),
                Target::Command(format!("package about {}", used.spec)),
            ));
        }
        for kept in typst_packages::on_machine(&dirs) {
            if used.iter().any(|used| used.spec == kept.spec) {
                continue;
            }
            let template =
                typst_packages::about(&kept.dir).is_ok_and(|about| about.template.is_some());
            let template = if template { " · a template" } else { "" };
            items.push((
                format!("{}\t{}{template}", said(&kept.spec), kept.place.said()),
                Target::Command(format!("package about {}", kept.spec)),
            ));
        }
        items.push((
            "newer versions of these…\tasks packages.typst.org".to_owned(),
            Target::Command("package updates".to_owned()),
        ));
        items.push((
            "find a package…\tasks packages.typst.org".to_owned(),
            Target::Ask("package search ".to_owned()),
        ));
        self.open_picker("packages", items, 0);
    }

    /// `:package about <spec>`: what it is, and what can be done with it.
    fn about_package(&mut self, argument: &str) {
        let spec = match typst_packages::parse_spec(argument) {
            Ok(spec) => spec,
            Err(why) => return self.alert(format!("no package {argument}: {why}")),
        };
        let dirs = self.project.packages().clone();
        let found = typst_packages::find(&dirs, &spec);
        let mut lines: Vec<String> = Vec::new();
        let mut actions: Vec<(String, Target)> = Vec::new();
        match &found {
            Some((dir, place)) => {
                match typst_packages::about(dir) {
                    Ok(about) => {
                        lines.push(format!(
                            "{}: {}",
                            said(&spec),
                            about
                                .description
                                .as_deref()
                                .unwrap_or("it says nothing of itself")
                        ));
                        let mut by = Vec::new();
                        if !about.authors.is_empty() {
                            by.push(format!("by {}", about.authors.join(", ")));
                        }
                        by.extend(about.license.clone());
                        by.push(format!("begins at {}", about.entrypoint));
                        lines.push(by.join(" · "));
                        lines.extend(
                            about
                                .readme
                                .iter()
                                .take(ABOUT_LINES)
                                .map(|line| format!("“{line}”")),
                        );
                        if about.template.is_some() {
                            lines.push(
                                "a template: :new in an empty document begins with it".to_owned(),
                            );
                        }
                        lines.extend(about.homepage);
                    }
                    Err(why) => lines.push(format!("{}: {why}", said(&spec))),
                }
                lines.push(format!("{} · in {}", place.said(), home(dir)));
                if *place == Place::Fetched {
                    actions.push((
                        "remove it from this machine…\tasked first".to_owned(),
                        Target::Command(format!("package remove {spec}")),
                    ));
                }
            }
            None => {
                lines.push(format!("{}: not on this machine", said(&spec)));
                if spec.namespace == NAMESPACE {
                    actions.push((
                        format!("fetch it\tasks packages.typst.org for {}", said(&spec)),
                        Target::Command(format!("package get {spec}")),
                    ));
                }
            }
        }
        match self.used().into_iter().find(|used| used.spec == spec) {
            Some(used) if used.uses.is_empty() => {
                lines.push(format!("imported in {}", used.files.join(", ")));
            }
            Some(used) => {
                let names: Vec<String> = used.uses.into_iter().collect();
                lines.push(format!(
                    "the document uses {} (in {})",
                    names.join(", "),
                    used.files.join(", ")
                ));
            }
            None => lines.push("this document does not import it".to_owned()),
        }
        let first = lines.len();
        let mut items: Vec<(String, Target)> = lines
            .into_iter()
            .map(|line| (line, Target::Nothing))
            .collect();
        items.extend(actions);
        let chosen = if items.len() > first { first } else { 0 };
        self.open_picker_for("package", items, chosen, "packages".to_owned(), None);
    }

    /// `:package remove <spec>`: asks first.
    fn ask_remove(&mut self, argument: &str) {
        let spec = match typst_packages::parse_spec(argument) {
            Ok(spec) => spec,
            Err(why) => return self.alert(format!("no package {argument}: {why}")),
        };
        let dirs = self.project.packages();
        let Some((dir, Place::Fetched)) = typst_packages::find(dirs, &spec) else {
            return self.alert(format!(
                "{} is not among the fetched packages: only those are removed",
                said(&spec)
            ));
        };
        let used = self.used().iter().any(|used| used.spec == spec);
        let after = if used {
            "this document cannot be typeset without it until it is fetched again"
        } else {
            "it can be fetched again"
        };
        let items = vec![
            (
                "no, keep it".to_owned(),
                Target::Command(format!("package about {spec}")),
            ),
            (
                format!("yes, remove {} ({})\t{after}", said(&spec), home(&dir)),
                Target::Command(format!("package remove! {spec}")),
            ),
        ];
        self.open_picker_for("remove?", items, 0, format!("package about {spec}"), None);
    }

    /// `:package remove! <spec>`: removes a fetched version. Returns
    /// whether the document is to be typeset again.
    fn remove_package(&mut self, argument: &str) -> bool {
        let spec = match typst_packages::parse_spec(argument) {
            Ok(spec) => spec,
            Err(why) => {
                self.alert(format!("no package {argument}: {why}"));
                return false;
            }
        };
        match typst_packages::remove(self.project.packages(), &spec) {
            Ok(dir) => {
                self.project.packages_changed();
                self.revision += 1;
                self.notice(format!(
                    "{} removed from {} (:package get {spec} fetches it again)",
                    said(&spec),
                    home(&dir)
                ));
                true
            }
            Err(why) => {
                self.alert(format!("not removed: {why}"));
                false
            }
        }
    }

    /// `:package get [name[:version]]`: what the document lacks, or the
    /// package named: in the version the document imports, else the
    /// newest. Returns whether the document is to be typeset again.
    fn get_named(&mut self, argument: &str) -> bool {
        if argument.is_empty() {
            let dirs = self.project.packages().clone();
            let mut wanted = self.missing_fetchable();
            for used in self.used() {
                let fetchable = used.spec.namespace == NAMESPACE;
                if fetchable
                    && typst_packages::find(&dirs, &used.spec).is_none()
                    && !wanted.contains(&used.spec)
                {
                    wanted.push(used.spec);
                }
            }
            return self.begin_fetch(Job::Packages(wanted), Then::Nothing);
        }
        let spec = if argument.contains(':') {
            typst_packages::parse_spec(argument)
        } else {
            let imported = self
                .used()
                .into_iter()
                .map(|used| used.spec)
                .find(|spec| spec.name == argument.trim_start_matches("@preview/"));
            match imported {
                Some(spec) => Ok(spec),
                // The newest, which the list says.
                None => match self.latest(argument) {
                    Some(version) => typst_packages::parse_spec(&format!("{argument}:{version}")),
                    None => return self.with_index(Then::Get(argument.to_owned())),
                },
            }
        };
        match spec {
            Ok(spec) if spec.namespace != NAMESPACE => {
                self.alert(format!(
                    "{} is not in the registry: packages of @{} are your own to put in place",
                    said(&spec),
                    spec.namespace
                ));
                false
            }
            Ok(spec) => {
                if let Some((dir, _)) = typst_packages::find(self.project.packages(), &spec) {
                    self.notice(format!(
                        "{} is on this machine already, in {}",
                        said(&spec),
                        home(&dir)
                    ));
                    return false;
                }
                self.begin_fetch(Job::Packages(vec![spec]), Then::Nothing)
            }
            Err(why) => {
                self.alert(format!("no package {argument}: {why}"));
                false
            }
        }
    }

    /// The newest version of `name` the list kept says there is, where
    /// one is kept and was asked for in this session.
    fn latest(&self, name: &str) -> Option<PackageVersion> {
        self.packages.index.as_ref()?.latest(name)
    }

    /// The newer versions of the packages the document uses.
    fn show_updates(&mut self) {
        let Some(index) = &self.packages.index else {
            return;
        };
        let mut items = vec![(
            format!("as packages.typst.org listed them {}", self.index_age()),
            Target::Nothing,
        )];
        let mut newest = 0;
        for used in self.used() {
            if used.spec.namespace != NAMESPACE {
                continue;
            }
            match index.latest(&used.spec.name) {
                Some(latest) if latest > used.spec.version => items.push((
                    format!(
                        "{} {} → {latest}\tchanges the import and fetches it; {} stays",
                        used.spec.name, used.spec.version, used.spec.version
                    ),
                    Target::Command(format!("package use {}:{latest}", used.spec.name)),
                )),
                _ => newest += 1,
            }
        }
        if items.len() == 1 {
            let age = self.index_age();
            return self.notice(format!(
                "{newest} packages used, each in its newest version (as packages.typst.org listed them {age})"
            ));
        }
        self.open_picker_for("newer versions", items, 1, "packages".to_owned(), None);
    }

    /// What the list has for `words`.
    fn show_found(&mut self, words: &str) {
        let Some(index) = &self.packages.index else {
            return;
        };
        let found = index.search(words);
        if found.is_empty() {
            let age = self.index_age();
            return self.notice(format!(
                "packages.typst.org lists nothing for “{words}” (asked {age})"
            ));
        }
        let mut items = vec![(
            format!(
                "{} for “{words}” (asked {}); Enter writes the import and fetches it",
                found.len(),
                self.index_age()
            ),
            Target::Nothing,
        )];
        items.extend(found.iter().take(FOUND_MOST).map(|listed| {
            (
                format!("{} {}\t{}", listed.name, listed.version, listed.description),
                Target::Command(format!("package add {}:{}", listed.name, listed.version)),
            )
        }));
        self.open_picker_for("found", items, 1, "packages".to_owned(), None);
    }

    /// `:package add <name>[:version]`: with a version, at once; without,
    /// once the list says which is the newest.
    fn add_or_index(&mut self, argument: &str) -> bool {
        if argument.contains(':') || self.latest(argument).is_some() {
            return self.add_package(argument);
        }
        self.with_index(Then::Add(argument.to_owned()))
    }

    /// Writes the import of `argument` (`name` or `name:version`) among
    /// the imports of the file being edited, as one undo step, and
    /// fetches it where it is not on the machine.
    fn add_package(&mut self, argument: &str) -> bool {
        let spec = if argument.contains(':') {
            typst_packages::parse_spec(argument)
        } else {
            match self.latest(argument) {
                Some(version) => typst_packages::parse_spec(&format!("{argument}:{version}")),
                None => Err("packages.typst.org lists no package of that name".to_owned()),
            }
        };
        let spec = match spec {
            Ok(spec) => spec,
            Err(why) => {
                self.alert(format!("no package {argument}: {why}"));
                return false;
            }
        };
        let Some(editor) = &mut self.editor else {
            self.alert("no document is open to import it into");
            return false;
        };
        let text = editor.text();
        let imports = typst_packages::imports(editor.source().root());
        if let Some(there) = imports
            .iter()
            .find(|import| import.spec.namespace == spec.namespace && import.spec.name == spec.name)
        {
            let line = text[..there.literal.start].matches('\n').count() + 1;
            let told = format!(
                "{} is imported already, on line {line} (:package use {}:{} changes the version)",
                there.spec.name, spec.name, spec.version
            );
            self.notice(told);
            return false;
        }
        // Behind the last import that has a line of its own; else at the
        // top.
        let at = imports
            .iter()
            .filter(|import| {
                let start = text[..import.literal.start]
                    .rfind('\n')
                    .map_or(0, |at| at + 1);
                text[start..].starts_with("#import")
            })
            .map(|import| {
                text[import.literal.end..]
                    .find('\n')
                    .map_or(text.len(), |end| import.literal.end + end + 1)
            })
            .max()
            .unwrap_or(0);
        let newline = if at > 0 && !text[..at].ends_with('\n') {
            "\n"
        } else {
            ""
        };
        let line = format!("{newline}#import \"{spec}\"\n");
        if let Err(why) = editor.replace(&[(at..at, line.as_str())]) {
            self.alert(format!("not changed: {why}"));
            return false;
        }
        self.text_changed(Edited::Command);
        let name = spec.name.clone();
        if typst_packages::find(self.project.packages(), &spec).is_some() {
            self.notice(format!(
                "#import \"{spec}\" written: {name}.… names what it has (u takes it back)"
            ));
            return true;
        }
        if self.packages.download == Download::Never {
            self.alert(format!(
                "#import \"{spec}\" written, but fetching is off ([packages] download = \"never\")"
            ));
            return true;
        }
        self.begin_fetch(Job::Packages(vec![spec]), Then::Nothing);
        true
    }

    /// `:package use <name:version>`: makes every import of that package
    /// in the file being edited one of that version, as one undo step,
    /// and fetches it where it is not on the machine. The old version
    /// stays where it is.
    fn use_version(&mut self, argument: &str) -> bool {
        let spec = match typst_packages::parse_spec(argument) {
            Ok(spec) => spec,
            Err(why) => {
                self.alert(format!("no package {argument}: {why}"));
                return false;
            }
        };
        let Some(editor) = &mut self.editor else {
            self.alert("no document is open");
            return false;
        };
        let imports = typst_packages::imports(editor.source().root());
        let old: Vec<_> = imports
            .iter()
            .filter(|import| {
                import.spec.namespace == spec.namespace && import.spec.name == spec.name
            })
            .collect();
        let Some(was) = old.first().map(|import| import.spec.version) else {
            self.alert(format!("this file does not import {}", spec.name));
            return false;
        };
        let literal = format!("\"{spec}\"");
        let edits: Vec<(std::ops::Range<usize>, &str)> = old
            .iter()
            .rev()
            .filter(|import| import.spec != spec)
            .map(|import| (import.literal.clone(), literal.as_str()))
            .collect();
        if edits.is_empty() {
            self.notice(format!("{} is imported already", said(&spec)));
            return false;
        }
        if let Err(why) = editor.replace(&edits) {
            self.alert(format!("not changed: {why}"));
            return false;
        }
        self.text_changed(Edited::Command);
        self.notice(format!(
            "{} {was} → {} in the import (u takes it back; {was} stays on this machine)",
            spec.name, spec.version
        ));
        if typst_packages::find(self.project.packages(), &spec).is_none()
            && self.packages.download != Download::Never
        {
            self.begin_fetch(Job::Packages(vec![spec]), Then::Nothing);
        }
        true
    }

    /// The packages on the machine that are templates, for the list of
    /// starters.
    pub(super) fn package_starters(&self) -> Vec<(String, Target)> {
        typst_packages::on_machine(self.project.packages())
            .into_iter()
            .filter_map(|kept| {
                let about = typst_packages::about(&kept.dir).ok()?;
                about.template.as_ref()?;
                let what = about
                    .description
                    .unwrap_or_else(|| "a package's template".to_owned());
                Some((
                    format!("{}\t{what}", said(&kept.spec)),
                    Target::Command(format!("template {}", kept.spec)),
                ))
            })
            .collect()
    }

    /// `:template @preview/name:version`: begins the empty document with
    /// that package's template: its files beside the document (none
    /// written over), its text in it.
    pub(super) fn begin_from_package(&mut self, which: &str) {
        let spec = match typst_packages::parse_spec(which) {
            Ok(spec) => spec,
            Err(why) => return self.alert(format!("no package {which}: {why}")),
        };
        let Some((dir, _)) = typst_packages::find(self.project.packages(), &spec) else {
            return self.alert(format!(
                "{} is not on this machine: :package get {spec} fetches it",
                said(&spec)
            ));
        };
        let empty = self.text().is_some_and(|text| text.trim().is_empty());
        if !empty {
            // Says why not, and does nothing.
            if let Err(why) = self.fill("", &[]) {
                self.alert(why);
            }
            return;
        }
        let folder = self.folder();
        let copied = match typst_packages::copy_template(&dir, &folder) {
            Ok(copied) => copied,
            Err(why) => return self.alert(format!("not begun from {}: {why}", said(&spec))),
        };
        if let Err(why) = self.fill(&copied.text, &[]) {
            return self.alert(why);
        }
        for path in &copied.made {
            self.file_changed(path);
        }
        let name = |path: &PathBuf| {
            path.strip_prefix(&folder)
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned()
        };
        let mut told = format!("begun from the template of {}", said(&spec));
        if !copied.made.is_empty() {
            let made: Vec<String> = copied.made.iter().map(name).collect();
            told.push_str(&format!("; beside it: {}", listed(&made)));
        }
        if !copied.kept.is_empty() {
            let kept: Vec<String> = copied.kept.iter().map(name).collect();
            told.push_str(&format!(
                "; {} were there and are left as they are",
                listed(&kept)
            ));
        }
        told.push_str(" (u takes the text back)");
        self.notice(told);
    }

    /// For agents (`list_packages`): what the document imports and
    /// whether each is on the machine, and what is on it; nothing is
    /// asked of the network.
    pub fn packages_listed(&self) -> serde_json::Value {
        use serde_json::json;
        let dirs = self.project.packages().clone();
        let used: Vec<serde_json::Value> = self
            .used()
            .into_iter()
            .map(|used| {
                let found = typst_packages::find(&dirs, &used.spec);
                json!({
                    "package": used.spec.to_string(),
                    "on_machine": found.is_some(),
                    "kept": found.map(|(_, place)| match place {
                        Place::Own => "own",
                        Place::Fetched => "fetched",
                    }),
                    "files": used.files,
                    "uses": used.uses,
                })
            })
            .collect();
        let kept: Vec<serde_json::Value> = typst_packages::on_machine(&dirs)
            .into_iter()
            .map(|kept| {
                let about = typst_packages::about(&kept.dir).ok();
                json!({
                    "package": kept.spec.to_string(),
                    "kept": match kept.place {
                        Place::Own => "own",
                        Place::Fetched => "fetched",
                    },
                    "description": about.as_ref().and_then(|about| about.description.clone()),
                    "entrypoint": about.as_ref().map(|about| about.entrypoint.clone()),
                    "template": about.as_ref().is_some_and(|about| about.template.is_some()),
                })
            })
            .collect();
        json!({
            "imported": used,
            "on_machine": kept,
            "note": "Import only packages that are on this machine. An agent cannot fetch one: \
                     an import of one that is not here is refused by Typst, and the user is \
                     asked whether to fetch it.",
        })
    }
}
