//! Purpose: the C entry points, implemented on `ruuah-vt-core`.
//! Public surface: the `ghostty_*` functions a consumer links against.
//! Why this file: this is the drop-in claim made executable. Everything here is a thin
//!   translation of a call the core already answers, and it is thin on purpose -- a fat ABI
//!   layer would be a second implementation of the terminal, which is the one thing the
//!   differential harness could not then catch.
//! NOT responsible for: terminal behaviour (the core), or layout (`types.rs`).
//! Test strategy: `tests/differential.rs` drives the same bytes through these functions and
//!   through the core's Rust API and requires the two snapshots to be identical, with a
//!   control that proves the comparison can fail.

use std::ffi::c_void;

use ruuah_vt_snapshot::{Color, Row, Screen, Snapshot, Underline, Wide};

use ruuah_vt_abi_types::*;

/// The boxed terminal a `GhosttyTerminal` handle points at.
///
/// The cached view is what keeps the ref-based readers cheap AND sound: `grid_ref` is the only
/// entry point that takes the terminal by pointer and can refresh it, so every reader reached
/// through a `GhosttyGridRef` needs nothing but shared access.
pub struct Terminal {
    core: ruuah_vt_core::Terminal,
    view: Option<Snapshot>,
}

impl Terminal {
    fn refresh(&mut self) {
        if self.view.is_none() {
            self.view = Some(self.core.snapshot());
        }
    }

    fn invalidate(&mut self) {
        self.view = None;
    }

    fn view(&self) -> Option<&Snapshot> {
        self.view.as_ref()
    }

    /// The row at an absolute SCREEN-space index: history first, then the active area.
    fn row_at(&self, y: u16) -> Option<&Row> {
        let view = self.view()?;
        let y = usize::from(y);
        if y < view.history.len() {
            view.history.get(y)
        } else {
            view.grid.get(y - view.history.len())
        }
    }
}

/// # Safety
/// `handle` must be a pointer returned by `ghostty_terminal_new` and not yet freed.
unsafe fn terminal<'a>(handle: GhosttyTerminal) -> Option<&'a mut Terminal> {
    if handle.is_null() {
        return None;
    }
    Some(unsafe { &mut *handle.cast::<Terminal>() })
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
pub unsafe extern "C" fn ghostty_terminal_new(
    _allocator: *const GhosttyAllocator,
    out: *mut GhosttyTerminal,
    options: GhosttyTerminalOptions,
) -> GhosttyResult {
    if out.is_null() || options.cols == 0 || options.rows == 0 {
        return GHOSTTY_INVALID_VALUE;
    }
    let terminal = Box::new(Terminal {
        core: ruuah_vt_core::Terminal::with_scrollback(
            options.cols,
            options.rows,
            options.max_scrollback,
        ),
        view: None,
    });
    unsafe { *out = Box::into_raw(terminal).cast::<c_void>() };
    GHOSTTY_SUCCESS
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_terminal_free(handle: GhosttyTerminal) {
    if handle.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(handle.cast::<Terminal>()) });
}

