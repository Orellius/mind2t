//! The mouse encoder, measured against the oracle's own.
//!
//! `ghostty_mouse_encoder_encode` is the reference implementation of the transform
//! `crates/pty/src/mouse.rs` performs when a pointer event becomes pty bytes. Three
//! layers of comparison:
//!   1. a full matrix over modes x formats x actions x buttons x mods x positions x
//!      button-held, byte-for-byte, on several geometries;
//!   2. motion-deduplication SEQUENCES through one stateful encoder on each side;
//!   3. end-to-end derived state: the same mode byte streams fed to both terminals,
//!      the oracle's encoder configured via `setopt_from_terminal`, ours via the
//!      core's `mouse_event()`/`mouse_format()` -- which pins the last-writer-wins
//!      mapping differentially, not just by unit test.
//! The controls at the bottom prove the comparison can fail and the matrix is not
//! vacuously silent.

use mind2t_vt_core::mouse::{MouseEvent, MouseFormat};
use mind2t_vt_ghostty::sys;
use mind2t_vt_pty::mouse::{self, Action, Button, Event, Mods, Options, Size};

/// One geometry shared with the Rust side.
#[derive(Clone, Copy)]
struct Geometry {
    size: Size,
}

const GEOMETRIES: &[Geometry] = &[
    // The app's real shape: 8px insets, uneven cell.
    Geometry {
        size: Size {
            screen_width: 1000,
            screen_height: 1000,
            cell_width: 10,
            cell_height: 20,
            padding_left: 8,
            padding_top: 8,
            padding_right: 8,
            padding_bottom: 8,
        },
    },
    // No padding, 1px cells: coordinates map straight through (the oracle's own
    // test size), which exercises the X10 222 boundary exactly.
    Geometry {
        size: Size {
            screen_width: 1000,
            screen_height: 1000,
            cell_width: 1,
            cell_height: 1,
            padding_left: 0,
            padding_top: 0,
            padding_right: 0,
            padding_bottom: 0,
        },
    },
    // Degenerate: screen smaller than one padded cell, grid floors at 1x1.
    Geometry {
        size: Size {
            screen_width: 30,
            screen_height: 30,
            cell_width: 40,
            cell_height: 40,
            padding_left: 4,
            padding_top: 4,
            padding_right: 4,
            padding_bottom: 4,
        },
    },
];

fn oracle_tracking(mode: MouseEvent) -> sys::GhosttyMouseTrackingMode {
    match mode {
        MouseEvent::None => sys::GhosttyMouseTrackingMode_GHOSTTY_MOUSE_TRACKING_NONE,
        MouseEvent::X10 => sys::GhosttyMouseTrackingMode_GHOSTTY_MOUSE_TRACKING_X10,
        MouseEvent::Normal => sys::GhosttyMouseTrackingMode_GHOSTTY_MOUSE_TRACKING_NORMAL,
        MouseEvent::Button => sys::GhosttyMouseTrackingMode_GHOSTTY_MOUSE_TRACKING_BUTTON,
        MouseEvent::Any => sys::GhosttyMouseTrackingMode_GHOSTTY_MOUSE_TRACKING_ANY,
    }
}

fn oracle_format(format: MouseFormat) -> sys::GhosttyMouseFormat {
    match format {
        MouseFormat::X10 => sys::GhosttyMouseFormat_GHOSTTY_MOUSE_FORMAT_X10,
        MouseFormat::Utf8 => sys::GhosttyMouseFormat_GHOSTTY_MOUSE_FORMAT_UTF8,
        MouseFormat::Sgr => sys::GhosttyMouseFormat_GHOSTTY_MOUSE_FORMAT_SGR,
        MouseFormat::Urxvt => sys::GhosttyMouseFormat_GHOSTTY_MOUSE_FORMAT_URXVT,
        MouseFormat::SgrPixels => sys::GhosttyMouseFormat_GHOSTTY_MOUSE_FORMAT_SGR_PIXELS,
    }
}

fn oracle_button(button: Button) -> sys::GhosttyMouseButton {
    match button {
        Button::Left => sys::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_LEFT,
        Button::Middle => sys::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_MIDDLE,
        Button::Right => sys::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_RIGHT,
        Button::Four => sys::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_FOUR,
        Button::Five => sys::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_FIVE,
        Button::Six => sys::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_SIX,
        Button::Seven => sys::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_SEVEN,
        Button::Eight => sys::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_EIGHT,
        Button::Nine => sys::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_NINE,
        // A button the protocol has no code for; both encoders must go silent.
        Button::Other => sys::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_TEN,
    }
}

