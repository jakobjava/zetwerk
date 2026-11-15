//! Sources found in text that was copied from elsewhere: a publisher's
//! page, a reference manager, another bibliography.
//!
//! BibTeX is what most of them give; RIS (`TY  - JOUR` …) and CSL-JSON
//! (what Zotero, Mendeley and the DOI services give as JSON) are read
//! too, and made into BibTeX entries. A BibTeX entry is put into the
//! file as it was written, only its key changed where that is taken:
//! what it says is the user's, not to be tidied.

use super::{Entry, closing};

/// What a source found in copied text was written as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    BibTex,
    Ris,
    CslJson,
}

impl Format {
    /// What it is called.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::BibTex => "BibTeX",
            Self::Ris => "RIS",
            Self::CslJson => "CSL-JSON",
        }
    }
}

/// A source found in copied text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// What is known of it; its range is in the text it was found in.
    pub entry: Entry,
    /// The entry as it was written, where it was written as BibTeX.
    pub written: Option<String>,
}

impl Found {
    /// The entry as it is to be put into a BibTeX file, cited by `key`.
    #[must_use]
    pub fn written_as(&self, key: &str) -> String {
        if let Some(written) = &self.written
            && let Some(rekeyed) = rekeyed(written, key)
        {
            return rekeyed;
        }
        let mut entry = self.entry.clone();
        key.clone_into(&mut entry.key);
        entry.written()
    }
}

/// `written`, a BibTeX entry, with `key` as its key.
fn rekeyed(written: &str, key: &str) -> Option<String> {
    let open = written.find(['{', '('])? + 1;
    let comma = open + written.get(open..)?.find(',')?;
    Some(format!("{}{key}{}", &written[..open], &written[comma..]))
}

/// The sources in `text`, and what they were written as; none if it
/// holds no source in any of the ways known here.
#[must_use]
pub fn read(text: &str) -> (Vec<Found>, Option<Format>) {
    let trimmed = text.trim_start();
    if trimmed.starts_with(['[', '{'])
        && let Some(found) = csl_json(trimmed)
    {
        return (found, Some(Format::CslJson));
    }
    if text
        .lines()
        .any(|line| tag(line).is_some_and(|(tag, _)| tag == "TY"))
    {
        let found = ris(text);
        if !found.is_empty() {
            return (found, Some(Format::Ris));
        }
    }
    let found: Vec<Found> = super::parse(text)
        .into_iter()
        .map(|entry| Found {
            written: text.get(entry.range.clone()).map(str::to_owned),
            entry,
        })
        .collect();
    let format = (!found.is_empty()).then_some(Format::BibTex);
    (found, format)
}

/// The abbreviations `text` (BibTeX) defines (`@string{jmlr = …}`): the
/// name of each, and the definition as it is written.
#[must_use]
pub fn abbreviations(text: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = text.get(from..).and_then(|rest| rest.find('@')) {
        let start = from + at;
        from = start + 1;
        let head = &text[start + 1..];
        if !head
            .get(..6)
            .is_some_and(|word| word.eq_ignore_ascii_case("string"))
        {
            continue;
        }
        let gap = head[6..].len() - head[6..].trim_start().len();
        let open = start + 7 + gap;
        let (opened, closed) = match text.as_bytes().get(open) {
            Some(b'{') => (b'{', b'}'),
            Some(b'(') => (b'(', b')'),
            _ => continue,
        };
        let Some(end) = closing(text, open + 1, opened, closed) else {
            continue;
        };
        let body = &text[open + 1..end - 1];
        if let Some((name, _)) = body.split_once('=') {
            found.push((name.trim().to_lowercase(), text[start..end].to_owned()));
        }
        from = end;
    }
    found
}

/// Whether `entry` uses the abbreviation `name` in one of its fields.
#[must_use]
pub fn uses(entry: &Entry, name: &str) -> bool {
    entry.fields.iter().any(|(_, value)| {
        value
            .split('#')
            .any(|part| part.trim().eq_ignore_ascii_case(name))
    })
}

/// `text` (a BibTeX file) with `written` (entries, and what they need)
/// put behind all it holds, in one go: thousands at once cost no more
/// than one.
#[must_use]
pub fn with_put(text: &str, written: &[String]) -> String {
    let kept = text.trim_end();
    let mut whole =
        String::with_capacity(kept.len() + written.iter().map(String::len).sum::<usize>());
    whole.push_str(kept);
    for one in written {
        if !whole.is_empty() {
            whole.push_str("\n\n");
        }
        whole.push_str(one.trim());
    }
    whole.push('\n');
    whole
}

