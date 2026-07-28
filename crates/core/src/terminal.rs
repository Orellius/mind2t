//! Purpose: drive the grid from a byte stream, using `vte` as the parser.
//! Public surface: `Terminal::new`, `Terminal::write`, `Terminal::snapshot`.
//! Why this file: the plan is explicit that VT parsing is solved and must not be rewritten,
//!   so this is only the `Perform` side -- what each parsed action does to the grid. It is
//!   the imperative shell over a functional core: no I/O, no clock, fully deterministic.
//! NOT responsible for: parsing (`vte`), style decoding (`sgr.rs`), storage (`grid.rs`), or
//!   anything slice 2 owns -- autowrap, scrolling, scroll regions, alt screen, tabs, erase.
//!   Those are deliberate omissions and the corpus expects the resulting diffs.
//! Test strategy: measured against libghostty-vt by the differential corpus, not by
//!   restating expected cell contents here.

use ruuah_vt_snapshot::{Cursor, Screen, Snapshot, Style};
use unicode_width::UnicodeWidthChar;
use vte::{Params, Perform};

use crate::cell::{Cell, CellFlags, Wide};
use crate::grid::Grid;
use crate::sgr;

/// A terminal core: bytes in, grid mutations out.
pub struct Terminal {
    parser: vte::Parser,
    state: State,
}

impl Terminal {
    pub fn new(cols: u16, rows: u16) -> Terminal {
        Terminal {
            parser: vte::Parser::new(),
            state: State::new(cols, rows),
        }
    }

    /// Feeds bytes to the parser. Resumable: `vte` carries partial UTF-8 and partial escape
    /// sequences across calls, so a sequence split mid-stream is handled.
    pub fn write(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.state, bytes);
    }

    pub fn snapshot(&self) -> Snapshot {
        self.state.snapshot()
    }
}

struct State {
    grid: Grid,
    cursor_x: u16,
    cursor_y: u16,
    /// The style newly printed cells take.
    pen: Style,
    /// Flat index of the last printed cell, so a following zero-width codepoint knows which
    /// cluster it belongs to. Cleared by anything that moves the cursor.
    last_print: Option<usize>,
}

impl State {
    fn new(cols: u16, rows: u16) -> State {
        State {
            grid: Grid::new(cols, rows),
            cursor_x: 0,
            cursor_y: 0,
            pen: Style::DEFAULT,
            last_print: None,
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            cols: self.grid.cols(),
            rows: self.grid.rows(),
            // Slice 2 owns the alternate screen; there is only one buffer today.
            screen: Screen::Primary,
            cursor: Cursor {
                x: self.cursor_x,
                y: self.cursor_y,
                // Slice 2 owns the autowrap phantom state.
                pending_wrap: false,
                visible: true,
                style: self.pen,
            },
            grid: self.grid.to_rows(),
        }
    }

    /// Writes a printable character at the cursor and advances.
    ///
    /// Past the last column the character is dropped rather than wrapped: autowrap is slice
    /// 2, and faking a wrap here would produce a grid that looks right while the cursor and
    /// the row wrap flags do not, which is worse to debug than a clean omission.
    fn print_char(&mut self, c: char, width: u16) {
        if self.cursor_x >= self.grid.cols() || self.cursor_y >= self.grid.rows() {
            return;
        }
        if width == 2 && self.cursor_x + 1 >= self.grid.cols() {
            self.cursor_x = self.grid.cols();
            self.last_print = None;
            return;
        }

        let style_id = self.grid.intern_style(self.pen);
        let index = self.grid.index(self.cursor_x, self.cursor_y);
        self.grid.write(
            index,
            Cell {
                codepoint: c as u32,
                style_id,
                wide: if width == 2 { Wide::Wide } else { Wide::Narrow },
                flags: CellFlags::NONE,
            },
        );
        self.last_print = Some(index);

        if width == 2 {
            self.grid.write(
                index + 1,
                Cell {
                    codepoint: 0,
                    style_id,
                    wide: Wide::SpacerTail,
                    flags: CellFlags::NONE,
                },
            );
        }
        self.cursor_x += width;
    }

    fn move_to(&mut self, x: u16, y: u16) {
        self.cursor_x = x.min(self.grid.cols().saturating_sub(1));
        self.cursor_y = y.min(self.grid.rows().saturating_sub(1));
        self.last_print = None;
    }

    fn line_feed(&mut self) {
        // Slice 2 owns scrolling; at the bottom the cursor clamps instead.
        self.cursor_y = (self.cursor_y + 1).min(self.grid.rows().saturating_sub(1));
        self.last_print = None;
    }

    fn carriage_return(&mut self) {
        self.cursor_x = 0;
        self.last_print = None;
    }

    fn backspace(&mut self) {
        self.cursor_x = self.cursor_x.saturating_sub(1);
        self.last_print = None;
    }
}

impl Perform for State {
    fn print(&mut self, c: char) {
        // A zero-width codepoint continues the previous cell's grapheme cluster rather than
        // claiming a cell of its own -- ranked failure mode 2, a cell is not a codepoint.
        // This is the width heuristic, not full UAX #29 incremental segmentation, so ZWJ
        // emoji sequences still split. The corpus is where that shows up.
        let width = UnicodeWidthChar::width(c).unwrap_or(0);
        if width == 0 {
            if let Some(index) = self.last_print {
                self.grid.push_grapheme(index, c);
            }
            return;
        }
        self.print_char(c, if width >= 2 { 2 } else { 1 });
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x08 => self.backspace(),
            0x0a | 0x0b | 0x0c => self.line_feed(),
            0x0d => self.carriage_return(),
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        // Private and intermediate-bearing sequences (DEC modes, `CSI ? ... h`) belong to
        // slice 2. Acting on them half-way would be worse than not acting at all.
        if ignore || !intermediates.is_empty() {
            return;
        }

        match action {
            'm' => sgr::apply(&mut self.pen, params),
            'A' => self.move_to(self.cursor_x, self.cursor_y.saturating_sub(arg(params, 0))),
            'B' => self.move_to(self.cursor_x, self.cursor_y.saturating_add(arg(params, 0))),
            'C' => self.move_to(self.cursor_x.saturating_add(arg(params, 0)), self.cursor_y),
            'D' => self.move_to(self.cursor_x.saturating_sub(arg(params, 0)), self.cursor_y),
            'E' => self.move_to(0, self.cursor_y.saturating_add(arg(params, 0))),
            'F' => self.move_to(0, self.cursor_y.saturating_sub(arg(params, 0))),
            'G' | '`' => self.move_to(arg(params, 0) - 1, self.cursor_y),
            'd' => self.move_to(self.cursor_x, arg(params, 0) - 1),
            'H' | 'f' => self.move_to(arg(params, 1) - 1, arg(params, 0) - 1),
            _ => {}
        }
    }
}

/// Reads a CSI parameter, applying the VT rule that a missing or zero parameter means 1.
///
/// Never returning 0 is what makes `arg(..) - 1` safe at every call site above.
fn arg(params: &Params, index: usize) -> u16 {
    params
        .iter()
        .nth(index)
        .and_then(|values| values.first().copied())
        .filter(|value| *value != 0)
        .unwrap_or(1)
}
