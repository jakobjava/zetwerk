//! The structure panel: the headings of the document in one list, and
//! its sections rearranged from there.
//!
//! `:structure` (or `gO`) lists the headings, each as far in as it is
//! deep, the one the cursor is in chosen. Enter on one shows what can be
//! done with its section: go there, move it up or down past the section
//! next to it, make it deeper or shallower with all under it, take it
//! out. After each of these the list is back with that heading chosen,
//! and what was done last is what Enter offers first: Enter alone takes
//! a section as far up as it is to go.
//!
//! `:section up` (`down`, `deeper`, `shallower`) does the same to the
//! section the cursor is in, without the list. Agents move sections the
//! same way (see `zetwerk_mcp`).
//!
//! What a section is and which edits move it is worked out in
//! [`crate::sections`]; this is the lists and the commands. Each change
//! is one step for `u`.

use std::ops::Range;

use super::agent::Outcome;
use super::assist::Target;
use super::{Edited, Viewer};
use crate::sections::{self, Change, Section, Way};

/// What is said where a window only shows pages.
const NOT_EDITING: &str = "no file is being edited";
/// What is said of a section that is not there (any more).
const NO_SECTION: &str = "the document has no such section";

/// What a section's list offers to do with it, and what each is listed
/// as.
const WAYS: &[(Way, &str)] = &[
    (Way::Up, "move it up, before the section before it"),
    (Way::Down, "move it down, behind the section behind it"),
    (Way::Deeper, "make it deeper, with all under it"),
    (Way::Shallower, "make it shallower, with all under it"),
];

/// `edits` as [`crate::editor::Editor::replace`] takes them.
fn borrowed(edits: &[(Range<usize>, String)]) -> Vec<(Range<usize>, &str)> {
    edits
        .iter()
        .map(|(range, with)| (range.clone(), with.as_str()))
        .collect()
}

impl Viewer {
    /// The sections of the file being edited.
    fn sections(&self) -> Result<Vec<Section>, String> {
        let editor = self.editor.as_ref().ok_or(NOT_EDITING)?;
        Ok(sections::sections(editor.source()))
    }

    /// The section the cursor is in: its place among all.
    fn section_here(&self) -> Result<usize, String> {
        let editor = self.editor.as_ref().ok_or(NOT_EDITING)?;
        let all = sections::sections(editor.source());
        sections::at(&all, editor.cursor())
            .ok_or_else(|| "the cursor is in no section: no heading is before it".to_owned())
    }

    /// The commands of the structure panel, without their `structure`:
    /// none for the list, else what to do and the place of the section
    /// among all, as the lists themselves give it.
    pub(super) fn structure_command(&mut self, command: &str) {
        let mut words = command.split_whitespace();
        let what = words.next().unwrap_or("");
        let index = words.next().and_then(|index| index.parse::<usize>().ok());
        let last = words.next().and_then(Way::named);
        let done = match (what, index) {
            ("", None) => self.list_structure(None, None),
            ("list", Some(index)) => self.list_structure(Some(index), last),
            ("open", Some(index)) => self.open_section(index, last),
            ("remove", Some(index)) => {
                let back = format!("structure open {index}");
                let items = vec![
                    ("no, keep it".to_owned(), Target::Command(back.clone())),
                    (
                        "yes, take the section out, with all under it".to_owned(),
                        Target::Command(format!("structure remove! {index}")),
                    ),
                ];
                self.open_picker_for("take it out?", items, 0, back, None);
                Ok(())
            }
            ("remove!", Some(index)) => self.remove_section(index).and_then(|now| {
                // With nothing left to list, what was said stays said.
                if self.sections()?.is_empty() {
                    return Ok(());
                }
                self.list_structure(now, None)
            }),
            (way, Some(index)) => match Way::named(way)
                .map(|way| (way, self.change_section(index, way)))
            {
                Some((way, Ok(now))) => self.list_structure(Some(now), Some(way)),
                // The list is back all the same: for another try.
                Some((way, Err(why))) => self.list_structure(Some(index), Some(way)).and(Err(why)),
                None => Err(format!("not a command: structure {command}")),
            },
            _ => Err(format!("not a command: structure {command}")),
        };
        if let Err(why) = done {
            self.alert(why);
        }
    }

    /// `:section up`, `:section down`, `:section deeper`, `:section
    /// shallower`: of the section the cursor is in. Alone, what can be
    /// done with that section, to choose from.
    pub(super) fn section_command(&mut self, what: &str) {
        let done = match (what, Way::named(what)) {
            ("", _) => self
                .section_here()
                .and_then(|index| self.open_section(index, None)),
            (_, Some(way)) => self
                .section_here()
                .and_then(|index| self.change_section(index, way))
                .map(|_| ()),
            _ => Err("up, down, deeper or shallower, please: :section up".to_owned()),
        };
        if let Err(why) = done {
            self.alert(why);
        }
    }

