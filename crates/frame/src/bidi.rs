//! Purpose: decide where each cell of a row is drawn, once right-to-left text is involved.
//! Public surface: `BaseDirection`, `VisualSpan`, `visual_spans`, `is_layout`.
//! Why this file: reordering must happen HERE and nowhere below. The core stays byte-exact
//!   against libghostty-vt, whose ABI has no bidi surface at all, so reordering in the core
//!   would make every RTL line diverge from the oracle by construction and delete the only
//!   correctness signal the project has. The renderer above never learns which way a run
//!   goes -- it asks `Run::column_of` -- so this file is the only thing slice 5.5 changes.
//! NOT responsible for: the UBA itself (`wezterm-bidi`, verified against the full Unicode
//!   suite -- see `tests/bidi_conformance.rs`), shaping, or drawing.
//! Test strategy: the algorithm is measured against 861,948 Unicode conformance cases in
//!   `tests/bidi_conformance.rs`. The terminal POLICY on top of it -- segment bounding and
//!   base direction -- is unit-tested below, because the Unicode suite has no opinion about
//!   either and cannot catch a wrong choice there.

use std::ops::Range;

use wezterm_bidi::{BidiContext, Direction as BidiDirection, ParagraphDirectionHint};

use crate::frame::{Direction, cell_width};
use crate::packed::{CLUSTER_BYTES, PackedCell};

/// The base direction a row is laid out in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BaseDirection {
    /// The terminal default, and deliberately not `Auto`.
    ///
    /// A terminal grid is addressed in columns by the program drawing into it: a status line
    /// written at column 0 must stay at column 0. Auto-detection would flip the whole row's
    /// base level the moment its first strong character is Hebrew, moving that text to the
    /// right edge and putting it somewhere the program never asked for. With an LTR base,
    /// right-to-left runs still reorder correctly *within their own span* -- a Hebrew phrase
    /// reads properly -- while every column the application computed stays where it put it.
    #[default]
    LeftToRight,
    RightToLeft,
    /// First-strong detection, falling back to LTR. Available, not the default.
    Auto,
}

impl BaseDirection {
    fn hint(self) -> ParagraphDirectionHint {
        match self {
            BaseDirection::LeftToRight => ParagraphDirectionHint::LeftToRight,
            BaseDirection::RightToLeft => ParagraphDirectionHint::RightToLeft,
            BaseDirection::Auto => ParagraphDirectionHint::AutoLeftToRight,
        }
    }
}

/// A contiguous logical span of cells that draws as one unit at one screen column.
///
/// Contiguous is not an accident: a bidi level run is by definition a contiguous range of the
/// logical text, which is exactly why reordering does not have to disturb the shape of a
/// `Run` -- only its starting column and its direction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisualSpan {
    pub logical: Range<usize>,
    pub direction: Direction,
    /// Leftmost screen column this span occupies.
    pub column: u16,
}

/// Characters that bound a reordering segment.
///
/// Box drawing and block elements. This is the whole of Orel's segment rule, and the reason
/// for it is that naive whole-line UBA is what shears a TUI apart: a table row is
/// `│ text │ text │`, and reordering it as one paragraph moves the frame characters relative
/// to the text they enclose. Bounding segments at the frame keeps every box where the program
/// drew it and reorders only the text inside each compartment.
pub fn is_layout(c: char) -> bool {
    // Box drawing and block elements are adjacent, so this is one range.
    matches!(u32::from(c), 0x2500..=0x259F)
}

/// The character a cell contributes to the bidi algorithm.
///
/// A cell with no text is a space: blank cells are the padding a terminal row is full of, and
/// treating them as neutrals is what lets rule L1 reset trailing whitespace to the base level
/// instead of dragging it into an RTL run.
fn cell_char(cell: PackedCell) -> char {
    let mut scratch = [0u8; CLUSTER_BYTES];
    cell.cluster(&mut scratch).chars().next().unwrap_or(' ')
}

fn direction_of(direction: BidiDirection) -> Direction {
    match direction {
        BidiDirection::LeftToRight => Direction::LeftToRight,
        BidiDirection::RightToLeft => Direction::RightToLeft,
    }
}

fn span_width(row: &[PackedCell], logical: &Range<usize>) -> u16 {
    row[logical.clone()].iter().copied().map(cell_width).sum()
}

/// Splits a row into the segments reordering may not cross.
///
/// Every layout character is a segment of its own; everything between them is one segment.
fn segments(row: &[PackedCell]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;

    for (index, cell) in row.iter().enumerate() {
        if is_layout(cell_char(*cell)) {
            if start < index {
                out.push(start..index);
            }
            out.push(index..index + 1);
            start = index + 1;
        }
    }
    if start < row.len() {
        out.push(start..row.len());
    }
    out
}

