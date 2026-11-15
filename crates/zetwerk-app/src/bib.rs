//! The sources of a document, as a BibTeX file holds them.
//!
//! Enough of BibTeX to list what a file holds, to add a source to it and
//! to change or take out one: entries with their fields, braces within
//! braces. What is not touched stays byte for byte what it was; an entry
//! that is changed is written anew, tidily.

use std::fmt::Write;
use std::ops::Range;

pub mod broken;
pub mod found;
pub mod same;
pub mod yaml;

/// One source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// What it is cited by.
    pub key: String,
    /// What kind of thing it is: `article`, `book`, …, in lower case.
    pub kind: String,
    /// Its fields, names in lower case, values without their braces or
    /// quotes, in the order they are written in.
    pub fields: Vec<(String, String)>,
    /// The bytes of the file it takes up.
    pub range: Range<usize>,
}

/// The kinds of sources that can be added, as they are listed, and the
/// fields each is asked for.
pub const KINDS: &[(&str, &str, &[&str])] = &[
    (
        "article in a journal",
        "article",
        &[
            "author", "title", "journal", "year", "volume", "number", "pages", "doi",
        ],
    ),
    (
        "book",
        "book",
        &["author", "title", "publisher", "year", "address", "isbn"],
    ),
    (
        "chapter of a book",
        "incollection",
        &[
            "author",
            "title",
            "booktitle",
            "editor",
            "publisher",
            "year",
            "pages",
        ],
    ),
    (
        "paper at a conference",
        "inproceedings",
        &["author", "title", "booktitle", "year", "pages", "doi"],
    ),
    (
        "thesis",
        "phdthesis",
        &["author", "title", "school", "year"],
    ),
    (
        "report",
        "techreport",
        &["author", "title", "institution", "year", "number"],
    ),
    (
        "page on the web",
        "online",
        &["author", "title", "url", "year", "urldate"],
    ),
    (
        "something else",
        "misc",
        &["author", "title", "howpublished", "year", "note"],
    ),
];

/// The fields a source of `kind` is asked for; of a kind not known
/// here, the ones nearly everything has.
#[must_use]
pub fn fields_of(kind: &str) -> &'static [&'static str] {
    KINDS
        .iter()
        .find(|(_, known, _)| *known == kind)
        .map_or(&["author", "title", "year", "note"], |(_, _, fields)| {
            fields
        })
}

/// Whether the file `name` ends in one of `endings` (without their dot),
/// in capitals or not.
#[must_use]
pub fn ends_in(name: &str, endings: &[&str]) -> bool {
    let ending = std::path::Path::new(name)
        .extension()
        .and_then(|ending| ending.to_str());
    ending.is_some_and(|ending| endings.iter().any(|one| ending.eq_ignore_ascii_case(one)))
}

impl Entry {
    /// What field `name` is, if it is anything.
    #[must_use]
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.as_str())
            .filter(|value| !value.trim().is_empty())
    }

    /// Makes field `name` be `value`; with nothing as its value, it goes.
    pub fn set(&mut self, name: &str, value: &str) {
        let value = value.trim();
        let at = self.fields.iter().position(|(field, _)| field == name);
        match (at, value.is_empty()) {
            (Some(at), true) => {
                self.fields.remove(at);
            }
            (Some(at), false) => value.clone_into(&mut self.fields[at].1),
            (None, false) => self.fields.push((name.to_owned(), value.to_owned())),
            (None, true) => {}
        }
    }

    /// Who wrote it, in short: the family name of the first author, and
    /// that there are more.
    #[must_use]
    pub fn who(&self) -> String {
        let Some(authors) = self.field("author").or_else(|| self.field("editor")) else {
            return String::new();
        };
        // "Family, Given" or "Given Family".
        let family = |name: &str| {
            let name = name.trim();
            let family = match name.split_once(',') {
                Some((family, _)) => family.trim(),
                None => name.rsplit(' ').next().unwrap_or(name),
            };
            family.trim_matches(['{', '}']).to_owned()
        };
        let all: Vec<&str> = authors.split(" and ").collect();
        match all.as_slice() {
            [one] => family(one),
            [one, other] => format!("{} and {}", family(one), family(other)),
            [first, ..] => format!("{} et al.", family(first)),
            [] => String::new(),
        }
    }

    /// The year it is of, if it says one: of a date, its first four
    /// digits.
    #[must_use]
    pub fn year(&self) -> Option<&str> {
        let said = self.field("year").or_else(|| self.field("date"))?;
        let digits = said.find(|c: char| c.is_ascii_digit())?;
        said.get(digits..digits + 4)
            .filter(|year| year.bytes().all(|byte| byte.is_ascii_digit()))
    }

    /// One line that says what it is: who, when, what.
    #[must_use]
    pub fn line(&self) -> String {
        let title = self.field("title").unwrap_or("(no title yet)");
        let title = title.replace(['{', '}'], "");
        match (self.who().as_str(), self.year()) {
            ("", None) => title,
            ("", Some(year)) => format!("({year}) {title}"),
            (who, None) => format!("{who}: {title}"),
            (who, Some(year)) => format!("{who} ({year}) {title}"),
        }
    }

    /// The entry as it is written into a file.
    #[must_use]
    pub fn written(&self) -> String {
        let mut written = format!("@{}{{{},\n", self.kind, self.key);
        for (name, value) in &self.fields {
            if !value.trim().is_empty() {
                let _ = writeln!(written, "  {name} = {{{}}},", balanced(value));
            }
        }
        written.push('}');
        written
    }
}

