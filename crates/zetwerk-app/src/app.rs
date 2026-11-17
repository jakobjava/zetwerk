//! The window and its event loop.
//!
//! This is the only place that touches the windowing system, the GPU and
//! the background threads. It turns events into calls on [`Viewer`] and puts
//! the viewer's scenes on screen. The loop sleeps unless something happened:
//! there is no timer and no animation, so an idle window costs nothing.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use typst_layout::PagedDocument;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key as WinitKey, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};
use zetwerk_render::Rasterizer;
use zetwerk_typst::{Compiler, Environment, FontSet, Scheduler, Urgency};

use crate::gpu::{self, Gpu, Quad};
use crate::keys::{Key, KeyPress};
use crate::power::PowerMode;
use crate::viewer::agent::AgentEdits;
use crate::viewer::{self, Edited, State, Viewer};
use crate::watch::FileWatcher;

mod present;
mod refusal;
mod switch;

/// How the window should be opened.
pub struct Options {
    /// The file to edit; or a directory, where there is none yet: the
    /// window then lists the documents opened last, and new ones are
    /// made there (see `switch`).
    pub path: PathBuf,
    /// The main file of the document it is part of, if it is not that
    /// itself.
    pub main: Option<PathBuf>,
    pub font_set: FontSet,
    pub power: PowerMode,
    /// Where the journals that make edits survive a crash are kept.
    pub state_dir: PathBuf,
    /// Whether agents change the text or propose changes to it.
    pub agent_edits: AgentEdits,
    /// Something to tell the user once the document is open, such as
    /// that the settings could not be read.
    pub complaint: Option<String>,
    /// The user's own keys.
    pub keys: viewer::Keymap,
    /// What the window shows to begin with, if not the usual, and whether
    /// pages are shown as slides.
    pub layout: Option<viewer::Panes>,
    pub slides: bool,
    /// How many times its usual size everything that is not a page is
    /// to be, if not once.
    pub ui_scale: Option<f64>,
    /// What the user wants in the menu of things to put in.
    pub inserts: Vec<crate::typst_text::Insert>,
    /// What the user has to say about spelling and grammar.
    pub language: zetwerk_language::Settings,
    /// Where the user's own starters for new documents are.
    pub templates: Option<PathBuf>,
    /// What the user says of reviewing with agents.
    pub review: viewer::review::Settings,
    /// What the user has to say about giving talks.
    pub talk: viewer::Talk,
    /// What is kept for a report of what happened (see `viewer::report`).
    pub report: viewer::report::Settings,
    /// What the user has to say about showing who wrote what.
    pub authors: viewer::authors::Settings,
    /// What the settings file says of the options, and how to keep
    /// what is changed (see [`crate::settings`]).
    pub settings: crate::settings::Stored,
}

/// How long a change may wait before the journal is forced onto the disk.
/// A crash of zetwerk loses nothing either way; this bounds what a power
/// failure can take.
const SYNC_AFTER: Duration = Duration::from_secs(1);

/// Something asked of a running window from another thread.
pub enum Request {
    /// Press these keys, as if typed.
    Keys(Vec<KeyPress>),
    /// Report the viewer's state.
    State(Sender<State>),
    /// Reply once the current revision is compiled and everything visible
    /// is rasterized and on screen.
    WhenSettled(Sender<()>),
    /// Reply with the window's contents once settled.
    Screenshot(Sender<Result<Screenshot, String>>),
    /// Reply with the document once the current revision is compiled, or
    /// `None` if it has never compiled.
    Document(Sender<Option<Arc<PagedDocument>>>),
    /// Do this with the viewer, now. What it changes is compiled and
    /// drawn as what keys change is.
    Do(Task),
    /// Do this with the viewer once the current revision is compiled.
    WhenCompiled(Task),
    Quit,
}

/// Something to do with the viewer on the window's thread.
pub type Task = Box<dyn FnOnce(&mut Viewer) + Send>;

/// The pixels of a window.
pub struct Screenshot {
    pub width: u32,
    pub height: u32,
    /// Rows of RGBA, top to bottom.
    pub rgba: Vec<u8>,
}

/// Sends [`Request`]s to a running window, or to a document that runs
/// without one.
#[derive(Clone)]
pub struct Handle {
    to: Receiving,
}

