//! The style manager: how headings, links, code, captions and the rest
//! look, in one list reached from the document manager.
//!
//! `:styles` (or the last row of `gD`) lists all there is to style, each
//! with what it is now. Enter on one puts up what it can be, with "as
//! Typst does it" first; Enter on one of those makes it so, and the page
//! behind the list shows it at once. A switch (small capitals, links
//! underlined, a theorem to write with `#theorem[…]`) is turned at once;
//! the first environment of a kind brings how that kind looks along, in
//! the same step. Where anything can be wanted (a size, a colour), it is
//! typed. Escape leads back.
//!
//! What is chosen is written into the document as `#show` rules (see
//! `style_text`), and read from there each time: nothing is kept here.

use typst::World;
use typst::syntax::Source;
use typst::text::FontFlags;

use super::assist::Target;
use super::{Edited, Viewer};
use crate::style_text::{self, STYLES, Typed};

/// What a style that Typst decides is listed as.
const USUAL: &str = "as Typst does it";

/// The row of the document manager that leads to the styles: what it is
/// listed as for `source`, and where it leads.
pub(super) fn manager_row(source: &Source) -> (String, Target) {
    let chosen = style_text::style_values(source)
        .iter()
        .filter(|value| value.is_some())
        .count();
    let now = match chosen {
        0 => USUAL.to_owned(),
        1 => "1 chosen".to_owned(),
        many => format!("{many} chosen"),
    };
    let label = format!("styles: headings, links, code, theorems, …\t{now}");
    // With a row: from here, Escape leads back here.
    (label, Target::Command("styles 0".to_owned()))
}

impl Viewer {
    /// The commands of the style manager, without their `styles`: none
    /// puts it up; a number, with that row chosen; `open`, what a row
    /// can be; `choose`, a row and the number of one of those; `set`, a
    /// row and what was typed for it; `usual`, a row that is to be as
    /// Typst does it.
    pub(super) fn styles_command(&mut self, command: &str) {
        let mut words = command.trim_start().splitn(3, ' ');
        let what = words.next().unwrap_or("");
        let row = words.next().and_then(|row| row.parse::<usize>().ok());
        let rest = words.next().unwrap_or("");
        match (what, row) {
            // Put up by itself (`:styles`), Escape takes it away: it
            // came from no list to lead back to.
            ("", _) => {
                self.open_styles(0);
                self.picker_by_itself();
            }
            ("open", Some(row)) => self.open_style(row),
            ("choose", Some(row)) => {
                let choices = self.style_choices(row);
                if let Some((label, value)) =
                    rest.parse::<usize>().ok().and_then(|at| choices.get(at))
                {
                    self.apply_style(row, Some(value), label);
                }
            }
            ("set", Some(row)) => self.type_style(row, rest),
            ("usual", Some(row)) => self.apply_style(row, None, USUAL),
            (number, None) if number.parse::<usize>().is_ok() => {
                self.open_styles(number.parse().unwrap_or(0));
            }
            _ => self.alert("styles: a row, or open, choose, set or usual and a row"),
        }
    }

    /// `:styles`: the style manager, with row `chosen` chosen. Lines of
    /// the styles written by hand that are none of them are told of.
    pub(super) fn open_styles(&mut self, chosen: usize) {
        let Some(editor) = &self.editor else {
            return;
        };
        let source = editor.source();
        let values = style_text::style_values(source);
        let mut items: Vec<(String, Target)> = STYLES
            .iter()
            .zip(&values)
            .enumerate()
            .map(|(row, (style, value))| {
                let now = match (value, style.typed) {
                    (None, Typed::Switch) => "off".to_owned(),
                    (None, _) => USUAL.to_owned(),
                    (Some(value), _) => style_text::listed(style, value),
                };
                let open = Target::Command(format!("styles open {row}"));
                (format!("{}\t{now}", style.name), open)
            })
            .collect();
        let unknown = style_text::not_understood(source).len();
        let broken = style_text::block(source).err();
        if let Some(why) = broken {
            items.push((format!("{why}: nothing is changed\t"), Target::Nothing));
            self.alert(format!(
                "{why}: the styles are not changed until it is back"
            ));
        } else if unknown > 0 {
            let lines = if unknown == 1 { "line" } else { "lines" };
            let told =
                format!("{unknown} {lines} among the styles not known here, left as written");
            items.push((format!("{told}\t"), Target::Nothing));
            self.notice(told);
        }
        let chosen = chosen.min(items.len() - 1);
        self.open_picker_for("styles", items, chosen, "doc".to_owned(), None);
    }

