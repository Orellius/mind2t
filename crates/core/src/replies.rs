//! Purpose: the answerback half of slice 9 -- DSR and DA queries produce reply BYTES,
//! queued here because the core does no I/O. The pump drains them to the pty; the seam
//! is `Terminal::take_replies`.
//! Reference, measured from the oracle's source 2026-07-30 (its ABI exposes no reply
//! surface, so source + unit tests gate this, the OSC 8 precedent):
//!   - DSR 5  -> `ESC[0n`                      (stream_handler.zig:795)
//!   - DSR 6  -> `ESC[{y+1};{x+1}R`, origin-relative under DECOM, saturating at the
//!               region top (stream_handler.zig:801; this core has no left margin)
//!   - DA1    -> `ESC[?62;22c`  vt220 conformance + ansi_color (device_attributes.zig)
//!   - DA2    -> `ESC[>1;0;0c`  vt220 device type, firmware 0, rom 0
//!   - DA3    -> `ESC P!|00000000 ESC \`  DECRPTUI, unit id 0
//! Why a separate queue from `events.rs`: replies are PROTOCOL, addressed to the child
//! and ordered against its input; events are UI requests addressed to the embedder.
//! Collapsing them would make the pump route every item by kind for no gain.

use crate::terminal::State;

impl State {
    /// DSR (CSI Ps n). 5 = operating status, 6 = cursor position report.
    pub(crate) fn device_status_report(&mut self, request: u16) {
        match request {
            5 => self.replies.extend_from_slice(b"\x1b[0n"),
            6 => {
                let screen = self.screen();
                let (mut row, column) = (screen.y, screen.x);
                if self.origin {
                    // Origin mode reports relative to the scroll region's top, exactly
                    // as the cursor is addressed. Saturating: a cursor parked above the
                    // region (possible after the region moved) reports row 1.
                    row = row.saturating_sub(self.screen().scroll_top);
                }
                let reply = format!("\x1b[{};{}R", row + 1, column + 1);
                self.replies.extend_from_slice(reply.as_bytes());
            }
            _ => {}
        }
    }

    pub(crate) fn device_attributes_primary(&mut self) {
        self.replies.extend_from_slice(b"\x1b[?62;22c");
    }

    pub(crate) fn device_attributes_secondary(&mut self) {
        self.replies.extend_from_slice(b"\x1b[>1;0;0c");
    }

    pub(crate) fn device_attributes_tertiary(&mut self) {
        self.replies.extend_from_slice(b"\x1bP!|00000000\x1b\\");
    }

    /// DECRQM (CSI Pm $ p, or CSI ? Pd $ p) -> DECRPM `CSI [?]{mode};{state}$y`.
    ///
    /// The oracle's contract, mirrored from source (modes.zig `getReport` + `encode`):
    /// state 1 = set, 2 = reset, 0 = not recognized; 3/4 (permanently set/reset) are
    /// never produced. This core answers 1/2 ONLY for modes it genuinely tracks --
    /// claiming an answer for a mode that has no behavior here would tell a program
    /// (this is how 2026-era TUIs detect synchronized output) that a feature exists
    /// when it does not. Everything else, and every ANSI-form mode, answers 0. Not
    /// gated behind the reports grant: mode state is protocol, not screen content,
    /// and the oracle answers unconditionally.
    pub(crate) fn mode_report(&mut self, mode: u16, ansi: bool) {
        let state: u8 = if ansi {
            0
        } else {
            let tracked = match mode {
                6 => Some(self.origin),
                7 => Some(self.autowrap),
                25 => Some(self.cursor_visible),
                47 | 1047 | 1049 => Some(matches!(self.active, crate::terminal::Active::Alternate)),
                2004 => Some(self.bracketed_paste),
                2026 => Some(self.synchronized_output),
                _ => None,
            };
            match tracked {
                Some(true) => 1,
                Some(false) => 2,
                None => 0,
            }
        };
        let prefix = if ansi { "" } else { "?" };
        let reply = format!("\x1b[{prefix}{mode};{state}$y");
        self.replies.extend_from_slice(reply.as_bytes());
    }

