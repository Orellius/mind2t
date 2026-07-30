//! Purpose: hand a whole frame from the parse thread to the renderer without either one
//!   waiting on the other.
//! Public surface: `channel`, `FrameWriter`, `FrameReader`, `Publish`, `ReadOutcome`,
//!   `CapacityExceeded`.
//! Why this file: a mutex here makes the two threads take turns, and the thread that loses
//!   is whichever one is not already running -- Ghostty hit exactly this and had to add a
//!   demand-and-handoff protocol on top of its render-state mutex (`../ruuah/src/renderer/
//!   State.zig`, `lockDemand`), because under sustained pty output the parse loop relocks
//!   faster than a sleeping renderer can be scheduled. A seqlock sidesteps the fairness
//!   question instead of solving it: the writer never blocks, and a reader that catches a
//!   publish in progress discards that frame and comes back.
//! NOT responsible for: bit layout (`packed.rs`), run building (`frame.rs`), or where the
//!   bytes came from (`publish.rs`, and the pty crate above it).
//! Test strategy: `tests/tearing.rs` runs both threads flat out against a frame carrying a
//!   self-consistency invariant, and proves the harness can fail by running the same load
//!   through a reader that ignores the counter.
//!
//! There is no `unsafe` here and no volatile trickery. The payload is `AtomicU64` read and
//! written `Relaxed`, which is defined behaviour under concurrent access -- the standard
//! library defines a data race as requiring a *non-atomic* access, so racing relaxed loads
//! are merely unordered, never undefined. The counter's `Acquire`/`Release` pair is what
//! turns a set of individually-atomic words into one consistent frame.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering, fence};

use crate::frame::Frame;
use crate::packed::PackedCell;

/// The geometry asked for does not fit the buffer the channel was built with.
///
/// A value rather than a panic: growing the buffer means replacing it, which is a decision
/// for the layer that owns both ends, not for the publish path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapacityExceeded {
    pub wanted: (u16, u16),
    pub capacity: (u16, u16),
}

/// What a read attempt produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOutcome {
    /// A complete, consistent frame was copied out. Carries its generation.
    Fresh(u64),
    /// Nothing has been published since the frame was last filled.
    Unchanged,
    /// A publish was in progress, or one landed mid-read. The frame must not be drawn.
    ///
    /// It is left either untouched (the publish was already in flight when the read started)
    /// or overwritten and explicitly invalidated (one landed mid-copy). Which of the two is
    /// deliberately not distinguished, because the only correct response to either is the
    /// same: come back, and draw the last good frame in the meantime. `Frame::is_valid` is
    /// what tells a careless caller which it is holding.
    Skipped,
}

struct Shared {
    /// Odd while a publish is in flight. The whole protocol is this one number.
    generation: AtomicU64,
    geometry: AtomicU64,
    /// Generation of the last whole-frame invalidation, so a renderer that skipped frames
    /// can still tell that per-row information stopped meaning anything at some point.
    full: AtomicU64,
    /// Terminal mode bits (`Frame::MODE_*`). One word for all of them, so a mode the
    /// renderer never looks at costs the channel nothing.
    modes: AtomicU64,
    /// Rows the published view is scrolled up into history. Zero is the live bottom.
    viewport: AtomicU64,
    cursor: [AtomicU64; 3],
    cells: Box<[AtomicU64]>,
    row_generation: Box<[AtomicU64]>,
    row_flags: Box<[AtomicU64]>,
    styles: Box<[AtomicU64]>,
    style_len: AtomicU64,
    /// OSC 8 URIs for the links visible in this frame, densely remapped per publish.
    /// Fixed slots of plain words for the same reason as everything else here: a torn
    /// read is garbage text wearing an invalid generation, never a fault.
    links: Box<[AtomicU64]>,
    link_len: AtomicU64,
    /// Kitty graphics placements visible this frame: image id + cell anchor + span.
    /// Two words each; the pixels themselves never ride the seqlock (they live in the
    /// shared image store the pump maintains).
    placements: Box<[AtomicU64]>,
    placement_len: AtomicU64,
    capacity: (u16, u16),
    style_capacity: usize,
}

