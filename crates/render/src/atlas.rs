//! Purpose: rasterize a glyph once and keep it.
//! Public surface: `Atlas`, `GlyphKey`, `Glyph`, `GlyphData`.
//! Why this file: rasterizing an outline is the expensive part of drawing a terminal and the
//!   part that repeats hardest -- a screen of text is a few dozen distinct glyphs shown
//!   thousands of times. The key is (font, glyph) rather than glyph because the stack under
//!   it is plural by necessity (see `font.rs`); a glyph id means nothing without the font
//!   that issued it, and keying on the id alone would draw Hebrew with Menlo's glyph numbers.
//!   Color emoji (sbix bitmap strikes) come out of the SAME cache as a second data kind:
//!   the strike is scaled to the cell box HERE, once, on the CPU -- so both backends receive
//!   identical pre-scaled bytes and bit-equality between them stays a statement about the
//!   blit, never about two resamplers.
//! NOT responsible for: font selection (`font.rs`), compositing (`canvas.rs`), or deciding
//!   what to draw (`renderer.rs`).
//! Test strategy: unit tests below cover the cache actually caching, a miss being cached as
//!   a miss, the same glyph from two fonts staying distinct, and an emoji rasterizing as
//!   COLOR data that fits the cell box.

use std::collections::HashMap;

use swash::scale::{Render, ScaleContext, Source, StrikeWith, image::Content};
use swash::zeno::Format;

use crate::font::FontStack;

/// What identifies a rasterized glyph. Size is absent because a stack is built at one size;
/// changing size builds a new stack and a new atlas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    pub font: u16,
    pub glyph: u16,
}

/// The pixels a glyph rasterized to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlyphData {
    /// One byte of coverage per pixel, tinted with the cell's foreground at blit time.
    Mask(Vec<u8>),
    /// Straight RGBA, four bytes per pixel, blitted as-is (emoji carry their own colors --
    /// tinting them with the foreground is the classic gray-silhouette bug).
    Color(Vec<u8>),
}

/// A rasterized glyph: its pixels plus where it sits relative to the pen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glyph {
    /// Offset from the pen position to the left edge of the bitmap.
    pub left: i32,
    /// Offset from the baseline UP to the top edge of the bitmap.
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub data: GlyphData,
}

impl Glyph {
    pub fn coverage_at(&self, x: u32, y: u32) -> u8 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        match &self.data {
            GlyphData::Mask(coverage) => coverage[(y * self.width + x) as usize],
            GlyphData::Color(rgba) => rgba[((y * self.width + x) * 4 + 3) as usize],
        }
    }
}

/// A cache of rasterized glyphs.
pub struct Atlas {
    context: ScaleContext,
    entries: HashMap<GlyphKey, Option<Glyph>>,
    rasterized: usize,
}

impl Atlas {
    pub fn new() -> Atlas {
        Atlas {
            context: ScaleContext::new(),
            entries: HashMap::new(),
            rasterized: 0,
        }
    }

