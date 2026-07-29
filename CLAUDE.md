# CLAUDE.md — ruuah-vt (project child config)

> **Parent stack layer:** `../CLAUDE.md` (tools/ 2026 stack, auto-inherited; don't restate it).
> Chain: `~/.claude/CLAUDE.md` (contract) → `Studio/CLAUDE.md` (index) → `tools/CLAUDE.md` (stack) → **this file (ruuah-vt specifics)**.
> This file = project specifics only. Last update stamp: 2026-07-28 (IDT).
> Posture: the global proactive co-pilot rule (initiative, three-steps-ahead, extreme ownership) is in force here via `~/.claude/CLAUDE.md`.

## What this is

A terminal core in Rust implementing the C ABI Ghostty already publishes as
**`libghostty-vt`**, so it can drop in behind an existing native GUI. The point is control
and craft, not speed — Rust and Zig are peers here and there is no performance win waiting.

**There is no consumer yet, and the note that used to sit here was wrong.** Measured
2026-07-28: RUUAH's Swift app calls **99 `ghostty_*` symbols and not one of them is a VT-core
symbol** -- they are all app-level (`ghostty_app_*`, `ghostty_surface_*`, `ghostty_config_*`,
`ghostty_inspector_*`). Ghostty's own app never routes through `src/terminal/c/` either; that
layer is reached only by `src/lib_vt.zig`, which exists for external embedders. So "RUUAH swaps
one link flag" was never true: swapping ruuah-vt in behind RUUAH's existing Swift app would
mean reimplementing the *application* ABI, which is a different project.

What is true, and what slice 6 makes testable: ruuah-vt can export the same C ABI
`libghostty-vt` publishes, so **something built against that ABI** can link either. The planned
consumer is a minimal Swift host (after slice 6), not RUUAH as it stands.

**The one architectural rule: the core is a pure, deterministic state machine.** Bytes in,
grid mutations out. No PTY, no GPU, no clock, no I/O. Everything else hangs off this,
because it is what makes headless CI and differential testing possible at all. Ghostty
enforces the same split physically (`src/terminal/` knows nothing about `src/renderer/`).

Plan of record: `~/.claude/plans/2026-07-28-rust-terminal-core.md`.
Architecture research it came from: `~/Desktop/claude-html/terminal-architecture-20260728-0132.html`.

## Status / current slice

**Slices 0 through 7 are done**, tagged `v0.7.0`, and the **S1 audit wave** on top of them is
tagged `v0.7.1`. 252 tests green; corpus 124 cases, 98 match / 26 diff, 124/124 met
expectation; `libruuah-vt.a` at 13/13 exports.

**The S1 wave, `[tested]`.** A full audit on 2026-07-28 found 31 defects, 7 at S1, and the
pattern behind nearly all of them was that **the harness rule had been applied to the core and
never to the comparator or the ABI**. Ten of the 24 fields `difference.rs` compares had no test
that failed when the comparison was deleted; because 90 of 97 cases expected MATCH, deleting a
comparison only ever made more snapshots agree. So the two harness repairs landed FIRST, and
everything after them was measured against a gate that could actually go red.

All seven are fixed, each with a control that was run against the broken version and seen to
fail: the GPU op log now retires on read rather than replaying all history (hard ceiling was
1,048,576 ops); a seqlock read interrupted mid-copy invalidates the caller's frame instead of
leaving it wearing a stale generation; CUU/CUD/CNL/CPL are bounded by the scroll region;
`ED 3` clears the scrollback; `ghostty_terminal_new` NULLs its out-param on failure; the
comparator's damage/screen/cursor comparisons have controls; and a corpus that loses cases is
refused rather than reported on.

Three things worth keeping from it, because each cost a wrong first attempt:

- **A control aimed at the wrong workload proves nothing.** The seqlock's tearing harness ran
  a writer flat out, which holds the counter odd almost continuously, so reads bail before
  copying: **6,802,136 skips in one run, zero of them the path being tested.** The bug needed
  an orchestrated workload (large grid, writer publishing 60us after the reader enters the
  copy) to reach it about half the time.
- **Self-consistency was the wrong question there too.** An interrupted copy usually finishes
  with clean content wearing the wrong generation, so every cell agrees with every other one.
  The test asks whether the frame was TOUCHED instead.
- **A check placed before the thing it guards does not guard it.** The corpus size floor in
  `load` passes, then the loader truncates on the way out -- re-running the original mutation
  with the floor in place still exited 0. The fix is a count re-derived from the raw file text
  in `tests/corpus.rs`, which shares no code with the loader.

**The S2 wave is in progress on branch `audit-fixes-s2`** (2026-07-29). Findings 8-12 are
fixed and committed, each with a control seen to fail first; gates green after every one
(253 tests, corpus 124 cases 102 match / 22 diff 124/124, 13/13 exports).

- 8 `ed5c115` DECRC with nothing saved restores the synthetic default cursor, not nothing.
- 9 `58378d1` HT no longer clears `pending_wrap`; upstream's horizontalTab never touches it.
- 10 `6feed8d` reflow gated on DECAWM (`reflow = modes.get(.wraparound)`).
- 11 `5239f3c` ED 2 at a prompt scrolls into scrollback first. Two halves: the row count comes
  from the last row holding text (`PageList.zig:3099`), and the cursor follows its tracked pin,
  homing ONLY when that row left the active area (`Screen.zig:844` `cursorReload`). The first
  attempt got the grid right and the cursor wrong, and **difftest exited 0 while still wrong** --
  a case declared `diff` stays green whether or not the fix worked, so the flip to MATCH is the
  only real signal. Read the dump, not the exit code.
- 12 `5080cb2` `STYLE_ID` had dead bits 25-40. The id is **derived from the style**, not a table
  index: upstream's number is its own allocation order and this ABI resolves styles by grid
  position, so there is no table to index. Guarantees provided: 0 means default, equal styles
  mean equal ids. Deliberate, documented divergence -- revisit if a consumer ever needs the id
  as a lookup key.

Findings 13 through 31 (rest of S2, then S3/S4) remain open in the audit's order, then slice 8.

Slice 5.6, `[tested]`: **OSC 133 and the visual caret.** The core tracks prompt / input /
output regions (`crates/core/src/semantic.rs`), and the renderer draws the caret at
`Frame::visual_column` rather than at `cursor.x` -- on a reordered row those are different
columns, and the glyph at the wrong one is a real character, which is why the old behaviour
looked entirely reasonable.

Sixteen corpus cases went in as `expect = "diff"` before the core could do anything and were
promoted to `expect = "match"` by the implementation. That promotion is the evidence: every
one disagreed with libghostty-vt before and agreed after, with the 71 pre-existing matches
never moving.

Three rules measured rather than assumed, each one a plausible implementation gets wrong:
**`B` and `I` diverge only across a soft wrap** (`index()` resets the clear-at-EOL state and
`printWrap` restores it); **`C` at column 0 un-marks the row and mid-row does not** (the rule
is positional, not content-based); and **reflow gives each destination row the mark of the
last source row that reached it** -- taking the last source row's mark unconditionally passes
a split and a rejoin and fails a two-into-three re-split, which is why `split` reports the
content offset each output row ended on.

`step_visually` is the arrow half and is deliberately **not** gated on the cell being input.
`Frame::is_input` is the gate and it belongs where the key event is handled, because outside
an input region the cursor is the running program's to place. There is no key encoder in this
repo, so the arrow work stops at the pure mapping; the GUI is what turns it into bytes.

Slices 0 through 5.5 detail follows.

Slice 5.5b, `[tested]`: **shaping.** `crates/render/src/shape.rs` runs each cell's cluster
through swash so combining marks are placed by the font's GPOS table instead of their own
bearings. Only clusters with more than one codepoint pay for it; a lone character takes
`place_at_origin`, which for one glyph is not an approximation but the exact answer.

The blind spot, eighth in a row: nothing could see where a mark LANDS. Every earlier test is
satisfied by a renderer that draws a niqqud at the cell's left edge -- the ink is still ink,
still inside the cell, still changes the pixels. Measured at 32px on a 19px cell, an unshaped
`בָּ` puts **9 pixels of ink in column 0** and a shaped one puts **none**. So the assertion is
positional and two-directional in one test: shaped, the leftmost columns must be empty;
unshaped, they must not be. `tests/shaping.rs` also guards the opposite failure -- a renderer
that "fixes" placement by dropping the marks entirely would satisfy emptiness perfectly, so a
second test requires the marks to add ink and to reach below the base.

**One cell is shaped at a time, deliberately.** A terminal cell is addressable, so shaping
across cells would let the renderer merge or re-space cells the program placed on purpose. The
cost is that Arabic contextual joining does not cross cells -- a limitation every terminal
shares, not a bug here.

Slice 5.5a, `[tested]`: **display bidi.** Hebrew now reorders. `crates/frame/src/bidi.rs`
lays a row out with `wezterm-bidi` under it, and the seam held exactly as designed -- the
renderer was **not touched**, because it never assumed a direction and asks `Run::column_of`
for every column it paints. 184 tests green; corpus untouched at 78/78, which is the proof
that bidi stayed out of the core.

Measured, not eyeballed: `crates/frame/tests/bidi_conformance.rs` runs **all 91,707
BidiCharacterTest cases through our own `visual_spans`** and passes, with 2,162 of the first
4,000 requiring real reordering so the comparison provably distinguishes visual from logical.
The suite caught a genuine bug on its first run -- the fast path that skips the algorithm for
plain text ignored the base direction, and under an RTL base even a row of pure neutrals
resolves to level 1 and reverses.

Verified by hand afterwards, through the real terminal path:

| logical | visual |
|---|---|
| `שלום עולם` | `םלוע םולש` (reads correctly right to left, word order kept) |
| `let msg = "שלום";` | `let msg = "םולש";` (code stays put) |
| `גיל 42 שנה` | `הנש 42 ליג` (**the number stays 42, not 24**) |
| `│אבג│abc│` | `│גבא│abc│` (bars stay in columns 0, 4, 8) |

Still missing in 5.5: **shaping**. A cluster's codepoints are still rasterized individually
and drawn at one pen position, so niqqud land on default bearings rather than where GPOS puts
them. `swash` shapes and its Hebrew GSUB/GPOS is verified, so this adds no dependency.

Slice 5, `[tested]`, tagged `v0.5.0`: all five acceptance criteria met. What remains from it
is the GPU backend, which is slice 6 territory.

Slice 5 step 4, `[tested]`: **it renders vim.** `ruuah-vt-render` is a CPU rasterizer -- glyph
atlas, xterm palette, damage-driven redraw -- built on `swash`. The backend is on the CPU
deliberately: the atlas, the run-to-column mapping and the damage logic are all
backend-agnostic, and only the final blit is not, so putting the reference backend here is
what lets "renders vim" be an assertion in CI rather than a screenshot somebody looked at
once. 168 tests green; corpus unchanged.

The harness went first again and the blind spot was, again, total -- nothing in the project
could see a **pixel**, so a renderer that drew nothing at all satisfied every existing test.
The invariant that closes it: **painting a sequence of frames incrementally must produce the
same bytes as painting the final frame in full.** One equality covers a row that changed
without being marked, a row marked but not repainted, stale pixels from a shortened line, and
a glyph overhanging into a row that is never redrawn. Proven able to fail by
`a_renderer_that_skips_a_stale_row_is_caught`, which runs the identical load through a
renderer that declines one row.

Criterion 5 is satisfied structurally rather than by promise: every column the renderer paints
comes from `Run::column_of`. Hebrew reaches the canvas today through the fallback font, in
logical order, and `hebrew_is_drawn_in_logical_order_today_and_slice_5_5_must_flip_this` pins
that by pixels -- when 5.5 reorders, that test fails, and the failure is the feature.

Slice 5 step 3, `[tested]`: the frame channel and the pty host, in two new crates.
`ruuah-vt-frame` publishes a whole frame from the parse thread to a renderer through a
**seqlock** -- a generation counter that is odd while a publish is in flight, so a reader
that arrives mid-write discards that frame instead of drawing half of one. `ruuah-vt-pty`
owns the pseudoterminal and the child, keeps the `Terminal` on its own pump thread, and is
the only crate in the project that performs I/O. 137 tests green; corpus unchanged at 78
cases, 71 match / 7 diff, 78/78 met expectation.

The harness came first here too, and this time the blind spot was total: a seqlock whose
reader ignores the counter passes every single-threaded test ever written against it.
`crates/frame/tests/tearing.rs` runs a writer and a reader flat out against a frame carrying
a self-consistency invariant, and **proves the invariant is sensitive** by running the same
load through a reader with the protocol removed, which must observe tearing. Both directions
were then confirmed by mutation: deleting the reader's `generation != before` re-check makes
the passing test fail with a real torn frame (`cell (1,0) is from publish 16 but (0,0) is
from 15`).

Slice 5 step 2, `[tested]`: damage tracking -- per-row dirty flags, a whole-frame flag, and
the cursor's contribution to both, measured against the oracle's `GhosttyRenderState`.

Slice 5 step 1, `[tested]`: the harness learned to see damage at all. `Snapshot` represented
neither dirty layer, so any damage implementation would have reported MATCH.

Slice 6 (the C ABI) and slice 7 (the GPU backend) both landed after this, and the shipped
artifact is now built and export-checked by `scripts/build-lib.sh`.

Next after the audit backlog: **slice 8, a minimal Swift host.** Not RUUAH -- measured
2026-07-28, its Swift app calls 99 `ghostty_*` symbols and not one is a VT-core symbol. The
host is what makes slice 7's GPU backend mean anything (it renders into a buffer nobody
displays today), and its key-encoder path is the same `Host::send` seam esctest2 needs for
DSR/DA replies, so slice 9 gets its blocker removed as a by-product.

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
back via DSR/DA query responses. The PTY host now exists (`ruuah-vt-pty`, slice 5 step 3), so
the remaining blocker is the *reply* half: the core still has no DSR/DA response path, because
answering a query means writing bytes back, and the core does no I/O. The seam for it is
`Host::send`. The differential corpus covers the same semantics without either.

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

