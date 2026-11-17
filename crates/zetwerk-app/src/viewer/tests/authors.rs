//! Who wrote what, as the author meets it: typed text, an agent's
//! change made live and one proposed and accepted, text yanked and put,
//! undo, a restart; the tint on the page, the story of a sentence, the
//! statement and the attribution as a file.

use super::*;
use zetwerk_core::{Kind, Origin};

/// The kinds of the bytes of `text` in the viewer's text, where it is.
fn kinds_of(viewer: &Viewer, text: &str) -> Vec<Kind> {
    let editor = viewer.editor.as_ref().unwrap();
    let at = editor
        .text()
        .find(text)
        .unwrap_or_else(|| panic!("{text:?} is not in the text"));
    let origins = editor.origins();
    (at..at + text.len())
        .map(|offset| origins.kind(origins.map().at(offset).unwrap()))
        .collect()
}

fn all(viewer: &Viewer, text: &str, kind: Kind) -> bool {
    kinds_of(viewer, text).iter().all(|found| *found == kind)
}

fn claude() -> Agent {
    Agent {
        name: "Claude Code".to_owned(),
        version: "2.1".to_owned(),
        session: "s1".to_owned(),
    }
}

use crate::viewer::authors::Agent;

#[test]
fn every_change_is_credited_to_whoever_made_it() {
    let (mut viewer, directory) = editing("Start.\n");
    press(&mut viewer, "A Mine.<Esc>");
    assert!(all(&viewer, "Mine.", Kind::Own));
    // What was in the file before is not said to be anyone's.
    assert!(all(&viewer, "Start.", Kind::Unknown));

    // An agent, live.
    viewer
        .as_agent(Some(claude()), |viewer| {
            viewer.agent_replace("Mine.", "Mine. The agent wrote this.", false, None, false)
        })
        .unwrap();
    assert!(all(&viewer, "The agent wrote this.", Kind::Agent));
    assert!(
        all(&viewer, "Mine.", Kind::Own),
        "the words it kept stay mine"
    );
    let editor = viewer.editor.as_ref().unwrap();
    let at = editor.text().find("agent").unwrap();
    let credited = editor
        .origins()
        .credited(editor.origins().map().at(at).unwrap());
    assert_eq!(credited.by.origin, Origin::Agent(claude()));

    // Proposed, then accepted by the author.
    viewer
        .as_agent(Some(claude()), |viewer| {
            viewer.agent_replace("Start.", "Begin here.", false, None, true)
        })
        .unwrap();
    press(&mut viewer, "gg0");
    assert_eq!(viewer.accept_proposals(false), 1);
    let editor = viewer.editor.as_ref().unwrap();
    let at = editor.text().find("Begin").unwrap();
    let credited = editor
        .origins()
        .credited(editor.origins().map().at(at).unwrap());
    assert_eq!(credited.by.origin, Origin::Accepted(claude()));

    // The author changes a word of the agent's: that word is the
    // author's, over the agent's.
    let at = viewer.text().unwrap().find("wrote").unwrap();
    viewer.editor.as_mut().unwrap().set_cursor(at);
    press(&mut viewer, "cwmade<Esc>");
    assert!(all(&viewer, "made", Kind::Edited));
    assert!(all(&viewer, "The agent", Kind::Agent));

    // Undo brings back the agent's word as the agent's.
    press(&mut viewer, "u");
    assert!(all(&viewer, "wrote", Kind::Agent));
    press(&mut viewer, "<C-r>");
    assert!(all(&viewer, "made", Kind::Edited));

    // A restart keeps it all.
    press(&mut viewer, ":w<CR>");
    drop(viewer);
    let (viewer, _) = open(&directory);
    assert!(all(&viewer, "made", Kind::Edited));
    assert!(all(&viewer, "The agent", Kind::Agent));
    assert!(all(&viewer, "Mine.", Kind::Own));
    assert!(
        all(&viewer, "Begin here.", Kind::Agent),
        "{:?}",
        kinds_of(&viewer, "Begin here.")
    );
}

#[test]
fn text_yanked_and_put_keeps_where_it_came_from() {
    let (mut viewer, _directory) = editing("One.\n\nTwo.\n");
    viewer
        .as_agent(Some(claude()), |viewer| {
            viewer.agent_replace("Two.", "Two words.", false, None, false)
        })
        .unwrap();
    press(&mut viewer, "gsgs");
    compile(&mut viewer);
    let at = viewer.text().unwrap().find("words").unwrap();
    viewer.editor.as_mut().unwrap().set_cursor(at);
    press(&mut viewer, "yiwgg0P");
    let text = viewer.text().unwrap().to_owned();
    assert!(text.starts_with("wordsOne."), "{text:?}");
    let kinds = kinds_of(&viewer, "wordsOne.");
    assert!(
        kinds[..5].iter().all(|kind| *kind == Kind::Agent),
        "{kinds:?}"
    );
    assert!(
        kinds[5..].iter().all(|kind| *kind == Kind::Unknown),
        "{kinds:?}"
    );

    // From the system clipboard: pasted.
    viewer.clipboard.read = || Ok("elsewhere".to_owned());
    press(&mut viewer, "G$\"+p");
    assert!(all(&viewer, "elsewhere", Kind::Pasted));
}

