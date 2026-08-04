//! Purpose: B2.2 - prove a TRANSPARENT webview composites over the GPU surface in the same
//!   window, and MEASURE where mouse and keyboard events land once it does.
//! Public surface: a binary. Nothing links against it.
//! Why this file: B2.1 proved the surface presents in a window this project does not own. The
//!   remaining unknown in B2 is the seam between that surface and the chrome drawn above it -
//!   z-order, transparency, and event routing. Those three are what a Tauri host is made of,
//!   and the third one is where fidelity is lost silently (SCAR-014: a chord that worked under
//!   the harness died under a human's Hebrew layout).
//! NOT responsible for: being Bindary's chrome. The page is a fixture, not a design; there is
//!   still no terminal, no pty and no IPC protocol here.
//! Test strategy: two directions in one screenshot, then a live tap for the routing.
//!   - Z-ORDER AND TRANSPARENCY: the page's marker overlaps the GPU pattern's green bar at 75%
//!     alpha. Composited above, it tints that bar and the other bars stay readable through the
//!     page; composited below, the marker is simply absent. An opaque page hides everything and
//!     is distinguishable from both. No single "it drew" observation separates these.
//!   - ROUTING: every event is printed with the CHANNEL it arrived on - `webview` over IPC,
//!     `window` from tao. Both are printed for the same gesture, so "the webview swallowed it"
//!     is a reading of two logs rather than an assumption about one.
//!
//! CREATION ORDER IS LOAD-BEARING, and it is the first thing to check if the marker vanishes.
//! On macOS wgpu creates the `CAMetalLayer` when the surface is created, and AppKit stacks the
//! most recently added sublayer on top - so the surface must exist BEFORE the webview subview,
//! not after. wry's own `examples/wgpu.rs` builds in that order too. Reversed, the surface
//! covers the page and everything still "works": DioxusLabs/dioxus#3727 is that failure,
//! reported after wgpu 0.24 and open with no root cause. Verified against wry 0.56 / wgpu 26 on
//! 2026-08-04.

use std::sync::Arc;

use muda::{Menu, PredefinedMenuItem, Submenu};
use ruuah_vt_render::{GpuContext, GpuSurface, Surface, WindowTarget};
use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event::{ElementState, Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;
use wry::dpi::{LogicalPosition, LogicalSize as WebviewSize};
use wry::{Rect, WebViewBuilder};

const CHROME: &str = include_str!("chrome.html");

/// Logical height of the chrome strip, matching `#bar` in the page.
const BAR_HEIGHT: f64 = 44.0;

/// How much of the window the webview covers.
///
/// Selected by `BINDARY_OVERLAY` because the two regimes route input differently and the
/// difference is the finding. A webview covering the whole window is the composited design and
/// the one that can steal every event; a webview clipped to the chrome strip cannot receive an
/// event outside its own bounds by construction, which is a weaker design and a stronger
/// guarantee. Measuring both costs one env var and settles the question for B2.3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Coverage {
    Full,
    Bar,
}

impl Coverage {
    fn from_env() -> Coverage {
        match std::env::var("BINDARY_OVERLAY").as_deref() {
            Ok("bar") => Coverage::Bar,
            _ => Coverage::Full,
        }
    }

    /// Webview bounds in LOGICAL units. wry positions a child webview in points; handing it
    /// physical pixels on a Retina display produces a webview twice the intended size, which
    /// looks like a layout bug rather than a unit bug.
    fn bounds(self, logical_width: f64, logical_height: f64) -> Rect {
        let height = match self {
            Coverage::Full => logical_height,
            Coverage::Bar => BAR_HEIGHT.min(logical_height),
        };
        Rect {
            position: LogicalPosition::new(0.0, 0.0).into(),
            size: WebviewSize::new(logical_width, height).into(),
        }
    }
}

/// Installs the standard macOS app and Edit menus.
///
/// This is not chrome and it is not polish. Measured live on 2026-08-04: with no menu, cmd+A in
/// the page's text field DELIVERS a keydown (`Meta`, then `a`/`KeyA`) and selects nothing,
/// because AppKit routes an editing key equivalent through the main menu and a WKWebView only
/// performs it when a menu item claims it. Nothing errors, nothing logs, and the keyboard probe
/// reports the chord arriving - so the only way to see this is to use the app.
///
/// The first submenu is the APPLICATION menu on macOS, whatever it is called; Edit must come
/// after it or the standard items land in the wrong place.
fn install_menus() -> Result<Menu, muda::Error> {
    let menu = Menu::new();

    let app = Submenu::new("Bindary", true);
    app.append(&PredefinedMenuItem::quit(None))?;

    let edit = Submenu::new("Edit", true);
    edit.append_items(&[
        &PredefinedMenuItem::undo(None),
        &PredefinedMenuItem::redo(None),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::cut(None),
        &PredefinedMenuItem::copy(None),
        &PredefinedMenuItem::paste(None),
        &PredefinedMenuItem::select_all(None),
    ])?;

    menu.append_items(&[&app, &edit])?;
    #[cfg(target_os = "macos")]
    menu.init_for_nsapp();
    Ok(menu)
}

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

