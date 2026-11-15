//! Hunspell dictionaries: where they are, which one a language means,
//! and reading them.
//!
//! Nothing is downloaded. A dictionary is the pair of files (`.aff`,
//! `.dic`) that the system's packages install, or that the user put
//! into a directory of their own.

use std::path::{Path, PathBuf};

use crate::words::Lexicon;

/// Where dictionaries are looked for after the user's own places.
const SYSTEM: &[&str] = &[
    "/usr/share/hunspell",
    "/usr/local/share/hunspell",
    "/usr/share/myspell",
    "/usr/share/myspell/dicts",
];

/// The directories dictionaries are looked for in, the first first:
/// `extra` (what the settings name), what `DICPATH` names (as Hunspell
/// itself reads it), the user's own (`zetwerk/dictionaries` and
/// `hunspell` under `$XDG_DATA_HOME` or `~/.local/share`), and where
/// packages install them.
#[must_use]
pub fn directories(extra: &[PathBuf]) -> Vec<PathBuf> {
    let set = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    let mut found: Vec<PathBuf> = extra.to_vec();
    if let Some(paths) = set("DICPATH") {
        found.extend(std::env::split_paths(&paths));
    }
    let data = match (set("XDG_DATA_HOME"), set("HOME")) {
        (Some(data), _) => Some(PathBuf::from(data)),
        (None, Some(home)) => Some(PathBuf::from(home).join(".local/share")),
        (None, None) => None,
    };
    if let Some(data) = data {
        found.push(data.join("zetwerk/dictionaries"));
        found.push(data.join("hunspell"));
    }
    found.extend(SYSTEM.iter().map(PathBuf::from));
    found
}

/// A dictionary that is there to be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// What it is called: `en_US`, `de_DE`.
    pub name: String,
    pub aff: PathBuf,
    pub dic: PathBuf,
}

/// A language as documents and users name it (`de`, `en-GB`, `en_us`):
/// its code in small letters and, if it names one, its region in
/// capitals.
fn parts(language: &str) -> (String, Option<String>) {
    let mut parts = language.trim().split(['-', '_']);
    let code = parts.next().unwrap_or("").to_lowercase();
    let region = parts.next().filter(|region| region.len() == 2);
    (code, region.map(str::to_uppercase))
}

/// The regions whose dictionary a language means where none is named,
/// the likeliest first: English is British, German is Germany's, Dutch
/// is that of the Netherlands.
fn regions(code: &str) -> Vec<String> {
    let known: &[&str] = match code {
        "en" => &["GB", "US", "AU", "CA"],
        "de" => &["DE", "AT", "CH"],
        "nl" => &["NL", "BE"],
        "pt" => &["PT", "BR"],
        "sv" => &["SE"],
        "da" => &["DK"],
        "cs" => &["CZ"],
        "el" => &["GR"],
        "uk" => &["UA"],
        "nb" | "nn" => &["NO"],
        _ => &[],
    };
    let mut regions: Vec<String> = known.iter().map(|region| (*region).to_owned()).collect();
    regions.push(code.to_uppercase());
    regions
}

/// The dictionary `language` means, by name, whether it is there or
/// not: `en_GB` for `en`, `de_DE` for `de`, `nl_NL` for `nl`, and for a
/// language that names its region (`en-US`), that region's.
#[must_use]
pub fn wanted(language: &str) -> String {
    let (code, region) = parts(language);
    let region = region.or_else(|| regions(&code).into_iter().next());
    format!("{code}_{}", region.unwrap_or_default())
}

