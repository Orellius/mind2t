//! Purpose: the embedder C surface -- spawn a shell, poll rendered pixels, send bytes.
//! Public surface: `mind2t_host_spawn/poll/send/resize/free` and their C types, mirrored
//!   one-to-one by `include/mind2t_host.h`.
//! Why this file: the GUI host (slice 8's Swift app) needs the pty -> core -> frame ->
//!   renderer pipeline behind one C handle, and none of that belongs in the `ghostty_*`
//!   mirror, which stays a pure VT readout. Depending on `mind2t-vt-abi` as an rlib puts
//!   both surfaces in one archive, because two Rust staticlibs cannot share a link.
//! NOT responsible for: VT semantics (core), the handoff protocol (frame), I/O (pty),
//!   rasterization (render). This crate only composes them and polices the boundary.
//! Test strategy: `tests/host_abi.rs` drives the whole chain through the C surface and
//!   byte-compares the pixels against a reference renderer fed the identical bytes through
//!   the Rust API, using the same `Publisher` the pump uses -- plus the skip-a-row control
//!   that proves the comparison can fail.

// The rlib dependency is what carries the 13 `ghostty_*` exports into this staticlib.
use mind2t_vt as _;

pub mod config;
pub mod cwd;
/// Mouse-reporting state and routing, shared by the C surface below and by `session`. One
/// policy, two callers - see its module card.
pub mod pointer;
/// The same pipeline as the C surface below, offered to Rust callers in this workspace.
/// See its module card for why both exist and when they converge.
pub mod session;
pub mod suggest;
pub mod workflow;

use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::Command;

use config::Config;
use mind2t_vt_frame::{BaseDirection, Frame};
use mind2t_vt_pty::{Geometry, Host, Options};
use mind2t_vt_render::{FontStack, GpuSurface, Palette, Renderer, Surface};

/// The font size used when `Mind2tHostOptions.font_size` is 0 -- the same size every
/// render-crate test measures with.
pub const DEFAULT_FONT_SIZE: f32 = 16.0;

/// Mirrors `Mind2tHostResult` in `mind2t_host.h`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mind2tHostResult {
    Success = 0,
    InvalidValue = 1,
    SpawnFailed = 2,
    ResizeRefused = 3,
    RenderFailed = 4,
    SendFailed = 5,
    /// The call was valid but the protocol produced nothing: mouse reporting is off,
    /// the event deduplicated away, or wheel routing left the event to the embedder
    /// (viewport scroll). Not an error -- the embedder's own handling is next.
    Ignored = 6,
}

/// Mirrors `Mind2tHostOptions` in `mind2t_host.h`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Mind2tHostOptions {
    pub cols: u16,
    pub rows: u16,
    pub font_size: f32,
    pub command: *const c_char,
    pub auto_direction: bool,
    /// Contributes ONLY the theme palette; NULL keeps the built-in scheme. The scalar
    /// settings are read by the embedder through the `mind2t_config_*` getters instead,
    /// because the embedder owns their precedence (CLI flags, Retina scaling).
    pub config: *const Mind2tConfig,
    /// Working directory for the child, or NULL for the default (home for an interactive
    /// shell, the caller's cwd for an explicit command). A path that does not exist is
    /// ignored, not an error. S5 workspaces set it to a worktree.
    pub cwd: *const c_char,
}

/// The state behind the opaque config handle: one loaded `Config` plus the C strings
/// its getters lend out.
pub struct Mind2tConfig {
    config: Config,
    shell: Option<CString>,
    font_family: Option<CString>,
    error: Option<CString>,
}

/// Mirrors `Mind2tHostFrame` in `mind2t_host.h`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Mind2tHostFrame {
    pub pixels: *const u8,
    pub width: u32,
    pub height: u32,
    pub generation: u64,
    pub drew: bool,
    pub child_exited: bool,
    /// The background the grid currently shows at its edge, RGBA -- resolved from the
    /// top-left cell's style, falling back to the palette default before any frame. A
    /// GUI painting window margins with it makes the terminal read as continuing into
    /// the frame, and it follows a program's own background (vim themes, BCE clears)
    /// as well as any future palette theme. Never sampled from pixels: the corner
    /// pixel belongs to the caret whenever the cursor sits at home.
    pub background: [u8; 4],
    /// One byte per grid row (`row_count` of them): the row's shell-semantic class per
    /// OSC 133 -- `MIND2T_ROW_OUTPUT`, `MIND2T_ROW_PROMPT` or `MIND2T_ROW_INPUT`. This is
    /// what a block gutter draws from (S2). Borrowed with the same lifetime as `pixels`:
    /// valid until the next poll, resize, or free. NULL before the first drawn frame.
    pub row_semantics: *const u8,
    pub row_count: u32,
    /// Rows the displayed view is scrolled up into history; 0 is the live bottom. Set by
    /// the pump after clamping, so it reports where the view actually IS -- a GUI drawing
    /// a scroll indicator reads this, never its own accumulated deltas.
    pub viewport_offset: u32,
    /// The caret's cell in this frame's coordinates, and whether it is shown. The
    /// ghost-suggestion layer (S4) anchors here; the renderer already draws the caret
    /// itself, so a GUI must never re-derive this from pixels.
    pub cursor_col: u16,
    pub cursor_row: u16,
    pub cursor_visible: bool,
}

pub const MIND2T_ROW_OUTPUT: u8 = 0;
pub const MIND2T_ROW_PROMPT: u8 = 1;
pub const MIND2T_ROW_INPUT: u8 = 2;
/// `mind2t_host_row_text` filter value: every cell regardless of its OSC 133 mark.
pub const MIND2T_TEXT_ALL: u8 = 255;

/// The state behind the opaque handle: the whole pipeline, composed.
pub struct Mind2tHost {
    host: Host,
    reader: mind2t_vt_frame::FrameReader,
    renderer: Renderer<GpuSurface>,
    frame: Frame,
    /// Stable storage backing the borrowed `pixels` pointer handed across the boundary.
    /// Replaced on every draw, which is exactly the documented lifetime: one poll.
    ///
    /// Left EMPTY while a window is attached: presenting reads the pixels on the GPU, so
    /// filling this would reinstate the very 12.5 MB per frame readback the window exists to
    /// avoid. `Mind2tHostFrame::pixels` is then null and the embedder draws nothing itself.
    pixels: Vec<u8>,
    /// The GPU context every renderer for this host is built on. Held so a rebuild lands on
    /// the same device an attached window's swapchain was created from.
    gpu: mind2t_vt_render::GpuContext,
    /// The window this host presents into, when one has been attached. `None` keeps the
    /// original CGImage path exactly as it was, which is what makes the swap reversible and
    /// keeps the old path available as the oracle.
    window: Option<mind2t_vt_render::WindowTarget>,
    drawn_generation: u64,
    font_size: f32,
    /// The configured lead font and ligature switch, kept because every renderer
    /// rebuild (resize, zoom) must reproduce them.
    font_family: Option<String>,
    ligatures: bool,
    /// The theme, kept because resize rebuilds the renderer -- a rebuild that forgot it
    /// would silently revert to the built-in scheme (pinned by the host_abi resize test).
    palette: Palette,
    /// The kitty image store the pump maintains; placements in the frame point at it.
    images: std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<u32, (u32, u32, std::sync::Arc<Vec<u8>>)>>,
    >,
    /// Events not yet handed to the embedder; refilled from the pump's queue whenever
    /// `mind2t_host_next_event` finds it empty. One event per call, oldest first.
    pending_events: std::collections::VecDeque<mind2t_vt_core::events::Event>,
    /// Stable storage backing the borrowed `row_semantics` pointer, one byte per row.
    /// Rebuilt on every draw, same lifetime contract as `pixels`.
    row_semantics: Vec<u8>,
    /// Mouse-reporting state the embedder cannot carry itself: view geometry (set via
    /// `mind2t_host_mouse_geometry`), which buttons are down, and the motion-dedup cell.
    /// The type is shared with `session` so both surfaces route a pointer identically.
    mouse: crate::pointer::Pointer,
    exited: bool,
}

/// One row's shell-semantic class, derived from the per-cell OSC 133 marks the core
/// tracks. Prompt wins over input: the row a prompt starts on usually also holds the
/// typed command, and the gutter wants block STARTS.
fn row_semantic(frame: &Frame, y: u16) -> u8 {
    let mut class = MIND2T_ROW_OUTPUT;
    for x in 0..frame.cols {
        match frame.cell(x, y).semantic() {
            mind2t_vt_snapshot::Semantic::Prompt => return MIND2T_ROW_PROMPT,
            mind2t_vt_snapshot::Semantic::Input => class = MIND2T_ROW_INPUT,
            mind2t_vt_snapshot::Semantic::Output => {}
        }
    }
    class
}

/// How a poll paints. `SkipRow` is the harness's broken renderer, never the real path.
enum DrawMode {
    Full,
    SkipRow(u16),
}

