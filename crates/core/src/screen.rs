//! Purpose: one screen buffer -- its grid, its cursor, its scroll region, and the VT
//!   operations that act on all three together.
//! Public surface: `Screen`, and the cursor / scroll / erase / insert-delete operations.
//! Why this file: a terminal has two of these (primary and alternate) and they are
//!   genuinely independent -- separate contents, separate cursors, separate margins. Making
//!   that a type instead of a pair of fields is what keeps alt-screen switching a swap.
//! NOT responsible for: parsing, mode state, tab stops, or which screen is active
//!   (`terminal.rs`). It does not know the other screen exists.
//! Test strategy: measured against libghostty-vt by the corpus; unit tests below cover the
//!   region arithmetic, where off-by-ones are cheapest to catch in isolation.

use mind2t_vt_snapshot::Semantic;

use crate::cell::{Cell, Protection};
use crate::grid::Grid;
use crate::history::History;
use crate::style::StyleId;

/// A saved cursor (DECSC / DECRC, and the alternate-screen save).
#[derive(Debug, Clone, Copy)]
pub struct SavedCursor {
    pub x: u16,
    pub y: u16,
    pub pending_wrap: bool,
    /// DECSC carries the protection pen (the oracle's saved cursor does too).
    pub protected: crate::cell::Protection,
}

/// One screen buffer.
#[derive(Debug)]
pub struct Screen {
    pub grid: Grid,
    pub x: u16,
    pub y: u16,
    /// The DEC phantom state: the last column has been written and the wrap is deferred
    /// until the next printable character. Any cursor movement cancels it.
    pub pending_wrap: bool,
    /// Inclusive scroll margins. Default is the whole screen.
    /// Kitty graphics placements anchored to this screen's rows. They scroll with
    /// the content and drop once fully above the region top.
    pub placements: Vec<crate::graphics::Placement>,
    pub scroll_top: u16,
    pub scroll_bottom: u16,
    /// Inclusive LEFT/RIGHT margins (DECSLRM). Default is the whole width. Only
    /// meaningful while DECLRMM (mode 69) is on - DECSLRM refuses to set them
    /// otherwise - but they stay set once established, which is why the mode bit
    /// and the margins are separate state.
    pub scroll_left: u16,
    pub scroll_right: u16,
    pub saved: Option<SavedCursor>,
    /// What OSC 133 says this screen's cursor is currently writing. Per-screen because it is
    /// per-cursor upstream (`Screen.zig:173`): every switch copies it across with the cursor
    /// EXCEPT a 1049 exit, whose `restoreCursor` restores everything but this -- so a `C`
    /// issued on the alternate screen must not leak back (finding 20).
    pub semantic_content: Semantic,
    /// The input state was declared to end at end-of-line, so the next row entry returns the
    /// cursor to output instead of marking a continuation.
    pub semantic_clear_at_eol: bool,
    /// Rows that have scrolled off the top of this screen. The alternate screen is created
    /// with a zero budget, which is how it ends up with no scrollback.
    pub history: History,
    /// The kitty keyboard flag stack. Per SCREEN, matching the oracle (`Screen.zig:73`)
    /// and kitty's spec: a TUI entering the alternate screen pushes its flags without
    /// disturbing the shell's stack, and leaving 1049 restores the shell's negotiation
    /// with no pop required.
    pub kitty_keyboard: crate::kitty_keys::KittyFlagStack,
    /// The protection pen: newly printed cells carry this kind. Per-cursor in the
    /// oracle, so per-screen here. SPA sets Iso, DECSCA 1 sets Dec, EPA and
    /// DECSCA 0/2 clear it.
    pub protected: crate::cell::Protection,
}

impl Screen {
    pub fn new(cols: u16, rows: u16, max_scrollback: usize) -> Screen {
        Screen {
            grid: Grid::new(cols, rows),
            history: History::new(cols, max_scrollback),
            x: 0,
            y: 0,
            pending_wrap: false,
            placements: Vec::new(),
            scroll_top: 0,
            scroll_bottom: rows.saturating_sub(1),
            scroll_left: 0,
            scroll_right: cols.saturating_sub(1),
            saved: None,
            protected: crate::cell::Protection::None,
            semantic_content: Semantic::Output,
            semantic_clear_at_eol: false,
            kitty_keyboard: crate::kitty_keys::KittyFlagStack::default(),
        }
    }

