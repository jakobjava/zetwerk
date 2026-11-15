//! More than one file of a document open at once.
//!
//! One file is edited at a time: it is `Viewer::editor`. The others wait
//! in `Viewer::others` with their text, their cursor and their history.
//! All of them are compiled as they are, saved or not. Registers, the
//! last search and what `.` repeats go along from file to file.

use std::path::{Path, PathBuf};

use typst::syntax::{FileId, LinkedNode, Side, SyntaxKind};
use zetwerk_core::Opened;

use super::{Edited, Viewer, assist, start};
use crate::editor::{self, Editor, Shown};
use crate::keys::{Key, KeyPress};

/// More Typst files than this are not listed to choose from.
const MAX_FILES: usize = 3000;
/// Directories deeper than this below the project's are not looked into.
const MAX_DEPTH: usize = 6;

/// Whether `a` and `b` name the same file.
pub(super) fn same_file(a: &Path, b: &Path) -> bool {
    a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

impl Viewer {
    /// The file being edited; the main file where nothing is.
    pub(super) fn file_id(&self) -> FileId {
        self.editor
            .as_ref()
            .map_or_else(|| self.project.main(), Editor::file)
    }

    /// Notes that the text of the file being edited has changed since
    /// the last compile.
    pub(super) fn touched(&mut self) {
        let file = self.file_id();
        if !self.stale_files.contains(&file) {
            self.stale_files.push(file);
        }
    }

    /// The name a file goes by: its path from the project directory.
    pub(super) fn name_of(&self, path: &Path) -> String {
        let root = self.project.root_dir();
        let shown = path.strip_prefix(root).unwrap_or(path);
        shown.to_string_lossy().into_owned()
    }

    /// Says what opening a file found, where it is worth saying;
    /// `restored` is when changes brought back were last made.
    pub(super) fn announce(&mut self, opened: &Opened, restored: Option<std::time::SystemTime>) {
        match opened {
            Opened::Clean => {}
            // Loud: text the user may have forgotten, or never meant to
            // keep, is not what the file holds.
            Opened::Restored => {
                let when = restored.map_or_else(
                    || "in the last session".to_owned(),
                    |then| start::ago(then, std::time::SystemTime::now()),
                );
                self.alert(format!(
                    "unsaved changes, last made {when}, are back; not in the file: \
                     :w writes them, :e! drops them (u brings them back)"
                ));
            }
            Opened::ChangedWhileClosed => {
                self.notice(
                    "the file changed since the last session: u brings back the text as it was",
                );
            }
            Opened::Conflict { .. } => self.alert(
                "the file changed while unsaved changes were pending: it is shown as it is \
                 now, u brings back the unsaved text",
            ),
            Opened::JournalUnusable { journal } => self.alert(format!(
                "the journal could not be read and was set aside as {}",
                journal.display()
            )),
        }
    }

    /// Says what else opening a file found (see [`Editor::remarks`]):
    /// a history taken over from where the file was before, a journal
    /// that was damaged. Loud, after whatever else is said.
    pub(super) fn announce_remarks(&mut self, remarks: &[String]) {
        if remarks.is_empty() {
            return;
        }
        let said = remarks.join("; ");
        match &self.message {
            Some(message) if message.alert => {
                let both = format!("{}; {said}", message.text);
                self.alert(both);
            }
            _ => self.alert(said),
        }
    }

    /// Says which files of the document that are not open hold changes
    /// left unsaved in an earlier session (the window was closed with
    /// them): the pages show those files as the disk has them, and the
    /// changes come back only when the file is opened.
    pub(super) fn announce_unsaved_elsewhere(&mut self) {
        let Some(state_dir) = self.state_dir.clone() else {
            return;
        };
        let open: Vec<PathBuf> = self
            .editor
            .iter()
            .chain(&self.others)
            .map(|editor| editor.path().to_owned())
            .collect();
        let waiting: Vec<String> = self
            .project
            .dependencies()
            .into_iter()
            .filter(|path| path.extension().is_some_and(|ending| ending == "typ"))
            .filter(|path| !open.iter().any(|open| same_file(open, path)))
            .filter(|path| zetwerk_core::has_unsaved_changes(path, &state_dir))
            .map(|path| self.name_of(&path))
            .collect();
        let Some(first) = waiting.first() else {
            return;
        };
        let said = format!(
            "unsaved changes from an earlier session wait in {}: :e /{first} brings them back",
            waiting.join(", ")
        );
        match &self.message {
            Some(message) if message.alert => {
                let both = format!("{}; {said}", message.text);
                self.alert(both);
            }
            _ => self.alert(said),
        }
    }

    /// Opens the file at `path` for editing, or goes to it if it is open.
    /// With `create`, a file that is not there is made.
    pub fn open_path(&mut self, path: &Path, create: bool) {
        let Some(file) = self.project.file_id(path) else {
            self.alert(format!(
                "{} is not in the document's directory, {}",
                path.display(),
                self.project.root_dir().display()
            ));
            return;
        };
        if !path.exists() {
            if !create {
                self.alert(format!("there is no {}; :e! makes it", self.name_of(path)));
                return;
            }
            // Made anew, never truncated: a file that came into being
            // since it was looked for is the user's, and stays as it is.
            let made = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path);
            if let Err(err) = made.map(drop) {
                self.alert(format!("cannot make {}: {err}", path.display()));
                return;
            }
        }
        self.open_file(file, None);
    }

    /// Makes `file` the file being edited, with the cursor at byte `at`
    /// of it if that is given.
    pub(super) fn open_file(&mut self, file: FileId, at: Option<usize>) {
        if let Some(editor) = &mut self.editor
            && editor.file() == file
        {
            if let Some(at) = at {
                editor.jump(at);
            }
            return self.follow_cursor();
        }
        let Some(mut current) = self.editor.take() else {
            return;
        };
        let waiting = self.others.iter().position(|other| other.file() == file);
        let mut restored = false;
        let mut next = if let Some(index) = waiting {
            self.others.remove(index)
        } else {
            let opened = match (self.project.path_of(file), &self.state_dir) {
                (Some(path), Some(state_dir)) => {
                    Editor::open(&path, state_dir, file).map_err(|err| err.to_string())
                }
                _ => Err("it is not a file of this document".to_owned()),
            };
            match opened {
                Ok((editor, opened)) => {
                    self.announce(&opened, editor.restored());
                    self.announce_remarks(editor.remarks());
                    restored = !matches!(opened, Opened::Clean);
                    self.opened.push(file);
                    editor
                }
                Err(why) => {
                    self.editor = Some(current);
                    self.alert(format!("not opened: {why}"));
                    return;
                }
            }
        };
        // What is being typed there is over.
        for _ in 0..2 {
            if current.mode() != editor::Mode::Normal {
                current.key(KeyPress::plain(Key::Escape), &Shown::Pages(None));
            }
        }
        next.share(&current);
        self.project.set_source(current.source().clone());
        self.project.set_source(next.source().clone());
        self.others.insert(0, current);
        if let Some(at) = at {
            next.jump(at);
        }
        self.file = self.name_of(next.path());
        self.editor = Some(next);
        self.swap_proposals();
        self.completion = None;
        self.tooltip = None;
        self.agent_mark = None;
        if restored {
            // The journal brought text that is not the file's: the pages
            // are not those of it.
            self.text_changed(Edited::Command);
        }
        self.sync_proposals();
        self.follow_cursor();
    }

    /// The files that are open, in the order they were opened, each with
    /// its name, whether it is the one being edited and whether it has
    /// changes that are not written.
    #[must_use]
    pub fn buffers(&self) -> Vec<(String, bool, bool)> {
        let current = self.editor.as_ref().map(Editor::file);
        let editors = || self.editor.iter().chain(&self.others);
        self.opened
            .iter()
            .filter_map(|file| editors().find(|editor| editor.file() == *file))
            .map(|editor| {
                (
                    self.name_of(editor.path()),
                    Some(editor.file()) == current,
                    editor.is_modified(),
                )
            })
            .collect()
    }

    /// The names of the open files with changes that are not written.
    #[must_use]
    pub fn unwritten(&self) -> Vec<String> {
        let modified = |(name, _, modified): (String, bool, bool)| modified.then_some(name);
        self.buffers().into_iter().filter_map(modified).collect()
    }

    /// Writes every open file that has changes. Returns whether all of
    /// them are written now.
    pub(super) fn save_all(&mut self) -> bool {
        let mut failed = None;
        let mut written = 0;
        for editor in self.editor.iter_mut().chain(&mut self.others) {
            if !editor.is_modified() {
                continue;
            }
            match editor.save(false) {
                Ok(()) => written += 1,
                Err(err) => {
                    let name = editor.path().display().to_string();
                    failed.get_or_insert(format!("{name} not written: {err}"));
                }
            }
        }
        if let Some(why) = failed {
            self.alert(why);
            return false;
        }
        self.notice(format!("{written} file(s) written"));
        true
    }

    /// `:q!`: leaves, and before that every open file with changes that
    /// are not written is as its file is again, as one step that undo
    /// takes back. So the next
    /// time it is opened it is what was written last, with nothing
    /// brought back, and the history still holds what was dropped.
    ///
    /// A file that cannot be read stays as it is: its changes are kept,
    /// and are back when it is opened, as after a window closed.
    pub(super) fn leave_dropping(&mut self) {
        for editor in self.editor.iter_mut().chain(&mut self.others) {
            if editor.is_modified() {
                let _ = editor.reload();
            }
        }
        self.quit = true;
    }

    /// A file that is open but not being edited changed on disk, perhaps.
    /// Returns whether it did and the document has to be compiled again;
    /// `None` if `path` is no such file.
    pub(super) fn waiting_file_changed(&mut self, path: &Path) -> Option<bool> {
        let index = self
            .others
            .iter()
            .position(|other| same_file(other.path(), path))?;
        let editor = &mut self.others[index];
        let name = editor.path().display().to_string();
        let changed = matches!(editor.disk_status(), Ok(zetwerk_core::DiskStatus::Changed));
        if !changed {
            return Some(false);
        }
        if editor.is_modified() {
            self.alert(format!(
                "{name} changed on disk and has unwritten changes here: go to it and decide"
            ));
            return Some(false);
        }
        match editor.reload() {
            Ok(()) => {
                let source = editor.source().clone();
                let file = editor.file();
                self.project.set_source(source);
                self.revision += 1;
                if !self.stale_files.contains(&file) {
                    self.stale_files.push(file);
                }
                Some(true)
            }
            Err(err) => {
                self.alert(format!("{name}: {err}"));
                Some(false)
            }
        }
    }

    /// Whether `path` is the file being edited.
    pub(super) fn is_edited(&self, path: &Path) -> bool {
        self.editor
            .as_ref()
            .is_some_and(|editor| same_file(editor.path(), path))
    }

    /// The commands about files. Returns whether `command` was one.
    pub(super) fn file_command(&mut self, command: &str) -> bool {
        let (name, argument) = match command.split_once(' ') {
            Some((name, argument)) => (name, argument.trim()),
            None => (command, ""),
        };
        if self.saving_command(name, argument) {
            return true;
        }
        match (name, argument) {
            ("e" | "edit" | "e!" | "edit!", path) if !path.is_empty() => {
                let path = self.beside_current(path);
                self.open_path(&path, name.ends_with('!'));
            }
            ("ls" | "buffers" | "files", "") if name != "files" => {
                let listed: Vec<String> = self
                    .buffers()
                    .iter()
                    .enumerate()
                    .map(|(index, (name, current, modified))| {
                        let current = if *current { "%" } else { "" };
                        let modified = if *modified { " [+]" } else { "" };
                        format!("{}{current} {name}{modified}", index + 1)
                    })
                    .collect();
                self.notice(listed.join("   "));
            }
            ("files", "") => self.pick_file(),
            ("bn" | "bnext" | "bp" | "bprevious", "") => {
                let count = self.opened.len();
                let current = self.file_id();
                let at = self.opened.iter().position(|file| *file == current);
                if let (Some(at), true) = (at, count > 1) {
                    let step = if name.starts_with("bn") { 1 } else { count - 1 };
                    self.open_file(self.opened[(at + step) % count], None);
                }
            }
            ("b#", "") | ("b" | "buffer", "#") => match self.others.first().map(Editor::file) {
                Some(file) => self.open_file(file, None),
                None => self.alert("no other file is open"),
            },
            ("b" | "buffer", wanted) if !wanted.is_empty() => {
                let by_number = wanted
                    .parse::<usize>()
                    .ok()
                    .and_then(|number| self.opened.get(number.checked_sub(1)?).copied());
                let named: Vec<FileId> = self
                    .editor
                    .iter()
                    .chain(&self.others)
                    .filter(|editor| self.name_of(editor.path()).contains(wanted))
                    .map(Editor::file)
                    .collect();
                match (by_number, named.as_slice()) {
                    (Some(file), _) => self.open_file(file, None),
                    (None, [file]) => self.open_file(*file, None),
                    (None, []) => self.alert(format!("no open file is called {wanted}")),
                    (None, _) => self.alert(format!("more than one open file has {wanted}")),
                }
            }
            ("wa" | "wall", "") => {
                self.save_all();
            }
            ("file", "") => self.open_file_under_cursor(),
            ("pdf" | "png" | "svg" | "pdf!" | "png!" | "svg!", argument) => {
                self.export_command(name, argument);
            }
            ("figure!", argument) if !argument.is_empty() => {
                self.export_command(name, argument);
            }
            // `:figure` alone is for the figure's card (see `figures`).
            ("figure", argument) if !argument.is_empty() => {
                self.export_command(name, argument);
            }
            _ => return false,
        }
        true
    }

    /// `name` as a path: from the project directory if it begins with a
    /// slash, else from the directory of the file being edited.
    fn beside_current(&self, name: &str) -> PathBuf {
        let root = self.project.root_dir();
        match name.strip_prefix('/') {
            Some(rooted) if !Path::new(name).exists() => root.join(rooted),
            _ => {
                let beside = self
                    .editor
                    .as_ref()
                    .and_then(|editor| editor.path().parent())
                    .unwrap_or(root);
                beside.join(name)
            }
        }
    }

    /// `gf`: opens the file whose name is under the cursor, as in
    /// `#include "chapter.typ"`.
    fn open_file_under_cursor(&mut self) {
        let Some(editor) = &self.editor else {
            return;
        };
        let leaf = LinkedNode::new(editor.source().root()).leaf_at(editor.cursor(), Side::After);
        let Some(name) = leaf
            .filter(|leaf| leaf.kind() == SyntaxKind::Str)
            .map(|leaf| leaf.leaf_text().trim_matches('"').to_owned())
        else {
            self.alert("the cursor is not on the name of a file (in quotation marks)");
            return;
        };
        // In Typst a path with a slash in front is from the project
        // directory.
        let path = match name.strip_prefix('/') {
            Some(rooted) => self.project.root_dir().join(rooted),
            None => self.beside_current(&name),
        };
        if path.extension().is_some_and(|ending| ending == "typ") {
            self.open_path(&path, false);
        } else {
            self.notice(format!("{name} is not a Typst file"));
        }
    }

    /// The Typst files of the project, to choose one from.
    fn pick_file(&mut self) {
        let root = self.project.root_dir().to_owned();
        let mut found: Vec<PathBuf> = Vec::new();
        let mut directories = vec![(root.clone(), 0)];
        while let Some((directory, depth)) = directories.pop() {
            let Ok(entries) = std::fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let hidden = entry.file_name().to_string_lossy().starts_with('.');
                if hidden || found.len() >= MAX_FILES {
                    continue;
                }
                if path.is_dir() {
                    if depth < MAX_DEPTH && entry.file_name() != "target" {
                        directories.push((path, depth + 1));
                    }
                } else if path.extension().is_some_and(|ending| ending == "typ") {
                    found.push(path);
                }
            }
        }
        found.sort();
        let items = found
            .into_iter()
            .map(|path| (self.name_of(&path), assist::Target::File(path)))
            .collect();
        self.open_picker("files", items, 0);
    }
}
