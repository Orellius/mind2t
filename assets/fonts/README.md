# Bundled fallback fonts

Two faces ship inside the app because the terminal is **wrong without them, silently.**

The stack in `crates/render/src/font.rs` loads faces BY FILE PATH and gives each fallback a
scale that makes its advance fit one cell. When Miriam Mono CLM is absent, Hebrew falls through
to Arial Hebrew, which is proportional: measured at a 19px cell, aleph advances 9.0 against
Latin's 10.0. Nothing errors. Hebrew simply drifts off the grid, and the only way to notice is
to look at Hebrew on screen and know what correct looks like. Arabic without Kawkab Mono has the
same shape one script over.

These were `$HOME/Library/Fonts` lookups, described in the code as "optional and user-installed".
That made a correct grid depend on a manual step nobody performs on a fresh machine - and a
machine format on 2026-08-11 proved it by silently degrading Hebrew on the first launch.

| file | face | licence |
|---|---|---|
| `MiriamMonoCLM-Book.ttf` | Hebrew, monospaced | GPL-2.0 with a font embedding exception |
| `KawkabMono-Regular.ttf` | Arabic and Persian, monospaced | SIL OFL 1.1 |

## Provenance, and why both may ship here

- **Miriam Mono CLM** comes from the Culmus project, `culmus-0.140`, unmodified. Its licence is
  GPL-2.0, and its embedding exception is written for DOCUMENTS, not for software - so the
  exception is NOT what permits this. What permits it is that a font file loaded at runtime by
  path is **aggregation**, not a derived work (GPL-2 section 2, final paragraph), which is the
  same basis on which every Linux distribution ships fonts beside GPL-incompatible software.
  The condition that comes with it: the file stays **unmodified** and `LICENSE-Culmus-GPL2.txt`
  travels with it. Do not subset, re-hint or re-generate this file. If a future slice needs a
  modified Miriam, that modified font is GPL-2 and must be distributed as such.
- **Kawkab Mono** comes from the `v0.501` release of `github.com/aiaf/kawkab-mono`, unmodified.
  OFL 1.1 permits bundling outright; it requires the licence to ride along, which
  `LICENSE-KawkabMono-OFL.txt` does, and it reserves the name - so a modified copy must be
  renamed rather than shipped as Kawkab Mono.

Both licences are in this directory rather than referenced, because a licence that lives only in
a URL is one dead link away from a compliance problem.

## Where they end up

`tauri.conf.json` copies this directory into `Mind2t.app/Contents/Resources/fonts/`. The stack
resolves that path from the running executable, so the app carries its own fallbacks and no
install step is required. A user-installed copy in `~/Library/Fonts` is still honoured when the
bundled one cannot be found, which is what makes `cargo run` work in a checkout.
