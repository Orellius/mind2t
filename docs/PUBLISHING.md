# Publishing to crates.io

**Nothing here is published, and `publish = false` is set workspace-wide.** Everything mechanical
is prepared, so the day the two open questions below are settled, publishing is one line plus nine
`cargo publish` calls in the order given.

## The two questions that are NOT mechanical

**1. The licence, and it contradicts the project's own pitch.** This is `AGPL-3.0-only`. On an
application that is a deliberate choice. On a **library** it means anyone who links the engine must
release their entire application under the same terms, including when it is only reachable over a
network. The engine's whole claim is a drop-in C ABI - "you can link either" - so as things stand
that claim is legally true and practically unusable to any commercial embedder.

The usual resolution is a split: permissive (MIT / Apache-2.0) on the engine crates, AGPL kept on
the product in `crates/mind2t`. The engine is the embeddable part; the app is the thing worth
protecting. Dual-licensing AGPL + commercial is the other option. **Decide before the first
publish, not after**: a licence change once other people depend on it needs every contributor's
consent, and a crates.io name can never be released once taken.

**2. The ABI is still moving.** Every exported symbol was renamed on 2026-08-08. Publishing a 0.x
that renames its whole surface is churn for anyone downstream, and there is no downstream yet - the
planned consumer is this repository's own Swift host.

## Publish order

Dependency order, derived from the graph rather than written by hand. Each crate must be on
crates.io before anything that depends on it, or `cargo publish` refuses with *no matching package
named X found*.

1. `mind2t-vt-abi-types` - no workspace deps
2. `mind2t-vt-snapshot` - no workspace deps
3. `mind2t-vt-vte` - no workspace deps
4. `mind2t-vt-core` - needs `mind2t-vt-snapshot`, `mind2t-vt-vte`
5. `mind2t-vt-abi` - needs `mind2t-vt-abi-types`, `mind2t-vt-core`, `mind2t-vt-snapshot`
6. `mind2t-vt-frame` - needs `mind2t-vt-core`, `mind2t-vt-snapshot`
7. `mind2t-vt-pty` - needs `mind2t-vt-core`, `mind2t-vt-frame`, `mind2t-vt-snapshot`
8. `mind2t-vt-render` - needs `mind2t-vt-core`, `mind2t-vt-frame`, `mind2t-vt-pty`, `mind2t-vt-snapshot`
9. `mind2t-vt-host` - needs `mind2t-vt-abi`, `mind2t-vt-core`, `mind2t-vt-frame`, `mind2t-vt-pty`, `mind2t-vt-render`, `mind2t-vt-snapshot`

## Never published, and why

| crate | reason |
|---|---|
| `mind2t-vt-ghostty` | **Cannot build off this machine.** Its `build.rs` links `libghostty-vt`, built from a local Ghostty checkout via `MIND2T_VT_ORACLE_SRC`. It would fail on docs.rs and on any consumer. It is a test oracle, not a library. |
| `mind2t-vt-difftest` | A binary, and it depends on the oracle above. |
| `mind2t` | The product. Distributed as a signed app, not as a crate. |

Each carries a hard `publish = false` in its own manifest rather than relying on the workspace
switch, so flipping the switch cannot publish them by accident.

## What was already done (2026-08-08)

- **Every intra-workspace path dependency carries a `version`.** cargo refuses to publish a crate
  whose dependency has only a path. The path stays, so local builds are unaffected: cargo uses the
  path here and the version on crates.io.
- **The vendored fork was renamed.** Its package was `vte`, which belongs to the upstream project
  it forks, so it could never have been published. It is `mind2t-vt-vte` now, and its **lib target
  is still named `vte`** - so every `use vte::...` in the workspace is untouched and the fork stays
  a re-vendorable diff rather than a rewrite.
- **Every crate has an explicit `publish` key.** Four had none, which meant they defaulted to
  publishable while every other crate inherited the workspace's `false` - the opposite of intended,
  and a `cargo publish` run in one of those directories would have gone through.
- **`description`, `license` and `repository` on all nine**, which crates.io requires.

## How far this is verified

`cargo package --no-verify` succeeds for the three leaves (`mind2t-vt-abi-types`,
`mind2t-vt-snapshot`, `mind2t-vt-vte`). The other six fail with *no matching package named X found
- location searched: crates.io index*, which is not a manifest defect: it is the registry correctly
reporting that their dependencies are not published yet, and it resolves itself as the order above
is walked.

**So the manifests are proven correct only as far as they can be without actually publishing.**
The first real publish is the first full test, which is one more reason to settle the licence first
rather than discover it at step 4 of 9.
