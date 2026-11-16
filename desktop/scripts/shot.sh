#!/usr/bin/env bash
# One command to see the whole window: build, run headless, save a PNG (and
# the element tree).
#
#   desktop/scripts/shot.sh [out.png] [--fixture NAME|FILE] [--sandbox NAME] [--type "text"]
#                           [--script s.json] [--at T] [--tree tree.json] [--chrome]
#
# Without --fixture: against the stand-in daemon (or SBXW_DAEMON + SBXW_TOKEN),
# with a bash terminal on --sandbox (default web-app; `--sandbox ''` for none).
# --fixture NAME reads fixtures/shell/NAME.json (or any fixture file): no daemon.
# --type types the text then Enter; --script replays a Rune script (taps on
# element keys, keys, typing, resizes; see scripts/headless/*.json). The PNG is
# taken at --at seconds (default: half a second after the script's `end`
# mark, or 1.5 s). --chrome draws macOS's window buttons and prints any element
# under them. SHOT_SIZE (1280x800) and SHOT_SCALE (1) set the output.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
out="$shots_root/shot.png"; type_text=""; script=""; at=""; tree=""; fixture=""; sandbox="${SBXW_SANDBOX-web-app}"; extra=()
while [ $# -gt 0 ]; do
  case "$1" in
    --type) type_text="$2"; shift 2 ;;
    --script) script="$2"; shift 2 ;;
    --at) at="$2"; shift 2 ;;
    --tree) tree="$2"; shift 2 ;;
    --fixture) fixture="$2"; shift 2 ;;
    --sandbox) sandbox="$2"; shift 2 ;;
    --chrome) extra+=(--chrome); shift ;;
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
args=(--snap "$out" "${extra[@]}")
[ -n "$script" ] && args+=(--script "$script") && : "${at:=mark:end+0.5}"
args+=(--at "${at:-1.5}")
[ -n "$tree" ] && args+=(--tree "$tree")
if [ -n "$fixture" ]; then
  case "$fixture" in */*|*.json) ;; *) fixture="$here/fixtures/shell/$fixture.json" ;; esac
  build
  args+=(--fixture "$fixture")
else
  start_daemon "$(dirname "$out")"
  [ -n "$sandbox" ] && args+=(--sandbox "$sandbox")
fi
run_app "$(dirname "$out")/app.log" "${args[@]}"
grep -h "^rune: chrome\|overlap" "$(dirname "$out")/app.log" || true
echo "$out"
[ -n "$tree" ] && echo "$tree"
exit 0