    /// What style `row` can be, as listed and as written: its own
    /// choices, and for a font the other fonts there are.
    fn style_choices(&self, row: usize) -> Vec<(String, String)> {
        let Some(style) = STYLES.get(row) else {
            return Vec::new();
        };
        let mut choices: Vec<(String, String)> = style
            .choices
            .iter()
            .map(|(label, value)| ((*label).to_owned(), (*value).to_owned()))
            .collect();
        if matches!(style.typed, Typed::Font | Typed::MathFont) {
            let world = self.project.snapshot();
            let book = world.book();
            for (family, mut faces) in book.families() {
                let listed = choices.iter().any(|(label, _)| label == family);
                // For equations, only fonts that are made for them.
                let fits = style.typed == Typed::Font
                    || faces.any(|face| {
                        book.info(face)
                            .is_some_and(|info| info.flags.contains(FontFlags::MATH))
                    });
                if !listed && fits {
                    choices.push((family.to_owned(), format!("\"{family}\"")));
                }
            }
        }
        choices
    }

    /// What style `row` can be, to choose from or to type; a switch is
    /// turned at once.
    fn open_style(&mut self, row: usize) {
        let (Some(style), Some(editor)) = (STYLES.get(row), &self.editor) else {
            return;
        };
        let written = style_text::style_value(editor.source(), row);
        if style.typed == Typed::Switch {
            let (value, told) = match written {
                Some(_) => (None, "off"),
                None => (Some(""), "on"),
            };
            return self.apply_style(row, value, told);
        }
        let mut items = vec![(
            USUAL.to_owned(),
            Target::Command(format!("styles usual {row}")),
        )];
        let mut chosen = 0;
        for (at, (label, value)) in self.style_choices(row).into_iter().enumerate() {
            if written.as_deref() == Some(value.as_str()) {
                chosen = items.len();
            }
            items.push((label, Target::Command(format!("styles choose {row} {at}"))));
        }
        let free = (style.typed != Typed::No).then(|| format!("styles set {row} "));
        self.open_picker_for(style.name, items, chosen, format!("styles {row}"), free);
    }

    /// Makes style `row` what was typed for it, if it can be that.
    fn type_style(&mut self, row: usize, typed: &str) {
        let Some(style) = STYLES.get(row) else {
            return;
        };
        if let Some(value) = style_text::typed_value(style.typed, typed) {
            self.apply_style(row, Some(&value), typed.trim());
        } else {
            self.open_styles(row);
            self.alert(format!("{} cannot be {typed}", style.name));
        }
    }