/// Marks a click that reached the NATIVE window, in the GPU surface itself.
///
/// The operator's report on the first tap was "I clicked, I have no feedback" - and he was
/// right: the routing was legible only in a log he could not see. A dot drawn by the layer
/// UNDER the webview answers both questions at once and needs no log: it exists, so the window
/// received the click, and it is visible, so the page above did not cover it.
fn mark_native_click(surface: &mut GpuSurface, at: PhysicalPosition<f64>) {
    const SIZE: u32 = 16;
    let x = at.x.max(0.0) as i32 - (SIZE as i32) / 2;
    let y = at.y.max(0.0) as i32 - (SIZE as i32) / 2;
    surface.fill(x, y, SIZE, SIZE, [255, 255, 255, 255]);
    surface.fill(x + 3, y + 3, SIZE - 6, SIZE - 6, [230, 60, 200, 255]);
}

fn main() {
    let coverage = Coverage::from_env();
    let event_loop = EventLoop::new();
    // After the event loop, never before: `init_for_nsapp` needs the NSApplication that
    // `EventLoop::new` creates. Held for the process lifetime - dropping the menu unhooks it.
    let _menu = install_menus().expect("the app and edit menus");
    // Arc, not a bare Window: the swapchain borrows the window for as long as it lives, and
    // `wgpu::Surface` is `'static`. Sharing ownership is how the window outlives it by
    // construction rather than by a promise nobody can check.
    let window = Arc::new(
        WindowBuilder::new()
            .with_title("Bindary B2.2 - webview over the GPU surface")
            .with_inner_size(LogicalSize::new(900.0, 560.0))
            .build(&event_loop)
            .expect("a window"),
    );

    let context = GpuContext::new().expect("a GPU");

    let scale = window.scale_factor();
    let physical = window.inner_size();
    let (width, height) = (physical.width.max(1), physical.height.max(1));

    // The window is handed to wgpu whole. On macOS wgpu creates and owns the CAMetalLayer,
    // which is the point of `from_window` over the AppKit path: one call covers Windows and
    // Linux too, so the platform seam is wgpu's rather than ours.
    //
    // FIRST. See the module card - the webview must be added after this, or it lands under it.
    let mut target = WindowTarget::from_window(&context, window.clone(), width, height)
        .expect("a swapchain over the tao window");

    let mut surface =
        GpuSurface::with_context(context.clone(), width, height).expect("a render surface");
    paint(&mut surface);

    let logical = physical.to_logical::<f64>(scale);
    let webview = WebViewBuilder::new()
        .with_transparent(true)
        .with_html(CHROME)
        .with_bounds(coverage.bounds(logical.width, logical.height))
        // A click on an unfocused window must not be eaten by the focus change - otherwise the
        // first gesture of every session disappears and the log reads as a routing failure that
        // is really a focus rule.
        .with_accept_first_mouse(true)
        .with_ipc_handler(|request| println!("bindary: {}", request.body()))
        .build_as_child(&window)
        .expect("a child webview over the window");

    println!(
        "bindary: window {width}x{height} physical at scale {scale}, view format {:?}, overlay {:?}",
        target.view_format(),
        coverage
    );
    println!("bindary: LIVE TAP - 1) click the magenta marker  2) click a bare GPU bar");
    println!("bindary:            3) click 'tap me'  4) type in the box in English");
    println!("bindary:            5) switch to the Hebrew layout and type again");
    println!("bindary: every line below names the CHANNEL that received the gesture,");
    println!("bindary: and a click the WINDOW received is marked on screen where it landed.");

    let mut cursor = PhysicalPosition::new(0.0, 0.0);

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
                let (width, height) = (size.width.max(1), size.height.max(1));
                target.resize(width, height);
                // The rasterized surface is rebuilt to match, which is B1's lesson written
                // down: a swapchain that grows while the surface does not leaves the frame in
                // the top-left corner and the rest cleared, and it looks like a layout bug.
                surface = GpuSurface::with_context(context.clone(), width, height)
                    .expect("a resized render surface");
                paint(&mut surface);

                // The child webview has no autoresizing mask; nothing moves it but this. A
                // webview that keeps its old bounds still draws, so the failure is a chrome
                // strip that stops short of the window edge rather than anything that errors.
                let logical = size.to_logical::<f64>(window.scale_factor());
                if let Err(error) = webview.set_bounds(coverage.bounds(logical.width, logical.height))
                {
                    eprintln!("bindary: webview set_bounds failed: {error}");
                }
                window.request_redraw();
            }

            // The routing channels. Printed for the same gestures the page reports over IPC, so
            // the two logs together say who received what - and, more importantly, who did not.
            // tao's `MouseInput` carries no position, so the last `CursorMoved` is where the
            // click was. Kept as physical pixels because that is what the surface is indexed in.
            Event::WindowEvent {
                event: WindowEvent::CursorMoved { position, .. },
                ..
            } => cursor = position,

            Event::WindowEvent {
                event: WindowEvent::MouseInput { state, button, .. },
                ..
            } => {
                if state == ElementState::Pressed {
                    println!(
                        "bindary: {{\"ch\":\"window\",\"kind\":\"mousedown\",\"button\":\"{button:?}\",\"x\":{:.0},\"y\":{:.0}}}",
                        cursor.x, cursor.y
                    );
                    mark_native_click(&mut surface, cursor);
                    window.request_redraw();
                }
            }

            Event::WindowEvent {
                event: WindowEvent::KeyboardInput { event, .. },
                ..
            } => {
                if event.state == ElementState::Pressed {
                    println!(
                        "bindary: {{\"ch\":\"window\",\"kind\":\"keydown\",\"logical\":\"{:?}\",\"text\":{:?}}}",
                        event.logical_key, event.text
                    );
                }
            }

            Event::WindowEvent {
                event: WindowEvent::ReceivedImeText(text),
                ..
            } => println!("bindary: {{\"ch\":\"window\",\"kind\":\"ime\",\"text\":{text:?}}}"),

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
