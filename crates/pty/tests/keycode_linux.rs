//! The generated Linux keycode table, and the invariant that makes a per-platform key source
//! survivable at all.
//!
//! Orel's call on 2026-08-08 was a NATIVE key source per platform: `NSEvent` on macOS, GTK's
//! `key-press-event` on Linux, and a third one on Windows later. The named risk of that route,
//! in his own words, is two tables that must never disagree - and a disagreement between them
//! is per-key and silent, which is the worst shape a defect can have here. One key stops
//! working on one platform and nothing errors, nothing logs, and no other test moves.
//!
//! Two things make it survivable, and this file is the second one:
//!
//! 1. **One generator.** `scripts/gen-keymap.ts` emits both tables from the SAME rows of
//!    Chromium's `dom_code_data` - a platform is a column index and a sentinel, nothing more.
//!    Two tables generated from one row cannot disagree about which physical key a DomCode is.
//! 2. **A test that reads the generated comments.** The tables carry their DomCode in a trailing
//!    comment, so the agreement in (1) is checkable at test time rather than promised by the
//!    shape of a script nobody runs.
//!
//! NOT gated on `target_os`: both tables are data, and a table that can only be tested on the
//! machine it targets is a table that never gets tested. This file runs on the developer's Mac
//! and on the Linux CI job alike.
//!
//! What this file CANNOT prove, said out loud rather than left to be discovered as "one key does
//! nothing": that GTK's `GdkEventKey.hardware_keycode` is in fact the X11/xkb keycode this table
//! is keyed on. GDK documents the field only as "the raw code of the key that was pressed or
//! released" (docs.gtk.org, read 2026-08-08) and names no convention. The table is right about
//! Chromium; whether it is right about GTK is a live tap on a real Linux machine.

use std::collections::BTreeMap;

use mind2t_vt_pty::key::Key;
use mind2t_vt_pty::keycode::{LINUX_KEYCODES, MACOS_KEYCODES, key_from_linux_keycode};

/// `DomCode -> GhosttyKey value`, read from the trailing comments of one generated table.
///
/// Parsing generated output is deliberate and is the same move the Swift parity test makes: the
/// comment is the only place the DomCode survives into the built artifact, and it is what the
/// two tables have to agree about.
fn dom_codes(table_name: &str) -> BTreeMap<String, u16> {
    let source = include_str!("../src/keycode.rs");
    let start = source
        .find(&format!("pub const {table_name}"))
        .unwrap_or_else(|| panic!("{table_name} is not in the generated file"));
    let body = &source[start..];
    let end = body.find("\n];").expect("the table is terminated");

    body[..end]
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (entry, dom) = line.split_once("// ")?;
            let (_, value) = entry.trim().trim_start_matches('(').trim_end_matches("),").split_once(',')?;
            Some((dom.trim().to_string(), value.trim().parse().ok()?))
        })
        .collect()
}

/// The invariant the per-platform route lives or dies on.
///
/// For every physical key both platforms have, the two tables must resolve it to the SAME key.
/// This is what "two tables that must never disagree" means, stated as an assertion.
#[test]
fn the_two_platform_tables_agree_about_every_key_they_share() {
    let mac = dom_codes("MACOS_KEYCODES");
    let linux = dom_codes("LINUX_KEYCODES");

    // A parser that found nothing would make the comparison below vacuous, and vacuous is what a
    // silent agreement failure looks like from outside.
    assert!(
        mac.len() > 100 && linux.len() > 100,
        "parsed {} macOS and {} Linux entries; the parser, not the tables, is wrong",
        mac.len(),
        linux.len()
    );

    let mut shared = 0;
    for (dom, mac_value) in &mac {
        let Some(linux_value) = linux.get(dom) else {
            continue;
        };
        shared += 1;
        assert_eq!(
            mac_value, linux_value,
            "{dom} is key {mac_value} on macOS and key {linux_value} on Linux - the tables were \
             not generated together; re-run scripts/gen-keymap.ts"
        );
    }

    // The overlap is most of a keyboard. If it collapsed, the two tables stopped describing the
    // same hardware and the assertion above would be passing on almost nothing.
    assert!(
        shared > 100,
        "only {shared} keys are shared between the two tables; the agreement check is hollow"
    );
}

/// The table is binary-searched, which is a silent-wrong-answer bug if it is ever unsorted.
#[test]
fn the_linux_table_is_sorted_because_the_lookup_binary_searches_it() {
    assert!(
        LINUX_KEYCODES.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "LINUX_KEYCODES is not strictly ascending; binary search would miss real keys"
    );
}

/// No entry may resolve to `Unidentified`.
///
/// A table entry pointing past `Key::ALL` produces exactly the same answer as no entry at all,
/// so a generator that emitted a wrong enum value would be invisible to every lookup test.
#[test]
fn every_linux_entry_names_a_real_key() {
    for (code, value) in LINUX_KEYCODES {
        let key = key_from_linux_keycode(*code);
        assert_ne!(
            key,
            Key::Unidentified,
            "xkb keycode {code} carries enum value {value}, which is not a key"
        );
    }
}

/// Spot values, and the control that stops a lookup returning one key for everything.
///
/// These are the xkb keycodes, which are the evdev scancodes plus 8 - the generator asserts that
/// relationship over every row of the source table, so these six also pin the COLUMN. Reading
/// the win column instead would still produce a sorted table of real keys and would fail here.
#[test]
fn known_linux_keys_resolve_and_unknown_ones_stay_unidentified() {
    assert_eq!(key_from_linux_keycode(38), Key::A); // evdev KEY_A 30
    assert_eq!(key_from_linux_keycode(36), Key::Enter); // evdev KEY_ENTER 28
    assert_eq!(key_from_linux_keycode(65), Key::Space); // evdev KEY_SPACE 57
    assert_eq!(key_from_linux_keycode(9), Key::Escape); // evdev KEY_ESC 1
    assert_eq!(key_from_linux_keycode(111), Key::ArrowUp); // evdev KEY_UP 103
    assert_eq!(key_from_linux_keycode(116), Key::ArrowDown); // evdev KEY_DOWN 108

    assert_eq!(key_from_linux_keycode(u16::MAX), Key::Unidentified);
    assert_eq!(key_from_linux_keycode(1000), Key::Unidentified);
}

/// The two tables are NOT the same numbers, which is the whole reason there are two of them.
///
/// **This test did NOT kill the mutant it was written for, and saying so is the point.** The
/// generator was made to emit the mac column into both tables, and this assertion stayed green:
/// the two columns use different absent-sentinels, so the duplicate came out one entry shorter
/// than the original and `assert_ne!` was satisfied by that single difference. What killed that
/// mutant was `known_linux_keys_resolve_and_unknown_ones_stay_unidentified`, which pins six
/// actual xkb numbers.
///
/// So this catches only an EXACT duplicate, which is a narrower thing than its name suggests.
/// It is kept because that narrow case is free to check, and it is documented because a test
/// credited with a kill it did not make is how a suite stops meaning anything.
#[test]
fn the_two_tables_are_keyed_on_different_hardware_codes() {
    let mac: BTreeMap<u16, u16> = MACOS_KEYCODES.iter().copied().collect();
    let linux: BTreeMap<u16, u16> = LINUX_KEYCODES.iter().copied().collect();
    assert_ne!(
        mac, linux,
        "the macOS and Linux keycode tables are identical; one column was generated twice"
    );
}
