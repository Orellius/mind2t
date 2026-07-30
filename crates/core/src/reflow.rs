//! Purpose: re-lay a sequence of rows at a new width, carrying the cursor through it.
//! Public surface: `Anchor`, `Reflowed`, `Mode`, `resize_rows`.
//! Why this file: reflow is the plan's ranked failure mode 1 and the one part of the core
//!   that is pure transformation -- rows and a cursor in, rows and a cursor out. Keeping it
//!   a free function over owned rows means it can be unit-tested without a terminal, and it
//!   is why the active area and the scrollback can be reflowed by the same code despite
//!   being stored differently.
//! NOT responsible for: deciding which rows are active and which are history, pruning to
//!   the scrollback budget, or touching a `Grid` (`screen.rs` does all three).
//! Test strategy: the differential corpus owns the semantics; the unit tests below cover the
//!   arithmetic that is cheapest to read in isolation (the cursor landing past the end of
//!   its line, wide cells at the new edge, blank-row deferral).

use ruuah_vt_snapshot::{RowSemantic, Semantic, Style};

use crate::cell::Wide;
use crate::grid::RowMeta;
use crate::page::{HistoryCell, HistoryRow};
use crate::split::{mark_for, split};

/// A position within a row sequence: which row, and which column of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    pub row: usize,
    pub x: u16,
}

/// Whether soft-wrapped rows rejoin at the new width.
///
/// The alternate screen is documented by the ABI as not reflowing, so it takes `Truncate`:
/// every row keeps its own identity and simply gains or loses columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Rejoin,
    Truncate,
}

/// The rows at the new width, and where the tracked positions ended up among them.
#[derive(Debug)]
pub struct Reflowed {
    /// Extra tracked anchors (placement cells), in input order. `None` = the anchor's
    /// row did not survive the transform. Saved-cursor clamping rules: an extra anchor
    /// never extends a line.
    pub extras: Vec<Option<Anchor>>,
    pub rows: Vec<HistoryRow>,
    pub cursor: Anchor,
    /// The saved (DECSC) cursor, if one was tracked. `None` means its row did not survive.
    pub saved: Option<Anchor>,
}

