//! The table of options: every setting the window has, each with how it
//! is put into the viewer and read back from it (see
//! [`crate::settings`]).
//!
//! An option of another part of the viewer is a few lines here: its
//! name (the section and key of the settings file), its group, what it
//! can be, what it usually is (for an aid on the page: in each of the
//! three sets of aids), a sentence that says what it does, and the two
//! functions. Commands that turn it on and off (`:numbers`) are named,
//! so that what they change is kept: the tests of `viewer/tests/settings.rs`
//! fail for a command that says "on or off" in `g?` and is named by no
//! option, and those of the settings file for a key it reads that no
//! option is for.
//!
//! The sets of aids, and why each aid is in them (see `set` for how
//! they are applied):
//!
//! - The numbers of the lines are for `5j`: wanted where one moves, not
//!   while one writes. They show in normal mode and go with the first
//!   key typed (`normal`), but in "everything".
//! - The frame of what typing goes into stays in all: on the page the
//!   source is never seen, and it is the one thing that says whether a
//!   letter goes into the equation or behind it. The words for empty
//!   places (exponent, numerator) explain what the box already shows:
//!   "everything" only.
//! - The compiler's complaints beside their line stay in all: they are
//!   what is wrong. Tags for what shows as nothing (labels, comments)
//!   are what the page does not show; a writer wants them, a reader not.
//! - Spelling is checked in all (`]s`, `z=`, `:issues`, agents ask for
//!   it), but underlined while writing; grammar, with its doubtful
//!   findings, only with "everything".
//! - The rest of a name in an equation or a command stands in for the
//!   list of completions that would come up otherwise, so it is the
//!   quieter of the two and stays; the rest of a word in running text
//!   is for writing.
//! - What can follow a key comes after a long pause (1.5 s) unless all
//!   aids are wanted: who knows the keys never sees it.
//! - The matches of a search, the flash on what was yanked and the
//!   marks of what `u` changed are what a key asked for, and go by
//!   themselves; they stay in all.

use super::Viewer;
use super::agent::AgentEdits;
use super::{Paging, Panes};
use crate::settings::{Group, Kind, MAYBE, Opt, Preset, Usual, Value, find, opt};

const PAGE: Group = Group::Page;
const ON: Usual = Usual::Is("on");
const OFF: Usual = Usual::Is("off");

/// The value `name` is held at in the viewer, for options the viewer
/// keeps only for the settings (what to begin with next time).
fn held(viewer: &Viewer, name: &str) -> Value {
    viewer.settings.held(name).cloned().unwrap_or_else(|| {
        find(name).map_or(Value::Text(String::new()), |opt| {
            opt.usual_for(Preset::Quiet)
        })
    })
}

