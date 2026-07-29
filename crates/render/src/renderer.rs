//! Purpose: paint a frame into a canvas, repainting only what changed.
//! Public surface: `Renderer`.
//! Why this file: this is the consumer the `Run` seam exists for. Every column it paints
//!   comes from `Run::column_of`, never from adding an index to a start -- which today is a
//!   distinction without a difference and in slice 5.5 is the difference between Hebrew
//!   rendering correctly and rendering backwards. Drawing a row is a pure function of the
//!   frame and the row index, which is what makes "an incremental redraw equals a full one"
//!   a testable claim rather than a hope.
//! NOT responsible for: pixels (`canvas.rs`), glyph shapes (`atlas.rs`), colour rules
//!   (`color.rs`), or font choice (`font.rs`).
//! Test strategy: `tests/redraw.rs` compares incremental against full and proves the
//!   comparison can fail; `tests/vim.rs` runs the real editor through the real pty.

use ruuah_vt_frame::{Frame, PackedCell, cell_width};

use crate::atlas::Atlas;
use crate::canvas::Canvas;
use crate::color::{Palette, Rgba};
use crate::font::FontStack;
use crate::shape::{PositionedGlyph, Shaper, needs_shaping};
use crate::surface::Surface;

/// Paints frames into a surface.
///
/// Generic over the backend, with the CPU canvas as the default so every existing caller and
/// test reads unchanged. `Renderer::new` resolves to the CPU reference; a second backend is
/// reached through `with_surface`, which is what `tests/backend.rs` uses to run the identical
/// drawing logic two ways.
pub struct Renderer<S = Canvas> {
    fonts: FontStack,
    atlas: Atlas,
    palette: Palette,
    canvas: S,
    shaper: Shaper,
    /// Scratch for one cell's positioned glyphs, so drawing allocates nothing per cell.
    positioned: Vec<PositionedGlyph>,
    /// Whether combining marks are placed by the font. Off is a test control only.
    shaping: bool,
    /// Whether the caret follows its cell through the bidi layout. Off is a test control only.
    visual_caret: bool,
    /// Whether same-style ASCII segments may form ligatures (config `font-ligatures`).
    /// With a non-ligating lead font this changes nothing, by construction.
    ligatures: bool,
    cols: u16,
    rows: u16,
    /// The last generation fully painted. Rows stamped above it are what is owed.
    drawn: u64,
    /// Scaled kitty placements, keyed (image, box) -- see `images.rs`.
    image_cache: crate::images::ScaledCache,
}

impl Renderer<Canvas> {
    /// The CPU reference backend.
    pub fn new(fonts: FontStack, cols: u16, rows: u16) -> Renderer<Canvas> {
        Renderer::with_surface(fonts, cols, rows)
    }
}

impl<S: Surface> Renderer<S> {
    /// The same renderer on any backend.
    pub fn with_surface(fonts: FontStack, cols: u16, rows: u16) -> Renderer<S> {
        let cell = fonts.metrics();
        let canvas = S::with_size(cell.width * u32::from(cols), cell.height * u32::from(rows));
        Renderer {
            fonts,
            atlas: Atlas::new(),
            shaper: Shaper::new(),
            positioned: Vec::with_capacity(8),
            shaping: true,
            visual_caret: true,
            ligatures: true,
            palette: Palette::xterm(),
            canvas,
            cols,
            rows,
            drawn: 0,
            image_cache: crate::images::ScaledCache::default(),
        }
    }

    pub fn canvas(&self) -> &S {
        &self.canvas
    }

    /// The finished pixels, however the backend stores them.
    pub fn pixels(&mut self) -> Vec<u8> {
        self.canvas.read_pixels()
    }

