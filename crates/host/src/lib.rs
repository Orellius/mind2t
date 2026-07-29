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
//!   the Rust API, using the same `Publisher` the pump uses.
//!
//! PHASE 1 STATE: contract stubs. Every entry point validates its arguments and then
//! reports failure, so the harness can be run against a host that does nothing and be seen
//! red. Phase 2 replaces the bodies; this note leaves with them.

// The rlib dependency is what carries the 13 `ghostty_*` exports into this staticlib.
use ruuah_vt as _;

use std::ffi::c_char;

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

/// The state behind the opaque handle. Phase 2 gives it fields.
pub struct RuuahHost {
    _private: (),
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
    // The failure contract holds from the first stub: the out-param never dangles.
    unsafe { out.write(std::ptr::null_mut()) };
    let options = unsafe { options.read() };
    if options.cols == 0 || options.rows == 0 {
        return RuuahHostResult::InvalidValue;
    }
    RuuahHostResult::SpawnFailed
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
    RuuahHostResult::RenderFailed
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
    RuuahHostResult::SendFailed
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
    RuuahHostResult::ResizeRefused
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
