# esctest2: measured against Ghostty, 2026-08-01

Both terminals ran the VENDORED suite (esctest.lock pin), same flags, same 80x25:
`--expected-terminal=xterm --max-vt-level=4 --timeout=1 --include=.`
Ghostty 1.3.1 (`ghostty --version`) from /Applications/Ghostty.app, driven with `-e`; ruuah-vt through
its own pty host (`crates/pty/tests/esctest.rs`). Same log grammar parsed both times.

| terminal | passed | failed | skipped |
|---|---|---|---|
| Ghostty  | 141 | 383 | 44 |
| ruuah-vt | 120 | 403 | 45 |

Overlap: **104 both pass · 37 only Ghostty · 16 only ruuah-vt**.

The headline is not the 21-test gap, it is that BOTH fail roughly three quarters of
the suite. esctest asserts xterm's behaviour and neither terminal is xterm. Ghostty is
a peer with a different set of holes, not a distant standard - and 16 tests pass here
that fail there, several of them because this core implements DECSTR and Ghostty has
no `!` intermediate dispatch at all (the divergence already corpus-pinned in CLAUDE.md).

## The 37 to close, ranked by cluster

- 4 DECRQSSTests
- 4 DECRQMTests
- 3 VPRTests
- 3 ChangeDynamicColorTests
- 3 ChangeColorTests
- 2 ResetColorTests
- 2 HPRTests
- 2 DECALNTests
- 2 CRTests
- 2 CPLTests
- 2 CNLTests
- 1 SCORCTests
- 1 DECSETTests
- 1 DECSCLTests
- 1 DECIDTests
- 1 CUFTests
- 1 CUBTests
- 1 CHTTests
- 1 BSTests

Twelve of the thirty-seven are plain cursor-movement edges (CNL, CPL, CR, HPR, CUF,
CUB, CHT, BS, VPR) - the cheapest wins in the list. The colour OSC family (8) and the
DECRQSS/DECRQM query pair (8) are the two real features behind it.

## Tests only Ghostty passes
```
BSTests.test_BS_StopsAtLeftMargin
CHTTests.test_CHT_IgnoresScrollingRegion
CNLTests.test_CNL_StopsAtBottomLineWhenBegunBelowScrollRegion
CNLTests.test_CNL_StopsAtBottomMarginInScrollRegion
CPLTests.test_CPL_StopsAtTopLineWhenBegunAboveScrollRegion
CPLTests.test_CPL_StopsAtTopMarginInScrollRegion
CRTests.test_CR_MovesToLeftMarginWhenRightOfLeftMargin
CRTests.test_CR_StaysPutWhenAtLeftMargin
CUBTests.test_CUB_StopsAtLeftMarginInScrollRegion
CUFTests.test_CUF_StopsAtRightMarginInScrollRegion
ChangeColorTests.test_ChangeColor_Hash6
ChangeColorTests.test_ChangeColor_Multiple
ChangeColorTests.test_ChangeColor_RGB
ChangeDynamicColorTests.test_ChangeDynamicColor_Hash6
ChangeDynamicColorTests.test_ChangeDynamicColor_Multiple
ChangeDynamicColorTests.test_ChangeDynamicColor_RGB
DECALNTests.test_DECALN_ClearsMargins
DECALNTests.test_DECALN_MovesCursorHome
DECIDTests.test_DECID_Basic
DECRQMTests.test_DECRQM_DEC_DECCOLM
DECRQMTests.test_DECRQM_DEC_DECLRMM
DECRQMTests.test_DECRQM_DEC_DECSCLM
DECRQMTests.test_DECRQM_DEC_DECSCNM
DECRQSSTests.test_DECRQSS_DECSCUSR
DECRQSSTests.test_DECRQSS_DECSLRM
DECRQSSTests.test_DECRQSS_DECSTBM
DECRQSSTests.test_DECRQSS_SGR
DECSCLTests.test_DECSCL_Level2DoesntSupportDECRQM
DECSETTests.test_DECSET_DECAWM_NoLineWrapOnTabWithLeftRightMargin
HPRTests.test_HPR_DefaultParams
HPRTests.test_HPR_DoesNotChangeRow
ResetColorTests.test_ResetColor_All
ResetColorTests.test_ResetColor_Standard
SCORCTests.test_SaveRestoreCursor_WorksInLRM
VPRTests.test_VPR_DefaultParams
VPRTests.test_VPR_DoesNotChangeColumn
VPRTests.test_VPR_StopsAtBottomEdge```

## Tests only ruuah-vt passes
```
CPLTests.test_CPL_StopsAtTopLine
CUDTests.test_CUD_ExplicitParam
CUDTests.test_CUD_StopsAtBottomLine
DECRQMTests.test_DECRQM
DECSCLTests.test_DSCSCL_Level3_SupportsDECRQMDoesntSupportDECSLRM
DECSETTests.test_DECSET_DECAWM_CursorAtRightMargin
DECSETTests.test_DECSET_DECAWM_TabDoesNotWrapAround
DECSETTests.test_DECSET_DECLRMM_ModeResetByDECSTR
DECSTRTests.test_DECSTR_DECAWM
DECSTRTests.test_DECSTR_DECLRMM
DECSTRTests.test_DECSTR_DECSC
DECSTRTests.test_DECSTR_STBM
HPATests.test_HPA_StopsAtRightEdge
HVPTests.test_HVP_OutOfBoundsParams
TBCTests.test_TBC_Default
TBCTests.test_TBC_NoOp```