/// Re-lays `rows` from `old_cols` to `new_cols`, mapping `cursor` through the transform.
///
/// Rows are grouped into logical lines by the soft-wrap flag, each line is rejoined into one
/// run of cells and re-split at the new width. Three rules are not obvious and all three were
/// measured against libghostty-vt on 2026-07-28:
///
/// - Trailing blanks are trimmed from the last row of a line only. The earlier rows of a
///   soft-wrapped line are full by construction, so trimming them would delete real spaces.
/// - The cursor's line is extended to reach the cursor even when that is past the content.
///   This is why narrowing a full line leaves an empty continuation row under it: the cursor
///   sat one cell past the text, and that cell has to exist somewhere.
/// - A line with no content at all is deferred, and emitted only if a line with content
///   follows. That is what drops trailing blank rows while keeping interior ones, and the
///   rule above is what stops it from eating the row the cursor is parked on.
pub fn resize_rows(
    rows: &[HistoryRow],
    old_cols: u16,
    new_cols: u16,
    cursor: Anchor,
    saved: Option<Anchor>,
    extras: &[Anchor],
    mode: Mode,
) -> Reflowed {
    if new_cols == 0 {
        return Reflowed {
            rows: Vec::new(),
            cursor: Anchor { row: 0, x: 0 },
            saved: None,
            extras: vec![None; extras.len()],
        };
    }

    let mut out: Vec<HistoryRow> = Vec::new();
    let mut deferred_blanks = 0usize;
    let mut out_cursor = Anchor { row: 0, x: 0 };
    let mut out_saved = None;
    let mut out_extras: Vec<Option<Anchor>> = vec![None; extras.len()];

    for line in lines(rows, mode) {
        let last = line.end;

        let mut content: Vec<HistoryCell> = Vec::new();
        let mut tracked: Vec<Option<usize>> = vec![None; 2 + extras.len()];
        // Where each source row's cells begin in `content`, with the mark it carried.
        let mut marks: Vec<(usize, RowSemantic)> = Vec::new();
        for (index, row) in rows[line.start..=last].iter().enumerate() {
            let y = line.start + index;
            let row_start = content.len();
            marks.push((row_start, row.meta.semantic_prompt));
            let width = match mode {
                Mode::Rejoin => old_cols,
                // Without a rejoin there is nowhere for an overhanging cell to go, so the
                // row is cut at the new width here rather than after the split.
                Mode::Truncate => new_cols.min(old_cols),
            };
            let cells = row_cells(row, width, y == last);
            let row_len = cells.len();
            content.extend(cells);

            if cursor.row == y {
                // Without a rejoin the cursor cannot push its row past the width: it is
                // clamped instead, so a row never splits in two and the count is preserved.
                let x = match mode {
                    Mode::Rejoin => cursor.x,
                    Mode::Truncate => cursor.x.min(new_cols - 1),
                };
                tracked[0] = Some(row_start + usize::from(x));
            }
            if let Some(saved) = saved.filter(|saved| saved.row == y) {
                // Inside its row's content the saved cursor travels with its cell, exactly
                // as the cursor does. Past the content it is clamped instead of extending
                // the line: only the cursor may add a row, and a saved position that moved
                // every row below it would relocate the whole screen.
                let x = if usize::from(saved.x) < row_len {
                    usize::from(saved.x)
                } else {
                    let column = row_start % usize::from(new_cols);
                    usize::from(saved.x).min(usize::from(new_cols) - 1 - column)
                };
                tracked[1] = Some(row_start + x);
            }
            for (slot, extra) in extras.iter().enumerate() {
                // Placement anchors take the saved-cursor rule: travel with the cell,
                // clamp past content, never extend a line -- an image may not add rows.
                if extra.row == y {
                    let x = if usize::from(extra.x) < row_len {
                        usize::from(extra.x)
                    } else {
                        let column = row_start % usize::from(new_cols);
                        usize::from(extra.x).min(usize::from(new_cols) - 1 - column)
                    };
                    tracked[2 + slot] = Some(row_start + x);
                }
            }
        }

        // A tracked cell has to exist, which is also what keeps the row the cursor is
        // parked on from being deferred as blank.
        let reach = tracked.iter().flatten().copied().max();
        if let Some(offset) = reach {
            while content.len() <= offset {
                content.push(blank_cell());
            }
        }

        if content.is_empty() {
            // A blank continuation row belongs to the line above it and is not re-emitted;
            // any other blank row waits to see whether anything follows it.
            if !rows[line.start].meta.wrap_continuation {
                deferred_blanks += 1;
            }
            continue;
        }

        for _ in 0..deferred_blanks {
            out.push(blank_row());
        }
        deferred_blanks = 0;

        let split = split(&content, new_cols, &tracked);
        let first = out.len();
        let count = split.rows.len();
        let content_end = split.content_end;
        for (index, cells) in split.rows.into_iter().enumerate() {
            out.push(HistoryRow {
                cells,
                meta: RowMeta {
                    wrap: if index + 1 < count {
                        true
                    } else {
                        rows[last].meta.wrap
                    },
                    wrap_continuation: if index > 0 {
                        true
                    } else {
                        rows[line.start].meta.wrap_continuation
                    },
                    // Upstream's reflow writer copies each source row's mark onto the
                    // destination row it is filling, so the last source row to reach a
                    // destination row wins. The three `reflow-semantic-*` cases pin it.
                    semantic_prompt: mark_for(&marks, content_end[index]),
                },
            });
        }

        if let Some((row, x)) = split.tracked[0] {
            out_cursor = Anchor {
                row: first + row,
                x,
            };
        }
        if let Some((row, x)) = split.tracked[1] {
            out_saved = Some(Anchor {
                row: first + row,
                x,
            });
        }
        for (slot, landed) in split.tracked.iter().enumerate().skip(2) {
            if let Some((row, x)) = landed {
                out_extras[slot - 2] = Some(Anchor {
                    row: first + row,
                    x: *x,
                });
            }
        }
    }

    Reflowed {
        rows: out,
        cursor: out_cursor,
        saved: out_saved,
        extras: out_extras,
    }
}

