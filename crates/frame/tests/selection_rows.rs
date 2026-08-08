//! The bridge D2b's gesture stands on: a FRAME can answer the selection rules.
//!
//! `mind2t_vt_core::selection` is gated differentially against libghostty-vt, but it reads
//! `mind2t_vt_snapshot::Row` and a host holds a `Frame` of packed cells. Without this bridge the
//! host's only options were to reach the core's scrollback across a thread it does not own, or
//! to re-derive the word rules from packed cells - a second copy of a set of boundary
//! codepoints with no oracle behind it.
//!
//! So the question these cases ask is not "are the selection rules right" (the corpus owns
//! that) but "does a frame reproduce the grid faithfully enough for those rules to reach the
//! same answer they reach on the core's own rows". Every case here therefore runs the SAME
//! probe against BOTH shapes and demands they agree - which is what makes them able to fail.
//! A `viewport_rows` that returned blanks, transposed x and y, or dropped the wrap flags would
//! be caught by the disagreement rather than by an assertion someone had to predict.

use mind2t_vt_core::Terminal;
use mind2t_vt_core::selection::{format, select_line, select_word};
use mind2t_vt_frame::{Frame, Publisher, ReadOutcome, channel};
use mind2t_vt_snapshot::{Point, Row};

const COLS: u16 = 20;
const ROWS: u16 = 4;

/// Writes `bytes` and returns the frame's rows beside the core's own, for the same grid.
fn both(bytes: &[u8]) -> (Vec<Row>, Vec<Row>) {
    let mut terminal = Terminal::new(COLS, ROWS);
    let (writer, reader) = channel(COLS, ROWS);
    let mut publisher = Publisher::new(writer);
    let mut frame = Frame::new();

    terminal.write(bytes);
    publisher.publish(&mut terminal).expect("fits");
    assert!(matches!(reader.read_into(&mut frame), ReadOutcome::Fresh(_)));

    // `grid` is the ACTIVE area, which is exactly what a frame carries. History is deliberately
    // left out of both sides: a frame has none, and comparing against a core that does would
    // measure the missing scrollback rather than the bridge.
    let core: Vec<Row> = terminal.snapshot().grid;
    (frame.viewport_rows(), core)
}

#[test]
fn a_word_selects_the_same_range_from_a_frame_as_from_the_core() {
    let (framed, core) = both(b"cargo test --workspace");
    let at = Point { x: 2, y: 0 };

    let from_frame = select_word(&framed, COLS, at);
    let from_core = select_word(&core, COLS, at);

    assert_eq!(from_frame, from_core, "the frame and the core disagree about the word");
    let found = from_frame.expect("a word under a letter");
    assert_eq!((found.start.x, found.start.y), (0, 0));
    assert_eq!(found.end.x, 4, "cargo ends at column 4");
}

#[test]
fn a_hyphenated_flag_selects_whole_because_the_rules_come_from_the_oracle() {
    // The single most useful thing double-click does in a terminal, and the reason the bridge
    // must reuse the core rules rather than any reasonable-looking local ones: `-` is NOT a
    // word boundary, so `--workspace` is one word. A host that split on punctuation would feel
    // wrong within a minute of use and no test that only checked "some range came back" would
    // notice.
    let (framed, _) = both(b"cargo test --workspace");
    let found = select_word(&framed, COLS, Point { x: 14, y: 0 }).expect("a word");
    assert_eq!(
        format(&framed, COLS, &found),
        "--workspace",
        "the double hyphen belongs to the word"
    );
}

#[test]
fn a_blank_cell_has_no_word_and_that_is_an_answer() {
    // Not an error, and the reason the gesture treats `false` as "leave the selection alone"
    // rather than as a failure to report.
    let (framed, core) = both(b"hi");
    let at = Point { x: 10, y: 2 };
    assert_eq!(select_word(&framed, COLS, at), None);
    assert_eq!(select_word(&core, COLS, at), None, "and the core agrees");
}

#[test]
fn a_line_selection_from_a_frame_matches_the_core_and_trims_the_trailing_blanks() {
    let (framed, core) = both(b"one\r\ntwo three\r\n");
    let at = Point { x: 1, y: 1 };

    assert_eq!(select_line(&framed, COLS, at), select_line(&core, COLS, at));
    let found = select_line(&framed, COLS, at).expect("a line");
    assert_eq!(
        format(&framed, COLS, &found),
        "two three",
        "the seventeen blank cells after the text are not part of the line"
    );
}

#[test]
fn a_soft_wrapped_word_stays_one_word_across_the_row_boundary() {
    // The case that fails if `viewport_rows` drops the wrap flags - and it fails SILENTLY in
    // the direction that looks fine: the selection simply stops at the right edge, which is
    // what a person would expect a naive terminal to do, so nobody reports it as a bug.
    // Twenty columns exactly, so the word crosses without a space.
    let (framed, core) = both(b"aaaaaaaaaaaaaaaaaaaabbbb");
    let at = Point { x: 2, y: 0 };

    let from_frame = select_word(&framed, COLS, at).expect("a word");
    assert_eq!(Some(from_frame.clone()), select_word(&core, COLS, at));
    assert_eq!(from_frame.end.y, 1, "the word runs onto the wrapped row");
    assert_eq!(
        format(&framed, COLS, &from_frame),
        "aaaaaaaaaaaaaaaaaaaabbbb",
        "and joins without the newline the grid stores it with"
    );
}

#[test]
fn the_frame_reports_the_wrap_flags_the_publisher_wrote() {
    // The direct reading of the same fact, because the test above proves it only through a
    // selection: a bridge that hardcoded `wrap: false` would pass every non-wrapping case here.
    let (framed, _) = both(b"aaaaaaaaaaaaaaaaaaaabbbb");
    assert!(framed[0].wrap, "row 0 soft-wraps into row 1");
    assert!(framed[1].wrap_continuation, "row 1 continues row 0");
    assert!(!framed[1].wrap, "and row 1 does not wrap onward");
}

#[test]
fn a_wide_glyphs_spacer_tail_is_not_selectable_through_the_frame_either() {
    // The property `has_text` in the core rules depends on, carried across the bridge: a packed
    // cell's tail must arrive as a tail and not as a blank that happens to look like one.
    let (framed, core) = both("\u{4f60}\u{597d}".as_bytes());
    let tail = Point { x: 1, y: 0 };
    assert_eq!(select_word(&framed, COLS, tail), None);
    assert_eq!(select_word(&core, COLS, tail), None, "and the core agrees");

    // And the consequence, measured rather than predicted - my first version of this case
    // asserted the pair selects as one word and it does not. A tail carries no text, so it is a
    // boundary, so every wide glyph is its own word. Both shapes agree on that, which is the
    // claim this file is entitled to make; whether the RULE is right is the oracle's to say and
    // the corpus already asks it.
    let head = Point { x: 0, y: 0 };
    let from_frame = select_word(&framed, COLS, head).expect("the head selects");
    assert_eq!(Some(from_frame.clone()), select_word(&core, COLS, head));
    assert_eq!(format(&framed, COLS, &from_frame), "\u{4f60}");
}
