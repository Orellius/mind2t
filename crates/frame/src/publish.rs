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
        let bracketed_paste = terminal.bracketed_paste();
        let cluster = &mut self.cluster;

        // OSC 8: the core's link ids are table-wide; the frame carries only the links
        // VISIBLE this publish, densely remapped so 64 slots is a per-frame budget and
        // not a per-session one. Links beyond the budget publish as unlinked cells for
        // this frame -- a bounded, documented degradation, never a fault.
        let mut link_slots: Vec<u16> = Vec::new();
        fn frame_link(slots: &mut Vec<u16>, core_id: u16) -> u8 {
            if let Some(slot) = slots.iter().position(|&id| id == core_id) {
                return (slot + 1) as u8;
            }
            if slots.len() >= crate::seqlock::LINK_SLOTS {
                return 0;
            }
            slots.push(core_id);
            slots.len() as u8
        }

        let generation = self.writer.publish(cols, rows, |frame| {
            frame.styles(grid.styles().as_slice().iter().map(pack_style));

            for y in 0..rows {
                for x in 0..cols {
                    let index = grid.index(x, y);
                    let cell = grid.cell(index);
                    grid.cluster_into(index, cluster);
                    let mut packed = PackedCell::new(
                        cluster,
                        cell.style_id,
                        cell.wide.into(),
                        cell.flags.semantic(),
                    );
                    if let Some(core_id) = grid.link_id(index) {
                        packed = packed.with_link(frame_link(&mut link_slots, core_id));
                    }
                    frame.cell(x, y, packed);
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

            frame.modes(if bracketed_paste {
                crate::frame::Frame::MODE_BRACKETED_PASTE
            } else {
                0
            });

            frame.cursor(
                cursor.x,
                cursor.y,
                cursor.pending_wrap,
                cursor.visible,
                pack_style(&cursor.style),
            );

            frame.links(
                link_slots
                    .iter()
                    .map(|&id| terminal.link_uri(id).unwrap_or(""))
                    .collect::<Vec<_>>()
                    .into_iter(),
            );

            frame.placements(
                screen
                    .placements
                    .iter()
                    .map(|p| (p.image, p.col, p.row, p.cols, p.rows))
                    .collect::<Vec<_>>()
                    .into_iter(),
            );
        })?;

        terminal.clear_damage();
        Ok(generation)
    }
}
