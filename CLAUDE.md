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

**Slices 0 through 4 are done.**

Slice 4, `[tested]`: reflow. Resize rejoins soft-wrapped rows into logical lines, re-splits at
the new width, and maps the cursor through the transform. Scrollback takes part -- a logical
line straddling the history / active boundary is one line -- so the whole buffer is drained
into one row sequence, reflowed, and split back at the bottom. 103 tests green; 64 corpus
cases, 57 MATCH / 7 DIFF, 64/64 met expectation.

The measured rules, all against the real library on 2026-07-28 and each with a `reflow-*`
corpus case: trailing blanks are trimmed from the last row of a line only; the cursor's line
extends to reach a cursor parked past its content (which is why narrowing a full line leaves
an empty continuation row); a line with no content is deferred and emitted only if content
follows, which drops trailing blank rows while keeping interior ones; rows leaving the top
become scrollback and growth reclaims them; the alternate screen does not reflow; DECSTBM is
reset. A rows-only change does not reflow at all, because with the width unchanged there is
nothing to rejoin and rejoining anyway would grow the cursor's line by a row.

Slice 4 also fixed a slice-2 rule that nothing before it could observe: a soft wrap is marked
only when the cursor is at the last column. A deferred wrap survives a reflow verbatim, so
widening the screen leaves the cursor mid-row with the wrap still pending, and marking that
one would fuse two lines that were never one.

Slice 3, `[tested]`: paged scrollback. History is a list of fixed-capacity pages, each owning
its own cells, style table and grapheme storage, so dropping a page frees all of it. The row
budget is exact (rows are pruned individually from the front; the page is released once it
empties). Trailing blank cells are trimmed on the way in and re-padded on read, so scrollback
cost follows content rather than width. 94 tests green; 36 corpus cases, 30 MATCH / 6 DIFF,
36/36 met expectation.

Slice 2, `[tested]`: the autowrap phantom state (with DECAWM), scrolling and scroll regions
(DECSTBM, IND, RI, NEL, SU, SD), the alternate screen (47/1047/1048/1049), tab stops
(HTS, TBC, CHT, CBT), and the erase / insert / delete family (EL, ED, ECH, ICH, DCH, IL, DL),
plus DECOM, DECTCEM, DECSC/DECRC and RIS. 74 tests green; 28 corpus cases, 22 MATCH / 6 DIFF,
28/28 met expectation.

Slice 1, `[tested]`: `vte 0.15` driving a flat row-major cell array. Echo, full SGR, cursor
movement.

The seven remaining DIFF cases are **not slice boundaries**. They are named omissions kept
deliberately, because a corpus where nothing differs cannot show the harness detects
disagreement: DECALN, private/selective erase, REP, IRM, grapheme-cluster mode 2027, reverse
wraparound (mode 45), and — added by slice 4 — a **saved (DECSC) cursor sitting in the blanks
past its row's text when that row reflows**. Upstream clamps such a pin against whatever
column its sequential reflow writer happens to be parked on, a value carried over from the
*previous* line (`PageList.zig`, `reflowRow`, the `p.x >= cols_len` branch). Matching it means
emulating that writer's leftover state, and the branch next to it calls this area unspecified
outright. A saved cursor is otherwise tracked exactly like the live one: inside its row's
content it travels with its cell, and it falls back to the top-left when its row does not
survive. Both of those are corpus-pinned as MATCH.

Cell layout is settled and enforced: `Cell` is **8 bytes**, asserted at compile time in
`cell.rs`. Style is an interned `u16` into a `StyleTable`; grapheme continuations live in a
side map keyed by flat cell index. History pages carry their own tables; the active grid's
table **compacts** against live cells once it exceeds `cols * rows`, so neither grows without
bound.

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

**Deliberate divergence from Ghostty, slices 3 and 4.** Ghostty keeps the active area and the
history in ONE page list, so scrolling moves a pointer and reflow streams through the pages in
place. Here the active area stays a flat fixed-size grid (already bounded at `cols * rows`) and
only the *history* is paged, because that is where the unbounded growth was. Two costs, both
recorded rather than hidden: scrolling a row into history is O(cols) rather than a pointer
move, and a resize materialises the entire scrollback as owned rows before writing it back, so
peak memory during a resize is roughly double the scrollback. Resize is a human-driven event,
which is what makes that trade acceptable; a streaming page-at-a-time reflow is the fix if it
ever stops being.