/// A live oracle encoder handle, kept across calls so dedup sequences are honest.
struct OracleEncoder {
    raw: sys::GhosttyMouseEncoder,
}

impl OracleEncoder {
    fn new(geometry: Geometry, track_last_cell: bool) -> Self {
        let mut raw: sys::GhosttyMouseEncoder = std::ptr::null_mut();
        let code = unsafe { sys::ghostty_mouse_encoder_new(std::ptr::null(), &mut raw) };
        assert_eq!(code, sys::GhosttyResult_GHOSTTY_SUCCESS, "encoder_new");

        let size = sys::GhosttyMouseEncoderSize {
            size: size_of::<sys::GhosttyMouseEncoderSize>(),
            screen_width: geometry.size.screen_width,
            screen_height: geometry.size.screen_height,
            cell_width: geometry.size.cell_width,
            cell_height: geometry.size.cell_height,
            padding_top: geometry.size.padding_top,
            padding_bottom: geometry.size.padding_bottom,
            padding_right: geometry.size.padding_right,
            padding_left: geometry.size.padding_left,
        };
        unsafe {
            sys::ghostty_mouse_encoder_setopt(
                raw,
                sys::GhosttyMouseEncoderOption_GHOSTTY_MOUSE_ENCODER_OPT_SIZE,
                (&raw const size).cast(),
            );
            sys::ghostty_mouse_encoder_setopt(
                raw,
                sys::GhosttyMouseEncoderOption_GHOSTTY_MOUSE_ENCODER_OPT_TRACK_LAST_CELL,
                (&raw const track_last_cell).cast(),
            );
        }
        Self { raw }
    }

    fn set_modes(&self, event_mode: MouseEvent, format: MouseFormat) {
        let tracking = oracle_tracking(event_mode);
        let fmt = oracle_format(format);
        unsafe {
            sys::ghostty_mouse_encoder_setopt(
                self.raw,
                sys::GhosttyMouseEncoderOption_GHOSTTY_MOUSE_ENCODER_OPT_EVENT,
                (&raw const tracking).cast(),
            );
            sys::ghostty_mouse_encoder_setopt(
                self.raw,
                sys::GhosttyMouseEncoderOption_GHOSTTY_MOUSE_ENCODER_OPT_FORMAT,
                (&raw const fmt).cast(),
            );
        }
    }

    fn set_any_button(&self, held: bool) {
        unsafe {
            sys::ghostty_mouse_encoder_setopt(
                self.raw,
                sys::GhosttyMouseEncoderOption_GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED,
                (&raw const held).cast(),
            );
        }
    }

    /// From the oracle TERMINAL's state -- the derived-mapping comparison.
    fn set_from_terminal(&self, terminal: &mind2t_vt_ghostty::Terminal) {
        unsafe {
            sys::ghostty_mouse_encoder_setopt_from_terminal(self.raw, terminal.raw());
        }
    }

    fn encode(&self, event: &Event) -> Vec<u8> {
        let mut raw_event: sys::GhosttyMouseEvent = std::ptr::null_mut();
        let code = unsafe { sys::ghostty_mouse_event_new(std::ptr::null(), &mut raw_event) };
        assert_eq!(code, sys::GhosttyResult_GHOSTTY_SUCCESS, "event_new");

        unsafe {
            sys::ghostty_mouse_event_set_action(
                raw_event,
                match event.action {
                    Action::Press => sys::GhosttyMouseAction_GHOSTTY_MOUSE_ACTION_PRESS,
                    Action::Release => sys::GhosttyMouseAction_GHOSTTY_MOUSE_ACTION_RELEASE,
                    Action::Motion => sys::GhosttyMouseAction_GHOSTTY_MOUSE_ACTION_MOTION,
                },
            );
            match event.button {
                Some(button) => {
                    sys::ghostty_mouse_event_set_button(raw_event, oracle_button(button));
                }
                None => sys::ghostty_mouse_event_clear_button(raw_event),
            }
            let mut mods: sys::GhosttyMods = 0;
            if event.mods.shift {
                mods |= sys::GHOSTTY_MODS_SHIFT as sys::GhosttyMods;
            }
            if event.mods.ctrl {
                mods |= sys::GHOSTTY_MODS_CTRL as sys::GhosttyMods;
            }
            if event.mods.alt {
                mods |= sys::GHOSTTY_MODS_ALT as sys::GhosttyMods;
            }
            sys::ghostty_mouse_event_set_mods(raw_event, mods);
            sys::ghostty_mouse_event_set_position(
                raw_event,
                sys::GhosttyMousePosition { x: event.x, y: event.y },
            );
        }

        let mut out = vec![0u8; 64];
        let mut written = 0usize;
        let code = unsafe {
            sys::ghostty_mouse_encoder_encode(
                self.raw,
                raw_event,
                out.as_mut_ptr().cast(),
                out.len(),
                &mut written,
            )
        };
        unsafe { sys::ghostty_mouse_event_free(raw_event) };
        assert_eq!(code, sys::GhosttyResult_GHOSTTY_SUCCESS, "encode");
        out.truncate(written);
        out
    }
}