    /// DECRQCRA (CSI Pid;Pp;Pt;Pl;Pb;Pr * y) -> DCS Pid ! ~ XXXX ST.
    ///
    /// Modern xterm semantics, which is what esctest2 asserts against: the checksum of a
    /// single plain cell IS its codepoint (esctest compares per-cell checksums to
    /// `ord(expected_char)`), a blank cell counts as 0x20 the way xterm's screen stores
    /// spaces, and the sum is 16-bit wrapping, reported as four uppercase hex digits.
    /// Pre-279 xterm negated the sum; esctest carries a compatibility branch for that,
    /// so the positive form is the current contract. Coordinates are 1-based inclusive,
    /// clamped to the screen; missing or zero rect params mean the full screen. Gated on
    /// `reports_enabled` -- a checksum lets the child read screen content back.
    pub(crate) fn checksum_report(&mut self, id: u16, rect: [u16; 4]) {
        if !self.reports_enabled {
            return;
        }
        let screen = self.screen();
        let (cols, rows) = (screen.cols(), screen.rows());
        let top = rect[0].max(1).min(rows);
        let left = rect[1].max(1).min(cols);
        let bottom = if rect[2] == 0 { rows } else { rect[2].min(rows) };
        let right = if rect[3] == 0 { cols } else { rect[3].min(cols) };

        let mut sum: u16 = 0;
        for y in (top - 1)..bottom {
            for x in (left - 1)..right {
                let cell = screen.grid.cell(screen.grid.index(x, y));
                let codepoint = if cell.codepoint == 0 { 0x20 } else { cell.codepoint };
                sum = sum.wrapping_add(codepoint as u16);
            }
        }

        let reply = format!("\x1bP{id}!~{sum:04X}\x1b\\");
        self.replies.extend_from_slice(reply.as_bytes());
    }

