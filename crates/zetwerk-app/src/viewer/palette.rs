//! Everything there is to do, in one list; and how the document as a
//! whole is to look, without writing the rules that say so.
//!
//! A program that is worked by keys alone has to say what its keys are.
//! `g?` (or `:help`) puts up all it can do, each with the keys for it,
//! narrowed by typing; Enter does what is chosen.
//!
//! How a document looks (its font, its paper, its margins) is said by
//! `#set` rules before the text. They are written and changed here, by
//! commands: nobody has to look at them.

use typst::World;

use super::assist::Target;
use super::{Edited, Viewer};
use crate::typst_text;

/// What an entry of the list does when it is chosen.
enum Does {
    /// Runs a command.
    Run(&'static str),
    /// Begins a command for the user to finish.
    Ask(&'static str),
    /// Nothing: it only says what a key is for.
    Tells,
}

/// All there is to do: what it is, the keys for it, and what choosing it
/// does.
const ALL: &[(&str, &str, Does)] = &[
    ("write: begin typing at the cursor", "i  a  o", Does::Tells),
    (
        "put in an equation, figure, table, … (while typing)",
        "#",
        Does::Tells,
    ),
    ("equation (while typing)", "$", Does::Tells),
    (
        "take what is listed or shown faint; the next ‹place to fill in›",
        "Tab",
        Does::Tells,
    ),
    (
        "bold, italic (while typing, or of what is selected)",
        "Ctrl-B  Ctrl-I",
        Does::Tells,
    ),
    (
        "bold, italic, a heading: of what a motion covers",
        "gb  gm  g+",
        Does::Tells,
    ),
    (
        "refer to a section, figure, equation, source (while typing)",
        "@",
        Does::Tells,
    ),
    (
        "leave a caption, a footnote, a cell of a table",
        "Enter",
        Does::Tells,
    ),
    ("undo, redo", "u  Ctrl-R", Does::Tells),
    (
        "back to where a jump came from, and on again",
        "Ctrl-O  Ctrl-I",
        Does::Tells,
    ),
    (
        "search for what is read on the page; \\= first: as written",
        "/  ?  n  N",
        Does::Tells,
    ),
    ("search for the word at the cursor", "*  #", Does::Tells),
    (
        "change the next match of the search; . the one after",
        "cgn",
        Does::Tells,
    ),
    (
        "replace what is written, in this line or (%) in all",
        ":s/this/that/  :%s/this/that/g",
        Does::Ask("%s/"),
    ),
    ("hide the matches of the search", ":noh", Does::Run("noh")),
    (
        "show the matches of searches, or not",
        ":hlsearch",
        Does::Run("hlsearch"),
    ),
    (
        "show what is yanked and put, tell what deleting took, or not",
        ":feedback",
        Does::Run("feedback"),
    ),
    (
        "join this paragraph with the next; part it at a blank",
        "J  r Enter",
        Does::Tells,
    ),
    (
        "put a character in place of what the cursor is on",
        "r",
        Does::Tells,
    ),
    ("take away an empty part of an equation", "x", Does::Tells),
    (
        "history: every step of the text, to look at and go back to",
        "gh",
        Does::Run("history"),
    ),
    ("save", ":w", Does::Run("w")),
    ("save everything and leave", ":x", Does::Run("x")),
    ("leave", ":q", Does::Run("q")),
    ("export as PDF", ":pdf", Does::Run("pdf")),
    (
        "export some pages as PDF",
        ":pdf 3-7 [file]",
        Does::Ask("pdf "),
    ),
    (
        "export pages as PNG images",
        ":png [pages] [300dpi] [file]",
        Does::Ask("png "),
    ),
    (
        "export pages as SVG images",
        ":svg [pages] [file]",
        Does::Ask("svg "),
    ),
    (
        "export the figure at the cursor as PNG",
        ":figure png [300dpi] [file]",
        Does::Run("figure png"),
    ),
    (
        "export the figure at the cursor as SVG",
        ":figure svg [file]",
        Does::Run("figure svg"),
    ),
    (
        "how many words, characters, pages",
        ":count",
        Does::Run("count"),
    ),
    ("go to a heading", "go", Does::Run("outline")),
    (
        "jump: a label on every word in view; type one to go there",
        "gw",
        Does::Run("jump"),
    ),
    ("go to a label", ":labels", Does::Run("labels")),
    (
        "structure: the headings — move sections, change their level",
        "gO",
        Does::Run("structure"),
    ),
    (
        "section: move it up, before the one before it",
        ":section up",
        Does::Run("section up"),
    ),
    (
        "section: move it down, behind the one behind it",
        ":section down",
        Does::Run("section down"),
    ),
    (
        "section: make it deeper, with all under it",
        ":section deeper",
        Does::Run("section deeper"),
    ),
    (
        "section: make it shallower, with all under it",
        ":section shallower",
        Does::Run("section shallower"),
    ),
    (
        "check references: to nothing, labels unused or there twice",
        ":references",
        Does::Run("references"),
    ),
    (
        "go to what a reference refers to, to where a name is defined",
        "gd",
        Does::Run("definition"),
    ),
    (
        "go to a line of the text (on a page: a paragraph, as a rule)",
        ":<number>",
        Does::Ask(""),
    ),
    (
        "go to the next page, the page before",
        "]]  [[",
        Does::Run("page next"),
    ),
    ("open a file of the document", ":files", Does::Run("files")),
    ("open a file by name", ":e <file>", Does::Ask("e ")),
    (
        "documents opened last, to open one; a new document",
        ":recent",
        Does::Run("recent"),
    ),
    (
        "a new document, in a file of its own, begun from a starter",
        ":new <name>",
        Does::Ask("new "),
    ),
    (
        "begin this empty document as an article, report, letter, slides, notes",
        ":new",
        Does::Run("new"),
    ),
    (
        "next problem the compiler has",
        "]d",
        Does::Run("diagnostic next"),
    ),
    (
        "why the page is behind the text, and the ways out",
        ":why",
        Does::Run("why"),
    ),
    (
        "spelling: next and previous thing found",
        "]s  [s",
        Does::Run("issue next"),
    ),
    (
        "spelling: what may be meant, to put in its place",
        "z=",
        Does::Run("suggest"),
    ),
    (
        "spelling: this is a word (and: it is none after all)",
        "zg  zw",
        Does::Run("word good"),
    ),
    (
        "spelling: all that was found",
        ":issues",
        Does::Run("issues"),
    ),
    ("spelling: check or do not", ":spell", Does::Run("spell")),
    (
        "spelling: in another language than the document's",
        ":spell nl",
        Does::Ask("spell "),
    ),
    (
        "spelling: what is checked, and what with",
        ":spell ?",
        Does::Run("spell ?"),
    ),
    ("grammar: check or do not", ":grammar", Does::Run("grammar")),
    ("what is this under the cursor?", "K", Does::Run("describe")),
    (
        "show the source, the page, or both",
        "gs",
        Does::Run("view"),
    ),
    ("pages as slides", ":slides", Does::Run("slides")),
    ("all pages at a glance", ":grid", Does::Run("grid")),
    ("zoom in, out, to the width", "+  -  =", Does::Tells),
    (
        "pointer: put the cursor there; held and moved, select",
        "click  drag",
        Does::Tells,
    ),
    (
        "pointer: select the word, the paragraph (in the source: the line)",
        "double click  triple click",
        Does::Tells,
    ),
    (
        "pointer: select from the cursor or the selection up to there",
        "Shift-click",
        Does::Tells,
    ),
    (
        "pointer: take an entry of a list",
        "click on it",
        Does::Tells,
    ),
    (
        "pointer: scroll; zoom where the pointer is",
        "wheel  Ctrl-wheel",
        Does::Tells,
    ),
    ("interface larger", ":ui +", Does::Run("ui +")),
    ("interface smaller", ":ui -", Does::Run("ui -")),
    (
        "document: how it looks — font, paper, page numbers, …",
        "gD",
        Does::Run("doc"),
    ),
    (
        "document: styles — headings, links, captions, tables, theorems, …",
        ":styles",
        Does::Run("styles"),
    ),
    (
        "sources: the bibliography — list, add, cite",
        "gB",
        Does::Run("bib"),
    ),
    (
        "sources: paste from the clipboard (BibTeX, RIS, CSL-JSON)",
        ":bib paste",
        Does::Run("bib paste"),
    ),
    (
        "sources: import another bibliography file",
        ":bib import",
        Does::Run("bib import"),
    ),
    (
        "sources: sort, only those not cited, there twice",
        ":bib view",
        Does::Run("bib view"),
    ),
    (
        "sources: undo the last change to the bibliography file",
        ":bib undo",
        Does::Run("bib undo"),
    ),
    (
        "figures and tables: list — caption, label, width, rows, …",
        "gF",
        Does::Run("figures"),
    ),
    (
        "figure or table under the cursor: all that is said of it",
        ":figure",
        Does::Run("figure"),
    ),
    ("document: font", ":font", Does::Run("font")),
    (
        "document: font size",
        ":fontsize 11",
        Does::Ask("fontsize "),
    ),
    ("document: paper", ":paper", Does::Run("paper")),
    ("document: margins", ":margin 2.5cm", Does::Ask("margin ")),
    ("document: columns", ":columns 2", Does::Ask("columns ")),
    ("document: language", ":lang de", Does::Ask("lang ")),
    (
        "document: justified lines on or off",
        ":justify",
        Does::Run("justify"),
    ),
    (
        "document: space between lines",
        ":spacing 0.65",
        Does::Ask("spacing "),
    ),
    (
        "document: indent paragraphs on or off",
        ":indent",
        Does::Run("indent"),
    ),
    (
        "document: number the headings on or off",
        ":numbering",
        Does::Run("numbering"),
    ),
    (
        "document: page numbers on or off",
        ":pagenumbers",
        Does::Run("pagenumbers"),
    ),
    (
        "table: a row more",
        "Tab in the last cell",
        Does::Run("table row"),
    ),
    (
        "table: a column more",
        ":table column",
        Does::Run("table column"),
    ),
    (
        "table: take out this row",
        ":table delete row",
        Does::Run("table delete row"),
    ),
    (
        "table: take out this column",
        ":table delete column",
        Does::Run("table delete column"),
    ),
    (
        "agents: change the text or propose changes",
        ":agents",
        Does::Run("agents"),
    ),
    (
        "note: on what is selected, or the paragraph or figure here",
        "gn  :note",
        Does::Run("note"),
    ),
    (
        "ask an agent about what is selected, or the paragraph here",
        "gq  :ask",
        Does::Run("ask"),
    ),
    (
        "notes: all of them — go to, answer, resolve, delete",
        ":notes",
        Does::Run("notes"),
    ),
    (
        "notes: next and previous",
        "]n  [n",
        Does::Run("notes next"),
    ),
    (
        "proposal: next, accept, reject",
        "]p  ga  gr",
        Does::Run("proposal next"),
    ),
    (
        "proposal: the page as it would be, or as it is",
        "zp",
        Does::Run("proposal flip"),
    ),
    (
        "lines say how far they are from the cursor (for 5j): on or off",
        ":numbers",
        Does::Run("numbers"),
    ),
    (
        "equation: show it as it is written, or as what it means",
        "gx",
        Does::Run("written"),
    ),
    (
        "frame what typing goes into; say what empty places are for: on or off",
        ":hints",
        Does::Run("hints"),
    ),
    (
        "what shows as nothing (labels, comments, breaks): next, previous",
        "]i  [i",
        Does::Run("invisible next"),
    ),
    (
        "what shows as nothing, gone to: take it out, type it anew",
        "x  Enter",
        Does::Tells,
    ),
    (
        "what shows as nothing: all of it, to go to",
        ":invisibles",
        Does::Run("invisibles"),
    ),
    (
        "tags beside the page for what shows as nothing: on or off",
        ":tags",
        Does::Run("tags"),
    ),
    (
        "the compiler's complaints beside the page: on or off",
        ":errors",
        Does::Run("errors"),
    ),
    ("what typing costs", ":perf", Does::Run("perf")),
    (
        "something is wrong: a report of what just happened, kept here",
        ":report [a few words]  g!  Ctrl-G",
        Does::Ask("report "),
    ),
    (
        "the reports written, to delete them",
        ":reports",
        Does::Run("reports"),
    ),
    (
        "typeset what changes by itself, at once: on or off",
        ":parts",
        Does::Run("parts"),
    ),
    (
        "what can follow g, z, ], d, … shown under the page: on or off",
        ":nextkeys",
        Does::Run("nextkeys"),
    ),
    (
        "the rest of a word, faint behind the cursor (Tab takes it): on or off",
        ":ghost",
        Does::Run("ghost"),
    ),
];

/// The keys of each entry of the list of all there is to do, and the
/// command it runs or begins, if it does.
pub(super) fn entries() -> impl Iterator<Item = (&'static str, Option<&'static str>)> {
    ALL.iter().map(|(_, keys, does)| match does {
        Does::Run(command) | Does::Ask(command) => (*keys, Some(*command)),
        Does::Tells => (*keys, None),
    })
}

/// The papers there are to choose from.
const PAPERS: &[&str] = &[
    "a4",
    "a5",
    "a3",
    "us-letter",
    "us-legal",
    "presentation-16-9",
    "presentation-4-3",
];

/// A text as Typst writes a string.
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `wanted` as a length: a number in `unit`, or a number with a unit of
/// its own.
fn length(wanted: &str, unit: &str) -> Option<String> {
    let number = wanted.trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == '%');
    let own = &wanted[number.len()..];
    let fine = number.parse::<f64>().is_ok_and(|number| number >= 0.0);
    let known = matches!(own, "" | "pt" | "mm" | "cm" | "in" | "em" | "%");
    (fine && known).then(|| format!("{number}{}", if own.is_empty() { unit } else { own }))
}

impl Viewer {
    /// `g?`: all there is to do, to choose from.
    pub(super) fn open_palette(&mut self) {
        let items = ALL
            .iter()
            .map(|(what, keys, does)| {
                let target = match does {
                    Does::Run(command) => Target::Command((*command).to_owned()),
                    Does::Ask(command) => Target::Ask((*command).to_owned()),
                    Does::Tells => Target::Nothing,
                };
                // The keys apart: they are what is looked for, and a
                // long description is cut at its end.
                (format!("{what}\t{keys}"), target)
            })
            .collect();
        self.open_picker("do", items, 0);
    }

