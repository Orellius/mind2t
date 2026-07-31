//! Purpose: the reader's own copy of a frame, and the only shape a renderer is allowed to
//!   draw from -- runs, never a raw cell array.
//! Public surface: `Frame`, `FrameCursor`, `Run`, `Direction`, `cell_width`.
//! Why this file: this is the seam that keeps bidi out of the renderer, and slice 5.5 proved
//!   it holds. Reordering turned on by changing `runs` and `bidi.rs` alone -- the renderer
//!   was not touched, because it never assumed a direction and asks `Run::column_of` for
//!   every column it paints. A renderer that had added an index to a run's start instead
//!   would have compiled, passed every test before 5.5, and drawn Hebrew backwards after it.
//! NOT responsible for: the thread handoff (`seqlock.rs`), bit layout (`packed.rs`), or the
//!   reordering itself (`bidi.rs`).
//! Test strategy: run splitting is unit-tested below against hand-built rows; the frame's
//!   agreement with the core is tested end to end in `tests/publish.rs`; the layout is
//!   measured against 91,707 Unicode cases in `tests/bidi_conformance.rs`.

use ruuah_vt_snapshot::{Semantic, Style, Wide};

use crate::bidi::{BaseDirection, visual_spans};
use crate::packed::{PackedCell, unpack_style};

/// Which way a run advances from its starting column.
///
/// One variant is produced today. The other is what slice 5.5 turns on, and every renderer
/// written against this enum keeps working when it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    LeftToRight,
    RightToLeft,
}

/// A span of cells a renderer can draw as a unit: one style, one direction, contiguous
/// columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run<'a> {
    /// Leftmost column the run occupies, in screen coordinates.
    pub start: u16,
    /// Column of the run's first cell in the row's LOGICAL order, which is the coordinate
    /// space the program addresses and the cursor is reported in. Equal to `start` only in a
    /// left-to-right row.
    pub logical_start: u16,
    pub direction: Direction,
    pub style: Style,
    /// The run's cells in logical order. `column_of` maps an index to its screen column.
    pub cells: &'a [PackedCell],
}

impl Run<'_> {
    /// Where the `index`th cell of this run is drawn.
    ///
    /// The whole point of the seam. A renderer that adds `index` to `start` itself works
    /// until the first right-to-left run and then draws every Hebrew line backwards.
    pub fn column_of(&self, index: usize) -> u16 {
        let offset = self.width_before(index);
        match self.direction {
            Direction::LeftToRight => self.start + offset,
            Direction::RightToLeft => {
                self.start + self.width().saturating_sub(offset + self.cell_width(index))
            }
        }
    }

    /// Columns the run occupies in total.
    pub fn width(&self) -> u16 {
        self.width_before(self.cells.len())
    }

    fn width_before(&self, index: usize) -> u16 {
        self.cells[..index.min(self.cells.len())]
            .iter()
            .map(|cell| cell_width(*cell))
            .sum()
    }

    fn cell_width(&self, index: usize) -> u16 {
        self.cells.get(index).copied().map(cell_width).unwrap_or(0)
    }
}

