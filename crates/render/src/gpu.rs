//! Purpose: the GPU backend, doing exactly the arithmetic the CPU reference does.
//! Public surface: `GpuSurface`, `GpuError`.
//! Why this file: a GPU backend has no oracle -- there is no third implementation to ask who
//!   is right. So it is built to be bit-identical to the CPU reference rather than merely
//!   close to it, which is what lets `tests/backend.rs` demand byte equality instead of a
//!   tolerance. The blend is integer arithmetic in the shader, the same expression as
//!   `canvas.rs`, so agreement is by construction and not by luck.
//! NOT responsible for: presenting to a screen. This writes into a storage buffer and reads
//!   it back; a window, a swapchain and a surface format are the embedder's problem.
//! Test strategy: `tests/backend.rs` renders identical frames through this and through the
//!   CPU canvas and requires identical bytes, with a truncating control proving the
//!   comparison catches a one-unit error.
//!
//! **The sRGB trap does not bite here, deliberately.** A wgpu surface whose format reports
//! `is_srgb()` interprets fills as LINEAR and gamma-encodes on output, which silently washes
//! out every colour (paid for on 2026-05-18 in another project; near-black rendered as medium
//! grey). Nothing here goes near a surface format: colours are written as bytes into a
//! storage buffer and read back as the same bytes. Any gamma decision belongs to whoever puts
//! these pixels on a screen.

use wgpu::util::DeviceExt;

use crate::color::Rgba;
use crate::surface::Surface;

#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    #[error("no GPU adapter is available: {0}")]
    NoAdapter(String),

    #[error("the GPU adapter refused a device: {0}")]
    NoDevice(String),

    #[error("reading the rendered pixels back from the GPU failed: {0}")]
    Readback(String),
}

/// One recorded drawing operation, replayed on the GPU when pixels are read.
///
/// Recording rather than dispatching immediately is what keeps the ordering honest: a blend
/// reads the pixel underneath it, so operations cannot be reordered or run concurrently with
/// each other. They are replayed as one command buffer, one dispatch per operation, in the
/// order the renderer issued them.
#[derive(Debug, Clone, Copy)]
struct Op {
    kind: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    color: u32,
    mask_offset: u32,
}

const KIND_FILL: u32 = 0;
const KIND_BLEND: u32 = 1;

/// The uniform block a dispatch reads its parameters from. Padded to the dynamic-offset
/// alignment the backend requires, which is why the struct is written out rather than
/// derived from `Op`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    color: u32,
    mask_offset: u32,
    surface_width: u32,
    surface_height: u32,
}

pub struct GpuSurface {
    width: u32,
    height: u32,
    /// Retained across reads, which is what lets a read dispatch only the operations recorded
    /// since the last one. Replaying new work onto the kept buffer is equivalent to replaying
    /// the whole history onto a cleared one, and the ops can then be retired instead of
    /// accumulating for the lifetime of the surface.
    pixels: wgpu::Buffer,
    device: wgpu::Device,
    queue: wgpu::Queue,
    fill: wgpu::ComputePipeline,
    blend: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    ops: Vec<Op>,
    coverage: Vec<u8>,
}

impl GpuSurface {
    /// How much work is queued but not yet dispatched.
    ///
    /// Exposed so a test can pin the retirement directly. Every behavioural test here would
    /// still pass with the log growing for the lifetime of the surface -- the pixels are
    /// right either way, and it is the unbounded memory and the quadratic dispatch cost that
    /// are the defect.
    #[doc(hidden)]
    pub fn recorded_ops(&self) -> usize {
        self.ops.len()
    }

    pub fn new(width: u32, height: u32) -> Result<GpuSurface, GpuError> {
        let instance = wgpu::Instance::default();
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .map_err(|error| GpuError::NoAdapter(error.to_string()))?;

        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .map_err(|error| GpuError::NoDevice(error.to_string()))?;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ruuah-vt raster"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ruuah-vt raster"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<Params>() as u64
                        ),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ruuah-vt raster"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });

