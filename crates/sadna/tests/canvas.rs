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

use sadna::canvas::{Canvas, PaneSpec};
use sadna::layout::{Canvas as Grid, Rect};
use ruuah_vt_render::{GpuContext, Surface, wgpu};

const FONT: f32 = 16.0;

fn gpu() -> GpuContext {
    GpuContext::new().expect("a GPU")
}

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
        &gpu(),
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

/// Every pane reaches ONE frame, at its own rect, byte for byte.
///
/// This is the check the canvas shipped without, and the gap was structural rather than an
/// oversight: nothing in the suite ever PRESENTED, so a canvas whose panes could not be drawn
/// together scored a perfect run with real children and correct geometry. Two failures live in
/// that gap and neither announces itself as what it is:
///
/// - **One device per pane.** A session used to build its own `GpuContext`, and a render pass can
///   only bind buffers from the device it runs on. Composited, that is a wgpu validation failure
///   - the frame is not slow, it does not exist.
/// - **A per-pane clear.** Blitting each pane in its own pass leaves only the last one on screen
///   and the window shows a single terminal beside a field of clear colour, which reads as "the
///   other shell never started".
///
/// The assertion is byte equality between what the target holds at a pane's rect and what that
/// pane's own surface holds - the same equality `render/tests/present.rs` uses for one surface,
/// per pane. Distinct content per pane is what makes it positional: identical panes would satisfy
/// it with the two swapped.
#[test]
fn every_pane_reaches_one_frame_at_its_own_rect() {
    // 512 wide keeps the readback's 256-byte row alignment (512 * 4 = 2048). The panes are
    // 256 x 256, which is small for a terminal and irrelevant to what is being measured.
    const WIDTH: u32 = 512;
    const HEIGHT: u32 = 256;

    let context = gpu();
    let area = Rect { x: 0, y: 0, width: WIDTH, height: HEIGHT };
    let index = std::cell::Cell::new(0u32);
    let mut canvas = Canvas::spawn(
        &context,
        Grid { rows: 1, cols: 2 },
        area,
        &[PaneSpec::shell(), PaneSpec::shell()],
        FONT,
        |_spec| {
            // Each child prints a DIFFERENT banner, because the whole claim is positional: two
            // panes drawing the same pixels would pass this test with left and right exchanged.
            let n = index.get();
            index.set(n + 1);
            let mut command = Command::new("/bin/sh");
            command
                .arg("-c")
                .arg(format!("printf 'pane-{n}-{}\\n'; exec cat", "x".repeat(n as usize + 1)));
            command
        },
    )
    .expect("a canvas");

    pump(&mut canvas, Duration::from_secs(10));
    for pane in canvas.panes_mut() {
        pane.session.poll();
    }

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let blitter = ruuah_vt_render::Blitter::new(&context, format).expect("a non-sRGB target");
    let device = context.device();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("canvas target"),
        size: wgpu::Extent3d { width: WIDTH, height: HEIGHT, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    // Where each pane's pixels must land, collected while the surfaces are borrowed: a surface is
    // whole CELLS, so it is at most its rect and the remainder is margin the target clears.
    let mut regions = Vec::new();
    let mut placements = Vec::new();
    for pane in canvas.panes_mut() {
        let rect = pane.rect;
        let surface = pane.session.surface_mut();
        regions.push((rect.x, rect.y, Surface::width(surface), Surface::height(surface)));
        placements.push((surface, (rect.x, rect.y)));
    }
    // Bright green: neither pane draws it, so a region that "matches" by holding clear colour
    // cannot pass unnoticed.
    blitter.blit_all(&mut placements, &view, wgpu::Color { r: 0.0, g: 1.0, b: 0.0, a: 1.0 });
    drop(placements);

    let target = read_back(&context, &texture, WIDTH, HEIGHT);

    let drawn: Vec<Vec<u8>> = canvas
        .panes_mut()
        .iter_mut()
        .map(|pane| pane.session.pixels())
        .collect();
    assert_ne!(
        drawn[0], drawn[1],
        "the two panes drew identical pixels, so this test cannot tell one position from the other"
    );

    for (pane, (x, y, width, height)) in regions.iter().copied().enumerate() {
        for row in 0..height {
            let from = (((y + row) * WIDTH + x) * 4) as usize;
            let got = &target[from..from + (width * 4) as usize];
            let at = (row * width * 4) as usize;
            let want = &drawn[pane][at..at + (width * 4) as usize];
            if got != want {
                let column = got
                    .chunks(4)
                    .zip(want.chunks(4))
                    .position(|(a, b)| a != b)
                    .unwrap_or(0);
                panic!(
                    "pane {pane} row {row} column {column}: the frame holds {:?} where the pane \
                     drew {:?} - the pane is missing, misplaced, or erased by its neighbour",
                    &got[column * 4..column * 4 + 4],
                    &want[column * 4..column * 4 + 4],
                );
            }
        }
    }

    canvas.shutdown();
}

/// Copies a render target back to the CPU. Test scaffolding: the real present path never reads
/// anything back, which is the entire point of blitting on the GPU.
fn read_back(context: &GpuContext, texture: &wgpu::Texture, width: u32, height: u32) -> Vec<u8> {
    let device = context.device();
    let bytes_per_row = width * 4;
    assert_eq!(bytes_per_row % 256, 0, "the target width must keep rows aligned");
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("canvas readback"),
        size: u64::from(bytes_per_row * height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    context.queue().submit(Some(encoder.finish()));

    let slice = readback.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device.poll(wgpu::PollType::Wait).expect("poll the device");
    receiver.recv().expect("map completed").expect("map succeeded");

    let data = slice.get_mapped_range();
    let out = data.to_vec();
    drop(data);
    readback.unmap();
    out
}

/// A resize must reach the children, not only the rects.
///
/// The control for the test above: same canvas, same panes, and the numbers must CHANGE. Without
/// it, a `resize` that updated `pane.rect` and never called the pty would satisfy every
/// geometric assertion in this file.
#[test]
fn a_resize_reaches_the_children() {
    let mut canvas = Canvas::spawn(
        &gpu(),
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
