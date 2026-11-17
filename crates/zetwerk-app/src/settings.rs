//! The user's settings as one table: every option the window has, what
//! it can be, what holds where the user says nothing, one sentence on
//! what it does, and how it is put into a running viewer and read back
//! from it.
//!
//! The table itself is in `viewer::options`, beside the viewer whose
//! insides its entries reach; what is here does not need a viewer: the
//! kinds of values, how they are typed (`:set numbers=always`), how they
//! are written in the settings file (`~/.config/zetwerk/config.toml`)
//! and read from it, and the three sets of aids on the page.
//!
//! An option's name is where the settings file keeps it: `aids.numbers`
//! is `numbers` under `[aids]`. What the user changes in the window is
//! written there, and only where it is not what holds anyway; the
//! writing itself is the program's (see [`Writer`]), which keeps the
//! file as the user wrote it.
//!
//! Adding an option is one entry of `viewer::options::TABLE`; the tests
//! there and in the settings file's reader say where one is missing.

use std::path::PathBuf;
use std::sync::Arc;

use crate::viewer::Viewer;

/// What an option is about: the panel lists them so, in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Page,
    Typing,
    Keys,
    Agents,
    Language,
    Window,
    Power,
    Files,
}

impl Group {
    pub const ALL: [Self; 8] = [
        Self::Page,
        Self::Typing,
        Self::Keys,
        Self::Agents,
        Self::Language,
        Self::Window,
        Self::Power,
        Self::Files,
    ];

    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Page => "what is shown on the page",
            Self::Typing => "typing",
            Self::Keys => "keys",
            Self::Agents => "agents",
            Self::Language => "language",
            Self::Window => "the window",
            Self::Power => "power",
            Self::Files => "files",
        }
    }
}

/// What an option can be.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// On or off.
    Switch,
    /// One of these.
    Choice(&'static [&'static str]),
    /// A number from `least` to `most`; with `whole`, without a fraction.
    /// `unit` is what it counts, as it is shown.
    Number {
        least: f64,
        most: f64,
        whole: bool,
        unit: &'static str,
    },
    /// Any text; nothing at all where it is empty.
    Text,
    /// Some texts, typed apart by commas, written as a list.
    List,
    /// Not said, yes or no: not written, `true` or `false`.
    Maybe,
}

/// What [`Kind::Maybe`] can be, as it is shown and typed.
pub const MAYBE: &[&str] = &["not said", "yes", "no"];

/// The value of an option. That of a choice is a text.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Switch(bool),
    Number(f64),
    Text(String),
    List(Vec<String>),
}

impl Value {
    /// The value as it is shown and as it would be typed.
    #[must_use]
    pub fn shown(&self) -> String {
        match self {
            Self::Switch(true) => "on".to_owned(),
            Self::Switch(false) => "off".to_owned(),
            Self::Number(number) => format!("{number}"),
            Self::Text(text) if text.is_empty() => "(none)".to_owned(),
            Self::Text(text) => text.clone(),
            Self::List(texts) if texts.is_empty() => "(none)".to_owned(),
            Self::List(texts) => texts.join(", "),
        }
    }

    /// Whether this is a switch that is on.
    #[must_use]
    pub fn is_on(&self) -> bool {
        matches!(self, Self::Switch(true))
    }

    /// The text of a choice or a text; nothing of anything else.
    #[must_use]
    pub fn text(&self) -> &str {
        match self {
            Self::Text(text) => text,
            _ => "",
        }
    }

    /// The number, of a number; else nothing.
    #[must_use]
    pub fn number(&self) -> f64 {
        match self {
            Self::Number(number) => *number,
            _ => 0.0,
        }
    }
}

/// A value as the settings file holds it.
#[derive(Debug, Clone, PartialEq)]
pub enum Written {
    Bool(bool),
    Integer(i64),
    Float(f64),
    Text(String),
    List(Vec<String>),
}

/// The sets of aids over the page, from the fewest to all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    /// The page, the cursor, the status line; what is needed when
    /// something is wrong, and what is asked for.
    Quiet,
    /// That, and spelling, the rest of a word, the tags in the margin.
    Writing,
    /// All aids there are.
    Everything,
}

impl Preset {
    pub const ALL: [Self; 3] = [Self::Quiet, Self::Writing, Self::Everything];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Quiet => "quiet",
            Self::Writing => "writing",
            Self::Everything => "everything",
        }
    }

    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|preset| preset.name() == name)
    }

    /// The one after this, as `za` goes round them.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Quiet => Self::Writing,
            Self::Writing => Self::Everything,
            Self::Everything => Self::Quiet,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Quiet => 0,
            Self::Writing => 1,
            Self::Everything => 2,
        }
    }
}

