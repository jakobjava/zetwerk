//! Writing the text elsewhere, and giving the file another name: `:w
//! <path>` writes a copy and stays, `:saveas <path>` writes and goes on
//! as the new file with the history, `:mv <path>` moves the file and its
//! journal. None of them writes over a file without `!` (`:mv` never
//! does), and none copies what the document needs beside it: they say
//! which of those the new place lacks.

use std::path::{Path, PathBuf};

use typst::syntax::FileId;
use zetwerk_core::DiskError;

use super::{Edited, Viewer};

/// No more of what a new place lacks is named than this.
const NAMED: usize = 4;

/// Whether `err` says that there is a file in the way.
fn in_the_way(err: &DiskError) -> bool {
    matches!(err, DiskError::Io(err) if err.kind() == std::io::ErrorKind::AlreadyExists)
}

impl Viewer {
    /// The commands of this module. Returns whether `name` is one of them
    /// with an `argument`.
    pub(super) fn saving_command(&mut self, name: &str, argument: &str) -> bool {
        if argument.is_empty() {
            return false;
        }
        match name {
            "w" | "write" | "w!" | "write!" => self.write_to(argument, name.ends_with('!')),
            "sav" | "saveas" | "sav!" | "saveas!" => self.save_as(argument, name.ends_with('!')),
            "mv" | "rename" => self.move_to(argument),
            _ => return false,
        }
        true
    }

    /// `name` as a path: as it is if absolute (`~/` is the home
    /// directory), else from the directory of the file being edited.
    fn path_named(&self, name: &str) -> PathBuf {
        let home = std::env::var_os("HOME").filter(|home| !home.is_empty());
        if let (Some(rest), Some(home)) = (name.strip_prefix("~/"), home) {
            return PathBuf::from(home).join(rest);
        }
        let path = Path::new(name);
        if path.is_absolute() {
            return path.to_owned();
        }
        let beside = self
            .editor
            .as_ref()
            .and_then(|editor| editor.path().parent())
            .unwrap_or(self.project.root_dir());
        beside.join(path)
    }

    /// What the document reads from beside the file being edited that is
    /// not beside `to`: images, the bibliography, files it includes. Said
    /// after what writing there said; empty where nothing is missing.
    fn lacking_beside(&self, to: &Path) -> String {
        let Some(here) = self
            .editor
            .as_ref()
            .and_then(|editor| editor.path().parent())
        else {
            return String::new();
        };
        let there = to.parent().unwrap_or(to);
        let same = |a: &Path, b: &Path| {
            a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
        };
        if same(here, there) {
            return String::new();
        }
        let edited = self.editor.as_ref().map(|editor| editor.path().to_owned());
        let lacking: Vec<String> = self
            .project
            .dependencies()
            .iter()
            .filter(|path| Some(path.as_path()) != edited.as_deref())
            .filter(|path| edited.as_deref().is_none_or(|edited| !same(path, edited)))
            .filter_map(|path| path.strip_prefix(here).ok())
            .filter(|relative| !there.join(relative).exists())
            .map(|relative| relative.display().to_string())
            .collect();
        if lacking.is_empty() {
            String::new()
        } else if lacking.len() <= NAMED {
            format!("; not there, and not copied: {}", lacking.join(", "))
        } else {
            format!(
                "; not there, and not copied: {} and {} more",
                lacking[..NAMED].join(", "),
                lacking.len() - NAMED
            )
        }
    }

    /// `:w <path>`: writes the text to `path` too, and stays with the file
    /// it is.
    fn write_to(&mut self, name: &str, overwrite: bool) {
        let to = self.path_named(name);
        let Some(editor) = &self.editor else {
            self.alert("this window only shows the file; there is nothing to write");
            return;
        };
        if super::buffers::same_file(editor.path(), &to) {
            self.save(overwrite);
            return;
        }
        match editor.write_copy(&to, overwrite) {
            Ok(()) => {
                let lines = editor.text().matches('\n').count();
                let lacking = self.lacking_beside(&to);
                self.notice(format!(
                    "{} written, {lines} lines; still editing {}{lacking}",
                    to.display(),
                    self.file
                ));
            }
            Err(err) if in_the_way(&err) => {
                self.alert(format!(
                    "{} exists: :w! {name} writes over it",
                    to.display()
                ));
            }
            Err(err) => self.alert(format!("not written to {}: {err}", to.display())),
        }
    }

