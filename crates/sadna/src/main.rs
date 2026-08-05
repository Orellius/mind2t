//! Purpose: B2.4 - Sadna's Tauri host. A Tauri window whose terminal is a native GPU surface
//!   and whose chrome is a React webview docked above it.
//! Public surface: a binary.
//! Why this file: Orel chose Tauri + React + TypeScript on 2026-08-04. Everything unusual here
//!   is downstream of one measurement and one law:
//!   - **Order.** B2.2 proved a wgpu surface created AFTER a webview lands on top of it on macOS
//!     (dioxus#3727). Tauri's ordinary `WebviewWindow` creates both at once, so this host uses
//!     `WindowBuilder` -> surface -> `Window::add_child(webview)`, which needs Tauri's `unstable`
//!     feature. That is the whole reason the flag is on.
//!   - **Keys.** `tauri::WindowEvent` has no keyboard variant (Resized, Moved, CloseRequested,
//!     Destroyed, Focused, ScaleFactorChanged, DragDrop, ThemeChanged - verified 2026-08-04), so
//!     Tauri cannot hand us the terminal's keystrokes. They must NOT come through the webview:
//!     project law 2 says no terminal bytes there, and routing keys through React would be
//!     xterm.js's mistake by another road. So this host reads `NSEvent` itself.
//! NOT responsible for: VT semantics, rendering, or the pipeline - `ruuah_vt_host::session` owns
//!   all of it. This file is a window, an event monitor, and a clock.
//! Test strategy: what can be checked by a machine is checked where it lives (session pipeline,
//!   blit origin, keycode tables, key encoding). What is left is a GUI and gets a live tap. The
//!   tao + wry oracle stays runnable as `cargo run --bin probe` for the question "does the pair
//!   do this too?".

use std::cell::{Cell, RefCell};
use std::process::Command;
use std::rc::Rc;

use sadna::canvas::{Canvas, PaneSpec};
use sadna::layout::{Canvas as Grid, Rect};
use ruuah_vt_host::session::{MouseAction, MouseMods, Session};
use ruuah_vt_render::{Fill, GpuContext, GpuSurface, WindowTarget};
use tauri::webview::WebviewBuilder;
use tauri::window::WindowBuilder;
use tauri::{Emitter, Listener, LogicalPosition, LogicalSize, Manager, RunEvent, WebviewUrl};

/// Logical height of the chrome strip. Must match the CSS that draws it.
const BAR_HEIGHT: f64 = 36.0;

/// Logical point size. The session is built at `FONT_SIZE * scale` so one buffer pixel is one
/// DEVICE pixel - see the project CLAUDE.md; this repo has learned it twice already.
const FONT_SIZE: f32 = 16.0;

/// The canvas the host opens with: ONE pane, like any other terminal (Orel, 2026-08-06).
///
/// It opened pre-split into two while B3.4 was proving that a canvas could be composited at all.
/// That was scaffolding, and it read as a window that had already been used - panes are the
/// operator's to make now, with cmd+D, and the wizard (B5) is what declares a bigger canvas up
/// front.
///
/// The gutter is ZERO here and filled in by [`grid`] from the display's scale, because this is a
/// const and scale is a runtime fact. A single pane has no neighbour and therefore no rule either
/// way; the gutter starts mattering at the first split.
const GRID: Grid = Grid { rows: 1, cols: 1, gutter: 0 };

/// Logical thickness of the rule between panes. One point - a hairline, not a border.
const DIVIDER: f64 = 1.0;

/// The opening canvas at a given display scale.
///
/// A rule declared in POINTS and used as pixels is invisible on the display this project is
/// developed on: at scale 2 it renders half as thick as asked, and one physical pixel of grey
/// between two dark terminals is a seam you have to hunt for. Same class as the font size, which
/// this repo has now paid for twice (slice 8, then B2.3 - the operator caught the second one from
/// the screen). At least one pixel, so the rule never rounds away entirely.
fn grid(scale: f64) -> Grid {
    Grid {
        gutter: ((DIVIDER * scale).round() as u32).max(1),
        ..GRID
    }
}

/// The rule's colour, derived from the terminal's own background rather than configured.
///
/// A fixed grey is wrong against half the themes it will meet - invisible on a light one, a bright
/// scar on a dark one. This lifts a dark background and drops a light one by a fixed amount, so
/// the rule reads as a seam in whatever the terminal is already wearing, and a theme change moves
/// it for free. It is a MECHANISM with a defensible default; the exact weight is the operator's
/// eye to judge, and until he has judged it the look is not claimed as verified.
fn divider_color(background: [u8; 4]) -> [u8; 4] {
    let luma = (u16::from(background[0]) * 2 + u16::from(background[1]) * 5 + u16::from(background[2]))
        / 8;
    let shift = |channel: u8| -> u8 {
        if luma < 128 {
            channel.saturating_add(38)
        } else {
            channel.saturating_sub(38)
        }
    };
    [shift(background[0]), shift(background[1]), shift(background[2]), 255]
}

const SESSION_EVENT: &str = "sadna://session";

/// Runs the whole host with NOTHING on screen: the window is created hidden, no key monitor is
/// installed, diagnostics are printed and the process exits on its own.
///
/// This is not a test double - it is the SAME host, with its window ordered out. That matters:
/// a headless mode that skipped the window would not exercise the swapchain, the origin or the
/// child webview, which are exactly the things being diagnosed. What it removes is the two
/// things that make a running window hostile to whoever owns the machine - it cannot take focus
/// and it cannot swallow a keystroke.
///
/// Enabled with `SADNA_HEADLESS=1`.
///
/// A CEILING, not a duration: the run ends the moment it has collected everything it came for,
/// so a healthy gate is far quicker than this. The number only has to outlast the slowest thing
/// being waited on - a shell that must start, echo a command, and print two hundred lines.
const HEADLESS_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);

/// The directory the smoke makes the child report over OSC 7.
///
/// Deliberately carries a HOST (`localhost`) that is discarded by `cwd::normalize`, and
/// deliberately names a directory that need not exist: what is under test is the report's
/// journey, not the filesystem.
const CWD_PROBE: &str = "/tmp/ruuah-cwd-probe";

/// What the smoke pastes. No newline: it must sit on the prompt as text, not execute.
const PASTE_PROBE: &str = "sadna-paste-probe";

/// What the smoke asks the child to print once its scrollback is filled.
const FILL_MARKER: &str = "SADNA-FILLED";

/// What the child prints when it has turned mouse reporting on.
const MOUSE_MARKER: &str = "SADNA-MOUSE";

/// The SGR report a press in the top-left cell produces, as `cat` echoes it back: ESC is drawn
/// `^[` by ECHOCTL, so this is the printable form, not the wire form.
const MOUSE_REPORT: &str = "^[[<0;1;1M";

/// Where the smoke clicks: cell (0,0) of the pane, one pixel in from each edge so it is
/// unambiguously inside that cell at any font size.
///
/// PANE-LOCAL, not window-space. Each pane's mouse geometry is its own rect with no padding
/// (`Canvas::spawn`), so the strip is no longer subtracted here - it is already excluded by the
/// rect. Passing window coordinates would silently report a cell some rows down.
fn click(session: &mut Session) -> bool {
    let pressed = session
        .mouse(MouseAction::Press, 1, MouseMods::default(), 1.0, 1.0)
        .unwrap_or(false);
    // The release is sent whatever the press did, because the held-button bookkeeping is not
    // conditional: a press recorded and a release dropped leaves a button held forever.
    let released = session
        .mouse(MouseAction::Release, 1, MouseMods::default(), 1.0, 1.0)
        .unwrap_or(false);
    pressed && released
}

/// How long after the chrome announces itself before its DOM is read. One render plus the
/// replayed state; generous, because the cost of being early is a false failure.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(400);

/// How often the DOM is re-read while it still disagrees with the host.
const REPROBE: std::time::Duration = std::time::Duration::from_millis(400);

fn headless() -> bool {
    std::env::args().any(|argument| argument == "--smoke")
        || std::env::var("SADNA_HEADLESS").is_ok_and(|value| value == "1")
}

/// What the smoke run learned, filled in as it happens and judged at the end.
#[derive(Default)]
struct Smoke {
    chrome_url: Option<String>,
    document: Option<String>,
    layers: Vec<String>,
    /// The FOCUSED pane's top-left in the window, in physical pixels.
    ///
    /// It used to be the window target's own origin, which is the number a single terminal is
    /// placed by. A canvas has no such number: every pane's position lives in its rect, and the
    /// rect is what the blit uses - so reading anything else would be checking a value the frame
    /// does not consult.
    origin: Option<(u32, u32)>,
    grid: Option<(u16, u16)>,
    /// Every pane's rect and grid, in order. The canvas as the LIVE WINDOW built it, which is a
    /// different claim from `layout.rs`'s arithmetic over a synthetic area.
    panes: Vec<(Rect, (u16, u16))>,
    /// The rules between the panes, as the LIVE canvas emits them, and the gutter it reserved.
    ///
    /// Recorded rather than recomputed here: the gutter is derived from the display's scale, and
    /// a check that computed its own expected value from `DIVIDER * scale` would agree with a host
    /// that had never applied the scale at all.
    dividers: Vec<Rect>,
    gutter: Option<u32>,
    /// How many panes the window OPENED with, before the gate split it.
    panes_at_open: Option<usize>,
    /// The window's own scale factor, so the gutter can be checked against the display rather
    /// than against the constant it was derived from.
    scale: Option<f64>,
    /// The same, taken immediately before and after the window is resized, with the size asked
    /// for. Three records rather than one because "it re-tiled" is only meaningful against what
    /// it was: a canvas that ignored the resize entirely still tiles perfectly, at the old size.
    before_resize: Vec<(Rect, (u16, u16))>,
    after_resize: Vec<(Rect, (u16, u16))>,
    resized_to: Option<u32>,
    /// Title bar inset plus strip, in physical pixels: what the origin MUST clear.
    reserved: Option<u32>,
    /// Every distinct directory the SESSION decoded, in the order it decoded them.
    ///
    /// A LIST rather than the final value, and that is a measurement rather than a preference:
    /// zsh's shell integration re-reports its own directory after every command, so the probe's
    /// report is overwritten seconds after it arrives and a run that read the value at the end
    /// found the repository - correctly, and uselessly. The question is whether the report ever
    /// reached the session at all, and only a record of the sequence can answer it.
    cwd_seen: Vec<String>,
    /// Whether the pasted fixture reached the child and came back as grid text.
    pasted: bool,
    /// The grid before the wheel, again after a pause with no input, and after scrolling. Three
    /// reads rather than two because the middle one is the control: it is what tells a viewport
    /// that MOVED from a grid that was simply still changing on its own.
    before_scroll: Option<String>,
    steady: Option<String>,
    after_scroll: Option<String>,
    /// Whether a click was reported to the child BEFORE it asked for mouse reporting, and after.
    /// Two directions of the same claim: a host that encodes unconditionally answers `true`
    /// twice, and a host whose mouse is not wired answers `false` twice.
    click_before_enable: Option<bool>,
    click_after_enable: Option<bool>,
    /// Whether the report came back off the child as text - the proof it was not merely encoded.
    mouse_echo: bool,
}