Bidi lives in the renderer if it lives anywhere, and never in the core (see below).

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
- **Extend the harness BEFORE building the slice. Nine for nine.** Every slice has had a blind
  spot that would have reported MATCH for a wrong implementation, and each was found by asking
  "can the harness even see this?" before writing code. Three of the nine were total:
  concurrency, pixels, and -- in slice 5.6 -- OSC 133, where `Snapshot` had no semantic surface
  at all, so a core with zero OSC handling scored a perfect match on every prompt and input
  region. A fourth, the caret's landing column, was invisible to the pixel harness itself:
  `redraw.rs`'s incremental-equals-full invariant holds whether the caret is on the right cell
  or the wrong one, because both renderers are consistently wrong.
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
- **The bidi oracle is Unicode, and it is not optional.** `./scripts/fetch-ucd.sh` vendors
  `BidiTest.txt` and `BidiCharacterTest.txt` into `vendor/ucd/`; `ucd.lock` pins the revision,
  so a conformance failure after a Unicode update can be told apart from a regression you
  caused. libghostty-vt cannot play this role -- it has no bidi surface at all.
- **`wezterm-bidi` was chosen by measurement, not by reputation.** Verified 2026-07-28 by
  running it over the whole suite: **770,241 / 770,241** BidiTest cases and **91,707 / 91,707**
  BidiCharacterTest cases, on paragraph level, resolved levels and visual order. It also fits
  the data model -- `resolve_paragraph(&[char], hint)` and level runs with contiguous logical
  ranges, which is why reordering did not change the shape of a `Run`. `unicode-bidi` (servo)
  is byte-indexed and would need an adapter for a cell grid.
