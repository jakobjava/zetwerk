//! The AI pane: beside the page, what is asked of agents, what came of
//! it, and what they are doing; asked from its own line.

use super::*;

/// Whether the pane was typeset and drawn: an image as wide as it, at
/// the right of the window.
fn pane_drawn(viewer: &mut Viewer) -> bool {
    let width = viewer.ai_width();
    let left = (viewer.window.0 - width) as f32;
    let scene = viewer.scene(&|_| false);
    scene
        .quads
        .iter()
        .any(|quad| (quad.x - left).abs() < 0.5 && (quad.width - width as f32).abs() < 2.0)
}

#[test]
fn the_pane_stands_beside_the_page_and_asks_from_its_own_line() {
    let (mut viewer, directory) = editing("A first paragraph, about averages.\n\nA second one.\n");
    press(&mut viewer, "gsgs");
    let whole = viewer.split().2;
    press(&mut viewer, "gI");
    let width = viewer.ai_width();
    assert!(width > 200, "{width}");
    assert_eq!(viewer.split(), (0, 0, whole - width));
    assert!(viewer.ai_focused());
    assert!(
        pane_drawn(&mut viewer),
        "the pane typesets with nothing asked"
    );

    // A question, typed in its line, about the paragraph of the cursor.
    press(&mut viewer, "iis this clear?<CR>");
    let asked: Vec<_> = viewer
        .notes()
        .iter()
        .filter(|note| note.to_agent)
        .map(|note| (note.text.clone(), note.quote.clone()))
        .collect();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(asked[0].0, "is this clear?");
    assert!(asked[0].1.contains("A first paragraph"), "{asked:?}");
    let threads = viewer.ai_threads();
    assert_eq!(threads.len(), 1);
    assert!(threads[0].markup.contains("is this clear?"));
    assert!(threads[0].markup.contains("waiting for an agent"));
    assert!(pane_drawn(&mut viewer), "the pane typesets with a thread");

    // A review, asked as one.
    press(&mut viewer, "i/review clarity, keep it short<CR>");
    assert_eq!(viewer.review_requests().len(), 1);
    assert_eq!(viewer.review_requests()[0].what, "clarity");
    assert_eq!(viewer.review_requests()[0].words, "keep it short");
    assert_eq!(viewer.ai_threads().len(), 2);
    assert!(pane_drawn(&mut viewer));

    // To the page and back; Enter on a thread goes to its text.
    press(&mut viewer, "<C-w>h");
    assert!(!viewer.ai_focused());
    press(&mut viewer, "j");
    assert!(!viewer.ai_focused(), "j is the page's");
    press(&mut viewer, "<C-w>l");
    assert!(viewer.ai_focused());
    press(&mut viewer, "j<CR>");
    assert!(!viewer.ai_focused(), "Enter goes to the page");
    // Put away: the page has the window again.
    press(&mut viewer, "<C-w>lq");
    assert_eq!(viewer.ai_width(), 0);
    assert_eq!(viewer.split().2, whole);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn what_the_line_asks_is_a_question_or_a_request() {
    use crate::viewer::ai_pane::asked;
    assert_eq!(
        asked("is (3) right?").map(|a| a.0).as_deref(),
        Some("ask is (3) right?")
    );
    assert_eq!(
        asked("/review").map(|a| a.0).as_deref(),
        Some("review this anything")
    );
    assert_eq!(
        asked("/check").map(|a| a.0).as_deref(),
        Some("review this maths")
    );
    assert!(asked("/cite").is_some_and(|a| a.0.starts_with("ask Find sources")));
    assert_eq!(asked("  "), None);
}

#[test]
fn the_pane_quotes_the_text_as_it_reads_not_its_source() {
    use crate::viewer::ai_pane::readable;
    assert_eq!(
        readable(
            "#set page(paper: \"a4\")\n#set text(size: 11pt)\n\n*Abstract.* We study $x^2$ here."
        ),
        "Abstract. We study x^2 here."
    );
    assert_eq!(readable("= Introduction\n\nWords."), "Introduction Words.");
}
