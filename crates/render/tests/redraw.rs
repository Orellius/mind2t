//! The blind spot slice 5 step 4 opens: nothing in this project could see a PIXEL.
//!
//! Every test before this one asserts on cells, damage flags and generations. A renderer that
//! draws nothing at all, or stacks every glyph at the origin, or repaints the wrong rows,
//! satisfies all of them. So the harness comes first again, and the invariant it checks is
//! the strongest one available here:
//!
//!   **painting a sequence of frames incrementally must produce the same bytes as painting
//!   the final frame in full.**
//!
//! That single equality covers the whole class: a row that changed without being marked, a
//! row marked but not repainted, stale pixels left behind by a shorter line, a glyph
//! overhanging into a row that is never redrawn. And it is proven able to fail --
//! `a_renderer_that_skips_a_stale_row_is_caught` runs the identical load through a renderer
//! that declines one row and asserts the buffers diverge.

use ruuah_vt_core::Terminal;
use ruuah_vt_frame::{Frame, Publisher, ReadOutcome, channel};
use ruuah_vt_render::{FontStack, Renderer};

const COLS: u16 = 40;
const ROWS: u16 = 12;

/// A sequence chosen to exercise the ways a row can change: plain writes, a cursor moving
/// between rows, colour, an erase, a scroll, and a line getting shorter.
const SCRIPT: &[&[u8]] = &[
    b"first line\r\nsecond line\r\nthird line",
    b"\x1b[1;1H\x1b[31mred now\x1b[0m",
    b"\x1b[2;1Ha much longer second line than before",
    b"\x1b[2;1Hshort\x1b[K",
    b"\x1b[5;1Hdown here",
    b"\x1b[2J\x1b[1;1Hafter a full clear",
    b"\r\nscrolling\r\nscrolling\r\nscrolling\r\nscrolling\r\nscrolling\r\n",
    b"\x1b[7mreverse video\x1b[0m tail",
    b"\x1b[4munderlined\x1b[0m",
];

fn fonts() -> FontStack {
    FontStack::system(16.0).expect("system fonts")
}

/// Runs the script, painting after every step with `paint`, and returns the final canvas
/// bytes alongside a full repaint of the same final frame.
fn incremental_versus_full(
    mut paint: impl FnMut(&mut Renderer, &Frame) -> usize,
) -> (Vec<u8>, Vec<u8>) {
    let mut terminal = Terminal::new(COLS, ROWS);
    let (writer, reader) = channel(COLS, ROWS);
    let mut publisher = Publisher::new(writer);
    let mut frame = Frame::new();
    let mut incremental = Renderer::new(fonts(), COLS, ROWS);

    for step in SCRIPT {
        terminal.write(step);
        publisher.publish(&mut terminal).expect("fits");
        assert!(matches!(
            reader.read_into(&mut frame),
            ReadOutcome::Fresh(_)
        ));
        paint(&mut incremental, &frame);
    }

    let mut full = Renderer::new(fonts(), COLS, ROWS);
    full.draw_all(&frame);

    (
        incremental.canvas().pixels().to_vec(),
        full.canvas().pixels().to_vec(),
    )
}

/// Where two canvases first disagree, as a pixel coordinate, for a legible failure.
fn first_difference(a: &[u8], b: &[u8], width: u32) -> Option<(u32, u32)> {
    a.iter()
        .zip(b.iter())
        .position(|(x, y)| x != y)
        .map(|byte| {
            let pixel = byte as u32 / 4;
            (pixel % width, pixel / width)
        })
}

#[test]
fn an_incremental_redraw_is_byte_identical_to_a_full_one() {
    let (incremental, full) = incremental_versus_full(|renderer, frame| renderer.draw(frame));

    let width = 40 * FontStack::system(16.0).unwrap().metrics().width;
    assert_eq!(incremental.len(), full.len());
    assert_eq!(
        first_difference(&incremental, &full, width),
        None,
        "damage-driven painting disagreed with a full repaint"
    );
}

#[test]
fn a_renderer_that_skips_a_stale_row_is_caught() {
    // The proof the test above can fail. If this ever stops finding a difference, the
    // equality upstairs has stopped being sensitive to damage bugs and certifies nothing.
    let (skipped, full) =
        incremental_versus_full(|renderer, frame| renderer.draw_skipping_for_testing(frame, 1));

    assert_ne!(
        skipped, full,
        "a renderer that never repainted row 1 produced the same pixels as a correct one, so \
         the redraw invariant cannot distinguish them"
    );
}

#[test]
fn a_quiet_frame_costs_no_rows() {
    // Damage-driven means the common case is free. Without this, "incremental equals full"
    // would still pass for a renderer that repaints everything every frame.
    let mut terminal = Terminal::new(COLS, ROWS);
    let (writer, reader) = channel(COLS, ROWS);
    let mut publisher = Publisher::new(writer);
    let mut frame = Frame::new();
    let mut renderer = Renderer::new(fonts(), COLS, ROWS);

    terminal.write(b"something to draw");
    publisher.publish(&mut terminal).expect("fits");
    reader.read_into(&mut frame);
    assert!(renderer.draw(&frame) > 0, "the first frame has work");

    publisher.publish(&mut terminal).expect("fits");
    reader.read_into(&mut frame);
    assert_eq!(
        renderer.draw(&frame),
        0,
        "nothing changed, nothing repainted"
    );
}

