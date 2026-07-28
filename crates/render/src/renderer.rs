//! Purpose: paint a frame into a canvas, repainting only what changed.
//! Public surface: `Renderer`.
//! Why this file: this is the consumer the `Run` seam exists for. Every column it paints
//!   comes from `Run::column_of`, never from adding an index to a start -- which today is a
//!   distinction without a difference and in slice 5.5 is the difference between Hebrew
//!   rendering correctly and rendering backwards. Drawing a row is a pure function of the
//!   frame and the row index, which is what makes "an incremental redraw equals a full one"
//!   a testable claim rather than a hope.
//! NOT responsible for: pixels (`canvas.rs`), glyph shapes (`atlas.rs`), colour rules
//!   (`color.rs`), or font choice (`font.rs`).
//! Test strategy: `tests/redraw.rs` compares incremental against full and proves the
//!   comparison can fail; `tests/vim.rs` runs the real editor through the real pty.

use ruuah_vt_frame::{Frame, PackedCell, cell_width};

use crate::atlas::{Atlas, GlyphKey};
use crate::canvas::Canvas;
use crate::color::{Palette, Rgba};
use crate::font::FontStack;

/// Paints frames into a pixel buffer.
pub struct Renderer {
    fonts: FontStack,
    atlas: Atlas,
    palette: Palette,
    canvas: Canvas,
    cols: u16,
    rows: u16,
    /// The last generation fully painted. Rows stamped above it are what is owed.
    drawn: u64,
}

impl Renderer {
    pub fn new(fonts: FontStack, cols: u16, rows: u16) -> Renderer {
        let cell = fonts.metrics();
        let canvas = Canvas::new(cell.width * u32::from(cols), cell.height * u32::from(rows));
        Renderer {
            fonts,
            atlas: Atlas::new(),
            palette: Palette::xterm(),
            canvas,
            cols,
            rows,
            drawn: 0,
        }
    }

    pub fn canvas(&self) -> &Canvas {
        &self.canvas
    }

    pub fn atlas(&self) -> &Atlas {
        &self.atlas
    }

    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    /// Repaints the rows this frame says are stale, and returns how many that was.
    ///
    /// Zero is the common and desirable answer: a terminal nobody is typing into should cost
    /// nothing per frame.
    pub fn draw(&mut self, frame: &Frame) -> usize {
        let stale: Vec<u16> = frame.stale_rows(self.drawn).collect();
        for y in &stale {
            self.draw_row(frame, *y);
        }
        self.drawn = frame.generation;
        stale.len()
    }

    /// The same as `draw`, but silently declines to repaint one stale row.
    ///
    /// This is a broken renderer on purpose. `tests/redraw.rs` compares an incremental redraw
    /// against a full one, and a comparison that has never been seen to fail is not evidence;
    /// this is what makes it fail. It has no legitimate caller.
    #[doc(hidden)]
    pub fn draw_skipping_for_testing(&mut self, frame: &Frame, skip: u16) -> usize {
        let stale: Vec<u16> = frame
            .stale_rows(self.drawn)
            .filter(|y| *y != skip)
            .collect();
        for y in &stale {
            self.draw_row(frame, *y);
        }
        self.drawn = frame.generation;
        stale.len()
    }

    /// Repaints every row regardless of damage. The reference the incremental path is
    /// measured against.
    pub fn draw_all(&mut self, frame: &Frame) -> usize {
        for y in 0..self.rows.min(frame.rows) {
            self.draw_row(frame, y);
        }
        self.drawn = frame.generation;
        usize::from(self.rows.min(frame.rows))
    }

