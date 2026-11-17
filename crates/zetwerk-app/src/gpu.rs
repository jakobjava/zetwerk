//! The compositor: puts already-rasterized images on screen.
//!
//! Nothing is rasterized on the GPU. Pages arrive as tiles of pixels and the
//! chrome as small images; the GPU only places them. That keeps scrolling and
//! zooming to moving a few textured rectangles, and keeps the pixels exactly
//! those Typst's own renderer produced.

use std::collections::HashMap;
use std::fmt;

use bytemuck::{Pod, Zeroable};

mod tone;
pub use tone::{Paper, Tone};

/// Identifies an image the compositor holds. Equal ids mean equal pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextureId(pub u128);

/// An image placed on screen. Coordinates are in pixels from the top-left
/// corner of the target; the image is stretched to `width` × `height`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quad {
    pub texture: TextureId,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Why the compositor could not be set up or could not draw.
#[derive(Debug)]
pub enum GpuError {
    NoAdapter(wgpu::RequestAdapterError),
    NoDevice(wgpu::RequestDeviceError),
    Readback(String),
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAdapter(err) => write!(f, "no usable graphics adapter: {err}"),
            Self::NoDevice(err) => write!(f, "cannot open the graphics device: {err}"),
            Self::Readback(err) => write!(f, "cannot read the rendered frame back: {err}"),
        }
    }
}

impl std::error::Error for GpuError {}

/// Textures the compositor may keep before it starts dropping the least
/// recently drawn ones. A full-screen 4K view needs about 35 MB of tiles.
const TEXTURE_BUDGET: u64 = 384 * 1024 * 1024;

const SHADER: &str = "
struct VertexOut {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

@vertex
fn vertex(@location(0) position: vec2f, @location(1) uv: vec2f) -> VertexOut {
    return VertexOut(vec4f(position, 0.0, 1.0), uv);
}

@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;

// How the pages are coloured (see `tone`): a map of straight colours,
// then how many pages and pictures, whether to map, what pictures are
// multiplied by; then where the pages and the pictures are.
struct Tone {
    red: vec4f,
    green: vec4f,
    blue: vec4f,
    add: vec4f,
    counts: vec4f,
    pages: array<vec4f, 16>,
    pictures: array<vec4f, 16>,
}

@group(1) @binding(0) var<uniform> tone: Tone;

fn inside(at: vec2f, rect: vec4f) -> bool {
    return at.x >= rect.x && at.y >= rect.y && at.x < rect.x + rect.z && at.y < rect.y + rect.w;
}

@fragment
fn fragment(in: VertexOut) -> @location(0) vec4f {
    let colour = textureSample(image, image_sampler, in.uv);
    if tone.counts.z < 0.5 || colour.a <= 0.0 {
        return colour;
    }
    let at = in.position.xy;
    var on_page = false;
    for (var i = 0u; i < u32(tone.counts.x); i++) {
        if inside(at, tone.pages[i]) {
            on_page = true;
            break;
        }
    }
    if !on_page {
        return colour;
    }
    for (var i = 0u; i < u32(tone.counts.y); i++) {
        if inside(at, tone.pictures[i]) {
            return vec4f(colour.rgb * tone.counts.w, colour.a);
        }
    }
    let straight = colour.rgb / colour.a;
    let mapped = vec3f(
        dot(tone.red.xyz, straight),
        dot(tone.green.xyz, straight),
        dot(tone.blue.xyz, straight),
    ) + tone.add.xyz;
    return vec4f(clamp(mapped, vec3f(0.0), vec3f(1.0)) * colour.a, colour.a);
}
";

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 2],
    uv: [f32; 2],
}

struct Entry {
    bind_group: wgpu::BindGroup,
    bytes: u64,
    /// The frame this texture was last drawn in.
    used: u64,
}

/// Draws [`Quad`]s into a target of one fixed pixel format.
pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    target_format: wgpu::TextureFormat,
    textures: HashMap<TextureId, Entry>,
    texture_bytes: u64,
    frame: u64,
    /// How the pages are coloured, as last asked; the uniform the shader
    /// reads it from; and what is bound for quads that take the tone and
    /// for all others.
    paper: Paper,
    tone: wgpu::Buffer,
    toned: wgpu::BindGroup,
    plain: wgpu::BindGroup,
}

