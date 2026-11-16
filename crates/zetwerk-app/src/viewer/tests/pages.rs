//! Pages put in whole, as a user meets them: put in from the keyboard,
//! on the page as one thing, in the pages panel, by an agent, and what
//! goes wrong.

use super::*;
use crate::page_text::tests::slides_pdf;

/// A document of two paragraphs on small pages that are numbered.
const TWO: &str = "#set page(width: 200pt, height: 300pt, numbering: \"1\")\n\
First paragraph.\n\
\n\
Second paragraph.\n";

/// The message the viewer shows, if any.
fn said(viewer: &Viewer) -> String {
    viewer
        .message
        .as_ref()
        .map_or_else(String::new, |message| message.text.clone())
}

/// A viewer editing `text`, with `slides.pdf` (three pages of 300 by 200
/// points) beside it, the pages typeset.
fn with_slides(text: &str) -> (Viewer, PathBuf) {
    let directory = directory(text);
    std::fs::write(directory.join("slides.pdf"), slides_pdf()).unwrap();
    let (viewer, _) = open(&directory);
    (viewer, directory)
}

/// The sizes of the pages on screen, in whole points.
fn sizes(viewer: &Viewer) -> Vec<(i64, i64)> {
    let document = viewer.document.as_ref().unwrap();
    document
        .pages()
        .iter()
        .map(|page| {
            let size = page.frame.size();
            (size.x.to_pt().round() as i64, size.y.to_pt().round() as i64)
        })
        .collect()
}

fn text(viewer: &Viewer) -> String {
    viewer.text().unwrap_or_default().to_owned()
}

/// The line that puts in page `page` of `slides.pdf`, as large as it is.
fn slide(page: usize) -> String {
    crate::page_text::written(&crate::page_text::Page {
        shows: crate::page_text::Shows::Pdf {
            file: "slides.pdf".to_owned(),
            page,
        },
        fit: false,
        counted: true,
    })
}

#[test]
fn a_page_of_a_pdf_is_put_in_after_the_paragraph_and_taken_out_on_the_page() {
    let (mut viewer, directory) = with_slides(TWO);
    press(&mut viewer, "gsgs/First<CR>:page add slides.pdf 2<CR>");
    // Where is asked, after the paragraph first.
    assert!(viewer.is_picking(), "{}", said(&viewer));
    press(&mut viewer, "<CR>");
    assert_eq!(
        text(&viewer),
        format!(
            "{}\n\n{}\n\nSecond paragraph.\n",
            &TWO[..TWO.find("\n\nSecond").unwrap()],
            slide(2)
        )
    );
    assert!(
        said(&viewer).contains("page 2 of slides.pdf put in after the paragraph"),
        "{}",
        said(&viewer)
    );
    compile(&mut viewer);
    assert_eq!(sizes(&viewer), [(200, 300), (300, 200), (200, 300)]);
    // The cursor is on it: typing does nothing to it, x takes it out,
    // u brings it back.
    assert!(viewer.page_here().is_some());
    for key in ["i", "a", "o", "p", "r", "s"] {
        press(&mut viewer, key);
        assert!(
            said(&viewer).contains("x takes it out"),
            "{key}: {}",
            said(&viewer)
        );
        assert!(text(&viewer).contains(&slide(2)), "{key}");
        assert!(viewer.page_here().is_some(), "{key}");
    }
    press(&mut viewer, "x");
    assert_eq!(text(&viewer), TWO);
    assert!(said(&viewer).contains("taken out"), "{}", said(&viewer));
    press(&mut viewer, "u");
    assert!(text(&viewer).contains(&slide(2)));
    compile(&mut viewer);
    // `j` and `k` come to rest on it and go on past it.
    press(&mut viewer, "gg");
    assert!(viewer.page_here().is_none());
    press(&mut viewer, "j");
    assert!(viewer.page_here().is_some(), "j rests on the page put in");
    assert_eq!(viewer.cursor_page.get(), 1);
    press(&mut viewer, "j");
    assert!(viewer.page_here().is_none());
    assert!(text(&viewer)[viewer.editor.as_ref().unwrap().cursor()..].starts_with("Second"));
    press(&mut viewer, "k");
    assert!(viewer.page_here().is_some(), "k comes back to it");
    // `dd` takes all of it.
    press(&mut viewer, "dd");
    assert_eq!(text(&viewer), TWO);
    let _ = std::fs::remove_dir_all(directory);
}

