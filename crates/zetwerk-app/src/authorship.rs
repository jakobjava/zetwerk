//! Who wrote what, in numbers and in words: the shares of a text by
//! origin, the story of a sentence, a statement for an "AI use"
//! declaration, and the attribution as a file of its own.
//!
//! Where every byte came from is kept by the document (see
//! [`zetwerk_core::origin`]); here it is counted and told. Nothing here
//! touches the text: it is tested on strings.
//!
//! **What counts.** The characters of the text that is read: letters,
//! digits and signs of running text, of headings, captions, footnotes
//! and the like, and of raw text (code listings). Not markup (the `=` of
//! a heading, the `*` around strong text, a list's marker), not code
//! (`#set` rules, the names of functions and their arguments but for the
//! text given to them), not comments, labels or references, and no
//! blanks. A word is a run of letters and digits. An equation counts as
//! one word and one character, for whoever wrote most of its symbols; an
//! equation that both an agent and the author wrote parts of counts as
//! agent text the author edited. The shares are of the text as it is
//! now: what was written and deleted again is in the history, not here.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::ops::Range;

use typst::syntax::{LinkedNode, Source, SyntaxKind};
use zetwerk_core::{Attribution, Credit, Given, Kind, Mark, Origin};

use crate::sections;

/// The kinds in the order they are shown.
pub const KINDS: [Kind; 6] = [
    Kind::Own,
    Kind::Agent,
    Kind::Edited,
    Kind::Pasted,
    Kind::Outside,
    Kind::Unknown,
];

/// What a kind is called where shares are shown.
#[must_use]
pub fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Own => "yours",
        Kind::Agent => "agent, as written",
        Kind::Edited => "agent, then edited by you",
        Kind::Pasted => "pasted",
        Kind::Outside => "changed outside zetwerk",
        Kind::Unknown => "unknown",
    }
}

/// Characters and words of each kind.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shares {
    pub chars: BTreeMap<Kind, usize>,
    pub words: BTreeMap<Kind, usize>,
    /// Characters each agent wrote (as written or edited since), by its
    /// name.
    pub agents: BTreeMap<String, usize>,
}

impl Shares {
    #[must_use]
    pub fn total_chars(&self) -> usize {
        self.chars.values().sum()
    }

    #[must_use]
    pub fn total_words(&self) -> usize {
        self.words.values().sum()
    }

    /// The share of `kind` in characters, in percent.
    #[must_use]
    pub fn percent(&self, kind: Kind) -> f64 {
        percent(
            self.chars.get(&kind).copied().unwrap_or(0),
            self.total_chars(),
        )
    }

    /// The share of `kind` in words, in percent.
    #[must_use]
    pub fn word_percent(&self, kind: Kind) -> f64 {
        percent(
            self.words.get(&kind).copied().unwrap_or(0),
            self.total_words(),
        )
    }

    fn add(&mut self, other: &Self) {
        for (kind, count) in &other.chars {
            *self.chars.entry(*kind).or_default() += count;
        }
        for (kind, count) in &other.words {
            *self.words.entry(*kind).or_default() += count;
        }
        for (agent, count) in &other.agents {
            *self.agents.entry(agent.clone()).or_default() += count;
        }
    }
}

fn percent(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
}

/// The shares of the whole text, and of each section.
#[derive(Debug, Clone, Default)]
pub struct Counted {
    pub whole: Shares,
    /// Each section by its title, level and the line its heading is on;
    /// what stands before the first heading first, titled `None`.
    pub sections: Vec<(Option<String>, usize, usize, Shares)>,
}

/// The kind of a stretch whose bytes are of `kinds`: their own kind if
/// they are all of one; agent text the author edited where an agent and
/// the author both wrote some of it; else the kind of most of it.
fn mixed(kinds: &BTreeMap<Kind, usize>) -> Kind {
    let has = |kind| kinds.get(&kind).is_some_and(|count| *count > 0);
    if kinds.values().filter(|count| **count > 0).count() == 1 {
        return kinds
            .iter()
            .find(|(_, count)| **count > 0)
            .map_or(Kind::Unknown, |(kind, _)| *kind);
    }
    if (has(Kind::Agent) || has(Kind::Edited)) && (has(Kind::Own) || has(Kind::Edited)) {
        return Kind::Edited;
    }
    kinds
        .iter()
        .max_by_key(|(_, count)| **count)
        .map_or(Kind::Unknown, |(kind, _)| *kind)
}

