//! Purpose: turn a parsed control sequence into operations on the terminal state.
//! Public surface: `State::csi` and `State::esc`, called by the `Perform` impl.
//! Why this file: `terminal.rs` answers "what does a character do"; this answers "what does
//!   a control sequence do". They grow at different rates -- the dispatch table gains an arm
//!   per VT feature while the character path barely changes -- and together they broke the
//!   500-line ceiling, which is the signal that they were two responsibilities.
//! NOT responsible for: the printing path, buffer operations (`screen.rs`), or SGR decoding
//!   (`sgr.rs`). It routes; it does not implement.
//! Test strategy: measured against libghostty-vt by the differential corpus.

use vte::Params;

use crate::sgr;
use crate::terminal::{Active, State};

impl State {
    fn set_mode(&mut self, mode: u16, on: bool) {
        match mode {
            6 => {
                self.origin = on;
                // DECOM homes the cursor, to the region origin when it is being enabled.
                self.cursor_position(0, 0);
            }
            7 => self.autowrap = on,
            25 => self.cursor_visible = on,
            47 | 1047 => self.switch_screen(on, false),
            1048 => {
                if on {
                    self.save_cursor();
                } else {
                    self.restore_cursor();
                }
            }
            1049 => self.switch_screen(on, true),
            _ => {}
        }
    }

    /// Enters or leaves the alternate screen. `save` is the 1049 behaviour: the cursor is
    /// preserved across the switch and the alternate buffer is cleared on entry.
    fn switch_screen(&mut self, to_alternate: bool, save: bool) {
        let target = if to_alternate {
            Active::Alternate
        } else {
            Active::Primary
        };
        if self.active == target {
            return;
        }

        if to_alternate {
            if save {
                self.save_cursor();
            }
            // Where the cursor sits before the switch, read while the primary is still
            // active. Clearing the alternate buffer does NOT home the cursor: measured
            // against libghostty-vt 2026-07-28, entering the alternate screen leaves it
            // exactly where it was. `reset` homes it, so it is put back afterwards.
            let (x, y) = (self.screen().x, self.screen().y);
            self.active = Active::Alternate;
            self.mark_full_damage();
            let blank = self.blank();
            self.alternate.reset(blank);
            self.alternate.x = x.min(self.alternate.cols().saturating_sub(1));
            self.alternate.y = y.min(self.alternate.rows().saturating_sub(1));
        } else {
            self.active = Active::Primary;
            self.mark_full_damage();
            if save {
                self.restore_cursor();
            }
        }
        self.last_print = None;
    }

    pub(crate) fn csi(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        if ignore {
            return;
        }

        if intermediates.first() == Some(&b'?') {
            let on = match action {
                'h' => true,
                'l' => false,
                _ => return,
            };
            for item in params.iter() {
                if let Some(&mode) = item.first() {
                    self.set_mode(mode, on);
                }
            }
            return;
        }

        // Other intermediates carry sequences this slice does not implement. Acting on them
        // half-way would be worse than not acting.
        if !intermediates.is_empty() {
            return;
        }

        let blank = self.blank();
        match action {
            'm' => sgr::apply(&mut self.pen, params),

            'A' => self.cursor_up(arg(params, 0)),
            'B' => self.cursor_down(arg(params, 0)),
            'C' => {
                let (x, y) = (self.cursor_x().saturating_add(arg(params, 0)), self.cursor_y());
                self.goto(x, y);
            }
            'D' => {
                let (x, y) = (self.cursor_x().saturating_sub(arg(params, 0)), self.cursor_y());
                self.goto(x, y);
            }
            // CNL and CPL are a clamped vertical move followed by a carriage return
            // (stream.zig:1224 and :1247), so they inherit the scroll-region bound.
            'E' => {
                self.cursor_down(arg(params, 0));
                let y = self.cursor_y();
                self.goto(0, y);
            }
            'F' => {
                self.cursor_up(arg(params, 0));
                let y = self.cursor_y();
                self.goto(0, y);
            }
            'G' | '`' => {
                let y = self.cursor_y();
                self.goto(arg(params, 0) - 1, y);
            }
            'd' => {
                let x = self.cursor_x();
                self.cursor_position(arg(params, 0) - 1, x);
            }
            'H' | 'f' => self.cursor_position(arg(params, 0) - 1, arg(params, 1) - 1),

            'I' => self.tab_forward(arg(params, 0)),
            'Z' => self.tab_backward(arg(params, 0)),
            'g' => {
                let x = self.cursor_x();
                match zero_arg(params, 0) {
                    0 => self.tabs.clear(x),
                    3 => self.tabs.clear_all(),
                    _ => {}
                }
            }

            'J' => {
                let mode = zero_arg(params, 0);
                // Only a COMPLETE erase is a whole-frame event. ED 0 and ED 1 leave content
                // behind, so their per-row damage is real information; measured 2026-07-28.
                if mode == 2 {
                    self.mark_full_damage();
                }
                self.screen_mut().erase_in_display(mode, blank);
            }
            'K' => self.screen_mut().erase_in_line(zero_arg(params, 0), blank),
            'X' => self.screen_mut().erase_chars(arg(params, 0), blank),
            '@' => self.screen_mut().insert_chars(arg(params, 0), blank),
            'P' => self.screen_mut().delete_chars(arg(params, 0), blank),
            'L' => self.screen_mut().insert_lines(arg(params, 0), blank),
            'M' => self.screen_mut().delete_lines(arg(params, 0), blank),
            'S' => self.screen_mut().scroll_up(arg(params, 0), blank),
            'T' => self.screen_mut().scroll_down(arg(params, 0), blank),

            'r' => {
                let top = arg(params, 0) - 1;
                let bottom = if params.len() > 1 {
                    arg(params, 1) - 1
                } else {
                    self.screen().rows().saturating_sub(1)
                };
                if self.screen_mut().set_scroll_region(top, bottom) {
                    // DECSTBM homes the cursor, to the region origin under DECOM.
                    self.cursor_position(0, 0);
                }
            }
            's' => self.save_cursor(),
            'u' => self.restore_cursor(),
            _ => {}
        }
    }

    pub(crate) fn esc(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        if ignore || !intermediates.is_empty() {
            return;
        }
        let blank = self.blank();
        match byte {
            b'H' => {
                let x = self.cursor_x();
                self.tabs.set(x);
            }
            b'M' => {
                self.screen_mut().reverse_index(blank);
                self.last_print = None;
            }
            b'D' => {
                self.index(blank);
                self.last_print = None;
            }
            b'E' => {
                self.index(blank);
                self.screen_mut().x = 0;
                self.last_print = None;
            }
            b'7' => self.save_cursor(),
            b'8' => self.restore_cursor(),
            b'c' => self.full_reset(),
            _ => {}
        }
    }
}

/// Reads a CSI parameter, applying the VT rule that a missing or zero parameter means 1.
///
/// Never returning 0 is what makes `arg(..) - 1` safe at every call site above.
fn arg(params: &Params, index: usize) -> u16 {
    params
        .iter()
        .nth(index)
        .and_then(|values| values.first().copied())
        .filter(|value| *value != 0)
        .unwrap_or(1)
}

/// Reads a CSI parameter whose default is 0 rather than 1, as the erase and TBC selectors are.
fn zero_arg(params: &Params, index: usize) -> u16 {
    params
        .iter()
        .nth(index)
        .and_then(|values| values.first().copied())
        .unwrap_or(0)
}
