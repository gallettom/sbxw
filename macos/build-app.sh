#!/usr/bin/env bash
# Build the sbxw Island menu-bar app into a distributable, ad-hoc-signed
# `SbxwIsland.app` bundle and a zip for the GitHub release / install.sh.
#
# Usage:
#   macos/build-app.sh [OUTPUT_DIR]
# Env:
#   SBXW_ISLAND_VERSION   version string baked into Info.plist (default 0.0.0-dev)
#
# Produces:
#   OUTPUT_DIR/SbxwIsland.app          the bundle (runnable locally)
#   OUTPUT_DIR/SbxwIsland-macos.zip    zipped bundle (the release artifact)
set -euo pipefail

APP_NAME="SbxwIsland"
HERE="$(cd "$(dirname "$0")" && pwd)"
PKG="$HERE/$APP_NAME"
OUT="${1:-$HERE/dist}"
VERSION="${SBXW_ISLAND_VERSION:-0.0.0-dev}"

[ "$(uname -s)" = "Darwin" ] || { echo "build-app.sh only runs on macOS" >&2; exit 1; }

echo "Building $APP_NAME $VERSION (universal arm64 + x86_64)…"
# One invocation produces a universal binary; --show-bin-path then reports where.
swift build -c release --package-path "$PKG" --arch arm64 --arch x86_64
BIN_DIR="$(swift build -c release --package-path "$PKG" --arch arm64 --arch x86_64 --show-bin-path)"
BIN="$BIN_DIR/$APP_NAME"
[ -x "$BIN" ] || { echo "built binary not found at $BIN" >&2; exit 1; }

APP="$OUT/$APP_NAME.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/$APP_NAME"
sed "s/__VERSION__/$VERSION/g" "$PKG/Info.plist" > "$APP/Contents/Info.plist"

# Ad-hoc signature: required for the binary to run at all on Apple Silicon, and
# it gives the app a stable identity so its Automation permission sticks.
codesign --force --deep --sign - "$APP"

# `ditto` preserves the bundle layout; --keepParent zips the .app itself.
rm -f "$OUT/$APP_NAME-macos.zip"
( cd "$OUT" && ditto -c -k --sequesterRsrc --keepParent "$APP_NAME.app" "$APP_NAME-macos.zip" )

echo "✓ $APP"
echo "✓ $OUT/$APP_NAME-macos.zip"
