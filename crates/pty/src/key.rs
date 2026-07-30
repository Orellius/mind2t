//! Purpose: turn a keyboard event into the bytes a terminal child reads.
//! Public surface: `KeyAction`, `Key`, `KeyMods` (+ bit constants), `KeyEvent`,
//!   `OptionAsAlt`, `KeyOptions`, `encode`.
//! Why this file: key encoding is an INPUT transform, exactly like paste and mouse --
//!   the core TRACKS the modes (DECCKM, DECKPAM, 1035/1036, modifyOtherKeys, the
//!   kitty flag stack) and encodes nothing, so the transform lives with the pty. It
//!   is pure so the differential harness can compare it byte-for-byte against the
//!   oracle's `ghostty_key_encoder_encode`.
//! NOT responsible for: tracking modes (the core), translating platform key events
//!   into `KeyEvent` (the host window does layout/IME work), or writing to the pty.
//! Test strategy: `crates/ghostty/tests/key.rs` drives the oracle's key-encoder ABI
//!   over a matrix of keys x actions x mods x option sets and demands byte equality;
//!   unit tests here pin the rules that motivated tricky branches. Every rule is
//!   measured from `src/input/key_encode.zig` / `function_keys.zig` / `kitty.zig`
//!   (v1.3.2) on 2026-07-30.
//!
//! PLATFORM NOTE: the oracle archive is compiled for macOS, so its `builtin.os.tag`
//! branches (option-as-alt gating, "super never encodes text") are BAKED IN. This
//! port mirrors the macOS behavior unconditionally -- which is also the only
//! platform this host runs on. Porting to another OS means revisiting every branch
//! commented "macOS".

/// Discriminants match `GHOSTTY_KEY_ACTION_*` (event.h: release 0, press 1, repeat 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Release,
    Press,
    Repeat,
}

/// Defines `Key` mirroring the `GhosttyKey` enum in event.h EXACTLY, in declaration
/// order, so `key as u32` equals the C value -- and `Key::ALL` from the same list,
/// so the two can never drift apart.
macro_rules! keys {
    ($($name:ident),* $(,)?) => {
        /// Physical key codes, W3C UI Events order (event.h `GhosttyKey`).
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Key { $($name),* }

        impl Key {
            /// Every key, in C declaration order. The differential matrix iterates
            /// this; `Key::ALL[i] as u32 == i` by construction.
            pub const ALL: &'static [Key] = &[$(Key::$name),*];
        }
    };
}

keys! {
    Unidentified,
    // Writing System Keys (W3C 3.1.1)
    Backquote, Backslash, BracketLeft, BracketRight, Comma,
    Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9,
    Equal, IntlBackslash, IntlRo, IntlYen,
    A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z,
    Minus, Period, Quote, Semicolon, Slash,
    // Functional Keys (W3C 3.1.2)
    AltLeft, AltRight, Backspace, CapsLock, ContextMenu, ControlLeft, ControlRight,
    Enter, MetaLeft, MetaRight, ShiftLeft, ShiftRight, Space, Tab, Convert, KanaMode,
    NonConvert,
    // Control Pad Section (W3C 3.2)
    Delete, End, Help, Home, Insert, PageDown, PageUp,
    // Arrow Pad Section (W3C 3.3)
    ArrowDown, ArrowLeft, ArrowRight, ArrowUp,
    // Numpad Section (W3C 3.4)
    NumLock,
    Numpad0, Numpad1, Numpad2, Numpad3, Numpad4, Numpad5, Numpad6, Numpad7, Numpad8,
    Numpad9,
    NumpadAdd, NumpadBackspace, NumpadClear, NumpadClearEntry, NumpadComma,
    NumpadDecimal, NumpadDivide, NumpadEnter, NumpadEqual, NumpadMemoryAdd,
    NumpadMemoryClear, NumpadMemoryRecall, NumpadMemoryStore, NumpadMemorySubtract,
    NumpadMultiply, NumpadParenLeft, NumpadParenRight, NumpadSubtract,
    NumpadSeparator, NumpadUp, NumpadDown, NumpadRight, NumpadLeft, NumpadBegin,
    NumpadHome, NumpadEnd, NumpadInsert, NumpadDelete, NumpadPageUp, NumpadPageDown,
    // Function Section (W3C 3.5)
    Escape,
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12, F13, F14, F15, F16, F17, F18,
    F19, F20, F21, F22, F23, F24, F25,
    Fn, FnLock, PrintScreen, ScrollLock, Pause,
    // Media Keys (W3C 3.6)
    BrowserBack, BrowserFavorites, BrowserForward, BrowserHome, BrowserRefresh,
    BrowserSearch, BrowserStop, Eject, LaunchApp1, LaunchApp2, LaunchMail,
    MediaPlayPause, MediaSelect, MediaStop, MediaTrackNext, MediaTrackPrevious,
    Power, Sleep, AudioVolumeDown, AudioVolumeMute, AudioVolumeUp, WakeUp,
    // Legacy, Non-standard, and Special Keys (W3C 3.7)
    Copy, Cut, Paste,
}

impl Key {
    /// The codepoint this physical key produces on a US layout, or `None` for a key
    /// that is not printable. Ported from `key.zig`'s `codepoint_map` (each key's
    /// FIRST entry there, which is what its `codepoint()` returns).
    pub fn codepoint(self) -> Option<u32> {
        Some(match self {
            Key::A => 'a',
            Key::B => 'b',
            Key::C => 'c',
            Key::D => 'd',
            Key::E => 'e',
            Key::F => 'f',
            Key::G => 'g',
            Key::H => 'h',
            Key::I => 'i',
            Key::J => 'j',
            Key::K => 'k',
            Key::L => 'l',
            Key::M => 'm',
            Key::N => 'n',
            Key::O => 'o',
            Key::P => 'p',
            Key::Q => 'q',
            Key::R => 'r',
            Key::S => 's',
            Key::T => 't',
            Key::U => 'u',
            Key::V => 'v',
            Key::W => 'w',
            Key::X => 'x',
            Key::Y => 'y',
            Key::Z => 'z',
            Key::Digit0 => '0',
            Key::Digit1 => '1',
            Key::Digit2 => '2',
            Key::Digit3 => '3',
            Key::Digit4 => '4',
            Key::Digit5 => '5',
            Key::Digit6 => '6',
            Key::Digit7 => '7',
            Key::Digit8 => '8',
            Key::Digit9 => '9',
            Key::Semicolon => ';',
            Key::Space => ' ',
            Key::Quote => '\'',
            Key::Comma => ',',
            Key::Backquote => '`',
            Key::Period => '.',
            Key::Slash => '/',
            Key::Minus => '-',
            Key::Equal => '=',
            Key::BracketLeft => '[',
            Key::BracketRight => ']',
            Key::Backslash => '\\',
            Key::Tab => '\t',
            Key::Numpad0 => '0',
            Key::Numpad1 => '1',
            Key::Numpad2 => '2',
            Key::Numpad3 => '3',
            Key::Numpad4 => '4',
            Key::Numpad5 => '5',
            Key::Numpad6 => '6',
            Key::Numpad7 => '7',
            Key::Numpad8 => '8',
            Key::Numpad9 => '9',
            Key::NumpadDecimal => '.',
            Key::NumpadDivide => '/',
            Key::NumpadMultiply => '*',
            Key::NumpadSubtract => '-',
            Key::NumpadAdd => '+',
            Key::NumpadEqual => '=',
            _ => return None,
        } as u32)
    }
}

/// Modifier bitmask, bit-compatible with `GhosttyMods` (event.h: shift 1<<0,
/// ctrl 1<<1, alt 1<<2, super 1<<3, caps_lock 1<<4, num_lock 1<<5, plus the side
/// bits, where a set side bit means the RIGHT key of that modifier).
pub type KeyMods = u16;

