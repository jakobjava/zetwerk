//! The brief: what the author has said once and for all to agents.
//!
//! An instruction is words for agents ("British spelling stays", "do not
//! touch the abstract") and, where the author made it of something,
//! what can be checked of it without understanding the words: text that
//! is not to be changed (what was selected, or what a suggestion was
//! about), a change that is not to be suggested again (the one rejected),
//! a category no suggestion is to be of. A suggestion that goes against
//! one of these is refused when it comes, unless the agent insists, and
//! then it is flagged; the words are for the agent to follow. The
//! program reads no meaning into them.
//!
//! `g!` makes an instruction of what the author just said (to the
//! suggestion under the cursor, or the one just rejected or answered),
//! or protects what is selected; `:always words` the same on the
//! command line, `:always! words` for all the author's documents.
//! `:brief` lists them, to change or drop.

use std::ops::Range;

use typst::syntax::FileId;

use super::super::assist::{Target, cut};
use super::{Asking, Instruction, Kind, Said, Viewer, Who, now};

impl Viewer {
    /// The instructions a change of `range` of `file` from `old` to `new`
    /// (a suggestion of `kind` and `category`) goes against.
    pub(super) fn contradicts(
        &self,
        file: FileId,
        range: &Range<usize>,
        kind: Kind,
        category: Option<&str>,
        old: &str,
        new: &str,
    ) -> Vec<u64> {
        let category = category.map(|category| category.trim().to_lowercase());
        self.review
            .brief
            .iter()
            .filter(|instruction| {
                let of_category = instruction.category.as_ref().is_some_and(|banned| {
                    category.as_deref() == Some(banned.trim().to_lowercase().as_str())
                });
                let protected = kind.changes()
                    && instruction.file == Some(file)
                    && instruction.protect.as_ref().is_some_and(|(kept, _)| {
                        range.start < kept.end && kept.start < range.end.max(range.start + 1)
                    });
                let again = kind.changes()
                    && instruction.refused.as_ref().is_some_and(|(was, would)| {
                        !was.is_empty()
                            && old.contains(was.as_str())
                            && new.contains(would.as_str())
                            || was.is_empty() && would == new
                    });
                of_category || protected || again
            })
            .map(|instruction| instruction.id)
            .collect()
    }

    /// Flags every suggestion that waits with the instructions it goes
    /// against, as the brief is now.
    pub(super) fn reflag_all(&mut self) {
        let flags: Vec<Vec<u64>> = self
            .review
            .all
            .iter()
            .map(|suggestion| {
                if suggestion.state.decided() {
                    return suggestion.flagged.clone();
                }
                self.contradicts(
                    suggestion.file,
                    &suggestion.range,
                    suggestion.kind,
                    Some(&suggestion.category),
                    &suggestion.quote,
                    &suggestion.new,
                )
            })
            .collect();
        for (suggestion, flagged) in self.review.all.iter_mut().zip(flags) {
            suggestion.flagged = flagged;
        }
    }

    /// `g!`: an instruction of what was said, or protecting what is
    /// selected; the command line opened for its words, with what the
    /// author said last in them.
    pub(super) fn begin_instruction(&mut self) {
        let Some(editor) = &self.editor else {
            return;
        };
        let cursor = editor.cursor();
        let selected = editor.selection().filter(|range| !range.is_empty());
        let here = self.suggestion_here().map(|at| self.review.all[at].id);
        let last = self
            .review
            .last_acted
            .as_ref()
            .filter(|(_, then, _)| *then == cursor)
            .map(|(id, _, said)| (*id, said.clone()));
        let (about, words) = match (selected.clone(), here, last) {
            (Some(_), ..) => (None, String::new()),
            (None, Some(id), _) => (Some(id), String::new()),
            (None, None, Some((id, said))) => (Some(id), said),
            (None, None, None) => (None, String::new()),
        };
        if let Some(range) = selected {
            let file = self.file_id();
            self.review.requesting = Some((file, range));
        } else {
            self.review.requesting = None;
        }
        self.begin_review_line(Asking::Always, about, &format!("always {words}"));
    }

    /// `:always words` (`:always!` for all documents): a standing
    /// instruction, about what `g!` took or the suggestion here.
    pub(super) fn add_instruction(&mut self, words: &str, everywhere: bool) {
        let words = words.trim();
        if words.is_empty() {
            return self.begin_instruction();
        }
        let asking = self.review.asking.take();
        let cursor = self
            .editor
            .as_ref()
            .map_or(0, crate::editor::Editor::cursor);
        let about = match asking {
            Some((Asking::Always, about, then)) if then == cursor => about,
            _ => self.suggestion_here().map(|at| self.review.all[at].id),
        };
        let protect = self.review.requesting.take().and_then(|(file, range)| {
            let quote = self.text()?.get(range.clone())?.to_owned();
            (file == self.file_id()).then_some((range, quote))
        });
        let refused = about
            .and_then(|id| self.suggestion_index(id))
            .map(|at| &self.review.all[at])
            .filter(|suggestion| suggestion.kind.changes())
            .map(|suggestion| (suggestion.quote.clone(), suggestion.offered.clone()));
        let everywhere = everywhere || (self.review.settings.everywhere && protect.is_none());
        self.review.last_instruction += 1;
        let id = self.review.last_instruction;
        let seq = self.review_told();
        let file = (!everywhere).then(|| self.file_id());
        self.review.brief.push(Instruction {
            id,
            text: words.to_owned(),
            everywhere,
            file,
            category: None,
            protect: if everywhere { None } else { protect },
            refused,
            from: about,
            at: now(),
            seq,
        });
        if let Some(at) = about.and_then(|id| self.suggestion_index(id)) {
            self.review_said(at, Who::Author, Said::Instruction(words.to_owned()));
        }
        self.review_brief_changed(everywhere);
        let scope = if everywhere {
            "for all your documents"
        } else {
            "for this document"
        };
        self.notice(format!(
            "instruction {id} stands {scope}; agents are given it. :brief lists them"
        ));
    }

