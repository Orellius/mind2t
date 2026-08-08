//! Unicode placeholders, at the pixel layer.
//!
//! The decoder's own rules are unit-tested in `frame/src/placeholder.rs`; nothing there
//! can see whether the right PART of the image reaches the right cell, which is the half
//! that goes wrong invisibly. A renderer that drew the whole image for every run would
//! satisfy every decoder test and put the top-left corner in all four quadrants.
//!
//! So the image under test is a four-colour quadrant chart, and each assertion names both
//! the cell and the colour that must be in it. Any crop error swaps a colour.

use std::sync::Arc;

use mind2t_vt_core::Terminal;
use mind2t_vt_frame::{Frame, Publisher, ReadOutcome, channel};
use mind2t_vt_render::{FontStack, Renderer};

const COLS: u16 = 6;
const ROWS: u16 = 3;

fn fonts() -> FontStack {
    FontStack::system(16.0).expect("system fonts")
}

/// 2x2 image: red, green / blue, white. Scaled to a 2x2 CELL box, each quadrant is one
/// cell, so "which quadrant is in this cell" answers "was the crop right".
fn quadrants() -> (u32, u32, Arc<Vec<u8>>) {
    #[rustfmt::skip]
    let pixels = vec![
        255, 0, 0, 255,   0, 255, 0, 255,
        0, 0, 255, 255,   255, 255, 255, 255,
    ];
    (2, 2, Arc::new(pixels))
}

/// Registers a 2x2-cell virtual placement for image `id`, then prints `text`.
fn painted(id: u32, text: &str) -> Vec<u8> {
    let mut terminal = Terminal::new(COLS, ROWS);
    let (writer, reader) = channel(COLS, ROWS);
    let mut publisher = Publisher::new(writer);
    let mut frame = Frame::new();
    let mut renderer = Renderer::new(fonts(), COLS, ROWS);

    // a=T with U=1: transmit and register a VIRTUAL placement, 2x2 cells. Nothing is
    // drawn by this command -- the placeholder cells below are what put it on screen.
    terminal.write(
        format!("\x1b_Ga=T,f=32,s=2,v=2,i={id},c=2,r=2,U=1;AAAA/wAAAP8AAAD/AAAA/w==\x1b\\")
            .as_bytes(),
    );
    terminal.write(text.as_bytes());

    publisher.publish(&mut terminal).expect("fits");
    assert!(matches!(
        reader.read_into(&mut frame),
        ReadOutcome::Fresh(_)
    ));

    renderer.draw_all(&frame);
    let (w, h, rgba) = quadrants();
    renderer.draw_placeholders(&frame, |image| {
        (image == id).then(|| (w, h, Arc::clone(&rgba)))
    });
    renderer.canvas().pixels().to_vec()
}

/// The colour at the middle of cell (col, row).
fn cell_colour(canvas: &[u8], col: u16, row: u16) -> (u8, u8, u8) {
    let metrics = fonts().metrics();
    let x = u32::from(col) * metrics.width + metrics.width / 2;
    let y = u32::from(row) * metrics.height + metrics.height / 2;
    let base = ((y * (u32::from(COLS) * metrics.width) + x) * 4) as usize;
    (canvas[base], canvas[base + 1], canvas[base + 2])
}

/// Which quadrant a sampled cell shows, named rather than matched exactly.
///
/// The image is SCALED to the cell box with a bilinear filter, so the middle of a cell
/// carries a little of its neighbour: the red quadrant samples as (243, 12, 0), not
/// (255, 0, 0). Exact equality failed three of these tests on a renderer that was working
/// correctly. Naming the dominant channel keeps every crop error visible -- each quadrant
/// is a different name -- without asserting a filter's arithmetic.
fn quadrant(canvas: &[u8], col: u16, row: u16) -> &'static str {
    let (r, g, b) = cell_colour(canvas, col, row);
    match (r > 180, g > 180, b > 180) {
        (true, false, false) => "red",
        (false, true, false) => "green",
        (false, false, true) => "blue",
        (true, true, true) => "white",
        _ => "none",
    }
}

/// A placeholder cell naming row 0 column 0 must show the image's top-left quadrant, and
/// the cell beside it -- continuing the run with no diacritics -- the top-right.
#[test]
fn a_run_draws_the_cells_it_names() {
    let canvas = painted(42, "\x1b[38;5;42m\u{10EEEE}\u{0305}\u{0305}\u{10EEEE}");

    assert_eq!(quadrant(&canvas, 0, 0), "red", "cell 0 is the top-left quadrant");
    assert_eq!(quadrant(&canvas, 1, 0), "green", "cell 1 is the top-right one");
}

/// THE CROP TEST. The second screen row names image row 1, so it must show the BOTTOM
/// half. A renderer that draws the whole image per run puts red and green here too and
/// fails both assertions -- and that renderer passes every decoder unit test.
#[test]
fn the_second_row_shows_the_bottom_half_not_the_top() {
    let canvas = painted(
        42,
        "\x1b[38;5;42m\u{10EEEE}\u{0305}\u{0305}\u{10EEEE}\r\n\u{10EEEE}\u{030D}\u{0305}\u{10EEEE}",
    );

    assert_eq!(quadrant(&canvas, 0, 0), "red", "top-left stays red");
    assert_eq!(
        quadrant(&canvas, 0, 1),
        "blue",
        "image row 1 column 0 is the BLUE quadrant, not the red one above it"
    );
    assert_eq!(quadrant(&canvas, 1, 1), "white", "image row 1 column 1 is white");
}

/// A placeholder naming an image that was never transmitted draws nothing rather than
/// drawing whatever image happens to be in the store.
#[test]
fn an_unknown_image_draws_nothing() {
    let canvas = painted(42, "\x1b[38;5;99m\u{10EEEE}\u{0305}\u{0305}");
    assert_eq!(quadrant(&canvas, 0, 0), "none");
}

/// The feature's entire point: the cells ARE the image, so scrolling the text scrolls the
/// picture with it, with no anchor to keep in step. One newline past the bottom of a
/// 3-row screen moves the placeholder run up one row, and the pixels must follow.
#[test]
fn a_placeholder_image_scrolls_with_its_text() {
    let before = painted(42, "\x1b[38;5;42m\u{10EEEE}\u{0305}\u{0305}");
    assert_eq!(quadrant(&before, 0, 0), "red");

    // Fill past the last row so the grid scrolls by one.
    let after = painted(42, "\x1b[38;5;42m\u{10EEEE}\u{0305}\u{0305}\r\n\r\n\r\n");
    assert_ne!(
        quadrant(&after, 0, 0),
        "red",
        "the run left row 0 when the screen scrolled"
    );
    assert_eq!(
        cell_colour(&after, 0, 0),
        cell_colour(&after, 5, 2),
        "and row 0 is now ordinary blank background"
    );
}

/// A stray U+10EEEE in text with no image colour must not summon an image. The default
/// foreground is not image 0's id, it is "no id at all".
#[test]
fn a_placeholder_without_a_colour_is_inert() {
    let canvas = painted(42, "\u{10EEEE}\u{0305}\u{0305}");
    assert_eq!(quadrant(&canvas, 0, 0), "none");
}