pub const KEY_MODS_SHIFT: KeyMods = 1 << 0;
pub const KEY_MODS_CTRL: KeyMods = 1 << 1;
pub const KEY_MODS_ALT: KeyMods = 1 << 2;
pub const KEY_MODS_SUPER: KeyMods = 1 << 3;
pub const KEY_MODS_CAPS_LOCK: KeyMods = 1 << 4;
pub const KEY_MODS_NUM_LOCK: KeyMods = 1 << 5;
pub const KEY_MODS_SHIFT_SIDE: KeyMods = 1 << 6;
pub const KEY_MODS_CTRL_SIDE: KeyMods = 1 << 7;
pub const KEY_MODS_ALT_SIDE: KeyMods = 1 << 8;
pub const KEY_MODS_SUPER_SIDE: KeyMods = 1 << 9;

/// The four bindable modifiers -- what `key_mods.zig`'s `binding()` keeps.
const MODS_BINDING: KeyMods = KEY_MODS_SHIFT | KEY_MODS_CTRL | KEY_MODS_ALT | KEY_MODS_SUPER;

#[derive(Debug, Clone)]
pub struct KeyEvent<'a> {
    pub action: KeyAction,
    pub key: Key,
    pub mods: KeyMods,
    /// Mods consumed to produce `utf8` (e.g. the shift in shift+a -> "A"). Only
    /// meaningful when `utf8` is non-empty, exactly as in `key.zig`.
    pub consumed_mods: KeyMods,
    /// Mid-dead-key composition: almost nothing encodes.
    pub composing: bool,
    /// The layout-produced text. Empty = the key produced none.
    pub utf8: &'a str,
    /// The codepoint this key yields with shift ignored (shift+a -> 'a'); 0 = none.
    pub unshifted_codepoint: u32,
}

/// Discriminants match `GHOSTTY_OPTION_AS_ALT_*` (encoder.h / config.zig).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionAsAlt {
    False,
    True,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy)]
pub struct KeyOptions {
    /// DECCKM (mode 1).
    pub cursor_key_application: bool,
    /// DECKPAM (mode 66).
    pub keypad_key_application: bool,
    /// Mode 1035: keypad application encoding yields to numlock.
    pub ignore_keypad_with_numlock: bool,
    /// Mode 1036: alt prefixes an ESC in legacy encoding.
    pub alt_esc_prefix: bool,
    /// xterm modifyOtherKeys state 2.
    pub modify_other_keys_state_2: bool,
    /// Kitty keyboard flags, wire layout (`ruuah_vt_core::kitty_keys` bit names).
    /// Only the low 5 bits are meaningful; the rest are masked off exactly as the
    /// oracle's C setopt truncates to a u5.
    pub kitty_flags: u8,
    pub macos_option_as_alt: OptionAsAlt,
    /// DECBKM: backspace emits 0x08 instead of 0x7f.
    pub backarrow_key_mode: bool,
}

// Kitty flag bits, mirrored from `ruuah_vt_core::kitty_keys` (one wire format).
// Disambiguate has no branch of its own -- ANY set flag selects the kitty path.
#[allow(dead_code)]
const KITTY_DISAMBIGUATE: u8 = 1 << 0;
const KITTY_REPORT_EVENTS: u8 = 1 << 1;
const KITTY_REPORT_ALTERNATES: u8 = 1 << 2;
const KITTY_REPORT_ALL: u8 = 1 << 3;
const KITTY_REPORT_ASSOCIATED: u8 = 1 << 4;

/// Encodes one key event. Empty vec = the event produces no bytes (a common,
/// correct outcome, not an error). Mirrors `key_encode.zig`'s `encode`: ANY kitty
/// flag routes to the kitty protocol, else legacy.
pub fn encode(event: &KeyEvent<'_>, opts: &KeyOptions) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    if opts.kitty_flags & 0x1F != 0 {
        kitty(&mut out, event, opts);
    } else {
        legacy(&mut out, event, opts);
    }
    out
}

/// `effectiveMods` (key.zig): with text, the mods consumed to make it don't count.
fn effective_mods(event: &KeyEvent<'_>) -> KeyMods {
    if event.utf8.is_empty() { event.mods } else { event.mods & !event.consumed_mods }
}

/// ASCII control, the libc definition (`isControl` in key_encode.zig).
fn is_control(cp: u32) -> bool {
    cp < 0x20 || cp == 0x7F
}

/// A single-BYTE control string (`isControlUtf8`): multi-byte strings are not.
fn is_control_utf8(s: &str) -> bool {
    s.len() == 1 && is_control(u32::from(s.as_bytes()[0]))
}

/// The string's one codepoint, or `None` if it has zero or several. Both the
/// modifyOtherKeys and CSIu paths refuse multi-codepoint text.
fn single_codepoint(s: &str) -> Option<u32> {
    let mut it = s.chars();
    let cp = it.next()?;
    if it.next().is_some() { None } else { Some(cp as u32) }
}

/// The xterm modifier parameter for a non-empty binding mask: 1 + shift(1) +
/// alt(2) + ctrl(4) + super(8). Produces exactly the `function_keys.modifiers`
/// list (index+2); `None` for no mods, where the sequence takes its plain form.
fn xterm_mod_code(mods: KeyMods) -> Option<u32> {
    let mods = mods & MODS_BINDING;
    if mods == 0 {
        return None;
    }
    let mut code: u32 = 1;
    if mods & KEY_MODS_SHIFT != 0 {
        code += 1;
    }
    if mods & KEY_MODS_ALT != 0 {
        code += 2;
    }
    if mods & KEY_MODS_CTRL != 0 {
        code += 4;
    }
    if mods & KEY_MODS_SUPER != 0 {
        code += 8;
    }
    Some(code)
}

// ---------------------------------------------------------------------------
// Legacy encoding (traditional terminals + xterm modifyOtherKeys + fixterms).
// ---------------------------------------------------------------------------

/// `legacy()` in key_encode.zig, same decision order: PC-style function keys,
/// C0 control sequences, the no-text alt prefix, modifyOtherKeys, ctrl CSIu,
/// the with-text alt prefix, the macOS super gate, then the text itself.
fn legacy(out: &mut Vec<u8>, event: &KeyEvent<'_>, opts: &KeyOptions) {
    let all_mods = event.mods;
    let binding_mods = effective_mods(event) & MODS_BINDING;

    // Legacy only encodes press/repeat, and never mid-composition.
    if !matches!(event.action, KeyAction::Press | KeyAction::Repeat) {
        return;
    }
    if event.composing {
        return;
    }

    if let Some(seq) = pc_style_function_key(event.key, all_mods, opts) {
        match pc_style_utf8_exception(event) {
            PcException::UsePc => {
                out.extend_from_slice(&seq);
                return;
            }
            PcException::Silent => return,
            PcException::CommitText => {}
        }
    }

    // Ctrl-derived C0 byte; the alt-ESC prefix on C0 is unconditional on the
    // 1036 option (the oracle prefixes on binding alt alone).
    if let Some(byte) =
        ctrl_seq(event.key, event.utf8, event.unshifted_codepoint, all_mods)
    {
        if binding_mods & KEY_MODS_ALT != 0 {
            out.push(0x1B);
        }
        out.push(byte);
        return;
    }

    // No text: the only remaining possibility is alt-prefixing the unshifted key.
    if event.utf8.is_empty() {
        if let Some(byte) = legacy_alt_prefix(event, binding_mods, opts) {
            out.push(0x1B);
            out.push(byte);
        }
        return;
    }

    if opts.modify_other_keys_state_2
        && let Some(bytes) = modify_other_keys(event, opts)
    {
        out.extend_from_slice(&bytes);
        return;
    }

    if all_mods & KEY_MODS_CTRL != 0
        && let Some(bytes) = csi_u(event)
    {
        out.extend_from_slice(&bytes);
        return;
    }

    if let Some(byte) = legacy_alt_prefix(event, binding_mods, opts) {
        out.push(0x1B);
        out.push(byte);
        return;
    }

    // macOS: command+key never encodes text (Terminal.app, iTerm2, TextEdit agree).
    if all_mods & KEY_MODS_SUPER != 0 {
        return;
    }

    out.extend_from_slice(event.utf8.as_bytes());
}