fn build_renderer(
    context: &mind2t_vt_render::GpuContext,
    font_size: f32,
    cols: u16,
    rows: u16,
    family: Option<&str>,
    ligatures: bool,
) -> Option<Renderer<GpuSurface>> {
    let fonts = FontStack::with_primary(family, font_size).ok()?;
    let cell = fonts.metrics();
    let context = context.clone();
    // Every rebuild lands on the SAME device. Building through `Renderer::with_surface`
    // would construct a fresh `GpuSurface`, and a fresh `GpuSurface` brings a whole new
    // instance, adapter, device and queue with it -- so a resize moved the renderer onto a
    // different device while the window's swapchain stayed on the old one. Nothing errored;
    // the frame simply stopped following the window (seen live 2026-08-04).
    //
    // The catch_unwind stays: surface construction can still fail on a machine with no
    // usable adapter, and across the C boundary that must be a reported failure rather than
    // an unwind into foreign frames.
    catch_unwind(AssertUnwindSafe(move || {
        let surface = GpuSurface::with_context(
            context,
            cell.width * u32::from(cols),
            cell.height * u32::from(rows),
        )
        .ok()?;
        let mut renderer = Renderer::<GpuSurface>::from_surface(fonts, surface, cols, rows);
        renderer.set_ligatures(ligatures);
        Some(renderer)
    }))
    .ok()
    .flatten()
}

fn poll_impl(host: &mut Mind2tHost, mode: DrawMode) -> Mind2tHostFrame {
    host.reader.read_into(&mut host.frame);

    let mut drew = false;
    if host.frame.is_valid() && host.frame.generation > host.drawn_generation {
        // A placement UNDER the text has to be blitted between the backgrounds and the
        // glyphs, which the damage-driven row path cannot express -- so such a frame takes
        // the layered path and repaints wholly. Checked before the ordinary draw so the
        // grid is not painted twice. Everything else, including every image drawn ON TOP,
        // keeps the incremental path it has always had.
        let layered = host.frame.placements.iter().any(|p| p.z < 0)
            && !matches!(mode, DrawMode::SkipRow(_));
        if layered {
            let resolved: Vec<_> = {
                let store = host.images.lock().expect("image store");
                host.frame
                    .placements
                    .iter()
                    .map(|placement| store.get(&placement.image).cloned())
                    .collect()
            };
            let placements = host.frame.placements.clone();
            host.renderer
                .draw_layered(&host.frame, &placements, &resolved);
        } else {
            match mode {
            // The very first paint covers every row: rows the child never touched carry no
            // damage stamp, and only `draw_all` gives them their background.
            DrawMode::Full if host.drawn_generation == 0 => {
                host.renderer.draw_all(&host.frame);
            }
            DrawMode::Full => {
                host.renderer.draw(&host.frame);
            }
            DrawMode::SkipRow(skip) => {
                host.renderer.draw_skipping_for_testing(&host.frame, skip);
            }
        }
        // Kitty placements blit over the drawn grid; the store lock is held only for
        // the Arc clones, never across the blend.
        if !host.frame.placements.is_empty() {
            let resolved: Vec<_> = {
                let store = host.images.lock().expect("image store");
                host.frame
                    .placements
                    .iter()
                    .map(|placement| store.get(&placement.image).cloned())
                    .collect()
            };
            let placements = host.frame.placements.clone();
            let mut cursor = resolved.into_iter();
            host.renderer
                .draw_images(&placements, move |_| cursor.next().flatten());
            }
        }
        // Unicode placeholders are re-resolved every polled frame rather than tracked:
        // they live in the grid, so any row repaint erases the picture on that row and it
        // has to go back on top. The scan costs a pass over the cells only when the child
        // has actually printed placeholder cells; `virtual_runs` bails on the first cell
        // of every ordinary row.
        if !host.frame.virtuals.is_empty() {
            let store = host.images.lock().expect("image store");
            let images: std::collections::HashMap<u32, (u32, u32, std::sync::Arc<Vec<u8>>)> =
                host.frame
                    .virtuals
                    .iter()
                    .filter_map(|v| store.get(&v.image).cloned().map(|found| (v.image, found)))
                    .collect();
            drop(store);
            host.renderer
                .draw_placeholders(&host.frame, |id| images.get(&id).cloned());
        }
        host.drawn_generation = host.frame.generation;
        // The readback exists only for embedders that draw the bytes themselves. With a window
        // attached the frame goes to the screen on the GPU, and copying it back would cost a
        // full frame across the bus every poll for nothing.
        host.pixels = if host.window.is_some() {
            Vec::new()
        } else {
            host.renderer.pixels()
        };
        // Rebuilt with the pixels so the two borrowed views always describe one frame.
        host.row_semantics.clear();
        host.row_semantics
            .extend((0..host.frame.rows).map(|y| row_semantic(&host.frame, y)));
        drew = true;
    }

    if !host.exited && matches!(host.host.try_wait(), Ok(Some(_))) {
        host.exited = true;
    }

    Mind2tHostFrame {
        pixels: if host.pixels.is_empty() {
            std::ptr::null()
        } else {
            host.pixels.as_ptr()
        },
        width: host.renderer.canvas().width(),
        height: host.renderer.canvas().height(),
        generation: host.drawn_generation,
        drew,
        child_exited: host.exited,
        background: {
            // The background the GRID currently shows at its edge, so a margin painted
            // with it reads as the terminal continuing (Ghostty's `extend`). Resolved
            // from the top-left cell's STYLE -- pixels there belong to the caret
            // whenever the cursor sits at home, but the style is the cell's own.
            let palette = host.renderer.palette();
            if host.frame.is_valid() {
                let style = host.frame.style(host.frame.cell(0, 0).style_id());
                palette.draw(&style).background
            } else {
                palette.default_background
            }
        },
        row_semantics: if host.row_semantics.is_empty() {
            std::ptr::null()
        } else {
            host.row_semantics.as_ptr()
        },
        row_count: host.row_semantics.len() as u32,
        viewport_offset: host.frame.viewport,
        cursor_col: host.frame.cursor.x,
        cursor_row: host.frame.cursor.y,
        cursor_visible: host.frame.cursor.visible,
    }
}

/// Builds the child command from the options: the configured command via `/bin/sh -c`,
/// or an interactive login shell. TERM is declared by the host (a Finder launch
/// inherits none) and the Claude Code child-session markers are scrubbed -- a terminal
/// window is a session boundary (seen live 2026-07-29). Returns None on a non-UTF-8
/// command string.
fn build_command(options: &Mind2tHostOptions) -> Option<Command> {
    let mut command = if options.command.is_null() {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let mut command = Command::new(shell);
        command.arg("-il");
        // A Finder-launched app inherits `/` as its cwd, and the login shell then opens
        // on the sealed read-only root -- which starship dresses in a padlock and the
        // operator read as a permission block (seen live 2026-07-30). A terminal
        // window means "a shell at home", so the default spawn says so; explicit
        // commands (the branch below) keep the caller's cwd, which is what the test
        // harness relies on.
        if let Ok(home) = std::env::var("HOME") {
            if std::path::Path::new(&home).is_dir() {
                command.current_dir(home);
            }
        }
        command
    } else {
        let text = unsafe { CStr::from_ptr(options.command) }.to_str().ok()?;
        let mut command = Command::new("/bin/sh");
        command.args(["-c", text]);
        command
    };
    // An explicit working directory outranks both defaults above (S5 workspaces: a
    // session belongs to a worktree). Set LAST so it wins over the home default without
    // that branch having to know this option exists.
    //
    // BOUNDARY, found in the S5 live tap: this places the child's STARTING directory,
    // and a configured command is free to walk away from it. `command = "cd X && exec
    // zsh"` lands in X regardless of `cwd`, because the cd runs after the exec. That is
    // correct precedence (an explicit command outranks a default), but it means a
    // workspace looks broken for anyone whose config.toml shell line contains a cd.
    //
    // A directory that does not exist is IGNORED rather than fatal. `Command::spawn`
    // reports a bad cwd as a plain ENOENT from the exec, indistinguishable from a missing
    // shell, and a workspace whose directory was deleted under us should open somewhere
    // usable rather than fail with a misleading error.
    if !options.cwd.is_null() {
        if let Ok(text) = unsafe { CStr::from_ptr(options.cwd) }.to_str() {
            if !text.is_empty() && std::path::Path::new(text).is_dir() {
                command.current_dir(text);
            }
        }
    }
    command.env("TERM", "xterm-256color");
    command.env_remove("CLAUDECODE");
    command.env_remove("CLAUDE_CODE_CHILD_SESSION");
    Some(command)
}

