//! Purpose: intern styles so a cell carries a 2-byte ID instead of inline colours.
//! Public surface: `StyleId`, `DEFAULT_STYLE_ID`, `StyleTable`.
//! Why this file: this is the memory decision. Alacritty stores fg/bg in the cell and lands
//!   near 24 bytes; Ghostty stores a u16 index into a per-page table and lands at 8. On an
//!   80-column grid with 10k scrollback that is roughly 6 MB against 19 MB, for a grid that
//!   is mostly blank. The C ABI also exposes the style as a `uint16_t` ID, so this is forced.
//! NOT responsible for: the meaning of the attributes -- `Style` is the shared comparison
//!   type from `ruuah-vt-snapshot`, so neither the core nor the oracle owns its definition.
//! Test strategy: unit tests below; end to end via the differential corpus.

use std::collections::HashMap;

use ruuah_vt_snapshot::Style;

/// Index into a `StyleTable`. Matches the C ABI's `GHOSTTY_CELL_DATA_STYLE_ID` width.
pub type StyleId = u16;

/// Always the default style. Reserved so the common case needs no lookup at all.
pub const DEFAULT_STYLE_ID: StyleId = 0;

/// Deduplicating store mapping styles to small IDs.
///
/// A terminal screen uses a handful of distinct styles across thousands of cells, so
/// interning collapses almost all of that to repeated IDs.
#[derive(Debug)]
pub struct StyleTable {
    styles: Vec<Style>,
    ids: HashMap<Style, StyleId>,
}

impl StyleTable {
    pub fn new() -> StyleTable {
        StyleTable {
            styles: vec![Style::DEFAULT],
            ids: HashMap::new(),
        }
    }

    /// Returns the ID for `style`, allocating one if this is the first sighting.
    ///
    /// If the table is exhausted the style is dropped to default rather than panicking:
    /// the core must absorb hostile input without dying, and a wrong colour is a visible
    /// diff while a panic takes the process. Slice 3 revisits this with per-page tables and
    /// reference counting, which is how Ghostty keeps the space bounded.
    pub fn intern(&mut self, style: Style) -> StyleId {
        if style.is_default() {
            return DEFAULT_STYLE_ID;
        }
        if let Some(id) = self.ids.get(&style) {
            return *id;
        }
        let Ok(id) = StyleId::try_from(self.styles.len()) else {
            return DEFAULT_STYLE_ID;
        };
        self.styles.push(style);
        self.ids.insert(style, id);
        id
    }

    /// Resolves an ID. An unknown ID reads as the default style rather than panicking.
    pub fn get(&self, id: StyleId) -> Style {
        self.styles
            .get(usize::from(id))
            .copied()
            .unwrap_or(Style::DEFAULT)
    }

    /// Number of distinct styles held, including the default at index 0.
    pub fn len(&self) -> usize {
        self.styles.len()
    }

    pub fn is_empty(&self) -> bool {
        false
    }
}

impl Default for StyleTable {
    fn default() -> Self {
        StyleTable::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruuah_vt_snapshot::Color;

    fn bold() -> Style {
        Style {
            bold: true,
            ..Style::DEFAULT
        }
    }

    #[test]
    fn the_default_style_is_always_id_zero_and_costs_no_slot() {
        let mut table = StyleTable::new();
        assert_eq!(table.intern(Style::DEFAULT), DEFAULT_STYLE_ID);
        assert_eq!(table.len(), 1, "default must not allocate a new slot");
    }

    #[test]
    fn the_same_style_interns_to_the_same_id() {
        let mut table = StyleTable::new();
        let first = table.intern(bold());
        let second = table.intern(bold());
        assert_eq!(first, second);
        assert_eq!(table.len(), 2, "one default plus one bold");
    }

    #[test]
    fn distinct_styles_get_distinct_ids_and_round_trip() {
        let mut table = StyleTable::new();
        let red = Style {
            fg: Color::Palette(1),
            ..Style::DEFAULT
        };
        let bold_id = table.intern(bold());
        let red_id = table.intern(red);

        assert_ne!(bold_id, red_id);
        assert_eq!(table.get(bold_id), bold());
        assert_eq!(table.get(red_id), red);
    }

    #[test]
    fn an_unknown_id_reads_as_default_rather_than_panicking() {
        let table = StyleTable::new();
        assert_eq!(table.get(9999), Style::DEFAULT);
    }
}