/// What a matched PC-style sequence does when the event ALSO carries text: dead
/// keys give escape/enter/backspace IME meanings (escape clears, enter commits,
/// backspace edits preedit), so committed text beats the function-key form.
enum PcException {
    UsePc,
    Silent,
    CommitText,
}

fn pc_style_utf8_exception(event: &KeyEvent<'_>) -> PcException {
    if event.utf8.is_empty() {
        return PcException::UsePc;
    }
    match event.key {
        Key::Backspace | Key::Enter | Key::Escape => {
            // macOS sends control characters as UTF-8 (plain enter is "\r");
            // those are NOT committed IME text and take the normal path.
            if is_control_utf8(event.utf8) {
                PcException::UsePc
            } else if event.key == Key::Backspace {
                // Backspace edited the preedit; nothing reaches the pty.
                PcException::Silent
            } else {
                PcException::CommitText
            }
        }
        _ => PcException::UsePc,
    }
}

/// `legacyAltPrefix`: alt (still effective after translation) + mode 1036 turns a
/// one-byte key into ESC+byte. On macOS, only when option acts as alt.
fn legacy_alt_prefix(
    event: &KeyEvent<'_>,
    binding_mods: KeyMods,
    opts: &KeyOptions,
) -> Option<u8> {
    if binding_mods & KEY_MODS_ALT == 0 || !opts.alt_esc_prefix {
        return None;
    }
    // macOS: option normally does a unicode translation instead of acting as alt.
    match opts.macos_option_as_alt {
        OptionAsAlt::False => return None,
        OptionAsAlt::Left if event.mods & KEY_MODS_ALT_SIDE != 0 => return None,
        OptionAsAlt::Right if event.mods & KEY_MODS_ALT_SIDE == 0 => return None,
        _ => {}
    }
    let utf8 = event.utf8.as_bytes();
    if utf8.len() == 1 {
        return Some(utf8[0]);
    }
    if event.unshifted_codepoint > 0 && event.unshifted_codepoint <= 0xFF {
        return Some(event.unshifted_codepoint as u8);
    }
    None
}

/// modifyOtherKeys state 2: `CSI 27 ; mod ; codepoint ~` for single-codepoint text
/// with a qualifying modifier set (xterm's `ModifyOtherKeys` predicate).
fn modify_other_keys(event: &KeyEvent<'_>, opts: &KeyOptions) -> Option<Vec<u8>> {
    let cp = single_codepoint(event.utf8)?;

    // The encoded mods are the binding mods, minus alt when macOS option is NOT
    // acting as alt (the option key did translation, not modification).
    let mut mods = event.mods & MODS_BINDING;
    let alt_is_alt = match opts.macos_option_as_alt {
        OptionAsAlt::False => false,
        OptionAsAlt::True => true,
        OptionAsAlt::Left => event.mods & KEY_MODS_ALT_SIDE == 0,
        OptionAsAlt::Right => event.mods & KEY_MODS_ALT_SIDE != 0,
    };
    if !alt_is_alt {
        mods &= !KEY_MODS_ALT;
    }

    // xterm's predicate: control-range input always; any non-shift mod; or
    // shift-only on space.
    let should_modify = (0x40..=0x7F).contains(&cp)
        || mods & !KEY_MODS_SHIFT != 0
        || cp == u32::from(' ');
    if !should_modify {
        return None;
    }

    // Empty mods have no entry in the modifier list -- fall through, no sequence.
    let code = xterm_mod_code(mods)?;
    Some(format!("\x1B[27;{code};{cp}~").into_bytes())
}

/// The fixterms CSIu fallback for ctrl + single-codepoint text that no C0 mapping
/// claimed. Kitty's divergence is kept: shifted A-Z is sent lowercase with the
/// shift mod, so programs can tell ctrl+m from ctrl+shift+m.
fn csi_u(event: &KeyEvent<'_>) -> Option<Vec<u8>> {
    // CSIu mods pack shift(1), alt(2), ctrl(4) -- NOT the KeyMods bit order.
    let mut cp = single_codepoint(event.utf8)?;
    let mut shift = event.mods & KEY_MODS_SHIFT != 0;
    let alt = event.mods & KEY_MODS_ALT != 0;

    if (u32::from('A')..=u32::from('Z')).contains(&cp) && shift {
        cp += 32; // toLower
    }
    // Shift is reported only when it did NOT produce the character (fixterms).
    if event.unshifted_codepoint != cp {
        shift = false;
    }

    let seq: u32 = 1 + u32::from(shift) + 2 * u32::from(alt) + 4;
    Some(format!("\x1B[{cp};{seq}u").into_bytes())
}

/// `ctrlSeq`: the C0 byte for ctrl+<char>, or `None` when the event should not
/// collapse to one (extra mods, shifted letters, unmappable keys).
fn ctrl_seq(key: Key, utf8: &str, unshifted_codepoint: u32, mods: KeyMods) -> Option<u8> {
    if mods & KEY_MODS_CTRL == 0 {
        return None;
    }
    // Alt never changes WHETHER a C0 fires; the ESC prefix is layered on after.
    let mut unset = (mods & MODS_BINDING) & !KEY_MODS_ALT;

    let bytes = utf8.as_bytes();
    let mut ch: u8 = if bytes.len() == 1 {
        bytes[0]
    } else if let Some(cp) = key.codepoint()
        && cp <= 0xFF
    {
        // Cyrillic-layout support: a physical C key with no ASCII text still
        // encodes ctrl+c -- but only under EXACTLY ctrl, shift would need the
        // layout to resolve and goes to CSIu instead.
        if unset != KEY_MODS_CTRL {
            return None;
        }
        cp as u8
    } else {
        return None;
    };

    // ctrl+shift+- must make 0x1F (emacs): shift is spent obtaining the char for
    // anything outside A-Z. Fixterms' one awkward exception: '@' keeps its shift.
    if unset & KEY_MODS_SHIFT != 0 && !ch.is_ascii_uppercase() && ch != b'@' {
        unset &= !KEY_MODS_SHIFT;
    }
    // Caps lock produced an uppercase letter: lowercase it via the unshifted
    // codepoint (shifted letters keep their shift mod and bail below).
    if ch.is_ascii_uppercase() && unshifted_codepoint > 0 && unshifted_codepoint <= 0xFF {
        ch = unshifted_codepoint as u8;
    }

    if unset != KEY_MODS_CTRL {
        return None;
    }
    ctrl_c0(ch)
}

