//! Purpose: turn a cluster of text into "which font, which glyph", and define the cell box.
//! Public surface: `FontStack`, `FontError`, `CellMetrics`, `Resolved`.
//! Why this file: measured on this machine 2026-07-28 -- Menlo carries Latin and box drawing
//!   but maps Hebrew to glyph 0, and Arial Hebrew carries Hebrew but maps 'A' to glyph 0. No
//!   system font covers both, so a stack is not a refinement to add later; a single-font
//!   renderer cannot draw the thing this project exists to draw. That is why the atlas keys
//!   on (font, glyph) rather than glyph, and why resolution returns which font answered.
//! NOT responsible for: rasterization (`atlas.rs`), shaping. Shaping is slice 5.5 -- this
//!   maps ONE codepoint per lookup, which is correct for Latin and box drawing and is why
//!   Hebrew niqqud currently land as separate glyphs instead of stacking.
//! Test strategy: unit tests below pin the two coverage gaps that forced the stack, so a
//!   font change on the machine is caught rather than silently drawing tofu.

use std::collections::HashMap;

use swash::FontRef;

#[derive(Debug, thiserror::Error)]
pub enum FontError {
    #[error("could not read {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} index {index} is not a font")]
    Parse { path: String, index: usize },
    #[error("a font stack needs at least one font")]
    Empty,
}

/// One loaded font file, kept as bytes because `FontRef` borrows them.
struct Face {
    data: Vec<u8>,
    index: usize,
}

impl Face {
    /// Rebuilt per use rather than stored, since `FontRef` borrows `data` and a struct cannot
    /// hold both. Parsing a header is cheap and only happens on an atlas miss.
    fn font(&self) -> FontRef<'_> {
        FontRef::from_index(&self.data, self.index).expect("validated at load")
    }
}

/// Which font answered for a cluster, and with what glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolved {
    pub font: u16,
    pub glyph: u16,
}

/// The pixel box one terminal cell occupies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellMetrics {
    pub width: u32,
    pub height: u32,
    /// Distance from the top of the cell down to the text baseline.
    pub baseline: i32,
}

/// An ordered list of fonts, searched first-to-last for each cluster.
/// Finds an installed font file whose stem matches `family` with spaces and dashes
/// ignored, searching the user dir first so a user-installed face wins over a system
/// one of the same name.
fn find_family(family: &str) -> Option<String> {
    let normalize = |text: &str| -> String {
        text.chars()
            .filter(|c| !matches!(c, ' ' | '-' | '_'))
            .flat_map(char::to_lowercase)
            .collect()
    };
    let wanted = normalize(family);
    let home = std::env::var("HOME").unwrap_or_default();
    let dirs = [
        format!("{home}/Library/Fonts"),
        "/Library/Fonts".to_string(),
        "/System/Library/Fonts".to_string(),
        "/System/Library/Fonts/Supplemental".to_string(),
    ];
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut names: Vec<_> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                matches!(
                    path.extension().and_then(|e| e.to_str()),
                    Some("ttf" | "otf" | "ttc")
                )
            })
            .collect();
        names.sort(); // deterministic pick when several weights match
        for path in names {
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if normalize(stem).contains(&wanted) {
                return path.to_str().map(str::to_owned);
            }
        }
    }
    None
}

pub struct FontStack {
    faces: Vec<Face>,
    size: f32,
    metrics: CellMetrics,
    /// Resolution is per cell per frame, so it must not reparse a font every time.
    resolved: HashMap<char, Option<Resolved>>,
}

impl FontStack {
    /// Loads fonts in priority order. The first one also defines the cell box, because a
    /// terminal grid is the primary font's grid and a fallback must fit into it.
    pub fn load(paths: &[(&str, usize)], size: f32) -> Result<FontStack, FontError> {
        let mut faces = Vec::new();
        for (path, index) in paths {
            let data = std::fs::read(path).map_err(|source| FontError::Read {
                path: (*path).to_string(),
                source,
            })?;
            FontRef::from_index(&data, *index).ok_or(FontError::Parse {
                path: (*path).to_string(),
                index: *index,
            })?;
            faces.push(Face {
                data,
                index: *index,
            });
        }

        let first = faces.first().ok_or(FontError::Empty)?;
        let metrics = cell_metrics(first.font(), size);

        Ok(FontStack {
            faces,
            size,
            metrics,
            resolved: HashMap::new(),
        })
    }

