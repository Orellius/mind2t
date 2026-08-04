//! Purpose: B2.4 - Bindary's Tauri host. A Tauri window whose terminal is a native GPU surface
//!   and whose chrome is a React webview docked above it.
//! Public surface: a binary.
//! Why this file: Orel chose Tauri + React + TypeScript on 2026-08-04. Everything unusual here
//!   is downstream of one measurement and one law:
//!   - **Order.** B2.2 proved a wgpu surface created AFTER a webview lands on top of it on macOS
//!     (dioxus#3727). Tauri's ordinary `WebviewWindow` creates both at once, so this host uses
//!     `WindowBuilder` -> surface -> `Window::add_child(webview)`, which needs Tauri's `unstable`
//!     feature. That is the whole reason the flag is on.
//!   - **Keys.** `tauri::WindowEvent` has no keyboard variant (Resized, Moved, CloseRequested,
//!     Destroyed, Focused, ScaleFactorChanged, DragDrop, ThemeChanged - verified 2026-08-04), so
//!     Tauri cannot hand us the terminal's keystrokes. They must NOT come through the webview:
//!     project law 2 says no terminal bytes there, and routing keys through React would be
//!     xterm.js's mistake by another road. So this host reads `NSEvent` itself.
//! NOT responsible for: VT semantics, rendering, or the pipeline - `ruuah_vt_host::session` owns
//!   all of it. This file is a window, an event monitor, and a clock.
//! Test strategy: what can be checked by a machine is checked where it lives (session pipeline,
//!   blit origin, keycode tables, key encoding). What is left is a GUI and gets a live tap. The
//!   tao + wry oracle stays runnable as `cargo run --bin probe` for the question "does the pair
//!   do this too?".

use std::cell::{Cell, RefCell};
use std::process::Command;
use std::rc::Rc;

use ruuah_vt_host::session::{Session, SessionGeometry};
use ruuah_vt_render::WindowTarget;
use tauri::webview::WebviewBuilder;
use tauri::window::WindowBuilder;
use tauri::{Emitter, Listener, LogicalPosition, LogicalSize, Manager, RunEvent, WebviewUrl};

/// Logical height of the chrome strip. Must match the CSS that draws it.
const BAR_HEIGHT: f64 = 36.0;

/// Logical point size. The session is built at `FONT_SIZE * scale` so one buffer pixel is one
/// DEVICE pixel - see the project CLAUDE.md; this repo has learned it twice already.
const FONT_SIZE: f32 = 16.0;

const SESSION_EVENT: &str = "bindary://session";

/// Runs the whole host with NOTHING on screen: the window is created hidden, no key monitor is
/// installed, diagnostics are printed and the process exits on its own.
///
/// This is not a test double - it is the SAME host, with its window ordered out. That matters:
/// a headless mode that skipped the window would not exercise the swapchain, the origin or the
/// child webview, which are exactly the things being diagnosed. What it removes is the two
/// things that make a running window hostile to whoever owns the machine - it cannot take focus
/// and it cannot swallow a keystroke.
///
/// Enabled with `BINDARY_HEADLESS=1`.
const HEADLESS_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);

/// How long after the chrome announces itself before its DOM is read. One render plus the
/// replayed state; generous, because the cost of being early is a false failure.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(1200);

fn headless() -> bool {
    std::env::args().any(|argument| argument == "--smoke")
        || std::env::var("BINDARY_HEADLESS").is_ok_and(|value| value == "1")
}

/// What the smoke run learned, filled in as it happens and judged at the end.
#[derive(Default)]
struct Smoke {
    chrome_url: Option<String>,
    document: Option<String>,
    layers: Vec<String>,
    origin: Option<(u32, u32)>,
    grid: Option<(u16, u16)>,
}