#[derive(Clone)]
enum Receiving {
    Window(EventLoopProxy<Event>),
    Thread(std::sync::mpsc::Sender<Request>),
}

impl Handle {
    /// A handle to a thread that takes requests from a channel.
    pub(crate) fn to_thread(requests: std::sync::mpsc::Sender<Request>) -> Self {
        Self {
            to: Receiving::Thread(requests),
        }
    }

    /// Sends `request`. Returns `false` if the window is gone.
    pub fn send(&self, request: Request) -> bool {
        match &self.to {
            Receiving::Window(proxy) => proxy.send_event(Event::Request(request)).is_ok(),
            Receiving::Thread(requests) => requests.send(request).is_ok(),
        }
    }
}

enum Event {
    /// A background thread has results to collect.
    Wake,
    FilesChanged(Vec<PathBuf>),
    Request(Request),
}

/// Opens the window and runs until it is closed. `ready` is called once
/// with a handle for talking to the window from other threads.
pub fn run(options: Options, ready: impl FnOnce(Handle)) -> Result<(), String> {
    // Typst parallelises through rayon's global pool, which by default
    // starts a thread per core the first time it is used. Document compiles
    // run in a pool of their own (see `Compiler`); what is left for the
    // global one is typesetting the chrome, which needs no more than this.
    // Failing means a pool already exists, which is fine too.
    let _ = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .thread_name(|_| "zetwerk-chrome".to_owned())
        .build_global();

    let event_loop = EventLoop::<Event>::with_user_event()
        .build()
        .map_err(|err| format!("cannot start the event loop: {err}"))?;
    event_loop.set_control_flow(ControlFlow::Wait);
    ready(Handle {
        to: Receiving::Window(event_loop.create_proxy()),
    });
    let mut app = App {
        options,
        proxy: event_loop.create_proxy(),
        session: None,
        error: None,
        refusal: None,
    };
    event_loop
        .run_app(&mut app)
        .map_err(|err| format!("the event loop failed: {err}"))?;
    app.error.map_or(Ok(()), Err)
}

struct App {
    options: Options,
    proxy: EventLoopProxy<Event>,
    /// Everything that exists while the window does.
    session: Option<Session>,
    error: Option<String>,
    /// The window that says why there is no session, where nobody would
    /// read it on standard error.
    refusal: Option<refusal::Refusal>,
}

struct Session {
    window: Arc<Window>,
    /// The fonts and the library, for documents opened later.
    env: Arc<Environment>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    /// What surfaces are made with, for a window of the audience's view
    /// (see `present`), and that window while there is one.
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    second: Option<present::Second>,
    gpu: Gpu,
    viewer: Viewer,
    compiler: Compiler,
    rasterizer: Rasterizer,
    scheduler: Scheduler,
    watcher: Option<FileWatcher>,
    modifiers: ModifiersState,
    cursor: (f64, f64),
    /// When the input that the next frame responds to arrived.
    input_at: Option<Instant>,
    /// How long input has to pause before the document is compiled.
    debounce: Duration,
    /// When the text was last changed by a command.
    last_edit: Option<Instant>,
    /// Requests waiting for the window to settle.
    waiting: Vec<Request>,
}

