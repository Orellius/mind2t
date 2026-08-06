# CLAUDE.md - mind2t (project child config)

> **Parent stack layer:** `../CLAUDE.md` (tools/ 2026 stack, auto-inherited; don't restate it).
> Chain: `~/.claude/CLAUDE.md` (contract) → `Studio/CLAUDE.md` (index) → `tools/CLAUDE.md` (stack) → **this file (mind2t specifics)**.
> This file = project specifics only. Last update stamp: 2026-08-06 (IDT).
> Posture: the global proactive co-pilot rule (initiative, three-steps-ahead, extreme ownership) is in force here via `~/.claude/CLAUDE.md`.

## What this is NOW - Mind2t (decided 2026-08-04, Orel; renamed twice, 2026-08-06)

**Naming history, and it is two renames in one day, both Orel's call.** The product was born
"Bindary" (2026-08-04), became **Sadna** (סדנה, workshop) on 2026-08-06 on a
pronunciation criterion, and became **MIND2T** the same day - directory, GitHub repo, product
crate, bundle identifier, config directory, chrome, gate script and the icon's filenames, end
to end. The plan file keeps its historical name (`2026-08-04-bindary.md`) because it is dated
provenance; the `B1..B9` slice namespace is unchanged.

The rebrand cost is recorded rather than waved through, because the plan file claimed it was
zero: the crate is `crates/mind2t` and the binary is `mind2t`, so **`~/.mind2t` is the config
directory and `~/.ruuah` remains the only fallback** (`~/.sadna` never existed on disk and is
deliberately not in the chain). A directory rename also invalidates Tauri's generated
permissions paths under `target/` - see the `cargo clean` gotcha below.

This repo builds **two things, and the names are not interchangeable**:

- **`ruuah-vt`** - the engine. The Rust VT core, pty, renderer and C ABI. Keeps its name
  forever; other people may embed it.
- **Mind2t** - the product built on that engine. An **AGPL, cross-platform agent workbench**:
  a fleet of coding-agent CLIs in real terminals, in git worktrees, with sessions that do not
  end because a context governor binds each one to the next.

**Plan of record: `docs/plans/2026-08-04-bindary.md`.** Read it before starting any slice.
Mind2t slices are **`B1..B9`** and are a SEPARATE namespace from the `S1..S9` app slices in
`docs/APP-BACKLOG-2026.md`. Never mix them.

The wedge, in one line: **we own the VT core, the pty and the renderer**, so agent state comes
from a typed grid (`Session.rowText(row, semantic:)`) instead of regexing ANSI out of a byte
stream. BridgeSpace rents xterm.js, Termic rents xterm.js, Claude Squad rents tmux. Evidence:
`~/Desktop/Studio/docs/research/bridgespace-teardown/`.

Three laws added by this decision:
1. **The Swift host is the ORACLE, not the corpse.** Do not delete it. Port the Tauri host to
   parity against it first, the same way the GPU backend is trusted because a CPU reference can
   disagree with it.
2. **No terminal bytes in the webview** - not pixels, not keystrokes, not frames. Ever.
3. **One wgpu surface for all panes.** Pane count is a Rust-side fact; if the webview needs to
   know it, the design is wrong.

## What this is (engine)

A terminal core in Rust implementing the C ABI Ghostty already publishes as
**`libghostty-vt`**, so it can drop in behind an existing native GUI. The point is control
and craft, not speed - Rust and Zig are peers here and there is no performance win waiting.

**There is no consumer yet, and the note that used to sit here was wrong.** Measured
2026-07-28: RUUAH's Swift app calls **99 `ghostty_*` symbols and not one of them is a VT-core
symbol** -- they are all app-level (`ghostty_app_*`, `ghostty_surface_*`, `ghostty_config_*`,
`ghostty_inspector_*`). Ghostty's own app never routes through `src/terminal/c/` either; that
layer is reached only by `src/lib_vt.zig`, which exists for external embedders. So "RUUAH swaps
one link flag" was never true: swapping ruuah-vt in behind RUUAH's existing Swift app would
mean reimplementing the *application* ABI, which is a different project.

What is true, and what slice 6 makes testable: ruuah-vt can export the same C ABI
`libghostty-vt` publishes, so **something built against that ABI** can link either. The planned
consumer is a minimal Swift host (after slice 6), not RUUAH as it stands.

**The one architectural rule: the core is a pure, deterministic state machine.** Bytes in,
grid mutations out. No PTY, no GPU, no clock, no I/O. Everything else hangs off this,
because it is what makes headless CI and differential testing possible at all. Ghostty
enforces the same split physically (`src/terminal/` knows nothing about `src/renderer/`).

Plan of record: `~/.claude/plans/2026-07-28-rust-terminal-core.md`.
Architecture research it came from: `~/Desktop/claude-html/terminal-architecture-20260728-0132.html`.

## Status / current slice

**D2b step 2 (the gesture), D5 and D6 DONE 2026-08-06, main `871db03`.** Mind2t can now be
copied out of, zoomed, and clicked through. Gates: **688 tests / difftest 223/223 / smoke 26
of 26**. Next by the parity plan: **D1 scrollback search**, which has NO oracle and whose gate
will be a reference implementation plus mutants - weaker than the corpus, and it has to be
said out loud every time it is cited.

- **`Frame::viewport_rows` is the seam, and it is the whole design.** It builds
  `ruuah_vt_snapshot::Row`s from packed cells so `core::selection` - the module gated against
  libghostty-vt on 15 corpus cases - runs unchanged on what is on screen. The alternative was
  re-deriving the word rules in the host, which would have been a second copy of a boundary
  set with no oracle behind it. `crates/frame/tests/selection_rows.rs` runs every probe
  against BOTH the frame's rows and the core's own and demands they agree, so a bridge that
  blanked cells or transposed coordinates is caught by disagreement rather than by an
  assertion someone had to predict.
- **The coordinate seam D2a called dangerous does not exist on this path.** The probe is fed
  VIEWPORT rows, so its answer is already viewport-relative and `FrameSelection` needs no
  conversion. The cost, stated: a selection cannot reach into scrollback the way
  `Terminal::select` can, because scrollback is not in a frame. cmd+A is therefore
  select-all-VISIBLE, and the oracle's `All` is not.
- **`set_selection` forces a full repaint, and that is not incidental.** A selection changes no
  terminal state, so the frame's generation does not move, so `poll` returns `false` - the
  highlight would appear only the next time the child happened to write. It also has to be a
  FULL repaint: a partial one touches only rows the child dirtied, and the row just
  deselected is not one of them.
- **cmd+C is conditional on purpose.** With nothing selected it falls through to the child as
  `^C`. A terminal that swallowed it unconditionally would stop being able to interrupt a
  running command the day it learned to copy.
- **Shift takes the pointer back from the child.** With mouse reporting on, `vim` and an agent
  CLI own every click, so without this there is no way to copy a line off the screen at all.
  The report is SKIPPED rather than sent-and-also-selected: sending it would have the program
  act on a click meant for the host.
- **Font size moves every pane together.** The wheel accumulator is shared and is correct only
  while panes agree on a cell height; a per-pane font would make a slow trackpad scroll round
  to zero in one pane and not its neighbour. Steps are multiplicative (ten percent) because
  this host stores font sizes already scaled, so a raw `+1.0` would move half a point on a 2x
  display and a whole one on a 1x.
- **The gate's `exec cat` shapes what the fixture can be.** The mouse stage replaces the shell,
  so after it there is no shell to run a `printf` and interrupting `cat` ends the session -
  both measured, in that order. `cat` echoes what it receives, so the selection fixture is
  sent as plain text. The fixture is `alpha-beta` because `-` is NOT a word boundary in the
  oracle's rules: a host that re-derived them answers `alpha`, one that selected the row
  answers the row, one with no selection answers nothing. Three distinguishable wrong answers.
- **What the gate cannot see, and it is most of the gesture**: the real drag, AppKit's own
  click counting (and therefore the machine's double-click interval), the pasteboard write,
  and cmd+click. All of it is live-tap debt in SCAR-014's exact shape - the demo works when
  the harness drives bytes and dies when a human drives the window.


**D2b step 1, the selection is DRAWN, `[tested]` by pixels 2026-08-06.** `Frame` carries a
`FrameSelection` and the renderer tints it. Gates: **681 tests / difftest 223/223 / smoke 23 of
23**. The GESTURE half - mouse drag, double and triple click, cmd+C - is **the next step and is
not built**; nothing sets `frame.selection` outside a test yet.

- **The blind spot was total, again.** D2a measured the range and the clipboard text against the
  oracle and put no pixel anywhere. Until `crates/render/tests/selection.rs` existed, a
  `draw_selection` that returned immediately - or tinted the wrong row, or every row - passed
  `redraw.rs`, `caret.rs`, `vim.rs` and the whole corpus, because each of them either sets no
  selection or compares two renders that are wrong the same way. Mutant seen red: the renderer
  ignoring `frame.selection` reports `[]` where the test demands `[2,3,4,5]`.
- **Measured positionally, in `caret.rs`'s shape**: paint the same frame twice, with and without
  a selection, and the columns whose pixels DIFFER are the highlighted columns. That answers
  "where is it", where a whole-canvas comparison answers "is it the same as some other render",
  which is the wrong question.
- **The tint is BLENDED OVER the finished row, not painted as a background**, and that is not
  cosmetic. Backgrounds are painted per cell interleaved with ink, so a selection painted as one
  would be erased by the next cell's background and would be invisible wherever the child
  coloured anything - a prompt, a diff, an `ls`. The cost, stated: the ink under the tint shifts
  colour slightly, where Ghostty swaps both foreground and background and keeps its contrast
  exactly. `text_survives_under_the_tint` is the guard that keeps it a highlight and not a
  redaction.
- **The selection colour is neutral grey, never a palette entry.** All sixteen are colours a
  program may paint text in, so tinting with one would make text of that colour vanish into its
  own highlight.
- **`FrameSelection` is VIEWPORT-relative; `core::selection` answers in ABSOLUTE rows** counting
  from the top of scrollback. Whoever sets the field owns the conversion, and getting it wrong
  paints the highlight one screen away from the pointer. This is the seam the gesture step lands
  on next.
- Endpoints keep the gesture's order (a drag upward has `start` after `end`) so the host knows
  which end the pointer holds; every reader goes through `ordered`.

**The pane's child environment, `[tested]` 2026-08-06. "No CLI can run inside Mind2t" was
real, and it was this host diverging from its own oracle.** `shell_from` built the child as
`Command::new($SHELL)` with no arguments and no environment, while the C ABI host
(`crates/host/src/lib.rs`) had declared `TERM`, passed `-il` and scrubbed the Claude session
markers since slice 8. Nothing compared the two. Gate is now **23 invariants**.

- **The gate had to be poisoned before it could measure anything.** `scripts/smoke-mind2t.sh`
  now launches the host through `env PATH=/usr/bin:/bin:/usr/sbin:/sbin TERM=dumb
  CLAUDECODE=smoke-poison ...`, because the run inherits the operator's own terminal: three of
  the four new checks PASSED against a host that declared nothing at all, since the child was
  handed a perfectly good environment by accident. The case being stood in for is a **Finder
  launch, which inherits nothing** - and that is exactly the launch nobody tests.
- **Two of my own diagnoses were wrong and the gate is what said so**, which is the whole
  reason it goes first:
  1. I wrote that the shell was not INTERACTIVE and that `.zshrc` never ran. It is and it did:
     POSIX makes a shell with no operands and a terminal on stdin interactive, and a pty is a
     terminal. The check passed against the broken host. What is genuinely missing is
     **LOGIN** - `/etc/zprofile` (path_helper) and `~/.zprofile` (homebrew's shellenv), which
     is the half that empties `PATH`.
  2. The login check then failed against the FIXED host, because the probe used `[ -o login ]`
     - zsh's single-bracket `test` rejects `-o` with "too many arguments", so it reported `no`
     for every shell alive. `[[ -o login ]]` is the form. The probe was the broken half.
- **`TERM` is DECLARED (`CHILD_TERM`), never passed through**, plus `COLORTERM=truecolor`. An
  app opened from the Dock inherits no environment, so an inheriting host hands the child an
  empty `TERM`, no terminfo entry is found and every ncurses program exits before drawing a
  cell. That is what "no CLI can run" looks like from outside the pty.
- **A terminal window is a session boundary**: `CLAUDECODE` and `CLAUDE_CODE_CHILD_SESSION`
  are scrubbed, so an agent CLI opened in a pane is a fresh session rather than a child of
  whatever launched Mind2t. Found live 2026-07-29, fixed in the C host, never carried here.
- Seen red both ways: reverting `-l`, the `TERM` declaration and the scrub makes the gate print
  `term dumb | login no | cc smoke-poison` and fail three checks; the fix makes it print
  `term xterm-256color | login yes | cc none`. **The gate prints the values it read**, because a
  failing environment check is unreadable without them.
- **The CLI check does NOT discriminate under the gate, and the code says so.** It reports `cli
  yes` even against the fully reverted host, because the pane's shell is interactive whatever
  the host does and `.zshrc` puts the tool back on `PATH`. It can only fail in the Finder case
  it stands in for. The three beside it are what actually proved the fix (SCAR-004: a check that
  cannot be seen to fail is not evidence).
- **Two harness bugs, both found by the gate failing on a run where only the app icon had
  changed** - and both would have made it lie rather than error:
  1. `visible_text().contains(FILL_MARKER)` matched the **echoed command line**. An interactive
     shell echoes what it was typed, so the marker was on screen before the child ran anything,
     the stage advanced instantly, and all four checks failed against a correct host. The
     environment report is now the completion signal, and `%s` is what tells a real line from
     the echoed format string.
  2. The report was one line carrying four fields, and **a pane during the gate is about 46
     columns** because the window is split. It wrapped; the capture read `cc=` and nothing after
     it. One field per line now.

**The app mark was redrawn, 2026-08-06, and Orel picked it** (`assets/icon/mind2t.svg` is the
source; `mind2t-1024.png` and `crates/mind2t/icons/icon.png` are generated from it with
`rsvg-convert` - regenerate BOTH after any edit, nothing does it automatically). A
cabinet-maker's screwdriver on the diagonal, with the RTL prompt `_<` on the counter-diagonal.

- **Colours are the terminal's own**, measured from `crates/render/src/color.rs`: #0d0d0d
  ground, #f0c674 handle (palette index 3), steel cool and desaturated so it separates without
  adding a third hue. The prompt is built from those same two materials rather than new ones.
  The mark it replaced was a blue-grey plate with an amber caret, belonging to no theme this
  terminal ships.
- **The tool is a bezier silhouette, not stacked rectangles.** Four rounded rects rotated
  together is what the first three attempts were and it is why they read as a toy: a driver's
  handle swells past the ferrule and flares at the butt, and that curve is the whole reason the
  object is recognisable. Proportion is the other half - handle about 2.4:1, shaft plus blade
  longer than the handle. A fat handle over a short shaft read as food at 32.
- **One light source, upper left**, held across handle crescent, shaft specular and ferrule
  bevel. Disagreeing highlights are what make an icon feel wrong before anyone can name why.
- **Judge every icon change at 32px.** Four versions died there while looking fine at 1024: ס
  drawn as a ring read as a record button; a chevron and cursor 44 units apart fused into one
  blob; a screwdriver assembled from rects lost its blade entirely; and the prompt tucked 22
  units off the plate edge read as falling out of the icon. `rsvg-convert -w 32` then
  `magick -filter point -resize 900%` is how to see it.
- **Two elements is the ceiling**, and only with real separation - 74 units between cursor and
  chevron here, and the prompt placed where the tool never crosses.

**D2a the selection MODEL, `[tested]` differentially 2026-08-06.** Word, line and select-all
ranges plus the clipboard text they format to, in `crates/core/src/selection.rs`, agreeing
with libghostty-vt on all 15 corpus cases. Gates: **675 tests / difftest 223/223 / smoke 19 of
19**. The GUI half - click-drag, double and triple click, cmd+C, and drawing the highlight -
is **D2b and is not built**; nothing in the app can select anything yet.

- **D2 was taken before D1 on measured evidence.** The oracle publishes a whole
  `selection.h` (1061 lines, 14 entry points) and **no search API at all**, so selection gets
  the differential gate that has caught every defect in this project for ten slices and search
  cannot. Selection is also what search needs to highlight a hit with, so the order is free.
- **The harness went first, and its blind spot here was total**: `Snapshot` could not
  represent a selected range, so a `select_word` returning the whole screen would have scored
  a perfect MATCH on every case in the corpus. `Snapshot` gained `selections`, `Case` gained
  `select` probes, and the 12 new cases were pinned as `diff` against a `Terminal::select`
  that answered nothing - **seen DIFF before a line of selection was written**, then promoted
  to `match` by the implementation. That promotion is the evidence.
- **A selection is a QUERY, not state.** No byte stream produces one, which is why it is the
  first thing in this corpus a case has to ASK for, and why probes run last on both sides -
  the oracle's refs are untracked snapshots that the next mutating call invalidates.
- **The probe point is ACTIVE-relative and the answer is ABSOLUTE**, counting from the top of
  scrollback. Nothing in the corpus could see that seam until
  `select-word-with-scrollback-above` was added: every other selection case runs with an empty
  history, where the two spaces coincide exactly. A candidate that drops the offset reports a
  word from the wrong line and passes everything else.
- **The oracle's own doc comment is WRONG and its code is the contract.** `selectWord` says "a
  word is exclusively whitespace or exclusively non-whitespace"; it actually tests membership
  in a boundary SET, and `.`, `/`, `-` and `_` are not in it. So a path, a filename and a flag
  select whole - which is the most useful thing double-click does in a terminal - while `:`,
  `;`, `(`, `)`, `$` and `│` do bound. Ported verbatim into `Rules`, then confirmed by
  measurement (`foo.bar` selects whole).
- **Nine mutants run, six killed outright, and the three survivors are recorded rather than
  quietly dropped** - two are now closed and the third stays as a correction:
  1. A forward scan that drops the last cell of a hard line survived the ENTIRE corpus,
     because no case had a word ending at the last column. `select-word-filling-a-whole-row`
     (5 columns, `abcde`) closes it. Without that cell, every word that fills a line loses its
     last character.
  2. Treating a wide cell's spacer tail as text survived, because this core writes a tail with
     no codepoint, so the emptiness test already answered correctly and the `SpacerTail` clause
     was doing nothing. Closed by a unit control that builds a tail carrying text, which the
     grid never produces and a hand-built `Row` can.
  3. Swapping the two guards in the BACKWARD word scan changed nothing, because both of them
     break - so their order cannot affect the answer. The comment claiming that asymmetry was
     load-bearing was wrong and now says so. The FORWARD asymmetry is real and is pinned by
     case 1 above.
- Six killed: `.` added to the boundary set, line selection not following a soft wrap upward,
  line selection joining upward across a hard break, the formatter keeping trailing blanks,
  the formatter emitting a newline across a soft wrap, and the history offset dropped.
- **RENAMING THE DIRECTORY REQUIRES A FULL `cargo clean`, AND THE NARROW ONE IS A TRAP I FELL
  INTO TWICE.** Measured on both renames of 2026-08-06.
  - First rename: `cargo test --workspace` failed in Tauri's build script on a generated
    permissions path baked with the OLD absolute directory; 2,819 files under `target/` held
    it. That produced the note "`cargo clean -p tauri -p <product>` is the fix", which was
    **wrong** - it described the first symptom, not the cause.
  - Second rename, with that narrow clean applied: `ruuah-vt-abi --test differential` failed
    with `the corpus is committed: NotFound`. **`CARGO_MANIFEST_DIR` is baked at COMPILE time**,
    so any cached artifact still resolves paths against the old directory. Seven files in this
    workspace bake it: `difftest/src/case.rs`, `abi/tests/differential.rs`,
    `frame/tests/bidi_conformance.rs`, `pty/tests/keycode.rs`, `pty/tests/esctest.rs`,
    `host/tests/shell_integration.rs`, `ghostty/build.rs`.
  - **The tell that nearly cost the finding**: the failing test PASSED when re-run standalone
    (that invocation rebuilt it) and failed again under `--workspace` (which reused a different
    cached binary). "It passes now" was available and would have been false.
  - The fix is a bare `cargo clean` and a full rebuild. It cost 53.7 GiB of artifacts and a few
    minutes. Gates re-run from the new path afterwards: 688 / 223 of 223 / smoke 26 of 26.
  - The errors name a MISSING FILE, never a stale path, so nothing in the message points at the
    rename. If a gate fails right after the directory moves, clean before diagnosing.

**B3.6 one window, panes on demand, `[tested]` headlessly 2026-08-06 (`b3-5-divider`).** Orel's
call: the window opens with ONE pane like any other terminal, and **cmd+D splits it to the right**,
as Ghostty does. The pre-split 1x2 canvas was B3.4 scaffolding and read as a window already in use.
Gate is now **19 invariants** - it opens with one pane, splits, and every geometry check runs
against the result.

- **`Canvas::split` validates before it mutates.** The new session is spawned and every new rect
  measured against the same `fit` the panes will use from then on; only after that do the existing
  panes move. The order is the point - the alternative resizes everyone, discovers the last cell is
  one column wide, and leaves the operator with a smaller canvas and no new pane. A refused split
  leaves the canvas exactly as it was.
- **Re-tiling goes through `resize`, not by hand**, so a split and a window resize cannot disagree
  about how a pane moves. One path updates rects, ptys and mouse geometry.
- **Single-row canvases only** (`CanvasError::NotSplittable`). Adding a column to a two-row grid
  adds TWO panes and renumbers every existing one, which is not what a key press asked for. Refused
  rather than approximated until a split tree exists; pinned by a test.
- **The gate drives `Canvas::split`, never a synthesized cmd+D** - a real chord would type into
  whatever the operator is doing. The chord itself (event mask, keycode match) is **live-tap debt**,
  the same boundary as every other AppKit path here (SCAR-014). It is matched on the KEYCODE, so
  the Hebrew layout cannot kill it.
- Mutant seen red: a split that adds a pane without resizing the existing one - "pane 0 still
  claims 180 columns after a split - it is drawing under its neighbour".

**B3.5 the pane divider, `[tested]` headlessly 2026-08-06 (`b3-5-divider`).** Two terminals no
longer read as one surface: the layout reserves a gutter between panes and a solid rule is painted
into exactly that gap, in the same render pass. Gates: workspace suite green, difftest 207/207,
`scripts/smoke-mind2t.sh` **17 of 17** (the new one is the rule in the live window).

- **The gutter is taken OUT of the panes, never painted over them.** The alternative covers a
  column the child is writing into, and a terminal whose last column sits under a rule looks like
  a program that truncates its own output. Panes genuinely have less room, their ptys are told so,
  and `each_pane_tells_its_child_its_own_size` now subtracts the rule's cost rather than demanding
  the full width - the honest half of the choice.
- **"Let the clear colour show through the gap" was designed and REJECTED before it was built.**
  It costs no renderer code at all, and it is wrong: a pane's surface is whole CELLS, so up to a
  cell of margin inside every pane's rect is already uncovered and shows the clear colour. Making
  the clear the divider colour would paint a grey band down every pane's right edge and along its
  bottom. The rule is drawn explicitly instead - `Fill` and a second pipeline in `present.rs`,
  recorded into the SAME pass, because a second pass is a second command buffer and this backend
  has already deadlocked once on Metal's 64-buffer pool.
- **One arithmetic, two outputs.** `edge()` is a free function shared by `tile` and `dividers`, so
  the rule cannot land beside the gap the panes gave up. The coverage map now counts panes AND
  dividers together: a pane-only map is satisfied by panes that leave a gap, which is what a gutter
  IS, so it could no longer tell a reserved gutter from a lost pixel.
- **The map found a real defect on first run**: an area narrower than its own gutters collapsed
  every cell to zero and left the rules claiming pixels past the canvas edge. Dividers are clipped
  to the area, and the degenerate case still tiles exactly.
- **A mutant that PASSED is recorded next to the ones that failed** (`present.rs` solid shader):
  dropping the "above or left of the rect" guard changes nothing, because an underflowed unsigned
  coordinate is enormous and the extent check discards it anyway. The pane path's identical comment
  claims otherwise and is wrong for the same reason - flagged, not rewritten. The comparison that
  IS load-bearing is `>=` versus `>`, seen red as "the rule bled into the right pane".
- Four mutants seen red: panes ignoring the gutter offset (double coverage), the extent off-by-one,
  the gutter measured in points instead of physical pixels (the scale trap, caught by the live
  gate), and a gutter reserved with nothing drawn into it.
- **The LOOK is `[untested - needs your eyes]`.** `divider_color` lifts a dark background and drops
  a light one by a fixed amount so the rule follows the theme; the weight is a defensible default,
  not a verdict. No window has been put on screen (standing order).

**B4.2 an agent launches into a pane and is SEEN from the grid, `[tested]` 2026-08-05
(`b4-agent-registry`).** `crates/mind2t/src/launch.rs`: spawn fitted, observe, retry with a
doubling backoff. **A real Claude Code CLI ran in a pane and its own interface was read back with
`Session::visible_text()`** - banner, `Sonnet 5 · Claude Max`, and the status line carrying model,
cwd and git branch, with no regex and no ANSI parsing anywhere. That is the wedge demonstrated
rather than argued: model, directory, branch and mode are already text on the grid. Verdict
`Running` on the first attempt in 1.22s. Live tap: `scripts/agent-live-tap.sh`.

- **`Running` requires wrote AND still alive**, and the exit check comes FIRST. A CLI that prints
  its usage and quits is `Exited { wrote: true }`, not a success. "Did anything appear on the
  grid" gets that case wrong and it is the common one for a bad argv. A 250ms settle after first
  output closes the microsecond race between a write and the exit behind it.
- **The retry loop is counted, not claimed.** Every attempt records its verdict and the wait
  before it; the test asserts the backoffs doubled AND that the wall clock really elapsed, since a
  loop that ran once ends identically to one that ran three times (SCAR-004).
- **A refusal never retries.** An approval bypass fails once, immediately. Three attempts would
  read as flakiness rather than as policy.
- **The real-agent test is `#[ignore]`d on purpose**, so the suite never starts authenticated
  agent processes on this machine. `scripts/agent-live-tap.sh` is how it is run deliberately, and
  the script exists partly because writing that command in prose trips the firewall's M5 arm.
- Fixture trap, cost one red run: `exec cat < /dev/null; sleep 30` does NOT express "alive but
  mute". The exec replaces the shell, cat hits EOF at once, the sleep never runs. `exec sleep` is
  the honest fixture.

**B4.1 the agent registry and the auto-approve guard, `[tested]` 2026-08-05
(`b4-agent-registry`).** `crates/mind2t/src/agent.rs`: ten agent CLIs with the fields that
actually differ (binary candidates, prompt strategy, spawn grace, resume template), a PATH probe
with the asymmetric cache (5 min hit / 10 s miss), and the guard that refuses to auto-type an
approval bypass. Nothing spawns yet - B4.2 puts an agent in a pane and verifies it from the typed
grid. Five of the ten are installed here: claude, codex, gemini, opencode, grok. Two gotchas
below carry the findings.

**B3.4 the host is a CANVAS, `[tested]` headlessly 2026-08-05 (`b3-4-host-canvas`).** Mind2t's
window no longer holds one terminal. It holds a `Canvas` - a wizard-shaped grid (hardcoded 1x2
until B5 declares one), one live session per cell, all of them presented in a single swapchain
frame by `present_all`. Gates: workspace suite green, `scripts/smoke-mind2t.sh` **16 of 16**.

- **One GPU context for the whole canvas.** See the gotcha below; a pane that owned its device
  could not be composited at all, and no test that never presents can see it.
- **Coordinates are PANE-LOCAL past `Canvas::pane_at`.** Each pane's mouse geometry is its own
  rect with zero padding, so a window-space point handed to a pane reports a cell displaced by
  that pane's origin - down by the strip for all of them, right by half a window for the second.
- **The press CAPTURES the pane.** Drag and release go to the pane that took the press, wherever
  the pointer travels, because the held-button bookkeeping lives inside that pane and a release
  delivered elsewhere leaves it held forever.
- **The strip is subtracted exactly once**, in `canvas_area`. Every pane's position is its rect,
  so `WindowTarget`'s own origin stays zero and `present_all` ignores it.
- What is NOT proven: the window's LOOK. Two shells side by side is `[untested - needs your
  eyes]` - no window has been put on screen (standing order), and the byte-level proof is the
  offscreen composite in `crates/mind2t/tests/canvas.rs`.

**S5.5 workspace sidebar, `[tested]` 2026-08-02 (`s55-workspace-sidebar`).** The tab
strip's right-hand button was decoration copied 1:1 from the Warp reference; it now
docks a sidebar listing every worktree, the sessions open in each, and which one is
active. Clicking a workspace focuses its session or opens one there. Off unless
`panels = true`.

- **Docked, not floating.** The sidebar takes width FROM the terminal pane, and
  everything downstream follows for free: `gridForPane` derives cols and rows from
  `view.bounds`, so shrinking the view is what resizes the pty and no geometry math
  learns that sidebars exist.
- **`ChromeLayout` is pure arithmetic** (`Window.swift`), extracted so the dock is
  assertable without NSViews. The invariant is EXACT TILING - pane and sidebar sum to
  the content width, no overlap and no gap - because the failure here is silent: a pane
  that keeps full width simply draws under the sidebar and looks normal with its right
  columns covered. `--smoke-dock` asserts it; the mutant (sidebar as a constant width
  instead of a remainder) fails on the narrow-window case.
- **One document, many panels.** `init` names the panel kind, so a new panel never adds
  a bundle to build, sign or ship. `Root.tsx` owns the handshake and the bridge replays
  the latest message per kind, which is what makes mount order irrelevant: the root
  sends `ready`, the host answers `init` plus data, and the panel mounts only after the
  kind is known - so its data provably arrives before it exists.
- The live tap caught a contradiction no test would: the primary row rendered as
  ACTIVE while claiming no session, because sessions in the primary tree carry no
  workspace label. The primary row now claims the unlabelled ones.

Gates: **605 tests + 6 web / difftest 207/207 / esctest 373 pinned / exports 14 + 52 /
five smokes (base, worktree, dock, panel, panel-control)**. Sidebar aesthetics are
`[untested - needs your eyes]`.

**S5 workspaces v1, `[tested]` headlessly and by live tap, 2026-08-02
(`s5-worktree-workspaces`).** "New Workspace" in the palette creates a git
worktree beside the repository and opens a session inside it; the tab pill shows
`⎇ branch`. "Close Workspace" closes the session and offers to remove the tree.

- **`RuuahHostOptions.cwd`** places the child. Applied LAST so it beats the home
  default. It sets where the child STARTS: a configured command containing its
  own `cd` still wins, which is correct precedence and cost one live tap to see.
- **`Worktrees.swift` is the only file in the app that mutates a repository**,
  and `remove` NEVER passes `--force`. git's refusal to delete a worktree with
  uncommitted changes is the feature; it is surfaced verbatim, never retried.
- **Worktrees are siblings** at `<repo>-worktrees/<branch>`, never nested inside
  the repository (which would pollute the parent's `git status` forever).
- Mutants seen red both times: dropping `current_dir` made the child report the
  test's own directory, and adding `--force` made `--smoke-worktree` fail on the
  dirty-tree assertion.

Gates: **605 tests / difftest 207/207 / esctest 373 pinned / exports 14 + 52 /
four smokes (base, worktree, panel, panel-control)**. The two modals (create
prompt, close confirmation) are `[untested - needs your eyes]`; the tap drives
the path past them, not the dialogs.

**S6 web panels, `[tested]` through the C surface and the bridge, 2026-08-02
(`s6-web-panel`).** The app grew its first WKWebView panel: a React + TS diff
review card (changed files, unified diff with both line-number gutters) over the
active session's repository, opened with cmd+shift+D or the palette's "Review
Changes". **Off unless `panels = true` in `config.toml`.**

The architectural line, and the reason this is not a Tauri rewrite. Orel asked
about wrapping RUUAH the way `simion/termic` wraps a terminal (Tauri 2 + React 19
+ **xterm.js**, AGPL-3.0). xterm.js carries its OWN VT parser and screen model
and consumes bytes rather than grids, so putting it in front of this core runs
two terminal emulators and bypasses ours entirely; 604 tests, difftest and
esctest would all be measuring code nothing calls. Blitting our RGBA into a
canvas instead is worse (1120x700 at 2x is 12.5 MB a frame). So: **the terminal
surface stays native, and only document-shaped panels get a browser engine.** No
terminal pixels, no pty-bound keystrokes and no frame data cross into a webview.

Rules this slice fixed in place:

- **Exactly one percent-decoder.** Swift needed the session's cwd to run git in
  it, and the decode already existed in Rust, so it got a C export
  (`ruuah_cwd_path`, reusing `cwd::normalize`) rather than a second
  implementation in another language. Exports 50 to 52.
- **The panel document is one self-contained file** (`vite-plugin-singlefile`),
  loaded from `file://` with a navigation policy that refuses everything else.
  Nothing resolves subresources, so the read-access grant has no blast radius.
- **The advertised file list is the allowed set.** A `requestDiff` for a path the
  host never sent is refused instead of handed to git.
- **`Git.swift` never mutates a repository**, and that boundary is deliberate.
- **git runs off the main thread** and both pipes are drained before `waitUntilExit`;
  draining after would deadlock past the 64 KiB pipe buffer, which any real diff
  passes at once.

Two defects the harness caught on first contact, both of the silent kind:

- **The control passed VACUOUSLY on run one.** `--smoke-panel-control` loads the
  same document with `window.__ruuahReceive` stripped and must not answer the
  probe. It "passed" while a navigation-policy bug (URL equality against a
  symlink-resolved path, `/tmp` vs `/private/tmp`) was refusing the document
  outright, so no nonce came back for a reason that had nothing to do with the
  bridge. The control now also demands the document loaded and mounted.
- **`didFinish` is not "the script ran".** The bundle is an ES module and module
  scripts are deferred, so WebKit reports the navigation finished before
  `window.__ruuahReceive` exists, and delivering to it throws "undefined is not a
  function". The queued messages now flush on the panel's own `ready`, which
  cannot arrive before its bridge module by construction.

Gates: **604 tests / difftest 207/207 / esctest 373 pinned / exports 14 + 52 /
Swift smoke / panel bridge smoke + control**. The panel's LOOK is
`[untested - needs your eyes]` (SCAR-014: aesthetics are Orel's verdict).


**cwd-keyed ghost history, 2026-07-31 (`s4-cwd-history`).** History entries carry the
directory they ran in; a suggestion PREFERS a match from the current directory and falls
back to the newest match anywhere (fish's rule -- requiring the directory would make the
ghost vanish the moment you `cd`). The shell integration now emits OSC 7 itself, because
nothing else does in our windows: macOS installs `update_terminal_cwd` only when
`$TERM_PROGRAM` is `Apple_Terminal`, which we never set. The host normalizes the URI, so
the raw report crosses the C surface untouched and exactly one place knows how to decode
percent-escapes. Old history files load unchanged (a line with no tab is a command with no
directory -- the pre-cwd format).

Two traps worth keeping: `path` is a SPECIAL variable in zsh (tied to `$PATH` as an
array), so the first encoder reported `file://host%2F` for every directory; and the
emitter/decoder pair is pinned by a test carrying the exact bytes a real zsh produced,
because they are two implementations in two languages that nothing else compares.
`[tested]` through the C surface with a real zsh and its control (an unintegrated shell
reports nothing); the Swift pass-through is `[untested - needs your eyes]`.

**images-v3, `[tested]` 2026-07-31, in two slices.**

**v3a, z-index** (`images-v3a-zindex`): three layers -- under the cell background, under
the text, over everything -- sorted by `(z, image id)` IN THE PUBLISHER, because the host
resolves image pixels positionally and a renderer that re-sorted would pair placements
with the wrong bytes. z rides free bits in the placement word pair, so the channel did not
grow. The row pass had to split (`Parts`): `draw_row` painted background and ink together
per cell, and an image under the text lands between them. The interleaved path stays the
default -- splitting also stops a later cell's background erasing the previous glyph's
overhang, which is defensible but not what the existing pixel tests were written against.
A frame carrying a below-text placement **repaints wholly**, because such an image spans
rows the damage tracker has no reason to call stale; that keeps incremental-equals-full
true rather than quietly false.

**v3b, unicode placeholders** (`images-v3b-placeholders`): `U=1` registers a VIRTUAL
placement that draws nothing, and U+10EEEE cells in the grid address it -- image id in the
foreground colour, row and column in the combining marks. The point is structural: the
image IS the text, so it scrolls, reflows and erases with no anchor to keep in step.
`frame/src/placeholder.rs` ports the oracle's `graphics_unicode.zig` (297-entry diacritic
table, `canAppend` run rules); the renderer crops per run, so a run showing the middle of a
scrolled image draws the middle of it rather than the top.

Gates: **541 tests / difftest 164/164 / 14 + 49 exports / Swift smoke**. Mutants seen red:
publisher not sorting, every placement claiming the top layer, the background skip removed,
and the crop offset ignored. Three z-order tests failed FIRST on my own bugs (a sized
placement steps the cursor past itself, so back-to-back placements never overlapped) and
the placeholder C-surface test failed on a UTF-8 byte computed in my head rather than
measured -- U+10EEEE is `f4 8e bb ae`, and the wrong byte decodes to a real codepoint, so
nothing errored and the test simply found nothing.

**OSC 7 working directory, `[tested]` 2026-07-31 (`osc7-cwd`).** The core tracks the
pwd the child reports: stored RAW and undecoded, terminal-global, cleared by an empty
report and by RIS but not by DECSTR. It has a real oracle -- the ABI answers
`GHOSTTY_TERMINAL_DATA_PWD` -- so unlike OSC 8 and OSC 52 the corpus can pin it, and
ours answers the same enum value because a drop-in that cannot is not a drop-in.
Harness went first as always: `Snapshot` had no pwd at all, so a core ignoring OSC 7
scored a perfect MATCH on every case that reports one.

Two rules that reading the source alone gets WRONG, both binary-searched against the
real library rather than inferred: `reportPwd`'s 4096-byte truncation is unreachable
dead code for OSC 7 (the parser captures into a fixed `[2048]u8`, so the cliff is 2047
bytes stored whole), and past that cliff the command is DROPPED rather than truncated,
leaving any previous pwd untouched. Our vte is built with `std`, where `osc_raw` is an
unbounded `Vec`, so the core has to enforce that limit itself.

The probe found a defect next door: the oracle routes **OSC 9;9** (ConEmu CurrentDir)
to the same pwd, while this core fell through to the notification branch and popped a
desktop notification reading `9;/Users/orel/src`. Fixed and pinned in the same slice.
**OSC 1337 CurrentDir is measured, unimplemented, and named** in `docs/BACKLOG-2026.md`.

Host seam: event kind 7 carries the raw report (empty = cleared), mirroring how a title
travels. No GUI consumer yet, so nothing here needs a live tap; cwd-keyed ghost history
is the follow-on slice, and `docs/APP-BACKLOG-2026.md` records the shell dependency it
will hit -- nothing emits OSC 7 into our windows today, because macOS only installs
`update_terminal_cwd` when `$TERM_PROGRAM` is `Apple_Terminal` and we do not set it.

Gates: **516 tests / difftest 164 cases 164/164 / esctest 120 pinned / 14 + 49 exports /
Miri green / Swift smoke**. Four mutants seen red first (truncate-instead-of-drop,
OSC 7 ignored entirely, RIS preserving the pwd, and a host arm that percent-decodes).

**2026-07-30 append-all wave (one session):** twelve features landed, each on its own
`--no-ff` branch, gates green at every merge, all pushed. New since v0.8.0+S2: proper
ad-hoc bundle signing (TCC), S2 starship compat (integration on the oracle's zsh
pattern; `crates/host/tests/shell_integration.rs`), live cmd+=/-/0 zoom
(`ruuah_host_set_font_size` + `ruuah_host_cell_metrics`), styled underlines, OSC 8
hyperlinks (`ruuah_host_link_at`, cmd+click), the event seam (`ruuah_host_next_event`:
OSC 52 write / notifications / bell, exactly-once), DSR/DA replies (slice 9's seam;
esctest2 wiring still open), kitty graphics v1 (vendored `crates/vte` APC fork -- the
ONE workspace fork, re-vendor note in its Cargo.toml; `png` dependency), sixel v1
(same image pipeline, no oracle -- unit-gated), ligatures + font-family/font-ligatures
config (substitution-guard keeps Menlo byte-identical), VS16 emoji presentation (width
stays oracle-narrow), top tab bar 1:1 to the operator's reference (sidebar deleted),
spawn retry under fork pressure. Gates: **392 tests / difftest 132 cases 132/132 / 32
exports / Swift smoke**. Host archive exports grew 20 -> 32.


**Slices 0 through 8 are done**; the audit waves on 0-7 are tagged `v0.7.1`/`v0.7.2` and
slice 8 is tagged `v0.8.0`. 300 tests green; corpus 129 cases, 118 match / 11 diff, 129/129
met expectation; `libruuah-vt.a` at 14/14 exports, `libruuah-vt-host.a` at 20/20.

**Backlog P0.1 landed 2026-07-29 (`paste-2004`): bracketed paste + cmd+V.** The core
tracks mode 2004 (terminal-global, corpus-pinned in both directions including the
alt-screen and RIS edges), `Frame` carries a mode-bits word across the thread boundary,
`ruuah_host_paste` applies the oracle-measured paste transform (differential test against
`ghostty_paste_encode`, byte-for-byte), and the window's cmd+V hands raw clipboard bytes
to the host. New ABI export `ghostty_terminal_mode_get` mirrors the oracle's own -- it
answers for 2004 and returns INVALID_VALUE for untracked modes rather than a guessed
"off". End-to-end proof in `host_abi.rs` uses the pty's ECHOCTL to make the fenceposts
visible as pixels; the fence appears exactly when the child enabled 2004. The cmd+V tap
in the installed app still needs live eyes.
**Two open backlogs - read both before picking the next slice.**
`docs/BACKLOG-2026.md` (protocol, written 2026-07-29 after the app ran Claude Code live):
P0.1 paste is DONE; still open are P0.2 color emoji (renderer feature, not a font line),
P1 = synchronized output (mode 2026), DSR/DA replies (= slice 9), SGR mouse, scrollback
viewport, kitty keyboard, and the P2 polish items.
`docs/APP-BACKLOG-2026.md` (app slices S1-S9, mapped 2026-07-29 from Warp's open-source
release and Superset): settings+themes, Blocks on our OSC 133 rails, palette+workflows,
autosuggestions, worktree workspaces, diff panel, splits, persistent sessions, MCP
control. Homegrown clean-room law stated in the file - never copy AGPL/ELv2 code.

**Slice 9 (esctest2) DONE 2026-07-30** (`esctest-wiring`): the suite runs as the child
of our own pty host (`crates/pty/tests/esctest.rs`) and gates the workspace -- 568
tests in ~154s, 114 passes pinned BOTH directions in `corpus/esctest-expected-pass.txt`
(regression and unpromoted pass are each red; the reports-grant mutant was seen to fail
the gate). Enablers: DECRQCRA (per-cell checksum == codepoint, modern xterm), WINOPS 18
size report -- both behind `Terminal::enable_reports`, an embedder grant RIS cannot
revoke, OFF by default (the OSC 52-read posture; the app does not grant it yet) -- and
DECSTR, a corpus-pinned deliberate divergence (the oracle has no `!` intermediate
dispatch at all; esctest sends DECSTR before every test). The 409 failures are the
ranked reply/feature to-do list: rectangle ops, DECLRMM, DECRQM/DECRQSS, charsets,
REP, DECALN, IRM. Re-triage: the ignored `print_esctest_results` authoring test;
suite pin in `esctest.lock`, refetch via `scripts/fetch-esctest.sh`.

**Slice 8, `[tested]` (2026-07-29): the embedder surface and the minimal Swift host.**
`crates/host` exports `ruuah_host_spawn/poll/send/resize/free` -- the pty -> core -> frame ->
`Renderer<GpuSurface>` pipeline behind one C handle, declared in
`crates/host/include/ruuah_host.h`. The archive question is structural: **two Rust staticlibs
cannot share one link** (each carries the Rust runtime), so the host crate depends on the abi
crate as an rlib and `libruuah-vt-host.a` carries both surfaces (18 exports), while the pure
drop-in `libruuah-vt.a` stays slim and untouched. `swift/` is the SwiftPM host: a
systemLibrary target imports the crate's own header (no copied declarations), `--smoke` is
the headless CI-assertable proof, and the window blits polled RGBA at backing resolution
(font_size scaled by `backingScaleFactor`, one buffer pixel per device pixel), forwards keys
as bytes, and derives resize geometry from the first frame. vim runs in it:
`docs/images/slice-8-vim-window.png`.

The slice's blind spot (tenth in a row) was the C boundary itself -- every pixel test stopped
at a Rust API. The harness spawns a child through the C surface and **byte-compares polled
RGBA against a reference built with the same `Publisher` the pump uses** and drawn on the CPU
backend, so it also re-asserts CPU == GPU through the boundary; its control (a poll whose
draw declines row 0) fails inside the skipped row's band. A second test proves `send` by
round trip: bytes to `cat`, echoed by the line discipline, repeated by the child, matched as
pixels against the doubled line.

**The window's first vim frame found a real slice 7 defect.** One compute pass per operation
makes wgpu's Metal backend open a command buffer per pass; none can complete before the one
submit, and **Metal's 64-command-buffer pool blocks the 65th request forever, mid-encoding**
-- every prior GPU test stayed under the pool by accident, and the hang reproduced headless
under a 30s watchdog before the fix. `execute()` now records one pass for all operations
(each dispatch is its own usage scope, so the blend's write->read hazard is still
synchronized), which is also structurally cheaper. The watchdog test stays in `backend.rs`
with the op-count floor asserted, and the fix is byte-equal to the CPU reference.

**The S1 wave, `[tested]`.** A full audit on 2026-07-28 found 31 defects, 7 at S1, and the
pattern behind nearly all of them was that **the harness rule had been applied to the core and
never to the comparator or the ABI**. Ten of the 24 fields `difference.rs` compares had no test
that failed when the comparison was deleted; because 90 of 97 cases expected MATCH, deleting a
comparison only ever made more snapshots agree. So the two harness repairs landed FIRST, and
everything after them was measured against a gate that could actually go red.

All seven are fixed, each with a control that was run against the broken version and seen to
fail: the GPU op log now retires on read rather than replaying all history (hard ceiling was
1,048,576 ops); a seqlock read interrupted mid-copy invalidates the caller's frame instead of
leaving it wearing a stale generation; CUU/CUD/CNL/CPL are bounded by the scroll region;
`ED 3` clears the scrollback; `ghostty_terminal_new` NULLs its out-param on failure; the
comparator's damage/screen/cursor comparisons have controls; and a corpus that loses cases is
refused rather than reported on.

Three things worth keeping from it, because each cost a wrong first attempt:

- **A control aimed at the wrong workload proves nothing.** The seqlock's tearing harness ran
  a writer flat out, which holds the counter odd almost continuously, so reads bail before
  copying: **6,802,136 skips in one run, zero of them the path being tested.** The bug needed
  an orchestrated workload (large grid, writer publishing 60us after the reader enters the
  copy) to reach it about half the time.
- **Self-consistency was the wrong question there too.** An interrupted copy usually finishes
  with clean content wearing the wrong generation, so every cell agrees with every other one.
  The test asks whether the frame was TOUCHED instead.
- **A check placed before the thing it guards does not guard it.** The corpus size floor in
  `load` passes, then the loader truncates on the way out -- re-running the original mutation
  with the floor in place still exited 0. The fix is a count re-derived from the raw file text
  in `tests/corpus.rs`, which shares no code with the loader.

**The 2026-07-28 audit is fully closed: all 31 findings fixed**, on branch `audit-fixes-s2`
(2026-07-29), tagged `v0.7.2` after merge. Each fix carries a control run against the broken
version and seen to fail; gates green after every one (271 tests, corpus 125 cases
114 match / 11 diff 125/125, 13/13 exports). **Miri is now in the toolbox** (nightly + miri
installed 2026-07-29): `cargo +nightly miri test -p ruuah-vt-abi --test soundness` is the
oracle for UB-class defects, where a native run passing proves nothing -- run it whenever
the ABI handle model changes. Slice 8 landed on top of this wave.

- 8 `ed5c115` DECRC with nothing saved restores the synthetic default cursor, not nothing.
- 9 `58378d1` HT no longer clears `pending_wrap`; upstream's horizontalTab never touches it.
- 10 `6feed8d` reflow gated on DECAWM (`reflow = modes.get(.wraparound)`).
- 11 `5239f3c` ED 2 at a prompt scrolls into scrollback first. Two halves: the row count comes
  from the last row holding text (`PageList.zig:3099`), and the cursor follows its tracked pin,
  homing ONLY when that row left the active area (`Screen.zig:844` `cursorReload`). The first
  attempt got the grid right and the cursor wrong, and **difftest exited 0 while still wrong** --
  a case declared `diff` stays green whether or not the fix worked, so the flip to MATCH is the
  only real signal. Read the dump, not the exit code.
- 12 `5080cb2` `STYLE_ID` had dead bits 25-40. The id is **derived from the style**, not a table
  index: upstream's number is its own allocation order and this ABI resolves styles by grid
  position, so there is no table to index. Guarantees provided: 0 means default, equal styles
  mean equal ids. Deliberate, documented divergence -- revisit if a consumer ever needs the id
  as a lookup key.
- 13 `744d504` `CONTENT_TAG` reports `CODEPOINT_GRAPHEME` when the cluster has more than one
  codepoint (bit 42 of the packed cell). Upstream rule pinned by measurement through its own
  ABI: `appendGrapheme` flips the tag, `hasGrapheme` IS the tag.
- 14+22 `2812f8c` reads are reads: the view cache is `Mutex<Option<Snapshot>>`, read entry
  points take `&Terminal`, and a grid ref's `node` is the raw handle rather than a
  `&mut`-derived pointer. Both controls live in `abi/tests/soundness.rs` and only fail under
  **Miri** -- natively, UB looks like passing. Run it whenever the handle model changes.
- 15 `413fdfe` a pure out-param's `.size` is written from the type, never read. The oracle
  whole-struct-assigns at all four equivalent sites and its own tests pass `undefined`
  out-params. `GHOSTTY_INIT_SIZED` governs structs passed IN, not out-params.
- 16 `0ddbb5e` a resize past the frame channel's capacity is refused at `Host::resize` (and
  `spawn`) with a structured error, BEFORE the pty sees it. The pump's three `let _ =`
  publishes became expects -- with both entries gated, a failed publish is a bug in the gate.
- 17 `c5c5762` the seqlock writer marks in-flight with `start | 1`, not `start + 1`: after a
  panic escaped a fill closure, the blind bump inverted the counter's parity and every torn
  read thereafter wore a valid generation. One-line fix, deterministic single-threaded control.
- 18 `961a522` every `expect = "diff"` case now pins `diff_paths`, the exact measured set of
  difference paths, and `met_expectation` demands set equality; the loader refuses an
  unpinned diff case. The `print_measured_diff_paths` authoring test (ignored) regenerates
  pins when the oracle moves. This closes the trap finding 11 walked into.
- 19 `de9f1e3` the scroll region is carried across the screen switch in both directions --
  upstream keeps ONE `Terminal.scrolling_region` and `switchScreenMode` never touches it.
  Both direction cases measured DIFF first, flipped, promoted.
- 20 `4b97769` OSC 133 state is per-Screen and travels with the cursor: every switch carries
  it EXCEPT a 1049 exit (`restoreCursor` restores everything but `semantic_content`), so a
  `C` issued on the alt screen no longer leaks back.

- 21 `1d54493` a rows-only resize keeps custom HTS stops; the rebuild is guarded on the
  column count, exactly as upstream guards it (`Terminal.zig:3766`).
- 23 `6fdc023` a NULL out-param on the four grid-ref readers validates instead of failing --
  the headers say "(may be NULL)" and the oracle skips only the write. An out-of-bounds
  point and a dead ref stay errors, so it is not always-success.
- 24 `aa2f6fb` the alignment parity assertion read `"alignment"` where the report says
  `"align"` and silently skipped every struct; now required, not if-let. Deadness proven by
  mutation both ways.
- 25 `17b55db` `screen`, `cursor.visible` and `cursor.style` comparisons have unit controls;
  the audit's mutations M2/M4/M5 now each kill exactly their control.
- 26 `91cd1f9` the 47/1047/1049 mode split is explicit: 47/1047 never erase and copy the
  cursor in BOTH directions; a second `1049h` still saves and re-clears; `1049l` on the
  primary is still a DECRC. Three probes flipped and promoted.
- 27 `3dbb50f` the spacer head is written through the ordinary cell path -- pen style and
  cursor semantic -- not the erase blank.
- 28 `4d6181a` `ROW_DATA_DIRTY` answers from the view's damage (bit 6 of the packed row)
  instead of returning INVALID_VALUE.
- 29 `a0420c4` the OSC 133 parser matches upstream's strictness: action letter alone, `L`
  takes no options, `k=` value exactly one byte.
- 30 `dbbabe8` all 13 exports document their real caller contract; clippy's
  missing_safety_doc count went 13 to 0.
- 31 the docs sweep: README carried 195 tests and stopped at slice 5.5b; both docs now
  state the wave and the current gate numbers.

The audit backlog is empty. Slice 8 (the minimal Swift host) followed it -- see the status
block above.

Slice 5.6, `[tested]`: **OSC 133 and the visual caret.** The core tracks prompt / input /
output regions (`crates/core/src/semantic.rs`), and the renderer draws the caret at
`Frame::visual_column` rather than at `cursor.x` -- on a reordered row those are different
columns, and the glyph at the wrong one is a real character, which is why the old behaviour
looked entirely reasonable.

Sixteen corpus cases went in as `expect = "diff"` before the core could do anything and were
promoted to `expect = "match"` by the implementation. That promotion is the evidence: every
one disagreed with libghostty-vt before and agreed after, with the 71 pre-existing matches
never moving.

Three rules measured rather than assumed, each one a plausible implementation gets wrong:
**`B` and `I` diverge only across a soft wrap** (`index()` resets the clear-at-EOL state and
`printWrap` restores it); **`C` at column 0 un-marks the row and mid-row does not** (the rule
is positional, not content-based); and **reflow gives each destination row the mark of the
last source row that reached it** -- taking the last source row's mark unconditionally passes
a split and a rejoin and fails a two-into-three re-split, which is why `split` reports the
content offset each output row ended on.

`step_visually` is the arrow half and is deliberately **not** gated on the cell being input.
`Frame::is_input` is the gate and it belongs where the key event is handled, because outside
an input region the cursor is the running program's to place. There is no key encoder in this
repo, so the arrow work stops at the pure mapping; the GUI is what turns it into bytes.

Slices 0 through 5.5 detail follows.

Slice 5.5b, `[tested]`: **shaping.** `crates/render/src/shape.rs` runs each cell's cluster
through swash so combining marks are placed by the font's GPOS table instead of their own
bearings. Only clusters with more than one codepoint pay for it; a lone character takes
`place_at_origin`, which for one glyph is not an approximation but the exact answer.

The blind spot, eighth in a row: nothing could see where a mark LANDS. Every earlier test is
satisfied by a renderer that draws a niqqud at the cell's left edge -- the ink is still ink,
still inside the cell, still changes the pixels. Measured at 32px on a 19px cell, an unshaped
`בָּ` puts **9 pixels of ink in column 0** and a shaped one puts **none**. So the assertion is
positional and two-directional in one test: shaped, the leftmost columns must be empty;
unshaped, they must not be. `tests/shaping.rs` also guards the opposite failure -- a renderer
that "fixes" placement by dropping the marks entirely would satisfy emptiness perfectly, so a
second test requires the marks to add ink and to reach below the base.

**One cell is shaped at a time, deliberately.** A terminal cell is addressable, so shaping
across cells would let the renderer merge or re-space cells the program placed on purpose. The
cost is that Arabic contextual joining does not cross cells -- a limitation every terminal
shares, not a bug here.

Slice 5.5a, `[tested]`: **display bidi.** Hebrew now reorders. `crates/frame/src/bidi.rs`
lays a row out with `wezterm-bidi` under it, and the seam held exactly as designed -- the
renderer was **not touched**, because it never assumed a direction and asks `Run::column_of`
for every column it paints. 184 tests green; corpus untouched at 78/78, which is the proof
that bidi stayed out of the core.

Measured, not eyeballed: `crates/frame/tests/bidi_conformance.rs` runs **all 91,707
BidiCharacterTest cases through our own `visual_spans`** and passes, with 2,162 of the first
4,000 requiring real reordering so the comparison provably distinguishes visual from logical.
The suite caught a genuine bug on its first run -- the fast path that skips the algorithm for
plain text ignored the base direction, and under an RTL base even a row of pure neutrals
resolves to level 1 and reverses.

Verified by hand afterwards, through the real terminal path:

| logical | visual |
|---|---|
| `שלום עולם` | `םלוע םולש` (reads correctly right to left, word order kept) |
| `let msg = "שלום";` | `let msg = "םולש";` (code stays put) |
| `גיל 42 שנה` | `הנש 42 ליג` (**the number stays 42, not 24**) |
| `│אבג│abc│` | `│גבא│abc│` (bars stay in columns 0, 4, 8) |

Still missing in 5.5: **shaping**. A cluster's codepoints are still rasterized individually
and drawn at one pen position, so niqqud land on default bearings rather than where GPOS puts
them. `swash` shapes and its Hebrew GSUB/GPOS is verified, so this adds no dependency.

Slice 5, `[tested]`, tagged `v0.5.0`: all five acceptance criteria met. What remains from it
is the GPU backend, which is slice 6 territory.

Slice 5 step 4, `[tested]`: **it renders vim.** `ruuah-vt-render` is a CPU rasterizer -- glyph
atlas, xterm palette, damage-driven redraw -- built on `swash`. The backend is on the CPU
deliberately: the atlas, the run-to-column mapping and the damage logic are all
backend-agnostic, and only the final blit is not, so putting the reference backend here is
what lets "renders vim" be an assertion in CI rather than a screenshot somebody looked at
once. 168 tests green; corpus unchanged.

The harness went first again and the blind spot was, again, total -- nothing in the project
could see a **pixel**, so a renderer that drew nothing at all satisfied every existing test.
The invariant that closes it: **painting a sequence of frames incrementally must produce the
same bytes as painting the final frame in full.** One equality covers a row that changed
without being marked, a row marked but not repainted, stale pixels from a shortened line, and
a glyph overhanging into a row that is never redrawn. Proven able to fail by
`a_renderer_that_skips_a_stale_row_is_caught`, which runs the identical load through a
renderer that declines one row.

Criterion 5 is satisfied structurally rather than by promise: every column the renderer paints
comes from `Run::column_of`. Hebrew reaches the canvas today through the fallback font, in
logical order, and `hebrew_is_drawn_in_logical_order_today_and_slice_5_5_must_flip_this` pins
that by pixels -- when 5.5 reorders, that test fails, and the failure is the feature.

Slice 5 step 3, `[tested]`: the frame channel and the pty host, in two new crates.
`ruuah-vt-frame` publishes a whole frame from the parse thread to a renderer through a
**seqlock** -- a generation counter that is odd while a publish is in flight, so a reader
that arrives mid-write discards that frame instead of drawing half of one. `ruuah-vt-pty`
owns the pseudoterminal and the child, keeps the `Terminal` on its own pump thread, and is
the only crate in the project that performs I/O. 137 tests green; corpus unchanged at 78
cases, 71 match / 7 diff, 78/78 met expectation.

The harness came first here too, and this time the blind spot was total: a seqlock whose
reader ignores the counter passes every single-threaded test ever written against it.
`crates/frame/tests/tearing.rs` runs a writer and a reader flat out against a frame carrying
a self-consistency invariant, and **proves the invariant is sensitive** by running the same
load through a reader with the protocol removed, which must observe tearing. Both directions
were then confirmed by mutation: deleting the reader's `generation != before` re-check makes
the passing test fail with a real torn frame (`cell (1,0) is from publish 16 but (0,0) is
from 15`).

Slice 5 step 2, `[tested]`: damage tracking -- per-row dirty flags, a whole-frame flag, and
the cursor's contribution to both, measured against the oracle's `GhosttyRenderState`.

Slice 5 step 1, `[tested]`: the harness learned to see damage at all. `Snapshot` represented
neither dirty layer, so any damage implementation would have reported MATCH.

Slice 6 (the C ABI) and slice 7 (the GPU backend) both landed after this, and the shipped
artifact is now built and export-checked by `scripts/build-lib.sh`.

Slice 8 delivered the host this paragraph used to promise -- and it is deliberately not
RUUAH (measured 2026-07-28, its Swift app calls 99 `ghostty_*` symbols and not one is a
VT-core symbol). The host made slice 7's GPU backend mean something (its buffer is now
displayed, and its first window-sized frame exposed the Metal pool deadlock), and the
key-encoder path proved the `Host::send` seam esctest2 needs for DSR/DA replies, so slice 9's
blocker is gone.

