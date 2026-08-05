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
    /// Where the surface's top-left lands in the TARGET, in physical pixels.
    ///
    /// Zero for every caller that fills a window with one terminal. A non-zero origin is what
    /// lets chrome own a strip of the window - the terminal starts below it instead of being
    /// covered by it, which is the difference between reserving space and hiding rows. Rides
    /// the two words the uniform was padding out anyway, so the block did not grow.
    origin_x: u32,
    origin_y: u32,
}

/// A solid rectangle painted over the composited panes - the rule between them.
///
/// Straight sRGB bytes, like every other colour that crosses this boundary, and the target is
/// non-sRGB by construction (see the module card), so what is asked for is byte-for-byte what
/// lands. That is what lets a test assert the divider's colour rather than approximately match it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fill {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub color: [u8; 4],
}

/// The uniform the solid shader reads. The rect leads, so the colour lands on the 16-byte
/// boundary a `vec4<f32>` requires - reordering these fields silently misaligns the block.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Solid {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    color: [f32; 4],
}

/// Copies a `GpuSurface`'s pixels into a render target, on the GPU.
///
/// One pipeline per target format, because a render pipeline is compiled against its format.
/// A window that changes format (moving between displays) rebuilds its blitter; a resize does
/// not, since size is a uniform rather than a pipeline constant.
///
/// Two pipelines, not one: panes come from a pixel buffer and dividers are a flat colour, and a
/// flat colour drawn through the pane pipeline would need a pixel buffer the size of the rule
/// just to say one colour N times.
#[derive(Debug)]
pub struct Blitter {
    context: GpuContext,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    solid_pipeline: wgpu::RenderPipeline,
    solid_layout: wgpu::BindGroupLayout,
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

