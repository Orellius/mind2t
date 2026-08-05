# Daily-driver parity - Sadna against Ghostty

**Decided 2026-08-06 (IDT) by Orel: make Sadna the terminal he actually lives in.**
Slice namespace `D1..D10`, separate from `B1..B9` (Sadna product) and `S1..S9` (app backlog).
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

Matching that list is not a slice; it is the whole remaining life of the project, and a Sadna that
succeeded at it would be a worse-resourced Ghostty with no reason to exist.

**So the target is: nothing Orel uses in a day is missing.** Measured against the audit's own
ranking of what a user notices first, that is the ten items below - not two hundred.

## What Sadna ALREADY has from that ranking

Not aspirational; these are gated in this repo today.

- GPU rendering, no readback (B1, `WindowTarget::present_all`)
- Splits - creation, tiling, rule between panes (B3.4-B3.6)
- True colour, 256 palette, styled underlines
- Kitty graphics v1 + sixel v1
- Shell integration: OSC 133 semantic zones, OSC 7 cwd
- Paged scrollback with reflow
- Ligatures, font fallback, shaping
- **Bidi. Ghostty has NONE** - `grep -ri bidi` over its `include/` and `src/terminal/` returns
  nothing. This is the one thing Sadna does that his current daily driver cannot, and it is the
  actual reason to switch: 91,707 Unicode conformance cases green, niqqud placed by GPOS.

## D0 - config, themes, Hebrew-first (DONE 2026-08-06)

The engine's `Config` already existed and the Tauri host had simply never called it. Wired:
`~/.sadna/config.toml` with a fallback to `~/.ruuah` (his existing theme is found rather than
silently ignored), theme palette, font family and size, configured shell, `auto_direction` ON by
default. `Session` now HOLDS its palette, because `resize` rebuilds the renderer and a
push-once palette silently reverted at the first window drag.

## The ten, in order

1. **D1 Scrollback search.** The single most-missed feature in terminals that lack it. Ghostty's is
   incremental and threaded over the whole scrollback (`src/terminal/search/`).
2. **D2 Selection and copy.** Click-drag, double-click word (with a word-chars notion),
   triple-click line, rectangular; cmd+C. Prerequisite for search-selection and for `copy-on-select`.
3. **D3 Tabs.**
4. **D4 Keybind configuration.** Sadna's chords are hardcoded today.
5. **D5 Font size chords** - cmd+plus / cmd+minus / cmd+0. The C surface already has
   `ruuah_host_set_font_size`; the Tauri host does not call it.
6. **D6 Clickable links.** The engine resolves OSC 8 and the host never asks (`ruuah_host_link_at`).
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
