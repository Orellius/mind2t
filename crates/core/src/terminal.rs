//! Purpose: drive two screen buffers from a byte stream, using `vte` as the parser.
//! Public surface: `Terminal::new`, `Terminal::write`, `Terminal::resize`,
//!   `Terminal::snapshot`.
//! Why this file: the plan is explicit that VT parsing is solved and must not be rewritten,
//!   so this is only the `Perform` side -- what each parsed action does. It is the
//!   imperative shell over a functional core: no I/O, no clock, fully deterministic.
//! NOT responsible for: parsing (`vte`), control-sequence dispatch (`dispatch.rs`), buffer
//!   operations (`screen.rs`, `grid.rs`), style decoding (`sgr.rs`), tabs (`tabs.rs`),
//!   scrollback (`history.rs`) or reflow (`reflow.rs`, `resize.rs`).
//! Test strategy: measured against libghostty-vt by the differential corpus rather than by
//!   restating expected cell contents here.

use ruuah_vt_snapshot::{
    Cursor, Damage, Dirty, RowSemantic, Screen as SnapshotScreen, Snapshot, Style,
};
use unicode_width::UnicodeWidthChar;
use vte::{Params, Perform};

use crate::cell::{Cell, CellFlags, Wide};
use crate::reflow::Mode;
use crate::screen::Screen;
use crate::tabs::TabStops;

/// A terminal core: bytes in, grid mutations out.
pub struct Terminal {
    parser: vte::Parser,
    state: State,
}

impl Terminal {
    pub fn new(cols: u16, rows: u16) -> Terminal {
        Terminal::with_scrollback(cols, rows, 0)
    }

    pub fn with_scrollback(cols: u16, rows: u16, max_scrollback: usize) -> Terminal {
        Terminal {
            parser: vte::Parser::new(),
            state: State::new(cols, rows, max_scrollback),
        }
    }

