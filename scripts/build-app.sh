#!/bin/bash
# Assembles the Swift host into a real macOS app: RUUAH VT.app.
#
# SwiftPM builds executables, not bundles, so the bundle is assembled by hand:
# Info.plist + the release binary + Resources (the RUUAH splash, the icon). The
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
./scripts/build-host.sh
(cd swift && swift build -c release)

APP_NAME="RUUAH VT"
BUILD="swift/.build/$APP_NAME.app"
ICON_SRC="../ruuah/images/Ghostty.icon/Assets/Ghostty.png"

rm -rf "$BUILD"
mkdir -p "$BUILD/Contents/MacOS" "$BUILD/Contents/Resources"

cat > "$BUILD/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>            <string>RUUAH VT</string>
  <key>CFBundleDisplayName</key>     <string>RUUAH VT</string>
  <key>CFBundleIdentifier</key>      <string>com.orellius.ruuah-vt</string>
  <key>CFBundleExecutable</key>      <string>ruuah-host</string>
  <key>CFBundlePackageType</key>     <string>APPL</string>
  <key>CFBundleShortVersionString</key> <string>0.8.0</string>
  <key>CFBundleIconFile</key>        <string>RuuahVT</string>
  <key>NSHighResolutionCapable</key> <true/>
  <key>LSMinimumSystemVersion</key>  <string>14.0</string>
</dict>
</plist>
PLIST

cp swift/.build/release/ruuah-host "$BUILD/Contents/MacOS/ruuah-host"
cp swift/Resources/banner.sh "$BUILD/Contents/Resources/banner.sh"

# Icon: the RUUAH ghost from the fork's own icon artwork, when the fork is
# present. Skipped silently otherwise -- the app runs fine without it.
if [ -f "$ICON_SRC" ]; then
  ICONSET="swift/.build/RuuahVT.iconset"
  rm -rf "$ICONSET"; mkdir -p "$ICONSET"
  # The artwork is 475x541; pad onto a square dark canvas before resizing so
  # the ghost is not stretched.
  sips -p 541 541 --padColor 16151B "$ICON_SRC" --out "$ICONSET/base.png" >/dev/null
  for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$ICONSET/base.png" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
    sips -z "$((size * 2))" "$((size * 2))" "$ICONSET/base.png" \
      --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
  done
  rm "$ICONSET/base.png"
  iconutil -c icns "$ICONSET" -o "$BUILD/Contents/Resources/RuuahVT.icns"
fi

# Install (replace-in-place is fine: the bundle is regenerable by definition).
INSTALL="$HOME/Applications/$APP_NAME.app"
rm -rf "$INSTALL"
mkdir -p "$HOME/Applications"
cp -R "$BUILD" "$INSTALL"

echo "installed $INSTALL"
