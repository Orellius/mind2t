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
use crate::page::{HistoryCell, HistoryRow};
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
        // Compact BEFORE interning, never after: compaction renumbers, so an ID handed out
        // first would point at the wrong style the moment the table was rebuilt.
        if self.styles.len() >= self.compaction_threshold() {
            self.compact_styles();
        }
        self.styles.intern(style)
    }

    /// The active area can reference at most one style per cell, so a table larger than that
    /// is provably holding entries nothing points at.
    fn compaction_threshold(&self) -> usize {
        (usize::from(self.cols) * usize::from(self.rows)).max(256)
    }

    /// Rebuilds the style table around the styles cells actually reference.
    pub fn compact_styles(&mut self) {
        let mut live: Vec<StyleId> = self.cells.iter().map(|cell| cell.style_id).collect();
        live.sort_unstable();
        live.dedup();

        let remap = self.styles.compact(&live);
        for cell in &mut self.cells {
            if let Some(&new_id) = remap.get(&cell.style_id) {
                cell.style_id = new_id;
            }
        }
    }

    pub fn style(&self, id: StyleId) -> Style {
        self.styles.get(id)
    }

    pub fn row_meta(&self, y: u16) -> RowMeta {
        self.row_meta
            .get(usize::from(y))
            .copied()
            .unwrap_or_default()
    }

    pub fn row_meta_mut(&mut self, y: u16) -> Option<&mut RowMeta> {
        self.row_meta.get_mut(usize::from(y))
    }

    /// Erases cells `from_x..=to_x` on row `y`, filling with `blank`.
    ///
    /// `blank` carries the current background, which is what makes background-colour erase
    /// work: erasing under `CSI 41m` leaves red cells, not empty ones.
    pub fn clear_span(&mut self, y: u16, from_x: u16, to_x: u16, blank: Cell) {
        for x in from_x..=to_x.min(self.cols.saturating_sub(1)) {
            let index = self.index(x, y);
            self.write(index, blank);
        }
    }

    /// Blanks a whole row and resets its wrap flags. A blanked row is not a continuation of
    /// anything, so leaving the flags behind would corrupt reflow in slice 4.
    pub fn blank_row(&mut self, y: u16, blank: Cell) {
        if self.cols > 0 {
            self.clear_span(y, 0, self.cols - 1, blank);
        }
        if let Some(meta) = self.row_meta_mut(y) {
            *meta = RowMeta::default();
        }
    }

    /// Shifts cells on a row right from `at_x`, discarding what falls off the end (ICH).
    pub fn shift_right(&mut self, y: u16, at_x: u16, count: u16, blank: Cell) {
        if self.cols == 0 || at_x >= self.cols {
            return;
        }
        let last = self.cols - 1;
        let count = count.min(self.cols - at_x);
        let mut x = last;
        while x >= at_x + count {
            let source = self.index(x - count, y);
            let target = self.index(x, y);
            self.move_cell(source, target);
            if x == 0 {
                break;
            }
            x -= 1;
        }
        self.clear_span(y, at_x, at_x + count - 1, blank);
    }

    /// Shifts cells on a row left into `at_x`, blanking the tail (DCH).
    pub fn shift_left(&mut self, y: u16, at_x: u16, count: u16, blank: Cell) {
        if self.cols == 0 || at_x >= self.cols {
            return;
        }
        let count = count.min(self.cols - at_x);
        for x in at_x..(self.cols - count) {
            let source = self.index(x + count, y);
            let target = self.index(x, y);
            self.move_cell(source, target);
        }
        self.clear_span(y, self.cols - count, self.cols - 1, blank);
    }

    /// Moves rows within `top..=bottom` up by `count`, blanking the vacated bottom rows.
    pub fn scroll_up(&mut self, top: u16, bottom: u16, count: u16, blank: Cell) {
        let height = bottom.saturating_sub(top) + 1;
        let count = count.min(height);
        for y in top..=bottom.saturating_sub(count) {
            if y + count <= bottom {
                self.move_row(y + count, y);
            }
        }
        for y in (bottom + 1 - count)..=bottom {
            self.blank_row(y, blank);
        }
    }

    /// Moves rows within `top..=bottom` down by `count`, blanking the vacated top rows.
    pub fn scroll_down(&mut self, top: u16, bottom: u16, count: u16, blank: Cell) {
        let height = bottom.saturating_sub(top) + 1;
        let count = count.min(height);
        let mut y = bottom;
        while y >= top + count {
            self.move_row(y - count, y);
            if y == 0 {
                break;
            }
            y -= 1;
        }
        for y in top..(top + count) {
            self.blank_row(y, blank);
        }
    }

    /// Relocates one cell along with its grapheme continuations.
    ///
    /// The continuations must travel with the cell. They are keyed by flat index, so moving
    /// the cell without rekeying them would leave a combining mark attached to whatever
    /// lands at the old position -- a corruption that surfaces far from its cause.
    fn move_cell(&mut self, source: usize, target: usize) {
        if source >= self.cells.len() || target >= self.cells.len() {
            return;
        }
        self.graphemes.remove(&target);
        if let Some(continuations) = self.graphemes.remove(&source) {
            self.graphemes.insert(target, continuations);
        }
        self.cells[target] = self.cells[source];
    }

    /// Relocates a whole row: cells, grapheme continuations and wrap flags.
    ///
    /// O(cols) with a map operation per cell. That is the honest cost of a flat array, and
    /// it is why slice 3 replaces this with a page list where scrolling moves pointers.
    fn move_row(&mut self, from_y: u16, to_y: u16) {
        if from_y == to_y {
            return;
        }
        for x in 0..self.cols {
            let source = self.index(x, from_y);
            let target = self.index(x, to_y);
            self.move_cell(source, target);
        }
        let meta = self.row_meta(from_y);
        if let Some(target) = self.row_meta_mut(to_y) {
            *target = meta;
        }
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

    /// Hands a row over for storage in history, resolving every style.
    ///
    /// Style IDs cannot travel: they index this grid's table, and a page has its own.
    pub fn extract_row(&self, y: u16) -> HistoryRow {
        let cells = (0..self.cols)
            .map(|x| {
                let index = self.index(x, y);
                let cell = self.cell(index);
                HistoryCell {
                    codepoint: cell.codepoint,
                    wide: cell.wide,
                    style: self.style(cell.style_id),
                    graphemes: if cell.flags.has_grapheme() {
                        self.graphemes.get(&index).cloned().unwrap_or_default()
                    } else {
                        Vec::new()
                    },
                }
            })
            .collect();
        HistoryRow {
            cells,
            meta: self.row_meta(y),
        }
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
    use ruuah_vt_snapshot::Color;

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
    fn the_style_table_stays_bounded_under_churn() {
        // 5000 distinct styles through a 16-cell grid. Without compaction the table would
        // hold all 5000 forever; the grid can only ever reference 16 of them.
        let mut grid = Grid::new(4, 4);
        for i in 0..5000u16 {
            let style = Style {
                fg: Color::Palette((i % 256) as u8),
                bold: i % 2 == 0,
                italic: i % 3 == 0,
                ..Style::DEFAULT
            };
            let id = grid.intern_style(style);
            let index = usize::from(i) % 16;
            grid.write(
                index,
                Cell {
                    codepoint: u32::from(b'x'),
                    style_id: id,
                    ..Cell::BLANK
                },
            );
        }
        assert!(
            grid.styles.len() <= 512,
            "style table grew to {}",
            grid.styles.len()
        );
    }

    #[test]
    fn compaction_preserves_what_every_cell_renders_as() {
        let mut grid = Grid::new(4, 1);
        let red = Style {
            fg: Color::Palette(1),
            ..Style::DEFAULT
        };
        let id = grid.intern_style(red);
        grid.write(
            0,
            Cell {
                codepoint: u32::from(b'a'),
                style_id: id,
                ..Cell::BLANK
            },
        );

        grid.compact_styles();

        let rows = grid.to_rows();
        assert_eq!(rows[0].cells[0].style, red, "the cell still renders red");
        assert_eq!(rows[0].cells[1].style, Style::DEFAULT);
    }

    #[test]
    fn out_of_bounds_writes_are_dropped_rather_than_panicking() {
        let mut grid = Grid::new(4, 1);
        grid.write(999, cell_with('x'));
        grid.push_grapheme(999, '\u{301}');
        assert_eq!(grid.cell(999), Cell::BLANK);
    }
}
