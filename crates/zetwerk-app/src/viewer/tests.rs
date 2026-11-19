//! Tests of the viewer as a whole: windows, pages, keys and compiles.
//! The tests of one part of it are in the modules below, one file each
//! in `tests/`; the helpers here (`compile`, `press`) are theirs too.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use zetwerk_typst::FontSet;

use super::*;
use crate::keys;

mod authors;
mod changes;
mod comfort;
mod compose;
mod cursor;
mod equations;
mod errors;
mod files;
mod frames;
mod hidden;
#[path = "tests/keys.rs"]
mod keyboard;
mod latex;
mod left_behind;
mod names;
mod packages;
mod pages;
mod panels;
mod parts;
mod paste;
mod present;
mod report;
mod review;
mod settings;
mod speed;
mod tutor;
mod typing;
mod typist;
mod vim;
mod waits;

/// A viewer in an 800×620 window showing `pages` A4 pages.
fn viewer(pages: usize) -> Viewer {
    let text = vec!["Page"; pages].join("\n#pagebreak()\n");
    let env = Environment::new(FontSet::Embedded);
    let project = Project::in_memory(Arc::clone(&env), PathBuf::from("/"), "main.typ", text)
        .unwrap_or_else(|| panic!("valid main file name"));
    let mut viewer =
        Viewer::new(env, project, (800, 620), 1.0).unwrap_or_else(|| panic!("viewer sets up"));
    compile(&mut viewer);
    viewer
}

/// Compiles the viewer's current revision on this thread.
pub(super) fn compile(viewer: &mut Viewer) {
    let (revision, world) = viewer.snapshot();
    let compiled = zetwerk_typst::compile(&world);
    viewer.compiled(CompileOutput {
        revision,
        world: Arc::new(world),
        compiled,
        duration: Duration::from_millis(3),
    });
}

pub(super) fn press(viewer: &mut Viewer, notation: &str) {
    for key in keys::parse(notation).unwrap_or_else(|err| panic!("{err}")) {
        viewer.key(key);
    }
}

#[test]
fn a_new_document_fits_the_window_width() {
    let viewer = viewer(3);
    let state = viewer.state();
    assert_eq!((state.pages, state.page), (3, 1));
    assert_eq!(state.compiled_revision, Some(0));
    assert!(viewer.is_compiled());
    // 800 px for 595.28 pt plus a 10 pt gap on each side.
    assert!(
        (state.zoom - 800.0 / 615.28).abs() < 1e-3,
        "zoom is {}",
        state.zoom
    );
}

#[test]
fn keys_scroll_through_the_document() {
    let mut viewer = viewer(3);
    press(&mut viewer, "G");
    assert_eq!(viewer.state().page, 3);
    press(&mut viewer, "gg");
    assert_eq!(viewer.state().page, 1);
    let top = viewer.view.top;
    press(&mut viewer, "j");
    assert!(viewer.view.top > top);
    press(&mut viewer, "k");
    assert_eq!(viewer.view.top, top);
    press(&mut viewer, "<C-f><C-f>");
    assert_eq!(viewer.state().page, 2);
    // A lone `g` does nothing, and does not turn the next `g` into `gg`
    // after another key came in between.
    press(&mut viewer, "gjg");
    assert_eq!(viewer.state().page, 2);
}

#[test]
fn zoom_keys_and_fit() {
    let mut viewer = viewer(1);
    let fitted = viewer.state().zoom;
    press(&mut viewer, "+");
    assert!((viewer.state().zoom - fitted * 1.25).abs() < 1e-9);
    // Once the user has chosen a zoom, resizing keeps it.
    viewer.resize((1200, 620), 1.0);
    assert!((viewer.state().zoom - fitted * 1.25).abs() < 1e-9);
    press(&mut viewer, "=");
    assert!((viewer.state().zoom - 1200.0 / 615.28).abs() < 1e-3);
    viewer.resize((800, 620), 1.0);
    assert!((viewer.state().zoom - fitted).abs() < 1e-9);
}

#[test]
fn command_line() {
    let mut viewer = viewer(3);
    press(&mut viewer, ":3");
    assert_eq!(viewer.state().mode, Mode::Command("3".to_owned()));
    press(&mut viewer, "<CR>");
    assert_eq!(
        (viewer.state().mode, viewer.state().page),
        (Mode::Normal, 3)
    );

    press(&mut viewer, ":perf<CR>");
    assert!(viewer.state().performance_panel);
    press(&mut viewer, ":nonsense<CR>");
    let message = viewer.message.as_ref().map(|message| message.text.as_str());
    assert_eq!(message, Some("not a command: nonsense"));
    // Backspace on an empty command line closes it; Escape always does.
    press(&mut viewer, ":a<BS><BS>");
    assert_eq!(viewer.state().mode, Mode::Normal);
    press(&mut viewer, ":q<Esc>");
    assert!(!viewer.wants_quit());
    press(&mut viewer, ":q<CR>");
    assert!(viewer.wants_quit());
}

#[test]
fn a_scene_asks_for_missing_tiles_and_draws_held_ones() {
    let mut viewer = viewer(2);
    let nothing = viewer.scene(&|_| false);
    // 800 px wide: two columns of 512 px tiles, two rows in view.
    assert_eq!(nothing.wanted.len(), 4);
    // Backdrop, paper and status line have images to upload.
    assert_eq!(nothing.images.len(), 3);

    let held: HashSet<TextureId> = nothing
        .wanted
        .iter()
        .map(|tile| texture(tile.key))
        .chain(nothing.images.iter().map(|image| image.id))
        .collect();
    let complete = viewer.scene(&|id| held.contains(&id));
    assert!(complete.wanted.is_empty() && complete.images.is_empty());
    // Backdrop, paper, four tiles, status line.
    assert_eq!(complete.quads.len(), 7);
    let last = complete.quads.last().unwrap_or_else(|| panic!("has quads"));
    assert_eq!(
        (last.x, last.y, last.width, last.height),
        (0.0, 600.0, 800.0, 20.0)
    );
}

#[test]
fn old_tiles_stand_in_after_a_zoom_step() {
    let mut viewer = viewer(1);
    let first = viewer.scene(&|_| false);
    let held: HashSet<TextureId> = first.wanted.iter().map(|t| texture(t.key)).collect();
    // Draw once with everything held, so the viewer knows what it showed.
    let _ = viewer.scene(&|id| held.contains(&id));

    press(&mut viewer, "+");
    let zoomed = viewer.scene(&|id| held.contains(&id));
    assert!(!zoomed.wanted.is_empty(), "the new zoom needs new tiles");
    let stand_ins: Vec<&Quad> = zoomed
        .quads
        .iter()
        .filter(|quad| held.contains(&quad.texture))
        .collect();
    // One per tile of the old view; blank tiles share one texture.
    assert_eq!(stand_ins.len(), first.wanted.len());
    // Stretched by the zoom step: a 512 px tile is drawn 640 px wide.
    assert!(
        stand_ins
            .iter()
            .any(|quad| (quad.width - 640.0).abs() < 0.01)
    );
}

#[test]
fn errors_keep_the_last_good_pages() {
    let mut viewer = viewer(2);
    viewer.project.set_main_text("#broken(");
    viewer.revision += 1;
    assert!(!viewer.is_compiled());
    compile(&mut viewer);
    let state = viewer.state();
    assert_eq!(state.pages, 2, "the old pages stay on screen");
    assert_eq!(state.compiled_revision, Some(1));
    assert_eq!(state.problems.len(), 1);
    assert!(
        state.problems[0].starts_with("main.typ:1:"),
        "{:?}",
        state.problems
    );
}

#[test]
fn an_older_compile_never_replaces_a_newer_one() {
    let mut viewer = viewer(1);
    let (_, old_world) = viewer.snapshot();
    viewer.project.set_main_text("A\n#pagebreak()\nB");
    viewer.revision += 1;
    compile(&mut viewer);
    assert_eq!(viewer.state().pages, 2);

    let compiled = zetwerk_typst::compile(&old_world);
    viewer.compiled(CompileOutput {
        revision: 0,
        world: Arc::new(old_world),
        compiled,
        duration: Duration::ZERO,
    });
    assert_eq!(viewer.state().pages, 2);
    assert_eq!(viewer.state().compiled_revision, Some(1));
}

