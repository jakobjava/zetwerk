//! What agents are doing, as the MCP server tells it: the status line,
//! the list of agents (`:agents`), which follows them, and the pause.

use std::sync::atomic::Ordering;

use super::*;
use crate::viewer::activity::{AGENTS_PAUSED, Event};

fn session() -> String {
    "s-1".to_owned()
}

fn listed(viewer: &Viewer) -> Vec<String> {
    let (_, items, _) = viewer.picker.as_ref().unwrap().listing();
    items.into_iter().map(str::to_owned).collect()
}

#[test]
fn what_an_agent_does_is_in_the_status_line_and_the_list_as_it_does_it() {
    let (mut viewer, directory) = editing("A sentence.\n");
    viewer.agent_activity(Event::Connected {
        session: session(),
        name: "claude-code".to_owned(),
        version: "2.1.3".to_owned(),
    });
    assert_eq!(viewer.agents_status(), "");
    viewer.agent_activity(Event::Began {
        session: session(),
        tool: "read_source".to_owned(),
        about: "from_line 1, to_line 80".to_owned(),
    });
    assert_eq!(viewer.agents_status(), "agent: read the source  ");
    // The list, opened while it works, follows it.
    press(&mut viewer, ":agents<CR>");
    let items = listed(&viewer);
    assert!(
        items[0].starts_with("● claude-code 2.1.3\tat work"),
        "{items:?}"
    );
    assert!(
        items
            .iter()
            .any(|item| item.contains("now: read the source  from_line 1, to_line 80")),
        "{items:?}"
    );
    viewer.agent_activity(Event::Ended {
        session: session(),
        tool: "read_source".to_owned(),
        millis: 12,
        error: None,
    });
    viewer.agent_activity(Event::Said {
        session: session(),
        message: "checking equation 3".to_owned(),
        done: Some(1),
        total: Some(4),
    });
    assert_eq!(viewer.agents_status(), "agent: checking equation 3 (1/4)  ");
    viewer.agent_activity(Event::Began {
        session: session(),
        tool: "edit_source".to_owned(),
        about: String::new(),
    });
    viewer.agent_activity(Event::Ended {
        session: session(),
        tool: "edit_source".to_owned(),
        millis: 3,
        error: Some("the text changed since it was read".to_owned()),
    });
    let items = listed(&viewer);
    assert!(items[0].contains("2 calls, 1 failed"), "{items:?}");
    assert!(
        items
            .iter()
            .any(|item| item.contains("“checking equation 3” (1/4)")),
        "{items:?}"
    );
    let failed = items
        .iter()
        .position(|item| item.contains("✗") && item.contains("changed the text"));
    let read = items
        .iter()
        .position(|item| item.contains("read the source") && item.ends_with("12 ms"));
    assert!(
        failed.is_some() && read.is_some() && failed < read,
        "newest first: {items:?}"
    );
    // Waiting for the writer, and gone.
    viewer.agent_activity(Event::Began {
        session: session(),
        tool: "wait_for_review".to_owned(),
        about: String::new(),
    });
    assert_eq!(viewer.agents_status(), "agent: waiting for you  ");
    viewer.agent_activity(Event::Left { session: session() });
    assert_eq!(viewer.agents_status(), "");
    assert!(
        listed(&viewer)[0].starts_with("○ claude-code 2.1.3\tgone"),
        "{:?}",
        listed(&viewer)
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn agents_are_paused_and_let_go_on() {
    let (mut viewer, directory) = editing("A sentence.\n");
    press(&mut viewer, ":agents pause<CR>");
    assert!(AGENTS_PAUSED.load(Ordering::Relaxed));
    assert_eq!(viewer.agents_status(), "agents paused  ");
    press(&mut viewer, ":agents<CR>");
    assert!(
        listed(&viewer)
            .iter()
            .any(|item| item.starts_with("let agents go on"))
    );
    press(&mut viewer, "<Esc>:agents resume<CR>");
    assert!(!AGENTS_PAUSED.load(Ordering::Relaxed));
    assert_eq!(viewer.agents_status(), "");
    // The option's own words still work.
    press(&mut viewer, ":agents propose<CR>");
    assert_eq!(
        viewer.agent_edits(),
        crate::viewer::agent::AgentEdits::Propose
    );
    let _ = std::fs::remove_dir_all(directory);
}