        let make = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            })
        };

        let pixel_bytes = ((width as u64) * (height as u64) * 4).max(4);
        let pixels = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pixels"),
            size: pixel_bytes,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // A surface starts cleared to transparent black, exactly as the CPU canvas does.
        queue.write_buffer(&pixels, 0, &vec![0u8; pixel_bytes as usize]);

        Ok(GpuSurface {
            width,
            height,
            pixels,
            fill: make("fill"),
            blend: make("blend"),
            layout,
            device,
            queue,
            ops: Vec::new(),
            coverage: Vec::new(),
        })
    }

    /// Dispatches the operations recorded since the last read, then reads the buffer back.
    ///
    /// The pixel buffer is NOT cleared here. It carries every earlier operation's result, so
    /// dispatching only the new ones lands on the same pixels as replaying the whole history
    /// onto a fresh buffer -- and lets the log be retired, which is the whole point. Clearing
    /// it and replaying only the recent ops would silently drop everything painted before the
    /// previous read.
    fn execute(&mut self) -> Result<Vec<u8>, GpuError> {
        let pixel_bytes = ((self.width as u64) * (self.height as u64) * 4).max(4);
        let pixels = &self.pixels;

        let alignment = self
            .device
            .limits()
            .min_uniform_buffer_offset_alignment
            .max(std::mem::size_of::<Params>() as u32) as usize;

        let mut uniform = vec![0u8; alignment * self.ops.len().max(1)];
        for (index, op) in self.ops.iter().enumerate() {
            let params = Params {
                x: op.x,
                y: op.y,
                width: op.width,
                height: op.height,
                color: op.color,
                mask_offset: op.mask_offset,
                surface_width: self.width,
                surface_height: self.height,
            };
            let base = index * alignment;
            uniform[base..base + std::mem::size_of::<Params>()]
                .copy_from_slice(bytemuck::bytes_of(&params));
        }

        let uniforms = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("params"),
                contents: &uniform,
                usage: wgpu::BufferUsages::UNIFORM,
            });

        // The masks are concatenated into one buffer and indexed by byte offset, so a frame
        // costs one upload rather than one per glyph.
        let mut coverage = self.coverage.clone();
        while coverage.len() % 4 != 0 || coverage.is_empty() {
            coverage.push(0);
        }
        let coverage_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("coverage"),
                contents: &coverage,
                usage: wgpu::BufferUsages::STORAGE,
            });

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ruuah-vt raster"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.pixels.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &uniforms,
                        offset: 0,
                        size: wgpu::BufferSize::new(std::mem::size_of::<Params>() as u64),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: coverage_buffer.as_entire_binding(),
                },
            ],
        });

        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: pixel_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

        // One pass per operation. Passes in a single encoder are ordered, which is what a
        // blend needs: it reads the pixel the previous operation wrote.
        for (index, op) in self.ops.iter().enumerate() {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(if op.kind == KIND_FILL {
                &self.fill
            } else {
                &self.blend
            });
            pass.set_bind_group(0, &bind_group, &[(index * alignment) as u32]);
            pass.dispatch_workgroups(op.width.div_ceil(8), op.height.div_ceil(8), 1);
        }

        encoder.copy_buffer_to_buffer(pixels, 0, &readback, 0, pixel_bytes);
        self.queue.submit(Some(encoder.finish()));

        let slice = readback.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device
            .poll(wgpu::PollType::Wait)
            .map_err(|error| GpuError::Readback(error.to_string()))?;
        receiver
            .recv()
            .map_err(|error| GpuError::Readback(error.to_string()))?
            .map_err(|error| GpuError::Readback(error.to_string()))?;

        let data = slice.get_mapped_range();
        let out = data[..pixel_bytes as usize].to_vec();
        drop(data);
        readback.unmap();

        // Retired: their result now lives in the retained buffer.
        self.ops.clear();
        self.coverage.clear();
        Ok(out)
    }
}

