#!/usr/bin/env bash
# Package Rho.app into a drag-to-Applications .dmg. macOS-only.
#
#   ./scripts/make-dmg.sh
#
# Builds the app bundle (via make-macos-app.sh), then lays out a disk image
# containing Rho.app next to an /Applications symlink so the user just drags
# one onto the other. Apple Silicon (arm64) only; unsigned.
#
# Output: target/macos/Rho-<version>-arm64.dmg
#
# NOTE: the .dmg is unsigned and un-notarized, so Gatekeeper will warn on
# first open. Users open it with right-click -> Open once, or:
#   xattr -dr com.apple.quarantine /Applications/Rho.app
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/target/macos"
APP="$OUT/Rho.app"
STAGE="$OUT/dmg"

[ "$(uname)" = "Darwin" ] || { echo "macOS only (needs hdiutil)." >&2; exit 1; }
[ "$(uname -m)" = "arm64" ] || echo "warning: not on arm64 — the binary will match this host's arch, not arm64." >&2

# Version from Cargo.toml ([package] version = "x.y.z"), for the filename.
VERSION="$(awk -F\" '/^version = /{print $2; exit}' "$ROOT/Cargo.toml")"
DMG="$OUT/Rho-${VERSION}-arm64.dmg"

echo "==> building app bundle"
"$ROOT/scripts/make-macos-app.sh"

echo "==> staging disk image contents"
rm -rf "$STAGE" "$DMG"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/Rho.app"
ln -s /Applications "$STAGE/Applications"

echo "==> creating $DMG"
hdiutil create \
    -volname "Rho ${VERSION}" \
    -srcfolder "$STAGE" \
    -fs HFS+ \
    -format UDZO \
    -ov \
    "$DMG" >/dev/null
rm -rf "$STAGE"

echo "==> done: $DMG"
echo "    test it with:  open \"$DMG\""
