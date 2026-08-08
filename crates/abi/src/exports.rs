//! Purpose: the C entry points, implemented on `mind2t-vt-core`.
//! Public surface: the `ghostty_*` functions a consumer links against.
//! Why this file: this is the drop-in claim made executable. Everything here is a thin
//!   translation of a call the core already answers, and it is thin on purpose -- a fat ABI
//!   layer would be a second implementation of the terminal, which is the one thing the
//!   differential harness could not then catch.
//! NOT responsible for: terminal behaviour (the core), or layout (`types.rs`).
//! Test strategy: `tests/differential.rs` drives the same bytes through these functions and
//!   through the core's Rust API and requires the two snapshots to be identical, with a
//!   control that proves the comparison can fail.

use std::collections::hash_map::DefaultHasher;
use std::ffi::c_void;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, MutexGuard};

use mind2t_vt_snapshot::{Color, Row, Screen, Snapshot, Underline, Wide};

use mind2t_vt_abi_types::*;

/// The boxed terminal a `GhosttyTerminal` handle points at.
///
/// The read entry points must be reads in the Rust-model sense, because they are reads in the
/// oracle: `ghostty_terminal_get` mutates nothing there (`terminal.zig`, `getTyped`), so a C
/// consumer may read from two threads at once and may hold a grid ref across an interleaved
/// read -- refs die on the next *update*, not the next read (`grid_ref.h`). The audit's
/// findings 14 and 22 were this port conjuring `&mut Terminal` on those paths to fill the
/// cached view lazily. The cache is therefore interior-mutable: readers share the terminal
/// and take the lock only for the duration of one call, and only the mutating entry points
/// (`vt_write`, `resize`) take the terminal exclusively. `tests/soundness.rs` holds both
/// Miri controls, each seen to fail against the `&mut` version.
pub struct Terminal {
    core: mind2t_vt_core::Terminal,
    view: Mutex<Option<Snapshot>>,
}

impl Terminal {
    /// Locks the cache, filling it if a write emptied it. The two ref-minting entry points
    /// (`terminal_get`, `grid_ref`) use this.
    fn view_filled(&self) -> MutexGuard<'_, Option<Snapshot>> {
        let mut guard = self.view.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            let mut view = self.core.snapshot();
            // The damage rides along so `ROW_DATA_DIRTY` can answer (finding 28).
            view.damage = self.core.damage();
            *guard = Some(view);
        }
        guard
    }

    /// Locks the cache without filling it. A reader reached through a grid ref sees the view
    /// that existed when its ref was minted -- or nothing, if a write killed the ref, which
    /// is exactly when the contract says the ref is dead.
    fn view_current(&self) -> MutexGuard<'_, Option<Snapshot>> {
        self.view.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Writers hold the terminal exclusively, so this bypasses the lock.
    fn invalidate(&mut self) {
        *self.view.get_mut().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// The row at an absolute SCREEN-space index: history first, then the active area.
fn row_at(view: &Snapshot, y: u16) -> Option<&Row> {
    let y = usize::from(y);
    if y < view.history.len() {
        view.history.get(y)
    } else {
        view.grid.get(y - view.history.len())
    }
}

/// # Safety
/// `handle` must be a pointer returned by `ghostty_terminal_new` and not yet freed, with no
/// concurrent call in flight -- this is the exclusive access the mutating entry points need.
unsafe fn terminal<'a>(handle: GhosttyTerminal) -> Option<&'a mut Terminal> {
    if handle.is_null() {
        return None;
    }
    Some(unsafe { &mut *handle.cast::<Terminal>() })
}

/// # Safety
/// `handle` must be a pointer returned by `ghostty_terminal_new` and not yet freed. Shared:
/// the read entry points use this, so concurrent readers and live grid refs stay legal.
unsafe fn terminal_shared<'a>(handle: GhosttyTerminal) -> Option<&'a Terminal> {
    if handle.is_null() {
        return None;
    }
    Some(unsafe { &*handle.cast::<Terminal>() })
}

/// # Safety
/// `node` must be a pointer stored by `ghostty_terminal_grid_ref`.
unsafe fn terminal_ref<'a>(node: *mut c_void) -> Option<&'a Terminal> {
    if node.is_null() {
        return None;
    }
    Some(unsafe { &*node.cast::<Terminal>() })
}

#[unsafe(no_mangle)]
/// # Safety
/// `out`, if non-null, must be valid for writing one pointer. A returned handle is owned by
/// the caller and must be released with `ghostty_terminal_free`, exactly once.
pub unsafe extern "C" fn ghostty_terminal_new(
    _allocator: *const GhosttyAllocator,
    out: *mut GhosttyTerminal,
    options: GhosttyTerminalOptions,
) -> GhosttyResult {
    if out.is_null() {
        return GHOSTTY_INVALID_VALUE;
    }
    // The out-param is cleared on failure, not left alone. Measured on the real library
    // (`oracle.rs::a_failed_creation_nulls_the_out_param`, and `terminal.zig:328`): a consumer
    // that checks the handle instead of the return code would otherwise read whatever was in
    // that variable before the call, and a stale non-null value looks like success.
    if options.cols == 0 || options.rows == 0 {
        unsafe { *out = std::ptr::null_mut() };
        return GHOSTTY_INVALID_VALUE;
    }
    let terminal = Box::new(Terminal {
        core: mind2t_vt_core::Terminal::with_scrollback(
            options.cols,
            options.rows,
            options.max_scrollback,
        ),
        view: Mutex::new(None),
    });
    unsafe { *out = Box::into_raw(terminal).cast::<c_void>() };
    GHOSTTY_SUCCESS
}

#[unsafe(no_mangle)]
/// # Safety
/// `handle` must be null or a pointer returned by `ghostty_terminal_new` that has not been
/// freed. No other call on this terminal may be in flight, and no grid ref minted from it
/// may be used afterwards.
pub unsafe extern "C" fn ghostty_terminal_free(handle: GhosttyTerminal) {
    if handle.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(handle.cast::<Terminal>()) });
}

