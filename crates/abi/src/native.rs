//! Purpose: the ABI under **our own name**, `mind2t_vt_*`, declared by our own header.
//!
//! The library published two things until now and only one of them was ours. The symbols were
//! `ghostty_*`, and the header describing them was `vendor/include/ghostty/vt/*.h`, built out of a
//! Ghostty checkout - so a consumer linking this archive read somebody else's documentation to use
//! our code, and the archive announced itself with somebody else's name.
//!
//! Both surfaces ship now, and they are not alternatives:
//!
//! - **`mind2t_vt_*`** is the one we own, and `include/mind2t_vt.h` is our own declaration of it.
//!   New consumers link this.
//! - **`ghostty_*`** stays, because it is the thesis rather than a courtesy. This engine's
//!   correctness signal is a differential corpus run against the real `libghostty-vt`, and the
//!   claim that earns - that this archive can be swapped in behind something built for that ABI -
//!   is true only while the symbols an existing consumer looks up are still present. Removing them
//!   would not make the project more homegrown; it would delete the evidence that the homegrown
//!   part is correct.
//!
//! Every function here is a THIN forward with no logic of its own, and that is the whole design.
//! Two entry points into two copies of an implementation is how they drift, and no test would
//! catch it because each copy would be self-consistent. There is exactly one implementation, in
//! [`crate::exports`]; this module renames it and nothing else.

use std::ffi::c_void;

use mind2t_vt_abi_types::*;

use crate::exports;

/// Creates a terminal. `mind2t_vt_terminal_free` releases it.
///
/// # Safety
/// `out` must be a valid, writable pointer. The allocator argument is accepted and ignored,
/// exactly as in [`exports::ghostty_terminal_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_vt_terminal_new(
    allocator: *const GhosttyAllocator,
    out: *mut GhosttyTerminal,
    options: GhosttyTerminalOptions,
) -> GhosttyResult {
    unsafe { exports::ghostty_terminal_new(allocator, out, options) }
}

/// Releases a terminal created by `mind2t_vt_terminal_new`.
///
/// # Safety
/// `handle` must be a live handle from this library, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_vt_terminal_free(handle: GhosttyTerminal) {
    unsafe { exports::ghostty_terminal_free(handle) }
}

/// Feeds bytes to the parser.
///
/// # Safety
/// `handle` must be live; `bytes` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_vt_terminal_vt_write(
    handle: GhosttyTerminal,
    bytes: *const u8,
    len: usize,
) {
    unsafe { exports::ghostty_terminal_vt_write(handle, bytes, len) }
}

/// Resizes the terminal, reflowing as the core's own rules require.
///
/// # Safety
/// `handle` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_vt_terminal_resize(
    handle: GhosttyTerminal,
    cols: u16,
    rows: u16,
    cell_width_px: u32,
    cell_height_px: u32,
) -> GhosttyResult {
    unsafe { exports::ghostty_terminal_resize(handle, cols, rows, cell_width_px, cell_height_px) }
}

/// Reads one terminal-level datum into `out`.
///
/// # Safety
/// `handle` must be live; `out` must point at storage of the type `data` selects.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_vt_terminal_get(
    handle: GhosttyTerminal,
    data: GhosttyTerminalData,
    out: *mut c_void,
) -> GhosttyResult {
    unsafe { exports::ghostty_terminal_get(handle, data, out) }
}

/// Reads one tracked mode. An untracked mode answers INVALID_VALUE rather than a guessed `false`.
///
/// # Safety
/// `handle` must be live; `out_value` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_vt_terminal_mode_get(
    handle: GhosttyTerminal,
    mode: GhosttyMode,
    out_value: *mut bool,
) -> GhosttyResult {
    unsafe { exports::ghostty_terminal_mode_get(handle, mode, out_value) }
}

/// Takes a reference to one grid position.
///
/// # Safety
/// `handle` must be live; `out` must be writable with its `size` field set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_vt_terminal_grid_ref(
    handle: GhosttyTerminal,
    point: GhosttyPoint,
    out: *mut GhosttyGridRef,
) -> GhosttyResult {
    unsafe { exports::ghostty_terminal_grid_ref(handle, point, out) }
}

/// Reads the cell a grid reference points at.
///
/// # Safety
/// `grid_ref` must be a live reference; `out` may be null, in which case only validation runs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_vt_grid_ref_cell(
    grid_ref: *const GhosttyGridRef,
    out: *mut GhosttyCell,
) -> GhosttyResult {
    unsafe { exports::ghostty_grid_ref_cell(grid_ref, out) }
}

/// Reads the row a grid reference points at.
///
/// # Safety
/// `grid_ref` must be a live reference; `out` may be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_vt_grid_ref_row(
    grid_ref: *const GhosttyGridRef,
    out: *mut GhosttyRow,
) -> GhosttyResult {
    unsafe { exports::ghostty_grid_ref_row(grid_ref, out) }
}

/// Copies the cell's grapheme continuation codepoints into `buf`.
///
/// # Safety
/// `grid_ref` must be live; `buf` must accept `buf_len` `uint32_t`s; `out_len` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_vt_grid_ref_graphemes(
    grid_ref: *const GhosttyGridRef,
    buf: *mut u32,
    buf_len: usize,
    out_len: *mut usize,
) -> GhosttyResult {
    unsafe { exports::ghostty_grid_ref_graphemes(grid_ref, buf, buf_len, out_len) }
}

/// Reads the style of the cell a grid reference points at.
///
/// # Safety
/// `grid_ref` must be live; `out` may be null, and must have its `size` field set when it is not.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_vt_grid_ref_style(
    grid_ref: *const GhosttyGridRef,
    out: *mut GhosttyStyle,
) -> GhosttyResult {
    unsafe { exports::ghostty_grid_ref_style(grid_ref, out) }
}

/// Reads one datum out of a packed cell.
///
/// # Safety
/// `out` must point at storage of the type `data` selects.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_vt_cell_get(
    cell: GhosttyCell,
    data: GhosttyCellData,
    out: *mut c_void,
) -> GhosttyResult {
    unsafe { exports::ghostty_cell_get(cell, data, out) }
}

/// Reads one datum out of a packed row.
///
/// # Safety
/// `out` must point at storage of the type `data` selects.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_vt_row_get(
    row: GhosttyRow,
    data: GhosttyRowData,
    out: *mut c_void,
) -> GhosttyResult {
    unsafe { exports::ghostty_row_get(row, data, out) }
}

/// Fills `out` with the default style.
///
/// # Safety
/// `out` must be writable and must have its `size` field set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mind2t_vt_style_default(out: *mut GhosttyStyle) {
    unsafe { exports::ghostty_style_default(out) }
}
