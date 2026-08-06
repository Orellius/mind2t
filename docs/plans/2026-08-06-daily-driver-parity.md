# Daily-driver parity - Mind2t against Ghostty

**Decided 2026-08-06 (IDT) by Orel: make Mind2t the terminal he actually lives in.**
Slice namespace `D1..D10`, separate from `B1..B9` (Mind2t product) and `S1..S9` (app backlog).
Never mix them.

## The target, and what it is NOT

Orel's words were "99.9% at the same level, no fluff no bullshit". The measured answer, and the
reason the target is stated differently below:

**Ghostty's surface, counted from the archived checkout on 2026-08-06** (`~/Archive/studio-parked-20260806/tools-ruuah`, Ghostty 1.3.2-dev):

- **205 top-level config keys** (`src/config/Config.zig`, 10,934 lines)
- **84 keybind actions** (`src/input/Binding.zig`, 4,887 lines)
- ~90 built-in command-palette entries (`src/input/command.zig`)
- five shell integrations, a live VT inspector, quick terminal, tmux control mode, Kitty
  graphics/keyboard/clipboard/DnD protocols, SSH terminfo propagation, custom GLSL shaders,
  Sparkle auto-update, App Intents, Secure Keyboard Entry

Matching that list is not a slice; it is the whole remaining life of the project, and a Mind2t that
succeeded at it would be a worse-resourced Ghostty with no reason to exist.

**So the target is: nothing Orel uses in a day is missing.** Measured against the audit's own
ranking of what a user notices first, that is the ten items below - not two hundred.

## What Mind2t ALREADY has from that ranking

Not aspirational; these are gated in this repo today.

- GPU rendering, no readback (B1, `WindowTarget::present_all`)
- Splits - creation, tiling, rule between panes (B3.4-B3.6)
- True colour, 256 palette, styled underlines
- Kitty graphics v1 + sixel v1
- Shell integration: OSC 133 semantic zones, OSC 7 cwd
- Paged scrollback with reflow
- Ligatures, font fallback, shaping
- **Bidi. Ghostty has NONE** - `grep -ri bidi` over its `include/` and `src/terminal/` returns
  nothing. This is the one thing Mind2t does that his current daily driver cannot, and it is the
  actual reason to switch: 91,707 Unicode conformance cases green, niqqud placed by GPOS.

## D0 - config, themes, Hebrew-first (DONE 2026-08-06)

The engine's `Config` already existed and the Tauri host had simply never called it. Wired:
`~/.mind2t/config.toml` with a fallback to `~/.ruuah` (his existing theme is found rather than
silently ignored), theme palette, font family and size, configured shell, `auto_direction` ON by
default. `Session` now HOLDS its palette, because `resize` rebuilds the renderer and a
push-once palette silently reverted at the first window drag.

## The ten, in order

**ORDER CHANGED 2026-08-06, on measurement: D2 goes before D1.** The oracle publishes a full
`selection.h` (14 entry points) and no search API whatsoever, so selection can be gated
differentially and search cannot. Selection is also what a search hit is highlighted WITH, so
nothing is lost by taking it first.

1. **D1 Scrollback search.** The single most-missed feature in terminals that lack it. Ghostty's is
   incremental and threaded over the whole scrollback (`src/terminal/search/`). **No oracle** -
   the C ABI has no search surface, so its gate will be a reference implementation plus mutants,
   which is weaker than the corpus and has to be said out loud.
2. **D2 Selection and copy.** Click-drag, double-click word (with a word-chars notion),
   triple-click line, rectangular; cmd+C. Prerequisite for search-selection and for `copy-on-select`.
   - **D2a the model: DONE 2026-08-06** `[tested]`, 15 corpus cases against the oracle. Word,
     line and select-all ranges plus the clipboard text. See the repo `CLAUDE.md`.
   - **D2b the gesture: DONE 2026-08-06** `[tested]` headlessly, `[untested - needs your eyes]`
     for every real pointer path. Drag, double click a word, triple click a line, cmd+A,
     cmd+C. The seam is `Frame::viewport_rows`, which hands `core::selection` the snapshot
     rows it reads, so the oracle-gated rules are reused rather than re-derived - and because
     those rows are VIEWPORT rows, the absolute-versus-viewport conversion D2a warned about
     does not exist on this path at all. Rectangular selection is still modelled
     (`Selection::rectangle`) and not implemented.
3. **D3 Tabs.**
4. **D4 Keybind configuration.** Mind2t's chords are hardcoded today.
5. **D5 Font size chords: DONE 2026-08-06** `[tested]` headlessly. cmd+plus / cmd+minus /
   cmd+0 through `Session::set_font_size`, which re-derives the grid from the pane's pixel
   area. Ten percent steps rather than a point, so the step does not depend on the display
   scale. **All panes move together, and that is a constraint rather than a choice**: the
   wheel accumulator is shared across panes and is correct only while they agree on a cell
   height.
6. **D6 Clickable links: DONE 2026-08-06** `[untested - needs your eyes]`. cmd+click calls
   `Session::link_at` and hands the target to `/usr/bin/open`. Nothing in the gate can press
   cmd+click, so this one has no headless evidence at all.
7. **D7 Kitty keyboard protocol.** Helix, Neovim and Zellij key handling.
8. **D8 Config live reload.**
9. **D9 Command palette.**
10. **D10 Split navigation, zoom, resize.** Only creation exists.

## Deliberately NOT built (Orel's call, 2026-08-06)

Quick terminal, the VT inspector, custom shaders, the entire Linux/GTK surface, App Intents,
Sparkle auto-update. Revisit only if he asks.

## Standing risk carried into this plan

The pty acquisition retry (`crates/pty/src/host.rs`, 3 attempts x 25ms) was sized for ONE terminal.
cmd+D opens ptys on demand and B8 opens many. The class is real - it fired once in four full-suite
runs during B3.