/// One logical line: a run of rows joined by the soft-wrap flag, inclusive at both ends.
struct Line {
    start: usize,
    end: usize,
}

fn lines(rows: &[HistoryRow], mode: Mode) -> Vec<Line> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < rows.len() {
        let mut end = start;
        if mode == Mode::Rejoin {
            while end + 1 < rows.len() && rows[end].meta.wrap {
                end += 1;
            }
        }
        out.push(Line { start, end });
        start = end + 1;
    }
    out
}

/// A row's contribution to its line: padded to `width`, spacer-head padding dropped, and
/// trailing blanks trimmed if it is the row that ends the line.
fn row_cells(row: &HistoryRow, width: u16, is_last: bool) -> Vec<HistoryCell> {
    let mut cells: Vec<HistoryCell> = row.cells.iter().take(usize::from(width)).cloned().collect();
    while cells.len() < usize::from(width) {
        cells.push(blank_cell());
    }

    // A spacer head is padding the old width left behind, not content. Keeping it would
    // reserve a column for a wide cell that may now fit on the line above.
    while cells.last().is_some_and(|cell| cell.wide == Wide::SpacerHead) {
        cells.pop();
    }

    if is_last {
        while cells.last().is_some_and(is_blank) {
            cells.pop();
        }
    }

    // Truncation can cut a wide cell in half. The orphaned head is blanked rather than left
    // behind: a head with no tail is a state nothing else in the grid can produce.
    if cells.last().is_some_and(|cell| cell.wide == Wide::Wide) {
        cells.pop();
        cells.push(blank_cell());
    }

    cells
}

fn is_blank(cell: &HistoryCell) -> bool {
    // A spacer tail is never trimmed on its own: it belongs to the wide cell to its left,
    // and dropping it would leave that cell claiming a column it no longer has.
    cell.codepoint == 0
        && cell.wide != Wide::SpacerTail
        && cell.graphemes.is_empty()
        && cell.style == Style::DEFAULT
}

pub(crate) fn blank_cell() -> HistoryCell {
    HistoryCell {
        link: None,
        codepoint: 0,
        wide: Wide::Narrow,
        style: Style::DEFAULT,
        semantic: Semantic::Output,
        graphemes: Vec::new(),
    }
}

