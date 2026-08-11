#!/bin/bash
# Assembles the SWIFT host into Mind2t.app and installs it to ~/Applications.
#
# Orel's call, 2026-08-11: the host goes back to Swift. This script is the Swift-era bundler
# (last seen at 0c72892, before T7 replaced it with the Tauri one) brought back and brought
# forward. It shipped briefly as `build-swift-app.sh` while the Tauri host was still alive and
# took this name back the moment that host was deleted, because one product has one install path.
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
# Used twice below - to UNREGISTER the staging bundle and to RE-REGISTER the installed one - so
# it is named once here rather than spelled out at both sites.
LSREGISTER_TOOL="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
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
  # LANCZOS, AND THE FILTER IS THE WHOLE POINT OF THIS BLOCK.
  #
  # The mark carries a SILVER HAIRLINE around the squircle (855e507). A hairline is one or two
  # pixels at 1024, so what happens to it on the way down to 32 is decided entirely by the
  # resampling filter. `sips -z` uses a soft one: measured 2026-08-11 at 128px, the rim row read
  # 48 71 07 - the bright edge smeared one pixel INWARD and dimmed - against 7A 43 00 in the
  # artwork Orel approved. RMSE 0.0817. At Dock sizes that reads as the rim being gone, and it
  # was reported twice as "the icon reverted to the old one".
  #
  # ImageMagick with -filter Lanczos reproduces the approved rim EXACTLY (7A 43 00 02) at
  # RMSE 0.0027, a thirty-fold improvement, because that is the filter the original per-size
  # renders were made with. Mitchell, Catrom, Triangle and Point were all measured and all lose.
  #
  # THE FALLBACK IS LOUD. A softened hairline is invisible unless you know to look, which is
  # exactly how this shipped twice; if magick is absent the build says so rather than quietly
  # producing the wrong icon again.
  if command -v magick >/dev/null 2>&1; then
    RESIZE_NOTE="magick -filter Lanczos"
  else
    RESIZE_NOTE="sips (SOFT: the silver hairline will not survive the downsample)"
    echo "WARNING: ImageMagick not found. The icon's hairline will soften at small sizes." >&2
    echo "         brew install imagemagick, then rebuild." >&2
  fi
  for size in 16 32 128 256 512; do
    for target in "$size" "$((size * 2))"; do
      if [ "$target" = "$size" ]; then
        out="$ICONSET/icon_${size}x${size}.png"
      else
        out="$ICONSET/icon_${size}x${size}@2x.png"
      fi
      if command -v magick >/dev/null 2>&1; then
        magick "$ICON_SRC" -filter Lanczos -resize "${target}x${target}" "$out"
      else
        sips -z "$target" "$target" "$ICON_SRC" --out "$out" >/dev/null
      fi
    done
  done
  iconutil -c icns "$ICONSET" -o "$BUILD/Contents/Resources/Mind2t.icns"

  # THE HAIRLINE IS ASSERTED, not assumed. The whole defect was that a wrong icon looks like a
  # right icon, so the build measures the rim rather than trusting the filter it just chose.
  # Row 64 of the 128px face, leftmost pixel: the approved artwork puts the rim's brightest
  # value ON the edge (0x7A). A soft downsample moves the peak inward and leaves the edge dark,
  # so a dim edge pixel is the signature of the bug and is what this refuses.
  if command -v magick >/dev/null 2>&1; then
    edge=$(magick "$ICONSET/icon_128x128.png" -crop 1x1+0+64 +repage -depth 8 -format "%[fx:int(255*r)]" info:)
    if [ "$edge" -lt 90 ]; then
      echo "error: the icon's silver hairline did not survive the downsample" >&2
      echo "       edge pixel at (0,64) of the 128px face is $edge, expected ~122" >&2
      exit 1
    fi
    ICON_NOTE="hairline ok (edge $edge, $RESIZE_NOTE)"
  else
    ICON_NOTE="UNVERIFIED ($RESIZE_NOTE)"
  fi
