<p align="center">
  <img src="assets/icon/mind2t-1024.png" width="140" alt="Mind2t" />
</p>

<h1 align="center">Mind2t</h1>

<p align="center">
  An agent workbench and a daily-driver terminal, on a terminal engine written from scratch in Rust and gated on a differential oracle.
</p>

---

**Mind2t is a terminal you can put agents in.** Coding-agent CLIs running in real terminals
inside git worktrees, on an engine written from scratch (`crates/`) - Hebrew-first,
GPU-rendered, built to be someone's daily driver rather than a demo. It is that person's
daily driver already.

The wedge is ownership. We own the VT core, the pty and the renderer, so an agent's state is
read from a **typed grid** - `Session::rowText(row, semantic:)` - instead of being regexed out
of an ANSI byte stream. A real Claude Code CLI has been launched into a pane and its banner,
model, working directory and git branch read straight back off the grid, with no regex and no
ANSI parsing anywhere. Everything else in this repository exists to make that grid trustworthy.

The engine is a pure, deterministic state machine: bytes in, grid mutations out, with no PTY,
no GPU, no clock and no I/O inside it. That constraint is what makes the whole thing testable,
and it is the only architectural rule the project actually enforces.

![the feature tour, rendered live](docs/images/feature-tour-20260730.png)

<p align="center"><em>The engine's feature tour, rendered live in the Swift reference host.</em></p>

## Three names, and they are not interchangeable

- **Mind2t** is the PRODUCT: the cross-platform app (`crates/mind2t`, Tauri 2 + a native GPU
  surface). Born `Bindary`, briefly `Sadna`, renamed to Mind2t on 2026-08-06.
- **`ruuah-vt`** is the ENGINE, and it keeps its name forever: the VT core, pty, CPU and GPU
  renderers, and the C ABI. It is the part somebody else could embed, so its name is its
  identity. The container carries the product's name; the crates carry the engine's.
- **RUUAH VT** (`swift/`) is the ORACLE HOST, not the corpse. The Tauri host is being ported to
  parity against it, the same way the GPU backend is trusted because a CPU reference can
  disagree with it. It is kept deliberately, and it is where several features listed below
  still live.

## About the `-vt`, and about Ghostty

The `-vt` is the older half of the story: the engine was started as a drop-in for the C ABI
Ghostty publishes as `libghostty-vt`, meant to sit behind somebody else's GUI. It grew its own
window, renderer, panes and agent launcher, and stopped being a component.

Two things are worth stating precisely, because "built on Ghostty" would be wrong in both
directions:

- **No Ghostty code ships here.** The app binary contains zero Ghostty symbols and links zero
  Ghostty libraries - measurable with `nm` and `otool` on the installed app. Nothing in this
  repository is copied from it; the parser is a vendored [vte](https://github.com/alacritty/vte)
  fork, and everything above it was written for this project.
- **Ghostty is the oracle, and that is load-bearing.** At *test* time the real `libghostty-vt`
  is built and linked by the differential harness, and the corpus compares our grid against its
  grid, case by case. It is depended on by exactly one crate - `difftest`. That is not a
  courtesy citation: it is the reason any correctness claim here means anything.

So the ABI promise is still real and still tested - you *can* link `libruuah-vt.a` where
`libghostty-vt` was expected - but read the `-vt` as heritage, not as scope, and read Ghostty as
the measuring instrument, not the foundation.

## Why this exists

Writing a terminal from scratch is only worth doing if you can prove it is right, so this
project is built the other way round from usual: the **differential harness came first**, before
a single line of terminal logic. Every slice since is measured against a real reference
implementation rather than against an opinion of what terminals do - including the corpus
verdicts pinned to *disagree*, because a harness that cannot detect disagreement proves nothing.
The CPU and GPU renderers are bit-identical by specification, not merely close.

And the reason to want your own engine at all, twice over. **Hebrew, end to end, done properly** -
reordering, mirrored brackets and GPOS-placed niqqud are not a plugin here; they are why the
renderer is shaped the way it is. And **agents you can actually see**: every competitor rents
xterm.js or tmux, which means their agent state arrives as bytes on a JS thread. Ours arrives as
cells.

## Features

**Terminal core**
- Full VT parsing on a vendored [vte](https://github.com/alacritty/vte) (one addition: APC dispatch)
- True color; styled underlines - single/double/curly/dotted/dashed, SGR 58 underline color
- OSC 8 hyperlinks; the stamps survive scroll and resize
- OSC 52 clipboard write, OSC 9 / OSC 777 notifications, bell
- OSC 7 working directory, stored exactly as reported - including the two rules the reference
  implementation's *source* gets wrong, found by binary search against the real library
- OSC 133 semantic regions - prompt, input, output - the rails everything else rides
- DSR / DA / DECRQM query replies through the pty (programs that probe get real answers)
- Synchronized output (mode 2026) with an anti-stuck budget, so a wedged frame cannot freeze
  the display
- Bracketed paste (mode 2004) with the oracle-measured encoding
- **Selection as a query, not as state** - word, line and select-all ranges plus the clipboard
  text they format to, agreeing with the oracle on all 15 corpus cases that ask. The word rules
  are ported from the reference's *code*, not its doc comment, which is wrong: `.`, `/`, `-` and
  `_` are not boundaries, so a path, a filename and a flag select whole
- **Kitty graphics** - direct transmission, RGB/RGBA/PNG, chunked, queries answered, z-ordering,
  and **unicode placeholders**: the cells *are* the image, so it scrolls, reflows and erases with
  the text instead of chasing it
- **Sixel**, decoded into the same image pipeline - and advertised in DA1, which is a deliberate
  divergence: the oracle omits attribute 4 because it has no decoder, and we do
- Grapheme clusters from day one; wide glyphs and spacer tails; VS16 emoji presentation
- Paged scrollback with an exact row budget

**Rendering**
- Glyph atlas, damage-driven redraw; a CPU reference backend and a wgpu compute backend,
  byte-equal to each other
- Color emoji (sbix), synthesized block mosaics, per-glyph font fallback measured on this machine
- **Bidi done right**: UBA reordering in the renderer (91,707/91,707 BidiCharacterTest), mirrored
  brackets in RTL runs, niqqud placed by GPOS shaping - and never in the core, where reordering
  would break cursor addressing
- Font ligatures behind a substitution guard: a non-ligating font renders byte-identically with
  the feature on or off
- Selection drawn as a tint **blended over the finished row**, never as a cell background - a
  background is erased by the next cell's, so a selection painted that way is invisible over
  anything the child coloured

**Input**
- **Kitty keyboard protocol** - the full flag stack, CSI-u encoding, `modifyOtherKeys`; the
  encoder is byte-compared against the oracle's own across 135,216 cases, zero divergent
- **SGR mouse** (1000/1002/1003/1006) plus alternate scroll, against a ~65k-case differential
  matrix; the wheel routes by mode - report, arrow keys, or viewport
- Chords match *physical* keys, so a Hebrew layout keeps every one of them

**Mind2t, the product host** (`crates/mind2t`)
- One window that opens with **one pane**, split to the right with cmd+D; a real gutter is
  reserved between panes and a rule is drawn into it, in the same render pass
- **One wgpu surface for the whole canvas.** Pane count is a Rust-side fact; a composited frame
  is one render pass, so a pane that owned its own GPU context could not be composited at all
- Click-drag, double-click and triple-click selection, cmd+A, cmd+C - conditional on purpose, so
  with nothing selected it still falls through to the child as `^C` and can interrupt a command.
  Shift takes the pointer back from a child that has captured the mouse
- cmd+V bracketed paste, cmd+= / cmd+- / cmd+0 live zoom across every pane, cmd+click hyperlinks
- **Agent launcher**: ten agent CLIs with the fields that actually differ, a PATH probe that
  MEASURES availability rather than trusting a table, spawn-observe-retry with counted backoff,
  and a guard that REFUSES an auto-approve bypass rather than stripping it and proceeding
- `~/.mind2t/config.toml` (falling back to `~/.ruuah`): font size, family, ligatures, shell,
  themes, auto-direction, and `reports` - screen-inspection replies (DECRQCRA, WINOPS 18), **off
  by default**, because they let a program read back what is on your screen

**The Swift reference host** (`swift/`, RUUAH VT.app) - the parity target the port is measured
against, and where these still live today
- Top tab bar with live program titles and work-state dots driven by OSC 9;4 progress - explicit
  signals only, never idle-guessing
- **Scrollback viewport** - wheel and cmd+PageUp/Home/End, pinned to content while a program
  prints, snapping back the moment you type
- **cmd+K command palette** with TOML workflows: placeholders discovered from the command text,
  filled one at a time, and *pasted* rather than executed
- **Autosuggestions** - fish-style ghost text keyed by working directory: a command you ran
  *here* outranks a newer one you ran elsewhere, falling back to the newest anywhere so the ghost
  never vanishes just because you changed directory
- OSC 133 blocks with a gutter - copy command, copy output, run again - built to survive
  prompt-rewriting themes (starship's transient prompt included)
- **Git-worktree workspaces** and a WKWebView diff-review panel over the active session's repo

## The three architectural rules

1. **The core is a pure, deterministic state machine.** Everything else hangs off that, because
   it is what makes headless CI and differential testing possible at all. I/O lives in exactly
   one crate, and it is not the core. Every serious terminal ends up drawing this line
   somewhere; here it is drawn at a crate boundary the compiler enforces.
2. **No terminal bytes in the webview** - not pixels, not keystrokes, not frames, ever. The
   chrome strip is a browser engine drawing *documents*. Renting xterm.js would mean running a
   second VT parser in front of ours, which would make every test in this repository measure
   code nothing calls.
3. **One wgpu surface for all panes.** Pane count is a Rust-side fact; if the webview needs to
   know it, the design is wrong.

## How it is tested

Five independent gates, all on exit codes:

| gate | what it proves |
|---|---|
| `cargo test --workspace` | **688 tests** - units, pixels, concurrency, C-surface round trips |
| `ruuah-vt-difftest` | **223 corpus cases**, every verdict met - including the 17 pinned to *disagree* |
| `esctest2` | **391 pinned passes** of 568, both directions: a regression fails, and so does an unpromoted pass |
| `scripts/smoke-mind2t.sh` | **26 invariants** about what AppKit, WebKit, the IPC and the child processes actually did - and it needs no screen |
| export check | **14 + 56** C symbols present in the shipped archives |

Plus a headless Swift smoke test, a headless history smoke test, and - for anything whose
contract ends at the GUI - a live tap driving synthesized input into a real window.

The rule behind all of it: **a test must be seen to fail.** Every gate above has been run
against a deliberate mutant and watched go red. A guard is never proven by the absence of what
it suppresses.

## Building

```sh
cargo test --workspace              # the gate: every test green
cargo run -p ruuah-vt-difftest      # the corpus, measured against libghostty-vt
cargo run -q -p mind2t --bin mind2t # Mind2t itself
./scripts/smoke-mind2t.sh           # the host gate: no window, no synthesized input
./scripts/build-lib.sh              # libruuah-vt.a       (the drop-in ABI)
./scripts/build-host.sh             # libruuah-vt-host.a  (ABI + embedder surface)
./scripts/build-swift.sh            # the Swift reference host + headless smoke test
./scripts/build-app.sh              # assemble + sign + install RUUAH VT.app
sh scripts/demo-features.sh         # the one-screen feature tour, inside the terminal
```

The differential harness needs a Ghostty checkout as its oracle. The default `../ruuah` was
archived on 2026-08-06, so a rebuild needs `RUUAH_VT_ORACLE_SRC` pointed at wherever that
checkout now lives; the already-built oracle in `vendor/` keeps every gate working meanwhile.
`oracle.lock` pins the exact commit the corpus verdicts were measured against, so an upstream
behavior change is distinguishable from a regression. `scripts/fetch-esctest.sh` vendors the
esctest2 suite, pinned in `esctest.lock`.

## Contributing

Work lands on `main`, one commit per verified seam:

1. **Extend the harness before the change.** Every slice so far had a blind spot the existing
   tests could not see; the first question is always "can the harness see this?" Ten for ten, and
   several of those blind spots were total - a wrong implementation would have scored a perfect
   match on the entire corpus.
2. A new test must be *seen to fail* - against the pre-fix code or a deliberate mutant. A test
   that has never been red is not evidence, and a guard is not proven by the absence of what it
   suppresses: make it fire, then feed it the neighbouring input it must NOT catch and watch it
   stay quiet.
3. Gates before anything lands: `cargo test --workspace` green, difftest meeting every corpus
   expectation, `scripts/smoke-mind2t.sh` passing. GUI-facing behavior (clicks, chords, drag,
   resize) needs a live tap or an explicit `untested` note - the demo that works when a harness
   drives bytes routinely dies when a human drives the window.
4. A corpus case pinned `expect = "diff"` is a to-do, not a pass. Implementing that behavior makes
   the case *fail*; promoting it to `match` is the evidence the change worked.
5. Read the oracle's source before inferring its behaviour. Probing establishes WHAT differs; the
   source explains WHY, and several rules here were only ever settled that way.

## License

[AGPL-3.0](LICENSE). The vendored `crates/vte` fork remains MIT/Apache-2.0 (both license texts
kept in-tree).

**No Ghostty source is copied into this repository.** What is true, and worth saying plainly
rather than leaving to a reader to discover: parts of the VT core's *behaviour* were derived by
reading Ghostty's source to find out why the real library does what it does. Black-box probing
establishes WHAT differs; the source explains WHY, and several rules here were only settled that
way. That is derivation of behaviour from a published MIT-licensed reference, not transcription
of it, and the distinction is the whole reason the differential harness exists: agreement is
*measured through the ABI boundary*, never assumed from a shared line of code.

## Acknowledgments

- [Ghostty](https://ghostty.org) (MIT) - the reference implementation this engine is measured
  against at test time, and the origin of the ABI. None of its code ships here.
- [vte](https://github.com/alacritty/vte) - the parser this project vendors and extends.
- [esctest2](https://github.com/ThomasDickey/esctest2) (GPL-2.0, test-time only) - the
  conformance suite run against our own pty.
- [Culmus](https://culmus.sourceforge.io) - Miriam Mono CLM, the only monospace font we found
  that does Hebrew niqqud correctly.
