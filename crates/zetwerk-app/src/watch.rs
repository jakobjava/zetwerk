//! Noticing when the document's files change on disk.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// Watches the directories that hold a document's files.
///
/// Directories are watched rather than files, because editors save by
/// writing a new file and renaming it over the old one: a watch on the file
/// itself would be left pointing at a file that no longer has a name.
pub struct FileWatcher {
    watcher: RecommendedWatcher,
    directories: HashSet<PathBuf>,
}

impl FileWatcher {
    /// Starts watching. `changed` is called on a background thread with
    /// every path something happened to; it gets paths of all files in the
    /// watched directories, not only of the document's.
    pub fn new(changed: impl Fn(Vec<PathBuf>) + Send + 'static) -> notify::Result<Self> {
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if let Ok(event) = event
                && !event.kind.is_access()
            {
                changed(event.paths);
            }
        })?;
        Ok(Self {
            watcher,
            directories: HashSet::new(),
        })
    }

    /// Makes sure the directories of `files` are watched. Directories that
    /// cannot be watched (deleted, no permission) are skipped; the document
    /// then simply does not reload when files in them change.
    pub fn watch(&mut self, files: &[PathBuf]) {
        for directory in files.iter().filter_map(|file| file.parent()) {
            if !self.directories.contains(directory)
                && self
                    .watcher
                    .watch(directory, RecursiveMode::NonRecursive)
                    .is_ok()
            {
                self.directories.insert(directory.to_owned());
            }
        }
    }

    /// Whether `path` is in a watched directory.
    #[must_use]
    pub fn covers(&self, path: &Path) -> bool {
        path.parent()
            .is_some_and(|directory| self.directories.contains(directory))
    }
}
