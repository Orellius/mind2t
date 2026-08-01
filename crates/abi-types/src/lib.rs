//! Purpose: the C types ruuah-vt publishes, laid out to match `ghostty/vt/*.h` exactly.
//! Public surface: every `Ghostty*` type and constant a consumer names.
//! Why this file: the whole claim of slice 6 is that a program compiled against
//!   libghostty-vt's headers can link this archive instead. That claim is a claim about
//!   BYTE LAYOUT, so the layouts live in one place and are pinned against the real library's
//!   own `ghostty_type_json()` report in `tests/layout.rs` -- not against the headers, which
//!   could drift, and not against a reading of them, which could be wrong.
//! NOT responsible for: behaviour (`exports.rs`) or the core (`ruuah-vt-core`).
//! Test strategy: `crates/ghostty/tests/abi_parity.rs` compares every size and offset here
//!   against the real library's own `ghostty_type_json()`. It lives in that crate because
//!   only that crate links libghostty-vt, and this one deliberately depends on nothing so it
//!   can be pulled in beside the oracle without a symbol clash.

#![allow(non_camel_case_types)]

use std::ffi::c_void;

pub type GhosttyResult = i32;
pub const GHOSTTY_SUCCESS: GhosttyResult = 0;
pub const GHOSTTY_OUT_OF_MEMORY: GhosttyResult = -1;
pub const GHOSTTY_INVALID_VALUE: GhosttyResult = -2;
pub const GHOSTTY_OUT_OF_SPACE: GhosttyResult = -3;
/// The query is valid but there is nothing to answer -- the oracle's result for an
/// unset dynamic colour (`types.h`: `GHOSTTY_NO_VALUE = -4`).
pub const GHOSTTY_NO_VALUE: GhosttyResult = -4;

/// Opaque handle. A `*mut Terminal` from `exports.rs`, never dereferenced by the caller.
pub type GhosttyTerminal = *mut c_void;

/// Opaque allocator handle. Accepted and ignored: this library allocates through Rust's
/// global allocator, and the ABI documents NULL as "use the default".
pub type GhosttyAllocator = c_void;

/// A cell, packed into one word. Opaque by contract -- the header documents that it is read
/// through `ghostty_cell_get` and never bitcast, which is what lets this hold a different
/// bit layout from Ghostty's own packed cell while answering every query identically.
pub type GhosttyCell = u64;

/// A row, packed into one word. Opaque on the same terms as `GhosttyCell`.
pub type GhosttyRow = u64;

pub type GhosttyTerminalData = i32;
pub const GHOSTTY_TERMINAL_DATA_COLS: GhosttyTerminalData = 1;
pub const GHOSTTY_TERMINAL_DATA_ROWS: GhosttyTerminalData = 2;
pub const GHOSTTY_TERMINAL_DATA_CURSOR_X: GhosttyTerminalData = 3;
pub const GHOSTTY_TERMINAL_DATA_CURSOR_Y: GhosttyTerminalData = 4;
pub const GHOSTTY_TERMINAL_DATA_CURSOR_PENDING_WRAP: GhosttyTerminalData = 5;
pub const GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN: GhosttyTerminalData = 6;
pub const GHOSTTY_TERMINAL_DATA_CURSOR_VISIBLE: GhosttyTerminalData = 7;
pub const GHOSTTY_TERMINAL_DATA_CURSOR_STYLE: GhosttyTerminalData = 10;
pub const GHOSTTY_TERMINAL_DATA_TITLE: GhosttyTerminalData = 12;
pub const GHOSTTY_TERMINAL_DATA_PWD: GhosttyTerminalData = 13;
pub const GHOSTTY_TERMINAL_DATA_TOTAL_ROWS: GhosttyTerminalData = 14;
pub const GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS: GhosttyTerminalData = 15;
// The colour readouts, values mirroring the oracle's Data enum exactly
// (`src/terminal/c/terminal.zig`: color_foreground = 18 .. color_palette_default = 25).
// The effective getters answer GHOSTTY_NO_VALUE when neither an OSC override nor an
// embedder default exists; the palette getters always succeed.
pub const GHOSTTY_TERMINAL_DATA_COLOR_FOREGROUND: GhosttyTerminalData = 18;
pub const GHOSTTY_TERMINAL_DATA_COLOR_BACKGROUND: GhosttyTerminalData = 19;
pub const GHOSTTY_TERMINAL_DATA_COLOR_CURSOR: GhosttyTerminalData = 20;
pub const GHOSTTY_TERMINAL_DATA_COLOR_PALETTE: GhosttyTerminalData = 21;
pub const GHOSTTY_TERMINAL_DATA_COLOR_FOREGROUND_DEFAULT: GhosttyTerminalData = 22;
pub const GHOSTTY_TERMINAL_DATA_COLOR_BACKGROUND_DEFAULT: GhosttyTerminalData = 23;
pub const GHOSTTY_TERMINAL_DATA_COLOR_CURSOR_DEFAULT: GhosttyTerminalData = 24;
pub const GHOSTTY_TERMINAL_DATA_COLOR_PALETTE_DEFAULT: GhosttyTerminalData = 25;

