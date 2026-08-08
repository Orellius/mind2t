//! The key encoder, measured against the oracle's own.
//!
//! `ghostty_key_encoder_encode` is the reference implementation of the transform
//! `crates/pty/src/key.rs` performs when a key event becomes pty bytes. Four
//! layers of comparison:
//!   1. the enum layout itself: our `Key` discriminants against the C constants,
//!      at section landmarks, so an ordering slip cannot silently remap every key;
//!   2. a full matrix over option sets x keys x actions x mods x text variants,
//!      byte-for-byte;
//!   3. composing (dead-key) events, which must be silent except plain modifiers;
//!   4. end-to-end derived state: the same mode byte streams fed to both
//!      terminals, the oracle's encoder configured via `setopt_from_terminal`,
//!      ours via the core's accessors -- pinning the mode -> option mapping
//!      differentially.
//! The control at the bottom proves the comparison can fail and the matrix floors
//! prove it is not vacuously silent.
//!
//! DECSTR is deliberately absent from the mode streams: our core resets
//! cursor/keypad modes on soft reset, but the oracle has NO `!` intermediate
//! dispatch at all (the corpus pins that divergence as
//! `decstr-soft-reset-diverges`), so a DECSTR stream would correctly fail here.
//! DECBKM (mode 67) is also absent: our core does not track it, so both sides
//! run with the default (backspace = 0x7f) in the derived-state layer.

use mind2t_vt_ghostty::sys;
use mind2t_vt_pty::key::{
    self, KEY_MODS_ALT, KEY_MODS_ALT_SIDE, KEY_MODS_CAPS_LOCK, KEY_MODS_CTRL,
    KEY_MODS_NUM_LOCK, KEY_MODS_SHIFT, KEY_MODS_SUPER, Key, KeyAction, KeyEvent, KeyMods,
    KeyOptions, OptionAsAlt,
};

/// A live oracle encoder handle.
struct OracleKeyEncoder {
    raw: sys::GhosttyKeyEncoder,
}

impl OracleKeyEncoder {
    fn new() -> Self {
        let mut raw: sys::GhosttyKeyEncoder = std::ptr::null_mut();
        let code = unsafe { sys::ghostty_key_encoder_new(std::ptr::null(), &mut raw) };
        assert_eq!(code, sys::GhosttyResult_GHOSTTY_SUCCESS, "encoder_new");
        Self { raw }
    }

    fn setopt(&self, option: sys::GhosttyKeyEncoderOption, value: *const std::ffi::c_void) {
        unsafe { sys::ghostty_key_encoder_setopt(self.raw, option, value) };
    }

    /// Mirrors every field of our `KeyOptions` onto the oracle.
    fn configure(&self, opts: &KeyOptions) {
        let bools: [(sys::GhosttyKeyEncoderOption, bool); 6] = [
            (
                sys::GhosttyKeyEncoderOption_GHOSTTY_KEY_ENCODER_OPT_CURSOR_KEY_APPLICATION,
                opts.cursor_key_application,
            ),
            (
                sys::GhosttyKeyEncoderOption_GHOSTTY_KEY_ENCODER_OPT_KEYPAD_KEY_APPLICATION,
                opts.keypad_key_application,
            ),
            (
                sys::GhosttyKeyEncoderOption_GHOSTTY_KEY_ENCODER_OPT_IGNORE_KEYPAD_WITH_NUMLOCK,
                opts.ignore_keypad_with_numlock,
            ),
            (
                sys::GhosttyKeyEncoderOption_GHOSTTY_KEY_ENCODER_OPT_ALT_ESC_PREFIX,
                opts.alt_esc_prefix,
            ),
            (
                sys::GhosttyKeyEncoderOption_GHOSTTY_KEY_ENCODER_OPT_MODIFY_OTHER_KEYS_STATE_2,
                opts.modify_other_keys_state_2,
            ),
            (
                sys::GhosttyKeyEncoderOption_GHOSTTY_KEY_ENCODER_OPT_BACKARROW_KEY_MODE,
                opts.backarrow_key_mode,
            ),
        ];
        for (option, value) in bools {
            self.setopt(option, (&raw const value).cast());
        }
        let flags: sys::GhosttyKittyKeyFlags = opts.kitty_flags;
        self.setopt(
            sys::GhosttyKeyEncoderOption_GHOSTTY_KEY_ENCODER_OPT_KITTY_FLAGS,
            (&raw const flags).cast(),
        );
        let option_as_alt: sys::GhosttyOptionAsAlt = match opts.macos_option_as_alt {
            OptionAsAlt::False => sys::GhosttyOptionAsAlt_GHOSTTY_OPTION_AS_ALT_FALSE,
            OptionAsAlt::True => sys::GhosttyOptionAsAlt_GHOSTTY_OPTION_AS_ALT_TRUE,
            OptionAsAlt::Left => sys::GhosttyOptionAsAlt_GHOSTTY_OPTION_AS_ALT_LEFT,
            OptionAsAlt::Right => sys::GhosttyOptionAsAlt_GHOSTTY_OPTION_AS_ALT_RIGHT,
        };
        self.setopt(
            sys::GhosttyKeyEncoderOption_GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT,
            (&raw const option_as_alt).cast(),
        );
    }