#[test]
fn the_page_is_tinted_by_who_wrote_it_and_the_status_line_tells_the_story() {
    let (mut viewer, _directory) = editing("Mine here.\n\nAnother line of text.\n");
    viewer
        .as_agent(Some(claude()), |viewer| {
            viewer.agent_replace("Another", "An agent's", false, None, false)
        })
        .unwrap();
    press(&mut viewer, "gsgs");
    compile(&mut viewer);
    press(&mut viewer, "gA");
    let scene = viewer.scene(&|_| false);
    assert!(!scene.quads.is_empty());
    let tinted = &viewer.authors.tinted;
    let cells: usize = tinted.values().map(|(_, cells)| cells.len()).sum();
    assert!(cells > 0, "the agent's words are tinted");
    // The story of the sentence under the cursor.
    let at = viewer.text().unwrap().find("agent").unwrap();
    viewer.editor.as_mut().unwrap().set_cursor(at);
    let told = viewer.authors_status().unwrap();
    assert!(told.contains("Written by Claude Code 2.1"), "{told}");
    // The step that put it there.
    press(&mut viewer, "gH");
    assert!(viewer.history.is_some(), "the history is up");
    press(&mut viewer, "<Esc>gA");
    assert!(viewer.authors_status().is_none());
}

#[test]
fn shares_a_statement_and_the_attribution_as_a_file() {
    let (mut viewer, directory) = editing("= Intro\nMine is this text.\n\n= Rest\nMore of mine.\n");
    viewer
        .as_agent(Some(claude()), |viewer| {
            viewer.agent_replace("More of mine.", "More from the agent.", false, None, false)
        })
        .unwrap();
    press(&mut viewer, ":authors<CR>");
    let scene = viewer.scene(&|_| false);
    assert!(!scene.quads.is_empty());
    let value = viewer.authorship(Some((1, 5))).unwrap();
    let agent = value["shares"]["characters_percent"]["agent, as written"]
        .as_f64()
        .unwrap();
    assert!(agent > 10.0 && agent < 60.0, "{value}");
    assert!(
        value["runs"]
            .as_array()
            .is_some_and(|runs| !runs.is_empty())
    );
    assert_eq!(
        value["sections"].as_array().map(Vec::len),
        Some(2),
        "{value}"
    );

    press(&mut viewer, ":authors statement<CR>");
    let statement = std::fs::read_to_string(directory.join("main.authorship.md")).unwrap();
    assert!(statement.contains("Claude Code 2.1"), "{statement}");
    assert!(statement.contains("only as complete as that history"));
    // Not written over without `!`.
    press(&mut viewer, ":authors statement<CR>");
    assert!(viewer.message.as_ref().is_some_and(|message| message.alert));

    press(&mut viewer, ":authors export<CR>");
    let exported = std::fs::read_to_string(directory.join("main.authorship.tsv")).unwrap();
    assert!(exported.contains("Claude Code"));

    // On another machine: the text without its history, the file beside
    // it, read back.
    let elsewhere = super::directory(&viewer.text().unwrap().replace("Mine is", "Mine was"));
    std::fs::write(elsewhere.join("main.authorship.tsv"), &exported).unwrap();
    let (mut other, _) = open(&elsewhere);
    assert!(all(&other, "from the agent", Kind::Unknown));
    assert!(all(&viewer, "from the agent.", Kind::Agent));
    press(&mut other, ":authors import<CR>");
    assert!(
        all(&other, "from the agent.", Kind::Agent),
        "{:?}",
        kinds_of(&other, "More from the agent.")
    );
    assert!(all(&other, "More", Kind::Unknown), "it was in the file before");
    // What changed since is unknown.
    assert!(kinds_of(&other, "Mine was").contains(&Kind::Unknown));
    press(&mut other, "u");
    assert!(all(&other, "from the agent", Kind::Unknown));
}

/// Whether the attribution of the text covers it exactly: every byte
/// once, in order, with credits that are known. Asked by the random
/// typist after every action.
pub(super) fn covers(editor: &crate::editor::Editor) -> Result<(), String> {
    let origins = editor.origins();
    let mut at = 0;
    for (range, mark) in origins.map().iter() {
        if range.start != at || range.is_empty() {
            return Err(format!("the attribution has {range:?} after byte {at}"));
        }
        for id in [Some(mark.by), mark.over].into_iter().flatten() {
            if id as usize >= origins.credits().len() {
                return Err(format!("credit {id} is not known"));
            }
        }
        at = range.end;
    }
    if at != editor.text().len() {
        return Err(format!(
            "the attribution covers {at} bytes of a text of {}",
            editor.text().len()
        ));
    }
    Ok(())
}