impl Smoke {
    /// Every check, and what each one exists to catch. Returns whether all of them held.
    ///
    /// Each line is a defect this session actually hit, in the order it hit them. That is the
    /// point of writing them down as a gate rather than as a paragraph: the next person to touch
    /// the host layer finds out in seconds, headlessly, instead of by staring at a window.
    fn judge(&self, page_finished: bool, chrome_ready: bool) -> bool {
        let document = self.document.clone().unwrap_or_default();
        let grid = self
            .grid
            .map(|(cols, rows)| format!("{cols}x{rows}"))
            .unwrap_or_default();

        let z_ordered = self.layers.len() >= 2
            && self
                .layers
                .iter()
                .position(|layer| layer.contains("Wgpu"))
                .zip(self.layers.iter().rposition(|layer| !layer.contains("Wgpu")))
                .is_some_and(|(terminal, chrome)| terminal < chrome);

        // Three reads, one verdict: the viewport moved AND the grid was otherwise still. Without
        // the middle term, a child that kept printing would satisfy "it changed" forever and the
        // check would pass on a host whose wheel is not wired at all.
        let scrolled = self.steady.is_some()
            && self.steady == self.before_scroll
            && self.after_scroll.is_some()
            && self.after_scroll != self.steady;

        // Every pane is a real terminal, and the panes TILE the area in the live window: pane
        // n+1 starts exactly where pane n ends, on the same row of the grid. Checked here as
        // well as in `layout.rs` because that test tiles a number, and this one tiles a window -
        // the strip, the scale factor and the title-bar inset are only in this path, and each of
        // them has been wrong once already.
        let gutter = self.gutter.unwrap_or(0);
        // Two panes, because the gate split the one it opened with. Not `GRID`, which now
        // describes the OPENING canvas and would make this check agree with a split that never
        // happened.
        let tiled = self.panes.len() == 2
            && self
                .panes
                .iter()
                .all(|(_, (cols, rows))| *cols > 1 && *rows > 1)
            && self.panes.windows(2).all(|pair| {
                pair[1].0.x == pair[0].0.x + pair[0].0.width + gutter && pair[1].0.y == pair[0].0.y
            });

        // The rule between the panes is REAL in the live window: one per boundary, exactly filling
        // the gap the panes left, and at least as thick as the display's scale demands.
        //
        // Two failures this catches that the arithmetic in `layout.rs` cannot, because both live
        // in the wiring above it. A host that reserves the gutter and never draws into it leaves a
        // hairline of clear colour that reads as a rendering artifact rather than as a missing
        // feature. And a gutter computed in POINTS instead of physical pixels is half as thick as
        // asked on this display and looks like a design choice - the same scale trap the font size
        // has now sprung twice, which is why the thickness is asserted against the window's own
        // scale rather than against the constant.
        let ruled = !self.dividers.is_empty()
            && self.dividers.len() == self.panes.len().saturating_sub(1)
            && gutter >= (DIVIDER * self.scale.unwrap_or(1.0)).round() as u32
            && self
                .panes
                .windows(2)
                .zip(&self.dividers)
                .all(|(pair, rule)| {
                    rule.x == pair[0].0.x + pair[0].0.width
                        && rule.width == gutter
                        && rule.height == pair[0].0.height
                });

        // A resize must reach BOTH halves: the rects re-tile the new window, and every pane's own
        // grid follows. A canvas that ignored the event tiles the OLD area perfectly, and a
        // canvas that re-tiled without resizing its ptys leaves every child drawing at its old
        // width, underneath its neighbour - neither errors, and both look healthy.
        let re_tiled = !self.after_resize.is_empty()
            && self.after_resize.len() == self.before_resize.len()
            && self
                .after_resize
                .windows(2)
                .all(|pair| pair[1].0.x == pair[0].0.x + pair[0].0.width + gutter)
            && self.after_resize[0].0.x == 0
            && self.resized_to.is_some_and(|width| {
                let last = self.after_resize[self.after_resize.len() - 1].0;
                last.x + last.width == width
            })
            && self
                .before_resize
                .iter()
                .zip(&self.after_resize)
                .all(|(before, after)| after.1.0 < before.1.0 && after.1.1 < before.1.1);

        let checks: [(bool, &str); 19] = [
            (
                self.panes_at_open == Some(1),
                "the window OPENS with one pane, like any other terminal - panes are made with \
                 cmd+D, and a window that arrives pre-split reads as one already in use",
            ),
            (
                self.panes.len() == 2 && self.panes_at_open == Some(1),
                "and a split ADDED one - the same call cmd+D makes, so a split that silently \
                 replaced the pane rather than adding beside it fails here",
            ),
            (
                self.grid.is_some_and(|(cols, rows)| cols > 1 && rows > 1),
                "the session has a real grid",
            ),
            (
                tiled,
                "the canvas has one live pane per cell and they tile the window with exactly the \
                 gutter between them - a pane that kept the full width draws UNDER its neighbour \
                 and looks entirely normal",
            ),
            (
                ruled,
                "a rule fills the gap between panes, thick enough for this display's scale - a \
                 reserved gutter nobody draws into is a hairline of clear colour, and a gutter \
                 measured in points is half as thick as asked at scale 2",
            ),
            (
                self.origin
                    .zip(self.reserved)
                    .is_some_and(|((x, y), reserved)| x == 0 && y >= reserved),
                "the terminal clears the title bar AND the strip - `y > 0` was too weak a check \
                 and passed while the chrome sat behind the title bar with 8pt showing",
            ),
            (
                self.chrome_url.as_deref() == Some("tauri://localhost"),
                "the chrome loads the BUILT assets - a devUrl in the config silently points every webview at a dev server that is not running",
            ),
            (page_finished, "the chrome document finished loading"),
            (
                document.contains("\"root\":true") && document.contains("\"scripts\":1"),
                "the loaded document really is ours - a failed load is an empty page, not an error",
            ),
            (
                chrome_ready,
                "the chrome reached the host over IPC - with no capability file it silently cannot",
            ),
            (
                !grid.is_empty() && document.contains(&grid),
                "the host reached the chrome: the strip renders the REAL grid, which also proves the replay-on-handshake path",
            ),
            (z_ordered, "the chrome layer sits ABOVE the terminal layer"),
            (
                self.cwd_seen.iter().any(|seen| seen == CWD_PROBE),
                "the child's OSC 7 report becomes the session's directory - the core stores it \
                 RAW, so a host that never drains the event queue shows a placeholder forever",
            ),
            (
                document.contains(CWD_PROBE),
                "and that directory reaches the chrome - the strip is the only place the \
                 operator sees it, and it is a second hop that can fail on its own",
            ),
            (
                self.pasted,
                "a paste reaches the child - the menu's Paste reaches the FIRST RESPONDER and \
                 the terminal is a GPU layer with none, so cmd+V does nothing unless the host \
                 does it itself",
            ),
            (
                scrolled,
                "the wheel scrolls the viewport, and the grid was otherwise still - a grid that \
                 kept changing would satisfy 'it moved' with the wheel unwired",
            ),
            (
                self.click_before_enable == Some(false) && self.click_after_enable == Some(true),
                "a click belongs to the HOST until the child asks for it and to the child after \
                 - both directions, because a host that encodes unconditionally and a host with \
                 no mouse at all each get one of them right",
            ),
            (
                self.mouse_echo,
                "and the report really reached the child: it came back off the pty as text, \
                 which encoding it into a buffer nobody wrote would not",
            ),
            (
                re_tiled,
                "a window resize re-tiles every pane to the NEW area and shrinks every pane's \
                 own grid with it - the first thing an operator does, and the one path where \
                 tiling and the pty can disagree",
            ),
        ];

        let mut passed = true;
        for (held, what) in checks {
            println!("sadna: [{}] {what}", if held { "PASS" } else { "FAIL" });
            passed &= held;
        }
        passed
    }
}


/// Drives the input paths a gate can exercise with NOTHING on screen, one stage at a time.
///
/// The three of them - a directory report, a paste, a wheel - all end in the same place (the
/// grid, or the chrome's document), so they cannot run at once: a paste landing mid-scroll
/// would make both readings ambiguous. Hence a machine rather than three timers.
///
/// It drives the SESSION directly, never AppKit. Synthesizing a real cmd+V or a real wheel
/// event would put input into whatever the operator is doing on this machine, which is exactly
/// what the headless gate exists to avoid - so the AppKit half (the monitor's mask, the chord
/// match, the pointer-versus-strip test) remains a live-tap item and is named as one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Waiting for the chrome to settle, so the strip can be read after the report arrives.
    Waiting,
    /// The child is printing; the marker says it finished.
    Filling,
    /// The fixture has been pasted; waiting for it to come back as grid text.
    Pasting,
    /// The directory report is in flight, and the child is deliberately still busy.
    Reporting,
    /// A pause with no input at all, which is what makes the scroll reading mean something.
    Steadying,
    /// Scrolled; waiting for the pump to publish the moved viewport.
    Scrolled,
    /// Clicked with reporting on; waiting for the report to come back off the child.
    Clicking,
    Done,
}

struct InputProbe {
    stage: Stage,
    since: std::time::Instant,
}

