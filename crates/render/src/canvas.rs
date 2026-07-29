//! Purpose: the pixel buffer, and the two operations a terminal needs to put things in it.
//! Public surface: `Canvas`.
//! Why this file: keeping the buffer dumb is what makes the redraw invariant testable. It
//!   has no idea what a cell or a frame is, so "an incremental redraw equals a full one" is
//!   a statement about bytes rather than about intent, and there is nowhere for the two
//!   paths to differ except in what they were asked to draw.
//! NOT responsible for: colour rules (`color.rs`), glyph shapes (`atlas.rs`), or deciding
//!   what to paint (`renderer.rs`).
//! Test strategy: unit tests below cover clipping at every edge (the failure that corrupts
//!   neighbouring rows and reads as a damage bug) and the alpha blend at its endpoints.

use crate::color::Rgba;

/// A straight-RGBA pixel buffer, row-major, top-down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Canvas {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Canvas {
        Canvas {
            width,
            height,
            pixels: vec![0; (width as usize) * (height as usize) * 4],
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn pixel(&self, x: u32, y: u32) -> Rgba {
        if x >= self.width || y >= self.height {
            return [0, 0, 0, 0];
        }
        let base = ((y * self.width + x) * 4) as usize;
        [
            self.pixels[base],
            self.pixels[base + 1],
            self.pixels[base + 2],
            self.pixels[base + 3],
        ]
    }

    /// Paints a solid rectangle, clipped to the canvas.
    pub fn fill(&mut self, x: i32, y: i32, width: u32, height: u32, color: Rgba) {
        for row in 0..height as i32 {
            for column in 0..width as i32 {
                self.set(x + column, y + row, color);
            }
        }
    }

    /// Blends `color` over the canvas using an 8-bit coverage mask.
    ///
    /// `coverage` is `mask_width * mask_height` bytes, row-major, positioned with its top
    /// left corner at (`x`, `y`). Anything falling outside the canvas is dropped rather than
    /// wrapping, which is the whole reason this is one function and not open-coded per call
    /// site: a glyph that overhangs its cell is normal, and a wrapped overhang corrupts a row
    /// somewhere else entirely.
    pub fn blend_mask(
        &mut self,
        x: i32,
        y: i32,
        mask_width: u32,
        mask_height: u32,
        coverage: &[u8],
        color: Rgba,
    ) {
        for row in 0..mask_height {
            for column in 0..mask_width {
                let alpha = coverage[(row * mask_width + column) as usize];
                if alpha == 0 {
                    continue;
                }
                self.blend(x + column as i32, y + row as i32, color, alpha);
            }
        }
    }

    /// Blends a straight-RGBA image over the canvas, its own alpha as coverage.
    ///
    /// The per-channel arithmetic is the SAME rounded integer expression as `blend_mask`
    /// -- one blend spec, two sources of color -- so the CPU/GPU differential holds for
    /// images by the same argument it holds for masks.
    pub fn blend_image(&mut self, x: i32, y: i32, width: u32, height: u32, rgba: &[u8]) {
        for row in 0..height {
            for column in 0..width {
                let base = ((row * width + column) * 4) as usize;
                let pixel = [rgba[base], rgba[base + 1], rgba[base + 2], rgba[base + 3]];
                if pixel[3] == 0 {
                    continue;
                }
                self.blend(x + column as i32, y + row as i32, pixel, pixel[3]);
            }
        }
    }

    fn set(&mut self, x: i32, y: i32, color: Rgba) {
        let Some(base) = self.offset(x, y) else {
            return;
        };
        self.pixels[base..base + 4].copy_from_slice(&color);
    }

    fn blend(&mut self, x: i32, y: i32, color: Rgba, alpha: u8) {
        let Some(base) = self.offset(x, y) else {
            return;
        };
        if alpha == u8::MAX {
            self.pixels[base..base + 4].copy_from_slice(&color);
            return;
        }
        let a = u32::from(alpha);
        for (target, source) in self.pixels[base..base + 3].iter_mut().zip(color) {
            let over = u32::from(source) * a + u32::from(*target) * (255 - a);
            // Rounded rather than truncated, so a full-coverage blend of X over X is X.
            *target = ((over + 127) / 255) as u8;
        }
        self.pixels[base + 3] = 255;
    }

    fn offset(&self, x: i32, y: i32) -> Option<usize> {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return None;
        }
        Some(((y as u32 * self.width + x as u32) * 4) as usize)
    }

    /// Serializes to a 32-bit BMP, so a rendered frame can be looked at by a human.
    ///
    /// BMP rather than PNG purely to avoid a compression dependency for what is a debugging
    /// artifact. Written top-down (a negative height), which is legal and avoids flipping.
    pub fn to_bmp(&self) -> Vec<u8> {
        const HEADER: u32 = 54;
        let image_size = self.width * self.height * 4;
        let mut out = Vec::with_capacity((HEADER + image_size) as usize);

        out.extend_from_slice(b"BM");
        out.extend_from_slice(&(HEADER + image_size).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&HEADER.to_le_bytes());

        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&(self.width as i32).to_le_bytes());
        out.extend_from_slice(&(-(self.height as i32)).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&image_size.to_le_bytes());
        out.extend_from_slice(&2835i32.to_le_bytes());
        out.extend_from_slice(&2835i32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());

        for pixel in self.pixels.chunks_exact(4) {
            out.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Rgba = [255, 0, 0, 255];
    const BLUE: Rgba = [0, 0, 255, 255];

    #[test]
    fn a_fill_lands_where_it_was_asked_to() {
        let mut canvas = Canvas::new(4, 4);
        canvas.fill(1, 1, 2, 2, RED);

        assert_eq!(canvas.pixel(1, 1), RED);
        assert_eq!(canvas.pixel(2, 2), RED);
        assert_eq!(canvas.pixel(0, 0), [0, 0, 0, 0]);
        assert_eq!(canvas.pixel(3, 3), [0, 0, 0, 0]);
    }

    #[test]
    fn drawing_off_every_edge_is_clipped_and_never_wraps() {
        // A glyph overhanging its cell is normal. A wrapped overhang paints a pixel on some
        // unrelated row, which surfaces as a damage bug and is very hard to trace back here.
        let mut canvas = Canvas::new(4, 4);
        canvas.fill(-10, -10, 3, 3, RED);
        canvas.fill(10, 10, 3, 3, RED);
        canvas.fill(-1, 0, 1, 1, RED);
        canvas.fill(4, 0, 1, 1, RED);

        assert!(
            canvas.pixels().iter().all(|byte| *byte == 0),
            "nothing outside the canvas may reach it"
        );
    }

    #[test]
    fn a_partial_fill_across_an_edge_keeps_only_the_inside() {
        let mut canvas = Canvas::new(4, 4);
        canvas.fill(-1, -1, 2, 2, RED);

        assert_eq!(canvas.pixel(0, 0), RED);
        assert_eq!(canvas.pixel(1, 0), [0, 0, 0, 0]);
        assert_eq!(canvas.pixel(0, 1), [0, 0, 0, 0]);
    }

    #[test]
    fn full_coverage_replaces_and_zero_coverage_leaves_alone() {
        let mut canvas = Canvas::new(2, 1);
        canvas.fill(0, 0, 2, 1, BLUE);
        canvas.blend_mask(0, 0, 2, 1, &[255, 0], RED);

        assert_eq!(canvas.pixel(0, 0), RED);
        assert_eq!(canvas.pixel(1, 0), BLUE);
    }

    #[test]
    fn a_half_coverage_blend_lands_between_the_two() {
        let mut canvas = Canvas::new(1, 1);
        canvas.fill(0, 0, 1, 1, [0, 0, 0, 255]);
        canvas.blend_mask(0, 0, 1, 1, &[128], [255, 255, 255, 255]);

        let blended = canvas.pixel(0, 0);
        assert!(blended[0] > 100 && blended[0] < 155, "got {blended:?}");
        assert_eq!(blended[3], 255);
    }

    #[test]
    fn blending_a_colour_over_itself_at_full_coverage_is_a_fixed_point() {
        // Rounding, not truncation. Without the +127 this drifts a shade darker every frame,
        // which would make the redraw invariant fail for a reason that has nothing to do
        // with damage.
        let mut canvas = Canvas::new(1, 1);
        canvas.fill(0, 0, 1, 1, [200, 100, 50, 255]);
        for _ in 0..50 {
            canvas.blend_mask(0, 0, 1, 1, &[254], [200, 100, 50, 255]);
        }
        assert_eq!(canvas.pixel(0, 0), [200, 100, 50, 255]);
    }

    #[test]
    fn the_bmp_header_describes_the_canvas() {
        let canvas = Canvas::new(3, 2);
        let bmp = canvas.to_bmp();

        assert_eq!(&bmp[0..2], b"BM");
        assert_eq!(bmp.len(), 54 + 3 * 2 * 4);
        assert_eq!(
            u32::from_le_bytes(bmp[2..6].try_into().unwrap()),
            bmp.len() as u32
        );
        assert_eq!(i32::from_le_bytes(bmp[18..22].try_into().unwrap()), 3);
        assert_eq!(
            i32::from_le_bytes(bmp[22..26].try_into().unwrap()),
            -2,
            "negative height means top-down"
        );
    }
}
