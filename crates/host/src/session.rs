//! Purpose: the RUST-native composition of pty -> core -> frame -> renderer -> window, for
//!   embedders that live in this workspace.
//! Public surface: `Session`, `SessionGeometry`, `SessionError`.
//! Why this file: `lib.rs` composes the same pieces behind the C ABI, and that surface exists
//!   for the Swift host and for outside embedders. Mind2t is neither - it is a Rust program in
//!   this workspace, and crossing a foreign-function boundary to reach its own crates would buy
//!   nothing and cost the type system. So the composition is offered twice, once per audience.
//! NOT responsible for: policy. It does not decide which shell to run, what the font is called
//!   or how big the window should be - the caller builds the `Command` and states the geometry.
//!   Also not responsible yet for kitty image placements, unicode placeholders or row
//!   semantics; the C path draws those and this one does not. **That gap is deliberate and
//!   recorded**: it is the convergence debt of having two compositions, and it closes when the
//!   Swift host retires (B7) and `lib.rs` delegates here rather than duplicating.
//! Test strategy: `tests/session.rs` spawns a real child through this type and asserts the
//!   rendered pixels change once the child has written - with the control that makes the
//!   assertion falsifiable, a session polled before any output must NOT satisfy it.

use std::process::Command;

use crate::pointer::{Input, Pointer, Wheel};
use mind2t_vt_core::events::Event;
use mind2t_vt_core::selection;
use mind2t_vt_frame::{BaseDirection, Frame, FrameReader, FrameSelection};
use mind2t_vt_pty::key::{KeyOptions, OptionAsAlt};
// Re-exported under host-facing names: a caller wiring a window should not have to learn that
// the mouse encoder lives in the pty crate to name a click.
pub use mind2t_vt_pty::mouse::{Action as MouseAction, Mods as MouseMods};
use mind2t_vt_pty::{Geometry, Host, Options, SpawnError};
use mind2t_vt_render::{
    CellMetrics, FontStack, GpuContext, GpuSurface, Palette, PresentError, Renderer, WindowTarget,
};

/// Smallest font the size chords will go to. Below this the cell is a couple of pixels wide and
/// the grid becomes large enough to be slow while being unreadable, which is not a state worth
/// being able to reach by holding a key down.
pub const MIN_FONT_SIZE: f32 = 6.0;

/// Largest font the size chords will go to. The ceiling exists because the grid floors at one
/// cell: past the point where a single glyph fills the pane, every further step tells the child
/// the same 1x1 grid while doing a full renderer rebuild each time.
pub const MAX_FONT_SIZE: f32 = 72.0;

/// A core selection range in the frame's own coordinates.
///
/// Total, with no fallible branch, precisely because the rows that produced the range came from
/// this frame: the conversion is an unwrap of two `Point`s, and the seam that used to be
/// dangerous - absolute scrollback rows against viewport rows - no longer exists on this path
/// because the probe was fed viewport rows to begin with.
fn into_frame_selection(found: &mind2t_vt_snapshot::Selection) -> FrameSelection {
    FrameSelection {
        start: (found.start.x, found.start.y),
        end: (found.end.x, found.end.y),
    }
}

/// The grid, in cells. Pixels are derived from it and the font, never the other way round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionGeometry {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug)]
pub enum SessionError {
    /// The child could not be started.
    Spawn(SpawnError),
    /// No GPU, no font, or a surface that could not be built.
    Render(String),
    /// The swapchain refused a frame. Never silently swallowed: a present that fails and falls
    /// back to another path is how B1's defects stayed invisible.
    Present(PresentError),
    /// Writing to the child failed.
    Send(std::io::Error),
    /// The requested grid does not fit the frame channel's fixed capacity.
    Resize(String),
    /// An operation that needs a window was called before one was attached.
    NoWindow,
}