impl InputProbe {
    /// How long a stage waits for the child before giving up and letting its check FAIL.
    ///
    /// A stage that gave up silently and skipped ahead would turn a real defect into a green
    /// run; every path out of a stage either records its evidence or leaves it missing.
    const PATIENCE: std::time::Duration = std::time::Duration::from_secs(4);

    /// The still window before and after the wheel. Long enough for the pump (25ms ticks) to
    /// publish, short enough that the whole gate stays a few seconds.
    const STILLNESS: std::time::Duration = std::time::Duration::from_millis(300);

    fn new() -> InputProbe {
        InputProbe {
            stage: Stage::Waiting,
            since: std::time::Instant::now(),
        }
    }

    fn done(&self) -> bool {
        self.stage == Stage::Done
    }

    fn enter(&mut self, stage: Stage) {
        self.stage = stage;
        self.since = std::time::Instant::now();
    }

    /// Advances one step if this stage's condition is met. `settled` is the chrome handshake
    /// plus its render window - the machine starts only after it, because the first thing it
    /// does is change what the strip must say.
    fn advance(&mut self, session: &mut Session, smoke: &mut Smoke, settled: bool) {
        let waited = self.since.elapsed();
        match self.stage {
            Stage::Waiting => {
                if !settled {
                    return;
                }
                // Enough output to give the wheel somewhere to scroll to, then a marker that
                // says the shell is finished with it.
                let command = format!("seq 1 200; printf '{FILL_MARKER}\\n'\r");
                if let Err(error) = session.send(command.as_bytes()) {
                    eprintln!("sadna: probe command refused: {error:?}");
                }
                // The first half of the mouse claim, taken here because it is the only moment
                // the child provably has NOT asked for reporting. Recorded, never asserted on
                // its own: silence is what a dead mouse path and a correct one both look like,
                // and the second half is what tells them apart.
                smoke.click_before_enable = Some(click(session));
                self.enter(Stage::Filling);
            }

            Stage::Filling => {
                // The marker, never the last number: `200` appears in the middle of the count
                // and inside `1200` if the window is ever bigger, so it would report finished
                // while the child is still printing.
                if session.visible_text().contains(FILL_MARKER) || waited >= Self::PATIENCE {
                    sadna::clipboard::paste_text(session, PASTE_PROBE);
                    self.enter(Stage::Pasting);
                }
            }

            Stage::Pasting => {
                if !session.visible_text().contains(PASTE_PROBE) && waited < Self::PATIENCE {
                    return;
                }
                smoke.pasted = session.visible_text().contains(PASTE_PROBE);

                // `\x15` first: the pasted fixture is sitting on the prompt as text, and a
                // command typed after it would run one long nonsense word. Kill-whole-line is
                // what a person would press, and it is a KEY byte, not a paste - the paste
                // encoder strips it precisely so a pasted one cannot do this.
                //
                // Three things in one command, and `exec cat` is what makes all three hold.
                //
                // zsh re-reports its own directory from `precmd`, so the moment a command ends
                // the probe's value is replaced - measured, and it is why the first version of
                // this gate failed while the code was correct. Replacing the shell means there
                // is no `precmd` ever again: the directory stays put, the grid stays still (the
                // stillness the scroll check's control depends on), and `cat` is a child that
                // echoes whatever it receives - which is the only way a mouse report, whose
                // whole nature is to travel AWAY from us, can be seen at all. ECHOCTL draws the
                // escape as printable `^[`.
                let command = format!(
                    "\x15printf '\\033]7;file://localhost{CWD_PROBE}\\a\\033[?1000h\\033[?1006h{MOUSE_MARKER}\\n'; exec cat\r"
                );
                if let Err(error) = session.send(command.as_bytes()) {
                    eprintln!("sadna: cwd probe refused: {error:?}");
                }
                self.enter(Stage::Reporting);
            }

            Stage::Reporting => {
                if session.cwd() == Some(CWD_PROBE) || waited >= Self::PATIENCE {
                    self.enter(Stage::Steadying);
                }
            }

            Stage::Steadying => {
                if waited < Self::STILLNESS {
                    // Read on the way in, so the pair of readings brackets a window in which
                    // the host sends nothing at all.
                    if smoke.before_scroll.is_none() {
                        smoke.before_scroll = Some(session.visible_text());
                    }
                    return;
                }
                smoke.steady = Some(session.visible_text());
                // Forty rows, which is more than the window holds: a scroll smaller than the
                // viewport could land on identical-looking text in a column of numbers.
                session.scroll(40);
                self.enter(Stage::Scrolled);
            }

            Stage::Scrolled => {
                if waited < Self::STILLNESS {
                    return;
                }
                smoke.after_scroll = Some(session.visible_text());

                // Back to the live bottom BEFORE clicking. The viewport is parked 40 rows up in
                // history, and a child's answer lands at the bottom - so the echo this stage is
                // about would be encoded, written, received and simply off-screen, which reads
                // exactly like a mouse path that does not work.
                session.scroll(-40);
                smoke.click_after_enable = Some(click(session));
                self.enter(Stage::Clicking);
            }

            Stage::Clicking => {
                if session.visible_text().contains(MOUSE_REPORT) {
                    smoke.mouse_echo = true;
                    self.enter(Stage::Done);
                } else if waited >= Self::PATIENCE {
                    self.enter(Stage::Done);
                }
            }

            Stage::Done => {}
        }
    }
}

/// What the chrome is told. Mirrors `SessionState` in `chrome/src/protocol.ts`.
///
/// Deliberately incapable of carrying grid content, pixels or keystrokes: the law is expressed
/// as a vocabulary, so breaking it would require adding a field rather than forgetting a rule.
#[derive(Clone, serde::Serialize)]
struct SessionState {
    cols: u16,
    rows: u16,
    cwd: String,
    exited: bool,
}

/// Where the keyboard is pointed.
///
/// Tracked from mouse clicks rather than asked of AppKit, because the thing we need to know is
/// not "who is first responder" but "which of the two surfaces did the operator last aim at" -
/// and the webview covers a known strip, so a click's position answers it exactly.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    /// The pane a click last landed in. An index rather than a reference, because the canvas is
    /// behind a `RefCell` and holding anything borrowed from it across an event would deadlock
    /// the first time a handler wanted to present.
    Pane(usize),
    Chrome,
}

/// Which pane the chrome describes and whose background clears the window.
///
/// Falls back to pane 0 while the chrome holds focus rather than tracking the last terminal the
/// operator aimed at. That is a simplification and it is stated: with one canvas it means the
/// strip can name pane 0 while pane 1 was the last one typed into. B5 gives the strip a real
/// per-pane vocabulary and this goes away with it.
fn active_pane(focus: &Cell<Focus>, panes: usize) -> usize {
    match focus.get() {
        Focus::Pane(index) if index < panes => index,
        _ => 0,
    }
}

/// Every pane's rect and its own grid, in order - the canvas as it currently stands.
fn snapshot(canvas: &Canvas) -> Vec<(Rect, (u16, u16))> {
    canvas
        .panes()
        .iter()
        .map(|pane| {
            let geometry = pane.session.geometry();
            (pane.rect, (geometry.cols, geometry.rows))
        })
        .collect()
}

fn shell() -> Command {
    let path = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    Command::new(path)
}

/// The area the canvas tiles: the window minus the chrome strip, in PHYSICAL pixels.
///
/// The strip is subtracted exactly here, once. Everything downstream reads a rect, so nothing
/// else in the host has to know that a chrome exists - which is the same rule the Swift host's
/// `ChromeLayout` follows and for the same reason: a second place that reasons about the strip
/// is a second place that can disagree with the first.
fn canvas_area(width: u32, height: u32, strip: u32) -> Rect {
    Rect {
        x: 0,
        y: strip,
        width,
        height: height.saturating_sub(strip),
    }
}

