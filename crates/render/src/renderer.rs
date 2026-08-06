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
    /// A full-coverage mask for the selection tint, grown on demand and reused. The alpha is
    /// uniform, so one buffer serves every span in every frame.
    selection_mask: Vec<u8>,
}

impl Renderer<Canvas> {
    /// The CPU reference backend.
    pub fn new(fonts: FontStack, cols: u16, rows: u16) -> Renderer<Canvas> {
        Renderer::with_surface(fonts, cols, rows)
    }
}

impl<S: Surface> Renderer<S> {
    /// The same renderer on any backend.
    ///
    /// Builds its own backing surface, which on a GPU backend means its own device. An
    /// embedder presenting into a window must NOT use this to rebuild - see
    /// `from_surface`.
    pub fn with_surface(fonts: FontStack, cols: u16, rows: u16) -> Renderer<S> {
        let cell = fonts.metrics();
        let canvas = S::with_size(cell.width * u32::from(cols), cell.height * u32::from(rows));
        Renderer::from_surface(fonts, canvas, cols, rows)
    }

    /// The same renderer over a surface the caller already owns.
    ///
    /// Why this exists: `with_surface` constructs the backend through `Surface::with_size`,
    /// and a GPU backend built that way brings a whole new device with it. A rebuild - resize,
    /// zoom, font change - therefore moved the renderer onto a different device while the
    /// window's swapchain stayed on the old one, silently: no error, no crash, just a frame
    /// that never followed the window again. Handing the surface in is what keeps one device
    /// across every rebuild.
    pub fn from_surface(fonts: FontStack, canvas: S, cols: u16, rows: u16) -> Renderer<S> {
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
            selection_mask: Vec::new(),
        }
    }

    /// How strongly the selection tint covers what is under it, out of 255.
    ///
    /// Set for READABILITY, not for weight: at this coverage the ink under the tint stays
    /// clearly darker than the tint itself, so selected text is still text. Raising it toward
    /// opaque is what turns a highlight into a redaction.
    pub const SELECTION_ALPHA: u8 = 0x54;

    /// The pixel cell the grid is laid out on. The host's mouse encoder divides
    /// pointer pixels by this, so it must come from the LIVE renderer -- a zoom
    /// rebuild changes it.
    pub fn cell_metrics(&self) -> crate::font::CellMetrics {
        self.fonts.metrics()
    }

    pub fn canvas(&self) -> &S {
        &self.canvas
    }

    /// The finished pixels, however the backend stores them.
    ///
    /// On a GPU backend this is a full frame copied back across the bus. A caller that is
    /// putting the frame on a screen does not need it and should reach the backend through
    /// `surface_mut` instead.
    pub fn pixels(&mut self) -> Vec<u8> {
        self.canvas.read_pixels()
    }

    /// The backend itself, for an embedder that presents rather than reads back.
    pub fn surface_mut(&mut self) -> &mut S {
        &mut self.canvas
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
        self.draw_row_parts(frame, y, Parts::Both, &[]);
        self.draw_selection(frame, y);
    }

    /// Tints the selected span of a row, OVER the text.
    ///
    /// A translucent overlay rather than a background swap, and the choice is not cosmetic.
    /// Backgrounds are painted per cell, interleaved with ink, so a selection painted as a
    /// background would be erased by the next cell's own background and would be invisible
    /// wherever the child coloured anything - a selected prompt, a selected diff line, a
    /// selected `ls` listing. Blending over the finished row costs one pass and works against
    /// every background there is, including an image.
    ///
    /// The cost, stated rather than hidden: the ink under the tint shifts colour slightly.
    /// Ghostty swaps foreground AND background for selected cells and keeps its contrast
    /// exactly; this keeps the text where it was and accepts a small shift. Revisit if a
    /// selected line ever reads as unreadable rather than as highlighted.
    fn draw_selection(&mut self, frame: &Frame, y: u16) {
        let Some(selection) = frame.selection else {
            return;
        };
        let Some((from, to)) = selection.span_on(y, self.cols) else {
            return;
        };
        let cell = self.fonts.metrics();
        let width = cell.width * u32::from(to - from + 1);
        let height = cell.height;
        // One full-coverage mask, reused for every span in the frame: the alpha is uniform,
        // so building it per row would allocate once per selected line for no difference.
        if self.selection_mask.len() < (width * height) as usize {
            self.selection_mask.resize((width * height) as usize, Self::SELECTION_ALPHA);
        }
        self.canvas.blend_mask(
            i32::from(from) * cell.width as i32,
            i32::from(y) * cell.height as i32,
            width,
            height,
            &self.selection_mask[..(width * height) as usize],
            self.palette.selection_background(),
        );
    }

    /// One row, or half of one.
    ///
    /// `Parts::Both` is the ordinary path and is byte-for-byte what this function always
    /// did: per cell, background then ink, interleaved. The split halves exist only for
    /// the layered path, where an image has to land BETWEEN all the backgrounds and all
    /// the text. Splitting is deliberately NOT the default -- interleaved, a later cell's
    /// background erases the previous cell's glyph overhang, and separating the passes
    /// keeps that overhang. Both are defensible; only one is what every existing pixel
    /// test was written against.
    ///
    /// `bg_skip` names cell spans lying over a below-background image, where the DEFAULT
    /// background must not be painted or the image it sits on would never be visible. A
    /// cell carrying a real background colour still paints: the child asked for that.
    fn draw_row_parts(&mut self, frame: &Frame, y: u16, parts: Parts, bg_skip: &[CellRect]) {
        let cell = self.fonts.metrics();
        let top = i32::from(y) * cell.height as i32;

        // The row is cleared first, so a shorter line of text does not leave the tail of a
        // longer one behind it.
        if parts.backgrounds() {
            for (start, span) in unskipped_spans(self.cols, y, bg_skip) {
                self.canvas.fill(
                    i32::from(start) * cell.width as i32,
                    top,
                    cell.width * u32::from(span),
                    cell.height,
                    self.palette.default_background,
                );
            }
        }

        for run in frame.runs(y) {
            let drawn = self.palette.draw(&run.style);
            // Ligature pass: plan which segments of this run form ligatures. Ink for
            // their cells is skipped below and blitted from the plan afterwards;
            // backgrounds and decorations stay per-cell.
            let plans = if self.ligatures && drawn.ink && parts.ink() {
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
                // A default background over a below-background image is the ONE fill that
                // yields; anything the child actually coloured still covers the image.
                let yields = drawn.background == self.palette.default_background
                    && covers(bg_skip, y, column, width);
                if parts.backgrounds() && !yields {
                    self.canvas.fill(
                        left,
                        top,
                        cell.width * u32::from(width),
                        cell.height,
                        drawn.background,
                    );
                }
                if !parts.ink() {
                    continue;
                }
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
            for (start, _, glyphs) in plans.iter().filter(|_| parts.ink()) {
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

        if parts.ink() {
            self.draw_cursor(frame, y);
        }
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

        // VS16 (emoji presentation): the selector's entire meaning is "take the emoji
        // face even though a text font covers the base". Resolved directly -- the
        // shaper would pick the face by the FIRST character and never consult the
        // selector. Width stays 1 (oracle-measured, vs16-cluster-stays-narrow); the
        // color arm below fits the artwork inside the single cell.
        if cluster.contains('\u{FE0F}') {
            if let Some(base) = cluster.chars().next() {
                if let Some(resolved) = self.fonts.resolve_emoji(base) {
                    let key = crate::atlas::GlyphKey {
                        font: resolved.font,
                        glyph: resolved.glyph,
                    };
                    if let Some(glyph) = self.atlas.glyph(&self.fonts, key) {
                        if let crate::atlas::GlyphData::Color(rgba) = &glyph.data {
                            let span_px = metrics.width * u32::from(span.max(1));
                            let (width, height) = if glyph.width > span_px {
                                // Refit by WIDTH so a square heart cannot bleed into
                                // the neighbor cell; deterministic fixed-point scale,
                                // both backends get the same bytes.
                                let height =
                                    (glyph.height as u64 * span_px as u64 / glyph.width as u64)
                                        .max(1) as u32;
                                (span_px, height)
                            } else {
                                (glyph.width, glyph.height)
                            };
                            let scaled = if (width, height) == (glyph.width, glyph.height) {
                                rgba.clone()
                            } else {
                                crate::images::scale_to(glyph.width, glyph.height, rgba, width, height)
                            };
                            let x = left + (span_px as i32 - width as i32) / 2;
                            let y = top + (metrics.height as i32 - height as i32) / 2;
                            self.canvas.blend_image(x, y, width, height, &scaled);
                            return;
                        }
                    }
                }
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

    /// Draws a whole frame in the three kitty z layers, for frames that need it.
    ///
    /// Returns the rows painted, which is always all of them: an image sitting under the
    /// text spans rows the damage tracker has no reason to consider stale, so a partial
    /// repaint would leave the previous frame's text sitting on top of it. Paying a full
    /// repaint whenever a below-text placement exists keeps the incremental-equals-full
    /// invariant true instead of quietly making it false, and such placements are rare.
    ///
    /// `resolved` is indexed in lockstep with `placements` -- the caller resolved the
    /// pixels, so this cannot reorder the list (and it must not: `placements` arrives in
    /// draw order from the publisher).
    pub fn draw_layered(
        &mut self,
        frame: &Frame,
        placements: &[ruuah_vt_frame::FramePlacement],
        resolved: &[Option<(u32, u32, std::sync::Arc<Vec<u8>>)>],
    ) -> usize {
        let layer_of = |index: usize| placements[index].layer();

        // Layer 0 first, and its covered cells are collected on the way: a default
        // background painted over them would hide the very image they belong to.
        let mut bg_skip = Vec::new();
        for index in (0..placements.len()).filter(|i| layer_of(*i) == 0) {
            if let Some(rect) = self.blit_placement(&placements[index], resolved[index].as_ref()) {
                bg_skip.push(rect);
            }
        }

        for y in 0..frame.rows {
            self.draw_row_parts(frame, y, Parts::Backgrounds, &bg_skip);
        }
        for index in (0..placements.len()).filter(|i| layer_of(*i) == 1) {
            self.blit_placement(&placements[index], resolved[index].as_ref());
        }
        for y in 0..frame.rows {
            self.draw_row_parts(frame, y, Parts::Ink, &bg_skip);
        }
        for index in (0..placements.len()).filter(|i| layer_of(*i) == 2) {
            self.blit_placement(&placements[index], resolved[index].as_ref());
        }

        self.drawn = frame.generation;
        usize::from(frame.rows)
    }

    /// Draws every unicode-placeholder run in the frame.
    ///
    /// The runs come from the grid itself (`ruuah_vt_frame::virtual_runs`), so an image
    /// placed this way scrolls, reflows and is erased exactly like the text it is made of
    /// -- no anchor to keep in step, which is the whole reason kitty added the feature.
    ///
    /// Each run draws a CROP: the run knows which cells of the image it shows, so the
    /// image is scaled once to its full cell box and the run copies its own slice out of
    /// that. Drawing the whole image per run and relying on clipping would be simpler and
    /// wrong -- a run in the middle of a scrolled image would draw the top of it.
    ///
    /// `lookup` resolves an image id to its pixels; an unknown image draws nothing, which
    /// is what a placeholder for an image that was never transmitted must do.
    pub fn draw_placeholders<F>(&mut self, frame: &Frame, mut lookup: F)
    where
        F: FnMut(u32) -> Option<(u32, u32, std::sync::Arc<Vec<u8>>)>,
    {
        let metrics = self.fonts.metrics();
        for y in 0..frame.rows {
            for run in ruuah_vt_frame::virtual_runs(frame, y) {
                let Some((width, height, rgba)) = lookup(run.image) else {
                    continue;
                };
                if width == 0 || height == 0 {
                    continue;
                }

                // The cell grid the image is divided into. A virtual placement may state
                // it; otherwise the image's own pixel size decides, rounded UP so the last
                // partial cell still belongs to the image.
                let declared = frame.virtuals.iter().find(|v| v.image == run.image);
                let (cols, rows) = match declared {
                    Some(v) if v.cols > 0 && v.rows > 0 => (u32::from(v.cols), u32::from(v.rows)),
                    _ => (
                        width.div_ceil(metrics.width.max(1)),
                        height.div_ceil(metrics.height.max(1)),
                    ),
                };
                if cols == 0 || rows == 0 || run.image_col >= cols || run.image_row >= rows {
                    continue;
                }

                let scaled = self.image_cache.get_or_scale(
                    run.image,
                    width,
                    height,
                    &rgba,
                    cols * metrics.width,
                    rows * metrics.height,
                );

                // The run's slice of that scaled image, clamped to what the image has.
                let take = u32::from(run.width).min(cols - run.image_col);
                let crop = crop_cells(
                    &scaled,
                    cols * metrics.width,
                    run.image_col * metrics.width,
                    run.image_row * metrics.height,
                    take * metrics.width,
                    metrics.height,
                );
                self.canvas.blend_image(
                    i32::from(run.screen_col) * metrics.width as i32,
                    i32::from(run.screen_row) * metrics.height as i32,
                    take * metrics.width,
                    metrics.height,
                    &crop,
                );
            }
        }
    }

    /// Scales and blits ONE placement, returning the cell box it covers.
    ///
    /// The cell box is what the below-background layer needs, and it is derived from the
    /// pixels actually drawn rather than re-derived from the placement, so the two can
    /// never disagree about where the image is.
    fn blit_placement(
        &mut self,
        placement: &ruuah_vt_frame::FramePlacement,
        image: Option<&(u32, u32, std::sync::Arc<Vec<u8>>)>,
    ) -> Option<CellRect> {
        let metrics = self.fonts.metrics();
        let (width, height, rgba) = image?;
        let (target_width, target_height) = if placement.cols > 0 && placement.rows > 0 {
            (
                u32::from(placement.cols) * metrics.width,
                u32::from(placement.rows) * metrics.height,
            )
        } else {
            (*width, *height)
        };
        if target_width == 0 || target_height == 0 || *width == 0 || *height == 0 {
            return None;
        }
        let scaled = self.image_cache.get_or_scale(
            placement.image,
            *width,
            *height,
            rgba,
            target_width,
            target_height,
        );
        let x = i32::from(placement.col) * metrics.width as i32;
        let y = i32::from(placement.row) * metrics.height as i32; // negative = clipped top
        self.canvas
            .blend_image(x, y, target_width, target_height, &scaled);

        // Ceiling division: a cell the image only partly covers is still covered.
        Some(CellRect {
            col: placement.col,
            row: placement.row,
            cols: target_width.div_ceil(metrics.width.max(1)) as u16,
            rows: target_height.div_ceil(metrics.height.max(1)) as u16,
        })
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
            let y = i32::from(placement.row) * metrics.height as i32; // negative = clipped top
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

/// Which half of a row a draw pass paints.
///
/// `Both` is the ordinary interleaved path; the halves exist so a below-text image can be
/// blitted between all the backgrounds and all the glyphs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Parts {
    Both,
    Backgrounds,
    Ink,
}

impl Parts {
    fn backgrounds(self) -> bool {
        matches!(self, Parts::Both | Parts::Backgrounds)
    }
    fn ink(self) -> bool {
        matches!(self, Parts::Both | Parts::Ink)
    }
}

/// A cell-space box covered by a below-background placement.
#[derive(Clone, Copy, Debug)]
struct CellRect {
    col: u16,
    /// Signed like the placement it came from: an image scrolled past the top covers
    /// rows starting above the screen, and clamping that to 0 would shield the wrong ones.
    row: i16,
    cols: u16,
    rows: u16,
}

/// Whether a cell span at `(y, col..col+width)` lies over any below-background image.
fn covers(rects: &[CellRect], y: u16, col: u16, width: u16) -> bool {
    rects.iter().any(|rect| {
        let top = i32::from(rect.row);
        let bottom = top + i32::from(rect.rows);
        let left = i32::from(rect.col);
        let right = left + i32::from(rect.cols);
        let row = i32::from(y);
        let (start, end) = (i32::from(col), i32::from(col) + i32::from(width));
        row >= top && row < bottom && start < right && end > left
    })
}

/// The column spans of row `y` NOT covered by any below-background image, left to right.
///
/// Used for the row-clear fill, which would otherwise paint the default background across
/// the whole width and bury the image before a single glyph was drawn.
fn unskipped_spans(cols: u16, y: u16, rects: &[CellRect]) -> Vec<(u16, u16)> {
    if rects.is_empty() {
        return vec![(0, cols)];
    }
    let mut spans = Vec::new();
    let mut start: Option<u16> = None;
    for column in 0..cols {
        if covers(rects, y, column, 1) {
            if let Some(from) = start.take() {
                spans.push((from, column - from));
            }
        } else if start.is_none() {
            start = Some(column);
        }
    }
    if let Some(from) = start {
        spans.push((from, cols - from));
    }
    spans
}

/// Copies a rectangle out of an RGBA image.
///
/// Rows outside the source contribute transparent pixels rather than wrapping to the next
/// line, which is the failure that makes a crop look like a shear.
fn crop_cells(
    source: &[u8],
    source_width: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let mut out = vec![0u8; (width * height * 4) as usize];
    for row in 0..height {
        let src_y = y + row;
        for column in 0..width {
            let src_x = x + column;
            if src_x >= source_width {
                continue;
            }
            let src = ((src_y * source_width + src_x) * 4) as usize;
            let dst = ((row * width + column) * 4) as usize;
            if src + 4 <= source.len() {
                out[dst..dst + 4].copy_from_slice(&source[src..src + 4]);
            }
        }
    }
    out
}
