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
//!
//! `a_publish_landing_mid_copy_leaves_no_frame_claiming_to_be_valid` asserts a second and
//! different invariant, because self-consistency provably cannot see the bug it exists for:
//! an interrupted copy usually finishes with clean content wearing the wrong generation, and
//! every cell agrees with every other one. What that test asks instead is whether the frame
//! was TOUCHED, which a skipped read is not allowed to do without invalidating it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use mind2t_vt_frame::{Frame, FrameReader, PackedCell, ReadOutcome, channel};
use mind2t_vt_snapshot::{Semantic, Wide};

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
    // Liveness, and it is deliberately NOT part of the invariant. `accepted > 0` says the
    // consistency check was actually exercised rather than skipped away, and on a two-core
    // shared runner with the writer publishing flat out the reader can lose the whole 400ms
    // window without landing a single read -- which failed CI on 2026-08-07 with "every read
    // was skipped", a complaint about the machine wearing the words of a complaint about the
    // code. So the window is EXTENDED while nothing has been accepted, up to a hard ceiling,
    // and the assertion still fires if the reader genuinely never lands one. The workload is
    // untouched: same writer, same race, same check. Only the control is excluded, because it
    // never tallies an accepted read and would run to the ceiling every time.
    let ceiling = Instant::now() + RUN_FOR * 25;

    while Instant::now() < deadline
        || (synchronized && tally.accepted == 0 && Instant::now() < ceiling)
    {
        if synchronized {
            match reader.read_into(&mut frame) {
                ReadOutcome::Fresh(_) => tally.accepted += 1,
                ReadOutcome::Skipped => {
                    tally.skipped += 1;
                    // A skipped read used to `continue` straight past this check, which is
                    // exactly why the frame it leaves behind went unexamined for two slices.
                    // The copy runs before the counter is re-read, so a publish landing
                    // mid-copy overwrites the caller's frame; if that frame still claimed a
                    // generation, it would be a mixture of two publishes wearing the identity
                    // of one. Either it is untouched and still consistent, or it is invalid.
                    if frame.is_valid()
                        && let Some(complaint) = inconsistency(&frame)
                    {
                        stop.store(true, Ordering::Relaxed);
                        panic!("a skipped read left a torn frame still claiming to be valid: {complaint}");
                    }
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
    let mut scratch = [0u8; mind2t_vt_frame::CLUSTER_BYTES];
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

/// A publish landing mid-copy must not leave the caller holding a mixture of two frames.
///
/// The copy runs before the counter is re-read, so the frame is already overwritten by the
/// time the read decides to skip. Without invalidating it there, it keeps the generation of
/// the publish before last and looks entirely fresh.
///
/// This needs its own workload, and the reason is measured. Under `hammer` the writer holds
/// the counter odd almost continuously, so reads bail at the first check and never copy at
/// all: 6,802,136 skips in one 400ms run, of which **zero** were this path. Slowing the writer
/// to a realistic cadence found exactly 1 in four million. So the timing is orchestrated
/// instead -- a large grid to make the copy slow, and a writer that publishes just after the
/// reader enters it, which converts a one-in-a-million race into roughly one attempt in two.
#[test]
fn a_publish_landing_mid_copy_leaves_no_frame_claiming_to_be_valid() {
    const WIDE: u16 = 1000;
    const TALL: u16 = 300;
    const ATTEMPTS: usize = 40;

    let (writer, reader) = channel(WIDE, TALL);
    let writer = Arc::new(std::sync::Mutex::new(writer));
    let mut frame = Frame::new();

    let fill = |stamp: u16| PackedCell::new("a", stamp, Wide::Narrow, Semantic::Output);
    writer
        .lock()
        .expect("uncontended")
        .publish(WIDE, TALL, |f| {
            for y in 0..TALL {
                for x in 0..WIDE {
                    f.cell(x, y, fill(1));
                }
                f.row_changed(y);
            }
        })
        .expect("fits");
    assert!(matches!(reader.read_into(&mut frame), ReadOutcome::Fresh(_)));

    let mut invalidated = 0;
    for attempt in 0..ATTEMPTS {
        let go = Arc::new(AtomicBool::new(false));
        let their_writer = Arc::clone(&writer);
        let their_go = Arc::clone(&go);
        let stamp = (attempt % 100 + 2) as u16;
        let scribe = thread::spawn(move || {
            while !their_go.load(Ordering::Acquire) {}
            // Long enough for the reader to be inside the copy, short enough to land before
            // it finishes. Measured at roughly a 50% hit rate on this grid.
            thread::sleep(Duration::from_micros(60));
            their_writer
                .lock()
                .expect("the reader never takes this lock")
                .publish(WIDE, TALL, |f| {
                    for y in 0..TALL {
                        for x in 0..WIDE {
                            f.cell(x, y, fill(stamp));
                        }
                        f.row_changed(y);
                    }
                })
                .expect("fits");
        });

        // The fingerprint is what makes this exact. Asking whether the frame is torn is the
        // wrong question: most interrupted copies here finish cleanly and are merely wearing
        // the wrong generation, which no cell-against-cell check can see. Asking whether the
        // frame CHANGED catches both, because a skipped read is only allowed to leave a frame
        // it did not touch.
        let before = frame.cell(0, 0).style_id();
        go.store(true, Ordering::Release);
        let outcome = reader.read_into(&mut frame);
        scribe.join().expect("writer thread panicked");

        if outcome == ReadOutcome::Skipped {
            let touched = frame.cell(0, 0).style_id() != before;
            assert!(
                !(touched && frame.is_valid()),
                "attempt {attempt}: a skipped read overwrote the frame (publish {before} -> {}) \
                 and left it claiming generation {}; a caller that ignores the outcome draws \
                 that as if it were fresh",
                frame.cell(0, 0).style_id(),
                frame.generation
            );
            if touched {
                invalidated += 1;
            }
        }
        if !frame.is_valid() {
            let _ = reader.read_into(&mut frame);
        }
    }

    assert!(
        invalidated > 0,
        "no read was interrupted mid-copy in {ATTEMPTS} attempts, so this test asserted \
         nothing; the orchestration has stopped working and needs re-measuring"
    );
    assert!(
        !frame.is_valid() || frame.generation != 0,
        "sanity: a valid frame carries a non-zero generation"
    );
}

/// A panic inside the fill closure strands the counter odd -- readers skip and keep the last
/// good frame, which is the honest state for a half-written buffer. What must NOT happen is
/// the audit's finding 17: a writer that recovers and publishes again bumping the counter
/// blindly (`start + 1`), which lands it EVEN during the recovery fill and ODD at completion.
/// From that point every torn mid-fill read wears a valid generation and every completed
/// frame reads as in-flight -- the protocol permanently inverted, silently.
#[test]
fn a_publish_after_a_panicked_publish_restores_the_protocol() {
    let (mut writer, reader) = channel(4, 2);
    let mut frame = Frame::new();

    let first = writer
        .publish(4, 2, |f| {
            f.cell(0, 0, PackedCell::new("a", 0, Wide::Narrow, Semantic::Output))
        })
        .expect("fits");
    assert_eq!(reader.read_into(&mut frame), ReadOutcome::Fresh(first));

    let died = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = writer.publish(4, 2, |_f| panic!("fill died mid-publish"));
    }));
    assert!(died.is_err(), "the panicking fill must actually panic");

    // Unrecovered, the channel refuses to serve the half-written frame.
    assert_eq!(reader.read_into(&mut frame), ReadOutcome::Skipped);

    // The recovery publish must come back as a readable frame, not an inverted counter.
    let recovered = writer
        .publish(4, 2, |f| {
            f.cell(0, 0, PackedCell::new("b", 0, Wide::Narrow, Semantic::Output))
        })
        .expect("fits");
    assert_eq!(
        reader.read_into(&mut frame),
        ReadOutcome::Fresh(recovered),
        "the publish after a panicked one must be readable -- an odd completed \
         generation means the parity inverted"
    );
    let mut scratch = [0u8; mind2t_vt_frame::CLUSTER_BYTES];
    assert_eq!(frame.cell(0, 0).cluster(&mut scratch), "b");
}
