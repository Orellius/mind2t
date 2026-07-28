# CLAUDE.md — ruuah-vt (project child config)

> **Parent stack layer:** `../CLAUDE.md` (tools/ 2026 stack, auto-inherited; don't restate it).
> Chain: `~/.claude/CLAUDE.md` (contract) → `Studio/CLAUDE.md` (index) → `tools/CLAUDE.md` (stack) → **this file (ruuah-vt specifics)**.
> This file = project specifics only. Last update stamp: 2026-07-28 (IDT).
> Posture: the global proactive co-pilot rule (initiative, three-steps-ahead, extreme ownership) is in force here via `~/.claude/CLAUDE.md`.

## What this is

A terminal core in Rust implementing the C ABI Ghostty already publishes as
**`libghostty-vt`**, so it can drop in behind an existing native GUI. The point is control
and craft, not speed — Rust and Zig are peers here and there is no performance win waiting.

The consumer is **RUUAH** (`../ruuah`, branch `ruuah`, config in `RUUAH.md` **not**
`CLAUDE.md`). RUUAH's Swift app links `libghostty-vt` today; if ruuah-vt ever wins, RUUAH swaps
one link flag.

**The one architectural rule: the core is a pure, deterministic state machine.** Bytes in,
grid mutations out. No PTY, no GPU, no clock, no I/O. Everything else hangs off this,
because it is what makes headless CI and differential testing possible at all. Ghostty
enforces the same split physically (`src/terminal/` knows nothing about `src/renderer/`).

Plan of record: `~/.claude/plans/2026-07-28-rust-terminal-core.md`.
Architecture research it came from: `~/Desktop/claude-html/terminal-architecture-20260728-0132.html`.

## Status / current slice

**Slices 0, 1 and 2 are done.**

Slice 2, `[tested]`: the autowrap phantom state (with DECAWM), scrolling and scroll regions
(DECSTBM, IND, RI, NEL, SU, SD), the alternate screen (47/1047/1048/1049), tab stops
(HTS, TBC, CHT, CBT), and the erase / insert / delete family (EL, ED, ECH, ICH, DCH, IL, DL),
plus DECOM, DECTCEM, DECSC/DECRC and RIS. 74 tests green; 28 corpus cases, 22 MATCH / 6 DIFF,
28/28 met expectation.

Slice 1, `[tested]`: `vte 0.15` driving a flat row-major cell array. Echo, full SGR, cursor
movement.

The six remaining DIFF cases are **not slice boundaries** — slice 2 closed every one of those.
They are named omissions kept deliberately, because a corpus where nothing differs cannot show
the harness detects disagreement: DECALN, private/selective erase, REP, IRM, grapheme-cluster
mode 2027, and reverse wraparound (mode 45).

Cell layout is settled and enforced: `Cell` is **8 bytes**, asserted at compile time in
`cell.rs`. Style is an interned `u16` into a `StyleTable`; grapheme continuations live in a
side map keyed by flat cell index. Moving that map and table into a page's own allocation is
slice 3 work, and is what unlocks scrollback compression.

**esctest2 is deferred, not skipped.** It drives a terminal through a real PTY and reads state
back via DSR/DA query responses. This core has neither by design, so wiring it up needs a PTY
host — slice 5/6 territory. The differential corpus covers the same semantics without one.

**Slice 0 detail.** The differential oracle harness runs, and the project is real.

- `[tested]` 32 tests green, `cargo test --workspace` exit 0.
- `[tested]` `ruuah-vt-difftest` over 13 corpus cases: 4 MATCH, 9 DIFF, 13/13 met expectation.
  Both directions demonstrated — the harness detects agreement *and* disagreement.
- `[tested]` The oracle readout itself: grapheme clusters, wide cells + spacer tails, the
  autowrap phantom state, alt screen, palette and RGB SGR, resumability across a write
  that splits a sequence mid-escape.
- `[tested]` Struct layouts pinned against the library's own `ghostty_type_json()`.
- `[tested]` `../ruuah` byte-identical and `git status` clean after a from-scratch oracle build.

**`crates/core` is a stub, not a design.** Printable ASCII, CR, LF, BS. It exists solely so
the harness has a second input. Slice 1 replaces it with the `vte` crate driving a real cell
grid; do not grow the stub instead.

Next: **slice 1 — parser + cell grid**. The nine DIFF cases are its specification; each
reported path (`cell[0,1].style`, `row[0].wrap`, `cursor.pending_wrap`) is a thing to build.

## Project rules & gotchas

- **`../ruuah` is read-only, including build artifacts.** Never run `zig build` in it with
  default paths — that writes `zig-out/` and `.zig-cache/`. `scripts/build-oracle.sh`
  redirects both `--prefix` and `--cache-dir` here and then *verifies* the checkout is still
  clean, failing if it is not. Its whole economics are a near-zero rebase tax upstream.
- **Zig must be exactly `0.16.0` at `/opt/homebrew/opt/zig/bin/zig`**, called by absolute
  path. The machine default is 0.15.2 and refuses. The build script hard-checks the version.
- **Link the static archive, never the dylib.** `libghostty-vt.dylib` ships with **no
  LC_RPATH**, so anything linking it aborts in dyld at startup unless `DYLD_LIBRARY_PATH`
  is set. Worse: with both `.a` and `.dylib` on one search path, `-l static=` held for every
  target *except* the lib test harness, which silently picked the dylib. `build.rs` therefore
  symlinks only the archive into `OUT_DIR/link` and searches there. Cost: one confusing
  SIGABRT on 2026-07-28 — do not re-derive it.
- **Sized structs must have `.size` set before the library sees them.** `GhosttyGridRef` and
  `GhosttyStyle` lead with a `size_t size` (the `GHOSTTY_INIT_SIZED` mechanism); leaving it
  zero claims to be compiled against a zero-byte struct. Every construction site in
  `terminal.rs` sets it.
- **`ghostty_type_json()` is the library describing its own ABI**, and it is the reason the
  bindings can be trusted rather than hoped about. `tests/abi_layout.rs` compares every
  offset this crate touches against it, which also catches the vendored headers drifting
  from the linked archive — something bindgen alone cannot see.
- **A corpus `expect = "diff"` is a to-do, not a pass.** When ruuah-vt implements that behaviour
  the case *fails*, and it gets promoted to `expect = "match"`. That is the mechanism, not a
  nuisance: a harness that cannot be wrong is not evidence. `tests/corpus.rs` additionally
  refuses a corpus that has drifted to a single direction.
- **The oracle is only the *active area*.** `max_scrollback = 0`. Scrollback comparison
  arrives with slice 3; do not read `GHOSTTY_POINT_TAG_SCREEN` before then.
- Cell text is a **grapheme cluster**, not a codepoint — encoded in `Snapshot` from day one
  because it is ranked failure mode 2 and 32 bits per cell is structurally insufficient.
- **Bidi is a slice 5 (renderer) item and must never enter the core.** Decided 2026-07-28.
  `../ruuah/include/` has **zero** bidi/RTL surface, so reordering in the core breaks ABI
  compatibility — the project's whole thesis — and makes every RTL line diverge from the
  oracle *by construction*, deleting the only correctness signal there is. Ghostty's own
  bidi-adjacent code sits in the font shaper, not the VT core. Scar
  `~/.claude/scars/2026-06-11-bidi-terminal-deadend.md` and memory
  `feedback-no-bidi-in-terminals`: emulator bidi structurally cannot serve a cursor-addressed
  TUI, because the cursor has no mapping after reorder. **"Support most languages" is not
  bidi** — it is grapheme clusters plus correct width tables, both of which are slice 1.
- **The shipped artifact is `libruuah-vt.a`, but cargo cannot name it that.** Measured
  2026-07-28: a package called `libruuah-vt` emits `lib`**`lib`**`ruuah_vt.a`, and
  `[lib] name = "ruuah-vt"` is a hard cargo error — *"library target names cannot contain
  hyphens"*. Ghostty gets the hyphen in `libghostty-vt` because zig names artifacts freely.
  So the project is `ruuah-vt` (no `lib` prefix in a directory name — Ghostty's own project
  dir is `ghostty`), cargo emits `libruuah_vt.a`, and **slice 6 renames it to
  `libruuah-vt.a` in the build step** so RUUAH's link flag mirrors `-lghostty-vt` exactly.
  Do not try to make cargo produce the hyphen directly.

## Repo and git workflow

`Orellius/ruuah-vt`, **private**. `origin` only.

**There is no `upstream` remote, deliberately.** ruuah-vt is original code with no shared
history to track, so a second remote would be theatre. The upstream that actually matters is
the **oracle**: libghostty-vt is a moving reference implementation, and when Ghostty changes
behaviour the corpus verdicts move with it. `oracle.lock` pins the exact Ghostty commit the
current oracle was built from, `scripts/build-oracle.sh` rewrites it and **announces when the
oracle moved**. Commit `oracle.lock` whenever it changes — without it, a corpus case flipping
overnight is indistinguishable from a regression you caused.

- **`main` holds verified slices only.** Every commit on it has `cargo test --workspace`
  green and `difftest` exiting 0.
- **One branch per slice**, `slice-N-<name>`, merged with `--no-ff` so slice boundaries stay
  visible in the history. `git log --first-parent main` then reads as one line per slice.
- **Annotated tag per completed slice**, `v0.N.0`. The tag message records the corpus state
  (case counts and verdicts), the test count, and the `oracle.lock` describe string — so a
  tag answers "what worked, measured against which Ghostty" without checking anything out.
  `v0.0.0` = slice 0 harness, `v0.1.0` = slice 1 parser + grid.
- Never push a slice branch that leaves the corpus failing its own expectations. A case whose
  verdict changed is either promoted (with its note rewritten) or explained.

## Toolchain

Rust 1.93.1, edition 2024, resolver 3. `cargo test --workspace` is the gate.

```sh
./scripts/build-oracle.sh      # build libghostty-vt into vendor/ (../ruuah stays clean)
cargo test --workspace         # 32 tests
cargo run -p ruuah-vt-difftest            # the corpus report (binary is `difftest`)
cargo run -p ruuah-vt-difftest -- --dump  # plus both grids, rendered, per case
```

Env overrides: `RUUAH_VT_ORACLE_SRC` (the Ghostty checkout to build the oracle from,
defaults to `../ruuah`) and `RUUAH_VT_ORACLE_PREFIX` (a prebuilt prefix with `include/`
and `lib/`, bypassing the build script).

`vendor/` and `target/` are gitignored; the oracle is rebuilt from RUUAH, never committed.

## In-repo docs (source of truth)

- `corpus/cases.toml` — every byte stream and the verdict it is asserted to produce.
- `crates/snapshot/src/grid.rs` — what "the grid" means for comparison. The contract both
  implementations satisfy; neither owns it.
- `crates/snapshot/src/difference.rs` — how disagreement is located and reported.
- `crates/ghostty/tests/abi_layout.rs` — the ABI pin. Read before touching `sys`.
- `crates/ghostty/tests/oracle.rs` — what the oracle is known to read correctly.
- Conformance canon (not yet wired in): xterm ctlseqs, DEC STD 070, esctest2 (the CI
  target), wraptest (line-wrapping specifically), vttest (interactive, final pass only).