impl Drop for OracleEncoder {
    fn drop(&mut self) {
        unsafe { sys::ghostty_mouse_encoder_free(self.raw) };
    }
}

/// Our encoder under matrix conditions: no dedup tracking, everything else from the
/// case. `None` from `encode` and "zero bytes" from the oracle are the same verdict.
fn ours(event: &Event, mode: MouseEvent, format: MouseFormat, geometry: Geometry, held: bool) -> Vec<u8> {
    mouse::encode(
        *event,
        Options {
            event_mode: mode,
            format,
            size: geometry.size,
            any_button_pressed: held,
            last_cell: None,
        },
    )
    .unwrap_or_default()
}

const MODES: &[MouseEvent] =
    &[MouseEvent::None, MouseEvent::X10, MouseEvent::Normal, MouseEvent::Button, MouseEvent::Any];
const FORMATS: &[MouseFormat] = &[
    MouseFormat::X10,
    MouseFormat::Utf8,
    MouseFormat::Sgr,
    MouseFormat::Urxvt,
    MouseFormat::SgrPixels,
];
const ACTIONS: &[Action] = &[Action::Press, Action::Release, Action::Motion];
const BUTTONS: &[Option<Button>] = &[
    None,
    Some(Button::Left),
    Some(Button::Middle),
    Some(Button::Right),
    Some(Button::Four),
    Some(Button::Five),
    Some(Button::Eight),
    Some(Button::Other),
];
const MODS: &[Mods] = &[
    Mods { shift: false, alt: false, ctrl: false },
    Mods { shift: true, alt: false, ctrl: false },
    Mods { shift: true, alt: true, ctrl: true },
];
/// Includes both viewport edges, the padding band, negatives, deep overshoot (past
/// X10's 222 limit on the 1px geometry), and fractional positions.
const POSITIONS: &[(f32, f32)] = &[
    (0.0, 0.0),
    (8.0, 8.0),
    (85.0, 50.0),
    (999.0, 999.0),
    (1000.0, 1000.0),
    (-5.0, 50.0),
    (2500.0, 50.0),
    (64.9, 127.3),
    (300.5, 300.5),
];

