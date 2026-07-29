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

pub mod config;

use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::process::Command;

use config::Config;
use ruuah_vt_frame::{BaseDirection, Frame};
use ruuah_vt_pty::{Geometry, Host, Options};
use ruuah_vt_render::{FontStack, GpuSurface, Palette, Renderer, Surface};

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
    /// Contributes ONLY the theme palette; NULL keeps the built-in scheme. The scalar
    /// settings are read by the embedder through the `ruuah_config_*` getters instead,
    /// because the embedder owns their precedence (CLI flags, Retina scaling).
    pub config: *const RuuahConfig,
}

/// The state behind the opaque config handle: one loaded `Config` plus the C strings
/// its getters lend out.
pub struct RuuahConfig {
    config: Config,
    shell: Option<CString>,
    error: Option<CString>,
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
    /// The background the grid currently shows at its edge, RGBA -- resolved from the
    /// top-left cell's style, falling back to the palette default before any frame. A
    /// GUI painting window margins with it makes the terminal read as continuing into
    /// the frame, and it follows a program's own background (vim themes, BCE clears)
    /// as well as any future palette theme. Never sampled from pixels: the corner
    /// pixel belongs to the caret whenever the cursor sits at home.
    pub background: [u8; 4],
    /// One byte per grid row (`row_count` of them): the row's shell-semantic class per
    /// OSC 133 -- `RUUAH_ROW_OUTPUT`, `RUUAH_ROW_PROMPT` or `RUUAH_ROW_INPUT`. This is
    /// what a block gutter draws from (S2). Borrowed with the same lifetime as `pixels`:
    /// valid until the next poll, resize, or free. NULL before the first drawn frame.
    pub row_semantics: *const u8,
    pub row_count: u32,
}

pub const RUUAH_ROW_OUTPUT: u8 = 0;
pub const RUUAH_ROW_PROMPT: u8 = 1;
pub const RUUAH_ROW_INPUT: u8 = 2;
/// `ruuah_host_row_text` filter value: every cell regardless of its OSC 133 mark.
pub const RUUAH_TEXT_ALL: u8 = 255;

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
    /// The theme, kept because resize rebuilds the renderer -- a rebuild that forgot it
    /// would silently revert to the built-in scheme (pinned by the host_abi resize test).
    palette: Palette,
    /// Stable storage backing the borrowed `row_semantics` pointer, one byte per row.
    /// Rebuilt on every draw, same lifetime contract as `pixels`.
    row_semantics: Vec<u8>,
    exited: bool,
}

/// One row's shell-semantic class, derived from the per-cell OSC 133 marks the core
/// tracks. Prompt wins over input: the row a prompt starts on usually also holds the
/// typed command, and the gutter wants block STARTS.
fn row_semantic(frame: &Frame, y: u16) -> u8 {
    let mut class = RUUAH_ROW_OUTPUT;
    for x in 0..frame.cols {
        match frame.cell(x, y).semantic() {
            ruuah_vt_snapshot::Semantic::Prompt => return RUUAH_ROW_PROMPT,
            ruuah_vt_snapshot::Semantic::Input => class = RUUAH_ROW_INPUT,
            ruuah_vt_snapshot::Semantic::Output => {}
        }
    }
    class
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
        // Rebuilt with the pixels so the two borrowed views always describe one frame.
        host.row_semantics.clear();
        host.row_semantics
            .extend((0..host.frame.rows).map(|y| row_semantic(&host.frame, y)));
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
    // The theme rides the config handle; NULL keeps the built-in scheme. Cloned out
    // because the handle's lifetime is the caller's -- freeing it after spawn is legal.
    let palette = if options.config.is_null() {
        Palette::default()
    } else {
        unsafe { &*options.config }.config.palette.clone()
    };

    // The renderer is built before the child so a machine that cannot render never spawns
    // a process it would immediately have to reap.
    let Some(mut renderer) = build_renderer(font_size, options.cols, options.rows) else {
        return RuuahHostResult::RenderFailed;
    };
    renderer.set_palette(palette.clone());

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
        palette,
        row_semantics: Vec::new(),
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
    let Some(mut renderer) = build_renderer(host.font_size, cols, rows) else {
        return RuuahHostResult::RenderFailed;
    };
    // The rebuild starts from the built-in scheme; the theme must survive it.
    renderer.set_palette(host.palette.clone());
    host.renderer = renderer;
    // Everything is owed again on the new canvas, and the old pixels describe a dead
    // geometry -- the borrowed pointer contract says they die here.
    host.drawn_generation = 0;
    host.pixels = Vec::new();
    host.row_semantics = Vec::new();
    RuuahHostResult::Success
}