    /// Makes style `row` be `value` (as written in its rule; `None`: as
    /// Typst does it; `told` is what it is called), as one step to undo,
    /// and puts the list up again: there may be more to decide.
    fn apply_style(&mut self, row: usize, value: Option<&str>, told: &str) {
        let (Some(style), Some(editor)) = (STYLES.get(row), &mut self.editor) else {
            return;
        };
        let edits = match value {
            Some(value) => style_text::set_style(editor.source(), row, value),
            None => style_text::unset_style(editor.source(), row),
        };
        let edits = match edits {
            Ok(edits) if edits.is_empty() => {
                self.notice(format!("{}: {told} already", style.name));
                return self.open_styles(row);
            }
            Ok(edits) => edits,
            Err(why) => {
                self.alert(format!("not changed: {why}"));
                return self.open_styles(row);
            }
        };
        let edits: Vec<(std::ops::Range<usize>, &str)> = edits
            .iter()
            .map(|(range, with)| (range.clone(), with.as_str()))
            .collect();
        match editor.replace(&edits) {
            Ok(()) => {
                self.text_changed(Edited::Command);
                self.notice(format!("{}: {told}  (u takes it back)", style.name));
            }
            Err(why) => self.alert(format!("not changed: {why}")),
        }
        self.open_styles(row);
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{compile, editing, press};
    use super::*;

    #[test]
    fn the_style_manager_decides_how_things_look() {
        let text = "= Hello\n\nSome words and #link(\"https://typst.app\")[a link].\n";
        let (mut viewer, directory) = editing(text);
        let has =
            |viewer: &Viewer, rule: &str| viewer.text().is_some_and(|text| text.contains(rule));
        // From the document manager.
        press(&mut viewer, "gDstyles<CR>");
        assert!(viewer.is_picking());
        // What it can be is listed; the list stays up for more.
        press(&mut viewer, "heading 1: size<CR>18pt<CR>");
        let size = "#show heading.where(level: 1): set text(size: 18pt)";
        assert!(has(&viewer, size), "{:?}", viewer.text());
        assert!(viewer.is_picking());
        // Anything else is typed.
        press(&mut viewer, "heading 1: size<CR>19.5<CR>");
        assert!(
            has(&viewer, "set text(size: 19.5pt)"),
            "{:?}",
            viewer.text()
        );
        press(&mut viewer, "links: colour<CR>#336699<CR>");
        assert!(has(&viewer, "#show link: set text(fill: rgb(\"#336699\"))"));
        // A switch is turned at once, on and off again.
        press(&mut viewer, "links: underlined<CR>");
        assert!(has(&viewer, "#show link: underline"));
        press(&mut viewer, "links: underlined<CR>");
        assert!(!has(&viewer, "#show link: underline"));
        // What cannot be is not written.
        press(&mut viewer, "heading 2: colour<CR>sky<CR>");
        assert!(!has(&viewer, "level: 2"));
        // Back to how Typst does it.
        press(&mut viewer, "links: colour<CR>as Typst<CR>");
        assert!(!has(&viewer, "#show link"));
        // Escape leads back to the document manager, then away.
        press(&mut viewer, "<Esc>");
        assert!(viewer.is_picking());
        press(&mut viewer, "<Esc>");
        assert!(!viewer.is_picking());
        compile(&mut viewer);
        assert_eq!(viewer.document_revision, Some(viewer.revision));
        // Each change was one step: the colour comes back, then 18pt.
        press(&mut viewer, "u");
        assert!(
            has(&viewer, "fill: rgb(\"#336699\")"),
            "{:?}",
            viewer.text()
        );
        press(&mut viewer, "uuuu");
        assert!(has(&viewer, size), "{:?}", viewer.text());
        press(&mut viewer, "u");
        assert_eq!(viewer.text(), Some(text));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_theorem_comes_with_how_it_looks_in_one_step() {
        let text = "Words.\n";
        let (mut viewer, directory) = editing(text);
        press(&mut viewer, ":styles<CR>environments: theorem<CR>");
        assert!(viewer.is_picking());
        let now = viewer.text().unwrap_or_default();
        assert!(now.contains("#let theorem = figure.with(kind: \"theorem\""));
        assert!(now.contains("#show figure.where(kind: \"theorem\"): it =>"));
        // Another look, chosen from the list.
        press(&mut viewer, "how theorems<CR>in a box<CR>");
        let boxed = viewer.text().unwrap_or_default().to_owned();
        assert!(boxed.contains("stroke: 0.5pt"), "{boxed}");
        // Used in the document, it is typeset.
        press(&mut viewer, "<Esc><Esc>");
        let used = format!("{boxed}#theorem[All is well.]\n");
        let edits = [(boxed.len()..boxed.len(), "#theorem[All is well.]\n")];
        if let Some(editor) = &mut viewer.editor {
            editor.replace(&edits).unwrap_or_else(|why| panic!("{why}"));
        }
        viewer.text_changed(Edited::Command);
        compile(&mut viewer);
        assert_eq!(viewer.text(), Some(used.as_str()));
        // Typeset without errors: what is shown is of this revision.
        assert_eq!(viewer.document_revision, Some(viewer.revision));
        // Taken back: the theorem was used, the look, then both at once.
        press(&mut viewer, "uu");
        assert!(
            viewer
                .text()
                .is_some_and(|now| now.contains("#let theorem"))
        );
        press(&mut viewer, "u");
        assert_eq!(viewer.text(), Some(text));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn styles_written_by_hand_are_told_of_and_kept() {
        let text = format!(
            "{}\n#show heading: set text(size: 20pt, fill: red)\n{}\nWords.\n",
            style_text::BEGIN,
            style_text::END
        );
        let (mut viewer, directory) = editing(&text);
        press(&mut viewer, ":styles<CR>");
        assert!(viewer.is_picking());
        let told = viewer.message.as_ref().map(|message| message.text.clone());
        assert!(
            told.as_ref().is_some_and(|told| told.contains("1 line")),
            "{told:?}"
        );
        press(&mut viewer, "strong: colour<CR>teal<CR><Esc><Esc>");
        let now = viewer.text().unwrap_or_default();
        assert!(now.contains("#show heading: set text(size: 20pt, fill: red)"));
        assert!(now.contains("#show strong: set text(fill: teal)"));
        let _ = std::fs::remove_dir_all(directory);
    }
}