#[test]
fn every_matrix_case_agrees_byte_for_byte() {
    let mut compared = 0usize;
    let mut nonempty = 0usize;
    for &geometry in GEOMETRIES {
        let oracle = OracleEncoder::new(geometry, false);
        for &mode in MODES {
            for &format in FORMATS {
                oracle.set_modes(mode, format);
                for &action in ACTIONS {
                    for &button in BUTTONS {
                        for &mods in MODS {
                            for &(x, y) in POSITIONS {
                                for held in [false, true] {
                                    oracle.set_any_button(held);
                                    let event = Event { action, button, mods, x, y };
                                    let expected = oracle.encode(&event);
                                    let got = ours(&event, mode, format, geometry, held);
                                    assert_eq!(
                                        got, expected,
                                        "mode={mode:?} format={format:?} action={action:?} \
                                         button={button:?} mods={mods:?} pos=({x},{y}) held={held} \
                                         geometry={:?}",
                                        geometry.size
                                    );
                                    compared += 1;
                                    if !expected.is_empty() {
                                        nonempty += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    // A matrix that never produced bytes would "agree" forever; demand real output.
    assert!(compared > 50_000, "{compared}");
    assert!(nonempty > 10_000, "only {nonempty} of {compared} cases produced bytes");
}

/// Motion dedup is stateful, so it is compared as a SEQUENCE through one encoder on
/// each side: cross-cell drags report, in-cell jitter is silent, and a press in the
/// same cell still reports (dedup is motion-only).
#[test]
fn a_drag_sequence_deduplicates_identically() {
    let geometry = GEOMETRIES[0];
    let oracle = OracleEncoder::new(geometry, true);
    oracle.set_modes(MouseEvent::Button, MouseFormat::Sgr);
    oracle.set_any_button(true);

    let mut last = None;
    let steps: &[(Action, f32, f32)] = &[
        (Action::Press, 85.0, 50.0),
        (Action::Motion, 86.0, 50.0),  // same cell
        (Action::Motion, 89.0, 51.0),  // same cell
        (Action::Motion, 95.0, 50.0),  // next column
        (Action::Motion, 95.5, 50.0),  // same cell again
        (Action::Motion, 85.0, 90.0),  // down two rows
        (Action::Press, 85.0, 90.0),   // press in the deduped cell must still report
        (Action::Release, 85.0, 90.0),
    ];
    let mut transcript_oracle = Vec::new();
    let mut transcript_ours = Vec::new();
    for &(action, x, y) in steps {
        let event =
            Event { action, button: Some(Button::Left), mods: Mods::default(), x, y };
        transcript_oracle.extend(oracle.encode(&event));
        if let Some(bytes) = mouse::encode(
            event,
            Options {
                event_mode: MouseEvent::Button,
                format: MouseFormat::Sgr,
                size: geometry.size,
                any_button_pressed: true,
                last_cell: Some(&mut last),
            },
        ) {
            transcript_ours.extend(bytes);
        }
    }
    assert_eq!(transcript_ours, transcript_oracle);
    // The sequence must have actually deduplicated: 8 steps, strictly fewer reports.
    let reports = transcript_oracle.iter().filter(|&&b| b == 0x1b).count();
    assert_eq!(reports, 5, "press, 2 cell changes, press, release");
}

/// The derived last-writer state, differentially: the same mode BYTES drive both
/// terminals, the oracle's encoder is configured by `setopt_from_terminal`, ours by
/// the core's accessors. This is what pins `MouseModes::set` to `stream_terminal.zig`
/// rather than to a unit test's opinion of it.
#[test]
fn terminal_derived_state_encodes_identically_after_every_mode_sequence() {
    let sequences: &[&str] = &[
        "\x1b[?1000h",
        "\x1b[?1000h\x1b[?1006h",
        "\x1b[?1000h\x1b[?1002h\x1b[?1002l", // the trap: raw bit survives, reporting off
        "\x1b[?1003h\x1b[?1016h",
        "\x1b[?9h",
        "\x1b[?1005h\x1b[?1000h",
        "\x1b[?1002h\x1b[?1015h\x1b[?1015l", // format falls back to x10, not utf8
        "\x1b[?1006h",                       // format without an event mode: silence
        "\x1b[?1000h\x1b[?1006h\x1bc",       // RIS clears both
        "\x1b[?1000h\x1b[?1006h\x1b[?1049h", // alt screen keeps both
    ];
    let geometry = GEOMETRIES[0];
    let probe = Event {
        action: Action::Press,
        button: Some(Button::Left),
        mods: Mods { shift: true, alt: false, ctrl: false },
        x: 85.0,
        y: 50.0,
    };
    let mut agreed_nonempty = 0;
    for bytes in sequences {
        let mut oracle_terminal =
            mind2t_vt_ghostty::Terminal::new(20, 5).expect("oracle terminal");
        oracle_terminal.write(bytes.as_bytes());
        let oracle = OracleEncoder::new(geometry, false);
        oracle.set_from_terminal(&oracle_terminal);
        oracle.set_any_button(true);
        let expected = oracle.encode(&probe);

        let mut terminal = mind2t_vt_core::terminal::Terminal::new(20, 5);
        terminal.write(bytes.as_bytes());
        let got = mouse::encode(
            probe,
            Options {
                event_mode: terminal.mouse_event(),
                format: terminal.mouse_format(),
                size: geometry.size,
                any_button_pressed: true,
                last_cell: None,
            },
        )
        .unwrap_or_default();

        assert_eq!(got, expected, "sequence {:?}", bytes.escape_debug().to_string());
        if !expected.is_empty() {
            agreed_nonempty += 1;
        }
    }
    assert!(agreed_nonempty >= 6, "only {agreed_nonempty} sequences produced reports");
}

/// The comparison can fail: a single deliberate misencoding (shift dropped) must
/// disagree with the oracle on a case where shift is set. Guards a harness whose
/// equality assert never sees real bytes.
#[test]
fn a_wrong_encoder_is_caught_by_the_matrix() {
    let geometry = GEOMETRIES[0];
    let oracle = OracleEncoder::new(geometry, false);
    oracle.set_modes(MouseEvent::Normal, MouseFormat::Sgr);
    oracle.set_any_button(true);
    let event = Event {
        action: Action::Press,
        button: Some(Button::Left),
        mods: Mods { shift: true, alt: false, ctrl: false },
        x: 85.0,
        y: 50.0,
    };
    let expected = oracle.encode(&event);
    let mut broken = event;
    broken.mods.shift = false;
    let got = ours(&broken, MouseEvent::Normal, MouseFormat::Sgr, geometry, true);
    assert_ne!(got, expected, "dropping shift must be visible");
    assert!(!expected.is_empty());
}