/// Spawns the command on a fresh pty and starts the parse/publish pipeline.
///
/// # Safety
/// `options` and `out` must be non-NULL and valid for the duration of the call;
/// `options.command` must be NULL or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_host_spawn(
    options: *const Mind2tHostOptions,
    out: *mut *mut Mind2tHost,
) -> Mind2tHostResult {
    if options.is_null() || out.is_null() {
        return Mind2tHostResult::InvalidValue;
    }
    // The failure contract: the out-param never dangles.
    unsafe { out.write(std::ptr::null_mut()) };
    let options = unsafe { options.read() };
    if options.cols == 0 || options.rows == 0 {
        return Mind2tHostResult::InvalidValue;
    }

    let Some(mut command) = build_command(&options) else {
        return Mind2tHostResult::InvalidValue;
    };
    let _ = &mut command; // rebuilt per retry below; the binding must stay mutable

    let font_size = if options.font_size > 0.0 {
        options.font_size
    } else {
        DEFAULT_FONT_SIZE
    };
    // The theme rides the config handle; NULL keeps the built-in scheme. Cloned out
    // because the handle's lifetime is the caller's -- freeing it after spawn is legal.
    let (palette, font_family, ligatures, reports) = if options.config.is_null() {
        // No config handle means no grant. Screen-inspection replies stay OFF for an
        // embedder that never opted in, which is the safe direction to be wrong in.
        (Palette::default(), None, true, false)
    } else {
        let config = &unsafe { &*options.config }.config;
        (
            config.palette.clone(),
            config.font_family.clone(),
            config.font_ligatures,
            config.reports,
        )
    };

    // One GPU context for this host's whole life. Every later rebuild - resize, zoom, font
    // change - is constructed on it, which is what keeps a window's swapchain valid across
    // rebuilds instead of orphaning it on a dead device.
    let Ok(gpu) = mind2t_vt_render::GpuContext::new() else {
        return Mind2tHostResult::RenderFailed;
    };

    // The renderer is built before the child so a machine that cannot render never spawns
    // a process it would immediately have to reap.
    let Some(mut renderer) = build_renderer(
        &gpu,
        font_size,
        options.cols,
        options.rows,
        font_family.as_deref(),
        ligatures,
    ) else {
        return Mind2tHostResult::RenderFailed;
    };
    renderer.set_palette(palette.clone());

    // fork/openpt can transiently EAGAIN when the machine is busy (measured under the
    // parallel test load, 2026-07-30: one spawn in a full run failed and passed alone).
    // A terminal window should survive that moment; genuine failures -- bad shell, no
    // pty -- fail identically on every attempt and still surface, 50ms later.
    let mut attempt = 0;
    let (host, reader) = loop {
        match Host::spawn(command, {
            // Screen-inspection replies are an EMBEDDER grant, off unless the
            // operator's config.toml says otherwise -- a program cannot ask for
            // them and RIS cannot revoke them.
            let mut spawn_options = Options::new(options.cols, options.rows);
            spawn_options.reports = reports;
            spawn_options
        }) {
            Ok(spawned) => break spawned,
            Err(_) if attempt < 2 => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(25));
                command = match build_command(&options) {
                    Some(rebuilt) => rebuilt,
                    None => return Mind2tHostResult::InvalidValue,
                };
            }
            Err(_) => return Mind2tHostResult::SpawnFailed,
        }
    };

    // Base direction is a reader-side layout preference on the host's own Frame -- the
    // publish channel never carries it and `read_into`/`resize` never write it, so setting
    // it once here holds for the handle's lifetime. Auto flips a row's flow only when the
    // row's own text resolves RTL; column-addressed TUI output stays where it was drawn.
    let mut frame = Frame::new();
    if options.auto_direction {
        frame.base_direction = BaseDirection::Auto;
    }

    let images = host.image_store();
    let handle = Box::new(Mind2tHost {
        host,
        reader,
        renderer,
        frame,
        pixels: Vec::new(),
        gpu,
        window: None,
        drawn_generation: 0,
        font_size,
        font_family,
        ligatures,
        palette,
        row_semantics: Vec::new(),
        images,
        pending_events: std::collections::VecDeque::new(),
        mouse: crate::pointer::Pointer::default(),
        exited: false,
    });
    unsafe { out.write(Box::into_raw(handle)) };
    Mind2tHostResult::Success
}

/// Reads the latest published frame and, if it is new, draws it.
///
/// # Safety
/// `host` must be a live handle from `mind2t_host_spawn`; `out` must be non-NULL and valid
/// for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_host_poll(
    host: *mut Mind2tHost,
    out: *mut Mind2tHostFrame,
) -> Mind2tHostResult {
    if host.is_null() || out.is_null() {
        return Mind2tHostResult::InvalidValue;
    }
    let frame = poll_impl(unsafe { &mut *host }, DrawMode::Full);
    unsafe { out.write(frame) };
    Mind2tHostResult::Success
}

/// Attaches a `CAMetalLayer` so polled frames are presented on the GPU instead of copied back.
///
/// Sizes are PHYSICAL pixels, not points. Passing point sizes on a Retina display configures a
/// half-resolution swapchain, and the result looks soft rather than broken - the kind of wrong
/// that ships.
///
/// Attaching stops `Mind2tHostFrame::pixels` being filled: with a window the frame reaches the
/// screen without ever crossing to the CPU, which is the whole point. An embedder that still
/// wants the bytes must detach first.
///
/// Refuses rather than degrades: an adapter that cannot drive this window, or a window that
/// offers no usable format, returns `RenderFailed` instead of a silently blank surface.
///
/// # Safety
/// `host` must be a live handle from `mind2t_host_spawn`. `layer` must be a live `CAMetalLayer`
/// that outlives the host or is removed with `mind2t_host_detach_layer` first.
#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_host_attach_layer(
    host: *mut Mind2tHost,
    layer: *mut std::ffi::c_void,
    width: u32,
    height: u32,
) -> Mind2tHostResult {
    if host.is_null() || layer.is_null() {
        return Mind2tHostResult::InvalidValue;
    }
    let host = unsafe { &mut *host };
    let context = host.renderer.surface_mut().context().clone();
    match unsafe {
        mind2t_vt_render::WindowTarget::from_metal_layer(&context, layer, width, height)
    } {
        Ok(window) => {
            host.window = Some(window);
            // The next poll must repaint: the window has nothing in it yet, and the frame the
            // embedder already drew lives in a CGImage we are about to stop producing.
            host.drawn_generation = 0;
            Mind2tHostResult::Success
        }
        Err(_) => Mind2tHostResult::RenderFailed,
    }
}

/// Drops the window, restoring the readback path on the next poll.
///
/// # Safety
/// `host` must be a live handle from `mind2t_host_spawn`.
#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_host_detach_layer(host: *mut Mind2tHost) -> Mind2tHostResult {
    if host.is_null() {
        return Mind2tHostResult::InvalidValue;
    }
    let host = unsafe { &mut *host };
    host.window = None;
    host.drawn_generation = 0;
    Mind2tHostResult::Success
}

/// Reconfigures the swapchain after the layer's drawable size changed. PHYSICAL pixels.
///
/// # Safety
/// `host` must be a live handle from `mind2t_host_spawn`.
#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_host_resize_layer(
    host: *mut Mind2tHost,
    width: u32,
    height: u32,
) -> Mind2tHostResult {
    if host.is_null() {
        return Mind2tHostResult::InvalidValue;
    }
    let host = unsafe { &mut *host };
    match host.window.as_mut() {
        Some(window) => {
            window.resize(width, height);
            Mind2tHostResult::Success
        }
        None => Mind2tHostResult::InvalidValue,
    }
}

/// Draws the current frame into the attached window and presents it.
///
/// Separate from `mind2t_host_poll` on purpose: polling advances the terminal, presenting puts
/// a frame on screen, and an embedder drives them at different rates - a resize presents
/// without polling, and a quiet terminal polls without needing to present.
///
/// # Safety
/// `host` must be a live handle from `mind2t_host_spawn`.
#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_host_present(host: *mut Mind2tHost) -> Mind2tHostResult {
    if host.is_null() {
        return Mind2tHostResult::InvalidValue;
    }
    let host = unsafe { &mut *host };
    if host.window.is_none() {
        return Mind2tHostResult::InvalidValue;
    }
    // The margin colour, resolved exactly the way the polled frame reports it: from the
    // top-left cell's STYLE, falling back to the palette default. A grid rounds to whole
    // cells, so the window is almost never an exact multiple of the surface and the
    // remainder is visible - clearing it to black while the terminal renders on 0x0d0d0d
    // draws a hard band down the edge.
    let clear = {
        let palette = host.renderer.palette();
        if host.frame.is_valid() {
            let style = host.frame.style(host.frame.cell(0, 0).style_id());
            palette.draw(&style).background
        } else {
            palette.default_background
        }
    };
    let window = host.window.as_mut().expect("checked above");
    match window.present(host.renderer.surface_mut(), clear) {
        Ok(()) => Mind2tHostResult::Success,
        Err(_) => Mind2tHostResult::RenderFailed,
    }
}

