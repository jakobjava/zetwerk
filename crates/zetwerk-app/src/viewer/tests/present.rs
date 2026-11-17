//! Giving a talk from the editor (see `present`): every key of it, what
//! the screen shows while it is given, where the cursor is after it, how
//! the clock wakes the window, rehearsing; and the keys that edit a deck.

use super::*;
use crate::viewer::deck::Found;
use crate::viewer::present::{BLACK, WHITE};

/// A deck of four slides on six pages: the second slide in three steps,
/// which keep its page number, as decks that count slides do.
const DECK: &str = "#set page(width: 320pt, height: 180pt, numbering: \"1\")\n\
    One.\n#pagebreak()\n\
    Two.\n#pagebreak()\n\
    #counter(page).update(n => n - 1)\nTwo, more.\n#pagebreak()\n\
    #counter(page).update(n => n - 1)\nTwo, all.\n#pagebreak()\n\
    Three.\n#metadata((note: \"Say three slowly.\")) <speaker-note>\n#pagebreak()\n\
    Four.\n";

/// The page the talk shows.
fn shown(viewer: &Viewer) -> usize {
    viewer.presented_page().unwrap_or(usize::MAX)
}

/// The tiles of page `index` as large as the window lets it be.
fn whole_page_tiles(viewer: &Viewer, index: usize) -> Vec<Tile> {
    let document = viewer.document.as_ref().unwrap();
    let page = &document.pages()[index];
    let (width, height) = (f64::from(viewer.window.0), f64::from(viewer.window.1));
    let zoom = (width / page.frame.width().to_pt()).min(height / page.frame.height().to_pt());
    let (across, down) = page_size_px(page, zoom);
    let whole = PixelRegion {
        x: 0,
        y: 0,
        width: across,
        height: down,
    };
    tiles(page, zoom, whole)
}

fn black() -> TextureId {
    TextureId(hash128(&("solid", BLACK)))
}