/// All options, in the order the panel lists them in their group.
pub(crate) const TABLE: &[Opt] = &[
    opt(
        "aids.preset",
        PAGE,
        Kind::Choice(&["quiet", "writing", "everything"]),
        Usual::Is("quiet"),
        "which aids the page shows: quiet (what is wrong, what is asked for), \
         writing (and spelling, word endings, tags), everything",
        |v| Value::Text(v.settings.preset.name().to_owned()),
        |v, value| {
            if let Some(preset) = Preset::named(value.text()) {
                v.apply_preset(preset);
            }
        },
    )
    .names(&["aids"])
    .on("everything"),
    opt(
        "aids.numbers",
        PAGE,
        Kind::Choice(&["off", "normal", "always"]),
        Usual::By(["normal", "normal", "always"]),
        "lines say how far they are from the cursor, for 5j: normal shows \
         them except while typing",
        |v| {
            Value::Text(
                match (v.numbers, v.settings.numbers_typing) {
                    (false, _) => "off",
                    (true, false) => "normal",
                    (true, true) => "always",
                }
                .to_owned(),
            )
        },
        |v, value| {
            v.numbers = value.text() != "off";
            v.settings.numbers_typing = value.text() == "always";
        },
    )
    .names(&["numbers", "nu", "rnu", "relativenumber"])
    .on("normal"),
    opt(
        "aids.reach",
        PAGE,
        Kind::Number {
            least: 1.0,
            most: 99.0,
            whole: true,
            unit: "lines",
        },
        Usual::Is("99"),
        "how many lines up and down from the cursor are numbered",
        |v| Value::Number(v.settings.reach as f64),
        |v, value| v.settings.reach = value.number() as usize,
    ),
    opt(
        "aids.tags",
        PAGE,
        Kind::Switch,
        Usual::By(["off", "on", "on"]),
        "tags beside the page for what shows as nothing: labels, comments, breaks",
        |v| Value::Switch(!v.margin.hide_tags),
        |v, value| v.margin.hide_tags = !value.is_on(),
    )
    .names(&["tags"]),
    opt(
        "aids.errors",
        PAGE,
        Kind::Switch,
        ON,
        "the compiler's complaints beside the line they are about",
        |v| Value::Switch(!v.margin.hide_errors),
        |v, value| v.margin.hide_errors = !value.is_on(),
    )
    .names(&["errors"]),
    opt(
        "aids.frame",
        PAGE,
        Kind::Switch,
        ON,
        "while typing, a thin frame around what the letters go into: an \
         equation, a cell, a caption",
        |v| Value::Switch(v.focus.on),
        |v, value| v.focus.on = value.is_on(),
    )
    .names(&["hints", "frame"]),
    opt(
        "aids.places",
        PAGE,
        Kind::Switch,
        Usual::By(["off", "off", "on"]),
        "small words beside empty places of equations: exponent, numerator",
        |v| Value::Switch(v.settings.place_words),
        |v, value| v.settings.place_words = value.is_on(),
    ),
    opt(
        "aids.nextkeys",
        PAGE,
        Kind::Switch,
        ON,
        "after g, z, d and the like, a pause lists the keys that can follow",
        |v| Value::Switch(v.hints.next_keys),
        |v, value| v.hints.next_keys = value.is_on(),
    )
    .names(&["nextkeys"]),
    opt(
        "aids.pause",
        PAGE,
        Kind::Number {
            least: 0.0,
            most: 5000.0,
            whole: true,
            unit: "ms",
        },
        Usual::By(["1500", "1500", "600"]),
        "how long the keys have to pause before what can follow is listed",
        |v| Value::Number(v.settings.pause.as_millis() as f64),
        |v, value| {
            v.settings.pause = std::time::Duration::from_millis(value.number() as u64);
        },
    ),
    opt(
        "aids.ghost",
        PAGE,
        Kind::Choice(&["off", "names", "words"]),
        Usual::By(["names", "words", "words"]),
        "the rest of what is typed, faint behind the cursor: names in \
         equations, labels and commands; words: in running text too",
        |v| {
            Value::Text(
                match (v.hints.ghost, v.settings.ghost_words) {
                    (false, _) => "off",
                    (true, false) => "names",
                    (true, true) => "words",
                }
                .to_owned(),
            )
        },
        |v, value| {
            v.hints.ghost = value.text() != "off";
            v.settings.ghost_words = value.text() == "words";
        },
    )
    .names(&["ghost"])
    .on("words"),
    opt(
        "aids.hlsearch",
        PAGE,
        Kind::Switch,
        ON,
        "all matches of a search are marked, until :noh or Escape",
        |v| Value::Switch(v.find.highlight),
        |v, value| {
            v.find.highlight = value.is_on();
            if !value.is_on() {
                v.find.shown = false;
            }
        },
    )
    .names(&["hlsearch", "hls"]),
    opt(
        "aids.feedback",
        PAGE,
        Kind::Switch,
        ON,
        "what is yanked or put flashes; deleting what shows as nothing says so",
        |v| Value::Switch(v.find.feedback),
        |v, value| v.find.feedback = value.is_on(),
    )
    .names(&["feedback"]),
    opt(
        "typing.tab_words",
        Group::Typing,
        Kind::Switch,
        OFF,
        "Tab takes the faint rest of a word in running text (Ctrl-Y always \
         does; names in equations take Tab in any case)",
        |v| Value::Switch(v.settings.tab_words),
        |v, value| v.settings.tab_words = value.is_on(),
    )
    .names(&["tabwords"]),
    opt(
        "typing.dollar",
        Group::Typing,
        Kind::Switch,
        ON,
        "$ in running text begins an equation; off, it is a dollar sign and \
         equations come from the # menu",
        |v| Value::Switch(v.settings.dollar),
        |v, value| v.settings.dollar = value.is_on(),
    ),
    opt(
        "typing.tidy",
        Group::Typing,
        Kind::Switch,
        ON,
        "equations are tidied: a mark that raises nothing goes with what it \
         raised, letters typed together become a product",
        |v| Value::Switch(v.settings.tidy),
        |v, value| v.settings.tidy = value.is_on(),
    ),
    opt(
        "keys.timeoutlen",
        Group::Keys,
        Kind::Number {
            least: 0.0,
            most: 10000.0,
            whole: true,
            unit: "ms",
        },
        Usual::Is("1000"),
        "how long the first keys of a mapping of yours wait for the rest",
        |v| Value::Number(v.settings.timeout as f64),
        |v, value| {
            v.settings.timeout = value.number() as u64;
            let keymap = std::mem::take(&mut v.keymap);
            v.keymap = keymap.with_timeout(v.settings.timeout);
        },
    )
    .names(&["tm"]),
    opt(
        "ai.edit_mode",
        Group::Agents,
        Kind::Choice(&["live", "propose"]),
        Usual::Is("live"),
        "agents change the text (each change one undo step), or propose \
         changes that wait for you",
        |v| {
            Value::Text(
                match v.agent_edits {
                    AgentEdits::Live => "live",
                    AgentEdits::Propose => "propose",
                }
                .to_owned(),
            )
        },
        |v, value| {
            v.agent_edits = if value.text() == "propose" {
                AgentEdits::Propose
            } else {
                AgentEdits::Live
            };
        },
    )
    .names(&["agents"])
    .on("propose"),
    opt(
        "ai.save",
        Group::Agents,
        Kind::Maybe,
        Usual::Is("not said"),
        "whether agents may write the file; not said: only where no window \
         shows it",
        |v| held(v, "ai.save"),
        |v, value| v.settings.hold("ai.save", value.clone()),
    )
    .next_start()
    .on(MAYBE[1]),
    opt(
        "aids.spelling",
        PAGE,
        Kind::Switch,
        Usual::By(["off", "on", "on"]),
        "words no dictionary knows are underlined on the page (]s and \
         :issues find them all the same)",
        |v| Value::Switch(v.settings.spelling_marks),
        |v, value| v.settings.spelling_marks = value.is_on(),
    ),
    opt(
        "aids.grammar",
        PAGE,
        Kind::Switch,
        Usual::By(["off", "off", "on"]),
        "what the grammar checker finds is underlined on the page",
        |v| Value::Switch(v.settings.grammar_marks),
        |v, value| v.settings.grammar_marks = value.is_on(),
    ),
    opt(
        "language.spelling",
        Group::Language,
        Kind::Switch,
        ON,
        "spelling is checked: ]s goes to what is found, z= says what may \
         be meant (aids.spelling: whether it is underlined)",
        |v| Value::Switch(v.language.settings.spelling),
        |v, value| {
            v.language.settings.spelling = value.is_on();
            v.reconfigure_language();
        },
    )
    .names(&["spell", "spelling"]),
    opt(
        "language.grammar",
        Group::Language,
        Kind::Switch,
        ON,
        "grammar is checked too (aids.grammar: whether it is underlined)",
        |v| Value::Switch(v.language.settings.grammar),
        |v, value| {
            v.language.settings.grammar = value.is_on();
            v.reconfigure_language();
        },
    )
    .names(&["grammar"]),
    opt(
        "language.language",
        Group::Language,
        Kind::Text,
        Usual::Is(""),
        "check in this language (de, nl, en-GB) whatever the document says; \
         none: as it says",
        |v| Value::Text(v.language.settings.language.clone().unwrap_or_default()),
        |v, value| {
            let language = value.text().trim();
            v.language.settings.language = (!language.is_empty()).then(|| language.to_owned());
            v.reconfigure_language();
        },
    )
    .names(&["spelllang"])
    .session(&["spell"]),
    opt(
        "language.dictionaries",
        Group::Language,
        Kind::List,
        Usual::Is(""),
        "more directories to look for Hunspell's dictionaries in, first",
        |v| held(v, "language.dictionaries"),
        |v, value| v.settings.hold("language.dictionaries", value.clone()),
    )
    .next_start(),
    opt(
        "language.languagetool",
        Group::Language,
        Kind::Text,
        Usual::Is(""),
        "a LanguageTool server of yours for the grammar of German, Dutch and \
         English: http://localhost:8081",
        |v| held(v, "language.languagetool"),
        |v, value| {
            v.settings.hold("language.languagetool", value.clone());
            v.language_server_from_settings();
        },
    ),
    opt(
        "language.allow_remote",
        Group::Language,
        Kind::Switch,
        OFF,
        "whether that server may be another machine: then the text of your \
         documents is sent there",
        |v| held(v, "language.allow_remote"),
        |v, value| {
            v.settings.hold("language.allow_remote", value.clone());
            v.language_server_from_settings();
        },
    )
    .asks(),
    opt(
        "view.layout",
        Group::Window,
        Kind::Choice(&["page", "split", "source"]),
        Usual::Is("page"),
        "what the window shows when a document is opened: the page, the \
         source, or both (gs switches for now)",
        |v| held(v, "view.layout"),
        |v, value| {
            v.settings.hold("view.layout", value.clone());
            if v.editor.is_some() {
                v.panes = match value.text() {
                    "split" => Panes::Split,
                    "source" => Panes::Source,
                    _ => Panes::Page,
                };
                v.arrange();
                v.follow_cursor();
            }
        },
    )
    .session(&["view", "view split", "view source", "view page"]),
    opt(
        "view.slides",
        Group::Window,
        Kind::Switch,
        OFF,
        "pages are shown one at a time, whole (:slides switches for now)",
        |v| held(v, "view.slides"),
        |v, value| {
            v.settings.hold("view.slides", value.clone());
            let paging = if value.is_on() {
                Paging::Slides
            } else {
                Paging::Scroll
            };
            if v.paging != paging {
                v.set_paging(paging);
            }
        },
    )
    .session(&["slides"]),
    opt(
        "view.ui_scale",
        Group::Window,
        Kind::Number {
            least: 0.5,
            most: 4.0,
            whole: false,
            unit: "times",
        },
        Usual::Is("1"),
        "how large the status line, the lists and the source are (:ui + and \
         :ui - while open)",
        |v| Value::Number((v.ui * 100.0).round() / 100.0),
        |v, value| v.set_ui_scale(value.number()),
    )
    .names(&["ui"]),
    opt(
        "power.mode",
        Group::Power,
        Kind::Choice(&["auto", "saver", "performance"]),
        Usual::Is("auto"),
        "how much of the machine zetwerk may use: auto saves on battery",
        |v| held(v, "power.mode"),
        |v, value| v.settings.hold("power.mode", value.clone()),
    )
    .next_start(),
    opt(
        "power.parts",
        Group::Power,
        Kind::Switch,
        ON,
        "what changes is typeset by itself at once, all of it when typing \
         pauses",
        |v| Value::Switch(v.in_parts),
        |v, value| v.in_parts = value.is_on(),
    )
    .names(&["parts"]),
    // Of the sections of reviewing, talks, who wrote what and reports:
    // read as the window opens, and kept for the next start.
    opt(
        "review.marks",
        Group::Agents,
        Kind::Choice(&["all", "current", "none"]),
        Usual::Is("all"),
        "the suggestions of agents marked on the page: all, the one at the \
         cursor, none",
        |v| held(v, "review.marks"),
        |v, value| v.settings.hold("review.marks", value.clone()),
    )
    .next_start(),
    opt(
        "review.instructions",
        Group::Agents,
        Kind::Choice(&["document", "everywhere"]),
        Usual::Is("document"),
        "an instruction made from a suggestion holds for this document, or \
         for every document",
        |v| held(v, "review.instructions"),
        |v, value| v.settings.hold("review.instructions", value.clone()),
    )
    .next_start(),
    opt(
        "present.clock",
        Group::Window,
        Kind::Switch,
        OFF,
        "the time shows in a corner of the slides from the start of a talk (t)",
        |v| held(v, "present.clock"),
        |v, value| v.settings.hold("present.clock", value.clone()),
    )
    .next_start(),
    opt(
        "present.duration",
        Group::Window,
        Kind::Text,
        Usual::Is(""),
        "how long a talk is to take (20min, 1h30), unless :present says",
        |v| held(v, "present.duration"),
        |v, value| v.settings.hold("present.duration", value.clone()),
    )
    .next_start(),
    opt(
        "authors.view",
        Group::Page,
        Kind::Choice(&["off", "origin", "age", "unread"]),
        Usual::Is("off"),
        "the page tinted by who wrote it, by its age, or what agents wrote \
         that you have not read (gH switches for now)",
        |v| held(v, "authors.view"),
        |v, value| v.settings.hold("authors.view", value.clone()),
    )
    .next_start(),
    opt(
        "authors.hide_unknown",
        Group::Page,
        Kind::Switch,
        OFF,
        "text of unknown origin is left untinted",
        |v| held(v, "authors.hide_unknown"),
        |v, value| v.settings.hold("authors.hide_unknown", value.clone()),
    )
    .next_start(),
    opt(
        "report.keys",
        Group::Window,
        Kind::Number {
            least: 0.0,
            most: 100_000.0,
            whole: true,
            unit: "keys",
        },
        Usual::Is("500"),
        "keys kept for a report (Ctrl-G, :report); 0 keeps none",
        |v| held(v, "report.keys"),
        |v, value| v.settings.hold("report.keys", value.clone()),
    )
    .next_start(),
];
