//! Whether two sources are one and the same.
//!
//! A source copied twice from two places is rarely written the same way:
//! braces, capitals, accents written as TeX or as letters, a DOI with
//! or without its `https://doi.org/`. Two sources are taken to be the
//! same if they have the same DOI, or the same title and the same year
//! once all that is set aside. A source handed over by its key (pasted,
//! imported, from an agent) is also the same as the one of the file
//! with that key unless they say they differ: in their titles or their
//! DOIs. Within one file, a key there twice is no pair to merge: it is
//! the error of a key, not of a source.

use std::collections::HashMap;

use super::Entry;

/// The DOI of `entry` as it is compared: in lower case, without what
/// may stand before it.
#[must_use]
pub fn doi(entry: &Entry) -> Option<String> {
    let said = entry.field("doi")?.trim().to_lowercase();
    let bare = ["https://", "http://"]
        .iter()
        .fold(said.as_str(), |doi, scheme| {
            doi.strip_prefix(scheme).unwrap_or(doi)
        });
    let bare = ["dx.doi.org/", "doi.org/", "doi:"]
        .iter()
        .fold(bare, |doi, before| doi.strip_prefix(before).unwrap_or(doi))
        .trim();
    bare.starts_with("10.").then(|| bare.to_owned())
}

/// The title and year of `entry` as they are compared; `None` if it has
/// no title.
#[must_use]
pub fn titled(entry: &Entry) -> Option<String> {
    let title = plain(entry.field("title")?);
    if title.is_empty() {
        return None;
    }
    Some(format!("{title}|{}", entry.year().unwrap_or("")))
}

/// `text` as it is compared: letters and digits in lower case, without
/// accents, one space between words; TeX's ways of writing letters
/// (`{\"o}`, `\ss`) read as the letters.
#[must_use]
pub fn plain(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            let mut command = String::new();
            while let Some(letter) = chars.peek().filter(|next| next.is_ascii_alphabetic()) {
                command.push(*letter);
                chars.next();
            }
            // The space that ends a command's name is no space.
            if !command.is_empty() && chars.peek() == Some(&' ') {
                chars.next();
            }
            match command.as_str() {
                // An accent or a sign written as a sign: it goes.
                "" => {
                    chars.next();
                }
                "ss" | "ae" | "oe" | "aa" | "o" | "l" | "i" | "j" => plain.push_str(&command),
                "AE" | "OE" | "AA" | "O" | "L" => plain.push_str(&command.to_lowercase()),
                // An accent written as a word (`\c{c}`), or anything else.
                _ => plain.push(' '),
            }
            continue;
        }
        // Braces only keep capitals or group: they are not there.
        if matches!(c, '{' | '}') {
            continue;
        }
        for lower in c.to_lowercase() {
            match folded(lower) {
                Some(letters) => plain.push_str(letters),
                None if lower.is_alphanumeric() => plain.push(lower),
                None => plain.push(' '),
            }
        }
    }
    plain.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The letters a letter with an accent is written as without it.
fn folded(c: char) -> Option<&'static str> {
    Some(match c {
        'à'..='å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'č' => "c",
        'ď' | 'đ' => "d",
        'è'..='ë' | 'ē' | 'ė' | 'ę' | 'ě' => "e",
        'ğ' => "g",
        'ì'..='ï' | 'ī' | 'į' | 'ı' => "i",
        'ł' | 'ľ' => "l",
        'ñ' | 'ń' | 'ň' => "n",
        'ò'..='ö' | 'ø' | 'ō' | 'ő' => "o",
        'œ' => "oe",
        'ř' => "r",
        'ß' => "ss",
        'ś' | 'š' | 'ş' => "s",
        'ť' | 'ţ' => "t",
        'ù'..='ü' | 'ū' | 'ů' | 'ű' | 'ų' => "u",
        'ý' | 'ÿ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        _ => return None,
    })
}

/// The sources of a file, by what they are compared by: to find the one
/// a new source is the same as at once, among thousands.
#[derive(Debug, Default)]
pub struct Index {
    dois: HashMap<String, usize>,
    titles: HashMap<String, usize>,
    /// By key: the place, and the DOI and title as compared.
    keys: HashMap<String, (usize, Option<String>, Option<String>)>,
}

