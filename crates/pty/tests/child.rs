//! Real children on a real pseudoterminal.
//!
//! Everything below this crate is tested without any I/O at all, which is the point of the
//! split -- but it also means nothing else in the project can catch a pty that was opened
//! wrong. The controlling-terminal test is the one that matters most: without `setsid` plus
//! `TIOCSCTTY` the child still runs and still prints, so a broken `pre_exec` looks like a
//! working terminal right up until something asks for job control or opens `/dev/tty`.

use std::process::Command;
use std::time::{Duration, Instant};

use ruuah_vt_frame::{CLUSTER_BYTES, Frame, FrameReader};
use ruuah_vt_pty::{Geometry, Host, Options};

const PATIENCE: Duration = Duration::from_secs(5);

/// The frame's visible text, rows joined by newlines.
fn text(frame: &Frame) -> String {
    let mut scratch = [0u8; CLUSTER_BYTES];
    let mut out = String::new();
    for y in 0..frame.rows {
        for x in 0..frame.cols {
            let cell = frame.cell(x, y);
            if cell.has_text() {
                out.push_str(cell.cluster(&mut scratch));
            } else if cell.wide() != ruuah_vt_snapshot::Wide::SpacerTail {
                out.push(' ');
            }
        }
        out.push('\n');
    }
    out
}

