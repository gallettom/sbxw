#!/usr/bin/env bash
# One PTY, two clients, headless: the app's terminal on the stand-in, then
# "another window" (the stand-in's simulated second client, as the web UI on
# the same sandbox) takes the PTY's size (70x16) and draws a ruler as wide as
# its own; the app's grid takes the PTY's size, top-left, and says so; then
# typing in the app takes the size back. Three shots, one per state:
#   $SBXW_SHOTS/shared-size-1-owner.png   the app sets the size
#   $SBXW_SHOTS/shared-size-2-other.png   another window set 70x16
#   $SBXW_SHOTS/shared-size-3-back.png    typed in the app: its size again
# OTHER=200x60 makes the other window larger than the pane (clipped).
# and the app's log lines about sizes.
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
other_size="${OTHER:-70x16}"; ocols="${other_size%x*}"; orows="${other_size#*x}"
other() {
  curl -s -X POST -H "Authorization: Bearer $token" -H 'Content-Type: application/json' \
    --data "{\"cols\":$ocols,\"rows\":$orows,\"type\":\"clear; stty size; printf '%0${ocols}d\\\\n' 0 | tr 0 =; echo drawn by another window at $ocols columns\\n\"}" \
    "http://$daemon/api/standin/other-client" >/dev/null
}
for shot in "1-owner 0.9" "2-other 1.1" "3-back mark:end+0.3"; do
  set -- $shot
  log="$shots/app-shared-size-$1.log"
  run_app "$log" --sandbox web-app --script "$script" --snap "$shots/shared-size-$1.png" --at "$2" &
  app=$!
  for _ in $(seq 300); do grep -q "hold 2500 ms" "$log" 2>/dev/null && break; sleep 0.05; done
  sleep 0.5
  other
  wait $app
  echo "$shots/shared-size-$1.png"
done
grep -hE "pty size|terminal wants|takes the size" "$shots/app-shared-size-3-back.log" | sed 's/^\[[^]]*\] //'
