//! The blit is the half of "put it on a screen" that a machine can check.
//!
//! What it pins: the pixels that reach a render target are the SAME BYTES `read_pixels` would
//! have returned. That single equality covers the two failures that are otherwise silent -
//! colours washed out by a double gamma encode, and red swapped with blue - because both change
//! the bytes while leaving everything running and looking plausible.
//!
//! The fixture is deliberately asymmetric in red versus blue. A grey or symmetric fixture
//! passes just as happily with the channels reversed, which would make this whole file theatre;
//! `the_fixture_can_detect_a_channel_swap` is the control that proves it cannot.

use ruuah_vt_render::{Blitter, GpuContext, GpuSurface, PresentError, Surface};

/// 64 pixels wide so a row is exactly 256 bytes, which is the texture-to-buffer copy alignment
/// wgpu requires. The real present path never copies back at all; this is test scaffolding.
const WIDTH: u32 = 64;
const HEIGHT: u32 = 32;

/// Paints a frame whose channels cannot be confused with each other.
fn paint(surface: &mut GpuSurface) {
    // Opaque background, red-dominant, blue-quiet.
    surface.fill(0, 0, WIDTH, HEIGHT, [200, 40, 10, 255]);
    // A second rectangle the other way round, so both orderings appear in one frame.
    surface.fill(4, 4, 20, 10, [10, 60, 220, 255]);
    // A partial-coverage blend, so the blended path is covered too and not just solid fills.
    let coverage: Vec<u8> = (0..8 * 8).map(|i| (i * 4) as u8).collect();
    surface.blend_mask(30, 12, 8, 8, &coverage, [0, 255, 90, 255]);
}

fn blit_to_texture(context: &GpuContext, surface: &mut GpuSurface) -> Vec<u8> {
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let blitter = Blitter::new(context, format).expect("a non-sRGB target is accepted");

    let device = context.device();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("blit target"),
        size: wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    blitter.blit(surface, &view);

    let bytes_per_row = WIDTH * 4;
    assert_eq!(bytes_per_row % 256, 0, "the test width must keep rows aligned");
    let size = (bytes_per_row * HEIGHT) as u64;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("blit readback"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(HEIGHT),
            },
        },
        wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
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

#[test]
fn the_blitted_frame_is_byte_identical_to_the_read_back_frame() {
    let context = GpuContext::new().expect("a GPU");
    let mut surface = GpuSurface::with_context(context.clone(), WIDTH, HEIGHT).expect("a surface");
    paint(&mut surface);

    // Blit BEFORE reading back: that is the real order of use, and it proves the blit does not
    // depend on someone else having flushed the surface first.
    let presented = blit_to_texture(&context, &mut surface);
    let read_back = surface.read_pixels();

    assert_eq!(
        presented.len(),
        read_back.len(),
        "the two paths disagree on frame size"
    );
    assert_eq!(
        presented, read_back,
        "the pixels that reached the render target are not the pixels read_pixels returns"
    );
}

#[test]
fn the_fixture_can_detect_a_channel_swap() {
    // The control for the test above. If the fixture were grey or symmetric, a shader that
    // unpacked blue where red belongs would produce identical bytes and the equality assertion
    // would pass while the screen showed the wrong colours. Swapping the channels of the real
    // frame must therefore produce something DIFFERENT.
    let context = GpuContext::new().expect("a GPU");
    let mut surface = GpuSurface::with_context(context.clone(), WIDTH, HEIGHT).expect("a surface");
    paint(&mut surface);
    let frame = surface.read_pixels();

    let swapped: Vec<u8> = frame
        .chunks_exact(4)
        .flat_map(|p| [p[2], p[1], p[0], p[3]])
        .collect();

    assert_ne!(
        frame, swapped,
        "the fixture is symmetric in red and blue, so the equality test above cannot catch a \
         channel swap and proves nothing"
    );
}

#[test]
fn an_srgb_target_is_refused_rather_than_silently_compensated() {
    let context = GpuContext::new().expect("a GPU");
    let error = Blitter::new(&context, wgpu::TextureFormat::Rgba8UnormSrgb)
        .expect_err("an sRGB target must be refused");
    // Named rather than wildcarded: the refusal must be THIS refusal. A blitter that failed
    // for some unrelated reason would otherwise satisfy an `is_err` assertion and the guard
    // would never be exercised at all.
    match error {
        PresentError::SrgbTarget(format) => {
            assert_eq!(format, wgpu::TextureFormat::Rgba8UnormSrgb)
        }
        other => panic!("expected the sRGB refusal, got {other:?}"),
    }
}

#[test]
fn a_non_srgb_target_is_accepted() {
    // The other direction of the guard. Without this, a Blitter::new that refused EVERY format
    // would pass the test above and no one would notice until nothing ever drew.
    let context = GpuContext::new().expect("a GPU");
    Blitter::new(&context, wgpu::TextureFormat::Bgra8Unorm)
        .expect("the format a macOS swapchain actually offers must be accepted");
}
