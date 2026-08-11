#!/bin/bash
# Assembles the SWIFT host into Mind2t.app and installs it to ~/Applications.
#
# Orel's call, 2026-08-11: the host goes back to Swift. This script is the Swift-era bundler
# (last seen at 0c72892, before T7 replaced it with the Tauri one) brought back and brought
# forward. It carries a NEW name on purpose - `build-app.sh` still builds the Tauri bundle and
# stays until that host is deleted, because a repo that can only ship after a rewrite lands is a
# repo that cannot ship.
#
# SwiftPM emits executables, not bundles, so the bundle is assembled by hand: Info.plist, the
# release binary, Resources. Tauri's bundler did this part for free and that convenience is the
# one real thing given up here.
#
# THE INSTALL REPLACES, NEVER DUPLICATES (Orel's standing order). One Mind2t.app exists at
# ~/Applications and it is whichever host was built last. Never under ~/Desktop: TCC kills
# disclaimed children there silently (SCAR-006). The repo is the source of truth; the bundle is
# a build product.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"

APP_NAME="Mind2t"
# The SAME identifier the Tauri bundle uses. This is a replacement, not a sibling: a second
# identifier would give macOS two apps to disagree about, two Dock entries and two sets of TCC
# grants, which is exactly the duplication the standing order forbids.
BUNDLE_ID="com.orellius.mind2t"
BUILD="swift/.build/$APP_NAME.app"
INSTALL="$HOME/Applications/$APP_NAME.app"
ICON_SRC="$ROOT/assets/icon/mind2t-1024.png"

# The full chain including the forced relink and all five smoke stages. The bundle only ever
# ships a binary the smoke has already seen draw.
./scripts/build-swift.sh

# THE VERSION COMES FROM THE TAG, NEVER FROM A LITERAL. It was hardcoded at 0.8.0 and stayed
# there through thirteen releases, so every app installed in that window claimed a version it was
# not - and nothing errors, because a plist string is only ever read by a human or an updater.
# Tag BEFORE building: `git describe` on an untagged tree yields the PREVIOUS tag and bakes it in.
VERSION="$(git describe --tags --abbrev=0 2>/dev/null | sed 's/^v//')"
VERSION="${VERSION:-0.0.0}"

echo "building $APP_NAME.app (swift host) at version $VERSION"

rm -rf "$BUILD"
mkdir -p "$BUILD/Contents/MacOS" "$BUILD/Contents/Resources"

cat > "$BUILD/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>               <string>Mind2t</string>
  <key>CFBundleDisplayName</key>        <string>Mind2t</string>
  <key>CFBundleIdentifier</key>         <string>__BUNDLE_ID__</string>
  <key>CFBundleExecutable</key>         <string>mind2t-host</string>
  <key>CFBundlePackageType</key>        <string>APPL</string>
  <key>CFBundleShortVersionString</key> <string>__VERSION__</string>
  <key>CFBundleIconFile</key>           <string>Mind2t</string>
  <key>NSHighResolutionCapable</key>    <true/>
  <key>LSMinimumSystemVersion</key>     <string>13.0</string>
</dict>
</plist>
PLIST

# Quoted heredoc above, deliberately - a plist is full of characters a shell would eat. The two
# values that must vary are substituted here instead.
sed -i '' "s/__VERSION__/$VERSION/; s/__BUNDLE_ID__/$BUNDLE_ID/" "$BUILD/Contents/Info.plist"

cp swift/.build/release/mind2t-host "$BUILD/Contents/MacOS/mind2t-host"
cp swift/Resources/banner.sh "$BUILD/Contents/Resources/banner.sh"

