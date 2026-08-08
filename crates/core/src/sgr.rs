//! Purpose: turn an SGR parameter list into a mutation of the current pen style.
//! Public surface: `apply`.
//! Why this file: SGR is the single largest branch in `csi_dispatch` and has its own
//!   grammar -- colours arrive in two incompatible shapes (`38;2;r;g;b` with semicolons and
//!   `38:2:r:g:b` with sub-parameters) and both appear in the wild. Keeping it out of the
//!   dispatch table keeps `terminal.rs` readable.
//! NOT responsible for: parsing the escape sequence (`vte` does that) or applying the
//!   result to cells (`terminal.rs` holds the pen).
//! Test strategy: unit tests below for each shape; the corpus checks it against Ghostty.

use mind2t_vt_snapshot::{Color, Style, Underline};
use vte::Params;

/// Applies an SGR sequence to `style`.
///
/// An empty parameter list means SGR 0: `CSI m` is a full reset, not a no-op.
pub fn apply(style: &mut Style, params: &Params) {
    let items: Vec<&[u16]> = params.iter().collect();
    if items.is_empty() {
        *style = Style::DEFAULT;
        return;
    }

    let mut i = 0;
    while i < items.len() {
        let item = items[i];
        let code = item.first().copied().unwrap_or(0);
        match code {
            0 => *style = Style::DEFAULT,
            1 => style.bold = true,
            2 => style.faint = true,
            3 => style.italic = true,
            4 => style.underline = underline_from(item),
            5 | 6 => style.blink = true,
            7 => style.inverse = true,
            8 => style.invisible = true,
            9 => style.strikethrough = true,
            21 => style.underline = Underline::Double,
            22 => {
                style.bold = false;
                style.faint = false;
            }
            23 => style.italic = false,
            24 => style.underline = Underline::None,
            25 => style.blink = false,
            27 => style.inverse = false,
            28 => style.invisible = false,
            29 => style.strikethrough = false,
            30..=37 => style.fg = Color::Palette((code - 30) as u8),
            38 => {
                if let Some((color, consumed)) = extended_color(&items, i) {
                    style.fg = color;
                    i += consumed;
                }
            }
            39 => style.fg = Color::Default,
            40..=47 => style.bg = Color::Palette((code - 40) as u8),
            48 => {
                if let Some((color, consumed)) = extended_color(&items, i) {
                    style.bg = color;
                    i += consumed;
                }
            }
            49 => style.bg = Color::Default,
            53 => style.overline = true,
            55 => style.overline = false,
            58 => {
                if let Some((color, consumed)) = extended_color(&items, i) {
                    style.underline_color = color;
                    i += consumed;
                }
            }
            59 => style.underline_color = Color::Default,
            90..=97 => style.fg = Color::Palette((code - 90 + 8) as u8),
            100..=107 => style.bg = Color::Palette((code - 100 + 8) as u8),
            _ => {}
        }
        i += 1;
    }
}

/// `4` alone is a single underline; `4:n` selects a style. An unknown `n` reads as single,
/// matching the general rule that an unrecognised SGR sub-parameter degrades rather than
/// disabling the attribute outright.
fn underline_from(item: &[u16]) -> Underline {
    match item.get(1).copied() {
        None => Underline::Single,
        Some(0) => Underline::None,
        Some(1) => Underline::Single,
        Some(2) => Underline::Double,
        Some(3) => Underline::Curly,
        Some(4) => Underline::Dotted,
        Some(5) => Underline::Dashed,
        Some(_) => Underline::Single,
    }
}

/// Reads a 256-colour or direct-RGB argument following a 38/48/58 selector.
///
/// Returns the colour and how many *extra* parameter slots it consumed. Both encodings are
/// handled: sub-parameters (`38:2:r:g:b`, consuming nothing extra because it is one slot)
/// and separate parameters (`38;2;r;g;b`, consuming four).
fn extended_color(items: &[&[u16]], at: usize) -> Option<(Color, usize)> {
    let item = items[at];
    if item.len() > 1 {
        return color_from_subparams(&item[1..]).map(|color| (color, 0));
    }

    match first(items, at + 1)? {
        5 => Some((Color::Palette(channel(first(items, at + 2)?)), 2)),
        2 => {
            let r = channel(first(items, at + 2)?);
            let g = channel(first(items, at + 3)?);
            let b = channel(first(items, at + 4)?);
            Some((Color::Rgb { r, g, b }, 4))
        }
        _ => None,
    }
}

/// The colon form. Some emitters include an empty colour-space slot (`38:2::r:g:b`), so the
/// RGB triple is read from the end rather than from a fixed offset.
fn color_from_subparams(sub: &[u16]) -> Option<Color> {
    match sub.first().copied()? {
        5 => Some(Color::Palette(channel(sub.get(1).copied()?))),
        2 if sub.len() >= 4 => {
            let rgb = &sub[sub.len() - 3..];
            Some(Color::Rgb {
                r: channel(rgb[0]),
                g: channel(rgb[1]),
                b: channel(rgb[2]),
            })
        }
        _ => None,
    }
}