/// What holds where the user says nothing: one value, or one for each
/// set of aids (quiet, writing, everything), as typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Usual {
    Is(&'static str),
    By([&'static str; 3]),
}

/// When a change of an option takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    /// At once: the page behind the panel shows it.
    Now,
    /// When zetwerk is started next.
    NextStart,
}

/// An option.
pub struct Opt {
    /// Its name, which is where the settings file keeps it: `aids.numbers`
    /// is `numbers` under `[aids]`.
    pub name: &'static str,
    pub group: Group,
    pub kind: Kind,
    pub usual: Usual,
    /// One sentence a writer understands.
    pub about: &'static str,
    pub when: When,
    /// Other names `:set` knows it by, and the commands that change it
    /// (`:numbers`): what they change is kept as `:set` keeps it.
    pub names: &'static [&'static str],
    /// Commands that change it for the time being only, not kept:
    /// `:slides` and `gs` show the document otherwise for now.
    pub session: &'static [&'static str],
    /// Of a choice, what `:set name` and `:set name!` make it.
    pub on: &'static str,
    /// Whether a change in the panel asks first, even of a switch.
    pub asks: bool,
    /// Reads it from a viewer, and puts it into one.
    pub get: fn(&Viewer) -> Value,
    pub set: fn(&mut Viewer, &Value),
}

/// An option of `kind` in `group`, with the usual for the rest.
#[must_use]
pub const fn opt(
    name: &'static str,
    group: Group,
    kind: Kind,
    usual: Usual,
    about: &'static str,
    get: fn(&Viewer) -> Value,
    set: fn(&mut Viewer, &Value),
) -> Opt {
    Opt {
        name,
        group,
        kind,
        usual,
        about,
        when: When::Now,
        names: &[],
        session: &[],
        on: "",
        asks: false,
        get,
        set,
    }
}

impl Opt {
    #[must_use]
    pub const fn names(mut self, names: &'static [&'static str]) -> Self {
        self.names = names;
        self
    }

    #[must_use]
    pub const fn session(mut self, commands: &'static [&'static str]) -> Self {
        self.session = commands;
        self
    }

    #[must_use]
    pub const fn on(mut self, choice: &'static str) -> Self {
        self.on = choice;
        self
    }

    #[must_use]
    pub const fn next_start(mut self) -> Self {
        self.when = When::NextStart;
        self
    }

    #[must_use]
    pub const fn asks(mut self) -> Self {
        self.asks = true;
        self
    }

    /// The section of the settings file it is kept in, and its key there.
    #[must_use]
    pub fn place(&self) -> (&'static str, &'static str) {
        self.name.split_once('.').unwrap_or(("", self.name))
    }

    /// Its name without its section: what `:set` knows it by first.
    #[must_use]
    pub fn short(&self) -> &'static str {
        self.place().1
    }

    /// Whether the set of aids decides what it is.
    #[must_use]
    pub fn by_preset(&self) -> bool {
        matches!(self.usual, Usual::By(_))
    }

    /// What holds where the user says nothing, with the aids of `preset`.
    #[must_use]
    pub fn usual_for(&self, preset: Preset) -> Value {
        let typed = match self.usual {
            Usual::Is(typed) => typed,
            Usual::By(each) => each[preset.index()],
        };
        self.parse(typed).unwrap_or(Value::Text(String::new()))
    }

    /// What `typed` makes it, or why it cannot be that.
    pub fn parse(&self, typed: &str) -> Result<Value, String> {
        let typed = typed.trim();
        match self.kind {
            Kind::Switch => match typed {
                "on" | "yes" | "true" | "1" => Ok(Value::Switch(true)),
                "off" | "no" | "false" | "0" => Ok(Value::Switch(false)),
                _ => Err(format!("{} is on or off, not {typed}", self.name)),
            },
            Kind::Choice(_) | Kind::Maybe => {
                let choices = self.choices();
                if let Some(exact) = choices.iter().find(|choice| **choice == typed) {
                    return Ok(Value::Text((*exact).to_owned()));
                }
                // As much of one as tells it from the others.
                let begun: Vec<&str> = choices
                    .iter()
                    .copied()
                    .filter(|choice| !typed.is_empty() && choice.starts_with(typed))
                    .collect();
                match begun.as_slice() {
                    [one] => Ok(Value::Text((*one).to_owned())),
                    _ => Err(format!(
                        "{} is one of {}, not {typed}",
                        self.name,
                        choices.join(", ")
                    )),
                }
            }
            Kind::Number {
                least,
                most,
                whole,
                unit,
            } => {
                let number = typed.trim_end_matches(|c: char| c.is_alphabetic() || c == '%');
                let fine = number
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|number| number.is_finite());
                match fine {
                    Some(number)
                        if (least..=most).contains(&number)
                            && (!whole || number.fract() == 0.0) =>
                    {
                        Ok(Value::Number(number))
                    }
                    _ => Err(format!(
                        "{} is a {}number from {least} to {most}{}, not {typed}",
                        self.name,
                        if whole { "whole " } else { "" },
                        if unit.is_empty() {
                            String::new()
                        } else {
                            format!(" ({unit})")
                        }
                    )),
                }
            }
            Kind::Text => Ok(Value::Text(typed.to_owned())),
            Kind::List => Ok(Value::List(
                typed
                    .split(',')
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .map(str::to_owned)
                    .collect(),
            )),
        }
    }

    /// `value` as the settings file holds it; nothing where it is not
    /// written at all.
    #[must_use]
    pub fn written(&self, value: &Value) -> Option<Written> {
        Some(match (self.kind, value) {
            (Kind::Maybe, Value::Text(text)) => match text.as_str() {
                "yes" => Written::Bool(true),
                "no" => Written::Bool(false),
                _ => return None,
            },
            (_, Value::Switch(on)) => Written::Bool(*on),
            (Kind::Number { whole: true, .. }, Value::Number(number)) => {
                Written::Integer(number.round() as i64)
            }
            (_, Value::Number(number)) => Written::Float(*number),
            (_, Value::Text(text)) => Written::Text(text.clone()),
            (_, Value::List(texts)) => Written::List(texts.clone()),
        })
    }

    /// What the settings file's `written` makes it, or why it cannot be
    /// that.
    pub fn read(&self, written: &Written) -> Result<Value, String> {
        let wrong = || {
            let wanted = match self.kind {
                Kind::Switch | Kind::Maybe => "true or false",
                Kind::Number { .. } => "a number",
                Kind::List => "a list of texts",
                Kind::Choice(_) | Kind::Text => "a text",
            };
            format!("{} should be {wanted}", self.name)
        };
        match (self.kind, written) {
            (Kind::Switch, Written::Bool(on)) => Ok(Value::Switch(*on)),
            (Kind::Maybe, Written::Bool(on)) => {
                Ok(Value::Text(if *on { "yes" } else { "no" }.to_owned()))
            }
            (Kind::Number { .. }, Written::Integer(number)) => {
                self.parse(&(*number as f64).to_string())
            }
            (Kind::Number { .. }, Written::Float(number)) => self.parse(&number.to_string()),
            (Kind::Choice(_) | Kind::Text, Written::Text(text)) => self.parse(text),
            (Kind::List, Written::List(texts)) => Ok(Value::List(texts.clone())),
            _ => Err(wrong()),
        }
    }

    /// What it can be, to choose from.
    #[must_use]
    pub fn choices(&self) -> Vec<&'static str> {
        match self.kind {
            Kind::Switch => vec!["on", "off"],
            Kind::Choice(choices) => choices.to_vec(),
            Kind::Maybe => MAYBE.to_vec(),
            Kind::Number { .. } | Kind::Text | Kind::List => Vec::new(),
        }
    }
}

