//! The document without a window: for an agent that starts zetwerk itself.
//!
//! The same viewer as the window's does the work, on a thread of its own
//! that takes the same requests. Nothing is drawn, and the text is
//! compiled when someone asks for something that needs it compiled, not
//! when typing pauses: there is nobody typing.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::Instant;

use zetwerk_typst::{CompileOutput, Environment, Project};

use crate::app::{Handle, Options, Request};
use crate::editor::Editor;
use crate::viewer::Viewer;

/// The size the viewer is told its window has. Nothing depends on it but
/// how many lines a page of scrolling is.
const WINDOW: (u32, u32) = (1200, 900);

/// Why the document could not be opened.
#[derive(Debug)]
pub enum OpenError {
    /// It is open in a window (or another agent has it): talk to that.
    AlreadyOpen,
    Other(String),
}

/// Opens the document of `options` and serves requests for it on a thread
/// of its own, until the handle and all its copies are dropped or one of
/// them sends [`Request::Quit`].
pub fn spawn(options: &Options) -> Result<(Handle, JoinHandle<()>), OpenError> {
    let other = |message: String| OpenError::Other(message);
    let env = Environment::with_packages(options.font_set, options.packages.dirs.clone());
    let (project, editor, opened) = open_document(&env, options)?;
    let viewer = Viewer::new(env, project, WINDOW, 1.0)
        .ok_or_else(|| other("no usable fonts were found".to_owned()))?
        .editing(editor, &opened)
        .configured(options);
    let (send, requests) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("zetwerk-document".into())
        .spawn(move || run(viewer, &requests))
        .map_err(|err| other(format!("cannot start a thread: {err}")))?;
    Ok((Handle::to_thread(send), thread))
}

/// Opens the document of `options`: its main file as a project, and the
/// file to edit in it.
pub(crate) fn open(
    env: &Arc<Environment>,
    options: &Options,
) -> Result<(Project, Editor, zetwerk_core::Opened), String> {
    open_document(env, options).map_err(|err| match err {
        OpenError::AlreadyOpen => {
            // The window that has it says so, and asks for attention.
            let asked = match zetwerk_core::knock(&options.path, &options.state_dir) {
                Ok(()) => "; that window was asked to come forward",
                Err(_) => "",
            };
            format!(
                "{} is open in another zetwerk window already{asked}: go on there, or close \
                 it there to open it here",
                options.path.display()
            )
        }
        OpenError::Other(why) => why,
    })
}

fn open_document(
    env: &Arc<Environment>,
    options: &Options,
) -> Result<(Project, Editor, zetwerk_core::Opened), OpenError> {
    let other = |message: String| OpenError::Other(message);
    let main = options.main.as_deref().unwrap_or(&options.path);
    let project = Project::open(Arc::clone(env), main).map_err(|err| other(err.to_string()))?;
    let file = project.file_id(&options.path).ok_or_else(|| {
        other(format!(
            "{} is not in the directory of its main file, {}",
            options.path.display(),
            main.display()
        ))
    })?;
    let (editor, opened) =
        Editor::open(&options.path, &options.state_dir, file).map_err(|err| match err {
            zetwerk_core::OpenError::AlreadyOpen(_) => OpenError::AlreadyOpen,
            err => other(err.to_string()),
        })?;
    Ok((project, editor, opened))
}

fn run(mut viewer: Viewer, requests: &Receiver<Request>) {
    let path = viewer.main_path();
    for request in requests {
        // Nobody watches the file here: look before anything is done.
        if let Some(path) = &path {
            viewer.file_changed(path);
        }
        match request {
            Request::Quit => break,
            Request::Keys(keys) => {
                for key in keys {
                    viewer.key(key);
                }
            }
            Request::State(reply) => {
                let _ = reply.send(viewer.state());
            }
            Request::Do(task) => task(&mut viewer),
            Request::WhenCompiled(task) => {
                compile(&mut viewer);
                task(&mut viewer);
            }
            Request::WhenSettled(reply) => {
                compile(&mut viewer);
                let _ = reply.send(());
            }
            Request::Document(reply) => {
                compile(&mut viewer);
                let _ = reply.send(viewer.document().cloned());
            }
            Request::Screenshot(reply) => {
                let _ = reply.send(Err("there is no window to capture".to_owned()));
            }
        }
        // What was changed is safe before the agent hears that it was.
        viewer.take_edited();
        viewer.sync();
        if viewer.wants_quit() {
            break;
        }
    }
    viewer.sync();
}

/// Compiles the text as it is, unless the pages are those of it already.
fn compile(viewer: &mut Viewer) {
    if viewer.is_compiled() {
        return;
    }
    viewer.report_began();
    let (revision, world) = viewer.snapshot();
    let started = Instant::now();
    let compiled = zetwerk_typst::compile(&world);
    viewer.compiled(CompileOutput {
        revision,
        world: Arc::new(world),
        compiled,
        duration: started.elapsed(),
    });
}
