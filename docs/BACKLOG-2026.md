# What a terminal needs in 2026 — the open backlog

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


Written 2026-07-29 (IDT), after the first day RUUAH VT ran real work (Claude Code,
vim, zsh) as an installed app. Everything below is verified-missing against this
repo on that date, not guessed: `grep` for the mode numbers, the live defect, or
the absent seam is named per item. Ordered by what a user hits first, not by
architectural interest. The iron rules hold for every item: harness BEFORE slice,
controls seen to fail, core stays pure (no I/O), bidi never enters the core.

## P0 — hit within minutes of real use

1. **DONE 2026-07-29 (branch `paste-2004`): paste (cmd+V) + bracketed paste (mode 2004).**
   Landed exactly on the planned shape -- core mode → `Frame.modes` word →
   `ruuah_host_paste` → NSPasteboard on cmd+V -- with two upgrades found on the way:
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

## P1 — the modern-TUI contract

3. **Synchronized output (DEC private mode 2026). DONE 2026-07-30**
   (`sync-output-2026`). Core tracks the mode (terminal-global; ANY resize clears
   it -- the oracle's measured rule, corpus-pinned with alt-screen survival,
   136/136); the PUMP gates publishes while a batch is open, releasing on close
   with a 150ms anti-stuck budget (forced frames carry MODE_SYNCHRONIZED_OUTPUT);
   DECRQM answers so TUIs can detect it (1/2 for genuinely tracked modes, 0
   otherwise -- reply grammar mirrored from oracle source);
   `ghostty_terminal_mode_get` answers 2026. Gate-removed mutant seen red 3/3 on
   the split-batch pty test.
4. **DSR / DA query replies — slice 9's seam.** Programs probe the terminal
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
   `ruuah_host_mouse_geometry` / `ruuah_host_mouse` / `ruuah_host_wheel` (36
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
   via `History::total_pushed`, `ruuah_host_scroll` + `viewport_offset` cross the
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
   Host: `ruuah_host_key` (37 exports) encodes against the polled frame's modes.
   Swift: the hand-rolled byte encoder is gone; keyDown/keyUp/flagsChanged forward
   events built by Ghostty's own macOS recipe, with the keycode map GENERATED from
   the oracle's table (`scripts/gen-keymap.ts`). Live tap: `ok^[[27u^[[A` -- text,
   kitty-form escape, legacy arrow under disambiguate, one window
   (docs/images/kitty-keyboard-tap-20260730.png). Not done, named: IME composition
   (composing always false, the same boundary the byte path had); option-as-alt
   config (encoder supports it, no config surface); DECBKM (mode 67) untracked.

## P2 — polish that reads as craft

8. **OSC 8 hyperlinks** (render underline + open on cmd+click).
9. **Curly / colored underlines** (SGR 4:3, 58) — Claude Code uses them for
   diagnostics; currently flattened to plain underline.
10. **OSC 52 clipboard write** (child copies to system clipboard; read stays
    off by default — it is a security surface).
11. **Wedge/rounded mosaic synthesis** (U+1FB3C..): today they fall back to
    Iosevka's narrow glyphs; synthesizing the triangles removes the last
    non-cell-geometry art. Extend `mosaic.rs`; the no-gutter test generalizes.
12. **Mirror table → full BidiMirroring.txt**, generated from the vendored UCD
    with the same lock discipline as the bidi suite (today: curated table,
    boundary documented in `bidi.rs::mirror`).
13. **OSC 7 working directory. DONE 2026-07-31** (`osc7-cwd`). Stored raw and
    undecoded, terminal-global, cleared by an empty report and by RIS but not
    by DECSTR — every rule measured against the library before implementing,
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
    sets the pwd in the oracle — measured, not assumed — and this core ignores
    it. Inert rather than harmful (nothing else claims OSC 1337 here, so it is
    simply dropped), so it is left as a one-line follow-up instead of being
    smuggled into this slice.

## Deliberately NOT on the list

- **Bidi in the core** — renderer-layer forever (measured ABI + oracle reasons,
  see CLAUDE.md). Auto base direction shipped 2026-07-29 at the frame layer.
- **Tabs / splits / config files** — that is RUUAH-the-app's territory, not the
  VT core's proof-of-consumability host.
- **GPU present path** (the buffer is blitted via CoreGraphics): measure first;
  at one 60Hz window the copy has never been the bottleneck.