#[test]
fn one_written_row_repaints_one_row() {
    let mut terminal = Terminal::new(COLS, ROWS);
    let (writer, reader) = channel(COLS, ROWS);
    let mut publisher = Publisher::new(writer);
    let mut frame = Frame::new();
    let mut renderer = Renderer::new(fonts(), COLS, ROWS);

    terminal.write(b"\x1b[3;1Hparked here");
    publisher.publish(&mut terminal).expect("fits");
    reader.read_into(&mut frame);
    renderer.draw(&frame);

    terminal.write(b"more on the same row");
    publisher.publish(&mut terminal).expect("fits");
    reader.read_into(&mut frame);

    assert_eq!(
        renderer.draw(&frame),
        1,
        "writing within one row must not repaint the screen"
    );
}

#[test]
fn text_actually_puts_ink_on_the_canvas_where_the_cell_is() {
    // The floor: without this, every equality above is satisfied by a renderer that draws
    // nothing whatsoever.
    let mut terminal = Terminal::new(COLS, ROWS);
    let (writer, reader) = channel(COLS, ROWS);
    let mut publisher = Publisher::new(writer);
    let mut frame = Frame::new();
    let mut renderer = Renderer::new(fonts(), COLS, ROWS);

    terminal.write(b"\x1b[2;3HW");
    publisher.publish(&mut terminal).expect("fits");
    reader.read_into(&mut frame);
    renderer.draw(&frame);

    let cell = renderer.canvas().pixels().len();
    assert!(cell > 0);

    let metrics = FontStack::system(16.0).unwrap().metrics();
    let background = renderer.palette().default_background;
    let canvas = renderer.canvas();

    let mut ink_in_the_cell = 0;
    for y in 0..metrics.height {
        for x in 0..metrics.width {
            let px = canvas.pixel(2 * metrics.width + x, metrics.height + y);
            if px != background {
                ink_in_the_cell += 1;
            }
        }
    }
    assert!(
        ink_in_the_cell > 0,
        "a W was written at row 1 column 2 and left no mark"
    );

    // And nowhere else on that row, which catches a glyph drawn at the origin regardless of
    // its column.
    let mut ink_at_the_origin = 0;
    for y in 0..metrics.height {
        for x in 0..metrics.width {
            if canvas.pixel(x, metrics.height + y) != background {
                ink_at_the_origin += 1;
            }
        }
    }
    assert_eq!(
        ink_at_the_origin, 0,
        "something was drawn in column 0 of a row whose only text is in column 2"
    );
}

#[test]
fn hebrew_is_reordered_so_its_last_logical_character_is_leftmost() {
    // Flipped deliberately in slice 5.5. Until then this asserted the opposite -- that column
    // 0 held the FIRST logical character -- and reordering made it fail, which was the whole
    // point of writing it that way round.
    //
    // Decided by pixels rather than by eye, because eyeballing a Hebrew screenshot is exactly
    // how you convince yourself of the wrong thing: the reader's eye runs right to left and
    // the renderer does not. Rendering the whole word and rendering one character in
    // isolation, then comparing a single column, is a claim a person cannot fool themselves
    // about.
    let word = "\u{05E9}\u{05DC}\u{05D5}\u{05DD}";
    let first: String = word.chars().next().unwrap().to_string();
    let last: String = word.chars().next_back().unwrap().to_string();

    let column_zero = |content: &str| -> Vec<u8> {
        let mut terminal = Terminal::new(COLS, ROWS);
        let (writer, reader) = channel(COLS, ROWS);
        let mut publisher = Publisher::new(writer);
        let mut frame = Frame::new();
        let mut renderer = Renderer::new(fonts(), COLS, ROWS);

        terminal.write(content.as_bytes());
        terminal.write(b"\x1b[?25l"); // the cursor would otherwise sit on the cell we sample
        publisher.publish(&mut terminal).expect("fits");
        reader.read_into(&mut frame);
        renderer.draw(&frame);

        let metrics = fonts().metrics();
        let canvas = renderer.canvas();
        (0..metrics.height)
            .flat_map(|y| (0..metrics.width).map(move |x| (x, y)))
            .flat_map(|(x, y)| canvas.pixel(x, y))
            .collect()
    };

    assert_eq!(
        column_zero(word),
        column_zero(&last),
        "the leftmost column of a Hebrew word must hold its LAST logical character"
    );
    assert_ne!(
        column_zero(word),
        column_zero(&first),
        "the word is not reordered at all -- this is the pre-5.5 behaviour returning"
    );
}

#[test]
fn hebrew_reaches_the_canvas_through_the_fallback_font() {
    // Not reordered -- that is 5.5. This asserts only that the fallback path survives all the
    // way to pixels, which is the piece that would otherwise be discovered broken in 5.5.
    let mut terminal = Terminal::new(COLS, ROWS);
    let (writer, reader) = channel(COLS, ROWS);
    let mut publisher = Publisher::new(writer);
    let mut frame = Frame::new();
    let mut renderer = Renderer::new(fonts(), COLS, ROWS);

    terminal.write("\u{05E9}\u{05DC}\u{05D5}\u{05DD}".as_bytes());
    publisher.publish(&mut terminal).expect("fits");
    reader.read_into(&mut frame);
    renderer.draw(&frame);

    let metrics = FontStack::system(16.0).unwrap().metrics();
    let background = renderer.palette().default_background;
    let canvas = renderer.canvas();

    for column in 0..4u32 {
        let mut ink = 0;
        for y in 0..metrics.height {
            for x in 0..metrics.width {
                if canvas.pixel(column * metrics.width + x, y) != background {
                    ink += 1;
                }
            }
        }
        assert!(ink > 0, "column {column} of a Hebrew word is blank");
    }
}
