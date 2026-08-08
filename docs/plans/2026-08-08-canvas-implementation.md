# Canvas C1 - implementation plan (Rust), and the profile it needs first

> Written 2026-08-08 09:3x IDT, from a read of the live tree rather than from the
> C1 prompt. `docs/plans/2026-08-08-canvas.md` is the WHAT; this file is the HOW,
> and it opens with the prerequisite that plan does not name.
> Mockup: `~/Desktop/claude-html/mind2t-canvas-mockup-20260808-0930.html`.

## 0. Why there is no dev profile, measured

Nobody decided against one. The question never came up, because until now a run
of Mind2t read one file and wrote nothing. Verified in the tree today:

| Thing | Where it resolves | Profile-aware? |
|---|---|---|
| config + themes | `crates/mind2t/src/main.rs:977` `config_dir()` - `~/.mind2t`, else `~/.ruuah` | no |
| command history | `crates/host/src/lib.rs:790` - `~/.ruuah/history`, hardcoded | no |
| workflow templates | `crates/host/src/lib.rs:1227` - `~/.ruuah/workflows`, hardcoded | no |
| bundle identity | `crates/mind2t/tauri.conf.json` says `com.orellius.mind2t`; **`scripts/build-app.sh:40` writes `com.orellius.mind2t-vt` and `CFBundleName` "Mind2t VT"** | no, and drifted |
| worktrees | `<repo>-worktrees/<branch>`, beside the real repository | no |
| the gate | `scripts/smoke-mind2t.sh:35` poisons `PATH`/`TERM`/`CLAUDECODE` via `env` | env poison, not a profile |

Two facts that make this urgent rather than tidy:

1. **`bundle.active` is `false`** in `tauri.conf.json`, so Mind2t has never
   produced a `.app`. `scripts/build-app.sh` builds the **Swift oracle host**
   (`mind2t-host`), which is why the installed app still says Mind2t VT. There is
   currently one binary and one identity, so a dev/prod split costs nothing to
   introduce and gets expensive the day a second one exists.
2. **C1 changes the blast radius.** Today a dev run opens a shell. C1 spawns
   **live agent CLIs into git worktrees**. A canvas test run without a profile
   spawns real agents against his real repositories, writes his real history
   file, and creates worktrees beside his real work. That is a Class C working
   state risk introduced by the slice, not by this document.

### C0 - the profile, one slice, before any canvas code

One type, one resolver, every path derived from it:

```rust
/// Every on-disk location Mind2t owns, keyed by profile.
pub struct Profile {
    pub name: String,        // "default" | "dev" | anything he types
    pub root: PathBuf,       // ~/.mind2t/profiles/<name>
    pub bundle_suffix: &'static str, // "" | ".dev"
}
```

- `root` gives `config.toml`, `themes/`, `history`, `workflows/`, and a new
  `worktrees/` root. Nothing hardcodes `~/.ruuah` any more; the fallback stays
  as an explicit MIGRATION on the `default` profile only, exactly as
  `config_dir()`'s comment justifies it.
- Selection, in precedence order: `--profile <name>` argv, then
  `MIND2T_PROFILE` env, then `default`. Debug builds default to `dev`
  (`cfg!(debug_assertions)`), which is what makes it end to end: `cargo run`
  is a dev profile without anyone remembering a flag, and a release build is
  never accidentally in it.
- **Identity follows the profile**: product name `Mind2t (dev)`, bundle id
  `com.orellius.mind2t.dev`, a tinted title so the window is unmistakable in a
  screenshot. Fix the `build-app.sh` drift in the same commit or delete that
  script's Mind2t claims.
- `scripts/smoke-mind2t.sh` gains `MIND2T_PROFILE=smoke` with `HOME` pointed at
  a scratch dir. The env poison stays - it is testing a Finder launch, which
  the profile does not replace.
- **Gate, both directions**: a run under `dev` must be shown to write into the
  dev root and to leave the default root untouched (mtime compare); a run under
  `default` must still find an existing `~/.ruuah/config.toml`. A profile that
  cannot be seen to redirect a write is SCAR-004 shaped - it looks identical to
  one that is ignored.
- Cost estimate: **2-3h**. Declare it before starting (VTA law).

## 1. The canvas in Rust - what the tree says it costs

Read today, not recalled:

- `crates/mind2t/src/layout.rs:43` `Canvas::tile` is a ROW of rects; a second row
  is refused (`CanvasError::NotSplittable`, `canvas.rs:58`). C1 is a different
  layout model, so this module is replaced, not extended.