/// One terminal: a child on a pty, a core parsing its bytes, a renderer drawing the grid, and
/// optionally a window the result is presented into.
pub struct Session {
    host: Host,
    reader: FrameReader,
    renderer: Renderer<GpuSurface>,
    frame: Frame,
    /// One GPU context for the session's whole life. Every renderer rebuild lands on the SAME
    /// device, because a rebuild on a fresh device orphans an attached window's swapchain and
    /// the frame simply stops following the window - no error, no log (measured 2026-08-04).
    gpu: GpuContext,
    window: Option<WindowTarget>,
    drawn_generation: u64,
    geometry: SessionGeometry,
    font_size: f32,
    font_family: Option<String>,
    /// The theme, HELD rather than only applied.
    ///
    /// `resize` rebuilds the renderer, and a fresh renderer starts on `Palette::xterm()`. A
    /// palette that was only pushed into the renderer would therefore survive until the first
    /// time the operator dragged the window, then silently revert - the terminal would simply be
    /// wearing different colours than it was a moment ago, with nothing in any log.
    palette: Palette,
    /// Mirrors `Renderer::ligatures` so a renderer rebuilt by `set_font_size` can be told again.
    ligatures: bool,
    /// The child's working directory, decoded from its last OSC 7 report. `None` until it
    /// reports one, and again after it reports an empty one.
    cwd: Option<String>,
    /// Mouse-reporting state, the same type the C surface carries. One policy, two callers.
    pointer: Pointer,
}

impl Session {
    /// Starts `command` on a pty and builds the pipeline around it.
    ///
    /// The renderer is built BEFORE the child, so a machine that cannot render never spawns a
    /// process it would immediately have to reap.
    pub fn spawn(
        command: Command,
        geometry: SessionGeometry,
        font_size: f32,
        font_family: Option<String>,
    ) -> Result<Session, SessionError> {
        let gpu = GpuContext::new().map_err(|error| SessionError::Render(error.to_string()))?;
        Session::spawn_on(&gpu, command, geometry, font_size, font_family)
    }

    /// The same, on a context the CALLER owns - which is what a canvas requires.
    ///
    /// A composited frame is one render pass, and a bind group can only carry buffers belonging
    /// to the device that pass runs on. So N panes on N devices is not a slow path, it is a wgpu
    /// validation failure at the first frame that draws more than one of them
    /// ([`mind2t_vt_render::WindowTarget::present_all`]).
    ///
    /// It cannot be caught by any test that never presents, which is exactly what happened: the
    /// canvas landed with real children, correct geometry and a green suite, and could not have
    /// drawn itself. The window's swapchain has the same requirement from the other side - it is
    /// built on a context too, and `is_surface_supported` only answers for the one it was given.
    pub fn spawn_on(
        gpu: &GpuContext,
        command: Command,
        geometry: SessionGeometry,
        font_size: f32,
        font_family: Option<String>,
    ) -> Result<Session, SessionError> {
        let gpu = gpu.clone();
        let renderer = build(&gpu, geometry, font_size, font_family.as_deref())?;
        let (host, reader) = Host::spawn(command, Options::new(geometry.cols, geometry.rows))
            .map_err(SessionError::Spawn)?;

        Ok(Session {
            host,
            reader,
            renderer,
            frame: Frame::default(),
            gpu,
            window: None,
            drawn_generation: 0,
            geometry,
            font_size,
            font_family,
            palette: Palette::xterm(),
            ligatures: true,
            cwd: None,
            pointer: Pointer::default(),
        })
    }

    /// Starts a session on `gpu`, sized to fit `width` x `height` PHYSICAL pixels, and tells the
    /// child that size from its very first breath.
    ///
    /// The difference from [`Session::spawn`] plus a resize is a race, and it is not theoretical:
    /// measured 2026-08-05, a pane spawned at the provisional 80x24 and resized a moment later
    /// had already answered `stty size` with **80x24** - the child ran before `TIOCSWINSZ`
    /// arrived. A shell shrugs at that. An agent CLI that prints a banner, a table or a progress
    /// bar sized on startup does not, and the wrongness is baked into scrollback where no later
    /// resize can fix it.
    ///
    /// The font is measured first (cell metrics come from the font, not from the grid), so the
    /// geometry is known before there is a child to tell.
    ///
    /// There is deliberately no context-less twin: every caller of this is a pane, and a pane
    /// that owns its own device cannot be composited (see [`Session::spawn_on`]).
    pub fn spawn_fitted_on(
        gpu: &GpuContext,
        command: Command,
        width: u32,
        height: u32,
        font_size: f32,
        font_family: Option<String>,
    ) -> Result<Session, SessionError> {
        let fonts = FontStack::with_primary(font_family.as_deref(), font_size)
            .map_err(|error| SessionError::Render(error.to_string()))?;
        let cell = fonts.metrics();
        let geometry = SessionGeometry {
            // Floors, then a floor of one: a zero-column pty is refused by the kernel, and a
            // caller who wanted to know the area was too small asked before spawning anything.
            cols: (width / cell.width.max(1)).max(1) as u16,
            rows: (height / cell.height.max(1)).max(1) as u16,
        };
        Session::spawn_on(gpu, command, geometry, font_size, font_family)
    }

