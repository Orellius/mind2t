//! Purpose: THE ORACLE. A live terminal in a raw `tao` + `wry` window - the shell Bindary was
//!   built on before Tauri, kept working so the Tauri host can be compared against something
//!   that is known to be right.
//! Why it survived the Tauri decision: the same reason the Swift host survived this one being
//!   written. A port with no reference is a rewrite with extra steps; when the Tauri window
//!   misbehaves, the question "does the pair do this too?" is answerable in one command
//!   (`cargo run --bin probe`) instead of by argument. It retires when Tauri reaches parity.
//!
//! Original card follows.
//! Purpose: B2.3 - a LIVE terminal in the Bindary window, with the chrome webview above it.
//! Public surface: a binary. Nothing links against it.
//! Why this file: B2.1 proved a GPU surface presents in a `tao` window; B2.2 proved a
//!   transparent `wry` webview composites over it and measured where input lands. Neither ran a
//!   shell. This one does: `Session` (pty -> core -> renderer) presents into the window under a
//!   chrome strip that owns the top of the window rather than covering it.
//! NOT responsible for: tabs, splits, palettes, agents, or anything Bindary will actually ship.
//!   The chrome page is still a fixture. Also not responsible for the modes the key encoder
//!   branches on - see `keys::options`, a named gap.
//! Test strategy: the parts that can be checked by a machine are checked where they live -
//!   `ruuah_vt_host::session` for the pipeline, `crates/render/tests/present.rs` for the origin
//!   that reserves the strip, `keys` for the W3C bridge. What remains here is the part only a
//!   human can judge: a shell that answers, at the right size, under the chrome.
//!
//! THE LAYOUT RULE. The chrome strip RESERVES its height; it does not overlap the terminal. The
//! terminal's surface is placed at `origin = (0, strip)` and the grid is derived from the space
//! that is left. Overlapping instead would look identical at a glance and permanently hide the
//! child's top rows - which is precisely the class of defect B1 kept producing.

use bindary::{clipboard, keys};