Slice 4, `[tested]`: reflow. Resize rejoins soft-wrapped rows into logical lines, re-splits at
the new width, and maps the cursor through the transform. Scrollback takes part -- a logical
line straddling the history / active boundary is one line -- so the whole buffer is drained
into one row sequence, reflowed, and split back at the bottom. 103 tests green; 64 corpus
cases, 57 MATCH / 7 DIFF, 64/64 met expectation.

The measured rules, all against the real library on 2026-07-28 and each with a `reflow-*`
corpus case: trailing blanks are trimmed from the last row of a line only; the cursor's line
extends to reach a cursor parked past its content (which is why narrowing a full line leaves
an empty continuation row); a line with no content is deferred and emitted only if content
follows, which drops trailing blank rows while keeping interior ones; rows leaving the top
become scrollback and growth reclaims them; the alternate screen does not reflow; DECSTBM is
reset. A rows-only change does not reflow at all, because with the width unchanged there is
nothing to rejoin and rejoining anyway would grow the cursor's line by a row.

Slice 4 also fixed a slice-2 rule that nothing before it could observe: a soft wrap is marked
only when the cursor is at the last column. A deferred wrap survives a reflow verbatim, so
widening the screen leaves the cursor mid-row with the wrap still pending, and marking that
one would fuse two lines that were never one.