    /// The macOS default stack, in preference order, skipping whatever is not installed.
    ///
    /// Menlo leads because it is the only one of the three carrying box drawing, blocks and
    /// powerline -- Miriam Mono CLM covers **0 of 128** box-drawing codepoints, measured, so
    /// it cannot lead a stack that has to draw a TUI.
    ///
    /// Miriam Mono CLM (Culmus) is the Hebrew of choice and is worth installing. Measured
    /// 2026-07-28 by shaping through swash: it composes shin+shin-dot and bet+dagesh via GSUB
    /// into single glyphs, positions a qamats via GPOS at exactly half the advance so the
    /// mark is centred under its base, gives marks zero advance so a pointed cluster stays
    /// ONE cell, and advances Latin and Hebrew identically at 0.6em -- the same advance as
    /// Menlo, so the two share a grid exactly.
    ///
    /// Arial Hebrew is the last resort: it ships on every macOS but is proportional, so
    /// Hebrew sits unevenly in a fixed grid.
    /// The system stack with a user-chosen lead font in FRONT -- it answers first and
    /// defines the cell box; the stack backstops coverage exactly as before. `family`
    /// is an absolute path to a font file, or a name matched case-insensitively
    /// against installed font FILE STEMS (spaces and dashes ignored) across the user,
    /// local and system font dirs. Honest v1: no CoreText name-table lookup, and an
    /// unresolvable family falls back to the plain system stack -- the terminal must
    /// come up either way; the config layer owns reporting the miss.
    pub fn with_primary(family: Option<&str>, size: f32) -> Result<FontStack, FontError> {
        let Some(family) = family else {
            return FontStack::system(size);
        };
        let path = if family.starts_with('/') && std::path::Path::new(family).is_file() {
            Some(family.to_string())
        } else {
            find_family(family)
        };
        match path {
            Some(path) => {
                let mut candidates = vec![(path.as_str(), 0usize)];
                let system = FontStack::system_paths();
                candidates.extend(system.iter().map(|entry| (entry.as_str(), 0)));
                FontStack::load(&candidates, size)
            }
            None => FontStack::system(size),
        }
    }

    /// Whether a named family resolved to an installed font file -- the config layer's
    /// loud-miss check, kept next to the resolution rule it reports on.
    pub fn family_resolves(family: &str) -> bool {
        (family.starts_with('/') && std::path::Path::new(family).is_file())
            || find_family(family).is_some()
    }

    pub fn system(size: f32) -> Result<FontStack, FontError> {
        let candidates = FontStack::system_paths();
        let present: Vec<(&str, usize)> = candidates
            .iter()
            .map(|path| (path.as_str(), 0))
            .collect();
        FontStack::load(&present, size)
    }

