//! The generated macOS keycode table.
//!
//! WHAT THIS FILE USED TO DO, and why saying so matters. Until 2026-08-08 it read
//! `swift/Sources/mind2t-host/KeyMap.swift` at test time and demanded exact agreement with the
//! Rust table. `scripts/gen-keymap.ts` wrote both from one source, and two generated copies are
//! safe only while they are actually regenerated together - the moment somebody hand-edits one,
//! or regenerates against a moved oracle and commits only the file their build touched, the two
//! hosts disagree about what a key IS. That failure is per-key and silent.
//!
//! The Swift host was retired in T6, so that test had nothing left to compare against and is
//! gone. **The property it guarded is NOT gone**: `keycode_linux.rs` makes the same check
//! between the macOS and Linux tables, by parsing the DomCode each entry carries in its
//! generated comment. It is a better version of the same test, because both sides now live in
//! this repository and neither can be edited without the other being visible in the same diff.
//!
//! What is genuinely lost is narrower and worth naming: nothing now proves the table is usable
//! from a language that is not Rust. That was never what this test measured, but the Swift file
//! existing was incidental evidence of it.

#![cfg(target_os = "macos")]

use mind2t_vt_pty::key::Key;
use mind2t_vt_pty::keycode::{MACOS_KEYCODES, key_from_macos_keycode};

/// The table is binary-searched, which is a silent-wrong-answer bug if it is ever unsorted.
#[test]
fn the_table_is_sorted_because_the_lookup_binary_searches_it() {
    assert!(
        MACOS_KEYCODES.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "MACOS_KEYCODES is not strictly ascending; binary search would miss real keys"
    );
}

/// Spot values a human can verify against a keyboard, plus the unmapped direction.
///
/// The second half is the control: without it, a lookup that returned `Key::A` for everything
/// would pass the first half whenever it was asked about A.
#[test]
fn known_keys_resolve_and_unknown_ones_stay_unidentified() {
    assert_eq!(key_from_macos_keycode(0), Key::A);
    assert_eq!(key_from_macos_keycode(36), Key::Enter);
    assert_eq!(key_from_macos_keycode(49), Key::Space);
    assert_eq!(key_from_macos_keycode(53), Key::Escape);
    assert_eq!(key_from_macos_keycode(126), Key::ArrowUp);
    assert_eq!(key_from_macos_keycode(125), Key::ArrowDown);

    assert_eq!(key_from_macos_keycode(u16::MAX), Key::Unidentified);
    assert_eq!(key_from_macos_keycode(1000), Key::Unidentified);
}

/// No entry may resolve to `Unidentified`.
///
/// Inherited from the Linux table's suite, because the reason applies to both: an entry pointing
/// past `Key::ALL` answers exactly as if there were no entry at all, so a generator that emitted
/// a wrong enum value would be invisible to every lookup test above.
#[test]
fn every_macos_entry_names_a_real_key() {
    for (code, value) in MACOS_KEYCODES {
        assert_ne!(
            key_from_macos_keycode(*code),
            Key::Unidentified,
            "macOS keycode {code} carries enum value {value}, which is not a key"
        );
    }
}