fn main() {
    let app = tauri::Builder::default()
        .build(tauri::generate_context!())
        .expect("the tauri app");

    // The window is created HERE rather than in `setup`, and that is not a style choice: `setup`
    // requires a `Send` closure, and everything below - the session, the GPU context, the event
    // monitor - is main-thread-bound by construction. Building the window in main keeps it all
    // on one thread with no locks and no `Send` gymnastics around types that must never leave
    // this thread anyway.
    let window = WindowBuilder::new(&app, "main")
        .title("Sadna")
        .inner_size(900.0, 560.0)
        .visible(!headless())
        .build()
        .expect("a window");

    let scale = window.scale_factor().unwrap_or(1.0);
    let physical = window.inner_size().expect("the window size");
    let strip = ((BAR_HEIGHT + titlebar_inset(&window)) * scale) as u32;

    // ONE context for every pane AND for the window's swapchain. A render pass binds buffers
    // from a single device, so a pane that built its own could not be composited at all - see
    // `Session::spawn_on`, and the gate `every_pane_reaches_one_frame_at_its_own_rect`.
    let gpu = match GpuContext::new() {
        Ok(gpu) => gpu,
        Err(error) => {
            eprintln!("sadna: no GPU: {error}");
            std::process::exit(1);
        }
    };

    let specs = vec![PaneSpec::shell(); usize::from(GRID.rows * GRID.cols)];
    let canvas = match Canvas::spawn(
        &gpu,
        grid(scale),
        canvas_area(physical.width, physical.height, strip),
        &specs,
        FONT_SIZE * scale as f32,
        |_spec| shell(),
    ) {
        Ok(canvas) => canvas,
        Err(error) => {
            eprintln!("sadna: no canvas: {error:?}");
            std::process::exit(1);
        }
    };

    // SURFACE FIRST. The webview is added after this call and therefore lands above it; reversed,
    // the terminal covers the chrome and nothing errors. See the module card.
    //
    // The target's own origin stays ZERO and is never set: with a canvas each pane is placed by
    // its rect, and the strip is already inside those rects. Two sources for one number is how a
    // pane ends up offset by exactly the strip height.
    let mut target = match WindowTarget::from_window(
        &gpu,
        window.clone(),
        physical.width.max(1),
        physical.height.max(1),
    ) {
        Ok(target) => target,
        Err(error) => {
            eprintln!("sadna: no swapchain: {error}");
            std::process::exit(1);
        }
    };

    // What the app WOULD serve, asked of the resolver rather than inferred from a blank page.
    // This is the control for the title probe: if the bytes are here and the page is still
    // empty, the fault is in the navigation; if they are missing, the embed is the fault and no
    // amount of navigating would have helped.
    for candidate in ["index.html", "/index.html"] {
        match app.asset_resolver().get(candidate.to_string()) {
            Some(asset) => println!(
                "sadna: asset {candidate:?} -> {} bytes, mime {}",
                asset.bytes.len(),
                asset.mime_type
            ),
            None => println!("sadna: asset {candidate:?} -> MISSING"),
        }
    }

    // MEASURED 2026-08-04: a child webview built before the run loop starts reports
    // `url=<none>, progress=1, loading=false` six seconds later - it never navigated at all.
    // Not a load failure and not a script failure: no navigation was ever requested. Asking for
    // it explicitly is the fix, and it is cheap enough to keep unconditionally - navigating to
    // the page it is already on is a no-op.
    if let Some(chrome) = window.get_webview("chrome") {
        match "tauri://localhost/index.html".parse() {
            Ok(url) => {
                if let Err(error) = chrome.navigate(url) {
                    eprintln!("sadna: the chrome refused to navigate: {error}");
                }
            }
            Err(error) => eprintln!("sadna: bad chrome url: {error}"),
        }
    }

    for (index, pane) in canvas.panes().iter().enumerate() {
        let geometry = pane.session.geometry();
        let cell = pane.session.cell_metrics();
        println!(
            "sadna: pane {index} {}x{} cells at {}x{}px, rect {:?}, strip {strip}px, scale {scale}",
            geometry.cols, geometry.rows, cell.width, cell.height, pane.rect
        );
    }
    // Measured, not assumed. Every number here has already been wrong once in this project, and
    // each was found by a print contradicting a screenshot rather than by reading the code.
    println!(
        "sadna: inner {}x{} outer {:?} area {:?}",
        physical.width,
        physical.height,
        window.outer_size().ok(),
        canvas.area(),
    );
    if let Some(chrome) = window.get_webview("chrome") {
        println!(
            "sadna: chrome webview position {:?} size {:?}",
            chrome.position().ok(),
            chrome.size().ok()
        );
    } else {
        println!("sadna: NO chrome webview is registered on this window");
    }
    #[cfg(target_os = "macos")]
    view_tree(&window);

    // Shared between the AppKit key monitor and the run loop. Both are the main thread, so this
    // is an `Rc`, not an `Arc` behind a lock: introducing a mutex here would buy nothing and
    // would invite a deadlock the moment a key handler wanted to present.
    let canvas = Rc::new(RefCell::new(canvas));
    let focus = Rc::new(Cell::new(Focus::Pane(0)));

    // Never in headless mode: a local monitor makes the app eat keystrokes the moment it is
    // active, and an invisible window that steals the keyboard is the worst of both worlds.
    #[cfg(target_os = "macos")]
    accept_mouse_moved(&window);

    #[cfg(target_os = "macos")]
    // The split's ingredients are captured, not rebuilt: the SAME context every existing pane was
    // spawned on, and the font already multiplied by this display's scale. Two clones because the
    // gate splits too, and a context is reference-counted - both hand out the same device.
    let split_gpu = gpu.clone();
    let smoke_gpu = gpu.clone();
    let _monitor = (!headless()).then(|| {
        input::monitor(
            Rc::clone(&canvas),
            Rc::clone(&focus),
            BAR_HEIGHT,
            move || (split_gpu.clone(), shell(), FONT_SIZE * scale as f32),
        )
    });

    // Registered before the run loop: the chrome can announce itself the moment its script
    // runs, and a listener attached later would miss exactly the fast case.
    // The chrome announces itself, and the host REPLAYS the current state in answer.
    //
    // Without this the strip shows placeholders forever, and it is not a race that shows up in
    // testing: the host emits on frame one, the chrome's module loads a few hundred milliseconds
    // later, and the state only ever changes again if the window is resized. Measured here as
    // `text: "SADNA\n-\n-"` while the session was 94x27. The Swift host's panel bridge solved
    // the identical problem by replaying the latest message per kind; this is the same rule.
    let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let announced_ready = std::sync::Arc::clone(&ready);
    let replay = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let requested = std::sync::Arc::clone(&replay);
    app.listen("sadna://chrome-ready", move |event| {
        println!("sadna: chrome reported ready, payload {}", event.payload());
        requested.store(true, std::sync::atomic::Ordering::Relaxed);
        announced_ready.store(true, std::sync::atomic::Ordering::Relaxed);
    });

    let handle = app.handle().clone();
    let mut announced: Option<(u16, u16, bool, Option<String>)> = None;
    let mut frames: u32 = 0;
    let started = std::time::Instant::now();
    // `AppHandle::exit` REQUESTS an exit; the loop is still entered afterwards, and without this
    // flag the farewell printed a hundred times. A flag, not `process::exit`, because leaving
    // through the process would skip the pty host's teardown (SCAR-016).
    let mut finished = false;
    let mut probed_at: Option<std::time::Instant> = None;
    // When the headless resize was asked for; `None` until it has been.
    let mut resized_at: Option<std::time::Instant> = None;
    let mut ready_at: Option<std::time::Instant> = None;
    let smoke = std::rc::Rc::new(std::cell::RefCell::new(Smoke::default()));
    let mut probe = InputProbe::new();
    let page_finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // The chrome is attached from INSIDE the run loop, on the first frame.
    //
    // Measured 2026-08-04, and it cost four probes to see: a child webview created BEFORE
    // `app.run` never navigates at all (`url=<none>`), and forcing it to navigate then loads a
    // 178-byte document with no scripts instead of the 392-byte page the asset resolver holds -
    // the `tauri://` protocol is not serving that webview. Everything about it looks healthy
    // from Rust: `add_child` returns Ok, the view is placed correctly, `hidden=false`, and the
    // navigation reports Finished.
    let mut chrome_attached = false;

    app.run(move |_app, event| match event {
        RunEvent::MainEventsCleared => {
            if !chrome_attached {
                chrome_attached = true;
                let finished = std::sync::Arc::clone(&page_finished);
                if let Some(url) = attach_chrome(&window, finished) {
                    smoke.borrow_mut().chrome_url = Some(url);
                }
            }

            let mut canvas = canvas.borrow_mut();
            let mut drew = false;
            for pane in canvas.panes_mut() {
                // `|=`, never `||`: short-circuiting would stop polling the moment one pane had
                // something, and the panes after it would advance only when their neighbour was
                // quiet - which looks like a terminal that lags rather than one that is starved.
                drew |= pane.session.poll();

                // Drained every frame, not occasionally: the core's queue is bounded at 128 and
                // drops its OLDEST entry on overflow, so a host that never drains keeps the last
                // events of a long session and silently loses the first. `Session` folds the OSC 7
                // report into its own cwd on the way past; the rest - title, bell, clipboard,
                // progress - have no consumer in this host yet and are deliberately dropped here
                // rather than left to rot in the queue.
                let _ = pane.session.take_events();
            }

            if drew {
                // ONE acquire, N blits, ONE present. Presenting per pane would be N frames racing
                // to the display for a single tick, which reads as tearing rather than as a bug.
                // The clear is the active pane's own background: a window is almost never an exact
                // multiple of a cell, so the remainder is visible margin.
                let active = active_pane(&focus, canvas.panes().len());
                let clear = canvas.panes()[active].session.clear_color();
                // The rules are collected BEFORE the panes are borrowed mutably: `dividers`
                // reads the canvas, `panes_mut` takes it exclusively, and the borrow checker is
                // right to refuse the other order.
                let rule = divider_color(clear);
                let fills: Vec<Fill> = canvas
                    .dividers()
                    .into_iter()
                    .map(|rect| Fill {
                        x: rect.x,
                        y: rect.y,
                        width: rect.width,
                        height: rect.height,
                        color: rule,
                    })
                    .collect();
                let mut panes: Vec<(&mut GpuSurface, (u32, u32))> = canvas
                    .panes_mut()
                    .iter_mut()
                    .map(|pane| {
                        let rect = pane.rect;
                        (pane.session.surface_mut(), (rect.x, rect.y))
                    })
                    .collect();
                if let Err(error) = target.present_all(&mut panes, &fills, clear) {
                    // Loud and fatal. A present that fails and falls back is how B1's four
                    // defects stayed invisible for a day.
                    eprintln!("sadna: present failed: {error:?}");
                    handle.exit(1);
                }
            }

            // The chrome is told only when something it displays actually changed. Emitting every
            // frame would push 120 events a second through the IPC boundary to redraw the same
            // three strings.
            let active = active_pane(&focus, canvas.panes().len());
            let geometry = canvas.panes()[active].session.geometry();
            // ALL of them, not any: one shell exiting must not take the window with it and kill
            // the agents beside it. A dead pane keeps its last frame until B4 owns pane lifecycle.
            let exited = canvas
                .panes_mut()
                .iter_mut()
                .all(|pane| pane.session.exited());
            let cwd = canvas.panes()[active].session.cwd().map(str::to_string);
            let state = (geometry.cols, geometry.rows, exited, cwd.clone());
            if replay.swap(false, std::sync::atomic::Ordering::Relaxed) {
                // A fresh listener has nothing; forget what was already said so it is said again.
                announced = None;
            }
            if announced.as_ref() != Some(&state) {
                announced = Some(state);
                if let Err(error) = handle.emit(
                    SESSION_EVENT,
                    SessionState {
                        cols: geometry.cols,
                        rows: geometry.rows,
                        // Empty means "the child has not said", which is a different fact from
                        // any particular directory and is what the strip shows a dash for. The
                        // decode happened once, in `Session`, through the workspace's one
                        // decoder - never here.
                        cwd: cwd.clone().unwrap_or_default(),
                        exited,
                    },
                ) {
                    eprintln!("sadna: could not reach the chrome: {error}");
                } else {
                    println!(
                        "sadna: told the chrome {}x{} cwd={} exited={exited}",
                        geometry.cols,
                        geometry.rows,
                        cwd.as_deref().unwrap_or("-"),
                    );
                }
            }

            if exited {
                handle.exit(0);
            }

            // Headless runs are bounded from the INSIDE (SCAR-016): the process reaches its own
            // exit and its destructors run, rather than being killed by a timeout that would
            // skip the child's teardown.
            // Bounded by the CLOCK, not by a frame count. The first version counted 120 frames
            // and finished in well under a second, which is far too fast for a webview to fetch
            // and run a document - so "the chrome never reported ready" was partly a statement
            // about the harness. A budget has to outlast the thing it is waiting for.
            frames += 1;
            // Fired once, midway: `evaluateJavaScript` answers asynchronously on this same
            // thread, so a probe fired at the exit would print after the process is gone.
            // The probe is timed off the HANDSHAKE, not off the clock. Fired at a fixed
            // fraction of the budget it was flaky: sometimes the replayed state had not rendered
            // yet and the strip still showed a placeholder, so the gate failed for a reason that
            // had nothing to do with the code under test. A settle window after the chrome
            // announces itself is the event this actually depends on.
            if ready.load(std::sync::atomic::Ordering::Relaxed) && ready_at.is_none() {
                ready_at = Some(std::time::Instant::now());
            }
            // POLLED, not timed. A single read after a settle window was ~50% flaky: the
            // replayed state renders whenever React gets to it, and no fixed delay is the right
            // one. Re-reading until the document agrees (or the budget ends) removes the race
            // instead of widening it - and a probe that never agrees still fails, so this makes
            // the gate less flaky WITHOUT making it more forgiving.
            let settled = ready_at.is_some_and(|at| at.elapsed() >= SETTLE);

            // The input paths, driven only when nothing is on screen. The session's own view of
            // the child's directory is recorded every tick rather than at the end, because it
            // is the thing the document check below is waiting to SEE - and a value read after
            // the strip has already been probed proves nothing about the order.
            if headless() {
                let mut record = smoke.borrow_mut();
                // Pane 0 throughout: the probe drives ONE session on purpose, so every reading it
                // takes belongs to the same child. Appended only when it CHANGES, so the list is
                // the sequence of directories the child reported rather than one entry per frame.
                if let Some(cwd) = canvas.panes()[0].session.cwd() {
                    if record.cwd_seen.last().map(String::as_str) != Some(cwd) {
                        record.cwd_seen.push(cwd.to_string());
                    }
                }
                probe.advance(&mut canvas.panes_mut()[0].session, &mut record, settled);
            }

            // The resize path, driven LAST so it cannot disturb any reading above - it changes
            // every grid in the window, which would invalidate the scroll control and the strip
            // comparison at once. `set_size` on a window that is already ordered out costs no
            // screen and still travels through AppKit, so what comes back is a real `Resized`
            // event and the whole path under test (`target.resize`, `canvas_area`,
            // `Canvas::resize`) runs exactly as it does for a human dragging a corner.
            if headless() && probe.done() {
                let mut record = smoke.borrow_mut();
                if resized_at.is_none() {
                    record.before_resize = snapshot(&canvas);
                    let _ = window.set_size(LogicalSize::new(600.0, 380.0));
                    resized_at = Some(std::time::Instant::now());
                } else if record.after_resize.is_empty()
                    && resized_at.is_some_and(|at| at.elapsed() >= SETTLE)
                {
                    record.after_resize = snapshot(&canvas);
                    // ASKED of the window, never the number we requested: a window manager may
                    // give something else, and asserting against our own request would pass on a
                    // canvas that tiled a size the window never took.
                    record.resized_to = window.inner_size().ok().map(|size| size.width.max(1));
                }
            }

            // The document must show BOTH the grid and the directory before the run is over.
            // Two hops, one condition: the grid proves the replay path, the directory proves the
            // event path, and either can be dead while the other works.
            let agreed = {
                let record = smoke.borrow();
                let grid = record.grid.map(|(cols, rows)| format!("{cols}x{rows}"));
                let document = record.document.clone();
                grid.zip(document).is_some_and(|(grid, document)| {
                    document.contains(&grid)
                        && (!headless() || document.contains(CWD_PROBE))
                })
            };
            let probing = headless() || std::env::var("SADNA_PROBE").is_ok();
            if probing && settled && !agreed && probed_at.is_none_or(|at| at.elapsed() >= REPROBE)
            {
                probed_at = Some(std::time::Instant::now());
                // The window OPENED with this many panes, read before anything splits it. One is
                // the claim (Orel, 2026-08-06: like any other terminal), and it is recorded rather
                // than assumed from the constant, because a host that ignored `GRID` entirely
                // would still satisfy a check written against `GRID`.
                if smoke.borrow().panes_at_open.is_none() {
                    smoke.borrow_mut().panes_at_open = Some(canvas.panes().len());

                    // Then SPLIT, the way cmd+D does, and let every geometry invariant below run
                    // against the result. The split is driven through the canvas and never through
                    // a synthesized key press: putting a real cmd+D into the machine would type
                    // into whatever the operator is doing. What that leaves untested is the chord
                    // itself - the event mask and the keycode match - and that is a live tap, not
                    // a covered path (SCAR-014).
                    let (command, font) = (shell(), FONT_SIZE * scale as f32);
                    if let Err(error) = canvas.split(&smoke_gpu, command, font) {
                        eprintln!("sadna: smoke could not split: {error:?}");
                    }
                }

                {
                    let mut record = smoke.borrow_mut();
                    // Pane 0's rect, which is the number the blit actually uses. The window
                    // target's origin stays zero with a canvas, so reading it would check a value
                    // nothing consults and pass while every pane sat under the chrome.
                    let first = canvas.panes()[0].rect;
                    record.origin = Some((first.x, first.y));
                    record.grid = Some((geometry.cols, geometry.rows));
                    record.panes = snapshot(&canvas);
                    record.dividers = canvas.dividers();
                    record.gutter = Some(canvas.grid().gutter);
                    record.scale = window.scale_factor().ok();
                    record.reserved = Some(
                        ((BAR_HEIGHT + titlebar_inset(&window)) * window.scale_factor().unwrap_or(1.0))
                            as u32,
                    );
                }
                #[cfg(target_os = "macos")]
                probe_document(&window, std::rc::Rc::clone(&smoke));
            }
            // Ends EARLY once everything has been collected, and at the budget otherwise. A gate
            // that always burned its ceiling would make every future invariant cost the operator
            // wall-clock time, and the ones that fail are exactly the runs worth waiting out.
            let collected = probe.done()
                && agreed
                && (!headless() || !smoke.borrow().after_resize.is_empty());
            if headless() && !finished && (collected || started.elapsed() >= HEADLESS_BUDGET) {
                finished = true;
                println!(
                    "sadna: headless run complete after {frames} frames in {:?}",
                    started.elapsed()
                );
                #[cfg(target_os = "macos")]
                {
                    // Re-dumped at the END: the first dump runs before the chrome is attached,
                    // so it could only ever show half the answer.
                    smoke.borrow_mut().layers = view_tree(&window);
                    webview_state(&window);
                }
                // The sequence itself, printed as evidence: a check that reads "did this value
                // ever appear" is only auditable if the values it saw are on the screen too.
                println!(
                    "sadna: directories reported {:?}",
                    smoke.borrow().cwd_seen
                );
                let passed = smoke.borrow().judge(
                    page_finished.load(std::sync::atomic::Ordering::Relaxed),
                    ready.load(std::sync::atomic::Ordering::Relaxed),
                );
                println!("sadna: SMOKE {}", if passed { "PASSED" } else { "FAILED" });
                // The exit CODE is the gate's whole output to a script, and `AppHandle::exit`
                // does NOT carry it - measured: a FAILED smoke still exited 0, which would have
                // made this gate decoration the moment anything trusted it. The child is shut
                // down explicitly first, because leaving through the process skips the
                // destructor that reaps it (SCAR-016).
                canvas.shutdown();
                drop(canvas);
                std::process::exit(if passed { 0 } else { 1 });
                // `AppHandle::exit` requests an exit; it does not stop THIS iteration, and the
                // first version of this line printed its farewell a hundred times because the
                // loop kept being entered. Leaving through the process is what actually ends it,
                // and the session's child is reaped by the pty host's own teardown on the way.
            }
        }

        RunEvent::WindowEvent {
            event: tauri::WindowEvent::Resized(size),
            ..
        } => {
            let mut canvas = canvas.borrow_mut();
            let (width, height) = (size.width.max(1), size.height.max(1));
            let scale = window.scale_factor().unwrap_or(1.0);
            let strip = ((BAR_HEIGHT + titlebar_inset(&window)) * scale) as u32;

            target.resize(width, height);
            // One call re-tiles the canvas, resizes EVERY pty from its own new rect, and restates
            // every pane's mouse geometry - which has to happen or the encoder converts pixels to
            // cells against a stale view and the child acts on the column the pointer used to be
            // over. A canvas that resized only the rects would leave every child at its old size,
            // drawing under its neighbour and looking entirely healthy.
            if let Err(error) = canvas.resize(canvas_area(width, height, strip)) {
                // Reported, not fatal: a window dragged smaller than the grid can hold is the
                // operator asking for something impossible, and the last good tiling stays on
                // screen until they let go.
                eprintln!("sadna: canvas resize refused: {error:?}");
            }

            // The webview is a child view with no autoresizing mask; nothing moves it but this.
            let logical = size.to_logical::<f64>(scale);
            if let Some(chrome) = window.get_webview("chrome") {
                let inset = titlebar_inset(&window);
                let _ = chrome.set_size(LogicalSize::new(logical.width, BAR_HEIGHT));
                let _ = chrome.set_position(LogicalPosition::new(0.0, inset));
            }
        }

        _ => {}
    });
}

