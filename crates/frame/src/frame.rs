//! Purpose: the reader's own copy of a frame, and the only shape a renderer is allowed to
//!   draw from -- runs, never a raw cell array.
//! Public surface: `Frame`, `FrameCursor`, `Run`, `Direction`, `Runs`.
//! Why this file: this is the seam that keeps bidi out of the renderer. A renderer that
//!   walks cells left to right has the logical order baked into every draw call, and slice
//!   5.5 would be a rewrite. A renderer that draws runs asks each run where it starts and
//!   which way it advances, so reordering later changes the run builder and nothing else.
//!   `Direction` is `LeftToRight` for every run today and is still load-bearing today: it
//!   forces the renderer to compute a column from the run rather than assume one.
//! NOT responsible for: the thread handoff (`seqlock.rs`) or bit layout (`packed.rs`).
//! Test strategy: run splitting is unit-tested below against hand-built rows; the frame's
//!   agreement with the core is tested end to end in `tests/publish.rs`.

use ruuah_vt_snapshot::{Style, Wide};

use crate::packed::{PackedCell, unpack_style};

/// Which way a run advances from its starting column.
///
/// One variant is produced today. The other is what slice 5.5 turns on, and every renderer
/// written against this enum keeps working when it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    LeftToRight,
    RightToLeft,
}

/// A span of cells a renderer can draw as a unit: one style, one direction, contiguous
/// columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run<'a> {
    /// Leftmost column the run occupies, in screen coordinates.
    pub start: u16,
    pub direction: Direction,
    pub style: Style,
    /// The run's cells in logical order. `column_of` maps an index to its screen column.
    pub cells: &'a [PackedCell],
}

impl Run<'_> {
    /// Where the `index`th cell of this run is drawn.
    ///
    /// The whole point of the seam. A renderer that adds `index` to `start` itself works
    /// until the first right-to-left run and then draws every Hebrew line backwards.
    pub fn column_of(&self, index: usize) -> u16 {
        let offset = self.width_before(index);
        match self.direction {
            Direction::LeftToRight => self.start + offset,
            Direction::RightToLeft => {
                self.start + self.width().saturating_sub(offset + self.cell_width(index))
            }
        }
    }

    /// Columns the run occupies in total.
    pub fn width(&self) -> u16 {
        self.width_before(self.cells.len())
    }

    fn width_before(&self, index: usize) -> u16 {
        self.cells[..index.min(self.cells.len())]
            .iter()
            .map(|cell| cell_width(*cell))
            .sum()
    }

    fn cell_width(&self, index: usize) -> u16 {
        self.cells.get(index).copied().map(cell_width).unwrap_or(0)
    }
}

/// A cell's contribution to the horizontal advance. Spacer tails are already accounted for
/// by the wide cell they follow, so they claim nothing of their own.
///
/// Public because a renderer needs the same answer when sizing a cell's background, and two
/// copies of this rule would disagree the first time one of them changed.
pub fn cell_width(cell: PackedCell) -> u16 {
    match cell.wide() {
        Wide::Wide => 2,
        Wide::SpacerTail => 0,
        _ => 1,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrameCursor {
    pub x: u16,
    pub y: u16,
    pub pending_wrap: bool,
    pub visible: bool,
    pub style: Style,
}

/// One consistent frame, owned by the thread that read it.
#[derive(Debug, Default)]
pub struct Frame {
    pub cols: u16,
    pub rows: u16,
    /// The publish this frame came from. Zero means nothing has been read into it yet.
    pub generation: u64,
    /// Generation of the last whole-frame invalidation.
    pub full_generation: u64,
    pub cursor: FrameCursor,
    pub(crate) cells: Vec<PackedCell>,
    pub(crate) row_generation: Vec<u64>,
    pub(crate) row_flags: Vec<(bool, bool)>,
    pub(crate) styles: Vec<[u64; 2]>,
}

impl Frame {
    pub fn new() -> Frame {
        Frame::default()
    }

    pub(crate) fn resize(&mut self, cols: u16, rows: u16) {
        let cells = usize::from(cols) * usize::from(rows);
        self.cols = cols;
        self.rows = rows;
        self.cells.resize(cells, PackedCell::BLANK);
        self.row_generation.resize(usize::from(rows), 0);
        self.row_flags.resize(usize::from(rows), (false, false));
    }

    pub fn cell(&self, x: u16, y: u16) -> PackedCell {
        let index = usize::from(y) * usize::from(self.cols) + usize::from(x);
        self.cells.get(index).copied().unwrap_or(PackedCell::BLANK)
    }

    pub fn style(&self, id: u16) -> Style {
        self.styles
            .get(usize::from(id))
            .copied()
            .map(unpack_style)
            .unwrap_or(Style::DEFAULT)
    }

    /// This row soft-wraps into the next.
    pub fn wraps(&self, y: u16) -> bool {
        self.row_flags.get(usize::from(y)).is_some_and(|f| f.0)
    }

    /// Whether the renderer still owes this row a repaint, given the last frame it drew.
    ///
    /// Comparing stamps rather than consuming flags is what makes a dropped frame harmless:
    /// a renderer that skipped generations 41 through 46 still sees every row touched in any
    /// of them, because their stamps are all above the 40 it last drew.
    pub fn row_is_stale(&self, y: u16, drawn_generation: u64) -> bool {
        if self.full_generation > drawn_generation {
            return true;
        }
        self.row_generation
            .get(usize::from(y))
            .is_some_and(|stamp| *stamp > drawn_generation)
    }

    /// Rows needing a repaint, given the last generation the caller drew.
    pub fn stale_rows(&self, drawn_generation: u64) -> impl Iterator<Item = u16> + '_ {
        (0..self.rows).filter(move |y| self.row_is_stale(*y, drawn_generation))
    }

    /// The row split into drawable runs.
    pub fn runs(&self, y: u16) -> Runs<'_> {
        let start = usize::from(y) * usize::from(self.cols);
        let end = start + usize::from(self.cols);
        Runs {
            frame: self,
            cells: self.cells.get(start..end).unwrap_or(&[]),
            column: 0,
            index: 0,
        }
    }
}