- **The terminal's base direction is LTR, deliberately, and it is a policy not a bug.** A grid
  is column-addressed by the program drawing into it, so auto-detection would move a Hebrew
  status line written at column 0 to the right edge. With an LTR base, RTL runs still reorder
  within their own span. `BaseDirection::Auto` exists for a caller that wants it.
- **Segments are bounded by box drawing and block elements** (U+2500..=U+259F). Whole-line UBA
  across a table moves the frame characters relative to the text they enclose; bounding at the
  frame keeps every box where the program drew it. The Unicode suite has no opinion about this
  and cannot catch a wrong choice here, so it is unit-tested separately in `bidi.rs`.
- **Bidi is a renderer-layer item and must never enter the core.** Decided 2026-07-28, and
  slice 5.5 confirmed it: reordering landed with the corpus untouched at 78/78.
  `../ruuah/include/` has **zero** bidi/RTL surface, so reordering in the core breaks ABI
  compatibility — the project's whole thesis — and makes every RTL line diverge from the
  oracle *by construction*, deleting the only correctness signal there is. Ghostty's own
  bidi-adjacent code sits in the font shaper, not the VT core. Scar
  `~/.claude/scars/2026-06-11-bidi-terminal-deadend.md` and memory
  `feedback-no-bidi-in-terminals`: emulator bidi structurally cannot serve a cursor-addressed
  TUI, because the cursor has no mapping after reorder. **"Support most languages" is not
  bidi** — it is grapheme clusters plus correct width tables, both of which are slice 1.