/// `value` with braces that pair up: what does not is left out, so that
/// no value ends the entry it is in.
fn balanced(value: &str) -> String {
    let mut depth = 0_usize;
    let mut kept = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '{' => depth += 1,
            '}' if depth == 0 => continue,
            '}' => depth -= 1,
            _ => {}
        }
        kept.push(c);
    }
    kept.extend(std::iter::repeat_n('}', depth));
    kept
}

/// The byte behind the bracket that closes the one opened just before
/// byte `from` of `text`.
fn closing(text: &str, from: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 1_usize;
    for (at, byte) in text.bytes().enumerate().skip(from) {
        if byte == open && open != close {
            depth += 1;
        } else if byte == close {
            depth -= 1;
            if depth == 0 {
                return Some(at + 1);
            }
        } else if byte == b'{' {
            // Braces count inside quotes and brackets alike.
            depth += usize::from(open != b'{');
        } else if byte == b'}' && open != b'{' {
            depth = depth.saturating_sub(1).max(1);
        }
    }
    None
}

/// The value that begins at byte `at` of `text` (between braces, between
/// quotes, or a bare word): what it says, and the byte behind it.
fn value(text: &str, at: usize) -> Option<(String, usize)> {
    match text.as_bytes().get(at)? {
        b'{' => {
            let end = closing(text, at + 1, b'{', b'}')?;
            Some((text.get(at + 1..end - 1)?.to_owned(), end))
        }
        b'"' => {
            let mut depth = 0_usize;
            for (offset, byte) in text.bytes().enumerate().skip(at + 1) {
                match byte {
                    b'{' => depth += 1,
                    b'}' => depth = depth.saturating_sub(1),
                    b'"' if depth == 0 => {
                        return Some((text.get(at + 1..offset)?.to_owned(), offset + 1));
                    }
                    _ => {}
                }
            }
            None
        }
        _ => {
            let rest = text.get(at..)?;
            let length = rest.find([',', '}', ')', '\n']).unwrap_or(rest.len());
            Some((rest[..length].trim().to_owned(), at + length))
        }
    }
}

/// The sources `text` (a BibTeX file) holds. What is no entry (comments,
/// abbreviations, anything broken) is passed over.
#[must_use]
pub fn parse(text: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut from = 0;
    while let Some(found) = text.get(from..).and_then(|rest| rest.find('@')) {
        let start = from + found;
        from = start + 1;
        let Some(head) = text.get(start + 1..) else {
            break;
        };
        let name = head
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(head.len());
        let kind = head[..name].to_lowercase();
        let open = start + 1 + name + (head[name..].len() - head[name..].trim_start().len());
        let (opened, closed) = match text.as_bytes().get(open) {
            Some(b'{') => (b'{', b'}'),
            Some(b'(') => (b'(', b')'),
            _ => continue,
        };
        let Some(end) = closing(text, open + 1, opened, closed) else {
            continue;
        };
        // One left open runs on into the next: it is not read, and the
        // next is (see `broken`).
        if broken::runs_on(&text[start..end]) {
            continue;
        }
        from = end;
        if kind.is_empty() || matches!(kind.as_str(), "comment" | "string" | "preamble") {
            continue;
        }
        let body = &text[open + 1..end - 1];
        let Some((key, rest)) = body.split_once(',') else {
            continue;
        };
        let mut fields = Vec::new();
        let mut at = open + 1 + key.len() + 1;
        let body_end = end - 1;
        while at < body_end {
            let Some(rest_of) = text.get(at..body_end) else {
                break;
            };
            let Some(equals) = rest_of.find('=') else {
                break;
            };
            let field = rest_of[..equals].trim().trim_start_matches(',').trim();
            let after = at + equals + 1;
            let after =
                after + (text[after..body_end].len() - text[after..body_end].trim_start().len());
            let Some((said, behind)) = value(text, after) else {
                break;
            };
            if !field.is_empty()
                && field
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
            {
                fields.push((field.to_lowercase(), said));
            }
            at = behind;
        }
        let _ = rest;
        entries.push(Entry {
            key: key.trim().to_owned(),
            kind,
            fields,
            range: start..end,
        });
    }
    entries
}

/// A key for `entry` that none of `taken` has: the family name of who
/// wrote it and the year, in lower case, with a letter behind it if
/// that is taken.
#[must_use]
pub fn key_for(entry: &Entry, taken: &[&str]) -> String {
    let who: String = entry
        .who()
        .split(' ')
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    let base = match (who.as_str(), entry.year()) {
        ("", None) => "source".to_owned(),
        ("", Some(year)) => format!("source{year}"),
        (who, None) => who.to_owned(),
        (who, Some(year)) => format!("{who}{year}"),
    };
    if !taken.contains(&base.as_str()) && base != "source" {
        return base;
    }
    let lettered = ('a'..='z').map(|letter| format!("{base}{letter}"));
    let numbered = (1..).map(|number| format!("{base}{number}"));
    let mut tried = lettered.filter(|_| base != "source").chain(numbered);
    tried
        .find(|key| !taken.contains(&key.as_str()))
        .unwrap_or(base)
}