/// Reads frames until one satisfies `wanted`, or gives up.
fn wait_for(reader: &FrameReader, wanted: impl Fn(&Frame) -> bool) -> Frame {
    let mut frame = Frame::new();
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        reader.read_into(&mut frame);
        // A read interrupted mid-copy leaves the frame invalid rather than untouched, so it
        // must not be inspected -- this loop used to discard the outcome and ask `wanted`
        // about whatever it was holding.
        if frame.is_valid() && wanted(&frame) {
            return frame;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!(
        "no frame matched within {PATIENCE:?}; last frame was:\n{}",
        text(&frame)
    );
}

fn sh(script: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command.arg("-c").arg(script);
    // A predictable environment: no user rc files, no colour heuristics from the outside.
    command.env("TERM", "xterm-256color");
    command
}

#[test]
fn a_child_s_output_reaches_a_published_frame() {
    let (_host, reader) =
        Host::spawn(sh("printf 'hello from the child'"), Options::new(40, 10)).expect("spawn");

    let frame = wait_for(&reader, |frame| {
        text(frame).contains("hello from the child")
    });
    assert_eq!((frame.cols, frame.rows), (40, 10));
}

#[test]
fn the_pty_is_the_child_s_controlling_terminal() {
    // What the `unsafe` pre_exec block buys. `tty` prints the terminal name when the process
    // has one and "not a tty" when it does not, so this fails loudly if setsid or TIOCSCTTY
    // silently stopped working.
    let (_host, reader) = Host::spawn(sh("tty"), Options::new(40, 5)).expect("spawn");

    let frame = wait_for(&reader, |frame| {
        let seen = text(frame);
        seen.contains("/dev/tty") || seen.contains("not a tty")
    });
    let seen = text(&frame);

    assert!(
        seen.contains("/dev/tty"),
        "the child has no controlling terminal:\n{seen}"
    );
}

#[test]
fn the_child_is_told_the_size_the_pty_was_opened_with() {
    let (_host, reader) = Host::spawn(sh("stty size"), Options::new(97, 31)).expect("spawn");

    // `stty size` prints "rows cols".
    let frame = wait_for(&reader, |frame| text(frame).contains("31 97"));
    assert!(text(&frame).contains("31 97"));
}

#[test]
fn a_resize_reaches_the_child_and_the_frame() {
    // The child blocks on `read` until after the resize has landed, so what it reports is
    // the new size rather than a race with the old one.
    let (host, reader) =
        Host::spawn(sh("read _ignored; stty size"), Options::new(80, 24)).expect("spawn");

    wait_for(&reader, |frame| frame.rows == 24);

    host.resize(Geometry {
        cols: 120,
        rows: 40,
    })
    .expect("resize");

    let resized = wait_for(&reader, |frame| frame.cols == 120 && frame.rows == 40);
    assert_eq!((resized.cols, resized.rows), (120, 40));

    host.send(b"\n").expect("send");
    let frame = wait_for(&reader, |frame| text(frame).contains("40 120"));
    assert!(
        text(&frame).contains("40 120"),
        "the child did not see the new size:\n{}",
        text(&frame)
    );
}

#[test]
fn input_sent_to_the_host_reaches_the_child() {
    let (host, reader) = Host::spawn(
        sh("read line; printf 'got:%s' \"$line\""),
        Options::new(40, 6),
    )
    .expect("spawn");

    wait_for(&reader, |frame| frame.rows == 6);
    host.send(b"knock\n").expect("send");

    let frame = wait_for(&reader, |frame| text(frame).contains("got:knock"));
    assert!(text(&frame).contains("got:knock"));
}

#[test]
fn hebrew_survives_the_pty_as_whole_clusters() {
    // End to end for the north star: bytes leave a child process, cross the pty, go through
    // the parser, and arrive as cells a renderer could draw. Still logical order -- the
    // reordering is slice 5.5 and happens in the run builder, not here.
    // Literal UTF-8 in the script, not `\u` escapes -- `/bin/sh`'s printf does not implement
    // them and emits a backslash instead, which quietly tests nothing.
    let word = "\u{05E9}\u{05C1}\u{05B8}\u{05DC}\u{05D5}\u{05B9}\u{05DD}";
    let (_host, reader) =
        Host::spawn(sh(&format!("printf '%s' '{word}'")), Options::new(20, 3)).expect("spawn");

    let frame = wait_for(&reader, |frame| frame.cell(0, 0).has_text());
    let mut scratch = [0u8; CLUSTER_BYTES];

    assert_eq!(
        frame.cell(0, 0).cluster(&mut scratch),
        "\u{05E9}\u{05C1}\u{05B8}",
        "shin with its dot and vowel is ONE cell"
    );
    assert!(!frame.cell(0, 0).is_truncated());
}

#[test]
fn a_child_that_exits_lets_the_host_reap_it() {
    let (mut host, reader) =
        Host::spawn(sh("printf done; exit 3"), Options::new(20, 3)).expect("spawn");

    wait_for(&reader, |frame| text(frame).contains("done"));

    let deadline = Instant::now() + PATIENCE;
    let status = loop {
        if let Some(status) = host.try_wait().expect("try_wait") {
            break status;
        }
        assert!(Instant::now() < deadline, "the child was never reaped");
        std::thread::sleep(Duration::from_millis(5));
    };

    assert_eq!(status.code(), Some(3));
}

#[test]
fn the_renderer_only_owes_the_rows_the_child_actually_touched() {
    // Damage all the way from a child process to what a renderer would repaint.
    let (host, reader) = Host::spawn(
        sh("printf 'top'; read _ignored; printf '\\033[3;1Hbottom'"),
        Options::new(30, 6),
    )
    .expect("spawn");

    let first = wait_for(&reader, |frame| text(frame).contains("top"));
    let drawn = first.generation;

    host.send(b"\n").expect("send");
    let second = wait_for(&reader, |frame| text(frame).contains("bottom"));

    let stale: Vec<u16> = second.stale_rows(drawn).collect();
    assert!(stale.contains(&2), "the row that was written: {stale:?}");
    assert!(
        !stale.contains(&4) && !stale.contains(&5),
        "rows nothing touched should not be repainted: {stale:?}"
    );
}

#[test]
fn a_resize_past_capacity_is_refused_and_the_display_keeps_updating() {
    // `Options` promises a resize past the channel's capacity is "reported rather than drawn
    // wrong". The audit's finding 16 measured the opposite: the resize succeeded, the pump's
    // publishes started failing into `let _ =`, and the display froze permanently while the
    // child kept drawing. Both halves are asserted: the report, and the display staying live.
    let (host, reader) = Host::spawn(
        sh("read _ignored; printf 'still alive'"),
        Options::new(40, 6),
    )
    .expect("spawn");

    wait_for(&reader, |frame| frame.rows == 6);

    // Default capacity is 400x160; this exceeds both axes.
    let result = host.resize(Geometry {
        cols: 500,
        rows: 200,
    });
    assert!(
        result.is_err(),
        "a resize past the channel's capacity must be reported, not swallowed"
    );

    // The refused resize must leave the pipeline running: later output still becomes frames.
    host.send(b"\n").expect("send");
    let frame = wait_for(&reader, |frame| text(frame).contains("still alive"));
    assert_eq!(
        (frame.cols, frame.rows),
        (40, 6),
        "the refused resize must not have reached the terminal"
    );
}