/// Kitty's ctrl table, repeated verbatim (the oracle repeats it from Kitty too).
/// 'i', 'm' and '[' are deliberately ABSENT per fixterms: they collide with tab,
/// enter and escape, so they encode as CSIu instead.
fn ctrl_c0(ch: u8) -> Option<u8> {
    Some(match ch {
        b' ' => 0,
        b'/' => 31,
        b'0' => 48,
        b'1' => 49,
        b'2' => 0,
        b'3' => 27,
        b'4' => 28,
        b'5' => 29,
        b'6' => 30,
        b'7' => 31,
        b'8' => 127,
        b'9' => 57,
        b'?' => 127,
        b'@' => 0,
        b'\\' => 28,
        b']' => 29,
        b'^' => 30,
        b'_' => 31,
        b'a' => 1,
        b'b' => 2,
        b'c' => 3,
        b'd' => 4,
        b'e' => 5,
        b'f' => 6,
        b'g' => 7,
        b'h' => 8,
        b'j' => 10,
        b'k' => 11,
        b'l' => 12,
        b'n' => 14,
        b'o' => 15,
        b'p' => 16,
        b'q' => 17,
        b'r' => 18,
        b's' => 19,
        b't' => 20,
        b'u' => 21,
        b'v' => 22,
        b'w' => 23,
        b'x' => 24,
        b'y' => 25,
        b'z' => 26,
        b'~' => 30,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// The PC-style function key table (function_keys.zig, ported).
// ---------------------------------------------------------------------------

/// `pcStyleFunctionKey`: the xterm function-key table. The comptime-generated
/// families (pcStyle / cursorKey / kpKeys) are re-generated here procedurally --
/// the same generator, not a re-derivation -- and the four hand-written tables
/// (backspace, tab, enter, escape) are ported entry-for-entry below.
fn pc_style_function_key(key: Key, mods: KeyMods, opts: &KeyOptions) -> Option<Vec<u8>> {
    let mods = mods & MODS_BINDING;
    // Mode 1035 on = always numerical keypad; off = the requested mode. (Numlock
    // itself never reaches the encoder: the KEYCODE already says kp_1 vs kp_end.)
    let keypad_app = !opts.ignore_keypad_with_numlock && opts.keypad_key_application;
    let cursor = opts.cursor_key_application;
    match key {
        Key::ArrowUp | Key::NumpadUp => cursor_key(b'A', mods, cursor),
        Key::ArrowDown | Key::NumpadDown => cursor_key(b'B', mods, cursor),
        Key::ArrowRight | Key::NumpadRight => cursor_key(b'C', mods, cursor),
        Key::ArrowLeft | Key::NumpadLeft => cursor_key(b'D', mods, cursor),
        Key::NumpadBegin => cursor_key(b'E', mods, cursor),
        Key::Home | Key::NumpadHome => cursor_key(b'H', mods, cursor),
        Key::End | Key::NumpadEnd => cursor_key(b'F', mods, cursor),
        Key::Insert | Key::NumpadInsert => tilde_key(2, mods, b"\x1B[2~"),
        Key::Delete | Key::NumpadDelete => tilde_key(3, mods, b"\x1B[3~"),
        Key::PageUp | Key::NumpadPageUp => tilde_key(5, mods, b"\x1B[5~"),
        Key::PageDown | Key::NumpadPageDown => tilde_key(6, mods, b"\x1B[6~"),
        Key::F1 => letter_key(b'P', mods, b"\x1BOP"),
        Key::F2 => letter_key(b'Q', mods, b"\x1BOQ"),
        Key::F3 => tilde_key(13, mods, b"\x1BOR"), // f3's mod form moved to 13~ in xterm
        Key::F4 => letter_key(b'S', mods, b"\x1BOS"),
        Key::F5 => tilde_key(15, mods, b"\x1B[15~"),
        Key::F6 => tilde_key(17, mods, b"\x1B[17~"),
        Key::F7 => tilde_key(18, mods, b"\x1B[18~"),
        Key::F8 => tilde_key(19, mods, b"\x1B[19~"),
        Key::F9 => tilde_key(20, mods, b"\x1B[20~"),
        Key::F10 => tilde_key(21, mods, b"\x1B[21~"),
        Key::F11 => tilde_key(23, mods, b"\x1B[23~"),
        Key::F12 => tilde_key(24, mods, b"\x1B[24~"),
        Key::Numpad0 => keypad_key(b'p', mods, keypad_app),
        Key::Numpad1 => keypad_key(b'q', mods, keypad_app),
        Key::Numpad2 => keypad_key(b'r', mods, keypad_app),
        Key::Numpad3 => keypad_key(b's', mods, keypad_app),
        Key::Numpad4 => keypad_key(b't', mods, keypad_app),
        Key::Numpad5 => keypad_key(b'u', mods, keypad_app),
        Key::Numpad6 => keypad_key(b'v', mods, keypad_app),
        Key::Numpad7 => keypad_key(b'w', mods, keypad_app),
        Key::Numpad8 => keypad_key(b'x', mods, keypad_app),
        Key::Numpad9 => keypad_key(b'y', mods, keypad_app),
        Key::NumpadDecimal => keypad_key(b'n', mods, keypad_app),
        Key::NumpadDivide => keypad_key(b'o', mods, keypad_app),
        Key::NumpadMultiply => keypad_key(b'j', mods, keypad_app),
        Key::NumpadSubtract => keypad_key(b'm', mods, keypad_app),
        Key::NumpadAdd => keypad_key(b'k', mods, keypad_app),
        // Application mode encodes like the other keypad keys; numerical mode has
        // an any-mods "\r" fallback the digits don't get.
        Key::NumpadEnter => {
            if keypad_app {
                keypad_key(b'M', mods, true)
            } else {
                Some(b"\r".to_vec())
            }
        }
        Key::Backspace => special_key(BACKSPACE_ENTRIES, mods, opts),
        Key::Tab => special_key(TAB_ENTRIES, mods, opts),
        Key::Enter => special_key(ENTER_ENTRIES, mods, opts),
        Key::Escape => special_key(ESCAPE_ENTRIES, mods, opts),
        _ => None,
    }
}

/// pcStyle("\x1b[1;{}X") ++ cursorKey("\x1b[X", "\x1bOX"): the modifier form is
/// checked FIRST (any cursor mode), the plain form then splits on DECCKM.
fn cursor_key(final_byte: u8, mods: KeyMods, application: bool) -> Option<Vec<u8>> {
    let f = char::from(final_byte);
    Some(match xterm_mod_code(mods) {
        Some(code) => format!("\x1B[1;{code}{f}").into_bytes(),
        None if application => format!("\x1BO{f}").into_bytes(),
        None => format!("\x1B[{f}").into_bytes(),
    })
}

/// pcStyle("\x1b[N;{}~") ++ a fixed plain fallback.
fn tilde_key(number: u32, mods: KeyMods, plain: &[u8]) -> Option<Vec<u8>> {
    Some(match xterm_mod_code(mods) {
        Some(code) => format!("\x1B[{number};{code}~").into_bytes(),
        None => plain.to_vec(),
    })
}

/// pcStyle("\x1b[1;{}X") ++ a fixed SS3 fallback (f1/f2/f4).
fn letter_key(final_byte: u8, mods: KeyMods, plain: &[u8]) -> Option<Vec<u8>> {
    let f = char::from(final_byte);
    Some(match xterm_mod_code(mods) {
        Some(code) => format!("\x1B[1;{code}{f}").into_bytes(),
        None => plain.to_vec(),
    })
}

/// kpKeys: entries exist ONLY for keypad application mode -- numerical mode falls
/// through to the text the layout produced.
fn keypad_key(suffix: u8, mods: KeyMods, keypad_app: bool) -> Option<Vec<u8>> {
    if !keypad_app {
        return None;
    }
    let s = char::from(suffix);
    Some(match xterm_mod_code(mods) {
        Some(code) => format!("\x1BO{code}{s}").into_bytes(),
        None => format!("\x1BO{s}").into_bytes(),
    })
}

/// Which modifyOtherKeys state an entry demands (`function_keys.ModifyKeys`):
/// `Set` = only OUTSIDE state 2, `SetOther` = only IN state 2.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Modify {
    Any,
    Set,
    SetOther,
}

/// One hand-written table entry. `mods == 0` with `empty_is_any` matches every
/// modifier set (the catch-all rows); non-zero mods require an exact match.
struct SpecialEntry {
    mods: KeyMods,
    empty_is_any: bool,
    modify: Modify,
    seq: &'static [u8],
    seq_decbkm: Option<&'static [u8]>,
}

const fn entry(mods: KeyMods, modify: Modify, seq: &'static [u8]) -> SpecialEntry {
    SpecialEntry { mods, empty_is_any: true, modify, seq, seq_decbkm: None }
}

/// First matching entry wins, in table order, exactly the oracle's loop; DECBKM
/// swaps in the alternate sequence where one exists.
fn special_key(
    entries: &[SpecialEntry],
    mods: KeyMods,
    opts: &KeyOptions,
) -> Option<Vec<u8>> {
    for e in entries {
        match e.modify {
            Modify::Set if opts.modify_other_keys_state_2 => continue,
            Modify::SetOther if !opts.modify_other_keys_state_2 => continue,
            _ => {}
        }
        if e.mods == 0 {
            if mods != 0 && !e.empty_is_any {
                continue;
            }
        } else if e.mods != mods {
            continue;
        }
        if opts.backarrow_key_mode
            && let Some(seq) = e.seq_decbkm
        {
            return Some(seq.to_vec());
        }
        return Some(e.seq.to_vec());
    }
    None
}

const S: KeyMods = KEY_MODS_SHIFT;
const C: KeyMods = KEY_MODS_CTRL;
const A: KeyMods = KEY_MODS_ALT;
const U: KeyMods = KEY_MODS_SUPER; // "super"; U to keep the rows scannable

/// function_keys.zig's backspace table, entry-for-entry.
const BACKSPACE_ENTRIES: &[SpecialEntry] = &[
    entry(S, Modify::Set, b"\x7F"),
    entry(A, Modify::Set, b"\x1B\x7F"),
    entry(A | S, Modify::Set, b"\x1B\x7F"),
    entry(C | S, Modify::Set, b"\x08"),
    entry(A | C, Modify::Set, b"\x1B\x08"),
    entry(U, Modify::Set, b"\x7F"),
    entry(U | S, Modify::Set, b"\x7F"),
    entry(A | U, Modify::Set, b"\x1B\x7F"),
    entry(A | U | S, Modify::Set, b"\x1B\x7F"),
    entry(U | C, Modify::Set, b"\x08"),
    entry(U | C | S, Modify::Set, b"\x08"),
    entry(A | U | C, Modify::Set, b"\x1B\x08"),
    entry(A | U | C | S, Modify::Set, b"\x1B\x08"),
    entry(S, Modify::SetOther, b"\x1B[27;2;127~"),
    entry(A, Modify::SetOther, b"\x1B[27;3;127~"),
    entry(A | S, Modify::SetOther, b"\x1B[27;4;127~"),
    entry(C | S, Modify::SetOther, b"\x1B[27;6;127~"),
    entry(A | C, Modify::SetOther, b"\x1B[27;7;127~"),
    entry(A | S | C, Modify::SetOther, b"\x1B[27;8;127~"),
    entry(U, Modify::SetOther, b"\x1B[27;9;127~"),
    entry(U | S, Modify::SetOther, b"\x1B[27;10;127~"),
    entry(A | U, Modify::SetOther, b"\x1B[27;11;127~"),
    entry(A | U | S, Modify::SetOther, b"\x1B[27;12;127~"),
    entry(U | C, Modify::SetOther, b"\x1B[27;13;127~"),
    entry(U | C | S, Modify::SetOther, b"\x1B[27;14;127~"),
    entry(A | U | C, Modify::SetOther, b"\x1B[27;15;127~"),
    entry(A | U | C | S, Modify::SetOther, b"\x1B[27;16;127~"),
    SpecialEntry {
        mods: C,
        empty_is_any: true,
        modify: Modify::Any,
        seq: b"\x08",
        seq_decbkm: Some(b"\x7F"),
    },
    SpecialEntry {
        mods: 0,
        empty_is_any: true,
        modify: Modify::Any,
        seq: b"\x7F",
        seq_decbkm: Some(b"\x08"),
    },
];

/// function_keys.zig's tab table, entry-for-entry.
const TAB_ENTRIES: &[SpecialEntry] = &[
    entry(S, Modify::Set, b"\x1B[Z"),
    entry(A, Modify::Set, b"\x1B\t"),
    entry(S, Modify::SetOther, b"\x1B[27;2;9~"),
    entry(A, Modify::SetOther, b"\x1B[27;3;9~"),
    entry(A | S, Modify::Any, b"\x1B[27;4;9~"),
    entry(C, Modify::Any, b"\x1B[27;5;9~"),
    entry(C | S, Modify::Any, b"\x1B[27;6;9~"),
    entry(A | C, Modify::Any, b"\x1B[27;7;9~"),
    entry(A | C | S, Modify::Any, b"\x1B[27;8;9~"),
    entry(U, Modify::Any, b"\x1B[27;9;9~"),
    entry(U | S, Modify::Any, b"\x1B[27;10;9~"),
    entry(A | U, Modify::Any, b"\x1B[27;11;9~"),
    entry(A | U | S, Modify::Any, b"\x1B[27;12;9~"),
    entry(U | C, Modify::Any, b"\x1B[27;13;9~"),
    entry(U | C | S, Modify::Any, b"\x1B[27;14;9~"),
    entry(A | U | C, Modify::Any, b"\x1B[27;15;9~"),
    entry(A | U | C | S, Modify::Any, b"\x1B[27;16;9~"),
    entry(0, Modify::Any, b"\t"),
];

/// function_keys.zig's enter table, entry-for-entry.
const ENTER_ENTRIES: &[SpecialEntry] = &[
    entry(S, Modify::Any, b"\x1B[27;2;13~"),
    entry(A, Modify::Set, b"\x1B\r"),
    entry(A, Modify::SetOther, b"\x1B[27;3;13~"),
    entry(A | S, Modify::Any, b"\x1B[27;4;13~"),
    entry(C, Modify::Any, b"\x1B[27;5;13~"),
    entry(C | S, Modify::Any, b"\x1B[27;6;13~"),
    entry(A | C, Modify::Any, b"\x1B[27;7;13~"),
    entry(A | C | S, Modify::Any, b"\x1B[27;8;13~"),
    entry(U, Modify::Any, b"\x1B[27;9;13~"),
    entry(U | S, Modify::Any, b"\x1B[27;10;13~"),
    entry(A | U, Modify::Any, b"\x1B[27;11;13~"),
    entry(A | U | S, Modify::Any, b"\x1B[27;12;13~"),
    entry(U | C, Modify::Any, b"\x1B[27;13;13~"),
    entry(U | C | S, Modify::Any, b"\x1B[27;14;13~"),
    entry(A | U | C, Modify::Any, b"\x1B[27;15;13~"),
    entry(A | U | C | S, Modify::Any, b"\x1B[27;16;13~"),
    entry(0, Modify::Any, b"\r"),
];

/// function_keys.zig's escape table, entry-for-entry.
const ESCAPE_ENTRIES: &[SpecialEntry] = &[
    entry(S, Modify::Any, b"\x1B[27;2;27~"),
    entry(A, Modify::Any, b"\x1B\x1B"),
    entry(A | S, Modify::Any, b"\x1B[27;4;27~"),
    entry(C, Modify::Any, b"\x1B[27;5;27~"),
    entry(C | S, Modify::Any, b"\x1B[27;6;27~"),
    entry(A | C, Modify::Any, b"\x1B[27;7;27~"),
    entry(A | C | S, Modify::Any, b"\x1B[27;8;27~"),
    entry(U, Modify::Any, b"\x1B[27;9;27~"),
    entry(U | S, Modify::Any, b"\x1B[27;10;27~"),
    entry(A | U, Modify::Any, b"\x1B[27;11;27~"),
    entry(A | U | S, Modify::Any, b"\x1B[27;12;27~"),
    entry(U | C, Modify::Any, b"\x1B[27;13;27~"),
    entry(U | C | S, Modify::Any, b"\x1B[27;14;27~"),
    entry(A | U | C, Modify::Any, b"\x1B[27;15;27~"),
    entry(A | U | C | S, Modify::Any, b"\x1B[27;16;27~"),
    entry(0, Modify::Any, b"\x1B"),
];

// ---------------------------------------------------------------------------
// Kitty keyboard protocol (kitty() in key_encode.zig; table from kitty.zig).
// ---------------------------------------------------------------------------

/// A kitty keymap row: key -> CSI number, final byte, is-a-modifier.
struct KittyEntry {
    code: u32,
    final_byte: u8,
    modifier: bool,
}

/// The functional-key table from kitty.zig, ported verbatim (same order; a linear
/// search over ~100 rows is the oracle's own recommendation).
const KITTY_TABLE: &[(Key, u32, u8, bool)] = &[
    (Key::Escape, 27, b'u', false),
    (Key::Enter, 13, b'u', false),
    (Key::Tab, 9, b'u', false),
    (Key::Backspace, 127, b'u', false),
    (Key::Insert, 2, b'~', false),
    (Key::Delete, 3, b'~', false),
    (Key::ArrowLeft, 1, b'D', false),
    (Key::ArrowRight, 1, b'C', false),
    (Key::ArrowUp, 1, b'A', false),
    (Key::ArrowDown, 1, b'B', false),
    (Key::PageUp, 5, b'~', false),
    (Key::PageDown, 6, b'~', false),
    (Key::Home, 1, b'H', false),
    (Key::End, 1, b'F', false),
    (Key::CapsLock, 57358, b'u', true),
    (Key::ScrollLock, 57359, b'u', false),
    (Key::NumLock, 57360, b'u', true),
    (Key::PrintScreen, 57361, b'u', false),
    (Key::Pause, 57362, b'u', false),
    (Key::F1, 1, b'P', false),
    (Key::F2, 1, b'Q', false),
    (Key::F3, 13, b'~', false),
    (Key::F4, 1, b'S', false),
    (Key::F5, 15, b'~', false),
    (Key::F6, 17, b'~', false),
    (Key::F7, 18, b'~', false),
    (Key::F8, 19, b'~', false),
    (Key::F9, 20, b'~', false),
    (Key::F10, 21, b'~', false),
    (Key::F11, 23, b'~', false),
    (Key::F12, 24, b'~', false),
    (Key::F13, 57376, b'u', false),
    (Key::F14, 57377, b'u', false),
    (Key::F15, 57378, b'u', false),
    (Key::F16, 57379, b'u', false),
    (Key::F17, 57380, b'u', false),
    (Key::F18, 57381, b'u', false),
    (Key::F19, 57382, b'u', false),
    (Key::F20, 57383, b'u', false),
    (Key::F21, 57384, b'u', false),
    (Key::F22, 57385, b'u', false),
    (Key::F23, 57386, b'u', false),
    (Key::F24, 57387, b'u', false),
    (Key::F25, 57388, b'u', false),
    (Key::Numpad0, 57399, b'u', false),
    (Key::Numpad1, 57400, b'u', false),
    (Key::Numpad2, 57401, b'u', false),
    (Key::Numpad3, 57402, b'u', false),
    (Key::Numpad4, 57403, b'u', false),
    (Key::Numpad5, 57404, b'u', false),
    (Key::Numpad6, 57405, b'u', false),
    (Key::Numpad7, 57406, b'u', false),
    (Key::Numpad8, 57407, b'u', false),
    (Key::Numpad9, 57408, b'u', false),
    (Key::NumpadDecimal, 57409, b'u', false),
    (Key::NumpadDivide, 57410, b'u', false),
    (Key::NumpadMultiply, 57411, b'u', false),
    (Key::NumpadSubtract, 57412, b'u', false),
    (Key::NumpadAdd, 57413, b'u', false),
    (Key::NumpadEnter, 57414, b'u', false),
    (Key::NumpadEqual, 57415, b'u', false),
    (Key::NumpadSeparator, 57416, b'u', false),
    (Key::NumpadLeft, 57417, b'u', false),
    (Key::NumpadRight, 57418, b'u', false),
    (Key::NumpadUp, 57419, b'u', false),
    (Key::NumpadDown, 57420, b'u', false),
    (Key::NumpadPageUp, 57421, b'u', false),
    (Key::NumpadPageDown, 57422, b'u', false),
    (Key::NumpadHome, 57423, b'u', false),
    (Key::NumpadEnd, 57424, b'u', false),
    (Key::NumpadInsert, 57425, b'u', false),
    (Key::NumpadDelete, 57426, b'u', false),
    (Key::NumpadBegin, 57427, b'u', false),
    (Key::ShiftLeft, 57441, b'u', true),
    (Key::ShiftRight, 57447, b'u', true),
    (Key::ControlLeft, 57442, b'u', true),
    (Key::ControlRight, 57448, b'u', true),
    (Key::MetaLeft, 57444, b'u', true),
    (Key::MetaRight, 57450, b'u', true),
    (Key::AltLeft, 57443, b'u', true),
    (Key::AltRight, 57449, b'u', true),
];

/// The table entry for a key, or the unshifted-codepoint fallback for text keys.
fn kitty_entry(event: &KeyEvent<'_>) -> Option<KittyEntry> {
    for &(key, code, final_byte, modifier) in KITTY_TABLE {
        if key == event.key {
            return Some(KittyEntry { code, final_byte, modifier });
        }
    }
    if event.unshifted_codepoint > 0 {
        return Some(KittyEntry {
            code: event.unshifted_codepoint,
            final_byte: b'u',
            modifier: false,
        });
    }
    None
}

/// Kitty mods pack shift(1), alt(2), ctrl(4), super(8), caps(64), num(128) --
/// hyper and meta exist on the wire but no platform here produces them.
fn kitty_mods(mods: KeyMods) -> u8 {
    let mut r: u8 = 0;
    if mods & KEY_MODS_SHIFT != 0 {
        r |= 1;
    }
    if mods & KEY_MODS_ALT != 0 {
        r |= 2;
    }
    if mods & KEY_MODS_CTRL != 0 {
        r |= 4;
    }
    if mods & KEY_MODS_SUPER != 0 {
        r |= 8;
    }
    if mods & KEY_MODS_CAPS_LOCK != 0 {
        r |= 64;
    }
    if mods & KEY_MODS_NUM_LOCK != 0 {
        r |= 128;
    }
    r
}

/// Kitty event-type subparam values (none omits the field on press).
const KITTY_EVENT_NONE: u8 = 0;
const KITTY_EVENT_PRESS: u8 = 1;
const KITTY_EVENT_REPEAT: u8 = 2;
const KITTY_EVENT_RELEASE: u8 = 3;

struct KittySeq<'a> {
    key: u32,
    final_byte: u8,
    mods: u8,
    event: u8,
    alternates: [Option<u32>; 2],
    text: &'a str,
}

/// `kitty()` in key_encode.zig: the preprocessing gauntlet, then the CSI u build.
fn kitty(out: &mut Vec<u8>, event: &KeyEvent<'_>, opts: &KeyOptions) {
    let flags = opts.kitty_flags & 0x1F;
    let report_events = flags & KITTY_REPORT_EVENTS != 0;
    let report_all = flags & KITTY_REPORT_ALL != 0;

    if event.action == KeyAction::Release {
        if !report_events {
            return;
        }
        // Enter/backspace/tab keep their legacy bytes without "report all", and a
        // release of a legacy byte is nothing.
        if !report_all && matches!(event.key, Key::Enter | Key::Backspace | Key::Tab) {
            return;
        }
    }

    let binding_mods = effective_mods(event) & MODS_BINDING;
    let entry = kitty_entry(event);

    'preprocessing: {
        // When composing, the only keys sent are plain modifiers.
        if event.composing {
            if let Some(e) = &entry
                && e.modifier
            {
                break 'preprocessing;
            }
            return;
        }
        // IME confirmation still presses enter: with committed (non-control) text,
        // send the text, and let backspace's preedit edit encode nothing.
        if !event.utf8.is_empty()
            && matches!(event.key, Key::Enter | Key::Backspace)
            && !is_control_utf8(event.utf8)
        {
            if event.key == Key::Backspace {
                return;
            }
            out.extend_from_slice(event.utf8.as_bytes());
            return;
        }
        if !report_all {
            // The kitty spec's stated exceptions: unmodified enter/tab/backspace
            // keep legacy bytes so `reset` still works in a wedged shell.
            if binding_mods == 0 {
                match event.key {
                    Key::Enter => {
                        out.push(b'\r');
                        return;
                    }
                    Key::Tab => {
                        out.push(b'\t');
                        return;
                    }
                    Key::Backspace => {
                        out.push(0x7F);
                        return;
                    }
                    _ => {}
                }
            }
            // Unmodified printable text passes through plain (releases are
            // encoded, never echoed).
            if !event.utf8.is_empty()
                && binding_mods == 0
                && event.action != KeyAction::Release
                && event.utf8.chars().all(|c| !is_control(c as u32))
            {
                out.extend_from_slice(event.utf8.as_bytes());
                return;
            }
        }
    }

    let Some(entry) = entry else {
        // No entry but text: a pure text event (composed/IME), sent as-is.
        if !event.utf8.is_empty() {
            out.extend_from_slice(event.utf8.as_bytes());
        }
        return;
    };
    // A bare modifier key reports only under "report all".
    if entry.modifier && !report_all {
        return;
    }

    let seq = build_kitty_seq(&entry, event, opts, flags);
    encode_kitty_seq(out, &seq);
}

