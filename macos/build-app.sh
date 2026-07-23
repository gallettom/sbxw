#!/usr/bin/env bash
# Build the sbxw Island menu-bar app into a distributable, ad-hoc-signed
# `SbxwIsland.app` bundle and a zip for the GitHub release / install.sh.
#
# Usage:
#   macos/build-app.sh [OUTPUT_DIR]
# Env:
#   SBXW_ISLAND_VERSION   override the version baked into Info.plist
#
# The app is versioned independently of the sbxw CLI (which the release tag
# tracks): bump ISLAND_VERSION below when the island itself ships a change.
#
# Produces:
#   OUTPUT_DIR/SbxwIsland.app          the bundle (runnable locally)
#   OUTPUT_DIR/SbxwIsland-macos.zip    zipped bundle (the release artifact)
#   OUTPUT_DIR/island-version.txt      the version, published alongside the zip
#                                      so `sbxw update` can tell whether the
#                                      installed app is stale without fetching it
set -euo pipefail

APP_NAME="SbxwIsland"
HERE="$(cd "$(dirname "$0")" && pwd)"
PKG="$HERE/$APP_NAME"
OUT="${1:-$HERE/dist}"
ISLAND_VERSION="1.0.0"
VERSION="${SBXW_ISLAND_VERSION:-$ISLAND_VERSION}"

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

# App icon: rasterize the 1024² sbxw mark into a multi-resolution AppIcon.icns
# (Info.plist references "AppIcon" via CFBundleIconFile). sips + iconutil ship
# with macOS, so no extra tooling is needed.
if [ -f "$PKG/AppIcon.png" ]; then
  ICONSET="$OUT/AppIcon.iconset"
  rm -rf "$ICONSET"; mkdir -p "$ICONSET"
  for sz in 16 32 128 256 512; do
    sips -z "$sz" "$sz"                 "$PKG/AppIcon.png" --out "$ICONSET/icon_${sz}x${sz}.png"    >/dev/null
    sips -z "$((sz*2))" "$((sz*2))"     "$PKG/AppIcon.png" --out "$ICONSET/icon_${sz}x${sz}@2x.png" >/dev/null
  done
  iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"
  rm -rf "$ICONSET"
else
  echo "warning: $PKG/AppIcon.png missing — building without an app icon" >&2
fi

# Ad-hoc signature: required for the binary to run at all on Apple Silicon, and
# it gives the app a stable identity so its Automation permission sticks.
codesign --force --deep --sign - "$APP"

# `ditto` preserves the bundle layout; --keepParent zips the .app itself.
rm -f "$OUT/$APP_NAME-macos.zip"
( cd "$OUT" && ditto -c -k --sequesterRsrc --keepParent "$APP_NAME.app" "$APP_NAME-macos.zip" )

# Published next to the zip: `sbxw update` reads it to compare against the
# installed bundle's CFBundleShortVersionString before re-downloading 5 MB.
printf '%s\n' "$VERSION" > "$OUT/island-version.txt"

echo "✓ $APP"
echo "✓ $OUT/$APP_NAME-macos.zip"
echo "✓ $OUT/island-version.txt ($VERSION)"
