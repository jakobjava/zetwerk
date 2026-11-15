//! Looking after the sources: the list in the order wanted and narrowed
//! to what is wanted, how often each is cited (and which not at all),
//! where, and which are there twice.
//!
//! What is cited is read off the text of the document's files, as they
//! are now and with nothing compiled: `@key` and `<key>` handed to a
//! function, as `ref_text` finds them. A file thousands of sources long
//! is listed at once: it is read, compared and counted in one go each.

use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::ops::Range;
use std::path::{Path, PathBuf};

use typst::syntax::Source;

use super::Viewer;
use super::assist::Target;
use super::sources_add::Pending;
use crate::bib::{self, Entry, same};
use crate::ref_text::{self, Kind};

/// What the bibliography manager keeps from one command to the next.
pub(super) struct Kept {
    /// What reads the clipboard: the system's, or in a test one given.
    pub(super) clipboard: fn() -> Result<String, String>,
    /// The order the list is in.
    order: Order,
    /// What was pasted or imported, while it waits to be added.
    pub(super) pending: Option<Pending>,
    /// The pairs of sources the user wants both of, though they seem
    /// to be one.
    apart: HashSet<(String, String)>,
    /// The revision of the text when the file was last written: `u`
    /// right after that says how the file's change is taken back.
    pub(super) written_at: Option<u64>,
    /// Sources agents proposed, waiting for the user's word.
    pub(super) proposed: Vec<super::sources_proposed::Proposed>,
}

impl Default for Kept {
    fn default() -> Self {
        Self {
            clipboard: crate::clipboard::text,
            order: Order::default(),
            pending: None,
            apart: HashSet::new(),
            written_at: None,
            proposed: Vec::new(),
        }
    }
}

/// The order of the list of sources.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Order {
    /// As they are in the file.
    #[default]
    File,
    Key,
    Author,
    /// The newest first.
    Year,
    /// The most cited first.
    Cited,
}

/// What the list of sources is narrowed to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Only {
    All,
    /// Those not cited in the document.
    Unused,
    Cited,
    Kind(String),
    /// Those of the years from one to the other.
    Years(u32, u32),
    /// Those one of whose authors or editors has this in the name.
    Author(String),
}

impl Only {
    /// What `said` (`unused`, `kind book`, `year 1980-1989`, …) narrows
    /// the list to.
    fn of(said: &str) -> Result<Self, String> {
        let (what, rest) = said.split_once(' ').unwrap_or((said, ""));
        let rest = rest.trim();
        let year = |said: &str| said.trim().parse::<u32>().ok();
        Ok(match (what, rest) {
            ("" | "all", _) => Self::All,
            ("unused", _) => Self::Unused,
            ("cited", _) => Self::Cited,
            ("kind", kind) if !kind.is_empty() => Self::Kind(kind.to_lowercase()),
            ("author", name) if !name.is_empty() => Self::Author(same::plain(name)),
            ("year", years) => {
                let wrong = "a year is four digits: 1984, 1980-1989";
                if let Some((from, to)) = years.split_once('-') {
                    let to = if to.trim().is_empty() {
                        u32::MAX
                    } else {
                        year(to).ok_or(wrong)?
                    };
                    Self::Years(year(from).unwrap_or(0), to)
                } else {
                    let year = year(years).ok_or(wrong)?;
                    Self::Years(year, year)
                }
            }
            _ => return Err(format!("the list cannot be narrowed to: {said}")),
        })
    }

    /// Whether `entry`, cited `cited` times, is kept in the list.
    fn keeps(&self, entry: &Entry, cited: usize) -> bool {
        match self {
            Self::All => true,
            Self::Unused => cited == 0,
            Self::Cited => cited > 0,
            Self::Kind(kind) => entry.kind == *kind,
            Self::Years(from, to) => entry
                .year()
                .and_then(|year| year.parse::<u32>().ok())
                .is_some_and(|year| (*from..=*to).contains(&year)),
            Self::Author(name) => {
                let people = [entry.field("author"), entry.field("editor")];
                people
                    .iter()
                    .flatten()
                    .any(|people| same::plain(people).contains(name.as_str()))
            }
        }
    }

