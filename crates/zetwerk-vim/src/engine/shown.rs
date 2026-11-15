//! Vim's keys where the host shows something else than the text.
//!
//! A host that shows the text as typeset pages has places for the cursor
//! that are not the characters of the text (see [`Host::stop`]): a letter
//! is one, so is a symbol written with several letters, so is all that a
//! reference shows; and a part of an equation that is yet to be typed is
//! one that holds no bytes at all. Markup shows as nothing and is none.
//! The commands here are Vim's, read for such places: what they count,
//! look for and change is what is shown, never what it is written with.
//!
//! Each of them says whether it did its work. Where it did not (the host
//! shows the text as it is, or does not show the place the cursor is at),
//! the command goes by the characters of the text, as Vim has it.

use std::ops::Range;

use super::{
    Awaiting, Column, Edit, Engine, Host, Mode, Moved, Operator, Seek, clamp, line_end_inclusive,
    lower, upper,
};
use crate::keys::{Key, KeyPress};
use crate::motion::{self, Kind};
use crate::object::Object;
use crate::text::Text;
use crate::took::Took;

/// How many places a command looks at in one line, or in one selection,
/// before it gives up: more than any line has, and few enough to be gone
/// through at a key press.
const MANY: usize = 4096;

/// Up to how many characters a place may be written with to be found by
/// any of them (see [`found_by`]).
const SHORT: usize = 3;

/// How many lines a whole-lines command looks on for the end of what
/// it would else cut in two: more than a figure or a table is written
/// over, as a rule.
const LOOK_AT: usize = 64;

/// Whether `f` and `t` find what is shown at bytes `stop` of `text` when
/// they look for `target`.
///
/// A character is found by itself, and a blank by the space key, however
/// the blank is written (a line break of the text is one). What is
/// written with more than it shows is found by the character it begins
/// with: a symbol by the first letter of its name, a reference by its
/// `@`. That is how it was typed, and what it shows may be on no
/// keyboard. Short ones are found by any of their characters: two
/// letters set as one, an escaped sign, a dash written with two hyphens.
fn found_by(text: &str, stop: &Range<usize>, target: char) -> bool {
    let Some(shown) = text.get(stop.clone()) else {
        return false;
    };
    if target == ' ' {
        return !shown.is_empty() && shown.chars().all(char::is_whitespace);
    }
    shown.starts_with(target) || (shown.chars().count() <= SHORT && shown.contains(target))
}

/// Whether `between`, the text between two places, parts paragraphs: it
/// has a line with nothing on it.
fn parts_paragraphs(between: &str) -> bool {
    let mut lines = between.split('\n');
    // What is left of the line of the first place, and what there is of
    // the line of the second, are not lines between them.
    lines.next();
    lines.next_back();
    lines.any(|line| line.trim().is_empty())
}

/// The bytes all of `stops` take up together, from the first of `stop`.
fn all_of(stop: &Range<usize>, stops: &[Range<usize>]) -> Range<usize> {
    stops.iter().fold(stop.clone(), |all, one| {
        all.start.min(one.start)..all.end.max(one.end)
    })
}

impl Engine {
    /// The place the host shows under the cursor, outside insert mode
    /// and where it shows places of its own at all.
    pub(super) fn shown_here(&self, host: &impl Host) -> Option<Range<usize>> {
        if !self.rows_first || self.mode == Mode::Insert {
            return None;
        }
        host.stop(self.cursor, Seek::At)
    }

    /// The places behind `from` in the line the host shows it in (before
    /// it, if not `forward`), the nearest first; `most` of them at most.
    pub(super) fn shown_in_line(
        host: &impl Host,
        from: &Range<usize>,
        forward: bool,
        most: usize,
    ) -> Vec<Range<usize>> {
        let seek = if forward { Seek::Next } else { Seek::Previous };
        let mut found: Vec<Range<usize>> = Vec::new();
        let mut at = from.start;
        while found.len() < most.min(MANY) {
            match host.stop(at, seek) {
                // A host that hands out a place twice would go on forever.
                Some(next) if next.start != at && !found.contains(&next) => {
                    at = next.start;
                    found.push(next);
                }
                _ => break,
            }
        }
        found
    }