/// A value as BibTeX reads it in a field that is not written verbatim:
/// the signs TeX takes as commands are escaped.
fn escaped(field: &str, value: &str) -> String {
    if matches!(field, "url" | "doi") {
        return value.to_owned();
    }
    let mut escaped = String::with_capacity(value.len());
    let mut after_backslash = false;
    for c in value.chars() {
        if matches!(c, '&' | '%' | '#' | '$' | '_') && !after_backslash {
            escaped.push('\\');
        }
        after_backslash = c == '\\';
        escaped.push(c);
    }
    escaped
}

/// A new entry of `kind` with `fields`, as copied text says them.
fn made(kind: &str, key: String, fields: Vec<(String, String)>) -> Found {
    let fields = fields
        .into_iter()
        .filter(|(_, value)| !value.trim().is_empty())
        .map(|(name, value)| {
            let value = escaped(&name, value.trim());
            (name, value)
        })
        .collect();
    Found {
        entry: Entry {
            key,
            kind: kind.to_owned(),
            fields,
            range: 0..0,
        },
        written: None,
    }
}

/// Whether `key` can be what a source is cited by as it is.
fn usable_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | ':' | '.'))
}

/// The tag and the value of a line of RIS: `AU  - Knuth, Donald`.
fn tag(line: &str) -> Option<(&str, &str)> {
    let line = line.trim_start_matches('\u{feff}');
    let tag = line.get(..2)?;
    let rest = line.get(2..)?.trim_start_matches(' ');
    let value = rest.strip_prefix('-')?;
    tag.bytes()
        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        .then(|| (tag, value.trim()))
}

/// The BibTeX kind of a RIS type.
fn ris_kind(kind: &str) -> &'static str {
    match kind {
        "JOUR" | "JFULL" | "MGZN" | "NEWS" | "EJOUR" | "INPR" => "article",
        "BOOK" | "EBOOK" | "EDBOOK" => "book",
        "CHAP" | "ECHAP" => "incollection",
        "CONF" | "CPAPER" => "inproceedings",
        "THES" => "phdthesis",
        "RPRT" => "techreport",
        "ELEC" | "WEB" | "BLOG" => "online",
        _ => "misc",
    }
}

/// The sources of `text`, written as RIS.
fn ris(text: &str) -> Vec<Found> {
    #[derive(Default)]
    struct Record {
        kind: String,
        key: String,
        authors: Vec<String>,
        editors: Vec<String>,
        fields: Vec<(String, String)>,
        pages: (String, String),
    }
    fn done(record: Record, found: &mut Vec<Found>) {
        let kind = ris_kind(&record.kind);
        let mut fields = record.fields;
        // The title of what it is in: a journal, or a book.
        for (name, _) in &mut fields {
            if name == "in" {
                if kind == "article" {
                    "journal"
                } else {
                    "booktitle"
                }
                .clone_into(name);
            }
        }
        let pages = match record.pages {
            (first, last) if last.is_empty() => first,
            (first, last) => format!("{first}--{last}"),
        };
        fields.insert(0, ("author".to_owned(), record.authors.join(" and ")));
        fields.push(("editor".to_owned(), record.editors.join(" and ")));
        fields.push(("pages".to_owned(), pages));
        let mut seen = Vec::new();
        fields.retain(|(name, _)| {
            let first = !seen.contains(name);
            seen.push(name.clone());
            first
        });
        let key = if usable_key(&record.key) {
            record.key
        } else {
            String::new()
        };
        found.push(made(kind, key, fields));
    }
    let mut found = Vec::new();
    let mut record: Option<Record> = None;
    for line in text.lines() {
        let Some((tag, value)) = tag(line) else {
            continue;
        };
        if tag == "TY" {
            record = Some(Record {
                kind: value.to_owned(),
                ..Record::default()
            });
            continue;
        }
        let Some(open) = &mut record else {
            continue;
        };
        let mut field = |name: &str| open.fields.push((name.to_owned(), value.to_owned()));
        match tag {
            "ER" => {
                if let Some(record) = record.take() {
                    done(record, &mut found);
                }
            }
            "AU" | "A1" => open.authors.push(value.to_owned()),
            "ED" | "A2" | "A3" => open.editors.push(value.to_owned()),
            "TI" | "T1" | "CT" => field("title"),
            "T2" | "JO" | "JF" | "JA" | "BT" | "J2" => field("in"),
            "PY" | "Y1" | "DA" => {
                let year = value.split(['/', '-']).next().unwrap_or("");
                open.fields.push(("year".to_owned(), year.to_owned()));
            }
            "VL" => field("volume"),
            "IS" => field("number"),
            "SP" => value.clone_into(&mut open.pages.0),
            "EP" => value.clone_into(&mut open.pages.1),
            "DO" => field("doi"),
            "UR" => field("url"),
            "PB" => field("publisher"),
            "CY" => field("address"),
            "SN" => field("isbn"),
            "N1" => field("note"),
            "ID" => value.clone_into(&mut open.key),
            _ => {}
        }
    }
    if let Some(record) = record {
        done(record, &mut found);
    }
    found
}

