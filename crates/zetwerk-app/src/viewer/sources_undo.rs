//! A way back for the bibliography file.
//!
//! `u` takes back what was done to the text; the sources' file is
//! written at once and is no part of that. So before each change to it
//! the file as it was is kept, in the state directory (where the
//! journals are, readable by the user alone), the last few of them:
//! `:bib undo` puts back the last, and keeps what it replaced, for
//! `:bib redo`.
//!
//! With each version is kept what the change made of the file (a hash
//! of it). Where the file is no longer that, it was changed elsewhere
//! since, by hand or by another program: it is then not put back
//! unless asked once more (`:bib undo!`), so that no change made
//! elsewhere is lost unawares.

use std::path::{Path, PathBuf};

use super::sources::write_whole;
use super::{Edited, Viewer};

/// How many versions of a file are kept.
const KEPT: usize = 20;
/// The name of what `:bib redo` would put back.
const REDO: &str = "redo";

/// A hash of `bytes` that stays the same from one build to the next
/// (FNV-1a): to name a folder after a path.
fn lasting_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The numbers of the versions kept in `folder`, the oldest first.
fn numbers(folder: &Path) -> Vec<u64> {
    let Ok(read) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut numbers: Vec<u64> = read
        .flatten()
        .filter_map(|file| {
            let name = file.file_name();
            name.to_str()?.strip_suffix(".text")?.parse().ok()
        })
        .collect();
    numbers.sort_unstable();
    numbers
}

/// Makes the folder `folder`, which only the user can enter.
fn private_folder(folder: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(folder)
}

/// What was kept as one version: the text, what was done to it, and
/// the hash of what that made of it, where that is known.
struct Version {
    text: String,
    what: String,
    after: Option<u64>,
    /// Whether there was no file: the change made it.
    absent: bool,
}

/// Keeps `text` in `folder` as `name`, with `what` was done to it, which
/// made it `after`.
fn keep(
    folder: &Path,
    name: &str,
    (text, absent): (&str, bool),
    what: &str,
    after: &str,
) -> std::io::Result<()> {
    private_folder(folder)?;
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    zetwerk_core::write_atomically(&folder.join(format!("{name}.text")), text.as_bytes())?;
    let after = lasting_hash(after.as_bytes());
    let absent = if absent { "absent\n" } else { "" };
    let told = format!("{seconds}\n{what}\n{after:016x}\n{absent}");
    zetwerk_core::write_atomically(&folder.join(format!("{name}.what")), told.as_bytes())
}

/// What was kept in `folder` as `name`.
fn kept(folder: &Path, name: &str) -> Option<Version> {
    let text = std::fs::read_to_string(folder.join(format!("{name}.text"))).ok()?;
    let told = std::fs::read_to_string(folder.join(format!("{name}.what"))).unwrap_or_default();
    let mut lines = told.lines().skip(1);
    let what = lines.next().unwrap_or("a change").to_owned();
    let after = lines
        .next()
        .and_then(|hash| u64::from_str_radix(hash, 16).ok());
    let absent = lines.next() == Some("absent");
    Some(Version {
        text,
        what,
        after,
        absent,
    })
}

/// Refuses to put a version back over `now` where the file is not what
/// the change it takes back made of it, unless `anyway`.
fn unchanged_since(version: &Version, now: &str, name: &str, anyway: bool) -> Result<(), String> {
    match version.after {
        Some(after) if !anyway && after != lasting_hash(now.as_bytes()) => Err(format!(
            "{name} was changed elsewhere since ({}): nothing is put back. :bib undo! \
             puts it back all the same; the file as it is now is kept for :bib redo",
            version.what
        )),
        _ => Ok(()),
    }
}

/// The name of the file at `path`, to tell the user.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Forgets what was kept in `folder` as `name`.
fn forget(folder: &Path, name: &str) {
    for ending in ["text", "what"] {
        let _ = std::fs::remove_file(folder.join(format!("{name}.{ending}")));
    }
}

impl Viewer {
    /// Where the versions of the file at `path` are kept; `None` where
    /// there is no state directory.
    fn versions_of(&self, path: &Path) -> Option<PathBuf> {
        let state = self.state_dir.as_ref()?;
        // The folder it is in exists, the file may not yet.
        let folder = path.parent().and_then(|folder| folder.canonicalize().ok());
        let whole = match (folder, path.file_name()) {
            (Some(folder), Some(name)) => folder.join(name),
            _ => path.to_owned(),
        };
        let name = whole
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        let hash = lasting_hash(whole.as_os_str().as_encoded_bytes());
        Some(
            state
                .join("sources")
                .join(format!("{}-{hash:016x}", name.unwrap_or_default())),
        )
    }

    /// Keeps `before`, the text of the file at `path`, before `what` is
    /// done to it, which makes it `after`; what `:bib redo` would have
    /// put back is forgotten.
    ///
    /// # Errors
    ///
    /// If it cannot be kept: then the change is not to be made.
    pub(super) fn keep_version(
        &self,
        path: &Path,
        before: &str,
        after: &str,
        what: &str,
    ) -> Result<(), String> {
        let Some(folder) = self.versions_of(path) else {
            return Ok(());
        };
        let had = numbers(&folder);
        let next = had.last().map_or(1, |last| last + 1);
        let absent = path.symlink_metadata().is_err();
        keep(
            &folder,
            &format!("{next:08}"),
            (before, absent),
            what,
            after,
        )
        .map_err(|err| {
            format!("nothing is changed: the file as it is could not be kept to go back to ({err})")
        })?;
        forget(&folder, REDO);
        for old in had.iter().take((had.len() + 1).saturating_sub(KEPT)) {
            forget(&folder, &format!("{old:08}"));
        }
        Ok(())
    }

