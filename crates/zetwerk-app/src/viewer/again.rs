//! Another start of zetwerk asked for a file that is open here. It is
//! refused (one window per file, see `zetwerk_core::Document::open`) and
//! leaves word with [`zetwerk_core::knock`]: this window says so, and the
//! window asks for the user's attention.

use std::path::PathBuf;

use super::Viewer;

impl Viewer {
    /// Where another start of zetwerk leaves word for the files open
    /// here. Their directory is made if it is not there, so that it can
    /// be watched.
    #[must_use]
    pub fn knocks(&self) -> Vec<PathBuf> {
        let Some(state_dir) = &self.state_dir else {
            return Vec::new();
        };
        let knocks: Vec<PathBuf> = self
            .editor
            .iter()
            .chain(&self.others)
            .map(|editor| zetwerk_core::knock_path(editor.path(), state_dir))
            .collect();
        if let Some(directory) = knocks.first().and_then(|knock| knock.parent()) {
            let _ = zetwerk_core::create_private_dir(directory);
        }
        knocks
    }

    /// Takes up the word another start of zetwerk left, if one of `paths`
    /// (that something happened to) is such word. Returns whether one
    /// was: the window is then to ask for the user's attention.
    pub fn knocked(&mut self, paths: &[PathBuf]) -> bool {
        let knocks = self.knocks();
        let mut asked = Vec::new();
        for (knock, editor) in knocks.iter().zip(self.editor.iter().chain(&self.others)) {
            // Taking the word up is something happening to it as well.
            if paths.contains(knock) && std::fs::remove_file(knock).is_ok() {
                asked.push(self.name_of(editor.path()));
            }
        }
        if asked.is_empty() {
            return false;
        }
        self.alert(format!(
            "asked to open again: {} is open here (one window to a file)",
            asked.join(", ")
        ));
        true
    }
}
