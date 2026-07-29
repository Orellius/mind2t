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

    #[test]
    fn taking_drains() {
        let mut terminal = Terminal::new(20, 10);
        terminal.write(b"\x1b[5n");
        assert_eq!(terminal.take_replies(), b"\x1b[0n");
        assert_eq!(terminal.take_replies(), b"");
    }
}
