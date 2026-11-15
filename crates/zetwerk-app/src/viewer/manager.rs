//! The document manager: how the document as a whole looks, in one list.
//!
//! `:doc` (or `gD`) lists what there is to decide about a document — its
//! font, its paper, where the page numbers stand — each with what it is
//! now. Enter on one puts up what it can be; Enter on one of those makes
//! it so, and the page behind the list shows it at once. Where anything
//! can be wanted (a size, a title), it is typed. Escape leads back.
//!
//! Nothing of this is kept anywhere but in the document: each decision
//! is a `#set` rule before the text, as Typst wants it, written and
//! changed here so that nobody has to look at it.

use typst::World;

use super::assist::Target;
use super::{Edited, Viewer};
use crate::typst_text;

/// What may be typed for a setting where the choices are not all there
/// is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Free {
    /// Nothing: one of the choices it is.
    No,
    /// A length; a bare number is in this unit.
    Length(&'static str),
    /// A text, written as a string.
    Text,
    /// Words that are set as they are typed.
    Words,
    /// A whole number.
    Count,
    /// A language, as its code of two or three letters (`de`).
    Language,
    /// A paper Typst knows by name.
    Paper,
    /// The family of a font there is.
    Font,
}

/// Something to decide about a document.
struct Setting {
    /// What it is listed as.
    name: &'static str,
    /// The function the rule is for and the argument it sets.
    function: &'static str,
    key: &'static str,
    /// What holds where no rule says otherwise, as it is listed.
    usual: &'static str,
    /// What it can be: as listed, and as written in the rule.
    choices: &'static [(&'static str, &'static str)],
    free: Free,
}

const fn setting(
    name: &'static str,
    function: &'static str,
    key: &'static str,
    usual: &'static str,
    choices: &'static [(&'static str, &'static str)],
    free: Free,
) -> Setting {
    Setting {
        name,
        function,
        key,
        usual,
        choices,
        free,
    }
}

const ON_OFF: &[(&str, &str)] = &[("on", "true"), ("off", "false")];

