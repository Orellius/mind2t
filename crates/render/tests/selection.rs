//! The blind spot D2b opens: NOTHING in this renderer could see a selected cell.
//!
//! D2a built the selection model and gated it against libghostty-vt, so the RANGE and the
//! clipboard text are measured. None of that puts a pixel on screen. Until this file existed a
//! `draw_selection` that returned immediately - or tinted the wrong columns, or the wrong row,
//! or every row - passed `redraw.rs`, `caret.rs`, `vim.rs` and the whole corpus, because every
//! one of them either never sets a selection or compares two renders that are wrong the same
//! way.
//!
//! The measurement is positional and two-directional, in the shape `caret.rs` established:
//! paint the same frame twice, once with a selection and once without, and the columns whose
//! pixels differ ARE the highlighted columns. That answers "where is it" rather than "is it
//! the same as some other render", which is the question a whole-canvas comparison answers and
//! it is the wrong one.

use mind2t_vt_core::Terminal;
use mind2t_vt_frame::{Frame, FrameSelection, Publisher, ReadOutcome, channel};
use mind2t_vt_render::{FontStack, Renderer};

const COLS: u16 = 12;
const ROWS: u16 = 3;

fn fonts() -> FontStack {
    FontStack::system(16.0).expect("system fonts")
}

fn cell_width() -> usize {
    fonts().metrics().width as usize
}

/// Paints `bytes` with `selection` applied, and returns the canvas bytes.
fn painted(bytes: &[u8], selection: Option<FrameSelection>) -> Vec<u8> {
    let mut terminal = Terminal::new(COLS, ROWS);
    let (writer, reader) = channel(COLS, ROWS);
    let mut publisher = Publisher::new(writer);
    let mut frame = Frame::new();
    let mut renderer = Renderer::new(fonts(), COLS, ROWS);

    // Hidden, so the caret cannot contribute a difference that gets read as a highlight.
    terminal.write(b"\x1b[?25l");
    terminal.write(bytes);

    publisher.publish(&mut terminal).expect("fits");
    assert!(matches!(reader.read_into(&mut frame), ReadOutcome::Fresh(_)));
    frame.selection = selection;
    renderer.draw_all(&frame);
    renderer.canvas().pixels().to_vec()
}

/// The columns of row `y` whose pixels changed when the selection was applied.
fn tinted_columns(bytes: &[u8], selection: FrameSelection, y: u16) -> Vec<u16> {
    let plain = painted(bytes, None);
    let selected = painted(bytes, Some(selection));
    assert_eq!(plain.len(), selected.len());

    let metrics = fonts().metrics();
    let stride = usize::from(COLS) * metrics.width as usize * 4;
    let top = usize::from(y) * metrics.height as usize;

    let mut columns = Vec::new();
    for column in 0..COLS {
        let mut differs = false;
        for row in top..top + metrics.height as usize {
            for x in 0..cell_width() {
                let at = row * stride + (usize::from(column) * cell_width() + x) * 4;
                if plain[at..at + 4] != selected[at..at + 4] {
                    differs = true;
                    break;
                }
            }
            if differs {
                break;
            }
        }
        if differs {
            columns.push(column);
        }
    }
    columns
}

#[test]
fn a_selection_tints_exactly_its_own_columns() {
    let selection = FrameSelection { start: (2, 0), end: (5, 0) };
    let tinted = tinted_columns(b"hello world", selection, 0);
    assert_eq!(tinted, vec![2, 3, 4, 5], "the highlight must cover the range and nothing else");
}

/// The half a range-only check cannot see: a highlight that paints the right columns of the
/// WRONG row looks perfect on the row it was asked about.
#[test]
fn a_selection_leaves_every_other_row_untouched() {
    let selection = FrameSelection { start: (2, 1), end: (5, 1) };
    assert!(tinted_columns(b"one\r\ntwo\r\nthree", selection, 0).is_empty());
    assert_eq!(tinted_columns(b"one\r\ntwo\r\nthree", selection, 1), vec![2, 3, 4, 5]);
    assert!(tinted_columns(b"one\r\ntwo\r\nthree", selection, 2).is_empty());
}

/// A multi-row selection runs to the end of every row it passes through. That is what makes it
/// a TEXT selection rather than a rectangle, and it is the case a per-row implementation that
/// reuses the start and end columns on every row gets wrong.
#[test]
fn a_multi_row_selection_covers_whole_middle_rows() {
    let selection = FrameSelection { start: (8, 0), end: (2, 2) };
    let bytes = b"aaaaaaaaaaaa\r\nbbbbbbbbbbbb\r\ncccccccccccc";

    assert_eq!(tinted_columns(bytes, selection, 0), vec![8, 9, 10, 11], "from the start to EOL");
    assert_eq!(
        tinted_columns(bytes, selection, 1),
        (0..COLS).collect::<Vec<u16>>(),
        "a middle row is covered end to end"
    );
    assert_eq!(tinted_columns(bytes, selection, 2), vec![0, 1, 2], "to the end column");
}

/// Endpoint order is the gesture's, not the reader's: a drag upward produces `start` after
/// `end` and must highlight the same cells as the same drag downward.
#[test]
fn a_selection_dragged_backwards_highlights_the_same_cells() {
    let forward = FrameSelection { start: (2, 0), end: (5, 0) };
    let backward = FrameSelection { start: (5, 0), end: (2, 0) };
    assert_eq!(
        tinted_columns(b"hello world", forward, 0),
        tinted_columns(b"hello world", backward, 0)
    );
}

/// The tint must leave the text READABLE, which is the whole reason it is blended over the row
/// rather than painted as a background. Measured: under the highlight the ink is still clearly
/// darker than the tint around it, so a selected line is highlighted and not redacted.
#[test]
fn text_survives_under_the_tint() {
    let selection = FrameSelection { start: (0, 0), end: (4, 0) };
    let selected = painted(b"HHHHH", Some(selection));

    let metrics = fonts().metrics();
    let stride = usize::from(COLS) * metrics.width as usize * 4;
    let mut darkest = 255u8;
    let mut lightest = 0u8;
    for row in 0..metrics.height as usize {
        for x in 0..cell_width() * 5 {
            let at = row * stride + x * 4;
            darkest = darkest.min(selected[at]);
            lightest = lightest.max(selected[at]);
        }
    }
    assert!(
        lightest - darkest > 40,
        "ink and tint must stay distinguishable, saw {darkest}..{lightest}"
    );
}

/// An empty or degenerate selection paints nothing. The zero-width case is what a click with
/// no drag produces, and it happens on every single click.
#[test]
fn a_selection_off_the_bottom_of_the_grid_paints_nothing() {
    let selection = FrameSelection { start: (0, 9), end: (4, 9) };
    for y in 0..ROWS {
        assert!(tinted_columns(b"hello", selection, y).is_empty());
    }
}
