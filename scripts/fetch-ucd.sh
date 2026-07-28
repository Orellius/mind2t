#!/usr/bin/env bash
# Fetches the Unicode bidi conformance suite into vendor/ucd/.
#
# This is slice 5.5's oracle, and it plays the same role libghostty-vt plays for the core:
# an external reference implementation that decides whether we are right. It is NOT the ABI
# -- `grep -rni bidi ../ruuah/include/ghostty/vt/*.h` returns nothing, so libghostty-vt has
# no opinion about reordering and cannot be the oracle here.
#
# Vendored rather than committed, for the same reason the oracle is: ~15 MB of static data
# that is reproducible from a URL. `vendor/` is gitignored.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dest="$here/vendor/ucd"
base="https://www.unicode.org/Public/UCD/latest/ucd"

mkdir -p "$dest"

for file in BidiTest.txt BidiCharacterTest.txt; do
    echo "fetching $file"
    curl -sSL --fail --max-time 120 -o "$dest/$file.tmp" "$base/$file"
    mv "$dest/$file.tmp" "$dest/$file"
done

# The suite is versioned, and a version bump can legitimately change expected results. Record
# it so a conformance failure can be told apart from a Unicode revision.
version="$(head -1 "$dest/BidiCharacterTest.txt" | sed 's/^# BidiCharacterTest-//; s/\.txt$//')"
cat > "$here/ucd.lock" <<EOF
# Which Unicode bidi conformance suite the reordering was verified against.
# Written by scripts/fetch-ucd.sh; commit this file when it changes.
version = "$version"
source = "$base"
EOF

echo
echo "Unicode $version"
wc -l "$dest"/*.txt
