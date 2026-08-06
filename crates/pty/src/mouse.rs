//! Purpose: turn a pointer event into the bytes a mouse report writes to the pty.
//! Public surface: `Options`, `Size`, `Event`, `Action`, `Button`, `Mods`, `encode`.
//! Why this file: a mouse report is an INPUT encoding, exactly like paste -- the core
//!   TRACKS the modes and encodes nothing, so the transform lives with the pty. It is
//!   pure so the differential harness can compare it byte-for-byte against the
//!   oracle's own `ghostty_mouse_encoder_encode`.
//! NOT responsible for: deciding which modes are active (the core tracks them; the
//!   host reads them off the frame), wheel routing (host policy), or writing to the
//!   pty (`host.rs`).
//! Test strategy: `crates/ghostty/tests/mouse.rs` drives the oracle's encoder ABI
//!   over a matrix of events, formats, geometries and edge positions and demands byte
//!   equality; unit tests here pin the rules that motivated each branch, all measured
//!   from `src/input/mouse_encode.zig` (v1.3.2) on 2026-07-30.

use ruuah_vt_core::mouse::{MouseEvent, MouseFormat};

/// The rendered geometry that turns surface-space pixels into cells. Mirrors the
/// oracle's `renderer_size.Size`: the grid is derived from the padded screen, so the
/// encoder clamps clicks in the padding onto the edge cells rather than reporting
/// coordinates the child never heard of.
#[derive(Debug, Clone, Copy)]
pub struct Size {
    pub screen_width: u32,
    pub screen_height: u32,
    /// Must be non-zero; a zero cell would divide by zero exactly like the oracle's
    /// `@intFromFloat` would trap.
    pub cell_width: u32,
    pub cell_height: u32,
    pub padding_left: u32,
    pub padding_top: u32,
    pub padding_right: u32,
    pub padding_bottom: u32,
}

