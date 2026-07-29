//! Purpose: kitty-graphics placements onto the canvas -- resolve each placement's
//! pixel box, scale its image once (cached), and blend it through the same surface op
//! emoji use, so CPU==GPU byte-equality holds for images the way it does for glyphs.
//! Why this file: the scaling must be CPU-side and deterministic (the P0.2 rule: never
//!   compare two backends' resamplers), and the cache is what keeps a 60Hz poll from
//!   rescaling a screenful of pixels per frame.

use std::collections::HashMap;
use std::sync::Arc;

use crate::atlas::{bilinear, split};

/// Scales straight-RGBA to an exact target box, 16.16 fixed-point bilinear --
/// deterministic across machines and backends (the atlas scaler generalized to a box
/// that need not preserve aspect; kitty's `c,r` box is the caller's promise).
pub(crate) fn scale_to(
    width: u32,
    height: u32,
    rgba: &[u8],
    target_width: u32,
    target_height: u32,
) -> Vec<u8> {
    if width == target_width && height == target_height {
        return rgba.to_vec();
    }
    let step_x = ((width as u64) << 16) / target_width.max(1) as u64;
    let step_y = ((height as u64) << 16) / target_height.max(1) as u64;
    let mut out = Vec::with_capacity((target_width * target_height * 4) as usize);
    for y in 0..target_height as u64 {
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
    out
}

/// The per-renderer cache of scaled placements, keyed by (image id, box).
#[derive(Debug, Default)]
pub(crate) struct ScaledCache {
    entries: HashMap<(u32, u32, u32), Arc<Vec<u8>>>,
}

impl ScaledCache {
    pub(crate) fn get_or_scale(
        &mut self,
        id: u32,
        width: u32,
        height: u32,
        rgba: &[u8],
        target_width: u32,
        target_height: u32,
    ) -> Arc<Vec<u8>> {
        self.entries
            .entry((id, target_width, target_height))
            .or_insert_with(|| Arc::new(scale_to(width, height, rgba, target_width, target_height)))
            .clone()
    }

    /// Drops every cached box for an image id (its pixels changed or it was deleted).
    pub(crate) fn evict(&mut self, id: u32) {
        self.entries.retain(|&(image, _, _), _| image != id);
    }
}