Slice 3, `[tested]`: paged scrollback. History is a list of fixed-capacity pages, each owning
its own cells, style table and grapheme storage, so dropping a page frees all of it. The row
budget is exact (rows are pruned individually from the front; the page is released once it
empties). Trailing blank cells are trimmed on the way in and re-padded on read, so scrollback
cost follows content rather than width. 94 tests green; 36 corpus cases, 30 MATCH / 6 DIFF,
36/36 met expectation.

Slice 2, `[tested]`: the autowrap phantom state (with DECAWM), scrolling and scroll regions
(DECSTBM, IND, RI, NEL, SU, SD), the alternate screen (47/1047/1048/1049), tab stops
(HTS, TBC, CHT, CBT), and the erase / insert / delete family (EL, ED, ECH, ICH, DCH, IL, DL),
plus DECOM, DECTCEM, DECSC/DECRC and RIS. 74 tests green; 28 corpus cases, 22 MATCH / 6 DIFF,
28/28 met expectation.

Slice 1, `[tested]`: `vte 0.15` driving a flat row-major cell array. Echo, full SGR, cursor
movement.

The seven remaining DIFF cases are **not slice boundaries**. They are named omissions kept
deliberately, because a corpus where nothing differs cannot show the harness detects
disagreement: DECALN, private/selective erase, REP, IRM, grapheme-cluster mode 2027, reverse
wraparound (mode 45), and - added by slice 4 - a **saved (DECSC) cursor sitting in the blanks
past its row's text when that row reflows**. Upstream clamps such a pin against whatever
column its sequential reflow writer happens to be parked on, a value carried over from the
*previous* line (`PageList.zig`, `reflowRow`, the `p.x >= cols_len` branch). Matching it means
emulating that writer's leftover state, and the branch next to it calls this area unspecified
outright. A saved cursor is otherwise tracked exactly like the live one: inside its row's
content it travels with its cell, and it falls back to the top-left when its row does not
survive. Both of those are corpus-pinned as MATCH.

