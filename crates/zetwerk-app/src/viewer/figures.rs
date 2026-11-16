//! The figure manager: the figures and tables of a file, in one list.
//!
//! `:figures` (or `gF`) lists every figure of the file being edited in
//! the order they stand in, each with what it shows, its number, its
//! caption, its label and the file of its image; tables and images
//! that stand in no figure are listed with them. Enter on one goes to
//! it and shows all that can be said of it: its caption and its label
//! are typed anew; an image gets another file or another width; a
//! table gets and loses rows and columns at its end, a header, rules,
//! and how its columns are aligned; a figure is taken out. After each
//! change the list of what can be said comes back, and the page behind
//! it shows the figure as it is now. `:figure` shows the same of the
//! figure or table the cursor is in.
//!
//! Nothing of this is kept anywhere but in the document: each decision
//! is an argument of the call that makes the figure, written and
//! changed here so that nobody has to look at it (see `figure_text`).
//! Every change is one step for undo.
//!
//! A label that is changed takes the references to it along, if the
//! user says so: where the file refers to the old label, it is asked
//! whether those references follow (they do unless the user says no).
//! Only this file is looked at; a label that is taken in it is not
//! given twice.

use std::ops::Range;

use typst::syntax::Source;

use super::assist::Target;
use super::{Edited, Viewer};
use crate::figure_text::{self, Figure, Kind};

/// What is said where no file is being edited.
const NOT_EDITING: &str = "no file is being edited";

/// A change to the text: the bytes to replace, and what with.
type Edit = (Range<usize>, String);

/// How wide an image can be: as listed, and as written. The smaller
/// before the larger: of two that begin as typed, the nearer is first.
const WIDTHS: &[(&str, &str)] = &[
    ("as large as it is", "auto"),
    ("10%", "10%"),
    ("20%", "20%"),
    ("25%", "25%"),
    ("30%", "30%"),
    ("40%", "40%"),
    ("50%", "50%"),
    ("60%", "60%"),
    ("70%", "70%"),
    ("75%", "75%"),
    ("80%", "80%"),
    ("90%", "90%"),
    ("100%", "100%"),
];

/// Where a figure can be put: as listed, and as written.
const PLACEMENTS: &[(&str, &str)] = &[
    ("where it is written", "none"),
    ("at the top or the bottom of a page, as fits", "auto"),
    ("at the top of a page", "top"),
    ("at the bottom of a page", "bottom"),
];

/// How all columns of a table can be aligned: as listed, and as
/// written.
const ALIGNMENTS: &[(&str, &str)] = &[
    ("as usual", "auto"),
    ("left", "left"),
    ("center", "center"),
    ("right", "right"),
];

/// What can be chosen or typed for something of a figure.
struct Asked {
    /// What the list is called.
    title: &'static str,
    /// What it can be: as listed, and as written.
    choices: Vec<(String, String)>,
    /// What is said of typing it.
    typing: &'static str,
    /// What it is now, as written.
    now: Option<String>,
}

/// What a value written for one of `choices` is listed as: the choice
/// it is, or else itself.
fn listed(choices: &[(&str, &str)], written: Option<&str>) -> String {
    let written = written.unwrap_or_else(|| choices.first().map_or("", |(_, usual)| usual));
    let choice = choices.iter().find(|(_, value)| *value == written);
    choice.map_or(written, |(label, _)| *label).to_owned()
}

/// Whether the table of `figure` is a `table`, which has rules, and not
/// a grid, which has none.
fn has_rules(source: &Source, figure: &Figure) -> bool {
    let from = figure.table.and_then(|table| source.text().get(table..));
    from.is_some_and(|from| from.starts_with("table"))
}

