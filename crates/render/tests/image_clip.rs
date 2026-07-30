//! Top-edge clipping for placements: an anchor at a negative row draws its visible
//! remainder, per-pixel clipped by both backends. The discriminator is two-tone -- an
//! image red on top, blue below, placed at row -1 with a two-row span: the canvas must
//! show the BLUE half at its top and contain no pure red anywhere. A renderer that
//! clamps the anchor to row 0 (the old amputation, inverted) paints red at the top and
//! fails; one that drops the placement paints nothing and fails.

use std::sync::Arc;

use ruuah_vt_frame::FramePlacement;
use ruuah_vt_render::{FontStack, Renderer};

#[test]
fn a_negative_row_placement_draws_its_bottom_half_only() {
    let fonts = FontStack::system(16.0).expect("fonts");
    let metrics = fonts.metrics();
    let mut renderer = Renderer::new(fonts, 4, 2);

    // 1x2: red over blue. Scaled to (cellw, 2*cellh); placed at row -1 the red half
    // sits above the canvas.
    let rgba: Arc<Vec<u8>> = Arc::new(vec![255, 0, 0, 255, 0, 0, 255, 255]);
    let placements = [FramePlacement {
        image: 1,
        col: 0,
        row: -1,
        cols: 1,
        rows: 2,
    }];
    renderer.draw_images(&placements, |_| Some((1, 2, Arc::clone(&rgba))));

    let canvas = renderer.canvas();
    let mut red = 0u32;
    let mut blue_top = 0u32;
    for y in 0..metrics.height * 2 {
        for x in 0..metrics.width * 4 {
            let px = canvas.pixel(x, y);
            if px[0] > 200 && px[2] < 60 {
                red += 1;
            }
            if y < metrics.height && px[2] > 200 && px[0] < 60 {
                blue_top += 1;
            }
        }
    }
    assert_eq!(red, 0, "the red half is above the canvas and must not appear");
    assert!(
        blue_top > 0,
        "the blue half is the visible remainder and must reach the canvas top"
    );
}
