#!/usr/bin/env bash
# Builds the whole slice 8 chain and proves it: the Rust archive (with its 18-export
# check), then the Swift host against it, then the headless smoke -- a child's output
# becoming ink, reached exclusively through the C surface, from Swift.
#
# `swift build` runs from swift/ because Package.swift's -L flag resolves against the
# invoking directory.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

"$ROOT/scripts/build-host.sh" release

cd "$ROOT/swift"
# SwiftPM does not track the external Rust archive as a link input: a Rust-only change
# rebuilds libmind2t-vt-host.a and leaves the previously linked binary in place (caught
# 2026-07-29 -- the braille font fix built, the shipped app didn't have it). Deleting
# the product forces the relink; everything else stays incremental.
rm -f .build/release/mind2t-host
swift build -c release

if [ "${1:-}" = "--no-smoke" ]; then
  exit 0
fi
.build/release/mind2t-host --smoke

# Chrome geometry. Pure arithmetic, no window, so it costs milliseconds. The failure it
# guards is SILENT: a pane at full width under a docked sidebar looks correct until the
# right-hand columns turn out to be covered. The narrow sizes are the discriminating
# ones - a sidebar computed as a constant rather than a remainder tiles perfectly at
# 1120 and overlaps at 300, so a gate that only tests a comfortable window passes on the
# wrong implementation. Carries its own control: undocked must give the pane the whole
# width, or "docked" and "undocked" would be the same assertion wearing two names.
.build/release/mind2t-host --smoke-chrome

# SSH hosts. Parses a fixture ssh_config it writes itself - never the operator's, because
# ~/.ssh is a credential directory and a gate that read it would print its contents into
# CI output the first time it failed. The discriminating cases are the ones a naive line
# splitter gets wrong: a wildcard pattern is not a host, a duplicate block must LOSE to
# first-value-wins, a Match block's keys must not stick to the host above it. All three
# mutants seen red 2026-08-14, each with its own message. Carries its own control: a
# missing config must read as an empty list, not as a failure.
.build/release/mind2t-host --smoke-ssh

# The connection form's WRITER, and it guards a mutation of a file this app did not author
# rather than a feature, so its assertions are about damage. The load-bearing one is shell
# quoting: the host runs a pane's command through `/bin/sh -c`, so an alias carrying a
# semicolon runs as two commands, and no validator upstream has any reason to reject a
# semicolon. It is proven against `/bin/sh` itself - five payloads re-split by a real shell
# must come back as the same five words - because inspecting the quoting proves nothing
# about what the shell does with it. Next after that: a newline in ANY field would append a
# second block to the operator's ssh config, a refused write must leave the file
# byte-identical, and a fresh config must be 0600. Mutants seen red 2026-08-14: a naive
# join, a disabled newline check, an allowed duplicate alias, a truncate instead of an
# append, a 0644 create, and the form's Port and User fields swapped.
.build/release/mind2t-host --smoke-ssh-write

# The connection dialog's GEOMETRY, and it exists because v0.28.0 shipped this form with
# two controls collapsed to 0pt and a dialog 590pt tall hanging off an 816x510 window,
# with all five gates green. The gates stop at Session by design, so nothing above it was
# ever measured. What makes this one able to see the defect is WHERE it is rooted: the
# form measured on its own reports every field at a correct 320pt in the broken build,
# because the collapse happens when the container re-frames it. It walks the container's
# tree instead, and asserts the height against a ceiling derived from the smallest window
# this host opens. Faults are reported together, not one per build.
#
# `--shot-ssh-dialog <path>` beside it renders the same dialog to a PNG. Not a gate: an
# assertion cannot answer "does it look right", and that question is what found this.
.build/release/mind2t-host --smoke-ssh-layout

# S5 workspaces. The load-bearing assertion is the REFUSAL: remove() never passes
# --force, and a dirty worktree surviving is what stands between this feature and
# deleting an agent's unpushed work. Mutant seen red (adding --force fails it).
.build/release/mind2t-host --smoke-worktree

# Agents. The load-bearing assertion is again the refusal: an approval-bypassing flag must
# come back REFUSED AND NAMED, through the Swift argv borrow, before anything is spawned. A
# fake agent on PATH, never a real one -- a build gate must not start authenticated agent
# processes on the machine it runs on.
#
# Two mutants seen red: collapsing the refusal into a generic failure (the operator stops
# being told which word to remove), and letting the argv borrow dangle - which does not crash,
# it lets `--yolo` through the guard entirely.
.build/release/mind2t-host --smoke-agent

# S6 panels. The web build is optional (no bun, no panels), so its absence SKIPS the
# probe out loud instead of leaving a silently unproven seam. Both directions run: the
# probe must round-trip a nonce, and the control -- the same document with the receiver
# removed -- must load, mount, and fail to answer. The control is the half that matters;
# on its first run it passed for the wrong reason and hid a real navigation-policy bug.
if "$ROOT/scripts/build-web.sh"; then
  .build/release/mind2t-host --smoke-panel --web-dir "$ROOT/web/dist"
  .build/release/mind2t-host --smoke-panel-control --web-dir "$ROOT/web/dist"
else
  status=$?
  [ "$status" -eq 2 ] || exit "$status"
  echo "[SKIPPED: panel bridge smoke - no web build]" >&2
fi
