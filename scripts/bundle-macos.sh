#!/usr/bin/env bash
# Build a release binary and wrap it in "SwitchHosts Lite.app".
#
# Usage: scripts/bundle-macos.sh [--target <triple>] [--out <dir>]
#   --target  Rust target triple, e.g. aarch64-apple-darwin or x86_64-apple-darwin
#             (default: the host)
#   --out     Directory to put the .app in (default: dist)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

APP_NAME="SwitchHosts Lite"
BIN_NAME="switch-hosts-lite"
BUNDLE_ID="com.switchhostslite.app"

TARGET=""
OUT_DIR="$ROOT/dist"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --target) TARGET="${2:?--target needs a value}"; shift 2 ;;
        --out) OUT_DIR="${2:?--out needs a value}"; shift 2 ;;
        -h|--help) sed -n '2,8p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
mkdir -p "$OUT_DIR"
OUT_DIR="$(cd "$OUT_DIR" && pwd)"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
TARGET_DIR="$(cargo metadata --format-version 1 --no-deps |
    python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])')"

if [[ -n "$TARGET" ]]; then
    cargo build --release --locked --target "$TARGET"
    BIN="$TARGET_DIR/$TARGET/release/$BIN_NAME"
else
    cargo build --release
    BIN="$TARGET_DIR/release/$BIN_NAME"
fi

APP="$OUT_DIR/$APP_NAME.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/$BIN_NAME"
cp assets/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>$APP_NAME</string>
    <key>CFBundleDisplayName</key>
    <string>$APP_NAME</string>
    <key>CFBundleIdentifier</key>
    <string>$BUNDLE_ID</string>
    <key>CFBundleExecutable</key>
    <string>$BIN_NAME</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleVersion</key>
    <string>$VERSION</string>
    <key>CFBundleShortVersionString</key>
    <string>$VERSION</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST

plutil -lint "$APP/Contents/Info.plist" >/dev/null
codesign --force --deep -s - "$APP" >/dev/null 2>&1 || echo "warning: ad-hoc signing failed" >&2

echo "Built $APP"
