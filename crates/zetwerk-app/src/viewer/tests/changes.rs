//! What changed since a version, on the page: bars beside the lines,
//! keys to go to them, the old version set over the new, a proof of all
//! of it, and git (in repositories each test makes for itself).

use super::*;
use crate::git::tests::{commit_all, init, isolated};
use crate::viewer::changes::{CHANGE_BAR, CHANGE_GONE};

const PAGE: &str = "#set page(width: 300pt, height: 400pt)\n";

fn text() -> String {
    format!(
        "{PAGE}First paragraph with a few words in it.\n\n\
         Second paragraph, which goes on for long enough to take more \
         than one line of the page.\n\nThird and last.\n"
    )
}

/// What the status line says of changes.
fn status_of(viewer: &mut Viewer) -> String {
    viewer.changes_status()
}

/// The message of the status line.
fn said(viewer: &Viewer) -> String {
    viewer
        .message
        .as_ref()
        .map_or_else(String::new, |message| message.text.clone())
}

fn cursor(viewer: &Viewer) -> usize {
    viewer.editor.as_ref().unwrap().cursor()
}

/// The bars drawn, by where they are.
fn bars(viewer: &mut Viewer) -> Vec<[i32; 4]> {
    let mut found: Vec<[i32; 4]> = tints(viewer, CHANGE_BAR)
        .iter()
        .map(|quad| [quad.x, quad.y, quad.width, quad.height].map(|v| v.round() as i32))
        .collect();
    found.sort_unstable();
    found
}