/// Looks up marks of bytes in order, cheaply: the runs are gone through
/// once for a text gone through from its start.
struct Marks<'a> {
    origins: &'a Attribution,
    runs: Vec<(Range<usize>, Mark)>,
    at: usize,
}

impl<'a> Marks<'a> {
    fn new(origins: &'a Attribution) -> Self {
        Self {
            origins,
            runs: origins.map().iter().collect(),
            at: 0,
        }
    }

    fn mark(&mut self, offset: usize) -> Option<Mark> {
        if self
            .runs
            .get(self.at)
            .is_some_and(|(range, _)| range.start > offset)
        {
            self.at = self.runs.partition_point(|(range, _)| range.end <= offset);
        }
        while self
            .runs
            .get(self.at)
            .is_some_and(|(range, _)| range.end <= offset)
        {
            self.at += 1;
        }
        self.runs
            .get(self.at)
            .filter(|(range, _)| range.contains(&offset))
            .map(|(_, mark)| *mark)
    }

    fn kind(&mut self, offset: usize) -> Kind {
        self.mark(offset)
            .map_or(Kind::Unknown, |mark| self.origins.kind(mark))
    }

    /// The agent behind byte `offset`, if an agent wrote it or the text
    /// it took the place of.
    fn agent(&mut self, offset: usize) -> Option<String> {
        let mark = self.mark(offset)?;
        [Some(mark.by), mark.over]
            .into_iter()
            .flatten()
            .find_map(|id| agent_name(&self.origins.credit(id).origin))
    }
}

/// The name an agent is shown by: its client's name and version.
#[must_use]
pub fn agent_name(origin: &Origin) -> Option<String> {
    match origin {
        Origin::Agent(agent) | Origin::Accepted(agent) => Some(if agent.version.is_empty() {
            agent.name.clone()
        } else {
            format!("{} {}", agent.name, agent.version)
        }),
        _ => None,
    }
}

/// Counts the text of `source` as [`Shares`], per section too.
#[must_use]
pub fn count(source: &Source, origins: &Attribution) -> Counted {
    let mut marks = Marks::new(origins);
    // The counted things in the order of the text: where they begin, and
    // their characters and words by kind.
    let mut pieces: Vec<(usize, Shares)> = Vec::new();
    walk(
        &LinkedNode::new(source.root()),
        source.text(),
        &mut marks,
        &mut pieces,
    );
    let all = sections::sections(source);
    let top = all.iter().map(|section| section.level).min().unwrap_or(1);
    let mut counted = Counted::default();
    let mut tops: Vec<&sections::Section> = all.iter().filter(|s| s.level == top).collect();
    tops.sort_by_key(|section| section.span.start);
    let first = tops
        .first()
        .map_or(usize::MAX, |section| section.span.start);
    let mut before = Shares::default();
    let mut each: Vec<Shares> = vec![Shares::default(); tops.len()];
    for (at, shares) in &pieces {
        counted.whole.add(shares);
        if *at < first {
            before.add(shares);
        } else if let Some(index) = tops.iter().rposition(|section| section.span.start <= *at) {
            each[index].add(shares);
        }
    }
    if before.total_chars() > 0 {
        counted.sections.push((None, 0, 1, before));
    }
    for (section, shares) in tops.into_iter().zip(each) {
        counted.sections.push((
            Some(section.title.clone()),
            section.level,
            section.line,
            shares,
        ));
    }
    counted
}