/// The dictionary in one of `directories` that `language` means: the
/// one it names, or one of the language for the likeliest region that
/// is there (`en` is `en_GB`, else `en_US`; `de` is `de_DE`; `nl` is
/// `nl_NL`). Where [`wanted`] is not what is found, another region's
/// dictionary stands in for it.
#[must_use]
pub fn find(language: &str, directories: &[PathBuf]) -> Option<Found> {
    let (code, region) = parts(language);
    if code.is_empty() {
        return None;
    }
    let mut names: Vec<String> = Vec::new();
    // A name given in full (`en_US-large`) is meant as it is.
    names.push(language.trim().to_owned());
    names.extend(region.map(|region| format!("{code}_{region}")));
    names.extend(
        regions(&code)
            .iter()
            .map(|region| format!("{code}_{region}")),
    );
    names.push(code.clone());
    // Whatever else there is of the language.
    let mut others: Vec<String> = installed(directories)
        .into_iter()
        .filter(|name| parts(name).0 == code)
        .collect();
    others.sort();
    names.extend(others);
    names.iter().find_map(|name| {
        directories.iter().find_map(|directory| {
            let aff = directory.join(format!("{name}.aff"));
            let dic = directory.join(format!("{name}.dic"));
            (aff.is_file() && dic.is_file()).then(|| Found {
                name: name.clone(),
                aff,
                dic,
            })
        })
    })
}

/// The names of the dictionaries in `directories`.
#[must_use]
pub fn installed(directories: &[PathBuf]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for directory in directories {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_dictionary = path.extension().is_some_and(|ending| ending == "dic")
                && path.with_extension("aff").is_file();
            let name = path.file_stem().and_then(|name| name.to_str());
            if let (true, Some(name)) = (is_dictionary, name)
                && !names.iter().any(|known| known == name)
            {
                names.push(name.to_owned());
            }
        }
    }
    names.sort();
    names
}

/// The package that brings the dictionary `language` means, as Arch
/// Linux calls it: `hunspell-en_gb`, `hunspell-de`, `hunspell-nl`.
#[must_use]
pub fn package(language: &str) -> String {
    let (code, region) = parts(language);
    match (code.as_str(), region.as_deref()) {
        ("en", Some(region @ ("US" | "AU" | "CA"))) => {
            format!("hunspell-en_{}", region.to_lowercase())
        }
        ("en", _) => "hunspell-en_gb".to_owned(),
        ("es", region) => format!("hunspell-es_{}", region.unwrap_or("ES").to_lowercase()),
        (code, _) => format!("hunspell-{code}"),
    }
}

/// The text of a dictionary file, which says in its `.aff` how it is
/// encoded: `SET UTF-8`, or one of the Latin encodings older ones use.
fn decode(bytes: &[u8], encoding: &str) -> Result<String, String> {
    let encoding = encoding.to_uppercase().replace(['-', '_'], "");
    match encoding.as_str() {
        "UTF8" => Ok(String::from_utf8_lossy(bytes).into_owned()),
        "ISO88591" => Ok(bytes.iter().map(|byte| char::from(*byte)).collect()),
        "ISO885915" => Ok(bytes
            .iter()
            .map(|byte| match byte {
                0xA4 => '€',
                0xA6 => 'Š',
                0xA8 => 'š',
                0xB4 => 'Ž',
                0xB8 => 'ž',
                0xBC => 'Œ',
                0xBD => 'œ',
                0xBE => 'Ÿ',
                byte => char::from(*byte),
            })
            .collect()),
        other => Err(format!("it is encoded as {other}, which is not read")),
    }
}

/// The encoding an `.aff` file names; Latin-1 where it names none, as
/// Hunspell has it.
fn encoding_of(aff: &[u8]) -> String {
    let named = aff.split(|byte| *byte == b'\n').find_map(|line| {
        let line = std::str::from_utf8(line).ok()?.trim();
        // Some files begin with a byte order mark.
        let line = line.trim_start_matches('\u{feff}');
        line.strip_prefix("SET ").map(|name| name.trim().to_owned())
    });
    named.unwrap_or_else(|| "ISO8859-1".to_owned())
}

/// A dictionary, read and ready to be asked.
pub struct Speller {
    name: String,
    dictionary: spellbook::Dictionary,
}

