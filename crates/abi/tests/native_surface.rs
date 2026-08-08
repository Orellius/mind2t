//! The `mind2t_vt_*` surface must do the same thing as the `ghostty_*` one, not merely exist.
//!
//! `scripts/build-lib.sh` proves both sets of symbols are in the archive. That is a spelling
//! check: an entry point that forwards to the wrong function, or that somebody later
//! reimplements instead of forwarding, exports perfectly and answers wrongly.
//!
//! So this drives the SAME bytes through both surfaces and demands the same answers. It is
//! deliberately not a unit test of either one - both are already covered by the differential
//! corpus through `ghostty_*`. What is uncovered without this file is the seam between them.

use std::ffi::c_void;

use mind2t_vt::{exports, native};
use mind2t_vt::types::*;

const COLS: u16 = 20;
const ROWS: u16 = 3;

/// Writes `bytes` through one surface and reads back (cell codepoint, style, cursor x/y).
///
/// Taken as a closure pair so the two surfaces run identical code apart from the calls
/// themselves; anything else and the test starts comparing its own two harnesses.
type Reading = (u32, u64, u16, u16);

unsafe fn drive_ghostty(bytes: &[u8]) -> Reading {
    unsafe {
        let mut handle: GhosttyTerminal = std::ptr::null_mut();
        let options = GhosttyTerminalOptions { cols: COLS, rows: ROWS, max_scrollback: 100 };
        assert_eq!(exports::ghostty_terminal_new(std::ptr::null(), &mut handle, options), 0);
        exports::ghostty_terminal_vt_write(handle, bytes.as_ptr(), bytes.len());

        let mut point = GhosttyPoint {
            tag: 0,
            value: GhosttyPointValue { coordinate: GhosttyPointCoordinate { x: 0, y: 0 } },
        };
        point.value.coordinate = GhosttyPointCoordinate { x: 0, y: 0 };
        let mut grid = GhosttyGridRef {
            size: std::mem::size_of::<GhosttyGridRef>(),
            node: std::ptr::null_mut(),
            x: 0,
            y: 0,
        };
        assert_eq!(exports::ghostty_terminal_grid_ref(handle, point, &mut grid), 0);
        let mut cell: GhosttyCell = 0;
        assert_eq!(exports::ghostty_grid_ref_cell(&grid, &mut cell), 0);
        let mut codepoint: u32 = 0;
        assert_eq!(
            exports::ghostty_cell_get(cell, 1, (&raw mut codepoint).cast::<c_void>()),
            0
        );

        let mut x: u16 = 0;
        let mut y: u16 = 0;
        assert_eq!(exports::ghostty_terminal_get(handle, 2, (&raw mut x).cast::<c_void>()), 0);
        assert_eq!(exports::ghostty_terminal_get(handle, 3, (&raw mut y).cast::<c_void>()), 0);

        exports::ghostty_terminal_free(handle);
        (codepoint, cell, x, y)
    }
}

unsafe fn drive_native(bytes: &[u8]) -> Reading {
    unsafe {
        let mut handle: GhosttyTerminal = std::ptr::null_mut();
        let options = GhosttyTerminalOptions { cols: COLS, rows: ROWS, max_scrollback: 100 };
        assert_eq!(native::mind2t_vt_terminal_new(std::ptr::null(), &mut handle, options), 0);
        native::mind2t_vt_terminal_vt_write(handle, bytes.as_ptr(), bytes.len());

        let mut point = GhosttyPoint {
            tag: 0,
            value: GhosttyPointValue { coordinate: GhosttyPointCoordinate { x: 0, y: 0 } },
        };
        point.value.coordinate = GhosttyPointCoordinate { x: 0, y: 0 };
        let mut grid = GhosttyGridRef {
            size: std::mem::size_of::<GhosttyGridRef>(),
            node: std::ptr::null_mut(),
            x: 0,
            y: 0,
        };
        assert_eq!(native::mind2t_vt_terminal_grid_ref(handle, point, &mut grid), 0);
        let mut cell: GhosttyCell = 0;
        assert_eq!(native::mind2t_vt_grid_ref_cell(&grid, &mut cell), 0);
        let mut codepoint: u32 = 0;
        assert_eq!(
            native::mind2t_vt_cell_get(cell, 1, (&raw mut codepoint).cast::<c_void>()),
            0
        );

        let mut x: u16 = 0;
        let mut y: u16 = 0;
        assert_eq!(
            native::mind2t_vt_terminal_get(handle, 2, (&raw mut x).cast::<c_void>()),
            0
        );
        assert_eq!(
            native::mind2t_vt_terminal_get(handle, 3, (&raw mut y).cast::<c_void>()),
            0
        );

        native::mind2t_vt_terminal_free(handle);
        (codepoint, cell, x, y)
    }
}

#[test]
fn both_surfaces_answer_identically() {
    // Non-trivial on purpose: SGR so the packed cell carries a style, a wide character so the
    // cell is not a plain ASCII byte, and a cursor move so the position is not the default.
    for bytes in [
        b"hello".as_slice(),
        b"\x1b[31;1mred\x1b[0m".as_slice(),
        "\u{4e16}\u{754c}".as_bytes(),
        b"\x1b[2;5Hplaced".as_slice(),
        b"line one\r\nline two\r\nline three\r\nline four".as_slice(),
    ] {
        let ours = unsafe { drive_native(bytes) };
        let theirs = unsafe { drive_ghostty(bytes) };
        assert_eq!(
            ours, theirs,
            "the two surfaces disagree on {:?}: mind2t_vt_* said {ours:?}, ghostty_* said {theirs:?}",
            String::from_utf8_lossy(bytes)
        );
    }
}

#[test]
fn the_default_style_agrees_across_both_surfaces() {
    let size = std::mem::size_of::<GhosttyStyle>();
    let mut ours = GhosttyStyle { size, ..unsafe { std::mem::zeroed() } };
    let mut theirs = GhosttyStyle { size, ..unsafe { std::mem::zeroed() } };
    unsafe {
        native::mind2t_vt_style_default(&mut ours);
        exports::ghostty_style_default(&mut theirs);
    }
    assert_eq!(ours.bold, theirs.bold);
    assert_eq!(ours.underline, theirs.underline);
    assert_eq!(ours.fg_color.tag, theirs.fg_color.tag);
    assert_eq!(ours.bg_color.tag, theirs.bg_color.tag);
}
