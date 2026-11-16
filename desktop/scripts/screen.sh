#!/usr/bin/env bash
# Snapshot one screen alone, from one fixture, and dump its element tree. No
# daemon. This is what a thread writing a screen runs after every edit.
#
#   desktop/scripts/screen.sh SCREEN [FIXTURE] [--out file.png] [--tree file.json]
#                             [--script s.json] [--at T] [--size WxH] [--file path.luau]
#
# SCREEN is a built-in name (sandboxes, topbar) or any name with --file.
# FIXTURE is a name under fixtures/SCREEN/ (default: basic) or a JSON file.
# --file runs that Luau file as SCREEN (a screen being written, not yet built
# in). Out: $SBXW_SHOTS/screen-SCREEN-FIXTURE.png and .json (the --tree dump),
# plus the app log beside them; screen errors are printed.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
screen="${1:?screen name}"; shift
fixture=basic
if [ $# -gt 0 ] && [ "${1#--}" = "$1" ]; then fixture="$1"; shift; fi
out=""; tree=""; script=""; at=""; size="${SHOT_SIZE:-360x720}"; file=""
while [ $# -gt 0 ]; do
  case "$1" in
    --out) out="$2"; shift 2 ;;
    --tree) tree="$2"; shift 2 ;;
    --script) script="$2"; shift 2 ;;
    --at) at="$2"; shift 2 ;;
    --size) size="$2"; shift 2 ;;
    --file) file="$2"; shift 2 ;;
    *) echo "unknown argument $1" >&2; exit 2 ;;
  esac
done
fname=$(basename "$fixture" .json)
out="${out:-$shots_root/screen-$screen-$fname.png}"
tree="${tree:-${out%.png}.json}"
mkdir -p "$(dirname "$out")"
case "$fixture" in */*|*.json) fx="$fixture" ;; *) fx="$here/fixtures/$screen/$fixture.json" ;; esac
[ -f "$fx" ] || { echo "no fixture $fx" >&2; exit 2; }
if [ -n "$file" ]; then
  dir=$(mktemp -d); cp "$file" "$dir/$screen.luau"; export SBXW_DESKTOP_SCREENS="$dir"
fi
build
args=(--fixture "$fx" --solo "$screen" --snap "$out" --tree "$tree")
[ -n "$script" ] && args+=(--script "$script") && : "${at:=mark:end+0.5}"
args+=(--at "${at:-0.5}")
SHOT_SIZE="$size" run_app "${out%.png}.log" "${args[@]}"
grep -h "screen $screen" "${out%.png}.log" | sed 's/^\[[^]]*\] //' || true
echo "$out"
echo "$tree"
