//! The blind spot slice 5 step 3 opens: nothing in this project could see a concurrency bug.
//!
//! A seqlock whose reader ignores the counter, or whose writer forgets to mark the publish
//! in flight, passes every single-threaded test ever written against it. So the harness comes
//! first, and it has to be shown capable of failing before it is allowed to certify anything:
//! `a_reader_that_ignores_the_counter_does_observe_torn_frames` runs the identical load
//! through a reader with the protocol removed and asserts it DOES catch the frame mid-write.
//! Without that control, the passing test below is indistinguishable from a test that cannot
//! fail.
//!
//! The invariant is self-consistency. Every cell of frame N carries N in its style slot and
//! every row stamp is N, so a frame assembled from two publishes disagrees with itself and
//! nothing else has to be known about it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use ruuah_vt_frame::{Frame, FrameReader, PackedCell, ReadOutcome, channel};
use ruuah_vt_snapshot::{Semantic, Wide};

const COLS: u16 = 120;
const ROWS: u16 = 40;
/// Long enough that a scheduler has to interleave the two threads, short enough to stay a
/// unit test. The reader's outcome is asserted, never the timing.
const RUN_FOR: Duration = Duration::from_millis(400);

#[derive(Debug, Default)]
struct Tally {
    accepted: u64,
    skipped: u64,
    torn: u64,
}

/// A frame disagrees with itself if two cells claim different publishes, or a row stamp does
/// not match the generation the frame was read at.
fn inconsistency(frame: &Frame) -> Option<String> {
    let expected = frame.cell(0, 0).style_id();
    for y in 0..frame.rows {
        for x in 0..frame.cols {
            let found = frame.cell(x, y).style_id();
            if found != expected {
                return Some(format!(
                    "cell ({x},{y}) is from publish {found} but (0,0) is from {expected}"
                ));
            }
        }
        if !frame.row_is_stale(y, 0) {
            return Some(format!("row {y} carries no stamp at all"));
        }
    }
    None
}

/// Runs one writer flat out against one reader, and reports what the reader saw.
///
/// `synchronized` selects the real protocol or the control that skips it.
fn hammer(synchronized: bool) -> Tally {
    let (mut writer, reader) = channel(COLS, ROWS);
    let stop = Arc::new(AtomicBool::new(false));
    let published = Arc::new(AtomicU64::new(0));

    let writer_stop = Arc::clone(&stop);
    let writer_published = Arc::clone(&published);
    let scribe = thread::spawn(move || {
        let mut stamp: u16 = 1;
        while !writer_stop.load(Ordering::Relaxed) {
            let cell = PackedCell::new("x", stamp, Wide::Narrow, Semantic::Output);
            writer
                .publish(COLS, ROWS, |frame| {
                    for y in 0..ROWS {
                        for x in 0..COLS {
                            frame.cell(x, y, cell);
                        }
                        frame.row_changed(y);
                    }
                })
                .expect("the channel was built at exactly this size");
            writer_published.fetch_add(1, Ordering::Relaxed);
            stamp = stamp.wrapping_add(1).max(1);
        }
    });

    let tally = observe(&reader, synchronized, &stop);
    stop.store(true, Ordering::Relaxed);
    scribe.join().expect("writer thread panicked");

    assert!(
        published.load(Ordering::Relaxed) > 0,
        "the writer never published, so nothing was tested"
    );
    tally
}

fn observe(reader: &FrameReader, synchronized: bool, stop: &AtomicBool) -> Tally {
    let mut frame = Frame::new();
    let mut tally = Tally::default();
    let deadline = Instant::now() + RUN_FOR;

    while Instant::now() < deadline {
        if synchronized {
            match reader.read_into(&mut frame) {
                ReadOutcome::Fresh(_) => tally.accepted += 1,
                ReadOutcome::Skipped => {
                    tally.skipped += 1;
                    continue;
                }
                ReadOutcome::Unchanged => continue,
            }
        } else {
            reader.read_into_unsynchronized_for_testing(&mut frame);
        }

        if let Some(complaint) = inconsistency(&frame) {
            tally.torn += 1;
            if synchronized {
                stop.store(true, Ordering::Relaxed);
                panic!("the reader accepted a torn frame: {complaint}");
            }
            // The control has made its point; no need to keep hammering.
            break;
        }
    }

    tally
}

#[test]
fn the_reader_never_accepts_a_frame_the_writer_was_still_writing() {
    let tally = hammer(true);

    assert_eq!(tally.torn, 0, "checked inside the loop, restated here");
    assert!(
        tally.accepted > 0,
        "every read was skipped, so consistency was never actually exercised"
    );
}

#[test]
fn a_reader_that_ignores_the_counter_does_observe_torn_frames() {
    // The proof that the test above can fail. If this one ever goes quiet, the invariant has
    // stopped being sensitive to tearing and the passing test upstairs is worthless -- fix
    // this before trusting that one. It races by construction, so it is written to stop at
    // the first violation rather than to measure a rate.
    let tally = hammer(false);

    assert!(
        tally.torn > 0,
        "a reader with the protocol removed never caught the writer mid-publish, so the \
         invariant cannot distinguish a working seqlock from a broken one"
    );
}

#[test]
fn a_publish_larger_than_the_buffer_is_refused_rather_than_truncated() {
    let (mut writer, reader) = channel(10, 4);
    let mut frame = Frame::new();

    writer
        .publish(10, 4, |f| {
            f.cell(0, 0, PackedCell::new("a", 0, Wide::Narrow, Semantic::Output))
        })
        .expect("fits");
    assert!(matches!(
        reader.read_into(&mut frame),
        ReadOutcome::Fresh(_)
    ));

    let refused = writer.publish(11, 4, |f| {
        f.cell(0, 0, PackedCell::new("z", 0, Wide::Narrow, Semantic::Output))
    });
    let error = refused.expect_err("11 columns do not fit a 10 column buffer");
    assert_eq!(error.wanted, (11, 4));
    assert_eq!(error.capacity, (10, 4));

    // And the frame that was already there survived intact.
    let mut scratch = [0u8; ruuah_vt_frame::CLUSTER_BYTES];
    assert!(matches!(
        reader.read_into(&mut frame),
        ReadOutcome::Unchanged
    ));
    assert_eq!(frame.cell(0, 0).cluster(&mut scratch), "a");
}

#[test]
fn an_unread_frame_reports_fresh_once_and_unchanged_after() {
    let (mut writer, reader) = channel(4, 2);
    let mut frame = Frame::new();

    let generation = writer
        .publish(4, 2, |f| {
            f.cell(0, 0, PackedCell::new("q", 0, Wide::Narrow, Semantic::Output))
        })
        .expect("fits");

    assert_eq!(reader.read_into(&mut frame), ReadOutcome::Fresh(generation));
    assert_eq!(reader.read_into(&mut frame), ReadOutcome::Unchanged);

    writer
        .publish(4, 2, |f| {
            f.cell(1, 0, PackedCell::new("r", 0, Wide::Narrow, Semantic::Output))
        })
        .expect("fits");
    assert!(matches!(
        reader.read_into(&mut frame),
        ReadOutcome::Fresh(_)
    ));
}
