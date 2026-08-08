//! VS16 emoji presentation: U+2764 U+FE0F asks for the EMOJI face of a character the
//! text fonts also cover. The width stays 1 (oracle-measured, corpus-pinned as
//! vs16-cluster-stays-narrow); what changes is the face choice -- the cluster must
//! reach Apple Color Emoji and land as CHROMATIC ink inside its single cell, scaled to
//! fit the cell's width so it cannot bleed over a neighbor.
//!
//! Chromatic = a pixel whose channels differ (the emoji_probe.rs rule): a mask glyph
//! tinted with the white foreground can never produce it.

use mind2t_vt_core::Terminal;
use mind2t_vt_frame::{Frame, Publisher, channel};
use mind2t_vt_render::{FontStack, Renderer};

const COLS: u16 = 6;
const SIZE: f32 = 24.0;

#[test]
fn a_vs16_cluster_draws_chromatic_ink_inside_its_cell() {
    let mut terminal = Terminal::new(COLS, 1);
    terminal.write(b"\x1b[?25l");
    terminal.write("a\u{2764}\u{FE0F}b".as_bytes());
    let (writer, reader) = channel(COLS, 1);
    let mut publisher = Publisher::new(writer);
    publisher.publish(&mut terminal).expect("fits");
    let mut frame = Frame::new();
    reader.read_into(&mut frame);

    let fonts = FontStack::system(SIZE).expect("fonts");
    let metrics = fonts.metrics();
    let mut renderer = Renderer::new(fonts, COLS, 1);
    renderer.draw_all(&frame);
    let canvas = renderer.canvas();

    let chromatic_in = |cell: u32| -> u32 {
        let mut found = 0;
        for y in 0..metrics.height {
            for x in 0..metrics.width {
                let px = canvas.pixel(cell * metrics.width + x, y);
                let (r, g, b) = (px[0] as i32, px[1] as i32, px[2] as i32);
                if (r - g).abs() > 24 || (g - b).abs() > 24 || (r - b).abs() > 24 {
                    found += 1;
                }
            }
        }
        found
    };
    assert!(
        chromatic_in(1) > 0,
        "the VS16 heart must reach the color face -- no chromatic ink in its cell"
    );
    assert_eq!(
        chromatic_in(2),
        0,
        "width is 1: nothing chromatic may bleed into b's cell"
    );
    assert_eq!(chromatic_in(0), 0, "or backwards into a's");
}
