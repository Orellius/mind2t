#!/usr/bin/env bash
# Builds libghostty-vt out of the RUUAH checkout and installs it into vtr/vendor.
#
# RUUAH is read-only. Its whole economics are a near-zero rebase tax against upstream
# Ghostty, so both zig's install prefix and its cache are redirected here; the checkout
# is left byte-identical and `git status` stays clean. Verified after every run below.
set -euo pipefail

readonly ZIG=/opt/homebrew/opt/zig/bin/zig
readonly REQUIRED_ZIG=0.16.0

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
ruuah=${VTR_RUUAH_DIR:-$(cd "$root/../ruuah" 2>/dev/null && pwd || true)}
prefix="$root/vendor/libghostty-vt"
cache="$root/vendor/.zig-cache"

if [[ -z "$ruuah" || ! -f "$ruuah/build.zig" ]]; then
  echo "error: no Ghostty checkout found. Set VTR_RUUAH_DIR to one." >&2
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

echo
echo "ok: $prefix/lib/libghostty-vt.a"
echo "ok: $ruuah is unchanged"