    /// Feeds bytes to the parser. Resumable: `vte` carries partial UTF-8 and partial escape
    /// sequences across calls, so a sequence split mid-stream is handled.
    pub fn write(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.state, bytes);
    }

    /// Resizes both screens. The primary reflows, the alternate does not.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.state.resize(cols, rows);
    }

    /// Discards accumulated damage, starting a fresh observation window.
    pub fn clear_damage(&mut self) {
        self.state.full_damage = false;
        self.state.screen_mut().grid.clear_dirty();
    }

    /// What a renderer would have to repaint since damage was last cleared.
    pub fn damage(&self) -> Option<Damage> {
        Some(self.state.damage())
    }

    pub fn snapshot(&self) -> Snapshot {
        self.state.snapshot()
    }

    /// The active screen buffer, for a consumer that reads the grid directly.
    ///
    /// `snapshot` allocates a `String` per cell, which is right for a corpus case and wrong
    /// for a renderer reading every frame. This is the same state without the convenience.
    pub fn screen(&self) -> &Screen {
        self.state.screen()
    }

    pub fn cursor(&self) -> Cursor {
        let screen = self.state.screen();
        Cursor {
            x: screen.x,
            y: screen.y,
            pending_wrap: screen.pending_wrap,
            visible: self.state.cursor_visible,
            style: self.state.pen,
        }
    }

    /// Whether the accumulated damage is a whole-frame event rather than a set of rows.
    pub fn is_wholly_damaged(&self) -> bool {
        self.state.full_damage
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Active {
    Primary,
    Alternate,
}

pub(crate) struct State {
    pub(crate) primary: Screen,
    pub(crate) alternate: Screen,
    pub(crate) active: Active,
    /// The style newly printed cells take.
    pub(crate) pen: Style,
    pub(crate) saved_pen: Option<Style>,
    pub(crate) tabs: TabStops,
    /// DECAWM (mode 7). On by default; when off, the last column overwrites in place.
    pub(crate) autowrap: bool,
    /// DECOM (mode 6). When on, row addressing is relative to the scroll region.
    pub(crate) origin: bool,
    /// DECTCEM (mode 25).
    pub(crate) cursor_visible: bool,
    /// Flat index of the last printed cell, so a following zero-width codepoint knows which
    /// cluster it belongs to. Cleared by anything that moves the cursor.
    pub(crate) last_print: Option<usize>,
    pub(crate) max_scrollback: usize,
    /// Something changed that no per-row flag can express, so the whole frame is stale.
    ///
    /// Four triggers, each confirmed in upstream's `Terminal.zig` as the places that set its
    /// `dirty.clear` bit: a complete erase (ED 2, but NOT ED 0/1/3), a resize, a screen
    /// switch in either direction, and RIS. Measured the same way from the outside.
    pub(crate) full_damage: bool,
}

impl State {
    fn new(cols: u16, rows: u16, max_scrollback: usize) -> State {
        State {
            primary: Screen::new(cols, rows, max_scrollback),
            // The alternate screen has no scrollback, by protocol.
            alternate: Screen::new(cols, rows, 0),
            active: Active::Primary,
            pen: Style::DEFAULT,
            saved_pen: None,
            tabs: TabStops::new(cols),
            autowrap: true,
            origin: false,
            cursor_visible: true,
            last_print: None,
            max_scrollback,
            full_damage: false,
        }
    }

    /// Flags the whole frame as stale, and every row with it.
    ///
    /// Both halves: upstream rebuilds the entire render state for these events, so every row
    /// comes back dirty as well as the global flag. Setting only the global one would report
    /// a clean row set for a frame that is entirely stale.
    pub(crate) fn mark_full_damage(&mut self) {
        self.full_damage = true;
        let rows = self.screen().rows();
        for y in 0..rows {
            self.screen_mut().grid.mark_dirty(y);
        }
    }

    /// What a renderer would have to repaint.
    ///
    /// `Partial` whenever any row is dirty, `Full` only for the whole-frame triggers. A
    /// scroll dirties every row and is still `Partial`: the distinction is not "how many
    /// rows" but "is per-row information meaningful at all".
    fn damage(&self) -> Damage {
        let rows: Vec<bool> = self.screen().grid.dirty_rows().to_vec();
        let global = if self.full_damage {
            Dirty::Full
        } else if rows.iter().any(|dirty| *dirty) {
            Dirty::Partial
        } else {
            Dirty::None
        };
        Damage { global, rows }
    }

    pub(crate) fn screen(&self) -> &Screen {
        match self.active {
            Active::Primary => &self.primary,
            Active::Alternate => &self.alternate,
        }
    }

    pub(crate) fn screen_mut(&mut self) -> &mut Screen {
        match self.active {
            Active::Primary => &mut self.primary,
            Active::Alternate => &mut self.alternate,
        }
    }

    /// The cell an erase paints with.
    ///
    /// Carries the pen's background and nothing else: background-colour erase fills with the
    /// current background, and the other attributes are not observable on a cell with no
    /// text. Interned against the active grid, since style tables are per-grid.
    pub(crate) fn blank(&mut self) -> Cell {
        let style = Style {
            bg: self.pen.bg,
            ..Style::DEFAULT
        };
        let style_id = self.screen_mut().grid.intern_style(style);
        Screen::blank_with(style_id)
    }

    fn snapshot(&self) -> Snapshot {
        let screen = self.screen();
        Snapshot {
            cols: screen.cols(),
            rows: screen.rows(),
            screen: match self.active {
                Active::Primary => SnapshotScreen::Primary,
                Active::Alternate => SnapshotScreen::Alternate,
            },
            cursor: Cursor {
                x: screen.x,
                y: screen.y,
                pending_wrap: screen.pending_wrap,
                visible: self.cursor_visible,
                style: self.pen,
            },
            grid: screen.grid.to_rows(),
            history: screen.history.to_rows(),
            damage: None,
        }
    }

    fn print_char(&mut self, c: char, width: u16) {
        let blank = self.blank();

        // A deferred wrap is resolved before the character lands, never after: the whole
        // point of the phantom state is that the wrap belongs to this character, not the
        // previous one.
        if self.screen().pending_wrap {
            if self.autowrap {
                self.wrap(blank);
            } else {
                self.screen_mut().pending_wrap = false;
            }
        }

        let cols = self.screen().cols();
        if cols == 0 {
            return;
        }

        // A wide character that cannot fit in the last column leaves a spacer head behind
        // and starts on the next row, rather than being split across the wrap.
        if width == 2 && self.screen().x + 1 >= cols {
            if !self.autowrap {
                return;
            }
            // Through the ordinary cell path, not the erase blank: upstream's spacer head
            // takes cursor.style_id and the cursor's semantic_content (Terminal.zig:1411
            // into the write at 1565), so it is bold-and-red under a bold red pen and part
            // of the input under OSC 133 (finding 27).
            let pen = self.pen;
            let semantic = self.screen().semantic_content;
            let style_id = self.screen_mut().grid.intern_style(pen);
            let (x, y) = (self.screen().x, self.screen().y);
            let index = self.screen().grid.index(x, y);
            self.screen_mut().grid.write(
                index,
                Cell {
                    codepoint: 0,
                    style_id,
                    wide: Wide::SpacerHead,
                    flags: CellFlags::with_semantic(semantic),
                },
            );
            self.wrap(blank);
        }

        let pen = self.pen;
        let semantic = self.screen().semantic_content;
        let style_id = self.screen_mut().grid.intern_style(pen);
        let (x, y) = (self.screen().x, self.screen().y);
        let index = self.screen().grid.index(x, y);
        self.screen_mut().grid.write(
            index,
            Cell {
                codepoint: c as u32,
                style_id,
                wide: if width == 2 { Wide::Wide } else { Wide::Narrow },
                flags: CellFlags::with_semantic(semantic),
            },
        );
        self.last_print = Some(index);

        if width == 2 {
            self.screen_mut().grid.write(
                index + 1,
                Cell {
                    codepoint: 0,
                    style_id,
                    wide: Wide::SpacerTail,
                    flags: CellFlags::with_semantic(semantic),
                },
            );
        }

        // At the right edge the cursor stays put and the wrap is deferred. Advancing to
        // `cols` instead would be off the grid and would lose the phantom state.
        if x + width >= cols {
            let screen = self.screen_mut();
            screen.x = cols - 1;
            screen.pending_wrap = true;
        } else {
            self.screen_mut().x = x + width;
        }
    }

    pub(crate) fn cursor_x(&self) -> u16 {
        self.screen().x
    }

    pub(crate) fn cursor_y(&self) -> u16 {
        self.screen().y
    }

    /// Moves the cursor and invalidates the grapheme anchor: a combining mark arriving after
    /// a jump belongs to nothing, and attaching it to a stale cell would corrupt that cell.
    pub(crate) fn goto(&mut self, x: u16, y: u16) {
        self.screen_mut().move_to(x, y);
        self.last_print = None;
    }

    /// Absolute cursor addressing, honouring DECOM: with origin mode on, row 1 is the top
    /// margin and the cursor cannot leave the region.
    pub(crate) fn cursor_position(&mut self, row: u16, col: u16) {
        let (top, bottom) = if self.origin {
            (self.screen().scroll_top, self.screen().scroll_bottom)
        } else {
            (0, self.screen().rows().saturating_sub(1))
        };
        let y = top.saturating_add(row).min(bottom);
        self.screen_mut().move_to(col, y);
        self.last_print = None;
    }

    /// CUU. The scroll region bounds the move, not the screen.
    ///
    /// A cursor at or below the top margin cannot be carried above it; one already above the
    /// region is bounded by the top of the screen instead, because a cursor outside the region
    /// is not confined by it (`Terminal.zig:1703`). Clamping to the screen alone lets CUU walk
    /// out of the region from below, which every full-screen TUI would hit.
    pub(crate) fn cursor_up(&mut self, count: u16) {
        let y = self.cursor_y();
        let top = self.screen().scroll_top;
        let limit = if y >= top { y - top } else { y };
        let x = self.cursor_x();
        self.goto(x, y - count.min(limit));
    }

    /// CUD, the mirror rule (`Terminal.zig:1721`).
    pub(crate) fn cursor_down(&mut self, count: u16) {
        let y = self.cursor_y();
        let bottom = self.screen().scroll_bottom;
        let limit = if y <= bottom {
            bottom - y
        } else {
            self.screen().rows().saturating_sub(1).saturating_sub(y)
        };
        let x = self.cursor_x();
        self.goto(x, y + count.min(limit));
    }

    /// ED 2 at a prompt scrolls the screen into scrollback before clearing, so `^L` keeps
    /// history (`Terminal.zig:3303-3337`, the `at_prompt` block).
    ///
    /// Upstream walks the active area upwards from the bottom, but `SemanticPrompt` has only
    /// the three values matched there -- a prompt row breaks the loop and a `none` row aborts
    /// it -- so the decision is settled by the bottom row alone. The alternate screen is
    /// excluded: it has no scrollback to keep.
    pub(crate) fn scroll_clear_at_prompt(&mut self, blank: Cell) {
        if self.active != Active::Primary {
            return;
        }
        let last = self.screen().rows().saturating_sub(1);
        let at_prompt = matches!(
            self.screen().grid.row_meta(last).semantic_prompt,
            RowSemantic::Prompt | RowSemantic::PromptContinuation
        );
        if at_prompt {
            self.screen_mut().scroll_clear(blank);
        }
    }

    pub(crate) fn save_cursor(&mut self) {
        self.screen_mut().save_cursor();
        self.saved_pen = Some(self.pen);
    }

    pub(crate) fn restore_cursor(&mut self) {
        self.screen_mut().restore_cursor();
        self.pen = self.saved_pen.unwrap_or(Style::DEFAULT);
        self.last_print = None;
    }

    /// Upstream's horizontalTab is a bare cursorRight loop that never touches
    /// `pending_wrap` (Terminal.zig:2111) -- at the last column it does nothing at all.
    pub(crate) fn tab_forward(&mut self, count: u16) {
        for _ in 0..count.max(1) {
            let next = self.tabs.next(self.screen().x);
            self.screen_mut().x = next;
        }
        self.last_print = None;
    }

    pub(crate) fn tab_backward(&mut self, count: u16) {
        for _ in 0..count.max(1) {
            let previous = self.tabs.previous(self.screen().x);
            self.screen_mut().x = previous;
        }
        self.screen_mut().pending_wrap = false;
        self.last_print = None;
    }

    /// Resizes both screens and rebuilds the tab stops for the new width.
    ///
    /// Both screens, not just the active one: a program that resizes while in the alternate
    /// screen and then leaves it would otherwise find the primary still at the old geometry.
    pub(crate) fn resize(&mut self, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 {
            return;
        }
        // The primary rejoins soft-wrapped lines only while DECAWM is on -- upstream passes
        // reflow = modes.get(.wraparound) (Terminal.zig:3783). The alternate is documented
        // by the ABI as not reflowing, so its rows only gain or lose columns.
        let primary_mode = if self.autowrap {
            Mode::Rejoin
        } else {
            Mode::Truncate
        };
        let cols_changed = cols != self.screen().cols();
        crate::resize::apply(&mut self.primary, cols, rows, primary_mode);
        crate::resize::apply(&mut self.alternate, cols, rows, Mode::Truncate);

        // Only when the column count changed -- upstream guards its rebuild on exactly that
        // (`Terminal.zig:3766`), so a rows-only resize keeps custom HTS stops (finding 21).
        if cols_changed {
            self.tabs = TabStops::new(cols);
        }
        self.last_print = None;
        // After the grids are rebuilt, so the whole NEW geometry is marked rather than the
        // old row count. Upstream sets the same clear bit from its own resize.
        self.mark_full_damage();
    }

    pub(crate) fn full_reset(&mut self) {
        let cols = self.screen().cols();
        let rows = self.screen().rows();
        *self = State::new(cols, rows, self.max_scrollback);
        self.mark_full_damage();
    }
}

impl Perform for State {
    fn print(&mut self, c: char) {
        // A zero-width codepoint continues the previous cell's grapheme cluster rather than
        // claiming a cell -- ranked failure mode 2, a cell is not a codepoint. This is the
        // width heuristic, which matches Ghostty while DEC mode 2027 is off (verified
        // 2026-07-28); full UAX #29 segmentation is only needed once 2027 is implemented.
        let width = UnicodeWidthChar::width(c).unwrap_or(0);
        if width == 0 {
            if let Some(index) = self.last_print {
                self.screen_mut().grid.push_grapheme(index, c);
            }
            return;
        }
        self.print_char(c, if width >= 2 { 2 } else { 1 });
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x08 => {
                let x = self.screen().x.saturating_sub(1);
                self.screen_mut().x = x;
                self.screen_mut().pending_wrap = false;
                self.last_print = None;
            }
            0x09 => self.tab_forward(1),
            0x0a | 0x0b | 0x0c => {
                let blank = self.blank();
                self.index(blank);
                self.last_print = None;
            }
            0x0d => {
                self.screen_mut().x = 0;
                self.screen_mut().pending_wrap = false;
                self.last_print = None;
            }
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        let blank = self.blank();
        self.osc(params, blank);
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        self.csi(params, intermediates, ignore, action);
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        self.esc(intermediates, ignore, byte);
    }
}
