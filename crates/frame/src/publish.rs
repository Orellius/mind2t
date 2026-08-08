//! Purpose: turn the core's state into one published frame.
//! Public surface: `Publisher`.
//! Why this file: it is the only place that knows both the core and the wire format, which
//!   is what keeps the core free of either. Taking the terminal by `&mut` is deliberate --
//!   publishing consumes the damage it reports, and pairing those two in one call is the
//!   difference between a renderer that repaints the right rows and one that repaints the
//!   same rows forever.
//! NOT responsible for: reading a pty (the pty crate), the handoff protocol (`seqlock.rs`).
//! Test strategy: `tests/publish.rs` writes bytes into a core, publishes, reads back, and
//!   compares text, styles, damage and cursor against the core's own snapshot;
//!   `tests/viewport.rs` proves a scrolled publish shows scrollback, against controls a
//!   publisher that ignores the offset was seen to fail.

use mind2t_vt_core::Terminal;

use crate::packed::{PackedCell, pack_style};
use crate::seqlock::{CapacityExceeded, FrameWriter};

/// Publishes frames from a terminal into a channel.
///
/// Owns the cluster scratch buffer so a frame costs no allocations at all after the first.
pub struct Publisher {
    writer: FrameWriter,
    cluster: String,
    /// The viewport offset of the previous publish, so returning to the live bottom is
    /// recognized as a whole-frame change -- per-row stamps say nothing about rows that
    /// moved because the WINDOW moved.
    last_offset: usize,
}