    /// What the list is called, narrowed so.
    fn title(&self) -> Option<&'static str> {
        Some(match self {
            Self::All => return None,
            Self::Unused => "sources not cited",
            Self::Cited => "sources cited",
            Self::Kind(_) => "sources of that kind",
            Self::Years(..) => "sources of those years",
            Self::Author(_) => "sources by that author",
        })
    }
}

/// "not cited", "cited once", "cited 3 times".
pub(super) fn times(cited: usize) -> String {
    match cited {
        0 => "not cited".to_owned(),
        1 => "cited once".to_owned(),
        _ => format!("cited {cited} times"),
    }
}

/// Those of `entries` that `only` keeps, cited as `counts` says, in
/// `order`.
fn listed<'a>(
    entries: &'a [Entry],
    counts: &HashMap<String, usize>,
    only: &Only,
    order: Order,
) -> Vec<&'a Entry> {
    let cited = |entry: &Entry| counts.get(&entry.key).copied().unwrap_or(0);
    let mut shown: Vec<&Entry> = entries
        .iter()
        .filter(|entry| only.keeps(entry, cited(entry)))
        .collect();
    match order {
        Order::File => {}
        Order::Key => shown.sort_by_cached_key(|entry| entry.key.to_lowercase()),
        Order::Author => shown.sort_by_cached_key(|entry| {
            (same::plain(&entry.who()), entry.year().map(str::to_owned))
        }),
        Order::Year => shown.sort_by_cached_key(|entry| {
            let year = entry.year().map(str::to_owned);
            (std::cmp::Reverse(year), same::plain(&entry.who()))
        }),
        Order::Cited => shown.sort_by_key(|entry| std::cmp::Reverse(cited(entry))),
    }
    shown
}

/// A citation of a source in the text.
struct Citation {
    key: String,
    /// The file it is in, and its bytes there.
    path: PathBuf,
    range: Range<usize>,
}

/// The citations in `files` of the sources whose keys are `keys`.
fn citations(files: &[(PathBuf, Source)], keys: &HashSet<&str>) -> Vec<Citation> {
    let mut found = Vec::new();
    for (path, source) in files {
        for (name, range) in ref_text::read(source).used {
            if keys.contains(name.as_str()) {
                found.push(Citation {
                    key: name,
                    path: path.clone(),
                    range,
                });
            }
        }
    }
    found
}

/// The words around bytes `range` of `text`, on their line.
fn around(text: &str, range: &Range<usize>) -> String {
    let start = text[..range.start].rfind('\n').map_or(0, |at| at + 1);
    let end = text[range.end..]
        .find('\n')
        .map_or(text.len(), |at| range.end + at);
    let before: String = {
        let before: Vec<char> = text[start..range.start].chars().collect();
        before[before.len().saturating_sub(36)..].iter().collect()
    };
    let behind: String = text[range.end..end].chars().take(24).collect();
    format!(
        "{}{}{}",
        before.trim_start(),
        &text[range.clone()],
        behind.trim_end()
    )
}

impl Viewer {
    /// The files of the document and their text as it is now: the one
    /// being edited, the main file and what the last compile read.
    fn document_files(&self) -> Vec<(PathBuf, Source)> {
        let whole = |path: &Path| std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
        let mut files = Vec::new();
        let mut seen = HashSet::new();
        if let Some(editor) = &self.editor {
            seen.insert(whole(editor.path()));
            files.push((editor.path().to_owned(), editor.source().clone()));
        }
        for path in self.project.dependencies() {
            let typst = path.extension().is_some_and(|ending| ending == "typ");
            if !typst || !seen.insert(whole(&path)) {
                continue;
            }
            let owned = self.project.file_id(&path);
            let owned = owned.and_then(|id| self.project.source(id).cloned());
            let read = || std::fs::read_to_string(&path).ok().map(Source::detached);
            if let Some(source) = owned.or_else(read) {
                files.push((path, source));
            }
        }
        files
    }

