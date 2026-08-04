//! Purpose: putting a rendered frame on a screen-shaped target without a readback.
//! Public surface: `Blitter`, `PresentError`.
//! Why this file: `gpu.rs` says a window, a swapchain and a surface format are the embedder's
//!   problem. This is the half of that problem which can be checked by a machine. A `Blitter`
//!   copies the pixel buffer straight into a render target on the GPU, so the frame reaches a
//!   screen without ever crossing to the CPU - today every drawn frame is copied back purely
//!   so AppKit can wrap it in a CGImage, 12.5 MB at 2240x1400.
//! NOT responsible for: owning a window, configuring a swapchain, or resizing one. Those need
//!   an OS window and can only be proven by looking at it; keeping them out of here is what
//!   lets the part that fails SILENTLY be tested headlessly.
//! Test strategy: `tests/present.rs` blits into an offscreen texture, reads it back, and
//!   requires byte equality with `read_pixels` on the same frame. The fixture is deliberately
//!   asymmetric in red versus blue, because a grey fixture cannot fail under a channel swap.
//!
//! **The sRGB trap is refused, not handled.** The pixel buffer already holds non-linear sRGB
//! bytes - that is what the CPU canvas produces and what the differential oracle pins. A render
//! target whose format reports `is_srgb()` would treat those bytes as LINEAR and gamma-encode
//! them again, which washes every colour out and looks like a design choice rather than a bug
//! (paid for on 2026-05-18 in another project; near-black rendered as medium grey). Rather than
//! silently compensating, `Blitter::new` REFUSES such a format. A caller that hands us one has
//! misconfigured its swapchain and needs to hear about it at construction, not at first light.

use crate::gpu::{GpuContext, GpuSurface};

#[derive(Debug, thiserror::Error)]
pub enum PresentError {
    /// The target would gamma-encode already-encoded bytes. See the module note.
    #[error(
        "render target format {0:?} is sRGB; the pixel buffer is already sRGB-encoded and would \
         be gamma-encoded twice. Configure the swapchain with the non-sRGB view format."
    )]
    SrgbTarget(wgpu::TextureFormat),

    #[error("creating a surface for the window failed: {0}")]
    Surface(String),

    /// The device we rasterize on cannot drive this window.
    ///
    /// Checked rather than assumed. The context is built before a window exists, so on a
    /// machine with more than one GPU the adapter that rasterizes may not be the one wired to
    /// the display. Discovering that as a blank window instead of an error is the failure this
    /// refuses to allow.
    #[error("the adapter this context rasterizes on cannot present to that window")]
    IncompatibleAdapter,

    #[error("the window reported no supported texture format")]
    NoFormat,

    #[error("acquiring the next frame from the window failed: {0}")]
    Acquire(String),
}

/// The uniform the fragment shader reads its bounds from.
///
/// Padded to 16 bytes because a uniform block's size is rounded up to that anyway; writing the
/// padding out keeps the Rust struct and the WGSL struct the same shape.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Bounds {
    width: u32,
    height: u32,
    _pad0: u32,
    _pad1: u32,
}

/// Copies a `GpuSurface`'s pixels into a render target, on the GPU.
///
/// One pipeline per target format, because a render pipeline is compiled against its format.
/// A window that changes format (moving between displays) rebuilds its blitter; a resize does
/// not, since size is a uniform rather than a pipeline constant.
#[derive(Debug)]
pub struct Blitter {
    context: GpuContext,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
}