/// All that is said of `figure` of `source`, row by row: what the row
/// is for (nothing, where it only tells) and how it is listed.
fn card(source: &Source, figure: &Figure) -> Vec<(&'static str, String)> {
    let or_none = |now: Option<&str>| now.filter(|now| !now.is_empty()).unwrap_or("–").to_owned();
    let mut rows = vec![("go", "go there".to_owned())];
    let caption = match (&figure.caption, figure.framed) {
        (None, false) => "–  (one makes it a figure with a number)".to_owned(),
        (caption, _) => or_none(caption.as_deref()),
    };
    rows.push(("caption", format!("caption\t{caption}")));
    if figure.marked {
        let label = figure.label.as_ref().map(|label| format!("<{label}>"));
        rows.push(("label", format!("label\t{}", or_none(label.as_deref()))));
    }
    if let Some(image) = figure.image {
        if let Some((_, file)) = &figure.file {
            rows.push(("file", format!("file\t{file}")));
        }
        let width = figure_text::named_text(source, image, "width");
        rows.push(("width", format!("width\t{}", listed(WIDTHS, width))));
    }
    if figure.framed {
        let placement = figure_text::named_text(source, figure.call, "placement");
        let placement = listed(PLACEMENTS, placement);
        rows.push(("placement", format!("placement\t{placement}")));
    }
    if let Some(start) = figure.table {
        match figure_text::table_from(source, start) {
            Some(table) => {
                let header = if table.header > 0 { "on" } else { "off" };
                rows.push(("columns", format!("columns\t{}", table.columns)));
                rows.push(("rows", format!("rows\t{}", figure_text::rows_of(&table))));
                rows.push(("header", format!("header row\t{header}")));
            }
            None => rows.push((
                "",
                "rows and columns\tit has more to it than cells: not changed here".to_owned(),
            )),
        }
        let align = figure_text::named_text(source, start, "align");
        let align = listed(ALIGNMENTS, align);
        let align = align.trim_start_matches('(').trim_end_matches(')');
        rows.push(("align", format!("columns aligned\t{align}")));
        if has_rules(source, figure) {
            let rules = match figure_text::named_text(source, start, "stroke") {
                None => "on".to_owned(),
                Some("none") => "off".to_owned(),
                Some(own) => format!("on  ({own})"),
            };
            rows.push(("rules", format!("rules\t{rules}")));
        }
    }
    if figure.marked {
        rows.push(("remove", "take it out of the document".to_owned()));
    }
    rows
}

/// What `what` of `figure` can be, to choose from or to type. `None`
/// if it is nothing that is asked for.
fn asked(source: &Source, figure: &Figure, what: &str) -> Option<Asked> {
    fn own(choices: &[(&str, &str)]) -> Vec<(String, String)> {
        let own = |(listed, written): &(&str, &str)| ((*listed).to_owned(), (*written).to_owned());
        choices.iter().map(own).collect()
    }
    let now = |call: Option<usize>, key: &str| {
        figure_text::named_text(source, call?, key).map(str::to_owned)
    };
    let (title, choices, typing, now) = match what {
        "caption" => (
            "caption",
            Vec::new(),
            "type it anew, then Enter  (a single - takes it away)",
            figure.caption.clone(),
        ),
        "label" => (
            "label",
            Vec::new(),
            "type the name it is referred to by, then Enter  (a single - takes it away)",
            figure.label.clone(),
        ),
        "file" => (
            "file",
            Vec::new(),
            "or type the path of the image, then Enter",
            figure.file.as_ref().map(|(_, file)| file.clone()),
        ),
        "width" => (
            "width",
            own(WIDTHS),
            "or type it: 70 (percent of the line), 7.5cm",
            now(figure.image, "width").or(Some("auto".to_owned())),
        ),
        "placement" => (
            "placement",
            own(PLACEMENTS),
            "",
            now(Some(figure.call), "placement").or(Some("none".to_owned())),
        ),
        "align" => (
            "columns aligned",
            own(ALIGNMENTS),
            "or type one for each column: l c r",
            now(figure.table, "align").or(Some("auto".to_owned())),
        ),
        "columns" | "rows" => {
            let table = figure_text::table_from(source, figure.table?)?;
            let (title, has) = if what == "columns" {
                ("columns", table.columns)
            } else {
                ("rows", figure_text::rows_of(&table))
            };
            let mut choices = vec![("one more, at the end".to_owned(), (has + 1).to_string())];
            if has > 1 {
                choices.push(("one fewer: the last goes".to_owned(), (has - 1).to_string()));
            }
            (title, choices, "or type how many, then Enter", None)
        }
        _ => return None,
    };
    Some(Asked {
        title,
        choices,
        typing,
        now,
    })
}

