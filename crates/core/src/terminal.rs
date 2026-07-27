//! Purpose: the smallest thing that can stand on the other side of the oracle.
//! Public surface: `Terminal::new`, `Terminal::write`, `Terminal::snapshot`.
//! Why this file: slice 0 is a gate on the harness, not on the terminal. A stub that
//!   handles plain text and nothing else makes the harness prove both directions -- it
//!   agrees with Ghostty on plain output and disagrees everywhere else -- which a stub
//!   that did nothing at all could not do.
//! NOT responsible for: escape sequences, wrapping, scrolling, scrollback, styles,
//!   or wide characters. Every one of those is a slice with its own gate. Deliberate
//!   omissions, not oversights: the corpus expects the resulting diffs.
//! Test strategy: none of its own. It is measured only against the oracle, by the corpus.

use ruuah_vt_snapshot::{Cell, Cursor, Row, Screen, Snapshot, Style};

/// A stub terminal handling printable ASCII, CR, LF and BS.
///
/// The pure-state-machine rule from the plan already holds here: bytes in, grid out,
/// no I/O, no clock, no allocation beyond the grid itself.
pub struct Terminal {
    cols: u16,
    rows: u16,
    cursor_x: u16,
    cursor_y: u16,
    grid: Vec<Vec<Cell>>,
}

impl Terminal {
    pub fn new(cols: u16, rows: u16) -> Terminal {
        Terminal {
            cols,
            rows,
            cursor_x: 0,
            cursor_y: 0,
            grid: vec![vec![Cell::blank(); cols as usize]; rows as usize],
        }
    }

    /// Consumes bytes. Anything that is not printable ASCII, CR, LF or BS is dropped
    /// rather than interpreted, so an escape sequence lands as its literal characters.
    /// That is the honest behaviour of a core with no parser, and the harness reports it.
    pub fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            match byte {
                b'\r' => self.cursor_x = 0,
                b'\n' => self.cursor_y = (self.cursor_y + 1).min(self.rows.saturating_sub(1)),
                0x08 => self.cursor_x = self.cursor_x.saturating_sub(1),
                0x20..=0x7e => self.print(byte as char),
                _ => {}
            }
        }
    }

    fn print(&mut self, ch: char) {
        if self.cursor_x >= self.cols || self.cursor_y >= self.rows {
            return;
        }
        let cell = &mut self.grid[self.cursor_y as usize][self.cursor_x as usize];
        cell.text = ch.to_string();
        self.cursor_x += 1;
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            cols: self.cols,
            rows: self.rows,
            screen: Screen::Primary,
            cursor: Cursor {
                x: self.cursor_x,
                y: self.cursor_y,
                pending_wrap: false,
                visible: true,
                style: Style::DEFAULT,
            },
            grid: self
                .grid
                .iter()
                .map(|cells| Row {
                    wrap: false,
                    wrap_continuation: false,
                    cells: cells.clone(),
                })
                .collect(),
        }
    }
}
