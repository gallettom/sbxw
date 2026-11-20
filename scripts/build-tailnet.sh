#!/usr/bin/env bash
# Builds sbxw-tailnet, the Tailscale node of remote access (tailnet/README.md).
#
#   scripts/build-tailnet.sh [GOOS/GOARCH] [OUT]
#
# GOOS/GOARCH defaults to this machine (darwin/arm64 on an Apple-silicon Mac).
# OUT defaults to target/tailnet/<goos>-<goarch>/sbxw-tailnet. The daemon looks
# for the binary next to its own executable; the last lines printed say where
# that is and the `cp` that puts it there (this script never copies by itself).
# `sbxw remote status` names the exact path the daemon looks at.
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

# Where the daemon will look: next to the `sbxw` on PATH. On Linux the daemon
# sees its executable with links resolved; on macOS as it was started.
if [ "$goos/$goarch" != "$(go env GOHOSTOS)/$(go env GOHOSTARCH)" ]; then
  echo "Built for another machine: on that $goos/$goarch machine, put it next to its sbxw binary."
elif sbxw="$(command -v sbxw 2>/dev/null)" && [ -n "$sbxw" ]; then
  if [ "$(uname -s)" != Darwin ]; then sbxw="$(readlink -f "$sbxw" 2>/dev/null || echo "$sbxw")"; fi
  dir="$(cd "$(dirname "$sbxw")" && pwd)"
  if [ "$out" = "$dir/sbxw-tailnet" ]; then
    echo "sbxw looks for it at $dir/sbxw-tailnet: already in place."
  else
    echo "sbxw looks for it next to the sbxw on your PATH, in $dir. Put it there:"
    printf '  cp %q %q/\n' "$out" "$dir"
  fi
else
  echo "sbxw looks for it next to your sbxw binary (none on PATH). Put it there:"
  printf '  cp %q <the folder of your sbxw binary>/\n' "$out"
fi