/// A fresh directory holding `main.typ` with `text`.
pub(super) fn directory(text: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let directory = std::env::temp_dir().join(format!(
        "zetwerk-viewer-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("main.typ"), text).unwrap();
    directory
}

/// A viewer in a 1000×620 window editing `main.typ` in `directory`,
/// and what opening the file found.
pub(super) fn open(directory: &Path) -> (Viewer, Opened) {
    let path = directory.join("main.typ");
    let env = Environment::new(FontSet::Embedded);
    let project = Project::open(Arc::clone(&env), &path).unwrap();
    let (editor, opened) = Editor::open(&path, &directory.join("state"), project.main()).unwrap();
    let mut viewer = Viewer::new(env, project, (1000, 620), 1.0)
        .unwrap_or_else(|| panic!("viewer sets up"))
        .editing(editor, &opened);
    viewer.state_dir = Some(directory.join("state"));
    compile(&mut viewer);
    (viewer, opened)
}

pub(super) fn editing(text: &str) -> (Viewer, PathBuf) {
    let directory = directory(text);
    (open(&directory).0, directory)
}

#[test]
fn typing_changes_the_text_and_wants_a_compile() {
    let (mut viewer, directory) = editing("world\n");
    assert_eq!(viewer.panes(), Panes::Split);
    assert_eq!(viewer.take_edited(), None);

    press(&mut viewer, "iHello ");
    assert_eq!(viewer.text(), Some("Hello world\n"));
    // So short a text tells too little about its lines for the page
    // to be worked out; the paragraph is typeset by itself instead,
    // and all of it once typing pauses.
    assert_eq!(viewer.take_edited(), Some(Edited::Typing));
    assert!(!viewer.is_compiled());
    let editing = viewer.state().editing.unwrap();
    assert_eq!(editing.mode, editor::Mode::Insert);
    assert_eq!((editing.line, editing.column), (1, 7));
    assert!(editing.modified);

    // Leaving insert mode with changes not yet compiled asks again.
    press(&mut viewer, "<Esc>");
    assert_eq!(viewer.take_edited(), Some(Edited::LeftInsert));
    compile(&mut viewer);
    assert!(viewer.is_compiled());
    press(&mut viewer, "<Esc>");
    assert_eq!(viewer.take_edited(), None);

    press(&mut viewer, "dd");
    assert_eq!(viewer.text(), Some(""));
    assert_eq!(viewer.take_edited(), Some(Edited::Command));
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some("Hello world\n"));
    // Nothing has touched the file.
    let file = std::fs::read_to_string(directory.join("main.typ")).unwrap();
    assert_eq!(file, "world\n");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn writing_and_quitting() {
    let (mut viewer, directory) = editing("one\n");
    let file = || std::fs::read_to_string(directory.join("main.typ")).unwrap();
    press(&mut viewer, "Atwo<Esc>");
    press(&mut viewer, ":q<CR>");
    assert!(!viewer.wants_quit(), "unsaved changes keep the window");
    press(&mut viewer, ":w<CR>");
    assert_eq!(file(), "onetwo\n");
    assert!(!viewer.state().editing.unwrap().modified);
    press(&mut viewer, ":q<CR>");
    assert!(viewer.wants_quit());

    // `ZZ` saves what is unsaved and leaves.
    let (mut viewer, other) = editing("one\n");
    press(&mut viewer, "xZZ");
    assert!(viewer.wants_quit());
    assert_eq!(
        std::fs::read_to_string(other.join("main.typ")).unwrap(),
        "ne\n"
    );
    for directory in [directory, other] {
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn unsaved_changes_come_back() {
    let (mut viewer, directory) = editing("one\n");
    // The window is closed, or the machine goes down: nobody said
    // that the changes were to go.
    press(&mut viewer, "otwo<Esc>");
    drop(viewer);
    assert_eq!(
        std::fs::read_to_string(directory.join("main.typ")).unwrap(),
        "one\n"
    );
    let (mut viewer, opened) = open(&directory);
    assert_eq!(opened, Opened::Restored);
    assert_eq!(viewer.text(), Some("one\ntwo\n"));
    // And can still be undone.
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some("one\n"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_change_on_disk_never_costs_unsaved_text() {
    let (mut viewer, directory) = editing("one\n");
    let path = directory.join("main.typ");
    // Nothing unsaved: the new text is taken over, and can be undone.
    std::fs::write(&path, "from outside\n").unwrap();
    assert!(viewer.file_changed(&path));
    assert_eq!(viewer.text(), Some("from outside\n"));
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some("one\n"));
    press(&mut viewer, "<C-r>");

    // Something unsaved: both sides stay as they are.
    press(&mut viewer, "x");
    std::fs::write(&path, "again from outside\n").unwrap();
    assert!(!viewer.file_changed(&path));
    assert_eq!(viewer.text(), Some("rom outside\n"));
    press(&mut viewer, ":w<CR>");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "again from outside\n"
    );
    press(&mut viewer, ":w!<CR>");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "rom outside\n");
    // Its own write is not a change from outside.
    assert!(!viewer.file_changed(&path));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn searching_moves_the_cursor() {
    let (mut viewer, directory) = editing("one two\nthree two\n");
    press(&mut viewer, "/two");
    let editing = viewer.state().editing.unwrap();
    assert_eq!(
        (editing.prompt, editing.command.as_deref()),
        ('/', Some("two"))
    );
    press(&mut viewer, "<CR>");
    let place = |viewer: &Viewer| {
        let editing = viewer.state().editing.unwrap();
        (editing.line, editing.column)
    };
    assert_eq!(place(&viewer), (1, 5));
    press(&mut viewer, "n");
    assert_eq!(place(&viewer), (2, 7));
    press(&mut viewer, "N");
    assert_eq!(place(&viewer), (1, 5));
    press(&mut viewer, "cwdrei<Esc>n.");
    assert_eq!(viewer.text(), Some("one drei\nthree drei\n"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_view_can_be_moved_and_moved_in() {
    let lines = "a line\n".repeat(200);
    let (mut viewer, directory) = editing(&lines);
    let held: HashSet<TextureId> = HashSet::new();
    let line = |viewer: &Viewer| viewer.state().editing.unwrap().line;
    let draw = |viewer: &mut Viewer| {
        viewer.scene(&|id| held.contains(&id));
    };
    draw(&mut viewer);
    press(&mut viewer, "L");
    let bottom = line(&viewer);
    assert!(bottom > 20, "the pane shows {bottom} lines");
    press(&mut viewer, "H");
    assert_eq!(line(&viewer), 1);
    press(&mut viewer, "M");
    assert_eq!(line(&viewer), 1 + (bottom - 1) / 2);
    // The cursor's line to the top; the lines kept in view around the
    // cursor stay above it.
    press(&mut viewer, "Lzt");
    draw(&mut viewer);
    press(&mut viewer, "H");
    assert_eq!(line(&viewer), bottom - SCROLL_MARGIN);
    // A page down and back.
    press(&mut viewer, "gg");
    draw(&mut viewer);
    press(&mut viewer, "<C-f>");
    assert_eq!(line(&viewer), bottom - 1);
    press(&mut viewer, "<C-b>");
    assert_eq!(line(&viewer), 1);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_cursor_is_shown_on_the_page() {
    let (mut viewer, directory) = editing("Hello *there* world.\n");
    let held: HashSet<TextureId> = HashSet::new();
    let count = |viewer: &mut Viewer, rgba: [u8; 4]| {
        let id = TextureId(hash128(&("tint", rgba)));
        let scene = viewer.scene(&|id| held.contains(&id));
        scene.quads.iter().filter(|quad| quad.texture == id).count()
    };
    assert_eq!(count(&mut viewer, PAGE_CURSOR), 1);
    assert_eq!(count(&mut viewer, PAGE_SELECTION), 0);
    // On markup that sets no glyph of its own there is no cursor to
    // show; on the text after it there is again.
    press(&mut viewer, "f*");
    assert_eq!(count(&mut viewer, PAGE_CURSOR), 0);
    press(&mut viewer, "l");
    assert_eq!(count(&mut viewer, PAGE_CURSOR), 1);
    // A selection over two words and the markup between them.
    press(&mut viewer, "0vee");
    assert!(count(&mut viewer, PAGE_SELECTION) >= 1);
    press(&mut viewer, "<Esc>");
    // After an edit the page is that of the text before it: the cursor
    // is shown where its text stands on that.
    press(&mut viewer, "0x");
    assert!(!viewer.is_compiled());
    assert_eq!(count(&mut viewer, PAGE_CURSOR), 1);
    compile(&mut viewer);
    assert_eq!(count(&mut viewer, PAGE_CURSOR), 1);
    // In insert mode the cursor is a bar.
    press(&mut viewer, "i");
    assert_eq!(count(&mut viewer, PAGE_BAR), 1);
    // Text that does not compile leaves the old page, with the cursor
    // on it as on any page that is behind.
    press(&mut viewer, "<Esc>f*x");
    compile(&mut viewer);
    assert!(viewer.is_compiled() && !viewer.state().problems.is_empty());
    assert_eq!(count(&mut viewer, PAGE_CURSOR), 1);
    // Two changes in two places: where things are is anyone's guess.
    press(&mut viewer, "$x");
    assert_eq!(count(&mut viewer, PAGE_CURSOR), 0);
    let _ = std::fs::remove_dir_all(directory);
}

/// The quads of one colour laid over the pages.
pub(super) fn tints(viewer: &mut Viewer, rgba: [u8; 4]) -> Vec<Quad> {
    let id = TextureId(hash128(&("tint", rgba)));
    let scene = viewer.scene(&|_| false);
    let of_colour = |quad: &&Quad| quad.texture == id;
    scene.quads.iter().filter(of_colour).copied().collect()
}

#[test]
fn what_a_reference_shows_is_one_thing_under_the_cursor() {
    let text = "#set heading(numbering: \"1.\")\n\
                = Start <start>\n\
                As @start says, more or less.\n";
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "/As<CR>");
    let letter = tints(&mut viewer, PAGE_CURSOR);
    assert_eq!(letter.len(), 1);
    // On the reference: all of "Section 1", wherever in it.
    press(&mut viewer, "/@<CR>");
    let whole = tints(&mut viewer, PAGE_CURSOR);
    assert_eq!(whole.len(), 1);
    assert!(whole[0].width > 4.0 * letter[0].width, "{whole:?}");
    press(&mut viewer, "lll");
    assert_eq!(tints(&mut viewer, PAGE_CURSOR), whole);
    // And past it, a letter again.
    press(&mut viewer, "w");
    let after = tints(&mut viewer, PAGE_CURSOR);
    assert!(after[0].x > whole[0].x + whole[0].width && after[0].width < whole[0].width);
    // A click on it goes to the reference.
    let (x, y) = (f64::from(whole[0].x) + 3.0, f64::from(whole[0].y) + 3.0);
    viewer.click((x, y));
    let cursor = viewer.editor.as_ref().unwrap().cursor();
    assert_eq!(&text[cursor..cursor + 6], "@start");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_cursor_moves_on_a_page_that_is_behind() {
    let text = "#set page(width: 300pt, height: 300pt)\n\
                The quick brown fox jumps over the lazy dog and runs on \
                through the wood until the line is full, and then some \
                more of it follows here to make a few lines of text, \
                the last of which is short.\n";
    let (mut viewer, directory) = editing(text);
    let cursor = |viewer: &Viewer| viewer.editor.as_ref().unwrap().cursor();
    // A star is no word of a line; with typesetting in parts off,
    // there is nothing to show without compiling.
    viewer.in_parts = false;
    press(&mut viewer, "gsgs/quick<CR>i*<Esc>");
    assert!(!viewer.is_compiled() && !viewer.is_predicted());
    // The star itself is on no page.
    assert_eq!(tints(&mut viewer, PAGE_CURSOR).len(), 0);
    press(&mut viewer, "l");
    let before = tints(&mut viewer, PAGE_CURSOR);
    assert_eq!(before.len(), 1, "the cursor is on the page as it was");
    let start = cursor(&viewer);
    press(&mut viewer, "j");
    assert!(cursor(&viewer) > start + 20, "down a line of the page");
    assert_eq!(viewer.editor.as_ref().unwrap().position().0, 2);
    let below = tints(&mut viewer, PAGE_CURSOR);
    assert!(below[0].y > before[0].y + 5.0);
    assert!(
        (below[0].x - before[0].x).abs() < 12.0,
        "{before:?} {below:?}"
    );
    press(&mut viewer, "k");
    assert_eq!(cursor(&viewer), start);
    // A click finds the text that was under it before the star.
    let (x, y) = (f64::from(below[0].x) + 1.0, f64::from(below[0].y) + 3.0);
    press(&mut viewer, "j");
    let below = cursor(&viewer);
    press(&mut viewer, "gg");
    viewer.click((x, y));
    assert_eq!(cursor(&viewer), below);
    compile(&mut viewer);
    assert_eq!(tints(&mut viewer, PAGE_CURSOR).len(), 1);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn what_is_typed_on_the_page_is_what_is_read() {
    let text = "#set page(width: 300pt, height: 300pt)\nSome words.\n\nMore.\n";
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "/words<CR>gsgs");
    press(&mut viewer, "A Use a_b * c [1] <lt>x> of me@x.de");
    let typed = viewer.text().unwrap();
    assert!(
        typed.contains("Use a\\_b \\* c \\[1\\] \\<x> of me\\@x.de"),
        "{typed:?}"
    );
    compile(&mut viewer);
    assert_eq!(viewer.document_revision, Some(viewer.revision));
    // Strong and emphasised text is begun and left by a key.
    press(
        &mut viewer,
        " and <C-b>bold<C-b> or <C-i>so<C-i>, b<C-b>ut<C-b>.",
    );
    let typed = viewer.text().unwrap();
    assert!(
        typed.contains(" and *bold* or _so_, b#strong[ut]."),
        "{typed:?}"
    );
    compile(&mut viewer);
    assert_eq!(viewer.document_revision, Some(viewer.revision));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn deleting_in_an_equation_leaves_an_equation() {
    let text = "#set page(width: 300pt, height: 300pt)\nBefore.\n\n\
                $ y_i = lambda x_i + (1 - lambda) x_(i+1) $\n\nAfter.\n";
    let (mut viewer, directory) = editing(text);
    let equation = |viewer: &Viewer| {
        let text = viewer.text().unwrap_or_default().to_owned();
        let from = text.find('$').unwrap_or(0);
        let to = text.rfind('$').map_or(text.len(), |at| at + 1);
        text[from..to].to_owned()
    };
    // Backwards through all of it while typing: what is raised or
    // lowered goes, and with the last of it the mark that did it.
    press(&mut viewer, "/i+1<CR>gsgsA");
    let mut seen = Vec::new();
    for _ in 0..10 {
        press(&mut viewer, "<BS>");
        compile(&mut viewer);
        seen.push(equation(&viewer));
        assert_eq!(
            viewer.document_revision,
            Some(viewer.revision),
            "no equation: {seen:?}"
        );
    }
    assert!(
        seen.contains(&"$ y_i = lambda x_i + (1 - lambda) x_() $".to_owned()),
        "{seen:?}"
    );
    assert!(
        seen.contains(&"$ y_i = lambda x_i + (1 - lambda) x $".to_owned()),
        "{seen:?}"
    );
    // A symbol written with several letters goes as the one thing
    // it shows as.
    assert_eq!(
        equation(&viewer).trim_end_matches([' ', '$']),
        "$ y_i = lambda x_i +",
        "{seen:?}"
    );
    // And with `x` on the page: nothing is left of what it took.
    press(&mut viewer, "<Esc>u");
    compile(&mut viewer);
    press(&mut viewer, "/i+1<CR>");
    press(&mut viewer, "xxx");
    compile(&mut viewer);
    assert_eq!(equation(&viewer), "$ y_i = lambda x_i + (1 - lambda) x $");
    assert_eq!(viewer.document_revision, Some(viewer.revision));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn an_equation_that_stands_by_itself_stays_so_while_it_is_typed() {
    let text = "#set page(width: 300pt, height: 300pt)\n\
                #set math.equation(numbering: \"(1)\")\n\
                Before.\n\n$ a + b $\n\nAfter.\n";
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "gsgs/\\=+ b<CR>");
    compile(&mut viewer);
    // The line of the equation is that of its number, the glyph furthest
    // right; the equation begins with the leftmost glyph in it.
    let furthest = |places: &[(i64, i64)]| places.iter().copied().max_by_key(|place| place.0);
    let number = |places: &[(i64, i64)]| furthest(places).map(|place| place.0);
    let begins = |places: &[(i64, i64)]| {
        let (_, line) = furthest(places)?;
        places
            .iter()
            .filter(|place| (place.1 - line).abs() < 40)
            .map(|place| place.0)
            .min()
    };
    let before = glyph_places(&viewer, 0);
    // Typed into, it begins where it began, and its number stands where
    // it stood; so too with the box for what is to be raised.
    press(&mut viewer, "la + 2");
    let typed = glyph_places(&viewer, 0);
    assert_eq!(begins(&typed), begins(&before));
    assert_eq!(number(&typed), number(&before));
    press(&mut viewer, "^");
    assert_eq!(
        viewer.document_revision,
        Some(viewer.revision),
        "{:?}",
        viewer.declined
    );
    let raised = glyph_places(&viewer, 0);
    assert_eq!(raised.len(), typed.len() + 1, "the box for what is to come");
    assert_eq!(begins(&raised), begins(&before));
    assert_eq!(
        number(&raised),
        number(&before),
        "the number stands where it stood"
    );
    // Typeset whole once typing pauses, it is in the middle again.
    compile(&mut viewer);
    let whole = glyph_places(&viewer, 0);
    assert!(
        begins(&whole) < begins(&before),
        "{:?} {:?}",
        begins(&whole),
        begins(&before)
    );
    assert_eq!(number(&whole), number(&before));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn letters_typed_together_in_an_equation_are_what_is_meant() {
    let text = "#set page(width: 300pt, height: 300pt)\nSome words\n\nMore.\n";
    let (mut viewer, directory) = editing(text);
    // `xy` is no name: it is x times y, as whoever types it means.
    // A name is a name, and so is one that is still being typed.
    press(&mut viewer, "/words<CR>gsgsA and $xy + alpha = abc");
    assert!(
        viewer
            .text()
            .is_some_and(|text| text.contains("$x y + alpha = abc$")),
        "{:?}",
        viewer.text()
    );
    press(&mut viewer, "<Esc>");
    assert!(
        viewer
            .text()
            .is_some_and(|text| text.contains("$x y + alpha = a b c$")),
        "{:?}",
        viewer.text()
    );
    compile(&mut viewer);
    assert_eq!(viewer.document_revision, Some(viewer.revision));
    assert_eq!(viewer.problems, Vec::<String>::new());
    // All of it is one step to take back.
    press(&mut viewer, "u");
    assert!(viewer.text().is_some_and(|text| !text.contains('$')));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn an_equation_typst_refuses_is_shown_as_it_is_written() {
    let text = "#set page(width: 300pt, height: 300pt)\n\
                Before $a + \"b$ after.\n\nMore words.\n";
    let (mut viewer, directory) = editing(text);
    // Typst has no meaning for it. It is shown as its characters
    // then, and all the rest is typeset around it.
    compile(&mut viewer);
    assert!(viewer.written.is_some(), "{:?}", viewer.problems);
    compile(&mut viewer);
    assert_eq!(
        viewer.document_revision,
        Some(viewer.revision),
        "{:?}",
        viewer.problems
    );
    let shown = glyph_places(&viewer, 0).len();
    // Each of its characters is a place for the cursor: what is in
    // the way can be gone to and taken away, on the page.
    press(&mut viewer, "gsgs/\\=\"b<CR>");
    assert_eq!(tints(&mut viewer, PAGE_CURSOR).len(), 1);
    press(&mut viewer, "x");
    assert!(
        viewer
            .text()
            .is_some_and(|text| text.contains("Before $a + b$ after.")),
        "{:?}",
        viewer.text()
    );
    // It is an equation again, and shown as one, at once.
    assert_eq!(viewer.written, None);
    assert_eq!(
        viewer.document_revision,
        Some(viewer.revision),
        "{:?}",
        viewer.declined
    );
    let meant = glyph_places(&viewer, 0).len();
    assert!(meant < shown, "no marks of the equation are shown");
    // For the asking, an equation is shown as written too, while
    // the cursor is in it.
    press(&mut viewer, "gx");
    compile(&mut viewer);
    assert!(viewer.written.is_some());
    assert!(
        glyph_places(&viewer, 0).len() > meant + 3,
        "its marks and blanks are shown: {:?}",
        viewer.shown_put
    );
    press(&mut viewer, "gx");
    compile(&mut viewer);
    assert_eq!(viewer.written, None);
    press(&mut viewer, "gx/More<CR>");
    compile(&mut viewer);
    assert_eq!(viewer.written, None, "left behind, it is what it means");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_cursor_is_behind_the_blank_that_was_typed() {
    let text = "#set page(width: 300pt, height: 300pt)\nSome words\n\nMore.\n";
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "/words<CR>gsgsA");
    let behind_word = tints(&mut viewer, PAGE_BAR);
    assert_eq!(behind_word.len(), 1);
    // A blank at the end of a line shows as nothing; the cursor
    // moves on by its width all the same, and stays to be seen.
    press(&mut viewer, " ");
    let behind_blank = tints(&mut viewer, PAGE_BAR);
    assert_eq!(behind_blank.len(), 1, "{:?}", viewer.declined);
    assert!(behind_blank[0].x > behind_word[0].x + 1.0);
    assert!(behind_blank[0].x < behind_word[0].x + 12.0);
    press(&mut viewer, " ");
    let further = tints(&mut viewer, PAGE_BAR);
    assert!(further[0].x > behind_blank[0].x + 1.0);
    // And once the page is that of the text, too.
    compile(&mut viewer);
    let compiled = tints(&mut viewer, PAGE_BAR);
    assert_eq!(compiled.len(), 1);
    assert!(
        (compiled[0].x - further[0].x).abs() < 2.0,
        "{compiled:?} {further:?}"
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_lines_say_how_far_they_are_from_the_cursor() {
    let text = "#set page(width: 300pt, height: 300pt)\n\
                One line.\n\nTwo lines.\n\nThree lines.\n\nFour lines.\n";
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "/Two<CR>gsgs");
    let numbered = |viewer: &mut Viewer| {
        let _ = viewer.scene(&|_| false);
        let (_, mut numbered) = viewer.numbered.clone().unwrap_or_default();
        numbered.sort_unstable();
        numbered
            .into_iter()
            .map(|(_, line, away)| (line, away))
            .collect::<Vec<_>>()
    };
    assert_eq!(numbered(&mut viewer), [(0, 1), (1, 0), (2, 1), (3, 2)]);
    // The number is the one `j` wants.
    press(&mut viewer, "2j");
    assert_eq!(numbered(&mut viewer), [(0, 3), (1, 2), (2, 1), (3, 0)]);
    press(&mut viewer, ":numbers<CR>");
    let before = viewer.scene(&|_| false).quads.len();
    press(&mut viewer, ":numbers<CR>");
    assert!(viewer.scene(&|_| false).quads.len() > before);
    // While something is typed the pages are behind it after each
    // key (here for good: nothing compiles them, and nothing is
    // typeset by itself). The numbers stay where they stood; they do
    // not go and come with every letter.
    let stood = numbered(&mut viewer);
    press(&mut viewer, ":parts<CR>A more");
    assert!(viewer.places().is_none(), "the pages are behind");
    assert_eq!(numbered(&mut viewer), stood);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_interface_is_as_large_as_asked() {
    let (mut viewer, directory) = editing("Some words.\n");
    let usual = viewer.status_height();
    press(&mut viewer, ":ui 2<CR>");
    assert_eq!(viewer.status_height(), 2 * usual);
    press(&mut viewer, ":ui 150%<CR>");
    assert!(viewer.status_height() > usual && viewer.status_height() < 2 * usual);
    press(&mut viewer, ":ui 1<CR>:ui +<CR>");
    assert!(viewer.status_height() > usual);
    press(&mut viewer, ":ui -<CR>");
    assert_eq!(viewer.status_height(), usual);
    // Another display's scale does not undo it.
    press(&mut viewer, ":ui 2<CR>");
    viewer.resize((1000, 800), 1.0);
    assert_eq!(viewer.status_height(), 2 * usual);
    press(&mut viewer, ":ui huge<CR>");
    assert_eq!(viewer.status_height(), 2 * usual);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_document_is_set_up_by_commands() {
    let (mut viewer, directory) = editing("Some words.\n");
    press(&mut viewer, ":fontsize 12<CR>");
    assert_eq!(viewer.text(), Some("#set text(size: 12pt)\nSome words.\n"));
    press(&mut viewer, ":justify<CR>:columns 2<CR>:justify<CR>");
    assert_eq!(
        viewer.text(),
        Some(
            "#set page(columns: 2)\n#set par(justify: false)\n\
             #set text(size: 12pt)\nSome words.\n"
        )
    );
    press(&mut viewer, "u");
    assert!(
        viewer
            .text()
            .is_some_and(|text| text.contains("justify: true"))
    );
    compile(&mut viewer);
    assert_eq!(viewer.document_revision, Some(viewer.revision));
    // Everything there is to do is in a list; what needs more said
    // asks for it.
    press(&mut viewer, "g?");
    assert!(viewer.is_picking());
    press(&mut viewer, "margins<CR>");
    assert!(!viewer.is_picking());
    press(&mut viewer, "3<CR>");
    assert!(
        viewer
            .text()
            .is_some_and(|text| text.contains("#set page(columns: 2, margin: 3cm)")),
        "{:?}",
        viewer.text()
    );
    press(&mut viewer, ":font<CR>");
    assert!(viewer.is_picking());
    press(&mut viewer, "<Esc>:margin wide<CR>");
    assert!(
        viewer
            .text()
            .is_some_and(|text| text.contains("margin: 3cm"))
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_document_manager_decides_how_the_document_looks() {
    let (mut viewer, directory) = editing("Some words.\n");
    let has = |viewer: &Viewer, rule: &str| viewer.text().is_some_and(|text| text.contains(rule));
    // One of two is switched at once; the list stays up for more.
    press(&mut viewer, "gDjustified<CR>");
    assert!(
        has(&viewer, "#set par(justify: true)"),
        "{:?}",
        viewer.text()
    );
    assert!(viewer.is_picking());
    // What it can be is listed; anything else is typed.
    press(&mut viewer, "font size<CR>12<CR>");
    assert!(has(&viewer, "#set text(size: 12pt)"), "{:?}", viewer.text());
    press(&mut viewer, "font size<CR>13.5<CR>");
    assert!(
        has(&viewer, "#set text(size: 13.5pt)"),
        "{:?}",
        viewer.text()
    );
    press(&mut viewer, "numbers stand<CR>top, right<CR>");
    assert!(
        has(&viewer, "number-align: right + top"),
        "{:?}",
        viewer.text()
    );
    press(&mut viewer, "running<CR>My paper<CR>");
    assert!(has(&viewer, "header: [My paper]"), "{:?}", viewer.text());
    // Escape leads back to the list, then away.
    press(&mut viewer, "paper<CR>");
    assert!(viewer.is_picking());
    press(&mut viewer, "<Esc>");
    assert!(viewer.is_picking());
    press(&mut viewer, "<Esc>");
    assert!(!viewer.is_picking());
    compile(&mut viewer);
    assert_eq!(viewer.document_revision, Some(viewer.revision));
    // What cannot be is not written.
    press(&mut viewer, ":doc<CR>columns<CR>many<CR><Esc>");
    assert!(!has(&viewer, "columns"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn what_is_selected_on_a_page_is_made_strong_by_the_key_that_types_strong() {
    let (mut viewer, directory) = editing("Some plain words here.\n");
    press(&mut viewer, "/plain<CR>gsgsviw<C-b>");
    assert_eq!(viewer.text(), Some("Some *plain* words here.\n"));
    compile(&mut viewer);
    press(&mut viewer, "/words<CR>ve<C-i>");
    assert_eq!(viewer.text(), Some("Some *plain* _words_ here.\n"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_bibliography_manager_keeps_the_sources() {
    let (mut viewer, directory) = editing("Some words here.\n");
    // A source is added: its kind is chosen, what is known is typed.
    press(&mut viewer, "gB");
    assert!(viewer.is_picking());
    press(&mut viewer, "<CR>book<CR>");
    press(&mut viewer, "<CR>Donald Knuth<CR>");
    press(&mut viewer, "<CR>The TeXbook<CR>");
    press(&mut viewer, "year<CR>1984<CR>");
    press(&mut viewer, "<Esc><Esc>");
    assert!(!viewer.is_picking());
    assert_eq!(
        viewer.text(),
        Some("Some words here.\n\n#bibliography(\"refs.bib\")\n")
    );
    let sources = viewer.sources().unwrap();
    assert_eq!(sources.entries.len(), 1);
    let knuth = &sources.entries[0];
    assert_eq!(
        (knuth.key.as_str(), knuth.kind.as_str()),
        ("knuth1984", "book")
    );
    assert_eq!(knuth.field("title"), Some("The TeXbook"));
    // It is cited from the list, behind the word the cursor is on.
    press(&mut viewer, "/words<CR>gBknuth<CR>cite<CR>");
    assert!(
        viewer
            .text()
            .is_some_and(|text| text.starts_with("Some words @knuth1984 here.")),
        "{:?}",
        viewer.text()
    );
    compile(&mut viewer);
    assert_eq!(viewer.document_revision, Some(viewer.revision));
    // Agents add sources as text; a key that is taken is not kept.
    let entry = "@article{knuth1984, author = {Ada Lovelace}, title = {Notes}, year = {1843}}";
    assert_eq!(
        viewer.agent_add_sources(entry),
        Ok(vec!["lovelace1843".to_owned()])
    );
    assert!(viewer.agent_add_sources("no entry").is_err());
    assert_eq!(viewer.sources().unwrap().entries.len(), 2);
    // Where agents only propose, it is a proposal: nothing is added.
    viewer.agent_edits = AgentEdits::Propose;
    assert_eq!(viewer.agent_add_sources(entry), Ok(Vec::new()));
    assert_eq!(viewer.sources().unwrap().entries.len(), 2);
    let (proposed, _) = viewer.proposed_sources()[0].clone();
    viewer.execute(&format!("bib reject {proposed}"));
    press(&mut viewer, "<Esc>");
    // Taken out, after being asked.
    press(&mut viewer, "gBlovelace<CR>take<CR>yes<CR><Esc>");
    let left = viewer.sources().unwrap();
    assert_eq!(left.entries.len(), 1);
    assert_eq!(left.entries[0].key, "knuth1984");
    let _ = std::fs::remove_dir_all(directory);
}

/// Where every glyph of page `index` stands, to a tenth of a point.
fn glyph_places(viewer: &Viewer, index: usize) -> Vec<(i64, i64)> {
    let (Some(document), Some(world)) = (&viewer.document, &viewer.compiled_world) else {
        return Vec::new();
    };
    let map = PageMap::build(world, &document.pages()[index].frame);
    let tenth = |length: typst::layout::Abs| (length.to_pt() * 10.0).round() as i64;
    map.glyphs()
        .iter()
        .map(|glyph| (tenth(glyph.rect.min.x), tenth(glyph.rect.min.y)))
        .collect()
}

#[test]
fn what_changed_is_typeset_by_itself_as_the_whole_would_set_it() {
    let text = "#set page(width: 300pt, height: 400pt)\n\
                = Title\n\
                First paragraph with words that go on for more than one line of the page.\n\n\
                Second $x^2$ one.\n\n\
                - item one\n- item two\n\n\
                Last paragraph.\n";
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "/x<CR>gsgs");
    // Each of these keys changes more than a word of a line: the
    // paragraph is typeset by itself, at once, and the page shows
    // the text as it is without the whole having been typeset.
    let keys = [
        "a", "_", "i", " ", "+", " ", "f", "f", "<Tab>", "a", "<Tab>", "b",
    ];
    for key in keys {
        press(&mut viewer, key);
        assert_eq!(
            viewer.document_revision,
            Some(viewer.revision),
            "after {key}: {:?}",
            viewer.declined
        );
    }
    assert!(!viewer.is_compiled());
    assert!(viewer.stats.local >= 8, "{}", viewer.stats.local);
    assert_eq!(tints(&mut viewer, PAGE_BAR).len(), 1);
    // And as the whole sets it: every glyph where it will be.
    let alone = glyph_places(&viewer, 0);
    compile(&mut viewer);
    assert_eq!(alone, glyph_places(&viewer, 0));
    // So with a paragraph begun behind another, and a heading.
    press(
        &mut viewer,
        "<Esc>/Second<CR>A<CR>A new paragraph, with *strong* words in it.<CR>== Part",
    );
    assert_eq!(
        viewer.document_revision,
        Some(viewer.revision),
        "{:?}",
        viewer.declined
    );
    let alone = glyph_places(&viewer, 0);
    compile(&mut viewer);
    assert_eq!(alone, glyph_places(&viewer, 0));
    // What is before everything changes everything: that waits.
    press(&mut viewer, "<Esc>:fontsize 9<CR>");
    assert_ne!(viewer.document_revision, Some(viewer.revision));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn tables_grow_and_shrink_by_keys() {
    let text = "#table(\n  columns: 2,\n  [a], [b],\n  [c], [d],\n)\n\nAfter.\n";
    let (mut viewer, directory) = editing(text);
    let table = |viewer: &Viewer| {
        let text = viewer.text().unwrap_or_default();
        text[..text.find("\n\nAfter").unwrap_or(text.len())].to_owned()
    };
    press(&mut viewer, "/d<CR>:table column<CR>");
    assert_eq!(
        table(&viewer),
        "#table(\n  columns: 3,\n  [a], [b], [],\n  [c], [d], [],\n)"
    );
    press(&mut viewer, "u/b]<CR>:table delete row<CR>");
    assert_eq!(table(&viewer), "#table(\n  columns: 2,\n  [c], [d],\n)");
    press(&mut viewer, ":table delete row<CR>");
    assert_eq!(table(&viewer), "#table(\n  columns: 2,\n  [c], [d],\n)");
    // While typing: Tab behind the last cell makes a row, Backspace
    // takes one nothing was typed into, Enter leaves the table.
    compile(&mut viewer);
    press(&mut viewer, "gsgs/d<CR>a<Tab>e<Tab>f<Tab>");
    assert_eq!(
        table(&viewer),
        "#table(\n  columns: 2,\n  [c], [d],\n  [e], [f],\n  [], [],\n)"
    );
    press(&mut viewer, "<BS>!<CR> Then");
    assert_eq!(
        viewer.text(),
        Some("#table(\n  columns: 2,\n  [c], [d],\n  [e], [f!],\n) Then\n\nAfter.\n")
    );
    press(&mut viewer, "/After<CR>:table row<CR>");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn an_equation_left_unfinished_does_not_take_the_document_down() {
    let text = "#set page(width: 300pt, height: 300pt)\nA $x^2$ b.\n\nMore words.\n";
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "/2<CR>gsgsa + ff<Tab><Esc>");
    compile(&mut viewer);
    press(&mut viewer, "/More<CR>A and more<Esc>");
    compile(&mut viewer);
    assert!(
        viewer
            .text()
            .is_some_and(|text| text.contains("$x^2 + frac(, )$") && text.contains("and more")),
        "{:?}",
        viewer.text()
    );
    assert_eq!(viewer.document_revision, Some(viewer.revision));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn what_the_user_wants_to_put_in_is_in_the_menu() {
    let (mut viewer, directory) = editing("Some words.\n");
    viewer.inserts = vec![crate::typst_text::Insert {
        name: "diagram",
        short: "",
        body: "#diagram(|)",
        block: true,
    }];
    press(&mut viewer, "gsgsA<CR>#dia<CR>x");
    assert_eq!(viewer.text(), Some("Some words.\n\n#diagram(x)\n"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_cursor_keys_go_by_the_page_while_typing() {
    let text = "#set page(width: 300pt, height: 300pt)\n\
                One $alpha + beta$ two.\n\nSecond paragraph here.\n";
    let (mut viewer, directory) = editing(text);
    let cursor = |viewer: &Viewer| viewer.editor.as_ref().map_or(0, Editor::cursor);
    press(&mut viewer, "/One<CR>gsgsA");
    // A symbol written with several letters is one step.
    press(&mut viewer, "<Left><Left><Left><Left><Left><Left>");
    assert_eq!(cursor(&viewer), text.find("beta").unwrap_or(0));
    press(&mut viewer, "<Left><Left>");
    assert_eq!(cursor(&viewer), text.find("alpha").unwrap_or(0));
    press(&mut viewer, "<Right>");
    assert_eq!(cursor(&viewer), text.find(" + beta").unwrap_or(0));
    // To the ends of the typeset line, and down a line of the page.
    press(&mut viewer, "<Home>");
    assert_eq!(cursor(&viewer), text.find("One").unwrap_or(0));
    press(&mut viewer, "<End>");
    assert_eq!(cursor(&viewer), text.find("two.").unwrap_or(0) + 4);
    press(&mut viewer, "<Home><Down>!");
    assert!(
        viewer
            .text()
            .is_some_and(|text| text.contains("\n\n!Second paragraph here.")),
        "{:?}",
        viewer.text()
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn an_equation_is_typed_where_it_stands() {
    let text = "#set page(width: 300pt, height: 300pt)\n\
                Some words and $x^2 + y$ in a line.\n\n\
                = A heading\n";
    let (mut viewer, directory) = editing(text);
    let lens = |viewer: &mut Viewer| {
        let edge = TextureId(hash128(&("solid", LENS_EDGE)));
        let scene = viewer.scene(&|_| false);
        let quads = scene.quads;
        quads.into_iter().find(|quad| quad.texture == edge)
    };
    // Its source is not shown: not for a cursor that stands on it,
    // and not while it is typed.
    press(&mut viewer, "/y<CR>gsgs");
    assert_eq!(lens(&mut viewer), None);
    assert_eq!(tints(&mut viewer, PAGE_CURSOR).len(), 1);
    // Half an equation is none to Typst. The page shows it all the
    // same, with a box for what it lacks, and the cursor in it.
    press(&mut viewer, "a^");
    compile(&mut viewer);
    assert_eq!(viewer.document_revision, Some(viewer.revision));
    assert!(viewer.text().unwrap().contains("$x^2 + y^$"));
    assert_eq!(lens(&mut viewer), None);
    assert_eq!(tints(&mut viewer, PAGE_BAR).len(), 1);
    // So with the parts of a fraction, and Tab leads through them.
    press(&mut viewer, "n + ff<Tab>");
    compile(&mut viewer);
    assert_eq!(viewer.document_revision, Some(viewer.revision));
    press(&mut viewer, "a<Tab>b<Tab><Tab> and");
    compile(&mut viewer);
    assert!(
        viewer
            .text()
            .unwrap()
            .contains("$x^2 + y^n + frac(a, b)$ and in a line."),
        "{:?}",
        viewer.text()
    );
    // A word that is not yet a name is shown as the word.
    press(&mut viewer, "<Esc>/b<CR>a + alp");
    compile(&mut viewer);
    assert_eq!(viewer.document_revision, Some(viewer.revision));
    assert_eq!(lens(&mut viewer), None);
    press(&mut viewer, "<Esc>");
    press(&mut viewer, "/words<CR>inew ");
    assert_eq!(lens(&mut viewer), None);
    // The cursor does not rest on code that shows nowhere.
    press(&mut viewer, "<Esc>");
    compile(&mut viewer);
    press(&mut viewer, "gg");
    assert_eq!(tints(&mut viewer, PAGE_CURSOR).len(), 1);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn typing_behind_an_escaped_character_is_shown_where_it_goes() {
    // `\#` is two bytes of the text and one character of the page:
    // what is typed behind it goes behind it, as the compile has it.
    for (text, keys) in [
        ("Some \\#s\n\nOther.\n", "/Some<CR>A"),
        ("Some \\#s words.\n\nOther.\n", "/s words<CR>a"),
        ("\\#s\n\nOther.\n", "ggjA"),
    ] {
        let text = format!("#set page(width: 300pt, height: 200pt)\n{text}");
        let (mut viewer, directory) = editing(&text);
        press(&mut viewer, keys);
        compile(&mut viewer);
        press(&mut viewer, "k");
        assert!(viewer.is_predicted(), "{text:?}: {:?}", viewer.declined);
        if let Some(predicted) = viewer.predicted.as_ref().map(|p| p.page.clone()) {
            let predicted = zetwerk_render::render_page(&predicted, 2.0);
            compile(&mut viewer);
            let document = viewer.document().unwrap();
            let compiled = zetwerk_render::render_page(&document.pages()[0], 2.0);
            assert!(
                predicted.data() == compiled.data(),
                "{text:?}: the prediction differs from the compile"
            );
        }
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn typing_a_name_behind_a_typed_hash_is_shown_where_it_goes() {
    // `#` typed is `\#` with a list up; the name typed behind it is
    // worked out without a compile, a few keys at a time, as Typst
    // then sets it.
    for (text, keys) in [
        ("Some words.\n\nOther.\n", "gsgs/Some<CR>A #"),
        ("Some words.\n\nOther.\n", "gsgs/Some<CR>o#"),
        ("Some words.\n\nOther.\n", "gsgs/Some<CR>o<CR>#"),
    ] {
        let text = format!("#set page(width: 300pt, height: 200pt)\n{text}");
        let (mut viewer, directory) = editing(&text);
        for key in keys::parse(keys).unwrap() {
            viewer.key(key);
            compile(&mut viewer);
        }
        press(&mut viewer, "s");
        compile(&mut viewer);
        for typed in ["kel", "et"] {
            press(&mut viewer, typed);
            let predicted = viewer.predicted.as_ref().map(|p| p.page.clone());
            let Some(predicted) = predicted else {
                panic!("{keys}{typed}: declined {:?}", viewer.declined);
            };
            let predicted = zetwerk_render::render_page(&predicted, 2.0);
            compile(&mut viewer);
            let document = viewer.document().unwrap();
            let compiled = zetwerk_render::render_page(&document.pages()[0], 2.0);
            assert!(
                predicted.data() == compiled.data(),
                "{keys}{typed}: the prediction differs from the compile: {:?}",
                viewer.text()
            );
        }
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn typing_shows_on_the_page_without_a_compile() {
    // A paragraph of several lines tells how wide a line may get.
    let text = "#set page(width: 300pt, height: 200pt)\n\
                The quick brown fox jumps.\n\n\
                Second paragraph here, which goes on for long enough to \
                be broken into more than one line of the page.\n";
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "/brown<CR>ivery ");
    assert!(viewer.is_predicted(), "{:?}", viewer.declined);
    assert_eq!(viewer.take_edited(), Some(Edited::Typing));
    // The cursor is shown behind what was typed.
    let held: HashSet<TextureId> = HashSet::new();
    let bar = TextureId(hash128(&("tint", PAGE_BAR)));
    let scene = viewer.scene(&|id| held.contains(&id));
    assert_eq!(
        scene
            .quads
            .iter()
            .filter(|quad| quad.texture == bar)
            .count(),
        1
    );
    // What is shown is what the compile then makes of the text.
    let predicted = viewer
        .predicted
        .as_ref()
        .map(|predicted| predicted.page.clone());
    let predicted = zetwerk_render::render_page(&predicted.unwrap(), 2.0);
    compile(&mut viewer);
    assert!(!viewer.is_predicted());
    let document = viewer.document().unwrap();
    let compiled = zetwerk_render::render_page(&document.pages()[0], 2.0);
    assert!(
        predicted.data() == compiled.data(),
        "the prediction differs from the compile"
    );

    // Markup is not text to be shown as typed: that takes a compile,
    // asked for at once where compiles are quick.
    press(&mut viewer, "*");
    assert!(!viewer.is_predicted());
    assert_eq!(viewer.take_edited(), Some(Edited::Unpredicted));
    // Taken back, the pages are those of the text again.
    press(&mut viewer, "<BS>");
    assert!(viewer.is_compiled());
    // Typing on, then back over part of it, is one insertion still.
    press(&mut viewer, "big<BS>");
    assert!(viewer.is_predicted());
    assert_eq!(
        viewer.text().map(|text| text.contains("very bibrown")),
        Some(true)
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_click_puts_the_cursor_on_the_text_under_it() {
    let (mut viewer, directory) = editing("Hello there world.\n\nSecond paragraph.\n");
    let held: HashSet<TextureId> = HashSet::new();
    // Find where the cursor is drawn on the page for a word, move
    // away, and click there.
    press(&mut viewer, "/there<CR>");
    let block = TextureId(hash128(&("tint", PAGE_CURSOR)));
    let scene = viewer.scene(&|id| held.contains(&id));
    let quad = scene
        .quads
        .iter()
        .find(|quad| quad.texture == block)
        .unwrap();
    let on_the_t = (
        f64::from(quad.x) + 1.0,
        f64::from(quad.y) + f64::from(quad.height) / 2.0,
    );
    press(&mut viewer, "G");
    assert_eq!(viewer.state().editing.unwrap().line, 3);
    viewer.click(on_the_t);
    let editing = viewer.state().editing.unwrap();
    assert_eq!((editing.line, editing.column), (1, 7));
    // And in the source: the third row, a few cells in.
    let (gutter, cell, row_height) = viewer.source_metrics;
    viewer.click((gutter + 7.5 * cell, 2.5 * row_height));
    let editing = viewer.state().editing.unwrap();
    assert_eq!((editing.line, editing.column), (3, 8));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn typing_past_the_end_of_a_line_moves_words_down() {
    let text = "#set page(width: 300pt, height: 300pt)\n\
                The quick brown fox jumps over the lazy dog and runs on \
                through the wood until the line is full, and then some \
                more of it follows here to make a few lines of text.\n\n\
                Second paragraph.\n";
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "/brown<CR>i");
    // Far more than the first line has room for.
    let typed = "very very very very very ";
    let mut shown = 0;
    for key in keys::parse(typed).unwrap() {
        viewer.key(key);
        shown += usize::from(viewer.is_predicted());
    }
    assert_eq!(shown, typed.len(), "{:?}", viewer.declined);
    // In ragged text Typst breaks lines the same way: first fit.
    let predicted = viewer
        .predicted
        .as_ref()
        .map(|predicted| predicted.page.clone());
    let predicted = zetwerk_render::render_page(&predicted.unwrap(), 2.0);
    compile(&mut viewer);
    let document = viewer.document().unwrap();
    let compiled = zetwerk_render::render_page(&document.pages()[0], 2.0);
    assert!(
        predicted.data() == compiled.data(),
        "the prediction differs from the compile"
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn typing_at_the_end_of_a_paragraph_adds_lines() {
    let text = "#set page(width: 300pt, height: 300pt)\n\
                The quick brown fox jumps over the lazy dog and runs on \
                through the wood until the line is full, and then some.\n\n\
                Second paragraph, which is also long enough to be broken \
                into two lines of the page.\n\n\
                Third.\n";
    let (mut viewer, directory) = editing(text);
    // At the end of the first paragraph: enough for two more lines.
    // The keys that add one are typeset by themselves, the others
    // worked out on their line; none waits.
    press(&mut viewer, "/some<CR>A");
    let typed =
        " More and more words are typed here, and the paragraph grows by a line and by another.";
    let mut shown = 0;
    for key in keys::parse(typed).unwrap() {
        viewer.key(key);
        shown +=
            usize::from(viewer.is_predicted() || viewer.document_revision == Some(viewer.revision));
    }
    assert_eq!(shown, typed.len(), "{:?}", viewer.declined);
    assert!(viewer.stats.local >= 1, "{}", viewer.stats.local);
    let predicted = viewer
        .predicted
        .as_ref()
        .map(|predicted| predicted.page.clone())
        .or_else(|| Some(viewer.document.as_ref()?.pages()[0].clone()));
    let predicted = zetwerk_render::render_page(&predicted.unwrap(), 2.0);
    compile(&mut viewer);
    let document = viewer.document().unwrap();
    let compiled = zetwerk_render::render_page(&document.pages()[0], 2.0);
    assert!(
        predicted.data() == compiled.data(),
        "the prediction differs from the compile"
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn deleting_and_typing_over_show_on_the_page_too() {
    let text = "#set page(width: 300pt, height: 300pt)\n\
                The quick brown fox jumps over the lazy dog and runs on \
                through the wood until the line is full, and then some.\n\n\
                A short one here.\n";
    let (mut viewer, directory) = editing(text);
    let same_as_compiled = |viewer: &mut Viewer| {
        let predicted = viewer
            .predicted
            .as_ref()
            .map(|predicted| predicted.page.clone());
        let predicted = zetwerk_render::render_page(&predicted.unwrap(), 2.0);
        compile(viewer);
        let document = viewer.document().unwrap();
        let compiled = zetwerk_render::render_page(&document.pages()[0], 2.0);
        predicted.data() == compiled.data()
    };
    // A character out of the last paragraph.
    press(&mut viewer, "/short<CR>x");
    assert!(viewer.is_predicted(), "{:?}", viewer.declined);
    assert!(same_as_compiled(&mut viewer));
    // Backspace over text that was there before typing began.
    press(&mut viewer, "A<BS><BS><BS>");
    assert!(viewer.is_predicted(), "{:?}", viewer.declined);
    assert_eq!(
        viewer.text().map(|text| text.contains("A hort one he\n")),
        Some(true)
    );
    assert!(same_as_compiled(&mut viewer));
    // A word changed into another.
    press(&mut viewer, "<Esc>/one<CR>cwtwo");
    assert!(viewer.is_predicted(), "{:?}", viewer.declined);
    assert!(same_as_compiled(&mut viewer));
    // A word with the blank behind it.
    press(&mut viewer, "<Esc>/hort<CR>dw");
    assert!(viewer.is_predicted(), "{:?}", viewer.declined);
    assert!(same_as_compiled(&mut viewer));
    // A word from between two blanks leaves two blanks, which Typst
    // sets as one: that is for the compiler.
    press(&mut viewer, "/brown<CR>diw");
    assert!(!viewer.is_predicted());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn deleting_brings_words_up_from_the_next_line() {
    let text = "#set page(width: 300pt, height: 300pt)\n\
                The quick brown fox jumps over the lazy dog and runs on \
                through the wood until the line is full, and then some \
                more of it follows here to make a few lines of text, \
                the last of which is short.\n\n\
                Second paragraph.\n";
    let (mut viewer, directory) = editing(text);
    let same_as_compiled = |viewer: &mut Viewer| {
        let predicted = viewer
            .predicted
            .as_ref()
            .map(|predicted| predicted.page.clone());
        let predicted = zetwerk_render::render_page(&predicted.unwrap(), 2.0);
        compile(viewer);
        let document = viewer.document().unwrap();
        let compiled = zetwerk_render::render_page(&document.pages()[0], 2.0);
        if let Some(dir) = std::env::var_os("ZETWERK_DUMP").map(PathBuf::from) {
            let _ = predicted.save_png(dir.join("predicted.png"));
            let _ = compiled.save_png(dir.join("compiled.png"));
        }
        predicted.data() == compiled.data()
    };
    // Two words out of the first line: the lines below move up a bit.
    press(&mut viewer, "/quick<CR>dw");
    assert!(viewer.is_predicted(), "{:?}", viewer.declined);
    assert!(same_as_compiled(&mut viewer), "after one word");
    press(&mut viewer, "dw");
    assert!(viewer.is_predicted(), "{:?}", viewer.declined);
    assert!(same_as_compiled(&mut viewer), "after two words");
    // Enough out of it that the paragraph loses its last line.
    press(&mut viewer, "d4w");
    assert!(viewer.is_predicted(), "{:?}", viewer.declined);
    assert!(same_as_compiled(&mut viewer), "after a line's worth");
    let _ = std::fs::remove_dir_all(directory);
}

/// Where the cursor is drawn on the first page, in points.
fn caret_on_page(viewer: &Viewer) -> (f64, f64) {
    let document = viewer.document.as_ref().unwrap();
    let world = viewer.compiled_world.as_ref().unwrap();
    let map = PageMap::build(world, &document.pages()[0].frame);
    let cursor = viewer.editor.as_ref().unwrap().cursor();
    let cell = map.caret(viewer.project.main(), cursor).unwrap().cell;
    (cell.min.x.to_pt(), cell.min.y.to_pt())
}

#[test]
fn on_the_page_the_cursor_goes_up_and_down_by_its_lines() {
    // The paragraph is one line of the source and five on the page.
    let text = "#set page(width: 300pt, height: 300pt)\n\
                The quick brown fox jumps over the lazy dog and runs on \
                through the wood until the line is full, and then some \
                more of it follows here to make a few lines of text, \
                the last of which is short.\n\n\
                Second paragraph.\n";
    let (mut viewer, directory) = editing(text);
    let cursor = |viewer: &Viewer| viewer.editor.as_ref().unwrap().cursor();
    let line = |viewer: &Viewer| viewer.editor.as_ref().unwrap().position().0;
    press(&mut viewer, "gsgs/brown<CR>");
    assert_eq!(viewer.panes(), Panes::Page);
    let start = cursor(&viewer);
    let (x, y) = caret_on_page(&viewer);

    press(&mut viewer, "j");
    assert_eq!(line(&viewer), 2, "still in the paragraph's one line");
    assert!(cursor(&viewer) > start);
    let (below_x, below_y) = caret_on_page(&viewer);
    assert!(below_y > y + 5.0, "a line further down: {y} to {below_y}");
    assert!(
        (below_x - x).abs() < 8.0,
        "below where it was: {x} to {below_x}"
    );
    press(&mut viewer, "k");
    assert_eq!(cursor(&viewer), start);

    // A count, and the aim holds across a line too short for it.
    press(&mut viewer, "/short<CR>$");
    let (end_x, _) = caret_on_page(&viewer);
    press(&mut viewer, "/full<CR>");
    let (x, y) = caret_on_page(&viewer);
    assert!(x > end_x + 20.0, "beyond the end of the last line");
    press(&mut viewer, "2j");
    assert_eq!(line(&viewer), 2);
    let (_, last_y) = caret_on_page(&viewer);
    assert!(last_y > y + 15.0, "two lines down: {y} to {last_y}");
    press(&mut viewer, "j");
    assert_eq!(line(&viewer), 4, "into the next paragraph");
    press(&mut viewer, "3k");
    let (back_x, back_y) = caret_on_page(&viewer);
    assert!((back_y - y).abs() < 0.5 && (back_x - x).abs() < 8.0);

    // With an operator, `j` is still about lines of the text.
    press(&mut viewer, "dj");
    assert_eq!(
        viewer.text(),
        Some("#set page(width: 300pt, height: 300pt)\nSecond paragraph.\n")
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn on_the_page_the_cursor_goes_through_columns_and_pages_in_reading_order() {
    let words: Vec<String> = (0..150).map(|i| format!("w{i}")).collect();
    let text = format!(
        "#set page(width: 300pt, height: 120pt, columns: 2)\n{}\n",
        words.join(" ")
    );
    let (mut viewer, directory) = editing(&text);
    assert!(viewer.document().unwrap().pages().len() > 1);
    let cursor = |viewer: &Viewer| viewer.editor.as_ref().unwrap().cursor();
    press(&mut viewer, "gsgsjw");
    // Down every line: each is further on in the text, to the last.
    let mut places = vec![cursor(&viewer)];
    loop {
        press(&mut viewer, "j");
        let now = cursor(&viewer);
        if places.last() == Some(&now) {
            break;
        }
        assert!(places.last() < Some(&now), "{places:?} then {now}");
        places.push(now);
    }
    assert!(places.len() > 12, "{places:?}");
    assert!(
        text[cursor(&viewer)..].split(' ').count() < 8,
        "the last line"
    );
    // The cursor is as far into each column as it was into the first.
    let words_in = |place: &usize| text[..*place].matches(' ').count();
    let per_line = words_in(&places[1]) - words_in(&places[0]);
    assert!(places.windows(2).all(|pair| {
        let step = words_in(&pair[1]) - words_in(&pair[0]);
        step.abs_diff(per_line) <= 2
    }));
    // And back up the same way.
    for place in places.iter().rev().skip(1) {
        press(&mut viewer, "k");
        assert_eq!(cursor(&viewer), *place);
    }
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn on_the_page_no_line_is_passed_over() {
    // A centred equation under a short line, the cells of a table,
    // a line at the right margin: none straight below the other.
    let text = "#set page(width: 300pt, height: 400pt)\n\
                Short.\n\n\
                $ a + b = c $\n\n\
                After it the text goes on.\n\n\
                #table(columns: 2, [one], [two], [three], [four])\n\n\
                #align(right)[End.]\n";
    let (mut viewer, directory) = editing(text);
    let word = |viewer: &Viewer| {
        let cursor = viewer.editor.as_ref().unwrap().cursor();
        let blank = |c: char| c.is_whitespace() || "[]$".contains(c);
        let start = text[..cursor].rfind(blank).map_or(0, |at| at + 1);
        let end = text[cursor..]
            .find(blank)
            .map_or(text.len(), |at| cursor + at);
        &text[start..end]
    };
    let walk = |viewer: &mut Viewer, key: &str| {
        let mut words = vec![word(viewer)];
        for _ in 0..12 {
            press(viewer, key);
            if words.last() == Some(&word(viewer)) {
                break;
            }
            words.push(word(viewer));
        }
        words
    };
    press(&mut viewer, "gsgs/Short<CR>");
    assert_eq!(
        walk(&mut viewer, "j"),
        ["Short.", "a", "After", "one", "three", "End."]
    );
    // And up again along the right-hand side.
    press(&mut viewer, "$");
    assert_eq!(
        walk(&mut viewer, "k"),
        ["End.", "four", "two", "on.", "c", "Short."]
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn in_the_source_gj_goes_down_a_row_of_a_wrapped_line() {
    let words = "word ".repeat(40);
    let (mut viewer, directory) = editing(&format!("{words}\nshort\n"));
    let cursor = |viewer: &Viewer| viewer.editor.as_ref().unwrap().cursor();
    let columns = viewer.source_size.0;
    assert!(columns < 200);
    press(&mut viewer, "llgj");
    let row = columns / 5 * 5;
    assert_eq!(cursor(&viewer), row + 2, "the same place in the next row");
    press(&mut viewer, "gj");
    assert_eq!(cursor(&viewer), 2 * row + 2);
    press(&mut viewer, "2gk");
    assert_eq!(cursor(&viewer), 2);
    // `j` goes by lines of the text there.
    press(&mut viewer, "j");
    assert_eq!(cursor(&viewer), 203);
    // With an operator `gj` takes the text up to that place.
    press(&mut viewer, "ggdgj");
    assert_eq!(viewer.text().map(str::len), Some(200 - row + 7));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_panes_can_be_switched() {
    let (mut viewer, directory) = editing("one\ntwo\n");
    press(&mut viewer, "gs");
    assert_eq!(viewer.panes(), Panes::Source);
    press(&mut viewer, "gs");
    assert_eq!(viewer.panes(), Panes::Page);
    // With only pages shown, keys still edit: the cursor is on the
    // page.
    press(&mut viewer, "x");
    assert_eq!(viewer.text(), Some("ne\ntwo\n"));
    press(&mut viewer, "gs");
    assert_eq!(viewer.panes(), Panes::Split);
    press(&mut viewer, "x");
    assert_eq!(viewer.text(), Some("e\ntwo\n"));
    press(&mut viewer, ":view source<CR>");
    assert_eq!(viewer.panes(), Panes::Source);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_source_is_drawn_row_by_row() {
    let (mut viewer, directory) = editing("one\ntwo\n\nfour\n");
    // Without the bar beside the line that changes: its colour is an
    // image too, made once.
    press(&mut viewer, ":changes off<CR>");
    let mut held = HashSet::new();
    let scene = viewer.scene(&|id| held.contains(&id));
    let images = scene.images.len();
    held.extend(scene.images.iter().map(|image| image.id));
    // Nothing new to make for the same frame again, or to move the
    // cursor.
    let scene = viewer.scene(&|id| held.contains(&id));
    assert!(scene.images.is_empty());
    press(&mut viewer, "j");
    let scene = viewer.scene(&|id| held.contains(&id));
    assert_eq!(scene.images.len(), 1, "only the status line changed");
    held.extend(scene.images.iter().map(|image| image.id));
    // Typing makes one row anew, and the status line.
    press(&mut viewer, "ix<Esc>");
    let scene = viewer.scene(&|id| held.contains(&id));
    assert_eq!(scene.images.len(), 2);
    assert!(images > 4, "rows, numbers and chrome: {images} images");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn an_agent_reads_and_changes_the_text() {
    use super::agent::Outcome;
    let text = "= Title\n\nThe quick brown fox.\nThe lazy dog.\n";
    let (mut viewer, directory) = editing(text);
    let lines = viewer.lines(Some(3), None).unwrap();
    assert_eq!(lines.text, "3\tThe quick brown fox.\n4\tThe lazy dog.\n");
    assert_eq!((lines.first, lines.last, lines.total), (3, 4, 4));
    assert!(viewer.lines(Some(9), None).is_err());
    let revision = lines.revision;

    // The cursor is the user's: it stays on its word.
    press(&mut viewer, "/lazy<CR>");
    let outcome = viewer
        .agent_replace(
            "quick brown",
            "slow red and very tired",
            false,
            Some(revision),
            false,
        )
        .unwrap();
    assert_eq!(
        outcome,
        Outcome::Changed {
            revision: revision + 1,
            lines: (3, 3)
        }
    );
    assert_eq!(
        viewer.text(),
        Some("= Title\n\nThe slow red and very tired fox.\nThe lazy dog.\n")
    );
    assert_eq!(viewer.take_edited(), Some(Edited::Agent));
    let cursor = viewer.editor.as_ref().unwrap().cursor();
    assert!(viewer.text().unwrap()[cursor..].starts_with("lazy"));
    assert!(viewer.agent_mark.is_some());

    // Made for text that is no more; not there; there twice.
    let stale = viewer.agent_replace("fox", "cat", false, Some(revision), false);
    assert!(stale.unwrap_err().contains("revision"));
    let missing = viewer.agent_replace("quick", "x", false, None, false);
    assert!(missing.unwrap_err().contains("not in the text"));
    let twice = viewer.agent_replace("The", "A", false, None, false);
    assert!(twice.unwrap_err().contains("2 times (lines 3, 4)"));
    assert_eq!(viewer.state().revision, revision + 1);
    // All of them, if asked: one step.
    viewer.agent_replace("The", "A", true, None, false).unwrap();
    assert_eq!(
        viewer.text(),
        Some("= Title\n\nA slow red and very tired fox.\nA lazy dog.\n")
    );
    press(&mut viewer, "u");
    assert_eq!(
        viewer.text(),
        Some("= Title\n\nThe slow red and very tired fox.\nThe lazy dog.\n")
    );
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some(text));
    assert!(viewer.agent_mark.is_none(), "a key ends the showing");

    // At the cursor, and in place of the selection.
    press(&mut viewer, "/dog<CR>");
    viewer.agent_insert("old ", None, false).unwrap();
    assert!(viewer.text().unwrap().contains("lazy old dog"));
    press(&mut viewer, "viw");
    let focus = viewer.focus().unwrap();
    let selected = focus.selection.unwrap();
    assert_eq!((selected.from, selected.to), ((4, 14), (4, 17)));
    assert_eq!(selected.text, "dog");
    viewer.agent_replace_selection("cat", None, false).unwrap();
    assert!(viewer.text().unwrap().contains("lazy old cat."));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn an_agents_change_is_a_step_of_its_own_while_the_user_types() {
    let (mut viewer, directory) = editing("one\ntwo\n");
    press(&mut viewer, "Aab");
    viewer
        .agent_replace("two", "three", false, None, false)
        .unwrap();
    press(&mut viewer, "cd<Esc>");
    assert_eq!(viewer.text(), Some("oneabcd\nthree\n"));
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some("oneab\nthree\n"));
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some("oneab\ntwo\n"));
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some("one\ntwo\n"));
    press(&mut viewer, "<C-r><C-r><C-r>");
    assert_eq!(viewer.text(), Some("oneabcd\nthree\n"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn proposals_wait_for_the_user() {
    use super::agent::{AgentEdits, Outcome};
    let text = "The quick brown fox.\nThe lazy dog.\nThe end.\n";
    let (mut viewer, directory) = editing(text);
    // Asked for by the agent, or by the user for all agents.
    let first = viewer
        .agent_replace("quick", "slow", false, None, true)
        .unwrap();
    press(&mut viewer, ":agents propose<CR>");
    assert_eq!(viewer.agent_edits(), AgentEdits::Propose);
    let second = viewer
        .agent_replace("lazy dog", "busy cat", false, None, false)
        .unwrap();
    let third = viewer
        .agent_replace("end", "END", false, None, false)
        .unwrap();
    let revision = viewer.state().revision;
    assert_eq!(first, Outcome::Proposed { id: 1, revision });
    assert_eq!(second, Outcome::Proposed { id: 2, revision });
    assert_eq!(third, Outcome::Proposed { id: 3, revision });
    assert_eq!(viewer.text(), Some(text), "nothing is changed yet");
    assert_eq!(viewer.proposals().len(), 3);

    // On a proposal, the panel says what it would do.
    let panel = |viewer: &mut Viewer| {
        let edge = TextureId(hash128(&("solid", LENS_EDGE)));
        let scene = viewer.scene(&|_| false);
        scene.quads.iter().any(|quad| quad.texture == edge)
    };
    assert!(!panel(&mut viewer));
    press(&mut viewer, "]p");
    let cursor = |viewer: &Viewer| viewer.editor.as_ref().unwrap().cursor();
    assert_eq!(cursor(&viewer), text.find("quick").unwrap());
    assert!(panel(&mut viewer));
    press(&mut viewer, "]p");
    assert_eq!(cursor(&viewer), text.find("lazy").unwrap());
    press(&mut viewer, "[p[p");
    assert_eq!(
        cursor(&viewer),
        text.find("end").unwrap(),
        "around the ends"
    );

    // Accepting is a change like any other, and moves the rest along.
    press(&mut viewer, "]pga");
    assert_eq!(
        viewer.text(),
        Some("The slow brown fox.\nThe lazy dog.\nThe end.\n")
    );
    assert_eq!(viewer.take_edited(), Some(Edited::Agent));
    let now = viewer.text().unwrap().to_owned();
    let ranges: Vec<&str> = viewer
        .proposals()
        .iter()
        .map(|proposal| &now[proposal.range.clone()])
        .collect();
    assert_eq!(ranges, ["lazy dog", "end"]);
    // Undone, the accepted one waits again.
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some(text));
    assert_eq!(viewer.proposals().len(), 3);

    // Typing in the text of one makes it stale: it can only go.
    press(&mut viewer, "/dog<CR>x");
    let stale: Vec<bool> = viewer.proposals().iter().map(|p| p.stale).collect();
    assert_eq!(stale, [false, true, false]);
    press(&mut viewer, "0]p");
    assert!(panel(&mut viewer));
    press(&mut viewer, "ga");
    assert!(viewer.text().unwrap().contains("lazy og"), "not made");
    assert_eq!(viewer.proposals().len(), 3);
    // Undoing the typing makes it good again.
    press(&mut viewer, "u");
    assert!(!viewer.proposals()[1].stale);
    press(&mut viewer, "gg0]pgrgg0]pgr");
    assert_eq!(viewer.proposals().len(), 1);
    press(&mut viewer, ":accept all<CR>");
    assert_eq!(
        viewer.text(),
        Some("The quick brown fox.\nThe lazy dog.\nThe END.\n")
    );
    assert_eq!(viewer.proposals(), []);
    // The agent may take one back itself.
    viewer
        .agent_replace("fox", "cat", false, None, false)
        .unwrap();
    let id = viewer.proposals()[0].id;
    assert!(viewer.withdraw_proposal(id + 7).is_err());
    viewer.withdraw_proposal(id).unwrap();
    assert_eq!(viewer.proposals(), []);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn an_agent_finds_its_way_in_the_document() {
    let text = "#set page(width: 300pt, height: 200pt)\n\
                = First <one>\n\
                Some words, and Words again.\n\
                #pagebreak()\n\
                == Second *part*\n\
                More words here.\n";
    let (mut viewer, directory) = editing(text);
    let (found, total) = viewer.search("words", false, 10).unwrap();
    let places: Vec<(usize, usize)> = found.iter().map(|at| (at.line, at.column)).collect();
    assert_eq!((places, total), (vec![(3, 6), (6, 6)], 2));
    let (found, total) = viewer.search("WORDS", true, 2).unwrap();
    assert_eq!((found.len(), total), (2, 3));
    assert_eq!(found[1].text, "Some words, and Words again.");

    let outline = viewer.outline().unwrap();
    let headings: Vec<(usize, &str, usize, Option<usize>)> = outline
        .headings
        .iter()
        .map(|heading| {
            (
                heading.level,
                heading.text.as_str(),
                heading.line,
                heading.page,
            )
        })
        .collect();
    assert_eq!(
        headings,
        [(1, "First", 2, Some(1)), (2, "Second *part*", 5, Some(2))]
    );
    assert_eq!(outline.labels, [("one".to_owned(), 2)]);

    // From the text to the page and back.
    let (page, x, y) = viewer.page_of(6, 6).unwrap();
    assert_eq!(page, 2);
    assert_eq!(
        viewer.source_of(page, x + 1.0, y + 2.0),
        Ok(("main.typ".to_owned(), 6, 6))
    );
    assert!(viewer.source_of(3, 0.0, 0.0).is_err());
    assert!(viewer.page_of(40, 1).is_err());
    // Not for text the pages are not of.
    press(&mut viewer, "x");
    assert!(viewer.page_of(6, 6).unwrap_err().contains("compiled"));
    assert_eq!(viewer.outline().unwrap().headings[0].page, None);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn formats_are_operators_and_typst_has_text_objects() {
    let text = "= Title\nThe quick brown fox and $x + y$ here.\n\n== Sub\nMore text.\n";
    let (mut viewer, directory) = editing(text);
    let line = |viewer: &Viewer, number: usize| {
        viewer
            .text()
            .unwrap()
            .lines()
            .nth(number - 1)
            .unwrap()
            .to_owned()
    };
    // On a motion, on a text object, on a selection; and again with `.`.
    press(&mut viewer, "/quick<CR>gbiw");
    assert_eq!(line(&viewer, 2), "The *quick* brown fox and $x + y$ here.");
    press(&mut viewer, "/fox<CR>.");
    assert_eq!(
        line(&viewer, 2),
        "The *quick* brown *fox* and $x + y$ here."
    );
    press(&mut viewer, "/brown<CR>vegm");
    assert_eq!(
        line(&viewer, 2),
        "The *quick* _brown_ *fox* and $x + y$ here."
    );
    // Off again where it is on: one undo step each.
    press(&mut viewer, "/quick<CR>gbi*");
    assert_eq!(
        line(&viewer, 2),
        "The quick _brown_ *fox* and $x + y$ here."
    );
    press(&mut viewer, "u");
    assert_eq!(
        line(&viewer, 2),
        "The *quick* _brown_ *fox* and $x + y$ here."
    );
    // In the equation bold is a function; the equation is an object.
    press(&mut viewer, "/y<CR>gbl");
    assert!(line(&viewer, 2).ends_with("$x + bold(y)$ here."));
    press(&mut viewer, "ci$a = b<Esc>");
    assert!(line(&viewer, 2).ends_with("and $a = b$ here."));
    press(&mut viewer, "da$");
    assert!(line(&viewer, 2).ends_with("and  here."));

    // Lines: a comment, a list, a heading's level.
    press(&mut viewer, "Ggcc");
    assert_eq!(line(&viewer, 5), "// More text.");
    press(&mut viewer, "gcc");
    assert_eq!(line(&viewer, 5), "More text.");
    press(&mut viewer, "gll");
    assert_eq!(line(&viewer, 5), "- More text.");
    press(&mut viewer, "u4Gg+");
    assert_eq!(line(&viewer, 4), "=== Sub");
    press(&mut viewer, "g-g-");
    assert_eq!(line(&viewer, 4), "= Sub");
    // A section goes as a whole.
    press(&mut viewer, "Gdah");
    assert_eq!(viewer.text().unwrap().lines().count(), 3);
    assert!(!viewer.text().unwrap().contains("Sub"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_cursor_goes_to_what_the_compiler_complains_about() {
    // Slips of syntax: the compiler tells of all of them at once.
    let text = "Fine.\n#(1 +)\nFine too.\n#(2 *)\n";
    let (mut viewer, directory) = editing(text);
    assert_eq!(viewer.state().problems.len(), 2);
    let line = |viewer: &Viewer| viewer.editor.as_ref().unwrap().position().0;
    let message = |viewer: &Viewer| viewer.message.as_ref().unwrap().text.clone();
    press(&mut viewer, "]d");
    assert_eq!(line(&viewer), 2);
    assert!(
        message(&viewer).starts_with("error: "),
        "{}",
        message(&viewer)
    );
    press(&mut viewer, "]d");
    assert_eq!(line(&viewer), 4);
    press(&mut viewer, "]d");
    assert_eq!(line(&viewer), 2, "around the end");
    press(&mut viewer, "[d");
    assert_eq!(line(&viewer), 4);
    // They are marked in the source while the text is that text.
    let marks = |viewer: &mut Viewer| {
        let id = TextureId(hash128(&("solid", ERROR_MARK)));
        let scene = viewer.scene(&|_| false);
        scene.quads.iter().filter(|quad| quad.texture == id).count()
    };
    assert_eq!(marks(&mut viewer), 2);
    press(&mut viewer, "x");
    assert_eq!(marks(&mut viewer), 0);
    // With nothing wrong there is nowhere to go.
    press(&mut viewer, "ggjddjdd");
    compile(&mut viewer);
    press(&mut viewer, "]d");
    assert_eq!(message(&viewer), "the compiler has no complaints");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn brackets_come_in_pairs_and_snippets_expand() {
    let (mut viewer, directory) = editing("\n");
    let text = |viewer: &Viewer| viewer.text().unwrap().trim_end().to_owned();
    // The closing one comes along, is stepped over, and goes with
    // its opening one.
    press(&mut viewer, "if(x<Esc>");
    assert_eq!(text(&viewer), "f(x)");
    press(&mut viewer, "ccf(x) and [y]<Esc>");
    assert_eq!(text(&viewer), "f(x) and [y]");
    press(&mut viewer, "cca(<BS>b<Esc>");
    assert_eq!(text(&viewer), "ab");
    // Before text, and for an apostrophe-like mark behind a word, no.
    press(&mut viewer, "0i(<Esc>");
    assert_eq!(text(&viewer), "(ab");
    press(&mut viewer, "ccsay \"hi\" and 5\" wide<Esc>");
    assert_eq!(text(&viewer), "say \"hi\" and 5\" wide");
    // An equation in its line, and one displayed.
    press(&mut viewer, "ccLet $x$ be.<Esc>");
    assert_eq!(text(&viewer), "Let $x$ be.");
    press(&mut viewer, "cc$ a + b<Esc>");
    assert_eq!(text(&viewer), "$ a + b $");
    // It is one undo step, and `.` types the same again.
    press(&mut viewer, "ccx<Esc>Ag(z)<Esc>.");
    assert_eq!(text(&viewer), "xg(z)g(z)");
    press(&mut viewer, "u");
    assert_eq!(text(&viewer), "xg(z)");

    // The name of a snippet and Tab.
    press(&mut viewer, "ccSee fn<Tab>the note<Esc>");
    assert_eq!(text(&viewer), "See #footnote[the note]");
    press(&mut viewer, "cc$ ff<Tab>a<Esc>");
    assert_eq!(text(&viewer), "$ frac(a, ) $");
    // Elsewhere Tab is a tab's worth of blanks.
    press(&mut viewer, "ccab<Tab>c<Esc>");
    assert_eq!(text(&viewer), "ab  c");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn typing_brings_up_what_could_be_typed() {
    let text = "#let width = 3cm\n#set heading(numbering: \"1.\")\n= Start <start>\nText.\n\n";
    let (mut viewer, directory) = editing(text);
    let labels = |viewer: &Viewer| -> Vec<String> {
        let completion = viewer.completion.as_ref();
        completion.map_or_else(Vec::new, |completion| {
            completion
                .candidates
                .iter()
                .map(|entry| entry.candidate.label.clone())
                .collect()
        })
    };
    let last = |viewer: &Viewer| viewer.text().unwrap().lines().last().unwrap().to_owned();
    // Behind a hash: functions, and what the document defines.
    press(&mut viewer, "Go#");
    assert!(labels(&viewer).len() > 20, "{:?}", labels(&viewer));
    press(&mut viewer, "wi");
    assert_eq!(labels(&viewer).first().map(String::as_str), Some("width"));
    press(&mut viewer, "<Tab>");
    assert_eq!(last(&viewer), "#width");
    assert!(viewer.completion.is_none());
    // A function comes with its brackets, the cursor between them.
    press(&mut viewer, " and #ali");
    assert!(labels(&viewer).contains(&"align".to_owned()));
    press(&mut viewer, "<C-n><C-p><Tab>center<Esc>");
    assert_eq!(last(&viewer), "#width and #align(center)");
    // One undo step with the rest of what was typed, and `.` types
    // it again.
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some(text));
    press(&mut viewer, ".");
    assert_eq!(last(&viewer), "#width and #align(center)");
    // In running text nothing comes up unasked; asked, it does; and
    // it goes away again.
    press(&mut viewer, "Goplain wor");
    assert_eq!(labels(&viewer), Vec::<String>::new());
    press(&mut viewer, "<Esc>o@st");
    assert_eq!(labels(&viewer), ["start"]);
    press(&mut viewer, "<C-e>");
    assert!(viewer.completion.is_none());
    press(&mut viewer, "<C-n>");
    assert_eq!(labels(&viewer), ["start"]);
    press(&mut viewer, "<C-y><Esc>");
    assert_eq!(last(&viewer), "@start");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn typst_tells_what_a_thing_is_and_where_it_is_from() {
    let text = "#let width = 3cm\n= Start\n#rect(width: width)\n\n== Sub part\nEnd.\n";
    let (mut viewer, directory) = editing(text);
    let line = |viewer: &Viewer| viewer.editor.as_ref().unwrap().position().0;
    press(&mut viewer, "/rect<CR>K");
    let told = viewer.tooltip.clone().unwrap();
    assert!(told.to_lowercase().contains("rectangle"), "{told}");
    press(&mut viewer, "l");
    assert_eq!(viewer.tooltip, None, "until the next key");
    // To the definition, and back.
    press(&mut viewer, "$hgd");
    assert_eq!(viewer.editor.as_ref().unwrap().position(), (1, 6));
    press(&mut viewer, "``");
    assert_eq!(line(&viewer), 3);
    press(&mut viewer, "/rect<CR>gd");
    assert!(viewer.message.as_ref().unwrap().text.contains("built in"));

    // The list of headings: narrowed by typing, Enter goes there.
    press(&mut viewer, "go");
    assert!(viewer.is_picking());
    press(&mut viewer, "x");
    assert_eq!(viewer.text(), Some(text), "keys are for the list");
    press(&mut viewer, "<BS>sub<CR>");
    assert!(!viewer.is_picking());
    assert_eq!(line(&viewer), 5);
    press(&mut viewer, "go<Esc>");
    assert_eq!(line(&viewer), 5);
    press(&mut viewer, ":labels<CR>");
    assert!(!viewer.is_picking(), "there are none");
    let _ = std::fs::remove_dir_all(directory);
}

/// A document of three files: a main one that includes two chapters.
fn chapters() -> (Viewer, PathBuf) {
    let main = "#set page(width: 300pt, height: 200pt)\n\
                = Thesis\n\
                Opening words.\n\
                #include \"one.typ\"\n\
                #include \"parts/two.typ\"\n\n\
                Closing words.\n";
    let directory = directory(main);
    std::fs::create_dir_all(directory.join("parts")).unwrap();
    std::fs::write(directory.join("one.typ"), "== One\nFirst chapter text.\n").unwrap();
    let two = "== Two\nSecond chapter text.\n";
    std::fs::write(directory.join("parts/two.typ"), two).unwrap();
    (open(&directory).0, directory)
}

#[test]
fn the_files_of_a_document_are_edited_together() {
    let (mut viewer, directory) = chapters();
    let name = |viewer: &Viewer| viewer.state().file;
    let message = |viewer: &Viewer| viewer.message.as_ref().unwrap().text.clone();
    assert_eq!(name(&viewer), "main.typ");
    // To the file named under the cursor, and back.
    press(&mut viewer, "/one<CR>gf");
    assert_eq!(name(&viewer), "one.typ");
    assert_eq!(viewer.text(), Some("== One\nFirst chapter text.\n"));
    press(&mut viewer, "<C-^>");
    assert_eq!(name(&viewer), "main.typ");
    assert_eq!(
        viewer.editor.as_ref().unwrap().position().0,
        4,
        "where it was"
    );
    // What is typed in one shows in the document, saved or not.
    press(&mut viewer, ":e parts/two.typ<CR>");
    assert_eq!(name(&viewer), "parts/two.typ");
    press(&mut viewer, "GAAdded.<Esc>");
    compile(&mut viewer);
    assert_eq!(viewer.state().problems, Vec::<String>::new());
    let on_disk = std::fs::read_to_string(directory.join("parts/two.typ")).unwrap();
    assert!(!on_disk.contains("Added"));
    // An agent asking what is at a place on the page is told the file.
    let (page, x, y) = viewer.page_of(2, 1).unwrap();
    let shown = viewer.source_of(page, x + 1.0, y + 2.0);
    assert_eq!(shown, Ok(("parts/two.typ".to_owned(), 2, 1)));
    // Registers go along from file to file.
    press(&mut viewer, "yiW:b main<CR>GP");
    assert!(viewer.text().unwrap().contains("Added."));
    assert_eq!(name(&viewer), "main.typ");
    // The list says what is open and what is unwritten.
    press(&mut viewer, ":ls<CR>");
    assert_eq!(
        message(&viewer),
        "1% main.typ [+]   2 one.typ   3 parts/two.typ [+]"
    );
    press(&mut viewer, ":q<CR>");
    assert!(!viewer.wants_quit());
    assert!(message(&viewer).contains("main.typ, parts/two.typ"));
    press(&mut viewer, ":wa<CR>");
    let on_disk = std::fs::read_to_string(directory.join("parts/two.typ")).unwrap();
    assert!(on_disk.contains("Added."));
    assert_eq!(viewer.unwritten(), Vec::<String>::new());
    // Round the open files, and to one by its number.
    press(&mut viewer, ":bn<CR>");
    assert_eq!(name(&viewer), "one.typ");
    press(&mut viewer, ":bp<CR>:bp<CR>");
    assert_eq!(name(&viewer), "parts/two.typ");
    press(&mut viewer, ":b 1<CR>");
    assert_eq!(name(&viewer), "main.typ");
    // What is not there is not made by accident.
    press(&mut viewer, ":e three.typ<CR>");
    assert!(message(&viewer).contains(":e! makes it"));
    press(&mut viewer, ":e! three.typ<CR>");
    assert_eq!(name(&viewer), "three.typ");
    assert!(directory.join("three.typ").is_file());
    press(&mut viewer, ":q<CR>");
    assert!(viewer.wants_quit());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_page_leads_into_the_file_its_text_is_from() {
    let (mut viewer, directory) = chapters();
    let name = |viewer: &Viewer| viewer.state().file;
    let word = |viewer: &Viewer| {
        let editor = viewer.editor.as_ref().unwrap();
        let rest = &editor.text()[editor.cursor()..];
        rest.split_whitespace().next().unwrap_or("").to_owned()
    };
    // Down the page from the main file: into the first chapter,
    // through the second and out again.
    press(&mut viewer, "gsgs/Opening<CR>");
    let mut walked = vec![(name(&viewer), word(&viewer))];
    for _ in 0..12 {
        press(&mut viewer, "j");
        let now = (name(&viewer), word(&viewer));
        if walked.last() == Some(&now) {
            break;
        }
        walked.push(now);
    }
    let walked: Vec<(&str, &str)> = walked
        .iter()
        .map(|(file, word)| (file.as_str(), word.as_str()))
        .collect();
    assert_eq!(
        walked,
        [
            ("main.typ", "Opening"),
            ("one.typ", "One"),
            ("one.typ", "First"),
            ("parts/two.typ", "Two"),
            ("parts/two.typ", "Second"),
            ("main.typ", "Closing"),
        ]
    );
    // And up again.
    press(&mut viewer, "kk");
    assert_eq!(
        (name(&viewer), word(&viewer)),
        ("parts/two.typ".to_owned(), "Two".to_owned())
    );
    // A click on text of another file goes to that file.
    press(&mut viewer, ":b main<CR>/Closing<CR>");
    let cursor = tints(&mut viewer, PAGE_CURSOR);
    assert_eq!(cursor.len(), 1);
    press(&mut viewer, "kk");
    let other = tints(&mut viewer, PAGE_CURSOR);
    assert_eq!(name(&viewer), "parts/two.typ");
    press(&mut viewer, ":b main<CR>");
    let (x, y) = (f64::from(other[0].x) + 2.0, f64::from(other[0].y) + 3.0);
    viewer.click((x, y));
    assert_eq!(
        (name(&viewer), word(&viewer)),
        ("parts/two.typ".to_owned(), "Two".to_owned())
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn pages_are_shown_as_slides_and_all_at_once() {
    let text = "#set page(width: 320pt, height: 180pt)\n\
                = One\nFirst.\n#pagebreak()\n\
                = Two\nSecond.\n#pagebreak()\n\
                = Three\nThird.\n";
    let (mut viewer, directory) = editing(text);
    let line = |viewer: &Viewer| viewer.editor.as_ref().unwrap().position().0;
    let visible = |viewer: &Viewer| -> Vec<usize> {
        let pages = viewer.view.visible(&viewer.layout);
        pages.iter().map(|page| page.index).collect()
    };
    press(&mut viewer, "gsgs");
    assert!(visible(&viewer).len() > 1, "one under the other");
    // One at a time, whole, wherever the cursor is.
    press(&mut viewer, ":slides<CR>");
    assert_eq!(viewer.paging(), Paging::Slides);
    assert_eq!(visible(&viewer), [0]);
    let page = viewer.layout.pages[0];
    assert!(page.height <= viewer.view.height && page.width <= viewer.view.width);
    press(&mut viewer, "/Second<CR>");
    assert_eq!(visible(&viewer), [1]);
    // To the next page and the one before, the cursor along.
    press(&mut viewer, "]]");
    assert_eq!((visible(&viewer), line(&viewer)), (vec![2], 8));
    press(&mut viewer, "]]");
    assert_eq!(visible(&viewer), [2], "the last one");
    press(&mut viewer, "[[[[");
    assert_eq!((visible(&viewer), line(&viewer)), (vec![0], 2));
    // A push at the view is the next slide, not half of one.
    viewer.scroll(0.0, 30.0);
    assert_eq!(visible(&viewer), [0]);
    viewer.scroll(0.0, 30.0);
    assert_eq!(visible(&viewer), [1]);
    viewer.scroll(0.0, -80.0);
    assert_eq!(visible(&viewer), [0]);

    // The overview: keys pick a page, Enter goes to it.
    press(&mut viewer, ":grid<CR>");
    assert!(viewer.is_grid());
    let scene = viewer.scene(&|_| false);
    assert_eq!(scene.wanted.len(), 3, "all pages are asked for, small");
    assert!(scene.wanted.iter().all(|tile| tile.region.width < 400));
    press(&mut viewer, "x");
    assert_eq!(viewer.text(), Some(text), "keys are for the overview");
    press(&mut viewer, "ll<CR>");
    assert!(!viewer.is_grid());
    assert_eq!((visible(&viewer), line(&viewer)), (vec![2], 8));
    press(&mut viewer, ":grid<CR>h<Esc>");
    assert_eq!(visible(&viewer), [2], "Escape leaves things as they were");
    // And scrolling again.
    press(&mut viewer, ":slides<CR>");
    assert_eq!(viewer.paging(), Paging::Scroll);
    press(&mut viewer, "gg]]");
    assert_eq!(line(&viewer), 5);
    assert!(visible(&viewer).contains(&1));
    press(&mut viewer, ":pdf<CR>");
    let pdf = std::fs::read(directory.join("main.pdf")).unwrap();
    assert!(pdf.starts_with(b"%PDF"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_users_own_keys_stand_for_others() {
    let (mut viewer, directory) = editing("a;b and more\n");
    let (keymap, complaint) = Keymap::parse(
        [(";", ":"), (",w", ":w<CR>"), ("<C-s>", ":w<CR>")],
        [("<C-s>", "<Esc>:w<CR>")],
    );
    assert_eq!(complaint, None);
    viewer.set_keymap(keymap);
    let saved = |directory: &Path| std::fs::read_to_string(directory.join("main.typ")).unwrap();
    // One key for another.
    press(&mut viewer, ";");
    assert_eq!(viewer.state().editing.unwrap().mode, editor::Mode::Command);
    press(&mut viewer, "<Esc>");
    // In the middle of a command a key is what it is.
    press(&mut viewer, "f;");
    assert_eq!(viewer.editor.as_ref().unwrap().cursor(), 1);
    // Several keys: waited for, and typed as they are if they do not
    // become the mapping.
    press(&mut viewer, "x,");
    assert_eq!(viewer.text(), Some("ab and more\n"));
    press(&mut viewer, "w");
    assert_eq!(saved(&directory), "ab and more\n");
    press(&mut viewer, ",x");
    assert_eq!(viewer.text(), Some("a and more\n"));
    // While typing, at once.
    press(&mut viewer, "Anew<C-s>");
    assert_eq!(saved(&directory), "a and morenew\n");
    assert_eq!(viewer.state().editing.unwrap().mode, editor::Mode::Normal);
    // What a mapping stands for is not looked up again.
    let (keymap, _) = Keymap::parse([("x", "dd"), ("dd", "x")], []);
    viewer.set_keymap(keymap);
    press(&mut viewer, "x");
    assert_eq!(viewer.text(), Some(""));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
#[allow(clippy::too_many_lines, reason = "one walk over a page")]
fn on_the_page_vim_goes_by_what_is_shown() {
    let text = "#set page(width: 300pt, height: 300pt)\n\
                #set heading(numbering: \"1.\")\n\
                = Start <start>\n\
                The *quick* brown fox jumps over the lazy dog and runs on \
                through the wood, as @start says, until the line is full \
                and then some more of it follows here.\n\n\
                Second paragraph.\n";
    let (mut viewer, directory) = editing(text);
    let under = |viewer: &Viewer| {
        let editor = viewer.editor.as_ref().unwrap();
        editor.text()[editor.cursor()..].chars().next().unwrap()
    };
    let paragraph = |viewer: &Viewer| viewer.text().unwrap().lines().nth(3).unwrap().to_owned();
    press(&mut viewer, "gsgs/The<CR>");
    // Character by character: over the marks, which show as nothing,
    // and with the cursor on the page all the way.
    let mut seen = String::new();
    for _ in 0..12 {
        seen.push(under(&viewer));
        assert_eq!(tints(&mut viewer, PAGE_CURSOR).len(), 1, "after {seen:?}");
        press(&mut viewer, "l");
    }
    assert_eq!(seen, "The quick br");
    press(&mut viewer, "hhhhhhhhh");
    assert_eq!(under(&viewer), ' ', "and back over them");
    press(&mut viewer, "h");
    assert_eq!(under(&viewer), 'e');
    // Backspace takes what is shown before the cursor, not the mark.
    press(&mut viewer, "/brown<CR>hi<BS><Esc>");
    assert!(paragraph(&viewer).starts_with("The *quic* brown"));
    press(&mut viewer, "u");

    // What a reference shows is one thing: stepped over, deleted whole.
    press(&mut viewer, "/\\=@start<CR>");
    assert_eq!(under(&viewer), '@');
    press(&mut viewer, "l");
    assert_eq!(under(&viewer), ' ', "past all of it");
    press(&mut viewer, "hx");
    assert!(
        paragraph(&viewer).contains("as  says"),
        "{}",
        paragraph(&viewer)
    );
    press(&mut viewer, "u");
    compile(&mut viewer);

    // The start and the end of a line are those of the line shown.
    press(&mut viewer, "/jumps<CR>");
    let line = {
        let lines = PageRows {
            document: viewer.document.as_ref().unwrap(),
            world: viewer.compiled_world.as_ref().unwrap(),
            file: viewer.file_id(),
            lines: &viewer.page_lines,
            cursor_page: &viewer.cursor_page,
            change: Change::NONE,
            landed: &viewer.landed,
            elsewhere: &viewer.elsewhere,
        };
        let cursor = viewer.editor.as_ref().unwrap().cursor();
        lines.line(cursor).unwrap()
    };
    let shown = &viewer.text().unwrap()[line.start..line.end].to_owned();
    assert!(
        shown.starts_with("The *quick*") && shown.len() < 70,
        "{shown}"
    );
    let cursor = |viewer: &Viewer| viewer.editor.as_ref().unwrap().cursor();
    press(&mut viewer, "$");
    assert_eq!(cursor(&viewer), line.last);
    press(&mut viewer, "0");
    assert_eq!(cursor(&viewer), line.start);
    // `V` takes the paragraph: the lines the page sets it in are one
    // line of the text to whoever wrote it, as a line Vim wraps is
    // one to `V`. `j` takes the next paragraph with it.
    press(&mut viewer, "/jumps<CR>V");
    assert_eq!(
        viewer.editor.as_ref().unwrap().mode(),
        editor::Mode::VisualLine
    );
    let whole = viewer.editor.as_ref().unwrap().selection().unwrap();
    assert_eq!(whole.start, line.start);
    assert!(
        whole.end > line.end + 20 && whole.end <= text.find("\n\nSecond").unwrap(),
        "{whole:?}"
    );
    press(&mut viewer, "j");
    let two = viewer.editor.as_ref().unwrap().selection().unwrap();
    assert_eq!(two.start, whole.start);
    assert!(two.end > text.find("Second paragraph").unwrap(), "{two:?}");
    press(&mut viewer, "<Esc>");
    // `dd` takes the paragraph: that is one line to whoever wrote
    // it, as a line Vim wraps is one to `dd`. The row shown, taken
    // out of its paragraph, could be put back nowhere but into a
    // word.
    press(&mut viewer, "/jumps<CR>dd");
    let rest = viewer.text().unwrap().to_owned();
    assert!(
        !rest.contains("jumps")
            && !rest.contains("follows here.")
            && rest.contains("Second paragraph."),
        "{rest}"
    );
    press(&mut viewer, "u");
    compile(&mut viewer);
    // At its end and at its start.
    press(&mut viewer, "/jumps<CR>A!<Esc>");
    assert_eq!(&viewer.text().unwrap()[line.end..=line.end], "!");
    press(&mut viewer, "u");
    compile(&mut viewer);
    press(&mut viewer, "/jumps<CR>I>><Esc>");
    assert_eq!(&viewer.text().unwrap()[line.start..line.start + 2], ">>");
    press(&mut viewer, "u");
    compile(&mut viewer);

    // A new paragraph below, and Enter ends one.
    press(&mut viewer, "/jumps<CR>oNew one.<CR>And another.<Esc>");
    assert!(
        viewer
            .text()
            .unwrap()
            .contains("follows here.\n\nNew one.\n\nAnd another.\n\nSecond paragraph.")
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn things_are_put_in_from_a_menu() {
    let (mut viewer, directory) = editing("Some text here.\n\n");
    let labels = |viewer: &Viewer| -> Vec<String> {
        let completion = viewer.completion.as_ref();
        completion.map_or_else(Vec::new, |completion| {
            let label = |entry: &assist::Entry| entry.candidate.label.clone();
            completion.candidates.iter().map(label).collect()
        })
    };
    let lines = |viewer: &Viewer| -> Vec<String> {
        let text = viewer.text().unwrap();
        text.lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    };
    // Behind a hash: what can be put in, narrowed by what is typed.
    press(&mut viewer, "Go#");
    assert_eq!(
        labels(&viewer)[..3],
        ["equation", "numbered equation", "inline equation"]
    );
    press(&mut viewer, "num");
    assert_eq!(labels(&viewer)[0], "numbered equation");
    press(&mut viewer, "<CR>x^2<Esc>");
    assert_eq!(
        lines(&viewer)[1],
        "#math.equation(block: true, numbering: \"(1)\", $ x^2 $)"
    );
    compile(&mut viewer);
    assert_eq!(viewer.state().problems, Vec::<String>::new());
    // What stands by itself parts the paragraph it is put into.
    press(&mut viewer, "gg/text<CR>i#fig<Tab>cat.png<Esc>");
    assert_eq!(
        lines(&viewer)[..6],
        [
            "Some ",
            "#figure(",
            "  image(\"cat.png\"),",
            "  caption: [],",
            ")",
            "text here."
        ]
    );
    // What stands in a line does not.
    press(&mut viewer, "G$a#foot<CR>a note<Esc>");
    assert!(viewer.text().unwrap().contains("$)#footnote[a note]"));
    // In an equation the names come up as they are typed: no hash.
    press(&mut viewer, "Go$sq");
    assert_eq!(labels(&viewer)[0], "square root");
    press(&mut viewer, "<Tab>2<Esc>");
    assert_eq!(lines(&viewer).last().map(String::as_str), Some("$sqrt(2)$"));
    press(&mut viewer, "i + fra");
    assert_eq!(labels(&viewer)[0], "fraction");
    press(&mut viewer, "<C-e><Esc>");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn what_is_typed_shows_where_it_is_typed() {
    let text = "#set page(width: 300pt, height: 300pt)\n\
                = Title\n\
                Some words and $x^2 + y^2$ in a line.\n";
    let (mut viewer, directory) = editing(text);
    // What is laid over the page is for when the part that changed
    // cannot be typeset by itself and put into it. The numbers
    // beside the lines are laid over it too, and not meant here.
    viewer.in_parts = false;
    viewer.numbers = false;
    // The images laid over the page that are neither its tiles nor a
    // colour: what was typeset in place.
    let pieces = |viewer: &mut Viewer| -> Vec<Quad> {
        let scene = viewer.scene(&|_| false);
        let drawn = scene.images.iter().filter(|image| image.pixmap.width() > 1);
        let ours: HashSet<TextureId> = drawn.map(|image| image.id).collect();
        // Not the status line, which is as wide as the window.
        let large = |quad: &&Quad| ours.contains(&quad.texture) && quad.width < 900.0;
        scene.quads.iter().filter(large).copied().collect()
    };
    let bars = |viewer: &mut Viewer| tints(viewer, PAGE_BAR);
    // A document that takes long to typeset: one that does not is
    // typeset anew instead.
    viewer.stats.compile_ms = 10.0 * QUICK_COMPILE_MS;
    press(&mut viewer, "gsgs/Title<CR>");
    let heading = tints(&mut viewer, PAGE_CURSOR)[0];
    // Words typed into a heading are not worked into the page: they
    // are set by themselves and laid where they are typed, in the
    // heading's size, with the cursor behind them.
    press(&mut viewer, "A more");
    assert!(!viewer.is_predicted());
    let typed = pieces(&mut viewer);
    assert_eq!(typed.len(), 1, "{typed:?}");
    assert!(
        (typed[0].y - heading.y).abs() < 2.0,
        "on the heading's line"
    );
    assert!(
        (typed[0].height - heading.height).abs() < 2.0,
        "and as high"
    );
    assert!(typed[0].x > heading.x + 3.0 * heading.width);
    let bar = bars(&mut viewer);
    assert_eq!(bar.len(), 1);
    assert!(
        (bar[0].x - (typed[0].x + typed[0].width)).abs() < 3.0,
        "{bar:?}"
    );
    // Wider with every key.
    press(&mut viewer, " words");
    assert!(pieces(&mut viewer)[0].width > typed[0].width + 10.0);
    press(&mut viewer, "<Esc>");
    compile(&mut viewer);
    assert_eq!(pieces(&mut viewer), [], "the page has it now");

    // A new paragraph: under the one it follows, at its left edge.
    viewer.stats.compile_ms = 10.0 * QUICK_COMPILE_MS;
    press(&mut viewer, "/Some<CR>");
    let first = tints(&mut viewer, PAGE_CURSOR)[0];
    press(&mut viewer, "oNext *one*");
    let typed = pieces(&mut viewer);
    assert_eq!(typed.len(), 1);
    assert!((typed[0].x - first.x).abs() < 2.0, "{typed:?} {first:?}");
    assert!(typed[0].y > first.y + first.height);
    assert!(typed[0].y < first.y + 4.0 * first.height);
    // Half an equation is set as one, with what it lacks.
    press(&mut viewer, " and $a^");
    let half = pieces(&mut viewer);
    assert_eq!(half.len(), 1);
    assert!(half[0].width > typed[0].width);
    assert_eq!(bars(&mut viewer).len(), 1);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_delete_key_deletes_forwards() {
    let (mut viewer, directory) = editing("abc *def* ghi\njkl\n");
    // What is under the cursor; what is selected; while typing, what
    // is behind the cursor, the end of a line too.
    press(&mut viewer, "<Del>");
    assert_eq!(viewer.text(), Some("bc *def* ghi\njkl\n"));
    press(&mut viewer, "vl<Del>");
    assert_eq!(viewer.text(), Some(" *def* ghi\njkl\n"));
    press(&mut viewer, "A<Del>!<Esc>");
    assert_eq!(viewer.text(), Some(" *def* ghi!jkl\n"));
    press(&mut viewer, "0i<Del><Del><Esc>");
    assert_eq!(viewer.text(), Some("def* ghi!jkl\n"));
    // At the very end there is nothing to take.
    press(&mut viewer, "A<Del><Esc>u");
    assert_eq!(viewer.text(), Some(" *def* ghi!jkl\n"));
    // On the page, while typing: what is shown behind the cursor, not
    // a mark that shows as nothing.
    compile(&mut viewer);
    press(&mut viewer, "gsgs0i<Del><Esc>");
    assert_eq!(viewer.text(), Some(" *ef* ghi!jkl\n"));
    let _ = std::fs::remove_dir_all(directory);
}

/// Whether `j` and `k` get through a whole document on its pages:
/// `ZETWERK_BENCH=file.typ cargo test --release -p zetwerk-app walking_the_pages -- --ignored --nocapture`
#[test]
#[ignore = "a measurement, not a test"]
fn walking_the_pages() {
    let Ok(file) = std::env::var("ZETWERK_BENCH") else {
        return;
    };
    let directory = directory(&std::fs::read_to_string(file).unwrap());
    let (mut viewer, _) = open(&directory);
    let cursor = |viewer: &Viewer| viewer.editor.as_ref().unwrap().cursor();
    let lines: usize = {
        let document = viewer.document.as_ref().unwrap();
        let world = viewer.compiled_world.as_ref().unwrap();
        let count = |page: &Page| PageMap::build(world, &page.frame).lines().len();
        document.pages().iter().map(count).sum()
    };
    let pages = viewer.document().unwrap().pages().len();
    press(&mut viewer, "gsgs");
    for key in ["j", "k"] {
        let start = std::time::Instant::now();
        let mut seen = HashSet::from([cursor(&viewer)]);
        let (mut steps, mut again) = (0, 0);
        let mut trail = Vec::new();
        loop {
            let before = cursor(&viewer);
            press(&mut viewer, key);
            if cursor(&viewer) == before {
                break;
            }
            steps += 1;
            again += usize::from(!seen.insert(cursor(&viewer)));
            trail.push((viewer.cursor_page.get() + 1, cursor(&viewer)));
            if steps > 4 * lines + 100 {
                let text = viewer.text().unwrap();
                for (page, at) in &trail[trail.len() - 40..] {
                    let shown: String = text[*at..].chars().take(30).collect();
                    println!("page {page}: {at} {shown:?}");
                }
                panic!("going round in circles");
            }
        }
        println!(
            "{key}: {steps} steps over {lines} lines, {again} to a place seen before, \
             ends on page {} of {pages}, {:.3} ms a step",
            viewer.cursor_page.get() + 1,
            start.elapsed().as_secs_f64() * 1000.0 / steps.max(1) as f64
        );
    }
    let _ = std::fs::remove_dir_all(directory);
}

/// What a key costs that changes more than a word of a line (an
/// equation, markup, a new paragraph), on a long document:
/// `ZETWERK_BENCH=file.typ cargo test --release -p zetwerk-app typing_in_parts_cost -- --ignored --nocapture`
#[test]
#[ignore = "a measurement, not a test"]
fn typing_in_parts_cost() {
    let Ok(file) = std::env::var("ZETWERK_BENCH") else {
        return;
    };
    let directory = directory(&std::fs::read_to_string(file).unwrap());
    let (mut viewer, _) = open(&directory);
    viewer.resize((1920, 1200), 1.25);
    let started = std::time::Instant::now();
    compile(&mut viewer);
    println!(
        "all of it, the first time: {:.1} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
    let mut held = HashSet::new();
    let mut frame = |viewer: &mut Viewer| {
        let scene = viewer.scene(&|id| held.contains(&id));
        held.extend(scene.images.iter().map(|image| image.id));
    };
    press(&mut viewer, "gsgs");
    let lines = viewer.text().map_or(0, |text| text.lines().count());
    press(&mut viewer, &format!("{}G", lines / 2));
    frame(&mut viewer);
    for (name, keys) in [
        (
            "an equation",
            "A Then $x_i^2 + ff<Tab>a<Tab>b<Tab> = alpha<Tab> holds.",
        ),
        ("strong words", " And <C-b>strong words<C-b> follow."),
        (
            "a paragraph",
            "<CR>A new paragraph begins here, with words.",
        ),
    ] {
        let keys = keys::parse(keys).unwrap();
        let (mut key_time, mut frame_time, mut worst) =
            (Duration::ZERO, Duration::ZERO, Duration::ZERO);
        let (local, declined) = (viewer.stats.local, viewer.stats.declined);
        for &key in &keys {
            let start = std::time::Instant::now();
            viewer.key(key);
            let took = start.elapsed();
            key_time += took;
            worst = worst.max(took);
            if std::env::var_os("ZETWERK_KEYS").is_some() {
                println!(
                    "    {key:?}: {:.2} ms, {} ms in parts, {:?}",
                    took.as_secs_f64() * 1000.0,
                    viewer.stats.local_ms.round(),
                    viewer.declined
                );
            }
            let start = std::time::Instant::now();
            frame(&mut viewer);
            frame_time += start.elapsed();
        }
        let per = |total: Duration| total.as_secs_f64() * 1000.0 / keys.len() as f64;
        println!(
            "{name:>14}: key {:.2} ms (worst {:.2}), frame {:.2} ms; {} keys, {} in parts, {} had to wait ({:?})",
            per(key_time),
            worst.as_secs_f64() * 1000.0,
            per(frame_time),
            keys.len(),
            viewer.stats.local - local,
            viewer.stats.declined - declined,
            viewer.declined
        );
    }
    let started = std::time::Instant::now();
    compile(&mut viewer);
    println!(
        "all of it, again: {:.1} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
}

/// What a keystroke costs before the compositor gets the frame:
/// `ZETWERK_BENCH=file.typ cargo test --release -p zetwerk-app typing_cost -- --ignored --nocapture`
#[test]
#[ignore = "a measurement, not a test"]
fn typing_cost() {
    let Ok(file) = std::env::var("ZETWERK_BENCH") else {
        return;
    };
    let directory = directory(&std::fs::read_to_string(file).unwrap());
    let (mut viewer, _) = open(&directory);
    viewer.resize((1920, 1200), 1.25);
    let mut held = HashSet::new();
    let mut frame = |viewer: &mut Viewer| {
        let scene = viewer.scene(&|id| held.contains(&id));
        held.extend(scene.images.iter().map(|image| image.id));
        scene.images.len()
    };
    press(&mut viewer, "300Gw");
    frame(&mut viewer);
    let mut report = |name: &str, keys: &str, viewer: &mut Viewer| {
        let keys = keys::parse(keys).unwrap();
        let (mut key_time, mut frame_time, mut images) = (Duration::ZERO, Duration::ZERO, 0);
        for &key in &keys {
            let start = std::time::Instant::now();
            viewer.key(key);
            key_time += start.elapsed();
            let start = std::time::Instant::now();
            images += frame(viewer);
            frame_time += start.elapsed();
        }
        let per = |total: Duration| total.as_secs_f64() * 1000.0 / keys.len() as f64;
        println!(
            "{name:>18}: key {:.3} ms, frame {:.3} ms, {:.1} images per key",
            per(key_time),
            per(frame_time),
            images as f64 / keys.len() as f64
        );
    };
    press(&mut viewer, "gsgs");
    report(
        "lines on the page",
        "jjjjjjjjjjkkkkkkkkkkjjjjjjjjjj",
        &mut viewer,
    );
    press(&mut viewer, "gs");
    report(
        "typing",
        "ithe quick brown fox jumps over the lazy dog and on",
        &mut viewer,
    );
    report("leaving insert", "<Esc>", &mut viewer);
    report(
        "moving in a line",
        "hhhhhhhhhhllllllllllwwwwwbbbbb",
        &mut viewer,
    );
    report(
        "moving by lines",
        "jjjjjjjjjjkkkkkkkkkkjjjjjjjjjj",
        &mut viewer,
    );
    report("with a list up", "A #figu<C-e><Esc>", &mut viewer);
    report("half a page down", "<C-d>", &mut viewer);
    report("deleting words", "dwdwdwdwdwdwdwdw", &mut viewer);
    report("undo", "uuuuuuuu", &mut viewer);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_string_left_open_in_an_equation_does_not_take_the_document_with_it() {
    let text = "#set page(width: 300pt, height: 300pt)\n\
                #set heading(numbering: \"1.\")\n\
                Before.\n\nLet $x_0$ be real, see @later.\n\n= Later <later>\nWords.\n";
    let (mut viewer, directory) = editing(text);
    // The quote that closed the string is taken away: to Typst the
    // string then goes on to the end of the document, label and all.
    press(&mut viewer, "/Before<CR>A Then $a + \"<Del> b");
    assert!(
        viewer
            .text()
            .is_some_and(|text| text.contains("Then $a + \" b$\n\nLet $x_0$")),
        "{:?}",
        viewer.text()
    );
    compile(&mut viewer);
    compile(&mut viewer);
    // Only the equation is shown as it is written; what follows its
    // paragraph is typeset as ever.
    assert!(viewer.written.is_some(), "{:?}", viewer.problems);
    assert_eq!(
        viewer.document_revision,
        Some(viewer.revision),
        "{:?}",
        viewer.problems
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn an_equation_with_a_number_keeps_it_while_it_is_shown_as_written() {
    let text = "#set page(width: 300pt, height: 300pt)\n\
                #set math.equation(numbering: \"(1)\")\n\
                Before.\n\n$ a + b $ <sum>\n\nSee @sum for it.\n";
    let (mut viewer, directory) = editing(text);
    let meant = glyph_places(&viewer, 0).len();
    press(&mut viewer, "gsgs/\\=a +<CR>gx");
    compile(&mut viewer);
    // What refers to it still finds it: the pages are made.
    assert!(viewer.written.is_some());
    assert_eq!(
        viewer.document_revision,
        Some(viewer.revision),
        "{:?}",
        viewer.problems
    );
    assert!(
        glyph_places(&viewer, 0).len() > meant + 3,
        "its marks are shown"
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn deleting_at_the_end_of_a_paragraph_takes_nothing_of_the_next() {
    let text = "#set page(width: 300pt, height: 300pt)\n\
                Before.\n\nLet $x_0$ be real.\n";
    let (mut viewer, directory) = editing(text);
    // In the string just begun nothing is shown behind the cursor
    // but the next paragraph, beyond marks that show as nothing.
    press(&mut viewer, "gsgs/Before<CR>A Then $a + \"");
    compile(&mut viewer);
    press(&mut viewer, "<Del>");
    assert!(
        viewer
            .text()
            .is_some_and(|text| text.contains("Then $a + \"\"$\n\nLet $x_0$")),
        "{:?}",
        viewer.text()
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_mark_that_takes_the_end_of_an_equation_with_it_ends_nothing_else() {
    let text = "#set page(width: 300pt, height: 300pt)\n\
                Before.\n\nLet $x_0$ be real, and $y$ too.\n\nAnd $z$ as well.\n";
    let (mut viewer, directory) = editing(text);
    // The backslash makes the `$` behind it a character: to Typst
    // the equation goes on to the next one's beginning, that one's
    // text is no equation, and what stands between them is one.
    press(&mut viewer, "gsgs/Before<CR>A Then $a + b ");
    // Typed on the page, a backslash begins a LaTeX name (see
    // `latex`); the sign itself comes as the source pane puts it.
    if let Some(editor) = viewer.editor.as_mut() {
        let at = editor.cursor();
        let _ = editor.replace(&[(at..at, "\\")]);
    }
    viewer.text_changed(Edited::Typing);
    assert!(
        viewer
            .text()
            .is_some_and(|text| text.contains("Then $a + b \\$\n\nLet $x_0$")),
        "{:?}",
        viewer.text()
    );
    compile(&mut viewer);
    compile(&mut viewer);
    // The equation being typed is shown as written, whatever else
    // Typst complained of; all else is typeset as ever.
    assert!(viewer.written.is_some(), "{:?}", viewer.problems);
    assert_eq!(
        viewer.document_revision,
        Some(viewer.revision),
        "{:?}",
        viewer.problems
    );
    // With something behind it the mark is that, and the equation
    // an equation.
    press(&mut viewer, "a");
    compile(&mut viewer);
    assert_eq!(viewer.written, None);
    assert_eq!(
        viewer.document_revision,
        Some(viewer.revision),
        "{:?}",
        viewer.problems
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_blank_typed_in_a_footnote_leaves_the_view_where_it_is() {
    // The note's number stands at the top of a long page, its text
    // at the foot, out of the window while the top is shown.
    let text = "#set page(width: 300pt, height: 900pt)\n\
                First words of the page.\n\nMore of them.\n";
    let (mut viewer, directory) = editing(text);
    let settle = |viewer: &mut Viewer| {
        compile(viewer);
        let _ = viewer.scene(&|_| false);
        viewer.view.top
    };
    // A note begun from the menu, and a word typed into it.
    press(&mut viewer, "gsgs/\\=words<CR>ea#foot<CR>");
    settle(&mut viewer);
    press(&mut viewer, "word");
    let top = settle(&mut viewer);
    assert!(
        viewer
            .text()
            .is_some_and(|text| text.contains("#footnote[word]")),
        "{:?}",
        viewer.text()
    );
    assert!(top > 100.0, "the foot of the page is shown: {top}");
    // Behind the blank the cursor has no place of its own in the
    // note. It is not looked for where the note's number stands.
    for key in [" ", "n", "e", "x", "t", " "] {
        press(&mut viewer, key);
        let _ = viewer.scene(&|_| false);
        let typed = viewer.view.top;
        let typeset = settle(&mut viewer);
        assert!(
            (typed - top).abs() < 1.0 && (typeset - top).abs() < 1.0,
            "after {key:?} the view is at {typed}, typeset at {typeset}; it was at {top}"
        );
    }
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_numbers_of_the_lines_stay_while_the_pages_are_made() {
    let text = "#set page(width: 300pt, height: 400pt)\n\
                One line of words.\n\nTwo lines of them, the second.\n\n\
                Three lines.\n\nFour lines.\n\nFive lines.\n";
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, "gsgs/second<CR>");
    compile(&mut viewer);
    // How often the number 1 is drawn: beside the line above the
    // cursor's and the one below.
    let ones = |viewer: &mut Viewer| {
        let markup = crate::chrome::margin_number_markup(1, 6.5);
        let zoom = viewer.view.zoom;
        let id = viewer.chrome.render(&markup, zoom).map(|image| image.id);
        let scene = viewer.scene(&|_| false);
        let of_one = |quad: &&Quad| Some(quad.texture) == id;
        scene.quads.iter().filter(of_one).count()
    };
    assert_eq!(ones(&mut viewer), 2);
    // After every key, in the frames before its pages are there and
    // in the one in which they come: also where they are made a
    // second time of the same text, after what was typeset by
    // itself.
    for keys in [
        "A", " ", "w", "o", "r", "d", " ", "<Esc>", "x", "x", "u", "ia", "b", "<Esc>", "p", "u",
    ] {
        press(&mut viewer, keys);
        assert_eq!(ones(&mut viewer), 2, "after {keys}");
        assert_eq!(ones(&mut viewer), 2, "after {keys}, the next frame");
        compile(&mut viewer);
        assert_eq!(ones(&mut viewer), 2, "after {keys}, with its pages");
    }
    let _ = std::fs::remove_dir_all(directory);
}
