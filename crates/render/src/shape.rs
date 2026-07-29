//! Purpose: turn one cell's grapheme cluster into positioned glyphs.
//! Public surface: `Shaper`, `PositionedGlyph`.
//! Why this file: a combining mark does not know where it goes. Hebrew niqqud carry no useful
//!   bearings of their own -- the font's GPOS table says where each one attaches to each base,
//!   and without consulting it a qamats lands wherever the glyph's default origin happens to
//!   be, which for Miriam Mono CLM is the left edge of the cell instead of centred under the
//!   letter. Measured: shaping moves it by half an advance.
//! NOT responsible for: reordering (that is `ruuah-vt-frame`'s `bidi.rs`, and it happens per
//!   ROW), rasterization (`atlas.rs`), or font choice (`font.rs`).
//! Test strategy: `tests/shaping.rs` measures where the mark's ink actually lands, and proves
//!   the measurement can fail by running the same cell through the unshaped path.
//!
//! **One cell is shaped at a time, by default.** A terminal cell is addressable: the program
//! owns which column its text sits in, so shaping across cells would let the renderer merge
//! or re-space cells the program placed deliberately. REVISED 2026-07-30 for ligatures, on
//! the operator's order, with the guard that keeps the original property: `shape_run` shapes
//! a same-style ASCII segment with calt/liga, and the renderer takes the result ONLY when a
//! ligature actually formed (fewer glyphs than characters). A font without ligatures -- Menlo
//! -- therefore produces byte-identical output to the per-cell path, and Arabic joining
//! remains out (segments are ASCII by construction).

use swash::shape::ShapeContext;
use swash::text::{Codepoint, Script};

use crate::atlas::GlyphKey;
use crate::font::FontStack;

/// A glyph and where to draw it, in pixels relative to the cell's pen position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionedGlyph {
    pub key: GlyphKey,
    pub x: f32,
    /// Positive is UP from the baseline, matching the sign convention of a glyph's `top`.
    pub y: f32,
}

/// Shapes clusters, reusing its internal caches across calls.
pub struct Shaper {
    context: ShapeContext,
    /// Scratch, so shaping a cell costs no allocation after the first.
    glyphs: Vec<PositionedGlyph>,
}

impl Shaper {
    pub fn new() -> Shaper {
        Shaper {
            context: ShapeContext::new(),
            glyphs: Vec::with_capacity(8),
        }
    }

    /// Positions every glyph in `cluster`, or returns an empty slice if no font covers it.
    ///
    /// The whole cluster is shaped with ONE font, chosen by its first character. A combining
    /// mark and the letter it attaches to effectively always come from the same face -- and
    /// they have to, because GPOS attachment is a statement about glyph ids within a single
    /// font and cannot span two.
    pub fn shape(&mut self, fonts: &mut FontStack, cluster: &str) -> &[PositionedGlyph] {
        self.glyphs.clear();

        let Some(first) = cluster.chars().next() else {
            return &self.glyphs;
        };
        let Some(resolved) = fonts.resolve(first) else {
            return &self.glyphs;
        };
        let Some(font) = fonts.face(resolved.font) else {
            return &self.glyphs;
        };

        let script = first.script();
        let mut shaper = self
            .context
            .builder(font)
            .script(script)
            .size(fonts.size())
            .build();
        shaper.add_str(cluster);

        // The pen walks by each glyph's advance; the offsets are relative to it. A mark has
        // zero advance, so it lands on the base it follows rather than after it.
        let mut pen = 0.0f32;
        let glyphs = &mut self.glyphs;
        shaper.shape_with(|cluster| {
            for glyph in cluster.glyphs {
                glyphs.push(PositionedGlyph {
                    key: GlyphKey {
                        font: resolved.font,
                        glyph: glyph.id,
                    },
                    x: pen + glyph.x,
                    y: glyph.y,
                });
                pen += glyph.advance;
            }
        });

        &self.glyphs
    }

    /// Shapes a same-style ASCII segment as one string with ligature features on
    /// (calt + liga -- programming fonts ship arrows under calt). Every glyph is
    /// positioned from the segment's own pen, one fixed advance per source character,
    /// so a formed ligature spans its cells exactly.
    ///
    /// Returns the glyphs and whether any SUBSTITUTION happened. Fewer glyphs than
    /// characters catches classic ligatures; comparing glyph ids against the nominal
    /// per-character mapping catches the Iosevka/Fira shape, where calt swaps pieces
    /// in WITHOUT changing the count (measured live 2026-07-30: Iosevka's `->` kept
    /// two glyphs and the count heuristic saw nothing). The caller falls back to the
    /// per-cell path when nothing substituted, which is what keeps non-ligating fonts
    /// byte-identical to the old renderer.
    pub fn shape_run(
        &mut self,
        fonts: &mut FontStack,
        text: &str,
    ) -> (&[PositionedGlyph], bool) {
        self.glyphs.clear();
        let chars = text.chars().count();
        let Some(first) = text.chars().next() else {
            return (&self.glyphs, false);
        };
        let Some(resolved) = fonts.resolve(first) else {
            return (&self.glyphs, false);
        };
        let Some(font) = fonts.face(resolved.font) else {
            return (&self.glyphs, false);
        };
        let mut shaper = self
            .context
            .builder(font)
            .script(Script::Latin)
            .size(fonts.size())
            .features(&[("calt", 1u16), ("liga", 1u16)])
            .build();
        shaper.add_str(text);
        let charmap = font.charmap();
        let nominal: Vec<u16> = text.chars().map(|c| charmap.map(c)).collect();
        let mut pen = 0.0f32;
        let glyphs = &mut self.glyphs;
        let mut count = 0usize;
        let mut substituted = false;
        shaper.shape_with(|cluster| {
            for glyph in cluster.glyphs {
                if nominal.get(count) != Some(&glyph.id) {
                    substituted = true;
                }
                glyphs.push(PositionedGlyph {
                    key: GlyphKey {
                        font: resolved.font,
                        glyph: glyph.id,
                    },
                    x: pen + glyph.x,
                    y: glyph.y,
                });
                pen += glyph.advance;
                count += 1;
            }
        });
        (&self.glyphs, substituted || count < chars)
    }