/// All there is to decide, in the order it is listed in. The font's
/// choices are the fonts there are.
const SETTINGS: &[Setting] = &[
    setting("font", "text", "font", "Libertinus Serif", &[], Free::Font),
    setting(
        "font size",
        "text",
        "size",
        "11pt",
        &[
            ("9pt", "9pt"),
            ("10pt", "10pt"),
            ("10.5pt", "10.5pt"),
            ("11pt", "11pt"),
            ("12pt", "12pt"),
            ("14pt", "14pt"),
        ],
        Free::Length("pt"),
    ),
    setting(
        "language",
        "text",
        "lang",
        "en",
        &[
            ("en  English", "\"en\""),
            ("de  Deutsch", "\"de\""),
            ("fr  français", "\"fr\""),
            ("es  español", "\"es\""),
            ("it  italiano", "\"it\""),
            ("nl  Nederlands", "\"nl\""),
            ("pt  português", "\"pt\""),
            ("pl  polski", "\"pl\""),
            ("sv  svenska", "\"sv\""),
            ("da  dansk", "\"da\""),
        ],
        Free::Language,
    ),
    setting(
        "hyphenation",
        "text",
        "hyphenate",
        "as needed",
        &[
            ("as needed", "auto"),
            ("always", "true"),
            ("never", "false"),
        ],
        Free::No,
    ),
    setting(
        "paper",
        "page",
        "paper",
        "a4",
        &[
            ("a4", "\"a4\""),
            ("a5", "\"a5\""),
            ("a3", "\"a3\""),
            ("us-letter", "\"us-letter\""),
            ("us-legal", "\"us-legal\""),
            ("slides 16:9", "\"presentation-16-9\""),
            ("slides 4:3", "\"presentation-4-3\""),
        ],
        Free::Paper,
    ),
    setting(
        "orientation",
        "page",
        "flipped",
        "upright",
        &[("upright", "false"), ("lying", "true")],
        Free::No,
    ),
    setting(
        "margins",
        "page",
        "margin",
        "2.5cm (of a4)",
        &[
            ("1.5cm", "1.5cm"),
            ("2cm", "2cm"),
            ("2.5cm", "2.5cm"),
            ("3cm", "3cm"),
            ("1in", "1in"),
            ("as the paper wants", "auto"),
        ],
        Free::Length("cm"),
    ),
    setting(
        "columns",
        "page",
        "columns",
        "1",
        &[("1", "1"), ("2", "2"), ("3", "3")],
        Free::Count,
    ),
    setting(
        "page numbers",
        "page",
        "numbering",
        "none",
        &[
            ("none", "none"),
            ("1", "\"1\""),
            ("1 / 12", "\"1 / 1\""),
            ("– 1 –", "\"– 1 –\""),
            ("i", "\"i\""),
            ("I", "\"I\""),
            ("a", "\"a\""),
        ],
        Free::Text,
    ),
    setting(
        "page numbers stand",
        "page",
        "number-align",
        "bottom, middle",
        &[
            ("bottom, middle", "center + bottom"),
            ("bottom, right", "right + bottom"),
            ("bottom, left", "left + bottom"),
            ("top, middle", "center + top"),
            ("top, right", "right + top"),
            ("top, left", "left + top"),
        ],
        Free::No,
    ),
    setting(
        "running head",
        "page",
        "header",
        "none",
        &[("none", "none")],
        Free::Words,
    ),
    setting("justified lines", "par", "justify", "off", ON_OFF, Free::No),
    setting(
        "between lines",
        "par",
        "leading",
        "0.65em",
        &[
            ("0.5em  tight", "0.5em"),
            ("0.65em  usual", "0.65em"),
            ("0.8em", "0.8em"),
            ("1em  wide", "1em"),
            ("1.3em  double", "1.3em"),
        ],
        Free::Length("em"),
    ),
    setting(
        "between paragraphs",
        "par",
        "spacing",
        "1.2em",
        &[
            ("0.65em  as between lines", "0.65em"),
            ("1em", "1em"),
            ("1.2em  usual", "1.2em"),
            ("1.6em", "1.6em"),
        ],
        Free::Length("em"),
    ),
    setting(
        "paragraphs begin indented",
        "par",
        "first-line-indent",
        "0pt",
        &[
            ("not at all", "0pt"),
            ("1em", "1em"),
            ("1.5em", "1.5em"),
            ("2em", "2em"),
        ],
        Free::Length("em"),
    ),
    setting(
        "headings numbered",
        "heading",
        "numbering",
        "none",
        &[
            ("none", "none"),
            ("1.1", "\"1.1\""),
            ("1.1.", "\"1.1.\""),
            ("1.a", "\"1.a\""),
            ("I.1", "\"I.1\""),
            ("A.1", "\"A.1\""),
        ],
        Free::Text,
    ),
    setting(
        "equations numbered",
        "math.equation",
        "numbering",
        "none",
        &[
            ("none", "none"),
            ("(1)", "\"(1)\""),
            ("[1]", "\"[1]\""),
            ("(1.1)", "\"(1.1)\""),
        ],
        Free::Text,
    ),
    setting(
        "captions stand",
        "figure.caption",
        "position",
        "below",
        &[("below", "bottom"), ("above", "top")],
        Free::No,
    ),
    setting(
        "figures are called",
        "figure",
        "supplement",
        "Figure",
        &[
            ("Figure", "[Figure]"),
            ("Fig.", "[Fig.]"),
            ("Abbildung", "[Abbildung]"),
            ("Abb.", "[Abb.]"),
        ],
        Free::Words,
    ),
    setting(
        "lists are marked",
        "list",
        "marker",
        "•",
        &[("•", "[•]"), ("–", "[–]"), ("‣", "[‣]"), ("∗", "[∗]")],
        Free::Words,
    ),
    setting(
        "numbered lists count",
        "enum",
        "numbering",
        "1.",
        &[
            ("1.", "\"1.\""),
            ("(1)", "\"(1)\""),
            ("a)", "\"a)\""),
            ("i.", "\"i.\""),
            ("A.", "\"A.\""),
        ],
        Free::Text,
    ),
    setting(
        "sources are cited as",
        "bibliography",
        "style",
        "ieee",
        &[
            ("ieee  [1]", "\"ieee\""),
            ("apa  (Author, 2024)", "\"apa\""),
            ("chicago  (Author 2024)", "\"chicago-author-date\""),
            ("chicago, notes", "\"chicago-notes\""),
            ("mla", "\"mla\""),
            ("harvard", "\"harvard-cite-them-right\""),
            ("nature", "\"nature\""),
            ("din 1505-2", "\"din-1505-2-numeric\""),
        ],
        Free::Text,
    ),
    setting(
        "title (of the file)",
        "document",
        "title",
        "none",
        &[],
        Free::Words,
    ),
    setting(
        "author (of the file)",
        "document",
        "author",
        "none",
        &[],
        Free::Text,
    ),
];