Cell layout is settled and enforced: `Cell` is **8 bytes**, asserted at compile time in
`cell.rs`. Style is an interned `u16` into a `StyleTable`; grapheme continuations live in a
side map keyed by flat cell index. History pages carry their own tables; the active grid's
table **compacts** against live cells once it exceeds `cols * rows`, so neither grows without
bound.

**esctest2 is deferred, not skipped.** It drives a terminal through a real PTY and reads state
back via DSR/DA query responses. The PTY host now exists (`ruuah-vt-pty`, slice 5 step 3), so
the remaining blocker is the *reply* half: the core still has no DSR/DA response path, because
answering a query means writing bytes back, and the core does no I/O. The seam for it is
`Host::send`. The differential corpus covers the same semantics without either.

**Slice 0 detail.** The differential oracle harness runs, and the project is real.

- `[tested]` 32 tests green, `cargo test --workspace` exit 0.
- `[tested]` `ruuah-vt-difftest` over 13 corpus cases: 4 MATCH, 9 DIFF, 13/13 met expectation.
  Both directions demonstrated - the harness detects agreement *and* disagreement.
- `[tested]` The oracle readout itself: grapheme clusters, wide cells + spacer tails, the
  autowrap phantom state, alt screen, palette and RGB SGR, resumability across a write
  that splits a sequence mid-escape.