/// What the list up shows, as listed.
fn listed(viewer: &Viewer) -> Vec<String> {
    let picker = viewer.picker.as_ref().unwrap();
    picker.listing().1.into_iter().map(str::to_owned).collect()
}

/// `TWO` with the pages of `slides.pdf` named put in after its first
/// paragraph.
fn two_with(pages: &[usize]) -> String {
    let lines: Vec<String> = pages.iter().map(|page| slide(*page)).collect();
    format!(
        "{}\n\n{}\n\nSecond paragraph.\n",
        &TWO[..TWO.find("\n\nSecond").unwrap()],
        lines.join("\n")
    )
}

#[test]
fn the_pages_panel_lists_the_pages_and_changes_those_put_in() {
    let text = two_with(&[1, 2]);
    let (mut viewer, directory) = with_slides(&text);
    press(&mut viewer, "gS");
    let rows = listed(&viewer);
    assert_eq!(rows.len(), 5, "{rows:?}");
    assert!(rows[0].starts_with("  1  First paragraph."), "{rows:?}");
    assert_eq!(rows[1], "  2  page 1 of slides.pdf\tas large as it is");
    assert_eq!(rows[2], "  3  page 2 of slides.pdf\tas large as it is");
    assert!(rows[3].starts_with("  4  Second paragraph."), "{rows:?}");
    // The second page put in: fit to the paper, out of the count, up
    // past the other.
    press(&mut viewer, "page 2 of<CR>");
    assert_eq!(viewer.picker.as_ref().unwrap().listing().0, "page put in");
    press(&mut viewer, "size<CR>");
    assert!(
        text_of(&viewer).contains("[#image(\"slides.pdf\", page: 2, width: 100%"),
        "{}",
        text_of(&viewer)
    );
    assert!(viewer.is_picking(), "the page comes back");
    press(&mut viewer, "counts<CR>");
    assert!(
        text_of(&viewer)
            .contains("[#counter(page).update(n => n - 1)#image(\"slides.pdf\", page: 2")
    );
    press(&mut viewer, "move it up<CR>");
    let now = text_of(&viewer);
    assert!(
        now.find("page: 2").unwrap() < now.find("page: 1").unwrap(),
        "{now}"
    );
    assert!(
        said(&viewer).contains("past the page put in next to it"),
        "{}",
        said(&viewer)
    );
    compile(&mut viewer);
    assert_eq!(
        sizes(&viewer),
        [(200, 300), (200, 300), (300, 200), (200, 300)]
    );
    // Each change one step for `u`.
    press(&mut viewer, "<Esc><Esc><Esc>uuu");
    assert_eq!(text_of(&viewer), text);
    // Both at once: up past the text, then out.
    compile(&mut viewer);
    press(&mut viewer, ":pages<CR>page 1 of<CR>move the 2 pages<CR>");
    let now = text_of(&viewer);
    assert!(
        now.find("page: 1").unwrap() < now.find("First").unwrap(),
        "{now}"
    );
    assert!(
        said(&viewer).contains("nearest page break"),
        "{}",
        said(&viewer)
    );
    press(&mut viewer, "take out all 2<CR>");
    // What they stood between is apart by a blank line.
    assert_eq!(text_of(&viewer), TWO.replacen(")\n", ")\n\n", 1));
    press(&mut viewer, "<Esc><Esc>uu");
    assert_eq!(text_of(&viewer), text);
    let _ = std::fs::remove_dir_all(directory);
}

fn text_of(viewer: &Viewer) -> String {
    text(viewer)
}