    /// The brief changed: what goes against it is flagged anew, it is to
    /// be written, and agents are told.
    fn review_brief_changed(&mut self, everywhere: bool) {
        self.reflag_all();
        if everywhere {
            self.review.everywhere_unwritten = true;
        }
        self.review_changed();
    }

    /// `:brief` and what it leads to: the list, and each instruction's
    /// menu.
    pub(super) fn brief_command(&mut self, rest: &str) {
        self.review_ready();
        let (verb, more) = rest
            .split_once(' ')
            .map_or((rest, ""), |(verb, more)| (verb, more.trim()));
        let (number, words) = more
            .split_once(' ')
            .map_or((more, ""), |(number, words)| (number, words.trim()));
        let id = number.parse::<u64>().ok();
        let at = id.and_then(|id| self.review.brief.iter().position(|i| i.id == id));
        match (verb, at) {
            ("", _) => self.list_review_brief(),
            ("edit", Some(at)) if !words.is_empty() => {
                words.clone_into(&mut self.review.brief[at].text);
                self.review_instruction_changed(at);
                self.notice("changed; agents are given it so");
            }
            ("scope", Some(at)) => {
                let file = self.file_id();
                let instruction = &mut self.review.brief[at];
                instruction.everywhere = !instruction.everywhere;
                instruction.file = (!instruction.everywhere).then_some(file);
                if instruction.everywhere {
                    instruction.protect = None;
                }
                self.review.everywhere_unwritten = true;
                self.review_instruction_changed(at);
                self.review_brief_menu(self.review.brief[at].id);
            }
            ("unprotect", Some(at)) => {
                self.review.brief[at].protect = None;
                self.review_instruction_changed(at);
                self.review_brief_menu(self.review.brief[at].id);
            }
            ("unrefuse", Some(at)) => {
                self.review.brief[at].refused = None;
                self.review_instruction_changed(at);
                self.review_brief_menu(self.review.brief[at].id);
            }
            ("category", Some(at)) => {
                let category = words.trim().to_lowercase();
                self.review.brief[at].category = (!category.is_empty()).then_some(category);
                self.review_instruction_changed(at);
                self.review_brief_menu(self.review.brief[at].id);
            }
            ("delete", Some(at)) => {
                let gone = self.review.brief.remove(at);
                self.review_told();
                self.review_brief_changed(gone.everywhere);
                self.notice(format!("instruction {} is dropped", gone.id));
                self.list_review_brief();
            }
            ("open", Some(at)) => self.review_brief_menu(self.review.brief[at].id),
            _ => self.alert("which instruction? :brief lists them"),
        }
    }

    /// Instruction `at` was changed by the author.
    fn review_instruction_changed(&mut self, at: usize) {
        let seq = self.review_told();
        let instruction = &mut self.review.brief[at];
        instruction.seq = seq;
        let everywhere = instruction.everywhere;
        self.review_brief_changed(everywhere);
    }

    /// `:brief`: the instructions, to change or drop one, or to add one.
    fn list_review_brief(&mut self) {
        let mut items: Vec<(String, Target)> = self
            .review
            .brief
            .iter()
            .map(|instruction| {
                let scope = if instruction.everywhere {
                    "all documents"
                } else {
                    "this document"
                };
                (
                    format!(
                        "{}. {}\t{scope}",
                        instruction.id,
                        cut(&instruction.text, 60)
                    ),
                    Target::Command(format!("brief open {}", instruction.id)),
                )
            })
            .collect();
        items.push((
            "add an instruction for this document …".to_owned(),
            Target::Ask("always ".to_owned()),
        ));
        items.push((
            "add an instruction for all your documents …".to_owned(),
            Target::Ask("always! ".to_owned()),
        ));
        self.open_picker("brief", items, 0);
    }

    /// Instruction `id`, and what can be done with it.
    fn review_brief_menu(&mut self, id: u64) {
        let Some(instruction) = self.review.brief.iter().find(|i| i.id == id) else {
            return self.list_review_brief();
        };
        let mut items: Vec<(String, Target)> =
            vec![(format!("“{}”", cut(&instruction.text, 70)), Target::Nothing)];
        if let Some((_, quote)) = &instruction.protect {
            items.push((
                format!("keeps agents off: “{}” — let them", cut(quote, 40)),
                Target::Command(format!("brief unprotect {id}")),
            ));
        }
        if let Some((old, new)) = &instruction.refused {
            items.push((
                format!(
                    "refuses “{}” → “{}” again — let it come",
                    cut(old, 24),
                    cut(new, 24)
                ),
                Target::Command(format!("brief unrefuse {id}")),
            ));
        }
        items.push((
            match &instruction.category {
                Some(category) => format!("refuses suggestions of category {category} — change …"),
                None => "refuse a category of suggestions …".to_owned(),
            },
            Target::Ask(format!("brief category {id} ")),
        ));
        items.push((
            "change the words …".to_owned(),
            Target::Ask(format!("brief edit {id} {}", instruction.text)),
        ));
        items.push((
            if instruction.everywhere {
                "for this document only".to_owned()
            } else {
                "for all your documents".to_owned()
            },
            Target::Command(format!("brief scope {id}")),
        ));
        items.push((
            "drop it".to_owned(),
            Target::Command(format!("brief delete {id}")),
        ));
        self.open_picker_for("instruction", items, 1, "brief".to_owned(), None);
    }
}
