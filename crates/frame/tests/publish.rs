//! Does a published frame say the same thing the core does?
//!
//! The seqlock tests prove the handoff is atomic; these prove it carries the right cargo.
//! The reference is the core's own `Snapshot`, which the differential corpus has already
//! measured against libghostty-vt -- so agreeing with it here inherits that evidence rather
//! than restating expected cell contents by hand.

use ruuah_vt_core::Terminal;
use ruuah_vt_frame::{CLUSTER_BYTES, Frame, FrameReader, Publisher, ReadOutcome, channel};

fn published(terminal: &mut Terminal, cols: u16, rows: u16) -> (Frame, FrameReader) {
    let (writer, reader) = channel(cols, rows);
    let mut publisher = Publisher::new(writer);
    publisher.publish(terminal).expect("fits");

    let mut frame = Frame::new();
    assert!(matches!(
        reader.read_into(&mut frame),
        ReadOutcome::Fresh(_)
    ));
    (frame, reader)
}

/// The frame's rows as text, in the same shape `Snapshot::row_text` produces.
fn frame_text(frame: &Frame) -> Vec<String> {
    let mut scratch = [0u8; CLUSTER_BYTES];
    (0..frame.rows)
        .map(|y| {
            let mut line = String::new();
            for x in 0..frame.cols {
                let cell = frame.cell(x, y);
                if cell.has_text() {
                    line.push_str(cell.cluster(&mut scratch));
                } else if cell.wide() != ruuah_vt_snapshot::Wide::SpacerTail {
                    line.push(' ');
                }
            }
            line.trim_end().to_string()
        })
        .collect()
}

#[test]
fn a_published_frame_carries_the_same_text_the_core_reports() {
    let mut terminal = Terminal::new(20, 3);
    terminal.write(b"hello\r\nworld");

    let snapshot = terminal.snapshot();
    let (frame, _reader) = published(&mut terminal, 20, 3);

    assert_eq!(frame_text(&frame)[0], snapshot.row_text(0));
    assert_eq!(frame_text(&frame)[1], snapshot.row_text(1));
    assert_eq!((frame.cols, frame.rows), (20, 3));
}

#[test]
fn styles_survive_the_crossing_by_id() {
    let mut terminal = Terminal::new(10, 1);
    terminal.write(b"\x1b[1;31mred\x1b[0m.");

    let snapshot = terminal.snapshot();
    let (frame, _reader) = published(&mut terminal, 10, 1);

    for x in 0..4u16 {
        assert_eq!(
            frame.style(frame.cell(x, 0).style_id()),
            snapshot.grid[0].cells[usize::from(x)].style,
            "column {x}"
        );
    }
    assert!(frame.style(frame.cell(0, 0).style_id()).bold);
    assert!(frame.style(frame.cell(3, 0).style_id()).is_default());
}

#[test]
fn a_hebrew_cluster_with_niqqud_arrives_as_one_cell() {
    // The north star, end to end: bytes into the core, one frame out, four codepoints still
    // in a single cell. Nothing here is reordered -- that is slice 5.5 and it happens in the
    // run builder, not in this path.
    let mut terminal = Terminal::new(8, 1);
    terminal.write("\u{05D1}\u{05BC}\u{05B8}\u{05DF}".as_bytes());

    let (frame, _reader) = published(&mut terminal, 8, 1);
    let mut scratch = [0u8; CLUSTER_BYTES];

    assert_eq!(
        frame.cell(0, 0).cluster(&mut scratch),
        "\u{05D1}\u{05BC}\u{05B8}"
    );
    assert_eq!(frame.cell(1, 0).cluster(&mut scratch), "\u{05DF}");
    assert!(!frame.cell(0, 0).is_truncated());
}

#[test]
fn a_wide_cell_keeps_its_spacer_and_its_columns() {
    let mut terminal = Terminal::new(6, 1);
    terminal.write("\u{4F60}\u{597D}".as_bytes());

    let (frame, _reader) = published(&mut terminal, 6, 1);
    let runs: Vec<_> = frame.runs(0).collect();

    assert_eq!(frame.cell(0, 0).wide(), ruuah_vt_snapshot::Wide::Wide);
    assert_eq!(frame.cell(1, 0).wide(), ruuah_vt_snapshot::Wide::SpacerTail);
    assert_eq!(runs.iter().map(|r| r.width()).sum::<u16>(), 6);
}

