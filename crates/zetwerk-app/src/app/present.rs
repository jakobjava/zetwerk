//! What the windows do for a talk (see `viewer::present`).
//!
//! As the talk begins the window goes fullscreen on the display it is on
//! (sway puts it on that output) and hides the pointer; as it ends it is
//! a window again, unless it was fullscreen before. A touch while
//! presenting is the next slide, as a click is.
//!
//! **Two screens.** Where the machine has another display (a projector),
//! the audience's view goes into a second window, fullscreen on that
//! display, and this window, fullscreen on its own, shows the
//! presenter's view: the slide, the next one, the notes, the time. Both
//! are drawn from the one viewer, with the one GPU and its textures, in
//! the same frame; keys and clicks in either are the talk's. Closing the
//! audience's window, or `:present here`, keeps the talk in this one.

use std::sync::Arc;

use winit::event::{TouchPhase, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::monitor::MonitorHandle;
use winit::window::{Fullscreen, Window, WindowId};
use zetwerk_render::Tile;

use super::Session;
use crate::viewer::Viewer;

/// The window that shows the audience's view on a screen of its own.
pub(super) struct Second {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
}

impl Second {
    /// Opens it fullscreen on `screen`.
    fn open(
        session: &Session,
        event_loop: &ActiveEventLoop,
        screen: MonitorHandle,
    ) -> Result<Self, String> {
        let attributes = Window::default_attributes()
            .with_title("zetwerk — the audience's view")
            .with_fullscreen(Some(Fullscreen::Borderless(Some(screen))));
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .map_err(|err| format!("cannot open the audience's window: {err}"))?,
        );
        window.set_cursor_visible(false);
        let surface = session
            .instance
            .create_surface(Arc::clone(&window))
            .map_err(|err| format!("cannot draw into the audience's window: {err}"))?;
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&session.adapter, size.width.max(1), size.height.max(1))
            .ok_or("the graphics adapter cannot draw into the audience's window")?;
        let target = session.gpu.target_format();
        if config.format.remove_srgb_suffix() != target {
            return Err("the audience's window draws in other colours than this one".to_owned());
        }
        if target != config.format {
            config.view_formats = vec![target];
        }
        config.present_mode = session.config.present_mode;
        surface.configure(session.gpu.device(), &config);
        Ok(Self {
            window,
            surface,
            config,
        })
    }
}

/// A display other than the one `window` is on, if there is one.
fn other_screen(window: &Window) -> Option<MonitorHandle> {
    let here = window.current_monitor();
    window
        .available_monitors()
        .find(|screen| Some(screen) != here.as_ref())
}

/// Makes the windows what the talk wants them to be, if that changed.
pub(super) fn follow(session: &mut Session, event_loop: &ActiveEventLoop) {
    let Some((presenting, change)) = session.viewer.take_screen_wish() else {
        return;
    };
    let window = Arc::clone(&session.window);
    window.set_cursor_visible(!presenting);
    if presenting
        && session.viewer.wants_two_screens()
        && let Some(screen) = other_screen(&window)
    {
        match Second::open(session, event_loop, screen) {
            Ok(second) => {
                session.second = Some(second);
                session.viewer.set_audience_apart(true);
            }
            // The talk is given in this window, then.
            Err(why) => eprintln!("zetwerk: {why}"),
        }
    }
    if !presenting {
        session.second = None;
    }
    if presenting && window.fullscreen().is_some() {
        session.viewer.screen_was_full();
    } else if presenting && change {
        window.set_fullscreen(Some(Fullscreen::Borderless(None)));
    } else if change {
        window.set_fullscreen(None);
    }
    window.request_redraw();
}

/// An event of the audience's window. Returns whether it was that
/// window's.
pub(super) fn second_event(session: &mut Session, id: WindowId, event: &WindowEvent) -> bool {
    let Some(second) = &mut session.second else {
        return false;
    };
    if second.window.id() != id {
        return false;
    }
    match event {
        WindowEvent::Resized(size) => {
            second.config.width = size.width.max(1);
            second.config.height = size.height.max(1);
            second
                .surface
                .configure(session.gpu.device(), &second.config);
            session.window.request_redraw();
        }
        // Drawn with this window, in the same frame.
        WindowEvent::RedrawRequested => session.window.request_redraw(),
        WindowEvent::CloseRequested | WindowEvent::Destroyed => {
            session.second = None;
            session.viewer.set_audience_apart(false);
            session.window.request_redraw();
        }
        WindowEvent::KeyboardInput { .. }
        | WindowEvent::ModifiersChanged(_)
        | WindowEvent::MouseInput { .. }
        | WindowEvent::MouseWheel { .. }
        | WindowEvent::CursorMoved { .. }
        | WindowEvent::Touch(_) => session.window_event(event.clone()),
        _ => {}
    }
    true
}

/// Draws the audience's window, if there is one, and adds the tiles it
/// lacks to `wanted`.
pub(super) fn draw_second(session: &mut Session, wanted: &mut Vec<Tile>) {
    let Some(second) = &mut session.second else {
        return;
    };
    let size = (second.config.width, second.config.height);
    let gpu = &session.gpu;
    let Some(scene) = session.viewer.audience_scene(size, &|id| gpu.has(id)) else {
        return;
    };
    for image in &scene.images {
        let pixmap = &image.pixmap;
        session
            .gpu
            .upload(image.id, pixmap.width(), pixmap.height(), pixmap.data());
    }
    wanted.extend(scene.wanted);
    use wgpu::CurrentSurfaceTexture as Current;
    let frame = match second.surface.get_current_texture() {
        Current::Success(frame) | Current::Suboptimal(frame) => frame,
        Current::Outdated | Current::Lost => {
            second
                .surface
                .configure(session.gpu.device(), &second.config);
            second.window.request_redraw();
            return;
        }
        _ => return,
    };
    let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
        format: Some(session.gpu.target_format()),
        ..Default::default()
    });
    session.gpu.draw(&view, size, &scene.quads);
    second.window.pre_present_notify();
    session.gpu.present(frame);
}

/// A touch on the screen while a talk is given. Returns whether it was
/// the talk's.
pub(super) fn touched(viewer: &mut Viewer, event: &WindowEvent) -> bool {
    match event {
        WindowEvent::Touch(touch) if viewer.is_presenting() => {
            if touch.phase == TouchPhase::Started {
                viewer.present_click((touch.location.x, touch.location.y));
            }
            true
        }
        _ => false,
    }
}
