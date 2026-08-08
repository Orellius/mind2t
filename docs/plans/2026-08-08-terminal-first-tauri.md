# Strip to a terminal, and make it cross-platform on Tauri v2

> Written 2026-08-08 (IDT) on Orel's order, at v0.24.0. Paste the fenced block below into a
> cleared session opened in this repo.
>
> His words: *"Start to strip down the product, I want it to work as is as a terminal like
> Ghostty does. Like other terminals do, I want it to be cross-platform. That means we'll need to
> move from Swift to Tauri V2. Make the foundations of Tauri V2. And change the icon, create an
> icon with 'M2T', and rebuild the terminal when you finish stripping out the Swift."*

## The decision this reverses

The canvas/session-map direction (`2026-08-08-canvas.md`, C0/C1) is **parked, not cancelled**.
The product becomes a plain cross-platform terminal first. Agent-workbench work resumes on top of
a terminal that runs everywhere, or not at all - his call, later.

## What is actually in the way, measured 2026-08-08

Not opinions. Read from the tree at `dd24d98`:

| thing | state | cost |
|---|---|---|
| `crates/core`, `frame`, `snapshot`, `difftest` | portable already, no `cfg(target_os)` | none |
| `crates/render` | wgpu, portable; one macOS cfg in `present.rs` | small |
| `crates/pty` | POSIX; one macOS cfg in `lib.rs` | Linux small, **Windows is a rewrite** (ConPTY, no POSIX pty) |
| `crates/mind2t` keys | **`NSEvent` via `objc2`** - `tauri::WindowEvent` carries no keyboard variant, which is why it was done this way | the real work |
| `crates/mind2t` clipboard | `NSPasteboard` via `objc2-app-kit` | small, needs a portable path |
| `block2`, `objc2` in `[dependencies]` | **NOT target-gated** - the crate cannot compile off macOS at all | one manifest edit |
| `swift/` | 4,656 lines, 16 files | see below |

**The Swift host is currently the ORACLE for the Tauri port** (project law, `CLAUDE.md`). It
carries tabs, scrollback viewport, cmd+K palette with TOML workflows, fish-style autosuggestions,
OSC 133 blocks, git-worktree workspaces and a WKWebView diff panel. The Tauri host has none of
those.

**But the bar just moved, and that is what makes stripping defensible.** "A plain terminal like
Ghostty" does not need the palette, the blocks, the worktrees or the diff panel. It needs tabs or
splits, a scrollback viewport, config, themes and fonts. That is a much shorter list than Swift
parity, and it is the list below.

## Push-back, once, then execute

- **Windows is not the same job as Linux.** Linux is a manifest edit plus a portable key and
  clipboard path. Windows needs ConPTY and has no POSIX pty at all. Recommend **macOS + Linux
  first**, Windows as its own slice, and say so rather than discovering it at 80%.
- **Delete Swift LAST, not first.** It is the only reference for what correct looks like on the
  paths being ported. Git keeps it regardless, so nothing is lost by ordering the deletion after
  parity - and a port with no reference is a rewrite with extra steps.

---