- **Darwin refuses `TIOCSWINSZ` on the pty MASTER.** Measured 2026-07-28 on macOS 25.5, and
  confirmed with raw `libc::ioctl` as well as through rustix, so it is a kernel rule and not
  a binding bug: setting the window size on the master returns `ENOTTY` (errno 25,
  "Inappropriate ioctl for device"). It must go to the **user side**; reading it back with
  `TIOCGWINSZ` works from either end. Linux accepts both, which is exactly why this is easy
  to write wrong and only fails on the machine the project targets. `host.rs` therefore
  *reopens* the pts by path for each resize rather than holding a slave fd — holding one open
  would mean the master never reports EOF when the child exits, because this process would
  still have the other end open. Cost: eight failing integration tests with a misleading
  errno. Do not re-derive it.
- **The seqlock payload is `AtomicU64` read and written `Relaxed`, and there is no `unsafe`
  in it.** A classic seqlock races the reader against the writer, which in Rust's model is a
  data race and therefore undefined — the usual workarounds are `read_volatile` or a raw
  `copy_nonoverlapping`, both of which are formally still UB. The way out is that the standard
  library defines a data race as requiring a *non-atomic* access, so relaxed atomic loads that
  race are merely unordered, never undefined; the generation counter's `Acquire`/`Release`
  pair supplies the ordering that makes a set of atomic words into one consistent frame. This
  is why `Cell` being exactly 8 bytes pays off twice: one cell is one `u64`.
  The `seqlock` crate (0.2.0, May 2026) was checked and does not fit — it requires `T: Copy`
  and has no dynamically-sized payload, and a terminal grid is sized at runtime.