impl ApplicationHandler<Event> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.session.is_some() {
            return;
        }
        match Session::open(&self.options, &self.proxy, event_loop) {
            Ok(session) => self.session = Some(session),
            Err(err) => {
                // Started without a terminal, standard error is read by
                // nobody: a window says it.
                if !std::io::IsTerminal::is_terminal(&std::io::stderr()) {
                    self.refusal = refusal::Refusal::show(event_loop, &err);
                }
                self.error = Some(err);
                if self.refusal.is_none() {
                    event_loop.exit();
                }
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if let Some(refusal) = &mut self.refusal {
            if refusal.event(&event) {
                event_loop.exit();
            }
            return;
        }
        let Some(session) = &mut self.session else {
            return;
        };
        if present::second_event(session, id, &event) {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            // Away from the window is a good moment to make sure. A
            // button let go there may never be heard of.
            WindowEvent::Focused(false) => {
                session.viewer.sync();
                session.viewer.pointer_released();
            }
            WindowEvent::RedrawRequested => session.redraw(),
            other => session.window_event(other),
        }
        if session.viewer.wants_quit() {
            event_loop.exit();
        }
        self.switch_if_wanted();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Event) {
        let Some(session) = &mut self.session else {
            return;
        };
        match event {
            Event::Wake if session.viewer.paste_progress() => session.after_keys(),
            Event::Wake => {}
            Event::FilesChanged(paths) => session.files_changed(&paths),
            Event::Request(Request::Quit) => event_loop.exit(),
            Event::Request(request) => session.request(request),
        }
        session.window.request_redraw();
        if session.viewer.wants_quit() {
            event_loop.exit();
        }
        self.switch_if_wanted();
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(refusal) = &self.refusal {
            if refusal.until() <= Instant::now() {
                event_loop.exit();
            } else {
                event_loop.set_control_flow(ControlFlow::WaitUntil(refusal.until()));
            }
            return;
        }
        let Some(session) = &mut self.session else {
            return;
        };
        present::follow(session, event_loop);
        let now = Instant::now();
        // A pointer held beyond the text while it selects moves the view
        // along, and keys that wait for another are explained once they
        // pause; nothing else here goes by the clock.
        if session.viewer.autoscroll(now) {
            session.window.request_redraw();
        }
        // The tint on what a key yanked or put goes when its time is up.
        if session.viewer.flash_over(now) {
            session.window.request_redraw();
        }
        // And the keys that can follow come up when typing has paused.
        if session.viewer.hints_due(now) {
            session.window.request_redraw();
        }
        // And the clock of a talk, where it is shown, when it shows
        // another minute.
        if session.viewer.present_due(now) {
            session.window.request_redraw();
        }
        // Keys that may begin a mapping are taken as they are when the
        // next is too long in coming.
        if session.viewer.keys_due(now) {
            session.after_keys();
            session.window.request_redraw();
        }
        if session.scheduler.take_due(now) {
            session.viewer.report_began();
            let (revision, world) = session.viewer.snapshot();
            session.compiler.compile(revision, world);
        }
        // A compile that newer text has waited behind long enough is
        // given up on, and the status line says what runs long.
        if session.compiler.overtake(now) {
            session.window.request_redraw();
        }
        let (running, held) = (session.compiler.running(), session.compiler.held_since());
        if session.viewer.compiling(running, held) {
            session.window.request_redraw();
        }
        let mut sync_at = session
            .viewer
            .unsynced_since()
            .map(|since| since + SYNC_AFTER);
        if sync_at.is_some_and(|at| at <= now) {
            session.viewer.sync();
            sync_at = None;
        }
        let deadline = [
            session.scheduler.deadline(),
            sync_at,
            session.viewer.autoscroll_at(),
            session.viewer.flash_at(),
            session.viewer.hints_at(),
            session.compiler.overtake_at(),
            session.viewer.compiling_at(now),
            session.viewer.keys_at(),
            session.viewer.present_at(),
        ]
        .into_iter()
        .flatten()
        .min();
        event_loop.set_control_flow(match deadline {
            Some(deadline) => ControlFlow::WaitUntil(deadline),
            None => ControlFlow::Wait,
        });
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        crate::clipboard::keep_after_exit();
        if let Some(session) = &mut self.session {
            session.viewer.sync();
        }
    }
}

