//! Purpose: N sessions in one area - the panes a wizard-declared canvas actually contains.
//! Public surface: `Canvas` (the live one), `PaneSpec`, `CanvasError`.
//! Why this file: `layout::Canvas` says where the rectangles ARE; this says what is inside them
//!   and, above all, that each pane's pty was told ITS OWN size. That is the silent failure the
//!   whole slice exists to prevent: a pane that keeps the full window's column count draws a
//!   perfectly healthy terminal whose right-hand columns land underneath its neighbour. Nothing
//!   errors, nothing looks broken, and the child is confidently writing into pixels nobody can
//!   see - which reads as an agent that stopped half way through a line.
//! NOT responsible for: windows, swapchains or input. The host owns those; this owns the panes
//!   and their geometry, so it can be tested with real shells and no screen at all.
//! Test strategy: the pty is asked what size it thinks it is, by the CHILD, with `stty size`.
//!   Deriving cols in Rust and asserting Rust's own arithmetic would pass on exactly the bug
//!   being hunted - the number has to come back from the other side of the pseudoterminal.

use std::process::Command;

use mind2t_vt_host::session::{Session, SessionError, SessionGeometry};
use mind2t_vt_render::{CellMetrics, GpuContext};

use crate::layout::{Canvas as Grid, Rect};

/// What a pane is asked to be, before it exists.
///
/// `agent` is carried and not yet used: B4 launches the CLI, and threading the field now means
/// the spec that crosses the IPC does not change shape when it does.
#[derive(Debug, Clone)]
pub struct PaneSpec {
    /// Where the child starts. `None` inherits the host's directory.
    pub cwd: Option<String>,
    /// Which agent CLI to launch in it, or `None` for a plain shell.
    pub agent: Option<String>,
}

impl PaneSpec {
    pub fn shell() -> PaneSpec {
        PaneSpec { cwd: None, agent: None }
    }
}

/// The narrowest pane a SPLIT will create, in columns.
///
/// `fit` refuses only at zero columns, which is the difference between a pane and no pane. It
/// is not the difference between a pane and a usable one, so cmd+D kept succeeding all the way
/// down to a single column: the operator's report was "there is no limit until app crashes".
/// Twenty is where a shell prompt plus a short command still fits on one line.
///
/// This bounds SPLITTING only, never `resize`. A split is the operator asking for a new pane
/// and being told no costs them nothing; a resize is the operator dragging the window, where
/// refusing would leave the window one size and the canvas another. A drag degrades, a split
/// declines.
pub const MIN_SPLIT_COLS: u16 = 20;

/// The shortest pane a split will leave behind. A horizontal split does not change height, so
/// this only fires when the window is already too short to be worth dividing.
pub const MIN_SPLIT_ROWS: u16 = 5;

#[derive(Debug)]
pub enum CanvasError {
    /// The area cannot hold the requested grid at the current font size.
    TooSmall { rows: u16, cols: u16 },
    /// The split would fit, and would leave a pane too small to work in.
    ///
    /// Distinct from `TooSmall` on purpose: that one means the geometry is impossible, this one
    /// means it is possible and refused. The caller can say so differently, and a test can tell
    /// "the limit held" from "the arithmetic collapsed".
    TooNarrow { cols: u16, rows: u16 },
    /// The pane index handed to `close` does not exist.
    NoSuchPane { index: usize, panes: usize },
    Session(SessionError),
    /// A split was asked of a canvas with more than one row.
    ///
    /// Adding a column to a two-row grid adds TWO panes, not one, and moves every existing pane's
    /// row-major index. That is a different operation from what the operator pressed a key for, so
    /// it is refused rather than approximated. The shape that lifts this is a split TREE, where
    /// `tile` becomes the leaf case - see the note on `layout::Canvas`.
    NotSplittable { rows: u16 },
}

/// One pane: a session and the rectangle it occupies, in physical pixels.
pub struct Pane {
    pub session: Session,
    pub rect: Rect,
}

/// A live canvas: the grid, the area it fills, and one session per cell.
pub struct Canvas {
    grid: Grid,
    area: Rect,
    panes: Vec<Pane>,
}