/// Counts what `node` holds into `pieces`.
fn walk(node: &LinkedNode, text: &str, marks: &mut Marks, pieces: &mut Vec<(usize, Shares)>) {
    match node.kind() {
        SyntaxKind::Equation => {
            let range = node.range();
            let mut kinds = BTreeMap::new();
            let mut agent = None;
            for (at, c) in text.get(range.clone()).unwrap_or_default().char_indices() {
                if !c.is_whitespace() && c != '$' {
                    *kinds.entry(marks.kind(range.start + at)).or_insert(0) += 1;
                    agent = agent.or_else(|| marks.agent(range.start + at));
                }
            }
            if kinds.is_empty() {
                return;
            }
            let kind = mixed(&kinds);
            let mut shares = Shares::default();
            shares.chars.insert(kind, 1);
            shares.words.insert(kind, 1);
            if let Some(agent) = agent.filter(|_| matches!(kind, Kind::Agent | Kind::Edited)) {
                shares.agents.insert(agent, 1);
            }
            pieces.push((range.start, shares));
        }
        SyntaxKind::Text
        | SyntaxKind::Escape
        | SyntaxKind::Shorthand
        | SyntaxKind::SmartQuote
        | SyntaxKind::RawTrimmed => {
            let start = node.offset();
            let text = text.get(node.range()).unwrap_or_default();
            let mut shares = Shares::default();
            let mut word: BTreeMap<Kind, usize> = BTreeMap::new();
            let end_word = |word: &mut BTreeMap<Kind, usize>, shares: &mut Shares| {
                if !word.is_empty() {
                    *shares.words.entry(mixed(word)).or_default() += 1;
                    word.clear();
                }
            };
            // An escaped sign or a shorthand is one character.
            let one = !matches!(node.kind(), SyntaxKind::Text | SyntaxKind::RawTrimmed);
            for (at, c) in text.char_indices() {
                if c.is_whitespace() {
                    end_word(&mut word, &mut shares);
                    continue;
                }
                let offset = start + at;
                let kind = marks.kind(offset);
                if !one || at == 0 {
                    *shares.chars.entry(kind).or_default() += 1;
                    if matches!(kind, Kind::Agent | Kind::Edited)
                        && let Some(agent) = marks.agent(offset)
                    {
                        *shares.agents.entry(agent).or_default() += 1;
                    }
                }
                if c.is_alphanumeric() {
                    *word.entry(kind).or_default() += 1;
                } else {
                    end_word(&mut word, &mut shares);
                }
            }
            end_word(&mut word, &mut shares);
            if shares.total_chars() > 0 {
                pieces.push((start, shares));
            }
        }
        // Raw text holds its words in `Text` leaves; code, comments,
        // labels and references count for nothing.
        SyntaxKind::LineComment
        | SyntaxKind::BlockComment
        | SyntaxKind::Label
        | SyntaxKind::Ref
        | SyntaxKind::Str => {}
        _ => {
            for child in node.children() {
                walk(&child, text, marks, pieces);
            }
        }
    }
}

/// The sentence of `text` that byte `at` is in: from after the end of
/// the sentence before (a `.`, `!` or `?` and a blank, or a blank line)
/// to the end of its own.
#[must_use]
pub fn sentence_at(text: &str, at: usize) -> Range<usize> {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    let ends = |before: &str, after: &str| {
        (before.ends_with(['.', '!', '?']) && after.starts_with(char::is_whitespace))
            || after.starts_with("\n\n")
            || before.ends_with("\n\n")
    };
    let mut start = at;
    while start > 0 {
        let previous = text[..start]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index);
        if ends(&text[..start], &text[start..]) && start < at {
            break;
        }
        start = previous;
    }
    let mut end = at;
    while end < text.len() {
        let next = end + text[end..].chars().next().map_or(1, char::len_utf8);
        end = next;
        if ends(&text[..end], &text[end..]) {
            break;
        }
    }
    let body = &text[start..end];
    let lead = body.len() - body.trim_start().len();
    let tail = body.len() - body.trim_end().len();
    (start + lead).min(end)..(end - tail).max(start + lead)
}

