//! Purpose: the bit layout of a cell and a style as they cross the thread boundary.
//! Public surface: `PackedCell`, `pack_style`, `unpack_style`, `CLUSTER_BYTES`.
//! Why this file: the published frame is read by a thread that may be racing the writer, so
//!   every field has to live in a plain integer -- no pointers, no lengths to trust, nothing
//!   that turns a torn read into a crash instead of a discarded frame. Three `u64` per cell
//!   is the price: sixteen bytes of inline UTF-8 for the grapheme cluster, plus one word of
//!   attributes. Inline rather than an arena because an arena needs an offset and a length,
//!   and a torn offset/length pair is the one thing that could still fault.
//! NOT responsible for: the publish protocol (`seqlock.rs`) or run building (`frame.rs`).
//! Test strategy: round-trip properties below -- every style and every cluster that fits
//!   comes back identical, and one that does not fit comes back marked truncated.

use ruuah_vt_snapshot::{Color, Semantic, Style, Underline, Wide};

/// How much UTF-8 a cell carries inline.
///
/// Sixteen bytes holds any base letter plus roughly seven combining marks, which covers
/// Hebrew niqqud (base + vowel + dagesh + accent is four) and Arabic harakat with room over.
/// Multi-person emoji ZWJ sequences exceed it; those are flagged, never silently shortened.
pub const CLUSTER_BYTES: usize = 16;

/// A cell in its published form: two words of UTF-8, one of attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PackedCell {
    pub text: [u64; 2],
    pub attrs: u64,
}

const WIDE_SHIFT: u32 = 16;
const SEMANTIC_SHIFT: u32 = 18;
const TRUNCATED_BIT: u64 = 1 << 24;

impl PackedCell {
    pub const BLANK: PackedCell = PackedCell {
        text: [0, 0],
        attrs: 0,
    };

    /// Packs a grapheme cluster and its attributes.
    ///
    /// The cluster needs no stored length: NUL is not a legal codepoint in a cell (a zero
    /// codepoint *is* "no text"), so the first zero byte ends the string. One less field to
    /// tear.
    pub fn new(cluster: &str, style_id: u16, wide: Wide, semantic: Semantic) -> PackedCell {
        let bytes = cluster.as_bytes();
        let (fitting, truncated) = if bytes.len() <= CLUSTER_BYTES {
            (bytes, false)
        } else {
            (&bytes[..floor_char_boundary(cluster, CLUSTER_BYTES)], true)
        };

        let mut raw = [0u8; CLUSTER_BYTES];
        raw[..fitting.len()].copy_from_slice(fitting);

        let attrs = u64::from(style_id)
            | (wide_code(wide) << WIDE_SHIFT)
            | (semantic_code(semantic) << SEMANTIC_SHIFT)
            | if truncated { TRUNCATED_BIT } else { 0 };

        PackedCell {
            text: [
                u64::from_le_bytes(raw[..8].try_into().expect("8 bytes")),
                u64::from_le_bytes(raw[8..].try_into().expect("8 bytes")),
            ],
            attrs,
        }
    }

    pub fn style_id(self) -> u16 {
        self.attrs as u16
    }

    pub fn wide(self) -> Wide {
        match (self.attrs >> WIDE_SHIFT) & 0b11 {
            1 => Wide::Wide,
            2 => Wide::SpacerTail,
            3 => Wide::SpacerHead,
            _ => Wide::Narrow,
        }
    }

    /// What OSC 133 says this cell holds. Carried through to the renderer because the caret
    /// only moves visually where the user is editing: everywhere else the cursor is the
    /// program's to place, and stepping it visually would fight the program.
    pub fn semantic(self) -> Semantic {
        match (self.attrs >> SEMANTIC_SHIFT) & 0b11 {
            1 => Semantic::Input,
            2 => Semantic::Prompt,
            _ => Semantic::Output,
        }
    }

    /// Whether the cluster was longer than a cell can carry. A renderer that cares can fall
    /// back; what it must not do is draw a shortened cluster believing it is complete.
    pub fn is_truncated(self) -> bool {
        self.attrs & TRUNCATED_BIT != 0
    }

    pub fn has_text(self) -> bool {
        self.text[0] != 0
    }

    /// The cluster, borrowed out of a caller-owned scratch array so reading a frame does not
    /// allocate per cell.
    pub fn cluster(self, scratch: &mut [u8; CLUSTER_BYTES]) -> &str {
        scratch[..8].copy_from_slice(&self.text[0].to_le_bytes());
        scratch[8..].copy_from_slice(&self.text[1].to_le_bytes());
        let end = scratch
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(CLUSTER_BYTES);
        // Only ever written from a `&str` above, so the prefix ending at a NUL is valid UTF-8
        // as long as the writer respected char boundaries -- which `new` does.
        std::str::from_utf8(&scratch[..end]).unwrap_or("")
    }
}

fn semantic_code(semantic: Semantic) -> u64 {
    match semantic {
        Semantic::Output => 0,
        Semantic::Input => 1,
        Semantic::Prompt => 2,
    }
}

fn wide_code(wide: Wide) -> u64 {
    match wide {
        Wide::Narrow => 0,
        Wide::Wide => 1,
        Wide::SpacerTail => 2,
        Wide::SpacerHead => 3,
    }
}

