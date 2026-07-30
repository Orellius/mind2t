#!/usr/bin/env bash
# Fetches esctest2 into vendor/esctest2/.
#
# This is slice 9's oracle: a vendor-neutral conformance suite (Dickey's maintained fork
# of iTerm2's esctest) that drives the terminal through its own pty and reads state back
# with DECRQCRA checksums and CPR -- an independent second opinion that does not care
# about the C ABI at all, which is exactly what the differential oracle cannot give for
# reply semantics. GPL-2.0: run as a separate process at test time, never linked, never
# shipped; `vendor/` is gitignored.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dest="$here/vendor/esctest2"
repo="https://github.com/ThomasDickey/esctest2"

# Pinned: a conformance failure after a suite update must be distinguishable from a
# regression we caused. Bump deliberately, rerun, re-triage the expectation pins.
pin="master"
if [[ -f "$here/esctest.lock" ]]; then
    pin="$(sed -n 's/^commit = "\(.*\)"$/\1/p' "$here/esctest.lock")"
    pin="${pin:-master}"
fi

if [[ -d "$dest/.git" ]]; then
    git -C "$dest" fetch --quiet origin
else
    rm -rf "$dest"
    GIT_TERMINAL_PROMPT=0 git clone --quiet "$repo" "$dest"
fi
git -C "$dest" checkout --quiet "$pin"

commit="$(git -C "$dest" rev-parse HEAD)"
cat > "$here/esctest.lock" <<EOF
# Which esctest2 the reply semantics were verified against.
# Written by scripts/fetch-esctest.sh; commit this file when it changes.
commit = "$commit"
EOF

echo "vendored esctest2 at $commit"
