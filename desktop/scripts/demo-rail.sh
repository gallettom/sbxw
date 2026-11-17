#!/usr/bin/env bash
# The left rail, headless, from fixtures/shell/panebar-4.json (agents'
# requests and sbx's requests waiting, a plan waiting). The fixture's plan
# dialog and Requests panel are closed first. Each with its tree (.json):
#   rail-<size>-badges.png     the rail with its badges (Requests 3, Network)
#   rail-<size>-network.png    Network opened from the rail
#   rail-<size>-jarvis.png     Jarvis opened from the rail
#   rail-<size>-nosidebar.png  the sidebar hidden (Cmd/Ctrl+B): the rail stays
#   rail-<size>-fullscreen.png no window buttons (full screen): the sbxw mark lines up with the sidebar's content
# for <size> 1284x816 and 720x480. MARKS="<name> ..." runs some.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root"; mkdir -p "$shots"
build
fixture="$here/fixtures/shell/panebar-4.json"
pre='{"t": 0.5, "type": "mark", "name": "wait:plan-x"}, {"t": 0.7, "type": "key", "name": "Escape"}, {"t": 0.8, "type": "mark", "name": "hold:300"}, {"t": 0.9, "type": "mark", "name": "wait:panel-close-relay:1500"}, {"t": 0.95, "type": "tap", "key": "panel-close-relay"}, {"t": 1.0, "type": "mark", "name": "hold:300"}'
run() { # name size events [env]
  local name="$1" size="$2" body="$3"
  case " ${MARKS:-$name} " in *" $name "*) ;; *) return 0 ;; esac
  printf '{"events": [%s%s, {"t": 9.0, "type": "mark", "name": "end"}]}\n' "$pre" "${body:+, $body}" | python3 -c '
import json, sys
d = json.load(sys.stdin)
ev = sorted(d["events"], key=lambda e: e["t"])
last = max(e["t"] for e in ev if e.get("name") != "end")
for e in ev:
    if e.get("name") == "end": e["t"] = last + 0.1
json.dump({"events": ev}, sys.stdout)' >"$shots/script-$name.json"
  SHOT_SIZE="$size" run_app "$shots/app-$name.log" --fixture "$fixture" --script "$shots/script-$name.json" \
    --snap "$shots/$name.png" --at mark:end+0.5 --tree "$shots/$name.json"
  grep -hE "gave up|script:" "$shots/app-$name.log" | grep -v panel-close-relay | sed 's/^\[[^]]*\] //' || true
  echo "$shots/$name.png"
}
for size in 1284x816 720x480; do
  run "rail-$size-badges" "$size" ""
  run "rail-$size-network" "$size" '{"t": 1.2, "type": "tap", "key": "rail-network"}, {"t": 1.4, "type": "mark", "name": "hold:300"}'
  run "rail-$size-jarvis" "$size" '{"t": 1.2, "type": "tap", "key": "jarvis-toggle"}, {"t": 1.4, "type": "mark", "name": "hold:300"}'
  run "rail-$size-nosidebar" "$size" '{"t": 1.2, "type": "key", "name": "b", "cmd": true}, {"t": 1.4, "type": "mark", "name": "hold:400"}'
  SBXW_DESKTOP_NO_LIGHTS=1 run "rail-$size-fullscreen" "$size" ""
done