impl Smoke {
    /// Every check, and what each one exists to catch. Returns whether all of them held.
    ///
    /// Each line is a defect this session actually hit, in the order it hit them. That is the
    /// point of writing them down as a gate rather than as a paragraph: the next person to touch
    /// the host layer finds out in seconds, headlessly, instead of by staring at a window.
    fn judge(&self, page_finished: bool, chrome_ready: bool) -> bool {
        let document = self.document.clone().unwrap_or_default();
        let grid = self
            .grid
            .map(|(cols, rows)| format!("{cols}x{rows}"))
            .unwrap_or_default();

        let z_ordered = self.layers.len() >= 2
            && self
                .layers
                .iter()
                .position(|layer| layer.contains("Wgpu"))
                .zip(self.layers.iter().rposition(|layer| !layer.contains("Wgpu")))
                .is_some_and(|(terminal, chrome)| terminal < chrome);

        let checks: [(bool, &str); 8] = [
            (
                self.grid.is_some_and(|(cols, rows)| cols > 1 && rows > 1),
                "the session has a real grid",
            ),
            (
                self.origin.is_some_and(|(x, y)| x == 0 && y > 0),
                "the terminal is offset below the strip, so no row hides under the chrome",
            ),
            (
                self.chrome_url.as_deref() == Some("tauri://localhost"),
                "the chrome loads the BUILT assets - a devUrl in the config silently points every webview at a dev server that is not running",
            ),
            (page_finished, "the chrome document finished loading"),
            (
                document.contains("\"root\":true") && document.contains("\"scripts\":1"),
                "the loaded document really is ours - a failed load is an empty page, not an error",
            ),
            (
                chrome_ready,
                "the chrome reached the host over IPC - with no capability file it silently cannot",
            ),
            (
                !grid.is_empty() && document.contains(&grid),
                "the host reached the chrome: the strip renders the REAL grid, which also proves the replay-on-handshake path",
            ),
            (z_ordered, "the chrome layer sits ABOVE the terminal layer"),
        ];

        let mut passed = true;
        for (held, what) in checks {
            println!("bindary: [{}] {what}", if held { "PASS" } else { "FAIL" });
            passed &= held;
        }
        passed
    }
}


/// What the chrome is told. Mirrors `SessionState` in `chrome/src/protocol.ts`.
///
/// Deliberately incapable of carrying grid content, pixels or keystrokes: the law is expressed
/// as a vocabulary, so breaking it would require adding a field rather than forgetting a rule.
#[derive(Clone, serde::Serialize)]
struct SessionState {
    cols: u16,
    rows: u16,
    cwd: String,
    exited: bool,
}

/// Where the keyboard is pointed.
///
/// Tracked from mouse clicks rather than asked of AppKit, because the thing we need to know is
/// not "who is first responder" but "which of the two surfaces did the operator last aim at" -
/// and the webview covers a known strip, so a click's position answers it exactly.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Terminal,
    Chrome,
}

fn shell() -> Command {
    let path = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    Command::new(path)
}

/// The grid that fits below the chrome strip. Floors: a partial row is not a row.
fn grid_for(width: u32, height: u32, strip: u32, cell_width: u32, cell_height: u32) -> SessionGeometry {
    let usable = height.saturating_sub(strip);
    SessionGeometry {
        cols: (width / cell_width.max(1)).max(1) as u16,
        rows: (usable / cell_height.max(1)).max(1) as u16,
    }
}

