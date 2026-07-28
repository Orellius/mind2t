//! The blind spot slice 5.5b opens: nothing could see where a combining mark LANDS.
//!
//! Every test before this one is satisfied by a renderer that draws a niqqud at the cell's
//! left edge, because the mark's ink is still ink, still inside the cell, and still changes
//! the pixels. Which is exactly what the renderer did: measured at 32px, an unshaped
//! `בָּ` puts 9 pixels of ink in column 0 of a 19-pixel cell, and a shaped one puts none --
//! the marks were sitting on the pen origin instead of on the letter.
//!
//! So the assertion is positional and two-directional in the same test: with shaping the
//! leftmost columns of the cell must be EMPTY, and with shaping switched off they must not be.
//! The control is not decorative -- without it, "the mark is somewhere in the cell" would pass
//! for both the working and the broken renderer.

use ruuah_vt_core::Terminal;
use ruuah_vt_frame::{Frame, Publisher, channel};
use ruuah_vt_render::{FontStack, Renderer};

const COLS: u16 = 4;
/// Large enough that a mark is several pixels wide, so "no ink here" is a real measurement
/// rather than a rounding artefact.
const SIZE: f32 = 32.0;

/// Per-column ink counts inside one cell, and the vertical extent of the ink.
struct Ink {
    columns: Vec<u32>,
    lowest: u32,
}

fn ink(text: &str, cell: u16, shaping: bool) -> Ink {
    let mut terminal = Terminal::new(COLS, 1);
    terminal.write(b"\x1b[?25l"); // the cursor would otherwise paint the cell we measure
    terminal.write(text.as_bytes());

    let (writer, reader) = channel(COLS, 1);
    let mut publisher = Publisher::new(writer);
    publisher.publish(&mut terminal).expect("fits");
    let mut frame = Frame::new();
    reader.read_into(&mut frame);

    let fonts = FontStack::system(SIZE).expect("system fonts");
    let metrics = fonts.metrics();
    let mut renderer = Renderer::new(fonts, COLS, 1);
    renderer.set_shaping_for_testing(shaping);
    renderer.draw_all(&frame);

    let background = renderer.palette().default_background;
    let canvas = renderer.canvas();
    let origin = u32::from(cell) * metrics.width;

    let mut columns = vec![0u32; metrics.width as usize];
    let mut lowest = 0;
    for y in 0..metrics.height {
        for x in 0..metrics.width {
            if canvas.pixel(origin + x, y) != background {
                columns[x as usize] += 1;
                lowest = lowest.max(y);
            }
        }
    }
    Ink { columns, lowest }
}

/// Hebrew clusters that carry marks: bet with dagesh and qamats, shin with its dot and qamats.
const POINTED: [&str; 2] = ["\u{05D1}\u{05BC}\u{05B8}", "\u{05E9}\u{05C1}\u{05B8}"];

#[test]
fn a_mark_is_placed_by_the_font_and_not_dumped_at_the_pen() {
    for cluster in POINTED {
        let shaped = ink(cluster, 0, true);
        let unshaped = ink(cluster, 0, false);

        let edge = |ink: &Ink| ink.columns[0] + ink.columns[1];

        assert_eq!(
            edge(&shaped),
            0,
            "shaped {cluster:?} still has ink at the cell's left edge: {:?}",
            shaped.columns
        );
        assert!(
            edge(&unshaped) > 0,
            "the control did not misplace the mark, so this test cannot tell a working GPOS \
             lookup from an ignored one: {:?}",
            unshaped.columns
        );
    }
}

#[test]
fn the_marks_are_actually_drawn_and_sit_below_the_base() {
    // Guards the opposite failure: a renderer that "fixes" placement by dropping the marks
    // altogether would satisfy the emptiness assertion above perfectly.
    for cluster in POINTED {
        let base: String = cluster.chars().next().unwrap().to_string();
        let pointed = ink(cluster, 0, true);
        let bare = ink(&base, 0, true);

        assert!(
            pointed.lowest > bare.lowest,
            "{cluster:?} reaches no lower than its base alone, so the qamats was not drawn \
             ({} vs {})",
            pointed.lowest,
            bare.lowest
        );
        let total = |ink: &Ink| ink.columns.iter().sum::<u32>();
        assert!(
            total(&pointed) > total(&bare),
            "the marks added no ink at all"
        );
    }
}

#[test]
fn a_pointed_cluster_still_occupies_exactly_one_cell() {
    // The terminal invariant. A cluster is one cell however many codepoints it holds, and a
    // mark that overhung into the next column would corrupt whatever the program drew there.
    for cluster in POINTED {
        let neighbour = ink(cluster, 1, true);
        assert_eq!(
            neighbour.columns.iter().sum::<u32>(),
            0,
            "{cluster:?} spilled into the next cell: {:?}",
            neighbour.columns
        );
    }
}

#[test]
fn a_cell_with_no_marks_renders_the_same_either_way() {
    // Shaping is skipped for single-codepoint clusters, which is nearly every cell on screen.
    // This pins that the skip is not merely an optimisation but produces identical pixels.
    for text in ["a", "\u{05D0}", "W"] {
        let shaped = ink(text, 0, true);
        let unshaped = ink(text, 0, false);
        assert_eq!(
            shaped.columns, unshaped.columns,
            "{text:?} renders differently depending on a path it should not take"
        );
    }
}

#[test]
fn a_pointed_cluster_is_centred_rather_than_leaning_left() {
    // The positive form of the same claim, and the one a person would check by eye: the ink
    // of a pointed letter should sit around the middle of its cell.
    let fonts = FontStack::system(SIZE).expect("system fonts");
    let width = fonts.metrics().width;

    for cluster in POINTED {
        let shaped = ink(cluster, 0, true);
        let weighted: u32 = shaped
            .columns
            .iter()
            .enumerate()
            .map(|(x, count)| x as u32 * count)
            .sum();
        let total: u32 = shaped.columns.iter().sum();
        let centroid = weighted as f32 / total as f32;
        let middle = width as f32 / 2.0;

        assert!(
            (centroid - middle).abs() < width as f32 / 4.0,
            "{cluster:?} centroid {centroid:.2} is not near the cell centre {middle:.2}"
        );
    }
}
