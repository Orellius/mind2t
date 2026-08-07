<p align="center">
  <img src="assets/icon/mind2t-1024.png" width="128" alt="Mind2t" />
</p>

<h1 align="center">Mind2t</h1>

<p align="center">
  A terminal written from zero in Rust, checked case by case against a real one.
</p>

<p align="center">
  <a href="LICENSE"><img alt="License: AGPL-3.0" src="https://img.shields.io/badge/license-AGPL--3.0-blue.svg"></a>
  <img alt="Rust 1.93+" src="https://img.shields.io/badge/rust-1.93%2B-orange.svg">
  <img alt="macOS arm64" src="https://img.shields.io/badge/platform-macOS%20arm64-lightgrey.svg">
  <img alt="692 tests" src="https://img.shields.io/badge/tests-692-brightgreen.svg">
</p>

---

## Why this exists

I wanted a terminal that renders Hebrew properly, and there wasn't one.

Not "supports Unicode". Properly: text that reorders right to left without dragging the numbers
backwards with it, brackets that mirror, and niqqud placed where the font's own tables say they
belong rather than wherever the default bearings drop them. Every terminal I tried got some part
of that wrong, and the ones that tried at all did it as a late addition on top of a renderer that
had been built assuming left to right.

That is not something you can patch in. It changes what a row is, so it has to be in the design
from the first commit.

The second reason arrived later and turned out to matter more. I run coding agents, several at
once, and every tool for doing that rents its terminal from `xterm.js` or `tmux`. That means the
agent's state arrives as a stream of escape codes that something has to guess its way back
through with regular expressions. If you own the terminal, you do not guess: the agent's model,
its working directory and its git branch are already sitting in cells, typed, because your own
parser put them there.

So the terminal came first, and the workbench grew on top of it.

## Written from zero

