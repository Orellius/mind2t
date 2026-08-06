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

use ruuah_vt_host::session::{Session, SessionError, SessionGeometry};
use ruuah_vt_render::{CellMetrics, GpuContext};

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

#[derive(Debug)]
pub enum CanvasError {
    /// The area cannot hold the requested grid at the current font size.
    TooSmall { rows: u16, cols: u16 },
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
                None,
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
            Session::spawn_fitted_on(gpu, command, last.width, last.height, font_size, None)
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
        session.set_mouse_geometry(last.width, last.height, 0, 0, 0, 0);

        self.grid = grown;
        self.panes.push(Pane { session, rect: last });
        // Re-tiling through `resize` rather than by hand, so a split and a window resize cannot
        // disagree about how a pane is moved: one path updates rects, ptys and mouse geometry.
        self.resize(self.area)?;
        Ok(self.panes.len() - 1)
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
