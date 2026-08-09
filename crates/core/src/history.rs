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

use mind2t_vt_snapshot::{Row, RowSemantic};

use crate::page::{HistoryRow, Page};

/// Which viewport offset a prompt jump lands on, or `None` when there is nowhere to go.
///
/// `offsets` is `History::prompt_offsets` and `current` is the viewport offset now - both in
/// offsets-from-end, which is the coordinate the pump's viewport already speaks. Free function
/// rather than a method so the pty crate can reach it without owning a `History`, and so the
/// arithmetic is testable against a literal slice with no scrollback to build.
///
/// `None` is a real answer and the load-bearing one: at the oldest prompt there is nowhere
/// further to go, and staying put is correct. Clamping to the top of history instead would
/// silently turn a prompt jump into a jump-to-top, moving the operator somewhere they never
/// asked to go and losing their place.
///
/// Strictly above / strictly below, never equal - a jump that could return `current` is a key
/// that sometimes does nothing while looking like it worked.
pub fn prompt_jump_offset(offsets: &[usize], current: usize, back: bool) -> Option<usize> {
    if back {
        // Further INTO history is a LARGER offset; `prompt_offsets` runs ascending, so the
        // first entry past `current` is the nearest mark above the viewport.
        offsets.iter().copied().find(|&offset| offset > current)
    } else {
        // Back toward the live bottom: the largest offset still under where we are. Asked as a
        // `max` rather than by position, so it stays correct if that ordering ever changes.
        offsets.iter().copied().filter(|&offset| offset < current).max()
    }
}

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

    /// Offsets-from-end of every row that BEGINS a shell prompt, nearest the live bottom first.
    ///
    /// The coordinate is deliberately the one `rows_from_end` and the pump's viewport offset
    /// already speak, so an answer can be handed straight back as a scroll position with no
    /// conversion - the seam class that made the D2a selection probe dangerous was exactly two
    /// row-coordinate spaces that looked alike.
    ///
    /// `Prompt` only, never `PromptContinuation`. A wrapped two-line prompt is ONE place the
    /// operator wants to land on, and stopping on its second row would scroll the command they
    /// were looking for off the top of the viewport.
    ///
    /// Walks the WHOLE history, O(rows) per call - stated rather than hidden. At the 10,000-row
    /// default that is ten thousand metadata reads on a key press, far inside a frame budget. If
    /// the budget ever grows enough to matter, the fix is an index maintained in `push`, not a
    /// cache here that can go stale against a prune.
    pub fn prompt_offsets(&self) -> Vec<usize> {
        let mut offsets = Vec::new();
        // Pages run oldest-first, so a count from the front converts to an offset-from-end by
        // subtraction. One pass, and no intermediate materialisation of rows.
        let mut from_start = 0usize;
        for page in &self.pages {
            for index in 0..page.len() {
                if let Some(row) = page.row(index) {
                    if row.semantic_prompt == RowSemantic::Prompt {
                        // `len - from_start`: the newest history row is offset 1, matching
                        // `rows_from_end`, where offset 0 is the live bottom and is not a
                        // history row at all.
                        offsets.push(self.len - from_start);
                    }
                }
                from_start += 1;
            }
        }
        // Nearest the bottom first - the order both jump directions read in.
        offsets.reverse();
        offsets
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
    use mind2t_vt_snapshot::{Semantic, Style};

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

    /// Offsets-from-end, ascending, exactly as `prompt_offsets` returns them: three prompts
    /// 5, 12 and 30 rows above the live bottom.
    const PROMPTS: &[usize] = &[5, 12, 30];

    #[test]
    fn a_jump_back_lands_on_the_nearest_prompt_above() {
        // From the live bottom the first jump is the NEWEST prompt, not the oldest.
        assert_eq!(prompt_jump_offset(PROMPTS, 0, true), Some(5));
        assert_eq!(prompt_jump_offset(PROMPTS, 5, true), Some(12));
        assert_eq!(prompt_jump_offset(PROMPTS, 12, true), Some(30));
    }

    #[test]
    fn a_jump_forward_lands_on_the_nearest_prompt_below() {
        assert_eq!(prompt_jump_offset(PROMPTS, 30, false), Some(12));
        assert_eq!(prompt_jump_offset(PROMPTS, 12, false), Some(5));
    }

    /// The load-bearing assertion. `None` at the ends, never a clamp: clamping would silently
    /// turn a prompt jump into a jump-to-top, moving the operator somewhere they never asked
    /// to go and losing their place in the scrollback.
    #[test]
    fn past_the_ends_is_none_and_never_a_clamp() {
        assert_eq!(prompt_jump_offset(PROMPTS, 30, true), None);
        assert_eq!(prompt_jump_offset(PROMPTS, 99, true), None);
        assert_eq!(prompt_jump_offset(PROMPTS, 5, false), None);
        assert_eq!(prompt_jump_offset(PROMPTS, 0, false), None);
        assert_eq!(prompt_jump_offset(&[], 0, true), None);
    }

    /// Landing where you already are is a key that does nothing while looking like it worked.
    #[test]
    fn a_jump_never_returns_the_offset_it_started_on() {
        for &at in PROMPTS {
            assert_ne!(prompt_jump_offset(PROMPTS, at, true), Some(at));
            assert_ne!(prompt_jump_offset(PROMPTS, at, false), Some(at));
        }
    }

    /// A history with no OSC 133 marks - the shell-integration-absent case, which is the
    /// common one until the operator installs it. Empty, not an error, and the jump no-ops.
    #[test]
    fn unmarked_history_has_no_prompts() {
        let mut history = History::new(4, 16);
        for _ in 0..8 {
            history.push(HistoryRow { cells: Vec::new(), meta: RowMeta::default() });
        }
        assert_eq!(history.len(), 8);
        assert!(history.prompt_offsets().is_empty());
        assert_eq!(prompt_jump_offset(&history.prompt_offsets(), 0, true), None);
    }

    /// The offsets a real history reports, in the SAME coordinate `rows_from_end` reads - the
    /// seam worth pinning, because two row spaces that look alike is what made the D2a
    /// selection probe dangerous. Rows go in oldest-first; the newest is offset 1.
    #[test]
    fn prompt_offsets_are_the_same_coordinate_rows_from_end_speaks() {
        let mut history = History::new(4, 16);
        let prompt = RowMeta { semantic_prompt: RowSemantic::Prompt, ..RowMeta::default() };
        // Six rows: prompts at push-order 1 and 4 (0-indexed).
        for index in 0..6 {
            let meta = if index == 1 || index == 4 { prompt } else { RowMeta::default() };
            history.push(HistoryRow { cells: Vec::new(), meta });
        }
        // len 6: push-order 4 is one from the newest -> offset 2; push-order 1 -> offset 5.
        assert_eq!(history.prompt_offsets(), vec![2, 5]);
        // And the coordinate really is rows_from_end's: asking for one row at that offset
        // returns a row carrying the mark. This is what makes the two spaces provably one.
        for offset in history.prompt_offsets() {
            let rows = history.rows_from_end(offset, 1);
            assert_eq!(rows.len(), 1, "offset {offset} names a real row");
            assert_eq!(rows[0].semantic_prompt, RowSemantic::Prompt, "offset {offset}");
        }
    }

    /// A continuation row is NOT a jump target: a wrapped two-line prompt is one place to land,
    /// and stopping on its second row scrolls the command being looked for off the top.
    #[test]
    fn a_prompt_continuation_is_not_a_target() {
        let mut history = History::new(4, 16);
        history.push(HistoryRow {
            cells: Vec::new(),
            meta: RowMeta { semantic_prompt: RowSemantic::Prompt, ..RowMeta::default() },
        });
        history.push(HistoryRow {
            cells: Vec::new(),
            meta: RowMeta {
                semantic_prompt: RowSemantic::PromptContinuation,
                ..RowMeta::default()
            },
        });
        assert_eq!(history.prompt_offsets(), vec![2]);
    }
}
