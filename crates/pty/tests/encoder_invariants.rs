//! Five encoder properties rescued from the Tauri host's key adapter before it was deleted.
//!
//! They lived in `crates/mind2t/src/keys.rs`, whose job was translating `tao`'s `KeyEvent` into
//! this crate's. Eight tests were in that file; three were genuinely about tao and GDK and died
//! with it. These five never touched tao at all - they build a `KeyEvent` by hand and assert what
//! the ENCODER does with it. They were in that file because that is where somebody happened to be
//! working, and deleting the host would have taken them with it silently.
//!
//! Checked before the move rather than assumed: no equivalent exists in `key.rs`'s own tests or
//! in `keycode.rs`/`keycode_linux.rs`. Dropping them would have been a real regression hidden
//! inside a deletion whose diff nobody would read line by line.

use mind2t_vt_pty::key::{
    KEY_MODS_ALT, KEY_MODS_CTRL, KEY_MODS_SHIFT, Key, KeyAction, KeyEvent, KeyMods, KeyOptions,
    OptionAsAlt, encode,
};

/// The pre-kitty, pre-modifyOtherKeys baseline: every extension off, so a byte that appears is
/// the legacy encoding and not some protocol's.
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

fn encode_key_with(
    key: Key,
    text: &str,
    mods: KeyMods,
    consumed: KeyMods,
    options: &KeyOptions,
) -> Vec<u8> {
    encode(
        &KeyEvent {
            action: KeyAction::Press,
            key,
            mods,
            consumed_mods: consumed,
            composing: false,
            utf8: text,
            unshifted_codepoint: key.codepoint().unwrap_or(0),
        },
        options,
    )
}

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

#[test]
fn a_consumed_modifier_is_observable_through_the_alt_prefix_and_only_there() {
    let mut alt = legacy();
    alt.alt_esc_prefix = true;
    alt.macos_option_as_alt = OptionAsAlt::True;

    // Alt is LIVE: the child is told, with an ESC ahead of the character.
    assert_eq!(
        encode_key_with(Key::A, "a", KEY_MODS_ALT, 0, &alt),
        b"\x1ba".to_vec()
    );
    // Alt was SPENT making the text: no prefix, just the character the layout produced.
    assert_eq!(
        encode_key_with(Key::A, "a", KEY_MODS_ALT, KEY_MODS_ALT, &alt),
        b"a".to_vec()
    );
}

#[test]
fn consuming_shift_is_oracle_parity_and_currently_changes_no_bytes() {
    let mut modify_other = legacy();
    modify_other.modify_other_keys_state_2 = true;
    let mut kitty = legacy();
    kitty.kitty_flags = 0b1_1111;

    for options in [legacy(), modify_other, kitty] {
        for (key, text) in [(Key::A, "A"), (Key::Digit1, "!"), (Key::Space, " ")] {
            assert_eq!(
                encode_key_with(key, text, KEY_MODS_SHIFT, 0, &options),
                encode_key_with(key, text, KEY_MODS_SHIFT, KEY_MODS_SHIFT, &options),
                "consuming shift became observable for {key:?} - update the claim in \
                 `encode_key`, it is no longer parity-only"
            );
        }
    }
}

#[test]
fn a_key_that_produced_no_text_keeps_every_modifier() {
    assert_eq!(
        encode_key_with(Key::C, "", KEY_MODS_CTRL, 0, &legacy()),
        vec![0x03]
    );
}
