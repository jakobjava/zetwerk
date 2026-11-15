//! An equation shown as it is written, where it stands.
//!
//! An equation is typed as what it means, and shown as that. But what
//! is typed can be something Typst has no meaning for — a bracket that
//! is not closed, a mark with nothing behind it that shows as nothing —
//! and then there was nothing on the page to see or to take hold of,
//! only a complaint in the status line and the way into the source.
//!
//! Now such an equation is shown as its characters, faintly and in
//! another colour, in the place where it stands: every character can be
//! gone to and changed like any text of the page, and the rest of the
//! document is typeset around it as ever. As soon as the characters are
//! an equation again, they are shown as one. `gx` shows the equation the
//! cursor is in that way for the asking, to see what is written there.
//!
//! Nothing of this is in the text: the characters are shown by what the
//! pages are made from being given marks around the equation that make
//! Typst show it as it is written (see `Project::snapshot_showing`).

use super::{Edited, Viewer};
use crate::typst_text;

/// An equation that is shown as it is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Written {
    /// The byte it begins at.
    start: usize,
    /// What Typst said against it; `None` if the user asked to see it
    /// so.
    why: Option<String>,
}

/// What is put before and behind an equation to show it as written:
/// in its line, or by itself as it would stand; in the colour of what
/// is no equation, or of what was asked for.
///
/// What is shown is an equation still, with the characters in it: so
/// one that stands by itself keeps its place, its number and its label
/// (what refers to it would have nothing to refer to otherwise, and the
/// document would not be typeset at all).
const REFUSED: (&str, &str) = ("$#text(fill: rgb(\"#b0503f\"))[`", "`]$");
const REFUSED_ALONE: (&str, &str) = ("$ #text(fill: rgb(\"#b0503f\"))[`", "`] $");
const ASKED: (&str, &str) = ("$#text(fill: luma(42%))[`", "`]$");
const ASKED_ALONE: (&str, &str) = ("$ #text(fill: luma(42%))[`", "`] $");

impl Viewer {
    /// Has an equation shown as it is written because Typst refuses the
    /// text at byte `at`, saying `why`. Returns whether that is news:
    /// whether it was not shown so already.
    ///
    /// It is the equation the cursor is in, if it is in one: what Typst
    /// complains of may be far from what is wrong (an equation that is
    /// not closed makes text of the equations behind it, and equations
    /// of the text between them). Else the one at `at`.
    ///
    /// This is tried once for each state of the text. Tried again, one
    /// refusal could lead to the next and back, without end.
    pub(super) fn show_as_written(&mut self, at: usize, why: &str) -> bool {
        if self.written_tried == Some(self.revision) {
            return false;
        }
        let Some(editor) = &self.editor else {
            return false;
        };
        let (source, cursor) = (editor.source(), editor.cursor());
        let own = typst_text::equation_at(source, cursor).filter(|own| own.start < cursor);
        let Some(equation) = own.or_else(|| typst_text::equation_at(source, at)) else {
            return false;
        };
        if self.written.as_ref().map(|written| written.start) == Some(equation.start) {
            return false;
        }
        self.written_tried = Some(self.revision);
        self.written = Some(Written {
            start: equation.start,
            why: Some(why.to_owned()),
        });
        self.alert(format!(
            "no equation yet, so it is shown as it is written: {why}"
        ));
        true
    }

    /// After the text changed: an equation Typst refused may be one
    /// now, and is tried again.
    pub(super) fn try_as_equation(&mut self) {
        if self
            .written
            .as_ref()
            .is_some_and(|written| written.why.is_some())
        {
            self.written = None;
        }
    }

    /// `gx`: shows the equation the cursor is in as it is written, or
    /// as what it means again.
    pub(super) fn toggle_written(&mut self) {
        let Some(editor) = &self.editor else {
            return;
        };
        if self.written.take().is_some() {
            self.notice("shown as what it means again");
        } else {
            let Some(equation) = typst_text::equation_at(editor.source(), editor.cursor()) else {
                self.notice("the cursor is in no equation: there is nothing more written here");
                return;
            };
            self.written = Some(Written {
                start: equation.start,
                why: None,
            });
            self.notice("shown as it is written; gx shows what it means again");
        }
        // The pages are to be made anew, of the same text.
        self.edited = Some(Edited::Command);
    }

    /// Changes `put` (what is put into the text to show it) so that the
    /// equation that is to be shown as written is: marks around it, and
    /// nothing in it.
    pub(super) fn put_as_written(&mut self, put: &mut Vec<(usize, &'static str)>) {
        let (Some(written), Some(editor)) = (&self.written, &self.editor) else {
            return;
        };
        let source = editor.source();
        let equation = typst_text::equation_as_seen(source, written.start);
        // It is gone, or begins elsewhere now; or the user asked and has
        // left it.
        let left = |equation: &std::ops::Range<usize>| {
            written.why.is_none() && !(equation.start..=equation.end).contains(&editor.cursor())
        };
        let Some(equation) = equation.filter(|equation| !left(equation)) else {
            self.written = None;
            return;
        };
        let Some(text) = source.text().get(equation.clone()) else {
            return;
        };
        // A mark of the kind that is put around it, in it: not this way.
        if text.contains('`') {
            return;
        }
        let alone = (text.starts_with("$ ") || text.starts_with("$\n"))
            && (text.ends_with(" $") || text.ends_with("\n$"));
        let (before, behind) = match (written.why.is_some(), alone) {
            (true, false) => REFUSED,
            (true, true) => REFUSED_ALONE,
            (false, false) => ASKED,
            (false, true) => ASKED_ALONE,
        };
        put.retain(|(at, _)| *at <= equation.start || *at >= equation.end);
        put.push((equation.start, before));
        put.push((equation.end, behind));
        put.sort_by_key(|(at, _)| *at);
    }
}