    /// From the oracle TERMINAL's state -- the derived-mapping comparison.
    fn set_from_terminal(&self, terminal: &mind2t_vt_ghostty::Terminal) {
        unsafe { sys::ghostty_key_encoder_setopt_from_terminal(self.raw, terminal.raw()) };
    }

    fn encode(&self, event: &KeyEvent<'_>) -> Vec<u8> {
        let mut raw_event: sys::GhosttyKeyEvent = std::ptr::null_mut();
        let code = unsafe { sys::ghostty_key_event_new(std::ptr::null(), &mut raw_event) };
        assert_eq!(code, sys::GhosttyResult_GHOSTTY_SUCCESS, "event_new");
        unsafe {
            sys::ghostty_key_event_set_action(
                raw_event,
                match event.action {
                    KeyAction::Release => sys::GhosttyKeyAction_GHOSTTY_KEY_ACTION_RELEASE,
                    KeyAction::Press => sys::GhosttyKeyAction_GHOSTTY_KEY_ACTION_PRESS,
                    KeyAction::Repeat => sys::GhosttyKeyAction_GHOSTTY_KEY_ACTION_REPEAT,
                },
            );
            sys::ghostty_key_event_set_key(raw_event, event.key as sys::GhosttyKey);
            sys::ghostty_key_event_set_mods(raw_event, event.mods);
            sys::ghostty_key_event_set_consumed_mods(raw_event, event.consumed_mods);
            sys::ghostty_key_event_set_composing(raw_event, event.composing);
            if event.utf8.is_empty() {
                sys::ghostty_key_event_set_utf8(raw_event, std::ptr::null(), 0);
            } else {
                sys::ghostty_key_event_set_utf8(
                    raw_event,
                    event.utf8.as_ptr().cast(),
                    event.utf8.len(),
                );
            }
            sys::ghostty_key_event_set_unshifted_codepoint(
                raw_event,
                event.unshifted_codepoint,
            );
        }

        // The sizing protocol from encoder.h: OUT_OF_SPACE hands back the
        // required capacity in `written`.
        let mut out = vec![0u8; 128];
        let mut written = 0usize;
        let mut code = unsafe {
            sys::ghostty_key_encoder_encode(
                self.raw,
                raw_event,
                out.as_mut_ptr().cast(),
                out.len(),
                &mut written,
            )
        };
        if code == sys::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
            out.resize(written, 0);
            code = unsafe {
                sys::ghostty_key_encoder_encode(
                    self.raw,
                    raw_event,
                    out.as_mut_ptr().cast(),
                    out.len(),
                    &mut written,
                )
            };
        }
        unsafe { sys::ghostty_key_event_free(raw_event) };
        assert_eq!(code, sys::GhosttyResult_GHOSTTY_SUCCESS, "encode");
        out.truncate(written);
        out
    }
}

