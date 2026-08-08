//! How many panes this host can actually hold, measured rather than assumed.
//!
//! Everything in this project has run at one or two panes. The canvas slice exists because the
//! operator runs FORTY-ONE agent sessions at once, so "does it scale" stopped being a curiosity
//! and became the assumption every canvas design rests on. An assumption nobody has executed is
//! the thing this repo has been bitten by ten slices running.
//!
//! Three costs are separable and only one of them is obvious:
//!
//! - **The pty.** One child, one master fd, one pump thread each. The known hazard is fd
//!   exhaustion and the transient ENXIO the pty host retries three times over 25ms - a retry
//!   budget sized for ONE terminal opening, not for N opening in a tight loop.
//! - **The GPU surface.** Every pane owns a pixel buffer and its bind group. They share one
//!   device by design (a composited frame is one render pass), so the ceiling here is memory and
//!   the backend's own limits, not the device count.
//! - **The frame.** Each pane publishes through its own seqlock, so nothing serialises across
//!   panes; what does serialise is the single present.
//!
//! IGNORED BY DEFAULT. It spawns dozens of real children and is a measurement, not an invariant -
//! a number that moves with the machine's load has no business failing anyone's commit. Run it
//! deliberately:
//!
//! ```sh
//! cargo test -p mind2t --test scale -- --ignored --nocapture
//! ```

use std::process::Command;
use std::time::{Duration, Instant};

use mind2t::canvas::{Canvas, PaneSpec};
use mind2t::layout::{Canvas as Grid, Rect};
use mind2t_vt_render::GpuContext;

const FONT: f32 = 16.0;

/// Same gutter the live canvas uses, so the per-pane width these numbers were measured at is
/// the width a real window would give them.
const GUTTER: u32 = 4;

/// Wide enough that forty-one panes each clear `MIN_SPLIT_COLS`, and tall enough for a real grid.
/// This is a headless surface, so the area is not bounded by any display.
fn area_for(panes: u32) -> Rect {
    // Per-pane width is overridable so the held-open reading can separate a cost that follows the
    // pane's PIXELS from one that is fixed per session. Without that, "204 MiB per pane" is a
    // number with no cause attached.
    let per_pane: u32 = std::env::var("MIND2T_SCALE_WIDTH")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(220);
    Rect { x: 0, y: 0, width: panes * per_pane, height: 700 }
}

fn shell(_spec: &PaneSpec) -> Command {
    let mut command = Command::new("/bin/sh");
    // `exec cat` keeps the child alive, silent and cheap. A shell would run its rc files N times
    // and turn a measurement of this host into a measurement of zsh's startup.
    command.arg("-c").arg("exec cat");
    command
}

/// Resident set size of this process, in MiB, straight from `ps`.
///
/// Read externally rather than from an allocator hook because the interesting memory is not all
/// ours: pty buffers and wgpu's device-side allocations are the ones expected to move, and an
/// in-process counter sees neither.
fn rss_mib() -> f64 {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<f64>()
        .unwrap_or(0.0)
        / 1024.0
}

fn measure(gpu: &GpuContext, count: u32) -> (Duration, Duration, f64, usize) {
    let before = rss_mib();
    let grid = Grid { rows: 1, cols: count as u16, gutter: GUTTER };
    let area = area_for(count);

    let spawned = Instant::now();
    let mut canvas = Canvas::spawn(gpu, grid, area, &[], FONT, shell).expect("a canvas");
    let spawn_time = spawned.elapsed();

    // One poll sweep across every pane, which is what a frame costs before anything is drawn.
    let polled = Instant::now();
    for pane in canvas.panes_mut() {
        pane.session.poll();
    }
    let poll_time = polled.elapsed();

    let after = rss_mib();
    let panes = canvas.panes().len();
    canvas.shutdown();
    (spawn_time, poll_time, after - before, panes)
}

#[test]
#[ignore = "measurement, not an invariant: spawns dozens of real children"]
fn how_many_panes_this_host_can_hold() {
    let gpu = GpuContext::new().expect("a GPU");

    println!("\npanes   spawn      poll     rss delta");
    println!("-----   --------   ------   ---------");
    for count in [1u32, 2, 4, 8, 16, 24, 41] {
        let (spawn, poll, rss, panes) = measure(&gpu, count);
        assert_eq!(panes as u32, count, "the canvas did not build {count} panes");
        println!(
            "{count:>5}   {:>6.0}ms   {:>4.1}ms   {rss:>+7.1} MiB",
            spawn.as_secs_f64() * 1000.0,
            poll.as_secs_f64() * 1000.0,
        );
    }
    println!();
}

