//! What of the bibliography file cannot be listed as it should: entries
//! that cannot be read (a brace left open, no key), and sources that are
//! cited by the same key.
//!
//! An entry that cannot be read is a row of its own in `:bib`, "cannot
//! be read", not left out unsaid; Enter on it offers to close it (where
//! it lacks a brace), to write it anew or to take it out. Nothing else
//! of the file changes. Two sources of one key clash: a citation can be
//! of one of them only. They are said to, and the later ones can be
//! given keys of their own.

use std::collections::HashMap;
use std::ops::Range;

use super::Viewer;
use super::assist::Target;
use super::sources::Sources;
use crate::bib::{self, Entry, broken};

/// The keys that more than one of `entries` has.
pub(super) fn clashing(entries: &[Entry]) -> HashMap<&str, usize> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for entry in entries {
        *counts.entry(entry.key.as_str()).or_default() += 1;
    }
    counts.retain(|_, count| *count > 1);
    counts
}

/// The rows of `:bib` for the entries of `sources` that cannot be read.
pub(super) fn unreadable_rows(sources: &Sources) -> Vec<(String, Target)> {
    if sources.hayagriva {
        return Vec::new();
    }
    broken::unreadable(&sources.text)
        .into_iter()
        .map(|range| {
            let words = broken::first_words(&sources.text[range.clone()]);
            let line = sources.text[..range.start].matches('\n').count() + 1;
            (
                format!("cannot be read: {words}\tline {line}"),
                Target::Command(format!("bib broken {}", range.start)),
            )
        })
        .collect()
}

impl Viewer {
    /// The commands for what cannot be listed as it should: `broken`,
    /// `mend`, `drop` and `anew` an entry that cannot be read (by the
    /// byte it begins at), `unclash` a key. Returns `None` where `what`
    /// is none of them.
    pub(super) fn broken_bib_command(
        &mut self,
        what: &str,
        rest: &str,
    ) -> Option<Result<(), String>> {
        let (first, typed) = rest.split_once(' ').unwrap_or((rest, ""));
        let at = first.parse::<usize>();
        Some(match (what, at) {
            ("broken", Ok(at)) => self.offer_mending(at),
            ("mend" | "drop" | "anew", Ok(at)) => self.mend_entry(what, at, typed),
            ("unclash", _) => self.unclash(first),
            ("broken" | "mend" | "drop" | "anew", Err(_)) => {
                Err(format!("bib {what}: the byte the entry begins at"))
            }
            _ => return None,
        })
    }

    /// The sources and the entry that cannot be read that begins at
    /// byte `at` of their file.
    fn unreadable_at(&self, at: usize) -> Result<(Sources, Range<usize>), String> {
        let sources = self.sources()?;
        sources.writable()?;
        let range = broken::unreadable(&sources.text)
            .into_iter()
            .find(|range| range.start == at)
            .ok_or("that entry is not there any more, or can be read now")?;
        Ok((sources, range))
    }

    /// What can be done with the entry that cannot be read at byte `at`.
    fn offer_mending(&mut self, at: usize) -> Result<(), String> {
        let (sources, range) = self.unreadable_at(at)?;
        let entry = &sources.text[range];
        let lacks = broken::unclosed(entry);
        let mut items = Vec::new();
        if lacks > 0 {
            let braces = "}".repeat(lacks);
            items.push((
                format!("close it: add the {braces} it lacks at its end"),
                Target::Command(format!("bib mend {at}")),
            ));
        }
        items.push((
            "take it out of the file".to_owned(),
            Target::Command(format!("bib drop {at}")),
        ));
        items.push((
            format!(
                "or type it anew, then Enter\t{}",
                broken::first_words(entry)
            ),
            Target::Nothing,
        ));
        let free = Some(format!("bib anew {at} "));
        self.open_picker_for("cannot be read", items, 0, "bib".to_owned(), free);
        Ok(())
    }

    /// Closes (`mend`), takes out (`drop`) or writes anew as `typed`
    /// (`anew`) the entry that cannot be read at byte `at`; the rest of
    /// the file stays as it is.
    fn mend_entry(&mut self, how: &str, at: usize, typed: &str) -> Result<(), String> {
        let (sources, range) = self.unreadable_at(at)?;
        let entry = &sources.text[range.clone()];
        let (with, end, told) = match how {
            "mend" => {
                let braces = "}".repeat(broken::unclosed(entry));
                (format!("{entry}{braces}"), range.end, "closed")
            }
            "drop" => {
                // With the line it ends, and the empty one behind it.
                let behind = &sources.text[range.end..];
                let gap = behind.len() - behind.trim_start_matches(['\n', '\r']).len();
                (String::new(), range.end + gap.min(2), "taken out")
            }
            _ => {
                let typed = typed.trim();
                if bib::parse(typed).len() != 1 {
                    self.offer_mending(at)?;
                    return Err(
                        "that is no entry that can be read: @kind{key, field = {…}, …}".to_owned(),
                    );
                }
                (typed.to_owned(), range.end, "written anew")
            }
        };
        let text = format!(
            "{}{with}{}",
            &sources.text[..range.start],
            &sources.text[end..]
        );
        let words = broken::first_words(entry);
        self.write_sources(&sources, &text, &format!("{words} {told}"))?;
        self.notice(format!("{words}: {told}  (:bib undo takes it back)"));
        self.list_sources()
    }

    /// Gives every source cited by `key` but the first a key of its own:
    /// what cites `key` keeps citing the first. Only the key is written
    /// anew; the rest of each entry stays as it is.
    fn unclash(&mut self, key: &str) -> Result<(), String> {
        let sources = self.sources()?;
        sources.writable()?;
        let mut taken: Vec<String> = sources.entries.iter().map(|e| e.key.clone()).collect();
        let mut text = sources.text.clone();
        let later: Vec<&Entry> = sources
            .entries
            .iter()
            .filter(|entry| entry.key == key)
            .skip(1)
            .collect();
        if later.is_empty() {
            return Err(format!("only one source is cited as {key}"));
        }
        let mut given = Vec::new();
        // From the last to the first: the places before stay good.
        for entry in later.into_iter().rev() {
            let written = &sources.text[entry.range.clone()];
            let Some(open) = written.find(['{', '(']) else {
                continue;
            };
            let Some(within) = written[open..].find(key) else {
                continue;
            };
            let names: Vec<&str> = taken.iter().map(String::as_str).collect();
            let mut renamed = entry.clone();
            renamed.key.clear();
            let new = bib::key_for(&renamed, &names);
            let start = entry.range.start + open + within;
            text.replace_range(start..start + key.len(), &new);
            taken.push(new.clone());
            given.push(new);
        }
        given.reverse();
        let what = format!("{key} given to one source only");
        self.write_sources(&sources, &text, &what)?;
        self.notice(format!(
            "{key} is the first one's; the others are cited as {}  (:bib undo takes it back)",
            given.join(", ")
        ));
        self.list_sources()
    }
}
