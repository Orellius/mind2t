//! Purpose: the scrollback -- an ordered list of pages with a hard row budget.
//! Public surface: `History`.
//! Why this file: "a paged structure with bounded memory, not an array of rows". Pages are
//!   the allocation unit; the budget is enforced in rows so the bound is exact rather than
//!   rounded up to a page. Dropping a page releases its cells, its style table and its
//!   grapheme storage together.
//! NOT responsible for: deciding WHEN a row leaves the active area (`screen.rs`), or how a
//!   row is stored inside a page (`page.rs`).
//! Test strategy: unit tests below own the prune policy, because it is not differentially
//!   testable -- Ghostty's limit is a memory budget scaled by width (measured 2026-07-28:
//!   3000 lines at 6 columns kept 2998, the same at 80 columns kept 634), while this one is
//!   a row count. Corpus cases stay under both thresholds, where the two agree exactly.

use std::collections::VecDeque;

use ruuah_vt_snapshot::Row;

use crate::page::{HistoryRow, Page};

/// Scrollback for one screen.
#[derive(Debug)]
pub struct History {
    pages: VecDeque<Page>,
    cols: u16,
    /// Row budget. Zero disables scrollback entirely, which is what the alternate screen and
    /// a `max_scrollback = 0` terminal both want.
    max_rows: usize,
    len: usize,
    /// Every row ever pushed, monotonic, never reduced by pruning or clearing. A viewport
    /// pinned to content needs "how many rows entered history since I last looked" -- `len`
    /// cannot answer that once the budget is met, because pushes stop changing it.
    total_pushed: u64,
}