impl Blitter {
    pub fn new(context: &GpuContext, format: wgpu::TextureFormat) -> Result<Blitter, PresentError> {
        if format.is_srgb() {
            return Err(PresentError::SrgbTarget(format));
        }

        let device = context.device();

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ruuah-vt blit"),
            source: wgpu::ShaderSource::Wgsl(BLIT_SHADER.into()),
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ruuah-vt blit"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<Bounds>() as u64),
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ruuah-vt blit"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ruuah-vt blit"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // REPLACE, not a blend. This is a copy: the target's previous contents are
                    // not part of the result, and blending here would make the frame depend on
                    // whatever the swapchain handed back, which is undefined.
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Ok(Blitter {
            context: context.clone(),
            pipeline,
            layout,
            format,
        })
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// Draws `surface`'s pixels into `target`.
    ///
    /// Flushes the surface first: the recorded operations must have RUN, or this samples a
    /// buffer still holding the previous frame. That is the whole difference between presenting
    /// and reading back - the work has to land either way, only the copy is optional.
    ///
    /// The target may be larger than the surface; fragments outside it are discarded, so a
    /// window mid-resize shows cleared black in the uncovered region instead of sampling past
    /// the end of the buffer.
    pub fn blit(&self, surface: &mut GpuSurface, target: &wgpu::TextureView) {
        surface.flush();

        let device = self.context.device();

        let bounds = Bounds {
            width: crate::surface::Surface::width(surface),
            height: crate::surface::Surface::height(surface),
            _pad0: 0,
            _pad1: 0,
        };
        let uniform = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("blit bounds"),
                contents: bytemuck::bytes_of(&bounds),
                usage: wgpu::BufferUsages::UNIFORM,
            },
        );

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ruuah-vt blit"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: surface.pixel_buffer().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });

        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("blit") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ruuah-vt blit"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Cleared rather than loaded: a discarded fragment outside the surface
                        // must be a defined colour, and LOAD on a freshly acquired swapchain
                        // texture reads undefined content.
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            // One oversized triangle rather than two: no vertex buffer, and no seam down the
            // diagonal where two triangles would meet.
            pass.draw(0..3, 0..1);
        }
        self.context.queue().submit(Some(encoder.finish()));
    }
}

/// A window's swapchain, plus the blitter that fills it.
///
/// This is the half that CANNOT be checked headlessly: it needs a real layer, a real display and
/// a real compositor. Everything that fails silently was kept out of it deliberately - what is
/// left here fails loudly (no adapter, no format, a lost swapchain) or is visible at a glance
/// (nothing on screen, wrong size).
#[derive(Debug)]
pub struct WindowTarget {
    context: GpuContext,
    surface: wgpu::Surface<'static>,
    blitter: Blitter,
    /// The format the swapchain is configured with, which may still be sRGB.
    config_format: wgpu::TextureFormat,
    /// The format the VIEW is created with, always non-sRGB. When the two differ, the swapchain
    /// stores sRGB-tagged texels and we write through a linear view, so the hardware performs no
    /// conversion on our already-encoded bytes.
    view_format: wgpu::TextureFormat,
    width: u32,
    height: u32,
}

impl WindowTarget {
    /// Builds a swapchain over a `CAMetalLayer`.
    ///
    /// # Safety
    /// `layer` must be a live `CAMetalLayer` that outlives this `WindowTarget`. Nothing here can
    /// check that; it is the caller's contract, and the reason this is the only unsafe entry
    /// point in the module.
    #[cfg(target_os = "macos")]
    pub unsafe fn from_metal_layer(
        context: &GpuContext,
        layer: *mut std::ffi::c_void,
        width: u32,
        height: u32,
    ) -> Result<WindowTarget, PresentError> {
        let surface = unsafe {
            context
                .instance()
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(layer))
        }
        .map_err(|error| PresentError::Surface(error.to_string()))?;