    /// The places the host shows that begin in bytes `range` of the
    /// text, in its order. `None` if they are too many to go through.
    fn shown_within(host: &impl Host, range: &Range<usize>) -> Option<Vec<Range<usize>>> {
        let text = Text(host.text());
        let mut found = Vec::new();
        let mut at = range.start;
        while let Some(stop) = host.stop(at, Seek::After) {
            if stop.start < at || stop.start >= range.end {
                break;
            }
            if found.len() == MANY {
                return None;
            }
            at = text.next(stop.start).max(stop.start + 1);
            found.push(stop);
        }
        Some(found)
    }

    /// The place after `from` in the order of the text (before it, if
    /// not `forward`): where `l` and `h` lead. They stay in their
    /// paragraph as Vim's stay in their line, unless they `wrap`, as
    /// Space and Backspace do.
    ///
    /// The order of the text, and not the line the host shows: what
    /// stands above and below a fraction's bar is not in the line the
    /// fraction is in, and is passed through all the same.
    fn shown_beside(
        host: &impl Host,
        from: &Range<usize>,
        forward: bool,
        wraps: bool,
    ) -> Option<Range<usize>> {
        let text = Text(host.text());
        let (next, between) = if forward {
            let next = host.stop(text.next(from.start), Seek::After)?;
            if next.start <= from.start {
                return None;
            }
            let between = from.end.min(next.start)..next.start;
            (next, between)
        } else {
            if from.start == 0 {
                return None;
            }
            let next = host.stop(text.prev(from.start), Seek::Before)?;
            if next.start >= from.start {
                return None;
            }
            let between = next.end.min(from.start)..from.start;
            (next, between)
        };
        let parted = text.0.get(between).is_some_and(parts_paragraphs);
        (wraps || !parted).then_some(next)
    }

    /// Applies the pending `operator` to bytes `range` of the text: what
    /// is shown at the places a motion passed.
    fn operate_on_shown(&mut self, operator: Operator, range: Range<usize>, host: &mut impl Host) {
        self.reset_pending_keep_register();
        self.nothing_selected = range.is_empty();
        let object = Object {
            start: range.start,
            end: range.end,
            linewise: false,
        };
        self.operate(operator, object, 1, range.start, host);
    }

    /// `h`, `l`, Backspace and Space on what the host shows: from place
    /// to place, a count being that many places. Markup between them is
    /// passed over, never stopped in. With an operator they are `X` and
    /// `x`: the places before the cursor in its line, or the one under
    /// it and those behind it.
    pub(super) fn shown_sideways(&mut self, c: char, host: &mut impl Host) -> bool {
        let Some(here) = self.shown_here(host) else {
            return false;
        };
        let forward = matches!(c, 'l' | ' ');
        if let Some(operator) = self.pending.operator {
            let (count, _) = self.take_count();
            let range = if forward {
                all_of(&here, &Self::shown_in_line(host, &here, true, count - 1))
            } else {
                let before = Self::shown_in_line(host, &here, false, count);
                let end = here.start;
                let start = before.iter().map(|stop| stop.start).min().unwrap_or(end);
                start.min(end)..end
            };
            self.operate_on_shown(operator, range, host);
            return true;
        }
        let wraps = matches!(c, ' ' | '\u{8}');
        let (count, _) = self.take_count();
        let mut to = here.clone();
        for _ in 0..count {
            match Self::shown_beside(host, &to, forward, wraps) {
                Some(next) => to = next,
                None => break,
            }
        }
        if to.start == here.start {
            self.fail();
            return true;
        }
        self.reset_pending();
        self.move_to(Text(host.text()), to.start, Column::Reset);
        true
    }

    /// `f`, `F`, `t` and `T` (and, `again`, `;` and `,`) on what the
    /// host shows: to the `count`-th place in the cursor's line that
    /// `target` finds (see [`found_by`]), or with `till` to the place
    /// before it on the way. The line is the one the host shows, with
    /// all that the user sees in it and nothing else.
    pub(super) fn shown_find(
        &mut self,
        (target, forward, till): (char, bool, bool),
        again: bool,
        host: &mut impl Host,
    ) -> bool {
        let Some(here) = self.shown_here(host) else {
            return false;
        };
        let (count, _) = self.take_count();
        let line = Self::shown_in_line(host, &here, forward, MANY);
        let text = host.text();
        let finds = |stop: &Range<usize>| found_by(text, stop, target);
        // Repeating `t` from right before its target would not move; as
        // in Vim it looks past that target instead.
        let past = again && till && count == 1 && line.first().is_some_and(finds);
        let found = line
            .iter()
            .enumerate()
            .filter(|(_, stop)| finds(stop))
            .map(|(index, _)| index)
            .nth(count - 1 + usize::from(past));
        let Some(found) = found else {
            self.fail();
            return true;
        };
        let onto = match (till, found.checked_sub(1)) {
            (false, _) => line[found].clone(),
            (true, Some(before)) => line[before].clone(),
            (true, None) => here.clone(),
        };
        let Some(operator) = self.pending.operator else {
            self.reset_pending();
            self.move_to(Text(host.text()), onto.start, Column::Reset);
            return true;
        };
        // Forward, what is found is taken along; backward, what the
        // cursor is on stays.
        let range = if forward {
            here.start.min(onto.start)..here.end.max(onto.end)
        } else if till && found == 0 {
            here.start..here.start
        } else {
            onto.start.min(here.start)..here.start
        };
        self.operate_on_shown(operator, range, host);
        true
    }