/// Lays a row out left to right, reordering right-to-left text within each segment.
pub fn visual_spans(row: &[PackedCell], base: BaseDirection) -> Vec<VisualSpan> {
    if row.is_empty() {
        return Vec::new();
    }

    let mut spans = Vec::new();
    let mut column = 0u16;
    let mut context = BidiContext::new();

    for segment in segments(row) {
        let chars: Vec<char> = row[segment.clone()]
            .iter()
            .copied()
            .map(cell_char)
            .collect();

        // A segment with nothing to reorder skips the algorithm entirely. Worth doing: the
        // overwhelmingly common row is pure ASCII and should not pay for the UBA at all.
        //
        // The base direction is part of the condition, and leaving it out was a real bug the
        // conformance suite caught on its first run: under an RTL base even a row of pure
        // neutrals resolves to level 1 and reverses, so "no right-to-left characters" is not
        // on its own a licence to skip. Auto is safe here only because a segment with no
        // strong RTL character auto-detects to LTR by definition.
        let skippable =
            !matches!(base, BaseDirection::RightToLeft) && chars.iter().all(|c| !needs_bidi(*c));
        if skippable {
            let logical = segment.clone();
            let width = span_width(row, &logical);
            spans.push(VisualSpan {
                logical,
                direction: Direction::LeftToRight,
                column,
            });
            column += width;
            continue;
        }

        context.resolve_paragraph(&chars, base.hint());
        for run in context.reordered_runs(0..chars.len()) {
            let logical = segment.start + run.range.start..segment.start + run.range.end;
            let width = span_width(row, &logical);
            spans.push(VisualSpan {
                logical,
                direction: direction_of(run.direction),
                column,
            });
            column += width;
        }
    }

    spans
}

