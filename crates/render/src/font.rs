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
    pub fn system(size: f32) -> Result<FontStack, FontError> {
        let home = std::env::var("HOME").unwrap_or_default();
        let candidates = [
            ("/System/Library/Fonts/Menlo.ttc".to_string(), 0),
            (format!("{home}/Library/Fonts/MiriamMonoCLM-Book.ttf"), 0),
            ("/System/Library/Fonts/ArialHB.ttc".to_string(), 0),
            // Braille patterns (U+2800..U+28FF): none of the fonts above carry them, so
            // dot art and braille spinners rendered as NOTHING until 2026-07-29, when the
            // RUUAH splash's ghost simply failed to appear. Apple Braille ships on every
            // macOS and sits last so it can never shadow a glyph the leads own.
            ("/System/Library/Fonts/Apple Braille.ttf".to_string(), 0),
        ];

        let present: Vec<(&str, usize)> = candidates
            .iter()
            .filter(|(path, _)| std::path::Path::new(path).is_file())
            .map(|(path, index)| (path.as_str(), *index))
            .collect();

        FontStack::load(&present, size)
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
