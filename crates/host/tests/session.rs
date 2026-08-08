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

use mind2t_vt_host::session::{MouseAction, MouseMods, Session, SessionGeometry};

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

/// Polls until the child's own text is on the grid, and returns whether it arrived.
///
/// Waiting for a DRAW COUNT is the flake this replaces. A draw means the renderer painted
/// something, which happens for the caret alone on a fresh terminal, so `pump(.., 1, ..)`
/// could return satisfied before the child had written a byte. It passed on an idle machine
/// and failed on a loaded one: measured 2026-08-07, red once in a full parallel run and green
/// 5 of 5 in isolation, at a load average of about 7.
///
/// Waiting for the TEXT is not a longer timeout, it is a different question. The condition is
/// now the thing the assertion depends on, so the test either observes what it is about to
/// assert on or says plainly that it never arrived.
fn pump_until_text(session: &mut Session, wanted: &str, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        session.poll();
        if session.visible_text().contains(wanted) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
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
    let mut session = spawn("printf 'MIND2T'; sleep 1");
    // Wait for the CHILD'S TEXT, never for a draw count. A draw fires for the caret alone, so
    // the old form could be satisfied before the child wrote anything and then assert on an
    // empty surface - which is exactly how this failed under parallel load and passed alone.
    assert!(
        pump_until_text(&mut session, "MIND2T", Duration::from_secs(20)),
        "the child's own output never reached the grid; surface holds:\n{}",
        session.visible_text()
    );
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
    let mut session = spawn("printf 'MIND2T'; sleep 1");
    pump(&mut session, 1, Duration::from_secs(5));
    assert!(
        session.visible_text().contains("MIND2T"),
        "the child printed MIND2T and the grid reads {:?}",
        session.visible_text()
    );

    // The control, in the same test because it is the same claim: a silent child's grid must
    // NOT contain it. Without this, a reader that returned the child's raw bytes, or a
    // constant, or anything containing everything, would satisfy the line above.
    let mut silent = spawn("sleep 1");
    pump(&mut silent, 1, Duration::from_millis(500));
    assert!(
        !silent.visible_text().contains("MIND2T"),
        "a silent child's grid claims to hold text it never printed"
    );
}