/// Words per cell in the shared buffer. Two of UTF-8, one of attributes.
pub(crate) const CELL_WORDS: usize = 3;
const STYLE_WORDS: usize = 2;
/// Link slots per frame and words per slot (word 0 = byte length, then the URI bytes).
/// 64 visible links x 248 bytes is 16KB of fixed payload -- small next to the cells.
pub const LINK_SLOTS: usize = 64;
/// Visible kitty placements per frame; past this the newest are dropped for the frame.
pub const PLACEMENT_SLOTS: usize = 16;
const PLACEMENT_WORDS: usize = 2;
const LINK_WORDS: usize = 32;
/// The longest URI a slot can carry; longer ones are truncated at a char boundary by
/// the publisher (a click target, not an archive).
pub const LINK_URI_BYTES: usize = (LINK_WORDS - 1) * 8;

fn zeroed(len: usize) -> Box<[AtomicU64]> {
    (0..len).map(|_| AtomicU64::new(0)).collect()
}

/// Builds a writer and a reader over one shared buffer sized for `cols` x `rows`.
///
/// `style_capacity` bounds the interned style table. The core compacts its own table at
/// `max(cols * rows, 256)` and can hold one more than that immediately after, so that plus
/// one is the exact ceiling rather than a guess.
pub fn channel(cols: u16, rows: u16) -> (FrameWriter, FrameReader) {
    let cells = usize::from(cols) * usize::from(rows);
    let style_capacity = cells.max(256) + 1;

    let shared = Arc::new(Shared {
        generation: AtomicU64::new(0),
        geometry: AtomicU64::new(0),
        full: AtomicU64::new(0),
        modes: AtomicU64::new(0),
        viewport: AtomicU64::new(0),
        cursor: [const { AtomicU64::new(0) }; 3],
        cells: zeroed(cells * CELL_WORDS),
        row_generation: zeroed(usize::from(rows)),
        row_flags: zeroed(usize::from(rows)),
        styles: zeroed(style_capacity * STYLE_WORDS),
        style_len: AtomicU64::new(0),
        links: zeroed(LINK_SLOTS * LINK_WORDS),
        link_len: AtomicU64::new(0),
        placements: zeroed(PLACEMENT_SLOTS * PLACEMENT_WORDS),
        placement_len: AtomicU64::new(0),
        capacity: (cols, rows),
        style_capacity,
    });

    (
        FrameWriter {
            shared: Arc::clone(&shared),
        },
        FrameReader { shared },
    )
}

/// The publishing end. Not `Clone`: the seqlock protocol admits exactly one writer, and that
/// is worth enforcing in the type rather than in a comment.
pub struct FrameWriter {
    shared: Arc<Shared>,
}

impl FrameWriter {
    pub fn capacity(&self) -> (u16, u16) {
        self.shared.capacity
    }

    /// How many style entries the channel can carry. The publisher caps its table here:
    /// entries past it would be silently dropped by `Publish::styles`, and a scrolled view
    /// appending history styles is the one caller that can actually reach the ceiling.
    pub fn style_capacity(&self) -> usize {
        self.shared.style_capacity
    }

