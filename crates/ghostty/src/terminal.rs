//! Purpose: drive a real libghostty-vt terminal and read its grid out as a `Snapshot`.
//! Public surface: `Terminal::new`, `Terminal::write`, `Terminal::snapshot`, `Error`.
//! Why this file: the oracle side of the differential harness. Every unsafe call to the
//!   C ABI is confined here, so the rest of the workspace is ordinary safe Rust.
//! NOT responsible for: judging correctness (`ruuah-vt-snapshot::diff` does that), PTY, or
//!   any behaviour of its own. It observes; it never interprets.
//! Test strategy: `tests/oracle.rs` drives it with known byte streams; `tests/abi_layout.rs`
//!   pins every struct layout against the library's own `ghostty_type_json()` report.

use std::ffi::c_void;
use std::mem;

use ruuah_vt_snapshot::{Cell, Color, Cursor, Row, Screen, Snapshot, Style, Underline, Wide};

use crate::sys;

/// A failure reaching or reading the oracle. Never a terminal-behaviour disagreement:
/// those are `Difference`s, not errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{call} returned {code} from libghostty-vt")]
    Call { call: &'static str, code: i32 },

    #[error("ghostty_terminal_new reported success but produced a null handle")]
    NullTerminal,

    #[error("cell[{y},{x}] holds U+{codepoint:04X}, which is not a Unicode scalar value")]
    InvalidCodepoint { y: u16, x: u16, codepoint: u32 },

    #[error("libghostty-vt reported {kind} = {value}, which this build does not know")]
    UnknownEnum { kind: &'static str, value: u32 },
}

/// A libghostty-vt terminal instance.
pub struct Terminal {
    raw: sys::GhosttyTerminal,
}

impl Terminal {
    /// Creates a terminal with no scrollback. Slice 0 compares the active area only.
    pub fn new(cols: u16, rows: u16) -> Result<Terminal, Error> {
        let options = sys::GhosttyTerminalOptions {
            cols,
            rows,
            max_scrollback: 0,
        };
        let mut raw: sys::GhosttyTerminal = std::ptr::null_mut();
        let code = unsafe { sys::ghostty_terminal_new(std::ptr::null(), &mut raw, options) };
        check("ghostty_terminal_new", code)?;
        if raw.is_null() {
            return Err(Error::NullTerminal);
        }
        Ok(Terminal { raw })
    }

    /// Feeds bytes through the VT parser. Documented never to fail: malformed input is
    /// absorbed rather than reported, which is exactly the property a fuzzed differential
    /// corpus depends on.
    pub fn write(&mut self, bytes: &[u8]) {
        unsafe { sys::ghostty_terminal_vt_write(self.raw, bytes.as_ptr(), bytes.len()) };
    }

    /// Reads the whole active area out into an implementation-neutral snapshot.
    pub fn snapshot(&self) -> Result<Snapshot, Error> {
        let cols: u16 = unsafe { self.get(sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_COLS, "COLS") }?;
        let rows: u16 = unsafe { self.get(sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_ROWS, "ROWS") }?;

        let mut grid = Vec::with_capacity(rows as usize);
        for y in 0..rows {
            grid.push(self.read_row(y, cols)?);
        }

        Ok(Snapshot {
            cols,
            rows,
            screen: self.screen()?,
            cursor: self.cursor()?,
            grid,
        })
    }

    fn cursor(&self) -> Result<Cursor, Error> {
        let mut raw: sys::GhosttyStyle = unsafe { mem::zeroed() };
        raw.size = mem::size_of::<sys::GhosttyStyle>();
        let code = unsafe {
            sys::ghostty_terminal_get(
                self.raw,
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_CURSOR_STYLE,
                (&raw mut raw).cast::<c_void>(),
            )
        };
        check("ghostty_terminal_get(CURSOR_STYLE)", code)?;

        Ok(Cursor {
            x: unsafe { self.get(sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_CURSOR_X, "CURSOR_X") }?,
            y: unsafe { self.get(sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_CURSOR_Y, "CURSOR_Y") }?,
            pending_wrap: unsafe {
                self.get(
                    sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_CURSOR_PENDING_WRAP,
                    "CURSOR_PENDING_WRAP",
                )
            }?,
            visible: unsafe {
                self.get(
                    sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_CURSOR_VISIBLE,
                    "CURSOR_VISIBLE",
                )
            }?,
            style: convert_style(&raw)?,
        })
    }

    fn screen(&self) -> Result<Screen, Error> {
        let raw: sys::GhosttyTerminalScreen = unsafe {
            self.get(
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN,
                "ACTIVE_SCREEN",
            )
        }?;
        match raw {
            sys::GhosttyTerminalScreen_GHOSTTY_TERMINAL_SCREEN_PRIMARY => Ok(Screen::Primary),
            sys::GhosttyTerminalScreen_GHOSTTY_TERMINAL_SCREEN_ALTERNATE => Ok(Screen::Alternate),
            other => Err(Error::UnknownEnum {
                kind: "GhosttyTerminalScreen",
                value: other,
            }),
        }
    }

    fn read_row(&self, y: u16, cols: u16) -> Result<Row, Error> {
        let first = self.grid_ref(0, y)?;
        let mut raw_row: sys::GhosttyRow = 0;
        check("ghostty_grid_ref_row", unsafe {
            sys::ghostty_grid_ref_row(&first, &mut raw_row)
        })?;

        let mut cells = Vec::with_capacity(cols as usize);
        for x in 0..cols {
            cells.push(self.read_cell(x, y)?);
        }

        Ok(Row {
            wrap: unsafe { row_get(raw_row, sys::GhosttyRowData_GHOSTTY_ROW_DATA_WRAP, "WRAP") }?,
            wrap_continuation: unsafe {
                row_get(
                    raw_row,
                    sys::GhosttyRowData_GHOSTTY_ROW_DATA_WRAP_CONTINUATION,
                    "WRAP_CONTINUATION",
                )
            }?,
            cells,
        })
    }

    fn read_cell(&self, x: u16, y: u16) -> Result<Cell, Error> {
        let cell_ref = self.grid_ref(x, y)?;

        let mut raw_cell: sys::GhosttyCell = 0;
        check("ghostty_grid_ref_cell", unsafe {
            sys::ghostty_grid_ref_cell(&cell_ref, &mut raw_cell)
        })?;

        let mut raw_style: sys::GhosttyStyle = unsafe { mem::zeroed() };
        raw_style.size = mem::size_of::<sys::GhosttyStyle>();
        check("ghostty_grid_ref_style", unsafe {
            sys::ghostty_grid_ref_style(&cell_ref, &mut raw_style)
        })?;

        let wide: sys::GhosttyCellWide =
            unsafe { cell_get(raw_cell, sys::GhosttyCellData_GHOSTTY_CELL_DATA_WIDE, "WIDE") }?;

        Ok(Cell {
            text: read_graphemes(&cell_ref, x, y)?,
            wide: convert_wide(wide)?,
            style: self.read_style(raw_cell, &raw_style)?,
        })
    }

    /// Reads a cell's effective style, folding in a background that lives in the cell rather
    /// than in the style table.
    ///
    /// Ghostty stores a cell that has *only* a background colour with a `bg_color` content
    /// tag, keeping it out of the style map entirely. `ghostty_grid_ref_style` then reports
    /// the default style for it. Measured 2026-07-28: after `CSI 41m CSI 2K`, the style says
    /// `bg: Default` while the cell is genuinely red. Reading only the style would leave the
    /// harness blind to background-colour erase, so an erase implementation that ignored BCE
    /// would be reported as MATCH. Normalising both representations into `style.bg` costs
    /// nothing observable: a cell with text is still distinguished by its non-empty `text`.
    fn read_style(
        &self,
        raw_cell: sys::GhosttyCell,
        raw_style: &sys::GhosttyStyle,
    ) -> Result<Style, Error> {
        let mut style = convert_style(raw_style)?;

        let tag: sys::GhosttyCellContentTag = unsafe {
            cell_get(
                raw_cell,
                sys::GhosttyCellData_GHOSTTY_CELL_DATA_CONTENT_TAG,
                "CONTENT_TAG",
            )
        }?;

        match tag {
            sys::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_BG_COLOR_PALETTE => {
                let index: u8 = unsafe {
                    cell_get(
                        raw_cell,
                        sys::GhosttyCellData_GHOSTTY_CELL_DATA_COLOR_PALETTE,
                        "COLOR_PALETTE",
                    )
                }?;
                style.bg = Color::Palette(index);
            }
            sys::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_BG_COLOR_RGB => {
                let rgb: sys::GhosttyColorRgb = unsafe {
                    cell_get(
                        raw_cell,
                        sys::GhosttyCellData_GHOSTTY_CELL_DATA_COLOR_RGB,
                        "COLOR_RGB",
                    )
                }?;
                style.bg = Color::Rgb {
                    r: rgb.r,
                    g: rgb.g,
                    b: rgb.b,
                };
            }
            _ => {}
        }

        Ok(style)
    }

    fn grid_ref(&self, x: u16, y: u16) -> Result<sys::GhosttyGridRef, Error> {
        let mut value: sys::GhosttyPointValue = unsafe { mem::zeroed() };
        value.coordinate = sys::GhosttyPointCoordinate { x, y: y as u32 };
        let point = sys::GhosttyPoint {
            tag: sys::GhosttyPointTag_GHOSTTY_POINT_TAG_ACTIVE,
            value,
        };

        let mut out: sys::GhosttyGridRef = unsafe { mem::zeroed() };
        out.size = mem::size_of::<sys::GhosttyGridRef>();
        check("ghostty_terminal_grid_ref", unsafe {
            sys::ghostty_terminal_grid_ref(self.raw, point, &mut out)
        })?;
        Ok(out)
    }

    /// # Safety
    /// `T` must be the output type documented for `data` in `terminal.h`.
    unsafe fn get<T>(&self, data: sys::GhosttyTerminalData, call: &'static str) -> Result<T, Error> {
        let mut out: T = unsafe { mem::zeroed() };
        let code = unsafe {
            sys::ghostty_terminal_get(self.raw, data, (&raw mut out).cast::<c_void>())
        };
        check(call, code)?;
        Ok(out)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        unsafe { sys::ghostty_terminal_free(self.raw) };
    }
}

/// # Safety
/// `T` must be the output type documented for `data` in `screen.h`.
unsafe fn cell_get<T>(cell: sys::GhosttyCell, data: sys::GhosttyCellData, call: &'static str) -> Result<T, Error> {
    let mut out: T = unsafe { mem::zeroed() };
    check(call, unsafe {
        sys::ghostty_cell_get(cell, data, (&raw mut out).cast::<c_void>())
    })?;
    Ok(out)
}

/// # Safety
/// `T` must be the output type documented for `data` in `screen.h`.
unsafe fn row_get<T>(row: sys::GhosttyRow, data: sys::GhosttyRowData, call: &'static str) -> Result<T, Error> {
    let mut out: T = unsafe { mem::zeroed() };
    check(call, unsafe {
        sys::ghostty_row_get(row, data, (&raw mut out).cast::<c_void>())
    })?;
    Ok(out)
}

/// Reads a cell's full grapheme cluster, retrying on the heap if the cluster is longer
/// than the inline buffer. A cell with no text yields an empty string.
fn read_graphemes(cell_ref: &sys::GhosttyGridRef, x: u16, y: u16) -> Result<String, Error> {
    const INLINE: usize = 8;
    let mut inline = [0u32; INLINE];
    let mut len: usize = 0;

    let code = unsafe {
        sys::ghostty_grid_ref_graphemes(cell_ref, inline.as_mut_ptr(), INLINE, &mut len)
    };

    let codepoints: Vec<u32> = if code == sys::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
        let mut heap = vec![0u32; len];
        check("ghostty_grid_ref_graphemes", unsafe {
            sys::ghostty_grid_ref_graphemes(cell_ref, heap.as_mut_ptr(), heap.len(), &mut len)
        })?;
        heap[..len].to_vec()
    } else {
        check("ghostty_grid_ref_graphemes", code)?;
        inline[..len].to_vec()
    };

    codepoints
        .into_iter()
        .filter(|cp| *cp != 0)
        .map(|cp| {
            char::from_u32(cp).ok_or(Error::InvalidCodepoint {
                y,
                x,
                codepoint: cp,
            })
        })
        .collect()
}

