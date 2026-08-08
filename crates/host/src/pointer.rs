//! Purpose: the host side of mouse reporting - geometry, held buttons, motion dedup, and the
//!   routing decision a wheel needs.
//! Public surface (crate): `Pointer`, `Wheel`.
//! Why this file: the ENCODER is pure and lives in `mind2t_vt_pty::mouse`, measured byte for byte
//!   against the oracle. What it cannot own is the state around it - which buttons the operator
//!   is holding, which cell was last reported, how big the view is - and that state has to live
//!   in the host. It lived in the C ABI only, so a Rust host wanting mouse support had two
//!   choices: reach through a foreign-function boundary into its own workspace, or write the
//!   policy a second time. A second policy is the worse one: the Swift host is the ORACLE for
//!   the Tauri port, and two implementations make "do the two hosts agree?" a question about
//!   the hosts rather than about the port.
//! NOT responsible for: the encoding (the pty crate), reading events (each host's window layer),
//!   or scrolling the viewport - `Wheel::Viewport` hands that decision back to the caller
//!   precisely because it is host policy, not protocol.
//! Test strategy: the five end-to-end mouse tests in `tests/host_abi.rs` drive real children
//!   through the C surface and were written before this file existed; they are what proves the
//!   extraction changed nothing. `tests/session.rs` proves the same paths through the Rust
//!   surface, so both callers are covered by tests that share no code.

use mind2t_vt_frame::Frame;
use mind2t_vt_pty::mouse::{Action, Button, Event, Mods, Options, Size};
use mind2t_vt_render::CellMetrics;

/// What a wheel tick should become. The caller decides nothing except how to act on it.
#[derive(Debug, PartialEq, Eq)]
pub enum Wheel {
    /// Write these bytes: either a mouse report, or the arrow keys alternate scroll turns a
    /// wheel into on the alternate screen.
    Send(Vec<u8>),
    /// The child asked for neither, so the wheel belongs to the host: scroll the viewport.
    Viewport,
}

/// One pointer event as a host reports it.
///
/// A struct rather than five parameters: `code` and the two coordinates are all numbers, and a
/// call site that transposed x and y - or passed a button where a code was wanted - would compile
/// and report the wrong cell forever.
#[derive(Debug, Clone, Copy)]
pub struct Input {
    pub action: Action,
    /// The protocol's button number: 0 motion with nothing held, 1 left, 2 middle, 3 right,
    /// 4..9 wheel and aux.
    pub code: u32,
    pub mods: Mods,
    /// Surface-space PHYSICAL pixels from the view's top-left.
    pub x: f32,
    pub y: f32,
}

/// The protocol's button number to the encoder's name. `0` is motion with nothing held.
///
/// Everything past the named codes is `Other`: a real hardware button the protocol cannot name
/// still takes part in the held-button bookkeeping, and the encoder answers silence for it.
pub fn button_from_code(code: u32) -> Option<Button> {
    match code {
        0 => None,
        1 => Some(Button::Left),
        2 => Some(Button::Middle),
        3 => Some(Button::Right),
        4 => Some(Button::Four),
        5 => Some(Button::Five),
        6 => Some(Button::Six),
        7 => Some(Button::Seven),
        8 => Some(Button::Eight),
        9 => Some(Button::Nine),
        _ => Some(Button::Other),
    }
}

/// The mouse state a host must carry between events.
///
/// Geometry is SET by the host rather than derived here, because only the host knows its view's
/// pixel size and the insets around the grid (Mind2t reserves a chrome strip; the Swift host
/// has its own chrome). Cell metrics are asked of the renderer at encode time instead of being
/// stored, because zoom rebuilds them.
#[derive(Debug, Default)]
pub struct Pointer {
    screen_width: u32,
    screen_height: u32,
    padding_left: u32,
    padding_top: u32,
    padding_right: u32,
    padding_bottom: u32,
    /// Buttons currently down, bit N for button code N. Updated BEFORE encoding (the oracle
    /// records click state first), so a release's own button is already clear and
    /// `any_button_pressed` reflects what else is held.
    buttons_held: u16,
    /// Last reported cell, the encoder's cross-call motion-dedup state.
    last_cell: Option<(u32, u32)>,
}

impl Pointer {
    /// The view the encoder converts through: surface size and content insets, in the same
    /// backing-pixel space the frame's pixels use. Until this is called, every pointer event
    /// encodes to nothing - which is correct, not a failure: a report needs a grid position and
    /// there is no grid to position against.
    pub fn set_geometry(
        &mut self,
        screen_width: u32,
        screen_height: u32,
        padding_left: u32,
        padding_top: u32,
        padding_right: u32,
        padding_bottom: u32,
    ) {
        self.screen_width = screen_width;
        self.screen_height = screen_height;
        self.padding_left = padding_left;
        self.padding_top = padding_top;
        self.padding_right = padding_right;
        self.padding_bottom = padding_bottom;
    }

    pub fn has_geometry(&self) -> bool {
        self.screen_width != 0 && self.screen_height != 0
    }

    /// Which viewport cell a surface pixel lands on, or `None` before geometry is set.
    ///
    /// Shares `pos_to_cell` with the report encoder rather than repeating its arithmetic, so
    /// the cell the operator sees highlighted and the cell the child is told about are the same
    /// cell by construction. They would otherwise agree everywhere except inside the padding
    /// and at the last column, which is exactly where a person notices.
    pub fn cell_at(&self, cell: CellMetrics, x: f32, y: f32) -> Option<(u16, u16)> {
        let size = self.size(cell)?;
        let (col, row) = mind2t_vt_pty::mouse::pos_to_cell(x, y, &size);
        Some((col.min(u32::from(u16::MAX)) as u16, row.min(u32::from(u16::MAX)) as u16))
    }

