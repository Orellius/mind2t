//! Purpose: kitty's unicode placeholder addressing -- U+10EEEE cells that name an image,
//!   a row and a column through their colour and their combining marks.
//! Public surface: `PLACEHOLDER`, `VirtualRun`, `virtual_runs`.
//! Why this file: a placeholder image lives IN THE GRID rather than beside it, so it
//!   scrolls, reflows and gets erased exactly like text, for free. That is the deep fix
//!   for images drifting away from their cells -- v2 taught anchors to ride reflow, and
//!   this makes the question not arise.
//! Why here rather than in the core: the core stays pixel-ignorant and, more to the point,
//!   these cells are ORDINARY TEXT to it. Nothing is parsed at print time; the meaning is
//!   read off the published frame, which is also what keeps it out of the differential
//!   corpus's way (the oracle's ABI exposes no graphics at all).
//! Reference: `../ruuah/src/terminal/kitty/graphics_unicode.zig`, whose rules are ported
//!   here -- `IncompletePlacement::init` for the decode and `canAppend` for the run rules.
//! Test strategy: unit tests below; the pixel half lives in `render/tests/placeholder.rs`.

/// The codepoint kitty reserves for a placeholder cell.
pub const PLACEHOLDER: char = '\u{10EEEE}';

/// The diacritics that encode a row or column index, in index order.
///
/// Vendored from kitty's published `rowcolumn-diacritics.txt` (via the oracle's copy of
/// the same table). The INDEX is the value; the codepoints themselves carry no arithmetic
/// meaning, which is why this is a table and not a formula. Sorted, so the lookup is a
/// binary search -- `a_lookup_is_only_valid_if_the_table_is_sorted` asserts that rather
/// than trusting it, because an unsorted table would silently decode wrong indices.
const DIACRITICS: [char; 297] = [
    '\u{0305}',
    '\u{030d}',
    '\u{030e}',
    '\u{0310}',
    '\u{0312}',
    '\u{033d}',
    '\u{033e}',
    '\u{033f}',
    '\u{0346}',
    '\u{034a}',
    '\u{034b}',
    '\u{034c}',
    '\u{0350}',
    '\u{0351}',
    '\u{0352}',
    '\u{0357}',
    '\u{035b}',
    '\u{0363}',
    '\u{0364}',
    '\u{0365}',
    '\u{0366}',
    '\u{0367}',
    '\u{0368}',
    '\u{0369}',
    '\u{036a}',
    '\u{036b}',
    '\u{036c}',
    '\u{036d}',
    '\u{036e}',
    '\u{036f}',
    '\u{0483}',
    '\u{0484}',
    '\u{0485}',
    '\u{0486}',
    '\u{0487}',
    '\u{0592}',
    '\u{0593}',
    '\u{0594}',
    '\u{0595}',
    '\u{0597}',
    '\u{0598}',
    '\u{0599}',
    '\u{059c}',
    '\u{059d}',
    '\u{059e}',
    '\u{059f}',
    '\u{05a0}',
    '\u{05a1}',
    '\u{05a8}',
    '\u{05a9}',
    '\u{05ab}',
    '\u{05ac}',
    '\u{05af}',
    '\u{05c4}',
    '\u{0610}',
    '\u{0611}',
    '\u{0612}',
    '\u{0613}',
    '\u{0614}',
    '\u{0615}',
    '\u{0616}',
    '\u{0617}',
    '\u{0657}',
    '\u{0658}',
    '\u{0659}',
    '\u{065a}',
    '\u{065b}',
    '\u{065d}',
    '\u{065e}',
    '\u{06d6}',
    '\u{06d7}',
    '\u{06d8}',
    '\u{06d9}',
    '\u{06da}',
    '\u{06db}',
    '\u{06dc}',
    '\u{06df}',
    '\u{06e0}',
    '\u{06e1}',
    '\u{06e2}',
    '\u{06e4}',
    '\u{06e7}',
    '\u{06e8}',
    '\u{06eb}',
    '\u{06ec}',
    '\u{0730}',
    '\u{0732}',
    '\u{0733}',
    '\u{0735}',
    '\u{0736}',
    '\u{073a}',
    '\u{073d}',
    '\u{073f}',
    '\u{0740}',
    '\u{0741}',
    '\u{0743}',
    '\u{0745}',
    '\u{0747}',
    '\u{0749}',
    '\u{074a}',
    '\u{07eb}',
    '\u{07ec}',
    '\u{07ed}',
    '\u{07ee}',
    '\u{07ef}',
    '\u{07f0}',
    '\u{07f1}',
    '\u{07f3}',
    '\u{0816}',
    '\u{0817}',
    '\u{0818}',
    '\u{0819}',
    '\u{081b}',
    '\u{081c}',
    '\u{081d}',
    '\u{081e}',
    '\u{081f}',
    '\u{0820}',
    '\u{0821}',
    '\u{0822}',
    '\u{0823}',
    '\u{0825}',
    '\u{0826}',
    '\u{0827}',
    '\u{0829}',
    '\u{082a}',
    '\u{082b}',
    '\u{082c}',
    '\u{082d}',
    '\u{0951}',
    '\u{0953}',
    '\u{0954}',
    '\u{0f82}',
    '\u{0f83}',
    '\u{0f86}',
    '\u{0f87}',
    '\u{135d}',
    '\u{135e}',
    '\u{135f}',
    '\u{17dd}',
    '\u{193a}',
    '\u{1a17}',
    '\u{1a75}',
    '\u{1a76}',
    '\u{1a77}',
    '\u{1a78}',
    '\u{1a79}',
    '\u{1a7a}',
    '\u{1a7b}',
    '\u{1a7c}',
    '\u{1b6b}',
    '\u{1b6d}',
    '\u{1b6e}',
    '\u{1b6f}',
    '\u{1b70}',
    '\u{1b71}',
    '\u{1b72}',
    '\u{1b73}',
    '\u{1cd0}',
    '\u{1cd1}',
    '\u{1cd2}',
    '\u{1cda}',
    '\u{1cdb}',
    '\u{1ce0}',
    '\u{1dc0}',
    '\u{1dc1}',
    '\u{1dc3}',
    '\u{1dc4}',
    '\u{1dc5}',
    '\u{1dc6}',
    '\u{1dc7}',
    '\u{1dc8}',
    '\u{1dc9}',
    '\u{1dcb}',
    '\u{1dcc}',
    '\u{1dd1}',
    '\u{1dd2}',
    '\u{1dd3}',
    '\u{1dd4}',
    '\u{1dd5}',
    '\u{1dd6}',
    '\u{1dd7}',
    '\u{1dd8}',
    '\u{1dd9}',
    '\u{1dda}',
    '\u{1ddb}',
    '\u{1ddc}',
    '\u{1ddd}',
    '\u{1dde}',
    '\u{1ddf}',
    '\u{1de0}',
    '\u{1de1}',
    '\u{1de2}',
    '\u{1de3}',
    '\u{1de4}',
    '\u{1de5}',
    '\u{1de6}',
    '\u{1dfe}',
    '\u{20d0}',
    '\u{20d1}',
    '\u{20d4}',
    '\u{20d5}',
    '\u{20d6}',
    '\u{20d7}',
    '\u{20db}',
    '\u{20dc}',
    '\u{20e1}',
    '\u{20e7}',
    '\u{20e9}',
    '\u{20f0}',
    '\u{2cef}',
    '\u{2cf0}',
    '\u{2cf1}',
    '\u{2de0}',
    '\u{2de1}',
    '\u{2de2}',
    '\u{2de3}',
    '\u{2de4}',
    '\u{2de5}',
    '\u{2de6}',
    '\u{2de7}',
    '\u{2de8}',
    '\u{2de9}',
    '\u{2dea}',
    '\u{2deb}',
    '\u{2dec}',
    '\u{2ded}',
    '\u{2dee}',
    '\u{2def}',
    '\u{2df0}',
    '\u{2df1}',
    '\u{2df2}',
    '\u{2df3}',
    '\u{2df4}',
    '\u{2df5}',
    '\u{2df6}',
    '\u{2df7}',
    '\u{2df8}',
    '\u{2df9}',
    '\u{2dfa}',
    '\u{2dfb}',
    '\u{2dfc}',
    '\u{2dfd}',
    '\u{2dfe}',
    '\u{2dff}',
    '\u{a66f}',
    '\u{a67c}',
    '\u{a67d}',
    '\u{a6f0}',
    '\u{a6f1}',
    '\u{a8e0}',
    '\u{a8e1}',
    '\u{a8e2}',
    '\u{a8e3}',
    '\u{a8e4}',
    '\u{a8e5}',
    '\u{a8e6}',
    '\u{a8e7}',
    '\u{a8e8}',
    '\u{a8e9}',
    '\u{a8ea}',
    '\u{a8eb}',
    '\u{a8ec}',
    '\u{a8ed}',
    '\u{a8ee}',
    '\u{a8ef}',
    '\u{a8f0}',
    '\u{a8f1}',
    '\u{aab0}',
    '\u{aab2}',
    '\u{aab3}',
    '\u{aab7}',
    '\u{aab8}',
    '\u{aabe}',
    '\u{aabf}',
    '\u{aac1}',
    '\u{fe20}',
    '\u{fe21}',
    '\u{fe22}',
    '\u{fe23}',
    '\u{fe24}',
    '\u{fe25}',
    '\u{fe26}',
    '\u{10a0f}',
    '\u{10a38}',
    '\u{1d185}',
    '\u{1d186}',
    '\u{1d187}',
    '\u{1d188}',
    '\u{1d189}',
    '\u{1d1aa}',
    '\u{1d1ab}',
    '\u{1d1ac}',
    '\u{1d1ad}',
    '\u{1d242}',
    '\u{1d243}',
    '\u{1d244}',
];