    /// How often each of `entries` is cited in the document, by key.
    pub(super) fn citation_counts_of(&self, entries: &[Entry]) -> HashMap<String, usize> {
        let keys: HashSet<&str> = entries.iter().map(|entry| entry.key.as_str()).collect();
        let mut counts: HashMap<String, usize> = HashMap::new();
        for citation in citations(&self.document_files(), &keys) {
            *counts.entry(citation.key).or_default() += 1;
        }
        counts
    }

    /// How often each source of the document's bibliography is cited
    /// in its text, by key; those not cited are not there.
    ///
    /// # Errors
    ///
    /// As [`Viewer::sources`].
    pub fn citation_counts(&self) -> Result<HashMap<String, usize>, String> {
        let sources = self.sources()?;
        Ok(self.citation_counts_of(&sources.entries))
    }

    /// The commands of the bibliography manager that look after the
    /// sources, and those that add many: `what` is the command, `rest`
    /// what follows it.
    pub(super) fn more_bib_command(&mut self, what: &str, rest: &str) -> Result<(), String> {
        let (first, second) = rest.split_once(' ').unwrap_or((rest, ""));
        match what {
            "paste" => self.paste_sources(),
            "import" => self.import_sources(rest),
            "take" => self.take_found(rest),
            "sort" => {
                self.sources_kept.order = match rest {
                    "file" | "" => Order::File,
                    "key" => Order::Key,
                    "author" => Order::Author,
                    "year" => Order::Year,
                    "cited" => Order::Cited,
                    _ => return Err(format!("the sources cannot be sorted by {rest}")),
                };
                self.list_sources()
            }
            "only" if matches!(rest, "year" | "author" | "kind") => self.offer_only(rest),
            "only" => self.list_sources_only(&Only::of(rest)?),
            "view" => self.offer_views(),
            "where" => self.where_cited(rest),
            "go" => self.go_to_citation(first, second),
            "duplicates" | "twice" => self.list_twice(),
            "pair" => self.offer_pair(first, second.trim()),
            "merge" => self.merge_sources(first, second.trim()),
            "apart" => {
                let pair = (first.to_owned(), second.trim().to_owned());
                self.sources_kept.apart.insert(pair);
                self.notice(format!("{first} and {} are both kept", second.trim()));
                self.list_twice()
            }
            "undo" => self.undo_sources(false),
            "undo!" => self.undo_sources(true),
            "redo" => self.redo_sources(),
            _ => self
                .broken_bib_command(what, rest)
                .or_else(|| self.proposed_bib_command(what, rest))
                .unwrap_or_else(|| Err(format!("not a command: bib {what}"))),
        }
    }

    /// `:bib`: the sources, and before them the ways to add some.
    pub(super) fn list_sources(&mut self) -> Result<(), String> {
        self.list_sources_only(&Only::All)
    }

    /// The sources that `only` keeps, in the order wanted.
    fn list_sources_only(&mut self, only: &Only) -> Result<(), String> {
        let sources = self.sources()?;
        let counts = self.citation_counts_of(&sources.entries);
        let cited = |entry: &Entry| counts.get(&entry.key).copied().unwrap_or(0);
        let order = self.sources_kept.order;
        let shown = listed(&sources.entries, &counts, only, order);
        let command =
            |label: &str, command: &str| (label.to_owned(), Target::Command(command.to_owned()));
        let mut items = vec![
            command("+ add a source", "bib add"),
            command(
                "+ paste from the clipboard\tBibTeX, RIS, CSL-JSON",
                "bib paste",
            ),
            command("+ import a file", "bib import"),
            command("… sort, narrow, twice, undo", "bib view"),
        ];
        // What agents propose, and what cannot be read, first.
        items.extend(self.proposed_rows());
        items.extend(super::sources_broken::unreadable_rows(&sources));
        let head = items.len();
        let clashing = super::sources_broken::clashing(&sources.entries);
        for entry in &shown {
            let open = Target::Command(format!("bib open {}", entry.key));
            let mark = match (clashing.get(entry.key.as_str()), cited(entry)) {
                (Some(count), _) => format!(" · {count} sources clash: cited by one key"),
                (None, 0) => " · unused".to_owned(),
                (None, count) => format!(" · {count}×"),
            };
            items.push((format!("{}{mark}\t{}", entry.key, entry.line()), open));
        }
        let title = only.title().unwrap_or(match order {
            Order::File => "sources",
            Order::Key => "sources by key",
            Order::Author => "sources by author",
            Order::Year => "sources, newest first",
            Order::Cited => "sources, most cited first",
        });
        let chosen = if shown.len() == 1 { head } else { 0 };
        if *only == Only::All {
            self.open_picker(title, items, chosen);
        } else {
            self.open_picker_for(title, items, chosen, "bib".to_owned(), None);
        }
        Ok(())
    }

