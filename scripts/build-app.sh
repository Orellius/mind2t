#!/bin/bash
# Builds Mind2t.app - the TAURI host - and installs it to ~/Applications.
#
# REWRITTEN 2026-08-08 (T7). This script used to assemble the SWIFT host by hand: SwiftPM emits
# executables rather than bundles, so it wrote an Info.plist, copied a binary and sealed the
# result itself. Tauri's bundler does all of that, so what is left here is the part the bundler
# does NOT do: build the chrome first, hand it the version, and install the result.
#
# Anything launched outside a terminal must not live under ~/Desktop - TCC kills disclaimed
# children there silently (SCAR-006) - which is why the install target is ~/Applications and the
# repo stays the source of truth. The bundle is a build product.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"

# THE CHROME IS BUILT HERE, NOT BY `beforeBuildCommand`.
#
# The config carried one and it was deleted, because its working directory is not the config's
# directory: measured 2026-08-08, `chrome` and `../../chrome` BOTH failed with ENOENT, which puts
# the cwd at `crates/`. A path that depends on an inference about somebody else's cwd is a path
# that breaks the first time they change it. The host embeds chrome/dist at COMPILE time, so a
# stale bundle ships silently and this step is not optional.
bun run --cwd chrome build >/dev/null

# THE VERSION COMES FROM THE TAG, NEVER FROM THE LITERAL IN tauri.conf.json.
#
# The Swift script learned this the expensive way: CFBundleShortVersionString was hardcoded at
# 0.8.0 and stayed there through thirteen releases, so every installed app claimed a version it
# was not, and nothing errors because a plist string is only ever read by a human or an updater.
# Passed as a `--config` OVERLAY rather than by rewriting the file, so a failed build cannot leave
# a mutated config behind.
#
# TAG BEFORE BUILDING. `git describe` on an untagged tree yields the PREVIOUS tag and bakes it in.
VERSION="$(git describe --tags --abbrev=0 2>/dev/null | sed 's/^v//')"
VERSION="${VERSION:-0.0.0}"

echo "building Mind2t.app at version $VERSION"
( cd crates/mind2t && cargo tauri build --config "{\"version\":\"$VERSION\"}" )

BUILD="$ROOT/target/release/bundle/macos/Mind2t.app"
[ -d "$BUILD" ] || { echo "the bundler produced no app at $BUILD" >&2; exit 1; }

# THE BUNDLE MUST CARRY THE HOST, NOT THE PROBE, AND THIS IS CHECKED RATHER THAN ASSUMED.
#
# The crate has two binaries - `mind2t` (the Tauri host) and `probe` (the tao + wry oracle) - and
# on the first bundle Tauri picked `probe`. The app launched, drew a terminal, and was the wrong
# program wearing Mind2t's identity: CFBundleExecutable read `probe` and nothing anywhere said so.
# `mainBinaryName` in tauri.conf.json fixes it; this assertion is what stops it coming back.
EXEC="$(/usr/libexec/PlistBuddy -c "Print :CFBundleExecutable" "$BUILD/Contents/Info.plist")"
[ "$EXEC" = "mind2t" ] || { echo "bundle ships '$EXEC', not the host binary" >&2; exit 1; }

PLIST_VERSION="$(/usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" "$BUILD/Contents/Info.plist")"
[ "$PLIST_VERSION" = "$VERSION" ] || {
  echo "bundle claims version $PLIST_VERSION, expected $VERSION" >&2; exit 1; }

# THE BUNDLED BINARY IS RUN BEFORE IT IS INSTALLED, headless, with no window on anybody's screen.
#
# A bundle that builds is not a bundle that works: the chrome is embedded at compile time and the
# GPU surface is created at launch, so the failures worth catching all happen in the first second
# of running. `MIND2T_HEADLESS=1` is the same switch the smoke uses. The host prints its geometry
# and the chrome's ready message; the probe prints neither, which makes this a discriminating
# check and not a liveness one.
echo "running the bundled binary headless"
OUT="$(timeout 60 env MIND2T_HEADLESS=1 "$BUILD/Contents/MacOS/mind2t" 2>&1 | head -40 || true)"
echo "$OUT" | grep -q "cells at" || { echo "the bundled host never reported its geometry:"; echo "$OUT"; exit 1; }
echo "$OUT" | grep -q '"kind":"ready"' || { echo "the bundled chrome never reported ready:"; echo "$OUT"; exit 1; }

# SIGNING. Tauri ad-hoc signs when no identity is configured, and re-signing here is belt and
# braces for the TCC problem the Swift bundle hit: TCC identifies an app by its signature, and an
# unsealed bundle is SILENTLY denied Desktop/Documents/Downloads with no permission prompt ever
# shown (measured live 2026-07-30, "cannot cd into Desktop" inside the app).
#
# AD-HOC IS ONLY SUFFICIENT WHILE THIS REPO IS PRIVATE. A stranger who downloads an ad-hoc-signed
# app gets "damaged and can't be opened", and macOS 15 removed the right-click-open bypass. Before
# a public release this is either Developer ID + notarization or a release note that says
# build-from-source. That is Orel's call and it is recorded in
# docs/plans/2026-08-08-terminal-first-tauri.md.
codesign --force -s - "$BUILD"
codesign -dv "$BUILD" 2>&1 | grep -q "Info.plist=not bound" && {
  echo "codesign failed to bind Info.plist" >&2; exit 1; }
codesign --verify --deep --strict "$BUILD"

INSTALL="$HOME/Applications/Mind2t.app"
mkdir -p "$HOME/Applications"
# Replace in place. The bundle is regenerable by definition, so this is not destroying anything a
# rebuild cannot produce again.
rm -rf "$INSTALL"
cp -R "$BUILD" "$INSTALL"

# THE STAGING BUNDLE IS REMOVED, and Spotlight is told never to index what replaces it.
#
# `cargo tauri build` writes its .app under target/, which Spotlight indexes, so Launchpad and
# Spotlight showed TWO Mind2t apps - the installed one and the build product. They are the same
# bytes, which makes it worse rather than better: the operator cannot tell which is which, and
# the answer "they are identical" is only true until the next build half-finishes.
#
# The marker is written BEFORE the removal so a build that fails after this point still leaves
# an unindexed tree. It lives in target/, which `cargo clean` wipes - hence recreated every run
# rather than committed.
: > "$ROOT/target/.metadata_never_index"
rm -rf "$BUILD"

echo "installed $INSTALL (version $VERSION, executable $EXEC)"
echo "staging bundle removed; target/ marked never-index so only one Mind2t is discoverable"
