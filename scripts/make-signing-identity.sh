#!/usr/bin/env bash
# Creates the stable local code-signing identity Mind2t is signed with. Run ONCE per machine.
#
# WHY THIS EXISTS. `codesign -s -` (ad-hoc) makes the app's designated requirement a bare cdhash,
# which is a hash of the code, so every rebuild is a DIFFERENT APPLICATION to macOS. Everything
# that remembers an app by identity then grows a row per build: TCC's Privacy panes, Launchpad,
# login items, "open with". That is the duplicate-Mind2t symptom, and no amount of cleaning up
# the duplicates fixes it, because the next build makes another one.
#
# A stable certificate makes the requirement `identifier "..." and certificate leaf = H"..."`,
# which does not move when the code does. One app, permanently.
#
# WHAT THIS IS NOT. Not Developer ID, not notarization, not distribution. It is self-signed and
# costs nothing, and a downloaded copy is still refused by Gatekeeper - correct, because Mind2t
# is a local driver tool until it is deliberately shipped. Locally built apps are not
# quarantined, so Gatekeeper never runs on this one.
#
# REVERSIBLE. Everything it adds is one certificate plus its key in the LOGIN keychain, named
# below. To undo: Keychain Access, search the name, delete both rows. Or:
#   security delete-certificate -c "Mind2t Local Signing" ~/Library/Keychains/login.keychain-db
#
# It touches nothing in the System keychain and needs no sudo.
set -euo pipefail

NAME="${MIND2T_SIGN_IDENTITY:-Mind2t Local Signing}"
KEYCHAIN="$HOME/Library/Keychains/login.keychain-db"

if security find-identity -v -p codesigning 2>/dev/null | grep -qF "$NAME"; then
  echo "ok: '$NAME' already exists; nothing to do."
  security find-identity -v -p codesigning | grep -F "$NAME"
  exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# 20 years. This is a local identity with no revocation story, and an expiry is a time bomb that
# would surface as a confusing signing failure years from now on a machine nobody is debugging.
#
# The extensions are not decoration: codesign REFUSES a certificate without the codeSigning EKU,
# and macOS refuses a leaf that claims to be a CA.
openssl req -x509 -newkey rsa:2048 -keyout "$work/key.pem" -out "$work/cert.pem" \
  -days 7300 -nodes -subj "/CN=$NAME/O=Orellius/C=IL" \
  -addext "basicConstraints=critical,CA:false" \
  -addext "keyUsage=critical,digitalSignature" \
  -addext "extendedKeyUsage=critical,codeSigning" 2>/dev/null

# THE LEGACY ALGORITHMS ARE LOAD-BEARING, not conservatism. OpenSSL 3.x defaults PKCS#12 to
# AES-256-CBC with a SHA-256 MAC, and Apple's Security framework cannot read that: the import
# fails with `SecKeychainItemImport: MAC verification failed during PKCS12 import (wrong
# password?)`, which names the one thing that is NOT wrong and sends you checking the password
# for as long as you believe it. Measured 2026-08-11 against Homebrew's OpenSSL 3.6.3, which is
# ahead of the system LibreSSL on PATH here. These flags produce pbeWithSHA1And3-KeyTripleDES-CBC
# with a SHA-1 MAC, which is what `security import` expects.
openssl pkcs12 -export -inkey "$work/key.pem" -in "$work/cert.pem" \
  -out "$work/bundle.p12" -passout pass:mind2t -name "$NAME" \
  -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES -macalg sha1 2>/dev/null

# -A, so codesign is not stopped by a keychain access prompt on every single build. The key is
# usable by local tools and never leaves this keychain.
security import "$work/bundle.p12" -k "$KEYCHAIN" -P mind2t -T /usr/bin/codesign -A

# Trust for CODE SIGNING ONLY, in the user domain. Not a root for TLS, not system-wide, no sudo.
# macOS may ask for the login password once here; that is the keychain, not this script.
security add-trusted-cert -r trustRoot -p codeSign -k "$KEYCHAIN" "$work/cert.pem" 2>/dev/null || {
  echo "note: trust settings were not applied. Signing usually still works; if codesign" >&2
  echo "      reports errSecInternalComponent, open Keychain Access, find '$NAME'," >&2
  echo "      Get Info -> Trust -> Code Signing: Always Trust." >&2
}

if ! security find-identity -v -p codesigning 2>/dev/null | grep -qF "$NAME"; then
  echo "error: '$NAME' was imported but is not a valid codesigning identity." >&2
  exit 1
fi

# PROVED, not assumed. A throwaway bundle is signed and its designated requirement read back: if
# it is still a bare cdhash then this identity did not solve the problem it exists for, and the
# script must say so rather than report success and leave the duplicates to keep appearing.
probe="$work/Probe.app/Contents/MacOS"
mkdir -p "$probe"
cp /usr/bin/true "$probe/Probe"
cat > "$work/Probe.app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleExecutable</key><string>Probe</string>
  <key>CFBundleIdentifier</key><string>com.orellius.mind2t.signing-probe</string>
</dict></plist>
PLIST
codesign --force -s "$NAME" "$work/Probe.app" 2>/dev/null
# `# ` is optional and `root` is not `leaf`: codesign prints the derived requirement with a
# comment marker in some cases and bare in others, and for a SELF-SIGNED certificate the leaf IS
# the root, so it reports `certificate root`. Matching on `certificate leaf` cost one red run
# against a signature that was already correct.
requirement="$(codesign -d -r- "$work/Probe.app" 2>/dev/null | sed -n 's/^#* *designated => //p')"

# The property, stated as itself: the requirement must be pinned to a CERTIFICATE and not to the
# code's own hash. Asserting a particular wording would keep breaking on wordings that are fine.
case "$requirement" in
  *certificate*)
    echo "ok: '$NAME' created and proven."
    security find-identity -v -p codesigning | grep -F "$NAME"
    echo
    echo "A signed bundle's designated requirement is now:"
    echo "  $requirement"
    echo
    echo "That does not change when the code does, so macOS stops treating each rebuild as a"
    echo "new app. Rebuild with ./scripts/build-app.sh to switch Mind2t onto it."
    ;;
  *)
    echo "error: signing with '$NAME' still produced an unstable requirement:" >&2
    echo "  $requirement" >&2
    echo "The duplicates would keep appearing, so this is a failure, not a warning." >&2
    exit 1 ;;
esac