    /// `r` on what the host shows: the key typed takes the place of what
    /// is under the cursor, a count being that many places of its line,
    /// or of each place in the selection.
    ///
    /// What shows as one thing goes as one: a symbol written with several
    /// letters is replaced whole. What the character is written as, and
    /// whether the place holds text that a character can take the place
    /// of at all, the host says (see [`Host::put_over`]): on a reference,
    /// say, nothing happens, and in a selection such places are left as
    /// they are. Enter parts the paragraph at the place.
    pub(super) fn shown_replace(&mut self, press: KeyPress, host: &mut impl Host) -> bool {
        let with = match press.key {
            Key::Char(c) if !press.ctrl => Some(c),
            Key::Enter => None,
            _ => return false,
        };
        let selected = if self.selecting() {
            let selection = self.selection(host.text()).filter(|_| self.rows_first);
            let shown = selection.filter(|range| host.stop(range.start, Seek::After).is_some());
            if shown.is_none() {
                return false;
            }
            shown
        } else {
            None
        };
        let stops = if let Some(range) = &selected {
            let stops = Self::shown_within(host, range).filter(|_| with.is_some());
            let text = Text(host.text());
            self.undo_cursor = Some(clamp(text, range.start, false));
            self.record_selection(text);
            self.record_count = None;
            self.mode = Mode::Normal;
            self.reset_pending();
            self.cursor = clamp(text, range.start, false);
            self.wanted_column = None;
            self.changed = true;
            let Some(stops) = stops else {
                self.fail();
                return true;
            };
            stops
        } else {
            let Some(here) = self.shown_here(host) else {
                return false;
            };
            let (count, _) = self.take_count();
            self.reset_pending();
            let mut stops = Self::shown_in_line(host, &here, true, count - 1);
            stops.insert(0, here);
            if stops.len() < count {
                // Not enough of them: nothing is replaced.
                self.fail();
                return true;
            }
            stops
        };
        let Some(c) = with else {
            self.shown_break(&stops, host);
            return true;
        };
        let mut puts = Vec::with_capacity(stops.len());
        for stop in stops {
            match host.put_over(stop, c) {
                Some(put) => puts.push(put),
                None if selected.is_some() => {}
                None => {
                    self.fail();
                    return true;
                }
            }
        }
        puts.sort_by_key(|(range, ..)| (range.start, range.end));
        let apart = puts.windows(2).all(|pair| pair[0].0.end <= pair[1].0.start);
        let in_text = puts.iter().all(|(range, ..)| {
            range.start <= range.end && host.text().get(range.clone()).is_some()
        });
        let edits: Vec<Edit> = puts
            .iter()
            .map(|(range, text, _)| Edit {
                range: range.clone(),
                text: text.clone(),
            })
            .collect();
        if !apart || !in_text || host.refuses(&edits) {
            self.fail();
            return true;
        }
        // Where the last of them comes to stand, with what the ones
        // before it add to the text or take from it.
        let mut landing = puts.last().map(|(range, _, within)| range.start + within);
        for (range, text, _) in puts.iter().rev().skip(1) {
            landing = landing.map(|at| (at + text.len()).saturating_sub(range.len()));
        }
        // From the last to the first: the ranges are those of the text
        // as it was.
        for (range, text, _) in puts.iter().rev() {
            self.edit(host, range.clone(), text);
        }
        if let (None, Some(landing)) = (&selected, landing) {
            self.cursor = landing;
        }
        self.cursor = clamp(Text(host.text()), self.cursor, false);
        self.wanted_column = None;
        true
    }