/// The row of the document manager that leads to the styles: the one
/// behind all settings.
pub(super) const STYLES_ROW: usize = SETTINGS.len();

/// The names of the papers Typst knows, as its own list has them.
fn paper_names() -> Vec<String> {
    use typst::foundations::{CastInfo, Reflect, Value};
    let mut names = Vec::new();
    <typst::layout::Paper as Reflect>::input().walk(|info| {
        if let CastInfo::Value(Value::Str(name), _) = info {
            names.push(name.as_str().to_owned());
        }
    });
    names
}

/// Of `names`, those nearest to `typed`, the nearest first: those it is
/// part of, then those a letter or a few away; none that is far off.
pub(super) fn nearest(typed: &str, names: impl IntoIterator<Item = String>) -> Vec<String> {
    /// How many are offered.
    const MOST: usize = 8;
    let typed = typed.trim().to_lowercase();
    let far = (typed.chars().count() / 3).max(2);
    let mut ranked: Vec<(usize, usize, String)> = names
        .into_iter()
        .filter_map(|name| {
            let known = name.to_lowercase();
            let away = if known.contains(&typed) {
                0
            } else {
                super::refused::distance(&typed, &known) + 1
            };
            (away <= far + 1).then(|| (away, name.chars().count(), name))
        })
        .collect();
    ranked.sort();
    ranked.dedup_by(|a, b| a.2 == b.2);
    ranked
        .into_iter()
        .take(MOST)
        .map(|(.., name)| name)
        .collect()
}

/// A text as Typst writes a string.
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// What means something in running text: what is typed for a setting
/// of words is written with a backslash before each of these.
const MARKS: &[char] = &[
    '\\', '[', ']', '#', '$', '*', '_', '`', '<', '>', '@', '~', '/', '=', '-', '+',
];

/// What was typed for a setting, as it is written in the rule; `None`
/// if it is nothing the setting can be.
fn typed_value(free: Free, typed: &str) -> Option<String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return None;
    }
    match free {
        Free::No => None,
        Free::Length(unit) => {
            let number = typed.trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == '%');
            let own = &typed[number.len()..];
            let fine = number.parse::<f64>().is_ok_and(|number| number >= 0.0);
            let known = matches!(own, "" | "pt" | "mm" | "cm" | "in" | "em" | "%");
            (fine && known).then(|| format!("{number}{}", if own.is_empty() { unit } else { own }))
        }
        Free::Text | Free::Font => Some(quoted(typed)),
        // Any other is refused by Typst, and the document with it.
        Free::Paper => paper_names()
            .into_iter()
            .find(|name| name.eq_ignore_ascii_case(typed))
            .map(|name| quoted(&name)),
        // Between brackets, with every mark that means something in
        // running text made to mean nothing: a dollar sign there began
        // an equation that never ended, and the document was no more.
        Free::Words => {
            let mut written = String::with_capacity(typed.len() + 2);
            written.push('[');
            for c in typed.chars() {
                if MARKS.contains(&c) {
                    written.push('\\');
                }
                written.push(c);
            }
            written.push(']');
            Some(written)
        }
        // Any other is refused by Typst, and the document with it.
        Free::Language => {
            typst_text::is_language(typed).then(|| quoted(&typed.to_ascii_lowercase()))
        }
        Free::Count => typed
            .parse::<usize>()
            .ok()
            .filter(|count| (1..=8).contains(count))
            .map(|count| count.to_string()),
    }
}