/// Polls until the grid says `wanted`, or the deadline passes. Returns whether it did.
///
/// The report a click produces is invisible by nature - it goes TO the child - so every mouse
/// test here runs `cat` and reads the ECHOCTL echo back off the grid as printable text. That is
/// the whole seam in one assertion: mode bits published through the seqlock, geometry converted,
/// encoder, pty write, and the child's answer parsed back into cells.
fn wait_for_text(session: &mut Session, wanted: &str, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        session.poll();
        if session.visible_text().contains(wanted) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

/// A generous screen: only the CELL comes from the renderer's metrics, so pixel (1,1) is inside
/// cell (0,0) at every font size.
fn wide_open(session: &mut Session) {
    session.set_mouse_geometry(10_000, 10_000, 0, 0, 0, 0);
}

#[test]
fn a_click_is_reported_when_the_child_enabled_sgr_mouse() {
    let mut session = spawn("printf '\\033[?1000h\\033[?1006hREADY\\n'; exec cat");
    assert!(
        wait_for_text(&mut session, "READY", Duration::from_secs(5)),
        "the child's READY line never appeared, so its mode bits were never polled"
    );
    wide_open(&mut session);

    assert!(
        session.mouse(MouseAction::Press, 1, MouseMods::default(), 1.0, 1.0).expect("press"),
        "the press was not reported to a child that asked for mouse events"
    );
    assert!(
        session.mouse(MouseAction::Release, 1, MouseMods::default(), 1.0, 1.0).expect("release"),
        "the release was not reported"
    );
    assert!(
        wait_for_text(&mut session, "^[[<0;1;1M^[[<0;1;1m", Duration::from_secs(5)),
        "the SGR press/release pair never reached the child; grid says {:?}",
        session.visible_text()
    );
}

/// The control. A host that encoded unconditionally passes the test above and fails this one,
/// which is what makes the pair evidence rather than a demonstration.
#[test]
fn a_click_is_the_hosts_when_the_child_never_asked_for_mouse() {
    let mut session = spawn("printf 'READY\\n'; exec cat");
    assert!(wait_for_text(&mut session, "READY", Duration::from_secs(5)));
    wide_open(&mut session);

    assert!(
        !session.mouse(MouseAction::Press, 1, MouseMods::default(), 1.0, 1.0).expect("press"),
        "a click was reported to a child that never asked for one"
    );
}

/// Wheel precedence, both branches that matter to a host: on the alternate screen with 1007 at
/// its default the wheel becomes arrow keys and the child sees them, while on the primary screen
/// it is handed back so the host can scroll its own viewport. Getting this backwards scrolls the
/// view out from under a full-screen program - which looks like a rendering bug, not a routing one.
#[test]
fn a_wheel_is_arrows_on_the_alternate_screen_and_the_hosts_on_the_primary() {
    let mut session = spawn("printf 'READY\\n'; exec cat");
    assert!(wait_for_text(&mut session, "READY", Duration::from_secs(5)));
    wide_open(&mut session);

    assert!(
        !session.wheel(1.0, 1.0, 2, MouseMods::default()).expect("wheel"),
        "the primary screen's wheel belongs to the host"
    );

    session.send(b"\x1b[?1049hALT\r\n").expect("enter the alternate screen");
    assert!(
        wait_for_text(&mut session, "ALT", Duration::from_secs(5)),
        "the alternate screen never appeared"
    );
    assert!(
        session.wheel(1.0, 1.0, 2, MouseMods::default()).expect("wheel"),
        "the alternate screen's wheel is the child's"
    );
    assert!(
        wait_for_text(&mut session, "^[[A^[[A", Duration::from_secs(5)),
        "two up-ticks did not reach the child as two arrows; grid says {:?}",
        session.visible_text()
    );
}

/// SHIFT TAKES THE WHEEL BACK, from a child that had it.
///
/// Reported live 2026-08-09, from inside Claude Code: "you cannot scroll with the mouse scroll
/// nor jump to bottom". Nothing was broken - a full-screen program owns the wheel, which is
/// correct - but there was no way to OVERRULE it, so the scrollback was unreachable from inside
/// `claude`, `vim`, `htop` or anything else that takes the mouse.
///
/// Driven through the ALTERNATE SCREEN rather than mode 1000, and that is a deliberate choice
/// after two failures: getting reporting genuinely enabled through `cat` needs the echo, the
/// parser and the mouse geometry all to line up, and when the assertion failed it could not say
/// which of the three was missing. The alternate-scroll path is proven reachable by the test
/// above it, so this measures the escape hatch and nothing else.
///
/// Both directions, because either half alone passes against a wrong implementation: without the
/// first assertion a host that ALWAYS scrolled its own view would pass, and that host breaks
/// every program that wants the wheel.
#[test]
fn shift_takes_the_wheel_back_from_a_child_that_had_it() {
    let mut session = spawn("printf 'READY\\n'; exec cat");
    assert!(wait_for_text(&mut session, "READY", Duration::from_secs(5)));
    wide_open(&mut session);

    session.send(b"\x1b[?1049hALT\r\n").expect("enter the alternate screen");
    assert!(
        wait_for_text(&mut session, "ALT", Duration::from_secs(5)),
        "the alternate screen never appeared"
    );

    assert!(
        session.wheel(1.0, 1.0, 2, MouseMods::default()).expect("wheel"),
        "a plain wheel on the alternate screen is the child's"
    );

    let shifted = MouseMods { shift: true, ..MouseMods::default() };
    assert!(
        !session.wheel(1.0, 1.0, 2, shifted).expect("wheel"),
        "shift+wheel must come back to the host even while the child has the wheel"
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
    let mut session = spawn("printf '\\033]7;file://localhost/tmp/mind2t-cwd\\a'; sleep 2");
    assert_eq!(
        wait_for_cwd(&mut session, Duration::from_secs(5)).as_deref(),
        Some("/tmp/mind2t-cwd"),
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
        "printf '\\033]7;file://localhost/tmp/mind2t-cwd\\a'; sleep 1; printf '\\033]7;\\a'; sleep 2",
    );
    assert_eq!(
        wait_for_cwd(&mut session, Duration::from_secs(5)).as_deref(),
        Some("/tmp/mind2t-cwd"),
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
