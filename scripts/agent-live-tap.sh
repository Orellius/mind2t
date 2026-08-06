#!/usr/bin/env bash
# The B4.2 live tap: launch a REAL agent CLI into a real pane and read its interface back off the
# typed grid. Prints the grid it saw.
#
# Not part of `cargo test --workspace`, and that is deliberate: the test it runs is `#[ignore]`d
# because an automated suite must not start authenticated agent processes on somebody's machine
# every time it runs. This script is how it gets run on purpose, so the command lives in one
# place instead of in a paragraph somebody retypes.
#
# It starts one agent, waits for it to prove itself, and shuts it down. No prompt is ever sent, so
# nothing is spent.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

# The bare separator matters: everything after it goes to the TEST harness, not to cargo. Without
# it cargo reads the ignore flag as its own and refuses.
exec cargo test -p mind2t --lib launch::tests::a_real_agent -- --ignored --nocapture "$@"