    /// XTERM_WINOPS reports (CSI Ps t). Only 18 -- the text area size in characters,
    /// `CSI 8 ; rows ; cols t` -- because it is what esctest's `GetScreenSize` runs on
    /// at init. The resize/move ops in the same namespace are refused outright (a child
    /// moving the operator's window is not a feature), and the pixel reports wait for a
    /// consumer. Gated with the checksum: both let the child inspect the screen.
    pub(crate) fn window_report(&mut self, op: u16) {
        if !self.reports_enabled || op != 18 {
            return;
        }
        let screen = self.screen();
        let reply = format!("\x1b[8;{};{}t", screen.rows(), screen.cols());
        self.replies.extend_from_slice(reply.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use crate::terminal::Terminal;

    fn replies_for(bytes: &[u8]) -> Vec<u8> {
        let mut terminal = Terminal::new(20, 10);
        terminal.write(bytes);
        terminal.take_replies()
    }

    #[test]
    fn dsr_five_reports_ok() {
        assert_eq!(replies_for(b"\x1b[5n"), b"\x1b[0n");
    }

    #[test]
    fn cpr_is_one_based_and_follows_the_cursor() {
        assert_eq!(replies_for(b"\x1b[6n"), b"\x1b[1;1R");
        assert_eq!(replies_for(b"\x1b[4;7H\x1b[6n"), b"\x1b[4;7R");
    }

    #[test]
    fn cpr_under_origin_mode_is_region_relative() {
        // Region rows 3..8, DECOM on homes to the region top; CPR must report 1;1
        // there, not 3;1 -- the exact split the oracle's origin branch implements.
        assert_eq!(replies_for(b"\x1b[3;8r\x1b[?6h\x1b[6n"), b"\x1b[1;1R");
        assert_eq!(replies_for(b"\x1b[3;8r\x1b[?6h\x1b[2;1H\x1b[6n"), b"\x1b[2;1R");
    }

    #[test]
    fn the_three_device_attribute_answers_mirror_the_oracle() {
        assert_eq!(replies_for(b"\x1b[c"), b"\x1b[?62;22c");
        assert_eq!(replies_for(b"\x1b[0c"), b"\x1b[?62;22c");
        assert_eq!(replies_for(b"\x1b[>c"), b"\x1b[>1;0;0c");
        assert_eq!(replies_for(b"\x1b[=c"), b"\x1bP!|00000000\x1b\\");
    }

    #[test]
    fn an_unknown_dsr_answers_nothing() {
        assert_eq!(replies_for(b"\x1b[7n"), b"");
        assert_eq!(replies_for(b"\x1b[n"), b"", "a missing parameter is 0, not 5");
    }

    fn reporting_terminal(bytes: &[u8]) -> Vec<u8> {
        let mut terminal = Terminal::new(20, 10);
        terminal.enable_reports(true);
        terminal.write(bytes);
        terminal.take_replies()
    }

    #[test]
    fn a_single_cell_checksum_is_its_codepoint() {
        // The exact per-cell contract esctest2 asserts: checksum == ord(char).
        assert_eq!(
            reporting_terminal(b"AB\x1b[1;1;1;1;1;1*y"),
            format!("\x1bP1!~{:04X}\x1b\\", u32::from('A')).as_bytes()
        );
        assert_eq!(
            reporting_terminal(b"AB\x1b[7;1;1;2;1;2*y"),
            format!("\x1bP7!~{:04X}\x1b\\", u32::from('B')).as_bytes()
        );
    }

    #[test]
    fn a_blank_cell_checksums_as_a_space() {
        // xterm's screen stores blanks as spaces; a zero here would make every esctest
        // assertion over an erased region fail.
        assert_eq!(
            reporting_terminal(b"\x1b[1;1;5;1;5;1*y"),
            b"\x1bP1!~0020\x1b\\"
        );
    }

    #[test]
    fn a_rect_checksum_sums_its_cells() {
        let expected: u16 = (u32::from('h') + u32::from('i')) as u16;
        assert_eq!(
            reporting_terminal(b"hi\x1b[2;1;1;1;1;2*y"),
            format!("\x1bP2!~{expected:04X}\x1b\\").as_bytes()
        );
    }

    #[test]
    fn the_size_report_answers_rows_then_cols() {
        assert_eq!(reporting_terminal(b"\x1b[18t"), b"\x1b[8;10;20t");
    }

    #[test]
    fn reports_are_silent_unless_enabled() {
        // The gate, proven in the failing direction: the same queries on a default
        // terminal answer NOTHING -- screen readback is opt-in (the OSC 52 posture).
        assert_eq!(replies_for(b"\x1b[1;1;1;1;1;1*y"), b"");
        assert_eq!(replies_for(b"\x1b[18t"), b"");
    }

    #[test]
    fn other_winops_are_refused_even_when_enabled() {
        // 2 = iconify, 3 = move: a child steering the operator's window is not a
        // feature. 14/16 (pixel reports) wait for a consumer.
        assert_eq!(reporting_terminal(b"\x1b[2t\x1b[3;0;0t\x1b[14t\x1b[16t"), b"");
    }

    #[test]
    fn decrqm_answers_only_for_modes_this_core_actually_tracks() {
        // The detection handshake for synchronized output: reset -> set -> reset again
        // after the mode drops. A wrong 0 here and every 2026-era TUI falls back.
        assert_eq!(replies_for(b"\x1b[?2026$p"), b"\x1b[?2026;2$y");
        assert_eq!(replies_for(b"\x1b[?2026h\x1b[?2026$p"), b"\x1b[?2026;1$y");
        assert_eq!(replies_for(b"\x1b[?25l\x1b[?25$p"), b"\x1b[?25;2$y");
        assert_eq!(replies_for(b"\x1b[?1049h\x1b[?1049$p"), b"\x1b[?1049;1$y");
        // Untracked DEC mode and every ANSI-form mode: not recognized, honestly.
        assert_eq!(replies_for(b"\x1b[?2027$p"), b"\x1b[?2027;0$y");
        assert_eq!(replies_for(b"\x1b[4$p"), b"\x1b[4;0$y");
    }

    #[test]
    fn a_resize_ends_a_synchronized_batch_even_at_the_same_size() {
        // The oracle's own rule, measured: its resize clears 2026 even when the cell
        // dimensions did not change.
        let mut terminal = Terminal::new(20, 10);
        terminal.write(b"\x1b[?2026h");
        assert!(terminal.synchronized_output());
        terminal.resize(20, 10);
        assert!(!terminal.synchronized_output(), "unchanged dimensions still clear it");

        terminal.write(b"\x1b[?2026h");
        terminal.resize(30, 10);
        assert!(!terminal.synchronized_output());
    }

    #[test]
    fn decstr_resets_state_but_never_the_grid() {
        let mut terminal = Terminal::new(20, 10);
        // Content on screen, cursor hidden, origin mode inside a region, red pen.
        terminal.write(b"keepme\x1b[?25l\x1b[3;6r\x1b[?6h\x1b[31m\x1b[!p");

        assert!(terminal.cursor().visible, "DECSTR shows the cursor (DECTCEM)");
        let snapshot = terminal.snapshot();
        assert_eq!(snapshot.row_text(0), "keepme", "the grid is RIS's job, not DECSTR's");

        // Origin off and margins reset: absolute addressing reaches row 1 again, and
        // the pen is back to default.
        terminal.write(b"\x1b[1;8HX");
        let snapshot = terminal.snapshot();
        assert_eq!(snapshot.row_text(0), "keepme X");
        let cell = &snapshot.grid[0].cells[7];
        assert_eq!(cell.style, ruuah_vt_snapshot::Style::DEFAULT, "SGR was soft-reset");
    }

    #[test]
    fn a_full_reset_keeps_the_embedders_reports_grant() {
        // RIS resets terminal state; the reports toggle is the EMBEDDER's grant, and a
        // child revoking it would break esctest's readback mid-run.
        let mut terminal = Terminal::new(20, 10);
        terminal.enable_reports(true);
        terminal.write(b"\x1bc\x1b[18t");
        assert_eq!(terminal.take_replies(), b"\x1b[8;10;20t");
    }

    #[test]
    fn taking_drains() {
        let mut terminal = Terminal::new(20, 10);
        terminal.write(b"\x1b[5n");
        assert_eq!(terminal.take_replies(), b"\x1b[0n");
        assert_eq!(terminal.take_replies(), b"");
    }
}