/// Forty-one panes at once, held open together, and every child asked to prove it is alive.
///
/// Separate from the sweep above because the sweep tears each canvas down before building the
/// next, so it never has more than N children at one moment. The ceiling that matters for the
/// canvas is N *concurrent*, which is where fd limits and the pty host's spawn retry live.
/// The same sweep with the canvas area held CONSTANT, which is the discriminating half.
///
/// The first sweep grows the area with the pane count, so a cost that follows the canvas and a
/// cost that follows the pane are indistinguishable in it - and the memory curve it produced was
/// wildly non-linear, so which of the two it is decides whether a canvas of 41 is possible at all.
/// Here the window never changes size; only how many ways it is divided.
#[test]
#[ignore = "measurement, not an invariant: spawns dozens of real children"]
fn the_same_sweep_with_the_window_held_still() {
    let gpu = GpuContext::new().expect("a GPU");
    println!("\nFIXED AREA 4000x700");
    println!("panes   spawn      poll     rss delta");
    println!("-----   --------   ------   ---------");
    for count in [1u32, 2, 4, 8, 16, 24, 41] {
        let before = rss_mib();
        let grid = Grid { rows: 1, cols: count as u16, gutter: GUTTER };
        let area = Rect { x: 0, y: 0, width: 4000, height: 700 };
        let started = Instant::now();
        let mut canvas = Canvas::spawn(&gpu, grid, area, &[], FONT, shell).expect("a canvas");
        let spawn = started.elapsed();
        let polled = Instant::now();
        for pane in canvas.panes_mut() {
            pane.session.poll();
        }
        let poll = polled.elapsed();
        let rss = rss_mib() - before;
        canvas.shutdown();
        println!(
            "{count:>5}   {:>6.0}ms   {:>4.1}ms   {rss:>+7.1} MiB",
            spawn.as_secs_f64() * 1000.0,
            poll.as_secs_f64() * 1000.0,
        );
    }
    println!();
}

#[test]
#[ignore = "measurement, not an invariant: spawns 41 real children at once"]
fn forty_one_panes_are_all_alive_at_the_same_time() {
    let gpu = GpuContext::new().expect("a GPU");
    // Overridable so the same held-open reading can be taken at other counts in FRESH processes,
    // which is the only way these numbers compare to each other.
    let count: u32 = std::env::var("MIND2T_SCALE_PANES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(41);
    let grid = Grid { rows: 1, cols: count as u16, gutter: GUTTER };

    let started = Instant::now();
    let mut canvas =
        Canvas::spawn(&gpu, grid, area_for(count), &[], FONT, shell).expect("41 panes");
    println!("\n{count} panes spawned in {:?}", started.elapsed());

    // Every pane gets its own byte and must show it. A canvas that built 41 rects over 3 live
    // children would satisfy any count-based assertion; only the round trip through each pty can
    // tell a pane from a rectangle.
    for (index, pane) in canvas.panes_mut().iter_mut().enumerate() {
        let mark = format!("p{index}\n");
        pane.session.send(mark.as_bytes()).expect("send");
    }

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut silent: Vec<usize> = Vec::new();
    loop {
        silent.clear();
        for (index, pane) in canvas.panes_mut().iter_mut().enumerate() {
            pane.session.poll();
            if !pane.session.visible_text().contains(&format!("p{index}")) {
                silent.push(index);
            }
        }
        if silent.is_empty() || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // Read while they are ALL still open. The sweeps above measure deltas inside one process and
    // they disagreed with each other at near-identical configurations, because a freed canvas does
    // not return its pages to the OS and the next iteration's "before" is already polluted. One
    // canvas in a fresh process is the only reading here that can be trusted.
    println!("{count} panes held open: rss {:.0} MiB ({:.1} MiB/pane)", rss_mib(), rss_mib() / f64::from(count));

    let quiet = silent.clone();
    canvas.shutdown();
    assert!(quiet.is_empty(), "panes that never echoed their own mark: {quiet:?}");
    println!("all {count} echoed their own mark\n");
}