/// A cell's contribution to the horizontal advance. Spacer tails are already accounted for
/// by the wide cell they follow, so they claim nothing of their own.
///
/// Public because a renderer needs the same answer when sizing a cell's background, and two
/// copies of this rule would disagree the first time one of them changed.
pub fn cell_width(cell: PackedCell) -> u16 {
    match cell.wide() {
        Wide::Wide => 2,
        Wide::SpacerTail => 0,
        _ => 1,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrameCursor {
    pub x: u16,
    pub y: u16,
    pub pending_wrap: bool,
    pub visible: bool,
    pub style: Style,
}

/// One consistent frame, owned by the thread that read it.
#[derive(Debug, Default)]
pub struct Frame {
    pub cols: u16,
    pub rows: u16,
    /// The publish this frame came from. Zero means nothing has been read into it yet.
    pub generation: u64,
    /// Generation of the last whole-frame invalidation.
    pub full_generation: u64,
    pub cursor: FrameCursor,
    /// Terminal mode bits (`Frame::MODE_*`). The frame is how mode state crosses the
    /// thread boundary: the pump owns the terminal, so a host deciding how to encode a
    /// paste reads the mode here rather than reaching into the core.
    pub modes: u64,
    /// Rows this view is scrolled up into history. Zero is the live bottom; while non-zero
    /// the top of the frame shows scrollback and the cursor may be published invisible
    /// because its cell sits below the window.
    pub viewport: u32,
    /// How rows are laid out. Left-to-right by default; see `BaseDirection`.
    pub base_direction: BaseDirection,
    pub(crate) cells: Vec<PackedCell>,
    pub(crate) row_generation: Vec<u64>,
    pub(crate) row_flags: Vec<(bool, bool)>,
    pub(crate) styles: Vec<[u64; 2]>,
    /// OSC 8 URIs for this frame, in slot order; cells point at slots 1-based.
    pub(crate) links: Vec<String>,
    /// Kitty graphics placements visible this frame. Pixels live in the shared image
    /// store; this is only where they anchor.
    pub placements: Vec<FramePlacement>,
}

/// One kitty placement as it crosses the thread boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FramePlacement {
    pub image: u32,
    pub col: u16,
    /// Signed: a negative row is an image partially scrolled off the top, drawn
    /// clipped (both backends bound-check per pixel).
    pub row: i16,
    /// 0 = native pixel size; the renderer resolves the cell box.
    pub cols: u16,
    pub rows: u16,
    /// Kitty `z=`. The list arrives ALREADY SORTED into draw order by the publisher, so
    /// a renderer may walk it start to finish and a layered one may cut it into the three
    /// contiguous bands (`FramePlacement::BELOW_BACKGROUND`, then below 0, then the rest).
    pub z: i32,
}

impl FramePlacement {
    /// Below this, a placement draws under the cell background rather than merely under
    /// the text. The oracle's own threshold (`renderer/image.zig`: `i32::MIN / 2`).
    pub const BELOW_BACKGROUND: i32 = i32::MIN / 2;

    /// Which of the three layers this placement belongs to, low to high.
    pub fn layer(&self) -> u8 {
        match self.z {
            z if z < Self::BELOW_BACKGROUND => 0,
            z if z < 0 => 1,
            _ => 2,
        }
    }
}

impl Frame {
    /// Bit set in `modes` while the child has bracketed paste (DEC 2004) enabled.
    pub const MODE_BRACKETED_PASTE: u64 = 1 << 0;
    /// Bit set while the child has synchronized output (DEC 2026) enabled. Mostly
    /// diagnostic on the reader side -- the pump already withholds publishes during a
    /// batch, so a frame carrying this bit is one the anti-stuck budget forced out.
    pub const MODE_SYNCHRONIZED_OUTPUT: u64 = 1 << 1;
    /// Bits 2-4: the DERIVED mouse event kind (0 none, 1 x10, 2 normal, 3 button,
    /// 4 any -- `ruuah_vt_core::mouse::MouseEvent` in declaration order). The frame
    /// carries the derived pair rather than the nine raw bits because the derived
    /// pair is what an embedder's routing consults; DECRQM owns the raw bits.
    pub const MODE_MOUSE_EVENT_SHIFT: u64 = 2;
    pub const MODE_MOUSE_EVENT_MASK: u64 = 0b111 << 2;
    /// Bits 5-7: the derived report format (0 x10, 1 utf8, 2 sgr, 3 urxvt,
    /// 4 sgr-pixels -- `MouseFormat` in declaration order).
    pub const MODE_MOUSE_FORMAT_SHIFT: u64 = 5;
    pub const MODE_MOUSE_FORMAT_MASK: u64 = 0b111 << 5;
    /// Bit set while alternate scroll (DEC 1007, default ON) is enabled.
    pub const MODE_MOUSE_ALTERNATE_SCROLL: u64 = 1 << 8;
    /// Bit set while the ALTERNATE screen is active. Wheel routing needs it: 1007
    /// converts wheel to arrows only there.
    pub const MODE_ALTERNATE_SCREEN: u64 = 1 << 9;
    /// Bit set while DECCKM (mode 1, application cursor keys) is on: arrows and
    /// alternate-scroll wheel bytes take the `ESC O` form instead of `ESC [`.
    pub const MODE_CURSOR_KEYS: u64 = 1 << 10;
    /// Bit set while DECKPAM (mode 66, keypad application) is on.
    pub const MODE_KEYPAD_KEYS: u64 = 1 << 11;
    /// Bit set while mode 1035 (numlock suppresses keypad application; default ON).
    pub const MODE_IGNORE_KEYPAD_WITH_NUMLOCK: u64 = 1 << 12;
    /// Bit set while mode 1036 (alt sends ESC prefix; default ON).
    pub const MODE_ALT_ESC_PREFIX: u64 = 1 << 13;
    /// Bit set while xterm modifyOtherKeys state 2 is on.
    pub const MODE_MODIFY_OTHER_KEYS_2: u64 = 1 << 14;
    /// Bits 15-19: the ACTIVE screen's negotiated kitty keyboard flags, the wire's
    /// own 5-bit layout (disambiguate 1, events 2, alternates 4, all 8, associated
    /// 16). The stack itself stays in the core; an encoder only ever needs the top.
    pub const MODE_KITTY_KEY_SHIFT: u64 = 15;
    pub const MODE_KITTY_KEY_MASK: u64 = 0b11111 << 15;

    pub fn new() -> Frame {
        Frame::default()
    }

    /// Whether the child enabled bracketed paste (DEC 2004).
    pub fn bracketed_paste(&self) -> bool {
        self.modes & Frame::MODE_BRACKETED_PASTE != 0
    }

    /// Whether the child had synchronized output (DEC 2026) enabled at publish time.
    pub fn synchronized_output(&self) -> bool {
        self.modes & Frame::MODE_SYNCHRONIZED_OUTPUT != 0
    }

    /// The derived mouse event kind at publish time. An unknown bit pattern (a frame
    /// from a newer writer) reads as `None`, which fails safe: no reports.
    pub fn mouse_event(&self) -> ruuah_vt_core::mouse::MouseEvent {
        use ruuah_vt_core::mouse::MouseEvent;
        match (self.modes & Frame::MODE_MOUSE_EVENT_MASK) >> Frame::MODE_MOUSE_EVENT_SHIFT {
            1 => MouseEvent::X10,
            2 => MouseEvent::Normal,
            3 => MouseEvent::Button,
            4 => MouseEvent::Any,
            _ => MouseEvent::None,
        }
    }

    /// The derived mouse report format at publish time; unknown patterns read as the
    /// legacy X10 encoding, the protocol's own fallback.
    pub fn mouse_format(&self) -> ruuah_vt_core::mouse::MouseFormat {
        use ruuah_vt_core::mouse::MouseFormat;
        match (self.modes & Frame::MODE_MOUSE_FORMAT_MASK) >> Frame::MODE_MOUSE_FORMAT_SHIFT {
            1 => MouseFormat::Utf8,
            2 => MouseFormat::Sgr,
            3 => MouseFormat::Urxvt,
            4 => MouseFormat::SgrPixels,
            _ => MouseFormat::X10,
        }
    }

    /// Whether alternate scroll (DEC 1007) was on at publish time.
    pub fn mouse_alternate_scroll(&self) -> bool {
        self.modes & Frame::MODE_MOUSE_ALTERNATE_SCROLL != 0
    }

    /// Whether the alternate screen was active at publish time.
    pub fn alternate_screen(&self) -> bool {
        self.modes & Frame::MODE_ALTERNATE_SCREEN != 0
    }

    /// Whether DECCKM (application cursor keys) was on at publish time.
    pub fn cursor_keys(&self) -> bool {
        self.modes & Frame::MODE_CURSOR_KEYS != 0
    }

    /// Whether DECKPAM (keypad application) was on at publish time.
    pub fn keypad_keys(&self) -> bool {
        self.modes & Frame::MODE_KEYPAD_KEYS != 0
    }

    /// Whether mode 1035 (numlock suppresses keypad application) was on.
    pub fn ignore_keypad_with_numlock(&self) -> bool {
        self.modes & Frame::MODE_IGNORE_KEYPAD_WITH_NUMLOCK != 0
    }

    /// Whether mode 1036 (alt sends ESC prefix) was on.
    pub fn alt_esc_prefix(&self) -> bool {
        self.modes & Frame::MODE_ALT_ESC_PREFIX != 0
    }

    /// Whether xterm modifyOtherKeys state 2 was on.
    pub fn modify_other_keys_2(&self) -> bool {
        self.modes & Frame::MODE_MODIFY_OTHER_KEYS_2 != 0
    }

    /// The active screen's negotiated kitty keyboard flags at publish time.
    pub fn kitty_key_flags(&self) -> u8 {
        ((self.modes & Frame::MODE_KITTY_KEY_MASK) >> Frame::MODE_KITTY_KEY_SHIFT) as u8
    }

    pub(crate) fn resize(&mut self, cols: u16, rows: u16) {
        let cells = usize::from(cols) * usize::from(rows);
        self.cols = cols;
        self.rows = rows;
        self.cells.resize(cells, PackedCell::BLANK);
        self.row_generation.resize(usize::from(rows), 0);
        self.row_flags.resize(usize::from(rows), (false, false));
    }

    /// The OSC 8 URI under a cell, if the cell was printed inside a hyperlink.
    pub fn link(&self, x: u16, y: u16) -> Option<&str> {
        let id = self.cell(x, y).link();
        if id == 0 {
            return None;
        }
        self.links.get(usize::from(id) - 1).map(String::as_str)
    }

    pub fn cell(&self, x: u16, y: u16) -> PackedCell {
        let index = usize::from(y) * usize::from(self.cols) + usize::from(x);
        self.cells.get(index).copied().unwrap_or(PackedCell::BLANK)
    }

    pub fn style(&self, id: u16) -> Style {
        self.styles
            .get(usize::from(id))
            .copied()
            .map(unpack_style)
            .unwrap_or(Style::DEFAULT)
    }

    /// This row soft-wraps into the next.
    pub fn wraps(&self, y: u16) -> bool {
        self.row_flags.get(usize::from(y)).is_some_and(|f| f.0)
    }

    /// Whether this frame corresponds to a real publish.
    ///
    /// False for a frame that has never been read, and for one a mid-copy publish invalidated
    /// -- see `ReadOutcome::Skipped`. A caller that checks the outcome never needs this; it
    /// exists so that one which does not still cannot draw a mixture of two frames unnoticed.
    pub fn is_valid(&self) -> bool {
        self.generation != 0
    }

    /// Whether the renderer still owes this row a repaint, given the last frame it drew.
    ///
    /// Comparing stamps rather than consuming flags is what makes a dropped frame harmless:
    /// a renderer that skipped generations 41 through 46 still sees every row touched in any
    /// of them, because their stamps are all above the 40 it last drew.
    pub fn row_is_stale(&self, y: u16, drawn_generation: u64) -> bool {
        if self.full_generation > drawn_generation {
            return true;
        }
        self.row_generation
            .get(usize::from(y))
            .is_some_and(|stamp| *stamp > drawn_generation)
    }

    /// Rows needing a repaint, given the last generation the caller drew.
    pub fn stale_rows(&self, drawn_generation: u64) -> impl Iterator<Item = u16> + '_ {
        (0..self.rows).filter(move |y| self.row_is_stale(*y, drawn_generation))
    }

    /// The row split into drawable runs, left to right.
    ///
    /// Two splits, in order. First `bidi::visual_spans` decides where each stretch of text
    /// sits and which way it advances; then each span is broken where its style changes,
    /// because a run is what the renderer draws in one go and it can carry only one style.
    /// The second split cannot cross the first: a style change inside a right-to-left span
    /// yields two runs laid out right to left, not two left-to-right runs.
    pub fn runs(&self, y: u16) -> Vec<Run<'_>> {
        let start = usize::from(y) * usize::from(self.cols);
        let end = start + usize::from(self.cols);
        let Some(row) = self.cells.get(start..end) else {
            return Vec::new();
        };

        let mut runs = Vec::new();
        for span in visual_spans(row, self.base_direction) {
            let cells = &row[span.logical.clone()];
            let span_width: u16 = cells.iter().copied().map(cell_width).sum();
            let mut offset = 0;

            while offset < cells.len() {
                let style_id = cells[offset].style_id();
                let mut end_offset = offset + 1;
                while end_offset < cells.len()
                    && (cells[end_offset].style_id() == style_id
                        || cells[end_offset].wide() == Wide::SpacerTail)
                {
                    end_offset += 1;
                }

                let before: u16 = cells[..offset].iter().copied().map(cell_width).sum();
                let piece: u16 = cells[offset..end_offset]
                    .iter()
                    .copied()
                    .map(cell_width)
                    .sum();

                runs.push(Run {
                    // In a right-to-left span the first logical piece is the RIGHTMOST one,
                    // so its column is measured from the span's far edge inwards.
                    start: match span.direction {
                        Direction::LeftToRight => span.column + before,
                        Direction::RightToLeft => span.column + span_width - before - piece,
                    },
                    logical_start: u16::try_from(span.logical.start + offset).unwrap_or(u16::MAX),
                    direction: span.direction,
                    style: self.style(style_id),
                    cells: &cells[offset..end_offset],
                });
                offset = end_offset;
            }
        }
        runs
    }

    /// Where the cell at logical column `x` is actually painted.
    ///
    /// The caret has to follow its cell, not its index. On a reordered row the cell the
    /// program addressed as column 5 can be drawn anywhere, so a caret placed at column 5
    /// highlights an unrelated glyph -- and the glyph it highlights is the one the renderer
    /// was already drawing there, which is why the mistake looks plausible on screen.
    ///
    /// Falls back to `x` for a column no run covers, which is what an all-blank row gives.
    pub fn visual_column(&self, x: u16, y: u16) -> u16 {
        for run in self.runs(y) {
            let start = run.logical_start;
            let end = start.saturating_add(run.cells.len() as u16);
            if x >= start && x < end {
                return run.column_of(usize::from(x - start));
            }
        }
        x
    }

    /// The logical column of whichever cell is painted at screen column `column`.
    ///
    /// The inverse of `visual_column`, and the same function a click has to go through to
    /// land on the character under the pointer. A wide cell answers for both of its columns;
    /// a zero-width spacer answers for none, because it owns no column of its own.
    pub fn logical_column(&self, column: u16, y: u16) -> Option<u16> {
        for run in self.runs(y) {
            for index in 0..run.cells.len() {
                let width = cell_width(run.cells[index]);
                if width == 0 {
                    continue;
                }
                let left = run.column_of(index);
                if column >= left && column < left + width {
                    return Some(run.logical_start + index as u16);
                }
            }
        }
        None
    }

    /// The logical column an arrow key should move the caret to, moving one cell as the eye
    /// sees it rather than as the buffer stores it.
    ///
    /// Inside a right-to-left run these disagree: logically-next is visually-leftward. A
    /// caller handling a key event asks for the visual motion and gets back the logical
    /// column to aim at, which for a shell means deciding whether to send Left or Right.
    ///
    /// This is deliberately not gated on the cell being input: the gate is `is_input`, and it
    /// belongs where the key event is handled. Outside an input region the cursor is the
    /// running program's to place, and stepping it visually would fight the program.
    ///
    /// Returns `x` unchanged at the row's edge, so a caller can detect "nowhere to go".
    pub fn step_visually(&self, x: u16, y: u16, motion: Motion) -> u16 {
        let mut column = self.visual_column(x, y);
        loop {
            column = match motion {
                Motion::Left if column == 0 => return x,
                Motion::Left => column - 1,
                Motion::Right if column + 1 >= self.cols => return x,
                Motion::Right => column + 1,
            };
            if let Some(logical) = self.logical_column(column, y)
                && logical != x
            {
                return logical;
            }
        }
    }

    /// Whether the cell at a logical position is part of user input, per OSC 133.
    ///
    /// The gate for visual caret motion. A shell marks its own input region, so this is the
    /// terminal knowing where the user is typing rather than guessing from the cursor.
    pub fn is_input(&self, x: u16, y: u16) -> bool {
        self.cell(x, y).semantic() == Semantic::Input
    }
}

