//! The bibliography manager: the sources of a document, in one list.
//!
//! `:bib` (or `gB`) lists the sources the document's bibliography file
//! holds, each with who wrote it, when, and what it is called. Enter on
//! one shows all that is known of it: any of it can be typed anew, the
//! source can be cited where the cursor is, or taken out. The first
//! entry of the list adds a source: what kind it is is chosen, what is
//! known of it is typed, and a key to cite it by is made of its author
//! and year.
//!
//! The file is a BibTeX file beside the document (`refs.bib`, unless the
//! document names another); the document is told to list it the first
//! time a source is added. Agents see the same sources and add to them
//! by the same way (see `zetwerk_mcp`). A Hayagriva file (`.yml`) is
//! read as well, and added to, but not changed (see `bib::yaml`).
//!
//! Sources that come many at a time (pasted, imported, from an agent)
//! are in `sources_add`; sorting, narrowing, what is cited where and
//! what is there twice in `sources_look`; the way back for the file in
//! `sources_undo`.

use std::io::Write;
use std::path::{Path, PathBuf};

use super::assist::Target;
use super::{Edited, Viewer};
use crate::bib::{self, Entry};
use crate::typst_text;

/// The file sources are kept in where the document names none.
const USUAL_FILE: &str = "refs.bib";

/// Writes `text` to `path` so that the file is the old one or the new
/// one whatever happens: beside it first, then in its place.
pub(super) fn write_whole(path: &Path, text: &str) -> Result<(), String> {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_owned());
    let name = path.file_name().map(|name| name.to_string_lossy());
    let name = name.ok_or_else(|| format!("{} is no file", path.display()))?;
    let beside = path.with_file_name(format!(".{name}.zetwerk-new"));
    let write = || -> std::io::Result<()> {
        let mut file = std::fs::File::create(&beside)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&beside, &path)
    };
    write().map_err(|err| {
        let _ = std::fs::remove_file(&beside);
        format!("cannot write {}: {err}", path.display())
    })
}

/// The sources of a document as its file holds them.
pub struct Sources {
    /// The file.
    pub path: PathBuf,
    /// What is in it.
    pub(super) text: String,
    /// The sources it holds.
    pub entries: Vec<Entry>,
    /// Whether the document names the file: lists what is cited of it.
    pub listed: bool,
    /// Whether it is a Hayagriva file (`.yml`), not a BibTeX file.
    pub hayagriva: bool,
}

impl Sources {
    /// Refuses a change that would rewrite a source of a Hayagriva
    /// file, saying what can be done instead.
    pub(super) fn writable(&self) -> Result<(), String> {
        if !self.hayagriva {
            return Ok(());
        }
        let name = self.path.file_name().map(|name| name.to_string_lossy());
        Err(format!(
            "{} is a Hayagriva file: sources are added to it (paste, import, agents), \
             changed or taken out only in it, by hand",
            name.unwrap_or_default()
        ))
    }
}

impl Viewer {
    /// The document's sources. Where it names no file for them, those
    /// of the usual file beside it, if there is one; else none.
    ///
    /// # Errors
    ///
    /// If no file is being edited, the file named is no BibTeX file, or
    /// it cannot be read.
    pub fn sources(&self) -> Result<Sources, String> {
        let editor = self.editor.as_ref().ok_or("no file is being edited")?;
        let folder = |file: &Path| file.parent().map(Path::to_owned).unwrap_or_default();
        let own = typst_text::bibliography_file(editor.source())
            .map(|named| (named, folder(editor.path())));
        // Named in the main file, where this is a chapter of it.
        let main = || {
            let source = self.project.source(self.project.main())?;
            let named = typst_text::bibliography_file(source)?;
            Some((named, folder(self.project.main_path()?)))
        };
        let named = own.or_else(main);
        let listed = named.is_some();
        let (name, folder) =
            named.unwrap_or_else(|| (USUAL_FILE.to_owned(), folder(editor.path())));
        let path = match name.strip_prefix('/') {
            Some(from_root) => self.project.root_dir().join(from_root),
            None => folder.join(&name),
        };
        let hayagriva = bib::ends_in(&name, &["yml", "yaml"]);
        if !hayagriva && !bib::ends_in(&name, &["bib"]) {
            return Err(format!(
                "the sources are in {name}: only BibTeX (.bib) and Hayagriva (.yml) files \
                 are managed here; what is in it can be cited with @ all the same"
            ));
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(err) => return Err(format!("cannot read {}: {err}", path.display())),
        };
        let entries = if hayagriva {
            bib::yaml::entries(&text).map_err(|err| format!("cannot read {name}: {err}"))?
        } else {
            bib::parse(&text)
        };
        Ok(Sources {
            path,
            text,
            entries,
            listed,
            hayagriva,
        })
    }

