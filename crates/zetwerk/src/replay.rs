//! `zetwerk --replay <report directory>`: a report played again in a
//! window.
//!
//! The files of the report's `before/` are put in a scratch directory
//! (under the temporary directory, with a state directory of its own
//! there: never the user's files, journals or word list), the window is
//! opened on them with the settings the report says were in force, the
//! stage is set as it was at the first key, and the keys are pressed with
//! the pauses they were typed with: each at most [`MAX_PAUSE`], and all
//! of them in [`MAX_TOTAL`] at most, shortened alike where they would
//! take longer. With `--paced` each key also waits for what the one
//! before led to. In the end the text is compared with the report's
//! `after/`, and whether it came out the same is said (and is the exit
//! status). With `--screenshot <path>` the window is pictured then and
//! closed; without, it stays open.
//!
//! Compiles land when they land in this window, not when they did; the
//! window is as large as it is here (the report says how large it was).

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use zetwerk_app::app::{self, Handle, Request, Task};
use zetwerk_app::viewer::Viewer;
use zetwerk_app::viewer::report::replay::{self as recorded, Recorded, Step};
use zetwerk_app::viewer::report::{DEFAULT_KEYS, Settings};

use crate::{Args, bridge, config};

/// The longest a key waits for the one before it.
const MAX_PAUSE: Duration = Duration::from_secs(2);
/// The longest all the pauses together take.
const MAX_TOTAL: Duration = Duration::from_secs(60);
/// How long the window may take to settle.
const PATIENCE: Duration = Duration::from_secs(60);

/// What a report says of the program: how many keys to keep, the
/// settings file, the version and the commit.
pub fn report_settings(config: &config::Config) -> Settings {
    Settings {
        keys: config.report.keys.unwrap_or(DEFAULT_KEYS),
        config: config::text(),
        built: built(),
    }
}

/// zetwerk's version and commit.
fn built() -> String {
    format!(
        "zetwerk {} (commit {})",
        env!("CARGO_PKG_VERSION"),
        env!("ZETWERK_COMMIT")
    )
}

