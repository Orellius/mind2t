//! Purpose: the boundary a rendering backend sits behind.
//! Public surface: `Surface`, and `TruncatingSurface`, which is a test control.
//! Why this file: slice 7 adds a second backend, and a second backend is only safe if the
//!   first one can be used to check it. Reducing a backend to four operations is what makes
//!   "the GPU agrees with the CPU" a statement about bytes rather than about intent -- the
//!   two paths share the atlas, the run-to-column mapping and the damage logic, so the only
//!   place they can disagree is here.
//! NOT responsible for: deciding what to paint (`renderer.rs`) or how a pixel is stored
//!   (`canvas.rs` for the CPU reference).
//! Test strategy: `tests/backend.rs` renders identical frames through two surfaces and
//!   requires byte-identical output, with `TruncatingSurface` proving that comparison fails
//!   when a backend is wrong.

use crate::canvas::Canvas;
use crate::color::Rgba;

/// Somewhere a frame can be painted.
///
/// Deliberately four operations and no more. Every backend-agnostic decision -- which rows
/// are stale, which column a run occupies, which glyph a cluster needs -- has already been
/// made by the time anything here is called.
///
/// **The blend is specified, not left to the backend.** `blend_mask` must compute
/// `(source * alpha + destination * (255 - alpha) + 127) / 255` in integer arithmetic, per
/// channel, and leave alpha at 255. A backend that blends in floating point and rounds
/// differently is not a rounding detail away from correct; it is a different renderer, and
/// the differential test exists to say so.
pub trait Surface {
    /// Creates a surface of the given pixel size, cleared to transparent black.
    fn with_size(width: u32, height: u32) -> Self
    where
        Self: Sized;

    fn width(&self) -> u32;

    fn height(&self) -> u32;

    /// Paints a solid rectangle, clipped to the surface.
    fn fill(&mut self, x: i32, y: i32, width: u32, height: u32, color: Rgba);

    /// Blends `color` over the surface through an 8-bit coverage mask, clipped.
    fn blend_mask(
        &mut self,
        x: i32,
        y: i32,
        mask_width: u32,
        mask_height: u32,
        coverage: &[u8],
        color: Rgba,
    );

    /// Blends a straight-RGBA image over the surface, its own alpha as coverage, clipped.
    ///
    /// The color-glyph path (emoji). Same per-channel expression as `blend_mask` with the
    /// source color coming from the image instead of a uniform -- one blend spec, so the
    /// backend differential covers both.
    fn blend_image(&mut self, x: i32, y: i32, width: u32, height: u32, rgba: &[u8]);

    /// The finished pixels: straight RGBA, row-major, top-down.
    ///
    /// Returns owned bytes rather than a borrow because a GPU backend has to read them back,
    /// and a trait that could only be implemented by something holding a CPU buffer would
    /// have quietly decided slice 7's outcome.
    ///
    /// Takes `&mut self` because reading is where a deferred backend does its work and
    /// retires what it has already done. A shared borrow forced the GPU surface to keep every
    /// operation it had ever recorded, since it could never clear them.
    fn read_pixels(&mut self) -> Vec<u8>;
}

impl Surface for Canvas {
    fn with_size(width: u32, height: u32) -> Canvas {
        Canvas::new(width, height)
    }

    fn width(&self) -> u32 {
        Canvas::width(self)
    }

    fn height(&self) -> u32 {
        Canvas::height(self)
    }

    fn fill(&mut self, x: i32, y: i32, width: u32, height: u32, color: Rgba) {
        Canvas::fill(self, x, y, width, height, color)
    }

    fn blend_mask(
        &mut self,
        x: i32,
        y: i32,
        mask_width: u32,
        mask_height: u32,
        coverage: &[u8],
        color: Rgba,
    ) {
        Canvas::blend_mask(self, x, y, mask_width, mask_height, coverage, color)
    }

    fn blend_image(&mut self, x: i32, y: i32, width: u32, height: u32, rgba: &[u8]) {
        Canvas::blend_image(self, x, y, width, height, rgba)
    }

    fn read_pixels(&mut self) -> Vec<u8> {
        Canvas::pixels(self).to_vec()
    }
}

/// A backend that truncates the blend instead of rounding it.
///
/// A control, not a feature, and wrong in the most plausible way a real backend goes wrong:
/// dropping the `+ 127` is what you get from a shader that computes in floats and lets the
/// hardware round toward zero. The error is at most one unit per channel and invisible to a
/// human, which is exactly why a comparison that has never been seen to fail cannot be
/// trusted to catch it. It has no legitimate caller.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct TruncatingSurface {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl TruncatingSurface {
    fn offset(&self, x: i32, y: i32) -> Option<usize> {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return None;
        }
        Some(((y as u32 * self.width + x as u32) * 4) as usize)
    }
}

impl Surface for TruncatingSurface {
    fn with_size(width: u32, height: u32) -> TruncatingSurface {
        TruncatingSurface {
            width,
            height,
            pixels: vec![0; (width as usize) * (height as usize) * 4],
        }
    }

    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn fill(&mut self, x: i32, y: i32, width: u32, height: u32, color: Rgba) {
        for row in 0..height as i32 {
            for column in 0..width as i32 {
                if let Some(base) = self.offset(x + column, y + row) {
                    self.pixels[base..base + 4].copy_from_slice(&color);
                }
            }
        }
    }

    fn blend_mask(
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
                let Some(base) = self.offset(x + column as i32, y + row as i32) else {
                    continue;
                };
                if alpha == u8::MAX {
                    self.pixels[base..base + 4].copy_from_slice(&color);
                    continue;
                }
                let a = u32::from(alpha);
                for (target, source) in self.pixels[base..base + 3].iter_mut().zip(color) {
                    let over = u32::from(source) * a + u32::from(*target) * (255 - a);
                    // The bug: no rounding term.
                    *target = (over / 255) as u8;
                }
                self.pixels[base + 3] = 255;
            }
        }
    }

    fn blend_image(&mut self, x: i32, y: i32, width: u32, height: u32, rgba: &[u8]) {
        // The same deliberate truncation as blend_mask, applied to the image path, so the
        // backend differential can be shown to fail for images too.
        for row in 0..height {
            for column in 0..width {
                let source_base = ((row * width + column) * 4) as usize;
                let alpha = u32::from(rgba[source_base + 3]);
                if alpha == 0 {
                    continue;
                }
                let Some(base) = self.offset(x + column as i32, y + row as i32) else {
                    continue;
                };
                if alpha == 255 {
                    self.pixels[base..base + 3].copy_from_slice(&rgba[source_base..source_base + 3]);
                    self.pixels[base + 3] = 255;
                    continue;
                }
                for channel in 0..3 {
                    let source = u32::from(rgba[source_base + channel]);
                    let destination = u32::from(self.pixels[base + channel]);
                    let over = source * alpha + destination * (255 - alpha);
                    // The bug: no rounding term.
                    self.pixels[base + channel] = (over / 255) as u8;
                }
                self.pixels[base + 3] = 255;
            }
        }
    }

    fn read_pixels(&mut self) -> Vec<u8> {
        self.pixels.clone()
    }
}