    /// Makes `key` of `function` be `value` for all of the file, by the
    /// rule that says so.
    fn set(&mut self, function: &str, key: &str, value: &str, told: &str) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let (range, with) = typst_text::set_rule(editor.source(), function, key, value);
        match editor.replace(&[(range, with.as_str())]) {
            Ok(()) => {
                self.text_changed(Edited::Command);
                self.notice(format!("{told}  (u takes it back)"));
            }
            Err(why) => self.alert(format!("not changed: {why}")),
        }
    }

    /// Turns `key` of `function` on or off: as `wanted` says (`on`,
    /// `off`), or to what it is not now.
    fn switch(
        &mut self,
        function: &str,
        key: &str,
        values: (&str, &str),
        wanted: &str,
        what: &str,
    ) {
        let Some(editor) = &self.editor else {
            return;
        };
        let now = typst_text::rule_value(editor.source(), function, key);
        let is_on = now.is_some_and(|now| now != values.1 && now != "false" && now != "none");
        let on = match wanted {
            "on" => true,
            "off" => false,
            _ => !is_on,
        };
        let value = if on { values.0 } else { values.1 };
        let told = format!("{what} {}", if on { "on" } else { "off" });
        self.set(function, key, value, &told);
    }

    /// `:count`: how much there is to read.
    fn count_words(&mut self) {
        let Some(editor) = &self.editor else {
            return;
        };
        let (words, characters) = typst_text::count(editor.source());
        let pages = self
            .document
            .as_ref()
            .map_or(0, |document| document.pages().len());
        self.notice(format!(
            "{words} words, {characters} characters, {pages} pages  \
             (of this file; equations and code not counted)"
        ));
    }