fn main() {
    let app = tauri::Builder::default()
        .build(tauri::generate_context!())
        .expect("the tauri app");

    // The window is created HERE rather than in `setup`, and that is not a style choice: `setup`
    // requires a `Send` closure, and everything below - the session, the GPU context, the event
    // monitor - is main-thread-bound by construction. Building the window in main keeps it all
    // on one thread with no locks and no `Send` gymnastics around types that must never leave
    // this thread anyway.
    let window = WindowBuilder::new(&app, "main")
        .title("Bindary")
        .inner_size(900.0, 560.0)
        .visible(!headless())
        .build()
        .expect("a window");

    let scale = window.scale_factor().unwrap_or(1.0);
    let physical = window.inner_size().expect("the window size");
    let strip = (BAR_HEIGHT * scale) as u32;

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
    let geometry = grid_for(physical.width, physical.height, strip, cell.width, cell.height);
    if let Err(error) = session.resize(geometry) {
        eprintln!("bindary: initial resize refused: {error:?}");
    }

    // SURFACE FIRST. The webview is added after this call and therefore lands above it; reversed,
    // the terminal covers the chrome and nothing errors. See the module card.
    let target = match WindowTarget::from_window(
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
    session.attach(target);
    session.set_origin(0, strip);

    // What the app WOULD serve, asked of the resolver rather than inferred from a blank page.
    // This is the control for the title probe: if the bytes are here and the page is still
    // empty, the fault is in the navigation; if they are missing, the embed is the fault and no
    // amount of navigating would have helped.
    for candidate in ["index.html", "/index.html"] {
        match app.asset_resolver().get(candidate.to_string()) {
            Some(asset) => println!(
                "bindary: asset {candidate:?} -> {} bytes, mime {}",
                asset.bytes.len(),
                asset.mime_type
            ),
            None => println!("bindary: asset {candidate:?} -> MISSING"),
        }
    }

    // MEASURED 2026-08-04: a child webview built before the run loop starts reports
    // `url=<none>, progress=1, loading=false` six seconds later - it never navigated at all.
    // Not a load failure and not a script failure: no navigation was ever requested. Asking for
    // it explicitly is the fix, and it is cheap enough to keep unconditionally - navigating to
    // the page it is already on is a no-op.
    if let Some(chrome) = window.get_webview("chrome") {
        match "tauri://localhost/index.html".parse() {
            Ok(url) => {
                if let Err(error) = chrome.navigate(url) {
                    eprintln!("bindary: the chrome refused to navigate: {error}");
                }
            }
            Err(error) => eprintln!("bindary: bad chrome url: {error}"),
        }
    }

    println!(
        "bindary: {}x{} cells at {}x{}px, strip {strip}px, scale {scale}",
        geometry.cols, geometry.rows, cell.width, cell.height
    );
    // Measured, not assumed. Every number here has already been wrong once in this project, and
    // each was found by a print contradicting a screenshot rather than by reading the code.
    println!(
        "bindary: inner {}x{} outer {:?} origin {:?}",
        physical.width,
        physical.height,
        window.outer_size().ok(),
        session.origin(),
    );
    if let Some(chrome) = window.get_webview("chrome") {
        println!(
            "bindary: chrome webview position {:?} size {:?}",
            chrome.position().ok(),
            chrome.size().ok()
        );
    } else {
        println!("bindary: NO chrome webview is registered on this window");
    }
    #[cfg(target_os = "macos")]
    view_tree(&window);

    // Shared between the AppKit key monitor and the run loop. Both are the main thread, so this
    // is an `Rc`, not an `Arc` behind a lock: introducing a mutex here would buy nothing and
    // would invite a deadlock the moment a key handler wanted to present.
    let session = Rc::new(RefCell::new(session));
    let focus = Rc::new(Cell::new(Focus::Terminal));

    // Never in headless mode: a local monitor makes the app eat keystrokes the moment it is
    // active, and an invisible window that steals the keyboard is the worst of both worlds.
    #[cfg(target_os = "macos")]
    let _monitor = (!headless()).then(|| input::monitor(Rc::clone(&session), Rc::clone(&focus), BAR_HEIGHT));

    // Registered before the run loop: the chrome can announce itself the moment its script
    // runs, and a listener attached later would miss exactly the fast case.
    // The chrome announces itself, and the host REPLAYS the current state in answer.
    //
    // Without this the strip shows placeholders forever, and it is not a race that shows up in
    // testing: the host emits on frame one, the chrome's module loads a few hundred milliseconds
    // later, and the state only ever changes again if the window is resized. Measured here as
    // `text: "BINDARY\n-\n-"` while the session was 94x27. The Swift host's panel bridge solved
    // the identical problem by replaying the latest message per kind; this is the same rule.
    let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let announced_ready = std::sync::Arc::clone(&ready);
    let replay = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let requested = std::sync::Arc::clone(&replay);
    app.listen("bindary://chrome-ready", move |event| {
        println!("bindary: chrome reported ready, payload {}", event.payload());
        requested.store(true, std::sync::atomic::Ordering::Relaxed);
        announced_ready.store(true, std::sync::atomic::Ordering::Relaxed);
    });

    let handle = app.handle().clone();
    let mut announced: Option<(u16, u16, bool)> = None;
    let mut frames: u32 = 0;
    let started = std::time::Instant::now();
    // `AppHandle::exit` REQUESTS an exit; the loop is still entered afterwards, and without this
    // flag the farewell printed a hundred times. A flag, not `process::exit`, because leaving
    // through the process would skip the pty host's teardown (SCAR-016).
    let mut finished = false;
    let mut probed = false;
    let mut ready_at: Option<std::time::Instant> = None;
    let smoke = std::rc::Rc::new(std::cell::RefCell::new(Smoke::default()));
    let page_finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // The chrome is attached from INSIDE the run loop, on the first frame.
    //
    // Measured 2026-08-04, and it cost four probes to see: a child webview created BEFORE
    // `app.run` never navigates at all (`url=<none>`), and forcing it to navigate then loads a
    // 178-byte document with no scripts instead of the 392-byte page the asset resolver holds -
    // the `tauri://` protocol is not serving that webview. Everything about it looks healthy
    // from Rust: `add_child` returns Ok, the view is placed correctly, `hidden=false`, and the
    // navigation reports Finished.
    let mut chrome_attached = false;

    app.run(move |_app, event| match event {
        RunEvent::MainEventsCleared => {
            if !chrome_attached {
                chrome_attached = true;
                let finished = std::sync::Arc::clone(&page_finished);
                if let Some(url) = attach_chrome(&window, finished) {
                    smoke.borrow_mut().chrome_url = Some(url);
                }
            }

            let mut session = session.borrow_mut();
            let drew = session.poll();
            if drew {
                if let Err(error) = session.present() {
                    // Loud and fatal. A present that fails and falls back is how B1's four
                    // defects stayed invisible for a day.
                    eprintln!("bindary: present failed: {error:?}");
                    handle.exit(1);
                }
            }

            // The chrome is told only when something it displays actually changed. Emitting every
            // frame would push 120 events a second through the IPC boundary to redraw the same
            // three strings.
            let geometry = session.geometry();
            let exited = session.exited();
            let state = (geometry.cols, geometry.rows, exited);
            if replay.swap(false, std::sync::atomic::Ordering::Relaxed) {
                // A fresh listener has nothing; forget what was already said so it is said again.
                announced = None;
            }
            if announced != Some(state) {
                announced = Some(state);
                if let Err(error) = handle.emit(
                    SESSION_EVENT,
                    SessionState {
                        cols: geometry.cols,
                        rows: geometry.rows,
                        // NAMED GAP: the child's OSC 7 report travels as a host event and is not
                        // plumbed into the session yet, so the chrome shows a placeholder rather
                        // than a wrong directory. Wiring it is the next step, not this one.
                        cwd: String::new(),
                        exited,
                    },
                ) {
                    eprintln!("bindary: could not reach the chrome: {error}");
                }
            }

            if exited {
                handle.exit(0);
            }

            // Headless runs are bounded from the INSIDE (SCAR-016): the process reaches its own
            // exit and its destructors run, rather than being killed by a timeout that would
            // skip the child's teardown.
            // Bounded by the CLOCK, not by a frame count. The first version counted 120 frames
            // and finished in well under a second, which is far too fast for a webview to fetch
            // and run a document - so "the chrome never reported ready" was partly a statement
            // about the harness. A budget has to outlast the thing it is waiting for.
            frames += 1;
            // Fired once, midway: `evaluateJavaScript` answers asynchronously on this same
            // thread, so a probe fired at the exit would print after the process is gone.
            // The probe is timed off the HANDSHAKE, not off the clock. Fired at a fixed
            // fraction of the budget it was flaky: sometimes the replayed state had not rendered
            // yet and the strip still showed a placeholder, so the gate failed for a reason that
            // had nothing to do with the code under test. A settle window after the chrome
            // announces itself is the event this actually depends on.
            if ready.load(std::sync::atomic::Ordering::Relaxed) && ready_at.is_none() {
                ready_at = Some(std::time::Instant::now());
            }
            let settled = ready_at.is_some_and(|at| at.elapsed() >= SETTLE);
            if headless() && !probed && settled {
                probed = true;
                {
                    let mut record = smoke.borrow_mut();
                    record.origin = session.origin();
                    record.grid = Some((geometry.cols, geometry.rows));
                }
                #[cfg(target_os = "macos")]
                probe_document(&window, std::rc::Rc::clone(&smoke));
            }
            if headless() && !finished && started.elapsed() >= HEADLESS_BUDGET {
                finished = true;
                println!("bindary: headless run complete after {frames} frames");
                #[cfg(target_os = "macos")]
                {
                    // Re-dumped at the END: the first dump runs before the chrome is attached,
                    // so it could only ever show half the answer.
                    smoke.borrow_mut().layers = view_tree(&window);
                    webview_state(&window);
                }
                let passed = smoke.borrow().judge(
                    page_finished.load(std::sync::atomic::Ordering::Relaxed),
                    ready.load(std::sync::atomic::Ordering::Relaxed),
                );
                println!("bindary: SMOKE {}", if passed { "PASSED" } else { "FAILED" });
                // The exit CODE is the gate's whole output to a script, and `AppHandle::exit`
                // does NOT carry it - measured: a FAILED smoke still exited 0, which would have
                // made this gate decoration the moment anything trusted it. The child is shut
                // down explicitly first, because leaving through the process skips the
                // destructor that reaps it (SCAR-016).
                session.shutdown();
                drop(session);
                std::process::exit(if passed { 0 } else { 1 });
                // `AppHandle::exit` requests an exit; it does not stop THIS iteration, and the
                // first version of this line printed its farewell a hundred times because the
                // loop kept being entered. Leaving through the process is what actually ends it,
                // and the session's child is reaped by the pty host's own teardown on the way.
            }
        }

        RunEvent::WindowEvent {
            event: tauri::WindowEvent::Resized(size),
            ..
        } => {
            let mut session = session.borrow_mut();
            let (width, height) = (size.width.max(1), size.height.max(1));
            let scale = window.scale_factor().unwrap_or(1.0);
            let strip = (BAR_HEIGHT * scale) as u32;

            if let Err(error) = session.resize_window(width, height) {
                eprintln!("bindary: window resize refused: {error:?}");
            }
            let cell = session.cell_metrics();
            let geometry = grid_for(width, height, strip, cell.width, cell.height);
            if let Err(error) = session.resize(geometry) {
                eprintln!("bindary: grid resize refused: {error:?}");
            }
            session.set_origin(0, strip);

            // The webview is a child view with no autoresizing mask; nothing moves it but this.
            let logical = size.to_logical::<f64>(scale);
            if let Some(chrome) = window.get_webview("chrome") {
                let _ = chrome.set_size(LogicalSize::new(logical.width, BAR_HEIGHT));
            }
        }

        _ => {}
    });
}

/// Reading the keyboard and the mouse from AppKit, because Tauri does not offer them.
#[cfg(target_os = "macos")]
mod input {
    use std::cell::{Cell, RefCell};
    use std::ptr::NonNull;
    use std::rc::Rc;

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags};
    use ruuah_vt_host::session::Session;
    use ruuah_vt_pty::key::{
        KEY_MODS_ALT, KEY_MODS_CTRL, KEY_MODS_SHIFT, KEY_MODS_SUPER, KeyAction, KeyEvent, KeyMods,
        encode,
    };
    use ruuah_vt_pty::keycode::key_from_macos_keycode;

    use super::Focus;

    /// Installs a LOCAL monitor: it sees this app's events before they are dispatched, and the
    /// value it returns decides their fate - the event to pass it on, null to swallow it.
    ///
    /// Local, never global: a global monitor would require Accessibility permission and would
    /// watch every application on the machine, which is an enormous ask for a terminal. The
    /// returned token must stay alive; dropping it removes the monitor and the keyboard goes
    /// quiet with nothing logged.
    pub fn monitor(
        session: Rc<RefCell<Session>>,
        focus: Rc<Cell<Focus>>,
        bar_height: f64,
    ) -> Option<Retained<AnyObject>> {
        // Captured once: the monitor and its handler both run on the main thread, and proving
        // that here is cheaper than proving it at every AppKit call inside the block.
        let mtm = objc2::MainThreadMarker::new()?;

        let handler = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            let event = unsafe { event.as_ref() };
            let pass = event as *const NSEvent as *mut NSEvent;

            match event.r#type() {
                objc2_app_kit::NSEventType::LeftMouseDown => {
                    // AppKit's window coordinates start at the BOTTOM left, so the strip - which
                    // is at the top - is the region with the LARGEST y. Getting this backwards
                    // would put focus exactly where it does not belong and look like the click
                    // being ignored.
                    let point = event.locationInWindow();
                    let height = event.window(mtm)
                        .map(|window| window.frame().size.height)
                        .unwrap_or(0.0);
                    focus.set(if point.y >= height - bar_height {
                        Focus::Chrome
                    } else {
                        Focus::Terminal
                    });
                    pass
                }

                objc2_app_kit::NSEventType::KeyDown => {
                    if focus.get() == Focus::Chrome {
                        return pass;
                    }
                    let flags = event.modifierFlags();
                    // Command chords are never eaten. They belong to the menu - cmd+Q, cmd+C,
                    // cmd+V - and a terminal that swallows them is a terminal you cannot quit.
                    // cmd+V is handled by the menu's paste path reaching the focused responder,
                    // so it is not special-cased here.
                    if flags.contains(NSEventModifierFlags::Command) {
                        return pass;
                    }

                    let key = key_from_macos_keycode(event.keyCode());
                    let text = event.characters().map(|value| value.to_string()).unwrap_or_default();
                    let unshifted = key.codepoint().unwrap_or(0);

                    let mut mods: KeyMods = 0;
                    if flags.contains(NSEventModifierFlags::Shift) {
                        mods |= KEY_MODS_SHIFT;
                    }
                    if flags.contains(NSEventModifierFlags::Control) {
                        mods |= KEY_MODS_CTRL;
                    }
                    if flags.contains(NSEventModifierFlags::Option) {
                        mods |= KEY_MODS_ALT;
                    }
                    if flags.contains(NSEventModifierFlags::Command) {
                        mods |= KEY_MODS_SUPER;
                    }

                    // Shift is CONSUMED when the layout used it to make the text; reporting it
                    // twice turns shift+a into a modified 'A' instead of an 'A'. Compared against
                    // the unshifted codepoint rather than assumed, because a layout may disagree
                    // (on the Hebrew layout shift+t is a different letter entirely).
                    let mut consumed: KeyMods = 0;
                    if !text.is_empty()
                        && mods & KEY_MODS_SHIFT != 0
                        && text.chars().next().map(u32::from) != Some(unshifted)
                    {
                        consumed |= KEY_MODS_SHIFT;
                    }

                    let session = session.borrow();
                    let bytes = encode(
                        &KeyEvent {
                            action: KeyAction::Press,
                            key,
                            mods,
                            consumed_mods: consumed,
                            composing: false,
                            utf8: &text,
                            unshifted_codepoint: unshifted,
                        },
                        &session.key_options(),
                    );
                    if bytes.is_empty() {
                        // Nothing to send is not the same as nothing to do: passing the event on
                        // keeps chords the terminal has no encoding for working elsewhere.
                        return pass;
                    }
                    if let Err(error) = session.send(&bytes) {
                        eprintln!("bindary: send failed: {error:?}");
                    }
                    std::ptr::null_mut()
                }

                _ => pass,
            }
        });

        unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                NSEventMask::KeyDown | NSEventMask::LeftMouseDown,
                &handler,
            )
        }
    }
}