    /// `r` with Enter on what the host shows at `stops`: they go, and
    /// the paragraph is parted where they stood, as Enter typed there
    /// would part it. On a blank that is the opposite of `J`.
    fn shown_break(&mut self, stops: &[Range<usize>], host: &mut impl Host) {
        let Some(first) = stops.first() else {
            return self.fail();
        };
        // Only what a character could take the place of.
        if stops
            .iter()
            .any(|stop| host.put_over(stop.clone(), 'x').is_none())
        {
            return self.fail();
        }
        let gone = all_of(first, stops);
        let at = gone.start;
        self.edit(host, gone, "");
        let (range, with) = host
            .line_break(at)
            .filter(|(range, _)| range.start <= range.end && range.end <= host.text().len())
            .unwrap_or((at..at, "\n".to_owned()));
        self.edit(host, range.clone(), &with);
        self.cursor = clamp(Text(host.text()), range.start + with.len(), false);
        self.wanted_column = None;
    }

    /// Switches the case of the characters at `letters` (ranges of the
    /// text, apart from each other and in its order) the way `operator`
    /// does. Returns whether any of them changed, and where byte
    /// `landing` of the text is afterwards.
    fn switch_case(
        &mut self,
        operator: Operator,
        letters: &[Range<usize>],
        landing: usize,
        host: &mut impl Host,
    ) -> (bool, usize) {
        let mut changed = false;
        let mut landed = landing;
        for range in letters.iter().rev() {
            let Some(original) = host.text().get(range.clone()).map(str::to_owned) else {
                continue;
            };
            let converted: String = original
                .chars()
                .map(|c| match operator {
                    Operator::Lower => lower(c),
                    Operator::Upper => upper(c),
                    _ if c.is_lowercase() => upper(c),
                    _ => lower(c),
                })
                .collect();
            if converted == original {
                continue;
            }
            if range.end <= landing {
                landed = (landed + converted.len()).saturating_sub(original.len());
            }
            self.edit(host, range.clone(), &converted);
            changed = true;
        }
        (changed, landed)
    }

    /// `~` on what the host shows: switches the case of the letters
    /// shown under the cursor, a count being that many places of its
    /// line, and moves on behind them. Only of what is read as letters,
    /// which the host knows (see [`Host::letters`]): the name a symbol
    /// is written with stays the name it is.
    pub(super) fn shown_toggle_case(&mut self, host: &mut impl Host) -> bool {
        let Some(here) = self.shown_here(host) else {
            return false;
        };
        if host.letters(here.clone()).is_none() {
            return false;
        }
        let (count, _) = self.take_count();
        self.reset_pending();
        let mut stops = Self::shown_in_line(host, &here, true, count - 1);
        stops.insert(0, here);
        let last = stops.last().map_or(self.cursor, |stop| stop.start);
        // On to the place behind them, as far as the line goes.
        let landing = host.stop(last, Seek::Next).map_or(last, |next| next.start);
        let mut letters: Vec<Range<usize>> = stops
            .into_iter()
            .filter_map(|stop| host.letters(stop))
            .flatten()
            .collect();
        letters.sort_by_key(|range| (range.start, range.end));
        letters.dedup();
        let apart = letters.windows(2).all(|pair| pair[0].end <= pair[1].start);
        let landing = if apart {
            self.switch_case(Operator::ToggleCase, &letters, landing, host)
                .1
        } else {
            landing
        };
        // Also where it found no letter, it is what `.` repeats.
        self.changed = true;
        self.cursor = clamp(Text(host.text()), landing, false);
        self.wanted_column = None;
        true
    }

    /// `g~`, `gu`, `gU` and their like in a selection, where the host
    /// shows the text as something else: the case of the letters read in
    /// bytes `range` is switched, and nothing else of what is written
    /// there. Returns whether the host had a say.
    pub(super) fn shown_case(
        &mut self,
        operator: Operator,
        range: Range<usize>,
        host: &mut impl Host,
    ) -> bool {
        if !self.rows_first {
            return false;
        }
        let Some(mut letters) = host.letters(range) else {
            return false;
        };
        letters.sort_by_key(|range| (range.start, range.end));
        let apart = letters.windows(2).all(|pair| pair[0].end <= pair[1].start);
        if !(apart && self.switch_case(operator, &letters, 0, host).0) {
            // Even if no letter changes, this is an undo step.
            self.touch(host);
        }
        true
    }

