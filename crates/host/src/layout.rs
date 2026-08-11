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
//! Test strategy: a coverage map over panes AND dividers together. Every pixel of the area is
//!   counted once across both, over sizes chosen to be indivisible by the grid, which is where
//!   the defects live. Counting panes alone was the earlier form of this assertion and it stopped
//!   being sufficient the moment a gutter existed: panes that leave a gap satisfy a pane-only
//!   map, and a divider drawn anywhere but that gap is invisible to it.

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
    /// Pixels of gap between neighbouring panes - where the divider is drawn.
    ///
    /// PHYSICAL pixels, like every other number here, so the host scales it: on a 2x display a
    /// one-point rule is two pixels, and a divider declared in points renders hairline-thin on
    /// exactly the display this project is developed on (the same scale trap as the font size,
    /// which this repo has now paid for twice).
    ///
    /// Zero is a canvas with no dividers and tiles exactly as it did before gutters existed,
    /// which is what keeps a single-pane canvas free of a rule it has no neighbour to need.
    pub gutter: u32,
}

/// The k-th boundary when `total` pixels are split into `count` cells.
///
/// Free rather than a closure because `tile` and `dividers` must not have their own copies: the
/// divider is drawn in the space the panes gave up, so a second implementation of this arithmetic
/// is a divider that lands beside the gap instead of in it - one pixel of pane erased on one side
/// and one pixel of gap left showing on the other, at some window widths and not others.
/// A rect trimmed to the area, empty if it falls outside it entirely.
///
/// Needed only for the degenerate case, and found by the coverage map rather than by thinking:
/// when an area is narrower than its own gutters every cell collapses to zero width and the rules
/// are left claiming more pixels than the canvas has, running past its right edge. The panes are
/// already refused upstream at that size (`CanvasError::TooSmall`), so nothing renders it - but a
/// layout that returns rectangles outside its own area is one an unwary caller will eventually
/// hand to something that indexes a buffer with them.
fn clip(rect: Rect, area: Rect) -> Rect {
    let right = (rect.x + rect.width).min(area.x + area.width);
    let bottom = (rect.y + rect.height).min(area.y + area.height);
    Rect {
        x: rect.x.min(right),
        y: rect.y.min(bottom),
        width: right.saturating_sub(rect.x),
        height: bottom.saturating_sub(rect.y),
    }
}

