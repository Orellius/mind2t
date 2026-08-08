# What a terminal needs in 2026 - the open backlog

> **2026-07-30 append-all wave (one session, Orel's order):** CLOSED from this file --
> P0.2 VS16 boundary (emoji presentation renders in its single cell; width stays
> oracle-narrow, corpus pin `vs16-cluster-stays-narrow`), DSR/DA replies (the slice 9
> SEAM: core answerback queue drained by the pump; esctest2 harness wiring itself is
> still open below), styled underlines (5 kinds + SGR 58/59 color, pixel-proven), OSC 8
> hyperlinks (cmd+click opens), OSC 52 write + notifications + bell (event seam,
> exactly-once), **kitty graphics v1** (vendored vte APC fork, direct transmit
> f=24/32/PNG, placements, a=q answers -- icat's probe works), **sixel v1** (same image
> pipeline, no oracle -- said loudly), ligatures + font-family + font-ligatures config,
> live cmd+=/-/0 zoom, and true color CONFIRMED already-built since slice 1 (sgr.rs
> parses both 38;2; and 38:2: shapes, corpus-pinned -- it was never missing). STILL
> OPEN here: synchronized output (2026), SGR mouse, scrollback viewport, kitty
> keyboard, esctest2 wiring, DA1 sixel-advertisement decision, kitty unicode
> placeholders/z-index/animation/file-transmission.
>
> **CLOSED 2026-08-01: DA1 sixel advertisement + the reports-grant posture.**
> DA1 now answers `?62;4;22c`. Ghostty answers `?62;22c` and omits attribute 4
> because it has no sixel decoder -- its only sixel mention IS the capability
> table the number comes from (`device_attributes.zig:53`). We DO decode sixel,
> so silence was the inaccurate answer: probing tools fall back to nothing when
> 4 is absent, which is a working decoder nothing ever reaches. The divergence
> is deliberate and asymmetric on purpose -- advertising a capability we HAVE is
> a different claim from matching a reference that lacks it -- and it carries its
> own named test so removing sixel support means deleting that test WITH it.
> Screen-inspection replies (DECRQCRA, WINOPS 18) became a config key,
> `reports = true`, **default FALSE**: they let a program read back what is on
> screen, the same posture question as OSC 52 reads, so the grant stays the
> operator's and travels through the config handle at spawn. A NULL config handle
> grants nothing, which is the safe direction to be wrong in. Still open here:
> animation and file-transmission for kitty graphics.


Written 2026-07-29 (IDT), after the first day Mind2t ran real work (Claude Code,
vim, zsh) as an installed app. Everything below is verified-missing against this
repo on that date, not guessed: `grep` for the mode numbers, the live defect, or
the absent seam is named per item. Ordered by what a user hits first, not by
architectural interest. The iron rules hold for every item: harness BEFORE slice,
controls seen to fail, core stays pure (no I/O), bidi never enters the core.

## P0 - hit within minutes of real use

1. **DONE 2026-07-29 (branch `paste-2004`): paste (cmd+V) + bracketed paste (mode 2004).**
   Landed exactly on the planned shape -- core mode → `Frame.modes` word →
   `mind2t_host_paste` → NSPasteboard on cmd+V -- with two upgrades found on the way:
   the oracle exports `ghostty_terminal_mode_get` and `ghostty_paste_encode`, so the
   mode is corpus-pinned (4 cases) and the encoder is differentially tested
   byte-for-byte instead of inferred. The end-to-end proof dropped `od` entirely:
   the pty's default ECHOCTL echoes a pasted ESC as printable `^[`, so the
   fenceposts are visible grid text with plain `cat` -- two runs differing only in
   the child's `2004h` are the discriminating pair (`host_abi.rs`). The cmd+V tap
   itself in the installed app is the one thing still owed eyes.
2. **Emoji / color glyphs. DONE 2026-07-29.** `[🧠 BRAIN]` renders in color
   (`docs/images/emoji-brain-20260729.png`). Landed as predicted: an atlas entry
   kind (`GlyphData::Mask`/`Color`), a fifth surface op (`blend_image`, same
   rounded integer blend spec), a GPU `image` shader entry sharing the coverage
   buffer word-aligned, and Apple Color Emoji in the stack. The sbix strike is
   scaled to the cell height IN THE ATLAS (fixed-point bilinear, once, CPU-side)
   so both backends receive identical bytes -- CPU==GPU stays byte-exact with an
   emoji line in backend.rs's script. Pixel pin: `emoji_probe.rs` requires
   CHROMATIC ink (a mask-tinted silhouette or nothing both score zero).
   **Known boundary:** a VS16 cluster (`❤️` = U+2764+FE0F) resolves its base
   char at the first TEXT font that carries it and draws text presentation;
   emoji-presentation override for VS16 clusters is future cluster-level
   resolution work. Single-codepoint emoji -- the entire flagged case -- work.

## P1 - the modern-TUI contract

3. **Synchronized output (DEC private mode 2026). DONE 2026-07-30**
   (`sync-output-2026`). Core tracks the mode (terminal-global; ANY resize clears
   it -- the oracle's measured rule, corpus-pinned with alt-screen survival,
   136/136); the PUMP gates publishes while a batch is open, releasing on close
   with a 150ms anti-stuck budget (forced frames carry MODE_SYNCHRONIZED_OUTPUT);
   DECRQM answers so TUIs can detect it (1/2 for genuinely tracked modes, 0
   otherwise -- reply grammar mirrored from oracle source);
   `ghostty_terminal_mode_get` answers 2026. Gate-removed mutant seen red 3/3 on
   the split-batch pty test.
4. **DSR / DA query replies - slice 9's seam.** Programs probe the terminal
   (`ESC[6n`, `ESC[c`) and hang or degrade without answers. The reply path is
   `Host::send`, proven end-to-end in slice 8; esctest2 is the oracle and was
   always the plan of record. **esctest2 WIRED 2026-07-30** (`esctest-wiring`):
   the whole suite (568 tests) gates in ~154s through our own pty with the new
   reports grant (DECRQCRA + WINOPS 18 + DECSTR), 114 passes pinned both
   directions in `corpus/esctest-expected-pass.txt`; the 409 failures are the
   ranked to-do list (rectangle ops, DECLRMM, DECRQM/DECRQSS, charsets, REP,
   DECALN, IRM lead it). Suite pinned by `esctest.lock`.
5. **Mouse reporting (SGR 1006). DONE 2026-07-30** (`sgr-mouse`). The whole
   family: core tracks raw bits AND the derived last-writer pair for
   9/1000/1002/1003 events, 1005/1006/1015/1016 formats, 1007 alternate scroll
   (default ON) and DECCKM (mode 1, picked up because alternate scroll selects
   its byte form by it) -- corpus-pinned via mode_get (150/150), DECRQM answers
   the raw bits, esctest promoted DECRQM_DEC_DECCKM (119 pinned). The encoder is
   pure Rust in the pty crate and BYTE-COMPARED against the oracle's own
   `ghostty_mouse_encoder_encode` ABI (~65k-case matrix, dedup sequences, and
   terminal-derived-state sequences through `setopt_from_terminal`). Exports:
   `mind2t_host_mouse_geometry` / `mind2t_host_mouse` / `mind2t_host_wheel` (36
   total); the wheel owns the three-way precedence (mouse mode -> 64/65; alt
   screen + 1007 -> arrows, ESC O under DECCKM; else viewport). SCAR-014 live
   taps: real click -> `^[[<0;30;15M/m` at the aimed cell, real wheel -> `^[[B`
   on the alt screen (docs/images/sgr-mouse-*-tap-20260730.png); the tap also
   caught and fixed the quiet-child cellHeight=0 wheel deadness. Item 6's "alt
   screen wheel does nothing" boundary is now closed. Not done: SGR-Pixels is
   encoded but the window always reports cells (no 1016 consumer yet); no
   selection/shift-capture policy.
6. **Scrollback in the window. DONE 2026-07-30** (`scrollback-viewport`). The
   offset never enters the core: `Publisher::publish_scrolled` stitches history
   above the active grid, the pump owns/clamps the offset and pins it to content
   via `History::total_pushed`, `mind2t_host_scroll` + `viewport_offset` cross the
   C surface, and the window wires wheel + cmd+PageUp/PageDown/Home/End with
   snap-on-typing. No oracle exists (libghostty-vt has no viewport surface) --
   unit/pty/host-gated, mutants seen red. Named v1 boundaries: OSC 8 links in
   scrolled-back rows are not clickable (link ids don't survive the page
   readout); resize snaps to the bottom (reflow-riding viewport anchor = future
   slice, images-v2's class); the whole frame repaints while scrolled; on the
   alt screen the wheel does nothing yet (alternate scroll mode 1007 = future,
   pairs with item 5's mouse work).
7. **Kitty keyboard protocol. DONE 2026-07-30** (`kitty-keyboard`) -- and it grew
   into the WHOLE key path. Core: per-screen flag stack (oracle's exact ring/pop
   semantics), the full CSI u family, the `CSI ? u` detection reply, modes
   66/1035/1036 + modifyOtherKeys tracked (corpus 155/155, esctest 120 with
   DECNKM promoted). Encoder: `pty/src/key.rs` ports key_encode.zig entirely --
   legacy tables, ctrl mapping, modifyOtherKeys, all five kitty flags, alternates,
   event types, associated text -- BYTE-COMPARED against the oracle's key-encoder
   ABI over 135,216 cases (zero divergent) plus a terminal-derived-state layer.
   Host: `mind2t_host_key` (37 exports) encodes against the polled frame's modes.
   Swift: the hand-rolled byte encoder is gone; keyDown/keyUp/flagsChanged forward
   events built by Ghostty's own macOS recipe, with the keycode map GENERATED from
   the oracle's table (`scripts/gen-keymap.ts`). Live tap: `ok^[[27u^[[A` -- text,
   kitty-form escape, legacy arrow under disambiguate, one window
   (docs/images/kitty-keyboard-tap-20260730.png). Not done, named: IME composition
   (composing always false, the same boundary the byte path had); option-as-alt
   config (encoder supports it, no config surface); DECBKM (mode 67) untracked.

## P2 - polish that reads as craft

8. **OSC 8 hyperlinks** (render underline + open on cmd+click).
9. **Curly / colored underlines** (SGR 4:3, 58) - Claude Code uses them for
   diagnostics; currently flattened to plain underline.
10. **OSC 52 clipboard write** (child copies to system clipboard; read stays
    off by default - it is a security surface).
11. **Wedge/rounded mosaic synthesis** (U+1FB3C..): today they fall back to
    Iosevka's narrow glyphs; synthesizing the triangles removes the last
    non-cell-geometry art. Extend `mosaic.rs`; the no-gutter test generalizes.
12. **Mirror table → full BidiMirroring.txt**, generated from the vendored UCD
    with the same lock discipline as the bidi suite (today: curated table,
    boundary documented in `bidi.rs::mirror`).
14. **Kitty z-index and unicode placeholders. DONE 2026-07-31**
    (`images-v3a-zindex`, `images-v3b-placeholders`). Placements gained the three
    z layers the protocol defines (under the cell background, under the text, over
    everything), sorted by `(z, image id)` in the publisher. And images can now
    live IN THE GRID as U+10EEEE placeholder cells, which means they scroll,
    reflow and erase exactly like the text they are made of, with no anchor to
    keep in step - the structural answer to images drifting away from their cells,
    where v2 could only teach the anchors to follow. Decoder ported from the
    oracle's `graphics_unicode.zig` (the 297-entry diacritic table and the
    `canAppend` run rules); the renderer crops per run, so a run showing the
    middle of a scrolled image draws the middle of it.
    **Not done, named:** animation, explicit source rectangles (`x,y,w,h` - a
    placeholder crops by CELL, a different mechanism), placement ids beyond
    run-splitting, and file/shared-memory transmission.
13. **OSC 7 working directory. DONE 2026-07-31** (`osc7-cwd`). Stored raw and
    undecoded, terminal-global, cleared by an empty report and by RIS but not
    by DECSTR - every rule measured against the library before implementing,
    and pinned in `crates/ghostty/tests/pwd.rs` (13) plus the corpus (8 cases,
    6 promoted diff → match). It has a REAL oracle, unlike OSC 8 and OSC 52:
    the ABI answers `GHOSTTY_TERMINAL_DATA_PWD`, so ours does too.
    The rule that reading the source alone gets wrong in both halves:
    `reportPwd`'s 4096-byte truncation is unreachable for OSC 7, because the
    parser captures into a fixed `[2048]u8`; the real cliff is 2047 bytes
    stored whole, and past it the command is DROPPED, not truncated, leaving
    any previous pwd untouched. Our vte is built with `std` (unbounded
    `osc_raw`), so the core enforces that limit itself. Host seam: event
    kind 7, raw payload, empty = cleared.
    The probe also found a real defect next door: the oracle routes **OSC 9;9**
    (ConEmu CurrentDir) to the same pwd, while this core fell through to its
    notification branch and popped a desktop notification reading
    `9;/Users/orel/src`. Fixed in the same slice and corpus-pinned, because a
    working directory report becoming notification spam is a misbehaviour, not
    a missing feature.
    **Not done, and named:** **OSC 1337 CurrentDir** (iTerm2 spelling) also
    sets the pwd in the oracle - measured, not assumed - and this core ignores
    it. Inert rather than harmful (nothing else claims OSC 1337 here, so it is
    simply dropped), so it is left as a one-line follow-up instead of being
    smuggled into this slice.

## P2 - an Arabic face overhangs its cell (found 2026-08-08, corrected same day)

Found while gating Arabic joining, and it is a SEPARATE defect that predates it.
A run is painted in LOGICAL order, which on a right-to-left row runs right to
left on screen, so each letter's overhang lands on a cell that has already been
painted and is never cleared. Measured: `"\u{0628} \u{0628}"` at 32px puts **50
inked pixels into the empty space between the two letters**, and
`"\u{05D0} \u{05D0}"` puts none.

**THE FIRST VERSION OF THIS ENTRY WAS WRONG AND IS CORRECTED HERE RATHER THAN
REWRITTEN.** It said the macOS Arabic faces are proportional, that installing
**Kawkab Mono** avoids the defect entirely, and that the font stack already
prefers it. Measured 2026-08-08 on a machine where Kawkab IS installed, IS
ranked first and DOES answer for beh: the 50 pixels are still there. Advances at
32px against a 19px cell:

| codepoint | face that answers | advance |
|---|---|---|
| `A` | Menlo | 19.27px |
| `\u{05D0}` | Miriam Mono CLM | 19.20px |
| `\u{0628}` | **Kawkab Mono** | **22.40px** |

So the mitigation does not exist. **Monospaced is a property of a face on its
own - uniform advance within itself - and a terminal grid needs something else
entirely: an advance equal to the PRIMARY face's.** No font ships promising
that, and Miriam Mono matching Menlo to 0.07px is luck rather than design. It is
also why Hebrew has never shown this and Arabic always will.

**The fix, named rather than guessed at.** Rasterise each fallback face at a
size whose advance equals the cell width: `size * cell / advance`, here
`32 * 19 / 22.40 = 27.1px`. That is UNIFORM scaling, so letter shapes are
preserved - which is what separates it from the three options this entry
originally listed and rejected (horizontal run scaling distorts shapes, glyph
clipping cuts the strokes that make cursive readable, requiring a monospaced
face does not work as shown above). Cost: three files - a per-face size on
`FontStack`, the atlas rasterising at it, and `shape.rs` positioning at it -
plus pixel proofs. Hebrew moves by 1% and Latin not at all, so the blast radius
is the scripts that are currently broken.

**Pinned, not just described**: `an_arabic_glyph_still_overhangs_its_cell` in
`crates/render/tests/arabic.rs` asserts the defect AS IT STANDS, the way a
corpus case marked `diff` does. When the normalisation lands, that test fails,
and the failure is the feature.

The other tests in that file are written around the defect rather than against
it: form is asserted on glyph ids, and pixel comparisons are only made between
cells that are equally contaminated or equally clean.

## Deliberately NOT on the list

- **Bidi in the core** - renderer-layer forever (measured ABI + oracle reasons,
  see the bidi module). Auto base direction shipped 2026-07-29 at the frame layer.
- **Tabs / splits / config files** - that is Mind2t-the-app's territory, not the
  VT core's proof-of-consumability host.
- **GPU present path** (the buffer is blitted via CoreGraphics): measure first;
  at one 60Hz window the copy has never been the bottleneck.
