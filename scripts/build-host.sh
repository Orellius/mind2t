#!/usr/bin/env bash
# Builds the embedder artifact: libmind2t-vt-host.a.
#
# One archive, both surfaces: the host crate depends on mind2t-vt-abi as an rlib, which
# carries the 13 `ghostty_*` exports in alongside the 5 `mind2t_host_*` ones. Two Rust
# staticlibs cannot share a link (each would bring its own copy of the Rust runtime), so
# the Swift host links this single archive. The pure drop-in `libmind2t-vt.a` remains
# `scripts/build-lib.sh`'s separate, slim artifact.
#
# The rename exists for the same reason as in build-lib.sh: cargo cannot emit a hyphenated
# archive name, and the link flag should read like the ghostty one does.

set -euo pipefail

PROFILE="${1:-release}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

case "$PROFILE" in
  release) cargo build --release -p mind2t-vt-host ;;
  debug)   cargo build -p mind2t-vt-host ;;
  *) echo "usage: $0 [release|debug]" >&2; exit 2 ;;
esac

OUT="target/$PROFILE"
SRC="$OUT/libmind2t_vt_host.a"
DST="$OUT/libmind2t-vt-host.a"

if [ ! -f "$SRC" ]; then
  echo "cargo did not produce $SRC" >&2
  exit 1
fi
cp "$SRC" "$DST"

# Both surfaces, by name. A host archive that lost a ghostty_* symbol has silently stopped
# carrying the VT readout; one that lost a mind2t_host_* symbol fails at the Swift link,
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
  mind2t_host_spawn
  mind2t_host_poll
  mind2t_host_send
  mind2t_host_paste
  mind2t_host_mouse_geometry
  mind2t_host_mouse
  mind2t_host_wheel
  mind2t_host_key
  mind2t_host_scroll
  mind2t_host_resize
  mind2t_host_cell_metrics
  mind2t_host_set_font_size
  mind2t_host_row_text
  mind2t_host_link_at
  mind2t_host_next_event
  mind2t_host_free
  mind2t_history_load
  mind2t_history_free
  mind2t_history_append
  mind2t_history_suggest
  mind2t_cwd_path
  mind2t_workflows_load
  mind2t_workflows_free
  mind2t_workflows_count
  mind2t_workflows_errors
  mind2t_workflow_field
  mind2t_workflow_arg_count
  mind2t_workflow_arg
  mind2t_workflow_render
  mind2t_config_load
  mind2t_config_font_size
  mind2t_config_auto_direction
  mind2t_config_shell
  mind2t_config_reports
  mind2t_config_panels
  mind2t_config_font_family
  mind2t_config_error
  mind2t_config_free
  mind2t_host_attach_layer
  mind2t_host_detach_layer
  mind2t_host_resize_layer
  mind2t_host_present
  mind2t_agent_count
  mind2t_agent_info
  mind2t_agent_resolve
  mind2t_agent_screen
  mind2t_host_spawn_agent
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