/// Reading the keyboard and the mouse from AppKit, because Tauri does not offer them.
#[cfg(target_os = "macos")]
mod input {
    use std::cell::{Cell, RefCell};
    use std::ptr::NonNull;
    use std::rc::Rc;

    use sadna::canvas::Canvas;
    use sadna::{clipboard, wheel};
    use ruuah_vt_render::GpuContext;
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags};
    use ruuah_vt_host::session::{MouseAction, MouseMods};
    use ruuah_vt_pty::key::{
        KEY_MODS_ALT, KEY_MODS_CTRL, KEY_MODS_SHIFT, KEY_MODS_SUPER, Key, KeyAction, KeyEvent,
        KeyMods, encode,
    };
    use ruuah_vt_pty::keycode::key_from_macos_keycode;

    use super::Focus;

    /// Whether to print what each input path saw. `SADNA_TRACE=1`.
    ///
    /// Off by default and not a debug leftover: the whole layer is invisible by construction -
    /// a key that does nothing, a click that does nothing and a path that was never reached
    /// look identical from outside, and the only alternative to this is guessing. Read once,
    /// because a `getenv` per mouse-moved event is a syscall per pixel of travel.
    fn tracing() -> bool {
        use std::sync::OnceLock;
        static ON: OnceLock<bool> = OnceLock::new();
        *ON.get_or_init(|| std::env::var("SADNA_TRACE").is_ok_and(|value| value == "1"))
    }

    /// Where an event landed, in the space the mouse encoder measures in.
    ///
    /// Returns surface-space PHYSICAL pixels from the window's top-left, whether the point is
    /// over the chrome strip, and the backing scale. Three conversions in one place because
    /// each of them has a wrong version that looks right:
    /// - AppKit's window space starts at the BOTTOM left, so the strip is the region with the
    ///   LARGEST y and the report's y is `height - point.y`, not `point.y`;
    /// - the delta is in POINTS and every size the renderer touches is a DEVICE pixel, so the
    ///   scale is not optional (project law, learned twice);
    /// - the title bar is INSIDE the content view here (B2.4), so the window's frame height is
    ///   the right height to subtract from.
    fn surface_point(
        event: &NSEvent,
        mtm: objc2::MainThreadMarker,
        bar_height: f64,
    ) -> Option<(f32, f32, bool, f64)> {
        let window = event.window(mtm)?;
        let point = event.locationInWindow();
        let height = window.frame().size.height;
        let scale = window.backingScaleFactor();
        let over_strip = point.y >= height - bar_height;
        let from_top = height - point.y;
        Some(((point.x * scale) as f32, (from_top * scale) as f32, over_strip, scale))
    }

    /// The protocol's button number for the event's button. 1 left, 2 middle, 3 right.
    ///
    /// Anything past those becomes 10, which the encoder names `Other`: it takes part in the
    /// held-button bookkeeping and produces no report, because the protocol's codes 4 and up
    /// mean the WHEEL - reporting a fourth physical button as code 4 would tell the child the
    /// view scrolled.
    fn button_code(event: &NSEvent) -> u32 {
        match event.buttonNumber() {
            0 => 1,
            1 => 3,
            2 => 2,
            _ => 10,
        }
    }

    fn mods_of(event: &NSEvent) -> MouseMods {
        let flags = event.modifierFlags();
        MouseMods {
            shift: flags.contains(NSEventModifierFlags::Shift),
            ctrl: flags.contains(NSEventModifierFlags::Control),
            alt: flags.contains(NSEventModifierFlags::Option),
        }
    }

    /// The pane under a window-space point, with the point translated into that pane's own space.
    ///
    /// Both halves or neither, and that is the whole trap of this layer: every pane's mouse
    /// geometry is its OWN RECT with no padding (`Canvas::spawn`), so a window-space coordinate
    /// handed to a pane reports a cell displaced by the pane's origin - down by the chrome strip
    /// for every pane, and right by half a window for the second one. Nothing errors. The child
    /// simply acts on a cell the operator never pointed at.
    fn pane_under(canvas: &Rc<RefCell<Canvas>>, x: f32, y: f32) -> Option<(usize, f32, f32)> {
        // Negatives are rejected rather than clamped: a point left of or above the window is
        // outside every pane, and clamping to zero would put it inside the first one.
        if x < 0.0 || y < 0.0 {
            return None;
        }
        let canvas = canvas.borrow();
        let index = canvas.pane_at(x as u32, y as u32)?;
        let (local_x, local_y) = local(&canvas, index, x, y)?;
        Some((index, local_x, local_y))
    }

    /// The same translation for a pane chosen by something other than the pointer - the captured
    /// pane, mid-drag, whose rect the pointer may already have left.
    fn local(canvas: &Canvas, index: usize, x: f32, y: f32) -> Option<(f32, f32)> {
        let rect = canvas.panes().get(index)?.rect;
        Some((x - rect.x as f32, y - rect.y as f32))
    }

    /// Hands one pointer event to a pane, in that pane's own coordinates. Silence is the common,
    /// correct outcome.
    fn report(
        canvas: &Rc<RefCell<Canvas>>,
        index: usize,
        event: &NSEvent,
        action: MouseAction,
        x: f32,
        y: f32,
    ) {
        let code = match action {
            // Motion with no button held is code 0. A DRAG is motion with its button, and the
            // encoder needs to know which one, so the two cannot share a constant.
            MouseAction::Motion if event.r#type() == objc2_app_kit::NSEventType::MouseMoved => 0,
            _ => button_code(event),
        };
        let mut canvas = canvas.borrow_mut();
        let Some(pane) = canvas.panes_mut().get_mut(index) else {
            return;
        };
        let mode = pane.session.frame().mouse_event();
        match pane.session.mouse(action, code, mods_of(event), x, y) {
            Ok(reported) => {
                if tracing() && (reported || action != MouseAction::Motion) {
                    println!(
                        "sadna: TRACE mouse {action:?} code={code} pane={index} at \
                         ({x:.0},{y:.0})px mode={mode:?} reported={reported}"
                    );
                }
            }
            Err(error) => eprintln!("sadna: mouse report refused: {error:?}"),
        }
    }

    /// Installs a LOCAL monitor: it sees this app's events before they are dispatched, and the
    /// value it returns decides their fate - the event to pass it on, null to swallow it.
    ///
    /// Local, never global: a global monitor would require Accessibility permission and would
    /// watch every application on the machine, which is an enormous ask for a terminal. The
    /// returned token must stay alive; dropping it removes the monitor and the keyboard goes
    /// quiet with nothing logged.
    /// `spawn` is what cmd+D puts in a new pane: the GPU context every pane must share, the
    /// command to run, and the already-scaled font size. Passed in rather than rebuilt here
    /// because a monitor that made its own context would produce panes that cannot be composited
    /// with the ones beside them - the B3.4 defect, one layer up.
    pub fn monitor(
        canvas: Rc<RefCell<Canvas>>,
        focus: Rc<Cell<Focus>>,
        bar_height: f64,
        spawn: impl Fn() -> (GpuContext, std::process::Command, f32) + 'static,
    ) -> Option<Retained<AnyObject>> {
        // Captured once: the monitor and its handler both run on the main thread, and proving
        // that here is cheaper than proving it at every AppKit call inside the block.
        let mtm = objc2::MainThreadMarker::new()?;

        // Lives as long as the monitor, because the REMAINDER is the state: a trackpad delivers
        // most of its deltas under one row, and an accumulator rebuilt per event rounds every
        // one of them to zero (`wheel::Accumulator`).
        let scroll = RefCell::new(wheel::Accumulator::default());

        // The pane that received the press owns the drag and the release, wherever the pointer
        // travels. Without capture, a drag out of pane 0 and a release over pane 1 leaves pane 0
        // holding a button forever - and the held-button bookkeeping lives inside each pane, so
        // the next bare motion over it reports a drag nobody is performing.
        let captured: Cell<Option<usize>> = Cell::new(None);

        let handler = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            let event = unsafe { event.as_ref() };
            let pass = event as *const NSEvent as *mut NSEvent;

            match event.r#type() {
                objc2_app_kit::NSEventType::LeftMouseDown
                | objc2_app_kit::NSEventType::RightMouseDown
                | objc2_app_kit::NSEventType::OtherMouseDown => {
                    // AppKit's window coordinates start at the BOTTOM left, so the strip - which
                    // is at the top - is the region with the LARGEST y. Getting this backwards
                    // would put focus exactly where it does not belong and look like the click
                    // being ignored.
                    let Some((x, y, over_strip, _)) = surface_point(event, mtm, bar_height) else {
                        if tracing() {
                            println!("sadna: TRACE mouse down with NO window on the event");
                        }
                        return pass;
                    };
                    if tracing() {
                        println!(
                            "sadna: TRACE mouse down at ({x:.0},{y:.0})px over_strip={over_strip}"
                        );
                    }
                    let under = (!over_strip)
                        .then(|| pane_under(&canvas, x, y))
                        .flatten();
                    if event.r#type() == objc2_app_kit::NSEventType::LeftMouseDown {
                        focus.set(match under {
                            Some((index, _, _)) => Focus::Pane(index),
                            None => Focus::Chrome,
                        });
                    }
                    if let Some((index, local_x, local_y)) = under {
                        captured.set(Some(index));
                        report(&canvas, index, event, MouseAction::Press, local_x, local_y);
                    }
                    // Passed on regardless. The window still needs the event for its own
                    // business - dragging by the title bar, the traffic lights, the webview's
                    // own clicks - and swallowing a mouse-down is how a window stops being
                    // movable with nothing in the log to say why.
                    pass
                }

                objc2_app_kit::NSEventType::LeftMouseUp
                | objc2_app_kit::NSEventType::RightMouseUp
                | objc2_app_kit::NSEventType::OtherMouseUp => {
                    // A RELEASE goes to the pane that took the press, wherever the pointer ended
                    // up - over the strip, over a neighbour, outside the window. That asymmetry
                    // is the point: the held-button bookkeeping lives inside that pane, so a
                    // release delivered anywhere else leaves it held forever and the next motion
                    // reports a drag nobody is performing.
                    if let Some((x, y, _, _)) = surface_point(event, mtm, bar_height)
                        && let Some(index) = captured.take()
                    {
                        // The translation is taken in its OWN statement so the shared borrow is
                        // released before `report` takes a mutable one. Inside the `if let` chain
                        // the guard would live to the end of the block and every release would
                        // panic on an already-borrowed RefCell.
                        let point = local(&canvas.borrow(), index, x, y);
                        if let Some((local_x, local_y)) = point {
                            report(&canvas, index, event, MouseAction::Release, local_x, local_y);
                        }
                    }
                    pass
                }

                objc2_app_kit::NSEventType::LeftMouseDragged
                | objc2_app_kit::NSEventType::RightMouseDragged
                | objc2_app_kit::NSEventType::OtherMouseDragged
                | objc2_app_kit::NSEventType::MouseMoved => {
                    if let Some((x, y, over_strip, _)) = surface_point(event, mtm, bar_height)
                        && !over_strip
                    {
                        // A drag belongs to the pane that captured it; a bare move belongs to the
                        // pane under the pointer. Sending a drag to whatever is underneath would
                        // report a selection sweep to a program that never saw the button go down.
                        let routed = match captured.get() {
                            Some(index) => {
                                let point = local(&canvas.borrow(), index, x, y);
                                point.map(|(local_x, local_y)| (index, local_x, local_y))
                            }
                            None => pane_under(&canvas, x, y),
                        };
                        if let Some((index, local_x, local_y)) = routed {
                            report(&canvas, index, event, MouseAction::Motion, local_x, local_y);
                        }
                    }
                    pass
                }

                // The wheel is aimed by the POINTER, not by focus: a scroll over the terminal
                // scrolls the terminal even when the operator last clicked in the chrome, which
                // is what every other application does and what the hand expects.
                objc2_app_kit::NSEventType::ScrollWheel => {
                    let Some((x, y, over_strip, scale)) = surface_point(event, mtm, bar_height)
                    else {
                        return pass;
                    };
                    if over_strip {
                        return pass;
                    }
                    // The pointer picks the pane, and a scroll over no pane at all is nobody's.
                    let Some((index, local_x, local_y)) = pane_under(&canvas, x, y) else {
                        return pass;
                    };

                    let mut canvas = canvas.borrow_mut();
                    let Some(pane) = canvas.panes_mut().get_mut(index) else {
                        return pass;
                    };
                    // One accumulator for every pane, which is correct only while they share a
                    // font size - they do, the host builds them all at `FONT_SIZE * scale`. A
                    // per-pane font would need a per-pane remainder, or a slow trackpad scroll
                    // would round to zero in one pane and not in its neighbour.
                    let rows = scroll.borrow_mut().rows(
                        event.scrollingDeltaY(),
                        event.hasPreciseScrollingDeltas(),
                        pane.session.cell_metrics().height,
                        scale,
                    );
                    if rows == 0 {
                        return std::ptr::null_mut();
                    }
                    // The CHILD is asked first, and it gets the whole wheel or none of it: a
                    // program that captured the mouse must not also have the view scrolled out
                    // from under it, and a pager relying on alternate scroll must not have its
                    // arrows duplicated by a viewport move. `Ok(false)` is the session saying
                    // the wheel is ours.
                    match pane.session.wheel(local_x, local_y, rows, mods_of(event)) {
                        Ok(true) => {}
                        Ok(false) => pane.session.scroll(rows),
                        Err(error) => eprintln!("sadna: wheel refused: {error:?}"),
                    }
                    std::ptr::null_mut()
                }

                objc2_app_kit::NSEventType::KeyDown => {
                    // The keyboard goes to the pane the operator last clicked in, and to nothing
                    // at all while the chrome holds it - the webview handles its own keys, and a
                    // terminal that also took them would type into both.
                    let Focus::Pane(index) = focus.get() else {
                        return pass;
                    };
                    let flags = event.modifierFlags();
                    // Command chords belong to the menu - cmd+Q, cmd+C - and a terminal that
                    // swallows them is a terminal you cannot quit. cmd+V is the ONE exception,
                    // and it has to be: the menu's Paste reaches the first responder, and the
                    // terminal is a GPU layer with no responder to reach. Nothing errors when
                    // this is missing - the chord simply does nothing, forever.
                    //
                    // Matched on the KEYCODE, never on the character: under the Hebrew layout
                    // cmd+V carries `ה`, so a match on the text is dead by construction. Same
                    // rule as the chrome's `e.code` finding (B2.2), one layer down.
                    if flags.contains(NSEventModifierFlags::Command) {
                        let chord = key_from_macos_keycode(event.keyCode());
                        let plain = !flags.intersects(
                            NSEventModifierFlags::Control
                                | NSEventModifierFlags::Option
                                | NSEventModifierFlags::Shift,
                        );
                        if tracing() {
                            println!(
                                "sadna: TRACE cmd chord keycode={} key={chord:?} plain={plain} pane={index}",
                                event.keyCode(),
                            );
                        }
                        // cmd+D splits the focused pane to the right, as Ghostty does. It is the
                        // second chord this host has to claim for itself, and for the same reason
                        // as cmd+V: there is no first responder to route a menu item to.
                        //
                        // A refused split is REPORTED and the canvas is left alone - pressing it
                        // in a window too narrow for another terminal does nothing visible, and a
                        // silent nothing is indistinguishable from a dead key binding.
                        if chord == Key::D && plain {
                            let mut canvas = canvas.borrow_mut();
                            let (gpu, command, font) = spawn();
                            match canvas.split(&gpu, command, font) {
                                Ok(index) => {
                                    // Focus follows the new pane, which is what every terminal
                                    // does and what makes the chord usable twice in a row.
                                    focus.set(Focus::Pane(index));
                                    if tracing() {
                                        println!("sadna: TRACE split into pane {index}");
                                    }
                                }
                                Err(error) => {
                                    eprintln!("sadna: cannot split: {error:?}");
                                }
                            }
                            return std::ptr::null_mut();
                        }

                        if chord == Key::V && plain {
                            let text = clipboard::text();
                            let canvas = canvas.borrow();
                            let Some(pane) = canvas.panes().get(index) else {
                                return pass;
                            };
                            if tracing() {
                                println!(
                                    "sadna: TRACE clipboard holds {:?} chars, bracketed={}",
                                    text.as_ref().map(|value| value.chars().count()),
                                    pane.session.bracketed_paste(),
                                );
                            }
                            match text {
                                Some(text) => clipboard::paste_text(&pane.session, &text),
                                None => eprintln!("sadna: the clipboard holds no text"),
                            }
                            return std::ptr::null_mut();
                        }
                        return pass;
                    }

                    let key = key_from_macos_keycode(event.keyCode());
                    let text = event.characters().map(|value| value.to_string()).unwrap_or_default();
                    let unshifted = key.codepoint().unwrap_or(0);

                    let mut mods: KeyMods = 0;
                    if flags.contains(NSEventModifierFlags::Shift) {
                        mods |= KEY_MODS_SHIFT;
                    }
                    if flags.contains(NSEventModifierFlags::Control) {
                        mods |= KEY_MODS_CTRL;
                    }
                    if flags.contains(NSEventModifierFlags::Option) {
                        mods |= KEY_MODS_ALT;
                    }
                    if flags.contains(NSEventModifierFlags::Command) {
                        mods |= KEY_MODS_SUPER;
                    }

                    // Shift is CONSUMED when the layout used it to make the text; reporting it
                    // twice turns shift+a into a modified 'A' instead of an 'A'. Compared against
                    // the unshifted codepoint rather than assumed, because a layout may disagree
                    // (on the Hebrew layout shift+t is a different letter entirely).
                    let mut consumed: KeyMods = 0;
                    if !text.is_empty()
                        && mods & KEY_MODS_SHIFT != 0
                        && text.chars().next().map(u32::from) != Some(unshifted)
                    {
                        consumed |= KEY_MODS_SHIFT;
                    }

                    let canvas = canvas.borrow();
                    let Some(pane) = canvas.panes().get(index) else {
                        return pass;
                    };
                    let bytes = encode(
                        &KeyEvent {
                            action: KeyAction::Press,
                            key,
                            mods,
                            consumed_mods: consumed,
                            composing: false,
                            utf8: &text,
                            unshifted_codepoint: unshifted,
                        },
                        &pane.session.key_options(),
                    );
                    if bytes.is_empty() {
                        // Nothing to send is not the same as nothing to do: passing the event on
                        // keeps chords the terminal has no encoding for working elsewhere.
                        return pass;
                    }
                    if let Err(error) = pane.session.send(&bytes) {
                        eprintln!("sadna: send failed: {error:?}");
                    }
                    std::ptr::null_mut()
                }

                _ => pass,
            }
        });

        unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                NSEventMask::KeyDown
                    | NSEventMask::LeftMouseDown
                    | NSEventMask::LeftMouseUp
                    | NSEventMask::RightMouseDown
                    | NSEventMask::RightMouseUp
                    | NSEventMask::OtherMouseDown
                    | NSEventMask::OtherMouseUp
                    | NSEventMask::LeftMouseDragged
                    | NSEventMask::RightMouseDragged
                    | NSEventMask::OtherMouseDragged
                    | NSEventMask::MouseMoved
                    | NSEventMask::ScrollWheel,
                &handler,
            )
        }
    }
}

