//! Purpose: track the DEC private mouse-reporting modes (9, 1000, 1002, 1003, 1005,
//!   1006, 1015, 1016, 1007) exactly as the oracle stores them.
//! Public surface: `MouseEvent`, `MouseFormat`, and (crate-side) `MouseModes`.
//! Why this file: the core TRACKS mouse state and encodes nothing -- reports are an
//!   input encoding written by the host, like paste. Two representations coexist by
//!   measurement, not choice: the oracle records every mode's RAW BIT in its mode
//!   table (`modes.zig` -- what `ghostty_terminal_mode_get` answers from, so the raw
//!   bits are the differential observable and what DECRQM answers) AND a DERIVED
//!   last-writer-wins pair (`stream_terminal.zig` `setMode`: enabling any event mode
//!   replaces the previous one, disabling ANY of them yields `.none`; formats
//!   likewise fall back to x10). Dropping either half loses observable behavior.
//! NOT responsible for: encoding reports (`ruuah-vt-pty`'s `mouse.rs`), deciding wheel
//!   routing (host policy), or answering DECRQM (`replies.rs` reads the raw bits).
//! Test strategy: derived semantics unit-tested here against the measured rules;
//!   raw bits corpus-pinned through `ghostty_terminal_mode_get` per case.

/// Which pointer events the child asked to receive. The derived, last-writer-wins
/// kind -- `MouseModes::set` maintains it under the oracle's rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseEvent {
    #[default]
    None,
    /// Mode 9: press only, left/middle/right only, no modifiers, no release.
    X10,
    /// Mode 1000: press and release.
    Normal,
    /// Mode 1002: press, release, and motion while a button is held.
    Button,
    /// Mode 1003: everything, including motion with no button.
    Any,
}

/// How reports are encoded on the wire. Selected independently of the event kind:
/// a format mode without an event mode reports nothing, and an event mode without a
/// format mode reports in the legacy X10 encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseFormat {
    #[default]
    X10,
    /// Mode 1005: coordinates as UTF-8 codepoints (extends the range past 223).
    Utf8,
    /// Mode 1006: `CSI < b ; x ; y M/m` -- the modern default, unambiguous releases.
    Sgr,
    /// Mode 1015: urxvt's decimal variant.
    Urxvt,
    /// Mode 1016: SGR with pixel coordinates instead of cells.
    SgrPixels,
}

/// The full tracked mouse state: nine raw bits plus the derived pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MouseModes {
    pub(crate) x10: bool,
    pub(crate) normal: bool,
    pub(crate) button: bool,
    pub(crate) any: bool,
    pub(crate) utf8: bool,
    pub(crate) sgr: bool,
    pub(crate) urxvt: bool,
    pub(crate) sgr_pixels: bool,
    /// Mode 1007: wheel events become arrow keys on the alternate screen. The one
    /// tracked mode whose default is ON (`modes.zig` marks it `.default = true`), so
    /// RIS RESTORES it rather than clearing it.
    pub(crate) alternate_scroll: bool,
    pub(crate) event: MouseEvent,
    pub(crate) format: MouseFormat,
}

impl Default for MouseModes {
    fn default() -> Self {
        Self {
            x10: false,
            normal: false,
            button: false,
            any: false,
            utf8: false,
            sgr: false,
            urxvt: false,
            sgr_pixels: false,
            alternate_scroll: true,
            event: MouseEvent::None,
            format: MouseFormat::X10,
        }
    }
}

impl MouseModes {
    /// Applies a DEC private set/reset if `mode` is one of the nine tracked here.
    /// Returns whether it was.
    ///
    /// The raw bit is recorded unconditionally (the oracle's `modes.set` runs before
    /// its special-casing), THEN the derived state: an event mode enables its own kind
    /// and any event-mode disable -- even of a kind that is not current -- drops to
    /// `None`; a format disable likewise falls back to `X10`.
    pub(crate) fn set(&mut self, mode: u16, on: bool) -> bool {
        match mode {
            9 => {
                self.x10 = on;
                self.event = if on { MouseEvent::X10 } else { MouseEvent::None };
            }
            1000 => {
                self.normal = on;
                self.event = if on { MouseEvent::Normal } else { MouseEvent::None };
            }
            1002 => {
                self.button = on;
                self.event = if on { MouseEvent::Button } else { MouseEvent::None };
            }
            1003 => {
                self.any = on;
                self.event = if on { MouseEvent::Any } else { MouseEvent::None };
            }
            1005 => {
                self.utf8 = on;
                self.format = if on { MouseFormat::Utf8 } else { MouseFormat::X10 };
            }
            1006 => {
                self.sgr = on;
                self.format = if on { MouseFormat::Sgr } else { MouseFormat::X10 };
            }
            1015 => {
                self.urxvt = on;
                self.format = if on { MouseFormat::Urxvt } else { MouseFormat::X10 };
            }
            1016 => {
                self.sgr_pixels = on;
                self.format = if on { MouseFormat::SgrPixels } else { MouseFormat::X10 };
            }
            1007 => self.alternate_scroll = on,
            _ => return false,
        }
        true
    }

    /// The raw bit for a tracked mode, for DECRQM. `None` for untracked modes.
    pub(crate) fn raw_bit(&self, mode: u16) -> Option<bool> {
        match mode {
            9 => Some(self.x10),
            1000 => Some(self.normal),
            1002 => Some(self.button),
            1003 => Some(self.any),
            1005 => Some(self.utf8),
            1006 => Some(self.sgr),
            1015 => Some(self.urxvt),
            1016 => Some(self.sgr_pixels),
            1007 => Some(self.alternate_scroll),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule that makes raw bits and the derived kind genuinely two states: after
    /// 1000h 1002h 1002l, the 1000 bit is still set (DECRQM says so) but reporting is
    /// OFF -- disabling any event mode drops the derived kind to None, it does not
    /// "fall back" to a previously enabled one.
    #[test]
    fn disabling_any_event_mode_stops_reporting_without_clearing_other_bits() {
        let mut m = MouseModes::default();
        m.set(1000, true);
        m.set(1002, true);
        m.set(1002, false);
        assert!(m.normal, "raw 1000 bit survives");
        assert!(!m.button);
        assert_eq!(m.event, MouseEvent::None);
    }

    #[test]
    fn event_modes_are_last_writer_wins() {
        let mut m = MouseModes::default();
        m.set(1000, true);
        m.set(1003, true);
        assert_eq!(m.event, MouseEvent::Any);
        assert!(m.normal && m.any, "both raw bits recorded");
    }

    #[test]
    fn disabling_a_format_falls_back_to_x10_not_to_an_earlier_format() {
        let mut m = MouseModes::default();
        m.set(1005, true);
        m.set(1006, true);
        assert_eq!(m.format, MouseFormat::Sgr);
        m.set(1006, false);
        assert_eq!(m.format, MouseFormat::X10, "not Utf8: no fallback stack");
        assert!(m.utf8, "raw 1005 bit still set");
    }

    #[test]
    fn alternate_scroll_defaults_on_and_toggles_without_touching_reporting() {
        let mut m = MouseModes::default();
        assert!(m.alternate_scroll);
        m.set(1007, false);
        assert!(!m.alternate_scroll);
        assert_eq!(m.event, MouseEvent::None);
        assert_eq!(m.format, MouseFormat::X10);
    }

    #[test]
    fn untracked_modes_are_refused() {
        let mut m = MouseModes::default();
        assert!(!m.set(2004, true));
        assert_eq!(m.raw_bit(2004), None);
    }
}