/// What a value written in a rule is listed as: the choice it is, or
/// else itself without its quotes or brackets.
fn listed(setting: &Setting, written: &str) -> String {
    let choice = setting.choices.iter().find(|(_, value)| *value == written);
    match choice {
        Some((label, _)) => (*label).to_owned(),
        None => match written.strip_prefix('[').and_then(|w| w.strip_suffix(']')) {
            // Words, as they were typed: without the backslashes that
            // keep their marks from meaning anything.
            Some(words) => {
                let mut shown = String::with_capacity(words.len());
                let mut chars = words.chars().peekable();
                while let Some(c) = chars.next() {
                    let escaped =
                        c == '\\' && chars.peek().is_some_and(|next| MARKS.contains(next));
                    shown.extend(if escaped { chars.next() } else { Some(c) });
                }
                shown
            }
            None => written.trim_matches('"').to_owned(),
        },
    }
}

impl Viewer {
    /// The commands of the document manager, without their `doc`: none
    /// puts it up; a number, with that row chosen; `set`, a row and what
    /// was typed for it make the row that.
    pub(super) fn doc_command(&mut self, command: &str) {
        match command.strip_prefix("set ").map(|set| set.split_once(' ')) {
            Some(Some((row, typed))) => {
                if let Ok(row) = row.parse::<usize>() {
                    self.type_setting(row, typed);
                }
            }
            Some(None) => {}
            None => self.open_manager(command.parse().unwrap_or(0)),
        }
    }

    /// `:doc`: the document manager, with row `chosen` chosen.
    pub(super) fn open_manager(&mut self, chosen: usize) {
        let Some(editor) = &self.editor else {
            return;
        };
        let source = editor.source();
        let mut items: Vec<(String, Target)> = SETTINGS
            .iter()
            .enumerate()
            .map(|(row, setting)| {
                let written = typst_text::rule_value(source, setting.function, setting.key);
                let now = written.map_or_else(
                    || setting.usual.to_owned(),
                    |written| listed(setting, &written),
                );
                (format!("{}\t{now}", setting.name), Target::Choose(row))
            })
            .collect();
        // Last, the styles: how headings, links and the rest look.
        items.push(super::styles::manager_row(source));
        let chosen = chosen.min(items.len() - 1);
        self.open_picker("document", items, chosen);
    }

    /// What setting `row` can be, to choose from or to type.
    pub(super) fn open_setting(&mut self, row: usize) {
        let (Some(setting), Some(editor)) = (SETTINGS.get(row), &self.editor) else {
            return;
        };
        let written = typst_text::rule_value(editor.source(), setting.function, setting.key);
        // One of two: the other, at once.
        if let ([one, other], Free::No) = (setting.choices, setting.free) {
            // Where no rule says, it is what it usually is.
            let usual = if one.0 == setting.usual {
                one.1
            } else {
                other.1
            };
            let now = written.as_deref().unwrap_or(usual);
            let (label, value) = if now == one.1 { other } else { one };
            return self.apply_setting(row, value, label);
        }
        let fonts: Vec<(String, String)> = if setting.key == "font" {
            let world = self.project.snapshot();
            world
                .book()
                .families()
                .map(|(family, _)| (family.to_owned(), quoted(family)))
                .collect()
        } else {
            Vec::new()
        };
        let choices = setting
            .choices
            .iter()
            .map(|(label, value)| ((*label).to_owned(), (*value).to_owned()))
            .chain(fonts);
        let mut chosen = 0;
        let mut items: Vec<(String, Target)> = Vec::new();
        for (label, value) in choices {
            if written.as_deref() == Some(value.as_str()) {
                chosen = items.len();
            }
            let set = Target::Set {
                row,
                value,
                told: label.clone(),
            };
            items.push((label, set));
        }
        if items.is_empty() {
            // Nothing to choose from: it is typed.
            items.push(("type it, then Enter".to_owned(), Target::Nothing));
        }
        let free = (setting.free != Free::No).then(|| format!("doc set {row} "));
        self.open_picker_for(setting.name, items, chosen, format!("doc {row}"), free);
        self.setting_accepts(setting.free);
    }

