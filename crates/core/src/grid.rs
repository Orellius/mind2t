//! Purpose: the flat row-major cell array, plus the side storage a POD cell cannot hold.
//! Public surface: `Grid`, `RowMeta`.
//! Why this file: the plan's slice 1 is "wire the parser to a flat row-major cell array",
//!   and this is that array. Grapheme continuations live in a side map keyed by flat cell
//!   index rather than in the cell, which is what keeps `Cell` at 8 bytes.
//! NOT responsible for: parsing, cursor movement, or scrolling (`terminal.rs`). It stores
//!   and reads back; it has no opinion about what wrote to it.
//! Test strategy: unit tests below for indexing and grapheme round-trip; the real proof is
//!   the differential corpus comparing the rendered snapshot against libghostty-vt.

use std::collections::HashMap;

use ruuah_vt_snapshot::{Row, Style};

use crate::cell::Cell;
use crate::style::{StyleId, StyleTable};

/// Per-row state that is not per-cell. Populated from slice 2 onward; carried now because
/// reflow in slice 4 is impossible without a soft-vs-hard wrap flag recorded at write time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RowMeta {
    pub wrap: bool,
    pub wrap_continuation: bool,
}

/// The cell array and everything hanging off it.
#[derive(Debug)]
pub struct Grid {
    cols: u16,
    rows: u16,
    cells: Vec<Cell>,
    row_meta: Vec<RowMeta>,
    /// Continuation codepoints only; the first codepoint of a cluster lives in the cell.
    /// Keyed by flat cell index, and only present when the cell's `has_grapheme` bit is set.
    graphemes: HashMap<usize, Vec<char>>,
    styles: StyleTable,
}

impl Grid {
    pub fn new(cols: u16, rows: u16) -> Grid {
        Grid {
            cols,
            rows,
            cells: vec![Cell::BLANK; usize::from(cols) * usize::from(rows)],
            row_meta: vec![RowMeta::default(); usize::from(rows)],
            graphemes: HashMap::new(),
            styles: StyleTable::new(),
        }
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }

    /// Flat index of a cell. Callers must have already bounds-checked against cols/rows.
    pub fn index(&self, x: u16, y: u16) -> usize {
        usize::from(y) * usize::from(self.cols) + usize::from(x)
    }

    pub fn cell(&self, index: usize) -> Cell {
        self.cells.get(index).copied().unwrap_or(Cell::BLANK)
    }

    /// Overwrites a cell, discarding any grapheme continuations it used to carry.
    ///
    /// Dropping the old continuations is not optional: leaving them behind would attach a
    /// previous cell's combining marks to whatever is written next, which reads as data
    /// corruption and is very hard to trace back to here.
    pub fn write(&mut self, index: usize, cell: Cell) {
        if index >= self.cells.len() {
            return;
        }
        if self.cells[index].flags.has_grapheme() {
            self.graphemes.remove(&index);
        }
        self.cells[index] = cell;
    }

    /// Appends a zero-width codepoint to an existing cell's cluster.
    pub fn push_grapheme(&mut self, index: usize, codepoint: char) {
        if index >= self.cells.len() || !self.cells[index].has_text() {
            return;
        }
        self.cells[index].flags.set_has_grapheme(true);
        self.graphemes.entry(index).or_default().push(codepoint);
    }

    pub fn intern_style(&mut self, style: Style) -> StyleId {
        self.styles.intern(style)
    }

    pub fn style(&self, id: StyleId) -> Style {
        self.styles.get(id)
    }

    pub fn row_meta_mut(&mut self, y: u16) -> Option<&mut RowMeta> {
        self.row_meta.get_mut(usize::from(y))
    }

    /// The full grapheme cluster in a cell, as the snapshot contract wants it: the primary
    /// codepoint followed by any continuations. Empty string for a cell with no text.
    fn cell_text(&self, index: usize) -> String {
        let cell = self.cell(index);
        let Some(first) = char::from_u32(cell.codepoint).filter(|_| cell.has_text()) else {
            return String::new();
        };
        let mut text = String::from(first);
        if cell.flags.has_grapheme() {
            if let Some(rest) = self.graphemes.get(&index) {
                text.extend(rest.iter());
            }
        }
        text
    }

    /// Renders the grid into the implementation-neutral comparison type.
    pub fn to_rows(&self) -> Vec<Row> {
        (0..self.rows)
            .map(|y| {
                let meta = self
                    .row_meta
                    .get(usize::from(y))
                    .copied()
                    .unwrap_or_default();
                Row {
                    wrap: meta.wrap,
                    wrap_continuation: meta.wrap_continuation,
                    cells: (0..self.cols)
                        .map(|x| {
                            let index = self.index(x, y);
                            let cell = self.cell(index);
                            ruuah_vt_snapshot::Cell {
                                text: self.cell_text(index),
                                wide: cell.wide.into(),
                                style: self.style(cell.style_id),
                            }
                        })
                        .collect(),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::{CellFlags, Wide};

    fn cell_with(codepoint: char) -> Cell {
        Cell {
            codepoint: codepoint as u32,
            style_id: 0,
            wide: Wide::Narrow,
            flags: CellFlags::NONE,
        }
    }

    #[test]
    fn indexing_is_row_major() {
        let grid = Grid::new(10, 4);
        assert_eq!(grid.index(0, 0), 0);
        assert_eq!(grid.index(3, 0), 3);
        assert_eq!(grid.index(0, 1), 10);
        assert_eq!(grid.index(9, 3), 39);
    }

    #[test]
    fn a_written_cell_reads_back_as_its_text() {
        let mut grid = Grid::new(4, 2);
        let index = grid.index(1, 1);
        grid.write(index, cell_with('q'));
        assert_eq!(grid.cell_text(index), "q");
    }

    #[test]
    fn an_untouched_cell_is_empty_not_a_space() {
        let grid = Grid::new(4, 2);
        assert_eq!(grid.cell_text(0), "");
        assert!(!grid.cell(0).has_text());
    }

    #[test]
    fn continuations_round_trip_as_one_cluster() {
        let mut grid = Grid::new(4, 1);
        grid.write(0, cell_with('e'));
        grid.push_grapheme(0, '\u{301}');
        assert_eq!(grid.cell_text(0), "e\u{301}");
        assert!(grid.cell(0).flags.has_grapheme());
    }

    #[test]
    fn overwriting_a_cell_drops_its_old_continuations() {
        // Otherwise the previous cell's combining marks silently attach to the new text.
        let mut grid = Grid::new(4, 1);
        grid.write(0, cell_with('e'));
        grid.push_grapheme(0, '\u{301}');
        grid.write(0, cell_with('x'));

        assert_eq!(grid.cell_text(0), "x");
        assert!(!grid.cell(0).flags.has_grapheme());
    }

    #[test]
    fn a_continuation_on_an_empty_cell_is_ignored() {
        let mut grid = Grid::new(4, 1);
        grid.push_grapheme(0, '\u{301}');
        assert_eq!(grid.cell_text(0), "");
    }

    #[test]
    fn out_of_bounds_writes_are_dropped_rather_than_panicking() {
        let mut grid = Grid::new(4, 1);
        grid.write(999, cell_with('x'));
        grid.push_grapheme(999, '\u{301}');
        assert_eq!(grid.cell(999), Cell::BLANK);
    }
}
