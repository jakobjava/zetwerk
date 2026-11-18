//! Nothing on screen flickers, jumps or lags: what is drawn over and
//! beside the pages, and where the view is, frame by frame, across the
//! life of a key: the frame after it and the next, before its pages are
//! made; the one in which a compile of the text before it lands late; the
//! one its own pages come in; and the one after they are made a second
//! time of the same text.
//!
//! No texture is held by the compositor here, so no tile is drawn: what a
//! frame draws is the paper, the backdrop and what lies over and beside
//! the pages, which is what is looked at.

use std::fmt::Write as _;

use super::*;

/// What one frame drew over and beside the pages: where the view was, and
/// every quad that is not paper or backdrop, by its colour or its image.
#[derive(Debug, Clone, PartialEq)]
struct Frame {
    top: f64,
    quads: Vec<(String, [i32; 4])>,
}

/// The frame around what typing goes into (see `focus`).
const FOCUS_FRAME: [u8; 4] = [0x45, 0x85, 0x88, 0x80];

fn name_of(texture: TextureId) -> String {
    let tints = [
        ("cursor", PAGE_CURSOR),
        ("bar", PAGE_BAR),
        ("selection", PAGE_SELECTION),
        ("focus", FOCUS_FRAME),
    ];
    for (name, rgba) in tints {
        if texture == TextureId(hash128(&("tint", rgba))) {
            return name.to_owned();
        }
    }
    for rgb in [BACKDROP, PAPER] {
        if texture == TextureId(hash128(&("solid", rgb))) {
            return String::new();
        }
    }
    format!("{:x}", texture.0 & 0xffff)
}

/// What is drawn over and beside the pages, not under the status line.
fn frame(viewer: &mut Viewer) -> Frame {
    let scene = viewer.scene(&|_| false);
    let height = f64::from(viewer.view.height) as f32;
    let quads = scene
        .quads
        .iter()
        .filter(|quad| quad.y < height)
        .map(|quad| (name_of(quad.texture), quad))
        .filter(|(name, _)| !name.is_empty())
        .map(|(name, quad)| {
            let rect = [quad.x, quad.y, quad.width, quad.height];
            (name, rect.map(|v| v.round() as i32))
        })
        .collect();
    Frame {
        top: viewer.view.top,
        quads,
    }
}

fn named(frame: &Frame, names: &[&str]) -> Vec<[i32; 4]> {
    let of = |(name, _): &&(String, [i32; 4])| names.contains(&name.as_str());
    frame.quads.iter().filter(of).map(|q| q.1).collect()
}

fn cursors(frame: &Frame) -> Vec<[i32; 4]> {
    named(frame, &["bar", "cursor"])
}

/// The image the status line is drawn from.
fn status(viewer: &mut Viewer) -> Option<TextureId> {
    let scene = viewer.scene(&|_| false);
    let height = f64::from(viewer.view.height) as f32;
    let below = |quad: &&Quad| quad.y >= height;
    scene
        .quads
        .iter()
        .rev()
        .find(below)
        .map(|quad| quad.texture)
}

/// The pages of the text as it is now, made later: a compile to land.
type Snapshot = (u64, zetwerk_typst::ProjectWorld);

fn land(viewer: &mut Viewer, (revision, world): Snapshot) {
    let compiled = zetwerk_typst::compile(&world);
    viewer.compiled(CompileOutput {
        revision,
        world: Arc::new(world),
        compiled,
        duration: Duration::from_millis(3),
    });
}

/// The frames of each of `keys` (one string per key), typed after
/// `setup` on `text`: after the key, the next, after a compile of the
/// text before the key lands late, after its own compile, after a second
/// one of the same text.
fn frames_of_keys(text: &str, setup: &str, keys: &[&str]) -> Vec<(String, Vec<Frame>)> {
    frames_of_keys_in(text, setup, keys, &|_| {})
}

