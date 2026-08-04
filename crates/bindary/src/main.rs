//! Purpose: B2.1 - prove a native GPU surface can present inside a window this project does
//!   not own, before any of Bindary is built on that assumption.
//! Public surface: a binary. Nothing links against it.
//! Why this file: B1 proved presenting works into an AppKit window the Swift host created and
//!   controls. B2 rests on something different and unproven - a window owned by tao (and later
//!   Tauri), with a webview composited over it. Everything else in B2 is grind; this is the
//!   part that can be impossible. Isolated to one variable deliberately: no terminal, no pty,
//!   no webview yet, so a failure here means the window, and nothing else.
//! NOT responsible for: being Bindary. It grows into the host, but today it draws a fixed
//!   pattern and shows it.
//! Test strategy: run it and look. A window that draws is the whole assertion, and it cannot
//!   be made headless - which is exactly why it is worth spending a binary on.
//!
//! The pattern is deliberately asymmetric in red and blue, for the same reason
//! `tests/present.rs` is: a grey square proves the surface presented, and proves nothing about
//! whether it presented CORRECTLY.

use std::sync::Arc;

use ruuah_vt_render::{GpuContext, GpuSurface, Surface, WindowTarget};
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;

fn paint(surface: &mut GpuSurface) {
    let width = Surface::width(surface);
    let height = Surface::height(surface);

    // Terminal background, so a window that fills with this is already saying something.
    surface.fill(0, 0, width, height, [0x0d, 0x0d, 0x0d, 255]);

    // Three bars, left to right: red, green, blue. Their ORDER is the assertion - a channel
    // swap shows blue where red belongs, which no amount of "it drew" would catch.
    let bar = height / 6;
    let inset = width / 12;
    for (index, color) in [
        [200u8, 40, 10, 255],
        [20, 200, 60, 255],
        [10, 60, 220, 255],
    ]
    .into_iter()
    .enumerate()
    {
        let x = inset as i32 + (index as i32) * (inset as i32 * 3);
        surface.fill(x, bar as i32, inset * 2, bar * 3, color);
    }

    // A single white cell at the TOP-LEFT. Orientation is otherwise unfalsifiable: a
    // vertically flipped frame of three vertical bars looks identical.
    surface.fill(0, 0, 24, 24, [255, 255, 255, 255]);
}

fn main() {
    let event_loop = EventLoop::new();
    // Arc, not a bare Window: the swapchain borrows the window for as long as it lives, and
    // `wgpu::Surface` is `'static`. Sharing ownership is how the window outlives it by
    // construction rather than by a promise nobody can check.
    let window = Arc::new(
        WindowBuilder::new()
            .with_title("Bindary B2.1 - GPU surface in a tao window")
            .with_inner_size(LogicalSize::new(900.0, 560.0))
            .build(&event_loop)
            .expect("a window"),
    );

    let context = GpuContext::new().expect("a GPU");

    let scale = window.scale_factor();
    let physical = window.inner_size();
    let (mut width, mut height) = (physical.width.max(1), physical.height.max(1));

    // The window is handed to wgpu whole. On macOS wgpu creates and owns the CAMetalLayer,
    // which is the point of `from_window` over the AppKit path: one call covers Windows and
    // Linux too, so the platform seam is wgpu's rather than ours.
    let mut target = WindowTarget::from_window(&context, window.clone(), width, height)
        .expect("a swapchain over the tao window");

    let mut surface =
        GpuSurface::with_context(context.clone(), width, height).expect("a render surface");
    paint(&mut surface);

    println!(
        "bindary: window {width}x{height} physical at scale {scale}, view format {:?}",
        target.view_format()
    );

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control_flow = ControlFlow::Exit,

            Event::WindowEvent {
                event: WindowEvent::Resized(size),
                ..
            } => {
                width = size.width.max(1);
                height = size.height.max(1);
                target.resize(width, height);
                // The rasterized surface is rebuilt to match, which is B1's lesson written
                // down: a swapchain that grows while the surface does not leaves the frame in
                // the top-left corner and the rest cleared, and it looks like a layout bug.
                surface = GpuSurface::with_context(context.clone(), width, height)
                    .expect("a resized render surface");
                paint(&mut surface);
                window.request_redraw();
            }

            Event::RedrawRequested(_) | Event::MainEventsCleared => {
                if let Err(error) = target.present(&mut surface, [0x0d, 0x0d, 0x0d, 255]) {
                    // Loud. A present that fails silently is the failure mode this whole slice
                    // exists to avoid repeating.
                    eprintln!("bindary: present failed: {error}");
                    *control_flow = ControlFlow::Exit;
                }
            }

            _ => {}
        }
    });
}
