//! Purpose: the OSC-addressable colour state (OSC 4/104 indexed, 10/11/12 + 110/111/112
//! dynamic) and the colour-spec parser both of them share.
//! Reference: measured from the oracle's source 2026-08-01:
//!   - state shape: `Terminal.zig` `Colors` (three `DynamicRGB` + a `DynamicPalette`
//!     carrying current/original/mask), applied in `stream_terminal.zig`
//!     `colorOperation`.
//!   - spec grammar and scaling: `color.zig` `RGB.parse`/`fromHex`/`fromIntensity`.
//!     A channel of n hex digits scales as `v * 255 / (16^n - 1)`, so `#fff` stores
//!     0xFF, NOT xterm's 0xF0. That is the oracle's rule, it is why Ghostty fails
//!     esctest's Hash3/Hash9/Hash12 cases, and this core mirrors it deliberately:
//!     drop-in parity outranks the scoreboard.
//!   - RIS: the oracle's `fullReset` never touches `colors`, so overrides SURVIVE a
//!     full reset (corpus-pinned). DECSTR does not touch them either.
//! NOT responsible for: replies (`replies.rs` formats query answers) or OSC parsing
//!   (`terminal.rs` routes the operations here).
//! Divergence, named: X11 colour NAMES ("ForestGreen") are not accepted; the oracle
//!   carries the full X11 rgb.txt map. No corpus case or esctest exercises names; the
//!   gap is recorded in docs/BACKLOG-2026.md and a name spec simply fails to parse,
//!   which per the oracle's error rule stops the request list at that point.

use ruuah_vt_snapshot::{Rgb, default_palette};

/// One dynamic colour (foreground, background or cursor): an OSC override over an
/// embedder default. Mirrors the oracle's `DynamicRGB`. This core has no config
/// surface writing `default_color` yet, but the field exists so the C-surface OPT
/// setters can land without reshaping state.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DynamicRgb {
    pub(crate) override_color: Option<Rgb>,
    pub(crate) default_color: Option<Rgb>,
}

impl DynamicRgb {
    /// The effective value: override, else default, else nothing. What the snapshot
    /// reports and what `GHOSTTY_TERMINAL_DATA_COLOR_*` answers.
    pub(crate) fn get(self) -> Option<Rgb> {
        self.override_color.or(self.default_color)
    }

    pub(crate) fn set(&mut self, color: Rgb) {
        self.override_color = Some(color);
    }

    /// OSC 110/111/112: back to the default. The oracle assigns
    /// `override = default`, which when no default is configured means unset.
    pub(crate) fn reset(&mut self) {
        self.override_color = self.default_color;
    }
}

/// The 256-entry palette with its default table and an override mask. The mask is what
/// makes OSC 104-with-no-arguments correct: it resets exactly the overridden entries.
/// It is also what would let a future default-table setter preserve per-index OSC
/// overrides, the documented OPT_COLOR_PALETTE contract.
#[derive(Debug, Clone)]
pub(crate) struct PaletteState {
    pub(crate) current: Vec<Rgb>,
    default: Vec<Rgb>,
    overridden: Vec<bool>,
}

impl Default for PaletteState {
    fn default() -> Self {
        let table = default_palette();
        PaletteState { current: table.clone(), default: table, overridden: vec![false; 256] }
    }
}

impl PaletteState {
    pub(crate) fn set(&mut self, index: u8, color: Rgb) {
        self.current[index as usize] = color;
        self.overridden[index as usize] = true;
    }

    pub(crate) fn reset(&mut self, index: u8) {
        self.current[index as usize] = self.default[index as usize];
        self.overridden[index as usize] = false;
    }

    /// OSC 104 with no arguments: only masked entries move, exactly the oracle's
    /// `reset_palette` loop over its mask.
    pub(crate) fn reset_all(&mut self) {
        for i in 0..256 {
            if self.overridden[i] {
                self.current[i] = self.default[i];
                self.overridden[i] = false;
            }
        }
    }
}

