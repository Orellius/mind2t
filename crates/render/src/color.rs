//! Purpose: turn a cell's style into the two colours it is actually drawn with.
//! Public surface: `Rgba`, `Palette`, `Drawn`.
//! Why this file: the attribute-to-colour rules are small but order-dependent, and getting
//!   the order wrong is invisible until a specific combination appears. Inverse has to be
//!   applied AFTER the defaults resolve (otherwise inverting a default-on-default cell is a
//!   no-op instead of a swap, and vim's status line vanishes), and invisible has to be
//!   applied after inverse (otherwise an inverse-invisible cell shows its text).
//! NOT responsible for: where the colours are painted (`canvas.rs`, `renderer.rs`).
//! Test strategy: unit tests below pin the ordering rules, which is the part that breaks.

use ruuah_vt_snapshot::{Color, Style};

/// Straight (non-premultiplied) 8-bit RGBA.
pub type Rgba = [u8; 4];

/// What a cell actually gets painted with, after every attribute has had its say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Drawn {
    pub foreground: Rgba,
    pub background: Rgba,
    /// False when the glyph must not be drawn at all, even though a background still is.
    pub ink: bool,
    pub underline: bool,
    pub strikethrough: bool,
}

/// The 256-colour table plus the two defaults.
#[derive(Debug, Clone)]
pub struct Palette {
    colors: [Rgba; 256],
    pub default_foreground: Rgba,
    pub default_background: Rgba,
}

impl Palette {
    /// The default scheme: Ghostty's sixteen named colours and defaults (measured from
    /// the oracle's `color.zig` and `Config.zig`, 2026-07-29), then the standard 6x6x6
    /// cube and 24-step grey ramp both palettes share.
    ///
    /// The original xterm primaries (`CD0000` red and friends) were the first version
    /// here, and the first live Hebrew session called the result "faded" -- harsh 1994
    /// phosphor values plus a D0D0D0 default foreground read as washed out next to any
    /// modern terminal. The fix is the same scheme the user's daily driver shows.
    pub fn xterm() -> Palette {
        let mut colors = [[0, 0, 0, 255]; 256];

        const SYSTEM: [[u8; 3]; 16] = [
            [0x1d, 0x1f, 0x21],
            [0xcc, 0x66, 0x66],
            [0xb5, 0xbd, 0x68],
            [0xf0, 0xc6, 0x74],
            [0x81, 0xa2, 0xbe],
            [0xb2, 0x94, 0xbb],
            [0x8a, 0xbe, 0xb7],
            [0xc5, 0xc8, 0xc6],
            [0x66, 0x66, 0x66],
            [0xd5, 0x4e, 0x53],
            [0xb9, 0xca, 0x4a],
            [0xe7, 0xc5, 0x47],
            [0x7a, 0xa6, 0xda],
            [0xc3, 0x97, 0xd8],
            [0x70, 0xc0, 0xb1],
            [0xea, 0xea, 0xea],
        ];
        for (index, rgb) in SYSTEM.iter().enumerate() {
            colors[index] = [rgb[0], rgb[1], rgb[2], 255];
        }

        const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
        for index in 0..216usize {
            colors[16 + index] = [
                STEPS[index / 36],
                STEPS[(index / 6) % 6],
                STEPS[index % 6],
                255,
            ];
        }

        for index in 0..24usize {
            let level = 8 + index as u8 * 10;
            colors[232 + index] = [level, level, level, 255];
        }

        Palette {
            colors,
            // Black-and-gray by Orel's call (2026-07-29): near-black ground, pure white
            // ink, the sixteen stay vivid. Chrome accents live in the Swift layer.
            default_foreground: [0xff, 0xff, 0xff, 255],
            default_background: [0x0d, 0x0d, 0x0d, 255],
        }
    }

    pub fn indexed(&self, index: u8) -> Rgba {
        self.colors[usize::from(index)]
    }

    fn resolve(&self, color: Color, fallback: Rgba) -> Rgba {
        match color {
            Color::Default => fallback,
            Color::Palette(index) => self.indexed(index),
            Color::Rgb { r, g, b } => [r, g, b, 255],
        }
    }

    /// Applies every attribute that changes what a cell looks like.
    pub fn draw(&self, style: &Style) -> Drawn {
        let mut foreground = self.resolve(style.fg, self.default_foreground);
        let mut background = self.resolve(style.bg, self.default_background);

        // Bold brightens only the low eight palette entries, which is the convention every
        // terminal follows and the reason bold red and bright red look the same.
        if style.bold {
            if let Color::Palette(index @ 0..=7) = style.fg {
                foreground = self.indexed(index + 8);
            }
        }

        // After the defaults have resolved, or inverting a default-on-default cell would do
        // nothing and vim's status line would disappear.
        if style.inverse {
            std::mem::swap(&mut foreground, &mut background);
        }

        Drawn {
            foreground,
            background,
            // After inverse, so an inverse-invisible cell still hides its text.
            ink: !style.invisible,
            underline: style.underline != ruuah_vt_snapshot::Underline::None,
            strikethrough: style.strikethrough,
        }
    }
}

impl Default for Palette {
    fn default() -> Palette {
        Palette::xterm()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruuah_vt_snapshot::Underline;

    #[test]
    fn the_cube_and_the_grey_ramp_land_where_xterm_puts_them() {
        let palette = Palette::xterm();
        assert_eq!(palette.indexed(16), [0, 0, 0, 255]);
        assert_eq!(palette.indexed(231), [255, 255, 255, 255]);
        assert_eq!(palette.indexed(232), [8, 8, 8, 255]);
        assert_eq!(palette.indexed(255), [238, 238, 238, 255]);
        assert_eq!(
            palette.indexed(196),
            [255, 0, 0, 255],
            "the classic bright red"
        );
    }

    #[test]
    fn inverse_on_a_default_cell_swaps_the_defaults() {
        // The one that matters: a status line is usually plain inverse with no colours set,
        // so an implementation that inverts before resolving draws nothing at all.
        let palette = Palette::xterm();
        let drawn = palette.draw(&Style {
            inverse: true,
            ..Style::DEFAULT
        });

        assert_eq!(drawn.foreground, palette.default_background);
        assert_eq!(drawn.background, palette.default_foreground);
    }

    #[test]
    fn invisible_survives_inverse() {
        let palette = Palette::xterm();
        let drawn = palette.draw(&Style {
            inverse: true,
            invisible: true,
            ..Style::DEFAULT
        });
        assert!(!drawn.ink);
    }

    #[test]
    fn bold_brightens_only_the_low_eight() {
        let palette = Palette::xterm();
        let bold_red = palette.draw(&Style {
            fg: Color::Palette(1),
            bold: true,
            ..Style::DEFAULT
        });
        assert_eq!(bold_red.foreground, palette.indexed(9));

        let bold_cube = palette.draw(&Style {
            fg: Color::Palette(100),
            bold: true,
            ..Style::DEFAULT
        });
        assert_eq!(bold_cube.foreground, palette.indexed(100), "unchanged");
    }

    #[test]
    fn rgb_passes_through_untouched() {
        let palette = Palette::xterm();
        let drawn = palette.draw(&Style {
            fg: Color::Rgb { r: 1, g: 2, b: 3 },
            underline: Underline::Curly,
            ..Style::DEFAULT
        });
        assert_eq!(drawn.foreground, [1, 2, 3, 255]);
        assert!(drawn.underline);
    }
}