#[unsafe(no_mangle)]
/// # Safety
/// `handle` must be null or a live handle from `ghostty_terminal_new`. This is a WRITE: the
/// caller must serialize it against every other call on the same terminal, and every grid
/// ref minted before it is dead afterwards. `bytes`, if non-null, must be valid for `len`
/// reads.
pub unsafe extern "C" fn ghostty_terminal_vt_write(
    handle: GhosttyTerminal,
    bytes: *const u8,
    len: usize,
) {
    let Some(terminal) = (unsafe { terminal(handle) }) else {
        return;
    };
    if bytes.is_null() || len == 0 {
        return;
    }
    terminal
        .core
        .write(unsafe { std::slice::from_raw_parts(bytes, len) });
    terminal.invalidate();
}

#[unsafe(no_mangle)]
/// # Safety
/// `handle` must be null or a live handle from `ghostty_terminal_new`. This is a WRITE: the
/// caller must serialize it against every other call on the same terminal, and every grid
/// ref minted before it is dead afterwards.
pub unsafe extern "C" fn ghostty_terminal_resize(
    handle: GhosttyTerminal,
    cols: u16,
    rows: u16,
    _cell_width_px: u32,
    _cell_height_px: u32,
) -> GhosttyResult {
    let Some(terminal) = (unsafe { terminal(handle) }) else {
        return GHOSTTY_INVALID_VALUE;
    };
    if cols == 0 || rows == 0 {
        return GHOSTTY_INVALID_VALUE;
    }
    terminal.core.resize(cols, rows);
    terminal.invalidate();
    GHOSTTY_SUCCESS
}

