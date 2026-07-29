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
swift build -c release

if [ "${1:-}" = "--no-smoke" ]; then
  exit 0
fi
.build/release/ruuah-host --smoke
