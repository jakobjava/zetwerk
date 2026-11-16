//! The record of a review, written out for people: `:review record`.
//!
//! It is written beside the document's main file as
//! `<document>.review.md`, and never over one that is there unless the
//! author says `:review record!`. Markdown, not Typst: it is read as it
//! is, in any editor, in a mail, in a repository's web view, by a
//! supervisor or a co-author who has no Typst; it can quote any text of
//! the document and of agents without that text being read as markup;
//! and a `.typ` file beside the document would look like part of it.
//!
//! It is honest rather than pretty: every suggestion of the files open
//! (in the order of the text), each with who made it and when, why, how
//! sure, the text before and what was offered and what was made of it,
//! the whole thread, and what became of it; the instructions that stand
//! and the reviews asked for. What is not in it is said.

use std::fmt::Write as _;
use std::path::PathBuf;

use super::super::agent::line_column;
use super::{Instruction, Said, State, Suggestion, Viewer, Who};

/// A time, in seconds since 1970, as a date and time of day in UTC.
pub(super) fn when(at: u64) -> String {
    let days = i64::try_from(at / 86_400).unwrap_or(0);
    let seconds = at % 86_400;
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        seconds / 3600,
        seconds % 3600 / 60
    )
}

/// `text` as a block quote, every line of it.
fn quoted(text: &str) -> String {
    if text.is_empty() {
        return "> (nothing)\n".to_owned();
    }
    text.lines().fold(String::new(), |mut out, line| {
        let _ = writeln!(out, "> {line}");
        out
    })
}

/// `text` in a line, as code: backticks enough that none in it ends it.
fn inline(text: &str) -> String {
    let one_line = text.replace('\n', " ⏎ ");
    let mut ticks = String::from("`");
    while one_line.contains(ticks.as_str()) {
        ticks.push('`');
    }
    let pad = if one_line.starts_with('`') || one_line.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{ticks}{pad}{one_line}{pad}{ticks}")
}

/// Who, as the record names them.
fn named(by: &Who) -> &str {
    match by {
        Who::Author => "the author",
        Who::Agent(name) => name,
    }
}

/// What became of a suggestion, in words for its heading.
fn outcome(suggestion: &Suggestion) -> String {
    match suggestion.state {
        State::Accepted if !suggestion.kind.changes() => "taken as read".to_owned(),
        State::Accepted => "accepted as offered".to_owned(),
        State::Rewritten => "accepted, rewritten by the author".to_owned(),
        State::Rejected => "rejected".to_owned(),
        State::Withdrawn => "withdrawn by the agent".to_owned(),
        State::Later => "put aside, undecided".to_owned(),
        State::Open if suggestion.waiting => "open, waiting for the agent".to_owned(),
        State::Open if suggestion.stale => "open, but its text has changed since".to_owned(),
        State::Open => "open".to_owned(),
    }
}

/// A step of a thread as a line of the record.
fn step(entry: &super::Entry) -> String {
    let who = named(&entry.by);
    let what = match &entry.said {
        Said::Made => "made the suggestion.".to_owned(),
        Said::Message(text) => format!("said: {}", inline(text)),
        Said::Version { text, reason } if reason.is_empty() => {
            format!("revised it to {}.", inline(text))
        }
        Said::Version { text, reason } => format!("revised it to {} — {reason}", inline(text)),
        Said::Alternatives(all) => {
            let all: Vec<String> = all.iter().map(|text| inline(text)).collect();
            format!("offered other wordings: {}.", all.join(", "))
        }
        Said::AskedAlternatives => "asked for other wordings.".to_owned(),
        Said::Accepted { before, after } if before.is_empty() && after.is_empty() => {
            "took it as read.".to_owned()
        }
        Said::Accepted { before, after } => {
            format!("accepted it: {} became {}.", inline(before), inline(after))
        }
        Said::Rewritten {
            before,
            offered,
            after,
        } => format!(
            "accepted it and rewrote it: {} became {} (offered: {}).",
            inline(before),
            inline(after),
            inline(offered)
        ),
        Said::Rejected(why) if why.is_empty() => "rejected it, without saying why.".to_owned(),
        Said::Rejected(why) => format!("rejected it: {}", inline(why)),
        Said::Later => "put it aside.".to_owned(),
        Said::Reopened => "opened it again (what was decided was taken back).".to_owned(),
        Said::Withdrawn => "withdrew it.".to_owned(),
        Said::Instruction(text) => format!("made it a standing instruction: {}", inline(text)),
    };
    format!("- {} — {who} {what}\n", when(entry.at))
}