impl Drop for OracleKeyEncoder {
    fn drop(&mut self) {
        unsafe { sys::ghostty_key_encoder_free(self.raw) };
    }
}

/// The default our app actually runs: fresh-terminal modes (1035 and 1036 on).
fn terminal_default_opts() -> KeyOptions {
    KeyOptions {
        cursor_key_application: false,
        keypad_key_application: false,
        ignore_keypad_with_numlock: true,
        alt_esc_prefix: true,
        modify_other_keys_state_2: false,
        kitty_flags: 0,
        macos_option_as_alt: OptionAsAlt::False,
        backarrow_key_mode: false,
    }
}

/// Encoder-fresh options: everything off, which is also `ghostty_key_encoder_new`'s
/// state (this is the "alt_esc_prefix off" arm of the matrix).
fn encoder_default_opts() -> KeyOptions {
    KeyOptions {
        alt_esc_prefix: false,
        ignore_keypad_with_numlock: false,
        ..terminal_default_opts()
    }
}

fn configs() -> Vec<(&'static str, KeyOptions)> {
    let t = terminal_default_opts();
    vec![
        ("encoder-defaults", encoder_default_opts()),
        ("terminal-defaults", t),
        ("cursor-app", KeyOptions { cursor_key_application: true, ..t }),
        // 1035 still on: application mode requested but numerical wins.
        ("keypad-app-ignored", KeyOptions { keypad_key_application: true, ..t }),
        (
            "keypad-app-honored",
            KeyOptions {
                keypad_key_application: true,
                ignore_keypad_with_numlock: false,
                ..t
            },
        ),
        ("modify-other-2", KeyOptions { modify_other_keys_state_2: true, ..t }),
        (
            "modify-other-2-option-alt",
            KeyOptions {
                modify_other_keys_state_2: true,
                macos_option_as_alt: OptionAsAlt::True,
                ..t
            },
        ),
        ("backarrow", KeyOptions { backarrow_key_mode: true, ..t }),
        ("option-as-alt-true", KeyOptions { macos_option_as_alt: OptionAsAlt::True, ..t }),
        ("option-as-alt-left", KeyOptions { macos_option_as_alt: OptionAsAlt::Left, ..t }),
        ("kitty-1", KeyOptions { kitty_flags: 1, ..t }),
        ("kitty-3", KeyOptions { kitty_flags: 1 | 2, ..t }),
        ("kitty-7", KeyOptions { kitty_flags: 1 | 2 | 4, ..t }),
        ("kitty-15", KeyOptions { kitty_flags: 1 | 2 | 4 | 8, ..t }),
        ("kitty-31", KeyOptions { kitty_flags: 31, ..t }),
        (
            "kitty-31-option-alt-true",
            KeyOptions {
                kitty_flags: 31,
                macos_option_as_alt: OptionAsAlt::True,
                ..t
            },
        ),
    ]
}

const ACTIONS: &[KeyAction] = &[KeyAction::Press, KeyAction::Release, KeyAction::Repeat];

const MODS_SET: &[KeyMods] = &[
    0,
    KEY_MODS_SHIFT,
    KEY_MODS_CTRL,
    KEY_MODS_ALT,
    KEY_MODS_SUPER,
    KEY_MODS_CTRL | KEY_MODS_ALT,
    KEY_MODS_SHIFT | KEY_MODS_CTRL | KEY_MODS_ALT,
    KEY_MODS_SHIFT | KEY_MODS_CTRL | KEY_MODS_ALT | KEY_MODS_SUPER,
    KEY_MODS_CAPS_LOCK,
    KEY_MODS_NUM_LOCK,
    // Right option held: distinguishes the Left/Right option-as-alt gates.
    KEY_MODS_ALT | KEY_MODS_ALT_SIDE,
];

