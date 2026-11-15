//! A window that says why zetwerk did not open, when it was started
//! without a terminal (from a launcher, a file manager): what is written
//! to standard error there is read by nobody, and nothing would happen
//! that anyone could see. It stays for a moment, or until a key.

use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;
use zetwerk_typst::{Environment, FontSet};

use crate::chrome::{self, Chrome};
use crate::gpu::{self, Gpu, Quad, TextureId};

/// How long the window stays unless a key closes it.
const SHOWN: Duration = Duration::from_secs(10);
/// How many characters a line of the message has at most.
const LINE: usize = 64;

/// The window, and what it shows.
pub(super) struct Refusal {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    gpu: Gpu,
    image: (TextureId, u32, u32),
    until: Instant,
}

/// `message` in lines of at most [`LINE`] characters, broken at blanks.
fn lines(message: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in message.lines() {
        let mut line = String::new();
        for word in paragraph.split(' ') {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > LINE {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    lines
}

impl Refusal {
    /// Opens the window with `message`. `None` if it cannot be opened or
    /// drawn into: then nothing more can be done.
    pub(super) fn show(event_loop: &ActiveEventLoop, message: &str) -> Option<Self> {
        let attributes = Window::default_attributes()
            .with_title("zetwerk did not open")
            .with_inner_size(LogicalSize::new(560.0, 160.0));
        let window = Arc::new(event_loop.create_window(attributes).ok()?);
        let mut chrome = Chrome::new(Environment::new(FontSet::Embedded))?;
        let mut shown = lines(message);
        shown.push(String::new());
        shown.push("(this goes by itself; any key closes it now)".to_owned());
        let image = chrome.render(&chrome::text_markup(&shown), window.scale_factor())?;
        let (width, height) = (image.pixmap.width(), image.pixmap.height());
        let _ = window.request_inner_size(PhysicalSize::new(width, height));

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
            Box::new(event_loop.owned_display_handle()),
        ));
        let surface = instance.create_surface(Arc::clone(&window)).ok()?;
        let adapter = gpu::adapter(&instance, Some(&surface)).ok()?;
        let size = window.inner_size();
        let mut config =
            surface.get_default_config(&adapter, size.width.max(1), size.height.max(1))?;
        let target_format = config.format.remove_srgb_suffix();
        if target_format != config.format {
            config.view_formats = vec![target_format];
        }
        let mut gpu = Gpu::new(&adapter, target_format).ok()?;
        surface.configure(gpu.device(), &config);
        gpu.upload(image.id, width, height, image.pixmap.data());
        window.request_redraw();
        Some(Self {
            window,
            surface,
            config,
            gpu,
            image: (image.id, width, height),
            until: Instant::now() + SHOWN,
        })
    }

    /// When the window goes by itself.
    pub(super) fn until(&self) -> Instant {
        self.until
    }

    /// Handles `event`. Returns whether the window is to go.
    pub(super) fn event(&mut self, event: WindowEvent) -> bool {
        match event {
            WindowEvent::CloseRequested | WindowEvent::KeyboardInput { .. } => return true,
            WindowEvent::Resized(size) => {
                self.config.width = size.width.max(1);
                self.config.height = size.height.max(1);
                self.surface.configure(self.gpu.device(), &self.config);
                self.window.request_redraw();
            }
            WindowEvent::RedrawRequested => self.draw(),
            _ => {}
        }
        false
    }

    fn draw(&mut self) {
        use wgpu::CurrentSurfaceTexture as Current;
        let frame = match self.surface.get_current_texture() {
            Current::Success(frame) | Current::Suboptimal(frame) => frame,
            Current::Outdated | Current::Lost => {
                self.surface.configure(self.gpu.device(), &self.config);
                self.window.request_redraw();
                return;
            }
            _ => return,
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.gpu.target_format()),
            ..Default::default()
        });
        let (texture, width, height) = self.image;
        let quad = Quad {
            texture,
            x: 0.0,
            y: 0.0,
            width: width as f32,
            height: height as f32,
        };
        self.gpu
            .draw(&view, (self.config.width, self.config.height), &[quad]);
        self.window.pre_present_notify();
        self.gpu.present(frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_is_broken_into_lines_at_blanks() {
        let message = "/home/someone/papers/a rather long name.typ is open in another \
                       zetwerk window already\nsecond paragraph";
        let broken = lines(message);
        assert!(broken.iter().all(|line| line.chars().count() <= LINE));
        assert_eq!(broken.join(" "), message.replace('\n', " "));
        assert_eq!(broken.last().map(String::as_str), Some("second paragraph"));
    }
}