    /// `:bib view`: the orders the list can be in, what it can be
    /// narrowed to, and what else there is to do with all the sources.
    fn offer_views(&mut self) -> Result<(), String> {
        let sources = self.sources()?;
        let counts = self.citation_counts_of(&sources.entries);
        let unused = sources
            .entries
            .iter()
            .filter(|entry| !counts.contains_key(&entry.key))
            .count();
        let order = self.sources_kept.order;
        let mut items = Vec::new();
        for (label, by, sorted) in [
            ("sort: as in the file", "file", Order::File),
            ("sort: by key", "key", Order::Key),
            ("sort: by author", "author", Order::Author),
            ("sort: newest first", "year", Order::Year),
            ("sort: most cited first", "cited", Order::Cited),
        ] {
            let now = if sorted == order { "\tnow" } else { "" };
            items.push((
                format!("{label}{now}"),
                Target::Command(format!("bib sort {by}")),
            ));
        }
        let only =
            |label: String, command: &str| (label, Target::Command(format!("bib only {command}")));
        items.push(only(format!("only those not cited\t{unused}"), "unused"));
        let cited = sources.entries.len() - unused;
        items.push(only(format!("only those cited\t{cited}"), "cited"));
        items.push(only("only of a kind…".to_owned(), "kind"));
        items.push(only("only of a year…".to_owned(), "year"));
        items.push(only("only by an author…".to_owned(), "author"));
        let twice = same::pairs(&sources.entries).len();
        items.push((
            format!("sources there twice\t{twice}"),
            Target::Command("bib duplicates".to_owned()),
        ));
        let undo = self.last_source_change();
        let undo = undo.unwrap_or_else(|| "nothing to take back".to_owned());
        items.push((
            format!("undo the last change to the file\t{undo}"),
            Target::Command("bib undo".to_owned()),
        ));
        self.open_picker_for("the sources", items, 0, "bib".to_owned(), None);
        Ok(())
    }

    /// `:bib only kind|year|author`: what there is of that to narrow
    /// the list to, each with how many; or something else, typed.
    fn offer_only(&mut self, by: &str) -> Result<(), String> {
        let sources = self.sources()?;
        let mut counts: HashMap<String, usize> = HashMap::new();
        for entry in &sources.entries {
            let said = match by {
                "kind" => Some(entry.kind.clone()),
                "year" => entry.year().map(str::to_owned),
                _ => Some(
                    entry
                        .who()
                        .split([' ', ','])
                        .next()
                        .unwrap_or("")
                        .to_owned(),
                ),
            };
            if let Some(said) = said.filter(|said| !said.is_empty()) {
                *counts.entry(said).or_default() += 1;
            }
        }
        let mut counted: Vec<(String, usize)> = counts.into_iter().collect();
        if by == "year" {
            counted.sort_by(|a, b| b.0.cmp(&a.0));
        } else {
            counted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        }
        let items: Vec<(String, Target)> = counted
            .into_iter()
            .map(|(said, count)| {
                let only = Target::Command(format!("bib only {by} {said}"));
                (format!("{said}\t{count}"), only)
            })
            .collect();
        if items.is_empty() {
            return Err(format!("no source says of what {by} it is"));
        }
        let free = Some(format!("bib only {by} "));
        let title = match by {
            "kind" => "of what kind",
            "year" => "of what year (or 1980-1989)",
            _ => "by whom",
        };
        self.open_picker_for(title, items, 0, "bib view".to_owned(), free);
        Ok(())
    }