    /// `:structure`: the headings, to choose one from. `chosen` comes up
    /// chosen, or the one the cursor is in; `last` is what was just done
    /// to a section, which the list of a section then offers first.
    fn list_structure(&mut self, chosen: Option<usize>, last: Option<Way>) -> Result<(), String> {
        let all = self.sections()?;
        if all.is_empty() {
            return Err(
                "the document has no headings yet: g+ makes the line the cursor is in one"
                    .to_owned(),
            );
        }
        let cursor = self
            .editor
            .as_ref()
            .map_or(0, crate::editor::Editor::cursor);
        let chosen = chosen.or_else(|| sections::at(&all, cursor)).unwrap_or(0);
        let again = last.map_or(String::new(), |way| format!(" {}", way.name()));
        let items = all
            .iter()
            .enumerate()
            .map(|(index, section)| {
                let indent = "  ".repeat(section.level.saturating_sub(1));
                let title = match section.title.as_str() {
                    "" => "(no title)".to_owned(),
                    // A tab would begin the list's second column.
                    title => title.replace('\t', " "),
                };
                let open = Target::Command(format!("structure open {index}{again}"));
                (format!("{indent}{title}"), open)
            })
            .collect();
        self.open_picker("structure", items, chosen.min(all.len() - 1));
        Ok(())
    }

    /// What can be done with section `index`, to choose from; `last`
    /// comes up chosen. Escape leads back to the list of all.
    fn open_section(&mut self, index: usize, last: Option<Way>) -> Result<(), String> {
        let all = self.sections()?;
        let section = all.get(index).ok_or(NO_SECTION)?;
        let mut items = vec![(
            format!("go there\t{}", section.title),
            Target::Offset(section.heading),
        )];
        for (way, listed) in WAYS {
            let run = Target::Command(format!("structure {} {index}", way.name()));
            items.push(((*listed).to_owned(), run));
        }
        items.push((
            "take it out of the document".to_owned(),
            Target::Command(format!("structure remove {index}")),
        ));
        let chosen = WAYS
            .iter()
            .position(|(way, _)| Some(*way) == last)
            .map_or(0, |way| way + 1);
        let back = format!("structure list {index}");
        self.open_picker_for("section", items, chosen, back, None);
        Ok(())
    }

    /// Takes section `index` the `way` asked for, as one step for `u`,
    /// and returns its place among all afterwards. The cursor stays
    /// where it is in the section, if it is in it; else it goes to the
    /// section's heading, so that the page shows what was moved.
    fn change_section(&mut self, index: usize, way: Way) -> Result<usize, String> {
        let editor = self.editor.as_mut().ok_or(NOT_EDITING)?;
        let all = sections::sections(editor.source());
        let section = all.get(index).ok_or(NO_SECTION)?;
        let change = sections::changed(editor.source(), index, way)?;
        let cursor = editor.cursor();
        // Not counting the blank lines behind it: they may stay behind.
        let written = editor.text()[section.span.clone()].trim_end().len();
        let within = section
            .span
            .contains(&cursor)
            .then(|| (cursor - section.span.start).min(written.saturating_sub(1)));
        editor
            .replace(&borrowed(&change.edits))
            .map_err(|why| format!("not changed: {why}"))?;
        match within {
            // Moved along with the section.
            Some(within) if matches!(way, Way::Up | Way::Down) => {
                editor.set_cursor(change.start + within);
            }
            // Where the edits left it: on what it was on.
            Some(_) => {}
            None => editor.jump(change.start + (section.heading - section.span.start)),
        }
        let now = sections::sections(editor.source());
        let index = sections::at(&now, change.start).ok_or(NO_SECTION)?;
        self.text_changed(Edited::Command);
        self.notice(format!("{}  (u takes it back)", way.told()));
        Ok(index)
    }

    /// Takes section `index` out of the text, with all under it, as one
    /// step for `u`. Returns the section that is where it was, if one is.
    fn remove_section(&mut self, index: usize) -> Result<Option<usize>, String> {
        let editor = self.editor.as_mut().ok_or(NOT_EDITING)?;
        let change = sections::removed(editor.source(), index)?;
        editor
            .replace(&borrowed(&change.edits))
            .map_err(|why| format!("not changed: {why}"))?;
        let now = sections::sections(editor.source());
        self.text_changed(Edited::Command);
        self.notice("the section is out of the document  (u takes it back)");
        Ok(sections::at(&now, change.start))
    }