impl Size {
    /// Grid dimensions the oracle way: padding subtracted, floor-divided, floored at 1
    /// (`GridSize.update`).
    fn grid(&self) -> (u32, u32) {
        let usable_w = self.screen_width.saturating_sub(self.padding_left + self.padding_right);
        let usable_h = self.screen_height.saturating_sub(self.padding_top + self.padding_bottom);
        (
            (usable_w / self.cell_width).max(1),
            (usable_h / self.cell_height).max(1),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Press,
    Release,
    Motion,
}

/// Buttons the protocol can name. `Six`..`Nine` exist because the encoding has room
/// for them (64..67, 128..129); anything else produces no report, matching the
/// oracle's `else => return null`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
    Four,
    Five,
    Six,
    Seven,
    Eight,
    Nine,
    Other,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// A normalized pointer event in surface-space pixels. Negative positions and
/// positions past the screen are allowed; the encoder decides what they mean.
#[derive(Debug, Clone, Copy)]
pub struct Event {
    pub action: Action,
    /// `None` is motion with nothing held.
    pub button: Option<Button>,
    pub mods: Mods,
    pub x: f32,
    pub y: f32,
}

/// Everything outside the event that shapes the encoding.
#[derive(Debug)]
pub struct Options<'a> {
    pub event_mode: MouseEvent,
    pub format: MouseFormat,
    pub size: Size,
    /// Whether any button is held, INCLUDING the one in the current event. Out-of-
    /// viewport motion is reported only while something is held (drag-out support);
    /// without a button it is dropped.
    pub any_button_pressed: bool,
    /// Motion deduplication state: `Some` tracks the last reported cell across calls
    /// and swallows repeat motion inside one cell (except SGR-Pixels, which reports
    /// raw pixels). `None` disables tracking.
    pub last_cell: Option<&'a mut Option<(u32, u32)>>,
}

/// Encodes one pointer event. `None` means this event produces no bytes -- most
/// events do not, and silence is a correct, common outcome, not an error.
pub fn encode(event: Event, opts: Options<'_>) -> Option<Vec<u8>> {
    if !should_report(&event, opts.event_mode) {
        return None;
    }

    // Out-of-viewport positions: releases always report (the child needs to learn the
    // drag ended); anything else needs a motion-capable mode AND a held button.
    if event.action != Action::Release && out_of_viewport(&event, &opts.size) {
        let sends_motion =
            matches!(opts.event_mode, MouseEvent::Button | MouseEvent::Any);
        if !sends_motion || !opts.any_button_pressed {
            return None;
        }
    }

    let cell = pos_to_cell(event.x, event.y, &opts.size);

    // Repeat motion inside one cell is noise to a cell-addressed child; SGR-Pixels is
    // exempt because its unit is the pixel, not the cell.
    let mut last_cell = opts.last_cell;
    if event.action == Action::Motion
        && opts.format != MouseFormat::SgrPixels
        && let Some(&mut Some(last)) = last_cell.as_deref_mut()
        && last == cell
    {
        return None;
    }
    // The oracle updates its tracked cell BEFORE the button lookup, so an unknown
    // button still refreshes the dedup state. Mirrored for byte-for-byte parity
    // across sequences, not because the order is otherwise observable.
    if let Some(last) = last_cell {
        *last = Some(cell);
    }

    let code = button_code(&event, opts.event_mode, opts.format)?;
    let mut out = Vec::with_capacity(24);
    match opts.format {
        MouseFormat::X10 => {
            // The legacy encoding is bytes 32+n: coordinates past 222 would wrap the
            // byte, so the oracle refuses rather than aliasing another cell.
            if cell.0 > 222 || cell.1 > 222 {
                return None;
            }
            out.extend_from_slice(b"\x1b[M");
            out.push(32 + code);
            out.push(32 + cell.0 as u8 + 1);
            out.push(32 + cell.1 as u8 + 1);
        }
        MouseFormat::Utf8 => {
            out.extend_from_slice(b"\x1b[M");
            out.push(32 + code);
            let mut buf = [0u8; 4];
            for coord in [cell.0 + 33, cell.1 + 33] {
                // In-range for char by construction: a grid coordinate is bounded by
                // the screen dimensions, far under the surrogate range.
                let cp = char::from_u32(coord).unwrap_or('\u{FFFD}');
                out.extend_from_slice(cp.encode_utf8(&mut buf).as_bytes());
            }
        }
        MouseFormat::Sgr => {
            let terminator = if event.action == Action::Release { 'm' } else { 'M' };
            out.extend_from_slice(
                format!("\x1b[<{};{};{}{}", code, cell.0 + 1, cell.1 + 1, terminator).as_bytes(),
            );
        }
        MouseFormat::Urxvt => {
            out.extend_from_slice(
                format!("\x1b[{};{};{}M", 32 + u32::from(code), cell.0 + 1, cell.1 + 1).as_bytes(),
            );
        }
        MouseFormat::SgrPixels => {
            let (px, py) = pos_to_pixels(event.x, event.y, &opts.size);
            let terminator = if event.action == Action::Release { 'm' } else { 'M' };
            out.extend_from_slice(
                format!("\x1b[<{code};{px};{py}{terminator}").as_bytes(),
            );
        }
    }
    Some(out)
}

/// Which events the active mode reports at all (`shouldReport`).
fn should_report(event: &Event, mode: MouseEvent) -> bool {
    match mode {
        MouseEvent::None => false,
        // X10 is presses of the three classic buttons, nothing else.
        MouseEvent::X10 => {
            event.action == Action::Press
                && matches!(event.button, Some(Button::Left | Button::Middle | Button::Right))
        }
        MouseEvent::Normal => event.action != Action::Motion,
        MouseEvent::Button => event.button.is_some(),
        MouseEvent::Any => true,
    }
}

/// The protocol button code plus modifier and motion bits (`buttonCode`). `None` is
/// a button the protocol has no number for.
fn button_code(event: &Event, mode: MouseEvent, format: MouseFormat) -> Option<u8> {
    let mut acc: u8 = match event.button {
        // Motion with nothing held.
        None => 3,
        Some(button) => {
            // Legacy formats cannot say WHICH button lifted, so every release is 3;
            // SGR grew the lowercase terminator precisely to keep the button.
            if event.action == Action::Release
                && format != MouseFormat::Sgr
                && format != MouseFormat::SgrPixels
            {
                3
            } else {
                match button {
                    Button::Left => 0,
                    Button::Middle => 1,
                    Button::Right => 2,
                    Button::Four => 64,
                    Button::Five => 65,
                    Button::Six => 66,
                    Button::Seven => 67,
                    Button::Eight => 128,
                    Button::Nine => 129,
                    Button::Other => return None,
                }
            }
        }
    };

    // X10 EVENT mode (not the format of the same name) predates modifiers.
    if mode != MouseEvent::X10 {
        if event.mods.shift {
            acc += 4;
        }
        if event.mods.alt {
            acc += 8;
        }
        if event.mods.ctrl {
            acc += 16;
        }
    }
    if event.action == Action::Motion {
        acc += 32;
    }
    Some(acc)
}

/// Strictly outside the screen. The boundary itself (`pos == width`) is IN, matching
/// the oracle's `>` comparison.
fn out_of_viewport(event: &Event, size: &Size) -> bool {
    event.x < 0.0
        || event.y < 0.0
        || event.x > size.screen_width as f32
        || event.y > size.screen_height as f32
}

/// Surface pixels to a zero-based cell, the oracle's exact arithmetic: padding off,
/// clamp at zero, floor-divide, clamp to the (padding-derived) grid edge.
///
/// Public because SELECTION needs the same answer. A host that computed its own
/// pixel-to-cell would put the highlight on one cell while the child was told the
/// pointer was on another, and the two would agree everywhere except the padding and
/// the last column -- which is precisely where a person notices.
pub fn pos_to_cell(x: f32, y: f32, size: &Size) -> (u32, u32) {
    let term_x = f64::from(x) - f64::from(size.padding_left);
    let term_y = f64::from(y) - f64::from(size.padding_top);
    let (cols, rows) = size.grid();
    let col = (term_x.max(0.0) / f64::from(size.cell_width)) as u32;
    let row = (term_y.max(0.0) / f64::from(size.cell_height)) as u32;
    (col.min(cols - 1), row.min(rows - 1))
}

/// Surface pixels to terminal-space pixels for SGR-Pixels: padding off, ROUNDED, and
/// deliberately unclamped -- negatives and overshoot are the protocol's to carry.
fn pos_to_pixels(x: f32, y: f32, size: &Size) -> (i32, i32) {
    let term_x = f64::from(x) - f64::from(size.padding_left);
    let term_y = f64::from(y) - f64::from(size.padding_top);
    (term_x.round() as i32, term_y.round() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size() -> Size {
        Size {
            screen_width: 1000,
            screen_height: 1000,
            cell_width: 10,
            cell_height: 20,
            padding_left: 8,
            padding_top: 8,
            padding_right: 8,
            padding_bottom: 8,
        }
    }

    fn press(x: f32, y: f32) -> Event {
        Event { action: Action::Press, button: Some(Button::Left), mods: Mods::default(), x, y }
    }

    fn opts(mode: MouseEvent, format: MouseFormat) -> Options<'static> {
        Options { event_mode: mode, format, size: size(), any_button_pressed: false, last_cell: None }
    }

    #[test]
    fn sgr_press_and_release_disagree_only_in_the_terminator_and_keep_the_button() {
        let p = encode(press(85.0, 50.0), opts(MouseEvent::Normal, MouseFormat::Sgr)).unwrap();
        // (85-8)/10 = col 7, (50-8)/20 = row 2, 1-based -> 8;3.
        assert_eq!(p, b"\x1b[<0;8;3M");
        let mut e = press(85.0, 50.0);
        e.action = Action::Release;
        let r = encode(e, opts(MouseEvent::Normal, MouseFormat::Sgr)).unwrap();
        assert_eq!(r, b"\x1b[<0;8;3m", "SGR keeps button 0 on release; legacy would say 3");
    }

    #[test]
    fn legacy_release_is_button_three_because_the_wire_cannot_name_it() {
        let mut e = press(85.0, 50.0);
        e.action = Action::Release;
        let out = encode(e, opts(MouseEvent::Normal, MouseFormat::X10)).unwrap();
        assert_eq!(out, &[0x1b, b'[', b'M', 32 + 3, 32 + 8, 32 + 3]);
    }

    #[test]
    fn x10_event_mode_strips_modifiers_and_everything_else_adds_them() {
        let mut e = press(8.0, 8.0);
        e.mods = Mods { shift: true, alt: true, ctrl: true };
        let x10 = encode(e, opts(MouseEvent::X10, MouseFormat::X10)).unwrap();
        assert_eq!(x10[3], 32, "no modifier bits under X10 event mode");
        let normal = encode(e, opts(MouseEvent::Normal, MouseFormat::X10)).unwrap();
        assert_eq!(normal[3], 32 + 4 + 8 + 16);
    }

    #[test]
    fn motion_in_the_same_cell_is_deduplicated_until_the_cell_changes() {
        let mut last = None;
        let motion = |x: f32, last: &mut Option<(u32, u32)>| {
            let e = Event {
                action: Action::Motion,
                button: Some(Button::Left),
                mods: Mods::default(),
                x,
                y: 50.0,
            };
            encode(
                e,
                Options {
                    event_mode: MouseEvent::Button,
                    format: MouseFormat::Sgr,
                    size: size(),
                    any_button_pressed: true,
                    last_cell: Some(last),
                },
            )
        };
        assert!(motion(85.0, &mut last).is_some(), "first report in the cell");
        assert!(motion(86.0, &mut last).is_none(), "same cell, swallowed");
        assert!(motion(95.0, &mut last).is_some(), "next cell reports again");
    }

    #[test]
    fn out_of_viewport_motion_reports_only_while_dragging() {
        let e = Event {
            action: Action::Motion,
            button: Some(Button::Left),
            mods: Mods::default(),
            x: -5.0,
            y: 50.0,
        };
        let mut o = opts(MouseEvent::Button, MouseFormat::Sgr);
        o.any_button_pressed = false;
        assert!(encode(e, o).is_none(), "no button held: dropped");
        let mut o = opts(MouseEvent::Button, MouseFormat::Sgr);
        o.any_button_pressed = true;
        let out = encode(e, o).unwrap();
        assert_eq!(out, b"\x1b[<32;1;3M", "clamped to column 1 while dragging out");
    }

    #[test]
    fn x10_format_refuses_coordinates_past_222_instead_of_aliasing() {
        let mut s = size();
        s.screen_width = 3000;
        s.cell_width = 10;
        let e = press(2500.0, 50.0); // col ~249
        let out = encode(
            e,
            Options {
                event_mode: MouseEvent::Normal,
                format: MouseFormat::X10,
                size: s,
                any_button_pressed: false,
                last_cell: None,
            },
        );
        assert!(out.is_none());
    }

    #[test]
    fn wheel_buttons_encode_in_the_64_range() {
        let mut e = press(8.0, 8.0);
        e.button = Some(Button::Four);
        let out = encode(e, opts(MouseEvent::Normal, MouseFormat::Sgr)).unwrap();
        assert_eq!(out, b"\x1b[<64;1;1M");
    }

    #[test]
    fn sgr_pixels_reports_rounded_unclamped_terminal_pixels() {
        let mut e = press(4.6, 50.0); // left of the padding: -3.4 -> -3
        e.action = Action::Motion;
        let mut o = opts(MouseEvent::Any, MouseFormat::SgrPixels);
        o.any_button_pressed = true;
        let out = encode(e, o).unwrap();
        assert_eq!(out, b"\x1b[<32;-3;42M");
    }
}