    /// The GPU context a window target must be built on.
    ///
    /// Exposed rather than hidden because the caller owns the window: it builds the
    /// `WindowTarget` from its own window handle and hands it back to [`Session::attach`]. The
    /// context has to match, or the swapchain belongs to a different device than the frame.
    pub fn context(&self) -> &GpuContext {
        &self.gpu
    }

    pub fn cell_metrics(&self) -> CellMetrics {
        self.renderer.cell_metrics()
    }

    pub fn geometry(&self) -> SessionGeometry {
        self.geometry
    }

    /// Presents into `window` from now on. The next poll repaints in full: the window has
    /// nothing in it yet.
    pub fn attach(&mut self, window: WindowTarget) {
        self.window = Some(window);
        self.drawn_generation = 0;
    }

    /// The last frame read, for a caller that needs terminal STATE rather than pixels.
    ///
    /// Read-only on purpose: everything a host asks of it - which modes are on, how big the
    /// grid is, what a row says - is a question, and the answers all come from the pump's
    /// published frame rather than from anything the host is allowed to change.
    pub fn frame(&self) -> &Frame {
        &self.frame
    }

    /// What the visible grid says, rows joined by newlines.
    ///
    /// The Rust twin of `mind2t_host_row_text`, and the same idea the product is built on: agent
    /// state comes from a TYPED GRID, not from regexing ANSI out of a byte stream. A wide cell's
    /// spacer tail contributes nothing (its glyph already came from the head), while a cell with
    /// no text is a space - so column positions in the result line up with columns on screen.
    ///
    /// Reads the last POLLED frame, exactly like every other reader here: a caller that has not
    /// polled gets what it last saw, not a fresh scrape of the child.
    pub fn visible_text(&self) -> String {
        let mut scratch = [0u8; mind2t_vt_frame::CLUSTER_BYTES];
        let mut out = String::new();
        for y in 0..self.frame.rows {
            for x in 0..self.frame.cols {
                let cell = self.frame.cell(x, y);
                if cell.has_text() {
                    out.push_str(cell.cluster(&mut scratch));
                } else if cell.wide() != mind2t_vt_snapshot::Wide::SpacerTail {
                    out.push(' ');
                }
            }
            out.push('\n');
        }
        out
    }

    /// The key encoder's options, derived from the modes the CHILD has actually entered.
    ///
    /// This is the difference between arrows that work inside `vim` and arrows that do not.
    /// DECCKM, DECKPAM, 1035, 1036, xterm modifyOtherKeys and the kitty flags are all terminal
    /// state, so a host that hardcodes the defaults encodes correctly right up until a program
    /// asks for something else - and then sends `ESC [ A` where `ESC O A` was expected, which
    /// looks like a broken key rather than a wrong mode.
    ///
    /// The one field NOT derived is `macos_option_as_alt`: that is operator policy, not terminal
    /// state, so it stays at the caller's default and the caller overrides it.
    pub fn key_options(&self) -> KeyOptions {
        KeyOptions {
            cursor_key_application: self.frame.cursor_keys(),
            keypad_key_application: self.frame.keypad_keys(),
            ignore_keypad_with_numlock: self.frame.ignore_keypad_with_numlock(),
            alt_esc_prefix: self.frame.alt_esc_prefix(),
            modify_other_keys_state_2: self.frame.modify_other_keys_2(),
            kitty_flags: self.frame.kitty_key_flags(),
            macos_option_as_alt: OptionAsAlt::False,
            backarrow_key_mode: false,
        }
    }