/// Asks the window for mouse-moved events, which it does NOT deliver by default.
///
/// `acceptsMouseMovedEvents` is NO on a fresh NSWindow, and the consequence is invisible: a
/// child in mode 1003 (report ALL motion) receives clicks and drags perfectly and never hears
/// about a bare move, so a menu that highlights under the cursor simply never highlights. No
/// error, no log, and the half that works makes the half that does not look like the program's
/// fault (SCAR-004: a guard you cannot see fire).
#[cfg(target_os = "macos")]
fn accept_mouse_moved(window: &tauri::Window) {
    use objc2::runtime::AnyObject;

    let Ok(handle) = window.ns_window() else {
        return;
    };
    let ns_window: *mut AnyObject = handle.cast();
    if ns_window.is_null() {
        return;
    }
    // Safety: the NSWindow Tauri just created, alive for as long as this window, not retained.
    unsafe {
        let _: () = objc2::msg_send![ns_window, setAcceptsMouseMovedEvents: true];
    }
}

/// Prints the window's real AppKit view hierarchy.
///
/// The question "where did the webview go" is not answerable from Rust types - Tauri reports a
/// webview it believes exists while AppKit decides what is actually drawn, and those two can
/// disagree. This asks AppKit.
#[cfg(target_os = "macos")]
fn view_tree(window: &tauri::Window) -> Vec<String> {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::NSView;

    let mut layers = Vec::new();
    let Ok(handle) = window.ns_window() else {
        println!("sadna: no ns_window to inspect");
        return layers;
    };
    let ns_window: *mut AnyObject = handle.cast();
    if ns_window.is_null() {
        println!("sadna: ns_window is null");
        return layers;
    }

    // Safety: `ns_window` is the NSWindow Tauri just created for this window; it is alive for as
    // long as the window is, and nothing here retains it past this call.
    unsafe {
        let content: Option<Retained<NSView>> = objc2::msg_send![ns_window, contentView];
        let Some(content) = content else {
            println!("sadna: window has no contentView");
            return layers;
        };
        let frame = content.frame();
        println!(
            "sadna: contentView {}x{}",
            frame.size.width, frame.size.height
        );
        // Z-ORDER, measured instead of looked at. A layer's position in its superlayer's
        // `sublayers` array IS its stacking order - later means on top - so this answers "is the
        // chrome above the terminal" with no screen involved. wgpu's `CAMetalLayer` and the
        // webview's layer are the two that matter.
        let layer: Option<Retained<AnyObject>> = objc2::msg_send![&*content, layer];
        if let Some(layer) = layer {
            let class: Retained<objc2_foundation::NSString> =
                objc2::msg_send![&*layer, description];
            println!("sadna: contentView.layer {}", class.to_string());
            let sublayers: Option<Retained<objc2_foundation::NSArray>> =
                objc2::msg_send![&*layer, sublayers];
            match sublayers {
                Some(sublayers) => {
                    let count: usize = objc2::msg_send![&*sublayers, count];
                    for index in 0..count {
                        let sublayer: Retained<AnyObject> =
                            objc2::msg_send![&*sublayers, objectAtIndex: index];
                        let name: Retained<objc2_foundation::NSString> =
                            objc2::msg_send![&*sublayer, className];
                        println!("sadna:   layer[{index}] {} (later = on top)", name.to_string());
                        layers.push(name.to_string());
                    }
                }
                None => println!("sadna: contentView.layer has no sublayers"),
            }
        }

        for (index, view) in content.subviews().iter().enumerate() {
            let frame = view.frame();
            let class = view.class().name().to_string_lossy().into_owned();
            println!(
                "sadna:   subview[{index}] {class} at ({}, {}) {}x{} hidden={}",
                frame.origin.x,
                frame.origin.y,
                frame.size.width,
                frame.size.height,
                view.isHidden(),
            );
        }
    }
    layers
}