    /// `:bib where <key>`: the places the source is cited, to go to.
    fn where_cited(&mut self, key: &str) -> Result<(), String> {
        let files = self.document_files();
        let keys = HashSet::from([key]);
        let mut items = Vec::new();
        for citation in citations(&files, &keys) {
            let Some((_, source)) = files.iter().find(|(path, _)| *path == citation.path) else {
                continue;
            };
            let text = source.text();
            // Under which heading, and in which file if not this one.
            let marks = ref_text::read(source);
            let heading = marks
                .targets
                .iter()
                .filter(|target| target.kind == Kind::Section)
                .rfind(|target| target.range.start <= citation.range.start)
                .map_or("before the first heading", |target| target.called.as_str());
            let within = if Some(citation.path.as_path())
                == self.editor.as_ref().map(crate::editor::Editor::path)
            {
                heading.to_owned()
            } else {
                let name = citation.path.file_name().map(|name| name.to_string_lossy());
                format!("{} · {heading}", name.unwrap_or_default())
            };
            let go = format!(
                "bib go {} {}",
                citation.range.start,
                citation.path.display()
            );
            items.push((
                format!("{}\t{within}", around(text, &citation.range)),
                Target::Command(go),
            ));
        }
        if items.is_empty() {
            self.open_source(key)?;
            self.notice(format!("{key} is not cited in the document yet"));
            return Ok(());
        }
        self.open_picker_for(
            "where it is cited",
            items,
            0,
            format!("bib open {key}"),
            None,
        );
        Ok(())
    }

    /// `:bib go <byte> <file>`: to a citation.
    fn go_to_citation(&mut self, at: &str, path: &str) -> Result<(), String> {
        let at: usize = at.parse().map_err(|_| format!("not a place: {at}"))?;
        let path = Path::new(path.trim());
        let whole = |path: &Path| std::path::absolute(path).ok();
        if let Some(editor) = &mut self.editor
            && whole(editor.path()) == whole(path)
        {
            editor.jump(at);
            self.follow_cursor();
            return Ok(());
        }
        let file = self
            .project
            .file_id(path)
            .ok_or_else(|| format!("{} is no file of the document", path.display()))?;
        self.open_file(file, Some(at));
        Ok(())
    }

    /// `:bib duplicates`: the sources that are there twice.
    fn list_twice(&mut self) -> Result<(), String> {
        let sources = self.sources()?;
        let entries = &sources.entries;
        let apart = &self.sources_kept.apart;
        let items: Vec<(String, Target)> = same::pairs(entries)
            .into_iter()
            .map(|(first, second)| (&entries[first], &entries[second]))
            .filter(|(first, second)| {
                !apart.contains(&(first.key.clone(), second.key.clone()))
                    && !apart.contains(&(second.key.clone(), first.key.clone()))
            })
            .map(|(first, second)| {
                let pair = Target::Command(format!("bib pair {} {}", first.key, second.key));
                (
                    format!("{} and {}\t{}", first.key, second.key, first.line()),
                    pair,
                )
            })
            .collect();
        if items.is_empty() {
            self.list_sources()?;
            self.notice("no source is there twice");
            return Ok(());
        }
        self.open_picker_for("there twice", items, 0, "bib".to_owned(), None);
        Ok(())
    }

    /// `:bib pair <a> <b>`: which of two sources that seem to be one to
    /// keep, or both.
    fn offer_pair(&mut self, first: &str, second: &str) -> Result<(), String> {
        let sources = self.sources()?;
        let counts = self.citation_counts_of(&sources.entries);
        let cited = |key: &str| times(counts.get(key).copied().unwrap_or(0));
        let items = vec![
            (
                format!(
                    "keep {first}, take {second} out\t{second}: {}",
                    cited(second)
                ),
                Target::Command(format!("bib merge {first} {second}")),
            ),
            (
                format!("keep {second}, take {first} out\t{first}: {}", cited(first)),
                Target::Command(format!("bib merge {second} {first}")),
            ),
            (
                "keep both".to_owned(),
                Target::Command(format!("bib apart {first} {second}")),
            ),
        ];
        self.open_picker_for("one source?", items, 0, "bib duplicates".to_owned(), None);
        Ok(())
    }