    /// Takes the section whose heading is on `line` the `way` asked
    /// for, for an agent: made at once, or proposed where the user (or
    /// the agent) wants that.
    ///
    /// # Errors
    ///
    /// If the text is not at `expected_revision`, `line` is not the
    /// heading of a section, or the section cannot go that way.
    pub fn agent_move_section(
        &mut self,
        line: usize,
        way: Way,
        expected_revision: Option<u64>,
        propose: bool,
    ) -> Result<Outcome, String> {
        self.expect(expected_revision)?;
        let editor = self.editor.as_ref().ok_or(NOT_EDITING)?;
        let all = sections::sections(editor.source());
        let index = all.iter().position(|section| section.line == line);
        let index = index.ok_or_else(|| {
            format!(
                "line {line} is not the heading of a section: get_outline lists the headings \
                 with their lines (one inside a block is part of that block and does not move \
                 by itself)"
            )
        })?;
        let Change { edits, .. } = sections::changed(editor.source(), index, way)?;
        // In the order of the text, as places in the text as it is: the
        // edits come last first, and all put in the same.
        let ranges: Vec<Range<usize>> =
            edits.iter().rev().map(|(range, _)| range.clone()).collect();
        let new = edits.first().map_or("", |(_, with)| with.as_str());
        self.agent_change(&ranges, new, propose)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a failed step should fail the test")]
mod tests {
    use super::super::agent::AgentEdits;
    use super::super::tests::{compile, editing, press};
    use super::*;

    const PAPER: &str = "\
Before everything.

= Introduction <intro>
Why.

== Aim
What for.

== Scope
How far.

= Methods
How.

= Results
What.
";

    /// The headings of the viewer's text as written, one after the
    /// other.
    fn outline(viewer: &Viewer) -> String {
        let headings: Vec<String> = viewer
            .sections()
            .unwrap()
            .iter()
            .map(|section| format!("{} {}", "=".repeat(section.level), section.title))
            .collect();
        headings.join(" | ")
    }

    /// The heading of the section the cursor is in.
    fn here(viewer: &Viewer) -> String {
        let all = viewer.sections().unwrap();
        all[viewer.section_here().unwrap()].title.clone()
    }

    #[test]
    fn sections_are_rearranged_from_the_list_of_headings() {
        let (mut viewer, directory) = editing(PAPER);
        press(&mut viewer, "/How far<CR>");
        // The section the cursor is in comes up chosen: it goes up, and
        // Enter alone does to it again what was done last.
        press(&mut viewer, "gO");
        assert!(viewer.is_picking());
        press(&mut viewer, "<CR>up<CR>");
        assert!(viewer.is_picking());
        assert_eq!(
            outline(&viewer),
            "= Introduction | == Scope | == Aim | = Methods | = Results"
        );
        assert_eq!(here(&viewer), "Scope");
        // Further up it does not go; the list stays for something else.
        press(&mut viewer, "<CR><CR>");
        assert!(viewer.is_picking());
        assert_eq!(
            outline(&viewer),
            "= Introduction | == Scope | == Aim | = Methods | = Results"
        );
        assert!(
            viewer
                .message
                .as_ref()
                .is_some_and(|message| message.text.contains("first of its level"))
        );
        // Another section is found by typing, and moved twice.
        press(&mut viewer, "Intro<CR>down<CR><CR><CR>");
        assert_eq!(
            outline(&viewer),
            "= Methods | = Results | = Introduction | == Scope | == Aim"
        );
        // The cursor went along, in the subsection it was in.
        assert_eq!(here(&viewer), "Scope");
        // Escape from what can be done leads back to the list, and
        // Escape there to the text.
        press(&mut viewer, "<CR><Esc>");
        assert!(viewer.is_picking());
        press(&mut viewer, "<Esc>");
        assert!(!viewer.is_picking());
        // Each move was a step of its own for `u`.
        press(&mut viewer, "uuu");
        assert_eq!(viewer.text(), Some(PAPER));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn levels_change_and_sections_go_from_the_list() {
        let (mut viewer, directory) = editing(PAPER);
        press(&mut viewer, ":structure<CR>Methods<CR>deeper<CR>");
        assert_eq!(
            outline(&viewer),
            "= Introduction | == Aim | == Scope | == Methods | = Results"
        );
        // The cursor was elsewhere: it is on the heading now.
        assert_eq!(here(&viewer), "Methods");
        press(&mut viewer, "<CR><CR><Esc>");
        assert_eq!(
            outline(&viewer),
            "= Introduction | == Aim | == Scope | === Methods | = Results"
        );
        press(&mut viewer, "gOIntro<CR>shallower<CR>");
        assert!(
            viewer
                .message
                .as_ref()
                .is_some_and(|message| message.text.contains("first level"))
        );
        // Taken out only after being asked; the label goes with it.
        press(&mut viewer, "<Esc>gOIntro<CR>take<CR>");
        assert!(viewer.text().is_some_and(|text| text.contains("<intro>")));
        press(&mut viewer, "<CR>");
        assert!(viewer.text().is_some_and(|text| text.contains("<intro>")));
        press(&mut viewer, "take<CR>yes<CR>");
        assert_eq!(
            viewer.text(),
            Some("Before everything.\n\n= Results\nWhat.\n")
        );
        assert!(viewer.is_picking());
        press(&mut viewer, "<Esc>u");
        assert!(viewer.text().is_some_and(|text| text.contains("<intro>")));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn the_section_the_cursor_is_in_is_moved_by_command() {
        let (mut viewer, directory) = editing(PAPER);
        press(&mut viewer, "/How<CR>n");
        let column = viewer.state().editing.unwrap().column;
        press(&mut viewer, ":section up<CR>");
        assert_eq!(
            outline(&viewer),
            "= Methods | = Introduction | == Aim | == Scope | = Results"
        );
        // The cursor went with it: it is on what it was on.
        let editing = viewer.state().editing.unwrap();
        assert_eq!((editing.line, editing.column), (4, column));
        // The pages are made anew, and are those of the text then.
        assert_eq!(viewer.take_edited(), Some(Edited::Command));
        compile(&mut viewer);
        assert_eq!(viewer.document_revision, Some(viewer.revision));
        press(&mut viewer, ":section down<CR>:section down<CR>");
        assert_eq!(
            outline(&viewer),
            "= Introduction | == Aim | == Scope | = Results | = Methods"
        );
        press(&mut viewer, ":section deeper<CR>");
        assert!(
            viewer
                .text()
                .is_some_and(|text| text.ends_with("== Methods\nHow.\n"))
        );
        press(&mut viewer, ":section shallower<CR>:section shallower<CR>");
        assert!(
            viewer
                .text()
                .is_some_and(|text| text.ends_with("\n= Methods\nHow.\n"))
        );
        // Before the first heading there is no section to move.
        press(&mut viewer, "gg:section up<CR>");
        assert!(
            viewer
                .message
                .as_ref()
                .is_some_and(|message| message.text.contains("no section"))
        );
        press(&mut viewer, ":section sideways<CR>");
        assert!(
            viewer
                .text()
                .is_some_and(|text| text.ends_with("\n= Methods\nHow.\n"))
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_document_without_headings_has_no_structure_to_show() {
        let (mut viewer, directory) = editing("Only words.\n");
        press(&mut viewer, "gO");
        assert!(!viewer.is_picking());
        assert!(
            viewer
                .message
                .as_ref()
                .is_some_and(|message| message.text.contains("no headings"))
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn agents_move_sections_by_the_line_of_their_heading() {
        let (mut viewer, directory) = editing(PAPER);
        let moved = viewer.agent_move_section(12, Way::Up, Some(0), false);
        assert!(
            matches!(moved, Ok(Outcome::Changed { revision: 1, .. })),
            "{moved:?}"
        );
        assert_eq!(
            outline(&viewer),
            "= Methods | = Introduction | == Aim | == Scope | = Results"
        );
        // Not by a line that is no heading, nor from an old revision,
        // nor where it cannot go.
        assert!(
            viewer
                .agent_move_section(4, Way::Down, None, false)
                .is_err()
        );
        assert!(
            viewer
                .agent_move_section(3, Way::Down, Some(0), false)
                .is_err()
        );
        assert!(viewer.agent_move_section(3, Way::Up, None, false).is_err());
        let deeper = viewer.agent_move_section(6, Way::Deeper, None, false);
        assert!(
            matches!(deeper, Ok(Outcome::Changed { lines: (6, 12), .. })),
            "{deeper:?}"
        );
        assert_eq!(
            outline(&viewer),
            "= Methods | == Introduction | === Aim | === Scope | = Results"
        );
        press(&mut viewer, "uu");
        assert_eq!(viewer.text(), Some(PAPER));
        // Where the user reviews changes, a move is proposed.
        viewer.agent_edits = AgentEdits::Propose;
        let proposed = viewer.agent_move_section(12, Way::Up, None, false);
        assert!(
            matches!(proposed, Ok(Outcome::Proposed { .. })),
            "{proposed:?}"
        );
        assert_eq!(viewer.text(), Some(PAPER));
        assert_eq!(viewer.proposals().len(), 1);
        let _ = std::fs::remove_dir_all(directory);
    }
}