    /// Makes the list that is up offer as typed only what a setting of
    /// kind `kind` can be: what it cannot be is not offered, and what is
    /// like it comes first.
    fn setting_accepts(&mut self, kind: Free) {
        let fonts = if kind == Free::Font {
            self.font_families()
        } else {
            Vec::new()
        };
        self.picker_accepts(move |typed| {
            typed_value(kind, typed).is_some()
                && (kind != Free::Font || fonts.contains(&typed.trim().to_lowercase()))
        });
    }

    /// The families of the fonts there are, in small letters: as Typst
    /// finds them by name.
    fn font_families(&self) -> Vec<String> {
        let world = self.project.snapshot();
        let families = world
            .book()
            .families()
            .map(|(family, _)| family.to_lowercase());
        families.collect()
    }

    /// Where what was typed for setting `row` is nothing it can be: says
    /// why, and puts up what is nearest to it, to choose from or to type
    /// again; or the list of settings where nothing is near.
    fn refuse_setting(&mut self, row: usize, typed: &str) {
        let Some(setting) = SETTINGS.get(row) else {
            return;
        };
        let typed = typed.trim();
        let listed = || setting.choices.iter().map(|(label, _)| (*label).to_owned());
        let (why, names): (String, Vec<String>) = match setting.free {
            Free::Paper => (
                format!("Typst knows no paper \u{201c}{typed}\u{201d}"),
                nearest(typed, paper_names()),
            ),
            Free::Font => {
                let world = self.project.snapshot();
                let families = world.book().families().map(|(family, _)| family.to_owned());
                let names = nearest(typed, families.collect::<Vec<_>>());
                (
                    format!("no font \u{201c}{typed}\u{201d} is installed"),
                    names,
                )
            }
            Free::Length(unit) => (
                format!(
                    "{} cannot be {typed}: a number and a unit, as 2{unit}",
                    setting.name
                ),
                Vec::new(),
            ),
            Free::Count => (
                format!(
                    "{} cannot be {typed}: a whole number from 1 to 8",
                    setting.name
                ),
                Vec::new(),
            ),
            Free::Language => (
                format!(
                    "{} cannot be {typed}: a code of two or three letters, as de",
                    setting.name
                ),
                nearest(typed, listed()),
            ),
            Free::No | Free::Text | Free::Words => {
                (format!("{} cannot be {typed}", setting.name), Vec::new())
            }
        };
        if names.is_empty() {
            self.open_manager(row);
            return self.alert(why);
        }
        let items: Vec<(String, Target)> = names
            .into_iter()
            .map(|name| {
                let choice = setting.choices.iter().find(|(label, _)| *label == name);
                let value = choice.map_or_else(|| quoted(&name), |(_, value)| (*value).to_owned());
                let set = Target::Set {
                    row,
                    value,
                    told: name.clone(),
                };
                (name, set)
            })
            .collect();
        let free = (setting.free != Free::No).then(|| format!("doc set {row} "));
        self.open_picker_for(setting.name, items, 0, format!("doc {row}"), free);
        self.setting_accepts(setting.free);
        self.alert(format!("{why}: the nearest are listed"));
    }