#[test]
fn a_line_that_changed_has_a_bar_until_it_is_as_it_was() {
    let text = text();
    let (mut viewer, directory) = editing(&text);
    press(&mut viewer, "gsgs");
    assert_eq!(bars(&mut viewer), Vec::<[i32; 4]>::new());
    assert_eq!(status_of(&mut viewer), "");
    press(&mut viewer, "/Third<CR>A Added words.<Esc>");
    compile(&mut viewer);
    let drawn = bars(&mut viewer);
    assert_eq!(drawn.len(), 1, "{drawn:?}");
    assert_eq!(status_of(&mut viewer), "1 change ]v  ");
    // Left of the text, right of the numbers of the lines.
    let lines = viewer
        .changes_lines(0, |lines| lines.extent(0))
        .flatten()
        .unwrap();
    let (_, left, _) = viewer.split();
    let page = viewer
        .view
        .visible(&viewer.layout)
        .into_iter()
        .next()
        .unwrap();
    let edge = f64::from(left) + page.x as f64 + lines.0 * viewer.view.zoom;
    let x = f64::from(drawn[0][0]);
    assert!(x < edge && x > edge - 20.0, "{drawn:?} by {edge}");
    // The first word of the long paragraph too: the bar of only its
    // first line, apart from the other (lines that follow each other
    // have one bar).
    press(&mut viewer, "/Second<CR>cwNext<Esc>");
    compile(&mut viewer);
    let drawn = bars(&mut viewer);
    assert_eq!(drawn.len(), 2, "{drawn:?}");
    assert_eq!(status_of(&mut viewer), "2 changes ]v  ");
    // Undone: gone.
    press(&mut viewer, "uu");
    compile(&mut viewer);
    assert_eq!(bars(&mut viewer), Vec::<[i32; 4]>::new());
    // Saved: what is compared with is the file as it is now.
    press(&mut viewer, "/First<CR>A More.<Esc>");
    compile(&mut viewer);
    assert_eq!(bars(&mut viewer).len(), 1);
    press(&mut viewer, ":w<CR>");
    assert_eq!(bars(&mut viewer).len(), 0);
    // Turned off and on.
    press(&mut viewer, "A Again.<Esc>:changes off<CR>");
    compile(&mut viewer);
    assert_eq!(bars(&mut viewer).len(), 0);
    press(&mut viewer, ":changes on<CR>");
    assert_eq!(bars(&mut viewer).len(), 1);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn only_blanks_and_line_breaks_of_the_source_are_no_change() {
    let text = text();
    let (mut viewer, directory) = editing(&text);
    press(&mut viewer, "gsgs/few<CR>i  <Esc>");
    compile(&mut viewer);
    assert_eq!(bars(&mut viewer).len(), 0);
    assert_eq!(status_of(&mut viewer), "");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_paragraph_taken_out_leaves_a_mark_between_the_lines() {
    let text = text();
    let (mut viewer, directory) = editing(&text);
    press(&mut viewer, "gsgs/Second<CR>dd");
    compile(&mut viewer);
    assert!(!viewer.text().unwrap().contains("Second"));
    let _ = viewer.scene(&|_| false);
    assert_eq!(viewer.changes_placed(0), (0, 1));
    assert_eq!(tints(&mut viewer, CHANGE_GONE).len(), 2);
    // `zc` on the paragraph after it shows what stood there; `:revert`
    // puts it back, as one step.
    press(&mut viewer, "/Third<CR>:revert<CR>");
    assert_eq!(viewer.text(), Some(text.as_str()));
    press(&mut viewer, "u");
    assert!(!viewer.text().unwrap().contains("Second"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn keys_go_from_change_to_change_and_round() {
    let text = text();
    let (mut viewer, directory) = editing(&text);
    press(&mut viewer, "gsgs/few<CR>cwmany<Esc>/last<CR>cwfinal<Esc>");
    compile(&mut viewer);
    let (first, last) = (text.find("few").unwrap(), text.find("last").unwrap() + 1);
    press(&mut viewer, "gg");
    let top = cursor(&viewer);
    press(&mut viewer, "]v");
    assert_eq!(cursor(&viewer), first, "from {top}: {}", said(&viewer));
    assert!(
        said(&viewer).starts_with("change 1 of 2"),
        "{}",
        said(&viewer)
    );
    press(&mut viewer, "]v");
    assert_eq!(cursor(&viewer), last);
    press(&mut viewer, "]v");
    assert_eq!(cursor(&viewer), first);
    assert!(said(&viewer).contains("other end"));
    press(&mut viewer, "[v");
    assert_eq!(cursor(&viewer), last);
    // The list of them goes to one.
    press(&mut viewer, "gg:changes list<CR>");
    let (_, shown, _) = viewer.picker.as_ref().unwrap().listing();
    assert_eq!(shown.len(), 2);
    assert!(shown[0].contains("many"), "{shown:?}");
    press(&mut viewer, "<CR>");
    assert_eq!(cursor(&viewer), first);
    // `:changes` alone is still Vim's list of the places of changes.
    press(&mut viewer, ":changes<CR>");
    assert!(viewer.picker.is_some());
    press(&mut viewer, "<Esc>");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_old_version_of_one_paragraph_comes_back_as_one_step() {
    let text = text();
    let (mut viewer, directory) = editing(&text);
    press(
        &mut viewer,
        "gsgs/few<CR>cwmany<Esc>/Third<CR>cwFourth<Esc>",
    );
    compile(&mut viewer);
    let changed = viewer.text().unwrap().to_owned();
    // Set over itself as it was: what went struck, what came tinted.
    press(&mut viewer, "zc");
    assert!(said(&viewer).starts_with("as it was"), "{}", said(&viewer));
    let images = viewer.scene(&|_| false).images.len();
    assert_ne!(images, 0);
    // Enter takes it back: only this paragraph.
    press(&mut viewer, "<CR>");
    let now = viewer.text().unwrap();
    assert!(
        now.contains("Third and last.") && now.contains("many words"),
        "{now}"
    );
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some(changed.as_str()));
    // `y` on it yanks the old text; `p` puts it.
    press(&mut viewer, "/many<CR>zcy");
    assert!(!viewer.slip_shown());
    press(&mut viewer, "Go<Esc>p");
    assert!(
        viewer.text().unwrap().ends_with("a few words in it.\n")
            || viewer
                .text()
                .unwrap()
                .contains("\nFirst paragraph with a few words in it.")
    );
    // Any other key takes it away and is a key as ever.
    press(&mut viewer, "u/many<CR>zcl");
    assert!(!viewer.slip_shown());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn what_is_compared_with_can_be_the_text_as_opened_or_a_step() {
    let text = text();
    let (mut viewer, directory) = editing(&text);
    press(&mut viewer, "gsgs/few<CR>cwmany<Esc>:w<CR>");
    assert_eq!(status_of(&mut viewer), "");
    press(&mut viewer, ":changes since opened<CR>");
    assert_eq!(status_of(&mut viewer), "1 change since opened ]v  ");
    press(&mut viewer, "/Third<CR>cwFourth<Esc>");
    assert_eq!(status_of(&mut viewer), "2 changes since opened ]v  ");
    // A step of the history: the list of them, and the one chosen.
    press(&mut viewer, ":changes since history<CR>");
    assert!(viewer.picker.is_some());
    press(&mut viewer, "<Esc>:changes since step 1<CR>");
    assert_eq!(status_of(&mut viewer), "1 change since step 1 ]v  ");
    press(&mut viewer, ":changes since saved<CR>");
    assert_eq!(status_of(&mut viewer), "1 change ]v  ");
    press(&mut viewer, ":changes since nothing-like-it<CR>");
    assert!(
        said(&viewer).starts_with("no git here"),
        "{}",
        said(&viewer)
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_bars_stay_while_a_word_is_typed_and_its_pages_come() {
    // Frame by frame: after a key, the frame after it, when the pages of
    // the text before it land late, and when its own pages come.
    let text = text();
    let (mut viewer, directory) = editing(&text);
    press(&mut viewer, "gsgs/Second<CR>A wor");
    compile(&mut viewer);
    let settled = bars(&mut viewer);
    assert_eq!(settled.len(), 1);
    for key in ["d", "s", " ", "a"] {
        let (revision, world) = viewer.snapshot();
        press(&mut viewer, key);
        assert_eq!(bars(&mut viewer), settled, "after {key}");
        assert_eq!(bars(&mut viewer), settled, "after {key}, again");
        let compiled = zetwerk_typst::compile(&world);
        viewer.compiled(CompileOutput {
            revision,
            world: Arc::new(world),
            compiled,
            duration: Duration::from_millis(3),
        });
        assert_eq!(bars(&mut viewer), settled, "after {key}, late pages");
        compile(&mut viewer);
        assert_eq!(bars(&mut viewer), settled, "after {key}, its pages");
    }
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_proof_marks_the_changes_in_the_typeset_document_and_is_written_out() {
    let text = text();
    let (mut viewer, directory) = editing(&text);
    press(&mut viewer, "gsgs/few<CR>cwmany<Esc>/Second<CR>dd");
    compile(&mut viewer);
    press(&mut viewer, ":changes proof<CR>");
    assert!(
        said(&viewer).starts_with("proof since saved: 2 changes marked"),
        "{}",
        said(&viewer)
    );
    let (_, world) = viewer.snapshot();
    let main = world.main();
    let proof = world.source(main).unwrap();
    assert!(
        proof
            .text()
            .contains("#zetwerk-old[few] #zetwerk-new[many]")
    );
    assert!(proof.text().contains("#zetwerk-gone[Second paragraph"));
    compile(&mut viewer);
    assert!(viewer.problems.is_empty(), "{:?}", viewer.problems);
    assert_eq!(viewer.document_revision, Some(viewer.revision));
    // The pages are not the text's: nothing is placed by them.
    assert!(viewer.places_of_text().is_none());
    // Read, not written: keys move through it.
    let before = viewer.text().unwrap().to_owned();
    press(&mut viewer, "xdd]vn");
    assert_eq!(viewer.text(), Some(before.as_str()));
    press(&mut viewer, ":changes pdf marked.pdf<CR>");
    let _ = viewer.scene(&|_| false);
    assert!(directory.join("marked.pdf").exists(), "{}", said(&viewer));
    press(&mut viewer, "<Esc>");
    assert!(!viewer.proof_shown());
    compile(&mut viewer);
    assert!(viewer.places_of_text().is_some());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn without_git_nothing_is_said_of_it_and_nothing_is_committed() {
    let text = text();
    let (mut viewer, directory) = editing(&text);
    viewer.git_on();
    let _ = viewer.scene(&|_| false);
    viewer.git_settled();
    assert!(!status_of(&mut viewer).contains("git"));
    press(&mut viewer, ":git commit All<CR>");
    assert!(
        said(&viewer).starts_with("no git here"),
        "{}",
        said(&viewer)
    );
    press(&mut viewer, ":git log<CR>");
    assert!(viewer.picker.is_none());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn git_says_the_branch_gives_older_versions_and_commits_the_documents_files() {
    let text = text();
    let directory = directory(&text);
    std::fs::write(directory.join(".gitignore"), "state/\n").unwrap();
    std::fs::write(directory.join("notes.txt"), "not the document's\n").unwrap();
    let repo = isolated(&directory);
    init(&repo);
    commit_all(&repo, "The first version");
    let (mut viewer, _) = open(&directory);
    viewer.git_on();
    let _ = viewer.scene(&|_| false);
    viewer.git_settled();
    assert_eq!(status_of(&mut viewer), "git main  ");
    // Changed and saved: differs from HEAD; asked again because saved.
    press(&mut viewer, "gsgs/few<CR>cwmany<Esc>:w<CR>");
    let _ = viewer.scene(&|_| false);
    viewer.git_settled();
    assert_eq!(status_of(&mut viewer), "git main*  ");
    // Compared with HEAD: the change saved is one.
    press(&mut viewer, ":changes since HEAD<CR>");
    viewer.git_settled();
    compile(&mut viewer);
    assert_eq!(
        status_of(&mut viewer),
        "1 change since HEAD ]v  git main*  "
    );
    assert!(
        said(&viewer).contains("The first version"),
        "{}",
        said(&viewer)
    );
    assert_eq!(bars(&mut viewer).len(), 1);
    // The commits of the file, to compare with.
    press(&mut viewer, ":git log<CR>");
    viewer.git_settled();
    let (_, shown, _) = viewer.picker.as_ref().unwrap().listing();
    assert_eq!(shown.len(), 1);
    assert!(shown[0].contains("The first version"));
    press(&mut viewer, "<Esc>");
    // Not with unsaved changes.
    press(&mut viewer, "/Third<CR>cwFourth<Esc>:git commit Words<CR>");
    assert!(
        said(&viewer).starts_with("unsaved changes"),
        "{}",
        said(&viewer)
    );
    // Saved: the files are shown first; n leaves them.
    std::fs::write(
        directory.join("notes.txt"),
        "changed, still not the document's\n",
    )
    .unwrap();
    press(&mut viewer, ":w<CR>:git commit Words<CR>");
    viewer.git_settled();
    assert!(
        said(&viewer).starts_with("commit 1 files?"),
        "{}",
        said(&viewer)
    );
    press(&mut viewer, "n");
    assert_eq!(said(&viewer), "not committed");
    press(&mut viewer, ":git commit Words<CR>");
    viewer.git_settled();
    press(&mut viewer, "y");
    viewer.git_settled();
    assert!(
        said(&viewer).starts_with("committed: [main"),
        "{}",
        said(&viewer)
    );
    let committed = std::process::Command::new("git")
        .args([
            "-C",
            directory.to_str().unwrap(),
            "show",
            "--name-only",
            "--format=%s",
            "HEAD",
        ])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CEILING_DIRECTORIES", directory.parent().unwrap())
        .output()
        .unwrap();
    let committed = String::from_utf8_lossy(&committed.stdout).into_owned();
    assert_eq!(committed.trim(), "Words\n\nmain.typ", "{committed}");
    // HEAD moved: git is asked again when the watcher says so; compared
    // with HEAD, nothing differs.
    let watched = viewer.git_watched();
    assert_eq!(watched.len(), 2);
    viewer.git_files_changed(&watched);
    let _ = viewer.scene(&|_| false);
    viewer.git_settled();
    let _ = viewer.changes_status();
    viewer.git_settled();
    assert_eq!(status_of(&mut viewer), "git main  ");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_key_and_its_frame_cost_about_as_much_on_a_longer_document() {
    // With a change in the middle, compared with the file as saved: what
    // is compared again with each key is the paragraphs, not all words.
    const ARTICLE: &str = include_str!("../../../../zetwerk-corpus/docs/article.typ");
    let cost = |copies: usize| {
        let body = ARTICLE.find("\n= ").unwrap_or(0);
        let mut text = ARTICLE[..body].to_owned();
        for _ in 0..copies {
            text.push_str(&ARTICLE[body..]);
        }
        let (mut viewer, directory) = editing(&text);
        press(&mut viewer, "gsgs");
        if let Some(editor) = viewer.editor.as_mut() {
            editor.set_cursor(text.len() / 2);
        }
        press(&mut viewer, "Oa new paragraph<Esc>");
        compile(&mut viewer);
        press(&mut viewer, "A");
        let mut took: Vec<f64> = "the quick brown fox"
            .chars()
            .map(|c| {
                let started = std::time::Instant::now();
                press(&mut viewer, &c.to_string());
                let _ = viewer.scene(&|_| false);
                started.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        took.sort_by(f64::total_cmp);
        assert!(status_of(&mut viewer).starts_with("1 change"));
        let _ = std::fs::remove_dir_all(directory);
        took[took.len() / 2]
    };
    let (short, long) = (cost(1), cost(10));
    if std::env::var_os("ZETWERK_SPEED").is_some() {
        println!("a key and its frame: {short:.2} ms on 2 pages, {long:.2} ms on 20");
    }
    assert!(long < 4.0 * short + 8.0, "{short:.2} ms → {long:.2} ms");
}