impl Gpu {
    /// Opens a device on `adapter` and prepares to draw into targets of
    /// `target_format`, which must not be an sRGB format: images are passed
    /// through as the bytes they are, and blended the way `tiny-skia`
    /// blended them.
    pub fn new(
        adapter: &wgpu::Adapter,
        target_format: wgpu::TextureFormat,
    ) -> Result<Self, GpuError> {
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("zetwerk"),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            ..Default::default()
        }))
        .map_err(GpuError::NoDevice)?;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quad"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("image"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let (tone_layout, tone, toned, plain) = tone_bindings(&device);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("quad"),
            bind_group_layouts: &[Some(&bind_layout), Some(&tone_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quad"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    // Pixmaps are premultiplied.
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("image"),
            // Only matters while a page is shown scaled, between a zoom
            // step and its sharp tiles arriving.
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Ok(Self {
            device,
            queue,
            pipeline,
            bind_layout,
            sampler,
            target_format,
            textures: HashMap::new(),
            texture_bytes: 0,
            frame: 0,
            paper: Paper::default(),
            tone,
            toned,
            plain,
        })
    }

    /// Colours the pages of the frames to come as `paper` says (see
    /// [`Paper`]).
    pub fn set_paper(&mut self, paper: &Paper) {
        if *paper == self.paper {
            return;
        }
        let uniform = tone::uniform(Some(paper));
        self.queue
            .write_buffer(&self.tone, 0, bytemuck::cast_slice(&uniform));
        paper.clone_into(&mut self.paper);
    }

    #[must_use]
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// Shows a window frame that [`Gpu::draw`] has drawn into.
    pub fn present(&self, frame: wgpu::SurfaceTexture) {
        self.queue.present(frame);
    }

    /// The format of the targets this compositor draws into.
    #[must_use]
    pub fn target_format(&self) -> wgpu::TextureFormat {
        self.target_format
    }

    /// Whether an image with this id has been uploaded and is still held.
    #[must_use]
    pub fn has(&self, id: TextureId) -> bool {
        self.textures.contains_key(&id)
    }

    /// Bytes of texture memory in use.
    #[must_use]
    pub fn texture_bytes(&self) -> u64 {
        self.texture_bytes
    }

    /// Stores an image of premultiplied RGBA pixels under `id`. Does nothing
    /// if `id` is already held or the size does not match the data.
    pub fn upload(&mut self, id: TextureId, width: u32, height: u32, rgba: &[u8]) {
        let bytes = u64::from(width) * u64::from(height) * 4;
        if self.has(id) || width == 0 || height == 0 || rgba.len() as u64 != bytes {
            return;
        }
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            texture.as_image_copy(),
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.textures.insert(
            id,
            Entry {
                bind_group,
                bytes,
                used: self.frame,
            },
        );
        self.texture_bytes += bytes;
    }

    /// Draws `quads` in order into `target`, which is `size` pixels large,
    /// over a black background. Quads whose image is not held are skipped.
    pub fn draw(&mut self, target: &wgpu::TextureView, size: (u32, u32), quads: &[Quad]) {
        self.frame += 1;
        let (width, height) = (size.0.max(1) as f32, size.1.max(1) as f32);
        let mut vertices = Vec::with_capacity(quads.len() * 6);
        let mut draws = Vec::with_capacity(quads.len());
        for (index, quad) in quads.iter().enumerate() {
            let Some(entry) = self.textures.get_mut(&quad.texture) else {
                continue;
            };
            entry.used = self.frame;
            let left = quad.x / width * 2.0 - 1.0;
            let right = (quad.x + quad.width) / width * 2.0 - 1.0;
            let top = 1.0 - quad.y / height * 2.0;
            let bottom = 1.0 - (quad.y + quad.height) / height * 2.0;
            let corner = |x, y, u, v| Vertex {
                position: [x, y],
                uv: [u, v],
            };
            let first = vertices.len() as u32;
            vertices.extend([
                corner(left, top, 0.0, 0.0),
                corner(left, bottom, 0.0, 1.0),
                corner(right, top, 1.0, 0.0),
                corner(right, top, 1.0, 0.0),
                corner(left, bottom, 0.0, 1.0),
                corner(right, bottom, 1.0, 1.0),
            ]);
            draws.push((quad.texture, first..first + 6, self.paper.tones(index)));
        }

        let vertex_buffer = (!vertices.is_empty()).then(|| {
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("quads"),
                size: size_of_val(vertices.as_slice()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queue
                .write_buffer(&buffer, 0, bytemuck::cast_slice(&vertices));
            buffer
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            if let Some(buffer) = &vertex_buffer {
                pass.set_pipeline(&self.pipeline);
                pass.set_vertex_buffer(0, buffer.slice(..));
                for (texture, range, toned) in draws {
                    if let Some(entry) = self.textures.get(&texture) {
                        pass.set_bind_group(0, &entry.bind_group, &[]);
                        let tone = if toned { &self.toned } else { &self.plain };
                        pass.set_bind_group(1, tone, &[]);
                        pass.draw(range, 0..1);
                    }
                }
            }
        }
        self.queue.submit([encoder.finish()]);
        self.evict();
    }

    /// Draws `quads` into an off-screen target and returns its pixels as
    /// straight (not premultiplied) RGBA rows, top to bottom. This goes
    /// through the same pipeline as drawing to the window.
    pub fn capture(&mut self, size: (u32, u32), quads: &[Quad]) -> Result<Vec<u8>, GpuError> {
        let (width, height) = (size.0.max(1), size.1.max(1));
        let extent = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("capture"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.target_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        self.draw(
            &texture.create_view(&wgpu::TextureViewDescriptor::default()),
            size,
            quads,
        );

        // Rows in a buffer copy must be padded to a fixed alignment.
        let row = 4 * width;
        let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("capture"),
            size: u64::from(padded) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("capture"),
            });
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            extent,
        );
        self.queue.submit([encoder.finish()]);

        let slice = buffer.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|err| GpuError::Readback(err.to_string()))?;
        receiver
            .recv()
            .map_err(|err| GpuError::Readback(err.to_string()))?
            .map_err(|err| GpuError::Readback(err.to_string()))?;

        let bgra = matches!(
            self.target_format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        let data = slice
            .get_mapped_range()
            .map_err(|err| GpuError::Readback(err.to_string()))?;
        let mut pixels = Vec::with_capacity((row * height) as usize);
        for line in data.chunks(padded as usize) {
            pixels.extend_from_slice(&line[..row as usize]);
        }
        if bgra {
            for pixel in pixels.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
        }
        Ok(pixels)
    }

    /// Drops the least recently drawn textures once over budget. Textures
    /// drawn in the current frame are never dropped.
    fn evict(&mut self) {
        if self.texture_bytes <= TEXTURE_BUDGET {
            return;
        }
        let mut idle: Vec<(u64, TextureId)> = self
            .textures
            .iter()
            .filter(|(_, entry)| entry.used < self.frame)
            .map(|(id, entry)| (entry.used, *id))
            .collect();
        idle.sort_unstable_by_key(|&(used, id)| (used, id.0));
        for (_, id) in idle {
            if self.texture_bytes <= TEXTURE_BUDGET {
                break;
            }
            if let Some(entry) = self.textures.remove(&id) {
                self.texture_bytes -= entry.bytes;
            }
        }
    }
}