impl Canvas {
    /// Spawns one session per cell on ONE GPU context, each sized from its own rect.
    ///
    /// The context is the caller's and is shared by every pane, because a canvas is composited:
    /// `present_all` puts all of them in one render pass, and a pass can only bind buffers from
    /// its own device. A pane that made its own context would be a wgpu validation failure the
    /// first time two panes were drawn together - and invisible to any test that never presents,
    /// which is how this arrived with real children and a green suite (`Session::spawn_on`).
    ///
    /// Refuses rather than degrades when the area cannot hold the grid: a canvas of one-column
    /// terminals is not a smaller canvas, it is an unusable one, and the operator asked for
    /// something the window cannot give. The caller has a wizard to say that in.
    pub fn spawn(
        gpu: &GpuContext,
        grid: Grid,
        area: Rect,
        specs: &[PaneSpec],
        font_size: f32,
        // The operator's `font-family`, threaded rather than defaulted. It was `None` at both
        // spawn sites until 2026-08-08 (T4), which meant the key existed in config.toml, parsed,
        // validated, reported a loud error for an unresolvable name - and then changed nothing in
        // the window. A setting that is read and discarded is worse than one that is missing.
        font_family: Option<&str>,
        shell: impl Fn(&PaneSpec) -> Command,
    ) -> Result<Canvas, CanvasError> {
        let rects = grid.tile(area);
        let mut panes = Vec::with_capacity(rects.len());

        for (index, rect) in rects.iter().enumerate() {
            let spec = specs.get(index).cloned().unwrap_or_else(PaneSpec::shell);
            // FITTED, never spawned-then-resized. A child that starts at a provisional 80x24
            // and is resized a moment later has already answered `stty size` with the wrong
            // number - measured, and the reason `spawn_fitted` exists. A banner printed at the
            // wrong width is in the scrollback for good.
            let mut session = Session::spawn_fitted_on(
                gpu,
                shell(&spec),
                rect.width,
                rect.height,
                font_size,
                font_family.map(str::to_string),
            )
            .map_err(CanvasError::Session)?;

            // The refusal happens against the SAME arithmetic the pane will use from now on, so
            // "it fits" cannot mean one thing here and another at the next resize.
            if fit(*rect, session.cell_metrics()).cols == 0
                || fit(*rect, session.cell_metrics()).rows == 0
            {
                session.shutdown();
                return Err(CanvasError::TooSmall { rows: grid.rows, cols: grid.cols });
            }
            session.set_mouse_geometry(rect.width, rect.height, 0, 0, 0, 0);

            panes.push(Pane { session, rect: *rect });
        }

        Ok(Canvas { grid, area, panes })
    }

    /// Re-tiles into a new area and resizes every pane's pty from its own new rect.
    ///
    /// Every pane is resized, including ones whose rect did not change - a resize that skipped
    /// "unchanged" panes would be correct only while the tiling is exact, and would fail
    /// silently the first time it is not.
    pub fn resize(&mut self, area: Rect) -> Result<(), CanvasError> {
        self.area = area;
        for (pane, rect) in self.panes.iter_mut().zip(self.grid.tile(area)) {
            pane.rect = rect;
            let geometry = fit(rect, pane.session.cell_metrics());
            if geometry.cols == 0 || geometry.rows == 0 {
                return Err(CanvasError::TooSmall {
                    rows: self.grid.rows,
                    cols: self.grid.cols,
                });
            }
            pane.session.resize(geometry).map_err(CanvasError::Session)?;
            pane.session
                .set_mouse_geometry(rect.width, rect.height, 0, 0, 0, 0);
        }
        Ok(())
    }

    /// Adds a pane to the right of the existing ones and re-tiles - what cmd+D does.
    ///
    /// Returns the new pane's index.
    ///
    /// **Nothing mutates until the new grid is known to fit.** The new pane is spawned first and
    /// measured against the same `fit` every pane uses, and only then are the existing panes moved
    /// and resized. The order matters because the alternative fails halfway: resize everyone,
    /// discover the last cell is a single column wide, and the operator is left with a canvas
    /// smaller than the one they had and no new pane to show for it. A refused split leaves the
    /// canvas exactly as it was.
    ///
    /// Single-row canvases only. Adding a column to a two-row grid adds TWO panes and renumbers
    /// every existing one, which is not what a key press asked for - see `CanvasError`.
    pub fn split(
        &mut self,
        gpu: &GpuContext,
        command: Command,
        font_size: f32,
        font_family: Option<&str>,
    ) -> Result<usize, CanvasError> {
        if self.grid.rows > 1 {
            return Err(CanvasError::NotSplittable { rows: self.grid.rows });
        }

        let grown = Grid { cols: self.grid.cols + 1, ..self.grid };
        let rects = grown.tile(self.area);
        let Some(last) = rects.last().copied() else {
            return Err(CanvasError::TooSmall { rows: grown.rows, cols: grown.cols });
        };

        let mut session =
            Session::spawn_fitted_on(gpu, command, last.width, last.height, font_size, font_family.map(str::to_string))
                .map_err(CanvasError::Session)?;
        let metrics = session.cell_metrics();

        // EVERY new rect is checked, not just the new one: the split takes width from the panes
        // that already exist, so the cell that becomes unusable is as likely to be an old one.
        if rects
            .iter()
            .any(|rect| fit(*rect, metrics).cols == 0 || fit(*rect, metrics).rows == 0)
        {
            session.shutdown();
            return Err(CanvasError::TooSmall { rows: grown.rows, cols: grown.cols });
        }

        // The USABILITY floor, above the possibility floor checked immediately above. Measured
        // against the same `fit`, over the same every-rect set, and evaluated BEFORE anything
        // mutates - so a refused split leaves the canvas exactly as the operator had it, which
        // is the whole reason the new session is spawned and measured before the old panes move.
        if let Some(cramped) = rects
            .iter()
            .map(|rect| fit(*rect, metrics))
            .find(|g| g.cols < MIN_SPLIT_COLS || g.rows < MIN_SPLIT_ROWS)
        {
            session.shutdown();
            return Err(CanvasError::TooNarrow { cols: cramped.cols, rows: cramped.rows });
        }
        session.set_mouse_geometry(last.width, last.height, 0, 0, 0, 0);

        self.grid = grown;
        self.panes.push(Pane { session, rect: last });
        // Re-tiling through `resize` rather than by hand, so a split and a window resize cannot
        // disagree about how a pane is moved: one path updates rects, ptys and mouse geometry.
        self.resize(self.area)?;
        Ok(self.panes.len() - 1)
    }

