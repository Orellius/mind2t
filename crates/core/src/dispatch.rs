//! Purpose: turn a parsed control sequence into operations on the terminal state.
//! Public surface: `State::csi` and `State::esc`, called by the `Perform` impl.
//! Why this file: `terminal.rs` answers "what does a character do"; this answers "what does
//!   a control sequence do". They grow at different rates -- the dispatch table gains an arm
//!   per VT feature while the character path barely changes -- and together they broke the
//!   500-line ceiling, which is the signal that they were two responsibilities.
//! NOT responsible for: the printing path, buffer operations (`screen.rs`), or SGR decoding
//!   (`sgr.rs`). It routes; it does not implement.
//! Test strategy: measured against libghostty-vt by the differential corpus.

use ruuah_vt_snapshot::Semantic;
use vte::Params;

use crate::sgr;
use crate::terminal::{Active, State};

/// Which legacy alternate-screen mode is driving the switch. They differ around the edges
/// -- what erases, what saves, and what happens when the screen does not actually change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AltMode {
    M47,
    M1047,
    M1049,
}

impl State {
    fn set_mode(&mut self, mode: u16, on: bool) {
        match mode {
            6 => {
                self.origin = on;
                // DECOM homes the cursor, to the region origin when it is being enabled.
                self.cursor_position(0, 0);
            }
            1 => self.cursor_keys = on,
            7 => self.autowrap = on,
            // The key-encoder trio: keypad application (also ESC = / ESC >), keypad-
            // ignored-under-numlock, and alt-sends-ESC-prefix. The latter two DEFAULT
            // ON in the oracle's table.
            66 => self.keypad_keys = on,
            1035 => self.ignore_keypad_with_numlock = on,
            1036 => self.alt_esc_prefix = on,
            25 => self.cursor_visible = on,
            47 => self.switch_screen(on, AltMode::M47),
            1047 => self.switch_screen(on, AltMode::M1047),
            1048 => {
                if on {
                    self.save_cursor();
                } else {
                    self.restore_cursor();
                }
            }
            1049 => self.switch_screen(on, AltMode::M1049),
            2004 => self.bracketed_paste = on,
            2026 => self.synchronized_output = on,
            // The mouse family (9/1000/1002/1003/1005/1006/1015/1016/1007): raw bit plus
            // derived last-writer state, both maintained inside `MouseModes::set`.
            9 | 1000 | 1002 | 1003 | 1005 | 1006 | 1015 | 1016 | 1007 => {
                self.mouse.set(mode, on);
            }
            _ => {}
        }
    }

    /// Enters or leaves the alternate screen, with the per-mode behaviours around the
    /// switch. Mirrors `switchScreenMode` (`Terminal.zig:4348`), which itself transcribes
    /// xterm's `charproc.c` -- finding 26 measured all three edges this used to get wrong:
    /// 47/1047 never erase on entry and copy the cursor in BOTH directions; a second
    /// `1049h` still saves and still clears, because both sit outside the screen-changed
    /// guard; and `1049l` on the primary is still a DECRC.
    fn switch_screen(&mut self, enabled: bool, mode: AltMode) {
        // Pre-switch behaviours.
        match mode {
            AltMode::M47 => {}
            // Disabling 1047 while on the alternate screen clears it first.
            AltMode::M1047 => {
                if !enabled && self.active == Active::Alternate {
                    self.erase_display_complete();
                }
            }
            // 1049 saves unconditionally on enable, even when already on the alternate.
            AltMode::M1049 => {
                if enabled {
                    self.save_cursor();
                }
            }
        }

        let target = if enabled {
            Active::Alternate
        } else {
            Active::Primary
        };
        let changed = self.active != target;

        // Upstream keeps ONE `Terminal.scrolling_region` for both screens and
        // `switchScreenMode` never touches it (`Terminal.zig:67`); storage here is
        // per-`Screen`, so the region is carried across (finding 19). The OSC 133 state
        // travels with the cursor (finding 20): it rides every cursor copy below, and a
        // 1049 exit restores instead of copying, which is exactly where it must not leak.
        let (scroll_top, scroll_bottom) = (self.screen().scroll_top, self.screen().scroll_bottom);
        let old_cursor = (self.screen().x, self.screen().y);
        let semantic = (
            self.screen().semantic_content,
            self.screen().semantic_clear_at_eol,
        );

        if changed {
            self.active = target;
            self.mark_full_damage();
            self.screen_mut().scroll_top = scroll_top;
            self.screen_mut().scroll_bottom = scroll_bottom;
            self.last_print = None;
        }

        // Post-switch behaviours.
        match mode {
            // The cursor is copied whenever the screen actually changed, regardless of
            // direction (`Terminal.zig:4382`).
            AltMode::M47 | AltMode::M1047 => {
                if changed {
                    self.copy_cursor_across(old_cursor, semantic);
                }
            }
            AltMode::M1049 => {
                if enabled {
                    // Outside the changed guard: a second 1049h re-clears.
                    self.erase_display_complete();
                    if changed {
                        self.copy_cursor_across(old_cursor, semantic);
                    }
                } else {
                    // Outside the changed guard: 1049l on the primary is still a DECRC.
                    self.restore_cursor();
                }
            }
        }
    }