impl std::fmt::Debug for Speller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut shown = f.debug_struct("Speller");
        shown.field("name", &self.name).finish_non_exhaustive()
    }
}

impl Speller {
    /// Reads the dictionary `found`. The error says why it could not be.
    pub fn load(found: &Found) -> Result<Self, String> {
        let read = |path: &Path| {
            std::fs::read(path).map_err(|err| format!("{} was not read: {err}", path.display()))
        };
        let (aff, dic) = (read(&found.aff)?, read(&found.dic)?);
        let encoding = encoding_of(&aff);
        let unread = |why: String| format!("the dictionary {} is not used: {why}", found.name);
        let aff = decode(&aff, &encoding).map_err(unread)?;
        let dic = decode(&dic, &encoding).map_err(unread)?;
        let dictionary =
            spellbook::Dictionary::new(&aff, &dic).map_err(|err| unread(err.to_string()))?;
        Ok(Self {
            name: found.name.clone(),
            dictionary,
        })
    }

    /// What the dictionary is called: `en_US`.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Up to `most` words that `word` may have been meant to be, the
    /// likeliest first. This takes milliseconds, tens of them for a
    /// long word: it is for when the user asks.
    #[must_use]
    pub fn suggest(&self, word: &str, most: usize) -> Vec<String> {
        let mut found = Vec::new();
        self.dictionary.suggest(word, &mut found);
        found.truncate(most);
        found
    }
}

