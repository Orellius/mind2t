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

/// DECRPM state byte for a tracked boolean: 1 = set, 2 = reset.
fn report_bool(on: bool) -> u8 {
    if on { 1 } else { 2 }
}

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

    /// DA1, and the ONE place this core deliberately claims more than the oracle.
    ///
    /// Ghostty answers `?62;22c` -- vt220 conformance plus ansi_color -- and omits
    /// attribute 4 because it has no sixel decoder at all; its only sixel mention
    /// IS the capability table this list comes from (`device_attributes.zig:53`,
    /// `sixel = 4`). This core does decode sixel, so staying silent would be the
    /// inaccurate answer: `img2sixel` and friends probe DA1 and fall back to
    /// nothing when 4 is absent, which is a working decoder no tool will ever use.
    ///
    /// Attributes ascend, so 4 goes before 22. Divergence from the oracle is
    /// deliberate, corpus-pinned, and named in BACKLOG-2026.md -- and it is
    /// asymmetric on purpose: advertising a capability we HAVE is a different
    /// claim from matching a reference implementation that lacks it.
    pub(crate) fn device_attributes_primary(&mut self) {
        self.replies.extend_from_slice(b"\x1b[?62;4;22c");
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
            match mode {
                // The four ANSI modes actually tracked (the oracle's own set).
                2 => report_bool(self.kam),
                4 => report_bool(self.insert_mode),
                12 => report_bool(self.send_receive),
                20 => report_bool(self.linefeed_mode),
                // The VT180-era print-form modes (GATM, SRTM, VEM, HEM, PUM, FEAM,
                // FETM, MATM, TTM, SATM, TSM, EBM): recognized and PERMANENTLY
                // RESET, xterm's own answer. This is a reply-only divergence from
                // the oracle (it says 0 = unknown); 4 is the more truthful claim -
                // these modes are known, refused, and never coming back.
                1 | 5 | 7 | 10 | 11 | 13 | 14 | 15 | 16 | 17 | 18 | 19 => 4,
                _ => 0,
            }
        } else {
            let tracked = match mode {
                1 => Some(self.cursor_keys),
                4 => Some(self.slow_scroll),
                5 => Some(self.reverse_colors),
                67 => Some(self.backarrow),
                6 => Some(self.origin),
                66 => Some(self.keypad_keys),
                1035 => Some(self.ignore_keypad_with_numlock),
                1036 => Some(self.alt_esc_prefix),
                7 => Some(self.autowrap),
                25 => Some(self.cursor_visible),
                47 | 1047 | 1049 => Some(matches!(self.active, crate::terminal::Active::Alternate)),
                2004 => Some(self.bracketed_paste),
                2026 => Some(self.synchronized_output),
                // The mouse family answers from the RAW bits, not the derived kind:
                // after 1000h 1002h 1002l the oracle's table still holds 1000 set, so
                // DECRQM(1000) is 1 even though reporting is off. xterm agrees.
                9 | 1000 | 1002 | 1003 | 1005 | 1006 | 1015 | 1016 | 1007 => {
                    self.mouse.raw_bit(mode)
                }
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

    /// DECSCUSR (CSI Ps SP q). Values 0-6; 0 means 1 (the VT rule); odd = blinking.
    /// Anything past 6 is ignored whole, mirroring the oracle's enum cast failure.
    pub(crate) fn set_cursor_shape(&mut self, style: u16) {
        let (shape, blink) = match style {
            0 | 1 => (0, true),
            2 => (0, false),
            3 => (1, true),
            4 => (1, false),
            5 => (2, true),
            6 => (2, false),
            _ => return,
        };
        self.cursor_shape = shape;
        self.cursor_blink = blink;
    }

    /// DECRQSS (DCS $ q <setting> ST) -> DECRPSS `DCS {1|0} $ r <value><setting> ST`.
    ///
    /// The supported settings mirror the ORACLE's exactly (`dcs.zig` DECRQSS enum):
    /// SGR ("m"), DECSCUSR (" q"), DECSTBM ("r"), and DECSLRM ("s") which answers
    /// invalid here because left/right margins are not tracked -- the same answer the
    /// oracle gives with mode 69 off. Everything else, xterm's wider catalogue
    /// included, is `DCS 0 $ r ST`: an honest "I do not carry that setting", the
    /// DECRQM posture applied to DCS.
    pub(crate) fn decrqss_reply(&mut self, setting: &[u8]) {
        let body: Option<String> = match setting {
            b"m" => Some(self.sgr_report()),
            b"r" => {
                let screen = self.screen();
                Some(format!("{};{}r", screen.scroll_top + 1, screen.scroll_bottom + 1))
            }
            b" q" => {
                // The inverse of set_cursor_shape's table, the oracle's own mapping:
                // blink picks the odd value of each pair.
                let style = match (self.cursor_shape, self.cursor_blink) {
                    (0, true) => 1,
                    (0, false) => 2,
                    (1, true) => 3,
                    (1, false) => 4,
                    (2, true) => 5,
                    _ => 6,
                };
                Some(format!("{style} q"))
            }
            _ => None,
        };
        let reply = match body {
            Some(body) => format!("\x1bP1$r{body}\x1b\\"),
            None => "\x1bP0$r\x1b\\".to_string(),
        };
        self.replies.extend_from_slice(reply.as_bytes());
    }

    /// The SGR half of DECRPSS, ported from the oracle's `printAttributes`
    /// (`Terminal.zig:3545`): always leads with 0, flags in fixed order (bold, faint,
    /// italic, underline, blink, inverse, invisible, strikethrough -- overline and the
    /// underline colour are NOT reported, matching it), non-single underlines use the
    /// colon subparameter form, and colours split palette<8 / bright<16 / indexed /
    /// direct exactly as it does.
    fn sgr_report(&self) -> String {
        use ruuah_vt_snapshot::{Color, Underline};
        use std::fmt::Write as _;
        let pen = &self.pen;
        let mut out = String::from("0");
        if pen.bold {
            out.push_str(";1");
        }
        if pen.faint {
            out.push_str(";2");
        }
        if pen.italic {
            out.push_str(";3");
        }
        match pen.underline {
            Underline::None => {}
            Underline::Single => out.push_str(";4"),
            other => {
                let kind = match other {
                    Underline::Double => 2,
                    Underline::Curly => 3,
                    Underline::Dotted => 4,
                    Underline::Dashed => 5,
                    _ => unreachable!(),
                };
                let _ = write!(out, ";4:{kind}");
            }
        }
        if pen.blink {
            out.push_str(";5");
        }
        if pen.inverse {
            out.push_str(";7");
        }
        if pen.invisible {
            out.push_str(";8");
        }
        if pen.strikethrough {
            out.push_str(";9");
        }
        match pen.fg {
            Color::Default => {}
            Color::Palette(i) if i < 8 => {
                let _ = write!(out, ";3{i}");
            }
            Color::Palette(i) if i < 16 => {
                let _ = write!(out, ";9{}", i - 8);
            }
            Color::Palette(i) => {
                let _ = write!(out, ";38:5:{i}");
            }
            Color::Rgb { r, g, b } => {
                let _ = write!(out, ";38:2::{r}:{g}:{b}");
            }
        }
        match pen.bg {
            Color::Default => {}
            Color::Palette(i) if i < 8 => {
                let _ = write!(out, ";4{i}");
            }
            Color::Palette(i) if i < 16 => {
                let _ = write!(out, ";10{}", i - 8);
            }
            Color::Palette(i) => {
                let _ = write!(out, ";48:5:{i}");
            }
            Color::Rgb { r, g, b } => {
                let _ = write!(out, ";48:2::{r}:{g}:{b}");
            }
        }
        out.push('m');
        out
    }

    /// The kitty keyboard query (`CSI ? u`) -> `CSI ? flags u`, the active screen's
    /// stack top. Unconditional like DECRQM -- this reply is HOW a 2026 CLI detects
    /// kitty support at all; refusing it would silently degrade every one of them.
    pub(crate) fn kitty_keyboard_report(&mut self) {
        let reply = format!("\x1b[?{}u", self.screen().kitty_keyboard.current());
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
    fn the_three_device_attribute_answers_mirror_the_oracle_except_sixel() {
        // DA2 and DA3 are byte-identical to the oracle. DA1 adds attribute 4,
        // because we decode sixel and it does not -- the one place this core
        // deliberately claims MORE. Asserted as exact bytes rather than a
        // "contains 4", so an accidental reordering or a dropped 22 fails here.
        assert_eq!(replies_for(b"\x1b[c"), b"\x1b[?62;4;22c");
        assert_eq!(replies_for(b"\x1b[0c"), b"\x1b[?62;4;22c");
        assert_eq!(replies_for(b"\x1b[>c"), b"\x1b[>1;0;0c");
        assert_eq!(replies_for(b"\x1b[=c"), b"\x1bP!|00000000\x1b\\");
    }

    /// The divergence stated as its own assertion, so deleting it is a visible act
    /// rather than an edit to a list. If sixel support were ever removed, this test
    /// is the one that must be deleted WITH it -- and its failure says why.
    #[test]
    fn da1_advertises_sixel_because_this_core_decodes_it() {
        let reply = replies_for(b"\x1b[c");
        let text = String::from_utf8(reply).unwrap();
        let attributes: Vec<&str> = text
            .trim_start_matches("\x1b[?")
            .trim_end_matches('c')
            .split(';')
            .collect();
        // The FIRST field is the conformance level (62 = vt220), not a capability --
        // the ascent rule applies to what follows it. Asserting otherwise failed on
        // its first run, which is the test doing its job on the test.
        assert_eq!(attributes.first(), Some(&"62"), "conformance level: {text:?}");
        let capabilities = &attributes[1..];
        assert!(capabilities.contains(&"4"), "sixel attribute missing: {text:?}");
        assert!(capabilities.contains(&"22"), "ansi_color must survive: {text:?}");
        let numbers: Vec<u32> = capabilities.iter().map(|a| a.parse().unwrap()).collect();
        assert!(numbers.windows(2).all(|w| w[0] < w[1]), "attributes must ascend: {text:?}");
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

    /// The kitty detection handshake, the exact probe a 2026 CLI sends: query, push,
    /// query again, pop, query a third time. A terminal that cannot answer the first
    /// query is one where every kitty-capable program silently falls back.
    #[test]
    fn the_kitty_query_reports_the_stack_top() {
        assert_eq!(replies_for(b"\x1b[?u"), b"\x1b[?0u");
        assert_eq!(replies_for(b"\x1b[>1u\x1b[?u"), b"\x1b[?1u");
        assert_eq!(replies_for(b"\x1b[>1u\x1b[<u\x1b[?u"), b"\x1b[?0u");
        // The = forms compose on the current entry: set 1, or 2, not 2 -> 1.
        assert_eq!(replies_for(b"\x1b[=1;1u\x1b[=2;2u\x1b[=2;3u\x1b[?u"), b"\x1b[?1u");
    }

    /// The stack is per SCREEN: flags pushed on the alternate screen vanish from the
    /// query the moment 1049 exits, with no pop -- vim's negotiation must not leak
    /// into the shell's.
    #[test]
    fn kitty_flags_are_per_screen() {
        assert_eq!(
            replies_for(b"\x1b[?1049h\x1b[>31u\x1b[?u\x1b[?1049l\x1b[?u"),
            b"\x1b[?31u\x1b[?0u"
        );
    }

    /// A flags value past the five defined bits invalidates the whole command --
    /// measured from the oracle (u5 cast failure warns and drops), not clamped.
    #[test]
    fn out_of_range_kitty_flags_are_dropped_not_clamped() {
        assert_eq!(replies_for(b"\x1b[>32u\x1b[?u"), b"\x1b[?0u");
        assert_eq!(replies_for(b"\x1b[=63;1u\x1b[?u"), b"\x1b[?0u");
    }

    #[test]
    fn decrqm_answers_only_for_modes_this_core_actually_tracks() {
        // The detection handshake for synchronized output: reset -> set -> reset again
        // after the mode drops. A wrong 0 here and every 2026-era TUI falls back.
        assert_eq!(replies_for(b"\x1b[?2026$p"), b"\x1b[?2026;2$y");
        assert_eq!(replies_for(b"\x1b[?2026h\x1b[?2026$p"), b"\x1b[?2026;1$y");
        assert_eq!(replies_for(b"\x1b[?25l\x1b[?25$p"), b"\x1b[?25;2$y");
        assert_eq!(replies_for(b"\x1b[?1049h\x1b[?1049$p"), b"\x1b[?1049;1$y");
        // Untracked DEC mode: not recognized, honestly. ANSI now has three tiers:
        // tracked (IRM answers live state), permanently reset (the VT180 print-form
        // modes answer 4), and genuinely unknown (3 = CRM, answers 0).
        assert_eq!(replies_for(b"\x1b[?2027$p"), b"\x1b[?2027;0$y");
        assert_eq!(replies_for(b"\x1b[4$p"), b"\x1b[4;2$y");
        assert_eq!(replies_for(b"\x1b[4h\x1b[4$p"), b"\x1b[4;1$y");
        assert_eq!(replies_for(b"\x1b[1$p"), b"\x1b[1;4$y", "GATM is permanently reset");
        assert_eq!(replies_for(b"\x1b[3$p"), b"\x1b[3;0$y", "CRM is unknown");
        assert_eq!(replies_for(b"\x1b[12$p"), b"\x1b[12;1$y", "SRM defaults SET");
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

    /// DECRQSS answers exactly the oracle's settings catalogue. The esctest grammar:
    /// ReadDCS sees `1$r<value><setting>`.
    #[test]
    fn decrqss_answers_sgr_region_and_cursor_style() {
        // Plain pen: "0m". Bold + red: "0;1;31m" (flags first, colours after).
        assert_eq!(replies_for(b"\x1bP$qm\x1b\\"), b"\x1bP1$r0m\x1b\\");
        assert_eq!(
            replies_for(b"\x1b[1;31m\x1bP$qm\x1b\\"),
            b"\x1bP1$r0;1;31m\x1b\\"
        );
        // Curly underline uses the colon subparameter form; bright bg the 10x form;
        // indexed and direct colours the 38/48 forms - all printAttributes rules.
        assert_eq!(
            replies_for(b"\x1b[4:3;103m\x1bP$qm\x1b\\"),
            b"\x1bP1$r0;4:3;103m\x1b\\"
        );
        assert_eq!(
            replies_for(b"\x1b[38;5;100;48;2;1;2;3m\x1bP$qm\x1b\\"),
            b"\x1bP1$r0;38:5:100;48:2::1:2:3m\x1b\\"
        );
        // DECSTBM reports the live region, 1-based.
        assert_eq!(replies_for(b"\x1b[5;6r\x1bP$qr\x1b\\"), b"\x1bP1$r5;6r\x1b\\");
        // DECSCUSR round-trips through the tracked shape+blink pair.
        assert_eq!(replies_for(b"\x1b[4 q\x1bP$q q\x1b\\"), b"\x1bP1$r4 q\x1b\\");
        assert_eq!(replies_for(b"\x1bP$q q\x1b\\"), b"\x1bP1$r1 q\x1b\\", "default: blinking block");
    }

    #[test]
    fn decrqss_refuses_what_it_does_not_carry() {
        // DECSLRM without left/right margins, and xterm's wider catalogue (DECSCA
        // here): an honest invalid, exactly the oracle's `0$r` shape.
        assert_eq!(replies_for(b"\x1bP$qs\x1b\\"), b"\x1bP0$r\x1b\\");
        assert_eq!(replies_for(b"\x1bP$q\"q\x1b\\"), b"\x1bP0$r\x1b\\");
        // An oversized request drops whole - the oracle's ignore state, not a reply.
        assert_eq!(replies_for(b"\x1bP$qabc\x1b\\"), b"");
    }

    #[test]
    fn taking_drains() {
        let mut terminal = Terminal::new(20, 10);
        terminal.write(b"\x1b[5n");
        assert_eq!(terminal.take_replies(), b"\x1b[0n");
        assert_eq!(terminal.take_replies(), b"");
    }
}