    /// `J` and `gJ` where the lines are the host's to show: what the
    /// cursor is in (a paragraph, a heading, an item of a list) is
    /// joined with what follows it, a count being that many of them
    /// and a selection all it touches. Lines of a page are no lines of
    /// the text, and joining two of them would change nothing to see.
    ///
    /// Where the two meet and what stands between them then, the host
    /// says (see [`Host::join`]). The cursor goes to where they meet.
    pub(super) fn shown_join(&mut self, spaced: bool, host: &mut impl Host) -> bool {
        if !self.rows_first || host.join(self.cursor, spaced).is_none() {
            return false;
        }
        let (count, _) = self.take_count();
        let mut joins = count.max(2) - 1;
        let mut until = None;
        if self.selecting()
            && let Some(range) = self.selection(host.text())
        {
            let text = Text(host.text());
            self.record_selection(text);
            self.mode = Mode::Normal;
            self.cursor = clamp(text, range.start, false);
            until = Some(range.end);
            joins = usize::MAX;
        }
        self.reset_pending();
        // Even a join that finds nothing to join is what `.` repeats.
        self.changed = true;
        let mut done = 0;
        while done < joins {
            let Some((seam, with)) = host.join(self.cursor, spaced) else {
                break;
            };
            let nothing = seam.is_empty() && with.is_empty();
            let beyond = done > 0 && until.is_some_and(|until| seam.start >= until);
            if nothing || beyond || host.text().get(seam.clone()).is_none() {
                break;
            }
            if let Some(until) = &mut until {
                *until = (*until).max(seam.end) - seam.len() + with.len();
            }
            self.edit(host, seam.clone(), &with);
            self.cursor = seam.start;
            done += 1;
        }
        if done == 0 {
            self.fail();
            return true;
        }
        self.cursor = clamp(Text(host.text()), self.cursor, false);
        self.wanted_column = None;
        true
    }

    /// `w`, `b`, `e` and their WORD kin (behind an operator only `b`
    /// and `e`), where the
    /// host reads the text as something else than it is written (see
    /// [`Host::reading`]): over the words as read, so that the marks
    /// strong text and equations are written with are no words of their
    /// own. `None` where there is no reading, or the motion gets nowhere
    /// in it: then the text's own words count.
    pub(super) fn shown_word(&mut self, c: char, host: &impl Host) -> Option<Moved> {
        let reading = host.reading()?;
        let read = Text(reading.text());
        let at = reading.read_at(self.cursor);
        if at >= read.len() {
            return None;
        }
        let count = self.pending.count.unwrap_or(1).max(1)
            * self.pending.operator_count.unwrap_or(1).max(1);
        let big = c.is_ascii_uppercase();
        let (to, ok) = match c.to_ascii_lowercase() {
            'w' => motion::word_forward(read, at, count, big, false),
            'b' => motion::word_backward(read, at, count, big),
            _ => motion::word_end(read, at, count, big, false),
        };
        if !ok || to == at || to >= read.len() {
            return None;
        }
        let written = reading.written(to..read.next(to)).start;
        // Not into or out of an equation (the host's object `$`): what
        // one reads as runs its parts together (`frac(a, b)` is `ab`),
        // and the places the host shows in it lead through it.
        let in_equation = |at: usize| host.object('$', at, true).is_some();
        if written == self.cursor || in_equation(self.cursor) || in_equation(written) {
            return None;
        }
        self.take_count();
        let kind = if c.eq_ignore_ascii_case(&'e') {
            Kind::Inclusive
        } else {
            Kind::Exclusive
        };
        Some(Moved {
            to: written,
            kind,
            column: Column::Reset,
        })
    }

