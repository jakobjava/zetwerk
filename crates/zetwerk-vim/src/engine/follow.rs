//! What can follow a key that waits for another.
//!
//! After `g`, `z`, `]`, `"`, an operator and the like the engine waits
//! for one more key. What that key can be, and what it then does, is
//! said here, beside the handlers of those keys, so that a host can show
//! it to whoever does not know the keys by heart. Each table is gone
//! through key by key against the engine itself by the tests below: a
//! key the engine handles that has no entry, or an entry for a key it
//! does not handle, makes them fail.

use super::{Awaiting, Engine, Format, Operator};

/// Where an entry of a table holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum When {
    /// Always: a motion, which an operator can take too.
    Always,
    /// Where no operator waits for its motion: in normal mode the key
    /// begins one, in a selection it applies it.
    NoOperator,
    /// Only where nothing else is under way and nothing is selected.
    Idle,
    /// Where nothing else is under way, whether something is selected or
    /// not.
    Unpending,
    /// Only behind an operator.
    Operator,
}

/// The keys after `g`: each key (or keys, apart by blanks), what it
/// does, and where.
const G: &[(&str, &str, When)] = &[
    ("g", "to the first line", When::Always),
    ("e E", "back to the end of a word, of a WORD", When::Always),
    ("_", "to the last character of the line", When::Always),
    ("j k", "down, up a row of what is shown", When::Always),
    ("b", "bold", When::NoOperator),
    ("m", "emphasis (italic)", When::NoOperator),
    ("t", "raw (typewriter)", When::NoOperator),
    ("c", "a comment", When::NoOperator),
    ("l", "items of a list", When::NoOperator),
    ("+ -", "heading deeper, shallower", When::NoOperator),
    (
        "u U ~",
        "lower case, upper case, the other case",
        When::NoOperator,
    ),
    ("J", "join lines without a blank", When::NoOperator),
    (
        "; ,",
        "to the place of an older, a newer change",
        When::NoOperator,
    ),
    ("&", "the last :s again, on all lines", When::Idle),
    ("i", "type where typing was left", When::Idle),
    ("v", "the last selection again", When::Idle),
    ("o", "go to a heading", When::Idle),
    ("O", "the headings, to rearrange", When::Idle),
    ("d", "to where it is defined", When::Idle),
    ("f", "open the file named here", When::Idle),
    ("a r", "accept, reject the proposed change", When::Idle),
    ("h", "the history of the text", When::Idle),
    ("D", "how the document looks", When::Idle),
    ("B", "the bibliography", When::Idle),
    ("F", "figures and tables", When::Idle),
    ("P", "give the talk from this slide", When::Idle),
    ("s", "the page, the source, or both", When::Idle),
    ("?", "all there is to do", When::Idle),
    ("w", "jump to a label in view", When::Unpending),
    ("x", "show the equation as it is written", When::Idle),
    (
        "n q",
        "a note, a question for agents, on this",
        When::Unpending,
    ),
    (
        "n N",
        "the next match of the search, the one before",
        When::Operator,
    ),
];

/// Keys after `g` that the table lists but that other parts of the
/// editor may handle (or not yet): `gw`, the labels to jump by, and `gx`,
/// the equation as it is written.
#[cfg(test)]
const ELSEWHERE: &[char] = &['w', 'x'];

/// The keys after `z`.
const Z: &[(&str, &str)] = &[
    ("=", "spelling: what may be meant"),
    ("g", "spelling: this is a word"),
    ("w", "spelling: this is no word"),
    ("z .", "this line to the middle"),
    ("t Enter", "this line to the top"),
    ("b -", "this line to the bottom"),
    ("p", "the proposed change: as it would be, as it is"),
];

/// The keys after `Z`.
const SHIFT_Z: &[(&str, &str)] = &[("Z", "save all and leave"), ("Q", "leave without saving")];

/// The keys after `]`, and after `[`.
const FORWARD: &[(&str, &str)] = &[
    ("]", "to the next page"),
    ("d", "to the next problem"),
    ("s", "to the next thing about spelling"),
    ("p", "to the next proposed change"),
    ("n", "to the next note"),
    ("i", "to the next thing that shows as nothing"),
];
const BACKWARD: &[(&str, &str)] = &[
    ("[", "to the page before"),
    ("d", "to the problem before"),
    ("s", "to the thing about spelling before"),
    ("p", "to the proposed change before"),
    ("n", "to the note before"),
    ("i", "to the thing before that shows as nothing"),
];

