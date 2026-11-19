//! The agent on call: the writer's own agent command, started by the
//! window when something is asked of agents. Here it is a stand-in, a
//! line for the shell that says what it was given.

use std::time::{Duration, Instant};

use super::*;
use crate::viewer::on_call::Settings;

/// A viewer editing a short text, its state kept in a directory of its
/// own, with `command` as its agent on call.
fn on_call(command: &str) -> (Viewer, PathBuf) {
    let (mut viewer, directory) = editing("A first sentence.\n\nA second one.\n");
    viewer.state_dir = Some(directory.join("state"));
    viewer.set_agent_on_call(&Settings {
        command: Some(command.replace("DIR", &directory.display().to_string())),
        socket: Some(PathBuf::from("/run/zetwerk-test.sock")),
    });
    press(&mut viewer, "gsgs");
    (viewer, directory)
}

fn said(viewer: &Viewer) -> String {
    viewer
        .message
        .as_ref()
        .map(|message| message.text.clone())
        .unwrap_or_default()
}

/// Waits until the call under way is over, as the window does when it is
/// woken.
fn done(viewer: &mut Viewer) {
    let until = Instant::now() + Duration::from_secs(10);
    while !viewer.agent_progress() {
        assert!(Instant::now() < until, "the agent did not finish");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_question_calls_the_agent_with_its_task_and_this_window() {
    let (mut viewer, directory) = on_call("printf '%s\\n' \"$ZETWERK_SOCKET\" >> DIR/called; cat");
    press(&mut viewer, ":ask is this sentence right?<CR>");
    assert_eq!(viewer.agent_at_work(), Some("1 question"));
    assert!(
        said(&viewer).starts_with("the agent is called for 1 question"),
        "{}",
        said(&viewer)
    );
    done(&mut viewer);
    assert!(
        said(&viewer).starts_with("the agent is done with 1 question"),
        "{}",
        said(&viewer)
    );
    assert_eq!(viewer.agent_at_work(), None);
    let called = std::fs::read_to_string(directory.join("called")).unwrap();
    assert_eq!(called, "/run/zetwerk-test.sock\n");
    // The task came on its standard input: what it printed.
    let log = std::fs::read_to_string(directory.join("state/agent.log")).unwrap();
    assert!(
        log.contains("1 open question (list_notes gives them"),
        "{log}"
    );
    assert!(log.contains("Do not save"), "{log}");
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn what_is_asked_while_it_works_calls_it_once_more() {
    let (mut viewer, directory) = on_call("sleep 0.3; echo called >> DIR/called; true");
    press(&mut viewer, ":ask first?<CR>");
    press(&mut viewer, "j:ask second?<CR>");
    assert_eq!(
        viewer.agent_at_work(),
        Some("1 question"),
        "still the first call"
    );
    done(&mut viewer);
    // Both still wait (the stand-in answers nothing): called again.
    assert_eq!(viewer.agent_at_work(), Some("2 questions"));
    done(&mut viewer);
    assert_eq!(viewer.agent_at_work(), None);
    let called = std::fs::read_to_string(directory.join("called")).unwrap();
    assert_eq!(called.lines().count(), 2);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn the_agent_is_stopped_by_the_writer_and_says_why_it_failed() {
    let (mut viewer, directory) = on_call("sleep 30");
    press(&mut viewer, ":ask a question?<CR>");
    assert!(viewer.agent_at_work().is_some());
    press(&mut viewer, ":agent stop<CR>");
    assert_eq!(viewer.agent_at_work(), None);
    assert!(
        said(&viewer).starts_with("the agent was stopped"),
        "{}",
        said(&viewer)
    );
    assert!(!viewer.agent_progress());
    let _ = std::fs::remove_dir_all(directory);

    // One that fails says so, with what it wrote last.
    let (mut viewer, directory) = on_call("echo 'not logged in' >&2; exit 3");
    press(&mut viewer, ":ask a question?<CR>");
    done(&mut viewer);
    let message = viewer.message.as_ref().unwrap();
    assert!(message.alert);
    assert!(
        message.text.contains("it ended with 3; not logged in"),
        "{}",
        message.text
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn without_an_agent_nothing_is_called_and_agent_says_how_to_name_one() {
    let (mut viewer, directory) = editing("A sentence.\n");
    press(&mut viewer, "gsgs:ask is it right?<CR>");
    assert_eq!(viewer.agent_at_work(), None);
    assert!(
        said(&viewer).starts_with("asked: an agent sees the question"),
        "{}",
        said(&viewer)
    );
    press(&mut viewer, ":agent<CR>");
    assert!(said(&viewer).contains("[ai] agent ="), "{}", said(&viewer));
    let _ = std::fs::remove_dir_all(directory);
}
