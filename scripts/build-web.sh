#!/usr/bin/env bash
# Builds the S6 panel document: web/dist/index.html, one self-contained file.
#
# Separate from build-swift.sh because it is the one part of the toolchain that needs
# something other than cargo and swift. The terminal builds and runs without it -- panels
# are off unless config.toml says `panels = true` -- so a machine with no bun still gets
# a complete app, and the panel smoke is SKIPPED loudly rather than passing vacuously.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v bun >/dev/null 2>&1; then
  echo "[SKIPPED: web panel build - bun is not installed]" >&2
  exit 2
fi

cd "$ROOT/web"
# --frozen-lockfile: the built document is an artifact that ships inside the app, so it
# is built from the resolved tree the lockfile pins and never from a silent float.
bun install --frozen-lockfile
bun test
bun run build

test -f dist/index.html || { echo "web build produced no dist/index.html" >&2; exit 1; }
echo "built web/dist/index.html ($(wc -c < dist/index.html | tr -d ' ') bytes, self-contained)"