/// The changes to `source` that make `what` of `figure` be `value`, and
/// what is told of them. No changes where it is that already.
///
/// # Errors
///
/// If the figure has no such thing, or it cannot be `value`.
fn changes(
    source: &Source,
    figure: &Figure,
    what: &str,
    value: &str,
) -> Result<(Vec<Edit>, String), String> {
    let one = |edit: Option<Edit>| edit.into_iter().collect::<Vec<Edit>>();
    let table = figure.table.ok_or("it shows no table");
    let told = format!("{what}: {value}");
    match what {
        "caption" if value.is_empty() => Err("what is the caption to be?".to_owned()),
        "caption" => {
            let words = (value != "-").then_some(value);
            Ok((one(figure_text::captioned(source, figure, words)), told))
        }
        "file" => {
            let (range, _) = figure.file.clone().ok_or("it names no file")?;
            Ok((vec![(range, figure_text::quoted(value))], told))
        }
        "width" => {
            let image = figure.image.ok_or("it shows no image")?;
            let width = match value {
                "auto" => None,
                typed => Some(figure_text::width(typed).ok_or_else(|| {
                    format!("the width cannot be {typed}: 70 (percent of the line), 7.5cm")
                })?),
            };
            let edit = figure_text::set_named(source, image, "width", width.as_deref());
            Ok((one(edit), told))
        }
        "placement" => {
            let placement = match value {
                "none" => None,
                "auto" | "top" | "bottom" => Some(value),
                _ => {
                    return Err(format!(
                        "it cannot be placed {value}: auto, top, bottom, none"
                    ));
                }
            };
            if !figure.framed {
                return Err("only a figure is placed: a caption makes it one".to_owned());
            }
            let edit = figure_text::set_named(source, figure.call, "placement", placement);
            Ok((one(edit), told))
        }
        "columns" | "rows" => {
            let table = table?;
            let wanted = value.parse::<usize>().ok();
            let (columns, rows) = if what == "columns" {
                (wanted, None)
            } else {
                (None, wanted)
            };
            let edit = wanted
                .and_then(|_| figure_text::table_resized(source, table, columns, rows))
                .ok_or_else(|| format!("the table cannot have {value} {what}"))?;
            let same = source.text().get(edit.0.clone()) == Some(edit.1.as_str());
            Ok((one((!same).then_some(edit)), told))
        }
        "align" => {
            let align = match value {
                "auto" => None,
                typed => Some(figure_text::alignment(typed).ok_or_else(|| {
                    format!("columns cannot be aligned {typed}: left, center, right, or l c r")
                })?),
            };
            let edit = figure_text::set_named(source, table?, "align", align.as_deref());
            Ok((one(edit), told))
        }
        "rules" => {
            let off = figure_text::named_text(source, table?, "stroke") == Some("none");
            let stroke = (!off).then_some("none");
            let edit = figure_text::set_named(source, table?, "stroke", stroke);
            Ok((
                one(edit),
                format!("rules {}", if off { "on" } else { "off" }),
            ))
        }
        "header" => {
            let table = figure_text::table_from(source, table?);
            let off = table.as_ref().is_some_and(|table| table.header > 0);
            let edit = table
                .and_then(|table| figure_text::header_switched(source, &table))
                .ok_or("the table has no whole first row to be its header")?;
            let told = format!("header row {}", if off { "off" } else { "on" });
            Ok((vec![edit], told))
        }
        _ => Err(format!("a figure has no {what} to set")),
    }
}

impl Viewer {
    /// The figures of the file being edited, in the order they stand
    /// in; tables and images that stand in no figure among them.
    ///
    /// # Errors
    ///
    /// If no file is being edited.
    pub fn figures(&self) -> Result<Vec<Figure>, String> {
        let editor = self.editor.as_ref().ok_or(NOT_EDITING)?;
        Ok(super::pages::not_pages(
            editor.source(),
            figure_text::figures(editor.source()),
        ))
    }

    /// Figure `number` of the file, counting from 1 as the list does.
    fn figure(&self, number: usize) -> Result<Figure, String> {
        let mut figures = self.figures()?;
        let index = number.checked_sub(1).filter(|index| *index < figures.len());
        let index = index.ok_or_else(|| format!("the file has no figure {number}"))?;
        Ok(figures.swap_remove(index))
    }

    /// The commands of the figure manager, without their `figures`:
    /// none lists the figures; `here` shows the one the cursor is in; a
    /// number, that figure, and what follows the number is done to it.
    pub(super) fn figures_command(&mut self, command: &str) {
        let mut words = command.splitn(4, ' ');
        let mut word = || words.next().unwrap_or("").trim();
        let done = match word() {
            "" => self.list_figures(),
            "here" => self.figure_here(),
            number => match number.parse::<usize>() {
                Ok(number) => {
                    let (verb, what, value) = (word(), word(), word());
                    self.figure_command(number, verb, what, value)
                }
                Err(_) => Err(format!("not a command: figures {command}")),
            },
        };
        if let Err(why) = done {
            self.alert(why);
        }
    }

    /// What `verb` does to figure `number`: shows it (`on` a row), goes
    /// to it, asks for or sets `what` of it, takes it out.
    fn figure_command(
        &mut self,
        number: usize,
        verb: &str,
        what: &str,
        value: &str,
    ) -> Result<(), String> {
        match verb {
            "" | "on" => self.open_figure(number, what),
            "go" => {
                let figure = self.figure(number)?;
                self.show_figure(&figure);
                Ok(())
            }
            "ask" => self.ask_figure(number, what),
            "set" if what == "label" => self.label_figure(number, value, None),
            "relabel" => self.label_figure(number, what, Some(value == "all")),
            "set" | "switch" => {
                // The figure comes back whatever became of the change.
                let done = self.set_figure(number, what, value);
                let shown = self.open_figure(number, what);
                done.and(shown)
            }
            "remove" => {
                let figure = self.figure(number)?;
                let lost = self.references_to(&figure);
                let warning = match lost {
                    0 => String::new(),
                    lost => format!("  ({lost} references to it will lead nowhere)"),
                };
                let items = vec![
                    (
                        "no, keep it".to_owned(),
                        Target::Command(format!("figures {number}")),
                    ),
                    (
                        format!("yes, take it out of the document{warning}"),
                        Target::Command(format!("figures {number} remove!")),
                    ),
                ];
                let back = format!("figures {number} on remove");
                self.open_picker_for("take it out?", items, 0, back, None);
                Ok(())
            }
            "remove!" => self.remove_figure(number),
            _ => Err(format!("not a command: figures {number} {verb}")),
        }
    }