/// The layout of the uniform that says how pages are coloured, the
/// buffer it is in, and what is bound for quads on pages and for others:
/// the latter is a uniform that maps nothing.
fn tone_bindings(
    device: &wgpu::Device,
) -> (
    wgpu::BindGroupLayout,
    wgpu::Buffer,
    wgpu::BindGroup,
    wgpu::BindGroup,
) {
    use wgpu::util::DeviceExt as _;
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("tone"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let buffer = |label, contents: &[f32]| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(contents),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        })
    };
    let nothing = tone::uniform(None);
    let (toned, plain) = (buffer("tone", &nothing), buffer("no tone", &nothing));
    let group = |buffer: &wgpu::Buffer| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        })
    };
    let (toned_group, plain_group) = (group(&toned), group(&plain));
    (layout, toned, toned_group, plain_group)
}

/// Finds a graphics adapter, preferring the one that uses least power: the
/// compositor only places a handful of rectangles.
pub fn adapter(
    instance: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'_>>,
) -> Result<wgpu::Adapter, GpuError> {
    pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: surface,
        ..Default::default()
    }))
    .map_err(GpuError::NoAdapter)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A compositor without a window, or `None` on machines without any
    /// graphics adapter (where there is nothing to test).
    fn headless() -> Option<Gpu> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = adapter(&instance, None).ok()?;
        Gpu::new(&adapter, wgpu::TextureFormat::Rgba8Unorm).ok()
    }

    fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * width + x) * 4) as usize;
        [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
    }

    #[test]
    fn quads_land_where_they_are_placed() {
        let Some(mut gpu) = headless() else {
            eprintln!("no graphics adapter; skipping");
            return;
        };
        let (red, half_white) = (TextureId(1), TextureId(2));
        gpu.upload(red, 1, 1, &[255, 0, 0, 255]);
        // Premultiplied 50 % white.
        gpu.upload(half_white, 1, 1, &[128, 128, 128, 128]);
        assert!(gpu.has(red) && !gpu.has(TextureId(3)));
        assert_eq!(gpu.texture_bytes(), 8);

        let quads = [
            Quad {
                texture: red,
                x: 10.0,
                y: 20.0,
                width: 30.0,
                height: 40.0,
            },
            Quad {
                texture: half_white,
                x: 30.0,
                y: 20.0,
                width: 30.0,
                height: 10.0,
            },
            // Not uploaded: skipped rather than failing the frame.
            Quad {
                texture: TextureId(3),
                x: 0.0,
                y: 0.0,
                width: 64.0,
                height: 64.0,
            },
        ];
        let pixels = gpu
            .capture((64, 64), &quads)
            .unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(pixels.len(), 64 * 64 * 4);
        assert_eq!(pixel(&pixels, 64, 0, 0), [0, 0, 0, 255], "background");
        assert_eq!(
            pixel(&pixels, 64, 10, 20),
            [255, 0, 0, 255],
            "top-left of red"
        );
        assert_eq!(
            pixel(&pixels, 64, 39, 59),
            [255, 0, 0, 255],
            "bottom-right of red"
        );
        assert_eq!(
            pixel(&pixels, 64, 40, 60),
            [0, 0, 0, 255],
            "just outside red"
        );
        // Half white over red, then over black.
        assert_eq!(pixel(&pixels, 64, 35, 25), [255, 128, 128, 255]);
        assert_eq!(pixel(&pixels, 64, 50, 25), [128, 128, 128, 255]);
    }

    #[test]
    fn pages_take_their_tone_where_they_are_and_pictures_keep_theirs() {
        let Some(mut gpu) = headless() else {
            eprintln!("no graphics adapter; skipping");
            return;
        };
        let white = TextureId(1);
        gpu.upload(white, 1, 1, &[255; 4]);
        let quad = |x: f32| Quad {
            texture: white,
            x,
            y: 0.0,
            width: 32.0,
            height: 32.0,
        };
        // The page is where the first quad is, and half of the second
        // reaches out of it; the third is the interface, over the page.
        let quads = [quad(0.0), quad(16.0), quad(0.0)];
        gpu.set_paper(&Paper {
            tone: Tone::Night,
            quads: std::iter::once(0..2).collect(),
            pages: vec![[0.0, 0.0, 40.0, 32.0]],
            pictures: vec![[4.0, 4.0, 4.0, 4.0]],
        });
        let pixels = gpu
            .capture((64, 32), &quads[..2])
            .unwrap_or_else(|err| panic!("{err}"));
        let night = Tone::Night.apply([255; 4]);
        let toned = pixel(&pixels, 64, 20, 20);
        let near = toned.iter().zip(night).all(|(a, b)| a.abs_diff(b) <= 2);
        assert!(near, "on the page: {toned:?}, not {night:?}");
        assert_eq!(pixel(&pixels, 64, 44, 20), [255; 4], "beside it");
        let picture = pixel(&pixels, 64, 5, 5);
        assert!(
            picture[0] > 200 && picture[0] < 230,
            "a picture: {picture:?}"
        );
        let pixels = gpu
            .capture((64, 32), &quads)
            .unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(pixel(&pixels, 64, 20, 20), [255; 4], "the interface");
        // By day, nothing.
        gpu.set_paper(&Paper::default());
        let pixels = gpu
            .capture((64, 32), &quads[..1])
            .unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(pixel(&pixels, 64, 20, 20), [255; 4]);
    }

    #[test]
    fn mismatched_uploads_are_ignored() {
        let Some(mut gpu) = headless() else { return };
        gpu.upload(TextureId(1), 2, 2, &[0; 4]);
        gpu.upload(TextureId(2), 0, 1, &[]);
        assert!(!gpu.has(TextureId(1)) && !gpu.has(TextureId(2)));
    }
}