impl Session {
    fn open(
        options: &Options,
        proxy: &EventLoopProxy<Event>,
        event_loop: &ActiveEventLoop,
    ) -> Result<Self, String> {
        let env = Environment::new(options.font_set);
        let document = switch::Document::open(&env, options)?;
        let attributes = Window::default_attributes()
            .with_title(switch::title(&options.path))
            .with_inner_size(LogicalSize::new(900.0, 1000.0));
        // The name a desktop knows the window by: its entry in the list
        // of applications and its icon go by it.
        #[cfg(all(unix, not(target_os = "macos")))]
        let attributes = {
            use winit::platform::wayland::WindowAttributesExtWayland;
            attributes.with_name("zetwerk", "zetwerk")
        };
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .map_err(|err| format!("cannot open a window: {err}"))?,
        );

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
            Box::new(event_loop.owned_display_handle()),
        ));
        let surface = instance
            .create_surface(Arc::clone(&window))
            .map_err(|err| format!("cannot draw into the window: {err}"))?;
        let adapter = gpu::adapter(&instance, Some(&surface)).map_err(|err| err.to_string())?;

        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("the graphics adapter cannot draw into this window")?;
        // Images are passed through as the bytes they are (see `Gpu::new`),
        // so the frame is written through a non-sRGB view of the surface.
        let target_format = config.format.remove_srgb_suffix();
        if target_format != config.format {
            config.view_formats = vec![target_format];
        }
        // Frames are only drawn when something changed, so not waiting for
        // the display's refresh costs nothing and shows input sooner.
        let capabilities = surface.get_capabilities(&adapter);
        if capabilities
            .present_modes
            .contains(&wgpu::PresentMode::Mailbox)
        {
            config.present_mode = wgpu::PresentMode::Mailbox;
        }
        let gpu = Gpu::new(&adapter, target_format).map_err(|err| err.to_string())?;
        surface.configure(gpu.device(), &config);

        let budget = options.power.budget();
        let wake = |proxy: &EventLoopProxy<Event>| {
            let proxy = proxy.clone();
            move || {
                let _ = proxy.send_event(Event::Wake);
            }
        };
        let compiler = Compiler::spawn(budget.compile_threads, wake(proxy))
            .map_err(|err| format!("cannot start the compile thread: {err}"))?;
        let rasterizer = Rasterizer::spawn(budget.raster_threads, wake(proxy))
            .map_err(|err| format!("cannot start the raster threads: {err}"))?;
        let files_changed = proxy.clone();
        // Without a watcher the document still shows; it just does not
        // follow changes on disk.
        let watcher = FileWatcher::new(move |paths| {
            let _ = files_changed.send_event(Event::FilesChanged(paths));
        })
        .ok();

        let viewer = document.viewer(
            Arc::clone(&env),
            (size.width, size.height),
            window.scale_factor(),
            options,
        )?;
        let mut viewer = viewer;
        viewer.wake_for_language(wake(proxy));
        viewer.wake_for_review(wake(proxy));
        viewer.wake_for_handouts(wake(proxy));
        let mut session = Self {
            window,
            env,
            surface,
            config,
            instance,
            adapter,
            second: None,
            gpu,
            viewer,
            compiler,
            rasterizer,
            scheduler: Scheduler::new(budget.debounce),
            watcher,
            modifiers: ModifiersState::empty(),
            cursor: (0.0, 0.0),
            input_at: None,
            debounce: budget.debounce,
            last_edit: None,
            waiting: Vec::new(),
        };
        session.viewer.export_in_background(wake(proxy));
        session.viewer.report_in_background(wake(proxy));
        session.viewer.paste_in_background(wake(proxy));
        session.watch_dependencies();
        switch::welcome(&mut session.viewer, options, false);
        session.scheduler.request(Urgency::Now, Instant::now());
        Ok(session)
    }

    fn window_event(&mut self, event: WindowEvent) {
        if present::touched(&mut self.viewer, &event) {
            return self.window.request_redraw();
        }
        match event {
            WindowEvent::Resized(size) => {
                self.config.width = size.width.max(1);
                self.config.height = size.height.max(1);
                self.surface.configure(self.gpu.device(), &self.config);
                self.viewer
                    .resize((size.width, size.height), self.window.scale_factor());
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let size = self.window.inner_size();
                self.viewer.resize((size.width, size.height), scale_factor);
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                return;
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                // Nothing to do, and nothing to draw, unless a button is
                // down.
                if !self.viewer.pointer_moved(self.cursor) {
                    return;
                }
                self.after_keys();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let ctrl = self.modifiers.control_key();
                let Some(key) = translate(&event.logical_key, event.text.as_deref(), ctrl) else {
                    return;
                };
                let key = shifted(key, self.modifiers.shift_key());
                let alt = alt_held(
                    event.text.as_deref(),
                    ctrl,
                    self.modifiers.alt_key(),
                    cfg!(target_os = "macos"),
                );
                self.viewer.key(KeyPress { key, ctrl, alt });
                self.after_keys();
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                if state == ElementState::Pressed {
                    let shift = self.modifiers.shift_key();
                    self.viewer
                        .pointer_pressed(self.cursor, shift, Instant::now());
                } else {
                    self.viewer.pointer_released();
                }
                // A press may end typing or take an entry of a list.
                self.after_keys();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let line = 60.0 * self.window.scale_factor();
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (f64::from(x) * line, f64::from(y) * line),
                    MouseScrollDelta::PixelDelta(position) => (position.x, position.y),
                };
                if self.modifiers.control_key() {
                    self.viewer.zoom_at(1.1_f64.powf(dy / line), self.cursor);
                } else {
                    // Turning the wheel away from the user moves the page
                    // down, which is the view moving up.
                    self.viewer.scroll(-dx, -dy);
                }
            }
            WindowEvent::PinchGesture { delta, .. } => {
                self.viewer.zoom_at(1.0 + delta, self.cursor);
            }
            _ => return,
        }
        self.input_at.get_or_insert_with(Instant::now);
        self.window.request_redraw();
    }

    fn request(&mut self, request: Request) {
        match request {
            Request::Keys(keys) => {
                for key in keys {
                    self.viewer.key(key);
                }
                self.after_keys();
                self.input_at.get_or_insert_with(Instant::now);
            }
            Request::State(reply) => {
                let _ = reply.send(self.viewer.state());
            }
            Request::Do(task) => {
                task(&mut self.viewer);
                self.after_keys();
            }
            // Answered at the end of a frame that finds everything settled.
            waiting => self.waiting.push(waiting),
        }
    }

    /// Schedules a compile if the keys just handled changed the text.
    ///
    /// Typing is compiled when it pauses. A command that changes the text
    /// is compiled at once, unless commands come in quick succession (a
    /// held key): those are compiled when they pause, like typing.
    fn after_keys(&mut self) {
        // Ctrl-C while a compile runs long: given up on, with what waits.
        if self.viewer.take_stop() {
            self.scheduler.cancel();
            let ran = self.compiler.abandon();
            self.viewer.stopped(ran);
        }
        let Some(edited) = self.viewer.take_edited() else {
            return;
        };
        let now = Instant::now();
        let rapid = self
            .last_edit
            .is_some_and(|last| now.duration_since(last) < self.debounce);
        let urgency = match edited {
            Edited::Typing => Urgency::WhenIdle,
            Edited::Command if rapid => Urgency::WhenIdle,
            Edited::Command | Edited::LeftInsert | Edited::Unpredicted | Edited::Agent => {
                Urgency::Now
            }
        };
        if edited == Edited::Command {
            self.last_edit = Some(now);
        }
        self.scheduler.request(urgency, now);
    }

    fn files_changed(&mut self, paths: &[PathBuf]) {
        if self.viewer.knocked(paths) {
            self.window
                .request_user_attention(Some(winit::window::UserAttentionType::Informational));
        }
        let dependencies = self.viewer.dependencies();
        let mut changed = false;
        let known = |path: &PathBuf| {
            self.watcher
                .as_ref()
                .map_or_else(|| path.clone(), |watcher| watcher.known_as(path))
        };
        let paths: Vec<PathBuf> = paths.iter().map(known).collect();
        for path in paths.iter().filter(|path| dependencies.contains(path)) {
            changed |= self.viewer.file_changed(path);
        }
        if changed {
            self.scheduler.request(Urgency::Now, Instant::now());
        }
    }

    fn watch_dependencies(&mut self) {
        if let Some(watcher) = &mut self.watcher {
            watcher.watch(&self.viewer.dependencies());
            // And where another start of zetwerk says it wanted a file
            // that is open here.
            watcher.watch(&self.viewer.knocks());
        }
    }

    fn redraw(&mut self) {
        let start = Instant::now();
        // Asked before the results are taken: see `Compiler::is_idle`.
        let compiled_all = self.compiler.is_idle();
        if let Some(output) = self.compiler.poll() {
            self.viewer.compiled(output);
            // The compile may have read files it had not read before.
            self.watch_dependencies();
            // And what it said may want the pages made again.
            self.after_keys();
        }
        self.viewer.export_progress();
        for tile in self.rasterizer.poll() {
            let (width, height) = (tile.pixmap.width(), tile.pixmap.height());
            self.gpu
                .upload(viewer::texture(tile.key), width, height, tile.pixmap.data());
            self.viewer.stats.tiles += 1;
        }
        self.viewer.stats.texture_bytes = self.gpu.texture_bytes();

        let gpu = &self.gpu;
        let mut scene = self.viewer.scene(&|id| gpu.has(id));
        for image in &scene.images {
            let pixmap = &image.pixmap;
            self.gpu
                .upload(image.id, pixmap.width(), pixmap.height(), pixmap.data());
        }
        // No pages are being made or about to be: those of this revision
        // may be made a second time, when Typst refused something that
        // is then shown another way.
        let settled = self.viewer.is_compiled()
            && compiled_all
            && self.scheduler.deadline().is_none()
            && scene.wanted.is_empty()
            && self.rasterizer.is_idle();
        present::draw_second(self, &mut scene.wanted);
        self.rasterizer.request(scene.wanted);
        self.present(&scene.quads);
        self.viewer
            .report_frame(&mut self.gpu, &self.config, &scene.quads);

        let stats = &mut self.viewer.stats;
        stats.frames += 1;
        stats.frame_ms = start.elapsed().as_secs_f64() * 1000.0;
        if let Some(input_at) = self.input_at.take() {
            stats.latency_ms = input_at.elapsed().as_secs_f64() * 1000.0;
        }
        self.answer_waiting(settled, &scene.quads);
    }

    fn present(&mut self, quads: &[Quad]) {
        use wgpu::CurrentSurfaceTexture as Current;
        let frame = match self.surface.get_current_texture() {
            Current::Success(frame) | Current::Suboptimal(frame) => frame,
            Current::Outdated | Current::Lost => {
                // The surface no longer matches the window; set it up again
                // and draw on the next round.
                self.surface.configure(self.gpu.device(), &self.config);
                self.window.request_redraw();
                return;
            }
            // Nothing to draw into right now (window hidden, compositor
            // busy). The next event draws again.
            _ => return,
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.gpu.target_format()),
            ..Default::default()
        });
        self.gpu
            .draw(&view, (self.config.width, self.config.height), quads);
        self.window.pre_present_notify();
        self.gpu.present(frame);
    }

    /// Replies to the requests that were waiting for the frame just drawn.
    fn answer_waiting(&mut self, settled: bool, quads: &[Quad]) {
        let compiled = self.viewer.is_compiled();
        let size = (self.config.width, self.config.height);
        let mut still_waiting = Vec::new();
        for request in std::mem::take(&mut self.waiting) {
            match request {
                Request::WhenSettled(reply) if settled => {
                    let _ = reply.send(());
                }
                Request::Screenshot(reply) if settled => {
                    let capture = self.gpu.capture(size, quads);
                    let _ = reply.send(
                        capture
                            .map(|rgba| Screenshot {
                                width: size.0,
                                height: size.1,
                                rgba,
                            })
                            .map_err(|err| err.to_string()),
                    );
                }
                Request::Document(reply) if compiled => {
                    let _ = reply.send(self.viewer.document().cloned());
                }
                Request::WhenCompiled(task) if compiled => {
                    task(&mut self.viewer);
                    // It may have changed something itself.
                    self.after_keys();
                    self.window.request_redraw();
                }
                waiting => still_waiting.push(waiting),
            }
        }
        self.waiting = still_waiting;
    }
}

