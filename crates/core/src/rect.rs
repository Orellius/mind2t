//! Purpose: the DEC rectangle operations - DECFRA (CSI Pch;Pt;Pl;Pb;Pr $ x) fills,
//! DECERA (CSI Pt;Pl;Pb;Pr $ z) erases, DECCRA (CSI ...8 params $ v) copies.
//!
//! Reference: xterm/DEC STD 070 semantics as esctest2 asserts them, because the ORACLE
//! DOES NOT DISPATCH THESE AT ALL (measured 2026-08-01: no `$x`/`$z`/`$v` handling
//! anywhere in `stream_terminal.zig`). Implementing them is therefore a deliberate
//! divergence in xterm's direction, the DECSTR/DECRQCRA pattern: every corpus case
//! carrying one of these sequences is pinned `expect = "diff"` with the exact paths.
//!
//! The rules, from esctest's own assertions (`tests/decrectops.py`):
//!   - coordinates are 1-based inclusive; missing or 0 params take defaults
//!     (top=1, left=1, bottom=rows, right=cols);
//!   - an inverted rectangle (bottom < top or right < left) does NOTHING;
//!   - an oversized rectangle clips to the screen;
//!   - the cursor does not move;
//!   - scroll margins are IGNORED for the ops' extent (they are not fenced);
//!   - under DECOM the row coordinates are region-relative (the column half of that
//!     rule needs left/right margins, which this core does not have yet - the
//!     respectsOriginMode esctest cases stay red until the DECLRMM slice).
//!
//! NOT responsible for: DECSERA (selective erase, needs the protection bit - slice 4)
//! or DECRQSS (a DCS reply, lives in `replies.rs`).

use crate::cell::Cell;
use crate::terminal::State;

/// A resolved rectangle in 0-based inclusive grid coordinates, already clipped.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GridRect {
    pub top: u16,
    pub left: u16,
    pub bottom: u16,
    pub right: u16,
}

impl State {
    /// Resolves 1-based params (0 or missing = default) against the active screen,
    /// applying DECOM's row offset. Returns `None` for an inverted or fully
    /// offscreen rectangle - the do-nothing cases.
    fn resolve_rect(&self, top: u16, left: u16, bottom: u16, right: u16) -> Option<GridRect> {
        let screen = self.screen();
        let (cols, rows) = (screen.cols(), screen.rows());
        if cols == 0 || rows == 0 {
            return None;
        }

        let mut top = if top == 0 { 1 } else { top } - 1;
        let mut bottom = if bottom == 0 { rows } else { bottom } - 1;
        if self.origin {
            // Region-relative rows, exactly as CUP addresses them; the bottom clamps
            // to the region under DECOM, matching the cursor's own fence.
            top = top.saturating_add(screen.scroll_top);
            bottom = bottom.saturating_add(screen.scroll_top).min(screen.scroll_bottom);
        }
        let left = if left == 0 { 1 } else { left } - 1;
        let right = if right == 0 { cols } else { right } - 1;

        bottom = bottom.min(rows - 1);
        let right = right.min(cols - 1);
        if top > bottom || left > right {
            return None;
        }
        Some(GridRect { top, left, bottom, right })
    }

    /// DECFRA. The fill character arrives as a decimal codepoint and is written with
    /// the CURRENT SGR (xterm's rule); controls and anything the width tables call
    /// non-single are refused, whole-command.
    pub(crate) fn fill_rect(&mut self, ch: u16, top: u16, left: u16, bottom: u16, right: u16) {
        use unicode_width::UnicodeWidthChar;
        let Some(ch) = char::from_u32(u32::from(ch)) else { return };
        if ch.is_control() || ch.width() != Some(1) {
            return;
        }
        let Some(rect) = self.resolve_rect(top, left, bottom, right) else { return };
        // Like print: the fill takes the current pen and the cursor's semantic kind,
        // not whatever the overwritten cells carried.
        let pen = self.pen;
        let semantic = self.screen().semantic_content;
        let style_id = self.screen_mut().grid.intern_style(pen);
        let cell = Cell {
            codepoint: ch as u32,
            style_id,
            wide: crate::cell::Wide::Narrow,
            flags: crate::cell::CellFlags::with_semantic(semantic),
        };
        let grid = &mut self.screen_mut().grid;
        for y in rect.top..=rect.bottom {
            for x in rect.left..=rect.right {
                let index = grid.index(x, y);
                grid.write(index, cell);
            }
        }
    }

