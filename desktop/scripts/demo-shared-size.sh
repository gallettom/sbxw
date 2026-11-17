#!/usr/bin/env bash
# One PTY, two clients, headless: the app's terminal on the stand-in, then
# "another window" (the stand-in's simulated second client, as the web UI on
# the same sandbox) takes the PTY's size (70x16) and draws a ruler as wide as
# its own; the app's grid takes the PTY's size, top-left, and says so; then
# typing in the app takes the size back. Three shots, one per state:
#   $SBXW_SHOTS/shared-size-1-owner.png   the app sets the size
#   $SBXW_SHOTS/shared-size-2-other.png   another window set 70x16
#   $SBXW_SHOTS/shared-size-3-back.png    typed in the app: its size again
# Then the other window larger than the pane: the app draws the whole grid
# scaled down to fit, then much larger: the floor (half size), cut, and the
# status line says so; then typing takes the size back:
#   $SBXW_SHOTS/shared-size-4-scaled.png        another window at 160x50
#   $SBXW_SHOTS/shared-size-5-floor.png         another window at 300x90
#   $SBXW_SHOTS/shared-size-6-scaled-back.png   typed in the app after 160x50
# OTHER=WxH changes the first other size; and the app's log lines about sizes.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root"; mkdir -p "$shots"
script="$shots/shared-size-script.json"
cat >"$script" <<JSON
{"events": [
  {"t": 0.3, "type": "type", "every": 0.02, "text": "stty size"},
  {"t": 0.6, "type": "key", "name": "Enter"},
  {"t": 1.0, "type": "mark", "name": "hold:2500"},
  {"t": 1.5, "type": "type", "every": 0.02, "text": "stty size"},
  {"t": 1.8, "type": "key", "name": "Enter"},
  {"t": 2.3, "type": "mark", "name": "end"}
]}
JSON
start_daemon "$shots"
other() {
  local ocols="${1%x*}" orows="${1#*x}"
  curl -s -X POST -H "Authorization: Bearer $token" -H 'Content-Type: application/json' \
    --data "{\"cols\":$ocols,\"rows\":$orows,\"type\":\"clear; stty size; printf '%0${ocols}d\\\\n' 0 | tr 0 =; echo drawn by another window at $ocols columns; seq 4 $((orows - 1))\\n\"}" \
    "http://$daemon/api/standin/other-client" >/dev/null
}
for shot in "1-owner 0.9 ${OTHER:-70x16}" "2-other 1.1 ${OTHER:-70x16}" "3-back mark:end+0.3 ${OTHER:-70x16}" \
            "4-scaled 1.1 160x50" "5-floor 1.1 300x90" "6-scaled-back mark:end+0.3 160x50"; do
  set -- $shot
  log="$shots/app-shared-size-$1.log"
  run_app "$log" --sandbox web-app --script "$script" --snap "$shots/shared-size-$1.png" --at "$2" &
  app=$!
  for _ in $(seq 300); do grep -q "hold 2500 ms" "$log" 2>/dev/null && break; sleep 0.05; done
  sleep 0.5
  other "$3"
  wait $app
  echo "$shots/shared-size-$1.png"
done
for l in 3-back 6-scaled-back; do
  grep -hE "pty size|terminal wants|takes the size" "$shots/app-shared-size-$l.log" | sed 's/^\[[^]]*\] //'
done
