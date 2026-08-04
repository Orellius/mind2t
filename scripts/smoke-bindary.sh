#!/usr/bin/env bash
# The Bindary host gate: builds the chrome, then runs the REAL Tauri host with its window ordered
# out and asserts eight invariants about what AppKit, WebKit and the IPC actually did.
#
# Nothing appears on screen and no keystroke is captured, which is what makes it runnable while
# the operator is working. Exit status is the verdict; the run prints one PASS or FAIL line per
# invariant, each naming the defect it exists to catch.
#
# Seen failing: remove crates/bindary/capabilities/chrome.json and it reports six FAIL lines and
# exits 1. A gate never seen red is not evidence.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

# The host embeds chrome/dist at COMPILE time, so a stale bundle silently ships an old chrome.
bun run --cwd chrome build >/dev/null

exec cargo run -q -p bindary --bin bindary -- --smoke
