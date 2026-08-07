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

#[test]
fn scrolling_shows_rows_the_screen_has_lost() {
    // 40 numbered lines through a 6-row screen; `cat` keeps the child alive so the pump
    // stays in its loop while we scroll. mark-05 left the active grid long ago -- only a
    // real history readout can bring it back.
    let (host, reader) = Host::spawn(
        sh("i=0; while [ $i -lt 40 ]; do echo mark-$i; i=$((i+1)); done; cat"),
        Options::new(40, 6),
    )
    .expect("spawn");

    wait_for(&reader, |frame| text(frame).contains("mark-39"));

    host.scroll(30);
    let scrolled = wait_for(&reader, |frame| frame.viewport > 0);
    assert!(
        text(&scrolled).contains("mark-5"),
        "scrolled 30 rows up, the frame shows the mark-5 region:\n{}",
        text(&scrolled)
    );
    assert!(
        !scrolled.cursor.visible,
        "the cursor's cell is far below the window"
    );

    host.scroll_to_bottom();
    let bottom = wait_for(&reader, |frame| frame.viewport == 0);
    assert!(text(&bottom).contains("mark-39"), "back at the live bottom");
    drop(host);
}

#[test]
fn a_scrolled_view_stays_pinned_to_its_content_while_the_child_prints() {
    // The drift everyone has seen in a lesser terminal: you scroll up to read, output
    // keeps arriving, and the text crawls out from under your eyes. The offset must grow
    // with the pushed rows so the visible window keeps showing the same content.
    let (host, reader) = Host::spawn(
        sh("i=0; while [ $i -lt 30 ]; do echo pin-$i; i=$((i+1)); done; cat"),
        Options::new(40, 6),
    )
    .expect("spawn");

    wait_for(&reader, |frame| text(frame).contains("pin-29"));

    host.scroll(20);
    let scrolled = wait_for(&reader, |frame| frame.viewport == 20);
    let held = text(&scrolled);
    assert!(held.contains("pin-9"), "the window under inspection:\n{held}");

    // `cat` echoes what we type: five more rows scroll into history underneath us.
    host.send(b"one\rtwo\rthree\rfour\rfive\r").expect("send");
    let after = wait_for(&reader, |frame| frame.viewport > 20);
    assert_eq!(
        text(&after),
        held,
        "new output moved the offset, never the content under the reader's eyes"
    );
    drop(host);
}

#[test]
fn a_synchronized_batch_never_shows_half_drawn() {
    // The batch is split across two writes 60ms apart -- separate pty reads, so an
    // ungated pump publishes the partial "hid" frame in between (the mutant with the
    // gate removed was seen to fail exactly there). Gated, the first frame that shows
    // any of the batch shows all of it.
    //
    // **The budget is raised out of the way, and that is the whole point of the option.**
    // With the shipped 150ms this test had a 2.5x margin against its own 60ms gap, which
    // is not a margin at all on a machine somebody else is sharing: GitHub's macOS runner
    // crossed it on 2026-08-08, the budget correctly force-published the half-drawn frame,
    // and a test about the GATE went red for a reason that had nothing to do with the gate
    // and could not be reproduced in 25 consecutive local runs. A test whose verdict
    // depends on how loaded the machine is does not report on the code. The budget's own
    // behaviour is asserted by the test below, which sets its own value for the same
    // reason: two claims, two numbers, neither of them the clock.
    let mut options = Options::new(40, 6);
    options.sync_budget = std::time::Duration::from_secs(30);
    let (host, reader) = Host::spawn(
        sh("printf 'ready\\n'; printf '\\033[?2026hhid'; sleep 0.06; printf 'den\\033[?2026l'; sleep 5"),
        options,
    )
    .expect("spawn");

    wait_for(&reader, |frame| text(frame).contains("ready"));
    let first_of_batch = wait_for(&reader, |frame| text(frame).contains("hid"));
    assert!(
        text(&first_of_batch).contains("hidden"),
        "the first frame showing the batch must show the WHOLE batch:\n{}",
        text(&first_of_batch)
    );
    drop(host);
}

#[test]
fn a_batch_the_child_never_closes_is_forced_out_by_the_budget() {
    // The anti-stuck bound: a child that opens 2026 and walks away cannot freeze the
    // display. The forced frame carries the mode bit, which is how a reader can tell
    // it is looking at a budget-expired batch rather than a closed one.
    //
    // A SHORT budget, deliberately, and it makes this test stronger rather than merely
    // faster. The child never closes the batch at all, so the only thing that can publish
    // `stuck` is the budget expiring -- with 20ms the assertion lands well inside
    // `wait_for`'s patience even on a loaded runner, where the shipped 150ms sat close
    // enough to it that a slow machine could time out and blame the budget for the
    // scheduler. The value is the one thing under test here, so it belongs in the test.
    let mut options = Options::new(40, 6);
    options.sync_budget = std::time::Duration::from_millis(20);
    let (host, reader) = Host::spawn(sh("printf '\\033[?2026hstuck'; sleep 10"), options)
        .expect("spawn");

    let frame = wait_for(&reader, |frame| text(frame).contains("stuck"));
    assert!(
        frame.synchronized_output(),
        "a budget-forced frame still reports the open batch"
    );
    drop(host);
}