impl Publisher {
    pub fn new(writer: FrameWriter) -> Publisher {
        Publisher {
            writer,
            cluster: String::with_capacity(crate::packed::CLUSTER_BYTES),
            last_offset: 0,
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
        self.publish_scrolled(terminal, 0)
    }

    /// Publishes the view scrolled `offset` rows up into history: the frame's top rows come
    /// from scrollback, the remainder from the top of the active grid, bottom-aligned the
    /// way every terminal scrolls.
    ///
    /// The offset is a VIEW concern and never enters the core -- the terminal has no idea
    /// it is being looked at somewhere other than the bottom, which is what keeps the
    /// differential corpus untouched by this feature. An offset past the top of history is
    /// clamped. While scrolled (and on the publish that returns to the bottom), the whole
    /// frame is marked changed: per-row damage stamps describe active-grid rows, and under
    /// a moved window they name the wrong screen positions.
    pub fn publish_scrolled(
        &mut self,
        terminal: &mut Terminal,
        offset: u32,
    ) -> Result<u64, CapacityExceeded> {
        let screen = terminal.screen();
        let grid = &screen.grid;
        let (cols, rows) = (grid.cols(), grid.rows());
        let offset = (offset as usize).min(screen.history.len());
        // Rows of the frame showing history; the rest show the active grid's top.
        let history_visible = offset.min(usize::from(rows));
        let history_rows = screen.history.rows_from_end(offset, history_visible);
        let window_moved = offset != self.last_offset;
        let wholly_damaged = terminal.is_wholly_damaged() || offset > 0 || window_moved;
        let cursor = terminal.cursor();
        let bracketed_paste = terminal.bracketed_paste();
        let synchronized_output = terminal.synchronized_output();
        // The discriminants ARE the wire values (declaration order both sides); the
        // frame accessors decode by the same table.
        let mouse_event = terminal.mouse_event() as u8;
        let mouse_format = terminal.mouse_format() as u8;
        let mouse_alternate_scroll = terminal.mouse_alternate_scroll();
        let alternate_screen = terminal.on_alternate_screen();
        let cursor_keys = terminal.cursor_keys();
        let keypad_keys = terminal.keypad_keys();
        let ignore_keypad_with_numlock = terminal.ignore_keypad_with_numlock();
        let alt_esc_prefix = terminal.alt_esc_prefix();
        let modify_other_keys_2 = terminal.modify_other_keys_2();
        let kitty_key_flags = terminal.kitty_key_flags();
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

        // History styles arrive by value -- their rows may predate the grid's compacted
        // table -- so the frame's table is the grid's interned entries with the visible
        // history styles appended, deduplicated. Past the channel's ceiling a history
        // cell publishes the default style: a bounded, visible degradation, never a
        // silently dropped table entry (Publish::styles truncates without telling).
        let mut styles: Vec<[u64; 2]> = grid.styles().as_slice().iter().map(pack_style).collect();
        let style_cap = self.writer.style_capacity();
        fn intern(styles: &mut Vec<[u64; 2]>, cap: usize, packed: [u64; 2]) -> u16 {
            if let Some(index) = styles.iter().position(|entry| *entry == packed) {
                return index as u16;
            }
            if styles.len() >= cap {
                return 0;
            }
            styles.push(packed);
            (styles.len() - 1) as u16
        }

        let generation = self.writer.publish(cols, rows, |frame| {
            for y in 0..rows {
                if usize::from(y) < history_visible {
                    // This frame row shows scrollback. Rows come back full-width from the
                    // page store; OSC 8 link ids do not survive the page readout, so
                    // scrolled-back text is not clickable (a named v1 boundary).
                    let Some(row) = history_rows.get(usize::from(y)) else {
                        continue;
                    };
                    for x in 0..cols {
                        let packed = match row.cells.get(usize::from(x)) {
                            Some(cell) => {
                                let id = intern(&mut styles, style_cap, pack_style(&cell.style));
                                PackedCell::new(&cell.text, id, cell.wide, cell.semantic)
                            }
                            None => PackedCell::BLANK,
                        };
                        frame.cell(x, y, packed);
                    }
                    frame.row_flags(y, row.wrap, row.wrap_continuation);
                    frame.row_changed(y);
                    continue;
                }

                // This frame row shows the active grid, shifted down by the scrolled rows.
                let source_y = y - history_visible as u16;
                for x in 0..cols {
                    let index = grid.index(x, source_y);
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

                let meta = grid.row_meta(source_y);
                frame.row_flags(y, meta.wrap, meta.wrap_continuation);

                if wholly_damaged || grid.dirty_rows().get(usize::from(source_y)) == Some(&true) {
                    frame.row_changed(y);
                }
            }

            // After the cell loop: interning during it may have appended entries.
            frame.styles(styles.iter().copied());

            if wholly_damaged {
                frame.whole_frame_changed();
            }

            let mut modes = 0;
            if bracketed_paste {
                modes |= crate::frame::Frame::MODE_BRACKETED_PASTE;
            }
            if synchronized_output {
                modes |= crate::frame::Frame::MODE_SYNCHRONIZED_OUTPUT;
            }
            modes |= (mouse_event as u64) << crate::frame::Frame::MODE_MOUSE_EVENT_SHIFT;
            modes |= (mouse_format as u64) << crate::frame::Frame::MODE_MOUSE_FORMAT_SHIFT;
            if mouse_alternate_scroll {
                modes |= crate::frame::Frame::MODE_MOUSE_ALTERNATE_SCROLL;
            }
            if alternate_screen {
                modes |= crate::frame::Frame::MODE_ALTERNATE_SCREEN;
            }
            if cursor_keys {
                modes |= crate::frame::Frame::MODE_CURSOR_KEYS;
            }
            if keypad_keys {
                modes |= crate::frame::Frame::MODE_KEYPAD_KEYS;
            }
            if ignore_keypad_with_numlock {
                modes |= crate::frame::Frame::MODE_IGNORE_KEYPAD_WITH_NUMLOCK;
            }
            if alt_esc_prefix {
                modes |= crate::frame::Frame::MODE_ALT_ESC_PREFIX;
            }
            if modify_other_keys_2 {
                modes |= crate::frame::Frame::MODE_MODIFY_OTHER_KEYS_2;
            }
            modes |= u64::from(kitty_key_flags) << crate::frame::Frame::MODE_KITTY_KEY_SHIFT;
            frame.modes(modes);

            frame.viewport(offset as u32);

            // The cursor lives in the active grid, so a scrolled view shifts it down with
            // its rows; once its cell leaves the bottom of the window it is published
            // invisible rather than drawn on the wrong row.
            let cursor_y = usize::from(cursor.y) + history_visible;
            frame.cursor(
                cursor.x,
                cursor_y.min(usize::from(rows)) as u16,
                cursor.pending_wrap,
                cursor.visible && cursor_y < usize::from(rows),
                pack_style(&cursor.style),
            );

            frame.links(
                link_slots
                    .iter()
                    .map(|&id| terminal.link_uri(id).unwrap_or(""))
                    .collect::<Vec<_>>()
                    .into_iter(),
            );

            // Placements anchor to active-grid rows and ride the same shift; both
            // backends clip per pixel at every canvas edge.
            // Sorted into DRAW ORDER here rather than renderer-side, because the host
            // resolves image pixels positionally against this list: a renderer that
            // re-sorted would pair every placement with the wrong image's bytes.
            //
            // The key mirrors the oracle's (`renderer/image.zig`): z, then image id, and
            // a STABLE sort keeps placement order as the final tiebreak, so two
            // placements of one image at one z draw in the order the child made them.
            let mut placements = screen
                .placements
                .iter()
                .map(|p| {
                    (
                        p.image,
                        p.col,
                        p.row
                            .saturating_add(i16::try_from(history_visible).unwrap_or(i16::MAX)),
                        p.cols,
                        p.rows,
                        p.z,
                    )
                })
                .collect::<Vec<_>>();
            placements.sort_by_key(|&(image, _, _, _, _, z)| (z, image));
            frame.placements(placements.into_iter());

            // Virtual placements carry no position, so they are published whole and
            // unsorted: the placeholder cells decide where and in what order anything is
            // drawn from them.
            frame.virtuals(terminal.virtuals().iter().copied());
        })?;

        self.last_offset = offset;
        terminal.clear_damage();
        Ok(generation)
    }
}