There was a predecessor. It was a fork of [Ghostty](https://ghostty.org), and forking taught me
where the seams were without teaching me why any of it worked. So I archived it and started
again from an empty directory on 2026-07-28.

The parser, the grid, scrollback, reflow, the pty, the renderer, bidi, shaping, the C ABI and
the app are all written for this project. The one piece not written here is the escape-sequence
tokenizer, a vendored fork of [vte](https://github.com/alacritty/vte), kept in-tree with its
licences and one addition of our own.

**No Ghostty code ships here.** The app binary contains zero of its symbols and links zero of
its libraries, which is checkable with `nm` and `otool`.

## How it is checked, and against what

Ghostty did not disappear from the project. It became the instrument.

The differential harness was written **before a single line of terminal logic**. At test time the
real `libghostty-vt` is built and linked, and a corpus feeds the same bytes to both
implementations and compares the resulting grids, case by case.

![the differential corpus, 223 cases](docs/images/difftest-corpus-20260807.png)

<p align="center"><em>The corpus, rendered by the terminal it is testing.</em></p>

Two hundred and six cases agree. **Seventeen are pinned to disagree on purpose**, because a
corpus where nothing ever differs cannot demonstrate that the harness detects difference at all.
The last line of that run is the point: agreement and disagreement are both detected.

A case pinned to disagree is a to-do rather than a failure. When the behaviour gets implemented
the case *fails*, and promoting it to `match` is the evidence the change worked.

![one disagreeing case, both grids](docs/images/difftest-oracle-vs-candidate-20260807.png)

<p align="center"><em>A pinned divergence: the reference grid, our grid, and every difference located to the cell.</em></p>

`oracle.lock` records the exact reference commit the verdicts were measured against, so a case
flipping overnight can be told apart from a regression.

### The rule underneath all of it

**A test must be seen to fail.** Every gate here has been run against a deliberately broken
version and watched go red. That is not a slogan; it is what found:

- a retry path matching the wrong errno on Darwin, which had never once executed
- a seqlock whose torn reads carried valid generation numbers, so every cell agreed with every other cell while the frame was wrong
- a Metal command-buffer pool deadlock that only appeared at a real window size
- a selection that outlived the pane it was made in and underflowed the renderer, found by splitting panes in the live window until it died

| gate | what it proves |
|---|---|
| `cargo test --workspace` | 692 tests: units, pixels, concurrency, C-surface round trips |
| `ruuah-vt-difftest` | 223 corpus cases, every verdict met, 17 of them pinned to disagree |
| `esctest2` | 391 pinned passes of 568, both directions: a regression fails, so does an unpromoted pass |
| `scripts/smoke-mind2t.sh` | 26 host invariants about what AppKit, WebKit, the IPC and the child processes actually did, with no screen required |
| export check | 14 + 56 C symbols present in the shipped archives |

The screenshots above were taken with `cargo run -p ruuah-vt-render --example screenshot`, which
runs a command in a real pty and renders the result with this terminal's own CPU backend. A
picture of some other terminal agreeing with our numbers would not be the claim being made.

## Architecture

Three rules, enforced by crate boundaries rather than by convention.

1. **The core is a pure, deterministic state machine.** Bytes in, grid mutations out. No pty, no GPU, no clock, no I/O. That is what makes headless CI and differential testing possible at all, and I/O lives in exactly one crate which is not the core.
2. **No terminal bytes in the webview.** Not pixels, not keystrokes, not frames. The chrome strip is a browser engine drawing documents. Renting `xterm.js` would mean running a second VT parser in front of ours, which would make every test here measure code nothing calls.
3. **One wgpu surface for all panes.** Pane count is a Rust-side fact. If the webview needs to know it, the design is wrong.

## Features

**Terminal core**

- Full VT parsing on a vendored [vte](https://github.com/alacritty/vte) fork (one addition: APC dispatch)
- True colour; styled underlines, single / double / curly / dotted / dashed, with SGR 58 underline colour
- OSC 8 hyperlinks whose stamps survive scroll and resize; OSC 52 clipboard write; OSC 9 and OSC 777 notifications; bell
- OSC 7 working directory, stored exactly as reported, including two rules the reference implementation's own source gets wrong
- OSC 133 semantic regions (prompt, input, output), the rails everything else rides
- DSR / DA / DECRQM query replies through the pty, so programs that probe get real answers
- Synchronized output (mode 2026) with an anti-stuck budget, so a wedged frame cannot freeze the display
- Bracketed paste (mode 2004) with the reference-measured encoding
- Selection as a query rather than as state: word, line and select-all ranges plus the clipboard text they format to. The word rules are ported from the reference's code, not its doc comment, which is wrong, so `.`, `/`, `-` and `_` are not boundaries and a path, a filename and a flag select whole
- Kitty graphics: direct transmission, RGB / RGBA / PNG, chunked, queries answered, z-ordering, and unicode placeholders where the cells *are* the image, so it scrolls, reflows and erases with the text
- Sixel, decoded into the same image pipeline, and advertised in DA1
- Grapheme clusters from day one; wide glyphs and spacer tails; VS16 emoji presentation
- Paged scrollback with an exact row budget

**Rendering**

- Glyph atlas with damage-driven redraw; a CPU reference backend and a wgpu compute backend, byte-equal to each other by specification
- Colour emoji (sbix), synthesized block mosaics, per-glyph font fallback
- **Bidi in the renderer**: UBA reordering at 91,707 of 91,707 BidiCharacterTest cases, mirrored brackets in RTL runs, niqqud placed by GPOS shaping. In the renderer and never in the core, because reordering there would break cursor addressing
- Font ligatures behind a substitution guard, so a non-ligating font renders byte-identically with the feature on or off
- Selection drawn as a tint blended over the finished row rather than as a cell background, which would be erased by the next cell's

**Input**

- Kitty keyboard protocol: the full flag stack, CSI-u encoding, `modifyOtherKeys`, byte-compared against the reference encoder across 135,216 cases with zero divergent
- SGR mouse (1000 / 1002 / 1003 / 1006) plus alternate scroll, against a ~65k-case differential matrix; the wheel routes by mode to report, arrow keys, or viewport
- Chords match physical keys, so a Hebrew layout keeps every one of them

**The workbench** (`crates/mind2t`)

- One window opening with one pane, split right with cmd+D, with a real gutter between panes and a rule drawn into it in the same render pass. Splitting stops before a pane becomes too narrow to use rather than continuing until something breaks
- A pane whose child has exited closes itself and gives its width back to the survivors; the last one closing closes the window
- Click-drag, double-click and triple-click selection, cmd+A, cmd+C. cmd+C is conditional on purpose: with nothing selected it falls through to the child as `^C` and can still interrupt a command. Shift takes the pointer back from a child that has captured the mouse
- cmd+V bracketed paste, cmd+= / cmd+- / cmd+0 live zoom across every pane, cmd+click hyperlinks
- Agent launcher: ten agent CLIs with the fields that actually differ, a PATH probe that measures availability rather than trusting a table, spawn-observe-retry with counted backoff, and a guard that refuses an auto-approve bypass rather than stripping it
- `~/.mind2t/config.toml`: font size, family, ligatures, shell, themes, auto-direction, and `reports`, off by default because it lets a program read back what is on your screen

**The Swift reference host** (`swift/`), the parity target the port is measured against

- Top tab bar with live program titles and work-state dots driven by OSC 9;4 progress
- Scrollback viewport: wheel and cmd+PageUp / Home / End, pinned to content while a program prints, snapping back the moment you type
- cmd+K command palette with TOML workflows: placeholders discovered from the command text, filled one at a time, and pasted rather than executed
- Autosuggestions, fish-style ghost text keyed by working directory
- OSC 133 blocks with a gutter: copy command, copy output, run again
- Git-worktree workspaces and a WKWebView diff-review panel over the active session's repository

## Status

Working, and in daily use on macOS, Apple Silicon. Pre-1.0, no stability promises. The workbench
host is still being ported to parity against the Swift reference host in `swift/`, which is where
some of the features above currently live. Other platforms are not built or tested; the core,
frame, render and difftest crates are portable and that work is welcome.

## Building

Requires Rust 1.93 or newer.

```sh
git clone https://github.com/Orellius/mind2t.git
cd mind2t
cargo run -q -p mind2t --bin mind2t     # run it
```

The rest:

```sh
cargo test --workspace                  # the gate: every test green
cargo run -p ruuah-vt-difftest          # the corpus, measured against libghostty-vt
./scripts/smoke-mind2t.sh               # host gate: no window, no synthesized input
./scripts/build-lib.sh                  # libruuah-vt.a       (the drop-in ABI)
./scripts/build-host.sh                 # libruuah-vt-host.a  (ABI plus embedder surface)
./scripts/build-swift.sh                # the Swift reference host and its headless smoke test
./scripts/build-app.sh                  # assemble, sign and install the app
sh scripts/demo-features.sh             # the one-screen feature tour, inside the terminal
```

Running the differential harness additionally needs a Ghostty checkout to build its oracle from,
pointed at by `RUUAH_VT_ORACLE_SRC`. See [CONTRIBUTING.md](CONTRIBUTING.md) for that and the rest
of the development setup.

## The names

- **Mind2t** is the product: the app in `crates/mind2t`.
- **`ruuah-vt`** is the engine, and it keeps its name. The VT core, pty, renderers and C ABI are
  the part somebody else could embed, so the crates carry the engine's name while the repository
  carries the product's.
- The `-vt` is heritage. The engine started as a drop-in for the C ABI Ghostty publishes as
  `libghostty-vt`, meant to sit behind somebody else's GUI, then grew its own window, renderer,
  panes and agent launcher. The ABI promise is still real and still tested: you can link
  `libruuah-vt.a` where `libghostty-vt` was expected.

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) first. The short version: extend the harness before you
change behaviour, and a new test must be seen to fail.

Security reports go through a [private advisory](https://github.com/Orellius/mind2t/security/advisories/new),
not a public issue. [SECURITY.md](SECURITY.md) documents what a remote byte stream is and is not
allowed to make this terminal do.

## License

[AGPL-3.0-only](LICENSE). The vendored `crates/vte` fork remains MIT OR Apache-2.0, with both
licence texts kept in-tree.

Ghostty is the measuring instrument, not the foundation: linked by exactly one crate at test
time, and absent from the shipped binary. [NOTICE](NOTICE) states the position in full, including
which parts of the core's behaviour were derived by reading the reference's source and why that is
derivation rather than transcription.

## Acknowledgments

- [Ghostty](https://ghostty.org) (MIT), the reference implementation this engine is measured against at test time, and the origin of the ABI.
- [vte](https://github.com/alacritty/vte), the tokenizer this project vendors and extends.
- [esctest2](https://github.com/ThomasDickey/esctest2) (GPL-2.0, test time only), the conformance suite run against our own pty.
- [Culmus](https://culmus.sourceforge.io), for Miriam Mono CLM, the only monospace font I found that does Hebrew niqqud correctly.