/// The whole colour state, carried across `full_reset` like the reports grant.
/// Measured, not assumed: the oracle's `fullReset` resets modes, tabs, pwd, title and
/// the screens, and leaves `colors` untouched.
#[derive(Debug, Clone, Default)]
pub(crate) struct ColorState {
    pub(crate) foreground: DynamicRgb,
    pub(crate) background: DynamicRgb,
    pub(crate) cursor: DynamicRgb,
    pub(crate) palette: PaletteState,
}

/// Parses one colour spec, mirroring the oracle's `RGB.parse` minus X11 names.
///
/// Accepted: `#` + 3/6/9/12 hex digits (n digits per channel scale as
/// `v * 255 / (16^n - 1)`), bare 3/6 hex digits (Ghostty config compatibility),
/// `rgb:h/h/h` with 1-4 digits per channel, `rgbi:f/f/f` with floats in 0..=1
/// (`v * 255`, truncating toward zero exactly as the oracle's `@intFromFloat`).
pub(crate) fn parse_spec(spec: &str) -> Option<Rgb> {
    let input = spec.trim_matches([' ', '\t']);
    if input.is_empty() {
        return None;
    }

    if let Some(hex) = input.strip_prefix('#') {
        let per_channel = match hex.len() {
            3 => 1,
            6 => 2,
            9 => 3,
            12 => 4,
            _ => return None,
        };
        return rgb_from_hex_triplet(hex, per_channel);
    }

    match input.len() {
        3 => return rgb_from_hex_triplet(input, 1),
        6 => return rgb_from_hex_triplet(input, 2),
        _ => {}
    }

    let rest = input.strip_prefix("rgb")?;
    let (intensity, rest) = match rest.strip_prefix("i:") {
        Some(r) => (true, r),
        None => (false, rest.strip_prefix(':')?),
    };
    let mut channels = rest.split('/');
    let (r, g, b) = (channels.next()?, channels.next()?, channels.next()?);
    if channels.next().is_some() {
        return None;
    }
    if intensity {
        Some(Rgb {
            r: intensity_channel(r)?,
            g: intensity_channel(g)?,
            b: intensity_channel(b)?,
        })
    } else {
        Some(Rgb { r: hex_channel(r)?, g: hex_channel(g)?, b: hex_channel(b)? })
    }
}

fn rgb_from_hex_triplet(hex: &str, per_channel: usize) -> Option<Rgb> {
    if !hex.is_ascii() {
        return None;
    }
    let (r, rest) = hex.split_at(per_channel);
    let (g, b) = rest.split_at(per_channel);
    Some(Rgb { r: hex_channel(r)?, g: hex_channel(g)?, b: hex_channel(b)? })
}

/// `fromHex`: parse 1-4 hex digits, scale by the maximum for that width. The integer
/// division happens LAST (`v * 255 / max`), matching the oracle's order.
fn hex_channel(digits: &str) -> Option<u8> {
    if digits.is_empty() || digits.len() > 4 {
        return None;
    }
    let value = u32::from_str_radix(digits, 16).ok()?;
    let max: u32 = (1u32 << (4 * digits.len() as u32)) - 1;
    Some((value * 255 / max) as u8)
}

/// `fromIntensity`: a float in 0..=1, times 255, truncated.
fn intensity_channel(digits: &str) -> Option<u8> {
    let value: f64 = digits.parse().ok()?;
    if !(0.0..=1.0).contains(&value) {
        return None;
    }
    Some((value * 255.0) as u8)
}