/// Like [`frames_of_keys`], in a viewer `adjust`ed first.
fn frames_of_keys_in(
    text: &str,
    setup: &str,
    keys: &[&str],
    adjust: &dyn Fn(&mut Viewer),
) -> Vec<(String, Vec<Frame>)> {
    let (mut viewer, directory) = editing(text);
    adjust(&mut viewer);
    press(&mut viewer, setup);
    compile(&mut viewer);
    let mut all = vec![("settled".to_owned(), vec![frame(&mut viewer)])];
    for key in keys {
        let before = viewer.snapshot();
        press(&mut viewer, key);
        let mut frames = vec![frame(&mut viewer), frame(&mut viewer)];
        land(&mut viewer, before);
        frames.push(frame(&mut viewer));
        compile(&mut viewer);
        frames.push(frame(&mut viewer));
        compile(&mut viewer);
        frames.push(frame(&mut viewer));
        all.push(((*key).to_owned(), frames));
    }
    let _ = std::fs::remove_dir_all(directory);
    all
}

const PAGE: &str = "#set page(width: 300pt, height: 400pt)\n";

fn paragraphs() -> String {
    format!(
        "{PAGE}First paragraph with several words that go on for more than one \
         line of the page so that it wraps.\n\nSecond one.\n"
    )
}

/// Documents, where in them the keys begin, and the keys: typing in
/// insert mode, and the keys of normal mode that change the text.
fn cases() -> Vec<(String, &'static str, Vec<&'static str>)> {
    let typing = vec![" ", "a", "b", " ", "<BS>", "<Esc>", "i", "<Left>", "c"];
    let mut cases = vec![
        (
            paragraphs(),
            "/Second<CR>",
            vec!["A", " ", "m", "o", " ", "<BS>", "<CR>", "x", "<Esc>", "u"],
        ),
        (
            paragraphs(),
            "/wraps<CR>",
            vec!["x", "x", "u", "p", "o", "a", "<Esc>", "J", "~", "rX"],
        ),
        (
            paragraphs(),
            "/\\=than<CR>i",
            vec!["a", " ", "b", "<Left>", "c", "<Esc>", "x", "u", "rZ", "p"],
        ),
    ];
    let places = [
        (
            "Words here#footnote[note text] and more.\n",
            "/\\=text]<CR>i",
        ),
        (
            "#figure(rect(width: 20pt), caption: [A cap])\n",
            "/\\=cap<CR>ea",
        ),
        ("= Head line\n\nBody.\n", "/\\=line<CR>ea"),
        ("- item one\n- item two\n\nAfter.\n", "/\\=two<CR>ea"),
        ("#table(columns: 2, [a], [b])\n", "/\\=b<CR>a"),
        ("Let $x + y$ be.\n", "/\\=y<CR>a"),
        ("One.\n\nTwo.\n", "/\\=One<CR>o"),
        ("= Head <lab>\n\nSome words of text.\n", "/\\=words<CR>ea"),
        (
            "Page one.\n#pagebreak()\nPage two words.\n",
            "/\\=two words<CR>ea",
        ),
    ];
    for (body, setup) in places {
        cases.push((format!("{PAGE}{body}"), setup, typing.clone()));
    }
    cases
}