    /// Whole lines `start..end` of the text that a motion took for an
    /// operator (`dgg`, `dG`, `dj`), where the host shows pages: without
    /// the lines at either end that show nothing although something is
    /// written there (the rules that set up the page, a comment). Those
    /// are not where the motion went on the page; `dgg` took them along.
    /// Empty lines stay: they part paragraphs. All of it where no line
    /// shows anything.
    pub(super) fn shown_lines_within(
        &self,
        start: usize,
        end: usize,
        host: &impl Host,
    ) -> (usize, usize) {
        if !self.rows_first || start >= end {
            return (start, end);
        }
        let text = Text(host.text());
        let hidden = |line: usize| {
            let line_end = text.line_end(line);
            line < line_end
                && host
                    .stop(line, Seek::After)
                    .is_none_or(|stop| stop.start >= line_end)
        };
        let (mut from, mut to) = (start, end);
        while from < to && hidden(from) {
            from = text.next_line(from).unwrap_or(to).min(to);
        }
        while from < to {
            let last = text.line_start(text.prev(to));
            if last < from || !hidden(last) {
                break;
            }
            to = last;
        }
        if from >= to {
            return (start, end);
        }
        // Lines that end or begin in the middle of something the page
        // shows as one (a figure written over several lines, a table):
        // all of it, as it is one line of the page. Else `dj` above a
        // figure took its first line and left the rest as text.
        if host.cuts(from..to) {
            let len = text.0.len();
            let mut later = to;
            for _ in 0..LOOK_AT {
                if later >= len || !host.cuts(from..later) {
                    break;
                }
                later = line_end_inclusive(text, later);
            }
            let mut earlier = from;
            for _ in 0..LOOK_AT {
                if earlier == 0 || !host.cuts(earlier..to) {
                    break;
                }
                earlier = text.line_start(text.prev(earlier));
            }
            if !host.cuts(from..later) {
                to = later;
            } else if !host.cuts(earlier..to) {
                from = earlier;
            } else if !host.cuts(earlier..later) {
                (from, to) = (earlier, later);
            }
        }
        (from, to)
    }

    /// After `{` where the host shows pages: the empty line it went to
    /// shows nothing, and coming to rest before it would be on the end
    /// of the paragraph before. To the start of the paragraph instead,
    /// that is, behind that line; or, where the cursor was there
    /// already (at `before`), to the start of the one before.
    pub(super) fn shown_paragraph_start(&mut self, before: usize, host: &impl Host) {
        let start_behind = |at: usize| {
            let text = Text(host.text());
            if !text.line_is_empty(at) {
                return None;
            }
            host.stop(at, Seek::After).map(|stop| stop.start)
        };
        let Some(start) = start_behind(self.cursor) else {
            return;
        };
        if start < before {
            self.cursor = start;
            return;
        }
        // At the start already: one more paragraph back.
        let text = Text(host.text());
        let mut line = Some(self.cursor);
        while let Some(at) = line.and_then(|at| text.prev_line(at)) {
            if text.line_is_empty(at) {
                line = Some(at);
                continue;
            }
            // Up to the empty line before this paragraph, or the start.
            let mut first = at;
            while let Some(above) = text
                .prev_line(first)
                .filter(|above| !text.line_is_empty(*above))
            {
                first = above;
            }
            let found = text
                .prev_line(first)
                .and_then(start_behind)
                .or_else(|| host.stop(first, Seek::After).map(|stop| stop.start));
            if let Some(found) = found.filter(|found| *found < before) {
                self.cursor = found;
            }
            return;
        }
    }

    /// `ip` and `ap` where the host shows pages: `found`, the paragraph
    /// of the text around byte `at`, without the lines next to it that
    /// the host shows as things of their own (see [`Host::open`]): the
    /// rules that set up the page, a heading, that stand against it with
    /// no empty line between. Else `dap` would take those, unseen.
    pub(super) fn shown_paragraph(&self, found: Object, at: usize, host: &impl Host) -> Object {
        if !self.rows_first {
            return found;
        }
        let (Some((start, above, _)), Some((end, below, _))) =
            (host.open(at, false), host.open(at, true))
        else {
            return found;
        };
        if above != "\n\n" || below != "\n\n" {
            return found;
        }
        let text = Text(host.text());
        let filled = |range: Range<usize>| {
            text.0
                .get(range)
                .is_some_and(|between| !between.trim().is_empty())
        };
        let mut shown = found;
        if found.start < start && filled(found.start..start) {
            shown.start = start;
        }
        if end < found.end && filled(end..found.end) {
            shown.end = text.next_line(end).unwrap_or(text.0.len());
        }
        shown
    }

    /// What `operator` (a delete, a change or a yank, not of whole lines)
    /// takes of `object` where taking all of it would cut what the host
    /// shows in two (see [`Host::cuts`]): the places shown in it, each
    /// run of them that can go together as one, in the order of the
    /// text. So `3x` from before an equation into it takes what is shown,
    /// and the dollar sign between stays. `None` where all of it may go.
    pub(super) fn shown_parts(
        &self,
        operator: Operator,
        object: &Object,
        host: &impl Host,
    ) -> Option<Vec<Range<usize>>> {
        let range = object.start..object.end;
        let deletes = matches!(
            operator,
            Operator::Delete | Operator::Change | Operator::Yank
        );
        if !self.rows_first || !deletes || object.linewise || range.is_empty() {
            return None;
        }
        if !host.cuts(range.clone()) {
            return None;
        }
        Some(Self::parts_within(host, &range))
    }