/// An instruction as a line of the record.
fn instruction_line(instruction: &Instruction) -> String {
    let mut line = format!("{}. {}", instruction.id, instruction.text.trim());
    line.push_str(if instruction.everywhere {
        " — for all of the author's documents"
    } else {
        " — for this document"
    });
    if let Some((_, quote)) = &instruction.protect {
        let _ = write!(line, "; keeps agents off {}", inline(quote));
    }
    if let Some((old, new)) = &instruction.refused {
        let _ = write!(line, "; refuses {} → {} again", inline(old), inline(new));
    }
    if let Some(category) = &instruction.category {
        let _ = write!(line, "; refuses suggestions of category {category}");
    }
    if let Some(from) = instruction.from {
        let _ = write!(line, " (from suggestion {from})");
    }
    let _ = write!(line, ", since {}", when(instruction.at));
    line.push('\n');
    line
}

impl Viewer {
    /// The record of the review of the open files, as Markdown; `now` is
    /// when it is written.
    #[allow(
        clippy::too_many_lines,
        reason = "the record, part by part, in the order it is read"
    )]
    pub(super) fn review_record(&mut self, now: u64) -> String {
        self.review_ready();
        let name = self
            .main_path()
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| self.file.clone());
        let mut out = format!("# Review of {name}\n\n");
        let all = &self.review.all;
        let mut agents: Vec<&str> = all.iter().map(|s| s.by.as_str()).collect();
        agents.sort_unstable();
        agents.dedup();
        let count = |test: &dyn Fn(&Suggestion) -> bool| all.iter().filter(|s| test(s)).count();
        let _ = writeln!(
            out,
            "Written {} by zetwerk from the review it keeps beside the document. {} suggestions \
             by {}: {} accepted as offered, {} accepted and rewritten, {} rejected, {} withdrawn, \
             {} put aside, {} open.\n",
            when(now),
            all.len(),
            if agents.is_empty() {
                "no agent".to_owned()
            } else {
                agents.join(", ")
            },
            count(&|s| s.state == State::Accepted),
            count(&|s| s.state == State::Rewritten),
            count(&|s| s.state == State::Rejected),
            count(&|s| s.state == State::Withdrawn),
            count(&|s| s.state == State::Later),
            count(&|s| s.state == State::Open),
        );
        let files: Vec<String> = self
            .review
            .loaded
            .iter()
            .map(|file| self.file_name_of_id(*file))
            .collect();
        let _ = writeln!(
            out,
            "It covers the files opened in this session ({}); other files of the document have \
             their own record once opened. Times are UTC; the author is the one whose document it \
             is, agents are named as their programs named themselves.\n",
            files.join(", ")
        );
        out.push_str("## Standing instructions\n\n");
        if self.review.brief.is_empty() {
            out.push_str("None.\n\n");
        } else {
            for instruction in &self.review.brief {
                out.push_str(&instruction_line(instruction));
            }
            out.push('\n');
        }
        out.push_str("## Reviews asked for\n\n");
        if self.review.requests.is_empty() {
            out.push_str("None.\n\n");
        }
        for request in &self.review.requests {
            let made: Vec<String> = all
                .iter()
                .filter(|s| s.request == Some(request.id))
                .map(|s| s.id.to_string())
                .collect();
            let _ = writeln!(
                out,
                "- Request {}: {} of {}{}, asked {}{}. {}",
                request.id,
                request.what,
                if request.section.is_empty() {
                    "the text before the first heading".to_owned()
                } else {
                    format!("section “{}”", request.section)
                },
                if request.words.is_empty() {
                    String::new()
                } else {
                    format!(" (“{}”)", request.words)
                },
                when(request.at),
                if request.done { ", done" } else { "" },
                if made.is_empty() {
                    "Nothing came of it yet.".to_owned()
                } else {
                    format!("Suggestions: {}.", made.join(", "))
                },
            );
        }
        if !self.review.requests.is_empty() {
            out.push('\n');
        }
        out.push_str("## Suggestions, in the order of the text\n\n");
        let mut order: Vec<&Suggestion> = all.iter().collect();
        order.sort_by_key(|s| (self.file_name_of_id(s.file), s.range.start, s.id));
        if order.is_empty() {
            out.push_str("None.\n");
        }
        for suggestion in order {
            let text = self
                .editor
                .iter()
                .chain(&self.others)
                .find(|e| e.file() == suggestion.file)
                .map(crate::editor::Editor::text)
                .unwrap_or_default();
            let line = line_column(text, suggestion.range.start).0;
            let section = if suggestion.section.is_empty() {
                String::new()
            } else {
                format!(", section “{}”", suggestion.section)
            };
            let _ = writeln!(
                out,
                "### {}. {} — {}\n",
                suggestion.id,
                suggestion.category,
                outcome(suggestion)
            );
            let _ = writeln!(
                out,
                "*{}* in {} line {line}{section}; by {} (session {}) on {}; {} sure, {} importance.\n",
                suggestion.kind.name(),
                self.file_name_of_id(suggestion.file),
                suggestion.by,
                if suggestion.session.is_empty() {
                    "unknown"
                } else {
                    &suggestion.session
                },
                when(suggestion.made),
                suggestion.confidence.sure(),
                suggestion.importance.weight(),
            );
            if !suggestion.reason.is_empty() {
                let label = if suggestion.kind.changes() {
                    "Why"
                } else {
                    "It says"
                };
                let _ = writeln!(out, "{label}: {}\n", suggestion.reason.trim());
            }
            if suggestion.kind.changes() {
                out.push_str("Text before:\n\n");
                out.push_str(&quoted(&suggestion.quote));
                out.push_str("\nOffered:\n\n");
                out.push_str(&quoted(&suggestion.offered));
                for (index, alternative) in suggestion.alternatives.iter().enumerate() {
                    let _ = writeln!(out, "\nOther wording {}:\n", index + 2);
                    out.push_str(&quoted(alternative));
                }
                let made = suggestion
                    .thread
                    .iter()
                    .rev()
                    .find_map(|entry| match &entry.said {
                        Said::Accepted { after, .. } | Said::Rewritten { after, .. } => Some(after),
                        _ => None,
                    });
                if let (Some(after), true) = (
                    made,
                    matches!(suggestion.state, State::Accepted | State::Rewritten),
                ) {
                    out.push_str("\nText after:\n\n");
                    out.push_str(&quoted(after));
                }
            } else if !suggestion.quote.is_empty() {
                out.push_str("About:\n\n");
                out.push_str(&quoted(&suggestion.quote));
            }
            if !suggestion.flagged.is_empty() {
                let flagged: Vec<String> = suggestion.flagged.iter().map(u64::to_string).collect();
                let _ = writeln!(
                    out,
                    "\nIt went against standing instruction {} and was made all the same.",
                    flagged.join(", ")
                );
            }
            out.push_str("\nThread:\n\n");
            for entry in &suggestion.thread {
                out.push_str(&step(entry));
            }
            out.push('\n');
        }
        out
    }

    /// Where the record of the document is written: beside its main
    /// file.
    fn review_record_path(&self) -> Option<PathBuf> {
        let main = self
            .main_path()
            .or_else(|| self.editor.as_ref().map(|editor| editor.path().to_owned()))?;
        let stem = main.file_stem()?.to_string_lossy().into_owned();
        Some(main.with_file_name(format!("{stem}.review.md")))
    }

    /// `:review record`, `:review record!`: the record written beside
    /// the document; over one that is there only with `!`.
    pub(super) fn write_review_record(&mut self, overwrite: bool) {
        let Some(path) = self.review_record_path() else {
            return self.alert("the document has no file to write the record beside");
        };
        if path.exists() && !overwrite {
            return self.alert(format!(
                "{} is there already: :review record! writes over it",
                path.display()
            ));
        }
        let record = self.review_record(super::now());
        match zetwerk_core::write_atomically(&path, record.as_bytes()) {
            Ok(()) => self.notice(format!("the review is written to {}", path.display())),
            Err(err) => self.alert(format!("the record could not be written: {err}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_dates_in_utc() {
        assert_eq!(when(0), "1970-01-01 00:00 UTC");
        assert_eq!(when(1_700_000_000), "2023-11-14 22:13 UTC");
        assert_eq!(when(951_782_400), "2000-02-29 00:00 UTC");
    }

    #[test]
    fn text_is_quoted_whatever_it_holds() {
        assert_eq!(inline("a `b` c"), "``a `b` c``");
        assert_eq!(inline("x\ny"), "`x ⏎ y`");
        assert_eq!(quoted("one\ntwo"), "> one\n> two\n");
    }
}