    pub fn atlas(&self) -> &Atlas {
        &self.atlas
    }

    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    /// Replaces the palette wholesale. Callers apply a theme before the first draw (or
    /// re-apply it after rebuilding the renderer on resize); pixels already painted are
    /// not repainted, so a mid-life swap shows only on rows drawn after it.
    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
    }

    /// Turns off font-driven mark placement, so every glyph of a cluster stacks at the pen.
    ///
    /// A control, not a feature. `tests/shaping.rs` requires this to put a niqqud somewhere
    /// the real path does not; a positioning test that has never been seen to fail cannot
    /// tell a working GPOS lookup from an ignored one.
    #[doc(hidden)]
    pub fn set_ligatures(&mut self, on: bool) {
        self.ligatures = on;
    }

    pub fn set_shaping_for_testing(&mut self, on: bool) {
        self.shaping = on;
    }

    /// Puts the caret back at the cursor's logical column, which is where it went before the
    /// visual mapping existed.
    ///
    /// A control, not a feature. On a left-to-right row this is indistinguishable from the
    /// real path, and that is the point: `tests/caret.rs` requires the two to agree on Latin
    /// and disagree on Hebrew, so a mapping that had quietly become the identity function
    /// would fail the second half instead of passing both.
    #[doc(hidden)]
    pub fn set_visual_caret_for_testing(&mut self, on: bool) {
        self.visual_caret = on;
    }

    /// Repaints the rows this frame says are stale, and returns how many that was.
    ///
    /// Zero is the common and desirable answer: a terminal nobody is typing into should cost
    /// nothing per frame.
    pub fn draw(&mut self, frame: &Frame) -> usize {
        let stale: Vec<u16> = frame.stale_rows(self.drawn).collect();
        for y in &stale {
            self.draw_row(frame, *y);
        }
        self.drawn = frame.generation;
        stale.len()
    }

    /// The same as `draw`, but silently declines to repaint one stale row.
    ///
    /// This is a broken renderer on purpose. `tests/redraw.rs` compares an incremental redraw
    /// against a full one, and a comparison that has never been seen to fail is not evidence;
    /// this is what makes it fail. It has no legitimate caller.
    #[doc(hidden)]
    pub fn draw_skipping_for_testing(&mut self, frame: &Frame, skip: u16) -> usize {
        let stale: Vec<u16> = frame
            .stale_rows(self.drawn)
            .filter(|y| *y != skip)
            .collect();
        for y in &stale {
            self.draw_row(frame, *y);
        }
        self.drawn = frame.generation;
        stale.len()
    }

    /// Repaints every row regardless of damage. The reference the incremental path is
    /// measured against.
    pub fn draw_all(&mut self, frame: &Frame) -> usize {
        for y in 0..self.rows.min(frame.rows) {
            self.draw_row(frame, y);
        }
        self.drawn = frame.generation;
        usize::from(self.rows.min(frame.rows))
    }

    /// Paints one row. A pure function of `(frame, y)` -- nothing here reads or writes state
    /// that outlives the call except the glyph cache, which cannot change what is drawn.
    fn draw_row(&mut self, frame: &Frame, y: u16) {
        let cell = self.fonts.metrics();
        let top = i32::from(y) * cell.height as i32;

        // The row is cleared first, so a shorter line of text does not leave the tail of a
        // longer one behind it.
        self.canvas.fill(
            0,
            top,
            cell.width * u32::from(self.cols),
            cell.height,
            self.palette.default_background,
        );

        for run in frame.runs(y) {
            let drawn = self.palette.draw(&run.style);
            // Ligature pass: plan which segments of this run form ligatures. Ink for
            // their cells is skipped below and blitted from the plan afterwards;
            // backgrounds and decorations stay per-cell.
            let plans = if self.ligatures && drawn.ink {
                self.ligature_plan(&run)
            } else {
                Vec::new()
            };
            for (index, glyph_cell) in run.cells.iter().enumerate() {
                let column = run.column_of(index);
                let width = cell_width(*glyph_cell);
                if width == 0 {
                    continue;
                }
                let left = i32::from(column) * cell.width as i32;
                self.canvas.fill(
                    left,
                    top,
                    cell.width * u32::from(width),
                    cell.height,
                    drawn.background,
                );
                let ligated = plans
                    .iter()
                    .any(|(start, count, _)| index >= *start && index < start + count);
                if drawn.ink && !ligated {
                    let mirrored = run.direction == ruuah_vt_frame::Direction::RightToLeft;
                    self.draw_cell(*glyph_cell, left, top, width, drawn.foreground, mirrored);
                }
                self.draw_decorations(&drawn, left, top, width);
            }
            // The ligature ink goes over the freshly painted backgrounds.
            for (start, _, glyphs) in &plans {
                let origin = i32::from(run.column_of(*start)) * cell.width as i32;
                let baseline = top + cell.baseline;
                for placed in glyphs {
                    let Some(glyph) = self.atlas.glyph(&self.fonts, placed.key) else {
                        continue;
                    };
                    if let crate::atlas::GlyphData::Mask(coverage) = &glyph.data {
                        let x = origin + placed.x.round() as i32 + glyph.left;
                        let y = baseline - placed.y.round() as i32 - glyph.top;
                        let (width, height) = (glyph.width, glyph.height);
                        self.canvas
                            .blend_mask(x, y, width, height, coverage, drawn.foreground);
                    }
                }
            }
        }

        self.draw_cursor(frame, y);
    }

    /// Plans ligature segments for one run: maximal stretches of single-codepoint
    /// printable-ASCII narrow cells, shaped as one string with calt/liga. A plan is
    /// kept ONLY when a ligature actually formed, so non-ligating fonts draw exactly
    /// as they always did.
    fn ligature_plan(
        &mut self,
        run: &ruuah_vt_frame::Run<'_>,
    ) -> Vec<(usize, usize, Vec<PositionedGlyph>)> {
        if run.direction == ruuah_vt_frame::Direction::RightToLeft {
            return Vec::new();
        }
        let mut plans = Vec::new();
        let mut index = 0usize;
        let cells = run.cells;
        while index < cells.len() {
            let mut text = String::new();
            let mut end = index;
            while end < cells.len() {
                let cell = cells[end];
                if cell_width(cell) != 1 {
                    break;
                }
                let mut scratch = [0u8; ruuah_vt_frame::CLUSTER_BYTES];
                let cluster = cell.cluster(&mut scratch);
                let mut chars = cluster.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) if ('!'..='~').contains(&c) => text.push(c),
                    _ => break,
                }
                end += 1;
            }
            if end - index >= 2 {
                let (glyphs, formed) = self.shaper.shape_run(&mut self.fonts, &text);
                if formed {
                    plans.push((index, end - index, glyphs.to_vec()));
                }
            }
            index = end.max(index + 1);
        }
        plans
    }

    fn draw_cell(
        &mut self,
        cell: PackedCell,
        left: i32,
        top: i32,
        span: u16,
        color: Rgba,
        mirrored: bool,
    ) {
        if !cell.has_text() {
            return;
        }
        let metrics = self.fonts.metrics();
        let baseline = top + metrics.baseline;

        let mut scratch = [0u8; ruuah_vt_frame::CLUSTER_BYTES];
        let mut cluster = cell.cluster(&mut scratch);

        // Block mosaics are synthesized at exactly this cell's geometry so they meet
        // edge-to-edge; a fallback font's block fills that font's em, not this grid's
        // cell, and the gutters shred any mosaic art (see mosaic.rs).
        let mut chars = cluster.chars();
        if let (Some(first), None) = (chars.next(), chars.next()) {
            if let Some(mask) = crate::mosaic::coverage(first, metrics.width, metrics.height) {
                self.canvas
                    .blend_mask(left, top, metrics.width, metrics.height, &mask, color);
                return;
            }
        }

        // UBA rule L4: paired punctuation at an RTL resolved level draws its mirrored
        // counterpart -- reordering alone turns `[OK]` into `]OK[`. Display-only, single
        // codepoints only (a cluster with marks is never a bracket), and the CELL is
        // untouched: programs reading the grid back see what they wrote.
        let mut mirror_buf = [0u8; 4];
        if mirrored {
            let mut chars = cluster.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                if let Some(m) = ruuah_vt_frame::mirror(c) {
                    cluster = m.encode_utf8(&mut mirror_buf);
                }
            }
        }

        // Shaping is what makes a combining mark land where the font says rather than where
        // its own origin happens to be. Only clusters that actually have marks pay for it:
        // a single codepoint has nothing to attach, so every character of code and English
        // takes `place_at_origin`, which for one glyph is not an approximation but the exact
        // answer.
        self.positioned.clear();
        let placed = if self.shaping && needs_shaping(cluster) {
            self.shaper.shape(&mut self.fonts, cluster)
        } else {
            self.shaper.place_at_origin(&mut self.fonts, cluster)
        };
        self.positioned.extend_from_slice(placed);

        for placed in 0..self.positioned.len() {
            let placed = self.positioned[placed];
            let Some(glyph) = self.atlas.glyph(&self.fonts, placed.key) else {
                continue;
            };

            match &glyph.data {
                crate::atlas::GlyphData::Mask(coverage) => {
                    // `y` is positive upwards in both the shaper's offsets and the glyph's
                    // `top`, and the canvas grows downwards, so both subtract.
                    let x = left + placed.x.round() as i32 + glyph.left;
                    let y = baseline - placed.y.round() as i32 - glyph.top;
                    let (width, height) = (glyph.width, glyph.height);
                    self.canvas.blend_mask(x, y, width, height, coverage, color);
                }
                crate::atlas::GlyphData::Color(rgba) => {
                    // Emoji carry their own colors and were pre-scaled to the cell height
                    // in the atlas; centered over the cell span (a wide cell is two cells
                    // of pixels). The foreground tint is deliberately NOT applied -- that
                    // is the gray-silhouette bug.
                    let span_px = (metrics.width * u32::from(span.max(1))) as i32;
                    let x = left + (span_px - glyph.width as i32) / 2;
                    let y = top + (metrics.height as i32 - glyph.height as i32) / 2;
                    self.canvas
                        .blend_image(x, y, glyph.width, glyph.height, rgba);
                }
            }
        }
    }

    fn draw_decorations(&mut self, drawn: &crate::color::Drawn, left: i32, top: i32, width: u16) {
        use ruuah_vt_snapshot::Underline;

        let metrics = self.fonts.metrics();
        let span = metrics.width * u32::from(width);

        if drawn.underline != Underline::None {
            // Thickness scales with the cell so a 4k window does not get a hairline;
            // every pattern is integer fills, so CPU and GPU stay byte-equal for free.
            let t = (metrics.height / 16).max(1);
            let y = (top + metrics.baseline as i32 + 1)
                .min(top + metrics.height as i32 - (3 * t) as i32);
            let color = drawn.underline_color;
            match drawn.underline {
                Underline::None => {}
                Underline::Single => self.canvas.fill(left, y, span, t, color),
                Underline::Double => {
                    self.canvas.fill(left, y, span, t, color);
                    self.canvas.fill(left, y + 2 * t as i32, span, t, color);
                }
                // A square undercurl: chunks of t alternate between two rows. Not a
                // sine, deliberately -- integer fills keep both backends identical.
                Underline::Curly => {
                    let mut x = 0u32;
                    let mut high = true;
                    while x < span {
                        let w = t.min(span - x);
                        let dy = if high { 0 } else { t as i32 };
                        self.canvas.fill(left + x as i32, y + dy, w, t, color);
                        x += t;
                        high = !high;
                    }
                }
                Underline::Dotted => {
                    let mut x = 0u32;
                    while x < span {
                        let w = t.min(span - x);
                        self.canvas.fill(left + x as i32, y, w, t, color);
                        x += 2 * t;
                    }
                }
                Underline::Dashed => {
                    let dash = 3 * t;
                    let mut x = 0u32;
                    while x < span {
                        let w = dash.min(span - x);
                        self.canvas.fill(left + x as i32, y, w, t, color);
                        x += dash + 2 * t;
                    }
                }
            }
        }
        if drawn.strikethrough {
            let y = top + metrics.baseline - (metrics.baseline / 3);
            self.canvas.fill(left, y, span, 1, drawn.foreground);
        }
    }

    /// Blits kitty placements over the drawn grid, through the same `blend_image` op
    /// emoji use -- CPU==GPU holds because both backends receive identical pre-scaled
    /// bytes (the P0.2 rule). `lookup` resolves an image id to (width, height, pixels);
    /// a placement whose image is gone draws nothing. A `cols`/`rows` of 0 means the
    /// image's native pixel size (the core is cell-pixel-ignorant by design).
    pub fn draw_images<F>(&mut self, placements: &[ruuah_vt_frame::FramePlacement], mut lookup: F)
    where
        F: FnMut(u32) -> Option<(u32, u32, std::sync::Arc<Vec<u8>>)>,
    {
        let metrics = self.fonts.metrics();
        for placement in placements {
            let Some((width, height, rgba)) = lookup(placement.image) else {
                continue;
            };
            let (target_width, target_height) = if placement.cols > 0 && placement.rows > 0 {
                (
                    u32::from(placement.cols) * metrics.width,
                    u32::from(placement.rows) * metrics.height,
                )
            } else {
                (width, height)
            };
            if target_width == 0 || target_height == 0 || width == 0 || height == 0 {
                continue;
            }
            let scaled = self.image_cache.get_or_scale(
                placement.image,
                width,
                height,
                &rgba,
                target_width,
                target_height,
            );
            let x = i32::from(placement.col) * metrics.width as i32;
            let y = i32::from(placement.row) * metrics.height as i32;
            self.canvas
                .blend_image(x, y, target_width, target_height, &scaled);
        }
    }

    /// Drops cached scaled boxes for an image whose pixels changed or vanished.
    pub fn evict_image(&mut self, id: u32) {
        self.image_cache.evict(id);
    }

    /// Draws the cursor, if it is on this row.
    ///
    /// Painted as part of the row rather than after every frame, which is what keeps the
    /// redraw invariant true: the core already marks both the row the cursor left and the
    /// row it arrived at as damaged, so a moved cursor repaints both and leaves no ghost.
    ///
    /// The column comes from `visual_column`, never from `cursor.x` directly. The cursor is
    /// reported in logical coordinates, and on a reordered row the cell it names is painted
    /// somewhere else -- so drawing at `cursor.x` puts the caret on a glyph the program never
    /// addressed, while still looking entirely reasonable.
    fn draw_cursor(&mut self, frame: &Frame, y: u16) {
        if !frame.cursor.visible || frame.cursor.y != y || frame.cursor.x >= self.cols {
            return;
        }
        let column = if self.visual_caret {
            frame.visual_column(frame.cursor.x, y)
        } else {
            frame.cursor.x
        };
        let metrics = self.fonts.metrics();
        let drawn = self.palette.draw(&frame.cursor.style);
        let left = i32::from(column) * metrics.width as i32;
        let top = i32::from(y) * metrics.height as i32;

        self.canvas
            .fill(left, top, metrics.width, metrics.height, drawn.foreground);
        // The caret redraws the cell it covers, so it must keep the run's mirroring --
        // otherwise a bracket in an RTL run flips back the moment the cursor lands on it.
        // `logical_start` is the cursor's coordinate space; `start` is not.
        let mirrored = frame.runs(y).into_iter().any(|run| {
            run.direction == ruuah_vt_frame::Direction::RightToLeft
                && (run.logical_start..run.logical_start + run.cells.len() as u16)
                    .contains(&frame.cursor.x)
        });
        let cell = frame.cell(frame.cursor.x, y);
        self.draw_cell(cell, left, top, cell_width(cell), drawn.background, mirrored);
    }
}