/// The same as `mind2t_host_poll`, but every draw silently declines one row.
///
/// This is a broken host on purpose: `tests/host_abi.rs` byte-compares polled pixels
/// against a reference, and a comparison that has never been seen to fail is not evidence.
/// Not part of the C surface, and it has no legitimate caller.
///
/// # Safety
/// Same contract as `mind2t_host_poll`.
#[doc(hidden)]
pub unsafe fn mind2t_host_poll_skipping_row_for_testing(
    host: *mut Mind2tHost,
    skip: u16,
    out: *mut Mind2tHostFrame,
) -> Mind2tHostResult {
    if host.is_null() || out.is_null() {
        return Mind2tHostResult::InvalidValue;
    }
    let frame = poll_impl(unsafe { &mut *host }, DrawMode::SkipRow(skip));
    unsafe { out.write(frame) };
    Mind2tHostResult::Success
}

/// Writes bytes to the child's input -- the `Host::send` seam.
///
/// # Safety
/// `host` must be a live handle; `bytes` must point to `len` readable bytes, or be NULL
/// when `len` is 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_host_send(
    host: *mut Mind2tHost,
    bytes: *const u8,
    len: usize,
) -> Mind2tHostResult {
    if host.is_null() || (bytes.is_null() && len != 0) {
        return Mind2tHostResult::InvalidValue;
    }
    let host = unsafe { &mut *host };
    let bytes = if len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(bytes, len) }
    };
    match host.host.send(bytes) {
        Ok(()) => Mind2tHostResult::Success,
        Err(_) => Mind2tHostResult::SendFailed,
    }
}

/// Encodes clipboard bytes for the child and writes them to the pty.
///
/// The transform is the oracle-measured paste encoding (`mind2t_vt_pty::paste`): xterm's
/// strip set becomes spaces always, then the data is wrapped in `ESC[200~`/`ESC[201~`
/// when the child has bracketed paste (mode 2004) on, or has its newlines folded to
/// carriage returns when it does not. Callers pass raw clipboard bytes and never build
/// either form themselves.
///
/// The mode rides the last polled frame -- the pump thread owns the terminal, so the
/// frame is how its state crosses to this thread. A host must therefore have polled
/// since the child enabled the mode; any host that renders does so continuously.
///
/// # Safety
/// `host` must be a live handle; `bytes` must point to `len` readable bytes, or be NULL
/// when `len` is 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_host_paste(
    host: *mut Mind2tHost,
    bytes: *const u8,
    len: usize,
) -> Mind2tHostResult {
    if host.is_null() || (bytes.is_null() && len != 0) {
        return Mind2tHostResult::InvalidValue;
    }
    let host = unsafe { &mut *host };
    let bytes = if len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(bytes, len) }
    };
    let encoded = mind2t_vt_pty::paste::encode(bytes, host.frame.bracketed_paste());
    match host.host.send(&encoded) {
        Ok(()) => Mind2tHostResult::Success,
        Err(_) => Mind2tHostResult::SendFailed,
    }
}

/// The state behind the opaque history handle: the store plus the path appends
/// persist to.
pub struct Mind2tHistory {
    history: suggest::History,
    path: PathBuf,
}

/// Opens (or starts) the command history at `path`, or `~/.ruuah/history` when NULL.
///
/// # Safety
/// `path`, if non-NULL, must be NUL-terminated; `out` must be valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_history_load(
    path: *const c_char,
    out: *mut *mut Mind2tHistory,
) -> Mind2tHostResult {
    if out.is_null() {
        return Mind2tHostResult::InvalidValue;
    }
    let path = if path.is_null() {
        let Some(home) = std::env::var_os("HOME") else {
            unsafe { out.write(std::ptr::null_mut()) };
            return Mind2tHostResult::InvalidValue;
        };
        Path::new(&home).join(".ruuah").join("history")
    } else {
        match unsafe { CStr::from_ptr(path) }.to_str() {
            Ok(path) => Path::new(path).to_path_buf(),
            Err(_) => {
                unsafe { out.write(std::ptr::null_mut()) };
                return Mind2tHostResult::InvalidValue;
            }
        }
    };
    let handle =
        Box::new(Mind2tHistory { history: suggest::History::load(&path), path });
    unsafe { out.write(Box::into_raw(handle)) };
    Mind2tHostResult::Success
}

/// # Safety
/// `handle` must be NULL or a live handle from `mind2t_history_load`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_history_free(handle: *mut Mind2tHistory) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// Records one executed command and persists the store. Blank, multiline, and
/// consecutive-duplicate commands are dropped by the store's own rules; a failed
/// save answers SendFailed but keeps the in-memory entry (suggestions still work
/// this session).
///
/// `cwd` is the RAW OSC 7 report (event kind 7), or NULL. It is normalized here rather
/// than by the caller: the core stores what the child sent, undecoded, so exactly one
/// place should know how to turn `file:///My%20Code` into a directory.
///
/// # Safety
/// `handle` live; `command` readable for `len` bytes; `cwd` readable for `cwd_len`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_history_append(
    handle: *mut Mind2tHistory,
    command: *const u8,
    len: usize,
    cwd: *const u8,
    cwd_len: usize,
) -> Mind2tHostResult {
    if handle.is_null() || (command.is_null() && len != 0) {
        return Mind2tHostResult::InvalidValue;
    }
    let cwd = unsafe { normalized_cwd(cwd, cwd_len) };
    let handle = unsafe { &mut *handle };
    let bytes = if len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(command, len) }
    };
    let Ok(command) = std::str::from_utf8(bytes) else {
        return Mind2tHostResult::InvalidValue;
    };
    let before = handle.history.len();
    handle.history.append(command, cwd.as_deref());
    if handle.history.len() == before {
        return Mind2tHostResult::Ignored;
    }
    match handle.history.save(&handle.path) {
        Ok(()) => Mind2tHostResult::Success,
        Err(_) => Mind2tHostResult::SendFailed,
    }
}

/// The most recent history entry `input` is a proper prefix of, via the buffer
/// protocol; `Ignored` with length 0 when nothing matches.
///
/// `cwd` is the RAW OSC 7 report, or NULL. A match made in that directory is preferred;
/// with no match there, or no directory at all, the newest match anywhere wins.
///
/// # Safety
/// `handle` live; `input` readable for `len` bytes; `cwd` readable for `cwd_len`;
/// `out`/`out_len` per the protocol.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_history_suggest(
    handle: *const Mind2tHistory,
    input: *const u8,
    len: usize,
    cwd: *const u8,
    cwd_len: usize,
    out: *mut u8,
    cap: usize,
    out_len: *mut usize,
) -> Mind2tHostResult {
    if handle.is_null() || (input.is_null() && len != 0) {
        return Mind2tHostResult::InvalidValue;
    }
    let cwd = unsafe { normalized_cwd(cwd, cwd_len) };
    let bytes = if len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(input, len) }
    };
    let Ok(input) = std::str::from_utf8(bytes) else {
        return Mind2tHostResult::InvalidValue;
    };
    match unsafe { &*handle }.history.suggest(input, cwd.as_deref()) {
        Some(suggestion) => copy_out(suggestion, out, cap, out_len),
        None => {
            if !out_len.is_null() {
                unsafe { out_len.write(0) };
            }
            Mind2tHostResult::Ignored
        }
    }
}

/// The filesystem path a raw OSC 7 report names, written into `out`.
///
/// The same normalizer the history calls use, exposed because the embedder needs the
/// decoded path for its own purposes (S6 runs git in it) and the repo's rule is that
/// exactly ONE place knows how to undo percent-escapes. A second decoder in Swift would
/// be a second implementation of a fiddly transform, in another language, with nothing
/// comparing the two -- which is precisely how the emitter/decoder pair drifted before.
///
/// `Ignored` (with `*out_len` set to 0) when the report names no directory, so "not a
/// path" is distinguishable from "buffer too small". Call with `out` NULL to size.
///
/// # Safety
/// `raw` readable for `len` bytes or NULL when `len` is 0; `out` writable for `cap`
/// bytes or NULL; `out_len` writable or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_cwd_path(
    raw: *const u8,
    len: usize,
    out: *mut u8,
    cap: usize,
    out_len: *mut usize,
) -> Mind2tHostResult {
    match unsafe { normalized_cwd(raw, len) } {
        Some(path) => copy_out(&path, out, cap, out_len),
        None => {
            if !out_len.is_null() {
                unsafe { out_len.write(0) };
            }
            Mind2tHostResult::Ignored
        }
    }
}

/// A raw OSC 7 report as a directory key, or `None` for NULL, empty, or unusable input.
///
/// # Safety
/// `raw` readable for `len` bytes, or NULL when `len` is 0.
unsafe fn normalized_cwd(raw: *const u8, len: usize) -> Option<String> {
    if raw.is_null() || len == 0 {
        return None;
    }
    crate::cwd::normalize(unsafe { std::slice::from_raw_parts(raw, len) })
}

