#!/usr/bin/env bash
# Builds the embedder artifact: libruuah-vt-host.a.
#
# One archive, both surfaces: the host crate depends on ruuah-vt-abi as an rlib, which
# carries the 13 `ghostty_*` exports in alongside the 5 `ruuah_host_*` ones. Two Rust
# staticlibs cannot share a link (each would bring its own copy of the Rust runtime), so
# the Swift host links this single archive. The pure drop-in `libruuah-vt.a` remains
# `scripts/build-lib.sh`'s separate, slim artifact.
#
# The rename exists for the same reason as in build-lib.sh: cargo cannot emit a hyphenated
# archive name, and the link flag should read like the ghostty one does.

set -euo pipefail

PROFILE="${1:-release}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

case "$PROFILE" in
  release) cargo build --release -p ruuah-vt-host ;;
  debug)   cargo build -p ruuah-vt-host ;;
  *) echo "usage: $0 [release|debug]" >&2; exit 2 ;;
esac

OUT="target/$PROFILE"
SRC="$OUT/libruuah_vt_host.a"
DST="$OUT/libruuah-vt-host.a"

if [ ! -f "$SRC" ]; then
  echo "cargo did not produce $SRC" >&2
  exit 1
fi
cp "$SRC" "$DST"

# Both surfaces, by name. A host archive that lost a ghostty_* symbol has silently stopped
# carrying the VT readout; one that lost a ruuah_host_* symbol fails at the Swift link,
# which is the worst place to find out.
EXPECTED=(
  ghostty_terminal_new
  ghostty_terminal_free
  ghostty_terminal_vt_write
  ghostty_terminal_resize
  ghostty_terminal_get
  ghostty_terminal_mode_get
  ghostty_terminal_grid_ref
  ghostty_grid_ref_cell
  ghostty_grid_ref_row
  ghostty_grid_ref_graphemes
  ghostty_grid_ref_style
  ghostty_cell_get
  ghostty_row_get
  ghostty_style_default
  ruuah_host_spawn
  ruuah_host_poll
  ruuah_host_send
  ruuah_host_paste
  ruuah_host_resize
  ruuah_host_free
)

# Captured once, not piped per symbol: `nm` exits non-zero on members with no symbols, and
# under pipefail that once reported every export missing while all were present.
SYMBOLS="$(mktemp)"
trap 'rm -f "$SYMBOLS"' EXIT
nm -gU "$DST" 2>/dev/null > "$SYMBOLS" || true

MISSING=0
for symbol in "${EXPECTED[@]}"; do
  if ! grep -q " _${symbol}$" "$SYMBOLS"; then
    echo "MISSING export: $symbol" >&2
    MISSING=1
  fi
done
if [ "$MISSING" -ne 0 ]; then
  exit 1
fi

echo "built $DST with all ${#EXPECTED[@]} exports"
