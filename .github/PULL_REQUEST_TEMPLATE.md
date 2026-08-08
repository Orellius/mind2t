## What this changes

<!-- One paragraph. Why, not what the diff already shows. -->

## Evidence

<!--
Which gates were run, and their numbers. Paste the counts, not "all green".
-->

- [ ] `cargo test --workspace`
- [ ] `cargo run -p mind2t-vt-difftest`
- [ ] `./scripts/smoke-mind2t.sh`

**The mutant.** Which deliberate break was used to prove the new test can fail, and what it
printed when it went red. A test that has never been red is not evidence.

<!-- e.g. "reverted the offset in Frame::viewport_rows -> selection_rows.rs fails with
     'row 2 disagrees with the core'" -->

**Corpus.** If a case was promoted from `expect = "diff"` to `expect = "match"`, say which.
If a new deliberate divergence was added, say why and that it is pinned.

## GUI seams

<!--
Anything whose contract ends at the GUI (click, chord, drag, resize) is not proven by a
headless test. Either name the live tap, or say plainly which seam is untested.
-->

- [ ] Live-tapped
- [ ] No GUI seam touched
- [ ] Untested seam, named above

## Checklist

- [ ] I read [CONTRIBUTING.md](../CONTRIBUTING.md)
- [ ] Only the files this change touches were reformatted
- [ ] My contribution is licensed AGPL-3.0-only, and no code was copied from another terminal
