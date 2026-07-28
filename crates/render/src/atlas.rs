//! Purpose: rasterize a glyph once and keep it.
//! Public surface: `Atlas`, `GlyphKey`, `Glyph`.
//! Why this file: rasterizing an outline is the expensive part of drawing a terminal and the
//!   part that repeats hardest -- a screen of text is a few dozen distinct glyphs shown
//!   thousands of times. The key is (font, glyph) rather than glyph because the stack under
//!   it is plural by necessity (see `font.rs`); a glyph id means nothing without the font
//!   that issued it, and keying on the id alone would draw Hebrew with Menlo's glyph numbers.
//! NOT responsible for: font selection (`font.rs`), compositing (`canvas.rs`), or deciding
//!   what to draw (`renderer.rs`).
//! Test strategy: unit tests below cover the cache actually caching, a miss being cached as
//!   a miss, and the same glyph from two fonts staying distinct.

use std::collections::HashMap;

use swash::scale::{Render, ScaleContext, Source};
use swash::zeno::Format;

use crate::font::FontStack;

/// What identifies a rasterized glyph. Size is absent because a stack is built at one size;
/// changing size builds a new stack and a new atlas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    pub font: u16,
    pub glyph: u16,
}

/// A rasterized glyph: 8-bit coverage plus where it sits relative to the pen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glyph {
    /// Offset from the pen position to the left edge of the bitmap.
    pub left: i32,
    /// Offset from the baseline UP to the top edge of the bitmap.
    pub top: i32,
    pub width: u32,
    pub height: u32,
    /// One byte of coverage per pixel, row-major, `width * height` long.
    pub coverage: Vec<u8>,
}

impl Glyph {
    pub fn coverage_at(&self, x: u32, y: u32) -> u8 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        self.coverage[(y * self.width + x) as usize]
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

        // Outlines only. Colour bitmaps (emoji) are a separate source and a separate blend
        // path; leaving them out is why an emoji currently draws as nothing rather than as
        // something wrong.
        let image = Render::new(&[Source::Outline])
            .format(Format::Alpha)
            .render(&mut scaler, key.glyph)?;

        if image.placement.width == 0 || image.placement.height == 0 {
            return None;
        }

        Some(Glyph {
            left: image.placement.left,
            top: image.placement.top,
            width: image.placement.width,
            height: image.placement.height,
            coverage: image.data,
        })
    }
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
        assert_eq!(glyph.coverage.len(), (glyph.width * glyph.height) as usize);
        assert!(
            glyph.coverage.iter().any(|c| *c > 0),
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

        assert!(glyph.coverage.iter().any(|c| *c > 0));
    }
}
