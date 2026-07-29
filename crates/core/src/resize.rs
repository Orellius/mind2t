//! Purpose: apply a new geometry to a screen -- the storage round-trip around a reflow.
//! Public surface: `apply`.
//! Why this file: `reflow.rs` is a pure transform over rows and knows nothing about where
//!   rows live. This is the half that does: it drains the scrollback and the active area
//!   into one sequence, reflows it, and splits the result back. That split is the whole
//!   reason the two halves are separate files -- the transform is worth testing without a
//!   `Screen`, and this part is worth reading without the transform in the way.
//! NOT responsible for: how a line re-splits or where the cursor lands within it
//!   (`reflow.rs`), or the scrollback budget (`history.rs` prunes on push).
//! Test strategy: the differential corpus. Every rule here was measured against
//!   libghostty-vt on 2026-07-28 and each has a corpus case named `reflow-*`.

use crate::grid::Grid;
use crate::reflow::{Anchor, Mode, resize_rows};
use crate::screen::{SavedCursor, Screen};

/// Resizes `screen`, reflowing its contents and mapping both cursors through the transform.
///
/// The scrollback takes part: a logical line straddling the boundary between history and the
/// active area is one line and rejoins as one. Everything is therefore drained into a single
/// row sequence and split back afterwards -- the active area is the last `rows` of the
/// result, and whatever is pushed above it becomes history.
///
/// The scroll region is reset rather than clamped: DECSTBM margins are set against a screen
/// of a known height, and carrying them across a resize would hand the program a region it
/// never asked for. The oracle agrees.
pub fn apply(screen: &mut Screen, cols: u16, rows: u16, mode: Mode) {
    if cols == 0 || rows == 0 {
        return;
    }

    let old_cols = screen.cols();

    // A rows-only change cannot reflow anything: with the width unchanged there is nothing
    // to rejoin, and rejoining anyway would let the cursor's line gain a row it never had.
    // Upstream routes this case to its no-reflow path for the same reason.
    let mode = if cols == old_cols { Mode::Truncate } else { mode };

    let mut input = screen.history.drain();
    let history_len = input.len();
    for y in 0..screen.rows() {
        input.push(screen.grid.extract_row(y));
    }

    // Placement anchors ride the same transform as the cursors, one Anchor per
    // placement. A negative row (image partially scrolled off the top) indexes into
    // the history rows just drained -- the same coordinate space, naturally. Anchors
    // pointing before the drained sequence are unmappable and their placements drop.
    let placements = std::mem::take(&mut screen.placements);
    let mut extra_anchors: Vec<Anchor> = Vec::new();
    let mut extra_sources: Vec<crate::graphics::Placement> = Vec::new();
    for placement in placements {
        let absolute = history_len as isize + isize::from(placement.row);
        if absolute >= 0 && (absolute as usize) < input.len() {
            extra_anchors.push(Anchor {
                row: absolute as usize,
                x: placement.col,
            });
            extra_sources.push(placement);
        }
    }

    let reflowed = resize_rows(
        &input,
        old_cols,
        cols,
        Anchor {
            row: history_len + usize::from(screen.y),
            x: screen.x,
        },
        screen.saved.map(|saved| Anchor {
            row: history_len + usize::from(saved.y),
            x: saved.x,
        }),
        &extra_anchors,
        mode,
    );

    let overflow = reflowed.rows.len().saturating_sub(usize::from(rows));
    screen.history.set_cols(cols);
    for row in &reflowed.rows[..overflow] {
        screen.history.push(row.clone());
    }

    screen.grid = Grid::new(cols, rows);
    for (y, row) in reflowed.rows[overflow..].iter().enumerate() {
        screen.grid.write_row(y as u16, row);
    }

    // A cursor whose row was pushed above the active area cannot follow it, so it clamps to
    // the top of what remains.
    screen.y = clamp_row(reflowed.cursor.row, overflow, rows);
    screen.x = reflowed.cursor.x.min(cols - 1);

    // The saved cursor in the same position has nowhere sensible to point at all, so it goes
    // to the top-left. Upstream does the same and says outright that any behaviour is fine
    // here because the case is unspecified.
    screen.saved = screen.saved.map(|saved| match reflowed.saved {
        Some(anchor) if anchor.row >= overflow => SavedCursor {
            x: anchor.x.min(cols - 1),
            y: clamp_row(anchor.row, overflow, rows),
            pending_wrap: saved.pending_wrap,
        },
        _ => SavedCursor {
            x: 0,
            y: 0,
            pending_wrap: false,
        },
    });

    // Mapped placements come back in the reflowed row space; the active area starts
    // at `overflow`, so anything above it lands at a negative row and keeps drawing
    // its clipped remainder, exactly as a scroll would have left it.
    for (source, mapped) in extra_sources.into_iter().zip(reflowed.extras) {
        let Some(anchor) = mapped else { continue };
        let row = anchor.row as isize - overflow as isize;
        if row > -256 && row < rows as isize {
            screen.placements.push(crate::graphics::Placement {
                image: source.image,
                col: anchor.x,
                row: row as i16,
                cols: source.cols,
                rows: source.rows,
            });
        }
    }

    screen.reset_scroll_region();
}

fn clamp_row(row: usize, overflow: usize, rows: u16) -> u16 {
    u16::try_from(row.saturating_sub(overflow))
        .unwrap_or(u16::MAX)
        .min(rows - 1)
}