impl crate::terminal::State {
    /// OSC 4/5/10-19/104/105/110-119, routed here by number from `osc_dispatch`.
    ///
    /// `args` are the params AFTER the operation number. Empty params are dropped
    /// before parsing, mirroring the oracle's separator-skipping tokenizer; a
    /// non-UTF-8 param becomes an unparseable token, which lands in the same error
    /// path a garbage spec does.
    pub(crate) fn osc_color(&mut self, op: u16, args: &[&[u8]], bell_terminated: bool) {
        // Replies echo the terminator the command arrived with (the oracle's parser
        // records it per command; esctest sends BEL and reads BEL back).
        let terminator: &'static str = if bell_terminated { "\x07" } else { "\x1b\\" };
        let args: Vec<&str> = args
            .iter()
            .filter(|p| !p.is_empty())
            .map(|p| std::str::from_utf8(p).unwrap_or("\u{FFFD}"))
            .collect();

        match op {
            4 => self.osc4_get_set(&args, terminator),
            // OSC 5 targets the xterm "special" colours (bold/underline/...), which
            // the oracle parses and then ignores on set, and answers nothing on
            // query (`colorOperation`: `.special => {}`). Parsing for side effects
            // only would be theatre; the observable behaviour is: nothing.
            5 => {}
            10..=19 => self.osc_dynamic_get_set(op, &args, terminator),
            104 => self.osc104_reset(&args),
            // OSC 105 resets specials: same observable no-op as OSC 5.
            105 => {}
            110..=119 => {
                // The oracle refuses the whole command if ANY argument is present.
                if args.is_empty() {
                    match op {
                        110 => self.colors.foreground.reset(),
                        111 => self.colors.background.reset(),
                        112 => self.colors.cursor.reset(),
                        // 113-119: pointer/tektronix/highlight, tracked by nobody.
                        _ => {}
                    }
                }
            }
            _ => unreachable!("osc_dispatch routes only the colour family here"),
        }
    }

    /// OSC 4: repeating `index;spec` pairs. Any error STOPS the list, keeping what
    /// parsed before it (xterm's ChangeAnsiColorRequest rule, kept by the oracle).
    fn osc4_get_set(&mut self, args: &[&str], terminator: &'static str) {
        let mut pairs = args.chunks_exact(2);
        for pair in &mut pairs {
            let Ok(index) = pair[0].parse::<u16>() else { return };
            let target = match index {
                0..=255 => Some(index as u8),
                // 256-260 address the specials (palette length + Special enum);
                // valid to name, observable effect nil. Anything else fails the
                // oracle's u9/enum casts and stops the list.
                256..=260 => None,
                _ => return,
            };
            if pair[1] == "?" {
                if let Some(i) = target {
                    let color = self.colors.palette.current[i as usize];
                    self.push_color_report(4, Some(i), color, terminator);
                }
                continue;
            }
            let Some(color) = parse_spec(pair[1]) else { return };
            if let Some(i) = target {
                self.colors.palette.set(i, color);
            }
        }
        // An unpaired trailing index is simply dropped, exactly like the oracle's
        // `it.next() orelse return result` on the missing spec.
    }

    /// OSC 10-19: each successive spec addresses the NEXT dynamic colour, so
    /// `OSC 10;a;b` sets foreground then background. Only 10/11/12 are backed by
    /// state; 13-19 parse (and can stop the list) but change nothing.
    fn osc_dynamic_get_set(&mut self, start: u16, args: &[&str], terminator: &'static str) {
        for (offset, spec) in args.iter().enumerate() {
            let target = start + offset as u16;
            if target > 19 {
                return;
            }
            if *spec == "?" {
                // xterm's report rule (`colorForXterm`): the cursor falls back to
                // the foreground; an unset colour answers nothing at all.
                let color = match target {
                    10 => self.colors.foreground.get(),
                    11 => self.colors.background.get(),
                    12 => self.colors.cursor.get().or(self.colors.foreground.get()),
                    _ => None,
                };
                if let Some(color) = color {
                    self.push_color_report(target, None, color, terminator);
                }
            } else {
                let Some(color) = parse_spec(spec) else { return };
                match target {
                    10 => self.colors.foreground.set(color),
                    11 => self.colors.background.set(color),
                    12 => self.colors.cursor.set(color),
                    _ => {}
                }
            }
        }
    }

    /// OSC 104: with arguments, reset the named indices, skipping unparseable ones
    /// (the oracle is deliberately laxer than xterm here and continues); with no
    /// arguments, reset exactly the overridden entries.
    fn osc104_reset(&mut self, args: &[&str]) {
        if args.is_empty() {
            self.colors.palette.reset_all();
            return;
        }
        for arg in args {
            let Ok(index) = arg.parse::<u16>() else { continue };
            if index <= 255 {
                self.colors.palette.reset(index as u8);
            }
        }
    }

