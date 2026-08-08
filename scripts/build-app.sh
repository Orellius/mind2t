#!/bin/bash
# Assembles the Swift host into a real macOS app: Mind2t.app.
#
# SwiftPM builds executables, not bundles, so the bundle is assembled by hand:
# Info.plist + the release binary + Resources (the Mind2t splash, the icon). The
# app is INSTALLED to ~/Applications because anything launched outside a
# terminal must not live under ~/Desktop (TCC kills disclaimed children there
# silently -- SCAR-006). The repo stays the source of truth; the bundle is a
# build product.
#
# Inside the bundle the host is Hebrew-first: banner.sh present => splash +
# auto base direction by default (see main.swift). The bare CLI binary is
# untouched by any of this.
set -euo pipefail

cd "$(dirname "$0")/.."
# The full chain including the forced relink and the headless smoke -- the bundle
# only ever ships a binary the smoke has seen draw.
./scripts/build-swift.sh

APP_NAME="Mind2t VT"
BUILD="swift/.build/$APP_NAME.app"
# Mind2t's own mark, in this repo (2026-08-06). It used to be Ghostty's icon out of the oracle
# checkout, which was borrowed goods AND a build that broke the moment that checkout moved - it
# was archived the same day. `assets/icon/mind2t.svg` is the source; the PNG beside it is committed
# so a build needs no SVG rasteriser.
ICON_SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/assets/icon/mind2t-1024.png"

rm -rf "$BUILD"
mkdir -p "$BUILD/Contents/MacOS" "$BUILD/Contents/Resources"

cat > "$BUILD/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>            <string>Mind2t VT</string>
  <key>CFBundleDisplayName</key>     <string>Mind2t VT</string>
  <key>CFBundleIdentifier</key>      <string>com.orellius.mind2t-vt</string>
  <key>CFBundleExecutable</key>      <string>mind2t-host</string>
  <key>CFBundlePackageType</key>     <string>APPL</string>
  <key>CFBundleShortVersionString</key> <string>0.8.0</string>
  <key>CFBundleIconFile</key>        <string>Mind2tVT</string>
  <key>NSHighResolutionCapable</key> <true/>
  <key>LSMinimumSystemVersion</key>  <string>14.0</string>
</dict>
</plist>
PLIST

cp swift/.build/release/mind2t-host "$BUILD/Contents/MacOS/mind2t-host"
cp swift/Resources/banner.sh "$BUILD/Contents/Resources/banner.sh"
# S6 panels: one self-contained document, or none at all. A bundle without it still
# runs -- WebPanel.documentURL returns nil and the chord reports rather than opening a
# blank card. build-swift.sh above has already built it when bun is available.
if [ -f web/dist/index.html ]; then
  mkdir -p "$BUILD/Contents/Resources/web"
  cp web/dist/index.html "$BUILD/Contents/Resources/web/index.html"
fi
# Shell integration (S2): the ZDOTDIR bootstrap + the OSC 133 hooks the blocks read.
mkdir -p "$BUILD/Contents/Resources/shell/zdotdir"
cp shell/mind2t-integration.zsh "$BUILD/Contents/Resources/shell/mind2t-integration.zsh"
cp shell/zdotdir/.zshenv "$BUILD/Contents/Resources/shell/zdotdir/.zshenv"

# Icon: Mind2t's own mark. Skipped silently if the artwork is missing -- the app runs without it.
if [ -f "$ICON_SRC" ]; then
  ICONSET="swift/.build/Mind2tVT.iconset"
  rm -rf "$ICONSET"; mkdir -p "$ICONSET"
  # Already square at 1024, so it is resized directly -- no padding pass. Padding a square source
  # onto a square canvas insets the whole mark, which is how an icon ends up looking smaller than
  # every other icon in the Dock.
  for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$ICON_SRC" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
    sips -z "$((size * 2))" "$((size * 2))" "$ICON_SRC" \
      --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
  done
  iconutil -c icns "$ICONSET" -o "$BUILD/Contents/Resources/Mind2tVT.icns"
fi

# Sign the assembled bundle. SwiftPM's binary arrives only linker-signed: Info.plist
# unbound, no sealed resources -- and TCC identifies an app by its signature, so macOS
# SILENTLY denies Desktop/Documents/Downloads without ever showing the permission
# prompt (measured live 2026-07-30: "cannot cd into Desktop" inside the app). A real
# ad-hoc signature over the bundle binds the plist and seals Resources, which is enough
# for TCC to attribute the app and prompt. Known cost: the ad-hoc identity is the
# cdhash, so every rebuild is a new identity and macOS may re-prompt after reinstall.
codesign --force -s - "$BUILD"
codesign -dv "$BUILD" 2>&1 | grep -q "Info.plist=not bound" && {
  echo "codesign failed to bind Info.plist" >&2; exit 1; }
codesign --verify --deep --strict "$BUILD"

# Install (replace-in-place is fine: the bundle is regenerable by definition).
INSTALL="$HOME/Applications/$APP_NAME.app"
rm -rf "$INSTALL"
mkdir -p "$HOME/Applications"
cp -R "$BUILD" "$INSTALL"

echo "installed $INSTALL"