    /// The places the host shows in bytes `range` (see
    /// [`Engine::shown_parts`]), each run of them that can be taken out
    /// together as one.
    fn parts_within(host: &impl Host, range: &Range<usize>) -> Vec<Range<usize>> {
        let stops = Self::shown_within(host, range).unwrap_or_default();
        let mut parts: Vec<Range<usize>> = Vec::new();
        for stop in stops.into_iter().filter(|stop| !stop.is_empty()) {
            match parts.last_mut() {
                Some(last) if stop.start <= last.end => last.end = last.end.max(stop.end),
                Some(last) if !host.cuts(last.start..stop.end) => last.end = stop.end,
                _ => parts.push(stop),
            }
        }
        parts
    }

    /// Before a key in normal mode that changes what is at the cursor
    /// (`x`, `d`, `r`, `p` and their like) where the host shows places
    /// of its own: a cursor that was put where nothing is shown, from
    /// outside the keys (to the mark of a heading a list of headings led
    /// to, to a label), comes to rest on what is shown nearest, as it
    /// does after every key. Else `x` or `dw` there would take marks
    /// nobody sees, and `i` would type before the mark of a heading,
    /// which makes it none. Other keys go from where it is: typing there
    /// goes into what that place is in.
    pub(super) fn rest_before_key(&mut self, press: KeyPress, host: &impl Host) {
        // Typing goes into what the place is in (at the dollar sign
        // that closes an equation: into it), but not before the mark
        // that begins a line: a heading, an item.
        let line_starts = || {
            let text = Text(host.text());
            let before = text.0.get(text.line_start(self.cursor)..self.cursor);
            before.is_some_and(|before| before.trim().is_empty())
        };
        let changes = !press.ctrl
            && match press.key {
                Key::Delete => true,
                Key::Char('i' | 'a') => line_starts(),
                Key::Char(c) => "xXsSrR~dcDCpPJ.".contains(c),
                _ => false,
            };
        let begun = self.pending.operator.is_some() || self.pending.awaiting != Awaiting::Nothing;
        if !self.rows_first || !changes || self.mode != Mode::Normal || begun {
            return;
        }
        if host.stop(self.cursor, Seek::At).is_some() {
            return;
        }
        let text = Text(host.text());
        let line = text.line_index(clamp(text, self.cursor, true));
        // What the cursor is in the middle of (a link, shown as one,
        // that a search led into); else what follows in its line before
        // what is before it.
        let within = host
            .stop(self.cursor, Seek::Before)
            .filter(|stop| stop.end > self.cursor);
        let after = host
            .stop(self.cursor, Seek::After)
            .filter(|stop| text.line_index(stop.start) == line);
        let stop = within.or(after);
        let stop = stop.or_else(|| host.stop(self.cursor, Seek::Before));
        let stop = stop.or_else(|| host.stop(self.cursor, Seek::After));
        if let Some(stop) = stop {
            self.cursor = stop.start;
            self.wanted_column = None;
        }
    }

    /// Ctrl-W (`word`) and Ctrl-U while typing, where the host shows the
    /// text as pages: back over what is shown, never into the marks it
    /// is made with. Ctrl-W goes back over the blanks before the cursor
    /// and then over the letters of a word, or the signs that are no
    /// letters, as shown; Ctrl-U to where typing began, or to the start
    /// of the paragraph. Where that would take a mark without what it
    /// marks (the star that ends strong text), only what is shown goes.
    ///
    /// `None` where the host does not show places of its own here: then
    /// these go by the characters of the text. Else whether anything went.
    pub(super) fn shown_erase(&mut self, word: bool, host: &mut impl Host) -> Option<bool> {
        if !self.rows_first || self.replacing {
            return None;
        }
        let text = Text(host.text());
        let start = text.line_start(self.cursor);
        let mut to = self.cursor;
        if word {
            let mut class = None;
            while to > start {
                let Some(stop) = host.stop(text.prev(to), Seek::Before) else {
                    break;
                };
                if stop.start >= to || stop.end > self.cursor || stop.start < start {
                    break;
                }
                let shown = text.0.get(stop.clone()).unwrap_or("");
                let blank = shown.chars().all(char::is_whitespace);
                let letters = shown.chars().all(|c| c.is_alphanumeric() || c == '_');
                match class {
                    None if blank => {}
                    None => class = Some(letters),
                    Some(_) if blank => break,
                    Some(was) if was != letters => break,
                    Some(_) => {}
                }
                to = stop.start;
            }
        } else {
            to = start;
        }
        let to = self.stop_at_insert_start(host.text(), to);
        if to >= self.cursor || host.stop(to, Seek::After).is_none() {
            return None;
        }
        let range = to..self.cursor;
        if !host.cuts(range.clone()) {
            self.insert_delete(to, host);
            return Some(true);
        }
        let parts = Self::parts_within(host, &range);
        let Some(first) = parts.first().map(|part| part.start) else {
            return Some(false);
        };
        for part in parts.iter().rev() {
            self.edit(host, part.clone(), "");
        }
        self.cursor = first;
        Some(true)
    }