    pub fn cols(&self) -> u16 {
        self.grid.cols()
    }

    pub fn rows(&self) -> u16 {
        self.grid.rows()
    }

    fn last_col(&self) -> u16 {
        self.cols().saturating_sub(1)
    }

    fn last_row(&self) -> u16 {
        self.rows().saturating_sub(1)
    }

    /// Sets the scroll region (DECSTBM). A region that is empty or inverted is rejected
    /// outright rather than clamped -- the spec says ignore, and clamping would silently
    /// give a program a region it did not ask for.
    pub fn set_scroll_region(&mut self, top: u16, bottom: u16) -> bool {
        if top >= bottom || bottom > self.last_row() {
            return false;
        }
        self.scroll_top = top;
        self.scroll_bottom = bottom;
        true
    }

    pub fn reset_scroll_region(&mut self) {
        self.scroll_top = 0;
        self.scroll_bottom = self.last_row();
        self.reset_margins();
    }

    pub fn reset_margins(&mut self) {
        self.scroll_left = 0;
        self.scroll_right = self.last_col();
    }

    /// Whether left/right margins are narrower than the screen. The fast path for
    /// every operation that only has to care when they are.
    pub fn has_margins(&self) -> bool {
        self.scroll_left > 0 || self.scroll_right < self.last_col()
    }

    /// DECSLRM (`Terminal.zig:2372`), measured: left is at least 1, right defaults
    /// to the full width when 0 and clamps to it, an empty or inverted pair is
    /// IGNORED outright, and a successful set homes the cursor. The mode-69 gate
    /// lives at the call site, exactly as the oracle checks it first.
    pub fn set_margins(&mut self, left_req: u16, right_req: u16) -> bool {
        let cols = self.cols();
        let left = left_req.max(1);
        let right = if right_req == 0 { cols } else { right_req.min(cols) };
        if left >= right {
            return false;
        }
        self.scroll_left = left - 1;
        self.scroll_right = right - 1;
        true
    }

    /// The right edge printing and wrapping obey. The oracle recomputes this per
    /// print (`Terminal.zig:1097`) rather than caching it, because a cursor parked
    /// to the RIGHT of the margin uses the screen edge instead - which is what lets
    /// a program write status text outside the margined column band.
    pub fn print_right_limit(&self) -> u16 {
        if self.x > self.scroll_right {
            self.last_col()
        } else {
            self.scroll_right
        }
    }

    pub fn move_to(&mut self, x: u16, y: u16) {
        self.x = x.min(self.last_col());
        self.set_y(y.min(self.last_row()));
        self.pending_wrap = false;
    }

    /// Moves the cursor to a row, damaging the one it left and the one it arrives at.
    ///
    /// Only when the row actually changes. Measured against libghostty-vt 2026-07-28: moving
    /// the cursor WITHIN a row damages nothing, and a cursor that leaves a row and returns to
    /// it still leaves the row it passed through dirty -- so this has to be marked on every
    /// move rather than derived from where the cursor started and ended.
    fn set_y(&mut self, y: u16) {
        if self.y == y {
            return;
        }
        let previous = self.y;
        self.y = y;
        self.grid.mark_dirty(previous);
        self.grid.mark_dirty(y);
    }

    /// Moves down one row, scrolling the region if already at its bottom margin.
    ///
    /// The bottom margin, not the bottom of the screen: a cursor below the region scrolls
    /// nothing and simply moves down, which is what lets a program park status text under
    /// a scrolling area.
    pub fn line_feed(&mut self, blank: Cell) {
        if self.y == self.scroll_bottom {
            self.scroll_region_up(1, blank);
        } else if self.y < self.last_row() {
            let next = self.y + 1;
            self.set_y(next);
        }
        self.pending_wrap = false;
    }

