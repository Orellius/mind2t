//! Purpose: drive two screen buffers from a byte stream, using `vte` as the parser.
//! Public surface: `Terminal::new`, `Terminal::write`, `Terminal::resize`,
//!   `Terminal::snapshot`.
//! Why this file: the plan is explicit that VT parsing is solved and must not be rewritten,
//!   so this is only the `Perform` side -- what each parsed action does. It is the
//!   imperative shell over a functional core: no I/O, no clock, fully deterministic.
//! NOT responsible for: parsing (`vte`), control-sequence dispatch (`dispatch.rs`), buffer
//!   operations (`screen.rs`, `grid.rs`), style decoding (`sgr.rs`), tabs (`tabs.rs`),
//!   scrollback (`history.rs`) or reflow (`reflow.rs`, `resize.rs`).
//! Test strategy: measured against libghostty-vt by the differential corpus rather than by
//!   restating expected cell contents here.

use ruuah_vt_snapshot::{
    Cursor, Damage, Dirty, RowSemantic, Screen as SnapshotScreen, Snapshot, Style,
};
use unicode_width::UnicodeWidthChar;
use vte::{Params, Perform};

use crate::cell::{Cell, CellFlags, Wide};
use crate::reflow::Mode;
use crate::screen::Screen;
use crate::tabs::TabStops;

/// A terminal core: bytes in, grid mutations out.
pub struct Terminal {
    parser: vte::Parser,
    state: State,
}

impl Terminal {
    pub fn new(cols: u16, rows: u16) -> Terminal {
        Terminal::with_scrollback(cols, rows, 0)
    }

    pub fn with_scrollback(cols: u16, rows: u16, max_scrollback: usize) -> Terminal {
        Terminal {
            parser: vte::Parser::new(),
            state: State::new(cols, rows, max_scrollback),
        }
    }

