//! Purpose: cut one logical line's cells into rows of a given width.
//! Public surface: `Split`, `split`.
//! Why this file: `reflow.rs` answers "which rows form a line and what does the line
//!   contain"; this answers "where does that content break". The second grew its own state
//!   -- tracked cursor offsets and, since slice 5.6, the source-row boundary each output row
//!   ended on -- and together the two broke the 500-line ceiling.
//! NOT responsible for: deciding what a line is, or what metadata an output row inherits
//!   (`reflow.rs` owns both, and the semantic rule it applies is measured there).
//! Test strategy: exercised through `reflow.rs`'s tests and the resize corpus cases.

use ruuah_vt_snapshot::RowSemantic;

use crate::cell::Wide;
use crate::page::HistoryCell;

use crate::reflow::blank_cell;

pub(crate) struct Split {
    pub(crate) rows: Vec<Vec<HistoryCell>>,
    /// Where each tracked offset landed, as (row within this line, column).
    pub(crate) tracked: [Option<(usize, u16)>; 2],
    /// One entry per row: the offset into `content` just past the last cell that row took.
    /// This is what lets a destination row be traced back to the source row that filled it,
    /// which the semantic prompt mark needs and no other field does.
    pub(crate) content_end: Vec<usize>,
}

/// Cuts a line's cells into rows of `new_cols`, keeping wide cells whole.
pub(crate) fn split(content: &[HistoryCell], new_cols: u16, offsets: [Option<usize>; 2]) -> Split {
    let width = usize::from(new_cols);
    let mut rows: Vec<Vec<HistoryCell>> = Vec::new();
    let mut current: Vec<HistoryCell> = Vec::new();
    let mut tracked: [Option<(usize, u16)>; 2] = [None, None];
    let mut content_end: Vec<usize> = Vec::new();

    let mut index = 0;
    while index < content.len() {
        let cell = &content[index];
        if cell.wide == Wide::SpacerHead {
            index += 1;
            continue;
        }

        let needed = if cell.wide == Wide::Wide { 2 } else { 1 };
        if current.len() + needed > width {
            // A wide cell that cannot fit leaves the last column spoken for, exactly as
            // printing one at the right edge does.
            if needed == 2 && current.len() < width {
                current.push(HistoryCell {
                    wide: Wide::SpacerHead,
                    ..blank_cell()
                });
            }
            rows.push(std::mem::take(&mut current));
            content_end.push(index);
        }

        mark(&mut tracked, offsets, index, rows.len(), current.len());
        current.push(cell.clone());

        if needed == 2 {
            index += 1;
            let tail = match content.get(index) {
                Some(cell) if cell.wide == Wide::SpacerTail => cell.clone(),
                _ => HistoryCell {
                    wide: Wide::SpacerTail,
                    ..blank_cell()
                },
            };
            mark(&mut tracked, offsets, index, rows.len(), current.len());
            current.push(tail);
        }
        index += 1;
    }

    if !current.is_empty() || rows.is_empty() {
        rows.push(current);
        content_end.push(content.len());
    }

    Split {
        rows,
        tracked,
        content_end,
    }
}

/// The mark a destination row inherits: the one belonging to the last source row that
/// reached it. `end` is exclusive, so a source row starting exactly at `end` began the next
/// destination row and does not count.
pub(crate) fn mark_for(marks: &[(usize, RowSemantic)], end: usize) -> RowSemantic {
    marks
        .iter()
        .rev()
        .find(|(start, _)| *start < end)
        .map(|(_, mark)| *mark)
        .unwrap_or_default()
}

fn mark(
    tracked: &mut [Option<(usize, u16)>; 2],
    offsets: [Option<usize>; 2],
    index: usize,
    row: usize,
    column: usize,
) {
    for slot in 0..2 {
        if offsets[slot] == Some(index) {
            tracked[slot] = Some((row, column as u16));
        }
    }
}