/// Plays the report in `report` again in a window.
pub fn run(args: &Args, report: &Path) -> Result<(), String> {
    let recorded = Recorded::read(report)?;
    let scratch = std::env::temp_dir().join(format!("zetwerk-replay-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let document = scratch.join("document");
    recorded.copy_to(&document)?;
    let (config, complaint) = match recorded.config.as_deref().map(config::parse) {
        Some(Ok(config)) => (config, None),
        Some(Err(why)) => (
            config::Config::default(),
            Some(format!("the report's settings are ignored: {why}")),
        ),
        None => (config::Config::default(), None),
    };
    let stage = &recorded.stage;
    let first = stage.open.first().unwrap_or(&stage.file);
    let path = document.join(first);
    let main = document.join(&recorded.main);
    let (keys, odd_key) = config.keys.keymap();
    let mut language = config.language.settings();
    language.words = Some(scratch.join("words.txt"));
    let options = app::Options {
        main: (main != path).then_some(main),
        path,
        font_set: recorded.font_set,
        power: args.power.unwrap_or_default(),
        state_dir: scratch.join("state"),
        agent_edits: config.ai.edit_mode.into(),
        complaint: complaint.or(odd_key),
        keys,
        layout: config.view.layout.map(Into::into),
        slides: config.view.slides,
        ui_scale: config.view.ui_scale,
        inserts: config.inserts(),
        language,
        templates: None,
        review: config.review.settings(),
        talk: config.present.talk(),
        authors: config.authors.settings(),
        report: Settings {
            keys: DEFAULT_KEYS,
            config: recorded.config.clone(),
            built: built(),
        },
    };
    println!(
        "replaying {} in a copy of its document, {}",
        report.display(),
        document.display()
    );
    for inexact in &recorded.inexact {
        println!("not as it was: {inexact}");
    }
    let (result, done) = mpsc::channel();
    let screenshot = args.screenshot.clone();
    let paced = args.paced;
    app::run(options, |handle| {
        std::thread::spawn(move || {
            let outcome = drive(&handle, &recorded, &document, screenshot.as_deref(), paced);
            match &outcome {
                Ok(said) | Err(said) => println!("{said}"),
            }
            let _ = result.send(outcome);
            if screenshot.is_some() {
                handle.send(Request::Quit);
            }
        });
    })?;
    match done.try_recv() {
        Ok(Ok(_)) | Err(_) => Ok(()),
        Ok(Err(why)) => Err(why),
    }
}

/// Sets the stage, presses the keys, and says whether the text came out
/// as it did.
fn drive(
    handle: &Handle,
    recorded: &Recorded,
    document: &Path,
    screenshot: Option<&Path>,
    paced: bool,
) -> Result<String, String> {
    let gone = || "the window closed before the replay was over".to_owned();
    let settled = || -> Result<(), String> {
        let (reply, answer) = mpsc::channel();
        handle
            .send(Request::WhenSettled(reply))
            .then_some(())
            .ok_or_else(gone)?;
        answer
            .recv_timeout(PATIENCE)
            .map_err(|_| format!("the window did not settle within {PATIENCE:?}"))
    };
    let task = |task: Task| {
        handle
            .send(Request::Do(task))
            .then_some(())
            .ok_or_else(gone)
    };
    settled()?;
    let stage = recorded.stage.clone();
    task(Box::new(move |viewer: &mut Viewer| {
        viewer.report_stage(&stage);
    }))?;
    settled()?;
    let pauses: Vec<Duration> = recorded
        .steps
        .iter()
        .filter_map(|step| match step {
            Step::Key { pause, .. } => Some((*pause).min(MAX_PAUSE)),
            _ => None,
        })
        .collect();
    let total: Duration = pauses.iter().sum();
    let shorten = if total > MAX_TOTAL {
        MAX_TOTAL.as_secs_f64() / total.as_secs_f64()
    } else {
        1.0
    };
    let mut pauses = pauses.into_iter();
    for step in &recorded.steps {
        match step {
            Step::Key { press, .. } => {
                let pause = pauses.next().unwrap_or_default();
                std::thread::sleep(pause.mul_f64(shorten));
                handle
                    .send(Request::Keys(vec![*press]))
                    .then_some(())
                    .ok_or_else(gone)?;
                if paced {
                    settled()?;
                }
            }
            Step::TextSet { files, cursor } => {
                let (files, cursor) = (files.clone(), *cursor);
                task(Box::new(move |viewer: &mut Viewer| {
                    viewer.report_set_texts(&files, cursor);
                }))?;
            }
            Step::Cursor(cursor) => {
                let cursor = *cursor;
                task(Box::new(move |viewer: &mut Viewer| {
                    viewer.report_set_texts(&[], Some(cursor));
                }))?;
            }
            Step::Opened(name, text) => recorded::write_inside(document, name, text.as_bytes())?,
            // The window compiles when it compiles.
            Step::Began { .. } | Step::Compiled { .. } => {}
        }
    }
    settled()?;
    let (reply, answer) = mpsc::channel();
    task(Box::new(move |viewer: &mut Viewer| {
        let _ = reply.send(viewer.report_texts());
    }))?;
    let texts = answer.recv_timeout(PATIENCE).map_err(|_| gone())?;
    if let Some(path) = screenshot {
        picture(handle, path)?;
    }
    let differences = recorded::differences(&texts, &recorded.after);
    if differences.is_empty() {
        Ok("reproduced: the text came out as it was when the report was written".to_owned())
    } else {
        Err(format!("not reproduced:\n{}", differences.join("\n")))
    }
}

/// Saves the window as it is to `path`.
fn picture(handle: &Handle, path: &Path) -> Result<(), String> {
    let (reply, shot) = mpsc::channel();
    handle
        .send(Request::Screenshot(reply))
        .then_some(())
        .ok_or("the window closed before it was pictured")?;
    let shot = shot
        .recv_timeout(PATIENCE)
        .map_err(|_| "the window gave no picture".to_owned())??;
    let png = bridge::encode(shot)?;
    std::fs::write(path, png).map_err(|err| format!("cannot write {}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_says_how_many_keys_and_how_the_program_was_built() {
        // (Not `report_settings`: it reads the user's settings file.)
        let keys = |text: &str| config::parse(text).map(|config| config.report.keys);
        assert_eq!(keys("[report]\nkeys = 0\n"), Ok(Some(0)));
        assert_eq!(keys(""), Ok(None));
        assert!(keys("[report]\nkeep = 3\n").is_err());
        assert!(built().contains("(commit "));
    }
}