    /// DECERA: erase to the same blank the in-band erases use (BCE - the blank
    /// carries the pen background), cursor untouched.
    pub(crate) fn erase_rect(&mut self, top: u16, left: u16, bottom: u16, right: u16) {
        let Some(rect) = self.resolve_rect(top, left, bottom, right) else { return };
        let blank = self.blank();
        let grid = &mut self.screen_mut().grid;
        for y in rect.top..=rect.bottom {
            grid.clear_span(y, rect.left, rect.right, blank);
        }
    }

    /// DECSERA (CSI Pt;Pl;Pb;Pr $ {): DECERA's geometry, DECSED's protection rule -
    /// only unprotected cells are blanked.
    pub(crate) fn selective_erase_rect(&mut self, top: u16, left: u16, bottom: u16, right: u16) {
        let Some(rect) = self.resolve_rect(top, left, bottom, right) else { return };
        let blank = self.blank();
        let grid = &mut self.screen_mut().grid;
        for y in rect.top..=rect.bottom {
            for x in rect.left..=rect.right {
                let index = grid.index(x, y);
                if grid.cell(index).flags.protection() != crate::cell::Protection::Dec {
                    grid.write(index, blank);
                }
            }
        }
    }

    /// DECCRA. Page parameters are accepted and ignored (one page here). The source
    /// clips to the screen; the destination clips on write. Capture-then-write makes
    /// overlap safe in both directions, and carries grapheme continuations and link
    /// stamps with the cells - a copy that drops combining marks is data corruption.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn copy_rect(
        &mut self,
        src_top: u16,
        src_left: u16,
        src_bottom: u16,
        src_right: u16,
        dst_top: u16,
        dst_left: u16,
    ) {
        let Some(src) = self.resolve_rect(src_top, src_left, src_bottom, src_right) else {
            return;
        };
        let screen = self.screen();
        let (cols, rows) = (screen.cols(), screen.rows());
        let mut dst_top = if dst_top == 0 { 1 } else { dst_top } - 1;
        let dst_left = if dst_left == 0 { 1 } else { dst_left } - 1;
        if self.origin {
            dst_top = dst_top.saturating_add(screen.scroll_top);
        }
        if dst_top >= rows || dst_left >= cols {
            return;
        }

        struct Captured {
            cell: Cell,
            continuations: Vec<char>,
            link: Option<u16>,
        }

        let grid = &self.screen().grid;
        let mut cluster = String::new();
        let mut captured: Vec<Vec<Captured>> = Vec::new();
        for y in src.top..=src.bottom {
            let mut row = Vec::with_capacity(usize::from(src.right - src.left + 1));
            for x in src.left..=src.right {
                let index = grid.index(x, y);
                let cell = grid.cell(index);
                let continuations = if cell.flags.has_grapheme() {
                    cluster.clear();
                    grid.cluster_into(index, &mut cluster);
                    cluster.chars().skip(1).collect()
                } else {
                    Vec::new()
                };
                row.push(Captured { cell, continuations, link: grid.link_id(index) });
            }
            captured.push(row);
        }

        let grid = &mut self.screen_mut().grid;
        for (dy, row) in captured.into_iter().enumerate() {
            let Ok(dy) = u16::try_from(dy) else { break };
            let y = dst_top + dy;
            if y >= rows {
                break;
            }
            for (dx, mut captured) in row.into_iter().enumerate() {
                let Ok(dx) = u16::try_from(dx) else { break };
                let x = dst_left + dx;
                if x >= cols {
                    break;
                }
                let index = grid.index(x, y);
                // `write` clears the old side-map entries; the flag must match what
                // gets re-pushed or a stale bit points at an empty map entry.
                captured.cell.flags.set_has_grapheme(false);
                grid.write(index, captured.cell);
                for c in captured.continuations {
                    grid.push_grapheme(index, c);
                }
                if let Some(link) = captured.link {
                    grid.set_link(index, link);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::terminal::Terminal;

    fn filled(bytes: &[u8]) -> Terminal {
        let mut terminal = Terminal::new(8, 4);
        terminal.write(b"abcdefgh\r\nijklmnop\r\nqrstuvwx\r\nyz012345");
        terminal.write(bytes);
        terminal
    }

    fn rows(terminal: &Terminal) -> Vec<String> {
        let snapshot = terminal.snapshot();
        (0..4).map(|y| snapshot.row_text(y)).collect()
    }

    #[test]
    fn decfra_fills_an_inclusive_rect_with_the_current_pen() {
        // 37 = '%'. Rows 2..3, cols 2..3, red pen: the fill carries the pen.
        let terminal = filled(b"\x1b[31m\x1b[37;2;2;3;3$x");
        assert_eq!(rows(&terminal), ["abcdefgh", "i%%lmnop", "q%%tuvwx", "yz012345"]);
        let snapshot = terminal.snapshot();
        let cell = &snapshot.grid[1].cells[1];
        assert_eq!(cell.style.fg, mind2t_vt_snapshot::Color::Palette(1), "pen rides the fill");
    }

    #[test]
    fn an_inverted_rect_does_nothing() {
        let terminal = filled(b"\x1b[37;3;3;2;2$x");
        assert_eq!(rows(&terminal), ["abcdefgh", "ijklmnop", "qrstuvwx", "yz012345"]);
    }

    #[test]
    fn missing_params_default_to_the_whole_screen_and_oversize_clips() {
        let all = filled(b"\x1b[42$x");
        assert_eq!(rows(&all), ["********", "********", "********", "********"]);
        let clipped = filled(b"\x1b[37;3;7;99;99$x");
        assert_eq!(rows(&clipped), ["abcdefgh", "ijklmnop", "qrstuv%%", "yz0123%%"]);
    }

    #[test]
    fn the_cursor_does_not_move_and_margins_do_not_fence() {
        let mut terminal = Terminal::new(8, 4);
        terminal.write(b"\x1b[2;3r\x1b[2;4H");
        terminal.write(b"\x1b[37;1;1;4;8$x");
        let snapshot = terminal.snapshot();
        assert_eq!((snapshot.cursor.y, snapshot.cursor.x), (1, 3), "cursor parked");
        assert_eq!(snapshot.row_text(0), "%%%%%%%%", "above the scroll region");
        assert_eq!(snapshot.row_text(3), "%%%%%%%%", "below the scroll region");
    }

    #[test]
    fn decom_offsets_rect_rows_by_the_region_top() {
        // Region rows 2..4, DECOM on: a rect at row 1 lands on absolute row 2.
        let terminal = filled(b"\x1b[2;4r\x1b[?6h\x1b[37;1;2;1;3$x");
        assert_eq!(rows(&terminal), ["abcdefgh", "i%%lmnop", "qrstuvwx", "yz012345"]);
    }

    #[test]
    fn decera_erases_to_blank() {
        let terminal = filled(b"\x1b[2;2;3;3$z");
        assert_eq!(rows(&terminal), ["abcdefgh", "i  lmnop", "q  tuvwx", "yz012345"]);
    }

    #[test]
    fn deccra_copies_without_moving_the_source() {
        // Copy rows 1..2 x cols 1..2 ("ab"/"ij") to 3,5.
        let terminal = filled(b"\x1b[1;1;2;2;1;3;5;1$v");
        assert_eq!(rows(&terminal), ["abcdefgh", "ijklmnop", "qrstabwx", "yz01ij45"]);
    }

    #[test]
    fn deccra_overlap_is_safe_in_both_directions() {
        // Down-right into itself: source 1,1-2,2 dest 2,2.
        let down = filled(b"\x1b[1;1;2;2;1;2;2;1$v");
        assert_eq!(rows(&down), ["abcdefgh", "iablmnop", "qijtuvwx", "yz012345"]);
        // Up-left over itself: source 2,2-3,3 dest 1,1.
        let up = filled(b"\x1b[2;2;3;3;1;1;1;1$v");
        assert_eq!(rows(&up)[0], "jkcdefgh");
        assert_eq!(rows(&up)[1], "rsklmnop");
    }

    #[test]
    fn deccra_clips_the_destination_at_the_screen_edge() {
        // Source rows 1-2 x cols 1-4 aimed at 4,7: only the first source row's first
        // two cells land (row 5 and column 9 are off the 8x4 screen).
        let terminal = filled(b"\x1b[1;1;2;4;1;4;7;1$v");
        assert_eq!(rows(&terminal)[3], "yz0123ab");
    }

    #[test]
    fn deccra_carries_grapheme_continuations() {
        let mut terminal = Terminal::new(8, 4);
        terminal.write("b\u{05B8}xy".as_bytes());
        terminal.write(b"\x1b[1;1;1;1;1;2;1;1$v");
        let snapshot = terminal.snapshot();
        assert_eq!(snapshot.grid[1].cells[0].text, "b\u{05B8}", "the cluster travelled whole");
    }
}

#[cfg(test)]
mod margin_tests {
    use crate::terminal::Terminal;

    fn rows(terminal: &Terminal, n: u16) -> Vec<String> {
        let snapshot = terminal.snapshot();
        (0..n).map(|y| snapshot.row_text(y.into())).collect()
    }

    /// The crash the whole-suite gate found the expensive way. Deleting every cell
    /// from column 0 to the right margin made the bounded DCH compute `last - count`
    /// as `at_x - 1` and underflow, panicking inside the PTY pump thread - so the
    /// terminal stopped answering and esctest hung for its full 600s timeout rather
    /// than failing. Nothing in the unit suite could see it; this is that test.
    #[test]
    fn deleting_a_whole_span_does_not_underflow() {
        let mut terminal = Terminal::new(8, 2);
        terminal.write(b"\x1b[?69h\x1b[1;4s\x1b[1;1Habcd\x1b[1;1H\x1b[9P");
        assert_eq!(rows(&terminal, 2)[0], "", "the band emptied without panicking");

        // The same shape without margins, and an insert that fills the span.
        let mut terminal = Terminal::new(8, 2);
        terminal.write(b"abcd\x1b[1;1H\x1b[99P");
        assert_eq!(rows(&terminal, 2)[0], "");
        let mut terminal = Terminal::new(8, 2);
        terminal.write(b"\x1b[?69h\x1b[1;4s\x1b[1;1Habcd\x1b[1;1H\x1b[9@");
        assert_eq!(rows(&terminal, 2)[0], "");
    }

    /// CUF stops at the right margin while CUB crosses the left one. The asymmetry
    /// is the oracle's (its cursorLeft fast path ignores margins entirely), and it
    /// is the rule this slice got wrong before the differential corrected it.
    #[test]
    fn cuf_stops_at_the_right_margin_but_cub_crosses_the_left() {
        let mut terminal = Terminal::new(20, 2);
        terminal.write(b"\x1b[?69h\x1b[3;6s\x1b[1;3H\x1b[20C");
        assert_eq!(terminal.snapshot().cursor.x, 5, "CUF clamped to the right margin");

        let mut terminal = Terminal::new(20, 2);
        terminal.write(b"\x1b[?69h\x1b[3;6s\x1b[1;3H\x1b[20D");
        assert_eq!(terminal.snapshot().cursor.x, 0, "CUB walked past the left margin");
    }

    /// DECSLRM is refused unless DECLRMM is on, and an inverted or empty band is
    /// refused outright rather than clamped - so a program cannot be handed a region
    /// it did not ask for.
    #[test]
    fn decslrm_needs_mode_69_and_refuses_a_degenerate_band() {
        let mut terminal = Terminal::new(20, 2);
        // Without mode 69 this is SCOSC, so printing still wraps at the screen edge.
        terminal.write(b"\x1b[3;6s\x1b[1;1H");
        terminal.write("abcdefgh".as_bytes());
        assert_eq!(rows(&terminal, 2)[0], "abcdefgh", "no band was set");

        let mut terminal = Terminal::new(20, 2);
        terminal.write(b"\x1b[?69h\x1b[6;3s\x1b[1;1H");
        terminal.write("abcdefgh".as_bytes());
        assert_eq!(rows(&terminal, 2)[0], "abcdefgh", "inverted band refused");
    }

    /// Turning DECLRMM off resets the margins, which is what esctest's own reset
    /// depends on between tests.
    #[test]
    fn clearing_mode_69_resets_the_margins() {
        let mut terminal = Terminal::new(20, 2);
        terminal.write(b"\x1b[?69h\x1b[3;6s\x1b[?69l\x1b[1;1H");
        terminal.write("abcdefgh".as_bytes());
        assert_eq!(rows(&terminal, 2)[0], "abcdefgh");
    }
}

#[cfg(test)]
mod protection_tests {
    use crate::terminal::Terminal;

    fn rows(terminal: &Terminal, n: u16) -> Vec<String> {
        let snapshot = terminal.snapshot();
        (0..n).map(|y| snapshot.row_text(y.into())).collect()
    }

    /// The esctest shape: protected text survives a selective erase, unprotected
    /// neighbours do not, and DECSCA 2 unprotects exactly like DECSCA 0.
    #[test]
    fn decsed_spares_protected_cells_only() {
        let mut terminal = Terminal::new(8, 2);
        terminal.write(b"\x1b[1\"qbcd\x1b[2\"qX\x1b[1;1H\x1b[?0J");
        assert_eq!(rows(&terminal, 2)[0], "bcd");
    }

    #[test]
    fn plain_ed_erases_protected_cells_too() {
        let mut terminal = Terminal::new(8, 2);
        terminal.write(b"\x1b[1\"qbcd\x1b[1;1H\x1b[0J");
        assert_eq!(rows(&terminal, 2)[0], "");
    }

    #[test]
    fn decsel_and_decsera_respect_protection_and_decera_does_not() {
        let mut terminal = Terminal::new(8, 2);
        terminal.write(b"a\x1b[1\"qb\x1b[0\"qc\x1b[1;1H\x1b[?2K");
        assert_eq!(rows(&terminal, 2)[0], " b", "DECSEL spares the protected b");

        let mut terminal = Terminal::new(8, 2);
        terminal.write(b"a\x1b[1\"qb\x1b[0\"qc\x1b[1;1;1;8${");
        assert_eq!(rows(&terminal, 2)[0], " b", "DECSERA spares the protected b");

        let mut terminal = Terminal::new(8, 2);
        terminal.write(b"a\x1b[1\"qb\x1b[0\"qc\x1b[1;1;1;8$z");
        assert_eq!(rows(&terminal, 2)[0], "", "DECERA ignores protection");
    }

    /// DECSTR drops the protecting pen AND strips protection from the screen -
    /// deliberate, and the reason is test isolation: protection is built to survive
    /// erasure, so without this a protected cell outlives every reset esctest runs
    /// between tests and lands in the next test's screen. Measured: that is exactly
    /// what regressed EDTests.test_ED_0 when protection first landed.
    #[test]
    fn decstr_strips_protection_from_the_screen() {
        let mut terminal = Terminal::new(8, 2);
        terminal.write(b"\x1b[1\"qab\x1b[!p\x1b[1;1H\x1b[?0J");
        assert_eq!(rows(&terminal, 2)[0], "", "DECSTR unprotected ab, so DECSED took it");

        // Without the DECSTR, the same protected cells survive - the control that
        // proves the strip is what did the work.
        let mut terminal = Terminal::new(8, 2);
        terminal.write(b"\x1b[1\"qab\x1b[1;1H\x1b[?0J");
        assert_eq!(rows(&terminal, 2)[0], "ab");
    }
}