    /// Whether the child asked for bracketed paste (DEC 2004).
    ///
    /// Pasting without the fences into a shell that wanted them is how a multi-line paste
    /// executes itself line by line instead of arriving as one edit.
    pub fn bracketed_paste(&self) -> bool {
        self.frame.bracketed_paste()
    }

    /// Drains the events the child asked its embedder to act on, folding the ones this type
    /// tracks and handing the rest to the caller untouched.
    ///
    /// Draining is the caller's job every frame, not an option: the core's queue is bounded at
    /// 128 and drops its OLDEST entry on overflow, so a host that never drains does not fail -
    /// it silently loses the first events of the session and keeps the last ones forever.
    ///
    /// `Event::Pwd` is decoded HERE, through [`crate::cwd::normalize`], because that is the one
    /// decoder in this workspace. The core stores the report raw (as the oracle does), the C
    /// path decodes it in `mind2t_cwd_path`, and a Rust host that percent-decoded a `file://`
    /// URI for itself would be the second implementation of a rule that has already been wrong
    /// once (`path` is a special variable in zsh, tied to $PATH as an array).
    pub fn take_events(&mut self) -> Vec<Event> {
        let events = self.host.take_events();
        for event in &events {
            if let Event::Pwd(raw) = event {
                // An empty report CLEARS, and `normalize` answers `None` for it - so this
                // assignment is the clear as well as the set, with no branch to forget.
                self.cwd = crate::cwd::normalize(raw);
            }
        }
        events
    }

