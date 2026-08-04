//! Two real shells in one canvas, each asked what size IT thinks it is.
//!
//! The defect this file exists to catch is the one that looks healthy: a pane whose pty kept the
//! whole window's column count. Its terminal renders perfectly, its colours are right, and its
//! right-hand columns are drawn underneath the neighbour - so a command's output is silently
//! truncated at the seam and it reads as a program that stopped mid-line.
//!
//! Deriving the expected columns in Rust and comparing against the session's own geometry would
//! pass on exactly that bug, because both sides would be the same wrong number. So the number
//! comes back through the pseudoterminal: each child runs `stty size` and we read the answer off
//! its own grid. That is the whole seam - layout arithmetic, `TIOCSWINSZ`, the child's idea of
//! itself, the parser, and the renderer - in one assertion per pane.

use std::process::Command;
use std::time::{Duration, Instant};

use bindary::canvas::{Canvas, PaneSpec};
use bindary::layout::{Canvas as Grid, Rect};

const FONT: f32 = 16.0;

fn shell(_spec: &PaneSpec) -> Command {
    let mut command = Command::new("/bin/sh");
    // `stty size` answers "<rows> <cols>", which is the child's own view. `exec cat` afterwards
    // keeps the pane alive and quiet so nothing repaints over the answer.
    command.arg("-c").arg("stty size; exec cat");
    command
}

/// Polls every pane until each grid holds something, or the deadline passes.
fn pump(canvas: &mut Canvas, budget: Duration) {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        let mut all = true;
        for pane in canvas.panes_mut() {
            pane.session.poll();
            if pane.session.visible_text().trim().is_empty() {
                all = false;
            }
        }
        if all {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// What the child said, as `(rows, cols)`.
fn reported(text: &str) -> Option<(u16, u16)> {
    let line = text.lines().find(|line| {
        let mut parts = line.split_whitespace();
        matches!(
            (parts.next().map(str::parse::<u16>), parts.next().map(str::parse::<u16>), parts.next()),
            (Some(Ok(_)), Some(Ok(_)), None)
        )
    })?;
    let mut parts = line.split_whitespace();
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

#[test]
fn each_pane_tells_its_child_its_own_size() {
    // Wide and deliberately ODD, so the two columns cannot both be the tidy half of it and a
    // dropped remainder shows up as a child reporting the wrong width.
    let area = Rect { x: 0, y: 0, width: 1801, height: 900 };
    let grid = Grid { rows: 1, cols: 2 };
    let mut canvas = Canvas::spawn(
        grid,
        area,
        &[PaneSpec::shell(), PaneSpec::shell()],
        FONT,
        shell,
    )
    .expect("a canvas");

    pump(&mut canvas, Duration::from_secs(10));

    let panes = canvas.panes();
    assert_eq!(panes.len(), 2);

    let mut seen = Vec::new();
    for (index, pane) in panes.iter().enumerate() {
        let text = pane.session.visible_text();
        let said = reported(&text)
            .unwrap_or_else(|| panic!("pane {index} never reported its size; grid says {text:?}"));
        let geometry = pane.session.geometry();
        assert_eq!(
            said,
            (geometry.rows, geometry.cols),
            "pane {index}: the child thinks it is {said:?} while the session says \
             {:?} - the pty was told a different size from the one being drawn",
            (geometry.rows, geometry.cols)
        );
        seen.push(said);
    }

    // The claim that matters: each pane is about HALF the window, not the whole of it. A pane
    // that kept the full width is the silent defect, and it passes every assertion above.
    let full = area.width / panes[0].session.cell_metrics().width.max(1);
    for (index, (_, cols)) in seen.iter().enumerate() {
        assert!(
            u32::from(*cols) < full * 3 / 4,
            "pane {index} has {cols} columns of a possible {full}: it kept the whole window and \
             is drawing underneath its neighbour"
        );
    }
    assert_eq!(
        u32::from(seen[0].1) + u32::from(seen[1].1) + 1 >= full,
        true,
        "the two panes together ({} + {}) do not account for the window's {full} columns",
        seen[0].1,
        seen[1].1
    );
}

/// A resize must reach the children, not only the rects.
///
/// The control for the test above: same canvas, same panes, and the numbers must CHANGE. Without
/// it, a `resize` that updated `pane.rect` and never called the pty would satisfy every
/// geometric assertion in this file.
#[test]
fn a_resize_reaches_the_children() {
    let mut canvas = Canvas::spawn(
        Grid { rows: 1, cols: 2 },
        Rect { x: 0, y: 0, width: 1800, height: 900 },
        &[PaneSpec::shell(), PaneSpec::shell()],
        FONT,
        |_| {
            let mut command = Command::new("/bin/sh");
            // Every report is MARKED, and that is not decoration: after the pane narrows, the
            // pre-resize line REFLOWS - a bare "24 90" can split across two rows and parse as a
            // different pair of numbers entirely. The marker makes the newest report findable in
            // a grid that has been rewritten underneath it.
            command
                .arg("-c")
                .arg("trap 'echo WINCH $(stty size)' WINCH; echo WINCH $(stty size); while :; do sleep 0.1; done");
            command
        },
    )
    .expect("a canvas");

    pump(&mut canvas, Duration::from_secs(10));
    let before = canvas.panes()[0].session.geometry().cols;

    canvas
        .resize(Rect { x: 0, y: 0, width: 900, height: 900 })
        .expect("resize");

    let after = canvas.panes()[0].session.geometry().cols;
    assert!(after < before, "the pane's own geometry did not shrink");

    // Polls until the child's LAST marked report agrees with the new width. A fixed wait would
    // be a race against SIGWINCH; this is the event itself, and a child that never reports still
    // fails when the budget runs out.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last = None;
    while Instant::now() < deadline {
        for pane in canvas.panes_mut() {
            pane.session.poll();
        }
        let text = canvas.panes()[0].session.visible_text();
        last = text
            .lines()
            .filter(|line| line.contains("WINCH"))
            .filter_map(|line| reported(line.trim_start_matches(|c: char| !c.is_ascii_digit())))
            .next_back();
        if last.is_some_and(|(_, cols)| cols == after) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    assert_eq!(
        last.map(|(_, cols)| cols),
        Some(after),
        "the child's last reported width disagrees with the session's {after}; grid says {:?}",
        canvas.panes()[0].session.visible_text()
    );
}