    /// Puts up `values` to choose from; the one chosen is what
    /// `command` is run with.
    fn choose(&mut self, title: &'static str, command: &str, values: Vec<String>) {
        let items = values
            .into_iter()
            .map(|value| {
                let run = Target::Command(format!("{command} {value}"));
                (value, run)
            })
            .collect();
        self.open_picker(title, items, 0);
    }

    /// The commands that say how the document looks, and those for
    /// tables. Returns whether `command` was one.
    pub(super) fn settings_command(&mut self, command: &str) -> bool {
        let (name, wanted) = match command.split_once(' ') {
            Some((name, wanted)) => (name, wanted.trim()),
            None => (command, ""),
        };
        if self.editor.is_none() {
            return false;
        }
        if self.panel_command(name, wanted) {
            return true;
        }
        match (name, wanted) {
            ("font", "") => {
                let world = self.project.snapshot();
                let families: Vec<String> = world
                    .book()
                    .families()
                    .map(|(family, _)| family.to_owned())
                    .collect();
                self.choose("fonts", "font", families);
            }
            ("font", family) => {
                self.set("text", "font", &quoted(family), &format!("font: {family}"));
            }
            ("paper", "") => {
                let papers = PAPERS.iter().map(|paper| (*paper).to_owned()).collect();
                self.choose("papers", "paper", papers);
            }
            ("paper", paper) => {
                self.set("page", "paper", &quoted(paper), &format!("paper: {paper}"));
            }
            ("fontsize", size) => match length(size, "pt") {
                Some(size) => self.set("text", "size", &size, &format!("font size: {size}")),
                None => self.alert("a size, please: :fontsize 11"),
            },
            ("margin", margin) => match length(margin, "cm") {
                Some(margin) => self.set("page", "margin", &margin, &format!("margins: {margin}")),
                None => self.alert("a width, please: :margin 2.5cm"),
            },
            ("spacing", leading) => match length(leading, "em") {
                Some(leading) => {
                    self.set(
                        "par",
                        "leading",
                        &leading,
                        &format!("between lines: {leading}"),
                    );
                }
                None => self.alert("how much, please: :spacing 0.65 (the usual), :spacing 1"),
            },
            ("columns", count) => match count.parse::<usize>() {
                Ok(count @ 1..=6) => {
                    self.set(
                        "page",
                        "columns",
                        &count.to_string(),
                        &format!("{count} columns"),
                    );
                }
                _ => self.alert("how many, please: :columns 2"),
            },
            ("lang", lang) if typst_text::is_language(lang) => {
                let lang = lang.to_ascii_lowercase();
                self.set("text", "lang", &quoted(&lang), &format!("language: {lang}"));
            }
            ("lang", _) => self.alert("a language, as its code: :lang de, :lang en, :lang nl"),
            ("justify" | "indent" | "pagenumbers", _) | ("numbering", "" | "on" | "off") => {
                let (function, key, values, what) = match name {
                    "justify" => ("par", "justify", ("true", "false"), "justified lines"),
                    "indent" => (
                        "par",
                        "first-line-indent",
                        ("1.5em", "0pt"),
                        "indented paragraphs",
                    ),
                    "numbering" => (
                        "heading",
                        "numbering",
                        ("\"1.1\"", "none"),
                        "numbered headings",
                    ),
                    _ => ("page", "numbering", ("\"1\"", "none"), "page numbers"),
                };
                self.switch(function, key, values, wanted, what);
            }
            ("numbering", pattern) => {
                let told = format!("headings numbered {pattern}");
                self.set("heading", "numbering", &quoted(pattern), &told);
            }
            _ => return false,
        }
        true
    }

