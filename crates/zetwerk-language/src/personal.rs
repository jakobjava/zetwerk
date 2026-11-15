//! The user's own words: names, terms, whatever no dictionary knows and
//! the user says is right.
//!
//! One file of one word a line. It is read again before every change,
//! so that two windows adding words do not lose each other's, and
//! written by renaming a finished file over it, so that a crash leaves
//! the list as it was or as it is to be, never half of it.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::words::Lexicon;

/// The user's own words.
#[derive(Debug, Clone, Default)]
pub struct Personal {
    /// The file they are kept in; without one they are kept as long as
    /// the program runs.
    path: Option<PathBuf>,
    words: Arc<BTreeSet<String>>,
}

/// The words `text` lists: one a line.
fn listed(text: &str) -> BTreeSet<String> {
    let lines = text.lines().map(str::trim);
    let words = lines.filter(|line| !line.is_empty() && !line.starts_with('#'));
    words.map(str::to_owned).collect()
}

/// The text of the file at `path`; empty if there is no such file.
fn read_text(path: &Path) -> std::io::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(err),
    }
}

/// The words in the file at `path`; none if there is no such file.
fn read(path: &Path) -> std::io::Result<BTreeSet<String>> {
    read_text(path).map(|text| listed(&text))
}

/// `text`, a list of words, made to list `words`: the lines of words no
/// longer among them are taken out, the words new to it are added at
/// the end, and every other line (comments, empty ones, the order the
/// user gave them) stays as it is.
fn relisted(text: &str, words: &BTreeSet<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut there = BTreeSet::new();
    for line in text.split_inclusive('\n') {
        let word = line.trim();
        if word.is_empty() || word.starts_with('#') || words.contains(word) {
            out.push_str(line);
            there.insert(word);
        }
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    for word in words.iter().filter(|word| !there.contains(word.as_str())) {
        out.push_str(word);
        out.push('\n');
    }
    out
}

/// Makes the file at `path` hold `text`, all of it or, if that fails,
/// as it was. Where `path` is a link, the file it leads to is written;
/// who may read and write it stays as it was.
fn write(path: &Path, text: &str) -> std::io::Result<()> {
    let path = &path.canonicalize().unwrap_or_else(|_| path.to_owned());
    let directory = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(directory)?;
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(format!(".{}.new", std::process::id()));
    let fresh = directory.join(name);
    let written = (|| {
        let mut file = std::fs::File::create(&fresh)?;
        file.write_all(text.as_bytes())?;
        if let Ok(old) = std::fs::metadata(path) {
            file.set_permissions(old.permissions())?;
        }
        file.sync_all()?;
        std::fs::rename(&fresh, path)?;
        // The new name is on the disk once its directory is.
        std::fs::File::open(directory)?.sync_all()
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&fresh);
    }
    written
}

impl Personal {
    /// The words kept in the file at `path`, and a complaint if there is
    /// such a file and it could not be read.
    #[must_use]
    pub fn open(path: PathBuf) -> (Self, Option<String>) {
        let (words, complaint) = match read(&path) {
            Ok(words) => (words, None),
            Err(err) => {
                let complaint = format!("{} was not read: {err}", path.display());
                (BTreeSet::new(), Some(complaint))
            }
        };
        let personal = Self {
            path: Some(path),
            words: Arc::new(words),
        };
        (personal, complaint)
    }

    /// The words, to hand to whoever checks.
    #[must_use]
    pub fn words(&self) -> Arc<BTreeSet<String>> {
        Arc::clone(&self.words)
    }

    /// Changes the list as `change` says, and writes it. What the file
    /// holds by now is what is changed. Returns whether `change` found
    /// something to do.
    fn change(
        &mut self,
        change: impl FnOnce(&mut BTreeSet<String>) -> bool,
    ) -> std::io::Result<bool> {
        let (text, mut words) = match &self.path {
            Some(path) => {
                let text = read_text(path)?;
                let words = listed(&text);
                (text, words)
            }
            None => (String::new(), (*self.words).clone()),
        };
        let changed = change(&mut words);
        if let (true, Some(path)) = (changed, &self.path) {
            write(path, &relisted(&text, &words))?;
        }
        self.words = Arc::new(words);
        Ok(changed)
    }

    /// Makes `word` a word. Returns whether it was none before; the
    /// error is why the list could not be written, and then it is as it
    /// was.
    pub fn add(&mut self, word: &str) -> std::io::Result<bool> {
        self.change(|words| words.insert(word.to_owned()))
    }

    /// Takes `word` from the list. Returns whether it was in it.
    pub fn remove(&mut self, word: &str) -> std::io::Result<bool> {
        self.change(|words| words.remove(word))
    }
}

/// Whether `words` hold `word`: as it is written, or, like dictionaries
/// have it, with a capital first letter where the list has it small.
pub(crate) fn listed_in(words: &BTreeSet<String>, word: &str) -> bool {
    if words.contains(word) {
        return true;
    }
    let mut letters = word.chars();
    let Some(first) = letters.next().filter(|first| first.is_uppercase()) else {
        return false;
    };
    let small: String = first.to_lowercase().chain(letters).collect();
    words.contains(&small)
}

impl Lexicon for Personal {
    fn knows(&self, word: &str) -> bool {
        listed_in(&self.words, word)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use super::*;
    use crate::dictionary::tests::scratch;

    #[test]
    fn words_are_kept_in_their_file() {
        let directory = scratch("words");
        let path = directory.join("deep/words.txt");
        let (mut personal, complaint) = Personal::open(path.clone());
        assert_eq!(complaint, None);
        assert!(!personal.knows("Typst"));
        assert!(personal.add("Typst").unwrap());
        assert!(!personal.add("Typst").unwrap());
        assert!(personal.add("zetwerk").unwrap());
        assert!(personal.knows("Typst") && personal.knows("zetwerk"));
        // A sentence may begin with it; a name stays a name.
        assert!(personal.knows("Zetwerk") && !personal.knows("typst"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Typst\nzetwerk\n");
        // Nothing is left lying about.
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );

        let (again, _) = Personal::open(path.clone());
        assert!(again.knows("Typst"));
        assert!(personal.remove("Typst").unwrap());
        assert!(!personal.remove("Typst").unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "zetwerk\n");
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn two_windows_do_not_lose_each_others_words() {
        let directory = scratch("shared");
        let path = directory.join("words.txt");
        let (mut one, _) = Personal::open(path.clone());
        let (mut other, _) = Personal::open(path.clone());
        one.add("eins").unwrap();
        other.add("zwei").unwrap();
        assert!(other.knows("eins") && other.knows("zwei"));
        one.remove("zwei").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "eins\n");
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_list_that_cannot_be_written_stays_as_it_was() {
        let directory = scratch("unwritable");
        // A file where the directory of the list would have to be.
        std::fs::write(directory.join("blocked"), "").unwrap();
        let (mut personal, _) = Personal::open(directory.join("blocked/words.txt"));
        assert!(personal.add("word").is_err());
        assert!(!personal.knows("word"));
        // Without a file the words last as long as the list does.
        let mut passing = Personal::default();
        assert!(passing.add("word").unwrap());
        assert!(passing.knows("word"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn comments_and_empty_lines_are_no_words() {
        assert_eq!(listed("# mine\n\n  Typst  \nzetwerk\n").len(), 2);
    }

    #[test]
    fn what_the_user_wrote_in_the_list_stays_as_it_was() {
        // `zg` rewrote the list as its words alone, sorted: the user's
        // comments and order were lost, and a link to a list kept with
        // the user's other settings became a file of its own.
        let directory = scratch("kept");
        let own = "# names\nZetwerk\n\n# terms of the trade\nkerning\nTypst";
        let kept = directory.join("dotfiles-words.txt");
        std::fs::write(&kept, own).unwrap();
        let path = directory.join("words.txt");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&kept, std::fs::Permissions::from_mode(0o600)).unwrap();
            std::os::unix::fs::symlink(&kept, &path).unwrap();
        }
        #[cfg(not(unix))]
        std::fs::copy(&kept, &path).unwrap();
        let (mut personal, _) = Personal::open(path.clone());
        assert!(personal.add("ligature").unwrap());
        let added = format!("{own}\nligature\n");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), added);
        assert!(personal.remove("kerning").unwrap());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# names\nZetwerk\n\n# terms of the trade\nTypst\nligature\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert!(std::fs::symlink_metadata(&path).unwrap().is_symlink());
            assert_eq!(
                std::fs::read_to_string(&kept).unwrap(),
                std::fs::read_to_string(&path).unwrap()
            );
            let mode = std::fs::metadata(&kept).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let _ = std::fs::remove_dir_all(directory);
    }
}