/// The row/column index a diacritic encodes, if it is one of them.
///
/// Binary search over `DIACRITICS`, whose index IS the value. An unknown mark is not an
/// error: kitty treats invalid diacritics as absent, and absent has its own meaning
/// (continue the run), so this returns `None` rather than refusing the cell.
fn index_of(mark: char) -> Option<u32> {
    DIACRITICS
        .binary_search(&mark)
        .ok()
        .map(|index| index as u32)
}

/// One decoded placeholder cell, before neighbouring cells are joined into a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Partial {
    /// Low 24 bits of the image id, carried by the cell's FOREGROUND colour. Always
    /// present -- a placeholder with no fg names image 0, which is no image.
    image_low: u32,
    /// High 8 bits, from the third diacritic. Absent is not zero: it means "continue",
    /// which is why every one of these is an `Option`.
    image_high: Option<u32>,
    /// From the underline colour. Decoded because the run rules compare it; this crate
    /// does not otherwise use it (placements are keyed by image).
    placement_id: Option<u32>,
    row: Option<u32>,
    col: Option<u32>,
}

/// A horizontal run of placeholder cells addressing one image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualRun {
    /// The image this run draws part of.
    pub image: u32,
    /// Where the run starts on screen.
    pub screen_col: u16,
    pub screen_row: u16,
    /// Which cell OF THE IMAGE the run starts at -- the source rectangle, in cells.
    pub image_col: u32,
    pub image_row: u32,
    /// How many cells wide the run is. Always one row tall: kitty's own iterator builds
    /// runs a row at a time, and a taller image is simply more runs.
    pub width: u16,
}