    /// `:bib merge <kept> <gone>`: takes `gone` out of the file, gives
    /// `kept` what only `gone` had, and has what cited `gone` in the
    /// file being edited cite `kept`.
    fn merge_sources(&mut self, kept: &str, gone: &str) -> Result<(), String> {
        let sources = self.sources()?;
        sources.writable()?;
        let find = |key: &str| {
            let entry = sources.entries.iter().find(|entry| entry.key == key);
            entry.ok_or_else(|| format!("the bibliography has no source {key}"))
        };
        let (keep, away) = (find(kept)?, find(gone)?);
        let mut merged = keep.clone();
        for (field, value) in &away.fields {
            if merged.field(field).is_none() {
                merged.set(field, value);
            }
        }
        let filled = merged.fields != keep.fields;
        // The one further on first, so that the other stays where it is.
        let mut text = sources.text.clone();
        let fill = |text: &str| {
            if filled {
                bib::with_replaced(text, &keep.range, Some(&merged))
            } else {
                text.to_owned()
            }
        };
        if keep.range.start > away.range.start {
            text = fill(&text);
            text = bib::with_replaced(&text, &away.range, None);
        } else {
            text = bib::with_replaced(&text, &away.range, None);
            text = fill(&text);
        }
        self.write_sources(&sources, &text, &format!("{gone} merged into {kept}"))?;
        let recited = self.recite(gone, kept)?;
        let counts = self.citation_counts_of(std::slice::from_ref(away));
        let left = counts.get(gone).copied().unwrap_or(0);
        self.list_sources()?;
        let mut told = format!("{kept} kept, {gone} taken out");
        if recited > 0 {
            let _ = write!(
                told,
                "; {recited} citations of it now cite {kept} (u takes them back)"
            );
        }
        if left > 0 {
            let _ = write!(told, "; still cited as {gone} in other files: {left}");
        }
        self.notice(format!("{told}  (:bib undo takes back the file)"));
        Ok(())
    }

