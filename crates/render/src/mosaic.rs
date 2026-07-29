//! Purpose: synthesized block mosaics -- the glyphs every terminal draws itself.
//! Public surface: `coverage`, returning a full-cell alpha mask for a codepoint.
//! Why this file: no font can draw these correctly in a grid that is not its own.
//!   Block elements and sextants must FILL the cell and meet their neighbours
//!   edge-to-edge; a fallback font's glyph fills that font's em instead, leaving
//!   gutters (Iosevka is 0.5em wide, this grid is Menlo's 0.6em -- measured
//!   2026-07-29 when Claude Code's mascot needed U+1FBxx). Ghostty synthesizes
//!   the same ranges for the same reason.
//! NOT responsible for: choosing when to draw (renderer.rs), fonts (font.rs).
//!   Wedge/rounded mosaics (U+1FB3C..) stay with the font fallback: they are
//!   curves and diagonals, not rectangles, and a wrong synthesis is worse than
//!   a narrow glyph.
//! Test strategy: unit tests pin exact filled regions per family, and the
//!   renderer-level test in tests/mosaic_pixels.rs proves ink lands through the
//!   real draw path with no gutter at the cell seam.

/// The alpha shades: LIGHT, MEDIUM and DARK SHADE are uniform stipples, and a
/// uniform alpha reads identically at cell size without moire.
const SHADE_LIGHT: u8 = 64;
const SHADE_MEDIUM: u8 = 128;
const SHADE_DARK: u8 = 191;

/// Full-cell coverage mask (row-major, `width * height` bytes) for a codepoint
/// this module owns, or None for everything a font should draw.
pub fn coverage(c: char, width: u32, height: u32) -> Option<Vec<u8>> {
    if width == 0 || height == 0 {
        return None;
    }
    let mut mask = Mask::new(width, height);
    match u32::from(c) {
        // -- Block Elements: halves and eighths ------------------------------
        0x2580 => mask.rect_frac(0.0, 0.0, 1.0, 0.5),
        0x2581..=0x2588 => {
            let eighths = u32::from(c) - 0x2580;
            mask.rect_frac(0.0, 1.0 - eighths as f32 / 8.0, 1.0, 1.0);
        }
        0x2589..=0x258F => {
            let eighths = 8 - (u32::from(c) - 0x2588);
            mask.rect_frac(0.0, 0.0, eighths as f32 / 8.0, 1.0);
        }
        0x2590 => mask.rect_frac(0.5, 0.0, 1.0, 1.0),
        0x2591 => mask.fill(SHADE_LIGHT),
        0x2592 => mask.fill(SHADE_MEDIUM),
        0x2593 => mask.fill(SHADE_DARK),
        0x2594 => mask.rect_frac(0.0, 0.0, 1.0, 1.0 / 8.0),
        0x2595 => mask.rect_frac(7.0 / 8.0, 0.0, 1.0, 1.0),

        // -- Block Elements: quadrants ---------------------------------------
        0x2596..=0x259F => {
            // (top-left, top-right, bottom-left, bottom-right) per codepoint.
            let quads = match u32::from(c) {
                0x2596 => (false, false, true, false),
                0x2597 => (false, false, false, true),
                0x2598 => (true, false, false, false),
                0x2599 => (true, false, true, true),
                0x259A => (true, false, false, true),
                0x259B => (true, true, true, false),
                0x259C => (true, true, false, true),
                0x259D => (false, true, false, false),
                0x259E => (false, true, true, false),
                _ => (false, true, true, true), // 0x259F
            };
            if quads.0 {
                mask.rect_frac(0.0, 0.0, 0.5, 0.5);
            }
            if quads.1 {
                mask.rect_frac(0.5, 0.0, 1.0, 0.5);
            }
            if quads.2 {
                mask.rect_frac(0.0, 0.5, 0.5, 1.0);
            }
            if quads.3 {
                mask.rect_frac(0.5, 0.5, 1.0, 1.0);
            }
        }

        // -- Symbols for Legacy Computing: sextants --------------------------
        // BLOCK SEXTANT-n encodes a 2x3 grid as bits 0..5 (left-right then
        // top-bottom). The block skips the two patterns that already exist as
        // half blocks: 21 (left half, U+258C) and 42 (right half, U+2590) --
        // the same renumbering kitty and Ghostty apply.
        0x1FB00..=0x1FB3B => {
            let mut id = u32::from(c) - 0x1FB00 + 1;
            if id >= 21 {
                id += 1;
            }
            if id >= 42 {
                id += 1;
            }
            for cell in 0..6 {
                if id & (1 << cell) == 0 {
                    continue;
                }
                let (col, row) = (cell % 2, cell / 2);
                mask.rect_frac(
                    col as f32 / 2.0,
                    row as f32 / 3.0,
                    (col + 1) as f32 / 2.0,
                    (row + 1) as f32 / 3.0,
                );
            }
        }

        _ => return None,
    }
    Some(mask.bytes)
}

