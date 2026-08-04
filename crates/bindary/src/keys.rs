//! Purpose: turning `tao` key events into the bytes a child on a pty expects.
//! Public surface: `encode_press`, `mods_from`.
//! Why this file: the encoder already exists and is corpus-pinned (`ruuah_vt_pty::key`), keyed
//!   on W3C `KeyCode` names. `tao` reports physical keys with the SAME W3C names. So the whole
//!   job here is the bridge between two spellings of one standard, and putting it in its own
//!   file is what keeps that claim testable instead of buried in an event loop.
//! NOT responsible for: knowing which modes the terminal is in. `KeyOptions` arrives as a
//!   parameter, built by the session from the frame the child's own escape sequences produced -
//!   this file never guesses at DECCKM, the kitty flags or option-as-alt policy.
//! Test strategy: the bridge is a NAME match, so a rename on either side silently produces
//!   `Key::Unidentified` and the key does nothing - the exact silent failure this project
//!   hunts. The tests below pin representative keys from every W3C section AND assert the
//!   unmapped case is reported rather than guessed.

use ruuah_vt_pty::key::{
    KEY_MODS_ALT, KEY_MODS_CTRL, KEY_MODS_SHIFT, KEY_MODS_SUPER, Key, KeyAction, KeyEvent,
    KeyMods, KeyOptions, encode,
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
///
/// `options` comes from the SESSION, not from a constant: DECCKM and friends are modes the child
/// enters at runtime, and encoding arrows the legacy way inside a program that asked for
/// application cursor keys sends `ESC [ A` where `ESC O A` was expected. That reads as a broken
/// arrow key, not as a wrong mode, which is why it is worth threading a parameter for.
pub fn encode_press(event: &TaoKeyEvent, mods: KeyMods, options: &KeyOptions) -> Vec<u8> {
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

    encode(&event, options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruuah_vt_pty::key::OptionAsAlt;

    /// A terminal in its default modes. Tests state the modes they encode against rather than
    /// borrowing a host's, so a change in what the session reports cannot silently rewrite what
    /// these assertions mean.
    fn legacy() -> KeyOptions {
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
        assert_eq!(encode(&event, &legacy()), vec![0x03]);
    }

    /// The reason `KeyOptions` is a parameter rather than a constant, pinned by bytes.
    ///
    /// An up arrow is `ESC [ A` in the default modes and `ESC O A` once a program has requested
    /// application cursor keys - one byte apart, and the wrong one inside vim moves nothing while
    /// leaving a stray character behind. A host that hardcoded the defaults would be correct in a
    /// shell and wrong in every full-screen program, which is the shape of bug that gets blamed
    /// on the keyboard.
    #[test]
    fn an_arrow_encodes_differently_once_the_child_asks_for_application_cursor_keys() {
        let arrow = KeyEvent {
            action: KeyAction::Press,
            key: Key::ArrowUp,
            mods: 0,
            consumed_mods: 0,
            composing: false,
            utf8: "",
            unshifted_codepoint: 0,
        };

        let mut application = legacy();
        application.cursor_key_application = true;

        assert_eq!(encode(&arrow, &legacy()), b"\x1b[A".to_vec());
        assert_eq!(encode(&arrow, &application), b"\x1bOA".to_vec());
    }
}