    /// The child's working directory, decoded, or `None` if it has never reported one.
    ///
    /// Only ever fresh as of the last [`Session::take_events`]: this reads state, it does not
    /// drain the queue, because a getter with a side effect on a queue is how one caller starves
    /// another.
    pub fn cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }

    /// Places the terminal's top-left inside the window, in PHYSICAL pixels.
    ///
    /// A host with chrome above the terminal sets it so the strip is RESERVED rather than
    /// covered. No-op without a window, because the origin is a property of presenting.
    pub fn set_origin(&mut self, x: u32, y: u32) {
        if let Some(window) = self.window.as_mut() {
            window.set_origin(x, y);
        }
    }

    /// Where the terminal's top-left currently sits in the window, or `None` with no window.
    ///
    /// A reader exists because the setter is the kind of call that can silently not happen -
    /// wrong order, wrong branch, a window attached after it - and the symptom (chrome drawn
    /// over the terminal's first rows) looks like a layout choice rather than a bug.
    pub fn origin(&self) -> Option<(u32, u32)> {
        self.window.as_ref().map(|window| window.origin())
    }

    /// Reads the newest published frame and draws it. Returns whether anything was drawn.
    ///
    /// A frame no newer than the one already drawn is not redrawn - the seqlock hands back
    /// whatever is current, and a quiet terminal publishes nothing new.
    pub fn poll(&mut self) -> bool {
        self.reader.read_into(&mut self.frame);
        if !self.frame.is_valid() || self.frame.generation <= self.drawn_generation {
            return false;
        }
        // The very first paint covers every row: rows the child never touched carry no damage
        // stamp, and only `draw_all` gives them their background.
        if self.drawn_generation == 0 {
            self.renderer.draw_all(&self.frame);
        } else {
            self.renderer.draw(&self.frame);
        }
        self.drawn_generation = self.frame.generation;
        true
    }

    /// The drawn pixels, for a caller with no window - tests, and headless assertions.
    ///
    /// A full-frame readback, so it is the expensive path by construction; presenting into a
    /// window never touches it.
    pub fn pixels(&mut self) -> Vec<u8> {
        self.renderer.pixels()
    }

    /// Puts the drawn frame on screen.
    ///
    /// Separate from [`Session::poll`] on purpose, exactly as the C surface separates them:
    /// polling advances the terminal, presenting puts a frame on screen, and the two run at
    /// different rates - a resize presents without polling, a quiet terminal polls without
    /// needing to present.
    /// The colour the margin around this session's grid should be cleared to.
    ///
    /// The frame's own top-left style, falling back to the palette default. A grid rounds to
    /// whole cells, so a window is almost never an exact multiple of the surface and the
    /// remainder is visible - clearing it to black while the terminal renders on its own
    /// background draws a hard band down the edge (operator-spotted 2026-08-04).
    ///
    /// Public because a canvas clears ONCE for the whole window and then blits N panes into it,
    /// so the colour has to be askable from outside the session that owns it.
    pub fn clear_color(&self) -> mind2t_vt_render::Rgba {
        let palette = self.renderer.palette();
        if self.frame.is_valid() {
            let style = self.frame.style(self.frame.cell(0, 0).style_id());
            palette.draw(&style).background
        } else {
            palette.default_background
        }
    }

    pub fn present(&mut self) -> Result<(), SessionError> {
        let clear = self.clear_color();
        let renderer = &mut self.renderer;
        let window = self.window.as_mut().ok_or(SessionError::NoWindow)?;
        window
            .present(renderer.surface_mut(), clear)
            .map_err(SessionError::Present)
    }

    /// The drawn surface, for a host that composites SEVERAL sessions into one frame.
    ///
    /// The single-pane path ([`Session::attach`] plus [`Session::present`]) has the session own
    /// its window and present itself, which is right when it is the only thing on screen. A
    /// canvas cannot work that way: one swapchain frame must hold every pane, so the window
    /// belongs to the host and the sessions hand it their surfaces
    /// (`WindowTarget::present_all`). Both paths exist on purpose - the first is what the Swift
    /// host and the oracle still use.
    pub fn surface_mut(&mut self) -> &mut GpuSurface {
        self.renderer.surface_mut()
    }

    /// Reconfigures the swapchain for a new WINDOW size, in physical pixels.
    pub fn resize_window(&mut self, width: u32, height: u32) -> Result<(), SessionError> {
        self.window
            .as_mut()
            .ok_or(SessionError::NoWindow)?
            .resize(width, height);
        Ok(())
    }

    /// Resizes the grid: the pty first, then the renderer.
    ///
    /// The pty leads because the child reacts to `SIGWINCH` by redrawing, and a renderer still
    /// sized for the old grid would draw that redraw clipped.
    pub fn resize(&mut self, geometry: SessionGeometry) -> Result<(), SessionError> {
        if geometry == self.geometry || geometry.cols == 0 || geometry.rows == 0 {
            return Ok(());
        }
        self.host
            .resize(Geometry {
                cols: geometry.cols,
                rows: geometry.rows,
            })
            .map_err(|error| SessionError::Resize(format!("{error:?}")))?;

        self.renderer = build(
            &self.gpu,
            geometry,
            self.font_size,
            self.font_family.as_deref(),
        )?;
        // The rebuild starts on the default scheme, so the theme is put back. Without this a
        // configured palette lasts exactly until the first window drag.
        self.renderer.set_palette(self.palette.clone());
        self.geometry = geometry;
        // Everything on screen belongs to the old grid; the next frame repaints in full.
        self.drawn_generation = 0;
        Ok(())
    }

    /// Applies a theme, and keeps it across every later renderer rebuild.
    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette.clone();
        self.renderer.set_palette(palette);
    }

    /// Whether same-style ASCII segments may form ligatures (config `font-ligatures`).
    ///
    /// STORED as well as forwarded, for the same reason the palette is: `set_font_size` builds a
    /// fresh renderer, and a fresh renderer starts on the default. Without the field the
    /// operator's setting would last exactly until the first zoom chord - which is a defect that
    /// only appears after an unrelated action and so never gets attributed to this call.
    pub fn set_ligatures(&mut self, on: bool) {
        self.ligatures = on;
        self.renderer.set_ligatures(on);
    }

    /// How rows are laid out - `Auto` is the Hebrew-first setting the `.app` ships.
    ///
    /// Set once and it holds: the publish channel does not carry a direction, and neither
    /// `read_into` nor `resize` writes this field, so a per-frame reapplication would be
    /// redundant rather than defensive (the C surface relies on the same property).
    pub fn set_base_direction(&mut self, direction: BaseDirection) {
        self.frame.base_direction = direction;
    }

    pub fn send(&self, bytes: &[u8]) -> Result<(), SessionError> {
        self.host
            .send(bytes)
            .map_err(|errno| SessionError::Send(std::io::Error::from(errno)))
    }

    pub fn scroll(&self, rows: i32) {
        self.host.scroll(rows);
    }

    /// Moves the view to the nearest OSC 133 prompt mark: `back` climbs into history to the
    /// previous command, otherwise it returns toward the newest.
    ///
    /// Needs shell integration to do anything - the marks come from the shell emitting OSC 133,
    /// and a shell that emits none leaves the history unmarked. That is a silent no-op by
    /// design and NOT a failure to report: a terminal cannot tell "no marks yet" from "no
    /// integration", and popping an error on a key press for the first case would be wrong.
    /// The honest tell is that the view does not move.
    pub fn jump_to_prompt(&self, back: bool) {
        self.host.jump_to_prompt(back);
    }

    /// The view pointer positions are measured in: surface size and the insets around the grid,
    /// in PHYSICAL pixels - the same space the frame's pixels use.
    ///
    /// Set it at launch and after every resize. Until it is set, every pointer event encodes to
    /// nothing, which is correct rather than a failure: a report is a grid position, and there
    /// is no grid to position against.
    pub fn set_mouse_geometry(
        &mut self,
        screen_width: u32,
        screen_height: u32,
        padding_left: u32,
        padding_top: u32,
        padding_right: u32,
        padding_bottom: u32,
    ) {
        self.pointer.set_geometry(
            screen_width,
            screen_height,
            padding_left,
            padding_top,
            padding_right,
            padding_bottom,
        );
    }

    /// Feeds one pointer event to the child. `Ok(false)` means the protocol produced nothing.
    ///
    /// `code` is the protocol's button number: 0 motion with nothing held, 1 left, 2 middle, 3
    /// right, 4..9 wheel and aux. Every press and release must reach this call EVEN WHILE
    /// REPORTING IS OFF - the held-button bookkeeping happens here, and a release the host
    /// swallowed leaves a button held forever, which the next drag reports as a phantom.
    ///
    /// `Ok(false)` hands the event back: it is the host's again for selection or a context menu.
    pub fn mouse(
        &mut self,
        action: MouseAction,
        code: u32,
        mods: MouseMods,
        x: f32,
        y: f32,
    ) -> Result<bool, SessionError> {
        let cell = self.renderer.cell_metrics();
        let Some(bytes) = self
            .pointer
            .button(&self.frame, cell, Input { action, code, mods, x, y })
        else {
            return Ok(false);
        };
        self.send(&bytes)?;
        Ok(true)
    }

    /// Routes a wheel tick. `Ok(false)` means the child wanted neither a report nor alternate
    /// scroll, so the wheel is the HOST's: scroll the viewport with [`Session::scroll`].
    ///
    /// Never both. A program that captured the mouse must not also have the view scrolled under
    /// it, which is why this returns a decision rather than doing half of it here and leaving
    /// the other half to a caller that might also act.
    pub fn wheel(
        &mut self,
        x: f32,
        y: f32,
        ticks: i32,
        mods: MouseMods,
    ) -> Result<bool, SessionError> {
        let cell = self.renderer.cell_metrics();
        match self.pointer.wheel(&self.frame, cell, x, y, ticks, mods) {
            Wheel::Send(bytes) => {
                if !bytes.is_empty() {
                    self.send(&bytes)?;
                }
                Ok(true)
            }
            Wheel::Viewport => Ok(false),
        }
    }

    /// Which viewport cell a surface pixel lands on, or `None` before mouse geometry is set.
    ///
    /// The same arithmetic the mouse encoder uses, so a highlight and a mouse report can never
    /// name different cells for one pointer position.
    pub fn cell_at(&self, x: f32, y: f32) -> Option<(u16, u16)> {
        self.pointer.cell_at(self.renderer.cell_metrics(), x, y)
    }

    /// The highlighted range, viewport-relative.
    pub fn selection(&self) -> Option<FrameSelection> {
        self.frame.selection
    }

    /// Sets or clears the highlight, and makes the next poll repaint.
    ///
    /// The repaint is NOT incidental. A selection changes no terminal state, so the frame's
    /// generation does not move, so `poll` would return `false` and the highlight would appear
    /// only the next time the child happened to write something - a selection that shows up
    /// when you press a key. Resetting `drawn_generation` forces the full-frame path, which is
    /// also what ERASING the previous highlight needs: a partial repaint touches only rows the
    /// child dirtied, and the row the operator just deselected is not one of them.
    pub fn set_selection(&mut self, selection: Option<FrameSelection>) {
        if self.frame.selection == selection {
            return;
        }
        self.frame.selection = selection;
        self.drawn_generation = 0;
    }

    /// Extends a drag: a selection anchored at `anchor`, held at the cell under the pointer.
    ///
    /// Endpoint order is the GESTURE's, not reading order - a drag upward leaves `start` after
    /// `end` - because that is what tells the host which end the pointer holds. Every reader
    /// goes through `FrameSelection::ordered`.
    pub fn select_to(&mut self, anchor: (u16, u16), x: f32, y: f32) -> bool {
        let Some(head) = self.cell_at(x, y) else {
            return false;
        };
        self.set_selection(Some(FrameSelection { start: anchor, end: head }));
        true
    }

    /// Selects the word under a pixel position. `false` means there is nothing selectable there,
    /// which is a real answer: a double-click on a blank cell selects nothing.
    pub fn select_word_at(&mut self, x: f32, y: f32) -> bool {
        self.select_with(x, y, selection::select_word)
    }

    /// Selects the line under a pixel position, trailing whitespace trimmed by the same rule the
    /// oracle uses.
    pub fn select_line_at(&mut self, x: f32, y: f32) -> bool {
        self.select_with(x, y, selection::select_line)
    }

    /// Selects the whole visible grid.
    ///
    /// Visible, not the whole scrollback: a frame is a viewport and history is not in it. Said
    /// out loud because `Terminal::select`'s `All` DOES reach into history, and the two answering
    /// differently is a difference a person can see.
    pub fn select_all_visible(&mut self) -> bool {
        let rows = self.frame.viewport_rows();
        match selection::select_all(&rows, self.frame.cols) {
            Some(found) => {
                self.set_selection(Some(into_frame_selection(&found)));
                true
            }
            None => false,
        }
    }

    /// The clipboard text for the current highlight, or `None` when nothing is selected.
    ///
    /// Formatted by `mind2t_vt_core::selection::format`, which is the function the differential
    /// corpus measures against the oracle - trailing whitespace, soft-wrap joining and all. A
    /// host that walked the grid itself would produce text that looks right and pastes wrong.
    pub fn selection_text(&self) -> Option<String> {
        let selection = self.frame.selection?;
        let rows = self.frame.viewport_rows();
        let ((sx, sy), (ex, ey)) = selection.ordered();
        Some(selection::format(
            &rows,
            self.frame.cols,
            &mind2t_vt_snapshot::Selection {
                start: mind2t_vt_snapshot::Point { x: sx, y: sy },
                end: mind2t_vt_snapshot::Point { x: ex, y: ey },
                rectangle: false,
            },
        ))
    }

    /// Shared body of the word and line probes: viewport rows in, frame-space highlight out.
    fn select_with(
        &mut self,
        x: f32,
        y: f32,
        probe: fn(
            &[mind2t_vt_snapshot::Row],
            u16,
            mind2t_vt_snapshot::Point,
        ) -> Option<mind2t_vt_snapshot::Selection>,
    ) -> bool {
        let Some((col, row)) = self.cell_at(x, y) else {
            return false;
        };
        let rows = self.frame.viewport_rows();
        match probe(&rows, self.frame.cols, mind2t_vt_snapshot::Point { x: col, y: row }) {
            Some(found) => {
                self.set_selection(Some(into_frame_selection(&found)));
                true
            }
            None => false,
        }
    }

    /// The OSC 8 link under a pixel position, if the child published one there.
    pub fn link_at(&self, x: f32, y: f32) -> Option<String> {
        let (col, row) = self.cell_at(x, y)?;
        self.frame.link(col, row).map(str::to_string)
    }

    /// Rebuilds at a new font size inside the SAME pixel area, re-deriving the grid from it.
    ///
    /// `width`/`height` are the pane's physical pixels and come from the caller because the
    /// caller is what tiles them. Deriving the area from the current grid instead would floor it
    /// against the old cell size, so every font-size step would shed up to one cell of width and
    /// the terminal would creep smaller each time the chord was pressed.
    ///
    /// The grid is DERIVED, never carried: larger glyphs in one window mean fewer columns, and a
    /// host that kept `cols` would draw wider than the window and tell the child a size it cannot
    /// see. `Ok(None)` means the size was already in force.
    pub fn set_font_size(
        &mut self,
        size: f32,
        width: u32,
        height: u32,
    ) -> Result<Option<SessionGeometry>, SessionError> {
        let size = size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        if (size - self.font_size).abs() < f32::EPSILON {
            return Ok(None);
        }
        let fonts = FontStack::with_primary(self.font_family.as_deref(), size)
            .map_err(|error| SessionError::Render(error.to_string()))?;
        let cell = fonts.metrics();
        let geometry = SessionGeometry {
            cols: (width / cell.width.max(1)).max(1) as u16,
            rows: (height / cell.height.max(1)).max(1) as u16,
        };

        // The pty leads, the same order `resize` uses and for the same reason: the child redraws
        // on SIGWINCH, and a renderer still sized for the old grid draws that redraw clipped.
        self.host
            .resize(Geometry { cols: geometry.cols, rows: geometry.rows })
            .map_err(|error| SessionError::Resize(format!("{error:?}")))?;
        self.font_size = size;
        self.renderer = build(&self.gpu, geometry, size, self.font_family.as_deref())?;
        // A fresh renderer starts on the default scheme; without this the theme lasts exactly
        // until the first font-size chord. Ligatures are the same story and the same fix.
        self.renderer.set_palette(self.palette.clone());
        self.renderer.set_ligatures(self.ligatures);
        self.geometry = geometry;
        // The highlight is in CELL coordinates and the cells just changed size underneath it, so
        // it now names different text than the operator selected. Clearing also marks the frame
        // for a full repaint, which the new grid needs anyway.
        self.frame.selection = None;
        self.drawn_generation = 0;
        Ok(Some(geometry))
    }

    /// The current font size in points.
    pub fn font_size(&self) -> f32 {
        self.font_size
    }

    /// Ends the child and its pump, cleanly.
    ///
    /// Exists because a host that needs a specific EXIT CODE cannot use the runtime's own exit -
    /// and leaving through `process::exit` would skip every destructor, including the one that
    /// reaps the child on its pty (SCAR-016: a kill signal skips cleanup). Calling this first
    /// makes the abrupt exit safe.
    pub fn shutdown(&mut self) {
        self.host.shutdown();
    }

    /// Whether the child has exited. Checked by the caller's event loop, not by a thread.
    pub fn exited(&mut self) -> bool {
        matches!(self.host.try_wait(), Ok(Some(_)))
    }
}

fn build(
    gpu: &GpuContext,
    geometry: SessionGeometry,
    font_size: f32,
    family: Option<&str>,
) -> Result<Renderer<GpuSurface>, SessionError> {
    let fonts = FontStack::with_primary(family, font_size)
        .map_err(|error| SessionError::Render(error.to_string()))?;
    let cell = fonts.metrics();
    let surface = GpuSurface::with_context(
        gpu.clone(),
        cell.width * u32::from(geometry.cols),
        cell.height * u32::from(geometry.rows),
    )
    .map_err(|error| SessionError::Render(error.to_string()))?;
    Ok(Renderer::<GpuSurface>::from_surface(
        fonts,
        surface,
        geometry.cols,
        geometry.rows,
    ))
}