pub type GhosttyTerminalScreen = i32;
pub const GHOSTTY_TERMINAL_SCREEN_PRIMARY: GhosttyTerminalScreen = 0;
pub const GHOSTTY_TERMINAL_SCREEN_ALTERNATE: GhosttyTerminalScreen = 1;

/// A packed terminal mode (`modes.h`): bits 0-14 carry the mode number, bit 15 set means
/// ANSI, clear means DEC private. A DEC private mode below 32768 is therefore its own
/// packed form -- bracketed paste is the literal 2004.
pub type GhosttyMode = u16;
pub const GHOSTTY_MODE_BRACKETED_PASTE: GhosttyMode = 2004;
pub const GHOSTTY_MODE_SYNCHRONIZED_OUTPUT: GhosttyMode = 2026;

pub type GhosttyPointTag = i32;
pub const GHOSTTY_POINT_TAG_ACTIVE: GhosttyPointTag = 0;
pub const GHOSTTY_POINT_TAG_VIEWPORT: GhosttyPointTag = 1;
pub const GHOSTTY_POINT_TAG_SCREEN: GhosttyPointTag = 2;
pub const GHOSTTY_POINT_TAG_HISTORY: GhosttyPointTag = 3;

pub type GhosttyCellData = i32;
pub const GHOSTTY_CELL_DATA_CODEPOINT: GhosttyCellData = 1;
pub const GHOSTTY_CELL_DATA_CONTENT_TAG: GhosttyCellData = 2;
pub const GHOSTTY_CELL_DATA_WIDE: GhosttyCellData = 3;
pub const GHOSTTY_CELL_DATA_HAS_TEXT: GhosttyCellData = 4;
pub const GHOSTTY_CELL_DATA_HAS_STYLING: GhosttyCellData = 5;
pub const GHOSTTY_CELL_DATA_STYLE_ID: GhosttyCellData = 6;
pub const GHOSTTY_CELL_DATA_HAS_HYPERLINK: GhosttyCellData = 7;
pub const GHOSTTY_CELL_DATA_PROTECTED: GhosttyCellData = 8;
pub const GHOSTTY_CELL_DATA_SEMANTIC_CONTENT: GhosttyCellData = 9;

pub type GhosttyRowData = i32;
pub const GHOSTTY_ROW_DATA_WRAP: GhosttyRowData = 1;
pub const GHOSTTY_ROW_DATA_WRAP_CONTINUATION: GhosttyRowData = 2;
pub const GHOSTTY_ROW_DATA_GRAPHEME: GhosttyRowData = 3;
pub const GHOSTTY_ROW_DATA_STYLED: GhosttyRowData = 4;
pub const GHOSTTY_ROW_DATA_HYPERLINK: GhosttyRowData = 5;
pub const GHOSTTY_ROW_DATA_SEMANTIC_PROMPT: GhosttyRowData = 6;
pub const GHOSTTY_ROW_DATA_DIRTY: GhosttyRowData = 8;

pub type GhosttyCellContentTag = i32;
pub const GHOSTTY_CELL_CONTENT_CODEPOINT: GhosttyCellContentTag = 0;
pub const GHOSTTY_CELL_CONTENT_CODEPOINT_GRAPHEME: GhosttyCellContentTag = 1;

pub type GhosttyCellWide = i32;
pub const GHOSTTY_CELL_WIDE_NARROW: GhosttyCellWide = 0;
pub const GHOSTTY_CELL_WIDE_WIDE: GhosttyCellWide = 1;
pub const GHOSTTY_CELL_WIDE_SPACER_TAIL: GhosttyCellWide = 2;
pub const GHOSTTY_CELL_WIDE_SPACER_HEAD: GhosttyCellWide = 3;

