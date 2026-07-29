//! Purpose: the embedder C surface -- spawn a shell, poll rendered pixels, send bytes.
//! Public surface: `ruuah_host_spawn/poll/send/resize/free` and their C types, mirrored
//!   one-to-one by `include/ruuah_host.h`.
//! Why this file: the GUI host (slice 8's Swift app) needs the pty -> core -> frame ->
//!   renderer pipeline behind one C handle, and none of that belongs in the `ghostty_*`
//!   mirror, which stays a pure VT readout. Depending on `ruuah-vt-abi` as an rlib puts
//!   both surfaces in one archive, because two Rust staticlibs cannot share a link.
//! NOT responsible for: VT semantics (core), the handoff protocol (frame), I/O (pty),
//!   rasterization (render). This crate only composes them and polices the boundary.
//! Test strategy: `tests/host_abi.rs` drives the whole chain through the C surface and
//!   byte-compares the pixels against a reference renderer fed the identical bytes through
//!   the Rust API, using the same `Publisher` the pump uses -- plus the skip-a-row control
//!   that proves the comparison can fail.

// The rlib dependency is what carries the 13 `ghostty_*` exports into this staticlib.
use ruuah_vt as _;

use std::ffi::{CStr, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::Command;

use ruuah_vt_frame::{BaseDirection, Frame};
use ruuah_vt_pty::{Geometry, Host, Options};
use ruuah_vt_render::{FontStack, GpuSurface, Renderer, Surface};

/// The font size used when `RuuahHostOptions.font_size` is 0 -- the same size every
/// render-crate test measures with.
pub const DEFAULT_FONT_SIZE: f32 = 16.0;

/// Mirrors `RuuahHostResult` in `ruuah_host.h`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuuahHostResult {
    Success = 0,
    InvalidValue = 1,
    SpawnFailed = 2,
    ResizeRefused = 3,
    RenderFailed = 4,
    SendFailed = 5,
}

/// Mirrors `RuuahHostOptions` in `ruuah_host.h`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RuuahHostOptions {
    pub cols: u16,
    pub rows: u16,
    pub font_size: f32,
    pub command: *const c_char,
    pub auto_direction: bool,
}

/// Mirrors `RuuahHostFrame` in `ruuah_host.h`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RuuahHostFrame {
    pub pixels: *const u8,
    pub width: u32,
    pub height: u32,
    pub generation: u64,
    pub drew: bool,
    pub child_exited: bool,
}

/// The state behind the opaque handle: the whole pipeline, composed.
pub struct RuuahHost {
    host: Host,
    reader: ruuah_vt_frame::FrameReader,
    renderer: Renderer<GpuSurface>,
    frame: Frame,
    /// Stable storage backing the borrowed `pixels` pointer handed across the boundary.
    /// Replaced on every draw, which is exactly the documented lifetime: one poll.
    pixels: Vec<u8>,
    drawn_generation: u64,
    font_size: f32,
    exited: bool,
}

/// How a poll paints. `SkipRow` is the harness's broken renderer, never the real path.
enum DrawMode {
    Full,
    SkipRow(u16),
}

fn build_renderer(font_size: f32, cols: u16, rows: u16) -> Option<Renderer<GpuSurface>> {
    let fonts = FontStack::system(font_size).ok()?;
    // `Surface::with_size` panics when no GPU adapter exists; across the C boundary that
    // must be a reported failure, not an unwind into foreign frames.
    catch_unwind(AssertUnwindSafe(move || {
        Renderer::<GpuSurface>::with_surface(fonts, cols, rows)
    }))
    .ok()
}

fn poll_impl(host: &mut RuuahHost, mode: DrawMode) -> RuuahHostFrame {
    host.reader.read_into(&mut host.frame);

    let mut drew = false;
    if host.frame.is_valid() && host.frame.generation > host.drawn_generation {
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
        host.drawn_generation = host.frame.generation;
        host.pixels = host.renderer.pixels();
        drew = true;
    }

    if !host.exited && matches!(host.host.try_wait(), Ok(Some(_))) {
        host.exited = true;
    }

    RuuahHostFrame {
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
    }
}

/// Spawns the command on a fresh pty and starts the parse/publish pipeline.
///
/// # Safety
/// `options` and `out` must be non-NULL and valid for the duration of the call;
/// `options.command` must be NULL or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_host_spawn(
    options: *const RuuahHostOptions,
    out: *mut *mut RuuahHost,
) -> RuuahHostResult {
    if options.is_null() || out.is_null() {
        return RuuahHostResult::InvalidValue;
    }
    // The failure contract: the out-param never dangles.
    unsafe { out.write(std::ptr::null_mut()) };
    let options = unsafe { options.read() };
    if options.cols == 0 || options.rows == 0 {
        return RuuahHostResult::InvalidValue;
    }

    let mut command = if options.command.is_null() {
        // An interactive login shell, which is what a terminal window means by "a shell".
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let mut command = Command::new(shell);
        command.arg("-il");
        command
    } else {
        let Ok(text) = unsafe { CStr::from_ptr(options.command) }.to_str() else {
            return RuuahHostResult::InvalidValue;
        };
        let mut command = Command::new("/bin/sh");
        command.args(["-c", text]);
        command
    };

    // The host owns the pty, so the host declares what it emulates -- a child launched
    // from Finder inherits no TERM at all, and one from a terminal inherits the wrong one.
    command.env("TERM", "xterm-256color");
    // A terminal window is a session boundary. When the app itself was launched from
    // inside a Claude Code session, the inherited child-session markers make a `claude`
    // run in this window believe it is nested and silently disable transcript saving
    // (seen live 2026-07-29). The shell in this window is a fresh session; scrub them.
    command.env_remove("CLAUDECODE");
    command.env_remove("CLAUDE_CODE_CHILD_SESSION");

    let font_size = if options.font_size > 0.0 {
        options.font_size
    } else {
        DEFAULT_FONT_SIZE
    };
    // The renderer is built before the child so a machine that cannot render never spawns
    // a process it would immediately have to reap.
    let Some(renderer) = build_renderer(font_size, options.cols, options.rows) else {
        return RuuahHostResult::RenderFailed;
    };

    let (host, reader) = match Host::spawn(command, Options::new(options.cols, options.rows)) {
        Ok(spawned) => spawned,
        Err(_) => return RuuahHostResult::SpawnFailed,
    };

    // Base direction is a reader-side layout preference on the host's own Frame -- the
    // publish channel never carries it and `read_into`/`resize` never write it, so setting
    // it once here holds for the handle's lifetime. Auto flips a row's flow only when the
    // row's own text resolves RTL; column-addressed TUI output stays where it was drawn.
    let mut frame = Frame::new();
    if options.auto_direction {
        frame.base_direction = BaseDirection::Auto;
    }

    let handle = Box::new(RuuahHost {
        host,
        reader,
        renderer,
        frame,
        pixels: Vec::new(),
        drawn_generation: 0,
        font_size,
        exited: false,
    });
    unsafe { out.write(Box::into_raw(handle)) };
    RuuahHostResult::Success
}

