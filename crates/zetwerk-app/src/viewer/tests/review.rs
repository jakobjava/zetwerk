//! Reviewing with agents as an author meets it: suggestions made by an
//! agent, decided on with keys, answered, rewritten, kept across a
//! restart, listed in the manager, made into instructions and written
//! out as a record.

use super::*;
use crate::viewer::review::{Agent, Kind, State, Wanted};

const TEXT: &str = "= Intro\n\
\n\
The quick brown fox jumps over the lazy dog. Colour is nice.\n\
\n\
= Methods\n\
\n\
We recieve data and analyse it carefuly.\n";

fn agent() -> Agent {
    Agent {
        name: "claude".to_owned(),
        session: "s1".to_owned(),
    }
}

fn want(kind: &str, old: &str, new: Option<&str>, category: &str) -> Wanted {
    Wanted {
        kind: kind.to_owned(),
        old_str: Some(old.to_owned()),
        new_str: new.map(str::to_owned),
        reason: format!("{category}: better so."),
        category: Some(category.to_owned()),
        ..Wanted::default()
    }
}

/// The state of the suggestion `id`.
fn state(viewer: &Viewer, id: u64) -> State {
    viewer
        .suggestions()
        .iter()
        .find(|suggestion| suggestion.id == id)
        .map_or_else(
            || panic!("no suggestion {id}"),
            |suggestion| suggestion.state,
        )
}

