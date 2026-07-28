//! Purpose: a fixed-capacity block of scrollback rows that owns everything those rows need.
//! Public surface: `Page`, `PAGE_ROWS`, and the transfer types `HistoryRow` / `HistoryCell`.
//! Why this file: pages are the unit of allocation for scrollback. Each one carries its own
//!   style table and grapheme storage, so dropping a page frees all of it at once -- that is
//!   what makes the memory bounded rather than merely large. Modelled on Ghostty's
//!   `page.zig`, where capacity likewise includes per-page styles and grapheme bytes.
//! NOT responsible for: the prune policy or ordering across pages (`history.rs`), or the
//!   active area (`grid.rs`).
//! Test strategy: unit tests below for trimming and round-trip; the corpus compares the
//!   expanded rows against libghostty-vt.

use std::collections::HashMap;

use ruuah_vt_snapshot::{Row, Semantic, Style};

use crate::cell::{Cell, Wide};
use crate::grid::RowMeta;
use crate::style::StyleTable;

/// Rows per page. A page is the granularity of allocation, not of pruning -- rows are
/// dropped individually from the front so the row budget is exact, and the page is released
/// once it empties.
pub const PAGE_ROWS: usize = 128;

/// One cell on its way into history, with its style already resolved.
///
/// Style IDs are per-table, so a cell leaving the active grid cannot carry its ID with it;
/// it would index a different table and silently render as the wrong style.
#[derive(Debug, Clone)]
pub struct HistoryCell {
    pub codepoint: u32,
    pub wide: Wide,
    pub style: Style,
    pub semantic: Semantic,
    pub graphemes: Vec<char>,
}

/// One row on its way into history.
#[derive(Debug, Clone)]
pub struct HistoryRow {
    pub cells: Vec<HistoryCell>,
    pub meta: RowMeta,
}

#[derive(Debug)]
struct StoredRow {
    /// Trailing blank cells are trimmed, so a mostly-empty 200-column row costs what its
    /// content costs. Ghostty pays full width per row here; this is a deliberate
    /// improvement, and it is invisible from the outside because reads re-pad.
    cells: Vec<Cell>,
    meta: RowMeta,
    graphemes: HashMap<u16, Vec<char>>,
}

/// A block of scrollback rows with its own style table.
#[derive(Debug)]
pub struct Page {
    cols: u16,
    rows: Vec<StoredRow>,
    /// Index of the first live row. Pruning advances this rather than shifting the vector.
    start: usize,
    styles: StyleTable,
}

impl Page {
    pub fn new(cols: u16) -> Page {
        Page {
            cols,
            rows: Vec::with_capacity(PAGE_ROWS),
            start: 0,
            styles: StyleTable::new(),
        }
    }

    /// Whether this page can accept another row. A full page is never reopened, even after
    /// pruning empties part of it, so row order across pages stays append-only.
    pub fn is_full(&self) -> bool {
        self.rows.len() >= PAGE_ROWS
    }