    /// Has the citations of `gone` in the file being edited cite `kept`;
    /// returns how many there were.
    fn recite(&mut self, gone: &str, kept: &str) -> Result<usize, String> {
        let Some(editor) = &mut self.editor else {
            return Ok(0);
        };
        let text = editor.text();
        let edits: Vec<(Range<usize>, String)> = ref_text::read(editor.source())
            .used
            .into_iter()
            .filter(|(name, _)| name == gone)
            .rev()
            .map(|(_, range)| {
                let with = if text[range.clone()].starts_with('@') {
                    format!("@{kept}")
                } else {
                    format!("<{kept}>")
                };
                (range, with)
            })
            .collect();
        if edits.is_empty() {
            return Ok(0);
        }
        let edits: Vec<(Range<usize>, &str)> = edits
            .iter()
            .map(|(range, with)| (range.clone(), with.as_str()))
            .collect();
        editor
            .replace(&edits)
            .map_err(|why| format!("the citations were not changed: {why}"))?;
        self.text_changed(super::Edited::Command);
        Ok(edits.len())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use super::super::tests::{directory, open, press};
    use super::*;

    const FILE: &str = "@article{shannon1948, author = {Shannon, Claude}, title = {Communication}, \
        year = 1948, doi = {10.1002/x}}\n\
        @book{knuth1984, author = {Knuth, Donald}, title = {The TeXbook}, year = 1984}\n\
        @book{knuth1986, author = {Knuth, Donald}, title = {Computers and Typesetting}, \
        year = 1986, publisher = {Addison-Wesley}}\n\
        @misc{again, author = {C. Shannon}, title = {A theory}, year = 1949, \
        doi = {https://doi.org/10.1002/X}}\n";

    const PAPER: &str = "= Introduction\nAs @knuth1984 shows, and #cite(<shannon1948>).\n\n\
        = More\nAgain @knuth1984[p. 3]; and @again.\n\n#bibliography(\"refs.bib\")\n";

    fn chosen(viewer: &Viewer) -> Option<Target> {
        viewer
            .picker
            .as_ref()
            .and_then(|picker| picker.chosen().cloned())
    }

    #[test]
    fn the_list_is_sorted_and_narrowed() {
        let entries = bib::parse(FILE);
        let counts = HashMap::from([("knuth1984".to_owned(), 2), ("shannon1948".to_owned(), 1)]);
        let keys = |only: &Only, order: Order| -> Vec<&str> {
            listed(&entries, &counts, only, order)
                .into_iter()
                .map(|entry| entry.key.as_str())
                .collect()
        };
        assert_eq!(
            keys(&Only::All, Order::Year),
            ["knuth1986", "knuth1984", "again", "shannon1948"]
        );
        assert_eq!(
            keys(&Only::All, Order::Cited)[..2],
            ["knuth1984", "shannon1948"]
        );
        assert_eq!(keys(&Only::All, Order::Key)[0], "again");
        assert_eq!(
            keys(&Only::of("unused").unwrap(), Order::File),
            ["knuth1986", "again"]
        );
        assert_eq!(
            keys(&Only::of("kind book").unwrap(), Order::File),
            ["knuth1984", "knuth1986"]
        );
        assert_eq!(
            keys(&Only::of("year 1940-1950").unwrap(), Order::Author),
            ["shannon1948", "again"]
        );
        assert_eq!(
            keys(&Only::of("year 1986").unwrap(), Order::File),
            ["knuth1986"]
        );
        assert_eq!(
            keys(&Only::of("author Shannon").unwrap(), Order::File).len(),
            2
        );
        assert!(Only::of("year soon").is_err());
        assert!(Only::of("colour blue").is_err());
    }

    #[test]
    fn citations_are_counted_found_and_merged() {
        let directory = directory(PAPER);
        std::fs::write(directory.join("refs.bib"), FILE).unwrap();
        let (mut viewer, _) = open(&directory);
        let counts = viewer.citation_counts().unwrap();
        assert_eq!(counts.get("knuth1984"), Some(&2));
        assert_eq!(counts.get("shannon1948"), Some(&1));
        assert_eq!(counts.get("knuth1986"), None);
        // Where it is cited: Enter goes there.
        viewer.bib_command("where knuth1984");
        assert!(viewer.is_picking());
        press(&mut viewer, "<C-n><CR>");
        let second = PAPER.find("@knuth1984[").unwrap();
        assert_eq!(
            viewer.editor.as_ref().map(crate::editor::Editor::cursor),
            Some(second)
        );
        viewer.bib_command("where knuth1986");
        assert!(viewer.is_picking());
        assert!(viewer.message.as_ref().is_some_and(|message| message.text == "knuth1986 is not cited in the document yet"));
        viewer.picker = None;
        // There twice, by their DOIs: one is kept, with what only the
        // other said, and is cited where the other was.
        viewer.bib_command("duplicates");
        assert_eq!(
            chosen(&viewer),
            Some(Target::Command("bib pair shannon1948 again".to_owned()))
        );
        press(&mut viewer, "<CR><CR>");
        let sources = viewer.sources().unwrap();
        let keys: Vec<&str> = sources
            .entries
            .iter()
            .map(|entry| entry.key.as_str())
            .collect();
        assert_eq!(keys, ["shannon1948", "knuth1984", "knuth1986"]);
        assert_eq!(sources.entries[0].field("title"), Some("Communication"));
        assert!(
            viewer
                .text()
                .is_some_and(|text| text.contains("; and @shannon1948."))
        );
        assert_eq!(
            viewer.citation_counts().unwrap().get("shannon1948"),
            Some(&2)
        );
        viewer.picker = None;
        // Taken back: the file as it was.
        viewer.bib_command("undo");
        assert_eq!(
            std::fs::read_to_string(directory.join("refs.bib")).unwrap(),
            FILE
        );
        // Both kept: not offered again.
        viewer.bib_command("apart shannon1948 again");
        assert_eq!(viewer.sources().unwrap().entries.len(), 4);
        assert!(
            viewer
                .message
                .as_ref()
                .is_some_and(|message| message.text == "no source is there twice")
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_hayagriva_file_is_listed_cited_and_added_to() {
        let yaml = "# kept by hand\nharry:\n  type: Book\n  title: Harry Potter\n  \
            author: Rowling, J. K.\n  date: 2003\n";
        let directory = directory("Some words here.\n\n#bibliography(\"refs.yml\")\n");
        let path = directory.join("refs.yml");
        std::fs::write(&path, yaml).unwrap();
        let (mut viewer, _) = open(&directory);
        let sources = viewer.sources().unwrap();
        assert!(sources.hayagriva);
        assert_eq!(sources.entries[0].line(), "Rowling (2003) Harry Potter");
        // Cited from its card.
        press(&mut viewer, "/words<CR>gBharry<CR><CR>");
        assert!(
            viewer
                .text()
                .is_some_and(|text| text.starts_with("Some words @harry here."))
        );
        // Not changed here, nor taken out.
        viewer.bib_command("set harry title Other");
        assert!(viewer.message.as_ref().is_some_and(|message| message.alert));
        viewer.bib_command("remove! harry");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), yaml);
        // Added to, behind what is there.
        viewer
            .agent_add_sources(
                "@book{knuth1984, author = {Knuth, Donald}, title = {The TeXbook}, year = 1984}",
            )
            .unwrap();
        let now = std::fs::read_to_string(&path).unwrap();
        assert!(now.starts_with(yaml.trim_end()));
        assert_eq!(viewer.sources().unwrap().entries.len(), 2);
        viewer.bib_command("undo");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), yaml);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    #[ignore = "a measurement, not a test: a bibliography of thousands of sources"]
    fn measure() {
        use std::fmt::Write;
        use std::time::Instant;
        let mut file = String::new();
        let mut paper = String::from("= Paper\n");
        for at in 0..5000 {
            let _ = write!(
                file,
                "@article{{key{at},\n  author = {{Family{at}, Given and Other, One}},\n  \
                 title = {{A {{Title}} about thing number {at}}},\n  journal = {{Journal}},\n  \
                 year = {},\n  doi = {{10.1000/{at}}},\n}}\n\n",
                1950 + at % 70
            );
            if at % 10 == 0 {
                let _ = writeln!(paper, "Words @key{at} and more words.\n");
            }
        }
        paper.push_str("#bibliography(\"refs.bib\")\n");
        let directory = directory(&paper);
        std::fs::write(directory.join("refs.bib"), &file).unwrap();
        let (mut viewer, _) = open(&directory);
        eprintln!("{} bytes, 5000 sources, 500 citations", file.len());
        let mut best = |what: &str, part: &mut dyn FnMut(&mut Viewer)| {
            let took = (0..7).map(|_| {
                let begun = Instant::now();
                part(&mut viewer);
                begun.elapsed()
            });
            eprintln!("{what}: {:?}", took.min().unwrap());
        };
        best("the file read", &mut |viewer| {
            let _ = viewer.sources();
        });
        best("the citations counted", &mut |viewer| {
            let _ = viewer.citation_counts();
        });
        best("the list made (gB)", &mut |viewer| {
            viewer.bib_command("");
        });
        best("the list made, by author", &mut |viewer| {
            viewer.bib_command("sort author");
        });
        best("there twice", &mut |viewer| {
            viewer.bib_command("duplicates");
        });
        best("a narrowing key", &mut |viewer| {
            press(viewer, "<Esc>gBk");
        });
        let pasted: String = (0..200).fold(String::new(), |mut pasted, at| {
            let _ = writeln!(
                pasted,
                "@misc{{p{at}, title = {{Pasted {at}}}, year = 2020}}"
            );
            pasted
        });
        best("200 pasted, looked for", &mut |viewer| {
            viewer.picker = None;
            let _ = viewer.offer_copied(&pasted);
        });
        let begun = Instant::now();
        press(&mut viewer, "<CR>");
        eprintln!("200 added: {:?}", begun.elapsed());
        let _ = std::fs::remove_dir_all(directory);
    }
}