/// The text variants a key event can realistically carry: none; the layout's
/// codepoint (shift-consumed uppercase when shift is down); macOS's control-byte
/// UTF-8 for the functional keys; and the layout edge cases (a translated option
/// character, a Cyrillic layout, a multi-codepoint IME commit).
fn utf8_variants(key: Key, mods: KeyMods) -> Vec<(String, u32, KeyMods)> {
    let mut variants: Vec<(String, u32, KeyMods)> = vec![(String::new(), 0, 0)];
    if let Some(cp) = key.codepoint() {
        let ch = char::from_u32(cp).expect("codepoint table holds scalar values");
        variants.push((ch.to_string(), cp, 0));
        if mods & KEY_MODS_SHIFT != 0 && ch.is_ascii_lowercase() {
            variants.push((ch.to_ascii_uppercase().to_string(), cp, KEY_MODS_SHIFT));
        }
    }
    match key {
        Key::Enter => {
            variants.push(("\r".to_string(), 13, 0));
            variants.push(("x".to_string(), 0, 0)); // IME commit through enter
        }
        Key::Escape => variants.push(("\x1b".to_string(), 27, 0)),
        Key::Backspace => {
            variants.push(("\x7f".to_string(), 127, 0));
            variants.push(("x".to_string(), 0, 0)); // preedit edit
        }
        Key::C => {
            // macOS option translation: one codepoint, TWO bytes -- splits the
            // byte-length and codepoint-count paths.
            variants.push(("ç".to_string(), 'c' as u32, 0));
            // Cyrillic layout: no ASCII text, the physical key must carry ctrl+c.
            variants.push(("с".to_string(), 'с' as u32, 0));
        }
        Key::Q => variants.push(("ab".to_string(), 'q' as u32, 0)),
        _ => {}
    }
    variants
}

/// Layer 1: the enum layout. One landmark per section of event.h; a miscounted
/// section shifts every landmark after it.
#[test]
fn key_discriminants_match_the_c_enum() {
    let landmarks: &[(Key, sys::GhosttyKey)] = &[
        (Key::Unidentified, sys::GhosttyKey_GHOSTTY_KEY_UNIDENTIFIED),
        (Key::Backquote, sys::GhosttyKey_GHOSTTY_KEY_BACKQUOTE),
        (Key::Slash, sys::GhosttyKey_GHOSTTY_KEY_SLASH),
        (Key::AltLeft, sys::GhosttyKey_GHOSTTY_KEY_ALT_LEFT),
        (Key::NonConvert, sys::GhosttyKey_GHOSTTY_KEY_NON_CONVERT),
        (Key::Delete, sys::GhosttyKey_GHOSTTY_KEY_DELETE),
        (Key::PageUp, sys::GhosttyKey_GHOSTTY_KEY_PAGE_UP),
        (Key::ArrowUp, sys::GhosttyKey_GHOSTTY_KEY_ARROW_UP),
        (Key::NumLock, sys::GhosttyKey_GHOSTTY_KEY_NUM_LOCK),
        (Key::NumpadSubtract, sys::GhosttyKey_GHOSTTY_KEY_NUMPAD_SUBTRACT),
        (Key::NumpadSeparator, sys::GhosttyKey_GHOSTTY_KEY_NUMPAD_SEPARATOR),
        (Key::NumpadPageDown, sys::GhosttyKey_GHOSTTY_KEY_NUMPAD_PAGE_DOWN),
        (Key::Escape, sys::GhosttyKey_GHOSTTY_KEY_ESCAPE),
        (Key::F25, sys::GhosttyKey_GHOSTTY_KEY_F25),
        (Key::Pause, sys::GhosttyKey_GHOSTTY_KEY_PAUSE),
        (Key::BrowserBack, sys::GhosttyKey_GHOSTTY_KEY_BROWSER_BACK),
        (Key::WakeUp, sys::GhosttyKey_GHOSTTY_KEY_WAKE_UP),
        (Key::Copy, sys::GhosttyKey_GHOSTTY_KEY_COPY),
        (Key::Paste, sys::GhosttyKey_GHOSTTY_KEY_PASTE),
    ];
    for &(ours, c_value) in landmarks {
        assert_eq!(ours as u32, c_value, "{ours:?}");
    }
    // And ALL is the full enum: its length must reach exactly one past the
    // last C value.
    assert_eq!(Key::ALL.len() as u32, sys::GhosttyKey_GHOSTTY_KEY_PASTE + 1);
}

