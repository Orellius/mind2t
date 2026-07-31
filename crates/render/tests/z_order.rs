//! The blind spot images-v3a opens: NOTHING COULD SEE DRAW ORDER.
//!
//! Every image test so far places one image, or places two that do not overlap, so a
//! renderer with the ordering reversed -- or with no ordering at all -- painted pixels
//! indistinguishable from a correct one. `image_clip.rs` is about geometry, `backend.rs`
//! about CPU/GPU agreement; neither can tell which of two images is on top, and none of
//! them can tell whether an image is over or under the text.
//!
//! So every assertion here is about OVERLAP. Two placements share cells and the question
//! is only which colour survives, which is the one thing a wrong layer order changes.
//!
//! The rules under test come from the oracle's renderer (`../ruuah/src/renderer/image.zig`,
//! the sort at line 359 and `bg_limit = i32::MIN / 2`): placements draw in `(z, image id)`
//! order, in three bands -- under the cell background, under the text, over everything.

use std::sync::Arc;

use ruuah_vt_core::Terminal;
use ruuah_vt_frame::{Frame, FramePlacement, Publisher, ReadOutcome, channel};
use ruuah_vt_render::{FontStack, Renderer};

const COLS: u16 = 6;
const ROWS: u16 = 2;

fn fonts() -> FontStack {
    FontStack::system(16.0).expect("system fonts")
}

/// A 1x1 opaque image of one colour, as the renderer's `resolved` tuple expects it.
fn swatch(r: u8, g: u8, b: u8) -> Option<(u32, u32, Arc<Vec<u8>>)> {
    Some((1, 1, Arc::new(vec![r, g, b, 255])))
}

/// Paints `bytes` through the real core and publisher, then draws every layer.
///
/// The core and publisher are in the loop deliberately: `z` is parsed there and the draw
/// ORDER is decided there (the publisher sorts, because the host resolves image pixels
/// positionally and a renderer that re-sorted would pair placements with the wrong bytes).
/// A test that hand-built its placements would prove the renderer draws a list in order
/// and nothing about whether the list is ever built in the right one.
fn painted(
    bytes: &[u8],
    images: &[Option<(u32, u32, Arc<Vec<u8>>)>],
) -> (Vec<u8>, Vec<i32>, Vec<u32>) {
    let mut terminal = Terminal::new(COLS, ROWS);
    let (writer, reader) = channel(COLS, ROWS);
    let mut publisher = Publisher::new(writer);
    let mut frame = Frame::new();
    let mut renderer = Renderer::new(fonts(), COLS, ROWS);

    terminal.write(bytes);
    publisher.publish(&mut terminal).expect("fits");
    assert!(matches!(
        reader.read_into(&mut frame),
        ReadOutcome::Fresh(_)
    ));

    let placements = frame.placements.clone();
    assert_eq!(
        placements.len(),
        images.len(),
        "the test must supply one image per placement, in the frame's own order"
    );
    renderer.draw_layered(&frame, &placements, images);
    (
        renderer.canvas().pixels().to_vec(),
        placements.iter().map(|p| p.z).collect(),
        placements.iter().map(|p| p.image).collect(),
    )
}

/// Transmit-and-display one 1x1 image spanning `c` by `r` cells at `z`, anchored at the
/// top-left cell.
///
/// The leading CUP is load-bearing and cost this file three failing tests: a placement
/// with an explicit span STEPS THE CURSOR PAST ITSELF, kitty-style, so two placements
/// emitted back to back land in different columns and never overlap. An overlap test whose
/// images do not overlap passes for every possible draw order.
fn place(id: u32, cols: u16, rows: u16, z: i32) -> Vec<u8> {
    // A single opaque pixel, base64: the payload never reaches these tests' pixels (the
    // renderer is handed the colours directly) but it must decode or no placement exists.
    format!("\x1b[1;1H\x1b_Ga=T,f=32,s=1,v=1,i={id},c={cols},r={rows},z={z};AAAA/w==\x1b\\")
        .into_bytes()
}

fn pixel(canvas: &[u8], x: u32, y: u32) -> [u8; 4] {
    let width = u32::from(COLS) * fonts().metrics().width;
    let base = ((y * width + x) * 4) as usize;
    [
        canvas[base],
        canvas[base + 1],
        canvas[base + 2],
        canvas[base + 3],
    ]
}

/// A point inside cell (0,0), away from the edges so glyph ink cannot be mistaken for it.
fn probe() -> (u32, u32) {
    let metrics = fonts().metrics();
    (metrics.width / 2, metrics.height / 2)
}

#[test]
fn a_higher_z_draws_over_a_lower_one() {
    let mut bytes = place(1, 2, 1, 1);
    bytes.extend(place(2, 2, 1, 5));
    let (canvas, order, _ids) = painted(&bytes, &[swatch(255, 0, 0), swatch(0, 0, 255)]);

    assert_eq!(order, vec![1, 5], "the publisher hands over z-sorted");
    let (x, y) = probe();
    assert_eq!(pixel(&canvas, x, y)[2], 255, "the z=5 blue must be on top");
}