    pub fn len(&self) -> usize {
        self.rows.len() - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn push(&mut self, row: HistoryRow) {
        let mut cells = Vec::with_capacity(row.cells.len());
        let mut graphemes = HashMap::new();

        for (x, cell) in row.cells.iter().enumerate() {
            let style_id = self.styles.intern(cell.style);
            if !cell.graphemes.is_empty() {
                graphemes.insert(x as u16, cell.graphemes.clone());
            }
            let mut flags = crate::cell::CellFlags::with_semantic(cell.semantic);
            flags.set_has_grapheme(!cell.graphemes.is_empty());
            cells.push(Cell {
                codepoint: cell.codepoint,
                style_id,
                wide: cell.wide,
                flags,
            });
        }

        while cells.last() == Some(&Cell::BLANK) {
            cells.pop();
        }

        self.rows.push(StoredRow {
            cells,
            meta: row.meta,
            graphemes,
        });
    }

    /// Drops the oldest row. Returns false when the page is already empty.
    pub fn pop_oldest(&mut self) -> bool {
        if self.is_empty() {
            return false;
        }
        // The row's cells are released; its styles stay until the whole page goes, which is
        // the point of per-page tables -- no refcounting, no compaction, just a bounded set.
        self.rows[self.start] = StoredRow {
            cells: Vec::new(),
            meta: RowMeta::default(),
            graphemes: HashMap::new(),
        };
        self.start += 1;
        true
    }

    /// Hands a stored row back out in transfer form, styles resolved.
    ///
    /// Trailing blanks are not re-padded here: reflow pads to whatever width it is working
    /// from, and padding twice would only make a short row look like content.
    pub fn take_row(&self, index: usize) -> Option<HistoryRow> {
        let stored = self.rows.get(self.start + index)?;
        let cells = stored
            .cells
            .iter()
            .enumerate()
            .map(|(x, cell)| HistoryCell {
                codepoint: cell.codepoint,
                wide: cell.wide,
                style: self.styles.get(cell.style_id),
                semantic: cell.flags.semantic(),
                graphemes: stored
                    .graphemes
                    .get(&(x as u16))
                    .cloned()
                    .unwrap_or_default(),
            })
            .collect();
        Some(HistoryRow {
            cells,
            meta: stored.meta,
        })
    }

    /// Expands a stored row back to full width for comparison.
    pub fn row(&self, index: usize) -> Option<Row> {
        let stored = self.rows.get(self.start + index)?;
        let mut cells = Vec::with_capacity(usize::from(self.cols));
        for x in 0..self.cols {
            let cell = stored
                .cells
                .get(usize::from(x))
                .copied()
                .unwrap_or(Cell::BLANK);
            let mut text = String::new();
            if let Some(first) = char::from_u32(cell.codepoint).filter(|_| cell.codepoint != 0) {
                text.push(first);
                if let Some(rest) = stored.graphemes.get(&x) {
                    text.extend(rest.iter());
                }
            }
            cells.push(ruuah_vt_snapshot::Cell {
                text,
                wide: cell.wide.into(),
                style: self.styles.get(cell.style_id),
                semantic: cell.flags.semantic(),
            });
        }
        Some(Row {
            wrap: stored.meta.wrap,
            wrap_continuation: stored.meta.wrap_continuation,
            semantic_prompt: stored.meta.semantic_prompt,
            cells,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruuah_vt_snapshot::Color;

    fn plain(text: &str, cols: usize) -> HistoryRow {
        let mut cells: Vec<HistoryCell> = text
            .chars()
            .map(|c| HistoryCell {
                codepoint: c as u32,
                wide: Wide::Narrow,
                style: Style::DEFAULT,
                semantic: Semantic::Output,
                graphemes: Vec::new(),
            })
            .collect();
        while cells.len() < cols {
            cells.push(HistoryCell {
                codepoint: 0,
                wide: Wide::Narrow,
                style: Style::DEFAULT,
                semantic: Semantic::Output,
                graphemes: Vec::new(),
            });
        }
        HistoryRow {
            cells,
            meta: RowMeta::default(),
        }
    }

    #[test]
    fn a_row_round_trips_at_full_width() {
        let mut page = Page::new(10);
        page.push(plain("hi", 10));
        let row = page.row(0).expect("row");
        assert_eq!(row.cells.len(), 10);
        assert_eq!(row.cells[0].text, "h");
        assert_eq!(row.cells[1].text, "i");
        assert_eq!(row.cells[9].text, "");
    }

    #[test]
    fn trailing_blanks_are_not_stored_but_are_still_read_back() {
        // The whole point of trimming: cost follows content, and nothing observable changes.
        let mut page = Page::new(200);
        page.push(plain("hi", 200));
        assert_eq!(page.rows[0].cells.len(), 2, "only the content is stored");
        assert_eq!(page.row(0).unwrap().cells.len(), 200, "read back full width");
    }

    #[test]
    fn a_trailing_background_is_not_trimmed_away() {
        // A cell with only a background is not blank, and trimming it would silently discard
        // background-colour erase.
        let mut row = plain("x", 6);
        row.cells[5].style = Style {
            bg: Color::Palette(1),
            ..Style::DEFAULT
        };
        let mut page = Page::new(6);
        page.push(row);

        assert_eq!(page.row(0).unwrap().cells[5].style.bg, Color::Palette(1));
    }

    #[test]
    fn styles_are_reinterned_into_the_pages_own_table() {
        let mut row = plain("ab", 4);
        row.cells[0].style = Style {
            bold: true,
            ..Style::DEFAULT
        };
        let mut page = Page::new(4);
        page.push(row);

        let read = page.row(0).unwrap();
        assert!(read.cells[0].style.bold);
        assert!(!read.cells[1].style.bold);
    }

    #[test]
    fn grapheme_continuations_survive_the_move() {
        let mut row = plain("e", 4);
        row.cells[0].graphemes = vec!['\u{301}'];
        let mut page = Page::new(4);
        page.push(row);

        assert_eq!(page.row(0).unwrap().cells[0].text, "e\u{301}");
    }

    #[test]
    fn pruning_advances_the_start_and_reindexes() {
        let mut page = Page::new(4);
        page.push(plain("a", 4));
        page.push(plain("b", 4));
        assert_eq!(page.len(), 2);

        assert!(page.pop_oldest());
        assert_eq!(page.len(), 1);
        assert_eq!(page.row(0).unwrap().cells[0].text, "b");
    }

    #[test]
    fn a_full_page_reports_full_and_stays_full_after_pruning() {
        // Reopening a pruned page would append newer rows behind older ones.
        let mut page = Page::new(2);
        for _ in 0..PAGE_ROWS {
            page.push(plain("x", 2));
        }
        assert!(page.is_full());
        page.pop_oldest();
        assert!(page.is_full(), "capacity is spent, not freed");
    }
}