/// The key a keyboard event stands for, if it is one zetwerk knows.
///
/// `text` is what the press typed, if it typed something. After a dead
/// key, or with compose, that is the character several presses made
/// (`^` and then Space is `^`, and then `e` is `ê`), and the key itself
/// does not say so: on a German or a US international keyboard, `^`,
/// `` ` ``, `'`, `"` and `~` are typed so. Not with Ctrl held: the key
/// is what counts then.
fn translate(key: &WinitKey, text: Option<&str>, ctrl: bool) -> Option<Key> {
    let typed = text.filter(|_| !ctrl).and_then(|text| {
        let mut chars = text.chars();
        let first = chars.next()?;
        (chars.next().is_none() && !first.is_control()).then_some(first)
    });
    let printable = matches!(
        key,
        WinitKey::Character(_) | WinitKey::Dead(_) | WinitKey::Named(NamedKey::Space)
    );
    if let (Some(c), true) = (typed, printable) {
        return Some(Key::Char(c));
    }
    Some(match key {
        WinitKey::Character(text) => Key::Char(text.chars().next()?),
        WinitKey::Named(named) => match named {
            NamedKey::Enter => Key::Enter,
            NamedKey::Escape => Key::Escape,
            NamedKey::Backspace => Key::Backspace,
            NamedKey::Delete => Key::Delete,
            NamedKey::Tab => Key::Tab,
            NamedKey::Space => Key::Char(' '),
            NamedKey::ArrowUp => Key::Up,
            NamedKey::ArrowDown => Key::Down,
            NamedKey::ArrowLeft => Key::Left,
            NamedKey::ArrowRight => Key::Right,
            NamedKey::PageUp => Key::PageUp,
            NamedKey::PageDown => Key::PageDown,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            NamedKey::F5 => Key::F(5),
            _ => return None,
        },
        // With Ctrl, a dead key is the key: Ctrl-^ on a German keyboard,
        // to the file edited before.
        WinitKey::Dead(Some(c)) if ctrl => Key::Char(*c),
        _ => return None,
    })
}