#[unsafe(no_mangle)]
/// # Safety
/// `handle` must be null or a live handle from `ghostty_terminal_new`. This is a READ: it
/// may run concurrently with other reads, but the caller must serialize it against writes.
/// `out` must be valid for writing the type the requested `data` documents.
pub unsafe extern "C" fn ghostty_terminal_get(
    handle: GhosttyTerminal,
    data: GhosttyTerminalData,
    out: *mut c_void,
) -> GhosttyResult {
    let Some(terminal) = (unsafe { terminal_shared(handle) }) else {
        return GHOSTTY_INVALID_VALUE;
    };
    if out.is_null() {
        return GHOSTTY_INVALID_VALUE;
    }
    let guard = terminal.view_filled();
    let Some(view) = guard.as_ref() else {
        return GHOSTTY_INVALID_VALUE;
    };

    unsafe {
        match data {
            GHOSTTY_TERMINAL_DATA_COLS => *out.cast::<u16>() = view.cols,
            GHOSTTY_TERMINAL_DATA_ROWS => *out.cast::<u16>() = view.rows,
            GHOSTTY_TERMINAL_DATA_CURSOR_X => *out.cast::<u16>() = view.cursor.x,
            GHOSTTY_TERMINAL_DATA_CURSOR_Y => *out.cast::<u16>() = view.cursor.y,
            GHOSTTY_TERMINAL_DATA_CURSOR_PENDING_WRAP => {
                *out.cast::<bool>() = view.cursor.pending_wrap
            }
            GHOSTTY_TERMINAL_DATA_CURSOR_VISIBLE => *out.cast::<bool>() = view.cursor.visible,
            GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN => {
                *out.cast::<GhosttyTerminalScreen>() = match view.screen {
                    Screen::Primary => GHOSTTY_TERMINAL_SCREEN_PRIMARY,
                    Screen::Alternate => GHOSTTY_TERMINAL_SCREEN_ALTERNATE,
                }
            }
            GHOSTTY_TERMINAL_DATA_CURSOR_STYLE => {
                let style = out.cast::<GhosttyStyle>();
                *style = pack_style(&view.cursor.style);
            }
            GHOSTTY_TERMINAL_DATA_TOTAL_ROWS => {
                *out.cast::<usize>() = view.history.len() + usize::from(view.rows)
            }
            GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS => *out.cast::<usize>() = view.history.len(),
            // Borrowed, exactly as the header specifies: the bytes live in the cached view
            // and the contract's "valid until the next vt_write or reset" is that cache's
            // own lifetime, since a write invalidates it. Empty answers with a NULL pointer
            // and zero length -- the oracle's own "no pwd has been set" state, and the
            // reason `pwd` is never `Option` on either side of the harness.
            GHOSTTY_TERMINAL_DATA_TITLE => {
                *out.cast::<GhosttyString>() = GhosttyString {
                    ptr: if view.title.is_empty() {
                        std::ptr::null()
                    } else {
                        view.title.as_ptr()
                    },
                    len: view.title.len(),
                }
            }
            GHOSTTY_TERMINAL_DATA_PWD => {
                *out.cast::<GhosttyString>() = GhosttyString {
                    ptr: if view.pwd.is_empty() { std::ptr::null() } else { view.pwd.as_ptr() },
                    len: view.pwd.len(),
                }
            }
            // Effective colours: OSC override or embedder default, NO_VALUE when
            // neither exists -- the oracle's exact result contract. The cursor getter
            // deliberately has no foreground fallback; that rule belongs to the OSC
            // query reply alone.
            GHOSTTY_TERMINAL_DATA_COLOR_FOREGROUND
            | GHOSTTY_TERMINAL_DATA_COLOR_BACKGROUND
            | GHOSTTY_TERMINAL_DATA_COLOR_CURSOR => {
                let color = match data {
                    GHOSTTY_TERMINAL_DATA_COLOR_FOREGROUND => view.colors.foreground,
                    GHOSTTY_TERMINAL_DATA_COLOR_BACKGROUND => view.colors.background,
                    _ => view.colors.cursor,
                };
                let Some(c) = color else { return GHOSTTY_NO_VALUE };
                *out.cast::<GhosttyColorRgb>() = GhosttyColorRgb { r: c.r, g: c.g, b: c.b };
            }
            // No embedder default surface exists yet (the OPT setters are unbuilt),
            // so the dynamic defaults are honestly absent rather than zero-filled.
            GHOSTTY_TERMINAL_DATA_COLOR_FOREGROUND_DEFAULT
            | GHOSTTY_TERMINAL_DATA_COLOR_BACKGROUND_DEFAULT
            | GHOSTTY_TERMINAL_DATA_COLOR_CURSOR_DEFAULT => return GHOSTTY_NO_VALUE,
            GHOSTTY_TERMINAL_DATA_COLOR_PALETTE => {
                let table = out.cast::<[GhosttyColorRgb; 256]>();
                for (slot, c) in (*table).iter_mut().zip(view.colors.palette.iter()) {
                    *slot = GhosttyColorRgb { r: c.r, g: c.g, b: c.b };
                }
            }
            GHOSTTY_TERMINAL_DATA_COLOR_PALETTE_DEFAULT => {
                let table = out.cast::<[GhosttyColorRgb; 256]>();
                for (slot, c) in
                    (*table).iter_mut().zip(mind2t_vt_snapshot::default_palette().iter())
                {
                    *slot = GhosttyColorRgb { r: c.r, g: c.g, b: c.b };
                }
            }
            _ => return GHOSTTY_INVALID_VALUE,
        }
    }
    GHOSTTY_SUCCESS
}

