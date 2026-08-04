//! Purpose: the RUST-native composition of pty -> core -> frame -> renderer -> window, for
//!   embedders that live in this workspace.
//! Public surface: `Session`, `SessionGeometry`, `SessionError`.
//! Why this file: `lib.rs` composes the same pieces behind the C ABI, and that surface exists
//!   for the Swift host and for outside embedders. Bindary is neither - it is a Rust program in
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

use ruuah_vt_core::events::Event;
use ruuah_vt_frame::{Frame, FrameReader};
use ruuah_vt_pty::key::{KeyOptions, OptionAsAlt};
use ruuah_vt_pty::{Geometry, Host, Options, SpawnError};
use ruuah_vt_render::{
    CellMetrics, FontStack, GpuContext, GpuSurface, PresentError, Renderer, WindowTarget,
};

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
    /// The child's working directory, decoded from its last OSC 7 report. `None` until it
    /// reports one, and again after it reports an empty one.
    cwd: Option<String>,
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
            cwd: None,
        })
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
    /// The Rust twin of `ruuah_host_row_text`, and the same idea the product is built on: agent
    /// state comes from a TYPED GRID, not from regexing ANSI out of a byte stream. A wide cell's
    /// spacer tail contributes nothing (its glyph already came from the head), while a cell with
    /// no text is a space - so column positions in the result line up with columns on screen.
    ///
    /// Reads the last POLLED frame, exactly like every other reader here: a caller that has not
    /// polled gets what it last saw, not a fresh scrape of the child.
    pub fn visible_text(&self) -> String {
        let mut scratch = [0u8; ruuah_vt_frame::CLUSTER_BYTES];
        let mut out = String::new();
        for y in 0..self.frame.rows {
            for x in 0..self.frame.cols {
                let cell = self.frame.cell(x, y);
                if cell.has_text() {
                    out.push_str(cell.cluster(&mut scratch));
                } else if cell.wide() != ruuah_vt_snapshot::Wide::SpacerTail {
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
    /// path decodes it in `ruuah_cwd_path`, and a Rust host that percent-decoded a `file://`
    /// URI for itself would be the second implementation of a rule that has already been wrong
    /// once (`path` is a special variable in zsh - see the project CLAUDE.md).
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
    pub fn present(&mut self) -> Result<(), SessionError> {
        // The margin colour comes from the frame's own top-left style, falling back to the
        // palette default. A grid rounds to whole cells, so a window is almost never an exact
        // multiple of the surface and the remainder is visible - clearing it to black while the
        // terminal renders on its own background draws a hard band down the edge.
        let clear = {
            let palette = self.renderer.palette();
            if self.frame.is_valid() {
                let style = self.frame.style(self.frame.cell(0, 0).style_id());
                palette.draw(&style).background
            } else {
                palette.default_background
            }
        };
        let renderer = &mut self.renderer;
        let window = self.window.as_mut().ok_or(SessionError::NoWindow)?;
        window
            .present(renderer.surface_mut(), clear)
            .map_err(SessionError::Present)
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
        self.geometry = geometry;
        // Everything on screen belongs to the old grid; the next frame repaints in full.
        self.drawn_generation = 0;
        Ok(())
    }

    pub fn send(&self, bytes: &[u8]) -> Result<(), SessionError> {
        self.host
            .send(bytes)
            .map_err(|errno| SessionError::Send(std::io::Error::from(errno)))
    }

    pub fn scroll(&self, rows: i32) {
        self.host.scroll(rows);
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
