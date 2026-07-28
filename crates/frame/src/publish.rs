//! Purpose: turn the core's state into one published frame.
//! Public surface: `Publisher`.
//! Why this file: it is the only place that knows both the core and the wire format, which
//!   is what keeps the core free of either. Taking the terminal by `&mut` is deliberate --
//!   publishing consumes the damage it reports, and pairing those two in one call is the
//!   difference between a renderer that repaints the right rows and one that repaints the
//!   same rows forever.
//! NOT responsible for: reading a pty (the pty crate), the handoff protocol (`seqlock.rs`).
//! Test strategy: `tests/publish.rs` writes bytes into a core, publishes, reads back, and
//!   compares text, styles, damage and cursor against the core's own snapshot.

use ruuah_vt_core::Terminal;

use crate::packed::{PackedCell, pack_style};
use crate::seqlock::{CapacityExceeded, FrameWriter};

/// Publishes frames from a terminal into a channel.
///
/// Owns the cluster scratch buffer so a frame costs no allocations at all after the first.
pub struct Publisher {
    writer: FrameWriter,
    cluster: String,
}

impl Publisher {
    pub fn new(writer: FrameWriter) -> Publisher {
        Publisher {
            writer,
            cluster: String::with_capacity(crate::packed::CLUSTER_BYTES),
        }
    }

    pub fn capacity(&self) -> (u16, u16) {
        self.writer.capacity()
    }

    /// Publishes the terminal's current state and clears the damage it just reported.
    ///
    /// A failed publish leaves the damage intact, so the rows that could not be sent are
    /// still owed and go out with the next successful frame.
    pub fn publish(&mut self, terminal: &mut Terminal) -> Result<u64, CapacityExceeded> {
        let screen = terminal.screen();
        let grid = &screen.grid;
        let (cols, rows) = (grid.cols(), grid.rows());
        let wholly_damaged = terminal.is_wholly_damaged();
        let cursor = terminal.cursor();
        let cluster = &mut self.cluster;

        let generation = self.writer.publish(cols, rows, |frame| {
            frame.styles(grid.styles().as_slice().iter().map(pack_style));

            for y in 0..rows {
                for x in 0..cols {
                    let index = grid.index(x, y);
                    let cell = grid.cell(index);
                    grid.cluster_into(index, cluster);
                    frame.cell(
                        x,
                        y,
                        PackedCell::new(cluster, cell.style_id, cell.wide.into()),
                    );
                }

                let meta = grid.row_meta(y);
                frame.row_flags(y, meta.wrap, meta.wrap_continuation);

                if wholly_damaged || grid.dirty_rows().get(usize::from(y)) == Some(&true) {
                    frame.row_changed(y);
                }
            }

            if wholly_damaged {
                frame.whole_frame_changed();
            }

            frame.cursor(
                cursor.x,
                cursor.y,
                cursor.pending_wrap,
                cursor.visible,
                pack_style(&cursor.style),
            );
        })?;

        terminal.clear_damage();
        Ok(generation)
    }
}
