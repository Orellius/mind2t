//! Does a scrolled publish show scrollback?
//!
//! The discriminating assertion everywhere here is text that has left the active grid
//! entirely: a publisher that ignores the offset and republishes the active area cannot
//! contain it, so each test fails against exactly the broken implementation it exists
//! for. There is no oracle for any of this -- libghostty-vt exports no viewport surface
//! at all -- so the viewport is unit- and host-gated the way sixel is, and said loudly.

use mind2t_vt_core::Terminal;
use mind2t_vt_frame::{CLUSTER_BYTES, Frame, FrameReader, Publisher, ReadOutcome, channel};
use mind2t_vt_snapshot::Color;

/// A terminal that wrote `lines` numbered rows through a `rows`-tall screen, so the early
/// ones are in scrollback and the late ones on the grid.
fn scrolled_terminal(cols: u16, rows: u16, lines: usize) -> Terminal {
    let mut terminal = Terminal::with_scrollback(cols, rows, 1000);
    for i in 0..lines {
        if i > 0 {
            terminal.write(b"\r\n");
        }
        terminal.write(format!("line-{i}").as_bytes());
    }
    terminal
}

fn publish_at(
    publisher: &mut Publisher,
    reader: &FrameReader,
    terminal: &mut Terminal,
    offset: u32,
) -> Frame {
    publisher
        .publish_scrolled(terminal, offset)
        .expect("geometry fits the channel");
    let mut frame = Frame::new();
    assert!(matches!(reader.read_into(&mut frame), ReadOutcome::Fresh(_)));
    frame
}

fn frame_text(frame: &Frame) -> Vec<String> {
    let mut scratch = [0u8; CLUSTER_BYTES];
    (0..frame.rows)
        .map(|y| {
            let mut line = String::new();
            for x in 0..frame.cols {
                let cell = frame.cell(x, y);
                if cell.has_text() {
                    line.push_str(cell.cluster(&mut scratch));
                } else if cell.wide() != mind2t_vt_snapshot::Wide::SpacerTail {
                    line.push(' ');
                }
            }
            line.trim_end().to_string()
        })
        .collect()
}

#[test]
fn a_scrolled_frame_shows_rows_the_active_grid_no_longer_holds() {
    // 20 lines through a 4-row screen: the active grid holds line-16..line-19 and can
    // never contain line-10 -- only a real history readout can put it in the frame.
    let mut terminal = scrolled_terminal(20, 4, 20);
    let (writer, reader) = channel(20, 4);
    let mut publisher = Publisher::new(writer);

    let bottom = publish_at(&mut publisher, &reader, &mut terminal, 0);
    assert_eq!(
        frame_text(&bottom),
        ["line-16", "line-17", "line-18", "line-19"],
        "the control: at the bottom the frame is the active grid"
    );
    assert_eq!(bottom.viewport, 0);

    let scrolled = publish_at(&mut publisher, &reader, &mut terminal, 6);
    assert_eq!(
        frame_text(&scrolled),
        ["line-10", "line-11", "line-12", "line-13"],
        "offset 6 on a 4-row screen shows history rows 10..14"
    );
    assert_eq!(scrolled.viewport, 6);
}

#[test]
fn a_partial_scroll_stitches_history_above_the_active_grid() {
    let mut terminal = scrolled_terminal(20, 4, 20);
    let (writer, reader) = channel(20, 4);
    let mut publisher = Publisher::new(writer);

    let frame = publish_at(&mut publisher, &reader, &mut terminal, 2);
    assert_eq!(
        frame_text(&frame),
        ["line-14", "line-15", "line-16", "line-17"],
        "two rows of history, then the top two active rows"
    );
}

#[test]
fn the_offset_clamps_at_the_top_of_history() {
    let mut terminal = scrolled_terminal(20, 4, 20);
    let (writer, reader) = channel(20, 4);
    let mut publisher = Publisher::new(writer);

    let frame = publish_at(&mut publisher, &reader, &mut terminal, 5000);
    // 20 lines on a 4-row screen leave 16 in history; the clamped window starts at line-0.
    assert_eq!(frame.viewport, 16);
    assert_eq!(frame_text(&frame)[0], "line-0");
}