#[test]
fn every_frame_of_a_key_has_the_cursor_and_the_view_stays() {
    // In every kind of place, typing and in normal mode: the cursor is in
    // each frame; a compile of the text before the key, landing late,
    // does not take it back to where it was; the view does not move.
    // So too zoomed in, with the interface larger.
    let zoomed = |viewer: &mut Viewer| {
        viewer.set_ui_scale(1.5);
        viewer.zoom_by(1.6, (0.0, 0.0));
    };
    let adjustments: [&dyn Fn(&mut Viewer); 2] = [&|_| {}, &zoomed];
    let mut wrong = Vec::new();
    for ((text, setup, keys), adjust) in cases()
        .into_iter()
        .flat_map(|case| adjustments.map(|adjust| (case.clone(), adjust)))
    {
        let all = frames_of_keys_in(&text, &format!("gsgs{setup}"), &keys, adjust);
        let top = all[0].1[0].top;
        for (key, frames) in &all[1..] {
            let shown: Vec<Vec<[i32; 4]>> = frames.iter().map(cursors).collect();
            let at = |of: &[[i32; 4]]| of.first().map(|quad| quad[0]);
            let whole = shown.iter().all(|quads| quads.len() == 1);
            let kept = at(&shown[2]) == at(&shown[0]);
            let still = frames.iter().all(|frame| (frame.top - top).abs() < 0.5);
            if !(whole && kept && still) {
                let tops: Vec<f64> = frames.iter().map(|frame| frame.top).collect();
                wrong.push(format!("{setup} then {key:?}: {shown:?} at {tops:?}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn keys_of_normal_mode_leave_a_block_cursor_before_the_pages_come() {
    // `x`, `u`, `r`, `p`: the page shows what they did at once (worked
    // out); the cursor on it is the block of normal mode, not the bar of
    // typing, nor gone, and about where the pages then put it.
    let all = frames_of_keys(&paragraphs(), "gsgs/wraps<CR>", &["x", "u", "rX", "p"]);
    for (key, frames) in &all[1..] {
        let block = named(&frames[0], &["cursor"]);
        let typeset = named(&frames[3], &["cursor"]);
        assert_eq!(block.len(), 1, "after {key}: {:?}", frames[0]);
        assert_eq!(named(&frames[0], &["bar"]), Vec::<[i32; 4]>::new());
        assert!(
            (block[0][0] - typeset[0][0]).abs() <= 8,
            "after {key}: {block:?}, typeset {typeset:?}"
        );
    }
}

#[test]
fn a_compile_of_an_older_text_leaves_what_was_typed_since_on_the_page() {
    // Typing pauses and the text is being typeset; two more letters are
    // typed and worked into the page. Then the older pages come: the
    // letters stay on the page, the cursor behind them, and the numbers
    // of the lines beside them.
    let (mut viewer, directory) = editing(&paragraphs());
    press(&mut viewer, "gsgs/Second<CR>A wor");
    compile(&mut viewer);
    // The numbers stand left of the text.
    let numbers = |frame: &Frame| {
        let left = |quad: &&(String, [i32; 4])| quad.1[0] < 130;
        frame.quads.iter().filter(left).cloned().collect::<Vec<_>>()
    };
    assert_ne!(numbers(&frame(&mut viewer)).len(), 0);
    press(&mut viewer, "d");
    let before = viewer.snapshot();
    press(&mut viewer, "s");
    let typed = frame(&mut viewer);
    assert!(viewer.is_predicted(), "{:?}", viewer.declined);
    land(&mut viewer, before);
    let late = frame(&mut viewer);
    assert!(
        viewer.is_predicted() || viewer.document_revision == Some(viewer.revision),
        "the letters typed since are gone from the page"
    );
    assert_eq!(cursors(&late), cursors(&typed));
    assert_eq!(numbers(&late), numbers(&typed));
    compile(&mut viewer);
    let compiled = frame(&mut viewer);
    assert!((cursors(&compiled)[0][0] - cursors(&typed)[0][0]).abs() <= 2);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_blank_typed_where_nothing_is_typeset_by_itself_leaves_the_cursor() {
    // On a page beside a page break nothing is typeset by itself; a blank
    // typed beside another is worked into no page either. Until the pages
    // come the cursor stands where it was typed, not gone.
    let text = format!("{PAGE}Page one.\n#pagebreak()\nPage two words.\n");
    let all = frames_of_keys(&text, "gsgs/\\=two words<CR>ea", &[" "]);
    for frame in &all[1].1 {
        assert_eq!(cursors(frame).len(), 1, "{frame:?}");
    }
}

#[test]
fn a_blank_typed_in_a_line_moves_the_cursor_by_a_blank_once() {
    let all = frames_of_keys(&paragraphs(), "gsgs/\\=than<CR>i", &["a", " "]);
    let (typed, typeset) = (cursors(&all[2].1[0]), cursors(&all[2].1[3]));
    assert!(
        (typed[0][0] - typeset[0][0]).abs() <= 2,
        "typed {typed:?}, typeset {typeset:?}"
    );
}

#[test]
fn the_tags_of_notes_stay_while_a_word_is_typed_elsewhere() {
    let setup = "gsgs/\\=Second<CR>:note look here<CR>/\\=paragraph<CR>ea";
    let all = frames_of_keys(&paragraphs(), setup, &["a", "b"]);
    // What is drawn at the note's line, the second paragraph: its ground,
    // its line and its tag.
    let note_line = |frame: &Frame| -> Vec<(String, [i32; 4])> {
        let mut quads: Vec<_> = frame
            .quads
            .iter()
            .filter(|(name, quad)| quad[1] >= 230 && quad[0] > 130 && name != "bar")
            .cloned()
            .collect();
        quads.sort_by_key(|quad| quad.1);
        quads
    };
    let settled = note_line(&all[0].1[0]);
    assert_eq!(settled.len(), 3, "{settled:?}");
    for (key, frames) in &all[1..] {
        for frame in frames {
            assert_eq!(note_line(frame), settled, "after {key}");
        }
    }
}

#[test]
fn a_blank_at_the_end_of_an_item_keeps_its_frame() {
    let text = format!("{PAGE}- item one\n- item two\n\nAfter.\n");
    let all = frames_of_keys(&text, "gsgs/\\=two<CR>ea", &[" ", "x"]);
    for (key, frames) in &all {
        for frame in frames {
            assert_eq!(named(frame, &["focus"]).len(), 4, "after {key}: {frame:?}");
        }
    }
}

#[test]
fn the_tags_in_the_margin_stay_where_they_are_as_a_line_grows() {
    // A label's tag stands right of the text of the page; a line typed
    // longer than all others does not push it, nor all other tags.
    let text = format!("{PAGE}= Head <lab>\n\nSome words.\n");
    let all = frames_of_keys(&text, "gsgs/\\=words<CR>ea", &[" ", "a", "b", "c", "d"]);
    let tags = |frame: &Frame| -> Vec<[i32; 4]> {
        let right = |quad: &&(String, [i32; 4])| quad.1[0] > 400 && quad.0 != "bar";
        frame
            .quads
            .iter()
            .filter(right)
            .map(|quad| quad.1)
            .collect()
    };
    let settled = tags(&all[0].1[0]);
    assert_eq!(settled.len(), 1, "{:?}", all[0].1[0]);
    for (key, frames) in &all[1..] {
        for frame in frames {
            assert_eq!(tags(frame), settled, "after {key}");
        }
    }
}

#[test]
fn the_status_line_does_not_change_while_the_pages_show_what_was_typed() {
    // "compiling" came and went with every key, and moved all beside it.
    let (mut viewer, directory) = editing(&paragraphs());
    press(&mut viewer, "gsgs/Second<CR>A wor");
    compile(&mut viewer);
    press(&mut viewer, "d");
    assert!(viewer.is_predicted() && !viewer.is_compiled());
    let typed = status(&mut viewer);
    compile(&mut viewer);
    assert_eq!(status(&mut viewer), typed);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_word_for_an_empty_place_stands_behind_it_where_there_is_room() {
    // Above an exponent its word stands over the line above; behind it,
    // at the end of what is typed, there is room.
    let text = "#set page(width: 360pt, height: 240pt)\n\
                Some text on the line above the equation, filling the line well \
                enough to wrap. Let $y^k$\n\nAnd text on the line below it.\n";
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "gsgs/\\=k<CR>s");
    compile(&mut viewer);
    let drawn = frame(&mut viewer);
    let bar = named(&drawn, &["bar"]);
    let known = ["bar", "cursor", "selection", "focus"];
    let words: Vec<[i32; 4]> = drawn
        .quads
        .iter()
        .filter(|(name, quad)| !known.contains(&name.as_str()) && quad[0] > 130 && quad[3] >= 8)
        .map(|(_, quad)| *quad)
        .collect();
    assert_eq!((bar.len(), words.len()), (1, 1), "{drawn:?}");
    assert!(
        words[0][0] > bar[0][0],
        "the word {words:?}, the cursor {bar:?}"
    );
    let _ = std::fs::remove_dir_all(directory);
}

/// The words drawn for empty places in `text` once `keys` are typed:
/// what is drawn that is no bar, cursor, selection or veil, right of the
/// margin.
fn place_words(text: &str, keys: &str) -> Vec<[i32; 4]> {
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, keys);
    compile(&mut viewer);
    let drawn = frame(&mut viewer);
    let known = ["bar", "cursor", "selection", "focus"];
    let words = drawn
        .quads
        .iter()
        .filter(|(name, quad)| !known.contains(&name.as_str()) && quad[0] > 130 && quad[3] >= 4)
        .map(|(_, quad)| *quad)
        .collect();
    let _ = std::fs::remove_dir_all(directory);
    words
}

#[test]
fn the_word_for_an_empty_place_with_no_room_is_smaller_where_it_stands() {
    // Room behind the exponent: the word as it always is.
    let roomy = place_words(
        "#set page(width: 360pt, height: 240pt)\n\
         Some text on the line above the equation, filling the line well \
         enough to wrap. Let $y^k$\n\nAnd text on the line below it.\n",
        "gsgs/\\=k<CR>s",
    );
    // Text behind it, and lines above and below: smaller, over the text.
    let tight = place_words(
        "#set page(width: 360pt, height: 240pt)\n\
         Some text on the line above the equation, filling the line well \
         enough to wrap. Let $y^k$ and the words go on behind it, well \
         past the end of this line and onto the next one as well, and \
         further still.\n",
        "gsgs/\\=k<CR>s",
    );
    assert_eq!((roomy.len(), tight.len()), (1, 1), "{roomy:?} {tight:?}");
    assert!(
        tight[0][3] < roomy[0][3],
        "with no room the word is smaller: {tight:?} against {roomy:?}"
    );
}

/// Prints the frames of `keys` after `setup` on `text`: what each frame
/// drew that the one before did not, and what it did not draw that the
/// one before did.
fn explore(text: &str, setup: &str, keys: &[&str]) {
    let all = frames_of_keys(text, setup, keys);
    let mut last = all[0].1[0].clone();
    println!("== {setup:?}: top {} cursor {:?}", last.top, cursors(&last));
    let when = ["key", "next", "late", "compiled", "again"];
    for (key, frames) in &all[1..] {
        let shown: Vec<String> = when
            .iter()
            .zip(frames)
            .map(|(when, f)| format!("{when}: top {:.0} {:?}", f.top, cursors(f)))
            .collect();
        println!("{key:?}: {}", shown.join(" | "));
        for (when, f) in when.iter().zip(frames) {
            let gone: Vec<_> = last.quads.iter().filter(|q| !f.quads.contains(q)).collect();
            let came: Vec<_> = f.quads.iter().filter(|q| !last.quads.contains(q)).collect();
            if !gone.is_empty() || !came.is_empty() {
                println!("    {when}: gone {gone:?} came {came:?}");
            }
            last = f.clone();
        }
    }
}

#[test]
fn the_view_stays_when_the_pages_come_with_the_cursor_near_its_foot() {
    // The cursor near the foot of the window, where keys that move it
    // would take the view along; keys that do not move it change the
    // text, and their pages come. The view stays.
    let text = (1..40).fold(String::new(), |mut text, n| {
        let _ = writeln!(text, "Paragraph {n} has a few words in it.\n");
        text
    });
    let (mut viewer, directory) = editing(&text);
    press(&mut viewer, "gsgs/Paragraph 20 <CR>");
    compile(&mut viewer);
    let at = cursors(&frame(&mut viewer))[0];
    let height = f64::from(viewer.view.height);
    viewer.view.top += (f64::from(at[1]) - 0.93 * height) / viewer.view.zoom;
    let top = viewer.view.top;
    for key in ["x", "x", "rX"] {
        press(&mut viewer, key);
        assert!((frame(&mut viewer).top - top).abs() < 0.5, "after {key}");
        compile(&mut viewer);
        assert!(
            (frame(&mut viewer).top - top).abs() < 0.5,
            "after {key}, typeset"
        );
    }
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
#[ignore = "prints what each frame drew, to look for what comes and goes"]
fn explore_frames() {
    for (text, setup, keys) in cases() {
        explore(&text, &format!("gsgs{setup}"), &keys);
    }
}