        let solid_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ruuah-vt solid"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<Solid>() as u64),
                },
                count: None,
            }],
        });

        let solid_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("ruuah-vt solid"),
                bind_group_layouts: &[&solid_layout],
                push_constant_ranges: &[],
            });

        let solid_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ruuah-vt solid"),
            layout: Some(&solid_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("solid_fragment"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // REPLACE, exactly as the pane path does: the divider is opaque chrome, not a
                    // tint over whatever the pane underneath happened to draw.
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
            solid_pipeline,
            solid_layout,
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
    pub fn blit(
        &self,
        surface: &mut GpuSurface,
        target: &wgpu::TextureView,
        clear: wgpu::Color,
        origin: (u32, u32),
    ) {
        self.blit_all(&mut [(surface, origin)], &[], target, clear);
    }

    /// Draws SEVERAL surfaces into one target: one clear, one pass, one submit.
    ///
    /// This is the whole of B3's compositing, and the reason it is not a loop over [`blit`] is a
    /// silent failure: that method's pass CLEARS the target on entry, so N calls leave only the
    /// last pane on screen and the rest are erased by the neighbour drawn after them. Nothing
    /// errors, and a two-pane window shows one pane and a field of clear colour - which reads as
    /// "the second terminal did not start" rather than as a compositing bug.
    ///
    /// Panes are drawn in order and are expected to be disjoint; the layout tree above is what
    /// guarantees that, and where they do overlap the later one wins, which is ordinary painter's
    /// order rather than a special case.
    /// `fills` are painted over the panes, in order, after every pane has landed - the dividers.
    pub fn blit_all(
        &self,
        panes: &mut [(&mut GpuSurface, (u32, u32))],
        fills: &[Fill],
        target: &wgpu::TextureView,
        clear: wgpu::Color,
    ) {
        let device = self.context.device();

        // Every surface is flushed BEFORE any bind group is built. Flushing inside the draw loop
        // would be correct too, but this keeps the GPU work of the panes together and the CPU
        // work of recording them together, which is what makes the whole frame one submit.
        let mut bind_groups = Vec::with_capacity(panes.len());
        for (surface, origin) in panes.iter_mut() {
            surface.flush();

            let bounds = Bounds {
                width: crate::surface::Surface::width(*surface),
                height: crate::surface::Surface::height(*surface),
                origin_x: origin.0,
                origin_y: origin.1,
            };
            let uniform = wgpu::util::DeviceExt::create_buffer_init(
                device,
                &wgpu::util::BufferInitDescriptor {
                    label: Some("blit bounds"),
                    contents: bytemuck::bytes_of(&bounds),
                    usage: wgpu::BufferUsages::UNIFORM,
                },
            );

            bind_groups.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
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
            }));
        }

        let fill_groups: Vec<wgpu::BindGroup> = fills
            .iter()
            .map(|fill| {
                let solid = Solid {
                    x: fill.x,
                    y: fill.y,
                    width: fill.width,
                    height: fill.height,
                    color: [
                        f32::from(fill.color[0]) / 255.0,
                        f32::from(fill.color[1]) / 255.0,
                        f32::from(fill.color[2]) / 255.0,
                        f32::from(fill.color[3]) / 255.0,
                    ],
                };
                let uniform = wgpu::util::DeviceExt::create_buffer_init(
                    device,
                    &wgpu::util::BufferInitDescriptor {
                        label: Some("solid rect"),
                        contents: bytemuck::bytes_of(&solid),
                        usage: wgpu::BufferUsages::UNIFORM,
                    },
                );
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("ruuah-vt solid"),
                    layout: &self.solid_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 2,
                        resource: uniform.as_entire_binding(),
                    }],
                })
            })
            .collect();

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
                        //
                        // The colour is the caller's, not black. A grid rounds to whole cells,
                        // so a window is almost never an exact multiple of the surface and the
                        // remainder is visible margin. Clearing it to black while the terminal
                        // renders on 0x0d0d0d draws a hard band down the edge - operator-spotted
                        // 2026-08-04, and the readback path never had it because AppKit painted
                        // that margin with the terminal's own reported background.
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            for bind_group in &bind_groups {
                pass.set_bind_group(0, bind_group, &[]);
                // One oversized triangle rather than two: no vertex buffer, and no seam down the
                // diagonal where two triangles would meet.
                pass.draw(0..3, 0..1);
            }

            // The rules go LAST and in the SAME pass. Last because a divider is chrome and owns
            // its pixels outright; the same pass because a second pass means a second command
            // buffer, and this backend has already deadlocked once on Metal's 64-buffer pool
            // (slice 8) - the frame stays one submit no matter how many panes or rules it holds.
            if !fill_groups.is_empty() {
                pass.set_pipeline(&self.solid_pipeline);
                for bind_group in &fill_groups {
                    pass.set_bind_group(0, bind_group, &[]);
                    pass.draw(0..3, 0..1);
                }
            }
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
    /// Where the terminal's top-left sits in the window, in PHYSICAL pixels.
    ///
    /// Default zero. A host with chrome above the terminal sets it so the reserved strip is
    /// clear colour rather than covered terminal - the failure it prevents is the quiet one:
    /// with origin zero the top rows still render, the chrome draws over them, and the terminal
    /// looks correct while the child's first lines are permanently hidden.
    origin: (u32, u32),
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

    /// Builds a swapchain over anything wgpu can make a surface from - a `tao` or `winit`
    /// window, a Tauri window, a raw handle pair.
    ///
    /// Preferred over `from_metal_layer` where the caller has a real window type: wgpu creates
    /// and owns the `CAMetalLayer` itself on macOS, and the same call is the Windows and Linux
    /// path, so nothing platform-specific has to be written twice. The metal-layer entry point
    /// stays for the AppKit host, which owns its layer already.
    ///
    /// Sizes are PHYSICAL pixels.
    pub fn from_window(
        context: &GpuContext,
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
    ) -> Result<WindowTarget, PresentError> {
        let surface = context
            .instance()
            .create_surface(target)
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
            origin: (0, 0),
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

    /// Places the terminal's top-left inside the window, in PHYSICAL pixels.
    ///
    /// The region outside the surface takes the clear colour `present` is given, so reserving a
    /// strip for chrome costs nothing extra and the seam wears the terminal's own background.
    pub fn set_origin(&mut self, x: u32, y: u32) {
        self.origin = (x, y);
    }

    pub fn origin(&self) -> (u32, u32) {
        self.origin
    }

    /// Draws the surface into the window's next frame and presents it.
    ///
    /// A lost or outdated swapchain is reconfigured and skipped for one frame rather than
    /// treated as fatal: it happens routinely on resize and display changes, and the next call
    /// succeeds.
    /// `clear` is straight RGBA bytes, the same encoding the pixel buffer uses - not a
    /// `wgpu::Color`. The embedder handing it in is the host, which knows the terminal's own
    /// background and should not have to depend on wgpu to say so.
    pub fn present(
        &mut self,
        surface: &mut GpuSurface,
        clear: [u8; 4],
    ) -> Result<(), PresentError> {
        let clear = wgpu::Color {
            r: f64::from(clear[0]) / 255.0,
            g: f64::from(clear[1]) / 255.0,
            b: f64::from(clear[2]) / 255.0,
            a: f64::from(clear[3]) / 255.0,
        };
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
        self.blitter.blit(surface, &view, clear, self.origin);
        frame.present();
        Ok(())
    }

    /// Presents SEVERAL surfaces as one frame, each at its own origin.
    ///
    /// The window's own `origin` is not used here: with one terminal it is what reserves the
    /// chrome strip, but with a layout tree every pane's position comes from its rect, and the
    /// strip is already accounted for in those rects. Two sources for one number is how a pane
    /// ends up offset by exactly the strip height.
    ///
    /// One acquire, one clear, one present, however many panes. Calling [`present`] per pane
    /// would acquire and present N times for a single frame - which is not tiling, it is N
    /// frames racing to the display, and it looks like tearing rather than like a bug.
    pub fn present_all(
        &mut self,
        panes: &mut [(&mut GpuSurface, (u32, u32))],
        fills: &[Fill],
        clear: [u8; 4],
    ) -> Result<(), PresentError> {
        let clear = wgpu::Color {
            r: f64::from(clear[0]) / 255.0,
            g: f64::from(clear[1]) / 255.0,
            b: f64::from(clear[2]) / 255.0,
            a: f64::from(clear[3]) / 255.0,
        };
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
        self.blitter.blit_all(panes, fills, &view, clear);
        frame.present();
        Ok(())
    }
}