impl Partial {
    /// The full image id, once the run is complete.
    fn image(&self) -> u32 {
        self.image_low | (self.image_high.unwrap_or(0) << 24)
    }

    /// Whether `next` continues this run, from the oracle's `canAppend`.
    ///
    /// The asymmetry is the whole trick: a cell that OMITS its row/col continues whatever
    /// came before, so a wide image is usually one fully-specified cell followed by bare
    /// placeholders. A cell that SPECIFIES them must agree -- the row identical, the
    /// column exactly the next one along.
    fn can_extend(&self, width: u16, next: &Partial) -> bool {
        self.image_low == next.image_low
            && self.placement_id == next.placement_id
            && next.row.is_none_or(|row| Some(row) == self.row)
            && next
                .col
                .is_none_or(|col| Some(col) == self.col.map(|start| start + u32::from(width)))
            && next.image_high.is_none_or(|high| Some(high) == self.image_high)
    }
}

/// Decodes one cell, if it is a placeholder at all.
fn decode(frame: &crate::frame::Frame, x: u16, y: u16) -> Option<Partial> {
    let cell = frame.cell(x, y);
    let mut scratch = [0u8; crate::packed::CLUSTER_BYTES];
    let cluster = cell.cluster(&mut scratch);
    let mut chars = cluster.chars();
    if chars.next()? != PLACEHOLDER {
        return None;
    }

    // The marks are positional: row, then column, then the image id's high byte. A mark
    // that is not in the table is treated as ABSENT rather than as an error, matching
    // kitty -- and absent means "continue the run", so this distinction has teeth.
    let marks: Vec<char> = chars.collect();
    let style = frame.style(cell.style_id());
    Some(Partial {
        image_low: color_to_id(style.fg),
        image_high: marks.get(2).copied().and_then(index_of),
        placement_id: match color_to_id(style.underline_color) {
            0 => None,
            id => Some(id),
        },
        row: marks.first().copied().and_then(index_of),
        col: marks.get(1).copied().and_then(index_of),
    })
}

