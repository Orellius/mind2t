//! Purpose: turning `tao` key events into the bytes a child on a pty expects.
//! Public surface: `encode_press`, `mods_from`.
//! Why this file: the encoder already exists and is corpus-pinned (`ruuah_vt_pty::key`), keyed
//!   on W3C `KeyCode` names. `tao` reports physical keys with the SAME W3C names. So the whole
//!   job here is the bridge between two spellings of one standard, and putting it in its own
//!   file is what keeps that claim testable instead of buried in an event loop.
//! NOT responsible for: the kitty keyboard protocol, DECCKM state, or option-as-alt policy.
//!   Those live in `KeyOptions`, which the caller owns and which is hardcoded to its defaults
//!   here until the session reports the modes it has entered (named gap, B2.x).
//! Test strategy: the bridge is a NAME match, so a rename on either side silently produces
//!   `Key::Unidentified` and the key does nothing - the exact silent failure this project
//!   hunts. The tests below pin representative keys from every W3C section AND assert the
//!   unmapped case is reported rather than guessed.

use ruuah_vt_pty::key::{
    KEY_MODS_ALT, KEY_MODS_CAPS_LOCK, KEY_MODS_CTRL, KEY_MODS_SHIFT, KEY_MODS_SUPER, Key,
    KeyAction, KeyEvent, KeyMods, KeyOptions, OptionAsAlt, encode,
};
use tao::event::KeyEvent as TaoKeyEvent;
use tao::keyboard::{KeyCode, ModifiersState};

/// Maps a `tao` physical key to the encoder's `Key`.
///
/// Both enums are the W3C `KeyboardEvent.code` list, so the mapping is name equality - with one
/// spelling difference: W3C writes letters as `KeyA` while `event.h`'s `GhosttyKey` writes `A`,
/// and this crate mirrors `event.h`. Matching on the Debug name rather than writing 120 arms is
/// deliberate: an explicit table of that size is where a single transposed pair hides forever,
/// and the property that makes the bridge correct - one shared standard - is exactly what a
/// name comparison checks. `Unidentified` is returned for anything with no twin, which the
/// encoder already treats as "produces no bytes unless it carries text".
fn key_from(code: KeyCode) -> Key {
    let spelling = format!("{code:?}");
    let name = spelling.strip_prefix("Key").unwrap_or(&spelling);
    Key::ALL
        .iter()
        .copied()
        .find(|candidate| format!("{candidate:?}") == name)
        .unwrap_or(Key::Unidentified)
}

pub fn mods_from(state: ModifiersState) -> KeyMods {
    let mut mods = 0;
    if state.shift_key() {
        mods |= KEY_MODS_SHIFT;
    }
    if state.control_key() {
        mods |= KEY_MODS_CTRL;
    }
    if state.alt_key() {
        mods |= KEY_MODS_ALT;
    }
    if state.super_key() {
        mods |= KEY_MODS_SUPER;
    }
    mods
}

/// The bytes one key PRESS should send to the child. Empty means "nothing to send", which is a
/// common and correct outcome (a bare modifier, a key the layout maps to nothing).
pub fn encode_press(event: &TaoKeyEvent, mods: KeyMods) -> Vec<u8> {
    let key = key_from(event.physical_key);
    let text = event.text.unwrap_or("");

    // Shift is CONSUMED when the layout used it to produce the text - the encoder must not then
    // also report shift as a live modifier, or shift+a arrives as a modified 'A' instead of an
    // 'A'. Compared against the unshifted codepoint rather than assumed, because a layout is
    // free to disagree: on the Hebrew layout shift+t produces a different letter entirely.
    let unshifted = key.codepoint().unwrap_or(0);
    let mut consumed = 0;
    if !text.is_empty()
        && mods & KEY_MODS_SHIFT != 0
        && text.chars().next().map(u32::from) != Some(unshifted)
    {
        consumed |= KEY_MODS_SHIFT;
    }

    let event = KeyEvent {
        action: KeyAction::Press,
        key,
        mods,
        consumed_mods: consumed,
        composing: false,
        utf8: text,
        unshifted_codepoint: unshifted,
    };

    encode(&event, &options())
}