fn mouse_mods(mods: u32) -> mind2t_vt_pty::mouse::Mods {
    mind2t_vt_pty::mouse::Mods {
        shift: mods & 1 != 0,
        ctrl: mods & 2 != 0,
        alt: mods & 4 != 0,
    }
}

/// Sets the view geometry mouse encoding converts through: the surface size in pixels
/// and the content insets around the grid, both in the same backing-pixel space the
/// frame's pixels use. Call after layout changes (resize, zoom, inset change); until
/// the first call, pointer events answer `Ignored`.
///
/// # Safety
/// `host` must be a live handle from `mind2t_host_spawn`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_host_mouse_geometry(
    host: *mut Mind2tHost,
    screen_width: u32,
    screen_height: u32,
    padding_left: u32,
    padding_top: u32,
    padding_right: u32,
    padding_bottom: u32,
) -> Mind2tHostResult {
    if host.is_null() || screen_width == 0 || screen_height == 0 {
        return Mind2tHostResult::InvalidValue;
    }
    let host = unsafe { &mut *host };
    host.mouse.set_geometry(
        screen_width,
        screen_height,
        padding_left,
        padding_top,
        padding_right,
        padding_bottom,
    );
    Mind2tHostResult::Success
}

/// Feeds one pointer event to the mouse-reporting protocol.
///
/// `action`: 0 press, 1 release, 2 motion. `button`: 0 none (motion with nothing
/// held), 1 left, 2 middle, 3 right, 4..9 the protocol's wheel/aux buttons. `mods`:
/// bit 0 shift, bit 1 ctrl, bit 2 alt. `x`/`y`: surface pixels from the view's
/// top-left, the same space `mind2t_host_mouse_geometry` described.
///
/// Returns `Success` when a report was encoded and written to the pty, `Ignored` when
/// the protocol produced nothing -- reporting off, motion deduplicated, position
/// outside the viewport with nothing held, or a button the protocol cannot name. On
/// `Ignored` the event is the embedder's again (selection, context menus). Button
/// bookkeeping happens on every call either way, so press/release pairs must reach
/// this function even while reporting is off.
///
/// The active modes ride the last polled frame, like `mind2t_host_paste`.
///
/// # Safety
/// `host` must be a live handle from `mind2t_host_spawn`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_host_mouse(
    host: *mut Mind2tHost,
    action: u32,
    button: u32,
    mods: u32,
    x: f32,
    y: f32,
) -> Mind2tHostResult {
    if host.is_null() || action > 2 {
        return Mind2tHostResult::InvalidValue;
    }
    let host = unsafe { &mut *host };

    use mind2t_vt_pty::mouse::Action;
    let action = match action {
        0 => Action::Press,
        1 => Action::Release,
        _ => Action::Motion,
    };

    // The policy - held-button bookkeeping, geometry, dedup - lives in `pointer`, because the
    // Rust surface routes a pointer through the same rules and a second copy of them would
    // diverge silently. Only the C contract (codes in, result codes out) is here.
    let cell = host.renderer.cell_metrics();
    let encoded = host.mouse.button(
        &host.frame,
        cell,
        crate::pointer::Input { action, code: button, mods: mouse_mods(mods), x, y },
    );
    match encoded {
        Some(bytes) => match host.host.send(&bytes) {
            Ok(()) => Mind2tHostResult::Success,
            Err(_) => Mind2tHostResult::SendFailed,
        },
        None => Mind2tHostResult::Ignored,
    }
}

/// Feeds one keyboard event to the key encoder and writes the result to the pty.
///
/// `action`: 0 release, 1 press, 2 repeat. `key`: the C key enum from
/// `ghostty/vt/key/event.h` (KeyMap.swift's values). `mods`/`consumed_mods`: the
/// GhosttyMods bitmask (shift 1, ctrl 2, alt 4, super 8, caps 16, num 32).
/// `text`/`text_len`: the translated UTF-8 for the event, or NULL/0 when the key
/// produced none. `unshifted_codepoint`: the key's codepoint with no modifiers, 0
/// when it has none.
///
/// Every encoding mode rides the last polled frame -- DECCKM, keypad application,
/// 1035/1036, modifyOtherKeys, and the active screen's kitty flags -- so a kitty
/// negotiation takes effect at the very next keystroke a rendering host forwards.
/// Returns `Success` when bytes were written, `Ignored` when the event encodes to
/// nothing (a bare modifier under legacy modes, a release without report-events,
/// mid-IME composition), `InvalidValue` for an out-of-range action/key or invalid
/// UTF-8.
///
/// # Safety
/// `host` must be a live handle from `mind2t_host_spawn`; `text`, if non-NULL, must
/// point to `text_len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_host_key(
    host: *mut Mind2tHost,
    action: u32,
    key: u32,
    mods: u32,
    consumed_mods: u32,
    text: *const u8,
    text_len: usize,
    unshifted_codepoint: u32,
) -> Mind2tHostResult {
    use mind2t_vt_pty::key::{Key, KeyAction, KeyEvent, KeyOptions, OptionAsAlt};
    if host.is_null() || action > 2 {
        return Mind2tHostResult::InvalidValue;
    }
    let host = unsafe { &mut *host };
    let action = match action {
        0 => KeyAction::Release,
        1 => KeyAction::Press,
        _ => KeyAction::Repeat,
    };
    // Key::ALL is in C declaration order by construction, so the C value indexes it.
    let Some(&key) = Key::ALL.get(key as usize) else {
        return Mind2tHostResult::InvalidValue;
    };
    let utf8 = if text.is_null() || text_len == 0 {
        ""
    } else {
        match std::str::from_utf8(unsafe { std::slice::from_raw_parts(text, text_len) }) {
            Ok(text) => text,
            Err(_) => return Mind2tHostResult::InvalidValue,
        }
    };

    let encoded = mind2t_vt_pty::key::encode(
        &KeyEvent {
            action,
            key,
            mods: mods as u16,
            consumed_mods: consumed_mods as u16,
            composing: false,
            utf8,
            unshifted_codepoint,
        },
        &KeyOptions {
            cursor_key_application: host.frame.cursor_keys(),
            keypad_key_application: host.frame.keypad_keys(),
            ignore_keypad_with_numlock: host.frame.ignore_keypad_with_numlock(),
            alt_esc_prefix: host.frame.alt_esc_prefix(),
            modify_other_keys_state_2: host.frame.modify_other_keys_2(),
            kitty_flags: host.frame.kitty_key_flags(),
            // The window owns option-as-alt policy; none is configured yet, and False
            // matches what setopt_from_terminal resets it to.
            macos_option_as_alt: OptionAsAlt::False,
            backarrow_key_mode: false,
        },
    );
    if encoded.is_empty() {
        return Mind2tHostResult::Ignored;
    }
    match host.host.send(&encoded) {
        Ok(()) => Mind2tHostResult::Success,
        Err(_) => Mind2tHostResult::SendFailed,
    }
}

/// Routes a wheel gesture through the terminal's three-way precedence, the oracle's
/// own (`Surface.zig` scrollCallback): an active mouse mode gets wheel-button reports
/// (64/65); otherwise the alternate screen with alternate scroll (1007, default on)
/// gets arrow keys, `ESC O` form under DECCKM and `ESC [` otherwise; otherwise the
/// event is `Ignored` and the embedder scrolls its viewport.
///
/// `ticks` is whole wheel notches, positive UP (toward history); the embedder owns
/// fractional banking. Returns `Success` when the terminal consumed the gesture --
/// including a mouse-mode wheel whose report encoded to nothing (X10 event mode
/// cannot name wheel buttons), because a program that captured the mouse must not
/// ALSO have the view scrolled under it.
///
/// # Safety
/// `host` must be a live handle from `mind2t_host_spawn`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_host_wheel(
    host: *mut Mind2tHost,
    x: f32,
    y: f32,
    ticks: i32,
    mods: u32,
) -> Mind2tHostResult {
    if host.is_null() {
        return Mind2tHostResult::InvalidValue;
    }
    let host = unsafe { &mut *host };
    if ticks == 0 {
        return Mind2tHostResult::Ignored;
    }

    // `Viewport` is the C surface's `Ignored`: this ABI has no viewport of its own, so handing
    // the wheel back to the embedder IS the answer. The Rust surface acts on it instead.
    let cell = host.renderer.cell_metrics();
    match host
        .mouse
        .wheel(&host.frame, cell, x, y, ticks, mouse_mods(mods))
    {
        crate::pointer::Wheel::Send(bytes) => {
            if !bytes.is_empty() && host.host.send(&bytes).is_err() {
                return Mind2tHostResult::SendFailed;
            }
            Mind2tHostResult::Success
        }
        crate::pointer::Wheel::Viewport => Mind2tHostResult::Ignored,
    }
}