/// Whether a character can possibly affect reordering.
///
/// Everything below U+0590 is left-to-right or neutral in a way that cannot reorder anything
/// on its own, so a row of code or English skips the algorithm. The ranges above are the
/// right-to-left scripts plus the explicit formatting characters.
fn needs_bidi(c: char) -> bool {
    matches!(u32::from(c),
        0x0590..=0x08FF        // Hebrew, Arabic, Syriac, Thaana, N'Ko, Samaritan, Arabic ext
        | 0x200E..=0x200F      // LRM / RLM
        | 0x202A..=0x202E      // embeddings and overrides
        | 0x2066..=0x2069      // isolates
        | 0xFB1D..=0xFDFF      // Hebrew and Arabic presentation forms A
        | 0xFE70..=0xFEFF      // Arabic presentation forms B
        | 0x10800..=0x10FFF    // Cypriot, Phoenician, Kharoshthi and friends
        | 0x1E800..=0x1EFFF    // Mende Kikakui, Adlam, Arabic mathematical
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruuah_vt_snapshot::{Semantic, Wide};

    fn row(text: &str) -> Vec<PackedCell> {
        text.chars()
            .map(|c| PackedCell::new(&c.to_string(), 0, Wide::Narrow, Semantic::Output))
            .collect()
    }

    /// The visual left-to-right reading of a row, as a string.
    fn rendered(row: &[PackedCell], base: BaseDirection) -> String {
        let mut out = String::new();
        let mut scratch = [0u8; CLUSTER_BYTES];
        for span in visual_spans(row, base) {
            let cells = &row[span.logical.clone()];
            let text: String = cells
                .iter()
                .map(|cell| cell.cluster(&mut scratch).chars().next().unwrap_or(' '))
                .collect();
            match span.direction {
                Direction::LeftToRight => out.push_str(&text),
                Direction::RightToLeft => out.extend(text.chars().rev()),
            }
        }
        out
    }

    #[test]
    fn pure_ascii_is_untouched_and_never_enters_the_algorithm() {
        let cells = row("fn main() { 42 }");
        let spans = visual_spans(&cells, BaseDirection::LeftToRight);

        assert_eq!(spans.len(), 1, "one span, no reordering");
        assert_eq!(spans[0].direction, Direction::LeftToRight);
        assert_eq!(spans[0].column, 0);
        assert_eq!(
            rendered(&cells, BaseDirection::LeftToRight),
            "fn main() { 42 }"
        );
    }

    #[test]
    fn a_hebrew_word_is_reversed_so_it_reads_right_to_left() {
        // Logical order is shin-lamed-vav-mem; on screen the shin must be RIGHTMOST.
        let cells = row("\u{05E9}\u{05DC}\u{05D5}\u{05DD}");
        assert_eq!(
            rendered(&cells, BaseDirection::LeftToRight),
            "\u{05DD}\u{05D5}\u{05DC}\u{05E9}"
        );
    }

    #[test]
    fn two_hebrew_words_keep_their_order_relative_to_each_other() {
        // The space between two RTL runs joins them, so the PHRASE reverses as a unit rather
        // than each word reversing in place. Getting this wrong reads as correct letters in
        // the wrong word order, which is the subtlest possible bidi bug.
        let cells = row("\u{05E9}\u{05DC}\u{05D5}\u{05DD} \u{05E2}\u{05D5}\u{05DC}\u{05DD}");
        assert_eq!(
            rendered(&cells, BaseDirection::LeftToRight),
            "\u{05DD}\u{05DC}\u{05D5}\u{05E2} \u{05DD}\u{05D5}\u{05DC}\u{05E9}"
        );
    }

    #[test]
    fn latin_around_hebrew_stays_put_and_only_the_hebrew_flips() {
        let cells = row("let x = \u{05E9}\u{05DC}\u{05D5}\u{05DD};");
        let visual = rendered(&cells, BaseDirection::LeftToRight);

        assert!(
            visual.starts_with("let x = "),
            "code keeps its place: {visual}"
        );
        assert!(visual.ends_with(';'));
        assert!(
            visual.contains("\u{05DD}\u{05D5}\u{05DC}\u{05E9}"),
            "the Hebrew reversed"
        );
    }

    #[test]
    fn an_ltr_base_leaves_a_hebrew_line_starting_at_column_zero() {
        // The policy assertion. A terminal program that wrote at column 0 must find its text
        // at column 0; auto-detection would move this whole row to the right edge.
        let mut cells = row("\u{05E9}\u{05DC}\u{05D5}\u{05DD}");
        cells.extend(row("          "));

        let spans = visual_spans(&cells, BaseDirection::LeftToRight);
        assert_eq!(spans[0].column, 0);
        assert_eq!(
            spans[0].direction,
            Direction::RightToLeft,
            "the text itself still reorders"
        );
    }

    #[test]
    fn box_drawing_bounds_a_segment_so_a_table_cannot_shear() {
        // The rule that makes this usable in a TUI. Reordered as ONE paragraph, the frame
        // characters would move relative to the text they enclose.
        let cells = row("\u{2502}\u{05D0}\u{05D1}\u{2502}ab\u{2502}");
        let spans = visual_spans(&cells, BaseDirection::LeftToRight);

        let bars: Vec<u16> = spans
            .iter()
            .filter(|s| s.logical.len() == 1 && is_layout(cell_char(cells[s.logical.start])))
            .map(|s| s.column)
            .collect();
        assert_eq!(
            bars,
            vec![0, 3, 6],
            "every bar stays in the column it was drawn in"
        );

        let visual = rendered(&cells, BaseDirection::LeftToRight);
        assert_eq!(visual, "\u{2502}\u{05D1}\u{05D0}\u{2502}ab\u{2502}");
    }

    #[test]
    fn every_span_is_contiguous_and_they_tile_the_row_exactly_once() {
        // The invariant the run builder depends on: spans partition the row. If they ever
        // overlapped or left a hole, cells would be drawn twice or not at all.
        let cells = row("ab \u{05D0}\u{05D1} cd \u{2502} \u{05D2}\u{05D3}");
        let spans = visual_spans(&cells, BaseDirection::LeftToRight);

        let mut covered: Vec<usize> = spans.iter().flat_map(|s| s.logical.clone()).collect();
        covered.sort_unstable();
        assert_eq!(covered, (0..cells.len()).collect::<Vec<_>>());
    }

    #[test]
    fn columns_are_assigned_left_to_right_without_gaps() {
        let cells = row("ab\u{05D0}\u{05D1}cd");
        let spans = visual_spans(&cells, BaseDirection::LeftToRight);

        let mut expected = 0u16;
        for span in &spans {
            assert_eq!(
                span.column, expected,
                "span {span:?} starts in the wrong column"
            );
            expected += span_width(&cells, &span.logical);
        }
        assert_eq!(expected, cells.len() as u16);
    }

    #[test]
    fn an_rtl_base_is_available_and_changes_the_layout() {
        let cells = row("ab \u{05D0}\u{05D1}");
        assert_ne!(
            rendered(&cells, BaseDirection::LeftToRight),
            rendered(&cells, BaseDirection::RightToLeft),
            "the base direction has to actually do something"
        );
    }

    #[test]
    fn an_empty_row_produces_no_spans() {
        assert!(visual_spans(&[], BaseDirection::LeftToRight).is_empty());
    }
}