- **The renderer's only input is `Frame::runs`, and a `Run` carries a `Direction`.** Nothing
  produces a `RightToLeft` run yet; the variant and `Run::column_of` exist so that slice 5.5
  changes the run builder and not one line of drawing code. A renderer that adds an index to
  `run.start` itself compiles fine today and draws every Hebrew line backwards later. This is
  slice 5's acceptance criterion 5, built before the renderer rather than after it.
- **A published cell carries 16 bytes of inline UTF-8, not a pointer into an arena.** An arena
  needs an offset and a length, and a torn offset/length pair is the one thing that could turn
  a discarded frame into a fault. Sixteen bytes holds a base letter plus roughly seven
  combining marks, which covers Hebrew niqqud with room over; a longer cluster (multi-person
  emoji ZWJ sequences) is **flagged** via `PackedCell::is_truncated`, never silently shortened.
- **No single font can draw this terminal, and the font question is now settled.** Measured
  2026-07-28 by walking every font in `/System/Library/Fonts`, its `Supplemental`,
  `/Library/Fonts` and `~/Library/Fonts`, then shaping Hebrew through each candidate.
  - Menlo maps Hebrew to glyph 0. Arial Hebrew maps `'A'` to glyph 0, and is proportional
    besides. Fallback is therefore required, not an enhancement — which is why `FontStack`
    is plural from its first commit and the atlas keys on **(font, glyph)** rather than
    glyph. A glyph id without its font is meaningless, and collapsing the two would draw
    Hebrew with Menlo's glyph numbering.
  - **The Hebrew font is `Miriam Mono CLM`** (Culmus, Maxim Iorsh, GPL v2), installed at
    `~/Library/Fonts/`. It is the only monospace font found that does Hebrew *correctly*:
    GSUB composes shin+shin-dot and bet+dagesh into single glyphs, GPOS puts a qamats at
    exactly half the advance so it is centred under its base, marks carry **zero advance** so
    a pointed cluster stays ONE cell, and Latin and Hebrew both advance 0.6em — the same as
    Menlo, so the two share a grid exactly.
  - **It must not lead the stack**: it covers **0 of 128** box-drawing codepoints, 0 blocks
    and 0 powerline. Menlo leads, Miriam sits behind it, Arial Hebrew is the last resort so a
    machine without Culmus still works. `system()` filters to what is installed.
  - Iosevka and JetBrains Mono have no Hebrew at all, so the popular programming faces are
    not options. Building a font is not one either — see the note below.
  - `font.rs` unit-tests the coverage gaps in both directions, so a font change on the
    machine surfaces as a failing test instead of tofu on screen.
- **Do not try to build or merge a font.** A Hebrew-plus-Latin monospace face with correct
  niqqud is person-years of type design, and the shortcut — merging Menlo's Latin and box
  drawing with Miriam's Hebrew via fontTools — buys nothing the fallback stack does not
  already give (the advances already match) while creating a licence problem: Menlo is
  Apple's and not redistributable, and Miriam is GPL v2, so the merged output could never
  ship. Fallback is the answer, and it is already built.