/// Splits a row into runs. A run breaks where the style changes; a spacer tail stays with
/// the wide cell it belongs to rather than starting a run of its own.
pub struct Runs<'a> {
    frame: &'a Frame,
    cells: &'a [PackedCell],
    column: u16,
    index: usize,
}

impl<'a> Iterator for Runs<'a> {
    type Item = Run<'a>;

    fn next(&mut self) -> Option<Run<'a>> {
        if self.index >= self.cells.len() {
            return None;
        }

        let start_index = self.index;
        let style_id = self.cells[start_index].style_id();
        let mut width = 0;

        while self.index < self.cells.len() {
            let cell = self.cells[self.index];
            let continues = self.index == start_index
                || cell.style_id() == style_id
                || cell.wide() == Wide::SpacerTail;
            if !continues {
                break;
            }
            width += cell_width(cell);
            self.index += 1;
        }

        let run = Run {
            start: self.column,
            direction: Direction::LeftToRight,
            style: self.frame.style(style_id),
            cells: &self.cells[start_index..self.index],
        };
        self.column += width;
        Some(run)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packed::pack_style;
    use ruuah_vt_snapshot::Color;

    fn frame_with(cells: Vec<PackedCell>, styles: Vec<[u64; 2]>) -> Frame {
        let cols = cells.len() as u16;
        Frame {
            cols,
            rows: 1,
            generation: 1,
            full_generation: 0,
            cursor: FrameCursor::default(),
            cells,
            row_generation: vec![1],
            row_flags: vec![(false, false)],
            styles,
        }
    }

    #[test]
    fn a_uniform_row_is_one_run() {
        let frame = frame_with(
            (0..4)
                .map(|_| PackedCell::new("a", 0, Wide::Narrow))
                .collect(),
            vec![[0, 0]],
        );
        let runs: Vec<_> = frame.runs(0).collect();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].start, 0);
        assert_eq!(runs[0].width(), 4);
    }

    #[test]
    fn a_style_change_starts_a_new_run_at_the_right_column() {
        let red = Style {
            fg: Color::Palette(1),
            ..Style::DEFAULT
        };
        let frame = frame_with(
            vec![
                PackedCell::new("a", 0, Wide::Narrow),
                PackedCell::new("b", 0, Wide::Narrow),
                PackedCell::new("c", 1, Wide::Narrow),
            ],
            vec![[0, 0], pack_style(&red)],
        );

        let runs: Vec<_> = frame.runs(0).collect();
        assert_eq!(runs.len(), 2);
        assert_eq!((runs[0].start, runs[0].width()), (0, 2));
        assert_eq!((runs[1].start, runs[1].width()), (2, 1));
        assert_eq!(runs[1].style, red);
    }

    #[test]
    fn a_wide_cell_and_its_spacer_stay_in_one_run_and_claim_two_columns() {
        let frame = frame_with(
            vec![
                PackedCell::new("\u{4F60}", 0, Wide::Wide),
                PackedCell::new("", 0, Wide::SpacerTail),
                PackedCell::new("x", 0, Wide::Narrow),
            ],
            vec![[0, 0]],
        );

        let runs: Vec<_> = frame.runs(0).collect();
        assert_eq!(runs.len(), 1);
        assert_eq!(
            runs[0].width(),
            3,
            "two columns for the wide cell, one for x"
        );
        assert_eq!(runs[0].column_of(2), 2, "x is drawn past the wide cell");
    }

    #[test]
    fn a_right_to_left_run_lays_its_cells_out_backwards() {
        // Nothing produces one yet. The assertion is the contract slice 5.5 will satisfy,
        // and the reason a renderer must ask `column_of` instead of adding to `start`.
        let cells: Vec<PackedCell> = "\u{05D0}\u{05D1}\u{05D2}"
            .chars()
            .map(|c| PackedCell::new(&c.to_string(), 0, Wide::Narrow))
            .collect();
        let run = Run {
            start: 4,
            direction: Direction::RightToLeft,
            style: Style::DEFAULT,
            cells: &cells,
        };

        assert_eq!(run.width(), 3);
        assert_eq!(
            run.column_of(0),
            6,
            "the first letter sits at the right edge"
        );
        assert_eq!(run.column_of(1), 5);
        assert_eq!(run.column_of(2), 4);
    }

    #[test]
    fn a_row_is_stale_only_above_the_generation_already_drawn() {
        let mut frame = frame_with(vec![PackedCell::BLANK], vec![[0, 0]]);
        frame.row_generation = vec![7];

        assert!(frame.row_is_stale(0, 6));
        assert!(!frame.row_is_stale(0, 7));
        assert!(!frame.row_is_stale(0, 9));
    }

    #[test]
    fn a_whole_frame_invalidation_outranks_every_row_stamp() {
        let mut frame = frame_with(vec![PackedCell::BLANK], vec![[0, 0]]);
        frame.row_generation = vec![1];
        frame.full_generation = 9;

        assert!(frame.row_is_stale(0, 8));
        assert_eq!(frame.stale_rows(8).count(), 1);
        assert_eq!(frame.stale_rows(9).count(), 0);
    }
}
