#!/usr/bin/env bash
# Builds the shipped artifact: libruuah-vt.a, with the hyphen.
#
# Cargo cannot emit that name. A package called `libruuah-vt` produces liblibruuah_vt.a, and
# `[lib] name = "ruuah-vt"` is a hard cargo error -- "library target names cannot contain
# hyphens". So the crate is named ruuah_vt, cargo emits libruuah_vt.a, and this renames it, so
# a consumer's link flag reads `-lruuah-vt` and mirrors `-lghostty-vt` exactly.
#
# Also verifies the archive actually exports the ABI. A drop-in that silently stopped
# exporting a symbol would fail at the consumer's link step, which is the worst place to find
# out, and no cargo test can see it because the symbols only reach the archive at build time.

set -euo pipefail

PROFILE="${1:-release}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

case "$PROFILE" in
  release) cargo build --release -p ruuah-vt-abi ;;
  debug)   cargo build -p ruuah-vt-abi ;;
  *) echo "usage: $0 [release|debug]" >&2; exit 2 ;;
esac

OUT="target/$PROFILE"
SRC="$OUT/libruuah_vt.a"
DST="$OUT/libruuah-vt.a"

if [ ! -f "$SRC" ]; then
  echo "cargo did not produce $SRC" >&2
  exit 1
fi
cp "$SRC" "$DST"

# Every symbol the differential harness verifies. If one is missing the archive is not a
# drop-in, whatever the tests say.
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
)

# The symbol table is captured once rather than piped per symbol. `nm` exits non-zero on
# archive members that carry no symbols, and under `set -o pipefail` that sinks the whole
# pipeline even when grep matched -- which reported all 13 exports missing while every one of
# them was present.
SYMBOLS="$(mktemp)"
trap 'rm -f "$SYMBOLS"' EXIT
nm -gU "$DST" 2>/dev/null > "$SYMBOLS" || true

missing=0
for symbol in "${EXPECTED[@]}"; do
  if ! grep -qE "^[0-9a-f]+ T _?$symbol\$" "$SYMBOLS"; then
    echo "MISSING export: $symbol" >&2
    missing=$((missing + 1))
  fi
done

if [ "$missing" -ne 0 ]; then
  echo "$missing of ${#EXPECTED[@]} exports missing; this archive is not a drop-in" >&2
  exit 1
fi

echo "built $DST"
echo "  exports  ${#EXPECTED[@]}/${#EXPECTED[@]} verified"
echo "  link as  -L$ROOT/$OUT -lruuah-vt"
echo "  headers  ghostty/vt/*.h from libghostty-vt, unchanged"
