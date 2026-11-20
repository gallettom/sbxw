#!/usr/bin/env bash
# Builds sbxw-tailnet, the Tailscale node of remote access (tailnet/README.md).
#
#   scripts/build-tailnet.sh [GOOS/GOARCH] [OUT]
#
# GOOS/GOARCH defaults to this machine (darwin/arm64 on an Apple-silicon Mac).
# OUT defaults to target/tailnet/<goos>-<goarch>/sbxw-tailnet. The daemon looks
# for the binary next to its own executable: copy it there, for example
#   cp target/tailnet/darwin-arm64/sbxw-tailnet "$(dirname "$(command -v sbxw)")/"
#
# Reproducible: the Go version is pinned by tailnet/go.mod (GOTOOLCHAIN=local:
# no toolchain download), modules by tailnet/go.sum (-mod=readonly), CGO off,
# -trimpath, no build id. Needs Go >= the version in tailnet/go.mod.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
src="$root/tailnet"

target="${1:-$(go env GOOS)/$(go env GOARCH)}"
goos="${target%/*}"
goarch="${target#*/}"
case "$goos/$goarch" in
  darwin/arm64|darwin/amd64|linux/arm64|linux/amd64) ;;
  *) echo "unsupported target: $target" >&2; exit 2 ;;
esac
out="${2:-$root/target/tailnet/$goos-$goarch/sbxw-tailnet}"
case "$out" in /*) ;; *) out="$PWD/$out" ;; esac

want="$(sed -n 's/^go //p' "$src/go.mod")"
have="$(go env GOVERSION)"
echo "go $have (go.mod asks for $want), $goos/$goarch"

tags="$(grep -v '^[[:space:]]*#' "$src/build-tags.txt" | grep -v '^[[:space:]]*$' | paste -sd, -)"

mkdir -p "$(dirname "$out")"
(
  cd "$src"
  GOTOOLCHAIN=local CGO_ENABLED=0 GOOS="$goos" GOARCH="$goarch" GOFLAGS=-mod=readonly \
    go build -trimpath -tags "$tags" -ldflags "-s -w -buildid=" -o "$out" .
)
ls -l "$out"