fn blank_row() -> HistoryRow {
    HistoryRow {
        cells: Vec::new(),
        meta: RowMeta::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(text: &str, wrap: bool, wrap_continuation: bool) -> HistoryRow {
        HistoryRow {
            cells: text
                .chars()
                .map(|c| HistoryCell {
                    link: None,
                    codepoint: c as u32,
                    wide: Wide::Narrow,
                    style: Style::DEFAULT,
                    semantic: Semantic::Output,
                    graphemes: Vec::new(),
                })
                .collect(),
            meta: RowMeta {
                wrap,
                wrap_continuation,
                semantic_prompt: RowSemantic::None,
            },
        }
    }

    fn texts(reflowed: &Reflowed) -> Vec<String> {
        reflowed
            .rows
            .iter()
            .map(|row| {
                row.cells
                    .iter()
                    .map(|cell| char::from_u32(cell.codepoint).filter(|c| *c != '\0').unwrap_or(' '))
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn a_wrapped_line_re_splits_at_the_new_width() {
        let rows = [row("abcdefghij", true, false), row("klmno", false, true)];
        let out = resize_rows(&rows, 10, 5, Anchor { row: 1, x: 4 }, None, &[], Mode::Rejoin);
        assert_eq!(texts(&out), ["abcde", "fghij", "klmno"]);
        assert_eq!(out.cursor, Anchor { row: 2, x: 4 });
    }

    #[test]
    fn widening_rejoins_the_rows_of_one_line() {
        let rows = [
            row("abcde", true, false),
            row("fghij", true, true),
            row("klmno", false, true),
        ];
        let out = resize_rows(&rows, 5, 10, Anchor { row: 2, x: 4 }, None, &[], Mode::Rejoin);
        assert_eq!(texts(&out), ["abcdefghij", "klmno"]);
        assert_eq!(out.cursor, Anchor { row: 1, x: 4 });
    }

    #[test]
    fn the_line_extends_to_reach_a_cursor_past_its_content() {
        // The cursor sat one cell past 'o'. That cell has to exist at the new width, which
        // is why an empty continuation row appears under a line that divided evenly.
        let rows = [row("abcdefghij", true, false), row("klmno", false, true)];
        let out = resize_rows(&rows, 10, 5, Anchor { row: 1, x: 5 }, None, &[], Mode::Rejoin);
        assert_eq!(texts(&out), ["abcde", "fghij", "klmno", ""]);
        assert_eq!(out.cursor, Anchor { row: 3, x: 0 });
        assert!(out.rows[2].meta.wrap, "the row above the cursor now wraps");
        assert!(out.rows[3].meta.wrap_continuation);
    }

    #[test]
    fn trailing_blank_rows_are_dropped_but_interior_ones_are_kept() {
        let rows = [
            row("a", false, false),
            row("", false, false),
            row("d", false, false),
            row("", false, false),
        ];
        let out = resize_rows(&rows, 10, 10, Anchor { row: 2, x: 1 }, None, &[], Mode::Rejoin);
        assert_eq!(texts(&out), ["a", "", "d"]);
    }

    #[test]
    fn the_row_the_cursor_sits_on_is_never_dropped_as_blank() {
        let rows = [row("", false, false), row("", false, false)];
        let out = resize_rows(&rows, 10, 10, Anchor { row: 1, x: 0 }, None, &[], Mode::Rejoin);
        assert_eq!(out.rows.len(), 2);
        assert_eq!(out.cursor, Anchor { row: 1, x: 0 });
    }

    #[test]
    fn a_wide_cell_that_cannot_fit_leaves_a_spacer_head() {
        let mut cells = vec![HistoryCell {
            codepoint: u32::from(b'a'),
            wide: Wide::Narrow,
            style: Style::DEFAULT,
            semantic: Semantic::Output,
            graphemes: Vec::new(),
            link: None,
        }];
        cells.push(HistoryCell {
            codepoint: '世' as u32,
            wide: Wide::Wide,
            style: Style::DEFAULT,
            semantic: Semantic::Output,
            graphemes: Vec::new(),
            link: None,
        });
        cells.push(HistoryCell {
            codepoint: 0,
            wide: Wide::SpacerTail,
            style: Style::DEFAULT,
            semantic: Semantic::Output,
            graphemes: Vec::new(),
            link: None,
        });
        let rows = [HistoryRow {
            cells,
            meta: RowMeta::default(),
        }];

        let out = resize_rows(&rows, 3, 2, Anchor { row: 0, x: 0 }, None, &[], Mode::Rejoin);
        assert_eq!(out.rows.len(), 2);
        assert_eq!(out.rows[0].cells[1].wide, Wide::SpacerHead);
        assert_eq!(out.rows[1].cells[0].codepoint, '世' as u32);
        assert_eq!(out.rows[1].cells[1].wide, Wide::SpacerTail);
    }

    #[test]
    fn truncate_mode_keeps_every_row_separate() {
        // The alternate screen: rows lose columns but never rejoin.
        let rows = [row("abcdefghij", true, false), row("klmno", false, true)];
        let out = resize_rows(&rows, 10, 5, Anchor { row: 1, x: 4 }, None, &[], Mode::Truncate);
        assert_eq!(texts(&out), ["abcde", "klmno"]);
        assert!(out.rows[0].meta.wrap, "the flags are carried, not recomputed");
        assert!(out.rows[1].meta.wrap_continuation);
    }

    #[test]
    fn a_grapheme_cluster_travels_with_its_cell() {
        let mut rows = [row("ab", false, false)];
        rows[0].cells[1].graphemes = vec!['\u{301}'];
        let out = resize_rows(&rows, 10, 1, Anchor { row: 0, x: 1 }, None, &[], Mode::Rejoin);
        assert_eq!(out.rows.len(), 2);
        assert_eq!(out.rows[1].cells[0].graphemes, vec!['\u{301}']);
    }
}