impl Lexicon for Speller {
    fn knows(&self, word: &str) -> bool {
        self.dictionary.check(word)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
pub(crate) mod tests {
    use super::*;

    /// The directory of the small dictionaries kept for tests.
    pub(crate) fn fixtures() -> PathBuf {
        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures"))
    }

    /// A fresh empty directory.
    pub(crate) fn scratch(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "zetwerk-language-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    /// The dictionary a package installed here, if one did: what the
    /// tests of real dictionaries are run with. They say so and pass
    /// where there is none, for no test may need what is installed.
    fn system(name: &str) -> Option<Speller> {
        let directories: Vec<PathBuf> = SYSTEM.iter().map(PathBuf::from).collect();
        let found = find(name, &directories).filter(|found| found.name == name);
        let Some(found) = found else {
            eprintln!("no {name} dictionary is installed: not tested with it");
            return None;
        };
        Some(Speller::load(&found).unwrap())
    }

    /// Asserts which of `right` and `wrong` `speller` knows.
    fn knows(speller: &Speller, right: &[&str], wrong: &[&str]) {
        for word in right {
            assert!(
                speller.knows(word),
                "{word} is a word in {}",
                speller.name()
            );
        }
        for word in wrong {
            assert!(
                !speller.knows(word),
                "{word} is no word in {}",
                speller.name()
            );
        }
    }

    #[test]
    fn a_language_means_the_dictionary_that_is_there() {
        let directories = [fixtures()];
        let name = |language: &str| find(language, &directories).map(|found| found.name);
        // The three languages the user writes in.
        assert_eq!(name("en").as_deref(), Some("en_GB"));
        assert_eq!(name("de").as_deref(), Some("de_DE"));
        assert_eq!(name("nl").as_deref(), Some("nl_NL"));
        assert_eq!(name("en-GB").as_deref(), Some("en_GB"));
        assert_eq!(name("EN_gb").as_deref(), Some("en_GB"));
        assert_eq!(name("de-AT").as_deref(), Some("de_DE"));
        assert_eq!(name("nl-BE").as_deref(), Some("nl_NL"));
        assert_eq!(name("fr"), None);
        assert_eq!(name(""), None);
        assert_eq!(installed(&directories), ["de_DE", "en_GB", "nl_NL"]);
        assert_eq!(
            (
                wanted("en"),
                wanted("de"),
                wanted("nl"),
                wanted("en-US"),
                wanted("fr")
            ),
            (
                "en_GB".to_owned(),
                "de_DE".to_owned(),
                "nl_NL".to_owned(),
                "en_US".to_owned(),
                "fr_FR".to_owned()
            )
        );
    }

    #[test]
    fn the_region_named_and_the_users_own_directory_come_first() {
        let own = scratch("own");
        for name in ["en_GB", "en_US", "en_ZA"] {
            std::fs::copy(
                fixtures().join("en_GB.aff"),
                own.join(format!("{name}.aff")),
            )
            .unwrap();
            std::fs::copy(
                fixtures().join("en_GB.dic"),
                own.join(format!("{name}.dic")),
            )
            .unwrap();
        }
        let directories = [own.clone(), fixtures()];
        let found = find("en-US", &directories).unwrap();
        assert_eq!(found.name, "en_US");
        let found = find("en", &directories).unwrap();
        assert_eq!(
            (found.name.as_str(), found.aff.starts_with(&own)),
            ("en_GB", true)
        );
        // English is American where there is no British dictionary: the
        // name found is not the name wanted, which is how it is told.
        std::fs::remove_file(own.join("en_GB.dic")).unwrap();
        let found = find("en", std::slice::from_ref(&own)).unwrap();
        assert_eq!(
            (found.name.as_str(), wanted("en").as_str()),
            ("en_US", "en_GB")
        );
        // A region nobody thought of, where it is all there is.
        std::fs::remove_file(own.join("en_US.dic")).unwrap();
        assert_eq!(
            find("en", std::slice::from_ref(&own)).unwrap().name,
            "en_ZA"
        );
        let _ = std::fs::remove_dir_all(own);
    }

    #[test]
    fn packages_are_named_as_arch_names_them() {
        assert_eq!(package("en"), "hunspell-en_gb");
        assert_eq!(package("en-GB"), "hunspell-en_gb");
        assert_eq!(package("en-US"), "hunspell-en_us");
        assert_eq!(package("de"), "hunspell-de");
        assert_eq!(package("de-CH"), "hunspell-de");
        assert_eq!(package("nl"), "hunspell-nl");
        assert_eq!(package("nl-BE"), "hunspell-nl");
        assert_eq!(package("es"), "hunspell-es_es");
    }

    #[test]
    fn english_words_and_their_forms_are_known() {
        let speller = Speller::load(&find("en", &[fixtures()]).unwrap()).unwrap();
        assert_eq!(speller.name(), "en_GB");
        knows(
            &speller,
            &[
                "hello", "Hello", "HELLO", "walk", "walked", "walking", "cats", "unhappy", "colour",
            ],
            &["helo", "walkked", "catz", "Haus", "color"],
        );
        let suggested = speller.suggest("helo", 5);
        assert!(
            suggested.iter().any(|word| word == "hello"),
            "{suggested:?}"
        );
        assert!(speller.suggest("walkked", 1).len() <= 1);
    }

    #[test]
    fn german_compounds_capitals_and_sharp_s_are_known() {
        let speller = Speller::load(&find("de", &[fixtures()]).unwrap()).unwrap();
        knows(
            &speller,
            &[
                "Haus",
                "Häuser",
                "Straße",
                "Tür",
                "Haustür",
                "Straßenhaus",
                "über",
                "Über",
            ],
            // A noun is written with a capital; a part of a compound is
            // no word by itself; Swiss spelling is not Germany's.
            &["Hauss", "haus", "Türhausx", "Strasse", "tür"],
        );
        // In capitals ß is written SS (CHECKSHARPS), and only then.
        knows(&speller, &["STRASSE"], &["HAUSS"]);
        let suggested = speller.suggest("Strasse", 5);
        assert!(
            suggested.iter().any(|word| word == "Straße"),
            "{suggested:?}"
        );
    }

    #[test]
    fn dutch_words_numbers_and_capitals_are_known() {
        let speller = Speller::load(&find("nl", &[fixtures()]).unwrap()).unwrap();
        knows(
            &speller,
            // Flags of two letters, forms by suffix, the digraph IJ, a
            // word that keeps its case, numbers put together by rule.
            &[
                "huis",
                "huizen",
                "fiets",
                "fietsen",
                "IJsland",
                "ijs",
                "woordenboek",
                "eenentwintig",
                "tweeëndertig",
            ],
            &[
                "huiz",
                "Ijsland",
                "ijsland",
                "boekwoorden",
                "twintigeenen",
                "fietsenhuis",
            ],
        );
        let suggested = speller.suggest("fiest", 5);
        assert!(
            suggested.iter().any(|word| word == "fiets"),
            "{suggested:?}"
        );
    }

    #[test]
    fn the_german_dictionary_installed_here_is_read_as_german_is_written() {
        let Some(speller) = system("de_DE") else {
            return;
        };
        knows(
            &speller,
            &[
                "Haus",
                "Häuser",
                "Straße",
                "STRASSE",
                "dass",
                "Rechtschreibprüfung",
                "Donaudampfschifffahrt",
                "Bundesregierung",
                "Schreibtischlampe",
                "gehen",
                "ging",
                "Über",
                "Maßnahme",
            ],
            &[
                "Strasse",
                "daß",
                "haus",
                "Rechtschreibfeler",
                "Massnahme",
                "Haüser",
            ],
        );
        let suggested = speller.suggest("Rechtschreibfeler", 5);
        assert!(
            suggested.iter().any(|word| word == "Rechtschreibfehler"),
            "{suggested:?}"
        );
    }

    #[test]
    fn the_dutch_dictionary_installed_here_is_read_as_dutch_is_written() {
        let Some(speller) = system("nl_NL") else {
            return;
        };
        knows(
            &speller,
            &[
                "huis",
                "huizen",
                "fiets",
                "woordenboek",
                "ziekenhuis",
                "fietsenstalling",
                "IJsland",
                "IJsselmeer",
                "ijs",
                "zeeën",
                "auto's",
                "eenentwintig",
                "Nederland",
                "onmiddellijk",
            ],
            &["huiz", "Ijsland", "nederland", "onmiddelijk", "ziekehuis"],
        );
        let suggested = speller.suggest("onmiddelijk", 5);
        assert!(
            suggested.iter().any(|word| word == "onmiddellijk"),
            "{suggested:?}"
        );
    }

    #[test]
    fn an_english_dictionary_installed_here_is_read() {
        for (name, right, wrong) in [("en_GB", "colour", "color"), ("en_US", "color", "colour")] {
            let Some(speller) = system(name) else {
                continue;
            };
            knows(
                &speller,
                &[right, "hello", "Hello", "don't", "walked", "dog's", "well"],
                &[wrong, "recieve", "teh", "speling"],
            );
            assert!(
                speller
                    .suggest("recieve", 5)
                    .iter()
                    .any(|word| word == "receive")
            );
        }
    }

    #[test]
    fn a_latin_dictionary_is_read_as_what_it_says_it_is() {
        let directory = scratch("latin");
        std::fs::write(directory.join("xx.aff"), b"SET ISO8859-1\nTRY \xfc\n").unwrap();
        std::fs::write(directory.join("xx.dic"), b"2\nT\xfcr\n\xfcber\n").unwrap();
        let speller =
            Speller::load(&find("xx", std::slice::from_ref(&directory)).unwrap()).unwrap();
        assert!(speller.knows("Tür") && speller.knows("über") && !speller.knows("Tur"));
        // One that cannot be read says so, and which.
        std::fs::write(directory.join("xx.aff"), b"SET KOI8-R\n").unwrap();
        let found = find("xx", std::slice::from_ref(&directory)).unwrap();
        let why = Speller::load(&found).unwrap_err();
        assert!(why.contains("xx") && why.contains("KOI8R"), "{why}");
        let _ = std::fs::remove_dir_all(directory);
    }
}