impl History {
    pub fn new(cols: u16, max_rows: usize) -> History {
        History {
            pages: VecDeque::new(),
            cols,
            max_rows,
            len: 0,
            total_pushed: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn enabled(&self) -> bool {
        self.max_rows > 0
    }

    /// Appends a row that has scrolled off the top of the active area, pruning to budget.
    pub fn push(&mut self, row: HistoryRow) {
        if self.max_rows == 0 {
            return;
        }

        let needs_page = self.pages.back().is_none_or(Page::is_full);
        if needs_page {
            self.pages.push_back(Page::new(self.cols));
        }
        if let Some(page) = self.pages.back_mut() {
            page.push(row);
            self.len += 1;
            self.total_pushed += 1;
        }

        self.prune();
    }

    /// Every row ever pushed, monotonic. Pruning and clearing never reduce it, so two reads
    /// of this bracket exactly the rows that scrolled off in between -- which is what keeps
    /// a scrolled viewport pinned to its content while the child keeps printing.
    pub fn total_pushed(&self) -> u64 {
        self.total_pushed
    }

    /// The `count` rows ending `offset` rows above the bottom of history, oldest first,
    /// expanded to full width. `offset` past the top is clamped; the tail is truncated at
    /// the bottom. `rows_from_end(0, n)` is the newest `n` rows.
    ///
    /// Walks pages rather than materialising the whole history: a 10,000-row scrollback
    /// must not be copied to show a 40-row window of it.
    pub fn rows_from_end(&self, offset: usize, count: usize) -> Vec<Row> {
        let offset = offset.min(self.len);
        let start = self.len - offset;
        let take = count.min(self.len - start);

        let mut rows = Vec::with_capacity(take);
        let mut skip = start;
        for page in &self.pages {
            if rows.len() == take {
                break;
            }
            if skip >= page.len() {
                skip -= page.len();
                continue;
            }
            for index in skip..page.len() {
                if rows.len() == take {
                    break;
                }
                if let Some(row) = page.row(index) {
                    rows.push(row);
                }
            }
            skip = 0;
        }
        rows
    }

    /// Drops the oldest rows until the budget is met, releasing whole pages as they empty.
    fn prune(&mut self) {
        while self.len > self.max_rows {
            let Some(front) = self.pages.front_mut() else {
                break;
            };
            if front.pop_oldest() {
                self.len -= 1;
            }
            if front.is_empty() {
                self.pages.pop_front();
            }
        }
    }

    /// Every history row, oldest first, expanded to full width.
    pub fn to_rows(&self) -> Vec<Row> {
        let mut rows = Vec::with_capacity(self.len);
        for page in &self.pages {
            for index in 0..page.len() {
                if let Some(row) = page.row(index) {
                    rows.push(row);
                }
            }
        }
        rows
    }

    pub fn clear(&mut self) {
        self.pages.clear();
        self.len = 0;
    }

    /// The row budget, so a resize can rebuild this history at a new width without the
    /// caller having to remember what it was created with.
    pub fn max_rows(&self) -> usize {
        self.max_rows
    }

    /// Empties the history, handing every row back oldest-first in transfer form.
    ///
    /// Reflow consumes the whole scrollback and re-pushes it at the new width, which is why
    /// this drains rather than borrows: the pages it came from are about to be dropped.
    pub fn drain(&mut self) -> Vec<HistoryRow> {
        let mut rows = Vec::with_capacity(self.len);
        for page in &self.pages {
            for index in 0..page.len() {
                if let Some(row) = page.take_row(index) {
                    rows.push(row);
                }
            }
        }
        self.clear();
        rows
    }

    /// Points this history at a new width. Only meaningful while it is empty, which is the
    /// state a resize leaves it in after draining.
    pub fn set_cols(&mut self, cols: u16) {
        self.cols = cols;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Wide;
    use crate::grid::RowMeta;
    use crate::page::{HistoryCell, PAGE_ROWS};
    use ruuah_vt_snapshot::{Semantic, Style};

    fn row(text: &str) -> HistoryRow {
        HistoryRow {
            cells: text
                .chars()
                .map(|c| HistoryCell {
                    link: None,
                    codepoint: c as u32,
                    wide: Wide::Narrow,
                    style: Style::DEFAULT,
                    semantic: Semantic::Output,
                    graphemes: Vec::new(),
                })
                .collect(),
            meta: RowMeta::default(),
        }
    }

    fn texts(history: &History) -> Vec<String> {
        history
            .to_rows()
            .iter()
            .map(|r| {
                r.cells
                    .iter()
                    .map(|c| c.text.as_str())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn rows_come_back_in_the_order_they_scrolled_off() {
        let mut history = History::new(4, 100);
        for text in ["a", "b", "c"] {
            history.push(row(text));
        }
        assert_eq!(texts(&history), ["a", "b", "c"]);
    }

    #[test]
    fn a_zero_budget_keeps_nothing() {
        let mut history = History::new(4, 0);
        history.push(row("a"));
        assert_eq!(history.len(), 0);
        assert!(!history.enabled());
    }

    #[test]
    fn the_budget_is_exact_not_rounded_up_to_a_page() {
        // Page-granularity pruning would overshoot by up to PAGE_ROWS, which is the usual
        // shortcut here. The budget is in rows, so it is honoured in rows.
        let mut history = History::new(4, 5);
        for i in 0..50 {
            history.push(row(&format!("l{i}")));
        }
        assert_eq!(history.len(), 5);
        assert_eq!(texts(&history), ["l45", "l46", "l47", "l48", "l49"]);
    }

    #[test]
    fn pages_are_released_once_they_empty() {
        let mut history = History::new(4, 10);
        for i in 0..(PAGE_ROWS * 3) {
            history.push(row(&format!("l{i}")));
        }
        assert_eq!(history.len(), 10);
        assert!(
            history.pages.len() <= 2,
            "emptied pages must be dropped, held {}",
            history.pages.len()
        );
    }

    #[test]
    fn a_new_page_opens_only_when_the_last_one_is_full() {
        let mut history = History::new(4, 100_000);
        for i in 0..PAGE_ROWS {
            history.push(row(&format!("l{i}")));
        }
        assert_eq!(history.pages.len(), 1);
        history.push(row("overflow"));
        assert_eq!(history.pages.len(), 2);
    }

    #[test]
    fn the_pushed_counter_keeps_counting_after_the_budget_is_met() {
        // The reason the counter exists: once the budget is met, `len` stops moving on a
        // push, and a viewport pinned by watching `len` silently drifts one row per prune.
        let mut history = History::new(4, 3);
        for i in 0..10 {
            history.push(row(&format!("l{i}")));
        }
        assert_eq!(history.len(), 3);
        assert_eq!(history.total_pushed(), 10);

        history.clear();
        assert_eq!(
            history.total_pushed(),
            10,
            "clearing forgets rows, never the count of rows that ever scrolled off"
        );
    }

    #[test]
    fn a_zero_budget_pushes_count_nothing() {
        let mut history = History::new(4, 0);
        history.push(row("a"));
        assert_eq!(history.total_pushed(), 0, "a refused push never happened");
    }

    #[test]
    fn rows_from_end_reads_the_window_a_viewport_shows() {
        let mut history = History::new(4, 100);
        for i in 0..10 {
            history.push(row(&format!("l{i}")));
        }

        let window: Vec<String> = history
            .rows_from_end(4, 2)
            .iter()
            .map(|r| {
                r.cells
                    .iter()
                    .map(|c| c.text.as_str())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect();
        assert_eq!(window, ["l6", "l7"], "offset 4 starts 4 rows above the bottom");

        assert_eq!(history.rows_from_end(0, 5).len(), 0, "offset 0 shows no history");
        assert_eq!(
            history.rows_from_end(3, 5).len(),
            3,
            "the take is truncated at the bottom of history"
        );
        assert_eq!(
            history.rows_from_end(500, 2).len(),
            2,
            "an offset past the top clamps rather than fails"
        );
    }

    #[test]
    fn rows_from_end_crosses_page_boundaries() {
        let mut history = History::new(4, 100_000);
        let total = PAGE_ROWS * 2 + 10;
        for i in 0..total {
            history.push(row(&format!("l{i}")));
        }
        // A window straddling the seam between page 0 and page 1.
        let offset = total - PAGE_ROWS + 2;
        let window = history.rows_from_end(offset, 4);
        let texts: Vec<String> = window
            .iter()
            .map(|r| {
                r.cells
                    .iter()
                    .map(|c| c.text.as_str())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect();
        let first = PAGE_ROWS - 2;
        assert_eq!(
            texts,
            (first..first + 4)
                .map(|i| format!("l{i}"))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn clearing_drops_everything() {
        let mut history = History::new(4, 100);
        history.push(row("a"));
        history.clear();
        assert!(history.is_empty());
        assert_eq!(history.to_rows().len(), 0);
    }
}