#[test]
fn the_slides_of_a_deck_are_the_runs_of_its_page_numbers() {
    let (mut viewer, directory) = editing(DECK);
    let deck = viewer.deck().unwrap();
    assert_eq!(deck.found, Found::Numbers);
    assert_eq!(deck.slides, [0..1, 1..4, 4..5, 5..6]);
    assert_eq!(deck.labels, ["1", "2", "3", "4"]);
    assert_eq!(deck.notes_of(4), ["Say three slowly."]);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_talk_shows_the_slide_and_nothing_else() {
    let (mut viewer, directory) = editing(DECK);
    press(&mut viewer, "/Three<CR>:present<CR>");
    assert!(viewer.is_presenting());
    assert_eq!(shown(&viewer), 4, "the slide the cursor is on");
    assert_eq!(viewer.take_screen_wish(), Some((true, true)), "full screen");
    // Everything held: a black quad, and the tiles of the page.
    let scene = viewer.scene(&|_| true);
    let tiles = whole_page_tiles(&viewer, 4);
    assert_eq!(scene.quads.len(), 1 + tiles.len(), "nothing but the slide");
    assert_eq!(scene.quads[0].texture, black());
    let (width, height) = (viewer.window.0 as f32, viewer.window.1 as f32);
    assert_eq!(
        (scene.quads[0].width, scene.quads[0].height),
        (width, height)
    );
    for (quad, tile) in scene.quads[1..].iter().zip(&tiles) {
        assert_eq!(quad.texture, texture(tile.key));
        assert!(quad.x >= 0.0 && quad.y >= 0.0, "on the screen");
        assert!(quad.x + quad.width <= width && quad.y + quad.height <= height);
        assert_eq!(quad.x.fract(), 0.0, "on whole pixels");
    }
    // Letterboxed: as wide as the window, and in its middle.
    let right = scene.quads[1..]
        .iter()
        .map(|quad| quad.x + quad.width)
        .fold(0.0, f32::max);
    assert_eq!(right, width);
    let top = scene.quads[1].y;
    let bottom = scene.quads[1..]
        .iter()
        .map(|quad| quad.y + quad.height)
        .fold(0.0, f32::max);
    assert!((top - (height - bottom)).abs() <= 1.0, "{top} {bottom}");
    // Nothing held: the page shown first, the pages behind it next, and
    // none before it.
    press(&mut viewer, "gg");
    let scene = viewer.scene(&|_| false);
    let wanted: Vec<TileKey> = scene.wanted.iter().map(|tile| tile.key).collect();
    let of = |index: usize| -> Vec<TileKey> {
        whole_page_tiles(&viewer, index)
            .iter()
            .map(|tile| tile.key)
            .collect()
    };
    let mut expected = of(0);
    expected.extend(of(1));
    expected.extend(of(2));
    assert_eq!(wanted, expected);
    assert_eq!(scene.quads.len(), 1, "no half slide: the black only");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_new_slide_stays_away_until_all_of_it_is_there() {
    let (mut viewer, directory) = editing(DECK);
    press(&mut viewer, "gg:present<CR>");
    let first: HashSet<TextureId> = whole_page_tiles(&viewer, 0)
        .iter()
        .map(|tile| texture(tile.key))
        .collect();
    let held = first.clone();
    viewer.scene(&|id| held.contains(&id) || id == black());
    press(&mut viewer, "<Space>");
    // The next page's tiles are not there yet: the first stays up whole.
    let scene = viewer.scene(&|id| held.contains(&id) || id == black());
    let drawn: HashSet<TextureId> = scene.quads[1..].iter().map(|quad| quad.texture).collect();
    assert_eq!(drawn, first);
    assert!(!scene.wanted.is_empty());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn every_key_of_a_talk() {
    let (mut viewer, directory) = editing(DECK);
    press(&mut viewer, "gg:present<CR>");
    assert_eq!(shown(&viewer), 0);
    // On and back, by every key that means it.
    for (keys, page) in [
        ("<Space>", 1),
        ("<CR>", 2),
        ("j", 3),
        ("l", 4),
        ("<Right>", 5),
        ("<Down>", 5),
        ("<BS>", 4),
        ("k", 3),
        ("h", 2),
        ("<Left>", 1),
        ("<Up>", 0),
        ("<Up>", 0),
        ("<PageDown>", 1),
        ("<PageUp>", 0),
        ("n", 1),
        ("N", 0),
        ("G", 5),
        ("gg", 0),
        ("<End>", 5),
        ("<Home>", 0),
    ] {
        press(&mut viewer, keys);
        assert_eq!(shown(&viewer), page, "{keys}");
    }
    // Back from the first step of a slide: the last step of the one
    // before.
    press(&mut viewer, "3<CR>");
    assert_eq!(shown(&viewer), 4, "slide 3 is page 5");
    press(&mut viewer, "<BS>");
    assert_eq!(shown(&viewer), 3);
    // A slide by its number, from its first step; and back from there.
    press(&mut viewer, "2G");
    assert_eq!(shown(&viewer), 1);
    press(&mut viewer, "`");
    assert_eq!(shown(&viewer), 3, "back to where the jump came from");
    press(&mut viewer, "<C-o>");
    assert_eq!(shown(&viewer), 1, "and on again");
    // A number typed and taken back, or left, goes nowhere.
    press(&mut viewer, "45<BS><Esc>");
    assert!(viewer.is_presenting());
    assert_eq!(shown(&viewer), 1);
    press(&mut viewer, "9<CR>");
    assert_eq!(shown(&viewer), 1, "there is no slide 9");
    // Blank: black, white, and any key brings the slide back.
    for (key, colour) in [("b", BLACK), (".", BLACK), ("w", WHITE), (",", WHITE)] {
        press(&mut viewer, key);
        let scene = viewer.scene(&|_| true);
        assert_eq!(scene.quads.len(), 1, "{key}");
        assert_eq!(
            scene.quads[0].texture,
            TextureId(hash128(&("solid", colour)))
        );
        press(&mut viewer, "x");
        assert_eq!(shown(&viewer), 1, "{key}: the key only brings it back");
        assert!(viewer.scene(&|_| true).quads.len() > 1);
    }
    // The overview: a picture of each slide, its last step.
    press(&mut viewer, "o");
    let scene = viewer.scene(&|_| false);
    let pages: HashSet<TileKey> = scene.wanted.iter().map(|tile| tile.key).collect();
    assert!(!pages.is_empty());
    press(&mut viewer, "l<CR>");
    assert_eq!(shown(&viewer), 4, "the slide after the one shown");
    press(&mut viewer, "<Tab>hh<Esc>");
    assert_eq!(shown(&viewer), 4, "Escape leaves it as it was");
    press(&mut viewer, "o4<CR>");
    assert_eq!(shown(&viewer), 5, "a number in the overview");
    // The clock and the presenter's view are drawn; F5 does nothing.
    press(&mut viewer, "t");
    let plain = viewer.scene(&|_| true).quads.len();
    assert!(
        plain > 1 + whole_page_tiles(&viewer, 5).len(),
        "the clock is up"
    );
    press(&mut viewer, "tp");
    assert!(viewer.is_presenter_view());
    viewer.scene(&|_| true);
    press(&mut viewer, "p<F5>");
    assert!(!viewer.is_presenter_view() && viewer.is_presenting());
    // The text was never touched.
    assert_eq!(viewer.text(), Some(DECK));
    press(&mut viewer, "q");
    assert!(!viewer.is_presenting());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn leaving_the_talk_puts_the_cursor_on_the_slide_it_ended_on() {
    for leave in ["q", "<Esc>"] {
        let (mut viewer, directory) = editing(DECK);
        press(&mut viewer, "gg:present<CR>");
        viewer.take_screen_wish();
        press(&mut viewer, &format!("3<CR>{leave}"));
        assert!(!viewer.is_presenting());
        assert_eq!(
            viewer.take_screen_wish(),
            Some((false, true)),
            "a window again"
        );
        let editor = viewer.editor.as_ref().unwrap();
        assert!(
            editor.text()[editor.cursor()..].starts_with("Three."),
            "{leave}: {:?}",
            &editor.text()[editor.cursor()..]
        );
        // The editor is back, with its status line.
        let scene = viewer.scene(&|_| true);
        assert!(scene.quads.iter().any(|quad| quad.texture != black()));
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn a_window_that_was_fullscreen_stays_so() {
    let (mut viewer, directory) = editing(DECK);
    press(&mut viewer, "<F5>");
    assert!(viewer.is_presenting(), "F5 begins a talk");
    assert_eq!(viewer.take_screen_wish(), Some((true, true)));
    viewer.screen_was_full();
    press(&mut viewer, "q");
    assert_eq!(viewer.take_screen_wish(), Some((false, false)));
    press(&mut viewer, "gP");
    assert!(viewer.is_presenting(), "gP begins one too");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn clicks_touches_and_the_wheel_go_on() {
    let (mut viewer, directory) = editing(DECK);
    press(&mut viewer, "gg:present<CR>");
    viewer.pointer_pressed((10.0, 10.0), false, Instant::now());
    viewer.pointer_released();
    assert_eq!(shown(&viewer), 1);
    assert!(viewer.present_click((500.0, 300.0)));
    assert_eq!(shown(&viewer), 2);
    viewer.scroll(0.0, 30.0);
    assert_eq!(shown(&viewer), 2, "a little of the wheel is nothing");
    viewer.scroll(0.0, 30.0);
    assert_eq!(shown(&viewer), 3);
    viewer.scroll(0.0, -60.0);
    assert_eq!(shown(&viewer), 2);
    // In the overview, a click picks the slide under it.
    press(&mut viewer, "o");
    viewer.scene(&|_| true);
    assert!(viewer.present_click((5.0, 5.0)));
    assert_eq!(shown(&viewer), 0);
    press(&mut viewer, "q");
    assert!(!viewer.present_click((5.0, 5.0)), "no talk, no click of it");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_clock_wakes_the_window_only_where_it_is_shown() {
    let (mut viewer, directory) = editing(DECK);
    press(&mut viewer, "gg:present<CR>");
    viewer.scene(&|_| true);
    assert_eq!(
        viewer.present_at(),
        None,
        "hidden: nothing wakes the window"
    );
    press(&mut viewer, "t");
    let before = Instant::now();
    viewer.scene(&|_| true);
    let at = viewer.present_at().unwrap();
    assert!(at > before && at <= before + Duration::from_mins(1) + Duration::from_millis(50));
    assert!(!viewer.present_due(before));
    assert!(viewer.present_due(at), "a minute it shows went by");
    assert_eq!(viewer.present_at(), None, "until it is drawn again");
    viewer.scene(&|_| true);
    assert!(viewer.present_at().is_some());
    // Blanked or in the overview, there is no clock.
    press(&mut viewer, "b");
    viewer.scene(&|_| true);
    assert_eq!(viewer.present_at(), None);
    press(&mut viewer, "xo");
    viewer.scene(&|_| true);
    assert_eq!(viewer.present_at(), None);
    // The presenter's view always has it.
    press(&mut viewer, "<Esc>tp");
    viewer.scene(&|_| true);
    assert!(viewer.present_at().is_some());
    press(&mut viewer, "q");
    assert_eq!(viewer.present_at(), None);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_rehearsal_lists_the_time_of_each_slide_and_is_kept() {
    let (mut viewer, directory) = editing(DECK);
    press(&mut viewer, "gg:present rehearse 20min<CR>");
    for _ in 0..5 {
        std::thread::sleep(Duration::from_millis(5));
        press(&mut viewer, "<Space>");
    }
    press(&mut viewer, "q");
    let Some(picker) = &viewer.picker else {
        panic!("the times are listed");
    };
    let (_, rows, _) = picker.listing();
    assert_eq!(rows.len(), 4, "a row for each slide: {rows:?}");
    assert!(rows[1].starts_with("slide   2"), "{rows:?}");
    assert!(rows[3].contains("total"));
    // Enter goes to the slide.
    press(&mut viewer, "<Down><Down><CR>");
    let editor = viewer.editor.as_ref().unwrap();
    assert!(editor.text()[editor.cursor()..].starts_with("Three."));
    // Kept: listed again, and the plan is there for the next talk.
    let talks = directory.join("state").join("talks");
    assert_eq!(std::fs::read_dir(&talks).unwrap().count(), 1);
    press(&mut viewer, ":present times<CR>");
    assert!(viewer.picker.is_some());
    press(&mut viewer, "<Esc>:present<CR>");
    let show = viewer.present.show.as_ref().unwrap();
    assert_eq!(show.plan.duration, Some(Duration::from_mins(20)));
    assert_eq!(show.plan.rehearsed.len(), 4);
    // The presenter's view says how far the talk is.
    press(&mut viewer, "p");
    viewer.scene(&|_| true);
    let foot = viewer.presenter_foot(&viewer.present.deck.as_ref().unwrap().1.clone(), 6);
    assert!(foot.contains("left"), "{foot}");
    assert!(foot.contains("ahead") || foot.contains("on time"), "{foot}");
    press(&mut viewer, "q");
    press(&mut viewer, ":present nonsense<CR>");
    assert!(!viewer.is_presenting());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_notes_of_the_slide_are_in_the_presenters_view() {
    let (mut viewer, directory) = editing(DECK);
    press(&mut viewer, "/Three<CR>:present<CR>p");
    let images = viewer.scene(&|_| false).images.len();
    assert!(images >= 3, "the notes, what is next, and the foot");
    let deck = viewer.deck().unwrap();
    assert_eq!(deck.notes_of(shown(&viewer)), ["Say three slowly."]);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn in_the_slides_view_brackets_go_by_slide() {
    let (mut viewer, directory) = editing(DECK);
    press(&mut viewer, "gg:slides<CR>");
    let at = |viewer: &Viewer| viewer.slide;
    for (keys, slide) in [
        ("]]", 1),
        ("]]", 4),
        ("]]", 5),
        ("[[", 4),
        ("[[", 1),
        ("[[", 0),
    ] {
        press(&mut viewer, keys);
        assert_eq!(at(&viewer), slide, "{keys}");
    }
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_new_slide_is_started_in_the_decks_own_way() {
    let starter = include_str!("../../../templates/slides.typ");
    let (mut viewer, directory) = editing(starter);
    press(&mut viewer, "/Second<CR>:slide new<CR>");
    let text = viewer.text().unwrap();
    let new = text.find("= ‹Title›").unwrap();
    assert!(new > text.find("‹Second slide›").unwrap());
    assert!(new < text.find("= Thank you").unwrap());
    // The cursor is on its title, to type it.
    press(&mut viewer, "Plans<Esc>");
    assert!(viewer.text().unwrap().contains("= Plans\n"));
    press(&mut viewer, "u");
    // Not a deck: said so, nothing done.
    let (mut paper, other) = editing("= Intro\n\nWords.\n");
    press(&mut paper, ":slide new<CR>");
    assert_eq!(paper.text(), Some("= Intro\n\nWords.\n"));
    assert!(paper.message.as_ref().unwrap().text.contains("deck"));
    let _ = std::fs::remove_dir_all(directory);
    let _ = std::fs::remove_dir_all(other);
}

#[test]
fn the_overview_moves_a_slide_that_is_one_block() {
    let text = "#set page(paper: \"presentation-16-9\")\n\
                #let slide(body) = [#body #pagebreak(weak: true)]\n\
                #slide[One]\n\n#slide[Two]\n\n#slide[Three]\n";
    let (mut viewer, directory) = editing(text);
    press(&mut viewer, ":grid<CR>gL");
    assert_eq!(
        viewer.text().unwrap(),
        text.replace("#slide[One]\n\n#slide[Two]", "#slide[Two]\n\n#slide[One]")
    );
    assert!(viewer.is_grid(), "the overview stays");
    assert_eq!(viewer.grid.as_ref().unwrap().chosen, 1, "with the slide");
    compile(&mut viewer);
    press(&mut viewer, "H");
    assert_eq!(viewer.text(), Some(text));
    // n starts a slide behind the one picked out.
    compile(&mut viewer);
    press(&mut viewer, "n");
    assert!(!viewer.is_grid());
    assert!(
        viewer
            .text()
            .unwrap()
            .contains("#slide[One]\n\n#slide[\n  ‹What")
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_handout_has_the_last_step_of_every_slide() {
    let (mut viewer, directory) = editing(DECK);
    press(&mut viewer, ":pdf handout<CR>");
    let told = viewer.message.as_ref().unwrap().text.clone();
    assert!(told.contains("4 slides, 6 to a page, on 1 page"), "{told}");
    let pdf = std::fs::read(directory.join("main-handout.pdf")).unwrap();
    assert!(pdf.starts_with(b"%PDF"));
    assert_eq!(viewer.text(), Some(DECK), "the document is not touched");
    press(&mut viewer, ":pdf handout 2<CR>");
    let told = viewer.message.as_ref().unwrap().text.clone();
    assert!(told.contains("on 2 page"), "{told}");
    press(&mut viewer, ":pdf handout 5<CR>");
    assert!(viewer.message.as_ref().unwrap().alert);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn with_a_screen_of_its_own_the_audience_has_a_window_of_its_own() {
    let (mut viewer, directory) = editing(DECK);
    press(&mut viewer, "/Three<CR>:present<CR>");
    assert!(viewer.wants_two_screens());
    viewer.set_audience_apart(true);
    assert!(viewer.is_presenter_view(), "this window: the presenter's");
    let size = (1920, 1080);
    let tiles = |viewer: &Viewer| {
        let document = viewer.document.as_ref().unwrap();
        let page = &document.pages()[4];
        let zoom = (1920.0 / page.frame.width().to_pt()).min(1080.0 / page.frame.height().to_pt());
        let (across, down) = page_size_px(page, zoom);
        let whole = PixelRegion {
            x: 0,
            y: 0,
            width: across,
            height: down,
        };
        tiles(page, zoom, whole).len()
    };
    let audience = viewer.audience_scene(size, &|_| true).unwrap();
    assert_eq!(
        audience.quads.len(),
        1 + tiles(&viewer),
        "nothing but the slide"
    );
    assert_eq!(audience.quads[0].width, 1920.0);
    // Blanked: the audience's screen is, the presenter's view is not.
    press(&mut viewer, "b");
    let audience = viewer.audience_scene(size, &|_| true).unwrap();
    assert_eq!(audience.quads.len(), 1);
    assert!(viewer.scene(&|_| true).quads.len() > 3);
    press(&mut viewer, "x");
    viewer.set_audience_apart(false);
    assert!(!viewer.is_presenter_view());
    press(&mut viewer, "q:present here<CR>");
    assert!(!viewer.wants_two_screens());
    press(&mut viewer, "q");
    assert!(viewer.audience_scene(size, &|_| true).is_none());
    let _ = std::fs::remove_dir_all(directory);
}