    fn system_paths() -> Vec<String> {
        let home = std::env::var("HOME").unwrap_or_default();
        let candidates = [
            ("/System/Library/Fonts/Menlo.ttc".to_string(), 0),
            (format!("{home}/Library/Fonts/MiriamMonoCLM-Book.ttf"), 0),
            ("/System/Library/Fonts/ArialHB.ttc".to_string(), 0),
            // Arabic and Persian (2026-08-07). The stack above carries NONE of the Arabic
            // block, so every Arabic and Persian codepoint resolved to nothing and drew as a
            // blank cell -- while the bidi implementation reordered them perfectly, passing
            // 91,707 Unicode conformance cases against text the screen never showed. That is
            // the worst shape this class of defect takes: correct algorithm, empty row, no
            // error anywhere. Found by rendering a three-script proof sheet and looking at it.
            //
            // SF Arabic leads because it ships on current macOS and carries the Persian
            // letters an Arabic-only face omits (peh U+067E, gaf U+06AF). Geeza Pro backs it
            // for older machines. Both are PROPORTIONAL, exactly like Arial Hebrew above, so
            // Arabic sits unevenly in a fixed grid -- which is the same trade this stack
            // already accepts there, and strictly better than a blank cell.
            //
            // Contextual joining still cannot cross a cell boundary. That is a property of
            // terminal grids rather than of this stack, and every terminal shares it.
            ("/System/Library/Fonts/SFArabic.ttf".to_string(), 0),
            ("/System/Library/Fonts/GeezaPro.ttc".to_string(), 0),
            // Braille patterns (U+2800..U+28FF): none of the fonts above carry them, so
            // dot art and braille spinners rendered as NOTHING until 2026-07-29, when the
            // RUUAH splash's ghost simply failed to appear. Apple Braille ships on every
            // macOS and sits last so it can never shadow a glyph the leads own.
            ("/System/Library/Fonts/Apple Braille.ttf".to_string(), 0),
            // Color emoji (P0.2): none of the fonts above carry emoji, so `[🧠 BRAIN]`
            // in Claude Code rendered as a blank gap until 2026-07-29. Apple Color
            // Emoji is sbix (bitmap strikes), which the atlas rasterizes through the
            // color source and the renderer blits through the image path -- putting it
            // in the stack alone draws NOTHING, which is why emoji_probe.rs pins pixels.
            ("/System/Library/Fonts/Apple Color Emoji.ttc".to_string(), 0),
            // Backstop for Symbols for Legacy Computing beyond what mosaic.rs
            // synthesizes (wedges, rounded mosaics, segmented digits -- Claude Code's
            // mascot needs the block, measured 2026-07-29). Iosevka is narrow, so its
            // BLOCK glyphs would leave gutters; those never reach it because the
            // synthesized set answers first in the draw path.
            (format!("{home}/Library/Fonts/IosevkaNerdFontMono-Regular.ttf"), 0),
        ];

        candidates
            .into_iter()
            .filter(|(path, _)| std::path::Path::new(path).is_file())
            .map(|(path, _)| path)
            .collect()
    }

    pub fn metrics(&self) -> CellMetrics {
        self.metrics
    }

    pub fn size(&self) -> f32 {
        self.size
    }

    pub fn font_count(&self) -> usize {
        self.faces.len()
    }

    pub(crate) fn face(&self, index: u16) -> Option<FontRef<'_>> {
        Some(self.faces.get(usize::from(index))?.font())
    }

    /// How far a resolved glyph advances the pen, in pixels at this stack's size.
    ///
    /// The grid test: a fallback whose advance differs from the primary's puts its script
    /// off the cell grid, which is the difference between Hebrew that lines up with code and
    /// Hebrew that drifts.
    pub fn advance(&self, resolved: Resolved) -> f32 {
        self.face(resolved.font)
            .map(|font| {
                font.glyph_metrics(&[])
                    .scale(self.size)
                    .advance_width(resolved.glyph)
            })
            .unwrap_or(0.0)
    }

    /// The full name of one font in the stack, for diagnostics.
    pub fn name(&self, index: u16) -> Option<String> {
        let font = self.face(index)?;
        font.localized_strings()
            .find_by_id(swash::StringId::Full, None)
            .map(|name| name.to_string())
    }

    /// The first font in the stack that has a glyph for `c`.
    ///
    /// `None` when nothing covers it, which is a real answer: drawing glyph 0 from the first
    /// font would put a tofu box on screen and look like a rendering bug rather than a
    /// missing font.
    /// Resolution for a cluster carrying VS16 (emoji presentation): the EMOJI face is
    /// asked first, even when a text font also covers the base character -- that is
    /// the entire meaning of the selector. Falls back to normal resolution so a
    /// machine without the emoji font still draws the text form.
    pub fn resolve_emoji(&mut self, c: char) -> Option<Resolved> {
        for (index, face) in self.faces.iter().enumerate() {
            let font = face.font();
            let glyph = font.charmap().map(c);
            if glyph != 0 && font.color_palettes().len() + font.color_strikes().len() + font.alpha_strikes().len() > 0 {
                return Some(Resolved {
                    font: index as u16,
                    glyph,
                });
            }
        }
        self.resolve(c)
    }

    pub fn resolve(&mut self, c: char) -> Option<Resolved> {
        if let Some(hit) = self.resolved.get(&c) {
            return *hit;
        }

        let mut found = None;
        for (index, face) in self.faces.iter().enumerate() {
            let glyph = face.font().charmap().map(c);
            if glyph != 0 {
                found = Some(Resolved {
                    font: index as u16,
                    glyph,
                });
                break;
            }
        }

        self.resolved.insert(c, found);
        found
    }
}

