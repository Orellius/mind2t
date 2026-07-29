//! The blind spot slice 7 opens: nothing could see a SECOND backend.
//!
//! Every pixel test in this project measures one renderer against itself -- `redraw.rs`
//! compares an incremental repaint with a full one, `caret.rs` compares a shown caret with a
//! hidden one. All of them stay green if the backend is uniformly wrong, because both sides
//! of every comparison come from the same code.
//!
//! A second backend breaks that. The atlas, the run-to-column mapping and the damage logic
//! are shared, so the only place a GPU can disagree with the CPU is in the four operations
//! behind `Surface` -- and the way it disagrees is by a unit or two per channel, which no
//! screenshot and no human will ever catch.
//!
//! So the requirement is byte equality, not a tolerance. That is affordable because the blend
//! is specified as integer arithmetic rather than left to the backend: any backend that does
//! `(source * alpha + destination * (255 - alpha) + 127) / 255` per channel lands on the same
//! bytes by construction, GPU or not.
//!
//! Proven able to fail by `a_backend_that_truncates_the_blend_is_caught`, which drops the
//! rounding term -- the exact mistake a shader that computes in floats and rounds toward zero
//! would make -- and is caught despite being off by at most one.

use ruuah_vt_core::Terminal;
use ruuah_vt_frame::{Frame, Publisher, ReadOutcome, channel};
use ruuah_vt_render::{Canvas, FontStack, GpuSurface, Renderer, Surface, TruncatingSurface};

const COLS: u16 = 24;
const ROWS: u16 = 6;

/// Text, colour, reverse video and Hebrew: everything that puts partial coverage on the
/// canvas, which is the only place two backends can drift.
const SCRIPT: &[&[u8]] = &[
    b"the quick brown fox",
    b"\r\n\x1b[31mred\x1b[0m \x1b[7mreverse\x1b[0m",
    "\r\nשלום עולם".as_bytes(),
    b"\r\n\x1b[4munderlined\x1b[0m tail",
    "\r\nmixed ab \x1b[32mגד\x1b[0m 42".as_bytes(),
];

fn fonts() -> FontStack {
    FontStack::system(16.0).expect("system fonts")
}

/// Runs the script through a renderer on backend `S` and returns the finished pixels.
fn render_through<S: Surface>() -> Vec<u8> {
    let mut terminal = Terminal::new(COLS, ROWS);
    let (writer, reader) = channel(COLS, ROWS);
    let mut publisher = Publisher::new(writer);
    let mut frame = Frame::new();
    let mut renderer: Renderer<S> = Renderer::with_surface(fonts(), COLS, ROWS);

    for step in SCRIPT {
        terminal.write(step);
        publisher.publish(&mut terminal).expect("fits");
        assert!(matches!(
            reader.read_into(&mut frame),
            ReadOutcome::Fresh(_)
        ));
        renderer.draw(&frame);
    }
    renderer.pixels()
}

/// The comparison has to be capable of agreeing, or "the backends differ" below means
/// nothing. Two runs on the reference backend must be identical.
#[test]
fn the_reference_backend_agrees_with_itself() {
    assert_eq!(
        render_through::<Canvas>(),
        render_through::<Canvas>(),
        "the CPU reference is not deterministic, so no backend can be measured against it"
    );
}

/// The control. Dropping the rounding term is at most a one-unit error per channel and
/// invisible on screen; if byte equality did not catch it, nothing would.
#[test]
fn a_backend_that_truncates_the_blend_is_caught() {
    let reference = render_through::<Canvas>();
    let truncating = render_through::<TruncatingSurface>();

    assert_ne!(
        reference, truncating,
        "a backend that truncates the blend went undetected"
    );

    // And the error really is sub-perceptual, which is the point: this is the size of mistake
    // the harness has to catch, because it is the size of mistake a GPU actually makes.
    let worst = reference
        .iter()
        .zip(&truncating)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .expect("a non-empty canvas");
    assert_eq!(
        worst, 1,
        "the control should be off by exactly one somewhere and no more"
    );

    let differing = reference
        .iter()
        .zip(&truncating)
        .filter(|(a, b)| a != b)
        .count();
    assert!(
        differing > 0,
        "no byte differed, so the script never exercised a partial blend"
    );
}

/// The script has to actually produce partial coverage, or the control above is comparing two
/// runs of solid fills and the whole file is theatre.
#[test]
fn the_script_puts_partial_coverage_on_the_canvas() {
    let reference = render_through::<Canvas>();
    let truncating = render_through::<TruncatingSurface>();
    let blended = reference
        .iter()
        .zip(&truncating)
        .filter(|(a, b)| a != b)
        .count();

    assert!(
        blended >= 100,
        "only {blended} bytes came from a partial blend; the script is not exercising glyphs"
    );
}

/// The slice. Same frames, same atlas, same run-to-column mapping, different backend -- and
/// the same bytes, because the shader does the CPU's integer blend rather than its own
/// floating-point approximation of it.
#[test]
fn the_gpu_backend_matches_the_cpu_reference_byte_for_byte() {
    let reference = render_through::<Canvas>();
    let gpu = render_through::<GpuSurface>();

    assert_eq!(
        reference.len(),
        gpu.len(),
        "the two backends disagree on size"
    );

    if reference != gpu {
        let worst = reference
            .iter()
            .zip(&gpu)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        let differing = reference.iter().zip(&gpu).filter(|(a, b)| a != b).count();
        let first = reference
            .iter()
            .zip(&gpu)
            .position(|(a, b)| a != b)
            .unwrap_or(0);
        panic!(
            "GPU and CPU disagree: {differing} of {} bytes, worst delta {worst}, first at byte \
             {first} (pixel {}, channel {})",
            reference.len(),
            first / 4,
            first % 4
        );
    }
}

