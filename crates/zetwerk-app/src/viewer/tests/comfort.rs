//! A document is where it was left, and comfortable to look at for
//! hours: the place found again, the typewriter, focus, zen, night
//! colours, and a window that changes its scale.

use super::*;

/// A text of many paragraphs over several pages, with a heading among
/// them and a word to find in the middle.
fn long_text() -> String {
    let mut text = String::from("#set page(width: 300pt, height: 400pt)\n\n= First\n\n");
    for paragraph in 0..40 {
        if paragraph == 20 {
            text.push_str("= Middle\n\n");
        }
        let words =
            format!("paragraph{paragraph} with words that fill a line or two of the page. ");
        text.push_str(&words.repeat(3));
        if paragraph == 25 {
            text.push_str("Here is the target word. ");
        }
        text.push_str("\n\n");
    }
    text
}

/// The viewer of the document in `directory`, opened anew, with its
/// first pages there.
fn reopened(directory: &Path) -> Viewer {
    let (mut viewer, _) = open(directory);
    compile(&mut viewer);
    viewer
}

/// The viewer of the document in `directory`, opened anew, before its
/// first pages are there.
fn before_the_pages(directory: &Path) -> Viewer {
    let path = directory.join("main.typ");
    let env = Environment::new(FontSet::Embedded);
    let project = Project::open(Arc::clone(&env), &path).unwrap();
    let (editor, opened) = Editor::open(&path, &directory.join("state"), project.main()).unwrap();
    let mut viewer = Viewer::new(env, project, (1000, 620), 1.0)
        .unwrap()
        .editing(editor, &opened);
    viewer.state_dir = Some(directory.join("state"));
    viewer
}

/// Where the middle of the cursor's line is on screen, in pixels from
/// the top of the view.
fn line_height_on_screen(viewer: &Viewer) -> Option<f64> {
    let (page, top, bottom) = viewer.cursor_line()?;
    let page = viewer.layout.pages.get(page)?;
    let zoom = viewer.view.zoom;
    Some(f64::from(page.y) + (top + bottom) / 2.0 * zoom - viewer.view.top * zoom)
}