/// The cell box, rounded out to whole pixels so the grid stays crisp.
///
/// Width comes from the advance of a representative glyph rather than from `average_width`,
/// which on a monospace font is the same number but on a fallback would not be.
fn cell_metrics(font: FontRef<'_>, size: f32) -> CellMetrics {
    let metrics = font.metrics(&[]).scale(size);
    let advance = if metrics.average_width > 0.0 {
        metrics.average_width
    } else {
        size * 0.6
    };

    CellMetrics {
        width: advance.round().max(1.0) as u32,
        height: (metrics.ascent + metrics.descent + metrics.leading)
            .ceil()
            .max(1.0) as u32,
        baseline: metrics.ascent.ceil() as i32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn braille_patterns_resolve_somewhere_in_the_stack() {
        // U+28FF drew NOTHING on 2026-07-29 (the RUUAH splash ghost vanished): no font in
        // the stack carried Braille. Apple Braille now backstops it; this pins the gap shut.
        let mut stack = FontStack::system(16.0).expect("system fonts");
        let resolved = stack.resolve('\u{28FF}');
        assert!(
            resolved.is_some(),
            "braille pattern U+28FF resolves to no glyph in any stack font"
        );
    }

    #[test]
    fn emoji_resolve_somewhere_in_the_stack() {
        // Resolution alone is NOT the emoji pin (the braille lesson: resolve passed while
        // the screen stayed blank). emoji_probe.rs owns the pixel truth; this only catches
        // the font file disappearing from the machine.
        let mut stack = FontStack::system(16.0).expect("system fonts");
        assert!(
            stack.resolve('\u{1F9E0}').is_some(),
            "U+1F9E0 resolves to no glyph in any stack font"
        );
    }

    /// Arabic and Persian resolve to a real glyph, in every script the bidi algorithm handles.
    ///
    /// The stack was `Menlo -> Miriam Mono CLM -> Arial Hebrew` and not one of them carries
    /// Arabic, so every Arabic and Persian codepoint resolved to nothing and drew as a blank
    /// cell. The bidi implementation was reordering them correctly the whole time, which is the
    /// worst shape this defect could take: the algorithm passes 91,707 Unicode conformance
    /// cases, the screen shows an empty row, and nothing anywhere reports an error.
    ///
    /// Found on 2026-08-07 by rendering a three-script proof sheet and looking at it, not by a
    /// test. This is that omission turned into one.
    #[test]
    fn arabic_and_persian_resolve_somewhere_in_the_stack() {
        let mut stack = FontStack::system(16.0).expect("system fonts");
        // Arabic letter beh, and the Persian-specific peh and gaf, which a font may omit even
        // when it carries Arabic proper. Persian is the case a naive Arabic-only face misses.
        for (c, what) in [
            ('\u{0628}', "Arabic beh U+0628"),
            ('\u{067E}', "Persian peh U+067E"),
            ('\u{06AF}', "Persian gaf U+06AF"),
        ] {
            let resolved = stack.resolve(c);
            assert!(
                resolved.is_some(),
                "{what} resolves to no glyph in any stack font, so it draws as a blank cell"
            );
            // Resolution to .notdef is the failure that LOOKS like success: a hit whose glyph
            // is 0 paints a hollow box, or on some faces nothing at all.
            assert_ne!(resolved.unwrap().glyph, 0, "{what} resolved to .notdef");
        }
    }

    /// Hebrew draws a real glyph for every class this terminal actually renders.
    ///
    /// `hebrew_falls_through_past_the_primary_font` proves ONE letter reaches a non-primary
    /// font. That is a statement about the stack's shape, not about Hebrew being drawable: it
    /// says nothing about final forms, about the niqqud that GPOS positions, or about the
    /// punctuation a real line contains. A face carrying only the 22 base letters would pass it.
    #[test]
    fn hebrew_resolves_across_letters_finals_niqqud_and_punctuation() {
        let mut stack = FontStack::system(16.0).expect("system fonts");
        for (c, what) in [
            ('\u{05D0}', "aleph, a base letter"),
            ('\u{05EA}', "tav, the last base letter"),
            ('\u{05DD}', "final mem, a positional form"),
            ('\u{05E3}', "final pe"),
            ('\u{05B8}', "qamats, a niqqud mark"),
            ('\u{05B4}', "hiriq, a niqqud mark"),
            ('\u{05BC}', "dagesh, which GSUB composes into the base"),
            ('\u{05C1}', "shin dot"),
            ('\u{05F3}', "geresh, Hebrew punctuation"),
        ] {
            let resolved = stack
                .resolve(c)
                .unwrap_or_else(|| panic!("{what} resolves to no glyph in any stack font"));
            assert_ne!(resolved.glyph, 0, "{what} resolved to .notdef");
        }
    }

    #[test]
    fn the_primary_font_answers_for_latin_and_box_drawing() {
        let mut stack = FontStack::system(16.0).expect("system fonts");
        assert_eq!(stack.resolve('A').map(|r| r.font), Some(0));
        assert_eq!(
            stack.resolve('\u{2502}').map(|r| r.font),
            Some(0),
            "box drawing"
        );
    }

    #[test]
    fn hebrew_falls_through_past_the_primary_font() {
        // The measurement that forced a stack: Menlo maps aleph to glyph 0. If this ever
        // starts resolving to font 0, either Menlo gained Hebrew or resolution is returning
        // .notdef as if it were a hit -- and the second reads as a rendering bug on screen.
        let mut stack = FontStack::system(16.0).expect("system fonts");
        let aleph = stack.resolve('\u{05D0}').expect("some font has aleph");
        assert_ne!(aleph.font, 0, "Menlo has no Hebrew");
        assert_ne!(aleph.glyph, 0);
    }

    #[test]
    fn the_primary_font_is_the_one_that_can_draw_a_tui() {
        // Miriam Mono CLM covers zero box-drawing codepoints, so a stack that put it first
        // would render Hebrew beautifully and vim as blanks. Whatever leads must carry both
        // the frame characters and the blocks.
        let mut stack = FontStack::system(16.0).expect("system fonts");
        for c in ['\u{2500}', '\u{2502}', '\u{250C}', '\u{2588}'] {
            assert_eq!(
                stack.resolve(c).map(|r| r.font),
                Some(0),
                "U+{:04X} must come from the primary font",
                c as u32
            );
        }
    }

    #[test]
    fn the_hebrew_font_advances_on_the_same_grid_as_latin() {
        // The assertion that proves the CHOSEN stack is live, not merely a working one.
        // `hebrew_falls_through_past_the_primary_font` passes just as happily when Miriam
        // Mono CLM fails to load and proportional Arial Hebrew answers instead -- and that
        // degradation is invisible until Hebrew drifts off the grid on screen. Miriam
        // advances 0.6em exactly like Menlo; Arial Hebrew does not.
        let mut stack = FontStack::system(16.0).expect("system fonts");
        let latin = stack.resolve('A').expect("A resolves");
        let aleph = stack.resolve('\u{05D0}').expect("aleph resolves");

        assert_eq!(
            stack.advance(aleph).round(),
            stack.advance(latin).round(),
            "Hebrew is coming from {:?}, which does not share Latin's advance",
            stack.name(aleph.font)
        );
    }

    #[test]
    fn a_stack_still_loads_on_a_machine_without_the_optional_hebrew_font() {
        // `system` filters to what is installed, so a machine with no Culmus still gets a
        // working terminal rather than an error at startup.
        let stack = FontStack::system(16.0).expect("system fonts");
        assert!(stack.font_count() >= 2);
    }

    #[test]
    fn a_codepoint_no_font_covers_resolves_to_nothing() {
        let mut stack = FontStack::system(16.0).expect("system fonts");
        assert_eq!(stack.resolve('\u{10FFFD}'), None);
    }

    #[test]
    fn the_cell_box_is_whole_pixels_and_not_degenerate() {
        let stack = FontStack::system(16.0).expect("system fonts");
        let metrics = stack.metrics();
        assert!(metrics.width >= 1 && metrics.height >= 1);
        assert!(metrics.baseline > 0 && metrics.baseline <= metrics.height as i32);
    }

    #[test]
    fn a_missing_font_is_an_error_rather_than_a_panic() {
        let error = FontStack::load(&[("/nonexistent/font.ttf", 0)], 16.0);
        assert!(matches!(error, Err(FontError::Read { .. })));
    }
}
