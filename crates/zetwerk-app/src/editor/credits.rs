//! Who an edit is by, where the editor can tell (see
//! [`zetwerk_core::Origin`]).
//!
//! Keys are the author's. What the editor does for the author after a
//! key (an equation tidied, a product spelled out) is the author's, done
//! by zetwerk. A change made without keys ([`Editor::replace_by`]) is by
//! whoever its caller says: an agent, an accepted proposal, a manager;
//! [`Editor::replace`] is zetwerk's for the author.
//!
//! Text put from a register keeps where it came from. When a key yanks
//! or deletes text, its credits are noted beside the text it took; when
//! the engine puts a register's text in (`p`, `P`, Ctrl-R while typing),
//! it says so ([`zetwerk_vim::Host::putting`]), and that text is credited
//! as what it was taken from. Text that came into a register from the
//! system clipboard, and that is not text yanked here, is pasted:
//! whatever its true source was. This costs nothing per key but for
//! yanks, deletions and puts.

use std::ops::Range;

use zetwerk_core::{By, Credit, Credited, Document, Given, Origin, Replace};

use super::{Editor, SourceBuffer};

/// How many texts taken into registers are remembered with their
/// credits.
const KEPT: usize = 16;

/// What the editor knows of who its edits are by.
#[derive(Debug, Clone, Default)]
pub(crate) struct Credits {
    /// Whom the edits are by while not the author's keys: set for the
    /// length of a change made without keys, or by whoever drives the
    /// keys (an agent sending keys).
    acting: Option<Origin>,
    /// Texts taken into registers (or come from the clipboard), the
    /// newest last, with their credits.
    taken: Vec<(String, Given)>,
    /// The register text the engine said it puts in next.
    putting: Option<String>,
    /// What this key's edits took out, where an edit took out one piece:
    /// where, what, and its credits.
    removed: Vec<(usize, String, Given)>,
    /// The credits of what the next edit puts in, as given.
    given: Option<Given>,
}

impl Credits {
    /// Makes the edits `replaces` (see [`Document::edit`]) as their
    /// makers: the author, whoever acts, or for text put from a
    /// register, whoever it was taken from.
    pub(crate) fn edit(
        &mut self,
        document: &mut Document<SourceBuffer>,
        replaces: &[Replace],
        join: bool,
    ) -> Result<(), zetwerk_core::DocumentError> {
        if let [one] = replaces
            && !one.range.is_empty()
            && let Some(old) = document.text().get(one.range.clone())
        {
            let credits = document.origins().carried(one.range.clone());
            self.removed
                .push((one.range.start, old.to_owned(), credits));
        }
        let origin = self.acting.clone().unwrap_or(Origin::Author);
        let mut given: Vec<Option<Given>> = Vec::new();
        if let Some(recredited) = self.given.take() {
            given.push(Some(recredited));
        }
        if let Some(put) = self.putting.clone() {
            let carried = self.look_up(&put);
            let mut any = false;
            // Whole lines go in as a paragraph of their own on a page,
            // without their last line break.
            let line = put.trim_end_matches('\n');
            for replace in replaces {
                let found = carried.as_ref().and_then(|carried| {
                    occurrences(&replace.text, &put, carried)
                        .or_else(|| occurrences(&replace.text, line, &cut(carried, 0..line.len())))
                });
                any |= found.is_some();
                given.push(found);
            }
            if any {
                self.putting = None;
            }
        }
        if given.iter().all(Option::is_none) {
            return document
                .edit_by(replaces, join, &By::Origin(&origin))
                .map(drop);
        }
        let by = By::Given {
            origin: &origin,
            given: &given,
        };
        document.edit_by(replaces, join, &by).map(drop)
    }

    /// The engine puts the text `text` of a register in with its next
    /// edit.
    pub(crate) fn putting(&mut self, text: &str) {
        self.putting = Some(text.to_owned());
    }

    /// After a key: what it yanked or deleted is noted with its credits,
    /// as of `document`; what it was about to put and did not is
    /// forgotten.
    pub(crate) fn after_key(
        &mut self,
        took: Option<&zetwerk_vim::Took>,
        document: &Document<SourceBuffer>,
    ) {
        self.putting = None;
        let removed = std::mem::take(&mut self.removed);
        match took {
            Some(zetwerk_vim::Took::Yanked(range)) => {
                if let Some(text) = document.text().get(range.clone()) {
                    let credits = document.origins().carried(range.clone());
                    self.keep(text.to_owned(), credits);
                }
            }
            Some(zetwerk_vim::Took::Deleted { text, .. }) => {
                let mut pieces = removed;
                // Taken from the last to the first, each where the text
                // was then: in the order of the text, they are the text.
                pieces.sort_by_key(|(at, ..)| *at);
                let mut joined = String::new();
                let mut credits = Given::new();
                for (_, piece, given) in pieces {
                    joined.push_str(&piece);
                    credits.extend(given);
                }
                let line = text.strip_suffix('\n').unwrap_or(text);
                if joined == *text || joined == line {
                    if joined.len() < text.len() {
                        credits.push((1, None));
                    }
                    self.keep(text.clone(), credits);
                }
            }
            _ => {}
        }
    }

    /// Notes that `text` came into a register from outside the editor:
    /// pasted, unless it is text taken here.
    pub(crate) fn from_outside(&mut self, text: &str) {
        if text.is_empty() || self.taken.iter().any(|(known, _)| known == text) {
            return;
        }
        let pasted = Credited {
            by: Credit {
                origin: Origin::Pasted,
                when: now(),
            },
            over: None,
        };
        self.keep(text.to_owned(), vec![(text.len(), Some(pasted))]);
    }