const BLIT_SHADER: &str = r#"
struct Bounds {
    width: u32,
    height: u32,
    origin_x: u32,
    origin_y: u32,
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
    // Outside the surface's rect in the target - above or left of the origin, or past the
    // surface's extent - is the caller's clear colour, not a wrapped sample. Unsigned
    // arithmetic makes the first two comparisons load-bearing: without them, a fragment above
    // the origin underflows to an enormous coordinate and reads whatever that indexes.
    if (x < bounds.origin_x || y < bounds.origin_y) {
        discard;
    }
    let sx = x - bounds.origin_x;
    let sy = y - bounds.origin_y;
    if (sx >= bounds.width || sy >= bounds.height) {
        discard;
    }
    let value = pixels[sy * bounds.width + sx];
    // Red is the LOW byte, matching `pack` in gpu.rs and the CPU canvas byte order. Reversing
    // these four lines is the channel-swap bug the asymmetric test fixture exists to catch.
    let r = f32(value & 0xffu) / 255.0;
    let g = f32((value >> 8u) & 0xffu) / 255.0;
    let b = f32((value >> 16u) & 0xffu) / 255.0;
    let a = f32((value >> 24u) & 0xffu) / 255.0;
    return vec4<f32>(r, g, b, a);
}

struct Solid {
    rect: vec4<u32>,
    color: vec4<f32>,
}

// Binding 2, not 0: one shader module holds both entry points, and two variables cannot share
// @group(0) @binding(0) with different types. Reflection is per entry point, so the pane path's
// bindings are simply unused here.
@group(0) @binding(2) var<uniform> solid: Solid;

// Shares the vertex entry point above - the same oversized triangle, clipped per fragment. The
// rect could have been expressed as vertex positions instead, which would rasterize fewer
// fragments; it is not, because that means clip-space arithmetic in a second place and this
// shader's whole job is to agree, pixel for pixel, with the layout that emitted the rect.
@fragment
fn solid_fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let x = u32(position.x);
    let y = u32(position.y);
    // Kept for symmetry with the blit path, and measured to be belt-and-braces rather than
    // load-bearing (2026-08-06): an underflowed coordinate is enormous, so the extent check below
    // discards it on its own. Removing this guard was run as a mutant and the frame was unchanged.
    // The comparison that IS load-bearing is the `>=` below - as `>` it paints one pixel past the
    // rect, over a column the pane beneath is writing into, and the test sees it.
    if (x < solid.rect.x || y < solid.rect.y) {
        discard;
    }
    if (x - solid.rect.x >= solid.rect.z || y - solid.rect.y >= solid.rect.w) {
        discard;
    }
    return solid.color;
}
"#;