    /// How many glyphs were actually put through the rasterizer.
    ///
    /// Exposed so a test can prove the cache is a cache rather than assuming it.
    pub fn rasterized(&self) -> usize {
        self.rasterized
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The rasterized glyph, drawing it on first sight.
    ///
    /// A glyph with no outline (a space, or one the font declines to render) caches as
    /// `None`, so it is asked for once and never retried.
    pub fn glyph(&mut self, stack: &FontStack, key: GlyphKey) -> Option<&Glyph> {
        if !self.entries.contains_key(&key) {
            let rendered = self.rasterize(stack, key);
            self.rasterized += 1;
            self.entries.insert(key, rendered);
        }
        self.entries.get(&key).and_then(|entry| entry.as_ref())
    }

    fn rasterize(&mut self, stack: &FontStack, key: GlyphKey) -> Option<Glyph> {
        let font = stack.face(key.font)?;
        let mut scaler = self
            .context
            .builder(font)
            .size(stack.size())
            .hint(true)
            .build();

        // Color bitmaps first: a font that has a strike for the glyph is an emoji font
        // answering with the artwork it was built around. Everything else takes the
        // outline path exactly as before.
        let image = Render::new(&[Source::ColorBitmap(StrikeWith::BestFit), Source::Outline])
            .format(Format::Alpha)
            .render(&mut scaler, key.glyph)?;

        if image.placement.width == 0 || image.placement.height == 0 {
            return None;
        }

        match image.content {
            Content::Mask => Some(Glyph {
                left: image.placement.left,
                top: image.placement.top,
                width: image.placement.width,
                height: image.placement.height,
                data: GlyphData::Mask(image.data),
            }),
            Content::Color => {
                // A strike comes back at ITS size (sbix ships fixed 20..160px bitmaps),
                // not the requested one -- scaled here, once, to fit the cell height.
                let metrics = stack.metrics();
                let (width, height, rgba) = fit_to_height(
                    image.placement.width,
                    image.placement.height,
                    &image.data,
                    metrics.height,
                );
                Some(Glyph {
                    // The blit centers color glyphs in the cell box; pen-relative
                    // placement belongs to outlines.
                    left: 0,
                    top: 0,
                    width,
                    height,
                    data: GlyphData::Color(rgba),
                })
            }
            // Subpixel masks are never requested (Format::Alpha above).
            Content::SubpixelMask => None,
        }
    }
}

/// Scales straight-RGBA pixels so their height is exactly `target_height`, preserving
/// aspect. Bilinear in 16.16 fixed point: deterministic across machines and backends,
/// which is what lets the CPU/GPU differential test stay byte-exact -- both receive the
/// SAME pre-scaled bytes.
pub(crate) fn fit_to_height(
    width: u32,
    height: u32,
    rgba: &[u8],
    target_height: u32,
) -> (u32, u32, Vec<u8>) {
    let target_height = target_height.max(1);
    if height == target_height {
        return (width, height, rgba.to_vec());
    }
    let target_width = ((width as u64 * target_height as u64) / height as u64).max(1) as u32;

    let step_x = ((width as u64) << 16) / target_width as u64;
    let step_y = ((height as u64) << 16) / target_height as u64;
    let mut out = Vec::with_capacity((target_width * target_height * 4) as usize);

    for y in 0..target_height as u64 {
        // Sample at the pixel center, clamped so the last row/column never reads past
        // the source edge.
        let sy = (y * step_y + step_y / 2).saturating_sub(1 << 15);
        let (y0, fy) = split(sy, height);
        for x in 0..target_width as u64 {
            let sx = (x * step_x + step_x / 2).saturating_sub(1 << 15);
            let (x0, fx) = split(sx, width);
            for channel in 0..4 {
                out.push(bilinear(rgba, width, height, x0, y0, fx, fy, channel));
            }
        }
    }
    (target_width, target_height, out)
}

/// A 16.16 coordinate into (integer part clamped to the source, 8-bit fraction).
pub(crate) fn split(coordinate: u64, limit: u32) -> (u32, u32) {
    let integer = ((coordinate >> 16) as u32).min(limit - 1);
    let fraction = ((coordinate >> 8) & 0xff) as u32;
    (integer, fraction)
}

/// One channel of a 2x2 bilinear tap, integer arithmetic only.
#[allow(clippy::too_many_arguments)]
pub(crate) fn bilinear(rgba: &[u8], width: u32, height: u32, x0: u32, y0: u32, fx: u32, fy: u32, channel: usize) -> u8 {
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let at = |x: u32, y: u32| u32::from(rgba[((y * width + x) * 4) as usize + channel]);

    let top = at(x0, y0) * (256 - fx) + at(x1, y0) * fx;
    let bottom = at(x0, y1) * (256 - fx) + at(x1, y1) * fx;
    (((top * (256 - fy) + bottom * fy) + (1 << 15)) >> 16) as u8
}

impl Default for Atlas {
    fn default() -> Atlas {
        Atlas::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack() -> FontStack {
        FontStack::system(16.0).expect("system fonts")
    }

    #[test]
    fn a_glyph_is_rasterized_once_however_often_it_is_asked_for() {
        let stack = stack();
        let mut atlas = Atlas::new();
        let key = GlyphKey { font: 0, glyph: 36 };

        for _ in 0..100 {
            assert!(atlas.glyph(&stack, key).is_some());
        }
        assert_eq!(atlas.rasterized(), 1, "the cache is doing its job");
    }

    #[test]
    fn a_rasterized_glyph_has_ink_and_a_sane_box() {
        let mut stack = stack();
        let resolved = stack.resolve('W').expect("W exists");
        let mut atlas = Atlas::new();

        let glyph = atlas
            .glyph(
                &stack,
                GlyphKey {
                    font: resolved.font,
                    glyph: resolved.glyph,
                },
            )
            .expect("W has an outline");

        assert!(glyph.width > 0 && glyph.height > 0);
        let GlyphData::Mask(coverage) = &glyph.data else {
            panic!("W is an outline glyph, not a color bitmap");
        };
        assert_eq!(coverage.len(), (glyph.width * glyph.height) as usize);
        assert!(
            coverage.iter().any(|c| *c > 0),
            "a rasterized W that is entirely blank means the scaler is not scaling"
        );
    }

    #[test]
    fn the_same_glyph_id_in_two_fonts_stays_two_entries() {
        // The reason the key carries the font. Glyph 36 means different things in Menlo and
        // in Arial Hebrew, and collapsing them would draw one with the other's outline.
        let stack = stack();
        let mut atlas = Atlas::new();

        atlas.glyph(&stack, GlyphKey { font: 0, glyph: 36 });
        atlas.glyph(&stack, GlyphKey { font: 1, glyph: 36 });

        assert_eq!(atlas.len(), 2);
        assert_eq!(atlas.rasterized(), 2);
    }

    #[test]
    fn a_glyph_with_no_outline_is_cached_as_a_miss_rather_than_retried() {
        let mut stack = stack();
        let space = stack.resolve(' ').expect("space exists");
        let mut atlas = Atlas::new();
        let key = GlyphKey {
            font: space.font,
            glyph: space.glyph,
        };

        assert!(atlas.glyph(&stack, key).is_none(), "a space has no ink");
        assert!(atlas.glyph(&stack, key).is_none());
        assert_eq!(atlas.rasterized(), 1);
    }

    #[test]
    fn hebrew_rasterizes_through_the_fallback_font() {
        let mut stack = stack();
        let aleph = stack.resolve('\u{05D0}').expect("aleph resolves");
        let mut atlas = Atlas::new();

        let glyph = atlas
            .glyph(
                &stack,
                GlyphKey {
                    font: aleph.font,
                    glyph: aleph.glyph,
                },
            )
            .expect("aleph has an outline");

        let GlyphData::Mask(coverage) = &glyph.data else {
            panic!("aleph is an outline glyph");
        };
        assert!(coverage.iter().any(|c| *c > 0));
    }

    #[test]
    fn an_emoji_rasterizes_as_color_data_scaled_to_the_cell() {
        let mut stack = stack();
        let brain = stack.resolve('\u{1F9E0}').expect("brain emoji resolves");
        let mut atlas = Atlas::new();

        let metrics = stack.metrics();
        let glyph = atlas
            .glyph(
                &stack,
                GlyphKey {
                    font: brain.font,
                    glyph: brain.glyph,
                },
            )
            .expect("the brain emoji has a bitmap strike");

        let GlyphData::Color(rgba) = &glyph.data else {
            panic!("an sbix emoji must come out as color data, got a mask");
        };
        assert_eq!(rgba.len(), (glyph.width * glyph.height * 4) as usize);
        assert_eq!(
            glyph.height, metrics.height,
            "the strike is scaled to the cell height, not left at its own size"
        );
        assert!(
            rgba.chunks(4)
                .any(|p| p[3] > 0 && (p[0] != p[1] || p[1] != p[2])),
            "a brain emoji with no chromatic pixels is a silhouette, not artwork"
        );
    }

    #[test]
    fn the_scaler_is_exact_on_a_solid_color() {
        // Downscaling a solid block must yield the same solid block -- any drift here is
        // arithmetic error, and arithmetic error is nondeterminism's favorite doorway.
        let solid: Vec<u8> = std::iter::repeat([10u8, 200, 30, 255])
            .take(64 * 64)
            .flatten()
            .collect();
        let (width, height, out) = fit_to_height(64, 64, &solid, 19);
        assert_eq!(height, 19);
        assert!(width > 0);
        for pixel in out.chunks(4) {
            assert_eq!(pixel, [10, 200, 30, 255]);
        }
    }
}