    /// Closes one pane and gives its width back to the survivors. Returns how many remain.
    ///
    /// This is pane lifecycle, and its absence was a defect the operator hit immediately: typing
    /// `exit` ended the child and the pane stayed on screen holding its last frame, because
    /// nothing owned removal and the host only asked whether ALL panes had exited.
    ///
    /// The re-tile is the half that is easy to omit and is the half that shows. Dropping the pane
    /// without shrinking the grid leaves the survivors at their old width with a dead strip beside
    /// them, which does not read as a closed pane - it reads as a renderer that stopped painting.
    /// It goes through `resize` for the same reason `split` does: one path updates rects, ptys and
    /// mouse geometry, so a close and a window drag cannot disagree about where a pane is.
    ///
    /// The child is shut down rather than dropped, so it reaches its own teardown instead of
    /// being reaped by a destructor at some later point.
    ///
    /// Single-row canvases only, for the same structural reason `split` is: removing a column
    /// from a two-row grid removes TWO panes.
    pub fn close(&mut self, index: usize) -> Result<usize, CanvasError> {
        if index >= self.panes.len() {
            return Err(CanvasError::NoSuchPane { index, panes: self.panes.len() });
        }
        if self.grid.rows > 1 {
            return Err(CanvasError::NotSplittable { rows: self.grid.rows });
        }

        let mut pane = self.panes.remove(index);
        pane.session.shutdown();

        self.grid = Grid { cols: self.grid.cols.saturating_sub(1), ..self.grid };
        if self.panes.is_empty() {
            return Ok(0);
        }
        self.resize(self.area)?;
        Ok(self.panes.len())
    }

    /// The panes whose child has exited, newest index first.
    ///
    /// Reverse order because the caller closes them by index and closing shifts everything after
    /// it down: walking forward would make the second close address the wrong pane. Returning the
    /// order that is safe to consume is cheaper than documenting a trap for every caller.
    pub fn exited_panes(&mut self) -> Vec<usize> {
        let mut done: Vec<usize> = self
            .panes
            .iter_mut()
            .enumerate()
            .filter_map(|(index, pane)| pane.session.exited().then_some(index))
            .collect();
        done.reverse();
        done
    }

    pub fn panes(&self) -> &[Pane] {
        &self.panes
    }

    pub fn panes_mut(&mut self) -> &mut [Pane] {
        &mut self.panes
    }

    pub fn area(&self) -> Rect {
        self.area
    }

    pub fn grid(&self) -> Grid {
        self.grid
    }

    /// The rules between the panes, in the SAME area the panes were tiled into.
    ///
    /// Derived on demand from the grid rather than stored, because a stored copy is a second
    /// truth that survives a resize: the rects would move and the dividers would stay, leaving
    /// rules floating across the middle of a pane. `resize` updates `area`, and this follows for
    /// free - the same reason `pane_at` recomputes instead of caching.
    pub fn dividers(&self) -> Vec<Rect> {
        self.grid.dividers(self.area)
    }

    /// The pane under a point in window space, or `None` outside every pane.
    ///
    /// Hit testing belongs here rather than in the host, because it must use the same rects the
    /// blit used. A host that recomputed them would drift from what is on screen the first time
    /// the two roundings disagree.
    pub fn pane_at(&self, x: u32, y: u32) -> Option<usize> {
        self.panes.iter().position(|pane| {
            x >= pane.rect.x
                && x < pane.rect.x + pane.rect.width
                && y >= pane.rect.y
                && y < pane.rect.y + pane.rect.height
        })
    }

    /// Ends every child cleanly. Called before the process exits, never left to a kill.
    pub fn shutdown(&mut self) {
        for pane in &mut self.panes {
            pane.session.shutdown();
        }
    }
}

/// The grid that fits a rect. Floors: a partial row is not a row, and a pty told it has a row it
/// cannot fully draw is how a program's last line lands under the pane below.
fn fit(rect: Rect, cell: CellMetrics) -> SessionGeometry {
    SessionGeometry {
        cols: (rect.width / cell.width.max(1)) as u16,
        rows: (rect.height / cell.height.max(1)) as u16,
    }
}
