//! Sources that come many at a time: pasted from the clipboard, imported
//! from another file, handed over by an agent.
//!
//! What is found is shown before anything is added: who, when, what,
//! and which of it is there already (the same DOI, the same title and
//! year, or the same key and nothing that tells them apart; see
//! `bib::same`). Enter adds what is new; what is there is kept
//! as it is, unless it is asked for once more. Each source added gets a
//! key that is free: its own where it has one that is not taken, else
//! one made of its author and year. A source typed in field by field is
//! looked for too, as soon as it says what it is.

use crate::bib::found::usable_key;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::Viewer;
use super::agent::AgentEdits;
use super::assist::Target;
use super::sources::Sources;
use crate::bib::found::{self, Found};
use crate::bib::{self, Entry, same};

/// Sources found and shown, that wait for the word to be added.
pub(super) struct Pending {
    found: Vec<Found>,
    /// The text they were found in, for the abbreviations it defines.
    copied: String,
    /// Where they were found: the clipboard, or the file named.
    from: Option<String>,
}

/// Which of the sources found are to be added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    /// Those that are not there yet.
    New,
    /// All of them.
    All,
    /// The one at this place, whether it is there or not.
    One(usize),
}

/// What became of sources that were to be added.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Added {
    /// The keys of those added, to cite them by.
    pub keys: Vec<String>,
    /// Those that were there already, and not added again.
    pub there: Vec<There>,
    /// Where they were proposed and not added: the proposal's number.
    pub proposal: Option<u64>,
}

/// A source that was there already.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct There {
    /// The key it was given as; empty if it had none.
    pub given: String,
    /// Who, when, what.
    pub line: String,
    /// The key of the one that is there.
    pub cited_as: String,
}

/// `named`, a file the user typed, as a path: from the folder `beside`
/// unless it says where it is.
fn typed_path(named: &str, beside: &Path) -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match (named.strip_prefix("~/"), home) {
        (Some(within), Some(home)) => home.join(within),
        _ => beside.join(named),
    }
}

/// "1 was", "3 were".
fn were(count: usize) -> String {
    format!("{count} {}", if count == 1 { "was" } else { "were" })
}

impl Viewer {
    /// `:bib paste`: the sources in the clipboard, shown to be added.
    pub(super) fn paste_sources(&mut self) -> Result<(), String> {
        let copied = (self.sources_kept.clipboard)()?;
        self.offer_copied(&copied)
    }

    /// The sources in `copied` (text the user copied), shown to be added.
    pub(super) fn offer_copied(&mut self, copied: &str) -> Result<(), String> {
        let (found, _) = found::read(copied);
        if found.is_empty() {
            return Err(
                "no source in the clipboard: copy one as BibTeX (@article{…}), RIS \
                 (TY  - JOUR …) or CSL-JSON"
                    .to_owned(),
            );
        }
        self.offer_found(found, copied, None)
    }