/// Fills mods, event type, alternates and associated text per the active flags.
fn build_kitty_seq<'a>(
    entry: &KittyEntry,
    event: &'a KeyEvent<'_>,
    opts: &KeyOptions,
    flags: u8,
) -> KittySeq<'a> {
    let mut seq = KittySeq {
        key: entry.code,
        final_byte: entry.final_byte,
        mods: kitty_mods(event.mods),
        event: KITTY_EVENT_NONE,
        alternates: [None, None],
        text: "",
    };
    if flags & KITTY_REPORT_EVENTS != 0 {
        seq.event = match event.action {
            KeyAction::Press => KITTY_EVENT_PRESS,
            KeyAction::Release => KITTY_EVENT_RELEASE,
            KeyAction::Repeat => KITTY_EVENT_REPEAT,
        };
    }
    // Alternates: the shifted codepoint (only if shift is down and it differs),
    // then the base-layout key (only when the single-codepoint text differs).
    if flags & KITTY_REPORT_ALTERNATES != 0 && !is_control(seq.key) {
        let mut it = event.utf8.chars();
        match it.next() {
            Some(cp1) => {
                let cp1 = cp1 as u32;
                if cp1 != seq.key && seq.mods & 1 != 0 {
                    seq.alternates[0] = Some(cp1);
                }
                let has_cp2 = it.next().is_some();
                if let Some(base) = event.key.codepoint()
                    && base != seq.key
                    && cp1 != base
                    && !has_cp2
                {
                    seq.alternates[1] = Some(base);
                }
            }
            None => {
                if let Some(base) = event.key.codepoint()
                    && base != seq.key
                {
                    seq.alternates[1] = Some(base);
                }
            }
        }
    }
    if flags & KITTY_REPORT_ASSOCIATED != 0 && seq.event != KITTY_EVENT_RELEASE {
        // macOS: option-as-alt decides whether alt is a text-preventing modifier
        // or the option key composing the text itself.
        let alt_prevents_text = match opts.macos_option_as_alt {
            OptionAsAlt::Left => event.mods & KEY_MODS_ALT_SIDE == 0,
            OptionAsAlt::Right => event.mods & KEY_MODS_ALT_SIDE != 0,
            OptionAsAlt::True => true,
            OptionAsAlt::False => false,
        };
        let prevents = (seq.mods & 2 != 0 && alt_prevents_text)
            || seq.mods & 4 != 0
            || seq.mods & 8 != 0;
        if !prevents {
            seq.text = event.utf8;
        }
    }
    seq
}

