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

3. **Synchronized output (DEC private mode 2026).** The 2026 pun is free but the
   mode is real: TUIs batch redraws with `ESC[?2026h/l` and expect no tearing
   between them. Core-side gate on publish; differential corpus case against the
   oracle (libghostty-vt implements it).
4. **DSR / DA query replies — slice 9's seam.** Programs probe the terminal
   (`ESC[6n`, `ESC[c`) and hang or degrade without answers. The reply path is
   `Host::send`, proven end-to-end in slice 8; esctest2 is the oracle and was
   always the plan of record.
5. **Mouse reporting (SGR 1006).** Claude Code, htop, lazygit all take clicks.
   Core tracks the modes; the window translates NSEvent to sequences; core stays
   I/O-free (encoding happens host-side, like keys — slice 5.6's rule).
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
7. **Kitty keyboard protocol (progressive enhancement).** 2026 CLIs (Claude Code
   included) request it for shift+enter and friends; without it they fall back
   silently. Mode negotiation in core, encoding host-side.

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

## Deliberately NOT on the list

- **Bidi in the core** — renderer-layer forever (measured ABI + oracle reasons,
  see CLAUDE.md). Auto base direction shipped 2026-07-29 at the frame layer.
- **Tabs / splits / config files** — that is RUUAH-the-app's territory, not the
  VT core's proof-of-consumability host.
- **GPU present path** (the buffer is blitted via CoreGraphics): measure first;
  at one 60Hz window the copy has never been the bottleneck.