/// The state behind the opaque workflows handle: parsed templates plus the loader's
/// error lines, joined for the one-string getter.
pub struct Mind2tWorkflows {
    workflows: Vec<workflow::Workflow>,
    errors: String,
}

/// Field selectors for `mind2t_workflow_field` / `mind2t_workflow_arg`.
pub const MIND2T_WORKFLOW_NAME: u32 = 0;
pub const MIND2T_WORKFLOW_DESCRIPTION: u32 = 1;
pub const MIND2T_WORKFLOW_COMMAND: u32 = 2;
pub const MIND2T_WORKFLOW_ARG_DEFAULT: u32 = 2;

/// Copies `value` out through the row_text buffer protocol: NULL `out` sizes, a short
/// buffer refuses with the needed length, and the copy carries no terminator.
fn copy_out(value: &str, out: *mut u8, cap: usize, out_len: *mut usize) -> Mind2tHostResult {
    if out_len.is_null() {
        return Mind2tHostResult::InvalidValue;
    }
    unsafe { out_len.write(value.len()) };
    if out.is_null() || cap < value.len() {
        return if out.is_null() && cap == 0 {
            Mind2tHostResult::Success
        } else {
            Mind2tHostResult::InvalidValue
        };
    }
    unsafe { std::ptr::copy_nonoverlapping(value.as_ptr(), out, value.len()) };
    Mind2tHostResult::Success
}

/// Loads the workflow templates from `dir`, or from `~/.ruuah/workflows` when NULL.
/// Broken files are skipped and their errors kept on the handle
/// (`mind2t_workflows_errors`) -- one bad template never hides the rest. Returns NULL
/// only when the out-param itself is unusable; an empty or missing directory is a
/// valid, empty handle.
///
/// # Safety
/// `dir`, if non-NULL, must be a NUL-terminated path; `out` must be valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_workflows_load(
    dir: *const c_char,
    out: *mut *mut Mind2tWorkflows,
) -> Mind2tHostResult {
    if out.is_null() {
        return Mind2tHostResult::InvalidValue;
    }
    let dir = if dir.is_null() {
        let Some(home) = std::env::var_os("HOME") else {
            unsafe { out.write(std::ptr::null_mut()) };
            return Mind2tHostResult::InvalidValue;
        };
        Path::new(&home).join(".ruuah").join("workflows")
    } else {
        match unsafe { CStr::from_ptr(dir) }.to_str() {
            Ok(path) => Path::new(path).to_path_buf(),
            Err(_) => {
                unsafe { out.write(std::ptr::null_mut()) };
                return Mind2tHostResult::InvalidValue;
            }
        }
    };
    let (workflows, errors) = workflow::load_dir(&dir);
    let handle = Box::new(Mind2tWorkflows { workflows, errors: errors.join("\n") });
    unsafe { out.write(Box::into_raw(handle)) };
    Mind2tHostResult::Success
}

/// # Safety
/// `handle` must be NULL or a live handle from `mind2t_workflows_load`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_workflows_free(handle: *mut Mind2tWorkflows) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// # Safety
/// `handle` must be a live handle from `mind2t_workflows_load`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_workflows_count(handle: *const Mind2tWorkflows) -> u32 {
    if handle.is_null() {
        return 0;
    }
    unsafe { &*handle }.workflows.len() as u32
}

/// The loader's error lines, newline-joined; empty when every file parsed. The GUI
/// shows this loudly, the config.rs posture.
///
/// # Safety
/// `handle` live; `out`/`out_len` per the buffer protocol.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_workflows_errors(
    handle: *const Mind2tWorkflows,
    out: *mut u8,
    cap: usize,
    out_len: *mut usize,
) -> Mind2tHostResult {
    if handle.is_null() {
        return Mind2tHostResult::InvalidValue;
    }
    copy_out(&unsafe { &*handle }.errors, out, cap, out_len)
}

/// One workflow's field: 0 name, 1 description, 2 command. The buffer protocol is
/// row_text's: NULL out sizes, short buffers refuse with the needed length.
///
/// # Safety
/// `handle` live; `out`/`out_len` per the buffer protocol.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_workflow_field(
    handle: *const Mind2tWorkflows,
    index: u32,
    field: u32,
    out: *mut u8,
    cap: usize,
    out_len: *mut usize,
) -> Mind2tHostResult {
    if handle.is_null() {
        return Mind2tHostResult::InvalidValue;
    }
    let Some(workflow) = unsafe { &*handle }.workflows.get(index as usize) else {
        return Mind2tHostResult::InvalidValue;
    };
    let value = match field {
        MIND2T_WORKFLOW_NAME => &workflow.name,
        MIND2T_WORKFLOW_DESCRIPTION => &workflow.description,
        MIND2T_WORKFLOW_COMMAND => &workflow.command,
        _ => return Mind2tHostResult::InvalidValue,
    };
    copy_out(value, out, cap, out_len)
}

/// # Safety
/// `handle` must be a live handle from `mind2t_workflows_load`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_workflow_arg_count(
    handle: *const Mind2tWorkflows,
    index: u32,
) -> u32 {
    if handle.is_null() {
        return 0;
    }
    let Some(workflow) = unsafe { &*handle }.workflows.get(index as usize) else {
        return 0;
    };
    workflow.args.len() as u32
}

/// One argument's field: 0 name, 1 description, 2 default. A missing default answers
/// `Ignored` with length 0, distinct from an empty-string default -- the palette
/// prefills one and prompts bare for the other.
///
/// # Safety
/// `handle` live; `out`/`out_len` per the buffer protocol.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_workflow_arg(
    handle: *const Mind2tWorkflows,
    index: u32,
    arg_index: u32,
    field: u32,
    out: *mut u8,
    cap: usize,
    out_len: *mut usize,
) -> Mind2tHostResult {
    if handle.is_null() {
        return Mind2tHostResult::InvalidValue;
    }
    let Some(arg) = unsafe { &*handle }
        .workflows
        .get(index as usize)
        .and_then(|workflow| workflow.args.get(arg_index as usize))
    else {
        return Mind2tHostResult::InvalidValue;
    };
    let value = match field {
        MIND2T_WORKFLOW_NAME => &arg.name,
        MIND2T_WORKFLOW_DESCRIPTION => &arg.description,
        MIND2T_WORKFLOW_ARG_DEFAULT => match &arg.default {
            Some(default) => default,
            None => {
                if !out_len.is_null() {
                    unsafe { out_len.write(0) };
                }
                return Mind2tHostResult::Ignored;
            }
        },
        _ => return Mind2tHostResult::InvalidValue,
    };
    copy_out(value, out, cap, out_len)
}

/// Renders one workflow's command with its placeholders substituted. `args_blob` is
/// pairs of NUL-terminated strings (name, value, name, value...), `blob_len` its
/// total byte length -- NUL separators because values may legally contain `=` or
/// newlines. An unresolved placeholder refuses with `InvalidValue`: a command with a
/// hole left in it must never reach the paste path.
///
/// # Safety
/// `handle` live; `args_blob` readable for `blob_len` bytes when non-NULL;
/// `out`/`out_len` per the buffer protocol.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_workflow_render(
    handle: *const Mind2tWorkflows,
    index: u32,
    args_blob: *const u8,
    blob_len: usize,
    out: *mut u8,
    cap: usize,
    out_len: *mut usize,
) -> Mind2tHostResult {
    if handle.is_null() || (args_blob.is_null() && blob_len != 0) {
        return Mind2tHostResult::InvalidValue;
    }
    let Some(workflow) = unsafe { &*handle }.workflows.get(index as usize) else {
        return Mind2tHostResult::InvalidValue;
    };
    let blob = if blob_len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(args_blob, blob_len) }
    };
    let mut parts = blob.split(|&b| b == 0).filter(|part| !part.is_empty());
    let mut values = Vec::new();
    while let (Some(name), Some(value)) = (parts.next(), parts.next()) {
        let (Ok(name), Ok(value)) =
            (std::str::from_utf8(name), std::str::from_utf8(value))
        else {
            return Mind2tHostResult::InvalidValue;
        };
        values.push((name.to_string(), value.to_string()));
    }
    match workflow::render(&workflow.command, &values) {
        Ok(rendered) => copy_out(&rendered, out, cap, out_len),
        Err(_) => Mind2tHostResult::InvalidValue,
    }
}

