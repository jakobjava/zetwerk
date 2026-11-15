//! The engine on a host that shows its text as something to be read.
//!
//! Where the host says what the text is read as (see `Host::reading`),
//! `/`, `?`, `n`, `*` and `#` look for that and not for what the text is
//! written with. Here the reading is made by hand: every piece of the
//! text says what it is read as. The engine also says what a key
//! yanked, deleted or put (`Engine::took`), for the host to show.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a broken harness should fail loudly"
)]

use std::ops::Range;
use std::sync::Arc;

use zetwerk_vim::keys;
use zetwerk_vim::{Edit, Engine, Host, Reading, Took};

/// A text and what it is read as, or only a text.
struct Read {
    text: String,
    reading: Option<Arc<Reading>>,
}

/// A piece of the text: as written, and as read (`None`: as written).
type Piece<'a> = (&'a str, Option<&'a str>);

impl Read {
    fn of(pieces: &[Piece]) -> Self {
        let mut text = String::new();
        let mut reading = Reading::new();
        for (written, read) in pieces {
            let start = text.len();
            text.push_str(written);
            match read {
                None => reading.push_verbatim(written, start),
                Some(read) => reading.push(read, start..text.len()),
            }
        }
        Self {
            text,
            reading: Some(Arc::new(reading)),
        }
    }

    fn plain(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            reading: None,
        }
    }
}

impl Host for Read {
    fn text(&self) -> &str {
        &self.text
    }

    fn edit(&mut self, edits: &[Edit], _join: bool) {
        for edit in edits {
            self.text.replace_range(edit.range.clone(), &edit.text);
        }
        // What it is read as is not known any more.
        self.reading = None;
    }

    fn undo(&mut self) -> Option<Range<usize>> {
        None
    }

    fn redo(&mut self) -> Option<Range<usize>> {
        None
    }

    fn reading(&self) -> Option<Arc<Reading>> {
        self.reading.clone()
    }
}

fn press(engine: &mut Engine, host: &mut Read, notation: &str) {
    for key in keys::parse(notation).unwrap() {
        engine.key(key, host);
    }
}

/// "The *quick*\nbrown fox: $alpha$ and Quick." as a page shows it.
fn prose() -> Read {
    Read::of(&[
        ("The", None),
        (" ", None),
        ("*", Some("")),
        ("quick", None),
        ("*", Some("")),
        ("\n", Some(" ")),
        ("brown fox: ", None),
        ("$", Some("")),
        ("alpha", Some("α")),
        ("$", Some("")),
        (" and Quick.", None),
    ])
}

fn at(host: &Read, needle: &str) -> usize {
    host.text.find(needle).unwrap()
}

#[test]
fn a_search_goes_over_what_is_read() {
    let mut host = prose();
    let mut engine = Engine::new();
    // Over markup and a line break of the text.
    press(&mut engine, &mut host, "/quick brown<CR>");
    assert_eq!(engine.cursor(), at(&host, "quick"));
    // A symbol is found as what it is, not by its name.
    press(&mut engine, &mut host, "gg/α<CR>");
    assert_eq!(engine.cursor(), at(&host, "alpha"));
    press(&mut engine, &mut host, "gg0/alpha<CR>");
    assert_eq!(engine.cursor(), 0);
    // `\=`: the text as written.
    press(&mut engine, &mut host, "/\\=alpha<CR>");
    assert_eq!(engine.cursor(), at(&host, "alpha"));
    press(&mut engine, &mut host, "gg0/\\=*quick*<CR>");
    assert_eq!(engine.cursor(), at(&host, "*quick*"));
}

#[test]
fn small_letters_find_capitals_too() {
    let mut host = prose();
    let mut engine = Engine::new();
    press(&mut engine, &mut host, "/quick<CR>");
    assert_eq!(engine.cursor(), at(&host, "quick"));
    press(&mut engine, &mut host, "n");
    assert_eq!(engine.cursor(), at(&host, "Quick"));
    press(&mut engine, &mut host, "n");
    assert_eq!(engine.cursor(), at(&host, "quick"));
    // A capital makes case count; so does `\C`.
    press(&mut engine, &mut host, "/Quick<CR>n");
    assert_eq!(engine.cursor(), at(&host, "Quick"));
    press(&mut engine, &mut host, "gg0/quick\\C<CR>n");
    assert_eq!(engine.cursor(), at(&host, "quick"));
    // `?` and `N` go back.
    press(&mut engine, &mut host, "G?the<CR>");
    assert_eq!(engine.cursor(), 0);
}

#[test]
fn star_takes_the_word_as_read() {
    let mut host = prose();
    let mut engine = Engine::new();
    // On the asterisk before "quick": the word behind it, whole, either
    // case.
    engine.set_cursor(at(&host, "*quick"), &host.text);
    press(&mut engine, &mut host, "*");
    assert_eq!(engine.cursor(), at(&host, "Quick"));
    press(&mut engine, &mut host, "#");
    assert_eq!(engine.cursor(), at(&host, "quick"));
    assert_eq!(engine.searches(), 2);
    let search = engine.last_search().unwrap();
    assert!(search.word_start && search.word_end);
}

#[test]
fn where_nothing_is_read_the_text_is_searched_as_vim_does() {
    let mut host = Read::plain("Foo foo\n");
    let mut engine = Engine::new();
    press(&mut engine, &mut host, "/foo<CR>");
    assert_eq!(engine.cursor(), 4);
    // Vim's `\c`.
    press(&mut engine, &mut host, "/\\cfoo<CR>");
    assert_eq!(engine.cursor(), 0);
}

#[test]
fn what_a_key_took_is_told() {
    let mut host = Read::plain("one two\nthree\n");
    let mut engine = Engine::new();
    press(&mut engine, &mut host, "yw");
    assert_eq!(engine.took(), Some(&Took::Yanked(0..4)));
    press(&mut engine, &mut host, "w");
    assert_eq!(engine.took(), None);
    press(&mut engine, &mut host, "P");
    assert_eq!(engine.took(), Some(&Took::Put(4..8)));
    assert_eq!(host.text, "one one two\nthree\n");
    press(&mut engine, &mut host, "dd");
    let deleted = Took::Deleted {
        text: "one one two\n".to_owned(),
        at: 0,
        linewise: true,
    };
    assert_eq!(engine.took(), Some(&deleted));
    press(&mut engine, &mut host, "0x");
    assert!(matches!(engine.took(), Some(Took::Deleted { text, .. }) if text == "t"));
}