/// The largest index `<= limit` that starts a character, so a truncated cluster is still
/// valid UTF-8 rather than a split codepoint.
fn floor_char_boundary(text: &str, limit: usize) -> usize {
    let mut index = limit.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

const COLOR_BITS: u32 = 26;

fn pack_color(color: Color) -> u64 {
    match color {
        Color::Default => 0,
        Color::Palette(index) => 1 | (u64::from(index) << 2),
        Color::Rgb { r, g, b } => {
            2 | (u64::from(r) << 2) | (u64::from(g) << 10) | (u64::from(b) << 18)
        }
    }
}

fn unpack_color(bits: u64) -> Color {
    match bits & 0b11 {
        1 => Color::Palette(((bits >> 2) & 0xff) as u8),
        2 => Color::Rgb {
            r: ((bits >> 2) & 0xff) as u8,
            g: ((bits >> 10) & 0xff) as u8,
            b: ((bits >> 18) & 0xff) as u8,
        },
        _ => Color::Default,
    }
}

fn pack_underline(underline: Underline) -> u64 {
    match underline {
        Underline::None => 0,
        Underline::Single => 1,
        Underline::Double => 2,
        Underline::Curly => 3,
        Underline::Dotted => 4,
        Underline::Dashed => 5,
    }
}

fn unpack_underline(bits: u64) -> Underline {
    match bits {
        1 => Underline::Single,
        2 => Underline::Double,
        3 => Underline::Curly,
        4 => Underline::Dotted,
        5 => Underline::Dashed,
        _ => Underline::None,
    }
}

/// A style as two words: colours and boolean attributes in the first, the underline colour
/// in the second. Three 26-bit colours plus eleven attribute bits do not fit in one word.
pub fn pack_style(style: &Style) -> [u64; 2] {
    let flags = [
        style.bold,
        style.italic,
        style.faint,
        style.blink,
        style.inverse,
        style.invisible,
        style.strikethrough,
        style.overline,
    ]
    .iter()
    .enumerate()
    .fold(0u64, |bits, (i, on)| bits | (u64::from(*on) << i));

    [
        pack_color(style.fg)
            | (pack_color(style.bg) << COLOR_BITS)
            | (flags << (COLOR_BITS * 2))
            | (pack_underline(style.underline) << (COLOR_BITS * 2 + 8)),
        pack_color(style.underline_color),
    ]
}

pub fn unpack_style(words: [u64; 2]) -> Style {
    let mask = (1u64 << COLOR_BITS) - 1;
    let flags = words[0] >> (COLOR_BITS * 2);
    let bit = |i: u32| flags & (1 << i) != 0;

    Style {
        fg: unpack_color(words[0] & mask),
        bg: unpack_color((words[0] >> COLOR_BITS) & mask),
        underline_color: unpack_color(words[1] & mask),
        bold: bit(0),
        italic: bit(1),
        faint: bit(2),
        blink: bit(3),
        inverse: bit(4),
        invisible: bit(5),
        strikethrough: bit(6),
        overline: bit(7),
        underline: unpack_underline((flags >> 8) & 0b111),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_style_field_survives_the_round_trip() {
        let style = Style {
            fg: Color::Rgb {
                r: 0x12,
                g: 0x34,
                b: 0x56,
            },
            bg: Color::Palette(200),
            underline_color: Color::Rgb {
                r: 0xff,
                g: 0x00,
                b: 0x7f,
            },
            bold: true,
            italic: false,
            faint: true,
            blink: false,
            inverse: true,
            invisible: false,
            strikethrough: true,
            overline: true,
            underline: Underline::Curly,
        };
        assert_eq!(unpack_style(pack_style(&style)), style);
    }

    #[test]
    fn the_default_style_packs_to_zero() {
        // Relied on by the published buffer: a freshly zeroed style table already reads as
        // the default style, so a frame published before any styles are interned is correct
        // rather than garbage.
        assert_eq!(pack_style(&Style::DEFAULT), [0, 0]);
        assert_eq!(unpack_style([0, 0]), Style::DEFAULT);
    }

    #[test]
    fn a_hebrew_cluster_with_niqqud_round_trips_whole() {
        // The north star in one assertion: base letter, vowel, dagesh and accent are four
        // codepoints in ONE cell, and all four have to survive the crossing.
        let cluster = "\u{05D1}\u{05BC}\u{05B8}\u{0591}";
        let cell = PackedCell::new(cluster, 3, Wide::Narrow, Semantic::Output);
        let mut scratch = [0u8; CLUSTER_BYTES];

        assert_eq!(cell.cluster(&mut scratch), cluster);
        assert!(!cell.is_truncated());
        assert_eq!(cell.style_id(), 3);
    }

    #[test]
    fn an_oversized_cluster_is_marked_rather_than_quietly_shortened() {
        let cluster = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}";
        let cell = PackedCell::new(cluster, 0, Wide::Wide, Semantic::Output);
        let mut scratch = [0u8; CLUSTER_BYTES];

        assert!(cell.is_truncated(), "the renderer has to be able to tell");
        let kept = cell.cluster(&mut scratch);
        assert!(
            cluster.starts_with(kept),
            "and what is kept is a valid prefix"
        );
    }

    #[test]
    fn a_blank_cell_has_no_text() {
        let mut scratch = [0u8; CLUSTER_BYTES];
        assert!(!PackedCell::BLANK.has_text());
        assert_eq!(PackedCell::BLANK.cluster(&mut scratch), "");
    }

    #[test]
    fn wide_and_style_id_survive_alongside_text() {
        let cell = PackedCell::new("\u{4F60}", 0xbeef, Wide::Wide, Semantic::Output);
        assert_eq!(cell.wide(), Wide::Wide);
        assert_eq!(cell.style_id(), 0xbeef);
        assert_eq!(
            PackedCell::new("", 0, Wide::SpacerTail, Semantic::Output).wide(),
            Wide::SpacerTail
        );
    }
}