/// Reports the pixel cell size a renderer would use at `font_size`, without a host.
///
/// The GUI's zoom flow needs this BEFORE any renderer at the new size exists: the window
/// keeps its pixel size, so the new grid is window-pixels over these metrics, and only
/// then is `ruuah_host_set_font_size` called with both. Pure query; builds a font stack
/// and throws it away.
///
/// # Safety
/// `out_width` and `out_height` must be non-NULL and valid for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_host_cell_metrics(
    font_size: f32,
    out_width: *mut u32,
    out_height: *mut u32,
) -> RuuahHostResult {
    if out_width.is_null() || out_height.is_null() || !(font_size > 0.0) {
        return RuuahHostResult::InvalidValue;
    }
    let Ok(fonts) = FontStack::system(font_size) else {
        return RuuahHostResult::RenderFailed;
    };
    let metrics = fonts.metrics();
    unsafe {
        out_width.write(metrics.width);
        out_height.write(metrics.height);
    }
    RuuahHostResult::Success
}

/// Changes the font size live: resizes the pty to the new grid and rebuilds the render
/// target at the new metrics, in one call.
///
/// A font change IS a geometry change -- the window keeps its pixel size, so the grid
/// that fits it moves with the cell metrics. The caller derives `cols`/`rows` from
/// `ruuah_host_cell_metrics` and passes both here; splitting this into set-size plus
/// `ruuah_host_resize` would rebuild the renderer twice and race a poll in between.
///
/// # Safety
/// `host` must be a live handle from `ruuah_host_spawn`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_host_set_font_size(
    host: *mut RuuahHost,
    font_size: f32,
    cols: u16,
    rows: u16,
) -> RuuahHostResult {
    if host.is_null() || cols == 0 || rows == 0 || !(font_size > 0.0) {
        return RuuahHostResult::InvalidValue;
    }
    let host = unsafe { &mut *host };
    if host.host.resize(Geometry { cols, rows }).is_err() {
        return RuuahHostResult::ResizeRefused;
    }
    let Some(mut renderer) = build_renderer(font_size, cols, rows) else {
        return RuuahHostResult::RenderFailed;
    };
    renderer.set_palette(host.palette.clone());
    host.renderer = renderer;
    host.font_size = font_size;
    host.drawn_generation = 0;
    host.pixels = Vec::new();
    host.row_semantics = Vec::new();
    RuuahHostResult::Success
}