```
Strip Mind2t to a cross-platform terminal on Tauri v2, and retire the Swift host.

READ FIRST, and treat this prompt as a hypothesis to verify against them:
  CLAUDE.md                                    (project truth, and the laws below)
  docs/plans/2026-08-08-terminal-first-tauri.md (this plan, with the measured gap table)
  docs/BACKLOG-2026.md                          (protocol gaps, P0/P1)

CONTEXT: the engine is done and is not what changes here. crates/core, frame, snapshot,
pty, render, abi, host all stay. This is about the PRODUCT layer only: crates/mind2t
(the Tauri host) and swift/ (the old one).

DECISION BEING EXECUTED: the agent-workbench direction is parked. Mind2t becomes a plain
terminal that works like Ghostty does, on more than one platform.

BUILD IN THIS ORDER, committing at each verified seam:

T1. MAKE IT COMPILE OFF macOS AT ALL. `block2` and `objc2` are in [dependencies]
    ungated, so crates/mind2t cannot build on Linux today. Gate them under
    [target.'cfg(target_os = "macos")'.dependencies], put every objc call behind
    cfg, and add a CI job that runs `cargo check -p mind2t` on ubuntu-latest. That
    job failing is the definition of done for this step - without it every later
    step regresses silently.

T2. KEYS WITHOUT AppKit. This is the real work and the reason the host looks the way
    it does: `tauri::WindowEvent` carries no keyboard variant, so keys are read from
    NSEvent. Find the portable source (tao's own event loop does carry keyboard
    events - `cargo run --bin probe` is a tao+wry host in this repo and is the place
    to measure it). Keys must NOT come through the webview: that is project law 2 and
    it does not bend. Prove the chord path on a non-Apple layout.

T3. CLIPBOARD WITHOUT NSPasteboard. crates/mind2t/src/clipboard.rs already splits
    `paste_text` from the read so a gate can drive it with a fixture. Keep that split.
    State why the standard path is insufficient before adding any dependency.

T4. THE TERMINAL FEATURE FLOOR, ported from swift/ into crates/mind2t. Only these:
      - scrollback viewport (wheel, cmd/ctrl+PageUp, Home, End)
      - tabs or splits, config-driven
      - config.toml + themes + font family/size/ligatures
      - OSC 133 semantic marks already exist in the core; the host must use them
    NOT ported: the cmd+K palette, TOML workflows, autosuggestions, OSC 133 blocks
    UI, git-worktree workspaces, the diff panel. Those were workbench features and
    they are parked with the workbench.

T5. THE ICON. Replace the screwdriver mark with a wordmark reading M2T. Source stays
    `assets/icon/mind2t.svg`; regenerate BOTH `mind2t-1024.png` and
    `crates/mind2t/icons/icon.png` with rsvg-convert - nothing does it automatically.
    Judge it at 32px, which is where four earlier icon attempts died. Colours from
    crates/render/src/color.rs, not invented ones.

T6. RETIRE THE SWIFT HOST, and only now. Delete swift/, scripts/build-swift.sh, the
    swift smoke stages and every reference. Git keeps it. Say in the commit what
    stopped being verifiable when it went, because the C-surface tests it drove are
    real coverage and losing them silently is worse than losing them.

T7. REBUILD AND RELEASE. scripts/build-app.sh currently assembles the SWIFT binary -
    it has to build the Tauri bundle instead (tauri.conf.json has bundle.active =
    false today and has never produced a .app). Tag, then build, then release: the
    bundle takes its version from `git describe` and describing an untagged tree
    bakes the previous one in.

CONSTRAINTS
- Rust, this workspace, existing crates. No new dependency without stating why the
  standard or built-in path is insufficient.
- Land on main, no PR (his standing order).
- Gates must stay green at every commit: `cargo test --workspace`,
  `cargo run -p mind2t-vt-difftest` (223/223), `scripts/smoke-mind2t.sh`,
  `scripts/build-lib.sh` (28 exports), `scripts/build-host.sh` (56 exports).
  NO WINDOW may open on his screen during a gate.
- Anything whose contract ends at a click, a drag, a chord or a resize is
  `[untested - needs your eyes]` or gets a live tap. This project has been bitten
  twice by "the C surface is not the app".

PLATFORM SCOPE, and say so out loud rather than discovering it late
- macOS and Linux are the target of this plan.
- WINDOWS IS A SEPARATE SLICE: there is no POSIX pty there, so crates/pty needs
  ConPTY. Do not start it inside this one, and do not claim cross-platform means
  Windows until it is built.

DONE MEANS
He opens Mind2t on macOS and it is a terminal he would use instead of Ghostty; the
same source builds and its tests pass on Linux CI; no Swift remains; the icon reads
M2T; and there is a tagged release with a bundle in it.

Start by telling me what you verified about the macOS coupling in crates/mind2t,
not what you plan to build.
```

## Order of danger, so it is not discovered at 80%

1. **T2 is the whole slice.** If keys cannot be read portably outside the webview, the Tauri
   direction does not hold and that has to surface in hours, not weeks. Do it before T4.
2. **T6 before T4 loses the reference.** The parity list in T4 is written against the Swift host;
   deleting it first turns "port" into "reinvent".
3. **T7 is where the app has never existed.** `bundle.active = false` means no Mind2t `.app` has
   ever been produced. Expect signing, icon-set and entitlement work that no test can predict.
