#!/usr/bin/env bash
# Builds libghostty-vt out of the RUUAH checkout and installs it into ruuah-vt/vendor.
#
# RUUAH is read-only. Its whole economics are a near-zero rebase tax against upstream
# Ghostty, so both zig's install prefix and its cache are redirected here; the checkout
# is left byte-identical and `git status` stays clean. Verified after every run below.
set -euo pipefail

readonly ZIG=/opt/homebrew/opt/zig/bin/zig
readonly REQUIRED_ZIG=0.16.0

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
ruuah=${RUUAH_VT_ORACLE_SRC:-$(cd "$root/../ruuah" 2>/dev/null && pwd || true)}
prefix="$root/vendor/libghostty-vt"
cache="$root/vendor/.zig-cache"

if [[ -z "$ruuah" || ! -f "$ruuah/build.zig" ]]; then
  echo "error: no Ghostty checkout found. Set RUUAH_VT_ORACLE_SRC to one." >&2
  exit 1
fi

# The machine default zig is 0.15.2 and refuses to build this; 0.16.0 is required and
# is called by absolute path so other projects keep their own default on PATH.
if [[ ! -x "$ZIG" ]]; then
  echo "error: zig $REQUIRED_ZIG not found at $ZIG" >&2
  exit 1
fi
have=$("$ZIG" version)
if [[ "$have" != "$REQUIRED_ZIG" ]]; then
  echo "error: $ZIG is $have, this build needs exactly $REQUIRED_ZIG" >&2
  exit 1
fi

before=$(cd "$ruuah" && git status --porcelain 2>/dev/null | wc -l | tr -d ' ')

echo "building libghostty-vt"
echo "  source $ruuah (read-only)"
echo "  prefix $prefix"
( cd "$ruuah" && "$ZIG" build -Demit-lib-vt --prefix "$prefix" --cache-dir "$cache" )

if [[ ! -f "$prefix/lib/libghostty-vt.a" ]]; then
  echo "error: the build reported success but produced no libghostty-vt.a" >&2
  exit 1
fi

after=$(cd "$ruuah" && git status --porcelain 2>/dev/null | wc -l | tr -d ' ')
if [[ "$before" != "$after" ]]; then
  echo "error: the build dirtied $ruuah ($before -> $after changed paths)." >&2
  echo "       That breaks the read-only rule; investigate before continuing." >&2
  exit 1
fi

# Record which Ghostty build the oracle came from.
#
# ruuah-vt has no git upstream -- it is original code, and adding a remote with no shared
# history would be theatre. But it does have a real upstream DEPENDENCY: the oracle is a
# moving target, and when Ghostty changes behaviour the corpus verdicts move with it. Without
# this pin, a case flipping overnight is indistinguishable from a regression we caused. This
# is the upstream tracking that actually matters here.
lock="$root/oracle.lock"
commit=$(cd "$ruuah" && git rev-parse HEAD)
describe=$(cd "$ruuah" && git describe --tags --always 2>/dev/null || echo unknown)

if [[ -f "$lock" ]]; then
  previous=$(grep '^commit = ' "$lock" | cut -d'"' -f2)
  if [[ -n "$previous" && "$previous" != "$commit" ]]; then
    echo
    echo "NOTE: the oracle moved."
    echo "        was $previous"
    echo "        now $commit"
    echo "      Corpus verdicts may change for reasons that are not yours. Re-run"
    echo "      ./target/debug/difftest and attribute any flip to this before debugging it."
  fi
fi

# The lock file is committed, so the checkout path is recorded home-relative: an absolute
# one publishes the machine's account name to everyone who clones the repository.
source_recorded="${ruuah/#$HOME/~}"

cat > "$lock" <<LOCK
# Which libghostty-vt the differential oracle was built from. Written by
# scripts/build-oracle.sh; commit this file when it changes.
#
# ruuah-vt has no git upstream. This is the upstream that matters: the oracle is the
# reference implementation, and when it moves the corpus can move with it.
source = "$source_recorded"
commit = "$commit"
describe = "$describe"
zig = "$REQUIRED_ZIG"
LOCK

echo
echo "ok: $prefix/lib/libghostty-vt.a"
echo "ok: $ruuah is unchanged"
echo "ok: oracle pinned at $describe ($lock)"