    /// Writes the sources' file as `text`, after keeping it as it was to
    /// go back to (`what` is done to it), has the document list it if it
    /// does not yet, and has the pages made anew.
    pub(super) fn write_sources(
        &mut self,
        sources: &Sources,
        text: &str,
        what: &str,
    ) -> Result<(), String> {
        self.keep_version(&sources.path, &sources.text, text, what)?;
        write_whole(&sources.path, text)?;
        if !sources.listed
            && let Some(editor) = &mut self.editor
        {
            let name = sources.path.file_name().map_or_else(
                || USUAL_FILE.to_owned(),
                |name| name.to_string_lossy().into_owned(),
            );
            let end = editor.text().len();
            let lead = if editor.text().ends_with('\n') || end == 0 {
                "\n"
            } else {
                "\n\n"
            };
            let call = format!("{lead}#bibliography(\"{name}\")\n");
            editor
                .replace(&[(end..end, call.as_str())])
                .map_err(|why| format!("the document could not be told of {name}: {why}"))?;
            self.text_changed(Edited::Command);
        }
        self.sources_written(&sources.path);
        Ok(())
    }

    /// Adds the sources written in `bibtex` (one entry or several) to
    /// the document's sources, for an agent; not those that are there
    /// already (see [`Viewer::agent_add_sources_checked`]). Each gets a
    /// key of its own where the one it has is taken or it has none.
    /// Returns the keys they can be cited by.
    ///
    /// # Errors
    ///
    /// If agents only propose changes, `bibtex` holds no entry, or the
    /// file cannot be read or written.
    pub fn agent_add_sources(&mut self, bibtex: &str) -> Result<Vec<String>, String> {
        self.agent_add_sources_checked(bibtex, false)
            .map(|added| added.keys)
    }

    /// The commands of the bibliography manager, without their `bib`.
    pub(super) fn bib_command(&mut self, command: &str) {
        let (what, rest) = match command.split_once(' ') {
            Some((what, rest)) => (what, rest.trim()),
            None => (command, ""),
        };
        let (key, more) = match rest.split_once(' ') {
            Some((key, more)) => (key, more.trim()),
            None => (rest, ""),
        };
        let done = match what {
            "" => self.list_sources(),
            "add" => {
                let kinds = bib::KINDS
                    .iter()
                    .map(|(listed, kind, _)| {
                        (
                            (*listed).to_owned(),
                            Target::Command(format!("bib new {kind}")),
                        )
                    })
                    .collect();
                self.open_picker_for("kind of source", kinds, 0, "bib".to_owned(), None);
                Ok(())
            }
            "new" => self.new_source(rest),
            "open" => self.open_source(key),
            "field" => {
                self.ask_field(key, more);
                Ok(())
            }
            "set" => {
                let (field, value) = more.split_once(' ').unwrap_or((more, ""));
                self.set_field(key, field, value.trim())
            }
            "cite" => self.cite(key),
            "remove" => {
                let items = vec![
                    (
                        "no, keep it".to_owned(),
                        Target::Command(format!("bib open {key}")),
                    ),
                    (
                        format!("yes, take {key} out of the bibliography"),
                        Target::Command(format!("bib remove! {key}")),
                    ),
                ];
                self.open_picker_for("take it out?", items, 0, format!("bib open {key}"), None);
                Ok(())
            }
            "remove!" => self.remove_source(key),
            _ => self.more_bib_command(what, rest),
        };
        if let Err(why) = done {
            self.alert(why);
        }
    }

    /// A new source of `kind`, with nothing known of it yet: it is put
    /// into the file, and shown to be filled in.
    fn new_source(&mut self, kind: &str) -> Result<(), String> {
        let sources = self.sources()?;
        sources.writable()?;
        let taken: Vec<&str> = sources
            .entries
            .iter()
            .map(|entry| entry.key.as_str())
            .collect();
        let mut entry = Entry {
            key: String::new(),
            kind: kind.to_owned(),
            fields: Vec::new(),
            range: 0..0,
        };
        entry.key = bib::key_for(&entry, &taken);
        let text = bib::with_added(&sources.text, &entry);
        self.write_sources(&sources, &text, &format!("added {}", entry.key))?;
        self.open_source(&entry.key)
    }

    /// The source cited by `key` among the document's sources.
    pub(super) fn source(&self, key: &str) -> Result<(Sources, Entry), String> {
        let sources = self.sources()?;
        let entry = sources
            .entries
            .iter()
            .find(|entry| entry.key == key)
            .cloned();
        let entry = entry.ok_or_else(|| format!("the bibliography has no source {key}"))?;
        Ok((sources, entry))
    }

