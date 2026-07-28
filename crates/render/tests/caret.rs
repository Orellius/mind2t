//! The blind spot slice 5.6 step 3 opens: nothing could see WHERE THE CARET LANDS.
//!
//! `tests/redraw.rs` proves an incremental repaint equals a full one, and it does so with the
//! caret painted either way -- the invariant holds whether the caret sits on the right cell or
//! the wrong one, because both renderers are consistently wrong. The bidi tests cover where
//! the TEXT goes and say nothing about the cursor.
//!
//! So on a reordered row the caret could sit on a glyph the program never addressed and every
//! existing test would pass. It looks entirely reasonable on screen too: the caret is on a
//! real character, just not that one.
//!
//! The caret is located by measurement rather than by comparing whole canvases: each row is
//! painted twice, once with the cursor visible and once hidden, and the columns whose pixels
//! differ are the columns the caret occupies. That answers "where is it" directly instead of
//! "is it the same as some other render", which was the first version of this file and was
//! wrong -- the caret redraws its own cell's glyph inside the block, so two carets at the same
//! column still differ pixel for pixel when they name different cells.
//!
//! `set_visual_caret_for_testing(false)` restores the old logical placement, and the
//! requirement is two-directional: the two must AGREE on Latin and DISAGREE on Hebrew. A
//! mapping that had quietly become the identity function would pass the first and fail the
//! second.

use ruuah_vt_core::Terminal;
use ruuah_vt_frame::{Frame, Publisher, ReadOutcome, channel};
use ruuah_vt_render::{FontStack, Renderer};

const COLS: u16 = 12;
const ROWS: u16 = 2;

fn fonts() -> FontStack {
    FontStack::system(16.0).expect("system fonts")
}

/// Paints `bytes` with the cursor parked at logical column `x`, and returns the canvas.
fn painted(bytes: &[u8], x: u16, visible: bool, visual_caret: bool) -> Vec<u8> {
    let mut terminal = Terminal::new(COLS, ROWS);
    let (writer, reader) = channel(COLS, ROWS);
    let mut publisher = Publisher::new(writer);
    let mut frame = Frame::new();
    let mut renderer = Renderer::new(fonts(), COLS, ROWS);
    renderer.set_visual_caret_for_testing(visual_caret);

    terminal.write(bytes);
    if !visible {
        terminal.write(b"\x1b[?25l");
    }
    // CUP is 1-based.
    terminal.write(format!("\x1b[1;{}H", x + 1).as_bytes());

    publisher.publish(&mut terminal).expect("fits");
    assert!(matches!(
        reader.read_into(&mut frame),
        ReadOutcome::Fresh(_)
    ));
    renderer.draw_all(&frame);
    renderer.canvas().pixels().to_vec()
}

/// The screen columns the caret occupies, found by diffing a visible caret against a hidden
/// one. Returns the columns rather than a single one so a caret straddling a wide cell, or a
/// caret that somehow painted in two places, is visible rather than silently reduced.
fn caret_columns(bytes: &[u8], x: u16, visual_caret: bool) -> Vec<u16> {
    let shown = painted(bytes, x, true, visual_caret);
    let hidden = painted(bytes, x, false, visual_caret);
    assert_eq!(shown.len(), hidden.len());

    let cell_width = FontStack::system(16.0)
        .expect("system fonts")
        .metrics()
        .width;
    let row_bytes = shown.len() / (ROWS as usize);

    let mut columns = Vec::new();
    for (index, (a, b)) in shown.iter().zip(hidden.iter()).enumerate() {
        if a == b {
            continue;
        }
        // Only the first text row carries the caret in these cases.
        if index >= row_bytes {
            continue;
        }
        let pixel = (index % row_bytes) / 4;
        let column = (pixel as u32 % (cell_width * u32::from(COLS))) / cell_width;
        let column = column as u16;
        if !columns.contains(&column) {
            columns.push(column);
        }
    }
    columns.sort_unstable();
    columns
}

#[test]
fn a_latin_caret_sits_at_the_column_its_index_names() {
    for x in 0..4 {
        assert_eq!(
            caret_columns(b"abcd", x, true),
            vec![x],
            "the caret should be at screen column {x}"
        );
    }
}

/// The feature. `שלום` reverses, so the cell the program calls column 0 is painted at the
/// right-hand end of the run and the caret has to be there with it.
#[test]
fn a_hebrew_caret_follows_its_cell_to_the_other_end() {
    for (logical, expected) in [(0u16, 3u16), (1, 2), (2, 1), (3, 0)] {
        assert_eq!(
            caret_columns("שלום".as_bytes(), logical, true),
            vec![expected],
            "logical column {logical} should paint at screen column {expected}"
        );
    }
}

/// The control, in both directions. On Latin it must be indistinguishable from the real path,
/// or it is scrambling something unrelated and the Hebrew half proves nothing; on Hebrew it
/// must disagree, or the mapping is the identity function.
#[test]
fn the_control_agrees_on_latin_and_disagrees_on_hebrew() {
    for x in 0..4 {
        assert_eq!(
            caret_columns(b"abcd", x, true),
            caret_columns(b"abcd", x, false),
            "column {x} moved on a row with nothing to reorder"
        );
    }

    assert_eq!(
        caret_columns("שלום".as_bytes(), 0, false),
        vec![0],
        "the old behaviour draws at the logical column, which is the bug this replaces"
    );
    assert_ne!(
        caret_columns("שלום".as_bytes(), 0, true),
        caret_columns("שלום".as_bytes(), 0, false),
    );
}

/// A mixed row is where an off-by-one in the run walk shows up: the Latin half stays put while
/// the Hebrew half reverses, and both happen in the same row.
#[test]
fn a_mixed_row_moves_only_its_right_to_left_half() {
    // "ab גד": columns 0..2 Latin, 3..4 Hebrew.
    assert_eq!(caret_columns("ab גד".as_bytes(), 0, true), vec![0]);
    assert_eq!(caret_columns("ab גד".as_bytes(), 1, true), vec![1]);
    assert_eq!(caret_columns("ab גד".as_bytes(), 3, true), vec![4]);
    assert_eq!(caret_columns("ab גד".as_bytes(), 4, true), vec![3]);
}