    /// One xterm colour report: `OSC 4 ; i ; rgb:rrrr/gggg/bbbb ST` for palette
    /// queries, `OSC n ; rgb:... ST` for dynamic ones. Channels are the stored
    /// 8-bit value times 257 (`encodeRgb16`), and the terminator echoes the query's.
    fn push_color_report(
        &mut self,
        osc: u16,
        palette_index: Option<u8>,
        color: Rgb,
        terminator: &'static str,
    ) {
        use std::fmt::Write as _;
        let mut reply = String::with_capacity(32);
        match palette_index {
            Some(i) => {
                let _ = write!(reply, "\x1b]{osc};{i};");
            }
            None => {
                let _ = write!(reply, "\x1b]{osc};");
            }
        }
        let _ = write!(
            reply,
            "rgb:{:04x}/{:04x}/{:04x}{terminator}",
            u16::from(color.r) * 257,
            u16::from(color.g) * 257,
            u16::from(color.b) * 257,
        );
        self.replies.extend_from_slice(reply.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_forms_scale_like_the_oracle_not_like_xterm() {
        // The load-bearing divergence from esctest's expectations: one hex digit
        // scales by 15, so f stores 255 where xterm stores 0xF0. Mirroring the
        // oracle is the deliberate choice; this test is where it is pinned.
        assert_eq!(parse_spec("#fff"), Some(Rgb { r: 255, g: 255, b: 255 }));
        assert_eq!(parse_spec("#888"), Some(Rgb { r: 0x88, g: 0x88, b: 0x88 }));
        assert_eq!(parse_spec("#ff0000"), Some(Rgb { r: 255, g: 0, b: 0 }));
        // 12- and 16-bit channels do NOT round-trip to xterm's truncation: f00 is
        // 3840, and 3840*255/4095 = 239 (0xEF), not 0xF0. The formula is the spec.
        assert_eq!(parse_spec("#f00f00f00"), Some(Rgb { r: 0xEF, g: 0xEF, b: 0xEF }));
        assert_eq!(parse_spec("#f000f000f000"), Some(Rgb { r: 0xEF, g: 0xEF, b: 0xEF }));
        // Bare forms, Ghostty config compatibility.
        assert_eq!(parse_spec("abc"), parse_spec("#abc"));
        assert_eq!(parse_spec("aabbcc"), Some(Rgb { r: 0xAA, g: 0xBB, b: 0xCC }));
    }

    #[test]
    fn rgb_forms_accept_one_to_four_digits_per_channel() {
        assert_eq!(parse_spec("rgb:12/34/56"), Some(Rgb { r: 0x12, g: 0x34, b: 0x56 }));
        assert_eq!(parse_spec("rgb:f/f/f"), Some(Rgb { r: 255, g: 255, b: 255 }));
        assert_eq!(parse_spec("rgb:f0f0/f0f0/f0f0"), Some(Rgb { r: 0xF0, g: 0xF0, b: 0xF0 }));
        assert_eq!(parse_spec("rgb:1212/3434/5656"), Some(Rgb { r: 0x12, g: 0x34, b: 0x56 }));
        assert_eq!(parse_spec("rgbi:1/1/1"), Some(Rgb { r: 255, g: 255, b: 255 }));
        assert_eq!(parse_spec("rgbi:0.5/0.5/0.5"), Some(Rgb { r: 127, g: 127, b: 127 }));
    }

    #[test]
    fn malformed_specs_are_refused() {
        for bad in
            ["", "?", "#ff00", "rgb:12/34", "rgb:12/34/56/78", "rgbi:2/0/0", "rgb:fffff/0/0",
             "ForestGreen"]
        {
            assert_eq!(parse_spec(bad), None, "{bad:?} must not parse");
        }
    }

    fn replies_for(bytes: &[u8]) -> Vec<u8> {
        let mut terminal = crate::terminal::Terminal::new(20, 10);
        terminal.write(bytes);
        terminal.take_replies()
    }

    /// The esctest contract, byte for byte: `ReadOSC("4")` sees `;{i};rgb:rrrr/gggg/bbbb`
    /// with 16-bit channels (`stored * 257`), and the reply echoes the query's
    /// terminator. Index 1's default is the measured Tomorrow red CC6666.
    #[test]
    fn a_palette_query_reports_sixteen_bit_channels_and_echoes_the_terminator() {
        assert_eq!(replies_for(b"\x1b]4;1;?\x07"), b"\x1b]4;1;rgb:cccc/6666/6666\x07");
        assert_eq!(replies_for(b"\x1b]4;1;?\x1b\\"), b"\x1b]4;1;rgb:cccc/6666/6666\x1b\\");
        assert_eq!(
            replies_for(b"\x1b]4;1;#ff0000\x07\x1b]4;1;?\x07"),
            b"\x1b]4;1;rgb:ffff/0000/0000\x07"
        );
    }

    /// Dynamic queries answer only what exists: no override and no default is silence,
    /// not a zero-filled reply -- and the cursor falls back to the foreground, the one
    /// place xterm's report rule differs from the DATA getter.
    #[test]
    fn dynamic_queries_answer_the_set_and_stay_silent_on_the_unset() {
        assert_eq!(replies_for(b"\x1b]10;?\x07"), b"");
        assert_eq!(replies_for(b"\x1b]12;?\x07"), b"");
        assert_eq!(
            replies_for(b"\x1b]10;#20df80\x07\x1b]10;?\x07"),
            b"\x1b]10;rgb:2020/dfdf/8080\x07"
        );
        assert_eq!(
            replies_for(b"\x1b]10;#20df80\x07\x1b]12;?\x07"),
            b"\x1b]12;rgb:2020/dfdf/8080\x07",
            "an unset cursor reports the foreground"
        );
        assert_eq!(
            replies_for(b"\x1b]12;#c0ffee\x07\x1b]12;?\x07"),
            b"\x1b]12;rgb:c0c0/ffff/eeee\x07",
            "a set cursor reports itself"
        );
    }

    /// esctest's Multiple case: one OSC 10 with two specs walks fg then bg, and one
    /// query with two `?` reads them back as two OSC replies numbered 10 and 11.
    #[test]
    fn a_second_parameter_walks_to_the_next_dynamic_colour() {
        assert_eq!(
            replies_for(b"\x1b]10;#111111;#222222\x07\x1b]10;?;?\x07"),
            b"\x1b]10;rgb:1111/1111/1111\x07\x1b]11;rgb:2222/2222/2222\x07"
        );
    }

    /// OSC 104 with no arguments resets exactly the overridden entries; OSC 110
    /// clears the foreground override so its query goes silent again.
    #[test]
    fn resets_return_to_the_default_table_and_to_silence() {
        assert_eq!(
            replies_for(b"\x1b]4;1;#ff0000\x07\x1b]104\x07\x1b]4;1;?\x07"),
            b"\x1b]4;1;rgb:cccc/6666/6666\x07"
        );
        assert_eq!(replies_for(b"\x1b]10;#20df80\x07\x1b]110\x07\x1b]10;?\x07"), b"");
    }

    /// RIS keeps colour state -- the oracle's fullReset never touches colors. The
    /// query after `ESC c` still reports the override.
    #[test]
    fn a_full_reset_keeps_colour_overrides() {
        assert_eq!(
            replies_for(b"\x1b]4;1;#ff0000\x07\x1bc\x1b]4;1;?\x07"),
            b"\x1b]4;1;rgb:ffff/0000/0000\x07"
        );
    }

    #[test]
    fn the_mask_is_what_a_bare_reset_walks() {
        let mut palette = PaletteState::default();
        let red = Rgb { r: 255, g: 0, b: 0 };
        palette.set(1, red);
        palette.set(200, red);
        assert_eq!(palette.current[1], red);
        palette.reset_all();
        assert_eq!(palette.current[1], palette.default[1]);
        assert_eq!(palette.current[200], palette.default[200]);
        assert!(!palette.overridden.iter().any(|&b| b));
    }
}
