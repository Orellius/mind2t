# Changelog

Per-release notes live on the
[Releases page](https://github.com/Orellius/mind2t/releases), which is generated from the
annotated tag and the merges it contains. That is the single source of truth for what shipped
and when; this file records the scheme and the shape of the history rather than restating it.

## Versioning

[SemVer 2.0.0](https://semver.org), `vX.Y.Z`, annotated tags only.

The project is pre-1.0, so the minor version is where breaking changes land. A release is an
act of shipping, not a merge: individual merges to `main` are never tagged, and a batch of
merged work bumps the minor version once. Every release carries the gate numbers it shipped
with, so a tag answers "what worked, measured how" without checking anything out.

`main` holds verified work only. Every commit on it has `cargo test --workspace` green and the
differential corpus meeting every expectation.

## Shape of the history

The engine was built in numbered slices, and the tags follow them.

| range | what it covered |
|---|---|
| `v0.0.0` | the differential harness, before any terminal logic existed |
| `v0.1.0` to `v0.5.x` | parser and grid, scrolling and screens, paged scrollback, reflow, the frame channel and pty, the CPU renderer, bidi and shaping, OSC 133 |
| `v0.6.0` to `v0.8.0` | the C ABI, the wgpu backend, the embedder surface and the Swift reference host |
| `v0.9.0` | backfilled: a 57-merge feature train that had shipped with no tags at all, which is why the release discipline above exists |
| `v0.10.0` to `v0.16.0` | protocol depth: OSC colour, rectangle ops, DECRQSS, selective erase, left and right margins, XTERMWINOPS, and the esctest2 conformance climb |
| `v0.17.0` to `v0.19.x` | web-rendered panels, git-worktree workspaces, the docked workspace sidebar |

Work after `v0.19.3` is unreleased and lives on `main`: the product host gaining panes,
selection, zoom and the agent launcher, and the rename of the repository and the product from
`mind2t-vt` to Mind2t. The engine crates keep the `mind2t-vt-` prefix.

## Notes on reading the history

- A corpus case pinned `expect = "diff"` is a to-do rather than a failure, and a release that
  promotes one to `match` says so in its notes. That promotion is the evidence the behaviour
  landed.
- `oracle.lock` moving in a release means the reference implementation was rebuilt at a new
  commit. Verdict changes in that release may originate upstream rather than here, and the
  notes call it out when that happened.