    fn keep(&mut self, text: String, credits: Given) {
        if text.is_empty() {
            return;
        }
        self.taken.retain(|(known, _)| *known != text);
        self.taken.push((text, credits));
        if self.taken.len() > KEPT {
            self.taken.remove(0);
        }
    }

    /// The credits of `text`, a register's: as it was taken, or as part
    /// of a text taken.
    fn look_up(&self, text: &str) -> Option<Given> {
        for (known, credits) in self.taken.iter().rev() {
            if known == text {
                return Some(credits.clone());
            }
            if let Some(at) = known.find(text) {
                return Some(cut(credits, at..at + text.len()));
            }
            if let Some(found) = occurrences(text, known, credits) {
                return Some(found);
            }
        }
        None
    }

    /// Takes over what `other` (the editor of another file) knows of
    /// texts taken into registers: they are shared.
    pub(crate) fn share(&mut self, other: &Self) {
        self.taken.clone_from(&other.taken);
    }
}

/// The bytes from the first to the last where `a` and `b`, the credits
/// of one text, differ; `None` if they do not.
fn differing(a: &Given, b: &Given) -> Option<Range<usize>> {
    let ends = |given: &Given| -> Vec<usize> {
        given
            .iter()
            .scan(0, |end, (len, _)| {
                *end += len;
                Some(*end)
            })
            .collect()
    };
    let (a_ends, b_ends) = (ends(a), ends(b));
    let mut cuts: Vec<usize> = a_ends.iter().chain(&b_ends).copied().collect();
    cuts.sort_unstable();
    cuts.dedup();
    let at = |given: &Given, ends: &[usize], offset: usize| {
        let index = ends.partition_point(|end| *end <= offset);
        given.get(index).map(|(_, credited)| credited.clone())
    };
    let mut start = 0;
    let (mut first, mut last) = (None, None);
    for end in cuts {
        if at(a, &a_ends, start) != at(b, &b_ends, start) {
            first = first.or(Some(start));
            last = Some(end);
        }
        start = end;
    }
    Some(first?..last?)
}

/// `credits` of the bytes `range` of the text they are of.
fn cut(credits: &Given, range: Range<usize>) -> Given {
    let mut out = Given::new();
    let mut start = 0;
    for (len, credited) in credits {
        let end = start + len;
        let (from, to) = (start.max(range.start), end.min(range.end));
        if from < to {
            out.push((to - from, credited.clone()));
        }
        start = end;
    }
    out
}

/// The credits of `text`, wherever `part` (credited `credits`) is in it;
/// `None` if it is nowhere in it.
fn occurrences(text: &str, part: &str, credits: &Given) -> Option<Given> {
    if part.is_empty() || !text.contains(part) {
        return None;
    }
    let mut out = Given::new();
    let mut at = 0;
    for (start, _) in text.match_indices(part) {
        if start > at {
            out.push((start - at, None));
        }
        out.extend(credits.iter().cloned());
        at = start + part.len();
    }
    if at < text.len() {
        out.push((text.len() - at, None));
    }
    Some(out)
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Something the editor does for the author: the author's, by zetwerk.
pub(crate) fn by_zetwerk(what: &str) -> Origin {
    Origin::Zetwerk(what.to_owned())
}

impl Editor {
    /// Changes the text without keys (see [`Editor::replace`]), as made
    /// by `origin`: an agent, a proposal accepted, the author through a
    /// panel.
    pub fn replace_by(
        &mut self,
        edits: &[(Range<usize>, &str)],
        origin: &Origin,
    ) -> Result<(), String> {
        let before = self.credits.acting.replace(origin.clone());
        let done = self.replace_now(edits);
        self.credits.acting = before;
        done
    }

    /// Changes the text without keys, as zetwerk does for the author:
    /// each of `edits` replaces a range of bytes by a text, the ranges
    /// referring to the text as the edits before them left it. All of
    /// them are one undo step. Nothing is changed if one of them cannot
    /// be made.
    pub fn replace(&mut self, edits: &[(Range<usize>, &str)]) -> Result<(), String> {
        self.replace_by(edits, &by_zetwerk(""))
    }

    /// Credits the whole text as `given` says, stretch by stretch, as
    /// `origin` doing it: one undo step that changes no text, where the
    /// credits are not those it has already.
    pub fn recredit(&mut self, given: &Given, origin: &Origin) -> Result<(), String> {
        let text = self.text().to_owned();
        let now = self.document.origins().carried(0..text.len());
        let Some(range) = differing(&now, given) else {
            return Ok(());
        };
        let with = text
            .get(range.clone())
            .ok_or("the credits are not of this text")?;
        self.credits.given = Some(cut(given, range.clone()));
        let done = self.replace_by(&[(range, with)], origin);
        self.credits.given = None;
        done
    }

    /// Makes the keys that follow, until this is called again with
    /// `None`, keys of `origin`: of an agent that sends keys.
    pub fn set_acting(&mut self, origin: Option<Origin>) {
        self.credits.acting = origin;
    }

    /// Where every byte of the text came from; see
    /// [`zetwerk_core::Attribution`].
    #[must_use]
    pub fn origins(&self) -> &zetwerk_core::Attribution {
        self.document.origins()
    }
}