- `[tested]` Struct layouts pinned against the library's own `ghostty_type_json()`.
- `[tested]` `../ruuah` byte-identical and `git status` clean after a from-scratch oracle build.

**Deliberate divergence from Ghostty, slices 3 and 4.** Ghostty keeps the active area and the
history in ONE page list, so scrolling moves a pointer and reflow streams through the pages in
place. Here the active area stays a flat fixed-size grid (already bounded at `cols * rows`) and
only the *history* is paged, because that is where the unbounded growth was. Two costs, both
recorded rather than hidden: scrolling a row into history is O(cols) rather than a pointer
move, and a resize materialises the entire scrollback as owned rows before writing it back, so
peak memory during a resize is roughly double the scrollback. Resize is a human-driven event,
which is what makes that trade acceptable; a streaming page-at-a-time reflow is the fix if it
ever stops being.

Bidi lives in the renderer if it lives anywhere, and never in the core (see below).

## Project rules & gotchas

- **A GENERIC BINARY NAME IS NOT A CANDIDATE** (B4.1, 2026-08-05). `crates/mind2t/src/agent.rs`
  carries the agent-CLI matrix recovered from BridgeSpace. It probes bare **`agent`** first for
  Cursor - and on this machine `agent` is `~/.grok/bin/agent`, so "launch Cursor" starts **Grok**,
  silently, with a working agent in the pane. Found by the probe's very first run against the
  shell's own `command -v`, which is why availability is MEASURED rather than trusted (SCAR-003 -
  the registry is a claim about the world, and the world disagrees). `no_two_agents_can_resolve_to
  _the_same_binary` now refuses a name claimed twice and a name generic enough to belong to
  somebody else. A false negative (agent reported missing) is always preferable to a false
  positive (the wrong vendor's agent running your prompt).
- **THE AUTO-APPROVE GUARD IS NEVER A SANITISER.** `agent::screen` REFUSES a launch carrying
  `--yolo`, `--dangerously-skip-permissions`, `-a never` and the rest; it never strips them and
  proceeds. Stripping would leave the operator believing approvals were off when they were not.
  Matched as WHOLE argv tokens, because `--auto` is Factory's bypass and `--autosave` is not, and
  a guard that refuses near-misses is one people learn to route around. Both directions are
  tested and both mutants were seen red: substring matching fails the near-miss test, a guard that
  never fires fails the other three.
- **MIND2T'S GATE IS `scripts/smoke-mind2t.sh`, AND IT NEEDS NO SCREEN.** Orel's standing order
  (2026-08-04) while he works in parallel sessions: no windows on his display, no synthetic
  input. The script runs the real Tauri host with its window ordered out and asserts **sixteen**
  invariants about what AppKit, WebKit, the IPC and the CHILDREN actually did, exit code and all.
  Run it before committing anything under `crates/mind2t`. It ends as soon as it has collected
  everything (about 4s of run time), and burns its 20s ceiling only when something is wrong.
  The last two arrived with the canvas (B3.4) and both were seen red by their own mutant: the
  panes TILE the live window edge to edge (mutant: every rect keeps the full width - both panes
  then report 90 columns of a 900px window and draw on top of each other), and a **resize
  re-tiles every pane and shrinks every pane's own grid** (mutant: the handler ignores
  `Canvas::resize`). The resize is driven by `window.set_size` on the already-hidden window, which
  costs no screen and still comes back as a real `Resized` event - so the operator's first
  instinct is a gated path rather than a live-tap item.
  Four Tauri traps it pins, each of which presents as a blank chrome strip and none of which
  errors:
  1. a `devUrl` in `tauri.conf.json` sends every webview to a dev server in debug builds,
     whatever else the config says - there is deliberately none;
  2. a child webview created before `app.run` never navigates at all;
  3. Tauri 2 grants a webview NOTHING without a capability file, and denies silently;
  4. `tauri::WindowEvent` carries no keyboard variant, so the terminal's keys come from `NSEvent`
     and never from the webview (project law 2).
  The chrome is embedded at COMPILE time from `chrome/dist`, so a stale bundle ships silently;
  the script rebuilds it first. `chrome/dist` and `crates/mind2t/gen` are gitignored, like
  `web/dist`.