/// `KittySequence.encode`: 'u'/'~' finals take the full form, letter finals the
/// short legacy-compatible form with "1" as the key.
fn encode_kitty_seq(out: &mut Vec<u8>, seq: &KittySeq<'_>) {
    let mods = u16::from(seq.mods) + 1;
    if seq.final_byte != b'u' && seq.final_byte != b'~' {
        let f = char::from(seq.final_byte);
        // NOTE the asymmetry with the full form: a press event IS emitted here.
        let s = if seq.event != KITTY_EVENT_NONE {
            format!("\x1B[1;{mods}:{}{f}", seq.event)
        } else if mods > 1 {
            format!("\x1B[1;{mods}{f}")
        } else {
            format!("\x1B[{f}")
        };
        out.extend_from_slice(s.as_bytes());
        return;
    }

    out.extend_from_slice(format!("\x1B[{}", seq.key).as_bytes());
    if let Some(shifted) = seq.alternates[0] {
        out.extend_from_slice(format!(":{shifted}").as_bytes());
    }
    if let Some(base) = seq.alternates[1] {
        let sep = if seq.alternates[0].is_none() { "::" } else { ":" };
        out.extend_from_slice(format!("{sep}{base}").as_bytes());
    }
    let mut emit_prior = false;
    if seq.event != KITTY_EVENT_NONE && seq.event != KITTY_EVENT_PRESS {
        out.extend_from_slice(format!(";{mods}:{}", seq.event).as_bytes());
        emit_prior = true;
    } else if mods > 1 {
        out.extend_from_slice(format!(";{mods}").as_bytes());
        emit_prior = true;
    }
    let mut count = 0usize;
    for cp in seq.text.chars().map(|c| c as u32) {
        if is_control(cp) {
            continue;
        }
        if count == 0 {
            if !emit_prior {
                out.push(b';');
            }
            out.push(b';');
        } else {
            out.push(b':');
        }
        out.extend_from_slice(format!("{cp}").as_bytes());
        count += 1;
    }
    out.push(seq.final_byte);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(key: Key, mods: KeyMods, utf8: &str, unshifted: u32) -> KeyEvent<'_> {
        KeyEvent {
            action: KeyAction::Press,
            key,
            mods,
            consumed_mods: 0,
            composing: false,
            utf8,
            unshifted_codepoint: unshifted,
        }
    }

    fn legacy_opts() -> KeyOptions {
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

    #[test]
    fn ctrl_a_is_a_c0_byte_and_ctrl_shift_a_is_csiu_lowercased() {
        let opts = legacy_opts();
        let plain = press(Key::A, KEY_MODS_CTRL, "a", 'a' as u32);
        assert_eq!(encode(&plain, &opts), vec![0x01]);
        // Shifted letters skip the C0 table so programs can tell the two apart
        // (the kitty divergence the oracle follows); the char is sent lowercase
        // with the shift mod intact.
        let shifted = press(Key::A, KEY_MODS_CTRL | KEY_MODS_SHIFT, "A", 'a' as u32);
        assert_eq!(encode(&shifted, &opts), b"\x1B[97;6u".to_vec());
    }

    #[test]
    fn ctrl_shift_minus_lets_shift_be_spent_and_encodes_0x1f() {
        // The emacs case: '-' with shift produced '_', shift is consumed by the
        // char so the C0 path still fires.
        let event = press(Key::Minus, KEY_MODS_CTRL | KEY_MODS_SHIFT, "_", '-' as u32);
        assert_eq!(encode(&event, &legacy_opts()), vec![31]);
    }

    #[test]
    fn alt_prefix_needs_option_acting_as_alt_on_macos() {
        let event = press(Key::C, KEY_MODS_ALT, "ç", 'c' as u32);
        // Option did a unicode translation: the text goes through untouched.
        assert_eq!(encode(&event, &legacy_opts()), "ç".as_bytes().to_vec());
        // Option as alt: ESC-prefix the unshifted byte instead.
        let mut opts = legacy_opts();
        opts.macos_option_as_alt = OptionAsAlt::True;
        let event = press(Key::C, KEY_MODS_ALT, "c", 'c' as u32);
        assert_eq!(encode(&event, &opts), b"\x1Bc".to_vec());
    }

    #[test]
    fn cursor_keys_split_on_decckm_only_without_mods() {
        let opts = legacy_opts();
        let plain = press(Key::ArrowUp, 0, "", 0);
        assert_eq!(encode(&plain, &opts), b"\x1B[A".to_vec());
        let mut app = legacy_opts();
        app.cursor_key_application = true;
        assert_eq!(encode(&plain, &app), b"\x1BOA".to_vec());
        // The modifier form ignores DECCKM entirely.
        let shifted = press(Key::ArrowUp, KEY_MODS_SHIFT, "", 0);
        assert_eq!(encode(&shifted, &app), b"\x1B[1;2A".to_vec());
    }

    #[test]
    fn keypad_application_yields_to_mode_1035() {
        let event = press(Key::Numpad1, 0, "1", '1' as u32);
        let mut opts = legacy_opts();
        opts.keypad_key_application = true;
        // 1035 on (the default): always numerical -- the text wins.
        assert_eq!(encode(&event, &opts), b"1".to_vec());
        opts.ignore_keypad_with_numlock = false;
        assert_eq!(encode(&event, &opts), b"\x1BOq".to_vec());
    }

    #[test]
    fn backspace_swaps_bytes_under_decbkm() {
        let event = press(Key::Backspace, 0, "\x7F", 0x7F);
        let mut opts = legacy_opts();
        assert_eq!(encode(&event, &opts), vec![0x7F]);
        opts.backarrow_key_mode = true;
        assert_eq!(encode(&event, &opts), vec![0x08]);
    }

    #[test]
    fn modify_other_keys_2_encodes_shift_space_and_shifted_letters() {
        let mut opts = legacy_opts();
        opts.modify_other_keys_state_2 = true;
        let space = press(Key::Space, KEY_MODS_SHIFT, " ", ' ' as u32);
        assert_eq!(encode(&space, &opts), b"\x1B[27;2;32~".to_vec());
        // xterm's IsControlInput range is 0x40..=0x7F, so a shifted LETTER
        // qualifies too -- 'A' is 65. Unmodified text never does (no mod entry).
        let letter = press(Key::A, KEY_MODS_SHIFT, "A", 'a' as u32);
        assert_eq!(encode(&letter, &opts), b"\x1B[27;2;65~".to_vec());
        let plain = press(Key::A, 0, "a", 'a' as u32);
        assert_eq!(encode(&plain, &opts), b"a".to_vec());
    }

    fn kitty_opts(flags: u8) -> KeyOptions {
        KeyOptions { kitty_flags: flags, ..legacy_opts() }
    }

    #[test]
    fn kitty_keeps_legacy_bytes_for_unmodified_enter_without_report_all() {
        let enter = press(Key::Enter, 0, "\r", 13);
        assert_eq!(encode(&enter, &kitty_opts(KITTY_DISAMBIGUATE)), b"\r".to_vec());
        assert_eq!(
            encode(&enter, &kitty_opts(KITTY_DISAMBIGUATE | KITTY_REPORT_ALL)),
            b"\x1B[13u".to_vec()
        );
    }

    #[test]
    fn kitty_release_needs_report_events_and_special_finals_report_press() {
        let mut release = press(Key::ArrowUp, 0, "", 0);
        release.action = KeyAction::Release;
        assert_eq!(encode(&release, &kitty_opts(KITTY_DISAMBIGUATE)), Vec::<u8>::new());
        let flags = KITTY_DISAMBIGUATE | KITTY_REPORT_EVENTS;
        assert_eq!(encode(&release, &kitty_opts(flags)), b"\x1B[1;1:3A".to_vec());
        // The letter-final form emits ":1" for a press -- the full 'u' form
        // omits it. Asymmetry pinned because it is easy to "fix" wrongly.
        let p = press(Key::ArrowUp, 0, "", 0);
        assert_eq!(encode(&p, &kitty_opts(flags)), b"\x1B[1;1:1A".to_vec());
    }

    #[test]
    fn kitty_alternates_and_associated_text_for_shift_a() {
        let mut event = press(Key::A, KEY_MODS_SHIFT, "A", 'a' as u32);
        event.consumed_mods = KEY_MODS_SHIFT;
        let flags = KITTY_DISAMBIGUATE
            | KITTY_REPORT_EVENTS
            | KITTY_REPORT_ALTERNATES
            | KITTY_REPORT_ALL
            | KITTY_REPORT_ASSOCIATED;
        // Key 97, shifted alternate 65, mods shift -> 2 (the full form omits a
        // press event), associated text "A" -> 65.
        assert_eq!(encode(&event, &kitty_opts(flags)), b"\x1B[97:65;2;65u".to_vec());
    }

    #[test]
    fn kitty_bare_modifier_needs_report_all() {
        let event = press(Key::ShiftLeft, KEY_MODS_SHIFT, "", 0);
        assert_eq!(encode(&event, &kitty_opts(KITTY_DISAMBIGUATE)), Vec::<u8>::new());
        assert_eq!(
            encode(&event, &kitty_opts(KITTY_DISAMBIGUATE | KITTY_REPORT_ALL)),
            b"\x1B[57441;2u".to_vec()
        );
    }

    #[test]
    fn composing_silences_everything_but_plain_modifiers() {
        let mut event = press(Key::A, 0, "a", 'a' as u32);
        event.composing = true;
        assert_eq!(encode(&event, &legacy_opts()), Vec::<u8>::new());
        assert_eq!(encode(&event, &kitty_opts(KITTY_DISAMBIGUATE)), Vec::<u8>::new());
        let mut modifier = press(Key::ShiftLeft, KEY_MODS_SHIFT, "", 0);
        modifier.composing = true;
        assert_eq!(
            encode(&modifier, &kitty_opts(KITTY_DISAMBIGUATE | KITTY_REPORT_ALL)),
            b"\x1B[57441;2u".to_vec()
        );
    }

    #[test]
    fn super_never_encodes_text_on_macos() {
        let event = press(Key::B, KEY_MODS_SUPER, "b", 'b' as u32);
        assert_eq!(encode(&event, &legacy_opts()), Vec::<u8>::new());
    }
}