/// Copies one grid row's text as UTF-8 into `out`, trailing blanks trimmed.
///
/// `semantic` filters by the per-cell OSC 133 mark: `RUUAH_TEXT_ALL` (255) takes every
/// cell; `RUUAH_ROW_OUTPUT`/`RUUAH_ROW_PROMPT`/`RUUAH_ROW_INPUT` take only cells wearing
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
/// Copies the OSC 8 URI under one cell into `out`, if the cell was printed inside a
/// hyperlink.
///
/// Reads the last POLLED frame, like `ruuah_host_row_text`. A cell with no link is
/// SUCCESS with `*len` 0 -- a click on plain text is not an error. INVALID_VALUE only
/// for an out-of-range cell or a host that has never polled. Truncation contract
/// matches `ruuah_host_row_text` (size `cap` from a first call's `*len`).
///
/// # Safety
/// `host` must be a live handle; `out` must point to `cap` writable bytes or be NULL
/// when `cap` is 0; `len` must be non-NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_host_link_at(
    host: *mut RuuahHost,
    col: u16,
    row: u16,
    out: *mut u8,
    cap: usize,
    len: *mut usize,
) -> RuuahHostResult {
    if host.is_null() || len.is_null() || (out.is_null() && cap != 0) {
        return RuuahHostResult::InvalidValue;
    }
    unsafe { len.write(0) };
    let host = unsafe { &mut *host };
    if !host.frame.is_valid() || row >= host.frame.rows || col >= host.frame.cols {
        return RuuahHostResult::InvalidValue;
    }
    let Some(uri) = host.frame.link(col, row) else {
        return RuuahHostResult::Success;
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
    RuuahHostResult::Success
}

/// # Safety
/// `host` must be a live handle; `out` must point to `cap` writable bytes or be NULL when
/// `cap` is 0; `len` must be non-NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_host_row_text(
    host: *mut RuuahHost,
    row: u16,
    semantic: u8,
    out: *mut u8,
    cap: usize,
    len: *mut usize,
) -> RuuahHostResult {
    if host.is_null() || len.is_null() || (out.is_null() && cap != 0) {
        return RuuahHostResult::InvalidValue;
    }
    unsafe { len.write(0) };
    let host = unsafe { &mut *host };
    if !host.frame.is_valid() || row >= host.frame.rows {
        return RuuahHostResult::InvalidValue;
    }

    let wanted = |cell_semantic: ruuah_vt_snapshot::Semantic| match semantic {
        RUUAH_TEXT_ALL => true,
        RUUAH_ROW_OUTPUT => cell_semantic == ruuah_vt_snapshot::Semantic::Output,
        RUUAH_ROW_PROMPT => cell_semantic == ruuah_vt_snapshot::Semantic::Prompt,
        RUUAH_ROW_INPUT => cell_semantic == ruuah_vt_snapshot::Semantic::Input,
        _ => false,
    };

    let mut text = String::new();
    let mut scratch = [0u8; ruuah_vt_frame::CLUSTER_BYTES];
    for x in 0..host.frame.cols {
        let cell = host.frame.cell(x, row);
        if ruuah_vt_frame::cell_width(cell) == 0 || !wanted(cell.semantic()) {
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

/// Loads `dir/config.toml` (and the theme it names) into a new handle.
///
/// Always yields a usable config: a missing file is the defaults, and a file that could
/// not be honoured is the defaults plus `ruuah_config_error`. `dir` NULL means `~/.ruuah`.
/// Fails only on a NULL out-param or a non-UTF-8 dir.
///
/// # Safety
/// `dir` must be NULL or a NUL-terminated string; `out` must be non-NULL and valid for
/// the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_config_load(
    dir: *const c_char,
    out: *mut *mut RuuahConfig,
) -> RuuahHostResult {
    if out.is_null() {
        return RuuahHostResult::InvalidValue;
    }
    unsafe { out.write(std::ptr::null_mut()) };
    let dir = if dir.is_null() {
        None
    } else {
        match unsafe { CStr::from_ptr(dir) }.to_str() {
            Ok(text) => Some(Path::new(text).to_path_buf()),
            Err(_) => return RuuahHostResult::InvalidValue,
        }
    };

    let config = Config::load(dir.as_deref());
    // Interior NULs cannot occur: both strings come from TOML text that CStr::from_ptr
    // sources would have terminated -- but a file can contain anything, so fall back.
    let shell = config.shell.as_deref().and_then(|text| CString::new(text).ok());
    let error = config.error.as_deref().and_then(|text| CString::new(text).ok());
    let handle = Box::new(RuuahConfig { config, shell, error });
    unsafe { out.write(Box::into_raw(handle)) };
    RuuahHostResult::Success
}

/// Font size in logical pixels, 0 when the config does not set one. The embedder applies
/// its own default and backing-scale factor.
///
/// # Safety
/// `config` must be a live handle from `ruuah_config_load`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_config_font_size(config: *const RuuahConfig) -> f32 {
    if config.is_null() {
        return 0.0;
    }
    unsafe { &*config }.config.font_size
}

/// The configured auto-direction, or `fallback` when the config does not say.
///
/// # Safety
/// `config` must be a live handle from `ruuah_config_load`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_config_auto_direction(
    config: *const RuuahConfig,
    fallback: bool,
) -> bool {
    if config.is_null() {
        return fallback;
    }
    unsafe { &*config }.config.auto_direction.unwrap_or(fallback)
}

/// The configured shell command line, or NULL when unset. Borrowed: valid until
/// `ruuah_config_free` on the same handle.
///
/// # Safety
/// `config` must be a live handle from `ruuah_config_load`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_config_shell(config: *const RuuahConfig) -> *const c_char {
    if config.is_null() {
        return std::ptr::null();
    }
    match &unsafe { &*config }.shell {
        Some(shell) => shell.as_ptr(),
        None => std::ptr::null(),
    }
}

/// Everything that went wrong while loading, newline-joined -- or NULL when the load was
/// clean. Borrowed: valid until `ruuah_config_free` on the same handle. A GUI shows this
/// loudly; a config that silently half-applies is worse than one that errors.
///
/// # Safety
/// `config` must be a live handle from `ruuah_config_load`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_config_error(config: *const RuuahConfig) -> *const c_char {
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
/// `config` must be NULL or a live handle from `ruuah_config_load`, and must not be used
/// again after this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_config_free(config: *mut RuuahConfig) {
    if config.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(config) });
}
