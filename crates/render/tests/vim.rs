//! Slice 5's acceptance gate: does it render vim?
//!
//! vim is the right program to gate on because it uses every part of the stack at once -- the
//! alternate screen, absolute cursor addressing, erases, a reverse-video status line, and the
//! tilde column that only appears if erase-to-end-of-display works. A terminal that renders
//! vim correctly is not guessing.
//!
//! Everything asserted here is structural rather than a golden image: font versions move, and
//! a test that breaks when Apple ships a new Menlo is a test nobody trusts. The frame is also
//! written out as a BMP so it can be looked at, which is the one part a machine cannot check.

use std::io::Write;
use std::time::{Duration, Instant};

use mind2t_vt_frame::{CLUSTER_BYTES, Frame, FrameReader};
use mind2t_vt_pty::{Host, Options};
use mind2t_vt_render::{FontStack, Renderer};

const COLS: u16 = 80;
const ROWS: u16 = 24;
const PATIENCE: Duration = Duration::from_secs(10);

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
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "no frame matched within {PATIENCE:?}; last frame was:\n{}",
        text(&frame)
    );
}

/// Writes the canvas somewhere a human can open it, and says where.
fn save(renderer: &Renderer, name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mind2t-vt-{name}.bmp"));
    let mut file = std::fs::File::create(&path).expect("create bmp");
    file.write_all(&renderer.canvas().to_bmp())
        .expect("write bmp");
    println!("wrote {}", path.display());
    path
}

#[test]
fn it_renders_vim() {
    let source = std::env::temp_dir().join("mind2t-vt-vim-fixture.txt");
    std::fs::write(
        &source,
        "the quick brown fox\njumps over the lazy dog\nSPHINX OF BLACK QUARTZ\n",
    )
    .expect("fixture");

    let mut command = std::process::Command::new("/usr/bin/vim");
    // -u NONE -N: no user config, but not vi-compatible either, so this renders the same on
    // any machine rather than whatever the local vimrc happens to say.
    // -n: no swap file. The host is killed rather than quit, so vim leaves through its
    // emergency path and PRESERVES the swap -- which the next run then reports as
    // `E325: ATTENTION`, pages at `-- More --`, and never draws the buffer. Measured
    // 2026-07-31: a passing run planted the file that made the following run time out at
    // ten seconds, so the gate was red on every second invocation for a reason that had
    // nothing to do with the renderer.
    command
        .arg("-u")
        .arg("NONE")
        .arg("-N")
        .arg("-n")
        .arg(&source)
        .env("TERM", "xterm-256color")
        .env("LANG", "en_US.UTF-8");

    let (host, reader) = Host::spawn(command, Options::new(COLS, ROWS)).expect("spawn vim");

    // The tilde column is vim's own proof that it painted a full screen: it only appears once
    // vim has cleared the display and drawn past the end of the buffer.
    let frame = wait_for(&reader, |frame| {
        let seen = text(frame);
        seen.contains("the quick brown fox") && seen.contains('~')
    });

    let mut renderer = Renderer::new(FontStack::system(16.0).expect("fonts"), COLS, ROWS);
    let painted = renderer.draw(&frame);
    assert!(painted > 0, "vim's first screen painted no rows");

    let metrics = FontStack::system(16.0).unwrap().metrics();
    let canvas = renderer.canvas();
    assert_eq!(canvas.width(), metrics.width * u32::from(COLS));
    assert_eq!(canvas.height(), metrics.height * u32::from(ROWS));

    let background = renderer.palette().default_background;
    let inked_rows = (0..ROWS)
        .filter(|y| {
            (0..canvas.width()).any(|x| {
                (0..metrics.height)
                    .any(|dy| canvas.pixel(x, u32::from(*y) * metrics.height + dy) != background)
            })
        })
        .count();

    // Three text rows, twenty tilde rows and a status line. Anything under five means large
    // parts of the screen came out blank.
    assert!(
        inked_rows >= 5,
        "only {inked_rows} of {ROWS} rows have any ink:\n{}",
        text(&frame)
    );

    let path = save(&renderer, "vim");
    assert!(std::fs::metadata(&path).expect("bmp exists").len() > 54);

    drop(host);
    let _ = std::fs::remove_file(&source);
}

#[test]
fn it_renders_vim_in_hebrew() {
    // Not reordered: this is logical order on screen, and it is exactly what slice 5.5 has to
    // fix. Rendering it now is what proves the fallback font, the cluster transport and the
    // atlas all survive to pixels, so 5.5 is a reordering problem and not an everything
    // problem.
    let source = std::env::temp_dir().join("mind2t-vt-vim-hebrew.txt");
    std::fs::write(
        &source,
        "\u{05E9}\u{05DC}\u{05D5}\u{05DD} \u{05E2}\u{05D5}\u{05DC}\u{05DD}\nhello world\n",
    )
    .expect("fixture");

    let mut command = std::process::Command::new("/usr/bin/vim");
    // -n for the same reason as the fixture test above: a killed vim keeps its swap file.
    command
        .arg("-u")
        .arg("NONE")
        .arg("-N")
        .arg("-n")
        .arg(&source)
        .env("TERM", "xterm-256color")
        .env("LANG", "en_US.UTF-8");

    let (host, reader) = Host::spawn(command, Options::new(COLS, ROWS)).expect("spawn vim");
    let frame = wait_for(&reader, |frame| text(frame).contains("hello world"));

    let mut renderer = Renderer::new(FontStack::system(16.0).expect("fonts"), COLS, ROWS);
    renderer.draw(&frame);

    let metrics = FontStack::system(16.0).unwrap().metrics();
    let background = renderer.palette().default_background;
    let canvas = renderer.canvas();

    let hebrew_ink = (0..4u32)
        .map(|column| {
            (0..metrics.height)
                .flat_map(|y| (0..metrics.width).map(move |x| (x, y)))
                .filter(|(x, y)| canvas.pixel(column * metrics.width + x, *y) != background)
                .count()
        })
        .collect::<Vec<_>>();

    assert!(
        hebrew_ink.iter().all(|ink| *ink > 0),
        "a Hebrew line reached vim but not the canvas: per-column ink {hebrew_ink:?}"
    );

    save(&renderer, "vim-hebrew");
    drop(host);
    let _ = std::fs::remove_file(&source);
}