impl Index {
    /// The index of `entries`; each is found by its place among them.
    #[must_use]
    pub fn of(entries: &[Entry]) -> Self {
        let mut index = Self::default();
        for (at, entry) in entries.iter().enumerate() {
            index.add(entry, at);
        }
        index
    }

    /// Lets `entry` be found as the one at `at`, unless one before it
    /// is found by the same.
    pub fn add(&mut self, entry: &Entry, at: usize) {
        if let Some(doi) = doi(entry) {
            self.dois.entry(doi).or_insert(at);
        }
        if let Some(titled) = titled(entry) {
            self.titles.entry(titled).or_insert(at);
        }
        let title = entry.field("title").map(plain);
        self.keys
            .entry(entry.key.clone())
            .or_insert((at, doi(entry), title));
    }

    /// The place of the source `entry` is the same as, if there is one.
    #[must_use]
    pub fn find(&self, entry: &Entry) -> Option<usize> {
        let by_doi = doi(entry).and_then(|doi| self.dois.get(&doi));
        let by_title = || titled(entry).and_then(|titled| self.titles.get(&titled));
        by_doi.or_else(by_title).copied()
    }

    /// The place of the source `entry`, handed over by its key, is the
    /// same as: as [`Index::find`], or the one with its key where they
    /// do not differ in their DOIs or titles.
    #[must_use]
    pub fn find_given(&self, entry: &Entry) -> Option<usize> {
        self.find(entry).or_else(|| {
            let (at, there_doi, there_title) = self.keys.get(&entry.key)?;
            let differ = |there: &Option<String>, given: Option<String>| {
                matches!((there, given), (Some(there), Some(given)) if *there != given)
            };
            let title = entry.field("title").map(plain);
            let apart = differ(there_doi, doi(entry)) || differ(there_title, title);
            (!entry.key.is_empty() && !apart).then_some(*at)
        })
    }
}

/// The sources of `entries` that are there twice: each that repeats one
/// before it, with the place of that one first.
#[must_use]
pub fn pairs(entries: &[Entry]) -> Vec<(usize, usize)> {
    let mut index = Index::default();
    let mut pairs = Vec::new();
    for (at, entry) in entries.iter().enumerate() {
        match index.find(entry) {
            Some(first) => pairs.push((first, at)),
            None => index.add(entry, at),
        }
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bib::parse;

    #[test]
    fn the_same_source_is_found_however_it_is_written() {
        assert_eq!(
            plain("{G}{\\\"o}del's {\\em Incompleteness}, Part~1"),
            "godel s incompleteness part 1"
        );
        assert_eq!(
            plain("Gödel’s INCOMPLETENESS: part 1."),
            "godel s incompleteness part 1"
        );
        assert_eq!(plain("Stra\\ss e und Straße"), "strasse und strasse");
        let entries = parse(
            "@article{a, title = {On {Computable} Numbers}, year = 1936, doi = {10.1112/PLMS/s2-42.1.230}}\n\
             @article{b, title = {On computable numbers.}, year = {1936}}\n\
             @misc{c, title = {Something else}, doi = {https://doi.org/10.1112/plms/s2-42.1.230}}\n\
             @misc{d, title = {On computable numbers}, year = {1937}}\n\
             @misc{e, title = {Untitled}}\n\
             @misc{f, author = {Nobody}}",
        );
        assert_eq!(
            doi(&entries[2]).as_deref(),
            Some("10.1112/plms/s2-42.1.230")
        );
        // By title and year; by DOI; not by title alone.
        assert_eq!(pairs(&entries), [(0, 1), (0, 2)]);
        let index = Index::of(&entries);
        assert_eq!(index.find(&entries[3]), Some(3));
        assert_eq!(index.find(&entries[5]), None);
        // Handed over by a key that is there: the same unless they say
        // they differ.
        let given = parse(
            "@misc{f, author = {Nobody, Else}}\n\
             @misc{e, title = {{U}ntitled.}, year = 1999}\n\
             @misc{e, title = {Titled}}\n\
             @misc{a, title = {Other}, doi = {10.1112/plms/s2-42.1.230}}\n\
             @misc{d, doi = {10.9/other}}",
        );
        let found: Vec<Option<usize>> = given.iter().map(|one| index.find_given(one)).collect();
        assert_eq!(found, [Some(5), Some(4), None, Some(0), Some(3)]);
        assert_eq!(index.find(&given[0]), None);
    }
}