/// The numbers the suggestions of a batch got.
fn made(answer: &serde_json::Value) -> Vec<u64> {
    answer["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|result| result["id"].as_u64())
        .collect()
}

fn said(viewer: &Viewer) -> String {
    viewer
        .message
        .as_ref()
        .map(|message| message.text.clone())
        .unwrap_or_default()
}

#[test]
fn a_batch_is_checked_whole_and_each_item_answered() {
    let (mut viewer, directory) = editing(TEXT);
    let batch = vec![
        want("replace", "recieve", Some("receive"), "spelling"),
        want("replace", "ve", Some("x"), "grammar"),
        want("replace", "nowhere", Some("x"), "grammar"),
        Wanted {
            section: Some("Methods".to_owned()),
            ..want("replace", "carefuly", Some("carefully"), "spelling")
        },
        want("delete", "recieve data", None, "concision"),
        want("insert", "lazy dog.", Some(" It sleeps."), "clarity"),
        Wanted {
            kind: "comment".to_owned(),
            old_str: None,
            section: Some("Intro".to_owned()),
            reason: "This section is short.".to_owned(),
            ..Wanted::default()
        },
        want("explode", "fox", Some("cat"), "x"),
    ];
    let answer = viewer
        .agent_suggest(&batch, None, false, Some(0), false, &agent())
        .unwrap();
    let statuses: Vec<&str> = answer["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|result| result["status"].as_str().unwrap())
        .collect();
    assert_eq!(
        statuses,
        [
            "anchored",
            "ambiguous",
            "not_found",
            "anchored",
            "overlaps",
            "anchored",
            "anchored",
            "invalid"
        ],
        "{answer}"
    );
    assert_eq!(viewer.text(), Some(TEXT), "suggested, not made");
    assert_eq!(viewer.suggestions().len(), 4);
    let comment = &viewer.suggestions()[3];
    assert_eq!(
        (comment.kind, &TEXT[comment.range.clone()]),
        (Kind::Comment, "= Intro")
    );
    // Each change waits as a proposal, for the older tools too.
    assert_eq!(viewer.proposals().len(), 3);
    // All or nothing: one bad item, none made.
    let none = viewer
        .agent_suggest(
            &[
                want("replace", "fox", Some("cat"), "x"),
                want("replace", "nowhere", Some("x"), "x"),
            ],
            None,
            false,
            None,
            true,
            &agent(),
        )
        .unwrap();
    assert_eq!(none["made"], 0, "{none}");
    // Made for another text: refused whole.
    assert!(
        viewer
            .agent_suggest(&batch, None, false, Some(7), false, &agent())
            .is_err()
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn suggestions_are_decided_with_one_key_each_and_undone() {
    let (mut viewer, directory) = editing(TEXT);
    let answer = viewer
        .agent_suggest(
            &[
                want("replace", "recieve", Some("receive"), "spelling"),
                want("replace", "carefuly", Some("carefully"), "spelling"),
                want("replace", "quick", Some("swift"), "tone"),
            ],
            None,
            false,
            None,
            false,
            &agent(),
        )
        .unwrap();
    let ids = made(&answer);
    assert_eq!(ids.len(), 3);
    press(&mut viewer, "gg]p");
    let cursor = viewer.editor.as_ref().unwrap().cursor();
    assert_eq!(
        cursor,
        TEXT.find("quick").unwrap(),
        "in the order of the text"
    );
    press(&mut viewer, "ga");
    assert!(viewer.text().unwrap().contains("The swift brown"));
    assert_eq!(state(&viewer, ids[2]), State::Accepted);
    // `u` takes the change back, and the suggestion is open again.
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some(TEXT));
    assert_eq!(state(&viewer, ids[2]), State::Open);
    assert_eq!(viewer.proposals().len(), 3);
    // Rejected, and taken back with :review undo.
    press(&mut viewer, "gr");
    assert_eq!(state(&viewer, ids[2]), State::Rejected);
    assert_eq!(viewer.proposals().len(), 2);
    press(&mut viewer, ":review undo<CR>");
    assert_eq!(state(&viewer, ids[2]), State::Open);
    // Many at once: one step.
    press(&mut viewer, ":review accept spelling<CR>");
    assert!(said(&viewer).contains("accepted 2"), "{}", said(&viewer));
    let text = viewer.text().unwrap().to_owned();
    assert!(
        text.contains("We receive data and analyse it carefully."),
        "{text}"
    );
    assert_eq!(state(&viewer, ids[2]), State::Open, "of another category");
    // The one between them still knows its text.
    let quick = viewer
        .proposals()
        .iter()
        .find(|p| p.old == "quick")
        .unwrap();
    assert_eq!(&text[quick.range.clone()], "quick");
    press(&mut viewer, "u");
    assert_eq!(viewer.text(), Some(TEXT));
    assert_eq!(state(&viewer, ids[0]), State::Open);
    assert_eq!(state(&viewer, ids[1]), State::Open);
    press(&mut viewer, "<C-r>");
    assert_eq!(state(&viewer, ids[0]), State::Accepted, "and done again");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_suggestion_is_rewritten_answered_and_its_wordings_stepped_through() {
    let (mut viewer, directory) = editing(TEXT);
    let ids = made(
        &viewer
            .agent_suggest(
                &[
                    want("replace", "quick brown", Some("fast brown"), "tone"),
                    want("replace", "lazy dog", Some("sleepy dog"), "tone"),
                ],
                None,
                false,
                None,
                false,
                &agent(),
            )
            .unwrap(),
    );
    // Rewritten: the agent's text put in, the cursor typing behind it.
    press(&mut viewer, "gg]p");
    press(&mut viewer, "gA");
    assert_eq!(
        viewer.editor.as_ref().unwrap().mode(),
        crate::editor::Mode::Insert
    );
    press(&mut viewer, "<BS><BS><BS><BS><BS>grey<Esc>");
    assert!(viewer.text().unwrap().contains("The fast grey fox"));
    viewer.review_ready();
    let rewritten = viewer
        .suggestions()
        .iter()
        .find(|s| s.id == ids[0])
        .unwrap();
    assert_eq!(rewritten.state, State::Rewritten);
    assert!(
        matches!(
            &rewritten.thread.last().unwrap().said,
            crate::viewer::review::Said::Rewritten { after, offered, .. }
                if after == "fast grey" && offered == "fast brown"
        ),
        "{:?}",
        rewritten.thread
    );
    // Answered: it waits for the agent then.
    press(&mut viewer, "]p");
    press(&mut viewer, "gq");
    assert_eq!(
        viewer.editor.as_ref().unwrap().command_line(),
        Some("answer ")
    );
    press(&mut viewer, "too cute<CR>");
    let lazy = viewer
        .suggestions()
        .iter()
        .find(|s| s.id == ids[1])
        .unwrap();
    assert!(lazy.waiting);
    let news = viewer.review_news(0);
    assert!(news.to_string().contains("too cute"), "{news}");
    // The agent offers other wordings; zp steps through them, ga takes
    // the one shown.
    viewer
        .agent_revise(
            ids[1],
            None,
            None,
            Some(vec!["drowsy dog".to_owned(), "idle dog".to_owned()]),
            Some("Here are two others."),
            None,
            &agent(),
        )
        .unwrap();
    press(&mut viewer, "zp");
    press(&mut viewer, "zp");
    assert!(
        said(&viewer).contains("wording 3 of 3"),
        "{}",
        said(&viewer)
    );
    press(&mut viewer, "ga");
    assert!(viewer.text().unwrap().contains("over the idle dog"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn a_rejection_becomes_an_instruction_that_refuses_it_next_time() {
    let (mut viewer, directory) = editing(TEXT);
    let colour = want("replace", "Colour", Some("Color"), "spelling");
    let ids = made(
        &viewer
            .agent_suggest(
                std::slice::from_ref(&colour),
                None,
                false,
                None,
                false,
                &agent(),
            )
            .unwrap(),
    );
    press(&mut viewer, "gg]p:reject British spelling stays<CR>");
    assert_eq!(state(&viewer, ids[0]), State::Rejected);
    press(&mut viewer, "g!");
    assert_eq!(
        viewer.editor.as_ref().unwrap().command_line(),
        Some("always British spelling stays")
    );
    press(&mut viewer, "<CR>");
    let [instruction] = viewer.brief() else {
        panic!("one instruction: {:?}", viewer.brief())
    };
    let instruction = instruction.clone();
    assert_eq!(
        instruction.refused,
        Some(("Colour".to_owned(), "Color".to_owned()))
    );
    // The same again is refused, and said why.
    let again = viewer
        .agent_suggest(
            std::slice::from_ref(&colour),
            None,
            false,
            None,
            false,
            &agent(),
        )
        .unwrap();
    assert_eq!(again["results"][0]["status"], "contradicts", "{again}");
    assert!(viewer.brief_text().contains("British spelling stays"));
    // Insisting, it is made, and flagged.
    let insisted = viewer
        .agent_suggest(
            &[Wanted {
                insist: true,
                ..colour
            }],
            None,
            false,
            None,
            false,
            &agent(),
        )
        .unwrap();
    assert_eq!(insisted["results"][0]["status"], "anchored");
    assert_eq!(
        viewer.suggestions().last().unwrap().flagged,
        vec![instruction.id]
    );
    // Text kept off by an instruction: no change of it is made.
    press(&mut viewer, "gg/lazy<CR>viwg!");
    press(&mut viewer, "leave the dog alone<CR>");
    let off = viewer
        .agent_suggest(
            &[want("replace", "lazy dog", Some("cat"), "tone")],
            None,
            false,
            None,
            false,
            &agent(),
        )
        .unwrap();
    assert_eq!(off["results"][0]["status"], "contradicts", "{off}");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_review_survives_a_restart_and_is_written_out() {
    let (mut viewer, directory) = editing(TEXT);
    let ids = made(
        &viewer
            .agent_suggest(
                &[
                    want("replace", "recieve", Some("receive"), "spelling"),
                    want("replace", "quick", Some("swift"), "tone"),
                ],
                None,
                false,
                None,
                false,
                &agent(),
            )
            .unwrap(),
    );
    press(&mut viewer, ":review accept spelling<CR>");
    press(&mut viewer, ":w<CR>");
    viewer.sync();
    drop(viewer);
    assert!(directory.join("state").join("review").is_dir());
    let (mut viewer, _) = open(&directory);
    viewer.review_ready();
    assert_eq!(viewer.suggestions().len(), 2);
    assert_eq!(state(&viewer, ids[0]), State::Accepted);
    assert_eq!(state(&viewer, ids[1]), State::Open);
    assert_eq!(viewer.proposals().len(), 1, "it waits again");
    let quick = &viewer.proposals()[0];
    assert_eq!(&viewer.text().unwrap()[quick.range.clone()], "quick");
    // The record, beside the document; not written over without `!`.
    press(&mut viewer, ":review record<CR>");
    let record = std::fs::read_to_string(directory.join("main.review.md")).unwrap();
    assert!(record.contains("# Review of main.typ"), "{record}");
    assert!(record.contains("accepted as offered"), "{record}");
    assert!(record.contains("`recieve` became `receive`"), "{record}");
    assert!(record.contains("by claude (session s1)"), "{record}");
    press(&mut viewer, ":review record<CR>");
    assert!(said(&viewer).contains("record!"), "{}", said(&viewer));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_manager_lists_walks_and_decides() {
    let (mut viewer, directory) = editing(TEXT);
    let ids = made(
        &viewer
            .agent_suggest(
                &[
                    want("replace", "recieve", Some("receive"), "spelling"),
                    want("replace", "carefuly", Some("carefully"), "spelling"),
                    want("replace", "quick", Some("swift"), "tone"),
                ],
                None,
                false,
                None,
                false,
                &agent(),
            )
            .unwrap(),
    );
    press(&mut viewer, "gsgs");
    compile(&mut viewer);
    press(&mut viewer, "gg");
    press(&mut viewer, "gR");
    assert!(viewer.is_reviewing());
    // Drawn, beside the page.
    let _ = viewer.scene(&|_| false);
    // Grouped by section: Intro first, the quick fox in it.
    press(&mut viewer, "jj");
    let cursor = viewer.editor.as_ref().unwrap().cursor();
    assert_eq!(cursor, TEXT.find("recieve").unwrap(), "the page follows");
    // `s` groups by category.
    press(&mut viewer, "s");
    let _ = viewer.scene(&|_| false);
    press(&mut viewer, "<Esc>");
    assert!(!viewer.is_reviewing());
    // Narrowed to a category: on its group's row, `a` decides all of it.
    press(&mut viewer, ":review only category spelling<CR>");
    assert!(viewer.is_reviewing());
    press(&mut viewer, "a");
    assert_eq!(state(&viewer, ids[0]), State::Accepted, "{}", said(&viewer));
    assert_eq!(state(&viewer, ids[1]), State::Accepted);
    assert_eq!(state(&viewer, ids[2]), State::Open);
    press(&mut viewer, "<Esc>");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn suggestions_are_marked_by_kind_and_older_proposals_become_suggestions() {
    let (mut viewer, directory) = editing(TEXT);
    viewer
        .agent_replace("lazy", "sleepy", false, None, true)
        .unwrap();
    viewer.review_ready();
    let [adopted] = viewer.suggestions() else {
        panic!("the proposal is a suggestion: {:?}", viewer.suggestions())
    };
    assert_eq!(
        (adopted.kind, adopted.by.as_str()),
        (Kind::Replace, "agent")
    );
    viewer
        .agent_suggest(
            &[Wanted {
                kind: "question".to_owned(),
                old_str: Some("analyse".to_owned()),
                reason: "Which method?".to_owned(),
                ..Wanted::default()
            }],
            None,
            false,
            None,
            false,
            &agent(),
        )
        .unwrap();
    press(&mut viewer, "gsgs");
    compile(&mut viewer);
    let remark = tints(&mut viewer, [0x45, 0x85, 0x88, 0x2a]);
    assert!(!remark.is_empty(), "the question is marked in its colour");
    // A question moves with the text typed before it.
    press(&mut viewer, "gg/recieve<CR>iVery <Esc>");
    let text = viewer.text().unwrap().to_owned();
    let question = viewer.suggestions().last().unwrap();
    assert_eq!(&text[question.range.clone()], "analyse");
    // Asked for a review of the section here; what comes back is of it.
    press(&mut viewer, "gQ");
    assert!(viewer.is_picking());
    press(&mut viewer, "<CR>");
    let [request] = viewer.review_requests() else {
        panic!("a request")
    };
    assert_eq!(
        (request.what.as_str(), request.section.as_str()),
        ("proofread", "Methods")
    );
    let id = request.id;
    let answer = viewer
        .agent_suggest(
            &[want("replace", "carefuly", Some("carefully"), "spelling")],
            Some(id),
            true,
            None,
            false,
            &agent(),
        )
        .unwrap();
    assert_eq!(answer["made"], 1);
    assert!(viewer.review_requests()[0].done);
    assert_eq!(viewer.suggestions().last().unwrap().request, Some(id));
    let _ = std::fs::remove_dir_all(directory);
}