- `crates/render/src/present.rs:285` `blit_all` copies each pane's pixel buffer
  **1:1 at an integer origin**. There is no scale anywhere in the blit path.
- `Fill` (`present.rs:78`) is a solid rect with a colour. No text.
- `Session::set_font_size` exists and re-rasterizes; `Session::resize` changes
  cols/rows and therefore signals the child.

### The one architectural decision: zoom is a VIEW transform, never a resize

Three candidates, and only one survives contact:

| | mechanism | verdict |
|---|---|---|
| A | zoom by `set_font_size` per card | **rejected.** Cols/rows change with the font, so every zoom step is a `SIGWINCH` to every child. Forty-one TUIs reflowing on a trackpad pinch. |
| B | zoom by scaling the blit | **taken.** The pane keeps its grid; only the sampling changes. One uniform, no pty involvement, no reflow. |
| C | render cards into one giant offscreen surface | rejected. Memory scales with the canvas, not with what is visible. |

So the first code change is in the renderer, and it is small: `blit_all` takes
`(&mut GpuSurface, Rect)` - a destination rect - instead of `(&mut GpuSurface,
(u32,u32))`, and the blit shader samples `pixels` with the ratio between source
and destination extents. Nearest sampling first (it is a pixel copy today);
box-filter only if the zoomed-out text is judged unacceptable by eye.

**The control that proves it**: the existing `crates/host/tests/host_abi.rs`
pattern - a destination rect equal to the source size must be **byte-identical**
to today's output. A scale that quietly resamples at 1:1 is the failure that
would otherwise ship as "slightly soft, nobody knows why" (the same class as the
`backingScaleFactor` trap this project already paid for twice).

### Level of detail, and why it is not optional

Blitting N live grids per frame is how this becomes unusable at the exact scale
it exists to serve. Below a zoom threshold (0.55 in the mockup, a constant with
its measurement written beside it) a card does **not** blit its terminal. It
draws:

- a `Fill` for the card body and border, and
- an identity line rendered as a **synthetic `Frame`** through the ordinary
  renderer, so card text and terminal text keep one implementation and one font
  stack. Adding a second text path is how the Hebrew fallback stack gets
  reimplemented badly.

Above the threshold, the card is the terminal, fully interactive - which is
already true today, just at a different rect.

### Order of build, each a commit at a verified seam

1. **C1a - `SessionIdentity`** (`crates/mind2t/src/identity.rs`). repo, branch,
   agent, task, state. Branch from `git rev-parse --abbrev-ref HEAD` run in the
   session's cwd, cached and invalidated on the cwd event that already exists
   (OSC 7 crosses as event kind 7). Agent from the child's argv, reusing
   `agent.rs`'s registry - and its measured law holds: **a generic binary name
   is not a candidate** (`agent` is Grok on this machine). No UI.
2. **C1b - the waiting detector.** Reads `Frame::viewport_rows`, not bytes. Two
   independent signals: a known question marker per agent (extends the `agent.rs`
   matrix) and quiescence (no grid generation change for N seconds while the
   last row is not a shell prompt). Gate both directions - a fixture that ASKS
   must go waiting, a fixture that is merely slow must not. This is the slice; if
   only one thing works, it is this.
3. **C1c - blit scaling** (above), with the 1:1 byte-identity control.
4. **C1d - canvas space.** Free rects plus `(pan, zoom)`; hit-testing inverts the
   transform before `pane_at`. Auto-arrange becomes a command over that model,
   which is where today's `tile` survives.
5. **C1e - LOD cards.**
6. **C1f - omnibox.** Fuzzy match over the C1a records. No agent behind it.

### Risks named up front

- **The pointer.** Every gesture on this surface (pan, pinch, click-through into
  a card) ends at AppKit and is `[untested - needs your eyes]` or gets a live
  tap. Twice bitten already.
- **The waiting detector is a heuristic wearing a state name.** A false
  "waiting" that clears itself is noise; a missed one is the bug the slice
  exists to fix. Bias the detector toward false positives and say so.
- **Forty-one panes is forty-one ptys and forty-one GPU surfaces.** Nothing here
  has been measured above two. Measure before promising; a surface pool with an
  eviction rule for off-screen cards is the fallback, and it is C1g, not C1.
- **`cargo clean` after any directory rename** (`CARGO_MANIFEST_DIR` is baked at
  compile time) - unchanged, and it bites hardest right after a profile change
  moves paths around.
