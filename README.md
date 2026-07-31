<p align="center">
  <img src="docs/images/logo.png" width="140" alt="RUUAH VT" />
</p>

<h1 align="center">RUUAH VT</h1>

<p align="center">
  A terminal core in Rust, gated on a differential oracle - and the macOS terminal built on top of it.
</p>

---

**RUUAH VT is a terminal.** A native macOS app (`swift/`, `RUUAH VT.app`) on a terminal
engine written from scratch in Rust (`crates/`) - Hebrew-first, GPU-rendered, built to be
someone's daily driver rather than a demo.

The engine is a pure, deterministic state machine: bytes in, grid mutations out, with no
PTY, no GPU, no clock and no I/O inside it. That constraint is what makes the whole thing
testable, and it is the only architectural rule the project actually enforces.

![the feature tour, rendered live](docs/images/feature-tour-20260730.png)

## About the name, and about Ghostty

**RUUAH** (רוח) is the terminal. The `-vt` names the VT engine, and it is the older half
of the story: the engine was started as a drop-in for the C ABI Ghostty publishes as
`libghostty-vt`, meant to sit behind somebody else's GUI. It grew its own window, tab bar,
palette, autosuggestions and renderer, and stopped being a component.

Two things are worth stating precisely, because "built on Ghostty" would be wrong in both
directions:

- **No Ghostty code ships here.** The app binary contains zero Ghostty symbols and links
  zero Ghostty libraries - measurable with `nm` and `otool` on the installed app. Nothing
  in this repository is copied from it; the parser is a vendored [vte](https://github.com/alacritty/vte)
  fork, and everything above it was written for this project.
- **Ghostty is the oracle, and that is load-bearing.** At *test* time the real
  `libghostty-vt` is built and linked by the differential harness, and the corpus compares
  our grid against its grid, case by case. It is depended on by exactly one crate -
  `difftest`. That is not a courtesy citation: it is the reason any correctness claim here
  means anything.

So the ABI promise is still real and still tested - you *can* link `libruuah-vt.a` where
`libghostty-vt` was expected - but read the `-vt` as heritage, not as scope, and read
Ghostty as the measuring instrument, not the foundation.

## Why this exists

Writing a terminal from scratch is only worth doing if you can prove it is right, so this
project is built the other way round from usual: the **differential harness came first**,
before a single line of terminal logic. Every slice since is measured against a real
reference implementation rather than against an opinion of what terminals do - including
the corpus verdicts pinned to *disagree*, because a harness that cannot detect
disagreement proves nothing. The CPU and GPU renderers are bit-identical by
specification, not merely close.

And the reason to want your own engine at all: a place to put the things a borrowed one
has no surface for. Chiefly - **Hebrew, end to end, done properly.** Reordering, mirrored
brackets and GPOS-placed niqqud are not a plugin here; they are why the renderer is shaped
the way it is.

## Features

**Terminal core**
- Full VT parsing on a vendored [vte](https://github.com/alacritty/vte) (one addition: APC dispatch)
- True color; styled underlines - single/double/curly/dotted/dashed, SGR 58 underline color
- OSC 8 hyperlinks; the stamps survive scroll and resize
- OSC 52 clipboard write · OSC 9 / OSC 777 notifications · bell
- OSC 7 working directory, stored exactly as reported - including the two rules the
  reference implementation's *source* gets wrong, found by binary search against the
  real library
- OSC 133 semantic regions - prompt, input, output - the rails everything else rides
- DSR / DA / DECRQM query replies through the pty (programs that probe get real answers)
- Synchronized output (mode 2026) with an anti-stuck budget, so a wedged frame can't
  freeze the display
- Bracketed paste (mode 2004) with the oracle-measured encoding
- **Kitty graphics** - direct transmission, RGB/RGBA/PNG, chunked, queries answered,
  z-ordering, and **unicode placeholders**: the cells *are* the image, so it scrolls,
  reflows and erases with the text instead of chasing it
- **Sixel**, decoded into the same image pipeline - and advertised in DA1, which is a
  deliberate divergence: the oracle omits attribute 4 because it has no decoder, and we do
- Grapheme clusters from day one; wide glyphs and spacer tails; VS16 emoji presentation
- Paged scrollback with an exact row budget

**Rendering**
- Glyph atlas, damage-driven redraw; a CPU reference backend and a wgpu compute backend,
  byte-equal to each other
- Color emoji (sbix), synthesized block mosaics, per-glyph font fallback measured on this machine
- **Bidi done right**: UBA reordering in the renderer (91,707/91,707 BidiCharacterTest),
  mirrored brackets in RTL runs, niqqud placed by GPOS shaping - and never in the core,
  where reordering would break cursor addressing
- Font ligatures behind a substitution guard: a non-ligating font renders byte-identically
  with the feature on or off

**Input**
- **Kitty keyboard protocol** - the full flag stack, CSI-u encoding, `modifyOtherKeys`;
  the encoder is byte-compared against the oracle's own across 135,216 cases, zero divergent
- **SGR mouse** (1000/1002/1003/1006) plus alternate scroll, against a ~65k-case
  differential matrix; the wheel routes by mode - report, arrow keys, or viewport
- Chords match *physical* keys, so a Hebrew layout keeps every one of them

**The app**
- Top tab bar with live program titles and work-state dots driven by OSC 9;4 progress -
  explicit signals only, never idle-guessing
- **Scrollback viewport** - wheel and cmd+PageUp/Home/End, pinned to content while a
  program prints, snapping back the moment you type
- **cmd+K command palette** with TOML workflows: placeholders discovered from the command
  text, filled one at a time, and *pasted* rather than executed
- **Autosuggestions** - fish-style ghost text keyed by working directory: a command you
  ran *here* outranks a newer one you ran elsewhere, falling back to the newest anywhere
  so the ghost never vanishes just because you changed directory
- OSC 133 blocks with a gutter - copy command, copy output, run again - built to survive
  prompt-rewriting themes (starship's transient prompt included)
- cmd+click opens hyperlinks; cmd+T / cmd+W / cmd+V; cmd+= / cmd+− / cmd+0 live zoom
- `~/.ruuah/config.toml`: font size, font family, ligatures, shell, themes, auto-direction,
  and `reports` - screen-inspection replies (DECRQCRA, WINOPS 18), **off by default**,
  because they let a program read back what is on your screen

## The one architectural rule

**The core is a pure, deterministic state machine.** Everything else hangs off that,
because it is what makes headless CI and differential testing possible at all. I/O lives
in exactly one crate, and it is not the core. Every serious terminal ends up drawing this
same line somewhere; here it is drawn at a crate boundary the compiler enforces.

## How it is tested

Four independent gates, all on exit codes:

| gate | what it proves |
|---|---|
| `cargo test --workspace` | **562 tests** - units, pixels, concurrency, C-surface round trips |
| `ruuah-vt-difftest` | **164 corpus cases**, every verdict met - including the ones pinned to *disagree* |
| `esctest2` | **120 pinned passes** of 568, both directions: a regression fails, and so does an unpromoted pass |
| export check | **14 + 50** C symbols present in the shipped archives |

Plus a headless Swift smoke test, a headless history smoke test, and - for anything whose
contract ends at the GUI - a live tap driving synthesized input into a real window.

## Building

```sh
cargo test --workspace          # the gate: every test green
cargo run -p ruuah-vt-difftest  # the corpus, measured against libghostty-vt
./scripts/build-lib.sh          # libruuah-vt.a       (the drop-in ABI)
./scripts/build-host.sh         # libruuah-vt-host.a  (ABI + embedder surface)
./scripts/build-swift.sh        # the Swift host + headless smoke test
./scripts/build-app.sh          # assemble + sign + install RUUAH VT.app
sh scripts/demo-features.sh     # the one-screen feature tour, inside the app
```

The differential harness needs a Ghostty checkout as its oracle (`RUUAH_VT_ORACLE_SRC`,
default `../ruuah`); `oracle.lock` pins the exact commit the corpus verdicts were
measured against, so an upstream behavior change is distinguishable from a regression.
`scripts/fetch-esctest.sh` vendors the esctest2 suite, pinned in `esctest.lock`.

## Contributing

Work lands through **pull requests** - no direct pushes to `main`:

1. Branch from `main`, one branch per slice or fix.
2. **Extend the harness before the change.** Every slice so far had a blind spot the
   existing tests could not see; the first question is always "can the harness see this?"
3. A new test must be *seen to fail* - against the pre-fix code or a deliberate mutant.
   A test that has never been red is not evidence, and a guard is not proven by the
   absence of what it suppresses: make it fire.
4. Gates before any merge: `cargo test --workspace` green, difftest meeting every corpus
   expectation, the Swift smoke passing. GUI-facing behavior (clicks, chords, resize)
   needs a live-tap test or an explicit `untested` note in the PR.
5. Merges are `--no-ff`, so slice boundaries stay visible in `git log --first-parent`.

## License

[AGPL-3.0](LICENSE). The vendored `crates/vte` fork remains MIT/Apache-2.0 (both
license texts kept in-tree).

**No Ghostty source is copied into this repository.** What is true, and worth saying
plainly rather than leaving to a reader to discover: parts of the VT core's *behaviour*
were derived by reading Ghostty's source to find out why the real library does what it
does. Black-box probing establishes WHAT differs; the source explains WHY, and several
rules here were only settled that way. That is derivation of behaviour from a published
MIT-licensed reference, not transcription of it, and the distinction is the whole reason
the differential harness exists: agreement is *measured through the ABI boundary*, never
assumed from a shared line of code.

## Acknowledgments

- [Ghostty](https://ghostty.org) (MIT) - the reference implementation this engine is
  measured against at test time, and the origin of the ABI. None of its code ships here.
- [vte](https://github.com/alacritty/vte) - the parser this project vendors and extends.
- [esctest2](https://github.com/ThomasDickey/esctest2) (GPL-2.0, test-time only) - the
  conformance suite run against our own pty.
- [Culmus](https://culmus.sourceforge.io) - Miriam Mono CLM, the only monospace font we
  found that does Hebrew niqqud correctly.
