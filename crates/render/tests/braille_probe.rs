//! The PIXEL pin for braille coverage. The resolve-level test in font.rs passed while
//! the app still drew nothing (stale link, and resolve is not ink) -- this one asserts
//! the only thing that matters: U+28FF leaves ink on the canvas.

use mind2t_vt_core::Terminal;
use mind2t_vt_frame::{Frame, Publisher, channel};
use mind2t_vt_render::{FontStack, Renderer};

#[test]
fn a_full_braille_cell_leaves_ink() {
    let mut terminal = Terminal::new(4, 1);
    terminal.write("\u{1b}[?25l\u{28FF}".as_bytes());
    let (writer, reader) = channel(4, 1);
    let mut publisher = Publisher::new(writer);
    publisher.publish(&mut terminal).expect("publish");
    let mut frame = Frame::new();
    reader.read_into(&mut frame);
    let fonts = FontStack::system(16.0).expect("fonts");
    let mut renderer = Renderer::new(fonts, 4, 1);
    renderer.draw_all(&frame);
    let pixels = renderer.pixels();
    let bg = (pixels[0], pixels[1], pixels[2]);
    let ink = pixels.chunks(4).filter(|p| (p[0], p[1], p[2]) != bg).count();
    assert!(ink > 0, "U+28FF drew zero ink pixels");
    println!("ink pixels: {ink}");
}
