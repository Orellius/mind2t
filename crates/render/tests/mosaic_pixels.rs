//! The renderer-level proof for synthesized mosaics: ink through the REAL draw
//! path, filling the whole cell with no gutter at the seam -- the exact failure
//! a narrow fallback font produces (its block fills the font's em, not the cell).
use ruuah_vt_core::Terminal;
use ruuah_vt_frame::{Frame, Publisher, channel};
use ruuah_vt_render::{FontStack, Renderer};

/// Renders on a 3-column row: the probe text in cells 0..2, cell 2 untouched so
/// the background is sampled OUTSIDE any ink.
fn pixels_of(text: &str) -> (Vec<u8>, u32, u32, u32) {
    let cols = 3u16;
    let mut terminal = Terminal::new(cols, 1);
    terminal.write(format!("\u{1b}[?25l{text}").as_bytes());
    let (writer, reader) = channel(cols, 1);
    let mut publisher = Publisher::new(writer);
    publisher.publish(&mut terminal).expect("publish");
    let mut frame = Frame::new();
    reader.read_into(&mut frame);
    let fonts = FontStack::system(16.0).expect("fonts");
    let cell_width = fonts.metrics().width;
    let mut renderer = Renderer::new(fonts, cols, 1);
    renderer.draw_all(&frame);
    let (w, h) = (renderer.canvas().width(), renderer.canvas().height());
    (renderer.pixels(), w, h, cell_width)
}

fn at(pixels: &[u8], width: u32, x: u32, y: u32) -> (u8, u8, u8) {
    let i = ((y * width + x) * 4) as usize;
    (pixels[i], pixels[i + 1], pixels[i + 2])
}

#[test]
fn two_full_blocks_leave_no_gutter_anywhere() {
    let (pixels, width, height, cell) = pixels_of("██");
    // Background sampled from the third, empty cell -- never from inside the ink.
    let bg = at(&pixels, width, width - 1, height / 2);
    let mut gaps = 0usize;
    for y in 0..height {
        for x in 0..cell * 2 {
            if at(&pixels, width, x, y) == bg {
                gaps += 1;
            }
        }
    }
    assert_eq!(gaps, 0, "{gaps} background pixels inside two full blocks");
}

#[test]
fn a_sextant_fills_its_cells_at_cell_geometry() {
    // U+1FB27 -> id 41 = bits 0,3,5: top-left, middle-right, bottom-right.
    let (pixels, width, height, cell) = pixels_of("\u{1FB27}");
    let bg = at(&pixels, width, width - 1, height / 2);
    assert_ne!(at(&pixels, width, 0, 0), bg, "top-left sixth is ink");
    assert_eq!(at(&pixels, width, 0, height - 1), bg, "bottom-left sixth empty");
    assert_ne!(
        at(&pixels, width, cell - 1, height - 1),
        bg,
        "bottom-right sixth reaches the cell's right edge -- a font-drawn narrow \
         glyph fails exactly here"
    );
    assert_eq!(at(&pixels, width, cell - 1, 0), bg, "top-right sixth empty");
}