    /// Publishes one frame. The closure fills it; the counter around it is what makes the
    /// result all-or-nothing for the reader.
    ///
    /// Geometry is checked before the counter moves, so a rejected publish leaves the
    /// previous frame intact and readable rather than half-overwritten.
    pub fn publish(
        &mut self,
        cols: u16,
        rows: u16,
        fill: impl FnOnce(&mut Publish<'_>),
    ) -> Result<u64, CapacityExceeded> {
        let (cap_cols, cap_rows) = self.shared.capacity;
        if cols > cap_cols || rows > cap_rows {
            return Err(CapacityExceeded {
                wanted: (cols, rows),
                capacity: self.shared.capacity,
            });
        }

        let shared = &*self.shared;
        let start = shared.generation.load(Ordering::Relaxed);
        // `| 1` rather than `+ 1`: if the previous publish panicked out of its fill closure,
        // the counter is still odd, and a blind bump would land it EVEN during this fill --
        // torn reads wearing valid generations -- and ODD at completion, inverting the
        // protocol permanently (finding 17). This way a normal publish is unchanged and a
        // recovery publish re-synchronizes the parity instead.
        let inflight = start | 1;
        let generation = inflight + 1;

        // Odd: a reader arriving now sees a publish in flight and leaves. The release fence
        // keeps the payload stores below from being observed before this marker.
        shared.generation.store(inflight, Ordering::Relaxed);
        fence(Ordering::Release);

        shared
            .geometry
            .store(u64::from(cols) | (u64::from(rows) << 16), Ordering::Relaxed);

        let mut publish = Publish {
            shared,
            generation,
            cols,
        };
        fill(&mut publish);

        shared.generation.store(generation, Ordering::Release);
        Ok(generation)
    }
}

/// The frame being filled, handed to the publish closure.
pub struct Publish<'a> {
    shared: &'a Shared,
    generation: u64,
    cols: u16,
}

impl Publish<'_> {
    /// The generation this frame will carry. Rows stamped with it are the ones that changed.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn cell(&mut self, x: u16, y: u16, cell: PackedCell) {
        let base = (usize::from(y) * usize::from(self.cols) + usize::from(x)) * CELL_WORDS;
        let Some(words) = self.shared.cells.get(base..base + CELL_WORDS) else {
            return;
        };
        words[0].store(cell.text[0], Ordering::Relaxed);
        words[1].store(cell.text[1], Ordering::Relaxed);
        words[2].store(cell.attrs, Ordering::Relaxed);
    }

    /// Stamps a row as changed in this frame.
    ///
    /// A stamp rather than a flag, because the reader is allowed to miss frames. A renderer
    /// that last drew generation 40 repaints every row stamped above 40, whether that row
    /// changed in frame 41 or 47 -- which a boolean cleared at publish time could not express.
    pub fn row_changed(&mut self, y: u16) {
        if let Some(slot) = self.shared.row_generation.get(usize::from(y)) {
            slot.store(self.generation, Ordering::Relaxed);
        }
    }

    pub fn row_flags(&mut self, y: u16, wrap: bool, wrap_continuation: bool) {
        if let Some(slot) = self.shared.row_flags.get(usize::from(y)) {
            slot.store(
                u64::from(wrap) | (u64::from(wrap_continuation) << 1),
                Ordering::Relaxed,
            );
        }
    }

    /// Marks the whole frame stale: per-row stamps carry no information across this point.
    pub fn whole_frame_changed(&mut self) {
        self.shared.full.store(self.generation, Ordering::Relaxed);
    }

    /// Publishes the terminal mode bits (`Frame::MODE_*`), one word for all of them.
    pub fn modes(&mut self, modes: u64) {
        self.shared.modes.store(modes, Ordering::Relaxed);
    }

    /// Publishes how many rows this view is scrolled up into history. Written by every
    /// publish -- the word persists across publishes, so an unwritten value would leak the
    /// previous frame's scroll position into this one.
    pub fn viewport(&mut self, offset: u32) {
        self.shared.viewport.store(u64::from(offset), Ordering::Relaxed);
    }

    pub fn cursor(&mut self, x: u16, y: u16, pending_wrap: bool, visible: bool, style: [u64; 2]) {
        self.shared.cursor[0].store(
            u64::from(x)
                | (u64::from(y) << 16)
                | (u64::from(pending_wrap) << 32)
                | (u64::from(visible) << 33),
            Ordering::Relaxed,
        );
        self.shared.cursor[1].store(style[0], Ordering::Relaxed);
        self.shared.cursor[2].store(style[1], Ordering::Relaxed);
    }

    /// Writes the frame's link table: one URI per slot, in the dense order the cells'
    /// link ids point at (slot 0 is id 1). Entries past `LINK_SLOTS` are dropped -- the
    /// publisher already refused to hand out ids for them.
    pub fn links<'uri>(&mut self, table: impl ExactSizeIterator<Item = &'uri str>) {
        let len = table.len().min(LINK_SLOTS);
        self.shared.link_len.store(len as u64, Ordering::Relaxed);
        for (slot, uri) in table.take(len).enumerate() {
            let base = slot * LINK_WORDS;
            let bytes = uri.as_bytes();
            let take = bytes.len().min(LINK_URI_BYTES);
            self.shared.links[base].store(take as u64, Ordering::Relaxed);
            for (offset, chunk) in bytes[..take].chunks(8).enumerate() {
                let mut raw = [0u8; 8];
                raw[..chunk.len()].copy_from_slice(chunk);
                self.shared.links[base + 1 + offset]
                    .store(u64::from_le_bytes(raw), Ordering::Relaxed);
            }
        }
    }

    /// Writes the frame's kitty placements: (image, col, row, cols, rows) per slot.
    pub fn placements(
        &mut self,
        table: impl ExactSizeIterator<Item = (u32, u16, i16, u16, u16)>,
    ) {
        let len = table.len().min(PLACEMENT_SLOTS);
        self.shared.placement_len.store(len as u64, Ordering::Relaxed);
        for (slot, (image, col, row, cols, rows)) in table.take(len).enumerate() {
            let base = slot * PLACEMENT_WORDS;
            self.shared.placements[base].store(
                u64::from(image) | (u64::from(col) << 32) | (u64::from(row as u16) << 48),
                Ordering::Relaxed,
            );
            self.shared.placements[base + 1]
                .store(u64::from(cols) | (u64::from(rows) << 16), Ordering::Relaxed);
        }
    }

    /// Writes the interned style table. Entries past the buffer are dropped, which the
    /// channel's sizing makes unreachable for a table the core produced.
    pub fn styles(&mut self, table: impl ExactSizeIterator<Item = [u64; 2]>) {
        let len = table.len().min(self.shared.style_capacity);
        self.shared.style_len.store(len as u64, Ordering::Relaxed);
        for (index, words) in table.take(len).enumerate() {
            let base = index * STYLE_WORDS;
            self.shared.styles[base].store(words[0], Ordering::Relaxed);
            self.shared.styles[base + 1].store(words[1], Ordering::Relaxed);
        }
    }
}

