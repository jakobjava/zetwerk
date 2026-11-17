//! LaTeX on request, and what is said of it (see [`crate::latex`]):
//!
//! - `:latex` makes the LaTeX of the equation under the cursor Typst; out
//!   of an equation, the mathematics of LaTeX in its paragraph (`$…$`,
//!   `\(…\)`, `\[…\]`, `$$…$$`) Typst's equations. One undo step.
//! - `:latex copy` (or `:copy latex`) puts the equation under the cursor
//!   as LaTeX into the clipboard and the unnamed register, for pasting
//!   into other programs.
//! - `:latex on` and `:latex off`: whether typing reads LaTeX.
//! - A LaTeX name typed that nobody knows stays as typed, and the status
//!   line says so when it is over.

use super::Viewer;
use super::assist::{Completion, Entry};
use crate::editor::{Editor, Mode};

/// Where the LaTeX name begins that the cursor is typing, before a key.
pub(super) fn pending(editor: &Editor) -> Option<usize> {
    if editor.mode() != Mode::Insert {
        return None;
    }
    crate::latex::typing::pending_at(editor.source(), editor.cursor())
        .map(|pending| pending.range.start)
}

/// The bytes of the paragraph of `text` that byte `at` is in.
fn paragraph(text: &str, at: usize) -> std::ops::Range<usize> {
    let at = at.min(text.len());
    let start = text[..at].rfind("\n\n").map_or(0, |found| found + 2);
    let end = text[at..]
        .find("\n\n")
        .map_or(text.len(), |found| at + found);
    start..end
}

/// How many LaTeX names are listed at most.
const LISTED: usize = 40;

impl Viewer {
    /// The list of LaTeX names that go on the one being typed, with what
    /// each becomes; Typst's own list is not for it. Whether a name is
    /// being typed.
    pub(super) fn latex_complete(&mut self) -> bool {
        let Some(editor) = &self.editor else {
            return false;
        };
        let Some(pending) = crate::latex::typing::pending_at(editor.source(), editor.cursor())
        else {
            return false;
        };
        let typed = pending.typed.as_str();
        if typed.is_empty() || !typed.chars().all(|c| c.is_ascii_alphabetic()) {
            return true;
        }
        // The letters begin behind the quotation mark and the backslash.
        let from = pending.range.start + 3;
        let mut names: Vec<(&str, String)> = crate::latex::SYMBOLS
            .iter()
            .filter(|(latex, _)| latex.starts_with(typed))
            .map(|(latex, typst)| {
                let shown =
                    crate::latex::symbol_char(latex).map_or_else(String::new, |c| format!("{c}  "));
                (*latex, format!("{shown}{typst}"))
            })
            .collect();
        names.extend(
            crate::latex::COMMANDS
                .iter()
                .filter(|(latex, _)| latex.starts_with(typed))
                .map(|(latex, shape)| (*latex, becomes(*shape))),
        );
        names.sort_by_key(|(latex, _)| (latex.len(), *latex));
        names.dedup_by_key(|(latex, _)| *latex);
        names.truncate(LISTED);
        let candidates: Vec<Entry> = names
            .into_iter()
            .map(|(latex, detail)| Entry {
                candidate: zetwerk_typst::Candidate {
                    label: format!("\\{latex}"),
                    kind: "LaTeX",
                    detail: Some(detail),
                    text: latex.to_owned(),
                    cursor: latex.len(),
                },
                from,
                insert: None,
            })
            .collect();
        if !candidates.is_empty() {
            self.completion = Some(Completion {
                candidates,
                chosen: 0,
            });
        }
        true
    }

    /// After a key: a name begun at `start` that is over and stayed text,
    /// as nobody knows it, is said to be so.
    pub(super) fn latex_told(&mut self, start: Option<usize>) {
        let Some(editor) = &self.editor else {
            return;
        };
        let Some(start) = start else {
            return;
        };
        let source = editor.source();
        if pending(editor) == Some(start) {
            return;
        }
        let Some(left) = crate::latex::typing::pending_from(source, start) else {
            return;
        };
        if crate::latex::typing::unknown(&left.typed) {
            self.notice(format!(
                "\\{} is no LaTeX name zetwerk knows: it stays as typed",
                left.typed
            ));
        }
    }

