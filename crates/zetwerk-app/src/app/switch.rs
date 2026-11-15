//! A window without a document, and going from one document to another
//! in the same window.
//!
//! Given a directory instead of a file, the window opens on no document:
//! the viewer then has a project that lives in memory, empty, rooted in
//! that directory, and puts up the list of the documents opened last
//! (see `viewer::start`). When the user chooses one, or makes a new one,
//! the viewer says so; the window then opens that document, and a new
//! viewer and a new compile thread take the place of the old ones. The
//! window, its surface and the raster threads stay: tiles are named by
//! what they show, so no tile of the old document is taken for one of
//! the new.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use winit::event_loop::EventLoopProxy;
use zetwerk_core::Opened;
use zetwerk_typst::{Compiler, Environment, Project, Urgency};

use super::{App, Event, Options, Session};
use crate::editor::Editor;
use crate::viewer::Viewer;
use crate::viewer::start::{self, Opening};

/// What a window is opened on.
pub(super) enum Document {
    /// A document, the file to edit in it, and what opening that found.
    Open(Box<(Project, Editor, Opened)>),
    /// No document yet: the directory new ones go into, as an empty
    /// project.
    Start(Project),
}

impl Document {
    /// Opens what `options` say: the document of the file, or, for a
    /// directory, nothing.
    pub(super) fn open(env: &Arc<Environment>, options: &Options) -> Result<Self, String> {
        if options.path.is_dir() {
            let directory = std::path::absolute(&options.path)
                .map_err(|err| format!("cannot open {}: {err}", options.path.display()))?;
            let project =
                Project::in_memory(Arc::clone(env), directory, "start.typ", String::new())
                    .ok_or("cannot set up a window without a document")?;
            return Ok(Self::Start(project));
        }
        let (project, editor, opened) = crate::headless::open(env, options)?;
        Ok(Self::Open(Box::new((project, editor, opened))))
    }

    /// The viewer of it, in a window of `window` device pixels at
    /// `scale` of them to a logical one.
    pub(super) fn viewer(
        self,
        env: Arc<Environment>,
        window: (u32, u32),
        scale: f64,
        options: &Options,
    ) -> Result<Viewer, String> {
        let viewer = match self {
            Self::Open(open) => {
                let (project, editor, opened) = *open;
                Viewer::new(env, project, window, scale)
                    .map(|viewer| viewer.editing(editor, &opened))
            }
            Self::Start(project) => Viewer::new(env, project, window, scale),
        };
        Ok(viewer
            .ok_or("cannot set up the interface")?
            .configured(options))
    }
}

/// What the window is called while it shows the document at `path`.
pub(super) fn title(path: &Path) -> String {
    match path.file_name() {
        Some(name) if !path.is_dir() => format!("{} — zetwerk", name.to_string_lossy()),
        _ => "zetwerk".to_owned(),
    }
}

/// The main file of the document the file at `path` is part of, if that
/// is another file.
fn main_of(path: &Path) -> Option<PathBuf> {
    let main = zetwerk_typst::find_main(path);
    let same = match (main.canonicalize(), path.canonicalize()) {
        (Ok(main), Ok(path)) => main == path,
        _ => main == path,
    };
    (!same).then_some(main)
}

/// What the window says or puts up first, now that `viewer` shows what
/// `options` opened; `new` if that is a document just made. A document
/// is noted among those opened last.
pub(super) fn welcome(viewer: &mut Viewer, options: &Options, new: bool) {
    let opening = if options.path.is_dir() {
        Opening::Start
    } else {
        // Not being able to note it takes nothing from the document.
        let _ = start::remember(&options.state_dir, &options.path);
        if new { Opening::New } else { Opening::Document }
    };
    viewer.welcome(opening, options.templates.clone());
}

/// What holds on for the document `to` that takes the place of `from`:
/// whether agents only propose changes (told so once, the user is not
/// to find an agent changing the next document at will), and revisions
/// that go on counting (an agent's change for the old text is refused).
fn carry_over(from: &Viewer, to: &mut Viewer) {
    to.take_over_from(from);
}

impl App {
    /// Opens the document the viewer asks for in place of the one it
    /// shows, if it asks for one. Where it cannot be opened, the viewer
    /// stays and says why.
    pub(super) fn switch_if_wanted(&mut self) {
        let Some(session) = &mut self.session else {
            return;
        };
        let Some((path, new)) = session.viewer.take_wanted_document() else {
            return;
        };
        let main = main_of(&path);
        let path_before = std::mem::replace(&mut self.options.path, path.clone());
        let main_before = std::mem::replace(&mut self.options.main, main);
        if let Err(why) = session.go_to(&self.options, &self.proxy, new) {
            self.options.path = path_before;
            self.options.main = main_before;
            session.viewer.not_opened(&path, &why);
            session.window.request_redraw();
        }
    }
}

impl Session {
    /// Shows the document `options` name in place of the one shown.
    fn go_to(
        &mut self,
        options: &Options,
        proxy: &EventLoopProxy<Event>,
        new: bool,
    ) -> Result<(), String> {
        let wake = || {
            let proxy = proxy.clone();
            move || {
                let _ = proxy.send_event(Event::Wake);
            }
        };
        let document = Document::open(&self.env, options)?;
        let size = self.window.inner_size();
        let mut viewer = document.viewer(
            Arc::clone(&self.env),
            (size.width, size.height),
            self.window.scale_factor(),
            options,
        )?;
        carry_over(&self.viewer, &mut viewer);
        // What compiles the old document may still be at it: its pages
        // are not to be taken for the new one's.
        let compiler = Compiler::spawn(options.power.budget().compile_threads, wake())
            .map_err(|err| format!("cannot start the compile thread: {err}"))?;
        let old = std::mem::replace(&mut self.compiler, compiler);
        // It finishes what it is doing before it ends; nobody waits.
        std::thread::spawn(move || drop(old));
        viewer.wake_for_language(wake());
        self.viewer.sync();
        self.viewer = viewer;
        self.viewer.export_in_background(wake());
        self.window.set_title(&title(&options.path));
        self.watch_dependencies();
        welcome(&mut self.viewer, options, new);
        self.scheduler.request(Urgency::Now, Instant::now());
        self.window.request_redraw();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_that_only_propose_still_do_in_the_next_document() {
        use crate::viewer::agent::AgentEdits;
        use zetwerk_typst::FontSet;
        let viewer = || {
            let env = Environment::new(FontSet::Embedded);
            let project = Project::in_memory(
                Arc::clone(&env),
                std::env::temp_dir(),
                "a.typ",
                String::new(),
            )
            .unwrap_or_else(|| panic!("a project"));
            Viewer::new(env, project, (800, 600), 1.0).unwrap_or_else(|| panic!("a viewer"))
        };
        let mut before = viewer();
        before.set_agent_edits(AgentEdits::Propose);
        let mut next = viewer();
        assert_eq!(next.agent_edits(), AgentEdits::Live);
        carry_over(&before, &mut next);
        assert_eq!(next.agent_edits(), AgentEdits::Propose);
    }

    #[test]
    fn a_window_is_called_after_its_document() {
        assert_eq!(title(Path::new("/tmp/paper.typ")), "paper.typ — zetwerk");
        assert_eq!(title(&std::env::temp_dir()), "zetwerk");
    }
}
