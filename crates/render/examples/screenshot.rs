//! Runs a command in a real pty and writes what this terminal drew, as a BMP.
//!
//! Why this exists: the screenshots in the README are of the test suite, and the honest way to
//! take them is with the engine the suite is testing. A screenshot captured from some other
//! terminal would be a picture of somebody else's renderer agreeing with our numbers, which is
//! not the claim being made. Rendered here, the image is the CPU backend's own output, and the
//! CPU backend is byte-identical to the GPU one by specification.
//!
//! It also needs no window and no display, which is why it can run on a machine somebody else
//! is using.
//!
//! ```sh
//! cargo run -p mind2t-vt-render --example screenshot -- \
//!     --cols 100 --rows 32 --out /tmp/shot.bmp --until "met expectation" -- ./target/debug/difftest
//! ```
//!
//! `--until` is the substring that means the command has finished drawing. Waiting for the
//! child to EXIT is the wrong signal: a program that prints and then exits has its last frame
//! published after the process is gone, and a program that never exits would hang forever.

use std::io::Write;
use std::time::{Duration, Instant};

use mind2t_vt_frame::{CLUSTER_BYTES, Frame, FrameReader};
use mind2t_vt_pty::{Host, Options};
use mind2t_vt_render::{FontStack, Renderer};

struct Args {
    cols: u16,
    rows: u16,
    font: f32,
    out: String,
    until: String,
    command: Vec<String>,
    patience: Duration,
}

fn parse() -> Args {
    let mut args = Args {
        cols: 100,
        rows: 32,
        font: 16.0,
        out: "/tmp/shot.bmp".into(),
        until: String::new(),
        command: Vec::new(),
        patience: Duration::from_secs(120),
    };
    let mut raw = std::env::args().skip(1);
    while let Some(flag) = raw.next() {
        match flag.as_str() {
            "--cols" => args.cols = raw.next().expect("--cols").parse().expect("cols"),
            "--rows" => args.rows = raw.next().expect("--rows").parse().expect("rows"),
            "--font" => args.font = raw.next().expect("--font").parse().expect("font"),
            "--out" => args.out = raw.next().expect("--out"),
            "--until" => args.until = raw.next().expect("--until"),
            "--seconds" => {
                args.patience = Duration::from_secs(raw.next().expect("--seconds").parse().unwrap())
            }
            "--" => {
                args.command = raw.collect();
                break;
            }
            other => panic!("unknown flag {other}"),
        }
    }
    assert!(!args.command.is_empty(), "no command after --");
    args
}

fn text(frame: &Frame) -> String {
    let mut scratch = [0u8; CLUSTER_BYTES];
    let mut out = String::new();
    for y in 0..frame.rows {
        for x in 0..frame.cols {
            let cell = frame.cell(x, y);
            if cell.has_text() {
                out.push_str(cell.cluster(&mut scratch));
            } else {
                out.push(' ');
            }
        }
        out.push('\n');
    }
    out
}

fn main() {
    let args = parse();

    let mut command = std::process::Command::new(&args.command[0]);
    command.args(&args.command[1..]);
    // Declared, never inherited: this may run from a harness with no TERM at all, and a child
    // that finds no terminfo entry exits before drawing a cell.
    command
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .env("LANG", "en_US.UTF-8");

    let (_host, reader) = Host::spawn(command, Options::new(args.cols, args.rows)).expect("spawn");

    let mut frame = Frame::new();
    let mut last = Frame::new();
    let deadline = Instant::now() + args.patience;
    let mut settled = None;
    while Instant::now() < deadline {
        reader.read_into(&mut frame);
        // A read interrupted mid-publish leaves the frame invalid rather than untouched, so it
        // must not be inspected at all.
        if frame.is_valid() {
            // `Frame` is deliberately not Clone (it owns its cell storage), so the good frame is
            // SWAPPED into `last` and the stale one goes back to be overwritten by the next read.
            std::mem::swap(&mut last, &mut frame);
            if !args.until.is_empty() && text(&frame).contains(&args.until) {
                // One more beat, so the line AFTER the marker lands too. A marker is usually
                // the last interesting line, not the last line.
                std::thread::sleep(Duration::from_millis(300));
                reader.read_into(&mut frame);
                if frame.is_valid() {
                    // `Frame` is deliberately not Clone (it owns its cell storage), so the good frame is
            // SWAPPED into `last` and the stale one goes back to be overwritten by the next read.
            std::mem::swap(&mut last, &mut frame);
                }
                settled = Some(());
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    if args.until.is_empty() {
        settled = Some(());
    }
    assert!(
        settled.is_some(),
        "the marker {:?} never appeared within {:?}; last frame was:\n{}",
        args.until,
        args.patience,
        text(&last)
    );

    let mut renderer = Renderer::new(
        FontStack::system(args.font).expect("fonts"),
        args.cols,
        args.rows,
    );
    let painted = renderer.draw(&last);
    assert!(painted > 0, "nothing was painted");

    let mut file = std::fs::File::create(&args.out).expect("create");
    file.write_all(&renderer.canvas().to_bmp()).expect("write");
    println!("wrote {} ({painted} rows painted)", args.out);
}