    /// `:latex …`; whether `command` is one.
    pub(super) fn latex_command(&mut self, command: &str) -> bool {
        match command.trim() {
            "latex" => self.latex_to_typst(),
            "latex copy" | "copy latex" => self.latex_copy(),
            "latex on" | "latex off" => {
                let on = command.trim() == "latex on";
                crate::latex::set_typing(on);
                self.notice(if on {
                    "LaTeX typed in an equation becomes Typst"
                } else {
                    "what is typed in an equation is Typst only"
                });
            }
            _ => return false,
        }
        true
    }

    /// `:latex`: the LaTeX under the cursor as Typst.
    fn latex_to_typst(&mut self) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let source = editor.source();
        let text = source.text();
        let cursor = editor.cursor();
        let equation = crate::typst_text::equation_at(source, cursor)
            .filter(|equation| equation.len() >= 2 && text[equation.clone()].ends_with('$'));
        let (range, converted) = if let Some(equation) = equation {
            let inside = equation.start + 1..equation.end - 1;
            let body = &text[inside.clone()];
            if !crate::latex::looks_like_latex(body) {
                self.notice("nothing of LaTeX in this equation");
                return;
            }
            let converted = crate::latex::to_typst(body.trim());
            let lead = &body[..body.len() - body.trim_start().len()];
            let trail = &body[body.trim_end().len()..];
            let mut text = converted.text.clone();
            if text.trim().is_empty() {
                text = "\"\"".to_owned();
            }
            let with = format!("{lead}{text}{trail}");
            (
                inside,
                crate::latex::Converted {
                    text: with,
                    ..converted
                },
            )
        } else {
            let range = paragraph(text, cursor);
            let Some(converted) = crate::latex::markup_to_typst(&text[range.clone()]) else {
                self.notice("no mathematics of LaTeX here");
                return;
            };
            (range, converted)
        };
        if let Err(why) = editor.replace(&[(range, converted.text.as_str())]) {
            self.alert(format!("not changed: {why}"));
            return;
        }
        self.text_changed(super::Edited::Command);
        if converted.kept.is_empty() {
            self.notice("made Typst");
        } else {
            self.notice(format!(
                "made Typst; kept as text: {}",
                converted.kept.join(", ")
            ));
        }
    }

    /// `:latex copy`: the equation under the cursor as LaTeX, in the
    /// clipboard.
    fn latex_copy(&mut self) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let source = editor.source();
        let text = source.text();
        let Some(equation) = crate::typst_text::equation_at(source, editor.cursor())
            .filter(|equation| equation.len() >= 2)
        else {
            self.notice("no equation here to copy as LaTeX");
            return;
        };
        let body = &text[equation.start + 1..equation.end - 1];
        let displayed =
            body.starts_with(char::is_whitespace) && body.ends_with(char::is_whitespace);
        let latex = crate::latex::to_latex(body.trim());
        let copied = if displayed {
            format!("\\[ {} \\]", latex.text)
        } else {
            format!("${}$", latex.text)
        };
        editor.set_register('"', &copied);
        editor.set_register('+', &copied);
        self.register_to_clipboard();
        if latex.kept.is_empty() {
            self.notice(format!("copied as LaTeX: {copied}"));
        } else {
            self.notice(format!(
                "copied as LaTeX, without {}: {copied}",
                latex.kept.join(", ")
            ));
        }
    }
}

/// What a LaTeX command with arguments becomes, said in a list.
fn becomes(shape: crate::latex::Shape) -> String {
    use crate::latex::Shape;
    match shape {
        Shape::Call(name, 1) => format!("{name}(…)"),
        Shape::Call(name, _) | Shape::Brace(name) => format!("{name}(…, …)"),
        Shape::Frac => "frac(…, …)".to_owned(),
        Shape::Sqrt => "sqrt(…), root(…, …)".to_owned(),
        Shape::Text => "\"…\"".to_owned(),
        Shape::Operator => "op(\"…\")".to_owned(),
        _ => "as Typst writes it".to_owned(),
    }
}
