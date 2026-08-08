#!/usr/bin/env bash
# Builds the S6 panel document: web/dist/index.html, one self-contained file.
#
# ORPHANED 2026-08-08 (T6) AND LEFT STANDING DELIBERATELY. The only consumer of web/dist was
# the Swift host's WKWebView panel, and that host was deleted. Nothing in this repository reads
# the document any more - measured, not assumed: a grep for `web/dist` and `--web-dir` across
# every source, script and config finds this file and nothing else.
#
# It is not deleted here because T6's ask was the Swift host and its references, and web/ is a
# separate orphan rather than a reference to it. Deleting somebody's work as a side effect of a
# different task is how a diff stops being reviewable. It is a one-line decision when Orel wants
# it: either the Tauri chrome grows a diff panel and this becomes its source, or `git rm -r web`.
#
# Was: the one part of the toolchain needing something other than cargo, kept separate so a
# machine with no bun still built a complete app.
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