/// The day of `when` (seconds since 1970) as year, month and day, in
/// Coordinated Universal Time.
#[must_use]
pub fn day_of(when: u64) -> (i64, u32, u32) {
    // Howard Hinnant's civil-from-days.
    let days = (when / 86_400) as i64 + 719_468;
    let era = days.div_euclid(146_097);
    let of_era = days.rem_euclid(146_097);
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * of_year + 2) / 153;
    let day = (of_year - (153 * month_part + 2) / 5 + 1) as u32;
    let month = if month_part < 10 {
        month_part + 3
    } else {
        month_part - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `when` as a day to read: `15 Nov`, with the year if it is not `now`'s.
#[must_use]
pub fn day(when: u64, now: u64) -> String {
    let (year, month, date) = day_of(when);
    let name = MONTHS[(month as usize).saturating_sub(1) % 12];
    if day_of(now).0 == year {
        format!("{date} {name}")
    } else {
        format!("{date} {name} {year}")
    }
}

/// `when` as a date and time in ISO 8601, in UTC.
#[must_use]
pub fn iso(when: u64) -> String {
    let (year, month, date) = day_of(when);
    let seconds = when % 86_400;
    format!(
        "{year:04}-{month:02}-{date:02}T{:02}:{:02}:{:02}Z",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

/// Who a credit is, in a sentence.
#[must_use]
pub fn who(credit: &Credit) -> String {
    match &credit.origin {
        Origin::Author => "you".to_owned(),
        Origin::Zetwerk(what) if what.is_empty() => "you (by zetwerk)".to_owned(),
        Origin::Zetwerk(what) => format!("you (zetwerk {what})"),
        Origin::Pasted => "pasting".to_owned(),
        Origin::Outside => "a change outside zetwerk".to_owned(),
        Origin::Unknown => "someone unknown".to_owned(),
        origin => agent_name(origin).unwrap_or_default(),
    }
}

/// The story of the text `range` of `text` in one sentence: who wrote
/// it, who changed it since, and when.
#[must_use]
pub fn story(text: &str, range: Range<usize>, origins: &Attribution, now: u64) -> String {
    let mut marks = Marks::new(origins);
    // Per kind: the earliest and the latest credit, and the words.
    let mut writers: BTreeMap<String, (u64, bool)> = BTreeMap::new();
    let mut changed_words = 0;
    let mut changed_when = 0;
    let mut added_words = 0;
    let mut own_when = u64::MAX;
    let mut other: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut unknown_when = None;
    let mut zetwerk = None;
    let mut in_word: Option<Kind> = None;
    let body = text.get(range.clone()).unwrap_or_default();
    for (at, c) in body.char_indices() {
        let offset = range.start + at;
        let Some(mark) = marks.mark(offset) else {
            continue;
        };
        let kind = origins.kind(mark);
        let by = origins.credit(mark.by);
        if c.is_alphanumeric() {
            if in_word != Some(kind) {
                match kind {
                    Kind::Edited => changed_words += 1,
                    Kind::Own => added_words += 1,
                    _ => {}
                }
            }
            in_word = Some(kind);
        } else {
            in_word = None;
        }
        if c.is_whitespace() {
            continue;
        }
        match kind {
            Kind::Agent => {
                let name = agent_name(&by.origin).unwrap_or_default();
                let accepted = matches!(by.origin, Origin::Accepted(_));
                let entry = writers.entry(name).or_insert((by.when, accepted));
                entry.0 = entry.0.min(by.when);
                entry.1 |= accepted;
            }
            Kind::Edited => {
                changed_when = changed_when.max(by.when);
                if let Some(over) = mark.over.map(|over| origins.credit(over)) {
                    let name = agent_name(&over.origin).unwrap_or_default();
                    let accepted = matches!(over.origin, Origin::Accepted(_));
                    let entry = writers.entry(name).or_insert((over.when, accepted));
                    entry.0 = entry.0.min(over.when);
                    entry.1 |= accepted;
                }
            }
            Kind::Own => {
                own_when = own_when.min(by.when);
                if let Origin::Zetwerk(what) = &by.origin {
                    zetwerk = Some(what.clone());
                }
            }
            Kind::Pasted => {
                let when = other.entry("pasted").or_insert(by.when);
                *when = (*when).min(by.when);
            }
            Kind::Outside => {
                let when = other.entry("outside").or_insert(by.when);
                *when = (*when).min(by.when);
            }
            Kind::Unknown => unknown_when = Some(by.when),
        }
    }
    let words = |count: usize| {
        if count == 1 {
            "1 word".to_owned()
        } else {
            format!("{count} words")
        }
    };
    let mut parts = Vec::new();
    for (name, (when, accepted)) in &writers {
        let accepted = if *accepted {
            ", accepted by you"
        } else {
            ", live"
        };
        parts.push(format!(
            "written by {name} on {}{accepted}",
            day(*when, now)
        ));
    }
    if writers.is_empty() && own_when != u64::MAX {
        let by = match &zetwerk {
            Some(what) if !what.is_empty() => format!(" (zetwerk {what} for you)"),
            Some(_) => " (zetwerk for you)".to_owned(),
            None => String::new(),
        };
        parts.push(format!("written by you{by} on {}", day(own_when, now)));
    }
    if changed_words > 0 {
        parts.push(format!(
            "you changed {} on {}",
            words(changed_words),
            day(changed_when, now)
        ));
    }
    if !writers.is_empty() && added_words > 0 {
        parts.push(format!("you added {}", words(added_words)));
    }
    if let Some(when) = other.get("pasted") {
        parts.push(format!(
            "pasted on {} (its true source is not known)",
            day(*when, now)
        ));
    }
    if let Some(when) = other.get("outside") {
        parts.push(format!(
            "changed outside zetwerk, noticed on {}",
            day(*when, now)
        ));
    }
    match unknown_when {
        Some(0) => parts.push("from before zetwerk kept where text came from".to_owned()),
        Some(when) => parts.push(format!(
            "in the file when zetwerk first opened it on {}",
            day(when, now)
        )),
        None => {}
    }
    if parts.is_empty() {
        return "nothing here is text that counts".to_owned();
    }
    let mut sentence = parts.join("; ");
    if let Some(first) = sentence.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    sentence
}

// ---- The attribution as a file --------------------------------------------

/// What an attribution file begins with.
const HEADER: &str = "# zetwerk authorship 1";

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// An origin as the fields of a line: its kind and, for an agent, its
/// name, version and session; for zetwerk, what it did.
fn origin_fields(origin: &Origin) -> Vec<String> {
    let agent = |kind: &str, agent: &zetwerk_core::Agent| {
        vec![
            kind.to_owned(),
            escape(&agent.name),
            escape(&agent.version),
            escape(&agent.session),
        ]
    };
    match origin {
        Origin::Unknown => vec!["unknown".to_owned()],
        Origin::Author => vec!["author".to_owned()],
        Origin::Zetwerk(what) => vec!["zetwerk".to_owned(), escape(what)],
        Origin::Pasted => vec!["pasted".to_owned()],
        Origin::Agent(who) => agent("agent", who),
        Origin::Accepted(who) => agent("accepted", who),
        Origin::Outside => vec!["outside".to_owned()],
    }
}

fn origin_of(fields: &[&str]) -> Option<Origin> {
    let agent = || {
        Some(zetwerk_core::Agent {
            name: unescape(fields.get(1)?),
            version: unescape(fields.get(2)?),
            session: unescape(fields.get(3)?),
        })
    };
    Some(match *fields.first()? {
        "unknown" => Origin::Unknown,
        "author" => Origin::Author,
        "zetwerk" => Origin::Zetwerk(unescape(fields.get(1).copied().unwrap_or_default())),
        "pasted" => Origin::Pasted,
        "agent" => Origin::Agent(agent()?),
        "accepted" => Origin::Accepted(agent()?),
        "outside" => Origin::Outside,
        _ => return None,
    })
}

/// The attribution of `text` as a file to keep beside it: the credits,
/// one line each, then the text in stretches of one credit, cut at line
/// ends, so that the file changes where the text changes and a line of
/// the text is a line (or a few) of the file.
///
/// ```text
/// # zetwerk authorship 1
/// credit	1	2026-11-15T10:31:05Z	agent	Claude Code	2.1	3f2a…
/// text	1	-	The cat sat on the mat.\n
/// ```
#[must_use]
pub fn export(name: &str, text: &str, origins: &Attribution) -> String {
    let mut out = format!(
        "{HEADER}\n# The attribution of {name}: who wrote each stretch of it, and when, as\n\
         # zetwerk recorded it. Read back by `:authors import`, which finds each\n\
         # stretch in the text again by what it says; what is not found is unknown.\n"
    );
    let mut used: Vec<u32> = Vec::new();
    let runs: Vec<(Range<usize>, Mark)> = origins.map().iter().collect();
    for (_, mark) in &runs {
        for id in [Some(mark.by), mark.over].into_iter().flatten() {
            if !used.contains(&id) {
                used.push(id);
            }
        }
    }
    used.sort_unstable();
    for id in &used {
        let credit = origins.credit(*id);
        let _ = writeln!(
            out,
            "credit\t{id}\t{}\t{}",
            iso(credit.when),
            origin_fields(&credit.origin).join("\t")
        );
    }
    for (range, mark) in runs {
        let Some(stretch) = text.get(range) else {
            continue;
        };
        let over = mark
            .over
            .map_or_else(|| "-".to_owned(), |over| over.to_string());
        for line in stretch.split_inclusive('\n') {
            let _ = writeln!(out, "text\t{}\t{over}\t{}", mark.by, escape(line));
        }
    }
    out
}

/// Reads an attribution file made by [`export`] for `text`, as it may
/// have changed since: each stretch is looked for in the text where the
/// one before it ended, or a little further on. The credits of the
/// whole text, stretch by stretch; what is not found is unknown. Also
/// how many bytes were found.
pub fn import(file: &str, text: &str) -> Result<(Given, usize), String> {
    let mut lines = file.lines();
    if lines.next() != Some(HEADER) {
        return Err("this is not an attribution file of zetwerk".to_owned());
    }
    let mut credits: BTreeMap<String, Credit> = BTreeMap::new();
    let unknown = zetwerk_core::Credited {
        by: Credit {
            origin: Origin::Unknown,
            when: 0,
        },
        over: None,
    };
    let mut given: Given = Vec::new();
    let mut at = 0;
    let mut found = 0;
    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.first().copied() {
            Some("credit") if fields.len() >= 4 => {
                let when = parse_iso(fields[2]).unwrap_or(0);
                let origin = origin_of(&fields[3..]).ok_or("an origin that is not known")?;
                credits.insert(fields[1].to_owned(), Credit { origin, when });
            }
            Some("text") if fields.len() >= 4 => {
                let stretch = unescape(&fields[3..].join("\t"));
                let by = credits
                    .get(fields[1])
                    .cloned()
                    .ok_or("a credit that is not given")?;
                let over = credits.get(fields[2]).cloned();
                // Where it is: right here, or (a stretch long enough not
                // to be found by chance) a little further on.
                let window = text.len().min(at + 4096 + stretch.len() * 4);
                let place = if text[at..].starts_with(&stretch) {
                    Some(at)
                } else if stretch.trim().len() >= 8 {
                    text.get(at..window)
                        .and_then(|rest| rest.find(&stretch))
                        .map(|offset| at + offset)
                } else {
                    None
                };
                let Some(place) = place else {
                    continue;
                };
                if place > at {
                    given.push((place - at, Some(unknown.clone())));
                }
                given.push((stretch.len(), Some(zetwerk_core::Credited { by, over })));
                found += stretch.len();
                at = place + stretch.len();
            }
            _ => {}
        }
    }
    if at < text.len() {
        given.push((text.len() - at, Some(unknown)));
    }
    Ok((given, found))
}

/// A time `iso` wrote, back as seconds since 1970.
fn parse_iso(text: &str) -> Option<u64> {
    let (date, time) = text.trim_end_matches('Z').split_once('T')?;
    let mut date = date.split('-').map(str::parse::<i64>);
    let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let mut time = time.split(':').map(str::parse::<u64>);
    let (hours, minutes, seconds) = (time.next()?.ok()?, time.next()?.ok()?, time.next()?.ok()?);
    // Howard Hinnant's days-from-civil.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year.rem_euclid(400);
    let month_part = if month > 2 { month - 3 } else { month + 9 };
    let of_year = (153 * month_part + 2) / 5 + day - 1;
    let of_era_days = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    let days = era * 146_097 + of_era_days - 719_468;
    u64::try_from(days)
        .ok()
        .map(|days| days * 86_400 + hours * 3600 + minutes * 60 + seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_are_read_and_written() {
        assert_eq!(day_of(0), (1970, 1, 1));
        let when = 1_763_200_000;
        assert_eq!(iso(when), "2025-11-15T09:46:40Z");
        assert_eq!(parse_iso(&iso(when)), Some(when));
        assert_eq!(day(when, when), "15 Nov");
        assert_eq!(day(when, when + 400 * 86_400), "15 Nov 2025");
    }

    #[test]
    fn a_sentence_is_found_around_a_place() {
        let text = "First one. Second one here! Third.\n\nNext paragraph";
        assert_eq!(&text[sentence_at(text, 14)], "Second one here!");
        assert_eq!(&text[sentence_at(text, 0)], "First one.");
        assert_eq!(&text[sentence_at(text, 40)], "Next paragraph");
    }

    #[test]
    fn escaped_text_comes_back() {
        let text = "a\tb\\n\nc";
        assert_eq!(unescape(&escape(text)), text);
    }
}
