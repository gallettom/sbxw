#!/usr/bin/env bash
# Build the sbxw desktop app into an ad-hoc-signed `sbxw.app` bundle and a zip
# for the GitHub release.
#
# Usage:
#   desktop/packaging/build-app.sh [OUTPUT_DIR]
# Env:
#   SBXW_DESKTOP_VERSION   the version written to Info.plist (default: the
#                          workspace version in the root Cargo.toml, which the
#                          release tag tracks)
#   SBXW_DESKTOP_ARCHS     the architectures to build, space separated
#                          (default: "aarch64 x86_64", a universal binary)
#
# Produces:
#   OUTPUT_DIR/sbxw.app                  the bundle (runnable locally)
#   OUTPUT_DIR/sbxw-desktop-macos.zip    the zipped bundle (the release artifact)
#   OUTPUT_DIR/desktop-version.txt       the version, published beside the zip
#
# The built-in screens, their docs and the icons are compiled into the binary
# (desktop/src/builtin.rs): the bundle holds the binary, Info.plist and the
# icon, nothing else. The app talks to a running sbxw daemon (127.0.0.1:7681,
# token in ~/.sbxw/run/ui-token): it does not start one.
set -euo pipefail

APP_NAME="sbxw"
HERE="$(cd "$(dirname "$0")" && pwd)"
DESKTOP="$(cd "$HERE/.." && pwd)"
ROOT="$(cd "$DESKTOP/.." && pwd)"
OUT="${1:-$DESKTOP/dist}"
ARCHS="${SBXW_DESKTOP_ARCHS:-aarch64 x86_64}"

[ "$(uname -s)" = "Darwin" ] || { echo "build-app.sh only runs on macOS" >&2; exit 1; }

# The workspace version: the first `version = "..."` after [workspace.package].
workspace_version() {
  awk '/^\[workspace\.package\]/ { on = 1; next } /^\[/ { on = 0 } on && /^version *=/ { gsub(/[" ]/, "", $0); sub(/^version=/, "", $0); print; exit }' "$ROOT/Cargo.toml"
}
VERSION="${SBXW_DESKTOP_VERSION:-$(workspace_version)}"
case "$VERSION" in
  ''|*[!0-9A-Za-z.+-]*) echo "not a version: '$VERSION'" >&2; exit 1 ;;
esac

echo "Building $APP_NAME desktop $VERSION ($ARCHS)…"
BINS=()
for arch in $ARCHS; do
  target="${arch}-apple-darwin"
  # The toolchain is the one desktop/rust-toolchain.toml pins (Rune's).
  ( cd "$DESKTOP" && rustup target add "$target" >/dev/null && cargo build --locked --release --bin "$APP_NAME" --target "$target" )
  bin="$DESKTOP/target/$target/release/$APP_NAME"
  [ -x "$bin" ] || { echo "built binary not found at $bin" >&2; exit 1; }
  BINS+=("$bin")
done

APP="$OUT/$APP_NAME.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
if [ "${#BINS[@]}" -gt 1 ]; then
  lipo -create -output "$APP/Contents/MacOS/$APP_NAME" "${BINS[@]}"
else
  cp "${BINS[0]}" "$APP/Contents/MacOS/$APP_NAME"
fi
chmod 755 "$APP/Contents/MacOS/$APP_NAME"

# Info.plist as kept in git, with the version set (PlistBuddy ships with macOS).
cp "$HERE/Info.plist" "$APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $VERSION" "$APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $VERSION" "$APP/Contents/Info.plist"
cp "$HERE/sbxw.icns" "$APP/Contents/Resources/sbxw.icns"

# Ad-hoc signature: required for the binary to run at all on Apple Silicon, and
# it gives the app a stable identity so its microphone permission sticks.
codesign --force --deep --sign - "$APP"

# `ditto` preserves the bundle layout; --keepParent zips the .app itself.
rm -f "$OUT/sbxw-desktop-macos.zip"
( cd "$OUT" && ditto -c -k --sequesterRsrc --keepParent "$APP_NAME.app" "sbxw-desktop-macos.zip" )
printf '%s\n' "$VERSION" > "$OUT/desktop-version.txt"

echo "✓ $APP"
echo "✓ $OUT/sbxw-desktop-macos.zip"
echo "✓ $OUT/desktop-version.txt ($VERSION)"