/// Scrolls the displayed view through scrollback: positive `rows` climbs into history,
/// negative returns toward the live bottom, and `INT32_MIN` snaps straight to it.
/// Deltas accumulate on the pump thread and are clamped against what history actually
/// holds; the landed position comes back in the next polled frame's `viewport_offset`.
/// Typing does NOT snap the view -- that is the embedder's policy to apply, via
/// `INT32_MIN`, at whatever input seam it owns.
///
/// # Safety
/// `host` must be a live handle from `mind2t_host_spawn`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_host_scroll(host: *mut Mind2tHost, rows: i32) -> Mind2tHostResult {
    if host.is_null() {
        return Mind2tHostResult::InvalidValue;
    }
    let host = unsafe { &mut *host };
    if rows == i32::MIN {
        host.host.scroll_to_bottom();
    } else {
        host.host.scroll(rows);
    }
    Mind2tHostResult::Success
}

/// Resizes the pty, the terminal and the render target.
///
/// # Safety
/// `host` must be a live handle from `mind2t_host_spawn`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_host_resize(
    host: *mut Mind2tHost,
    cols: u16,
    rows: u16,
) -> Mind2tHostResult {
    if host.is_null() || cols == 0 || rows == 0 {
        return Mind2tHostResult::InvalidValue;
    }
    let host = unsafe { &mut *host };
    if host.host.resize(Geometry { cols, rows }).is_err() {
        return Mind2tHostResult::ResizeRefused;
    }
    let Some(mut renderer) = build_renderer(
        &host.gpu,
        host.font_size,
        cols,
        rows,
        host.font_family.as_deref(),
        host.ligatures,
    ) else {
        return Mind2tHostResult::RenderFailed;
    };
    // The rebuild starts from the built-in scheme; the theme must survive it.
    renderer.set_palette(host.palette.clone());
    host.renderer = renderer;
    // Everything is owed again on the new canvas, and the old pixels describe a dead
    // geometry -- the borrowed pointer contract says they die here.
    host.drawn_generation = 0;
    host.pixels = Vec::new();
    host.row_semantics = Vec::new();
    Mind2tHostResult::Success
}

/// Reports the pixel cell size a renderer would use at `font_size`, without a host.
///
/// The GUI's zoom flow needs this BEFORE any renderer at the new size exists: the window
/// keeps its pixel size, so the new grid is window-pixels over these metrics, and only
/// then is `mind2t_host_set_font_size` called with both. Pure query; builds a font stack
/// and throws it away.
///
/// # Safety
/// `out_width` and `out_height` must be non-NULL and valid for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_host_cell_metrics(
    font_size: f32,
    font_family: *const c_char,
    out_width: *mut u32,
    out_height: *mut u32,
) -> Mind2tHostResult {
    if out_width.is_null() || out_height.is_null() || !(font_size > 0.0) {
        return Mind2tHostResult::InvalidValue;
    }
    let family = if font_family.is_null() {
        None
    } else {
        unsafe { CStr::from_ptr(font_family) }.to_str().ok()
    };
    let Ok(fonts) = FontStack::with_primary(family, font_size) else {
        return Mind2tHostResult::RenderFailed;
    };
    let metrics = fonts.metrics();
    unsafe {
        out_width.write(metrics.width);
        out_height.write(metrics.height);
    }
    Mind2tHostResult::Success
}

/// Changes the font size live: resizes the pty to the new grid and rebuilds the render
/// target at the new metrics, in one call.
///
/// A font change IS a geometry change -- the window keeps its pixel size, so the grid
/// that fits it moves with the cell metrics. The caller derives `cols`/`rows` from
/// `mind2t_host_cell_metrics` and passes both here; splitting this into set-size plus
/// `mind2t_host_resize` would rebuild the renderer twice and race a poll in between.
///
/// # Safety
/// `host` must be a live handle from `mind2t_host_spawn`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_host_set_font_size(
    host: *mut Mind2tHost,
    font_size: f32,
    cols: u16,
    rows: u16,
) -> Mind2tHostResult {
    if host.is_null() || cols == 0 || rows == 0 || !(font_size > 0.0) {
        return Mind2tHostResult::InvalidValue;
    }
    let host = unsafe { &mut *host };
    if host.host.resize(Geometry { cols, rows }).is_err() {
        return Mind2tHostResult::ResizeRefused;
    }
    let Some(mut renderer) = build_renderer(
        &host.gpu,
        font_size,
        cols,
        rows,
        host.font_family.as_deref(),
        host.ligatures,
    ) else {
        return Mind2tHostResult::RenderFailed;
    };
    renderer.set_palette(host.palette.clone());
    host.renderer = renderer;
    host.font_size = font_size;
    host.drawn_generation = 0;
    host.pixels = Vec::new();
    host.row_semantics = Vec::new();
    Mind2tHostResult::Success
}

/// Copies one grid row's text as UTF-8 into `out`, trailing blanks trimmed.
///
/// `semantic` filters by the per-cell OSC 133 mark: `MIND2T_TEXT_ALL` (255) takes every
/// cell; `MIND2T_ROW_OUTPUT`/`MIND2T_ROW_PROMPT`/`MIND2T_ROW_INPUT` take only cells wearing
/// that mark (a filtered-out cell contributes nothing, not a space). This is what makes
/// "copy command" exact: the prompt row holds `$ ls -la`, and the input filter returns
/// `ls -la` alone.
///
/// Reads the last POLLED frame -- poll at least once first; a rendering host does so
/// continuously. Spacer cells of wide glyphs are skipped, so a row with emoji comes back
/// as its clusters once each. Writes at most `cap` bytes (no NUL terminator is added),
/// stores the full length in `*len`, and fails with INVALID_VALUE when the row is out of
/// range or no frame has been polled. A `cap` smaller than `*len` copies the truncated
/// prefix (backed off to a UTF-8 boundary); the caller sizes `cap` from it and calls
/// again.
///
/// This is the copy-command/copy-output seam for blocks (S2): the GUI groups rows with
/// `row_semantics` and reads the text of the rows it selected.
///
/// Pops the next host-facing event: 0 = none pending, 1 = set the system clipboard to
/// the payload bytes, 2 = post a notification (payload is `title\ntitle-less body` --
/// the first newline separates title from body), 3 = bell (no payload), 4 = the
/// program set its title (payload = UTF-8 text), 5 = OSC 9;4 progress (payload = two
/// bytes: state 0..4, value 0..100).
///
/// One event per call, oldest first, exactly-once -- but an event is CONSUMED only
/// when `cap` held its whole payload. A smaller `cap` (zero included) reports kind and
/// `*len` and leaves the event queued, so the two-call size-then-fetch pattern works
/// without losing anything. The embedder decides policy -- the terminal only relays
/// what the child asked for.
///
/// # Safety
/// `host` must be a live handle; `kind` and `len` must be non-NULL; `out` must point to
/// `cap` writable bytes or be NULL when `cap` is 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_host_next_event(
    host: *mut Mind2tHost,
    kind: *mut u32,
    out: *mut u8,
    cap: usize,
    len: *mut usize,
) -> Mind2tHostResult {
    use mind2t_vt_core::events::Event;

    if host.is_null() || kind.is_null() || len.is_null() || (out.is_null() && cap != 0) {
        return Mind2tHostResult::InvalidValue;
    }
    unsafe {
        kind.write(0);
        len.write(0);
    }
    let host = unsafe { &mut *host };
    if host.pending_events.is_empty() {
        host.pending_events.extend(host.host.take_events());
    }
    let Some(event) = host.pending_events.front() else {
        return Mind2tHostResult::Success;
    };

    let (code, payload): (u32, Vec<u8>) = match event {
        Event::ClipboardSet(bytes) => (1, bytes.clone()),
        Event::Notify { title, body } => {
            let mut payload = title.clone().into_bytes();
            payload.push(b'\n');
            payload.extend_from_slice(body.as_bytes());
            (2, payload)
        }
        Event::Bell => (3, Vec::new()),
        Event::Title(title) => (4, title.clone().into_bytes()),
        Event::Progress { state, value } => (5, vec![*state, *value]),
        Event::CommandStart => (6, Vec::new()),
        // OSC 7. The payload is the raw report, usually a file:// URI: the host decodes
        // it, because percent-decoding here would diverge from the core's stored value
        // and from the oracle, and the seam's job is delivery rather than interpretation.
        Event::Pwd(pwd) => (7, pwd.clone()),
    };
    unsafe {
        kind.write(code);
        len.write(payload.len());
    }
    if payload.len() > cap {
        // Sizing call: the event stays queued for the fetch that fits it.
        return Mind2tHostResult::Success;
    }
    if !payload.is_empty() {
        unsafe { std::ptr::copy_nonoverlapping(payload.as_ptr(), out, payload.len()) };
    }
    host.pending_events.pop_front();
    Mind2tHostResult::Success
}