/// Reading the pixels between draws must not change the final image.
///
/// The blind spot this closes: every other test here reads once, at the end, so all recorded
/// work is replayed in a single batch and an incremental backend is never exercised at all.
/// The GPU surface retains its pixel buffer and dispatches only the operations recorded since
/// the last read -- equivalent to replaying the whole history onto a cleared buffer, but only
/// if the retention actually works. Clearing the buffer per read, or replaying only the last
/// frame's operations, both lose everything painted before the previous read and are caught
/// here rather than in a screenshot months later.
#[test]
fn reading_between_draws_leaves_the_same_image_as_reading_once() {
    fn render_reading_every_step<S: Surface>() -> Vec<u8> {
        let mut terminal = Terminal::new(COLS, ROWS);
        let (writer, reader) = channel(COLS, ROWS);
        let mut publisher = Publisher::new(writer);
        let mut frame = Frame::new();
        let mut renderer: Renderer<S> = Renderer::with_surface(fonts(), COLS, ROWS);

        let mut last = Vec::new();
        for step in SCRIPT {
            terminal.write(step);
            publisher.publish(&mut terminal).expect("fits");
            assert!(matches!(
                reader.read_into(&mut frame),
                ReadOutcome::Fresh(_)
            ));
            renderer.draw(&frame);
            last = renderer.pixels();
        }
        last
    }

    assert_eq!(
        render_reading_every_step::<Canvas>(),
        render_through::<Canvas>(),
        "the CPU reference must not care when it is read, or it cannot judge the GPU"
    );
    assert_eq!(
        render_reading_every_step::<GpuSurface>(),
        render_through::<GpuSurface>(),
        "reading between draws changed the GPU's final image, so painting is not cumulative"
    );
    assert_eq!(
        render_reading_every_step::<GpuSurface>(),
        render_through::<Canvas>(),
        "and the incrementally-read GPU image must still equal the CPU reference"
    );
}

/// The recorded work is retired by a read, not accumulated for the surface's lifetime.
///
/// This is the defect itself rather than a symptom of it, and no image comparison can see it:
/// replaying the entire history onto a cleared buffer produces exactly the right pixels, which
/// is why it survived a slice. What it does not survive is a long-running terminal -- the
/// dispatch cost grows with everything ever drawn, and the log has a hard ceiling at 1,048,576
/// operations.
#[test]
fn a_read_retires_the_recorded_work() {
    let mut terminal = Terminal::new(COLS, ROWS);
    let (writer, reader) = channel(COLS, ROWS);
    let mut publisher = Publisher::new(writer);
    let mut frame = Frame::new();
    let mut renderer: Renderer<GpuSurface> = Renderer::with_surface(fonts(), COLS, ROWS);

    let mut high_water = 0;
    for round in 0..12 {
        terminal.write(format!("\r\nline {round} with some text").as_bytes());
        publisher.publish(&mut terminal).expect("fits");
        assert!(matches!(
            reader.read_into(&mut frame),
            ReadOutcome::Fresh(_)
        ));
        renderer.draw(&frame);

        let queued = renderer.canvas().recorded_ops();
        assert!(queued > 0, "round {round} recorded no work at all");
        high_water = high_water.max(queued);

        let _ = renderer.pixels();
        assert_eq!(
            renderer.canvas().recorded_ops(),
            0,
            "round {round}: a read left {} operations behind; they accumulate for the lifetime \
             of the surface and every later read replays them again",
            renderer.canvas().recorded_ops()
        );
    }

    // Twelve rounds of a full-screen redraw would be several thousand operations if they were
    // kept; the ceiling here is what one round costs.
    assert!(
        high_water < 12 * COLS as usize * ROWS as usize,
        "the per-round backlog reached {high_water}, which is more than one round of work"
    );
}

/// A frame with more operations than Metal's command-buffer pool must still complete.
///
/// Found by slice 8's window, hung on its first vim frame: recording one compute pass per
/// operation makes the Metal backend open a command buffer per pass, none of which can
/// complete before the one submit at the end -- and the queue's pool (64) blocks the 65th
/// request forever, mid-encoding. Every other test in this file stays under the pool by
/// accident; this one is a full screen of glyphs, and it is run under a watchdog because
/// the failure mode is not a wrong byte but a hang.
#[test]
fn a_frame_with_more_ops_than_the_command_buffer_pool_completes() {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut terminal = Terminal::new(80, 24);
        for row in 0..24 {
            let line = "abcdefghij".repeat(7);
            terminal.write(line.as_bytes());
            if row < 23 {
                terminal.write(b"\r\n");
            }
        }
        let (writer, reader) = channel(80, 24);
        let mut publisher = Publisher::new(writer);
        publisher.publish(&mut terminal).expect("fits");
        let mut frame = Frame::new();
        assert!(matches!(
            reader.read_into(&mut frame),
            ReadOutcome::Fresh(_)
        ));

        let mut gpu: Renderer<GpuSurface> = Renderer::with_surface(fonts(), 80, 24);
        gpu.draw_all(&frame);
        let recorded = gpu.canvas().recorded_ops();
        assert!(
            recorded > 64,
            "only {recorded} operations were recorded; this test no longer exceeds the pool"
        );
        let gpu_pixels = gpu.pixels();

        let mut cpu: Renderer<Canvas> = Renderer::with_surface(fonts(), 80, 24);
        cpu.draw_all(&frame);
        let cpu_pixels = cpu.pixels();

        let _ = sender.send((gpu_pixels, cpu_pixels));
    });

    let (gpu_pixels, cpu_pixels) = receiver
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the GPU never finished the frame -- encoding exhausted the command-buffer pool");
    assert_eq!(
        gpu_pixels, cpu_pixels,
        "the backends disagree on a large frame"
    );
}