    /// Paints one row. A pure function of `(frame, y)` -- nothing here reads or writes state
    /// that outlives the call except the glyph cache, which cannot change what is drawn.
    fn draw_row(&mut self, frame: &Frame, y: u16) {
        let cell = self.fonts.metrics();
        let top = i32::from(y) * cell.height as i32;

        // The row is cleared first, so a shorter line of text does not leave the tail of a
        // longer one behind it.
        self.canvas.fill(
            0,
            top,
            cell.width * u32::from(self.cols),
            cell.height,
            self.palette.default_background,
        );

        for run in frame.runs(y) {
            let drawn = self.palette.draw(&run.style);
            for (index, glyph_cell) in run.cells.iter().enumerate() {
                let column = run.column_of(index);
                let width = cell_width(*glyph_cell);
                if width == 0 {
                    continue;
                }
                let left = i32::from(column) * cell.width as i32;
                self.canvas.fill(
                    left,
                    top,
                    cell.width * u32::from(width),
                    cell.height,
                    drawn.background,
                );
                if drawn.ink {
                    self.draw_cell(*glyph_cell, left, top, drawn.foreground);
                }
                self.draw_decorations(&drawn, left, top, width);
            }
        }

        self.draw_cursor(frame, y);
    }

    fn draw_cell(&mut self, cell: PackedCell, left: i32, top: i32, color: Rgba) {
        if !cell.has_text() {
            return;
        }
        let metrics = self.fonts.metrics();
        let baseline = top + metrics.baseline;

        let mut scratch = [0u8; ruuah_vt_frame::CLUSTER_BYTES];
        let cluster = cell.cluster(&mut scratch);

        // One glyph per codepoint, all drawn at the same pen position. Correct for a single
        // character, and approximate for a cluster: a combining mark relies on its own
        // bearings to land in the right place instead of on GPOS mark attachment. Real
        // stacking is shaping, which is slice 5.5 -- this is why niqqud currently sit where
        // the font's default bearings put them rather than where the font says they belong.
        for c in cluster.chars() {
            let Some(resolved) = self.fonts.resolve(c) else {
                continue;
            };
            let key = GlyphKey {
                font: resolved.font,
                glyph: resolved.glyph,
            };
            let Some(glyph) = self.atlas.glyph(&self.fonts, key) else {
                continue;
            };

            let x = left + glyph.left;
            let y = baseline - glyph.top;
            let (width, height) = (glyph.width, glyph.height);
            // Copied out because the blend borrows the canvas mutably while the glyph is
            // borrowed from the atlas.
            let coverage = glyph.coverage.clone();
            self.canvas
                .blend_mask(x, y, width, height, &coverage, color);
        }
    }

    fn draw_decorations(&mut self, drawn: &crate::color::Drawn, left: i32, top: i32, width: u16) {
        let metrics = self.fonts.metrics();
        let span = metrics.width * u32::from(width);

        if drawn.underline {
            let y = top + (metrics.baseline + 1).min(metrics.height as i32 - 1);
            self.canvas.fill(left, y, span, 1, drawn.foreground);
        }
        if drawn.strikethrough {
            let y = top + metrics.baseline - (metrics.baseline / 3);
            self.canvas.fill(left, y, span, 1, drawn.foreground);
        }
    }

    /// Draws the cursor, if it is on this row.
    ///
    /// Painted as part of the row rather than after every frame, which is what keeps the
    /// redraw invariant true: the core already marks both the row the cursor left and the
    /// row it arrived at as damaged, so a moved cursor repaints both and leaves no ghost.
    fn draw_cursor(&mut self, frame: &Frame, y: u16) {
        if !frame.cursor.visible || frame.cursor.y != y || frame.cursor.x >= self.cols {
            return;
        }
        let metrics = self.fonts.metrics();
        let drawn = self.palette.draw(&frame.cursor.style);
        let left = i32::from(frame.cursor.x) * metrics.width as i32;
        let top = i32::from(y) * metrics.height as i32;

        self.canvas
            .fill(left, top, metrics.width, metrics.height, drawn.foreground);
        self.draw_cell(frame.cell(frame.cursor.x, y), left, top, drawn.background);
    }
}
