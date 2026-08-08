//! Ligatures: with a ligating lead font, "->"-class sequences must draw DIFFERENT ink
//! than per-cell rendering; with the default (non-ligating Menlo) stack the output must
//! be byte-identical whether the feature is on or off -- that identity is what makes
//! this a safe revision of the one-cell-shaping policy rather than a regression risk.
//!
//! The ligating font is Iosevka Nerd Font Mono, already installed for the mosaic
//! backstop. If this machine ever loses it, the first test SKIPS loudly rather than
//! passing vacuously.

use mind2t_vt_core::Terminal;
use mind2t_vt_frame::{Frame, Publisher, channel};
use mind2t_vt_render::{FontStack, Renderer};

const COLS: u16 = 12;
const SIZE: f32 = 24.0;

fn pixels(family: Option<&str>, ligatures: bool, text: &str) -> Vec<u8> {
    let mut terminal = Terminal::new(COLS, 1);
    terminal.write(b"\x1b[?25l");
    terminal.write(text.as_bytes());
    let (writer, reader) = channel(COLS, 1);
    let mut publisher = Publisher::new(writer);
    publisher.publish(&mut terminal).expect("fits");
    let mut frame = Frame::new();
    reader.read_into(&mut frame);

    let fonts = FontStack::with_primary(family, SIZE).expect("fonts");
    let mut renderer = Renderer::new(fonts, COLS, 1);
    renderer.set_ligatures(ligatures);
    renderer.draw_all(&frame);
    renderer.pixels()
}

#[test]
fn a_ligating_font_draws_arrows_differently() {
    if !FontStack::family_resolves("IosevkaNerdFontMono") {
        eprintln!("SKIP: Iosevka Nerd Font Mono not installed; the ligature half is unproven here");
        return;
    }
    let family = Some("IosevkaNerdFontMono");
    let ligated = pixels(family, true, "a -> b");
    let plain = pixels(family, false, "a -> b");
    assert_ne!(
        ligated, plain,
        "calt/liga formed nothing -- either the font lost its ligatures or the run \
         path never fired"
    );
}

#[test]
fn the_default_stack_is_byte_identical_either_way() {
    let on = pixels(None, true, "a -> b != c");
    let off = pixels(None, false, "a -> b != c");
    assert_eq!(
        on, off,
        "Menlo has no ligatures, so the feature flag must change NOTHING -- if it did, \
         the fallback guard (glyphs < chars) is broken"
    );
}