#[unsafe(no_mangle)]
/// # Safety
/// `handle` must be null or a live handle from `ghostty_terminal_new`. This is a READ and
/// may run concurrently with other reads. `out_value`, if non-null, must be valid for
/// writing one `bool`.
///
/// The modes this core tracks as queryable state: DEC 2004 (bracketed paste -- what a
/// GUI host consults before every paste), DEC 2026 (synchronized output -- what the
/// pump's publish gate reads), and the mouse family 9/1000/1002/1003 (event kinds),
/// 1005/1006/1015/1016 (report formats) and 1007 (alternate scroll, default ON) --
/// raw bits as set/reset last left them, matching the oracle's own table. Anything
/// else answers `GHOSTTY_INVALID_VALUE` rather than a guessed `false`: a wrong "off"
/// would make a consumer silently drop a feature, which is worse than an error it can
/// see. Extend per mode, each with its own corpus pin, as consumers need them.
pub unsafe extern "C" fn ghostty_terminal_mode_get(
    handle: GhosttyTerminal,
    mode: GhosttyMode,
    out_value: *mut bool,
) -> GhosttyResult {
    let Some(terminal) = (unsafe { terminal_shared(handle) }) else {
        return GHOSTTY_INVALID_VALUE;
    };
    if out_value.is_null() {
        return GHOSTTY_INVALID_VALUE;
    }
    let guard = terminal.view_filled();
    let Some(view) = guard.as_ref() else {
        return GHOSTTY_INVALID_VALUE;
    };

    // Bit 15 of a packed GhosttyMode distinguishes ANSI (set) from DEC private (clear);
    // 2004 is DEC private, so the packed form IS the number.
    match mode {
        // ANSI modes arrive with bit 15 set (ghostty_mode_new(value, true)) - the
        // oracle's packing, mirrored so GHOSTTY_MODE_INSERT and friends work here.
        0x8002 => {
            unsafe { *out_value = view.modes.kam };
            GHOSTTY_SUCCESS
        }
        0x8004 => {
            unsafe { *out_value = view.modes.insert };
            GHOSTTY_SUCCESS
        }
        0x800C => {
            unsafe { *out_value = view.modes.send_receive };
            GHOSTTY_SUCCESS
        }
        0x8014 => {
            unsafe { *out_value = view.modes.linefeed };
            GHOSTTY_SUCCESS
        }
        4 => {
            unsafe { *out_value = view.modes.slow_scroll };
            GHOSTTY_SUCCESS
        }
        5 => {
            unsafe { *out_value = view.modes.reverse_colors };
            GHOSTTY_SUCCESS
        }
        67 => {
            unsafe { *out_value = view.modes.backarrow };
            GHOSTTY_SUCCESS
        }
        69 => {
            unsafe { *out_value = view.modes.left_right_margin };
            GHOSTTY_SUCCESS
        }
        // DECCOLM and its gate, mode 40: both in the oracle's own table, so the
        // drop-in answers the same queries (the corpus reads them through here).
        3 => {
            unsafe { *out_value = view.modes.column_132 };
            GHOSTTY_SUCCESS
        }
        40 => {
            unsafe { *out_value = view.modes.enable_mode_3 };
            GHOSTTY_SUCCESS
        }
        2004 => {
            unsafe { *out_value = view.modes.bracketed_paste };
            GHOSTTY_SUCCESS
        }
        2026 => {
            unsafe { *out_value = view.modes.synchronized_output };
            GHOSTTY_SUCCESS
        }
        // The mouse family: raw bits, the same table DECRQM answers from. A GUI host
        // reads 1000/1002/1003 to decide whether a pointer event becomes bytes at all,
        // 1005/1006/1015/1016 to pick the encoding, and 1007 to route the wheel on the
        // alternate screen.
        9 => {
            unsafe { *out_value = view.modes.mouse_event_x10 };
            GHOSTTY_SUCCESS
        }
        1000 => {
            unsafe { *out_value = view.modes.mouse_event_normal };
            GHOSTTY_SUCCESS
        }
        1002 => {
            unsafe { *out_value = view.modes.mouse_event_button };
            GHOSTTY_SUCCESS
        }
        1003 => {
            unsafe { *out_value = view.modes.mouse_event_any };
            GHOSTTY_SUCCESS
        }
        1005 => {
            unsafe { *out_value = view.modes.mouse_format_utf8 };
            GHOSTTY_SUCCESS
        }
        1006 => {
            unsafe { *out_value = view.modes.mouse_format_sgr };
            GHOSTTY_SUCCESS
        }
        1015 => {
            unsafe { *out_value = view.modes.mouse_format_urxvt };
            GHOSTTY_SUCCESS
        }
        1016 => {
            unsafe { *out_value = view.modes.mouse_format_sgr_pixels };
            GHOSTTY_SUCCESS
        }
        1007 => {
            unsafe { *out_value = view.modes.mouse_alternate_scroll };
            GHOSTTY_SUCCESS
        }
        // DECCKM, tracked for the host's arrow and alternate-scroll byte forms.
        1 => {
            unsafe { *out_value = view.modes.cursor_keys };
            GHOSTTY_SUCCESS
        }
        // The key-encoder trio (66 keypad application, 1035 numlock-ignores-keypad,
        // 1036 alt-sends-ESC), tracked for the host's key encoding.
        66 => {
            unsafe { *out_value = view.modes.keypad_keys };
            GHOSTTY_SUCCESS
        }
        1035 => {
            unsafe { *out_value = view.modes.ignore_keypad_with_numlock };
            GHOSTTY_SUCCESS
        }
        1036 => {
            unsafe { *out_value = view.modes.alt_esc_prefix };
            GHOSTTY_SUCCESS
        }
        _ => GHOSTTY_INVALID_VALUE,
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// `handle` must be null or a live handle from `ghostty_terminal_new`. This is a READ and
/// may run concurrently with other reads. `out`, if non-null, must be valid for writing a
/// `GhosttyGridRef`. The ref written is valid until the next write to the terminal.
pub unsafe extern "C" fn ghostty_terminal_grid_ref(
    handle: GhosttyTerminal,
    point: GhosttyPoint,
    out: *mut GhosttyGridRef,
) -> GhosttyResult {
    let Some(terminal) = (unsafe { terminal_shared(handle) }) else {
        return GHOSTTY_INVALID_VALUE;
    };
    let guard = terminal.view_filled();
    let Some(view) = guard.as_ref() else {
        return GHOSTTY_INVALID_VALUE;
    };

    let coordinate = unsafe { point.value.coordinate };
    // The viewport is the active area here: this core tracks no scroll position, so there is
    // no window to be offset from it.
    let absolute = match point.tag {
        GHOSTTY_POINT_TAG_HISTORY | GHOSTTY_POINT_TAG_SCREEN => u64::from(coordinate.y),
        GHOSTTY_POINT_TAG_ACTIVE | GHOSTTY_POINT_TAG_VIEWPORT => {
            view.history.len() as u64 + u64::from(coordinate.y)
        }
        _ => return GHOSTTY_INVALID_VALUE,
    };

    let total = view.history.len() as u64 + u64::from(view.rows);
    if absolute >= total || absolute > u64::from(u16::MAX) || coordinate.x >= view.cols {
        return GHOSTTY_INVALID_VALUE;
    }

    // NULL out is the validate-only idiom, honoured after every check above -- the header
    // says "(may be NULL)" and the oracle skips the write, never the validation (finding 23).
    if !out.is_null() {
        unsafe {
            *out = GhosttyGridRef {
                size: size_of::<GhosttyGridRef>(),
                // The raw handle, not a reference-derived pointer: the ref must keep the
                // handle's full provenance so it survives later shared reads (finding 22).
                node: handle,
                x: coordinate.x,
                y: absolute as u16,
            };
        }
    }
    GHOSTTY_SUCCESS
}

#[unsafe(no_mangle)]
/// # Safety
/// `grid_ref` must be null or point at a ref produced by `ghostty_terminal_grid_ref` whose
/// terminal is still alive. This is a READ and may run concurrently with other reads.
/// `out`, if non-null, must be valid for writing a `GhosttyCell`.
pub unsafe extern "C" fn ghostty_grid_ref_cell(
    grid_ref: *const GhosttyGridRef,
    out: *mut GhosttyCell,
) -> GhosttyResult {
    let Some((terminal, grid_ref)) = (unsafe { resolve(grid_ref) }) else {
        return GHOSTTY_INVALID_VALUE;
    };
    let guard = terminal.view_current();
    let Some(cell) = guard
        .as_ref()
        .and_then(|view| row_at(view, grid_ref.y))
        .and_then(|row| row.cells.get(usize::from(grid_ref.x)))
    else {
        return GHOSTTY_INVALID_VALUE;
    };
    // NULL out validates the ref without reading it, matching the oracle (finding 23).
    if !out.is_null() {
        unsafe { *out = pack_cell(cell) };
    }
    GHOSTTY_SUCCESS
}

#[unsafe(no_mangle)]
/// # Safety
/// `grid_ref` must be null or point at a ref produced by `ghostty_terminal_grid_ref` whose
/// terminal is still alive. This is a READ and may run concurrently with other reads.
/// `out`, if non-null, must be valid for writing a `GhosttyRow`.
pub unsafe extern "C" fn ghostty_grid_ref_row(
    grid_ref: *const GhosttyGridRef,
    out: *mut GhosttyRow,
) -> GhosttyResult {
    let Some((terminal, grid_ref)) = (unsafe { resolve(grid_ref) }) else {
        return GHOSTTY_INVALID_VALUE;
    };
    let guard = terminal.view_current();
    let Some(view) = guard.as_ref() else {
        return GHOSTTY_INVALID_VALUE;
    };
    let Some(row) = row_at(view, grid_ref.y) else {
        return GHOSTTY_INVALID_VALUE;
    };
    // Damage is tracked for the active area; a history row is settled and never dirty.
    let dirty = usize::from(grid_ref.y)
        .checked_sub(view.history.len())
        .and_then(|y| view.damage.as_ref().and_then(|damage| damage.rows.get(y)))
        .copied()
        .unwrap_or(false);
    // NULL out validates the ref without reading it, matching the oracle (finding 23).
    if !out.is_null() {
        unsafe { *out = pack_row(row, dirty) };
    }
    GHOSTTY_SUCCESS
}

#[unsafe(no_mangle)]
/// # Safety
/// `grid_ref` must be null or point at a ref produced by `ghostty_terminal_grid_ref` whose
/// terminal is still alive. This is a READ and may run concurrently with other reads.
/// `buf`, if non-null, must be valid for `buf_len` writes of `u32`; `out_len` must be valid
/// for writing one `usize`.
pub unsafe extern "C" fn ghostty_grid_ref_graphemes(
    grid_ref: *const GhosttyGridRef,
    buf: *mut u32,
    buf_len: usize,
    out_len: *mut usize,
) -> GhosttyResult {
    let Some((terminal, grid_ref)) = (unsafe { resolve(grid_ref) }) else {
        return GHOSTTY_INVALID_VALUE;
    };
    if out_len.is_null() {
        return GHOSTTY_INVALID_VALUE;
    }
    let guard = terminal.view_current();
    let Some(cell) = guard
        .as_ref()
        .and_then(|view| row_at(view, grid_ref.y))
        .and_then(|row| row.cells.get(usize::from(grid_ref.x)))
    else {
        return GHOSTTY_INVALID_VALUE;
    };

    let codepoints: Vec<u32> = cell.text.chars().map(u32::from).collect();
    unsafe { *out_len = codepoints.len() };
    if codepoints.is_empty() {
        return GHOSTTY_SUCCESS;
    }
    if buf.is_null() || buf_len < codepoints.len() {
        return GHOSTTY_OUT_OF_SPACE;
    }
    unsafe { std::ptr::copy_nonoverlapping(codepoints.as_ptr(), buf, codepoints.len()) };
    GHOSTTY_SUCCESS
}

#[unsafe(no_mangle)]
/// # Safety
/// `grid_ref` must be null or point at a ref produced by `ghostty_terminal_grid_ref` whose
/// terminal is still alive. This is a READ and may run concurrently with other reads.
/// `out`, if non-null, must be valid for writing a `GhosttyStyle`.
pub unsafe extern "C" fn ghostty_grid_ref_style(
    grid_ref: *const GhosttyGridRef,
    out: *mut GhosttyStyle,
) -> GhosttyResult {
    let Some((terminal, grid_ref)) = (unsafe { resolve(grid_ref) }) else {
        return GHOSTTY_INVALID_VALUE;
    };
    let guard = terminal.view_current();
    let Some(cell) = guard
        .as_ref()
        .and_then(|view| row_at(view, grid_ref.y))
        .and_then(|row| row.cells.get(usize::from(grid_ref.x)))
    else {
        return GHOSTTY_INVALID_VALUE;
    };
    // NULL out validates the ref without reading it, matching the oracle (finding 23).
    if !out.is_null() {
        unsafe { *out = pack_style(&cell.style) };
    }
    GHOSTTY_SUCCESS
}

#[unsafe(no_mangle)]
/// # Safety
/// `cell` is a plain value; the only obligation is `out`, which must be valid for writing
/// the type the requested `data` documents.
pub unsafe extern "C" fn ghostty_cell_get(
    cell: GhosttyCell,
    data: GhosttyCellData,
    out: *mut c_void,
) -> GhosttyResult {
    if out.is_null() {
        return GHOSTTY_INVALID_VALUE;
    }
    unsafe {
        match data {
            GHOSTTY_CELL_DATA_CODEPOINT => *out.cast::<u32>() = (cell & 0x1F_FFFF) as u32,
            GHOSTTY_CELL_DATA_CONTENT_TAG => {
                // A multi-codepoint cluster wears the grapheme tag, matching upstream's rule
                // (`appendGrapheme` flips the tag the moment a continuation codepoint lands).
                // The bg-color tags are the one deliberate omission: Ghostty keeps a
                // background-colour-only cell out of its style table and tags it; here the
                // background always lives in the style, so a consumer reading the style gets
                // the same answer without the second representation.
                *out.cast::<GhosttyCellContentTag>() = if (cell >> 42) & 1 != 0 {
                    GHOSTTY_CELL_CONTENT_CODEPOINT_GRAPHEME
                } else {
                    GHOSTTY_CELL_CONTENT_CODEPOINT
                }
            }
            GHOSTTY_CELL_DATA_WIDE => {
                *out.cast::<GhosttyCellWide>() = ((cell >> 21) & 0b11) as GhosttyCellWide
            }
            GHOSTTY_CELL_DATA_SEMANTIC_CONTENT => {
                *out.cast::<GhosttyCellSemanticContent>() =
                    ((cell >> 23) & 0b11) as GhosttyCellSemanticContent
            }
            GHOSTTY_CELL_DATA_STYLE_ID => *out.cast::<u16>() = ((cell >> 25) & 0xFFFF) as u16,
            GHOSTTY_CELL_DATA_HAS_TEXT => *out.cast::<bool>() = (cell & 0x1F_FFFF) != 0,
            GHOSTTY_CELL_DATA_HAS_STYLING => *out.cast::<bool>() = (cell >> 41) & 1 != 0,
            // Neither is tracked by this core yet; reporting false is the truth about it.
            GHOSTTY_CELL_DATA_HAS_HYPERLINK | GHOSTTY_CELL_DATA_PROTECTED => {
                *out.cast::<bool>() = false
            }
            _ => return GHOSTTY_INVALID_VALUE,
        }
    }
    GHOSTTY_SUCCESS
}

#[unsafe(no_mangle)]
/// # Safety
/// `row` is a plain value; the only obligation is `out`, which must be valid for writing
/// the type the requested `data` documents.
pub unsafe extern "C" fn ghostty_row_get(
    row: GhosttyRow,
    data: GhosttyRowData,
    out: *mut c_void,
) -> GhosttyResult {
    if out.is_null() {
        return GHOSTTY_INVALID_VALUE;
    }
    unsafe {
        match data {
            GHOSTTY_ROW_DATA_WRAP => *out.cast::<bool>() = row & 1 != 0,
            GHOSTTY_ROW_DATA_WRAP_CONTINUATION => *out.cast::<bool>() = (row >> 1) & 1 != 0,
            GHOSTTY_ROW_DATA_GRAPHEME => *out.cast::<bool>() = (row >> 2) & 1 != 0,
            GHOSTTY_ROW_DATA_STYLED => *out.cast::<bool>() = (row >> 3) & 1 != 0,
            GHOSTTY_ROW_DATA_SEMANTIC_PROMPT => {
                *out.cast::<GhosttyRowSemanticPrompt>() =
                    ((row >> 4) & 0b11) as GhosttyRowSemanticPrompt
            }
            GHOSTTY_ROW_DATA_HYPERLINK => *out.cast::<bool>() = false,
            // What a renderer owes since damage was last cleared -- upstream reads the
            // row's own dirty bit (row.zig:122); here it rides bit 6 of the packed row
            // (finding 28).
            GHOSTTY_ROW_DATA_DIRTY => *out.cast::<bool>() = (row >> 6) & 1 != 0,
            _ => return GHOSTTY_INVALID_VALUE,
        }
    }
    GHOSTTY_SUCCESS
}

#[unsafe(no_mangle)]
/// # Safety
/// `out`, if non-null, must be valid for writing a `GhosttyStyle`.
pub unsafe extern "C" fn ghostty_style_default(out: *mut GhosttyStyle) {
    if out.is_null() {
        return;
    }
    unsafe { *out = pack_style(&mind2t_vt_snapshot::Style::DEFAULT) };
}

/// # Safety
/// `grid_ref` must point at a ref produced by `ghostty_terminal_grid_ref`.
unsafe fn resolve<'a>(grid_ref: *const GhosttyGridRef) -> Option<(&'a Terminal, GhosttyGridRef)> {
    if grid_ref.is_null() {
        return None;
    }
    let grid_ref = unsafe { *grid_ref };
    let terminal = unsafe { terminal_ref(grid_ref.node) }?;
    Some((terminal, grid_ref))
}

fn pack_cell(cell: &mind2t_vt_snapshot::Cell) -> GhosttyCell {
    let codepoint = u64::from(cell.text.chars().next().map(u32::from).unwrap_or(0)) & 0x1F_FFFF;
    let wide = match cell.wide {
        Wide::Narrow => 0u64,
        Wide::Wide => 1,
        Wide::SpacerTail => 2,
        Wide::SpacerHead => 3,
    };
    let semantic = match cell.semantic {
        mind2t_vt_snapshot::Semantic::Output => 0u64,
        mind2t_vt_snapshot::Semantic::Input => 1,
        mind2t_vt_snapshot::Semantic::Prompt => 2,
    };
    let styled = u64::from(!cell.style.is_default());
    let style_id = u64::from(style_id(&cell.style));
    let grapheme = u64::from(cell.text.chars().count() > 1);
    codepoint
        | (wide << 21)
        | (semantic << 23)
        | (style_id << 25)
        | (styled << 41)
        | (grapheme << 42)
}

/// The id a consumer reads back through `GHOSTTY_CELL_DATA_STYLE_ID`.
///
/// Upstream's id indexes its style table. This ABI has no table to index -- `ghostty_grid_ref_style`
/// resolves a style from the grid position, not from an id -- so the id here is derived from the
/// style itself. That keeps the two properties a consumer can actually rely on: the default style
/// is 0, matching upstream, and equal styles get equal ids. It does NOT promise upstream's exact
/// numbering, which is an allocation order no other implementation could reproduce anyway.
fn style_id(style: &mind2t_vt_snapshot::Style) -> u16 {
    if style.is_default() {
        return 0;
    }
    let mut hasher = DefaultHasher::new();
    style.hash(&mut hasher);
    // Fold into 1..=u16::MAX: zero is reserved for the default style above.
    let folded = (hasher.finish() % u64::from(u16::MAX)) as u16;
    folded + 1
}

fn pack_row(row: &Row, dirty: bool) -> GhosttyRow {
    let semantic = match row.semantic_prompt {
        mind2t_vt_snapshot::RowSemantic::None => 0u64,
        mind2t_vt_snapshot::RowSemantic::Prompt => 1,
        mind2t_vt_snapshot::RowSemantic::PromptContinuation => 2,
    };
    let grapheme = row.cells.iter().any(|cell| cell.text.chars().count() > 1);
    let styled = row.cells.iter().any(|cell| !cell.style.is_default());
    u64::from(row.wrap)
        | (u64::from(row.wrap_continuation) << 1)
        | (u64::from(grapheme) << 2)
        | (u64::from(styled) << 3)
        | (semantic << 4)
        | (u64::from(dirty) << 6)
}

/// A pure out-param's `.size` is written from the type, never read from the caller -- the
/// oracle whole-struct-assigns at every equivalent site and its own tests pass `undefined`
/// out-params, so the incoming field may be uninitialised memory (finding 15).
fn pack_style(style: &mind2t_vt_snapshot::Style) -> GhosttyStyle {
    GhosttyStyle {
        size: size_of::<GhosttyStyle>(),
        fg_color: pack_color(style.fg),
        bg_color: pack_color(style.bg),
        underline_color: pack_color(style.underline_color),
        bold: style.bold,
        italic: style.italic,
        faint: style.faint,
        blink: style.blink,
        inverse: style.inverse,
        invisible: style.invisible,
        strikethrough: style.strikethrough,
        overline: style.overline,
        underline: match style.underline {
            Underline::None => GHOSTTY_SGR_UNDERLINE_NONE,
            Underline::Single => GHOSTTY_SGR_UNDERLINE_SINGLE,
            Underline::Double => GHOSTTY_SGR_UNDERLINE_DOUBLE,
            Underline::Curly => GHOSTTY_SGR_UNDERLINE_CURLY,
            Underline::Dotted => GHOSTTY_SGR_UNDERLINE_DOTTED,
            Underline::Dashed => GHOSTTY_SGR_UNDERLINE_DASHED,
        },
    }
}

fn pack_color(color: Color) -> GhosttyStyleColor {
    match color {
        Color::Default => GhosttyStyleColor {
            tag: GHOSTTY_STYLE_COLOR_NONE,
            value: GhosttyStyleColorValue { _padding: 0 },
        },
        Color::Palette(index) => GhosttyStyleColor {
            tag: GHOSTTY_STYLE_COLOR_PALETTE,
            value: GhosttyStyleColorValue { palette: index },
        },
        Color::Rgb { r, g, b } => GhosttyStyleColor {
            tag: GHOSTTY_STYLE_COLOR_RGB,
            value: GhosttyStyleColorValue {
                rgb: GhosttyColorRgb { r, g, b },
            },
        },
    }
}
