//! The sources of a Hayagriva file (`.yml`), Typst's own way of keeping
//! them, read by the library Typst reads them with.
//!
//! They are made into entries as a BibTeX file has them, so that the
//! list, the card and citing work alike for both. A Hayagriva file is
//! added to, never rewritten: what is added is put behind all the file
//! holds, and the file is read again before it is written, to be sure
//! that it still says all it said. Changing or taking out one source of
//! such a file is left to the user's own editor: YAML keeps comments and
//! layout a library cannot give back.

use hayagriva::Entry as Source;
use hayagriva::types::Person;

use super::Entry;

/// The sources of `text`, a Hayagriva file.
///
/// # Errors
///
/// If it is no Hayagriva file Typst could read.
pub fn entries(text: &str) -> Result<Vec<Entry>, String> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let library = hayagriva::io::from_yaml_str(text).map_err(|err| err.to_string())?;
    Ok(library.iter().map(entry).collect())
}

/// Names as BibTeX writes them: `Family, Given and …`.
fn names(people: &[Person]) -> String {
    let named: Vec<String> = people
        .iter()
        .map(|person| {
            let family = match &person.prefix {
                Some(prefix) => format!("{prefix} {}", person.name),
                None => person.name.clone(),
            };
            match &person.given_name {
                Some(given) => format!("{family}, {given}"),
                None => family,
            }
        })
        .collect();
    named.join(" and ")
}

/// The BibTeX kind a Hayagriva entry of type `kind` is closest to, with
/// what it is part of.
fn kind(source: &Source) -> String {
    let named = |source: &Source| format!("{:?}", source.entry_type()).to_lowercase();
    let within = source.parents().first().map(named);
    match (named(source).as_str(), within.as_deref()) {
        ("article", Some("proceedings" | "conference")) => "inproceedings".to_owned(),
        ("article", _) => "article".to_owned(),
        ("chapter" | "anthos", _) => "incollection".to_owned(),
        ("thesis", _) => "phdthesis".to_owned(),
        ("report", _) => "techreport".to_owned(),
        ("web" | "post", _) => "online".to_owned(),
        (other, _) => other.to_owned(),
    }
}

/// One source of a Hayagriva file as an entry of a BibTeX file.
fn entry(source: &Source) -> Entry {
    let mut fields = Vec::new();
    let mut put = |name: &str, value: Option<String>| {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            fields.push((name.to_owned(), value));
        }
    };
    put("author", source.authors().map(names));
    put("title", source.title().map(ToString::to_string));
    let parent = source.parents().first();
    let within = if kind(source) == "article" {
        "journal"
    } else {
        "booktitle"
    };
    put(
        within,
        parent.and_then(Source::title).map(ToString::to_string),
    );
    put("editor", source.editors().map(names));
    put("year", source.date_any().map(|date| date.year.to_string()));
    put("volume", source.volume().map(ToString::to_string));
    put("number", source.issue().map(ToString::to_string));
    put("pages", source.page_range().map(ToString::to_string));
    let publisher = source.publisher().or_else(|| parent?.publisher());
    put(
        "publisher",
        publisher.and_then(|publisher| publisher.name().map(ToString::to_string)),
    );
    put("doi", source.doi().map(str::to_owned));
    put("isbn", source.isbn().map(str::to_owned));
    put("url", source.url_any().map(|url| url.value.to_string()));
    put("note", source.note().map(ToString::to_string));
    Entry {
        key: source.key().to_owned(),
        kind: kind(source),
        fields,
        range: 0..0,
    }
}

/// `text`, a Hayagriva file, with the sources of `bibtex` put behind
/// all it holds, as Hayagriva writes them.
///
/// # Errors
///
/// If Hayagriva cannot read `bibtex`, or the file would not say all it
/// said and the sources with it.
pub fn with_added(text: &str, bibtex: &str) -> Result<String, String> {
    let had = entries(text)?.len();
    let added = hayagriva::io::from_biblatex_str(bibtex).map_err(|errors| {
        let told: Vec<String> = errors.iter().map(|err| format!("{err:?}")).collect();
        format!("Hayagriva cannot read the sources: {}", told.join("; "))
    })?;
    let written = hayagriva::io::to_yaml_str(&added).map_err(|err| err.to_string())?;
    let written = written.trim_start_matches("---").trim();
    let kept = text.trim_end();
    let between = if kept.is_empty() { "" } else { "\n\n" };
    let whole = format!("{kept}{between}{written}\n");
    let now = entries(&whole).map_err(|err| format!("the file would not be read: {err}"))?;
    if now.len() != had + added.len() {
        return Err("the file would not say what it said: nothing is added".to_owned());
    }
    Ok(whole)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use super::*;

    const FILE: &str = "# my sources\nharry:\n  type: Book\n  title: Harry Potter and the Order of the Phoenix\n  \
        author: Rowling, J. K.\n  volume: 5\n  page-total: 768\n  date: 2003-06-21\n\n\
        electronic:\n  type: Web\n  title: Program Theory\n  author: Romero, Lucas\n  url: { value: https://www.lucas-romero.de, date: 2020-05-15 }\n\n\
        kinetics:\n  type: Article\n  title: Kinetics and luminescence of the excitations\n  \
        author: [Kim, Dong-Min, Okamoto, Koichi]\n  page-range: 11-15\n  serial-number:\n    doi: 10.1016/0022-2313(81)90134-X\n  \
        parent:\n    type: Periodical\n    title: Journal of Luminescence\n    volume: 25\n";

    #[test]
    fn hayagriva_sources_are_read_and_added_to() {
        let read = entries(FILE).unwrap();
        assert_eq!(read.len(), 3);
        assert_eq!(read[0].key, "harry");
        assert_eq!(read[0].kind, "book");
        assert_eq!(
            read[0].line(),
            "Rowling (2003) Harry Potter and the Order of the Phoenix"
        );
        assert_eq!(read[1].kind, "online");
        assert_eq!(read[1].field("url"), Some("https://www.lucas-romero.de/"));
        assert_eq!(read[2].field("journal"), Some("Journal of Luminescence"));
        assert_eq!(read[2].field("doi"), Some("10.1016/0022-2313(81)90134-X"));
        assert!(entries("not: [a, hayagriva").is_err());
        assert_eq!(entries("\n").unwrap(), Vec::new());
        // Added behind what is there, which stays as it was.
        let added = with_added(
            FILE,
            "@book{knuth1984, author = {Knuth, Donald E.}, title = {The {TeX}book}, year = {1984}}",
        )
        .unwrap();
        assert!(added.starts_with(FILE.trim_end()));
        let now = entries(&added).unwrap();
        assert_eq!(now.len(), 4);
        assert_eq!(now[3].key, "knuth1984");
        assert_eq!(now[3].who(), "Knuth");
        assert_eq!(now[3].year(), Some("1984"));
        assert!(with_added(FILE, "@book{harry, title = {Twice}}").is_err());
    }
}