#[unsafe(no_mangle)]
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
pub unsafe extern "C" fn ghostty_terminal_get(
    handle: GhosttyTerminal,
    data: GhosttyTerminalData,
    out: *mut c_void,
) -> GhosttyResult {
    let Some(terminal) = (unsafe { terminal(handle) }) else {
        return GHOSTTY_INVALID_VALUE;
    };
    if out.is_null() {
        return GHOSTTY_INVALID_VALUE;
    }
    terminal.refresh();
    let Some(view) = terminal.view() else {
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
                let size = (*style).size;
                *style = pack_style(&view.cursor.style, size);
            }
            GHOSTTY_TERMINAL_DATA_TOTAL_ROWS => {
                *out.cast::<usize>() = view.history.len() + usize::from(view.rows)
            }
            GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS => *out.cast::<usize>() = view.history.len(),
            _ => return GHOSTTY_INVALID_VALUE,
        }
    }
    GHOSTTY_SUCCESS
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_terminal_grid_ref(
    handle: GhosttyTerminal,
    point: GhosttyPoint,
    out: *mut GhosttyGridRef,
) -> GhosttyResult {
    let Some(terminal) = (unsafe { terminal(handle) }) else {
        return GHOSTTY_INVALID_VALUE;
    };
    if out.is_null() {
        return GHOSTTY_INVALID_VALUE;
    }
    terminal.refresh();
    let Some(view) = terminal.view() else {
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

    unsafe {
        let size = (*out).size;
        *out = GhosttyGridRef {
            size,
            node: (terminal as *mut Terminal).cast::<c_void>(),
            x: coordinate.x,
            y: absolute as u16,
        };
    }
    GHOSTTY_SUCCESS
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_grid_ref_cell(
    grid_ref: *const GhosttyGridRef,
    out: *mut GhosttyCell,
) -> GhosttyResult {
    let Some((terminal, grid_ref)) = (unsafe { resolve(grid_ref) }) else {
        return GHOSTTY_INVALID_VALUE;
    };
    if out.is_null() {
        return GHOSTTY_INVALID_VALUE;
    }
    let Some(cell) = terminal
        .row_at(grid_ref.y)
        .and_then(|row| row.cells.get(usize::from(grid_ref.x)))
    else {
        return GHOSTTY_INVALID_VALUE;
    };
    unsafe { *out = pack_cell(cell) };
    GHOSTTY_SUCCESS
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_grid_ref_row(
    grid_ref: *const GhosttyGridRef,
    out: *mut GhosttyRow,
) -> GhosttyResult {
    let Some((terminal, grid_ref)) = (unsafe { resolve(grid_ref) }) else {
        return GHOSTTY_INVALID_VALUE;
    };
    if out.is_null() {
        return GHOSTTY_INVALID_VALUE;
    }
    let Some(row) = terminal.row_at(grid_ref.y) else {
        return GHOSTTY_INVALID_VALUE;
    };
    unsafe { *out = pack_row(row) };
    GHOSTTY_SUCCESS
}

#[unsafe(no_mangle)]
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
    let Some(cell) = terminal
        .row_at(grid_ref.y)
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
pub unsafe extern "C" fn ghostty_grid_ref_style(
    grid_ref: *const GhosttyGridRef,
    out: *mut GhosttyStyle,
) -> GhosttyResult {
    let Some((terminal, grid_ref)) = (unsafe { resolve(grid_ref) }) else {
        return GHOSTTY_INVALID_VALUE;
    };
    if out.is_null() {
        return GHOSTTY_INVALID_VALUE;
    }
    let Some(cell) = terminal
        .row_at(grid_ref.y)
        .and_then(|row| row.cells.get(usize::from(grid_ref.x)))
    else {
        return GHOSTTY_INVALID_VALUE;
    };
    unsafe {
        let size = (*out).size;
        *out = pack_style(&cell.style, size);
    }
    GHOSTTY_SUCCESS
}

#[unsafe(no_mangle)]
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
                // Every cell this core stores is a codepoint cell. Ghostty additionally keeps
                // a background-colour-only cell out of its style table and tags it; here the
                // background always lives in the style, so a consumer reading the style gets
                // the same answer without the second representation.
                *out.cast::<GhosttyCellContentTag>() = GHOSTTY_CELL_CONTENT_CODEPOINT
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
            _ => return GHOSTTY_INVALID_VALUE,
        }
    }
    GHOSTTY_SUCCESS
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_style_default(out: *mut GhosttyStyle) {
    if out.is_null() {
        return;
    }
    unsafe {
        let size = (*out).size;
        *out = pack_style(&ruuah_vt_snapshot::Style::DEFAULT, size);
    }
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

fn pack_cell(cell: &ruuah_vt_snapshot::Cell) -> GhosttyCell {
    let codepoint = u64::from(cell.text.chars().next().map(u32::from).unwrap_or(0)) & 0x1F_FFFF;
    let wide = match cell.wide {
        Wide::Narrow => 0u64,
        Wide::Wide => 1,
        Wide::SpacerTail => 2,
        Wide::SpacerHead => 3,
    };
    let semantic = match cell.semantic {
        ruuah_vt_snapshot::Semantic::Output => 0u64,
        ruuah_vt_snapshot::Semantic::Input => 1,
        ruuah_vt_snapshot::Semantic::Prompt => 2,
    };
    let styled = u64::from(!cell.style.is_default());
    codepoint | (wide << 21) | (semantic << 23) | (styled << 41)
}

fn pack_row(row: &Row) -> GhosttyRow {
    let semantic = match row.semantic_prompt {
        ruuah_vt_snapshot::RowSemantic::None => 0u64,
        ruuah_vt_snapshot::RowSemantic::Prompt => 1,
        ruuah_vt_snapshot::RowSemantic::PromptContinuation => 2,
    };
    let grapheme = row.cells.iter().any(|cell| cell.text.chars().count() > 1);
    let styled = row.cells.iter().any(|cell| !cell.style.is_default());
    u64::from(row.wrap)
        | (u64::from(row.wrap_continuation) << 1)
        | (u64::from(grapheme) << 2)
        | (u64::from(styled) << 3)
        | (semantic << 4)
}

fn pack_style(style: &ruuah_vt_snapshot::Style, size: usize) -> GhosttyStyle {
    GhosttyStyle {
        size,
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
