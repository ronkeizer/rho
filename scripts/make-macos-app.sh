#!/usr/bin/env bash
# Build a macOS Rho.app bundle (with a proper Dock icon) from the release
# binary. macOS-only — uses sips + iconutil, which ship with the OS.
#
#   ./scripts/make-macos-app.sh
#
# Output: target/macos/Rho.app  (and the derived target/macos/Rho.icns).
# The master image is assets/icon.png — regenerate it with
# scripts/gen-icon.py if you change the artwork.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MASTER="$ROOT/assets/icon.png"
OUT="$ROOT/target/macos"
APP="$OUT/Rho.app"
ICONSET="$OUT/Rho.iconset"
ICNS="$OUT/Rho.icns"

[ "$(uname)" = "Darwin" ] || { echo "macOS only (needs sips/iconutil)." >&2; exit 1; }
[ -f "$MASTER" ] || { echo "missing $MASTER — run: python3 scripts/gen-icon.py" >&2; exit 1; }

echo "==> building release binary"
( cd "$ROOT" && cargo build --release )

echo "==> rendering iconset from $MASTER"
rm -rf "$ICONSET" "$APP"
mkdir -p "$ICONSET" "$OUT"
for sz in 16 32 128 256 512; do
    sips -z "$sz" "$sz"       "$MASTER" --out "$ICONSET/icon_${sz}x${sz}.png"    >/dev/null
    sips -z $((sz*2)) $((sz*2)) "$MASTER" --out "$ICONSET/icon_${sz}x${sz}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$ICNS"

echo "==> assembling $APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$ROOT/target/release/rho" "$APP/Contents/MacOS/rho"
cp "$ICNS" "$APP/Contents/Resources/Rho.icns"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>            <string>Rho</string>
    <key>CFBundleDisplayName</key>     <string>Rho</string>
    <key>CFBundleIdentifier</key>      <string>com.ronkeizer.rho</string>
    <key>CFBundleVersion</key>         <string>0.1.0</string>
    <key>CFBundleShortVersionString</key><string>0.1.0</string>
    <key>CFBundlePackageType</key>     <string>APPL</string>
    <key>CFBundleExecutable</key>      <string>rho</string>
    <key>CFBundleIconFile</key>        <string>Rho.icns</string>
    <key>NSHighResolutionCapable</key> <true/>
    <!-- Lets the bundle prompt for Automation permission so the SSH /
         Docker shell / Claude Code actions can open a terminal via Apple
         events. Without a usage string macOS may deny silently. -->
    <key>NSAppleEventsUsageDescription</key>
    <string>Rho opens a terminal window to start SSH, Docker, and Claude Code sessions.</string>
</dict>
</plist>
PLIST

echo "==> done: $APP"
echo "    open it with:  open \"$APP\""