#[test]
fn history_styles_survive_the_scroll_by_value() {
    // The style-table trap: history rows carry styles by VALUE, and the grid's interned
    // table may no longer contain them. A publisher that maps history cells through the
    // active table draws scrolled-back color as default.
    let mut terminal = Terminal::with_scrollback(20, 2, 1000);
    terminal.write(b"\x1b[31mred-line\x1b[0m");
    for _ in 0..6 {
        terminal.write(b"\r\nplain");
    }

    let (writer, reader) = channel(20, 2);
    let mut publisher = Publisher::new(writer);
    let frame = publish_at(&mut publisher, &reader, &mut terminal, 5);

    assert_eq!(frame_text(&frame)[0], "red-line");
    let style = frame.style(frame.cell(0, 0).style_id());
    assert_eq!(
        style.fg,
        Color::Palette(1),
        "the scrolled-back row keeps the red it scrolled off with"
    );
}

#[test]
fn the_cursor_rides_the_shift_and_vanishes_off_the_window() {
    let mut terminal = scrolled_terminal(20, 4, 20);
    let (writer, reader) = channel(20, 4);
    let mut publisher = Publisher::new(writer);

    let bottom = publish_at(&mut publisher, &reader, &mut terminal, 0);
    assert!(bottom.cursor.visible);
    let live_row = bottom.cursor.y;

    // One row up: the cursor's cell is still inside the window, one row lower.
    let nudged = publish_at(&mut publisher, &reader, &mut terminal, 1);
    if usize::from(live_row) + 1 < usize::from(nudged.rows) {
        assert!(nudged.cursor.visible);
        assert_eq!(nudged.cursor.y, live_row + 1);
    }

    // Deep in history the cursor's cell is below the window: published invisible,
    // never drawn on a scrollback row it was never on.
    let deep = publish_at(&mut publisher, &reader, &mut terminal, 10);
    assert!(!deep.cursor.visible);
}

#[test]
fn scrolling_and_returning_both_invalidate_the_whole_frame() {
    // Per-row damage stamps name active-grid rows; under a moved window they point at the
    // wrong screen positions. Both edges of a scroll must therefore repaint everything --
    // a renderer that trusts row stamps across a window move shows stale interleavings.
    let mut terminal = scrolled_terminal(20, 4, 20);
    let (writer, reader) = channel(20, 4);
    let mut publisher = Publisher::new(writer);

    let bottom = publish_at(&mut publisher, &reader, &mut terminal, 0);
    let scrolled = publish_at(&mut publisher, &reader, &mut terminal, 6);
    assert!(
        scrolled.full_generation > bottom.generation,
        "scrolling marked the whole frame changed"
    );

    let returned = publish_at(&mut publisher, &reader, &mut terminal, 0);
    assert!(
        returned.full_generation > scrolled.generation,
        "returning to the bottom marked the whole frame changed too"
    );
}

#[test]
fn an_alternate_screen_has_nothing_to_scroll_into() {
    let mut terminal = scrolled_terminal(20, 4, 20);
    // Enter the alternate screen; its history has a zero budget by construction.
    terminal.write(b"\x1b[?1049halt-content");

    let (writer, reader) = channel(20, 4);
    let mut publisher = Publisher::new(writer);
    let frame = publish_at(&mut publisher, &reader, &mut terminal, 6);

    assert_eq!(frame.viewport, 0, "the offset clamps against an empty history");
    // 1049h keeps the cursor's row, so the text lands wherever the primary left it --
    // the point is only that the frame shows the ALT grid, not history.
    assert!(
        frame_text(&frame).iter().any(|row| row.contains("alt-content")),
        "the frame shows the alternate screen, not scrollback"
    );
}
