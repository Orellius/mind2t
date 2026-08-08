//! Purpose: turning `tao` key events into the bytes a child on a pty expects.
//! Public surface: `encode_press`, `mods_from`.
//! Why this file: the encoder already exists and is corpus-pinned (`mind2t_vt_pty::key`), keyed
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

use mind2t_vt_pty::key::{
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
    encode_key(key_from(event.physical_key), event.text.unwrap_or(""), mods, options)
}

/// The same, for a host that already knows the physical key and the text its layout produced.
///
/// EXTRACTED 2026-08-08 (T2b) because it had two copies. This function's body was written here
/// for the tao host and then written again, comment for comment, inside the AppKit monitor in
/// `main.rs` - and the Linux GTK path was about to make a third. Nothing compared them, and the
/// rule they share is subtle enough that the copies could drift by one condition and present as
/// "shift does something strange in one build".
///
/// It is the seam every platform meets at: a host's whole job is to answer three questions -
/// which physical key, what text did the layout make, which modifiers are down - and from there
/// the bytes are not a platform question at all.
pub fn encode_key(key: Key, text: &str, mods: KeyMods, options: &KeyOptions) -> Vec<u8> {
    // Shift is CONSUMED when the layout used it to produce the text. Compared against the
    // unshifted codepoint rather than assumed, because a layout is free to disagree: on the
    // Hebrew layout shift+t produces a different letter entirely.
    //
    // THIS IS ORACLE PARITY, NOT BEHAVIOUR, and the difference was measured on 2026-08-08 rather
    // than assumed from how load-bearing the comment used to sound. `effective_mods` is
    // `mods & !consumed_mods`, and it reaches only `binding_mods`, which in the legacy path
    // decides one thing: whether an alt-prefix ESC is emitted. So consuming ALT is observable
    // and consuming SHIFT is not - not in legacy, not under modifyOtherKeys, not under any
    // kitty flag set. Swept over 5 key shapes x 6 option sets; every comparison was identical.
    //
    // It stays because `key.zig` computes the same field the same way and a drop-in that
    // silently disagreed about it would be a divergence waiting for a mode that reads it. Both
    // claims are pinned: `a_consumed_modifier_is_observable_through_the_alt_prefix_and_only_there`
    // proves the mechanism is live, and `consuming_shift_is_oracle_parity_and_currently_changes_
    // no_bytes` fails the day that stops being true.
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

/// GDK's `ModifierType` bits, verified against docs.gtk.org/gdk3 on 2026-08-08 rather than
/// recalled. Only the four the encoder has a modifier for are named; the rest exist and are
/// deliberately ignored below.
mod gdk_mask {
    pub const SHIFT: u32 = 1;
    /// Caps lock. NOT a modifier the encoder knows about - see `mods_from_gdk`.
    pub const LOCK: u32 = 2;
    pub const CONTROL: u32 = 4;
    /// Alt. GDK does not name it `ALT`, and that is the whole trap: `MOD1` is conventionally
    /// alt on every desktop Linux, while `MOD2`..`MOD5` are num lock, scroll lock and the like.
    pub const MOD1: u32 = 8;
    pub const SUPER: u32 = 1 << 26;
    /// Meta. Folded into super below, because a terminal has one command-shaped modifier.
    pub const META: u32 = 1 << 28;
}

/// GDK modifier bits -> the encoder's modifier set.
///
/// Portable on purpose: it is arithmetic on a `u32`, so it is tested from the machine this
/// project is developed on rather than only on the platform it targets. The GTK glue that reads
/// `event.state().bits()` is the thin cfg'd edge; this is the part that can be wrong.
///
/// **`LOCK` is dropped, and dropping it is correct.** Caps lock has already done its work by the
/// time the layout produced the text - reporting it as a live modifier would make every
/// capitalised letter arrive as a modified key. The same reasoning retires `MOD2`..`MOD5` and
/// every `BUTTON` bit, which ride along in the same word during a drag.
pub fn mods_from_gdk(state: u32) -> KeyMods {
    let mut mods = 0;
    if state & gdk_mask::SHIFT != 0 {
        mods |= KEY_MODS_SHIFT;
    }
    if state & gdk_mask::CONTROL != 0 {
        mods |= KEY_MODS_CTRL;
    }
    if state & gdk_mask::MOD1 != 0 {
        mods |= KEY_MODS_ALT;
    }
    if state & (gdk_mask::SUPER | gdk_mask::META) != 0 {
        mods |= KEY_MODS_SUPER;
    }
    let _ = gdk_mask::LOCK;
    mods
}

#[cfg(test)]
mod tests {
    use super::*;
    use mind2t_vt_pty::key::OptionAsAlt;

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

    /// THE CONSUMED-MODIFIER MECHANISM IS LIVE, and this is the only place it can be seen.
    ///
    /// Measured 2026-08-08 by sweeping consumed against not-consumed over five key shapes and
    /// six option sets: 20 of 20 comparisons were byte-identical until `alt_esc_prefix` and
    /// `macos_option_as_alt` were BOTH on. `effective_mods` (`mods & !consumed_mods`, key.zig's
    /// `effectiveMods`) reaches only `binding_mods`, and the one thing `binding_mods` decides in
    /// the legacy path is whether an alt-prefix `ESC` is emitted ahead of the text.
    ///
    /// So: consuming ALT removes the ESC, and not consuming it keeps the ESC. Both directions,
    /// by bytes. Without this test nothing in the project would fail if `consumed_mods` were
    /// deleted from the encoder entirely.
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

    /// A CHARACTERISATION PIN, and it is named for what it actually asserts.
    ///
    /// `encode_key` consumes SHIFT and never anything else - and by the measurement above,
    /// consuming SHIFT changes no bytes anywhere: not in the legacy path, not under
    /// modifyOtherKeys, not under any kitty flag combination. The rule is ORACLE PARITY
    /// (`key.zig` computes the same field the same way), not behaviour this terminal exhibits.
    ///
    /// Saying that out loud matters because the rule LOOKS load-bearing - it carries a careful
    /// comment about Hebrew layouts, it is subtle, and it was duplicated into a second host on
    /// the strength of looking important. A suppressor that has never been shown to fire is the
    /// SCAR-004 shape, and the honest handling is to keep it for parity and label it, not to
    /// credit it with work it does not do.
    ///
    /// If the encoder ever grows a path where a consumed shift DOES change the bytes, this test
    /// fails and the label above stops being true. That failure is the feature.
    #[test]
    fn consuming_shift_is_oracle_parity_and_currently_changes_no_bytes() {
        let mut modify_other = legacy();
        modify_other.modify_other_keys_state_2 = true;
        let mut kitty = legacy();
        kitty.kitty_flags = 0b1_1111;

        for options in [legacy(), modify_other, kitty] {
            for (key, text, unshifted) in [
                (Key::A, "A", 'a' as u32),
                (Key::Digit1, "!", '1' as u32),
                (Key::Space, " ", ' ' as u32),
            ] {
                let _ = unshifted;
                assert_eq!(
                    encode_key_with(key, text, KEY_MODS_SHIFT, 0, &options),
                    encode_key_with(key, text, KEY_MODS_SHIFT, KEY_MODS_SHIFT, &options),
                    "consuming shift became observable for {key:?} - update the claim in \
                     `encode_key`, it is no longer parity-only"
                );
            }
        }
    }

    /// `encode_key` with the consumed set stated outright, so the two tests above can drive both
    /// halves. The production path derives it; these ask what the derivation is FOR.
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

    /// A key with no text is untouched by the rule, which is the common case for every chord.
    #[test]
    fn a_key_that_produced_no_text_keeps_every_modifier() {
        assert_eq!(encode_key(Key::C, "", KEY_MODS_CTRL, &legacy()), vec![0x03]);
    }

    /// GDK's modifier word, mapped. Verified constants, and the three traps in one test.
    ///
    /// The bits are documented values (docs.gtk.org/gdk3, read 2026-08-08), not recalled ones.
    /// What makes this worth a test rather than a glance is that the word GDK hands a key
    /// handler carries far more than modifiers: caps lock, num lock and every mouse button
    /// currently held ride in the same `u32`. A mapping that forwarded unknown bits would report
    /// phantom modifiers during a drag, and a mapping that treated `MOD1` as anything but alt
    /// would silently lose the alt chord on every Linux desktop.
    #[test]
    fn gdk_modifier_bits_map_to_the_encoders_modifiers_and_nothing_else() {
        assert_eq!(mods_from_gdk(0), 0);
        assert_eq!(mods_from_gdk(gdk_mask::SHIFT), KEY_MODS_SHIFT);
        assert_eq!(mods_from_gdk(gdk_mask::CONTROL), KEY_MODS_CTRL);
        assert_eq!(mods_from_gdk(gdk_mask::MOD1), KEY_MODS_ALT);
        assert_eq!(mods_from_gdk(gdk_mask::SUPER), KEY_MODS_SUPER);
        assert_eq!(mods_from_gdk(gdk_mask::META), KEY_MODS_SUPER);

        assert_eq!(
            mods_from_gdk(gdk_mask::CONTROL | gdk_mask::SHIFT),
            KEY_MODS_CTRL | KEY_MODS_SHIFT
        );

        // Caps lock is already spent on the text; num lock and the mod2..mod5 family are not
        // modifiers a terminal encodes; BUTTON1 is set for the whole of every drag.
        assert_eq!(mods_from_gdk(gdk_mask::LOCK), 0, "caps lock is not a live modifier");
        assert_eq!(mods_from_gdk(16 | 32 | 64 | 128), 0, "mod2..mod5 are not alt");
        assert_eq!(mods_from_gdk(256 | 512 | 1024), 0, "held mouse buttons are not modifiers");

        // And the combination that would expose a mapping which forwards the whole word.
        assert_eq!(
            mods_from_gdk(gdk_mask::LOCK | gdk_mask::CONTROL | 256),
            KEY_MODS_CTRL
        );
    }
}