#[test]
fn a_document_opens_where_it_was_left() {
    let (mut viewer, directory) = editing(&long_text());
    compile(&mut viewer);
    press(&mut viewer, "gsgs/target<CR>");
    let target = viewer.text().unwrap().find("target").unwrap();
    assert_eq!(viewer.editor.as_ref().unwrap().cursor(), target);
    // A mark, a register, an aid, a zoom; and the cursor left where it is.
    press(&mut viewer, "ma\"ayiwgg");
    press(&mut viewer, "`a:numbers off<CR>");
    viewer.zoom_by(1.25, (100.0, 100.0));
    press(&mut viewer, ":night<CR>");
    let (top, zoom) = (viewer.view.top, viewer.view.zoom);
    viewer.scene(&|_| false);
    viewer.leaving();
    drop(viewer);

    let mut viewer = reopened(&directory);
    let editor = viewer.editor.as_ref().unwrap();
    assert_eq!(editor.cursor(), target);
    assert_eq!(editor.marks_kept(), [('a', target)]);
    assert_eq!(editor.engine().register('a'), Some(("target", false)));
    assert_eq!(editor.search_kept(), Some(("target", true, false, false)));
    assert_eq!(viewer.panes(), Panes::Page);
    assert!(!viewer.numbers);
    assert_eq!(viewer.page_tone(), crate::gpu::Tone::Night);
    assert!(
        (viewer.view.zoom - zoom).abs() < 1e-6,
        "{} {zoom}",
        viewer.view.zoom
    );
    assert!(
        (viewer.view.top - top).abs() < 1e-3,
        "{} {top}",
        viewer.view.top
    );
    // `n` goes on with the search.
    press(&mut viewer, "ggn");
    assert_eq!(viewer.editor.as_ref().unwrap().cursor(), target);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_place_is_found_by_its_text_where_the_file_changed_outside() {
    let text = long_text();
    let (mut viewer, directory) = editing(&text);
    compile(&mut viewer);
    press(&mut viewer, "gsgs/target<CR>");
    viewer.leaving();
    drop(viewer);
    // Another program put a paragraph in before it.
    let changed = text.replacen(
        "= First\n\n",
        "= First\n\nA paragraph that was not there.\n\n",
        1,
    );
    std::fs::write(directory.join("main.typ"), &changed).unwrap();
    let viewer = reopened(&directory);
    let at = changed.find("target").unwrap();
    assert_eq!(viewer.editor.as_ref().unwrap().cursor(), at);
    // And in view.
    let height = line_height_on_screen(&viewer).unwrap();
    assert!(
        height >= 0.0 && height <= f64::from(viewer.view.height),
        "{height}"
    );
    let _ = std::fs::remove_dir_all(&directory);

    // Its paragraph rewritten: under the heading above it.
    let (mut viewer, directory) = editing(&text);
    compile(&mut viewer);
    press(&mut viewer, "gsgs/target<CR>");
    viewer.leaving();
    drop(viewer);
    let start = text.find("paragraph25 ").unwrap();
    let end = text[start..].find("\n\n").unwrap() + start;
    let rewritten = format!("{}Something else entirely.{}", &text[..start], &text[end..]);
    std::fs::write(directory.join("main.typ"), &rewritten).unwrap();
    let viewer = reopened(&directory);
    assert_eq!(
        viewer.editor.as_ref().unwrap().cursor(),
        rewritten.find("= Middle").unwrap()
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn fresh_and_keys_before_the_pages_leave_the_cursor_alone() {
    let (mut viewer, directory) = editing(&long_text());
    compile(&mut viewer);
    press(&mut viewer, "gsgs/target<CR>");
    viewer.leaving();
    drop(viewer);
    // `--fresh`: at the start.
    let mut viewer = before_the_pages(&directory);
    viewer.place.fresh = true;
    compile(&mut viewer);
    assert_eq!(viewer.editor.as_ref().unwrap().cursor(), 0);
    drop(viewer);
    // A key before the first pages came: where the key put it.
    let mut viewer = before_the_pages(&directory);
    press(&mut viewer, "j");
    let after_key = viewer.editor.as_ref().unwrap().cursor();
    compile(&mut viewer);
    assert_eq!(viewer.editor.as_ref().unwrap().cursor(), after_key);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_place_goes_along_with_a_file_moved() {
    let (mut viewer, directory) = editing(&long_text());
    compile(&mut viewer);
    press(&mut viewer, "gsgs/target<CR>:mv moved.typ<CR>");
    viewer.leaving();
    drop(viewer);
    let path = directory.join("moved.typ");
    let env = Environment::new(FontSet::Embedded);
    let project = Project::open(Arc::clone(&env), &path).unwrap();
    let (editor, opened) = Editor::open(&path, &directory.join("state"), project.main()).unwrap();
    let mut viewer = Viewer::new(env, project, (1000, 620), 1.0)
        .unwrap()
        .editing(editor, &opened);
    viewer.state_dir = Some(directory.join("state"));
    compile(&mut viewer);
    let at = viewer.text().unwrap().find("target").unwrap();
    assert_eq!(viewer.editor.as_ref().unwrap().cursor(), at);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn nothing_is_kept_where_the_settings_say_so() {
    let (mut viewer, directory) = editing(&long_text());
    viewer.place.never = true;
    compile(&mut viewer);
    press(&mut viewer, "gsgs/target<CR>");
    viewer.leaving();
    assert!(!directory.join("state").join("places").exists());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_typewriter_keeps_the_line_at_its_height_over_a_paragraph_of_typing() {
    let (mut viewer, directory) = editing(&long_text());
    compile(&mut viewer);
    press(&mut viewer, "gsgs/paragraph20 <CR>zT");
    assert!(viewer.comfort.typewriter.is_some());
    let height = viewer.comfort.typewriter.unwrap() * f64::from(viewer.view.height);
    let at = |viewer: &Viewer| line_height_on_screen(viewer).unwrap();
    assert!(
        (at(&viewer) - height).abs() < 1.5,
        "{} {height}",
        at(&viewer)
    );
    press(&mut viewer, "A");
    let mut lines = 0;
    let mut last = (viewer.cursor_line(), viewer.view.top);
    for word in 0..60 {
        press(&mut viewer, &format!(" word{word}"));
        // Typing pauses now and then, and the pages come.
        if word % 4 == 3 {
            compile(&mut viewer);
            viewer.follow_cursor_by(0.0, 0.0);
        }
        let now = (viewer.cursor_line(), viewer.view.top);
        let Some((page, top, _)) = now.0 else {
            continue;
        };
        let same_line = last
            .0
            .is_some_and(|(was, was_top, _)| was == page && (was_top - top).abs() < 2.0);
        if same_line {
            assert_eq!(
                now.1, last.1,
                "the view moved within a line, at word {word}"
            );
        } else if last.0.is_some() {
            lines += 1;
        }
        // Wherever the line is, it is at the height it was kept at.
        assert!(
            (at(&viewer) - height).abs() < 1.5,
            "word {word}: {} {height}",
            at(&viewer)
        );
        last = now;
    }
    assert!(lines >= 3, "the typing went over {lines} lines");
    // And off: the view follows the cursor as usual again.
    press(&mut viewer, "<Esc>zT");
    assert!(viewer.comfort.typewriter.is_none());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn focus_leaves_the_paragraph_clear_and_veils_the_rest() {
    let (mut viewer, directory) = editing(&long_text());
    compile(&mut viewer);
    press(&mut viewer, "gsgs/target<CR>");
    let plain = viewer.scene(&|_| false);
    assert!(super::super::comfort::veils(&plain).is_empty());
    press(&mut viewer, "zf");
    let scene = viewer.scene(&|_| false);
    let veils = super::super::comfort::veils(&scene);
    assert!(!veils.is_empty() && veils.len() <= 4 * 3, "{veils:?}");
    let (x, top, bottom) = viewer.cursor_at.unwrap();
    let middle = (top + bottom) / 2.0;
    let covered = |[vx, vy, w, h]: [f32; 4]| {
        let (vx, vy, w, h) = (f64::from(vx), f64::from(vy), f64::from(w), f64::from(h));
        x > vx && x < vx + w && middle > vy && middle < vy + h
    };
    assert!(
        !veils.iter().any(|veil| covered(*veil)),
        "the cursor is veiled"
    );
    // The sentence only: more is veiled.
    press(&mut viewer, ":focus sentence<CR>");
    let sentence = super::super::comfort::veils(&viewer.scene(&|_| false));
    let area = |veils: &[[f32; 4]]| veils.iter().map(|v| v[2] * v[3]).sum::<f32>();
    assert!(area(&sentence) > area(&veils));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn zen_hides_the_status_line_and_the_aids_and_brings_them_back() {
    let (mut viewer, directory) = editing(&long_text());
    compile(&mut viewer);
    press(&mut viewer, "gsgs:numbers on<CR>");
    let usual = viewer.view.height;
    press(&mut viewer, "ze");
    assert_eq!(viewer.status_height(), 0);
    assert!(viewer.view.height > usual);
    assert!(!viewer.numbers && viewer.comfort.focus.is_some());
    // Nothing below the view, unless there is something to say.
    viewer.message = None;
    let height = f64::from(viewer.window.1) as f32;
    let below = |scene: &Scene| {
        scene
            .quads
            .iter()
            .filter(|q| q.y + q.height > height - 1.0 && q.y > height - 40.0)
            .count()
    };
    let quiet = viewer.scene(&|_| false);
    press(&mut viewer, ":");
    let typing = viewer.scene(&|_| false);
    assert!(below(&typing) > below(&quiet), "the command line shows");
    press(&mut viewer, "<Esc>ze");
    assert_eq!(viewer.view.height, usual);
    assert!(viewer.numbers && viewer.comfort.focus.is_none());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn night_colours_the_pages_on_screen_and_not_one_byte_of_an_export() {
    let text = "#set page(width: 200pt, height: 200pt)\n= Night\n\nWords in #text(red)[red].\n\n\
                #image(bytes(\"<svg xmlns='http://www.w3.org/2000/svg' width='40' height='30'>\
                <rect width='40' height='30' fill='blue'/></svg>\"), width: 40pt)\n";
    let (mut viewer, directory) = editing(text);
    compile(&mut viewer);
    press(&mut viewer, "gsgs");
    let wanted = super::super::export::Wanted::parse("png", "").unwrap();
    let day = viewer.scene(&|_| false);
    assert_eq!(day.paper, crate::gpu::Paper::default());
    let exported = |viewer: &Viewer| {
        let outcome = viewer.plan_export(&wanted).unwrap().run().unwrap();
        let _ = outcome;
        std::fs::read(directory.join("main.png")).unwrap()
    };
    let by_day = exported(&viewer);
    press(&mut viewer, "zn");
    assert_eq!(viewer.page_tone(), crate::gpu::Tone::Night);
    let night = viewer.scene(&|_| false);
    let paper = &night.paper;
    assert_eq!(paper.tone, crate::gpu::Tone::Night);
    // The page's paper and what lies on it are toned; the status line
    // is not.
    let page = paper.pages[0];
    let first = paper.quads[0].start;
    assert_eq!(
        [night.quads[first].x, night.quads[first].y],
        [page[0], page[1]]
    );
    assert!(!paper.tones(night.quads.len() - 1));
    // The picture keeps its colours, where it is drawn.
    assert_eq!(paper.pictures.len(), 1, "{:?}", paper.pictures);
    let picture = paper.pictures[0];
    let zoom = viewer.view.zoom as f32;
    assert!((picture[2] - 40.0 * zoom).abs() < 1.0, "{picture:?}");
    assert!(picture[0] >= page[0] && picture[1] >= page[1]);
    assert_eq!(exported(&viewer), by_day, "night changed the export");
    // And back.
    press(&mut viewer, ":night<CR>");
    assert_eq!(viewer.scene(&|_| false).paper, crate::gpu::Paper::default());
    press(&mut viewer, ":sepia<CR>");
    assert_eq!(viewer.page_tone(), crate::gpu::Tone::Sepia);
    press(&mut viewer, ":colours day<CR>");
    assert_eq!(viewer.page_tone(), crate::gpu::Tone::Day);
    // Others colour what they draw on a page outside the toned quads
    // as the page is.
    press(&mut viewer, "zn");
    assert_ne!(viewer.on_paper([0, 0, 0, 255]), [0, 0, 0, 255]);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_zoom_chosen_stays_as_large_on_screen_when_the_display_scale_changes() {
    let mut viewer = viewer(2);
    press(&mut viewer, "+");
    let zoom = viewer.view.zoom;
    // Moved to an output of scale 1.5: as many pixels again and half.
    viewer.resize((1200, 930), 1.5);
    assert!((viewer.view.zoom - zoom * 1.5).abs() < 1e-9);
    assert!((viewer.scale - 1.5).abs() < 1e-9, "the interface follows");
    // Fitted to the width, it follows the width as before.
    press(&mut viewer, "=");
    let fitted = viewer.view.zoom;
    viewer.resize((800, 620), 1.0);
    assert!(viewer.view.zoom < fitted);
}

#[test]
fn at_a_fractional_scale_images_are_drawn_on_whole_pixels_at_their_size() {
    // Blurred (or a pixel off) where an image of the interface lands
    // between pixels or is stretched.
    let (mut viewer, directory) = editing(&long_text());
    viewer.resize((1000, 620), 1.5);
    compile(&mut viewer);
    press(&mut viewer, "gsgs/target<CR>:numbers on<CR>:perf<CR>");
    for keys in ["", "zf", "i", "<Esc>:"] {
        press(&mut viewer, keys);
        let scene = viewer.scene(&|_| false);
        let sizes: HashMap<TextureId, (u32, u32)> = scene
            .images
            .iter()
            .map(|image| (image.id, (image.pixmap.width(), image.pixmap.height())))
            .collect();
        for quad in &scene.quads {
            let Some(&(width, height)) = sizes.get(&quad.texture) else {
                continue;
            };
            if width == 1 && height == 1 {
                continue;
            }
            assert_eq!(
                [quad.x.fract(), quad.y.fract()],
                [0.0, 0.0],
                "after {keys:?}: {quad:?}"
            );
            assert_eq!(
                [quad.width, quad.height],
                [width as f32, height as f32],
                "after {keys:?}: {quad:?}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(directory);
}