/// Packs a straight-RGBA colour the way the CPU canvas stores it: red at the lowest address,
/// which on a little-endian target is the low byte of the word.
fn pack(color: Rgba) -> u32 {
    u32::from(color[0])
        | (u32::from(color[1]) << 8)
        | (u32::from(color[2]) << 16)
        | (u32::from(color[3]) << 24)
}

impl Surface for GpuSurface {
    /// Panics if no GPU is available.
    ///
    /// The trait cannot report failure, and a renderer explicitly constructed on the GPU
    /// backend has no meaningful way to continue without one. `GpuSurface::new` is the
    /// fallible constructor for a caller that wants to choose.
    fn with_size(width: u32, height: u32) -> GpuSurface {
        GpuSurface::new(width, height).expect("a GPU adapter and device")
    }

    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn fill(&mut self, x: i32, y: i32, width: u32, height: u32, color: Rgba) {
        if width == 0 || height == 0 {
            return;
        }
        self.ops.push(Op {
            kind: KIND_FILL,
            x,
            y,
            width,
            height,
            color: pack(color),
            mask_offset: 0,
        });
    }

    fn blend_mask(
        &mut self,
        x: i32,
        y: i32,
        mask_width: u32,
        mask_height: u32,
        coverage: &[u8],
        color: Rgba,
    ) {
        if mask_width == 0 || mask_height == 0 {
            return;
        }
        let mask_offset = self.coverage.len() as u32;
        self.coverage.extend_from_slice(coverage);
        self.ops.push(Op {
            kind: KIND_BLEND,
            x,
            y,
            width: mask_width,
            height: mask_height,
            color: pack(color),
            mask_offset,
        });
    }

    fn read_pixels(&mut self) -> Vec<u8> {
        self.execute().expect("the GPU replayed the recorded frame")
    }
}

const SHADER: &str = r#"
struct Params {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    color: u32,
    mask_offset: u32,
    surface_width: u32,
    surface_height: u32,
}

@group(0) @binding(0) var<storage, read_write> pixels: array<u32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read> coverage: array<u32>;

fn pixel_offset(gid: vec3<u32>) -> i32 {
    let px = params.x + i32(gid.x);
    let py = params.y + i32(gid.y);
    if (px < 0 || py < 0 || px >= i32(params.surface_width) || py >= i32(params.surface_height)) {
        return -1;
    }
    return py * i32(params.surface_width) + px;
}

@compute @workgroup_size(8, 8, 1)
fn fill(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.width || gid.y >= params.height) { return; }
    let offset = pixel_offset(gid);
    if (offset < 0) { return; }
    pixels[u32(offset)] = params.color;
}

@compute @workgroup_size(8, 8, 1)
fn blend(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= params.width || gid.y >= params.height) { return; }

    let index = params.mask_offset + gid.y * params.width + gid.x;
    let word = coverage[index / 4u];
    let alpha = (word >> ((index % 4u) * 8u)) & 255u;
    if (alpha == 0u) { return; }

    let offset = pixel_offset(gid);
    if (offset < 0) { return; }

    if (alpha == 255u) {
        pixels[u32(offset)] = params.color;
        return;
    }

    // The same integer expression as the CPU reference, rounded rather than truncated, so a
    // full-coverage blend of X over X is X. This is where bit-exactness is won or lost.
    let dst = pixels[u32(offset)];
    var out: u32 = 255u << 24u;
    for (var channel: u32 = 0u; channel < 3u; channel = channel + 1u) {
        let source = (params.color >> (channel * 8u)) & 255u;
        let destination = (dst >> (channel * 8u)) & 255u;
        let value = (source * alpha + destination * (255u - alpha) + 127u) / 255u;
        out = out | (value << (channel * 8u));
    }
    pixels[u32(offset)] = out;
}
"#;