/// Prints the window's real AppKit view hierarchy.
///
/// The question "where did the webview go" is not answerable from Rust types - Tauri reports a
/// webview it believes exists while AppKit decides what is actually drawn, and those two can
/// disagree. This asks AppKit.
#[cfg(target_os = "macos")]
fn view_tree(window: &tauri::Window) -> Vec<String> {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::NSView;

    let mut layers = Vec::new();
    let Ok(handle) = window.ns_window() else {
        println!("bindary: no ns_window to inspect");
        return layers;
    };
    let ns_window: *mut AnyObject = handle.cast();
    if ns_window.is_null() {
        println!("bindary: ns_window is null");
        return layers;
    }

    // Safety: `ns_window` is the NSWindow Tauri just created for this window; it is alive for as
    // long as the window is, and nothing here retains it past this call.
    unsafe {
        let content: Option<Retained<NSView>> = objc2::msg_send![ns_window, contentView];
        let Some(content) = content else {
            println!("bindary: window has no contentView");
            return layers;
        };
        let frame = content.frame();
        println!(
            "bindary: contentView {}x{}",
            frame.size.width, frame.size.height
        );
        // Z-ORDER, measured instead of looked at. A layer's position in its superlayer's
        // `sublayers` array IS its stacking order - later means on top - so this answers "is the
        // chrome above the terminal" with no screen involved. wgpu's `CAMetalLayer` and the
        // webview's layer are the two that matter.
        let layer: Option<Retained<AnyObject>> = objc2::msg_send![&*content, layer];
        if let Some(layer) = layer {
            let class: Retained<objc2_foundation::NSString> =
                objc2::msg_send![&*layer, description];
            println!("bindary: contentView.layer {}", class.to_string());
            let sublayers: Option<Retained<objc2_foundation::NSArray>> =
                objc2::msg_send![&*layer, sublayers];
            match sublayers {
                Some(sublayers) => {
                    let count: usize = objc2::msg_send![&*sublayers, count];
                    for index in 0..count {
                        let sublayer: Retained<AnyObject> =
                            objc2::msg_send![&*sublayers, objectAtIndex: index];
                        let name: Retained<objc2_foundation::NSString> =
                            objc2::msg_send![&*sublayer, className];
                        println!("bindary:   layer[{index}] {} (later = on top)", name.to_string());
                        layers.push(name.to_string());
                    }
                }
                None => println!("bindary: contentView.layer has no sublayers"),
            }
        }

        for (index, view) in content.subviews().iter().enumerate() {
            let frame = view.frame();
            let class = view.class().name().to_string_lossy().into_owned();
            println!(
                "bindary:   subview[{index}] {class} at ({}, {}) {}x{} hidden={}",
                frame.origin.x,
                frame.origin.y,
                frame.size.width,
                frame.size.height,
                view.isHidden(),
            );
        }
    }
    layers
}