/// The BibTeX kind of a CSL type.
fn csl_kind(kind: &str) -> &'static str {
    match kind {
        "article-journal" | "article-magazine" | "article-newspaper" | "article" | "review" => {
            "article"
        }
        "book" => "book",
        "chapter" | "entry-encyclopedia" | "entry-dictionary" => "incollection",
        "paper-conference" => "inproceedings",
        "thesis" => "phdthesis",
        "report" => "techreport",
        "webpage" | "post-weblog" | "post" => "online",
        _ => "misc",
    }
}

/// The sources of `text`, written as CSL-JSON: one item or a list of
/// them. `None` if it is no such thing.
fn csl_json(text: &str) -> Option<Vec<Found>> {
    use serde_json::Value;
    let value: Value = serde_json::from_str(text).ok()?;
    let items = match value {
        Value::Array(items) => items,
        item @ Value::Object(_) => vec![item],
        _ => return None,
    };
    let said = |value: &Value| match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    };
    let people = |value: Option<&Value>| {
        let names = value.and_then(Value::as_array).into_iter().flatten();
        let names: Vec<String> = names
            .filter_map(|person| {
                let part = |name: &str| person.get(name).and_then(Value::as_str);
                match (part("family"), part("given"), part("literal")) {
                    (Some(family), Some(given), _) => Some(format!("{family}, {given}")),
                    (Some(family), None, _) => Some(family.to_owned()),
                    (None, _, Some(literal)) => Some(format!("{{{literal}}}")),
                    _ => None,
                }
            })
            .collect();
        names.join(" and ")
    };
    let mut found = Vec::new();
    for item in &items {
        let kind = csl_kind(item.get("type").and_then(Value::as_str)?);
        let field = |name: &str| item.get(name).and_then(said).unwrap_or_default();
        let issued = item.get("issued");
        let year = issued
            .and_then(|date| date.pointer("/date-parts/0/0"))
            .and_then(said)
            .or_else(|| {
                let raw = issued.and_then(|date| date.get("raw").or_else(|| date.get("literal")));
                raw.and_then(Value::as_str).map(str::to_owned)
            })
            .unwrap_or_default();
        let within = if kind == "article" {
            "journal"
        } else {
            "booktitle"
        };
        let fields = vec![
            ("author".to_owned(), people(item.get("author"))),
            ("title".to_owned(), field("title")),
            (within.to_owned(), field("container-title")),
            ("editor".to_owned(), people(item.get("editor"))),
            ("year".to_owned(), year),
            ("volume".to_owned(), field("volume")),
            ("number".to_owned(), field("issue")),
            (
                "pages".to_owned(),
                field("page").replace('-', "--").replace("----", "--"),
            ),
            ("publisher".to_owned(), field("publisher")),
            ("address".to_owned(), field("publisher-place")),
            ("isbn".to_owned(), field("ISBN")),
            ("doi".to_owned(), field("DOI")),
            ("url".to_owned(), field("URL")),
            ("note".to_owned(), field("note")),
        ];
        let key = field("id");
        let key = if usable_key(&key) { key } else { String::new() };
        found.push(made(kind, key, fields));
    }
    Some(found)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use super::*;

    #[test]
    fn bibtex_is_found_and_kept_as_it_was_written() {
        let copied = "Cite this:\n@string{tcs = {Theoretical Computer Science}}\n\
            @Article{Knuth:1984,\n  author = \"Knuth, D. E.\",\n  journal = tcs,\n  title = {Literate {P}rogramming},\n  year = 1984, month = jan\n}\n\
            and more text @ the end";
        let (found, format) = read(copied);
        assert_eq!(format, Some(Format::BibTex));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].entry.key, "Knuth:1984");
        let written = found[0].written_as("knuth1984");
        assert!(written.starts_with("@Article{knuth1984,\n  author = \"Knuth, D. E.\","));
        assert!(written.ends_with("month = jan\n}"));
        assert_eq!(
            found[0].written_as("Knuth:1984"),
            found[0].written.clone().unwrap()
        );
        let abbreviations = abbreviations(copied);
        assert_eq!(abbreviations.len(), 1);
        assert_eq!(abbreviations[0].0, "tcs");
        assert!(uses(&found[0].entry, "TCS"));
        assert!(!uses(&found[0].entry, "jmlr"));
        assert_eq!(read("nothing here").1, None);
        let put = with_put(
            "@misc{a, title = {A}}\n\n\n",
            &[written, "@misc{b,}".to_owned()],
        );
        assert_eq!(super::super::parse(&put).len(), 3);
        assert!(put.starts_with("@misc{a, title = {A}}\n\n@Article{knuth1984"));
        assert_eq!(with_put("", &["@misc{b,}".to_owned()]), "@misc{b,}\n");
    }

    #[test]
    fn ris_is_made_into_bibtex() {
        let copied = "TY  - JOUR\nAU  - Shannon, Claude E.\nTI  - A Mathematical Theory of Communication\n\
            JO  - Bell System Technical Journal\nPY  - 1948/07//\nVL  - 27\nIS  - 3\nSP  - 379\nEP  - 423\n\
            DO  - 10.1002/j.1538-7305.1948.tb01338.x\nER  - \n\nTY  - BOOK\nAU  - Knuth, Donald\n\
            TI  - The Art of Computer Programming & more\nPY  - 1968\nPB  - Addison-Wesley\nER  -\n";
        let (found, format) = read(copied);
        assert_eq!(format, Some(Format::Ris));
        assert_eq!(found.len(), 2);
        let shannon = &found[0].entry;
        assert_eq!(shannon.kind, "article");
        assert_eq!(
            shannon.field("journal"),
            Some("Bell System Technical Journal")
        );
        assert_eq!(shannon.field("year"), Some("1948"));
        assert_eq!(shannon.field("pages"), Some("379--423"));
        assert_eq!(
            shannon.line(),
            "Shannon (1948) A Mathematical Theory of Communication"
        );
        let knuth = &found[1].entry;
        assert_eq!(knuth.kind, "book");
        assert_eq!(
            knuth.field("title"),
            Some("The Art of Computer Programming \\& more")
        );
        assert_eq!(knuth.field("editor"), None);
        let written = found[0].written_as("shannon1948");
        let again = super::super::parse(&written);
        assert_eq!(again[0].field("doi"), shannon.field("doi"));
    }

    #[test]
    fn csl_json_is_made_into_bibtex() {
        let copied = r#"{"type": "article-journal", "id": "https://doi.org/x",
            "author": [{"family": "Turing", "given": "A. M."}, {"literal": "The Group"}],
            "title": "On Computable Numbers, 50% of them", "container-title": "Proc. LMS",
            "issued": {"date-parts": [[1937, 1]]}, "page": "230-265", "volume": 42,
            "DOI": "10.1112/plms/s2-42.1.230"}"#;
        let (found, format) = read(copied);
        assert_eq!(format, Some(Format::CslJson));
        let turing = &found[0].entry;
        assert_eq!(turing.key, "");
        assert_eq!(
            turing.field("author"),
            Some("Turing, A. M. and {The Group}")
        );
        assert_eq!(
            turing.field("title"),
            Some("On Computable Numbers, 50\\% of them")
        );
        assert_eq!(turing.field("journal"), Some("Proc. LMS"));
        assert_eq!(turing.field("year"), Some("1937"));
        assert_eq!(turing.field("volume"), Some("42"));
        assert_eq!(turing.field("pages"), Some("230--265"));
        let listed = format!("[{copied}, {}]", copied.replace("Turing", "Church"));
        assert_eq!(read(&listed).0.len(), 2);
        // JSON that is no source is not taken for one.
        assert_eq!(read("{\"a\": 1}").1, None);
    }
}
