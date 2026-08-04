//! Purpose: turn a canvas declaration (rows x cols) into the exact rectangles the panes occupy.
//! Public surface: `Rect`, `Canvas`.
//! Why this file: the failure here is SILENT and expensive. A pane whose rect is too wide draws
//!   underneath its neighbour and looks entirely normal - you simply lose the columns underneath,
//!   which reads as an agent that stopped printing. A pane one pixel short leaves a seam. Neither
//!   errors, neither fails a build, and neither is visible in a screenshot unless you already
//!   suspect it. So the arithmetic lives apart from any window, where it can be asserted.
//! NOT responsible for: what goes IN a pane (the session), how it is drawn (`present_all`), or
//!   where the chrome strip is - the strip is subtracted by the caller and handed in as `area`,
//!   because a layout that knows about chrome cannot be reused for a pane that has none.
//! Test strategy: a coverage map. Every pixel of the area is counted across all rects and must
//!   be covered EXACTLY ONCE - one assertion that catches overlap and gap together, over sizes
//!   chosen to be indivisible by the grid, which is where both defects actually live.

/// A rectangle in PHYSICAL pixels, top-left origin - the same space `present_all` blits into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn area(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// A wizard-declared grid: `rows` x `cols` panes filling their area.
///
/// The Canvas is what the wizard emits and what B3's spec carries. It is deliberately a GRID and
/// not yet a split tree: the product asks you to choose a grid, and a tree whose only shape is a
/// grid is a tree nobody can test. Splits with dragged dividers extend this - the tiling rule
/// below is what they must keep - and the day they arrive, `tile` becomes the leaf case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Canvas {
    pub rows: u16,
    pub cols: u16,
}

impl Canvas {
    /// The rects, row-major: index `row * cols + col`.
    ///
    /// **The remainder is distributed, never dropped.** A 100px width over 3 columns is 34+33+33,
    /// not 33+33+33 - the naive division loses pixels, and lost pixels are a seam of clear colour
    /// down the inside of the window that looks like a deliberate gutter until you resize and
    /// watch it breathe. The first cells take the extra pixel, which is arbitrary and stated
    /// rather than left for someone to rediscover.
    ///
    /// Cells CAN come back zero-sized when the area is smaller than the grid. That is not
    /// silently corrected here: the tiling stays exact, and refusing an impossible canvas is the
    /// caller's decision, made where there is something to say to the operator.
    pub fn tile(&self, area: Rect) -> Vec<Rect> {
        let cols = u32::from(self.cols.max(1));
        let rows = u32::from(self.rows.max(1));

        // Every boundary is computed from the SAME division, so a cell's right edge is by
        // construction its neighbour's left edge. Computing width per cell and then summing
        // positions is what introduces the off-by-one that overlaps two panes by a column.
        let edge = |index: u32, total: u32, count: u32| -> u32 {
            let base = total / count;
            let extra = total % count;
            // The first `extra` cells are one pixel wider, so the k-th boundary is
            // k*base + min(k, extra) - which lands exactly on `total` at k == count.
            index * base + index.min(extra)
        };

        let mut out = Vec::with_capacity((rows * cols) as usize);
        for row in 0..rows {
            let top = edge(row, area.height, rows);
            let bottom = edge(row + 1, area.height, rows);
            for col in 0..cols {
                let left = edge(col, area.width, cols);
                let right = edge(col + 1, area.width, cols);
                out.push(Rect {
                    x: area.x + left,
                    y: area.y + top,
                    width: right - left,
                    height: bottom - top,
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{Canvas, Rect};

    fn area(width: u32, height: u32) -> Rect {
        Rect { x: 0, y: 0, width, height }
    }

    /// Counts how many rects cover each pixel. Every pixel must be covered exactly once: a 2
    /// means panes overlap and one is hidden under the other, a 0 means a seam.
    fn coverage(canvas: Canvas, area: Rect) -> Vec<u8> {
        let mut map = vec![0u8; (area.width * area.height) as usize];
        for rect in canvas.tile(area) {
            for y in rect.y..rect.y + rect.height {
                for x in rect.x..rect.x + rect.width {
                    map[(y * area.width + x) as usize] += 1;
                }
            }
        }
        map
    }

    /// The sizes are deliberately indivisible by the grids. A 1800x1000 window over 2x2 divides
    /// perfectly and would pass with any implementation, including the broken one.
    #[test]
    fn every_pixel_is_covered_exactly_once() {
        let sizes = [(100, 100), (101, 97), (1799, 1001), (7, 3), (64, 1)];
        let grids = [(1, 1), (1, 2), (2, 1), (2, 2), (3, 2), (2, 3), (4, 4)];

        for (width, height) in sizes {
            for (rows, cols) in grids {
                let canvas = Canvas { rows, cols };
                let map = coverage(canvas, area(width, height));
                let bad = map.iter().enumerate().find(|(_, count)| **count != 1);
                assert!(
                    bad.is_none(),
                    "{rows}x{cols} over {width}x{height}: pixel {} covered {} times",
                    bad.unwrap().0,
                    bad.unwrap().1
                );
            }
        }
    }

    /// The remainder rule, stated as a number so the intent survives a refactor.
    #[test]
    fn the_remainder_goes_to_the_first_cells() {
        let rects = Canvas { rows: 1, cols: 3 }.tile(area(100, 10));
        let widths: Vec<u32> = rects.iter().map(|rect| rect.width).collect();
        assert_eq!(widths, vec![34, 33, 33]);
        assert_eq!(widths.iter().sum::<u32>(), 100, "three columns lost a pixel");
    }

    /// Row-major, and each rect where it belongs. Without this, a canvas could tile perfectly
    /// while putting pane 3 where pane 1 should be - agents in the wrong cells, perfect geometry.
    #[test]
    fn rects_come_back_row_major() {
        let rects = Canvas { rows: 2, cols: 2 }.tile(area(100, 50));
        assert_eq!(rects.len(), 4);
        assert_eq!(rects[0], Rect { x: 0, y: 0, width: 50, height: 25 });
        assert_eq!(rects[1], Rect { x: 50, y: 0, width: 50, height: 25 });
        assert_eq!(rects[2], Rect { x: 0, y: 25, width: 50, height: 25 });
        assert_eq!(rects[3], Rect { x: 50, y: 25, width: 50, height: 25 });
    }

    /// The area's own origin is carried, because the chrome strip is subtracted by the caller and
    /// arrives as a non-zero `y`. A layout that assumed (0,0) would put every pane exactly the
    /// strip's height too high - the whole grid under the chrome, looking merely misaligned.
    #[test]
    fn the_areas_origin_is_carried_into_every_rect() {
        let strip = 136;
        let rects = Canvas { rows: 1, cols: 2 }.tile(Rect {
            x: 0,
            y: strip,
            width: 100,
            height: 50,
        });
        assert!(rects.iter().all(|rect| rect.y == strip));
        assert_eq!(rects[1].x, 50);
    }

    /// An area too small for the grid still tiles exactly; it simply hands back empty cells. The
    /// caller refuses, where there is an operator to tell.
    #[test]
    fn an_impossible_canvas_tiles_to_empty_cells_rather_than_lying() {
        let rects = Canvas { rows: 1, cols: 4 }.tile(area(2, 10));
        assert_eq!(rects.len(), 4);
        assert_eq!(rects.iter().filter(|rect| rect.is_empty()).count(), 2);
        assert_eq!(rects.iter().map(Rect::area).sum::<u64>(), 20);
    }
}