    /// Moves up one row, scrolling the region down if already at its top margin (RI).
    pub fn reverse_index(&mut self, blank: Cell) {
        if self.y == self.scroll_top {
            let (left, right) = (self.scroll_left, self.scroll_right);
            self.grid
                .scroll_down_in(self.scroll_top, self.scroll_bottom, left, right, 1, blank);
        } else if self.y > 0 {
            let previous = self.y - 1;
            self.set_y(previous);
        }
        self.pending_wrap = false;
    }

    /// Resolves a deferred wrap: moves to the next line and, if the cursor was genuinely at
    /// the right edge, marks the two rows as one soft-wrapped line.
    ///
    /// The edge test is not redundant. A deferred wrap survives a reflow verbatim, so the
    /// cursor can arrive here sitting well short of the last column -- widening a screen
    /// leaves exactly that state. Marking a wrap there would join two lines that never were
    /// one, and the next reflow would rejoin them into nonsense. Upstream applies the same
    /// test in `printWrap`.
    pub fn wrap_line(&mut self, blank: Cell) {
        // Only a wrap at the TRUE end of the row is a soft wrap. A wrap at a narrower
        // right margin folds text inside a column band, and the row it leaves is not a
        // continuation of anything - measured: the oracle sets no wrap flag there, and
        // marking one would make reflow rejoin two lines that were never one.
        let at_edge = self.x == self.last_col();
        if at_edge && let Some(meta) = self.grid.row_meta_mut(self.y) {
            meta.wrap = true;
        }
        self.line_feed(blank);
        // The left MARGIN, not column zero: the oracle's printWrap ends with
        // cursorHorizontalAbsolute(scrolling_region.left).
        self.x = self.scroll_left;
        self.pending_wrap = false;
        if at_edge && let Some(meta) = self.grid.row_meta_mut(self.y) {
            meta.wrap_continuation = true;
        }
    }

    /// EL: 0 erases cursor to end of line, 1 start to cursor, 2 the whole line.
    pub fn erase_in_line(&mut self, mode: u16, blank: Cell, spare: Protection) {
        let (from, to) = match mode {
            0 => (self.x, self.last_col()),
            1 => (0, self.x),
            2 => (0, self.last_col()),
            _ => return,
        };
        if spare == Protection::None {
            self.grid.clear_span(self.y, from, to, blank);
        } else {
            self.selective_span(self.y, from, to, blank, spare);
        }
        self.pending_wrap = false;
    }

    /// ED: 0 erases cursor to end of screen, 1 start to cursor, 2 everything,
    /// 3 the scrollback.
    ///
    /// Mode 3 returns before the `pending_wrap` reset rather than falling through it.
    /// Upstream dispatches it straight to `eraseHistory` (`Terminal.zig:3398`) and the other
    /// three modes each clear the phantom state themselves, so touching it here would make
    /// ED 3 cancel a deferred wrap that upstream leaves standing.
    pub fn erase_in_display(&mut self, mode: u16, blank: Cell, spare: Protection) {
        if mode == 3 {
            self.history.clear();
            return;
        }
        if spare != Protection::None {
            let last = self.last_col();
            match mode {
                0 => {
                    self.selective_span(self.y, self.x, last, blank, spare);
                    for y in (self.y + 1)..self.rows() {
                        self.selective_span(y, 0, last, blank, spare);
                    }
                }
                1 => {
                    for y in 0..self.y {
                        self.selective_span(y, 0, last, blank, spare);
                    }
                    self.selective_span(self.y, 0, self.x, blank, spare);
                }
                2 => {
                    for y in 0..self.rows() {
                        self.selective_span(y, 0, last, blank, spare);
                    }
                }
                _ => return,
            }
            self.pending_wrap = false;
            return;
        }
        match mode {
            0 => {
                self.grid.clear_span(self.y, self.x, self.last_col(), blank);
                for y in (self.y + 1)..self.rows() {
                    self.grid.blank_row(y, blank);
                }
            }
            1 => {
                for y in 0..self.y {
                    self.grid.blank_row(y, blank);
                }
                self.grid.clear_span(self.y, 0, self.x, blank);
            }
            2 => {
                for y in 0..self.rows() {
                    self.grid.blank_row(y, blank);
                }
            }
            _ => return,
        }
        self.pending_wrap = false;
    }

