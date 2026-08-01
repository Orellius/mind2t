//! Purpose: drive a real libghostty-vt terminal and read its grid out as a `Snapshot`.
//! Public surface: `Terminal::new`, `Terminal::write`, `Terminal::resize`,
//!   `Terminal::snapshot`, `Error`.
//! Why this file: the oracle side of the differential harness. Every unsafe call to the
//!   C ABI is confined here, so the rest of the workspace is ordinary safe Rust.
//! NOT responsible for: judging correctness (`ruuah-vt-snapshot::diff` does that), PTY, or
//!   any behaviour of its own. It observes; it never interprets.
//! Test strategy: `tests/oracle.rs` drives it with known byte streams; `tests/abi_layout.rs`
//!   pins every struct layout against the library's own `ghostty_type_json()` report.

use std::ffi::c_void;
use std::mem;

use ruuah_vt_snapshot::{Cell, Color, Colors, Cursor, Modes, Rgb, Row, Screen, Snapshot, Style};

use crate::convert::{convert_row_semantic, convert_semantic, convert_style, convert_wide};
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
    /// Creates a terminal with no scrollback.
    pub fn new(cols: u16, rows: u16) -> Result<Terminal, Error> {
        Terminal::with_scrollback(cols, rows, 0)
    }

    /// Creates a terminal retaining up to `max_scrollback` lines of history.
    pub fn with_scrollback(
        cols: u16,
        rows: u16,
        max_scrollback: usize,
    ) -> Result<Terminal, Error> {
        let options = sys::GhosttyTerminalOptions {
            cols,
            rows,
            max_scrollback,
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

    /// The raw handle, for the render state to read from.
    /// The raw handle, for tests that drive oracle ABI surfaces this wrapper does not
    /// cover (the mouse encoder's `setopt_from_terminal`). Reads only.
    pub fn raw(&self) -> sys::GhosttyTerminal {
        self.raw
    }

    /// Resizes the terminal, reflowing the primary screen as the library sees fit.
    ///
    /// The pixel dimensions are nominal. They feed image protocols and size reports, neither
    /// of which the snapshot observes, but the call requires them and passing zero would
    /// claim a zero-sized cell.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<(), Error> {
        const CELL_WIDTH_PX: u32 = 10;
        const CELL_HEIGHT_PX: u32 = 20;
        let code = unsafe {
            sys::ghostty_terminal_resize(self.raw, cols, rows, CELL_WIDTH_PX, CELL_HEIGHT_PX)
        };
        check("ghostty_terminal_resize", code)
    }

    /// Reads the whole active area out into an implementation-neutral snapshot.
    pub fn snapshot(&self) -> Result<Snapshot, Error> {
        let cols: u16 = unsafe { self.get(sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_COLS, "COLS") }?;
        let rows: u16 = unsafe { self.get(sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_ROWS, "ROWS") }?;

        let mut grid = Vec::with_capacity(rows as usize);
        for y in 0..rows {
            grid.push(self.read_row(y, cols)?);
        }

        // History is read top-down in the HISTORY coordinate space, which addresses the
        // scrollback alone. Reading it as SCREEN would require knowing where the active area
        // starts, which is exactly the arithmetic a differential harness should not restate.
        let scrollback = self.scrollback_rows()?;
        let mut history = Vec::with_capacity(scrollback);
        for y in 0..scrollback {
            history.push(self.read_row_at(
                sys::GhosttyPointTag_GHOSTTY_POINT_TAG_HISTORY,
                u32::try_from(y).unwrap_or(u32::MAX),
                cols,
            )?);
        }

        Ok(Snapshot {
            cols,
            rows,
            screen: self.screen()?,
            cursor: self.cursor()?,
            modes: Modes {
                bracketed_paste: self.dec_mode(2004)?,
                synchronized_output: self.dec_mode(2026)?,
                mouse_event_x10: self.dec_mode(9)?,
                mouse_event_normal: self.dec_mode(1000)?,
                mouse_event_button: self.dec_mode(1002)?,
                mouse_event_any: self.dec_mode(1003)?,
                mouse_format_utf8: self.dec_mode(1005)?,
                mouse_format_sgr: self.dec_mode(1006)?,
                mouse_format_urxvt: self.dec_mode(1015)?,
                mouse_format_sgr_pixels: self.dec_mode(1016)?,
                mouse_alternate_scroll: self.dec_mode(1007)?,
                cursor_keys: self.dec_mode(1)?,
                keypad_keys: self.dec_mode(66)?,
                ignore_keypad_with_numlock: self.dec_mode(1035)?,
                alt_esc_prefix: self.dec_mode(1036)?,
            },
            grid,
            history,
            damage: None,
            pwd: self.pwd()?,
            colors: self.colors()?,
        })
    }

    /// One dynamic colour (foreground/background/cursor). Three-state on purpose:
    /// `GHOSTTY_NO_VALUE` is the documented fresh-terminal answer -- no OSC override
    /// and no embedder default -- and folding it into the error path would make every
    /// snapshot of a plain terminal fail.
    fn dynamic_color(
        &self,
        data: sys::GhosttyTerminalData,
        call: &'static str,
    ) -> Result<Option<Rgb>, Error> {
        let mut out: sys::GhosttyColorRgb = unsafe { mem::zeroed() };
        let code = unsafe {
            sys::ghostty_terminal_get(self.raw, data, (&raw mut out).cast::<c_void>())
        };
        if code == sys::GhosttyResult_GHOSTTY_NO_VALUE {
            return Ok(None);
        }
        check(call, code)?;
        Ok(Some(Rgb { r: out.r, g: out.g, b: out.b }))
    }

    /// The OSC-addressable colour state: effective dynamic colours plus the current
    /// 256-entry palette ("with any OSC overrides" -- `terminal.h`). The palette getter
    /// is documented to always succeed, so a failure there is a real error.
    fn colors(&self) -> Result<Colors, Error> {
        let mut palette: [sys::GhosttyColorRgb; 256] = unsafe { mem::zeroed() };
        check("ghostty_terminal_get(COLOR_PALETTE)", unsafe {
            sys::ghostty_terminal_get(
                self.raw,
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_COLOR_PALETTE,
                (&raw mut palette).cast::<c_void>(),
            )
        })?;
        Ok(Colors {
            foreground: self.dynamic_color(
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_COLOR_FOREGROUND,
                "ghostty_terminal_get(COLOR_FOREGROUND)",
            )?,
            background: self.dynamic_color(
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_COLOR_BACKGROUND,
                "ghostty_terminal_get(COLOR_BACKGROUND)",
            )?,
            cursor: self.dynamic_color(
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_COLOR_CURSOR,
                "ghostty_terminal_get(COLOR_CURSOR)",
            )?,
            palette: palette.iter().map(|c| Rgb { r: c.r, g: c.g, b: c.b }).collect(),
        })
    }

    /// The working directory the child last reported, as raw bytes.
    ///
    /// The library stores whatever the shell emitted without parsing it
    /// (`stream_terminal.zig`, `reportPwd`), so this is the URI for OSC 7 and typically a
    /// bare path for the other sources. Empty means "never set, or cleared" -- the ABI has
    /// no third state, because `setPwd("")` clears the buffer outright.
    ///
    /// The pointer is borrowed and documented as valid only until the next write or reset,
    /// so it is copied here rather than held.
    ///
    /// Bytes, not a `String`: a path is not required to be UTF-8, and decoding lossily on
    /// both sides of a differential would make two different payloads compare equal.
    pub fn pwd(&self) -> Result<Vec<u8>, Error> {
        let raw: sys::GhosttyString =
            unsafe { self.get(sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_PWD, "PWD") }?;
        if raw.ptr.is_null() || raw.len == 0 {
            return Ok(Vec::new());
        }
        Ok(unsafe { std::slice::from_raw_parts(raw.ptr, raw.len) }.to_vec())
    }

    /// Reads a DEC private mode's current state.
    ///
    /// A `GhosttyMode` packs the numeric value in bits 0-14 with bit 15 clear for DEC
    /// private modes (`modes.h`); a plain mode number below 32768 IS the packed form.
    pub fn dec_mode(&self, mode: u16) -> Result<bool, Error> {
        let mut value = false;
        let code = unsafe { sys::ghostty_terminal_mode_get(self.raw, mode & 0x7FFF, &mut value) };
        check("ghostty_terminal_mode_get", code)?;
        Ok(value)
    }

    /// Total rows in the active screen including scrollback.
    pub fn total_rows(&self) -> Result<usize, Error> {
        unsafe { self.get(sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_TOTAL_ROWS, "TOTAL_ROWS") }
    }

    /// Rows of scrollback history above the active area.
    pub fn scrollback_rows(&self) -> Result<usize, Error> {
        unsafe {
            self.get(
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS,
                "SCROLLBACK_ROWS",
            )
        }
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
        self.read_row_at(sys::GhosttyPointTag_GHOSTTY_POINT_TAG_ACTIVE, u32::from(y), cols)
    }

    fn read_row_at(&self, tag: sys::GhosttyPointTag, y: u32, cols: u16) -> Result<Row, Error> {
        let first = self.grid_ref_at(tag, 0, y)?;
        let mut raw_row: sys::GhosttyRow = 0;
        check("ghostty_grid_ref_row", unsafe {
            sys::ghostty_grid_ref_row(&first, &mut raw_row)
        })?;

        let mut cells = Vec::with_capacity(cols as usize);
        for x in 0..cols {
            cells.push(self.read_cell_at(tag, x, y)?);
        }

        let semantic_prompt: sys::GhosttyRowSemanticPrompt = unsafe {
            row_get(
                raw_row,
                sys::GhosttyRowData_GHOSTTY_ROW_DATA_SEMANTIC_PROMPT,
                "SEMANTIC_PROMPT",
            )
        }?;

        Ok(Row {
            wrap: unsafe { row_get(raw_row, sys::GhosttyRowData_GHOSTTY_ROW_DATA_WRAP, "WRAP") }?,
            wrap_continuation: unsafe {
                row_get(
                    raw_row,
                    sys::GhosttyRowData_GHOSTTY_ROW_DATA_WRAP_CONTINUATION,
                    "WRAP_CONTINUATION",
                )
            }?,
            semantic_prompt: convert_row_semantic(semantic_prompt)?,
            cells,
        })
    }

    fn read_cell_at(&self, tag: sys::GhosttyPointTag, x: u16, y: u32) -> Result<Cell, Error> {
        let cell_ref = self.grid_ref_at(tag, x, y)?;

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

        let semantic: sys::GhosttyCellSemanticContent = unsafe {
            cell_get(
                raw_cell,
                sys::GhosttyCellData_GHOSTTY_CELL_DATA_SEMANTIC_CONTENT,
                "SEMANTIC_CONTENT",
            )
        }?;

        Ok(Cell {
            text: read_graphemes(&cell_ref, x, u16::try_from(y).unwrap_or(u16::MAX))?,
            wide: convert_wide(wide)?,
            style: self.read_style(raw_cell, &raw_style)?,
            semantic: convert_semantic(semantic)?,
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

    fn grid_ref_at(
        &self,
        tag: sys::GhosttyPointTag,
        x: u16,
        y: u32,
    ) -> Result<sys::GhosttyGridRef, Error> {
        let mut value: sys::GhosttyPointValue = unsafe { mem::zeroed() };
        value.coordinate = sys::GhosttyPointCoordinate { x, y };
        let point = sys::GhosttyPoint { tag, value };

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

pub(crate) fn check(call: &'static str, code: sys::GhosttyResult) -> Result<(), Error> {
    if code == sys::GhosttyResult_GHOSTTY_SUCCESS {
        Ok(())
    } else {
        Err(Error::Call { call, code })
    }
}
