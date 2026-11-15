//! Noticing when the document's files change on disk.

use std::collections::{HashMap, HashSet};
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
    /// The files that symbolic links among the watched files lead to, by
    /// the name the document knows them by: a file written through its
    /// link changes in the directory of the file, not of the link.
    targets: HashMap<PathBuf, PathBuf>,
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
            targets: HashMap::new(),
        })
    }

    /// Makes sure the directories of `files` are watched. Directories that
    /// cannot be watched (deleted, no permission) are skipped; the document
    /// then simply does not reload when files in them change.
    pub fn watch(&mut self, files: &[PathBuf]) {
        let mut targets = Vec::new();
        for file in files {
            let is_link = file
                .symlink_metadata()
                .is_ok_and(|meta| meta.file_type().is_symlink());
            if is_link && let Ok(target) = std::fs::canonicalize(file) {
                self.targets.insert(target.clone(), file.clone());
                targets.push(target);
            }
        }
        for directory in files
            .iter()
            .chain(&targets)
            .filter_map(|file| file.parent())
        {
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

    /// The name the document knows the file at `path` by: the link that
    /// leads to it, if it is the file of a watched link.
    #[must_use]
    pub fn known_as(&self, path: &Path) -> PathBuf {
        self.targets
            .get(path)
            .cloned()
            .unwrap_or_else(|| path.to_owned())
    }

    /// Whether `path` is in a watched directory.
    #[must_use]
    pub fn covers(&self, path: &Path) -> bool {
        path.parent()
            .is_some_and(|directory| self.directories.contains(directory))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_file_written_through_a_link_is_heard_of_by_the_links_name() {
        // A document opened through a symbolic link into another
        // directory did not reload: only the link's directory was watched.
        let root = std::env::temp_dir().join(format!("zetwerk-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (real, shown) = (root.join("real"), root.join("shown"));
        std::fs::create_dir_all(&real).unwrap();
        std::fs::create_dir_all(&shown).unwrap();
        let target = real.join("paper.typ");
        std::fs::write(&target, "One.\n").unwrap();
        let link = shown.join("paper.typ");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let (send, heard) = std::sync::mpsc::channel();
        let mut watcher = FileWatcher::new(move |paths| {
            let _ = send.send(paths);
        })
        .unwrap();
        watcher.watch(std::slice::from_ref(&link));
        std::fs::write(&target, "Two.\n").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut known = Vec::new();
        while !known.contains(&link) && std::time::Instant::now() < deadline {
            if let Ok(paths) = heard.recv_timeout(std::time::Duration::from_millis(100)) {
                known.extend(paths.iter().map(|path| watcher.known_as(path)));
            }
        }
        assert!(known.contains(&link), "{known:?}");
        let _ = std::fs::remove_dir_all(root);
    }
}