/// What can follow an operator, besides itself doubled.
const MOTIONS: &[(&str, &str)] = &[
    ("w b e", "words on, back, to the end of one"),
    ("W B E", "the same, by WORDS"),
    ("h l", "a character back, on"),
    ("j k", "lines down, up"),
    (
        "0 ^ $",
        "to the start, the first character, the end of the line",
    ),
    ("{ }", "paragraphs back, on"),
    ("gg G", "to the first, the last line"),
    ("f t", "to a character, up to it (F T: back)"),
    ("i a", "in, around something: a word, brackets, …"),
    ("/ ?", "to what is searched for"),
    ("` '", "to a mark, to its line"),
    ("%", "to the bracket that matches"),
];

/// The text objects of Vim, after `i` or `a`.
const OBJECTS: &[(&str, &str)] = &[
    ("w W", "a word, a WORD"),
    ("p", "a paragraph"),
    ("( ) b", "round brackets"),
    ("[ ]", "square brackets"),
    ("{ } B", "curly brackets"),
    ("< >", "angle brackets"),
    ("\" ' `", "quotes"),
];

/// The keys after `"`.
const REGISTERS: &[(&str, &str)] = &[
    ("a–z", "a register of its own"),
    ("A–Z", "added to that register"),
    ("\"", "the usual one"),
    ("0", "what was last copied"),
    ("1–9", "lines deleted, the last first"),
    ("-", "what was last deleted in a line"),
    ("_", "nowhere: nothing is kept"),
    ("+", "the system clipboard, where the host has one"),
    (":", "the last command line"),
];

/// The keys after `m`, after `` ` `` or `'`, after `q` and after `@`.
const MARK: &[(&str, &str)] = &[
    ("a–z", "remember this place by that name"),
    ("` '", "as the place to jump back to"),
];
const GOTO: &[(&str, &str)] = &[
    ("a–z", "to the place of that name"),
    ("` '", "back to where the cursor jumped from"),
    ("< >", "to the start, the end of the last selection"),
    ("^", "to where typing was left"),
];
const RECORD: &[(&str, &str)] = &[
    ("a–z 0–9", "record keys into that register; q ends"),
    ("A–Z", "record more keys into it"),
    (":", "the command lines entered, to run one again"),
    ("/ ?", "the searches made, to make one again"),
];
const PLAY: &[(&str, &str)] = &[
    ("a–z 0–9", "play the keys of that register"),
    ("@", "play the last played again"),
    (":", "the last command line again"),
];

/// What the engine waits for, and what can come next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Following {
    /// The keys it waits behind, as they are written: `g`, `d`, `di`.
    pub typed: String,
    /// What can come next: a key (or keys, apart by blanks), and what it
    /// does.
    pub next: Vec<(String, &'static str)>,
    /// Whether the text objects of the host's language may come too, as
    /// after `di`.
    pub objects: bool,
}

/// How an operator is written, and the key that completes it doubled
/// (`dd`, `guu`, `gcc`) to work on lines.
fn operator_keys(operator: Operator) -> (&'static str, &'static str) {
    match operator {
        Operator::Delete => ("d", "d"),
        Operator::Change => ("c", "c"),
        Operator::Yank => ("y", "y"),
        Operator::Indent => (">", ">"),
        Operator::Dedent => ("<", "<"),
        Operator::ToggleCase => ("g~", "~"),
        Operator::Lower => ("gu", "u"),
        Operator::Upper => ("gU", "U"),
        Operator::Format(Format::Strong) => ("gb", "gb"),
        Operator::Format(Format::Emph) => ("gm", "gm"),
        Operator::Format(Format::Raw) => ("gt", "gt"),
        Operator::Format(Format::Comment) => ("gc", "c"),
        Operator::Format(Format::ListItem) => ("gl", "gl"),
        Operator::Format(Format::Deeper) => ("g+", "g+"),
        Operator::Format(Format::Shallower) => ("g-", "g-"),
    }
}

