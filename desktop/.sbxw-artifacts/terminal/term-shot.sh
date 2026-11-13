#!/usr/bin/env bash
# Headless shots of the terminal alone (src/bin/term-demo.rs) against the
# stand-in daemon, through Rune's own flags.
#
#   desktop/.sbxw-artifacts/terminal/term-shot.sh <out.png> --script scripts/headless/term-x.json [--at T] [demo args...]
#
# Writes <out>.txt (the terminal's text, size, scroll, selection, events)
# beside the PNG. Uses scripts/lib.sh to build and start the stand-in.
set -euo pipefail
here_t=$(cd "$(dirname "$0")" && pwd)
source "$here_t/../../scripts/lib.sh"
out="$1"; shift
script=""; at=""; rest=()
while [ $# -gt 0 ]; do
  case "$1" in
    --script) script="$2"; shift 2 ;;
    --at) at="$2"; shift 2 ;;
    *) rest+=("$1"); shift ;;
  esac
done
mkdir -p "$(dirname "$out")"
start_daemon "$(dirname "$out")"
args=(--snap "$out" --at "${at:-mark:end+0.4}" --size "${SHOT_SIZE:-960x640}" --scale "${SHOT_SCALE:-1}")
[ -n "$script" ] && args+=(--script "$script")
env -u DISPLAY -u WAYLAND_DISPLAY RUST_LOG="${RUST_LOG:-info,wgpu=warn,naga=warn}" "$bin/term-demo" --daemon "$daemon" --token "$token" --sandbox web-app \
  --dump-text "${out%.png}.txt" "${args[@]}" "${rest[@]}" >"${out%.png}.log" 2>&1 \
  || { echo "term-demo failed; see ${out%.png}.log" >&2; tail -5 "${out%.png}.log" >&2; exit 1; }
echo "$out"