/// The consuming end. Cloneable: any number of readers can watch one writer.
#[derive(Clone)]
pub struct FrameReader {
    shared: Arc<Shared>,
}

impl FrameReader {
    /// Copies the newest complete frame into `frame`.
    ///
    /// Single attempt by design. A caller that catches the writer mid-publish is told so and
    /// decides for itself whether to retry now or wait for the next vsync; retrying inside
    /// here would put the renderer back into a spin the writer can win indefinitely.
    pub fn read_into(&self, frame: &mut Frame) -> ReadOutcome {
        let shared = &*self.shared;

        let before = shared.generation.load(Ordering::Acquire);
        if before & 1 != 0 {
            return ReadOutcome::Skipped;
        }
        if before == frame.generation && before != 0 {
            return ReadOutcome::Unchanged;
        }

        self.copy_payload(frame);

        // Everything above must be observed before the counter is re-read, or a frame
        // overwritten during the copy could still look untouched.
        fence(Ordering::Acquire);
        if shared.generation.load(Ordering::Relaxed) != before {
            // The copy already wrote into `frame`, and undoing it would need the shadow buffer
            // this design exists to avoid. So the frame is marked as belonging to no publish
            // rather than left claiming the one it held before the call: without this it holds
            // a mixture of two publishes while reporting a third's generation, and a caller
            // that ignores the outcome draws it without anything looking wrong.
            frame.generation = 0;
            return ReadOutcome::Skipped;
        }

        frame.generation = before;
        ReadOutcome::Fresh(before)
    }