/// Copies the OSC 8 URI under one cell into `out`, if the cell was printed inside a
/// hyperlink.
///
/// Reads the last POLLED frame, like `mind2t_host_row_text`. A cell with no link is
/// SUCCESS with `*len` 0 -- a click on plain text is not an error. INVALID_VALUE only
/// for an out-of-range cell or a host that has never polled. Truncation contract
/// matches `mind2t_host_row_text` (size `cap` from a first call's `*len`).
///
/// # Safety
/// `host` must be a live handle; `out` must point to `cap` writable bytes or be NULL
/// when `cap` is 0; `len` must be non-NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_host_link_at(
    host: *mut Mind2tHost,
    col: u16,
    row: u16,
    out: *mut u8,
    cap: usize,
    len: *mut usize,
) -> Mind2tHostResult {
    if host.is_null() || len.is_null() || (out.is_null() && cap != 0) {
        return Mind2tHostResult::InvalidValue;
    }
    unsafe { len.write(0) };
    let host = unsafe { &mut *host };
    if !host.frame.is_valid() || row >= host.frame.rows || col >= host.frame.cols {
        return Mind2tHostResult::InvalidValue;
    }
    let Some(uri) = host.frame.link(col, row) else {
        return Mind2tHostResult::Success;
    };
    let bytes = uri.as_bytes();
    unsafe { len.write(bytes.len()) };
    if cap > 0 {
        let mut take = bytes.len().min(cap);
        while take > 0 && !uri.is_char_boundary(take) {
            take -= 1;
        }
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, take) };
    }
    Mind2tHostResult::Success
}

/// # Safety
/// `host` must be a live handle; `out` must point to `cap` writable bytes or be NULL when
/// `cap` is 0; `len` must be non-NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_host_row_text(
    host: *mut Mind2tHost,
    row: u16,
    semantic: u8,
    out: *mut u8,
    cap: usize,
    len: *mut usize,
) -> Mind2tHostResult {
    if host.is_null() || len.is_null() || (out.is_null() && cap != 0) {
        return Mind2tHostResult::InvalidValue;
    }
    unsafe { len.write(0) };
    let host = unsafe { &mut *host };
    if !host.frame.is_valid() || row >= host.frame.rows {
        return Mind2tHostResult::InvalidValue;
    }

    let wanted = |cell_semantic: mind2t_vt_snapshot::Semantic| match semantic {
        MIND2T_TEXT_ALL => true,
        MIND2T_ROW_OUTPUT => cell_semantic == mind2t_vt_snapshot::Semantic::Output,
        MIND2T_ROW_PROMPT => cell_semantic == mind2t_vt_snapshot::Semantic::Prompt,
        MIND2T_ROW_INPUT => cell_semantic == mind2t_vt_snapshot::Semantic::Input,
        _ => false,
    };

    let mut text = String::new();
    let mut scratch = [0u8; mind2t_vt_frame::CLUSTER_BYTES];
    for x in 0..host.frame.cols {
        let cell = host.frame.cell(x, row);
        if mind2t_vt_frame::cell_width(cell) == 0 || !wanted(cell.semantic()) {
            continue;
        }
        let cluster = cell.cluster(&mut scratch);
        if cluster.is_empty() {
            text.push(' ');
        } else {
            text.push_str(cluster);
        }
    }
    let trimmed = text.trim_end_matches(' ');

    unsafe { len.write(trimmed.len()) };
    if cap > 0 {
        let copy = trimmed.len().min(cap);
        // A UTF-8 boundary is not guaranteed at `copy`; back off to one so a truncated
        // read is still valid UTF-8 rather than a torn code unit.
        let boundary = (0..=copy).rev().find(|i| trimmed.is_char_boundary(*i)).unwrap_or(0);
        unsafe { std::ptr::copy_nonoverlapping(trimmed.as_ptr(), out, boundary) };
    }
    Mind2tHostResult::Success
}

/// Tears down the child, the pump thread and the renderer. NULL is a no-op.
///
/// # Safety
/// `host` must be NULL or a live handle from `mind2t_host_spawn`, and must not be used
/// again after this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_host_free(host: *mut Mind2tHost) {
    if host.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(host) });
}

/// Loads `dir/config.toml` (and the theme it names) into a new handle.
///
/// Always yields a usable config: a missing file is the defaults, and a file that could
/// not be honoured is the defaults plus `mind2t_config_error`. `dir` NULL means `~/.ruuah`.
/// Fails only on a NULL out-param or a non-UTF-8 dir.
///
/// # Safety
/// `dir` must be NULL or a NUL-terminated string; `out` must be non-NULL and valid for
/// the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_config_load(
    dir: *const c_char,
    out: *mut *mut Mind2tConfig,
) -> Mind2tHostResult {
    if out.is_null() {
        return Mind2tHostResult::InvalidValue;
    }
    unsafe { out.write(std::ptr::null_mut()) };
    let dir = if dir.is_null() {
        None
    } else {
        match unsafe { CStr::from_ptr(dir) }.to_str() {
            Ok(text) => Some(Path::new(text).to_path_buf()),
            Err(_) => return Mind2tHostResult::InvalidValue,
        }
    };

    let config = Config::load(dir.as_deref());
    // Interior NULs cannot occur: both strings come from TOML text that CStr::from_ptr
    // sources would have terminated -- but a file can contain anything, so fall back.
    let shell = config.shell.as_deref().and_then(|text| CString::new(text).ok());
    let font_family = config
        .font_family
        .as_deref()
        .and_then(|text| CString::new(text).ok());
    let error = config.error.as_deref().and_then(|text| CString::new(text).ok());
    let handle = Box::new(Mind2tConfig { config, shell, font_family, error });
    unsafe { out.write(Box::into_raw(handle)) };
    Mind2tHostResult::Success
}

/// Font size in logical pixels, 0 when the config does not set one. The embedder applies
/// its own default and backing-scale factor.
///
/// # Safety
/// `config` must be a live handle from `mind2t_config_load`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_config_font_size(config: *const Mind2tConfig) -> f32 {
    if config.is_null() {
        return 0.0;
    }
    unsafe { &*config }.config.font_size
}

/// The configured auto-direction, or `fallback` when the config does not say.
///
/// # Safety
/// `config` must be a live handle from `mind2t_config_load`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_config_auto_direction(
    config: *const Mind2tConfig,
    fallback: bool,
) -> bool {
    if config.is_null() {
        return fallback;
    }
    unsafe { &*config }.config.auto_direction.unwrap_or(fallback)
}

/// Whether the operator granted screen-inspection replies (DECRQCRA, WINOPS 18).
///
/// FALSE unless `config.toml` says `reports = true`, and false for a NULL handle.
/// Exposed so an embedder can show the posture rather than guess it; the grant
/// itself travels through `Mind2tHostOptions.config` at spawn, not through this.
///
/// # Safety
/// `config` must be NULL or a live handle from `mind2t_config_load`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_config_reports(config: *const Mind2tConfig) -> bool {
    if config.is_null() {
        return false;
    }
    unsafe { &*config }.config.reports
}

/// Whether the embedder may show web-rendered panels (S6 diff review).
///
/// FALSE unless `config.toml` says `panels = true`, and false for a NULL handle. The
/// core is unaffected either way; this is purely the embedder asking what its operator
/// allowed.
///
/// # Safety
/// `config` must be NULL or a live handle from `mind2t_config_load`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_config_panels(config: *const Mind2tConfig) -> bool {
    if config.is_null() {
        return false;
    }
    unsafe { &*config }.config.panels
}

/// The configured lead font family, or NULL when unset. Borrowed: valid until
/// `mind2t_config_free`.
///
/// # Safety
/// `config` must be NULL or a live handle from `mind2t_config_load`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_config_font_family(config: *const Mind2tConfig) -> *const c_char {
    if config.is_null() {
        return std::ptr::null();
    }
    match &unsafe { &*config }.font_family {
        Some(family) => family.as_ptr(),
        None => std::ptr::null(),
    }
}

/// The configured shell command line, or NULL when unset. Borrowed: valid until
/// `mind2t_config_free` on the same handle.
///
/// # Safety
/// `config` must be a live handle from `mind2t_config_load`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_config_shell(config: *const Mind2tConfig) -> *const c_char {
    if config.is_null() {
        return std::ptr::null();
    }
    match &unsafe { &*config }.shell {
        Some(shell) => shell.as_ptr(),
        None => std::ptr::null(),
    }
}

/// Everything that went wrong while loading, newline-joined -- or NULL when the load was
/// clean. Borrowed: valid until `mind2t_config_free` on the same handle. A GUI shows this
/// loudly; a config that silently half-applies is worse than one that errors.
///
/// # Safety
/// `config` must be a live handle from `mind2t_config_load`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_config_error(config: *const Mind2tConfig) -> *const c_char {
    if config.is_null() {
        return std::ptr::null();
    }
    match &unsafe { &*config }.error {
        Some(error) => error.as_ptr(),
        None => std::ptr::null(),
    }
}

/// Frees a config handle. NULL is a no-op. Strings lent by the getters die here.
///
/// # Safety
/// `config` must be NULL or a live handle from `mind2t_config_load`, and must not be used
/// again after this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_config_free(config: *mut Mind2tConfig) {
    if config.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(config) });
}