    /// The commands that put up a list or act on something other than
    /// the rules of the document: the lists of all there is to do, of
    /// the document's settings and of its sources, tables, counting.
    /// Returns whether `name` was one of them.
    fn panel_command(&mut self, name: &str, wanted: &str) -> bool {
        match (name, wanted) {
            ("help", "") => self.open_palette(),
            ("parts", "" | "on" | "off") => {
                self.in_parts = match wanted {
                    "on" => true,
                    "off" => false,
                    _ => !self.in_parts,
                };
                self.notice(if self.in_parts {
                    "what changes is typeset by itself, at once; all of it when typing pauses"
                } else {
                    "what changes waits for all of the document to be typeset"
                });
            }
            ("count" | "words", "") => self.count_words(),
            ("written", "") => self.toggle_written(),
            ("hints", "" | "on" | "off") => self.hints_command(wanted),
            ("nextkeys" | "ghost", "" | "on" | "off") => self.following_command(name, wanted),
            ("numbers", "" | "on" | "off") => {
                self.numbers = match wanted {
                    "on" => true,
                    "off" => false,
                    _ => !self.numbers,
                };
                self.notice(if self.numbers {
                    "each line says how far it is from the cursor: the number for j and k"
                } else {
                    "the lines say nothing"
                });
            }
            ("doc" | "document", command) => self.doc_command(command),
            ("styles" | "style", command) => self.styles_command(command),
            ("bib" | "sources", command) => self.bib_command(command),
            ("figures", command) => self.figures_command(command),
            // Alone only: `:figure png` and the like write the figure
            // out as an image (see `export`).
            ("figure", "") => self.figures_command("here"),
            ("structure", command) => self.structure_command(command),
            ("section", what) => self.section_command(what),
            ("table", what) => self.table_command(what),
            ("tags" | "errors" | "invisible" | "invisibles", _) => {
                self.margin_command(name, wanted);
            }
            _ => return false,
        }
        true
    }