    /// The same copy with the counter ignored, so a test can demonstrate that the invariant
    /// it checks is one a broken reader would actually violate.
    ///
    /// This exists because a concurrency test that never fails proves nothing. It is not a
    /// fallback and has no legitimate caller outside `tests/tearing.rs`.
    #[doc(hidden)]
    pub fn read_into_unsynchronized_for_testing(&self, frame: &mut Frame) {
        self.copy_payload(frame);
        frame.generation = self.shared.generation.load(Ordering::Relaxed);
    }

    fn copy_payload(&self, frame: &mut Frame) {
        let shared = &*self.shared;

        let geometry = shared.geometry.load(Ordering::Relaxed);
        let cols = geometry as u16;
        let rows = (geometry >> 16) as u16;
        frame.resize(cols, rows);
        frame.modes = shared.modes.load(Ordering::Relaxed);
        frame.viewport = shared.viewport.load(Ordering::Relaxed) as u32;

        let cells = usize::from(cols) * usize::from(rows);
        for index in 0..cells {
            let base = index * CELL_WORDS;
            frame.cells[index] = PackedCell {
                text: [
                    shared.cells[base].load(Ordering::Relaxed),
                    shared.cells[base + 1].load(Ordering::Relaxed),
                ],
                attrs: shared.cells[base + 2].load(Ordering::Relaxed),
            };
        }

        for y in 0..usize::from(rows) {
            frame.row_generation[y] = shared.row_generation[y].load(Ordering::Relaxed);
            let flags = shared.row_flags[y].load(Ordering::Relaxed);
            frame.row_flags[y] = (flags & 1 != 0, flags & 2 != 0);
        }

        let style_len =
            (shared.style_len.load(Ordering::Relaxed) as usize).min(shared.style_capacity);
        frame.styles.clear();
        for index in 0..style_len {
            let base = index * STYLE_WORDS;
            frame.styles.push([
                shared.styles[base].load(Ordering::Relaxed),
                shared.styles[base + 1].load(Ordering::Relaxed),
            ]);
        }

        let link_len = (shared.link_len.load(Ordering::Relaxed) as usize).min(LINK_SLOTS);
        frame.links.clear();
        for slot in 0..link_len {
            let base = slot * LINK_WORDS;
            let len = (shared.links[base].load(Ordering::Relaxed) as usize).min(LINK_URI_BYTES);
            let mut bytes = Vec::with_capacity(len);
            for offset in 0..len.div_ceil(8) {
                let word = shared.links[base + 1 + offset].load(Ordering::Relaxed);
                bytes.extend_from_slice(&word.to_le_bytes());
            }
            bytes.truncate(len);
            // Lossy on purpose: a torn read is garbage text wearing an invalid
            // generation -- the frame gets discarded, and this must never fault first.
            frame
                .links
                .push(String::from_utf8_lossy(&bytes).into_owned());
        }

        let placement_len =
            (shared.placement_len.load(Ordering::Relaxed) as usize).min(PLACEMENT_SLOTS);
        frame.placements.clear();
        for slot in 0..placement_len {
            let base = slot * PLACEMENT_WORDS;
            let a = shared.placements[base].load(Ordering::Relaxed);
            let b = shared.placements[base + 1].load(Ordering::Relaxed);
            frame.placements.push(crate::frame::FramePlacement {
                image: a as u32,
                col: (a >> 32) as u16,
                row: (a >> 48) as u16 as i16,
                cols: b as u16,
                rows: (b >> 16) as u16,
            });
        }

        frame.full_generation = shared.full.load(Ordering::Relaxed);
        let cursor = shared.cursor[0].load(Ordering::Relaxed);
        frame.cursor = crate::frame::FrameCursor {
            x: cursor as u16,
            y: (cursor >> 16) as u16,
            pending_wrap: cursor & (1 << 32) != 0,
            visible: cursor & (1 << 33) != 0,
            style: crate::packed::unpack_style([
                shared.cursor[1].load(Ordering::Relaxed),
                shared.cursor[2].load(Ordering::Relaxed),
            ]),
        };
    }
}
