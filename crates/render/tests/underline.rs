//! Styled underlines: the state machine has carried SGR 4:n and 58/59 since slice 1,
//! the comparator compares them, and the renderer then collapsed all five kinds into
//! one foreground-colored line -- a curly red underline drew exactly like a plain one,
//! and no existing test could tell (state tests stop at the Snapshot; the pixel harness
//! only checks that ink equals ink).
//!
//! These tests are positional and differential: the decoration band below the glyph
//! must wear the UNDERLINE color when one is set, and the five kinds must produce
//! distinguishable pixel patterns (dotted strictly less ink than single, double strictly
//! more, curly reaching rows single never touches). Seen red against the collapsed
//! renderer, 2026-07-30.

use ruuah_vt_core::Terminal;
use ruuah_vt_frame::{Frame, Publisher, channel};
use ruuah_vt_render::{FontStack, Renderer};

const COLS: u16 = 3;
const SIZE: f32 = 32.0;

const RED: [u8; 4] = [255, 0, 0, 255];

struct Band {
    /// Ink pixels per canvas row across the first cell, decorated rows only counted
    /// where they differ from background.
    rows: Vec<u32>,
    red: u32,
}

/// Renders `sgr` + "x" and measures the first cell: per-row ink counts and how many
/// pixels wear pure red.
fn band(sgr: &str) -> Band {
    let mut terminal = Terminal::new(COLS, 1);
    terminal.write(b"\x1b[?25l");
    terminal.write(sgr.as_bytes());
    terminal.write(b"x");

    let (writer, reader) = channel(COLS, 1);
    let mut publisher = Publisher::new(writer);
    publisher.publish(&mut terminal).expect("fits");
    let mut frame = Frame::new();
    reader.read_into(&mut frame);

    let fonts = FontStack::system(SIZE).expect("system fonts");
    let metrics = fonts.metrics();
    let mut renderer = Renderer::new(fonts, COLS, 1);
    renderer.draw_all(&frame);

    let background = renderer.palette().default_background;
    let canvas = renderer.canvas();
    let mut rows = vec![0u32; metrics.height as usize];
    let mut red = 0;
    for y in 0..metrics.height {
        for x in 0..metrics.width {
            let pixel = canvas.pixel(x, y);
            if pixel != background {
                rows[y as usize] += 1;
            }
            if pixel == RED {
                red += 1;
            }
        }
    }
    Band { rows, red }
}

fn total(band: &Band) -> u32 {
    band.rows.iter().sum()
}

#[test]
fn an_underline_color_reaches_the_pixels() {
    let plain = band("\x1b[4m");
    let colored = band("\x1b[4m\x1b[58;2;255;0;0m");
    assert_eq!(plain.red, 0, "no underline color set, no red ink");
    assert!(
        colored.red > 0,
        "SGR 58 sets the underline color and the decoration must wear it"
    );
    // The glyph itself keeps the foreground: the red ink must be at most the size of
    // the decoration, not the whole cell's ink.
    assert!(
        colored.red < total(&colored),
        "only the decoration is red, never the glyph"
    );
}

#[test]
fn the_five_kinds_draw_distinguishable_decorations() {
    let none = band("");
    let single = band("\x1b[4m");
    let double = band("\x1b[4:2m");
    let curly = band("\x1b[4:3m");
    let dotted = band("\x1b[4:4m");
    let dashed = band("\x1b[4:5m");

    let deco = |kind: &Band| total(kind).saturating_sub(total(&none));
    assert!(deco(&single) > 0, "single adds ink");
    assert!(
        deco(&double) > deco(&single),
        "double is two bars: strictly more ink than single ({} vs {})",
        deco(&double),
        deco(&single)
    );
    assert!(
        deco(&dotted) < deco(&single),
        "dotted has gaps: strictly less ink than single ({} vs {})",
        deco(&dotted),
        deco(&single)
    );
    assert!(
        deco(&dashed) < deco(&single) && deco(&dashed) > deco(&dotted),
        "dashed sits between dotted and single ({} / {} / {})",
        deco(&dotted),
        deco(&dashed),
        deco(&single)
    );
    // Curly must reach rows a straight line never touches -- amplitude, not just ink.
    let occupied = |kind: &Band| -> Vec<usize> {
        kind.rows
            .iter()
            .enumerate()
            .filter(|&(row, &count)| count > none.rows[row])
            .map(|(row, _)| row)
            .collect()
    };
    let single_rows = occupied(&single);
    let curly_rows = occupied(&curly);
    assert!(
        curly_rows.iter().any(|row| !single_rows.contains(row)),
        "curly reaches rows the straight line does not: {curly_rows:?} vs {single_rows:?}"
    );
}