    /// Strips protection from every cell on screen. Called only by DECSTR - see the
    /// justification there.
    pub fn clear_protection(&mut self) {
        for y in 0..self.rows() {
            for x in 0..self.cols() {
                let index = self.grid.index(x, y);
                let mut cell = self.grid.cell(index);
                if cell.flags.protection() != Protection::None {
                    cell.flags.set_protection(Protection::None);
                    self.grid.write(index, cell);
                }
            }
        }
    }

    /// A protection-aware span wipe: cells whose protection kind matches `spare`
    /// survive. xterm's matrix - the plain erases spare Iso, the selective spare Dec.
    fn selective_span(&mut self, y: u16, from: u16, to: u16, blank: Cell, spare: Protection) {
        for x in from..=to {
            let index = self.grid.index(x, y);
            if self.grid.cell(index).flags.protection() != spare {
                self.grid.write(index, blank);
            }
        }
    }

    /// ECH: erase `count` cells from the cursor without shifting anything.
    pub fn erase_chars(&mut self, count: u16, blank: Cell, spare: Protection) {
        let to = self.x.saturating_add(count.saturating_sub(1)).min(self.last_col());
        if spare == Protection::None {
            self.grid.clear_span(self.y, self.x, to, blank);
        } else {
            self.selective_span(self.y, self.x, to, blank, spare);
        }
        self.pending_wrap = false;
    }

    /// ICH: insert `count` blanks at the cursor, pushing the rest of the row right.
    pub fn insert_chars(&mut self, count: u16, blank: Cell) {
        let right = self.last_col();
        self.insert_chars_to(count, right, blank);
    }

    pub fn insert_chars_to(&mut self, count: u16, right: u16, blank: Cell) {
        self.grid.shift_right_to(self.y, self.x, right, count, blank);
        self.pending_wrap = false;
    }

    /// DCH: delete `count` cells at the cursor, pulling the rest of the row left.
    pub fn delete_chars(&mut self, count: u16, blank: Cell) {
        let right = self.last_col();
        self.delete_chars_to(count, right, blank);
    }

    pub fn delete_chars_to(&mut self, count: u16, right: u16, blank: Cell) {
        self.grid.shift_left_to(self.y, self.x, right, count, blank);
        self.pending_wrap = false;
    }

    /// IL / DL. Both are no-ops outside the scroll region, and both operate from the cursor
    /// row to the bottom margin rather than to the bottom of the screen.
    pub fn insert_lines(&mut self, count: u16, blank: Cell) {
        if !self.in_region() {
            return;
        }
        let (left, right) = (self.scroll_left, self.scroll_right);
        self.grid
            .scroll_down_in(self.y, self.scroll_bottom, left, right, count, blank);
        self.x = left;
        self.pending_wrap = false;
    }

    pub fn delete_lines(&mut self, count: u16, blank: Cell) {
        if !self.in_region() {
            return;
        }
        let (left, right) = (self.scroll_left, self.scroll_right);
        self.grid
            .scroll_up_in(self.y, self.scroll_bottom, left, right, count, blank);
        self.x = left;
        self.pending_wrap = false;
    }

    /// SU / SD: scroll the region without moving the cursor.
    pub fn scroll_up(&mut self, count: u16, blank: Cell) {
        self.scroll_region_up(count, blank);
    }

    /// Scrolls the region up, saving what leaves to history first.
    ///
    /// Only when the region starts at the top of the screen. A row pushed out of a region
    /// that begins lower down has not left the screen -- it has been overwritten inside it,
    /// and putting it in scrollback would invent history that never scrolled. Same reason
    /// `delete_lines` does not route through here.
    fn scroll_region_up(&mut self, count: u16, blank: Cell) {
        // A row leaving a COLUMN-BANDED region has not left the screen: the cells
        // outside the band stay exactly where they are, so the row still exists and
        // pushing it to scrollback would invent a line that never scrolled. Same
        // reasoning as the top-margin test beside it.
        if self.scroll_top == 0 && !self.has_margins() && self.history.enabled() {
            let limit = count.min(self.rows());
            for y in 0..limit {
                let row = self.grid.extract_row(y);
                self.history.push(row);
            }
        }
        let (left, right) = (self.scroll_left, self.scroll_right);
        self.grid
            .scroll_up_in(self.scroll_top, self.scroll_bottom, left, right, count, blank);
        self.scroll_placements_up(count);
    }