pub type GhosttyCellSemanticContent = i32;
pub const GHOSTTY_CELL_SEMANTIC_OUTPUT: GhosttyCellSemanticContent = 0;
pub const GHOSTTY_CELL_SEMANTIC_INPUT: GhosttyCellSemanticContent = 1;
pub const GHOSTTY_CELL_SEMANTIC_PROMPT: GhosttyCellSemanticContent = 2;

pub type GhosttyRowSemanticPrompt = i32;
pub const GHOSTTY_ROW_SEMANTIC_NONE: GhosttyRowSemanticPrompt = 0;
pub const GHOSTTY_ROW_SEMANTIC_PROMPT: GhosttyRowSemanticPrompt = 1;
pub const GHOSTTY_ROW_SEMANTIC_PROMPT_CONTINUATION: GhosttyRowSemanticPrompt = 2;

pub type GhosttyStyleColorTag = i32;
pub const GHOSTTY_STYLE_COLOR_NONE: GhosttyStyleColorTag = 0;
pub const GHOSTTY_STYLE_COLOR_PALETTE: GhosttyStyleColorTag = 1;
pub const GHOSTTY_STYLE_COLOR_RGB: GhosttyStyleColorTag = 2;

pub type GhosttySgrUnderline = i32;
pub const GHOSTTY_SGR_UNDERLINE_NONE: GhosttySgrUnderline = 0;
pub const GHOSTTY_SGR_UNDERLINE_SINGLE: GhosttySgrUnderline = 1;
pub const GHOSTTY_SGR_UNDERLINE_DOUBLE: GhosttySgrUnderline = 2;
pub const GHOSTTY_SGR_UNDERLINE_CURLY: GhosttySgrUnderline = 3;
pub const GHOSTTY_SGR_UNDERLINE_DOTTED: GhosttySgrUnderline = 4;
pub const GHOSTTY_SGR_UNDERLINE_DASHED: GhosttySgrUnderline = 5;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyColorRgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union GhosttyStyleColorValue {
    pub palette: u8,
    pub rgb: GhosttyColorRgb,
    pub _padding: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyStyleColor {
    pub tag: GhosttyStyleColorTag,
    pub value: GhosttyStyleColorValue,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyStyle {
    pub size: usize,
    pub fg_color: GhosttyStyleColor,
    pub bg_color: GhosttyStyleColor,
    pub underline_color: GhosttyStyleColor,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub blink: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    pub overline: bool,
    pub underline: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyTerminalOptions {
    pub cols: u16,
    pub rows: u16,
    pub max_scrollback: usize,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyPointCoordinate {
    pub x: u16,
    pub y: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union GhosttyPointValue {
    pub coordinate: GhosttyPointCoordinate,
    /// Reserved by the ABI. Not optional padding: without it the union is 8 bytes instead of
    /// 16 and `GhosttyPoint` shrinks from 24 to 12, so a consumer compiled against the real
    /// header would pass a struct this library reads short. Caught by `abi_parity.rs` on its
    /// first run, which is the entire reason that test exists.
    pub _padding: [u64; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyPoint {
    pub tag: GhosttyPointTag,
    pub value: GhosttyPointValue,
}

/// A borrowed, non-owning string: pointer plus byte length, never NUL-terminated by
/// contract (`types.h`).
///
/// The producer documents how long the bytes stay valid. For
/// `GHOSTTY_TERMINAL_DATA_PWD` that is "until the next `vt_write` or `reset`", which is
/// exactly the lifetime of this library's cached view -- so the pointer aims into the
/// cache rather than at a fresh allocation the caller would have to free.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyString {
    pub ptr: *const u8,
    pub len: usize,
}

/// A resolved position in the grid.
///
/// Ghostty stores a page node plus a coordinate within that page. This core does not page the
/// active area, so `node` is the terminal itself and `y` is an absolute row in SCREEN space:
/// history rows first, then the active area. The visible consequence is that this library
/// cannot address beyond row 65535, where Ghostty can -- its `y` is within-page and its pages
/// are bounded. Recorded rather than hidden; `grid_ref` refuses out-of-range rather than
/// wrapping.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyGridRef {
    pub size: usize,
    pub node: *mut c_void,
    pub x: u16,
    pub y: u16,
}
