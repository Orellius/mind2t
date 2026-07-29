//! UBA rule L4: a paired-punctuation glyph at an RTL resolved level renders MIRRORED.
//! Found live on 2026-07-29: the first auto-direction window drew `[OK]` as `]OK[` --
//! reordering swapped the brackets' POSITIONS, and nothing swapped their GLYPHS.
//!
//! The oracle needs no reimplementation: an RTL row holding `א[` must land `]` at the
//! visual left column and `א` beside it -- which is byte-for-byte what an LTR row holding
//! `]א` draws. If mirroring is absent the RTL row draws `[` there instead, and the byte
//! comparison fails; the LTR-vs-RTL control proves the comparison sees layout at all.

use ruuah_vt_core::Terminal;
use ruuah_vt_frame::{BaseDirection, Frame, Publisher, channel};
use ruuah_vt_render::{FontStack, Renderer};

const COLS: u16 = 8;
const ROWS: u16 = 2;
const FONT_SIZE: f32 = 16.0;

fn pixels_of(text: &str, base: BaseDirection) -> Vec<u8> {
    let mut terminal = Terminal::new(COLS, ROWS);
    terminal.write(text.as_bytes());

    let (writer, reader) = channel(COLS, ROWS);
    let mut publisher = Publisher::new(writer);
    publisher.publish(&mut terminal).expect("publish");

    let mut frame = Frame::new();
    frame.base_direction = base;
    reader.read_into(&mut frame);
    assert!(frame.is_valid(), "single-threaded read cannot tear");

    let fonts = FontStack::system(FONT_SIZE).expect("system fonts");
    let mut renderer = Renderer::new(fonts, COLS, ROWS);
    renderer.draw_all(&frame);
    renderer.pixels()
}

/// DECTCEM hide: the caret's visual column differs between the two constructions and is
/// not what this oracle is about, so both frames render without it.
const HIDE: &str = "\u{1b}[?25l";

#[test]
fn a_bracket_in_an_rtl_run_draws_its_mirrored_glyph() {
    // First strong character is Hebrew, so Auto resolves the whole row RTL: the trailing
    // blanks land at the visual LEFT and the content anchors to the RIGHT grid edge --
    // aleph at the last column, the bracket beside it.
    let rtl = pixels_of(&format!("{HIDE}א["), BaseDirection::Auto);
    // The hand-built expectation, laid out LTR at the same columns: six spaces, then the
    // `]` glyph, then the aleph.
    let want = pixels_of(&format!("{HIDE}      ]א"), BaseDirection::LeftToRight);
    assert_eq!(
        rtl, want,
        "an RTL `[` must draw the `]` glyph (UBA L4); it drew something else"
    );
}

#[test]
fn the_comparison_itself_can_fail() {
    // Control: the same content under the two bases lays out differently, so the byte
    // comparison the passing test relies on is provably sensitive to layout.
    let rtl = pixels_of(&format!("{HIDE}א["), BaseDirection::Auto);
    let ltr = pixels_of(&format!("{HIDE}א["), BaseDirection::LeftToRight);
    assert_ne!(rtl, ltr, "the two bases drew identically -- the oracle is blind");
}