/// The same two images with their z values swapped. Without this the test above passes for
/// a renderer that always draws the LAST transmitted image on top, which is what the old
/// unordered code did and is right half the time by luck.
#[test]
fn the_order_follows_z_and_not_arrival() {
    let mut bytes = place(1, 2, 1, 5);
    bytes.extend(place(2, 2, 1, 1));
    let (canvas, order, _ids) = painted(&bytes, &[swatch(0, 0, 255), swatch(255, 0, 0)]);

    assert_eq!(order, vec![1, 5], "sorted, so the LATER placement is drawn first");
    let (x, y) = probe();
    assert_eq!(
        pixel(&canvas, x, y)[0],
        255,
        "the z=5 image arrived first and must still be on top"
    );
}

/// Equal z falls back to image id, then to placement order -- the oracle's tiebreak.
///
/// Asserts the ORDER, not only the surviving colour. The colour alone is not
/// discriminating here: with the sort removed, the arrival order happens to put the same
/// colour on top, so the pixel assertion passed against a renderer doing no sorting at all.
/// Caught by mutation, which is the only reason this test is worth its line count.
#[test]
fn equal_z_breaks_the_tie_by_image_id() {
    let mut bytes = place(7, 2, 1, 3);
    bytes.extend(place(2, 2, 1, 3));
    // Sorted by id at equal z, so image 2 is drawn first and image 7 lands on top.
    let (canvas, _, ids) = painted(&bytes, &[swatch(0, 0, 255), swatch(255, 0, 0)]);

    assert_eq!(ids, vec![2, 7], "equal z sorts by image id, lowest drawn first");
    let (x, y) = probe();
    assert_eq!(pixel(&canvas, x, y)[0], 255, "the higher image id wins a z tie");
}

/// The layer that motivated splitting the row pass in two.
#[test]
fn a_negative_z_image_draws_under_the_text() {
    let mut bytes = b"\x1b[1;1HX".to_vec();
    bytes.extend(place(1, 2, 1, -1));
    let (under, _, _) = painted(&bytes, &[swatch(255, 0, 0)]);

    let mut bytes = b"\x1b[1;1HX".to_vec();
    bytes.extend(place(1, 2, 1, 0));
    let (over, _, _) = painted(&bytes, &[swatch(255, 0, 0)]);

    let metrics = fonts().metrics();
    let ink = |canvas: &[u8]| -> u32 {
        let mut count = 0;
        for y in 0..metrics.height {
            for x in 0..metrics.width {
                // Anything that is not the flat red of the image is glyph ink showing.
                let px = pixel(canvas, x, y);
                if px[0] != 255 || px[1] != 0 || px[2] != 0 {
                    count += 1;
                }
            }
        }
        count
    };

    assert!(ink(&under) > 0, "at z=-1 the glyph must survive over the image");
    assert_eq!(ink(&over), 0, "at z=0 the image must bury the glyph");
}

/// The deepest band draws BEFORE the backgrounds, so on an empty cell it is visible only
/// because the default background yields to it. Without that skip the row clear would
/// paint over the image before a glyph was drawn and the whole band would be dead code.
#[test]
fn a_below_background_image_shows_through_a_default_background_cell() {
    let below_bg = i32::MIN / 2 - 1;
    let (deep, _, _) = painted(&place(1, 2, 1, below_bg), &[swatch(255, 0, 0)]);

    let (x, y) = probe();
    assert_eq!(
        pixel(&deep, x, y)[0],
        255,
        "below the background, the image must be what an empty cell shows"
    );
}

/// The two lower bands are indistinguishable on an empty cell -- both end up visible --
/// and diverge only where the child COLOURED the background. That is the discriminator,
/// and getting it wrong is what made the first version of the test above assert the
/// opposite of the truth.
#[test]
fn a_coloured_background_separates_the_two_lower_bands() {
    let below_bg = i32::MIN / 2 - 1;
    let coloured = |z: i32| {
        // Blue background on the cell, then the image anchored over it.
        let mut bytes = b"\x1b[1;1H\x1b[44m \x1b[0m".to_vec();
        bytes.extend(place(1, 2, 1, z));
        painted(&bytes, &[swatch(255, 0, 0)]).0
    };

    let (x, y) = probe();
    assert_ne!(
        pixel(&coloured(below_bg), x, y)[0],
        255,
        "below the BACKGROUND, a cell the child coloured is opaque over the image"
    );
    assert_eq!(
        pixel(&coloured(-1), x, y)[0],
        255,
        "below the TEXT, the image covers that same coloured background"
    );
}

/// Hand-built placements, because the point is the boundary VALUES rather than the parse.
#[test]
fn the_layer_boundaries_sit_exactly_where_the_oracle_puts_them() {
    let at = |z: i32| FramePlacement {
        image: 1,
        col: 0,
        row: 0,
        cols: 1,
        rows: 1,
        z,
    };
    assert_eq!(at(i32::MIN).layer(), 0);
    assert_eq!(at(FramePlacement::BELOW_BACKGROUND - 1).layer(), 0);
    assert_eq!(at(FramePlacement::BELOW_BACKGROUND).layer(), 1, "the bound is exclusive");
    assert_eq!(at(-1).layer(), 1);
    assert_eq!(at(0).layer(), 2, "zero is the DEFAULT layer, over the text");
    assert_eq!(at(i32::MAX).layer(), 2);
}