- **The renderer has no shaping yet, and that is the honest gap.** A cluster's codepoints are
  rasterized individually and drawn at one pen position, which is right for Latin and only
  approximate for combining marks: a niqqud lands where the font's default bearings put it,
  not where GPOS mark attachment says it belongs. Real stacking is slice 5.5. `swash` was
  chosen partly because it also does shaping, so 5.5 adds no new dependency; `cosmic-text`
  pairs HarfRust with swash if its shaper turns out to be the better one, and 5.5 has a
  conformance oracle to decide that with rather than a preference.
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

Dependencies stay deliberately few: `vte`, `unicode-width`, `thiserror`, `serde`, `toml`,
`rustix` (slice 5 step 3), `swash` (step 4, font parsing + rasterization + the shaper 5.5 will
need) and `wezterm-bidi` (5.5, chosen by conformance measurement). **`wgpu` is the one large
exception** (slice 7): measured at +123 packages against +9 for a Mac-only `objc2-metal`
backend, taking the tree from 80 to 181. Orel chose it over the Metal route with those numbers
in hand, for portability the roadmap does not yet want but may. `portable-pty` was evaluated for the pty host and rejected — on
macOS it costs thirteen crates including `serial2`, a serial-port library, and a second
`thiserror` major version alongside the workspace's. `rustix` costs three and the pty dance is
about sixty lines we own. **`cargo fmt --all` reformats the whole repo**, which was never
rustfmt-clean; format only the files a change actually touches, or the diff drowns.

```sh
./scripts/build-lib.sh         # build the shipped libruuah-vt.a and verify its 13 exports
./scripts/build-oracle.sh      # build libghostty-vt into vendor/ (the core's oracle)
./scripts/fetch-ucd.sh         # vendor the Unicode bidi suite (the reordering oracle) (../ruuah stays clean)
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
- `crates/frame/src/seqlock.rs` — the thread handoff. Read the module card before touching the
  ordering; every `fence` in it is load-bearing.
- `crates/frame/src/frame.rs` — what a renderer is allowed to draw from. The `Run` /
  `Direction` seam that keeps bidi out of the renderer.
- `crates/frame/tests/tearing.rs` — the concurrency harness, including the control that proves
  it can fail.
- `crates/pty/src/host.rs` — the only I/O in the project, and the one `unsafe` block.
- `crates/render/src/renderer.rs` — the consumer the `Run` seam exists for. Every column comes
  from `Run::column_of`.
- `crates/render/src/font.rs` — why the font stack is plural, with the measurement.
- `crates/render/tests/redraw.rs` — the pixel harness: incremental equals full, plus the
  control that proves it can fail, plus the logical-order pin 5.5 must flip.
- `crates/render/tests/vim.rs` — the acceptance gate. Writes a BMP to the temp dir for eyes.
- `crates/frame/src/bidi.rs` — reordering, and the two terminal policies on top of the UBA
  (LTR base, segments bounded by box drawing).
- `crates/frame/tests/bidi_conformance.rs` — the Unicode oracle, run against our own layout.
- `crates/core/src/semantic.rs` — OSC 133, and the three places its rules apply at once.
- `crates/ghostty/tests/semantic.rs` — what the oracle is known to do with OSC 133, measured.
- `crates/render/tests/caret.rs` — where the caret lands, found by diffing shown against
  hidden, plus the control that pins the old logical placement.
- `crates/render/src/surface.rs` — the backend seam, and the specified integer blend both
  backends must produce. Also holds the truncating control.
- `crates/render/src/gpu.rs` — the wgpu compute backend, and why the sRGB trap does not bite.
- `crates/render/tests/backend.rs` — CPU against GPU, byte for byte.
- `crates/abi-types/src/lib.rs` — the C types this library publishes. Depends on nothing so
  it can be linked beside the oracle without a symbol clash.
- `crates/abi/src/exports.rs` — the C entry points, thin on purpose.
- `crates/abi/tests/differential.rs` — the whole corpus driven through the C ABI and compared
  against the Rust API, plus the wrong-row control.
- `crates/ghostty/tests/abi_parity.rs` — our published layouts against libghostty-vt's own
  `ghostty_type_json()`. Caught a real `GhosttyPoint` size bug on its first run.
- `scripts/build-lib.sh` — builds `libruuah-vt.a` and verifies its exports.
- `crates/ghostty/tests/abi_layout.rs` — the ABI pin. Read before touching `sys`.
- `crates/ghostty/tests/oracle.rs` — what the oracle is known to read correctly.
- Conformance canon (not yet wired in): xterm ctlseqs, DEC STD 070, esctest2 (the CI
  target), wraptest (line-wrapping specifically), vttest (interactive, final pass only).