#[test]
fn pages_go_where_they_are_asked_to_and_other_pages_of_the_file_are_chosen() {
    let (mut viewer, directory) = with_slides(TWO);
    // A PDF of more pages is asked about, saying how many it has.
    press(&mut viewer, "gsgs:page add slides.pdf<CR>");
    let rows = listed(&viewer);
    assert_eq!(rows[0], "all 3 pages");
    assert!(
        rows[2].contains("slides.pdf: 3 pages, 300 × 200 pt"),
        "{rows:?}"
    );
    press(&mut viewer, "1,3<CR>");
    assert_eq!(viewer.picker.as_ref().unwrap().listing().0, "where?");
    press(&mut viewer, "end of<CR>");
    assert_eq!(
        text_of(&viewer),
        format!("{TWO}\n{}\n{}\n", slide(1), slide(3))
    );
    // A blank page at the start, behind the rules.
    press(&mut viewer, ":page add blank<CR>start<CR>");
    assert!(
        text_of(&viewer).starts_with(&format!(
            "{}\n\n#page(header: none, footer: none)[]\n\nFirst",
            TWO.lines().next().unwrap()
        )),
        "{}",
        text_of(&viewer)
    );
    compile(&mut viewer);
    // The blank page, the text, the two pages of the file.
    assert_eq!(viewer.state().pages, 4);
    // Before a page of the text, from the panel: where it begins.
    press(&mut viewer, ":pages<CR>  2  First<CR>before<CR>");
    assert_eq!(viewer.picker.as_ref().unwrap().listing().0, "put in");
    press(&mut viewer, "a page break<CR>");
    assert!(
        text_of(&viewer).contains("[]\n#pagebreak()\n\nFirst"),
        "{}",
        text_of(&viewer)
    );
    press(&mut viewer, "u");
    compile(&mut viewer);
    // The pages of the file put in at the end become others.
    press(
        &mut viewer,
        ":pages<CR>page 3 of<CR>pages of the file<CR>2-3<CR>",
    );
    assert!(
        text_of(&viewer).ends_with(&format!("{}\n{}\n", slide(2), slide(3))),
        "{}",
        text_of(&viewer)
    );
    // The blank page is found where it is, and gone to.
    compile(&mut viewer);
    press(&mut viewer, "<Esc><Esc>:pages<CR>blank<CR>go<CR>");
    assert_eq!(viewer.cursor_page.get(), 0);
    assert!(viewer.page_here().is_some());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn what_cannot_be_a_page_is_said_and_nothing_changes() {
    let (mut viewer, directory) = with_slides(TWO);
    std::fs::write(directory.join("notes.txt"), "Just words.").unwrap();
    let slides = slides_pdf();
    std::fs::write(directory.join("broken.pdf"), &slides[..slides.len() / 3]).unwrap();
    press(&mut viewer, "gsgs");
    for (keys, word) in [
        (":page add notes.txt<CR>", "neither a PDF nor an image"),
        (":page add nowhere.pdf<CR>", "no file nowhere.pdf"),
        (
            ":page add slides.pdf 9<CR>",
            "there is no page 9: the file has 3 pages",
        ),
        (":page add slides.pdf<CR>4-5<CR>", "there is no page 4"),
        (":page add broken.pdf<CR>", "broken.pdf"),
    ] {
        press(&mut viewer, keys);
        assert!(said(&viewer).contains(word), "{keys}: {}", said(&viewer));
        press(&mut viewer, "<Esc><Esc>");
        assert_eq!(text_of(&viewer), TWO, "{keys}");
    }
    // Many pages are asked about first.
    let many = {
        let env = Environment::new(FontSet::Embedded);
        let text = vec!["Page"; 60].join("\n#pagebreak()\n");
        let project = Project::in_memory(env, PathBuf::from("/"), "main.typ", text).unwrap();
        let world = project.snapshot();
        let document = zetwerk_typst::compile(&world).document.unwrap();
        zetwerk_typst::export_pdf(&document, &world).unwrap()
    };
    std::fs::write(directory.join("many.pdf"), many).unwrap();
    press(&mut viewer, ":page add many.pdf<CR><CR>");
    assert_eq!(
        viewer.picker.as_ref().unwrap().listing().0,
        "so many pages?"
    );
    press(&mut viewer, "<CR>");
    assert_eq!(text_of(&viewer), TWO, "no is chosen first");
    press(&mut viewer, "<CR>yes<CR><CR>");
    assert_eq!(text_of(&viewer).matches("#page(").count(), 60);
    press(&mut viewer, "u");
    // A file from elsewhere is copied in; the original stays.
    let elsewhere = directory.with_extension("from");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("slides.pdf"), &slides[..slides.len() - 10]).unwrap();
    let original = std::fs::read(elsewhere.join("slides.pdf")).unwrap();
    std::fs::write(
        elsewhere.join("cover.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"/>",
    )
    .unwrap();
    let cover = elsewhere.join("cover.svg");
    let typed = format!(":page add {}<CR><CR>", cover.display());
    let folder = std::fs::read_dir(&directory).unwrap().count();
    let copy = directory.join("cover.svg");
    press(&mut viewer, &typed);
    assert!(said(&viewer).contains("copied to"), "{}", said(&viewer));
    assert!(copy.is_file());
    assert!(
        text_of(&viewer)
            .contains("#image(\"cover.svg\", width: 100%, height: 100%, fit: \"contain\")"),
        "{}",
        text_of(&viewer)
    );
    assert_eq!(
        std::fs::read(elsewhere.join("slides.pdf")).unwrap(),
        original
    );
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), folder + 1);
    // A file of that name with other bytes gets another name.
    std::fs::write(
        &cover,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"10\"/>",
    )
    .unwrap();
    press(&mut viewer, &typed);
    assert!(directory.join("cover-2.svg").is_file());
    assert!(text_of(&viewer).contains("cover-2.svg"));
    let _ = std::fs::remove_dir_all(directory);
    let _ = std::fs::remove_dir_all(elsewhere);
}