    /// Feeds bytes to the parser. Resumable: `vte` carries partial UTF-8 and partial escape
    /// sequences across calls, so a sequence split mid-stream is handled.
    pub fn write(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.state, bytes);
    }

    /// Resizes both screens. The primary reflows, the alternate does not.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.state.resize(cols, rows);
    }

    /// Discards accumulated damage, starting a fresh observation window.
    pub fn clear_damage(&mut self) {
        self.state.full_damage = false;
        self.state.screen_mut().grid.clear_dirty();
    }

    /// What a renderer would have to repaint since damage was last cleared.
    pub fn damage(&self) -> Option<Damage> {
        Some(self.state.damage())
    }

    pub fn snapshot(&self) -> Snapshot {
        self.state.snapshot()
    }

    /// The active screen buffer, for a consumer that reads the grid directly.
    ///
    /// `snapshot` allocates a `String` per cell, which is right for a corpus case and wrong
    /// for a renderer reading every frame. This is the same state without the convenience.
    /// Drains the host-facing event queue (OSC 52, notifications, BEL), in order.
    pub fn take_events(&mut self) -> Vec<crate::events::Event> {
        std::mem::take(&mut self.state.events)
    }

    /// The working directory the child last reported via OSC 7, raw and undecoded.
    ///
    /// Empty means never reported or cleared: the oracle has no third state, because an
    /// empty report clears the buffer outright. Decoding the `file://` URI is the
    /// caller's job by design; see `pwd.rs`.
    /// Kitty virtual (U=1) placements: (image, cols, rows). The renderer pairs these
    /// with the placeholder cells that address them.
    pub fn virtuals(&self) -> &[(u32, u16, u16)] {
        &self.state.virtuals
    }

    pub fn pwd(&self) -> &[u8] {
        &self.state.pwd
    }

    /// Drains the protocol replies (DSR/DA) owed to the child, in order. The pump
    /// writes these to the pty; the core never does I/O.
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.state.replies)
    }

    /// Drains the kitty-graphics store ops (add/remove image), in order. The pump
    /// applies them to the shared store the renderer reads -- pixels move once.
    pub fn take_image_ops(&mut self) -> Vec<crate::graphics::ImageOp> {
        std::mem::take(&mut self.state.graphics.ops)
    }

    /// The URI behind a grid cell's link stamp (`Grid::link_id`). The stamp is a table
    /// index; the table is terminal-global so one link keeps one identity everywhere.
    pub fn link_uri(&self, id: u16) -> Option<&str> {
        self.state
            .link_table
            .get(usize::from(id))
            .map(|(_, uri)| uri.as_str())
    }

    pub fn screen(&self) -> &Screen {
        self.state.screen()
    }

    pub fn cursor(&self) -> Cursor {
        let screen = self.state.screen();
        Cursor {
            x: screen.x,
            y: screen.y,
            pending_wrap: screen.pending_wrap,
            visible: self.state.cursor_visible,
            style: self.state.pen,
        }
    }

    /// Whether the accumulated damage is a whole-frame event rather than a set of rows.
    pub fn is_wholly_damaged(&self) -> bool {
        self.state.full_damage
    }

    /// Enables screen-inspection reports (DECRQCRA checksums, XTERM_WINOPS size). Off by
    /// default -- they let the child read screen state back, the security class of the
    /// refused OSC 52 read. The esctest conformance harness is the intended caller.
    pub fn enable_reports(&mut self, on: bool) {
        self.state.reports_enabled = on;
    }

    /// Whether the child enabled synchronized output (DEC mode 2026). The pump reads
    /// this to hold frames back until the batch closes (or its anti-stuck budget runs
    /// out); the core itself renders nothing and gates nothing.
    pub fn synchronized_output(&self) -> bool {
        self.state.synchronized_output
    }

    /// Whether the child enabled bracketed paste (DEC mode 2004).
    ///
    /// The host consults this before writing a paste: wrapped in `ESC[200~`/`ESC[201~`
    /// when on, newlines folded to carriage returns when off. The core itself never
    /// writes either -- it does no I/O.
    pub fn bracketed_paste(&self) -> bool {
        self.state.bracketed_paste
    }

    /// The derived mouse-reporting kind (modes 9/1000/1002/1003, last writer wins).
    /// The host consults this to decide whether a pointer event becomes bytes; the
    /// core itself never encodes a report -- it does no I/O.
    pub fn mouse_event(&self) -> crate::mouse::MouseEvent {
        self.state.mouse.event
    }

    /// The derived report encoding (modes 1005/1006/1015/1016; legacy X10 otherwise).
    pub fn mouse_format(&self) -> crate::mouse::MouseFormat {
        self.state.mouse.format
    }

    /// Whether wheel events become arrow keys on the alternate screen (mode 1007,
    /// default ON). Routing itself is host policy; this is only the tracked state.
    pub fn mouse_alternate_scroll(&self) -> bool {
        self.state.mouse.alternate_scroll
    }

    /// DECCKM (mode 1): application cursor keys. Selects `ESC O A` over `ESC [ A` in
    /// the host's arrow and alternate-scroll encodings.
    pub fn cursor_keys(&self) -> bool {
        self.state.cursor_keys
    }

    /// Whether the alternate screen is active. The host's wheel routing needs it:
    /// alternate scroll (1007) applies only there.
    pub fn on_alternate_screen(&self) -> bool {
        matches!(self.state.active, Active::Alternate)
    }

    /// DECKPAM (mode 66): keypad application mode, for the host's key encoder.
    pub fn keypad_keys(&self) -> bool {
        self.state.keypad_keys
    }

    /// Mode 1035 (default ON): keypad application encoding yields to numlock.
    pub fn ignore_keypad_with_numlock(&self) -> bool {
        self.state.ignore_keypad_with_numlock
    }

    /// Mode 1036 (default ON): alt prefixes an ESC in legacy key encoding.
    pub fn alt_esc_prefix(&self) -> bool {
        self.state.alt_esc_prefix
    }

    /// xterm modifyOtherKeys state 2, for the host's key encoder.
    pub fn modify_other_keys_2(&self) -> bool {
        self.state.modify_other_keys_2
    }

    /// The ACTIVE screen's negotiated kitty keyboard flags -- the stack top, which is
    /// all an encoder ever consults. The stack itself stays per-screen and private.
    pub fn kitty_key_flags(&self) -> crate::kitty_keys::KittyKeyFlags {
        self.state.screen().kitty_keyboard.current()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Active {
    Primary,
    Alternate,
}

pub(crate) struct State {
    pub(crate) primary: Screen,
    pub(crate) alternate: Screen,
    pub(crate) active: Active,
    /// The style newly printed cells take.
    pub(crate) pen: Style,
    pub(crate) saved_pen: Option<Style>,
    pub(crate) tabs: TabStops,
    /// DECAWM (mode 7). On by default; when off, the last column overwrites in place.
    pub(crate) autowrap: bool,
    /// DECOM (mode 6). When on, row addressing is relative to the scroll region.
    pub(crate) origin: bool,
    /// DECTCEM (mode 25).
    pub(crate) cursor_visible: bool,
    /// DECCKM (mode 1). Application cursor keys: arrows encode `ESC O A` instead of
    /// `ESC [ A`. The core only TRACKS it; the host's key and alternate-scroll paths
    /// read it to pick the byte form, exactly as the oracle's Surface does.
    pub(crate) cursor_keys: bool,
    /// DECKPAM/DECKPNM (mode 66, also ESC = / ESC >): keypad application mode.
    pub(crate) keypad_keys: bool,
    /// Mode 1035 (default ON): numlock suppresses keypad application encoding.
    pub(crate) ignore_keypad_with_numlock: bool,
    /// Mode 1036 (default ON): alt-modified keys get an ESC prefix in legacy encoding.
    pub(crate) alt_esc_prefix: bool,
    /// xterm modifyOtherKeys state 2 (`CSI > 4 ; 2 m`). Not a DEC mode -- no
    /// mode_get/DECRQM surface -- so its only gate is the key-encoder differential.
    pub(crate) modify_other_keys_2: bool,
    /// Bracketed paste (mode 2004). Terminal-global, not per-screen: measured against the
    /// oracle, entering the alternate screen keeps it and only RIS or `2004l` clears it.
    /// The core only TRACKS it -- wrapping paste bytes is the host's job, because the
    /// core does no I/O.
    pub(crate) bracketed_paste: bool,
    /// Synchronized output (mode 2026). Terminal-global like 2004. The core only TRACKS
    /// it -- the batching itself is the pump's job (publish gating), because rendering
    /// cadence is I/O-adjacent policy the core must not own. Cleared by RIS and by ANY
    /// resize: measured on the oracle, whose resize clears synchronized output even at
    /// unchanged cell dimensions (stream_terminal.zig test, v1.3.2).
    pub(crate) synchronized_output: bool,
    /// The mouse-reporting modes (9/1000/1002/1003 events, 1005/1006/1015/1016 formats,
    /// 1007 alternate scroll). Terminal-global like 2004; the core only TRACKS them --
    /// encoding reports is the host's job, because the core does no I/O. `full_reset`
    /// restores defaults via `State::new`, which is what re-enables 1007.
    pub(crate) mouse: crate::mouse::MouseModes,
    /// Flat index of the last printed cell, so a following zero-width codepoint knows which
    /// cluster it belongs to. Cleared by anything that moves the cursor.
    pub(crate) last_print: Option<usize>,
    /// Host-facing requests (OSC 52, notifications, BEL), drained by the pump. See
    /// `events.rs` for the bounds.
    pub(crate) events: Vec<crate::events::Event>,
    /// Protocol replies (DSR/DA) owed to the child, drained by the pump to the pty.
    pub(crate) replies: Vec<u8>,
    /// Whether screen-inspection reports (DECRQCRA, XTERM_WINOPS size) answer. Off by
    /// default: they let a child read screen state back, the same security class as the
    /// refused OSC 52 read. The esctest harness turns them on; the app's posture is the
    /// operator's call.
    pub(crate) reports_enabled: bool,
    /// Kitty graphics: the image store and its op log (`graphics.rs`).
    pub(crate) graphics: crate::graphics::Graphics,
    /// The APC string being accumulated, if any; `true` when it overflowed and the
    /// whole command is to be dropped at the terminator.
    pub(crate) apc: Vec<u8>,
    pub(crate) apc_overflow: bool,
    /// A sixel DCS being decoded, between hook('q') and unhook.
    pub(crate) sixel: Option<crate::sixel::SixelDecoder>,
    /// Round-robin id source for sixel images in their private range.
    pub(crate) sixel_counter: u32,
    /// OSC 8: interned (explicit id, uri) pairs the grids' cell stamps point into.
    /// Terminal-global so a link spanning a screen switch keeps one identity.
    pub(crate) link_table: Vec<(String, String)>,
    /// The link newly printed cells are stamped with, as a `link_table` index.
    pub(crate) cursor_link: Option<u16>,
    pub(crate) max_scrollback: usize,
    /// OSC 7: the working directory the child last reported, raw. Terminal-global like
    /// the oracle's own (`Terminal.zig`), so a screen switch cannot disturb it and RIS
    /// clears it for free by rebuilding this struct. Empty is the only "unset".
    pub(crate) pwd: Vec<u8>,
    /// Kitty virtual (U=1) placements: (image, cols, rows). Terminal-global rather than
    /// per-screen, because a placeholder cell can be printed on either buffer and the
    /// image it names is one image either way.
    pub(crate) virtuals: Vec<(u32, u16, u16)>,
    /// Something changed that no per-row flag can express, so the whole frame is stale.
    ///
    /// Four triggers, each confirmed in upstream's `Terminal.zig` as the places that set its
    /// `dirty.clear` bit: a complete erase (ED 2, but NOT ED 0/1/3), a resize, a screen
    /// switch in either direction, and RIS. Measured the same way from the outside.
    pub(crate) full_damage: bool,
}

impl State {
    fn new(cols: u16, rows: u16, max_scrollback: usize) -> State {
        State {
            primary: Screen::new(cols, rows, max_scrollback),
            // The alternate screen has no scrollback, by protocol.
            alternate: Screen::new(cols, rows, 0),
            active: Active::Primary,
            pen: Style::DEFAULT,
            saved_pen: None,
            tabs: TabStops::new(cols),
            autowrap: true,
            origin: false,
            cursor_visible: true,
            cursor_keys: false,
            keypad_keys: false,
            ignore_keypad_with_numlock: true,
            alt_esc_prefix: true,
            modify_other_keys_2: false,
            bracketed_paste: false,
            synchronized_output: false,
            mouse: crate::mouse::MouseModes::default(),
            last_print: None,
            events: Vec::new(),
            pwd: Vec::new(),
            virtuals: Vec::new(),
            replies: Vec::new(),
            reports_enabled: false,
            graphics: crate::graphics::Graphics::default(),
            apc: Vec::new(),
            apc_overflow: false,
            sixel: None,
            sixel_counter: 0,
            link_table: Vec::new(),
            cursor_link: None,
            max_scrollback,
            full_damage: false,
        }
    }

    /// Flags the whole frame as stale, and every row with it.
    ///
    /// Both halves: upstream rebuilds the entire render state for these events, so every row
    /// comes back dirty as well as the global flag. Setting only the global one would report
    /// a clean row set for a frame that is entirely stale.
    pub(crate) fn mark_full_damage(&mut self) {
        self.full_damage = true;
        let rows = self.screen().rows();
        for y in 0..rows {
            self.screen_mut().grid.mark_dirty(y);
        }
    }

    /// What a renderer would have to repaint.
    ///
    /// `Partial` whenever any row is dirty, `Full` only for the whole-frame triggers. A
    /// scroll dirties every row and is still `Partial`: the distinction is not "how many
    /// rows" but "is per-row information meaningful at all".
    fn damage(&self) -> Damage {
        let rows: Vec<bool> = self.screen().grid.dirty_rows().to_vec();
        let global = if self.full_damage {
            Dirty::Full
        } else if rows.iter().any(|dirty| *dirty) {
            Dirty::Partial
        } else {
            Dirty::None
        };
        Damage { global, rows }
    }

    pub(crate) fn screen(&self) -> &Screen {
        match self.active {
            Active::Primary => &self.primary,
            Active::Alternate => &self.alternate,
        }
    }

    pub(crate) fn screen_mut(&mut self) -> &mut Screen {
        match self.active {
            Active::Primary => &mut self.primary,
            Active::Alternate => &mut self.alternate,
        }
    }

    /// The cell an erase paints with.
    ///
    /// Carries the pen's background and nothing else: background-colour erase fills with the
    /// current background, and the other attributes are not observable on a cell with no
    /// text. Interned against the active grid, since style tables are per-grid.
    pub(crate) fn blank(&mut self) -> Cell {
        let style = Style {
            bg: self.pen.bg,
            ..Style::DEFAULT
        };
        let style_id = self.screen_mut().grid.intern_style(style);
        Screen::blank_with(style_id)
    }

    fn snapshot(&self) -> Snapshot {
        let screen = self.screen();
        Snapshot {
            cols: screen.cols(),
            rows: screen.rows(),
            screen: match self.active {
                Active::Primary => SnapshotScreen::Primary,
                Active::Alternate => SnapshotScreen::Alternate,
            },
            modes: ruuah_vt_snapshot::Modes {
                bracketed_paste: self.bracketed_paste,
                synchronized_output: self.synchronized_output,
                mouse_event_x10: self.mouse.x10,
                mouse_event_normal: self.mouse.normal,
                mouse_event_button: self.mouse.button,
                mouse_event_any: self.mouse.any,
                mouse_format_utf8: self.mouse.utf8,
                mouse_format_sgr: self.mouse.sgr,
                mouse_format_urxvt: self.mouse.urxvt,
                mouse_format_sgr_pixels: self.mouse.sgr_pixels,
                mouse_alternate_scroll: self.mouse.alternate_scroll,
                cursor_keys: self.cursor_keys,
                keypad_keys: self.keypad_keys,
                ignore_keypad_with_numlock: self.ignore_keypad_with_numlock,
                alt_esc_prefix: self.alt_esc_prefix,
            },
            cursor: Cursor {
                x: screen.x,
                y: screen.y,
                pending_wrap: screen.pending_wrap,
                visible: self.cursor_visible,
                style: self.pen,
            },
            grid: screen.grid.to_rows(),
            history: screen.history.to_rows(),
            damage: None,
            pwd: self.pwd.clone(),
        }
    }

    fn print_char(&mut self, c: char, width: u16) {
        let blank = self.blank();

        // A deferred wrap is resolved before the character lands, never after: the whole
        // point of the phantom state is that the wrap belongs to this character, not the
        // previous one.
        if self.screen().pending_wrap {
            if self.autowrap {
                self.wrap(blank);
            } else {
                self.screen_mut().pending_wrap = false;
            }
        }

        let cols = self.screen().cols();
        if cols == 0 {
            return;
        }

        // A wide character that cannot fit in the last column leaves a spacer head behind
        // and starts on the next row, rather than being split across the wrap.
        if width == 2 && self.screen().x + 1 >= cols {
            if !self.autowrap {
                return;
            }
            // Through the ordinary cell path, not the erase blank: upstream's spacer head
            // takes cursor.style_id and the cursor's semantic_content (Terminal.zig:1411
            // into the write at 1565), so it is bold-and-red under a bold red pen and part
            // of the input under OSC 133 (finding 27).
            let pen = self.pen;
            let semantic = self.screen().semantic_content;
            let style_id = self.screen_mut().grid.intern_style(pen);
            let (x, y) = (self.screen().x, self.screen().y);
            let index = self.screen().grid.index(x, y);
            self.screen_mut().grid.write(
                index,
                Cell {
                    codepoint: 0,
                    style_id,
                    wide: Wide::SpacerHead,
                    flags: CellFlags::with_semantic(semantic),
                },
            );
            self.stamp_link(index);
            self.wrap(blank);
        }

        let pen = self.pen;
        let semantic = self.screen().semantic_content;
        let style_id = self.screen_mut().grid.intern_style(pen);
        let (x, y) = (self.screen().x, self.screen().y);
        let index = self.screen().grid.index(x, y);
        self.screen_mut().grid.write(
            index,
            Cell {
                codepoint: c as u32,
                style_id,
                wide: if width == 2 { Wide::Wide } else { Wide::Narrow },
                flags: CellFlags::with_semantic(semantic),
            },
        );
        self.stamp_link(index);
        self.last_print = Some(index);

        if width == 2 {
            self.screen_mut().grid.write(
                index + 1,
                Cell {
                    codepoint: 0,
                    style_id,
                    wide: Wide::SpacerTail,
                    flags: CellFlags::with_semantic(semantic),
                },
            );
            self.stamp_link(index + 1);
        }

        // At the right edge the cursor stays put and the wrap is deferred. Advancing to
        // `cols` instead would be off the grid and would lose the phantom state.
        if x + width >= cols {
            let screen = self.screen_mut();
            screen.x = cols - 1;
            screen.pending_wrap = true;
        } else {
            self.screen_mut().x = x + width;
        }
    }

    pub(crate) fn cursor_x(&self) -> u16 {
        self.screen().x
    }

    pub(crate) fn cursor_y(&self) -> u16 {
        self.screen().y
    }

    /// Moves the cursor and invalidates the grapheme anchor: a combining mark arriving after
    /// a jump belongs to nothing, and attaching it to a stale cell would corrupt that cell.
    pub(crate) fn goto(&mut self, x: u16, y: u16) {
        self.screen_mut().move_to(x, y);
        self.last_print = None;
    }

    /// Absolute cursor addressing, honouring DECOM: with origin mode on, row 1 is the top
    /// margin and the cursor cannot leave the region.
    pub(crate) fn cursor_position(&mut self, row: u16, col: u16) {
        let (top, bottom) = if self.origin {
            (self.screen().scroll_top, self.screen().scroll_bottom)
        } else {
            (0, self.screen().rows().saturating_sub(1))
        };
        let y = top.saturating_add(row).min(bottom);
        self.screen_mut().move_to(col, y);
        self.last_print = None;
    }

    /// CUU. The scroll region bounds the move, not the screen.
    ///
    /// A cursor at or below the top margin cannot be carried above it; one already above the
    /// region is bounded by the top of the screen instead, because a cursor outside the region
    /// is not confined by it (`Terminal.zig:1703`). Clamping to the screen alone lets CUU walk
    /// out of the region from below, which every full-screen TUI would hit.
    pub(crate) fn cursor_up(&mut self, count: u16) {
        let y = self.cursor_y();
        let top = self.screen().scroll_top;
        let limit = if y >= top { y - top } else { y };
        let x = self.cursor_x();
        self.goto(x, y - count.min(limit));
    }

    /// CUD, the mirror rule (`Terminal.zig:1721`).
    pub(crate) fn cursor_down(&mut self, count: u16) {
        let y = self.cursor_y();
        let bottom = self.screen().scroll_bottom;
        let limit = if y <= bottom {
            bottom - y
        } else {
            self.screen().rows().saturating_sub(1).saturating_sub(y)
        };
        let x = self.cursor_x();
        self.goto(x, y + count.min(limit));
    }

    /// ED 2 at a prompt scrolls the screen into scrollback before clearing, so `^L` keeps
    /// history (`Terminal.zig:3303-3337`, the `at_prompt` block).
    ///
    /// Upstream walks the active area upwards from the bottom, but `SemanticPrompt` has only
    /// the three values matched there -- a prompt row breaks the loop and a `none` row aborts
    /// it -- so the decision is settled by the bottom row alone. The alternate screen is
    /// excluded: it has no scrollback to keep.
    pub(crate) fn scroll_clear_at_prompt(&mut self, blank: Cell) {
        if self.active != Active::Primary {
            return;
        }
        let last = self.screen().rows().saturating_sub(1);
        let at_prompt = matches!(
            self.screen().grid.row_meta(last).semantic_prompt,
            RowSemantic::Prompt | RowSemantic::PromptContinuation
        );
        if at_prompt {
            self.screen_mut().scroll_clear(blank);
        }
    }

    pub(crate) fn save_cursor(&mut self) {
        self.screen_mut().save_cursor();
        self.saved_pen = Some(self.pen);
    }

    pub(crate) fn restore_cursor(&mut self) {
        self.screen_mut().restore_cursor();
        self.pen = self.saved_pen.unwrap_or(Style::DEFAULT);
        self.last_print = None;
    }

    /// Upstream's horizontalTab is a bare cursorRight loop that never touches
    /// `pending_wrap` (Terminal.zig:2111) -- at the last column it does nothing at all.
    pub(crate) fn tab_forward(&mut self, count: u16) {
        for _ in 0..count.max(1) {
            let next = self.tabs.next(self.screen().x);
            self.screen_mut().x = next;
        }
        self.last_print = None;
    }

    pub(crate) fn tab_backward(&mut self, count: u16) {
        for _ in 0..count.max(1) {
            let previous = self.tabs.previous(self.screen().x);
            self.screen_mut().x = previous;
        }
        self.screen_mut().pending_wrap = false;
        self.last_print = None;
    }

    /// Resizes both screens and rebuilds the tab stops for the new width.
    ///
    /// Both screens, not just the active one: a program that resizes while in the alternate
    /// screen and then leaves it would otherwise find the primary still at the old geometry.
    pub(crate) fn resize(&mut self, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 {
            return;
        }
        // ANY resize ends a synchronized batch -- the oracle clears 2026 even when the
        // cell dimensions did not change (its own test pins that), because a program
        // holding the gate across a resize would freeze the redraw the user just asked
        // for. Corpus-pinned in both the changed and unchanged directions.
        self.synchronized_output = false;
        // The primary rejoins soft-wrapped lines only while DECAWM is on -- upstream passes
        // reflow = modes.get(.wraparound) (Terminal.zig:3783). The alternate is documented
        // by the ABI as not reflowing, so its rows only gain or lose columns.
        let primary_mode = if self.autowrap {
            Mode::Rejoin
        } else {
            Mode::Truncate
        };
        let cols_changed = cols != self.screen().cols();
        crate::resize::apply(&mut self.primary, cols, rows, primary_mode);
        crate::resize::apply(&mut self.alternate, cols, rows, Mode::Truncate);

        // Inline images ride the reflow: `resize::apply` maps each placement's anchor
        // through the same transform as the cursors (v2, replacing the interim
        // clear-on-resize). An anchor whose row does not survive drops its placement.

        // Only when the column count changed -- upstream guards its rebuild on exactly that
        // (`Terminal.zig:3766`), so a rows-only resize keeps custom HTS stops (finding 21).
        if cols_changed {
            self.tabs = TabStops::new(cols);
        }
        self.last_print = None;
        // After the grids are rebuilt, so the whole NEW geometry is marked rather than the
        // old row count. Upstream sets the same clear bit from its own resize.
        self.mark_full_damage();
    }

    pub(crate) fn full_reset(&mut self) {
        let cols = self.screen().cols();
        let rows = self.screen().rows();
        // The reports toggle survives: it is the EMBEDDER's capability grant, not
        // terminal state -- a child must not be able to revoke it with a RIS (esctest
        // would lose its screen readback mid-run).
        let reports = self.reports_enabled;
        *self = State::new(cols, rows, self.max_scrollback);
        self.reports_enabled = reports;
        self.mark_full_damage();
    }

    /// DECSTR (CSI ! p), the soft reset. The oracle ignores the sequence entirely (no
    /// `!` intermediate dispatch in its stream, v1.3.2), so this is a deliberate,
    /// corpus-pinned divergence in xterm's direction: esctest2 runs DECSTR before every
    /// test, and a terminal that ignores it carries state across test boundaries,
    /// making every result order-dependent.
    ///
    /// Effects are xterm's core set: cursor visible (DECTCEM), origin mode off (DECOM),
    /// autowrap ON -- xterm resets it to its resource default rather than the spec's
    /// off, its own comment saying applications rely on it, and esctest re-sets it
    /// after every DECSTR anyway -- margins reset, pen and saved pen to default,
    /// pending wrap cancelled, saved cursor cleared. The grid is NOT touched: that is
    /// RIS's job.
    pub(crate) fn soft_reset(&mut self) {
        self.cursor_visible = true;
        self.origin = false;
        self.autowrap = true;
        // xterm's DECSTR resets DECCKM to normal; part of the same xterm-shaped set,
        // as is the keypad returning to numeric (VT510's DECSTR table lists both).
        self.cursor_keys = false;
        self.keypad_keys = false;
        self.pen = Style::DEFAULT;
        self.saved_pen = None;
        let screen = self.screen_mut();
        screen.reset_scroll_region();
        screen.pending_wrap = false;
        screen.saved = None;
        self.mark_full_damage();
    }
}

impl Perform for State {
    fn print(&mut self, c: char) {
        // A zero-width codepoint continues the previous cell's grapheme cluster rather than
        // claiming a cell -- ranked failure mode 2, a cell is not a codepoint. This is the
        // width heuristic, which matches Ghostty while DEC mode 2027 is off (verified
        // 2026-07-28); full UAX #29 segmentation is only needed once 2027 is implemented.
        let width = UnicodeWidthChar::width(c).unwrap_or(0);
        if width == 0 {
            if let Some(index) = self.last_print {
                self.screen_mut().grid.push_grapheme(index, c);
            }
            return;
        }
        self.print_char(c, if width >= 2 { 2 } else { 1 });
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x07 => self.push_event(crate::events::Event::Bell),
            0x08 => {
                let x = self.screen().x.saturating_sub(1);
                self.screen_mut().x = x;
                self.screen_mut().pending_wrap = false;
                self.last_print = None;
            }
            0x09 => self.tab_forward(1),
            0x0a | 0x0b | 0x0c => {
                let blank = self.blank();
                self.index(blank);
                self.last_print = None;
            }
            0x0d => {
                self.screen_mut().x = 0;
                self.screen_mut().pending_wrap = false;
                self.last_print = None;
            }
            _ => {}
        }
    }

    fn hook(&mut self, _params: &vte::Params, _intermediates: &[u8], _ignore: bool, action: char) {
        if action == 'q' {
            self.sixel = Some(crate::sixel::SixelDecoder::new());
        }
    }

    fn put(&mut self, byte: u8) {
        if let Some(decoder) = self.sixel.as_mut() {
            decoder.put(byte);
        }
    }

    fn unhook(&mut self) {
        let Some(decoder) = self.sixel.take() else {
            return;
        };
        let Some(image) = decoder.finish() else {
            return;
        };
        self.sixel_counter = self.sixel_counter.wrapping_add(1) & 0x000F_FFFF;
        let id = crate::sixel::SIXEL_ID_BASE + self.sixel_counter;
        // Through the SAME store and placement path kitty uses -- one pipeline. Budget
        // and eviction rules apply identically.
        self.graphics.images.insert(id, image.clone());
        self.graphics.budget_used += image.rgba.len();
        self.graphics.ops.push(crate::graphics::ImageOp::Add(id, image.clone()));
        self.place_sixel(id);
    }

    fn apc_start(&mut self) {
        self.apc.clear();
        self.apc_overflow = false;
    }

    fn apc_put(&mut self, byte: u8) {
        if self.apc.len() >= crate::graphics::APC_CEILING {
            self.apc_overflow = true;
            return;
        }
        self.apc.push(byte);
    }

    fn apc_end(&mut self) {
        if self.apc_overflow || self.apc.is_empty() {
            return;
        }
        if self.apc.first() == Some(&b'G') {
            let apc = std::mem::take(&mut self.apc);
            self.apc_graphics(&apc);
            self.apc = apc; // reuse the allocation; content is dead
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        match params.first().copied() {
            Some(b"0") | Some(b"2") => self.osc_title(params),
            Some(b"7") => self.osc_pwd(params),
            Some(b"8") => self.osc_hyperlink(params),
            Some(b"52") => self.osc_clipboard(params),
            Some(b"9") => self.osc_notify_9(params),
            Some(b"777") => self.osc_notify_777(params),
            _ => {
                let blank = self.blank();
                self.osc(params, blank);
            }
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        self.csi(params, intermediates, ignore, action);
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        self.esc(intermediates, ignore, byte);
    }
}