fn edge(index: u32, total: u32, count: u32) -> u32 {
    let base = total / count;
    let extra = total % count;
    // The first `extra` cells are one pixel wider, so the k-th boundary is
    // k*base + min(k, extra) - which lands exactly on `total` at k == count.
    index * base + index.min(extra)
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
    /// **The gutter is taken out of the panes, before the split.** The alternative - full-width
    /// panes with a divider painted over the seam - covers a column a child is writing into, and
    /// a terminal whose last column is under a rule looks like a program that truncates its own
    /// output. Panes genuinely have less room, their ptys are told so by `Canvas::spawn`, and the
    /// operator's column count is honest.
    ///
    /// Cells CAN come back zero-sized when the area is smaller than the grid. That is not
    /// silently corrected here: the tiling stays exact, and refusing an impossible canvas is the
    /// caller's decision, made where there is something to say to the operator.
    pub fn tile(&self, area: Rect) -> Vec<Rect> {
        let cols = u32::from(self.cols.max(1));
        let rows = u32::from(self.rows.max(1));
        let (usable_width, usable_height) = self.usable(area);

        let mut out = Vec::with_capacity((rows * cols) as usize);
        for row in 0..rows {
            let top = edge(row, usable_height, rows);
            let bottom = edge(row + 1, usable_height, rows);
            for col in 0..cols {
                let left = edge(col, usable_width, cols);
                let right = edge(col + 1, usable_width, cols);
                out.push(Rect {
                    // Each cell is pushed past every gutter BEFORE it. Adding one gutter per cell
                    // regardless of index is the plausible wrong version: it tiles perfectly for
                    // two columns and drifts by a gutter per extra column after that.
                    x: area.x + left + self.gutter * col,
                    y: area.y + top + self.gutter * row,
                    width: right - left,
                    height: bottom - top,
                });
            }
        }
        out
    }

    /// The divider rectangles: exactly the pixels `tile` did NOT give to a pane.
    ///
    /// Vertical dividers run the full height of the area; horizontal ones are cut into one
    /// segment per column, so the two families never overlap where they cross. That is not
    /// cosmetic bookkeeping - drawn in one colour the result is an unbroken cross either way, but
    /// overlapping rects would make the coverage map count a crossing twice and the map is the
    /// only thing that can see a divider drawn in the wrong place.
    ///
    /// Empty when the gutter is zero, so a caller need not special-case a canvas without rules.
    pub fn dividers(&self, area: Rect) -> Vec<Rect> {
        if self.gutter == 0 {
            return Vec::new();
        }
        let cols = u32::from(self.cols.max(1));
        let rows = u32::from(self.rows.max(1));
        let (usable_width, usable_height) = self.usable(area);

        let mut out = Vec::new();
        for col in 1..cols {
            out.push(clip(
                Rect {
                    x: area.x + edge(col, usable_width, cols) + self.gutter * (col - 1),
                    y: area.y,
                    width: self.gutter,
                    height: area.height,
                },
                area,
            ));
        }
        for row in 1..rows {
            let y = area.y + edge(row, usable_height, rows) + self.gutter * (row - 1);
            for col in 0..cols {
                let left = edge(col, usable_width, cols);
                let right = edge(col + 1, usable_width, cols);
                out.push(clip(
                    Rect {
                        x: area.x + left + self.gutter * col,
                        y,
                        width: right - left,
                        height: self.gutter,
                    },
                    area,
                ));
            }
        }
        out.retain(|rect| !rect.is_empty());
        out
    }

    /// The pixels left for panes once every gutter is reserved.
    ///
    /// Saturating: an area narrower than its own gutters yields zero usable pixels and therefore
    /// empty cells, rather than wrapping to an enormous width and tiling a canvas the size of the
    /// address space.
    fn usable(&self, area: Rect) -> (u32, u32) {
        let cols = u32::from(self.cols.max(1));
        let rows = u32::from(self.rows.max(1));
        (
            area.width
                .saturating_sub(self.gutter.saturating_mul(cols - 1)),
            area.height
                .saturating_sub(self.gutter.saturating_mul(rows - 1)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{Canvas, Rect};

    fn area(width: u32, height: u32) -> Rect {
        Rect { x: 0, y: 0, width, height }
    }

    /// Counts how many rects cover each pixel, panes AND dividers together. Every pixel must be
    /// covered exactly once: a 2 means two rects overlap and one is hidden under the other, a 0
    /// means a seam nothing draws.
    ///
    /// Both families in ONE map is what makes this catch the gutter defects. A pane-only map is
    /// satisfied by panes that leave a gap - which is what a gutter IS - so it can no longer tell
    /// a reserved gutter from a lost pixel, and it never had an opinion about where the divider
    /// went. Together they must still account for the area exactly.
    fn coverage(canvas: Canvas, area: Rect) -> Vec<u8> {
        let mut map = vec![0u8; (area.width * area.height) as usize];
        for rect in canvas.tile(area).into_iter().chain(canvas.dividers(area)) {
            for y in rect.y..rect.y + rect.height {
                for x in rect.x..rect.x + rect.width {
                    map[((y - area.y) * area.width + (x - area.x)) as usize] += 1;
                }
            }
        }
        map
    }

    /// The sizes are deliberately indivisible by the grids. A 1800x1000 window over 2x2 divides
    /// perfectly and would pass with any implementation, including the broken one. The gutters
    /// are likewise odd: an even gutter over an even area hides an off-by-one in the reservation.
    #[test]
    fn every_pixel_is_covered_exactly_once() {
        let sizes = [(100, 100), (101, 97), (1799, 1001), (7, 3), (64, 1)];
        let grids = [(1, 1), (1, 2), (2, 1), (2, 2), (3, 2), (2, 3), (4, 4)];
        let gutters = [0, 1, 2, 3];

        for (width, height) in sizes {
            for (rows, cols) in grids {
                for gutter in gutters {
                    let canvas = Canvas { rows, cols, gutter };
                    let map = coverage(canvas, area(width, height));
                    let bad = map.iter().enumerate().find(|(_, count)| **count != 1);
                    assert!(
                        bad.is_none(),
                        "{rows}x{cols} gutter {gutter} over {width}x{height}: pixel {} covered \
                         {} times",
                        bad.unwrap().0,
                        bad.unwrap().1
                    );
                }
            }
        }
    }

    /// The divider occupies exactly the pixels the panes gave up, and the panes are genuinely
    /// narrower for it.
    ///
    /// Stated as numbers because the two failure modes are neighbours and look identical on
    /// screen at a glance: a divider drawn OVER full-width panes (which covers a column the child
    /// is writing into) satisfies "there is a rule between the panes" perfectly, and so does one
    /// drawn one pixel off, which leaves a hairline of stale pixels beside it.
    #[test]
    fn the_divider_occupies_exactly_the_gap_the_panes_left() {
        let canvas = Canvas { rows: 1, cols: 2, gutter: 2 };
        let rects = canvas.tile(area(100, 40));
        let dividers = canvas.dividers(area(100, 40));

        assert_eq!(rects[0].width + rects[1].width, 98, "the gutter was not taken from the panes");
        assert_eq!(dividers.len(), 1);
        assert_eq!(dividers[0], Rect { x: 49, y: 0, width: 2, height: 40 });
        assert_eq!(rects[0].x + rects[0].width, dividers[0].x, "a gap before the divider");
        assert_eq!(dividers[0].x + dividers[0].width, rects[1].x, "a gap after the divider");
    }

    /// A canvas with no gutter tiles exactly as it did before dividers existed, and draws none.
    /// The single-pane case is the one that matters: a lone terminal has no neighbour, and a rule
    /// down the edge of the window would be a decoration nobody asked for.
    #[test]
    fn a_canvas_without_a_gutter_has_no_dividers() {
        let canvas = Canvas { rows: 2, cols: 2, gutter: 0 };
        assert!(canvas.dividers(area(100, 50)).is_empty());
        assert_eq!(canvas.tile(area(100, 50))[3], Rect { x: 50, y: 25, width: 50, height: 25 });
        assert!(Canvas { rows: 1, cols: 1, gutter: 4 }.dividers(area(100, 50)).is_empty());
    }

    /// Where the two families cross, they must not both claim the crossing. Covered by the map
    /// above, and named here because the fix is easy to undo: making the horizontal divider span
    /// the full width reads as the tidier code and double-covers every intersection.
    #[test]
    fn dividers_do_not_overlap_where_they_cross() {
        let canvas = Canvas { rows: 2, cols: 2, gutter: 3 };
        let dividers = canvas.dividers(area(101, 51));
        for (i, a) in dividers.iter().enumerate() {
            for b in &dividers[i + 1..] {
                let overlaps = a.x < b.x + b.width
                    && b.x < a.x + a.width
                    && a.y < b.y + b.height
                    && b.y < a.y + a.height;
                assert!(!overlaps, "{a:?} and {b:?} overlap at their crossing");
            }
        }
    }

    /// The remainder rule, stated as a number so the intent survives a refactor.
    #[test]
    fn the_remainder_goes_to_the_first_cells() {
        let rects = Canvas { rows: 1, cols: 3, gutter: 0 }.tile(area(100, 10));
        let widths: Vec<u32> = rects.iter().map(|rect| rect.width).collect();
        assert_eq!(widths, vec![34, 33, 33]);
        assert_eq!(widths.iter().sum::<u32>(), 100, "three columns lost a pixel");
    }

    /// Row-major, and each rect where it belongs. Without this, a canvas could tile perfectly
    /// while putting pane 3 where pane 1 should be - agents in the wrong cells, perfect geometry.
    #[test]
    fn rects_come_back_row_major() {
        let rects = Canvas { rows: 2, cols: 2, gutter: 0 }.tile(area(100, 50));
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
        let rects = Canvas { rows: 1, cols: 2, gutter: 0 }.tile(Rect {
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
        let rects = Canvas { rows: 1, cols: 4, gutter: 0 }.tile(area(2, 10));
        assert_eq!(rects.len(), 4);
        assert_eq!(rects.iter().filter(|rect| rect.is_empty()).count(), 2);
        assert_eq!(rects.iter().map(Rect::area).sum::<u64>(), 20);
    }
}