/// `table` as entries of [`Following::next`].
fn listed(table: &[(&str, &'static str)]) -> Vec<(String, &'static str)> {
    table
        .iter()
        .map(|(keys, does)| ((*keys).to_owned(), *does))
        .collect()
}

impl Engine {
    /// What can follow the keys typed so far, where they wait for another
    /// key: after `g`, `z`, `]`, an operator, `"` and the like. `None`
    /// where no key waits, or only a count.
    #[must_use]
    pub fn following(&self) -> Option<Following> {
        let operator = self.pending.operator.map(operator_keys);
        let before = operator.map_or("", |(written, _)| written);
        let (typed, next): (String, Vec<(String, &'static str)>) = match self.pending.awaiting {
            Awaiting::Nothing => {
                let (written, doubled) = operator?;
                let mut next = vec![(doubled.to_owned(), "lines: this one, or as many as counted")];
                next.extend(listed(MOTIONS));
                (written.to_owned(), next)
            }
            Awaiting::G => {
                // As the engine sees it once the `g` is taken.
                let pending = &self.pending;
                let unpending = pending.count.is_none()
                    && pending.operator_count.is_none()
                    && pending.register.is_none()
                    && operator.is_none();
                let holds = |when: When| match when {
                    When::Always => true,
                    When::NoOperator => operator.is_none(),
                    When::Idle => unpending && !self.selecting(),
                    When::Unpending => unpending,
                    When::Operator => operator.is_some(),
                };
                let next = G
                    .iter()
                    .filter(|(_, _, when)| holds(*when))
                    .map(|(keys, does, _)| ((*keys).to_owned(), *does))
                    .collect();
                (format!("{before}g"), next)
            }
            Awaiting::View => ("z".to_owned(), listed(Z)),
            Awaiting::Z => ("Z".to_owned(), listed(SHIFT_Z)),
            Awaiting::Bracket { forward: true } => ("]".to_owned(), listed(FORWARD)),
            Awaiting::Bracket { forward: false } => ("[".to_owned(), listed(BACKWARD)),
            Awaiting::Object { around } => {
                let typed = format!("{before}{}", if around { 'a' } else { 'i' });
                return Some(Following {
                    typed,
                    next: listed(OBJECTS),
                    objects: true,
                });
            }
            Awaiting::Register => ("\"".to_owned(), listed(REGISTERS)),
            Awaiting::Mark => ("m".to_owned(), listed(MARK)),
            Awaiting::Goto { linewise } => {
                let typed = if linewise { "'" } else { "`" };
                (typed.to_owned(), listed(GOTO))
            }
            Awaiting::Record => ("q".to_owned(), listed(RECORD)),
            Awaiting::Play => ("@".to_owned(), listed(PLAY)),
            Awaiting::Find { forward, till } => {
                let key = match (forward, till) {
                    (true, false) => 'f',
                    (true, true) => 't',
                    (false, false) => 'F',
                    (false, true) => 'T',
                };
                let does = match (forward, till) {
                    (true, false) => "to the next of it in the line",
                    (true, true) => "up to the next of it in the line",
                    (false, false) => "back to the one before in the line",
                    (false, true) => "back up to the one before in the line",
                };
                let next = vec![("any character".to_owned(), does)];
                (format!("{before}{key}"), next)
            }
            Awaiting::Replace => {
                let does = "put in place of what the cursor is on (Enter: part the line)";
                ("r".to_owned(), vec![("any character".to_owned(), does)])
            }
        };
        Some(Following {
            typed,
            next,
            objects: false,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use std::collections::BTreeSet;
    use std::ops::Range;

    use super::*;
    use crate::engine::{Edit, Effect, Host, Mode};
    use crate::keys::{Key, KeyPress, parse};

    /// A text and nothing more: the engine's own ways for everything.
    struct Plain {
        text: String,
    }

    impl Host for Plain {
        fn text(&self) -> &str {
            &self.text
        }

        fn edit(&mut self, edits: &[Edit], _join: bool) {
            for edit in edits {
                self.text.replace_range(edit.range.clone(), &edit.text);
            }
        }

        fn undo(&mut self) -> Option<Range<usize>> {
            None
        }

        fn redo(&mut self) -> Option<Range<usize>> {
            None
        }

        /// Every format can be set: the formats are keys too.
        fn format(&self, _format: Format, range: Range<usize>) -> Option<Vec<Edit>> {
            Some(vec![Edit {
                range: range.start..range.start,
                text: "*".to_owned(),
            }])
        }
    }

    const TEXT: &str = "one two three\nfour five six\nseven eight nine\n";

    /// An engine on [`TEXT`], its cursor in the middle line, a search and
    /// a selection made before and left, and `keys` typed.
    fn engine(keys: &str) -> (Engine, Plain) {
        let mut host = Plain {
            text: TEXT.to_owned(),
        };
        let mut engine = Engine::new();
        // Changes made and taken back in each line, and the change list
        // gone back to the middle one: `g;` and `g,` both lead somewhere.
        let changes = "kxujxujxug;g;";
        for press in parse(&format!("/e<CR>gg0jwvl<Esc>{changes}{keys}")).unwrap() {
            engine.key(press, &mut host);
        }
        (engine, host)
    }

    /// The keys of `table` that are characters, one by one.
    fn keys_of<'a>(table: impl Iterator<Item = &'a str>) -> BTreeSet<char> {
        table
            .flat_map(str::split_whitespace)
            .filter_map(|key| {
                let mut chars = key.chars();
                let first = chars.next()?;
                chars.next().is_none().then_some(first)
            })
            .collect()
    }

    /// Every key a character can be typed with.
    fn every_key() -> impl Iterator<Item = char> {
        (b'!'..=b'~').map(char::from)
    }

    /// The keys that the engine does something with after `before`:
    /// those that do not fail.
    fn handled_after(before: &str) -> BTreeSet<char> {
        every_key()
            .filter(|c| {
                let (mut engine, mut host) = engine(before);
                engine.key(KeyPress::plain(Key::Char(*c)), &mut host);
                !engine.failed
            })
            .collect()
    }

    /// The keys that make the engine ask its host for something after
    /// `before`.
    fn asked_after(before: &str) -> BTreeSet<char> {
        every_key()
            .filter(|c| {
                let (mut engine, mut host) = engine(before);
                let effects: Vec<Effect> = engine.key(KeyPress::plain(Key::Char(*c)), &mut host);
                !effects.is_empty()
            })
            .collect()
    }

    /// The keys listed as following, after `before`.
    fn listed_after(before: &str) -> BTreeSet<char> {
        let (engine, _) = engine(before);
        let following = engine
            .following()
            .unwrap_or_else(|| panic!("something follows {before}"));
        keys_of(following.next.iter().map(|(keys, _)| keys.as_str()))
    }

    #[test]
    fn every_key_after_g_is_listed_and_every_listed_one_works() {
        // In normal mode, behind an operator, in a selection. The keys
        // that may be handled elsewhere aside.
        let ours = |keys: BTreeSet<char>| {
            let mut keys = keys;
            keys.retain(|key| !ELSEWHERE.contains(key));
            keys
        };
        for before in ["g", "dg", "vg"] {
            assert_eq!(
                ours(listed_after(before)),
                ours(handled_after(before)),
                "the keys after {before}"
            );
        }
        // Those listed where they would be handled: `gw` in a selection
        // too, `gx` not, and neither behind an operator.
        assert!(listed_after("vg").contains(&'w') && !listed_after("vg").contains(&'x'));
        assert!(listed_after("g").contains(&'x'));
        assert!(!listed_after("dg").contains(&'w'));
    }

    #[test]
    fn every_key_after_z_and_brackets_is_listed() {
        for before in ["z", "Z", "]", "["] {
            assert_eq!(
                listed_after(before),
                asked_after(before),
                "the keys after {before}"
            );
        }
        // And Enter, which is no character.
        let (mut engine, mut host) = engine("z");
        let effects = engine.key(KeyPress::plain(Key::Enter), &mut host);
        assert_ne!(effects.len(), 0);
        assert!(Z.iter().any(|(keys, _)| keys.contains("Enter")));
    }

    #[test]
    fn what_follows_is_said_where_a_key_waits_and_only_there() {
        let typed = |keys: &str| engine(keys).0.following().map(|following| following.typed);
        assert_eq!(typed(""), None);
        assert_eq!(typed("3"), None);
        assert_eq!(typed("g").as_deref(), Some("g"));
        assert_eq!(typed("d").as_deref(), Some("d"));
        assert_eq!(typed("2gu").as_deref(), Some("gu"));
        assert_eq!(typed("di").as_deref(), Some("di"));
        assert_eq!(typed("\"").as_deref(), Some("\""));
        assert_eq!(typed("\"ay"), Some("y".to_owned()));
        assert_eq!(typed("dt").as_deref(), Some("dt"));
        // Behind an operator, its doubled key first; behind `i`, the
        // host's objects too.
        let (lower, _) = engine("gu");
        let following = lower.following().unwrap();
        assert_eq!(following.next[0].0, "u");
        assert!(!following.objects);
        assert!(engine("ci").0.following().unwrap().objects);
        // In a selection, `g` offers no command of its own.
        let (visual, _) = engine("vg");
        assert_eq!(visual.mode(), Mode::Visual);
        let keys = keys_of(
            visual
                .following()
                .unwrap()
                .next
                .iter()
                .map(|(k, _)| k.as_str()),
        );
        assert!(keys.contains(&'b') && !keys.contains(&'?'));
        // The key that follows takes it away.
        let (mut after, mut host) = engine("g");
        after.key(KeyPress::plain(Key::Char('g')), &mut host);
        assert_eq!(after.following(), None);
    }
}
