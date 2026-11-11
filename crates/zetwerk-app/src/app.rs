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
use zetwerk_typst::{Compiler, Environment, FontSet, Project, Scheduler, Urgency};

use crate::editor::Editor;
use crate::gpu::{self, Gpu, Quad};
use crate::keys::{Key, KeyPress};
use crate::power::PowerMode;
use crate::viewer::{self, Edited, State, Viewer};
use crate::watch::FileWatcher;

/// How the window should be opened.
pub struct Options {
    /// The main file of the document.
    pub path: PathBuf,
    pub font_set: FontSet,
    pub power: PowerMode,
    /// Where the journals that make edits survive a crash are kept.
    pub state_dir: PathBuf,
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
    Quit,
}

/// The pixels of a window.
pub struct Screenshot {
    pub width: u32,
    pub height: u32,
    /// Rows of RGBA, top to bottom.
    pub rgba: Vec<u8>,
}

/// Sends [`Request`]s to a running window.
#[derive(Clone)]
pub struct Handle {
    proxy: EventLoopProxy<Event>,
}

impl Handle {
    /// Sends `request`. Returns `false` if the window is gone.
    pub fn send(&self, request: Request) -> bool {
        self.proxy.send_event(Event::Request(request)).is_ok()
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
        proxy: event_loop.create_proxy(),
    });
    let mut app = App {
        options,
        proxy: event_loop.create_proxy(),
        session: None,
        error: None,
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
}

struct Session {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
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
                self.error = Some(err);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(session) = &mut self.session else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            // Away from the window is a good moment to make sure.
            WindowEvent::Focused(false) => session.viewer.sync(),
            WindowEvent::RedrawRequested => session.redraw(),
            other => session.window_event(other),
        }
        if session.viewer.wants_quit() {
            event_loop.exit();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Event) {
        let Some(session) = &mut self.session else {
            return;
        };
        match event {
            Event::Wake => {}
            Event::FilesChanged(paths) => session.files_changed(&paths),
            Event::Request(Request::Quit) => event_loop.exit(),
            Event::Request(request) => session.request(request),
        }
        session.window.request_redraw();
        if session.viewer.wants_quit() {
            event_loop.exit();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(session) = &mut self.session else {
            return;
        };
        let now = Instant::now();
        if session.scheduler.take_due(now) {
            let (revision, world) = session.viewer.snapshot();
            session.compiler.compile(revision, world);
        }
        let mut sync_at = session
            .viewer
            .unsynced_since()
            .map(|since| since + SYNC_AFTER);
        if sync_at.is_some_and(|at| at <= now) {
            session.viewer.sync();
            sync_at = None;
        }
        let deadline = [session.scheduler.deadline(), sync_at]
            .into_iter()
            .flatten()
            .min();
        event_loop.set_control_flow(match deadline {
            Some(deadline) => ControlFlow::WaitUntil(deadline),
            None => ControlFlow::Wait,
        });
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
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
        let project =
            Project::open(Arc::clone(&env), &options.path).map_err(|err| err.to_string())?;
        let (editor, opened) = Editor::open(&options.path, &options.state_dir, project.main())
            .map_err(|err| err.to_string())?;
        let name = options
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        let title = name.map_or_else(|| "zetwerk".to_owned(), |name| format!("{name} — zetwerk"));

        let attributes = Window::default_attributes()
            .with_title(title)
            .with_inner_size(LogicalSize::new(900.0, 1000.0));
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

        let viewer = Viewer::new(
            env,
            project,
            (size.width, size.height),
            window.scale_factor(),
        )
        .ok_or("cannot set up the interface")?
        .editing(editor, &opened);
        let mut session = Self {
            window,
            surface,
            config,
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
        session.watch_dependencies();
        session.scheduler.request(Urgency::Now, Instant::now());
        Ok(session)
    }

    fn window_event(&mut self, event: WindowEvent) {
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
                return;
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let Some(key) = translate(&event.logical_key) else {
                    return;
                };
                self.viewer.key(KeyPress {
                    key,
                    ctrl: self.modifiers.control_key(),
                    alt: self.modifiers.alt_key(),
                });
                self.after_keys();
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => self.viewer.click(self.cursor),
            WindowEvent::MouseWheel { delta, .. } => {
                let line = 60.0 * self.window.scale_factor();
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (f64::from(x) * line, f64::from(y) * line),
                    MouseScrollDelta::PixelDelta(position) => (position.x, position.y),
                };
                if self.modifiers.control_key() {
                    self.viewer.zoom_by(1.1_f64.powf(dy / line), self.cursor);
                } else {
                    // Turning the wheel away from the user moves the page
                    // down, which is the view moving up.
                    self.viewer.scroll(-dx, -dy);
                }
            }
            WindowEvent::PinchGesture { delta, .. } => {
                self.viewer.zoom_by(1.0 + delta, self.cursor);
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
            Edited::Command | Edited::LeftInsert | Edited::Unpredicted => Urgency::Now,
        };
        if edited == Edited::Command {
            self.last_edit = Some(now);
        }
        self.scheduler.request(urgency, now);
    }

    fn files_changed(&mut self, paths: &[PathBuf]) {
        let dependencies = self.viewer.dependencies();
        let mut changed = false;
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
        }
    }

    fn redraw(&mut self) {
        let start = Instant::now();
        if let Some(output) = self.compiler.poll() {
            self.viewer.compiled(output);
            // The compile may have read files it had not read before.
            self.watch_dependencies();
        }
        for tile in self.rasterizer.poll() {
            let (width, height) = (tile.pixmap.width(), tile.pixmap.height());
            self.gpu
                .upload(viewer::texture(tile.key), width, height, tile.pixmap.data());
            self.viewer.stats.tiles += 1;
        }
        self.viewer.stats.texture_bytes = self.gpu.texture_bytes();

        let gpu = &self.gpu;
        let scene = self.viewer.scene(&|id| gpu.has(id));
        for image in &scene.images {
            let pixmap = &image.pixmap;
            self.gpu
                .upload(image.id, pixmap.width(), pixmap.height(), pixmap.data());
        }
        let settled =
            self.viewer.is_compiled() && scene.wanted.is_empty() && self.rasterizer.is_idle();
        self.rasterizer.request(scene.wanted);
        self.present(&scene.quads);

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
                waiting => still_waiting.push(waiting),
            }
        }
        self.waiting = still_waiting;
    }
}

/// The key a keyboard event stands for, if it is one zetwerk knows.
fn translate(key: &WinitKey) -> Option<Key> {
    Some(match key {
        WinitKey::Character(text) => Key::Char(text.chars().next()?),
        WinitKey::Named(named) => match named {
            NamedKey::Enter => Key::Enter,
            NamedKey::Escape => Key::Escape,
            NamedKey::Backspace => Key::Backspace,
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
            _ => return None,
        },
        _ => return None,
    })
}