    /// Scrolls everything written into scrollback, so a clear at a prompt keeps history.
    ///
    /// Mirrors `PageList.scrollClear` (`PageList.zig:3099`): it walks the active area from
    /// the bottom for the first row holding text and scrolls by the count that reaches it,
    /// so trailing blank rows are not pushed. It ignores the scroll region -- upstream grows
    /// the page list itself rather than scrolling a region -- which is why this does not
    /// route through `scroll_region_up`.
    pub fn scroll_clear(&mut self, blank: Cell) {
        let rows = self.rows();
        let mut count = 0;
        for y in (0..rows).rev() {
            let row_has_text = (0..self.grid.cols())
                .any(|x| self.grid.cell(self.grid.index(x, y)).has_text());
            if row_has_text {
                count = y + 1;
                break;
            }
        }
        if count == 0 {
            return;
        }
        if self.history.enabled() {
            for y in 0..count {
                let row = self.grid.extract_row(y);
                self.history.push(row);
            }
        }
        let last = self.last_row();
        self.grid.scroll_up(0, last, count, blank);
        self.scroll_placements_up(count);

        // `Screen.scrollClear` then calls `cursorReload` (`Screen.zig:844`), which derives the
        // cursor from its tracked pin: the pin follows its row up, and only when that row has
        // left the active area entirely does it reset to the top-left.
        if self.y >= count {
            let y = self.y - count;
            self.set_y(y);
        } else {
            self.x = 0;
            self.set_y(0);
        }
    }

    pub fn scroll_down(&mut self, count: u16, blank: Cell) {
        self.grid
            .scroll_down_in(
                self.scroll_top,
                self.scroll_bottom,
                self.scroll_left,
                self.scroll_right,
                count,
                blank,
            );
        let (top, bottom) = (self.scroll_top as i16, self.scroll_bottom as i16);
        // Content pushed past the region bottom is DESTROYED (not scrolled away), so a
        // placement following it goes with it.
        self.placements.retain_mut(|placement| {
            if placement.row < top || placement.row > bottom {
                return true;
            }
            placement.row += count as i16;
            placement.row <= bottom
        });
    }

    /// Rows moved up by `count` inside the region. With the region at the screen top
    /// the content flows toward scrollback, so an anchor may go NEGATIVE and keep
    /// drawing its visible remainder (the renderer clips); far enough gone, it drops.
    /// A region starting lower destroys what leaves it, and placements with it.
    fn scroll_placements_up(&mut self, count: u16) {
        let (top, bottom) = (self.scroll_top as i16, self.scroll_bottom as i16);
        const GONE: i16 = -256;
        self.placements.retain_mut(|placement| {
            if top == 0 && placement.row < 0 {
                placement.row -= count as i16;
                return placement.row > GONE;
            }
            if placement.row < top || placement.row > bottom {
                return true;
            }
            placement.row -= count as i16;
            if top == 0 {
                placement.row > GONE
            } else {
                placement.row >= top
            }
        });
    }

    /// Whether the cursor sits inside the scroll region - BOTH axes. IL/DL refuse
    /// outright when the cursor is outside the column band too, measured from the
    /// oracle's own guard (`Terminal.zig:2676-2679`).
    fn in_region(&self) -> bool {
        self.y >= self.scroll_top
            && self.y <= self.scroll_bottom
            && self.x >= self.scroll_left
            && self.x <= self.scroll_right
    }

    pub fn save_cursor(&mut self) {
        self.saved = Some(SavedCursor {
            x: self.x,
            y: self.y,
            pending_wrap: self.pending_wrap,
            protected: self.protected,
        });
    }

