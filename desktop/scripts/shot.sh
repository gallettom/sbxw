#!/usr/bin/env bash
# One command an agent can run to see what it built: build the app, run it
# headless against the stand-in daemon, save a PNG (and the element tree).
#
#   desktop/scripts/shot.sh [out.png] [--type "text"] [--script s.json] [--at T] [--tree tree.json]
#
# --type types the text then Enter (shifted characters included); --script
# replays a Rune script (taps on element keys, keys, typing, resizes; see
# scripts/headless/*.json). The PNG is taken at --at seconds (default: half a
# second after the script's `end` mark, or 1.5 s). SBXW_DAEMON + SBXW_TOKEN use
# a real daemon; SBXW_SANDBOX picks the terminal's sandbox; SBXW_SCREEN another
# screen file; SHOT_SIZE (1280x800) and SHOT_SCALE (1) the output.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
out="$here/shots/shot.png"; type_text=""; script=""; at=""; tree=""
while [ $# -gt 0 ]; do
  case "$1" in
    --type) type_text="$2"; shift 2 ;;
    --script) script="$2"; shift 2 ;;
    --at) at="$2"; shift 2 ;;
    --tree) tree="$2"; shift 2 ;;
    *) out="$1"; shift ;;
  esac
done
mkdir -p "$(dirname "$out")"
if [ -n "$type_text" ]; then
  script=$(mktemp --suffix=.json)
  python3 - "$type_text" >"$script" <<'PY'
import json, sys
text = sys.argv[1]
every = 0.02
print(json.dumps({"events": [
  {"t": 0.3, "type": "type", "every": every, "text": text},
  {"t": 0.3 + every * len(text) + 0.2, "type": "key", "name": "Enter"},
  {"t": 0.3 + every * len(text) + 0.6, "type": "mark", "name": "end"},
]}))
PY
fi
args=(--snap "$out")
[ -n "$script" ] && args+=(--script "$script") && : "${at:=mark:end+0.5}"
args+=(--at "${at:-1.5}")
[ -n "$tree" ] && args+=(--tree "$tree")
[ -n "${SBXW_SCREEN:-}" ] && args+=(--screen "$SBXW_SCREEN")
start_daemon "$(dirname "$out")"
run_app "$(dirname "$out")/app.log" "${args[@]}"
echo "$out"
[ -n "$tree" ] && echo "$tree"
exit 0