    /// `:table row`, `:table column`, `:table delete row`, `:table
    /// delete column`: of the table the cursor is in.
    fn table_command(&mut self, what: &str) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let (source, cursor) = (editor.source(), editor.cursor());
        // The cursor stands on a character of a cell: behind its start.
        let found = typst_text::table_at(source, cursor + 1).and_then(|table| {
            let cell = table
                .cell_at(cursor + 1)
                .or_else(|| table.cell_at(cursor))?;
            Some((table, cell))
        });
        let Some((table, cell)) = found else {
            self.alert("the cursor is in no table (or in one with more to it than cells)");
            return;
        };
        let (edits, told) = match what {
            "row" => match typst_text::table_new_row(source, &table) {
                Some((place, row, _)) => (vec![(place..place, row)], "a row more"),
                None => (Vec::new(), ""),
            },
            "column" | "col" => (
                typst_text::table_new_column(source, &table),
                "a column more",
            ),
            "delete row" => (
                typst_text::table_without_row(&table, cell),
                "the row is gone",
            ),
            "delete column" | "delete col" => (
                typst_text::table_without_column(&table, cell),
                "the column is gone",
            ),
            _ => {
                self.alert("row, column, delete row or delete column, please");
                return;
            }
        };
        if edits.is_empty() {
            self.alert("that cannot be done to this table: it is all it has");
            return;
        }
        let edits: Vec<(std::ops::Range<usize>, &str)> = edits
            .iter()
            .map(|(range, with)| (range.clone(), with.as_str()))
            .collect();
        match editor.replace(&edits) {
            Ok(()) => {
                self.text_changed(Edited::Command);
                self.notice(format!("{told}  (u takes it back)"));
            }
            Err(why) => self.alert(format!("not changed: {why}")),
        }
    }
}