fn first(items: &[&[u16]], at: usize) -> Option<u16> {
    items.get(at)?.first().copied()
}

fn channel(value: u16) -> u8 {
    u8::try_from(value).unwrap_or(u8::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drives the real `vte` parser so the tests exercise the parameter shapes the parser
    /// actually produces, rather than a hand-built `Params` that might not match.
    fn style_after(sequence: &[u8]) -> Style {
        struct Capture(Style);
        impl vte::Perform for Capture {
            fn csi_dispatch(&mut self, params: &Params, _: &[u8], _: bool, action: char) {
                if action == 'm' {
                    apply(&mut self.0, params);
                }
            }
        }
        let mut capture = Capture(Style::DEFAULT);
        vte::Parser::new().advance(&mut capture, sequence);
        capture.0
    }

    #[test]
    fn bare_csi_m_is_a_full_reset() {
        let style = style_after(b"\x1b[1;3m\x1b[m");
        assert_eq!(style, Style::DEFAULT);
    }

    #[test]
    fn attributes_accumulate_and_reset_individually() {
        let style = style_after(b"\x1b[1;3;9m\x1b[23m");
        assert!(style.bold);
        assert!(!style.italic, "SGR 23 clears italic only");
        assert!(style.strikethrough);
    }

    #[test]
    fn sgr_22_clears_both_bold_and_faint() {
        let style = style_after(b"\x1b[1;2m\x1b[22m");
        assert!(!style.bold);
        assert!(!style.faint);
    }

    #[test]
    fn basic_and_bright_palette_colours() {
        assert_eq!(style_after(b"\x1b[31m").fg, Color::Palette(1));
        assert_eq!(style_after(b"\x1b[44m").bg, Color::Palette(4));
        assert_eq!(style_after(b"\x1b[91m").fg, Color::Palette(9));
        assert_eq!(style_after(b"\x1b[103m").bg, Color::Palette(11));
    }

    #[test]
    fn indexed_and_rgb_colours_in_semicolon_form() {
        assert_eq!(style_after(b"\x1b[38;5;200m").fg, Color::Palette(200));
        assert_eq!(
            style_after(b"\x1b[38;2;10;20;30m").fg,
            Color::Rgb {
                r: 10,
                g: 20,
                b: 30
            }
        );
        assert_eq!(
            style_after(b"\x1b[48;2;1;2;3m").bg,
            Color::Rgb { r: 1, g: 2, b: 3 }
        );
    }

    #[test]
    fn indexed_and_rgb_colours_in_colon_form() {
        assert_eq!(style_after(b"\x1b[38:5:200m").fg, Color::Palette(200));
        assert_eq!(
            style_after(b"\x1b[38:2:10:20:30m").fg,
            Color::Rgb {
                r: 10,
                g: 20,
                b: 30
            }
        );
    }

    #[test]
    fn the_colour_space_slot_in_colon_form_is_tolerated() {
        // `38:2::r:g:b` is emitted in the wild; reading the triple from the end absorbs it.
        assert_eq!(
            style_after(b"\x1b[38:2::10:20:30m").fg,
            Color::Rgb {
                r: 10,
                g: 20,
                b: 30
            }
        );
    }

    #[test]
    fn a_colour_does_not_swallow_the_parameters_after_it() {
        // The classic off-by-one: consuming the wrong count makes the trailing attribute
        // vanish, and it looks like the attribute is unimplemented rather than eaten.
        let style = style_after(b"\x1b[38;5;200;1m");
        assert_eq!(style.fg, Color::Palette(200));
        assert!(style.bold, "SGR 1 after an indexed colour must still apply");
    }

    #[test]
    fn underline_styles_and_underline_colour() {
        assert_eq!(style_after(b"\x1b[4m").underline, Underline::Single);
        assert_eq!(style_after(b"\x1b[4:3m").underline, Underline::Curly);
        assert_eq!(style_after(b"\x1b[4:0m").underline, Underline::None);
        assert_eq!(style_after(b"\x1b[21m").underline, Underline::Double);
        assert_eq!(
            style_after(b"\x1b[58;5;9m").underline_color,
            Color::Palette(9)
        );
    }

    #[test]
    fn default_colour_codes_clear_the_slot() {
        assert_eq!(style_after(b"\x1b[31m\x1b[39m").fg, Color::Default);
        assert_eq!(style_after(b"\x1b[41m\x1b[49m").bg, Color::Default);
    }

    #[test]
    fn an_unknown_code_is_ignored_without_disturbing_the_rest() {
        let style = style_after(b"\x1b[1;77;3m");
        assert!(style.bold);
        assert!(style.italic);
    }
}