# THE FONTS, which the Tauri bundle started shipping on 2026-08-11 and this one must too.
# `crates/render/src/font.rs` probes `<exe>/../Resources/fonts` FIRST, and the Swift executable
# sits at Contents/MacOS/mind2t-host, so that path resolves here with no Rust change at all.
# Without them Hebrew falls through to Arial Hebrew, which is proportional: nothing errors, the
# grid simply drifts, and the only way to notice is to know what correct looks like.
mkdir -p "$BUILD/Contents/Resources/fonts"
cp assets/fonts/*.ttf assets/fonts/LICENSE-*.txt "$BUILD/Contents/Resources/fonts/"

# S6 panels: one self-contained document, or none at all. A bundle without it still runs -
# WebPanel.documentURL returns nil and the chord reports rather than opening a blank card.
if [ -f web/dist/index.html ]; then
  mkdir -p "$BUILD/Contents/Resources/web"
  cp web/dist/index.html "$BUILD/Contents/Resources/web/index.html"
fi

# Shell integration (S2): the ZDOTDIR bootstrap and the OSC 133 hooks the blocks read. The Tauri
# bundle ships NEITHER, which is why its panes have no prompt marks.
mkdir -p "$BUILD/Contents/Resources/shell/zdotdir"
cp shell/mind2t-integration.zsh "$BUILD/Contents/Resources/shell/mind2t-integration.zsh"
cp shell/zdotdir/.zshenv "$BUILD/Contents/Resources/shell/zdotdir/.zshenv"

# Icon. Skipped out loud rather than silently if the artwork is missing - the app runs without
# one, but a build that quietly drops the product's mark is a build nobody checks.
if [ -f "$ICON_SRC" ]; then
  ICONSET="swift/.build/Mind2t.iconset"
  rm -rf "$ICONSET"; mkdir -p "$ICONSET"
  # Already square at 1024, so it is resized directly - no padding pass. Padding a square source
  # onto a square canvas insets the whole mark, which is how an icon ends up looking smaller
  # than every other icon in the Dock.
  for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$ICON_SRC" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
    sips -z "$((size * 2))" "$((size * 2))" "$ICON_SRC" \
      --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
  done
  iconutil -c icns "$ICONSET" -o "$BUILD/Contents/Resources/Mind2t.icns"
else
  echo "WARNING: $ICON_SRC missing, the bundle ships without an icon" >&2
fi

# THE BUNDLED BINARY MUST BE THE ONE JUST BUILT. b0a1d30 added this to the Tauri script after a
# build installed a binary it had not made; the same hole exists here the moment anyone adds a
# second copy step above.
#
# BEFORE `codesign`, NOT AFTER. Signing REWRITES the executable in place - a bundle signature is
# stored in the Mach-O - so the two sums stop matching the instant the bundle is signed, and this
# check placed after it fails 100% of the time for a reason that has nothing to do with what it
# is testing. Measured here on the first run, and it also explains a discrepancy noticed earlier
# the same day between ~/Applications/Mind2t.app's binary and target/release/mind2t: that pair
# differs for the same harmless reason and is not evidence of a stale install.
BUILT_SUM="$(shasum -a 256 swift/.build/release/mind2t-host | cut -d' ' -f1)"
BUNDLED_SUM="$(shasum -a 256 "$BUILD/Contents/MacOS/mind2t-host" | cut -d' ' -f1)"
[ "$BUILT_SUM" = "$BUNDLED_SUM" ] || {
  echo "the bundled binary is not the one just built ($BUNDLED_SUM != $BUILT_SUM)" >&2; exit 1; }

# Sign the assembled bundle. SwiftPM's binary arrives only LINKER-signed: Info.plist unbound, no
# sealed resources - and TCC identifies an app by its signature, so macOS SILENTLY denies
# Desktop/Documents/Downloads without ever showing the permission prompt (measured live
# 2026-07-30: "cannot cd into Desktop" inside the app). A real ad-hoc signature binds the plist
# and seals Resources, which is enough for TCC to attribute the app and prompt.
#
# Known cost, unchanged: the ad-hoc identity IS the cdhash, so every rebuild is a new identity
# and macOS may re-prompt after a reinstall. Insufficient for distribution now the repo is
# public - a downloaded artifact reports "damaged and can't be opened" - which is an open item
# for a Developer ID, not something this script can fix.
codesign --force -s - "$BUILD"
codesign -dv "$BUILD" 2>&1 | grep -q "Info.plist=not bound" && {
  echo "codesign failed to bind Info.plist" >&2; exit 1; }
codesign --verify --deep --strict "$BUILD"

# THE BUNDLE ITSELF RUNS, not just the loose binary. This is the assertion that discriminates:
# `--smoke` drives a child's output all the way to pixels and prints SMOKE OK, and no other
# binary in this tree prints that. It runs the BUNDLED copy so a broken plist, a bad signature
# or a missing resource is caught here rather than by Orel double-clicking.
#
# macOS ships no GNU `timeout`; `brew install coreutils` provides one. Absent, the gate still
# runs, unbounded - refusing to check at all because the watchdog is missing would be worse.
TIMEOUT="$(command -v timeout || command -v gtimeout || true)"
if [ -n "$TIMEOUT" ]; then
  SMOKE_OUT="$("$TIMEOUT" 90 "$BUILD/Contents/MacOS/mind2t-host" --smoke 2>&1)"
else
  echo "note: no GNU timeout on PATH (brew install coreutils), running the gate unbounded" >&2
  SMOKE_OUT="$("$BUILD/Contents/MacOS/mind2t-host" --smoke 2>&1)"
fi
echo "$SMOKE_OUT" | grep -q "SMOKE OK" || {
  echo "the assembled bundle did not pass its own smoke:" >&2; echo "$SMOKE_OUT" >&2; exit 1; }

# Install, replacing in place. The bundle is regenerable by definition, and the standing order
# is one app, never a second one beside it.
rm -rf "$INSTALL"
mkdir -p "$HOME/Applications"
cp -R "$BUILD" "$INSTALL"

# BUST THE ICON CACHE, EVERY TIME, because replacing in place guarantees this problem.
#
# macOS caches an app's icon against its bundle identifier and path, and this script deliberately
# keeps BOTH constant so the install replaces rather than duplicates. The cost of that choice is
# that the Dock and Finder keep serving the icon they already had: measured 2026-08-11, the
# freshly installed bundle carried the correct silver-rimmed artwork at all ten sizes while every
# on-screen surface still drew the previous one, and Orel reported it as the icon having reverted.
#
# The artwork was never wrong, and that is the trap worth naming: the on-disk .icns is not
# evidence about what is on screen. Check `Contents/Resources/*.icns` before touching any
# artwork, because regenerating a correct icon to fix a cache produces a second wrong answer.
#
# `touch` alone is not enough - LaunchServices caches independently of mtime - so the bundle is
# re-registered and the Dock restarted. The Dock reappears immediately and no window is lost.
touch "$INSTALL"
LSREGISTER="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
[ -x "$LSREGISTER" ] && "$LSREGISTER" -f "$INSTALL"
killall Dock 2>/dev/null || true

echo "installed $INSTALL"
echo "  version    $VERSION"
echo "  host       swift"
echo "  smoke      $(echo "$SMOKE_OUT" | grep 'SMOKE OK' | head -1)"