        WindowTarget::from_surface(context, surface, width, height)
    }

    fn from_surface(
        context: &GpuContext,
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
    ) -> Result<WindowTarget, PresentError> {
        // Asked, not assumed. The context was built before any window existed, so this is the
        // first moment the pairing can be checked at all.
        if !context.adapter().is_surface_supported(&surface) {
            return Err(PresentError::IncompatibleAdapter);
        }

        let capabilities = surface.get_capabilities(context.adapter());
        let preferred = *capabilities.formats.first().ok_or(PresentError::NoFormat)?;
        let linear = preferred.remove_srgb_suffix();

        // Prefer configuring the swapchain itself as linear. When the platform only offers the
        // sRGB form, keep it and write through a linear VIEW instead - `view_formats` is what
        // makes that legal, and the effect is identical: no conversion touches our bytes.
        let (config_format, view_formats) = if capabilities.formats.contains(&linear) {
            (linear, Vec::new())
        } else {
            (preferred, vec![linear])
        };

        // Alpha mode is decided in `configure`, which is also the path a resize takes, so the
        // choice cannot drift between the first configuration and every later one.
        let blitter = Blitter::new(context, linear)?;

        let target = WindowTarget {
            context: context.clone(),
            surface,
            blitter,
            config_format,
            view_format: linear,
            width: width.max(1),
            height: height.max(1),
        };
        target.configure(&view_formats);
        Ok(target)
    }

    fn configure(&self, view_formats: &[wgpu::TextureFormat]) {
        self.surface.configure(
            self.context.device(),
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: self.config_format,
                width: self.width,
                height: self.height,
                // Vsync. Always supported, and a terminal has no reason to render faster than
                // the display can show it.
                present_mode: wgpu::PresentMode::Fifo,
                alpha_mode: if self
                    .surface
                    .get_capabilities(self.context.adapter())
                    .alpha_modes
                    .contains(&wgpu::CompositeAlphaMode::Opaque)
                {
                    wgpu::CompositeAlphaMode::Opaque
                } else {
                    wgpu::CompositeAlphaMode::Auto
                },
                view_formats: view_formats.to_vec(),
                desired_maximum_frame_latency: 2,
            },
        );
    }

    /// Reconfigures for a new size in PHYSICAL pixels.
    ///
    /// Physical, not points: handing this the point size on a Retina display configures a
    /// swapchain at half resolution and the result looks soft rather than broken, which is the
    /// kind of wrong that ships.
    pub fn resize(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if width == self.width && height == self.height {
            return;
        }
        self.width = width;
        self.height = height;
        let view_formats = if self.config_format == self.view_format {
            Vec::new()
        } else {
            vec![self.view_format]
        };
        self.configure(&view_formats);
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn view_format(&self) -> wgpu::TextureFormat {
        self.view_format
    }

    /// Draws the surface into the window's next frame and presents it.
    ///
    /// A lost or outdated swapchain is reconfigured and skipped for one frame rather than
    /// treated as fatal: it happens routinely on resize and display changes, and the next call
    /// succeeds.
    pub fn present(&mut self, surface: &mut GpuSurface) -> Result<(), PresentError> {
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                let view_formats = if self.config_format == self.view_format {
                    Vec::new()
                } else {
                    vec![self.view_format]
                };
                self.configure(&view_formats);
                return Ok(());
            }
            Err(error) => return Err(PresentError::Acquire(error.to_string())),
        };

        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.view_format),
            ..Default::default()
        });
        self.blitter.blit(surface, &view);
        frame.present();
        Ok(())
    }
}

const BLIT_SHADER: &str = r#"
struct Bounds {
    width: u32,
    height: u32,
    pad0: u32,
    pad1: u32,
}

@group(0) @binding(0) var<storage, read> pixels: array<u32>;
@group(0) @binding(1) var<uniform> bounds: Bounds;

// One triangle covering the whole clip volume: (-1,-1), (3,-1), (-1,3).
@vertex
fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let x = f32(i32(index & 1u) * 4 - 1);
    let y = f32(i32(index >> 1u) * 4 - 1);
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // position is in framebuffer space, origin top-left, which is the orientation the pixel
    // buffer is already stored in. No flip, deliberately: flipping here and flipping again in
    // a window is how an image ends up upside down on exactly one platform.
    let x = u32(position.x);
    let y = u32(position.y);
    if (x >= bounds.width || y >= bounds.height) {
        discard;
    }
    let value = pixels[y * bounds.width + x];
    // Red is the LOW byte, matching `pack` in gpu.rs and the CPU canvas byte order. Reversing
    // these four lines is the channel-swap bug the asymmetric test fixture exists to catch.
    let r = f32(value & 0xffu) / 255.0;
    let g = f32((value >> 8u) & 0xffu) / 255.0;
    let b = f32((value >> 16u) & 0xffu) / 255.0;
    let a = f32((value >> 24u) & 0xffu) / 255.0;
    return vec4<f32>(r, g, b, a);
}
"#;
