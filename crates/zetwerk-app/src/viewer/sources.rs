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
//! by the same way (see `zetwerk_mcp`).

use std::io::Write;
use std::path::{Path, PathBuf};

use super::agent::AgentEdits;
use super::assist::Target;
use super::{Edited, Viewer};
use crate::bib::{self, Entry};
use crate::typst_text;

/// The file sources are kept in where the document names none.
const USUAL_FILE: &str = "refs.bib";

/// Writes `text` to `path` so that the file is the old one or the new
/// one whatever happens: beside it first, then in its place.
fn write_whole(path: &Path, text: &str) -> Result<(), String> {
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
    text: String,
    /// The sources it holds.
    pub entries: Vec<Entry>,
    /// Whether the document names the file: lists what is cited of it.
    pub listed: bool,
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
        if !name.to_lowercase().ends_with(".bib") {
            return Err(format!(
                "the sources are in {name}: only BibTeX files (.bib) are managed here; \
                 what is in it can be cited with @ all the same"
            ));
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(err) => return Err(format!("cannot read {}: {err}", path.display())),
        };
        let entries = bib::parse(&text);
        Ok(Sources {
            path,
            text,
            entries,
            listed,
        })
    }

    /// Writes the sources' file as `text`, has the document list it if
    /// it does not yet, and has the pages made anew.
    fn write_sources(&mut self, sources: &Sources, text: &str) -> Result<(), String> {
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
        if self.file_changed(&sources.path) {
            self.edited = Some(Edited::Command);
        }
        Ok(())
    }

    /// Adds the sources written in `bibtex` (one entry or several) to
    /// the document's sources, for an agent. Each gets a key of its own
    /// where the one it has is taken or it has none. Returns the keys
    /// they can be cited by.
    ///
    /// # Errors
    ///
    /// If agents only propose changes, `bibtex` holds no entry, or the
    /// file cannot be read or written.
    pub fn agent_add_sources(&mut self, bibtex: &str) -> Result<Vec<String>, String> {
        if self.agent_edits == AgentEdits::Propose {
            return Err(
                "the user reviews what agents change (edit_mode = \"propose\"): a source \
                 cannot be proposed. Ask the user to add it (:bib), or to let agents \
                 change things (:agents live)."
                    .to_owned(),
            );
        }
        let new = bib::parse(bibtex);
        if new.is_empty() {
            return Err(
                "no BibTeX entry in that: an entry looks like @article{key, author = {…}, \
                 title = {…}, year = {…}}"
                    .to_owned(),
            );
        }
        let sources = self.sources()?;
        let mut taken: Vec<String> = sources
            .entries
            .iter()
            .map(|entry| entry.key.clone())
            .collect();
        let mut text = sources.text.clone();
        let mut keys = Vec::new();
        for mut entry in new {
            let free = !entry.key.is_empty() && !taken.contains(&entry.key);
            if !free {
                let taken: Vec<&str> = taken.iter().map(String::as_str).collect();
                entry.key = bib::key_for(&entry, &taken);
            }
            text = bib::with_added(&text, &entry);
            taken.push(entry.key.clone());
            keys.push(entry.key);
        }
        self.write_sources(&sources, &text)?;
        let name = sources.path.file_name().map(|name| name.to_string_lossy());
        self.notice(format!(
            "an agent added {} to {}  (:bib shows the sources)",
            keys.join(", "),
            name.unwrap_or_default()
        ));
        Ok(keys)
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
            _ => Err(format!("not a command: bib {command}")),
        };
        if let Err(why) = done {
            self.alert(why);
        }
    }

    /// `:bib`: the sources, and before them the way to add one.
    fn list_sources(&mut self) -> Result<(), String> {
        let sources = self.sources()?;
        let mut items = vec![(
            "+ add a source".to_owned(),
            Target::Command("bib add".to_owned()),
        )];
        for entry in &sources.entries {
            let open = Target::Command(format!("bib open {}", entry.key));
            items.push((format!("{}\t{}", entry.key, entry.line()), open));
        }
        self.open_picker("sources", items, usize::from(sources.entries.len() == 1));
        Ok(())
    }

    /// A new source of `kind`, with nothing known of it yet: it is put
    /// into the file, and shown to be filled in.
    fn new_source(&mut self, kind: &str) -> Result<(), String> {
        let sources = self.sources()?;
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
        self.write_sources(&sources, &text)?;
        self.open_source(&entry.key)
    }

    /// The source cited by `key` among the document's sources.
    fn source(&self, key: &str) -> Result<(Sources, Entry), String> {
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
    fn open_source(&mut self, key: &str) -> Result<(), String> {
        let (_, entry) = self.source(key)?;
        let mut items = vec![(
            "cite it where the cursor is".to_owned(),
            Target::Command(format!("bib cite {key}")),
        )];
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
        let chosen = lacking.map_or(0, |field| field + 1);
        self.open_picker_for("source", items, chosen, "bib".to_owned(), None);
        Ok(())
    }

    /// Asks what `field` of the source cited by `key` is to be.
    fn ask_field(&mut self, key: &str, field: &str) {
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
        self.write_sources(&sources, &text)?;
        self.open_source(&entry.key)
    }

    /// Takes the source cited by `key` out of the file.
    fn remove_source(&mut self, key: &str) -> Result<(), String> {
        let (sources, entry) = self.source(key)?;
        let text = bib::with_replaced(&sources.text, &entry.range, None);
        self.write_sources(&sources, &text)?;
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