#[test]
fn a_folder_that_cannot_be_written_takes_no_copy() {
    use std::os::unix::fs::PermissionsExt;
    let (mut viewer, directory) = with_slides(TWO);
    let elsewhere = directory.with_extension("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("cover.pdf"), slides_pdf()).unwrap();
    let locked = std::fs::Permissions::from_mode(0o555);
    std::fs::set_permissions(&directory, locked).unwrap();
    press(
        &mut viewer,
        &format!(
            "gsgs:page add {} 1<CR><CR>",
            elsewhere.join("cover.pdf").display()
        ),
    );
    let message = said(&viewer);
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(message.contains("cannot be written"), "{message}");
    assert_eq!(text_of(&viewer), TWO);
    let _ = std::fs::remove_dir_all(directory);
    let _ = std::fs::remove_dir_all(elsewhere);
}

#[test]
fn deleting_over_a_page_put_in_takes_all_of_it_or_nothing() {
    let text = two_with(&[2]);
    for keys in [
        "dj", "d}", "dG", "d2j", "vjd", "Vjd", "jdk", "jd{", "jdgg", "jx", "jdw", "jd$", "jD", "jX",
    ] {
        let (mut viewer, directory) = with_slides(&text);
        press(&mut viewer, &format!("gsgs/First<CR>{keys}"));
        let now = text_of(&viewer);
        let whole = now.contains(&slide(2));
        let none = !now.contains("#page") && !now.contains("#image");
        assert!(whole || none, "{keys}: {now:?}");
        let source = typst::syntax::Source::detached(now.as_str());
        assert!(!source.root().diagnosis().errors, "{keys}: {now:?}");
        let _ = std::fs::remove_dir_all(directory);
    }
}

#[test]
fn a_file_taken_away_is_named_in_what_the_document_lacks() {
    let text = two_with(&[2]);
    let (mut viewer, directory) = with_slides(&text);
    std::fs::remove_file(directory.join("slides.pdf")).unwrap();
    viewer.file_changed(&directory.join("slides.pdf"));
    press(&mut viewer, "A <Esc>");
    compile(&mut viewer);
    let (_, _, _, message) = viewer
        .diagnostics
        .iter()
        .find(|(_, _, error, _)| *error)
        .cloned()
        .unwrap();
    assert_eq!(
        super::refused::for_writer(&message),
        "there is no file “slides.pdf”"
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_page_put_in_is_drawn_once_exported_whole_and_typing_beside_it_shows_the_whole() {
    let text = two_with(&[2]);
    let (mut viewer, directory) = with_slides(&text);
    let document = viewer.document.clone().unwrap();
    let world = viewer.compiled_world.clone().unwrap();
    // The window shows the PDF's page: its words are dark on the paper.
    let pixmap = zetwerk_render::render_page(&document.pages()[1], 1.0);
    let dark = pixmap
        .pixels()
        .iter()
        .filter(|pixel| pixel.red() < 100)
        .count();
    assert!(dark > 20, "{dark} dark pixels");
    // The PDF written holds it as a form, drawn as lines: no picture.
    let pdf = zetwerk_typst::export_pdf(document.document(), &world).unwrap();
    let written = String::from_utf8_lossy(&pdf);
    assert!(written.contains("/Count 3"));
    assert!(written.contains("/Form") && !written.contains("/Subtype /Image"));
    // Typed into the paragraph behind it, then all typeset again: the
    // tiles of the page put in are those of before, drawn once.
    let region = zetwerk_render::PixelRegion {
        x: 0,
        y: 0,
        width: 300,
        height: 200,
    };
    let before: Vec<_> = zetwerk_render::tiles(&document.pages()[1], 1.0, region)
        .iter()
        .map(|tile| tile.key)
        .collect();
    press(&mut viewer, "gsgs/Second<CR>iNew <Esc>");
    if viewer.document_revision == Some(viewer.revision) {
        let shown = viewer.document.clone().unwrap();
        assert_eq!(
            shown.pages().len(),
            3,
            "a part typeset by itself keeps the pages"
        );
    }
    compile(&mut viewer);
    let after = viewer.document.clone().unwrap();
    let again: Vec<_> = zetwerk_render::tiles(&after.pages()[1], 1.0, region)
        .iter()
        .map(|tile| tile.key)
        .collect();
    assert_eq!(before, again);
    // As slides and in the overview: `]]` comes to it.
    press(&mut viewer, "gg:slides<CR>]]");
    assert!(viewer.page_here().is_some());
    press(&mut viewer, ":grid<CR>");
    assert!(viewer.is_grid());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn an_agent_puts_pages_in_or_proposes_them() {
    let (mut viewer, directory) = with_slides(TWO);
    let wanted = crate::page_text::Wanted {
        file: "slides.pdf".to_owned(),
        pages: Some("2".to_owned()),
        place: Some("end".to_owned()),
        ..crate::page_text::Wanted::default()
    };
    viewer.agent_edits = super::super::agent::AgentEdits::Propose;
    let proposed = viewer.agent_insert_pages(&wanted, Some(0), false);
    assert!(
        matches!(proposed, Ok(super::super::agent::Outcome::Proposed { .. })),
        "{proposed:?}"
    );
    assert_eq!(text_of(&viewer), TWO);
    viewer.agent_edits = super::super::agent::AgentEdits::Live;
    viewer.reject_proposals(true);
    assert!(viewer.agent_insert_pages(&wanted, Some(0), false).is_ok());
    assert_eq!(text_of(&viewer), format!("{TWO}\n{}\n", slide(2)));
    let listed = viewer.agent_pages_put_in();
    assert_eq!(listed[0]["file"], "slides.pdf");
    assert!(
        viewer.agent_insert_pages(&wanted, Some(0), false).is_err(),
        "an old revision"
    );
    let _ = std::fs::remove_dir_all(directory);
}