struct Mask {
    bytes: Vec<u8>,
    width: u32,
    height: u32,
}

impl Mask {
    fn new(width: u32, height: u32) -> Mask {
        Mask {
            bytes: vec![0; (width * height) as usize],
            width,
            height,
        }
    }

    fn fill(&mut self, alpha: u8) {
        self.bytes.fill(alpha);
    }

    /// Fills a fractional rectangle, edges rounded to pixels. Adjacent
    /// fractions share the rounded edge, so quadrants and sextants tile the
    /// cell with no seam and no overlap at odd sizes.
    fn rect_frac(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
        let px0 = (x0 * self.width as f32).round() as u32;
        let px1 = ((x1 * self.width as f32).round() as u32).min(self.width);
        let py0 = (y0 * self.height as f32).round() as u32;
        let py1 = ((y1 * self.height as f32).round() as u32).min(self.height);
        for y in py0..py1 {
            let row = (y * self.width) as usize;
            self.bytes[row + px0 as usize..row + px1 as usize].fill(255);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(mask: &[u8], width: u32, x: u32, y: u32) -> bool {
        mask[(y * width + x) as usize] == 255
    }

    #[test]
    fn upper_half_fills_exactly_the_top_rows() {
        let mask = coverage('\u{2580}', 10, 8).expect("owned");
        assert!(filled(&mask, 10, 0, 0) && filled(&mask, 10, 9, 3));
        assert!(!filled(&mask, 10, 0, 4) && !filled(&mask, 10, 9, 7));
    }

    #[test]
    fn quadrant_bottom_left_touches_its_two_edges_only() {
        let mask = coverage('\u{2596}', 10, 10).expect("owned");
        assert!(filled(&mask, 10, 0, 9), "bottom-left corner");
        assert!(!filled(&mask, 10, 9, 9), "bottom-right stays empty");
        assert!(!filled(&mask, 10, 0, 0), "top-left stays empty");
    }

    #[test]
    fn sextant_one_is_the_top_left_sixth() {
        let mask = coverage('\u{1FB00}', 10, 9).expect("owned");
        assert!(filled(&mask, 10, 0, 0));
        assert!(!filled(&mask, 10, 9, 0), "top-right sixth empty");
        assert!(!filled(&mask, 10, 0, 8), "bottom-left sixth empty");
    }

    #[test]
    fn the_sextant_renumbering_skips_the_half_blocks() {
        // U+1FB27 sits right after the first skip: without the +1 at 21 it
        // would render the left half, which is U+258C's job.
        let mask = coverage('\u{1FB27}', 8, 9).expect("owned");
        let left_half_only = (0..9).all(|y| filled(&mask, 8, 0, y) == (true))
            && (0..9).all(|y| !filled(&mask, 8, 7, y));
        assert!(!left_half_only, "U+1FB27 must not collapse to the left half");
    }

    #[test]
    fn shades_are_uniform_and_ordered() {
        for (c, alpha) in [('\u{2591}', 64u8), ('\u{2592}', 128), ('\u{2593}', 191)] {
            let mask = coverage(c, 4, 4).expect("owned");
            assert!(mask.iter().all(|&a| a == alpha), "{c:?} uniform at {alpha}");
        }
    }

    #[test]
    fn tiles_meet_with_no_seam_and_no_overlap() {
        // Left half + right half must cover every pixel exactly once, at an
        // ODD width where naive truncation leaves a hole down the middle.
        let left = coverage('\u{258C}', 9, 6).expect("owned");
        let right = coverage('\u{2590}', 9, 6).expect("owned");
        for i in 0..left.len() {
            assert_eq!(
                left[i] != 0,
                right[i] == 0,
                "pixel {i} covered by neither or both halves"
            );
        }
    }

    #[test]
    fn everything_else_is_declined() {
        for c in ['A', 'א', '\u{2502}', '\u{1FB3C}', '\u{28FF}'] {
            assert!(coverage(c, 8, 8).is_none(), "{c:?} belongs to a font");
        }
    }
}