/// Docks the chrome webview across the top of the window.
///
/// Called on the first frame rather than before the loop; see `chrome_attached`.
fn attach_chrome(
    window: &tauri::Window,
    finished: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Option<String> {
    let Ok(physical) = window.inner_size() else {
        eprintln!("bindary: no window size, no chrome");
        return None;
    };
    let scale = window.scale_factor().unwrap_or(1.0);
    let logical = physical.to_logical::<f64>(scale);

    match window.add_child(
        // NOT `transparent(true)`: on macOS that is gated behind Tauri's `macos-private-api`
        // feature, and private Apple API in an AGPL app people run is a bad trade for a
        // see-through bar. The strip is opaque and its background matches the terminal's, so the
        // seam does not flash before the page paints.
        WebviewBuilder::new("chrome", WebviewUrl::App("index.html".into()))
            .background_color(tauri::webview::Color(0x10, 0x10, 0x16, 0xff))
            .on_page_load(move |webview, payload| {
                println!(
                    "bindary: chrome page {:?} url={}",
                    payload.event(),
                    webview.url().map(|url| url.to_string()).unwrap_or_default()
                );
                if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                    finished.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }),
        LogicalPosition::new(0.0, 0.0),
        LogicalSize::new(logical.width, BAR_HEIGHT),
    ) {
        Ok(chrome) => {
            let url = chrome.url().map(|url| url.to_string()).ok();
            println!("bindary: chrome attached, url={url:?} size={:?}", chrome.size().ok());
            url
        }
        Err(error) => {
            eprintln!("bindary: the chrome webview refused to attach: {error}");
            None
        }
    }
}

/// Runs JS inside the chrome and prints what the DOCUMENT says about itself.
///
/// The instrument of last resort, and the only one that cannot be fooled by a wrong selector:
/// `title` read through Objective-C came back empty while the asset resolver held a valid 392
/// byte document and the navigation reported Finished. One of those three was lying, and asking
/// the DOM directly is what tells us which.
#[cfg(target_os = "macos")]
fn probe_document(window: &tauri::Window, smoke: std::rc::Rc<std::cell::RefCell<Smoke>>) {
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::NSView;
    use objc2_foundation::NSString;

    let Ok(handle) = window.ns_window() else { return };
    let ns_window: *mut AnyObject = handle.cast();
    if ns_window.is_null() {
        return;
    }

    // Safety: read-only selectors on a live NSWindow and its WKWebView subview; the completion
    // block is retained by WebKit for the duration of the call.
    unsafe {
        let content: Option<Retained<NSView>> = objc2::msg_send![ns_window, contentView];
        let Some(content) = content else { return };
        for view in content.subviews().iter() {
            let responds: bool =
                objc2::msg_send![&**view, respondsToSelector: objc2::sel!(evaluateJavaScript:completionHandler:)];
            if !responds {
                continue;
            }
            let script = NSString::from_str(
                "JSON.stringify({title: document.title, html: document.documentElement.outerHTML.length, root: !!document.getElementById('root'), scripts: document.scripts.length, text: document.body.innerText})",
            );
            let record = std::rc::Rc::clone(&smoke);
            let handler = RcBlock::new(move |value: *mut AnyObject, error: *mut AnyObject| {
                if !value.is_null() {
                    let text: Retained<NSString> = objc2::msg_send![value, description];
                    let text = text.to_string();
                    println!("bindary: document says {text}");
                    record.borrow_mut().document = Some(text);
                }
                if !error.is_null() {
                    let text: Retained<NSString> = objc2::msg_send![error, description];
                    println!("bindary: document probe failed: {}", text.to_string());
                }
            });
            let _: () = objc2::msg_send![&**view, evaluateJavaScript: &*script, completionHandler: &*handler];
        }
    }
}

/// Asks the real `WKWebView` what it thinks it is doing.
///
/// The Rust side reports a webview that exists and is placed correctly while the page never
/// runs, and those two facts are consistent with three different bugs. WebKit's own `URL` and
/// `estimatedProgress` separate them: no URL means loading never started, a URL with progress
/// below 1 means it started and stalled, and a completed load with no handshake means the
/// document ran and the SCRIPT is the problem.
#[cfg(target_os = "macos")]
fn webview_state(window: &tauri::Window) {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::NSView;
    use objc2_foundation::NSString;

    let Ok(handle) = window.ns_window() else {
        return;
    };
    let ns_window: *mut AnyObject = handle.cast();
    if ns_window.is_null() {
        return;
    }

    // Safety: the NSWindow is alive for the life of the Tauri window, and every selector below
    // is read-only. `estimatedProgress` and `URL` are WKWebView's; the subview is one.
    unsafe {
        let content: Option<Retained<NSView>> = objc2::msg_send![ns_window, contentView];
        let Some(content) = content else { return };
        for view in content.subviews().iter() {
            let responds: bool =
                objc2::msg_send![&**view, respondsToSelector: objc2::sel!(estimatedProgress)];
            if !responds {
                continue;
            }
            let progress: f64 = objc2::msg_send![&**view, estimatedProgress];
            let url: Option<Retained<AnyObject>> = objc2::msg_send![&**view, URL];
            let described = match url {
                Some(url) => {
                    let text: Retained<NSString> = objc2::msg_send![&*url, absoluteString];
                    text.to_string()
                }
                None => "<none>".to_string(),
            };
            let loading: bool = objc2::msg_send![&**view, isLoading];
            let title: Option<Retained<NSString>> = objc2::msg_send![&**view, title];
            let title = title.map(|value| value.to_string()).unwrap_or_default();
            println!(
                "bindary: WKWebView url={described} progress={progress} loading={loading} title={title:?}"
            );
        }
    }
}
