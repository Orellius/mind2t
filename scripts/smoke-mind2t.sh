#!/usr/bin/env bash
# The Mind2t host gate: builds the chrome, then runs the REAL Tauri host with its window ordered
# out and asserts twenty-six invariants about what AppKit, WebKit, the IPC and the children did.
#
# Nothing appears on screen and no keystroke is captured, which is what makes it runnable while
# the operator is working. Exit status is the verdict; the run prints one PASS or FAIL line per
# invariant, each naming the defect it exists to catch.
#
# Seen failing: remove crates/mind2t/capabilities/chrome.json and it reports six FAIL lines and
# exits 1. A gate never seen red is not evidence.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

# The host embeds chrome/dist at COMPILE time, so a stale bundle silently ships an old chrome.
bun run --cwd chrome build >/dev/null

# Built first, then launched through a POISONED environment. Both halves matter.
#
# Without the poison the environment invariants pass while measuring nothing: the gate runs under
# the operator's own terminal, so a host that simply passes its environment through hands the
# child a perfectly good TERM and a PATH already full of tools, and the checks end up reading the
# launching shell rather than the host. Measured - three of the four passed against a host that
# declared nothing at all.
#
# The case being stood in for is a Finder launch, which inherits NOTHING. `PATH` is cut to the
# system default (only a LOGIN shell's /etc/zprofile and ~/.zprofile put homebrew back), `TERM`
# is set to a type with no useful terminfo, and the Claude session markers are set so a host that
# forgets to scrub them reports them straight back.
#
# The build happens before the poison because cargo itself needs the real PATH.
cargo build -q -p mind2t --bin mind2t

exec env \
    PATH=/usr/bin:/bin:/usr/sbin:/sbin \
    TERM=dumb \
    CLAUDECODE=smoke-poison \
    CLAUDE_CODE_CHILD_SESSION=smoke-poison \
    ./target/debug/mind2t --smoke