    /// The cursor half of upstream's `cursorCopy`: position and the OSC 133 state that
    /// rides on the cursor. The pen needs no copying because it is terminal-global here.
    fn copy_cursor_across(&mut self, (x, y): (u16, u16), semantic: (Semantic, bool)) {
        let screen = self.screen_mut();
        screen.move_to(x, y);
        screen.semantic_content = semantic.0;
        screen.semantic_clear_at_eol = semantic.1;
        self.last_print = None;
    }

    /// ED 2 as the mode switches perform it: the same complete erase `CSI 2 J` runs,
    /// including the scroll-into-scrollback at a prompt and the whole-frame damage.
    fn erase_display_complete(&mut self) {
        let blank = self.blank();
        self.mark_full_damage();
        self.scroll_clear_at_prompt(blank);
        self.screen_mut().erase_in_display(2, blank);
    }

    /// The kitty keyboard CSI family (`stream.zig`'s 'u' intermediate dispatch):
    /// `?` queries, `>` pushes, `<` pops, `=` combines. A flags value past the five
    /// defined bits invalidates the WHOLE command (the oracle warns and drops it,
    /// no clamping); a push with any param count other than exactly one pushes 0.
    fn kitty_keyboard_csi(&mut self, params: &Params, intermediates: &[u8]) {
        use crate::kitty_keys::{KITTY_ALL, SetMode};
        let count = params.len();
        match intermediates.first() {
            Some(&b'?') => self.kitty_keyboard_report(),
            Some(&b'>') => {
                let flags = if count == 1 { arg_or_zero(params, 0) } else { 0 };
                if flags > u16::from(KITTY_ALL) {
                    return;
                }
                self.screen_mut().kitty_keyboard.push(flags as u8);
            }
            Some(&b'<') => {
                // vte reports an absent parameter list as one zero-valued group, so
                // `CSI < u` and `CSI < 0 u` are indistinguishable here; the VT
                // missing-or-zero-means-one rule resolves both to a pop of one,
                // which matches the oracle on the only spelling programs send.
                self.screen_mut().kitty_keyboard.pop(usize::from(arg(params, 0)));
            }
            Some(&b'=') => {
                let flags = if count >= 1 { arg_or_zero(params, 0) } else { 0 };
                if flags > u16::from(KITTY_ALL) {
                    return;
                }
                let mode = match if count >= 2 { arg_or_zero(params, 1) } else { 1 } {
                    1 => SetMode::Set,
                    2 => SetMode::Or,
                    3 => SetMode::Not,
                    _ => return,
                };
                self.screen_mut().kitty_keyboard.set(mode, flags as u8);
            }
            _ => {}
        }
    }

    /// xterm modifyOtherKeys (`CSI > Pp ; Pv m`). Only `>4;2m` turns the numeric
    /// form ON; every other VALID form -- including bare `CSI > m` -- resets it,
    /// and an invalid one changes nothing (the oracle warns and drops, no reset).
    fn modify_key_format(&mut self, params: &Params) {
        let count = params.len();
        if count == 0 {
            self.modify_other_keys_2 = false;
            return;
        }
        if count > 2 || !matches!(arg_or_zero(params, 0), 0 | 1 | 2 | 4) {
            return;
        }
        self.modify_other_keys_2 =
            arg_or_zero(params, 0) == 4 && count == 2 && arg_or_zero(params, 1) == 2;
    }