    /// Whether the host refuses what `operator` would take of `object`
    /// (`parts` of it, where only those go): see [`Host::refuses`]. Then
    /// the command fails and nothing changes.
    pub(super) fn shown_refused(
        &mut self,
        operator: Operator,
        object: &Object,
        parts: Option<&[Range<usize>]>,
        host: &mut impl Host,
    ) -> bool {
        let deletes = matches!(operator, Operator::Delete | Operator::Change);
        if !self.rows_first || !deletes || object.linewise || object.start >= object.end {
            return false;
        }
        let whole = object.start..object.end;
        let taken: Vec<Edit> = parts
            .unwrap_or(std::slice::from_ref(&whole))
            .iter()
            .map(|range| Edit {
                range: range.clone(),
                text: String::new(),
            })
            .collect();
        if !host.refuses(&taken) {
            return false;
        }
        self.fail();
        self.changed = false;
        self.undo_block = None;
        self.undo_cursor = None;
        self.column_before = None;
        self.wanted_column = None;
        self.cursor = clamp(Text(host.text()), self.cursor, false);
        true
    }

    /// Takes `parts` (see [`Engine::shown_parts`]) out of the text for
    /// `operator`, as one change: what they held goes into the register
    /// together, and the cursor to where the first of them was, to type
    /// there after a change. Where nothing is shown in them, nothing is
    /// taken, and the command fails.
    pub(super) fn delete_parts(
        &mut self,
        operator: Operator,
        parts: &[Range<usize>],
        host: &mut impl Host,
    ) {
        let Some(first) = parts.first().map(|part| part.start) else {
            self.cursor = clamp(Text(host.text()), self.cursor, false);
            return self.fail();
        };
        let text = host.text();
        let taken: String = parts
            .iter()
            .filter_map(|part| text.get(part.clone()))
            .collect();
        self.took = Some(Took::Deleted {
            text: taken.clone(),
            at: first,
            linewise: false,
        });
        self.store(taken, false, true);
        // From the last: the parts before it stay where they are.
        for part in parts.iter().rev() {
            self.edit(host, part.clone(), "");
        }
        if operator == Operator::Change {
            self.cursor = first;
            self.enter_insert(1, host.text());
        } else {
            self.cursor = clamp(Text(host.text()), first, false);
        }
    }

    /// `x` and `s` on a place that shows no bytes, at byte `at`: a part
    /// of an equation that is yet to be typed. `s` begins to type it.
    /// `x` takes what made the place, as Backspace does while typing
    /// (see [`Host::unmake`]): the mark that raises or lowers nothing,
    /// the fraction with no parts. Where that cannot go by itself (the
    /// other part of the fraction is there) nothing happens. The cursor
    /// goes to what is shown before what went.
    pub(super) fn shown_empty(&mut self, c: char, at: usize, host: &mut impl Host) {
        self.reset_pending();
        if c == 's' {
            self.cursor = at;
            return self.enter_insert(1, host.text());
        }
        match host.unmake(at) {
            Some((place, left)) if place.start < place.end && place.end <= host.text().len() => {
                // On to what it stood by: what was to be raised stays,
                // and is what is shown before the mark that goes.
                let before = host
                    .stop(Text(host.text()).prev(place.start), Seek::Before)
                    .filter(|stop| stop.start < place.start);
                self.edit(host, place.clone(), &left);
                let rest = before.map_or(place.start, |stop| stop.start);
                self.cursor = clamp(Text(host.text()), rest, false);
                self.wanted_column = None;
            }
            _ => self.fail(),
        }
    }
}
