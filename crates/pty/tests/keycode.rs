//! The generated macOS keycode table, checked against its OTHER generated copy.
//!
//! `scripts/gen-keymap.ts` writes two files from one source: a Swift dictionary the AppKit host
//! reads and a Rust table Mind2t reads. Two generated copies are safe only while they are
//! actually regenerated together - the moment someone hand-edits one, or regenerates against a
//! moved oracle and commits only the file their build touched, the hosts disagree about what a
//! key IS. The failure is per-key and silent: one key stops working in one app.
//!
//! So this test reads the SWIFT file at test time and requires exact agreement. It is the only
//! thing in the repo that can see that drift.

#![cfg(target_os = "macos")]

use std::collections::BTreeMap;

use ruuah_vt_pty::key::Key;
use ruuah_vt_pty::keycode::{MACOS_KEYCODES, key_from_macos_keycode};

fn swift_table() -> BTreeMap<u16, u16> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../swift/Sources/ruuah-host/KeyMap.swift"
    );
    let text = std::fs::read_to_string(path).expect("the Swift keymap is where it has always been");
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let (code, rest) = line.split_once(':')?;
            let (value, _) = rest.split_once(',')?;
            Some((code.trim().parse().ok()?, value.trim().parse().ok()?))
        })
        .collect()
}

#[test]
fn the_rust_and_swift_tables_are_the_same_table() {
    let swift = swift_table();
    assert!(
        swift.len() > 100,
        "parsed only {} entries from the Swift keymap; the parser, not the table, is wrong",
        swift.len()
    );

    let rust: BTreeMap<u16, u16> = MACOS_KEYCODES.iter().copied().collect();
    assert_eq!(
        rust, swift,
        "the two generated keycode tables disagree - regenerate BOTH with scripts/gen-keymap.ts"
    );
}

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