/// The height of the window's title bar, in POINTS.
///
/// Tauri's content view spans the WHOLE window, title bar included, so point zero is behind the
/// traffic lights rather than below them. Placing the chrome at zero puts it under the title bar
/// where about eight points of it show - which reads exactly like "the terminal overlaps the top
/// bar", and is what Orel saw on first sight. Asked of AppKit rather than hardcoded to 28,
/// because the answer changes with the title bar style and with the system.
#[cfg(target_os = "macos")]
fn titlebar_inset(window: &tauri::Window) -> f64 {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::NSView;
    use objc2_foundation::NSRect;

    let Ok(handle) = window.ns_window() else {
        return 0.0;
    };
    let ns_window: *mut AnyObject = handle.cast();
    if ns_window.is_null() {
        return 0.0;
    }

    // Safety: read-only geometry on the live NSWindow Tauri owns.
    unsafe {
        let content: Option<Retained<NSView>> = objc2::msg_send![ns_window, contentView];
        let Some(content) = content else { return 0.0 };
        // `contentLayoutRect` is the part of the content view NOT covered by the title bar.
        // The difference between it and the view is the inset, whatever the style.
        let layout: NSRect = objc2::msg_send![ns_window, contentLayoutRect];
        let frame: NSRect = objc2::msg_send![ns_window, frame];
        let inset = (content.frame().size.height - layout.size.height).max(0.0);
        // Printed because the derived number was wrong once and the gate could not see it: an
        // inset of 0 and a correct inset both satisfy "origin clears the strip".
        // Printed only when something is measuring: the derived number was wrong once and the
        // gate could not see it, so the raw inputs stay available - but not on every launch.
        if headless() || std::env::var("SADNA_PROBE").is_ok() {
            println!(
                "sadna: window frame h={} contentView h={} contentLayoutRect h={} inset={inset}",
                frame.size.height,
                content.frame().size.height,
                layout.size.height,
            );
        }
        inset
    }
}