    /// `:bib import <file>`: the sources of another file, shown to be
    /// added; without a file, the files there are to choose from.
    pub(super) fn import_sources(&mut self, named: &str) -> Result<(), String> {
        let sources = self.sources()?;
        if named.is_empty() {
            self.choose_import(&sources);
            return Ok(());
        }
        // From where the file being edited is, as the files offered are.
        let beside = self
            .editor
            .as_ref()
            .and_then(|editor| editor.path().parent().map(Path::to_owned))
            .unwrap_or_default();
        let path = typed_path(named, &beside);
        let same_file = matches!(
            (path.canonicalize(), sources.path.canonicalize()),
            (Ok(path), Ok(own)) if path == own
        );
        if same_file {
            return Err(format!("{named} is the bibliography itself"));
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
        let found: Vec<Found> = if bib::ends_in(named, &["yml", "yaml"]) {
            let entries = bib::yaml::entries(&text).map_err(|err| format!("{named}: {err}"))?;
            let found = entries.into_iter().map(|entry| Found {
                entry,
                written: None,
            });
            found.collect()
        } else {
            found::read(&text).0
        };
        if found.is_empty() {
            return Err(format!("no source in {named}"));
        }
        self.offer_found(found, &text, Some(named.to_owned()))
    }

    /// The files beside the document that sources could be imported
    /// from; or the path of another, typed.
    fn choose_import(&mut self, sources: &Sources) {
        let own = sources.path.canonicalize().ok();
        let beside = self
            .editor
            .as_ref()
            .and_then(|editor| editor.path().parent().map(Path::to_owned))
            .unwrap_or_default();
        let mut items: Vec<(String, Target)> = self
            .files_beside(&["bib", "yml", "yaml", "ris", "json"])
            .into_iter()
            .filter(|file| beside.join(file).canonicalize().ok() != own)
            .map(|file| {
                let import = Target::Command(format!("bib import {file}"));
                (file, import)
            })
            .collect();
        if items.is_empty() {
            items.push((
                "no other bibliography beside the document: type the path of one".to_owned(),
                Target::Nothing,
            ));
        }
        let free = Some("bib import ".to_owned());
        self.open_picker_for("import which file", items, 0, "bib".to_owned(), free);
    }

    /// Shows `found` (found in `copied`, in the file `from` or else the
    /// clipboard), and which of it is there already, to be added.
    fn offer_found(
        &mut self,
        found: Vec<Found>,
        copied: &str,
        from: Option<String>,
    ) -> Result<(), String> {
        let sources = self.sources()?;
        let index = same::Index::of(&sources.entries);
        let there: Vec<Option<&str>> = found
            .iter()
            .map(|one| {
                index
                    .find_given(&one.entry)
                    .map(|at| sources.entries[at].key.as_str())
            })
            .collect();
        let new = there.iter().filter(|there| there.is_none()).count();
        let mut items = Vec::new();
        if new > 0 {
            let all = match (new, found.len()) {
                (1, 1) => "add it".to_owned(),
                (new, all) if new == all => format!("add all {all}"),
                (1, _) => "add the one that is new".to_owned(),
                (new, _) => format!("add the {new} that are new"),
            };
            let told = format!("{} there already", found.len() - new);
            items.push((
                format!("{all}\t{told}"),
                Target::Command("bib take new".to_owned()),
            ));
        }
        if new < found.len() {
            items.push((
                "add all of it, also what is there already".to_owned(),
                Target::Command("bib take all".to_owned()),
            ));
        }
        for (at, (one, there)) in found.iter().zip(&there).enumerate() {
            let now = there.map_or_else(|| "new".to_owned(), |key| format!("there: {key}"));
            let take = Target::Command(format!("bib take {at}"));
            items.push((format!("{}\t{now}", one.entry.line()), take));
        }
        let title = if from.is_some() {
            "found in the file"
        } else {
            "found in the clipboard"
        };
        let count = found.len();
        self.sources_kept.pending = Some(Pending {
            found,
            copied: copied.to_owned(),
            from,
        });
        self.open_picker_for(title, items, 0, "bib".to_owned(), None);
        self.notice(if new == 0 {
            format!("all {count} are there already: Escape adds nothing")
        } else {
            format!("{count} found: {new} new, {} there already", count - new)
        });
        Ok(())
    }

    /// `:bib take new|all|<n>`: adds what was found, as shown.
    pub(super) fn take_found(&mut self, which: &str) -> Result<(), String> {
        let pending = self
            .sources_kept
            .pending
            .take()
            .ok_or("nothing waits to be added: :bib paste or :bib import first")?;
        let which = match which {
            "new" | "" => Which::New,
            "all" => Which::All,
            number => Which::One(
                number
                    .parse()
                    .map_err(|_| format!("not one of what was found: {number}"))?,
            ),
        };
        let what = match &pending.from {
            Some(file) => format!("imported from {file}"),
            None => "pasted".to_owned(),
        };
        let added = self.add_found(&pending.found, &pending.copied, which, &what)?;
        let told = match (added.keys.as_slice(), added.there.len()) {
            ([], _) => "nothing added".to_owned(),
            ([key], 0) => format!("added as {key}"),
            ([key], there) => format!("added as {key}, {} there already", were(there)),
            (keys, 0) => format!("{} added", keys.len()),
            (keys, there) => format!("{} added, {} there already", keys.len(), were(there)),
        };
        if let [key] = added.keys.as_slice() {
            // One: to be cited, at once.
            self.open_source(key)?;
        } else {
            self.list_sources()?;
        }
        self.notice(format!("{told}  (:bib undo takes it back)"));
        Ok(())
    }

    /// Adds `which` of `found` (found in `copied`) to the sources' file,
    /// as `what` was done to it: those that are there already only if
    /// they are asked for one by one, or all are.
    fn add_found(
        &mut self,
        found: &[Found],
        copied: &str,
        which: Which,
        what: &str,
    ) -> Result<Added, String> {
        let sources = self.sources()?;
        let mut index = same::Index::of(&sources.entries);
        let mut keys: Vec<String> = sources
            .entries
            .iter()
            .map(|entry| entry.key.clone())
            .collect();
        let mut taken: HashSet<String> = keys.iter().cloned().collect();
        let mut added = Added::default();
        let mut written = Vec::new();
        let mut entries: Vec<Entry> = Vec::new();
        for (at, one) in found.iter().enumerate() {
            if matches!(which, Which::One(only) if only != at) {
                continue;
            }
            if which == Which::New
                && let Some(first) = index.find_given(&one.entry)
            {
                added.there.push(There {
                    given: one.entry.key.clone(),
                    line: one.entry.line(),
                    cited_as: keys[first].clone(),
                });
                continue;
            }
            let own = &one.entry.key;
            let key = if usable_key(own) && !taken.contains(own) {
                own.clone()
            } else {
                let taken: Vec<&str> = taken.iter().map(String::as_str).collect();
                bib::key_for(&one.entry, &taken)
            };
            let mut entry = one.entry.clone();
            key.clone_into(&mut entry.key);
            index.add(&entry, keys.len());
            keys.push(key.clone());
            taken.insert(key.clone());
            written.push(one.written_as(&key));
            entries.push(entry);
            added.keys.push(key);
        }
        if added.keys.is_empty() {
            return Ok(added);
        }
        // The abbreviations what is added uses, where the file has none
        // of those names.
        let defined: HashSet<String> = found::abbreviations(&sources.text)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        let needed = found::abbreviations(copied)
            .into_iter()
            .filter(|(name, _)| !defined.contains(name))
            .filter(|(name, _)| entries.iter().any(|entry| found::uses(entry, name)))
            .map(|(_, written)| written);
        let written: Vec<String> = needed.chain(written).collect();
        let text = if sources.hayagriva {
            bib::yaml::with_added(&sources.text, &written.join("\n\n"))?
        } else {
            found::with_put(&sources.text, &written)
        };
        self.write_sources(&sources, &text, what)?;
        Ok(added)
    }

    /// Adds the sources in `text` (BibTeX, RIS or CSL-JSON) for an agent:
    /// those that are there already only if `anyway`. Where agents only
    /// propose changes, they are proposed: nothing is written until the
    /// user takes them (see `sources_proposed`).
    ///
    /// # Errors
    ///
    /// If `text` holds no source, or the file cannot be read or written.
    pub fn agent_add_sources_checked(&mut self, text: &str, anyway: bool) -> Result<Added, String> {
        let (found, _) = found::read(text);
        if found.is_empty() {
            return Err(
                "no BibTeX entry in that: an entry looks like @article{key, author = {…}, \
                 title = {…}, year = {…}}"
                    .to_owned(),
            );
        }
        if self.agent_edits == AgentEdits::Propose {
            return Ok(self.propose_sources(text, anyway, &found));
        }
        self.add_agents_sources(text, &found, anyway)
    }

    /// Adds the sources `found` in `text` for an agent, as
    /// [`Viewer::agent_add_sources_checked`] does.
    pub(super) fn add_agents_sources(
        &mut self,
        text: &str,
        found: &[Found],
        anyway: bool,
    ) -> Result<Added, String> {
        let which = if anyway { Which::All } else { Which::New };
        let added = self.add_found(found, text, which, "added by an agent")?;
        let name = self.sources().ok().and_then(|sources| {
            let name = sources.path.file_name()?;
            Some(name.to_string_lossy().into_owned())
        });
        let mut told = Vec::new();
        if !added.keys.is_empty() {
            told.push(format!(
                "an agent added {} to {}",
                added.keys.join(", "),
                name.unwrap_or_default()
            ));
        }
        if !added.there.is_empty() {
            let there: Vec<&str> = added
                .there
                .iter()
                .map(|there| there.cited_as.as_str())
                .collect();
            told.push(format!("not again: {}", there.join(", ")));
        }
        self.notice(format!("{}  (:bib shows the sources)", told.join("; ")));
        Ok(added)
    }

    /// Where the source `key`, as typed in, is now one that is there
    /// already (and was not before, as `before`), asks which to keep.
    /// Returns whether it asked.
    pub(super) fn offer_kept_one(&mut self, before: &Entry, key: &str) -> bool {
        let Ok((sources, entry)) = self.source(key) else {
            return false;
        };
        let others: Vec<Entry> = sources
            .entries
            .iter()
            .filter(|other| other.key != key)
            .cloned()
            .collect();
        let index = same::Index::of(&others);
        let Some(there) = index.find(&entry).map(|at| &others[at]) else {
            return false;
        };
        if index.find(before).is_some() {
            return false;
        }
        let items = vec![
            (
                format!(
                    "keep the one that is there: {}\t{}",
                    there.key,
                    there.line()
                ),
                Target::Command(format!("bib merge {} {key}", there.key)),
            ),
            (
                format!("keep both\t{}", entry.line()),
                Target::Command(format!("bib open {key}")),
            ),
        ];
        let back = format!("bib open {key}");
        self.open_picker_for("it is there already", items, 0, back, None);
        self.notice(format!(
            "{} is there already: what this one has that it lacks is kept",
            there.key
        ));
        true
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use super::super::tests::{directory, open, press};
    use super::*;

    const MINE: &str = "% my own sources, in my own order\n\n\
        @book{knuth1984,\n    author = {Donald E. Knuth},\n    title = {The {TeX}book},\n    \
        year = 1984, publisher = aw\n}\n";

    fn message(viewer: &Viewer) -> String {
        viewer
            .message
            .as_ref()
            .map_or_else(String::new, |message| message.text.clone())
    }

    fn chosen(viewer: &Viewer) -> Option<Target> {
        viewer
            .picker
            .as_ref()
            .and_then(|picker| picker.chosen().cloned())
    }

    #[test]
    fn pasted_and_imported_sources_are_added_once_and_taken_back() {
        let directory = directory("Words @knuth1984 here.\n\n#bibliography(\"refs.bib\")\n");
        let refs = directory.join("refs.bib");
        std::fs::write(&refs, MINE).unwrap();
        let (mut viewer, _) = open(&directory);
        // Two copied: one is there already, by its title and year.
        let copied = "@Book{TeXbook, author = {Knuth, Donald}, title = {The TeXbook}, year = {1984}}\n\
            @article{knuth1984, author = {Shannon, C. E.}, title = {A Mathematical Theory of \
            Communication}, journal = bstj, year = 1948, month = jul}\n\
            @string{bstj = {Bell System Technical Journal}}\n@string{unused = {Not needed}}";
        viewer.offer_copied(copied).unwrap();
        assert_eq!(
            chosen(&viewer),
            Some(Target::Command("bib take new".to_owned()))
        );
        assert_eq!(message(&viewer), "2 found: 1 new, 1 there already");
        press(&mut viewer, "<CR>");
        let text = std::fs::read_to_string(&refs).unwrap();
        // What was there is as it was; what is added is as it was copied,
        // with a key of its own and the abbreviation it needs.
        assert!(text.starts_with(MINE.trim_end()), "{text}");
        assert!(text.contains(
            "@string{bstj = {Bell System Technical Journal}}\n\n@article{shannon1948, author"
        ));
        assert!(text.contains("journal = bstj, year = 1948, month = jul}"));
        assert!(!text.contains("Not needed"));
        assert_eq!(bib::parse(&text).len(), 2);
        assert!(
            message(&viewer).starts_with("added as shannon1948"),
            "{}",
            message(&viewer)
        );
        // The card of the one added is up, at what it lacks.
        assert_eq!(
            chosen(&viewer),
            Some(Target::Command("bib field shannon1948 volume".to_owned()))
        );
        viewer.picker = None;

        // Taken back, and made again.
        viewer.bib_command("undo");
        assert_eq!(std::fs::read_to_string(&refs).unwrap(), MINE);
        assert!(message(&viewer).starts_with("taken back: pasted"));
        viewer.bib_command("redo");
        assert_eq!(std::fs::read_to_string(&refs).unwrap(), text);
        viewer.bib_command("undo");
        viewer.bib_command("undo");
        assert!(message(&viewer).starts_with("nothing to take back"));
        assert_eq!(std::fs::read_to_string(&refs).unwrap(), MINE);

        // Imported from another file: the same is not added twice.
        std::fs::write(
            directory.join("other.bib"),
            "@misc{a, title = {First}, year = 2001, doi = {10.1/a}}\n\
             @misc{b, title = {Second}, year = 2002}\n\
             @misc{c, title = {The {T}e{X}book.}, year = 1984}\n",
        )
        .unwrap();
        viewer.bib_command("import other.bib");
        assert_eq!(
            chosen(&viewer),
            Some(Target::Command("bib take new".to_owned()))
        );
        press(&mut viewer, "<CR>");
        assert!(
            message(&viewer).starts_with("2 added, 1 was there already"),
            "{}",
            message(&viewer)
        );
        let keys: Vec<String> = viewer
            .sources()
            .unwrap()
            .entries
            .into_iter()
            .map(|entry| entry.key)
            .collect();
        assert_eq!(keys, ["knuth1984", "a", "b"]);
        viewer.picker = None;
        // Without a name: the files there are to choose from.
        viewer.bib_command("import");
        assert_eq!(
            chosen(&viewer),
            Some(Target::Command("bib import other.bib".to_owned()))
        );
        viewer.picker = None;
        assert!(viewer.import_sources("refs.bib").is_err());
        assert!(viewer.offer_copied("nothing to be found").is_err());

        // An agent is told what is there instead of adding it again.
        let again = "@misc{again, title = {Second}, year = 2002}";
        let added = viewer.agent_add_sources_checked(again, false).unwrap();
        assert_eq!(added.keys, Vec::<String>::new());
        assert_eq!(added.there[0].cited_as, "b");
        let added = viewer.agent_add_sources_checked(again, true).unwrap();
        assert_eq!(added.keys, ["again"]);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_source_typed_in_that_is_there_already_is_offered_to_be_kept() {
        let directory = directory("Words.\n\n#bibliography(\"refs.bib\")\n");
        let refs = directory.join("refs.bib");
        std::fs::write(&refs, MINE.replace(", publisher = aw", "")).unwrap();
        let (mut viewer, _) = open(&directory);
        viewer.bib_command("new book");
        press(&mut viewer, "<CR>Knuth<CR>");
        press(&mut viewer, "<CR>The TeXbook<CR>");
        press(&mut viewer, "<CR>Addison-Wesley<CR>");
        assert_eq!(viewer.sources().unwrap().entries.len(), 2);
        press(&mut viewer, "<CR>1984<CR>");
        assert_eq!(
            chosen(&viewer),
            Some(Target::Command("bib merge knuth1984 knuth1984a".to_owned()))
        );
        press(&mut viewer, "<CR>");
        let sources = viewer.sources().unwrap();
        assert_eq!(sources.entries.len(), 1);
        // What only the one typed in said is kept.
        assert_eq!(
            sources.entries[0].field("publisher"),
            Some("Addison-Wesley")
        );
        assert!(
            std::fs::read_to_string(&refs)
                .unwrap()
                .starts_with("% my own sources")
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_clipboard_is_pasted_from_by_the_command_and_never_read_in_a_test() {
        let directory = directory("Words.\n\n#bibliography(\"refs.yml\")\n");
        let refs = directory.join("refs.yml");
        let yaml = "# by hand\nharry:\n  type: Book\n  title: Harry Potter\n  date: 2003\n";
        std::fs::write(&refs, yaml).unwrap();
        let (mut viewer, _) = open(&directory);
        // No clipboard of the user's is read here.
        viewer.bib_command("paste");
        assert!(viewer.message.as_ref().is_some_and(|message| message.alert));
        assert!(!viewer.is_picking());
        // What a clipboard holds (RIS here) is shown, then added to the
        // Hayagriva file behind what it holds; what is there is not.
        viewer.sources_kept.clipboard = || {
            Ok("TY  - BOOK\nAU  - Rowling, J. K.\nTI  - Harry Potter\nPY  - 2003\nER  -\n\
                TY  - JOUR\nAU  - Shannon, Claude E.\nTI  - A Mathematical Theory\nPY  - 1948\nER  -\n"
                .to_owned())
        };
        viewer.bib_command("paste");
        assert_eq!(message(&viewer), "2 found: 1 new, 1 there already");
        press(&mut viewer, "<CR>");
        let now = std::fs::read_to_string(&refs).unwrap();
        assert!(now.starts_with(yaml.trim_end()), "{now}");
        let keys: Vec<String> = viewer
            .sources()
            .unwrap()
            .entries
            .into_iter()
            .map(|entry| entry.key)
            .collect();
        assert_eq!(keys, ["harry", "shannon1948"]);
        viewer.picker = None;
        viewer.bib_command("undo");
        assert_eq!(std::fs::read_to_string(&refs).unwrap(), yaml);
        let _ = std::fs::remove_dir_all(directory);
    }
}