fn convert_style(raw: &sys::GhosttyStyle) -> Result<Style, Error> {
    Ok(Style {
        fg: convert_color(&raw.fg_color)?,
        bg: convert_color(&raw.bg_color)?,
        underline_color: convert_color(&raw.underline_color)?,
        bold: raw.bold,
        italic: raw.italic,
        faint: raw.faint,
        blink: raw.blink,
        inverse: raw.inverse,
        invisible: raw.invisible,
        strikethrough: raw.strikethrough,
        overline: raw.overline,
        underline: convert_underline(raw.underline)?,
    })
}

fn convert_color(raw: &sys::GhosttyStyleColor) -> Result<Color, Error> {
    match raw.tag {
        sys::GhosttyStyleColorTag_GHOSTTY_STYLE_COLOR_NONE => Ok(Color::Default),
        sys::GhosttyStyleColorTag_GHOSTTY_STYLE_COLOR_PALETTE => {
            Ok(Color::Palette(unsafe { raw.value.palette }))
        }
        sys::GhosttyStyleColorTag_GHOSTTY_STYLE_COLOR_RGB => {
            let rgb = unsafe { raw.value.rgb };
            Ok(Color::Rgb {
                r: rgb.r,
                g: rgb.g,
                b: rgb.b,
            })
        }
        other => Err(Error::UnknownEnum {
            kind: "GhosttyStyleColorTag",
            value: other,
        }),
    }
}