/// `key` as it is with Shift held: Tab and Enter have a meaning of their
/// own then. (Characters have Shift in them already.)
fn shifted(key: Key, shift: bool) -> Key {
    match key {
        Key::Tab if shift => Key::BackTab,
        Key::Enter if shift => Key::ShiftEnter,
        key => key,
    }
}

/// Whether a key counts as pressed with Alt. On a Mac, Option (which is
/// Alt) is how many characters are typed, as `AltGr` is elsewhere: a
/// character it typed (`text`, without Ctrl) is that character, not
/// Alt and a key. Elsewhere Alt is Alt (`AltGr` is not reported as it).
fn alt_held(text: Option<&str>, ctrl: bool, alt: bool, mac: bool) -> bool {
    let typed = text.is_some_and(|text| {
        let mut chars = text.chars();
        chars
            .next()
            .is_some_and(|first| chars.next().is_none() && !first.is_control())
    });
    alt && !(mac && typed && !ctrl)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_tab_and_shift_enter_are_keys_of_their_own() {
        assert_eq!(shifted(Key::Tab, true), Key::BackTab);
        assert_eq!(shifted(Key::Enter, true), Key::ShiftEnter);
        assert_eq!(shifted(Key::Tab, false), Key::Tab);
        // Shift is in a character already, and an arrow is an arrow.
        assert_eq!(shifted(Key::Char('A'), true), Key::Char('A'));
        assert_eq!(shifted(Key::Left, true), Key::Left);
    }

    #[test]
    fn a_character_typed_with_option_on_a_mac_is_that_character() {
        // Option-2 on a German Mac types `@`: no Alt, so that it can be
        // typed on the command line too.
        assert!(!alt_held(Some("@"), false, true, true));
        assert!(!alt_held(Some("\u{e5}"), false, true, true));
        // Option with a key that types nothing (a dead key, an arrow) is
        // still Alt, as is Alt everywhere else.
        assert!(alt_held(None, false, true, true));
        assert!(alt_held(Some("x"), false, true, false));
        assert!(alt_held(Some("\u{1b}"), false, true, true));
        assert!(!alt_held(Some("@"), false, false, true));
    }

    #[test]
    fn keyboard_dead_keys_and_compose_type_what_they_make() {
        let character = |c: &str| WinitKey::Character(c.into());
        let space = WinitKey::Named(NamedKey::Space);
        // A dead key alone types nothing yet; with Ctrl it is the key.
        assert_eq!(translate(&WinitKey::Dead(Some('^')), None, false), None);
        assert_eq!(
            translate(&WinitKey::Dead(Some('^')), None, true),
            Some(Key::Char('^'))
        );
        // Then Space types the accent itself, a letter the two together.
        assert_eq!(translate(&space, Some("^"), false), Some(Key::Char('^')));
        assert_eq!(translate(&space, Some("`"), false), Some(Key::Char('`')));
        assert_eq!(
            translate(&character("e"), Some("\u{ea}"), false),
            Some(Key::Char('\u{ea}'))
        );
        // AltGr and Shift are in the key already.
        assert_eq!(
            translate(&character("@"), Some("@"), false),
            Some(Key::Char('@'))
        );
        assert_eq!(translate(&space, Some(" "), false), Some(Key::Char(' ')));
        // With Ctrl, the key: what it types is a control character, or
        // nothing.
        assert_eq!(
            translate(&character("w"), Some("\u{17}"), true),
            Some(Key::Char('w'))
        );
        assert_eq!(translate(&character("w"), None, true), Some(Key::Char('w')));
        assert_eq!(
            translate(&WinitKey::Named(NamedKey::Tab), Some("\t"), false),
            Some(Key::Tab)
        );
    }
}