    pub(crate) fn csi(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        if ignore {
            return;
        }

        // DECRQM (CSI Pm $ p / CSI ? Pd $ p) must precede the private-mode branch: the
        // DEC form's intermediates START with `?`, and the h/l matcher below would
        // otherwise swallow it silently.
        if action == 'p' && (intermediates == [b'$'] || intermediates == [b'?', b'$']) {
            let ansi = intermediates.len() == 1;
            return self.mode_report(arg_or_zero(params, 0), ansi);
        }

        // The kitty keyboard family (CSI ? u / > u / < u / = u) must also precede the
        // private-mode branch: the query's intermediates start with `?` and action `u`
        // would fall into its silent `_ => return`. Bare `CSI u` (SCORC) is not handled
        // here -- it falls through to the intermediate-free match below, the oracle's
        // own split (`stream.zig` dispatches on intermediates.len for 'u').
        if action == 'u' && !intermediates.is_empty() {
            return self.kitty_keyboard_csi(params, intermediates);
        }

        // xterm modifyOtherKeys (CSI > Pp ; Pv m). Before the private-mode branch for
        // symmetry, though only the `>` marker reaches it.
        if action == 'm' && intermediates.first() == Some(&b'>') {
            return self.modify_key_format(params);
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

        // DA2 (CSI > c) and DA3 (CSI = c): the private markers arrive as intermediates.
        if action == 'c' {
            match intermediates.first() {
                Some(&b'>') => return self.device_attributes_secondary(),
                Some(&b'=') => return self.device_attributes_tertiary(),
                _ => {}
            }
        }

        // DECSTR (CSI ! p): the soft reset esctest2 runs before every test.
        if action == 'p' && intermediates.first() == Some(&b'!') {
            return self.soft_reset();
        }

        // DECRQCRA (CSI Pid;Pp;Pt;Pl;Pb;Pr * y): the checksum readback esctest2's screen
        // assertions run on. Pp (the page) is ignored -- there is one page here.
        if action == 'y' && intermediates.first() == Some(&b'*') {
            let id = arg_or_zero(params, 0);
            let rect = [
                arg_or_zero(params, 2),
                arg_or_zero(params, 3),
                arg_or_zero(params, 4),
                arg_or_zero(params, 5),
            ];
            return self.checksum_report(id, rect);
        }

        // Other intermediates carry sequences this slice does not implement. Acting on them
        // half-way would be worse than not acting.
        if !intermediates.is_empty() {
            return;
        }

        let blank = self.blank();
        match action {
            'm' => sgr::apply(&mut self.pen, params),

            'n' => self.device_status_report(arg_or_zero(params, 0)),
            't' => self.window_report(arg_or_zero(params, 0)),
            'c' => {
                if arg_or_zero(params, 0) == 0 {
                    self.device_attributes_primary();
                }
            }

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
                    self.scroll_clear_at_prompt(blank);
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
            // DECKPAM / DECKPNM route through the same mode-66 state as CSI ?66h/l --
            // the oracle's ESC dispatch emits set_mode/reset_mode for them, so
            // mode_get(66) observes both spellings identically.
            b'=' => self.keypad_keys = true,
            b'>' => self.keypad_keys = false,
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

/// Like `arg`, but a missing parameter is 0, not 1 -- DSR and DA distinguish the two
/// (`CSI c` and `CSI 0 c` are both primary DA; `CSI n` is nothing).
fn arg_or_zero(params: &Params, index: usize) -> u16 {
    params
        .iter()
        .nth(index)
        .and_then(|values| values.first().copied())
        .unwrap_or(0)
}

/// Reads a CSI parameter whose default is 0 rather than 1, as the erase and TBC selectors are.
fn zero_arg(params: &Params, index: usize) -> u16 {
    params
        .iter()
        .nth(index)
        .and_then(|values| values.first().copied())
        .unwrap_or(0)
}
