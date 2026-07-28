//! Purpose: the storage unit of the grid, fixed at 8 bytes with no per-cell allocation.
//! Public surface: `Cell`, `Wide`, `CellFlags`.
//! Why this file: the cell layout is the one decision the rest of the core cannot walk
//!   back. It is modelled on Ghostty's `packed struct(u64)` (`ruuah/src/terminal/page.zig`)
//!   and on the C ABI, which already exposes `GHOSTTY_CELL_DATA_STYLE_ID` as a `uint16_t` --
//!   so an interned style ID is inherited, not chosen. Inline colours would cost 3x here.
//! NOT responsible for: what a style ID means (`style.rs`), where continuation codepoints
//!   live (`grid.rs`), or page-internal offset allocation (deferred to slice 3).
//! Test strategy: the 8-byte guarantee is a compile-time assertion, not a hope.

use ruuah_vt_snapshot::Semantic;

use crate::style::{DEFAULT_STYLE_ID, StyleId};

/// How much horizontal space a cell claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Wide {
    /// Ordinary width-1 cell.
    Narrow = 0,
    /// First half of a width-2 cell.
    Wide = 1,
    /// Second half of a width-2 cell. Carries no text and is not rendered.
    SpacerTail = 2,
    /// Filler where a wide cell could not fit before a soft wrap.
    SpacerHead = 3,
}

/// Per-cell bits that would otherwise be padding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellFlags(u8);

impl CellFlags {
    pub const NONE: CellFlags = CellFlags(0);

    const HAS_GRAPHEME: u8 = 1 << 0;
    const SEMANTIC_SHIFT: u8 = 1;
    const SEMANTIC_MASK: u8 = 0b11 << Self::SEMANTIC_SHIFT;

    /// Whether this cell has continuation codepoints in the grid's grapheme map.
    ///
    /// Checked before the map lookup, which is the entire reason the bit exists: the
    /// overwhelming majority of cells are a single codepoint and must not pay for a hash.
    pub fn has_grapheme(self) -> bool {
        self.0 & Self::HAS_GRAPHEME != 0
    }

    pub fn set_has_grapheme(&mut self, on: bool) {
        if on {
            self.0 |= Self::HAS_GRAPHEME;
        } else {
            self.0 &= !Self::HAS_GRAPHEME;
        }
    }

    /// What OSC 133 said this cell's content is.
    ///
    /// Two spare bits rather than a field: `Cell` is asserted at 8 bytes and every one of
    /// them is already spoken for, so a new field would double scrollback's per-cell cost to
    /// carry two bits. Ghostty packs the same value into the same slack for the same reason.
    pub fn semantic(self) -> Semantic {
        match (self.0 & Self::SEMANTIC_MASK) >> Self::SEMANTIC_SHIFT {
            1 => Semantic::Input,
            2 => Semantic::Prompt,
            _ => Semantic::Output,
        }
    }

    pub fn set_semantic(&mut self, semantic: Semantic) {
        let bits = match semantic {
            Semantic::Output => 0,
            Semantic::Input => 1,
            Semantic::Prompt => 2,
        };
        self.0 = (self.0 & !Self::SEMANTIC_MASK) | (bits << Self::SEMANTIC_SHIFT);
    }

    pub fn with_semantic(semantic: Semantic) -> CellFlags {
        let mut flags = CellFlags::NONE;
        flags.set_semantic(semantic);
        flags
    }
}

/// One grid cell. Plain old data, copyable, never owning a heap allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    /// The first codepoint of the cell's grapheme cluster. Zero means no text.
    pub codepoint: u32,
    /// Index into the grid's style table. Zero is always the default style.
    pub style_id: StyleId,
    pub wide: Wide,
    pub flags: CellFlags,
}

/// The layout promise, enforced by the compiler rather than by a comment. A regression here
/// silently multiplies scrollback memory, which is exactly the failure slice 3 is braced for.
const _: () = assert!(
    size_of::<Cell>() == 8,
    "Cell must stay 8 bytes; scrollback cost is driven by width, so every byte is per blank cell"
);

impl Cell {
    pub const BLANK: Cell = Cell {
        codepoint: 0,
        style_id: DEFAULT_STYLE_ID,
        wide: Wide::Narrow,
        flags: CellFlags::NONE,
    };

    /// Whether the cell holds text. A cell holding U+0020 does; an untouched cell does not.
    pub fn has_text(self) -> bool {
        self.codepoint != 0
    }
}

impl Default for Cell {
    fn default() -> Self {
        Cell::BLANK
    }
}

impl From<Wide> for ruuah_vt_snapshot::Wide {
    fn from(wide: Wide) -> Self {
        match wide {
            Wide::Narrow => ruuah_vt_snapshot::Wide::Narrow,
            Wide::Wide => ruuah_vt_snapshot::Wide::Wide,
            Wide::SpacerTail => ruuah_vt_snapshot::Wide::SpacerTail,
            Wide::SpacerHead => ruuah_vt_snapshot::Wide::SpacerHead,
        }
    }
}