use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use muda::{Menu, PredefinedMenuItem, Submenu};
use ruuah_vt_host::session::{Session, SessionError, SessionGeometry};
use ruuah_vt_render::WindowTarget;
use tao::dpi::LogicalSize;
use tao::event::{ElementState, Event, MouseScrollDelta, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::keyboard::{KeyCode, ModifiersState};
use tao::window::WindowBuilder;
use wry::dpi::{LogicalPosition, LogicalSize as WebviewSize};
use wry::{Rect, WebViewBuilder};

const CHROME: &str = include_str!("../chrome.html");

/// Logical height of the chrome strip, matching `#bar` in the page.
const BAR_HEIGHT: f64 = 44.0;

/// Logical point size. The session is built at `FONT_SIZE * scale` so one buffer pixel is one
/// DEVICE pixel: handing the renderer the point size on a Retina display rasterizes at half
/// resolution, and the result is soft rather than broken - the kind of wrong that ships. Slice
/// 8's Swift host applies the same rule, and this is the second place to learn it.
const FONT_SIZE: f32 = 16.0;

/// How often the terminal is polled while nothing else happens.
///
/// The pump publishes frames on its own thread; this is only how often we look. 8ms is under a
/// 120Hz frame, so a keystroke's echo is never waiting on the clock.
const POLL_INTERVAL: Duration = Duration::from_millis(8);

/// Installs the standard macOS app and Edit menus.
///
/// Measured on 2026-08-04: with no menu, cmd+A in a webview text field DELIVERS a keydown and
/// selects nothing, because AppKit routes editing key equivalents through the main menu and a
/// WKWebView only performs one when a menu item claims it. Nothing errors and nothing logs.
fn install_menus() -> Result<Menu, muda::Error> {
    let menu = Menu::new();

    // The FIRST submenu is the application menu on macOS, whatever it is called; Edit must come
    // after it or the standard items land in the wrong place.
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

/// The grid that fits the window below the chrome strip.
///
/// Floors deliberately: a partial row is not a row, and rounding one up gives the child a line
/// it cannot fully draw. The leftover pixels become margin, which `present` clears with the
/// terminal's own background.
fn grid_for(width: u32, height: u32, strip: u32, cell_width: u32, cell_height: u32) -> SessionGeometry {
    let usable = height.saturating_sub(strip);
    SessionGeometry {
        cols: (width / cell_width.max(1)).max(1) as u16,
        rows: (usable / cell_height.max(1)).max(1) as u16,
    }
}

/// The user's shell, or `/bin/sh` when the environment does not say.
///
/// `-l` is deliberately NOT passed: a login shell re-runs the profile chain, which on this
/// machine takes long enough to be visible and is not what a pane inside a workbench wants.
fn shell() -> Command {
    let path = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    Command::new(path)
}

fn main() {
    let event_loop = EventLoop::new();
    // After the event loop, never before: `init_for_nsapp` needs the NSApplication that
    // `EventLoop::new` creates. Held for the process lifetime - dropping the menu unhooks it.
    let _menu = install_menus().expect("the app and edit menus");

    let window = Arc::new(
        WindowBuilder::new()
            .with_title("Bindary")
            .with_inner_size(LogicalSize::new(900.0, 560.0))
            .build(&event_loop)
            .expect("a window"),
    );

    let scale = window.scale_factor();
    let physical = window.inner_size();
    let strip = (BAR_HEIGHT * scale) as u32;

    // A session at a provisional grid: the real one needs the cell metrics, and the cell
    // metrics need a font, which the session owns. One resize after construction settles it,
    // and the resize path is the same one every window resize takes - so the first frame
    // exercises the code every later frame depends on, rather than a special case.
    let mut session = match Session::spawn(
        shell(),
        SessionGeometry { cols: 80, rows: 24 },
        FONT_SIZE * scale as f32,
        None,
    ) {
        Ok(session) => session,
        Err(error) => {
            eprintln!("bindary: no session: {error:?}");
            std::process::exit(1);
        }
    };

    let cell = session.cell_metrics();
    let geometry = grid_for(
        physical.width,
        physical.height,
        strip,
        cell.width,
        cell.height,
    );
    if let Err(error) = session.resize(geometry) {
        eprintln!("bindary: initial resize refused: {error:?}");
    }

    // The GPU surface FIRST, the webview second. On macOS wgpu creates the CAMetalLayer here and
    // AppKit stacks the newest sublayer on top, so a webview added after this one lands above
    // it. Reversed, the terminal covers the chrome and nothing errors - dioxus#3727.
    let mut target = match WindowTarget::from_window(
        session.context(),
        window.clone(),
        physical.width.max(1),
        physical.height.max(1),
    ) {
        Ok(target) => target,
        Err(error) => {
            eprintln!("bindary: no swapchain: {error}");
            std::process::exit(1);
        }
    };
    // Reserve, do not overlap. See the module card.
    target.set_origin(0, strip);
    session.attach(target);

    let logical = physical.to_logical::<f64>(scale);
    let webview = WebViewBuilder::new()
        .with_transparent(true)
        .with_html(CHROME)
        .with_bounds(Rect {
            position: LogicalPosition::new(0.0, 0.0).into(),
            size: WebviewSize::new(logical.width, BAR_HEIGHT).into(),
        })
        .with_accept_first_mouse(true)
        .with_ipc_handler(|request| println!("bindary: chrome {}", request.body()))
        .build_as_child(&window)
        .expect("a child webview over the window");

    println!(
        "bindary: {}x{} cells at {}x{}px, strip {strip}px, scale {scale}",
        geometry.cols, geometry.rows, cell.width, cell.height
    );

    let mut modifiers = ModifiersState::empty();

    event_loop.run(move |event, _, control_flow| {
        // Time-based rather than `Poll`: `Poll` spins a core for a terminal that is usually
        // idle, and `Wait` alone would leave a running command's output on screen only when
        // some other event happened to wake the loop.
        *control_flow = ControlFlow::WaitUntil(Instant::now() + POLL_INTERVAL);

        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control_flow = ControlFlow::Exit,

            Event::WindowEvent {
                event: WindowEvent::ModifiersChanged(state),
                ..
            } => modifiers = state,

            Event::WindowEvent {
                event: WindowEvent::KeyboardInput { event, .. },
                ..
            } => {
                if event.state == ElementState::Pressed {
                    // cmd+V is intercepted BEFORE encoding: the clipboard is the host's to read,
                    // and the child must receive the text, not the chord.
                    if modifiers.super_key() && event.physical_key == KeyCode::KeyV {
                        clipboard::paste(&session);
                        return;
                    }
                    // The options come from the SESSION every press, because the child changes
                    // them at runtime - entering vim turns application cursor keys on, and an
                    // arrow encoded against a stale snapshot moves nothing.
                    let bytes =
                        keys::encode_press(&event, keys::mods_from(modifiers), &session.key_options());
                    if !bytes.is_empty() {
                        if let Err(error) = session.send(&bytes) {
                            eprintln!("bindary: send failed: {error:?}");
                        }
                    }
                }
            }

            // A click that reached the WINDOW landed on the terminal, not on the chrome strip
            // (the strip is a webview and would have swallowed it). Taking focus here is what
            // sends the keyboard back to the child after the operator has typed in the chrome -
            // without it the terminal is visibly focused and silently deaf.
            Event::WindowEvent {
                event: WindowEvent::MouseInput { state, .. },
                ..
            } => {
                if state == ElementState::Pressed {
                    window.set_focus();
                }
            }

            Event::WindowEvent {
                event: WindowEvent::MouseWheel { delta, .. },
                ..
            } => {
                // Viewport scroll only: the child sees nothing. Mouse REPORTING (a program that
                // asked to receive wheel events itself) is not wired yet and is named in the
                // module card - routing the wheel to both would be worse than routing it to one.
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    // A trackpad reports pixels; the cell height is what turns those into rows,
                    // and it is a physical measure exactly like the delta.
                    MouseScrollDelta::PixelDelta(position) => {
                        position.y as f32 / session.cell_metrics().height.max(1) as f32
                    }
                    _ => 0.0,
                };
                if lines != 0.0 {
                    session.scroll(lines.round() as i32);
                }
            }

            Event::WindowEvent {
                event: WindowEvent::Resized(size),
                ..
            } => {
                let (width, height) = (size.width.max(1), size.height.max(1));
                let scale = window.scale_factor();
                let strip = (BAR_HEIGHT * scale) as u32;

                if let Err(error) = session.resize_window(width, height) {
                    eprintln!("bindary: window resize refused: {error:?}");
                }
                let cell = session.cell_metrics();
                let geometry = grid_for(width, height, strip, cell.width, cell.height);
                if let Err(error) = session.resize(geometry) {
                    eprintln!("bindary: grid resize refused: {error:?}");
                }

                // The origin is re-applied because the strip is a function of the SCALE, and a
                // window dragged to a different display changes scale without changing its
                // logical size.
                session.set_origin(0, strip);

                let logical = size.to_logical::<f64>(scale);
                if let Err(error) = webview.set_bounds(Rect {
                    position: LogicalPosition::new(0.0, 0.0).into(),
                    size: WebviewSize::new(logical.width, BAR_HEIGHT).into(),
                }) {
                    eprintln!("bindary: webview set_bounds failed: {error}");
                }
                window.request_redraw();
            }

            Event::RedrawRequested(_) | Event::MainEventsCleared => {
                let drew = session.poll();
                if session.exited() {
                    *control_flow = ControlFlow::Exit;
                    return;
                }
                // Presenting only on a new frame is what keeps an idle terminal off the GPU.
                // A redraw request presents anyway: the window may have been uncovered.
                if drew || matches!(event, Event::RedrawRequested(_)) {
                    match session.present() {
                        Ok(()) => {}
                        // Loud, and fatal. A present that fails and falls back to another path
                        // is exactly how B1's four defects stayed invisible.
                        Err(SessionError::NoWindow) => unreachable!("a window is attached"),
                        Err(error) => {
                            eprintln!("bindary: present failed: {error:?}");
                            *control_flow = ControlFlow::Exit;
                        }
                    }
                }
            }

            _ => {}
        }
    });
}