/// `text` with `entry` put behind all it holds.
#[must_use]
pub fn with_added(text: &str, entry: &Entry) -> String {
    let kept = text.trim_end();
    let between = if kept.is_empty() { "" } else { "\n\n" };
    format!("{kept}{between}{}\n", entry.written())
}

/// `text` with the entry at bytes `range` of it written anew as `entry`;
/// or, with none, taken out.
#[must_use]
pub fn with_replaced(text: &str, range: &Range<usize>, entry: Option<&Entry>) -> String {
    let (Some(before), Some(behind)) = (text.get(..range.start), text.get(range.end..)) else {
        return text.to_owned();
    };
    if let Some(entry) = entry {
        return format!("{before}{}{behind}", entry.written());
    }
    let before = before.trim_end();
    let behind = behind.trim_start();
    let between = if before.is_empty() || behind.is_empty() {
        ""
    } else {
        "\n\n"
    };
    let end = if behind.is_empty() && !before.is_empty() {
        "\n"
    } else {
        ""
    };
    format!("{before}{between}{behind}{end}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "% my sources\n@string{jmlr = \"JMLR\"}\n\n\
        @article{knuth1984,\n  author = {Knuth, Donald E. and Plass, Michael F.},\n  \
        title = {Breaking {Paragraphs} into Lines},\n  journal = \"Software\",\n  year = 1981,\n}\n\n\
        @Book(lamport94,\n  author = \"Leslie Lamport\",\n  title = {LaTeX},\n  year = {1994}\n)\n";

    #[test]
    fn entries_are_read_with_their_fields() {
        let entries = parse(FILE);
        assert_eq!(entries.len(), 2);
        let knuth = &entries[0];
        assert_eq!(
            (knuth.key.as_str(), knuth.kind.as_str()),
            ("knuth1984", "article")
        );
        assert_eq!(
            knuth.field("title"),
            Some("Breaking {Paragraphs} into Lines")
        );
        assert_eq!(knuth.field("journal"), Some("Software"));
        assert_eq!(knuth.field("year"), Some("1981"));
        assert_eq!(
            knuth.line(),
            "Knuth and Plass (1981) Breaking Paragraphs into Lines"
        );
        let lamport = &entries[1];
        assert_eq!(
            (lamport.key.as_str(), lamport.kind.as_str()),
            ("lamport94", "book")
        );
        assert_eq!(lamport.who(), "Lamport");
        assert_eq!(&FILE[lamport.range.clone()][..6], "@Book(");
        // Nothing of what is broken, and no panic.
        assert_eq!(parse("@article{half, title = {no end").len(), 0);
        assert_eq!(parse("@ @{} @x{y} text").len(), 0);
    }

    #[test]
    fn sources_are_added_changed_and_taken_out() {
        let mut entry = Entry {
            key: String::new(),
            kind: "article".to_owned(),
            fields: Vec::new(),
            range: 0..0,
        };
        entry.set(
            "author",
            "Ada Lovelace and Charles Babbage and Someone Else",
        );
        entry.set("title", "Notes on the {Engine}}");
        entry.set("year", "1843");
        entry.key = key_for(&entry, &["knuth1984"]);
        assert_eq!(entry.key, "lovelace1843");
        assert_eq!(key_for(&entry, &["lovelace1843"]), "lovelace1843a");
        let added = with_added(FILE, &entry);
        let entries = parse(&added);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[2].field("title"), Some("Notes on the {Engine}"));
        assert_eq!(entries[2].who(), "Lovelace et al.");
        assert!(added.starts_with(FILE.trim_end()));
        // Changed: only it is written anew.
        let mut knuth = entries[0].clone();
        knuth.set("year", "1984");
        knuth.set("journal", "");
        let changed = with_replaced(&added, &entries[0].range, Some(&knuth));
        let again = parse(&changed);
        assert_eq!(again[0].field("year"), Some("1984"));
        assert_eq!(again[0].field("journal"), None);
        assert_eq!(again[1], {
            let mut lamport = entries[1].clone();
            lamport.range = again[1].range.clone();
            lamport
        });
        assert!(changed.contains("@Book(lamport94"));
        // Taken out.
        let fewer = with_replaced(&changed, &again[1].range, None);
        let left = parse(&fewer);
        assert_eq!(left.len(), 2);
        assert!(!fewer.contains("lamport94"));
        // Into a file with nothing in it.
        assert_eq!(parse(&with_added("", &entry)).len(), 1);
        let unnamed = Entry {
            key: String::new(),
            kind: "misc".to_owned(),
            fields: Vec::new(),
            range: 0..0,
        };
        assert_eq!(key_for(&unnamed, &[]), "source1");
        assert_eq!(key_for(&unnamed, &["source1"]), "source2");
    }
}