    /// The file `to` names, if the document can go on with it there: in
    /// its directory, and not open already.
    fn file_for(&mut self, name: &str, to: &Path) -> Option<FileId> {
        let Some(file) = self.project.file_id(to) else {
            self.alert(format!(
                "{} is not in the document's directory, {}: :w {name} writes a copy there",
                to.display(),
                self.project.root_dir().display()
            ));
            return None;
        };
        if self.others.iter().any(|other| other.file() == file) {
            self.alert(format!("{name} is open here: :b {name} goes to it"));
            return None;
        }
        Some(file)
    }

    /// `:saveas <path>`: writes the text to `path` and goes on as that
    /// file, with the history.
    fn save_as(&mut self, name: &str, overwrite: bool) {
        let to = self.path_named(name);
        let Some(editor) = &self.editor else {
            self.alert("this window only shows the file; there is nothing to write");
            return;
        };
        if super::buffers::same_file(editor.path(), &to) {
            self.save(overwrite);
            return;
        }
        let Some(file) = self.file_for(name, &to) else {
            return;
        };
        // The notes go along: those stored are read first.
        self.notes_ready();
        let Some(editor) = &mut self.editor else {
            return;
        };
        let (old, old_name) = (editor.file(), self.file.clone());
        let was_main = old == self.project.main();
        match editor.save_as(&to, file, overwrite) {
            Ok(()) => {
                let lines = editor.text().matches('\n').count();
                let lacking = self.lacking_beside(&to);
                self.named(old, &to);
                let shown = if was_main {
                    String::new()
                } else {
                    format!("; the document still includes {old_name}, as it is written")
                };
                self.notice(format!(
                    "{} written, {lines} lines; editing it now, {old_name} is as it was \
                     written last{shown}{lacking}",
                    self.file
                ));
            }
            Err(err) if in_the_way(&err) => {
                self.alert(format!(
                    "{} exists: :saveas! {name} writes over it",
                    to.display()
                ));
            }
            Err(err) => self.alert(format!("not written to {}: {err}", to.display())),
        }
    }

    /// `:mv <path>`: moves the file to `path`, with its history and what
    /// is not written of it. Never over a file that is there.
    fn move_to(&mut self, name: &str) {
        let to = self.path_named(name);
        let Some(editor) = &self.editor else {
            self.alert("this window only shows the file; there is nothing to move");
            return;
        };
        if super::buffers::same_file(editor.path(), &to) {
            self.notice(format!("{name} is this file"));
            return;
        }
        let Some(file) = self.file_for(name, &to) else {
            return;
        };
        // The notes go along: those stored are read first.
        self.notes_ready();
        let Some(editor) = &mut self.editor else {
            return;
        };
        let (old, old_name) = (editor.file(), self.file.clone());
        let was_main = old == self.project.main();
        let lacking = self.lacking_beside(&to);
        let Some(editor) = &mut self.editor else {
            return;
        };
        match editor.rename(&to, file) {
            Ok(left) => {
                self.named(old, &to);
                let left = left.map_or_else(String::new, |err| {
                    format!("; {old_name} could not be removed ({err}) and is a copy now")
                });
                let shown = if was_main {
                    String::new()
                } else {
                    format!("; files that include it still name {old_name}")
                };
                self.notice(format!(
                    "moved to {}, with its history{left}{shown}{lacking}",
                    self.file
                ));
            }
            Err(err) if in_the_way(&err) => {
                self.alert(format!("{} exists: :mv writes over no file", to.display()));
            }
            Err(err) => self.alert(format!("not moved: {err}")),
        }
    }

    /// The file being edited, which was `old`, is the one at `to` now.
    fn named(&mut self, old: FileId, to: &Path) {
        let Some(editor) = &self.editor else {
            return;
        };
        let (new, source) = (editor.file(), editor.source().clone());
        self.project.renamed(old, source, to);
        self.notes_renamed(old, new);
        for file in self.opened.iter_mut().chain(&mut self.stale_files) {
            if *file == old {
                *file = new;
            }
        }
        self.file = to
            .strip_prefix(self.project.root_dir())
            .unwrap_or(to)
            .to_string_lossy()
            .into_owned();
        if let Some(state_dir) = &self.state_dir
            && new == self.project.main()
        {
            // The document is known by its new name among those opened
            // last; not being able to note it takes nothing from it.
            let _ = super::start::remember(state_dir, to);
        }
        self.text_changed(Edited::Command);
    }
}