#[cfg(not(target_os = "macos"))]
fn titlebar_inset(_window: &tauri::Window) -> f64 {
    0.0
}

/// Docks the chrome webview across the top of the window.
///
/// Called on the first frame rather than before the loop; see `chrome_attached`.
fn attach_chrome(
    window: &tauri::Window,
    finished: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Option<String> {
    let Ok(physical) = window.inner_size() else {
        eprintln!("sadna: no window size, no chrome");
        return None;
    };
    let scale = window.scale_factor().unwrap_or(1.0);
    let logical = physical.to_logical::<f64>(scale);
    let inset = titlebar_inset(window);

    match window.add_child(
        // NOT `transparent(true)`: on macOS that is gated behind Tauri's `macos-private-api`
        // feature, and private Apple API in an AGPL app people run is a bad trade for a
        // see-through bar. The strip is opaque and its background matches the terminal's, so the
        // seam does not flash before the page paints.
        WebviewBuilder::new("chrome", WebviewUrl::App("index.html".into()))
            .background_color(tauri::webview::Color(0x10, 0x10, 0x16, 0xff))
            .on_page_load(move |webview, payload| {
                println!(
                    "sadna: chrome page {:?} url={}",
                    payload.event(),
                    webview.url().map(|url| url.to_string()).unwrap_or_default()
                );
                if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                    finished.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }),
        LogicalPosition::new(0.0, inset),
        LogicalSize::new(logical.width, BAR_HEIGHT),
    ) {
        Ok(chrome) => {
            let url = chrome.url().map(|url| url.to_string()).ok();
            println!("sadna: chrome attached, url={url:?} size={:?}", chrome.size().ok());
            url
        }
        Err(error) => {
            eprintln!("sadna: the chrome webview refused to attach: {error}");
            None
        }
    }
}

/// Runs JS inside the chrome and prints what the DOCUMENT says about itself.
///
/// The instrument of last resort, and the only one that cannot be fooled by a wrong selector:
/// `title` read through Objective-C came back empty while the asset resolver held a valid 392
/// byte document and the navigation reported Finished. One of those three was lying, and asking
/// the DOM directly is what tells us which.
#[cfg(target_os = "macos")]
fn probe_document(window: &tauri::Window, smoke: std::rc::Rc<std::cell::RefCell<Smoke>>) {
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::NSView;
    use objc2_foundation::NSString;

    let Ok(handle) = window.ns_window() else { return };
    let ns_window: *mut AnyObject = handle.cast();
    if ns_window.is_null() {
        return;
    }

    // Safety: read-only selectors on a live NSWindow and its WKWebView subview; the completion
    // block is retained by WebKit for the duration of the call.
    unsafe {
        let content: Option<Retained<NSView>> = objc2::msg_send![ns_window, contentView];
        let Some(content) = content else { return };
        for view in content.subviews().iter() {
            let responds: bool =
                objc2::msg_send![&**view, respondsToSelector: objc2::sel!(evaluateJavaScript:completionHandler:)];
            if !responds {
                continue;
            }
            let script = NSString::from_str(
                "JSON.stringify({title: document.title, html: document.documentElement.outerHTML.length, root: !!document.getElementById('root'), scripts: document.scripts.length, text: document.body.innerText})",
            );
            let record = std::rc::Rc::clone(&smoke);
            let handler = RcBlock::new(move |value: *mut AnyObject, error: *mut AnyObject| {
                if !value.is_null() {
                    let text: Retained<NSString> = objc2::msg_send![value, description];
                    let text = text.to_string();
                    println!("sadna: document says {text}");
                    record.borrow_mut().document = Some(text);
                }
                if !error.is_null() {
                    let text: Retained<NSString> = objc2::msg_send![error, description];
                    println!("sadna: document probe failed: {}", text.to_string());
                }
            });
            let _: () = objc2::msg_send![&**view, evaluateJavaScript: &*script, completionHandler: &*handler];
        }
    }
}

/// Asks the real `WKWebView` what it thinks it is doing.
///
/// The Rust side reports a webview that exists and is placed correctly while the page never
/// runs, and those two facts are consistent with three different bugs. WebKit's own `URL` and
/// `estimatedProgress` separate them: no URL means loading never started, a URL with progress
/// below 1 means it started and stalled, and a completed load with no handshake means the
/// document ran and the SCRIPT is the problem.
#[cfg(target_os = "macos")]
fn webview_state(window: &tauri::Window) {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::NSView;
    use objc2_foundation::NSString;

    let Ok(handle) = window.ns_window() else {
        return;
    };
    let ns_window: *mut AnyObject = handle.cast();
    if ns_window.is_null() {
        return;
    }

    // Safety: the NSWindow is alive for the life of the Tauri window, and every selector below
    // is read-only. `estimatedProgress` and `URL` are WKWebView's; the subview is one.
    unsafe {
        let content: Option<Retained<NSView>> = objc2::msg_send![ns_window, contentView];
        let Some(content) = content else { return };
        for view in content.subviews().iter() {
            let responds: bool =
                objc2::msg_send![&**view, respondsToSelector: objc2::sel!(estimatedProgress)];
            if !responds {
                continue;
            }
            let progress: f64 = objc2::msg_send![&**view, estimatedProgress];
            let url: Option<Retained<AnyObject>> = objc2::msg_send![&**view, URL];
            let described = match url {
                Some(url) => {
                    let text: Retained<NSString> = objc2::msg_send![&*url, absoluteString];
                    text.to_string()
                }
                None => "<none>".to_string(),
            };
            let loading: bool = objc2::msg_send![&**view, isLoading];
            let title: Option<Retained<NSString>> = objc2::msg_send![&**view, title];
            let title = title.map(|value| value.to_string()).unwrap_or_default();
            println!(
                "sadna: WKWebView url={described} progress={progress} loading={loading} title={title:?}"
            );
        }
    }
}