    /// How many places of the file refer to the label of `figure`.
    fn references_to(&self, figure: &Figure) -> usize {
        let (Some(editor), Some(label)) = (&self.editor, &figure.label) else {
            return 0;
        };
        figure_text::label_uses(editor.source(), label).1.len()
    }

    /// `:figures`: the figures of the file, the one the cursor is in or
    /// behind chosen.
    fn list_figures(&mut self) -> Result<(), String> {
        let figures = self.figures()?;
        let editor = self.editor.as_ref().ok_or(NOT_EDITING)?;
        let (source, cursor) = (editor.source(), editor.cursor());
        let mut items = Vec::new();
        for (index, figure) in figures.iter().enumerate() {
            let mut more = Vec::new();
            more.extend(figure.label.as_ref().map(|label| format!("<{label}>")));
            more.extend(figure.file.as_ref().map(|(_, file)| file.clone()));
            let table = figure
                .table
                .and_then(|at| figure_text::table_from(source, at));
            more.extend(
                table.map(|table| format!("{} × {}", table.columns, figure_text::rows_of(&table))),
            );
            if !figure.framed {
                more.push("in no figure".to_owned());
            }
            let caption = figure.caption.as_deref().unwrap_or("");
            let listed = format!("{}  {caption}\t{}", figure.name(), more.join("  "));
            let open = Target::Command(format!("figures {}", index + 1));
            items.push((listed, open));
        }
        let within = figures.iter().rposition(|figure| figure.holds(cursor));
        let behind = || figures.iter().rposition(|figure| figure.call <= cursor);
        let chosen = within.or_else(behind).unwrap_or(0);
        self.open_picker("figures", items, chosen);
        Ok(())
    }

    /// `:figure`: all that is said of the figure or table the cursor is
    /// in, the innermost.
    fn figure_here(&mut self) -> Result<(), String> {
        let cursor = self.editor.as_ref().ok_or(NOT_EDITING)?.cursor();
        let here = self
            .figures()?
            .iter()
            .rposition(|figure| figure.holds(cursor));
        match here {
            Some(index) => self.open_figure(index + 1, ""),
            None => Err("the cursor is in no figure or table  (gF lists them)".to_owned()),
        }
    }

