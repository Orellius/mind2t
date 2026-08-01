# esctest2: measured against Ghostty, re-measured 2026-08-01

**This document was rewritten after the first measurement turned out to be invalid.**
The original numbers (Ghostty 141, ruuah-vt 126) were taken under esctest's DEFAULT
checksum convention, which negates every DECRQCRA reply it reads. That flag decides
whether a terminal's screen can be read back at all, so it was never a like-for-like
comparison. The correction and what it exposed are below.

Both terminals ran the VENDORED suite (`esctest.lock` pin), same 80x25, same log
grammar parsed both times: `--expected-terminal=xterm --max-vt-level=4 --timeout=1
--include=.`. Ghostty 1.3.1 (`ghostty --version`) from /Applications, driven with
`-e`; ruuah-vt through its own pty host (`crates/pty/tests/esctest.rs`).

## The instrument was mis-set, and it only ever mattered for one side

`--xterm-checksum` tells esctest which DECRQCRA convention to expect. The default (0)
assumes a pre-279 xterm that returns NEGATED sums; modern xterm (patch 334+, 2018)
returns positive ones, and so does this core. Under the default, every
`AssertScreenCharsInRectEqual` in the suite compared a negated number against a
positive one, so **not a single content-asserting test had ever passed here** - across
EL, ICH, DCH, IND, RI, BS, CR, TAB and more. Declaring `--xterm-checksum=334`
promoted 118 tests with zero regressions.

Ghostty was then re-run under both conventions to be fair to it:

| Ghostty 1.3.1 | passed | failed | skipped |
|---|---|---|---|
| `--xterm-checksum=0` (the old default) | 140 | 411 | 17 |
| `--xterm-checksum=334` | **142** | 409 | 17 |

**Two points of difference. The flag is nearly inert for Ghostty, and the reason is
the finding that matters: Ghostty does not implement DECRQCRA at all.** Verified by
source (`grep -rn DECRQCRA ../ruuah/src/` returns nothing; its stream has no `*y`
dispatch) and independently by the numbers above - a convention flag cannot move a
terminal that never answers the query it governs.

## What the comparison actually measures

| terminal | passed of 568 |
|---|---|
| Ghostty 1.3.1 | 142 |
| ruuah-vt (this repo, at v0.15.0) | **368** |

Overlap: **130 both pass · 12 only Ghostty · 238 only ruuah-vt.**

**That 238 is not 238 behaviours Ghostty gets wrong.** esctest verifies screen content
by asking the terminal to checksum a rectangle. A terminal that cannot answer fails
every such test regardless of whether its grid is correct. Ghostty's ED, EL, ICH, DCH
and scrolling are very probably fine; this instrument simply cannot see them. The
honest statement of the gap is:

> ruuah-vt implements the readback query esctest needs, so its content behaviour is
> measurable and measured. Ghostty's is not measurable by this suite. The score
> difference is mostly instrumentation, not correctness.

That also revises the original framing, which was already careful to call Ghostty a
peer rather than a standard, but still treated the counts as comparable. They are not.
Where the two are genuinely comparable is the 12 tests Ghostty passes and we do not.

## The 12 tests only Ghostty passes

```
BSTests.test_BS_StopsAtLeftMargin
CUBTests.test_CUB_StopsAtLeftMarginInScrollRegion
CHTTests.test_CHT_IgnoresScrollingRegion
DECALNTests.test_DECALN_ClearsMargins
DECALNTests.test_DECALN_MovesCursorHome
DECIDTests.test_DECID_Basic
DECRQMTests.test_DECRQM_DEC_DECCOLM
DECRQMTests.test_DECRQM_DEC_DECLRMM
DECRQSSTests.test_DECRQSS_DECSLRM
DECSCLTests.test_DECSCL_Level2DoesntSupportDECRQM
DECSETTests.test_DECSET_DECAWM_NoLineWrapOnTabWithLeftRightMargin
DECSETTests.test_DECSET_ReverseWraparound_Multi
```

One of these is already closed: **`DECRQSS_DECSLRM`** was fixed the moment this
re-measurement named it. We answered "invalid" for DECSLRM, which was correct while
margins did not exist and went stale the instant they landed. It now reports the live
band and is pinned - the re-measurement paying for itself.

The rest are ranked, with the reason each is open:

- **`BS_StopsAtLeftMargin` and `CUB_StopsAtLeftMarginInScrollRegion`** are the
  interesting pair. This core matches the ORACLE, which clamps a plain CUB to column 0
  and ignores the left margin (its `cursorLeft` fast path skips the margin logic
  whenever no reverse-wrap mode is active, `Terminal.zig:1766`) - and there is a corpus
  case pinning that agreement. The shipped Ghostty 1.3.1 app passes the esctest
  expectation instead. **These are two different Ghostty versions**: the differential
  oracle is built from the `../ruuah` checkout, the comparison target is the release
  app, and they have diverged here. Nothing to fix without picking which Ghostty to
  follow; the corpus follows the oracle, deliberately, because that is the drop-in
  contract.
- **`DECALN_*` (2)** - DECALN is a corpus-pinned unimplemented divergence.
- **`DECRQM_DECCOLM` / `DECRQM_DECLRMM`** - DECCOLM is column resize (refused here on
  purpose); DECLRMM now exists, so its DECRQM answer is a small follow-up.
- **`DECSCL_*` (1)** and **`DECID_Basic`** - conformance-level tracking and DECID,
  neither implemented.
- **`ReverseWraparound_Multi`** - reverse wrap (mode 45), a named unimplemented mode.
- **`CHT_IgnoresScrollingRegion`** and **`DECAWM_NoLineWrapOnTabWithLeftRightMargin`** -
  tab interaction with margins, the one margin edge the DECLRMM slice did not close.

## The standing honesty clause

esctest measures conformance to XTERM, not terminal quality. 368 beating 142 means
nothing to someone using either app, and after this re-measurement it means even less
than it looks, because most of the difference is one missing query rather than 238
behavioural gaps. It is a good scoreboard only because it is external and unarguable -
and only when the instrument is set correctly for both sides, which it was not until
today.

Method note for anyone repeating this: the Ghostty runs used
`scripts/` equivalents in the session scratchpad, driving
`/Applications/Ghostty.app/Contents/MacOS/ghostty --window-width=80 --window-height=25 -e python3 <suite>`
with the flags above. Re-measure before quoting any number here; both sides move.