#[test]
fn only_the_rows_that_changed_come_back_stale() {
    let mut terminal = Terminal::new(10, 4);
    terminal.write(b"one\r\ntwo\r\nthree");

    let (writer, reader) = channel(10, 4);
    let mut publisher = Publisher::new(writer);
    let mut frame = Frame::new();

    let first = publisher.publish(&mut terminal).expect("fits");
    reader.read_into(&mut frame);
    assert_eq!(frame.stale_rows(0).count(), 3, "three rows were written");

    // Nothing touched since the last publish consumed the damage.
    let second = publisher.publish(&mut terminal).expect("fits");
    reader.read_into(&mut frame);
    assert_eq!(frame.stale_rows(first).count(), 0);

    // Row 0 is written, and row 2 is where the cursor was parked. Both are damaged: a
    // cursor move dirties the row it LEFT as well as the row it arrived at, which is the
    // rule step 2 measured against upstream. The point of this assertion is that the other
    // row -- row 1, untouched and uncrossed -- stays clean.
    terminal.write(b"\x1b[1;1Hx");
    publisher.publish(&mut terminal).expect("fits");
    reader.read_into(&mut frame);
    let stale: Vec<u16> = frame.stale_rows(second).collect();
    assert_eq!(
        stale,
        vec![0, 2],
        "written row and vacated cursor row, not row 1"
    );
}

#[test]
fn a_renderer_that_missed_frames_still_owes_every_row_touched_in_between() {
    // The reason rows carry a stamp instead of a flag. Three publishes each touch a
    // different row; a renderer that drew none of them repaints all three.
    let mut terminal = Terminal::new(10, 4);
    let (writer, reader) = channel(10, 4);
    let mut publisher = Publisher::new(writer);
    let mut frame = Frame::new();

    publisher.publish(&mut terminal).expect("fits");
    reader.read_into(&mut frame);
    let drawn = frame.generation;

    for row in 1..=3 {
        terminal.write(format!("\x1b[{row};1Hx").as_bytes());
        publisher.publish(&mut terminal).expect("fits");
    }

    reader.read_into(&mut frame);
    let stale: Vec<u16> = frame.stale_rows(drawn).collect();
    assert_eq!(
        stale,
        vec![0, 1, 2],
        "every row touched while it was not looking"
    );
}

#[test]
fn a_whole_frame_event_outranks_the_row_stamps() {
    let mut terminal = Terminal::new(10, 3);
    terminal.write(b"content");

    let (writer, reader) = channel(10, 3);
    let mut publisher = Publisher::new(writer);
    let mut frame = Frame::new();

    let drawn = publisher.publish(&mut terminal).expect("fits");
    reader.read_into(&mut frame);

    // ED 2 is one of the four whole-frame triggers the core measured against upstream.
    terminal.write(b"\x1b[2J");
    publisher.publish(&mut terminal).expect("fits");
    reader.read_into(&mut frame);

    assert!(frame.full_generation > drawn);
    assert_eq!(frame.stale_rows(drawn).count(), 3, "all of them");
}

#[test]
fn the_cursor_crosses_with_its_position_and_its_pen() {
    let mut terminal = Terminal::new(10, 3);
    terminal.write(b"\x1b[2;4H\x1b[4mabc");

    let snapshot = terminal.snapshot();
    let (frame, _reader) = published(&mut terminal, 10, 3);

    assert_eq!(frame.cursor.x, snapshot.cursor.x);
    assert_eq!(frame.cursor.y, snapshot.cursor.y);
    assert_eq!(frame.cursor.visible, snapshot.cursor.visible);
    assert_eq!(frame.cursor.style, snapshot.cursor.style);
}

#[test]
fn a_soft_wrap_flag_reaches_the_frame() {
    let mut terminal = Terminal::new(4, 3);
    terminal.write(b"abcdef");

    let (frame, _reader) = published(&mut terminal, 4, 3);
    assert!(frame.wraps(0), "row 0 wrapped into row 1");
    assert!(!frame.wraps(1));
}
