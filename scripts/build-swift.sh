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
# rebuilds libruuah-vt-host.a and leaves the previously linked binary in place (caught
# 2026-07-29 -- the braille font fix built, the shipped app didn't have it). Deleting
# the product forces the relink; everything else stays incremental.
rm -f .build/release/ruuah-host
swift build -c release

if [ "${1:-}" = "--no-smoke" ]; then
  exit 0
fi
.build/release/ruuah-host --smoke

# S6 panels. The web build is optional (no bun, no panels), so its absence SKIPS the
# probe out loud instead of leaving a silently unproven seam. Both directions run: the
# probe must round-trip a nonce, and the control -- the same document with the receiver
# removed -- must load, mount, and fail to answer. The control is the half that matters;
# on its first run it passed for the wrong reason and hid a real navigation-policy bug.
if "$ROOT/scripts/build-web.sh"; then
  .build/release/ruuah-host --smoke-panel --web-dir "$ROOT/web/dist"
  .build/release/ruuah-host --smoke-panel-control --web-dir "$ROOT/web/dist"
else
  status=$?
  [ "$status" -eq 2 ] || exit "$status"
  echo "[SKIPPED: panel bridge smoke - no web build]" >&2
fi