- **ONE GPU CONTEXT PER CANVAS, NOT PER SESSION** (B3.4, 2026-08-05). `Session::spawn` builds its
  own `GpuContext`, which is right for the one-terminal hosts and wrong for every pane: a
  composited frame is ONE render pass, and a pass can only bind buffers from its own device. So a
  canvas spawns through `Session::spawn_on` / `spawn_fitted_on` with the host's context, and the
  window's swapchain is built on that same one. Two things make this worth writing down rather
  than fixing quietly. It is **invisible to any test that does not present** - the canvas landed
  with real children, exact tiling and a green suite while being unable to draw itself. And it
  does not fail as "wrong device": wgpu reports it at `create_bind_group` as a usage-flags
  complaint about the wrong buffer entirely, so the message points away from the cause.
  `every_pane_reaches_one_frame_at_its_own_rect` (`crates/mind2t/tests/canvas.rs`) is the gate,
  and the mutant - one context per pane - was seen red while the other two canvas tests stayed
  green.
- **THE GATE DRIVES THE SESSION, NEVER APPKIT - AND THAT BOUNDARY IS THE HONEST PART** (B2.5).
  Four of the twelve invariants exercise a real child: a directory report, a paste, and a wheel
  scroll with its stillness control. All three are driven through `Session`, because
  synthesizing a cmd+V or a wheel event would put input into whatever the operator is doing on
  this machine. What that leaves untested is the AppKit half - the monitor's event mask, the
  keycode match for the chord, the pointer-versus-strip test - and it is a live tap, not a
  covered path. Say "tested through the session" and name the seam (SCAR-014).
- **A GATE THAT WAITS ON A SHELL MUST HOLD THE SHELL STILL.** Two measurements, both of which
  turned a correct implementation red first:
  1. **zsh re-reports its own directory from `precmd`**, so a synthetic OSC 7 report is replaced
     within milliseconds of the command ending. Reading the session's cwd at the END of a run
     finds the repository, correctly and uselessly. The gate records the SEQUENCE of directories
     and asks whether the probe's value ever appeared.
  2. A returning prompt changes the grid **with no input**, which breaks the "nothing else moved"
     control of the scroll check for reasons that have nothing to do with scrolling.
  The fix for both is **`exec cat`**: replacing the shell means no `precmd` ever runs again, so
  the directory stays put and the grid goes still - and `cat` echoes what it receives, which is
  the only way to SEE a mouse report, whose whole nature is to travel away from us. ECHOCTL
  draws the escape as a printable `^[`, so the assertion is ordinary grid text.
- **MOUSE POLICY LIVES IN `crates/host/src/pointer.rs`, ONCE** (B2.6). The encoder is pure and
  measured against the oracle; what it cannot own is the state around it - held buttons, the
  motion-dedup cell, the view geometry - and that state used to exist only inside the C ABI. Both
  surfaces now call the same `Pointer`, because the Swift host is the ORACLE for this port and
  two policies would make "do the two hosts agree?" a question about the hosts rather than about
  the port. The five end-to-end mouse tests in `tests/host_abi.rs` were written before the
  extraction and are what proved it changed nothing.
- **THE WHEEL GOES TO EXACTLY ONE PLACE**, and the order is the rule: a child that captured the
  mouse gets the report, otherwise the alternate screen with mode 1007 gets arrow keys, otherwise
  the host scrolls its viewport. `Session::wheel` answers `Ok(false)` for that last case rather
  than scrolling itself, so the decision cannot be taken twice. Scrolling the view under a
  full-screen program looks like a rendering bug and is a routing one.
- **`acceptsMouseMovedEvents` is NO by default on an NSWindow**, and the failure is invisible: a
  child in mode 1003 receives clicks and drags perfectly and never hears a bare move, so a menu
  that highlights under the cursor simply never highlights. Set once at launch.
- **The tao + wry host survives as `cargo run --bin probe`**, on the same rule that keeps the
  Swift host alive: a port with no reference is a rewrite with extra steps. Retire it when the
  Tauri host reaches parity. It no longer carries its own paste: `mind2t::clipboard` is the one
  implementation both hosts call, and the split inside it is deliberate - `paste_text` takes the
  text as an argument so a gate can drive the whole encode-and-send path with a fixture and
  never read, let alone disturb, the operator's real clipboard.