else
  echo "WARNING: $ICON_SRC missing, the bundle ships without an icon" >&2
  ICON_NOTE="ABSENT"
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
# A STABLE IDENTITY, AND THIS IS WHY IT IS NOT AD-HOC ANY MORE.
#
# `codesign -s -` makes the app's DESIGNATED REQUIREMENT a bare cdhash:
#
#   # designated => cdhash H"f0c23e67af5314117812a89791b1f076fa32f97f"
#
# The cdhash is a hash of the code, so EVERY REBUILD IS A DIFFERENT APPLICATION as far as macOS
# is concerned. Everything that remembers an app by identity then accumulates a row per build:
# TCC's Privacy panes, Launchpad, login items, "open with". Measured 2026-08-11, after Orel
# reported duplicate Mind2t apps for the second time - the first time it was diagnosed as a
# LaunchServices staging-bundle registration, which was a real and different bug, fixed, and
# NOT this one. Two causes, same symptom; that is why the first fix did not end it.
#
# Signing with a stable certificate instead makes the requirement
#
#   identifier "com.orellius.mind2t" and certificate leaf = H"<cert hash>"
#
# which does not move when the code does. One app, permanently, and TCC grants survive rebuilds
# so Screen Recording and friends stop needing to be re-granted.
#
# SELF-SIGNED, deliberately. This is NOT Developer ID and NOT notarization: it costs nothing,
# needs no Apple account, and a downloaded copy is still refused - which is correct, because
# Mind2t is a local driver tool until Orel says it ships (his call, 2026-08-11). It fixes the
# LOCAL identity churn and claims nothing about distribution.
#
# Absent the identity the build still works, ad-hoc, with the consequence named out loud rather
# than a silent regression to the behaviour above. `scripts/make-signing-identity.sh` creates it.
SIGN_IDENTITY="${MIND2T_SIGN_IDENTITY:-Mind2t Local Signing}"
if security find-identity -v -p codesigning 2>/dev/null | grep -qF "$SIGN_IDENTITY"; then
  codesign --force --options runtime -s "$SIGN_IDENTITY" "$BUILD"
  SIGNED_AS="$SIGN_IDENTITY"
else
  codesign --force -s - "$BUILD"
  SIGNED_AS="ad-hoc"
  echo "warning: no '$SIGN_IDENTITY' code-signing identity; signed ad-hoc." >&2
  echo "         Every rebuild will be a NEW identity to macOS, so duplicate Mind2t entries" >&2
  echo "         will accumulate in Privacy & Security and Launchpad. Fix it once with:" >&2
  echo "           ./scripts/make-signing-identity.sh" >&2
fi
codesign -dv "$BUILD" 2>&1 | grep -q "Info.plist=not bound" && {
  echo "codesign failed to bind Info.plist" >&2; exit 1; }
codesign --verify --deep --strict "$BUILD"

# THE GATE, and it is the whole point of the block above: the requirement must not be a bare
# cdhash. Asserting "we signed with an identity" would pass on a signature that produced a
# cdhash requirement anyway; this asserts the PROPERTY that makes the duplicates stop.
# `#* *` because codesign prints the derived requirement with a comment marker in some cases and
# bare in others, and `certificate` rather than `certificate leaf` because a self-signed leaf IS
# the root and reports as `certificate root`. Both measured 2026-08-11.
REQUIREMENT="$(codesign -d -r- "$BUILD" 2>/dev/null | sed -n 's/^#* *designated => //p')"
if [ "$SIGNED_AS" != "ad-hoc" ]; then
  case "$REQUIREMENT" in
    *certificate*) : ;;
    *)
      echo "signed as '$SIGNED_AS' and the requirement is still identity-unstable:" >&2
      echo "  $REQUIREMENT" >&2
      echo "Every rebuild would keep minting a new app identity. Refusing to install." >&2
      exit 1 ;;
  esac
fi

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

# THE STAGING BUNDLE IS REMOVED, and Spotlight is told never to index what replaces it.
#
# `build-app.sh` has carried this block since the Tauri host shipped and it says why: the build
# product and the installed app are the SAME BYTES, which makes two of them worse rather than
# better - the operator cannot tell which is which, and "they are identical" is only true until
# the next build half-finishes. This script shipped without it in #48 and Orel reported "there
# are double apps now" within the hour. Reintroducing a defect another script had already fixed
# and documented is the argument for reading the sibling before writing the replacement.
#
# Three steps, because two of them are not enough. LaunchServices had the staging path REGISTERED
# (`lsregister -dump` listed both), and it keeps that registration after the directory is gone,
# so the bundle is unregistered explicitly rather than left for LS to notice. The marker is
# written BEFORE the removal so a build that dies after this point still leaves an unindexed
# tree, and it lives under swift/.build, which `swift package clean` wipes - hence written every
# run rather than committed.
: > "$ROOT/swift/.build/.metadata_never_index"
[ -x "$LSREGISTER_TOOL" ] && "$LSREGISTER_TOOL" -u "$BUILD" 2>/dev/null || true
rm -rf "$BUILD"

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
[ -x "$LSREGISTER_TOOL" ] && "$LSREGISTER_TOOL" -f "$INSTALL"
killall Dock 2>/dev/null || true

echo "installed $INSTALL"
echo "  version    $VERSION"
echo "  host       swift"
echo "  smoke      $(echo "$SMOKE_OUT" | grep 'SMOKE OK' | head -1)"
echo "  icon       ${ICON_NOTE:-unknown}"
