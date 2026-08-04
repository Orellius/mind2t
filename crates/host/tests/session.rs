//! The Rust-native `Session` entry, proven at the level the C surface is proven at: PIXELS.
//!
//! The blind spot this closes is total. `Session::poll` returns a bool, and a session that
//! never advances, a renderer that draws nothing, and a child that produced no output are
//! indistinguishable from each other through that bool - all three simply keep returning
//! `false`, which is also what a perfectly healthy quiet terminal returns.
//!
//! So the assertion is on the drawn buffer, and it is two-directional in the same file: a child
//! that PRINTS must put ink on the surface, and a child that prints NOTHING must not. The
//! second is the control. Without it, "there is more than one colour here" would pass on a
//! renderer that painted a stripe of garbage, and would keep passing if the child were never
//! spawned at all.

use std::collections::HashSet;
use std::process::Command;
use std::time::{Duration, Instant};

use ruuah_vt_host::session::{Session, SessionGeometry};

const GEOMETRY: SessionGeometry = SessionGeometry { cols: 24, rows: 6 };

fn spawn(script: &str) -> Session {
    let mut command = Command::new("/bin/sh");
    command.arg("-c").arg(script);
    Session::spawn(command, GEOMETRY, 16.0, None).expect("a session")
}

/// Polls until the session draws something, or the deadline passes. Returns whether it drew.
///
/// Bounded by a deadline inside the loop rather than by an external timeout: a killer that
/// terminates the process skips every destructor, and the child on the pty is exactly the kind
/// of resource whose cleanup must run (SCAR-016).
fn pump(session: &mut Session, at_least: usize, budget: Duration) -> usize {
    let deadline = Instant::now() + budget;
    let mut draws = 0;
    while Instant::now() < deadline {
        if session.poll() {
            draws += 1;
            if draws >= at_least {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    draws
}

/// How many distinct colours the surface holds PAST THE FIRST CELL.
///
/// The first cell is excluded because the CARET lives there on a fresh terminal, and the caret
/// is ink. Measured, not assumed: the first version of this helper counted the whole surface,
/// and the control below failed with two colours on a child that printed nothing - which is the
/// control earning its place. A whole-surface count cannot tell a drawn glyph from a drawn
/// cursor, so it would have passed on a renderer that drew no text at all.
///
/// Counting colours rather than comparing against a known ink value keeps the assertion
/// independent of the palette and of whichever glyph the shell happened to produce.
fn colours_past_the_caret(session: &mut Session) -> usize {
    let cell = session.cell_metrics();
    let width = cell.width * u32::from(GEOMETRY.cols);
    let pixels = session.pixels();
    pixels
        .chunks_exact(4)
        .enumerate()
        .filter(|(index, _)| (*index as u32 % width) >= cell.width)
        .map(|(_, pixel)| [pixel[0], pixel[1], pixel[2], pixel[3]])
        .collect::<HashSet<_>>()
        .len()
}

#[test]
fn a_child_that_prints_puts_ink_on_the_surface() {
    // `printf` rather than `echo`: no trailing newline to scroll the grid, and no shell
    // builtin differences between /bin/sh implementations.
    let mut session = spawn("printf 'RUUAH'; sleep 1");
    let draws = pump(&mut session, 1, Duration::from_secs(5));
    assert!(draws > 0, "the session never drew a frame");
    let seen = colours_past_the_caret(&mut session);
    assert!(
        seen > 1,
        "a child printed text and the surface is a flat fill of {seen} colour(s)"
    );
}

/// The control. Same pipeline, same polling, same assertion - and it must NOT hold.
///
/// This is what makes the test above evidence: if `colours` counted more than one on a blank
/// terminal (a renderer drawing noise, a stale buffer, an uninitialised surface), the positive
/// test would pass for a reason that has nothing to do with the child's output.
#[test]
fn a_child_that_prints_nothing_leaves_the_surface_flat() {
    let mut session = spawn("sleep 1");
    pump(&mut session, 1, Duration::from_millis(500));
    let seen = colours_past_the_caret(&mut session);
    assert_eq!(
        seen, 1,
        "a silent child left {seen} colours on the surface; the ink assertion cannot distinguish"
    );
}

#[test]
fn sent_bytes_reach_the_child_and_come_back_as_pixels() {
    // `cat` echoes through the line discipline, so bytes written here are drawn without the
    // child interpreting them - the same round trip the C surface's `send` test uses.
    let mut session = spawn("cat");
    session.send(b"XYZ").expect("send");
    let draws = pump(&mut session, 1, Duration::from_secs(5));
    assert!(draws > 0, "nothing was drawn after sending bytes");
    assert!(
        colours_past_the_caret(&mut session) > 1,
        "bytes were sent to the child and the surface stayed flat"
    );
}

#[test]
fn the_visible_grid_reads_back_as_text() {
    let mut session = spawn("printf 'RUUAH'; sleep 1");
    pump(&mut session, 1, Duration::from_secs(5));
    assert!(
        session.visible_text().contains("RUUAH"),
        "the child printed RUUAH and the grid reads {:?}",
        session.visible_text()
    );

    // The control, in the same test because it is the same claim: a silent child's grid must
    // NOT contain it. Without this, a reader that returned the child's raw bytes, or a
    // constant, or anything containing everything, would satisfy the line above.
    let mut silent = spawn("sleep 1");
    pump(&mut silent, 1, Duration::from_millis(500));
    assert!(
        !silent.visible_text().contains("RUUAH"),
        "a silent child's grid claims to hold text it never printed"
    );
}

/// Drains events until the session holds a cwd, or the deadline passes.
///
/// Draining is what advances it - `Session::cwd` is a reader with no side effect - so a test
/// that polled frames alone would wait forever on a report that had already arrived.
fn wait_for_cwd(session: &mut Session, budget: Duration) -> Option<String> {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        session.take_events();
        if let Some(cwd) = session.cwd() {
            return Some(cwd.to_string());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    session.cwd().map(str::to_string)
}

#[test]
fn an_osc_7_report_becomes_the_sessions_cwd() {
    // The host name is deliberately present and deliberately not this machine's: `normalize`
    // discards it, and a session that kept it would answer `localhost/tmp/...`.
    let mut session = spawn("printf '\\033]7;file://localhost/tmp/ruuah-cwd\\a'; sleep 2");
    assert_eq!(
        wait_for_cwd(&mut session, Duration::from_secs(5)).as_deref(),
        Some("/tmp/ruuah-cwd"),
        "the child reported a directory and the session did not learn it"
    );
}

/// The control, and the clear rule in one test.
///
/// A session that simply assigned every report would end holding the first directory, and a
/// session that ignored OSC 7 entirely would end holding `None` - which is also the correct
/// answer here. So the positive test above is what makes this one evidence, and vice versa:
/// together they distinguish "tracks the report" from "never tracked anything".
#[test]
fn an_empty_osc_7_report_clears_the_cwd() {
    let mut session = spawn(
        "printf '\\033]7;file://localhost/tmp/ruuah-cwd\\a'; sleep 1; printf '\\033]7;\\a'; sleep 2",
    );
    assert_eq!(
        wait_for_cwd(&mut session, Duration::from_secs(5)).as_deref(),
        Some("/tmp/ruuah-cwd"),
        "the first report never arrived, so the clear below would prove nothing"
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        session.take_events();
        if session.cwd().is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("an empty OSC 7 report did not clear the cwd: still {:?}", session.cwd());
}

#[test]
fn the_grid_resizes_and_the_renderer_follows() {
    let mut session = spawn("sleep 1");
    pump(&mut session, 1, Duration::from_millis(500));
    let cell = session.cell_metrics();

    let wider = SessionGeometry { cols: 40, rows: 10 };
    session.resize(wider).expect("resize");
    assert_eq!(session.geometry(), wider);

    // The renderer is rebuilt at the new grid, which is the half that silently does not happen
    // if the pty is resized alone: the child reflows, the surface keeps the old size, and the
    // window shows a correct terminal with its right-hand columns missing.
    let pixels = session.pixels();
    let expected = (cell.width * u32::from(wider.cols) * cell.height * u32::from(wider.rows) * 4)
        as usize;
    assert_eq!(pixels.len(), expected, "the surface did not follow the grid");
}