    /// Makes setting `row` what was typed for it, if it can be that.
    pub(super) fn type_setting(&mut self, row: usize, typed: &str) {
        let Some(setting) = SETTINGS.get(row) else {
            return;
        };
        let installed = || {
            let fonts = self.font_families();
            fonts.contains(&typed.trim().to_lowercase())
        };
        match typed_value(setting.free, typed) {
            Some(value) if setting.free != Free::Font || installed() => {
                self.apply_setting(row, &value, typed.trim());
            }
            _ => self.refuse_setting(row, typed),
        }
    }

    /// Makes setting `row` be `value` (as written in the rule; `told` is
    /// what it is called), and puts the list up again: there may be more
    /// to decide.
    pub(super) fn apply_setting(&mut self, row: usize, value: &str, told: &str) {
        let (Some(setting), Some(editor)) = (SETTINGS.get(row), &mut self.editor) else {
            return;
        };
        let (range, with) =
            typst_text::set_rule(editor.source(), setting.function, setting.key, value);
        match editor.replace(&[(range, with.as_str())]) {
            Ok(()) => {
                self.text_changed(Edited::Command);
                self.notice(format!("{}: {told}  (u takes it back)", setting.name));
            }
            Err(why) => self.alert(format!("not changed: {why}")),
        }
        self.open_manager(row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_typed_for_a_setting_is_written_as_typst_wants_it() {
        assert_eq!(
            typed_value(Free::Length("pt"), "12").as_deref(),
            Some("12pt")
        );
        assert_eq!(
            typed_value(Free::Length("cm"), "1in").as_deref(),
            Some("1in")
        );
        assert_eq!(typed_value(Free::Length("cm"), "wide"), None);
        assert_eq!(
            typed_value(Free::Text, "Noto Serif").as_deref(),
            Some("\"Noto Serif\"")
        );
        assert_eq!(
            typed_value(Free::Words, "Draft [1]").as_deref(),
            Some("[Draft \\[1\\]]")
        );
        assert_eq!(typed_value(Free::Count, "2").as_deref(), Some("2"));
        assert_eq!(typed_value(Free::Count, "many"), None);
        assert_eq!(typed_value(Free::No, "x"), None);
        assert_eq!(typed_value(Free::Language, "DE").as_deref(), Some("\"de\""));
        assert_eq!(typed_value(Free::Language, "German"), None);
        assert_eq!(typed_value(Free::Language, "d e"), None);
    }

    #[test]
    fn what_is_typed_for_a_setting_keeps_the_document_whole() {
        use std::sync::Arc;
        use zetwerk_typst::{Environment, FontSet, Project};
        let env = Environment::new(FontSet::Embedded);
        for typed in [
            "Price $5",
            "a@b.c",
            "*Draft*",
            "- 1. = x // y",
            "`code",
            "<x>",
            "\\",
        ] {
            let value = typed_value(Free::Words, typed).unwrap_or_default();
            let text = format!("#set page(header: {value})\nBody.\n");
            let project = Project::in_memory(
                Arc::clone(&env),
                std::path::PathBuf::from("/"),
                "main.typ",
                text.clone(),
            )
            .unwrap_or_else(|| panic!("a project"));
            let compiled = zetwerk_typst::compile(&project.snapshot());
            assert!(
                compiled.errors.is_empty(),
                "{text:?}: {:?}",
                compiled.errors
            );
        }
    }

    #[test]
    fn words_are_listed_as_they_were_typed() {
        let head = SETTINGS
            .iter()
            .find(|setting| setting.name == "running head")
            .unwrap_or_else(|| panic!("a running head"));
        for typed in ["Draft [1]", "Price $5 \\ more", "a@b.c - 2"] {
            let value = typed_value(Free::Words, typed).unwrap_or_default();
            assert_eq!(listed(head, &value), typed, "{value}");
        }
    }

    #[test]
    fn every_setting_can_be_something() {
        for setting in SETTINGS {
            assert!(
                !setting.choices.is_empty() || setting.free != Free::No,
                "{} can be nothing",
                setting.name
            );
        }
    }
}