/// A direction on screen, as the eye sees it. Distinct from `Direction`, which is how a run
/// advances: a caller pressing the left arrow means this, never that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packed::pack_style;
    use ruuah_vt_snapshot::Color;

    fn frame_with(cells: Vec<PackedCell>, styles: Vec<[u64; 2]>) -> Frame {
        let cols = cells.len() as u16;
        Frame {
            cols,
            rows: 1,
            generation: 1,
            full_generation: 0,
            cursor: FrameCursor::default(),
            modes: 0,
            viewport: 0,
            base_direction: BaseDirection::default(),
            cells,
            row_generation: vec![1],
            row_flags: vec![(false, false)],
            styles,
            links: Vec::new(),
            placements: Vec::new(),
        }
    }

    /// A row of `text` at one style, so a caret test can be written in one line.
    fn row(text: &str) -> Frame {
        frame_with(
            text.chars()
                .map(|c| PackedCell::new(&c.to_string(), 0, Wide::Narrow, Semantic::Output))
                .collect(),
            vec![[0, 0]],
        )
    }

    #[test]
    fn a_left_to_right_caret_sits_where_its_index_says() {
        let frame = row("abcd");
        for x in 0..4 {
            assert_eq!(frame.visual_column(x, 0), x);
        }
    }

    /// The case the whole mapping exists for. `שלום` reverses, so the cell the program calls
    /// column 0 is painted at the right-hand end of the run.
    #[test]
    fn a_right_to_left_caret_follows_its_cell_to_the_other_end() {
        let frame = row("שלום");

        assert_eq!(frame.visual_column(0, 0), 3);
        assert_eq!(frame.visual_column(1, 0), 2);
        assert_eq!(frame.visual_column(2, 0), 1);
        assert_eq!(frame.visual_column(3, 0), 0);
    }

    #[test]
    fn visual_and_logical_columns_are_inverses() {
        for text in ["abcd", "שלום", "ab גד", "גיל 42"] {
            let frame = row(text);
            for x in 0..frame.cols {
                let column = frame.visual_column(x, 0);
                assert_eq!(
                    frame.logical_column(column, 0),
                    Some(x),
                    "{text:?} column {x} landed at {column}"
                );
            }
        }
    }

    /// Inside a right-to-left run, visually-left is logically-forward. A caller that sent a
    /// Left key straight through would walk the caret the wrong way along Hebrew.
    #[test]
    fn stepping_left_inside_a_right_to_left_run_moves_logically_forward() {
        let frame = row("שלום");

        assert_eq!(frame.step_visually(1, 0, Motion::Left), 2);
        assert_eq!(frame.step_visually(1, 0, Motion::Right), 0);
    }

    #[test]
    fn stepping_in_a_left_to_right_run_is_the_ordinary_thing() {
        let frame = row("abcd");

        assert_eq!(frame.step_visually(1, 0, Motion::Left), 0);
        assert_eq!(frame.step_visually(1, 0, Motion::Right), 2);
    }

    /// Crossing the boundary between a Latin and a Hebrew run: the caret keeps moving one
    /// column at a time on screen even though the logical column jumps.
    #[test]
    fn stepping_crosses_a_direction_boundary_by_screen_column() {
        let frame = row("ab גד");
        let width = frame.cols;

        let mut seen = Vec::new();
        let mut x = frame.logical_column(0, 0).expect("leftmost cell");
        seen.push(x);
        for _ in 1..width {
            let next = frame.step_visually(x, 0, Motion::Right);
            assert_ne!(next, x, "stepping stalled at logical {x}");
            x = next;
            seen.push(x);
        }

        let mut columns: Vec<u16> = seen.iter().map(|x| frame.visual_column(*x, 0)).collect();
        columns.sort_unstable();
        assert_eq!(
            columns,
            (0..width).collect::<Vec<_>>(),
            "every screen column visited exactly once"
        );
    }

    #[test]
    fn stepping_stops_at_the_edges() {
        let frame = row("abcd");

        assert_eq!(frame.step_visually(0, 0, Motion::Left), 0);
        assert_eq!(frame.step_visually(3, 0, Motion::Right), 3);
    }

    #[test]
    fn a_wide_cell_answers_for_both_of_its_columns() {
        let frame = frame_with(
            vec![
                PackedCell::new("\u{4F60}", 0, Wide::Wide, Semantic::Output),
                PackedCell::new("", 0, Wide::SpacerTail, Semantic::Output),
                PackedCell::new("x", 0, Wide::Narrow, Semantic::Output),
            ],
            vec![[0, 0]],
        );

        assert_eq!(frame.logical_column(0, 0), Some(0));
        assert_eq!(frame.logical_column(1, 0), Some(0));
        assert_eq!(frame.logical_column(2, 0), Some(2));
        // Stepping off the wide cell lands past its tail, not on it.
        assert_eq!(frame.step_visually(0, 0, Motion::Right), 2);
    }

    #[test]
    fn the_input_gate_reads_the_cells_own_mark() {
        let frame = frame_with(
            vec![
                PackedCell::new("$", 0, Wide::Narrow, Semantic::Prompt),
                PackedCell::new("a", 0, Wide::Narrow, Semantic::Input),
                PackedCell::new("o", 0, Wide::Narrow, Semantic::Output),
            ],
            vec![[0, 0]],
        );

        assert!(!frame.is_input(0, 0));
        assert!(frame.is_input(1, 0));
        assert!(!frame.is_input(2, 0));
    }

    #[test]
    fn a_uniform_row_is_one_run() {
        let frame = frame_with(
            (0..4)
                .map(|_| PackedCell::new("a", 0, Wide::Narrow, Semantic::Output))
                .collect(),
            vec![[0, 0]],
        );
        let runs = frame.runs(0);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].start, 0);
        assert_eq!(runs[0].width(), 4);
    }

    #[test]
    fn a_style_change_starts_a_new_run_at_the_right_column() {
        let red = Style {
            fg: Color::Palette(1),
            ..Style::DEFAULT
        };
        let frame = frame_with(
            vec![
                PackedCell::new("a", 0, Wide::Narrow, Semantic::Output),
                PackedCell::new("b", 0, Wide::Narrow, Semantic::Output),
                PackedCell::new("c", 1, Wide::Narrow, Semantic::Output),
            ],
            vec![[0, 0], pack_style(&red)],
        );

        let runs = frame.runs(0);
        assert_eq!(runs.len(), 2);
        assert_eq!((runs[0].start, runs[0].width()), (0, 2));
        assert_eq!((runs[1].start, runs[1].width()), (2, 1));
        assert_eq!(runs[1].style, red);
    }

    #[test]
    fn a_wide_cell_and_its_spacer_stay_in_one_run_and_claim_two_columns() {
        let frame = frame_with(
            vec![
                PackedCell::new("\u{4F60}", 0, Wide::Wide, Semantic::Output),
                PackedCell::new("", 0, Wide::SpacerTail, Semantic::Output),
                PackedCell::new("x", 0, Wide::Narrow, Semantic::Output),
            ],
            vec![[0, 0]],
        );

        let runs = frame.runs(0);
        assert_eq!(runs.len(), 1);
        assert_eq!(
            runs[0].width(),
            3,
            "two columns for the wide cell, one for x"
        );
        assert_eq!(runs[0].column_of(2), 2, "x is drawn past the wide cell");
    }

    #[test]
    fn a_right_to_left_run_lays_its_cells_out_backwards() {
        // Nothing produces one yet. The assertion is the contract slice 5.5 will satisfy,
        // and the reason a renderer must ask `column_of` instead of adding to `start`.
        let cells: Vec<PackedCell> = "\u{05D0}\u{05D1}\u{05D2}"
            .chars()
            .map(|c| PackedCell::new(&c.to_string(), 0, Wide::Narrow, Semantic::Output))
            .collect();
        let run = Run {
            start: 4,
            logical_start: 0,
            direction: Direction::RightToLeft,
            style: Style::DEFAULT,
            cells: &cells,
        };

        assert_eq!(run.width(), 3);
        assert_eq!(
            run.column_of(0),
            6,
            "the first letter sits at the right edge"
        );
        assert_eq!(run.column_of(1), 5);
        assert_eq!(run.column_of(2), 4);
    }

    #[test]
    fn a_row_is_stale_only_above_the_generation_already_drawn() {
        let mut frame = frame_with(vec![PackedCell::BLANK], vec![[0, 0]]);
        frame.row_generation = vec![7];

        assert!(frame.row_is_stale(0, 6));
        assert!(!frame.row_is_stale(0, 7));
        assert!(!frame.row_is_stale(0, 9));
    }

    #[test]
    fn a_whole_frame_invalidation_outranks_every_row_stamp() {
        let mut frame = frame_with(vec![PackedCell::BLANK], vec![[0, 0]]);
        frame.row_generation = vec![1];
        frame.full_generation = 9;

        assert!(frame.row_is_stale(0, 8));
        assert_eq!(frame.stale_rows(8).count(), 1);
        assert_eq!(frame.stale_rows(9).count(), 0);
    }
}