/// A colour read as an image id, the way the protocol specifies it.
///
/// A palette index is that number; an RGB triple is its 24 bits. The default foreground
/// is NOT an id -- a cell with no colour set names image 0, which never exists, so such a
/// placeholder draws nothing instead of drawing whatever image happened to be first.
fn color_to_id(color: mind2t_vt_snapshot::Color) -> u32 {
    match color {
        mind2t_vt_snapshot::Color::Default => 0,
        mind2t_vt_snapshot::Color::Palette(index) => u32::from(index),
        mind2t_vt_snapshot::Color::Rgb { r, g, b } => {
            (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
        }
    }
}

/// Every placeholder run on row `y`, left to right.
///
/// Runs never span rows: kitty's own iterator builds them a row at a time, and a taller
/// image is simply more runs, each carrying its own image row.
pub fn virtual_runs(frame: &crate::frame::Frame, y: u16) -> Vec<VirtualRun> {
    let mut runs: Vec<VirtualRun> = Vec::new();
    let mut open: Option<(Partial, u16, u16)> = None; // partial, start column, width

    for x in 0..frame.cols {
        let Some(partial) = decode(frame, x, y) else {
            // A non-placeholder cell ends whatever run was open; runs are contiguous.
            if let Some((start, col, width)) = open.take() {
                runs.push(finish(&start, col, y, width));
            }
            continue;
        };

        match open.as_mut() {
            Some((start, _, width)) if start.can_extend(*width, &partial) => *width += 1,
            _ => {
                if let Some((start, col, width)) = open.take() {
                    runs.push(finish(&start, col, y, width));
                }
                open = Some((partial, x, 1));
            }
        }
    }
    if let Some((start, col, width)) = open.take() {
        runs.push(finish(&start, col, y, width));
    }
    runs.retain(|run| run.image != 0);
    runs
}

fn finish(start: &Partial, col: u16, row: u16, width: u16) -> VirtualRun {
    VirtualRun {
        image: start.image(),
        screen_col: col,
        screen_row: row,
        image_col: start.col.unwrap_or(0),
        image_row: start.row.unwrap_or(0),
        width,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Frame;
    use crate::publish::Publisher;
    use crate::seqlock::{ReadOutcome, channel};
    use mind2t_vt_core::Terminal;

    const COLS: u16 = 10;
    const ROWS: u16 = 3;

    /// The binary search is only correct if the table is sorted, and nothing else in this
    /// file would fail if it were not -- a mis-ordered table decodes wrong INDICES, which
    /// looks like an image drawn from the wrong offset rather than like a crash.
    #[test]
    fn the_diacritic_table_is_sorted_and_has_no_duplicates() {
        assert!(DIACRITICS.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(DIACRITICS.len(), 297);
    }

    #[test]
    fn the_index_of_a_diacritic_is_its_position() {
        assert_eq!(index_of('\u{0305}'), Some(0));
        assert_eq!(index_of('\u{030D}'), Some(1));
        assert_eq!(index_of(DIACRITICS[296]), Some(296));
        assert_eq!(index_of('a'), None, "an ordinary character is not an index");
    }

    /// Drives the real core so the cells under test are printed the way a program would
    /// print them, then reads the runs off the published frame.
    fn runs_of(bytes: &[u8], y: u16) -> Vec<VirtualRun> {
        let mut terminal = Terminal::new(COLS, ROWS);
        let (writer, reader) = channel(COLS, ROWS);
        let mut publisher = Publisher::new(writer);
        let mut frame = Frame::new();
        terminal.write(bytes);
        publisher.publish(&mut terminal).expect("fits");
        assert!(matches!(
            reader.read_into(&mut frame),
            ReadOutcome::Fresh(_)
        ));
        virtual_runs(&frame, y)
    }

    /// Foreground 42 plus row 0 and column 0: the minimal complete placeholder.
    #[test]
    fn a_single_fully_specified_cell_is_a_run() {
        let runs = runs_of("\x1b[38;5;42m\u{10EEEE}\u{0305}\u{0305}".as_bytes(), 0);
        assert_eq!(
            runs,
            vec![VirtualRun {
                image: 42,
                screen_col: 0,
                screen_row: 0,
                image_col: 0,
                image_row: 0,
                width: 1,
            }]
        );
    }

    /// The rule that makes wide images cheap to emit: cells with NO diacritics continue
    /// the run rather than starting one. This is how kitty's own output looks.
    #[test]
    fn bare_placeholders_continue_the_run() {
        let runs = runs_of(
            "\x1b[38;5;42m\u{10EEEE}\u{0305}\u{0305}\u{10EEEE}\u{10EEEE}".as_bytes(),
            0,
        );
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].width, 3);
    }

    /// A cell that SPECIFIES its column must name the next one along, or it starts a new
    /// run. Without this a repeated column would silently stretch one run.
    #[test]
    fn an_explicit_column_must_be_the_next_one() {
        let continues = runs_of(
            "\x1b[38;5;42m\u{10EEEE}\u{0305}\u{0305}\u{10EEEE}\u{0305}\u{030D}".as_bytes(),
            0,
        );
        assert_eq!(continues.len(), 1, "column 1 follows column 0");
        assert_eq!(continues[0].width, 2);

        let breaks = runs_of(
            "\x1b[38;5;42m\u{10EEEE}\u{0305}\u{0305}\u{10EEEE}\u{0305}\u{0305}".as_bytes(),
            0,
        );
        assert_eq!(breaks.len(), 2, "column 0 twice is two runs, not one wide one");
        assert!(breaks.iter().all(|run| run.width == 1));
    }

    /// A different image on the next cell is a different run even with no gap.
    #[test]
    fn a_different_image_starts_a_new_run() {
        let runs = runs_of(
            "\x1b[38;5;42m\u{10EEEE}\u{0305}\u{0305}\x1b[38;5;7m\u{10EEEE}\u{0305}\u{0305}"
                .as_bytes(),
            0,
        );
        assert_eq!(runs.len(), 2);
        assert_eq!((runs[0].image, runs[1].image), (42, 7));
    }

    /// The third diacritic carries the image id's high byte, so ids above 255 are
    /// addressable at all. 42 with high byte 1 is 42 | (1 << 24).
    #[test]
    fn the_third_diacritic_is_the_image_ids_high_byte() {
        let runs = runs_of(
            "\x1b[38;5;42m\u{10EEEE}\u{0305}\u{0305}\u{030D}".as_bytes(),
            0,
        );
        assert_eq!(runs[0].image, 42 | (1 << 24));
    }

    /// An RGB foreground is 24 bits of image id, not a colour lookup.
    #[test]
    fn an_rgb_foreground_is_the_image_id() {
        let runs = runs_of(
            "\x1b[38;2;0;1;2m\u{10EEEE}\u{0305}\u{0305}".as_bytes(),
            0,
        );
        assert_eq!(runs[0].image, 0x000102);
    }

    /// A placeholder with no colour names image 0, which never exists. Dropping it is what
    /// stops a stray U+10EEEE in ordinary text from drawing whatever image is around.
    #[test]
    fn a_placeholder_with_no_foreground_draws_nothing() {
        assert!(runs_of("\u{10EEEE}\u{0305}\u{0305}".as_bytes(), 0).is_empty());
    }

    #[test]
    fn ordinary_text_produces_no_runs() {
        assert!(runs_of(b"hello", 0).is_empty());
    }

    /// Non-placeholder cells cut a run: two placeholder groups separated by a space are
    /// two runs, and the second one's screen column is where it actually starts.
    #[test]
    fn a_gap_splits_a_run() {
        let runs = runs_of(
            "\x1b[38;5;42m\u{10EEEE}\u{0305}\u{0305} \u{10EEEE}\u{0305}\u{030D}".as_bytes(),
            0,
        );
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].screen_col, 0);
        assert_eq!(runs[1].screen_col, 2);
        assert_eq!(runs[1].image_col, 1);
    }

    /// The row diacritic is what makes a tall image work: each screen row carries the
    /// image row it shows, so the renderer crops rather than redrawing the whole image.
    #[test]
    fn the_row_diacritic_selects_the_image_row() {
        let bytes = "\x1b[38;5;42m\u{10EEEE}\u{0305}\u{0305}\r\n\u{10EEEE}\u{030D}\u{0305}";
        assert_eq!(runs_of(bytes.as_bytes(), 0)[0].image_row, 0);
        assert_eq!(runs_of(bytes.as_bytes(), 1)[0].image_row, 1);
    }
}