    /// With nothing saved, upstream restores a synthetic default cursor
    /// (Terminal.zig:1872): home, no pending wrap. The pen half lives in `Terminal`.
    pub fn restore_cursor(&mut self) {
        let saved = self.saved.unwrap_or(SavedCursor {
            x: 0,
            y: 0,
            pending_wrap: false,
            protected: crate::cell::Protection::None,
        });
        self.x = saved.x.min(self.last_col());
        let y = saved.y.min(self.last_row());
        self.set_y(y);
        self.pending_wrap = saved.pending_wrap;
        self.protected = saved.protected;
    }

    /// Clears the whole buffer, its history and the cursor, as entering the alternate
    /// screen does.
    pub fn reset(&mut self, blank: Cell) {
        self.history.clear();
        for y in 0..self.rows() {
            self.grid.blank_row(y, blank);
        }
        self.x = 0;
        self.y = 0;
        self.pending_wrap = false;
        self.reset_scroll_region();
    }

    /// A blank cell carrying the given style, so erases paint the current background.
    pub fn blank_with(style_id: StyleId) -> Cell {
        Cell {
            style_id,
            ..Cell::BLANK
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(screen: &Screen, y: usize) -> String {
        let rows = screen.grid.to_rows();
        rows[y]
            .cells
            .iter()
            .map(|cell| {
                if cell.text.is_empty() {
                    ' '
                } else {
                    cell.text.chars().next().unwrap()
                }
            })
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    fn put(screen: &mut Screen, y: u16, text: &str) {
        for (x, ch) in text.chars().enumerate() {
            let index = screen.grid.index(x as u16, y);
            screen.grid.write(
                index,
                Cell {
                    codepoint: ch as u32,
                    ..Cell::BLANK
                },
            );
        }
    }

    #[test]
    fn an_inverted_scroll_region_is_rejected_not_clamped() {
        let mut screen = Screen::new(10, 5, 0);
        assert!(!screen.set_scroll_region(3, 1));
        assert_eq!((screen.scroll_top, screen.scroll_bottom), (0, 4));
    }

    #[test]
    fn a_region_past_the_last_row_is_rejected() {
        let mut screen = Screen::new(10, 5, 0);
        assert!(!screen.set_scroll_region(0, 9));
        assert_eq!(screen.scroll_bottom, 4);
    }

    #[test]
    fn line_feed_at_the_bottom_margin_scrolls_only_the_region() {
        let mut screen = Screen::new(10, 5, 0);
        for y in 0..5 {
            put(&mut screen, y, &format!("row{y}"));
        }
        assert!(screen.set_scroll_region(1, 3));
        screen.y = 3;
        screen.line_feed(Cell::BLANK);

        assert_eq!(text_of(&screen, 0), "row0", "above the region is untouched");
        assert_eq!(text_of(&screen, 1), "row2");
        assert_eq!(text_of(&screen, 2), "row3");
        assert_eq!(text_of(&screen, 3), "", "vacated row is blank");
        assert_eq!(text_of(&screen, 4), "row4", "below the region is untouched");
    }

    #[test]
    fn reverse_index_at_the_top_margin_scrolls_the_region_down() {
        let mut screen = Screen::new(10, 4, 0);
        for y in 0..4 {
            put(&mut screen, y, &format!("row{y}"));
        }
        assert!(screen.set_scroll_region(1, 2));
        screen.y = 1;
        screen.reverse_index(Cell::BLANK);

        assert_eq!(text_of(&screen, 0), "row0");
        assert_eq!(text_of(&screen, 1), "");
        assert_eq!(text_of(&screen, 2), "row1");
        assert_eq!(text_of(&screen, 3), "row3");
    }

    #[test]
    fn insert_and_delete_chars_shift_within_the_row_only() {
        let mut screen = Screen::new(8, 2, 0);
        put(&mut screen, 0, "abcdef");
        put(&mut screen, 1, "keepme");

        screen.move_to(2, 0);
        screen.insert_chars(2, Cell::BLANK);
        assert_eq!(text_of(&screen, 0), "ab  cdef");
        assert_eq!(text_of(&screen, 1), "keepme", "the next row is untouched");

        screen.delete_chars(2, Cell::BLANK);
        assert_eq!(text_of(&screen, 0), "abcdef");
    }

    #[test]
    fn erase_in_line_covers_each_mode() {
        let mut screen = Screen::new(6, 1, 0);

        put(&mut screen, 0, "abcdef");
        screen.move_to(3, 0);
        screen.erase_in_line(0, Cell::BLANK, Protection::None);
        assert_eq!(text_of(&screen, 0), "abc");

        put(&mut screen, 0, "abcdef");
        screen.move_to(3, 0);
        screen.erase_in_line(1, Cell::BLANK, Protection::None);
        assert_eq!(text_of(&screen, 0), "    ef");

        put(&mut screen, 0, "abcdef");
        screen.erase_in_line(2, Cell::BLANK, Protection::None);
        assert_eq!(text_of(&screen, 0), "");
    }

    #[test]
    fn insert_lines_stops_at_the_bottom_margin_not_the_screen() {
        let mut screen = Screen::new(6, 5, 0);
        for y in 0..5 {
            put(&mut screen, y, &format!("r{y}"));
        }
        assert!(screen.set_scroll_region(0, 2));
        screen.move_to(0, 0);
        screen.insert_lines(1, Cell::BLANK);

        assert_eq!(text_of(&screen, 0), "");
        assert_eq!(text_of(&screen, 1), "r0");
        assert_eq!(text_of(&screen, 2), "r1");
        assert_eq!(text_of(&screen, 3), "r3", "below the margin is untouched");
    }

    #[test]
    fn line_ops_outside_the_region_do_nothing() {
        let mut screen = Screen::new(6, 5, 0);
        for y in 0..5 {
            put(&mut screen, y, &format!("r{y}"));
        }
        assert!(screen.set_scroll_region(1, 3));
        screen.move_to(0, 4);
        screen.delete_lines(1, Cell::BLANK);

        assert_eq!(text_of(&screen, 4), "r4");
    }

    #[test]
    fn any_cursor_movement_cancels_a_pending_wrap() {
        let mut screen = Screen::new(6, 2, 0);
        screen.pending_wrap = true;
        screen.move_to(1, 0);
        assert!(!screen.pending_wrap);
    }

    #[test]
    fn a_wrapped_line_records_both_flags_for_reflow() {
        let mut screen = Screen::new(6, 3, 0);
        screen.y = 0;
        screen.x = 5;
        screen.pending_wrap = true;
        screen.wrap_line(Cell::BLANK);

        assert!(screen.grid.row_meta(0).wrap, "the row we left soft-wrapped");
        assert!(
            screen.grid.row_meta(1).wrap_continuation,
            "the row we arrived at continues it"
        );
        assert_eq!((screen.x, screen.y), (0, 1));
        assert!(!screen.pending_wrap);
    }

    #[test]
    fn a_deferred_wrap_away_from_the_edge_moves_down_without_joining_the_lines() {
        // Only reachable after a reflow, which carries the phantom state verbatim: widening
        // the screen leaves the cursor mid-row with the wrap still pending. Marking it would
        // fuse two unrelated lines, and the next reflow would rejoin them.
        let mut screen = Screen::new(6, 3, 0);
        screen.y = 0;
        screen.x = 2;
        screen.pending_wrap = true;
        screen.wrap_line(Cell::BLANK);

        assert!(!screen.grid.row_meta(0).wrap);
        assert!(!screen.grid.row_meta(1).wrap_continuation);
        assert_eq!((screen.x, screen.y), (0, 1));
    }

    #[test]
    fn a_blanked_row_loses_its_wrap_flags() {
        // Otherwise a recycled row claims to continue a line that scrolled away.
        let mut screen = Screen::new(6, 2, 0);
        if let Some(meta) = screen.grid.row_meta_mut(0) {
            meta.wrap = true;
            meta.wrap_continuation = true;
        }
        screen.grid.blank_row(0, Cell::BLANK);

        assert!(!screen.grid.row_meta(0).wrap);
        assert!(!screen.grid.row_meta(0).wrap_continuation);
    }
}