fn convert_wide(raw: sys::GhosttyCellWide) -> Result<Wide, Error> {
    match raw {
        sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_NARROW => Ok(Wide::Narrow),
        sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_WIDE => Ok(Wide::Wide),
        sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_SPACER_TAIL => Ok(Wide::SpacerTail),
        sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_SPACER_HEAD => Ok(Wide::SpacerHead),
        other => Err(Error::UnknownEnum {
            kind: "GhosttyCellWide",
            value: other,
        }),
    }
}

fn convert_underline(raw: i32) -> Result<Underline, Error> {
    match raw as u32 {
        sys::GhosttySgrUnderline_GHOSTTY_SGR_UNDERLINE_NONE => Ok(Underline::None),
        sys::GhosttySgrUnderline_GHOSTTY_SGR_UNDERLINE_SINGLE => Ok(Underline::Single),
        sys::GhosttySgrUnderline_GHOSTTY_SGR_UNDERLINE_DOUBLE => Ok(Underline::Double),
        sys::GhosttySgrUnderline_GHOSTTY_SGR_UNDERLINE_CURLY => Ok(Underline::Curly),
        sys::GhosttySgrUnderline_GHOSTTY_SGR_UNDERLINE_DOTTED => Ok(Underline::Dotted),
        sys::GhosttySgrUnderline_GHOSTTY_SGR_UNDERLINE_DASHED => Ok(Underline::Dashed),
        other => Err(Error::UnknownEnum {
            kind: "GhosttySgrUnderline",
            value: other,
        }),
    }
}

fn check(call: &'static str, code: sys::GhosttyResult) -> Result<(), Error> {
    if code == sys::GhosttyResult_GHOSTTY_SUCCESS {
        Ok(())
    } else {
        Err(Error::Call { call, code })
    }
}