- **EVERY SIZE THIS RENDERER TOUCHES IS A DEVICE PIXEL, AND THE FONT SIZE IS THE ONE PEOPLE
  FORGET.** A host builds the session at `font_size * scale_factor`, never at the point size.
  Handing the renderer 16.0 on a 2x display rasterizes the whole grid at half resolution: the
  terminal works, the colours are right, the layout is right, and it is simply soft and small -
  nothing errors and no test fails. It has now been written twice, once in the Swift host
  (slice 8, `backingScaleFactor`) and once in Mind2t (B2.3, 2026-08-04), and the second time
  the operator caught it from the screen before any assertion did.
  Orel's display makes this trap permanent rather than occasional: an LG 2K panel driven in a
  faked Retina mode by his own `Studio/macos/opendisplay`, so `scale_factor` is **always 2.0**
  and a scale bug is always a half-size grid. Full measurement in `~/.claude/DEFAULT_MODE_NETWORK.md` §I.7.
  The window resize path has the matching gap: the font is built once at launch, so dragging to
  a display with a different scale does not re-rasterize. Named, not fixed (B2.4).

- **The oracle checkout moved (2026-08-06, Orel's call): `../ruuah` no longer exists.** RUUAH
  was archived to `~/Archive/studio-parked-20260806/tools-ruuah` and `Orellius/ruuah` on GitHub
  is archived read-only. The built oracle in `vendor/` keeps every gate working; a REBUILD
  (`scripts/build-oracle.sh`, `scripts/build-app.sh` icon) needs
  `RUUAH_VT_ORACLE_SRC=~/Archive/studio-parked-20260806/tools-ruuah`.
- **The oracle checkout is read-only, including build artifacts.** Never run `zig build` in it
  with default paths - that writes `zig-out/` and `.zig-cache/`. `scripts/build-oracle.sh`
  redirects both `--prefix` and `--cache-dir` here and then *verifies* the checkout is still
  clean, failing if it is not. Its whole economics are a near-zero rebase tax upstream.
- **Zig must be exactly `0.16.0` at `/opt/homebrew/opt/zig/bin/zig`**, called by absolute
  path. The machine default is 0.15.2 and refuses. The build script hard-checks the version.
- **Link the static archive, never the dylib.** `libghostty-vt.dylib` ships with **no
  LC_RPATH**, so anything linking it aborts in dyld at startup unless `DYLD_LIBRARY_PATH`
  is set. Worse: with both `.a` and `.dylib` on one search path, `-l static=` held for every
  target *except* the lib test harness, which silently picked the dylib. `build.rs` therefore
  symlinks only the archive into `OUT_DIR/link` and searches there. Cost: one confusing
  SIGABRT on 2026-07-28 - do not re-derive it.
- **Sized structs must have `.size` set before the library sees them.** `GhosttyGridRef` and
  `GhosttyStyle` lead with a `size_t size` (the `GHOSTTY_INIT_SIZED` mechanism); leaving it
  zero claims to be compiled against a zero-byte struct. Every construction site in
  `terminal.rs` sets it.
- **`ghostty_type_json()` is the library describing its own ABI**, and it is the reason the
  bindings can be trusted rather than hoped about. `tests/abi_layout.rs` compares every
  offset this crate touches against it, which also catches the vendored headers drifting
  from the linked archive - something bindgen alone cannot see.
- **Extend the harness BEFORE building the slice. Ten for ten.** Every slice has had a blind
  spot that would have reported MATCH for a wrong implementation, and each was found by asking
  "can the harness even see this?" before writing code. Slice 8's was the C boundary itself:
  every pixel test stopped at a Rust API, so `tests/host_abi.rs` byte-compares pixels polled
  through the C surface against a reference built with the pump's own `Publisher` -- and that
  harness caught the Metal command-buffer-pool deadlock on its first window-sized frame.
  Three of the earlier nine were total:
  concurrency, pixels, and -- in slice 5.6 -- OSC 133, where `Snapshot` had no semantic surface
  at all, so a core with zero OSC handling scored a perfect match on every prompt and input
  region. A fourth, the caret's landing column, was invisible to the pixel harness itself:
  `redraw.rs`'s incremental-equals-full invariant holds whether the caret is on the right cell
  or the wrong one, because both renderers are consistently wrong.
  - **Slice 2** - background-colour erase was invisible. Ghostty keeps a cell with only a
    background out of the style map, so `grid_ref_style` reported Default for a red cell. An
    erase ignoring BCE would have passed.
  - **Slice 3** - scrollback was invisible. The oracle ran `max_scrollback = 0` and `Snapshot`
    held only the active area, so any history implementation would have passed.
  - **Slice 4** - resize was not expressible at all: `Case` had no resize field and neither
    `Terminal` had a `resize` method, so any reflow would have passed. `Case` gained `resize`
    and `after`, and both were needed: `after` is the only way a grid comparison can see where
    the cursor landed, because a cursor that never writes leaves no trace. With the harness
    extended and a non-reflowing resize in place, wrapped cases DIFFed and flat ones MATCHed -
    both directions, before a line of reflow was written.
  Treat this as the project's dominant risk, not a coincidence. The differential harness only
  catches what the `Snapshot` represents, so the first question of every slice is what new
  observable it needs.
- **Read the oracle's source before inferring its behaviour.** `../ruuah` is a Ghostty
  checkout, so `src/terminal/PageList.zig` and `src/terminal/Screen.zig` are the reference
  implementation of everything the ABI exposes. Slice 4 burned five rounds of black-box probes
  on the saved-cursor mapping and got three mutually contradictory rules; the answer was
  twenty lines of `reflowRow`. Probe to find out WHAT differs, read the source to find out WHY.
  Both matter - the probes are what made the corpus, and the source is what stopped the
  guessing.
- **A corpus `expect = "diff"` is a to-do, not a pass.** When ruuah-vt implements that behaviour
  the case *fails*, and it gets promoted to `expect = "match"`. That is the mechanism, not a
  nuisance: a harness that cannot be wrong is not evidence. `tests/corpus.rs` additionally
  refuses a corpus that has drifted to a single direction.
- **Ghostty's scrollback limit is a memory budget scaled by WIDTH, not a row count.** Measured
  2026-07-28 against the real library: `max_scrollback` behaves as a boolean (0 disables, any
  non-zero value behaves the same), and writing 3000 lines kept **2998** rows at 6 columns but
  only **634** at 80. This core budgets in rows instead. The two prune POLICIES are therefore
  not comparable and must never be corpus-tested against each other - every scrollback case
  stays far under both thresholds, where contents agree exactly, and the policy is unit-tested
  in `history.rs`. This is the plan's ranked failure mode 4, confirmed on the real thing.
- **Only rows leaving the TOP OF THE SCREEN become history.** A scroll region that starts
  below row 0 pushes rows out inside the screen, not off it, and `delete_lines` removes them
  outright. Neither feeds scrollback; there is a corpus case pinning each.
- **The alternate screen has no scrollback**, by protocol. It is constructed with a zero
  budget, so this is structural rather than a check that can be forgotten.
- Cell text is a **grapheme cluster**, not a codepoint - encoded in `Snapshot` from day one
  because it is ranked failure mode 2 and 32 bits per cell is structurally insufficient.
- **The bidi oracle is Unicode, and it is not optional.** `./scripts/fetch-ucd.sh` vendors
  `BidiTest.txt` and `BidiCharacterTest.txt` into `vendor/ucd/`; `ucd.lock` pins the revision,
  so a conformance failure after a Unicode update can be told apart from a regression you
  caused. libghostty-vt cannot play this role -- it has no bidi surface at all.
- **`wezterm-bidi` was chosen by measurement, not by reputation.** Verified 2026-07-28 by
  running it over the whole suite: **770,241 / 770,241** BidiTest cases and **91,707 / 91,707**
  BidiCharacterTest cases, on paragraph level, resolved levels and visual order. It also fits
  the data model -- `resolve_paragraph(&[char], hint)` and level runs with contiguous logical
  ranges, which is why reordering did not change the shape of a `Run`. `unicode-bidi` (servo)
  is byte-indexed and would need an adapter for a cell grid.
- **The terminal's base direction is LTR, deliberately, and it is a policy not a bug.** A grid
  is column-addressed by the program drawing into it, so auto-detection would move a Hebrew
  status line written at column 0 to the right edge. With an LTR base, RTL runs still reorder
  within their own span. `BaseDirection::Auto` exists for a caller that wants it.
- **Segments are bounded by box drawing and block elements** (U+2500..=U+259F). Whole-line UBA
  across a table moves the frame characters relative to the text they enclose; bounding at the
  frame keeps every box where the program drew it. The Unicode suite has no opinion about this
  and cannot catch a wrong choice here, so it is unit-tested separately in `bidi.rs`.
- **Bidi is a renderer-layer item and must never enter the core.** Decided 2026-07-28, and
  slice 5.5 confirmed it: reordering landed with the corpus untouched at 78/78.
  `../ruuah/include/` has **zero** bidi/RTL surface, so reordering in the core breaks ABI
  compatibility - the project's whole thesis - and makes every RTL line diverge from the
  oracle *by construction*, deleting the only correctness signal there is. Ghostty's own
  bidi-adjacent code sits in the font shaper, not the VT core. Scar
  `~/.claude/scars/2026-06-11-bidi-terminal-deadend.md` and memory
  `feedback-no-bidi-in-terminals`: emulator bidi structurally cannot serve a cursor-addressed
  TUI, because the cursor has no mapping after reorder. **"Support most languages" is not
  bidi** - it is grapheme clusters plus correct width tables, both of which are slice 1.
- **Darwin refuses `TIOCSWINSZ` on the pty MASTER.** Measured 2026-07-28 on macOS 25.5, and
  confirmed with raw `libc::ioctl` as well as through rustix, so it is a kernel rule and not
  a binding bug: setting the window size on the master returns `ENOTTY` (errno 25,
  "Inappropriate ioctl for device"). It must go to the **user side**; reading it back with
  `TIOCGWINSZ` works from either end. Linux accepts both, which is exactly why this is easy
  to write wrong and only fails on the machine the project targets. `host.rs` therefore
  *reopens* the pts by path for each resize rather than holding a slave fd - holding one open
  would mean the master never reports EOF when the child exits, because this process would
  still have the other end open. Cost: eight failing integration tests with a misleading
  errno. Do not re-derive it.
- **The seqlock payload is `AtomicU64` read and written `Relaxed`, and there is no `unsafe`
  in it.** A classic seqlock races the reader against the writer, which in Rust's model is a
  data race and therefore undefined - the usual workarounds are `read_volatile` or a raw
  `copy_nonoverlapping`, both of which are formally still UB. The way out is that the standard
  library defines a data race as requiring a *non-atomic* access, so relaxed atomic loads that
  race are merely unordered, never undefined; the generation counter's `Acquire`/`Release`
  pair supplies the ordering that makes a set of atomic words into one consistent frame. This
  is why `Cell` being exactly 8 bytes pays off twice: one cell is one `u64`.
  The `seqlock` crate (0.2.0, May 2026) was checked and does not fit - it requires `T: Copy`
  and has no dynamically-sized payload, and a terminal grid is sized at runtime.
- **The renderer's only input is `Frame::runs`, and a `Run` carries a `Direction`.** Nothing
  produces a `RightToLeft` run yet; the variant and `Run::column_of` exist so that slice 5.5
  changes the run builder and not one line of drawing code. A renderer that adds an index to
  `run.start` itself compiles fine today and draws every Hebrew line backwards later. This is
  slice 5's acceptance criterion 5, built before the renderer rather than after it.
- **A published cell carries 16 bytes of inline UTF-8, not a pointer into an arena.** An arena
  needs an offset and a length, and a torn offset/length pair is the one thing that could turn
  a discarded frame into a fault. Sixteen bytes holds a base letter plus roughly seven
  combining marks, which covers Hebrew niqqud with room over; a longer cluster (multi-person
  emoji ZWJ sequences) is **flagged** via `PackedCell::is_truncated`, never silently shortened.
- **No single font can draw this terminal, and the font question is now settled.** Measured
  2026-07-28 by walking every font in `/System/Library/Fonts`, its `Supplemental`,
  `/Library/Fonts` and `~/Library/Fonts`, then shaping Hebrew through each candidate.
  - Menlo maps Hebrew to glyph 0. Arial Hebrew maps `'A'` to glyph 0, and is proportional
    besides. Fallback is therefore required, not an enhancement - which is why `FontStack`
    is plural from its first commit and the atlas keys on **(font, glyph)** rather than
    glyph. A glyph id without its font is meaningless, and collapsing the two would draw
    Hebrew with Menlo's glyph numbering.
  - **The Hebrew font is `Miriam Mono CLM`** (Culmus, Maxim Iorsh, GPL v2), installed at
    `~/Library/Fonts/`. It is the only monospace font found that does Hebrew *correctly*:
    GSUB composes shin+shin-dot and bet+dagesh into single glyphs, GPOS puts a qamats at
    exactly half the advance so it is centred under its base, marks carry **zero advance** so
    a pointed cluster stays ONE cell, and Latin and Hebrew both advance 0.6em - the same as
    Menlo, so the two share a grid exactly.
  - **It must not lead the stack**: it covers **0 of 128** box-drawing codepoints, 0 blocks
    and 0 powerline. Menlo leads, Miriam sits behind it, Arial Hebrew is the last resort so a
    machine without Culmus still works. `system()` filters to what is installed.
  - Iosevka and JetBrains Mono have no Hebrew at all, so the popular programming faces are
    not options. Building a font is not one either - see the note below.
  - `font.rs` unit-tests the coverage gaps in both directions, so a font change on the
    machine surfaces as a failing test instead of tofu on screen.
- **Do not try to build or merge a font.** A Hebrew-plus-Latin monospace face with correct
  niqqud is person-years of type design, and the shortcut - merging Menlo's Latin and box
  drawing with Miriam's Hebrew via fontTools - buys nothing the fallback stack does not
  already give (the advances already match) while creating a licence problem: Menlo is
  Apple's and not redistributable, and Miriam is GPL v2, so the merged output could never
  ship. Fallback is the answer, and it is already built.
- **The renderer has no shaping yet, and that is the honest gap.** A cluster's codepoints are
  rasterized individually and drawn at one pen position, which is right for Latin and only
  approximate for combining marks: a niqqud lands where the font's default bearings put it,
  not where GPOS mark attachment says it belongs. Real stacking is slice 5.5. `swash` was
  chosen partly because it also does shaping, so 5.5 adds no new dependency; `cosmic-text`
  pairs HarfRust with swash if its shaper turns out to be the better one, and 5.5 has a
  conformance oracle to decide that with rather than a preference.
- **The shipped artifact is `libruuah-vt.a`, but cargo cannot name it that.** Measured
  2026-07-28: a package called `libruuah-vt` emits `lib`**`lib`**`ruuah_vt.a`, and
  `[lib] name = "ruuah-vt"` is a hard cargo error - *"library target names cannot contain
  hyphens"*. Ghostty gets the hyphen in `libghostty-vt` because zig names artifacts freely.
  So the project is `ruuah-vt` (no `lib` prefix in a directory name - Ghostty's own project
  dir is `ghostty`), cargo emits `libruuah_vt.a`, and **slice 6 renames it to
  `libruuah-vt.a` in the build step** so RUUAH's link flag mirrors `-lghostty-vt` exactly.
  Do not try to make cargo produce the hyphen directly.

## Repo and git workflow

**`Orellius/mind2t`, private, at `~/Desktop/Studio/tools/mind2t`.** `origin` only.

**Renamed from `ruuah-vt` on 2026-08-06 (Orel's call), and the split is deliberate: the
CONTAINER is named after the product, the CRATES stay named after the engine.** `crates/*` are
still `ruuah-vt-core`, `ruuah-vt-frame`, `ruuah-vt-host` and so on, because the engine is the
part somebody else might embed and its name is its identity. GitHub serves a permanent redirect
from the old URL, so an old clone's `origin` keeps working - do not rely on that, run
`git remote set-url origin https://github.com/Orellius/mind2t.git`.

**There is no `upstream` remote, deliberately.** ruuah-vt is original code with no shared
history to track, so a second remote would be theatre. The upstream that actually matters is
the **oracle**: libghostty-vt is a moving reference implementation, and when Ghostty changes
behaviour the corpus verdicts move with it. `oracle.lock` pins the exact Ghostty commit the
current oracle was built from, `scripts/build-oracle.sh` rewrites it and **announces when the
oracle moved**. Commit `oracle.lock` whenever it changes - without it, a corpus case flipping
overnight is indistinguishable from a regression you caused.

- **PR WORKFLOW LAW (Orel's order, 2026-07-30): work lands through pull requests.** One
  branch per slice or fix, pushed to origin, `gh pr create`, gates green in the PR,
  merged `--no-ff` (via `gh pr merge --merge`). No direct pushes to `main`. GUI-facing
  changes carry a live-tap result or an explicit untested note in the PR body (SCAR-014).
  **AUTO-MERGE AMENDMENT (Orel, 2026-07-30 ~17:0x): when EVERY gate is green (full
  workspace suite on exit codes, difftest, export counts, smoke, live taps where the
  slice is GUI-facing), Claude merges the PR and rebuilds+reinstalls the app without
  waiting. A red or skipped gate, a named-divergence question, or anything touching
  the security posture still waits for Orel.**
- **RELEASE LAW (Orel's order, 2026-08-01; the floor is `~/.claude/MOTOR_CORTEX_EXECUTION.md`
  section 3b).** Every rebuild+reinstall of the app after merges is a RELEASE: annotated tag
  `vX.Y.Z` on main in the same session (message = what shipped + gate numbers, the old
  slice-tag shape; minor bump per rebuild batch, never per PR), then
  `gh release create <tag> --verify-tag --generate-notes` attaching
  `ruuah-vt-vX.Y.Z-aarch64-apple-darwin.tar.xz` (libruuah-vt.a + libruuah-vt-host.a +
  ruuah_host.h), `RUUAH-VT-vX.Y.Z-aarch64-apple-darwin.zip` (the ad-hoc-signed app), and
  `SHA256SUMS`. Never mint tags through GitHub's release UI (lightweight-tag trap). The
  57-merge/zero-tag train that forced this rule is backfilled as v0.9.0.
- **`main` holds verified slices only.** Every commit on it has `cargo test --workspace`
  green and `difftest` exiting 0.
- **One branch per slice**, `slice-N-<name>`, merged with `--no-ff` so slice boundaries stay
  visible in the history. `git log --first-parent main` then reads as one line per slice.
- **Annotated tag per completed slice**, `v0.N.0`. The tag message records the corpus state
  (case counts and verdicts), the test count, and the `oracle.lock` describe string - so a
  tag answers "what worked, measured against which Ghostty" without checking anything out.
  `v0.0.0` = slice 0 harness, `v0.1.0` = slice 1 parser + grid.
- Never push a slice branch that leaves the corpus failing its own expectations. A case whose
  verdict changed is either promoted (with its note rewritten) or explained.

## Toolchain

Rust 1.93.1, edition 2024, resolver 3. `cargo test --workspace` is the gate.

Dependencies stay deliberately few: `vte`, `unicode-width`, `thiserror`, `serde`, `toml`,
`rustix` (slice 5 step 3), `swash` (step 4, font parsing + rasterization + the shaper 5.5 will
need) and `wezterm-bidi` (5.5, chosen by conformance measurement). **`wgpu` is the one large
exception** (slice 7): measured at +123 packages against +9 for a Mac-only `objc2-metal`
backend, taking the tree from 80 to 181. Orel chose it over the Metal route with those numbers
in hand, for portability the roadmap does not yet want but may. `portable-pty` was evaluated for the pty host and rejected - on
macOS it costs thirteen crates including `serial2`, a serial-port library, and a second
`thiserror` major version alongside the workspace's. `rustix` costs three and the pty dance is
about sixty lines we own. **`cargo fmt --all` reformats the whole repo**, which was never
rustfmt-clean; format only the files a change actually touches, or the diff drowns.

```sh
./scripts/build-lib.sh         # build the shipped libruuah-vt.a and verify its 13 exports
./scripts/build-host.sh        # build libruuah-vt-host.a (both surfaces) and verify its 18 exports
./scripts/build-swift.sh       # archive -> swift build -> headless smoke, one command
./scripts/build-oracle.sh      # build libghostty-vt into vendor/ (the core's oracle)
./scripts/fetch-ucd.sh         # vendor the Unicode bidi suite (the reordering oracle) (../ruuah stays clean)
cargo test --workspace         # 32 tests
cargo run -p ruuah-vt-difftest            # the corpus report (binary is `difftest`)
cargo run -p ruuah-vt-difftest -- --dump  # plus both grids, rendered, per case
```

Env overrides: `RUUAH_VT_ORACLE_SRC` (the Ghostty checkout to build the oracle from,
defaults to `../ruuah`) and `RUUAH_VT_ORACLE_PREFIX` (a prebuilt prefix with `include/`
and `lib/`, bypassing the build script).

`vendor/` and `target/` are gitignored; the oracle is rebuilt from RUUAH, never committed.

## In-repo docs (source of truth)

- `corpus/cases.toml` - every byte stream and the verdict it is asserted to produce.
- `crates/snapshot/src/grid.rs` - what "the grid" means for comparison. The contract both
  implementations satisfy; neither owns it.
- `crates/core/src/reflow.rs` - the re-lay itself, as a pure transform over rows. Every
  non-obvious rule in it carries the measurement it came from.
- `crates/core/src/resize.rs` - the storage round-trip around that transform: drain, reflow,
  split back into history and active area.
- `crates/snapshot/src/difference.rs` - how disagreement is located and reported.
- `crates/frame/src/seqlock.rs` - the thread handoff. Read the module card before touching the
  ordering; every `fence` in it is load-bearing.
- `crates/frame/src/frame.rs` - what a renderer is allowed to draw from. The `Run` /
  `Direction` seam that keeps bidi out of the renderer.
- `crates/frame/tests/tearing.rs` - the concurrency harness, including the control that proves
  it can fail.
- `crates/pty/src/host.rs` - the only I/O in the project, and the one `unsafe` block.
- `crates/render/src/renderer.rs` - the consumer the `Run` seam exists for. Every column comes
  from `Run::column_of`.
- `crates/render/src/font.rs` - why the font stack is plural, with the measurement.
- `crates/render/tests/redraw.rs` - the pixel harness: incremental equals full, plus the
  control that proves it can fail, plus the logical-order pin 5.5 must flip.
- `crates/render/tests/vim.rs` - the acceptance gate. Writes a BMP to the temp dir for eyes.
- `crates/frame/src/bidi.rs` - reordering, and the two terminal policies on top of the UBA
  (LTR base, segments bounded by box drawing).
- `crates/frame/tests/bidi_conformance.rs` - the Unicode oracle, run against our own layout.
- `crates/core/src/semantic.rs` - OSC 133, and the three places its rules apply at once.
- `crates/ghostty/tests/semantic.rs` - what the oracle is known to do with OSC 133, measured.
- `crates/render/tests/caret.rs` - where the caret lands, found by diffing shown against
  hidden, plus the control that pins the old logical placement.
- `crates/render/src/surface.rs` - the backend seam, and the specified integer blend both
  backends must produce. Also holds the truncating control.
- `crates/render/src/gpu.rs` - the wgpu compute backend, and why the sRGB trap does not bite.
- `crates/render/tests/backend.rs` - CPU against GPU, byte for byte.
- `crates/abi-types/src/lib.rs` - the C types this library publishes. Depends on nothing so
  it can be linked beside the oracle without a symbol clash.
- `crates/abi/src/exports.rs` - the C entry points, thin on purpose.
- `crates/host/src/lib.rs` - the embedder C surface: the whole pipeline behind one handle.
- `crates/host/include/ruuah_host.h` - the contract the Swift host imports directly.
- `crates/host/tests/host_abi.rs` - pixels byte-compared through the C boundary, the
  skip-a-row control, and the `send` round trip via `cat`.
- `swift/Sources/ruuah-host/` - the minimal host: `--smoke` headless proof and the window.
- `crates/abi/tests/differential.rs` - the whole corpus driven through the C ABI and compared
  against the Rust API, plus the wrong-row control.
- `crates/ghostty/tests/abi_parity.rs` - our published layouts against libghostty-vt's own
  `ghostty_type_json()`. Caught a real `GhosttyPoint` size bug on its first run.
- `scripts/build-lib.sh` - builds `libruuah-vt.a` and verifies its exports.
- `crates/ghostty/tests/abi_layout.rs` - the ABI pin. Read before touching `sys`.
- `crates/ghostty/tests/oracle.rs` - what the oracle is known to read correctly.
- Conformance canon (not yet wired in): xterm ctlseqs, DEC STD 070, esctest2 (the CI
  target), wraptest (line-wrapping specifically), vttest (interactive, final pass only).