/// Reads the latest published frame and, if it is new, draws it.
///
/// # Safety
/// `host` must be a live handle from `ruuah_host_spawn`; `out` must be non-NULL and valid
/// for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_host_poll(
    host: *mut RuuahHost,
    out: *mut RuuahHostFrame,
) -> RuuahHostResult {
    if host.is_null() || out.is_null() {
        return RuuahHostResult::InvalidValue;
    }
    let frame = poll_impl(unsafe { &mut *host }, DrawMode::Full);
    unsafe { out.write(frame) };
    RuuahHostResult::Success
}

/// The same as `ruuah_host_poll`, but every draw silently declines one row.
///
/// This is a broken host on purpose: `tests/host_abi.rs` byte-compares polled pixels
/// against a reference, and a comparison that has never been seen to fail is not evidence.
/// Not part of the C surface, and it has no legitimate caller.
///
/// # Safety
/// Same contract as `ruuah_host_poll`.
#[doc(hidden)]
pub unsafe fn ruuah_host_poll_skipping_row_for_testing(
    host: *mut RuuahHost,
    skip: u16,
    out: *mut RuuahHostFrame,
) -> RuuahHostResult {
    if host.is_null() || out.is_null() {
        return RuuahHostResult::InvalidValue;
    }
    let frame = poll_impl(unsafe { &mut *host }, DrawMode::SkipRow(skip));
    unsafe { out.write(frame) };
    RuuahHostResult::Success
}

/// Writes bytes to the child's input -- the `Host::send` seam.
///
/// # Safety
/// `host` must be a live handle; `bytes` must point to `len` readable bytes, or be NULL
/// when `len` is 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_host_send(
    host: *mut RuuahHost,
    bytes: *const u8,
    len: usize,
) -> RuuahHostResult {
    if host.is_null() || (bytes.is_null() && len != 0) {
        return RuuahHostResult::InvalidValue;
    }
    let host = unsafe { &mut *host };
    let bytes = if len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(bytes, len) }
    };
    match host.host.send(bytes) {
        Ok(()) => RuuahHostResult::Success,
        Err(_) => RuuahHostResult::SendFailed,
    }
}

/// Encodes clipboard bytes for the child and writes them to the pty.
///
/// The transform is the oracle-measured paste encoding (`ruuah_vt_pty::paste`): xterm's
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
pub unsafe extern "C" fn ruuah_host_paste(
    host: *mut RuuahHost,
    bytes: *const u8,
    len: usize,
) -> RuuahHostResult {
    if host.is_null() || (bytes.is_null() && len != 0) {
        return RuuahHostResult::InvalidValue;
    }
    let host = unsafe { &mut *host };
    let bytes = if len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(bytes, len) }
    };
    let encoded = ruuah_vt_pty::paste::encode(bytes, host.frame.bracketed_paste());
    match host.host.send(&encoded) {
        Ok(()) => RuuahHostResult::Success,
        Err(_) => RuuahHostResult::SendFailed,
    }
}

/// Resizes the pty, the terminal and the render target.
///
/// # Safety
/// `host` must be a live handle from `ruuah_host_spawn`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_host_resize(
    host: *mut RuuahHost,
    cols: u16,
    rows: u16,
) -> RuuahHostResult {
    if host.is_null() || cols == 0 || rows == 0 {
        return RuuahHostResult::InvalidValue;
    }
    let host = unsafe { &mut *host };
    if host.host.resize(Geometry { cols, rows }).is_err() {
        return RuuahHostResult::ResizeRefused;
    }
    let Some(renderer) = build_renderer(host.font_size, cols, rows) else {
        return RuuahHostResult::RenderFailed;
    };
    host.renderer = renderer;
    // Everything is owed again on the new canvas, and the old pixels describe a dead
    // geometry -- the borrowed pointer contract says they die here.
    host.drawn_generation = 0;
    host.pixels = Vec::new();
    RuuahHostResult::Success
}

/// Tears down the child, the pump thread and the renderer. NULL is a no-op.
///
/// # Safety
/// `host` must be NULL or a live handle from `ruuah_host_spawn`, and must not be used
/// again after this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_host_free(host: *mut RuuahHost) {
    if host.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(host) });
}
