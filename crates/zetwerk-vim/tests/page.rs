//! The engine on a host that shows its text as a page.
//!
//! Where the host has places for the cursor that are not the characters
//! of the text (see `Host::stop`), keys go by those. Here a page is made
//! by hand: every piece of the text says how it is shown, and the tests
//! check that what Vim's keys count, find and change is what is shown.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a broken harness should fail loudly"
)]

use std::ops::Range;

use zetwerk_vim::keys;
use zetwerk_vim::{Edit, Engine, Host, Mode, Seek, Shown};

/// A piece of the text, and how the page shows it.
#[derive(Clone, Copy)]
enum Piece<'a> {
    /// Every character is a place for the cursor.
    Each(&'a str),
    /// All of it is one place: a symbol written by its name, a reference.
    One(&'a str),
    /// Nothing is shown of it: markup.
    Hidden(&'a str),
    /// Nothing is shown of it, and the line the page shows ends there.
    Broken(&'a str),
    /// No text, but a place: a part of an equation yet to be typed.
    Empty,
}
use Piece::{Broken, Each, Empty, Hidden, One};

/// A text and the page it was typeset as.
struct Page {
    text: String,
    /// The places for the cursor in the order of the page, each with the
    /// line it is in.
    stops: Vec<(Range<usize>, usize)>,
    /// Whether the text has changed since: a page shows the text it was
    /// made of.
    stale: bool,
}

impl Page {
    fn of(pieces: &[Piece]) -> Self {
        let mut page = Self {
            text: String::new(),
            stops: Vec::new(),
            stale: false,
        };
        let mut line = 0;
        for piece in pieces {
            let at = page.text.len();
            match *piece {
                Each(text) => {
                    for (offset, c) in text.char_indices() {
                        let start = at + offset;
                        page.stops.push((start..start + c.len_utf8(), line));
                    }
                    page.text.push_str(text);
                }
                One(text) => {
                    page.stops.push((at..at + text.len(), line));
                    page.text.push_str(text);
                }
                Hidden(text) => page.text.push_str(text),
                Broken(text) => {
                    page.text.push_str(text);
                    line += 1;
                }
                Empty => page.stops.push((at..at, line)),
            }
        }
        page
    }

    /// Typesets the text anew, as `pieces`: they must be the text as it
    /// is now.
    fn typeset(&mut self, pieces: &[Piece]) {
        let page = Self::of(pieces);
        assert_eq!(page.text, self.text);
        self.stops = page.stops;
        self.stale = false;
    }

    /// Which place begins at byte `at`; of two, the one with no bytes.
    fn index_at(&self, at: usize) -> Option<usize> {
        let begins = |(_, (stop, _)): &(usize, &(Range<usize>, usize))| stop.start == at;
        let found = self.stops.iter().enumerate().filter(begins);
        found
            .min_by_key(|(_, (stop, _))| stop.end)
            .map(|(index, _)| index)
    }
}

impl Host for Page {
    fn text(&self) -> &str {
        &self.text
    }

    fn edit(&mut self, edits: &[Edit], _join: bool) {
        for edit in edits {
            self.text.replace_range(edit.range.clone(), &edit.text);
        }
        self.stale = true;
    }

    fn undo(&mut self) -> Option<Range<usize>> {
        None
    }

    fn redo(&mut self) -> Option<Range<usize>> {
        None
    }

    fn stop(&self, at: usize, seek: Seek) -> Option<Range<usize>> {
        if self.stale {
            return None;
        }
        let stops = || self.stops.iter().map(|(stop, _)| stop);
        let found = match seek {
            Seek::At => return self.index_at(at).map(|index| self.stops[index].0.clone()),
            Seek::After => stops()
                .filter(|stop| stop.start >= at)
                .min_by_key(|stop| (stop.start, stop.end)),
            Seek::Before => stops()
                .filter(|stop| stop.start <= at)
                .max_by_key(|stop| (stop.start, std::cmp::Reverse(stop.end))),
            Seek::Next | Seek::Previous => {
                let index = self.index_at(at)?;
                let line = self.stops[index].1;
                let beside = |(stop, of): &&(Range<usize>, usize)| *of == line && stop.start != at;
                let found = if seek == Seek::Next {
                    self.stops[index + 1..].iter().find(beside)
                } else {
                    self.stops[..index].iter().rev().find(beside)
                };
                found.map(|(stop, _)| stop)
            }
        };
        found.cloned()
    }

    fn shown_line(&self, at: usize) -> Option<Shown> {
        if self.stale {
            return None;
        }
        let (_, line) = self
            .stops
            .iter()
            .filter(|(stop, _)| stop.start <= at)
            .max_by_key(|(stop, _)| stop.start)?;
        let mut in_line = self.stops.iter().filter(|(_, of)| of == line);
        let first = in_line.next()?.0.clone();
        let last = in_line
            .next_back()
            .map_or(first.clone(), |(stop, _)| stop.clone());
        Some(Shown {
            start: first.start,
            last: last.start,
            end: last.end,
        })
    }

    /// A reference is no text; all else is, and is written as it is.
    fn put_over(&self, stop: Range<usize>, c: char) -> Option<(Range<usize>, String, usize)> {
        let refers = self.text.get(stop.clone())?.starts_with('@');
        (!refers).then(|| (stop, c.to_string(), 0))
    }

    /// The characters that are places of their own, where they are
    /// letters.
    fn letters(&self, range: Range<usize>) -> Option<Vec<Range<usize>>> {
        let letter = |stop: &Range<usize>| {
            let within = range.start <= stop.start && stop.end <= range.end;
            let read = self.text.get(stop.clone())?.chars().next()?;
            (within && stop.len() == read.len_utf8() && read.is_alphabetic()).then(|| stop.clone())
        };
        let mut letters: Vec<Range<usize>> = self
            .stops
            .iter()
            .filter_map(|(stop, _)| letter(stop))
            .collect();
        letters.sort_by_key(|range| range.start);
        Some(letters)
    }

    /// Paragraphs are parted by an empty line.
    fn join(&self, at: usize, spaced: bool) -> Option<(Range<usize>, String)> {
        let with = if spaced { " " } else { "" };
        let seam = self.text.get(at..)?.find("\n\n").map(|found| at + found);
        Some(seam.map_or((at..at, String::new()), |seam| {
            (seam..seam + 2, with.to_owned())
        }))
    }

    /// What raises or lowers nothing goes; nothing else does.
    fn unmake(&self, at: usize) -> Option<(Range<usize>, String)> {
        let mark = at.checked_sub(1)?;
        matches!(self.text.get(mark..at)?, "^" | "_").then(|| (mark..at, String::new()))
    }

    fn line_break(&self, at: usize) -> Option<(Range<usize>, String)> {
        Some((at..at, "\n\n".to_owned()))
    }
}

/// A Vim on a page, with the cursor at byte `at`.
fn vim(page: &Page, at: usize) -> Engine {
    let mut engine = Engine::new();
    engine.set_rows_first(true);
    engine.set_cursor(at, page.text());
    engine
}

fn press(engine: &mut Engine, page: &mut Page, notation: &str) {
    for key in keys::parse(notation).unwrap() {
        engine.key(key, page);
    }
}

/// Where `needle` begins in the text of `page`.
fn at(page: &Page, needle: &str) -> usize {
    page.text.find(needle).unwrap()
}

/// Two lines of a paragraph with marks and an equation in it, and a
/// second paragraph.
fn prose() -> Page {
    Page::of(&[
        Each("One "),
        Hidden("*"),
        Each("two"),
        Hidden("*"),
        Each(" "),
        Hidden("$"),
        One("alpha"),
        Hidden(" "),
        Each("+"),
        Hidden(" "),
        One("beta"),
        Hidden("$"),
        Each(" on"),
        Broken(" "),
        Each("to one."),
        Broken("\n\n"),
        Each("Next."),
        Hidden("\n"),
    ])
}

#[test]
fn finding_goes_by_what_the_line_shows() {
    let mut page = prose();
    let mut engine = vim(&page, 0);
    // Over marks, which show as nothing.
    press(&mut engine, &mut page, "ft");
    assert_eq!(engine.cursor(), at(&page, "two"));
    // A symbol is found by the letter its name begins with, and by no
    // other of its letters.
    press(&mut engine, &mut page, "fb");
    assert_eq!(engine.cursor(), at(&page, "beta"));
    press(&mut engine, &mut page, "0fa");
    assert_eq!(engine.cursor(), at(&page, "alpha"));
    press(&mut engine, &mut page, ";");
    assert_eq!(engine.cursor(), at(&page, "alpha"), "no other in the line");
    // The line is the one shown: what follows on the next is not in it.
    press(&mut engine, &mut page, "fo");
    assert_eq!(engine.cursor(), at(&page, "on"));
    press(&mut engine, &mut page, ";");
    assert_eq!(engine.cursor(), at(&page, "on"));
    press(&mut engine, &mut page, ",");
    assert_eq!(engine.cursor(), at(&page, "o*"), "and back");
    // Backwards, and up to what is found.
    press(&mut engine, &mut page, "$Fe");
    assert_eq!(engine.cursor(), at(&page, "e "));
    press(&mut engine, &mut page, "tb");
    assert_eq!(engine.cursor(), at(&page, "+"), "the place before it");
    press(&mut engine, &mut page, "0Tn");
    assert_eq!(engine.cursor(), 0, "nothing before the first place");
    press(&mut engine, &mut page, "$TO");
    assert_eq!(engine.cursor(), 1);
    // Counted, they are places that are counted.
    press(&mut engine, &mut page, "02fo");
    assert_eq!(engine.cursor(), at(&page, "on"));
    // A blank is found by the space key.
    press(&mut engine, &mut page, "0f<Space>;");
    assert_eq!(engine.cursor(), at(&page, " $"));
}

#[test]
fn an_operator_takes_what_was_found_whole() {
    let mut page = prose();
    let mut engine = vim(&page, at(&page, "+"));
    press(&mut engine, &mut page, "dfb");
    assert_eq!(page.text, "One *two* $alpha $ on to one.\n\nNext.\n");
    // Up to it, and backwards: what the cursor is on stays.
    let mut page = prose();
    let mut engine = vim(&page, at(&page, "alpha"));
    press(&mut engine, &mut page, "dtb");
    assert_eq!(page.text, "One *two* $ beta$ on to one.\n\nNext.\n");
    let mut page = prose();
    let mut engine = vim(&page, at(&page, "beta"));
    press(&mut engine, &mut page, "dFa");
    assert_eq!(page.text, "One *two* $beta$ on to one.\n\nNext.\n");
    let mut page = prose();
    let mut engine = vim(&page, at(&page, "beta"));
    press(&mut engine, &mut page, "dTa");
    assert_eq!(page.text, "One *two* $alpha beta$ on to one.\n\nNext.\n");
}

#[test]
fn counts_count_what_is_shown() {
    let mut page = prose();
    let mut engine = vim(&page, 0);
    let mut seen = Vec::new();
    for keys in ["3l", "4l", "2l", "l", "3h", "h", "20h"] {
        press(&mut engine, &mut page, keys);
        seen.push(engine.cursor());
        let shown = page.stop(engine.cursor(), Seek::At);
        assert!(shown.is_some(), "after {keys} the cursor is on nothing");
    }
    let places = [" *", " $", "+", "beta", " $", "o*", "One"];
    assert_eq!(seen, places.map(|needle| at(&page, needle)));
    // On through the lines of a paragraph, and no further; the space
    // key and Backspace go on.
    press(&mut engine, &mut page, "/to<CR>10l");
    assert_eq!(engine.cursor(), at(&page, ".\n\nNext"));
    press(&mut engine, &mut page, "l");
    assert_eq!(engine.cursor(), at(&page, ".\n\nNext"));
    press(&mut engine, &mut page, "<Space>");
    assert_eq!(engine.cursor(), at(&page, "Next"));
    press(&mut engine, &mut page, "h");
    assert_eq!(engine.cursor(), at(&page, "Next"));
    press(&mut engine, &mut page, "<BS>");
    assert_eq!(engine.cursor(), at(&page, ".\n\nNext"));
    // Deleting takes as many places, also the symbol that is one.
    engine.set_cursor(0, &page.text);
    press(&mut engine, &mut page, "3x");
    assert_eq!(page.text, " *two* $alpha + beta$ on to one.\n\nNext.\n");
    let mut page = prose();
    let mut engine = vim(&page, at(&page, "o*"));
    press(&mut engine, &mut page, "2X");
    assert_eq!(page.text, "One *o* $alpha + beta$ on to one.\n\nNext.\n");
    let mut page = prose();
    let mut engine = vim(&page, at(&page, " $"));
    press(&mut engine, &mut page, "d3l");
    assert_eq!(page.text, "One *two* beta$ on to one.\n\nNext.\n");
    let mut page = prose();
    let mut engine = vim(&page, at(&page, "+"));
    press(&mut engine, &mut page, "dh");
    assert_eq!(page.text, "One *two* $+ beta$ on to one.\n\nNext.\n");
}

/// A line with a reference and an equation in it.
fn referring() -> Page {
    Page::of(&[
        Each("See "),
        One("@ref"),
        Each(" and "),
        Hidden("$"),
        One("alpha"),
        Hidden("$"),
        Each(" ok."),
        Hidden("\n"),
    ])
}

#[test]
fn a_character_takes_the_place_of_what_is_shown() {
    let mut page = referring();
    let mut engine = vim(&page, 0);
    press(&mut engine, &mut page, "rT");
    assert_eq!(page.text, "Tee @ref and $alpha$ ok.\n");
    assert_eq!(engine.cursor(), 0);
    // A symbol goes whole; what is no text stays as it is.
    let mut page = referring();
    let mut engine = vim(&page, at(&page, "alpha"));
    press(&mut engine, &mut page, "rx");
    assert_eq!(page.text, "See @ref and $x$ ok.\n");
    assert_eq!(engine.cursor(), at(&page, "x$"));
    let mut page = referring();
    let mut engine = vim(&page, at(&page, "@ref"));
    press(&mut engine, &mut page, "rx");
    assert_eq!(page.text, "See @ref and $alpha$ ok.\n");
    // Counted: that many places, or none if there are not as many.
    let mut engine = vim(&page, at(&page, "ok."));
    press(&mut engine, &mut page, "4rz");
    assert_eq!(page.text, "See @ref and $alpha$ ok.\n");
    press(&mut engine, &mut page, "3rz");
    assert_eq!(page.text, "See @ref and $alpha$ zzz\n");
    assert_eq!(engine.cursor(), at(&page, "z\n"));
    // In a selection: each place that holds text.
    let mut page = referring();
    let mut engine = vim(&page, at(&page, "e "));
    press(&mut engine, &mut page, "vfnr-");
    assert_eq!(page.text, "Se--@ref---d $alpha$ ok.\n");
    assert_eq!(engine.mode(), Mode::Normal);
    // Enter parts the paragraph where a blank stood.
    let mut page = referring();
    let mut engine = vim(&page, at(&page, " and"));
    press(&mut engine, &mut page, "r<CR>");
    assert_eq!(page.text, "See @ref\n\nand $alpha$ ok.\n");
    assert_eq!(engine.cursor(), at(&page, "and"));
}

#[test]
fn case_is_switched_of_the_letters_shown() {
    let mut page = referring();
    let mut engine = vim(&page, 0);
    press(&mut engine, &mut page, "3~");
    assert_eq!(page.text, "sEE @ref and $alpha$ ok.\n");
    assert_eq!(engine.cursor(), at(&page, " @"), "on behind them");
    // The name of a symbol is no letters to read.
    let mut page = referring();
    let mut engine = vim(&page, at(&page, "alpha"));
    press(&mut engine, &mut page, "~");
    assert_eq!(page.text, "See @ref and $alpha$ ok.\n");
    assert_eq!(engine.cursor(), at(&page, " ok"));
    // So with the operators, on a line and in a selection.
    press(&mut engine, &mut page, "g~~");
    assert_eq!(page.text, "sEE @ref AND $alpha$ OK.\n");
    let mut page = referring();
    let mut engine = vim(&page, 0);
    press(&mut engine, &mut page, "v$U");
    assert_eq!(page.text, "SEE @ref AND $alpha$ OK.\n");
}

#[test]
fn paragraphs_are_joined_not_lines() {
    let three = || {
        Page::of(&[
            Each("One"),
            Broken(" "),
            Each("more."),
            Broken("\n\n"),
            Each("Two."),
            Broken("\n\n"),
            Each("Three."),
            Hidden("\n"),
        ])
    };
    let mut page = three();
    let mut engine = vim(&page, 0);
    press(&mut engine, &mut page, "J");
    assert_eq!(page.text, "One more. Two.\n\nThree.\n");
    assert_eq!(engine.cursor(), at(&page, " Two"), "where they meet");
    let mut page = three();
    let mut engine = vim(&page, 0);
    press(&mut engine, &mut page, "3J");
    assert_eq!(page.text, "One more. Two. Three.\n");
    let mut page = three();
    let mut engine = vim(&page, 0);
    press(&mut engine, &mut page, "gJ");
    assert_eq!(page.text, "One more.Two.\n\nThree.\n");
    // All that a selection touches.
    let mut page = three();
    let mut engine = vim(&page, 0);
    press(&mut engine, &mut page, "v/Thr<CR>J");
    assert_eq!(page.text, "One more. Two. Three.\n");
    assert_eq!(engine.mode(), Mode::Normal);
    // The last one has nothing to be joined with.
    let mut page = three();
    let mut engine = vim(&page, at(&page, "Three"));
    press(&mut engine, &mut page, "J");
    assert_eq!(page.text, three().text);
}

/// A line with an equation that raises nothing, and one with a fraction
/// that has no parts yet.
fn unfinished() -> [Piece<'static>; 14] {
    [
        Each("a "),
        Hidden("$"),
        Each("x"),
        Hidden("^"),
        Empty,
        Hidden("$"),
        Each(" b "),
        Hidden("$frac("),
        Empty,
        Hidden(", "),
        Empty,
        Hidden(")$"),
        Each(" c"),
        Hidden("\n"),
    ]
}

#[test]
fn the_cursor_rests_on_what_is_yet_to_be_typed() {
    let mut page = Page::of(&unfinished());
    let (raised, above, below) = (at(&page, "$ b"), at(&page, ", )"), at(&page, ")$"));
    let mut engine = vim(&page, at(&page, "x^"));
    // Through the empty places and back, and never into the word that
    // shows as a bar.
    let mut seen = Vec::new();
    for _ in 0..7 {
        press(&mut engine, &mut page, "l");
        seen.push(engine.cursor());
    }
    let onward = [
        raised,
        raised + 1,
        raised + 2,
        raised + 3,
        above,
        below,
        below + 2,
    ];
    assert_eq!(seen, onward);
    seen.clear();
    for _ in 0..4 {
        press(&mut engine, &mut page, "h");
        seen.push(engine.cursor());
    }
    assert_eq!(seen, [below, above, raised + 3, raised + 2]);
    press(&mut engine, &mut page, "02w");
    assert!(page.stop(engine.cursor(), Seek::At).is_some());
    // A selection that ends on one takes nothing of what follows it.
    engine.set_cursor(at(&page, "x^"), &page.text);
    press(&mut engine, &mut page, "vl");
    assert_eq!(engine.selection(&page.text), Some(raised - 2..raised));
    press(&mut engine, &mut page, "<Esc>");
    // Typing begins in the place.
    engine.set_cursor(above, &page.text);
    press(&mut engine, &mut page, "ia<Esc>");
    assert_eq!(page.text, "a $x^$ b $frac(a, )$ c\n");
    let mut page = Page::of(&unfinished());
    let mut engine = vim(&page, below);
    press(&mut engine, &mut page, "ab<Esc>");
    assert_eq!(page.text, "a $x^$ b $frac(, b)$ c\n");
    let mut page = Page::of(&unfinished());
    let mut engine = vim(&page, raised);
    press(&mut engine, &mut page, "sn<Esc>");
    assert_eq!(page.text, "a $x^n$ b $frac(, )$ c\n");
}

#[test]
fn an_empty_place_goes_with_what_made_it() {
    let mut page = Page::of(&unfinished());
    let (raised, above, below) = (at(&page, "$ b"), at(&page, ", )"), at(&page, ")$"));
    let mut engine = vim(&page, raised);
    press(&mut engine, &mut page, "x");
    assert_eq!(page.text, "a $x$ b $frac(, )$ c\n");
    assert_eq!(engine.cursor(), at(&page, "x"), "on what it stood by");
    // What cannot go by itself stays.
    let mut page = Page::of(&unfinished());
    let mut engine = vim(&page, below);
    press(&mut engine, &mut page, "x");
    assert_eq!(page.text, "a $x^$ b $frac(, )$ c\n");
    // While typing, the cursor keys lead from one to the next, and out
    // to before what is shown next; Backspace behind one leads into it.
    engine.set_cursor(above, &page.text);
    press(&mut engine, &mut page, "i<Right>");
    assert_eq!(engine.cursor(), below);
    press(&mut engine, &mut page, "<Left>");
    assert_eq!(engine.cursor(), above);
    press(&mut engine, &mut page, "<Right><Right>");
    assert_eq!(engine.cursor(), below + 2);
    press(&mut engine, &mut page, "<BS>");
    assert_eq!((engine.cursor(), page.stale), (below, false));
    press(&mut engine, &mut page, "<Esc>");
    assert_eq!(page.text, "a $x^$ b $frac(, )$ c\n");
    // Typeset anew, the text is the page again.
    engine.set_cursor(raised, &page.text);
    press(&mut engine, &mut page, "x");
    assert_eq!(engine.cursor(), at(&page, "x"));
    page.typeset(&[
        Each("a "),
        Hidden("$"),
        Each("x"),
        Hidden("$"),
        Each(" b "),
        Hidden("$frac("),
        Empty,
        Hidden(", "),
        Empty,
        Hidden(")$"),
        Each(" c"),
        Hidden("\n"),
    ]);
    press(&mut engine, &mut page, "2l");
    assert_eq!(engine.cursor(), at(&page, "b "));
}
