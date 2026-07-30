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
pub mod suggest;
pub mod workflow;

use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
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
    /// The call was valid but the protocol produced nothing: mouse reporting is off,
    /// the event deduplicated away, or wheel routing left the event to the embedder
    /// (viewport scroll). Not an error -- the embedder's own handling is next.
    Ignored = 6,
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
    font_family: Option<CString>,
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
    /// `ruuah_host_next_event` finds it empty. One event per call, oldest first.
    pending_events: std::collections::VecDeque<ruuah_vt_core::events::Event>,
    /// Stable storage backing the borrowed `row_semantics` pointer, one byte per row.
    /// Rebuilt on every draw, same lifetime contract as `pixels`.
    row_semantics: Vec<u8>,
    /// Mouse-reporting state the embedder cannot carry itself: view geometry (set via
    /// `ruuah_host_mouse_geometry`), which buttons are down, and the motion-dedup cell.
    mouse: HostMouse,
    exited: bool,
}

/// The host side of mouse reporting. Geometry arrives from the embedder because only
/// it knows the view's pixel size and content insets; cell metrics come from the live
/// renderer at encode time because zoom rebuilds change them.
#[derive(Debug, Default)]
struct HostMouse {
    screen_width: u32,
    screen_height: u32,
    padding_left: u32,
    padding_top: u32,
    padding_right: u32,
    padding_bottom: u32,
    /// Buttons currently down, bit N for button code N. Updated BEFORE encoding (the
    /// oracle records click_state first), so a release's own button is already clear
    /// and `any_button_pressed` reflects what else is held.
    buttons_held: u16,
    /// Last reported cell for motion dedup, the encoder's cross-call state.
    last_cell: Option<(u32, u32)>,
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

fn build_renderer(
    font_size: f32,
    cols: u16,
    rows: u16,
    family: Option<&str>,
    ligatures: bool,
) -> Option<Renderer<GpuSurface>> {
    let fonts = FontStack::with_primary(family, font_size).ok()?;
    // `Surface::with_size` panics when no GPU adapter exists; across the C boundary that
    // must be a reported failure, not an unwind into foreign frames.
    catch_unwind(AssertUnwindSafe(move || {
        let mut renderer = Renderer::<GpuSurface>::with_surface(fonts, cols, rows);
        renderer.set_ligatures(ligatures);
        renderer
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
fn build_command(options: &RuuahHostOptions) -> Option<Command> {
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

    let Some(mut command) = build_command(&options) else {
        return RuuahHostResult::InvalidValue;
    };
    let _ = &mut command; // rebuilt per retry below; the binding must stay mutable

    let font_size = if options.font_size > 0.0 {
        options.font_size
    } else {
        DEFAULT_FONT_SIZE
    };
    // The theme rides the config handle; NULL keeps the built-in scheme. Cloned out
    // because the handle's lifetime is the caller's -- freeing it after spawn is legal.
    let (palette, font_family, ligatures) = if options.config.is_null() {
        (Palette::default(), None, true)
    } else {
        let config = &unsafe { &*options.config }.config;
        (
            config.palette.clone(),
            config.font_family.clone(),
            config.font_ligatures,
        )
    };

    // The renderer is built before the child so a machine that cannot render never spawns
    // a process it would immediately have to reap.
    let Some(mut renderer) =
        build_renderer(font_size, options.cols, options.rows, font_family.as_deref(), ligatures)
    else {
        return RuuahHostResult::RenderFailed;
    };
    renderer.set_palette(palette.clone());

    // fork/openpt can transiently EAGAIN when the machine is busy (measured under the
    // parallel test load, 2026-07-30: one spawn in a full run failed and passed alone).
    // A terminal window should survive that moment; genuine failures -- bad shell, no
    // pty -- fail identically on every attempt and still surface, 50ms later.
    let mut attempt = 0;
    let (host, reader) = loop {
        match Host::spawn(command, Options::new(options.cols, options.rows)) {
            Ok(spawned) => break spawned,
            Err(_) if attempt < 2 => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(25));
                command = match build_command(&options) {
                    Some(rebuilt) => rebuilt,
                    None => return RuuahHostResult::InvalidValue,
                };
            }
            Err(_) => return RuuahHostResult::SpawnFailed,
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
    let handle = Box::new(RuuahHost {
        host,
        reader,
        renderer,
        frame,
        pixels: Vec::new(),
        drawn_generation: 0,
        font_size,
        font_family,
        ligatures,
        palette,
        row_semantics: Vec::new(),
        images,
        pending_events: std::collections::VecDeque::new(),
        mouse: HostMouse::default(),
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

/// The state behind the opaque history handle: the store plus the path appends
/// persist to.
pub struct RuuahHistory {
    history: suggest::History,
    path: PathBuf,
}

/// Opens (or starts) the command history at `path`, or `~/.ruuah/history` when NULL.
///
/// # Safety
/// `path`, if non-NULL, must be NUL-terminated; `out` must be valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_history_load(
    path: *const c_char,
    out: *mut *mut RuuahHistory,
) -> RuuahHostResult {
    if out.is_null() {
        return RuuahHostResult::InvalidValue;
    }
    let path = if path.is_null() {
        let Some(home) = std::env::var_os("HOME") else {
            unsafe { out.write(std::ptr::null_mut()) };
            return RuuahHostResult::InvalidValue;
        };
        Path::new(&home).join(".ruuah").join("history")
    } else {
        match unsafe { CStr::from_ptr(path) }.to_str() {
            Ok(path) => Path::new(path).to_path_buf(),
            Err(_) => {
                unsafe { out.write(std::ptr::null_mut()) };
                return RuuahHostResult::InvalidValue;
            }
        }
    };
    let handle =
        Box::new(RuuahHistory { history: suggest::History::load(&path), path });
    unsafe { out.write(Box::into_raw(handle)) };
    RuuahHostResult::Success
}

/// # Safety
/// `handle` must be NULL or a live handle from `ruuah_history_load`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_history_free(handle: *mut RuuahHistory) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// Records one executed command and persists the store. Blank, multiline, and
/// consecutive-duplicate commands are dropped by the store's own rules; a failed
/// save answers SendFailed but keeps the in-memory entry (suggestions still work
/// this session).
///
/// # Safety
/// `handle` live; `command` readable for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_history_append(
    handle: *mut RuuahHistory,
    command: *const u8,
    len: usize,
) -> RuuahHostResult {
    if handle.is_null() || (command.is_null() && len != 0) {
        return RuuahHostResult::InvalidValue;
    }
    let handle = unsafe { &mut *handle };
    let bytes = if len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(command, len) }
    };
    let Ok(command) = std::str::from_utf8(bytes) else {
        return RuuahHostResult::InvalidValue;
    };
    let before = handle.history.len();
    handle.history.append(command);
    if handle.history.len() == before {
        return RuuahHostResult::Ignored;
    }
    match handle.history.save(&handle.path) {
        Ok(()) => RuuahHostResult::Success,
        Err(_) => RuuahHostResult::SendFailed,
    }
}

/// The most recent history entry `input` is a proper prefix of, via the buffer
/// protocol; `Ignored` with length 0 when nothing matches.
///
/// # Safety
/// `handle` live; `input` readable for `len` bytes; `out`/`out_len` per the protocol.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_history_suggest(
    handle: *const RuuahHistory,
    input: *const u8,
    len: usize,
    out: *mut u8,
    cap: usize,
    out_len: *mut usize,
) -> RuuahHostResult {
    if handle.is_null() || (input.is_null() && len != 0) {
        return RuuahHostResult::InvalidValue;
    }
    let bytes = if len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(input, len) }
    };
    let Ok(input) = std::str::from_utf8(bytes) else {
        return RuuahHostResult::InvalidValue;
    };
    match unsafe { &*handle }.history.suggest(input) {
        Some(suggestion) => copy_out(suggestion, out, cap, out_len),
        None => {
            if !out_len.is_null() {
                unsafe { out_len.write(0) };
            }
            RuuahHostResult::Ignored
        }
    }
}

/// Builds the encoder geometry from embedder-set view pixels plus the LIVE renderer's
/// cell metrics (zoom rebuilds move them). `None` until the embedder has called
/// `ruuah_host_mouse_geometry`.
fn mouse_size(host: &RuuahHost) -> Option<ruuah_vt_pty::mouse::Size> {
    if host.mouse.screen_width == 0 || host.mouse.screen_height == 0 {
        return None;
    }
    let cell = host.renderer.cell_metrics();
    Some(ruuah_vt_pty::mouse::Size {
        screen_width: host.mouse.screen_width,
        screen_height: host.mouse.screen_height,
        cell_width: cell.width,
        cell_height: cell.height,
        padding_left: host.mouse.padding_left,
        padding_top: host.mouse.padding_top,
        padding_right: host.mouse.padding_right,
        padding_bottom: host.mouse.padding_bottom,
    })
}

fn mouse_mods(mods: u32) -> ruuah_vt_pty::mouse::Mods {
    ruuah_vt_pty::mouse::Mods {
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
/// `host` must be a live handle from `ruuah_host_spawn`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_host_mouse_geometry(
    host: *mut RuuahHost,
    screen_width: u32,
    screen_height: u32,
    padding_left: u32,
    padding_top: u32,
    padding_right: u32,
    padding_bottom: u32,
) -> RuuahHostResult {
    if host.is_null() || screen_width == 0 || screen_height == 0 {
        return RuuahHostResult::InvalidValue;
    }
    let host = unsafe { &mut *host };
    host.mouse.screen_width = screen_width;
    host.mouse.screen_height = screen_height;
    host.mouse.padding_left = padding_left;
    host.mouse.padding_top = padding_top;
    host.mouse.padding_right = padding_right;
    host.mouse.padding_bottom = padding_bottom;
    RuuahHostResult::Success
}

/// Feeds one pointer event to the mouse-reporting protocol.
///
/// `action`: 0 press, 1 release, 2 motion. `button`: 0 none (motion with nothing
/// held), 1 left, 2 middle, 3 right, 4..9 the protocol's wheel/aux buttons. `mods`:
/// bit 0 shift, bit 1 ctrl, bit 2 alt. `x`/`y`: surface pixels from the view's
/// top-left, the same space `ruuah_host_mouse_geometry` described.
///
/// Returns `Success` when a report was encoded and written to the pty, `Ignored` when
/// the protocol produced nothing -- reporting off, motion deduplicated, position
/// outside the viewport with nothing held, or a button the protocol cannot name. On
/// `Ignored` the event is the embedder's again (selection, context menus). Button
/// bookkeeping happens on every call either way, so press/release pairs must reach
/// this function even while reporting is off.
///
/// The active modes ride the last polled frame, like `ruuah_host_paste`.
///
/// # Safety
/// `host` must be a live handle from `ruuah_host_spawn`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_host_mouse(
    host: *mut RuuahHost,
    action: u32,
    button: u32,
    mods: u32,
    x: f32,
    y: f32,
) -> RuuahHostResult {
    if host.is_null() || action > 2 {
        return RuuahHostResult::InvalidValue;
    }
    let host = unsafe { &mut *host };

    use ruuah_vt_pty::mouse::{Action, Button, Event, Options};
    let action = match action {
        0 => Action::Press,
        1 => Action::Release,
        _ => Action::Motion,
    };
    let button_enum = match button {
        0 => None,
        1 => Some(Button::Left),
        2 => Some(Button::Middle),
        3 => Some(Button::Right),
        4 => Some(Button::Four),
        5 => Some(Button::Five),
        6 => Some(Button::Six),
        7 => Some(Button::Seven),
        8 => Some(Button::Eight),
        9 => Some(Button::Nine),
        // Real hardware buttons the protocol has no code for still take part in the
        // held bookkeeping below; the encoder answers silence for them.
        _ => Some(Button::Other),
    };

    // Held-state first, the oracle's order: a release's own button is already clear
    // when the encoder asks what else is held.
    if button > 0 {
        let bit = 1u16 << (button.min(15) as u16);
        match action {
            Action::Press => host.mouse.buttons_held |= bit,
            Action::Release => host.mouse.buttons_held &= !bit,
            Action::Motion => {}
        }
    }

    let Some(size) = mouse_size(host) else {
        return RuuahHostResult::Ignored;
    };
    let encoded = ruuah_vt_pty::mouse::encode(
        Event { action, button: button_enum, mods: mouse_mods(mods), x, y },
        Options {
            event_mode: host.frame.mouse_event(),
            format: host.frame.mouse_format(),
            size,
            any_button_pressed: host.mouse.buttons_held != 0,
            last_cell: Some(&mut host.mouse.last_cell),
        },
    );
    match encoded {
        Some(bytes) => match host.host.send(&bytes) {
            Ok(()) => RuuahHostResult::Success,
            Err(_) => RuuahHostResult::SendFailed,
        },
        None => RuuahHostResult::Ignored,
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
/// `host` must be a live handle from `ruuah_host_spawn`; `text`, if non-NULL, must
/// point to `text_len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_host_key(
    host: *mut RuuahHost,
    action: u32,
    key: u32,
    mods: u32,
    consumed_mods: u32,
    text: *const u8,
    text_len: usize,
    unshifted_codepoint: u32,
) -> RuuahHostResult {
    use ruuah_vt_pty::key::{Key, KeyAction, KeyEvent, KeyOptions, OptionAsAlt};
    if host.is_null() || action > 2 {
        return RuuahHostResult::InvalidValue;
    }
    let host = unsafe { &mut *host };
    let action = match action {
        0 => KeyAction::Release,
        1 => KeyAction::Press,
        _ => KeyAction::Repeat,
    };
    // Key::ALL is in C declaration order by construction, so the C value indexes it.
    let Some(&key) = Key::ALL.get(key as usize) else {
        return RuuahHostResult::InvalidValue;
    };
    let utf8 = if text.is_null() || text_len == 0 {
        ""
    } else {
        match std::str::from_utf8(unsafe { std::slice::from_raw_parts(text, text_len) }) {
            Ok(text) => text,
            Err(_) => return RuuahHostResult::InvalidValue,
        }
    };

    let encoded = ruuah_vt_pty::key::encode(
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
        return RuuahHostResult::Ignored;
    }
    match host.host.send(&encoded) {
        Ok(()) => RuuahHostResult::Success,
        Err(_) => RuuahHostResult::SendFailed,
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
/// `host` must be a live handle from `ruuah_host_spawn`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_host_wheel(
    host: *mut RuuahHost,
    x: f32,
    y: f32,
    ticks: i32,
    mods: u32,
) -> RuuahHostResult {
    if host.is_null() {
        return RuuahHostResult::InvalidValue;
    }
    let host = unsafe { &mut *host };
    if ticks == 0 {
        return RuuahHostResult::Ignored;
    }

    use ruuah_vt_pty::mouse::{Action, Button, Event, Options};
    if host.frame.mouse_event() != ruuah_vt_core::mouse::MouseEvent::None {
        let Some(size) = mouse_size(host) else {
            return RuuahHostResult::Ignored;
        };
        let button = if ticks > 0 { Button::Four } else { Button::Five };
        let mut out = Vec::new();
        for _ in 0..ticks.unsigned_abs().min(64) {
            if let Some(bytes) = ruuah_vt_pty::mouse::encode(
                Event { action: Action::Press, button: Some(button), mods: mouse_mods(mods), x, y },
                Options {
                    event_mode: host.frame.mouse_event(),
                    format: host.frame.mouse_format(),
                    size,
                    any_button_pressed: host.mouse.buttons_held != 0,
                    last_cell: Some(&mut host.mouse.last_cell),
                },
            ) {
                out.extend(bytes);
            }
        }
        if !out.is_empty() && host.host.send(&out).is_err() {
            return RuuahHostResult::SendFailed;
        }
        return RuuahHostResult::Success;
    }

    if host.frame.alternate_screen() && host.frame.mouse_alternate_scroll() {
        let seq: &[u8] = match (host.frame.cursor_keys(), ticks > 0) {
            (true, true) => b"\x1bOA",
            (true, false) => b"\x1bOB",
            (false, true) => b"\x1b[A",
            (false, false) => b"\x1b[B",
        };
        let mut out = Vec::with_capacity(seq.len() * ticks.unsigned_abs().min(64) as usize);
        for _ in 0..ticks.unsigned_abs().min(64) {
            out.extend_from_slice(seq);
        }
        return match host.host.send(&out) {
            Ok(()) => RuuahHostResult::Success,
            Err(_) => RuuahHostResult::SendFailed,
        };
    }

    RuuahHostResult::Ignored
}

/// The state behind the opaque workflows handle: parsed templates plus the loader's
/// error lines, joined for the one-string getter.
pub struct RuuahWorkflows {
    workflows: Vec<workflow::Workflow>,
    errors: String,
}

/// Field selectors for `ruuah_workflow_field` / `ruuah_workflow_arg`.
pub const RUUAH_WORKFLOW_NAME: u32 = 0;
pub const RUUAH_WORKFLOW_DESCRIPTION: u32 = 1;
pub const RUUAH_WORKFLOW_COMMAND: u32 = 2;
pub const RUUAH_WORKFLOW_ARG_DEFAULT: u32 = 2;

/// Copies `value` out through the row_text buffer protocol: NULL `out` sizes, a short
/// buffer refuses with the needed length, and the copy carries no terminator.
fn copy_out(value: &str, out: *mut u8, cap: usize, out_len: *mut usize) -> RuuahHostResult {
    if out_len.is_null() {
        return RuuahHostResult::InvalidValue;
    }
    unsafe { out_len.write(value.len()) };
    if out.is_null() || cap < value.len() {
        return if out.is_null() && cap == 0 {
            RuuahHostResult::Success
        } else {
            RuuahHostResult::InvalidValue
        };
    }
    unsafe { std::ptr::copy_nonoverlapping(value.as_ptr(), out, value.len()) };
    RuuahHostResult::Success
}

/// Loads the workflow templates from `dir`, or from `~/.ruuah/workflows` when NULL.
/// Broken files are skipped and their errors kept on the handle
/// (`ruuah_workflows_errors`) -- one bad template never hides the rest. Returns NULL
/// only when the out-param itself is unusable; an empty or missing directory is a
/// valid, empty handle.
///
/// # Safety
/// `dir`, if non-NULL, must be a NUL-terminated path; `out` must be valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_workflows_load(
    dir: *const c_char,
    out: *mut *mut RuuahWorkflows,
) -> RuuahHostResult {
    if out.is_null() {
        return RuuahHostResult::InvalidValue;
    }
    let dir = if dir.is_null() {
        let Some(home) = std::env::var_os("HOME") else {
            unsafe { out.write(std::ptr::null_mut()) };
            return RuuahHostResult::InvalidValue;
        };
        Path::new(&home).join(".ruuah").join("workflows")
    } else {
        match unsafe { CStr::from_ptr(dir) }.to_str() {
            Ok(path) => Path::new(path).to_path_buf(),
            Err(_) => {
                unsafe { out.write(std::ptr::null_mut()) };
                return RuuahHostResult::InvalidValue;
            }
        }
    };
    let (workflows, errors) = workflow::load_dir(&dir);
    let handle = Box::new(RuuahWorkflows { workflows, errors: errors.join("\n") });
    unsafe { out.write(Box::into_raw(handle)) };
    RuuahHostResult::Success
}

/// # Safety
/// `handle` must be NULL or a live handle from `ruuah_workflows_load`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_workflows_free(handle: *mut RuuahWorkflows) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// # Safety
/// `handle` must be a live handle from `ruuah_workflows_load`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_workflows_count(handle: *const RuuahWorkflows) -> u32 {
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
pub unsafe extern "C" fn ruuah_workflows_errors(
    handle: *const RuuahWorkflows,
    out: *mut u8,
    cap: usize,
    out_len: *mut usize,
) -> RuuahHostResult {
    if handle.is_null() {
        return RuuahHostResult::InvalidValue;
    }
    copy_out(&unsafe { &*handle }.errors, out, cap, out_len)
}

/// One workflow's field: 0 name, 1 description, 2 command. The buffer protocol is
/// row_text's: NULL out sizes, short buffers refuse with the needed length.
///
/// # Safety
/// `handle` live; `out`/`out_len` per the buffer protocol.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_workflow_field(
    handle: *const RuuahWorkflows,
    index: u32,
    field: u32,
    out: *mut u8,
    cap: usize,
    out_len: *mut usize,
) -> RuuahHostResult {
    if handle.is_null() {
        return RuuahHostResult::InvalidValue;
    }
    let Some(workflow) = unsafe { &*handle }.workflows.get(index as usize) else {
        return RuuahHostResult::InvalidValue;
    };
    let value = match field {
        RUUAH_WORKFLOW_NAME => &workflow.name,
        RUUAH_WORKFLOW_DESCRIPTION => &workflow.description,
        RUUAH_WORKFLOW_COMMAND => &workflow.command,
        _ => return RuuahHostResult::InvalidValue,
    };
    copy_out(value, out, cap, out_len)
}

/// # Safety
/// `handle` must be a live handle from `ruuah_workflows_load`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_workflow_arg_count(
    handle: *const RuuahWorkflows,
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
pub unsafe extern "C" fn ruuah_workflow_arg(
    handle: *const RuuahWorkflows,
    index: u32,
    arg_index: u32,
    field: u32,
    out: *mut u8,
    cap: usize,
    out_len: *mut usize,
) -> RuuahHostResult {
    if handle.is_null() {
        return RuuahHostResult::InvalidValue;
    }
    let Some(arg) = unsafe { &*handle }
        .workflows
        .get(index as usize)
        .and_then(|workflow| workflow.args.get(arg_index as usize))
    else {
        return RuuahHostResult::InvalidValue;
    };
    let value = match field {
        RUUAH_WORKFLOW_NAME => &arg.name,
        RUUAH_WORKFLOW_DESCRIPTION => &arg.description,
        RUUAH_WORKFLOW_ARG_DEFAULT => match &arg.default {
            Some(default) => default,
            None => {
                if !out_len.is_null() {
                    unsafe { out_len.write(0) };
                }
                return RuuahHostResult::Ignored;
            }
        },
        _ => return RuuahHostResult::InvalidValue,
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
pub unsafe extern "C" fn ruuah_workflow_render(
    handle: *const RuuahWorkflows,
    index: u32,
    args_blob: *const u8,
    blob_len: usize,
    out: *mut u8,
    cap: usize,
    out_len: *mut usize,
) -> RuuahHostResult {
    if handle.is_null() || (args_blob.is_null() && blob_len != 0) {
        return RuuahHostResult::InvalidValue;
    }
    let Some(workflow) = unsafe { &*handle }.workflows.get(index as usize) else {
        return RuuahHostResult::InvalidValue;
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
            return RuuahHostResult::InvalidValue;
        };
        values.push((name.to_string(), value.to_string()));
    }
    match workflow::render(&workflow.command, &values) {
        Ok(rendered) => copy_out(&rendered, out, cap, out_len),
        Err(_) => RuuahHostResult::InvalidValue,
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
/// `host` must be a live handle from `ruuah_host_spawn`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_host_scroll(host: *mut RuuahHost, rows: i32) -> RuuahHostResult {
    if host.is_null() {
        return RuuahHostResult::InvalidValue;
    }
    let host = unsafe { &mut *host };
    if rows == i32::MIN {
        host.host.scroll_to_bottom();
    } else {
        host.host.scroll(rows);
    }
    RuuahHostResult::Success
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
    let Some(mut renderer) = build_renderer(
        host.font_size,
        cols,
        rows,
        host.font_family.as_deref(),
        host.ligatures,
    ) else {
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
    font_family: *const c_char,
    out_width: *mut u32,
    out_height: *mut u32,
) -> RuuahHostResult {
    if out_width.is_null() || out_height.is_null() || !(font_size > 0.0) {
        return RuuahHostResult::InvalidValue;
    }
    let family = if font_family.is_null() {
        None
    } else {
        unsafe { CStr::from_ptr(font_family) }.to_str().ok()
    };
    let Ok(fonts) = FontStack::with_primary(family, font_size) else {
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
    let Some(mut renderer) = build_renderer(
        font_size,
        cols,
        rows,
        host.font_family.as_deref(),
        host.ligatures,
    ) else {
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
pub unsafe extern "C" fn ruuah_host_next_event(
    host: *mut RuuahHost,
    kind: *mut u32,
    out: *mut u8,
    cap: usize,
    len: *mut usize,
) -> RuuahHostResult {
    use ruuah_vt_core::events::Event;

    if host.is_null() || kind.is_null() || len.is_null() || (out.is_null() && cap != 0) {
        return RuuahHostResult::InvalidValue;
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
        return RuuahHostResult::Success;
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
    };
    unsafe {
        kind.write(code);
        len.write(payload.len());
    }
    if payload.len() > cap {
        // Sizing call: the event stays queued for the fetch that fits it.
        return RuuahHostResult::Success;
    }
    if !payload.is_empty() {
        unsafe { std::ptr::copy_nonoverlapping(payload.as_ptr(), out, payload.len()) };
    }
    host.pending_events.pop_front();
    RuuahHostResult::Success
}

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
    let font_family = config
        .font_family
        .as_deref()
        .and_then(|text| CString::new(text).ok());
    let error = config.error.as_deref().and_then(|text| CString::new(text).ok());
    let handle = Box::new(RuuahConfig { config, shell, font_family, error });
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

/// The configured lead font family, or NULL when unset. Borrowed: valid until
/// `ruuah_config_free`.
///
/// # Safety
/// `config` must be NULL or a live handle from `ruuah_config_load`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ruuah_config_font_family(config: *const RuuahConfig) -> *const c_char {
    if config.is_null() {
        return std::ptr::null();
    }
    match &unsafe { &*config }.font_family {
        Some(family) => family.as_ptr(),
        None => std::ptr::null(),
    }
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