    /// What the last change to the sources' file was, if it can be
    /// taken back.
    pub(super) fn last_source_change(&self) -> Option<String> {
        let sources = self.sources().ok()?;
        let folder = self.versions_of(&sources.path)?;
        let last = *numbers(&folder).last()?;
        kept(&folder, &format!("{last:08}")).map(|version| version.what)
    }

    /// `:bib undo`: the sources' file as it was before the last change
    /// to it; `anyway` also where it was changed elsewhere since.
    pub(super) fn undo_sources(&mut self, anyway: bool) -> Result<(), String> {
        let sources = self.sources()?;
        let folder = self
            .versions_of(&sources.path)
            .ok_or("no versions of the file are kept here")?;
        let last = numbers(&folder)
            .last()
            .copied()
            .ok_or("nothing to take back: no change to the sources' file is kept")?;
        let name = format!("{last:08}");
        let version = kept(&folder, &name).ok_or("the version kept cannot be read")?;
        unchanged_since(&version, &sources.text, &file_name(&sources.path), anyway)?;
        let now = (sources.text.as_str(), false);
        keep(&folder, REDO, now, &version.what, &version.text).map_err(|err| {
            format!("nothing is taken back: the file as it is could not be kept ({err})")
        })?;
        if version.absent {
            // The change made the file: taking it back takes the file.
            std::fs::remove_file(&sources.path)
                .map_err(|err| format!("cannot remove {}: {err}", sources.path.display()))?;
        } else {
            write_whole(&sources.path, &version.text)?;
        }
        forget(&folder, &name);
        self.sources_written(&sources.path);
        let what = version.what;
        self.notice(format!("taken back: {what}  (:bib redo makes it again)"));
        Ok(())
    }

    /// `:bib redo`: the change last taken back, made again.
    pub(super) fn redo_sources(&mut self) -> Result<(), String> {
        let sources = self.sources()?;
        let folder = self
            .versions_of(&sources.path)
            .ok_or("no versions of the file are kept here")?;
        let version = kept(&folder, REDO).ok_or("nothing to make again")?;
        unchanged_since(&version, &sources.text, &file_name(&sources.path), false)?;
        let what = version.what;
        self.keep_version(&sources.path, &sources.text, &version.text, &what)?;
        write_whole(&sources.path, &version.text)?;
        self.sources_written(&sources.path);
        self.notice(format!("made again: {what}  (:bib undo takes it back)"));
        Ok(())
    }

    /// Has the pages made anew after the sources' file was written.
    pub(super) fn sources_written(&mut self, path: &Path) {
        if self.file_changed(path) {
            self.edited = Some(Edited::Command);
        }
        self.sources_kept.written_at = Some(self.revision);
    }

    /// `u` right after the bibliography file was changed, with nothing
    /// typed since: `u` does not take that back, and would take back
    /// something else unsaid. It says so instead, once. Returns whether
    /// it took the key.
    pub(super) fn bib_undo_key(&mut self, press: crate::keys::KeyPress) -> bool {
        let plain_u = press.key == crate::keys::Key::Char('u') && !press.ctrl && !press.alt;
        let Some(editor) = &self.editor else {
            return false;
        };
        let idle = editor.mode() == crate::editor::Mode::Normal && editor.is_idle();
        if !plain_u || !idle || self.picker.is_some() {
            return false;
        }
        if self.sources_kept.written_at.take() != Some(self.revision) {
            return false;
        }
        self.alert(
            "the last change was to the bibliography file, which u does not take back: \
             :bib undo does  (u again takes back the text's last change)",
        );
        true
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use super::super::tests::{directory, open};
    use super::*;

    fn message(viewer: &Viewer) -> String {
        viewer
            .message
            .as_ref()
            .map_or_else(String::new, |message| message.text.clone())
    }

    #[test]
    fn what_was_changed_elsewhere_is_not_put_back_unawares() {
        let directory = directory("Words.\n\n#bibliography(\"refs.bib\")\n");
        let refs = directory.join("refs.bib");
        let mine = "% mine\n@misc{a, title = {A}, year = 2001}\n";
        std::fs::write(&refs, mine).unwrap();
        let (mut viewer, _) = open(&directory);
        viewer.bib_command("remove! a");
        let removed = std::fs::read_to_string(&refs).unwrap();
        assert!(!removed.contains("@misc{a"));
        assert_eq!(viewer.last_source_change().as_deref(), Some("a taken out"));
        // Changed by hand since: not put back over that.
        let by_hand = format!("{removed}@misc{{b, title = {{By hand}}}}\n");
        std::fs::write(&refs, &by_hand).unwrap();
        viewer.bib_command("undo");
        assert!(viewer.message.as_ref().is_some_and(|message| message.alert));
        assert!(
            message(&viewer).contains("changed elsewhere"),
            "{}",
            message(&viewer)
        );
        assert_eq!(std::fs::read_to_string(&refs).unwrap(), by_hand);
        // Asked once more: put back, and what was there kept for redo.
        viewer.bib_command("undo!");
        assert_eq!(std::fs::read_to_string(&refs).unwrap(), mine);
        viewer.bib_command("redo");
        assert_eq!(std::fs::read_to_string(&refs).unwrap(), by_hand);
        viewer.bib_command("undo");
        assert_eq!(std::fs::read_to_string(&refs).unwrap(), mine);
        // Only so many versions are kept, the newest.
        for _ in 0..KEPT + 3 {
            viewer.bib_command("new misc");
        }
        let folder = viewer.versions_of(&refs).unwrap();
        assert_eq!(numbers(&folder).len(), KEPT);
        let _ = std::fs::remove_dir_all(directory);
    }
}