/// The terminal modes the encoder branches on.
///
/// Hardcoded to the defaults, and that is a NAMED GAP rather than a decision: DECCKM and the
/// kitty flags are terminal state the core tracks, so arrow keys inside a full-screen program
/// that requested application cursor mode will encode the wrong way until the session reports
/// them. Wiring that is the next slice; pretending it is fine is how it would never be wired.
fn options() -> KeyOptions {
    KeyOptions {
        cursor_key_application: false,
        keypad_key_application: false,
        ignore_keypad_with_numlock: false,
        alt_esc_prefix: false,
        modify_other_keys_state_2: false,
        kitty_flags: 0,
        macos_option_as_alt: OptionAsAlt::False,
        backarrow_key_mode: false,
    }
}

/// Unused today, kept beside its siblings so the mask is defined in one place if caps lock ever
/// needs reporting; the encoder ignores it in the legacy path.
#[allow(dead_code)]
const CAPS: KeyMods = KEY_MODS_CAPS_LOCK;

#[cfg(test)]
mod tests {
    use super::*;

    /// One key from every W3C section the bridge has to cross, spelled by tao on the left and by
    /// `event.h` on the right. A rename or a reordering on either side turns one of these into
    /// `Unidentified`, which is the failure that would otherwise present as "that key does
    /// nothing" with no error anywhere.
    #[test]
    fn the_two_spellings_of_the_w3c_code_list_agree() {
        let pairs = [
            (KeyCode::KeyA, Key::A),
            (KeyCode::KeyZ, Key::Z),
            (KeyCode::Digit0, Key::Digit0),
            (KeyCode::Minus, Key::Minus),
            (KeyCode::Enter, Key::Enter),
            (KeyCode::Backspace, Key::Backspace),
            (KeyCode::Tab, Key::Tab),
            (KeyCode::Space, Key::Space),
            (KeyCode::Escape, Key::Escape),
            (KeyCode::ArrowUp, Key::ArrowUp),
            (KeyCode::ArrowDown, Key::ArrowDown),
            (KeyCode::ArrowLeft, Key::ArrowLeft),
            (KeyCode::ArrowRight, Key::ArrowRight),
            (KeyCode::Home, Key::Home),
            (KeyCode::PageUp, Key::PageUp),
            (KeyCode::Delete, Key::Delete),
            (KeyCode::F1, Key::F1),
            (KeyCode::F12, Key::F12),
            (KeyCode::ControlLeft, Key::ControlLeft),
            (KeyCode::ShiftRight, Key::ShiftRight),
            (KeyCode::Numpad7, Key::Numpad7),
            (KeyCode::NumpadEnter, Key::NumpadEnter),
        ];
        for (code, expected) in pairs {
            assert_eq!(key_from(code), *&expected, "{code:?} lost its twin");
        }
    }

    /// The control. If `key_from` returned something plausible for everything, the test above
    /// would pass on a bridge that always answered `Key::A` - so a code with no twin in the
    /// encoder's list must come back Unidentified, not guessed.
    #[test]
    fn a_code_with_no_twin_is_unidentified_rather_than_guessed() {
        assert_eq!(key_from(KeyCode::Lang1), Key::Unidentified);
        assert_eq!(key_from(KeyCode::Abort), Key::Unidentified);
    }

    /// ctrl+c is the one chord whose absence turns a terminal into a trap, so it is pinned by
    /// its bytes rather than by the mapping alone: 0x03, the ASCII ETX the child's line
    /// discipline turns into SIGINT.
    #[test]
    fn ctrl_c_encodes_to_the_interrupt_byte() {
        let event = KeyEvent {
            action: KeyAction::Press,
            key: Key::C,
            mods: KEY_MODS_CTRL,
            consumed_mods: 0,
            composing: false,
            utf8: "",
            unshifted_codepoint: u32::from('c'),
        };
        assert_eq!(encode(&event, &options()), vec![0x03]);
    }
}