    /// All that is known of the source cited by `key`, and what can be
    /// done with it.
    pub(super) fn open_source(&mut self, key: &str) -> Result<(), String> {
        let (sources, entry) = self.source(key)?;
        let cited = self.citation_counts_of(&sources.entries);
        let cited = cited.get(key).copied().unwrap_or(0);
        let mut items = vec![
            (
                "cite it where the cursor is".to_owned(),
                Target::Command(format!("bib cite {key}")),
            ),
            (
                format!("where it is cited\t{}", super::sources_look::times(cited)),
                Target::Command(format!("bib where {key}")),
            ),
        ];
        // What a source of its kind is asked for, then what else it has.
        let asked = bib::fields_of(&entry.kind);
        let others = entry
            .fields
            .iter()
            .map(|(field, _)| field.as_str())
            .filter(|field| !asked.contains(field));
        for field in asked.iter().copied().chain(others) {
            let now = entry.field(field).unwrap_or("–");
            let change = Target::Command(format!("bib field {key} {field}"));
            items.push((format!("{field}\t{now}"), change));
        }
        items.push((
            format!("cited as\t{key}"),
            Target::Command(format!("bib field {key} key")),
        ));
        items.push((
            "take it out of the bibliography".to_owned(),
            Target::Command(format!("bib remove {key}")),
        ));
        // The first thing it lacks comes up chosen: to be typed next.
        let lacking = asked.iter().position(|field| entry.field(field).is_none());
        let lacking = lacking.filter(|_| !sources.hayagriva);
        let chosen = lacking.map_or(0, |field| field + 2);
        self.open_picker_for("source", items, chosen, "bib".to_owned(), None);
        Ok(())
    }

    /// Asks what `field` of the source cited by `key` is to be.
    fn ask_field(&mut self, key: &str, field: &str) {
        if let Err(why) = self.sources().and_then(|sources| sources.writable()) {
            return self.alert(why);
        }
        let now = self
            .source(key)
            .ok()
            .and_then(|(_, entry)| entry.field(field).map(str::to_owned));
        let hint = match field {
            "author" | "editor" => "type the names, with `and` between them, then Enter",
            "key" => "type what it is to be cited by, then Enter",
            _ => "type it, then Enter  (a single - takes it away)",
        };
        let items = vec![(
            format!("{hint}\t{}", now.unwrap_or_default()),
            Target::Nothing,
        )];
        let free = format!("bib set {key} {field} ");
        self.open_picker_for("it is", items, 0, format!("bib open {key}"), Some(free));
    }

    /// Makes `field` of the source cited by `key` be `value`, and shows
    /// the source again. A source that had no key of its own gets one of
    /// its author and year as soon as it has those.
    fn set_field(&mut self, key: &str, field: &str, value: &str) -> Result<(), String> {
        let (sources, mut entry) = self.source(key)?;
        sources.writable()?;
        let before = entry.clone();
        let others: Vec<&str> = sources
            .entries
            .iter()
            .map(|entry| entry.key.as_str())
            .filter(|other| *other != key)
            .collect();
        if field == "key" {
            let fine = !value.is_empty()
                && value
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | ':' | '.'));
            if !fine || others.contains(&value) {
                self.open_source(key)?;
                return Err(format!(
                    "{value} cannot be what it is cited by: taken, or not letters and digits"
                ));
            }
            value.clone_into(&mut entry.key);
        } else {
            entry.set(field, if value == "-" { "" } else { value });
            let unnamed = key
                .strip_prefix("source")
                .is_some_and(|number| number.bytes().all(|byte| byte.is_ascii_digit()));
            if unnamed && !entry.who().is_empty() && entry.year().is_some() {
                entry.key = bib::key_for(&entry, &others);
            }
        }
        let text = bib::with_replaced(&sources.text, &entry.range, Some(&entry));
        let what = format!("{field} of {key} changed");
        self.write_sources(&sources, &text, &what)?;
        // Now that it says what it is, it may be one that is there.
        if self.offer_kept_one(&before, &entry.key) {
            return Ok(());
        }
        self.open_source(&entry.key)
    }

    /// Takes the source cited by `key` out of the file.
    fn remove_source(&mut self, key: &str) -> Result<(), String> {
        let (sources, entry) = self.source(key)?;
        sources.writable()?;
        let text = bib::with_replaced(&sources.text, &entry.range, None);
        self.write_sources(&sources, &text, &format!("{key} taken out"))?;
        self.notice(format!("{key} is out of the bibliography"));
        self.list_sources()
    }

    /// Cites the source `key` where the cursor is: behind the word it is
    /// on, before the full stop that may end it.
    fn cite(&mut self, key: &str) -> Result<(), String> {
        let editor = self.editor.as_mut().ok_or("no file is being edited")?;
        let (text, cursor) = (editor.text(), editor.cursor());
        let rest = text.get(cursor..).unwrap_or("");
        let word = &rest[..rest.find(char::is_whitespace).unwrap_or(rest.len())];
        let at = cursor
            + word
                .trim_end_matches(['.', ',', ';', ':', '!', '?', ')'])
                .len();
        let apart = at == 0 || text[..at].ends_with([' ', '\n', '(']);
        let cited = format!("{}@{key}", if apart { "" } else { " " });
        editor
            .replace(&[(at..at, cited.as_str())])
            .map_err(|why| format!("not cited: {why}"))?;
        self.text_changed(Edited::Command);
        self.notice(format!("cited: {key}  (u takes it back)"));
        Ok(())
    }
}
