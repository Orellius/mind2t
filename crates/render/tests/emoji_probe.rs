//! The PIXEL pin for color emoji (P0.2). The braille lesson applies squared here: a
//! resolve-level test proves a font answered, not that ink landed -- and for emoji even
//! INK is not enough, because a color glyph pushed through the mask path draws a gray
//! silhouette tinted with the foreground. The only observable that separates a working
//! sbix blit from every broken shape is CHROMATIC ink: pixels whose channels differ.
//!
//! U+1F9E0 (brain) is the pin deliberately -- `[🧠 BRAIN]` in Claude Code inside Mind2t
//! was the live gap that opened this backlog item (2026-07-29).

use mind2t_vt_core::Terminal;
use mind2t_vt_frame::{Frame, Publisher, channel};
use mind2t_vt_render::{FontStack, Renderer};

fn render(text: &str) -> Vec<u8> {
    let mut terminal = Terminal::new(4, 1);
    terminal.write(format!("\u{1b}[?25l{text}").as_bytes());
    let (writer, reader) = channel(4, 1);
    let mut publisher = Publisher::new(writer);
    publisher.publish(&mut terminal).expect("publish");
    let mut frame = Frame::new();
    reader.read_into(&mut frame);
    let fonts = FontStack::system(16.0).expect("fonts");
    let mut renderer = Renderer::new(fonts, 4, 1);
    renderer.draw_all(&frame);
    renderer.pixels()
}

/// Chromatic = the channels disagree. The background, a white-tinted mask, and "nothing
/// drawn" are all achromatic, so every broken path scores zero here.
fn chromatic(pixels: &[u8]) -> usize {
    pixels
        .chunks(4)
        .filter(|p| p[0] != p[1] || p[1] != p[2])
        .count()
}

#[test]
fn a_brain_emoji_leaves_chromatic_ink() {
    let pixels = render("\u{1F9E0}");
    let colored = chromatic(&pixels);
    assert!(
        colored > 0,
        "U+1F9E0 drew zero chromatic pixels -- the color path is not live"
    );
    println!("chromatic pixels: {colored}");
}

#[test]
fn plain_text_stays_achromatic() {
    // The probe's own control: white-on-near-black 'W' must score zero, or the chromatic
    // counter is measuring something other than color glyphs and the test above proves
    // nothing.
    let pixels = render("W");
    assert_eq!(
        chromatic(&pixels),
        0,
        "the default scheme draws gray-on-gray text; chromatic ink here means the probe is broken"
    );
}