/// Layer 2: the matrix.
#[test]
fn every_matrix_case_agrees_byte_for_byte() {
    let mut compared = 0usize;
    let mut nonempty = 0usize;
    for (name, opts) in configs() {
        let oracle = OracleKeyEncoder::new();
        oracle.configure(&opts);
        for &key in Key::ALL {
            for &action in ACTIONS {
                for &mods in MODS_SET {
                    for (utf8, unshifted, consumed) in utf8_variants(key, mods) {
                        let event = KeyEvent {
                            action,
                            key,
                            mods,
                            consumed_mods: consumed,
                            composing: false,
                            utf8: &utf8,
                            unshifted_codepoint: unshifted,
                        };
                        let expected = oracle.encode(&event);
                        let got = key::encode(&event, &opts);
                        assert_eq!(
                            got,
                            expected,
                            "config={name} key={key:?} action={action:?} mods={mods:#06x} \
                             utf8={:?} unshifted={unshifted} consumed={consumed:#06x}",
                            utf8.escape_debug().to_string(),
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
    // A matrix that never produced bytes would "agree" forever; demand real output.
    // Measured 2026-07-30: 135,216 compared, 55,833 nonempty -- the floors have
    // headroom for matrix trims but die on a vacuous run.
    assert!(compared > 100_000, "{compared}");
    assert!(nonempty > 40_000, "only {nonempty} of {compared} cases produced bytes");
}

/// Layer 3: composing (dead-key) events -- silent everywhere except plain
/// modifier keys under kitty report-all.
#[test]
fn composing_events_agree_everywhere() {
    let composing_configs = [
        ("terminal-defaults", terminal_default_opts()),
        ("kitty-1", KeyOptions { kitty_flags: 1, ..terminal_default_opts() }),
        ("kitty-31", KeyOptions { kitty_flags: 31, ..terminal_default_opts() }),
    ];
    let mut nonempty = 0usize;
    for (name, opts) in composing_configs {
        let oracle = OracleKeyEncoder::new();
        oracle.configure(&opts);
        for &key in Key::ALL {
            for &mods in &[0, KEY_MODS_SHIFT, KEY_MODS_ALT] {
                for (utf8, unshifted) in [("", 0u32), ("n", 'n' as u32)] {
                    let event = KeyEvent {
                        action: KeyAction::Press,
                        key,
                        mods,
                        consumed_mods: 0,
                        composing: true,
                        utf8,
                        unshifted_codepoint: unshifted,
                    };
                    let expected = oracle.encode(&event);
                    let got = key::encode(&event, &opts);
                    assert_eq!(
                        got, expected,
                        "config={name} key={key:?} mods={mods:#06x} utf8={utf8:?}"
                    );
                    if !expected.is_empty() {
                        nonempty += 1;
                    }
                }
            }
        }
    }
    // The modifier keys under kitty-31 must have actually encoded; a suite
    // where everything is silent proves only that silence agrees.
    assert!(nonempty >= 8, "only {nonempty} composing cases produced bytes");
}

/// Layer 4: the derived option state. The same mode BYTES drive both terminals;
/// the oracle's encoder is configured by `setopt_from_terminal`, ours by the
/// core's accessors. This pins mode 1/66/1035/1036, modifyOtherKeys and the
/// kitty flag stack (including alt-screen isolation and RIS) differentially.
#[test]
fn terminal_derived_state_encodes_identically_after_every_mode_sequence() {
    let sequences: &[&str] = &[
        "",
        "\x1b[?1h",
        "\x1b[?1h\x1b[?1l",
        "\x1b=", // DECKPAM spells mode 66 without CSI
        "\x1b[?66h",
        "\x1b[?66h\x1b[?1035l", // application keypad actually honored
        "\x1b[?1035l",
        "\x1b[?1036l",
        "\x1b[>4;2m",
        "\x1b[>4;1m", // state 1 is NOT state 2
        "\x1b[>4;2m\x1b[>4;0m",
        "\x1b[>1u",
        "\x1b[>31u",
        "\x1b[>1u\x1b[>2u\x1b[<1u", // push, push, pop
        "\x1b[=3;1u",               // set
        "\x1b[=2;2u",               // or
        "\x1b[?1049h\x1b[>31u",     // the alternate screen has its own stack
        "\x1b[?1049h\x1b[>31u\x1b[?1049l", // ...which vanishes on exit
        "\x1b[>31u\x1bc",           // RIS
        "\x1b[?1h\x1b=\x1b[?1035l\x1b[>4;2m\x1b[>5u", // kitty wins over everything
    ];
    let probes: &[(Key, KeyMods, &str, u32)] = &[
        (Key::ArrowUp, 0, "", 0),
        (Key::A, KEY_MODS_CTRL, "a", 'a' as u32),
        (Key::Enter, 0, "\r", 13),
        (Key::F5, 0, "", 0),
        (Key::Numpad5, 0, "5", '5' as u32),
        (Key::Backspace, 0, "\x7f", 127),
        (Key::C, KEY_MODS_ALT, "ç", 'c' as u32),
    ];
    let mut agreed_nonempty = 0usize;
    for bytes in sequences {
        let mut oracle_terminal =
            mind2t_vt_ghostty::Terminal::new(20, 5).expect("oracle terminal");
        oracle_terminal.write(bytes.as_bytes());
        let oracle = OracleKeyEncoder::new();
        oracle.set_from_terminal(&oracle_terminal);

        let mut terminal = mind2t_vt_core::terminal::Terminal::new(20, 5);
        terminal.write(bytes.as_bytes());
        // `setopt_from_terminal` cannot know option-as-alt (resets it to false)
        // and our core does not track DECBKM -- both sides run the defaults.
        let opts = KeyOptions {
            cursor_key_application: terminal.cursor_keys(),
            keypad_key_application: terminal.keypad_keys(),
            ignore_keypad_with_numlock: terminal.ignore_keypad_with_numlock(),
            alt_esc_prefix: terminal.alt_esc_prefix(),
            modify_other_keys_state_2: terminal.modify_other_keys_2(),
            kitty_flags: terminal.kitty_key_flags(),
            macos_option_as_alt: OptionAsAlt::False,
            backarrow_key_mode: false,
        };

        for &(key, mods, utf8, unshifted) in probes {
            let event = KeyEvent {
                action: KeyAction::Press,
                key,
                mods,
                consumed_mods: 0,
                composing: false,
                utf8,
                unshifted_codepoint: unshifted,
            };
            let expected = oracle.encode(&event);
            let got = key::encode(&event, &opts);
            assert_eq!(
                got,
                expected,
                "sequence={:?} key={key:?}",
                bytes.escape_debug().to_string(),
            );
            if !expected.is_empty() {
                agreed_nonempty += 1;
            }
        }
    }
    assert!(agreed_nonempty >= 60, "only {agreed_nonempty} probes produced bytes");
}

/// The comparison can fail: a single deliberate misencoding (ctrl dropped) must
/// disagree with the oracle on a ctrl+a case. Guards a harness whose equality
/// assert never sees real bytes.
#[test]
fn a_wrong_encoder_is_caught_by_the_matrix() {
    let opts = terminal_default_opts();
    let oracle = OracleKeyEncoder::new();
    oracle.configure(&opts);
    let event = KeyEvent {
        action: KeyAction::Press,
        key: Key::A,
        mods: KEY_MODS_CTRL,
        consumed_mods: 0,
        composing: false,
        utf8: "a",
        unshifted_codepoint: 'a' as u32,
    };
    let expected = oracle.encode(&event);
    assert!(!expected.is_empty());
    let mut broken = event.clone();
    broken.mods = 0;
    let got = key::encode(&broken, &opts);
    assert_ne!(got, expected, "dropping ctrl must be visible");
}