    fn size(&self, cell: CellMetrics) -> Option<Size> {
        if !self.has_geometry() {
            return None;
        }
        Some(Size {
            screen_width: self.screen_width,
            screen_height: self.screen_height,
            cell_width: cell.width,
            cell_height: cell.height,
            padding_left: self.padding_left,
            padding_top: self.padding_top,
            padding_right: self.padding_right,
            padding_bottom: self.padding_bottom,
        })
    }

    /// Records the button and encodes the report, or `None` when this event produces nothing.
    ///
    /// `code` is the protocol's button number - 0 for motion with nothing held, 1 left, 2
    /// middle, 3 right, 4..9 the wheel and aux buttons - and the bookkeeping happens for every
    /// call, reporting on or off. That is not an optimisation: a press that arrives while
    /// reporting is off and a release that arrives after it was turned on must still leave the
    /// held set correct, or the next drag reports a button nobody is holding.
    pub fn button(&mut self, frame: &Frame, cell: CellMetrics, input: Input) -> Option<Vec<u8>> {
        let Input { action, code, mods, x, y } = input;
        let button = button_from_code(code);
        if code > 0 {
            let bit = 1u16 << (code.min(15) as u16);
            match action {
                Action::Press => self.buttons_held |= bit,
                Action::Release => self.buttons_held &= !bit,
                Action::Motion => {}
            }
        }

        let size = self.size(cell)?;
        mind2t_vt_pty::mouse::encode(
            Event { action, button, mods, x, y },
            Options {
                event_mode: frame.mouse_event(),
                format: frame.mouse_format(),
                size,
                any_button_pressed: self.buttons_held != 0,
                last_cell: Some(&mut self.last_cell),
            },
        )
    }

    /// Routes a wheel tick: a report if the child is watching the mouse, arrow keys if it is on
    /// the alternate screen with alternate scroll, and otherwise back to the host.
    ///
    /// The order is the whole rule. A program that captured the mouse must NOT also have the
    /// view scrolled under it, and alternate scroll exists so that a pager which never asked
    /// for mouse reporting still moves under a wheel - so reporting is checked first and the
    /// viewport is the last resort, never a parallel one.
    pub fn wheel(
        &mut self,
        frame: &Frame,
        cell: CellMetrics,
        x: f32,
        y: f32,
        ticks: i32,
        mods: Mods,
    ) -> Wheel {
        if ticks == 0 {
            return Wheel::Viewport;
        }

        // SHIFT TAKES THE WHEEL BACK FROM THE CHILD, and without it the scrollback is
        // unreachable from inside any program that captured the mouse.
        //
        // Reported live 2026-08-09: "you cannot scroll with the mouse scroll nor jump to bottom",
        // from inside Claude Code. The routing below was correct and complete - reporting on
        // means the wheel is the child's - and that is exactly the problem: `claude`, `vim`,
        // `htop` and every other full-screen program turn reporting on, so every tick belonged
        // to them and the view could never move. Nothing was broken; something was missing.
        //
        // The CLICK path has had this escape hatch since D2b (`selects_instead`, shift takes the
        // pointer back so a line can be copied out of a program that owns the mouse). The wheel
        // never got it. Same reasoning, same modifier, and it is what every terminal does.
        //
        // Checked BEFORE the reporting branch on purpose: an escape hatch that only works when
        // the thing it escapes is absent is not an escape hatch.
        if mods.shift {
            return Wheel::Viewport;
        }

        // A tick count is a gesture, not a promise: a flick can deliver an enormous number and
        // every one of them would become bytes on the pty.
        let repeats = ticks.unsigned_abs().min(64);

        if frame.mouse_event() != mind2t_vt_core::mouse::MouseEvent::None {
            let Some(size) = self.size(cell) else {
                return Wheel::Viewport;
            };
            let button = if ticks > 0 { Button::Four } else { Button::Five };
            let mut out = Vec::new();
            for _ in 0..repeats {
                if let Some(bytes) = mind2t_vt_pty::mouse::encode(
                    Event { action: Action::Press, button: Some(button), mods, x, y },
                    Options {
                        event_mode: frame.mouse_event(),
                        format: frame.mouse_format(),
                        size,
                        any_button_pressed: self.buttons_held != 0,
                        last_cell: Some(&mut self.last_cell),
                    },
                ) {
                    out.extend(bytes);
                }
            }
            // Reporting is ON, so the wheel is the child's even when the encoding produced
            // nothing (X10 cannot name a wheel button). Falling through to the viewport here
            // would scroll the view under a program that had captured the mouse.
            return Wheel::Send(out);
        }

        if frame.alternate_screen() && frame.mouse_alternate_scroll() {
            let seq: &[u8] = match (frame.cursor_keys(), ticks > 0) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1bOB",
                (false, true) => b"\x1b[A",
                (false, false) => b"\x1b[B",
            };
            let mut out = Vec::with_capacity(seq.len() * repeats as usize);
            for _ in 0..repeats {
                out.extend_from_slice(seq);
            }
            return Wheel::Send(out);
        }

        Wheel::Viewport
    }
}