Next: **slice 5 — render.** Glyph atlas, damage-driven redraw from a dirty-row bitset, and a
seqlock between the PTY thread and the renderer. Bidi lives here if it lives anywhere, and
never in the core (see below).

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
- **Extend the harness BEFORE building the slice. Four for four.** Every slice has had a blind
  spot that would have reported MATCH for a wrong implementation, and each was found by asking
  "can the harness even see this?" before writing code:
  - **Slice 2** — background-colour erase was invisible. Ghostty keeps a cell with only a
    background out of the style map, so `grid_ref_style` reported Default for a red cell. An
    erase ignoring BCE would have passed.
  - **Slice 3** — scrollback was invisible. The oracle ran `max_scrollback = 0` and `Snapshot`
    held only the active area, so any history implementation would have passed.
  - **Slice 4** — resize was not expressible at all: `Case` had no resize field and neither
    `Terminal` had a `resize` method, so any reflow would have passed. `Case` gained `resize`
    and `after`, and both were needed: `after` is the only way a grid comparison can see where
    the cursor landed, because a cursor that never writes leaves no trace. With the harness
    extended and a non-reflowing resize in place, wrapped cases DIFFed and flat ones MATCHed —
    both directions, before a line of reflow was written.
  Treat this as the project's dominant risk, not a coincidence. The differential harness only
  catches what the `Snapshot` represents, so the first question of every slice is what new
  observable it needs.
- **Read the oracle's source before inferring its behaviour.** `../ruuah` is a Ghostty
  checkout, so `src/terminal/PageList.zig` and `src/terminal/Screen.zig` are the reference
  implementation of everything the ABI exposes. Slice 4 burned five rounds of black-box probes
  on the saved-cursor mapping and got three mutually contradictory rules; the answer was
  twenty lines of `reflowRow`. Probe to find out WHAT differs, read the source to find out WHY.
  Both matter — the probes are what made the corpus, and the source is what stopped the
  guessing.
- **A corpus `expect = "diff"` is a to-do, not a pass.** When ruuah-vt implements that behaviour
  the case *fails*, and it gets promoted to `expect = "match"`. That is the mechanism, not a
  nuisance: a harness that cannot be wrong is not evidence. `tests/corpus.rs` additionally
  refuses a corpus that has drifted to a single direction.
- **Ghostty's scrollback limit is a memory budget scaled by WIDTH, not a row count.** Measured
  2026-07-28 against the real library: `max_scrollback` behaves as a boolean (0 disables, any
  non-zero value behaves the same), and writing 3000 lines kept **2998** rows at 6 columns but
  only **634** at 80. This core budgets in rows instead. The two prune POLICIES are therefore
  not comparable and must never be corpus-tested against each other — every scrollback case
  stays far under both thresholds, where contents agree exactly, and the policy is unit-tested
  in `history.rs`. This is the plan's ranked failure mode 4, confirmed on the real thing.
- **Only rows leaving the TOP OF THE SCREEN become history.** A scroll region that starts
  below row 0 pushes rows out inside the screen, not off it, and `delete_lines` removes them
  outright. Neither feeds scrollback; there is a corpus case pinning each.
- **The alternate screen has no scrollback**, by protocol. It is constructed with a zero
  budget, so this is structural rather than a check that can be forgotten.
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
- `crates/core/src/reflow.rs` — the re-lay itself, as a pure transform over rows. Every
  non-obvious rule in it carries the measurement it came from.
- `crates/core/src/resize.rs` — the storage round-trip around that transform: drain, reflow,
  split back into history and active area.
- `crates/snapshot/src/difference.rs` — how disagreement is located and reported.
- `crates/ghostty/tests/abi_layout.rs` — the ABI pin. Read before touching `sys`.
- `crates/ghostty/tests/oracle.rs` — what the oracle is known to read correctly.
- Conformance canon (not yet wired in): xterm ctlseqs, DEC STD 070, esctest2 (the CI
  target), wraptest (line-wrapping specifically), vttest (interactive, final pass only).