    /// One glyph per character, all at the pen origin.
    ///
    /// Exactly right for a single-codepoint cluster, which is nearly every cell on screen, so
    /// this is the fast path rather than a fallback. For a cluster WITH marks it is the wrong
    /// answer, and that is its second job: `tests/shaping.rs` uses it as the control that must
    /// put a niqqud somewhere the shaped path does not, because a positioning test that has
    /// never been seen to fail cannot tell a working GPOS lookup from an ignored one.
    pub fn place_at_origin(&mut self, fonts: &mut FontStack, cluster: &str) -> &[PositionedGlyph] {
        self.glyphs.clear();
        for c in cluster.chars() {
            if let Some(resolved) = fonts.resolve(c) {
                self.glyphs.push(PositionedGlyph {
                    key: GlyphKey {
                        font: resolved.font,
                        glyph: resolved.glyph,
                    },
                    x: 0.0,
                    y: 0.0,
                });
            }
        }
        &self.glyphs
    }
}

impl Default for Shaper {
    fn default() -> Shaper {
        Shaper::new()
    }
}

/// Whether a cluster needs the shaper at all.
///
/// A single codepoint has nothing to attach to anything, so the overwhelming majority of cells
/// -- every character of code and English -- skip shaping entirely.
pub fn needs_shaping(cluster: &str) -> bool {
    cluster.chars().nth(1).is_some()
}

/// The script a cluster belongs to, for callers that want to reason about it.
pub fn script_of(cluster: &str) -> Option<Script> {
    cluster.chars().next().map(|c| c.script())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fonts() -> FontStack {
        FontStack::system(16.0).expect("system fonts")
    }

    #[test]
    fn a_lone_character_does_not_need_the_shaper() {
        assert!(!needs_shaping("a"));
        assert!(!needs_shaping("\u{05D0}"));
        assert!(needs_shaping("\u{05D1}\u{05BC}"));
    }

    #[test]
    fn a_pointed_hebrew_cluster_shapes_to_a_base_plus_zero_advance_marks() {
        let mut fonts = fonts();
        let mut shaper = Shaper::new();
        // bet + dagesh + qamats
        let glyphs = shaper.shape(&mut fonts, "\u{05D1}\u{05BC}\u{05B8}");

        assert!(glyphs.len() >= 2, "got {glyphs:?}");
        assert_eq!(glyphs[0].x, 0.0, "the base sits at the pen");
        assert!(
            glyphs[1..].iter().all(|g| g.x != 0.0),
            "a mark left at the pen origin means GPOS was not applied: {glyphs:?}"
        );
    }

    #[test]
    fn the_mark_is_placed_within_the_cell_it_belongs_to() {
        let mut fonts = fonts();
        let width = fonts.metrics().width as f32;
        let mut shaper = Shaper::new();
        let glyphs = shaper.shape(&mut fonts, "\u{05E9}\u{05C1}\u{05B8}");

        for glyph in glyphs {
            assert!(
                glyph.x > -width && glyph.x < width * 2.0,
                "glyph escaped its cell: {glyph:?} (cell width {width})"
            );
        }
    }

    #[test]
    fn latin_shapes_to_one_glyph_per_character() {
        let mut fonts = fonts();
        let mut shaper = Shaper::new();
        assert_eq!(
            shaper.shape(&mut fonts, "fi").len(),
            2,
            "no cross-cell ligature"
        );
    }

    #[test]
    fn placing_at_the_origin_stacks_everything_there() {
        let mut fonts = fonts();
        let mut shaper = Shaper::new();
        let glyphs = shaper.place_at_origin(&mut fonts, "\u{05D1}\u{05BC}");

        assert_eq!(glyphs.len(), 2);
        assert!(glyphs.iter().all(|g| g.x == 0.0 && g.y == 0.0));
    }

    #[test]
    fn a_cluster_no_font_covers_shapes_to_nothing() {
        let mut fonts = fonts();
        let mut shaper = Shaper::new();
        assert!(shaper.shape(&mut fonts, "\u{10FFFD}").is_empty());
    }
}