/// All options, in the order the panel lists them in each group.
#[must_use]
pub fn table() -> &'static [Opt] {
    crate::viewer::options::TABLE
}

/// The option `name` stands for: by its whole name, by its name without
/// its section where only one has that, or by another of its names.
#[must_use]
pub fn find(name: &str) -> Option<&'static Opt> {
    let table = table();
    if let Some(whole) = table.iter().find(|opt| opt.name == name) {
        return Some(whole);
    }
    let short: Vec<&Opt> = table.iter().filter(|opt| opt.short() == name).collect();
    if let [one] = short.as_slice() {
        return Some(one);
    }
    table.iter().find(|opt| opt.names.contains(&name))
}

/// A change of the settings file: the value at `path` (its section and
/// key, or for a mapping `keys`, `normal` and the keys) is to be
/// `value`, or to go.
#[derive(Debug, Clone, PartialEq)]
pub struct Edit {
    pub path: Vec<String>,
    pub value: Option<Written>,
}

/// What writes changes into the settings file: all of them at once, or
/// nothing and why.
pub type Writer = Arc<dyn Fn(&[Edit]) -> Result<(), String> + Send + Sync>;

/// What the settings file says, as the window is opened with it.
#[derive(Clone, Default)]
pub struct Stored {
    /// The options it sets, each with its value.
    pub explicit: Vec<(&'static str, Value)>,
    /// The user's own keys, as written: of normal mode and of typing.
    pub normal: Vec<(String, String)>,
    pub insert: Vec<(String, String)>,
    /// What writes changes back; none where there is no file to keep
    /// them in (and in tests).
    pub writer: Option<Writer>,
    /// The file, to say where things are kept.
    pub file: Option<PathBuf>,
    /// Whether the file was there when zetwerk was started: whoever has
    /// one is told once that the page has become quieter.
    pub existed: bool,
    /// Whether a window shows what is opened: only there is anything
    /// told.
    pub window: bool,
}

impl Stored {
    /// What the file says `name` is, if it says.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.explicit
            .iter()
            .find(|(set, _)| *set == name)
            .map(|(_, value)| value)
    }
}