    /// Puts the cursor into `figure`, if it is not there, so that the
    /// page shows it: on its first words, which the page is surest to
    /// have. As a jump: `` ` ` `` leads back.
    fn show_figure(&mut self, figure: &Figure) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        if !figure.holds(editor.cursor()) {
            let words = figure_text::words_of(editor.source(), figure);
            editor.jump(words);
        }
        self.follow_cursor();
    }

    /// Goes to figure `number` and puts up all that is said of it, the
    /// row for `on` chosen; or, where nothing says which, the caption
    /// of a figure that has none: it is to be typed next.
    fn open_figure(&mut self, number: usize, on: &str) -> Result<(), String> {
        let figure = self.figure(number)?;
        self.show_figure(&figure);
        let editor = self.editor.as_ref().ok_or(NOT_EDITING)?;
        let rows = card(editor.source(), &figure);
        let lacks = figure.framed && figure.caption.as_deref().is_none_or(str::is_empty);
        let wanted = if on.is_empty() && lacks {
            "caption"
        } else {
            on
        };
        let chosen = rows.iter().position(|(what, _)| *what == wanted);
        let items = rows
            .into_iter()
            .map(|(what, listed)| {
                let target = match what {
                    "" => Target::Nothing,
                    "go" | "remove" => Target::Command(format!("figures {number} {what}")),
                    "rules" | "header" => {
                        Target::Command(format!("figures {number} switch {what}"))
                    }
                    _ => Target::Command(format!("figures {number} ask {what}")),
                };
                (listed, target)
            })
            .collect();
        let title = match (figure.framed, figure.kind) {
            (true, _) => "figure",
            (false, Kind::Table) => "table",
            (false, _) => "image",
        };
        let back = "figures".to_owned();
        self.open_picker_for(title, items, chosen.unwrap_or(0), back, None);
        Ok(())
    }

    /// Asks what `what` of figure `number` is to be: what it can be is
    /// listed, anything else is typed.
    fn ask_figure(&mut self, number: usize, what: &str) -> Result<(), String> {
        let figure = self.figure(number)?;
        let editor = self.editor.as_ref().ok_or(NOT_EDITING)?;
        let mut asked = asked(editor.source(), &figure, what)
            .ok_or_else(|| format!("a figure has no {what} to ask for"))?;
        if what == "file" {
            let files = self.images_beside();
            asked.choices = files.into_iter().map(|file| (file.clone(), file)).collect();
        }
        let set = format!("figures {number} set {what} ");
        let now = asked.now.as_deref();
        let chosen = asked
            .choices
            .iter()
            .position(|(_, written)| Some(written.as_str()) == now);
        let mut items: Vec<(String, Target)> = asked
            .choices
            .iter()
            .map(|(listed, written)| {
                let choose = Target::Command(format!("{set}{written}"));
                (listed.clone(), choose)
            })
            .collect();
        if !asked.typing.is_empty() {
            // What it is now beside what is said of typing it, where
            // nothing listed says so.
            let now = now.filter(|_| asked.choices.is_empty()).unwrap_or("");
            items.push((format!("{}\t{now}", asked.typing), Target::Nothing));
        }
        let back = format!("figures {number} on {what}");
        let source = editor.source().clone();
        let free = (!asked.typing.is_empty()).then_some(set);
        self.open_picker_for(asked.title, items, chosen.unwrap_or(0), back, free);
        // What is typed is offered as itself where it can be that: a
        // file by a name with an ending (there or not: one that is not
        // is said to be missing, not taken for one like it).
        let what = what.to_owned();
        self.picker_accepts(move |typed| match what.as_str() {
            "file" => typed
                .rsplit_once('.')
                .is_some_and(|(name, _)| !name.is_empty()),
            _ => changes(&source, &figure, &what, typed).is_ok(),
        });
        Ok(())
    }

    /// Makes `edits` (from the last to the first) as one step, and
    /// tells of it.
    fn change_figure(&mut self, edits: &[Edit], told: &str) -> Result<(), String> {
        if edits.is_empty() {
            return Ok(());
        }
        let editor = self.editor.as_mut().ok_or(NOT_EDITING)?;
        let edits: Vec<(Range<usize>, &str)> = edits
            .iter()
            .map(|(range, with)| (range.clone(), with.as_str()))
            .collect();
        editor
            .replace(&edits)
            .map_err(|why| format!("not changed: {why}"))?;
        self.text_changed(Edited::Command);
        self.notice(format!("{told}  (u takes it back)"));
        Ok(())
    }

    /// Makes `what` of figure `number` be `value`.
    fn set_figure(&mut self, number: usize, what: &str, value: &str) -> Result<(), String> {
        let figure = self.figure(number)?;
        let editor = self.editor.as_ref().ok_or(NOT_EDITING)?;
        if what == "file" {
            // A file that is not there would leave the page as it was.
            let folder = editor.path().parent().unwrap_or(std::path::Path::new("."));
            let path = match value.strip_prefix('/') {
                Some(from_root) => self.project.root_dir().join(from_root),
                None => folder.join(value),
            };
            if value.is_empty() || !path.is_file() {
                return Err(format!("there is no file {value} beside the document"));
            }
        }
        let (edits, told) = changes(editor.source(), &figure, what, value)?;
        self.change_figure(&edits, &told)
    }

    /// Gives figure `number` the label `typed` (with or without its
    /// angle brackets; a single `-` takes the label away), and shows
    /// the figure again. Where the file refers to the label it had, it
    /// is asked first whether those references are to follow, unless
    /// `references` says so already.
    fn label_figure(
        &mut self,
        number: usize,
        typed: &str,
        references: Option<bool>,
    ) -> Result<(), String> {
        let figure = self.figure(number)?;
        let editor = self.editor.as_ref().ok_or(NOT_EDITING)?;
        let source = editor.source();
        let name = typed.trim_start_matches('<').trim_end_matches('>');
        let referred = self.references_to(&figure);
        let refused = |viewer: &mut Self, why: String| {
            viewer.open_figure(number, "label")?;
            Err(why)
        };
        let (edits, told) = if name == "-" {
            let lost = match referred {
                0 => String::new(),
                lost => format!("; {lost} references to it lead nowhere now"),
            };
            let edits = figure_text::relabelled(source, &figure, None, false);
            (edits, format!("the label is gone{lost}"))
        } else if !figure_text::is_label_name(name) {
            let why = format!("{name} cannot be a label: letters, digits and - _ : . only");
            return refused(self, why);
        } else if figure.label.as_deref() == Some(name) {
            (Vec::new(), String::new())
        } else if let Some(line) = figure_text::label_uses(source, name).0.first() {
            let why = format!("<{name}> is taken: line {line} has it");
            return refused(self, why);
        } else if let (Some(old), None, true) = (&figure.label, references, referred > 0) {
            let items = vec![
                (
                    format!("yes: the {referred} references to <{old}> follow it"),
                    Target::Command(format!("figures {number} relabel {name} all")),
                ),
                (
                    "no: they stay as they are".to_owned(),
                    Target::Command(format!("figures {number} relabel {name} only")),
                ),
            ];
            let back = format!("figures {number} on label");
            self.open_picker_for("references too?", items, 0, back, None);
            return Ok(());
        } else {
            let follow = references.unwrap_or(false) && referred > 0;
            let edits = figure_text::relabelled(source, &figure, Some(name), follow);
            let with = if follow {
                format!(", and {referred} references with it")
            } else {
                String::new()
            };
            (edits, format!("label: <{name}>{with}"))
        };
        let done = self.change_figure(&edits, &told);
        let shown = self.open_figure(number, "label");
        done.and(shown)
    }

    /// Takes figure `number` out of the document, and lists what is
    /// left.
    fn remove_figure(&mut self, number: usize) -> Result<(), String> {
        let figure = self.figure(number)?;
        if !figure.marked {
            return Err("it is part of something else: not taken out here".to_owned());
        }
        let editor = self.editor.as_ref().ok_or(NOT_EDITING)?;
        let range = figure_text::removal(editor.text(), &figure);
        self.change_figure(
            &[(range, String::new())],
            &format!("{} is out", figure.name()),
        )?;
        if self.figures()?.is_empty() {
            return Ok(());
        }
        self.list_figures()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use super::*;
    use crate::viewer::tests::{compile, editing, press};

    const PAPER: &str = "= Results\n\n\
        #figure(\n  image(\"plot.svg\"),\n  caption: [A plot],\n) <fig:plot>\n\n\
        See @fig:plot for more.\n\n\
        #figure(\n  table(\n    columns: 2,\n    [a], [b],\n    [c], [d],\n  ),\n  caption: [],\n)\n\n\
        #image(\"plot.svg\")\n";

    /// A viewer editing `text` with two images beside it.
    fn paper(text: &str) -> (Viewer, std::path::PathBuf) {
        let (mut viewer, directory) = editing(text);
        let image = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"40\" height=\"20\"/>";
        std::fs::write(directory.join("plot.svg"), image).unwrap();
        std::fs::write(directory.join("other.svg"), image).unwrap();
        // As the watcher tells of them.
        viewer.file_changed(&directory.join("plot.svg"));
        viewer.file_changed(&directory.join("other.svg"));
        compile(&mut viewer);
        (viewer, directory)
    }

    fn has(viewer: &Viewer, written: &str) -> bool {
        viewer.text().is_some_and(|text| text.contains(written))
    }

    fn message(viewer: &Viewer) -> String {
        let message = viewer.message.as_ref();
        message.map_or_else(String::new, |message| message.text.clone())
    }

    #[test]
    fn the_figures_are_listed_and_what_is_said_of_one_is_changed() {
        let (mut viewer, directory) = paper(PAPER);
        let figures = viewer.figures().unwrap();
        let names: Vec<String> = figures.iter().map(Figure::name).collect();
        assert_eq!(names, ["Figure 1", "Table 1", "image"]);
        // The list; Enter goes to the figure and shows what is said of it.
        press(&mut viewer, "gF");
        assert!(viewer.is_picking());
        press(&mut viewer, "<CR>");
        assert!(viewer.is_picking());
        let cursor = viewer.editor.as_ref().unwrap().cursor();
        assert!(figures[0].holds(cursor), "{cursor}");
        // A caption is typed anew, as words.
        press(&mut viewer, "caption<CR>The *new* plot<CR>");
        assert!(
            has(&viewer, "caption: [The \\*new\\* plot],"),
            "{:?}",
            viewer.text()
        );
        assert!(viewer.is_picking(), "the figure comes back");
        // Width and placement are chosen or typed.
        press(&mut viewer, "width<CR>80<CR>");
        assert!(
            has(&viewer, "image(\"plot.svg\", width: 80%)"),
            "{:?}",
            viewer.text()
        );
        press(&mut viewer, "width<CR>7.5cm<CR>");
        assert!(has(&viewer, "width: 7.5cm"), "{:?}", viewer.text());
        press(&mut viewer, "width<CR>as large<CR>");
        assert!(has(&viewer, "image(\"plot.svg\"),"), "{:?}", viewer.text());
        press(&mut viewer, "placement<CR>top of<CR>");
        assert!(has(
            &viewer,
            "caption: [The \\*new\\* plot],\n  placement: top,\n)"
        ));
        press(&mut viewer, "placement<CR>where<CR>");
        assert!(!has(&viewer, "placement"), "{:?}", viewer.text());
        // The file is one of those beside the document.
        press(&mut viewer, "file<CR>other<CR>");
        assert!(has(&viewer, "image(\"other.svg\"),"), "{:?}", viewer.text());
        press(&mut viewer, "file<CR>nowhere.png<CR>");
        assert!(has(&viewer, "image(\"other.svg\"),"), "{:?}", viewer.text());
        assert!(message(&viewer).contains("no file nowhere.png"));
        assert!(viewer.is_picking());
        // Escape leads back to the list, then away.
        press(&mut viewer, "<Esc>");
        assert!(viewer.is_picking());
        press(&mut viewer, "<Esc>");
        assert!(!viewer.is_picking());
        // Each change is one step for undo.
        press(&mut viewer, "u");
        assert!(has(&viewer, "image(\"plot.svg\"),"), "{:?}", viewer.text());
        compile(&mut viewer);
        assert_eq!(viewer.document_revision, Some(viewer.revision));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_label_is_given_and_takes_its_references_along() {
        let (mut viewer, directory) = paper(PAPER);
        press(&mut viewer, ":figures 1<CR>");
        // The references are asked about; they follow unless told not to.
        press(&mut viewer, "label<CR>plot<CR>");
        assert!(has(&viewer, "<fig:plot>"), "asked first");
        press(&mut viewer, "<CR>");
        assert!(
            has(&viewer, ") <plot>\n\nSee @plot for more."),
            "{:?}",
            viewer.text()
        );
        press(&mut viewer, "label<CR><lt>fig:one><CR>no<CR>");
        assert!(
            has(&viewer, ") <fig:one>\n\nSee @plot for more."),
            "{:?}",
            viewer.text()
        );
        // What cannot be a label, or is one already, is not written.
        press(&mut viewer, "<Esc><Esc>:figures 2<CR>label<CR>fig:one<CR>");
        assert!(
            message(&viewer).contains("<fig:one> is taken: line 6"),
            "{}",
            message(&viewer)
        );
        press(&mut viewer, "label<CR>two words<CR>");
        assert!(
            message(&viewer).contains("cannot be a label"),
            "{}",
            message(&viewer)
        );
        press(&mut viewer, "label<CR>tab:n<CR>");
        assert!(
            has(&viewer, "caption: [],\n) <tab:n>\n"),
            "{:?}",
            viewer.text()
        );
        press(&mut viewer, "label<CR>-<CR>");
        assert!(has(&viewer, "caption: [],\n)\n"), "{:?}", viewer.text());
        // A label nothing refers to is changed without a question.
        press(&mut viewer, "<Esc><Esc>:figures 1<CR>label<CR>plot<CR>");
        assert!(
            has(&viewer, ") <plot>\n\nSee @plot for more."),
            "{:?}",
            viewer.text()
        );
        assert!(viewer.is_picking());
        compile(&mut viewer);
        assert_eq!(viewer.document_revision, Some(viewer.revision));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_table_gets_rows_columns_a_header_and_its_looks() {
        let (mut viewer, directory) = paper(PAPER);
        // Of the table the cursor is in.
        press(&mut viewer, "/[c]<CR>:figure<CR>");
        assert!(viewer.is_picking(), "{}", message(&viewer));
        press(&mut viewer, "columns<CR>one more<CR>");
        assert!(has(
            &viewer,
            "columns: 3,\n    [a], [b], [],\n    [c], [d], [],\n"
        ));
        press(&mut viewer, "rows<CR>4<CR>");
        assert!(has(
            &viewer,
            "[c], [d], [],\n    [], [], [],\n    [], [], [],\n  ),"
        ));
        press(&mut viewer, "rows<CR>1<CR>");
        assert!(
            has(&viewer, "columns: 3,\n    [a], [b], [],\n  ),"),
            "{:?}",
            viewer.text()
        );
        press(
            &mut viewer,
            "<Esc><Esc>u:figures 2<CR>rows<CR>one fewer<CR>rows<CR>one fewer<CR>",
        );
        assert!(has(&viewer, "[c], [d], [],\n  ),"), "{:?}", viewer.text());
        press(&mut viewer, "header<CR>");
        assert!(has(
            &viewer,
            "table.header([a], [b], []),\n    [c], [d], [],"
        ));
        assert!(viewer.is_picking());
        press(&mut viewer, "aligned<CR>l c r<CR>");
        assert!(has(
            &viewer,
            "columns: 3,\n    align: (left, center, right),\n"
        ));
        press(&mut viewer, "aligned<CR>center<CR>");
        assert!(has(&viewer, "align: center,\n"), "{:?}", viewer.text());
        press(&mut viewer, "aligned<CR>as usual<CR>");
        assert!(!has(&viewer, "align"), "{:?}", viewer.text());
        press(&mut viewer, "rules<CR>");
        assert!(
            has(&viewer, "columns: 3,\n    stroke: none,\n"),
            "{:?}",
            viewer.text()
        );
        press(&mut viewer, "rules<CR>");
        assert!(!has(&viewer, "stroke"), "{:?}", viewer.text());
        press(&mut viewer, "header<CR>");
        assert!(has(
            &viewer,
            "columns: 3,\n    [a], [b], [],\n    [c], [d], [],\n"
        ));
        // What a table cannot be is said, and nothing is written.
        press(&mut viewer, "columns<CR>0<CR>");
        assert!(
            message(&viewer).contains("cannot have 0 columns"),
            "{}",
            message(&viewer)
        );
        press(&mut viewer, "<Esc><Esc>");
        compile(&mut viewer);
        assert_eq!(viewer.document_revision, Some(viewer.revision));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_figure_is_made_of_an_image_and_taken_out() {
        let (mut viewer, directory) = paper(PAPER);
        // An image by itself becomes a figure by its caption.
        press(&mut viewer, "G:figure<CR>");
        assert!(viewer.is_picking(), "{}", message(&viewer));
        press(&mut viewer, "caption<CR>A second plot<CR>");
        assert!(has(
            &viewer,
            "#figure(caption: [A second plot], image(\"plot.svg\"))\n"
        ));
        let names: Vec<String> = viewer.figures().unwrap().iter().map(Figure::name).collect();
        assert_eq!(names, ["Figure 1", "Table 1", "Figure 2"]);
        // Taking one out is asked about first.
        press(&mut viewer, "take<CR>");
        assert!(has(&viewer, "A second plot"));
        press(&mut viewer, "<CR>");
        assert!(has(&viewer, "A second plot"), "kept");
        press(&mut viewer, "take<CR>yes<CR>");
        assert!(!has(&viewer, "A second plot"), "{:?}", viewer.text());
        assert!(
            viewer
                .text()
                .is_some_and(|text| text.ends_with("caption: [],\n)\n\n"))
        );
        assert!(viewer.is_picking(), "the list of what is left");
        press(&mut viewer, "<Esc>u");
        assert!(has(&viewer, "A second plot"));
        // Where there is none, that is said.
        press(&mut viewer, "gg:figure<CR>");
        assert!(!viewer.is_picking());
        assert!(message(&viewer).contains("in no figure or table"));
        compile(&mut viewer);
        assert_eq!(viewer.document_revision, Some(viewer.revision));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_page_behind_shows_the_figure_that_is_changed() {
        let text = "One.\n#pagebreak()\nTwo.\n\
                    #pagebreak()\n#figure(\n  rect(width: 40pt),\n  caption: [Far down],\n)\n";
        let (mut viewer, directory) = editing(text);
        assert_eq!(viewer.state().page, 1);
        press(&mut viewer, "gF<CR>");
        assert_eq!(viewer.state().page, 3);
        // And where it is not left, the cursor stays where it was put.
        let cursor = viewer.editor.as_ref().unwrap().cursor();
        press(&mut viewer, "placement<CR>bottom<CR>");
        compile(&mut viewer);
        assert_eq!(viewer.state().page, 3);
        assert_eq!(viewer.editor.as_ref().unwrap().cursor(), cursor);
        press(&mut viewer, "go<CR>");
        assert!(!viewer.is_picking());
        assert_eq!(viewer.editor.as_ref().unwrap().cursor(), cursor);
        // The jump leads back.
        press(&mut viewer, "``");
        assert_eq!(viewer.editor.as_ref().unwrap().cursor(), 0);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn what_is_said_of_a_figure_is_listed_row_by_row() {
        let source = Source::detached(PAPER);
        let figures = figure_text::figures(&source);
        let rows = |figure: &Figure| -> Vec<String> {
            let rows = card(&source, figure);
            rows.into_iter().map(|(_, listed)| listed).collect()
        };
        assert_eq!(
            rows(&figures[0]),
            [
                "go there",
                "caption\tA plot",
                "label\t<fig:plot>",
                "file\tplot.svg",
                "width\tas large as it is",
                "placement\twhere it is written",
                "take it out of the document",
            ]
        );
        assert_eq!(
            rows(&figures[1]),
            [
                "go there",
                "caption\t–",
                "label\t–",
                "placement\twhere it is written",
                "columns\t2",
                "rows\t2",
                "header row\toff",
                "columns aligned\tas usual",
                "rules\ton",
                "take it out of the document",
            ]
        );
        assert_eq!(
            rows(&figures[2])[1],
            "caption\t–  (one makes it a figure with a number)"
        );
        // Every row that is asked for can be something.
        for figure in &figures {
            for (what, _) in card(&source, figure) {
                let asks = !matches!(what, "" | "go" | "remove" | "rules" | "header");
                assert!(!asks || asked(&source, figure, what).is_some(), "{what}");
            }
        }
    }
}
