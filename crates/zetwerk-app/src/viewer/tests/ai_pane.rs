//! The AI pane: beside the page, what is asked of agents, what came of
//! it, and what they are doing; asked from the command line.

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
fn the_pane_stands_beside_the_page_and_asks_from_the_command_line() {
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

    // A question about the paragraph of the cursor: `i` puts `:ai ` on
    // the command line, and the pane is in front again once it is asked.
    press(&mut viewer, "i");
    assert_eq!(
        viewer.editor.as_ref().and_then(|e| e.command_line()),
        Some("ai ")
    );
    press(&mut viewer, "is this clear?<CR>");
    assert!(viewer.ai_focused(), "back to the pane");
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
    assert!(threads[0].title.contains("is this clear?"));
    assert!(threads[0].state.text.contains("waiting for an agent"));
    assert!(threads[0].open);
    assert!(
        threads[0]
            .body
            .iter()
            .any(|line| line.plain().contains("A first paragraph"))
    );
    assert!(pane_drawn(&mut viewer), "the pane typesets with a thread");

    // A review, asked as one.
    press(&mut viewer, "i/review clarity, keep it short<CR>");
    assert!(viewer.ai_focused());
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
    // Left with Escape, the command line gives the pane back too.
    press(&mut viewer, "ifoo<Esc>");
    assert_eq!(viewer.notes().iter().filter(|n| n.to_agent).count(), 1);
    press(&mut viewer, "zR");
    assert!(pane_drawn(&mut viewer), "the pane typesets unfolded");
    press(&mut viewer, "zMGgg");
    press(&mut viewer, "]]j<CR>");
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
    assert_eq!(
        readable("A. Author #h(2em) B. Author #text(size: 9pt)[Institute] here"),
        "A. Author B. Author Institute here"
    );
}

#[test]
fn a_question_can_be_asked_where_there_is_no_text_yet() {
    // A new document: an agent is asked to start it.
    let (mut viewer, directory) = editing("");
    press(
        &mut viewer,
        "gIistart a short paper on the Euler-Lagrange equation<CR>",
    );
    let asked: Vec<_> = viewer
        .notes()
        .iter()
        .filter(|n| n.to_agent)
        .cloned()
        .collect();
    assert_eq!(asked.len(), 1, "asked on an empty line");
    assert_eq!(asked[0].quote, "");
    assert!(viewer.ai_threads()[0].title.contains("start a short paper"));
    assert!(viewer.ai_focused());

    // What is written there later does not lose the question its place.
    press(&mut viewer, "<C-w>hiThe equation.<Esc>");
    let note = viewer.notes().iter().find(|n| n.to_agent).cloned();
    assert!(note.is_some_and(|note| !note.detached));
    // A plain note still needs text to be on.
    press(&mut viewer, "o<Esc>:note here<CR>");
    assert_eq!(viewer.notes().iter().filter(|n| !n.to_agent).count(), 0);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_pane_is_a_buffer_to_move_in_and_copy_from() {
    let (mut viewer, directory) = editing("A first paragraph, about averages.\n");
    press(&mut viewer, "gIiis it clear?<CR>");
    let id = viewer.notes().iter().find(|n| n.to_agent).map(|n| n.id);
    let Some(id) = id else {
        panic!("asked");
    };
    // The agent answers in several lines; they stay lines in the pane.
    let answered =
        viewer.agent_answer_note(id, "Mostly.\nThe second sentence\nis long.", None, None);
    assert!(answered.is_ok(), "{answered:?}");
    assert!(pane_drawn(&mut viewer));
    assert!(viewer.ai_focused());
    let register = |viewer: &Viewer| {
        viewer
            .editor
            .as_ref()
            .and_then(|e| e.engine().register('"').map(|(text, _)| text.to_owned()))
    };
    // To the last line (the answer's last), and the last two copied.
    press(&mut viewer, "GkVjy");
    assert_eq!(
        register(&viewer).as_deref(),
        Some("The second sentence\nis long.\n")
    );
    // A word of a line, by character.
    press(&mut viewer, "G^vly");
    assert_eq!(register(&viewer).as_deref(), Some("is"));
    assert!(viewer.ai_focused(), "copying keeps the pane in front");
    press(&mut viewer, "<Esc>");
    assert!(!viewer.ai_focused());
    let _ = std::fs::remove_dir_all(directory);
}
